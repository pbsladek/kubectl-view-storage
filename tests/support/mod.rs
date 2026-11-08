//! Shared law oracles used by Proptest and deterministic regression anchors.
#![allow(clippy::unwrap_used, clippy::expect_used)]
use super::storage::{
    cli::{Cli, Dimension, Query, View},
    collect::Collected,
    join, metrics, output,
    pipeline::{self, Report},
    policy::Exit,
    report::{self, Collection, Cost},
    transport::Inventory,
};
use clap::Parser;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

const NOW: &str = "2026-11-06T20:00:00Z";

fn query(pv: bool, args: &[&str]) -> Query {
    let mut command = vec!["cmd"];
    if pv {
        command.extend(["-A", "--kind", "pv"]);
    }
    command.extend(args);
    Cli::try_parse_from(command).unwrap().query("demo").unwrap()
}

fn record_id(pv: bool, index: usize) -> String {
    if pv {
        format!("pv:pv-{index:03}")
    } else {
        format!("pvc:claim-{index:03}")
    }
}

fn pod(index: usize, claim: usize) -> Value {
    json!({"metadata":{"uid":format!("pod-{index:03}"),"name":format!("pod-{index:03}"),"namespace":"demo"},
        "spec":{"nodeName":format!("node-{}",index%3),"volumes":[{"name":"data","persistentVolumeClaim":{"claimName":format!("claim-{claim:03}")}}],
        "containers":[{"name":"app","image":"example.invalid/property:test","volumeMounts":[{"name":"data","mountPath":"/data"},{"name":"data","mountPath":"/same-claim"}]}]},"status":{"phase":"Running"}})
}

fn dataset(capacities: &[u64], consumers: usize) -> Inventory {
    assert!(!capacities.is_empty());
    let mut claims = vec![];
    let mut volumes = vec![];
    for (index, &capacity) in capacities.iter().enumerate() {
        let claim = format!("claim-{index:03}");
        let volume = format!("pv-{index:03}");
        claims.push(json!({"metadata":{"uid":claim,"name":claim,"namespace":"demo","creationTimestamp":"2026-11-06T19:00:00Z"},
            "spec":{"volumeName":volume,"storageClassName":"fast","resources":{"requests":{"storage":capacity.to_string()}}},
            "status":{"phase":"Bound","capacity":{"storage":capacity.to_string()}}}));
        volumes.push(json!({"metadata":{"uid":volume,"name":volume},"spec":{"storageClassName":"fast","capacity":{"storage":capacity.to_string()},
            "claimRef":{"namespace":"demo","name":claim,"uid":claim},"csi":{"driver":"property.csi.test","volumeHandle":format!("handle-{index}")}},"status":{"phase":"Bound"}}));
    }
    serde_json::from_value(json!({"pvcs":claims,"pvs":volumes,
        "storage_classes":[{"metadata":{"name":"fast"},"provisioner":"property.csi.test"}],
        "pods":(0..consumers).map(|index|pod(index,index%capacities.len())).collect::<Vec<_>>()}))
    .unwrap()
}

fn finish(
    input: &Inventory,
    q: &Query,
    pods_complete: bool,
    summaries: &BTreeMap<String, Value>,
) -> Report {
    let graph = join::normalize(input, q, pods_complete).unwrap();
    let (mut graph, selection) = report::select(graph, q);
    metrics::apply(
        &mut graph,
        summaries,
        &BTreeMap::new(),
        q,
        NOW.parse().unwrap(),
    );
    pipeline::finish(
        Collected {
            graph,
            selection,
            collection: Collection {
                started_at: NOW.parse().unwrap(),
                finished_at: NOW.parse().unwrap(),
                context: "property-fixture".into(),
                sources: vec![],
                cost: Cost::initial(),
            },
        },
        q,
    )
    .unwrap()
}

fn permute<T>(items: &mut Vec<T>, keys: &[u8]) {
    assert!(!keys.is_empty());
    let mut indexed: Vec<_> = std::mem::take(items).into_iter().enumerate().collect();
    indexed.sort_by_key(|(index, _)| (keys[index % keys.len()], *index));
    *items = indexed.into_iter().map(|(_, item)| item).collect();
}

fn assert_same_outputs(left: &Report, right: &Report) {
    assert_eq!(left.document, right.document);
    assert_eq!(left.exit, right.exit);
    for view in [View::Tree, View::Table, View::Summary, View::Issues] {
        assert_eq!(
            output::text(left, view, true, false),
            output::text(right, view, true, false)
        );
    }
    assert_eq!(
        output::csv_rows(&left.document),
        output::csv_rows(&right.document)
    );
}

type ConsumerEdges = BTreeMap<String, BTreeSet<(String, String)>>;

fn group_oracle(
    groups: &[Value],
    quantities: &BTreeMap<String, Option<u64>>,
    input: &Inventory,
    pv: bool,
) -> BTreeSet<String> {
    let mut edges = ConsumerEdges::new();
    for pod in &input.pods {
        if !matches!(
            pod.status["phase"].as_str(),
            Some("Running" | "Pending" | "Unknown")
        ) {
            continue;
        }
        let node = pod.spec["nodeName"].as_str().unwrap().to_owned();
        let label = format!(
            "demo/{}@{}",
            pod.metadata.name,
            pod.metadata.uid.as_deref().unwrap()
        );
        for volume in pod.spec["volumes"].as_array().unwrap() {
            let name = volume["persistentVolumeClaim"]["claimName"]
                .as_str()
                .unwrap();
            let index = input
                .pvcs
                .iter()
                .position(|claim| claim.metadata.name == name)
                .unwrap();
            edges
                .entry(record_id(pv, index))
                .or_default()
                .insert((node.clone(), label.clone()));
        }
    }
    verify_groups(groups, quantities, &edges, None, None)
}

fn verify_groups(
    groups: &[Value],
    quantities: &BTreeMap<String, Option<u64>>,
    edges: &ConsumerEdges,
    node: Option<&str>,
    pod: Option<&str>,
) -> BTreeSet<String> {
    let mut union = BTreeSet::new();
    for group in groups {
        let members: BTreeSet<_> = group["member_ids"]
            .as_array()
            .unwrap()
            .iter()
            .map(|id| id.as_str().unwrap().to_owned())
            .collect();
        assert_eq!(members.len(), group["member_ids"].as_array().unwrap().len());
        let node = if group["dimension"] == "node" {
            group["value"].as_str().or(node)
        } else {
            node
        };
        let pod = if group["dimension"] == "pod" {
            group["value"].as_str().or(pod)
        } else {
            pod
        };
        if node.is_some() || pod.is_some() {
            for member in &members {
                assert!(
                    edges
                        .get(member)
                        .is_some_and(|references| references.iter().any(|(n, p)| {
                            node.is_none_or(|value| value == n)
                                && pod.is_none_or(|value| value == p)
                        })),
                    "group fabricated a node/pod combination for {member}: {node:?}/{pod:?}"
                );
            }
        }
        let known: Vec<_> = members.iter().filter_map(|id| quantities[id]).collect();
        let sum: u128 = known.iter().map(|&value| u128::from(value)).sum();
        for field in ["requested", "bound", "provisioned"] {
            let total = &group["totals"][field];
            assert_eq!(total["known_count"], known.len());
            assert_eq!(total["total_count"], members.len());
            assert_eq!(total["complete"], known.len() == members.len());
            assert_eq!(
                total["known_sum_bytes"],
                if known.is_empty() && !members.is_empty() {
                    Value::Null
                } else {
                    json!(sum.to_string())
                }
            );
        }
        let children = group["children"].as_array().unwrap();
        if !children.is_empty() {
            assert_eq!(
                verify_groups(children, quantities, edges, node, pod),
                members
            );
        }
        union.extend(members);
    }
    union
}

pub fn consumer_capacity_law(capacities: &[u64], consumers: usize, claim_index: usize, pv: bool) {
    let mut input = dataset(capacities, consumers);
    let q = query(pv, &["-g", "node,pod,pvc"]);
    let before = finish(&input, &q, true, &BTreeMap::new());
    let mut additional = pod(consumers, claim_index % capacities.len());
    match claim_index % 4 {
        1 => additional["metadata"]["deletionTimestamp"] = json!("2026-11-06T19:59:59Z"),
        2 => additional["status"]["phase"] = json!("Succeeded"),
        3 => additional["status"]["phase"] = json!("FuturePhase"),
        _ => {}
    }
    input.pods.push(serde_json::from_value(additional).unwrap());
    let after = finish(&input, &q, true, &BTreeMap::new());
    let expected: u128 = capacities.iter().map(|&value| u128::from(value)).sum();
    for field in ["requested", "bound", "provisioned"] {
        assert_eq!(
            after.document["summary"][field],
            before.document["summary"][field]
        );
        assert_eq!(
            after.document["summary"][field]["known_sum_bytes"],
            expected.to_string()
        );
    }
    let quantities = capacities
        .iter()
        .enumerate()
        .map(|(i, &value)| (record_id(pv, i), Some(value)))
        .collect();
    assert_eq!(
        group_oracle(
            after.document["groups"].as_array().unwrap(),
            &quantities,
            &input,
            pv
        )
        .len(),
        capacities.len()
    );
    assert_eq!(
        after.document["relationships"].as_array().unwrap().len(),
        consumers + 1
    );
}

pub fn dimension_order_law(
    capacities: &[u64],
    consumers: usize,
    keys: &[u8],
    prefix: usize,
    pv: bool,
) {
    let input = dataset(capacities, consumers);
    let mut q = query(pv, &[]);
    let original = finish(&input, &q, true, &BTreeMap::new());
    let mut dimensions = vec![
        Dimension::Storageclass,
        Dimension::Namespace,
        Dimension::Pvc,
        Dimension::Pv,
        Dimension::Driver,
        Dimension::Node,
        Dimension::Pod,
    ];
    permute(&mut dimensions, keys);
    q.group_by = dimensions.into_iter().take(prefix.clamp(1, 7)).collect();
    let grouped = finish(&input, &q, true, &BTreeMap::new());
    for field in [
        "requested",
        "bound",
        "provisioned",
        "filesystem",
        "inodes",
        "record_count",
        "finding_counts",
        "phase_counts",
        "selection_complete",
    ] {
        assert_eq!(
            grouped.document["summary"][field],
            original.document["summary"][field]
        );
    }
    let quantities = capacities
        .iter()
        .enumerate()
        .map(|(i, &value)| (record_id(pv, i), Some(value)))
        .collect();
    assert_eq!(
        group_oracle(
            grouped.document["groups"].as_array().unwrap(),
            &quantities,
            &input,
            pv
        )
        .len(),
        capacities.len()
    );
    let mut shuffled = input.clone();
    permute(&mut shuffled.pvcs, keys);
    permute(&mut shuffled.pvs, keys);
    permute(&mut shuffled.pods, keys);
    assert_same_outputs(&grouped, &finish(&shuffled, &q, true, &BTreeMap::new()));
}

pub fn unknown_quantity_law(values: &[(u64, bool)], pv: bool) {
    let mut input = dataset(
        &values.iter().map(|(value, _)| *value).collect::<Vec<_>>(),
        0,
    );
    for (index, (_, missing)) in values.iter().enumerate() {
        if *missing {
            input.pvcs[index].spec["resources"]["requests"]
                .as_object_mut()
                .unwrap()
                .remove("storage");
            input.pvcs[index].status["capacity"]
                .as_object_mut()
                .unwrap()
                .remove("storage");
            input.pvs[index].spec["capacity"]
                .as_object_mut()
                .unwrap()
                .remove("storage");
        }
    }
    let report = finish(&input, &query(pv, &[]), true, &BTreeMap::new());
    let quantities: BTreeMap<_, _> = values
        .iter()
        .enumerate()
        .map(|(i, (value, missing))| (record_id(pv, i), (!missing).then_some(*value)))
        .collect();
    for record in report.document["records"].as_array().unwrap() {
        let expected = quantities[record["id"].as_str().unwrap()]
            .map(|value| json!(value.to_string()))
            .unwrap_or(Value::Null);
        for field in ["requested_bytes", "bound_bytes", "provisioned_bytes"] {
            assert_eq!(record[field], expected);
        }
    }
    group_oracle(
        report.document["groups"].as_array().unwrap(),
        &quantities,
        &input,
        pv,
    );
}

pub fn incomplete_selection_law(capacities: &[u64], driver: bool, pv: bool) {
    let mut input = dataset(capacities, capacities.len());
    let missing_index = capacities.len() - 1;
    if driver {
        if pv {
            input.pvs[missing_index]
                .spec
                .as_object_mut()
                .unwrap()
                .remove("csi");
            input.storage_classes.clear();
        } else {
            input.pvs.pop();
        }
    } else {
        input.pods.clear();
    }
    for strict in ["--require-metrics", "--fail-on"] {
        let mut args = if driver {
            vec!["--driver", "property.csi.test"]
        } else {
            vec!["--node", "node-0"]
        };
        args.push(strict);
        if strict == "--fail-on" {
            args.push("critical");
        }
        let mut q = query(pv, &args);
        let report = finish(&input, &q, driver, &BTreeMap::new());
        assert_eq!(report.exit, Exit::Inconclusive);
        assert_eq!(report.document["selection"]["complete"], false);
        assert_eq!(
            report.document["selection"]["unknown_count"],
            if driver { 1 } else { capacities.len() }
        );
        assert!(output::text(&report, View::Tree, false, false).contains("Selection: incomplete"));
        q.filters.storage_class = Some("definite-nonmatch".into());
        let empty = finish(&input, &q, driver, &BTreeMap::new());
        assert_eq!(
            empty.exit,
            Exit::Success,
            "known false predicate must dominate unknown metadata"
        );
        assert_eq!(empty.document["selection"]["complete"], true);
        assert!(empty.document["records"].as_array().unwrap().is_empty());
    }
}

fn summary(input: &Inventory, values: &[(u64, u64, bool)]) -> BTreeMap<String, Value> {
    let mut nodes: BTreeMap<String, Value> = BTreeMap::new();
    for (index, pod) in input.pods.iter().enumerate() {
        let (capacity, used, missing) = values[index];
        let mut sample = json!({"name":"data","pvcRef":{"namespace":"demo","name":format!("claim-{index:03}")},"time":"2026-11-06T19:59:50Z",
            "capacityBytes":capacity,"usedBytes":used,"availableBytes":capacity-used,"inodes":1000,"inodesUsed":100,"inodesFree":850});
        if missing {
            sample.as_object_mut().unwrap().remove("usedBytes");
        }
        let node = pod.spec["nodeName"].as_str().unwrap().to_owned();
        nodes.entry(node).or_insert_with(||json!({"pods":[]}))["pods"].as_array_mut().unwrap().push(json!({
            "podRef":{"uid":pod.metadata.uid,"namespace":"demo","name":pod.metadata.name},"volume":[sample]}));
    }
    nodes
}

pub fn independent_pressure_law(
    scale: u64,
    percent: u8,
    inodes: bool,
    exhaustion: bool,
    invalid: bool,
    pv: bool,
) {
    let capacity = scale.clamp(1, u64::MAX / 100) * 100;
    let used =
        u64::try_from(u128::from(capacity) * u128::from(percent.clamp(80, 100)) / 100).unwrap();
    let input = dataset(&[1024u64.pow(3)], 1);
    let q = query(pv, &["--require-metrics"]);
    let mut summaries = summary(&input, &[(1000, 100, false)]);
    let sample = &mut summaries.get_mut("node-0").unwrap()["pods"][0]["volume"][0];
    let (cap_key, used_key, free_key) = if inodes {
        ("inodes", "inodesUsed", "inodesFree")
    } else {
        ("capacityBytes", "usedBytes", "availableBytes")
    };
    sample[cap_key] = json!(capacity);
    sample[used_key] = json!(used);
    sample[free_key] = json!(if exhaustion { 0 } else { capacity - used });
    let before = finish(&input, &q, true, &summaries);
    let field = if exhaustion { used_key } else { free_key };
    let sample = &mut summaries.get_mut("node-0").unwrap()["pods"][0]["volume"][0];
    if invalid {
        sample[field] = json!(true);
    } else {
        sample.as_object_mut().unwrap().remove(field);
    }
    let after = finish(&input, &q, true, &summaries);
    let code = match (inodes, exhaustion) {
        (false, false) => "filesystem_usage_high",
        (false, true) => "filesystem_space_exhausted",
        (true, false) => "inode_usage_high",
        (true, true) => "filesystem_inodes_exhausted",
    };
    let expected = if exhaustion || u128::from(used) * 100 >= u128::from(capacity) * 90 {
        "critical"
    } else {
        "warning"
    };
    for report in [&before, &after] {
        let finding = report.document["findings"]
            .as_array()
            .unwrap()
            .iter()
            .find(|finding| finding["code"] == code)
            .unwrap();
        assert_eq!(finding["severity"], expected);
    }
    assert_eq!(before.exit, Exit::Success);
    assert_eq!(
        after.exit,
        if inodes {
            Exit::Success
        } else {
            Exit::Inconclusive
        }
    );
}

pub fn requested_sort_law(values: &[(u64, bool)], keys: &[u8], descending: bool, pv: bool) {
    let mut input = dataset(
        &values.iter().map(|(value, _)| *value).collect::<Vec<_>>(),
        0,
    );
    for (index, (_, missing)) in values.iter().enumerate() {
        if *missing {
            input.pvcs[index].spec["resources"]["requests"]
                .as_object_mut()
                .unwrap()
                .remove("storage");
        }
    }
    let q = query(
        pv,
        &[
            "-g",
            "pvc",
            "--sort",
            if descending {
                "requested:desc"
            } else {
                "requested:asc"
            },
        ],
    );
    let report = finish(&input, &q, true, &BTreeMap::new());
    let mut expected: Vec<_> = values
        .iter()
        .enumerate()
        .map(|(index, (value, missing))| {
            (
                (
                    *missing,
                    if *missing {
                        0
                    } else if descending {
                        u64::MAX - value
                    } else {
                        *value
                    },
                    index,
                ),
                record_id(pv, index),
            )
        })
        .collect();
    expected.sort_by_key(|(key, _)| *key);
    assert_order(
        &report,
        &expected.into_iter().map(|(_, id)| id).collect::<Vec<_>>(),
    );
    permute(&mut input.pvcs, keys);
    permute(&mut input.pvs, keys);
    assert_same_outputs(&report, &finish(&input, &q, true, &BTreeMap::new()));
}

fn assert_order(report: &Report, expected: &[String]) {
    let actual: Vec<_> = report.document["records"]
        .as_array()
        .unwrap()
        .iter()
        .map(|record| record["id"].as_str().unwrap())
        .collect();
    assert_eq!(actual, expected);
    let groups: Vec<_> = report.document["groups"]
        .as_array()
        .unwrap()
        .iter()
        .map(|group| group["member_ids"][0].as_str().unwrap())
        .collect();
    assert_eq!(groups, expected);
    let csv = output::csv_rows(&report.document);
    assert_eq!(
        csv.iter().map(|row| row[3].as_str()).collect::<Vec<_>>(),
        expected
            .iter()
            .map(|id| id.split_once(':').unwrap().1)
            .collect::<Vec<_>>()
    );
}

pub fn ratio_sort_law(values: &[(u64, u64, bool)], keys: &[u8], descending: bool, pv: bool) {
    let values: Vec<_> = values
        .iter()
        .map(|&(capacity, used, missing)| (capacity.max(1), used.min(capacity.max(1)), missing))
        .collect();
    let mut input = dataset(&vec![1024u64.pow(3); values.len()], values.len());
    let mut summaries = summary(&input, &values);
    let q = query(
        pv,
        &[
            "-u",
            "-g",
            "pvc",
            "--sort",
            if descending {
                "used-percent:desc"
            } else {
                "used-percent:asc"
            },
        ],
    );
    let report = finish(&input, &q, true, &summaries);
    let mut indices: Vec<_> = (0..values.len()).collect();
    indices.sort_by(|&a, &b| {
        let (ac, au, am) = values[a];
        let (bc, bu, bm) = values[b];
        am.cmp(&bm)
            .then_with(|| {
                if am || bm {
                    std::cmp::Ordering::Equal
                } else {
                    let order =
                        (u128::from(au) * u128::from(bc)).cmp(&(u128::from(bu) * u128::from(ac)));
                    if descending { order.reverse() } else { order }
                }
            })
            .then(a.cmp(&b))
    });
    assert_order(
        &report,
        &indices
            .into_iter()
            .map(|index| record_id(pv, index))
            .collect::<Vec<_>>(),
    );
    permute(&mut input.pvcs, keys);
    permute(&mut input.pvs, keys);
    permute(&mut input.pods, keys);
    for summary in summaries.values_mut() {
        summary["pods"].as_array_mut().unwrap().reverse();
    }
    assert_same_outputs(&report, &finish(&input, &q, true, &summaries));
}
