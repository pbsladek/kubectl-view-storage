//! One finalization path for every output format and text view.
use crate::{
    cli::{Query, Severity},
    collect::Collected,
    findings, group,
    policy::{self, Exit},
    report, sort,
    work::{self, CheckedError, Checkpoint, RunControl, RunError, Stop, Unbounded},
};
use serde_json::Value;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

#[derive(Debug)]
pub struct Report {
    pub document: Value,
    pub exit: Exit,
    pub strict_conflict: Option<Severity>,
}

#[derive(Debug, thiserror::Error)]
pub enum FinishError {
    #[error("{0}")]
    Invalid(String),
    #[error("run interrupted")]
    Interrupted,
    #[error("report processing deadline exceeded")]
    Deadline,
    #[error("report worker failed")]
    Worker,
}

/// Build the report on an owned, cooperatively cancelled CPU worker.
///
/// Collection has finished: its deadline no longer discards retained inventory.
/// User cancellation remains effective through validation, accounting, sorting,
/// grouping, and wire conversion. Cancel the token and await this future during
/// shutdown so the worker is joined before the command returns.
pub async fn finish_cancellable(
    collected: Collected,
    query: &Query,
    cancel: CancellationToken,
) -> Result<Report, FinishError> {
    let query = query.clone();
    let control = RunControl::retained_inventory(
        cancel,
        Arc::new(crate::clock::SystemClock),
        tokio::time::Instant::now(),
    );
    finish_controlled(collected, query, control).await
}

async fn finish_controlled(
    collected: Collected,
    query: Query,
    control: RunControl,
) -> Result<Report, FinishError> {
    work::run(control, move |control| {
        finish_checked(collected, &query, control)
    })
    .await
    .map_err(|error| match error {
        RunError::Invalid(message) => FinishError::Invalid(message),
        RunError::Stopped(Stop::Cancelled) => FinishError::Interrupted,
        RunError::Stopped(Stop::Deadline) => FinishError::Deadline,
        RunError::Worker => FinishError::Worker,
    })
}
pub fn finish(collected: Collected, query: &Query) -> Result<Report, String> {
    finish_checked(collected, query, &mut Unbounded).map_err(CheckedError::into_message)
}
pub(crate) fn finish_checked<C: Checkpoint>(
    mut collected: Collected,
    query: &Query,
    control: &mut C,
) -> Result<Report, CheckedError<C::Error>> {
    collected.graph.validate_checked(control)?;
    let findings = findings::evaluate_checked(
        &collected.graph,
        query,
        collected.collection.finished_at,
        control,
    )
    .map_err(CheckedError::Interrupted)?;
    let exit = policy::decide_checked(
        collected.selection.complete,
        &collected.graph,
        &findings,
        query,
        control,
    )
    .map_err(CheckedError::Interrupted)?;
    let strict_conflict = if exit == Exit::Inconclusive
        && policy::conflicts_reach_policy_checked(&collected.graph, query, control)
            .map_err(CheckedError::Interrupted)?
    {
        query.fail_on
    } else {
        None
    };
    sort::records_checked(&mut collected.graph.records, query.sort, &findings, control)
        .map_err(CheckedError::Interrupted)?;
    let mut groups = group::build_checked(
        &collected.graph,
        query,
        &findings,
        collected.selection.complete,
        control,
    )
    .map_err(CheckedError::Interrupted)?;
    sort::groups_checked(&mut groups, query, control).map_err(CheckedError::Interrupted)?;
    sort::diagnostics_checked(&mut collected.graph.diagnostics, control)
        .map_err(CheckedError::Interrupted)?;
    let document = report::build_checked(
        &collected.graph,
        query,
        &collected.selection,
        &collected.collection,
        &groups,
        &findings,
        control,
    )?;
    Ok(Report {
        document,
        exit,
        strict_conflict,
    })
}

#[cfg(test)]
#[path = "../tests/unit/pipeline.rs"]
mod tests;

#[cfg(test)]
#[path = "../tests/unit/finalization.rs"]
mod finalization_tests;
