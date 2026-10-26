// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! Recoverable canonical groups. The shared mutation gate orders writers before
//! index snapshots; a prepared journal is completed before another write starts.
use super::{
    identity::ItemId,
    io::{LockMode, lock_file, write_json_atomic},
    storage_error::StorageError,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{fs, path::Path};

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Journal {
    applied: bool,
    items: Vec<Value>,
}

pub(crate) fn lock(root: &Path, mode: LockMode) -> Result<fs::File, StorageError> {
    lock_file(&root.join(".transactions.lock"), mode)
}

fn journals(root: &Path) -> Result<Vec<(std::path::PathBuf, Journal)>, String> {
    let directory = root.join(".transactions");
    let entries = match fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.to_string()),
    };
    let mut result = Vec::new();
    for entry in entries {
        let path = entry.map_err(|error| error.to_string())?.path();
        if path.extension().is_none_or(|extension| extension != "json") {
            continue;
        }
        let journal: Journal =
            serde_json::from_slice(&fs::read(&path).map_err(|error| error.to_string())?)
                .map_err(|error| format!("{}: {error}", path.display()))?;
        for item in &journal.items {
            let id = item["id"].as_str().ok_or("Journal item has no identity")?;
            super::item::validate_item(item, id)?;
        }
        result.push((path, journal));
    }
    result.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(result)
}

fn apply(root: &Path, items: &[Value]) -> Result<(), String> {
    for item in items {
        let id = item["id"].as_str().ok_or("Journal item has no identity")?;
        let path = ItemId::try_from(id)?.path(root);
        write_json_atomic(&path, item).map_err(|error| error.to_string())?;
        super::usage::pending(
            root,
            id,
            item["revision"]
                .as_str()
                .ok_or("Journal item has no revision")?,
        )?;
        if super::named_items::is_named(item["meta"]["type"].as_str().unwrap_or_default()) {
            super::named_items::observe(root, id)?;
        }
    }
    Ok(())
}

/// Caller holds the exclusive mutation gate. Applied journals are retained until
/// their complete group is visible in the index, so filesystem events cannot
/// publish only one half of a multi-item change.
pub(crate) fn write_group(root: &Path, items: Vec<Value>) -> Result<(), String> {
    let path = root
        .join(".transactions")
        .join(format!("{}.json", uuid::Uuid::now_v7()));
    let mut journal = Journal {
        applied: false,
        items,
    };
    for item in &journal.items {
        super::item::validate_item(
            item,
            item["id"].as_str().ok_or("Journal item has no identity")?,
        )?;
    }
    write_json_atomic(&path, &journal).map_err(|error| error.to_string())?;
    apply(root, &journal.items)?;
    journal.applied = true;
    write_json_atomic(&path, &journal).map_err(|error| error.to_string())
}

/// Caller holds the exclusive mutation gate, before inspecting canonical state.
pub(crate) fn recover(root: &Path) -> Result<(), String> {
    for (path, mut journal) in journals(root)? {
        if !journal.applied {
            apply(root, &journal.items)?;
            journal.applied = true;
            write_json_atomic(&path, &journal).map_err(|error| error.to_string())?;
        }
    }
    Ok(())
}

/// Caller holds the shared gate until the returned group is published.
pub(crate) fn pending(root: &Path) -> Result<Vec<(String, String)>, String> {
    let mut sources = std::collections::BTreeMap::new();
    for (_, journal) in journals(root)? {
        if !journal.applied {
            return Err("A canonical transaction needs recovery before indexing".into());
        }
        for item in journal.items {
            sources.insert(
                item["id"]
                    .as_str()
                    .ok_or("Journal item has no identity")?
                    .to_owned(),
                item["revision"]
                    .as_str()
                    .ok_or("Journal item has no revision")?
                    .to_owned(),
            );
        }
    }
    Ok(sources.into_iter().collect())
}

/// Called only after all pending groups have been published, under the gate.
pub(crate) fn acknowledge(root: &Path) -> Result<(), String> {
    for (path, journal) in journals(root)? {
        if journal.applied {
            fs::remove_file(path).map_err(|error| error.to_string())?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn interrupted_group_recovers_once_and_remains_pending_until_publication() {
        let root = tempfile::tempdir().unwrap();
        super::super::config::open_or_create_project(root.path(), "UTC").unwrap();
        let mut planned = Vec::new();
        for title in ["First", "Second"] {
            let original = super::super::write::write_item_input(
                root.path(),
                json!({
                    "meta":{"type":"page","title":title},
                    "content":{"type":"doc","content":[{"type":"paragraph"}]}
                }),
                None,
                "2026-10-24",
            )
            .unwrap()["item"]
                .clone();
            planned.push(
                super::super::assemble::assemble_item(
                    &json!({
                        "id":original["id"],"meta":{"state":"archived"}
                    }),
                    Some(&original),
                    "UTC",
                )
                .unwrap(),
            );
        }
        let _gate = lock(root.path(), LockMode::Exclusive).unwrap();
        let path = root.path().join(".transactions/interrupted.json");
        write_json_atomic(
            &path,
            &Journal {
                applied: false,
                items: planned.clone(),
            },
        )
        .unwrap();
        apply(root.path(), &planned[..1]).unwrap();
        assert!(pending(root.path()).is_err());
        // Search caches are disposable even while a canonical group needs
        // recovery. Neither its journal nor its mutation lock belongs there.
        fs::remove_dir_all(root.path().join(".cache")).unwrap();
        assert!(path.exists());
        assert!(root.path().join(".transactions.lock").exists());
        recover(root.path()).unwrap();
        for item in &planned {
            let id = item["id"].as_str().unwrap();
            assert_eq!(
                super::super::item::read_project_item(root.path(), id)
                    .unwrap()
                    .unwrap(),
                *item
            );
        }
        assert_eq!(pending(root.path()).unwrap().len(), 2);
        let changed = super::super::assemble::assemble_item(
            &json!({
                "id":planned[0]["id"],"meta":{"title":"Later write"}
            }),
            Some(&planned[0]),
            "UTC",
        )
        .unwrap();
        apply(root.path(), std::slice::from_ref(&changed)).unwrap();
        recover(root.path()).unwrap();
        assert_eq!(
            super::super::item::read_project_item(root.path(), changed["id"].as_str().unwrap())
                .unwrap()
                .unwrap()["meta"]["title"],
            "Later write"
        );
        acknowledge(root.path()).unwrap();
        assert_eq!(
            pending(root.path()).unwrap(),
            Vec::<(String, String)>::new()
        );
    }
}
