#![forbid(unsafe_code)]
mod csv_output;
mod exec_supervisor;
mod kubernetes;
mod stdio;

use clap::Parser;
use kubectl_view_storage::{
    cli::{Cli, Output},
    clock::SystemClock,
    collect::{self, Failure},
    output, pipeline,
    policy::Exit,
};
use std::{
    io::{self, IsTerminal},
    process::ExitCode,
    sync::Arc,
};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    let isolated = exec_supervisor::worker_parent().is_some();
    let exit = run(cli).await;
    if isolated {
        // Owned collection/CPU/output tasks have joined and stdio is flushed.
        // An SDK-internal blocking credential refresh can outlive its cancelled
        // request future. The parent owns termination/reaping of this process
        // and its helpers, so do not wait indefinitely in Runtime::drop.
        std::process::exit(i32::from(exit as u8));
    }
    ExitCode::from(exit as u8)
}

async fn run(cli: Cli) -> Exit {
    let cancel = CancellationToken::new();
    let _cancel_on_drop = cancel.clone().drop_guard();
    let relay = exec_supervisor::worker_parent().map(exec_supervisor::relay_interrupt);
    let operation = run_command(cli, cancel.clone());
    tokio::pin!(operation);
    let exit = tokio::select! {
        biased;
        signal = tokio::signal::ctrl_c() => {
            cancel.cancel();
            // Collection, report processing and the output worker all own
            // cooperative work. Always join them before returning to the shell.
            let _ = operation.await;
            if signal.is_err() { Exit::Fatal } else { Exit::Interrupted }
        }
        result = &mut operation => result,
    };
    if let Some(relay) = relay {
        relay.abort();
        let _ = relay.await;
    }
    exit
}

async fn run_command(mut cli: Cli, cancel: CancellationToken) -> Exit {
    if exec_supervisor::worker_parent().is_some()
        && let Err(message) = exec_supervisor::wait_ready(&cancel).await
    {
        if cancel.is_cancelled() {
            return Exit::Interrupted;
        }
        let _ = error_message(cancel, message).await;
        return Exit::Fatal;
    }
    if exec_supervisor::worker_parent().is_some() {
        match exec_supervisor::remaining_budget() {
            Ok(remaining) => cli.timeout = cli.timeout.min(remaining),
            Err(_) => {
                let _ =
                    error_message(cancel, "Cannot read credential-worker collection budget").await;
                return Exit::Fatal;
            }
        }
    }
    if let Err(message) = cli.validate() {
        let _ = error_message(cancel, &message).await;
        return Exit::Invalid;
    }
    let baseline = if let Some(path) = cli.compare.clone() {
        match tokio::task::spawn_blocking(move || {
            kubectl_view_storage::calculations::Baseline::load(&path)
        })
        .await
        {
            Ok(Ok(value)) => Some(Arc::new(value)),
            Ok(Err(message)) => {
                let _ = error_message(cancel, &message).await;
                return Exit::Invalid;
            }
            Err(_) => return Exit::Fatal,
        }
    } else {
        None
    };
    let started = Instant::now();
    let configuration = tokio::select! {
        biased;
        _ = cancel.cancelled() => return Exit::Interrupted,
        result = tokio::time::timeout(cli.timeout, kubernetes::Kubernetes::load(&cli)) => result,
    };
    let (client, namespace, context) = match configuration {
        Ok(Ok(kubernetes::Loaded::Direct(client, namespace, context))) => {
            (client, namespace, context)
        }
        Ok(Ok(kubernetes::Loaded::IsolateCredentials)) => {
            let remaining = cli.timeout.saturating_sub(started.elapsed());
            return match exec_supervisor::supervise(remaining, cancel.clone()).await {
                Ok(exit) => exit,
                Err(message) => {
                    let _ = error_message(cancel, &message).await;
                    Exit::Fatal
                }
            };
        }
        Ok(Err(message)) => {
            let _ = error_message(cancel, &message).await;
            return Exit::Fatal;
        }
        Err(_) => {
            let _ = error_message(cancel, "Kubernetes configuration deadline exceeded").await;
            return Exit::Fatal;
        }
    };
    if exec_supervisor::worker_parent().is_some()
        && exec_supervisor::configuration_complete().is_err()
    {
        let _ = error_message(
            cancel,
            "Cannot signal credential-worker configuration readiness",
        )
        .await;
        return Exit::Fatal;
    }
    cli.timeout = cli.timeout.saturating_sub(started.elapsed());
    if cli.timeout.is_zero() {
        let _ = error_message(cancel, "collection deadline exceeded before inventory").await;
        return Exit::Fatal;
    }
    let mut query = match cli.query(&namespace) {
        Ok(query) => query,
        Err(message) => {
            let _ = error_message(cancel, &message).await;
            return Exit::Invalid;
        }
    };
    query.baseline = baseline;
    let collected = match collect::collect(
        Arc::new(client),
        Arc::new(SystemClock),
        cancel.clone(),
        &cli,
        &query,
        context,
    )
    .await
    {
        Ok(collected) => collected,
        Err(Failure::Cancelled) => return Exit::Interrupted,
        Err(error) => {
            let _ = error_message(cancel, &format!("required Kubernetes collection failed ({error}); check credentials, permissions, API availability, and timeouts")).await;
            return Exit::Fatal;
        }
    };
    let report = match pipeline::finish_cancellable(collected, &query, cancel.clone()).await {
        Ok(report) => report,
        Err(pipeline::FinishError::Interrupted) => return Exit::Interrupted,
        Err(error) => {
            let _ = error_message(cancel, &format!("cannot build report: {error}")).await;
            return Exit::Fatal;
        }
    };
    let worker_cancel = cancel.clone();
    let result =
        tokio::task::spawn_blocking(move || write_report(report, cli, worker_cancel)).await;
    match result {
        Ok(Ok(exit)) => exit,
        _ if cancel.is_cancelled() => Exit::Interrupted,
        Ok(Err(error)) => {
            if error.kind() != io::ErrorKind::BrokenPipe {
                let _ =
                    error_message(cancel, &format!("cannot write report ({})", error.kind())).await;
            }
            Exit::Fatal
        }
        Err(_) => {
            let _ = error_message(cancel, "output worker failed").await;
            Exit::Fatal
        }
    }
}

fn write_report(report: pipeline::Report, cli: Cli, cancel: CancellationToken) -> io::Result<Exit> {
    let stdout = io::stdout();
    let color = output::use_color(
        cli.color,
        stdout.is_terminal(),
        std::env::var_os("NO_COLOR").is_some(),
    );
    stdio::with_writer(stdout, cancel.clone(), |writer| match cli.output {
        Output::Json => output::write_json(writer, &report.document),
        Output::Csv => csv_output::write(writer, &report.document, &cancel),
        Output::Text => output::write_text(writer, &report, cli.view, cli.wide, color, &cancel),
    })?;
    let has_diagnostics = report.document["diagnostics"]
        .as_array()
        .is_some_and(|items| !items.is_empty());
    if cli.output != Output::Text
        && (report.document["selection"]["complete"] == false || has_diagnostics)
    {
        stdio::with_writer(io::stderr(), cancel, |writer| {
            if report.document["selection"]["complete"] == false {
                writeln!(
                    writer,
                    "selection incomplete: query membership is unresolved; totals cover confirmed matches only"
                )?;
            }
            if let Some(diagnostics) = report.document["diagnostics"].as_array() {
                for diagnostic in diagnostics {
                    writeln!(
                        writer,
                        "diagnostic {}: {}",
                        output::escape(diagnostic["reason"].as_str().unwrap_or("unknown")),
                        output::escape(
                            diagnostic["message"]
                                .as_str()
                                .unwrap_or("collection incomplete")
                        )
                    )?;
                }
            }
            Ok(())
        })?;
    }
    Ok(report.exit)
}

async fn error_message(cancel: CancellationToken, message: &str) -> io::Result<()> {
    let message = format!("error: {}\n", output::escape(message));
    tokio::task::spawn_blocking(move || {
        stdio::with_writer(io::stderr(), cancel, |writer| {
            writer.write_all(message.as_bytes())
        })
    })
    .await
    .map_err(|_| io::Error::other("diagnostic worker failed"))?
}
