//! Normalize API objects once, then preserve real record → pod → node edges.
use crate::{
    cli::{Kind, Query},
    model::*,
    qty::Bytes,
    transport::{Inventory, Object, string_at},
    work::{self, CheckedError, Checkpoint, Unbounded},
};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone)]
pub struct Graph {
    pub records: Vec<Record>,
    pub consumers: Vec<Consumer>,
    pub relationships: Vec<Relationship>,
    pub diagnostics: Vec<Diagnostic>,
}

impl Graph {
    pub fn validate(&self) -> Result<(), String> {
        self.validate_checked(&mut Unbounded)
            .map_err(CheckedError::into_message)
    }
    pub(crate) fn validate_checked<C: Checkpoint>(
        &self,
        control: &mut C,
    ) -> Result<(), CheckedError<C::Error>> {
        let mut records = BTreeMap::new();
        for record in &self.records {
            control.checked()?;
            records.insert(&record.id, record);
        }
        let mut consumers = BTreeMap::new();
        for consumer in &self.consumers {
            control.checked()?;
            consumers.insert(&consumer.id, consumer);
        }
        if records.len() != self.records.len() || consumers.len() != self.consumers.len() {
            return Err("duplicate graph identity".into());
        }
        let mut names = BTreeSet::new();
        for record in &self.records {
            control.checked()?;
            if RecordId::new(record.kind, &record.uid).map_err(|e| e.to_string())? != record.id
                || !names.insert((record.kind as u8, record.namespace.known(), &record.name))
            {
                return Err("record identity is inconsistent".into());
            }
        }
        for consumer in &self.consumers {
            control.checked()?;
            if ConsumerId::new(&consumer.uid).map_err(|e| e.to_string())? != consumer.id {
                return Err("consumer identity is inconsistent".into());
            }
        }
        let mut edges = BTreeSet::new();
        let mut members: BTreeMap<&RecordId, BTreeSet<&ConsumerId>> = BTreeMap::new();
        for edge in &self.relationships {
            control.checked()?;
            if !records.contains_key(&edge.record_id)
                || !consumers.contains_key(&edge.consumer_id)
                || !edges.insert((&edge.record_id, &edge.consumer_id))
            {
                return Err("dangling or duplicate relationship".into());
            }
            members
                .entry(&edge.record_id)
                .or_default()
                .insert(&edge.consumer_id);
        }
        for record in &self.records {
            control.checked()?;
            let expected = members.get(&record.id);
            if record.consumers.len() != expected.map_or(0, BTreeSet::len) {
                return Err("record consumer IDs disagree with relationships".into());
            }
            let (mut active, mut terminating, mut unsupported) = (0, 0, false);
            for id in &record.consumers {
                control.checked()?;
                if !expected.is_some_and(|members| members.contains(id)) {
                    return Err("record consumer IDs disagree with relationships".into());
                }
                match consumers.get(id).map(|consumer| consumer.state) {
                    Some(ReferenceState::Active) => active += 1,
                    Some(ReferenceState::Terminating) => terminating += 1,
                    Some(ReferenceState::Unknown) => unsupported = true,
                    _ => {}
                }
            }
            if unsupported && matches!(record.consumer_visibility, ConsumerVisibility::Complete) {
                return Err("unknown consumer lifecycle cannot have complete visibility".into());
            }
            let counts = match record.consumer_visibility {
                ConsumerVisibility::Complete => (Some(active), Some(terminating)),
                ConsumerVisibility::Unknown(_) => (None, None),
            };
            if (record.active_count, record.terminating_count) != counts {
                return Err("consumer counts disagree with visibility or lifecycle".into());
            }
        }
        Ok(())
    }
}

pub fn normalize(input: &Inventory, query: &Query, pods_complete: bool) -> Result<Graph, String> {
    normalize_checked(input, query, pods_complete, &mut Unbounded)
        .map_err(CheckedError::into_message)
}

pub(crate) fn normalize_checked<C: Checkpoint>(
    input: &Inventory,
    query: &Query,
    pods_complete: bool,
    control: &mut C,
) -> Result<Graph, CheckedError<C::Error>> {
    let mut graph = Graph {
        records: vec![],
        consumers: vec![],
        relationships: vec![],
        diagnostics: vec![],
    };
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
    let mut identities = BTreeSet::new();
    let mut filesystem_records: BTreeMap<(&str, &str), Vec<usize>> = BTreeMap::new();
    let objects = if query.kind == Kind::Pvc {
        &input.pvcs
    } else {
        &input.pvs
    };
    for object in objects {
        control.checked()?;
        let uid = object.uid()?;
        if object.metadata.name.is_empty() {
            return Err("missing object name".into());
        }
        let id = RecordId::new(query.kind, uid).map_err(|e| e.to_string())?;
        if !identities.insert(id.clone()) {
            return Err("duplicate base UID".into());
        }
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
        let class = match object.spec.get("storageClassName") {
            None | Some(serde_json::Value::Null) if query.kind == Kind::Pvc => {
                MetadataValue::Unassigned
            }
            None | Some(serde_json::Value::Null) => MetadataValue::Absent,
            Some(serde_json::Value::String(name)) if name.is_empty() => MetadataValue::Absent,
            Some(serde_json::Value::String(name)) => MetadataValue::Known(name.clone()),
            _ => {
                graph.diagnostics.push(diagnostic(
                    "invalid_api_field",
                    &id,
                    "storage_class",
                    "Storage class has an unsupported type; classification remains unknown.",
                ));
                MetadataValue::Unknown
            }
        };
        let driver = verified_pv
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
            });
        let phase = Phase::from_api_for(
            query.kind,
            string_at(&object.status, "/phase").unwrap_or("Unknown"),
        );
        if matches!(phase, Phase::Other(_)) {
            graph.diagnostics.push(diagnostic(
                "unsupported_api_value",
                &id,
                "phase",
                "unfamiliar API phase preserved",
            ));
        }
        let raw_mode = match object.spec.get("volumeMode") {
            None | Some(serde_json::Value::Null) => Some("Filesystem"),
            Some(serde_json::Value::String(mode)) => Some(mode.as_str()),
            _ => None,
        };
        let mode = match raw_mode {
            Some("Filesystem") => VolumeMode::Filesystem,
            Some("Block") => VolumeMode::Block,
            Some(s) => {
                graph.diagnostics.push(diagnostic(
                    "unsupported_api_value",
                    &id,
                    "volume_mode",
                    "unsupported volume mode",
                ));
                VolumeMode::Unsupported(s.to_owned())
            }
            None => {
                graph.diagnostics.push(diagnostic(
                    "invalid_api_field",
                    &id,
                    "volume_mode",
                    "volume mode has an invalid type; classification remains unknown",
                ));
                VolumeMode::Invalid
            }
        };
        let requested = quantity(
            compatible_claim.and_then(|pvc| pvc.spec.pointer("/resources/requests/storage")),
            &id,
            "requested_bytes",
            &mut graph.diagnostics,
        );
        let bound = quantity(
            compatible_claim.and_then(|pvc| pvc.status.pointer("/capacity/storage")),
            &id,
            "bound_bytes",
            &mut graph.diagnostics,
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
            &mut graph.diagnostics,
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
        // CSI defines volumeHandle as the unique volume name within a driver.
        // An exact duplicate proves overlap; equal capacities, class names or
        // opaque handle prefixes never do. Keep identifiers internal.
        if mode == VolumeMode::Filesystem
            && let Some(pv) = verified_pv
            && pv
                .spec
                .get("volumeMode")
                .is_none_or(|mode| mode.is_null() || mode.as_str() == Some("Filesystem"))
            && let (Some(driver), Some(handle)) = (
                string_at(&pv.spec, "/csi/driver").filter(|s| !s.is_empty()),
                string_at(&pv.spec, "/csi/volumeHandle").filter(|s| !s.is_empty()),
            )
        {
            filesystem_records
                .entry((driver, handle))
                .or_default()
                .push(graph.records.len());
        }
        graph.records.push(Record {
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
                .and_then(|name| classes.get(name.as_str()))
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
        });
    }
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
    let mut lookup = BTreeMap::new();
    for (index, record) in graph.records.iter().enumerate() {
        control.checked()?;
        if (record.kind != Kind::Pv || !record.binding_conflict)
            && let (Some(namespace), Some(claim)) =
                (record.namespace.known(), record.pvc_name.known())
        {
            lookup.insert((namespace.as_str(), claim.as_str()), index);
        }
    }
    let mut edges: BTreeMap<(usize, ConsumerId), Relationship> = BTreeMap::new();
    for pod in &input.pods {
        control.checked()?;
        let ns = pod.namespace()?;
        let mut refs: BTreeMap<usize, BTreeSet<String>> = BTreeMap::new();
        if let Some(volumes) = pod
            .spec
            .get("volumes")
            .and_then(serde_json::Value::as_array)
        {
            for volume in volumes {
                control.checked()?;
                if let (Some(name), Some(volume_name)) = (
                    string_at(volume, "/persistentVolumeClaim/claimName"),
                    string_at(volume, "/name"),
                ) && let Some(&index) = lookup.get(&(ns, name))
                {
                    refs.entry(index)
                        .or_default()
                        .insert(volume_name.to_owned());
                }
            }
        }
        if refs.is_empty() {
            continue;
        }
        let uid = pod.uid()?;
        let id = ConsumerId::new(uid).map_err(|e| e.to_string())?;
        let phase = string_at(&pod.status, "/phase").unwrap_or("Unknown");
        let state = match phase {
            "Succeeded" | "Failed" => ReferenceState::Terminal,
            "Pending" | "Running" | "Unknown" => {
                if pod.metadata.deletion_timestamp.is_some() {
                    ReferenceState::Terminating
                } else {
                    ReferenceState::Active
                }
            }
            _ => ReferenceState::Unknown,
        };
        graph.consumers.push(Consumer {
            id: id.clone(),
            uid: uid.to_owned(),
            namespace: ns.to_owned(),
            name: pod.metadata.name.clone(),
            phase: phase.to_owned(),
            deletion_timestamp: parse_timestamp(pod.metadata.deletion_timestamp.as_deref()),
            node: string_at(&pod.spec, "/nodeName")
                .filter(|s| !s.is_empty())
                .map(str::to_owned),
            state,
        });
        for (record_index, volume_names) in refs {
            control.checked()?;
            let mounts =
                mounts_checked(pod, &volume_names, control).map_err(CheckedError::Interrupted)?;
            edges.insert(
                (record_index, id.clone()),
                Relationship {
                    record_id: graph.records[record_index].id.clone(),
                    consumer_id: id.clone(),
                    volume_names,
                    mounts,
                },
            );
        }
    }
    let mut references = BTreeMap::new();
    for consumer in &graph.consumers {
        control.checked()?;
        references.insert(&consumer.id, consumer.state);
    }
    let mut memberships = Vec::with_capacity(graph.records.len());
    for _ in 0..graph.records.len() {
        control.checked()?;
        memberships.push(BTreeSet::new());
    }
    for (index, id) in edges.keys() {
        control.checked()?;
        memberships[*index].insert(id.clone());
    }
    drop(lookup);
    for (record, members) in graph.records.iter_mut().zip(memberships) {
        control.checked()?;
        let (mut active, mut terminating, mut unsupported) = (0, 0, false);
        for id in &members {
            control.checked()?;
            match references.get(id) {
                Some(ReferenceState::Active) => active += 1,
                Some(ReferenceState::Terminating) => terminating += 1,
                Some(ReferenceState::Unknown) => unsupported = true,
                _ => {}
            }
        }
        if unsupported {
            record.consumer_visibility =
                ConsumerVisibility::Unknown("unsupported_pod_lifecycle".into());
            record.active_count = None;
            record.terminating_count = None;
            let mut note = diagnostic(
                "unsupported_api_value",
                &record.id,
                "consumer_phase",
                "unfamiliar pod phase preserved; consumer lifecycle and counts remain unknown",
            );
            note.affected_fields.extend([
                "active_consumer_count".into(),
                "terminating_consumer_count".into(),
            ]);
            graph.diagnostics.push(note);
        } else if pods_complete {
            record.active_count = Some(active);
            record.terminating_count = Some(terminating);
        }
        record.consumers = members;
    }
    for edge in edges.into_values() {
        control.checked()?;
        graph.relationships.push(edge);
    }
    work::sort_by(
        &mut graph.records,
        |a, b| {
            (&a.namespace.known(), &a.name, &a.uid).cmp(&(&b.namespace.known(), &b.name, &b.uid))
        },
        control,
    )
    .map_err(CheckedError::Interrupted)?;
    work::sort_by(
        &mut graph.consumers,
        |a, b| (&a.namespace, &a.name, &a.uid).cmp(&(&b.namespace, &b.name, &b.uid)),
        control,
    )
    .map_err(CheckedError::Interrupted)?;
    work::sort_by(
        &mut graph.relationships,
        |a, b| (&a.record_id, &a.consumer_id).cmp(&(&b.record_id, &b.consumer_id)),
        control,
    )
    .map_err(CheckedError::Interrupted)?;
    Ok(graph)
}

pub fn diagnostic(code: &str, id: &RecordId, field: &str, message: &str) -> Diagnostic {
    Diagnostic {
        code: code.into(),
        subject_id: Some(id.clone()),
        source: "kubernetes".into(),
        reason: code.into(),
        affected_fields: vec![field.into()],
        message: message.into(),
    }
}
fn parse_timestamp(value: Option<&str>) -> Option<jiff::Timestamp> {
    value.and_then(|s| s.parse().ok())
}
fn quantity(
    value: Option<&serde_json::Value>,
    id: &RecordId,
    field: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Field<Bytes> {
    match value {
        None | Some(serde_json::Value::Null) => Field::Missing,
        Some(value) => match value.as_str().and_then(|s| s.parse::<Bytes>().ok()) {
            Some(bytes) => Field::Valid(bytes),
            None => {
                diagnostics.push(diagnostic(
                    "invalid_quantity",
                    id,
                    field,
                    "quantity is malformed or outside u64 range",
                ));
                Field::Invalid
            }
        },
    }
}
fn mounts_checked<C: Checkpoint>(
    pod: &Object,
    names: &BTreeSet<String>,
    control: &mut C,
) -> Result<Vec<Mount>, C::Error> {
    let mut out = vec![];
    for (key, container_kind) in [
        ("containers", ContainerKind::App),
        ("initContainers", ContainerKind::Init),
        ("ephemeralContainers", ContainerKind::Ephemeral),
    ] {
        control.step()?;
        for container in pod
            .spec
            .get(key)
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
        {
            control.step()?;
            for (key, kind, path) in [
                ("volumeMounts", MountKind::Filesystem, "/mountPath"),
                ("volumeDevices", MountKind::Block, "/devicePath"),
            ] {
                control.step()?;
                for mount in container
                    .get(key)
                    .and_then(serde_json::Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    control.step()?;
                    if let (Some(name), Some(path)) =
                        (string_at(mount, "/name"), string_at(mount, path))
                        && names.contains(name)
                    {
                        out.push(Mount {
                            container_kind: container_kind.clone(),
                            container: string_at(container, "/name").unwrap_or("").to_owned(),
                            volume_name: name.to_owned(),
                            path: path.to_owned(),
                            kind: kind.clone(),
                            read_only: if kind == MountKind::Filesystem {
                                match mount.get("readOnly") {
                                    None => Some(false),
                                    Some(value) => value.as_bool(),
                                }
                            } else {
                                None
                            },
                            sub_path: string_at(mount, "/subPath").map(str::to_owned),
                            sub_path_expr: string_at(mount, "/subPathExpr").map(str::to_owned),
                        });
                    }
                }
            }
        }
    }
    work::sort_by(&mut out, Ord::cmp, control)?;
    let mut unique = vec![];
    for mount in out {
        control.step()?;
        if unique.last() != Some(&mount) {
            unique.push(mount);
        }
    }
    Ok(unique)
}

fn strings_checked<C: Checkpoint>(
    value: &serde_json::Value,
    path: &str,
    control: &mut C,
) -> Result<Vec<String>, C::Error> {
    let mut strings = vec![];
    for value in value
        .pointer(path)
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
    {
        control.step()?;
        if let Some(value) = value.as_str() {
            strings.push(value.to_owned());
        }
    }
    Ok(strings)
}
fn pod_owner_checked<C: Checkpoint>(
    claim: Option<&Object>,
    control: &mut C,
) -> Result<Option<PodOwner>, C::Error> {
    let Some(claim) = claim else {
        return Ok(None);
    };
    for owner in &claim.metadata.owner_references {
        control.step()?;
        if string_at(owner, "/kind") == Some("Pod")
            && let Some(name) = string_at(owner, "/name")
        {
            return Ok(Some(PodOwner {
                name: name.into(),
                id: string_at(owner, "/uid").and_then(|uid| ConsumerId::new(uid).ok()),
            }));
        }
    }
    Ok(None)
}

pub fn matches(
    record: &Record,
    query: &Query,
    consumers: &BTreeMap<ConsumerId, &Consumer>,
) -> Predicate {
    matches_checked(record, query, consumers, &mut Unbounded).unwrap_or_else(|never| match never {})
}
pub(crate) fn matches_checked<C: Checkpoint>(
    record: &Record,
    query: &Query,
    consumers: &BTreeMap<ConsumerId, &Consumer>,
    control: &mut C,
) -> Result<Predicate, C::Error> {
    let mut result = Predicate::Match;
    for (filter, value, field) in [
        (&query.filters.pvc, &record.pvc_name, "pvc"),
        (
            &query.filters.storage_class,
            &record.storage_class,
            "storage_class",
        ),
        (&query.filters.driver, &record.driver, "driver"),
    ] {
        if let Some(filter) = filter {
            let predicate = match value {
                MetadataValue::Known(value) if filter == value => Predicate::Match,
                MetadataValue::Absent if filter.is_empty() => Predicate::Match,
                MetadataValue::Unknown => Predicate::Unknown(BTreeSet::from([field.into()])),
                _ => Predicate::NoMatch,
            };
            result = result.and(predicate);
        }
    }
    if let Some(filter) = &query.filters.pv {
        result = result.and(if record.pv_name.as_ref() == Some(filter) {
            Predicate::Match
        } else {
            Predicate::NoMatch
        });
    }
    if let Some(node) = &query.filters.node {
        let (mut known, mut uncertain) = (false, false);
        for id in &record.consumers {
            control.step()?;
            if let Some(consumer) = consumers.get(id) {
                known |= consumer.state.live() && consumer.node.as_ref() == Some(node);
                uncertain |= consumer.state == ReferenceState::Unknown;
                if known {
                    break;
                }
            }
        }
        result = result.and(if known {
            Predicate::Match
        } else if !matches!(record.consumer_visibility, ConsumerVisibility::Complete) || uncertain {
            Predicate::Unknown(BTreeSet::from(["node".into()]))
        } else {
            Predicate::NoMatch
        });
    }
    Ok(result)
}

#[cfg(test)]
#[path = "../tests/unit/join.rs"]
mod tests;
