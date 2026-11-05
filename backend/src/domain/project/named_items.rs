// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! Deterministic identities shared by declarations, snippets and portable writers.
use serde_json::Value;
use sha2::{Digest, Sha256};
#[cfg(test)]
use std::path::Path;

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

#[cfg(test)]
pub(crate) fn list(root: &Path, kind: &str) -> Result<Vec<Value>, String> {
    crate::store::Store::shared(root)
        .and_then(|store| {
            store.read(|tx| {
                let ids = tx
                    .prepare(
                        "SELECT uuid FROM item WHERE type=?1 AND state='active' ORDER BY uuid",
                    )?
                    .query_map([kind], |row| row.get::<_, String>(0))?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                ids.iter()
                    .map(|id| {
                        crate::store::item::read(tx, id, true)?.ok_or(crate::store::Error::NotFound)
                    })
                    .collect()
            })
        })
        .map_err(|error| error.to_string())
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
}
