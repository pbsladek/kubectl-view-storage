#!/usr/bin/env python3
"""Offline release orchestration regressions; never push tags or publish releases."""
import importlib.util
import json
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest
from unittest import mock

ROOT = Path(__file__).resolve().parents[1]


def load(name, filename):
    spec = importlib.util.spec_from_file_location(name, ROOT / "scripts" / filename)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


release = load("release_commands", "release.py")
header_fixture = load("release_archive_fixtures", "test-release-packaging.py").header_fixture
COMMIT = "a" * 40


class ReleaseTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory(prefix="release-commands-")
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        (self.root / "src").mkdir()
        (self.root / "src/lib.rs").write_text("")
        (self.root / "fuzz").mkdir()
        (self.root / "fuzz/src").mkdir()
        (self.root / "fuzz/src/lib.rs").write_text("")
        (self.root / "Cargo.toml").write_text('[package]\nname="kubectl-view-storage"\nversion = "0.1.0" # keep this comment\nedition="2024"\n')
        (self.root / "fuzz/Cargo.toml").write_text('[package]\nname="fuzz-fixture"\nversion="0.0.0"\n[dependencies]\nkubectl-view-storage={path=".."}\n[workspace]\n')
        (self.root / "Cargo.lock").write_text('version=4\n[[package]]\nname="kubectl-view-storage"\nversion="0.1.0"\n')
        (self.root / "fuzz/Cargo.lock").write_text('version=4\n[[package]]\nname="fuzz-fixture"\nversion="0.0.0"\ndependencies=["kubectl-view-storage"]\n[[package]]\nname="kubectl-view-storage"\nversion="0.1.0"\n')
        (self.root / "completions").mkdir()
        (self.root / "completions/fixture.bash").write_text("original completion")
        for name in ("LICENSE", ".krew.yaml"):
            shutil.copy2(ROOT / name, self.root / name)
        self.addCleanup(mock.patch.stopall)
        mock.patch.object(release, "ROOT", self.root).start()
        mock.patch.object(release.packaging, "ROOT", self.root).start()
        self.calls = []
        self.remote = COMMIT
        self.local = COMMIT
        self.dirty = ""
        self.origin = "git@github.com:example/storage.git"
        self.marker = ""
        self.info = {"id": 123, "head_sha": COMMIT, "head_branch": "v0.1.0", "path": release.WORKFLOW_PATH,
                     "event": "workflow_dispatch", "status": "completed", "conclusion": "success", "run_attempt": 1}
        mock.patch.object(release, "run", side_effect=self.command).start()

    def command(self, args, **options):
        self.calls.append(args)
        stdout = ""
        code = 0
        if args[:2] == ["cargo", "metadata"]:
            return subprocess.run(args, cwd=self.root, check=True, capture_output=True, text=True, timeout=30)
        if args[0] == "git":
            if "status" in args:
                stdout = self.dirty
            elif args[1:3] == ["rev-parse", "HEAD"]:
                stdout = COMMIT
            elif args[1:3] == ["remote", "get-url"]:
                stdout = self.origin
            elif args[1] == "ls-remote":
                if self.remote:
                    stdout = f"{'b'*40}\trefs/tags/v0.1.0\n{self.remote}\trefs/tags/v0.1.0^{{}}\n"
            elif args[1] == "rev-parse":
                stdout, code = self.local or "", 0 if self.local else 1
            elif args[1] == "tag":
                self.local = COMMIT
            elif args[1] == "push":
                self.remote = self.local
            else:
                raise AssertionError(args)
        elif args[:2] == ["gh", "api"]:
            stdout = json.dumps(self.info)
        elif args[:3] == ["gh", "repo", "view"]:
            stdout = '{"nameWithOwner":"example/storage"}'
        elif args[:3] == ["gh", "workflow", "run"]:
            self.marker = next(item.removeprefix("release_id=") for item in args if item.startswith("release_id="))
        elif args[:3] == ["gh", "run", "list"]:
            stdout = json.dumps([{"databaseId": 999, "headSha": COMMIT, "displayTitle": "someone else's dispatch"},
                                 {"databaseId": 123, "headSha": COMMIT, "displayTitle": self.marker}])
        elif args[:3] == ["gh", "run", "download"]:
            self.artifacts(Path(args[args.index("--dir") + 1]))
        elif args[:3] in (["gh", "run", "watch"], ["gh", "release", "create"]) or args[:2] == ["cargo", "run"]:
            pass
        else:
            raise AssertionError(args)
        return subprocess.CompletedProcess(args, code, stdout)

    def artifacts(self, directory):
        for target in release.packaging.TARGETS:
            binary = self.root / target
            binary.write_bytes(header_fixture(target))
            binary.chmod(0o755)
            release.packaging.pack(binary, target, directory, "0.1.0")
        release.packaging.generate(directory, "https://github.com/example/storage", "0.1.0")

    def candidate(self):
        return release.Release("0.1.0", self.root / "artifacts/release-0.1.0")

    def published_calls(self):
        return [args for args in self.calls if args[:3] == ["gh", "release", "create"]]

    def test_prepare_refreshes_both_real_cargo_lockfiles_without_changing_dependencies(self):
        release.prepare("0.2.0")
        release.check_versions("0.2.0")
        self.assertIn('# keep this comment', (self.root / "Cargo.toml").read_text())
        self.assertTrue(any(args[:3] == ["cargo", "run", "--locked"] for args in self.calls))
        self.assertFalse(any(args[0] in ("gh", "git") for args in self.calls))

    def test_prepare_rolls_back_manifest_locks_and_completions_when_a_step_fails(self):
        originals = {p: p.read_bytes() for p in self.root.rglob("*") if p.is_file()}
        def fail(args, **options):
            (self.root / "completions/fixture.bash").write_text("changed")
            raise subprocess.CalledProcessError(1, args)
        with mock.patch.object(release, "run", side_effect=fail):
            with self.assertRaises(subprocess.CalledProcessError):
                release.prepare("0.2.0")
        self.assertEqual(originals, {p: p.read_bytes() for p in originals})

    def test_prepare_rejects_dependency_resolution_drift_and_restores_inputs(self):
        old = (self.root / "Cargo.lock").read_bytes()
        original_command = self.command
        def drift(args, **options):
            result = original_command(args, **options)
            if args[:2] == ["cargo", "metadata"] and "fuzz/Cargo.toml" in args:
                with (self.root / "Cargo.lock").open("a") as output:
                    output.write('[[package]]\nname="unreviewed"\nversion="1.0.0"\n')
            return result
        with mock.patch.object(release, "run", side_effect=drift):
            with self.assertRaisesRegex(ValueError, "beyond the local package version"):
                release.prepare("0.2.0")
        self.assertEqual(old, (self.root / "Cargo.lock").read_bytes())
        release.check_versions("0.1.0")

    def test_clean_checkout_and_matching_manifest_locks_are_required(self):
        self.dirty = "?? unpublished.rs\n"
        with self.assertRaisesRegex(ValueError, "clean checkout"):
            self.candidate()
        self.dirty = ""
        (self.root / "fuzz/Cargo.lock").write_text((self.root / "fuzz/Cargo.lock").read_text().replace('0.1.0', '0.1.0-dev'))
        with self.assertRaisesRegex(ValueError, "fuzz/Cargo.lock version differs"):
            self.candidate()

    def test_github_origin_is_resolved_without_using_an_upstream_default(self):
        for origin in ["https://github.com/example/storage.git", "git@github.com:example/storage.git", "ssh://git@github.com/example/storage"]:
            self.origin = origin
            self.assertEqual(self.candidate().repository, "example/storage")
        self.origin = "https://secret@github.com/example/storage.git"
        with self.assertRaisesRegex(ValueError, "origin must identify"):
            self.candidate()

    def test_build_tags_committed_source_and_downloads_the_exact_successful_dispatch(self):
        self.local = self.remote = None
        candidate = self.candidate()
        candidate.build()
        receipt = json.loads((candidate.directory / "release-receipt.json").read_text())
        self.assertEqual(receipt["run_id"], 123)
        self.assertEqual(receipt["commit"], COMMIT)
        self.assertEqual(len(receipt["files"]), 6)
        self.assertTrue(any(args[:2] == ["git", "tag"] for args in self.calls))
        self.assertTrue(any(args[:2] == ["git", "push"] for args in self.calls))
        self.assertTrue(any(args[:4] == ["gh", "run", "watch", "123"] for args in self.calls))
        self.assertFalse(self.published_calls())

    def test_resume_does_not_create_tags_push_or_dispatch_another_build(self):
        self.info["path"] += "@refs/tags/v0.1.0"
        self.candidate().build("123")
        self.assertFalse(any(args[:2] in (["git", "tag"], ["git", "push"]) or args[:3] == ["gh", "workflow", "run"] for args in self.calls))

    def test_remote_or_local_tag_conflicts_are_never_overwritten(self):
        for local, remote in [(COMMIT, "c" * 40), ("c" * 40, COMMIT)]:
            self.local, self.remote = local, remote
            with self.assertRaisesRegex(ValueError, "another commit"):
                self.candidate().build()
        self.assertFalse(any(args[:2] in (["git", "tag"], ["git", "push"]) for args in self.calls))

    def test_failed_workflow_or_wrong_source_never_downloads_or_publishes(self):
        for field, bad in [("head_sha", "c" * 40), ("head_branch", "main"), ("path", ".github/workflows/ci.yml"),
                           ("event", "pull_request"), ("conclusion", "failure"), ("status", "in_progress")]:
            original = self.info[field]
            self.info[field] = bad
            with self.assertRaises(ValueError):
                self.candidate().build("123")
            self.info[field] = original
        self.assertFalse(any(args[:3] == ["gh", "run", "download"] for args in self.calls))
        self.assertFalse(self.published_calls())

    def test_dispatch_failure_stops_before_watching_or_downloading(self):
        def fail(args, **options):
            if args[:3] == ["gh", "workflow", "run"]:
                raise subprocess.CalledProcessError(1, args)
            return self.command(args, **options)
        with mock.patch.object(release, "run", side_effect=fail):
            with self.assertRaises(subprocess.CalledProcessError):
                self.candidate().build()
        self.assertFalse(any(args[:3] in (["gh", "run", "watch"], ["gh", "run", "download"]) for args in self.calls))

    def test_watch_failure_cannot_leave_a_passing_download_receipt(self):
        candidate = self.candidate()
        def fail(args, **options):
            if args[:3] == ["gh", "run", "watch"]:
                raise subprocess.CalledProcessError(1, args)
            return self.command(args, **options)
        with mock.patch.object(release, "run", side_effect=fail):
            with self.assertRaises(subprocess.CalledProcessError):
                candidate.build("123")
        self.assertFalse(candidate.directory.exists())
        self.assertFalse(any(args[:3] == ["gh", "run", "download"] for args in self.calls))

    def test_missing_dispatch_run_is_bounded_and_never_guesses_a_neighbor(self):
        def not_listed(args, **options):
            if args[:3] == ["gh", "run", "list"]:
                return subprocess.CompletedProcess(args, 0, '[]')
            return self.command(args, **options)
        with mock.patch.object(release, "run", side_effect=not_listed), mock.patch.object(release.time, "sleep") as sleep:
            with self.assertRaisesRegex(ValueError, "resume with RUN_ID"):
                self.candidate().dispatch()
        self.assertEqual(sleep.call_count, 30)

    def test_invalid_downloads_are_removed_and_cannot_be_published(self):
        candidate = self.candidate()
        def corrupt(args, **options):
            result = self.command(args, **options)
            if args[:3] == ["gh", "run", "download"]:
                directory = Path(args[args.index("--dir") + 1])
                (directory / "SHA256SUMS").write_text('incorrect checksums')
            return result
        with mock.patch.object(release, "run", side_effect=corrupt):
            with self.assertRaisesRegex(ValueError, "SHA256SUMS"):
                candidate.build("123")
        self.assertFalse(candidate.directory.exists())
        self.assertEqual(list(candidate.directory.parent.glob('release-download-*')), [])
        self.assertFalse(self.published_calls())

    def test_existing_download_directory_is_preserved(self):
        candidate = self.candidate()
        candidate.directory.mkdir(parents=True)
        sentinel = candidate.directory / "existing"
        sentinel.write_text("retain")
        with self.assertRaisesRegex(ValueError, "already exists"):
            candidate.build()
        self.assertEqual(sentinel.read_text(), "retain")
        self.assertFalse(any(args[:3] == ["gh", "workflow", "run"] for args in self.calls))

    def test_publish_uploads_only_the_six_verified_files_without_rebuilding(self):
        candidate = self.candidate()
        candidate.build("123")
        self.calls.clear()
        candidate.publish(draft=True)
        command = self.published_calls()[0]
        self.assertIn("--draft", command)
        self.assertIn("--verify-tag", command)
        self.assertIn("--generate-notes", command)
        self.assertEqual(len([argument for argument in command if argument.startswith(str(candidate.directory))]), 6)
        self.assertFalse(any(args[0] == "cargo" or args[:3] == ["gh", "workflow", "run"] for args in self.calls))

    def test_modified_assets_wrong_receipts_and_reruns_cannot_be_published(self):
        candidate = self.candidate()
        candidate.build("123")
        receipt_path = candidate.directory / "release-receipt.json"
        receipt = json.loads(receipt_path.read_text())
        for field, bad in [("commit", "c" * 40), ("repository", "another/repo"), ("version", "0.2.0"), ("run_attempt", 2), ("files", {})]:
            receipt_path.write_text(json.dumps(receipt | {field: bad}))
            with self.assertRaises(ValueError):
                candidate.publish()
        receipt_path.write_text(json.dumps(receipt))
        with (candidate.directory / "SHA256SUMS").open("a") as output:
            output.write("tampered\n")
        with self.assertRaises(ValueError):
            candidate.publish()
        self.assertFalse(self.published_calls())

    def test_versioned_release_notes_are_used_when_present(self):
        candidate = self.candidate()
        candidate.build("123")
        notes = self.root / "docs/releases/v0.1.0.md"
        notes.parent.mkdir(parents=True)
        notes.write_text("First experimental release\n")
        candidate.publish()
        command = self.published_calls()[0]
        self.assertEqual(command[command.index("--notes-file") + 1], str(notes))
        self.assertNotIn("--generate-notes", command)

    def test_release_version_validation_and_make_forwarding_have_no_side_effects(self):
        for invalid in ["", "v0.1.0", "01.1.0", "0.1", "0.1.0;touch sentinel", "0.1.0-01"]:
            with self.assertRaises(ValueError):
                release.version(invalid)
        for valid in ["0.1.0", "1.2.3-rc.1", "0.1.0-dev"]:
            self.assertEqual(release.version(valid), valid)
        for target in ["release", "release-prepare", "release-publish"]:
            result = subprocess.run(["make", target, "VERSION=0.1.0;touch sentinel"], cwd=ROOT, capture_output=True, text=True, timeout=15)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("Set VERSION", result.stderr)
        self.assertFalse((ROOT / "sentinel").exists())
        result = subprocess.run(["make", "release-help"], cwd=ROOT, capture_output=True, text=True, check=True, timeout=15)
        self.assertIn("make release VERSION=0.1.0", result.stdout)


if __name__ == "__main__":
    unittest.main()
