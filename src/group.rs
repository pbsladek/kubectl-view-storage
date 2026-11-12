//! Reference-aware grouping: every total is computed from a member-ID union.
use crate::{
    cli::{Dimension, Query},
    join::Graph,
    model::*,
    qty::{Bytes, ExactRatio, Inodes},
    work::{Checkpoint, Unbounded},
};
use jiff::Timestamp;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone)]
pub struct QuantitySum {
    pub sum: Option<u128>,
    pub known_count: usize,
    pub total_count: usize,
    pub complete: bool,
}
#[derive(Debug, Clone, Default)]
pub struct Coverage {
    pub eligible: usize,
    pub fresh_complete: usize,
    pub not_applicable: usize,
    pub by_reason: BTreeMap<String, usize>,
}
#[derive(Debug, Clone)]
pub struct MetricTotal {
    pub status: &'static str,
    pub reason: Option<String>,
    pub capacity: Option<u128>,
    pub used: Option<u128>,
    pub available: Option<u128>,
    pub ratio: Option<ExactRatio>,
    pub coverage: Coverage,
    pub oldest: Option<Timestamp>,
    pub newest: Option<Timestamp>,
}
#[derive(Debug, Clone)]
pub struct Totals {
    pub selection_complete: bool,
    pub record_count: usize,
    pub phase_counts: BTreeMap<String, usize>,
    pub requested: QuantitySum,
    pub bound: QuantitySum,
    pub provisioned: QuantitySum,
    pub filesystem: MetricTotal,
    pub inodes: MetricTotal,
    pub finding_counts: BTreeMap<&'static str, usize>,
}
#[derive(Debug, Clone)]
pub struct Group {
    pub id: String,
    pub dimension: Dimension,
    pub value: MetadataValue<String>,
    pub member_ids: BTreeSet<RecordId>,
    pub overlapping_children: bool,
    pub totals: Totals,
    pub children: Vec<Group>,
}

type FindingIndex<'a> = BTreeMap<&'a RecordId, [usize; 3]>;
fn finding_index<'a, C: Checkpoint>(
    findings: &'a [Finding],
    control: &mut C,
) -> Result<FindingIndex<'a>, C::Error> {
    let mut index = BTreeMap::new();
    for finding in findings {
        control.step()?;
        let slot = match finding.severity {
            crate::cli::Severity::Critical => 0,
            crate::cli::Severity::Warning => 1,
            crate::cli::Severity::Info => 2,
        };
        index.entry(&finding.subject_id).or_insert([0usize; 3])[slot] += 1;
    }
    Ok(index)
}
pub fn totals(
    records: &[&Record],
    findings: &[Finding],
    selection_complete: bool,
    utilization: bool,
) -> Totals {
    totals_checked(
        records,
        findings,
        selection_complete,
        utilization,
        &mut Unbounded,
    )
    .unwrap_or_else(|never| match never {})
}
pub(crate) fn totals_checked<C: Checkpoint>(
    records: &[&Record],
    findings: &[Finding],
    selection_complete: bool,
    utilization: bool,
    control: &mut C,
) -> Result<Totals, C::Error> {
    let index = finding_index(findings, control)?;
    totals_indexed(records, &index, selection_complete, utilization, control)
}
fn totals_indexed<C: Checkpoint>(
    records: &[&Record],
    findings: &FindingIndex<'_>,
    selection_complete: bool,
    utilization: bool,
    control: &mut C,
) -> Result<Totals, C::Error> {
    let mut phases = BTreeMap::new();
    let mut counted = BTreeSet::new();
    let mut counts = [0usize; 3];
    for record in records {
        control.step()?;
        *phases.entry(record.phase.as_str().to_owned()).or_default() += 1;
        if counted.insert(&record.id)
            && let Some(found) = findings.get(&record.id)
        {
            for (count, amount) in counts.iter_mut().zip(found) {
                *count += amount;
            }
        }
    }
    Ok(Totals {
        selection_complete,
        record_count: records.len(),
        phase_counts: phases,
        requested: quantity_sum_checked(
            records,
            |record| &record.requested,
            selection_complete,
            control,
        )?,
        bound: quantity_sum_checked(records, |record| &record.bound, selection_complete, control)?,
        provisioned: quantity_sum_checked(
            records,
            |record| &record.provisioned,
            selection_complete,
            control,
        )?,
        filesystem: metric_total_checked::<Bytes, C>(
            records,
            |record| &record.filesystem,
            utilization,
            control,
        )?,
        inodes: metric_total_checked::<Inodes, C>(
            records,
            |record| &record.inodes,
            utilization,
            control,
        )?,
        finding_counts: BTreeMap::from([
            ("critical", counts[0]),
            ("warning", counts[1]),
            ("info", counts[2]),
        ]),
    })
}
fn quantity_sum_checked<C: Checkpoint>(
    records: &[&Record],
    field: impl Fn(&Record) -> &Field<Bytes>,
    selection_complete: bool,
    control: &mut C,
) -> Result<QuantitySum, C::Error> {
    let mut sum = Some(0u128);
    let mut known = 0;
    for record in records {
        control.step()?;
        if let Some(value) = field(record).value() {
            known += 1;
            sum = sum.and_then(|sum| sum.checked_add(value.get() as u128));
        }
    }
    Ok(QuantitySum {
        sum: if known == 0 && !records.is_empty() {
            None
        } else {
            sum
        },
        known_count: known,
        total_count: records.len(),
        complete: selection_complete && known == records.len() && sum.is_some(),
    })
}
fn metric_total_checked<T: Counter, C: Checkpoint>(
    records: &[&Record],
    family: impl Fn(&Record) -> &MetricFamily<T>,
    utilization: bool,
    control: &mut C,
) -> Result<MetricTotal, C::Error> {
    let mut total = MetricTotal {
        status: if utilization {
            "unavailable"
        } else {
            "not_requested"
        },
        reason: None,
        capacity: None,
        used: None,
        available: None,
        ratio: None,
        coverage: Coverage::default(),
        oldest: None,
        newest: None,
    };
    let mut sums = [Some(0u128); 3];
    for record in records {
        control.step()?;
        if record.volume_mode == VolumeMode::Block {
            total.coverage.not_applicable += 1;
            continue;
        }
        total.coverage.eligible += 1;
        if !utilization {
            continue;
        }
        let metric = family(record);
        if record.overlapping_filesystem {
            *total
                .coverage
                .by_reason
                .entry("overlapping_filesystem".into())
                .or_default() += 1;
            continue;
        }
        match metric
            .fresh_unambiguous()
            .filter(|candidate| candidate.complete())
        {
            Some(candidate) => {
                total.coverage.fresh_complete += 1;
                for (sum, field) in sums.iter_mut().zip([
                    &candidate.capacity,
                    &candidate.used,
                    &candidate.available,
                ]) {
                    control.step()?;
                    *sum = sum.and_then(|sum| sum.checked_add(field.value()?.raw() as u128));
                }
                if let Some(time) = candidate.observed_at {
                    total.oldest = Some(total.oldest.map(|old| old.min(time)).unwrap_or(time));
                    total.newest = Some(total.newest.map(|new| new.max(time)).unwrap_or(time));
                }
            }
            None => {
                let reason = metric.reason().unwrap_or_else(|| metric.status().into());
                *total.coverage.by_reason.entry(reason).or_default() += 1;
            }
        }
    }
    if !utilization {
        return Ok(total);
    }
    if total.coverage.eligible == 0 && total.coverage.not_applicable > 0 {
        total.status = "not_applicable";
        return Ok(total);
    }
    if total
        .oldest
        .zip(total.newest)
        .is_some_and(|(old, new)| new.as_nanosecond() - old.as_nanosecond() > 60_000_000_000)
    {
        total.reason = Some("time_skew".into());
        return Ok(total);
    }
    if total
        .coverage
        .by_reason
        .contains_key("overlapping_filesystem")
    {
        total.reason = Some("overlapping_filesystem".into());
    }
    if total.coverage.fresh_complete == 0 {
        total.reason.get_or_insert("no_usable_samples".into());
        return Ok(total);
    }
    if sums.iter().any(Option::is_none) {
        total.reason = Some("aggregate_overflow".into());
        return Ok(total);
    }
    total.status = if total.coverage.fresh_complete == total.coverage.eligible {
        "ok"
    } else {
        "partial"
    };
    if total.status == "partial" {
        total.reason.get_or_insert("incomplete_coverage".into());
    }
    total.capacity = sums[0];
    total.used = sums[1];
    total.available = sums[2];
    total.ratio = total
        .used
        .zip(total.capacity)
        .and_then(|(used, capacity)| ExactRatio::new(used, capacity).ok());
    Ok(total)
}

pub fn build(
    graph: &Graph,
    query: &Query,
    findings: &[Finding],
    selection_complete: bool,
) -> Vec<Group> {
    build_checked(graph, query, findings, selection_complete, &mut Unbounded)
        .unwrap_or_else(|never| match never {})
}
pub(crate) fn build_checked<C: Checkpoint>(
    graph: &Graph,
    query: &Query,
    findings: &[Finding],
    selection_complete: bool,
    control: &mut C,
) -> Result<Vec<Group>, C::Error> {
    let mut consumers = BTreeMap::new();
    for consumer in &graph.consumers {
        control.step()?;
        consumers.insert(&consumer.id, consumer);
    }
    let references = query.group_by.iter().any(|dimension| {
        matches!(
            dimension,
            Dimension::Node | Dimension::Pod | Dimension::Statefulset
        )
    });
    let mut paths: BTreeSet<PathEntry> = BTreeSet::new();
    for record in &graph.records {
        control.step()?;
        let mut contexts = vec![];
        if references {
            for id in &record.consumers {
                control.step()?;
                if let Some(&consumer) = consumers.get(id)
                    && consumer.state.live()
                {
                    contexts.push(Some(consumer));
                }
            }
        } else {
            contexts.push(None);
        }
        if contexts.is_empty()
            || references && !matches!(record.consumer_visibility, ConsumerVisibility::Complete)
        {
            contexts.push(None);
        }
        for consumer in contexts {
            control.step()?;
            let mut path = vec![];
            for &dimension in &query.group_by {
                control.step()?;
                let key = match dimension_value(record, consumer, dimension) {
                    MetadataValue::Known(value) => (value, 0, dimension),
                    MetadataValue::Absent => (String::new(), 1, dimension),
                    MetadataValue::Unknown => (String::new(), 2, dimension),
                    MetadataValue::Unassigned => (String::new(), 3, dimension),
                };
                path.push(key);
            }
            paths.insert((path, record.id.clone()));
        }
    }
    let mut entries = vec![];
    for entry in paths {
        control.step()?;
        entries.push(entry);
    }
    let mut records = BTreeMap::new();
    for record in &graph.records {
        control.step()?;
        records.insert(&record.id, record);
    }
    let findings = finding_index(findings, control)?;
    GroupBuilder {
        records,
        findings,
        complete: selection_complete,
        utilization: query.utilization,
        control,
    }
    .level(entries, 0, "")
}

fn dimension_value(
    record: &Record,
    consumer: Option<&Consumer>,
    dimension: Dimension,
) -> MetadataValue<String> {
    match dimension {
        Dimension::Storageclass => record.storage_class.clone(),
        Dimension::Namespace => record.namespace.clone(),
        Dimension::Pvc => record.pvc_name.clone(),
        Dimension::Pv => record
            .pv_name
            .clone()
            .map(MetadataValue::Known)
            .unwrap_or(MetadataValue::Absent),
        Dimension::Driver => record.driver.clone(),
        Dimension::Phase => MetadataValue::Known(record.phase.as_str().into()),
        Dimension::Volumemode => match &record.volume_mode {
            VolumeMode::Filesystem => MetadataValue::Known("Filesystem".into()),
            VolumeMode::Block => MetadataValue::Known("Block".into()),
            VolumeMode::Unsupported(value) => MetadataValue::Known(value.clone()),
            VolumeMode::Invalid => MetadataValue::Unknown,
        },
        Dimension::Accessmode => {
            let modes: BTreeSet<_> = record.access_modes.iter().map(String::as_str).collect();
            if modes.is_empty() {
                MetadataValue::Unknown
            } else {
                MetadataValue::Known(modes.into_iter().collect::<Vec<_>>().join("+"))
            }
        }
        Dimension::Statefulset => match consumer {
            Some(consumer) => match &consumer.statefulset {
                MetadataValue::Known(owner) => MetadataValue::Known(format!(
                    "{}/{}@{}",
                    consumer.namespace, owner.name, owner.uid
                )),
                MetadataValue::Unknown => MetadataValue::Unknown,
                _ => MetadataValue::Absent,
            },
            None if matches!(record.consumer_visibility, ConsumerVisibility::Complete) => {
                MetadataValue::Absent
            }
            None => MetadataValue::Unknown,
        },
        Dimension::Pod => consumer
            .map(|c| MetadataValue::Known(format!("{}/{}@{}", c.namespace, c.name, c.uid)))
            .unwrap_or_else(|| {
                if matches!(record.consumer_visibility, ConsumerVisibility::Complete) {
                    MetadataValue::Absent
                } else {
                    MetadataValue::Unknown
                }
            }),
        Dimension::Node => consumer
            .and_then(|c| c.node.clone())
            .map(MetadataValue::Known)
            .unwrap_or_else(|| {
                if matches!(record.consumer_visibility, ConsumerVisibility::Complete) {
                    MetadataValue::Absent
                } else {
                    MetadataValue::Unknown
                }
            }),
    }
}
type PathEntry = (Vec<(String, u8, Dimension)>, RecordId);
struct GroupBuilder<'a, 'c, C> {
    records: BTreeMap<&'a RecordId, &'a Record>,
    findings: FindingIndex<'a>,
    complete: bool,
    utilization: bool,
    control: &'c mut C,
}
impl<'a, C: Checkpoint> GroupBuilder<'a, '_, C> {
    fn level(
        &mut self,
        entries: Vec<PathEntry>,
        depth: usize,
        parent: &str,
    ) -> Result<Vec<Group>, C::Error> {
        let mut buckets: BTreeMap<(String, u8, Dimension), Vec<PathEntry>> = BTreeMap::new();
        for (path, id) in entries {
            self.control.step()?;
            if let Some(key) = path.get(depth) {
                buckets.entry(key.clone()).or_default().push((path, id));
            }
        }
        let mut groups = vec![];
        for ((value, state, dimension), entries) in buckets {
            self.control.step()?;
            let value = match state {
                0 => MetadataValue::Known(value),
                1 => MetadataValue::Absent,
                3 => MetadataValue::Unassigned,
                _ => MetadataValue::Unknown,
            };
            let encoded = match &value {
                MetadataValue::Known(value) => percent_escape_checked(value, self.control)?,
                MetadataValue::Absent => "~absent".into(),
                MetadataValue::Unassigned => "~unassigned".into(),
                MetadataValue::Unknown => "~unknown".into(),
            };
            let id = if parent.is_empty() {
                format!("group:{dimension}={encoded}")
            } else {
                format!("{parent}/{dimension}={encoded}")
            };
            let mut members = BTreeSet::new();
            for (_, id) in &entries {
                self.control.step()?;
                members.insert(id.clone());
            }
            let mut selected: Vec<&'a Record> = vec![];
            for id in &members {
                self.control.step()?;
                if let Some(&record) = self.records.get(id) {
                    selected.push(record);
                }
            }
            let children = self.level(entries, depth + 1, &id)?;
            let overlapping_children = overlap_checked(&children, self.control)?;
            let totals = totals_indexed(
                &selected,
                &self.findings,
                self.complete,
                self.utilization,
                self.control,
            )?;
            groups.push(Group {
                id,
                dimension,
                value,
                member_ids: members,
                overlapping_children,
                totals,
                children,
            });
        }
        Ok(groups)
    }
}
pub fn overlap(groups: &[Group]) -> bool {
    overlap_checked(groups, &mut Unbounded).unwrap_or_else(|never| match never {})
}
pub(crate) fn overlap_checked<C: Checkpoint>(
    groups: &[Group],
    control: &mut C,
) -> Result<bool, C::Error> {
    let mut seen = BTreeSet::new();
    for group in groups {
        control.step()?;
        for id in &group.member_ids {
            control.step()?;
            if !seen.insert(id) {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

fn percent_escape_checked<C: Checkpoint>(input: &str, control: &mut C) -> Result<String, C::Error> {
    let mut result = String::new();
    for byte in input.bytes() {
        control.step()?;
        if byte.is_ascii_alphanumeric() || b"-._".contains(&byte) {
            result.push(char::from(byte));
        } else {
            use std::fmt::Write;
            let _ = write!(result, "%{byte:02X}");
        }
    }
    Ok(result)
}

#[cfg(test)]
#[path = "../tests/unit/group.rs"]
mod tests;
