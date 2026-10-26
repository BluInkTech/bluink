// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use std::sync::{Arc, Mutex};

use crossbeam_channel::Sender;
use serde::Serialize;

use super::coordinator::IndexCoordinator;
use super::source::SourceId;
use crate::model::{CalendarResults, SearchResults, TargetCount};

#[derive(Clone)]
pub(crate) enum CachedResult {
    Search(SearchResults),
    Calendar(CalendarResults),
    Facets(Vec<TargetCount>),
}

#[derive(Debug, Serialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
#[expect(
    clippy::struct_field_names,
    reason = "Serialized timing fields include their millisecond unit."
)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub(crate) struct ReconciliationTimings {
    pub(crate) walk_ms: f64,
    pub(crate) pin_scope_ms: f64,
    pub(crate) file_stamp_ms: f64,
    pub(crate) indexed_stamp_ms: f64,
    pub(crate) diff_ms: f64,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ServiceStatus {
    pub(crate) state: String,
    pub(crate) phase: String,
    pub(crate) processed: usize,
    pub(crate) total: usize,
    pub(crate) checked: usize,
    pub(crate) changed: usize,
    pub(crate) pending: usize,
    pub(crate) documents: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) last_committed_at: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) error: Option<String>,
    pub(crate) cold: bool,
    pub(crate) schema_fingerprint: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) reconciliation_timings: Option<ReconciliationTimings>,
}

pub(crate) enum Work {
    Mutation {
        sources: Vec<(SourceId, String)>,
        completion: Sender<Result<(), String>>,
    },
    Upsert {
        id: SourceId,
        expected_revision: Option<String>,
    },
    Shutdown,
}

pub(crate) struct Service {
    pub(crate) worker: Option<std::thread::JoinHandle<()>>,
    pub(crate) handle: ServiceHandle,
}

#[derive(Clone)]
pub(crate) struct ServiceHandle {
    pub(crate) project_root: String,
    pub(crate) sender: Sender<Work>,
    pub(crate) coordinator: Arc<IndexCoordinator>,
    /// Serialize the complete schema read/publish operation, outside registry/state locks.
    pub(crate) schema_refresh: Arc<Mutex<()>>,
}
