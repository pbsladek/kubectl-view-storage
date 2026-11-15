//! Resolve supported Kubernetes controller chains using verified object UIDs.
use crate::{
    model::{MetadataValue, Workload},
    transport::Object,
    work::Checkpoint,
};
use std::collections::{BTreeMap, BTreeSet};

pub fn selector(input: &str) -> Result<String, String> {
    let (kind, name) = input.split_once('/').ok_or("workload requires KIND/NAME")?;
    if resource(kind).is_none()
        || name.is_empty()
        || name.len() > 253
        || !name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'-' | b'.'))
    {
        return Err("workload requires deployment, replicaset, daemonset, statefulset, job, or cronjob followed by /NAME".into());
    }
    Ok(format!("{kind}/{name}"))
}

pub(crate) fn resource(kind: &str) -> Option<(&'static str, &'static str, &'static str)> {
    Some(match kind {
        "deployment" => ("apps/v1", "deployments", "Deployment"),
        "replicaset" => ("apps/v1", "replicasets", "ReplicaSet"),
        "daemonset" => ("apps/v1", "daemonsets", "DaemonSet"),
        "statefulset" => ("apps/v1", "statefulsets", "StatefulSet"),
        "job" => ("batch/v1", "jobs", "Job"),
        "cronjob" => ("batch/v1", "cronjobs", "CronJob"),
        _ => return None,
    })
}

pub(crate) fn controller<C: Checkpoint>(
    object: &Object,
    control: &mut C,
) -> Result<MetadataValue<Workload>, C::Error> {
    let mut owner = MetadataValue::Absent;
    for value in &object.metadata.owner_references {
        control.step()?;
        if !value.is_object() || value.get("controller").is_some_and(|v| !v.is_boolean()) {
            return Ok(MetadataValue::Unknown);
        }
        if value.get("controller").and_then(serde_json::Value::as_bool) != Some(true) {
            continue;
        }
        if !matches!(owner, MetadataValue::Absent) {
            return Ok(MetadataValue::Unknown);
        }
        let kind = value["kind"].as_str().unwrap_or("").to_ascii_lowercase();
        let Some((version, _, expected_kind)) = resource(&kind) else {
            return Ok(MetadataValue::Unknown);
        };
        let (Some(name), Some(uid)) = (value["name"].as_str(), value["uid"].as_str()) else {
            return Ok(MetadataValue::Unknown);
        };
        if value["apiVersion"] != version
            || value["kind"] != expected_kind
            || selector(&format!("{kind}/{name}")).is_err()
            || crate::model::ConsumerId::new(uid).is_err()
        {
            return Ok(MetadataValue::Unknown);
        }
        owner = MetadataValue::Known(Workload {
            kind,
            name: name.into(),
            uid: uid.into(),
        });
    }
    Ok(owner)
}

pub(crate) type Index<'a> = BTreeMap<(&'a str, String, &'a str), &'a Object>;
pub(crate) fn index<'a, C: Checkpoint>(
    objects: &'a [Object],
    control: &mut C,
) -> Result<Index<'a>, C::Error> {
    let mut index = BTreeMap::new();
    for object in objects {
        control.step()?;
        if let (Some(ns), Some(kind)) =
            (object.metadata.namespace.as_deref(), object.kind.as_deref())
        {
            index.insert(
                (ns, kind.to_ascii_lowercase(), object.metadata.name.as_str()),
                object,
            );
        }
    }
    Ok(index)
}

pub(crate) fn resolve<C: Checkpoint>(
    pod: &Object,
    index: &Index<'_>,
    control: &mut C,
) -> Result<(MetadataValue<Workload>, Vec<Workload>), C::Error> {
    let mut next = controller(pod, control)?;
    let mut chain = vec![];
    let mut seen = BTreeSet::new();
    for _ in 0..8 {
        control.step()?;
        let current = match next {
            MetadataValue::Known(owner) => owner,
            MetadataValue::Absent => {
                return Ok((
                    chain
                        .last()
                        .cloned()
                        .map(MetadataValue::Known)
                        .unwrap_or(MetadataValue::Absent),
                    chain,
                ));
            }
            _ => return Ok((MetadataValue::Unknown, chain)),
        };
        if !seen.insert(current.uid.clone()) {
            break;
        }
        let Some(object) = index.get(&(
            pod.metadata.namespace.as_deref().unwrap_or(""),
            current.kind.clone(),
            current.name.as_str(),
        )) else {
            break;
        };
        let Some((version, _, kind)) = resource(&current.kind) else {
            break;
        };
        if object.metadata.uid.as_deref() != Some(&current.uid)
            || object.api_version.as_deref() != Some(version)
            || object.kind.as_deref() != Some(kind)
        {
            break;
        }
        chain.push(current.clone());
        if !matches!(current.kind.as_str(), "replicaset" | "job") {
            return Ok((MetadataValue::Known(current), chain));
        }
        next = controller(object, control)?;
    }
    Ok((MetadataValue::Unknown, chain))
}

#[cfg(test)]
#[path = "../tests/unit/workloads.rs"]
mod tests;
