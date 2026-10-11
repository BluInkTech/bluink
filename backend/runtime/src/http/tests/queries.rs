// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use super::support::*;
use axum::http::StatusCode;
use chrono::Utc;
use serde_json::{Value, json};
use std::io::Write;

#[test]
fn projection_failure_larger_than_the_bounded_queue_joins_and_releases_ownership() {
    let mut f = Fixture::new(false);
    for _ in 0..300 {
        let path = f.item_path(&json!(uuid::Uuid::new_v4().to_string()));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, "{}").unwrap();
    }
    let error = f.runtime.block_on(f.app.initialize(true)).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("unsupported or mismatched item file")
    );
    f.runtime.block_on(f.app.stop()).unwrap();
    std::fs::remove_dir_all(f.root.path().join("items")).unwrap();
    f.reopen(true);
    let status: Value = serde_json::from_str(&f.app.status().unwrap()).unwrap();
    assert_eq!(status["index"]["documents"], 0);
}

#[test]
fn query_projection_preserves_previews_progress_and_calendar_and_reconciles_warm_starts_and_deletions()
 {
    let mut f = Fixture::new(false);
    let item=f.call("create_item",json!({"meta":{"type":"page","title":"Native search"},"content":{"type":"doc","content":[{"type":"paragraph","content":[{"type":"text","text":"A searchable portable document"}]},{"type":"image","attrs":{"src":"/assets/cover.jpg"}}]}}));
    f.initialize_index();
    let status: Value = serde_json::from_str(&f.app.status().unwrap()).unwrap();
    let status = &status["index"];
    assert_eq!(status["state"], "ready");
    assert_eq!(status["phase"], "Indexing complete");
    assert_eq!(status["processed"], 1);
    assert_eq!(status["total"], 1);
    let initial = f.call("query_items", json!({"query":"searchable"}));
    assert_eq!(initial["count"], 1);
    assert_eq!(initial["items"][0]["id"], item["id"]);
    assert_eq!(
        initial["items"][0]["preview"],
        "A searchable portable document"
    );
    assert_eq!(initial["items"][0]["previewImage"], "/assets/cover.jpg");
    let date = &item["createdAt"].as_str().unwrap()[..10];
    assert_eq!(f.call("read_calendar_buckets",json!({"query":"","field":"created","startDate":date,"endDate":date,"granularity":"day"})),json!({"buckets":[{"date":date,"count":1}]}));
    f.call("update_item",json!({"id":item["id"],"version":item["version"],"meta":item["meta"],"content":document("Externally changed content")}));
    f.settled();
    assert_eq!(
        f.call("query_items", json!({"query":"externally"}))["items"][0]["id"],
        item["id"]
    );
    f.reopen(true);
    let status: Value = serde_json::from_str(&f.app.status().unwrap()).unwrap();
    let status = &status["index"];
    assert_eq!(status["state"], "ready");
    assert_eq!(status["checked"], 1);
    assert_eq!(status["changed"], 0);
    assert_eq!(status["phase"], "Project cache is current");
    assert_eq!(status["reconciliationTimings"]["pinScopeMs"], 0.0);
    for key in ["fileStampMs", "indexedStampMs"] {
        assert!(status["reconciliationTimings"][key].is_number());
    }
    assert_eq!(
        f.call("query_items", json!({"query":"externally"}))["items"][0]["id"],
        item["id"]
    );
    f.runtime.block_on(f.app.stop()).unwrap();
    std::fs::remove_file(f.item_path(&item["id"])).unwrap();
    f.reopen(true);
    let status: Value = serde_json::from_str(&f.app.status().unwrap()).unwrap();
    let status = &status["index"];
    assert_eq!(status["state"], "ready");
    assert_eq!(status["checked"], 0);
    assert_eq!(status["changed"], 1);
    assert_eq!(status["documents"], 0);
    assert_eq!(
        f.call("query_items", json!({"query":"+type:page"}))["count"],
        0
    );
}

#[test]
fn task_index_keeps_page_owned_tasks_when_prose_is_replaced() {
    let f = Fixture::new(false);
    let open = "55555555-5555-4555-8555-555555555555";
    let orphan = "66666666-6666-4666-8666-666666666666";
    let created = "2026-09-01T00:00:00.000Z";
    let note=f.call("create_item",json!({"meta":{"type":"page","title":"Task host"},"content":{"type":"doc","content":[{"type":"paragraph","attrs":{"id":"44444444-4444-4444-8444-444444444444"},"content":[{"type":"text","text":"Draft"}]}]},"tasks":[{"id":open,"text":"Review","createdAt":created},{"id":orphan,"text":"Find source","createdAt":created}]}));
    f.initialize_index();
    let initial = f.call(
        "query_note_tasks",
        json!({"query":"+type:page","status":"open","limit":1,"offset":0}),
    );
    assert_eq!(initial["count"], 2);
    assert_eq!(initial["items"].as_array().unwrap().len(), 1);
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
    assert_eq!(
        tasks,
        vec![
            (open, note["id"].as_str().unwrap(), "Review"),
            (orphan, note["id"].as_str().unwrap(), "Find source")
        ]
    );
    assert_eq!(
        f.call(
            "query_note_tasks",
            json!({"query":"+tag:missing","status":"open"})
        )["count"],
        0
    );
    f.call("update_item",json!({"id":note["id"],"version":note["version"],"meta":note["meta"],"content":document("Replaced all prose"),"tasks":[{"id":open,"text":"Review","createdAt":created,"completedAt":Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)}]}));
    f.settled();
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
    let source=f.call("create_item",json!({"meta":{"type":"page","title":"Source","author":"Roberto Bolaño"},"tags":["research_methods"],"content":{"type":"doc","content":[{"type":"paragraph","content":[{"type":"text","text":"Tagged source","marks":[{"type":"link","attrs":{"href":format!("/?note={}",target["id"].as_str().unwrap())}}]}]}]}}));
    f.initialize_index();
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
    assert_eq!(
        &fields.as_array().unwrap()[..2],
        &[
            json!({"key":"summary","label":"Summary","valueType":"text","source":"builtin"}),
            json!({"key":"author","label":"Author","valueType":"text","source":"registered"})
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
        "+tag:research_methods",
        "author:\"Roberto Bolaño\"",
        "+tag:research_methods sort:modified-desc",
    ] {
        let result = f.call("query_items", json!({"query":query}));
        assert_eq!(result["items"].as_array().unwrap().len(), 1);
        assert_eq!(result["items"][0]["id"], source["id"]);
    }
    assert_eq!(
        f.call("query_items", json!({"query":"+tag:missing"}))["items"],
        json!([])
    );
    let mut ids = f.call("query_items", json!({"query":"+type:page"}))["items"]
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
            "analyze_query",
            json!({"query":"author:\"Roberto Bolaño\""})
        )["valid"],
        true
    );
    let invalid = f.call("analyze_query", json!({"query":"unknown_property:value"}));
    assert_eq!(invalid["valid"], false);
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
    let beta=f.call("create_item",json!({"meta":{"type":"page","title":"Beta","genre":"science_fiction","rating":2.5},"content":document("Beta")}));
    let alpha=f.call("create_item",json!({"meta":{"type":"page","title":"Alpha","genre":"biography","rating":4.5},"content":document("Alpha")}));
    let missing=f.call("create_item",json!({"meta":{"type":"page","title":"Missing","rating":3.5},"content":document("Missing")}));
    f.initialize_index();
    for query in [
        "+type:page sort:genre-asc",
        "sort:rating-desc +type:page sort:title-asc",
    ] {
        assert_eq!(
            ids(&f.call("query_items", json!({"query":query,"limit":3}))),
            json!([alpha["id"], beta["id"], missing["id"]])
        );
    }
    let page = f.call(
        "query_items",
        json!({"query":"+type:page sort:rating-desc","limit":1,"offset":1}),
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
fn direct_pins_are_applied_before_sorted_paging() {
    let f = Fixture::new(false);
    let view = f.call(
        "create_item",
        json!({"meta":{"type":"workspace","title":"Pinned view"},"content":{"mode":"dashboard"}}),
    );
    let alpha = f.page("Alpha", "Alpha");
    let pinned=f.call("create_item",json!({"meta":{"type":"page","title":"Zulu"},"content":document("Pinned Zulu"),"pins":[view["id"]]}));
    let beta = f.page("Beta", "Beta");
    f.initialize_index();
    let first = f.call(
        "query_items",
        json!({"query":"+type:page sort:title-asc","limit":2,"offset":0,"pinScopeId":view["id"]}),
    );
    assert_eq!(first["pinnedCount"], 1);
    assert_eq!(ids(&first), json!([pinned["id"], alpha["id"]]));
    assert_eq!(ids(&f.call("query_items",json!({"query":"+type:page sort:title-asc","limit":2,"offset":2,"pinScopeId":view["id"]}))),json!([beta["id"]]));
    assert_eq!(
        ids(&f.call(
            "query_items",
            json!({"query":"+type:page +is:pinned","limit":10,"offset":0,"pinScopeId":view["id"]})
        )),
        json!([pinned["id"]])
    );
}

#[test]
fn pending_activity_journals_recover_exactly_once_and_aggregate_task_completion() {
    let mut f = Fixture::new(false);
    let note = f.page("Activity note", "one two");
    let edited=f.call("update_item",json!({"id":note["id"],"version":note["version"],"meta":note["meta"],"content":document("one two three four")}));
    let now = Utc::now();
    let day = now.format("%Y-%m-%d").to_string();
    let completed=f.call("update_item",json!({"id":note["id"],"version":edited["version"],"meta":note["meta"],"content":document("one two three four"),"tasks":[{"id":"33333333-3333-4333-8333-333333333333","text":"A daily goal","createdAt":now.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),"completedAt":now.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)}]}));
    let event = json!({"state":"pending","event":{"id":format!("{}:{}:recovered",completed["id"].as_str().unwrap(),completed["version"].as_str().unwrap()),"date":day,"occurredAt":now.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),"itemId":completed["id"],"revision":completed["version"],"kind":"task_completed","wordsAdded":0,"wordsDeleted":0,"netWordChange":0}});
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(f.root.path().join(format!("analytics/events/{day}.jsonl")))
        .unwrap();
    writeln!(file, "{event}").unwrap();
    drop(file);
    f.initialize_index();
    let bounds = json!({"startDate":day,"endDate":day});
    let recovered = f.call("read_dashboard_analytics", bounds.clone());
    f.reopen(true);
    let response = f.call("read_dashboard_analytics", bounds);
    assert_eq!(response, recovered);
    for (key, value) in [
        ("wordsAdded", 4),
        ("wordsDeleted", 0),
        ("netWordChange", 4),
        ("tasksCompleted", 2),
    ] {
        assert_eq!(response["days"][0][key], value);
    }
    assert_eq!(response["summary"]["currentStreak"], 1);
    assert_eq!(response["summary"]["activeDays"], 1);
    let details = f.call("read_dashboard_day_details", json!({"date":day}));
    assert_eq!(ids(&details), json!([note["id"]]));
    assert_eq!(details["items"][0]["tasksCompleted"], 2);
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
    let first=f.call("create_item",json!({"meta":{"type":"page","title":"Source A"},"content":{"type":"doc","content":[{"type":"paragraph","attrs":{"labels":["bibliography_key","point_to_remember"]},"content":[{"type":"text","text":"A formatted "},{"type":"text","text":"citation","marks":[{"type":"strong"}]}]},{"type":"paragraph","attrs":{"labels":["point_to_remember"]},"content":[{"type":"text","text":"Another point"}]},{"type":"blockquote","content":[{"type":"paragraph","attrs":{"labels":["quoted_section"]},"content":[{"type":"text","text":"Quoted paragraph"}]}]}]}}));
    f.initialize_index();
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
    let second=f.call("create_item",json!({"meta":{"type":"page","title":"Source B"},"content":{"type":"doc","content":[{"type":"paragraph","attrs":{"labels":["bibliography_key"]},"content":[{"type":"text","text":"Competing reference"}]}]}}));
    f.settled();
    let shared = f.call("query_label_passages", json!({"label":"bibliography_key"}));
    assert_eq!(shared["count"], 2);
    assert_eq!(shared["items"].as_array().unwrap().len(), 2);
    f.call("update_item",json!({"id":first["id"],"version":first["version"],"meta":first["meta"],"content":document("No key here")}));
    f.settled();
    let remaining = f.call("query_label_passages", json!({"label":"bibliography_key"}));
    assert_eq!(remaining["count"], 1);
    assert_eq!(remaining["items"][0]["itemId"], second["id"]);
}
