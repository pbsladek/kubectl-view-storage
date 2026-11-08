"""Owned-cluster credential fixtures and independent live report assertions."""
import json
import os
from pathlib import Path
import subprocess

NAMESPACE = "view-storage-test"
PROFILES = {
    "inventory": {"pv": True, "class": True},
    "namespace-only": {"pv": False, "class": False},
    "denied-pv": {"pv": False, "class": True},
    "denied-proxy": {"pv": True, "class": True},
}


def token_kubeconfig(admin, token, account):
    """Copy the selected cluster/CA only, never the administrator's user key."""
    context = next(item["context"] for item in admin["contexts"] if item["name"] == admin["current-context"])
    cluster = next(item for item in admin["clusters"] if item["name"] == context["cluster"])
    name = "view-storage-" + account
    return {"apiVersion": "v1", "kind": "Config", "current-context": name,
            "clusters": [cluster], "users": [{"name": account, "user": {"token": token}}],
            "contexts": [{"name": name, "context": {"cluster": cluster["name"], "user": account, "namespace": NAMESPACE}}]}


def write_private_json(path, value):
    path = Path(path)
    # O_EXCL refuses existing files and symlinks. Tokens never enter logs or
    # argv; generated files live only in the owned ignored kubeconfig directory.
    descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(descriptor, "w") as output:
        json.dump(value, output)
        output.write("\n")


def can_i(config, verb, resource, *, namespace=None, subresource=None):
    args = ["kubectl", "--kubeconfig", str(config), "--request-timeout=15s", "auth", "can-i", verb, resource, "--quiet"]
    if namespace:
        args.extend(["--namespace", namespace])
    else:
        # The generated context has a fixture namespace. Explicit cluster scope
        # avoids kubectl warning on stderr for PV/class/node capability checks;
        # actual transport/authentication errors must still fail verification.
        args.append("--all-namespaces")
    if subresource:
        args.extend(["--subresource", subresource])
    result = subprocess.run(args, text=True, capture_output=True, timeout=25)
    if result.returncode not in (0, 1):
        raise RuntimeError("RBAC capability query failed; inspect the owned test credentials")
    # Quiet mode exits 1 for a denial as well as some connection errors. Require
    # no diagnostic payload for a denial rather than treating an outage as RBAC.
    if result.returncode == 1 and (result.stderr.strip() or result.stdout.strip()):
        raise RuntimeError("RBAC capability query returned an error instead of a denial")
    return result.returncode == 0


def verify_capabilities(configs):
    results = {}
    for account, permissions in PROFILES.items():
        config = configs[account]
        checks = [("claims", "list", "persistentvolumeclaims", True, NAMESPACE, None),
                  ("pods", "list", "pods", True, NAMESPACE, None),
                  ("pv", "get", "persistentvolumes", permissions["pv"], None, None),
                  ("class", "get", "storageclasses.storage.k8s.io", permissions["class"], None, None),
                  ("proxy", "get", "nodes", False, None, "proxy"),
                  ("secrets", "get", "secrets", False, NAMESPACE, None),
                  ("delete_claim", "delete", "persistentvolumeclaims", False, NAMESPACE, None)]
        results[account] = {}
        for label, verb, resource, expected, namespace, subresource in checks:
            allowed = can_i(config, verb, resource, namespace=namespace, subresource=subresource)
            if allowed != expected:
                raise RuntimeError(f"unexpected {label} permission for fixture account {account}")
            results[account][label] = allowed
    return results


def plugin_json(binary, config, *args, expected_exit=0):
    command = [str(binary), "--kubeconfig", str(config), "--timeout", "15s", "--request-timeout", "5s", "-o", "json", *args]
    result = subprocess.run(command, text=True, capture_output=True, timeout=25)
    if result.returncode != expected_exit:
        raise RuntimeError(f"plugin fixture query returned exit {result.returncode}; expected {expected_exit}")
    report = json.loads(result.stdout)
    if report.get("schema_version") != "view-storage/v1alpha1":
        raise RuntimeError("plugin did not return the expected storage report schema")
    if not report["selection"]["complete"]:
        raise RuntimeError("inventory fixture query unexpectedly has unresolved membership")
    return report


def verify_plugin(binary, admin_config, configs, evidence):
    binary = Path(binary).resolve()
    if not binary.is_file() or not os.access(binary, os.X_OK):
        raise RuntimeError("build the real plugin before verification, or supply --binary; fixture presence alone cannot pass")
    reports = {}
    expected_names = {"data", "unreferenced", "pending", "classless"}
    for account, permissions in PROFILES.items():
        report = plugin_json(binary, configs[account])
        records = {record["name"]: record for record in report["records"]}
        if set(records) != expected_names or report["collection"]["scope"]["namespace"] != NAMESPACE:
            raise RuntimeError(f"incorrect default namespace or missing claims for {account}")
        if any(records[name]["phase"] != phase for name, phase in (("data", "Bound"), ("unreferenced", "Bound"), ("pending", "Pending"), ("classless", "Pending"))):
            raise RuntimeError("fixture claim phases are incorrect")
        if records["classless"]["storage_class_state"] != "classless":
            raise RuntimeError("explicitly empty storage class was not preserved")
        if not permissions["pv"] and any(record["provisioned_bytes"] is not None for record in records.values()):
            raise RuntimeError("denied PV access produced an invented provisioned capacity")
        if records["unreferenced"]["active_consumer_count"] != 0 or records["unreferenced"]["terminating_consumer_count"] != 0:
            raise RuntimeError("unreferenced bound claim acquired invented consumers")
        reports[account] = report
    selected = plugin_json(binary, configs["inventory"], "-l", "app=storage-fixture")
    if [record["name"] for record in selected["records"]] != ["data"]:
        raise RuntimeError("claim label selection is incorrect")
    if len(selected["consumers"]) != 4 or not any(pod["node"] is None for pod in selected["consumers"]):
        raise RuntimeError("claim labels filtered consumers or an unscheduled reference disappeared")
    if not any(pod["reference_state"] == "terminating" for pod in selected["consumers"]):
        raise RuntimeError("terminating consumer was not retained")
    if selected["records"][0]["active_consumer_count"] != 3 or selected["records"][0]["terminating_consumer_count"] != 1:
        raise RuntimeError("shared/repeated consumer accounting is incorrect")
    reports["claim-selector"] = selected
    for strict in (False, True):
        args = ["-u", "--pvc", "data"] + (["--require-metrics"] if strict else [])
        report = plugin_json(binary, configs["denied-proxy"], *args, expected_exit=4 if strict else 0)
        if report["records"][0]["filesystem"]["status"] != "forbidden":
            raise RuntimeError("denied node proxy did not remain an explicit metric failure")
        reports["denied-proxy-strict" if strict else "denied-proxy-usage"] = report
    volumes = plugin_json(binary, admin_config, "-A", "--kind", "pv")
    phases = {record["name"]: record["phase"] for record in volumes["records"]}
    if phases.get("view-storage-available") != "Available" or phases.get("view-storage-released") != "Released":
        raise RuntimeError("available/released volume fixture phases are missing")
    reports["cluster-pv"] = volumes
    evidence = Path(evidence)
    for name, report in reports.items():
        (evidence / f"report-{name}.json").write_text(json.dumps(report, indent=2) + "\n")
    return list(reports)
