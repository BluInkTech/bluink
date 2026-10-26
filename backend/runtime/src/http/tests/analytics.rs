// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use super::support::*;
use axum::http::StatusCode;
use chrono::Utc;
use serde_json::json;

#[test]
fn analytics_keep_date_defaults_query_scopes_and_storage_failure_classification() {
    let f = Fixture::new(false);
    let today = Utc::now().format("%Y-%m-%d").to_string();
    let empty = f.call("read_dashboard_analytics", json!({}));
    assert_eq!(empty["days"].as_array().unwrap().len(), 365);
    assert_eq!(
        empty["days"].as_array().unwrap().last().unwrap()["date"],
        today
    );
    assert_eq!(empty["summary"]["wordsAdded"], 0);
    assert_eq!(
        f.call("read_dashboard_day_details", json!({"date":today})),
        json!({"date":today,"items":[]})
    );
    for (name, body) in [
        (
            "read_dashboard_analytics",
            json!({"startDate":"2026-02-30"}),
        ),
        (
            "read_dashboard_analytics",
            json!({"startDate":"2026-10-26","endDate":"2026-10-25"}),
        ),
        ("read_dashboard_analytics", json!({"endDate":42})),
        ("read_dashboard_day_details", json!({})),
        ("read_dashboard_day_details", json!({"date":"../escape"})),
    ] {
        f.error(name, body, StatusCode::BAD_REQUEST);
    }
    let first=f.call("create_item",json!({"meta":{"type":"page","title":"First"},"tags":["research"],"content":document("one two three")}));
    f.call("create_item",json!({"meta":{"type":"page","title":"Second"},"tags":["other"],"content":document("four five")}));
    f.initialize_index();
    let bounds = json!({"startDate":today,"endDate":today});
    let all = f.call("read_dashboard_analytics", bounds.clone());
    assert_eq!(all["days"].as_array().unwrap().len(), 1);
    assert_eq!(all["days"][0]["wordsAdded"], 5);
    assert_eq!(all["days"][0]["notesTouched"], 2);
    for key in ["activeDays", "currentStreak", "bestStreak"] {
        assert_eq!(all["summary"][key], 1);
    }
    let scoped = f.call(
        "read_dashboard_analytics",
        json!({"startDate":today,"endDate":today,"query":crate::query::document::test_query("#research")}),
    );
    assert_eq!(scoped["summary"]["wordsAdded"], 3);
    assert_eq!(scoped["days"][0]["notesTouched"], 1);
    assert_eq!(
        f.call(
            "read_dashboard_day_details",
            json!({"date":today,"query":crate::query::document::test_query("#research")})
        )["items"],
        json!([{"id":first["id"],"title":"First","wordsAdded":3,"wordsDeleted":0,"netWordChange":3,"editingEvents":1,"tasksCompleted":0}])
    );
    assert_eq!(
        f.call(
            "read_dashboard_day_details",
            json!({"date":today,"query":crate::query::document::test_query("#missing")})
        ),
        json!({"date":today,"items":[]})
    );
    let path = f.root.path().join(format!("analytics/{today}.json"));
    let saved = std::fs::read(&path).unwrap();
    std::fs::write(&path, "{").unwrap();
    f.error(
        "read_dashboard_analytics",
        bounds.clone(),
        StatusCode::INTERNAL_SERVER_ERROR,
    );
    f.error(
        "read_dashboard_day_details",
        json!({"date":today}),
        StatusCode::INTERNAL_SERVER_ERROR,
    );
    std::fs::write(path, saved).unwrap();
    assert_eq!(f.call("read_dashboard_analytics", bounds), all);
}
