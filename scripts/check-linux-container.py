#!/usr/bin/env python3
"""Run locked Linux tests/builds in an owned, disposable official Rust container."""
import argparse
import hashlib
import json
from pathlib import Path
import platform
import subprocess
import time

ROOT = Path(__file__).resolve().parents[1]
VERSIONS = json.loads((ROOT / "dev/versions.json").read_text())
IMAGE = VERSIONS["linux_validation"]["image"]
PLATFORMS = VERSIONS["linux_validation"]["platforms"]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--arch", choices=PLATFORMS, default="arm64")
    args = parser.parse_args()
    arch = args.arch
    target = PLATFORMS[arch]["target"]
    manifest_digest = PLATFORMS[arch]["manifest_digest"]
    name = f"view-storage-rust-linux-{arch}"
    evidence = ROOT / "artifacts" / f"linux-{arch}-container"
    build = ROOT / "target" / f"linux-{arch}-container"
    registry = ROOT / ".local" / "linux-cargo-registry"
    for directory in (evidence, build, registry):
        directory.mkdir(parents=True, exist_ok=True)
    # A fresh name is required. Never adopt/remove an unrelated existing container.
    existing = subprocess.run(["docker", "ps", "-a", "--filter", f"name=^/{name}$", "--format", "{{.ID}}"], check=True, text=True, capture_output=True, timeout=20)
    if existing.stdout.strip():
        parser.exit(1, f"Inspect the existing {name} container before retrying.\n")
    source = hashlib.sha256()
    for path in [ROOT / "Cargo.toml", ROOT / "Cargo.lock", *sorted((ROOT / "src").rglob("*.rs")), *sorted((ROOT / "tests").rglob("*.rs"))]:
        source.update(str(path.relative_to(ROOT)).encode())
        source.update(path.read_bytes())
    receipt = {
        "status": "running", "host": platform.platform(), "host_architecture": platform.machine(),
        "image": IMAGE, "platform_manifest": manifest_digest, "platform": f"linux/{arch}",
        "target": target, "source_sha256": source.hexdigest(),
        "lock_sha256": hashlib.sha256((ROOT / "Cargo.lock").read_bytes()).hexdigest(),
        "scope": "Linux container tests and musl build; host CI/native-machine certification is separate",
        "cpu_limit": 3, "memory_limit": "2500m", "build_jobs": 2, "test_threads": 1,
        "python_apk": VERSIONS["linux_validation"]["python_apk"],
    }
    result_path = evidence / "validation.json"
    result_path.write_text(json.dumps(receipt, indent=2) + "\n")
    command = [
        "docker", "run", "--rm", "--name", name, "--platform", f"linux/{arch}",
        "--label", f"view-storage.workspace={ROOT}", "--cpus", "3", "--memory", "2500m",
        "--mount", f"type=bind,source={ROOT},target=/workspace,readonly",
        "--mount", f"type=bind,source={build},target=/target",
        "--mount", f"type=bind,source={registry},target=/usr/local/cargo/registry",
        "--mount", f"type=bind,source={evidence},target=/evidence",
        "--workdir", "/workspace", "--env", "CARGO_TARGET_DIR=/target",
        "--env", "CARGO_BUILD_JOBS=2", "--env", "RUST_TEST_THREADS=1",
        "--env", "VIEW_STORAGE_TEST_REPORTS=/evidence/api-reports",
        "--env", "VIEW_STORAGE_HTTP_REPORTS=/evidence/http-reports",
        "--env", f"VIEW_STORAGE_RUST={VERSIONS['rust']['channel']}",
        "--env", f"VIEW_STORAGE_CARGO={VERSIONS['rust']['cargo_version']}",
        "--env", f"VIEW_STORAGE_PYTHON_APK={VERSIONS['linux_validation']['python_apk']}",
        "--env", f"VIEW_STORAGE_TARGET={target}", "-i", IMAGE, "sh", "-s",
    ]
    script = """set -eu
rustc --version > /evidence/rustc-version.txt
cargo --version > /evidence/cargo-version.txt
test "$(rustc --version | cut -d ' ' -f 2)" = "$VIEW_STORAGE_RUST"
test "$(cargo --version | cut -d ' ' -f 2)" = "$VIEW_STORAGE_CARGO"
apk add --no-cache "python3=$VIEW_STORAGE_PYTHON_APK"
python3 --version > /evidence/python-version.txt
apk info -v python3 > /evidence/python-package.txt
cargo test --locked --all-targets
python3 scripts/test-credential-terminal.py --binary /target/debug/kubectl-view_storage --evidence-directory /evidence/authentication
cargo build --locked --release --target "$VIEW_STORAGE_TARGET" --message-format=json-render-diagnostics > /evidence/release-build.jsonl
"/target/$VIEW_STORAGE_TARGET/release/kubectl-view_storage" --version > /evidence/plugin-version.txt
"""
    started = time.monotonic()
    try:
        with (evidence / "build-test.log").open("w") as log:
            subprocess.run(command, input=script, text=True, stdout=log, stderr=subprocess.STDOUT, check=True, timeout=1500)
        binary = build / target / "release/kubectl-view_storage"
        receipt.update(status="passed", binary_sha256=hashlib.sha256(binary.read_bytes()).hexdigest(), binary_bytes=binary.stat().st_size)
    except BaseException as error:
        receipt.update(status="failed", error_kind=type(error).__name__)
        raise
    finally:
        receipt["elapsed_seconds"] = time.monotonic() - started
        # If the client was interrupted/timed out, stop only this workspace's
        # labeled container. --rm performs removal after its process exits.
        remaining = subprocess.run(["docker", "ps", "-aq", "--filter", f"name=^/{name}$", "--filter", f"label=view-storage.workspace={ROOT}"], text=True, capture_output=True, timeout=20)
        if remaining.returncode == 0 and remaining.stdout.strip():
            stopped = subprocess.run(["docker", "stop", "--timeout", "10", name], text=True, capture_output=True, timeout=30)
            if stopped.returncode:
                receipt["status"] = "failed"
                receipt["cleanup"] = "failed"
        remaining = subprocess.run(["docker", "ps", "-aq", "--filter", f"name=^/{name}$", "--filter", f"label=view-storage.workspace={ROOT}"], text=True, capture_output=True, timeout=20)
        receipt["cleanup"] = "passed" if remaining.returncode == 0 and not remaining.stdout.strip() else "failed"
        if receipt["cleanup"] != "passed":
            receipt["status"] = "failed"
        result_path.write_text(json.dumps(receipt, indent=2) + "\n")
    print(f"{receipt['status'].upper()} {target}; evidence {result_path}")
    return 0 if receipt["status"] == "passed" else 1


if __name__ == "__main__":
    raise SystemExit(main())
