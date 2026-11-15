//! Verify real controller requests, UID matching and shared-claim accounting.
use super::http_reports::{bundle, mount_bundle};
use super::*;

fn owner(kind: &str, name: &str, uid: &str) -> Value {
    json!({"apiVersion":if matches!(kind,"Job"|"CronJob") {"batch/v1"} else {"apps/v1"},
        "kind":kind,"name":name,"uid":uid,"controller":true})
}
async fn controller(
    server: &MockServer,
    reference: &Value,
    parent: Option<Value>,
    wrong_uid: bool,
) {
    let plural = match reference["kind"].as_str().unwrap() {
        "Deployment" => "deployments",
        "ReplicaSet" => "replicasets",
        "DaemonSet" => "daemonsets",
        "StatefulSet" => "statefulsets",
        "Job" => "jobs",
        "CronJob" => "cronjobs",
        _ => unreachable!(),
    };
    let object = json!({"apiVersion":reference["apiVersion"],"kind":reference["kind"],
        "metadata":{"namespace":"demo","name":reference["name"],"uid":if wrong_uid {json!("recreated-owner")} else {reference["uid"].clone()},
            "ownerReferences":parent.into_iter().collect::<Vec<_>>()}, "spec":{},"status":{}});
    Mock::given(method("GET"))
        .and(path(format!(
            "/apis/{}/namespaces/demo/{plural}/{}",
            reference["apiVersion"].as_str().unwrap(),
            reference["name"].as_str().unwrap()
        )))
        .respond_with(ResponseTemplate::new(200).set_body_json(object))
        .with_priority(1)
        .expect(1)
        .mount(server)
        .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn workload_types_follow_verified_owner_chains_without_multiplying_shared_storage() {
    for kind in ["Deployment", "DaemonSet", "StatefulSet", "Job", "CronJob"] {
        let server = MockServer::start().await;
        let config = Kubeconfig::new(&server);
        let mut input = bundle("shared-rwx");
        let root = owner(kind, "workload", "root-uid");
        let immediate = match kind {
            "Deployment" => owner("ReplicaSet", "replica", "replica-uid"),
            "CronJob" => owner("Job", "scheduled", "scheduled-uid"),
            _ => root.clone(),
        };
        for pod in input["kubernetes"]["pods"].as_array_mut().unwrap() {
            pod["metadata"]["ownerReferences"] = json!([immediate.clone()]);
        }
        controller(&server, &root, None, false).await;
        if immediate != root {
            controller(&server, &immediate, Some(root.clone()), false).await;
        }
        mount_bundle(&server, input).await;
        let filter = format!("{}/workload", kind.to_ascii_lowercase());
        let output = config.run(&["--workload", &filter, "-g", "workload", "-o", "json"]);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let report: Value = serde_json::from_slice(&output.stdout).unwrap();
        save_integration(&format!("workload-{}", kind.to_ascii_lowercase()), &report);
        assert_eq!(report["selection"]["matched_count"], 1);
        assert_eq!(
            report["groups"][0]["value"],
            format!("demo/{filter}@root-uid")
        );
        assert_eq!(
            report["summary"]["provisioned"]["known_sum_bytes"],
            "107374182400"
        );
        assert!(
            report["consumers"]
                .as_array()
                .unwrap()
                .iter()
                .all(|pod| pod["workload"]["uid"] == "root-uid")
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn recreated_workload_uid_cannot_turn_unknown_membership_into_a_strict_success() {
    let server = MockServer::start().await;
    let config = Kubeconfig::new(&server);
    let mut input = bundle("healthy");
    let root = owner("DaemonSet", "agent", "old-uid");
    input["kubernetes"]["pods"][0]["metadata"]["ownerReferences"] = json!([root.clone()]);
    controller(&server, &root, None, true).await;
    mount_bundle(&server, input).await;
    let output = config.run(&[
        "--workload",
        "daemonset/agent",
        "--require-metrics",
        "-o",
        "json",
    ]);
    assert_eq!(output.status.code(), Some(4));
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["selection"]["unknown_count"], 1);
    assert_eq!(report["selection"]["complete"], false);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn denied_parent_keeps_inventory_and_verified_partial_owner_membership() {
    for selected in [None, Some("deployment/app"), Some("replicaset/replica")] {
        let server = MockServer::start().await;
        let config = Kubeconfig::new(&server);
        let mut input = bundle("healthy");
        let immediate = owner("ReplicaSet", "replica", "replica-uid");
        let parent = owner("Deployment", "app", "app-uid");
        input["kubernetes"]["pods"][0]["metadata"]["ownerReferences"] = json!([immediate.clone()]);
        controller(&server, &immediate, Some(parent), false).await;
        Mock::given(method("GET"))
            .and(path("/apis/apps/v1/namespaces/demo/deployments/app"))
            .respond_with(ResponseTemplate::new(403))
            .with_priority(1)
            .expect(1)
            .mount(&server)
            .await;
        mount_bundle(&server, input).await;
        let mut arguments = vec!["-g", "workload", "-o", "json"];
        if let Some(selected) = selected {
            arguments.extend(["--workload", selected, "--fail-on", "critical"]);
        }
        let output = config.run(&arguments);
        let unknown = selected == Some("deployment/app");
        assert_eq!(
            output.status.code(),
            Some(if unknown { 4 } else { 0 }),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let report: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(report["selection"]["complete"], !unknown);
        if !unknown {
            assert_eq!(report["records"].as_array().unwrap().len(), 1);
            assert_eq!(report["consumers"][0]["workload_state"], "unknown");
            assert_eq!(
                report["consumers"][0]["workload_chain"][0]["uid"],
                "replica-uid"
            );
        }
        assert!(
            report["diagnostics"]
                .as_array()
                .unwrap()
                .iter()
                .any(|diagnostic| diagnostic["reason"] == "forbidden")
        );
    }
}
