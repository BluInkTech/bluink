// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use std::path::{Path, PathBuf};

/// A validated canonical UUID, retaining the spelling used in portable files.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct ItemId(String);

impl TryFrom<&str> for ItemId {
    type Error = String;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        if !is_item_id(value) {
            return Err(format!("Invalid Bluink item id: {value}"));
        }
        Ok(Self(value.to_owned()))
    }
}

impl ItemId {
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }

    pub(crate) fn shard_directory(&self, root: &Path) -> PathBuf {
        root.join("items").join(self.0[..2].to_ascii_lowercase())
    }

    pub(crate) fn directory(&self, root: &Path) -> PathBuf {
        self.shard_directory(root).join(&self.0)
    }

    pub(crate) fn path(&self, root: &Path) -> PathBuf {
        self.directory(root).join("item.json")
    }
}

#[must_use]
pub(crate) fn is_item_id(value: &str) -> bool {
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
    fn preserves_identifier_case_and_uses_lowercase_shard() {
        let value = "ABCDEF01-2345-7000-ABCD-EF0123456789";
        let id = ItemId::try_from(value).unwrap();
        assert_eq!(id.as_str(), value);
        assert_eq!(
            id.path(Path::new("project")),
            Path::new("project")
                .join("items")
                .join("ab")
                .join(value)
                .join("item.json")
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
}
