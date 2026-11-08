#!/usr/bin/env python3
"""Record real release compiler units/features, elapsed time and stripped size."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import subprocess
import time
import tomllib

ROOT = Path(__file__).resolve().parents[1]
TARGETS = ("x86_64-unknown-linux-musl","aarch64-unknown-linux-musl","x86_64-apple-darwin","aarch64-apple-darwin")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--target",choices=TARGETS,required=True)
    parser.add_argument("--runner",required=True)
    parser.add_argument("--target-dir",type=Path)
    parser.add_argument("--require-fresh-profile",action="store_true")
    parser.add_argument("--notes",default="")
    args = parser.parse_args()
    if not re.fullmatch(r"[A-Za-z0-9_-]+",args.runner):
        parser.error("runner must be an alphanumeric label with - or _")
    metadata = subprocess.run(["cargo","metadata","--locked","--offline","--no-deps","--format-version","1"],cwd=ROOT,text=True,capture_output=True,check=True,timeout=30)
    target_dir = (args.target_dir or Path(os.environ.get("CARGO_TARGET_DIR",json.loads(metadata.stdout)["target_directory"]))).absolute()
    profile = target_dir / args.target / "release"
    existed = profile.exists()
    if args.require_fresh_profile and existed:
        parser.error("selected release profile already exists; choose a new owned target directory")
    destination = ROOT / "artifacts/dependency-baseline"
    destination.mkdir(parents=True,exist_ok=True)
    receipt_path = destination / f"{args.runner}.json"
    log_path = destination / f"{args.runner}-build.jsonl"
    source = hashlib.sha256()
    for path in [ROOT/"Cargo.toml",ROOT/"Cargo.lock",ROOT/"rust-toolchain.toml",*sorted((ROOT/"src").rglob("*.rs"))]:
        source.update(str(path.relative_to(ROOT)).encode()+b"\0"+path.read_bytes()+b"\0")
    revision = subprocess.run(["git","rev-parse","HEAD"],cwd=ROOT,text=True,capture_output=True,timeout=10)
    receipt = {
        "status":"running","runner":args.runner,"platform":platform.platform(),"architecture":platform.machine(),
        "target":args.target,"production_source_sha256":source.hexdigest(),
        "revision":revision.stdout.strip() if revision.returncode==0 else None,
        "rustc":subprocess.check_output(["rustc","--version"],text=True).strip(),
        "cargo":subprocess.check_output(["cargo","--version"],text=True).strip(),
        "lock_sha256":hashlib.sha256((ROOT/"Cargo.lock").read_bytes()).hexdigest(),
        "resolved_package_versions":len(tomllib.loads((ROOT/"Cargo.lock").read_text())["package"]),
        "target_profile_preexisting":existed,"build_jobs":os.environ.get("CARGO_BUILD_JOBS"),
        "timing_scope":"entire Cargo build invocation; existing registry cache allowed; includes any fetch", "notes":args.notes,
    }
    receipt_path.write_text(json.dumps(receipt,indent=2)+"\n")
    started = time.monotonic()
    try:
        with log_path.open("w") as log:
            subprocess.run(["cargo","build","--locked","--release","--target",args.target,"--message-format=json-render-diagnostics"],cwd=ROOT,env=dict(os.environ,CARGO_TARGET_DIR=str(target_dir)),stdout=log,check=True,timeout=1200)
        units = []
        executables = set()
        with log_path.open() as log:
            for line in log:
                event = json.loads(line)
                if event.get("reason")!="compiler-artifact":
                    continue
                units.append({"package_id":event["package_id"],"features":event["features"],"target_kind":event["target"]["kind"],"fresh":event["fresh"]})
                if event.get("executable") and event["target"]["name"]=="kubectl-view_storage" and "bin" in event["target"]["kind"]:
                    executables.add(Path(event["executable"]))
        if len(executables)!=1:
            raise RuntimeError("build did not identify one actual plugin executable")
        binary = executables.pop()
        receipt.update(status="passed",binary_bytes=binary.stat().st_size,binary_sha256=hashlib.sha256(binary.read_bytes()).hexdigest(),compiled_unique_package_ids=len({unit["package_id"] for unit in units}),compiled_units=units)
    except BaseException as error:
        receipt.update(status="failed",error_kind=type(error).__name__)
        raise
    finally:
        receipt["elapsed_seconds"]=time.monotonic()-started
        receipt_path.write_text(json.dumps(receipt,indent=2)+"\n")
    print(f"PASS actual {args.target}: {receipt['compiled_unique_package_ids']} compiled packages, {receipt['binary_bytes']} bytes, {receipt['elapsed_seconds']:.2f}s")


if __name__=="__main__":
    main()
