// SPDX-License-Identifier: MPL-2.0
use super::{self as git, CapturedSource};
use crate::store::{
    Store,
    item::{self, ExpectedRevision, PreparedItem},
};
use crate::versioning::checkpoints;
use rusqlite::{
    hooks::{AuthAction, AuthContext, Authorization},
    params,
};
use serde_json::{Value, json};
use std::io::{Seek, Write};
use uuid::Uuid;

fn seed(store: &Store) -> Value {
    let value=crate::store::commands::assemble_item(&json!({
        "meta":{"type":"page","title":"Original"},"tags":["history","writing"],
        "content":{"document":{"type":"doc","content":[{"type":"paragraph","attrs":{"id":"first"},"content":[{"type":"text","text":"Original prose"}]}]}}
    }),None,"UTC").unwrap();
    let prepared = PreparedItem::new(&value).unwrap();
    let copy = value.clone();
    store.write(move |tx|{
        prepared.install(tx,ExpectedRevision::New)?;
        tx.execute("UPDATE project_state SET generation=1",[])?;
        git::mark_item_dirty(tx,copy["id"].as_str().unwrap(),copy["revision"].as_str().unwrap(),1)?;
        let project:String=tx.query_row("SELECT project_uuid FROM project_state",[],|row|row.get(0))?;
        git::put_record(tx,"bluink.json",&json!({"format":"bluink-project","formatVersion":20,"id":project,"defaultWorkspaceId":Uuid::now_v7().to_string(),"analytics":{"timezone":"UTC","eventRetentionDays":14}}),&Uuid::now_v7().to_string(),1)?;
        Ok(())
    }).unwrap();
    value
}

#[test]
fn captures_pin_exact_headers_bodies_and_memberships_without_reading_prose() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::create(root.path(), Uuid::now_v7()).unwrap();
    let value = seed(&store);
    let capture = store
        .write(|tx| {
            tx.flush_prepared_statement_cache();
            tx.authorizer(Some(|context: AuthContext<'_>| match context.action {
                AuthAction::Read {
                    table_name: "body",
                    column_name: "content_json",
                    ..
                }
                | AuthAction::Read {
                    table_name: "block",
                    column_name: "document_json",
                    ..
                } => Authorization::Deny,
                _ => Authorization::Allow,
            }))?;
            let result = git::capture(tx, &Uuid::now_v7().to_string(), None, 1);
            tx.authorizer(None::<fn(AuthContext<'_>) -> Authorization>)?;
            result
        })
        .unwrap();
    let uuid = value["id"].as_str().unwrap().to_owned();
    let revision = value["revision"].as_str().unwrap().to_owned();
    let edited_uuid = uuid.clone();
    store
        .write(move |tx| {
            let header = item::edit_metadata(
                tx,
                &edited_uuid,
                &revision,
                json!({"title":"Later save"}).as_object().unwrap(),
            )?;
            tx.execute("UPDATE project_state SET generation=2", [])?;
            git::mark_item_dirty(
                tx,
                &edited_uuid,
                header.value["revision"].as_str().unwrap(),
                2,
            )
        })
        .unwrap();
    store
        .read(|tx| {
            let paths = git::captured_paths(tx, &capture.id)?;
            let item_path = paths
                .iter()
                .find(|path| path.starts_with("items/"))
                .unwrap();
            let CapturedSource::Json(captured) = git::captured_source(tx, &capture.id, item_path)?
            else {
                panic!("expected captured JSON")
            };
            assert_eq!(captured, value);
            assert_ne!(item::read(tx, &uuid, true)?.unwrap(), captured);
            Ok(())
        })
        .unwrap();
    let operation = store
        .write(move |tx| git::prepare_checkpoint(tx, &capture, &"a".repeat(40)))
        .unwrap();
    store
        .write(move |tx| git::acknowledge(tx, &operation, &operation.candidate_head))
        .unwrap();
    store
        .read(|tx| {
            assert_eq!(
                tx.query_row::<i64, _, _>(
                    "SELECT count(*) FROM dirty_path WHERE generation=2",
                    [],
                    |row| row.get(0)
                )?,
                1
            );
            // Acknowledgement prunes all completed bookkeeping and its pins.
            for table in [
                "capture",
                "capture_item",
                "git_operation",
                "revision_pin",
                "object_pin",
            ] {
                assert_eq!(
                    tx.query_row::<i64, _, _>(
                        &format!("SELECT count(*) FROM {table}"),
                        [],
                        |row| { row.get(0) }
                    )?,
                    0,
                    "{table}"
                );
            }
            Ok(())
        })
        .unwrap();
}

#[test]
fn checkpoints_use_bare_git_and_canonical_encoder_without_item_files() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::create(root.path(), Uuid::now_v7()).unwrap();
    let value = seed(&store);
    checkpoints::initialize(root.path()).unwrap();
    let head =
        crate::versioning::test_support::checkpoint(root.path(), &store, "device", "Test device")
            .unwrap()
            .unwrap();
    let repo = checkpoints::open(root.path()).unwrap();
    assert!(repo.is_bare());
    assert!(!root.path().join("items").exists());
    assert!(!repo.path().join("index").exists());
    let path = crate::domain::project::identity::ItemId::try_from(value["id"].as_str().unwrap())
        .unwrap()
        .path(std::path::Path::new(""));
    let entry = repo
        .find_commit(head)
        .unwrap()
        .tree()
        .unwrap()
        .get_path(&path)
        .unwrap();
    assert_eq!(
        repo.find_blob(entry.id()).unwrap().content(),
        crate::domain::project::canonical_json::to_vec(&value).unwrap()
    );
    assert_eq!(
        crate::versioning::test_support::checkpoint(root.path(), &store, "device", "Test device")
            .unwrap(),
        Some(head)
    );
}

#[test]
fn recovery_handles_both_sides_of_ref_publication_and_rejects_outside_tip() {
    for already_published in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let store = Store::create(root.path(), Uuid::now_v7()).unwrap();
        seed(&store);
        checkpoints::initialize(root.path()).unwrap();
        let head =
            crate::versioning::test_support::checkpoint(root.path(), &store, "device", "Test")
                .unwrap()
                .unwrap();
        let repo = checkpoints::open(root.path()).unwrap();
        let old = repo.find_commit(head).unwrap();
        let signature = git2::Signature::now("Fixture", "fixture@example.invalid").unwrap();
        let candidate = repo
            .commit(
                None,
                &signature,
                &signature,
                "Recovered",
                &old.tree().unwrap(),
                &[&old],
            )
            .unwrap();
        let previous = head.to_string();
        let next = candidate.to_string();
        store
            .write(move |tx| {
                let capture = git::capture(tx, &Uuid::now_v7().to_string(), Some(&previous), 1)?;
                git::prepare_checkpoint(tx, &capture, &next)?;
                Ok(())
            })
            .unwrap();
        if already_published {
            repo.reference_matching("refs/heads/main", candidate, true, head, "crash fixture")
                .unwrap();
        }
        checkpoints::recover(&repo, &store).unwrap();
        assert_eq!(repo.refname_to_id("refs/heads/main").unwrap(), candidate);
        assert_eq!(
            store.read(git::expected_head).unwrap(),
            Some(candidate.to_string())
        );
        checkpoints::recover(&repo, &store).unwrap();
        repo.reference_matching("refs/heads/main", head, true, candidate, "outside edit")
            .unwrap();
        assert!(checkpoints::recover(&repo, &store).is_err());
        assert_eq!(repo.refname_to_id("refs/heads/main").unwrap(), head);
    }
}

#[test]
fn recovery_completes_partial_named_refs_after_reopen_and_preserves_newer_saves() {
    use crate::versioning::{model::NamedVersionRequest, test_support};
    use std::collections::BTreeMap;

    let root = tempfile::tempdir().unwrap();
    let store = Store::create(root.path(), Uuid::now_v7()).unwrap();
    let page = seed(&store);
    checkpoints::initialize(root.path()).unwrap();
    let head = test_support::checkpoint(root.path(), &store, "device", "Test")
        .unwrap()
        .unwrap();
    let repo = checkpoints::open(root.path()).unwrap();
    let mut references = BTreeMap::new();
    for title in ["First milestone", "Second milestone"] {
        let request = NamedVersionRequest {
            request_id: Uuid::now_v7().to_string(),
            title: title.to_owned(),
            item_id: None,
        };
        test_support::named_version(root.path(), &store, &request, "device", "Test").unwrap();
        let name = format!("refs/tags/bluink/versions/{}", request.request_id);
        let mut reference = repo.find_reference(&name).unwrap();
        references.insert(name, reference.target().unwrap().to_string());
        reference.delete().unwrap();
    }
    let pending_refs = references.clone();
    let head_text = head.to_string();
    store
        .write(move |tx| {
            git::begin_transition(tx, &head_text)?;
            git::prepare_transition(tx, &head_text, &head_text, &pending_refs)?;
            Ok(())
        })
        .unwrap();
    let (first, oid) = references.first_key_value().unwrap();
    repo.reference(
        first,
        git2::Oid::from_str(oid).unwrap(),
        false,
        "Partial publication",
    )
    .unwrap();

    // A newer acknowledged edit must survive acknowledgement of the older operation.
    let id = page["id"].as_str().unwrap().to_owned();
    let saved_id = id.clone();
    let revision = page["revision"].as_str().unwrap().to_owned();
    store
        .write(move |tx| {
            let updated = item::edit_metadata(
                tx,
                &saved_id,
                &revision,
                json!({"title":"Newer save"}).as_object().unwrap(),
            )?;
            tx.execute("UPDATE project_state SET generation=2", [])?;
            git::mark_item_dirty(
                tx,
                &saved_id,
                updated.value["revision"].as_str().unwrap(),
                2,
            )
        })
        .unwrap();
    drop(store);

    let reopened = Store::open(root.path()).unwrap();
    checkpoints::recover(&repo, &reopened).unwrap();
    checkpoints::recover(&repo, &reopened).unwrap();
    for (name, oid) in references {
        assert_eq!(repo.refname_to_id(&name).unwrap().to_string(), oid);
    }
    reopened
        .read(|tx| {
            assert_eq!(git::expected_head(tx)?, Some(head.to_string()));
            assert!(git::pending_operations(tx)?.is_empty());
            assert!(
                !tx.prepare("SELECT 1 FROM object_pin WHERE owner_kind='git_operation'")?
                    .exists([])?
            );
            assert!(
                tx.prepare("SELECT 1 FROM dirty_path WHERE generation=2")?
                    .exists([])?
            );
            assert_eq!(
                item::read(tx, &id, false)?.unwrap()["meta"]["title"],
                "Newer save"
            );
            Ok(())
        })
        .unwrap();
}

#[test]
fn verified_current_attachments_use_standard_bare_lfs_objects() {
    use crate::store::portable::Source;
    let root = tempfile::tempdir().unwrap();
    let store = Store::create(root.path(), Uuid::now_v7()).unwrap();
    let value = seed(&store);
    checkpoints::initialize(root.path()).unwrap();
    let mut file = tempfile::tempfile().unwrap();
    file.write_all(b"Attachment bytes").unwrap();
    file.rewind().unwrap();
    let asset = checkpoints::install_asset(root.path(), &mut file).unwrap();
    let oid = asset.oid().to_owned();
    let uuid = value["id"].as_str().unwrap().to_owned();
    let identity = crate::domain::project::identity::ItemId::try_from(uuid.as_str()).unwrap();
    let path = identity
        .asset_directory(std::path::Path::new(""))
        .join("image.png")
        .to_string_lossy()
        .replace('\\', "/");
    let saved_path = path.clone();
    store
        .write(move |tx| {
            let owner = tx.query_row("SELECT id FROM entity WHERE uuid=?1", [uuid], |row| {
                row.get(0)
            })?;
            git::put_asset(tx, &saved_path, Some(owner), &asset, 1)
        })
        .unwrap();
    let head = crate::versioning::test_support::checkpoint(root.path(), &store, "device", "Test")
        .unwrap()
        .unwrap();
    let repo = checkpoints::open(root.path()).unwrap();
    let entry = repo
        .find_commit(head)
        .unwrap()
        .tree()
        .unwrap()
        .get_path(std::path::Path::new(&path))
        .unwrap();
    let blob = repo.find_blob(entry.id()).unwrap();
    let pointer = std::str::from_utf8(blob.content()).unwrap();
    assert!(pointer.contains(&format!("oid sha256:{oid}")));
    assert!(pointer.contains("size 16"));
    let object = repo
        .path()
        .join("lfs/objects")
        .join(&oid[..2])
        .join(&oid[2..4])
        .join(&oid);
    assert_eq!(std::fs::read(&object).unwrap(), b"Attachment bytes");
    store
        .read(|tx| {
            assert_eq!(
                tx.query_row::<i64, _, _>(
                    "SELECT local_available FROM asset WHERE oid=?1",
                    params![oid],
                    |row| row.get(0)
                )?,
                1
            );
            Ok(())
        })
        .unwrap();
    let source = checkpoints::portable::GitSource::open(repo.path()).unwrap();
    let mut restored = Vec::new();
    std::io::Read::read_to_end(&mut source.open_asset(&path).unwrap(), &mut restored).unwrap();
    assert_eq!(restored, b"Attachment bytes");
    std::fs::remove_file(object).unwrap();
    let destination = tempfile::tempdir().unwrap();
    assert!(source.open_asset(&path).is_err());
    assert!(source.seed_history(destination.path()).is_err());
    assert!(
        checkpoints::open(destination.path())
            .unwrap()
            .refname_to_id("refs/heads/main")
            .is_err()
    );
}

#[test]
fn naming_an_unchanged_page_adds_indexed_history_without_an_extra_commit() {
    use crate::versioning::model::{HistoryRequest, NamedVersionRequest};
    let root = tempfile::tempdir().unwrap();
    let store = Store::create(root.path(), Uuid::now_v7()).unwrap();
    let value = seed(&store);
    checkpoints::initialize(root.path()).unwrap();
    let first = crate::versioning::test_support::checkpoint(root.path(), &store, "device", "Test")
        .unwrap()
        .unwrap();
    store
        .write(|tx| {
            let source: String = tx.query_row(
                "SELECT canonical_json FROM project_record WHERE path='bluink.json'",
                [],
                |row| row.get(0),
            )?;
            let mut value: Value = serde_json::from_str(&source)?;
            value["appearance"] = json!({"font":"serif"});
            tx.execute("UPDATE project_state SET generation=2", [])?;
            git::put_record(tx, "bluink.json", &value, &Uuid::now_v7().to_string(), 2)
        })
        .unwrap();
    let second = crate::versioning::test_support::checkpoint(root.path(), &store, "device", "Test")
        .unwrap()
        .unwrap();
    assert_ne!(first, second);
    let request = HistoryRequest {
        item_id: Some(value["id"].as_str().unwrap().to_owned()),
        before: None,
        limit: Some(1),
    };
    let before = checkpoints::history::read(root.path(), &store, &request).unwrap();
    assert_eq!(before.entries[0].id, first.to_string());
    let name = NamedVersionRequest {
        request_id: Uuid::now_v7().to_string(),
        title: "Milestone".to_owned(),
        item_id: request.item_id.clone(),
    };
    let named = crate::versioning::test_support::named_version(
        root.path(),
        &store,
        &name,
        "device",
        "Test",
    )
    .unwrap();
    assert_eq!(named.commit_id, second.to_string());
    assert_eq!(
        crate::versioning::test_support::named_version(
            root.path(),
            &store,
            &name,
            "device",
            "Test"
        )
        .unwrap(),
        named
    );
    let after = checkpoints::history::read(root.path(), &store, &request).unwrap();
    assert_eq!(after.entries[0].id, second.to_string());
    assert_eq!(after.entries[0].named_versions.len(), 1);
    assert_eq!(after.cursor, Some(second.to_string()));
    let next = checkpoints::history::read(
        root.path(),
        &store,
        &HistoryRequest {
            before: after.cursor,
            ..request
        },
    )
    .unwrap();
    assert_eq!(next.entries[0].id, first.to_string());
    assert!(next.cursor.is_none());
}

#[test]
fn theme_manifests_and_templates_keep_the_portable_text_policy() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::create(root.path(), Uuid::now_v7()).unwrap();
    seed(&store);
    checkpoints::initialize(root.path()).unwrap();
    let manifest = json!({"name":"Sample", "version":"1", "tokens":{"body":{"size":1}}});
    let expected = crate::domain::project::canonical_json::to_vec(&manifest).unwrap();
    store
        .write(move |tx| {
            git::put_record(
                tx,
                "themes/theme-test/manifest.json",
                &manifest,
                &Uuid::now_v7().to_string(),
                1,
            )
        })
        .unwrap();
    let examples: &[(&str, &[u8])] = &[
        ("style.css", b"p { color: blue; }\n"),
        ("page.minijinja", b"<main>{{ title }}</main>\n"),
        ("picture.png", b"image bytes"),
    ];
    for &(name, bytes) in examples {
        let mut file = tempfile::tempfile().unwrap();
        file.write_all(bytes).unwrap();
        file.rewind().unwrap();
        let asset = checkpoints::install_asset(root.path(), &mut file).unwrap();
        let path = format!("themes/theme-test/{name}");
        store
            .write(move |tx| git::put_asset(tx, &path, None, &asset, 1))
            .unwrap();
    }
    let head = crate::versioning::test_support::checkpoint(root.path(), &store, "device", "Test")
        .unwrap()
        .unwrap();
    let repo = checkpoints::open(root.path()).unwrap();
    let tree = repo.find_commit(head).unwrap().tree().unwrap();
    let entry = tree
        .get_path(std::path::Path::new("themes/theme-test/manifest.json"))
        .unwrap();
    assert_eq!(repo.find_blob(entry.id()).unwrap().content(), expected);
    for &(name, bytes) in examples {
        let entry = tree
            .get_path(std::path::Path::new(&format!("themes/theme-test/{name}")))
            .unwrap();
        let blob = repo.find_blob(entry.id()).unwrap();
        if name == "picture.png" {
            assert!(
                blob.content()
                    .starts_with(b"version https://git-lfs.github.com/spec/v1\n")
            );
        } else {
            assert_eq!(blob.content(), bytes);
        }
    }
}

#[test]
fn git_import_reads_captured_objects_and_copies_history_independently() {
    use crate::store::portable::Source;
    use crate::versioning::model::NamedVersionRequest;
    let root = tempfile::tempdir().unwrap();
    let store = Store::create(root.path(), Uuid::now_v7()).unwrap();
    let value = seed(&store);
    checkpoints::initialize(root.path()).unwrap();
    let named = crate::versioning::test_support::named_version(
        root.path(),
        &store,
        &NamedVersionRequest {
            request_id: Uuid::now_v7().to_string(),
            title: "Imported milestone".to_owned(),
            item_id: Some(value["id"].as_str().unwrap().to_owned()),
        },
        "device",
        "Test",
    )
    .unwrap();
    let destination = tempfile::tempdir().unwrap();
    let source =
        checkpoints::portable::GitSource::open(&root.path().join(checkpoints::DIRECTORY)).unwrap();
    let path = source
        .paths()
        .unwrap()
        .into_iter()
        .find(|path| path.starts_with("items/"))
        .unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&source.read_json(&path).unwrap()).unwrap(),
        value
    );
    assert_eq!(
        source.seed_history(destination.path()).unwrap(),
        Some(named.commit_id.clone())
    );
    drop(source);
    drop(store);
    root.close().unwrap();
    let imported = checkpoints::open(destination.path()).unwrap();
    assert_eq!(
        imported
            .refname_to_id("refs/heads/main")
            .unwrap()
            .to_string(),
        named.commit_id
    );
    let reference = imported
        .find_reference(&format!("refs/tags/bluink/versions/{}", named.id))
        .unwrap();
    assert_eq!(
        reference.peel_to_commit().unwrap().id().to_string(),
        named.commit_id
    );
    let tree = imported
        .find_commit(reference.peel_to_commit().unwrap().id())
        .unwrap()
        .tree()
        .unwrap();
    let entry = tree.get_path(std::path::Path::new(&path)).unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(imported.find_blob(entry.id()).unwrap().content()).unwrap(),
        value
    );
}
