// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use std::collections::BTreeMap;
use std::sync::{Arc, OnceLock};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use napi::bindgen_prelude::Result;
use napi_derive::napi;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tracing::field::{Field, Visit};
use tracing::{Event, Id, Subscriber};
use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::layer::{Context, Layer};
use tracing_subscriber::prelude::*;
use tracing_subscriber::registry::LookupSpan;

pub(crate) mod waits;
#[path = "diagnostics/writer.rs"]
mod writer;

static DIAGNOSTICS: OnceLock<std::result::Result<Arc<writer::Sink>, String>> = OnceLock::new();
static SLOW_THRESHOLD: OnceLock<f64> = OnceLock::new();

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DiagnosticEvent {
    pub(crate) timestamp_ms: u128,
    pub(crate) level: String,
    pub(crate) target: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) message: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(crate) fields: BTreeMap<String, Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) spans: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) duration_ms: Option<f64>,
}

#[derive(Default)]
struct SafeFields(BTreeMap<String, Value>);

impl SafeFields {
    fn insert(&mut self, field: &Field, value: Value) {
        let name = field.name();
        self.0.insert(
            name.to_owned(),
            if name != "message" && is_private_field(name) {
                Value::String("[redacted]".to_owned())
            } else {
                value
            },
        );
    }
}

fn is_private_field(name: &str) -> bool {
    let mut lower = String::new();
    for character in name.chars() {
        if character.is_ascii_uppercase() && !lower.is_empty() {
            lower.push('_');
        }
        lower.push(character.to_ascii_lowercase());
    }
    [
        "content",
        "text",
        "message",
        "query",
        "path",
        "root",
        "request",
        "body",
        "document",
        "payload",
        "input",
        "filename",
        "file_name",
        "id",
        "revision",
        "hash",
        "source",
        "error",
        "token",
        "secret",
        "password",
        "url",
        "title",
        "snippet",
        "tag",
    ]
    .iter()
    .any(|part| lower == *part || lower.ends_with(&format!("_{part}")))
}

impl Visit for SafeFields {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        self.insert(field, Value::String(format!("{value:?}")));
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        self.insert(field, Value::from(value));
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        self.insert(field, Value::from(value));
    }

    fn record_f64(&mut self, field: &Field, value: f64) {
        self.insert(field, Value::from(value));
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        self.insert(field, Value::from(value));
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        self.insert(field, Value::String(value.to_owned()));
    }
}

#[derive(Default)]
struct SpanState {
    started: Option<Instant>,
    fields: BTreeMap<String, Value>,
}

struct RustDiagnosticsLayer {
    sink: Arc<writer::Sink>,
}

impl RustDiagnosticsLayer {
    fn emit(&self, event: DiagnosticEvent) {
        self.sink.emit(event);
    }
}

impl<S> Layer<S> for RustDiagnosticsLayer
where
    S: Subscriber + for<'lookup> LookupSpan<'lookup>,
{
    fn enabled(&self, metadata: &tracing::Metadata<'_>, _context: Context<'_, S>) -> bool {
        // Context must survive even when only errors are captured. Sink-level
        // filtering still suppresses unwanted span-close and normal events.
        metadata.is_span()
            || self
                .sink
                .enabled(&metadata.level().to_string().to_ascii_lowercase())
    }

    fn on_new_span(
        &self,
        attributes: &tracing::span::Attributes<'_>,
        id: &Id,
        context: Context<'_, S>,
    ) {
        if let Some(span) = context.span(id) {
            let mut fields = SafeFields::default();
            attributes.record(&mut fields);
            span.extensions_mut().insert(SpanState {
                started: Some(Instant::now()),
                fields: fields.0,
            });
        }
    }

    fn on_record(&self, id: &Id, values: &tracing::span::Record<'_>, context: Context<'_, S>) {
        if let Some(span) = context.span(id)
            && let Some(state) = span.extensions_mut().get_mut::<SpanState>()
        {
            let mut fields = SafeFields::default();
            values.record(&mut fields);
            state.fields.extend(fields.0);
        }
    }

    fn on_event(&self, event: &Event<'_>, context: Context<'_, S>) {
        let mut fields = BTreeMap::new();
        let mut spans = Vec::new();
        if let Some(scope) = context.event_scope(event) {
            for span in scope.from_root() {
                spans.push(span.metadata().name().to_owned());
                if let Some(state) = span.extensions().get::<SpanState>() {
                    fields.extend(state.fields.clone());
                }
            }
        }
        let mut own = SafeFields::default();
        event.record(&mut own);
        let message = own.0.remove("message").and_then(|value| {
            value
                .as_str()
                .filter(|value| safe_message(value))
                .map(str::to_owned)
        });
        fields.extend(own.0);
        fields.remove("message");
        let name = fields
            .get("event")
            .and_then(Value::as_str)
            .map_or_else(|| event.metadata().name().to_owned(), safe_identifier);
        self.emit(DiagnosticEvent {
            timestamp_ms: now_ms(),
            level: event.metadata().level().to_string().to_ascii_lowercase(),
            target: event.metadata().target().to_owned(),
            name: Some(name),
            message,
            fields,
            spans,
            duration_ms: None,
        });
    }

    fn on_close(&self, id: Id, context: Context<'_, S>) {
        if let Some(span) = context.span(&id) {
            // Request completion is emitted explicitly at response handoff.
            // Async jobs may retain its context long after the response.
            if matches!(span.metadata().name(), "http.request" | "export.phase") {
                return;
            }
            let extensions = span.extensions();
            if let Some(state) = extensions.get::<SpanState>() {
                let duration_ms = state
                    .started
                    .map(|started| started.elapsed().as_secs_f64() * 1000.0);
                self.emit(DiagnosticEvent {
                    timestamp_ms: now_ms(),
                    level: "debug".to_owned(),
                    target: span.metadata().target().to_owned(),
                    name: Some(span.metadata().name().to_owned()),
                    message: None,
                    fields: span
                        .scope()
                        .from_root()
                        .fold(BTreeMap::new(), |mut fields, parent| {
                            if let Some(state) = parent.extensions().get::<SpanState>() {
                                fields.extend(state.fields.clone());
                            }
                            fields.remove("message");
                            fields
                        }),
                    spans: Vec::new(),
                    duration_ms,
                });
            }
        }
    }
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default()
}

#[napi]
pub fn configure_native_diagnostics() -> Result<()> {
    let result = DIAGNOSTICS.get_or_init(|| {
        let sink = Arc::new(writer::Sink::from_environment()?);
        let subscriber = tracing_subscriber::registry()
            .with(LevelFilter::TRACE)
            .with(RustDiagnosticsLayer { sink: sink.clone() });
        if let Err(error) = subscriber.try_init() {
            let _ = sink.shutdown();
            return Err(error.to_string());
        }
        Ok(sink)
    });
    match result {
        Ok(_) => {
            slow_threshold_ms();
            Ok(())
        }
        Err(error) => Err(crate::error::native_error(error.clone())),
    }
}

/// One-way delivery for the few remaining Node bootstrap/watcher events.
/// Normal backend events never cross the JavaScript boundary.
#[napi]
#[allow(
    clippy::needless_pass_by_value,
    reason = "N-API supplies an owned String at the JavaScript boundary."
)]
pub fn record_native_diagnostic(source: String) -> Result<()> {
    let mut event: DiagnosticEvent = serde_json::from_str(&source)
        .map_err(|_| crate::error::native_error("Invalid diagnostic event".to_owned()))?;
    if !matches!(
        event.level.as_str(),
        "trace" | "debug" | "info" | "warn" | "error"
    ) {
        return Err(crate::error::native_error(
            "Invalid diagnostic level".to_owned(),
        ));
    }
    event.target = safe_identifier(&event.target);
    event.name = event.name.map(|value| safe_identifier(&value));
    event.spans = event
        .spans
        .into_iter()
        .map(|value| safe_identifier(&value))
        .collect();
    // Only known fixed messages from the current Node adapters are admitted.
    // Arbitrary messages can contain user content and paths.
    event.message = event.message.filter(|message| safe_message(message));
    redact_fields(&mut event.fields);
    if let Some(Ok(sink)) = DIAGNOSTICS.get() {
        sink.emit(event);
    }
    Ok(())
}

fn safe_identifier(value: &str) -> String {
    if value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._:-".contains(&byte))
    {
        value.to_owned()
    } else {
        "[redacted]".to_owned()
    }
}

// Only reviewed fixed messages are allowed through either producer. In
// particular, formatted errors, paths and user prose never become log messages.
fn safe_message(message: &str) -> bool {
    matches!(
        message,
        "HTTP request completed"
            | "slow HTTP request"
            | "HTTP request rejected"
            | "HTTP request failed"
            | "HTTP request interrupted"
            | "Invalid BLUINK_RPC_SLOW_MS; using 250 milliseconds"
            | "Export generation failed"
            | "Export phase completed"
            | "HTTP connection drain timed out"
            | "Could not open the browser; use the displayed address"
            | "Console action failed; stopping Bluink"
    )
}

pub(crate) fn file_capture_active() -> bool {
    DIAGNOSTICS
        .get()
        .and_then(|sink| sink.as_ref().ok())
        .is_some_and(|sink| sink.file_active())
}

fn parse_slow_threshold(value: Option<&str>) -> (f64, bool) {
    value.map_or((250.0, false), |value| match value.parse::<f64>() {
        Ok(value) if value.is_finite() && value >= 0.0 => (value, false),
        _ => (250.0, true),
    })
}

pub(crate) fn slow_threshold_ms() -> f64 {
    *SLOW_THRESHOLD.get_or_init(|| {
        let (value, invalid) =
            parse_slow_threshold(std::env::var("BLUINK_RPC_SLOW_MS").ok().as_deref());
        if invalid {
            tracing::warn!(target: "bluink.diagnostics", event = "invalid_slow_threshold",
                "Invalid BLUINK_RPC_SLOW_MS; using 250 milliseconds");
        }
        value
    })
}

fn redact_fields(fields: &mut BTreeMap<String, Value>) {
    for (name, value) in fields {
        redact_value(name, value);
    }
}

fn redact_value(name: &str, value: &mut Value) {
    if is_private_field(name) {
        *value = Value::String("[redacted]".to_owned());
    } else {
        match value {
            Value::Object(fields) => {
                for (name, value) in fields {
                    redact_value(name, value);
                }
            }
            Value::Array(values) => {
                for value in values {
                    redact_value("", value);
                }
            }
            _ => {}
        }
    }
}

fn with_sink(
    action: impl FnOnce(&writer::Sink) -> std::result::Result<writer::Statistics, String>,
) -> Result<String> {
    let statistics = match DIAGNOSTICS.get() {
        Some(Ok(sink)) => action(sink).map_err(crate::error::native_error)?,
        Some(Err(error)) => return Err(crate::error::native_error(error.clone())),
        None => writer::Statistics::default(),
    };
    serde_json::to_string(&statistics)
        .map_err(|error| crate::error::native_error(error.to_string()))
}

/// Flush without stopping the process-wide sink; other app handles may be live.
#[allow(
    clippy::trailing_empty_array,
    reason = "N-API generates a zero-sized argument array for parameterless async functions."
)]
#[napi]
pub async fn flush_native_diagnostics() -> Result<String> {
    tokio::task::spawn_blocking(|| with_sink(writer::Sink::flush))
        .await
        .map_err(|error| crate::error::native_error(error.to_string()))?
}

/// Called by the launcher after all applications have stopped.
#[allow(
    clippy::trailing_empty_array,
    reason = "N-API generates a zero-sized argument array for parameterless async functions."
)]
#[napi]
pub async fn shutdown_native_diagnostics() -> Result<String> {
    tokio::task::spawn_blocking(|| with_sink(writer::Sink::shutdown))
        .await
        .map_err(|error| crate::error::native_error(error.to_string()))?
}

#[cfg(test)]
mod tests {
    use super::{is_private_field, parse_slow_threshold, redact_fields, safe_identifier};

    #[test]
    fn slow_threshold_rejects_nonfinite_negative_and_malformed_values() {
        assert_eq!(parse_slow_threshold(None), (250.0, false));
        assert_eq!(parse_slow_threshold(Some("0")), (0.0, false));
        assert_eq!(parse_slow_threshold(Some("12.5")), (12.5, false));
        for value in ["", "-1", "NaN", "inf", "1e999", "250ms"] {
            assert_eq!(parse_slow_threshold(Some(value)), (250.0, true));
        }
    }

    #[test]
    fn redacts_content_identifiers_and_paths_from_diagnostic_fields() {
        assert!(is_private_field("note_id"));
        assert!(is_private_field("noteId"));
        assert!(is_private_field("file_path"));
        assert!(is_private_field("request_body"));
        assert!(is_private_field("expected_revision"));
        assert!(is_private_field("error"));
        assert!(!is_private_field("response_bytes"));
        assert!(!is_private_field("duration_ms"));
        assert!(!is_private_field("query_length"));
    }

    #[test]
    fn bridge_fields_redact_nested_content_and_reject_control_characters() {
        let mut fields = std::collections::BTreeMap::from([
            ("noteId".to_owned(), serde_json::json!("private")),
            (
                "details".to_owned(),
                serde_json::json!({"filePath":"private", "count":3}),
            ),
        ]);
        redact_fields(&mut fields);
        assert_eq!(fields["noteId"], "[redacted]");
        assert_eq!(fields["details"]["filePath"], "[redacted]");
        assert_eq!(fields["details"]["count"], 3);
        assert_eq!(safe_identifier("bluink.rpc"), "bluink.rpc");
        assert_eq!(safe_identifier("injected\nlog"), "[redacted]");
    }
}
