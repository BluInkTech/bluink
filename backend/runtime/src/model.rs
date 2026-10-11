// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Instant;

use crossbeam_channel::Sender;
use serde::{Deserialize, Serialize};
use serde_json::{Value as JsonValue, json};
use tantivy::{Index, IndexReader};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub(crate) struct PassageRecord {
    pub(crate) item_id: String,
    pub(crate) block_id: String,
    pub(crate) revision: String,
    pub(crate) note_title: String,
    pub(crate) labels: Vec<String>,
    pub(crate) text: String,
    #[cfg_attr(
        test,
        ts(as = "std::collections::BTreeMap<String, crate::engine::project::types::OpaqueJson>")
    )]
    pub(crate) fragment: JsonValue,
    pub(crate) truncated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(test, derive(ts_rs::TS))]
pub(crate) struct TaskRecord {
    pub(crate) note_id: String,
    pub(crate) task_id: String,
    pub(crate) revision: String,
    pub(crate) note_title: String,
    pub(crate) text: String,
    pub(crate) created_at: String,
    pub(crate) completed_at: Option<String>,
    pub(crate) due_date: Option<String>,
    pub(crate) modified_at: String,
}

pub(crate) fn empty_object() -> JsonValue {
    json!({})
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SearchDocument {
    pub(crate) id: String,
    #[serde(default)]
    pub(crate) index_stamp: String,
    #[serde(default)]
    pub(crate) revision: String,
    #[serde(default)]
    pub(crate) created_at: String,
    #[serde(default)]
    pub(crate) modified_at: String,
    pub(crate) title: String,
    #[serde(default)]
    pub(crate) title_sort: String,
    pub(crate) preview: String,
    #[serde(default)]
    pub(crate) preview_image: Option<String>,
    pub(crate) body: String,
    pub(crate) item_type: String,
    pub(crate) state: String,
    #[serde(default)]
    pub(crate) tags: Vec<String>,
    #[serde(default)]
    pub(crate) labels: Vec<String>,
    #[serde(default)]
    pub(crate) reference_targets: Vec<String>,
    #[serde(default)]
    pub(crate) snippet_citation_targets: Vec<String>,
    #[serde(default)]
    pub(crate) snippet_embed_targets: Vec<String>,
    #[serde(default)]
    pub(crate) passages: Vec<PassageRecord>,
    #[serde(default)]
    pub(crate) tasks: Vec<TaskRecord>,
    #[serde(default)]
    pub(crate) pins: Vec<String>,
    #[serde(default = "empty_object")]
    pub(crate) metadata: JsonValue,
}

#[derive(Debug, Clone)]
pub(crate) struct SortSpec {
    pub(crate) field: String,
    pub(crate) descending: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SearchHit {
    pub(crate) id: String,
    pub(crate) revision: String,
    pub(crate) created_at: String,
    pub(crate) modified_at: String,
    pub(crate) score: f32,
    pub(crate) preview: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) preview_image: Option<String>,
    pub(crate) metadata: JsonValue,
    pub(crate) tags: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct TargetCount {
    pub(crate) tid: String,
    pub(crate) count: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SearchResults {
    pub(crate) hits: Vec<SearchHit>,
    pub(crate) count: usize,
    pub(crate) pinned_count: Option<usize>,
}

#[derive(Debug, Clone, serde::Deserialize, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub(crate) struct CalendarBucket {
    pub(crate) date: String,
    pub(crate) count: u64,
}

#[derive(Debug, Clone, serde::Deserialize, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub(crate) struct CalendarResults {
    pub(crate) buckets: Vec<CalendarBucket>,
}

#[derive(Debug, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub(crate) struct ResultPage<T> {
    pub(crate) items: Vec<T>,
    pub(crate) count: usize,
}

#[derive(Clone)]
pub(crate) enum CachedResult {
    Search(SearchResults),
    Calendar(CalendarResults),
    Facets(Vec<TargetCount>),
}

pub(crate) type AggregationCache = Arc<Mutex<HashMap<String, CachedResult>>>;

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
    Upsert {
        id: String,
        expected_revision: Option<String>,
    },
    Shutdown,
}

pub(crate) type DirtyRevisions = Arc<Mutex<HashMap<String, String>>>;

pub(crate) struct Service {
    pub(crate) last_read: Arc<Mutex<Instant>>,
    pub(crate) worker: Option<std::thread::JoinHandle<()>>,
    pub(crate) project_root: String,
    pub(crate) query_fields: Arc<RwLock<HashSet<String>>>,
    pub(crate) sender: Sender<Work>,
    pub(crate) status: Arc<Mutex<ServiceStatus>>,
    pub(crate) dirty: DirtyRevisions,
    pub(crate) index: Arc<RwLock<Option<Index>>>,
    pub(crate) reader: Arc<RwLock<Option<IndexReader>>>,
    pub(crate) passage_index: Arc<RwLock<Option<Index>>>,
    pub(crate) passage_reader: Arc<RwLock<Option<IndexReader>>>,
    pub(crate) task_index: Arc<RwLock<Option<Index>>>,
    pub(crate) task_reader: Arc<RwLock<Option<IndexReader>>>,
    pub(crate) aggregation_cache: AggregationCache,
}

pub(crate) struct WorkerState {
    pub(crate) last_read: Arc<Mutex<Instant>>,
    pub(crate) status: Arc<Mutex<ServiceStatus>>,
    pub(crate) dirty: DirtyRevisions,
    pub(crate) index: Arc<RwLock<Option<Index>>>,
    pub(crate) reader: Arc<RwLock<Option<IndexReader>>>,
    pub(crate) passage_index: Arc<RwLock<Option<Index>>>,
    pub(crate) passage_reader: Arc<RwLock<Option<IndexReader>>>,
    pub(crate) task_index: Arc<RwLock<Option<Index>>>,
    pub(crate) task_reader: Arc<RwLock<Option<IndexReader>>>,
    pub(crate) aggregation_cache: AggregationCache,
}
