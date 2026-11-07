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

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

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

const SUCCESS: u8 = 0;
const FAILURE: u8 = 1;

/// How a host process such as Node starts Bluink in-process.
pub struct Launch {
    /// Command-line arguments, without the program name.
    pub arguments: Vec<String>,
    /// The installed `bluink` package. Inside a host the executable path
    /// names the host, so the loader supplies the package it was loaded from.
    pub package_root: Option<PathBuf>,
    /// The script the host ran to load Bluink. A PDF renderer child runs the
    /// host with it again, so the child loads Bluink the same way.
    pub launcher: Option<PathBuf>,
}

/// Who owns process signals: Bluink itself, or the host that loaded it.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Signals {
    Process,
    Host,
}

/// The running embedded instance, for [`request_shutdown`]. A request that
/// arrives before it starts is kept in `EARLY_SHUTDOWN` and applied on start.
static EMBEDDED: Mutex<Option<Arc<Lifecycle>>> = Mutex::new(None);
static EARLY_SHUTDOWN: AtomicBool = AtomicBool::new(false);

/// The executable: arguments from the process, signals handled by Bluink.
pub fn run() -> ExitCode {
    let Some(arguments) = std::env::args_os()
        .skip(1)
        .map(std::ffi::OsString::into_string)
        .collect::<Result<Vec<_>, _>>()
        .ok()
    else {
        output::error("Arguments must be valid Unicode.");
        return ExitCode::FAILURE;
    };
    ExitCode::from(start(&arguments, Signals::Process))
}

/// Run inside a host process that owns signals and forwards a stop through
/// [`request_shutdown`]. Blocks until Bluink finishes; returns the exit code.
#[must_use]
pub fn run_embedded(launch: Launch) -> u8 {
    if let Some(root) = launch.package_root {
        resources::use_package_root(root);
    }
    if let Some(launcher) = launch.launcher {
        resources::use_launcher(launcher);
    }
    start(&launch.arguments, Signals::Host)
}

/// Ask the embedded instance to stop gracefully, exactly as Ctrl+C would.
pub fn request_shutdown() {
    let running = EMBEDDED.lock().ok().and_then(|instance| instance.clone());
    match running {
        Some(lifecycle) => {
            lifecycle.request(true);
        }
        None => EARLY_SHUTDOWN.store(true, Ordering::Release),
    }
}

fn start(arguments: &[String], signals: Signals) -> u8 {
    if let Some((command, rest)) = arguments.split_first()
        && command == crate::export_jobs::pdf::RENDER_COMMAND
    {
        return render_pdf(rest);
    }
    let invocation = match cli::parse(arguments) {
        Ok(invocation) => invocation,
        Err(message) => {
            output::error(&message);
            if message.starts_with("Unknown command") {
                output::line("");
                cli::overall_help(&resources::version());
            }
            return FAILURE;
        }
    };
    let (command, values, positionals) = match invocation {
        cli::Invocation::Version => {
            output::line(&format!("bluink v{}", resources::version()));
            return SUCCESS;
        }
        cli::Invocation::Help(None) => {
            cli::overall_help(&resources::version());
            return SUCCESS;
        }
        cli::Invocation::Missing => {
            cli::overall_help(&resources::version());
            return FAILURE;
        }
        cli::Invocation::Help(Some(command)) => {
            cli::command_help(command, &resources::version());
            return SUCCESS;
        }
        cli::Invocation::Run {
            command,
            values,
            positionals,
        } => (command, values, positionals),
    };
    if command.name == "build-info" {
        output::line(resources::build_identity());
        return SUCCESS;
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
            return FAILURE;
        }
    };
    let lifecycle = Arc::new(Lifecycle::default());
    match signals {
        Signals::Process => watch_signals(&runtime, &lifecycle),
        Signals::Host => {
            if let Ok(mut instance) = EMBEDDED.lock() {
                *instance = Some(Arc::clone(&lifecycle));
            }
            if EARLY_SHUTDOWN.swap(false, Ordering::AcqRel) {
                lifecycle.request(true);
            }
        }
    }
    let outcome = if command.name == "server" {
        server::run(&runtime, &lifecycle, &values)
    } else {
        commands::run(&runtime, &lifecycle, command.name, values, positionals)
    };
    let drained = crate::diagnostics::shutdown_native_diagnostics();
    let finished = outcome.finish(&runtime);
    drop(runtime);
    if signals == Signals::Host
        && let Ok(mut instance) = EMBEDDED.lock()
    {
        *instance = None;
    }
    let mut code = SUCCESS;
    if let Some(error) = finished.error {
        let interrupted = lifecycle.stopping()
            && matches!(
                error.downcast_ref::<crate::application::error::ApplicationError>(),
                Some(crate::application::error::ApplicationError::ShuttingDown)
            );
        if !interrupted {
            output::error(&format!("{error:#}"));
            code = FAILURE;
        }
    }
    if finished.cleanup_failed || drained.is_err() {
        output::notice("Backend cleanup failed");
        code = FAILURE;
    }
    code
}

/// A PDF renderer child, started by an export. Typst recurses deeply on long
/// documents, so it runs on a thread with a large stack. Its diagnostics go
/// to stderr unadorned, for the parent to map back to the source page.
fn render_pdf(arguments: &[String]) -> u8 {
    let arguments = arguments.to_vec();
    let worker = std::thread::Builder::new()
        .name("typst".to_owned())
        .stack_size(256 * 1024 * 1024)
        .spawn(move || crate::export_jobs::pdf::render_child(&arguments));
    match worker.map(std::thread::JoinHandle::join) {
        Ok(Ok(Ok(()))) => SUCCESS,
        Ok(Ok(Err(diagnostics))) => {
            output::notice(diagnostics.trim_end());
            FAILURE
        }
        Ok(Err(_)) => {
            output::notice("error: Typst stopped unexpectedly.");
            FAILURE
        }
        Err(error) => {
            output::notice(&format!("error: Cannot start Typst: {error}"));
            FAILURE
        }
    }
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
