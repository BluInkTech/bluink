// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use super::*;

fn document(text: &str) -> Value {
    json!({"type":"doc","content":[{"type":"paragraph","content":[{"type":"text","text":text}]}]})
}

#[test]
fn analytics_reports_have_typed_shapes_and_project_local_defaults_without_an_index() {
    let project = TestProject::new("analytics-defaults");
    let app = project.application();
    app.initialize_sync(false).unwrap();
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
    assert!(app.state().unwrap().index.is_none());
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
    assert!(matches!(
        execute(
            &app,
            "read_dashboard_analytics",
            json!({"query":crate::query::document::test_query("+type:page")}),
        ),
        Err(ApplicationError::NotInitialized(
            "Initialize the search index before querying items."
        ))
    ));
    app.stop_sync().unwrap();
    project.close().unwrap();
}

#[test]
fn analytics_scopes_preserve_dense_calendar_days_and_fail_on_corrupt_activity() {
    let project = TestProject::new("analytics-scopes");
    let app = project.application();
    app.initialize_sync(false).unwrap();
    let first = crate::engine::project::write::write_item_input(&app.root,
        json!({"meta":{"type":"page","title":"First"},"tags":["research"],"content":document("one two")}),
        None, "2026-10-25").unwrap()["item"].clone();
    let second = crate::engine::project::write::write_item_input(&app.root,
        json!({"meta":{"type":"page","title":"Second"},"tags":["other"],"content":document("three four five")}),
        None, "2026-10-25").unwrap()["item"].clone();
    crate::engine::project::write::write_item_input(&app.root,
        json!({"id":first["id"],"meta":first["meta"],"tags":["research"],"content":document("one two three four")}),
        first["revision"].as_str(), "2026-10-26").unwrap();
    app.initialize_sync(true).unwrap();
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
    scope["query"] = json!(crate::query::document::test_query("#research"));
    assert_eq!(
        execute(&app, "read_dashboard_analytics", scope).unwrap()["summary"]["wordsAdded"],
        4
    );
    let details = execute(
        &app,
        "read_dashboard_day_details",
        json!({"date":"2026-10-25","query":crate::query::document::test_query("#research")}),
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
            json!({"date":"2026-10-25","query":crate::query::document::test_query("#other")})
        )
        .unwrap()["items"][0]["id"],
        second["id"]
    );
    let file = app.root.join("analytics/2026-10-25.json");
    let saved = std::fs::read(&file).unwrap();
    std::fs::write(&file, b"{").unwrap();
    for (operation, body) in [
        ("read_dashboard_analytics", request.clone()),
        ("read_dashboard_day_details", json!({"date":"2026-10-25"})),
    ] {
        assert!(
            matches!(
                execute(&app, operation, body),
                Err(ApplicationError::Internal(_))
            ),
            "{operation}"
        );
    }
    std::fs::write(file, saved).unwrap();
    assert_eq!(
        execute(&app, "read_dashboard_analytics", request).unwrap(),
        all
    );
    app.stop_sync().unwrap();
    project.close().unwrap();
}
