// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! Typed views over independent canonical declaration items.
use super::named_items;
use super::write::MutationError;
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::Path;

pub(crate) type Attributes = BTreeMap<String, Value>;
pub(crate) struct DeclarationStore<T> {
    pub(crate) kind: &'static str,
    pub(crate) validate: fn(&str, &T) -> Result<(), String>,
}
impl<T: DeserializeOwned + Serialize> DeclarationStore<T> {
    pub(crate) fn initialize(&self, root: &Path) -> Result<(), MutationError> {
        let path = root.join("items");
        std::fs::create_dir_all(&path)
            .map_err(|error| super::storage_error::StorageError::io(&path, error))?;
        self.read(root, |_| Ok(()))
    }
    pub(crate) fn read<R>(
        &self,
        root: &Path,
        action: impl FnOnce(&BTreeMap<String, T>) -> Result<R, MutationError>,
    ) -> Result<R, MutationError> {
        let records = named_items::list(root, self.kind).map_err(MutationError::Internal)?;
        let mut entries = BTreeMap::new();
        for record in records {
            if record["meta"]["state"] != "active" {
                continue;
            }
            let key = record["content"]["key"]
                .as_str()
                .ok_or_else(|| MutationError::Internal("Declaration has no key".into()))?;
            let entry: T = serde_json::from_value(record["content"].clone())
                .map_err(MutationError::Serialization)?;
            (self.validate)(key, &entry).map_err(MutationError::Internal)?;
            entries.insert(key.to_owned(), entry);
        }
        action(&entries)
    }
    /// The caller holds the project mutation gate. Only this declaration is read
    /// and replaced; unrelated item files and declaration payloads are untouched.
    pub(crate) fn update<R>(
        &self,
        root: &Path,
        key: &str,
        action: impl FnOnce(Option<T>) -> Result<(Option<T>, R), MutationError>,
    ) -> Result<R, MutationError> {
        let id = named_items::id(self.kind, key);
        let _lock = super::write::lock_item(root, &id)?;
        let existing = named_items::read(root, self.kind, key).map_err(MutationError::Internal)?;
        let value = existing
            .as_ref()
            .filter(|item| item["meta"]["state"] == "active")
            .map(|item| serde_json::from_value::<T>(item["content"].clone()))
            .transpose()
            .map_err(MutationError::Serialization)?;
        let (next, result) = action(value)?;
        let (content, state) = if let Some(next) = next {
            (self.validate)(key, &next).map_err(MutationError::Validation)?;
            let mut content = serde_json::to_value(next).map_err(MutationError::Serialization)?;
            content
                .as_object_mut()
                .ok_or_else(|| {
                    MutationError::Validation("Declaration attributes must be an object".into())
                })?
                .insert("key".into(), json!(key));
            (content, "active")
        } else {
            (
                existing.as_ref().ok_or_else(|| {
                    MutationError::NotFound(format!("Declaration {key} does not exist"))
                })?["content"]
                    .clone(),
                "archived",
            )
        };
        let next = super::assemble::assemble_item(
            &json!({"id":id,"meta":{"type":self.kind,"state":state,"title":key},"content":content}),
            existing.as_ref(),
            "UTC",
        )
        .map_err(MutationError::Validation)?;
        super::item::validate_item(&next, &id).map_err(MutationError::Validation)?;
        super::write::persist_single_item(root, &next)?;
        Ok(result)
    }
}
