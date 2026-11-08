#!/usr/bin/env python3
"""Synthetic archive/header tests; these do not build or run platform binaries/Krew."""
import importlib.util
import io
import json
from pathlib import Path
import struct
import tarfile
import tempfile
import tomllib
import unittest
from unittest import mock

spec = importlib.util.spec_from_file_location("release_packaging", Path(__file__).with_name("package-release.py"))
release = importlib.util.module_from_spec(spec)
spec.loader.exec_module(release)
VERSION = "0.1.0-test"
REPOSITORY = "https://github.com/example/kubectl-view-storage"


def header_fixture(target, segment=1):
    """Minimal executable-shaped bytes for validator tests, never executable code."""
    system, _, machine = release.TARGETS[target]
    if system == "darwin":
        return struct.pack("<8I", 0xFEEDFACF, machine, 0, 2, 1, 24, 0, 0) + struct.pack("<IIQQ", 0x80000028, 24, 0, 0)
    ident = b"\x7fELF\x02\x01\x01" + bytes(9)
    header = ident + struct.pack("<HHIQQQIHHHHHH", 2, machine, 1, 0, 64, 0, 0, 64, 56, 1, 0, 0, 0)
    return header + struct.pack("<IIQQQQQQ", segment, 5, 0, 0, 0, 120, 120, 4096)


class PackagingTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="view-storage-package-unit-")
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.artifacts = self.root / "artifacts"

    def pack(self, target, directory=None):
        source = self.root / target
        source.write_bytes(header_fixture(target))
        source.chmod(0o755)
        return release.pack(source, target, directory or self.artifacts, VERSION)

    def complete_set(self):
        for target in release.TARGETS:
            self.pack(target)
        return release.generate(self.artifacts, REPOSITORY, VERSION)

    def test_cargo_source_allowlist_excludes_nested_runtime_and_upstream_files(self):
        # Exercise Cargo's real gitignore-style matching. Unanchored README.md
        # or LICENSE patterns also match nested, ignored upstream checkouts.
        source = self.root / "source-selection"
        source.mkdir()
        include = tomllib.loads((release.ROOT / "Cargo.toml").read_text())["package"]["include"]
        (source / "Cargo.toml").write_text('[package]\nname="package-selection-probe"\nversion="0.1.0"\nedition="2024"\ninclude=' + json.dumps(include) + '\n')
        expected = {"Cargo.lock", "Cargo.toml", "Cargo.toml.orig", "LICENSE", "README.md", "src/main.rs", "rust-toolchain.toml"}
        for name in expected - {"Cargo.lock", "Cargo.toml", "Cargo.toml.orig"}:
            path = source / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text((release.ROOT / name).read_text() if name == "rust-toolchain.toml" else 'fn main() {}' if name.endswith('.rs') else 'source fixture')
        for name in (".local/kubeconfigs/token.json", ".local/upstream/demo/README.md", ".local/upstream/demo/LICENSE", "artifacts/README.md", "target/README.md"):
            path = source / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text('private canary; never package')
        release.subprocess.run(["cargo", "generate-lockfile", "--offline"], cwd=source, check=True, capture_output=True, text=True, timeout=30)
        result = release.subprocess.run(["cargo", "package", "--list", "--locked", "--offline", "--allow-dirty"], cwd=source, check=True, capture_output=True, text=True, timeout=30)
        files = set(result.stdout.splitlines())
        self.assertEqual(files, expected)

    def test_four_targets_have_distinct_selectors_hashes_and_canonical_layout(self):
        artifacts = self.complete_set()
        self.assertEqual(len({item["sha256"] for item in artifacts}), 4)
        manifest = json.loads((self.artifacts / "view-storage.yaml").read_text())
        selectors = {tuple(sorted(entry["selector"]["matchLabels"].items())) for entry in manifest["spec"]["platforms"]}
        self.assertEqual(len(selectors), 4)
        self.assertEqual(manifest["spec"]["version"], "v" + VERSION)
        for artifact in artifacts:
            self.assertEqual((self.artifacts / artifact["archive"]).stat().st_mode & 0o777, 0o644)
            with tarfile.open(self.artifacts / artifact["archive"]) as archive:
                self.assertEqual(archive.getnames(), [release.BINARY, "LICENSE"])
                self.assertEqual(archive.getmember(release.BINARY).mode, 0o755)
                self.assertEqual(archive.extractfile("LICENSE").read(), (release.ROOT / "LICENSE").read_bytes())

    def test_identical_input_produces_identical_archive_bytes(self):
        target = next(iter(release.TARGETS))
        first = self.pack(target)
        second = self.pack(target, self.root / "second")
        self.assertEqual(first, second)
        self.assertEqual((self.artifacts / first["archive"]).read_bytes(), (self.root / "second" / second["archive"]).read_bytes())

    def test_wrong_architecture_script_and_truncated_binary_are_rejected(self):
        target = "aarch64-unknown-linux-musl"
        for data in (header_fixture("x86_64-unknown-linux-musl"), b"#!/bin/sh\n", b"\x7fELF\x02\x01\x01"):
            with self.subTest(data=data[:20]), self.assertRaises(ValueError):
                release.inspect_binary(data, target)

    def test_linux_dynamic_interpreter_or_needed_library_is_rejected(self):
        target = "x86_64-unknown-linux-musl"
        with self.assertRaisesRegex(ValueError, "dynamic interpreter"):
            release.inspect_binary(header_fixture(target, segment=3), target)
        data = bytearray(header_fixture(target, segment=2))
        # Point the dynamic segment at a DT_NEEDED entry rather than the header.
        struct.pack_into("<Q", data, 64 + 8, len(data))
        struct.pack_into("<Q", data, 64 + 32, 16)
        data.extend(struct.pack("<QQ", 1, 0))
        with self.assertRaisesRegex(ValueError, "dynamic library"):
            release.inspect_binary(data, target)

    def test_existing_archives_are_not_overwritten(self):
        target = next(iter(release.TARGETS))
        self.pack(target)
        with self.assertRaisesRegex(ValueError, "refusing to replace"):
            self.pack(target)

    def test_missing_platform_cannot_generate_a_manifest(self):
        self.pack(next(iter(release.TARGETS)))
        with self.assertRaisesRegex(ValueError, "absent"):
            release.generate(self.artifacts, REPOSITORY, VERSION)
        self.assertFalse((self.artifacts / "view-storage.yaml").exists())

    def test_manifest_tampering_is_rejected(self):
        self.complete_set()
        path = self.artifacts / "view-storage.yaml"
        original = path.read_text()
        for change in ("version", "checksum", "binary", "selector"):
            mutated = json.loads(original)
            if change == "version":
                mutated["spec"]["version"] = "v9.9.9"
            elif change == "checksum":
                mutated["spec"]["platforms"][0]["sha256"] = "0" * 64
            elif change == "binary":
                mutated["spec"]["platforms"][0]["bin"] = "kubectl-view-storage"
            else:
                mutated["spec"]["platforms"][0]["selector"] = mutated["spec"]["platforms"][1]["selector"]
            path.write_text(json.dumps(mutated))
            with self.subTest(change=change), self.assertRaisesRegex(ValueError, "manifest differs"):
                release.verify(self.artifacts, VERSION)

    def test_checksum_file_tampering_is_rejected(self):
        self.complete_set()
        (self.artifacts / "SHA256SUMS").write_text("0" * 64)
        with self.assertRaisesRegex(ValueError, "SHA256SUMS"):
            release.verify(self.artifacts, VERSION)

    def test_link_traversal_extra_file_and_license_tampering_are_rejected(self):
        target = next(iter(release.TARGETS))
        artifact = self.pack(target)
        path = self.artifacts / artifact["archive"]
        for name, kind in (("../outside", tarfile.REGTYPE), (release.BINARY, tarfile.SYMTYPE), ("LICENSE", tarfile.REGTYPE)):
            with tarfile.open(path, "w:gz") as archive:
                entry = tarfile.TarInfo(name)
                entry.mode, entry.type = 0o644, kind
                payload = b"wrong license"
                entry.size = len(payload) if kind == tarfile.REGTYPE else 0
                archive.addfile(entry, io.BytesIO(payload) if entry.size else None)
                if name == "LICENSE":
                    binary = tarfile.TarInfo(release.BINARY)
                    binary.mode, binary.size = 0o755, len(header_fixture(target))
                    archive.addfile(binary, io.BytesIO(header_fixture(target)))
            with self.subTest(name=name, kind=kind), self.assertRaises(ValueError):
                release.inspect_archive(path, target, VERSION)
        self.assertFalse((self.root / "outside").exists())

    def test_missing_krew_invalidates_previous_smoke_success(self):
        self.complete_set()
        record = self.artifacts / "krew-smoke.json"
        record.write_text('{"status":"passed"}')
        with mock.patch.object(release.shutil, "which", return_value=None), self.assertRaisesRegex(ValueError, "install Krew"):
            release.smoke(self.artifacts, VERSION)
        self.assertEqual(json.loads(record.read_text())["status"], "failed")

    def test_smoke_uses_owned_root_and_cleans_up_without_user_configuration(self):
        artifacts = self.complete_set()
        calls, roots = [], []
        native = next(item for item in artifacts if item["target"] == "aarch64-apple-darwin")

        def run(command, **options):
            env = options["env"]
            owned = Path(env["KREW_ROOT"])
            self.assertTrue(env["KUBECONFIG"].startswith(str(owned)))
            self.assertFalse(Path(env["KUBECONFIG"]).exists())
            self.assertTrue(env["PATH"].startswith(str(owned / "bin")))
            self.assertEqual(env["KREW_NO_UPGRADE_CHECK"], "1")
            self.assertNotIn("KREW_OS", env)
            self.assertNotIn("KREW_ARCH", env)
            calls.append(command)
            roots.append(owned)
            if command[1] == "install":
                stored = owned / "store" / "plugin"
                stored.parent.mkdir()
                stored.write_bytes(header_fixture(native["target"]))
                stored.chmod(0o755)
                (owned / "bin").mkdir()
                (owned / "bin" / release.BINARY).symlink_to(stored)
                return subprocess_result("")
            return subprocess_result(f"kubectl-view-storage {VERSION}\n" if command[-1] == "--version" else "--require-metrics")

        with mock.patch.object(release.platform, "system", return_value="Darwin"), mock.patch.object(release.platform, "machine", return_value="arm64"), mock.patch.object(release.subprocess, "run", side_effect=run):
            release.smoke(self.artifacts, VERSION, Path("/mock/krew"), Path("/mock/kubectl"))
        self.assertEqual(len(calls), 5)
        self.assertIn("check-plugin.py",calls[3][1])
        self.assertIn("test-credential-terminal.py",calls[4][1])
        self.assertTrue(all(not path.exists() for path in roots))
        self.assertEqual(json.loads((self.artifacts / "krew-smoke.json").read_text())["status"], "passed")


def subprocess_result(stdout):
    return release.subprocess.CompletedProcess([], 0, stdout=stdout, stderr="")


if __name__ == "__main__":
    unittest.main()
