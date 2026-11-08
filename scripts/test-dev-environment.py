#!/usr/bin/env python3
"""Offline checks for fixture permissions, private credentials, and cleanup bounds."""
import importlib.util
import json
from pathlib import Path
import stat
import sys
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "scripts"))
import dev_rbac

spec = importlib.util.spec_from_file_location("dev_cluster", ROOT / "scripts/dev-cluster.py")
cluster = importlib.util.module_from_spec(spec)
spec.loader.exec_module(cluster)


class EnvironmentChecks(unittest.TestCase):
    def test_node_selection_requires_reviewed_digest_and_matching_client(self):
        self.assertEqual(cluster.kubectl_pin(cluster.VERSIONS["baseline_node"]), cluster.VERSIONS["kubectl"])
        for image in cluster.VERSIONS["compatibility_nodes"]:
            expected = image.split(":v", 1)[1].split("@", 1)[0]
            self.assertEqual(cluster.kubectl_pin(image), expected)
            with patch.object(cluster, "run", return_value=json.dumps({"clientVersion": {"gitVersion": "v" + expected}})):
                cluster.require_kubectl(image)
            with patch.object(cluster, "run", return_value=json.dumps({"clientVersion": {"gitVersion": "v0.0.0"}})):
                with self.assertRaisesRegex(RuntimeError, "selected node"):
                    cluster.require_kubectl(image)
        with self.assertRaisesRegex(RuntimeError, "reviewed digest"):
            cluster.kubectl_pin("kindest/node:latest")

    def test_rbac_manifest_grants_only_intended_read_capabilities(self):
        items = json.loads((ROOT / "dev/fixtures/rbac.json").read_text())["items"]
        roles = {(item["kind"], item["metadata"]["name"]): item for item in items if item["kind"] in ("Role", "ClusterRole")}
        expected = {"inventory": {"persistentvolumes", "storageclasses"}, "namespace-only": set(),
                    "denied-pv": {"storageclasses"}, "denied-proxy": {"persistentvolumes", "storageclasses"}}
        for account, extra in expected.items():
            allowed = set()
            for binding in items:
                if binding["kind"] not in ("RoleBinding", "ClusterRoleBinding"):
                    continue
                if not any(subject["name"] == account and subject["namespace"] == dev_rbac.NAMESPACE for subject in binding["subjects"]):
                    continue
                role = roles[(binding["roleRef"]["kind"], binding["roleRef"]["name"])]
                for rule in role["rules"]:
                    self.assertLessEqual(set(rule["verbs"]), {"get", "list"})
                    self.assertNotIn("*", rule["apiGroups"])
                    allowed.update(rule["resources"])
            self.assertEqual(allowed, {"persistentvolumeclaims", "pods"} | extra)
            self.assertNotIn("nodes/proxy", allowed)
            self.assertNotIn("secrets", allowed)

    def test_inventory_cases_have_shared_unscheduled_and_held_terminating_references(self):
        items = json.loads((ROOT / "dev/fixtures/inventory-cases.json").read_text())["items"]
        pods = {item["metadata"]["name"]: item for item in items if item["kind"] == "Pod"}
        claims = [volume["persistentVolumeClaim"]["claimName"] for volume in pods["shared-consumer"]["spec"]["volumes"]]
        self.assertEqual(claims, ["data", "data"])
        self.assertNotIn("nodeName", pods["unscheduled-consumer"]["spec"])
        self.assertTrue(pods["unscheduled-consumer"]["spec"]["nodeSelector"])
        terminating = pods["terminating-consumer"]
        self.assertEqual(terminating["spec"]["containers"][0]["lifecycle"]["preStop"]["sleep"]["seconds"], 3600)
        self.assertEqual(terminating["spec"]["terminationGracePeriodSeconds"], 3600)
        volumes = {item["metadata"]["name"]: item for item in items if item["kind"] == "PersistentVolume"}
        self.assertNotIn("claimRef", volumes["view-storage-available"]["spec"])
        self.assertEqual(volumes["view-storage-released"]["spec"]["claimRef"]["name"], "removed-claim")
        self.assertIn("uid", volumes["view-storage-released"]["spec"]["claimRef"])

    def test_restricted_config_copies_selected_ca_but_no_admin_authentication(self):
        admin = {"current-context": "owned", "contexts": [{"name": "owned", "context": {"cluster": "owned-cluster", "user": "admin"}}],
                 "clusters": [{"name": "decoy", "cluster": {"server": "https://other.invalid"}},
                              {"name": "owned-cluster", "cluster": {"server": "https://owned.invalid", "certificate-authority-data": "PUBLIC_CA"}}],
                 "users": [{"name": "admin", "user": {"client-key-data": "PRIVATE_ADMIN_KEY"}}]}
        config = dev_rbac.token_kubeconfig(admin, "FAKE_TEST_TOKEN", "namespace-only")
        self.assertEqual(config["clusters"][0]["name"], "owned-cluster")
        self.assertEqual(config["contexts"][0]["context"]["namespace"], "view-storage-test")
        self.assertEqual(config["users"], [{"name": "namespace-only", "user": {"token": "FAKE_TEST_TOKEN"}}])
        self.assertNotIn("PRIVATE_ADMIN_KEY", json.dumps(config))
        self.assertEqual(admin["users"][0]["user"]["client-key-data"], "PRIVATE_ADMIN_KEY")

    def test_private_credentials_never_overwrite_files_or_follow_symlinks(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "credential.json"
            dev_rbac.write_private_json(path, {"token": "FAKE"})
            self.assertEqual(stat.S_IMODE(path.stat().st_mode), 0o600)
            with self.assertRaises(FileExistsError):
                dev_rbac.write_private_json(path, {"token": "REPLACEMENT"})
            self.assertEqual(json.loads(path.read_text()), {"token": "FAKE"})
            link = Path(directory) / "link.json"
            link.symlink_to(path)
            with self.assertRaises(FileExistsError):
                dev_rbac.write_private_json(link, {})

    def test_capability_outage_is_not_reported_as_denial(self):
        for status, stderr, expected in [(0, "", True), (1, "", False)]:
            with patch.object(dev_rbac.subprocess, "run", return_value=SimpleNamespace(returncode=status, stdout="", stderr=stderr)) as run:
                self.assertEqual(dev_rbac.can_i("owned-config", "get", "nodes", subresource="proxy"), expected)
                self.assertIn("--kubeconfig", run.call_args.args[0])
                self.assertIn("--all-namespaces", run.call_args.args[0])
                self.assertEqual(run.call_args.kwargs["timeout"], 25)
        with patch.object(dev_rbac.subprocess, "run", return_value=SimpleNamespace(returncode=1, stdout="", stderr="connection refused")):
            with self.assertRaisesRegex(RuntimeError, "error instead of a denial"):
                dev_rbac.can_i("owned-config", "get", "nodes")

    def test_missing_plugin_cannot_pass_fixture_verification(self):
        with tempfile.TemporaryDirectory() as directory, patch.object(dev_rbac, "plugin_json") as query:
            with self.assertRaisesRegex(RuntimeError, "real plugin"):
                dev_rbac.verify_plugin(Path(directory) / "missing-binary", "owned", {}, directory)
            query.assert_not_called()

    def test_cleanup_rejects_credential_path_outside_owned_run_before_cluster_deletion(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            state = {"run_id": "11111111-1111-4111-8111-111111111111", "restricted_kubeconfigs": [str(root / "outside.json")]}
            owned = ("view-storage-inventory", root / "inventory.yaml", root / "state.json", state)
            with patch.object(cluster, "LOCAL", root), patch.object(cluster, "read_state", return_value=owned), patch.object(cluster, "run") as run:
                with self.assertRaisesRegex(RuntimeError, "outside the owned run"):
                    cluster.delete("inventory")
                run.assert_not_called()
            with patch.object(cluster, "LOCAL", root):
                with self.assertRaises(ValueError):
                    cluster.credential_directory("inventory", {"run_id": "../../elsewhere"})

    def test_failed_create_without_a_container_can_clean_up_without_deleting_a_cluster(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            config, state_file = root / "inventory.yaml", root / "state.json"
            config.write_text("owned partial config")
            state_file.write_text("{}")
            state = {"run_id": "11111111-1111-4111-8111-111111111111", "container_id": None}
            owned = ("view-storage-inventory", config, state_file, state)
            with patch.object(cluster, "LOCAL", root), patch.object(cluster, "read_state", return_value=owned), patch.object(cluster, "run", side_effect=["", ""]) as run, patch("builtins.print"):
                cluster.delete("inventory")
                self.assertFalse(any(call.args[:3] == ("kind", "delete", "cluster") for call in run.call_args_list))
            self.assertFalse(config.exists())
            self.assertFalse(state_file.exists())

    def test_changed_container_identity_cannot_be_adopted(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            state = {"container_id": "original-container"}
            owned = ("view-storage-inventory", root / "inventory.yaml", root / "state.json", state)
            with patch.object(cluster, "read_state", return_value=owned), patch.object(cluster, "container_id", return_value="replacement-container"), patch.object(cluster, "kubectl") as kubectl:
                with self.assertRaisesRegex(RuntimeError, "container identity changed"):
                    cluster.read_owned("inventory")
                kubectl.assert_not_called()

    def test_failed_verification_replaces_stale_passing_evidence(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            state = {"run_id": "11111111-1111-4111-8111-111111111111"}
            evidence = root / "artifacts" / state["run_id"]
            evidence.mkdir(parents=True)
            result = evidence / "inventory-validation.json"
            result.write_text('{"status":"passed"}')
            owned = ("view-storage-inventory", root / "inventory.yaml", root / "state.json", state)
            with patch.object(cluster, "ROOT", root), patch.object(cluster, "LOCAL", root / ".local"), patch.object(cluster, "read_owned", return_value=owned), patch.object(cluster, "require_kubectl"), patch.object(cluster, "verify_inventory", side_effect=RuntimeError("fixture failure")):
                with self.assertRaisesRegex(RuntimeError, "fixture failure"):
                    cluster.verify("inventory")
            status = json.loads(result.read_text())
            self.assertEqual(status["status"], "failed")
            self.assertEqual(status["error_kind"], "RuntimeError")


if __name__ == "__main__":
    unittest.main()
