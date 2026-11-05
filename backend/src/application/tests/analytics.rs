// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use super::*;
use crate::structured_query::fixtures::{criteria, field, tag};

/// A page's content holding one paragraph of text.
fn page_content(text: &str) -> Value {
    json!({"document":{"type":"doc","content":[{"type":"paragraph","content":[{"type":"text","text":text}]}]}})
}

#[test]
fn analytics_reports_have_typed_shapes_and_project_local_defaults() {
    let project = TestProject::new("analytics-defaults");
    let app = project.application();
    app.initialize_sync().unwrap();
    let report = execute(&app, "read_dashboard_analytics", json!({})).unwrap();
    assert_eq!(report["days"].as_array().unwrap().len(), 365);
    assert_eq!(
        report["days"].as_array().unwrap().last().unwrap()["date"],
        app.activity_date().unwrap()
    );
    assert_eq!(
        report["summary"],
        json!({"activeDays":0,"wordsAdded":0,"wordsDeleted":0,"netWordChange":0,"editingEvents":0,"tasksCompleted":0,"currentStreak":0,"bestStreak":0})
    );
    assert_eq!(
        execute(
            &app,
            "read_dashboard_day_details",
            json!({"date":"2026-10-25"})
        )
        .unwrap(),
        json!({"date":"2026-10-25","items":[]})
    );
    for (operation, body) in [
        (
            "read_dashboard_analytics",
            json!({"startDate":"2026-02-30"}),
        ),
        ("read_dashboard_analytics", json!({"endDate":"bad"})),
        (
            "read_dashboard_analytics",
            json!({"startDate":"2026-10-26","endDate":"2026-10-25"}),
        ),
        ("read_dashboard_day_details", json!({"date":"2026-02-30"})),
        ("read_dashboard_day_details", json!({})),
    ] {
        assert!(
            matches!(
                execute(&app, operation, body),
                Err(ApplicationError::InvalidRequest(_))
            ),
            "{operation}"
        );
    }
    let scoped = execute(
        &app,
        "read_dashboard_analytics",
        json!({"query":criteria([field("type", "page")])}),
    )
    .unwrap();
    assert_eq!(scoped["summary"], report["summary"]);
    app.stop_sync().unwrap();
    project.close().unwrap();
}

#[test]
fn analytics_scopes_preserve_dense_calendar_days_after_rejected_activity_update() {
    let project = TestProject::new("analytics-scopes");
    let app = project.application();
    app.initialize_sync().unwrap();
    let first = crate::application::item_writes::write_item_input(&app.root,
        json!({"meta":{"type":"page","title":"First"},"tags":["research"],"content":page_content("one two")}),
        None, "2026-10-25").unwrap()["item"].clone();
    let second = crate::application::item_writes::write_item_input(&app.root,
        json!({"meta":{"type":"page","title":"Second"},"tags":["other"],"content":page_content("three four five")}),
        None, "2026-10-25").unwrap()["item"].clone();
    crate::application::item_writes::write_item_input(&app.root,
        json!({"id":first["id"],"meta":first["meta"],"tags":["research"],"content":page_content("one two three four")}),
        first["revision"].as_str(), "2026-10-26").unwrap();
    app.initialize_sync().unwrap();
    let request = json!({"startDate":"2026-10-24","endDate":"2026-10-26"});
    let all = execute(&app, "read_dashboard_analytics", request.clone()).unwrap();
    assert_eq!(all["days"].as_array().unwrap().len(), 3);
    assert_eq!(all["days"][0]["wordsAdded"], 0);
    assert_eq!(all["days"][1]["wordsAdded"], 5);
    assert_eq!(all["days"][2]["wordsAdded"], 2);
    assert_eq!(all["summary"]["wordsAdded"], 7);
    assert_eq!(all["summary"]["activeDays"], 2);
    assert_eq!(all["summary"]["currentStreak"], 2);
    assert_eq!(all["summary"]["bestStreak"], 2);
    let mut scope = request.clone();
    scope["query"] = json!(criteria([tag("research")]));
    assert_eq!(
        execute(&app, "read_dashboard_analytics", scope).unwrap()["summary"]["wordsAdded"],
        4
    );
    let details = execute(
        &app,
        "read_dashboard_day_details",
        json!({"date":"2026-10-25","query":criteria([tag("research")])}),
    )
    .unwrap();
    assert_eq!(details["items"].as_array().unwrap().len(), 1);
    assert_eq!(details["items"][0]["id"], first["id"]);
    assert_eq!(details["items"][0]["title"], "First");
    assert_eq!(details["items"][0]["wordsAdded"], 2);
    assert_eq!(
        execute(
            &app,
            "read_dashboard_day_details",
            json!({"date":"2026-10-25","query":criteria([tag("other")])})
        )
        .unwrap()["items"][0]["id"],
        second["id"]
    );
    let rejected = app.database().unwrap().write(|tx| {
        tx.execute("UPDATE activity_daily SET words_added=-1", [])?;
        Ok(())
    });
    assert!(rejected.is_err());
    assert_eq!(
        execute(&app, "read_dashboard_analytics", request).unwrap(),
        all
    );
    app.stop_sync().unwrap();
    project.close().unwrap();
}
