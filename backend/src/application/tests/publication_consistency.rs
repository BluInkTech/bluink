// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use crate::structured_query::fixtures::{criteria, field, tag, term};

use super::*;

fn query(app: &Application, document: &crate::structured_query::document::QueryDocument) -> Value {
    execute(app, "query_items", json!({"query":document})).unwrap()
}

#[test]
fn snippets_share_item_publication_and_warm_reconciliation() {
    let project = TestProject::new("unified-snippets");
    let app = project.application();
    app.initialize_sync().unwrap();
    let snippet = execute(
        &app,
        "create_snippet",
        json!({"id":"shared",
        "paragraph":{"type":"paragraph","content":[{"type":"text","text":"Snippet sentinel"}]}}),
    )
    .unwrap();
    assert_eq!(query(&app, &criteria([term("sentinel")]))["count"], 0);
    assert_eq!(
        execute(&app, "query_snippets", json!({"query":"sentinel"})).unwrap()["count"],
        1
    );
    app.stop_sync().unwrap();
    let app = project.application();
    app.initialize_sync().unwrap();
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
    app.initialize_sync().unwrap();
    let created = execute(
        &app,
        "create_item",
        json!({
            "meta":{"type":"page","title":"Original"}, "tags":["draft"],
            "content":{"document": {"type":"doc","content":[{"type":"paragraph","attrs":{"labels":["review"]},
                "content":[{"type":"text","text":"Original paragraph"}]}]}}
        }),
    )
    .unwrap();
    let id = &created["id"];
    let found = query(&app, &criteria([tag("draft")]));
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
            "content":{"document": {"type":"doc","content":[{"type":"paragraph"}]}}
        }),
    )
    .unwrap();
    assert_eq!(query(&app, &criteria([tag("draft")]))["count"], 0);
    let found = query(&app, &criteria([tag("published")]));
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
    assert_eq!(
        query(
            &app,
            &criteria([tag("published"), field("state", "active")])
        )["count"],
        0
    );
    assert_eq!(
        execute(&app, "query_note_tasks", json!({"status":"open"})).unwrap()["count"],
        0
    );
    assert_eq!(
        query(
            &app,
            &criteria([tag("published"), field("state", "archived")])
        )["count"],
        1
    );
    app.stop_sync().unwrap();
    project.close().unwrap();
}
