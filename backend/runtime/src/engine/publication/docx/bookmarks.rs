// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

struct Bookmark {
    id: usize,
    name: String,
    defined: bool,
    referenced: bool,
}
#[derive(Default)]
pub(super) struct Bookmarks {
    entries: BTreeMap<String, Bookmark>,
}

impl Bookmarks {
    fn reserve(&mut self, key: &str) -> &mut Bookmark {
        let id = self.entries.len();
        self.entries.entry(key.into()).or_insert_with(|| Bookmark {
            id,
            name: format!(
                "b_{}",
                &super::super::digest_hex(&Sha256::digest(key.as_bytes()))[..32]
            ),
            defined: false,
            referenced: false,
        })
    }

    pub(super) fn link(&mut self, key: &str) -> String {
        let bookmark = self.reserve(key);
        bookmark.referenced = true;
        bookmark.name.clone()
    }

    pub(super) fn define(&mut self, key: &str) -> Option<(usize, String)> {
        let bookmark = self.reserve(key);
        if bookmark.defined {
            return None;
        }
        bookmark.defined = true;
        Some((bookmark.id, bookmark.name.clone()))
    }

    /// Word deletes the cached TOC's target bookmarks when it rebuilds entries.
    /// Keep those hidden targets independent of authored page/block links.
    pub(super) fn define_contents(&mut self, key: &str) -> Option<(usize, String)> {
        let bookmark = self.reserve(&format!("_toc:{key}"));
        if bookmark.defined {
            return None;
        }
        bookmark.defined = true;
        bookmark.name = bookmark.name.replacen("b_", "_Toc", 1);
        Some((bookmark.id, bookmark.name.clone()))
    }

    pub(super) fn validate(&self) -> Result<(), String> {
        if let Some((key, _)) = self
            .entries
            .iter()
            .find(|(_, bookmark)| bookmark.referenced && !bookmark.defined)
        {
            return Err(format!("DOCX bookmark target was not rendered: {key}"));
        }
        Ok(())
    }
}
