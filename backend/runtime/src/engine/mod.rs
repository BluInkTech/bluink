// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

pub(crate) mod document;
pub(crate) mod media_type;
pub(crate) mod project;
pub(crate) mod publication;
pub(crate) mod xml;

#[cfg(test)]
mod test_support {
    pub(crate) fn seed_fixture_snippets(root: &std::path::Path) {
        crate::engine::project::properties::initialize(root).unwrap();
        let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(
            "test-fixtures/pdf-project/items/4f/4f4f2275-a477-8949-8953-3d3f340d5570/item.json",
        );
        let snippet: serde_json::Value =
            serde_json::from_slice(&std::fs::read(fixture).unwrap()).unwrap();
        crate::engine::project::snippets::write_snippet(
            root,
            "source_one",
            snippet["content"]["paragraph"].clone(),
            None,
        )
        .unwrap();
    }
}
