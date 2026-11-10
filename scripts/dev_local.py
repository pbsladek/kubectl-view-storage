"""Local-path data and persistence gate, alongside the inventory/RBAC fixture."""

import hashlib
import json
from pathlib import Path
import subprocess

import dev_csi

NAMESPACE = "view-storage-local-path"
DRIVER = "rancher.io/local-path"
PROBE = "/data/view-storage-local-probe.bin"
WRITE_BYTES = 16 * 1024**2


def render_fixture(image, run_id):
    fixture = dev_csi.render_fixture(image, run_id)
    fixture = json.loads(json.dumps(fixture).replace(dev_csi.NAMESPACE, NAMESPACE))
    fixture["items"] = [
        item for item in fixture["items"] if item["kind"] != "StorageClass"
    ]
    for item in fixture["items"]:
        if item["kind"] == "PersistentVolumeClaim":
            item["spec"]["storageClassName"] = "view-storage-local"
            item["spec"]["resources"]["requests"]["storage"] = "128Mi"
        if item["kind"] == "Pod" and item["metadata"]["name"] == "peer":
            item["spec"]["affinity"] = {
                "podAffinity": {
                    "requiredDuringSchedulingIgnoredDuringExecution": [
                        {
                            "labelSelector": {
                                "matchLabels": {"view-storage/writer": run_id}
                            },
                            "topologyKey": "kubernetes.io/hostname",
                        }
                    ]
                }
            }
        if item["kind"] == "Pod" and item["metadata"]["name"] == "writer":
            item["metadata"]["labels"]["view-storage/writer"] = run_id
    return fixture


def query(binary, config, strict=False):
    command = [
        str(Path(binary).resolve()),
        "--kubeconfig",
        str(config),
        "-n",
        NAMESPACE,
        "--pvc",
        "data",
        "-u",
        "--timeout",
        "15s",
        "--request-timeout",
        "5s",
        "-g",
        "pod,pvc",
        "-o",
        "json",
    ]
    if strict:
        command.append("--require-metrics")
    result = subprocess.run(command, text=True, capture_output=True, timeout=25)
    if result.returncode not in (0, 4):
        raise RuntimeError(f"local-path plugin failed with exit {result.returncode}")
    try:
        return json.loads(result.stdout), result.returncode
    except ValueError as error:
        raise RuntimeError("local-path plugin output is not JSON") from error


def validate_report(report, code, fixture, strict):
    try:
        records = report["records"]
        if (
            report["selection"]["complete"] is not True
            or report["selection"]["matched_count"] != 1
            or len(records) != 1
        ):
            raise RuntimeError("local-path selection must contain one complete claim")
        record = records[0]
        if (
            record["namespace"],
            record["name"],
            record["uid"],
            record["pv_name"],
            record["driver"],
            record["phase"],
        ) != (NAMESPACE, "data", fixture["uid"], fixture["volume"], DRIVER, "Bound"):
            raise RuntimeError(
                "local-path report identity/binding differs from live resources"
            )
        consumers = {pod["id"]: pod["node"] for pod in report["consumers"]}
        if (
            consumers != fixture["pod_nodes"]
            or len(record["consumer_ids"]) != 2
            or set(record["consumer_ids"]) != set(consumers)
            or record["active_consumer_count"] != 2
        ):
            raise RuntimeError("local-path report lost or duplicated mounted consumers")
        if (
            record["requested_bytes"] != str(128 * 1024**2)
            or report["summary"]["provisioned"]["known_sum_bytes"]
            != record["provisioned_bytes"]
        ):
            raise RuntimeError("local-path logical capacity was changed or duplicated")
        metric = record["filesystem"]
        if metric["status"] != "ok" and any(
            metric[field] is not None
            for field in ("capacity_bytes", "used_bytes", "available_bytes")
        ):
            raise RuntimeError("local-path unavailable counters must remain unknown")
        expected = 4 if strict and metric["status"] != "ok" else 0
        if code != expected or report["query"]["require_metrics"] is not strict:
            raise RuntimeError("local-path missing metrics exit contract differs")
        return metric["status"]
    except (KeyError, TypeError, IndexError) as error:
        raise RuntimeError("local-path report is structurally invalid") from error


def verify(kubectl, config, binary, evidence, run_id, fixture_path):
    evidence = Path(evidence)
    for pod in ("writer", "peer"):
        kubectl(
            config,
            "-n",
            NAMESPACE,
            "wait",
            "--for=condition=Ready",
            "pod/" + pod,
            "--timeout=120s",
        )
    claim = json.loads(
        kubectl(config, "-n", NAMESPACE, "get", "pvc", "data", "-o", "json")
    )
    volume = json.loads(
        kubectl(config, "get", "pv", claim["spec"]["volumeName"], "-o", "json")
    )
    if (
        claim["status"]["phase"] != "Bound"
        or volume["spec"]["claimRef"]["uid"] != claim["metadata"]["uid"]
        or volume["metadata"]["annotations"].get("pv.kubernetes.io/provisioned-by")
        != DRIVER
    ):
        raise RuntimeError("local-path fixture binding/provider differs")

    def pod_identity():
        pods = [
            json.loads(
                kubectl(config, "-n", NAMESPACE, "get", "pod", pod, "-o", "json")
            )
            for pod in ("writer", "peer")
        ]
        nodes = {p["spec"]["nodeName"] for p in pods}
        if len(nodes) != 1:
            raise RuntimeError("local-path RWO consumers must share a node")
        return {"pod:" + p["metadata"]["uid"]: p["spec"]["nodeName"] for p in pods}

    original = pod_identity()

    def execute(pod, *args):
        return kubectl(config, "-n", NAMESPACE, "exec", pod, "-c", "probe", "--", *args)

    expected_hash = hashlib.sha256(bytes(WRITE_BYTES)).hexdigest()
    failed = False
    try:
        execute(
            "writer",
            "dd",
            "if=/dev/zero",
            "of=" + PROBE,
            "bs=1048576",
            "count=16",
            "conv=fsync",
        )
        for pod in ("writer", "peer"):
            if (
                execute(pod, "wc", "-c", PROBE).split()[0] != str(WRITE_BYTES)
                or execute(pod, "sha256sum", PROBE).split()[0] != expected_hash
            ):
                raise RuntimeError("local-path shared content differs")
        kubectl(
            config,
            "-n",
            NAMESPACE,
            "delete",
            "pod",
            "writer",
            "--wait=true",
            "--timeout=60s",
        )
        kubectl(config, "apply", "-f", str(fixture_path))
        kubectl(
            config,
            "-n",
            NAMESPACE,
            "wait",
            "--for=condition=Ready",
            "pod/writer",
            "--timeout=120s",
        )
        current = pod_identity()
        if set(current) == set(original) or set(current.values()) != set(
            original.values()
        ):
            raise RuntimeError(
                "local-path persistence requires a replacement pod on the bound node"
            )
        if execute("writer", "sha256sum", PROBE).split()[0] != expected_hash:
            raise RuntimeError("local-path data did not survive pod recreation")
        fixture = {
            "uid": claim["metadata"]["uid"],
            "volume": volume["metadata"]["name"],
            "pod_nodes": current,
        }
        statuses = {}
        for strict in (False, True):
            report, code = query(binary, config, strict)
            statuses[str(strict).lower()] = validate_report(
                report, code, fixture, strict
            )
            (
                evidence / f"local-path-{'strict' if strict else 'default'}-report.json"
            ).write_text(json.dumps(report, indent=2) + "\n")
        result = {
            "status": "passed",
            "run_id": run_id,
            "plugin_sha256": hashlib.sha256(Path(binary).read_bytes()).hexdigest(),
            "write_bytes": WRITE_BYTES,
            "content_sha256": expected_hash,
            "pod_recreated": True,
            "filesystem_status": statuses,
            "scope": "local-path binding, same-node sharing, persistence and unknown-metrics behavior",
        }
        (evidence / "local-path-validation.json").write_text(
            json.dumps(result, indent=2) + "\n"
        )
        return result
    except BaseException:
        failed = True
        raise
    finally:
        try:
            execute("peer", "rm", "-f", PROBE)
        except BaseException:
            (evidence / "local-path-probe-cleanup.json").write_text(
                '{"status":"failed"}\n'
            )
            if not failed:
                raise
