//! Exit precedence is centralized and consumes the same data as the report.
use crate::{
    cli::Query,
    findings,
    join::Graph,
    model::*,
    work::{Checkpoint, Unbounded},
};
use std::collections::BTreeSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Exit {
    Success = 0,
    Fatal = 1,
    Invalid = 2,
    Findings = 3,
    Inconclusive = 4,
    Interrupted = 130,
}

pub fn decide(
    selection_complete: bool,
    graph: &Graph,
    observed: &[Finding],
    query: &Query,
) -> Exit {
    decide_checked(selection_complete, graph, observed, query, &mut Unbounded)
        .unwrap_or_else(|never| match never {})
}
pub(crate) fn decide_checked<C: Checkpoint>(
    selection_complete: bool,
    graph: &Graph,
    observed: &[Finding],
    query: &Query,
    control: &mut C,
) -> Result<Exit, C::Error> {
    let strict = query.require_metrics || query.fail_on.is_some();
    if strict && !selection_complete {
        return Ok(Exit::Inconclusive);
    }
    let mut unknown_consumers = BTreeSet::new();
    if query.fail_on.is_some() {
        for consumer in &graph.consumers {
            control.step()?;
            if consumer.state == ReferenceState::Unknown {
                unknown_consumers.insert(&consumer.id);
            }
        }
    }
    for record in &graph.records {
        control.step()?;
        if strict
            && (query.utilization
                && matches!(
                    record.volume_mode,
                    VolumeMode::Unsupported(_) | VolumeMode::Invalid
                )
                || query.fail_on.is_some() && matches!(record.phase, Phase::Other(_)))
        {
            return Ok(Exit::Inconclusive);
        }
        if query.fail_on.is_some() {
            for id in &record.consumers {
                control.step()?;
                if unknown_consumers.contains(id) {
                    return Ok(Exit::Inconclusive);
                }
            }
        }
        if query.require_metrics
            && record.volume_mode != VolumeMode::Block
            && !record
                .filesystem
                .fresh_unambiguous()
                .is_some_and(Candidate::complete)
        {
            return Ok(Exit::Inconclusive);
        }
    }
    if let Some(level) = query.fail_on {
        if conflicts_reach_policy_checked(graph, query, control)? {
            return Ok(Exit::Inconclusive);
        }
        for finding in observed {
            control.step()?;
            if finding.severity >= level {
                return Ok(Exit::Findings);
            }
        }
    }
    Ok(Exit::Success)
}
pub fn conflicts_reach_policy(graph: &Graph, query: &Query) -> bool {
    conflicts_reach_policy_checked(graph, query, &mut Unbounded)
        .unwrap_or_else(|never| match never {})
}
pub(crate) fn conflicts_reach_policy_checked<C: Checkpoint>(
    graph: &Graph,
    query: &Query,
    control: &mut C,
) -> Result<bool, C::Error> {
    if query.utilization
        && let Some(level) = query.fail_on
    {
        for record in &graph.records {
            control.step()?;
            if findings::conflicting_pressure_checked(&record.filesystem, query, control)?
                .is_some_and(|severity| severity >= level)
                || findings::conflicting_pressure_checked(&record.inodes, query, control)?
                    .is_some_and(|severity| severity >= level)
            {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

#[cfg(test)]
#[path = "../tests/unit/policy.rs"]
mod tests;
