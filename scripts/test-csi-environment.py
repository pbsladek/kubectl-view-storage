#!/usr/bin/env python3
"""Offline CSI harness checks; these do not establish live CSI support."""
import copy
import hashlib
import importlib.util
import json
from pathlib import Path
import shutil
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch

import dev_csi as csi

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("dev_cluster", ROOT / "scripts/dev-cluster.py")
cluster = importlib.util.module_from_spec(spec)
spec.loader.exec_module(cluster)
RUN = "11111111-1111-4111-8111-111111111111"
DRIVER_IMAGE = "example.invalid/hostpath:v1.19.0@sha256:" + "a" * 64
WORKLOAD_IMAGE = "example.invalid/busybox:1@sha256:" + "b" * 64
LOCK = {"snapshot_controller": {"namespace": "kube-system", "name": "snapshot-controller"}, "driver_image": DRIVER_IMAGE, "workload_image": WORKLOAD_IMAGE, "images": [{"image": DRIVER_IMAGE}, {"image": WORKLOAD_IMAGE}]}
IDENTITY = {"uid": "claim", "node": "node-a", "consumers": {"pod:writer", "pod:peer"}, "volume": "pv-data"}
BASE = csi.timestamp_ns("2026-11-07T12:00:00Z")


class Clock:
    def __init__(self):
        self.time = 0

    def monotonic(self):
        return self.time

    def sleep(self, seconds):
        self.time += seconds


def report(second=1, *, used=40 * 1024**3, available=50 * 1024**3):
    metric = {"status": "ok", "capacity_bytes": str(100 * 1024**3), "used_bytes": str(used), "available_bytes": str(available),
              "field_status": {field: "ok" for field in ("capacity_bytes", "used_bytes", "available_bytes")},
              "observed_at": f"2026-11-07T12:00:{second:02}Z",
              "source": {"provider": "kubelet", "node": "node-a", "pod_id": "pod:writer", "volume_name": "data"}}
    return {"schema_version": "view-storage/v1alpha1", "query": {"require_metrics": True},
            "selection": {"complete": True, "matched_count": 1},
            "collection": {"scope": {"namespace": csi.NAMESPACE}, "cost": {"node_targets": 1, "node_requests": 1}},
            "records": [{"kind": "pvc", "namespace": csi.NAMESPACE, "name": "data", "uid": "claim", "phase": "Bound",
                         "volume_mode": "Filesystem", "pv_name": "pv-data", "driver": csi.DRIVER, "storage_class": csi.NAMESPACE,
                         "consumer_ids": ["pod:writer", "pod:peer"], "active_consumer_count": 2, "terminating_consumer_count": 0,
                         "requested_bytes": str(1024**3), "provisioned_bytes": str(1024**3), "filesystem": metric, "inodes": {"status": "missing"}}],
            "consumers": [{"id": name, "node": "node-a", "reference_state": "active"} for name in sorted(IDENTITY["consumers"])],
            "summary": {"provisioned": {"known_sum_bytes": str(1024**3)}}}


def controller():
    return {"metadata": {"generation": 1}, "spec": {"replicas": 1, "template": {"spec": {"containers": [{"name": "hostpath", "image": DRIVER_IMAGE}]}}},
            "status": {"observedGeneration": 1, "readyReplicas": 1, "updatedReplicas": 1}}


def kubectl_stub(calls, *, cleanup_fails=False):
    def call(config, *args):
        assert config == "owned-config"
        calls.append(args)
        if "statefulset" in args or "deployment" in args:
            return json.dumps(controller())
        if "wc" in args:
            return f"{csi.WRITE_BYTES} {csi.PROBE}"
        if cleanup_fails and "rm" in args and sum("rm" in operation for operation in calls) == 2:
            raise RuntimeError("controlled cleanup error")
        return ""
    return call


def make_lock(directory):
    vendor = Path(directory) / "vendor"
    vendor.mkdir()
    def file(name, data):
        path = vendor / name
        path.write_text(data)
        return {"file": name, "sha256": hashlib.sha256(path.read_bytes()).hexdigest()}
    crds = {"kind": "List", "items": [{"kind": "CustomResourceDefinition", "metadata": {"name": name}} for name in csi.CRDS]}
    driver = {"kind": "StatefulSet", "metadata": {"name": "csi-hostpathplugin", "namespace": "default"}, "spec": controller()["spec"]}
    snapshot = copy.deepcopy(driver)
    snapshot.update(kind="Deployment", metadata={"name": "snapshot-controller", "namespace": "kube-system"})
    resources = {"kind": "List", "items": [{"kind": "CSIDriver", "metadata": {"name": csi.DRIVER}}, driver, snapshot]}
    images = [{"image": image, "platform_digests": {"amd64": "sha256:" + "c" * 64, "arm64": "sha256:" + "d" * 64}} for image in (DRIVER_IMAGE, WORKLOAD_IMAGE)]
    lock = {"status": "reviewed", "driver_release": "v1.19.0", "source_revision": "a" * 40,
            "snapshot_dependency_revisions": {"external-snapshotter": "b" * 40}, "snapshot_controller": LOCK["snapshot_controller"],
            "images": images, "workload_image": WORKLOAD_IMAGE, "driver_image": DRIVER_IMAGE,
            "manifests": [file("crds.json", json.dumps(crds)), file("driver.json", json.dumps(resources))],
            "licenses": [file("LICENSE.txt", "Synthetic offline test fixture; no upstream review is claimed.")]}
    versions = {"csi_hostpath_release": "v1.19.0", "csi_hostpath_image": DRIVER_IMAGE.split("@")[0], "csi_workload_image": WORKLOAD_IMAGE.split("@")[0]}
    save_lock(vendor, lock, versions)
    return vendor, lock, versions


def save_lock(vendor, lock, versions):
    path = vendor / "lock.json"
    path.write_text(json.dumps(lock))
    versions["csi_lock_sha256"] = hashlib.sha256(path.read_bytes()).hexdigest()


class CsiChecks(unittest.TestCase):
    def test_fixture_is_bounded_unprivileged_and_uses_only_locked_workload_image(self):
        fixture = csi.render_fixture(WORKLOAD_IMAGE, RUN)
        pods = [item for item in fixture["items"] if item["kind"] == "Pod"]
        self.assertEqual({pod["metadata"]["name"] for pod in pods}, set(csi.PODS))
        for pod in pods:
            self.assertFalse(pod["spec"]["automountServiceAccountToken"])
            self.assertTrue(pod["spec"]["securityContext"]["runAsNonRoot"])
            container = pod["spec"]["containers"][0]
            self.assertEqual(container["image"], WORKLOAD_IMAGE)
            self.assertEqual(container["command"], ["sleep", "3600"])
            self.assertTrue(container["securityContext"]["readOnlyRootFilesystem"])
        self.assertLessEqual(csi.WRITE_BYTES, 32 * 1024**2)
        with self.assertRaisesRegex(RuntimeError, "digest"):
            csi.render_fixture("example.invalid/busybox:latest", RUN)

    def test_lock_requires_reviewed_pins_sources_platforms_and_all_dependencies(self):
        with tempfile.TemporaryDirectory() as directory:
            vendor, original, versions = make_lock(directory)
            self.assertEqual(csi.validate_lock(vendor, versions), original)
            for mutate, message in [
                (lambda lock: lock.update(status="selected"), "reviewed"),
                (lambda lock: lock.update(source_revision="master"), "commit revisions"),
                (lambda lock: lock["images"][0]["platform_digests"].pop("arm64"), "arm64"),
                (lambda lock: lock["images"][0].update(image="example.invalid/image:latest"), "immutable digests"),
                (lambda lock: lock["manifests"].pop(0), "snapshot CRDs"),
                (lambda lock: lock["manifests"].reverse(), "CRDs must precede"),
            ]:
                lock = copy.deepcopy(original)
                mutate(lock)
                save_lock(vendor, lock, versions)
                with self.assertRaisesRegex(RuntimeError, message):
                    csi.validate_lock(vendor, versions)
            save_lock(vendor, original, versions)
            versions["csi_lock_sha256"] = "0" * 64
            with self.assertRaisesRegex(RuntimeError, "checksum"):
                csi.validate_lock(vendor, versions)

    def test_lock_rejects_changed_manifest_escaped_license_and_old_driver_default(self):
        with tempfile.TemporaryDirectory() as directory:
            vendor, lock, versions = make_lock(directory)
            path = vendor / "driver.json"
            original = path.read_text()
            path.write_text(original + "\n")
            with self.assertRaisesRegex(RuntimeError, "checksum mismatch"):
                csi.validate_lock(vendor, versions)
            path.write_text(original)
            escaped = copy.deepcopy(lock)
            escaped["licenses"][0]["file"] = "../outside.txt"
            save_lock(vendor, escaped, versions)
            with self.assertRaisesRegex(RuntimeError, "escapes"):
                csi.validate_lock(vendor, versions)
            old = DRIVER_IMAGE.replace("v1.19.0", "v1.18.0")
            path.write_text(original.replace(DRIVER_IMAGE, old))
            lock["manifests"][1]["sha256"] = hashlib.sha256(path.read_bytes()).hexdigest()
            lock["images"][0]["image"] = old
            save_lock(vendor, lock, versions)
            with self.assertRaisesRegex(RuntimeError, "wrong driver release"):
                csi.validate_lock(vendor, versions)

    def test_timestamps_and_counters_keep_exactness_and_reject_bad_wire_values(self):
        self.assertEqual(csi.timestamp_ns("2026-11-07T12:00:00.000000001Z") - BASE, 1)
        self.assertEqual(csi.counter(str(2**64 - 1)), 2**64 - 1)
        for value in (None, True, 0, "-1", "01", "1.0", str(2**64)):
            with self.subTest(value=value), self.assertRaises(RuntimeError):
                csi.counter(value)
        for value in (None, 7, "2026-11-07T12:00:00+01:00", "2026-02-30T00:00:00Z"):
            with self.subTest(value=value), self.assertRaises(RuntimeError):
                csi.timestamp_ns(value)

    def test_byte_gate_requires_fresh_complete_attributable_report_and_honest_exit(self):
        original = report()
        value = csi.observation(original, 0, IDENTITY, BASE + 2 * 10**9, after_ns=BASE)
        self.assertEqual(value["inode_status"], "missing")
        for now, after in [(BASE + 92 * 10**9, BASE), (BASE - 31 * 10**9, BASE - 100 * 10**9), (BASE + 2 * 10**9, BASE + 10**9)]:
            self.assertIsNone(csi.observation(original, 0, IDENTITY, now, after_ns=after))
        for mutate in (lambda r: r["records"][0]["filesystem"]["source"].update(pod_id="pod:unrelated"),
                       lambda r: r["records"][0].update(uid="recreated"),
                       lambda r: r["summary"]["provisioned"].update(known_sum_bytes=str(2 * 1024**3)),
                       lambda r: r["collection"]["cost"].update(node_targets=True),
                       lambda r: r["records"][0]["filesystem"].update(used_bytes="-1")):
            changed = copy.deepcopy(original)
            mutate(changed)
            with self.assertRaises(RuntimeError):
                csi.observation(changed, 0, IDENTITY, BASE + 2 * 10**9, after_ns=BASE)
        missing = copy.deepcopy(original)
        missing["records"][0]["filesystem"]["status"] = "missing"
        self.assertIsNone(csi.observation(missing, 4, IDENTITY, BASE + 2 * 10**9, after_ns=BASE))
        with self.assertRaisesRegex(RuntimeError, "passed without"):
            csi.observation(missing, 0, IDENTITY, BASE + 2 * 10**9, after_ns=BASE)
        with self.assertRaisesRegex(RuntimeError, "exit 0"):
            csi.observation(original, 4, IDENTITY, BASE + 2 * 10**9, after_ns=BASE)
        with self.assertRaisesRegex(RuntimeError, "structurally invalid"):
            csi.observation({"schema_version":"view-storage/v1alpha1", "query":{"require_metrics":True}}, 0, IDENTITY, BASE, after_ns=BASE)

    def test_poll_uses_monotonic_deadline_and_rejects_late_success(self):
        clock = Clock()
        self.assertEqual(csi.poll(lambda: "ready" if clock.time >= 10 else None, 11, "missing", monotonic=clock.monotonic, sleep=clock.sleep), "ready")
        clock.time = 9
        def late():
            clock.time += 2
            return "late"
        with self.assertRaisesRegex(RuntimeError, "missing"):
            csi.poll(late, 10, "missing", monotonic=clock.monotonic, sleep=clock.sleep)

    def test_fixture_requires_bound_verified_volume_and_ready_distinct_same_node_pods(self):
        claim = {"metadata": {"namespace": csi.NAMESPACE, "uid": "claim"}, "spec": {"storageClassName": csi.NAMESPACE, "volumeMode": "Filesystem", "volumeName": "pv-data"}, "status": {"phase": "Bound"}}
        pods = {"items": [{"metadata": {"name": name, "namespace": csi.NAMESPACE, "uid": name}, "spec": {"nodeName": "node-a"}, "status": {"phase": "Running", "conditions": [{"type": "Ready", "status": "True"}]}} for name in csi.PODS]}
        volume = {"metadata": {"name": "pv-data"}, "spec": {"csi": {"driver": csi.DRIVER}, "claimRef": {"uid": "claim"}}}
        def read(config, *args):
            self.assertEqual(config, "owned-config")
            return json.dumps(claim if "pvc" in args else pods if "pods" in args else volume)
        self.assertEqual(csi.fixture_identity(read, "owned-config"), IDENTITY)
        claim["status"]["phase"] = "Pending"
        self.assertIsNone(csi.fixture_identity(read, "owned-config"))
        claim["status"]["phase"] = "Bound"
        pods["items"][0]["status"]["conditions"] = []
        self.assertIsNone(csi.fixture_identity(read, "owned-config"))
        pods["items"][0]["status"]["conditions"] = [{"type": "Ready", "status": "True"}]
        pods["items"][1]["spec"]["nodeName"] = "node-b"
        with self.assertRaisesRegex(RuntimeError, "share one"):
            csi.fixture_identity(read, "owned-config")
        pods["items"][1]["spec"]["nodeName"] = "node-a"
        volume["spec"]["claimRef"]["uid"] = "recreated"
        with self.assertRaisesRegex(RuntimeError, "binding"):
            csi.fixture_identity(read, "owned-config")

    def test_controller_readiness_requires_current_generation_and_updated_replicas(self):
        value = controller()
        with patch.object(csi.json, "loads", return_value=value):
            self.assertTrue(csi.controllers_ready(lambda *args: "{}", "owned-config", LOCK))
            for key in ("observedGeneration", "readyReplicas", "updatedReplicas"):
                value["status"][key] = 0
                self.assertIsNone(csi.controllers_ready(lambda *args: "{}", "owned-config", LOCK))
                value["status"][key] = 1

    def test_real_plugin_query_uses_owned_config_and_never_exposes_subprocess_payloads(self):
        response = SimpleNamespace(returncode=0, stdout=json.dumps(report()), stderr="PRIVATE_TEST_DATA")
        with patch.object(csi.subprocess, "run", return_value=response) as run:
            csi.query("real-plugin", "owned-config")
            command = run.call_args.args[0]
            self.assertEqual(command[1:3], ["--kubeconfig", "owned-config"])
            self.assertIn("--require-metrics", command)
            self.assertEqual(run.call_args.kwargs["timeout"], 25)
        for code, stdout in [(1, "PRIVATE_TEST_DATA"), (0, "PRIVATE_TEST_DATA")]:
            with patch.object(csi.subprocess, "run", return_value=SimpleNamespace(returncode=code, stdout=stdout)):
                with self.assertRaises(RuntimeError) as error:
                    csi.query("plugin", "owned-config")
                self.assertNotIn("PRIVATE_TEST_DATA", str(error.exception))

    def run_gate(self, directory, query, *, cleanup_fails=False, utc=None, calculation_error=None):
        root = Path(directory)
        binary = root / "test-plugin"
        binary.write_text("Synthetic executable; subprocess query is mocked.")
        binary.chmod(0o700)
        calls, clock = [], Clock()
        if utc is None:
            times = iter([BASE, BASE + 2 * 10**9, BASE + 3 * 10**9, BASE + 5 * 10**9])
            utc = lambda: next(times)
        with patch.object(csi, "fixture_identity", return_value=IDENTITY), patch.object(csi, "query", side_effect=query), patch.object(csi.dev_calculations, "verify_csi", return_value={"status":"passed"}, side_effect=calculation_error):
            result = csi.verify(kubectl_stub(calls, cleanup_fails=cleanup_fails), "owned-config", binary, root, RUN, LOCK,
                                monotonic=clock.monotonic, sleep=clock.sleep, utc_ns=utc)
        return result, calls

    def test_success_requires_advanced_observation_after_one_bounded_shared_write_and_cleanup(self):
        with tempfile.TemporaryDirectory() as directory:
            result, calls = self.run_gate(directory, [(report(), 0), (report(4, used=40 * 1024**3 + csi.WRITE_BYTES, available=50 * 1024**3 - csi.WRITE_BYTES), 0)])
            self.assertEqual(result["status"], "passed")
            self.assertGreater(result["after"]["observed_ns"], result["before"]["observed_ns"])
            self.assertEqual(result["write_bytes"], csi.WRITE_BYTES)
            writes = [call for call in calls if "dd" in call]
            self.assertEqual(len(writes), 1)
            self.assertIn("count=16", writes[0])
            self.assertIn("bs=1048576", writes[0])
            self.assertEqual(sum("wc" in call for call in calls), 2)
            self.assertEqual(sum("rm" in call for call in calls), 2)
            self.assertTrue((Path(directory) / "csi-after-report.json").exists())

    def test_missing_or_nonadvancing_metrics_fail_instead_of_skipping(self):
        missing = report()
        missing["records"][0]["filesystem"]["status"] = "missing"
        with tempfile.TemporaryDirectory() as directory:
            with self.assertRaisesRegex(RuntimeError, "no fresh attributable baseline"):
                self.run_gate(directory, lambda *args: (missing, 4), utc=lambda: BASE + 2 * 10**9)
        with tempfile.TemporaryDirectory() as directory:
            count = 0
            def unchanged(*args):
                nonlocal count
                count += 1
                return report(1 if count == 1 else 4), 0
            with self.assertRaisesRegex(RuntimeError, "no advancing compatible"):
                self.run_gate(directory, unchanged, utc=lambda: BASE)

    def test_cleanup_failure_cannot_leave_a_passing_result(self):
        with tempfile.TemporaryDirectory() as directory:
            with self.assertRaisesRegex(RuntimeError, "cleanup failed"):
                self.run_gate(directory, [(report(), 0), (report(4, used=40 * 1024**3 + csi.WRITE_BYTES, available=50 * 1024**3 - csi.WRITE_BYTES), 0)], cleanup_fails=True)
            self.assertEqual(json.loads((Path(directory) / "csi-probe-cleanup.json").read_text())["status"], "failed")

    def test_invalid_plugin_payload_is_not_persisted_as_a_support_report(self):
        with tempfile.TemporaryDirectory() as directory:
            with self.assertRaisesRegex(RuntimeError, "required-metrics contract"):
                self.run_gate(directory, [({"raw_node_payload": "PRIVATE_TEST_DATA"}, 0)])
            self.assertFalse((Path(directory) / "csi-before-report.json").exists())
            self.assertNotIn("PRIVATE_TEST_DATA", "".join(path.read_text() for path in Path(directory).glob("*.json")))

    def test_missing_binary_and_low_space_cannot_start_probe_writes(self):
        with tempfile.TemporaryDirectory() as directory:
            with self.assertRaisesRegex(RuntimeError, "real plugin"):
                csi.verify(lambda *args: self.fail("missing binary contacted a cluster"), "owned", Path(directory) / "missing", directory, RUN, LOCK)
            low = report(available=csi.WRITE_BYTES)
            calls = []
            binary = Path(directory) / "plugin"
            binary.write_text("offline fixture")
            binary.chmod(0o700)
            with patch.object(csi, "fixture_identity", return_value=IDENTITY), patch.object(csi, "query", return_value=(low, 0)):
                with self.assertRaisesRegex(RuntimeError, "lacks free space"):
                    csi.verify(kubectl_stub(calls), "owned-config", binary, directory, RUN, LOCK, utc_ns=lambda: BASE)
            self.assertFalse(any("dd" in call for call in calls))
            self.assertEqual(sum("rm" in call for call in calls), 2)

    def test_creation_applies_established_crds_before_workload_and_verifies_owned_profile(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            vendor, lock, _ = make_lock(root)
            (root / "dev").mkdir()
            shutil.copytree(vendor, root / "dev/csi")
            local = root / ".local"
            with patch.object(cluster, "ROOT", root), patch.object(cluster, "LOCAL", local), patch.object(cluster, "preflight"), patch.object(cluster, "run", return_value=""), patch.object(cluster.subprocess, "run") as create, patch.object(cluster, "container_id", return_value="owned-container"), patch.object(cluster, "read_owned"), patch.object(cluster, "verify_csi_lock", return_value=lock), patch.object(cluster, "kubectl") as kubectl, patch.object(cluster, "verify") as verify:
                cluster.create("csi", binary="real-plugin")
                verify.assert_called_once_with("csi", binary="real-plugin")
                command = create.call_args.args[0]
                self.assertIn("--kubeconfig", command)
                operations = [call.args for call in kubectl.call_args_list]
                self.assertTrue(all(call[0] == local / "kubeconfigs/csi.yaml" for call in operations))
                self.assertEqual(sum("--for=condition=Established" in call for call in operations), 3)
                self.assertEqual(operations[0][1:3], ("apply", "-f"))
                self.assertEqual(operations[-1][1:3], ("apply", "-f"))
            state = json.loads((local / "clusters/csi.json").read_text())
            fixture = Path(state["csi_fixture"])
            self.assertEqual(fixture.name, f"csi-{state['run_id']}-fixture.json")
            self.assertTrue(fixture.is_file())
            self.assertTrue(all(item["metadata"]["labels"]["view-storage/run-id"] == state["run_id"] for item in json.loads(fixture.read_text())["items"]))

    def test_cleanup_deletes_only_recorded_owned_csi_fixture(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            local = root / ".local"
            (local / "clusters").mkdir(parents=True)
            fixture = local / "clusters" / f"csi-{RUN}-fixture.json"
            config, state_file = root / "config.yaml", root / "state.json"
            for path in (fixture, config, state_file):
                path.write_text("owned offline fixture")
            state = {"run_id": RUN, "csi_fixture": str(fixture)}
            owned = ("view-storage-csi", config, state_file, state)
            with patch.object(cluster, "LOCAL", local), patch.object(cluster, "read_state", return_value=owned), patch.object(cluster, "run", side_effect=["", ""]), patch("builtins.print"):
                cluster.delete("csi")
            self.assertFalse(fixture.exists())
            state["csi_fixture"] = str(root / "unowned.json")
            with patch.object(cluster, "LOCAL", local), patch.object(cluster, "read_state", return_value=owned), patch.object(cluster, "run") as run:
                with self.assertRaisesRegex(RuntimeError, "outside the owned run"):
                    cluster.delete("csi")
                run.assert_not_called()

    def test_csi_failure_replaces_stale_success_and_interrupt_is_not_success(self):
        for error in (RuntimeError("no fresh CSI metrics"), KeyboardInterrupt()):
            with tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                evidence = root / "artifacts" / RUN
                evidence.mkdir(parents=True)
                validation = evidence / "csi-validation.json"
                validation.write_text('{"status":"passed"}')
                owned = ("view-storage-csi", root / "config.yaml", root / "state.json", {"run_id": RUN})
                with patch.object(cluster, "ROOT", root), patch.object(cluster, "LOCAL", root / ".local"), patch.object(cluster, "read_owned", return_value=owned), patch.object(cluster, "verify_csi_lock", return_value=LOCK), patch.object(csi, "verify", side_effect=error):
                    with self.assertRaises(type(error)):
                        cluster.verify("csi")
                self.assertEqual(json.loads(validation.read_text())["status"], "failed")


if __name__ == "__main__":
    unittest.main()
