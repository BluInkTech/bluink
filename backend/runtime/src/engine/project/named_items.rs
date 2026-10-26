// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! Named records use ordinary item files. This inventory is only discovery data;
//! mutations and validation always read their canonical record by deterministic ID.
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use serde_json::Value;
use sha2::{Digest, Sha256};

use super::identity::ItemId;
use crate::projection::FileStamp;

struct NamedEntry {
    kind: String,
    key: String,
    active: bool,
    stamp: FileStamp,
}
type Inventory = BTreeMap<String, NamedEntry>;
type InventorySlot = Arc<Mutex<Option<Inventory>>>;
static INVENTORIES: OnceLock<Mutex<HashMap<PathBuf, InventorySlot>>> = OnceLock::new();

pub(crate) fn id(kind: &str, key: &str) -> String {
    let hash = Sha256::digest(format!("bluink:item:{kind}:{key}").as_bytes());
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&hash[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x80;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    uuid::Uuid::from_bytes(bytes).to_string()
}

pub(crate) fn is_named(kind: &str) -> bool {
    matches!(kind, "tag" | "label" | "property" | "snippet")
}

pub(crate) fn validate_identity(item: &Value, expected_id: &str) -> Result<(), String> {
    let kind = item["meta"]["type"].as_str().unwrap_or_default();
    if !is_named(kind) {
        return Ok(());
    }
    let key = item["content"]["key"]
        .as_str()
        .filter(|key| !key.is_empty())
        .ok_or_else(|| format!("Named {kind} item has no key."))?;
    if id(kind, key) != expected_id {
        return Err(format!(
            "Named {kind} item identity does not match its key."
        ));
    }
    Ok(())
}

pub(crate) fn read(root: &Path, kind: &str, key: &str) -> Result<Option<Value>, String> {
    super::item::read_project_item(root, &id(kind, key)).map_err(|error| error.to_string())
}
#[cfg(test)]
pub(crate) fn path(root: &Path, kind: &str, key: &str) -> Result<PathBuf, String> {
    Ok(ItemId::try_from(id(kind, key).as_str())?.path(root))
}

/// Called while the common item writer holds its relationship lock.
pub(crate) fn validate_mutation(
    root: &Path,
    existing: Option<&Value>,
    next: &Value,
) -> Result<(), super::write::MutationError> {
    use super::write::MutationError;
    let kind = next["meta"]["type"].as_str().unwrap_or_default();
    if !matches!(kind, "tag" | "label" | "property") {
        return Ok(());
    }
    let key = next["content"]["key"]
        .as_str()
        .ok_or_else(|| MutationError::Validation("Declaration has no key".into()))?;
    if kind == "property"
        && let Some(previous) = existing.filter(|previous| previous["meta"]["state"] == "active")
    {
        if previous["content"]["valueType"] != next["content"]["valueType"] {
            return Err(MutationError::Validation(
                "A property's value type cannot change.".into(),
            ));
        }
        for option in previous["content"]["options"]
            .as_array()
            .into_iter()
            .flatten()
        {
            if !next["content"]["options"]
                .as_array()
                .is_some_and(|options| options.contains(option))
            {
                return Err(MutationError::Validation(
                    "Existing property options cannot be removed.".into(),
                ));
            }
        }
    }
    if next["meta"]["state"] == "archived"
        && existing.is_some_and(|previous| previous["meta"]["state"] == "active")
    {
        if kind == "property" && super::config::is_required_builtin_property(key) {
            return Err(MutationError::Validation(format!(
                "Built-in property {key} is required and cannot be deleted."
            )));
        }
        let usage = if kind == "property" {
            super::usage::Usage::Property(key)
        } else {
            super::usage::Usage::facet(kind, key).map_err(MutationError::Validation)?
        };
        if crate::catalogue_usage::UsageCheck::new(root, usage)
            .used()
            .map_err(|error| MutationError::Internal(error.to_string()))?
        {
            return Err(MutationError::Validation(format!(
                "The {kind} {key} is still used."
            )));
        }
    }
    Ok(())
}

fn slot(root: &Path) -> Result<InventorySlot, String> {
    let key = root.canonicalize().map_err(|error| error.to_string())?;
    Ok(INVENTORIES
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .map_err(|error| error.to_string())?
        .entry(key)
        .or_default()
        .clone())
}

fn scan(root: &Path) -> Result<Inventory, String> {
    let mut inventory = Inventory::new();
    for id in super::item::list_project_item_ids(root)? {
        // Named identities are UUIDv8 by contract. Ordinary UUIDv7 notes/tasks
        // need no body read while discovering declarations.
        if id.as_bytes()[14] != b'8' {
            continue;
        }
        let Some(record) =
            super::item::read_project_item(root, &id).map_err(|error| error.to_string())?
        else {
            continue;
        };
        let kind = record["meta"]["type"].as_str().unwrap_or_default();
        if is_named(kind) {
            super::item::validate_item(&record, &id)?;
            inventory.insert(id.clone(), named_entry(root, &id, &record)?);
        }
    }
    Ok(inventory)
}

pub(crate) fn invalidate(root: &Path) -> Result<(), String> {
    let key = root.canonicalize().map_err(|error| error.to_string())?;
    INVENTORIES
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .map_err(|error| error.to_string())?
        .remove(&key);
    Ok(())
}

pub(crate) fn observe(root: &Path, item_id: &str) -> Result<Option<String>, String> {
    let record =
        super::item::read_project_item(root, item_id).map_err(|error| error.to_string())?;
    let entry = slot(root)?;
    let mut state = entry.lock().map_err(|error| error.to_string())?;
    if state.is_none() {
        *state = Some(scan(root)?);
    }
    let inventory = state.as_mut().ok_or("Inventory was not loaded")?;
    let previous_kind = inventory.remove(item_id).map(|entry| entry.kind);
    let mut current_kind = None;
    if let Some(record) = record {
        let kind = record["meta"]["type"].as_str().unwrap_or_default();
        if is_named(kind) {
            current_kind = Some(kind.to_owned());
            inventory.insert(item_id.to_owned(), named_entry(root, item_id, &record)?);
        }
    }
    drop(state);
    Ok(current_kind.or(previous_kind))
}

fn named_entry(root: &Path, id: &str, record: &Value) -> Result<NamedEntry, String> {
    let stamp = FileStamp::read(&ItemId::try_from(id)?.path(root))?
        .ok_or_else(|| format!("Named item {id} disappeared while being read"))?;
    Ok(NamedEntry {
        kind: record["meta"]["type"]
            .as_str()
            .ok_or("Named item has no type")?
            .to_owned(),
        key: record["content"]["key"]
            .as_str()
            .ok_or("Named item has no key")?
            .to_owned(),
        active: record["meta"]["state"] == "active",
        stamp,
    })
}

/// Discovery needs only the in-memory names; canonical payloads are read by
/// callers that actually need declaration content.
pub(crate) fn keys(root: &Path, kind: &str) -> Result<Vec<String>, String> {
    let entry = slot(root)?;
    let mut state = entry.lock().map_err(|error| error.to_string())?;
    if state.is_none() {
        *state = Some(scan(root)?);
    }
    let cached: Vec<_> = state
        .as_ref()
        .ok_or("Inventory was not loaded")?
        .iter()
        .filter(|(_, candidate)| candidate.kind == kind)
        .map(|(id, candidate)| (id.clone(), candidate.stamp))
        .collect();
    drop(state);
    for (id, stamp) in cached {
        if FileStamp::read(&ItemId::try_from(id.as_str())?.path(root))? != Some(stamp) {
            observe(root, &id)?;
        }
    }
    let state = entry.lock().map_err(|error| error.to_string())?;
    let mut keys: Vec<_> = state
        .as_ref()
        .ok_or("Inventory was not loaded")?
        .values()
        .filter(|candidate| candidate.active && candidate.kind == kind)
        .map(|candidate| candidate.key.clone())
        .collect();
    drop(state);
    keys.sort_unstable();
    Ok(keys)
}

pub(crate) fn list(root: &Path, kind: &str) -> Result<Vec<Value>, String> {
    let mut records = Vec::new();
    for key in keys(root, kind)? {
        if let Some(item) = read(root, kind, &key)? {
            records.push(item);
        }
    }
    Ok(records)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn named_identity_is_stable_namespaced_and_a_valid_item_uuid() {
        let tag = id("tag", "history");
        assert_eq!(tag, id("tag", "history"));
        assert_ne!(tag, id("label", "history"));
        assert_ne!(tag, id("tag", "history/1900"));
        assert!(super::super::identity::is_item_id(&tag));
    }

    #[test]
    fn inventory_does_not_open_unrelated_note_files() {
        let root = tempfile::tempdir().unwrap();
        let note = ItemId::try_from("01a0e2a0-0000-7000-8000-000000000002")
            .unwrap()
            .path(root.path());
        std::fs::create_dir_all(note.parent().unwrap()).unwrap();
        std::fs::write(note, "not readable as a note").unwrap();
        super::super::properties::initialize(root.path()).unwrap();
        super::super::properties::create_property(
            root.path(),
            serde_json::json!({"key":"topic","valueType":"text"}),
        )
        .unwrap();
        invalidate(root.path()).unwrap();
        assert_eq!(list(root.path(), "tag").unwrap(), Vec::<Value>::new());
        assert_eq!(list(root.path(), "property").unwrap().len(), 1);
        assert!(read(root.path(), "property", "topic").unwrap().is_some());
        std::fs::remove_file(path(root.path(), "property", "topic").unwrap()).unwrap();
        invalidate(root.path()).unwrap();
        assert_eq!(list(root.path(), "property").unwrap(), Vec::<Value>::new());
    }
}
