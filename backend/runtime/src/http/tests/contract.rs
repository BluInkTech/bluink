// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use super::support::*;
use axum::http::{StatusCode, header};
use serde_json::{Value, json};

#[test]
fn packaged_analysis_workers_are_allowed_without_broadening_document_scripts() {
    let fixture = Fixture::new(false);
    let (_, headers, _) = fixture.request("GET", "/", "");
    let policy = headers[header::CONTENT_SECURITY_POLICY].to_str().unwrap();
    let directives: Vec<_> = policy.split(';').collect();
    assert!(directives.contains(&"worker-src 'self' blob:"));
    assert!(directives.contains(&"script-src 'self' 'wasm-unsafe-eval'"));
    assert!(directives.contains(&"script-src-attr 'none'"));
    assert!(directives.contains(&"object-src 'none'"));
}

#[test]
fn every_rpc_preserves_methods_and_required_fields_through_the_production_router() {
    let fixture = Fixture::new(true);
    let contract: Value =
        serde_json::from_str(include_str!("../../../../test-fixtures/http-contract.json")).unwrap();
    for (name, entry) in contract.as_object().unwrap() {
        assert_ne!(entry["owner"].as_str().unwrap(), "");
        let method = entry["method"].as_str().unwrap();
        assert_eq!(
            crate::application::operation::Request::http_method(name),
            Some(method)
        );
        let (status, _, bytes) = fixture.request(
            if method == "POST" { "GET" } else { "POST" },
            &format!("/api/{name}"),
            "",
        );
        assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED, "{name}");
        assert_eq!(
            serde_json::from_slice::<Value>(&bytes).unwrap(),
            json!({"error":"ERR_RPC_INVALID_METHOD","diagnostics":[]})
        );
        if let Some(required) = entry["required"].as_array().unwrap().first() {
            assert_eq!(
                fixture.error(
                    &format!("{name}?contract=1"),
                    json!({}),
                    StatusCode::BAD_REQUEST
                ),
                format!("Missing required field: {}", required.as_str().unwrap())
            );
        }
    }
}

#[test]
fn malformed_bodies_raw_operation_paths_and_missing_items_keep_json_error_classification() {
    let fixture = Fixture::new(false);
    for (endpoint, body, status, error) in [
        ("read_item", "{", 400, "ERR_INVALID_JSON"),
        ("read_item", "null", 400, "Request body must be an object"),
        ("read_item", "[]", 400, "Request body must be an object"),
        (
            "read_item",
            "\"text\"",
            400,
            "Request body must be an object",
        ),
        (
            "create_item",
            r#"{"meta":{"type":"page","title":"\ud83d"}}"#,
            400,
            "ERR_INVALID_JSON",
        ),
        ("read_item", r#"{"id":"\uD800"}"#, 400, "ERR_INVALID_JSON"),
        ("missing_operation", "{}", 404, "ERR_RPC_ENDPOINT_NOT_FOUND"),
        ("analyze_query", "{}", 404, "ERR_RPC_ENDPOINT_NOT_FOUND"),
        ("parse_query", "{}", 404, "ERR_RPC_ENDPOINT_NOT_FOUND"),
        ("format_query", "{}", 404, "ERR_RPC_ENDPOINT_NOT_FOUND"),
        (
            "read_home_widget_types",
            "{}",
            404,
            "ERR_RPC_ENDPOINT_NOT_FOUND",
        ),
        ("read%5fitem", "{}", 404, "ERR_RPC_ENDPOINT_NOT_FOUND"),
        ("%ff", "{}", 404, "ERR_RPC_ENDPOINT_NOT_FOUND"),
        ("memory_usage", "{}", 404, "ERR_RPC_ENDPOINT_NOT_FOUND"),
        (
            "read_item",
            r#"{"id":"00000000-0000-7000-8000-000000000999"}"#,
            400,
            "Item 00000000-0000-7000-8000-000000000999 not found",
        ),
    ] {
        let (actual, headers, bytes) =
            fixture.request("POST", &format!("/api/{endpoint}"), body.to_owned());
        assert_eq!(actual.as_u16(), status, "{endpoint}");
        assert_eq!(headers[header::X_CONTENT_TYPE_OPTIONS], "nosniff");
        assert_eq!(
            serde_json::from_slice::<Value>(&bytes).unwrap(),
            json!({"error":error,"diagnostics":[]})
        );
    }
}

#[test]
fn document_writes_keep_larger_limits_and_unicode_public_projection() {
    let fixture = Fixture::new(false);
    for (endpoint, size) in [
        ("read_item", 1_048_577),
        ("create_item", 16 * 1_048_576 + 1),
        ("update_item", 16 * 1_048_576 + 1),
    ] {
        let (status, _, bytes) =
            fixture.request("POST", &format!("/api/{endpoint}"), " ".repeat(size));
        assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
        assert_eq!(
            serde_json::from_slice::<Value>(&bytes).unwrap(),
            json!({"error":"ERR_PAYLOAD_TOO_LARGE","diagnostics":[]})
        );
    }
    let payload = json!({"meta":{"type":"page","title":"Unicode 文学 🌍"},"content":{"type":"doc","content":[{"type":"paragraph"}]}});
    let (status, _, bytes) = fixture.request(
        "POST",
        "/api/create_item",
        format!("{}{payload}", " ".repeat(1_048_576)),
    );
    assert_eq!(status, StatusCode::OK);
    let item: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(item["meta"]["title"], payload["meta"]["title"]);
    assert!(item["version"].is_string());
    assert!(item.get("revision").is_none());
    assert_eq!(
        fixture.call(
            "read_item",
            json!({"id":item["id"],"options":{"content":false}})
        )["content"],
        json!({})
    );
    let large = fixture.page("Large content", &"a".repeat(1_048_576));
    assert_eq!(
        large["content"]["content"][0]["content"][0]["text"]
            .as_str()
            .unwrap()
            .len(),
        1_048_576
    );
}

#[test]
fn empty_queries_calendar_metadata_and_cache_status_keep_their_http_shapes() {
    let fixture = Fixture::new(true);
    for (name, body, expected) in [
        (
            "query_note_tasks",
            json!({"status":"open"}),
            json!({"items":[],"count":0}),
        ),
        (
            "query_label_passages",
            json!({"label":"review"}),
            json!({"items":[],"count":0}),
        ),
        ("list_facets", json!({}), json!([])),
        ("read_facet_counts", json!({"kind":"tag"}), json!([])),
    ] {
        assert_eq!(fixture.call(name, body), expected);
    }
    for field in ["created", "modified", "completed"] {
        assert_eq!(fixture.call("read_calendar_buckets", json!({"query":crate::query::document::QueryDocument::active_pages(),"field":field,"startDate":"2026-10-01","endDate":"2026-10-31","granularity":"day"})), json!({"buckets":[]}));
    }
    assert!(
        fixture
            .call("read_metadata_fields", json!({}))
            .as_array()
            .unwrap()
            .iter()
            .any(|field| field["key"] == "summary" && field["source"] == "builtin")
    );
    assert!(
        !fixture.call(
            "validate_query",
            json!({"document":crate::query::document::test_query("unregistered:value")})
        )["valid"]
            .as_bool()
            .unwrap()
    );
    let status = fixture.call("cache_status", json!({}));
    assert_eq!(status["structuralState"], "ready");
    assert_eq!(
        status["documents"],
        crate::engine::project::item::list_project_item_ids(fixture.root.path())
            .unwrap()
            .len()
    );
    assert_eq!(status["schemaVersion"], 1);
    let config = crate::engine::project::config::read_project_config(fixture.root.path())
        .unwrap()
        .unwrap();
    assert_eq!(status["projectId"], config["id"]);
    for (name, body) in [
        ("query_note_tasks", json!({"status":"open","limit":201})),
        ("query_note_tasks", json!({"status":"open","offset":-1})),
        ("list_facets", json!({"kind":"invalid"})),
        ("read_facet_counts", json!({"kind":"invalid"})),
    ] {
        fixture.error(name, body, StatusCode::BAD_REQUEST);
    }
}
