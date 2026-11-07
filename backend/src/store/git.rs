// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! Durable portable mappings, exact checkpoint captures and ref acknowledgements.
//! Git object construction happens after capture, outside the SQLite writer.

use std::{collections::BTreeMap, path::Path};

use rusqlite::{OptionalExtension, Transaction, params};
use serde_json::Value;

use crate::domain::project::identity::ItemId;

use super::{Error, Result, item};

pub(crate) const SCHEMA: &str = include_str!("git_schema.sql");
pub(crate) mod history;
pub(crate) mod inventory;

#[cfg(test)]
mod tests;

#[derive(Clone, Debug)]
pub(crate) struct Capture {
    pub(crate) id: String,
    pub(crate) expected_head: Option<String>,
}

#[derive(Debug)]
pub(crate) enum CapturedSource {
    Deleted,
    Json(Value),
    Asset { oid: String, size: u64 },
}

#[derive(Clone, Debug)]
pub(crate) struct Operation {
    pub(crate) id: String,
    /// Only checkpoints capture dirty paths; an accepted sync or restore has none.
    pub(crate) capture_id: Option<String>,
    pub(crate) expected_head: Option<String>,
    pub(crate) candidate_head: String,
    pub(crate) references: BTreeMap<String, String>,
}

fn invalid(message: impl Into<String>) -> Error {
    Error::Invalid(message.into())
}

fn portable_path(path: &str) -> Result<()> {
    crate::versioning::checkpoints::validate_path(path).map_err(|error| invalid(error.to_string()))
}

pub(crate) fn expected_head(transaction: &Transaction<'_>) -> Result<Option<String>> {
    Ok(transaction.query_row(
        "SELECT expected_head FROM git_state WHERE singleton=1",
        [],
        |row| row.get(0),
    )?)
}

/// Call in the transaction installing this exact canonical revision.
pub(crate) fn mark_item_dirty(
    transaction: &Transaction<'_>,
    uuid: &str,
    revision: &str,
    generation: i64,
) -> Result<()> {
    let identity = ItemId::try_from(uuid).map_err(invalid)?;
    let path = identity
        .path(Path::new(""))
        .to_string_lossy()
        .replace('\\', "/");
    let id: i64 = transaction
        .query_row(
            "SELECT id FROM item WHERE uuid=?1 AND revision=?2",
            params![uuid, revision],
            |row| row.get(0),
        )
        .optional()?
        .ok_or(Error::RevisionConflict)?;
    transaction.execute("INSERT INTO dirty_path(path,generation,operation,source_kind,item_id,revision_uuid)
        VALUES (?1,?2,'upsert','item',?3,?4) ON CONFLICT(path) DO UPDATE SET
        generation=excluded.generation,operation=excluded.operation,source_kind=excluded.source_kind,
        item_id=excluded.item_id,revision_uuid=excluded.revision_uuid", params![path,generation,id,revision])?;
    Ok(())
}

/// The old source may already be deleted; the portable delete remains durable.
pub(crate) fn mark_deleted(
    transaction: &Transaction<'_>,
    path: &str,
    generation: i64,
) -> Result<()> {
    portable_path(path)?;
    transaction.execute("INSERT INTO dirty_path(path,generation,operation,source_kind)
        VALUES (?1,?2,'delete','record') ON CONFLICT(path) DO UPDATE SET
        generation=excluded.generation,operation='delete',source_kind='record',item_id=NULL,revision_uuid=NULL",
        params![path,generation])?;
    Ok(())
}

pub(crate) fn put_record(
    transaction: &Transaction<'_>,
    path: &str,
    value: &Value,
    revision: &str,
    generation: i64,
) -> Result<()> {
    crate::versioning::checkpoints::validate_record_path(path)
        .map_err(|error| invalid(error.to_string()))?;
    ItemId::try_from(revision).map_err(invalid)?;
    if path == "bluink.json" {
        let config =
            crate::domain::project::config::parse_project_config(&serde_json::to_vec(value)?)
                .map_err(invalid)?;
        let project: String =
            transaction.query_row("SELECT project_uuid FROM project_state", [], |row| {
                row.get(0)
            })?;
        if config["id"].as_str() != Some(project.as_str()) {
            return Err(invalid("Project settings belong to another database."));
        }
    }
    transaction.execute("INSERT INTO project_record(path,canonical_json,revision,generation) VALUES (?1,?2,?3,?4)
        ON CONFLICT(path) DO UPDATE SET canonical_json=excluded.canonical_json,revision=excluded.revision,generation=excluded.generation",
        params![path,serde_json::to_string(value)?,revision,generation])?;
    mark_mapping(transaction, path, "record", generation)
}

/// A `VerifiedAsset` is constructed only after verified durable object installation.
pub(crate) fn put_asset(
    transaction: &Transaction<'_>,
    path: &str,
    owner: Option<i64>,
    asset: &crate::versioning::checkpoints::VerifiedAsset,
    generation: i64,
) -> Result<()> {
    crate::versioning::checkpoints::validate_asset_path(path)
        .map_err(|error| invalid(error.to_string()))?;
    if path.starts_with("assets/") {
        let owner = owner.ok_or_else(|| invalid("An item attachment requires its owner."))?;
        let uuid: String =
            transaction.query_row("SELECT uuid FROM entity WHERE id=?1", [owner], |row| {
                row.get(0)
            })?;
        if path.split('/').nth(2) != Some(uuid.as_str()) {
            return Err(invalid("Attachment path does not match its owner."));
        }
    } else if owner.is_some() {
        return Err(invalid("Theme attachments cannot have an item owner."));
    }
    let size = i64::try_from(asset.size())
        .map_err(|_| invalid("Attachment exceeds SQLite size range."))?;
    let previous: Option<i64> = transaction
        .query_row(
            "SELECT size FROM asset WHERE oid=?1",
            [asset.oid()],
            |row| row.get(0),
        )
        .optional()?;
    if previous.is_some_and(|previous| previous != size) {
        return Err(invalid("Attachment identity has inconsistent size."));
    }
    transaction.execute(
        "INSERT INTO asset(oid,size,local_available) VALUES (?1,?2,1)
        ON CONFLICT(oid) DO UPDATE SET local_available=1",
        params![asset.oid(), size],
    )?;
    transaction.execute("INSERT INTO asset_path(path,owner_id,oid,generation) VALUES (?1,?2,?3,?4)
        ON CONFLICT(path) DO UPDATE SET owner_id=excluded.owner_id,oid=excluded.oid,generation=excluded.generation",
        params![path,owner,asset.oid(),generation])?;
    mark_mapping(transaction, path, "asset", generation)
}

fn mark_mapping(
    transaction: &Transaction<'_>,
    path: &str,
    kind: &str,
    generation: i64,
) -> Result<()> {
    transaction.execute("INSERT INTO dirty_path(path,generation,operation,source_kind) VALUES (?1,?2,'upsert',?3)
        ON CONFLICT(path) DO UPDATE SET generation=excluded.generation,operation='upsert',source_kind=excluded.source_kind,item_id=NULL,revision_uuid=NULL",
        params![path,generation,kind])?;
    Ok(())
}

/// Freeze only small headers and immutable references; never assemble document bodies.
pub(crate) fn capture(
    transaction: &Transaction<'_>,
    id: &str,
    head: Option<&str>,
    now_ms: i64,
) -> Result<Capture> {
    ItemId::try_from(id).map_err(invalid)?;
    if expected_head(transaction)?.as_deref() != head {
        return Err(invalid("Git head changed before capture."));
    }
    let stale:bool=transaction.query_row("SELECT EXISTS(SELECT 1 FROM dirty_path d LEFT JOIN item i ON i.id=d.item_id
        WHERE d.operation='upsert' AND d.source_kind='item' AND (i.id IS NULL OR i.revision<>d.revision_uuid))",[],|row|row.get(0))?;
    if stale {
        return Err(invalid(
            "Dirty item revision no longer matches canonical state.",
        ));
    }
    let generation: i64 = transaction.query_row(
        "SELECT generation FROM project_state WHERE singleton=1",
        [],
        |row| row.get(0),
    )?;
    transaction.execute(
        "INSERT INTO capture(id,expected_head,generation,created_ms) VALUES (?1,?2,?3,?4)",
        params![id, head, generation, now_ms],
    )?;
    transaction.execute("INSERT INTO capture_item(capture_id,path,generation,operation,source_kind,item_id,revision_uuid,
        record_json,object_oid)
        SELECT ?1,d.path,d.generation,d.operation,d.source_kind,d.item_id,d.revision_uuid,p.canonical_json,a.oid
        FROM dirty_path d
        LEFT JOIN project_record p ON p.path=d.path AND d.source_kind='record' AND d.operation='upsert'
        LEFT JOIN asset_path a ON a.path=d.path AND d.source_kind='asset' AND d.operation='upsert'",[id])?;
    transaction.execute("INSERT INTO revision_pin(owner_kind,owner_id,item_id,revision_uuid)
        SELECT 'git_capture',capture_id,item_id,revision_uuid FROM capture_item WHERE capture_id=?1 AND item_id IS NOT NULL",[id])?;
    transaction.execute("INSERT INTO object_pin(owner_kind,owner_id,object_kind,object_id)
        SELECT DISTINCT 'git_capture',capture_id,'lfs',object_oid FROM capture_item WHERE capture_id=?1 AND object_oid IS NOT NULL",[id])?;
    Ok(Capture {
        id: id.to_owned(),
        expected_head: head.map(str::to_owned),
    })
}

pub(crate) fn captured_paths(transaction: &Transaction<'_>, capture: &str) -> Result<Vec<String>> {
    Ok(transaction
        .prepare_cached("SELECT path FROM capture_item WHERE capture_id=?1 ORDER BY path")?
        .query_map([capture], |row| row.get(0))?
        .collect::<rusqlite::Result<_>>()?)
}

/// Read one pinned source under a short read snapshot. The full book is never held
/// under the writer or assembled in memory at once during object construction.
pub(crate) fn captured_source(
    transaction: &Transaction<'_>,
    capture: &str,
    path: &str,
) -> Result<CapturedSource> {
    let row = transaction.query_row(
        "SELECT c.operation,c.source_kind,e.uuid,c.revision_uuid,c.record_json,c.object_oid
        FROM capture_item c LEFT JOIN entity e ON e.id=c.item_id WHERE c.capture_id=?1 AND c.path=?2",
        params![capture, path],
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, Option<String>>(5)?,
            ))
        },
    )?;
    if row.0 == "delete" {
        return Ok(CapturedSource::Deleted);
    }
    match row.1.as_str() {
        // The capture's revision pin keeps this revision readable, current or superseded.
        "item" => {
            let (Some(uuid), Some(revision)) = (row.2, row.3) else {
                return Err(invalid("Capture revision missing."));
            };
            item::read_revision(transaction, &uuid, &revision)?
                .map(CapturedSource::Json)
                .ok_or_else(|| invalid("Captured revision is no longer retained."))
        }
        "record" => Ok(CapturedSource::Json(serde_json::from_str(
            &row.4.ok_or_else(|| invalid("Capture record missing."))?,
        )?)),
        "asset" => {
            let oid = row.5.ok_or_else(|| invalid("Capture object missing."))?;
            let size: i64 =
                transaction.query_row("SELECT size FROM asset WHERE oid=?1", [&oid], |row| {
                    row.get(0)
                })?;
            Ok(CapturedSource::Asset {
                oid,
                size: u64::try_from(size).map_err(|_| invalid("Invalid object size."))?,
            })
        }
        _ => Err(invalid("Unknown capture source.")),
    }
}

fn insert_operation(
    transaction: &Transaction<'_>,
    capture: Option<&str>,
    expected: Option<&str>,
    candidate: &str,
    references: &BTreeMap<String, String>,
) -> Result<Operation> {
    if expected_head(transaction)?.as_deref() != expected {
        return Err(invalid("Git head changed during checkpoint construction."));
    }
    let id = uuid::Uuid::now_v7().to_string();
    transaction.execute(
        "INSERT INTO git_operation(id,capture_id,expected_head,candidate_head,recovery_json)
         VALUES (?1,?2,?3,?4,?5)",
        params![
            id,
            capture,
            expected,
            candidate,
            serde_json::to_string(references)?
        ],
    )?;
    for oid in std::iter::once(candidate).chain(references.values().map(String::as_str)) {
        transaction.execute("INSERT INTO object_pin(owner_kind,owner_id,object_kind,object_id) VALUES ('git_operation',?1,'git',?2) ON CONFLICT DO NOTHING",params![id,oid])?;
    }
    Ok(Operation {
        id,
        capture_id: capture.map(str::to_owned),
        expected_head: expected.map(str::to_owned),
        candidate_head: candidate.to_owned(),
        references: references.clone(),
    })
}

pub(crate) fn prepare_checkpoint(
    transaction: &Transaction<'_>,
    capture: &Capture,
    candidate: &str,
) -> Result<Operation> {
    insert_operation(
        transaction,
        Some(&capture.id),
        capture.expected_head.as_deref(),
        candidate,
        &BTreeMap::new(),
    )
}

/// An incoming candidate may replace canonical state only while every
/// acknowledged save is already part of the expected head.
pub(crate) fn begin_transition(transaction: &Transaction<'_>, head: &str) -> Result<()> {
    if expected_head(transaction)?.as_deref() != Some(head)
        || transaction.query_row("SELECT EXISTS(SELECT 1 FROM dirty_path)", [], |row| {
            row.get::<_, bool>(0)
        })?
    {
        return Err(Error::RevisionConflict);
    }
    Ok(())
}

/// Call after applying the candidate's complete delta in the transaction that
/// called `begin_transition`. Every dirty path then came from that delta and is
/// already in the candidate tree, so no capture is needed: acknowledgement only
/// advances the head. A later save records a new dirty path and survives.
pub(crate) fn prepare_transition(
    transaction: &Transaction<'_>,
    head: &str,
    candidate: &str,
    refs: &BTreeMap<String, String>,
) -> Result<Operation> {
    transaction.execute("DELETE FROM dirty_path", [])?;
    insert_operation(transaction, None, Some(head), candidate, refs)
}

pub(crate) fn pending_operations(transaction: &Transaction<'_>) -> Result<Vec<Operation>> {
    let mut statement = transaction.prepare_cached(
        "SELECT id,capture_id,expected_head,candidate_head,recovery_json FROM git_operation ORDER BY id",
    )?;
    let mut rows = statement.query([])?;
    let mut operations = Vec::new();
    while let Some(row) = rows.next()? {
        operations.push(Operation {
            id: row.get(0)?,
            capture_id: row.get(1)?,
            expected_head: row.get(2)?,
            candidate_head: row.get(3)?,
            references: serde_json::from_str(&row.get::<_, String>(4)?)?,
        });
    }
    Ok(operations)
}

/// Under the history lease, an unprepared checkpoint capture belongs to an
/// interrupted builder. No ref can reference it, and the dirty paths still exist.
pub(crate) fn discard_unprepared_captures(transaction: &Transaction<'_>) -> Result<()> {
    for table in ["revision_pin", "object_pin"] {
        transaction.execute(&format!("DELETE FROM {table} WHERE owner_kind='git_capture' AND owner_id IN (
            SELECT c.id FROM capture c WHERE c.kind='checkpoint' AND NOT EXISTS(SELECT 1 FROM git_operation o WHERE o.capture_id=c.id))"), [])?;
    }
    transaction.execute("DELETE FROM capture WHERE kind='checkpoint' AND NOT EXISTS(SELECT 1 FROM git_operation o WHERE o.capture_id=capture.id)", [])?;
    Ok(())
}

/// Run only after observing candidate at the Git ref. Newer saves survive because
/// acknowledgement deletes only the path generations frozen in this capture.
/// Completed bookkeeping has no reader, so it is pruned with its pins.
pub(crate) fn acknowledge(
    transaction: &Transaction<'_>,
    operation: &Operation,
    observed_head: &str,
) -> Result<()> {
    if observed_head != operation.candidate_head {
        return Err(invalid("Git ref does not match the prepared checkpoint."));
    }
    let expected = expected_head(transaction)?;
    if expected.as_deref() != operation.expected_head.as_deref()
        && expected.as_deref() != Some(observed_head)
    {
        return Err(invalid("Unexpected checkpoint acknowledgement base."));
    }
    transaction.execute(
        "UPDATE git_state SET expected_head=?1 WHERE singleton=1",
        [observed_head],
    )?;
    if let Some(capture) = &operation.capture_id {
        transaction.execute(
            "DELETE FROM dirty_path WHERE EXISTS(SELECT 1 FROM capture_item c
            WHERE c.capture_id=?1 AND c.path=dirty_path.path AND c.generation=dirty_path.generation)",
            [capture],
        )?;
        for table in ["revision_pin", "object_pin"] {
            transaction.execute(
                &format!("DELETE FROM {table} WHERE owner_kind='git_capture' AND owner_id=?1"),
                [capture],
            )?;
        }
    }
    transaction.execute(
        "DELETE FROM object_pin WHERE owner_kind='git_operation' AND owner_id=?1",
        [&operation.id],
    )?;
    transaction.execute("DELETE FROM git_operation WHERE id=?1", [&operation.id])?;
    if let Some(capture) = &operation.capture_id {
        // Captured members cascade with their header.
        transaction.execute("DELETE FROM capture WHERE id=?1", [capture])?;
    }
    Ok(())
}
