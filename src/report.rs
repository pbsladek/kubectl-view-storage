//! Explicit domain → wire adapters. Counters are strings; unknowns stay null.
use crate::{
    cli::Query,
    group::{self, Group, MetricTotal, QuantitySum, Totals},
    join::{self, Graph},
    model::*,
    qty::ExactRatio,
    work::{self, CheckedError, Checkpoint, Unbounded},
};
use jiff::Timestamp;
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Default, Serialize)]
pub struct Cost {
    pub enrichment_strategy: BTreeMap<String, String>,
    pub enrichment_budget: usize,
    pub enrichment_attempts: usize,
    pub base_attempts: usize,
    pub node_targets: usize,
    pub node_requests: usize,
    pub response_bytes: usize,
    pub priority_sequence: Vec<String>,
    pub unresolved_prerequisites: BTreeSet<String>,
}
impl Cost {
    pub fn initial() -> Self {
        Self {
            enrichment_budget: 256,
            priority_sequence: vec![
                "membership".into(),
                "utilization_targets".into(),
                "display".into(),
            ],
            ..Self::default()
        }
    }
}
#[derive(Debug, Clone, Serialize)]
pub struct SourceCollection {
    pub resource: String,
    pub scope: String,
    pub verb: String,
    pub status: String,
    pub resource_version: Option<String>,
}
#[derive(Debug, Clone)]
pub struct Collection {
    pub started_at: Timestamp,
    pub finished_at: Timestamp,
    pub context: String,
    pub sources: Vec<SourceCollection>,
    pub cost: Cost,
}
#[derive(Debug, Clone, Serialize)]
pub struct UnknownRecord {
    pub id: String,
    pub namespace: Option<String>,
    pub name: String,
    pub unresolved_fields: BTreeSet<String>,
}
#[derive(Debug, Clone, Default, Serialize)]
pub struct Selection {
    pub complete: bool,
    pub base_count: usize,
    pub matched_count: usize,
    pub excluded_count: usize,
    pub unknown_count: usize,
    pub unknown_records: Vec<UnknownRecord>,
}

pub fn select(graph: Graph, query: &Query) -> (Graph, Selection) {
    select_checked(graph, query, &mut Unbounded).unwrap_or_else(|never| match never {})
}
pub(crate) fn select_checked<C: Checkpoint>(
    mut graph: Graph,
    query: &Query,
    control: &mut C,
) -> Result<(Graph, Selection), C::Error> {
    let mut selection = Selection {
        complete: true,
        base_count: graph.records.len(),
        ..Selection::default()
    };
    let mut lookup = BTreeMap::new();
    for consumer in &graph.consumers {
        control.step()?;
        lookup.insert(consumer.id.clone(), consumer);
    }
    let mut selected = vec![];
    for record in std::mem::take(&mut graph.records) {
        control.step()?;
        match join::matches_checked(&record, query, &lookup, control)? {
            Predicate::Match => {
                selection.matched_count += 1;
                selected.push(record);
            }
            Predicate::NoMatch => {
                selection.excluded_count += 1;
            }
            Predicate::Unknown(fields) => {
                selection.complete = false;
                selection.unknown_count += 1;
                selection.unknown_records.push(UnknownRecord {
                    id: record.id.as_str().into(),
                    namespace: record.namespace.known().cloned(),
                    name: record.name.clone(),
                    unresolved_fields: fields,
                });
            }
        }
    }
    drop(lookup);
    graph.records = selected;
    let mut records = BTreeSet::new();
    for record in &graph.records {
        control.step()?;
        records.insert(record.id.clone());
    }
    let mut consumers = BTreeSet::new();
    for edge in std::mem::take(&mut graph.relationships) {
        control.step()?;
        if records.contains(&edge.record_id) {
            consumers.insert(edge.consumer_id.clone());
            graph.relationships.push(edge);
        }
    }
    for consumer in std::mem::take(&mut graph.consumers) {
        control.step()?;
        if consumers.contains(&consumer.id) {
            graph.consumers.push(consumer);
        }
    }
    for candidate in &selection.unknown_records {
        control.step()?;
        graph.diagnostics.push(Diagnostic {
            code: "selection_unknown".into(),
            subject_id: RecordId::new(
                query.kind,
                candidate
                    .id
                    .split_once(':')
                    .map(|(_, uid)| uid)
                    .unwrap_or(""),
            )
            .ok(),
            source: "kubernetes".into(),
            reason: "unresolved_predicate".into(),
            affected_fields: candidate.unresolved_fields.iter().cloned().collect(),
            message: "Filter membership remains unknown; query totals are lower bounds.".into(),
        });
    }
    Ok((graph, selection))
}

pub fn build(
    graph: &Graph,
    query: &Query,
    selection: &Selection,
    collection: &Collection,
    groups: &[Group],
    findings: &[Finding],
) -> Result<Value, String> {
    build_checked(
        graph,
        query,
        selection,
        collection,
        groups,
        findings,
        &mut Unbounded,
    )
    .map_err(CheckedError::into_message)
}
pub(crate) fn build_checked<C: Checkpoint>(
    graph: &Graph,
    query: &Query,
    selection: &Selection,
    collection: &Collection,
    groups: &[Group],
    findings: &[Finding],
    control: &mut C,
) -> Result<Value, CheckedError<C::Error>> {
    let mut records = vec![];
    let mut refs = vec![];
    for record in &graph.records {
        control.checked()?;
        records.push(record_checked(record, control).map_err(CheckedError::Interrupted)?);
        refs.push(record);
    }
    let mut consumers = vec![];
    for consumer in &graph.consumers {
        control.checked()?;
        let mut rendered = json!({"id":consumer.id.as_str(),"uid":consumer.uid,"namespace":consumer.namespace,"name":consumer.name,"phase":consumer.phase,"deletion_timestamp":timestamp(consumer.deletion_timestamp),"node":consumer.node,"active":(consumer.state!=ReferenceState::Unknown).then_some(consumer.state==ReferenceState::Active),"reference_state":consumer.state.as_str()});
        if let Some(owner) = consumer.statefulset.known() {
            rendered["statefulset"] = json!({"name":owner.name,"uid":owner.uid});
        } else if matches!(consumer.statefulset, MetadataValue::Unknown) {
            rendered["statefulset_state"] = json!("unknown");
        }
        consumers.push(rendered);
    }
    let mut relationships = vec![];
    for edge in &graph.relationships {
        control.checked()?;
        let mut value = json!({"id":format!("edge:{}:{}", edge.record_id.as_str().split_once(':').map(|(_,uid)|uid).unwrap_or(""),edge.consumer_id.as_str().split_once(':').map(|(_,uid)|uid).unwrap_or("")),"record_id":edge.record_id.as_str(),"consumer_id":edge.consumer_id.as_str(),"runtime_state":"unverified"});
        value["volume_names"] =
            strings_checked(edge.volume_names.iter().map(String::as_str), control)
                .map_err(CheckedError::Interrupted)?;
        let mut mounts = vec![];
        for declared in &edge.mounts {
            control.checked()?;
            mounts.push(mount(declared));
        }
        value["mounts"] = Value::Array(mounts);
        relationships.push(value);
    }
    let total = group::totals_checked(
        &refs,
        findings,
        selection.complete,
        query.utilization,
        control,
    )
    .map_err(CheckedError::Interrupted)?;
    let mut summary = totals_checked(&total, control).map_err(CheckedError::Interrupted)?;
    summary["overlapping_children"] =
        Value::Bool(group::overlap_checked(groups, control).map_err(CheckedError::Interrupted)?);
    let mut rendered_groups = vec![];
    for group in groups {
        control.checked()?;
        rendered_groups.push(group_checked(group, control).map_err(CheckedError::Interrupted)?);
    }
    let mut rendered_findings = vec![];
    for finding in findings {
        control.checked()?;
        rendered_findings
            .push(finding_checked(finding, control).map_err(CheckedError::Interrupted)?);
    }
    let mut diagnostics = vec![];
    for diagnostic in &graph.diagnostics {
        control.checked()?;
        diagnostics
            .push(diagnostic_checked(diagnostic, control).map_err(CheckedError::Interrupted)?);
    }
    // Move completed containers into the DOM. Feeding a large Vec<Value> back
    // through json! would recursively clone it without processing checkpoints.
    let mut result = json!({"schema_version":"view-storage/v1alpha1", "query":{"kind":query.kind,"group_by":query.group_by,"filters":query.filters,"utilization":query.utilization,"metrics_source":if query.utilization {Some("kubelet")} else {None},"max_metrics_age_seconds":duration_seconds(query.max_metrics_age),"warn_used_percent":query.warn_used.to_string(),"critical_used_percent":query.critical_used.to_string(),"require_metrics":query.require_metrics,"fail_on":query.fail_on,"sort":query.sort.to_string()}});
    result["records"] = Value::Array(records);
    result["consumers"] = Value::Array(consumers);
    result["relationships"] = Value::Array(relationships);
    result["summary"] = summary;
    result["groups"] = Value::Array(rendered_groups);
    result["findings"] = Value::Array(rendered_findings);
    result["diagnostics"] = Value::Array(diagnostics);
    result["collection"] =
        collection_checked(collection, query, control).map_err(CheckedError::Interrupted)?;
    result["selection"] =
        selection_checked(selection, control).map_err(CheckedError::Interrupted)?;
    Ok(result)
}

fn duration_seconds(value: std::time::Duration) -> Value {
    if value.subsec_nanos() == 0 {
        json!(value.as_secs())
    } else {
        json!(value.as_secs_f64())
    }
}

fn timestamp(value: Option<Timestamp>) -> Option<String> {
    value.map(|t| t.to_string())
}
fn source(value: Option<&Source>) -> Value {
    value.map(|s|json!({"provider":"kubelet","node":s.node,"pod_id":s.pod_id.as_str(),"volume_name":s.volume_name})).unwrap_or(Value::Null)
}
fn percentage(value: Option<ExactRatio>) -> Value {
    value
        .and_then(|ratio| {
            let mut display = ratio.percent();
            if !display.contains('.') {
                display.push_str(".0");
            }
            display.parse::<serde_json::Number>().ok()
        })
        .map(Value::Number)
        .unwrap_or(Value::Null)
}
fn counter<T: Counter>(field: &Field<T>) -> Value {
    field
        .value()
        .map(|v| json!(v.raw().to_string()))
        .unwrap_or(Value::Null)
}
fn mode(mode: &VolumeMode) -> Option<&str> {
    match mode {
        VolumeMode::Filesystem => Some("Filesystem"),
        VolumeMode::Block => Some("Block"),
        VolumeMode::Unsupported(s) => Some(s),
        VolumeMode::Invalid => None,
    }
}
fn record_checked<C: Checkpoint>(record: &Record, control: &mut C) -> Result<Value, C::Error> {
    let mut value = json!({"id":record.id.as_str(),"kind":record.kind,"uid":record.uid,"namespace":record.namespace.known(),"name":record.name,"created_at":timestamp(record.created_at),"phase":record.phase.as_str(),
        "storage_class":record.storage_class.known(),"storage_class_state":if matches!(record.storage_class, MetadataValue::Absent) {"classless"} else {record.storage_class.state()},"driver":record.driver.known(),"volume_mode":mode(&record.volume_mode),"reclaim_policy":record.reclaim_policy,"pv_name":record.pv_name,
        "requested_bytes":counter(&record.requested),"bound_bytes":counter(&record.bound),"provisioned_bytes":counter(&record.provisioned),"consumer_visibility":if matches!(record.consumer_visibility,ConsumerVisibility::Complete) {"complete"} else {"unknown"},
        "active_consumer_count":record.active_count,"terminating_consumer_count":record.terminating_count});
    if record.kind == crate::cli::Kind::Pv {
        value["pvc_name"] = json!(record.pvc_name.known());
        value["pvc_uid"] = json!(record.pvc_uid);
    }
    if let Some(time) = record.deletion_timestamp {
        value["deletion_timestamp"] = json!(timestamp(Some(time)));
    }
    if record.overlapping_filesystem {
        value["overlapping_filesystem"] = json!(true);
    }
    if let Some(mode) = &record.volume_binding_mode {
        value["volume_binding_mode"] = json!(mode);
    }
    if let Some(owner) = &record.pod_owner {
        value["pod_owner"] = json!({"name":owner.name,"id":owner.id.as_ref().map(ConsumerId::as_str),"relationship":"declared_owner"});
    }
    value["consumer_ids"] =
        strings_checked(record.consumers.iter().map(ConsumerId::as_str), control)?;
    value["access_modes"] =
        strings_checked(record.access_modes.iter().map(String::as_str), control)?;
    value["filesystem"] = family_checked(
        &record.filesystem,
        ["capacity_bytes", "used_bytes", "available_bytes"],
        control,
    )?;
    value["inodes"] = family_checked(
        &record.inodes,
        ["inodes_total", "inodes_used", "inodes_free"],
        control,
    )?;
    Ok(value)
}
fn candidate<T: Counter>(value: &Candidate<T>, keys: [&str; 3]) -> Value {
    let reason = value.reason();
    let mut result = json!({"status":value.status(),"reason":reason,"source":source(Some(&value.source)),"observed_at":timestamp(value.observed_at),"field_status":{},"used_percent":percentage(value.ratio())});
    for (key, field) in keys
        .into_iter()
        .zip([&value.capacity, &value.used, &value.available])
    {
        result[key] = counter(field);
        result["field_status"][key] = json!(field.status());
    }
    result
}
fn family_checked<T: Counter, C: Checkpoint>(
    value: &MetricFamily<T>,
    keys: [&str; 3],
    control: &mut C,
) -> Result<Value, C::Error> {
    let mut result = match value {
        MetricFamily::Observed { selected, .. } => candidate(selected, keys),
        _ => {
            let reason = if let MetricFamily::Unavailable { reason, .. } = value {
                Some(reason.as_str())
            } else {
                None
            };
            let mut result = json!({"status":value.status(),"reason":reason,"source":null,"observed_at":null,"field_status":{},"used_percent":null});
            for key in keys {
                result[key] = Value::Null;
                result["field_status"][key] = json!("missing");
            }
            result
        }
    };
    let mut candidates = vec![];
    if let MetricFamily::Observed {
        candidates: observations,
        ..
    } = value
    {
        for observation in observations {
            control.step()?;
            candidates.push(candidate(observation, keys));
        }
    }
    result["candidate_count"] = json!(candidates.len());
    result["candidates"] = Value::Array(candidates);
    result["observed_range_percent"] = value
        .observed_range_checked(control)?
        .map(|(min, max)| json!({"min":percentage(Some(min)),"max":percentage(Some(max))}))
        .unwrap_or(Value::Null);
    if matches!(
        value,
        MetricFamily::Observed {
            disagreement: Some(_),
            ..
        }
    ) {
        result["status"] = json!("uncertain");
        result["reason"] = json!("metrics_disagreement");
    }
    Ok(result)
}
fn mount(value: &Mount) -> Value {
    json!({"container":value.container,"container_kind":match value.container_kind {ContainerKind::App=>"app",ContainerKind::Init=>"init",ContainerKind::Ephemeral=>"ephemeral"},"volume_name":value.volume_name,"kind":match value.kind {MountKind::Filesystem=>"filesystem",MountKind::Block=>"block"},"path":value.path,"read_only":value.read_only,"sub_path":value.sub_path,"sub_path_expr":value.sub_path_expr})
}
fn quantity_sum(value: &QuantitySum) -> Value {
    json!({"known_sum_bytes":value.sum.map(|v|v.to_string()),"known_count":value.known_count,"total_count":value.total_count,"complete":value.complete})
}
fn metric_total_checked<C: Checkpoint>(
    value: &MetricTotal,
    keys: [&str; 3],
    control: &mut C,
) -> Result<Value, C::Error> {
    let mut result = json!({"status":value.status,"reason":value.reason,"used_percent":percentage(value.ratio),"coverage":{"eligible":value.coverage.eligible,"fresh_complete":value.coverage.fresh_complete,"not_applicable":value.coverage.not_applicable},"oldest_observed_at":timestamp(value.oldest),"newest_observed_at":timestamp(value.newest),"physical_total":false});
    result["coverage"]["by_reason"] = counts_checked(
        value
            .coverage
            .by_reason
            .iter()
            .map(|(key, count)| (key.as_str(), *count)),
        control,
    )?;
    for (key, value) in keys
        .into_iter()
        .zip([value.capacity, value.used, value.available])
    {
        result[key] = json!(value.map(|value| value.to_string()));
    }
    Ok(result)
}
fn totals_checked<C: Checkpoint>(value: &Totals, control: &mut C) -> Result<Value, C::Error> {
    let mut result = json!({"selection_complete":value.selection_complete,"record_count":value.record_count,"requested":quantity_sum(&value.requested),"bound":quantity_sum(&value.bound),"provisioned":quantity_sum(&value.provisioned),"finding_counts":value.finding_counts});
    result["phase_counts"] = counts_checked(
        value
            .phase_counts
            .iter()
            .map(|(key, count)| (key.as_str(), *count)),
        control,
    )?;
    result["filesystem"] = metric_total_checked(
        &value.filesystem,
        ["capacity_bytes", "used_bytes", "available_bytes"],
        control,
    )?;
    result["inodes"] = metric_total_checked(
        &value.inodes,
        ["inodes_total", "inodes_used", "inodes_free"],
        control,
    )?;
    Ok(result)
}
fn group_checked<C: Checkpoint>(value: &Group, control: &mut C) -> Result<Value, C::Error> {
    let mut result = json!({"id":value.id,"dimension":value.dimension,"value":value.value.known(),"value_state":value.value.state(),"overlapping_children":value.overlapping_children});
    result["member_ids"] = strings_checked(value.member_ids.iter().map(RecordId::as_str), control)?;
    result["totals"] = totals_checked(&value.totals, control)?;
    let mut children = vec![];
    for child in &value.children {
        control.step()?;
        children.push(group_checked(child, control)?);
    }
    result["children"] = Value::Array(children);
    Ok(result)
}
fn finding_checked<C: Checkpoint>(value: &Finding, control: &mut C) -> Result<Value, C::Error> {
    let mut result = json!({"code":value.code,"severity":value.severity,"subject_id":value.subject_id.as_str(),"suggested_action":value.suggested_action});
    let mut evidence = vec![];
    for item in &value.evidence {
        control.step()?;
        let mut rendered = json!({"field":item.field,"unit":item.unit,"observed_at":timestamp(item.observed_at),"source":source(item.source.as_ref())});
        rendered["value"] = work::clone_json(&item.value, control)?;
        evidence.push(rendered);
    }
    result["evidence"] = Value::Array(evidence);
    Ok(result)
}
fn diagnostic_checked<C: Checkpoint>(
    value: &Diagnostic,
    control: &mut C,
) -> Result<Value, C::Error> {
    let mut result = json!({"code":value.code,"severity":"info","subject_id":value.subject_id.as_ref().map(RecordId::as_str),"source":value.source,"reason":value.reason,"message":value.message});
    result["affected_fields"] =
        strings_checked(value.affected_fields.iter().map(String::as_str), control)?;
    Ok(result)
}
fn strings_checked<'a, C: Checkpoint>(
    values: impl IntoIterator<Item = &'a str>,
    control: &mut C,
) -> Result<Value, C::Error> {
    let mut result = vec![];
    for value in values {
        control.step()?;
        result.push(Value::String(value.into()));
    }
    Ok(Value::Array(result))
}
fn counts_checked<'a, C: Checkpoint>(
    values: impl IntoIterator<Item = (&'a str, usize)>,
    control: &mut C,
) -> Result<Value, C::Error> {
    let mut result = serde_json::Map::new();
    for (key, count) in values {
        control.step()?;
        result.insert(key.into(), json!(count));
    }
    Ok(Value::Object(result))
}
fn collection_checked<C: Checkpoint>(
    collection: &Collection,
    query: &Query,
    control: &mut C,
) -> Result<Value, C::Error> {
    let cost = &collection.cost;
    let mut result = json!({"started_at":timestamp(Some(collection.started_at)),"finished_at":timestamp(Some(collection.finished_at)),"context":collection.context,"scope":{"all_namespaces":query.namespace.is_none(),"namespace":query.namespace}});
    let mut sources = vec![];
    for source in &collection.sources {
        control.step()?;
        sources.push(json!({"resource":source.resource,"scope":source.scope,"verb":source.verb,"status":source.status,"resource_version":source.resource_version}));
    }
    result["sources"] = Value::Array(sources);
    let mut rendered_cost = json!({"enrichment_budget":cost.enrichment_budget,"enrichment_attempts":cost.enrichment_attempts,"base_attempts":cost.base_attempts,"node_targets":cost.node_targets,"node_requests":cost.node_requests,"response_bytes":cost.response_bytes});
    let mut strategy = serde_json::Map::new();
    for (key, value) in &cost.enrichment_strategy {
        control.step()?;
        strategy.insert(key.clone(), Value::String(value.clone()));
    }
    rendered_cost["enrichment_strategy"] = Value::Object(strategy);
    rendered_cost["priority_sequence"] =
        strings_checked(cost.priority_sequence.iter().map(String::as_str), control)?;
    rendered_cost["unresolved_prerequisites"] = strings_checked(
        cost.unresolved_prerequisites.iter().map(String::as_str),
        control,
    )?;
    result["cost"] = rendered_cost;
    Ok(result)
}
fn selection_checked<C: Checkpoint>(
    selection: &Selection,
    control: &mut C,
) -> Result<Value, C::Error> {
    let mut result = json!({"complete":selection.complete,"base_count":selection.base_count,"matched_count":selection.matched_count,"excluded_count":selection.excluded_count,"unknown_count":selection.unknown_count});
    let mut unknown = vec![];
    for candidate in &selection.unknown_records {
        control.step()?;
        let mut value =
            json!({"id":candidate.id,"namespace":candidate.namespace,"name":candidate.name});
        value["unresolved_fields"] = strings_checked(
            candidate.unresolved_fields.iter().map(String::as_str),
            control,
        )?;
        unknown.push(value);
    }
    result["unknown_records"] = Value::Array(unknown);
    Ok(result)
}

#[cfg(test)]
#[path = "../tests/unit/report.rs"]
mod tests;
