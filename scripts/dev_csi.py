"""Required CSI hostpath gate for an already owned disposable cluster."""
import calendar
from datetime import datetime
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import time

ROOT = Path(__file__).resolve().parents[1]
NAMESPACE = "view-storage-csi"
DRIVER = "hostpath.csi.k8s.io"
PODS = ("writer", "peer")
PROBE = "/data/view-storage-probe.bin"
WRITE_BYTES = 16 * 1024 * 1024
MAX_AGE_SECONDS = 90
POLL_SECONDS = 5
CRDS = ("volumesnapshots.snapshot.storage.k8s.io", "volumesnapshotcontents.snapshot.storage.k8s.io", "volumesnapshotclasses.snapshot.storage.k8s.io")


def objects(document):
    return document["items"] if document.get("kind") == "List" else [document]


def validate_lock(vendor, versions):
    """Check the reviewed local lock; registry/release review precedes this step."""
    try:
        return _validate_lock(vendor, versions)
    except (KeyError, TypeError, AttributeError, IndexError) as error:
        raise RuntimeError("CSI lock is structurally invalid") from error


def _validate_lock(vendor, versions):
    vendor = Path(vendor)
    if vendor.is_symlink():
        raise RuntimeError("CSI vendor directory must not be a symlink")
    vendor = vendor.resolve()
    path = vendor / "lock.json"
    if path.is_symlink():
        raise RuntimeError("CSI lock must not be a symlink")
    if not path.is_file():
        raise RuntimeError("CSI deployment/image lock is unresolved; vendor and review dev/csi/lock.json before creating the CSI profile")
    if hashlib.sha256(path.read_bytes()).hexdigest() != versions.get("csi_lock_sha256"):
        raise RuntimeError("CSI lock checksum does not match dev/versions.json")
    lock = json.loads(path.read_text())
    if lock.get("status") != "reviewed" or lock.get("driver_release") != versions["csi_hostpath_release"]:
        raise RuntimeError("CSI lock must record the selected reviewed driver release")
    revisions = [lock.get("source_revision"), *lock.get("snapshot_dependency_revisions", {}).values()]
    if len(revisions) < 2 or any(not isinstance(revision, str) or not re.fullmatch(r"[0-9a-f]{40}", revision) for revision in revisions):
        raise RuntimeError("CSI driver and snapshot sources require exact commit revisions")
    images = set()
    for image in lock.get("images", []):
        ref = image.get("image")
        if not isinstance(ref, str) or not re.fullmatch(r"[^\s]+@sha256:[0-9a-f]{64}", ref):
            raise RuntimeError("CSI images require full immutable digests")
        platforms = image.get("platform_digests", {})
        if any(not re.fullmatch(r"sha256:[0-9a-f]{64}", platforms.get(arch, "")) for arch in ("amd64", "arm64")):
            raise RuntimeError("CSI lock lacks pinned amd64/arm64 image evidence")
        images.add(ref)
    workload = lock.get("workload_image")
    if not isinstance(workload, str) or workload not in images or not workload.startswith(versions["csi_workload_image"] + "@sha256:"):
        raise RuntimeError("CSI workload must use the selected locked image")
    def checked_file(entry):
        source = (vendor / entry["file"]).resolve()
        if not source.is_relative_to(vendor):
            raise RuntimeError("CSI manifest/license path escapes vendor directory")
        if hashlib.sha256(source.read_bytes()).hexdigest() != entry["sha256"]:
            raise RuntimeError("CSI manifest/license checksum mismatch")
        return source
    if not lock.get("licenses") or not lock.get("manifests"):
        raise RuntimeError("CSI lock requires reviewed manifests and licenses")
    for entry in lock["licenses"]:
        checked_file(entry)
    resources, referenced, driver_images = set(), set(), []
    def containers(value):
        if isinstance(value, dict):
            for key, child in value.items():
                if key in ("containers", "initContainers", "ephemeralContainers") and isinstance(child, list):
                    referenced.update(container["image"] for container in child)
                containers(child)
        elif isinstance(value, list):
            for child in value:
                containers(child)
    seen_workloads = False
    for entry in lock["manifests"]:
        source = checked_file(entry)
        if source.suffix != ".json":
            raise RuntimeError("CSI reviewed manifests must be canonical Kubernetes JSON")
        document = json.loads(source.read_text())
        entries = objects(document)
        has_crd = any(item["kind"] == "CustomResourceDefinition" for item in entries)
        if has_crd and seen_workloads:
            raise RuntimeError("CSI CRDs must precede other deployment manifests")
        seen_workloads |= not has_crd
        if any(item["kind"] == "CustomResourceDefinition" for item in entries) and any(item["kind"] != "CustomResourceDefinition" for item in entries):
            raise RuntimeError("CSI CRDs must be applied separately before dependent resources")
        for item in entries:
            resources.add((item["kind"], item["metadata"].get("namespace"), item["metadata"]["name"]))
            if (item["kind"], item["metadata"].get("namespace"), item["metadata"]["name"]) == ("StatefulSet", "default", "csi-hostpathplugin"):
                driver_images.extend(container["image"] for container in item["spec"]["template"]["spec"]["containers"] if container.get("name") == "hostpath")
        containers(document)
    snapshot = lock.get("snapshot_controller", {})
    required = {("CustomResourceDefinition", None, name) for name in CRDS}
    required |= {("CSIDriver", None, DRIVER), ("StatefulSet", "default", "csi-hostpathplugin"),
                 ("Deployment", snapshot.get("namespace"), snapshot.get("name"))}
    if not required.issubset(resources) or not snapshot.get("namespace") or not snapshot.get("name"):
        raise RuntimeError("CSI lock lacks the driver, snapshot CRDs, or snapshot controller")
    expected_driver = lock.get("driver_image")
    if not isinstance(expected_driver, str) or not expected_driver.startswith(versions["csi_hostpath_image"] + "@sha256:") or driver_images != [expected_driver] or not referenced.issubset(images):
        raise RuntimeError("CSI manifests contain an unlocked image or the wrong driver release image")
    return lock


def timestamp_ns(value):
    if not isinstance(value, str):
        raise RuntimeError("CSI report timestamp must be RFC 3339 UTC")
    match = re.fullmatch(r"(\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d)(?:\.(\d{1,9}))?(?:Z|\+00:00)", value or "")
    if not match:
        raise RuntimeError("CSI report timestamp must be RFC 3339 UTC")
    try:
        seconds = calendar.timegm(datetime.strptime(match[1], "%Y-%m-%dT%H:%M:%S").timetuple())
    except ValueError as error:
        raise RuntimeError("CSI report timestamp has an invalid date") from error
    return seconds * 10**9 + int((match[2] or "0").ljust(9, "0"))


def counter(value):
    if not isinstance(value, str) or not re.fullmatch(r"0|[1-9][0-9]{0,19}", value) or int(value) > 2**64 - 1:
        raise RuntimeError("CSI report counter must be an unsigned canonical decimal string")
    return int(value)


def render_fixture(image, run_id):
    if not isinstance(image, str) or not re.fullmatch(r"[^\s]+@sha256:[0-9a-f]{64}", image):
        raise RuntimeError("CSI workload image requires a reviewed digest")
    fixture = json.loads((ROOT / "dev/fixtures/csi.json").read_text())
    for item in fixture["items"]:
        item["metadata"].setdefault("labels", {})["view-storage/run-id"] = run_id
        if item["kind"] == "Pod":
            for container in item["spec"]["containers"]:
                if container["image"] != "__LOCKED_CSI_WORKLOAD_IMAGE__":
                    raise RuntimeError("CSI fixture contains an unreviewed image")
                container["image"] = image
    return fixture


def poll(operation, deadline, failure, *, monotonic=time.monotonic, sleep=time.sleep):
    while monotonic() < deadline:
        value = operation()
        if monotonic() >= deadline:
            break
        if value is not None:
            return value
        remaining = deadline - monotonic()
        if remaining > 0:
            sleep(min(POLL_SECONDS, remaining))
    raise RuntimeError(failure)


def fixture_identity(kubectl, config):
    claim = json.loads(kubectl(config, "-n", NAMESPACE, "get", "pvc", "data", "-o", "json"))
    pods = json.loads(kubectl(config, "-n", NAMESPACE, "get", "pods", "-l", "app=view-storage-csi-probe", "-o", "json"))["items"]
    selected = {pod["metadata"]["name"]: pod for pod in pods}
    if set(selected) != set(PODS):
        raise RuntimeError("CSI fixture does not have exactly its two consumers")
    if claim["metadata"].get("namespace") != NAMESPACE or not claim["metadata"].get("uid") or claim["spec"].get("volumeMode", "Filesystem") != "Filesystem" or claim["spec"].get("storageClassName") != NAMESPACE:
        raise RuntimeError("CSI fixture claim identity/class/mode is incorrect")
    if claim.get("status", {}).get("phase") != "Bound" or not claim.get("spec", {}).get("volumeName"):
        return None
    for pod in selected.values():
        if pod.get("status", {}).get("phase") in ("Failed", "Succeeded"):
            raise RuntimeError("CSI fixture consumer terminated before verification")
        if not any(condition.get("type") == "Ready" and condition.get("status") == "True" for condition in pod.get("status", {}).get("conditions", [])):
            return None
        if pod.get("status", {}).get("phase") != "Running":
            return None
    nodes = {pod["spec"].get("nodeName") for pod in selected.values()}
    if len({pod["metadata"].get("uid") for pod in selected.values()}) != 2 or any(not pod["metadata"].get("uid") or pod["metadata"].get("namespace") != NAMESPACE for pod in selected.values()):
        raise RuntimeError("CSI fixture consumers lack distinct namespaced identities")
    if len(nodes) != 1 or not next(iter(nodes)):
        raise RuntimeError("CSI hostpath consumers must share one scheduled node")
    volume = json.loads(kubectl(config, "get", "pv", claim["spec"]["volumeName"], "-o", "json"))
    if volume["spec"].get("csi", {}).get("driver") != DRIVER or volume["spec"].get("claimRef", {}).get("uid") != claim["metadata"]["uid"]:
        raise RuntimeError("CSI fixture volume driver/binding is incorrect")
    return {"uid": claim["metadata"]["uid"], "node": next(iter(nodes)),
            "consumers": {"pod:" + pod["metadata"]["uid"] for pod in selected.values()},
            "volume": volume["metadata"]["name"]}


def query(binary, config):
    command = [str(binary), "--kubeconfig", str(config), "-n", NAMESPACE, "--pvc", "data", "--require-metrics",
               "--max-metrics-age", f"{MAX_AGE_SECONDS}s", "--timeout", "15s", "--request-timeout", "5s", "-g", "pod,pvc", "-o", "json"]
    result = subprocess.run(command, text=True, capture_output=True, timeout=25)
    if result.returncode not in (0, 4):
        raise RuntimeError(f"CSI plugin query failed with exit {result.returncode}")
    try:
        report = json.loads(result.stdout)
    except json.JSONDecodeError as error:
        raise RuntimeError("CSI plugin did not produce a JSON report") from error
    return report, result.returncode


def observation(report, exit_code, identity, now_ns, *, after_ns):
    try:
        return _observation(report, exit_code, identity, now_ns, after_ns=after_ns)
    except (KeyError, TypeError, AttributeError, IndexError) as error:
        raise RuntimeError("CSI plugin report is structurally invalid") from error


def _observation(report, exit_code, identity, now_ns, *, after_ns):
    if report.get("schema_version") != "view-storage/v1alpha1" or report["query"]["require_metrics"] is not True:
        raise RuntimeError("CSI plugin did not honor the required-metrics contract")
    if report["selection"]["complete"] is not True or type(report["selection"]["matched_count"]) is not int or report["selection"]["matched_count"] != 1 or len(report["records"]) != 1:
        raise RuntimeError("CSI plugin did not retain the complete selected claim")
    record = report["records"][0]
    if report["collection"]["scope"]["namespace"] != NAMESPACE or record["phase"] != "Bound":
        raise RuntimeError("CSI report scope/phase differs from the ready fixture")
    if (record["kind"], record["namespace"], record["name"], record["uid"], record["volume_mode"]) != ("pvc", NAMESPACE, "data", identity["uid"], "Filesystem"):
        raise RuntimeError("CSI report claim identity or volume mode differs from the fixture")
    if record["pv_name"] != identity["volume"] or record["driver"] != DRIVER or record["storage_class"] != NAMESPACE:
        raise RuntimeError("CSI report omitted the known volume/driver/class identity")
    if len(record["consumer_ids"]) != 2 or set(record["consumer_ids"]) != identity["consumers"] or type(record["active_consumer_count"]) is not int or type(record["terminating_consumer_count"]) is not int or record["active_consumer_count"] != 2 or record["terminating_consumer_count"] != 0:
        raise RuntimeError("CSI shared consumers were lost or counted incorrectly")
    consumers = report["consumers"]
    if len(consumers) != 2 or {pod["id"] for pod in consumers} != identity["consumers"] or any(pod["node"] != identity["node"] or pod["reference_state"] != "active" for pod in consumers):
        raise RuntimeError("CSI consumer provenance differs from the ready fixture")
    if counter(record["requested_bytes"]) != 1024**3 or counter(report["summary"]["provisioned"]["known_sum_bytes"]) != counter(record["provisioned_bytes"]):
        raise RuntimeError("CSI report duplicates logical capacity or changes the claim request")
    cost = report["collection"]["cost"]
    if type(cost["node_targets"]) is not int or type(cost["node_requests"]) is not int or cost["node_targets"] != 1 or not 1 <= cost["node_requests"] <= 3:
        raise RuntimeError("CSI report does not use one logical node target")
    metric = record["filesystem"]
    if metric["status"] != "ok":
        if exit_code == 0:
            raise RuntimeError("CSI required metrics passed without a complete byte tuple")
        return None
    if exit_code != 0:
        raise RuntimeError("CSI complete required byte metrics did not produce exit 0")
    fields = ("capacity_bytes", "used_bytes", "available_bytes")
    if any(metric["field_status"][field] != "ok" for field in fields):
        raise RuntimeError("CSI byte tuple has invalid or missing fields")
    capacity, used, available = (counter(metric[field]) for field in fields)
    if capacity == 0 or used > capacity or available > capacity:
        raise RuntimeError("CSI byte tuple has inconsistent counters")
    source = metric["source"]
    if source["provider"] != "kubelet" or source["node"] != identity["node"] or source["pod_id"] not in identity["consumers"] or source["volume_name"] != "data":
        raise RuntimeError("CSI byte tuple has incompatible provenance")
    observed = timestamp_ns(metric["observed_at"])
    if now_ns - observed > MAX_AGE_SECONDS * 10**9 or observed - now_ns > 30 * 10**9:
        return None
    if observed <= after_ns:
        return None
    return {"observed_at": metric["observed_at"], "observed_ns": observed, "capacity": capacity, "used": used, "available": available,
            "inode_status": record["inodes"]["status"]}


def controllers_ready(kubectl, config, lock):
    snapshot = lock["snapshot_controller"]
    for kind, namespace, name in (("statefulset", "default", "csi-hostpathplugin"), ("deployment", snapshot["namespace"], snapshot["name"])):
        controller = json.loads(kubectl(config, "-n", namespace, "get", kind, name, "-o", "json"))
        desired, status = controller["spec"].get("replicas", 1), controller.get("status", {})
        if desired < 1 or status.get("observedGeneration", 0) < controller["metadata"]["generation"] or status.get("readyReplicas", 0) < desired or status.get("updatedReplicas", 0) < desired:
            return None
    return True


def verify(kubectl, config, binary, evidence, run_id, lock, *, query_config=None, monotonic=time.monotonic, sleep=time.sleep, utc_ns=time.time_ns):
    try:
        return _verify(kubectl, config, binary, evidence, run_id, lock, query_config=query_config, monotonic=monotonic, sleep=sleep, utc_ns=utc_ns)
    except (KeyError, TypeError, AttributeError, IndexError) as error:
        raise RuntimeError("CSI fixture or plugin report is structurally invalid") from error


def _verify(kubectl, config, binary, evidence, run_id, lock, *, query_config, monotonic, sleep, utc_ns):
    binary, evidence = Path(binary).resolve(), Path(evidence)
    if not binary.is_file() or not os.access(binary, os.X_OK):
        raise RuntimeError("build the real plugin before CSI verification, or supply --binary")
    deadline = monotonic() + 360
    poll(lambda: controllers_ready(kubectl, config, lock), deadline, "CSI driver/snapshot controllers did not become ready", monotonic=monotonic, sleep=sleep)
    identity = poll(lambda: fixture_identity(kubectl, config), deadline, "CSI bound/mounted fixture did not become ready", monotonic=monotonic, sleep=sleep)
    driver = json.loads(kubectl(config, "-n", "default", "get", "statefulset", "csi-hostpathplugin", "-o", "json"))
    containers = driver["spec"]["template"]["spec"]["containers"]
    if [container["image"] for container in containers if container.get("name") == "hostpath"] != [lock["driver_image"]] or not {container["image"] for container in containers}.issubset({image["image"] for image in lock["images"]}):
        raise RuntimeError("CSI runtime controller images differ from the reviewed lock")
    (evidence / "csi-environment.json").write_text(json.dumps({"run_id": run_id, "driver": DRIVER, "claim_uid": identity["uid"],
        "node": identity["node"], "driver_images": [container["image"] for container in driver["spec"]["template"]["spec"]["containers"]]}, indent=2) + "\n")
    def exec_probe(pod, *args):
        return kubectl(config, "-n", NAMESPACE, "exec", pod, "-c", "probe", "--", *args)
    failed = False
    try:
        exec_probe("writer", "rm", "-f", PROBE)
        exec_probe("writer", "sync")
        cleared = utc_ns()
        def sample(name, after_ns, baseline=None):
            report, exit_code = query(binary, query_config or config)
            value = observation(report, exit_code, identity, utc_ns(), after_ns=after_ns)
            # Never persist an unvalidated response as a support report.
            (evidence / f"csi-{name}-report.json").write_text(json.dumps(report, indent=2) + "\n")
            if value is not None and baseline is not None:
                if value["observed_ns"] <= baseline["observed_ns"] or value["capacity"] != baseline["capacity"] or value["used"] < baseline["used"] or value["available"] > baseline["available"]:
                    return None
                if (value["used"], value["available"]) == (baseline["used"], baseline["available"]):
                    return None
            return value
        before = poll(lambda: sample("before", cleared), deadline, "CSI profile produced no fresh attributable baseline byte tuple", monotonic=monotonic, sleep=sleep)
        if before["available"] < 4 * WRITE_BYTES:
            raise RuntimeError("CSI backing filesystem lacks free space for the bounded probe")
        exec_probe("writer", "dd", "if=/dev/zero", "of=" + PROBE, "bs=1048576", "count=16", "conv=fsync")
        for pod in PODS:
            if exec_probe(pod, "wc", "-c", PROBE).split()[0] != str(WRITE_BYTES):
                raise RuntimeError("CSI probe file size differs across shared consumers")
        written = utc_ns()
        after = poll(lambda: sample("after", written, before), deadline, "CSI profile produced no advancing compatible byte tuple after the bounded write", monotonic=monotonic, sleep=sleep)
        result = {"status": "passed", "run_id": run_id, "plugin_sha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
                  "write_bytes": WRITE_BYTES, "before": before, "after": after, "inode_status": after["inode_status"],
                  "scope": "single-node CSI hostpath Filesystem; no cross-node RWX or raw-block claim"}
    except BaseException:
        failed = True
        raise
    finally:
        try:
            exec_probe("writer", "rm", "-f", PROBE)
        except (OSError, RuntimeError, subprocess.SubprocessError):
            (evidence / "csi-probe-cleanup.json").write_text(json.dumps({"status": "failed", "run_id": run_id}) + "\n")
            if not failed:
                raise RuntimeError("CSI probe cleanup failed; inspect the owned cluster")
    return result
