// SPDX-License-Identifier: MPL-2.0
use crate::domain::project::types::{ItemState, ItemType};
use crate::store::{Error, Result, item, project, query};
use crate::{
    domain::project::query::QueryDocument,
    records::items::{PublicItem, QueryItemsRequest, QueryItemsResult, ReadItemsRequest},
};
use rusqlite::Transaction;

pub(crate) fn read_item(tx: &Transaction<'_>, id: &str, content: bool) -> Result<PublicItem> {
    read_optional(tx, id, content)?.ok_or_else(|| Error::Invalid(format!("Item {id} not found")))
}

fn read_optional(tx: &Transaction<'_>, id: &str, content: bool) -> Result<Option<PublicItem>> {
    if !crate::domain::project::identity::is_item_id(id) {
        return Err(Error::Invalid(format!("Invalid Bluink item id: {id}")));
    }
    item::read(tx, id, content)?
        .map(|value| crate::records::items::canonical_item(value, content))
        .transpose()
}

pub(crate) fn read_items(
    tx: &Transaction<'_>,
    request: &ReadItemsRequest,
    timezone: chrono_tz::Tz,
) -> Result<Vec<PublicItem>> {
    if let Some(ids) = &request.ids {
        if request.query.is_some() || ids.len() > 512 {
            return Err(Error::Invalid(
                "Read up to 512 item IDs, without a query.".into(),
            ));
        }
        return ids
            .iter()
            .map(|id| read_optional(tx, id, request.options.content == Some(true)))
            .filter_map(std::result::Result::transpose)
            .collect();
    }
    Ok(query_items(
        tx,
        &QueryItemsRequest {
            query: request.query.clone().unwrap_or_default(),
            options: crate::records::items::ReadOptions {
                content: request.options.content,
            },
            limit: request.limit,
            offset: request.offset,
            pin_scope_id: None,
            prioritize_pins: None,
        },
        timezone,
    )?
    .items)
}

pub(crate) fn query_items(
    tx: &Transaction<'_>,
    request: &QueryItemsRequest,
    timezone: chrono_tz::Tz,
) -> Result<QueryItemsResult> {
    let limit = request.limit.unwrap_or(1000);
    if !(1..=10_000).contains(&limit) {
        return Err(Error::Invalid(
            "Choose an item page size from 1 to 10000.".into(),
        ));
    }
    query::search(
        tx,
        &request.query,
        &query::SearchOptions {
            content: request.options.content == Some(true),
            limit,
            offset: request.offset.unwrap_or(0),
            pin_scope: request.pin_scope_id.as_deref(),
            prioritize_pins: request.prioritize_pins.unwrap_or(true),
            timezone,
        },
    )
}

pub(crate) fn workspace_context(tx: &Transaction<'_>) -> Result<(String, PublicItem)> {
    let config = project::read_config(tx)?;
    let id = config["defaultWorkspaceId"]
        .as_str()
        .ok_or_else(|| Error::Invalid("Default workspace identity is missing.".into()))?;
    let workspace = read_item(tx, id, true)?;
    active_workspace(&workspace)?;
    Ok((
        config["id"]
            .as_str()
            .ok_or_else(|| Error::Invalid("Project identity is missing.".into()))?
            .to_owned(),
        workspace,
    ))
}

fn active_workspace(workspace: &PublicItem) -> Result<()> {
    if workspace.meta.item_type != Some(ItemType::Workspace)
        || workspace.meta.state != Some(ItemState::Active)
    {
        return Err(Error::Invalid("Choose an active workspace.".into()));
    }
    Ok(())
}

pub(crate) fn resolve_workspace_notes(
    tx: &Transaction<'_>,
    workspace_id: &str,
    ids: &[String],
    limit: u8,
    timezone: chrono_tz::Tz,
) -> Result<Vec<PublicItem>> {
    if ids.len() > 50 || !(1..=50).contains(&limit) {
        return Err(Error::Invalid(
            "Resolve up to 50 workspace note candidates and choose a limit from 1 to 50.".into(),
        ));
    }
    if ids
        .iter()
        .any(|id| !crate::domain::project::identity::is_item_id(id))
    {
        return Err(Error::Invalid(
            "Workspace note candidates need valid Bluink note IDs.".into(),
        ));
    }
    let workspace = read_item(tx, workspace_id, true)?;
    active_workspace(&workspace)?;
    let document: QueryDocument =
        serde_json::from_value(
            workspace.content.get("query").cloned().ok_or_else(|| {
                Error::Invalid("This workspace needs collection criteria.".into())
            })?,
        )?;
    query::candidate_page_cards(tx, &document, Some(workspace_id), ids, limit, timezone)
}
