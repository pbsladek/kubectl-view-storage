use super::*;
#[path = "../support/budget.rs"]
mod scenario;

#[derive(Default)]
struct DeniedBulk {
    paths: Mutex<Vec<String>>,
}
impl Transport for DeniedBulk {
    fn get(&self, path: String) -> ResponseFuture<'_> {
        Box::pin(async move {
            self.paths.lock().unwrap().push(path.clone());
            let body = match path.as_str() {
                "/api/v1/persistentvolumeclaims?limit=500" => scenario::claims(),
                "/api/v1/pods?limit=500" => scenario::pods(),
                "/api/v1/persistentvolumes?limit=500" => {
                    return Ok(Response {
                        status: 403,
                        body: serde_json::to_vec(&scenario::denied()).unwrap(),
                        retry_after: None,
                    });
                }
                _ => {
                    let index = path
                        .strip_prefix("/api/v1/persistentvolumes/pv-")
                        .unwrap()
                        .parse::<usize>()
                        .unwrap();
                    assert!(
                        index < scenario::TARGETED_GETS,
                        "request exceeded the enrichment budget"
                    );
                    scenario::volume(index)
                }
            };
            Ok(response(body))
        })
    }
}

#[tokio::test(start_paused = true)]
async fn denied_bulk_then_allowed_targeted_gets_share_the_same_enrichment_budget() {
    let api = Arc::new(DeniedBulk::default());
    let cli = Cli::try_parse_from(["cmd", "-A"]).unwrap();
    let query = cli.query("demo").unwrap();
    let collected = collect(
        api.clone(),
        clock(),
        CancellationToken::new(),
        &cli,
        &query,
        "combined-budget".into(),
    )
    .await
    .unwrap();
    assert_eq!(*api.paths.lock().unwrap(), scenario::expected_paths());
    let report = crate::pipeline::finish(collected, &query).unwrap();
    assert_eq!(report.exit, crate::policy::Exit::Success);
    scenario::assert_report(&report.document);
}
