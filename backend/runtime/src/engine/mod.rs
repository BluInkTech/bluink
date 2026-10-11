// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

pub(crate) mod document;
pub(crate) mod project;
pub(crate) mod publication;

#[cfg(test)]
mod test_support {
    pub(crate) fn seed_fixture_snippets(root: &std::path::Path) {
        let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("test-fixtures/pdf-project/snippets/snippet_source_one.json");
        let snippet: serde_json::Value =
            serde_json::from_slice(&std::fs::read(fixture).unwrap()).unwrap();
        crate::engine::project::snippets::write_snippet(
            root,
            "source_one",
            snippet["paragraph"].clone(),
            None,
        )
        .unwrap();
    }
}
