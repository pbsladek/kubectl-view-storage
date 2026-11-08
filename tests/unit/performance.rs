#![allow(clippy::unwrap_used, clippy::expect_used)]
//! The specification's 10,000-claim/50,000-reference request-cost fixture.
use crate::{
    cli::Cli,
    clock::tests::TestClock,
    collect::{self, Response, ResponseFuture, Transport},
    group,
};
use clap::Parser;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use tokio_util::sync::CancellationToken;

struct PagedApi {
    paths: Mutex<Vec<String>>,
}
impl Transport for PagedApi {
    fn get(&self, path: String) -> ResponseFuture<'_> {
        Box::pin(async move {
            self.paths.lock().unwrap().push(path.clone());
            let (url, query) = path.split_once('?').unwrap_or((&path, ""));
            let start = query
                .split('&')
                .find_map(|part| part.strip_prefix("continue="))
                .map(|v| v.parse::<usize>().unwrap())
                .unwrap_or(0);
            let body = match url {
                "/api/v1/persistentvolumeclaims"
                | "/api/v1/namespaces/demo/persistentvolumeclaims" => page(start, 10_000, claim),
                "/api/v1/persistentvolumes" => page(start, 10_000, pv),
                "/api/v1/pods" | "/api/v1/namespaces/demo/pods" => page(start, 50_000, pod),
                "/apis/storage.k8s.io/v1/storageclasses/fast" => {
                    json!({"metadata":{"name":"fast"},"provisioner":"test.csi"})
                }
                _ if url.contains("/stats/summary") => {
                    let node = url
                        .split('/')
                        .nth(4)
                        .unwrap()
                        .strip_prefix("node-")
                        .unwrap()
                        .parse::<usize>()
                        .unwrap();
                    json!({"pods":(node*500..node*500+500).map(|i|json!({"podRef":{"name":format!("app-{i}"),"namespace":"demo","uid":format!("pod-{i}")},"volume":[{"name":"data","pvcRef":{"name":format!("claim-{}",i/5),"namespace":"demo"},"time":"2026-11-06T19:59:50Z","capacityBytes":100,"usedBytes":40,"availableBytes":55}]})).collect::<Vec<_>>()})
                }
                _ if url.starts_with("/api/v1/persistentvolumes/pv-") => {
                    pv(url.rsplit('-').next().unwrap().parse().unwrap())
                }
                _ => {
                    return Ok(Response {
                        status: 404,
                        body: vec![],
                        retry_after: None,
                    });
                }
            };
            Ok(Response {
                status: 200,
                body: serde_json::to_vec(&body).unwrap(),
                retry_after: None,
            })
        })
    }
}
fn page(start: usize, total: usize, object: fn(usize) -> Value) -> Value {
    let end = (start + 500).min(total);
    json!({"metadata":{"resourceVersion":"100","continue":if end<total {end.to_string()} else {String::new()}},"items":(start..end).map(object).collect::<Vec<_>>()})
}
fn claim(i: usize) -> Value {
    json!({"metadata":{"name":format!("claim-{i}"),"namespace":"demo","uid":format!("claim-{i}"),"creationTimestamp":"2026-11-01T00:00:00Z"},"spec":{"storageClassName":"fast","volumeName":format!("pv-{i}"),"resources":{"requests":{"storage":"100Gi"}}},"status":{"phase":"Bound","capacity":{"storage":"100Gi"}}})
}
fn pv(i: usize) -> Value {
    json!({"metadata":{"name":format!("pv-{i}"),"uid":format!("pv-{i}")},"spec":{"storageClassName":"fast","capacity":{"storage":"100Gi"},"csi":{"driver":"test.csi"},"claimRef":{"namespace":"demo","name":format!("claim-{i}"),"uid":format!("claim-{i}")}},"status":{"phase":"Bound"}})
}
fn pod(i: usize) -> Value {
    json!({"metadata":{"name":format!("app-{i}"),"namespace":"demo","uid":format!("pod-{i}")},"spec":{"nodeName":format!("node-{}",i/500),"volumes":[{"name":"data","persistentVolumeClaim":{"claimName":format!("claim-{}",i/5)}}]},"status":{"phase":"Running"}})
}

#[tokio::test(start_paused = true)]
async fn specification_scale_uses_bulk_pages_and_one_statistics_request_per_node() {
    let api = Arc::new(PagedApi {
        paths: Mutex::new(vec![]),
    });
    let cli = Cli::try_parse_from(["cmd", "-A", "-u"]).unwrap();
    let query = cli.query("demo").unwrap();
    let clock = Arc::new(TestClock::new("2026-11-06T20:00:00Z".parse().unwrap()));
    let collected = collect::collect(
        api.clone(),
        clock,
        CancellationToken::new(),
        &cli,
        &query,
        "scale".into(),
    )
    .await
    .unwrap();
    assert_eq!(collected.graph.records.len(), 10_000);
    assert_eq!(collected.graph.relationships.len(), 50_000);
    assert_eq!(collected.collection.cost.base_attempts, 20);
    assert_eq!(collected.collection.cost.enrichment_attempts, 121);
    assert_eq!(collected.collection.cost.node_requests, 100);
    assert_eq!(collected.collection.cost.node_targets, 100);
    let paths = api.paths.lock().unwrap();
    assert_eq!(
        paths
            .iter()
            .filter(|p| p.starts_with("/api/v1/persistentvolumes?"))
            .count(),
        20
    );
    assert!(
        !paths
            .iter()
            .any(|p| p.starts_with("/api/v1/persistentvolumes/pv-"))
    );
    drop(paths);
    let totals = group::totals(
        &collected.graph.records.iter().collect::<Vec<_>>(),
        &[],
        true,
        true,
    );
    assert_eq!(totals.provisioned.sum, Some(10_000 * 107374182400));
    assert_eq!(totals.filesystem.used, Some(400_000));
    assert_eq!(totals.filesystem.coverage.fresh_complete, 10_000);
}

#[tokio::test(start_paused = true)]
async fn specification_scale_namespace_query_stops_at_enrichment_budget() {
    let api = Arc::new(PagedApi {
        paths: Mutex::new(vec![]),
    });
    let cli = Cli::try_parse_from(["cmd", "-n", "demo", "-u"]).unwrap();
    let query = cli.query("demo").unwrap();
    let clock = Arc::new(TestClock::new("2026-11-06T20:00:00Z".parse().unwrap()));
    let collected = collect::collect(
        api.clone(),
        clock,
        CancellationToken::new(),
        &cli,
        &query,
        "scale".into(),
    )
    .await
    .unwrap();
    assert_eq!(collected.collection.cost.enrichment_attempts, 256);
    assert_eq!(collected.collection.cost.node_requests, 100);
    assert_eq!(collected.graph.records.len(), 10_000);
    let known = collected
        .graph
        .records
        .iter()
        .filter(|r| r.provisioned.value().is_some())
        .count();
    assert_eq!(known, 156);
    assert!(
        collected
            .graph
            .records
            .iter()
            .all(|r| r.filesystem.status() == "ok")
    );
    let paths = api.paths.lock().unwrap();
    assert_eq!(
        paths
            .iter()
            .filter(|p| p.starts_with("/api/v1/persistentvolumes/pv-"))
            .count(),
        156
    );
    assert!(
        !paths
            .iter()
            .any(|p| p.starts_with("/api/v1/persistentvolumes?"))
    );
}
