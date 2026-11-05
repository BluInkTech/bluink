// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

/// A validated canonical UUID, retaining the spelling used in portable files.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ItemId {
    value: String,
    bucket: u8,
}

impl TryFrom<&str> for ItemId {
    type Error = String;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        if !is_item_id(value) {
            return Err(format!("Invalid Bluink item id: {value}"));
        }
        let uuid = uuid::Uuid::parse_str(value).map_err(|error| error.to_string())?;
        Ok(Self {
            value: value.to_owned(),
            bucket: Sha256::digest(uuid.as_bytes())[0],
        })
    }
}

impl ItemId {
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.value
    }

    #[must_use]
    pub fn shard_directory(&self, root: &Path) -> PathBuf {
        root.join("items").join(self.bucket())
    }

    #[must_use]
    pub fn bucket(&self) -> String {
        // Validation guarantees a canonical UUID spelling. Hash parsed bytes so
        // UUIDv7's shared timestamp and letter case cannot skew the buckets.
        format!("{:02x}", self.bucket)
    }

    #[must_use]
    pub fn asset_directory(&self, root: &Path) -> PathBuf {
        root.join("assets").join(self.bucket()).join(&self.value)
    }

    #[must_use]
    pub fn path(&self, root: &Path) -> PathBuf {
        self.shard_directory(root)
            .join(format!("{}.json", self.value))
    }
}

#[must_use]
pub fn is_item_id(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 36
        && [8, 13, 18, 23].iter().all(|index| bytes[*index] == b'-')
        && bytes
            .iter()
            .enumerate()
            .all(|(index, byte)| [8, 13, 18, 23].contains(&index) || byte.is_ascii_hexdigit())
        && (b'1'..=b'8').contains(&bytes[14])
        && matches!(bytes[19].to_ascii_lowercase(), b'8' | b'9' | b'a' | b'b')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_identifier_case_with_case_independent_hash_bucket() {
        let value = "ABCDEF01-2345-7000-ABCD-EF0123456789";
        let id = ItemId::try_from(value).unwrap();
        assert_eq!(id.as_str(), value);
        assert_eq!(
            id.path(Path::new("project")),
            Path::new("project")
                .join("items")
                .join(id.bucket())
                .join(format!("{value}.json"))
        );
        assert_eq!(
            id.bucket(),
            ItemId::try_from(value.to_lowercase().as_str())
                .unwrap()
                .bucket()
        );
        assert_eq!(
            id.bucket(),
            format!(
                "{:02x}",
                Sha256::digest(uuid::Uuid::parse_str(value).unwrap().as_bytes())[0]
            )
        );
    }

    #[test]
    fn rejects_invalid_identity_before_constructing_a_path() {
        for value in [
            "",
            "../escape",
            "éBCDEF01-2345-7000-ABCD-EF0123456789",
            "ABCDEF01-2345-0000-ABCD-EF0123456789",
            "ABCDEF01-2345-7000-0BCD-EF0123456789",
            "ABCDEF01_2345-7000-ABCD-EF0123456789",
        ] {
            assert!(ItemId::try_from(value).is_err(), "{value}");
        }
    }

    #[test]
    fn same_window_uuidv7_records_spread_across_all_hash_buckets() {
        let mut counts = std::collections::BTreeMap::<String, usize>::new();
        for number in 0..60_000 {
            let value = format!("01a0e2a0-0000-7000-8000-{number:012x}");
            let id = ItemId::try_from(value.as_str()).unwrap();
            *counts.entry(id.bucket()).or_default() += 1;
        }
        assert_eq!(counts.len(), 256);
        assert!(counts.values().all(|count| *count < 470));
    }
}
