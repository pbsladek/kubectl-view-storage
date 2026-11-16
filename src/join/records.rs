//! Project base objects and verified bindings into distinct storage records.
use super::{Graph, diagnostic, parse_timestamp, pod_owner_checked, quantity, strings_checked};
use crate::{
    cli::{Kind, Query},
    model::*,
    transport::{Inventory, Object, string_at},
    work::{CheckedError, Checkpoint},
};
use std::collections::{BTreeMap, BTreeSet};

pub(super) struct InventoryIndex<'a> {
    pvs: BTreeMap<&'a str, &'a Object>,
    claims: BTreeMap<(Option<&'a str>, &'a str), &'a Object>,
    classes: BTreeMap<&'a str, &'a Object>,
}

impl<'a> InventoryIndex<'a> {
    pub(super) fn new<C: Checkpoint>(
        input: &'a Inventory,
        control: &mut C,
    ) -> Result<Self, CheckedError<C::Error>> {
        let mut pvs = BTreeMap::new();
        for pv in &input.pvs {
            control.checked()?;
            pvs.insert(pv.metadata.name.as_str(), pv);
        }
        let mut claims = BTreeMap::new();
        for claim in &input.pvcs {
            control.checked()?;
            claims.insert(
                (
                    claim.metadata.namespace.as_deref(),
                    claim.metadata.name.as_str(),
                ),
                claim,
            );
        }
        let mut classes = BTreeMap::new();
        for class in &input.storage_classes {
            control.checked()?;
            classes.insert(class.metadata.name.as_str(), class);
        }
        Ok(Self {
            pvs,
            claims,
            classes,
        })
    }
}

struct Binding<'a> {
    claim: Option<&'a Object>,
    pv: Option<&'a Object>,
    namespace: MetadataValue<String>,
    pvc_name: MetadataValue<String>,
    binding_conflict: bool,
}

impl<'a> Binding<'a> {
    fn resolve(
        object: &'a Object,
        query: &Query,
        index: &InventoryIndex<'a>,
    ) -> Result<Self, String> {
        let InventoryIndex { pvs, claims, .. } = index;
        let (claim, pv, namespace, pvc_name) = if query.kind == Kind::Pvc {
            let ns = object.namespace()?.to_owned();
            (
                Some(object),
                string_at(&object.spec, "/volumeName").and_then(|name| pvs.get(name).copied()),
                MetadataValue::Known(ns),
                MetadataValue::Known(object.metadata.name.clone()),
            )
        } else {
            let ns = string_at(&object.spec, "/claimRef/namespace");
            let name = string_at(&object.spec, "/claimRef/name");
            (
                name.and_then(|name| claims.get(&(ns, name)).copied()),
                Some(object),
                ns.map(|s| MetadataValue::Known(s.to_owned()))
                    .unwrap_or(MetadataValue::Absent),
                name.map(|s| MetadataValue::Known(s.to_owned()))
                    .unwrap_or(MetadataValue::Absent),
            )
        };
        let binding_conflict = match (claim, pv) {
            (Some(claim), Some(pv)) => {
                let reference = &pv.spec;
                string_at(reference, "/claimRef/name") != Some(claim.metadata.name.as_str())
                    || string_at(reference, "/claimRef/namespace")
                        != claim.metadata.namespace.as_deref()
                    || string_at(reference, "/claimRef/uid")
                        .is_some_and(|u| Some(u) != claim.metadata.uid.as_deref())
                    || string_at(&claim.spec, "/volumeName") != Some(pv.metadata.name.as_str())
            }
            _ => false,
        };
        Ok(Self {
            claim,
            pv,
            namespace,
            pvc_name,
            binding_conflict,
        })
    }
}

fn storage_class(
    object: &Object,
    query: &Query,
    id: &RecordId,
    diagnostics: &mut Vec<Diagnostic>,
) -> MetadataValue<String> {
    match object.spec.get("storageClassName") {
        None | Some(serde_json::Value::Null) if query.kind == Kind::Pvc => {
            MetadataValue::Unassigned
        }
        None | Some(serde_json::Value::Null) => MetadataValue::Absent,
        Some(serde_json::Value::String(name)) if name.is_empty() => MetadataValue::Absent,
        Some(serde_json::Value::String(name)) => MetadataValue::Known(name.clone()),
        _ => {
            diagnostics.push(diagnostic(
                "invalid_api_field",
                id,
                "storage_class",
                "Storage class has an unsupported type; classification remains unknown.",
            ));
            MetadataValue::Unknown
        }
    }
}

fn storage_driver(
    object: &Object,
    query: &Query,
    binding_conflict: bool,
    verified_pv: Option<&Object>,
    class: &MetadataValue<String>,
    classes: &BTreeMap<&str, &Object>,
) -> MetadataValue<String> {
    verified_pv
        .and_then(|pv| string_at(&pv.spec, "/csi/driver"))
        .or_else(|| {
            // A class describes intended provisioning. When a bound PV is
            // inaccessible/conflicting it does not prove that PV's driver.
            if binding_conflict
                || (query.kind == Kind::Pvc
                    && string_at(&object.spec, "/volumeName").is_some()
                    && verified_pv.is_none())
            {
                return None;
            }
            class.known().and_then(|name| {
                classes
                    .get(name.as_str())
                    .and_then(|sc| sc.provisioner.as_deref())
            })
        })
        .map(|s| MetadataValue::Known(s.to_owned()))
        .unwrap_or_else(|| {
            if verified_pv.is_some() && matches!(class, MetadataValue::Absent) {
                MetadataValue::Absent
            } else {
                MetadataValue::Unknown
            }
        })
}

fn volume_mode(object: &Object, id: &RecordId, diagnostics: &mut Vec<Diagnostic>) -> VolumeMode {
    let raw_mode = match object.spec.get("volumeMode") {
        None | Some(serde_json::Value::Null) => Some("Filesystem"),
        Some(serde_json::Value::String(mode)) => Some(mode.as_str()),
        _ => None,
    };
    match raw_mode {
        Some("Filesystem") => VolumeMode::Filesystem,
        Some("Block") => VolumeMode::Block,
        Some(s) => {
            diagnostics.push(diagnostic(
                "unsupported_api_value",
                id,
                "volume_mode",
                "unsupported volume mode",
            ));
            VolumeMode::Unsupported(s.to_owned())
        }
        None => {
            diagnostics.push(diagnostic(
                "invalid_api_field",
                id,
                "volume_mode",
                "volume mode has an invalid type; classification remains unknown",
            ));
            VolumeMode::Invalid
        }
    }
}

fn filesystem_identity<'a>(
    pv: Option<&'a Object>,
    mode: &VolumeMode,
) -> Option<(&'a str, &'a str)> {
    // CSI defines volumeHandle as the unique volume name within a driver.
    // An exact duplicate proves overlap; equal capacities, class names or
    // opaque handle prefixes never do. Keep identifiers internal.
    if *mode == VolumeMode::Filesystem
        && let Some(pv) = pv
        && pv
            .spec
            .get("volumeMode")
            .is_none_or(|mode| mode.is_null() || mode.as_str() == Some("Filesystem"))
        && let (Some(driver), Some(handle)) = (
            string_at(&pv.spec, "/csi/driver").filter(|s| !s.is_empty()),
            string_at(&pv.spec, "/csi/volumeHandle").filter(|s| !s.is_empty()),
        )
    {
        return Some((driver, handle));
    }
    None
}

pub(super) fn normalize_record<'a, C: Checkpoint>(
    object: &'a Object,
    id: RecordId,
    query: &Query,
    pods_complete: bool,
    index: &InventoryIndex<'a>,
    diagnostics: &mut Vec<Diagnostic>,
    control: &mut C,
) -> Result<NormalizedRecord<'a>, CheckedError<C::Error>> {
    let uid = object.uid()?;
    let Binding {
        claim,
        pv,
        namespace,
        pvc_name,
        binding_conflict,
    } = Binding::resolve(object, query, index)?;
    let verified_pv = if binding_conflict && query.kind == Kind::Pvc {
        None
    } else {
        pv
    };
    let compatible_claim = if binding_conflict && query.kind == Kind::Pv {
        None
    } else {
        claim
    };
    let class = storage_class(object, query, &id, diagnostics);
    let driver = storage_driver(
        object,
        query,
        binding_conflict,
        verified_pv,
        &class,
        &index.classes,
    );
    let phase = Phase::from_api_for(
        query.kind,
        string_at(&object.status, "/phase").unwrap_or("Unknown"),
    );
    if matches!(phase, Phase::Other(_)) {
        diagnostics.push(diagnostic(
            "unsupported_api_value",
            &id,
            "phase",
            "unfamiliar API phase preserved",
        ));
    }
    let mode = volume_mode(object, &id, diagnostics);
    let requested = quantity(
        compatible_claim.and_then(|pvc| pvc.spec.pointer("/resources/requests/storage")),
        &id,
        "requested_bytes",
        diagnostics,
    );
    let bound = quantity(
        compatible_claim.and_then(|pvc| pvc.status.pointer("/capacity/storage")),
        &id,
        "bound_bytes",
        diagnostics,
    );
    let provisioned = quantity(
        (if query.kind == Kind::Pv {
            pv
        } else {
            verified_pv
        })
        .and_then(|pv| pv.spec.pointer("/capacity/storage")),
        &id,
        "provisioned_bytes",
        diagnostics,
    );
    let metric = if !query.utilization {
        MetricFamily::NotRequested
    } else if mode == VolumeMode::Block {
        MetricFamily::NotApplicable
    } else {
        MetricFamily::Unavailable {
            status: "missing".into(),
            reason: "no_sample".into(),
        }
    };
    let inodes = if !query.utilization {
        MetricFamily::NotRequested
    } else if mode == VolumeMode::Block {
        MetricFamily::NotApplicable
    } else {
        MetricFamily::Unavailable {
            status: "missing".into(),
            reason: "no_sample".into(),
        }
    };
    let filesystem_identity = filesystem_identity(verified_pv, &mode);
    let record = Record {
        id,
        kind: query.kind,
        uid: uid.to_owned(),
        namespace,
        name: object.metadata.name.clone(),
        created_at: parse_timestamp(object.metadata.creation_timestamp.as_deref()),
        deletion_timestamp: parse_timestamp(object.metadata.deletion_timestamp.as_deref()),
        pod_owner: pod_owner_checked(compatible_claim, control)
            .map_err(CheckedError::Interrupted)?,
        claim_created_at: compatible_claim
            .and_then(|c| parse_timestamp(c.metadata.creation_timestamp.as_deref())),
        phase,
        volume_binding_mode: class
            .known()
            .and_then(|name| index.classes.get(name.as_str()))
            .and_then(|class| class.volume_binding_mode.clone()),
        storage_class: class,
        driver,
        volume_mode: mode,
        access_modes: strings_checked(&object.spec, "/accessModes", control)
            .map_err(CheckedError::Interrupted)?,
        reclaim_policy: verified_pv
            .and_then(|pv| string_at(&pv.spec, "/persistentVolumeReclaimPolicy"))
            .map(str::to_owned),
        pv_name: if query.kind == Kind::Pv {
            Some(object.metadata.name.clone())
        } else {
            string_at(&object.spec, "/volumeName").map(str::to_owned)
        },
        pvc_name,
        pvc_uid: compatible_claim.and_then(|claim| claim.metadata.uid.clone()),
        requested,
        bound,
        provisioned,
        consumers: BTreeSet::new(),
        consumer_visibility: if pods_complete {
            ConsumerVisibility::Complete
        } else {
            ConsumerVisibility::Unknown("consumers_unknown".into())
        },
        active_count: pods_complete.then_some(0),
        terminating_count: pods_complete.then_some(0),
        binding_conflict,
        filesystem: metric,
        inodes,
        overlapping_filesystem: false,
    };
    Ok(NormalizedRecord {
        record,
        filesystem: filesystem_identity,
    })
}

pub(super) fn mark_overlapping_filesystems<C: Checkpoint>(
    graph: &mut Graph,
    filesystem_records: BTreeMap<(&str, &str), Vec<usize>>,
    control: &mut C,
) -> Result<(), CheckedError<C::Error>> {
    for indices in filesystem_records.values() {
        control.checked()?;
        if indices.len() < 2 {
            continue;
        }
        for index in indices {
            control.checked()?;
            let record = &mut graph.records[*index];
            record.overlapping_filesystem = true;
            let mut note = diagnostic(
                "overlapping_filesystem",
                &record.id,
                "filesystem",
                "Distinct records refer to the same CSI volume identity; their observations are excluded from additive filesystem totals.",
            );
            note.affected_fields.push("inodes".into());
            graph.diagnostics.push(note);
        }
    }
    Ok(())
}

pub(super) struct NormalizedRecord<'a> {
    pub(super) record: Record,
    pub(super) filesystem: Option<(&'a str, &'a str)>,
}
