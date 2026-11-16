//! Prioritized optional enrichment and query membership planning.
use super::Transport;
use super::{
    Collector, Failure, append_encoded, inventory_job, namespace_path, processing_failure,
};
use crate::{
    cli::{Kind, Query},
    clock::Clock,
    join,
    model::*,
    transport::{ApiScope, Inventory, Object},
    work::{self, CheckedError, Checkpoint, RunControl},
};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
    time::Duration,
};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

async fn enrich_workloads<T: Transport>(
    collector: &mut Collector<T>,
    inventory: &mut Inventory,
    query: &Query,
    relevant: &Arc<BTreeSet<RecordId>>,
    pods_complete: bool,
) -> Result<(), Failure> {
    let planned = workload_plan(collector, inventory, query, relevant, pods_complete).await?;
    let mut pending: std::collections::VecDeque<_> = planned.into_iter().collect();
    let mut seen = BTreeSet::new();
    while let Some((namespace, owner)) = pending.pop_front() {
        tokio::task::yield_now().await;
        if collector.request.cancel.is_cancelled() {
            return Err(Failure::Cancelled);
        }
        if !seen.insert((namespace.clone(), owner.clone())) {
            continue;
        }
        let Some((version, resource, kind)) = crate::workloads::resource(&owner.kind) else {
            continue;
        };
        let ns = namespace.clone();
        let name = owner.name.clone();
        let path = work::run(
            RunControl::retained_inventory(
                collector.request.cancel.clone(),
                collector.request.clock.clone(),
                collector.overall,
            ),
            move |control| {
                let mut path = format!("/apis/{version}/namespaces/");
                append_encoded(&mut path, &ns, control).map_err(CheckedError::Interrupted)?;
                path.push_str(&format!("/{resource}/"));
                append_encoded(&mut path, &name, control).map_err(CheckedError::Interrupted)?;
                Ok(path)
            },
        )
        .await
        .map_err(processing_failure)?;
        let result = collector
            .object(resource, &ApiScope::Namespace(namespace.clone()), &path)
            .await;
        match result {
            Ok(object) if owner_matches(&object, &owner, version, kind) => {
                let (object, parent) = work::run(
                    RunControl::retained_inventory(
                        collector.request.cancel.clone(),
                        collector.request.clock.clone(),
                        collector.overall,
                    ),
                    move |control| {
                        let parent = crate::workloads::controller(&object, control)
                            .map_err(CheckedError::Interrupted)?;
                        Ok((object, parent))
                    },
                )
                .await
                .map_err(processing_failure)?;
                if matches!(owner.kind.as_str(), "replicaset" | "job")
                    && let MetadataValue::Known(parent) = parent
                {
                    pending.push_back((namespace, parent));
                }
                inventory.workloads.push(object);
            }
            Ok(_) => collector.diagnose(resource, &path, &Failure::Invalid),
            Err(error) => {
                if error.fatal() || matches!(error, Failure::Cancelled) {
                    return Err(error);
                }
                collector.diagnose(resource, &path, &error);
                if matches!(error, Failure::Budget | Failure::Deadline) {
                    break;
                }
            }
        }
    }
    Ok(())
}

fn selected_uids<'a, C: Checkpoint>(
    relevant: &'a BTreeSet<RecordId>,
    kind: Kind,
    control: &mut C,
) -> Result<BTreeSet<&'a str>, C::Error> {
    let prefix = if kind == Kind::Pv { "pv:" } else { "pvc:" };
    let mut selected = BTreeSet::new();
    for id in relevant {
        control.step()?;
        if let Some(uid) = id.as_str().strip_prefix(prefix) {
            selected.insert(uid);
        }
    }
    Ok(selected)
}

fn selected_object(object: &Object, selected: &BTreeSet<&str>) -> bool {
    object
        .metadata
        .uid
        .as_deref()
        .is_some_and(|uid| selected.contains(uid))
}

pub(super) fn claim_plan<C: Checkpoint>(
    inventory: &Inventory,
    relevant: &BTreeSet<RecordId>,
    control: &mut C,
) -> Result<BTreeSet<(String, String, String)>, C::Error> {
    let selected = selected_uids(relevant, Kind::Pv, control)?;
    let mut claims = BTreeSet::new();
    for pv in &inventory.pvs {
        control.step()?;
        if !selected_object(pv, &selected) {
            continue;
        }
        if let (Some(namespace), Some(name)) = (
            pv.spec
                .pointer("/claimRef/namespace")
                .and_then(Value::as_str),
            pv.spec.pointer("/claimRef/name").and_then(Value::as_str),
        ) {
            let mut path = String::from("/api/v1/namespaces/");
            append_encoded(&mut path, namespace, control)?;
            path.push_str("/persistentvolumeclaims/");
            append_encoded(&mut path, name, control)?;
            claims.insert((
                work::clone_string(namespace, control)?,
                work::clone_string(name, control)?,
                path,
            ));
        }
    }
    Ok(claims)
}

pub(super) fn volume_plan<C: Checkpoint>(
    inventory: &Inventory,
    kind: Kind,
    relevant: &BTreeSet<RecordId>,
    control: &mut C,
) -> Result<BTreeSet<String>, C::Error> {
    let selected = selected_uids(relevant, Kind::Pvc, control)?;
    let mut names = BTreeSet::new();
    for pvc in &inventory.pvcs {
        control.step()?;
        if kind != Kind::Pv && !selected_object(pvc, &selected) {
            continue;
        }
        if let Some(name) = pvc
            .spec
            .get("volumeName")
            .and_then(Value::as_str)
            .filter(|name| !name.is_empty())
        {
            names.insert(work::clone_string(name, control)?);
        }
    }
    Ok(names)
}

pub(super) fn class_plan<C: Checkpoint>(
    inventory: &Inventory,
    kind: Kind,
    relevant: &BTreeSet<RecordId>,
    volumes: &BTreeSet<String>,
    control: &mut C,
) -> Result<BTreeSet<String>, C::Error> {
    let selected = selected_uids(relevant, kind, control)?;
    let mut classes = BTreeSet::new();
    // Keep PVC identities and PV names in their own resource scopes. A PVC
    // named like a referenced PV must not add unrelated class prerequisites.
    for (objects, pv_objects) in [(&inventory.pvcs, false), (&inventory.pvs, true)] {
        for object in objects {
            control.step()?;
            let include = match (kind, pv_objects) {
                (Kind::Pvc, false) | (Kind::Pv, true) => selected_object(object, &selected),
                (Kind::Pvc, true) => volumes.contains(&object.metadata.name),
                (Kind::Pv, false) => false,
            };
            if !include {
                continue;
            }
            if let Some(class) = object
                .spec
                .get("storageClassName")
                .and_then(Value::as_str)
                .filter(|name| !name.is_empty())
            {
                classes.insert(work::clone_string(class, control)?);
            }
        }
    }
    Ok(classes)
}

pub(super) fn driver_class_plan<C: Checkpoint>(
    inventory: &Inventory,
    query: &Query,
    relevant: &BTreeSet<RecordId>,
    control: &mut C,
) -> Result<BTreeSet<String>, CheckedError<C::Error>> {
    let graph = join::normalize_checked(inventory, query, false, control)?;
    let mut volumes = BTreeSet::new();
    for volume in &inventory.pvs {
        control.checked()?;
        volumes.insert(volume.metadata.name.as_str());
    }
    let mut classes = BTreeSet::new();
    for record in graph.records {
        control.checked()?;
        if !relevant.contains(&record.id)
            || !matches!(record.driver, MetadataValue::Unknown)
            || record.binding_conflict
            || record.kind == Kind::Pvc
                && record
                    .pv_name
                    .as_deref()
                    .is_some_and(|name| !volumes.contains(name))
        {
            continue;
        }
        if let MetadataValue::Known(class) = record.storage_class {
            classes.insert(class);
        }
    }
    Ok(classes)
}

async fn enrich_pvs<T: Transport>(
    collector: &mut Collector<T>,
    inventory: &mut Inventory,
    query: &Query,
    relevant: &Arc<BTreeSet<RecordId>>,
) -> Result<(), Failure> {
    if query.kind == Kind::Pv {
        return Ok(());
    }
    let selected = relevant.clone();
    let kind = query.kind;
    let names = Arc::new(
        inventory_job(inventory, &collector.request, move |inventory, control| {
            volume_plan(inventory, kind, &selected, control).map_err(CheckedError::Interrupted)
        })
        .await?,
    );
    // allow_bulk is false for namespace scope, preserving targeted reads.
    inventory.pvs = collector
        .get_objects(
            "persistentvolumes",
            "/api/v1/persistentvolumes",
            names.clone(),
        )
        .await?;
    Ok(())
}

async fn enrich_classes<T: Transport>(
    collector: &mut Collector<T>,
    inventory: &mut Inventory,
    query: &Query,
    relevant: &Arc<BTreeSet<RecordId>>,
) -> Result<(), Failure> {
    let selected = relevant.clone();
    let kind = query.kind;
    let classes = inventory_job(inventory, &collector.request, move |inventory, control| {
        let volumes = if kind == Kind::Pvc {
            volume_plan(inventory, kind, &selected, control).map_err(CheckedError::Interrupted)?
        } else {
            BTreeSet::new()
        };
        let mut classes = class_plan(inventory, kind, &selected, &volumes, control)
            .map_err(CheckedError::Interrupted)?;
        for existing in &inventory.storage_classes {
            control.checked()?;
            classes.remove(&existing.metadata.name);
        }
        Ok(classes)
    })
    .await?;
    if classes.is_empty() {
        return Ok(());
    }
    let added = collector
        .get_objects(
            "storageclasses",
            "/apis/storage.k8s.io/v1/storageclasses",
            Arc::new(classes),
        )
        .await?;
    inventory_job(inventory, &collector.request, move |inventory, control| {
        for class in added {
            control.checked()?;
            inventory.storage_classes.push(class);
        }
        Ok(())
    })
    .await?;
    Ok(())
}

async fn candidates(
    input: Inventory,
    query: &Query,
    pods_complete: bool,
    retain_unknown: bool,
    control: RunControl,
) -> Result<(Inventory, BTreeSet<RecordId>), Failure> {
    let query = query.clone();
    work::run(control, move |control| {
        let graph = join::normalize_checked(&input, &query, pods_complete, control)?;
        let mut consumers = BTreeMap::new();
        for consumer in &graph.consumers {
            control.checked()?;
            consumers.insert(consumer.id.clone(), consumer);
        }
        let mut selected = BTreeSet::new();
        for record in &graph.records {
            control.checked()?;
            let include = match join::matches_checked(record, &query, &consumers, control)
                .map_err(CheckedError::Interrupted)?
            {
                Predicate::Match => true,
                Predicate::Unknown(_) => retain_unknown,
                Predicate::NoMatch => false,
            };
            if include {
                selected.insert(record.id.clone());
            }
        }
        Ok((input, selected))
    })
    .await
    .map_err(processing_failure)
}

pub(super) async fn enrich_inventory<T: Transport>(
    collector: &mut Collector<T>,
    mut inventory: Inventory,
    query: &Query,
    cancel: &CancellationToken,
) -> Result<(Inventory, bool), Failure> {
    let clock = collector.request.clock.clone();
    let overall = collector.overall;
    let mut inventory_query = query.clone();
    // A CSI PV can already settle its driver predicate. Keep unresolved PVC
    // drivers as candidates, but do not enrich a known inventory nonmatch.
    inventory_query.filters.node = None;
    inventory_query.filters.statefulset = None;
    inventory_query.filters.workload = None;
    let (restored, relevant) = candidates(
        inventory,
        &inventory_query,
        false,
        true,
        RunControl::retained_inventory(cancel.clone(), clock.clone(), overall),
    )
    .await?;
    inventory = restored;
    let mut relevant = Arc::new(relevant);
    let remaining = overall.saturating_duration_since(clock.monotonic());
    let allowance = Duration::from_secs(30).min(if query.utilization {
        remaining / 2
    } else {
        remaining
    });
    collector.request.deadline = clock.monotonic() + allowance;
    collector.enrichment = true;
    // Resolve only driver prerequisites before consumer discovery. A known CSI
    // driver does not need its class merely to decide query membership.
    if query.filters.driver.is_some() {
        enrich_driver_prerequisites(collector, &mut inventory, query, &relevant).await?;
        (inventory, relevant) =
            driver_candidates(inventory, query, cancel, &clock, overall).await?;
    }
    if query.kind == Kind::Pv {
        enrich_claims(collector, &mut inventory, &relevant, cancel).await?;
        // Claim verification may reveal a binding conflict that makes a class
        // provisioner unsuitable as evidence for this PV's driver.
        if query.filters.driver.is_some() {
            (inventory, relevant) =
                driver_candidates(inventory, query, cancel, &clock, overall).await?;
        }
    }
    if let Some(error) = collector.fatal() {
        return Err(error);
    }
    let pods_complete = discover_consumers(collector, &mut inventory, query, &relevant).await?;
    if needs_consumer_membership(query) {
        let (restored, matches) = candidates(
            inventory,
            query,
            pods_complete,
            false,
            RunControl::retained_inventory(cancel.clone(), clock.clone(), overall),
        )
        .await?;
        inventory = restored;
        relevant = Arc::new(matches);
    }
    if query.filters.driver.is_none() {
        enrich_pvs(collector, &mut inventory, query, &relevant).await?;
    }
    enrich_classes(collector, &mut inventory, query, &relevant).await?;
    if let Some(error) = collector.fatal() {
        return Err(error);
    }
    if cancel.is_cancelled() {
        return Err(Failure::Cancelled);
    }
    Ok((inventory, pods_complete))
}

async fn enrich_driver_prerequisites<T: Transport>(
    collector: &mut Collector<T>,
    inventory: &mut Inventory,
    query: &Query,
    relevant: &Arc<BTreeSet<RecordId>>,
) -> Result<(), Failure> {
    enrich_pvs(collector, inventory, query, relevant).await?;
    let selected = relevant.clone();
    let driver_query = query.clone();
    let classes = inventory_job(inventory, &collector.request, move |inventory, control| {
        driver_class_plan(inventory, &driver_query, &selected, control)
    })
    .await?;
    inventory.storage_classes = collector
        .get_objects(
            "storageclasses",
            "/apis/storage.k8s.io/v1/storageclasses",
            Arc::new(classes),
        )
        .await?;
    Ok(())
}

async fn driver_candidates(
    inventory: Inventory,
    query: &Query,
    cancel: &CancellationToken,
    clock: &Arc<dyn Clock>,
    overall: Instant,
) -> Result<(Inventory, Arc<BTreeSet<RecordId>>), Failure> {
    let mut driver_query = query.clone();
    driver_query.filters.node = None;
    driver_query.filters.statefulset = None;
    driver_query.filters.workload = None;
    let (restored, matches) = candidates(
        inventory,
        &driver_query,
        false,
        needs_consumer_membership(query),
        RunControl::retained_inventory(cancel.clone(), clock.clone(), overall),
    )
    .await?;

    Ok((restored, Arc::new(matches)))
}

async fn enrich_claims<T: Transport>(
    collector: &mut Collector<T>,
    inventory: &mut Inventory,
    relevant: &Arc<BTreeSet<RecordId>>,
    cancel: &CancellationToken,
) -> Result<(), Failure> {
    let selected = relevant.clone();
    let claims = inventory_job(inventory, &collector.request, move |inventory, control| {
        claim_plan(inventory, &selected, control).map_err(CheckedError::Interrupted)
    })
    .await?;
    for (index, (namespace, name, path)) in claims.into_iter().enumerate() {
        if index % work::CHECK_INTERVAL == 0 {
            tokio::task::yield_now().await;
            if cancel.is_cancelled() {
                return Err(Failure::Cancelled);
            }
        }
        match collector
            .object(
                "persistentvolumeclaims",
                &ApiScope::Namespace(namespace),
                &path,
            )
            .await
        {
            Ok(object) if object.metadata.name == name => {
                inventory.pvcs.push(object);
            }
            Ok(_) => collector.diagnose("persistentvolumeclaims", &path, &Failure::Invalid),
            Err(error) => {
                if error.fatal() || matches!(error, Failure::Cancelled) {
                    return Err(error);
                }
                collector.diagnose("persistentvolumeclaims", &path, &error);
            }
        }
    }
    Ok(())
}

async fn workload_plan<T: Transport>(
    collector: &Collector<T>,
    inventory: &mut Inventory,
    query: &Query,
    relevant: &Arc<BTreeSet<RecordId>>,
    pods_complete: bool,
) -> Result<BTreeSet<(String, Workload)>, Failure> {
    let cpu_query = query.clone();
    let selected = relevant.clone();
    let planned = inventory_job(inventory, &collector.request, move |inventory, control| {
        let graph = join::normalize_checked(inventory, &cpu_query, pods_complete, control)?;
        let mut consumers = BTreeSet::new();
        for record in &graph.records {
            control.checked()?;
            if selected.contains(&record.id) {
                for consumer in &record.consumers {
                    control.checked()?;
                    consumers.insert(consumer.as_str());
                }
            }
        }
        let mut pending = BTreeSet::new();
        for pod in &inventory.pods {
            control.checked()?;
            if !pod
                .metadata
                .uid
                .as_ref()
                .is_some_and(|uid| consumers.contains(format!("pod:{uid}").as_str()))
                || matches!(pod.status["phase"].as_str(), Some("Succeeded" | "Failed"))
            {
                continue;
            }
            if let MetadataValue::Known(owner) =
                crate::workloads::controller(pod, control).map_err(CheckedError::Interrupted)?
            {
                pending.insert((pod.metadata.namespace.clone().unwrap_or_default(), owner));
            }
        }
        Ok(pending)
    })
    .await?;
    Ok(planned)
}

async fn discover_consumers<T: Transport>(
    collector: &mut Collector<T>,
    inventory: &mut Inventory,
    query: &Query,
    relevant: &Arc<BTreeSet<RecordId>>,
) -> Result<bool, Failure> {
    let scope = ApiScope::from_namespace(query.namespace.as_deref());
    let pod_path = namespace_path(query.namespace.as_deref(), "pods");
    let (pods, pods_complete) = if relevant.is_empty() {
        (vec![], Ok(()))
    } else {
        collector.list("pods", &scope, &pod_path, None).await
    };
    inventory.pods = pods;
    if let Err(error) = &pods_complete
        && (error.fatal() || matches!(error, Failure::Cancelled))
    {
        return Err(error.clone());
    }
    if query.needs_workloads() {
        enrich_workloads(collector, inventory, query, relevant, pods_complete.is_ok()).await?;
    }
    Ok(pods_complete.is_ok())
}

fn owner_matches(object: &Object, owner: &Workload, version: &str, kind: &str) -> bool {
    object.metadata.name == owner.name
        && object.metadata.uid.as_deref() == Some(owner.uid.as_str())
        && object.api_version.as_deref() == Some(version)
        && object.kind.as_deref() == Some(kind)
}

fn needs_consumer_membership(query: &Query) -> bool {
    query.filters.node.is_some()
        || query.filters.statefulset.is_some()
        || query.filters.workload.is_some()
}
