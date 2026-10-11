// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

mod model;
#[cfg(all(test, unix))]
mod pty;
mod terminal;

use std::io::IsTerminal;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::thread::JoinHandle;
use std::time::Instant;

use crossbeam_channel::{Sender, bounded};
use napi_derive::napi;
use tokio::sync::mpsc;

use crate::application::{Application, BluinkApplication};
use crate::diagnostics::DiagnosticEvent;

static SESSION: Mutex<Option<Weak<Core>>> = Mutex::new(None);

#[napi(object)]
pub struct ConsoleOptions {
    pub cli_version: String,
    pub node_version: String,
    pub mode: Option<String>,
}

#[napi(object)]
pub struct ConsoleAction {
    pub kind: String,
    pub url: Option<String>,
}

struct Core {
    options: ConsoleOptions,
    events: Sender<DiagnosticEvent>,
    application: Mutex<Option<Weak<Application>>>,
    address: Mutex<Option<String>>,
    actions: Mutex<Option<mpsc::Sender<ConsoleAction>>>,
    stop_requested: AtomicBool,
    shutdown_requested: AtomicBool,
    browser_pending: AtomicBool,
    errors: AtomicU64,
    warnings: AtomicU64,
    dropped: AtomicU64,
    rss: AtomicU64,
    started: Instant,
    worker: Mutex<Option<JoinHandle<std::io::Result<()>>>>,
    failure: Mutex<Option<String>>,
}

fn session() -> Option<Arc<Core>> {
    SESSION
        .lock()
        .ok()?
        .as_ref()?
        .upgrade()
        .filter(|core| !core.stop_requested.load(Ordering::Acquire))
}

pub(crate) fn admitted(event: &DiagnosticEvent) {
    if let Some(core) = session() {
        core.admit(event);
    }
}

impl Core {
    fn admit(&self, event: &DiagnosticEvent) {
        match event.level.as_str() {
            "error" => {
                self.errors.fetch_add(1, Ordering::Relaxed);
            }
            "warn" => {
                self.warnings.fetch_add(1, Ordering::Relaxed);
            }
            _ => {}
        }
    }
}

pub(crate) fn dropped() {
    if let Some(core) = session() {
        core.dropped.fetch_add(1, Ordering::Relaxed);
    }
}

/// Returning true suppresses direct stderr writes while a renderer owns it.
pub(crate) fn deliver(event: &DiagnosticEvent) -> bool {
    let Some(core) = session() else {
        return false;
    };
    core.deliver(event);
    true
}

pub(crate) fn http_listening(application: &Arc<Application>, url: &str) {
    if let Some(core) = session()
        && core
            .application
            .lock()
            .ok()
            .and_then(|current| current.as_ref().and_then(Weak::upgrade))
            .is_some_and(|current| Arc::ptr_eq(&current, application))
        && let Ok(mut address) = core.address.lock()
    {
        *address = Some(url.to_owned());
    }
}

impl Core {
    fn deliver(&self, event: &DiagnosticEvent) {
        if self.events.try_send(event.clone()).is_err() {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn status(&self) -> model::Status {
        let application = self
            .application
            .lock()
            .ok()
            .and_then(|current| current.as_ref().and_then(Weak::upgrade));
        let mut status = application.map_or_else(model::Status::default, |application| {
            application.status().map_or_else(
                |_| model::Status {
                    failed: true,
                    phase: "Status unavailable".to_owned(),
                    ..Default::default()
                },
                |status| {
                    model::Status::from_value(
                        application.project_root().display().to_string(),
                        &status,
                    )
                },
            )
        });
        status.stopping |= self.shutdown_requested.load(Ordering::Acquire);
        status
    }

    fn address(&self) -> Option<String> {
        self.address.lock().ok().and_then(|address| address.clone())
    }

    fn action(&self, kind: &str) {
        let url = if kind == "openBrowser" {
            let Some(url) = self.address() else {
                return;
            };
            if self.browser_pending.swap(true, Ordering::AcqRel) {
                return;
            }
            Some(url)
        } else {
            if self.shutdown_requested.swap(true, Ordering::AcqRel) {
                return;
            }
            None
        };
        if let Ok(actions) = self.actions.lock()
            && let Some(actions) = &*actions
            && actions
                .try_send(ConsoleAction {
                    kind: kind.to_owned(),
                    url,
                })
                .is_err()
        {
            if kind == "shutdown" {
                self.shutdown_requested.store(false, Ordering::Release);
            } else {
                self.browser_pending.store(false, Ordering::Release);
            }
        }
    }
}

/// One process-wide terminal owner. Backend events never pass through JavaScript.
#[napi]
pub struct BluinkConsole {
    core: Arc<Core>,
    actions: tokio::sync::Mutex<mpsc::Receiver<ConsoleAction>>,
    stop: tokio::sync::Mutex<()>,
}

#[napi]
impl BluinkConsole {
    #[napi(constructor)]
    pub fn new(options: ConsoleOptions) -> napi::Result<Self> {
        if !matches!(options.mode.as_deref(), None | Some("auto" | "plain")) {
            return Err(crate::error::native_error(
                "Console mode must be auto or plain.",
            ));
        }
        let (events, incoming) = bounded(1_024);
        let (actions, outgoing) = mpsc::channel(32);
        let core = Arc::new(Core {
            options,
            events,
            application: Mutex::new(None),
            address: Mutex::new(None),
            actions: Mutex::new(Some(actions)),
            stop_requested: AtomicBool::new(false),
            shutdown_requested: AtomicBool::new(false),
            browser_pending: AtomicBool::new(false),
            errors: AtomicU64::new(0),
            warnings: AtomicU64::new(0),
            dropped: AtomicU64::new(0),
            rss: AtomicU64::new(0),
            started: Instant::now(),
            worker: Mutex::new(None),
            failure: Mutex::new(None),
        });
        let mut current = SESSION.lock().map_err(crate::error::native_error)?;
        if current.as_ref().and_then(Weak::upgrade).is_some() {
            return Err(crate::error::native_error(
                "A console session is already active.",
            ));
        }
        let owner = Arc::clone(&core);
        let worker = std::thread::Builder::new()
            .name("bluink-console".to_owned())
            .spawn(move || {
                let result = terminal::run(&owner, &incoming);
                owner.stop_requested.store(true, Ordering::Release);
                if let Ok(mut actions) = owner.actions.lock() {
                    actions.take();
                }
                result
            })
            .map_err(crate::error::native_error)?;
        *core.worker.lock().map_err(crate::error::native_error)? = Some(worker);
        *current = Some(Arc::downgrade(&core));
        drop(current);
        Ok(Self {
            core,
            actions: tokio::sync::Mutex::new(outgoing),
            stop: tokio::sync::Mutex::new(()),
        })
    }

    #[napi]
    pub fn attach_application(&self, application: &BluinkApplication) -> napi::Result<()> {
        *self
            .core
            .application
            .lock()
            .map_err(crate::error::native_error)? = Some(Arc::downgrade(&application.inner));
        Ok(())
    }

    #[napi]
    pub fn update_rss(&self, rss_bytes: Option<f64>) -> napi::Result<()> {
        let rss_bytes = rss_bytes.unwrap_or(0.0);
        if !rss_bytes.is_finite()
            || rss_bytes < 0.0
            || rss_bytes.fract() != 0.0
            || rss_bytes > 9_007_199_254_740_991.0
        {
            return Err(crate::error::native_error(
                "RSS must be a nonnegative safe integer in bytes.",
            ));
        }
        #[allow(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "Validated JavaScript safe integer fits u64."
        )]
        self.core.rss.store(rss_bytes as u64, Ordering::Relaxed);
        Ok(())
    }

    #[napi]
    pub async fn next_action(&self) -> Option<ConsoleAction> {
        let action = self.actions.lock().await.recv().await;
        if action
            .as_ref()
            .is_some_and(|action| action.kind == "openBrowser")
        {
            self.core.browser_pending.store(false, Ordering::Release);
        }
        action
    }

    #[napi]
    pub async fn stop(&self) -> napi::Result<()> {
        let _stop = self.stop.lock().await;
        self.core.stop_requested.store(true, Ordering::Release);
        let worker = self
            .core
            .worker
            .lock()
            .map_err(crate::error::native_error)?
            .take();
        if let Some(worker) = worker {
            let result = tokio::task::spawn_blocking(move || {
                worker
                    .join()
                    .map_err(|_| "Console worker panicked".to_owned())
                    .and_then(|result| result.map_err(|error| error.to_string()))
            })
            .await
            .map_err(|error| error.to_string())
            .and_then(std::convert::identity);
            if let Err(error) = result {
                *self
                    .core
                    .failure
                    .lock()
                    .map_err(crate::error::native_error)? = Some(error);
            }
        }
        let mut current = SESSION.lock().map_err(crate::error::native_error)?;
        if current
            .as_ref()
            .and_then(Weak::upgrade)
            .is_some_and(|core| Arc::ptr_eq(&core, &self.core))
        {
            current.take();
        }
        drop(current);
        self.core
            .failure
            .lock()
            .map_err(crate::error::native_error)?
            .clone()
            .map_or(Ok(()), |error| Err(crate::error::native_error(error)))
    }
}

impl Drop for BluinkConsole {
    fn drop(&mut self) {
        self.core.stop_requested.store(true, Ordering::Release);
    }
}

fn interactive(options: &ConsoleOptions) -> bool {
    options.mode.as_deref() != Some("plain")
        && std::io::stdin().is_terminal()
        && std::io::stderr().is_terminal()
        && std::io::stdout().is_terminal()
        && !std::env::var("CI")
            .is_ok_and(|value| !value.is_empty() && !matches!(value.as_str(), "0" | "false"))
        && std::env::var("TERM").as_deref() != Ok("dumb")
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(super) fn fixture() -> (Arc<Core>, mpsc::Receiver<ConsoleAction>) {
        let (events, _) = bounded(1);
        let (actions, outgoing) = mpsc::channel(4);
        (
            Arc::new(Core {
                options: ConsoleOptions {
                    cli_version: "26.10.1-alpha".to_owned(),
                    node_version: "26.9.0".to_owned(),
                    mode: Some("plain".to_owned()),
                },
                events,
                application: Mutex::new(None),
                address: Mutex::new(None),
                actions: Mutex::new(Some(actions)),
                stop_requested: AtomicBool::new(false),
                shutdown_requested: AtomicBool::new(false),
                browser_pending: AtomicBool::new(false),
                errors: AtomicU64::new(0),
                warnings: AtomicU64::new(0),
                dropped: AtomicU64::new(0),
                rss: AtomicU64::new(184 * 1_048_576),
                started: Instant::now(),
                worker: Mutex::new(None),
                failure: Mutex::new(None),
            }),
            outgoing,
        )
    }

    #[test]
    fn host_actions_require_a_ready_address_and_coalesce_shutdown() {
        let (core, mut actions) = fixture();
        core.action("openBrowser");
        assert!(actions.try_recv().is_err());
        *core.address.lock().unwrap() = Some("http://127.0.0.1:1234".to_owned());
        for _ in 0..100 {
            core.action("openBrowser");
        }
        assert_eq!(
            actions.try_recv().unwrap().url.as_deref(),
            Some("http://127.0.0.1:1234")
        );
        core.action("shutdown");
        core.action("shutdown");
        assert_eq!(actions.try_recv().unwrap().kind, "shutdown");
        assert!(actions.try_recv().is_err());
    }

    #[test]
    fn counts_precede_bounded_delivery_and_survive_history_eviction() {
        let (mut core, _) = fixture();
        let (events, incoming) = bounded(1);
        Arc::get_mut(&mut core).unwrap().events = events;
        let mut history = model::History::default();
        for _ in 0..1_100 {
            let event = DiagnosticEvent {
                timestamp_ms: 1,
                level: "error".to_owned(),
                target: "bluink.fixture".to_owned(),
                name: None,
                message: None,
                fields: std::collections::BTreeMap::new(),
                spans: Vec::new(),
                duration_ms: None,
            };
            core.admit(&event);
            core.deliver(&event);
            history.push(&event);
        }
        assert_eq!(core.errors.load(Ordering::Relaxed), 1_100);
        assert_eq!(core.dropped.load(Ordering::Relaxed), 1_099);
        assert_eq!(incoming.len(), 1);
        assert_eq!(history.evicted, 100);
    }

    #[test]
    fn rss_accepts_only_safe_nonnegative_bytes_and_allows_unavailable() {
        let (core, actions) = fixture();
        let console = BluinkConsole {
            core,
            actions: tokio::sync::Mutex::new(actions),
            stop: tokio::sync::Mutex::new(()),
        };
        for invalid in [f64::NAN, f64::INFINITY, -1.0, 1.5, 9_007_199_254_740_992.0] {
            assert!(console.update_rss(Some(invalid)).is_err());
        }
        console.update_rss(Some(1_048_576.0)).unwrap();
        assert_eq!(console.core.rss.load(Ordering::Relaxed), 1_048_576);
        console.update_rss(None).unwrap();
        assert_eq!(console.core.rss.load(Ordering::Relaxed), 0);
    }
}
