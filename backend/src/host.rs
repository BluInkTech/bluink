// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! The process host: arguments, installed resources, signals, the server
//! console and orderly shutdown. Application behaviour stays in `application`.

mod cli;
mod commands;
mod memory;
mod output;
mod resources;
mod server;
pub(crate) mod session;

use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use tokio::sync::Notify;

/// One shutdown request per process; repeats are coalesced.
#[derive(Default)]
struct Lifecycle {
    requested: AtomicBool,
    notify: Notify,
}

impl Lifecycle {
    /// Returns true for the first request only.
    fn request(&self, announce: bool) -> bool {
        if self.requested.swap(true, Ordering::AcqRel) {
            return false;
        }
        if announce {
            if output::console_active() {
                tracing::info!(target: "bluink.host", event = "shutdown_requested");
            } else {
                output::notice("Performing cleanup");
            }
        }
        self.notify.notify_waiters();
        true
    }

    fn stopping(&self) -> bool {
        self.requested.load(Ordering::Acquire)
    }

    async fn requested(&self) {
        let notified = self.notify.notified();
        if self.stopping() {
            return;
        }
        notified.await;
    }
}

/// Ctrl+C everywhere; SIGTERM and SIGHUP on Unix; Ctrl+Break and console close
/// on Windows. Each announces and requests the same graceful stop.
fn watch_signals(runtime: &tokio::runtime::Runtime, lifecycle: &Arc<Lifecycle>) {
    let watch = |lifecycle: Arc<Lifecycle>, signal: std::io::Result<()>| {
        if signal.is_ok() {
            lifecycle.request(true);
        }
    };
    let owner = Arc::clone(lifecycle);
    runtime.spawn(async move {
        loop {
            let signal = tokio::signal::ctrl_c().await;
            let failed = signal.is_err();
            watch(Arc::clone(&owner), signal);
            if failed {
                return;
            }
        }
    });
    #[cfg(unix)]
    for kind in [
        tokio::signal::unix::SignalKind::terminate(),
        tokio::signal::unix::SignalKind::hangup(),
    ] {
        let owner = Arc::clone(lifecycle);
        runtime.spawn(async move {
            let Ok(mut stream) = tokio::signal::unix::signal(kind) else {
                return;
            };
            while stream.recv().await.is_some() {
                owner.request(true);
            }
        });
    }
    #[cfg(windows)]
    {
        let owner = Arc::clone(lifecycle);
        runtime.spawn(async move {
            let Ok(mut stream) = tokio::signal::windows::ctrl_break() else {
                return;
            };
            while stream.recv().await.is_some() {
                owner.request(true);
            }
        });
        let owner = Arc::clone(lifecycle);
        runtime.spawn(async move {
            let Ok(mut stream) = tokio::signal::windows::ctrl_close() else {
                return;
            };
            while stream.recv().await.is_some() {
                owner.request(true);
            }
        });
    }
}

// HTTP polling uses a small I/O pool. SQLite writes use a dedicated thread
// and CPU-bound work uses Rayon. Bound blocking work so request bursts cannot create
// hundreds of threads and their allocator arenas. HTTP requests block a pool
// thread while they wait for a reader or the writer queue, so the pool covers
// every admitted request plus long-running sync, console and preview work;
// idle threads exit after Tokio's keep-alive.
fn runtime() -> std::io::Result<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .max_blocking_threads(crate::http::BLOCKING_CAPACITY + 4)
        .enable_all()
        .build()
}

pub(crate) fn run() -> ExitCode {
    let Some(arguments) = std::env::args_os()
        .skip(1)
        .map(std::ffi::OsString::into_string)
        .collect::<Result<Vec<_>, _>>()
        .ok()
    else {
        output::error("Arguments must be valid Unicode.");
        return ExitCode::FAILURE;
    };
    let invocation = match cli::parse(&arguments) {
        Ok(invocation) => invocation,
        Err(message) => {
            output::error(&message);
            if message.starts_with("Unknown command") {
                output::line("");
                cli::overall_help(&resources::version());
            }
            return ExitCode::FAILURE;
        }
    };
    let (command, values, positionals) = match invocation {
        cli::Invocation::Version => {
            output::line(&format!("@bluink/cli v{}", resources::version()));
            return ExitCode::SUCCESS;
        }
        cli::Invocation::Help(None) => {
            cli::overall_help(&resources::version());
            return ExitCode::SUCCESS;
        }
        cli::Invocation::Missing => {
            cli::overall_help(&resources::version());
            return ExitCode::FAILURE;
        }
        cli::Invocation::Help(Some(command)) => {
            cli::command_help(command, &resources::version());
            return ExitCode::SUCCESS;
        }
        cli::Invocation::Run {
            command,
            values,
            positionals,
        } => (command, values, positionals),
    };
    if command.name == "build-info" {
        output::line(resources::build_identity());
        return ExitCode::SUCCESS;
    }
    if let Err(error) = crate::diagnostics::configure_native_diagnostics() {
        output::notice(&format!("Native diagnostics are unavailable: {error}"));
    }
    // Before any repository opens; synchronization reports a failure itself.
    if let Err(error) = crate::versioning::transport::register() {
        output::notice(&format!("Remote synchronization is unavailable: {error}"));
    }
    let runtime = match runtime() {
        Ok(runtime) => runtime,
        Err(error) => {
            output::error(&format!("Cannot start Bluink's runtime: {error}"));
            return ExitCode::FAILURE;
        }
    };
    let lifecycle = Arc::new(Lifecycle::default());
    watch_signals(&runtime, &lifecycle);
    let outcome = if command.name == "server" {
        server::run(&runtime, &lifecycle, &values)
    } else {
        commands::run(&runtime, &lifecycle, command.name, values, positionals)
    };
    let drained = crate::diagnostics::shutdown_native_diagnostics();
    let finished = outcome.finish(&runtime);
    drop(runtime);
    let mut code = ExitCode::SUCCESS;
    if let Some(error) = finished.error {
        let interrupted = lifecycle.stopping()
            && matches!(
                error.downcast_ref::<crate::application::error::ApplicationError>(),
                Some(crate::application::error::ApplicationError::ShuttingDown)
            );
        if !interrupted {
            output::error(&format!("{error:#}"));
            code = ExitCode::FAILURE;
        }
    }
    if finished.cleanup_failed || drained.is_err() {
        output::notice("Backend cleanup failed");
        code = ExitCode::FAILURE;
    }
    code
}

/// A command's result plus the console, which must stop only after the
/// diagnostic sink has drained so its final events still render.
struct Outcome {
    error: Option<anyhow::Error>,
    cleanup_failed: bool,
    console: Option<server::Console>,
}

struct Finished {
    error: Option<anyhow::Error>,
    cleanup_failed: bool,
}

impl Outcome {
    const fn done(error: Option<anyhow::Error>, cleanup_failed: bool) -> Self {
        Self {
            error,
            cleanup_failed,
            console: None,
        }
    }

    fn finish(self, runtime: &tokio::runtime::Runtime) -> Finished {
        let console_failed = self
            .console
            .is_some_and(|console| console.stop(runtime).is_err());
        Finished {
            error: self.error,
            cleanup_failed: self.cleanup_failed || console_failed,
        }
    }
}
