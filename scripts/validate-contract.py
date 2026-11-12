#!/usr/bin/env python3
# /// script
# requires-python = ">=3.12"
# dependencies = ["jsonschema==4.26.0"]
# ///
"""Validate design fixtures; this is not the plugin implementation."""

from collections import Counter
from copy import deepcopy
from decimal import Decimal
from datetime import datetime
from fractions import Fraction
import json
from pathlib import Path
import re

from jsonschema import Draft202012Validator, FormatChecker

ROOT = Path(__file__).resolve().parents[1]
BASE = ROOT / "examples/contract"
BYTE_FIELDS = ("capacity_bytes", "used_bytes", "available_bytes")
INODE_FIELDS = ("inodes_total", "inodes_used", "inodes_free")
PRESSURE_CODES = {"filesystem_usage_high", "inode_usage_high", "filesystem_space_exhausted", "filesystem_inodes_exhausted"}


def check(condition, message):
    if not condition:
        raise AssertionError(message)


def load(path):
    return json.loads(path.read_text(), parse_float=Decimal)


def time(value):
    return datetime.fromisoformat(value.replace("Z", "+00:00"))


def fixture_bytes(value):
    """Only the simple integral quantities used by these API fixtures."""
    match = re.fullmatch(r"([0-9]+)(Gi|Mi|Ki)?", value)
    check(match is not None, f"unsupported fixture quantity {value}")
    return int(match[1]) * {None: 1, "Ki": 1024, "Mi": 1024**2, "Gi": 1024**3}[match[2]]


def percentage(values):
    if not values[0] or values[1] is None:
        return None
    units, remainder = divmod(100 * values[1] * 10**6, values[0])
    units += 2 * remainder >= values[0]
    return Decimal(units) / 10**6


def check_totals(t, members, findings, complete, utilization=True):
    check(t["selection_complete"] == complete and t["record_count"] == len(members), "incorrect totals completeness or count")
    check(t["phase_counts"] == dict(Counter(r["phase"] or "Unknown" for r in members)), "incorrect phase counts")
    for label, field in (("requested", "requested_bytes"), ("bound", "bound_bytes"), ("provisioned", "provisioned_bytes")):
        vals = [int(r[field]) for r in members if r[field] is not None]
        expected = {"known_sum_bytes": str(sum(vals)) if vals or not members else None, "known_count": len(vals),
                    "total_count": len(members), "complete": complete and len(vals) == len(members)}
        check(t[label] == expected, f"{label}: incorrect sum or completeness")
    for family, fields in (("filesystem", BYTE_FIELDS), ("inodes", INODE_FIELDS)):
        a = t[family]
        eligible = [r for r in members if r["volume_mode"] != "Block"]
        usable = [r[family] for r in eligible if r[family]["status"] == "ok" and not r.get("overlapping_filesystem", False)]
        expected_coverage = {"eligible": len(eligible), "fresh_complete": len(usable), "not_applicable": len(members)-len(eligible),
            "by_reason": dict(Counter("overlapping_filesystem" if r.get("overlapping_filesystem", False) else r[family]["reason"] or r[family]["status"]
                                      for r in eligible if r.get("overlapping_filesystem", False) or r[family]["status"] != "ok")) if utilization else {}}
        check(a["coverage"] == expected_coverage, "incorrect metric coverage")
        times = sorted(m["observed_at"] for m in usable)
        check(a["oldest_observed_at"] == (times[0] if times else None), "incorrect oldest sample")
        check(a["newest_observed_at"] == (times[-1] if times else None), "incorrect newest sample")
        skew = bool(times) and (time(times[-1])-time(times[0])).total_seconds() > 60
        vals = [sum(int(m[k]) for m in usable) if usable and not skew else None for k in fields]
        check(all(a[k] == (str(v) if v is not None else None) for k,v in zip(fields,vals)), "incorrect common-subset sum")
        check(a["used_percent"] == percentage(vals) and not a["physical_total"], "incorrect weighted or physical total")
        if skew:
            check(a["status"] == "unavailable" and a["reason"] == "time_skew", "skew not surfaced")
        elif usable:
            check(a["status"] == ("ok" if len(usable) == len(eligible) else "partial"), "incorrect aggregate status")
        else:
            check(a["status"] in ("unavailable","not_requested","not_applicable"), "missing aggregate reported as usable")
    ids = {r["id"] for r in members}
    check(t["finding_counts"] == {s: sum(f["severity"] == s and f["subject_id"] in ids for f in findings)
                                for s in ("critical","warning","info")}, "incorrect finding counts")


def candidate_risk(metric, fields, warning, critical):
    if metric["status"] not in ("ok","partial"):
        return None
    severity = None
    if all(metric["field_status"][k] == "ok" for k in fields[:2]) and int(metric[fields[0]]) > 0:
        ratio = Fraction(100 * int(metric[fields[1]]), int(metric[fields[0]]))
        if ratio >= Fraction(warning):
            severity = "critical" if ratio >= Fraction(critical) else "warning"
    if metric["field_status"][fields[2]] == "ok" and int(metric[fields[2]]) == 0:
        severity = "critical"
    return severity


def pressure_findings(report):
    expected = {}
    q = report["query"]
    for r in report["records"]:
        for family, fields, usage, exhausted in (
            ("filesystem",BYTE_FIELDS,"filesystem_usage_high","filesystem_space_exhausted"),
            ("inodes",INODE_FIELDS,"inode_usage_high","filesystem_inodes_exhausted")):
            m = r[family]
            if m["status"] not in ("ok","partial"):
                continue
            if all(m["field_status"][k] == "ok" for k in fields[:2]) and int(m[fields[0]]) > 0:
                ratio = Fraction(100 * int(m[fields[1]]), int(m[fields[0]]))
                if ratio >= Fraction(q["warn_used_percent"]):
                    expected[(r["id"],usage)] = "critical" if ratio >= Fraction(q["critical_used_percent"]) else "warning"
            if m["field_status"][fields[2]] == "ok" and int(m[fields[2]]) == 0:
                expected[(r["id"],exhausted)] = "critical"
    return expected


def expected_exit(report):
    q, s = report["query"], report["selection"]
    if not s["complete"] and (q["require_metrics"] or q["fail_on"]):
        return 4
    strict = q["require_metrics"] or q["fail_on"]
    if strict and q["utilization"] and any(r["volume_mode"] not in ("Filesystem", "Block") for r in report["records"]):
        return 4
    if q["fail_on"]:
        phases = {"pvc": {"Pending", "Bound", "Lost"}, "pv": {"Available", "Bound", "Released", "Failed"}}
        if any(r["phase"] not in phases[r["kind"]] for r in report["records"]) or any(p["reference_state"] == "unknown" for p in report["consumers"]):
            return 4
    if q["require_metrics"] and any(r["volume_mode"] != "Block" and r["filesystem"]["status"] != "ok" for r in report["records"]):
        return 4
    if q["utilization"] and q["fail_on"]:
        for record in report["records"]:
            for family, fields in (("filesystem",BYTE_FIELDS),("inodes",INODE_FIELDS)):
                metric = record[family]
                if metric["status"] != "uncertain":
                    continue
                for candidate in metric["candidates"]:
                    severity = candidate_risk(candidate,fields,q["warn_used_percent"],q["critical_used_percent"])
                    if severity == "critical" or severity == q["fail_on"]:
                        return 4
    if q["fail_on"] and any(f["severity"] == "critical" or f["severity"] == q["fail_on"] for f in report["findings"]):
        return 3
    return 0


def check_semantics(report, exit_code):
    query = report["query"]
    check(not query["require_metrics"] or query["utilization"], "required metrics did not imply utilization")
    check(0 < Fraction(query["warn_used_percent"]) < Fraction(query["critical_used_percent"]) <= 100, "invalid threshold order")
    for key in ("warn_used_percent","critical_used_percent"):
        canonical = format(Decimal(query[key]),"f")
        if "." in canonical:
            canonical = canonical.rstrip("0").rstrip(".")
        check(query[key] == canonical, "threshold is not a canonical decimal string")
    check(query["utilization"] == (query["metrics_source"] is not None), "metrics source/request mismatch")
    for diagnostic in report["diagnostics"]:
        check(diagnostic["affected_fields"] == sorted(set(diagnostic["affected_fields"])), "diagnostic field names are not canonical")
    def diagnostic_key(d):
        return (d["subject_id"] is not None, d["subject_id"] or "", d["code"], d["source"], d["reason"], d["affected_fields"], d["message"])
    check(report["diagnostics"] == sorted(report["diagnostics"], key=diagnostic_key), "diagnostic order depends on input order")
    records = {r["id"]:r for r in report["records"]}
    pods = {p["id"]:p for p in report["consumers"]}
    check(len(records) == len(report["records"]) and len(pods) == len(report["consumers"]), "duplicate record or consumer")
    s = report["selection"]
    check(s["base_count"] == s["matched_count"]+s["excluded_count"]+s["unknown_count"], "selection partition mismatch")
    check(s["matched_count"] == len(records) and s["unknown_count"] == len(s["unknown_records"]), "selection counts mismatch")
    check(s["complete"] == (s["unknown_count"] == 0), "incorrect selection completeness")
    unknown = {r["id"] for r in s["unknown_records"]}
    check(len(unknown) == s["unknown_count"] and not unknown & records.keys(), "unknown candidates counted as matches")
    edges = set()
    for e in report["relationships"]:
        pair = (e["record_id"],e["consumer_id"])
        check(pair not in edges and pair[0] in records and pair[1] in pods, "duplicate edge or dangling endpoint")
        edges.add(pair)
        for m in e["mounts"]:
            check(m["volume_name"] in e["volume_names"], "mount volume absent from edge")
            check(not (m["sub_path"] is not None and m["sub_path_expr"] is not None), "mutually exclusive subpaths")
            check(m["kind"] != "block" or m["read_only"] is None, "block mapping treated as mount")
    for r in records.values():
        check(r["id"] == r["kind"]+":"+r["uid"], "identity mismatch")
        targets = {p for rec,p in edges if rec == r["id"]}
        check(set(r["consumer_ids"]) == targets, "consumer IDs do not resolve to edges")
        if any(pods[p]["reference_state"] == "unknown" for p in targets):
            check(r["consumer_visibility"] == "unknown", "unknown lifecycle asserted complete consumer visibility")
            check(not any(f["subject_id"] == r["id"] and f["code"] == "no_active_consumer" for f in report["findings"]), "unknown lifecycle asserted no active consumers")
            check(any(d["subject_id"] == r["id"] and d["code"] == "unsupported_api_value" and "consumer_phase" in d["affected_fields"] for d in report["diagnostics"]), "unknown lifecycle lacks diagnostic")
        if r["volume_mode"] not in ("Filesystem", "Block"):
            check(any(d["subject_id"] == r["id"] and "volume_mode" in d["affected_fields"] and d["code"] in ("unsupported_api_value", "invalid_api_field") for d in report["diagnostics"]), "unknown volume mode lacks diagnostic")
        expected_count = sum(pods[p]["reference_state"] == "active" for p in targets) if r["consumer_visibility"] == "complete" else None
        check(r["active_consumer_count"] == expected_count, "incorrect consumer count")
        terminating = sum(pods[p]["reference_state"] == "terminating" for p in targets) if r["consumer_visibility"] == "complete" else None
        check(r["terminating_consumer_count"] == terminating, "incorrect terminating consumer count")
        for family, fields in (("filesystem",BYTE_FIELDS),("inodes",INODE_FIELDS)):
            m = r[family]
            if r["volume_mode"] == "Block":
                check(m["status"] == ("not_applicable" if query["utilization"] else "not_requested"), "raw block filesystem metrics must be inapplicable")
                check(all(m[field] is None for field in fields) and m["source"] is None and m["observed_at"] is None and m["candidate_count"] == 0 and not m["candidates"], "raw block acquired filesystem observations")
            if query["utilization"] and r["volume_mode"] not in ("Filesystem", "Block"):
                check(m["status"] == "invalid" and m["candidate_count"] == 0 and all(m[field] is None for field in fields), "unknown volume mode asserted usable metrics")
            check(m["candidate_count"] == len(m["candidates"]), "candidate count mismatch")
            for candidate in m["candidates"]:
                check(candidate["source"]["pod_id"] in targets, "candidate source is not a consumer")
                check(pods[candidate["source"]["pod_id"]]["reference_state"] in ("active", "terminating"), "candidate source has unknown or terminal lifecycle")
                cv = [int(candidate[k]) if candidate[k] is not None else None for k in fields]
                check(all(v is None or 0 <= v <= 2**64-1 for v in cv), "candidate counter outside u64")
                check(candidate["used_percent"] == percentage(cv), "candidate percentage mismatch")
                if candidate["status"] in ("ok","partial"):
                    age = (time(report["collection"]["finished_at"])-time(candidate["observed_at"])).total_seconds()
                    check(-30 <= age <= query["max_metrics_age_seconds"], "stale candidate marked fresh")
            check(all((m[k] is not None) == (m["field_status"][k] == "ok") for k in fields), "value/field_status mismatch")
            vals = [int(m[k]) if m[k] is not None else None for k in fields]
            check(m["used_percent"] == percentage(vals), "incorrect leaf percentage")
            if m["status"] == "ok":
                check(all(m["field_status"][k] == "ok" for k in fields), "partial tuple marked complete")
            if m["source"] is not None:
                check(m["source"]["pod_id"] in targets, "metric source is not a consumer")
                check(pods[m["source"]["pod_id"]]["node"] == m["source"]["node"], "metric attributed to wrong node")
            if m["status"] in ("ok","partial"):
                age = (time(report["collection"]["finished_at"])-time(m["observed_at"])).total_seconds()
                check(-30 <= age <= report["query"]["max_metrics_age_seconds"], "stale observation marked fresh")
    actual = {}
    for f in report["findings"]:
        check(f["subject_id"] in records, "finding subject absent")
        if f["code"] in PRESSURE_CODES:
            key = (f["subject_id"],f["code"])
            check(key not in actual, "duplicate pressure finding")
            actual[key] = f["severity"]
            m = records[key[0]]["inodes" if "inode" in key[1] else "filesystem"]
            check(all(e["observed_at"] == m["observed_at"] and e["source"] == m["source"] for e in f["evidence"]), "pressure provenance lost")
            for evidence in f["evidence"]:
                value = m[evidence["field"]]
                # JSON 95 and 95.0 are the same exact number. Python booleans
                # compare equal to 0/1, but are never valid pressure evidence.
                check(not isinstance(evidence["value"], bool) and evidence["value"] == value,
                      "pressure evidence differs from observed value")
    check(actual == pressure_findings(report), "field-specific pressure finding missing or incorrect")
    check_totals(report["summary"],list(records.values()),report["findings"],s["complete"],query["utilization"])
    seen_groups = set()

    def groups(items, expected_members):
        union = set()
        for g in items:
            check(g["id"] not in seen_groups, "duplicate group ID")
            seen_groups.add(g["id"])
            ids = set(g["member_ids"])
            check(ids <= records.keys(), "dangling group member")
            union |= ids
            check_totals(g["totals"],[records[i] for i in ids],report["findings"],s["complete"],query["utilization"])
            if g["dimension"] == "storageclass":
                for identity in ids:
                    record = records[identity]
                    expected_state = {"known":"known", "classless":"absent", "unassigned":"unassigned", "unknown":"unknown"}[record["storage_class_state"]]
                    check(g["value_state"] == expected_state and g["value"] == record["storage_class"], "storage class bucket differs from record state")
            if g["dimension"] == "node":
                check(all(any(pods[p]["node"] == g["value"] for rec,p in edges if rec == i) for i in ids), "fabricated node relationship")
            if g["value_state"] == "known" and g["dimension"] in ("accessmode", "volumemode", "phase"):
                for identity in ids:
                    record = records[identity]
                    expected = "+".join(sorted(set(record["access_modes"]))) if g["dimension"] == "accessmode" else record["volume_mode" if g["dimension"] == "volumemode" else "phase"]
                    check(g["value"] == expected, "type/phase bucket differs from record")
            if g["value_state"] == "known" and g["dimension"] == "statefulset":
                for identity in ids:
                    owners = [pod for pod in pods.values() if pod["id"] in records[identity]["consumer_ids"] and pod["reference_state"] in ("active", "terminating") and "statefulset" in pod]
                    check(any(f'{pod["namespace"]}/{pod["statefulset"]["name"]}@{pod["statefulset"]["uid"]}' == g["value"] for pod in owners), "fabricated StatefulSet relationship")
            if g["children"]:
                groups(g["children"],ids)
            children = [set(c["member_ids"]) for c in g["children"]]
            overlap = any(a & b for i,a in enumerate(children) for b in children[i+1:])
            check(g["overlapping_children"] == overlap, "incorrect overlap marker")
        check(union == expected_members, "group union differs from parent members")

    groups(report["groups"],set(records))
    root_children = [set(g["member_ids"]) for g in report["groups"]]
    root_overlap = any(a & b for i,a in enumerate(root_children) for b in root_children[i+1:])
    check(report["summary"]["overlapping_children"] == root_overlap, "root overlap marker missing or incorrect")
    cost = report["collection"]["cost"]
    check(cost["enrichment_attempts"] <= cost["enrichment_budget"] == 256, "enrichment budget exceeded")
    check(cost["node_requests"] <= 3 * cost["node_targets"], "node request budget exceeded")
    check(expected_exit(report) == exit_code, "incorrect strict-policy exit")
    for pod in pods.values():
        expected_state = "terminal" if pod["phase"] in ("Succeeded","Failed") else ("terminating" if pod["deletion_timestamp"] else "active") if pod["phase"] in ("Pending", "Running", "Unknown") else "unknown"
        expected_active = None if expected_state == "unknown" else expected_state == "active"
        check(pod["reference_state"] == expected_state and pod["active"] is expected_active, "incorrect consumer lifecycle state")


def check_inputs(report, bundle):
    check(bundle["clock"] == report["collection"]["finished_at"], "fixture clock mismatch")
    check({k:v for k,v in bundle["request"].items() if k != "namespace"} == report["query"], "fixture query mismatch")
    api = bundle["kubernetes"]
    kind = report["query"]["kind"]
    scope = bundle["request"]["namespace"]
    base = api["pvs"] if kind == "pv" else [claim for claim in api["pvcs"] if scope is None or claim["metadata"]["namespace"] == scope]
    check(len(base) == report["selection"]["base_count"], "fixture inventory mismatch")
    for source in report["collection"]["sources"]:
        key = source["resource"]+"/"+source["scope"]
        if source["verb"] == "list" and source["status"] == "complete":
            check(source["resource_version"] == bundle["resource_versions"][key], "list resource version mismatch")
    identities = {p["metadata"]["uid"]:p for p in base}
    pvcs = {(p["metadata"]["namespace"], p["metadata"]["name"]):p for p in api["pvcs"]}
    pvs = {p["metadata"]["name"]:p for p in api["pvs"]}
    def unavailable(path):
        return any(f["path"] == path and f["status"] >= 400 for f in bundle["failures"])
    def compatible(claim, volume):
        reference = volume["spec"].get("claimRef", {})
        return (claim["metadata"]["namespace"], claim["metadata"]["name"]) == (reference.get("namespace"), reference.get("name")) and reference.get("uid", claim["metadata"]["uid"]) == claim["metadata"]["uid"] and claim["spec"].get("volumeName") == volume["metadata"]["name"]
    for r in report["records"]:
        p = identities[r["uid"]]
        if kind == "pvc":
            claim = p
            volume_name = claim["spec"].get("volumeName")
            volume = pvs.get(volume_name)
            if volume is not None and (unavailable("/api/v1/persistentvolumes/" + volume_name) or not compatible(claim, volume)):
                volume = None
            claim_name, claim_namespace = claim["metadata"]["name"], claim["metadata"]["namespace"]
        else:
            volume = p
            reference = volume["spec"].get("claimRef", {})
            claim_namespace, claim_name = reference.get("namespace"), reference.get("name")
            claim = pvcs.get((claim_namespace, claim_name))
            if claim is not None and (unavailable(f"/api/v1/namespaces/{claim_namespace}/persistentvolumeclaims/{claim_name}") or not compatible(claim, volume)):
                claim = None
        check(r["namespace"] == claim_namespace, "record namespace differs from input")
        raw_mode = p["spec"].get("volumeMode")
        expected_mode = "Filesystem" if raw_mode is None else raw_mode if isinstance(raw_mode, str) else None
        check(r["volume_mode"] == expected_mode, "volume mode differs from input")
        if expected_mode not in ("Filesystem", "Block"):
            code = "invalid_api_field" if expected_mode is None else "unsupported_api_value"
            check(any(d["subject_id"] == r["id"] and d["code"] == code and "volume_mode" in d["affected_fields"] for d in report["diagnostics"]), "volume mode diagnostic differs from input classification")
        check(r["phase"] == (p["status"].get("phase") if p["status"].get("phase") is not None else "Unknown"), "record phase differs from input")
        raw_request = claim["spec"].get("resources", {}).get("requests", {}).get("storage") if claim else None
        check(r["requested_bytes"] == (str(fixture_bytes(raw_request)) if raw_request is not None else None), "API request mismatch")
        raw_bound = claim.get("status", {}).get("capacity", {}).get("storage") if claim else None
        check(r["bound_bytes"] == (str(fixture_bytes(raw_bound)) if raw_bound is not None else None), "API bound capacity mismatch")
        raw_class = p["spec"].get("storageClassName")
        class_state = ("unassigned" if kind == "pvc" else "classless") if raw_class is None else "classless" if raw_class == "" else "known" if isinstance(raw_class, str) else "unknown"
        check(r["storage_class_state"] == class_state and r["storage_class"] == (raw_class if class_state == "known" else None), "storage class state differs from input")
        raw_provisioned = volume["spec"].get("capacity", {}).get("storage") if volume else None
        check(r["provisioned_bytes"] == (str(fixture_bytes(raw_provisioned)) if raw_provisioned is not None else None), "API provisioned capacity mismatch")
        for family, mapping in (("filesystem",dict(zip(BYTE_FIELDS,("capacityBytes","usedBytes","availableBytes")))),
                                ("inodes",dict(zip(INODE_FIELDS,("inodes","inodesUsed","inodesFree"))))):
            for m in [r[family], *r[family]["candidates"]]:
                if m["source"] is None:
                    continue
                source = m["source"]
                node = bundle["summary_responses"][source["node"]]
                pod = next(p for p in node["pods"] if "pod:"+p["podRef"]["uid"] == source["pod_id"])
                sample = next(v for v in pod["volume"] if v["name"] == source["volume_name"] and v["pvcRef"]["name"] == claim_name and v["pvcRef"]["namespace"] == claim_namespace)
                check(sample["time"] == m["observed_at"], "observation timestamp mismatch")
                check(all(m[k] == (str(sample[raw]) if raw in sample else None) for k,raw in mapping.items()), "API observation mismatch")
    raw_pods = {p["metadata"]["uid"]:p for p in api["pods"]}
    for pod in report["consumers"]:
        check(pod["deletion_timestamp"] == raw_pods[pod["uid"]]["metadata"].get("deletionTimestamp"), "deletion timestamp differs from input")
        raw_phase = raw_pods[pod["uid"]].get("status", {}).get("phase")
        check(pod["phase"] == (raw_phase if raw_phase is not None else "Unknown"), "pod phase differs from input")
    for e in report["relationships"]:
        pod = raw_pods[e["consumer_id"].removeprefix("pod:")]
        for m in e["mounts"]:
            containers = pod["spec"][{"app":"containers","init":"initContainers","ephemeral":"ephemeralContainers"}[m["container_kind"]]]
            c = next(c for c in containers if c["name"] == m["container"])
            field = "volumeMounts" if m["kind"] == "filesystem" else "volumeDevices"
            raw = next(v for v in c[field] if v["name"] == m["volume_name"] and v.get("mountPath",v.get("devicePath")) == m["path"])
            check(m["sub_path"] == raw.get("subPath") and m["sub_path_expr"] == raw.get("subPathExpr"), "subpath changed")
            check(m["read_only"] == (raw.get("readOnly",False) if field == "volumeMounts" else None), "read-only declaration changed")


def check_views(scenario, report):
    check(set(scenario["views"]) == {"tree","table","summary","issues"}, "missing text view")
    for view, path in scenario["views"].items():
        text = (BASE/path).read_text()
        check("Context: example" in text and "Selection: " in text and "Metrics: " in text, "scope/completeness absent")
        if not report["selection"]["complete"]:
            check("Selection: incomplete" in text and "lower bounds" in text, "incomplete selection hidden")
            check("No matching resources." not in text, "false empty-success message")
        if view == "issues":
            check(all(f["code"] in text for f in report["findings"]), "issue view lost a finding")
        if "--wide" in scenario["view_options"][view]:
            check("runtime unverified" in text, "wide output asserts runtime mounts")
            check(all(m["path"] in text for e in report["relationships"] for m in e["mounts"]), "wide output lost a path")


def negative_checks(reports):
    def rejects(name, report, code, reason=None):
        try:
            check_semantics(report,code)
        except AssertionError as error:
            if reason is not None:
                check(reason in str(error), "wrong rejection for " + name)
            return
        raise AssertionError("accepted invalid mutation: "+name)
    r = deepcopy(reports["healthy"])
    r["records"][0]["consumer_ids"].append("pod:missing")
    rejects("dangling consumer",r,0)
    r = deepcopy(reports["healthy"])
    r["relationships"].append(deepcopy(r["relationships"][0]))
    rejects("duplicate edge",r,0)
    r = deepcopy(reports["healthy"])
    r["groups"][0]["member_ids"].append("pvc:missing")
    rejects("dangling group member",r,0,"dangling group member")
    r = deepcopy(reports["shared-rwx"])
    r["summary"]["provisioned"]["known_sum_bytes"] = str(300*1024**3)
    rejects("RWX inflation",r,0)
    rejects("unknown membership passed as empty",deepcopy(reports["unknown-selection"]),0)
    r = deepcopy(reports["field-pressure"])
    r["findings"] = [f for f in r["findings"] if f["subject_id"] != "pvc:partial-alert"]
    rejects("partial-data pressure suppressed",r,4)
    rejects("conflicting critical observations passed",deepcopy(reports["ambiguous-critical"]),0)
    return 7


def exact_arithmetic_checks():
    capacity = 2**60
    used = capacity*4//5
    check(Fraction(100*used,capacity) < Fraction("80"), "boundary must be strictly below warning")
    metric = {"status":"partial","capacity_bytes":str(capacity),"used_bytes":str(used),"available_bytes":None,
              "field_status":{"capacity_bytes":"ok","used_bytes":"ok","available_bytes":"missing"}}
    check(candidate_risk(metric,BYTE_FIELDS,"80","90") is None, "float rounding created a false warning")
    check(percentage([capacity,used,None]) == Decimal("80"), "display rounding contract changed")
    metric["used_bytes"] = str((capacity*4+4)//5)
    check(candidate_risk(metric,BYTE_FIELDS,"80","90") == "warning", "just-above threshold not detected")
    check(percentage([200000000,1,None]) == Decimal("0.000001"), "percentage must round half up")


def main():
    schema = load(ROOT/"docs/schema/report.schema.json")
    Draft202012Validator.check_schema(schema)
    validator = Draft202012Validator(schema,format_checker=FormatChecker())
    reports = {}
    scenarios = load(BASE/"manifest.json")["scenarios"]
    for s in scenarios:
        r = load(BASE/s["report"])
        validator.validate(r)
        check_semantics(r,s["expected_exit"])
        check_inputs(r,load(BASE/s["input"]))
        check_views(s,r)
        reports[s["id"]] = r
        print("PASS",s["id"])
    count = negative_checks(reports)
    exact_arithmetic_checks()
    print(f"PASS schema, {len(reports)} API/report scenarios, {len(reports)*4} text views, {count} semantic rejection checks, exact threshold boundaries")


if __name__ == "__main__":
    main()
