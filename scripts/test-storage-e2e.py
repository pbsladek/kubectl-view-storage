#!/usr/bin/env python3
"""Offline regressions for real storage gates and suite ownership/failure handling."""

import hashlib
import importlib.util
import json
from pathlib import Path
import shutil
import tempfile
import unittest
from unittest.mock import patch

import dev_local
import dev_nfs

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location(
    "storage_suite", ROOT / "scripts/e2e-storage.py"
)
suite = importlib.util.module_from_spec(spec)
spec.loader.exec_module(suite)
VERSIONS = json.loads((ROOT / "dev/versions.json").read_text())
IDENTITY = {
    "uid": "claim-uid",
    "volume": "pv-real",
    "pod_nodes": {"pod:writer-uid": "node-a", "pod:peer-uid": "node-b"},
}
NOW = 1794301200 * 10**9


def report():
    metric = {
        "status": "ok",
        "observed_at": "2026-11-10T09:00:00Z",
        "capacity_bytes": "1000",
        "used_bytes": "100",
        "available_bytes": "900",
    }
    inode = {
        "status": "ok",
        "observed_at": metric["observed_at"],
        "inodes_total": "100",
        "inodes_used": "10",
        "inodes_free": "90",
    }
    for value, fields in (
        (metric, ("capacity_bytes", "used_bytes", "available_bytes")),
        (inode, ("inodes_total", "inodes_used", "inodes_free")),
    ):
        value["field_status"] = {field: "ok" for field in fields}
        value["candidates"] = [
            {
                "status": "ok",
                "observed_at": value["observed_at"],
                "source": {
                    "provider": "kubelet",
                    "node": node,
                    "pod_id": pod,
                    "volume_name": "data",
                },
            }
            for pod, node in IDENTITY["pod_nodes"].items()
        ]
    record = {
        "namespace": dev_nfs.NAMESPACE,
        "name": "data",
        "uid": IDENTITY["uid"],
        "id": "pvc:" + IDENTITY["uid"],
        "pv_name": IDENTITY["volume"],
        "driver": dev_nfs.DRIVER,
        "storage_class": dev_nfs.NAMESPACE,
        "volume_mode": "Filesystem",
        "consumer_ids": list(IDENTITY["pod_nodes"]),
        "active_consumer_count": 2,
        "terminating_consumer_count": 0,
        "requested_bytes": str(1024**3),
        "provisioned_bytes": str(1024**3),
        "filesystem": metric,
        "inodes": inode,
    }
    return {
        "schema_version": "view-storage/v1alpha1",
        "query": {"require_metrics": True},
        "selection": {"complete": True, "matched_count": 1},
        "records": [record],
        "groups": [
            {
                "dimension": "node",
                "value": node,
                "member_ids": [record["id"]],
                "totals": {"provisioned": {"known_sum_bytes": str(1024**3)}},
            }
            for node in IDENTITY["pod_nodes"].values()
        ],
        "consumers": [
            {"id": pod, "node": node} for pod, node in IDENTITY["pod_nodes"].items()
        ],
        "collection": {"cost": {"node_targets": 2, "node_requests": 2}},
        "summary": {
            "overlapping_children": True,
            "provisioned": {"known_sum_bytes": str(1024**3)},
            "filesystem": dict(metric, physical_total=False),
            "inodes": dict(inode, physical_total=False),
        },
    }


class StorageGates(unittest.TestCase):
    def test_reviewed_bundle_and_two_architectures(self):
        lock = dev_nfs.validate_lock(ROOT / "dev/nfs", VERSIONS)
        self.assertEqual(len(lock["images"]), 6)
        self.assertIn(
            lock["workload_image"], {image["image"] for image in lock["images"]}
        )

    def test_lock_rejects_tampering_unlocked_images_missing_arch_and_escape(self):
        for fault in ("tamper", "arch", "driver", "workload", "escape"):
            with self.subTest(fault=fault), tempfile.TemporaryDirectory() as temp:
                vendor = Path(temp) / "nfs"
                shutil.copytree(ROOT / "dev/nfs", vendor)
                lock_path = vendor / "lock.json"
                lock = json.loads(lock_path.read_text())
                if fault == "arch":
                    lock["images"][0]["platform_digests"].pop("arm64")
                if fault == "driver":
                    lock["driver_image"] = lock["server_image"]
                if fault == "workload":
                    lock["workload_image"] = "unreviewed:latest"
                if fault == "escape":
                    lock["manifests"][0]["file"] = "../escape.json"
                lock_path.write_text(json.dumps(lock))
                versions = dict(
                    VERSIONS,
                    nfs_lock_sha256=hashlib.sha256(lock_path.read_bytes()).hexdigest(),
                )
                if fault == "tamper":
                    (vendor / lock["manifests"][0]["file"]).write_text("{}")
                with self.assertRaises(RuntimeError):
                    dev_nfs.validate_lock(vendor, versions)

    def test_nfs_is_rwx_with_enforced_cross_node_consumers_and_pinned_images(self):
        lock = dev_nfs.validate_lock(ROOT / "dev/nfs", VERSIONS)
        fixture = dev_nfs.render_fixture(lock, "test-run")
        claims = [o for o in fixture["items"] if o["kind"] == "PersistentVolumeClaim"]
        self.assertEqual(claims[0]["spec"]["accessModes"], ["ReadWriteMany"])
        for pod in [o for o in fixture["items"] if o["kind"] == "Pod"]:
            self.assertIn(
                "requiredDuringSchedulingIgnoredDuringExecution",
                pod["spec"]["affinity"]["podAntiAffinity"],
            )
            self.assertEqual(
                pod["spec"]["containers"][0]["image"], lock["workload_image"]
            )
            self.assertFalse(pod["spec"]["automountServiceAccountToken"])
        server = next(o for o in fixture["items"] if o["kind"] == "Deployment")
        command = server["spec"]["template"]["spec"]["containers"][0]["command"][-1]
        self.assertIn("rpcbind -w", command)
        self.assertIn("-p /run/ganesha/ganesha.pid", command)

    def test_local_path_does_not_duplicate_inventory_claims_and_pins_peer_to_writer(
        self,
    ):
        lock = json.loads((ROOT / "dev/csi/lock.json").read_text())
        items = dev_local.render_fixture(lock["workload_image"], "run")["items"]
        self.assertFalse(any(o["kind"] == "StorageClass" for o in items))
        claim = next(o for o in items if o["kind"] == "PersistentVolumeClaim")
        self.assertEqual(claim["metadata"]["namespace"], dev_local.NAMESPACE)
        self.assertEqual(claim["spec"]["storageClassName"], "view-storage-local")
        peer = next(o for o in items if o["metadata"]["name"] == "peer")
        self.assertTrue(
            peer["spec"]["affinity"]["podAffinity"][
                "requiredDuringSchedulingIgnoredDuringExecution"
            ]
        )

    def test_nfs_observation_requires_real_identity_fresh_both_node_counters_and_single_capacity(
        self,
    ):
        self.assertIsNotNone(
            dev_nfs.observation(report(), 0, IDENTITY, NOW, NOW - 10**9)
        )
        for fault in (
            "uid",
            "nodes",
            "capacity",
            "source",
            "fields",
            "duplicates",
            "physical",
            "malformed",
            "overlap",
        ):
            with self.subTest(fault=fault):
                value = report()
                record = value["records"][0]
                if fault == "uid":
                    record["uid"] = "wrong"
                if fault == "nodes":
                    value["collection"]["cost"]["node_targets"] = 1
                if fault == "capacity":
                    value["summary"]["provisioned"]["known_sum_bytes"] = str(
                        2 * 1024**3
                    )
                if fault == "source":
                    record["filesystem"]["candidates"][0]["source"]["node"] = "other"
                if fault == "fields":
                    record["filesystem"]["field_status"]["used_bytes"] = "missing"
                if fault == "duplicates":
                    record["consumer_ids"].append(record["consumer_ids"][0])
                if fault == "physical":
                    value["summary"]["filesystem"]["physical_total"] = True
                if fault == "malformed":
                    value.pop("records")
                if fault == "overlap":
                    value["summary"]["overlapping_children"] = False
                with self.assertRaises(RuntimeError):
                    dev_nfs.observation(value, 0, IDENTITY, NOW, NOW - 10**9)
        for fault in ("stale", "single-node", "inodes"):
            value = report()
            if fault == "stale":
                value["records"][0]["filesystem"]["candidates"][0]["observed_at"] = (
                    "2020-01-01T00:00:00Z"
                )
            if fault == "single-node":
                value["records"][0]["filesystem"]["candidates"].pop()
            if fault == "inodes":
                value["records"][0]["inodes"]["status"] = "missing"
            self.assertIsNone(dev_nfs.observation(value, 0, IDENTITY, NOW, NOW - 10**9))

    def test_missing_metrics_cannot_false_pass(self):
        value = report()
        value["records"][0]["filesystem"]["status"] = "missing"
        with self.assertRaisesRegex(RuntimeError, "falsely passed"):
            dev_nfs.observation(value, 0, IDENTITY, NOW, NOW - 10**9)
        self.assertIsNone(dev_nfs.observation(value, 4, IDENTITY, NOW, NOW - 10**9))

    def test_local_unknown_is_null_and_strict_exits_four(self):
        value = report()
        record = value["records"][0]
        record.update(
            namespace=dev_local.NAMESPACE,
            driver=dev_local.DRIVER,
            phase="Bound",
            requested_bytes=str(128 * 1024**2),
        )
        record["filesystem"].update(
            status="missing", capacity_bytes=None, used_bytes=None, available_bytes=None
        )
        for strict, code in ((True, 4), (False, 0)):
            value["query"]["require_metrics"] = strict
            self.assertEqual(
                dev_local.validate_report(value, code, IDENTITY, strict), "missing"
            )
        with self.assertRaises(RuntimeError):
            dev_local.validate_report(value, 4, IDENTITY, False)
        record["filesystem"]["used_bytes"] = "0"
        with self.assertRaisesRegex(RuntimeError, "remain unknown"):
            dev_local.validate_report(value, 0, IDENTITY, False)

    def test_suite_cleans_owned_partial_failure_and_never_deletes_preexisting_profile(
        self,
    ):
        for failure in (False, True, "preexisting"):
            with self.subTest(failure=failure), tempfile.TemporaryDirectory() as temp:
                root = Path(temp)
                binary = root / "plugin"
                binary.write_text("test bytes")
                binary.chmod(0o755)
                state = root / "owned.json"
                selected = {}

                def create(profile, **kwargs):
                    selected["run_id"] = (
                        kwargs["run_id"] if failure != "preexisting" else "someone-else"
                    )
                    state.write_text("{}")
                    if failure:
                        raise RuntimeError("fixture did not pass")

                with (
                    patch.object(suite, "ROOT", root),
                    patch.object(suite, "user_configs", return_value={}),
                    patch.object(suite.cluster, "create", side_effect=create),
                    patch.object(
                        suite.cluster, "paths", return_value=(None, None, state)
                    ),
                    patch.object(
                        suite.cluster,
                        "read_state",
                        side_effect=lambda _: (None, None, None, selected),
                    ),
                    patch.object(
                        suite.cluster, "delete", side_effect=lambda _: state.unlink()
                    ) as delete,
                ):
                    self.assertEqual(
                        suite.run_suite(binary, ["nfs-csi"]), failure is False
                    )
                    self.assertEqual(
                        delete.call_count, 0 if failure == "preexisting" else 1
                    )
                receipt = json.loads(
                    next((root / "artifacts").glob("*/suite.json")).read_text()
                )
                self.assertEqual(
                    receipt["status"], "passed" if failure is False else "failed"
                )

    def test_nfs_deadline_cannot_pass_and_attempts_probe_cleanup(self):
        lock = json.loads((ROOT / "dev/nfs/lock.json").read_text())
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            binary = root / "plugin"
            binary.write_text("fixture")
            binary.chmod(0o755)
            clock = [0]

            def sleep(seconds):
                clock[0] += seconds

            with (
                patch.object(dev_nfs, "identity", return_value=IDENTITY),
                patch.object(dev_nfs, "query", return_value=({}, 4)),
                patch.object(dev_nfs, "observation", return_value=None),
            ):
                calls = []

                def kubectl(*args):
                    calls.append(args)
                    if "get" in args:
                        image = (
                            lock["server_image"]
                            if "nfs-server" in args
                            else lock["driver_image"]
                        )
                        return json.dumps(
                            {
                                "spec": {
                                    "template": {
                                        "spec": {"containers": [{"image": image}]}
                                    }
                                }
                            }
                        )
                    return ""

                with self.assertRaisesRegex(RuntimeError, "no fresh"):
                    dev_nfs.verify(
                        kubectl,
                        "owned-config",
                        binary,
                        root,
                        "run",
                        lock,
                        monotonic=lambda: clock[0],
                        sleep=sleep,
                        utc_ns=lambda: NOW,
                    )
                self.assertEqual(clock[0], 420)
                self.assertEqual(
                    calls[-1][-3:], ("rm", "-f", "/data/view-storage-nfs-probe.bin")
                )


if __name__ == "__main__":
    unittest.main()
