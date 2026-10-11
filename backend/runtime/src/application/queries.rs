// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use serde::{Deserialize, Serialize};

use super::Application;
use super::error::{Result, internal_error, invalid_request};
use crate::engine::project::types::{CalendarGranularity, FacetKind, TaskStatus};
use crate::model::{CalendarResults, PassageRecord, ReconciliationTimings, ResultPage, TaskRecord};
use crate::{calendar, catalogue, passages, query, relations, reports, service, tasks};

#[derive(Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(optional_fields = nullable))]
pub(crate) struct EmptyRequest {}

#[derive(Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
#[cfg_attr(test, ts(optional_fields = nullable))]
pub(crate) struct QueryTasksRequest {
    query: Option<String>,
    note_id: Option<String>,
    status: TaskStatus,
    limit: Option<u32>,
    offset: Option<u32>,
}

#[derive(Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(optional_fields = nullable))]
pub(crate) struct QueryPassagesRequest {
    label: String,
    limit: Option<u32>,
    offset: Option<u32>,
}

#[derive(Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(optional_fields = nullable))]
pub(crate) struct AnalyzeQueryRequest {
    query: String,
}

#[derive(Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
#[cfg_attr(test, ts(optional_fields = nullable))]
pub(crate) struct CalendarRequest {
    query: Option<String>,
    field: String,
    start_date: String,
    end_date: String,
    granularity: CalendarGranularity,
    pin_scope_id: Option<String>,
}

#[derive(Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(optional_fields = nullable))]
pub(crate) struct ListFacetsRequest {
    kind: Option<FacetKind>,
}

#[derive(Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
#[cfg_attr(test, ts(optional_fields = nullable))]
pub(crate) struct FacetCountsRequest {
    kind: FacetKind,
    keys: Option<Vec<String>>,
    source_type: Option<String>,
}

#[derive(Debug, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub(crate) struct FacetCount {
    key: String,
    count: usize,
}

#[derive(Debug, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub(crate) struct CacheStatus {
    structural_state: String,
    pending: usize,
    documents: usize,
    schema_version: u8,
    schema_fingerprint: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    reconciliation_timings: Option<ReconciliationTimings>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    last_reconciled_at: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    error: Option<String>,
}

impl Application {
    pub(crate) fn query_tasks(&self, request: QueryTasksRequest) -> Result<ResultPage<TaskRecord>> {
        let _operation = self.begin_operation()?;
        if !matches!(request.status.as_str(), "open" | "completed") {
            return Err(invalid_request("Choose open or completed tasks."));
        }
        let limit = request.limit.unwrap_or(50);
        if !(1..=200).contains(&limit) {
            return Err(invalid_request("Choose a valid task page."));
        }
        tasks::query_collection(
            self.index_path()?,
            request.query,
            request.note_id,
            request.status.as_str().to_owned(),
            limit,
            request.offset.unwrap_or(0),
        )
    }

    pub(crate) fn query_passages(
        &self,
        request: QueryPassagesRequest,
    ) -> Result<ResultPage<PassageRecord>> {
        let _operation = self.begin_operation()?;
        passages::query_service(
            self.index_path()?,
            request.label,
            request.limit.unwrap_or(50),
            request.offset.unwrap_or(0),
        )
    }
    #[expect(
        clippy::needless_pass_by_value,
        reason = "The service consumes its decoded request or owned work at the application boundary."
    )]
    pub(crate) fn analyze_query(
        &self,
        request: AnalyzeQueryRequest,
    ) -> Result<query::QueryAnalysis> {
        let _operation = self.begin_operation()?;
        let fields = service::query_fields(&self.index_path()?)?;
        Ok(query::analyze_bluink_query_with_schema(
            &request.query,
            &fields,
        ))
    }

    pub(crate) fn calendar_buckets(&self, request: CalendarRequest) -> Result<CalendarResults> {
        let _operation = self.begin_operation()?;
        let valid_property = request.field.strip_prefix("meta.").is_some_and(|key| {
            key.as_bytes().first().is_some_and(u8::is_ascii_lowercase)
                && key
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
        });
        if !matches!(request.field.as_str(), "created" | "modified" | "completed")
            && !valid_property
        {
            return Err(invalid_request(
                "Choose a created, modified, completed, or registered date field.",
            ));
        }
        let index = self.index_path()?;
        if request.field == "completed" {
            return reports::completed_calendar(
                &self.root,
                &index,
                &request.start_date,
                &request.end_date,
                request.granularity.as_str(),
                request.query.as_deref(),
            );
        }
        calendar::calendar_buckets_service_sync(
            index,
            request.query.unwrap_or_default(),
            request.field,
            request.start_date,
            request.end_date,
            request.granularity.as_str().to_owned(),
            request.pin_scope_id,
        )
    }

    pub(crate) fn metadata_fields(&self) -> Result<Vec<catalogue::MetadataField>> {
        let _operation = self.begin_operation()?;
        catalogue::read_metadata_fields(
            self.root
                .to_str()
                .ok_or_else(|| internal_error("Invalid project path."))?,
        )
    }
    #[expect(
        clippy::needless_pass_by_value,
        reason = "The service consumes its decoded request or owned work at the application boundary."
    )]
    pub(crate) fn list_facets(
        &self,
        request: ListFacetsRequest,
    ) -> Result<Vec<catalogue::FacetEntry>> {
        let _operation = self.begin_operation()?;
        catalogue::list_facet_entries(
            self.root
                .to_str()
                .ok_or_else(|| internal_error("Invalid project path."))?,
            &self.index_path()?,
            request.kind.map(FacetKind::as_str),
        )
    }

    pub(crate) fn facet_counts(&self, request: FacetCountsRequest) -> Result<Vec<FacetCount>> {
        let _operation = self.begin_operation()?;
        let field = match request.kind.as_str() {
            "tag" => "tags",
            "label" => "labels",
            _ => return Err(invalid_request("Facet kind must be tag or label.")),
        };
        Ok(relations::facet_service_sync(
            self.index_path()?,
            field.to_owned(),
            request.keys.unwrap_or_default(),
            request.source_type,
        )?
        .into_iter()
        .map(|entry| FacetCount {
            key: entry.tid,
            count: entry.count,
        })
        .collect())
    }

    pub(crate) fn cache_status(&self) -> Result<CacheStatus> {
        let _operation = self.begin_operation()?;
        let status = service::index_service_status_typed(&self.index_path()?)?;
        Ok(CacheStatus {
            structural_state: status.state,
            pending: status.pending,
            documents: status.documents,
            schema_version: 1,
            schema_fingerprint: status.schema_fingerprint,
            reconciliation_timings: status.reconciliation_timings,
            last_reconciled_at: status.last_committed_at.filter(|value| *value != 0),
            error: status.error.filter(|value| !value.is_empty()),
        })
    }
}
