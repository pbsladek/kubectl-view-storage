#!/usr/bin/env python3
"""Owned disposable kind clusters; every Kubernetes operation has an explicit kubeconfig."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import uuid
import dev_rbac
import dev_csi
import dev_nfs
import dev_local

ROOT = Path(__file__).resolve().parents[1]
LOCAL = ROOT / ".local"
VERSIONS = json.loads((ROOT / "dev/versions.json").read_text())


def run(*args, capture=True, timeout=180):
    completed = subprocess.run(args, cwd=ROOT, check=True, text=True, capture_output=capture, timeout=timeout)
    return completed.stdout.strip() if capture else ""


def paths(profile):
    return f"view-storage-{profile}", LOCAL / "kubeconfigs" / f"{profile}.yaml", LOCAL / "clusters" / f"{profile}.json"


def kubectl(config, *args):
    return run("kubectl", "--kubeconfig", str(config), "--request-timeout=15s", *args)


def container_id(name):
    return run("docker", "inspect", "--format", "{{.Id}}", name + "-control-plane")


def kubectl_pin(node_image):
    if node_image not in [VERSIONS["baseline_node"], *VERSIONS["compatibility_nodes"]]:
        raise RuntimeError("node image must be a reviewed digest pin in dev/versions.json")
    if node_image == VERSIONS["baseline_node"]:
        return VERSIONS["kubectl"]
    return node_image.split(":v", 1)[1].split("@", 1)[0]


def require_kubectl(node_image):
    expected = kubectl_pin(node_image)
    if json.loads(run("kubectl", "version", "--client=true", "-o", "json"))["clientVersion"]["gitVersion"] != "v" + expected:
        raise RuntimeError(f"kubectl must match the selected node's recorded client pin ({expected}); select its directory through PATH")


def preflight(profile, node_image=None, binary=None):
    node_image = node_image or VERSIONS["baseline_node"]
    for executable in (("docker", "kind", "kubectl") if binary is not None else ("docker", "kind", "kubectl", "rustc", "cargo")):
        if not shutil.which(executable):
            raise RuntimeError(f"missing executable: {executable}")
    if binary is not None and (not Path(binary).is_file() or not os.access(binary, os.X_OK)):
        raise RuntimeError("prebuilt plugin must be a real executable")
    for tool, version in (() if binary is not None else (("rustc", VERSIONS["rust"]["channel"]), ("cargo", VERSIONS["rust"]["cargo_version"]))):
        if run(tool, "--version").split()[1] != version:
            raise RuntimeError(f"{tool} must match the recorded toolchain bundle ({version})")
    if run("kind", "version").split()[1] != "v" + VERSIONS["kind"]:
        raise RuntimeError("kind version does not match dev/versions.json")
    require_kubectl(node_image)
    run("docker", "info", "--format", "{{.ServerVersion}}", timeout=20)
    if profile in ("inventory", "csi"):
        verify_csi_lock()
    if profile == "nfs":
        verify_csi_lock()
        dev_nfs.validate_lock(ROOT / "dev/nfs", VERSIONS)
    print(f"PASS preflight {profile}")


def verify_csi_lock():
    return dev_csi.validate_lock(ROOT / "dev/csi", VERSIONS)


def read_state(profile):
    name, config, state_path = paths(profile)
    state = json.loads(state_path.read_text())
    if state["root"] != str(ROOT) or state["name"] != name or state["kubeconfig"] != str(config):
        raise RuntimeError("ownership state does not match this workspace/profile")
    return name, config, state_path, state


def read_owned(profile):
    name, config, state_path, state = read_state(profile)
    if not state.get("container_id") or container_id(name) != state["container_id"]:
        raise RuntimeError("cluster container identity changed; refusing to operate on an unowned cluster")
    if kubectl(config, "config", "current-context") != "kind-" + name:
        raise RuntimeError("generated kubeconfig context does not match owned cluster")
    return name, config, state_path, state


def create(profile, binary=None, node_image=None, run_id=None):
    node_image = node_image or VERSIONS["baseline_node"]
    preflight(profile, node_image, binary=binary)
    name, config, state_path = paths(profile)
    existing = run("kind", "get", "clusters").splitlines()
    if name in existing or state_path.exists() or config.exists() or config.is_symlink():
        raise RuntimeError("cluster, ownership state, or kubeconfig already exists; inspect the existing profile before creation")
    config.parent.mkdir(parents=True, exist_ok=True)
    state_path.parent.mkdir(parents=True, exist_ok=True)
    state = {"root": str(ROOT), "name": name, "kubeconfig": str(config), "run_id": str(uuid.UUID(run_id)) if run_id else str(uuid.uuid4()), "container_id": None, "node_image": node_image, "kubectl_pin": kubectl_pin(node_image)}
    dev_rbac.write_private_json(state_path, state)
    try:
        env = dict(os.environ, KIND_EXPERIMENTAL_PROVIDER="docker")
        subprocess.run(["kind", "create", "cluster", "--name", name, "--config", str(ROOT / f"dev/kind/{profile}.yaml"), "--image", node_image, "--kubeconfig", str(config), "--wait", "120s"], env=env, cwd=ROOT, check=True, timeout=240)
    finally:
        try:
            state["container_id"] = container_id(name)
            state_path.write_text(json.dumps(state, indent=2) + "\n")
        except subprocess.SubprocessError:
            pass
    read_owned(profile)
    if profile in ("csi", "nfs"):
        lock = verify_csi_lock() if profile == "csi" else dev_nfs.validate_lock(ROOT / "dev/nfs", VERSIONS)
        bundle = ROOT / ("dev/csi" if profile == "csi" else "dev/nfs")
        for manifest in lock["manifests"]:
            path = bundle / manifest["file"]
            kubectl(config, "apply", "-f", str(path))
            for item in dev_csi.objects(json.loads(path.read_text())):
                if item["kind"] == "CustomResourceDefinition":
                    kubectl(config, "wait", "--for=condition=Established", "crd/" + item["metadata"]["name"], "--timeout=60s")
        fixture = LOCAL / "clusters" / f"{profile}-{state['run_id']}-fixture.json"
        rendered = dev_csi.render_fixture(lock["workload_image"], state["run_id"]) if profile == "csi" else dev_nfs.render_fixture(lock, state["run_id"])
        fixture.write_text(json.dumps(rendered, indent=2) + "\n")
        state["csi_fixture"] = str(fixture)
        state_path.write_text(json.dumps(state, indent=2) + "\n")
        kubectl(config, "apply", "-f", str(fixture))
        verify(profile, binary=binary)
        return
    kubectl(config, "apply", "-f", str(ROOT / "dev/fixtures/inventory.yaml"))
    kubectl(config, "apply", "-f", str(ROOT / "dev/fixtures/inventory-cases.json"))
    kubectl(config, "apply", "-f", str(ROOT / "dev/fixtures/rbac.json"))
    fixture = LOCAL / "clusters" / f"{profile}-{state['run_id']}-fixture.json"
    fixture.write_text(json.dumps(dev_local.render_fixture(verify_csi_lock()["workload_image"], state["run_id"]), indent=2) + "\n")
    state["csi_fixture"] = str(fixture)
    state_path.write_text(json.dumps(state, indent=2) + "\n")
    kubectl(config, "apply", "-f", str(fixture))
    for pod in ("consumer", "shared-consumer", "terminating-consumer"):
        kubectl(config, "-n", dev_rbac.NAMESPACE, "wait", "--for=condition=Ready", "pod/" + pod, "--timeout=120s")
    kubectl(config, "-n", dev_rbac.NAMESPACE, "delete", "pod", "terminating-consumer", "--wait=false", "--grace-period=3600")
    verify(profile, binary=binary)


def credential_directory(profile, state):
    # The state-derived component must remain a UUID, never a path fragment.
    run_id = str(uuid.UUID(state["run_id"]))
    if run_id != state["run_id"]:
        raise RuntimeError("ownership run ID is not canonical")
    path = LOCAL / "kubeconfigs" / f"{profile}-{run_id}"
    if path.is_symlink():
        raise RuntimeError("owned credential directory must not be a symlink")
    return path


def restricted_credentials(profile, config, state_path, state):
    directory = credential_directory(profile, state)
    directory.mkdir(mode=0o700, exist_ok=True)
    admin = json.loads(kubectl(config, "config", "view", "--minify", "--raw", "--flatten", "-o", "json"))
    configs = {}
    for account in dev_rbac.PROFILES:
        token = kubectl(config, "-n", dev_rbac.NAMESPACE, "create", "token", account, "--duration=20m")
        path = directory / f"{account}-{uuid.uuid4()}.json"
        dev_rbac.write_private_json(path, dev_rbac.token_kubeconfig(admin, token, account))
        # Record each successfully written private file before the next request,
        # so partial setup can still clean up every credential it created.
        state.setdefault("restricted_kubeconfigs", []).append(str(path))
        state_path.write_text(json.dumps(state, indent=2) + "\n")
        configs[account] = path
    return configs


def verify(profile, binary=None, plugin_kubeconfig=None):
    if plugin_kubeconfig is not None and profile != "csi":
        raise RuntimeError("separate plugin credentials apply only to CSI verification")
    _, config, state_path, state = read_owned(profile)
    credential_directory(profile, state)
    evidence = ROOT / "artifacts" / state["run_id"]
    evidence.mkdir(parents=True, exist_ok=True)
    validation = evidence / f"{profile}-validation.json"
    validation.write_text(json.dumps({"status": "running", "run_id": state["run_id"]}, indent=2) + "\n")
    try:
        require_kubectl(state.get("node_image", VERSIONS["baseline_node"]))
        if profile == "csi":
            lock = verify_csi_lock()
            result = dev_csi.verify(kubectl, config, binary or ROOT / "target/debug/kubectl-view_storage", evidence, state["run_id"], lock, query_config=plugin_kubeconfig)
            result["selection"] = VERSIONS
            result["node_image"] = state.get("node_image", VERSIONS["baseline_node"])
            result["kubectl"] = json.loads(kubectl(config, "version", "-o", "json"))
            result["query_credentials"] = "explicit_test_config" if plugin_kubeconfig is not None else "owned_admin"
            result["csi_lock"] = lock
            validation.write_text(json.dumps(result, indent=2) + "\n")
            print(f"PASS CSI hostpath Filesystem byte telemetry; evidence {evidence}")
        elif profile == "nfs":
            lock = dev_nfs.validate_lock(ROOT / "dev/nfs", VERSIONS)
            result = dev_nfs.verify(kubectl, config, binary or ROOT / "target/debug/kubectl-view_storage", evidence, state["run_id"], lock)
            result["node_image"] = state["node_image"]
            result["kubectl"] = json.loads(kubectl(config, "version", "-o", "json"))
            result["nfs_lock_sha256"] = VERSIONS["nfs_lock_sha256"]
            validation.write_text(json.dumps(result, indent=2) + "\n")
            print(f"PASS NFS CSI cross-node RWX byte/inode/write gate; evidence {evidence}")
        else:
            verify_inventory(profile, binary, config, state_path, state, evidence)
    except BaseException as error:
        validation.write_text(json.dumps({"status": "failed", "run_id": state["run_id"], "error_kind": type(error).__name__}, indent=2) + "\n")
        raise


def verify_inventory(profile, binary, config, state_path, state, evidence):
    kubectl(config, "wait", "--for=condition=Ready", "nodes", "--all", "--timeout=120s")
    for claim in ("data", "unreferenced"):
        kubectl(config, "-n", dev_rbac.NAMESPACE, "wait", "--for=jsonpath={.status.phase}=Bound", "pvc/" + claim, "--timeout=120s")
    for volume, phase in (("view-storage-available", "Available"), ("view-storage-released", "Released")):
        kubectl(config, "wait", "--for=jsonpath={.status.phase}=" + phase, "pv/" + volume, "--timeout=120s")
    claims = json.loads(kubectl(config, "-n", "view-storage-test", "get", "pvc", "-o", "json"))
    expected = {"pending", "classless", "data", "unreferenced"}
    if not expected.issubset({item["metadata"]["name"] for item in claims["items"]}):
        raise RuntimeError("inventory fixture claims are missing")
    for resource in ("pvc", "pod", "events"):
        (evidence / f"{resource}.json").write_text(kubectl(config, "-n", "view-storage-test", "get", resource, "-o", "json") + "\n")
    (evidence / "environment.json").write_text(json.dumps({"run_id": state["run_id"], "selection": VERSIONS, "node_image": state.get("node_image", VERSIONS["baseline_node"]), "kubectl": json.loads(kubectl(config, "version", "-o", "json")), "status": "inventory_fixture_present", "telemetry": "not_tested"}, indent=2) + "\n")
    configs = restricted_credentials(profile, config, state_path, state)
    capabilities = dev_rbac.verify_capabilities(configs)
    reports = dev_rbac.verify_plugin(binary or ROOT / "target/debug/kubectl-view_storage", config, configs, evidence)
    (evidence / "inventory-validation.json").write_text(json.dumps({"status": "passed", "run_id": state["run_id"], "plugin_sha256": hashlib.sha256(Path(binary or ROOT / "target/debug/kubectl-view_storage").read_bytes()).hexdigest(), "capabilities": capabilities, "reports": reports, "telemetry": "denied_proxy_tested; CSI fresh tuple not tested"}, indent=2) + "\n")
    dev_local.verify(kubectl, config, binary or ROOT / "target/debug/kubectl-view_storage", evidence, state["run_id"], Path(state["csi_fixture"]))
    print(f"PASS inventory/RBAC and local-path write/persistence gate; evidence {evidence}")


def delete(profile):
    name, config, state_path, state = read_state(profile)
    directory = credential_directory(profile, state)
    credentials = [Path(path) for path in state.get("restricted_kubeconfigs", [])]
    fixture = Path(state["csi_fixture"]) if "csi_fixture" in state else None
    if fixture is not None and (profile not in ("inventory", "csi", "nfs") or fixture != LOCAL / "clusters" / f"{profile}-{state['run_id']}-fixture.json" or fixture.is_symlink()):
        raise RuntimeError("CSI fixture cleanup path is outside the owned run")
    for path in credentials:
        if path.parent != directory or path.is_symlink() or path.suffix != ".json":
            raise RuntimeError("credential cleanup path is outside the owned run")
    present = name in run("kind", "get", "clusters").splitlines()
    if present:
        read_owned(profile)
        if profile == "nfs":
            # Unmount clients while the disposable server is still available.
            # Stopping all kind nodes concurrently can otherwise strand NFS I/O.
            try:
                claim_json = kubectl(config, "-n", dev_nfs.NAMESPACE, "get", "pvc", "data", "--ignore-not-found=true", "-o", "json")
                volume = json.loads(claim_json).get("spec", {}).get("volumeName") if claim_json else None
                kubectl(config, "-n", dev_nfs.NAMESPACE, "delete", "pod", "writer", "peer", "--ignore-not-found=true", "--wait=true", "--timeout=60s")
                kubectl(config, "-n", dev_nfs.NAMESPACE, "delete", "pvc", "data", "--ignore-not-found=true", "--wait=true", "--timeout=60s")
                if volume:
                    kubectl(config, "wait", "--for=delete", "pv/" + volume, "--timeout=60s")
            except subprocess.SubprocessError:
                print("NFS client drain unavailable; deleting the owned cluster with bounded fixture mounts.")
        run("kind", "delete", "cluster", "--name", name, capture=False)
    elif run("docker", "ps", "-a", "--filter", f"name=^/{name}-control-plane$", "--format", "{{.ID}}"):
        raise RuntimeError("a control-plane container remains without a matching kind profile; preserve ownership state for inspection")
    for path in credentials:
        path.unlink(missing_ok=True)
    if directory.exists() and not any(directory.iterdir()):
        directory.rmdir()
    config.unlink(missing_ok=True)
    if fixture is not None:
        fixture.unlink(missing_ok=True)
    state_path.unlink()
    print(f"Deleted owned cluster {name}" if present else f"Removed local ownership files for absent cluster {name}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=("preflight", "create", "verify", "delete"))
    parser.add_argument("profile", choices=("inventory", "csi", "nfs"))
    parser.add_argument("--binary", type=Path, help="Real built plugin for profile verification")
    parser.add_argument("--plugin-kubeconfig",type=Path,help="CSI verify only: separate read-only query credentials for negative telemetry gates; fixture setup remains owned")
    parser.add_argument("--node-image", choices=[VERSIONS["baseline_node"], *VERSIONS["compatibility_nodes"]], help="Reviewed node digest for create/preflight; older pins require matching kubectl on PATH")
    args = parser.parse_args()
    if args.plugin_kubeconfig and (args.action!="verify" or args.profile!="csi"):
        parser.error("--plugin-kubeconfig applies only to verify csi")
    if args.node_image and args.action not in ("create", "preflight"):
        parser.error("--node-image applies to create/preflight; verify uses the owned cluster's recorded pin")
    try:
        if args.action == "create":
            create(args.profile, binary=args.binary, node_image=args.node_image)
        elif args.action == "verify":
            verify(args.profile, binary=args.binary, plugin_kubeconfig=args.plugin_kubeconfig)
        elif args.action == "preflight":
            preflight(args.profile, node_image=args.node_image, binary=args.binary)
        else:
            globals()[args.action](args.profile)
    except KeyboardInterrupt:
        print(f"Interrupted {args.action} {args.profile}; owned state remains available for deletion.", file=sys.stderr)
        return 130
    except (RuntimeError, OSError, KeyError, ValueError, subprocess.SubprocessError) as error:
        # Command stderr may contain endpoint/credential-helper details. Report
        # operation and exit category; never copy arbitrary subprocess payloads.
        if isinstance(error, subprocess.CalledProcessError):
            message = f"{Path(error.cmd[0]).name} failed (exit {error.returncode}); inspect local runtime/configuration"
        else:
            message = str(error)
        print(f"FAIL {args.action} {args.profile}: {message}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
