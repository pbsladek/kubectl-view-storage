//! Attach declared pod references and preserve consumer lifecycle uncertainty.
use super::{diagnostic, parse_timestamp};
use crate::{
    cli::{Kind, Query},
    join::Graph,
    model::*,
    transport::{Inventory, Object, string_at},
    work::{self, CheckedError, Checkpoint},
};
use std::collections::{BTreeMap, BTreeSet};

pub(super) fn attach_consumers<C: Checkpoint>(
    graph: &mut Graph,
    input: &Inventory,
    query: &Query,
    pods_complete: bool,
    control: &mut C,
) -> Result<(), CheckedError<C::Error>> {
    let workload_index =
        crate::workloads::index(&input.workloads, control).map_err(CheckedError::Interrupted)?;
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
        let refs = pod_references(pod, ns, &lookup, control).map_err(CheckedError::Interrupted)?;
        if refs.is_empty() {
            continue;
        }
        let uid = pod.uid()?;
        let id = ConsumerId::new(uid).map_err(|e| e.to_string())?;
        let phase = string_at(&pod.status, "/phase").unwrap_or("Unknown");
        let state = pod_reference_state(pod, phase);
        let (workload, workload_chain) = if query.needs_workloads() {
            crate::workloads::resolve(pod, &workload_index, control)
                .map_err(CheckedError::Interrupted)?
        } else {
            (MetadataValue::Absent, vec![])
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
            statefulset: statefulset_owner(pod, control).map_err(CheckedError::Interrupted)?,
            workload,
            workload_chain,
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
    update_memberships(graph, edges, pods_complete, control)
}

fn update_memberships<C: Checkpoint>(
    graph: &mut Graph,
    edges: BTreeMap<(usize, ConsumerId), Relationship>,
    pods_complete: bool,
    control: &mut C,
) -> Result<(), CheckedError<C::Error>> {
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
    Ok(())
}

fn pod_references<C: Checkpoint>(
    pod: &Object,
    ns: &str,
    lookup: &BTreeMap<(&str, &str), usize>,
    control: &mut C,
) -> Result<BTreeMap<usize, BTreeSet<String>>, C::Error> {
    let mut refs: BTreeMap<usize, BTreeSet<String>> = BTreeMap::new();
    if let Some(volumes) = pod
        .spec
        .get("volumes")
        .and_then(serde_json::Value::as_array)
    {
        for volume in volumes {
            control.step()?;
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
    Ok(refs)
}

fn pod_reference_state(pod: &Object, phase: &str) -> ReferenceState {
    match phase {
        "Succeeded" | "Failed" => ReferenceState::Terminal,
        "Pending" | "Running" | "Unknown" => {
            if pod.metadata.deletion_timestamp.is_some() {
                ReferenceState::Terminating
            } else {
                ReferenceState::Active
            }
        }
        _ => ReferenceState::Unknown,
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

fn statefulset_owner<C: Checkpoint>(
    pod: &Object,
    control: &mut C,
) -> Result<MetadataValue<StatefulSet>, C::Error> {
    let mut owner = MetadataValue::Absent;
    let mut controllers = 0;
    for reference in &pod.metadata.owner_references {
        control.step()?;
        if !reference.is_object() {
            return Ok(MetadataValue::Unknown);
        }
        if reference
            .get("controller")
            .is_some_and(|value| !value.is_boolean())
        {
            return Ok(MetadataValue::Unknown);
        }
        if reference
            .get("controller")
            .and_then(serde_json::Value::as_bool)
            != Some(true)
        {
            continue;
        }
        controllers += 1;
        if controllers > 1 {
            return Ok(MetadataValue::Unknown);
        }
        if string_at(reference, "/kind") == Some("StatefulSet") {
            owner = match (
                string_at(reference, "/apiVersion"),
                string_at(reference, "/name"),
                string_at(reference, "/uid"),
            ) {
                (Some("apps/v1"), Some(name), Some(uid))
                    if !name.is_empty()
                        && !name.chars().any(char::is_control)
                        && ConsumerId::new(uid).is_ok() =>
                {
                    MetadataValue::Known(StatefulSet {
                        name: name.into(),
                        uid: uid.into(),
                    })
                }
                _ => MetadataValue::Unknown,
            };
        } else if string_at(reference, "/kind").is_none() {
            owner = MetadataValue::Unknown;
        }
    }
    Ok(owner)
}
