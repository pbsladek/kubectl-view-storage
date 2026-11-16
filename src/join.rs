//! Normalize API objects once, then preserve real record → pod → node edges.
use crate::{
    cli::{Kind, Query},
    model::*,
    qty::Bytes,
    transport::{Inventory, Object, string_at},
    work::{self, CheckedError, Checkpoint, Unbounded},
};
use std::collections::{BTreeMap, BTreeSet};

mod consumers;
mod records;
use consumers::attach_consumers;
use records::{InventoryIndex, NormalizedRecord, mark_overlapping_filesystems, normalize_record};

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
        self.validate_identities(control)?;
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
            validate_consumer_membership(record, members.get(&record.id), &consumers, control)?;
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
    let index = InventoryIndex::new(input, control)?;
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
        let id = RecordId::new(query.kind, uid).map_err(|error| error.to_string())?;
        if !identities.insert(id.clone()) {
            return Err("duplicate base UID".into());
        }
        let NormalizedRecord { record, filesystem } = normalize_record(
            object,
            id,
            query,
            pods_complete,
            &index,
            &mut graph.diagnostics,
            control,
        )?;
        if let Some(identity) = filesystem {
            filesystem_records
                .entry(identity)
                .or_default()
                .push(graph.records.len());
        }
        graph.records.push(record);
    }
    mark_overlapping_filesystems(&mut graph, filesystem_records, control)?;
    attach_consumers(&mut graph, input, query, pods_complete, control)?;
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
        result = result.and(matches_node(record, node, consumers, control)?);
    }
    if let Some(name) = &query.filters.statefulset {
        result = result.and(matches_statefulset(record, name, consumers, control)?);
    }
    if let Some(filter) = &query.filters.workload {
        result = result.and(matches_workload(record, filter, consumers, control)?);
    }
    Ok(result)
}

fn validate_consumer_membership<C: Checkpoint>(
    record: &Record,
    expected: Option<&BTreeSet<&ConsumerId>>,
    consumers: &BTreeMap<&ConsumerId, &Consumer>,
    control: &mut C,
) -> Result<(), CheckedError<C::Error>> {
    control.checked()?;
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
    Ok(())
}

fn matches_node<C: Checkpoint>(
    record: &Record,
    node: &String,
    consumers: &BTreeMap<ConsumerId, &Consumer>,
    control: &mut C,
) -> Result<Predicate, C::Error> {
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
    Ok(if known {
        Predicate::Match
    } else if !matches!(record.consumer_visibility, ConsumerVisibility::Complete) || uncertain {
        Predicate::Unknown(BTreeSet::from(["node".into()]))
    } else {
        Predicate::NoMatch
    })
}

fn matches_statefulset<C: Checkpoint>(
    record: &Record,
    name: &String,
    consumers: &BTreeMap<ConsumerId, &Consumer>,
    control: &mut C,
) -> Result<Predicate, C::Error> {
    let (mut known, mut uncertain) = (false, false);
    for id in &record.consumers {
        control.step()?;
        if let Some(consumer) = consumers.get(id) {
            known |= consumer.state.live()
                && consumer
                    .statefulset
                    .known()
                    .is_some_and(|owner| &owner.name == name);
            uncertain |= consumer.state == ReferenceState::Unknown
                || consumer.state.live() && matches!(consumer.statefulset, MetadataValue::Unknown);
        }
    }
    Ok(if known {
        Predicate::Match
    } else if !matches!(record.consumer_visibility, ConsumerVisibility::Complete) || uncertain {
        Predicate::Unknown(BTreeSet::from(["statefulset".into()]))
    } else {
        Predicate::NoMatch
    })
}

fn matches_workload<C: Checkpoint>(
    record: &Record,
    filter: &String,
    consumers: &BTreeMap<ConsumerId, &Consumer>,
    control: &mut C,
) -> Result<Predicate, C::Error> {
    let (mut known, mut uncertain) = (false, false);
    for id in &record.consumers {
        control.step()?;
        if let Some(consumer) = consumers.get(id) {
            if consumer.state.live() {
                for owner in &consumer.workload_chain {
                    control.step()?;
                    known |= format!("{}/{}", owner.kind, owner.name) == *filter;
                }
                uncertain |= matches!(consumer.workload, MetadataValue::Unknown);
            }
            uncertain |= consumer.state == ReferenceState::Unknown;
        }
    }
    Ok(if known {
        Predicate::Match
    } else if uncertain || !matches!(record.consumer_visibility, ConsumerVisibility::Complete) {
        Predicate::Unknown(BTreeSet::from(["workload".into()]))
    } else {
        Predicate::NoMatch
    })
}

impl Graph {
    fn validate_identities<C: Checkpoint>(
        &self,
        control: &mut C,
    ) -> Result<(), CheckedError<C::Error>> {
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
        Ok(())
    }
}

#[cfg(test)]
#[path = "../tests/unit/join.rs"]
mod tests;
