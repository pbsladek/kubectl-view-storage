#![no_main]
use clap::Parser;
use kubectl_view_storage::{
    cli::{Cli, Query},
    join::{self, Graph},
    prometheus, transport,
};
use libfuzzer_sys::fuzz_target;
use std::sync::LazyLock;

static FIXTURE: LazyLock<(Graph, Query)> = LazyLock::new(|| {
    let input: serde_json::Value =
        serde_json::from_str(include_str!("../../examples/contract/healthy/input.json"))
            .expect("fixed fixture");
    let query = Cli::try_parse_from([
        "fuzz",
        "--metrics-source",
        "prometheus",
        "--prometheus-url",
        "http://localhost:9090",
        "--prometheus-single-cluster",
    ])
    .expect("fixed invocation")
    .query("demo")
    .expect("valid query");
    let graph = join::normalize(
        &serde_json::from_value(input["kubernetes"].clone()).expect("fixed inventory"),
        &query,
        true,
    )
    .expect("fixed graph");
    (graph, query)
});
fuzz_target!(|data: &[u8]| {
    if let Ok(value) = transport::decode_json(data) {
        let (graph, query) = &*FIXTURE;
        let mut graph = graph.clone();
        let now = "2026-11-06T20:00:00Z".parse().expect("fixed clock");
        let _ = prometheus::apply(&mut graph, &value, query, now);
        assert_eq!(
            graph.records.len(),
            1,
            "provider data cannot remove base inventory"
        );
        graph
            .validate()
            .expect("provider data cannot corrupt graph relationships");
    }
});
