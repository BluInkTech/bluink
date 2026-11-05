// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use super::{Folder, import};
use crate::domain::project::{config::PROJECT_FORMAT_VERSION, identity::ItemId};
use crate::store::{Store, commands::assemble_item, item, project};
use serde_json::{Value, json};
use uuid::Uuid;

fn source() -> (tempfile::TempDir, Value, Value) {
    let source = tempfile::tempdir().unwrap();
    let template: Value = serde_json::from_slice(
        &std::fs::read(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("resources/workspace-starter.json"),
        )
        .unwrap(),
    )
    .unwrap();
    let workspace = assemble_item(&json!({"meta":{"type":"workspace","title":"Main"},"content":crate::store::commands::with_authoring_values(|values| crate::domain::project::workspace::instantiate(&template, values)).unwrap()}),None,"UTC").unwrap();
    let page = assemble_item(&json!({"meta":{"type":"page","title":"An imported page"},"tags":["history"],"content":{"document":{"type":"doc","content":[{"type":"paragraph","attrs":{"id":"paragraph-1"},"content":[{"type":"text","text":"A durable imported paragraph"}]}]}}}),None,"UTC").unwrap();
    let config = json!({"id":Uuid::now_v7().to_string(),"format":"bluink-project","formatVersion":PROJECT_FORMAT_VERSION,"defaultWorkspaceId":workspace["id"],"analytics":{"timezone":"UTC","eventRetentionDays":14}});
    std::fs::write(
        source.path().join("bluink.json"),
        serde_json::to_vec(&config).unwrap(),
    )
    .unwrap();
    for value in [&workspace, &page] {
        let path = ItemId::try_from(value["id"].as_str().unwrap())
            .unwrap()
            .path(source.path());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, serde_json::to_vec(value).unwrap()).unwrap();
    }
    (source, page, config)
}

#[test]
fn folder_import_preserves_project_and_items_without_local_activity() {
    let (source, page, config) = source();
    let parent = tempfile::tempdir().unwrap();
    let destination = parent.path().join("local");
    let mut phases = Vec::new();
    import(
        &Folder::open(source.path()).unwrap(),
        &destination,
        |phase, _, _| phases.push(phase.to_owned()),
    )
    .unwrap();
    source.close().unwrap();
    let store = Store::open(&destination).unwrap();
    store
        .read(|tx| {
            assert_eq!(project::read_config(tx)?, config);
            assert_eq!(
                item::read(tx, page["id"].as_str().unwrap(), true)?,
                Some(page.clone())
            );
            assert_eq!(
                tx.query_row::<i64, _, _>("SELECT count(*) FROM activity_event", [], |row| row
                    .get(0))?,
                0
            );
            assert_eq!(
                tx.query_row::<i64, _, _>("SELECT count(*) FROM dirty_path", [], |row| row.get(0))?,
                3
            );
            assert_eq!(
                tx.query_row::<i64, _, _>("SELECT count(*) FROM item_fts", [], |row| row.get(0))?,
                4
            );
            Ok(())
        })
        .unwrap();
    assert!(destination.join("history.git").is_dir());
    assert!(!destination.join("items").exists());
    assert_eq!(phases.last().map(String::as_str), Some("Ready"));
}

#[test]
fn failed_import_never_publishes_a_project() {
    let (source, _, mut config) = source();
    config["defaultWorkspaceId"] = json!(Uuid::now_v7().to_string());
    std::fs::write(
        source.path().join("bluink.json"),
        serde_json::to_vec(&config).unwrap(),
    )
    .unwrap();
    let parent = tempfile::tempdir().unwrap();
    let destination = parent.path().join("local");
    assert!(
        import(
            &Folder::open(source.path()).unwrap(),
            &destination,
            |_, _, _| {}
        )
        .is_err()
    );
    assert!(!destination.exists());
    assert_eq!(std::fs::read_dir(parent.path()).unwrap().count(), 0);
}

#[test]
fn export_includes_uncheckpointed_assets_and_rejects_corruption_before_publication() {
    let (source, page, config) = source();
    let owner = ItemId::try_from(page["id"].as_str().unwrap()).unwrap();
    let directory = owner.asset_directory(source.path());
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(directory.join("cover.png"), b"cover bytes").unwrap();
    let root = tempfile::tempdir().unwrap();
    let local = root.path().join("local");
    import(&Folder::open(source.path()).unwrap(), &local, |_, _, _| {}).unwrap();
    let store = Store::open(&local).unwrap();
    let destination = root.path().join("portable");
    super::export(&store, &destination).unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&std::fs::read(destination.join("bluink.json")).unwrap())
            .unwrap(),
        config
    );
    assert_eq!(
        std::fs::read(owner.path(&destination)).unwrap(),
        crate::domain::project::canonical_json::to_vec(&page).unwrap()
    );
    assert_eq!(
        std::fs::read(owner.asset_directory(&destination).join("cover.png")).unwrap(),
        b"cover bytes"
    );
    assert!(!destination.join(crate::store::DATABASE_NAME).exists());
    assert!(!destination.join("history.git").exists());
    assert!(super::export(&store, &destination).is_err());
    let oid = store
        .read(|tx| Ok(tx.query_row::<String, _, _>("SELECT oid FROM asset", [], |row| row.get(0))?))
        .unwrap();
    let path = crate::versioning::checkpoints::available_asset_path(&local, &oid, 11).unwrap();
    std::fs::write(path, b"wrong bytes").unwrap();
    let incomplete = root.path().join("incomplete");
    assert!(super::export(&store, &incomplete).is_err());
    assert!(!incomplete.exists());
    assert!(!store.read(crate::store::maintenance::protected).unwrap());
}

#[test]
fn first_creation_publishes_valid_default_workspace_and_reopens_same_database() {
    let parent = tempfile::tempdir().unwrap();
    let root = parent.path().join("local");
    let template: Value = serde_json::from_slice(
        &std::fs::read(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("resources/workspace-starter.json"),
        )
        .unwrap(),
    )
    .unwrap();
    let (store, config) = project::open_or_create(&root, "UTC", &template).unwrap();
    store
        .read(|tx| {
            let workspace =
                item::read(tx, config["defaultWorkspaceId"].as_str().unwrap(), true)?.unwrap();
            assert_eq!(workspace["meta"]["type"], "workspace");
            assert!(
                tx.query_row::<i64, _, _>(
                    "SELECT count(*) FROM field_definition WHERE declaration_id IS NOT NULL",
                    [],
                    |row| row.get(0)
                )? > 0
            );
            Ok(())
        })
        .unwrap();
    let (second, reopened) = project::open_or_create(&root, "UTC", &template).unwrap();
    assert_eq!(config, reopened);
    assert!(std::sync::Arc::ptr_eq(&store, &second));
}
