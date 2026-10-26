// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use super::support::*;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::{Value, json};

#[test]
fn mutations_preserve_canonical_revisions_omitted_fields_and_invalidate_indexed_results() {
    let f = Fixture::new(true);
    let created = f.call("create_item", json!({"meta":{"type":"page","title":"Original"},"tags":["original"],"content":document("Visible preview text")}));
    let id = &created["id"];
    f.call(
        "query_items",
        json!({"query":crate::query::document::test_query("#original")}),
    );
    let updated = f.call("update_item", json!({"id":id,"version":created["version"],"meta":{"title":"Changed"},"content":created["content"]}));
    assert_eq!(updated["tags"], json!(["original"]));
    assert!(
        f.error(
            "update_item",
            json!({"id":id,"version":created["version"],"meta":{},"content":created["content"]}),
            StatusCode::BAD_REQUEST
        )
        .contains("changed since it was read")
    );
    let tagged = f.call(
        "set_item_tags",
        json!({"id":id,"version":updated["version"],"tags":["changed"]}),
    );
    assert_eq!(tagged["content"], created["content"]);
    let metadata = f.call(
        "update_item_meta",
        json!({"id":id,"version":tagged["version"],"meta":{"title":"Metadata"}}),
    );
    assert_eq!(metadata["content"], json!({}));
    assert_eq!(metadata["tags"], json!(["changed"]));
    let task = f.call("mutate_note_task", json!({"noteId":id,"version":metadata["version"],"change":{"kind":"create","text":"Review this"}}));
    assert_eq!(task["content"]["text"], "Review this");
    let pin = f.call(
        "set_note_pin",
        json!({"noteId":id,"version":metadata["version"],"pinned":true,"scopeId":f.call("read_workspace_context",json!({}))["defaultWorkspace"]["id"]}),
    );
    assert_eq!(pin["pinned"], true);
    let read = f.call("read_item", json!({"id":id}));
    assert_eq!(read["version"], pin["version"]);
    assert_eq!(read["meta"]["title"], "Metadata");
    assert_eq!(read["content"], created["content"]);
    f.settled();
    assert_eq!(
        f.call(
            "query_items",
            json!({"query":crate::query::document::test_query("#changed")})
        )["items"][0]["id"],
        *id
    );
    assert_eq!(
        f.call(
            "query_items",
            json!({"query":crate::query::document::test_query("#original")})
        )["count"],
        0
    );
    for body in [
        json!({"noteId":id,"version":read["version"],"pinned":"yes"}),
        json!({"noteId":id,"version":123,"pinned":true}),
    ] {
        f.error("set_note_pin", body, StatusCode::BAD_REQUEST);
    }
    assert_eq!(
        f.call("read_item", json!({"id":id}))["version"],
        read["version"]
    );
    assert_eq!(
        f.call("delete_item", json!({"id":id})),
        json!({"deleted":true})
    );
    assert_eq!(
        f.call("read_item", json!({"id":id}))["meta"]["state"],
        "archived"
    );
}

#[test]
fn public_reads_queries_and_batch_hydration_share_projection_and_validation() {
    let f = Fixture::new(false);
    let note = f.call("create_item", json!({"meta":{"type":"page","title":"Alpha"},"tags":["research"],"content":document("Visible preview text")}));
    let id = &note["id"];
    f.initialize_index();
    let full = f.call("read_item", json!({"id":id}));
    assert_eq!(full["version"], note["version"]);
    assert_eq!(full["content"], note["content"]);
    assert_eq!(full["tags"], json!(["research"]));
    assert!(full.get("analytics").is_none());
    assert_eq!(
        f.call("read_item", json!({"id":id,"options":{"content":false}}))["content"],
        json!({})
    );
    let query = f.call(
        "query_items",
        json!({"query":crate::query::document::test_query("+type:page sort:title-asc"),"limit":10}),
    );
    assert_eq!(query["count"], 1);
    assert_eq!(query["items"][0]["id"], *id);
    assert!(
        query["items"][0]["preview"]
            .as_str()
            .unwrap()
            .contains("Visible preview text")
    );
    assert_eq!(
        f.call(
            "query_items",
            json!({"query":crate::query::document::test_query("+type:page sort:title-asc"),"options":{"content":true}})
        )["items"][0]["content"],
        note["content"]
    );
    let batch = f.call(
        "read_items",
        json!({"ids":[uuid::Uuid::new_v4().to_string(),id]}),
    );
    assert_eq!(batch.as_array().unwrap().len(), 1);
    assert_eq!(batch[0]["id"], *id);
    assert_eq!(batch[0]["content"], json!({}));
    let all = f.call("read_items", json!({}));
    assert_eq!(
        all.as_array().unwrap().len(),
        crate::engine::project::item::list_project_item_ids(f.root.path())
            .unwrap()
            .len()
    );
    assert!(all.as_array().unwrap().iter().any(|item| item["id"] == *id));
    for body in [
        json!({"ids":[id],"query":crate::query::document::test_query("")}),
        json!({"ids":vec![id;513]}),
    ] {
        assert_eq!(
            f.error("read_items", body, StatusCode::BAD_REQUEST),
            "Read up to 512 item IDs, without a query."
        );
    }
    for (name, body) in [
        (
            "query_items",
            json!({"query":crate::query::document::test_query("+unregistered:value")}),
        ),
        (
            "query_items",
            json!({"query":crate::query::document::test_query(""),"limit":0}),
        ),
        ("read_item", json!({"id":uuid::Uuid::new_v4().to_string()})),
    ] {
        f.error(name, body, StatusCode::BAD_REQUEST);
    }
}

#[test]
fn task_queries_keep_collection_membership_page_defaults_scope_and_paging() {
    let f = Fixture::new(true);
    let mut notes = Vec::new();
    for tag in ["writing", "research"] {
        let note = f.call(
            "create_item",
            json!({"meta":{"type":"page","title":tag},"tags":[tag],"content":document("")}),
        );
        f.call(
            "mutate_note_task",
            json!({"noteId":note["id"],"change":{"kind":"create","text":format!("Check {tag}")}}),
        );
        notes.push(note);
    }
    f.settled();
    assert_eq!(
        f.call(
            "query_note_tasks",
            json!({"query":crate::query::document::test_query("+type:page +state:active sort:modified-desc"),"status":"open"})
        )["count"],
        2
    );
    let selected = f.call(
        "query_note_tasks",
        json!({"query":crate::query::document::test_query("+type:page +state:active #writing sort:modified-desc"),"status":"open"}),
    );
    assert_eq!(selected["count"], 1);
    assert_eq!(selected["items"][0]["noteId"], notes[0]["id"]);
    let scoped = f.call(
        "query_note_tasks",
        json!({"query":crate::query::document::test_query("unregistered:value"),"noteId":notes[1]["id"],"status":"open"}),
    );
    assert_eq!(scoped["count"], 1);
    assert_eq!(scoped["items"][0]["noteId"], notes[1]["id"]);
    let page = f.call(
        "query_note_tasks",
        json!({"status":"open","limit":1,"offset":1}),
    );
    assert_eq!(page["count"], 2);
    assert_eq!(page["items"].as_array().unwrap().len(), 1);
    assert_eq!(
        f.call("query_note_tasks", json!({"status":"completed"}))["count"],
        0
    );
    for body in [
        json!({"status":"invalid"}),
        json!({"status":"open","limit":0}),
        json!({"status":"open","limit":201}),
        json!({"status":"open","offset":-1}),
        json!({"status":"open","limit":1.5}),
    ] {
        f.error("query_note_tasks", body, StatusCode::BAD_REQUEST);
    }
}

fn pending_completions(f: &Fixture) -> usize {
    std::fs::read_dir(f.root.path().join("analytics/events"))
        .unwrap()
        .flat_map(|entry| {
            std::fs::read_to_string(entry.unwrap().path())
                .unwrap()
                .lines()
                .map(|line| serde_json::from_str::<Value>(line).unwrap())
                .collect::<Vec<_>>()
        })
        .filter(|entry| entry["state"] == "pending" && entry["event"]["kind"] == "task_completed")
        .count()
}

#[test]
fn page_tasks_read_acknowledged_changes_from_shared_index() {
    let f = Fixture::new(true);
    let note = f.call(
        "create_item",
        json!({"meta":{"type":"page","title":"Task consistency"},"content":document("Draft")}),
    );
    let created = f.call("mutate_note_task", json!({"noteId":note["id"],"version":note["version"],"change":{"kind":"create","text":"Read immediately"}}));
    let open = f.call(
        "query_note_tasks",
        json!({"noteId":note["id"],"status":"open"}),
    );
    assert_eq!(open["count"], 1);
    assert_eq!(open["items"][0]["text"], "Read immediately");
    assert_eq!(open["items"][0]["revision"], created["version"]);
    assert_eq!(
        f.call(
            "query_note_tasks",
            json!({"noteId":note["id"],"status":"open","offset":1})
        )["items"],
        json!([])
    );
    let completed = f.call("mutate_note_task", json!({"noteId":note["id"],"version":created["version"],"change":{"kind":"complete","taskId":created["id"],"revision":created["version"]}}));
    assert_eq!(
        f.call(
            "query_note_tasks",
            json!({"noteId":note["id"],"status":"open"})
        )["count"],
        0
    );
    assert_eq!(
        f.call(
            "query_note_tasks",
            json!({"noteId":note["id"],"status":"completed"})
        )["items"][0]["revision"],
        completed["version"]
    );
    f.error(
        "query_note_tasks",
        json!({"noteId":"../outside","status":"open"}),
        StatusCode::BAD_REQUEST,
    );
}

#[test]
fn task_reopen_records_each_completion_and_text_edits_do_not_create_writing_activity() {
    let f = Fixture::new(false);
    let note = f.call("create_item",json!({"meta":{"type":"page","title":"Draft","due_date":"2026-10-01"},"content":document("")}));
    let created = f.call(
        "mutate_note_task",
        json!({"noteId":note["id"],"change":{"kind":"create","text":"Revise"}}),
    );
    let task = &created["id"];
    let edited = f.call("mutate_note_task",json!({"noteId":note["id"],"change":{"kind":"edit","taskId":task,"revision":created["version"],"text":"Revise outline"}}));
    assert_eq!(edited["content"]["text"], "Revise outline");
    assert_eq!(edited["meta"]["due_date"], "2026-10-01");
    let changed_note = f.call(
        "update_item_meta",
        json!({"id":note["id"],"version":note["version"],"meta":{"due_date":"2026-11-01"}}),
    );
    assert_eq!(
        f.call("read_item", json!({"id":task}))["meta"]["due_date"],
        "2026-10-01"
    );
    assert_eq!(pending_completions(&f), 0);
    let completed = f.call("mutate_note_task",json!({"noteId":note["id"],"change":{"kind":"complete","taskId":task,"revision":edited["version"]}}));
    assert!(completed["content"]["completedAt"].is_string());
    f.error("mutate_note_task",json!({"noteId":note["id"],"change":{"kind":"reopen","taskId":task,"revision":edited["version"]}}),StatusCode::BAD_REQUEST);
    let reopened = f.call("mutate_note_task",json!({"noteId":note["id"],"change":{"kind":"reopen","taskId":task,"revision":completed["version"]}}));
    assert!(reopened["content"].get("completedAt").is_none());
    let second = f.call("mutate_note_task",json!({"noteId":note["id"],"change":{"kind":"complete","taskId":task,"revision":reopened["version"]}}));
    assert!(second["content"]["completedAt"].is_string());
    assert_eq!(
        f.call("read_item", json!({"id":note["id"]}))["version"],
        changed_note["version"]
    );
    assert_eq!(pending_completions(&f), 2);
}

#[test]
fn goals_preserve_scopes_dates_and_atomic_reservations() {
    let f = Fixture::new(true);
    let body = json!({"scope":"all_notes","startDate":"2026-09-01","endDate":"2026-10-31","dailyWordTarget":500});
    let first = f.call("create_writing_goal", body.clone());
    assert_eq!(first["meta"]["type"], "goal");
    assert_eq!(first["content"]["dailyWordTarget"], 500);
    assert!(f.error("create_writing_goal", json!({"scope":"all_notes","startDate":"2026-10-01","endDate":"2026-11-30","dailyWordTarget":250}), StatusCode::BAD_REQUEST).contains("already has a writing goal"));
    let later = f.call("create_writing_goal", json!({"scope":"all_notes","startDate":"2026-11-01","endDate":"2026-12-31","dailyWordTarget":300}));
    assert_ne!(later["id"], first["id"]);
    let view = f.call("create_item", json!({"meta":{"type":"workspace","title":"Novel chapters"},"content":{"query":crate::query::document::test_query("#novel sort:modified-desc")}}));
    let scoped = f.call("create_writing_goal", json!({"scope":"view","viewId":view["id"],"startDate":"2026-09-01","endDate":"2026-10-31","dailyWordTarget":500}));
    assert_eq!(scoped["content"]["viewId"], view["id"]);
    assert_eq!(
        scoped["content"]["query"],
        json!(crate::query::document::test_query(
            "#novel sort:modified-desc"
        ))
    );
    let concurrent = Fixture::new(false);
    let request = || {
        Request::builder()
            .method("POST")
            .uri("/api/create_writing_goal")
            .body(Body::from(body.to_string()))
            .unwrap()
    };
    let (a, b) = concurrent.runtime.block_on(futures_util::future::join(
        concurrent.send_async(request()),
        concurrent.send_async(request()),
    ));
    let mut statuses = [a.0, b.0];
    statuses.sort();
    assert_eq!(statuses, [StatusCode::OK, StatusCode::BAD_REQUEST]);
}

#[test]
fn concurrent_note_save_and_task_mutation_have_independent_revisions() {
    let f = Fixture::new(false);
    let note = f.call(
        "create_item",
        json!({"meta":{"type":"page","title":"Draft"},"content":document("")}),
    );
    let task = f.call(
        "mutate_note_task",
        json!({"noteId":note["id"],"change":{"kind":"create","text":"Revise"}}),
    );
    let request = |name: &str, body: Value| {
        Request::builder()
            .method("POST")
            .uri(format!("/api/{name}"))
            .body(Body::from(body.to_string()))
            .unwrap()
    };
    let a = request(
        "update_item",
        json!({"id":note["id"],"version":note["version"],"meta":{"title":"Updated"},"content":note["content"]}),
    );
    let b = request(
        "mutate_note_task",
        json!({"noteId":note["id"],"version":note["version"],"change":{"kind":"complete","taskId":task["id"],"revision":task["version"]}}),
    );
    let (a, b) = f
        .runtime
        .block_on(futures_util::future::join(f.send_async(a), f.send_async(b)));
    let mut statuses = [a.0, b.0];
    statuses.sort();
    assert_eq!(statuses, [StatusCode::OK, StatusCode::OK]);
}
