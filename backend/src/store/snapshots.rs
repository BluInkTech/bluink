// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! Concrete snapshot readers for shells that start with a project root.
use crate::domain::project::identity::ItemId;

#[cfg(test)]
use crate::domain::project::snippets::validate_references_with;
use crate::domain::project::snippets::{item_id, record};
use serde_json::Value;
use std::path::Path;

#[cfg(test)]
pub(crate) fn read_project_config(root: &Path) -> Result<Option<Value>, String> {
    if !root
        .join(crate::store::DATABASE_NAME)
        .try_exists()
        .map_err(|error| error.to_string())?
    {
        return Ok(None);
    }
    crate::store::Store::shared(root)
        .and_then(|store| store.read(crate::store::project::read_config))
        .map(Some)
        .map_err(|error| error.to_string())
}

/// Read one item from a store snapshot. A missing item is `None`.
pub(crate) fn read_project_item(root: &Path, id: &str) -> crate::store::Result<Option<Value>> {
    ItemId::try_from(id)
        .map_err(|message| crate::store::Error::Internal(anyhow::Error::msg(message)))?;
    crate::store::Store::shared(root)
        .and_then(|store| store.read(|tx| crate::store::item::read(tx, id, true)))
}

#[cfg(test)]
pub(crate) fn read_named(root: &Path, kind: &str, key: &str) -> Result<Option<Value>, String> {
    read_project_item(root, &crate::domain::project::named_items::id(kind, key))
        .map_err(|error| error.to_string())
}

pub(crate) fn read_snippet(root: &Path, key: &str) -> Result<Option<Value>, String> {
    read_project_item(root, &item_id(key)?)
        .map_err(|error| error.to_string())?
        .map(|item| record(&item))
        .transpose()
}

/// Called under the relationship lock, making page saves race-safe with archiving.
#[cfg(test)]
pub(crate) fn validate_page_snippet_references(root: &Path, node: &Value) -> Result<(), String> {
    validate_references_with(node, |id| {
        let record =
            read_snippet(root, id)?.ok_or_else(|| format!("Snippet {id} does not exist."))?;
        if record["state"] != "active" {
            return Err(format!("Snippet {id} is archived."));
        }
        Ok(())
    })
}
