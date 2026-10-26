// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use super::catalogues::{Attributes, DeclarationStore};
use super::properties::is_snake_case;
use super::write::MutationError;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::Path;

#[derive(Default, Deserialize, Serialize)]
struct Definition {
    #[serde(flatten)]
    attributes: Attributes,
}

fn catalogue(kind: &str) -> Result<DeclarationStore<Definition>, String> {
    match kind {
        "tag" => Ok(DeclarationStore {
            kind: "tag",
            validate: |key, _| validate_facet_key("tag", key),
        }),
        "label" => Ok(DeclarationStore {
            kind: "label",
            validate: |key, _| validate_facet_key("label", key),
        }),
        _ => Err("Facet kind must be tag or label.".to_owned()),
    }
}

pub(crate) fn initialize(root: &Path) -> Result<(), MutationError> {
    for kind in ["tag", "label"] {
        catalogue(kind)
            .map_err(MutationError::Internal)?
            .initialize(root)?;
    }
    Ok(())
}

pub(crate) fn list_facets(root: &Path, kind: Option<&str>) -> Result<Value, String> {
    if let Some(kind) = kind {
        catalogue(kind)?;
    }
    let mut facets = Vec::new();
    for current in ["label", "tag"] {
        if kind.is_some_and(|kind| kind != current) {
            continue;
        }
        facets.extend(
            super::named_items::keys(root, current)?
                .into_iter()
                .map(|key| json!({"key":key,"kind":current})),
        );
    }
    Ok(Value::Array(facets))
}

pub(crate) fn create_facet(root: &Path, kind: &str, key: &str) -> Result<Value, MutationError> {
    let _gate = super::transaction::lock(root, super::io::LockMode::Exclusive)?;
    super::transaction::recover(root).map_err(MutationError::Internal)?;
    validate_facet_key(kind, key).map_err(MutationError::Validation)?;
    catalogue(kind)
        .map_err(MutationError::Validation)?
        .update(root, key, |existing| {
            if existing.is_some() {
                return Err(MutationError::Validation(format!(
                    "Facet {key}.{kind} already exists."
                )));
            }
            Ok((Some(Definition::default()), json!({"key":key,"kind":kind})))
        })
}

pub(crate) fn remove_facet(
    root: &Path,
    kind: &str,
    key: &str,
    used: impl FnOnce() -> Result<bool, MutationError>,
) -> Result<(), MutationError> {
    let _gate = super::transaction::lock(root, super::io::LockMode::Exclusive)?;
    super::transaction::recover(root).map_err(MutationError::Internal)?;
    validate_facet_key(kind, key).map_err(MutationError::Validation)?;
    let catalogue = catalogue(kind).map_err(MutationError::Validation)?;
    let usage = super::usage::Usage::facet(kind, key).map_err(MutationError::Validation)?;
    super::usage::remove_if_unused(
        root,
        usage,
        || Ok(()),
        || used().map(usize::from),
        |()| {
            catalogue.update(root, key, |existing| {
                existing.ok_or_else(|| {
                    MutationError::NotFound(format!("Facet {key}.{kind} does not exist."))
                })?;
                Ok((None, ()))
            })
        },
    )
}

pub(crate) fn validate_content(kind: &str, content: &Value) -> Result<(), String> {
    let key = content["key"].as_str().ok_or("Facet has no key")?;
    validate_facet_key(kind, key)
}

fn validate_facet_key(kind: &str, key: &str) -> Result<(), String> {
    if !matches!(kind, "tag" | "label") {
        return Err("Facet kind must be tag or label.".to_owned());
    }
    if kind == "label" {
        return if is_snake_case(key) {
            Ok(())
        } else {
            Err("Label keys must be lowercase snake-case tokens.".to_owned())
        };
    }
    let valid = key.split('/').enumerate().all(|(index, segment)| {
        is_snake_case(segment)
            || (index > 0
                && (1..=4).contains(&segment.len())
                && segment.bytes().all(|byte| byte.is_ascii_digit()))
    });
    if valid {
        Ok(())
    } else {
        Err(
            "Tag keys must be lowercase snake-case paths; numeric leaf segments are allowed."
                .to_owned(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn declaration_items_preserve_hierarchy_metadata_and_removal_rules() {
        let root = tempfile::tempdir().unwrap();
        initialize(root.path()).unwrap();
        create_facet(root.path(), "tag", "births/1900").unwrap();
        create_facet(root.path(), "label", "foreshadowing").unwrap();
        assert_eq!(
            list_facets(root.path(), None).unwrap(),
            json!([
                {"key":"foreshadowing","kind":"label"}, {"key":"births/1900","kind":"tag"}
            ])
        );
        let file = super::super::named_items::path(root.path(), "tag", "births/1900").unwrap();
        let mut data: Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
        data["content"]["color"] = json!("blue");
        data["content"]["description"] = json!("Birth year");
        std::fs::write(&file, data.to_string()).unwrap();
        create_facet(root.path(), "tag", "history").unwrap();
        let updated: Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
        assert_eq!(updated["content"], data["content"]);
        assert!(create_facet(root.path(), "tag", "history").is_err());
        assert!(create_facet(root.path(), "label", "bad/path").is_err());
        assert!(remove_facet(root.path(), "tag", "history", || Ok(true)).is_err());
        remove_facet(root.path(), "tag", "history", || Ok(false)).unwrap();
        assert!(matches!(
            remove_facet(root.path(), "tag", "history", || Ok(false)),
            Err(MutationError::NotFound(_))
        ));
        assert_eq!(
            list_facets(root.path(), Some("label"))
                .unwrap()
                .as_array()
                .unwrap()
                .len(),
            1
        );
        let history_path = super::super::named_items::path(root.path(), "tag", "history").unwrap();
        let archived: Value =
            serde_json::from_slice(&std::fs::read(&history_path).unwrap()).unwrap();
        assert_eq!(archived["meta"]["state"], "archived");
        create_facet(root.path(), "tag", "history").unwrap();
        let restored: Value =
            serde_json::from_slice(&std::fs::read(history_path).unwrap()).unwrap();
        assert_eq!(restored["id"], archived["id"]);
        assert_eq!(restored["meta"]["state"], "active");
        assert_ne!(restored["revision"], archived["revision"]);
        assert!(!root.path().join("facets").exists());
    }

    #[test]
    fn invalid_named_record_is_not_overwritten_and_does_not_block_unrelated_creation() {
        let root = tempfile::tempdir().unwrap();
        initialize(root.path()).unwrap();
        create_facet(root.path(), "tag", "broken").unwrap();
        let file = super::super::named_items::path(root.path(), "tag", "broken").unwrap();
        std::fs::write(&file, "invalid JSON").unwrap();
        assert!(list_facets(root.path(), Some("tag")).is_err());
        assert!(create_facet(root.path(), "tag", "broken").is_err());
        create_facet(root.path(), "tag", "unrelated").unwrap();
        assert_eq!(std::fs::read_to_string(file).unwrap(), "invalid JSON");
        assert!(list_facets(root.path(), Some("label")).is_ok());
    }
}
