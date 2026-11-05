// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use serde::Deserialize;
use serde_json::{Map, Value};

use super::identity::is_item_id;
use super::item::{ITEM_FORMAT_VERSION, is_canonical_instant};
use super::types::{ItemState, ItemType};

/// Validated stable fields borrowed from the complete canonical JSON.
/// Domain-specific validation remains with the item validator. Borrowing keeps
/// extension fields and the distinction between absent and null values intact.
pub(crate) struct ItemEnvelope<'a> {
    pub(crate) revision: &'a str,
    pub(crate) created_at: &'a str,
    pub(crate) modified_at: &'a str,
    pub(crate) metadata: &'a Map<String, Value>,
    pub(crate) kind: ItemType,
    pub(crate) state: ItemState,
    pub(crate) content: &'a Value,
}

impl<'a> ItemEnvelope<'a> {
    pub(crate) fn read(item: &'a Value, expected_id: &str) -> Result<Self, String> {
        if !is_item_id(expected_id) {
            return Err(format!("Invalid Bluink item id: {expected_id}"));
        }
        if item.get("format").and_then(Value::as_str) != Some("bluink.item")
            || item.get("formatVersion").and_then(Value::as_u64) != Some(ITEM_FORMAT_VERSION)
            || item.get("id").and_then(Value::as_str) != Some(expected_id)
        {
            return Err(format!(
                "Item {expected_id} has an unsupported or mismatched item file."
            ));
        }
        let required = |field: &str| {
            item.get(field)
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| format!("Item {expected_id} is missing {field}."))
        };
        let revision = required("revision")?;
        let created_at = required("createdAt")?;
        let modified_at = required("modifiedAt")?;
        if !is_canonical_instant(created_at) || !is_canonical_instant(modified_at) {
            return Err(format!(
                "Item {expected_id} has an invalid canonical timestamp."
            ));
        }
        let metadata = item
            .get("meta")
            .and_then(Value::as_object)
            .ok_or_else(|| format!("Item {expected_id} has invalid metadata."))?;
        let kind = metadata
            .get("type")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("Item {expected_id} has no type."))?;
        let kind = ItemType::deserialize(&metadata["type"])
            .map_err(|_| format!("Item {expected_id} has an unsupported type: {kind}."))?;
        let state = ItemState::deserialize(metadata.get("state").unwrap_or(&Value::Null))
            .map_err(|_| format!("Item {expected_id} has an invalid state."))?;
        let content = item
            .get("content")
            .filter(|value| value.is_object())
            .ok_or_else(|| format!("Item {expected_id} has invalid content."))?;
        if !item.get("analytics").is_some_and(Value::is_object) {
            return Err(format!("Item {expected_id} has invalid analytics."));
        }
        Ok(Self {
            revision,
            created_at,
            modified_at,
            metadata,
            kind,
            state,
            content,
        })
    }
}
