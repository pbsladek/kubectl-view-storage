"""Owned kind NFS CSI fixture; normal execution uses JSON and the standard library."""

import hashlib
import json
from pathlib import Path
import re
import time
import os

ROOT = Path(__file__).resolve().parents[1]
NAMESPACE = "view-storage-nfs"
DRIVER = "nfs.csi.k8s.io"


def validate_lock(vendor, versions):
    vendor = Path(vendor)
    if vendor.is_symlink():
        raise RuntimeError("NFS vendor directory must not be a symlink")
    vendor = vendor.resolve()
    path = vendor / "lock.json"
    if (
        path.is_symlink()
        or not path.is_file()
        or hashlib.sha256(path.read_bytes()).hexdigest()
        != versions.get("nfs_lock_sha256")
    ):
        raise RuntimeError("NFS lock checksum does not match the reviewed versions")
    try:
        lock = json.loads(path.read_text())
        if (
            lock["status"] != "reviewed"
            or lock["driver_release"] != versions["nfs_csi_release"]
            or not re.fullmatch("[0-9a-f]{40}", lock["source_revision"])
        ):
            raise RuntimeError("NFS release/source selection is not reviewed")
        for entry in [
            *lock["manifests"],
            *lock["licenses"],
            lock["server_recipe_reference"],
        ]:
            selected = vendor / entry["file"]
            if (
                selected.is_symlink()
                or not selected.resolve().is_relative_to(vendor)
                or hashlib.sha256(selected.read_bytes()).hexdigest() != entry["sha256"]
            ):
                raise RuntimeError("NFS bundle file/hash changed")
        images = {}
        for image in lock["images"]:
            if not re.fullmatch(r"[^\s]+@sha256:[0-9a-f]{64}", image["image"]) or any(
                not re.fullmatch(
                    "sha256:[0-9a-f]{64}", image["platform_digests"].get(arch, "")
                )
                for arch in ("amd64", "arm64")
            ):
                raise RuntimeError("NFS image architecture lock is incomplete")
            images[image["image"]] = image
        if not lock["driver_image"].startswith(
            "registry.k8s.io/sig-storage/nfsplugin:"
            + versions["nfs_csi_release"]
            + "@sha256:"
        ):
            raise RuntimeError("NFS driver release image differs")
        if (
            lock["driver_image"] not in images
            or lock["server_image"] not in images
            or lock["workload_image"] not in images
        ):
            raise RuntimeError("NFS driver/server image is unlocked")

        def inspect(value):
            if isinstance(value, dict):
                for key, child in value.items():
                    if key in ("containers", "initContainers", "ephemeralContainers"):
                        if any(c["image"] not in images for c in child):
                            raise RuntimeError(
                                "NFS manifest references an unlocked image"
                            )
                    inspect(child)
            elif isinstance(value, list):
                for child in value:
                    inspect(child)

        resources = set()
        for entry in lock["manifests"]:
            document = json.loads((vendor / entry["file"]).read_text())
            inspect(document)
            resources.update(
                (o["kind"], o["metadata"].get("namespace"), o["metadata"]["name"])
                for o in document["items"]
            )
        expected = {
            ("CSIDriver", None, DRIVER),
            ("Deployment", "kube-system", "csi-nfs-controller"),
            ("DaemonSet", "kube-system", "csi-nfs-node"),
        }
        if not expected.issubset(resources):
            raise RuntimeError("NFS driver/controller/node resources are missing")
        return lock
    except (KeyError, TypeError, ValueError) as error:
        raise RuntimeError("NFS lock is structurally invalid") from error


def render_fixture(lock, run_id):
    ns = NAMESPACE
    objects = [{"apiVersion": "v1", "kind": "Namespace", "metadata": {"name": ns}}]
    config = """NFS_CORE_PARAM { Protocols = 4; NFS_Port = 2049; Enable_NLM = false; Enable_RQUOTA = false; fsid_device = true; }
EXPORT {
  Export_Id = 1; Path = /export/data; Pseudo = /;
  Access_Type = RW; Squash = No_Root_Squash; SecType = sys;
  FSAL { Name = VFS; }
}
"""
    objects.append(
        {
            "apiVersion": "v1",
            "kind": "ConfigMap",
            "metadata": {"name": "ganesha", "namespace": ns},
            "data": {"ganesha.conf": config},
        }
    )
    objects.append(
        {
            "apiVersion": "apps/v1",
            "kind": "Deployment",
            "metadata": {"name": "nfs-server", "namespace": ns},
            "spec": {
                "replicas": 1,
                "strategy": {"type": "Recreate"},
                "selector": {"matchLabels": {"app": "nfs-server"}},
                "template": {
                    "metadata": {"labels": {"app": "nfs-server"}},
                    "spec": {
                        "nodeSelector": {"view-storage.test/nfs-server": "true"},
                        "tolerations": [
                            {
                                "key": "node-role.kubernetes.io/control-plane",
                                "operator": "Exists",
                                "effect": "NoSchedule",
                            }
                        ],
                        "containers": [
                            {
                                "name": "server",
                                "image": lock["server_image"],
                                "command": [
                                    "/bin/sh",
                                    "-ec",
                                    "mkdir -p /export/data /run/ganesha /var/lib/nfs/ganesha; chmod 0777 /export/data; rpcbind -w; mkdir -p /run/dbus; dbus-daemon --system --fork; exec /usr/bin/ganesha.nfsd -F -p /run/ganesha/ganesha.pid -L /dev/stdout -f /etc/ganesha/ganesha.conf",
                                ],
                                "securityContext": {"privileged": True},
                                "resources": {
                                    "requests": {"cpu": "10m", "memory": "32Mi"},
                                    "limits": {"memory": "256Mi"},
                                },
                                "ports": [{"containerPort": 2049}],
                                "readinessProbe": {
                                    "tcpSocket": {"port": 2049},
                                    "initialDelaySeconds": 1,
                                    "periodSeconds": 2,
                                },
                                "volumeMounts": [
                                    {"name": "export", "mountPath": "/export"},
                                    {
                                        "name": "config",
                                        "mountPath": "/etc/ganesha",
                                        "readOnly": True,
                                    },
                                ],
                            }
                        ],
                        "volumes": [
                            {
                                "name": "export",
                                "emptyDir": {"sizeLimit": "128Mi"},
                            },
                            {"name": "config", "configMap": {"name": "ganesha"}},
                        ],
                    },
                },
            },
        }
    )
    objects.append(
        {
            "apiVersion": "v1",
            "kind": "Service",
            "metadata": {"name": "nfs-server", "namespace": ns},
            "spec": {
                "selector": {"app": "nfs-server"},
                "ports": [
                    {"name": "nfs", "port": 2049, "targetPort": 2049, "protocol": "TCP"}
                ],
            },
        }
    )
    objects.append(
        {
            "apiVersion": "storage.k8s.io/v1",
            "kind": "StorageClass",
            "metadata": {"name": ns},
            "provisioner": DRIVER,
            "parameters": {
                "server": f"nfs-server.{ns}.svc.cluster.local",
                "share": "/",
                "mountPermissions": "0777",
            },
            "mountOptions": ["nfsvers=4.1", "soft", "timeo=50", "retrans=2"],
            "reclaimPolicy": "Delete",
            "volumeBindingMode": "Immediate",
        }
    )
    objects.append(
        {
            "apiVersion": "v1",
            "kind": "PersistentVolumeClaim",
            "metadata": {"name": "data", "namespace": ns},
            "spec": {
                "storageClassName": ns,
                "volumeMode": "Filesystem",
                "accessModes": ["ReadWriteMany"],
                "resources": {"requests": {"storage": "1Gi"}},
            },
        }
    )
    for name in ("writer", "peer"):
        objects.append(
            {
                "apiVersion": "v1",
                "kind": "Pod",
                "metadata": {
                    "name": name,
                    "namespace": ns,
                    "labels": {"app": "view-storage-nfs-probe"},
                },
                "spec": {
                    "automountServiceAccountToken": False,
                    "nodeSelector": {"view-storage.test/nfs-client": "true"},
                    "affinity": {
                        "podAntiAffinity": {
                            "requiredDuringSchedulingIgnoredDuringExecution": [
                                {
                                    "labelSelector": {
                                        "matchLabels": {"app": "view-storage-nfs-probe"}
                                    },
                                    "topologyKey": "kubernetes.io/hostname",
                                }
                            ]
                        }
                    },
                    "securityContext": {
                        "runAsUser": 1000,
                        "runAsGroup": 1000,
                        "runAsNonRoot": True,
                    },
                    "containers": [
                        {
                            "name": "probe",
                            "image": lock["workload_image"],
                            "command": ["sh", "-c", "while :; do sleep 3600; done"],
                            "securityContext": {
                                "allowPrivilegeEscalation": False,
                                "readOnlyRootFilesystem": True,
                                "capabilities": {"drop": ["ALL"]},
                            },
                            "resources": {
                                "requests": {"cpu": "5m", "memory": "8Mi"},
                                "limits": {"memory": "32Mi"},
                            },
                            "volumeMounts": [{"name": "data", "mountPath": "/data"}],
                        }
                    ],
                    "volumes": [
                        {"name": "data", "persistentVolumeClaim": {"claimName": "data"}}
                    ],
                },
            }
        )
    for obj in objects:
        obj["metadata"].setdefault("labels", {})["view-storage/run-id"] = run_id
    return {"apiVersion": "v1", "kind": "List", "items": objects}


def query(binary, config):
    import subprocess

    result = subprocess.run(
        [
            str(binary),
            "--kubeconfig",
            str(config),
            "-n",
            NAMESPACE,
            "--pvc",
            "data",
            "--require-metrics",
            "--max-metrics-age",
            "120s",
            "--timeout",
            "15s",
            "--request-timeout",
            "5s",
            "-g",
            "node,pod,pvc",
            "-o",
            "json",
        ],
        text=True,
        capture_output=True,
        timeout=25,
    )
    if result.returncode not in (0, 4):
        raise RuntimeError(f"NFS plugin query failed with exit {result.returncode}")
    try:
        return json.loads(result.stdout), result.returncode
    except ValueError as error:
        raise RuntimeError("NFS plugin output is not JSON") from error


def identity(kubectl, config):
    claim = json.loads(
        kubectl(config, "-n", NAMESPACE, "get", "pvc", "data", "-o", "json")
    )
    pods = json.loads(
        kubectl(
            config,
            "-n",
            NAMESPACE,
            "get",
            "pods",
            "-l",
            "app=view-storage-nfs-probe",
            "-o",
            "json",
        )
    )["items"]
    if len(pods) != 2 or {p["metadata"]["name"] for p in pods} != {"writer", "peer"}:
        raise RuntimeError("NFS fixture must have exactly two named consumers")
    if claim.get("status", {}).get("phase") != "Bound":
        return None
    if (
        claim["spec"].get("accessModes") != ["ReadWriteMany"]
        or claim["spec"].get("volumeMode", "Filesystem") != "Filesystem"
    ):
        raise RuntimeError("NFS fixture is not a real RWX filesystem claim")
    if any(
        not any(
            c.get("type") == "Ready" and c.get("status") == "True"
            for c in p.get("status", {}).get("conditions", [])
        )
        for p in pods
    ):
        return None
    mapping = {"pod:" + p["metadata"]["uid"]: p["spec"].get("nodeName") for p in pods}
    if len(mapping) != 2 or len(set(mapping.values())) != 2 or None in mapping.values():
        raise RuntimeError("NFS consumers must run on distinct nodes")
    pv = json.loads(
        kubectl(config, "get", "pv", claim["spec"]["volumeName"], "-o", "json")
    )
    if (
        pv["spec"].get("csi", {}).get("driver") != DRIVER
        or pv["spec"].get("claimRef", {}).get("uid") != claim["metadata"]["uid"]
    ):
        raise RuntimeError("NFS PV driver or binding differs from the live claim")
    return {
        "uid": claim["metadata"]["uid"],
        "volume": pv["metadata"]["name"],
        "pod_nodes": mapping,
    }


def observation(report, exit_code, fixture, now_ns, after_ns):
    try:
        return _observation(report, exit_code, fixture, now_ns, after_ns)
    except (KeyError, TypeError, AttributeError, IndexError) as error:
        raise RuntimeError("NFS plugin report is structurally invalid") from error


def _observation(report, exit_code, fixture, now_ns, after_ns):
    from dev_csi import counter, timestamp_ns

    if (
        report["schema_version"] != "view-storage/v1alpha1"
        or report["query"]["require_metrics"] is not True
    ):
        raise RuntimeError("NFS query contract differs")
    selection = report["selection"]
    records = report["records"]
    if (
        selection["complete"] is not True
        or type(selection["matched_count"]) is not int
        or selection["matched_count"] != 1
        or len(records) != 1
    ):
        raise RuntimeError("NFS selection is incomplete or duplicated")
    record = records[0]
    if (
        record["namespace"],
        record["name"],
        record["uid"],
        record["pv_name"],
        record["driver"],
        record["storage_class"],
        record["volume_mode"],
    ) != (
        NAMESPACE,
        "data",
        fixture["uid"],
        fixture["volume"],
        DRIVER,
        NAMESPACE,
        "Filesystem",
    ):
        raise RuntimeError("NFS report identity differs from live resources")
    if (
        len(record["consumer_ids"]) != 2
        or set(record["consumer_ids"]) != set(fixture["pod_nodes"])
        or record["active_consumer_count"] != 2
        or record["terminating_consumer_count"] != 0
    ):
        raise RuntimeError("NFS consumer references are missing or duplicated")
    consumers = {p["id"]: p["node"] for p in report["consumers"]}
    if len(report["consumers"]) != 2 or consumers != fixture["pod_nodes"]:
        raise RuntimeError("NFS report fabricated a node/consumer relationship")
    if counter(record["requested_bytes"]) != 1024**3 or counter(
        report["summary"]["provisioned"]["known_sum_bytes"]
    ) != counter(record["provisioned_bytes"]):
        raise RuntimeError("NFS shared claim capacity was counted more than once")
    groups = report["groups"]
    if (
        report["summary"]["overlapping_children"] is not True
        or len(groups) != 2
        or {group["value"] for group in groups} != set(fixture["pod_nodes"].values())
        or any(
            group["dimension"] != "node"
            or group["member_ids"] != [record["id"]]
            or group["totals"]["provisioned"]["known_sum_bytes"]
            != record["provisioned_bytes"]
            for group in groups
        )
    ):
        raise RuntimeError(
            "NFS node groups lost shared membership or the overlap warning"
        )
    if (
        report["collection"]["cost"]["node_targets"] != 2
        or not 2 <= report["collection"]["cost"]["node_requests"] <= 6
    ):
        raise RuntimeError("NFS collection did not target both consumer nodes")
    if record["filesystem"]["status"] != "ok":
        if exit_code == 0:
            raise RuntimeError(
                "NFS required metrics falsely passed without a fresh tuple"
            )
        return None
    if exit_code != 0:
        raise RuntimeError(
            "NFS complete byte tuple did not produce a successful strict report"
        )
    for family, fields in [
        ("filesystem", ("capacity_bytes", "used_bytes", "available_bytes")),
        ("inodes", ("inodes_total", "inodes_used", "inodes_free")),
    ]:
        metric = record[family]
        if metric["status"] != "ok":
            return None
        if any(metric["field_status"][key] != "ok" for key in fields):
            raise RuntimeError("NFS complete metric has unavailable fields")
        seen = set()
        for candidate in metric["candidates"]:
            source = candidate["source"]
            if (
                not source
                or source["provider"] != "kubelet"
                or source["pod_id"] not in fixture["pod_nodes"]
                or source["node"] != fixture["pod_nodes"][source["pod_id"]]
                or source["volume_name"] != "data"
            ):
                raise RuntimeError(
                    "NFS observation provenance does not match the mounted claim"
                )
            if candidate["status"] != "ok":
                continue
            observed = timestamp_ns(candidate["observed_at"])
            if (
                after_ns < observed <= now_ns + 30 * 10**9
                and now_ns - observed <= 120 * 10**9
            ):
                seen.add(source["node"])
        if seen != set(fixture["pod_nodes"].values()):
            return None
        capacity, used, available = (counter(metric[key]) for key in fields)
        if capacity == 0 or used > capacity or available > capacity:
            raise RuntimeError("NFS counters exceed their reported capacity")
        total = report["summary"][family]
        if total["physical_total"] is not False or any(
            total[key] != metric[key] for key in fields
        ):
            raise RuntimeError("NFS reference groups inflated filesystem totals")
    fs = record["filesystem"]
    return {
        "observed_at": fs["observed_at"],
        "observed_ns": timestamp_ns(fs["observed_at"]),
        "capacity": counter(fs["capacity_bytes"]),
        "used": counter(fs["used_bytes"]),
        "available": counter(fs["available_bytes"]),
        "inode_status": record["inodes"]["status"],
        "candidate_nodes": sorted(fixture["pod_nodes"].values()),
    }


def verify(
    kubectl,
    config,
    binary,
    evidence,
    run_id,
    lock,
    *,
    monotonic=time.monotonic,
    sleep=time.sleep,
    utc_ns=time.time_ns,
):
    from dev_csi import poll

    evidence = Path(evidence)
    binary = Path(binary).resolve()
    deadline = monotonic() + 420
    if not binary.is_file() or not os.access(binary, os.X_OK):
        raise RuntimeError("build the real plugin before NFS verification")
    allowed_images = {image["image"] for image in lock["images"]}
    for kind, namespace, name in (
        ("deployment", "kube-system", "csi-nfs-controller"),
        ("daemonset", "kube-system", "csi-nfs-node"),
        ("deployment", NAMESPACE, "nfs-server"),
    ):
        resource = json.loads(
            kubectl(config, "-n", namespace, "get", kind, name, "-o", "json")
        )
        images = [
            c["image"] for c in resource["spec"]["template"]["spec"]["containers"]
        ]
        expected = (
            lock["server_image"] if name == "nfs-server" else lock["driver_image"]
        )
        if expected not in images or not set(images).issubset(allowed_images):
            raise RuntimeError("NFS runtime images differ from the reviewed bundle")
    for name in ("csi-nfs-controller", "nfs-server"):
        ns = "kube-system" if name.startswith("csi-") else NAMESPACE
        kubectl(
            config,
            "-n",
            ns,
            "rollout",
            "status",
            "deployment/" + name,
            "--timeout=180s",
        )
    kubectl(
        config,
        "-n",
        "kube-system",
        "rollout",
        "status",
        "daemonset/csi-nfs-node",
        "--timeout=180s",
    )
    fixture = poll(
        lambda: identity(kubectl, config),
        deadline,
        "NFS RWX consumers did not become ready on distinct nodes",
        monotonic=monotonic,
        sleep=sleep,
    )
    path = "/data/view-storage-nfs-probe.bin"
    write_bytes = 16 * 1024**2

    def exec_probe(pod, *args):
        return kubectl(config, "-n", NAMESPACE, "exec", pod, "-c", "probe", "--", *args)

    def sample(name, after_ns, baseline=None):
        report, code = query(binary, config)
        value = observation(report, code, fixture, utc_ns(), after_ns)
        (evidence / f"nfs-{name}-report.json").write_text(
            json.dumps(report, indent=2) + "\n"
        )
        if (
            value
            and baseline
            and (
                value["observed_ns"] <= baseline["observed_ns"]
                or value["capacity"] != baseline["capacity"]
                or value["used"] < baseline["used"]
                or value["available"] > baseline["available"]
                or (value["used"], value["available"])
                == (baseline["used"], baseline["available"])
            )
        ):
            return None
        return value

    failed = False

    def refresh_driver():
        kubectl(
            config, "-n", "kube-system", "rollout", "restart", "daemonset/csi-nfs-node"
        )
        kubectl(
            config,
            "-n",
            "kube-system",
            "rollout",
            "status",
            "daemonset/csi-nfs-node",
            "--timeout=120s",
        )

    try:
        exec_probe("writer", "rm", "-f", path)
        exec_probe("writer", "sync")
        refresh_driver()
        cleared = utc_ns()
        before = poll(
            lambda: sample("before", cleared),
            deadline,
            "NFS produced no fresh byte/inode observations from both nodes",
            monotonic=monotonic,
            sleep=sleep,
        )
        if before["available"] < 4 * write_bytes:
            raise RuntimeError("NFS backing filesystem lacks safe probe headroom")
        exec_probe(
            "writer",
            "dd",
            "if=/dev/zero",
            "of=" + path,
            "bs=1048576",
            "count=16",
            "conv=fsync",
        )
        hashes = []
        for pod in ("writer", "peer"):
            if exec_probe(pod, "wc", "-c", path).split()[0] != str(write_bytes):
                raise RuntimeError("NFS file was not shared across the two nodes")
            hashes.append(exec_probe(pod, "sha256sum", path).split()[0])
        if (
            len(set(hashes)) != 1
            or hashes[0] != hashlib.sha256(bytes(write_bytes)).hexdigest()
        ):
            raise RuntimeError("NFS cross-node content differs")
        # v4.13.6 hardcodes a ten-minute NodeGetVolumeStats cache with no CLI
        # override. Restart only the owned node driver after the write: this
        # exercises recovery and invalidates that provider cache without a
        # ten-minute sleep in every CI cell. Kubelet still must collect fresh
        # observations from both real mounts. This does not certify TTL latency.
        refresh_driver()
        written = utc_ns()
        after = poll(
            lambda: sample("after", written, before),
            deadline,
            "NFS counters did not advance compatibly after the cross-node write",
            monotonic=monotonic,
            sleep=sleep,
        )
        for pod in ("writer", "peer"):
            if exec_probe(pod, "sha256sum", path).split()[0] != hashes[0]:
                raise RuntimeError("NFS content did not survive node driver recovery")
        return {
            "status": "passed",
            "run_id": run_id,
            "plugin_sha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
            "write_bytes": write_bytes,
            "cross_node_sha256": hashes[0],
            "before": before,
            "after": after,
            "stats_refresh": "owned_node_driver_restart; upstream fixed ten-minute cache; TTL latency not tested",
            "scope": "NFS CSI Filesystem with two real RWX consumer nodes; per-claim reported counters, not physical totals",
        }
    except BaseException:
        failed = True
        raise
    finally:
        try:
            exec_probe("writer", "rm", "-f", path)
        except BaseException:
            (evidence / "nfs-probe-cleanup.json").write_text(
                json.dumps({"status": "failed", "run_id": run_id}) + "\n"
            )
            if not failed:
                raise
