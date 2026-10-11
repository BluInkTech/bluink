// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use super::*;

fn wait_for_count(app: &Application, query: &str, count: usize) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let result = execute(app, "query_items", json!({"query": query})).unwrap();
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
        let status = crate::service::index_service_status_typed(index).unwrap();
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
fn periodic_scan_discovers_deep_external_creates_updates_and_deletions() {
    let app = application("periodic-scan");
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
    let initial_commit = crate::service::index_service_status_typed(&index)
        .unwrap()
        .last_committed_at;
    let created = write("rescanoriginal", None);
    wait_for_count(&app, "rescanoriginal", 1);
    let created_commit = wait_for_commit(&index, initial_commit);
    write("rescanreplacement", created["item"]["revision"].as_str());
    wait_for_count(&app, "rescanreplacement", 1);
    wait_for_count(&app, "rescanoriginal", 0);
    // Queries can read the dirty overlay before the worker records its commit.
    // Compare unchanged rescans only after that pending update is fully committed.
    let committed = wait_for_commit(&index, created_commit);
    std::thread::sleep(Duration::from_millis(400));
    assert_eq!(
        crate::service::index_service_status_typed(&index)
            .unwrap()
            .last_committed_at,
        committed,
        "Unchanged rescans must not enqueue documents or commit the index"
    );
    std::fs::remove_file(crate::project::item_path(&app.root, id)).unwrap();
    wait_for_count(&app, "rescanreplacement", 0);
    assert!(scanner.failure().is_none());
    scanner.stop().unwrap();
    app.stop_sync().unwrap();
    std::fs::remove_dir_all(&app.root).unwrap();
}

#[test]
fn idle_cache_release_preserves_snapshots_and_accepts_subsequent_writes() {
    let app = application("idle-readers");
    app.initialize_sync(true).unwrap();
    execute(
        &app,
        "create_item",
        json!({"meta":{"type":"page","title":"firstidle"},"content":{"type":"doc","content":[{"type":"paragraph"}]}}),
    )
    .unwrap();
    wait_for_count(&app, "firstidle", 1);
    let index = app.index_path().unwrap();
    let (native_index, reader, _) = crate::service::service_index(&index).unwrap();
    let snapshot = reader.searcher();
    let title = native_index.schema().get_field("title").unwrap();
    drop(reader);
    let cache = crate::service::aggregation_cache(&index).unwrap();
    cache.lock().unwrap().insert(
        "idle-sentinel".to_owned(),
        crate::model::CachedResult::Facets(Vec::new()),
    );
    let deadline = Instant::now() + Duration::from_secs(8);
    while !cache.lock().unwrap().is_empty() {
        assert!(Instant::now() < deadline, "Idle caches were not released");
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(snapshot.num_docs(), 1);
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
        execute(&app, "query_items", json!({"query":"+type:page"})).unwrap()["count"],
        2
    );
    assert_eq!(snapshot.num_docs(), 1);
    drop(snapshot);
    app.stop_sync().unwrap();
    std::fs::remove_dir_all(&app.root).unwrap();
}
