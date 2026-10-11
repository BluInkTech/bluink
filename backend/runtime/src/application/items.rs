// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::Application;
use super::error::{Result, internal_error, invalid_request};
use crate::engine::project::types::{Metadata, NoteTask};
use crate::model::SearchHit;
use crate::search;

#[derive(Default, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(optional_fields = nullable))]
pub(crate) struct ReadOptions {
    pub(crate) content: Option<bool>,
}

#[derive(Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(optional_fields = nullable))]
pub(crate) struct ReadItemRequest {
    pub(crate) id: String,
    #[serde(default)]
    #[cfg_attr(test, ts(as = "Option<ReadOptions>", optional))]
    pub(crate) options: ReadOptions,
}

#[derive(Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(optional_fields = nullable))]
pub(crate) struct ReadItemsRequest {
    pub(crate) ids: Option<Vec<String>>,
    pub(crate) query: Option<String>,
    #[serde(default)]
    #[cfg_attr(test, ts(as = "Option<ReadOptions>", optional))]
    pub(crate) options: ReadOptions,
    pub(crate) limit: Option<u32>,
    pub(crate) offset: Option<u32>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(optional_fields = nullable))]
pub(crate) struct QueryItemsRequest {
    pub(crate) query: String,
    #[serde(default)]
    #[cfg_attr(test, ts(as = "Option<ReadOptions>", optional))]
    pub(crate) options: ReadOptions,
    pub(crate) limit: Option<u32>,
    pub(crate) offset: Option<u32>,
    pub(crate) pin_scope_id: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(optional_fields))]
pub(crate) struct PublicItem {
    pub(crate) id: String,
    #[serde(rename(deserialize = "revision"))]
    pub(crate) version: String,
    pub(crate) created_at: String,
    pub(crate) modified_at: String,
    pub(crate) meta: Metadata,
    #[cfg_attr(
        test,
        ts(as = "std::collections::BTreeMap<String, crate::engine::project::types::OpaqueJson>")
    )]
    pub(crate) content: serde_json::Map<String, Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub(crate) tags: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub(crate) pins: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub(crate) tasks: Option<Vec<NoteTask>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub(crate) preview: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub(crate) preview_image: Option<String>,
}

#[derive(Debug, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub(crate) struct QueryItemsResult {
    pub(crate) items: Vec<PublicItem>,
    pub(crate) count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub(crate) pinned_count: Option<usize>,
}

pub(crate) fn canonical_item(value: Value, with_content: bool) -> Result<PublicItem> {
    let mut item: PublicItem = serde_json::from_value(value).map_err(internal_error)?;
    if !with_content {
        item.content = serde_json::Map::new();
    }
    Ok(item)
}

fn indexed_item(hit: SearchHit) -> Result<PublicItem> {
    Ok(PublicItem {
        id: hit.id,
        version: hit.revision,
        created_at: hit.created_at,
        modified_at: hit.modified_at,
        meta: serde_json::from_value(hit.metadata).map_err(internal_error)?,
        content: serde_json::Map::new(),
        tags: (!hit.tags.is_empty()).then_some(hit.tags),
        pins: None,
        tasks: None,
        preview: (!hit.preview.is_empty()).then_some(hit.preview),
        preview_image: hit.preview_image,
    })
}

fn read_canonical(root: &Path, id: &str, with_content: bool) -> Result<Option<PublicItem>> {
    if !crate::engine::project::item::is_item_id(id) {
        return Err(invalid_request(format!("Invalid Bluink item id: {id}")));
    }
    crate::engine::project::item::read_project_item(root, id)
        .map_err(internal_error)?
        .map(|value| canonical_item(value, with_content))
        .transpose()
}

impl Application {
    #[tracing::instrument(level = "debug", skip_all)]
    #[expect(
        clippy::needless_pass_by_value,
        reason = "The service consumes its decoded request or owned work at the application boundary."
    )]
    pub(crate) fn read_item(&self, request: ReadItemRequest) -> Result<PublicItem> {
        let _operation = self.begin_operation()?;
        read_canonical(
            &self.root,
            &request.id,
            request.options.content != Some(false),
        )?
        .ok_or_else(|| invalid_request(format!("Item {} not found", request.id)))
    }

    pub(crate) fn read_items(&self, request: ReadItemsRequest) -> Result<Vec<PublicItem>> {
        let _operation = self.begin_operation()?;
        if let Some(ids) = request.ids {
            if request.query.is_some() || ids.len() > 512 {
                return Err(invalid_request("Read up to 512 item IDs, without a query."));
            }
            return ids
                .into_iter()
                .map(|id| read_canonical(&self.root, &id, request.options.content == Some(true)))
                .filter_map(std::result::Result::transpose)
                .collect();
        }
        Ok(self
            .query_items_inner(QueryItemsRequest {
                query: request.query.unwrap_or_default(),
                options: request.options,
                limit: request.limit,
                offset: request.offset,
                pin_scope_id: None,
            })?
            .items)
    }

    pub(crate) fn query_items(&self, request: QueryItemsRequest) -> Result<QueryItemsResult> {
        let _operation = self.begin_operation()?;
        self.query_items_inner(request)
    }

    fn query_items_inner(&self, request: QueryItemsRequest) -> Result<QueryItemsResult> {
        let limit = request.limit.unwrap_or(1000);
        if limit == 0 {
            return Err(invalid_request("Choose a positive item page size."));
        }
        let found = search::search_service_sync(
            self.index_path()?,
            request.query,
            limit,
            request.offset.unwrap_or(0),
            request.pin_scope_id,
        )?;
        let items = found
            .hits
            .into_iter()
            .map(|hit| {
                if request.options.content == Some(true)
                    && let Some(canonical) = read_canonical(&self.root, &hit.id, true)?
                {
                    return Ok(canonical);
                }
                indexed_item(hit)
            })
            .collect::<Result<_>>()?;
        Ok(QueryItemsResult {
            items,
            count: found.count,
            pinned_count: found.pinned_count,
        })
    }
}
