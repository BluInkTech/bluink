// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk
use super::*;
use serde_json::{Value, json};
use std::{path::PathBuf, sync::Arc};
mod gitea;

struct Pair {
    temp: tempfile::TempDir,
    first: Arc<History>,
    second: Arc<History>,
    origin: PathBuf,
}
fn sync(store: &History) {
    let prepared = store.prepare_sync().unwrap();
    store.apply_sync(&prepared).unwrap();
    store.push_sync(&prepared).unwrap();
    assert!(store.finish_sync(None).unwrap().error.is_none());
}
fn page(store: &History, text: &str) -> Value {
    crate::versioning::test_support::save(&store.root,json!({"meta":{"type":"page","title":text},"content":{"document":{"type":"doc","content":[{"type":"paragraph","content":[{"type":"text","text":text}]}]}}}),None,"2026-10-31").unwrap()["item"].clone()
}
fn load(store: &History, id: &str) -> Value {
    crate::versioning::test_support::read(&store.root, id)
        .unwrap()
        .unwrap()
}
impl Pair {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("first");
        crate::versioning::test_support::create(&root, "UTC").unwrap();
        let first = History::register(&root, temp.path().join("user1").to_str()).unwrap();
        first.initialize().unwrap();
        let origin = temp.path().join("origin.git");
        first
            .configure_sync(&model::ConfigureSync {
                origin: Some(origin.to_string_lossy().into_owned()),
                create: Some(true),
            })
            .unwrap();
        sync(&first);
        let second_root = temp.path().join("second");
        first
            .clone_project(&model::CloneProject {
                origin: origin.to_string_lossy().into_owned(),
                destination: second_root.to_string_lossy().into_owned(),
            })
            .unwrap();
        let second = History::register(&second_root, temp.path().join("user2").to_str()).unwrap();
        second.initialize().unwrap();
        Self {
            temp,
            first,
            second,
            origin,
        }
    }
    fn close(self) {
        self.first.stop().unwrap();
        self.second.stop().unwrap();
        drop(self.first);
        drop(self.second);
        self.temp.close().unwrap();
    }
}

#[test]
fn a_named_version_at_an_already_synced_head_needs_publication() {
    let pair = Pair::new();
    assert_eq!(pair.second.sync_status().unwrap().state, "synced");
    let before = pair.second.status().unwrap().head;
    pair.second
        .named_version(&crate::versioning::model::NamedVersionRequest {
            request_id: uuid::Uuid::now_v7().to_string(),
            title: "Reviewed".into(),
            item_id: None,
        })
        .unwrap();
    assert_eq!(pair.second.status().unwrap().head, before);
    assert_eq!(pair.second.sync_status().unwrap().state, "checkpointed");
    sync(&pair.second);
    assert_eq!(pair.second.sync_status().unwrap().state, "synced");
    let generation = pair.second.generation().unwrap();
    sync(&pair.second);
    assert_eq!(
        pair.second.generation().unwrap(),
        generation,
        "An already-published tag is not another logical change"
    );
    pair.close();
}

#[test]
fn two_projects_converge_preserving_independent_edits_assets_and_named_versions() {
    let pair = Pair::new();
    let a = page(&pair.first, "One");
    let b = page(&pair.second, "Two");
    let id = a["id"].as_str().unwrap();
    crate::versioning::test_support::asset(&pair.first.root, id, "cover.png", b"image bytes")
        .unwrap();
    let version = pair
        .first
        .named_version(&crate::versioning::model::NamedVersionRequest {
            request_id: uuid::Uuid::now_v7().to_string(),
            title: "Draft".into(),
            item_id: Some(id.into()),
        })
        .unwrap();
    sync(&pair.first);
    sync(&pair.second);
    sync(&pair.first);
    assert_eq!(load(&pair.second, id), load(&pair.first, id));
    assert_eq!(load(&pair.first, b["id"].as_str().unwrap()), b);
    assert_eq!(
        std::fs::read(
            pair.second
                .historical_asset(&version.commit_id, id, "cover.png")
                .unwrap()
        )
        .unwrap(),
        b"image bytes"
    );
    assert_eq!(
        pair.first.status().unwrap().head,
        pair.second.status().unwrap().head
    );
    assert_eq!(pair.first.sync_status().unwrap().state, "synced");
    pair.close();
}

#[test]
fn same_paragraph_conflict_survives_as_one_decision_and_one_visible_version() {
    let pair = Pair::new();
    let note = page(&pair.first, "Original");
    sync(&pair.first);
    sync(&pair.second);
    let id = note["id"].as_str().unwrap();
    for (store, text) in [(&pair.first, "First edit"), (&pair.second, "Second edit")] {
        let mut value = load(store, id);
        value["content"]["document"]["content"][0]["content"][0]["text"] = json!(text);
        crate::versioning::test_support::save(
            &store.root,
            value.clone(),
            value["revision"].as_str(),
            "2026-10-31",
        )
        .unwrap();
    }
    sync(&pair.first);
    sync(&pair.second);
    sync(&pair.first);
    assert_eq!(load(&pair.first, id), load(&pair.second, id));
    assert_eq!(decision_items(&pair.first).len(), 1);
    let repo = crate::versioning::checkpoints::open(&pair.first.root).unwrap();
    let files = repository::files(&repo, repository::head(&repo).unwrap().unwrap()).unwrap();
    let conflicts = files
        .iter()
        .filter(|(p, _)| p.starts_with("items/"))
        .map(|(_, oid)| serde_json::from_slice(repo.find_blob(*oid).unwrap().content()).unwrap())
        .filter(|v: &Value| v["meta"]["type"] == "conflict")
        .count();
    assert_eq!(conflicts, 1);
    drop(repo);
    pair.close();
}

#[test]
fn edits_after_preparation_reject_publication_without_overwrite() {
    let pair = Pair::new();
    let note = page(&pair.first, "Remote");
    sync(&pair.first);
    let prepared = pair.second.prepare_sync().unwrap();
    let local = page(&pair.second, "Typed while fetching");
    assert!(pair.second.apply_sync(&prepared).is_err());
    assert_eq!(load(&pair.second, local["id"].as_str().unwrap()), local);
    assert!(
        crate::versioning::test_support::read(&pair.second.root, note["id"].as_str().unwrap())
            .unwrap()
            .is_none()
    );
    sync(&pair.second);
    sync(&pair.first);
    assert_eq!(load(&pair.first, local["id"].as_str().unwrap()), local);
    pair.close();
}

#[test]
fn disconnect_keeps_history_and_unrelated_origin_is_rejected() {
    let pair = Pair::new();
    let before = pair.first.status().unwrap().head;
    pair.first
        .configure_sync(&model::ConfigureSync {
            origin: None,
            create: Some(false),
        })
        .unwrap();
    assert_eq!(pair.first.sync_status().unwrap().state, "local_only");
    assert_eq!(pair.first.status().unwrap().head, before);
    let unrelated = pair.temp.path().join("unrelated");
    crate::versioning::test_support::create(&unrelated, "UTC").unwrap();
    let store = History::register(&unrelated, pair.temp.path().join("user3").to_str()).unwrap();
    store.initialize().unwrap();
    assert!(
        store
            .configure_sync(&model::ConfigureSync {
                origin: Some(pair.origin.to_string_lossy().into_owned()),
                create: Some(false)
            })
            .is_err()
    );
    assert!(store.sync_status().unwrap().origin.is_none());
    drop(store);
    pair.close();
}

fn decision_items(store: &History) -> Vec<Value> {
    let repo = crate::versioning::checkpoints::open(&store.root).unwrap();
    let files = repository::files(&repo, repository::head(&repo).unwrap().unwrap()).unwrap();
    files
        .iter()
        .filter(|(path, _)| path.starts_with("items/"))
        .map(|(_, oid)| serde_json::from_slice(repo.find_blob(*oid).unwrap().content()).unwrap())
        .filter(|value: &Value| value["meta"]["type"] == "conflict")
        .collect()
}

#[test]
fn attachment_choices_reject_stale_preview_and_keep_both_bytes() {
    use crate::domain::merge::decisions::Selection;
    let pair = Pair::new();
    let note = page(&pair.first, "Illustrated");
    let id = note["id"].as_str().unwrap();
    crate::versioning::test_support::asset(&pair.first.root, id, "cover.png", b"original").unwrap();
    sync(&pair.first);
    sync(&pair.second);
    crate::versioning::test_support::asset(&pair.first.root, id, "cover.png", b"first image")
        .unwrap();
    crate::versioning::test_support::asset(&pair.second.root, id, "cover.png", b"second image")
        .unwrap();
    sync(&pair.first);
    sync(&pair.second);
    let records = decision_items(&pair.second);
    assert_eq!(records.len(), 1);
    let decision = records[0]["id"].as_str().unwrap();
    let stale = pair.second.preview_file(decision).unwrap();
    page(&pair.second, "Newer writing");
    assert!(
        pair.second
            .prepare_file_choice(decision, &stale.expected, Selection::Both, "second")
            .is_err()
    );
    let preview = pair.second.preview_file(decision).unwrap();
    assert!(preview.allowed.contains(&Selection::Both));
    let (prepared, _, _) = pair
        .second
        .prepare_file_choice(decision, &preview.expected, Selection::Both, "second")
        .unwrap();
    pair.second.apply_sync(&prepared).unwrap();
    sync(&pair.second);
    sync(&pair.first);
    let names=pair.first.canonical().unwrap().read(|tx|{
        let mut statement=tx.prepare("SELECT p.path FROM asset_path p JOIN entity e ON e.id=p.owner_id WHERE e.uuid=?1 ORDER BY p.path")?;
        Ok(statement.query_map([id],|row|row.get::<_,String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?)
    }).unwrap();
    let mut bytes: Vec<_> = names
        .iter()
        .map(|path| {
            std::fs::read(
                crate::versioning::test_support::asset_file(
                    &pair.first.root,
                    id,
                    path.rsplit('/').next().unwrap(),
                )
                .unwrap(),
            )
            .unwrap()
        })
        .collect();
    bytes.sort();
    assert_eq!(
        bytes,
        vec![b"first image".to_vec(), b"second image".to_vec()]
    );
    assert!(!decision_items(&pair.first)[0]["content"]["resolution"].is_null());
    pair.close();
}

#[test]
fn rewritten_origin_stops_and_preserves_accepted_history() {
    let pair = Pair::new();
    let initial = pair.first.status().unwrap().head.unwrap();
    let note = page(&pair.first, "Do not lose this");
    sync(&pair.first);
    let head = pair.first.status().unwrap().head.unwrap();
    let remote = Repository::open_bare(&pair.origin).unwrap();
    remote
        .reference(
            "refs/heads/main",
            Oid::from_str(&initial).unwrap(),
            true,
            "Simulated external reset",
        )
        .unwrap();
    drop(remote);
    assert!(
        pair.first
            .prepare_sync()
            .err()
            .unwrap()
            .to_string()
            .contains("rewritten")
    );
    assert_eq!(
        pair.first.status().unwrap().head.as_deref(),
        Some(head.as_str())
    );
    assert_eq!(load(&pair.first, note["id"].as_str().unwrap()), note);
    let repo = crate::versioning::checkpoints::open(&pair.first.root).unwrap();
    assert_eq!(
        repo.refname_to_id(&format!("refs/bluink/recovery/{head}"))
            .unwrap()
            .to_string(),
        head
    );
    drop(repo);
    pair.close();
}

#[test]
fn historical_attachment_is_downloaded_only_when_requested() {
    let pair = Pair::new();
    let note = page(&pair.first, "With history");
    let id = note["id"].as_str().unwrap();
    crate::versioning::test_support::asset(&pair.first.root, id, "cover.png", b"old image")
        .unwrap();
    let old = pair.first.checkpoint().unwrap();
    crate::versioning::test_support::asset(&pair.first.root, id, "cover.png", b"new image")
        .unwrap();
    sync(&pair.first);
    sync(&pair.second);
    let estimate = pair.second.history_download().unwrap();
    assert_eq!(estimate.reachable_bytes, 18);
    assert_eq!(estimate.missing_bytes, 9);
    let file = pair
        .second
        .historical_asset(&old.head.unwrap(), id, "cover.png")
        .unwrap();
    assert_eq!(std::fs::read(file).unwrap(), b"old image");
    assert_eq!(pair.second.history_download().unwrap().missing_bytes, 0);
    pair.close();
}

#[test]
fn rejected_push_retains_the_merge_and_intervening_saves_for_retry() {
    let pair = Pair::new();
    page(&pair.second, "Local draft");
    page(&pair.first, "Remote draft");
    sync(&pair.first);
    let merge = pair.second.prepare_sync().unwrap();
    pair.second.apply_sync(&merge).unwrap();
    let local = page(&pair.second, "Typed after accepting the merge");
    let remote = page(&pair.first, "Remote raced the push");
    sync(&pair.first);
    assert!(
        pair.second
            .push_sync(&merge)
            .unwrap_err()
            .to_string()
            .contains("Push rejected")
    );
    sync(&pair.second);
    sync(&pair.first);
    let repo = crate::versioning::checkpoints::open(&pair.first.root).unwrap();
    assert!(
        repo.graph_descendant_of(repository::head(&repo).unwrap().unwrap(), merge.candidate)
            .unwrap()
    );
    drop(repo);
    for (store, note) in [(&pair.first, local), (&pair.second, remote)] {
        assert_eq!(load(store, note["id"].as_str().unwrap()), note);
    }
    pair.close();
}

#[test]
fn independent_merges_stop_on_multiple_bases_with_both_histories_recoverable() {
    let pair = Pair::new();
    page(&pair.first, "A");
    sync(&pair.first);
    page(&pair.second, "B");
    pair.second.checkpoint().unwrap();
    let second = pair.second.prepare_sync().unwrap();
    let repo = crate::versioning::checkpoints::open(&pair.first.root).unwrap();
    // Deliver the other client's unpublished head to manufacture the two
    // simultaneous successful preparations without depending on wall time.
    repo.remote_anonymous(
        pair.second
            .root
            .join(crate::versioning::checkpoints::DIRECTORY)
            .to_str()
            .unwrap(),
    )
    .unwrap()
    .fetch(&["refs/heads/main:refs/bluink/test/other"], None, None)
    .unwrap();
    let mut first = pair.first.prepare_sync().unwrap();
    let (candidate, _) = pair
        .first
        .merge_heads(&repo, first.previous, second.previous)
        .unwrap();
    first.candidate = candidate;
    drop(repo);
    pair.first.apply_sync(&first).unwrap();
    pair.second.apply_sync(&second).unwrap();
    pair.second.push_sync(&second).unwrap();
    for _ in 0..2 {
        let error = pair.first.prepare_sync().err().unwrap();
        assert!(
            error.to_string().contains("unique common ancestor"),
            "{error}"
        );
    }
    let repo = crate::versioning::checkpoints::open(&pair.first.root).unwrap();
    for tip in [first.candidate, second.candidate] {
        assert_eq!(
            repo.refname_to_id(&format!("refs/bluink/recovery/{tip}"))
                .unwrap(),
            tip
        );
    }
    assert_eq!(repository::head(&repo).unwrap(), Some(first.candidate));
    drop(repo);
    pair.close();
}

#[test]
fn sync_acknowledges_canonical_rows_and_head_before_returning() {
    let pair = Pair::new();
    let note = page(&pair.first, "Recover this sync");
    sync(&pair.first);
    let next = pair.second.prepare_sync().unwrap();
    pair.second.apply_sync(&next).unwrap();
    let repo = crate::versioning::checkpoints::open(&pair.second.root).unwrap();
    assert_eq!(repository::head(&repo).unwrap(), Some(next.candidate));
    let (head, bookkeeping) = pair
        .second
        .canonical()
        .unwrap()
        .read(|tx| {
            // The applied delta is already in the candidate: no capture copies
            // its rows, and acknowledgement leaves no dirty path or operation.
            let rows = tx.query_row(
                "SELECT (SELECT count(*) FROM dirty_path)+(SELECT count(*) FROM capture)
                +(SELECT count(*) FROM git_operation)+(SELECT count(*) FROM revision_pin WHERE owner_kind='git_capture')",
                [],
                |row| row.get::<_, i64>(0),
            )?;
            Ok((crate::store::git::expected_head(tx)?, rows))
        })
        .unwrap();
    assert_eq!(head, Some(next.candidate.to_string()));
    assert_eq!(bookkeeping, 0);
    assert_eq!(load(&pair.second, note["id"].as_str().unwrap()), note);
    // A sync that ends without pushing releases its prepared candidate.
    assert!(repo.find_reference("refs/bluink/pending/sync").is_ok());
    pair.second.finish_sync(None).unwrap();
    assert!(repo.find_reference("refs/bluink/pending/sync").is_err());
    drop(repo);
    pair.close();
}
