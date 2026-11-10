#!/usr/bin/env python3
"""Check the exact local pin and optionally compare the official stable channel."""
import argparse
import json
from pathlib import Path
import re
import subprocess
import tomllib
import urllib.request


def configuration(root):
    selected = tomllib.loads((root / "rust-toolchain.toml").read_text())["toolchain"]["channel"]
    package = tomllib.loads((root / "Cargo.toml").read_text())["package"]
    recorded = json.loads((root / "dev/versions.json").read_text())["rust"]
    if not re.fullmatch(r"\d+\.\d+\.\d+", selected):
        raise ValueError("pin an exact stable Rust release")
    if package.get("rust-version") != selected or recorded.get("channel") != selected:
        raise ValueError("toolchain, rust-version, and environment Rust pins differ")
    if str(package.get("edition")) != "2024" or recorded.get("edition") != "2024":
        raise ValueError("package and environment must select Rust 2024")
    cargo = recorded.get("cargo_version", "")
    if recorded.get("cargo") != "bundled" or not re.fullmatch(r"\d+\.\d+\.\d+", cargo):
        raise ValueError("record the Cargo version bundled with the selected toolchain")
    return selected, cargo


def latest_bundle(selected, cargo, manifest, installed_cargo_verbose):
    latest = manifest["pkg"]["rust"]["version"].split()[0]
    if selected != latest:
        raise ValueError(f"Rust pin {selected} needs a tested update to latest stable {latest}")
    # The channel manifest uses Cargo's crate version (0.100.0 here), while
    # `cargo --version` uses the toolchain release (1.99.1). Verify the binary
    # release and upstream Cargo commit rather than equating those namespaces.
    upstream = re.fullmatch(r"\d+\.\d+\.\d+ \(([0-9a-f]{7,40}) \d{4}-\d\d-\d\d\)", manifest["pkg"]["cargo"]["version"])
    commit = re.search(r"^commit-hash: ([0-9a-f]{40})$", installed_cargo_verbose, re.M)
    release = re.search(r"^release: (\S+)$", installed_cargo_verbose, re.M)
    if not upstream or not commit or not release or release[1] != cargo or not commit[1].startswith(upstream[1]):
        raise ValueError(f"installed Cargo {cargo} does not match the official stable bundle release/commit")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--latest", action="store_true")
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[1]
    try:
        selected, cargo = configuration(root)
    except ValueError as error:
        parser.exit(1, f"{error}\n")
    if args.latest:
        with urllib.request.urlopen("https://static.rust-lang.org/dist/channel-rust-stable.toml", timeout=30) as response:
            manifest = tomllib.loads(response.read(2 * 1024 * 1024).decode())
        try:
            latest_bundle(selected, cargo, manifest, subprocess.check_output(["cargo", "--version", "--verbose"], text=True))
        except ValueError as error:
            parser.exit(1, f"{error}\n")
    installed = subprocess.check_output(["rustc", "--version"], text=True).split()[1]
    if installed != selected:
        parser.exit(1, f"installed rustc {installed} differs from pin {selected}\n")
    installed_cargo = subprocess.check_output(["cargo", "--version"], text=True).split()[1]
    if installed_cargo != cargo:
        parser.exit(1, f"installed cargo {installed_cargo} differs from recorded bundle {cargo}\n")
    print(f"PASS Rust {selected}, bundled Cargo {cargo}, edition 2024")


if __name__ == "__main__":
    main()
