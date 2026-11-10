#!/usr/bin/env python3
"""Run the same owned, disposable storage gates locally and in GitHub Actions."""

import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import time
import uuid

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location(
    "dev_cluster", ROOT / "scripts/dev-cluster.py"
)
cluster = importlib.util.module_from_spec(spec)
spec.loader.exec_module(cluster)
PROFILES = {"local-path": "inventory", "csi-hostpath": "csi", "nfs-csi": "nfs"}


def user_configs():
    paths = {Path.home() / ".kube/config"}
    paths.update(
        Path(p).expanduser()
        for p in os.environ.get("KUBECONFIG", "").split(os.pathsep)
        if p
    )
    return {
        str(path): hashlib.sha256(path.read_bytes()).hexdigest()
        if path.is_file()
        else None
        for path in paths
    }


def run_suite(binary, profiles, node_image=None):
    binary = Path(binary).resolve()
    if not binary.is_file() or not os.access(binary, os.X_OK):
        raise RuntimeError("build an executable plugin first, or select --binary")
    suite_id = str(uuid.uuid4())
    evidence = ROOT / "artifacts" / ("storage-e2e-" + suite_id)
    evidence.mkdir(parents=True)
    receipt = {
        "status": "running",
        "suite_id": suite_id,
        "node_image": node_image or cluster.VERSIONS["baseline_node"],
        "plugin_sha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
        "profiles": {},
    }
    configs_before = user_configs()

    def save():
        (evidence / "suite.json").write_text(json.dumps(receipt, indent=2) + "\n")

    save()
    interrupted = False
    for selected in profiles:
        profile = PROFILES[selected]
        run_id = str(uuid.uuid4())
        result = {
            "status": "running",
            "run_id": run_id,
            "evidence": run_id,
            "cleanup": "not_created",
        }
        receipt["profiles"][selected] = result
        started = time.monotonic()
        save()
        print(f"START {selected}; run {run_id}", flush=True)
        try:
            cluster.create(profile, binary=binary, node_image=node_image, run_id=run_id)
            result["status"] = "passed"
        except KeyboardInterrupt:
            result["status"] = "failed"
            result["error_kind"] = "KeyboardInterrupt"
            interrupted = True
        except Exception as error:
            result["status"] = "failed"
            result["error_kind"] = type(error).__name__
            # Child stderr may contain credentials or endpoints. Persist categories only.
            print(
                f"FAIL {selected}: {type(error).__name__}; inspect owned run {run_id}",
                flush=True,
            )
        finally:
            state_path = cluster.paths(profile)[2]
            if state_path.exists():
                try:
                    state = cluster.read_state(profile)[3]
                    if state["run_id"] == run_id:
                        cluster.delete(profile)
                        result["cleanup"] = "passed"
                    else:
                        result["cleanup"] = "preexisting_profile_preserved"
                        result["status"] = "failed"
                except Exception as error:
                    result["status"] = "failed"
                    result["cleanup"] = "failed"
                    result["cleanup_error_kind"] = type(error).__name__
            result["duration_seconds"] = round(time.monotonic() - started, 2)
            save()
        print(
            f"{result['status'].upper()} {selected}; cleanup={result['cleanup']}; {result['duration_seconds']}s",
            flush=True,
        )
        if interrupted:
            break
    receipt["user_kubeconfigs_unchanged"] = configs_before == user_configs()
    receipt["status"] = (
        "passed"
        if (
            len(receipt["profiles"]) == len(profiles)
            and all(
                r["status"] == "passed" and r["cleanup"] == "passed"
                for r in receipt["profiles"].values()
            )
            and receipt["user_kubeconfigs_unchanged"]
        )
        else "failed"
    )
    save()
    print(f"{receipt['status'].upper()} storage suite; evidence {evidence}", flush=True)
    return receipt["status"] == "passed"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--binary", type=Path, default=ROOT / "target/debug/kubectl-view_storage"
    )
    parser.add_argument(
        "--profiles", nargs="+", choices=tuple(PROFILES), default=list(PROFILES)
    )
    parser.add_argument(
        "--node-image",
        choices=[
            cluster.VERSIONS["baseline_node"],
            *cluster.VERSIONS["compatibility_nodes"],
        ],
    )
    args = parser.parse_args()
    if len(set(args.profiles)) != len(args.profiles):
        parser.error("profiles must be unique")
    try:
        return 0 if run_suite(args.binary, args.profiles, args.node_image) else 1
    except (RuntimeError, OSError, subprocess.SubprocessError) as error:
        print(f"FAIL storage suite: {type(error).__name__}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
