#![allow(clippy::unwrap_used, clippy::expect_used)]
use super::*;
use crate::work::Unbounded;
use clap::Parser;
fn query() -> Query {
    Cli::try_parse_from([
        "cmd",
        "--metrics-source",
        "prometheus",
        "--prometheus-url",
        "http://localhost:9090",
        "--prometheus-single-cluster",
    ])
    .unwrap()
    .query("demo")
    .unwrap()
}
fn graph() -> Graph {
    let value: Value =
        serde_json::from_str(include_str!("../../examples/contract/healthy/input.json")).unwrap();
    crate::join::normalize(
        &serde_json::from_value(value["kubernetes"].clone()).unwrap(),
        &query(),
        true,
    )
    .unwrap()
}
fn response() -> Value {
    let now: Timestamp = "2026-11-06T20:00:00Z".parse().unwrap();
    json!({"status":"success","data":{"resultType":"matrix","result":FIELDS.iter().zip([100,95,4,1000,100,850]).map(|(field,value)|
        json!({"metric":{"__name__":format!("kubelet_volume_stats_{field}"),"namespace":"demo","persistentvolumeclaim":"data"},"values":[[now.as_second(),value.to_string()]]})).collect::<Vec<_>>()}})
}
#[test]
fn raw_sample_timestamps_keep_nanoseconds_and_bound_scientific_exponents() {
    for (input, nanos) in [
        ("1794000000.123456789", 1794000000123456789i128),
        ("1.794e9", 1794000000000000000),
        ("-0.000000001", -1),
    ] {
        assert_eq!(
            sample_time(&serde_json::from_str(input).unwrap())
                .unwrap()
                .as_nanosecond(),
            nanos
        );
    }
    for value in [
        json!("not-time"),
        serde_json::from_str("1e999999").unwrap(),
        json!(null),
    ] {
        assert!(sample_time(&value).is_none());
    }
}

#[test]
fn floating_sample_strings_require_exact_nonnegative_integer_counters() {
    for (raw, value) in [
        ("1000", 1000),
        ("1000.0", 1000),
        ("1e3", 1000),
        ("1.23e5", 123000),
        ("-0", 0),
        ("1.00e-0", 1),
        ("18446744073709551615.0", u64::MAX),
    ] {
        assert_eq!(counter_value(raw), Some(value), "{raw}");
    }
    for raw in [
        "NaN",
        "+Inf",
        "-Inf",
        "-1",
        "0.5",
        "1.1e-1",
        "1e999999999",
        "1e-99999999",
        "18446744073709551616",
        "x1.0",
        "1.0.0",
    ] {
        assert!(counter_value(raw).is_none(), "{raw}");
    }
}

#[test]
fn provider_configuration_files_are_bounded_regular_inputs() {
    struct Directory(std::path::PathBuf);
    impl Drop for Directory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "view-storage-prom-config-{}-{stamp}",
        std::process::id()
    ));
    let directory = Directory(path);
    std::fs::create_dir(&directory.0).unwrap();
    let file = directory.0.join("token");
    assert!(read_configuration_file(&file, 64).is_err());
    assert!(read_configuration_file(&directory.0, 64).is_err());
    std::fs::write(&file, b"private-test-token").unwrap();
    assert_eq!(
        read_configuration_file(&file, 64).unwrap(),
        b"private-test-token"
    );
    let error = read_configuration_file(&file, 1).unwrap_err();
    assert!(!error.contains("private-test-token"));
    #[cfg(unix)]
    {
        let fifo = directory.0.join("fifo");
        nix::unistd::mkfifo(&fifo, nix::sys::stat::Mode::S_IRUSR).unwrap();
        assert!(read_configuration_file(&fifo, 64).is_err());
    }
}
#[test]
fn explicit_scope_and_tls_options_are_validated_before_configuration() {
    let base = Cli::try_parse_from(["cmd"]).unwrap();
    let mut cli = base.clone();
    cli.prometheus_url = Some("https://prom.example".into());
    assert!(cli.validate().is_err());
    cli.metrics_source = MetricsSource::Prometheus;
    assert!(cli.validate().is_err());
    cli.prometheus_single_cluster = true;
    assert!(cli.validate().is_ok());
    cli.prometheus_token_file = Some("token".into());
    cli.prometheus_url = Some("http://prom.example".into());
    assert!(cli.validate().is_err());
    cli.prometheus_url = Some("http://127.0.0.1:9090/prom".into());
    assert!(cli.validate().is_ok());
    cli.prometheus_single_cluster = false;
    cli.prometheus_cluster = Some("cluster=production".into());
    assert!(cli.validate().is_ok());
    for selector in [
        "cluster",
        "=x",
        "9label=x",
        "namespace=demo",
        "cluster=",
        "cluster=x\n",
    ] {
        cli.prometheus_cluster = Some(selector.into());
        assert!(cli.validate().is_err());
    }
    for url in [
        "/relative",
        "ftp://prom.example",
        "https://user:secret@prom.example",
        "https://prom.example?token=secret",
        "https://prom.example/#fragment",
    ] {
        assert!(endpoint(url).is_err());
    }
}
#[test]
fn every_supplied_claim_uid_must_match_the_current_claim() {
    let now = "2026-11-06T20:00:00Z".parse().unwrap();
    for wrong_label in [None, Some("pvc_uid"), Some("persistentvolumeclaim_uid")] {
        let mut graph = graph();
        let mut input = response();
        for series in input["data"]["result"].as_array_mut().unwrap() {
            for label in ["pvc_uid", "persistentvolumeclaim_uid"] {
                series["metric"][label] = json!(if wrong_label == Some(label) {
                    "recreated-claim"
                } else {
                    graph.records[0].uid.as_str()
                });
            }
        }
        assert!(apply(&mut graph, &input, &query(), now));
        assert_eq!(
            graph.records[0].filesystem.status(),
            if wrong_label.is_some() {
                "identity_uncertain"
            } else {
                "ok"
            }
        );
        assert_eq!(graph.records.len(), 1);
    }
}

#[test]
fn independent_metric_fields_survive_invalid_siblings_and_no_samples_are_not_zero() {
    let now = "2026-11-06T20:00:00Z".parse().unwrap();
    let mut input = response();
    input["data"]["result"][2]["values"][0][1] = json!("NaN");
    let mut graph = graph();
    apply_checked(&mut graph, Some(&input), "", &query(), now, &mut Unbounded).unwrap();
    let sample = graph.records[0].filesystem.selected().unwrap();
    assert_eq!(sample.used.value().unwrap().get(), 95);
    assert!(matches!(sample.available, Field::Invalid));
    input["data"]["result"] = json!([]);
    apply_checked(&mut graph, Some(&input), "", &query(), now, &mut Unbounded).unwrap();
    assert_eq!(graph.records[0].filesystem.status(), "missing");
    graph.records[0].volume_mode = VolumeMode::Block;
    apply_checked(&mut graph, Some(&input), "", &query(), now, &mut Unbounded).unwrap();
    assert_eq!(graph.records[0].filesystem.status(), "not_applicable");
    graph.records[0].volume_mode = VolumeMode::Unsupported("future".into());
    apply_checked(&mut graph, Some(&input), "", &query(), now, &mut Unbounded).unwrap();
    assert_eq!(graph.records[0].filesystem.status(), "invalid");
}
#[test]
fn malformed_envelopes_and_series_cannot_escape_as_usable_observations() {
    let mut retained = graph();
    assert!(!apply(
        &mut retained,
        &json!({}),
        &query(),
        Timestamp::now()
    ));
    assert_eq!(retained.records.len(), 1);
    assert_eq!(retained.records[0].filesystem.status(), "invalid");
    for input in [
        json!({}),
        json!({"status":"error"}),
        json!({"status":"success","data":{"resultType":"vector","result":[]}}),
        json!({"status":"success","warnings":["truncated"],"data":{"resultType":"matrix","result":[]}}),
    ] {
        assert!(tuples(&input, &mut Unbounded).unwrap().is_none());
    }
    for pointer in [
        "/data/result/0/metric",
        "/data/result/0/values",
        "/data/result/0/values/0",
        "/data/result/0/values/0/0",
        "/data/result/0/values/0/1",
    ] {
        let mut input = response();
        *input.pointer_mut(pointer).unwrap() = Value::Null;
        assert!(
            tuples(&input, &mut Unbounded).unwrap().is_none(),
            "{pointer}"
        );
    }
    struct Stop;
    impl Checkpoint for Stop {
        type Error = ();
        fn step(&mut self) -> Result<(), ()> {
            Err(())
        }
    }
    assert!(tuples(&response(), &mut Stop).is_err());
    assert!(query_path(&query(), &mut Stop).is_err());
    let mut graph = graph();
    assert!(
        apply_checked(
            &mut graph,
            None,
            "timeout",
            &query(),
            Timestamp::now(),
            &mut Stop
        )
        .is_err()
    );
}

#[test]
fn equal_time_conflicts_and_replica_order_do_not_choose_arbitrary_values() {
    let mut input = response();
    let pair = input["data"]["result"][0]["values"][0].clone();
    input["data"]["result"][0]["values"]
        .as_array_mut()
        .unwrap()
        .push(json!([pair[0], "999"]));
    assert!(tuples(&input, &mut Unbounded).unwrap().is_none());
    input["data"]["result"][0]["values"]
        .as_array_mut()
        .unwrap()
        .reverse();
    assert!(tuples(&input, &mut Unbounded).unwrap().is_none());
    let future = pair[0].as_i64().unwrap() + 1;
    input["data"]["result"][0]["values"]
        .as_array_mut()
        .unwrap()
        .push(json!([future, "100"]));
    assert!(tuples(&input, &mut Unbounded).unwrap().is_some());
    input["data"]["result"][0]["values"]
        .as_array_mut()
        .unwrap()
        .reverse();
    assert!(tuples(&input, &mut Unbounded).unwrap().is_some());
}
