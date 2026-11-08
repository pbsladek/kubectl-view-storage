//! Production adapter. Kubernetes authentication/TLS/proxy handling belongs to kube.
use http_body_util::BodyExt;
use kube::{
    Client, Config,
    config::{ExecInteractiveMode, KubeConfigOptions, Kubeconfig},
};
use kubectl_view_storage::{
    cli::Cli,
    collect::{Failure, Response, ResponseFuture, Transport},
    transport::MAX_RESPONSE_BYTES,
};
use std::{
    sync::atomic::{AtomicUsize, Ordering},
    time::Duration,
};

pub struct Kubernetes {
    client: Client,
    received: AtomicUsize,
}

pub enum Loaded {
    Direct(Kubernetes, String, String),
    IsolateCredentials,
}

impl Kubernetes {
    pub async fn load(cli: &Cli) -> Result<Loaded, String> {
        let config = match &cli.kubeconfig {
            Some(path) => Kubeconfig::read_from(path),
            None => Kubeconfig::read(),
        }
        .map_err(|_| {
            "Cannot read kubeconfig; check --kubeconfig, KUBECONFIG, and file permissions."
                .to_owned()
        })?;
        let context = cli
            .context
            .clone()
            .or_else(|| config.current_context.clone())
            .ok_or("Kubeconfig has no current context; select --context.")?;
        let options = KubeConfigOptions {
            context: Some(context.clone()),
            ..KubeConfigOptions::default()
        };
        let mut config=Config::from_custom_kubeconfig(config,&options).await.map_err(|_|"Cannot resolve Kubernetes context; check context, cluster, CA, and credential-helper configuration.".to_owned())?;
        let namespace = if config.default_namespace.is_empty() {
            "default".to_owned()
        } else {
            config.default_namespace.clone()
        };
        // The collector clips every operation to the remaining stage deadline.
        config.connect_timeout = Some(cli.request_timeout);
        config.read_timeout = Some(cli.request_timeout);
        // One adapter call is one wire attempt. kube's default retry layer
        // would hide 429/503/504 retries from our shared budget and counters.
        config.default_retry = false;
        // Resolve the Kubernetes exec-plugin stdin contract before delegating
        // execution/parsing to kube. IfAvailable must not inherit a terminal or
        // raw helper stderr when this process has no interactive stdin.
        if let Some(exec) = &mut config.auth_info.exec {
            if exec.api_version.as_deref() == Some("client.authentication.k8s.io/v1")
                && exec.interactive_mode.is_none()
            {
                return Err("Credential-helper configuration requires interactiveMode for ExecCredential v1; check kubeconfig.".into());
            }
            if !crate::exec_supervisor::interactive_stdin() {
                if matches!(exec.interactive_mode, Some(ExecInteractiveMode::Always)) {
                    return Err("Credential helper requires terminal input; authenticate in a terminal or select noninteractive credentials.".into());
                }
                exec.interactive_mode = Some(ExecInteractiveMode::Never);
            }
        }
        if (config.auth_info.exec.is_some() || config.auth_info.auth_provider.is_some())
            && crate::exec_supervisor::worker_parent().is_none()
        {
            return Ok(Loaded::IsolateCredentials);
        }
        let client=Client::try_from(config).map_err(|_|"Cannot create Kubernetes client; check TLS, proxy, and credential-helper configuration.".to_owned())?;
        Ok(Loaded::Direct(
            Self {
                client,
                received: AtomicUsize::new(0),
            },
            namespace,
            context,
        ))
    }
}

impl Transport for Kubernetes {
    fn get(&self, path: String) -> ResponseFuture<'_> {
        Box::pin(async move {
            let request = http::Request::get(path)
                .header(http::header::ACCEPT, "application/json")
                .header(
                    http::header::USER_AGENT,
                    concat!("kubectl-view-storage/", env!("CARGO_PKG_VERSION")),
                )
                .body(kube::client::Body::from(Vec::new()))
                .map_err(|_| Failure::Invalid)?;
            let response = self.client.send(request).await.map_err(classify)?;
            let status = response.status().as_u16();
            let retry_after = response
                .headers()
                .get(http::header::RETRY_AFTER)
                .and_then(|h| h.to_str().ok())
                .and_then(retry_after);
            // Authentication is fatal even when its body exceeds the cap, stalls,
            // or fails in transit. Do not let a body-read failure mask the status.
            // Unread payload bytes are deliberately absent from consumed-byte cost.
            if status == 401 {
                return Ok(Response {
                    status,
                    body: Vec::new(),
                    retry_after,
                });
            }
            // Count bounded frames for other statuses. Error payloads may contain
            // unrelated metadata, so discard their bytes without decoding,
            // retaining or logging them. The outer request deadline covers reads.
            let successful = (200..300).contains(&status);
            let mut stream = response.into_body();
            let mut body = Vec::new();
            let mut observed = 0;
            while let Some(frame) = stream.frame().await {
                let frame = frame.map_err(|_| Failure::Transport)?;
                if let Some(data) = frame.data_ref() {
                    self.received.fetch_add(data.len(), Ordering::Relaxed);
                    if data.len() > MAX_RESPONSE_BYTES - observed {
                        return Err(Failure::TooLarge);
                    }
                    observed += data.len();
                    if successful {
                        body.extend_from_slice(data);
                    }
                }
            }
            Ok(Response {
                status,
                body,
                retry_after,
            })
        })
    }
    fn response_bytes(&self) -> Option<usize> {
        Some(self.received.load(Ordering::Relaxed))
    }
}

fn classify(error: kube::Error) -> Failure {
    let mut source: Option<&(dyn std::error::Error + 'static)> = Some(&error);
    while let Some(error) = source {
        if matches!(
            error.downcast_ref::<kube::Error>(),
            Some(kube::Error::Auth(_))
        ) || error.is::<kube::client::AuthError>()
        {
            return Failure::Auth;
        }
        source = error.source();
    }
    Failure::Transport
}

fn retry_after(value: &str) -> Option<Duration> {
    if let Ok(seconds) = value.parse::<u64>() {
        return Some(Duration::from_secs(seconds));
    }
    // HTTP's IMF-fixdate is also accepted by Jiff's RFC 2822 parser.
    let time = jiff::fmt::rfc2822::DateTimeParser::new()
        .parse_timestamp(value)
        .ok()?;
    let nanos = time.as_nanosecond() - jiff::Timestamp::now().as_nanosecond();
    if nanos <= 0 {
        Some(Duration::ZERO)
    } else {
        u64::try_from(nanos).ok().map(Duration::from_nanos)
    }
}
