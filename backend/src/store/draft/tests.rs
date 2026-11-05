// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use super::*;
use crate::store::{Store, item, mutations, query};
use crate::structured_query::fixtures::{criteria, term};
use uuid::Uuid;

mod performance;

/// Read the base in its own snapshot, then prepare outside it.
fn prepare(
    store: &Store,
    request: &MergeDraftRequest,
    timezone: &str,
    hash: &str,
) -> Result<Option<Prepared>> {
    let base = store.read(|tx| read_base(tx, request))?;
    super::prepare(base, request, timezone, hash)
}

fn start(nodes: Vec<Value>) -> (tempfile::TempDir, Store, Value) {
    let root = tempfile::tempdir().unwrap();
    let store = Store::create(root.path(), Uuid::now_v7()).unwrap();
    let page=crate::store::commands::assemble_item(&json!({"meta":{"type":"page","title":"A page"},"content":{"document":{"type":"doc","content":Value::Array(nodes)}}}),None,"UTC").unwrap();
    let change = mutations::PreparedChange::new(page.clone(), None).unwrap();
    store
        .write(move |tx| mutations::apply(tx, &[change], &[], None))
        .unwrap();
    (root, store, page)
}

fn patch(page: &Value, order: &[String], changed: &BTreeMap<String, Value>) -> MergeDraftRequest {
    serde_json::from_value(json!({"id":page["id"],"baseRevision":page["revision"],"device":"test","submissionId":Uuid::now_v7().to_string(),"draft":{"kind":"documentPatch","meta":page["meta"],"content":{},"document":{"order":order,"blocks":changed}}})).unwrap()
}

fn apply(store: &Store, request: &MergeDraftRequest) -> Value {
    let hash = crate::domain::project::submissions::hash(request).unwrap();
    let prepared = prepare(store, request, "UTC", &hash).unwrap().unwrap();
    store
        .write(move |tx| prepared.apply(tx, "2026-11-01"))
        .unwrap()
        .unwrap()
}

#[test]
fn sparse_patch_reorders_passages_matches_full_projection_and_replays_receipt() {
    let (_root, store, page) = start(vec![
        json!({"type":"heading","attrs":{"level":1},"content":[{"type":"text","text":"Heading"}]}),
        json!({"type":"paragraph","attrs":{"labels":["quote"]},"content":[{"type":"text","text":"Old phrase"}]}),
        json!({"type":"paragraph","attrs":{"labels":["quote"]},"content":[{"type":"text","text":"Untouched context"}]}),
    ]);
    let nodes = page["content"]["document"]["content"].as_array().unwrap();
    let order = vec![
        nodes[2]["attrs"]["id"].as_str().unwrap().to_owned(),
        nodes[1]["attrs"]["id"].as_str().unwrap().to_owned(),
        nodes[0]["attrs"]["id"].as_str().unwrap().to_owned(),
    ];
    let mut replacement = nodes[1].clone();
    replacement["content"][0]["text"] = json!("New searchable words");
    let request = patch(
        &page,
        &order,
        &BTreeMap::from([(order[1].clone(), replacement)]),
    );
    let hash = crate::domain::project::submissions::hash(&request).unwrap();
    let replay = prepare(&store, &request, "UTC", &hash).unwrap().unwrap();
    let mut stale = request.clone();
    stale.submission_id = Uuid::now_v7().to_string();
    let loser = prepare(&store, &stale, "UTC", "different")
        .unwrap()
        .unwrap();
    let ack = apply(&store, &request);
    assert_eq!(
        store
            .write(move |tx| replay.apply(tx, "2026-11-01"))
            .unwrap(),
        Some(ack.clone())
    );
    assert!(
        store
            .write(move |tx| loser.apply(tx, "2026-11-01"))
            .unwrap()
            .is_none()
    );
    let next = store
        .read(|tx| item::read(tx, &request.id, true))
        .unwrap()
        .unwrap();
    assert_eq!(next["revision"], ack["version"]);
    let expanded = request.draft.expand(&page).unwrap();
    let expected = crate::store::commands::assemble_item(&expanded, Some(&page), "UTC").unwrap();
    assert_eq!(next["content"], expected["content"]);
    assert_eq!(next["analytics"], expected["analytics"]);
    store
        .read(|tx| {
            let ordered = tx
                .prepare("SELECT block_id FROM passage ORDER BY position")?
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            assert_eq!(ordered, order[..2]);
            let facts: String =
                tx.query_row("SELECT export_facts_json FROM item_projection", [], |row| {
                    row.get(0)
                })?;
            assert_eq!(
                serde_json::from_str::<Value>(&facts)?,
                json!(crate::domain::publication::preflight::document_facts(
                    &next["content"]["document"]
                ))
            );
            assert_eq!(
                query::matching_ids(
                    tx,
                    &criteria([term("searchable")]),
                    None,
                    None,
                    chrono_tz::UTC
                )?,
                std::slice::from_ref(&request.id)
            );
            assert_eq!(
                item::read_revision(tx, &request.id, &request.base_revision)?.unwrap(),
                page
            );
            Ok(())
        })
        .unwrap();
    let noop = patch(&next, &order, &BTreeMap::new());
    assert_eq!(apply(&store, &noop)["version"], next["revision"]);
    assert_eq!(store.read_changes(None).unwrap().cursor.generation, "2");
}

#[test]
fn copied_nested_ids_request_a_reconciled_document() {
    let (_root, store, page) = start(vec![
        json!({"type":"paragraph","content":[{"type":"text","text":"One"}]}),
        json!({"type":"paragraph","content":[{"type":"text","text":"Two"}]}),
    ]);
    let nodes = page["content"]["document"]["content"].as_array().unwrap();
    let keys = nodes
        .iter()
        .map(|node| node["attrs"]["id"].as_str().unwrap().to_owned())
        .collect::<Vec<_>>();
    let boxed = json!({"type":"box","attrs":{"id":keys[1],"kind":"quote"},"content":[nodes[0]]});
    let request = patch(&page, &keys, &BTreeMap::from([(keys[1].clone(), boxed)]));
    assert!(prepare(&store, &request, "UTC", "test").unwrap().is_none());
}

#[test]
fn header_only_draft_preserves_manifest_and_body_postings() {
    use rusqlite::hooks::{AuthAction, AuthContext, Authorization};
    fn protect(context: AuthContext<'_>) -> Authorization {
        match context.action {
            AuthAction::Read {
                table_name: "block",
                column_name: "document_json",
                ..
            }
            | AuthAction::Read {
                table_name: "block_fact",
                ..
            }
            | AuthAction::Insert {
                table_name: "body" | "block" | "block_fact",
                ..
            } => Authorization::Deny,
            _ => Authorization::Allow,
        }
    }
    let (_root, store, page) = start(vec![
        json!({"type":"paragraph","content":[{"type":"text","text":"Still searchable"}]}),
    ]);
    let key = page["content"]["document"]["content"][0]["attrs"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let mut request = patch(&page, &[key], &BTreeMap::new());
    let EditorDraft::DocumentPatch { meta, .. } = request.draft.as_mut() else {
        unreachable!()
    };
    meta.title = Some("Renamed".into());
    store
        .read(|tx| {
            tx.authorizer(Some(protect))?;
            Ok(())
        })
        .unwrap();
    let prepared = prepare(&store, &request, "UTC", "header").unwrap().unwrap();
    store
        .read(|tx| {
            tx.authorizer(None::<fn(AuthContext<'_>) -> Authorization>)?;
            Ok(())
        })
        .unwrap();
    store
        .write(move |tx| {
            tx.authorizer(Some(protect))?;
            let result = prepared.apply(tx, "2026-11-01");
            tx.authorizer(None::<fn(AuthContext<'_>) -> Authorization>)?;
            result
        })
        .unwrap()
        .unwrap();
    store
        .read(|tx| {
            assert_eq!(
                tx.query_row::<i64, _, _>("SELECT count(*) FROM body", [], |row| row.get(0))?,
                1
            );
            assert_eq!(
                query::matching_ids(
                    tx,
                    &criteria([term("renamed"), term("searchable")]),
                    None,
                    None,
                    chrono_tz::UTC
                )?,
                [request.id]
            );
            Ok(())
        })
        .unwrap();
}

#[test]
fn writer_rechecks_only_snippet_references_the_patch_adds() {
    let (_root, store, page) = start(vec![
        json!({"type":"paragraph","content":[{"type":"text","text":"Body"}]}),
    ]);
    let first = page["content"]["document"]["content"][0]["attrs"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let embed =
        json!({"type":"embed","attrs":{"id":Uuid::now_v7().to_string(),"snippetId":"reference"}});
    let added = embed["attrs"]["id"].as_str().unwrap().to_owned();
    let order = [first, added.clone()];
    let request = patch(&page, &order, &BTreeMap::from([(added, embed)]));
    let hash = crate::domain::project::submissions::hash(&request).unwrap();
    let missing = prepare(&store, &request, "UTC", &hash).unwrap().unwrap();
    assert!(
        store
            .write(move |tx| missing.apply(tx, "2026-11-01"))
            .unwrap()
            .is_none()
    );
    let snippet = crate::store::commands::assemble_item(&json!({"id":crate::domain::project::named_items::id("snippet","reference"),"meta":{"type":"snippet"},"content":{"key":"reference","paragraph":{"type":"paragraph","content":[{"type":"text","text":"Shared"}]}}}),None,"UTC").unwrap();
    let change = mutations::PreparedChange::new(snippet, None).unwrap();
    store
        .write(move |tx| mutations::apply(tx, &[change], &[], None))
        .unwrap();
    let ack = apply(&store, &request);
    let next = store
        .read(|tx| item::read(tx, &request.id, true))
        .unwrap()
        .unwrap();
    assert_eq!(next["revision"], ack["version"]);
    // The retained reference protects its snippet, so later saves skip it.
    let snippet_id = crate::domain::project::named_items::id("snippet", "reference");
    let archived = store.write(move |tx| {
        let header = item::read_header(tx, &snippet_id)?.unwrap();
        let revision = header.value["revision"].as_str().unwrap().to_owned();
        mutations::metadata(
            tx,
            &snippet_id,
            &revision,
            json!({"state":"archived"}).as_object().unwrap(),
        )
        .map(|_| ())
    });
    assert!(matches!(archived, Err(Error::Invalid(_))));
    let unchanged = patch(&next, &order, &BTreeMap::new());
    assert_eq!(apply(&store, &unchanged)["version"], next["revision"]);
}
