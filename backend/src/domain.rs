// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! Product rules and deterministic document, merge and publication transformations.
//! Callers supply records, timestamps and identities; shells own storage and execution.

pub(crate) mod document;
pub(crate) mod media_type;
pub(crate) mod merge;
pub(crate) mod project;
pub(crate) mod publication;
pub(crate) mod xml;

#[cfg(test)]
pub(crate) mod test_support {
    pub(crate) fn portable_fixture() -> (tempfile::TempDir, std::path::PathBuf) {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("installed");
        let source =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("test-fixtures/pdf-project");
        let source = crate::store::portable::Folder::open(&source).unwrap();
        crate::store::portable::import(&source, &root, |_, _, _| {}).unwrap();
        (directory, root)
    }

    pub(crate) fn activity_events(root: &std::path::Path) -> Vec<serde_json::Value> {
        let store = crate::store::Store::shared(root).unwrap();
        store.read(|tx|tx.prepare("SELECT json_object('itemId',p.uuid,'sourceItemId',s.uuid,'revision',a.revision_uuid,'kind',a.kind,'wordsAdded',a.words_added,'wordsDeleted',a.words_deleted,'netWordChange',a.words_added-a.words_deleted) FROM activity_event a JOIN entity p ON p.id=a.page_id JOIN entity s ON s.id=a.source_id ORDER BY a.occurred_ms,a.revision_uuid")?.query_map([],|row|row.get::<_,String>(0))?.map(|row|Ok(serde_json::from_str(&row?)?)).collect::<crate::store::Result<Vec<_>>>()).unwrap()
    }

    pub(crate) fn seed_fixture_snippets(root: &std::path::Path) {
        crate::domain::project::properties::initialize(root).unwrap();
        let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("test-fixtures/pdf-project/items/12/4f4f2275-a477-8949-8953-3d3f340d5570.json");
        let snippet: serde_json::Value =
            serde_json::from_slice(&std::fs::read(fixture).unwrap()).unwrap();
        crate::application::snippets::write_snippet(
            root,
            "source_one",
            snippet["content"]["paragraph"].clone(),
            None,
        )
        .unwrap();
    }
}
