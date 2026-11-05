// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use serde::{Deserialize, Serialize};

use super::Application;
use super::error::Result;
use crate::catalogue;
use crate::domain::project::types::{CalendarGranularity, FacetKind, TaskStatus};
use crate::records::{CalendarResults, PassageRecord, ResultPage, TaskRecord};
use crate::store::{query, reads};
use crate::structured_query::document::QueryDocument;

#[derive(Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(optional_fields = nullable))]
pub(crate) struct EmptyRequest {}

#[derive(Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
#[cfg_attr(test, ts(optional_fields = nullable))]
pub(crate) struct QueryTasksRequest {
    query: Option<QueryDocument>,
    pin_scope_id: Option<String>,
    note_id: Option<String>,
    /// Conditions on the tasks themselves, applied after the page scope.
    task_query: Option<QueryDocument>,
    status: TaskStatus,
    limit: Option<u32>,
    offset: Option<u32>,
}

#[derive(Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
#[cfg_attr(test, ts(optional_fields = nullable))]
pub(crate) struct QueryPassagesRequest {
    label: String,
    query: Option<QueryDocument>,
    pin_scope_id: Option<String>,
    limit: Option<u32>,
    offset: Option<u32>,
}

#[derive(Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
#[cfg_attr(test, ts(optional_fields = nullable))]
pub(crate) struct CalendarRequest {
    query: Option<QueryDocument>,
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
pub(crate) struct StorageStatus {
    project_id: String,
    schema_version: u32,
    generation: String,
    item_count: usize,
    pending_checkpoint_paths: usize,
}

#[expect(
    clippy::needless_pass_by_value,
    reason = "RPC dispatch takes ownership of each decoded request."
)]
impl Application {
    pub(crate) fn query_tasks(&self, request: QueryTasksRequest) -> Result<ResultPage<TaskRecord>> {
        let _operation = self.begin_operation()?;
        self.database()?
            .read(|tx| {
                reads::collections::tasks(
                    tx,
                    &reads::collections::Scope {
                        document: request.query.as_ref(),
                        pin_scope_id: request.pin_scope_id.as_deref(),
                        timezone: reads::timezone(tx)?,
                        limit: request.limit.unwrap_or(50),
                        offset: request.offset.unwrap_or(0),
                    },
                    request.note_id.as_deref(),
                    request.task_query.as_ref(),
                    request.status,
                )
            })
            .map_err(Into::into)
    }
    pub(crate) fn query_passages(
        &self,
        request: QueryPassagesRequest,
    ) -> Result<ResultPage<PassageRecord>> {
        let _operation = self.begin_operation()?;
        self.database()?
            .read(|tx| {
                reads::collections::passages(
                    tx,
                    &reads::collections::Scope {
                        document: request.query.as_ref(),
                        pin_scope_id: request.pin_scope_id.as_deref(),
                        timezone: reads::timezone(tx)?,
                        limit: request.limit.unwrap_or(50),
                        offset: request.offset.unwrap_or(0),
                    },
                    &request.label,
                )
            })
            .map_err(Into::into)
    }
    pub(crate) fn calendar_buckets(&self, request: CalendarRequest) -> Result<CalendarResults> {
        let _operation = self.begin_operation()?;
        self.database()?
            .read(|tx| {
                reads::calendar::buckets(
                    tx,
                    &reads::collections::Scope {
                        document: request.query.as_ref(),
                        pin_scope_id: request.pin_scope_id.as_deref(),
                        timezone: reads::timezone(tx)?,
                        limit: 1,
                        offset: 0,
                    },
                    &request.field,
                    &request.start_date,
                    &request.end_date,
                    request.granularity,
                )
            })
            .map_err(Into::into)
    }
    pub(crate) fn metadata_fields(&self) -> Result<Vec<catalogue::MetadataField>> {
        let _operation = self.begin_operation()?;
        self.database()?
            .read(reads::catalogue::metadata_fields)
            .map_err(Into::into)
    }
    pub(crate) fn list_facets(
        &self,
        request: ListFacetsRequest,
    ) -> Result<Vec<catalogue::FacetEntry>> {
        let _operation = self.begin_operation()?;
        self.database()?
            .list_facets(request.kind)
            .map_err(Into::into)
    }
    pub(crate) fn facet_counts(&self, request: FacetCountsRequest) -> Result<Vec<FacetCount>> {
        let _operation = self.begin_operation()?;
        let keys = request.keys.unwrap_or_default();
        self.database()?
            .read(|tx| {
                Ok(query::facet_counts(
                    tx,
                    request.kind.as_str(),
                    &keys,
                    request.source_type.as_deref(),
                )?
                .into_iter()
                .map(|value| FacetCount {
                    key: value.tid,
                    count: value.count,
                })
                .collect())
            })
            .map_err(Into::into)
    }
    pub(crate) fn storage_status(&self) -> Result<StorageStatus> {
        let _operation = self.begin_operation()?;
        self.database()?.read(|tx| {
            let (project_id,generation)=tx.query_row("SELECT project_uuid,CAST(generation AS TEXT) FROM project_state WHERE singleton=1",[],|row|Ok((row.get(0)?,row.get(1)?)))?;
            let (items,dirty):(i64,i64)=tx.query_row("SELECT (SELECT count(*) FROM item),(SELECT count(*) FROM dirty_path)",[],|row|Ok((row.get(0)?,row.get(1)?)))?;
            let count=|n|usize::try_from(n).map_err(|error:std::num::TryFromIntError|crate::store::Error::Invalid(error.to_string()));
            Ok(StorageStatus{project_id,generation,schema_version:tx.pragma_query_value(None,"user_version",|row|row.get(0))?,item_count:count(items)?,pending_checkpoint_paths:count(dirty)?})
        }).map_err(Into::into)
    }
}
