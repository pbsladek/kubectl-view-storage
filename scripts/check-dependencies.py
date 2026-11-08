#!/usr/bin/env python3
"""Check the lightweight manifest policy; this does not audit resolved crates."""
import argparse
from pathlib import Path
import re
import tomllib

# Twelve original capabilities plus safe Unix nonblocking output; see
# DEVELOPMENT.md for the reviewed cancellation/flag-restoration rationale.
MAX_RUNTIME_DEPENDENCIES = 13
REVIEWED_RUNTIME_PACKAGES = frozenset({
    "clap", "kube", "k8s-openapi", "tokio", "tokio-util", "serde", "serde_json",
    "thiserror", "jiff", "http", "http-body-util", "csv", "nix",
})


def inspect_manifest(manifest):
    runtime = set()
    errors = []
    if manifest.get("patch") or manifest.get("replace"):
        errors.append("source overrides require a reviewed policy change; patch/replace can bypass crates.io releases")
    scopes = [("package", manifest)] + list(manifest.get("target", {}).items())
    for scope, tables in scopes:
        if tables.get("build-dependencies"):
            errors.append(f"{scope}: direct build dependencies require a reviewed build-cost rationale and policy change")
        for name, dependency in tables.get("dependencies", {}).items():
            options = dependency if isinstance(dependency, dict) else {"version": dependency}
            package = options.get("package", name)
            runtime.add(package)
            location = f"{scope}: {name}"
            if package not in REVIEWED_RUNTIME_PACKAGES:
                errors.append(f"{location}: {package} needs an upstream maintenance/cost review and recorded retention rationale")
            version = options.get("version", "")
            if not version or "*" in version or re.search(r"\d-[0-9A-Za-z]", version):
                errors.append(f"{location}: specify a stable version requirement without wildcards")
            if any(key in options for key in ("git", "path", "registry", "workspace")):
                errors.append(f"{location}: runtime sources must use the reviewed crates.io release")
            if options.get("default-features") is not False:
                errors.append(f"{location}: explicitly disable defaults and select required features")
            features = set(options.get("features", []))
            if package == "tokio" and "full" in features:
                errors.append(f"{location}: select Tokio capabilities individually, not full")
            if package == "kube" and features & {"runtime", "derive"}:
                errors.append(f"{location}: controller runtime and CRD derives are outside P0")
            if package == "nix" and features - {"fs", "process", "signal", "term", "time", "event"}:
                errors.append(f"{location}: Unix output/process supervision requires only fs/process/signal/term/time and macOS event capabilities")
            if package == "nix" and "event" in features and scope != 'cfg(target_os = "macos")':
                errors.append(f"{location}: non-reaping kqueue observation is macOS-only")
    if len(runtime) > MAX_RUNTIME_DEPENDENCIES:
        errors.append(f"{len(runtime)} runtime dependencies exceed the reviewed budget of {MAX_RUNTIME_DEPENDENCIES}")
    return runtime, errors


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--manifest", type=Path, default=Path(__file__).resolve().parents[1] / "Cargo.toml")
    args = parser.parse_args()
    runtime, errors = inspect_manifest(tomllib.loads(args.manifest.read_text()))
    if errors:
        parser.exit(1, "\n".join(errors) + "\n")
    print(f"PASS manifest policy: {len(runtime)}/{MAX_RUNTIME_DEPENDENCIES} direct runtime dependencies; resolved maintenance/advisory audit still required")


if __name__ == "__main__":
    main()
