// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use std::collections::HashSet;
use std::path::Path;

use crate::search;

pub(crate) fn selected_ids(
    index_path: &str,
    query: Option<&str>,
) -> crate::application::error::Result<Option<HashSet<String>>> {
    let Some(query) = query.filter(|query| !query.is_empty()) else {
        return Ok(None);
    };
    let ids = search::matching_item_ids_service_sync(index_path.to_owned(), query.to_owned())?;
    Ok(Some(ids.into_iter().collect()))
}

pub(crate) fn completed_calendar(
    root: &Path,
    index_path: &str,
    start_date: &str,
    end_date: &str,
    granularity: &str,
    query: Option<&str>,
) -> crate::application::error::Result<crate::model::CalendarResults> {
    let selected = selected_ids(index_path, query)?;
    let value = crate::engine::project::reports::completed_calendar_buckets(
        root,
        start_date,
        end_date,
        granularity,
        selected.as_ref(),
    )
    .map_err(crate::application::error::internal_error)?;
    serde_json::from_value(value).map_err(crate::application::error::internal_error)
}
