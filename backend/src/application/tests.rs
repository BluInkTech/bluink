// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use crate::structured_query::fixtures::{criteria, desc, field, term};

mod analytics;
mod catalogue;
mod catalogue_writes;
mod commands;
mod exports;
mod history;
mod history_session;
mod item_writes;
mod publication_consistency;
mod publication_contract;
mod settings;
mod setups;
mod shutdown;
mod snippets;

use super::*;
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

/// Declare before applications so panic unwinding drops workers before their files.
struct TestProject(tempfile::TempDir);

impl TestProject {
    fn new(name: &str) -> Self {
        Self(
            tempfile::Builder::new()
                .prefix(&format!("bluink-{name}-"))
                .tempdir()
                .unwrap(),
        )
    }

    fn application(&self) -> Application {
        let root = self.0.path().join("project");
        Application::new(ApplicationOptions {
            workspace_template_path: concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/resources/workspace-starter.json"
            )
            .into(),
            project_root: root.to_str().unwrap().to_owned(),
            user_data_dir: Some(
                std::path::PathBuf::from(root.to_str().unwrap().to_owned())
                    .join(".cache/test-user")
                    .to_string_lossy()
                    .into_owned(),
            ),
            timezone: Some("Europe/London".to_owned()),
            public_dir: None,
            web_font_folder: None,
            pdf_font_folder: None,
            docx_font_folder: None,
            typst_binary: None,
        })
        .unwrap()
    }

    /// Successful tests explicitly stop their applications before checking cleanup.
    fn close(self) -> std::io::Result<()> {
        self.0.close()
    }
}

#[test]
fn initialization_and_shutdown_update_application_lifecycle() {
    let project = TestProject::new("direct");
    let app = project.application();
    app.initialize_sync().unwrap();
    assert_eq!(app.status().unwrap()["ready"], true);
    assert_eq!(app.activity_date().unwrap().len(), 10);
    app.stop_sync().unwrap();
    app.stop_sync().unwrap();
    assert!(app.initialize_sync().is_err());
    project.close().unwrap();
}

#[test]
fn runtime_workspace_preset_is_shared_by_bootstrap_and_new_workspaces() {
    let project = TestProject::new("runtime-workspace-preset");
    let mut template = crate::domain::project::workspace::initial_content();
    template["widgets"] = json!([{
        "id": uuid::Uuid::now_v7().to_string(),
        "title": "My reading desk",
        "topBar": true,
        "home": {"span": 12, "hidden": true},
        "config": {"kind": "layout", "mode": "table", "query": {"version": 1, "clauses": [], "sort": []}},
    }]);
    let template_path = project.0.path().join("starter.json");
    std::fs::write(&template_path, serde_json::to_vec(&template).unwrap()).unwrap();
    let mut options = project.application().options.clone();
    options.workspace_template_path = template_path.to_string_lossy().into_owned();
    let app = Application::new(options).unwrap();
    app.initialize_sync().unwrap();
    let main = crate::domain::project::config::read_default_workspace(&app.root).unwrap();
    let created = execute(
        &app,
        "create_item",
        json!({"meta":{"type":"workspace","title":"Reading"}}),
    )
    .unwrap();
    for workspace in [&main, &created] {
        let widgets = workspace["content"]["widgets"].as_array().unwrap();
        assert_eq!(widgets.len(), 1);
        assert_eq!(widgets[0]["title"], "My reading desk");
    }
    assert_ne!(
        main["content"]["widgets"][0]["id"],
        created["content"]["widgets"][0]["id"]
    );
    app.stop_sync().unwrap();
    project.close().unwrap();
}

#[test]
fn request_timing_accounts_for_application_admission_waits() {
    let project = TestProject::new("admission-timing");
    let app = Arc::new(project.application());
    app.initialize_sync().unwrap();
    let gate = app.operations.write().unwrap();
    let worker = Arc::clone(&app);
    let (started, ready) = std::sync::mpsc::channel();
    let (finished, result) = std::sync::mpsc::channel();
    let thread = std::thread::spawn(move || {
        let started_at = Instant::now();
        let (response, waits) = crate::diagnostics::waits::capture(|| {
            started.send(()).unwrap();
            worker.execute(operations::Request::decode("read_metadata_fields", b"{}").unwrap())
        });
        finished
            .send((response, waits, started_at.elapsed()))
            .unwrap();
    });
    ready.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(result.recv_timeout(Duration::from_millis(30)).is_err());
    drop(gate);
    let (response, waits, elapsed) = result.recv_timeout(Duration::from_secs(5)).unwrap();
    response.unwrap();
    assert!(waits.admission > Duration::ZERO);
    assert!(waits.admission <= elapsed);
    assert_eq!(waits.sqlite_write, Duration::ZERO);
    thread.join().unwrap();
    app.stop_sync().unwrap();
    project.close().unwrap();
}

#[test]
fn simultaneous_project_handles_observe_the_same_committed_revision() {
    let project = TestProject::new("shared-database");
    let first = project.application();
    let second = project.application();
    first.initialize_sync().unwrap();
    second.initialize_sync().unwrap();
    let page=execute(&first,"create_item",json!({"meta":{"type":"page","title":"Shared"},"content":{"document":{"type":"doc","content":[{"type":"paragraph"}]}}})).unwrap();
    assert_eq!(
        execute(&second, "read_item", json!({"id":page["id"]})).unwrap()["version"],
        page["version"]
    );
    first.stop_sync().unwrap();
    assert_eq!(
        execute(&second, "read_item", json!({"id":page["id"]})).unwrap()["meta"]["title"],
        "Shared"
    );
    second.stop_sync().unwrap();
    project.close().unwrap();
}

#[test]
fn shutdown_rejects_new_operations_and_waits_for_accepted_work() {
    let project = TestProject::new("operation-drain");
    let app = Arc::new(project.application());
    app.initialize_sync().unwrap();
    let accepted = app.begin_operation().unwrap();
    let (finished, completion) = std::sync::mpsc::channel();
    let stopping = Arc::clone(&app);
    let worker = std::thread::spawn(move || {
        stopping.stop_sync().unwrap();
        finished.send(()).unwrap();
    });
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while !app.stopping.load(Ordering::Acquire) {
        assert!(std::time::Instant::now() < deadline);
        std::thread::yield_now();
    }
    assert!(matches!(
        app.begin_operation(),
        Err(ApplicationError::ShuttingDown)
    ));
    assert!(matches!(
        completion.try_recv(),
        Err(std::sync::mpsc::TryRecvError::Empty)
    ));
    drop(accepted);
    completion.recv_timeout(Duration::from_secs(5)).unwrap();
    worker.join().unwrap();
    assert_eq!(app.status().unwrap()["phase"], "Stopped");
    project.close().unwrap();
}

#[test]
fn item_operations_keep_two_project_handles_isolated_without_an_index() {
    let first_project = TestProject::new("read-isolation-first");
    let first = first_project.application();
    let second_project = TestProject::new("read-isolation-second");
    let second = second_project.application();
    let id = "00000000-0000-7000-8000-000000000212";
    for (app, title) in [(&first, "First project"), (&second, "Second project")] {
        app.initialize_sync().unwrap();
        crate::application::item_writes::write_item_input(
            &app.root,
            json!({"id": id, "meta": {"type": "page", "title": title}, "content": {"document": {"type": "doc", "content": [{"type": "paragraph"}]}}}),
            None, &app.activity_date().unwrap(),
        ).unwrap();
        let value = app
            .read_item(request::decode(format!(r#"{{"id":"{id}"}}"#).as_bytes()).unwrap())
            .unwrap();
        assert_eq!(value.meta.title.as_deref().unwrap(), title);
        assert!(app.database().is_ok());
    }
    first.stop_sync().unwrap();
    assert!(
        first
            .read_item(request::decode(format!(r#"{{"id":"{id}"}}"#).as_bytes()).unwrap())
            .is_err()
    );
    let value = second
        .read_item(request::decode(format!(r#"{{"id":"{id}"}}"#).as_bytes()).unwrap())
        .unwrap();
    assert_eq!(value.meta.title.as_deref().unwrap(), "Second project");
    second.stop_sync().unwrap();
    first_project.close().unwrap();
    second_project.close().unwrap();
}
#[expect(
    clippy::needless_pass_by_value,
    reason = "Test requests own fixture values just like decoded production requests."
)]
fn execute(app: &Application, name: &str, body: Value) -> Result<Value> {
    let request = operations::Request::decode(name, &serde_json::to_vec(&body).unwrap())?;
    serde_json::to_value(app.execute(request)?).map_err(error::internal_error)
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "Keep each end-to-end scenario together so setup, failure checks and cleanup remain visible."
)]
fn application_mutations_preserve_revisions_and_canonical_fields_without_an_index() {
    let project = TestProject::new("mutation-contract");
    let app = project.application();
    app.initialize_sync().unwrap();
    let created = execute(
        &app,
        "create_item",
        json!({
            "meta": {"type": "page", "title": "Original"},
            "tags": ["research"],
            "content": {"document": {"type": "doc", "content": [{"type": "paragraph",
                "content": [{"type": "text", "text": "Original prose"}]}]}}
        }),
    )
    .unwrap();
    let id = created["id"].as_str().unwrap();
    assert!(created["revision"].is_null());
    assert!(created["version"].is_string());
    assert!(created["analytics"].is_null());
    let changed = execute(
        &app,
        "update_item",
        json!({
            "id": id, "version": created["version"],
            "meta": {"title": "Changed"}, "content": created["content"]
        }),
    )
    .unwrap();
    assert_eq!(changed["tags"], json!(["research"]));
    assert!(matches!(
        execute(
            &app,
            "update_item",
            json!({
                "id": id, "version": created["version"],
                "meta": {"title": "Lost update"}, "content": created["content"]
            })
        ),
        Err(ApplicationError::InvalidRequest(_))
    ));
    let meta = execute(
        &app,
        "update_item_meta",
        json!({
            "id": id, "version": changed["version"], "meta": {"title": "Metadata"}
        }),
    )
    .unwrap();
    assert_eq!(meta["content"], json!({}));
    assert_eq!(meta["tags"], created["tags"]);
    let tagged = execute(
        &app,
        "set_item_tags",
        json!({
            "id": id, "version": meta["version"], "tags": ["changed"]
        }),
    )
    .unwrap();
    assert_eq!(tagged["content"], created["content"]);
    assert_eq!(tagged["meta"]["title"], "Metadata");
    let task = execute(
        &app,
        "mutate_note_task",
        json!({
            "noteId": id, "version": tagged["version"],
            "change": {"kind": "create", "text": "Review this"}
        }),
    )
    .unwrap();
    assert_eq!(task["content"]["text"], "Review this");
    let pin = execute(
        &app,
        "set_note_pin",
        json!({
            "noteId": id, "version": tagged["version"], "pinned": true,
            "scopeId":execute(&app,"read_workspace_context",json!({})).unwrap()["defaultWorkspace"]["id"]
        }),
    )
    .unwrap();
    assert_eq!(pin["pinned"], true);
    assert!(pin["version"].is_string());
    assert_eq!(
        execute(&app, "delete_item", json!({"id": id})).unwrap(),
        json!({"deleted": true})
    );
    let canonical = crate::store::snapshots::read_project_item(&app.root, id)
        .unwrap()
        .unwrap();
    assert_eq!(canonical["meta"]["state"], "archived");
    assert_eq!(canonical["content"], created["content"]);
    assert!(canonical.get("tasks").is_none());
    assert_eq!(
        crate::store::snapshots::read_project_item(&app.root, task["id"].as_str().unwrap())
            .unwrap()
            .unwrap()["meta"]["state"],
        "archived"
    );
    let goal = json!({"scope": "all_notes", "startDate": "2026-10-01",
        "endDate": "2026-10-31", "dailyWordTarget": 500});
    assert_eq!(
        execute(&app, "create_writing_goal", goal.clone()).unwrap()["meta"]["type"],
        "goal"
    );
    assert!(matches!(
        execute(&app, "create_writing_goal", goal),
        Err(ApplicationError::InvalidRequest(_))
    ));
    assert!(app.database().is_ok());
    app.stop_sync().unwrap();
    assert!(execute(&app, "create_item", json!({})).is_err());
    project.close().unwrap();
}

#[test]
fn typed_queries_observe_commits_and_preserve_domain_errors() {
    let project = TestProject::new("typed-queries");
    let app = project.application();
    app.initialize_sync().unwrap();
    let id = "00000000-0000-7000-8000-000000000211";
    let first = crate::application::item_writes::write_item_input(
        &app.root,
        json!({
            "id": id,
            "meta": {"type": "page", "title": "Typed query fixture"},
            "tags": ["research"],
            "content": {"document": {"type": "doc", "content": [{
                "type": "paragraph", "attrs": {"labels": ["review"]},
                "content": [{"type": "text", "text": "Initial searchable paragraph"}]
            }]}}
        }),
        None,
        &app.activity_date().unwrap(),
    )
    .unwrap();
    crate::application::item_writes::mutate_note_task(
        &app.database().unwrap(),
        id,
        &json!({"kind": "create", "text": "Check the fixture"}),
        &app.activity_date().unwrap(),
    )
    .unwrap();
    let direct = app
        .read_item(request::decode(format!(r#"{{"id":"{id}"}}"#).as_bytes()).unwrap())
        .unwrap();
    assert_eq!(direct.version, first["item"]["revision"]);
    assert_eq!(direct.content["document"]["type"], "doc");
    let compact = app
        .read_items(request::decode(format!(r#"{{"ids":["{id}"]}}"#).as_bytes()).unwrap())
        .unwrap();
    assert!(compact[0].content.is_empty());
    let found = execute(
        &app,
        "query_items",
        json!({"query":criteria([term("searchable")])}),
    )
    .unwrap();
    assert_eq!(found["count"], 1);
    assert_eq!(found["items"][0]["id"], id);
    assert_eq!(
        execute(&app, "query_note_tasks", json!({"status":"open"})).unwrap()["count"],
        1
    );
    assert_eq!(
        execute(&app, "query_label_passages", json!({"label":"review"})).unwrap()["items"][0]["text"],
        "Initial searchable paragraph"
    );
    let invalid = execute(
        &app,
        "query_items",
        json!({"query":criteria([field("unregistered", "value")])}),
    )
    .unwrap_err();
    assert!(matches!(invalid, ApplicationError::InvalidQuery(_)));
    app.stop_sync().unwrap();
    project.close().unwrap();
}

#[test]
fn empty_application_queries_return_empty_results_and_keep_validation() {
    let project = TestProject::new("empty-queries");
    let app = project.application();
    app.initialize_sync().unwrap();
    for field in ["created", "modified", "meta.date", "completed"] {
        let result = execute(
            &app,
            "read_calendar_buckets",
            json!({
                "field": field, "startDate": "2026-10-01", "endDate": "2026-10-31",
                "query":crate::structured_query::document::QueryDocument::active_pages(),
                "granularity": "day"
            }),
        )
        .unwrap();
        assert_eq!(result, json!({"buckets": []}));
    }
    for (name, body, expected) in [
        (
            "query_note_tasks",
            json!({"status": "open"}),
            json!({"items": [], "count": 0}),
        ),
        (
            "query_label_passages",
            json!({"label": "review"}),
            json!({"items": [], "count": 0}),
        ),
        ("list_facets", json!({}), json!([])),
        ("read_facet_counts", json!({"kind": "tag"}), json!([])),
    ] {
        assert_eq!(execute(&app, name, body).unwrap(), expected);
    }
    for (name, body) in [
        ("query_note_tasks", json!({"status": "invalid"})),
        ("query_note_tasks", json!({"status": "open", "limit": 0})),
        ("read_facet_counts", json!({"kind": "invalid"})),
        ("list_facets", json!({"kind": "invalid"})),
        (
            "read_calendar_buckets",
            json!({"field": "title",
            "startDate": "2026-10-01", "endDate": "2026-10-31", "granularity": "day"}),
        ),
    ] {
        assert!(matches!(
            execute(&app, name, body),
            Err(ApplicationError::InvalidRequest(_))
        ));
    }
    assert!(
        !execute(
            &app,
            "validate_query",
            json!({"document":criteria([field("unregistered", "value")])})
        )
        .unwrap()["valid"]
            .as_bool()
            .unwrap()
    );
    assert!(
        execute(
            &app,
            "validate_query",
            json!({"document":criteria([field("type", "page")]).sorted([desc("modified")])})
        )
        .unwrap()["valid"]
            .as_bool()
            .unwrap()
    );
    app.stop_sync().unwrap();
    project.close().unwrap();
}
