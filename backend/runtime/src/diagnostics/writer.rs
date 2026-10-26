// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::{IsTerminal, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use crossbeam_channel::{Receiver, Sender, bounded};
use serde::Serialize;
use serde_json::{Value, json};

use super::{DiagnosticEvent, now_ms};

const QUEUE_CAPACITY: usize = 1024;
const FLUSH_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Statistics {
    dropped_events: u64,
    write_failures: u64,
}

#[derive(Default)]
struct Counters {
    dropped: AtomicU64,
    failures: AtomicU64,
    file_active: AtomicBool,
}

enum Command {
    Event(DiagnosticEvent),
    Flush(Sender<()>),
    Stop(Sender<()>),
}

pub(super) struct Sink {
    sender: Sender<Command>,
    counters: Arc<Counters>,
    worker: Mutex<Option<JoinHandle<()>>>,
    console_level: u8,
    capture_level: u8,
}

fn level(value: &str) -> u8 {
    match value {
        "trace" => 0,
        "debug" => 1,
        "warn" => 3,
        "error" => 4,
        _ => 2,
    }
}

fn thresholds(explicit: Option<&str>, capture: bool) -> (u8, u8) {
    let console = level(explicit.unwrap_or("info"));
    let file = if capture {
        level(explicit.unwrap_or("debug"))
    } else {
        5
    };
    (console, file)
}

fn color_enabled(terminal: bool, no_color: bool, term: Option<&str>) -> bool {
    terminal && !no_color && term != Some("dumb")
}

fn console_line(event: &DiagnosticEvent, color: bool) -> String {
    let operation = event
        .fields
        .get("operation")
        .and_then(Value::as_str)
        .or(event.name.as_deref())
        .unwrap_or("event");
    let message = event
        .message
        .as_ref()
        .map(|value| format!(": {value}"))
        .unwrap_or_default();
    let duration = event
        .duration_ms
        .map(|value| format!(" ({value:.1} ms)"))
        .unwrap_or_default();
    let fields = if event.fields.is_empty() {
        String::new()
    } else {
        format!(
            " {}",
            serde_json::to_string(&event.fields).unwrap_or_default()
        )
    };
    let plain = format!(
        "{} [{}] {operation}{message}{duration}{fields}",
        event.level.to_ascii_uppercase(),
        event.target
    );
    if color {
        let code = match event.level.as_str() {
            "warn" => 33,
            "error" => 31,
            "info" => 36,
            _ => 2,
        };
        format!("\x1b[{code}m{plain}\x1b[0m\n")
    } else {
        format!("{plain}\n")
    }
}

fn file_warning() -> DiagnosticEvent {
    DiagnosticEvent {
        timestamp_ms: now_ms(),
        level: "warn".to_owned(),
        target: "bluink.diagnostics".to_owned(),
        name: Some("optional_file_unavailable".to_owned()),
        message: Some("Optional diagnostics file unavailable".to_owned()),
        fields: BTreeMap::new(),
        spans: Vec::new(),
        duration_ms: None,
    }
}

fn open_file(path: &str) -> std::io::Result<File> {
    let path = Path::new(path);
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent)?;
    }
    OpenOptions::new().create(true).append(true).open(path)
}

impl Sink {
    pub(super) fn from_environment() -> Result<Self, String> {
        let filename = std::env::var("BLUINK_DIAGNOSTICS_FILE")
            .ok()
            .filter(|value| !value.is_empty());
        let explicit_level = std::env::var("BLUINK_LOG_LEVEL").ok();
        let (console_level, capture_level) =
            thresholds(explicit_level.as_deref(), filename.is_some());
        let color = color_enabled(
            std::io::stderr().is_terminal(),
            std::env::var_os("NO_COLOR").is_some(),
            std::env::var("TERM").ok().as_deref(),
        );
        let counters = Arc::new(Counters::default());
        let file = filename.and_then(|filename| {
            open_file(&filename).map_or_else(
                |_| {
                    counters.failures.fetch_add(1, Ordering::Relaxed);
                    let warning = file_warning();
                    crate::console::admitted(&warning);
                    if !crate::console::deliver(&warning) {
                        let _ =
                            std::io::stderr().write_all(console_line(&warning, color).as_bytes());
                    }
                    None
                },
                |file| {
                    Some(Box::new(std::io::BufWriter::with_capacity(64 * 1024, file))
                        as Box<dyn Write + Send>)
                },
            )
        });
        Self::start(
            Box::new(std::io::stderr()),
            file,
            console_level,
            capture_level,
            color,
            counters,
        )
    }

    fn start(
        console: Box<dyn Write + Send>,
        file: Option<Box<dyn Write + Send>>,
        console_level: u8,
        capture_level: u8,
        color: bool,
        counters: Arc<Counters>,
    ) -> Result<Self, String> {
        let (sender, receiver) = bounded(QUEUE_CAPACITY);
        counters
            .file_active
            .store(file.is_some(), Ordering::Release);
        let writer_counters = counters.clone();
        let worker = std::thread::Builder::new()
            .name("bluink-diagnostics".to_owned())
            .spawn(move || {
                Writer {
                    console,
                    file,
                    console_level,
                    capture_level,
                    color,
                    counters: writer_counters,
                    reported_drops: 0,
                }
                .run(receiver);
            })
            .map_err(|error| error.to_string())?;
        Ok(Self {
            sender,
            counters,
            worker: Mutex::new(Some(worker)),
            console_level,
            capture_level,
        })
    }

    pub(super) fn emit(&self, event: DiagnosticEvent) {
        if !self.enabled(&event.level) {
            return;
        }
        let visible = level(&event.level) >= self.console_level;
        if visible {
            crate::console::admitted(&event);
        }
        if self.sender.try_send(Command::Event(event)).is_err() {
            self.counters.dropped.fetch_add(1, Ordering::Relaxed);
            if visible {
                crate::console::dropped();
            }
        }
    }

    pub(super) fn enabled(&self, priority: &str) -> bool {
        let priority = level(priority);
        priority >= self.console_level || priority >= self.capture_level
    }

    pub(super) fn file_active(&self) -> bool {
        self.counters.file_active.load(Ordering::Acquire)
    }

    fn statistics(&self) -> Statistics {
        Statistics {
            dropped_events: self.counters.dropped.load(Ordering::Relaxed),
            write_failures: self.counters.failures.load(Ordering::Relaxed),
        }
    }
    #[expect(
        clippy::significant_drop_tightening,
        reason = "Retain the worker guard until the diagnostic flush or shutdown result is known."
    )]
    pub(super) fn flush(&self) -> Result<Statistics, String> {
        // Serialize flush/stop so a barrier cannot be queued behind Stop.
        let worker = self
            .worker
            .lock()
            .map_err(|_| "Diagnostics worker lock was poisoned".to_owned())?;
        if worker.is_some() {
            let (sender, receiver) = bounded(1);
            self.sender
                .send_timeout(Command::Flush(sender), FLUSH_TIMEOUT)
                .map_err(|_| "Diagnostics flush queue timed out".to_owned())?;
            receiver
                .recv_timeout(FLUSH_TIMEOUT)
                .map_err(|_| "Diagnostics flush timed out".to_owned())?;
        }
        Ok(self.statistics())
    }

    pub(super) fn shutdown(&self) -> Result<Statistics, String> {
        let (mut worker, cleanup) = crate::application::error::recover_for_cleanup(
            self.worker.lock(),
            "Diagnostics worker",
        );
        if worker.is_some() {
            let (sender, receiver) = bounded(1);
            self.sender
                .send_timeout(Command::Stop(sender), FLUSH_TIMEOUT)
                .map_err(|_| "Diagnostics stop queue timed out".to_owned())?;
            receiver
                .recv_timeout(FLUSH_TIMEOUT)
                .map_err(|_| "Diagnostics stop timed out".to_owned())?;
            if let Some(worker) = worker.take() {
                worker
                    .join()
                    .map_err(|_| "Diagnostics writer panicked".to_owned())?;
            }
        }
        drop(worker);
        cleanup.map_err(|error| error.to_string())?;
        Ok(self.statistics())
    }
}

struct Writer {
    console: Box<dyn Write + Send>,
    file: Option<Box<dyn Write + Send>>,
    console_level: u8,
    capture_level: u8,
    color: bool,
    counters: Arc<Counters>,
    reported_drops: u64,
}

impl Writer {
    fn write_internal_event(&mut self, event: &DiagnosticEvent) {
        if level(&event.level) >= self.console_level {
            crate::console::admitted(event);
        }
        self.write_event(event);
    }

    fn write_event(&mut self, event: &DiagnosticEvent) {
        if level(&event.level) >= self.console_level
            && !crate::console::deliver(event)
            && self
                .console
                .write_all(console_line(event, self.color).as_bytes())
                .is_err()
        {
            self.counters.failures.fetch_add(1, Ordering::Relaxed);
        }
        if level(&event.level) >= self.capture_level
            && let Some(file) = &mut self.file
        {
            let result = serde_json::to_writer(&mut *file, event)
                .map_err(std::io::Error::other)
                .and_then(|()| file.write_all(b"\n"));
            if result.is_err() {
                self.file_failed();
            }
        }
    }

    fn file_failed(&mut self) {
        self.counters.failures.fetch_add(1, Ordering::Relaxed);
        self.file = None;
        self.counters.file_active.store(false, Ordering::Release);
        let warning = file_warning();
        self.write_internal_event(&warning);
    }

    fn flush(&mut self) {
        let dropped = self.counters.dropped.load(Ordering::Relaxed);
        if dropped > self.reported_drops {
            self.write_internal_event(&DiagnosticEvent {
                timestamp_ms: now_ms(),
                level: "warn".to_owned(),
                target: "bluink.diagnostics".to_owned(),
                name: Some("queue_overflow".to_owned()),
                message: Some("Diagnostic events dropped".to_owned()),
                fields: BTreeMap::from([("droppedEvents".to_owned(), json!(dropped))]),
                spans: Vec::new(),
                duration_ms: None,
            });
            self.reported_drops = dropped;
        }
        if self.file.as_mut().is_some_and(|file| file.flush().is_err()) {
            self.file_failed();
        }
        if self.console.flush().is_err() {
            self.counters.failures.fetch_add(1, Ordering::Relaxed);
        }
    }
    #[expect(
        clippy::needless_pass_by_value,
        reason = "The worker owns its receiver and lifetime until shutdown."
    )]
    fn run(mut self, receiver: Receiver<Command>) {
        loop {
            match receiver.recv_timeout(Duration::from_millis(250)) {
                Ok(Command::Event(event)) => self.write_event(&event),
                Ok(Command::Flush(done)) => {
                    self.flush();
                    let _ = done.send(());
                }
                Ok(Command::Stop(done)) => {
                    self.flush();
                    let _ = done.send(());
                    break;
                }
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => self.flush(),
                Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                    self.flush();
                    break;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Default)]
    struct Memory(Arc<Mutex<Vec<u8>>>);
    impl Write for Memory {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    impl Memory {
        fn text(&self) -> String {
            String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
        }
    }
    fn event(priority: &str) -> DiagnosticEvent {
        DiagnosticEvent {
            timestamp_ms: 1,
            level: priority.to_owned(),
            target: "bluink.test".to_owned(),
            name: Some("sample".to_owned()),
            message: None,
            fields: BTreeMap::from([("durationMs".to_owned(), json!(4.2))]),
            spans: Vec::new(),
            duration_ms: None,
        }
    }

    #[test]
    fn poisoned_shutdown_still_drains_and_joins_the_writer() {
        let output = Memory::default();
        let sink =
            Sink::start(Box::new(output.clone()), None, 2, 5, false, Arc::default()).unwrap();
        std::thread::scope(|scope| {
            let sink = &sink;
            assert!(
                scope
                    .spawn(move || {
                        let _guard = sink.worker.lock().unwrap();
                        panic!("Injected writer ownership failure");
                    })
                    .join()
                    .is_err()
            );
        });
        sink.emit(event("warn"));
        assert!(sink.flush().is_err());
        assert!(sink.shutdown().err().unwrap().contains("poisoned"));
        assert!(output.text().contains("sample"));
        let worker = sink
            .worker
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert!(worker.is_none());
        drop(worker);
        assert!(sink.shutdown().is_err());
    }

    #[test]
    fn filters_console_separately_from_optional_file_and_flushes() {
        let console = Memory::default();
        let file = Memory::default();
        let sink = Sink::start(
            Box::new(console.clone()),
            Some(Box::new(file.clone())),
            2,
            1,
            false,
            Arc::default(),
        )
        .unwrap();
        sink.emit(event("debug"));
        sink.emit(event("warn"));
        assert_eq!(sink.flush().unwrap().dropped_events, 0);
        assert!(!console.text().contains("DEBUG"));
        assert!(console.text().contains("WARN [bluink.test] sample"));
        let captured: Vec<Value> = file
            .text()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(captured.len(), 2);
        assert_eq!(captured[0]["level"], "debug");
        sink.shutdown().unwrap();
        sink.shutdown().unwrap();
        sink.flush().unwrap();
    }

    #[test]
    fn terminal_colors_and_environment_precedence() {
        assert!(console_line(&event("warn"), true).starts_with("\x1b[33m"));
        assert!(console_line(&event("error"), true).starts_with("\x1b[31m"));
        assert!(console_line(&file_warning(), true).starts_with("\x1b[33m"));
        assert!(!console_line(&file_warning(), false).contains('\x1b'));
        assert!(!console_line(&event("warn"), false).contains('\x1b'));
        assert!(color_enabled(true, false, Some("xterm")));
        assert!(!color_enabled(false, false, None));
        assert!(!color_enabled(true, true, None));
        assert!(!color_enabled(true, false, Some("dumb")));
        assert_eq!(thresholds(None, true), (2, 1));
        assert_eq!(thresholds(Some("error"), true), (4, 4));
        assert_eq!(thresholds(Some("trace"), false), (0, 5));
        // An invalid explicit level falls back to info for both sinks; an
        // absent level retains the more detailed optional file default.
        assert_eq!(thresholds(Some("invalid"), true), (2, 2));
        assert_eq!(thresholds(Some(""), false), (2, 5));
    }

    #[test]
    fn rust_spans_and_events_reach_the_writer_with_redaction_and_elapsed_time() {
        use tracing_subscriber::prelude::*;
        let console = Memory::default();
        let file = Memory::default();
        let sink = Arc::new(
            Sink::start(
                Box::new(console),
                Some(Box::new(file.clone())),
                2,
                1,
                false,
                Arc::default(),
            )
            .unwrap(),
        );
        let subscriber = tracing_subscriber::registry()
            .with(super::super::RustDiagnosticsLayer { sink: sink.clone() });
        tracing::subscriber::with_default(subscriber, || {
            let _span =
                tracing::info_span!("project.operation", note_id = "private-note", count = 3)
                    .entered();
            tracing::info!(target: "bluink.test", file_path = "private-path", count = 4, "private message");
        });
        sink.flush().unwrap();
        let events: Vec<Value> = file
            .text()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0]["fields"]["file_path"], "[redacted]");
        assert_eq!(events[0]["fields"]["count"], 4);
        assert_eq!(events[0]["fields"]["note_id"], "[redacted]");
        assert_eq!(events[0]["spans"][0], "project.operation");
        assert_eq!(events[1]["fields"]["note_id"], "[redacted]");
        assert!(events[1]["durationMs"].as_f64().unwrap() >= 0.0);
        assert!(!file.text().contains("private message"));
        sink.shutdown().unwrap();
    }

    #[test]
    fn error_only_capture_retains_parent_context_and_only_admits_fixed_messages() {
        use tracing_subscriber::prelude::*;
        let file = Memory::default();
        let sink = Arc::new(
            Sink::start(
                Box::new(Memory::default()),
                Some(Box::new(file.clone())),
                4,
                4,
                false,
                Arc::default(),
            )
            .unwrap(),
        );
        let subscriber = tracing_subscriber::registry()
            .with(super::super::RustDiagnosticsLayer { sink: sink.clone() });
        tracing::subscriber::with_default(subscriber, || {
            let _parent = tracing::info_span!(
                "http.request",
                request_seq = 17_u64,
                operation = "create_export"
            )
            .entered();
            let _child = tracing::debug_span!("export.job", export_seq = 22_u64).entered();
            tracing::error!(target: "bluink.export", event = "export_failed", "Export generation failed");
            tracing::error!(target: "bluink.export", "private content {}", "do not publish");
        });
        sink.shutdown().unwrap();
        let events: Vec<Value> = file
            .text()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0]["message"], "Export generation failed");
        assert_eq!(events[0]["fields"]["request_seq"], 17);
        assert_eq!(events[0]["fields"]["export_seq"], 22);
        assert_eq!(events[0]["spans"], json!(["http.request", "export.job"]));
        assert!(events[1].get("message").is_none());
        assert!(!file.text().contains("do not publish"));
    }

    #[test]
    fn saturated_queue_counts_drops_without_blocking_the_operation() {
        let (sender, _receiver) = bounded(1);
        let sink = Sink {
            sender,
            counters: Arc::default(),
            worker: Mutex::new(None),
            console_level: 0,
            capture_level: 5,
        };
        sink.emit(event("info"));
        sink.emit(event("info"));
        assert_eq!(sink.statistics().dropped_events, 1);
        let console = Memory::default();
        let mut writer = Writer {
            console: Box::new(console.clone()),
            file: None,
            console_level: 0,
            capture_level: 5,
            color: false,
            counters: sink.counters,
            reported_drops: 0,
        };
        writer.flush();
        writer.flush();
        assert_eq!(console.text().matches("queue_overflow").count(), 1);
    }

    #[test]
    fn real_writer_saturation_retains_accepted_events_and_reports_drops_on_shutdown() {
        struct PausedConsole {
            output: Memory,
            entered: Option<std::sync::mpsc::Sender<()>>,
            resume: std::sync::mpsc::Receiver<()>,
        }
        impl Write for PausedConsole {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                if let Some(entered) = self.entered.take() {
                    entered.send(()).unwrap();
                    self.resume.recv_timeout(Duration::from_secs(5)).unwrap();
                }
                self.output.write(bytes)
            }
            fn flush(&mut self) -> std::io::Result<()> {
                self.output.flush()
            }
        }
        let console = Memory::default();
        let (entered, waiting) = std::sync::mpsc::channel();
        let (resume, gate) = std::sync::mpsc::channel();
        let sink = Sink::start(
            Box::new(PausedConsole {
                output: console.clone(),
                entered: Some(entered),
                resume: gate,
            }),
            None,
            0,
            5,
            false,
            Arc::default(),
        )
        .unwrap();
        sink.emit(event("info"));
        waiting.recv_timeout(Duration::from_secs(5)).unwrap();
        // The actual writer is paused inside IO, so precisely one event exceeds
        // its bounded queue. Producers must finish without waiting for IO.
        for _ in 0..=QUEUE_CAPACITY {
            sink.emit(event("info"));
        }
        assert_eq!(sink.statistics().dropped_events, 1);
        resume.send(()).unwrap();
        let statistics = sink.shutdown().unwrap();
        assert_eq!(statistics.dropped_events, 1);
        assert_eq!(statistics.write_failures, 0);
        assert_eq!(
            console.text().matches("INFO [bluink.test] sample").count(),
            QUEUE_CAPACITY + 1
        );
        assert_eq!(console.text().matches("queue_overflow").count(), 1);
    }

    struct Broken;
    impl Write for Broken {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            Err(std::io::Error::other("sensitive path"))
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Err(std::io::Error::other("sensitive path"))
        }
    }
    #[test]
    fn file_failure_is_reported_once_without_exposing_paths() {
        let console = Memory::default();
        let sink = Sink::start(
            Box::new(console.clone()),
            Some(Box::new(Broken)),
            2,
            0,
            false,
            Arc::default(),
        )
        .unwrap();
        sink.emit(event("info"));
        sink.emit(event("info"));
        assert_eq!(sink.flush().unwrap().write_failures, 1);
        assert_eq!(console.text().matches("file unavailable").count(), 1);
        assert!(!console.text().contains("sensitive path"));
        sink.shutdown().unwrap();
    }
}
