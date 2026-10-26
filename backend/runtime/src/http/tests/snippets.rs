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
    let owner=f.call("create_item",json!({"meta":{"type":"page","title":"Snippet owner"},"content":{"type":"doc","content":[{"type":"paragraph","content":[{"type":"citation","attrs":{"source":"snippet","snippetId":"source"}}]},{"type":"embed","attrs":{"snippetId":"source"}}]}}));
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
    f.call("update_item",json!({"id":owner["id"],"version":owner["version"],"meta":owner["meta"],"content":document("")}));
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
    let filename = crate::engine::project::snippets::path(f.root.path(), "source").unwrap();
    let saved = std::fs::read(&filename).unwrap();
    std::fs::write(&filename, "{").unwrap();
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
        assert_eq!(std::fs::read_to_string(&filename).unwrap(), "{");
    }
    std::fs::write(filename, saved).unwrap();
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
    f.initialize_index();
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
    let indexes: Vec<_> = std::fs::read_dir(f.root.path().join(".cache"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.join("meta.json").is_file())
        .collect();
    assert_eq!(indexes.len(), 1);
    assert!(!f.root.path().join(".cache/snippets").exists());
    f.runtime.block_on(f.app.stop()).unwrap();
    std::fs::remove_dir_all(&indexes[0]).unwrap();
    f.reopen(true);
    let result = f.call("query_snippets", json!({"query":"replacement"}));
    assert_eq!(result["items"].as_array().unwrap().len(), 1);
    assert_eq!(result["items"][0]["id"], "alpha");
    assert_eq!(
        f.call("read_snippet", json!({"id":"alpha"}))["paragraph"]["content"][0]["text"],
        "Replacement"
    );
}

fn reference(id: &str) -> serde_json::Value {
    json!({"type":"doc","content":[{"type":"paragraph","content":[{"type":"citation","attrs":{"source":"snippet","snippetId":id}}]}]})
}

#[test]
fn snippet_usage_checks_include_pending_writes_archived_owners_and_direct_file_removal() {
    let f = Fixture::new(false);
    let snippet = f.call(
        "create_snippet",
        json!({"id":"shared","paragraph":paragraph("Shared source")}),
    );
    let owner = f.page("Owner", "");
    f.initialize_index();
    let referencing=f.call("update_item",json!({"id":owner["id"],"version":owner["version"],"meta":owner["meta"],"content":{"type":"doc","content":[{"type":"paragraph","content":[{"type":"citation","attrs":{"source":"snippet","snippetId":"shared"}},{"type":"citation","attrs":{"source":"snippet","snippetId":"shared"}}]},{"type":"embed","attrs":{"snippetId":"shared"}}]}}));
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
    f.settled();
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
    std::fs::remove_dir_all(f.item_path(&owner["id"]).parent().unwrap()).unwrap();
    assert_eq!(
        f.call("read_snippet_usages", json!({"id":"shared"})),
        json!([])
    );
    assert_eq!(
        f.call("query_snippets", json!({}))["items"][0]["usageCount"],
        0
    );
}

#[test]
fn snippet_counts_check_externally_changed_indexed_owners() {
    let f = Fixture::new(false);
    f.call(
        "create_snippet",
        json!({"id":"shared","paragraph":paragraph("Shared source")}),
    );
    let owner = f.call(
        "create_item",
        json!({"meta":{"type":"page","title":"Owner"},"content":reference("shared")}),
    );
    f.initialize_index();
    assert_eq!(
        f.call("query_snippets", json!({}))["items"][0]["usageCount"],
        1
    );
    let filename = f.item_path(&owner["id"]);
    let mut changed: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&filename).unwrap()).unwrap();
    changed["content"]["content"][0]["content"] =
        json!([{"type":"text","text":"Reference removed outside Bluink"}]);
    std::fs::write(filename, changed.to_string()).unwrap();
    assert_eq!(
        f.call("query_snippets", json!({}))["items"][0]["usageCount"],
        0
    );
}

#[test]
fn archive_checks_observed_external_references_before_index_commit() {
    let f = Fixture::new(false);
    let snippet = f.call(
        "create_snippet",
        json!({"id":"external_source","paragraph":paragraph("Reusable")}),
    );
    let owner = f.page("External owner", "");
    f.initialize_index();
    let filename = f.item_path(&owner["id"]);
    let original = std::fs::read(&filename).unwrap();
    let mut changed: serde_json::Value = serde_json::from_slice(&original).unwrap();
    changed["revision"] = json!("external-revision");
    changed["content"]["content"][0]["content"] =
        reference("external_source")["content"][0]["content"].clone();
    std::fs::write(&filename, changed.to_string()).unwrap();
    // Model watcher delivery without letting the worker commit. Like declaration
    // removal, snippet archiving includes observed changes in the dirty overlay.
    crate::indexing::service::mark_pending_write(
        f.root.path(),
        owner["id"].as_str().unwrap().to_owned(),
        String::new(),
    )
    .unwrap();
    assert!(
        f.error(
            "archive_snippet",
            json!({"id":"external_source","revision":snippet["revision"]}),
            StatusCode::BAD_REQUEST
        )
        .contains("used by")
    );
    std::fs::write(filename, original).unwrap();
    assert_eq!(
        f.call(
            "archive_snippet",
            json!({"id":"external_source","revision":snippet["revision"]})
        )["state"],
        "archived"
    );
    assert!(f.error("update_item",json!({"id":owner["id"],"version":owner["version"],"meta":owner["meta"],"content":reference("external_source")}),StatusCode::BAD_REQUEST).contains("archived"));
}

#[test]
fn unused_snippet_archive_does_not_enumerate_item_storage() {
    let f = Fixture::new(false);
    let snippet = f.call(
        "create_snippet",
        json!({"id":"unused","paragraph":paragraph("Reusable")}),
    );
    let unrelated = f.page("Unrelated", "No references");
    f.initialize_index();
    // An unrelated broken note must not be read by an indexed usage check.
    // The shallow watcher does not subscribe to this nested item file.
    let file = f.item_path(&unrelated["id"]);
    let original = std::fs::read(&file).unwrap();
    std::fs::write(&file, "{").unwrap();
    let archived = f.call(
        "archive_snippet",
        json!({"id":"unused","revision":snippet["revision"]}),
    );
    std::fs::write(file, original).unwrap();
    assert_eq!(archived["state"], "archived");
}
