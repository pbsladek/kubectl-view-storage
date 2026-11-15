#![allow(clippy::unwrap_used, clippy::expect_used)]
use super::*;
use crate::work::Unbounded;
use serde_json::{Value, json};
fn reference(kind: &str, name: &str, uid: &str) -> Value {
    json!({"apiVersion":if kind=="Job" || kind=="CronJob"{"batch/v1"}else{"apps/v1"},"kind":kind,"name":name,"uid":uid,"controller":true})
}
fn object(kind: &str, name: &str, uid: &str, owner: Option<Value>) -> Object {
    serde_json::from_value(json!({"apiVersion":if kind=="Job"||kind=="CronJob"{"batch/v1"}else{"apps/v1"},"kind":kind,
        "metadata":{"namespace":"demo","name":name,"uid":uid,"ownerReferences":owner.into_iter().collect::<Vec<_>>()},"spec":{}})).unwrap()
}
#[test]
fn controller_references_and_selectors_never_guess_invalid_ownership() {
    let valid = reference("Deployment", "api", "uid");
    for owners in [
        json!([null]),
        json!([{"controller":"true"}]),
        json!([valid.clone(), valid.clone()]),
        json!([{"controller":true,"kind":"CustomController"}]),
        json!([{"apiVersion":"apps/v1","controller":true,"kind":"Deployment","name":"api"}]),
        json!([{"apiVersion":"apps/v2","controller":true,"kind":"Deployment","name":"api","uid":"uid"}]),
        json!([{"apiVersion":"apps/v1","controller":true,"kind":"deployment","name":"api","uid":"uid"}]),
    ] {
        let mut pod = object("Pod", "pod", "pod", None);
        pod.metadata.owner_references = serde_json::from_value(owners).unwrap();
        assert!(matches!(
            controller(&pod, &mut Unbounded).unwrap(),
            MetadataValue::Unknown
        ));
    }
    for value in [
        "deployment/api",
        "replicaset/api-1",
        "daemonset/agent",
        "job/backup",
        "cronjob/backup",
        "statefulset/db",
    ] {
        assert_eq!(selector(value).unwrap(), value);
    }
    for value in [
        "deployment",
        "other/api",
        "deployment/",
        "job/a/b",
        "job/UPPER",
        "job/\n",
    ] {
        assert!(selector(value).is_err());
    }
}
#[test]
fn resolution_preserves_verified_partial_chains_and_stops_cycles_or_missing_objects() {
    let pod = object(
        "Pod",
        "pod",
        "pod",
        Some(reference("Job", "run", "run-uid")),
    );
    let job = object(
        "Job",
        "run",
        "run-uid",
        Some(reference("CronJob", "schedule", "cron-uid")),
    );
    let objects = vec![job.clone()];
    let (root, chain) = resolve(
        &pod,
        &index(&objects, &mut Unbounded).unwrap(),
        &mut Unbounded,
    )
    .unwrap();
    assert!(matches!(root, MetadataValue::Unknown));
    assert_eq!(chain.len(), 1);
    let objects = vec![job, object("CronJob", "schedule", "cron-uid", None)];
    let (root, chain) = resolve(
        &pod,
        &index(&objects, &mut Unbounded).unwrap(),
        &mut Unbounded,
    )
    .unwrap();
    assert_eq!(root.known().unwrap().kind, "cronjob");
    assert_eq!(chain.len(), 2);
    let direct = object("Pod", "pod", "pod", None);
    assert!(matches!(
        resolve(
            &direct,
            &index(&[], &mut Unbounded).unwrap(),
            &mut Unbounded
        )
        .unwrap()
        .0,
        MetadataValue::Absent
    ));
    let standalone = object("Job", "run", "run-uid", None);
    let objects = vec![standalone];
    assert_eq!(
        resolve(
            &pod,
            &index(&objects, &mut Unbounded).unwrap(),
            &mut Unbounded
        )
        .unwrap()
        .0
        .known()
        .unwrap()
        .kind,
        "job"
    );
    let objects = vec![
        object(
            "Job",
            "run",
            "run-uid",
            Some(reference("Job", "peer", "peer-uid")),
        ),
        object(
            "Job",
            "peer",
            "peer-uid",
            Some(reference("Job", "run", "run-uid")),
        ),
    ];
    assert!(matches!(
        resolve(
            &pod,
            &index(&objects, &mut Unbounded).unwrap(),
            &mut Unbounded
        )
        .unwrap()
        .0,
        MetadataValue::Unknown
    ));
    let mut wrong = objects[0].clone();
    wrong.api_version = Some("batch/v2".into());
    assert!(matches!(
        resolve(
            &pod,
            &index(&[wrong], &mut Unbounded).unwrap(),
            &mut Unbounded
        )
        .unwrap()
        .0,
        MetadataValue::Unknown
    ));
}
#[test]
fn owner_resolution_observes_cancellation() {
    struct Stop;
    impl Checkpoint for Stop {
        type Error = ();
        fn step(&mut self) -> Result<(), ()> {
            Err(())
        }
    }
    let pod = object(
        "Pod",
        "pod",
        "pod",
        Some(reference("Job", "run", "run-uid")),
    );
    assert!(controller(&pod, &mut Stop).is_err());
    assert!(index(std::slice::from_ref(&pod), &mut Stop).is_err());
    assert!(resolve(&pod, &BTreeMap::new(), &mut Stop).is_err());
}

#[test]
fn owner_chain_depth_is_bounded_even_for_valid_unique_references() {
    let pod = object(
        "Pod",
        "pod",
        "pod",
        Some(reference("Job", "job-0", "uid-0")),
    );
    let objects: Vec<_> = (0..10)
        .map(|i| {
            object(
                "Job",
                &format!("job-{i}"),
                &format!("uid-{i}"),
                (i < 9).then(|| {
                    reference("Job", &format!("job-{}", i + 1), &format!("uid-{}", i + 1))
                }),
            )
        })
        .collect();
    let (root, chain) = resolve(
        &pod,
        &index(&objects, &mut Unbounded).unwrap(),
        &mut Unbounded,
    )
    .unwrap();
    assert!(matches!(root, MetadataValue::Unknown));
    assert_eq!(chain.len(), 8);
}
