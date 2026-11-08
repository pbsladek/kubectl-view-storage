//! One scenario shared by in-memory collection and actual HTTP integration.
#![allow(clippy::unwrap_used, clippy::expect_used)]
use serde_json::{Value, json};

pub const CLAIMS: usize = 300;
pub const TARGETED_GETS: usize = 254; // 256 attempts minus pods and denied bulk PV list.

pub fn claims() -> Value {
    json!({"apiVersion":"v1","kind":"PersistentVolumeClaimList","metadata":{"resourceVersion":"100"},
        "items":(0..CLAIMS).map(|index|json!({"apiVersion":"v1","kind":"PersistentVolumeClaim",
            "metadata":{"name":format!("claim-{index:03}"),"namespace":"demo","uid":format!("claim-{index:03}")},
            "spec":{"storageClassName":"","volumeName":format!("pv-{index:03}"),"resources":{"requests":{"storage":"1Gi"}}},
            "status":{"phase":"Bound","capacity":{"storage":"1Gi"}}})).collect::<Vec<_>>()})
}
pub fn pods() -> Value {
    json!({"apiVersion":"v1","kind":"PodList","metadata":{"resourceVersion":"101"},"items":[]})
}
pub fn denied() -> Value {
    json!({"apiVersion":"v1","kind":"Status","status":"Failure","reason":"Forbidden","code":403,"message":"bulk PV listing is forbidden"})
}
pub fn volume(index: usize) -> Value {
    json!({"apiVersion":"v1","kind":"PersistentVolume",
        "metadata":{"name":format!("pv-{index:03}"),"uid":format!("pv-{index:03}")},
        "spec":{"storageClassName":"","capacity":{"storage":"2Gi"},"claimRef":{"namespace":"demo","name":format!("claim-{index:03}"),"uid":format!("claim-{index:03}")}},"status":{"phase":"Bound"}})
}
pub fn expected_paths() -> Vec<String> {
    [
        "/api/v1/persistentvolumeclaims?limit=500",
        "/api/v1/pods?limit=500",
        "/api/v1/persistentvolumes?limit=500",
    ]
    .into_iter()
    .map(str::to_owned)
    .chain((0..TARGETED_GETS).map(|index| format!("/api/v1/persistentvolumes/pv-{index:03}")))
    .collect()
}
pub fn expected_response_bytes() -> usize {
    [claims(), pods(), denied()]
        .into_iter()
        .chain((0..TARGETED_GETS).map(volume))
        .map(|value| serde_json::to_vec(&value).unwrap().len())
        .sum()
}
pub fn assert_report(report: &Value) {
    let records = report["records"].as_array().unwrap();
    assert_eq!(records.len(), CLAIMS);
    assert_eq!(report["selection"]["complete"], true);
    assert_eq!(report["selection"]["matched_count"], CLAIMS);
    for (index, record) in records.iter().enumerate() {
        assert_eq!(record["id"], format!("pvc:claim-{index:03}"));
        assert_eq!(record["requested_bytes"], "1073741824");
        assert_eq!(record["bound_bytes"], "1073741824");
        assert_eq!(
            record["provisioned_bytes"],
            if index < TARGETED_GETS {
                json!("2147483648")
            } else {
                Value::Null
            }
        );
        assert_eq!(record["filesystem"]["status"], "not_requested");
    }
    let provisioned = &report["summary"]["provisioned"];
    assert_eq!(provisioned["known_count"], TARGETED_GETS);
    assert_eq!(provisioned["total_count"], CLAIMS);
    assert_eq!(
        provisioned["known_sum_bytes"],
        (TARGETED_GETS as u128 * 2 * 1024u128.pow(3)).to_string()
    );
    assert_eq!(provisioned["complete"], false);
    let cost = &report["collection"]["cost"];
    assert_eq!(cost["enrichment_attempts"], 256);
    assert_eq!(cost["base_attempts"], 1);
    assert_eq!(cost["node_requests"], 0);
    assert_eq!(cost["response_bytes"], expected_response_bytes());
    let unresolved = cost["unresolved_prerequisites"].as_array().unwrap();
    for index in 0..CLAIMS {
        assert_eq!(
            unresolved.contains(&json!(format!("/api/v1/persistentvolumes/pv-{index:03}"))),
            index >= TARGETED_GETS
        );
    }
    assert!(unresolved.contains(&json!("/api/v1/persistentvolumes?limit=500")));
    for reason in ["forbidden", "budget_exhausted"] {
        assert!(
            report["diagnostics"]
                .as_array()
                .unwrap()
                .iter()
                .any(|diagnostic| diagnostic["reason"] == reason)
        );
    }
}
