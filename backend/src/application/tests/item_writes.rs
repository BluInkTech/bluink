// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use crate::application::item_writes::write_item_input;
use crate::domain::project::config::create_project_config;
use crate::store::snapshots::read_project_item;
use std::path::Path;

/// Commit a prepared item through the store's own validation, as merges
/// and imports do, rather than assembling authoring intent.
fn write_project_item(
    root: &Path,
    next: &serde_json::Value,
    expected_revision: Option<&str>,
    activity_date: &str,
) -> Result<(), String> {
    let store = crate::store::Store::shared(root).map_err(|error| error.to_string())?;
    let id = next["id"].as_str().ok_or("Item has no ID")?;
    let previous = store
        .read(|tx| crate::store::item::read(tx, id, true))
        .map_err(|error| error.to_string())?;
    crate::store::commands::validate_content(previous.as_ref(), next)
        .map_err(|error| error.to_string())?;
    let prepared = crate::store::mutations::PreparedChange::new(
        next.clone(),
        expected_revision.map(str::to_owned),
    )
    .map_err(|error| error.to_string())?;
    let day = activity_date.to_owned();
    let next = next.clone();
    store
        .write(move |tx| {
            crate::store::commands::validate_state(tx, previous.as_ref(), &next)?;
            crate::store::mutations::apply(tx, &[prepared], &[], Some(&day)).map(drop)
        })
        .map_err(|error| error.to_string())
}

#[test]
fn assembles_and_commits_input_under_the_revision_lock() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().join("project");
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("test-fixtures/pdf-project");
    let config: serde_json::Value =
        serde_json::from_slice(&std::fs::read(fixture.join("bluink.json")).unwrap()).unwrap();
    create_project_config(&root, config).unwrap();
    crate::domain::test_support::seed_fixture_snippets(&root);
    crate::domain::project::properties::initialize(&root).unwrap();
    let id = uuid::Uuid::now_v7().to_string();
    let input = serde_json::json!({
        "id": id,
        "meta": {"type": "page", "title": "First"},
        "content": {"document": {"type": "doc", "content": [{"type": "paragraph", "content": [{"type": "text", "text": "Two words"}]}]}},
        "tags": ["astronomy", "astronomy"]
    });
    let first = write_item_input(&root, input.clone(), None, "2026-09-28").unwrap();
    assert_eq!(first["item"]["analytics"]["current"]["wordCount"], 2);
    assert_eq!(first["item"]["tags"], serde_json::json!(["astronomy"]));
    assert!(first["item"]["content"]["document"]["content"][0]["attrs"]["id"].is_string());
    assert_eq!(
        read_project_item(&root, &id).unwrap().unwrap(),
        first["item"]
    );
    let revision = first["item"]["revision"].as_str().unwrap();
    let mut changed = input;
    changed["meta"]["title"] = serde_json::json!("Second");
    let second = write_item_input(&root, changed.clone(), Some(revision), "2026-09-28").unwrap();
    assert_eq!(second["item"]["createdAt"], first["item"]["createdAt"]);
    assert_eq!(second["item"]["meta"]["title"], "Second");
    let patch = serde_json::json!({"id": id, "meta": {}, "tags": ["physics"]});
    let patched = write_item_input(
        &root,
        patch,
        second["item"]["revision"].as_str(),
        "2026-09-28",
    )
    .unwrap();
    assert_eq!(patched["item"]["content"], second["item"]["content"]);
    assert_eq!(patched["item"]["meta"], second["item"]["meta"]);
    assert_eq!(patched["item"]["tags"], serde_json::json!(["physics"]));
    assert!(
        write_item_input(&root, changed, Some(revision), "2026-09-28")
            .unwrap_err()
            .contains("changed since it was read")
    );
    temporary.close().unwrap();
}

#[test]
fn http_link_syntax_is_checked_before_save_without_contacting_servers() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().join("project");
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("test-fixtures/pdf-project");
    let config =
        serde_json::from_slice(&std::fs::read(fixture.join("bluink.json")).unwrap()).unwrap();
    create_project_config(&root, config).unwrap();
    crate::domain::test_support::seed_fixture_snippets(&root);
    crate::domain::project::properties::initialize(&root).unwrap();
    let id = uuid::Uuid::now_v7().to_string();
    let make_input = |href: &str| {
        serde_json::json!({
            "id":id,"meta":{"type":"page","title":"Private network links"},
            "content":{"document": {"type":"doc","content":[{"type":"paragraph","content":[{
                "type":"citation","attrs":{"source":"inline","paragraph":{"type":"paragraph","content":[{
                    "type":"text","text":"Corporate reference","marks":[{"type":"link","attrs":{"href":href}}]
                }]}}}
            ]}]}}
        })
    };
    let first = write_item_input(
        &root,
        make_input("https://intranet.corp.internal/manual"),
        None,
        "2026-09-30",
    )
    .unwrap();
    let second = write_item_input(
        &root,
        make_input("http://127.0.0.1:9/offline"),
        first["item"]["revision"].as_str(),
        "2026-09-30",
    )
    .unwrap();
    let error = write_item_input(
        &root,
        make_input("https://doi:10.21926/example"),
        second["item"]["revision"].as_str(),
        "2026-09-30",
    )
    .unwrap_err();
    assert!(error.contains("URL"), "{error}");
    assert_eq!(
        read_project_item(&root, &id).unwrap().unwrap(),
        second["item"]
    );
    // Prepared/imported writes use the same transaction validation.
    let mut prepared = second["item"].clone();
    prepared["revision"] = serde_json::json!(uuid::Uuid::now_v7().to_string());
    prepared["content"]["document"]["content"][0]["content"][0]["attrs"]["paragraph"]["content"]
        [0]["marks"][0]["attrs"]["href"] = serde_json::json!("https://[broken");
    assert!(
        write_project_item(
            &root,
            &prepared,
            second["item"]["revision"].as_str(),
            "2026-09-30"
        )
        .unwrap_err()
        .contains("URL")
    );
    assert_eq!(
        read_project_item(&root, &id).unwrap().unwrap(),
        second["item"]
    );
    temporary.close().unwrap();
}

#[test]
fn rejects_independently_authored_page_references() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().join("project");
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("test-fixtures/pdf-project");
    let config: serde_json::Value =
        serde_json::from_slice(&std::fs::read(fixture.join("bluink.json")).unwrap()).unwrap();
    create_project_config(&root, config).unwrap();
    crate::domain::test_support::seed_fixture_snippets(&root);
    crate::domain::project::properties::initialize(&root).unwrap();
    let id = uuid::Uuid::now_v7().to_string();
    let input = serde_json::json!({"id":id,"meta":{"type":"page","title":"Page"},
            "content":{"document": {"type":"doc","content":[{"type":"paragraph"}]}}});
    let first = write_item_input(&root, input.clone(), None, "2026-09-28").unwrap();
    let mut self_reference = input;
    self_reference["references"] = serde_json::json!([id]);
    assert!(
        write_item_input(
            &root,
            self_reference,
            first["item"]["revision"].as_str(),
            "2026-09-28"
        )
        .unwrap_err()
        .contains("authored in the document")
    );
    temporary.close().unwrap();
}

#[test]
fn rejects_stale_writes_and_snapshots_changed_content() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().join("project");
    let config: serde_json::Value = serde_json::from_slice(
        &std::fs::read(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("test-fixtures/pdf-project/bluink.json"),
        )
        .unwrap(),
    )
    .unwrap();
    create_project_config(&root, config).unwrap();
    crate::domain::test_support::seed_fixture_snippets(&root);
    crate::domain::project::properties::initialize(&root).unwrap();
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("test-fixtures/pdf-project/items/be/01a0e2a0-0000-7000-8000-000000000001.json");
    let mut item: serde_json::Value =
        serde_json::from_slice(&std::fs::read(fixture).unwrap()).unwrap();
    let id = item["id"].as_str().unwrap().to_owned();
    let first_revision = item["revision"].as_str().unwrap().to_owned();
    write_project_item(&root, &item, None, "2026-09-28").unwrap();
    assert!(write_project_item(&root, &item, None, "2026-09-28").is_err());
    item["revision"] = serde_json::json!("01a0e2a7-d131-792d-a0bd-26a49033486e");
    item["analytics"]["current"]["contentHash"] = serde_json::json!("changed");
    write_project_item(&root, &item, Some(&first_revision), "2026-09-28").unwrap();
    assert_eq!(
        read_project_item(&root, &id).unwrap().unwrap()["revision"],
        item["revision"]
    );
    assert!(write_project_item(&root, &item, Some(&first_revision), "2026-09-28").is_err());
    let count = crate::store::Store::shared(&root)
        .unwrap()
        .read(|tx| {
            Ok(tx.query_row::<i64, _, _>(
                "SELECT count(*) FROM revision r JOIN entity e ON e.id=r.item_id WHERE e.uuid=?1",
                [&id],
                |row| row.get(0),
            )?)
        })
        .unwrap();
    assert_eq!(count, 1);
    temporary.close().unwrap();
}

#[test]
fn checks_property_declarations_before_an_item_is_committed() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().join("project");
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("test-fixtures/pdf-project");
    let config: serde_json::Value =
        serde_json::from_slice(&std::fs::read(fixture.join("bluink.json")).unwrap()).unwrap();
    create_project_config(&root, config).unwrap();
    crate::domain::test_support::seed_fixture_snippets(&root);
    crate::domain::project::properties::initialize(&root).unwrap();
    crate::application::catalogue_writes::create_property(
        &crate::store::Store::shared(&root).unwrap(),
        &serde_json::json!({"key":"workflow","valueType":"select","options":["draft"]}),
    )
    .unwrap();
    let mut item: serde_json::Value = serde_json::from_slice(
        &std::fs::read(fixture.join("items/be/01a0e2a0-0000-7000-8000-000000000001.json")).unwrap(),
    )
    .unwrap();
    item["meta"]["workflow"] = serde_json::json!("review");
    assert!(
        write_project_item(&root, &item, None, "2026-09-28")
            .unwrap_err()
            .contains("declared option")
    );
    let id = item["id"].as_str().unwrap();
    assert!(read_project_item(&root, id).unwrap().is_none());
    item["meta"]["workflow"] = serde_json::json!("draft");
    write_project_item(&root, &item, None, "2026-09-28").unwrap();
    temporary.close().unwrap();
}

#[test]
fn checks_new_pin_targets_in_the_item_transaction() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().join("project");
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("test-fixtures/pdf-project");
    let config: serde_json::Value =
        serde_json::from_slice(&std::fs::read(fixture.join("bluink.json")).unwrap()).unwrap();
    create_project_config(&root, config).unwrap();
    crate::domain::test_support::seed_fixture_snippets(&root);
    crate::domain::project::properties::initialize(&root).unwrap();
    let mut item: serde_json::Value = serde_json::from_slice(
        &std::fs::read(fixture.join("items/be/01a0e2a0-0000-7000-8000-000000000001.json")).unwrap(),
    )
    .unwrap();
    let missing_scope = "01a0e2a7-d131-792d-a0bd-26a49033486e";
    item["pins"] = serde_json::json!([missing_scope]);
    assert!(
        write_project_item(&root, &item, None, "2026-09-28")
            .unwrap_err()
            .contains("active workspace")
    );
    let mut scope = item.clone();
    let scope_id = "01a0e2a7-d131-792d-a0bd-26a49033486f";
    scope["id"] = serde_json::json!(scope_id);
    scope["revision"] = serde_json::json!(uuid::Uuid::now_v7().to_string());
    scope["meta"]["type"] = serde_json::json!("workspace");
    scope["content"] = crate::domain::project::workspace::initial_content();
    scope.as_object_mut().unwrap().remove("pins");
    write_project_item(&root, &scope, None, "2026-09-28").unwrap();
    item["pins"] = serde_json::json!([scope_id]);
    write_project_item(&root, &item, None, "2026-09-28").unwrap();
    temporary.close().unwrap();
}
