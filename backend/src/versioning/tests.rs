// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk
use super::*;
use crate::domain::project::identity::ItemId;
use serde_json::{Value, json};
use std::path::PathBuf;

#[cfg(test)]
mod stock_client;

struct Fixture {
    store: std::sync::Arc<History>,
    root: PathBuf,
    temporary: tempfile::TempDir,
}
impl Fixture {
    fn new() -> Self {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("project");
        crate::versioning::test_support::create(&root, "UTC").unwrap();
        let store = History::register(&root, temporary.path().join("user").to_str()).unwrap();
        store.initialize().unwrap();
        Self {
            store,
            root,
            temporary,
        }
    }
    fn page(&self, text: &str) -> Value {
        crate::versioning::test_support::save(&self.root, json!({"meta":{"type":"page","title":text},"content":{"document":{"type":"doc","content":[{"type":"paragraph","content":[{"type":"text","text":text}]}]}}}), None, "2026-10-30").unwrap()["item"].clone()
    }
    fn close(self) {
        self.store.stop().unwrap();
        drop(self.store);
        self.temporary.close().unwrap();
    }
}

#[test]
fn named_versions_are_idempotent_and_read_original_content_without_a_remote() {
    let fixture = Fixture::new();
    let page = fixture.page("Original text");
    let request = model::NamedVersionRequest {
        request_id: uuid::Uuid::now_v7().to_string(),
        title: "First draft".into(),
        item_id: page["id"].as_str().map(str::to_owned),
    };
    let version = fixture.store.named_version(&request).unwrap();
    crate::versioning::test_support::save(
        &fixture.root,
        json!({"id":page["id"],"meta":{"title":"Changed title"}}),
        page["revision"].as_str(),
        "2026-10-30",
    )
    .unwrap();
    assert_eq!(fixture.store.named_version(&request).unwrap(), version);
    let original = fixture
        .store
        .historical_item(&model::HistoryItemRequest {
            commit_id: version.commit_id.clone(),
            item_id: page["id"].as_str().unwrap().into(),
        })
        .unwrap();
    assert_eq!(original, page);
    assert!(fixture.store.status().unwrap().dirty);
    let repo = crate::versioning::checkpoints::open(&fixture.root).unwrap();
    assert!(repo.remotes().unwrap().is_empty());
    assert!(repo.is_bare());
    assert!(!repo.path().join("index").exists());
    assert!(
        !fixture
            .temporary
            .path()
            .join("user/credentials.json")
            .exists()
    );
    let files = repository::files(&repo, git2::Oid::from_str(&version.commit_id).unwrap()).unwrap();
    assert!(files.keys().all(|path| {
        ![".cache", ".revisions", "analytics", ".transactions"]
            .iter()
            .any(|prefix| path.starts_with(prefix))
    }));
    drop(repo);
    fixture.close();
}

#[test]
fn local_assets_use_lfs_pointers_and_historical_bytes_survive_replacement() {
    let fixture = Fixture::new();
    let page = fixture.page("Images");
    let id = page["id"].as_str().unwrap();
    crate::versioning::test_support::asset(&fixture.root, id, "cover.png", b"original-image")
        .unwrap();
    let first = fixture.store.checkpoint().unwrap().head.unwrap();
    crate::versioning::test_support::asset(&fixture.root, id, "cover.png", b"new-image").unwrap();
    let replacement = fixture.store.checkpoint().unwrap().head.unwrap();
    let historical = fixture
        .store
        .historical_asset(&first, id, "cover.png")
        .unwrap();
    assert_eq!(std::fs::read(historical).unwrap(), b"original-image");
    let history = fixture
        .store
        .history(&model::HistoryRequest {
            item_id: Some(id.into()),
            before: None,
            limit: None,
        })
        .unwrap();
    assert_eq!(
        history.entries[0].id, replacement,
        "An asset-only edit belongs in its owner's history"
    );
    let repo = crate::versioning::checkpoints::open(&fixture.root).unwrap();
    let tree = repo
        .find_commit(git2::Oid::from_str(&first).unwrap())
        .unwrap()
        .tree()
        .unwrap();
    let entry = tree
        .get_path(
            &ItemId::try_from(id)
                .unwrap()
                .asset_directory(std::path::Path::new(""))
                .join("cover.png"),
        )
        .unwrap();
    assert!(
        std::str::from_utf8(repo.find_blob(entry.id()).unwrap().content())
            .unwrap()
            .starts_with("version https://git-lfs.github.com/spec/v1\n")
    );
    drop(entry);
    drop(tree);
    drop(repo);
    // Page history checks only that page's attachments.
    let other = fixture.page("Other images");
    let other = other["id"].as_str().unwrap();
    crate::versioning::test_support::asset(&fixture.root, other, "cover.png", b"other-image")
        .unwrap();
    fixture.store.checkpoint().unwrap();
    std::fs::remove_file(
        crate::versioning::test_support::asset_file(&fixture.root, other, "cover.png").unwrap(),
    )
    .unwrap();
    let request = |item_id: Option<&str>| model::HistoryRequest {
        item_id: item_id.map(str::to_owned),
        before: None,
        limit: None,
    };
    let scoped = fixture.store.history(&request(Some(id))).unwrap();
    assert!(
        scoped
            .entries
            .iter()
            .all(|entry| entry.attachments_available)
    );
    let project = fixture.store.history(&request(None)).unwrap();
    assert!(!project.entries[0].attachments_available);
    fixture.close();
}

#[test]
fn attachment_inventory_persists_history_and_current_tree_coverage() {
    use checkpoints::inventory::{Work, current, history};
    let fixture = Fixture::new();
    let page = fixture.page("Cover");
    let id = page["id"].as_str().unwrap();
    for value in [b"original".as_slice(), b"replacement".as_slice()] {
        super::test_support::asset(&fixture.root, id, "cover.png", value).unwrap();
        fixture.store.checkpoint().unwrap();
    }
    let repo = checkpoints::open(&fixture.root).unwrap();
    let parent = repository::head(&repo).unwrap().unwrap();
    super::test_support::save(
        &fixture.root,
        json!({"id":id,"meta":{"title":"New title"}}),
        page["revision"].as_str(),
        "2026-10-30",
    )
    .unwrap();
    fixture.store.checkpoint().unwrap();
    let head = repository::head(&repo).unwrap().unwrap();
    let sql = crate::store::Store::open(&fixture.root).unwrap();
    let mut warm = Work::default();
    assert_eq!(
        history(&repo, &sql, &[head], &[parent], &mut warm).unwrap(),
        Vec::<lfs::Pointer>::new()
    );
    let expected = history(&repo, &sql, &[head], &[], &mut warm).unwrap();
    assert_eq!(expected.len(), 2);
    assert_eq!(
        (warm.commit_diffs, warm.pointer_blobs),
        (0, 0),
        "local capture installed complete commit deltas"
    );
    let tip = current(&repo, &sql, &[head], &mut warm).unwrap();
    assert_eq!(tip.len(), 1);
    assert_eq!(tip[0].identity().1, 11);
    assert!(warm.shard_fills > 0);
    sql.write(|tx| {
        tx.execute("DELETE FROM git_inventory_commit", [])?;
        Ok(())
    })
    .unwrap();
    let mut cold = Work::default();
    assert_eq!(
        history(&repo, &sql, &[head], &[], &mut cold).unwrap(),
        expected
    );
    assert!(cold.commit_diffs >= 3);
    drop(sql);
    let reopened = crate::store::Store::open(&fixture.root).unwrap();
    let mut work = Work::default();
    assert_eq!(
        history(&repo, &reopened, &[head], &[], &mut work).unwrap(),
        expected
    );
    assert_eq!(current(&repo, &reopened, &[head], &mut work).unwrap(), tip);
    assert_eq!(
        (work.commit_diffs, work.pointer_blobs, work.shard_fills),
        (0, 0, 0)
    );
    drop(reopened);
    drop(repo);
    fixture.close();
}

#[test]
fn no_change_page_versions_remain_visible_and_conflicting_retries_are_rejected() {
    let fixture = Fixture::new();
    let page = fixture.page("Named version");
    let head = fixture.store.checkpoint().unwrap().head.unwrap();
    let request = model::NamedVersionRequest {
        request_id: uuid::Uuid::now_v7().to_string(),
        title: "Review copy".into(),
        item_id: page["id"].as_str().map(str::to_owned),
    };
    let version = fixture.store.named_version(&request).unwrap();
    assert_eq!(version.commit_id, head);
    let history = fixture
        .store
        .history(&model::HistoryRequest {
            item_id: request.item_id.clone(),
            before: None,
            limit: None,
        })
        .unwrap();
    assert!(
        history
            .entries
            .iter()
            .any(|entry| entry.named_versions.contains(&version))
    );
    let request = model::NamedVersionRequest {
        title: "Different request".into(),
        ..request
    };
    assert!(fixture.store.named_version(&request).is_err());
    fixture.close();
}

#[test]
fn bulk_checkpoints_persist_every_record_and_reopening_creates_no_empty_commit() {
    let fixture = Fixture::new();
    let pages: Vec<_> = (0..520)
        .map(|index| fixture.page(&format!("Bulk page {index}")))
        .collect();
    let head = fixture.store.checkpoint().unwrap().head.unwrap();
    let repo = crate::versioning::checkpoints::open(&fixture.root).unwrap();
    let tree = repo
        .find_commit(git2::Oid::from_str(&head).unwrap())
        .unwrap()
        .tree()
        .unwrap();
    for page in &pages {
        let path = ItemId::try_from(page["id"].as_str().unwrap())
            .unwrap()
            .path(std::path::Path::new(""));
        let entry = tree.get_path(&path).unwrap();
        let saved: Value =
            serde_json::from_slice(repo.find_blob(entry.id()).unwrap().content()).unwrap();
        assert_eq!(&saved, page);
    }
    assert!(
        std::fs::read_dir(repo.path().join("objects/pack"))
            .unwrap()
            .any(|entry| entry
                .unwrap()
                .path()
                .extension()
                .is_some_and(|extension| extension == "pack"))
    );
    drop(tree);
    drop(repo);
    fixture.store.initialize().unwrap();
    assert_eq!(
        fixture.store.status().unwrap().head.as_deref(),
        Some(head.as_str())
    );
    assert!(!fixture.store.status().unwrap().dirty);
    fixture.close();
}
