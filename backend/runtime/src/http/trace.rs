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

#[derive(Default)]
struct Timings {
    body: Duration,
    decode: Duration,
    queue: Duration,
    admission: Duration,
    readiness: Duration,
    execute: Duration,
    encode: Duration,
}

#[derive(Clone, Default)]
pub(super) struct Record(Arc<Mutex<Timings>>);

impl Record {
    pub(super) fn reading(&self) -> BodyRead {
        BodyRead {
            record: self.clone(),
            started: Instant::now(),
        }
    }
    pub(super) fn decode(&self, elapsed: Duration) {
        self.0.lock().unwrap().decode = elapsed;
    }
    pub(super) fn queue(&self, elapsed: Duration) {
        self.0.lock().unwrap().queue = elapsed;
    }
    pub(super) fn encode(&self, elapsed: Duration) {
        self.0.lock().unwrap().encode += elapsed;
    }
    pub(super) fn execution(&self, elapsed: Duration, waits: waits::Timings) {
        let mut timings = self.0.lock().unwrap();
        timings.admission = waits.admission;
        timings.readiness = waits.readiness;
        timings.execute = elapsed.saturating_sub(waits.admission + waits.readiness);
    }
}

pub(super) struct BodyRead {
    record: Record,
    started: Instant,
}

impl Drop for BodyRead {
    fn drop(&mut self) {
        self.record
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .body = self.started.elapsed();
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
        let timings = self
            .record
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let duration_ms = milliseconds(self.started.elapsed());
        let status = self.status;
        let message = match status {
            499 => "HTTP request interrupted",
            500..=599 => "HTTP request failed",
            400..=498 => "HTTP request rejected",
            _ if duration_ms >= slow_threshold_ms() => "slow HTTP request",
            _ => "HTTP request completed",
        };
        let level = if status >= 500 {
            tracing::Level::ERROR
        } else if duration_ms >= slow_threshold_ms() {
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
                        encode_ms = milliseconds(timings.encode),
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
