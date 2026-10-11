// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use std::collections::VecDeque;
use std::time::Instant;

use chrono::{DateTime, Local};
use serde_json::Value;

use crate::diagnostics::DiagnosticEvent;

const HISTORY_COUNT: usize = 1_000;
const HISTORY_BYTES: usize = 2 * 1_024 * 1_024;
const ENTRY_CHARACTERS: usize = 16_384;

/// Treat every external string as text, never as terminal instructions.
pub(super) fn text(value: &str) -> String {
    value
        .chars()
        .filter(|character| {
            !character.is_control()
                && !matches!(*character, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
        })
        .take(ENTRY_CHARACTERS)
        .collect()
}

pub(super) fn summary(event: &DiagnosticEvent) -> String {
    let timestamp = i64::try_from(event.timestamp_ms)
        .ok()
        .and_then(DateTime::from_timestamp_millis)
        .map_or_else(
            || "--:--:--".to_owned(),
            |date| date.with_timezone(&Local).format("%H:%M:%S").to_string(),
        );
    let operation = event
        .fields
        .get("operation")
        .and_then(Value::as_str)
        .or(event.name.as_deref())
        .unwrap_or("event");
    let message = event.message.as_deref().unwrap_or(operation);
    let suffix = if message == operation {
        String::new()
    } else {
        format!(" · {operation}")
    };
    let duration = event
        .duration_ms
        .map(|duration| format!(" ({duration:.1} ms)"))
        .unwrap_or_default();
    text(&format!(
        "{timestamp}  {:5}  {message}{suffix}{duration}",
        event.level.to_uppercase()
    ))
}

pub(super) struct Entry {
    pub(super) id: u64,
    pub(super) level: String,
    pub(super) summary: String,
    pub(super) details: String,
}

#[derive(Default)]
pub(super) struct History {
    pub(super) entries: VecDeque<Entry>,
    pub(super) evicted: u64,
    bytes: usize,
    sequence: u64,
}

impl History {
    pub(super) fn push(&mut self, event: &DiagnosticEvent) {
        let summary = summary(event);
        let details = text(&serde_json::to_string(event).unwrap_or_default());
        self.sequence += 1;
        self.bytes += summary.len() + details.len() + event.level.len();
        self.entries.push_back(Entry {
            id: self.sequence,
            level: event.level.clone(),
            summary,
            details,
        });
        while self.entries.len() > HISTORY_COUNT || self.bytes > HISTORY_BYTES {
            if let Some(entry) = self.entries.pop_front() {
                self.bytes -= entry.summary.len() + entry.details.len() + entry.level.len();
                self.evicted += 1;
            }
        }
    }
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum View {
    #[default]
    Status,
    Logs,
    Errors,
    Info,
    Help,
}

#[derive(Default)]
pub(super) struct Model {
    pub(super) history: History,
    pub(super) view: View,
    pub(super) selected: Option<u64>,
    pub(super) details: bool,
    pub(super) following: bool,
    pub(super) scroll: u16,
}

impl Model {
    pub(super) fn entries(&self) -> Vec<&Entry> {
        self.history
            .entries
            .iter()
            .filter(|entry| self.view != View::Errors || entry.level == "error")
            .collect()
    }

    pub(super) fn open(&mut self, view: View) {
        self.view = view;
        self.following = true;
        self.details = false;
        self.scroll = 0;
        self.selected = self.entries().last().map(|entry| entry.id);
    }

    pub(super) fn push(&mut self, event: &DiagnosticEvent) {
        self.history.push(event);
        if self.following {
            self.selected = self.entries().last().map(|entry| entry.id);
        } else if !self
            .entries()
            .iter()
            .any(|entry| Some(entry.id) == self.selected)
        {
            self.selected = self.entries().first().map(|entry| entry.id);
        }
    }

    pub(super) fn navigate(&mut self, offset: isize, edge: Option<bool>) {
        let entries = self.entries();
        if entries.is_empty() {
            return;
        }
        let current = entries
            .iter()
            .position(|entry| Some(entry.id) == self.selected)
            .unwrap_or(entries.len() - 1);
        let index = edge.map_or_else(
            || current.saturating_add_signed(offset).min(entries.len() - 1),
            |end| if end { entries.len() - 1 } else { 0 },
        );
        let id = entries[index].id;
        self.following = index == entries.len() - 1;
        self.selected = Some(id);
    }

    pub(super) fn scroll_content(&mut self, offset: i16, edge: Option<bool>, limit: u16) {
        self.scroll = edge.map_or_else(
            || {
                self.scroll
                    .min(limit)
                    .saturating_add_signed(offset)
                    .min(limit)
            },
            |end| if end { limit } else { 0 },
        );
    }
}

#[derive(Default, PartialEq, Eq)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "Read-only snapshot of independent application, watcher and indexing states."
)]
pub(super) struct Status {
    pub(super) project: String,
    pub(super) phase: String,
    pub(super) ready: bool,
    pub(super) stopping: bool,
    pub(super) failed: bool,
    pub(super) processed: u64,
    pub(super) total: u64,
    pub(super) pending: u64,
    pub(super) updating: bool,
}

impl Status {
    pub(super) fn from_value(project: String, value: &Value) -> Self {
        let index = &value["index"];
        let ready = value["ready"].as_bool() == Some(true);
        let updating = !index.is_null() && index["state"].as_str() != Some("ready");
        Self {
            project,
            phase: text(
                index["phase"]
                    .as_str()
                    .or_else(|| value["phase"].as_str())
                    .unwrap_or("Starting"),
            ),
            ready,
            stopping: value["stopping"].as_bool() == Some(true),
            failed: !value["error"].is_null()
                || !value["watcherError"].is_null()
                || index["state"].as_str() == Some("error"),
            processed: index["processed"].as_u64().unwrap_or(0),
            total: index["total"].as_u64().unwrap_or(0),
            pending: index["pending"].as_u64().unwrap_or(0),
            updating,
        }
    }

    pub(super) fn label(&self, listening: bool, started: Instant) -> String {
        if self.stopping {
            return "Stopping…".to_owned();
        }
        if self.failed {
            return format!("Attention needed · {}", self.phase);
        }
        let state = if listening { "Running" } else { "Starting" };
        if self.ready && !self.updating {
            return format!("{state} · Search up to date");
        }
        self.processed
            .min(self.total)
            .saturating_mul(100)
            .checked_div(self.total)
            .map_or_else(
                || {
                    format!(
                        "{state} · {} · {:.1} s",
                        self.phase,
                        started.elapsed().as_secs_f64()
                    )
                },
                |percent| {
                    format!(
                        "{state} · {} · {}/{} · {percent}%",
                        self.phase, self.processed, self.total
                    )
                },
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(level: &str, message: &str) -> DiagnosticEvent {
        DiagnosticEvent {
            timestamp_ms: 0,
            level: level.to_owned(),
            target: "test".to_owned(),
            name: None,
            message: Some(message.to_owned()),
            fields: std::collections::BTreeMap::default(),
            spans: vec![],
            duration_ms: None,
        }
    }

    #[test]
    fn history_is_bounded_by_count_and_bytes() {
        let mut history = History::default();
        for _ in 0..1_010 {
            history.push(&event("info", "example"));
        }
        assert_eq!(history.entries.len(), 1_000);
        assert_eq!(history.evicted, 10);
        for _ in 0..100 {
            history.push(&event("error", &"x".repeat(16_384)));
        }
        assert!(history.bytes <= HISTORY_BYTES);
        assert!(history.evicted > 10);
    }

    #[test]
    fn arrival_preserves_selection_until_following_is_requested() {
        let mut model = Model::default();
        model.push(&event("error", "first"));
        model.push(&event("info", "second"));
        model.open(View::Logs);
        model.navigate(-1, None);
        let selected = model.selected;
        model.push(&event("error", "third"));
        assert_eq!(model.selected, selected);
        model.navigate(0, Some(true));
        model.push(&event("info", "fourth"));
        assert_eq!(model.selected, Some(4));
        model.open(View::Errors);
        assert_eq!(model.selected, Some(3));
    }

    #[test]
    fn terminal_control_strings_are_inert() {
        assert_eq!(text("hello\x1b[2J\n\r\u{202e}world"), "hello[2Jworld");
        assert!(!summary(&event("warn", "\x1b[31mBAD")).contains('\x1b'));
    }

    #[test]
    fn unknown_totals_never_claim_zero_percent() {
        let status = Status::from_value(
            "fixture".to_owned(),
            &serde_json::json!({"phase":"Discovering project files"}),
        );
        assert!(!status.label(false, Instant::now()).contains('%'));
    }
}
