// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use super::*;
use crate::indexing::service;
use crate::query::document::test_query;

fn query(app: &Application, text: &str) -> Value {
    execute(app, "query_items", json!({"query":test_query(text)})).unwrap()
}

#[test]
fn snippets_share_item_publication_and_warm_reconciliation() {
    let project = TestProject::new("unified-snippets");
    let app = project.application();
    app.initialize_sync(true).unwrap();
    let index = app.index_path().unwrap();
    let before = service::query_snapshot(&index).unwrap();
    let before_count = before.reader.searcher().num_docs();
    let items_before = crate::indexing::index::item_count(&before.reader).unwrap();
    let snippet = execute(
        &app,
        "create_snippet",
        json!({"id":"shared",
        "paragraph":{"type":"paragraph","content":[{"type":"text","text":"Snippet sentinel"}]}}),
    )
    .unwrap();
    let after = service::query_snapshot(&index).unwrap();
    assert_eq!(after.reader.searcher().num_docs(), before_count + 1);
    assert_eq!(before.reader.searcher().num_docs(), before_count);
    assert_eq!(
        crate::indexing::index::item_count(&after.reader).unwrap(),
        items_before + 1
    );
    assert_eq!(query(&app, "sentinel")["count"], 0);
    assert_eq!(
        execute(&app, "query_snippets", json!({"query":"sentinel"})).unwrap()["count"],
        1
    );
    drop((before, after));
    app.stop_sync().unwrap();
    let app = project.application();
    app.initialize_sync(true).unwrap();
    assert_eq!(
        service::index_service_status_typed(&index).unwrap().changed,
        0
    );
    assert_eq!(
        execute(&app, "query_snippets", json!({"query":"sentinel"})).unwrap()["count"],
        1
    );
    execute(
        &app,
        "archive_snippet",
        json!({"id":"shared","revision":snippet["revision"]}),
    )
    .unwrap();
    assert_eq!(
        execute(&app, "query_snippets", json!({})).unwrap()["count"],
        0
    );
    app.stop_sync().unwrap();
    project.close().unwrap();
}

#[test]
fn successful_writes_are_immediately_visible_across_projections_without_polling() {
    let project = TestProject::new("publication-consistency");
    let app = project.application();
    app.initialize_sync(true).unwrap();
    let created = execute(
        &app,
        "create_item",
        json!({
            "meta":{"type":"page","title":"Original"}, "tags":["draft"],
            "content":{"type":"doc","content":[{"type":"paragraph","attrs":{"labels":["review"]},
                "content":[{"type":"text","text":"Original paragraph"}]}]}
        }),
    )
    .unwrap();
    let id = &created["id"];
    let found = query(&app, "#draft");
    assert_eq!(found["count"], 1);
    assert_eq!(found["items"][0]["version"], created["version"]);
    assert_eq!(
        execute(&app, "query_label_passages", json!({"label":"review"})).unwrap()["count"],
        1
    );
    let task = execute(
        &app,
        "mutate_note_task",
        json!({"noteId":id,"version":created["version"],
        "change":{"kind":"create","text":"Review the note"}}),
    )
    .unwrap();
    let tasks = execute(&app, "query_note_tasks", json!({"status":"open"})).unwrap();
    assert_eq!(tasks["count"], 1);
    assert_eq!(tasks["items"][0]["revision"], task["version"]);
    let updated = execute(
        &app,
        "update_item",
        json!({"id":id,"version":created["version"],
            "meta":{"title":"Published"}, "tags":["published"],
            "content":{"type":"doc","content":[{"type":"paragraph"}]}
        }),
    )
    .unwrap();
    assert_eq!(query(&app, "#draft")["count"], 0);
    let found = query(&app, "#published");
    assert_eq!(found["count"], found["items"].as_array().unwrap().len());
    assert_eq!(found["items"][0]["version"], updated["version"]);
    assert_eq!(
        execute(&app, "query_note_tasks", json!({"status":"open"})).unwrap()["count"],
        1
    );
    assert_eq!(
        execute(&app, "query_label_passages", json!({"label":"review"})).unwrap()["count"],
        0
    );
    let facets = execute(&app, "list_facets", json!({"kind":"tag"})).unwrap();
    assert!(
        facets
            .as_array()
            .unwrap()
            .iter()
            .any(|facet| facet["key"] == "published" && facet["usageCount"] == 1)
    );
    execute(&app, "delete_item", json!({"id":id})).unwrap();
    assert_eq!(query(&app, "#published +state:active")["count"], 0);
    assert_eq!(
        execute(&app, "query_note_tasks", json!({"status":"open"})).unwrap()["count"],
        0
    );
    assert_eq!(query(&app, "#published +state:archived")["count"], 1);
    app.stop_sync().unwrap();
    project.close().unwrap();
}

#[test]
fn pending_edits_do_not_remove_rows_from_a_published_snapshot() {
    let project = TestProject::new("consistent-snapshot");
    let app = project.application();
    app.initialize_sync(true).unwrap();
    let created = execute(
        &app,
        "create_item",
        json!({"meta":{"type":"page","title":"Before"},
        "tags":["draft"],"content":{"type":"doc","content":[{"type":"paragraph"}]}}),
    )
    .unwrap();
    let index = app.index_path().unwrap();
    app.state()
        .unwrap()
        .index
        .as_mut()
        .unwrap()
        .events
        .stop()
        .unwrap();
    let (before, cache) = service::cached_index(&index).unwrap();
    // Stop the watcher to control publication explicitly. The write marks its
    // revision pending, but only committed() below queues index publication.
    let pending = crate::engine::project::write::mutate_existing_item(
        &app.root,
        created["id"].as_str().unwrap(),
        Some(created["version"].as_str().unwrap()),
        &app.activity_date().unwrap(),
        |_| Ok(json!({"id":created["id"],"meta":{"title":"After"},"tags":["published"]})),
    )
    .unwrap();
    let during = query(&app, "#draft");
    assert_eq!(during["count"], 1);
    assert_eq!(during["items"].as_array().unwrap().len(), 1);
    assert_eq!(during["items"][0]["meta"]["title"], "Before");
    let content_error = execute(
        &app,
        "query_items",
        json!({
            "query":test_query("#draft"), "options":{"content":true}
        }),
    )
    .unwrap_err();
    assert!(
        content_error
            .to_string()
            .contains("changed while reading query content")
    );
    mutations::committed(&app.root, Ok(pending)).unwrap();
    assert_eq!(query(&app, "#draft")["count"], 0);
    assert_eq!(
        query(&app, "#published")["items"][0]["meta"]["title"],
        "After"
    );
    let old =
        crate::search::search_snapshot(&before, &cache, &test_query("#draft"), 10, 0, None, false)
            .unwrap();
    assert_eq!(old.count, 1);
    assert_eq!(old.hits.len(), 1);
    drop((before, cache));
    app.stop_sync().unwrap();
    project.close().unwrap();
}

#[test]
fn failed_publication_preserves_the_saved_file_and_rejects_queries() {
    let project = TestProject::new("publication-failure");
    let app = project.application();
    app.initialize_sync(true).unwrap();
    app.state()
        .unwrap()
        .index
        .as_mut()
        .unwrap()
        .events
        .stop()
        .unwrap();
    let index = app.index_path().unwrap();
    let created = execute(
        &app,
        "create_item",
        json!({"meta":{"type":"page","title":"Valid"},"content":{"type":"doc","content":[{"type":"paragraph"}]}}),
    )
    .unwrap();
    let id = created["id"].as_str().unwrap();
    let path = crate::engine::project::identity::ItemId::try_from(id)
        .unwrap()
        .path(&app.root);
    let original = std::fs::read(&path).unwrap();
    std::fs::write(&path, b"{invalid JSON").unwrap();
    service::enqueue_external_index_item(index.clone(), id.to_owned()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while service::index_service_status_typed(&index)
        .unwrap()
        .error
        .is_none()
    {
        assert!(
            Instant::now() < deadline,
            "Worker did not report the corrupt item"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(service::cached_index(&index).is_err());
    std::fs::write(&path, original).unwrap();
    let error = execute(&app, "update_item_meta", json!({"id":id,"version":created["version"],"meta":{"title":"Saved despite indexing failure"}})).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("were saved, but search publication failed"),
        "{error}"
    );
    let saved = execute(&app, "read_item", json!({"id":id})).unwrap();
    assert_eq!(saved["meta"]["title"], "Saved despite indexing failure");
    app.stop_sync().unwrap();
    // Reopening repairs the rebuildable projection from canonical files.
    let reopened = project.application();
    reopened.initialize_sync(true).unwrap();
    assert_eq!(
        query(&reopened, "+type:page")["items"][0]["version"],
        saved["version"]
    );
    reopened.stop_sync().unwrap();
    project.close().unwrap();
}
