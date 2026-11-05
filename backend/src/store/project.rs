// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! First creation publishes a complete database, never a half-seeded project.
use super::{DATABASE_NAME, Error, Result, Store, git, mutations};
use crate::domain::project::{config, named_items, workspace};
use crate::store::io;
use rusqlite::{Connection, OptionalExtension};
use serde_json::{Value, json};
use std::{path::Path, sync::Arc};
use uuid::Uuid;

pub(crate) fn open_or_create(
    root: &Path,
    timezone: &str,
    template: &Value,
) -> Result<(Arc<Store>, Value)> {
    timezone
        .parse::<chrono_tz::Tz>()
        .map_err(|error| Error::Invalid(error.to_string()))?;
    let root = super::location::validate(root)?;
    std::fs::create_dir_all(&root)?;
    // This cross-process lock protects publication only; ordinary writes use SQLite.
    let _initialization = io::lock_file(&root.join(".project-init.lock"), io::LockMode::Exclusive)
        .map_err(|error| Error::Invalid(format!("{error:#}")))?;
    if root.join(DATABASE_NAME).try_exists()? {
        let store = Store::shared(&root)?;
        let config = store.read(read_config)?;
        return Ok((store, config));
    }
    if root.join("bluink.json").try_exists()? {
        return Err(Error::Invalid("This is a portable project folder. Import it into a new local project before opening it.".to_owned()));
    }
    let project_id = Uuid::now_v7();
    let workspace_id = Uuid::now_v7().to_string();
    let config = json!({"format":"bluink-project","formatVersion":config::PROJECT_FORMAT_VERSION,"id":project_id.to_string(),"defaultWorkspaceId":workspace_id,"analytics":{"timezone":timezone,"eventRetentionDays":14}});
    let changes = initial_items(&workspace_id, timezone, template)?;
    let stage = tempfile::Builder::new()
        .prefix(".initializing-")
        .tempdir_in(&root)?;
    let store = Store::create(stage.path(), project_id)?;
    let written = config.clone();
    store.write(move |tx| {
        let generation = mutations::apply(tx, &changes, &[], None)?;
        git::put_record(
            tx,
            "bluink.json",
            &written,
            &Uuid::now_v7().to_string(),
            generation,
        )?;
        Ok(())
    })?;
    checkpoint(&store)?;
    drop(store);
    std::fs::rename(stage.path().join(DATABASE_NAME), root.join(DATABASE_NAME))?;
    Ok((Store::shared(&root)?, config))
}

fn initial_items(
    workspace_id: &str,
    timezone: &str,
    template: &Value,
) -> Result<Vec<mutations::PreparedChange>> {
    let mut values = Vec::new();
    for mut property in config::default_user_properties() {
        let key = property["key"]
            .as_str()
            .ok_or_else(|| Error::Invalid("Default property has no key.".to_owned()))?
            .to_owned();
        if config::is_required_builtin_property(&key) {
            property["system"] = json!(true);
        }
        values.push(crate::store::commands::assemble_item(&json!({"id":named_items::id("property",&key),"meta":{"type":"property","title":key},"content":property}),None,timezone).map_err(Error::Invalid)?);
    }
    values.push(crate::store::commands::assemble_item(&json!({"id":workspace_id,"meta":{"type":"workspace","title":"All notes"},"content":crate::store::commands::with_authoring_values(|values| workspace::instantiate(template, values)).map_err(Error::Invalid)?}),None,timezone).map_err(Error::Invalid)?);
    values
        .into_iter()
        .map(|value| mutations::PreparedChange::new(value, None))
        .collect()
}

pub(crate) fn read_config(transaction: &rusqlite::Transaction<'_>) -> Result<Value> {
    let raw: Option<String> = transaction
        .query_row(
            "SELECT canonical_json FROM project_record WHERE path='bluink.json'",
            [],
            |row| row.get(0),
        )
        .optional()?;
    config::parse_project_config(
        raw.ok_or_else(|| Error::corrupt("Project configuration is missing."))?
            .as_bytes(),
    )
    .map_err(Error::corrupt)
}

/// Call only while an unpublished database or quiesced project has no writers.
pub(crate) fn checkpoint(store: &Store) -> Result<()> {
    let connection =
        Connection::open_with_flags(store.path(), rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE)?;
    connection.busy_timeout(std::time::Duration::from_secs(5))?;
    let (busy, log, done): (i64, i64, i64) =
        connection.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })?;
    if busy != 0 || log != done {
        return Err(Error::Busy);
    }
    Ok(())
}
