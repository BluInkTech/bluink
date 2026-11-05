// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! A response contains complete committed generations, including deletion tombstones.

use std::collections::BTreeSet;

use rusqlite::{Transaction, params};

use serde::{Deserialize, Serialize};

use crate::domain::project::types::ItemType;

use super::{Error, Result, Store};

const BATCH_LIMIT: i64 = 64;
const DETAIL_LIMIT: usize = 512;

pub(crate) struct Change<'a> {
    pub(crate) id: &'a str,
    pub(crate) item_type: ItemType,
    pub(crate) operation: ChangeOperation,
    pub(crate) revision: Option<&'a str>,
}

/// A projection rebuild changes query answers without changing item revisions.
pub(super) fn reset_after_rebuild(tx: &Transaction<'_>) -> Result<()> {
    let domains = BTreeSet::from([
        ChangeDomain::Items,
        ChangeDomain::Fields,
        ChangeDomain::Decisions,
    ]);
    append_batch(tx, &domains, &[], true).map(|_| ())
}

/// Called in the same transaction as canonical writes and their projections.
pub(crate) fn publish(
    transaction: &Transaction<'_>,
    domains: &BTreeSet<ChangeDomain>,
    changes: &[Change<'_>],
) -> Result<i64> {
    append_batch(transaction, domains, changes, changes.len() > DETAIL_LIMIT)
}

fn append_batch(
    transaction: &Transaction<'_>,
    domains: &BTreeSet<ChangeDomain>,
    changes: &[Change<'_>],
    reset: bool,
) -> Result<i64> {
    let generation: i64 = transaction.query_row(
        "UPDATE project_state SET generation = generation + 1 WHERE singleton = 1 RETURNING generation",
        [],
        |row| row.get(0),
    )?;
    transaction.execute(
        "INSERT INTO change_batch(generation,created_ms,mode,domains_json) VALUES (?1,?2,?3,?4)",
        params![
            generation,
            chrono::Utc::now().timestamp_millis(),
            if reset { "reset" } else { "items" },
            serde_json::to_string(domains)?,
        ],
    )?;
    if !reset {
        let mut insert = transaction.prepare_cached(
            "INSERT INTO change_item(generation,item_uuid,item_type,operation,revision_uuid)
             VALUES (?1,?2,?3,?4,?5)",
        )?;
        for change in changes {
            insert.execute(params![
                generation,
                change.id,
                change.item_type.as_str(),
                match change.operation {
                    ChangeOperation::Upsert => "upsert",
                    ChangeOperation::Delete => "delete",
                },
                change.revision,
            ])?;
        }
    }
    Ok(generation)
}

impl Store {
    pub(crate) fn read_changes(&self, cursor: Option<&ChangeCursor>) -> Result<ReadChangesResult> {
        self.read(|transaction| {
            let (epoch, current, floor): (String, i64, i64) = transaction.query_row(
                "SELECT change_epoch,generation,change_floor FROM project_state WHERE singleton = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )?;
            let Some(cursor) = cursor else {
                return Ok(baseline(epoch, current, false));
            };
            let generation = parse_generation(&cursor.generation)?;
            if cursor.epoch != epoch || generation < floor || generation > current {
                return Ok(baseline(epoch, current, true));
            }
            let mut statement = transaction.prepare_cached(
                "SELECT generation,mode,domains_json FROM change_batch
                 WHERE generation > ?1 ORDER BY generation LIMIT ?2",
            )?;
            let mut rows = statement.query(params![generation, BATCH_LIMIT])?;
            let mut batches = Vec::new();
            let mut last = generation;
            while let Some(row) = rows.next()? {
                last = row.get(0)?;
                let mode: String = row.get(1)?;
                let domains: String = row.get(2)?;
                batches.push(ChangeBatch {
                    generation: last.to_string(),
                    reset: mode == "reset",
                    domains: serde_json::from_str(&domains)?,
                    items: read_items(transaction, last)?,
                });
            }
            Ok(ReadChangesResult {
                cursor: ChangeCursor { epoch, generation: last.to_string() },
                reset: false,
                batches,
                has_more: last < current,
            })
        })
    }
}

fn parse_generation(value: &str) -> Result<i64> {
    let generation = value
        .parse::<i64>()
        .map_err(|_| Error::Invalid("Invalid change cursor generation.".to_owned()))?;
    if generation < 0 || generation.to_string() != value {
        return Err(Error::Invalid(
            "Invalid change cursor generation.".to_owned(),
        ));
    }
    Ok(generation)
}

fn baseline(epoch: String, generation: i64, reset: bool) -> ReadChangesResult {
    ReadChangesResult {
        cursor: ChangeCursor {
            epoch,
            generation: generation.to_string(),
        },
        reset,
        batches: Vec::new(),
        has_more: false,
    }
}

fn read_items(transaction: &Transaction<'_>, generation: i64) -> Result<Vec<ChangedItem>> {
    let mut statement = transaction.prepare_cached(
        "SELECT item_uuid,item_type,operation FROM change_item WHERE generation = ?1 ORDER BY item_uuid",
    )?;
    let mut rows = statement.query([generation])?;
    let mut items = Vec::new();
    while let Some(row) = rows.next()? {
        let item_type: String = row.get(1)?;
        let operation: String = row.get(2)?;
        items.push(ChangedItem {
            id: row.get(0)?,
            item_type: Some(serde_json::from_value(serde_json::Value::String(
                item_type,
            ))?),
            operation: match operation.as_str() {
                "upsert" => ChangeOperation::Upsert,
                "delete" => ChangeOperation::Delete,
                _ => {
                    return Err(Error::Invalid(
                        "Invalid stored change operation.".to_owned(),
                    ));
                }
            },
        });
    }
    Ok(items)
}

const PRUNE_BATCHES: i64 = 64;

/// Remove only whole generations. A cursor at the floor can still catch up.
/// Returns whether a full step was removed, so the caller can continue.
pub(crate) fn prune(transaction: &Transaction<'_>, before_ms: i64) -> Result<bool> {
    let (floor, count): (Option<i64>, i64) = transaction.query_row(
        "SELECT max(generation),count(*) FROM (SELECT generation FROM change_batch WHERE created_ms < ?1 ORDER BY generation LIMIT ?2)",
        params![before_ms, PRUNE_BATCHES],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    if let Some(floor) = floor {
        transaction.execute("DELETE FROM change_batch WHERE generation <= ?1", [floor])?;
        transaction.execute(
            "UPDATE project_state SET change_floor = max(change_floor, ?1) WHERE singleton = 1",
            [floor],
        )?;
    }
    Ok(count >= PRUNE_BATCHES)
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub(crate) struct ChangeCursor {
    pub(crate) epoch: String,
    /// Decimal SQLite generation; strings preserve the entire integer range in JS.
    pub(crate) generation: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub(crate) struct ReadChangesRequest {
    #[cfg_attr(test, ts(optional))]
    pub(crate) cursor: Option<ChangeCursor>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(test, derive(ts_rs::TS))]
pub(crate) enum ChangeDomain {
    Items,
    Fields,
    Decisions,
    History,
    Activity,
    Settings,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(test, derive(ts_rs::TS))]
pub(crate) enum ChangeOperation {
    Upsert,
    Delete,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(test, derive(ts_rs::TS))]
pub(crate) struct ChangedItem {
    pub(crate) id: String,
    pub(crate) operation: ChangeOperation,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub(crate) item_type: Option<ItemType>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(test, derive(ts_rs::TS))]
pub(crate) struct ChangeBatch {
    pub(crate) generation: String,
    pub(crate) domains: Vec<ChangeDomain>,
    /// A reset invalidates every record in these domains, without enumerating IDs.
    pub(crate) reset: bool,
    pub(crate) items: Vec<ChangedItem>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(test, derive(ts_rs::TS))]
pub(crate) struct ReadChangesResult {
    pub(crate) cursor: ChangeCursor,
    pub(crate) reset: bool,
    pub(crate) batches: Vec<ChangeBatch>,
    pub(crate) has_more: bool,
}
