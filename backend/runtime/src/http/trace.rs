// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! One completion event per request, including rejections and interrupted
//! handlers. Handoff timing is not a measurement of a streamed download.
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::body::HttpBody;
use axum::extract::Request;
use axum::middleware::Next;
use axum::response::Response;
use tracing::Instrument;

use crate::application::operation;
use crate::diagnostics::{slow_threshold_ms, waits};

static SEQUENCE: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, Default)]
struct Timings {
    body: Duration,
    decode: Duration,
    queue: Duration,
    admission: Duration,
    readiness: Duration,
    execute: Duration,
    storage_lock: Duration,
    publication: Duration,
    encode: Duration,
    /// The request made a sized image copy: slow by design, once per image.
    image_copy: Duration,
}

#[derive(Clone, Default)]
pub(super) struct Record(Arc<Mutex<Timings>>);

impl Record {
    // Diagnostics are best-effort bookkeeping, not canonical state. A poisoned
    // timing lock must never turn a subsequent HTTP request into another panic.
    fn timings(&self) -> std::sync::MutexGuard<'_, Timings> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
    pub(super) fn reading(&self) -> BodyRead {
        BodyRead {
            record: self.clone(),
            started: Instant::now(),
        }
    }
    pub(super) fn decode(&self, elapsed: Duration) {
        self.timings().decode = elapsed;
    }
    pub(super) fn queue(&self, elapsed: Duration) {
        self.timings().queue = elapsed;
    }
    pub(super) fn encode(&self, elapsed: Duration) {
        self.timings().encode += elapsed;
    }
    pub(super) fn image_copy(&self, elapsed: Duration) {
        self.timings().image_copy = elapsed;
    }
    pub(super) fn execution(&self, elapsed: Duration, waits: waits::Timings) {
        let mut timings = self.timings();
        timings.admission = waits.admission;
        timings.readiness = waits.readiness;
        timings.execute = elapsed.saturating_sub(waits.admission + waits.readiness);
        // These are parts of execution, not additional request phases.
        timings.storage_lock = waits.storage_lock;
        timings.publication = waits.publication;
    }
}

pub(super) struct BodyRead {
    record: Record,
    started: Instant,
}

impl Drop for BodyRead {
    fn drop(&mut self) {
        self.record.timings().body = self.started.elapsed();
    }
}

struct Completion {
    record: Record,
    span: tracing::Span,
    started: Instant,
    status: u16,
    bytes: Option<u64>,
}

impl Drop for Completion {
    fn drop(&mut self) {
        let timings = *self.record.timings();
        let duration_ms = milliseconds(self.started.elapsed());
        let status = self.status;
        // Making an image copy is expected work, so it is not reported as slow.
        let slow = duration_ms - milliseconds(timings.image_copy) >= slow_threshold_ms();
        let message = match status {
            499 => "HTTP request interrupted",
            500..=599 => "HTTP request failed",
            400..=498 => "HTTP request rejected",
            _ if slow => "slow HTTP request",
            _ if !timings.image_copy.is_zero() => "image copy made",
            _ => "HTTP request completed",
        };
        let level = if status >= 500 {
            tracing::Level::ERROR
        } else if slow {
            tracing::Level::WARN
        } else {
            tracing::Level::DEBUG
        };
        self.span.in_scope(|| {
            // tracing levels are callsite constants, so select the callsite
            // while keeping the completion fields identical.
            macro_rules! emit {
                ($level:expr) => {
                    tracing::event!(target: "bluink.http", $level,
                        event = "request_complete", status,
                        duration_ms, body_read_ms = milliseconds(timings.body),
                        decode_ms = milliseconds(timings.decode),
                        worker_queue_ms = milliseconds(timings.queue),
                        admission_ms = milliseconds(timings.admission),
                        readiness_ms = milliseconds(timings.readiness),
                        execute_ms = milliseconds(timings.execute),
                        storage_lock_ms = milliseconds(timings.storage_lock),
                        publication_ms = milliseconds(timings.publication),
                        encode_ms = milliseconds(timings.encode),
                        image_copy_ms = milliseconds(timings.image_copy),
                        response_bytes = self.bytes, message)
                };
            }
            match level {
                tracing::Level::ERROR => emit!(tracing::Level::ERROR),
                tracing::Level::WARN => emit!(tracing::Level::WARN),
                _ => emit!(tracing::Level::DEBUG),
            }
        });
    }
}

pub(crate) fn milliseconds(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

pub(super) async fn request(mut request: Request, next: Next) -> Response {
    let started = Instant::now();
    // Never include a raw path or unknown operation: either may be user data.
    let path = request.uri().path();
    let operation = path
        .strip_prefix("/api/")
        .filter(|name| operation::Request::http_method(name).is_some())
        .unwrap_or_else(|| {
            if matches!(path, "/api/upload_asset" | "/api/upload_asset/") {
                "upload_asset"
            } else if path.starts_with("/api/export_artifact/") {
                "export_artifact"
            } else if !path.starts_with("/api") {
                "static_file"
            } else {
                "unknown"
            }
        });
    let span = tracing::info_span!(target: "bluink.http", "http.request",
        request_seq = SEQUENCE.fetch_add(1, Ordering::Relaxed), operation);
    let record = Record(Arc::default());
    request.extensions_mut().insert(record.clone());
    let mut completion = Completion {
        record,
        span: span.clone(),
        started,
        status: 499,
        bytes: None,
    };
    let response = next.run(request).instrument(span).await;
    completion.status = response.status().as_u16();
    completion.bytes = response.body().size_hint().exact().or_else(|| {
        response
            .headers()
            .get(axum::http::header::CONTENT_LENGTH)?
            .to_str()
            .ok()?
            .parse()
            .ok()
    });
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn execution_breakdown_preserves_total_and_wait_categories() {
        let record = Record::default();
        record.execution(
            Duration::from_millis(100),
            waits::Timings {
                admission: Duration::from_millis(5),
                readiness: Duration::from_millis(10),
                storage_lock: Duration::from_millis(20),
                publication: Duration::from_millis(30),
            },
        );
        let timings = record.timings();
        assert_eq!(timings.execute, Duration::from_millis(85));
        assert_eq!(timings.storage_lock, Duration::from_millis(20));
        assert_eq!(timings.publication, Duration::from_millis(30));
    }

    #[test]
    fn poisoned_diagnostics_do_not_panic_subsequent_request_bookkeeping() {
        let record = Record::default();
        let poison = record.clone();
        assert!(
            std::panic::catch_unwind(move || {
                let _guard = poison.0.lock().unwrap();
                panic!("Simulate interrupted timing bookkeeping");
            })
            .is_err()
        );
        assert!(record.0.is_poisoned());
        record.decode(Duration::from_millis(2));
        record.queue(Duration::from_millis(3));
        record.encode(Duration::from_millis(4));
        record.execution(Duration::from_millis(8), waits::Timings::default());
        drop(record.reading());
        let timings = record.timings();
        assert_eq!(timings.decode, Duration::from_millis(2));
        assert_eq!(timings.queue, Duration::from_millis(3));
        assert_eq!(timings.encode, Duration::from_millis(4));
        assert_eq!(timings.execute, Duration::from_millis(8));
        drop(timings);
        drop(Completion {
            record,
            span: tracing::Span::none(),
            started: Instant::now(),
            status: 200,
            bytes: None,
        });
    }
}
