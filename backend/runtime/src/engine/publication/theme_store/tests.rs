// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use super::*;
use crate::engine::project::{config::open_or_create_project, write::write_item_input};
use serde_json::json;

fn project() -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    open_or_create_project(root.path(), "UTC").unwrap();
    root
}

#[test]
fn atomic_crud_revisions_ids_all_targets_and_builtin_cloning() {
    let root = project();
    let builtin = read(root.path(), "classic", None).unwrap();
    assert_eq!(list(root.path(), None).unwrap().len(), 2);
    assert!(
        update(
            root.path(),
            "classic",
            &builtin.summary.revision,
            &builtin.definition
        )
        .unwrap_err()
        .to_string()
        .contains("read-only")
    );
    assert!(
        delete(root.path(), "classic", &builtin.summary.revision)
            .unwrap_err()
            .to_string()
            .contains("cannot be deleted")
    );
    let created = create(root.path(), "  Research notes  ").unwrap();
    let id = &created.summary.id;
    assert!(theme::is_theme_id(id));
    assert_eq!(created.summary.name, "Research notes");
    assert!(created.assets.is_empty());
    let mut draft = created.definition.clone();
    draft.root.font_size = super::super::theme_format::Value::Literal("14pt".parse().unwrap());
    let validated = validate(root.path(), id, &created.summary.revision, &draft).unwrap();
    assert_eq!(
        validated.resolved.len(),
        super::super::plan::PublicationFormat::ALL.len()
    );
    assert!(
        validated
            .resolved
            .values()
            .all(|value| (value.root_font_size_pt - 14.0).abs() < f64::EPSILON)
    );
    assert_eq!(
        read(root.path(), id, None).unwrap().summary.revision,
        created.summary.revision
    );
    let changed = update(root.path(), id, &created.summary.revision, &draft).unwrap();
    assert_ne!(changed.summary.revision, created.summary.revision);
    assert!(
        update(root.path(), id, &created.summary.revision, &draft)
            .unwrap_err()
            .to_string()
            .contains("Version conflict")
    );
    assert!(
        delete(root.path(), id, &created.summary.revision)
            .unwrap_err()
            .to_string()
            .contains("Version conflict")
    );
    let duplicate = duplicate(
        root.path(),
        "classic",
        &builtin.summary.revision,
        "Cloned Classic",
        None,
    )
    .unwrap();
    assert_ne!(duplicate.summary.id, builtin.summary.id);
    assert_ne!(duplicate.summary.id, *id);
    assert_eq!(duplicate.assets.len(), builtin.assets.len());
    assert_eq!(duplicate.definition.web, builtin.definition.web);
    assert_eq!(list(root.path(), None).unwrap().len(), 4);
    delete(root.path(), id, &changed.summary.revision).unwrap();
    assert!(read(root.path(), id, None).is_err());
    assert!(
        fs::read_dir(root.path().join("themes"))
            .unwrap()
            .all(|entry| !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with('.'))
    );
}

#[test]
fn invalid_drafts_leave_canonical_bytes_and_assets_untouched() {
    let root = project();
    let builtin = read(root.path(), "classic", None).unwrap();
    let created = duplicate(
        root.path(),
        "classic",
        &builtin.summary.revision,
        "Custom",
        None,
    )
    .unwrap();
    let id = &created.summary.id;
    let path = root.path().join("themes").join(id).join("manifest.json");
    let bytes = fs::read(&path).unwrap();
    let mut invalid = created.definition.clone();
    invalid.id = "renamed".into();
    assert!(
        update(root.path(), id, &created.summary.revision, &invalid)
            .unwrap_err()
            .to_string()
            .contains("immutable")
    );
    invalid = created.definition.clone();
    invalid.web.html = Some("../outside.html".into());
    assert!(update(root.path(), id, &created.summary.revision, &invalid).is_err());
    invalid = created.definition.clone();
    invalid.targets.insert(
        "docx".into(),
        serde_json::from_value(json!({"root":{"fontSize":"-2pt"}})).unwrap(),
    );
    assert!(validate(root.path(), id, &created.summary.revision, &invalid).is_err());
    assert!(update(root.path(), id, &created.summary.revision, &invalid).is_err());
    assert_eq!(fs::read(&path).unwrap(), bytes);
    let mut valid = created.definition.clone();
    valid.name = "New display name".into();
    let saved = update(root.path(), id, &created.summary.revision, &valid).unwrap();
    assert_eq!(
        serde_json::to_value(saved.assets).unwrap(),
        serde_json::to_value(created.assets).unwrap()
    );
    for name in ["", "\nBad", &"x".repeat(121)] {
        assert!(create(root.path(), name).is_err());
    }
    assert!(create(root.path(), "Bad\nname").is_err());
}

#[test]
fn saved_and_archived_publications_guard_deletion_without_search() {
    let root = project();
    let created = create(root.path(), "Shared").unwrap();
    let id = &created.summary.id;
    let mut publication: serde_json::Value = serde_json::from_str(include_str!("../../../../../test-fixtures/pdf-project/items/01/01a0e7f5-d342-7f23-bd3d-6e27064a16cd/item.json")).unwrap();
    publication["content"]["themeId"] = json!(id);
    let input =
        json!({"id":publication["id"],"meta":publication["meta"],"content":publication["content"]});
    let saved = write_item_input(root.path(), input, None, "2026-10-14").unwrap();
    assert!(
        delete(root.path(), id, &created.summary.revision)
            .unwrap_err()
            .to_string()
            .contains("used")
    );
    let record = read(root.path(), id, None).unwrap();
    assert_eq!(record.summary.usages.len(), 1);
    assert!(!record.summary.usages[0].archived);
    crate::engine::project::item_mutations::archive_item(
        root.path(),
        saved["item"]["id"].as_str().unwrap(),
        "2026-10-14",
    )
    .unwrap();
    assert!(read(root.path(), id, None).unwrap().summary.usages[0].archived);
    assert!(delete(root.path(), id, &created.summary.revision).is_err());
    publication["content"]["themeId"] = json!("missing-theme");
    assert!(
        write_item_input(
            root.path(),
            json!({"meta":publication["meta"],"content":publication["content"]}),
            None,
            "2026-10-14"
        )
        .unwrap_err()
        .contains("does not exist")
    );
}

#[test]
fn concurrent_updates_have_exactly_one_revision_winner() {
    let root = project();
    let created = create(root.path(), "Shared").unwrap();
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(3));
    let writers = ["First", "Second"].map(|name| {
        let path = root.path().to_owned();
        let id = created.summary.id.clone();
        let revision = created.summary.revision.clone();
        let mut definition = created.definition.clone();
        definition.name = name.into();
        let barrier = barrier.clone();
        std::thread::spawn(move || {
            barrier.wait();
            update(&path, &id, &revision, &definition)
        })
    });
    barrier.wait();
    let results = writers.map(|writer| writer.join().unwrap());
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert!(
        results
            .iter()
            .filter_map(|result| result.as_ref().err())
            .all(|error| error.to_string().contains("Version conflict"))
    );
}

#[test]
fn external_asset_changes_conflict_and_clones_preserve_binary_bytes() {
    let root = project();
    let created = create(root.path(), "Assets").unwrap();
    let folder = root.path().join("themes").join(&created.summary.id);
    fs::create_dir(folder.join("assets")).unwrap();
    fs::write(folder.join("assets/binary.bin"), [0, 255, 1, 0]).unwrap();
    assert!(
        update(
            root.path(),
            &created.summary.id,
            &created.summary.revision,
            &created.definition
        )
        .unwrap_err()
        .to_string()
        .contains("Version conflict")
    );
    let current = read(root.path(), &created.summary.id, None).unwrap();
    let clone = duplicate(
        root.path(),
        &current.summary.id,
        &current.summary.revision,
        "Copy",
        None,
    )
    .unwrap();
    assert_eq!(
        fs::read(
            root.path()
                .join("themes")
                .join(clone.summary.id)
                .join("assets/binary.bin")
        )
        .unwrap(),
        [0, 255, 1, 0]
    );
    let oversized = fs::File::create(folder.join("assets/oversized.bin")).unwrap();
    oversized
        .set_len(theme::MAX_THEME_BYTES as u64 + 1)
        .unwrap();
    assert!(
        read(root.path(), &created.summary.id, None)
            .unwrap_err()
            .to_string()
            .contains("16 MiB")
    );
}

#[test]
fn duplicates_unsaved_values_atomically_without_changing_source_assets_or_definition() {
    let root = project();
    let original = read(root.path(), "classic", None).unwrap();
    let mut draft = original.definition.clone();
    draft.root.font_size = super::super::theme_format::Value::Literal("15pt".parse().unwrap());
    let copied = duplicate(
        root.path(),
        "classic",
        &original.summary.revision,
        "Edited copy",
        Some(&draft),
    )
    .unwrap();
    assert_eq!(copied.definition.root.font_size, draft.root.font_size);
    assert_ne!(copied.definition.id, draft.id);
    assert_eq!(
        serde_json::to_value(copied.assets).unwrap(),
        serde_json::to_value(original.assets).unwrap()
    );
    assert_eq!(
        read(root.path(), "classic", None).unwrap().definition,
        original.definition
    );
    draft.id = "other".into();
    assert!(
        duplicate(
            root.path(),
            "classic",
            &original.summary.revision,
            "Invalid",
            Some(&draft)
        )
        .unwrap_err()
        .to_string()
        .contains("immutable")
    );
    assert_eq!(list(root.path(), None).unwrap().len(), 3);
}

#[test]
fn linked_packages_and_builtin_shadowing_are_rejected() {
    let root = project();
    let outside = project();
    let outside_entries = fs::read_dir(outside.path()).unwrap().count();
    #[cfg(windows)]
    assert!(
        std::process::Command::new("cmd")
            .args(["/c", "mklink", "/J"])
            .arg(root.path().join("themes"))
            .arg(outside.path())
            .output()
            .unwrap()
            .status
            .success()
    );
    #[cfg(unix)]
    std::os::unix::fs::symlink(outside.path(), root.path().join("themes")).unwrap();
    assert!(create(root.path(), "Outside").is_err());
    assert!(list(root.path(), None).is_err());
    assert!(read(root.path(), "classic", None).is_err());
    assert_eq!(
        fs::read_dir(outside.path()).unwrap().count(),
        outside_entries
    );
    #[cfg(windows)]
    fs::remove_dir(root.path().join("themes")).unwrap();
    #[cfg(unix)]
    fs::remove_file(root.path().join("themes")).unwrap();
    let shadow = project();
    fs::create_dir_all(shadow.path().join("themes/classic")).unwrap();
    assert!(
        read(shadow.path(), "classic", None)
            .unwrap_err()
            .to_string()
            .contains("reserved")
    );
    for id in ["../outside", "C:/outside", "CON", "con", "a/b"] {
        assert!(read(shadow.path(), id, None).is_err());
    }
}

#[test]
fn publication_save_and_theme_deletion_cannot_both_succeed() {
    let root = project();
    let created = create(root.path(), "Shared").unwrap();
    let mut publication: serde_json::Value = serde_json::from_str(include_str!("../../../../../test-fixtures/pdf-project/items/01/01a0e7f5-d342-7f23-bd3d-6e27064a16cd/item.json")).unwrap();
    publication["content"]["themeId"] = json!(created.summary.id);
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(3));
    let path = root.path().to_owned();
    let deleting = barrier.clone();
    let deletion = std::thread::spawn(move || {
        deleting.wait();
        delete(&path, &created.summary.id, &created.summary.revision)
    });
    let path = root.path().to_owned();
    let writing = barrier.clone();
    let save = std::thread::spawn(move || {
        writing.wait();
        write_item_input(
            &path,
            json!({"meta":publication["meta"],"content":publication["content"]}),
            None,
            "2026-10-14",
        )
    });
    barrier.wait();
    let deleted = deletion.join().unwrap();
    let saved = save.join().unwrap();
    assert_ne!(deleted.is_ok(), saved.is_ok());
    if let Err(error) = deleted {
        assert!(error.to_string().contains("used"));
    }
    if let Err(error) = saved {
        assert!(error.contains("does not exist"));
    }
}
