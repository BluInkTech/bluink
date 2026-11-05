// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use super::{PreparedChange, apply, metadata};
use crate::store::{Store, item, query};
use crate::structured_query::fixtures::{criteria, term};
use rusqlite::hooks::{AuthAction, AuthContext, Authorization};
use serde_json::{Value, json};
use uuid::Uuid;

fn page() -> Value {
    crate::store::commands::assemble_item(&json!({"meta":{"type":"page","title":"Original title"},"tags":["research/history"],"content":{"document":{"type":"doc","content":[{"type":"paragraph","attrs":{"id":"first","labels":["quote"]},"content":[{"type":"text","text":"A lasting phrase"}]}]}}}),None,"UTC").unwrap()
}

fn task(owner: &str) -> Value {
    crate::store::commands::assemble_item(&json!({"meta":{"type":"task","title":"Read source"},"content":{"noteId":owner,"text":"Read source"}}),None,"UTC").unwrap()
}

#[test]
fn item_group_projects_relationships_search_activity_and_dirty_paths_atomically() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::create(root.path(), Uuid::now_v7()).unwrap();
    let page = page();
    let id = page["id"].as_str().unwrap().to_owned();
    let task = task(&id);
    let changes = vec![
        PreparedChange::new(page.clone(), None).unwrap(),
        PreparedChange::new(task, None).unwrap(),
    ];
    store
        .write(move |tx| apply(tx, &changes, &[], Some("2026-11-01")))
        .unwrap();
    let baseline = store.read_changes(None).unwrap();
    assert_eq!(baseline.cursor.generation, "1");
    store
        .read(|tx| {
            assert_eq!(item::read(tx, &id, true)?.unwrap(), page);
            assert_eq!(
                tx.query_row::<i64, _, _>("SELECT count(*) FROM dirty_path", [], |row| row.get(0))?,
                2
            );
            assert_eq!(
                tx.query_row::<i64, _, _>(
                    "SELECT sum(words_added) FROM activity_daily",
                    [],
                    |row| row.get(0)
                )?,
                3
            );
            assert_eq!(
                tx.query_row::<i64, _, _>("SELECT count(*) FROM passage_label", [], |row| row
                    .get(0))?,
                1
            );
            let query = query::search(
                tx,
                &criteria([term("lasting")]),
                &query::SearchOptions {
                    content: false,
                    limit: 10,
                    offset: 0,
                    pin_scope: None,
                    prioritize_pins: false,
                    timezone: chrono_tz::UTC,
                },
            )?;
            assert_eq!(query.items.len(), 1);
            assert_eq!(query.items[0].id, id);
            Ok(())
        })
        .unwrap();
}

#[test]
fn invalid_relationship_rolls_back_items_fts_feed_activity_and_revisions() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::create(root.path(), Uuid::now_v7()).unwrap();
    let changes = vec![
        PreparedChange::new(page(), None).unwrap(),
        PreparedChange::new(task(&Uuid::now_v7().to_string()), None).unwrap(),
    ];
    assert!(
        store
            .write(move |tx| apply(tx, &changes, &[], Some("2026-11-01")))
            .is_err()
    );
    store
        .read(|tx| {
            for table in [
                "item",
                "body",
                "block",
                "entity",
                "item_fts",
                "dirty_path",
                "activity_event",
                "activity_daily",
                "change_batch",
            ] {
                assert_eq!(
                    tx.query_row::<i64, _, _>(
                        &format!("SELECT count(*) FROM {table}"),
                        [],
                        |row| row.get(0)
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
fn title_and_archive_cascade_never_read_document_payloads() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::create(root.path(), Uuid::now_v7()).unwrap();
    let page = page();
    let id = page["id"].as_str().unwrap().to_owned();
    let revision = page["revision"].as_str().unwrap().to_owned();
    let task = task(&id);
    let task_id = task["id"].as_str().unwrap().to_owned();
    let changes = vec![
        PreparedChange::new(page, None).unwrap(),
        PreparedChange::new(task, None).unwrap(),
    ];
    store
        .write(move |tx| apply(tx, &changes, &[], None))
        .unwrap();
    let baseline = store.read_changes(None).unwrap();
    let writer_id = id.clone();
    store
        .write(move |tx| {
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
            let result = metadata(
                tx,
                &writer_id,
                &revision,
                json!({"title":"Renamed","state":"archived"})
                    .as_object()
                    .unwrap(),
            );
            tx.authorizer(None::<fn(AuthContext<'_>) -> Authorization>)?;
            result.map(|_| ())
        })
        .unwrap();
    let result = store.read_changes(Some(&baseline.cursor)).unwrap();
    assert_eq!(result.batches.len(), 1);
    assert_eq!(result.batches[0].items.len(), 2);
    store
        .read(|tx| {
            assert_eq!(
                item::read(tx, &task_id, false)?.unwrap()["meta"]["state"],
                "archived"
            );
            assert_eq!(
                item::read(tx, &id, false)?.unwrap()["meta"]["title"],
                "Renamed"
            );
            assert_eq!(
                tx.query_row::<i64, _, _>("SELECT count(*) FROM body", [], |row| row.get(0))?,
                2
            );
            assert_eq!(
                tx.query_row::<i64, _, _>("SELECT count(*) FROM revision", [], |row| row.get(0))?,
                2
            );
            Ok(())
        })
        .unwrap();
}

#[test]
fn unchanged_metadata_and_memberships_keep_their_revision_and_generation() {
    use crate::store::commands::{self, MembershipChange};
    let root = tempfile::tempdir().unwrap();
    let store = Store::create(root.path(), Uuid::now_v7()).unwrap();
    let page = page();
    let id = page["id"].as_str().unwrap().to_owned();
    let revision = page["revision"].as_str().unwrap().to_owned();
    let changes = vec![PreparedChange::new(page, None).unwrap()];
    store
        .write(move |tx| apply(tx, &changes, &[], Some("2026-11-01")))
        .unwrap();
    let generation = || store.read_changes(None).unwrap().cursor.generation;
    let before = generation();
    let patch = serde_json::Map::from_iter([("title".to_owned(), json!("Original title"))]);
    let same = commands::metadata(&store, &id, Some(&revision), patch, false).unwrap();
    assert_eq!(same["revision"], revision);
    let tags = MembershipChange::Tags(vec!["research/history".to_owned()]);
    let same = commands::memberships(&store, &id, &revision, tags, false).unwrap();
    assert_eq!(same["revision"], revision);
    assert_eq!(generation(), before);
    let patch = serde_json::Map::from_iter([("title".to_owned(), json!("Renamed"))]);
    let renamed = commands::metadata(&store, &id, Some(&revision), patch, false).unwrap();
    assert_ne!(renamed["revision"], revision);
    assert_ne!(generation(), before);
}
