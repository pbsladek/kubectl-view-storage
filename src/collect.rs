//! Staged collection, deterministic optional budgets, cached lookups, and owned workers.
use crate::{
    cli::{Cli, Kind, Query},
    clock::Clock,
    join, metrics,
    model::*,
    report::{Collection, Cost, Selection, SourceCollection},
    transport::{self, ApiScope, Inventory, Object},
    work::{self, CheckedError, Checkpoint, RunControl, RunError, Stop},
};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    future::Future,
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::{
    task::JoinSet,
    time::{Instant, sleep, timeout_at},
};
use tokio_util::sync::CancellationToken;

pub type ResponseFuture<'a> = Pin<Box<dyn Future<Output = Result<Response, Failure>> + Send + 'a>>;
pub trait Transport: Send + Sync + 'static {
    /// Implementations enforce the body cap while streaming, including errors.
    fn get(&self, path: String) -> ResponseFuture<'_>;
    /// Cumulative bytes consumed from response streams, including interrupted
    /// and rejected bodies. Real streaming adapters expose this counter.
    fn response_bytes(&self) -> Option<usize> {
        None
    }
}
#[derive(Debug, Clone)]
pub struct Response {
    pub status: u16,
    pub body: Vec<u8>,
    pub retry_after: Option<Duration>,
}

#[derive(Debug, Clone, thiserror::Error)]
pub enum Failure {
    #[error("HTTP status {0}")]
    Http(u16),
    #[error("transport failure")]
    Transport,
    #[error("authentication failed")]
    Auth,
    #[error("request deadline exceeded")]
    Timeout,
    #[error("collection deadline exceeded")]
    Deadline,
    #[error("optional request budget exhausted")]
    Budget,
    #[error("run interrupted")]
    Cancelled,
    #[error("response exceeds size limit")]
    TooLarge,
    #[error("invalid response document")]
    Invalid,
    #[error("collection worker failed")]
    Worker,
}
impl Failure {
    pub fn reason(&self) -> &'static str {
        match self {
            Self::Http(403) => "forbidden",
            Self::Http(404) => "not_found",
            Self::Http(_) => "http_error",
            Self::Transport => "transport_error",
            Self::Auth => "authentication_failed",
            Self::Timeout => "timeout",
            Self::Deadline => "deadline_exceeded",
            Self::Budget => "budget_exhausted",
            Self::Cancelled => "cancelled",
            Self::TooLarge => "response_too_large",
            Self::Invalid => "invalid_response",
            Self::Worker => "worker_failure",
        }
    }
    fn fatal(&self) -> bool {
        matches!(self, Self::Auth | Self::Http(401) | Self::Worker)
    }
}
#[derive(Debug)]
pub struct Collected {
    pub graph: join::Graph,
    pub selection: Selection,
    pub collection: Collection,
}

struct RequestContext<T: Transport> {
    transport: Arc<T>,
    clock: Arc<dyn Clock>,
    cancel: CancellationToken,
    deadline: Instant,
    request_timeout: Duration,
    totals: Arc<AtomicAttempts>,
}
impl<T: Transport> Clone for RequestContext<T> {
    fn clone(&self) -> Self {
        Self {
            transport: self.transport.clone(),
            clock: self.clock.clone(),
            cancel: self.cancel.clone(),
            deadline: self.deadline,
            request_timeout: self.request_timeout,
            totals: self.totals.clone(),
        }
    }
}
#[derive(Debug, Default)]
struct Attempts {
    count: usize,
    bytes: usize,
}

#[derive(Default)]
struct AtomicAttempts {
    count: AtomicUsize,
    bytes: AtomicUsize,
}

struct DocumentBody {
    bytes: Vec<u8>,
    deadline: Instant,
}
enum AttemptOutcome {
    Body(DocumentBody),
    Http(u16, Option<Duration>),
}

async fn attempt<T: Transport>(
    context: &RequestContext<T>,
    path: &str,
    counts: &mut Attempts,
) -> Result<AttemptOutcome, Failure> {
    if context.cancel.is_cancelled() {
        return Err(Failure::Cancelled);
    }
    if context.clock.monotonic() >= context.deadline {
        return Err(Failure::Deadline);
    }
    let deadline = context
        .clock
        .monotonic()
        .checked_add(context.request_timeout)
        .unwrap_or(context.deadline)
        .min(context.deadline);
    counts.count += 1;
    context.totals.count.fetch_add(1, Ordering::Relaxed);
    let response = tokio::select! {
        biased;
        _=context.cancel.cancelled()=>return Err(Failure::Cancelled),
        result=timeout_at(deadline,context.transport.get(path.to_owned()))=>result.map_err(|_|Failure::Timeout)??,
    };
    counts.bytes += response.body.len();
    context
        .totals
        .bytes
        .fetch_add(response.body.len(), Ordering::Relaxed);
    if response.body.len() > transport::MAX_RESPONSE_BYTES {
        return Err(Failure::TooLarge);
    }
    let retry = response.retry_after;
    if !(200..300).contains(&response.status) {
        return Ok(AttemptOutcome::Http(response.status, retry));
    }
    Ok(AttemptOutcome::Body(DocumentBody {
        bytes: response.body,
        deadline,
    }))
}

async fn decode_document<T: Transport>(
    context: &RequestContext<T>,
    body: DocumentBody,
) -> Result<Value, Failure> {
    let request_limited = body.deadline < context.deadline;
    work::run(
        RunControl::bounded(context.cancel.clone(), context.clock.clone(), body.deadline),
        move |control| transport::decode_json_checked(&body.bytes, control),
    )
    .await
    .map_err(|error| match error {
        RunError::Stopped(Stop::Deadline) if request_limited => Failure::Timeout,
        other => processing_failure(other),
    })
}

fn retryable(error: &Failure) -> bool {
    matches!(
        error,
        Failure::Transport | Failure::Http(429 | 500 | 502 | 503 | 504)
    )
}
async fn backoff<T: Transport>(
    context: &RequestContext<T>,
    retry: usize,
    delay: Option<Duration>,
) -> Result<(), Failure> {
    let delay = delay.unwrap_or(Duration::from_millis(100 * (retry as u64 + 1)));
    if context
        .clock
        .monotonic()
        .checked_add(delay)
        .is_none_or(|end| end >= context.deadline)
    {
        return Err(Failure::Deadline);
    }
    tokio::select! {biased;_ = context.cancel.cancelled()=>Err(Failure::Cancelled),_ = sleep(delay)=>Ok(())}
}

struct Collector<T: Transport> {
    request: RequestContext<T>,
    overall: Instant,
    enrichment: bool,
    allow_bulk: bool,
    cost: Cost,
    cache: BTreeMap<String, Result<Arc<Value>, Failure>>,
    fatal_failure: Option<Failure>,
    sources: Vec<SourceCollection>,
    diagnostics: Vec<Diagnostic>,
}
impl<T: Transport> Collector<T> {
    fn fatal(&self) -> Option<Failure> {
        self.fatal_failure.clone()
    }
    fn remember_fatal(&mut self, error: &Failure) {
        if error.fatal() {
            self.fatal_failure.get_or_insert_with(|| error.clone());
        }
    }
    async fn fetch(&mut self, path: &str) -> Result<Arc<Value>, Failure> {
        if self.request.cancel.is_cancelled() {
            return Err(Failure::Cancelled);
        }
        if let Some(error) = self.fatal() {
            return Err(error);
        }
        if let Some(result) = self.cache.get(path) {
            return result.clone();
        }
        let mut result = Err(Failure::Transport);
        for retry in 0..3 {
            if self.enrichment && self.cost.enrichment_attempts >= self.cost.enrichment_budget {
                result = Err(Failure::Budget);
                break;
            }
            let mut attempts = Attempts::default();
            let response = attempt(&self.request, path, &mut attempts).await;
            if self.enrichment {
                self.cost.enrichment_attempts += attempts.count;
            } else {
                self.cost.base_attempts += attempts.count;
            }
            self.cost.response_bytes += attempts.bytes;
            let (next, delay) = match response {
                Ok(AttemptOutcome::Http(code, delay)) => (Err(Failure::Http(code)), delay),
                Ok(AttemptOutcome::Body(body)) => (
                    decode_document(&self.request, body).await.map(Arc::new),
                    None,
                ),
                Err(error) => (Err(error), None),
            };
            let again = next.as_ref().err().is_some_and(retryable) && retry < 2;
            result = next;
            if !again {
                break;
            }
            if let Err(error) = backoff(&self.request, retry, delay).await {
                result = Err(error);
                break;
            }
        }
        if let Err(error) = &result {
            self.remember_fatal(error);
        }
        if self.enrichment && result.is_err() {
            self.cost.unresolved_prerequisites.insert(path.into());
        }
        self.cache.insert(path.into(), result.clone());
        result
    }

    async fn object(
        &mut self,
        resource: &str,
        scope: &ApiScope,
        path: &str,
    ) -> Result<Object, Failure> {
        let value = self.fetch(path).await?;
        let resource = resource.to_owned();
        let scope = scope.to_owned();
        let result = work::run(
            RunControl::retained_inventory(
                self.request.cancel.clone(),
                self.request.clock.clone(),
                self.request.deadline,
            ),
            move |control| {
                let object = Object::from_value_checked(&value, control)?;
                if !object
                    .valid_api_object_checked(&resource, &scope, control)
                    .map_err(CheckedError::Interrupted)?
                {
                    return Err("invalid_api_object".into());
                }
                Ok(object)
            },
        )
        .await
        .map_err(processing_failure);
        if let Err(error) = &result
            && error.fatal()
        {
            self.remember_fatal(error);
            self.cache.insert(path.to_owned(), Err(error.clone()));
        }
        result
    }

    async fn list(
        &mut self,
        resource: &str,
        scope: &ApiScope,
        path: &str,
        selector: Option<&str>,
    ) -> (Vec<Object>, Result<(), Failure>) {
        let mut objects = vec![];
        let mut continuation = String::new();
        let mut seen = BTreeSet::new();
        let mut rv = None;
        let mut invalid_objects = false;
        let mut identities = BTreeSet::new();
        let result = loop {
            let base_path = path.to_owned();
            let selector = selector.map(str::to_owned);
            let token = continuation;
            let url = match work::run(
                RunControl::retained_inventory(
                    self.request.cancel.clone(),
                    self.request.clock.clone(),
                    self.request.deadline,
                ),
                move |control| {
                    let mut url = format!("{base_path}?limit=500");
                    if let Some(selector) = selector {
                        url.push_str("&labelSelector=");
                        append_encoded(&mut url, &selector, control)
                            .map_err(CheckedError::Interrupted)?;
                    }
                    if !token.is_empty() {
                        url.push_str("&continue=");
                        append_encoded(&mut url, &token, control)
                            .map_err(CheckedError::Interrupted)?;
                    }
                    Ok(url)
                },
            )
            .await
            .map_err(processing_failure)
            {
                Ok(url) => url,
                Err(error) => break Err(error),
            };
            let page = match self.fetch(&url).await {
                Ok(page) => page,
                Err(error) => break Err(error),
            };
            let resource_name = resource.to_owned();
            let scope_name = scope.to_owned();
            let enrichment = self.enrichment;
            let projected = work::run(
                RunControl::retained_inventory(
                    self.request.cancel.clone(),
                    self.request.clock.clone(),
                    self.request.deadline,
                ),
                move |control| {
                    let projection = project_page(
                        &page,
                        &resource_name,
                        &scope_name,
                        enrichment,
                        &mut identities,
                        control,
                    )?;
                    let repeated = !projection.continuation.is_empty()
                        && !seen.insert(
                            work::clone_string(&projection.continuation, control)
                                .map_err(CheckedError::Interrupted)?,
                        );
                    Ok((projection, identities, seen, repeated))
                },
            )
            .await
            .map_err(processing_failure);
            let (projection, restored, restored_seen, repeated) = match projected {
                Ok(projected) => projected,
                Err(error) => {
                    if error.fatal() {
                        self.cache.insert(url, Err(error.clone()));
                    }
                    break Err(error);
                }
            };
            identities = restored;
            seen = restored_seen;
            objects.extend(projection.objects);
            invalid_objects |= projection.invalid_objects;
            let next = projection.continuation;
            rv = projection.resource_version.or(rv);
            if repeated {
                break Err(Failure::Invalid);
            }
            if next.is_empty() {
                break if invalid_objects {
                    Err(Failure::Invalid)
                } else {
                    Ok(())
                };
            }
            continuation = next;
        };
        self.sources.push(SourceCollection {
            resource: resource.into(),
            scope: scope.label().into(),
            verb: "list".into(),
            status: match &result {
                Ok(()) => "complete",
                Err(Failure::Http(403)) => "forbidden",
                Err(Failure::Timeout | Failure::Deadline) => "timeout",
                Err(Failure::Budget) => "budget_exhausted",
                Err(_) => "partial",
            }
            .into(),
            resource_version: rv,
        });
        if let Err(error) = &result {
            self.remember_fatal(error);
            self.diagnose(resource, path, error);
        }
        (objects, result)
    }
    fn diagnose(&mut self, resource: &str, path: &str, error: &Failure) {
        self.diagnostics.push(Diagnostic{code:format!("{resource}_unavailable"),subject_id:None,source:"kubernetes".into(),reason:error.reason().into(),affected_fields:vec![resource.into()],message:format!("{resource} collection failed ({error}); inspect permissions, API availability, and collection limits.")});
        if self.enrichment {
            self.cost.unresolved_prerequisites.insert(path.into());
        }
    }
    async fn get_objects(
        &mut self,
        resource: &str,
        prefix: &str,
        names: Arc<BTreeSet<String>>,
    ) -> Result<Vec<Object>, Failure> {
        if self.request.cancel.is_cancelled() {
            return Err(Failure::Cancelled);
        }
        if let Some(error) = self.fatal() {
            return Err(error);
        }
        let bulk = names.len() >= 32;
        let cluster_scope = self.allow_bulk;
        self.cost.enrichment_strategy.insert(
            resource.into(),
            if bulk && cluster_scope {
                "bulk_list"
            } else {
                "targeted_get"
            }
            .into(),
        );
        let mut objects = BTreeMap::new();
        if bulk && cluster_scope {
            let (listed, result) = self.list(resource, &ApiScope::Cluster, prefix, None).await;
            if let Err(error) = &result
                && (error.fatal() || matches!(error, Failure::Cancelled))
            {
                return Err(error.clone());
            }
            let selected_names = names.clone();
            objects = work::run(
                RunControl::retained_inventory(
                    self.request.cancel.clone(),
                    self.request.clock.clone(),
                    self.request.deadline,
                ),
                move |control| {
                    let mut selected = BTreeMap::new();
                    for object in listed {
                        control.checked()?;
                        if selected_names.contains(&object.metadata.name) {
                            let name = work::clone_string(&object.metadata.name, control)
                                .map_err(CheckedError::Interrupted)?;
                            selected.insert(name, object);
                        }
                    }
                    Ok(selected)
                },
            )
            .await
            .map_err(processing_failure)?;
            if result.is_ok() {
                return self.finish_objects(objects).await;
            }
        }
        let resource_prefix = prefix.to_owned();
        let (restored, lookups) = work::run(
            RunControl::retained_inventory(
                self.request.cancel.clone(),
                self.request.clock.clone(),
                self.request.deadline,
            ),
            move |control| {
                let mut lookups = vec![];
                for name in names.iter() {
                    control.checked()?;
                    if objects.contains_key(name) {
                        continue;
                    }
                    let mut path = format!("{resource_prefix}/");
                    append_encoded(&mut path, name, control).map_err(CheckedError::Interrupted)?;
                    lookups.push((
                        work::clone_string(name, control).map_err(CheckedError::Interrupted)?,
                        path,
                    ));
                }
                Ok((objects, lookups))
            },
        )
        .await
        .map_err(processing_failure)?;
        objects = restored;
        for (index, (name, path)) in lookups.into_iter().enumerate() {
            if index % work::CHECK_INTERVAL == 0 {
                tokio::task::yield_now().await;
                if self.request.cancel.is_cancelled() {
                    return Err(Failure::Cancelled);
                }
            }
            match self.object(resource, &ApiScope::Cluster, &path).await {
                Ok(object) if object.metadata.name == name => {
                    objects.insert(name, object);
                }
                Ok(_) => self.diagnose(resource, &path, &Failure::Invalid),
                Err(error) => {
                    if error.fatal() || matches!(error, Failure::Cancelled) {
                        return Err(error);
                    }
                    self.diagnose(resource, &path, &error);
                }
            }
        }
        self.finish_objects(objects).await
    }
    async fn finish_objects(
        &self,
        objects: BTreeMap<String, Object>,
    ) -> Result<Vec<Object>, Failure> {
        work::run(
            RunControl::retained_inventory(
                self.request.cancel.clone(),
                self.request.clock.clone(),
                self.request.deadline,
            ),
            move |control| {
                let mut result = vec![];
                for object in objects.into_values() {
                    control.checked()?;
                    result.push(object);
                }
                Ok(result)
            },
        )
        .await
        .map_err(processing_failure)
    }
}

struct PageProjection {
    objects: Vec<Object>,
    invalid_objects: bool,
    continuation: String,
    resource_version: Option<String>,
}
fn project_page<C: Checkpoint>(
    page: &Value,
    resource: &str,
    scope: &ApiScope,
    enrichment: bool,
    identities: &mut BTreeSet<String>,
    control: &mut C,
) -> Result<PageProjection, CheckedError<C::Error>> {
    let metadata = page
        .get("metadata")
        .and_then(Value::as_object)
        .ok_or("invalid_api_page")?;
    let optional = |key: &str| -> Result<Option<&str>, String> {
        match metadata.get(key) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::String(value)) => Ok(Some(value)),
            _ => Err("invalid_api_page".into()),
        }
    };
    let continuation = work::clone_string(optional("continue")?.unwrap_or(""), control)
        .map_err(CheckedError::Interrupted)?;
    let resource_version = optional("resourceVersion")?
        .map(|value| work::clone_string(value, control))
        .transpose()
        .map_err(CheckedError::Interrupted)?;
    let items = page
        .get("items")
        .and_then(Value::as_array)
        .ok_or("invalid_api_page")?;
    let mut result = PageProjection {
        objects: vec![],
        invalid_objects: false,
        continuation,
        resource_version,
    };
    for item in items {
        control.checked()?;
        let object = match Object::from_value_checked(item, control) {
            Ok(object) => Some(object),
            Err(CheckedError::Invalid(_)) => None,
            Err(error) => return Err(error),
        };
        if let Some(object) = object
            && object
                .valid_api_object_checked(resource, scope, control)
                .map_err(CheckedError::Interrupted)?
        {
            let identity = object
                .metadata
                .uid
                .as_deref()
                .unwrap_or(&object.metadata.name);
            if identities
                .insert(work::clone_string(identity, control).map_err(CheckedError::Interrupted)?)
            {
                result.objects.push(object);
                continue;
            }
        }
        if !enrichment {
            return Err("invalid_api_object".into());
        }
        result.invalid_objects = true;
    }
    Ok(result)
}

pub async fn collect<T: Transport>(
    transport: Arc<T>,
    clock: Arc<dyn Clock>,
    cancel: CancellationToken,
    cli: &Cli,
    query: &Query,
    context_name: String,
) -> Result<Collected, Failure> {
    let stream_counter = transport.clone();
    let initial_bytes = transport.response_bytes();
    let started_at = clock.utc();
    let overall = clock
        .monotonic()
        .checked_add(cli.timeout)
        .ok_or(Failure::Deadline)?;
    let request = RequestContext {
        transport,
        clock: clock.clone(),
        cancel: cancel.child_token(),
        deadline: overall,
        request_timeout: cli.request_timeout,
        totals: Arc::new(AtomicAttempts::default()),
    };
    let mut collector = Collector {
        request,
        overall,
        enrichment: false,
        allow_bulk: query.namespace.is_none(),
        cost: Cost::initial(),
        cache: BTreeMap::new(),
        fatal_failure: None,
        sources: vec![],
        diagnostics: vec![],
    };
    let scope = ApiScope::from_namespace(query.namespace.as_deref());
    let base_path = if query.kind == Kind::Pv {
        "/api/v1/persistentvolumes".into()
    } else {
        namespace_path(query.namespace.as_deref(), "persistentvolumeclaims")
    };
    let resource = if query.kind == Kind::Pv {
        "persistentvolumes"
    } else {
        "persistentvolumeclaims"
    };
    let (base, complete) = collector
        .list(
            resource,
            &scope,
            &base_path,
            query.filters.selector.as_deref(),
        )
        .await;
    complete?;
    let mut inventory = Inventory::default();
    if query.kind == Kind::Pv {
        inventory.pvs = base;
    } else {
        inventory.pvcs = base;
    }
    let mut inventory_query = query.clone();
    // A CSI PV can already settle its driver predicate. Keep unresolved PVC
    // drivers as candidates, but do not enrich a known inventory nonmatch.
    inventory_query.filters.node = None;
    inventory_query.filters.statefulset = None;
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
        enrich_pvs(&mut collector, &mut inventory, query, &relevant).await?;
        let selected = relevant.clone();
        let driver_query = query.clone();
        let classes = inventory_job(
            &mut inventory,
            &collector.request,
            move |inventory, control| {
                driver_class_plan(inventory, &driver_query, &selected, control)
            },
        )
        .await?;
        inventory.storage_classes = collector
            .get_objects(
                "storageclasses",
                "/apis/storage.k8s.io/v1/storageclasses",
                Arc::new(classes),
            )
            .await?;
        let mut driver_query = query.clone();
        driver_query.filters.node = None;
        driver_query.filters.statefulset = None;
        let (restored, matches) = candidates(
            inventory,
            &driver_query,
            false,
            query.filters.node.is_some() || query.filters.statefulset.is_some(),
            RunControl::retained_inventory(cancel.clone(), clock.clone(), overall),
        )
        .await?;
        inventory = restored;
        relevant = Arc::new(matches);
    }
    if query.kind == Kind::Pv {
        let selected = relevant.clone();
        let claims = inventory_job(
            &mut inventory,
            &collector.request,
            move |inventory, control| {
                claim_plan(inventory, &selected, control).map_err(CheckedError::Interrupted)
            },
        )
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
        // Claim verification may reveal a binding conflict that makes a class
        // provisioner unsuitable as evidence for this PV's driver.
        if query.filters.driver.is_some() {
            let mut driver_query = query.clone();
            driver_query.filters.node = None;
            driver_query.filters.statefulset = None;
            let (restored, matches) = candidates(
                inventory,
                &driver_query,
                false,
                query.filters.node.is_some() || query.filters.statefulset.is_some(),
                RunControl::retained_inventory(cancel.clone(), clock.clone(), overall),
            )
            .await?;
            inventory = restored;
            relevant = Arc::new(matches);
        }
    }
    if let Some(error) = collector.fatal() {
        return Err(error);
    }
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
    if query.filters.node.is_some() || query.filters.statefulset.is_some() {
        let (restored, matches) = candidates(
            inventory,
            query,
            pods_complete.is_ok(),
            false,
            RunControl::retained_inventory(cancel.clone(), clock.clone(), overall),
        )
        .await?;
        inventory = restored;
        relevant = Arc::new(matches);
    }
    if query.filters.driver.is_none() {
        enrich_pvs(&mut collector, &mut inventory, query, &relevant).await?;
    }
    enrich_classes(&mut collector, &mut inventory, query, &relevant).await?;
    if let Some(error) = collector.fatal() {
        return Err(error);
    }
    if cancel.is_cancelled() {
        return Err(Failure::Cancelled);
    }
    let cpu_query = query.clone();
    let (mut graph, selection, targets, node_jobs) = work::run(
        RunControl::retained_inventory(cancel.clone(), clock.clone(), overall),
        move |control| {
            let graph =
                join::normalize_checked(&inventory, &cpu_query, pods_complete.is_ok(), control)?;
            let (graph, selection) = crate::report::select_checked(graph, &cpu_query, control)
                .map_err(CheckedError::Interrupted)?;
            let targets = if cpu_query.utilization {
                metrics::node_targets_checked(&graph, control).map_err(CheckedError::Interrupted)?
            } else {
                BTreeSet::new()
            };
            let mut jobs = vec![];
            for node in &targets {
                control.checked()?;
                let mut path = String::from("/api/v1/nodes/");
                append_encoded(&mut path, node, control).map_err(CheckedError::Interrupted)?;
                path.push_str("/proxy/stats/summary");
                jobs.push((
                    work::clone_string(node, control).map_err(CheckedError::Interrupted)?,
                    path,
                ));
            }
            Ok((graph, selection, targets, jobs))
        },
    )
    .await
    .map_err(processing_failure)?;
    graph.diagnostics.extend(collector.diagnostics);
    let mut responses = BTreeMap::new();
    let mut failures: BTreeMap<String, String> = BTreeMap::new();
    if query.utilization {
        collector.cost.node_targets = targets.len();
        let request = RequestContext {
            deadline: collector.overall,
            totals: Arc::new(AtomicAttempts::default()),
            ..collector.request.clone()
        };
        let mut pending = node_jobs.into_iter();
        let mut workers = JoinSet::new();
        loop {
            while workers.len() < usize::from(cli.concurrency)
                && clock.monotonic() < overall
                && !cancel.is_cancelled()
            {
                let Some((node, path)) = pending.next() else {
                    break;
                };
                let context = request.clone();
                workers.spawn(async move {
                    let (result, counts) = node_request(&context, path).await;
                    (node, result, counts)
                });
            }
            if workers.is_empty() {
                break;
            }
            let result = tokio::select! {biased;_ = cancel.cancelled()=>None,_ = tokio::time::sleep_until(overall)=>None,result = workers.join_next()=>result};
            match result {
                Some(Ok((node, result, _counts))) => match result {
                    Ok(body) => {
                        // Only the supervisor owns parsing work. Async HTTP
                        // workers may be aborted after the shutdown allowance;
                        // that must never drop an unjoined blocking parser.
                        match decode_document(&request, body).await {
                            Ok(value) if valid_summary(&value) => {
                                responses.insert(node, value);
                            }
                            Ok(_) => {
                                failures.insert(node, Failure::Invalid.reason().into());
                            }
                            Err(error) => {
                                if error.fatal() || matches!(error, Failure::Cancelled) {
                                    request.cancel.cancel();
                                    shutdown(&mut workers, clock.as_ref()).await;
                                    return Err(error);
                                }
                                failures.insert(node, error.reason().into());
                            }
                        }
                    }
                    Err(error) => {
                        if error.fatal() {
                            request.cancel.cancel();
                            shutdown(&mut workers, clock.as_ref()).await;
                            return Err(error);
                        }
                        failures.insert(node, error.reason().into());
                    }
                },
                Some(Err(_)) => {
                    request.cancel.cancel();
                    shutdown(&mut workers, clock.as_ref()).await;
                    return Err(Failure::Worker);
                }
                None => {
                    request.cancel.cancel();
                    shutdown(&mut workers, clock.as_ref()).await;
                    break;
                }
            }
        }
        collector.cost.node_requests = request.totals.count.load(Ordering::Relaxed);
        collector.cost.response_bytes += request.totals.bytes.load(Ordering::Relaxed);
        // Cancellation due to the overall deadline retains completed inventory.
        if cancel.is_cancelled() {
            return Err(Failure::Cancelled);
        }
    }
    let finished_at = clock.utc();
    let cpu_query = query.clone();
    graph = work::run(
        RunControl::retained_inventory(cancel.clone(), clock.clone(), overall),
        move |control| {
            for node in targets {
                control.checked()?;
                if !responses.contains_key(&node) && !failures.contains_key(&node) {
                    failures.insert(node, "deadline_exceeded".into());
                }
            }
            for (node, reason) in &failures {
                control.checked()?;
                graph.diagnostics.push(Diagnostic {
                    code: "statistics_unavailable".into(),
                    subject_id: None,
                    source: format!("kubelet/{node}"),
                    reason: reason.clone(),
                    affected_fields: vec!["filesystem".into(), "inodes".into()],
                    message: "Node statistics could not be collected; inspect proxy permissions, response validity, and timeouts.".into(),
                });
            }
            metrics::apply_checked(&mut graph, &responses, &failures, &cpu_query, finished_at, control)
                .map_err(CheckedError::Interrupted)?;
            Ok(graph)
        },
    ).await.map_err(processing_failure)?;
    if let (Some(start), Some(end)) = (initial_bytes, stream_counter.response_bytes()) {
        collector.cost.response_bytes = end.checked_sub(start).ok_or(Failure::Worker)?;
    }
    Ok(Collected {
        graph,
        selection,
        collection: Collection {
            started_at,
            finished_at,
            context: context_name,
            sources: collector.sources,
            cost: collector.cost,
        },
    })
}

async fn inventory_job<T: Transport, R: Send + 'static>(
    inventory: &mut Inventory,
    request: &RequestContext<T>,
    operation: impl FnOnce(&mut Inventory, &mut RunControl) -> Result<R, CheckedError<Stop>>
    + Send
    + 'static,
) -> Result<R, Failure> {
    let mut owned = std::mem::take(inventory);
    let (restored, result) = work::run(
        RunControl::retained_inventory(
            request.cancel.clone(),
            request.clock.clone(),
            request.deadline,
        ),
        move |control| {
            let result = operation(&mut owned, control)?;
            Ok((owned, result))
        },
    )
    .await
    .map_err(processing_failure)?;
    *inventory = restored;
    Ok(result)
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
fn claim_plan<C: Checkpoint>(
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
fn volume_plan<C: Checkpoint>(
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
fn class_plan<C: Checkpoint>(
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
fn driver_class_plan<C: Checkpoint>(
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

fn processing_failure(error: RunError) -> Failure {
    match error {
        RunError::Invalid(_) => Failure::Invalid,
        RunError::Stopped(Stop::Cancelled) => Failure::Cancelled,
        RunError::Stopped(Stop::Deadline) => Failure::Deadline,
        RunError::Worker => Failure::Worker,
    }
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

fn valid_summary(value: &Value) -> bool {
    value.is_object()
        && !value
            .get("pods")
            .is_some_and(|pods| !pods.is_array() && !pods.is_null())
}

async fn node_request<T: Transport>(
    context: &RequestContext<T>,
    path: String,
) -> (Result<DocumentBody, Failure>, Attempts) {
    let mut counts = Attempts::default();
    let mut result = Err(Failure::Transport);
    for retry in 0..3 {
        let response = attempt(context, &path, &mut counts).await;
        let (next, delay) = match response {
            Ok(AttemptOutcome::Http(code, delay)) => (Err(Failure::Http(code)), delay),
            Ok(AttemptOutcome::Body(body)) => (Ok(body), None),
            Err(error) => (Err(error), None),
        };
        let again = next.as_ref().err().is_some_and(retryable) && retry < 2;
        result = next;
        if !again {
            break;
        }
        if let Err(error) = backoff(context, retry, delay).await {
            result = Err(error);
            break;
        }
    }
    (result, counts)
}
async fn shutdown<T: 'static>(workers: &mut JoinSet<T>, clock: &dyn Clock) {
    let now = clock.monotonic();
    let deadline = now.checked_add(Duration::from_secs(1)).unwrap_or(now);
    if timeout_at(deadline, async {
        while workers.join_next().await.is_some() {}
    })
    .await
    .is_err()
    {
        workers.abort_all();
        while workers.join_next().await.is_some() {}
    }
}
fn namespace_path(namespace: Option<&str>, resource: &str) -> String {
    namespace
        .map(|ns| format!("/api/v1/namespaces/{}/{resource}", encode(ns)))
        .unwrap_or_else(|| format!("/api/v1/{resource}"))
}
fn encode(input: &str) -> String {
    let mut result = String::new();
    append_encoded(&mut result, input, &mut work::Unbounded).unwrap_or_else(|never| match never {});
    result
}
fn append_encoded<C: Checkpoint>(
    result: &mut String,
    input: &str,
    control: &mut C,
) -> Result<(), C::Error> {
    use std::fmt::Write;
    for byte in input.bytes() {
        control.step()?;
        if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) {
            result.push(char::from(byte));
        } else {
            let _ = write!(result, "%{byte:02X}");
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "../tests/unit/collect.rs"]
mod tests;
