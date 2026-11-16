//! Provider requests and observation processing with owned worker shutdown.
use super::{
    AtomicAttempts, Collector, DocumentBody, Failure, PrometheusTransport, RequestContext,
    Transport, decode_document, node_request, processing_failure, shutdown, valid_summary,
};
use crate::{
    cli::{Cli, Query},
    join, metrics,
    model::*,
    report::SourceCollection,
    work::{self, CheckedError, Checkpoint, RunControl},
};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, atomic::Ordering},
};
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

async fn retain_node_response<T: Transport>(
    request: &RequestContext<T>,
    node: String,
    result: Result<DocumentBody, Failure>,
    responses: &mut BTreeMap<String, Value>,
    failures: &mut BTreeMap<String, String>,
) -> Result<(), Failure> {
    match result {
        Ok(body) => {
            // Only the supervisor owns parsing work. Async HTTP
            // workers may be aborted after the shutdown allowance;
            // that must never drop an unjoined blocking parser.
            match decode_document(request, body).await {
                Ok(value) if valid_summary(&value) => {
                    responses.insert(node, value);
                }
                Ok(_) => {
                    failures.insert(node, Failure::Invalid.reason().into());
                }
                Err(error) => {
                    if error.fatal() || matches!(error, Failure::Cancelled) {
                        return Err(error);
                    }
                    failures.insert(node, error.reason().into());
                }
            }
        }
        Err(error) => {
            if error.fatal() {
                return Err(error);
            }
            failures.insert(node, error.reason().into());
        }
    };
    Ok(())
}

pub(super) async fn collect_prometheus<T: Transport>(
    collector: &mut Collector<T>,
    query: &Query,
    cancel: &CancellationToken,
) -> Result<(Option<Value>, String), Failure> {
    let clock = collector.request.clock.clone();
    let overall = collector.overall;
    let mut prometheus_response = None;
    let mut prometheus_failure = "no_sample".to_owned();
    let request = RequestContext {
        transport: Arc::new(PrometheusTransport(collector.request.transport.clone())),
        clock: clock.clone(),
        cancel: cancel.child_token(),
        deadline: overall,
        request_timeout: collector.request.request_timeout,
        totals: Arc::new(AtomicAttempts::default()),
    };
    let cpu_query = query.clone();
    let path = work::run(
        RunControl::retained_inventory(cancel.clone(), clock.clone(), overall),
        move |control| {
            crate::prometheus::query_path(&cpu_query, control).map_err(CheckedError::Interrupted)
        },
    )
    .await
    .map_err(processing_failure)?;
    let (response, _) = node_request(&request, path).await;
    let response = match response {
        Ok(body) => decode_document(&request, body).await,
        Err(error) => Err(error),
    };
    match response {
        Ok(value) => prometheus_response = Some(value),
        Err(error) => {
            if matches!(error, Failure::Cancelled | Failure::Worker) {
                return Err(error);
            }
            prometheus_failure = error.reason().into();
        }
    }
    collector.cost.prometheus_requests = request.totals.count.load(Ordering::Relaxed);
    collector.cost.response_bytes += request.totals.bytes.load(Ordering::Relaxed);
    collector.sources.push(SourceCollection {
        resource: "prometheus/query".into(),
        scope: query
            .prometheus_cluster
            .clone()
            .unwrap_or_else(|| "single-cluster".into()),
        verb: "get".into(),
        status: if prometheus_response.is_some() {
            "complete"
        } else {
            "unavailable"
        }
        .into(),
        resource_version: None,
    });
    Ok((prometheus_response, prometheus_failure))
}

pub(super) async fn collect_node_samples<T: Transport>(
    collector: &mut Collector<T>,
    cli: &Cli,
    cancel: &CancellationToken,
    node_jobs: Vec<(String, String)>,
    target_count: usize,
) -> Result<NodeSamples, Failure> {
    let clock = collector.request.clock.clone();
    let overall = collector.overall;
    let mut responses = BTreeMap::new();
    let mut failures = BTreeMap::new();
    collector.cost.node_targets = target_count;
    let request = RequestContext {
        deadline: collector.overall,
        totals: Arc::new(AtomicAttempts::default()),
        ..collector.request.clone()
    };
    let mut pending = node_jobs.into_iter();
    let mut workers = JoinSet::new();
    loop {
        while workers.len() < usize::from(cli.concurrency)
            && clock.monotonic() < overall
            && !cancel.is_cancelled()
        {
            let Some((node, path)) = pending.next() else {
                break;
            };
            let context = request.clone();
            workers.spawn(async move {
                let (result, counts) = node_request(&context, path).await;
                (node, result, counts)
            });
        }
        if workers.is_empty() {
            break;
        }
        let result = tokio::select! {biased;_ = cancel.cancelled()=>None,_ = tokio::time::sleep_until(overall)=>None,result = workers.join_next()=>result};
        match result {
            Some(Ok((node, result, _counts))) => {
                if let Err(error) =
                    retain_node_response(&request, node, result, &mut responses, &mut failures)
                        .await
                {
                    request.cancel.cancel();
                    shutdown(&mut workers, clock.as_ref()).await;
                    return Err(error);
                }
            }
            Some(Err(_)) => {
                request.cancel.cancel();
                shutdown(&mut workers, clock.as_ref()).await;
                return Err(Failure::Worker);
            }
            None => {
                request.cancel.cancel();
                shutdown(&mut workers, clock.as_ref()).await;
                break;
            }
        }
    }
    collector.cost.node_requests = request.totals.count.load(Ordering::Relaxed);
    collector.cost.response_bytes += request.totals.bytes.load(Ordering::Relaxed);
    // Cancellation due to the overall deadline retains completed inventory.
    if cancel.is_cancelled() {
        return Err(Failure::Cancelled);
    }
    Ok((responses, failures))
}

type NodeSamples = (BTreeMap<String, Value>, BTreeMap<String, String>);

pub(super) struct Observations {
    pub(super) targets: BTreeSet<String>,
    pub(super) responses: BTreeMap<String, Value>,
    pub(super) failures: BTreeMap<String, String>,
    pub(super) prometheus_response: Option<Value>,
    pub(super) prometheus_failure: String,
}

pub(super) async fn apply_observations(
    mut graph: join::Graph,
    observations: Observations,
    query: &Query,
    finished_at: jiff::Timestamp,
    control: RunControl,
) -> Result<(join::Graph, bool), Failure> {
    let Observations {
        targets,
        responses,
        mut failures,
        prometheus_response,
        prometheus_failure,
    } = observations;
    let cpu_query = query.clone();
    let result = work::run(
        control,
        move |control| {
            for node in targets {
                control.checked()?;
                if !responses.contains_key(&node) && !failures.contains_key(&node) {
                    failures.insert(node, "deadline_exceeded".into());
                }
            }
            for (node, reason) in &failures {
                control.checked()?;
                graph.diagnostics.push(Diagnostic {
                    code: "statistics_unavailable".into(),
                    subject_id: None,
                    source: format!("kubelet/{node}"),
                    reason: reason.clone(),
                    affected_fields: vec!["filesystem".into(), "inodes".into()],
                    message: "Node statistics could not be collected; inspect proxy permissions, response validity, and timeouts.".into(),
                });
            }
            let valid_prometheus = if cpu_query.metrics_source == crate::cli::MetricsSource::Prometheus {
                crate::prometheus::apply_checked(&mut graph,prometheus_response.as_ref(),&prometheus_failure,&cpu_query,finished_at,control).map_err(CheckedError::Interrupted)?
            } else {
                metrics::apply_checked(&mut graph, &responses, &failures, &cpu_query, finished_at, control)
                    .map_err(CheckedError::Interrupted)?;
                true
            };
            Ok((graph, valid_prometheus))
        },
    ).await.map_err(processing_failure)?;
    Ok(result)
}
