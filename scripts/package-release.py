#!/usr/bin/env python3
"""Prepare and verify local release artifacts; never publish or submit to an index."""
import argparse
import gzip
import hashlib
import io
import json
import os
from pathlib import Path
import platform
import re
import shutil
import struct
import subprocess
import sys
import tarfile
import tempfile
import tomllib

ROOT = Path(__file__).resolve().parents[1]
BINARY = "kubectl-view_storage"
TARGETS = {
    "x86_64-unknown-linux-musl": ("linux", "amd64", 62),
    "aarch64-unknown-linux-musl": ("linux", "arm64", 183),
    "x86_64-apple-darwin": ("darwin", "amd64", 0x01000007),
    "aarch64-apple-darwin": ("darwin", "arm64", 0x0100000C),
}
# Header layouts/identities: Linux include/uapi/linux/{elf,elf-em}.h and
# LLVM llvm/include/llvm/BinaryFormat/MachO.h. Validate only the declared format,
# target and static Linux requirements; native runtime checks remain separate.
MAX_BINARY = 128 * 1024 * 1024


def require(condition, message):
    if not condition:
        raise ValueError(message)


def version():
    selected = tomllib.loads((ROOT / "Cargo.toml").read_text())["package"]["version"]
    require(re.fullmatch(r"\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?", selected), "invalid Cargo package version")
    return selected


def archive_name(target, selected):
    operating_system, arch, _ = TARGETS[target]
    return f"kubectl-view-storage_v{selected}_{operating_system}_{arch}.tar.gz"


def inspect_binary(data, target):
    """Check executable format/architecture, not source provenance or runtime behavior."""
    require(0 < len(data) <= MAX_BINARY, "binary is empty or exceeds 128 MiB")
    operating_system, _, machine = TARGETS[target]
    if operating_system == "darwin":
        require(len(data) >= 32, "truncated Mach-O header")
        magic, cpu, _, kind, commands, command_bytes, _, _ = struct.unpack_from("<8I", data)
        require(magic == 0xFEEDFACF and cpu == machine and kind == 2, "expected a thin 64-bit Mach-O executable for the selected target")
        require(commands > 0 and 0 < command_bytes <= len(data) - 32, "missing/truncated Mach-O load commands")
    else:
        require(len(data) >= 64 and data[:7] == b"\x7fELF\x02\x01\x01", "expected a little-endian ELF64 executable")
        kind, cpu, elf_version, _, phoff, _, _, ehsize, phsize, phnum, _, _, _ = struct.unpack_from("<HHIQQQIHHHHHH", data, 16)
        require(kind in (2, 3) and cpu == machine and elf_version == 1 and ehsize == 64, "ELF architecture/type differs from the selected target")
        require(phsize == 56 and 0 < phnum <= 4096 and 64 <= phoff <= len(data) - phsize * phnum, "invalid ELF program header table")
        for index in range(phnum):
            segment, _, offset, _, _, size, _, _ = struct.unpack_from("<IIQQQQQQ", data, phoff + index * phsize)
            require(segment != 3, "Linux release binary requires a dynamic interpreter; build the musl target statically")
            require(offset <= len(data) and size <= len(data) - offset, "truncated ELF segment")
            if segment == 2:  # Static PIE may have relocations, but no DT_NEEDED libraries.
                require(size % 16 == 0, "invalid ELF dynamic table")
                for position in range(offset, offset + size, 16):
                    tag, _ = struct.unpack_from("<QQ", data, position)
                    if tag == 0:
                        break
                    require(tag != 1, "Linux release binary has a dynamic library dependency")


def pack(binary, target, directory, selected):
    binary = Path(binary)
    require(binary.is_file() and os.access(binary, os.X_OK), "select an executable release binary")
    require(binary.stat().st_size <= MAX_BINARY, "binary exceeds 128 MiB")
    data = binary.read_bytes()
    inspect_binary(data, target)
    directory.mkdir(parents=True, exist_ok=True)
    destination = directory / archive_name(target, selected)
    require(not destination.exists(), f"refusing to replace {destination.name}")
    # Fixed metadata and gzip time make archives reproducible from identical bytes.
    with tempfile.NamedTemporaryFile(dir=directory, prefix=".archive-", delete=False) as output:
        temporary = Path(output.name)
        try:
            with gzip.GzipFile(fileobj=output, filename="", mode="wb", mtime=0) as compressed:
                with tarfile.open(fileobj=compressed, mode="w", format=tarfile.USTAR_FORMAT) as archive:
                    for name, payload, mode in [(BINARY, data, 0o755), ("LICENSE", (ROOT / "LICENSE").read_bytes(), 0o644)]:
                        entry = tarfile.TarInfo(name)
                        entry.size, entry.mode, entry.mtime = len(payload), mode, 0
                        archive.addfile(entry, io.BytesIO(payload))
            output.flush()
            temporary.chmod(0o644)
            temporary.rename(destination)
        finally:
            temporary.unlink(missing_ok=True)
    return inspect_archive(destination, target, selected)


def inspect_archive(path, target, selected):
    require(path.name == archive_name(target, selected), "archive filename differs from its target/version")
    require(path.is_file() and path.stat().st_size <= MAX_BINARY + 1024 * 1024, "archive is absent or oversized")
    entries = {}
    with tarfile.open(path, "r:gz") as archive:
        for entry in archive:
            require(entry.name in (BINARY, "LICENSE") and entry.name not in entries and entry.isfile(), "unexpected, duplicate or linked archive entry")
            expected_mode, limit = (0o755, MAX_BINARY) if entry.name == BINARY else (0o644, 128 * 1024)
            require(entry.mode == expected_mode and 0 < entry.size <= limit, "invalid archived file mode/size")
            payload = archive.extractfile(entry).read(limit + 1)
            require(len(payload) == entry.size, "truncated archived file")
            entries[entry.name] = payload
    require(set(entries) == {BINARY, "LICENSE"}, "archive must contain exactly the canonical binary and LICENSE")
    require(entries["LICENSE"] == (ROOT / "LICENSE").read_bytes(), "archive license differs from the project license")
    inspect_binary(entries[BINARY], target)
    operating_system, arch, _ = TARGETS[target]
    with path.open("rb") as source:
        checksum = hashlib.file_digest(source, "sha256").hexdigest()
    return {"target": target, "os": operating_system, "arch": arch, "archive": path.name,
            "sha256": checksum,
            "binary_sha256": hashlib.sha256(entries[BINARY]).hexdigest()}


def repository_url(url):
    url = url.rstrip("/")
    require(re.fullmatch(r"https://github\.com/[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+", url), "select an HTTPS GitHub owner/repository URL")
    return url


def expected_manifest(repository, selected, artifacts):
    manifest = json.loads((ROOT / ".krew.yaml").read_text())
    manifest["spec"].update(version="v" + selected, homepage=repository_url(repository))
    manifest["spec"]["platforms"] = [{"selector": {"matchLabels": {"os": artifact["os"], "arch": artifact["arch"]}},
        "uri": f"{repository_url(repository)}/releases/download/v{selected}/{artifact['archive']}",
        "sha256": artifact["sha256"], "bin": BINARY} for artifact in artifacts]
    return manifest


def inspect_all(directory, selected):
    return [inspect_archive(directory / archive_name(target, selected), target, selected) for target in TARGETS]


def checksums(artifacts):
    return "".join(f"{artifact['sha256']}  {artifact['archive']}\n" for artifact in artifacts)


def generate(directory, repository, selected):
    artifacts = inspect_all(directory, selected)
    manifest = expected_manifest(repository, selected, artifacts)
    # JSON is a YAML subset: avoid adding a YAML dependency for release tooling.
    (directory / "view-storage.yaml").write_text(json.dumps(manifest, indent=2) + "\n")
    (directory / "SHA256SUMS").write_text(checksums(artifacts))
    return verify(directory, selected)


def verify(directory, selected):
    artifacts = inspect_all(directory, selected)
    manifest = json.loads((directory / "view-storage.yaml").read_text())
    expected = expected_manifest(manifest["spec"]["homepage"], selected, artifacts)
    require(manifest == expected, "Krew manifest differs from the actual target archives/version")
    require((directory / "SHA256SUMS").read_text() == checksums(artifacts), "SHA256SUMS differs from the actual archives")
    return artifacts


def smoke(directory, selected, krew=None, kubectl=None, download=False):
    record = directory / "krew-smoke.json"
    record.write_text(json.dumps({"status": "running", "version": selected}) + "\n")
    try:
        artifacts = verify(directory, selected)
        operating_system = {"Linux": "linux", "Darwin": "darwin"}.get(platform.system())
        arch = {"x86_64": "amd64", "amd64": "amd64", "aarch64": "arm64", "arm64": "arm64"}.get(platform.machine().lower())
        native = next((artifact for artifact in artifacts if (artifact["os"], artifact["arch"]) == (operating_system, arch)), None)
        require(native is not None, "host is outside the four release platforms")
        krew, kubectl = krew or shutil.which("kubectl-krew"), kubectl or shutil.which("kubectl")
        require(krew and kubectl, "install Krew and kubectl before the actual local-install gate")
        krew, kubectl = str(Path(krew).resolve()), str(Path(kubectl).resolve())
        with tempfile.TemporaryDirectory(prefix="view-storage-krew-") as temporary:
            owned = Path(temporary)
            env = dict(os.environ, KREW_ROOT=temporary, KREW_NO_UPGRADE_CHECK="1", KUBECONFIG=str(owned / "absent-config"),
                       PATH=str(owned / "bin") + os.pathsep + os.environ.get("PATH", ""), NO_COLOR="1")
            for key in ("KREW_OS", "KREW_ARCH"):
                env.pop(key, None)
            command = [krew, "install", "--manifest=" + str((directory / "view-storage.yaml").resolve())]
            if not download:
                command.append("--archive=" + str((directory / native["archive"]).resolve()))
            subprocess.run(command, env=env, check=True, capture_output=True, text=True, timeout=90)
            installed = owned / "bin" / BINARY
            require(installed.is_file() and os.access(installed, os.X_OK) and installed.resolve().is_relative_to(owned.resolve()), "Krew did not install the canonical plugin in the owned root")
            require(hashlib.sha256(installed.read_bytes()).hexdigest() == native["binary_sha256"], "installed binary differs from its archive")
            installed_license = installed.resolve().parent / "LICENSE"
            require(installed_license.is_file() and installed_license.resolve().is_relative_to(owned.resolve())
                    and installed_license.read_bytes() == (ROOT / "LICENSE").read_bytes(),
                    "Krew did not preserve the project LICENSE in the owned installation")
            for flag in ("--help", "--version"):
                result = subprocess.run([kubectl, "view-storage", flag], env=env, check=True, capture_output=True, text=True, timeout=5)
                require(not result.stderr, "installed help/version wrote to stderr")
                require(result.stdout == f"kubectl-view-storage {selected}\n" if flag == "--version" else "--require-metrics" in result.stdout, "installed help/version differs from the package contract")
            # Inherit the controlled fixtures' output so CI retains the last
            # completed stage on a child failure or watchdog timeout.
            subprocess.run([sys.executable,str(ROOT / "scripts/check-plugin.py"),"--binary",str(installed),"--kubectl",kubectl],env=env,check=True,text=True,timeout=30)
            subprocess.run([sys.executable,str(ROOT / "scripts/test-credential-terminal.py"),"--binary",str(installed),"--evidence-directory",str(directory / "installed-authentication")],env=env,check=True,text=True,timeout=30)
        record.write_text(json.dumps({"status": "passed", "version": selected, "artifact": native,
            "installation_source": "manifest-uri" if download else "local-archive", "license_verified": True}, indent=2) + "\n")
    except BaseException as error:
        record.write_text(json.dumps({"status": "failed", "version": selected, "reason": str(error)}) + "\n")
        raise


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    archive = commands.add_parser("archive")
    archive.add_argument("--target", choices=TARGETS, required=True)
    archive.add_argument("--binary", type=Path, required=True)
    manifest = commands.add_parser("manifest")
    manifest.add_argument("--repository-url", required=True)
    commands.add_parser("verify")
    install = commands.add_parser("smoke")
    install.add_argument("--krew", type=Path)
    install.add_argument("--kubectl", type=Path)
    install.add_argument("--download", action="store_true", help="install from the manifest URI instead of overriding with the local archive; requires published assets")
    for command in commands.choices.values():
        command.add_argument("--directory", type=Path, required=True)
    args = parser.parse_args()
    try:
        selected = version()
        if args.command == "archive":
            pack(args.binary, args.target, args.directory, selected)
        elif args.command == "manifest":
            generate(args.directory, args.repository_url, selected)
        elif args.command == "verify":
            verify(args.directory, selected)
        else:
            smoke(args.directory, selected, args.krew, args.kubectl, args.download)
    except (OSError, ValueError, KeyError, tarfile.TarError, struct.error, subprocess.SubprocessError) as error:
        parser.exit(1, f"FAIL release artifacts: {error}\n")
    print(f"PASS local {args.command} checks for v{selected}; publication and cross-platform runtime evidence are separate")


if __name__ == "__main__":
    main()
