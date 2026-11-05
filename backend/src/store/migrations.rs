// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! Version one has no predecessors. Future upgrades are ordered, forward-only,
//! protected by a verified private snapshot, and atomic before startup opens UI.
use super::{Error, Result, SCHEMA_VERSION, location, snapshot};
use crate::{
    store::io::{self, LockMode},
    versioning::checkpoints,
};
use rusqlite::{Connection, OpenFlags, OptionalExtension, Transaction, TransactionBehavior};
use std::path::Path;

// Schema one has no supported predecessor. Add a contiguous step here when a
// subsequent release first upgrades existing canonical author data.
const STEPS: &[Step] = &[];

/// Inspect in read-only mode before opening a writer or creating a backup.
pub(super) fn before_open(root: &Path, path: &Path) -> Result<()> {
    upgrade(root, path, SCHEMA_VERSION, STEPS)
}

fn upgrade(root: &Path, path: &Path, target: i64, steps: &[Step]) -> Result<()> {
    let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let current =
        connection.pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))?;
    if current == target {
        return Ok(());
    }
    let chain = forward_chain(current, target, steps)?;
    // Resume a previously verified migration snapshot. A missing or damaged
    // protected snapshot fails closed and remains available for recovery.
    let pending = connection.query_row(
        "SELECT id,destination FROM maintenance_pin WHERE kind='migration' ORDER BY created_ms DESC LIMIT 1",
        [],
        |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
    ).optional()?;
    drop(connection);
    let prepared = if let Some((pin, path)) = pending {
        let saved = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        Prepared {
            snapshot: snapshot::verify_schema(&saved, &pin, current)?,
            path: path.into(),
        }
    } else {
        let directory = root.join(".backups");
        std::fs::create_dir_all(&directory)?;
        let destination = directory.join(format!(
            "schema-{current}-to-{target}-{}.sqlite3",
            uuid::Uuid::now_v7()
        ));
        prepare(root, &destination, chain, target)?
    };
    apply(root, &prepared, chain, target)
}

fn forward_chain(current: i64, target: i64, steps: &[Step]) -> Result<&[Step]> {
    let Some(start) = steps.iter().position(|step| step.from == current) else {
        return Err(Error::Invalid(format!(
            "Unsupported project database schema {current}; this build supports {target}."
        )));
    };
    let chain = &steps[start..];
    validate_steps(current, target, chain)?;
    Ok(chain)
}

pub(super) fn require_current(connection: &Connection) -> Result<()> {
    let version =
        connection.pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))?;
    if version != SCHEMA_VERSION {
        return Err(Error::Invalid(format!(
            "Unsupported project database schema {version}; this build supports {SCHEMA_VERSION}."
        )));
    }
    Ok(())
}

pub(crate) struct Step {
    pub(crate) from: i64,
    pub(crate) to: i64,
    pub(crate) apply: fn(&Transaction<'_>) -> Result<()>,
}

/// This private proof can only be obtained after verifying the on-disk snapshot.
pub(crate) struct Prepared {
    snapshot: snapshot::Snapshot,
    path: std::path::PathBuf,
}

/// Called with project operations quiesced during upgrade setup. If another
/// process writes afterward, apply refuses the stale snapshot without upgrading.
pub(crate) fn prepare(
    root: &Path,
    destination: &Path,
    steps: &[Step],
    target: i64,
) -> Result<Prepared> {
    let root = location::validate(root)?;
    let history = checkpoints::open(&root).map_err(|error| Error::Invalid(error.to_string()))?;
    let _lease = io::lock_file(&history.path().join("bluink.lock"), LockMode::Exclusive)
        .map_err(|error| Error::Invalid(format!("{error:#}")))?;
    let path = root.join(super::DATABASE_NAME);
    let mut connection = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
    let version = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
    validate_steps(version, target, steps)?;
    let pin = uuid::Uuid::now_v7().to_string();
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    transaction.execute(
        "INSERT INTO maintenance_pin(id,kind,destination,created_ms) VALUES (?1,'migration',?2,?3)",
        rusqlite::params![
            pin,
            destination.to_string_lossy(),
            chrono::Utc::now().timestamp_millis()
        ],
    )?;
    transaction.commit()?;
    let saved = snapshot::create_database(&path, &pin, destination, version)?;
    let copy = Connection::open_with_flags(destination, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    if snapshot::verify_schema(&copy, &pin, version)? != saved {
        return Err(Error::Invalid(
            "Migration snapshot verification changed.".to_owned(),
        ));
    }
    Ok(Prepared {
        snapshot: saved,
        path: destination.to_owned(),
    })
}

/// Database-only migrations never rewrite Git refs or external objects. Their
/// old snapshot and durable global object pin remain available on every failure.
/// New migration steps must be explicitly supplied by the supported release.
pub(crate) fn apply(root: &Path, prepared: &Prepared, steps: &[Step], target: i64) -> Result<()> {
    let root = location::validate(root)?;
    let history = checkpoints::open(&root).map_err(|error| Error::Invalid(error.to_string()))?;
    let _lease = io::lock_file(&history.path().join("bluink.lock"), LockMode::Exclusive)
        .map_err(|error| Error::Invalid(format!("{error:#}")))?;
    let old = Connection::open_with_flags(&prepared.path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    if snapshot::verify_schema(
        &old,
        &prepared.snapshot.maintenance_pin,
        prepared.snapshot.schema_version,
    )? != prepared.snapshot
    {
        return Err(Error::Invalid(
            "Migration snapshot no longer matches its recorded state.".to_owned(),
        ));
    }
    let mut connection = Connection::open_with_flags(
        root.join(super::DATABASE_NAME),
        OpenFlags::SQLITE_OPEN_READ_WRITE,
    )?;
    connection.pragma_update(None, "foreign_keys", true)?;
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Exclusive)?;
    let current: i64 = transaction.pragma_query_value(None, "user_version", |row| row.get(0))?;
    validate_steps(current, target, steps)?;
    verify_preconditions(&transaction, prepared)?;
    if git_state(&transaction)? != git_state(&old)? {
        return Err(Error::Invalid(
            "Git history changed after the migration snapshot; retry preparation.".to_owned(),
        ));
    }
    for step in steps {
        (step.apply)(&transaction)?;
        transaction.pragma_update(None, "user_version", step.to)?;
    }
    snapshot::verify_schema(&transaction, &prepared.snapshot.maintenance_pin, target)?;
    // Publish the verified schema and release its protection atomically. A
    // crash leaves either the old schema and pin, or the completed upgrade.
    transaction.execute(
        "DELETE FROM maintenance_pin WHERE id=?1",
        [&prepared.snapshot.maintenance_pin],
    )?;
    transaction.commit()?;
    Ok(())
}

fn validate_steps(current: i64, target: i64, steps: &[Step]) -> Result<()> {
    let mut expected = current;
    for step in steps {
        if step.from != expected || step.to.checked_sub(1) != Some(step.from) {
            return Err(Error::Invalid(
                "Schema migrations must be contiguous forward steps.".to_owned(),
            ));
        }
        expected = step.to;
    }
    if current < 1 || target < current || expected != target {
        return Err(Error::Invalid(
            "No supported forward migration path exists for this schema.".to_owned(),
        ));
    }
    Ok(())
}

fn verify_preconditions(transaction: &Transaction<'_>, prepared: &Prepared) -> Result<()> {
    let live = snapshot::verify_schema(
        transaction,
        &prepared.snapshot.maintenance_pin,
        prepared.snapshot.schema_version,
    )?;
    if live != prepared.snapshot {
        return Err(Error::Invalid(
            "Project changed after its migration snapshot; retry upgrade preparation.".to_owned(),
        ));
    }
    let migration: bool = transaction.query_row(
        "SELECT EXISTS(SELECT 1 FROM maintenance_pin WHERE id=?1 AND kind='migration')",
        [&prepared.snapshot.maintenance_pin],
        |row| row.get(0),
    )?;
    if !migration {
        return Err(Error::Invalid(
            "Migration protection is missing.".to_owned(),
        ));
    }
    let pending: bool =
        transaction.query_row("SELECT EXISTS(SELECT 1 FROM git_operation)", [], |row| {
            row.get(0)
        })?;
    if pending {
        return Err(Error::Invalid(
            "Finish pending Git recovery before upgrading the database.".to_owned(),
        ));
    }
    Ok(())
}

#[derive(PartialEq, Eq)]
struct GitState {
    head: Option<String>,
    names: Vec<(String, String)>,
}

fn git_state(connection: &Connection) -> Result<GitState> {
    let head = connection.query_row(
        "SELECT expected_head FROM git_state WHERE singleton=1",
        [],
        |row| row.get(0),
    )?;
    let mut names =
        connection.prepare("SELECT ref_name,tag_oid FROM history_name ORDER BY ref_name")?;
    let names = names
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    Ok(GitState { head, names })
}

#[cfg(test)]
mod tests;
