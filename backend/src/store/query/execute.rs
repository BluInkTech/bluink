// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use super::compile;
use crate::domain::project::query::QueryDocument;
use crate::records::items::{PublicItem, QueryItemsResult, TaskCounts};
use crate::store::Result;
use rusqlite::{Transaction, params_from_iter, types::Value};
#[cfg(test)]
use std::fmt::Write as _;

pub(crate) struct SearchOptions<'a> {
    pub(crate) content: bool,
    pub(crate) limit: u32,
    pub(crate) offset: u32,
    pub(crate) pin_scope: Option<&'a str>,
    pub(crate) prioritize_pins: bool,
    pub(crate) timezone: chrono_tz::Tz,
}

pub(crate) fn search(
    tx: &Transaction<'_>,
    document: &QueryDocument,
    options: &SearchOptions<'_>,
) -> Result<QueryItemsResult> {
    let query = compile(tx, document, options.pin_scope, options.timezone)?;
    let mut parameters = query.parameters[..query.filter_parameters].to_vec();
    let pinned = pin_expression(options, &mut parameters);
    let count_sql = format!(
        "{}SELECT count(*),{} FROM item i INDEXED BY {} WHERE {}",
        query.with(),
        pinned
            .as_ref()
            .map_or_else(|| "0".into(), |pin| format!("coalesce(sum({pin}),0)")),
        query.index,
        query.filter
    );
    let (count, pinned_count) =
        tx.query_row(&count_sql, params_from_iter(parameters.iter()), |row| {
            Ok((super::read_count(row, 0)?, super::read_count(row, 1)?))
        })?;
    let mut parameters = query.parameters.clone();
    let pinned = pin_expression(options, &mut parameters);
    let mut order = query.ordering.clone();
    if let Some(pin) = &pinned {
        order.insert(0, format!("{pin} DESC"));
    }
    parameters.push(Value::Integer(i64::from(options.limit)));
    let limit = parameters.len();
    parameters.push(Value::Integer(i64::from(options.offset)));
    let offset = parameters.len();
    let sql = format!(
        "{}SELECT i.uuid FROM {} WHERE {} ORDER BY {} LIMIT ?{limit} OFFSET ?{offset}",
        query.with(),
        query.from,
        query.filter,
        order.join(",")
    );
    let mut statement = tx.prepare(&sql)?;
    let ids = statement
        .query_map(params_from_iter(parameters.iter()), |row| {
            row.get::<_, String>(0)
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let items = if options.content {
        ids.iter()
            .map(|id| crate::store::reads::items::read_item(tx, id, true))
            .collect::<Result<_>>()?
    } else {
        cards(tx, &ids)?
    };
    Ok(QueryItemsResult {
        items,
        count,
        pinned_count: pinned.map(|_| pinned_count),
    })
}

/// Hydrate the selected page in one statement, preserving its original order.
/// Tags are sorted by their portable keys. Pin membership affects ordering but
/// is returned only by the full-item reads, as in the existing card contract.
/// A page with tasks also carries how many it has and how many are open.
fn cards(tx: &Transaction<'_>, ids: &[String]) -> Result<Vec<PublicItem>> {
    let mut stmt = tx.prepare_cached(
        "SELECT i.uuid,i.revision,i.header_json->>'$.createdAt',i.header_json->>'$.modifiedAt',
            i.header_json->'$.meta',p.preview,p.preview_image,
            (SELECT json_group_array(e.named_key ORDER BY e.named_key) FROM relation r
             JOIN entity e ON e.id=r.target_id WHERE r.source_id=i.id AND r.kind='tag'),
            (SELECT count(*) FROM task_projection t JOIN item ti ON ti.id=t.item_id
             WHERE t.note_id=i.id AND ti.state='active'),
            (SELECT count(*) FROM task_projection t JOIN item ti ON ti.id=t.item_id
             WHERE t.note_id=i.id AND ti.state='active' AND t.completed_ms IS NULL)
         FROM json_each(?1) selected JOIN item i ON i.uuid=selected.value
         LEFT JOIN item_projection p ON p.item_id=i.id ORDER BY selected.key",
    )?;
    let mut rows = stmt.query([serde_json::to_string(ids)?])?;
    let mut items = Vec::with_capacity(ids.len());
    while let Some(row) = rows.next()? {
        let tags: Vec<String> = serde_json::from_str(&row.get::<_, String>(7)?)?;
        let preview: Option<String> = row.get(5)?;
        items.push(PublicItem {
            id: row.get(0)?,
            version: row.get(1)?,
            created_at: row.get(2)?,
            modified_at: row.get(3)?,
            meta: serde_json::from_str(&row.get::<_, String>(4)?)?,
            content: serde_json::Map::new(),
            tags: (!tags.is_empty()).then_some(tags),
            pins: None,
            preview: preview.filter(|s| !s.is_empty()),
            preview_image: row.get(6)?,
            tasks: match row.get::<_, u32>(8)? {
                0 => None,
                total => Some(TaskCounts {
                    open: row.get(9)?,
                    total,
                }),
            },
        });
    }
    Ok(items)
}

/// Every match in UUID order, for membership assertions.
#[cfg(test)]
pub(crate) fn matching_ids(
    tx: &Transaction<'_>,
    document: &QueryDocument,
    pin_scope: Option<&str>,
    candidates: Option<&[String]>,
    timezone: chrono_tz::Tz,
) -> Result<Vec<String>> {
    let mut query = compile(tx, document, pin_scope, timezone)?;
    query.parameters.truncate(query.filter_parameters);
    if let Some(candidates) = candidates {
        if candidates.is_empty() {
            return Ok(Vec::new());
        }
        // A single table-valued parameter also handles a full workspace without
        // crossing SQLite's host-parameter limit or interpolating identifiers.
        query
            .parameters
            .push(Value::Text(serde_json::to_string(candidates)?));
        // Resolve UUIDs to local IDs so the ID-first header index can seek.
        let _ = write!(
            query.filter,
            " AND i.id IN (SELECT id FROM item WHERE uuid IN (SELECT value FROM json_each(?{})))",
            query.parameters.len()
        );
    }
    let sql = format!(
        "{}SELECT i.uuid FROM item i INDEXED BY item_query_header WHERE {} ORDER BY i.uuid",
        query.with(),
        query.filter
    );
    Ok(tx
        .prepare(&sql)?
        .query_map(params_from_iter(query.parameters.iter()), |row| row.get(0))?
        .collect::<rusqlite::Result<_>>()?)
}

/// Cards for the active pages among a short candidate list that match the
/// document, in first-candidate order without duplicates. Candidates drive the
/// join, so the cost follows the list rather than the project.
pub(crate) fn candidate_page_cards(
    tx: &Transaction<'_>,
    document: &QueryDocument,
    pin_scope: Option<&str>,
    candidates: &[String],
    limit: u8,
    timezone: chrono_tz::Tz,
) -> Result<Vec<PublicItem>> {
    if candidates.is_empty() {
        return Ok(Vec::new());
    }
    let mut query = compile(tx, document, pin_scope, timezone)?;
    query.parameters.truncate(query.filter_parameters);
    query
        .parameters
        .push(Value::Text(serde_json::to_string(candidates)?));
    let selected = query.parameters.len();
    query.parameters.push(Value::Integer(i64::from(limit)));
    let limit = query.parameters.len();
    let sql = format!(
        "{}SELECT i.uuid FROM json_each(?{selected}) selected CROSS JOIN item i ON i.uuid=selected.value
         WHERE {} AND i.type='page' AND i.state='active'
         GROUP BY i.id ORDER BY min(selected.key) LIMIT ?{limit}",
        query.with(),
        query.filter
    );
    let ids = tx
        .prepare(&sql)?
        .query_map(params_from_iter(query.parameters.iter()), |row| row.get(0))?
        .collect::<rusqlite::Result<Vec<String>>>()?;
    cards(tx, &ids)
}

fn pin_expression(options: &SearchOptions<'_>, parameters: &mut Vec<Value>) -> Option<String> {
    options.pin_scope.filter(|_| options.prioritize_pins).map(|scope| {
        parameters.push(Value::Text(scope.into()));
        format!("EXISTS(SELECT 1 FROM relation pin JOIN entity scope ON scope.id=pin.target_id WHERE pin.source_id=i.id AND pin.kind='pin' AND scope.uuid=?{})", parameters.len())
    })
}

/// Export discovery keeps explicit/relevance order without loading result cards.
pub(crate) fn ordered_ids(
    tx: &Transaction<'_>,
    document: &QueryDocument,
    pin_scope: Option<&str>,
    timezone: chrono_tz::Tz,
) -> Result<Vec<String>> {
    let query = compile(tx, document, pin_scope, timezone)?;
    let sql = format!(
        "{}SELECT i.uuid FROM {} WHERE {} ORDER BY {}",
        query.with(),
        query.from,
        query.filter,
        query.ordering.join(",")
    );
    Ok(tx
        .prepare(&sql)?
        .query_map(params_from_iter(query.parameters.iter()), |row| row.get(0))?
        .collect::<rusqlite::Result<_>>()?)
}
