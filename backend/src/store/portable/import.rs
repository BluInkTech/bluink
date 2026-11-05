// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! Prepare and validate a complete project before publishing its local directory.
use std::path::Path;

use rusqlite::OptionalExtension;
use serde_json::Value;
use uuid::Uuid;

use crate::domain::project::{config, identity::ItemId};
use crate::versioning::checkpoints;

use super::Source;
use crate::store::{Error, Result, Store, fields, fts, git, item, project, projections, relations};

const BATCH_ITEMS: usize = 128;
const BATCH_BYTES: usize = 8 * 1024 * 1024;

pub(crate) fn import(
    source: &impl Source,
    destination: &Path,
    mut progress: impl FnMut(&str, usize, usize),
) -> Result<()> {
    let destination = crate::store::location::validate(destination)?;
    if destination.try_exists()? {
        return Err(Error::Invalid(
            "Import requires a new project directory.".to_owned(),
        ));
    }
    let parent = destination
        .parent()
        .ok_or_else(|| Error::Invalid("Project requires a parent directory.".to_owned()))?;
    std::fs::create_dir_all(parent)?;
    let stage = tempfile::Builder::new()
        .prefix(".bluink-import-")
        .tempdir_in(parent)?;
    let config =
        config::parse_project_config(&source.read_json("bluink.json")?).map_err(Error::Invalid)?;
    config["analytics"]["timezone"]
        .as_str()
        .ok_or_else(|| Error::Invalid("Project has no timezone.".to_owned()))?
        .parse::<chrono_tz::Tz>()
        .map_err(|error| Error::Invalid(error.to_string()))?;
    let project_id = Uuid::parse_str(
        config["id"]
            .as_str()
            .ok_or_else(|| Error::Invalid("Project has no ID.".to_owned()))?,
    )
    .map_err(|error| Error::Invalid(error.to_string()))?;
    let paths = source.paths()?;
    let head = source.seed_history(stage.path())?;
    let store = Store::create(stage.path(), project_id)?;
    let ids = install_items(source, &store, &paths, &mut progress)?;
    declare_fields(&store)?;
    install_mappings(source, &store, stage.path(), &paths, &mut progress)?;
    for theme_id in store.read(crate::store::theme_packages::ids)? {
        let (_, files) = crate::store::theme_packages::load(stage.path(), &theme_id)?;
        crate::domain::publication::theme::validate_package(&theme_id, &files)
            .map_err(Error::Invalid)?;
    }
    project_items(&store, &ids, head.is_none(), &mut progress)?;
    finish(&store, &config, head.as_deref())?;
    {
        // Potentially long checks run on the unpublished database, outside the interactive writer queue.
        let connection = rusqlite::Connection::open(store.path())?;
        if connection.prepare("PRAGMA foreign_key_check")?.exists([])? {
            return Err(Error::Invalid(
                "Imported relationships failed integrity checks.".to_owned(),
            ));
        }
        connection.execute_batch("ANALYZE; PRAGMA optimize;")?;
    }
    project::checkpoint(&store)?;
    drop(store);
    // The destination has never been visible to the application. A single rename
    // publishes canonical data, verified objects and history together.
    std::fs::rename(stage.path(), &destination)?;
    progress("Ready", ids.len(), ids.len());
    Ok(())
}

fn install_items(
    source: &impl Source,
    store: &Store,
    paths: &[String],
    progress: &mut impl FnMut(&str, usize, usize),
) -> Result<Vec<(String, usize)>> {
    let paths: Vec<_> = paths
        .iter()
        .filter(|path| path.starts_with("items/"))
        .collect();
    let mut ids = Vec::with_capacity(paths.len());
    let mut pending = Vec::new();
    let mut bytes = 0;
    for path in &paths {
        let raw = source.read_json(path)?;
        let value: Value = serde_json::from_slice(&raw)?;
        let id = value["id"]
            .as_str()
            .ok_or_else(|| Error::Invalid("Portable item has no ID.".to_owned()))?;
        let identity = ItemId::try_from(id).map_err(Error::Invalid)?;
        let expected = identity
            .path(Path::new(""))
            .to_string_lossy()
            .replace('\\', "/");
        if expected != path.as_str() {
            return Err(Error::Invalid(format!(
                "Item ID does not match its portable path: {path}"
            )));
        }
        let prepared = item::PreparedItem::new(&value)?;
        ids.push((id.to_owned(), raw.len()));
        bytes += raw.len();
        pending.push(prepared);
        if pending.len() >= BATCH_ITEMS || bytes >= BATCH_BYTES {
            flush_items(store, std::mem::take(&mut pending))?;
            bytes = 0;
            progress("Importing canonical items", ids.len(), paths.len());
        }
    }
    flush_items(store, pending)?;
    Ok(ids)
}

fn flush_items(store: &Store, items: Vec<item::PreparedItem>) -> Result<()> {
    if items.is_empty() {
        return Ok(());
    }
    store.write(move |tx| {
        for item in items {
            item.install(tx, item::ExpectedRevision::New)?;
        }
        Ok(())
    })
}

fn declare_fields(store: &Store) -> Result<()> {
    let ids = store.read(|tx| {
        Ok(tx
            .prepare("SELECT uuid FROM item WHERE type='property' AND state='active' ORDER BY id")?
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?)
    })?;
    for id in ids {
        store.write(move |tx| {
            let header = item::read_header(tx, &id)?.ok_or(Error::NotFound)?;
            let content = crate::store::body::read(tx, header.body)?;
            fields::declare(tx, header.id, &content)
        })?;
    }
    Ok(())
}

struct Projection {
    value: Value,
    facts: projections::Prepared,
    fts: fts::Prepared,
}

fn project_items(
    store: &Store,
    ids: &[(String, usize)],
    initial_checkpoint: bool,
    progress: &mut impl FnMut(&str, usize, usize),
) -> Result<()> {
    let mut pending = Vec::new();
    let mut bytes = 0;
    for (offset, (id, source_bytes)) in ids.iter().enumerate() {
        // Read the immutable imported database, never read a mutable folder twice.
        let value = store
            .read(|tx| item::read(tx, id, true))?
            .ok_or(Error::NotFound)?;
        bytes += source_bytes;
        let facts = projections::Prepared::new(&value)?;
        let fts = fts::Prepared::new(&facts.document);
        pending.push(Projection { value, facts, fts });
        if pending.len() >= BATCH_ITEMS || bytes >= BATCH_BYTES {
            flush_projections(store, std::mem::take(&mut pending), initial_checkpoint)?;
            bytes = 0;
            progress("Building search and relationships", offset + 1, ids.len());
        }
    }
    flush_projections(store, pending, initial_checkpoint)
}

fn flush_projections(
    store: &Store,
    pending: Vec<Projection>,
    initial_checkpoint: bool,
) -> Result<()> {
    if pending.is_empty() {
        return Ok(());
    }
    store.write(move |tx| {
        for projection in pending {
            let id: i64 = tx.query_row(
                "SELECT id FROM item WHERE uuid=?1",
                [&projection.facts.document.id],
                |row| row.get(0),
            )?;
            let meta = projection.value["meta"]
                .as_object()
                .ok_or_else(|| Error::Invalid("Item has no metadata.".to_owned()))?;
            fields::replace(tx, id, meta, None)?;
            projection.facts.install(tx, id)?;
            projection.fts.install(tx, id)?;
            relations::validate(tx, &projection.value, &projection.facts.document)?;
            super::super::theme_packages::require_reference(tx, &projection.value)?;
            if initial_checkpoint {
                git::mark_item_dirty(
                    tx,
                    &projection.facts.document.id,
                    &projection.facts.document.revision,
                    1,
                )?;
            }
        }
        Ok(())
    })
}

fn install_mappings(
    source: &impl Source,
    store: &Store,
    root: &Path,
    paths: &[String],
    progress: &mut impl FnMut(&str, usize, usize),
) -> Result<()> {
    for (offset, path) in paths.iter().enumerate() {
        if path.starts_with("items/") {
            continue;
        }
        checkpoints::validate_path(path).map_err(|error| Error::Invalid(error.to_string()))?;
        let path = path.clone();
        if checkpoints::validate_record_path(&path).is_ok() {
            let value: Value = serde_json::from_slice(&source.read_json(&path)?)?;
            store.write(move |tx| {
                git::put_record(tx, &path, &value, &Uuid::now_v7().to_string(), 1)
            })?;
        } else {
            let asset = checkpoints::install_asset(root, &mut source.open_asset(&path)?)
                .map_err(|error| Error::Invalid(error.to_string()))?;
            store.write(move |tx| {
                let owner = if path.starts_with("assets/") {
                    let uuid = path
                        .split('/')
                        .nth(2)
                        .ok_or_else(|| Error::Invalid("Attachment owner is missing.".to_owned()))?;
                    Some(
                        tx.query_row("SELECT id FROM item WHERE uuid=?1", [uuid], |row| {
                            row.get::<_, i64>(0)
                        })
                        .optional()?
                        .ok_or_else(|| {
                            Error::Invalid("Attachment owner does not exist.".to_owned())
                        })?,
                    )
                } else {
                    None
                };
                git::put_asset(tx, &path, owner, &asset, 1)
            })?;
        }
        progress("Verifying project assets", offset + 1, paths.len());
    }
    Ok(())
}

fn finish(store: &Store, config: &Value, head: Option<&str>) -> Result<()> {
    let config = config.clone();
    let head = head.map(str::to_owned);
    store.write(move |tx| {
        let stored = project::read_config(tx)?;
        if stored != config { return Err(Error::Invalid("Project configuration changed during import.".to_owned())); }
        let valid: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM item WHERE uuid=?1 AND type='workspace' AND state='active')", [config["defaultWorkspaceId"].as_str()], |row| row.get(0))?;
        if !valid { return Err(Error::Invalid("Default workspace is missing or inactive.".to_owned())); }
        if head.is_some() {
            tx.execute("DELETE FROM dirty_path", [])?;
        }
        tx.execute("UPDATE project_state SET generation=1,change_floor=1,change_epoch=?1", [Uuid::now_v7().to_string()])?;
        tx.execute("UPDATE git_state SET expected_head=?1", [head])?;
        Ok(())
    })
}
