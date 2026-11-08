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
        latest = manifest["pkg"]["rust"]["version"].split()[0]
        if selected != latest:
            parser.exit(1, f"Rust pin {selected} needs a tested update to latest stable {latest}\n")
        latest_cargo = manifest["pkg"]["cargo"]["version"].split()[0]
        if cargo != latest_cargo:
            parser.exit(1, f"recorded Cargo {cargo} differs from stable bundle {latest_cargo}\n")
    else:
        installed = subprocess.check_output(["rustc", "--version"], text=True).split()[1]
        if installed != selected:
            parser.exit(1, f"installed rustc {installed} differs from pin {selected}\n")
        installed_cargo = subprocess.check_output(["cargo", "--version"], text=True).split()[1]
        if installed_cargo != cargo:
            parser.exit(1, f"installed cargo {installed_cargo} differs from recorded bundle {cargo}\n")
    print(f"PASS Rust {selected}, bundled Cargo {cargo}, edition 2024")


if __name__ == "__main__":
    main()
