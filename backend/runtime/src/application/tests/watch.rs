// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use super::*;
use crate::indexing::coordinator::MainSnapshot;

#[test]
fn periodic_scan_publishes_external_snippet_changes_and_deletions() {
    let project = TestProject::new("native-snippet-events");
    let app = project.application();
    app.initialize_sync(true).unwrap();
    let mut scanner = crate::application::watch::Coordinator::start_with_interval(
        app.root.clone(),
        app.index_path().unwrap(),
        Duration::from_millis(50),
    )
    .unwrap();
    let wait = |text: &str, count: usize| {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let result = execute(&app, "query_snippets", json!({"query":text})).unwrap();
            if result["count"] == count {
                break;
            }
            assert!(Instant::now() < deadline, "Snippet query {text}: {result}");
            std::thread::sleep(Duration::from_millis(20));
        }
    };
    let write = |text, revision| {
        crate::engine::project::snippets::write_snippet(
            &app.root,
            "external",
            json!({"type":"paragraph","content":[{"type":"text","text":text}]}),
            revision,
        )
        .unwrap()
    };
    let first = write("originalsentinel", None);
    wait("originalsentinel", 1);
    write("replacedsentinel", first["revision"].as_str());
    wait("replacedsentinel", 1);
    wait("originalsentinel", 0);
    std::fs::remove_file(crate::engine::project::snippets::path(&app.root, "external").unwrap())
        .unwrap();
    wait("replacedsentinel", 0);
    assert!(scanner.failure().is_none());
    scanner.stop().unwrap();
    app.stop_sync().unwrap();
    project.close().unwrap();
}

fn wait_for_count(app: &Application, query: &str, count: usize) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let result = execute(
            app,
            "query_items",
            json!({"query": crate::query::document::test_query(query)}),
        )
        .unwrap();
        if result["count"] == count {
            return;
        }
        assert!(Instant::now() < deadline, "Query {query}: {result}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn wait_for_commit(index: &str, previous: Option<u64>) -> Option<u64> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let status = crate::indexing::service::index_service_status_typed(index).unwrap();
        if status.pending == 0 && status.state == "ready" && status.last_committed_at != previous {
            return status.last_committed_at;
        }
        assert!(
            Instant::now() < deadline,
            "Index commit did not finish: {status:?}"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn native_root_events_reconcile_external_content_without_waiting_for_periodic_scan() {
    let project = TestProject::new("native-root-events");
    let app = project.application();
    app.initialize_sync(true).unwrap();
    let id = "ABCDEF02-2345-7000-ABCD-EF0123456789";
    crate::engine::project::write::write_item_input(
        &app.root,
        json!({"id": id, "meta": {"type": "page", "title": "nativewatchsentinel"},
            "content": {"type": "doc", "content": [{"type": "paragraph"}]}}),
        None,
        &app.activity_date().unwrap(),
    )
    .unwrap();
    // Creating/removing a shard is visible to the shallow OS subscription;
    // these assertions finish well before the production five-minute scan.
    wait_for_count(&app, "nativewatchsentinel", 1);
    let item = crate::engine::project::identity::ItemId::try_from(id)
        .unwrap()
        .path(&app.root);
    let shard = item.parent().unwrap().parent().unwrap();
    std::fs::rename(shard, app.root.join("removed-shard")).unwrap();
    wait_for_count(&app, "nativewatchsentinel", 0);
    assert!(app.status().unwrap()["watcherError"].is_null());
    app.stop_sync().unwrap();
    project.close().unwrap();
}

#[test]
fn periodic_scan_discovers_deep_external_creates_updates_and_deletions() {
    let project = TestProject::new("periodic-scan");
    let app = project.application();
    app.initialize_sync(true).unwrap();
    let index = app.index_path().unwrap();
    let mut scanner = crate::application::watch::Coordinator::start_with_interval(
        app.root.clone(),
        index.clone(),
        Duration::from_millis(50),
    )
    .unwrap();
    // These canonical writes deliberately bypass application notifications and
    // OS subscriptions. The production five-minute timer is shortened here.
    let id = "ABCDEF01-2345-7000-ABCD-EF0123456789";
    let write = |title, revision| {
        crate::engine::project::write::write_item_input(
            &app.root,
            json!({"id": id, "meta": {"type": "page", "title": title},
                "content": {"type": "doc", "content": [{"type": "paragraph"}]}}),
            revision,
            &app.activity_date().unwrap(),
        )
        .unwrap()
    };
    let initial_commit = crate::indexing::service::index_service_status_typed(&index)
        .unwrap()
        .last_committed_at;
    let created = write("rescanoriginal", None);
    wait_for_count(&app, "rescanoriginal", 1);
    let created_commit = wait_for_commit(&index, initial_commit);
    write("rescanreplacement", created["item"]["revision"].as_str());
    wait_for_count(&app, "rescanreplacement", 1);
    wait_for_count(&app, "rescanoriginal", 0);
    // Publication precedes the status update; wait for its commit timestamp
    // before comparing unchanged rescans.
    let committed = wait_for_commit(&index, created_commit);
    std::thread::sleep(Duration::from_millis(400));
    assert_eq!(
        crate::indexing::service::index_service_status_typed(&index)
            .unwrap()
            .last_committed_at,
        committed,
        "Unchanged rescans must not enqueue documents or commit the index"
    );
    std::fs::remove_file(
        crate::engine::project::identity::ItemId::try_from(id)
            .unwrap()
            .path(&app.root),
    )
    .unwrap();
    wait_for_count(&app, "rescanreplacement", 0);
    assert!(scanner.failure().is_none());
    scanner.stop().unwrap();
    app.stop_sync().unwrap();
    project.close().unwrap();
}

#[test]
fn idle_cache_release_preserves_snapshots_and_accepts_subsequent_writes() {
    let project = TestProject::new("idle-readers");
    let app = project.application();
    app.initialize_sync(true).unwrap();
    execute(
        &app,
        "create_item",
        json!({"meta":{"type":"page","title":"firstidle"},"content":{"type":"doc","content":[{"type":"paragraph"}]}}),
    )
    .unwrap();
    wait_for_count(&app, "firstidle", 1);
    let index = app.index_path().unwrap();
    let MainSnapshot {
        index: native_index,
        reader,
        ..
    } = crate::indexing::service::service_index(&index).unwrap();
    let snapshot = reader.searcher();
    let original_count = snapshot.num_docs();
    let title = native_index.schema().get_field("title").unwrap();
    drop(reader);
    let cache = crate::indexing::service::aggregation_cache(&index).unwrap();
    cache
        .insert(
            "idle-sentinel".to_owned(),
            crate::indexing::state::CachedResult::Facets(Vec::new()),
        )
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(8);
    while !cache.values().unwrap().is_empty() {
        assert!(Instant::now() < deadline, "Idle caches were not released");
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(snapshot.num_docs(), original_count);
    let first = tantivy::query::TermQuery::new(
        tantivy::Term::from_field_text(title, "firstidle"),
        tantivy::schema::IndexRecordOption::Basic,
    );
    assert_eq!(
        snapshot.search(&first, &tantivy::collector::Count).unwrap(),
        1
    );
    execute(
        &app,
        "create_item",
        json!({"meta":{"type":"page","title":"secondidle"},"content":{"type":"doc","content":[{"type":"paragraph"}]}}),
    )
    .unwrap();
    wait_for_count(&app, "secondidle", 1);
    assert_eq!(
        execute(
            &app,
            "query_items",
            json!({"query":crate::query::document::test_query("+type:page")})
        )
        .unwrap()["count"],
        2
    );
    assert_eq!(snapshot.num_docs(), original_count);
    drop(snapshot);
    app.stop_sync().unwrap();
    project.close().unwrap();
}
