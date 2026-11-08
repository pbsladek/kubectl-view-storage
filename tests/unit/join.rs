#![allow(clippy::unwrap_used, clippy::expect_used)]
use super::*;
use clap::Parser;
fn fixture(input: &str) -> Inventory {
    serde_json::from_value(
        serde_json::from_str::<serde_json::Value>(input).unwrap()["kubernetes"].clone(),
    )
    .unwrap()
}
fn query() -> Query {
    crate::cli::Cli::try_parse_from(["cmd", "-u"])
        .unwrap()
        .query("demo")
        .unwrap()
}

struct InterruptAfter {
    remaining: usize,
}
impl Checkpoint for InterruptAfter {
    type Error = &'static str;
    fn step(&mut self) -> Result<(), Self::Error> {
        self.remaining = self.remaining.checked_sub(1).ok_or("CPU interrupted")?;
        Ok(())
    }
}

#[test]
fn unfamiliar_lifecycle_cannot_assert_complete_visibility_or_known_counts() {
    let mut input = fixture(include_str!("../../examples/contract/healthy/input.json"));
    input.pods[0].status["phase"] = "FuturePhase".into();
    let mut graph = normalize(&input, &query(), true).unwrap();
    graph.validate().unwrap();
    assert!(graph.records[0].active_count.is_none());
    assert!(graph.records[0].terminating_count.is_none());
    graph.records[0].consumer_visibility = ConsumerVisibility::Complete;
    graph.records[0].active_count = Some(0);
    graph.records[0].terminating_count = Some(0);
    assert!(
        graph
            .validate()
            .unwrap_err()
            .contains("unknown consumer lifecycle")
    );
    graph.records[0].consumer_visibility =
        ConsumerVisibility::Unknown("unsupported_pod_lifecycle".into());
    assert!(graph.validate().unwrap_err().contains("counts disagree"));
    input.pods[0].spec["volumes"][0]["persistentVolumeClaim"]["claimName"] = "unrelated".into();
    let graph = normalize(&input, &query(), true).unwrap();
    graph.validate().unwrap();
    assert_eq!(graph.records[0].active_count, Some(0));
    assert!(graph.consumers.is_empty());
}

#[test]
fn large_nested_normalization_and_filter_loops_have_bounded_checkpoints() {
    let baseline = fixture(include_str!("../../examples/contract/healthy/input.json"));
    for kind in ["owners", "modes", "volumes", "mounts", "containers"] {
        let mut input = baseline.clone();
        let repeated = match kind {
            "owners" => serde_json::json!({"kind":"Other", "name":"owner", "uid":"owner"}),
            "modes" => serde_json::json!("ReadWriteOnce"),
            "volumes" => input.pods[0].spec["volumes"][0].clone(),
            "mounts" => input.pods[0].spec["containers"][0]["volumeMounts"][0].clone(),
            _ => input.pods[0].spec["containers"][0].clone(),
        };
        let array = serde_json::Value::Array(vec![repeated; 1000]);
        match kind {
            "owners" => input.pvcs[0].metadata.owner_references = array.as_array().unwrap().clone(),
            "modes" => input.pvcs[0].spec["accessModes"] = array,
            "volumes" => input.pods[0].spec["volumes"] = array,
            "mounts" => input.pods[0].spec["containers"][0]["volumeMounts"] = array,
            _ => input.pods[0].spec["containers"] = array,
        }
        normalize(&input, &query(), true)
            .unwrap()
            .validate()
            .unwrap();
        let result = normalize_checked(
            &input,
            &query(),
            true,
            &mut InterruptAfter { remaining: 64 },
        );
        assert!(
            matches!(result, Err(CheckedError::Interrupted("CPU interrupted"))),
            "{kind}: {result:?}"
        );
    }
    let mut input = baseline;
    let pod = input.pods[0].clone();
    input.pods = (0..1000)
        .map(|index| {
            let mut pod = pod.clone();
            pod.metadata.uid = Some(format!("pod-{index}"));
            pod.metadata.name = format!("pod-{index}");
            pod
        })
        .collect();
    let graph = normalize(&input, &query(), true).unwrap();
    graph.validate().unwrap();
    let consumers = graph
        .consumers
        .iter()
        .map(|consumer| (consumer.id.clone(), consumer))
        .collect();
    let mut query = query();
    query.filters.node = Some("never-scheduled-here".into());
    assert_eq!(
        matches(&graph.records[0], &query, &consumers),
        Predicate::NoMatch
    );
    assert!(
        matches_checked(
            &graph.records[0],
            &query,
            &consumers,
            &mut InterruptAfter { remaining: 64 }
        )
        .is_err()
    );
    assert!(
        crate::report::select_checked(graph, &query, &mut InterruptAfter { remaining: 64 })
            .is_err()
    );
}
#[test]
fn shared_relationships_and_mounts_do_not_duplicate_capacity() {
    let input = fixture(include_str!(
        "../../examples/contract/shared-rwx/input.json"
    ));
    let graph = normalize(&input, &query(), true).unwrap();
    assert_eq!(graph.records.len(), 1);
    assert_eq!(graph.relationships.len(), 3);
    assert_eq!(graph.records[0].consumers.len(), 3);
    let input = fixture(include_str!("../../examples/contract/healthy/input.json"));
    let graph = normalize(&input, &query(), true).unwrap();
    assert_eq!(graph.relationships[0].mounts.len(), 2);
    assert_eq!(
        graph.relationships[0].mounts[1].sub_path_expr.as_deref(),
        Some("$(POD_NAME)")
    );
    assert_eq!(
        graph.records[0].provisioned,
        Field::Valid(Bytes::new(107374182400))
    );
}

#[test]
fn overlap_requires_exact_visible_csi_identity_and_verified_pvc_binding() {
    let mut input = fixture(include_str!("../../examples/contract/healthy/input.json"));
    let mut claim = input.pvcs[0].clone();
    claim.metadata.name = "second".into();
    claim.metadata.uid = Some("second".into());
    claim.spec["volumeName"] = "pv-second".into();
    let mut volume = input.pvs[0].clone();
    volume.metadata.name = "pv-second".into();
    volume.metadata.uid = Some("pv-second".into());
    volume.spec["claimRef"]["name"] = "second".into();
    volume.spec["claimRef"]["uid"] = "second".into();
    input.pvcs.push(claim);
    input.pvs.push(volume);
    let graph = normalize(&input, &query(), true).unwrap();
    assert!(
        graph
            .records
            .iter()
            .all(|record| record.overlapping_filesystem)
    );
    assert_eq!(
        graph
            .diagnostics
            .iter()
            .filter(|note| note.code == "overlapping_filesystem")
            .count(),
        2
    );
    assert_eq!(
        graph
            .records
            .iter()
            .filter_map(|record| record.provisioned.value())
            .map(|bytes| u128::from(bytes.get()))
            .sum::<u128>(),
        214748364800
    );
    for (path, value) in [
        ("/csi/driver", "another.example"),
        ("/csi/volumeHandle", "handle-other"),
        ("/csi/volumeHandle", ""),
        ("/claimRef/uid", "recreated-claim"),
        ("/volumeMode", "Block"),
        ("/volumeMode", "FutureMode"),
    ] {
        let mut changed = input.clone();
        *changed.pvs[1].spec.pointer_mut(path).unwrap() = value.into();
        assert!(
            normalize(&changed, &query(), true)
                .unwrap()
                .records
                .iter()
                .all(|record| !record.overlapping_filesystem),
            "{path}: {value}"
        );
    }
    let q = crate::cli::Cli::try_parse_from(["cmd", "-A", "--kind", "pv"])
        .unwrap()
        .query("demo")
        .unwrap();
    assert!(
        normalize(&input, &q, true)
            .unwrap()
            .records
            .iter()
            .all(|record| record.overlapping_filesystem)
    );
    input.pvs.clear();
    assert!(
        normalize(&input, &query(), true)
            .unwrap()
            .records
            .iter()
            .all(|record| !record.overlapping_filesystem)
    );
}

#[test]
fn malformed_read_only_is_unknown_and_omission_uses_api_default() {
    let input = fixture(include_str!("../../examples/contract/healthy/input.json"));
    let path = input.pods[0].spec["containers"][0]["volumeMounts"][0]["mountPath"]
        .as_str()
        .unwrap();
    for (value, expected) in [
        (None, Some(false)),
        (Some(serde_json::json!(true)), Some(true)),
        (Some(serde_json::json!("false")), None),
        (Some(serde_json::Value::Null), None),
    ] {
        let mut changed = input.clone();
        let mount = changed.pods[0].spec["containers"][0]["volumeMounts"][0]
            .as_object_mut()
            .unwrap();
        if let Some(value) = value {
            mount.insert("readOnly".into(), value);
        } else {
            mount.remove("readOnly");
        }
        let graph = normalize(&changed, &query(), true).unwrap();
        assert_eq!(
            graph.relationships[0]
                .mounts
                .iter()
                .find(|mount| mount.path == path)
                .unwrap()
                .read_only,
            expected
        );
    }
}
#[test]
fn denied_pods_and_terminal_pods_never_invent_empty_counts() {
    let mut input = fixture(include_str!("../../examples/contract/healthy/input.json"));
    input.pods.clear();
    let q = query();
    let graph = normalize(&input, &q, false).unwrap();
    assert_eq!(graph.records[0].active_count, None);
    let mut q = q;
    q.filters.node = Some("worker-a".into());
    assert!(matches!(
        matches(&graph.records[0], &q, &BTreeMap::new()),
        Predicate::Unknown(_)
    ));
    assert_eq!(
        normalize(&input, &q, true).unwrap().records[0].active_count,
        Some(0)
    );
    input = fixture(include_str!(
        "../../examples/contract/terminating-consumer/input.json"
    ));
    let graph = normalize(&input, &q, true).unwrap();
    assert_eq!(graph.records[0].terminating_count, Some(1));
    let consumers = graph.consumers.iter().map(|c| (c.id.clone(), c)).collect();
    assert_eq!(matches(&graph.records[0], &q, &consumers), Predicate::Match);
}
#[test]
fn binding_mismatch_and_invalid_quantity_keep_record() {
    let mut input = fixture(include_str!("../../examples/contract/healthy/input.json"));
    input.pvs[0].spec["claimRef"]["uid"] = "recreated".into();
    input.pvcs[0].spec["resources"]["requests"]["storage"] = "-1Gi".into();
    let graph = normalize(&input, &query(), true).unwrap();
    let record = &graph.records[0];
    assert!(record.binding_conflict);
    assert_eq!(record.provisioned, Field::Missing);
    assert_eq!(record.requested, Field::Invalid);
    assert_eq!(graph.diagnostics.len(), 1);
    input.pvcs[0].metadata.uid = None;
    assert!(normalize(&input, &query(), true).is_err());
}
#[test]
fn pv_mode_and_open_phase_preserved() {
    let mut input = fixture(include_str!("../../examples/contract/healthy/input.json"));
    input.pvs[0].status["phase"] = "FuturePhase".into();
    let q = crate::cli::Cli::try_parse_from(["cmd", "-A", "--kind", "pv"])
        .unwrap()
        .query("demo")
        .unwrap();
    let graph = normalize(&input, &q, true).unwrap();
    assert_eq!(graph.records[0].id.as_str(), "pv:uid-pv-data");
    assert_eq!(graph.records[0].phase.as_str(), "FuturePhase");
    assert!(
        graph
            .diagnostics
            .iter()
            .any(|d| d.code == "unsupported_api_value")
    );
}

#[test]
fn graph_validation_rejects_dangling_duplicate_and_inconsistent_edges() {
    let input = fixture(include_str!("../../examples/contract/healthy/input.json"));
    let graph = normalize(&input, &query(), true).unwrap();
    assert!(graph.validate().is_ok());
    let mut bad = graph.clone();
    bad.relationships.push(bad.relationships[0].clone());
    assert!(bad.validate().is_err());
    let mut bad = graph.clone();
    bad.consumers.clear();
    assert!(bad.validate().is_err());
    let mut bad = graph.clone();
    bad.records[0].consumers.clear();
    assert!(bad.validate().is_err());
    let mut bad = graph.clone();
    bad.records[0].active_count = Some(0);
    assert!(bad.validate().is_err());
    let mut bad = graph.clone();
    bad.records.push(bad.records[0].clone());
    assert!(bad.validate().is_err());
    let mut bad = graph;
    bad.consumers[0].uid = "different".into();
    assert!(bad.validate().is_err());
}

#[test]
fn pv_capacity_survives_conflicting_claim_without_attributing_new_consumers() {
    let mut input = fixture(include_str!("../../examples/contract/healthy/input.json"));
    input.pvs[0].spec["claimRef"]["uid"] = "old-claim".into();
    let q = crate::cli::Cli::try_parse_from(["cmd", "-A", "--kind", "pv"])
        .unwrap()
        .query("demo")
        .unwrap();
    let graph = normalize(&input, &q, true).unwrap();
    let record = &graph.records[0];
    assert!(record.binding_conflict);
    assert_eq!(record.provisioned, Field::Valid(Bytes::new(107374182400)));
    assert_eq!(record.requested, Field::Missing);
    assert!(graph.relationships.is_empty());
    assert!(graph.validate().is_ok());
}

#[test]
fn malformed_classification_remains_unknown_and_owner_deletion_metadata_is_retained() {
    let mut input = fixture(include_str!("../../examples/contract/healthy/input.json"));
    input.pvcs[0].spec["storageClassName"] = serde_json::json!(7);
    input.pvcs[0].status["phase"] = "Available".into();
    input.pvcs[0].metadata.deletion_timestamp = Some("2026-11-06T19:59:50Z".into());
    input.pvcs[0].metadata.owner_references =
        vec![serde_json::json!({"kind":"Pod","name":"owner","uid":"owner-uid"})];
    let q = query();
    let graph = normalize(&input, &q, true).unwrap();
    let record = &graph.records[0];
    assert_eq!(record.storage_class, MetadataValue::Unknown);
    assert!(matches!(record.phase, Phase::Other(_)));
    assert!(record.deletion_timestamp.is_some());
    assert_eq!(
        record
            .pod_owner
            .as_ref()
            .unwrap()
            .id
            .as_ref()
            .unwrap()
            .as_str(),
        "pod:owner-uid"
    );
    let collection = crate::report::Collection {
        started_at: "2026-11-06T20:00:00Z".parse().unwrap(),
        finished_at: "2026-11-06T20:00:00Z".parse().unwrap(),
        context: "example".into(),
        sources: vec![],
        cost: crate::report::Cost::initial(),
    };
    let (graph, selection) = crate::report::select(graph, &q);
    let json = crate::report::build(&graph, &q, &selection, &collection, &[], &[]).unwrap();
    assert_eq!(
        json["records"][0]["pod_owner"]["relationship"],
        "declared_owner"
    );
    assert!(json["records"][0]["deletion_timestamp"].is_string());
    input.pvcs[0].spec["volumeMode"] = serde_json::json!(7);
    let graph = normalize(&input, &q, true).unwrap();
    assert!(matches!(graph.records[0].volume_mode, VolumeMode::Invalid));
    let report = crate::report::build(&graph, &q, &selection, &collection, &[], &[]).unwrap();
    assert!(report["records"][0]["volume_mode"].is_null());
}
