// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use super::*;
use crate::application::commands::{CommandRequest, CommandResult};

const HOME: &str = "00000000-0000-7000-8000-000000000501";
const CHAPTER: &str = "00000000-0000-7000-8000-000000000502";
#[expect(
    clippy::needless_pass_by_value,
    reason = "Test requests own fixture values just like decoded production requests."
)]
fn run(app: &Application, command: &str, action: &str, values: Value) -> Result<Value> {
    let result = app.execute_command(
        command,
        CommandRequest {
            positionals: vec![action.to_owned()],
            values: values.as_object().unwrap().clone(),
            cwd: app.root.clone(),
        },
    )?;
    serde_json::to_value(result).map_err(error::internal_error)
}

fn pages(app: &Application) {
    app.initialize_sync(false).unwrap();
    for (id, title) in [(HOME, "Home"), (CHAPTER, "Chapter")] {
        crate::engine::project::write::write_item_input(
            &app.root,
            json!({"id": id, "meta": {"type": "page", "title": title, "lang": "en"},
                "tags": ["fiction"], "content": {"type": "doc", "content": [{"type": "paragraph",
                    "content": [{"type": "text", "text": format!("{title} body.")}]}]}}),
            None,
            &app.activity_date().unwrap(),
        )
        .unwrap();
    }
}

fn publication(app: &Application, collection: bool) -> String {
    let input = json!({"title": "Command book", "content": {
        "kind": "publication", "preset": "book", "selectedOutput": "pdf",
        "rootPageId": HOME, "outline": if collection {
            json!([{"kind": "section", "id": "00000000-0000-7000-8000-000000000503", "title": "Chapters", "children": [
                {"kind": "collection", "id": "00000000-0000-7000-8000-000000000504", "title": "Fiction", "query": crate::query::document::test_query("#fiction")}
            ]}])
        } else { json!([{"kind": "page", "pageId": CHAPTER, "children": []}]) },
        "selectionRules": [], "themeId": "classic", "targets": {
            "pdf": {"profile": "default", "outsideLinks": "error"},
            "html": {"outsideLinks": "error"},"docx": {"outsideLinks": "error"},"epub":{"outsideLinks": "error"},
            "site": {"layout": "documentation", "outsideLinks": "error"}
        }
    }});
    std::fs::write(app.root.join("publication.json"), input.to_string()).unwrap();
    run(
        app,
        "publication",
        "create",
        json!({"input": "publication.json"}),
    )
    .unwrap()["value"]
        .as_str()
        .unwrap()
        .to_owned()
}

#[test]
fn standalone_commands_validate_and_plan_without_an_index_or_listener() {
    let project = TestProject::new("cli-note");
    let app = project.application();
    pages(&app);
    for format in ["pdf", "docx", "html"] {
        let plan = run(&app, "export", format, json!({"note": HOME, "check": true})).unwrap();
        assert_eq!(plan["value"]["pageId"], HOME);
        assert_eq!(plan["value"]["title"], "Home");
        assert!(app.state().unwrap().index.is_none());
    }
    for (format, values, message) in [
        (
            "html",
            json!({"note": HOME, "profile": "default", "check": true}),
            "--profile applies only to PDF",
        ),
        (
            "pdf",
            json!({"note": HOME, "profile": "bad", "check": true}),
            "--profile must be",
        ),
        (
            "pdf",
            json!({"note": HOME, "outside-links": "bad", "check": true}),
            "--outside-links must be",
        ),
        (
            "pdf",
            json!({"note": HOME, "trust-theme-sha256": "ABC", "check": true}),
            "lowercase hash",
        ),
        ("html", json!({"note": HOME}), "--out must specify"),
        (
            "html",
            json!({"note": "bad", "check": true}),
            "Invalid Bluink item id",
        ),
    ] {
        let error = run(&app, "export", format, values).unwrap_err();
        assert!(matches!(error, ApplicationError::InvalidRequest(_)));
        assert!(error.to_string().contains(message), "{error}");
    }
    app.stop_sync().unwrap();
    assert!(run(&app, "export", "html", json!({"note": HOME, "check": true})).is_err());
    project.close().unwrap();
}

#[test]
fn linked_commands_plan_every_format_without_an_index_or_listener() {
    let project = TestProject::new("cli-linked");
    let app = project.application();
    pages(&app);
    let page = crate::engine::project::item::read_project_item(&app.root, HOME)
        .unwrap()
        .unwrap();
    crate::engine::project::write::write_item_input(&app.root, json!({"id":HOME,"meta":{},"content":{"type":"doc","content":[{"type":"paragraph","content":[{"type":"text","text":"Chapter","marks":[{"type":"link","attrs":{"href":format!("/?note={CHAPTER}")}}]}]}]}}), page["revision"].as_str(), &app.activity_date().unwrap()).unwrap();
    for (format, target) in [("pdf", "pdf"), ("docx", "docx"), ("html", "site")] {
        let plan = run(
            &app,
            "export",
            format,
            json!({"note":HOME,"include-linked":true,"check":true}),
        )
        .unwrap();
        assert_eq!(plan["kind"], "publication_plan");
        assert_eq!(plan["value"]["sourceKind"], "linked_pages");
        assert_eq!(plan["value"]["format"], target);
        assert_eq!(plan["value"]["pages"].as_array().unwrap().len(), 2);
        assert!(app.state().unwrap().index.is_none());
    }
    app.stop_sync().unwrap();
    project.close().unwrap();
}

#[test]
fn static_publication_create_inspect_and_plan_share_canonical_rules_without_indexing() {
    let project = TestProject::new("cli-static-publication");
    let app = project.application();
    pages(&app);
    let id = publication(&app, false);
    let item = run(&app, "publication", "inspect", json!({"id": id})).unwrap();
    assert_eq!(item["value"]["meta"]["type"], "publication");
    assert!(item["value"]["revision"].is_string());
    for format in ["pdf", "docx", "html", "site"] {
        let plan = run(
            &app,
            "publication",
            "plan",
            json!({"id": id, "format": format}),
        )
        .unwrap();
        assert_eq!(plan["kind"], "publication_plan");
        assert_eq!(plan["value"]["pages"].as_array().unwrap().len(), 2);
        assert!(app.state().unwrap().index.is_none());
    }
    std::fs::write(
        app.root.join("invalid.json"),
        r#"{"title":"Invalid","content":{}}"#,
    )
    .unwrap();
    let before = crate::engine::project::item::list_project_item_ids(&app.root).unwrap();
    assert!(
        run(
            &app,
            "publication",
            "create",
            json!({"input": "invalid.json"})
        )
        .is_err()
    );
    assert_eq!(
        crate::engine::project::item::list_project_item_ids(&app.root).unwrap(),
        before
    );
    app.stop_sync().unwrap();
    project.close().unwrap();
}

#[test]
fn query_publication_uses_owned_index_and_reports_exclusion_until_owner_stops() {
    let project = TestProject::new("cli-query-publication");
    let app = project.application();
    pages(&app);
    let id = publication(&app, true);
    let first = run(
        &app,
        "publication",
        "plan",
        json!({"id": id, "format": "html"}),
    )
    .unwrap();
    let membership = &first["value"]["collections"][0]["resolvedIds"];
    assert_eq!(membership, &json!([CHAPTER, HOME]));
    let index = app.index_path().unwrap();
    let other = project.application();
    let error = run(
        &other,
        "publication",
        "plan",
        json!({"id": id, "format": "html"}),
    )
    .unwrap_err();
    assert!(error.to_string().contains("already"), "{error}");
    assert!(other.state().unwrap().index.is_none());
    app.stop_sync().unwrap();
    let next = run(
        &other,
        "publication",
        "plan",
        json!({"id": id, "format": "html"}),
    )
    .unwrap();
    assert_eq!(&next["value"]["collections"][0]["resolvedIds"], membership);
    assert_eq!(other.index_path().unwrap(), index);
    other.stop_sync().unwrap();
    project.close().unwrap();
}

#[test]
fn commands_do_not_create_missing_projects_and_hold_plans_typed() {
    let project = TestProject::new("cli-missing-project");
    let app = project.application();
    assert!(!app.root.exists());
    let error = run(&app, "export", "html", json!({"note": HOME, "check": true})).unwrap_err();
    assert!(error.to_string().contains("No Bluink project exists"));
    assert!(!app.root.exists());
    pages(&app);
    let result = app
        .execute_command(
            "export",
            CommandRequest {
                positionals: vec!["html".to_owned()],
                values: json!({"note": HOME, "check": true})
                    .as_object()
                    .unwrap()
                    .clone(),
                cwd: app.root.clone(),
            },
        )
        .unwrap();
    let CommandResult::PagePlan(plan) = result else {
        panic!("Expected a typed article plan")
    };
    let item = crate::engine::project::item::read_project_item(&app.root, HOME)
        .unwrap()
        .unwrap();
    crate::engine::project::write::write_item_input(
        &app.root,
        json!({"id": HOME, "meta": {"title": "Changed after planning"}}),
        item["revision"].as_str(),
        &app.activity_date().unwrap(),
    )
    .unwrap();
    let output = app.root.join("stale.html");
    let error = crate::export::render_article(
        &app.root,
        &plan,
        Some(&output),
        None,
        &app.export_resources(),
    )
    .err()
    .unwrap();
    assert!(error.to_string().contains("changed"), "{error}");
    assert!(!output.exists());
    app.stop_sync().unwrap();
    project.close().unwrap();
}
