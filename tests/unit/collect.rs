#![allow(clippy::unwrap_used, clippy::expect_used)]
use super::*;
use clap::Parser;
use std::sync::Mutex;

#[path = "collect_clock.rs"]
mod clock_interactions;
#[path = "collect_budget.rs"]
mod combined_budget;

#[derive(Default)]
struct MockTransport {
    replies: Mutex<BTreeMap<String, Vec<Response>>>,
    paths: Mutex<Vec<String>>,
}
impl Transport for MockTransport {
    fn get(&self, path: String) -> ResponseFuture<'_> {
        Box::pin(async move {
            self.paths.lock().unwrap().push(path.clone());
            self.replies
                .lock()
                .unwrap()
                .get_mut(&path)
                .and_then(|replies| {
                    if replies.is_empty() {
                        None
                    } else {
                        Some(replies.remove(0))
                    }
                })
                .ok_or(Failure::Transport)
        })
    }
}
fn response(value: Value) -> Response {
    Response {
        status: 200,
        body: serde_json::to_vec(&value).unwrap(),
        retry_after: None,
    }
}
fn fixture_transport() -> Arc<MockTransport> {
    let input: Value =
        serde_json::from_str(include_str!("../../examples/contract/healthy/input.json")).unwrap();
    let transport = Arc::new(MockTransport::default());
    transport.replies.lock().unwrap().extend([
        ("/api/v1/namespaces/demo/persistentvolumeclaims?limit=500".into(),vec![response(serde_json::json!({"metadata":{"resourceVersion":"100"},"items":input["kubernetes"]["pvcs"]}))]),
        ("/api/v1/namespaces/demo/pods?limit=500".into(),vec![response(serde_json::json!({"metadata":{"resourceVersion":"101"},"items":input["kubernetes"]["pods"]}))]),
        ("/api/v1/persistentvolumes/pv-data".into(),vec![response(input["kubernetes"]["pvs"][0].clone())]),
        ("/apis/storage.k8s.io/v1/storageclasses/fast".into(),vec![response(input["kubernetes"]["storage_classes"][0].clone())]),
        ("/api/v1/nodes/worker-a/proxy/stats/summary".into(),vec![response(input["summary_responses"]["worker-a"].clone())]),
    ]);
    transport
}
fn clock() -> Arc<dyn Clock> {
    Arc::new(crate::clock::tests::TestClock::new(
        "2026-11-06T20:00:00Z".parse().unwrap(),
    ))
}

#[tokio::test]
async fn namespace_named_cluster_never_accepts_objects_from_other_namespaces() {
    let input: Value =
        serde_json::from_str(include_str!("../../examples/contract/healthy/input.json")).unwrap();
    for (claim_namespace, pod_namespace) in [
        ("other", "cluster"),
        ("cluster", "other"),
        ("cluster", "cluster"),
    ] {
        let transport = fixture_transport();
        let mut claim = input["kubernetes"]["pvcs"][0].clone();
        claim["metadata"]["namespace"] = claim_namespace.into();
        let mut pod = input["kubernetes"]["pods"][0].clone();
        pod["metadata"]["namespace"] = pod_namespace.into();
        let mut volume = input["kubernetes"]["pvs"][0].clone();
        volume["spec"]["claimRef"]["namespace"] = "cluster".into();
        transport.replies.lock().unwrap().extend([
            (
                "/api/v1/namespaces/cluster/persistentvolumeclaims?limit=500".into(),
                vec![response(serde_json::json!({"metadata":{},"items":[claim]}))],
            ),
            (
                "/api/v1/namespaces/cluster/pods?limit=500".into(),
                vec![response(serde_json::json!({"metadata":{},"items":[pod]}))],
            ),
            (
                "/api/v1/persistentvolumes/pv-data".into(),
                vec![response(volume)],
            ),
        ]);
        let cli = Cli::try_parse_from(["cmd", "-n", "cluster"]).unwrap();
        let query = cli.query("ignored").unwrap();
        let result = collect(
            transport.clone(),
            clock(),
            CancellationToken::new(),
            &cli,
            &query,
            "test".into(),
        )
        .await;
        if claim_namespace != "cluster" {
            assert!(
                matches!(result, Err(Failure::Invalid)),
                "a wrong-namespace base object must fail required inventory"
            );
            assert_eq!(transport.paths.lock().unwrap().len(), 1);
        } else {
            let result = result.unwrap();
            assert_eq!(result.graph.records.len(), 1);
            assert_eq!(
                result.graph.records[0]
                    .namespace
                    .known()
                    .map(String::as_str),
                Some("cluster")
            );
            assert_eq!(
                result.graph.records[0].active_count,
                if pod_namespace == "cluster" {
                    Some(1)
                } else {
                    None
                }
            );
            assert_eq!(
                result
                    .collection
                    .sources
                    .iter()
                    .find(|source| source.resource == "pods")
                    .unwrap()
                    .status,
                if pod_namespace == "cluster" {
                    "complete"
                } else {
                    "partial"
                }
            );
        }
        assert!(
            transport
                .paths
                .lock()
                .unwrap()
                .iter()
                .all(|path| !path.starts_with("/api/v1/pods?")
                    && !path.starts_with("/api/v1/persistentvolumeclaims?"))
        );
    }
}

struct PlanningBudget(usize);
impl Checkpoint for PlanningBudget {
    type Error = ();
    fn step(&mut self) -> Result<(), ()> {
        self.0 = self.0.checked_sub(1).ok_or(())?;
        Ok(())
    }
}

#[test]
fn page_projection_retains_optional_siblings_and_checks_nested_work() {
    let input: Value =
        serde_json::from_str(include_str!("../../examples/contract/healthy/input.json")).unwrap();
    let pvc = input["kubernetes"]["pvcs"][0].clone();
    let page = serde_json::json!({"metadata":{"resourceVersion":"1"},"items":[pvc,{"metadata":{"name":"broken"}}]});
    let mut identities = BTreeSet::new();
    let projected = project_page(
        &page,
        "persistentvolumeclaims",
        &ApiScope::Namespace("demo".into()),
        true,
        &mut identities,
        &mut PlanningBudget(1000),
    )
    .unwrap();
    assert_eq!(projected.objects.len(), 1);
    assert!(projected.invalid_objects);
    assert!(matches!(
        project_page(
            &page,
            "persistentvolumeclaims",
            &ApiScope::Namespace("demo".into()),
            false,
            &mut BTreeSet::new(),
            &mut PlanningBudget(1000)
        ),
        Err(CheckedError::Invalid(_))
    ));
    let duplicate = serde_json::json!({"metadata":{},"items":[pvc,pvc]});
    let projected = project_page(
        &duplicate,
        "persistentvolumeclaims",
        &ApiScope::Namespace("demo".into()),
        true,
        &mut BTreeSet::new(),
        &mut PlanningBudget(1000),
    )
    .unwrap();
    assert_eq!(projected.objects.len(), 1);
    assert!(projected.invalid_objects);
    let mut huge = page.clone();
    huge["items"][0]["metadata"]["labels"] = Value::Object(
        (0..1000)
            .map(|n| (format!("label-{n}"), Value::from("value")))
            .collect(),
    );
    assert!(matches!(
        project_page(
            &huge,
            "persistentvolumeclaims",
            &ApiScope::Namespace("demo".into()),
            true,
            &mut BTreeSet::new(),
            &mut PlanningBudget(64)
        ),
        Err(CheckedError::Interrupted(()))
    ));
    for malformed in [
        serde_json::json!({"metadata":null,"items":[]}),
        serde_json::json!({"metadata":{"continue":false},"items":[]}),
        serde_json::json!({"metadata":{"resourceVersion":7},"items":[]}),
        serde_json::json!({"metadata":{},"items":{}}),
    ] {
        assert!(
            project_page(
                &malformed,
                "pods",
                &ApiScope::Namespace("demo".into()),
                true,
                &mut BTreeSet::new(),
                &mut PlanningBudget(1000)
            )
            .is_err()
        );
    }
}

#[test]
fn driver_class_prerequisites_require_unresolved_driver_and_usable_binding() {
    let input: Value =
        serde_json::from_str(include_str!("../../examples/contract/healthy/input.json")).unwrap();
    let mut original: Inventory = serde_json::from_value(input["kubernetes"].clone()).unwrap();
    original.storage_classes.clear();
    let cli = Cli::try_parse_from(["cmd", "--driver", "example.csi.test"]).unwrap();
    let query = cli.query("demo").unwrap();
    let relevant = BTreeSet::from([RecordId::new(Kind::Pvc, "data").unwrap()]);
    assert!(
        driver_class_plan(&original, &query, &relevant, &mut PlanningBudget(1000))
            .unwrap()
            .is_empty()
    );
    let mut no_csi = original.clone();
    no_csi.pvs[0].spec.as_object_mut().unwrap().remove("csi");
    assert_eq!(
        driver_class_plan(&no_csi, &query, &relevant, &mut PlanningBudget(1000)).unwrap(),
        BTreeSet::from(["fast".into()])
    );
    let mut unavailable = no_csi.clone();
    unavailable.pvs.clear();
    assert!(
        driver_class_plan(&unavailable, &query, &relevant, &mut PlanningBudget(1000))
            .unwrap()
            .is_empty()
    );
    unavailable.pvcs[0]
        .spec
        .as_object_mut()
        .unwrap()
        .remove("volumeName");
    assert_eq!(
        driver_class_plan(&unavailable, &query, &relevant, &mut PlanningBudget(1000)).unwrap(),
        BTreeSet::from(["fast".into()])
    );
    let mut conflict = no_csi.clone();
    conflict.pvs[0].spec["claimRef"]["uid"] = "recreated".into();
    assert!(
        driver_class_plan(&conflict, &query, &relevant, &mut PlanningBudget(1000))
            .unwrap()
            .is_empty()
    );
    let pv_query =
        Cli::try_parse_from(["cmd", "-A", "--kind", "pv", "--driver", "example.csi.test"])
            .unwrap()
            .query("ignored")
            .unwrap();
    let pv_ids =
        BTreeSet::from([
            RecordId::new(Kind::Pv, no_csi.pvs[0].metadata.uid.as_deref().unwrap()).unwrap(),
        ]);
    assert_eq!(
        driver_class_plan(&no_csi, &pv_query, &pv_ids, &mut PlanningBudget(1000)).unwrap(),
        BTreeSet::from(["fast".into()])
    );
    assert!(matches!(
        driver_class_plan(&no_csi, &query, &relevant, &mut PlanningBudget(1)),
        Err(CheckedError::Interrupted(()))
    ));
}

#[test]
fn dependency_plans_check_scans_and_keep_resource_identity_scopes_separate() {
    let input: Value =
        serde_json::from_str(include_str!("../../examples/contract/healthy/input.json")).unwrap();
    let mut inventory: Inventory = serde_json::from_value(input["kubernetes"].clone()).unwrap();
    let pvc = inventory.pvcs[0].clone();
    let pv = inventory.pvs[0].clone();
    let pvc_ids =
        BTreeSet::from([RecordId::new(Kind::Pvc, pvc.metadata.uid.as_deref().unwrap()).unwrap()]);
    let pv_ids =
        BTreeSet::from([RecordId::new(Kind::Pv, pv.metadata.uid.as_deref().unwrap()).unwrap()]);
    let mut unrelated = pvc.clone();
    unrelated.metadata.uid = Some("unrelated-uid".into());
    unrelated.metadata.name = pv.metadata.name.clone();
    unrelated.spec["storageClassName"] = "unrelated".into();
    inventory.pvcs.push(unrelated);
    let volumes = volume_plan(&inventory, Kind::Pvc, &pvc_ids, &mut PlanningBudget(1000)).unwrap();
    assert_eq!(volumes, BTreeSet::from(["pv-data".into()]));
    assert_eq!(
        class_plan(
            &inventory,
            Kind::Pvc,
            &pvc_ids,
            &volumes,
            &mut PlanningBudget(1000)
        )
        .unwrap(),
        BTreeSet::from(["fast".into()])
    );
    assert_eq!(
        class_plan(
            &inventory,
            Kind::Pv,
            &pv_ids,
            &volumes,
            &mut PlanningBudget(1000)
        )
        .unwrap(),
        BTreeSet::from(["fast".into()])
    );
    assert_eq!(
        claim_plan(&inventory, &pv_ids, &mut PlanningBudget(1000)).unwrap(),
        BTreeSet::from([(
            "demo".into(),
            "data".into(),
            "/api/v1/namespaces/demo/persistentvolumeclaims/data".into()
        )])
    );
    for index in 0..1000 {
        let mut claim = pvc.clone();
        claim.metadata.uid = Some(format!("other-claim-{index}"));
        inventory.pvcs.push(claim);
        let mut volume = pv.clone();
        volume.metadata.uid = Some(format!("other-volume-{index}"));
        inventory.pvs.push(volume);
    }
    assert!(volume_plan(&inventory, Kind::Pvc, &pvc_ids, &mut PlanningBudget(64)).is_err());
    assert!(
        class_plan(
            &inventory,
            Kind::Pvc,
            &pvc_ids,
            &volumes,
            &mut PlanningBudget(64)
        )
        .is_err()
    );
    assert!(claim_plan(&inventory, &pv_ids, &mut PlanningBudget(64)).is_err());
    let mut path = String::from("/prefix/");
    append_encoded(&mut path, "a/b% β", &mut PlanningBudget(100)).unwrap();
    assert_eq!(path, "/prefix/a%2Fb%25%20%CE%B2");
    assert!(append_encoded(&mut path, &"x".repeat(10000), &mut PlanningBudget(64)).is_err());
}

#[tokio::test(start_paused = true)]
async fn a_nonmatching_claim_named_like_a_volume_does_not_spend_class_lookup_budget() {
    let transport = fixture_transport();
    let input: Value =
        serde_json::from_str(include_str!("../../examples/contract/healthy/input.json")).unwrap();
    let mut unrelated = input["kubernetes"]["pvcs"][0].clone();
    unrelated["metadata"]["uid"] = "unrelated-uid".into();
    unrelated["metadata"]["name"] = "pv-data".into();
    unrelated["spec"]["storageClassName"] = "unrelated".into();
    transport.replies.lock().unwrap().insert("/api/v1/namespaces/demo/persistentvolumeclaims?limit=500".into(),vec![response(serde_json::json!({"metadata":{"resourceVersion":"100"},"items":[input["kubernetes"]["pvcs"][0],unrelated]}))]);
    let cli = Cli::try_parse_from(["cmd", "--pvc", "data"]).unwrap();
    let query = cli.query("demo").unwrap();
    let result = collect(
        transport.clone(),
        clock(),
        CancellationToken::new(),
        &cli,
        &query,
        "example".into(),
    )
    .await
    .unwrap();
    assert_eq!(result.graph.records.len(), 1);
    assert_eq!(result.selection.base_count, 2);
    assert_eq!(result.selection.excluded_count, 1);
    assert_eq!(result.collection.cost.enrichment_attempts, 3);
    assert!(
        !transport
            .paths
            .lock()
            .unwrap()
            .iter()
            .any(|path| path.contains("unrelated"))
    );
}

#[tokio::test]
async fn successful_cache_entries_share_immutable_json_and_cancel_before_reuse() {
    let transport = fixture_transport();
    let mut collector = Collector {
        request: RequestContext {
            transport: transport.clone(),
            clock: clock(),
            cancel: CancellationToken::new(),
            deadline: Instant::now() + Duration::from_secs(60),
            request_timeout: Duration::from_secs(10),
            totals: Arc::new(AtomicAttempts::default()),
        },
        overall: Instant::now() + Duration::from_secs(60),
        enrichment: true,
        allow_bulk: false,
        cost: Cost::initial(),
        cache: BTreeMap::new(),
        fatal_failure: None,
        sources: vec![],
        diagnostics: vec![],
    };
    let path = "/api/v1/persistentvolumes/pv-data";
    let first = collector.fetch(path).await.unwrap();
    let second = collector.fetch(path).await.unwrap();
    assert!(Arc::ptr_eq(&first, &second));
    let mut object = collector
        .object("persistentvolumes", &ApiScope::Cluster, path)
        .await
        .unwrap();
    object.spec["capacity"]["storage"] = "1Gi".into();
    assert_eq!(first["spec"]["capacity"]["storage"], "100Gi");
    assert_eq!(collector.cost.enrichment_attempts, 1);
    assert_eq!(transport.paths.lock().unwrap().len(), 1);
    assert!(collector.fatal().is_none());
    let denied = "/apis/storage.k8s.io/v1/storageclasses/fast";
    transport.replies.lock().unwrap().insert(
        denied.into(),
        vec![Response {
            status: 401,
            body: vec![],
            retry_after: None,
        }],
    );
    assert!(matches!(
        collector.fetch(denied).await,
        Err(Failure::Http(401))
    ));
    assert!(matches!(collector.fatal(), Some(Failure::Http(401))));
    assert!(
        matches!(collector.fetch(path).await, Err(Failure::Http(401))),
        "fatal state must precede an otherwise valid cached hit"
    );
    assert_eq!(transport.paths.lock().unwrap().len(), 2);
    collector.request.cancel.cancel();
    assert!(matches!(
        collector.fetch(path).await,
        Err(Failure::Cancelled)
    ));
    assert!(matches!(
        collector
            .object("persistentvolumes", &ApiScope::Cluster, path)
            .await,
        Err(Failure::Cancelled)
    ));
    assert_eq!(transport.paths.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn repeated_continuation_retains_all_valid_optional_pages_without_more_requests() {
    let transport = fixture_transport();
    let input: Value =
        serde_json::from_str(include_str!("../../examples/contract/healthy/input.json")).unwrap();
    let first = input["kubernetes"]["pods"][0].clone();
    let mut second = first.clone();
    second["metadata"]["uid"] = "second-pod".into();
    second["metadata"]["name"] = "second".into();
    let path = "/api/v1/namespaces/demo/pods";
    transport.replies.lock().unwrap().extend([
        (format!("{path}?limit=500"),vec![response(serde_json::json!({"metadata":{"resourceVersion":"101","continue":"again"},"items":[first]}))]),
        (format!("{path}?limit=500&continue=again"),vec![response(serde_json::json!({"metadata":{"resourceVersion":"101","continue":"again"},"items":[second]}))]),
    ]);
    let mut collector = Collector {
        request: RequestContext {
            transport: transport.clone(),
            clock: clock(),
            cancel: CancellationToken::new(),
            deadline: Instant::now() + Duration::from_secs(60),
            request_timeout: Duration::from_secs(10),
            totals: Arc::new(AtomicAttempts::default()),
        },
        overall: Instant::now() + Duration::from_secs(60),
        enrichment: true,
        allow_bulk: false,
        cost: Cost::initial(),
        cache: BTreeMap::new(),
        fatal_failure: None,
        sources: vec![],
        diagnostics: vec![],
    };
    let (objects, result) = collector
        .list("pods", &ApiScope::Namespace("demo".into()), path, None)
        .await;
    assert!(matches!(result, Err(Failure::Invalid)));
    assert_eq!(objects.len(), 2);
    assert_eq!(transport.paths.lock().unwrap().len(), 2);
    assert_eq!(collector.sources[0].status, "partial");
    assert_eq!(
        collector.sources[0].resource_version.as_deref(),
        Some("101")
    );
}

#[tokio::test(start_paused = true)]
async fn pv_inventory_plans_claims_and_cluster_consumers_with_typed_identity() {
    let transport = fixture_transport();
    let input: Value =
        serde_json::from_str(include_str!("../../examples/contract/healthy/input.json")).unwrap();
    transport.replies.lock().unwrap().extend([
        ("/api/v1/persistentvolumes?limit=500".into(),vec![response(serde_json::json!({"metadata":{"resourceVersion":"100"},"items":input["kubernetes"]["pvs"]}))]),
        ("/api/v1/namespaces/demo/persistentvolumeclaims/data".into(),vec![response(input["kubernetes"]["pvcs"][0].clone())]),
        ("/api/v1/pods?limit=500".into(),vec![response(serde_json::json!({"metadata":{"resourceVersion":"101"},"items":input["kubernetes"]["pods"]}))]),
    ]);
    let cli = Cli::try_parse_from(["cmd", "-A", "--kind", "pv"]).unwrap();
    let query = cli.query("ignored").unwrap();
    let result = collect(
        transport.clone(),
        clock(),
        CancellationToken::new(),
        &cli,
        &query,
        "example".into(),
    )
    .await
    .unwrap();
    assert_eq!(result.graph.records.len(), 1);
    assert_eq!(result.graph.records[0].kind, Kind::Pv);
    assert_eq!(result.graph.records[0].active_count, Some(1));
    assert_eq!(result.graph.relationships.len(), 1);
    assert_eq!(result.collection.cost.enrichment_attempts, 3);
    let paths = transport.paths.lock().unwrap();
    assert_eq!(paths[0], "/api/v1/persistentvolumes?limit=500");
    assert!(
        paths
            .iter()
            .any(|path| path == "/api/v1/namespaces/demo/persistentvolumeclaims/data")
    );
    assert!(paths.iter().any(|path| path == "/api/v1/pods?limit=500"));
    assert!(
        !paths
            .iter()
            .any(|path| path == "/api/v1/namespaces/demo/pods?limit=500")
    );
}

#[tokio::test(start_paused = true)]
async fn bulk_enrichment_retains_valid_items_falls_back_on_denial_and_stops_on_auth_failure() {
    let input: Value =
        serde_json::from_str(include_str!("../../examples/contract/healthy/input.json")).unwrap();
    let volumes: Vec<_> = (0..32)
        .map(|index| {
            let mut value = input["kubernetes"]["pvs"][0].clone();
            value["metadata"]["name"] = format!("pv-{index:02}").into();
            value["metadata"]["uid"] = format!("volume-{index:02}").into();
            value
        })
        .collect();
    let names = Arc::new(
        (0..32)
            .map(|index| format!("pv-{index:02}"))
            .collect::<BTreeSet<_>>(),
    );
    for status in [200, 403, 401] {
        let transport = Arc::new(MockTransport::default());
        let body = if status == 200 {
            response(
                serde_json::json!({"metadata":{"resourceVersion":"1"},"items":[volumes[0],{"metadata":{"name":"invalid"}}]}),
            )
        } else {
            Response {
                status,
                body: vec![],
                retry_after: None,
            }
        };
        transport
            .replies
            .lock()
            .unwrap()
            .insert("/api/v1/persistentvolumes?limit=500".into(), vec![body]);
        for (index, volume) in volumes.iter().enumerate() {
            transport.replies.lock().unwrap().insert(
                format!("/api/v1/persistentvolumes/pv-{index:02}"),
                vec![response(volume.clone())],
            );
        }
        let mut collector = Collector {
            request: RequestContext {
                transport: transport.clone(),
                clock: clock(),
                cancel: CancellationToken::new(),
                deadline: Instant::now() + Duration::from_secs(60),
                request_timeout: Duration::from_secs(10),
                totals: Arc::new(AtomicAttempts::default()),
            },
            overall: Instant::now() + Duration::from_secs(60),
            enrichment: true,
            allow_bulk: true,
            cost: Cost::initial(),
            cache: BTreeMap::new(),
            fatal_failure: None,
            sources: vec![],
            diagnostics: vec![],
        };
        let objects = collector
            .get_objects(
                "persistentvolumes",
                "/api/v1/persistentvolumes",
                names.clone(),
            )
            .await;
        if status == 401 {
            assert!(matches!(objects, Err(Failure::Http(401))));
            assert_eq!(transport.paths.lock().unwrap().len(), 1);
            assert!(matches!(collector.fatal(), Some(Failure::Http(401))));
        } else {
            assert_eq!(objects.unwrap().len(), 32);
            let expected = if status == 200 { 32 } else { 33 };
            assert_eq!(collector.cost.enrichment_attempts, expected);
            assert_eq!(transport.paths.lock().unwrap().len(), expected);
            assert_eq!(
                transport
                    .paths
                    .lock()
                    .unwrap()
                    .contains(&"/api/v1/persistentvolumes/pv-00".into()),
                status == 403
            );
        }
    }
}

struct ProjectionClock {
    async_thread: std::thread::ThreadId,
    cancel: CancellationToken,
    checks: AtomicUsize,
    panic: bool,
}
impl Clock for ProjectionClock {
    fn monotonic(&self) -> Instant {
        if std::thread::current().id() != self.async_thread {
            let check = self.checks.fetch_add(1, Ordering::SeqCst);
            assert!(!self.panic, "controlled typed projection worker panic");
            if check == 1 {
                self.cancel.cancel();
            }
        }
        Instant::now()
    }
    fn utc(&self) -> jiff::Timestamp {
        "2026-11-06T20:00:00Z".parse().unwrap()
    }
}

#[tokio::test(flavor = "current_thread")]
async fn typed_projection_cancellation_and_worker_failure_are_joined_and_never_succeed() {
    let input: Value =
        serde_json::from_str(include_str!("../../examples/contract/healthy/input.json")).unwrap();
    for panic in [false, true] {
        let transport = fixture_transport();
        let cancel = CancellationToken::new();
        let clock = Arc::new(ProjectionClock {
            async_thread: std::thread::current().id(),
            cancel: cancel.clone(),
            checks: 0.into(),
            panic,
        });
        let path = "/api/v1/namespaces/demo/persistentvolumeclaims/data";
        let mut document = input["kubernetes"]["pvcs"][0].clone();
        document["metadata"]["labels"] = Value::Object(
            (0..1000)
                .map(|index| (format!("label-{index}"), Value::from("value")))
                .collect(),
        );
        let mut collector = Collector {
            request: RequestContext {
                transport: transport.clone(),
                clock: clock.clone(),
                cancel: cancel.clone(),
                deadline: Instant::now() + Duration::from_secs(60),
                request_timeout: Duration::from_secs(10),
                totals: Arc::new(AtomicAttempts::default()),
            },
            overall: Instant::now() + Duration::from_secs(60),
            enrichment: true,
            allow_bulk: false,
            cost: Cost::initial(),
            cache: BTreeMap::from([(path.into(), Ok(Arc::new(document)))]),
            fatal_failure: None,
            sources: vec![],
            diagnostics: vec![],
        };
        let result = collector
            .object(
                "persistentvolumeclaims",
                &ApiScope::Namespace("demo".into()),
                path,
            )
            .await;
        if panic {
            assert!(matches!(result, Err(Failure::Worker)));
            assert!(matches!(collector.fatal(), Some(Failure::Worker)));
            assert!(matches!(
                collector.fetch("/never-dispatch").await,
                Err(Failure::Worker)
            ));
        } else {
            assert!(matches!(result, Err(Failure::Cancelled)));
            assert_eq!(clock.checks.load(Ordering::SeqCst), 2);
        }
        let checks = clock.checks.load(Ordering::SeqCst);
        tokio::task::yield_now().await;
        assert_eq!(clock.checks.load(Ordering::SeqCst), checks);
        assert!(transport.paths.lock().unwrap().is_empty());
    }
}

#[tokio::test(flavor = "current_thread")]
async fn dependency_planning_cancellation_joins_before_dispatch() {
    let input: Value =
        serde_json::from_str(include_str!("../../examples/contract/healthy/input.json")).unwrap();
    let mut inventory: Inventory = serde_json::from_value(input["kubernetes"].clone()).unwrap();
    let template = inventory.pvcs[0].clone();
    let relevant =
        BTreeSet::from([
            RecordId::new(Kind::Pvc, template.metadata.uid.as_deref().unwrap()).unwrap(),
        ]);
    for index in 0..1000 {
        let mut other = template.clone();
        other.metadata.uid = Some(format!("other-{index}"));
        inventory.pvcs.push(other);
    }
    let transport = fixture_transport();
    let cancel = CancellationToken::new();
    let clock = Arc::new(ProjectionClock {
        async_thread: std::thread::current().id(),
        cancel: cancel.clone(),
        checks: 0.into(),
        panic: false,
    });
    let request = RequestContext {
        transport: transport.clone(),
        clock: clock.clone(),
        cancel: cancel.clone(),
        deadline: Instant::now() + Duration::from_secs(60),
        request_timeout: Duration::from_secs(10),
        totals: Arc::new(AtomicAttempts::default()),
    };
    let result = inventory_job(&mut inventory, &request, move |inventory, control| {
        volume_plan(inventory, Kind::Pvc, &relevant, control).map_err(CheckedError::Interrupted)
    })
    .await;
    assert!(matches!(result, Err(Failure::Cancelled)));
    assert_eq!(clock.checks.load(Ordering::SeqCst), 2);
    tokio::task::yield_now().await;
    assert_eq!(clock.checks.load(Ordering::SeqCst), 2);
    assert!(transport.paths.lock().unwrap().is_empty());
}

#[tokio::test]
async fn decode_joins_and_preserves_timeout_deadline_and_cancellation_reasons() {
    let mut context = RequestContext {
        transport: fixture_transport(),
        clock: clock(),
        cancel: CancellationToken::new(),
        deadline: Instant::now() + Duration::from_secs(60),
        request_timeout: Duration::from_secs(10),
        totals: Arc::new(AtomicAttempts::default()),
    };
    let expired = Instant::now() - Duration::from_secs(1);
    assert!(matches!(
        decode_document(
            &context,
            DocumentBody {
                bytes: b"{broken".to_vec(),
                deadline: expired
            }
        )
        .await,
        Err(Failure::Timeout)
    ));
    context.deadline = expired;
    assert!(matches!(
        decode_document(
            &context,
            DocumentBody {
                bytes: b"{broken".to_vec(),
                deadline: expired
            }
        )
        .await,
        Err(Failure::Deadline)
    ));
    context.cancel.cancel();
    assert!(matches!(
        decode_document(
            &context,
            DocumentBody {
                bytes: b"{}".to_vec(),
                deadline: expired
            }
        )
        .await,
        Err(Failure::Cancelled)
    ));
}

#[tokio::test]
async fn decoder_worker_panics_are_observed_as_worker_failures() {
    struct PanickingClock;
    impl Clock for PanickingClock {
        fn monotonic(&self) -> Instant {
            panic!("controlled decoder worker panic")
        }
        fn utc(&self) -> jiff::Timestamp {
            "2026-11-07T00:00:00Z".parse().unwrap()
        }
    }
    let context = RequestContext {
        transport: fixture_transport(),
        clock: Arc::new(PanickingClock),
        cancel: CancellationToken::new(),
        deadline: Instant::now() + Duration::from_secs(60),
        request_timeout: Duration::from_secs(10),
        totals: Arc::new(AtomicAttempts::default()),
    };
    assert!(matches!(
        decode_document(
            &context,
            DocumentBody {
                bytes: b"{}".to_vec(),
                deadline: context.deadline
            }
        )
        .await,
        Err(Failure::Worker)
    ));
}

#[tokio::test(flavor = "current_thread")]
async fn decoding_cancellation_joins_the_parser_and_a_concurrent_http_worker() {
    struct TwoNodes {
        inner: Arc<MockTransport>,
        active: AtomicUsize,
        started: AtomicUsize,
    }
    impl Transport for TwoNodes {
        fn get(&self, path: String) -> ResponseFuture<'_> {
            Box::pin(async move {
                if path == "/api/v1/nodes/worker-b/proxy/stats/summary" {
                    self.inner.paths.lock().unwrap().push(path);
                    self.started.fetch_add(1, Ordering::SeqCst);
                    self.active.fetch_add(1, Ordering::SeqCst);
                    let _active = Active(&self.active);
                    tokio::time::sleep(Duration::from_secs(100)).await;
                    Err(Failure::Transport)
                } else {
                    self.inner.get(path).await
                }
            })
        }
    }
    struct CancelParser {
        transport: Arc<TwoNodes>,
        cancel: CancellationToken,
        async_thread: std::thread::ThreadId,
        checks: AtomicUsize,
    }
    impl Clock for CancelParser {
        fn monotonic(&self) -> Instant {
            if std::thread::current().id() != self.async_thread
                && self.transport.active.load(Ordering::SeqCst) == 1
                && self.checks.fetch_add(1, Ordering::SeqCst) == 1
            {
                self.cancel.cancel();
            }
            Instant::now()
        }
        fn utc(&self) -> jiff::Timestamp {
            "2026-11-06T20:00:00Z".parse().unwrap()
        }
    }
    let input: Value =
        serde_json::from_str(include_str!("../../examples/contract/healthy/input.json")).unwrap();
    let inner = fixture_transport();
    let mut pods = input["kubernetes"]["pods"].as_array().unwrap().clone();
    let mut other = pods[0].clone();
    other["metadata"]["uid"] = "other-pod".into();
    other["metadata"]["name"] = "other".into();
    other["spec"]["nodeName"] = "worker-b".into();
    pods.push(other);
    let mut summary = input["summary_responses"]["worker-a"].clone();
    summary["unknown"] = "x".repeat(1024 * 1024).into();
    inner.replies.lock().unwrap().extend([
        (
            "/api/v1/namespaces/demo/pods?limit=500".into(),
            vec![response(
                serde_json::json!({"metadata":{"resourceVersion":"101"},"items":pods}),
            )],
        ),
        (
            "/api/v1/nodes/worker-a/proxy/stats/summary".into(),
            vec![response(summary)],
        ),
    ]);
    let transport = Arc::new(TwoNodes {
        inner,
        active: 0.into(),
        started: 0.into(),
    });
    let cancel = CancellationToken::new();
    let clock = Arc::new(CancelParser {
        transport: transport.clone(),
        cancel: cancel.clone(),
        async_thread: std::thread::current().id(),
        checks: 0.into(),
    });
    let cli = Cli::try_parse_from(["cmd", "-u", "--timeout", "5s"]).unwrap();
    let query = cli.query("demo").unwrap();
    let result = collect(
        transport.clone(),
        clock.clone(),
        cancel.clone(),
        &cli,
        &query,
        "example".into(),
    )
    .await;
    assert!(
        cancel.is_cancelled(),
        "the parser never observed a concurrently active worker"
    );
    assert!(matches!(result, Err(Failure::Cancelled)));
    assert_eq!(transport.started.load(Ordering::SeqCst), 1);
    assert_eq!(transport.active.load(Ordering::SeqCst), 0);
    assert_eq!(clock.checks.load(Ordering::SeqCst), 2);
    let requests = transport.inner.paths.lock().unwrap().len();
    tokio::task::yield_now().await;
    assert_eq!(transport.inner.paths.lock().unwrap().len(), requests);
    assert_eq!(
        clock.checks.load(Ordering::SeqCst),
        2,
        "parser survived its join"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn cancellation_during_statistics_cpu_work_joins_before_return() {
    struct CancelStatisticsCpu {
        async_thread: std::thread::ThreadId,
        cancel: CancellationToken,
        transport: Arc<MockTransport>,
        cpu_checks: AtomicUsize,
    }
    impl Clock for CancelStatisticsCpu {
        fn monotonic(&self) -> Instant {
            if std::thread::current().id() != self.async_thread
                && self
                    .transport
                    .paths
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|path| path.ends_with("/stats/summary"))
                && self.cpu_checks.fetch_add(1, Ordering::Relaxed) == 1
            {
                self.cancel.cancel();
            }
            Instant::now()
        }
        fn utc(&self) -> jiff::Timestamp {
            "2026-11-06T20:00:00Z".parse().unwrap()
        }
    }
    let transport = fixture_transport();
    let bundle: Value =
        serde_json::from_str(include_str!("../../examples/contract/healthy/input.json")).unwrap();
    let mut summary = bundle["summary_responses"]["worker-a"].clone();
    summary["pods"][0]["volume"] =
        Value::Array(vec![summary["pods"][0]["volume"][0].clone(); 1000]);
    transport.replies.lock().unwrap().insert(
        "/api/v1/nodes/worker-a/proxy/stats/summary".into(),
        vec![response(summary)],
    );
    let cli = Cli::try_parse_from(["cmd", "-u"]).unwrap();
    let query = cli.query("demo").unwrap();
    let cancel = CancellationToken::new();
    let clock = Arc::new(CancelStatisticsCpu {
        async_thread: std::thread::current().id(),
        cancel: cancel.clone(),
        transport: transport.clone(),
        cpu_checks: AtomicUsize::new(0),
    });
    let result = collect(
        transport.clone(),
        clock.clone(),
        cancel,
        &cli,
        &query,
        "example".into(),
    )
    .await;
    assert!(matches!(result, Err(Failure::Cancelled)));
    assert!(clock.cpu_checks.load(Ordering::Relaxed) >= 2);
    assert_eq!(transport.paths.lock().unwrap().len(), 5);
    let joined = clock.cpu_checks.load(Ordering::Relaxed);
    tokio::task::yield_now().await;
    assert_eq!(clock.cpu_checks.load(Ordering::Relaxed), joined);
    assert_eq!(transport.paths.lock().unwrap().len(), 5);
}

#[tokio::test(flavor = "current_thread")]
async fn cancellation_during_inventory_cpu_work_is_joined_and_stops_optional_dispatch() {
    struct CancelDuringCpu {
        async_thread: std::thread::ThreadId,
        cancel: CancellationToken,
        cpu_checks: AtomicUsize,
        transport: Arc<MockTransport>,
    }
    impl Clock for CancelDuringCpu {
        fn monotonic(&self) -> Instant {
            if std::thread::current().id() != self.async_thread
                && !self.transport.paths.lock().unwrap().is_empty()
                && self.cpu_checks.fetch_add(1, Ordering::Relaxed) == 1
            {
                self.cancel.cancel();
            }
            Instant::now()
        }
        fn utc(&self) -> jiff::Timestamp {
            "2026-11-06T20:00:00Z".parse().unwrap()
        }
    }
    let transport = fixture_transport();
    let input: Value =
        serde_json::from_str(include_str!("../../examples/contract/healthy/input.json")).unwrap();
    let claims: Vec<_> = (0..1000)
        .map(|index| {
            let mut claim = input["kubernetes"]["pvcs"][0].clone();
            claim["metadata"]["uid"] = format!("claim-{index}").into();
            claim["metadata"]["name"] = format!("claim-{index}").into();
            claim
        })
        .collect();
    transport.replies.lock().unwrap().insert(
        "/api/v1/namespaces/demo/persistentvolumeclaims?limit=500".into(),
        vec![response(
            serde_json::json!({"metadata":{"resourceVersion":"100"},"items":claims}),
        )],
    );
    let cli = Cli::try_parse_from(["cmd", "-u"]).unwrap();
    let query = cli.query("demo").unwrap();
    let cancel = CancellationToken::new();
    let clock = Arc::new(CancelDuringCpu {
        async_thread: std::thread::current().id(),
        cancel: cancel.clone(),
        cpu_checks: AtomicUsize::new(0),
        transport: transport.clone(),
    });
    let result = collect(
        transport.clone(),
        clock.clone(),
        cancel,
        &cli,
        &query,
        "example".into(),
    )
    .await;
    assert!(matches!(result, Err(Failure::Cancelled)));
    assert!(clock.cpu_checks.load(Ordering::Relaxed) >= 2);
    assert_eq!(
        transport.paths.lock().unwrap().as_slice(),
        ["/api/v1/namespaces/demo/persistentvolumeclaims?limit=500"]
    );
    let joined = clock.cpu_checks.load(Ordering::Relaxed);
    tokio::task::yield_now().await;
    assert_eq!(clock.cpu_checks.load(Ordering::Relaxed), joined);
}

#[tokio::test(start_paused = true)]
async fn inventory_then_usage_paths_counts_and_provenance() {
    let transport = fixture_transport();
    let cli = Cli::try_parse_from(["cmd", "-n", "demo", "-u"]).unwrap();
    let query = cli.query("demo").unwrap();
    let result = collect(
        transport.clone(),
        clock(),
        CancellationToken::new(),
        &cli,
        &query,
        "example".into(),
    )
    .await
    .unwrap();
    assert_eq!(result.graph.records.len(), 1);
    assert_eq!(result.graph.records[0].filesystem.status(), "ok");
    assert_eq!(result.collection.cost.base_attempts, 1);
    assert_eq!(result.collection.cost.enrichment_attempts, 3);
    assert_eq!(result.collection.cost.node_targets, 1);
    assert_eq!(result.collection.cost.node_requests, 1);
    assert_eq!(transport.paths.lock().unwrap().len(), 5);
}

#[tokio::test(start_paused = true)]
async fn denied_optional_pv_keeps_inventory_and_live_bytes() {
    let transport = fixture_transport();
    transport.replies.lock().unwrap().insert(
        "/api/v1/persistentvolumes/pv-data".into(),
        vec![Response {
            status: 403,
            body: vec![],
            retry_after: None,
        }],
    );
    let cli = Cli::try_parse_from(["cmd", "-u"]).unwrap();
    let query = cli.query("demo").unwrap();
    let result = collect(
        transport.clone(),
        clock(),
        CancellationToken::new(),
        &cli,
        &query,
        "example".into(),
    )
    .await
    .unwrap();
    assert_eq!(result.graph.records[0].provisioned, Field::Missing);
    assert_eq!(result.graph.records[0].filesystem.status(), "ok");
    assert!(
        result
            .graph
            .diagnostics
            .iter()
            .any(|d| d.reason == "forbidden")
    );
    assert_eq!(
        transport
            .paths
            .lock()
            .unwrap()
            .iter()
            .filter(|p| p.ends_with("pv-data"))
            .count(),
        1
    );
}

#[tokio::test(start_paused = true)]
async fn failed_second_base_page_is_fatal_and_no_optional_requests_start() {
    let transport = fixture_transport();
    transport.replies.lock().unwrap().insert(
        "/api/v1/namespaces/demo/persistentvolumeclaims?limit=500".into(),
        vec![response(
            serde_json::json!({"metadata":{"continue":"page two"},"items":[]}),
        )],
    );
    transport.replies.lock().unwrap().insert(
        "/api/v1/namespaces/demo/persistentvolumeclaims?limit=500&continue=page%20two".into(),
        vec![Response {
            status: 403,
            body: vec![],
            retry_after: None,
        }],
    );
    let cli = Cli::try_parse_from(["cmd"]).unwrap();
    let query = cli.query("demo").unwrap();
    assert!(matches!(
        collect(
            transport.clone(),
            clock(),
            CancellationToken::new(),
            &cli,
            &query,
            "example".into()
        )
        .await,
        Err(Failure::Http(403))
    ));
    assert_eq!(transport.paths.lock().unwrap().len(), 2);
}

#[tokio::test(start_paused = true)]
async fn transient_node_response_retries_but_forbidden_does_not() {
    let transport = fixture_transport();
    let path = "/api/v1/nodes/worker-a/proxy/stats/summary";
    let success = transport
        .replies
        .lock()
        .unwrap()
        .get_mut(path)
        .unwrap()
        .remove(0);
    transport.replies.lock().unwrap().insert(
        path.into(),
        vec![
            Response {
                status: 429,
                body: vec![],
                retry_after: Some(Duration::from_secs(1)),
            },
            success,
        ],
    );
    let cli = Cli::try_parse_from(["cmd", "-u"]).unwrap();
    let query = cli.query("demo").unwrap();
    let result = collect(
        transport.clone(),
        clock(),
        CancellationToken::new(),
        &cli,
        &query,
        "example".into(),
    )
    .await
    .unwrap();
    assert_eq!(result.collection.cost.node_requests, 2);
    assert_eq!(result.graph.records[0].filesystem.status(), "ok");
}

#[tokio::test(start_paused = true)]
async fn optional_budget_and_failed_get_cache_are_enforced_before_dispatch() {
    let transport = fixture_transport();
    let request = RequestContext {
        transport: transport.clone(),
        clock: clock(),
        cancel: CancellationToken::new(),
        deadline: Instant::now() + Duration::from_secs(30),
        request_timeout: Duration::from_secs(10),
        totals: Arc::new(AtomicAttempts::default()),
    };
    let mut collector = Collector {
        request,
        overall: Instant::now() + Duration::from_secs(60),
        enrichment: true,
        allow_bulk: false,
        cost: Cost::initial(),
        cache: BTreeMap::new(),
        fatal_failure: None,
        sources: vec![],
        diagnostics: vec![],
    };
    let path = "/api/v1/persistentvolumes/pv-data";
    transport.replies.lock().unwrap().insert(
        path.into(),
        vec![Response {
            status: 403,
            body: vec![],
            retry_after: None,
        }],
    );
    assert!(matches!(
        collector.fetch(path).await,
        Err(Failure::Http(403))
    ));
    assert!(matches!(
        collector.fetch(path).await,
        Err(Failure::Http(403))
    ));
    assert_eq!(collector.cost.enrichment_attempts, 1);
    assert_eq!(transport.paths.lock().unwrap().len(), 1);
    collector.cost.enrichment_attempts = 256;
    assert!(matches!(
        collector.fetch("/other").await,
        Err(Failure::Budget)
    ));
    assert_eq!(transport.paths.lock().unwrap().len(), 1);
}

#[tokio::test(start_paused = true)]
async fn successful_unknown_http_status_field_does_not_change_transport_status() {
    let transport = fixture_transport();
    let path = "/api/v1/namespaces/demo/pods?limit=500";
    transport.replies.lock().unwrap().insert(
        path.into(),
        vec![response(
            serde_json::json!({"http_status":403,"metadata":{},"items":[]}),
        )],
    );
    let cli = Cli::try_parse_from(["cmd"]).unwrap();
    let query = cli.query("demo").unwrap();
    let result = collect(
        transport,
        clock(),
        CancellationToken::new(),
        &cli,
        &query,
        "example".into(),
    )
    .await
    .unwrap();
    assert_eq!(result.graph.records[0].active_count, Some(0));
}

struct StallingTransport {
    inner: Arc<MockTransport>,
    active: std::sync::atomic::AtomicUsize,
    started: std::sync::atomic::AtomicUsize,
}
struct Active<'a>(&'a std::sync::atomic::AtomicUsize);
impl Drop for Active<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    }
}
impl Transport for StallingTransport {
    fn get(&self, path: String) -> ResponseFuture<'_> {
        Box::pin(async move {
            if path.contains("/stats/summary") {
                self.started
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                self.active
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let _active = Active(&self.active);
                tokio::time::sleep(Duration::from_secs(100)).await;
                Err(Failure::Transport)
            } else {
                self.inner.get(path).await
            }
        })
    }
}

#[tokio::test(start_paused = true)]
async fn interruption_joins_workers_and_no_late_request_survives_return() {
    use std::sync::atomic::Ordering::SeqCst;
    let transport = Arc::new(StallingTransport {
        inner: fixture_transport(),
        active: 0.into(),
        started: 0.into(),
    });
    let cancel = CancellationToken::new();
    let task_transport = transport.clone();
    let task_cancel = cancel.clone();
    let task = tokio::spawn(async move {
        let cli = Cli::try_parse_from(["cmd", "-u"]).unwrap();
        let query = cli.query("demo").unwrap();
        collect(
            task_transport,
            clock(),
            task_cancel,
            &cli,
            &query,
            "example".into(),
        )
        .await
    });
    while transport.active.load(SeqCst) == 0 {
        tokio::task::yield_now().await;
    }
    cancel.cancel();
    assert!(matches!(task.await.unwrap(), Err(Failure::Cancelled)));
    assert_eq!(transport.active.load(SeqCst), 0);
    let count = transport.started.load(SeqCst);
    tokio::time::advance(Duration::from_secs(200)).await;
    assert_eq!(transport.started.load(SeqCst), count);
}

#[tokio::test(start_paused = true)]
async fn overall_deadline_keeps_completed_inventory_and_records_missing_node() {
    let transport = Arc::new(StallingTransport {
        inner: fixture_transport(),
        active: 0.into(),
        started: 0.into(),
    });
    let cli = Cli::try_parse_from(["cmd", "-u", "--timeout", "1s"]).unwrap();
    let query = cli.query("demo").unwrap();
    let result = collect(
        transport.clone(),
        clock(),
        CancellationToken::new(),
        &cli,
        &query,
        "example".into(),
    )
    .await
    .unwrap();
    assert_eq!(result.graph.records.len(), 1);
    assert_eq!(result.graph.records[0].filesystem.status(), "timeout");
    assert_eq!(result.collection.cost.node_requests, 1);
    assert_eq!(
        transport.active.load(std::sync::atomic::Ordering::SeqCst),
        0
    );
}

#[tokio::test(start_paused = true)]
async fn authentication_failures_are_fatal_in_optional_enrichment_and_metrics() {
    for path in [
        "/api/v1/persistentvolumes/pv-data",
        "/api/v1/nodes/worker-a/proxy/stats/summary",
    ] {
        let transport = fixture_transport();
        transport.replies.lock().unwrap().insert(
            path.into(),
            vec![Response {
                status: 401,
                body: vec![],
                retry_after: None,
            }],
        );
        let cli = Cli::try_parse_from(["cmd", "-u"]).unwrap();
        let query = cli.query("demo").unwrap();
        assert!(matches!(
            collect(
                transport.clone(),
                clock(),
                CancellationToken::new(),
                &cli,
                &query,
                "example".into()
            )
            .await,
            Err(Failure::Http(401))
        ));
        assert_eq!(
            transport
                .paths
                .lock()
                .unwrap()
                .iter()
                .filter(|p| p.as_str() == path)
                .count(),
            1
        );
    }
}

#[tokio::test(start_paused = true)]
async fn known_nonmatches_do_not_spend_optional_budget() {
    let transport = fixture_transport();
    let cli = Cli::try_parse_from(["cmd", "--pvc", "another"]).unwrap();
    let query = cli.query("demo").unwrap();
    let result = collect(
        transport.clone(),
        clock(),
        CancellationToken::new(),
        &cli,
        &query,
        "example".into(),
    )
    .await
    .unwrap();
    assert!(result.graph.records.is_empty());
    assert!(result.selection.complete);
    assert_eq!(result.selection.excluded_count, 1);
    assert_eq!(transport.paths.lock().unwrap().len(), 1);
}

#[tokio::test(start_paused = true)]
async fn pv_driver_known_nonmatch_needs_no_claim_class_pod_or_metric_requests() {
    let transport = fixture_transport();
    let input: Value =
        serde_json::from_str(include_str!("../../examples/contract/healthy/input.json")).unwrap();
    transport.replies.lock().unwrap().insert(
        "/api/v1/persistentvolumes?limit=500".into(),
        vec![response(
            serde_json::json!({"metadata":{},"items":input["kubernetes"]["pvs"]}),
        )],
    );
    let cli = Cli::try_parse_from([
        "cmd",
        "-A",
        "--kind",
        "pv",
        "--driver",
        "different.driver",
        "-u",
    ])
    .unwrap();
    let query = cli.query("ignored").unwrap();
    let result = collect(
        transport.clone(),
        clock(),
        CancellationToken::new(),
        &cli,
        &query,
        "test".into(),
    )
    .await
    .unwrap();
    assert!(result.graph.records.is_empty());
    assert!(result.selection.complete);
    assert_eq!(result.selection.excluded_count, 1);
    assert_eq!(result.collection.cost.enrichment_attempts, 0);
    assert_eq!(result.collection.cost.node_requests, 0);
    assert_eq!(
        *transport.paths.lock().unwrap(),
        ["/api/v1/persistentvolumes?limit=500"]
    );
}

#[tokio::test(start_paused = true)]
async fn driver_membership_prunes_nonmatches_before_display_and_consumer_requests() {
    for matches_driver in [false, true] {
        for node in [None, Some("elsewhere")] {
            let transport = fixture_transport();
            let mut args = vec![
                "cmd",
                "--driver",
                if matches_driver {
                    "example.csi.test"
                } else {
                    "different.driver"
                },
            ];
            if let Some(node) = node {
                args.extend(["--node", node]);
            }
            let cli = Cli::try_parse_from(args).unwrap();
            let query = cli.query("demo").unwrap();
            let result = collect(
                transport.clone(),
                clock(),
                CancellationToken::new(),
                &cli,
                &query,
                "example".into(),
            )
            .await
            .unwrap();
            assert_eq!(
                result.graph.records.len(),
                usize::from(matches_driver && node.is_none())
            );
            assert!(result.selection.complete);
            let paths = transport.paths.lock().unwrap();
            if matches_driver && node.is_none() {
                let pods = paths
                    .iter()
                    .position(|path| path.contains("/pods?"))
                    .unwrap();
                let class = paths
                    .iter()
                    .position(|path| path.contains("/storageclasses/"))
                    .unwrap();
                assert!(
                    pods < class,
                    "known CSI driver needs no class membership lookup"
                );
                assert_eq!(
                    paths
                        .iter()
                        .filter(|path| path.ends_with("pv-data"))
                        .count(),
                    1
                );
                assert_eq!(
                    paths
                        .iter()
                        .filter(|path| path.contains("/storageclasses/"))
                        .count(),
                    1
                );
            } else if matches_driver {
                assert_eq!(paths.len(), 3);
                assert!(paths.last().unwrap().contains("/pods?"));
                assert_eq!(result.collection.cost.enrichment_attempts, 2);
                assert_eq!(result.selection.excluded_count, 1);
            } else {
                assert_eq!(
                    *paths,
                    [
                        "/api/v1/namespaces/demo/persistentvolumeclaims?limit=500",
                        "/api/v1/persistentvolumes/pv-data"
                    ]
                );
                assert_eq!(result.collection.cost.enrichment_attempts, 1);
            }
        }
    }
}

#[tokio::test(start_paused = true)]
async fn pv_class_membership_precedes_claims_reuses_classes_and_rechecks_binding() {
    let input: Value =
        serde_json::from_str(include_str!("../../examples/contract/healthy/input.json")).unwrap();
    for condition in ["match", "denied", "conflict"] {
        let transport = fixture_transport();
        let mut first = input["kubernetes"]["pvs"][0].clone();
        first["spec"].as_object_mut().unwrap().remove("csi");
        let mut second = first.clone();
        second["metadata"]["name"] = "pv-other".into();
        second["metadata"]["uid"] = "pv-other".into();
        second["spec"]["storageClassName"] = "slow".into();
        second["spec"]["claimRef"]["name"] = "other".into();
        second["spec"]["claimRef"]["uid"] = "other".into();
        let mut slow = input["kubernetes"]["storage_classes"][0].clone();
        slow["metadata"]["name"] = "slow".into();
        slow["provisioner"] = "different.driver".into();
        let mut claim = input["kubernetes"]["pvcs"][0].clone();
        if condition == "conflict" {
            claim["metadata"]["uid"] = "recreated".into();
        }
        transport.replies.lock().unwrap().extend([
            (
                "/api/v1/persistentvolumes?limit=500".into(),
                vec![response(
                    serde_json::json!({"metadata":{},"items":[first,second]}),
                )],
            ),
            (
                "/apis/storage.k8s.io/v1/storageclasses/slow".into(),
                vec![response(slow)],
            ),
            (
                "/api/v1/namespaces/demo/persistentvolumeclaims/data".into(),
                vec![response(claim)],
            ),
            (
                "/api/v1/pods?limit=500".into(),
                vec![response(
                    serde_json::json!({"metadata":{},"items":input["kubernetes"]["pods"]}),
                )],
            ),
        ]);
        if condition == "denied" {
            transport.replies.lock().unwrap().insert(
                "/apis/storage.k8s.io/v1/storageclasses/fast".into(),
                vec![Response {
                    status: 403,
                    body: vec![],
                    retry_after: None,
                }],
            );
        }
        let cli =
            Cli::try_parse_from(["cmd", "-A", "--kind", "pv", "--driver", "example.csi.test"])
                .unwrap();
        let query = cli.query("ignored").unwrap();
        let result = collect(
            transport.clone(),
            clock(),
            CancellationToken::new(),
            &cli,
            &query,
            "example".into(),
        )
        .await
        .unwrap();
        assert_eq!(result.selection.base_count, 2);
        assert_eq!(
            result.selection.excluded_count, 1,
            "the class that proves a nonmatch must survive display enrichment"
        );
        assert_eq!(
            result.selection.unknown_count,
            usize::from(condition != "match")
        );
        assert_eq!(
            result.graph.records.len(),
            usize::from(condition == "match")
        );
        let paths = transport.paths.lock().unwrap();
        assert_eq!(
            &paths[..3],
            [
                "/api/v1/persistentvolumes?limit=500",
                "/apis/storage.k8s.io/v1/storageclasses/fast",
                "/apis/storage.k8s.io/v1/storageclasses/slow"
            ]
        );
        assert!(
            !paths
                .iter()
                .any(|path| path.ends_with("/persistentvolumeclaims/other"))
        );
        assert_eq!(
            paths
                .iter()
                .filter(|path| path.contains("/storageclasses/"))
                .count(),
            2
        );
        if condition == "match" {
            assert_eq!(paths.len(), 5);
            assert_eq!(
                paths[3],
                "/api/v1/namespaces/demo/persistentvolumeclaims/data"
            );
            assert_eq!(paths[4], "/api/v1/pods?limit=500");
        } else {
            assert_eq!(paths.len(), if condition == "denied" { 3 } else { 4 });
            assert!(!paths.iter().any(|path| path.contains("/pods?")));
        }
    }
}

#[tokio::test(start_paused = true)]
async fn driver_prerequisites_precede_consumers_and_known_driver_nonmatch_stops_pod_listing() {
    let transport = fixture_transport();
    let cli = Cli::try_parse_from(["cmd", "--driver", "example.csi.test"]).unwrap();
    let query = cli.query("demo").unwrap();
    let result = collect(
        transport.clone(),
        clock(),
        CancellationToken::new(),
        &cli,
        &query,
        "example".into(),
    )
    .await
    .unwrap();
    assert_eq!(result.graph.records.len(), 1);
    {
        let paths = transport.paths.lock().unwrap();
        let pv = paths.iter().position(|p| p.ends_with("pv-data")).unwrap();
        let pods = paths
            .iter()
            .position(|p| p.ends_with("pods?limit=500"))
            .unwrap();
        assert!(pv < pods);
    }
    let transport = fixture_transport();
    let cli = Cli::try_parse_from(["cmd", "--driver", "different.driver"]).unwrap();
    let query = cli.query("demo").unwrap();
    let result = collect(
        transport.clone(),
        clock(),
        CancellationToken::new(),
        &cli,
        &query,
        "example".into(),
    )
    .await
    .unwrap();
    assert!(result.graph.records.is_empty());
    assert!(
        !transport
            .paths
            .lock()
            .unwrap()
            .iter()
            .any(|p| p.contains("/pods"))
    );
}

#[tokio::test(start_paused = true)]
async fn shutdown_budget_aborts_uncooperative_async_worker() {
    use std::sync::atomic::Ordering::SeqCst;
    let active = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let worker_active = active.clone();
    let mut workers = JoinSet::new();
    workers.spawn(async move {
        worker_active.fetch_add(1, SeqCst);
        let _active = Active(&worker_active);
        tokio::time::sleep(Duration::from_secs(100)).await;
    });
    while active.load(SeqCst) == 0 {
        tokio::task::yield_now().await;
    }
    let started = Instant::now();
    shutdown(&mut workers, clock().as_ref()).await;
    assert!(
        started.elapsed() <= Duration::from_secs(1),
        "shutdown exceeded one second"
    );
    assert_eq!(active.load(SeqCst), 0);
    assert!(workers.is_empty());
}

#[tokio::test(start_paused = true)]
async fn malformed_optional_pod_preserves_inventory_and_valid_sibling_references() {
    let transport = fixture_transport();
    let path = "/api/v1/namespaces/demo/pods?limit=500";
    let mut page: Value =
        serde_json::from_slice(&transport.replies.lock().unwrap().get(path).unwrap()[0].body)
            .unwrap();
    page["items"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({"metadata":{"name":"bad","namespace":"demo"}}));
    transport
        .replies
        .lock()
        .unwrap()
        .insert(path.into(), vec![response(page)]);
    let cli = Cli::try_parse_from(["cmd", "-u"]).unwrap();
    let query = cli.query("demo").unwrap();
    let result = collect(
        transport,
        clock(),
        CancellationToken::new(),
        &cli,
        &query,
        "example".into(),
    )
    .await
    .unwrap();
    assert_eq!(result.graph.records.len(), 1);
    assert_eq!(result.graph.relationships.len(), 1);
    assert_eq!(result.graph.records[0].active_count, None);
    assert_eq!(result.graph.records[0].filesystem.status(), "ok");
    assert!(
        result
            .graph
            .diagnostics
            .iter()
            .any(|d| d.reason == "invalid_response")
    );
}

#[tokio::test(start_paused = true)]
async fn missing_required_uid_fails_before_enrichment() {
    let transport = fixture_transport();
    let path = "/api/v1/namespaces/demo/persistentvolumeclaims?limit=500";
    transport.replies.lock().unwrap().insert(path.into(),vec![response(serde_json::json!({"metadata":{},"items":[{"metadata":{"name":"bad","namespace":"demo"}}]}))]);
    let cli = Cli::try_parse_from(["cmd"]).unwrap();
    let query = cli.query("demo").unwrap();
    assert!(matches!(
        collect(
            transport.clone(),
            clock(),
            CancellationToken::new(),
            &cli,
            &query,
            "example".into()
        )
        .await,
        Err(Failure::Invalid)
    ));
    assert_eq!(transport.paths.lock().unwrap().len(), 1);
}

#[tokio::test(start_paused = true)]
async fn malformed_statistics_are_explicit_invalid_metrics_with_inventory_retained() {
    let transport = fixture_transport();
    transport.replies.lock().unwrap().insert(
        "/api/v1/nodes/worker-a/proxy/stats/summary".into(),
        vec![response(serde_json::json!({"pods":7}))],
    );
    let cli = Cli::try_parse_from(["cmd", "-u", "--fail-on", "warning"]).unwrap();
    let query = cli.query("demo").unwrap();
    let result = collect(
        transport,
        clock(),
        CancellationToken::new(),
        &cli,
        &query,
        "example".into(),
    )
    .await
    .unwrap();
    assert_eq!(result.graph.records.len(), 1);
    assert_eq!(result.graph.records[0].filesystem.status(), "invalid");
    let report = crate::pipeline::finish(result, &query).unwrap();
    assert_eq!(report.exit, crate::policy::Exit::Findings);
    assert!(
        report.document["findings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f["code"] == "invalid_metrics")
    );
}

#[tokio::test(start_paused = true)]
async fn malformed_pod_node_or_timestamp_never_proves_an_empty_node_selection() {
    for field in ["node", "timestamp"] {
        let transport = fixture_transport();
        let path = "/api/v1/namespaces/demo/pods?limit=500";
        let mut page: Value =
            serde_json::from_slice(&transport.replies.lock().unwrap().get(path).unwrap()[0].body)
                .unwrap();
        if field == "node" {
            page["items"][0]["spec"]["nodeName"] = serde_json::json!(7);
        } else {
            page["items"][0]["metadata"]["deletionTimestamp"] = "invalid-time".into();
        }
        transport
            .replies
            .lock()
            .unwrap()
            .insert(path.into(), vec![response(page)]);
        let cli = Cli::try_parse_from(["cmd", "--node", "worker-a", "--require-metrics"]).unwrap();
        let query = cli.query("demo").unwrap();
        let result = collect(
            transport,
            clock(),
            CancellationToken::new(),
            &cli,
            &query,
            "example".into(),
        )
        .await
        .unwrap();
        assert!(!result.selection.complete);
        assert_eq!(result.selection.unknown_count, 1);
        assert_eq!(
            crate::pipeline::finish(result, &query).unwrap().exit,
            crate::policy::Exit::Inconclusive
        );
    }
}

#[tokio::test(start_paused = true)]
async fn enormous_valid_json_counter_keeps_critical_sibling_measurements() {
    let transport = fixture_transport();
    let path = "/api/v1/nodes/worker-a/proxy/stats/summary";
    let body=br#"{"pods":[{"podRef":{"uid":"app","name":"app","namespace":"demo"},"volume":[{"name":"data","pvcRef":{"name":"data","namespace":"demo"},"time":"2026-11-06T19:59:50Z","capacityBytes":100,"usedBytes":95,"availableBytes":1e9999}]}]}"#;
    transport.replies.lock().unwrap().insert(
        path.into(),
        vec![Response {
            status: 200,
            body: body.to_vec(),
            retry_after: None,
        }],
    );
    let cli = Cli::try_parse_from(["cmd", "-u", "--fail-on", "critical"]).unwrap();
    let query = cli.query("demo").unwrap();
    let result = collect(
        transport,
        clock(),
        CancellationToken::new(),
        &cli,
        &query,
        "example".into(),
    )
    .await
    .unwrap();
    assert_eq!(result.graph.records[0].filesystem.status(), "partial");
    let candidate = result.graph.records[0].filesystem.selected().unwrap();
    assert_eq!(candidate.available, Field::Invalid);
    assert_eq!(candidate.used.value().unwrap().get(), 95);
    let report = crate::pipeline::finish(result, &query).unwrap();
    assert_eq!(report.exit, crate::policy::Exit::Findings);
    assert!(
        report.document["findings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f["code"] == "filesystem_usage_high" && f["severity"] == "critical")
    );
}
