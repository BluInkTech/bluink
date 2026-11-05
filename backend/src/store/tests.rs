// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use super::{DATABASE_NAME, Error, Store};
use rusqlite::Connection;
use std::sync::Arc;
use uuid::Uuid;

fn generation(store: &Store) -> i64 {
    store
        .read(|connection| {
            Ok(connection
                .query_row("SELECT generation FROM project_state", [], |row| row.get(0))?)
        })
        .unwrap()
}

#[test]
fn callers_share_one_project_writer_queue() {
    let root = tempfile::tempdir().unwrap();
    drop(Store::create(root.path(), Uuid::now_v7()).unwrap());
    let first = Store::shared(root.path()).unwrap();
    let second = Store::shared(root.path()).unwrap();
    assert!(Arc::ptr_eq(&first, &second));
}

#[test]
fn saves_are_durable_and_failed_operations_are_atomic() {
    let root = tempfile::tempdir().unwrap();
    let id = Uuid::now_v7();
    let store = Store::create(root.path(), id).unwrap();
    store
        .write(|connection| {
            connection.execute("UPDATE project_state SET generation=1", [])?;
            Ok(())
        })
        .unwrap();
    let failure = store.write(|connection| {
        connection.execute("UPDATE project_state SET generation=2", [])?;
        Err::<(), _>(Error::Invalid("Rejected fixture".to_owned()))
    });
    assert!(failure.is_err());
    assert_eq!(generation(&store), 1);
    drop(store);
    let reopened = Store::open(root.path()).unwrap();
    assert_eq!(generation(&reopened), 1);
    assert_eq!(
        reopened.path().canonicalize().unwrap(),
        root.path().join(DATABASE_NAME).canonicalize().unwrap()
    );
    assert!(Store::create(root.path(), Uuid::now_v7()).is_err());
}

#[test]
fn readers_keep_a_snapshot_while_another_connection_commits() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::create(root.path(), Uuid::now_v7()).unwrap();
    let other = Store::open(root.path()).unwrap();
    store
        .read(|connection| {
            let read = || {
                connection.query_row::<i64, _, _>(
                    "SELECT generation FROM project_state",
                    [],
                    |row| row.get(0),
                )
            };
            assert_eq!(read()?, 0);
            other.write(|writer| {
                writer.execute("UPDATE project_state SET generation=1", [])?;
                Ok(())
            })?;
            assert_eq!(read()?, 0);
            Ok(())
        })
        .unwrap();
    assert_eq!(generation(&store), 1);
}

#[test]
fn concurrent_writers_serialize_without_losing_updates() {
    let root = tempfile::tempdir().unwrap();
    let store = Arc::new(Store::create(root.path(), Uuid::now_v7()).unwrap());
    let threads: Vec<_> = (0..8)
        .map(|_| {
            let store = Arc::clone(&store);
            std::thread::spawn(move || {
                for _ in 0..10 {
                    store
                        .write(|connection| {
                            connection
                                .execute("UPDATE project_state SET generation=generation+1", [])?;
                            Ok(())
                        })
                        .unwrap();
                }
            })
        })
        .collect();
    for thread in threads {
        thread.join().unwrap();
    }
    assert_eq!(generation(&store), 80);
}

#[test]
fn newer_schema_is_refused_without_mutating_it() {
    let root = tempfile::tempdir().unwrap();
    let connection = Connection::open(root.path().join(DATABASE_NAME)).unwrap();
    connection.pragma_update(None, "user_version", 99).unwrap();
    connection
        .execute_batch("CREATE TABLE preserved(value TEXT); INSERT INTO preserved VALUES('kept')")
        .unwrap();
    assert!(Store::open(root.path()).is_err());
    assert!(Store::create(root.path(), Uuid::now_v7()).is_err());
    let mode: String = connection
        .pragma_query_value(None, "journal_mode", |row| row.get(0))
        .unwrap();
    assert_eq!(mode, "delete");
    assert_eq!(
        connection
            .query_row::<String, _, _>("SELECT value FROM preserved", [], |row| row.get(0))
            .unwrap(),
        "kept"
    );
}

#[test]
fn block_manifests_share_unchanged_content_and_preserve_historical_order() {
    use super::body::{self, PreparedBlock, PreparedBody};
    use serde_json::json;
    use std::collections::BTreeMap;

    let root = tempfile::tempdir().unwrap();
    let store = Store::create(root.path(), Uuid::now_v7()).unwrap();
    let content = json!({"ignoredFindings":[],"document":{"type":"doc","attrs":{"lang":"en"},"content":[
        {"type":"paragraph","attrs":{"id":"a"},"content":[{"type":"text","text":"one"}]},
        {"type":"paragraph","attrs":{"id":"b"},"content":[{"type":"text","text":"two"}]}
    ]}});
    let prepared = PreparedBody::new(&content).unwrap();
    let first = store
        .write(move |transaction| prepared.install(transaction))
        .unwrap();
    let next_block =
        json!({"type":"paragraph","attrs":{"id":"a"},"content":[{"type":"text","text":"changed"}]});
    let expected_block = next_block.clone();
    let changed = BTreeMap::from([("a".to_owned(), PreparedBlock::new(&next_block).unwrap())]);
    let second = store
        .write(move |transaction| {
            body::patch(
                transaction,
                first,
                r#"{"ignoredFindings":[],"document":{"type":"doc"}}"#,
                &["b".to_owned(), "a".to_owned()],
                &changed,
            )
        })
        .unwrap();
    store
        .read(|transaction| {
            assert_eq!(body::read(transaction, first)?, content);
            let updated = body::read(transaction, second)?;
            assert_eq!(
                updated["document"]["content"][0],
                content["document"]["content"][1]
            );
            assert_eq!(updated["document"]["content"][1], expected_block);
            assert!(updated["document"].get("attrs").is_none());
            assert_eq!(
                transaction
                    .query_row::<i64, _, _>("SELECT count(*) FROM block", [], |row| row.get(0))?,
                3
            );
            assert_eq!(
                transaction.query_row::<i64, _, _>(
                    "SELECT max(installed_generation) FROM body",
                    [],
                    |row| row.get(0)
                )?,
                2
            );
            Ok(())
        })
        .unwrap();
    drop(store);
    let store = Store::open(root.path()).unwrap();
    assert_eq!(
        store
            .read(|transaction| body::read(transaction, first))
            .unwrap(),
        content
    );
}

#[test]
fn malformed_manifests_and_missing_blocks_fail_without_partial_results() {
    use super::body;
    assert!(body::decode(b"BIB2").is_err());
    assert!(body::encode(&[0]).is_err());
    let mut packed = body::encode(&[1, 2]).unwrap();
    packed.pop();
    assert!(body::decode(&packed).is_err());
    let valid = body::encode(&(1..=1024).collect::<Vec<_>>()).unwrap();
    assert_eq!(valid.len(), 8204);
    assert_eq!(body::decode(&valid).unwrap().len(), 1024);
    let root = tempfile::tempdir().unwrap();
    let store = Store::create(root.path(), Uuid::now_v7()).unwrap();
    let id = store.write(|transaction| {
        Ok(transaction.query_row("INSERT INTO body(content_json,block_ids,installed_generation) VALUES (?1,?2,1) RETURNING id", rusqlite::params![r#"{"document":{"type":"doc"}}"#,body::encode(&[999])?], |row| row.get(0))?)
    }).unwrap();
    assert!(
        store
            .read(|transaction| body::read(transaction, id))
            .is_err()
    );
}

#[test]
fn change_feed_preserves_atomic_batches_tombstones_and_cursor_recovery() {
    use super::changes::{self, Change};
    use crate::domain::project::types::ItemType;
    use crate::store::changes::{ChangeCursor, ChangeDomain, ChangeOperation};
    use std::collections::BTreeSet;
    let root = tempfile::tempdir().unwrap();
    let store = Store::create(root.path(), Uuid::now_v7()).unwrap();
    let initial = store.read_changes(None).unwrap();
    assert!(!initial.reset);
    store
        .write(|transaction| {
            changes::publish(
                transaction,
                &BTreeSet::from([ChangeDomain::Items]),
                &[
                    Change {
                        id: "a",
                        item_type: ItemType::Page,
                        operation: ChangeOperation::Upsert,
                        revision: Some("r1"),
                    },
                    Change {
                        id: "b",
                        item_type: ItemType::Task,
                        operation: ChangeOperation::Delete,
                        revision: None,
                    },
                ],
            )?;
            Ok(())
        })
        .unwrap();
    let result = store.read_changes(Some(&initial.cursor)).unwrap();
    assert_eq!(result.batches.len(), 1);
    assert_eq!(result.batches[0].items.len(), 2);
    assert!(matches!(
        result.batches[0].items[1].operation,
        ChangeOperation::Delete
    ));
    assert!(!result.has_more);
    let wrong = ChangeCursor {
        epoch: "different".to_owned(),
        generation: "0".to_owned(),
    };
    assert!(store.read_changes(Some(&wrong)).unwrap().reset);
    let future = ChangeCursor {
        epoch: initial.cursor.epoch.clone(),
        generation: "100".to_owned(),
    };
    assert!(store.read_changes(Some(&future)).unwrap().reset);
    store
        .write(|transaction| changes::prune(transaction, i64::MAX))
        .unwrap();
    assert!(store.read_changes(Some(&initial.cursor)).unwrap().reset);
    assert!(!store.read_changes(Some(&result.cursor)).unwrap().reset);
    let malformed = ChangeCursor {
        epoch: initial.cursor.epoch,
        generation: "-1".to_owned(),
    };
    assert!(store.read_changes(Some(&malformed)).is_err());
}

#[test]
fn large_transaction_emits_domain_reset_instead_of_partial_ids() {
    use super::changes::{self, Change};
    use crate::domain::project::types::ItemType;
    use crate::store::changes::{ChangeDomain, ChangeOperation};
    use std::collections::BTreeSet;
    let root = tempfile::tempdir().unwrap();
    let store = Store::create(root.path(), Uuid::now_v7()).unwrap();
    let initial = store.read_changes(None).unwrap();
    store
        .write(|transaction| {
            let ids: Vec<_> = (0..513).map(|n| n.to_string()).collect();
            let changes: Vec<_> = ids
                .iter()
                .map(|id| Change {
                    id,
                    item_type: ItemType::Page,
                    operation: ChangeOperation::Upsert,
                    revision: None,
                })
                .collect();
            changes::publish(
                transaction,
                &BTreeSet::from([ChangeDomain::Items]),
                &changes,
            )?;
            Ok(())
        })
        .unwrap();
    let result = store.read_changes(Some(&initial.cursor)).unwrap();
    assert!(result.batches[0].reset);
    assert!(result.batches[0].items.is_empty());
    assert_eq!(result.cursor.generation, "1");
}

#[test]
fn item_revisions_round_trip_memberships_and_share_bodies_for_metadata_edits() {
    use super::item::{self, ExpectedRevision, PreparedItem};
    use serde_json::json;
    let root = tempfile::tempdir().unwrap();
    let store = Store::create(root.path(), Uuid::now_v7()).unwrap();
    let value = crate::store::commands::assemble_item(&json!({
        "meta":{"type":"page","title":"First"},"tags":["history","writing"],
        "content":{"document":{"type":"doc","content":[{"type":"paragraph","attrs":{"id":"first"},"content":[{"type":"text","text":"Original prose"}]}]}}
    }),None,"Europe/London").unwrap();
    let prepared = PreparedItem::new(&value).unwrap();
    store
        .write(move |transaction| prepared.install(transaction, ExpectedRevision::New))
        .unwrap();
    let uuid = value["id"].as_str().unwrap().to_owned();
    let first_revision = value["revision"].as_str().unwrap().to_owned();
    assert_eq!(
        store
            .read(|transaction| item::read(transaction, &uuid, true))
            .unwrap()
            .unwrap(),
        value
    );
    let uuid_for_write = uuid.clone();
    let expected = first_revision.clone();
    store
        .write(move |transaction| {
            use rusqlite::hooks::{AuthAction, AuthContext, Authorization};
            transaction.flush_prepared_statement_cache();
            transaction.authorizer(Some(|context: AuthContext<'_>| match context.action {
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
            let edited = item::edit_metadata(
                transaction,
                &uuid_for_write,
                &expected,
                json!({"title":"Changed title only"}).as_object().unwrap(),
            );
            transaction.authorizer(None::<fn(AuthContext<'_>) -> Authorization>)?;
            edited?;
            Ok(())
        })
        .unwrap();
    store
        .read(|transaction| {
            assert_eq!(
                item::read_revision(transaction, &uuid, &first_revision)?.unwrap(),
                value
            );
            let current = item::read(transaction, &uuid, false)?.unwrap();
            assert!(current.get("content").is_none());
            assert_eq!(current["meta"]["title"], "Changed title only");
            assert_eq!(current["tags"], value["tags"]);
            assert_eq!(
                transaction
                    .query_row::<i64, _, _>("SELECT count(*) FROM body", [], |row| row.get(0))?,
                1
            );
            let header = item::read_header(transaction, &uuid)?.unwrap();
            assert!(matches!(
                item::check_revision(Some(&header), ExpectedRevision::Exact(&first_revision)),
                Err(Error::RevisionConflict)
            ));
            Ok(())
        })
        .unwrap();
}

#[test]
fn receipts_replay_only_identical_requests_and_pins_require_an_exact_revision() {
    use super::item::{ExpectedRevision, PreparedItem};
    use super::receipts::{self, Submission};
    use serde_json::json;
    let root = tempfile::tempdir().unwrap();
    let store = Store::create(root.path(), Uuid::now_v7()).unwrap();
    let value = crate::store::commands::assemble_item(&json!({
        "meta":{"type":"page","title":"Receipt"},
        "content":{"document":{"type":"doc","content":[{"type":"paragraph","attrs":{"id":"first"}}]}}
    }),None,"Europe/London").unwrap();
    let revision = value["revision"].as_str().unwrap().to_owned();
    let prepared = PreparedItem::new(&value).unwrap();
    let (id, revision) = store
        .write(move |transaction| {
            let id = prepared.install(transaction, ExpectedRevision::New)?;
            let submission = Submission {
                item: id,
                device: "device-a",
                id: "submission-a",
                hash: "hash-a",
                base: &revision,
            };
            assert!(receipts::replay(transaction, &submission)?.is_none());
            receipts::complete(
                transaction,
                &submission,
                &revision,
                &json!({"kind":"ack","version":revision}),
            )?;
            receipts::pin(transaction, "editor", "editor-a", id, &revision, None)?;
            assert!(receipts::pin(transaction, "editor", "editor-a", id, "missing", None).is_err());
            Ok((id, revision))
        })
        .unwrap();
    drop(store);
    let store = Store::open(root.path()).unwrap();
    store
        .read(|transaction| {
            let mut submission = Submission {
                item: id,
                device: "device-a",
                id: "submission-a",
                hash: "hash-a",
                base: &revision,
            };
            assert_eq!(
                receipts::replay(transaction, &submission)?.unwrap(),
                json!({"kind":"ack","version":revision})
            );
            submission.hash = "different-request";
            assert!(receipts::replay(transaction, &submission).is_err());
            submission.device = "device-b";
            assert!(receipts::replay(transaction, &submission)?.is_none());
            Ok(())
        })
        .unwrap();
}

#[test]
fn unchanged_full_writes_reuse_the_current_body_without_a_new_manifest() {
    use super::item::{self, ExpectedRevision, PreparedItem};
    use serde_json::json;
    let root = tempfile::tempdir().unwrap();
    let store = Store::create(root.path(), Uuid::now_v7()).unwrap();
    let first = crate::store::commands::assemble_item(&json!({
        "meta":{"type":"page","title":"First"},
        "content":{"document":{"type":"doc","content":[{"type":"paragraph","attrs":{"id":"a"},"content":[{"type":"text","text":"Kept"}]}]}}
    }),None,"UTC").unwrap();
    let renamed = crate::store::commands::assemble_item(
        &json!({"id":first["id"],"meta":{"type":"page","title":"Renamed"}}),
        Some(&first),
        "UTC",
    )
    .unwrap();
    let edited = crate::store::commands::assemble_item(
        &json!({"id":first["id"],"meta":{"type":"page","title":"Renamed"},"content":{"document":{"type":"doc","content":[{"type":"paragraph","attrs":{"id":"a"},"content":[{"type":"text","text":"Changed"}]}]}}}),
        Some(&renamed),
        "UTC",
    )
    .unwrap();
    let state = |store: &Store| {
        store
            .read(|transaction| {
                Ok(transaction.query_row(
                    "SELECT (SELECT count(*) FROM body),manifest_generation FROM project_state",
                    [],
                    |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
                )?)
            })
            .unwrap()
    };
    let r1 = first["revision"].as_str().unwrap().to_owned();
    let r2 = renamed["revision"].as_str().unwrap().to_owned();
    let prepared = PreparedItem::new(&first).unwrap();
    store
        .write(move |transaction| prepared.install(transaction, ExpectedRevision::New))
        .unwrap();
    let (bodies, generation) = state(&store);
    let prepared = PreparedItem::new(&renamed).unwrap();
    store
        .write(move |transaction| prepared.install(transaction, ExpectedRevision::Exact(&r1)))
        .unwrap();
    assert_eq!(state(&store), (bodies, generation));
    let prepared = PreparedItem::new(&edited).unwrap();
    store
        .write(move |transaction| prepared.install(transaction, ExpectedRevision::Exact(&r2)))
        .unwrap();
    assert_eq!(state(&store), (bodies + 1, generation + 1));
    let id = first["id"].as_str().unwrap();
    let r1 = first["revision"].as_str().unwrap();
    store
        .read(|transaction| {
            assert_eq!(item::read_revision(transaction, id, r1)?.unwrap(), first);
            assert_eq!(item::read(transaction, id, true)?.unwrap(), edited);
            Ok(())
        })
        .unwrap();
}
