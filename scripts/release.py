#!/usr/bin/env python3
"""Makefile release commands; reuse the native workflow and archive verifier."""
import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import re
import shlex
import subprocess
import tempfile
import time
import tomllib
import uuid

ROOT = Path(__file__).resolve().parents[1]
WORKFLOW = "native-distribution.yml"
WORKFLOW_PATH = ".github/workflows/" + WORKFLOW
PACKAGE = "kubectl-view-storage"
spec = importlib.util.spec_from_file_location("release_packaging", ROOT / "scripts/package-release.py")
packaging = importlib.util.module_from_spec(spec)
spec.loader.exec_module(packaging)


def require(condition, message):
    if not condition:
        raise ValueError(message)


def version(value):
    require(re.fullmatch(r"(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)(?:-[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?", value),
            "Set VERSION to a semantic version without v, for example: make release-prepare VERSION=0.1.0")
    if "-" in value:
        for part in value.split("-", 1)[1].split("."):
            require(not (part.isdecimal() and len(part) > 1 and part.startswith("0")), "Numeric prerelease identifiers cannot have leading zeros")
    return value


def run(arguments, *, capture=False, check=True, timeout=120):
    print("+ " + shlex.join(str(argument) for argument in arguments), flush=True)
    return subprocess.run(arguments, cwd=ROOT, check=check, text=True,
                          stdout=subprocess.PIPE if capture else None, timeout=timeout)


def lock_version(path):
    entries = tomllib.loads(path.read_text())["package"]
    own = [item for item in entries if item["name"] == PACKAGE and "source" not in item]
    require(len(own) == 1, f"{path}: expected one local {PACKAGE} entry")
    return own[0]["version"]


def check_versions(selected):
    require(tomllib.loads((ROOT / "Cargo.toml").read_text())["package"]["version"] == selected,
            f"Cargo.toml does not select {selected}; run make release-prepare VERSION={selected}")
    for name in ("Cargo.lock", "fuzz/Cargo.lock"):
        require(lock_version(ROOT / name) == selected, f"{name} version differs; run make release-prepare VERSION={selected}")


def prepare(selected):
    paths = [ROOT / name for name in ("Cargo.toml", "Cargo.lock", "fuzz/Cargo.lock")]
    paths.extend(path for path in (ROOT / "completions").iterdir() if path.is_file())
    originals = {path: path.read_bytes() for path in paths}
    try:
        manifest = paths[0].read_text()
        current = tomllib.loads(manifest)["package"]["version"]
        old = f'version = "{current}"'
        require(old in manifest, "Cannot locate the package version assignment in Cargo.toml")
        paths[0].write_text(manifest.replace(old, f'version = "{selected}"', 1))
        # Resolution is intentionally unlocked only while changing the local
        # package version. Keep all third-party versions/features unchanged.
        for manifest_path in ("Cargo.toml", "fuzz/Cargo.toml"):
            run(["cargo", "metadata", "--offline", "--manifest-path", manifest_path, "--format-version", "1"], capture=True)
        for path in paths[1:3]:
            before = tomllib.loads(originals[path].decode())
            after = tomllib.loads(path.read_text())
            for document in (before, after):
                for item in document["package"]:
                    if item["name"] == PACKAGE and "source" not in item:
                        item["version"] = selected
            require(before == after, f"{path}: resolution changed beyond the local package version; review dependencies separately")
        check_versions(selected)
        run(["cargo", "run", "--locked", "--offline", "--example", "completions"])
    except BaseException:
        for path, content in originals.items():
            path.write_bytes(content)
        raise
    print(f"Prepared {selected}. Run make verify and make coverage, then review, commit and push the release inputs.")


class Release:
    def __init__(self, selected, directory):
        self.version = selected
        self.tag = "v" + selected
        self.directory = directory.resolve()
        check_versions(selected)
        require(not run(["git", "-c", "core.fsmonitor=false", "status", "--porcelain", "--untracked-files=normal"], capture=True).stdout.strip(),
                "Commit the reviewed release inputs first; release commands require a clean checkout")
        self.commit = run(["git", "rev-parse", "HEAD"], capture=True).stdout.strip()
        origin = run(["git", "remote", "get-url", "origin"], capture=True).stdout.strip()
        match = re.fullmatch(r"(?:git@github\.com:|https://github\.com/|ssh://git@github\.com/)([A-Za-z0-9][A-Za-z0-9_.-]*/[A-Za-z0-9][A-Za-z0-9_.-]*?)(?:\.git)?", origin)
        require(match, "origin must identify a GitHub repository over HTTPS or SSH")
        self.repository = match[1]

    def gh(self, arguments, **options):
        return run(["gh", *arguments, "--repo", self.repository], **options)

    def remote_tag(self):
        output = run(["git", "ls-remote", "--tags", "origin", "refs/tags/" + self.tag, "refs/tags/" + self.tag + "^{}"], capture=True).stdout
        refs = dict(line.split()[::-1] for line in output.splitlines())
        return refs.get("refs/tags/" + self.tag + "^{}", refs.get("refs/tags/" + self.tag))

    def ensure_tag(self):
        remote = self.remote_tag()
        require(remote is None or remote == self.commit, f"Remote {self.tag} points to another commit; tags are never moved")
        local = run(["git", "rev-parse", "--verify", "--quiet", "refs/tags/" + self.tag + "^{commit}"], capture=True, check=False)
        require(local.returncode != 0 or local.stdout.strip() == self.commit, f"Local {self.tag} points to another commit; tags are never moved")
        if remote is None:
            if local.returncode:
                run(["git", "tag", "-a", self.tag, "-m", "Release " + self.tag])
            run(["git", "push", "origin", "refs/tags/" + self.tag])

    def run_info(self, identifier, *, complete=False):
        require(str(identifier).isdigit(), "RUN_ID must be a numeric GitHub Actions run ID")
        info = json.loads(run(["gh", "api", f"repos/{self.repository}/actions/runs/{identifier}"], capture=True).stdout)
        require(info["head_sha"] == self.commit and info["head_branch"] == self.tag,
                "Workflow run does not match this release tag and checked-out commit")
        require(info["path"].split("@", 1)[0] == WORKFLOW_PATH and info["event"] == "workflow_dispatch",
                "Select a manually dispatched native-distribution.yml run")
        if complete:
            require(info["status"] == "completed" and info["conclusion"] == "success", "The entire native workflow, including installation checks, must pass")
        return info

    def dispatch(self):
        marker = "release-" + self.tag + "-" + uuid.uuid4().hex
        self.gh(["workflow", "run", WORKFLOW, "--ref", self.tag, "-f", "release_id=" + marker])
        for _ in range(30):
            runs = json.loads(self.gh(["run", "list", "--workflow", WORKFLOW, "--event", "workflow_dispatch", "--branch", self.tag,
                                      "--limit", "30", "--json", "databaseId,displayTitle,headSha"], capture=True).stdout)
            matching = [item for item in runs if item["displayTitle"] == marker and item["headSha"] == self.commit]
            if matching:
                return matching[0]["databaseId"]
            time.sleep(2)
        raise ValueError("Workflow dispatched but its run is not listed yet. Find it in Actions and resume with RUN_ID=<id>")

    def assets(self, directory):
        artifacts = packaging.verify(directory, self.version)
        manifest = json.loads((directory / "view-storage.yaml").read_text())
        require(manifest["spec"]["homepage"] == "https://github.com/" + self.repository, "Release manifest belongs to a different repository")
        return [directory / item["archive"] for item in artifacts] + [directory / "SHA256SUMS", directory / "view-storage.yaml"]

    @staticmethod
    def hashes(paths):
        return {path.name: hashlib.sha256(path.read_bytes()).hexdigest() for path in paths}

    def command_hint(self, target, extra=""):
        return f"make {target} VERSION={self.version} RELEASE_DIRECTORY={shlex.quote(str(self.directory))}{extra}"

    def build(self, identifier=None):
        require(not self.directory.exists(), f"{self.directory} already exists; select a fresh RELEASE_DIRECTORY")
        # Verify access before creating/pushing a tag. Resume never creates tags.
        run(["gh", "repo", "view", self.repository, "--json", "nameWithOwner"], capture=True)
        if identifier is None:
            self.ensure_tag()
            identifier = self.dispatch()
        else:
            require(self.remote_tag() == self.commit, "Remote release tag does not match this commit")
        self.run_info(identifier)
        print("Resume this build with: " + self.command_hint("release", f" RUN_ID={identifier}"), flush=True)
        self.gh(["run", "watch", str(identifier), "--exit-status", "--compact", "--interval", "10"], timeout=7200)
        info = self.run_info(identifier, complete=True)
        self.directory.parent.mkdir(parents=True, exist_ok=True)
        with tempfile.TemporaryDirectory(prefix="release-download-", dir=self.directory.parent) as temporary:
            downloaded = Path(temporary) / "assets"
            self.gh(["run", "download", str(identifier), "--name", "verified-local-release-set", "--dir", str(downloaded)])
            assets = self.assets(downloaded)
            receipt = {"version": self.version, "tag": self.tag, "commit": self.commit, "repository": self.repository,
                       "run_id": int(identifier), "run_attempt": info["run_attempt"], "files": self.hashes(assets)}
            (downloaded / "release-receipt.json").write_text(json.dumps(receipt, indent=2) + "\n")
            require(not self.directory.exists(), f"{self.directory} appeared during the build; select a fresh RELEASE_DIRECTORY")
            downloaded.rename(self.directory)
        print(f"Verified release artifacts: {self.directory}\nPublish with: " + self.command_hint("release-publish"))

    def publish(self, draft=False):
        require(self.remote_tag() == self.commit, "Remote release tag does not match this commit")
        receipt = json.loads((self.directory / "release-receipt.json").read_text())
        for key, expected in {"version": self.version, "tag": self.tag, "commit": self.commit, "repository": self.repository}.items():
            require(receipt[key] == expected, f"Release receipt has a different {key}; download the correct run")
        info = self.run_info(receipt["run_id"], complete=True)
        require(info["run_attempt"] == receipt["run_attempt"], "Workflow was rerun; download and verify its current artifact set")
        assets = self.assets(self.directory)
        require(self.hashes(assets) == receipt["files"], "Release files changed after download; download and verify them again")
        arguments = ["release", "create", self.tag, *(str(path) for path in assets), "--verify-tag", "--title", self.tag]
        notes = ROOT / "docs/releases" / (self.tag + ".md")
        arguments.extend(["--notes-file", str(notes)] if notes.is_file() else ["--generate-notes"])
        if draft:
            arguments.append("--draft")
        if "-" in self.version:
            arguments.append("--prerelease")
        self.gh(arguments)


HELP = """Release commands (Python 3.12+, pinned Cargo, Git, authenticated gh):
  make release-prepare VERSION=0.1.0   Update version, both lockfiles and completions
  make verify                        Run local checks and independent report oracles
  make coverage                      Check production coverage floors
  # Review, commit and push the release inputs before continuing.
  make release VERSION=0.1.0          Tag/push HEAD, build/test all native targets,
                                     wait, download and verify archives/checksums
  make release-publish VERSION=0.1.0  Publish the verified set to GitHub Releases

Optional: RUN_ID=<id> resumes an existing build; DRAFT=1 creates a draft release.
RELEASE_DIRECTORY defaults to artifacts/release-VERSION when VERSION is set.
Use a fresh directory for downloads. Publishing never rebuilds or replaces assets.
The wrapper never commits changes, moves existing tags, or submits a Krew index PR.
"""


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=("help", "prepare", "build", "publish"))
    args = parser.parse_args()
    if args.command == "help":
        print(HELP)
        return 0
    try:
        selected = version(os.environ.get("RELEASE_VERSION", ""))
        if args.command == "prepare":
            prepare(selected)
        else:
            directory = Path(os.environ.get("RELEASE_DIRECTORY") or f"artifacts/release-{selected}")
            release = Release(selected, directory)
            if args.command == "build":
                release.build(os.environ.get("RELEASE_RUN_ID") or None)
            else:
                draft = os.environ.get("RELEASE_DRAFT", "")
                require(draft in ("", "0", "1"), "DRAFT must be 0 or 1")
                release.publish(draft == "1")
    except (OSError, ValueError, KeyError, subprocess.SubprocessError) as error:
        parser.exit(1, f"FAIL release: {error}\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
