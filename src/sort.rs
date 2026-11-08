//! Reverse the known primary key only; unknowns and identity ties stay ordered.
use crate::{
    cli::{Query, Sort, SortField},
    group::{Group, QuantitySum},
    model::*,
    work::{self, Checkpoint, Unbounded},
};
use std::{cmp::Ordering, collections::BTreeMap};

pub fn optional<T: Ord>(a: Option<T>, b: Option<T>, descending: bool) -> Ordering {
    match (a, b) {
        (Some(a), Some(b)) => {
            if descending {
                b.cmp(&a)
            } else {
                a.cmp(&b)
            }
        }
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}
pub(crate) fn diagnostics_checked<C: Checkpoint>(
    diagnostics: &mut Vec<Diagnostic>,
    control: &mut C,
) -> Result<(), C::Error> {
    for diagnostic in diagnostics.iter_mut() {
        control.step()?;
        work::sort_by(&mut diagnostic.affected_fields, Ord::cmp, control)?;
        for field in std::mem::take(&mut diagnostic.affected_fields) {
            control.step()?;
            if diagnostic.affected_fields.last() != Some(&field) {
                diagnostic.affected_fields.push(field);
            }
        }
    }
    work::sort_by(
        diagnostics,
        |a, b| {
            (
                &a.subject_id,
                &a.code,
                &a.source,
                &a.reason,
                &a.affected_fields,
                &a.message,
            )
                .cmp(&(
                    &b.subject_id,
                    &b.code,
                    &b.source,
                    &b.reason,
                    &b.affected_fields,
                    &b.message,
                ))
        },
        control,
    )
}

pub fn records(records: &mut Vec<Record>, sort: Sort, findings: &[Finding]) {
    records_checked(records, sort, findings, &mut Unbounded).unwrap_or_else(|never| match never {})
}
pub(crate) fn records_checked<C: Checkpoint>(
    records: &mut Vec<Record>,
    sort: Sort,
    findings: &[Finding],
    control: &mut C,
) -> Result<(), C::Error> {
    let mut severities = BTreeMap::new();
    for f in findings {
        control.step()?;
        severities
            .entry(&f.subject_id)
            .and_modify(|s: &mut crate::cli::Severity| *s = (*s).max(f.severity))
            .or_insert(f.severity);
    }
    work::sort_by(
        records,
        |a, b| {
            let order = match sort.field {
                SortField::Name => optional(Some(&a.name), Some(&b.name), sort.descending),
                SortField::Requested => {
                    optional(a.requested.value(), b.requested.value(), sort.descending)
                }
                SortField::Provisioned => optional(
                    a.provisioned.value(),
                    b.provisioned.value(),
                    sort.descending,
                ),
                SortField::Used => optional(
                    a.filesystem.selected().and_then(|m| m.used.value()),
                    b.filesystem.selected().and_then(|m| m.used.value()),
                    sort.descending,
                ),
                SortField::Available => optional(
                    a.filesystem.selected().and_then(|m| m.available.value()),
                    b.filesystem.selected().and_then(|m| m.available.value()),
                    sort.descending,
                ),
                SortField::UsedPercent => optional(
                    a.filesystem.selected().and_then(Candidate::ratio),
                    b.filesystem.selected().and_then(Candidate::ratio),
                    sort.descending,
                ),
                SortField::InodePercent => optional(
                    a.inodes.selected().and_then(Candidate::ratio),
                    b.inodes.selected().and_then(Candidate::ratio),
                    sort.descending,
                ),
                SortField::Severity => optional(
                    Some(severities.get(&a.id)),
                    Some(severities.get(&b.id)),
                    sort.descending,
                ),
            };
            order.then_with(|| {
                (a.namespace.known(), &a.name, &a.uid).cmp(&(b.namespace.known(), &b.name, &b.uid))
            })
        },
        control,
    )
}
pub fn groups(groups: &mut Vec<Group>, query: &Query) {
    groups_checked(groups, query, &mut Unbounded).unwrap_or_else(|never| match never {})
}
pub(crate) fn groups_checked<C: Checkpoint>(
    groups: &mut Vec<Group>,
    query: &Query,
    control: &mut C,
) -> Result<(), C::Error> {
    let known_sum = |sum: &QuantitySum| if sum.complete { sum.sum } else { None };
    work::sort_by(
        groups,
        |a, b| {
            let comparison = match query.sort.field {
                SortField::Name => {
                    optional(a.value.known(), b.value.known(), query.sort.descending)
                }
                SortField::Requested => optional(
                    known_sum(&a.totals.requested),
                    known_sum(&b.totals.requested),
                    query.sort.descending,
                ),
                SortField::Provisioned => optional(
                    known_sum(&a.totals.provisioned),
                    known_sum(&b.totals.provisioned),
                    query.sort.descending,
                ),
                SortField::Used => optional(
                    (a.totals.filesystem.status == "ok")
                        .then_some(a.totals.filesystem.used)
                        .flatten(),
                    (b.totals.filesystem.status == "ok")
                        .then_some(b.totals.filesystem.used)
                        .flatten(),
                    query.sort.descending,
                ),
                SortField::Available => optional(
                    (a.totals.filesystem.status == "ok")
                        .then_some(a.totals.filesystem.available)
                        .flatten(),
                    (b.totals.filesystem.status == "ok")
                        .then_some(b.totals.filesystem.available)
                        .flatten(),
                    query.sort.descending,
                ),
                SortField::UsedPercent => optional(
                    (a.totals.filesystem.status == "ok")
                        .then_some(a.totals.filesystem.ratio)
                        .flatten(),
                    (b.totals.filesystem.status == "ok")
                        .then_some(b.totals.filesystem.ratio)
                        .flatten(),
                    query.sort.descending,
                ),
                SortField::InodePercent => optional(
                    (a.totals.inodes.status == "ok")
                        .then_some(a.totals.inodes.ratio)
                        .flatten(),
                    (b.totals.inodes.status == "ok")
                        .then_some(b.totals.inodes.ratio)
                        .flatten(),
                    query.sort.descending,
                ),
                SortField::Severity => {
                    let severity = |g: &Group| {
                        if g.totals.finding_counts["critical"] > 0 {
                            3
                        } else if g.totals.finding_counts["warning"] > 0 {
                            2
                        } else if g.totals.finding_counts["info"] > 0 {
                            1
                        } else {
                            0
                        }
                    };
                    optional(Some(severity(a)), Some(severity(b)), query.sort.descending)
                }
            };
            comparison.then_with(|| a.id.cmp(&b.id))
        },
        control,
    )?;
    for group in groups {
        control.step()?;
        groups_checked(&mut group.children, query, control)?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "../tests/unit/sort.rs"]
mod tests;
