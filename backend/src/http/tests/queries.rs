// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use crate::structured_query::fixtures::{asc, criteria, desc, field, term};

use super::support::*;
use axum::http::StatusCode;
use chrono::Utc;
use serde_json::{Value, json};

#[test]
fn query_previews_and_calendar_are_visible_after_commit_and_reopen() {
    let mut f = Fixture::new(false);
    let item=f.call("create_item",json!({"meta":{"type":"page","title":"Native search"},"content":{"document": {"type":"doc","content":[{"type":"paragraph","content":[{"type":"text","text":"A searchable portable document"}]},{"type":"image","attrs":{"src":"/assets/cover.jpg"}}]}}}));
    let initial = f.call(
        "query_items",
        json!({"query":criteria([term("searchable")])}),
    );
    assert_eq!(initial["count"], 1);
    assert_eq!(initial["items"][0]["id"], item["id"]);
    assert_eq!(
        initial["items"][0]["preview"],
        "A searchable portable document"
    );
    assert_eq!(initial["items"][0]["previewImage"], "/assets/cover.jpg");
    let date = &item["createdAt"].as_str().unwrap()[..10];
    assert_eq!(f.call("read_calendar_buckets",json!({"query":crate::structured_query::document::QueryDocument::active_pages(),"field":"created","startDate":date,"endDate":date,"granularity":"day"})),json!({"buckets":[{"date":date,"count":1}]}));
    f.call("update_item",json!({"id":item["id"],"version":item["version"],"meta":item["meta"],"content":page_content("Externally changed content")}));
    assert_eq!(
        f.call(
            "query_items",
            json!({"query":criteria([term("externally")])})
        )["items"][0]["id"],
        item["id"]
    );
    f.reopen(true);
    assert_eq!(
        f.call(
            "query_items",
            json!({"query":criteria([term("externally")])})
        )["items"][0]["id"],
        item["id"]
    );
}

#[test]
fn task_index_keeps_page_owned_tasks_when_prose_is_replaced() {
    let f = Fixture::new(false);
    let note = f.page("Task host", "Draft");
    let review = f.call(
        "mutate_note_task",
        json!({"noteId":note["id"],"change":{"kind":"create","text":"Review"}}),
    );
    let source = f.call(
        "mutate_note_task",
        json!({"noteId":note["id"],"change":{"kind":"create","text":"Find source"}}),
    );
    let open = review["id"].as_str().unwrap();
    let orphan = source["id"].as_str().unwrap();
    let initial = f.call(
        "query_note_tasks",
        json!({"query":criteria([field("type", "page")]),"status":"open","limit":1,"offset":0}),
    );
    assert_eq!(initial["count"], 2);
    assert_eq!(initial["items"].as_array().unwrap().len(), 1);
    assert_eq!(initial["items"][0]["noteTitle"], "Task host");
    f.page("Unrelated one", "No tasks");
    f.page("Unrelated two", "No tasks");
    let broad = f.call(
        "query_note_tasks",
        json!({"query":criteria([field("type", "page")]),"status":"open"}),
    );
    assert_eq!(broad["count"], 2);
    assert!(
        broad["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|task| task["noteTitle"] == "Task host")
    );
    let all = f.call(
        "query_note_tasks",
        json!({"noteId":note["id"],"status":"open"}),
    );
    let mut tasks = all["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|task| {
            (
                task["taskId"].as_str().unwrap(),
                task["noteId"].as_str().unwrap(),
                task["text"].as_str().unwrap(),
            )
        })
        .collect::<Vec<_>>();
    tasks.sort_unstable();
    let mut expected = vec![
        (open, note["id"].as_str().unwrap(), "Review"),
        (orphan, note["id"].as_str().unwrap(), "Find source"),
    ];
    expected.sort_unstable();
    assert_eq!(tasks, expected);
    assert_eq!(
        f.call(
            "query_note_tasks",
            json!({"query":criteria([field("tag", "missing")]),"status":"open"})
        )["count"],
        0
    );
    f.call("update_item",json!({"id":note["id"],"version":note["version"],"meta":note["meta"],"content":page_content("Replaced all prose")}));
    f.call("mutate_note_task",json!({"noteId":note["id"],"change":{"kind":"complete","taskId":open,"revision":review["version"]}}));
    f.call("mutate_note_task",json!({"noteId":note["id"],"change":{"kind":"delete","taskId":orphan,"revision":source["version"]}}));
    assert_eq!(
        f.call(
            "query_note_tasks",
            json!({"noteId":note["id"],"status":"completed"})
        )["items"][0]["taskId"],
        open
    );
    assert_eq!(
        f.call(
            "query_note_tasks",
            json!({"noteId":note["id"],"status":"open"})
        )["count"],
        0
    );
}

#[test]
fn named_facets_and_custom_properties_preserve_inference_counts_and_query_analysis() {
    let f = Fixture::new(false);
    f.call(
        "create_property",
        json!({"key":"author","valueType":"text"}),
    );
    let target = f.page("Target", "Target");
    let source=f.call("create_item",json!({"meta":{"type":"page","title":"Source","author":"Roberto Bolaño"},"tags":["research_methods"],"content":{"document": {"type":"doc","content":[{"type":"paragraph","content":[{"type":"text","text":"Tagged source","marks":[{"type":"link","attrs":{"href":format!("/?note={}",target["id"].as_str().unwrap())}}]}]}]}}}));
    assert_eq!(
        f.call(
            "read_facet_counts",
            json!({"kind":"tag","keys":[],"sourceType":"page"})
        ),
        json!([{"key":"research_methods","count":1}])
    );
    let tags = json!([{"key":"research_methods","kind":"tag","usageCount":1,"inferred":true}]);
    assert_eq!(f.call("list_facets", json!({"kind":"tag"})), tags);
    f.call("create_facet", json!({"kind":"tag","key":"unused"}));
    assert_eq!(
        f.call("list_facets", json!({"kind":"tag"})),
        json!([tags[0],{"key":"unused","kind":"tag","usageCount":0,"inferred":false}])
    );
    f.call("remove_facet", json!({"kind":"tag","key":"unused"}));
    assert_eq!(f.call("list_facets", json!({"kind":"tag"})), tags);
    f.call("create_facet", json!({"kind":"label","key":"review"}));
    assert_eq!(f.call("list_facets", json!({"kind":"tag"})), tags);
    assert_eq!(
        f.call("list_facets", json!({"kind":"label"})),
        json!([{"key":"review","kind":"label","usageCount":0,"inferred":false}])
    );
    f.call("remove_facet", json!({"kind":"label","key":"review"}));
    assert_eq!(f.call("list_facets", json!({"kind":"label"})), json!([]));
    let fields = f.call("read_metadata_fields", json!({}));
    assert!(fields[1]["revision"].is_string());
    assert_eq!(
        &fields.as_array().unwrap()[..2],
        &[
            json!({"key":"summary","valueType":"text","source":"builtin"}),
            json!({"key":"author","valueType":"text","source":"registered","revision":fields[1]["revision"]})
        ]
    );
    assert!(
        fields
            .as_array()
            .unwrap()
            .iter()
            .any(|field| field["key"] == "status" && field["system"] == true)
    );
    for query in [
        criteria([field("tag", "research_methods")]),
        criteria([field("author", "Roberto Bolaño")]),
        criteria([field("tag", "research_methods")]).sorted([desc("modified")]),
    ] {
        let result = f.call("query_items", json!({"query":query}));
        assert_eq!(result["items"].as_array().unwrap().len(), 1);
        assert_eq!(result["items"][0]["id"], source["id"]);
    }
    assert_eq!(
        f.call(
            "query_items",
            json!({"query":criteria([field("tag", "missing")])})
        )["items"],
        json!([])
    );
    let mut ids = f.call(
        "query_items",
        json!({"query":criteria([field("type", "page")])}),
    )["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["id"].as_str().unwrap().to_owned())
        .collect::<Vec<_>>();
    ids.sort();
    let mut expected = vec![
        source["id"].as_str().unwrap().to_owned(),
        target["id"].as_str().unwrap().to_owned(),
    ];
    expected.sort();
    assert_eq!(ids, expected);
    assert_eq!(
        f.call(
            "validate_query",
            json!({"document":criteria([field("author", "Roberto Bolaño")])})
        )["valid"],
        true
    );
    let invalid = f.call(
        "validate_query",
        json!({"document":criteria([field("unknown_property", "value")])}),
    );
    assert!(!invalid["valid"].as_bool().unwrap());
    assert!(
        invalid["diagnostics"][0]["message"]
            .as_str()
            .unwrap()
            .contains("Unknown query field")
    );
}

#[test]
fn string_and_numeric_fast_fields_keep_sort_precedence_missing_values_and_paging() {
    let f = Fixture::new(false);
    f.call(
        "create_property",
        json!({"key":"genre","valueType":"select","options":["biography","science_fiction"]}),
    );
    f.call(
        "create_property",
        json!({"key":"rating","valueType":"number"}),
    );
    let beta=f.call("create_item",json!({"meta":{"type":"page","title":"Beta","genre":"science_fiction","rating":2.5},"content":page_content("Beta")}));
    let alpha=f.call("create_item",json!({"meta":{"type":"page","title":"Alpha","genre":"biography","rating":4.5},"content":page_content("Alpha")}));
    let missing=f.call("create_item",json!({"meta":{"type":"page","title":"Missing","rating":3.5},"content":page_content("Missing")}));
    for query in [
        criteria([field("type", "page")]).sorted([asc("genre")]),
        criteria([field("type", "page")]).sorted([asc("title")]),
    ] {
        assert_eq!(
            ids(&f.call("query_items", json!({"query":query,"limit":3}))),
            json!([alpha["id"], beta["id"], missing["id"]])
        );
    }
    let page = f.call(
        "query_items",
        json!({"query":criteria([field("type", "page")]).sorted([desc("rating")]),"limit":1,"offset":1}),
    );
    assert_eq!(ids(&page), json!([missing["id"]]));
    assert_eq!(page["count"], 3);
}

fn ids(result: &Value) -> Value {
    Value::Array(
        result["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|hit| hit["id"].clone())
            .collect(),
    )
}

#[test]
fn query_paging_is_bounded_and_calendar_rejects_impossible_dates() {
    let f = Fixture::new(false);
    f.call(
        "create_item",
        json!({"meta":{"type":"page","title":"Page"},"content":page_content("One")}),
    );
    let query = crate::structured_query::document::QueryDocument::active_pages();
    for limit in [0, 10_001, u32::MAX] {
        f.error(
            "query_items",
            json!({"query":query,"limit":limit}),
            StatusCode::BAD_REQUEST,
        );
    }
    let result = f.call(
        "query_items",
        json!({"query":query,"limit":10,"offset":u32::MAX}),
    );
    assert_eq!(result["count"], 1);
    assert_eq!(result["items"], json!([]));
    for date in ["2026-02-30", "2026-13-01", "2025-02-29"] {
        f.error("read_calendar_buckets", json!({"query":query,"field":"created","startDate":date,"endDate":date,"granularity":"day"}), StatusCode::BAD_REQUEST);
    }
}

#[test]
fn direct_pins_are_applied_before_sorted_paging() {
    let f = Fixture::new(false);
    let view = f.call(
        "create_item",
        json!({"meta":{"type":"workspace","title":"Pinned view"},"content":{}}),
    );
    let alpha = f.page("Alpha", "Alpha");
    let pinned=f.call("create_item",json!({"meta":{"type":"page","title":"Zulu"},"content":page_content("Pinned Zulu"),"pins":[view["id"]]}));
    let beta = f.page("Beta", "Beta");
    let first = f.call(
        "query_items",
        json!({"query":criteria([field("type", "page")]).sorted([asc("title")]),"limit":2,"offset":0,"pinScopeId":view["id"]}),
    );
    assert_eq!(first["pinnedCount"], 1);
    assert_eq!(ids(&first), json!([pinned["id"], alpha["id"]]));
    assert_eq!(ids(&f.call("query_items",json!({"query":criteria([field("type", "page")]).sorted([asc("title")]),"limit":2,"offset":2,"pinScopeId":view["id"]}))),json!([beta["id"]]));
    assert_eq!(
        ids(&f.call(
            "query_items",
            json!({"query":criteria([field("type", "page"), crate::structured_query::fixtures::pinned()]),"limit":10,"offset":0,"pinScopeId":view["id"]})
        )),
        json!([pinned["id"]])
    );
}

#[test]
fn committed_activity_survives_reopen_and_counts_task_completion_once() {
    let mut f = Fixture::new(false);
    let note = f.page("Activity note", "one two");
    let edited=f.call("update_item",json!({"id":note["id"],"version":note["version"],"meta":note["meta"],"content":page_content("one two three four")}));
    let now = Utc::now();
    let day = now.format("%Y-%m-%d").to_string();
    let task = f.call(
        "mutate_note_task",
        json!({"noteId":note["id"],"change":{"kind":"create","text":"A daily goal"}}),
    );
    f.call("mutate_note_task",json!({"noteId":note["id"],"change":{"kind":"complete","taskId":task["id"],"revision":task["version"]}}));
    assert_eq!(
        f.call("read_item", json!({"id":note["id"]}))["version"],
        edited["version"]
    );
    let bounds = json!({"startDate":day,"endDate":day});
    let recovered = f.call("read_dashboard_analytics", bounds.clone());
    f.reopen(true);
    let response = f.call("read_dashboard_analytics", bounds);
    assert_eq!(response, recovered);
    for (key, value) in [
        ("wordsAdded", 4),
        ("wordsDeleted", 0),
        ("netWordChange", 4),
        ("tasksCompleted", 1),
    ] {
        assert_eq!(response["days"][0][key], value);
    }
    assert_eq!(response["summary"]["currentStreak"], 1);
    assert_eq!(response["summary"]["activeDays"], 1);
    let details = f.call("read_dashboard_day_details", json!({"date":day}));
    assert_eq!(ids(&details), json!([note["id"]]));
    assert_eq!(details["items"][0]["tasksCompleted"], 1);
    assert_eq!(
        f.call(
            "read_calendar_buckets",
            json!({"field":"completed","startDate":day,"endDate":day,"granularity":"month"})
        ),
        json!({"buckets":[{"date":&day[..7],"count":1}]})
    );
    f.error(
        "read_dashboard_analytics",
        json!({"startDate":"2026-02-30","endDate":day}),
        StatusCode::BAD_REQUEST,
    );
}

#[test]
fn paragraph_labels_retain_formatting_and_support_shared_classification_and_removal() {
    let f = Fixture::new(false);
    let first=f.call("create_item",json!({"meta":{"type":"page","title":"Source A"},"content":{"document": {"type":"doc","content":[{"type":"paragraph","attrs":{"labels":["bibliography_key","point_to_remember"]},"content":[{"type":"text","text":"A formatted "},{"type":"text","text":"citation","marks":[{"type":"strong"}]}]},{"type":"paragraph","attrs":{"labels":["point_to_remember"]},"content":[{"type":"text","text":"Another point"}]},{"type":"box","attrs":{"kind":"quote"},"content":[{"type":"paragraph","attrs":{"labels":["quoted_section"]},"content":[{"type":"text","text":"Quoted paragraph"}]}]}]}}}));
    for offset in [0, 1] {
        let points = f.call(
            "query_label_passages",
            json!({"label":"point_to_remember","limit":1,"offset":offset}),
        );
        assert_eq!(points["count"], 2);
        assert_eq!(points["items"].as_array().unwrap().len(), 1);
    }
    let classified = f.call("query_label_passages", json!({"label":"bibliography_key"}));
    assert_eq!(classified["count"], 1);
    assert_eq!(classified["items"][0]["itemId"], first["id"]);
    assert!(
        classified["items"][0]["fragment"]
            .to_string()
            .contains("\"type\":\"strong\"")
    );
    uuid::Uuid::parse_str(classified["items"][0]["blockId"].as_str().unwrap()).unwrap();
    assert_eq!(
        f.call("query_label_passages", json!({"label":"quoted_section"}))["count"],
        1
    );
    let second=f.call("create_item",json!({"meta":{"type":"page","title":"Source B"},"content":{"document": {"type":"doc","content":[{"type":"paragraph","attrs":{"labels":["bibliography_key"]},"content":[{"type":"text","text":"Competing reference"}]}]}}}));
    let shared = f.call("query_label_passages", json!({"label":"bibliography_key"}));
    assert_eq!(shared["count"], 2);
    assert_eq!(shared["items"].as_array().unwrap().len(), 2);
    f.call("update_item",json!({"id":first["id"],"version":first["version"],"meta":first["meta"],"content":page_content("No key here")}));
    let remaining = f.call("query_label_passages", json!({"label":"bibliography_key"}));
    assert_eq!(remaining["count"], 1);
    assert_eq!(remaining["items"][0]["itemId"], second["id"]);
}
