// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use super::support::*;
use axum::http::StatusCode;
use serde_json::json;

fn paragraph(text: &str) -> serde_json::Value {
    json!({"type":"paragraph","content":[{"type":"text","text":text}]})
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "Keep each end-to-end scenario together so setup, failure checks and cleanup remain visible."
)]
fn snippets_preserve_named_identity_revisions_usages_and_storage_error_responses() {
    let f = Fixture::new(true);
    assert!(f.call("read_snippet", json!({"id":"missing"})).is_null());
    assert_eq!(
        f.call("query_snippets", json!({})),
        json!({"items":[],"count":0})
    );
    let created = f.call(
        "create_snippet",
        json!({"id":"source","paragraph":paragraph("Original")}),
    );
    assert_eq!(created["id"], "source");
    assert_eq!(f.call("read_snippet", json!({"id":"source"})), created);
    let updated=f.call("update_snippet",json!({"id":"source","revision":created["revision"],"paragraph":paragraph("文学 [literal]")}));
    assert_ne!(updated["revision"], created["revision"]);
    assert_eq!(updated["createdAt"], created["createdAt"]);
    let mut hit = updated.clone();
    hit["usageCount"] = json!(0);
    assert_eq!(
        f.call("query_snippets", json!({"query":"[literal]","limit":1})),
        json!({"count":1,"items":[hit]})
    );
    assert_eq!(
        f.call("query_snippets", json!({"query":".*"})),
        json!({"items":[],"count":0})
    );
    assert_eq!(
        f.call("query_snippets", json!({"offset":1})),
        json!({"items":[],"count":1})
    );
    for (name, body) in [
        (
            "create_snippet",
            json!({"id":"source","paragraph":paragraph("Duplicate")}),
        ),
        (
            "update_snippet",
            json!({"id":"source","revision":created["revision"],"paragraph":paragraph("Stale")}),
        ),
        (
            "archive_snippet",
            json!({"id":"source","revision":created["revision"]}),
        ),
        ("read_snippet", json!({"id":"../escape"})),
        ("read_snippet_usages", json!({"id":"Bad Key"})),
        (
            "create_snippet",
            json!({"id":"invalid","paragraph":{"type":"heading"}}),
        ),
        ("query_snippets", json!({"limit":0})),
        ("query_snippets", json!({"limit":201})),
        ("query_snippets", json!({"offset":-1})),
        (
            "update_snippet",
            json!({"id":"source","paragraph":paragraph("No revision")}),
        ),
    ] {
        f.error(name, body, StatusCode::BAD_REQUEST);
    }
    let owner=f.call("create_item",json!({"meta":{"type":"page","title":"Snippet owner"},"content":{"document": {"type":"doc","content":[{"type":"paragraph","content":[{"type":"citation","attrs":{"source":"snippet","snippetId":"source"}}]},{"type":"embed","attrs":{"snippetId":"source"}}]}}}));
    let usages = f.call("read_snippet_usages", json!({"id":"source"}));
    assert_eq!(usages[0]["noteId"], owner["id"]);
    assert_eq!(
        f.call("query_snippets", json!({}))["items"][0]["usageCount"],
        1
    );
    assert_eq!(usages[0]["noteTitle"], "Snippet owner");
    assert_eq!(
        usages[0]["occurrences"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v["kind"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec!["citation", "embed"]
    );
    assert_eq!(
        f.call("query_snippets", json!({}))["items"][0]["usageCount"],
        1
    );
    f.error(
        "archive_snippet",
        json!({"id":"source","revision":updated["revision"]}),
        StatusCode::BAD_REQUEST,
    );
    f.call("update_item",json!({"id":owner["id"],"version":owner["version"],"meta":owner["meta"],"content":page_content("")}));
    assert_eq!(
        f.call("read_snippet_usages", json!({"id":"source"})),
        json!([])
    );
    let archived = f.call(
        "archive_snippet",
        json!({"id":"source","revision":updated["revision"]}),
    );
    assert_eq!(archived["state"], "archived");
    assert_eq!(archived["id"], created["id"]);
    assert_eq!(
        f.call("read_snippet", json!({"id":"source"}))["paragraph"],
        updated["paragraph"]
    );
    assert!(
        f.error(
            "create_snippet",
            json!({"id":"source","paragraph":paragraph("Reuse key")}),
            StatusCode::BAD_REQUEST
        )
        .contains("already exists")
    );
    assert!(f.error("update_snippet",json!({"id":"source","revision":archived["revision"],"paragraph":paragraph("Edit archived")}),StatusCode::BAD_REQUEST).contains("archived"));
    assert_eq!(
        f.call("query_snippets", json!({})),
        json!({"items":[],"count":0})
    );
    let db = rusqlite::Connection::open(f.root.path().join(crate::store::DATABASE_NAME)).unwrap();
    db.execute_batch("ALTER TABLE body RENAME TO unavailable_body")
        .unwrap();
    for (name, body) in [
        ("read_snippet", json!({"id":"source"})),
        (
            "update_snippet",
            json!({"id":"source","revision":archived["revision"],"paragraph":paragraph("Changed")}),
        ),
        (
            "archive_snippet",
            json!({"id":"source","revision":archived["revision"]}),
        ),
    ] {
        f.error(name, body, StatusCode::INTERNAL_SERVER_ERROR);
    }
    db.execute_batch("ALTER TABLE unavailable_body RENAME TO body")
        .unwrap();
    assert_eq!(f.call("read_snippet", json!({"id":"source"})), archived);
}

#[test]
fn snippets_reject_invalid_keys_dependencies_and_formatting_and_isolate_namespaces() {
    let f = Fixture::new(true);
    for kind in ["tag", "label"] {
        f.call("create_facet", json!({"kind":kind,"key":"shared"}));
    }
    f.call(
        "create_snippet",
        json!({"id":"shared","paragraph":paragraph("Different record")}),
    );
    for id in ["../escape", "Smith2024"] {
        assert!(
            f.error(
                "create_snippet",
                json!({"id":id,"paragraph":paragraph("Unsafe")}),
                StatusCode::BAD_REQUEST
            )
            .contains("lowercase snake-case")
        );
    }
    assert!(
        f.error(
            "query_snippets",
            json!({"query":"","limit":0}),
            StatusCode::BAD_REQUEST
        )
        .contains("limit")
    );
    assert_eq!(
        f.call("query_snippets", json!({"query":"","limit":1,"offset":1}))["items"],
        json!([])
    );
    for (id, marks, error) in [
        (
            "internal",
            json!([{"type":"link","attrs":{"href":"/?note=01a0e2a0-0000-7000-8000-000000000002"}}]),
            "snippets cannot link to pages",
        ),
        (
            "mixed",
            json!([{"type":"strong"},{"type":"emphasis"}]),
            "Invalid collection of marks",
        ),
    ] {
        assert!(f.error("create_snippet",json!({"id":id,"paragraph":{"type":"paragraph","content":[{"type":"text","text":"Source","marks":marks}]}}),StatusCode::BAD_REQUEST).contains(error));
    }
    assert_eq!(f.call("create_snippet",json!({"id":"external","paragraph":{"type":"paragraph","content":[{"type":"text","text":"Private source","marks":[{"type":"link","attrs":{"href":"https://private.corporate.example/source"}}]}]}}))["id"],"external");
}

#[test]
fn concurrent_snippet_creation_across_two_application_handles_has_exactly_one_winner() {
    use axum::body::Body;
    use axum::http::Request;
    let f = Fixture::new(false);
    let second = Fixture::with_options(false, |options| {
        options.project_root = f.root.path().to_string_lossy().into_owned();
    });
    let request = |text: &str| {
        Request::builder()
            .method("POST")
            .uri("/api/create_snippet")
            .body(Body::from(
                json!({"id":"shared","paragraph":paragraph(text)}).to_string(),
            ))
            .unwrap()
    };
    let (a, b) = f.runtime.block_on(futures_util::future::join(
        f.send_async(request("First")),
        second.send_async(request("Second")),
    ));
    let mut statuses = [a.0, b.0];
    statuses.sort();
    assert_eq!(statuses, [StatusCode::OK, StatusCode::BAD_REQUEST]);

    assert_eq!(f.call("query_snippets", json!({}))["count"], 1);
}

#[test]
fn snippet_search_tracks_canonical_changes_and_rebuilds_its_disposable_cache() {
    let mut f = Fixture::new(true);
    let first = f.call(
        "create_snippet",
        json!({"id":"alpha","paragraph":paragraph("Mixed CASE [literal].")}),
    );
    let second = f.call(
        "create_snippet",
        json!({"id":"beta","paragraph":paragraph("Second source")}),
    );
    let page = f.call("query_snippets", json!({"query":"","limit":1,"offset":1}));
    assert_eq!(page["items"].as_array().unwrap().len(), 1);
    assert_eq!(page["items"][0]["id"], "beta");
    for (query, count) in [("case", 1), ("[literal]", 1), (".*", 0)] {
        assert_eq!(
            f.call("query_snippets", json!({"query":query}))["count"],
            count
        );
    }
    f.call(
        "update_snippet",
        json!({"id":"alpha","revision":first["revision"],"paragraph":paragraph("Replacement")}),
    );
    assert_eq!(
        f.call("query_snippets", json!({"query":"case"}))["count"],
        0
    );
    assert_eq!(
        f.call("query_snippets", json!({"query":"replacement"}))["count"],
        1
    );
    f.call(
        "archive_snippet",
        json!({"id":"beta","revision":second["revision"]}),
    );
    assert_eq!(f.call("query_snippets", json!({}))["count"], 1);
    f.runtime.block_on(f.app.stop()).unwrap();
    f.reopen(true);
    let result = f.call("query_snippets", json!({"query":"replacement"}));
    assert_eq!(result["items"].as_array().unwrap().len(), 1);
    assert_eq!(result["items"][0]["id"], "alpha");
    assert_eq!(
        f.call("read_snippet", json!({"id":"alpha"}))["paragraph"]["content"][0]["text"],
        "Replacement"
    );
}

#[test]
fn snippet_usage_checks_include_archived_owners() {
    let f = Fixture::new(false);
    let snippet = f.call(
        "create_snippet",
        json!({"id":"shared","paragraph":paragraph("Shared source")}),
    );
    let owner = f.page("Owner", "");

    let referencing=f.call("update_item",json!({"id":owner["id"],"version":owner["version"],"meta":owner["meta"],"content":{"document": {"type":"doc","content":[{"type":"paragraph","content":[{"type":"citation","attrs":{"source":"snippet","snippetId":"shared"}},{"type":"citation","attrs":{"source":"snippet","snippetId":"shared"}}]},{"type":"embed","attrs":{"snippetId":"shared"}}]}}}));
    assert_eq!(
        f.call("query_snippets", json!({}))["items"][0]["usageCount"],
        1
    );
    let usages = f.call("read_snippet_usages", json!({"id":"shared"}));
    assert_eq!(usages[0]["occurrences"].as_array().unwrap().len(), 3);
    assert_eq!(usages[0]["noteId"], owner["id"]);
    f.call(
        "update_item_meta",
        json!({"id":owner["id"],"version":referencing["version"],"meta":{"state":"archived"}}),
    );
    assert_eq!(
        f.call("read_snippet_usages", json!({"id":"shared"}))
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        f.call("query_snippets", json!({}))["items"][0]["usageCount"],
        1
    );
    assert!(
        f.error(
            "archive_snippet",
            json!({"id":"shared","revision":snippet["revision"]}),
            StatusCode::BAD_REQUEST
        )
        .contains("used by")
    );
}
