// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

mod analytics;
mod cache_process;
mod catalogue;
mod commands;
mod exports;
mod publication_contract;
mod settings;
mod shutdown;
mod snippets;
mod watch;

use super::*;
use std::sync::Arc;

fn application(name: &str) -> Application {
    let root = std::env::temp_dir().join(format!("bluink-app-{name}-{}", std::process::id()));
    Application::new(ApplicationOptions {
        project_root: root.to_str().unwrap().to_owned(),
        timezone: Some("Europe/London".to_owned()),
        cache_location: None,
        public_dir: None,
        data_dir: None,
        web_font_folder: None,
        pdf_font_folder: None,
        typst_binary: None,
    })
    .unwrap()
}

#[test]
fn direct_operations_do_not_create_an_index() {
    let app = application("direct");
    let opened = app.initialize_sync(false).unwrap();
    assert!(opened["indexPath"].is_null());
    assert_eq!(app.status().unwrap()["ready"], true);
    assert_eq!(app.activity_date().unwrap().len(), 10);
    app.stop_sync().unwrap();
    app.stop_sync().unwrap();
    assert!(app.initialize_sync(false).is_err());
    std::fs::remove_dir_all(&app.root).unwrap();
}

#[test]
fn request_timing_accounts_for_application_admission_waits() {
    let app = Arc::new(application("admission-timing"));
    app.initialize_sync(false).unwrap();
    let gate = app.operations.write().unwrap();
    let worker = Arc::clone(&app);
    let (started, ready) = std::sync::mpsc::channel();
    let (finished, result) = std::sync::mpsc::channel();
    let thread = std::thread::spawn(move || {
        let started_at = Instant::now();
        let (response, waits) = crate::diagnostics::waits::capture(|| {
            started.send(()).unwrap();
            worker.execute(operation::Request::decode("read_metadata_fields", b"{}").unwrap())
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
    assert_eq!(waits.readiness, Duration::ZERO);
    thread.join().unwrap();
    app.stop_sync().unwrap();
    std::fs::remove_dir_all(&app.root).unwrap();
}

#[test]
fn index_ownership_is_exclusive_and_released_after_join() {
    let first = application("exclusive");
    let second = application("exclusive");
    let opened = first.initialize_sync(true).unwrap();
    assert!(opened["indexPath"].is_string());
    assert!(second.initialize_sync(true).is_err());
    assert_eq!(first.status().unwrap()["ready"], true);
    first.stop_sync().unwrap();
    second.initialize_sync(true).unwrap();
    second.stop_sync().unwrap();
    std::fs::remove_dir_all(&first.root).unwrap();
}

#[test]
fn failed_index_initialization_releases_ownership() {
    let first = application("failure");
    first.initialize_sync(false).unwrap();
    let id = "00000000-0000-7000-8000-000000000999";
    let item = first.root.join("items/00").join(id).join("item.json");
    std::fs::create_dir_all(item.parent().unwrap()).unwrap();
    std::fs::write(&item, "{}").unwrap();
    assert!(first.initialize_sync(true).is_err());
    std::fs::remove_file(item).unwrap();
    let second = application("failure");
    second.initialize_sync(true).unwrap();
    first.stop_sync().unwrap();
    second.stop_sync().unwrap();
    std::fs::remove_dir_all(&first.root).unwrap();
}

#[test]
fn shutdown_rejects_new_operations_and_waits_for_accepted_work() {
    let app = Arc::new(application("operation-drain"));
    app.initialize_sync(false).unwrap();
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
        Err(ApplicationError::InvalidRequest(_))
    ));
    assert!(matches!(
        completion.try_recv(),
        Err(std::sync::mpsc::TryRecvError::Empty)
    ));
    drop(accepted);
    completion.recv_timeout(Duration::from_secs(5)).unwrap();
    worker.join().unwrap();
    assert_eq!(app.status().unwrap()["phase"], "Stopped");
    std::fs::remove_dir_all(&app.root).unwrap();
}

#[test]
fn item_operations_keep_two_project_handles_isolated_without_an_index() {
    let first = application("read-isolation-first");
    let second = application("read-isolation-second");
    let id = "00000000-0000-7000-8000-000000000212";
    for (app, title) in [(&first, "First project"), (&second, "Second project")] {
        app.initialize_sync(false).unwrap();
        crate::engine::project::write::write_item_input(
            &app.root,
            json!({"id": id, "meta": {"type": "page", "title": title}, "content": {"type": "doc", "content": [{"type": "paragraph"}]}}),
            None, &app.activity_date().unwrap(),
        ).unwrap();
        let value = app
            .read_item(request::decode(format!(r#"{{"id":"{id}"}}"#).as_bytes()).unwrap())
            .unwrap();
        assert_eq!(value.meta.title.as_deref().unwrap(), title);
        assert!(app.state().unwrap().index.is_none());
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
    std::fs::remove_dir_all(&first.root).unwrap();
    std::fs::remove_dir_all(&second.root).unwrap();
}
#[expect(
    clippy::needless_pass_by_value,
    reason = "Test requests own fixture values just like decoded production requests."
)]
fn execute(app: &Application, name: &str, body: Value) -> Result<Value> {
    let request = operation::Request::decode(name, &serde_json::to_vec(&body).unwrap())?;
    serde_json::to_value(app.execute(request)?).map_err(error::internal_error)
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "Keep each end-to-end scenario together so setup, failure checks and cleanup remain visible."
)]
fn application_mutations_preserve_revisions_and_canonical_fields_without_an_index() {
    let app = application("mutation-contract");
    app.initialize_sync(false).unwrap();
    let created = execute(
        &app,
        "create_item",
        json!({
            "meta": {"type": "page", "title": "Original"},
            "tags": ["research"],
            "content": {"type": "doc", "content": [{"type": "paragraph",
                "content": [{"type": "text", "text": "Original prose"}]}]}
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
    assert_eq!(task["tasks"][0]["text"], "Review this");
    let pin = execute(
        &app,
        "set_note_pin",
        json!({
            "noteId": id, "version": task["version"], "pinned": true
        }),
    )
    .unwrap();
    assert_eq!(pin["pinned"], true);
    assert!(pin["version"].is_string());
    assert_eq!(
        execute(&app, "delete_item", json!({"id": id})).unwrap(),
        json!({"deleted": true})
    );
    let canonical = crate::engine::project::item::read_project_item(&app.root, id)
        .unwrap()
        .unwrap();
    assert_eq!(canonical["meta"]["state"], "archived");
    assert_eq!(canonical["content"], created["content"]);
    assert_eq!(canonical["tasks"][0]["text"], "Review this");
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
    assert!(app.state().unwrap().index.is_none());
    app.stop_sync().unwrap();
    assert!(execute(&app, "create_item", json!({})).is_err());
    std::fs::remove_dir_all(&app.root).unwrap();
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "Keep each end-to-end scenario together so setup, failure checks and cleanup remain visible."
)]
fn typed_queries_share_cache_invalidation_and_keep_domain_errors() {
    use crate::model::CachedResult;
    use crate::{calendar, passages, relations, search, tasks};

    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let app = application(&format!("typed-queries-{unique}"));
    app.initialize_sync(false).unwrap();
    let id = "00000000-0000-7000-8000-000000000211";
    let first = crate::engine::project::write::write_item_input(
        &app.root,
        json!({
            "id": id,
            "meta": {"type": "page", "title": "Typed query fixture"},
            "tags": ["research"],
            "content": {"type": "doc", "content": [{
                "type": "paragraph", "attrs": {"labels": ["review"]},
                "content": [{"type": "text", "text": "Initial searchable paragraph"}]
            }]}
        }),
        None,
        &app.activity_date().unwrap(),
    )
    .unwrap();
    let with_task = crate::engine::project::task::mutate_note_task(
        &app.root,
        id,
        first["item"]["revision"].as_str().unwrap(),
        &json!({"kind": "create", "text": "Check the fixture"}),
        &app.activity_date().unwrap(),
    )
    .unwrap();
    let direct = app
        .read_item(request::decode(format!(r#"{{"id":"{id}"}}"#).as_bytes()).unwrap())
        .unwrap();
    assert_eq!(direct.version, with_task["item"]["revision"]);
    assert_eq!(direct.content["type"], "doc");
    let compact = app
        .read_items(request::decode(format!(r#"{{"ids":["{id}"]}}"#).as_bytes()).unwrap())
        .unwrap();
    assert!(compact[0].content.is_empty());
    let opened = app.initialize_sync(true).unwrap();
    let index = opened["indexPath"].as_str().unwrap().to_owned();
    let found =
        search::search_service_sync(index.clone(), "searchable".to_owned(), 10, 0, None).unwrap();
    assert_eq!(found.count, 1);
    assert_eq!(found.hits[0].id, id);
    let public = app
        .query_items(
            request::decode(
                br#"{"query":"searchable", "pinScopeId":"00000000-0000-7000-8000-000000000001"}"#,
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(public.items[0].id, id);
    assert_eq!(
        public.items[0].preview.as_deref(),
        Some("Initial searchable paragraph")
    );
    assert_eq!(public.pinned_count, Some(0));
    assert_eq!(
        search::matching_item_ids_service_sync(index.clone(), "searchable".to_owned()).unwrap(),
        vec![id]
    );
    let cached = service::aggregation_cache(&index).unwrap();
    assert!(
        cached
            .lock()
            .unwrap()
            .values()
            .any(|entry| matches!(entry, CachedResult::Search(_)))
    );
    let again =
        search::search_service_sync(index.clone(), "searchable".to_owned(), 10, 0, None).unwrap();
    assert_eq!(again.hits[0].revision, found.hits[0].revision);
    let counts =
        relations::facet_service_sync(index.clone(), "tags".to_owned(), Vec::new(), None).unwrap();
    assert_eq!(counts[0].tid, "research");
    assert_eq!(counts[0].count, 1);
    let day = &first["item"]["createdAt"].as_str().unwrap()[..10];
    let buckets = calendar::calendar_buckets_service_sync(
        index.clone(),
        String::new(),
        "created".to_owned(),
        day.to_owned(),
        day.to_owned(),
        "day".to_owned(),
        None,
    )
    .unwrap();
    assert_eq!(buckets.buckets[0].date, day);
    assert_eq!(buckets.buckets[0].count, 1);
    let paragraphs = passages::query_service(index.clone(), "review".to_owned(), 10, 0).unwrap();
    assert_eq!(paragraphs.count, 1);
    assert_eq!(paragraphs.items[0].text, "Initial searchable paragraph");
    let selected = tasks::query_service(
        index.clone(),
        Some(vec![id.to_owned()]),
        None,
        "open".to_owned(),
        10,
        0,
    )
    .unwrap();
    assert_eq!(selected.count, 1);
    assert_eq!(selected.items[0].text, "Check the fixture");
    let selected = execute(
        &app,
        "query_note_tasks",
        json!({"query": "#research", "status": "open"}),
    )
    .unwrap();
    assert_eq!(selected["count"], 1);
    let facets = execute(&app, "list_facets", json!({"kind": "tag"})).unwrap();
    assert!(
        facets
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["key"] == "research" && entry["usageCount"] == 1)
    );
    let counts = execute(&app, "read_facet_counts", json!({"kind": "label"})).unwrap();
    assert_eq!(counts, json!([{"key": "review", "count": 1}]));
    assert_eq!(
        execute(
            &app,
            "read_facet_counts",
            json!({
                "kind": "label", "keys": ["review"]
            })
        )
        .unwrap(),
        json!([])
    );
    assert!(
        execute(&app, "read_metadata_fields", json!({}))
            .unwrap()
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["key"] == "summary" && entry["source"] == "builtin")
    );
    let status = execute(&app, "cache_status", json!({})).unwrap();
    assert_eq!(status["structuralState"], "ready");
    assert_eq!(status["documents"], 1);
    assert_eq!(status["schemaVersion"], 1);
    assert_eq!(
        tasks::query_service(
            index.clone(),
            Some(Vec::new()),
            None,
            "open".to_owned(),
            10,
            0
        )
        .unwrap()
        .count,
        0
    );
    for error in [
        search::search_service_sync(index.clone(), "unregistered:value".to_owned(), 10, 0, None)
            .unwrap_err(),
        passages::query_service(index.clone(), String::new(), 10, 0).unwrap_err(),
        tasks::query_service(index.clone(), None, None, "invalid".to_owned(), 10, 0).unwrap_err(),
        calendar::calendar_buckets_service_sync(
            index.clone(),
            String::new(),
            "created".to_owned(),
            "invalid".to_owned(),
            day.to_owned(),
            "day".to_owned(),
            None,
        )
        .unwrap_err(),
        relations::facet_service_sync(index.clone(), "invalid".to_owned(), Vec::new(), None)
            .unwrap_err(),
    ] {
        assert!(matches!(error, ApplicationError::InvalidRequest(_)));
    }
    execute(
        &app,
        "update_item",
        json!({
            "id": id, "version": with_task["item"]["revision"],
            "meta": {"title": "Changed query fixture"}, "tags": ["changed"],
            "content": with_task["item"]["content"]
        }),
    )
    .unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        let counts =
            relations::facet_service_sync(index.clone(), "tags".to_owned(), Vec::new(), None)
                .unwrap();
        if service::index_service_status_typed(&index).unwrap().pending == 0
            && counts.iter().any(|entry| entry.tid == "changed")
        {
            assert!(!counts.iter().any(|entry| entry.tid == "research"));
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "Typed facet cache did not invalidate after a canonical write"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    app.stop_sync().unwrap();
    std::fs::remove_dir_all(&app.root).unwrap();
}

#[test]
fn empty_application_queries_return_empty_results_and_keep_validation() {
    let app = application("empty-queries");
    app.initialize_sync(true).unwrap();
    for field in ["created", "modified", "meta.date", "completed"] {
        let result = execute(
            &app,
            "read_calendar_buckets",
            json!({
                "field": field, "startDate": "2026-10-01", "endDate": "2026-10-31",
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
    assert_eq!(
        execute(
            &app,
            "analyze_query",
            json!({"query": "unregistered:value"})
        )
        .unwrap()["valid"],
        false
    );
    assert_eq!(
        execute(
            &app,
            "analyze_query",
            json!({"query": "+type:page sort:modified-desc"})
        )
        .unwrap()["valid"],
        true
    );
    app.stop_sync().unwrap();
    std::fs::remove_dir_all(&app.root).unwrap();
}
