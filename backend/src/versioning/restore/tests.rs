// SPDX-License-Identifier: MPL-2.0
use super::*;
use crate::versioning::test_support;

#[test]
fn scoped_archive_restores_owned_assets_and_only_current_active_children() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().join("project");
    test_support::create(&root, "UTC").unwrap();
    let store = History::register(&root, temporary.path().join("user").to_str()).unwrap();
    store.initialize().unwrap();
    let save = |value, previous| {
        test_support::save(&root, value, previous, "2026-11-01").unwrap()["item"].clone()
    };
    let archived = save(
        json!({"meta":{"type":"page","state":"archived","title":"Original"},"content":{"document":{"type":"doc","content":[{"type":"paragraph"}]}}}),
        None,
    );
    let id = archived["id"].as_str().unwrap();
    test_support::asset(&root, id, "cover.png", b"old cover").unwrap();
    let first = store.checkpoint().unwrap().head.unwrap();
    save(
        json!({"id":id,"meta":{"state":"active"}}),
        archived["revision"].as_str(),
    );
    let other = save(
        json!({"meta":{"type":"page","title":"Another page"},"content":{"document":{"type":"doc","content":[{"type":"paragraph"}]}}}),
        None,
    );
    let task = |owner: &Value| {
        save(
            json!({"meta":{"type":"task","title":"Review"},"content":{"text":"Review","noteId":owner["id"]}}),
            None,
        )
    };
    let owned = task(&archived);
    let unrelated = task(&other);
    test_support::asset(&root, id, "cover.png", b"new cover").unwrap();
    test_support::asset(
        &root,
        other["id"].as_str().unwrap(),
        "cover.png",
        b"other cover",
    )
    .unwrap();
    let preview = store
        .preview_restore(&HistoryPreviewRequest {
            commit_id: first.clone(),
            item_id: Some(id.into()),
        })
        .unwrap();
    assert_eq!(preview.archived_tasks, vec![owned["id"].as_str().unwrap()]);
    assert_eq!(preview.changed_assets, 1);
    store
        .apply_restore(&HistoryRestoreRequest {
            commit_id: first,
            item_id: Some(id.into()),
            expected_head: preview.expected_head,
            expected_generation: preview.expected_generation,
        })
        .unwrap();
    for (item, state) in [(&owned, "archived"), (&unrelated, "active")] {
        assert_eq!(
            test_support::read(&root, item["id"].as_str().unwrap())
                .unwrap()
                .unwrap()["meta"]["state"],
            state
        );
    }
    assert_eq!(
        std::fs::read(test_support::asset_file(&root, id, "cover.png").unwrap()).unwrap(),
        b"old cover"
    );
    assert_eq!(
        std::fs::read(
            test_support::asset_file(&root, other["id"].as_str().unwrap(), "cover.png").unwrap()
        )
        .unwrap(),
        b"other cover"
    );
}

#[test]
fn scoped_preview_checks_restored_references_without_decoding_unrelated_blobs() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().join("project");
    test_support::create(&root, "UTC").unwrap();
    let store = History::register(&root, temporary.path().join("user").to_str()).unwrap();
    store.initialize().unwrap();
    let page=test_support::save(&root,json!({"meta":{"type":"page"},"content":{"document":{"type":"doc","content":[{"type":"paragraph"}]}}}),None,"2026-11-01").unwrap()["item"].clone();
    let id = page["id"].as_str().unwrap();
    let head = Oid::from_str(&store.checkpoint().unwrap().head.unwrap()).unwrap();
    let repo = crate::versioning::checkpoints::open(&root).unwrap();
    let generation = store.generation().unwrap();
    let path = |id: &str| {
        ItemId::try_from(id)
            .unwrap()
            .path(Path::new(""))
            .to_string_lossy()
            .replace('\\', "/")
    };
    let other = uuid::Uuid::now_v7().to_string();
    let historical = |replacement: &Value| {
        let files = BTreeMap::from([
            (
                path(&other),
                Some(repo.blob(b"unrelated invalid JSON").unwrap()),
            ),
            (
                path(id),
                Some(
                    repo.blob(&project::canonical_json::to_vec(replacement).unwrap())
                        .unwrap(),
                ),
            ),
        ]);
        let tree = repository::changed_tree(&repo, Some(head), &files).unwrap();
        store
            .commit(&repo, Some(head), tree, "Historical test")
            .unwrap()
    };
    let selected = historical(&page);
    scoped::destination(&store, &repo, head, selected, generation, id).unwrap();
    assert!(scoped::destination(&store, &repo, head, selected, -1, id).is_err());
    let mut invalid = page.clone();
    invalid["pins"] = json!([uuid::Uuid::now_v7().to_string()]);
    let error =
        scoped::destination(&store, &repo, head, historical(&invalid), generation, id).unwrap_err();
    assert!(error.to_string().contains("workspace"), "{error}");
}

#[test]
fn restore_creates_fresh_revision_and_retains_superseded_content() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().join("project");
    test_support::create(&root, "UTC").unwrap();
    let store = History::register(&root, temporary.path().join("user").to_str()).unwrap();
    store.initialize().unwrap();
    let original=test_support::save(&root,json!({"meta":{"type":"page","title":"Original"},"content":{"document":{"type":"doc","content":[{"type":"paragraph","content":[{"type":"text","text":"First draft"}]}]}}}),None,"2026-11-01").unwrap()["item"].clone();
    let head = store.checkpoint().unwrap().head.unwrap();
    let changed = test_support::save(
        &root,
        json!({"id":original["id"],"meta":{"title":"Changed"}}),
        original["revision"].as_str(),
        "2026-11-01",
    )
    .unwrap()["item"]
        .clone();
    let preview = store
        .preview_restore(&HistoryPreviewRequest {
            commit_id: head.clone(),
            item_id: Some(original["id"].as_str().unwrap().into()),
        })
        .unwrap();
    store
        .apply_restore(&HistoryRestoreRequest {
            commit_id: head,
            item_id: preview.item_id,
            expected_head: preview.expected_head,
            expected_generation: preview.expected_generation,
        })
        .unwrap();
    let current = test_support::read(&root, original["id"].as_str().unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(current["meta"]["title"], "Original");
    assert_eq!(current["content"], original["content"]);
    assert_ne!(current["revision"], original["revision"]);
    assert_ne!(current["revision"], changed["revision"]);
    store
        .canonical()
        .unwrap()
        .read(|tx| {
            assert_eq!(
                crate::store::item::read_revision(
                    tx,
                    changed["id"].as_str().unwrap(),
                    changed["revision"].as_str().unwrap()
                )?
                .unwrap(),
                changed
            );
            Ok(())
        })
        .unwrap();
    assert!(!store.status().unwrap().dirty);
    assert!(!root.join("items").exists());
}

#[test]
fn saves_after_preview_are_preserved_and_require_a_new_preview() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().join("project");
    test_support::create(&root, "UTC").unwrap();
    let store = History::register(&root, temporary.path().join("user").to_str()).unwrap();
    store.initialize().unwrap();
    let original=test_support::save(&root,json!({"meta":{"type":"page","title":"Original"},"content":{"document":{"type":"doc","content":[{"type":"paragraph"}]}}}),None,"2026-11-01").unwrap()["item"].clone();
    let head = store.checkpoint().unwrap().head.unwrap();
    let preview = store
        .preview_restore(&HistoryPreviewRequest {
            commit_id: head.clone(),
            item_id: Some(original["id"].as_str().unwrap().into()),
        })
        .unwrap();
    let changed = test_support::save(
        &root,
        json!({"id":original["id"],"meta":{"title":"Keep this save"}}),
        original["revision"].as_str(),
        "2026-11-01",
    )
    .unwrap()["item"]
        .clone();
    assert!(
        store
            .apply_restore(&HistoryRestoreRequest {
                commit_id: head,
                item_id: preview.item_id,
                expected_head: preview.expected_head,
                expected_generation: preview.expected_generation
            })
            .is_err()
    );
    assert_eq!(
        test_support::read(&root, changed["id"].as_str().unwrap())
            .unwrap()
            .unwrap(),
        changed
    );
}

#[test]
fn restore_cannot_remove_or_reopen_current_decisions() {
    use crate::domain::merge::{
        decisions::{Proposal, prepare},
        item::merge_item,
        model::{Context, Source},
    };
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().join("project");
    test_support::create(&root, "UTC").unwrap();
    let store = History::register(&root, temporary.path().join("user").to_str()).unwrap();
    store.initialize().unwrap();
    let repo = crate::versioning::checkpoints::open(&store.root).unwrap();
    let head = repository::head(&repo).unwrap().unwrap();
    let historical = repository::files(&repo, head).unwrap();
    let id = uuid::Uuid::now_v7().to_string();
    let base = crate::store::commands::assemble_item(&json!({"id":id,"meta":{"type":"page","title":"Base"},"content":{"document":{"type":"doc","content":[{"type":"paragraph"}]}}}), None, "UTC").unwrap();
    let mut left = base.clone();
    left["meta"]["title"] = json!("First");
    let mut right = base.clone();
    right["meta"]["title"] = json!("Second");
    let context = Context {
        target: id,
        base_epoch: "commit:base".into(),
        left: Source::Commit {
            id: "a".into(),
            committer_time: 1,
            device: "Laptop".into(),
        },
        right: Source::Commit {
            id: "b".into(),
            committer_time: 2,
            device: "Desktop".into(),
        },
    };
    let merged = merge_item(&context, Some(&base), Some(&left), Some(&right)).unwrap();
    let open = crate::store::commands::with_authoring_values(|values| {
        prepare(
            Proposal::Item(Box::new(merged.decisions.into_iter().next().unwrap())),
            None,
            "UTC",
            values,
        )
    })
    .unwrap();
    let mut resolved = open.clone();
    resolved["content"]["resolution"] =
        json!({"selection":"first","resolved_at":"2026-10-31T12:00:00.000Z","device":"Laptop"});
    let identity = ItemId::try_from(open["id"].as_str().unwrap()).unwrap();
    let path = format!(
        "items/{}/{}.json",
        identity.bucket(),
        open["id"].as_str().unwrap()
    );
    let mut current = historical.clone();
    current.insert(
        path.clone(),
        repo.blob(&project::canonical_json::to_vec(&resolved).unwrap())
            .unwrap(),
    );
    assert!(
        validate_target(&repo, &historical, &current)
            .unwrap_err()
            .to_string()
            .contains("decision")
    );
    let mut reopened = historical;
    reopened.insert(
        path,
        repo.blob(&project::canonical_json::to_vec(&open).unwrap())
            .unwrap(),
    );
    assert!(
        validate_target(&repo, &reopened, &current)
            .unwrap_err()
            .to_string()
            .contains("decision")
    );
    validate_target(&repo, &current, &current).unwrap();
}

#[test]
fn incremental_validation_checks_changes_and_dependents_of_changed_targets() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().join("project");
    test_support::create(&root, "UTC").unwrap();
    let store = History::register(&root, temporary.path().join("user").to_str()).unwrap();
    store.initialize().unwrap();
    let save =
        |value| test_support::save(&root, value, None, "2026-11-01").unwrap()["item"].clone();
    let page = save(
        json!({"meta":{"type":"page"},"content":{"document":{"type":"doc","content":[{"type":"paragraph"}]}}}),
    );
    save(
        json!({"meta":{"type":"task","title":"Review"},"content":{"text":"Review","noteId":page["id"]}}),
    );
    let head = Oid::from_str(&store.checkpoint().unwrap().head.unwrap()).unwrap();
    let repo = crate::versioning::checkpoints::open(&root).unwrap();
    let base = repository::files(&repo, head).unwrap();
    let path = |id: &str| {
        ItemId::try_from(id)
            .unwrap()
            .path(Path::new(""))
            .to_string_lossy()
            .replace('\\', "/")
    };
    let with = |files: &BTreeMap<String, Oid>, value: &Value| {
        let mut files = files.clone();
        files.insert(
            path(value["id"].as_str().unwrap()),
            repo.blob(&project::canonical_json::to_vec(value).unwrap())
                .unwrap(),
        );
        files
    };
    // An accepted base is trusted: an unchanged item is never decoded again.
    let mut accepted = base.clone();
    accepted.insert(
        path(&uuid::Uuid::now_v7().to_string()),
        repo.blob(b"unchanged invalid JSON").unwrap(),
    );
    let mut edited = page.clone();
    edited["meta"]["title"] = json!("Edited");
    validate_changes(&repo, &with(&accepted, &edited), &accepted).unwrap();
    // A changed item's own references are checked.
    let mut pinned = page.clone();
    pinned["pins"] = json!([uuid::Uuid::now_v7().to_string()]);
    let error = validate_changes(&repo, &with(&base, &pinned), &base).unwrap_err();
    assert!(error.to_string().contains("workspace"), "{error}");
    // Archiving the owner rechecks its unchanged active task.
    let mut archived = page;
    archived["meta"]["state"] = json!("archived");
    assert!(validate_changes(&repo, &with(&base, &archived), &base).is_err());
    drop(repo);
    store.stop().unwrap();
}
