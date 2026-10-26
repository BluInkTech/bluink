// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use super::*;
use crate::indexing::coordinator::MainSnapshot;
use fs4::fs_std::FileExt;
use std::io::Write;
use std::net::{Shutdown, TcpStream};

#[test]
fn poisoned_shutdown_gates_still_release_workers_and_projections() {
    for poisoned in ["lifecycle", "operations", "state"] {
        let project = TestProject::new("poisoned-shutdown-gate");
        let app = project.application();
        app.initialize_sync(true).unwrap();
        execute(&app, "query_snippets", json!({})).unwrap();
        std::thread::scope(|scope| {
            let app = &app;
            assert!(
                scope
                    .spawn(move || {
                        if poisoned == "lifecycle" {
                            let _guard = app.lifecycle.lock().unwrap();
                            panic!("Injected lifecycle panic");
                        }
                        if poisoned == "state" {
                            let _guard = app.state.lock().unwrap();
                            panic!("Injected state panic");
                        }
                        let _guard = app.operations.write().unwrap();
                        panic!("Injected admission panic");
                    })
                    .join()
                    .is_err()
            );
        });
        let error = app.stop_sync().unwrap_err().to_string();
        assert!(error.contains(poisoned));
        if poisoned == "state" {
            assert!(app.status().is_err());
            let state = app
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            assert_eq!(state.phase, "Stopped");
            assert!(state.index.is_none());
            assert!(!state.ready);
            drop(state);
        } else {
            assert_eq!(app.status().unwrap()["phase"], "Stopped");
        }
        assert!(app.begin_operation().is_err());
        let next = project.application();
        next.initialize_sync(true).unwrap();
        next.stop_sync().unwrap();
        // Recovery permits cleanup only; it must not clear the poison.
        assert!(app.stop_sync().is_err());
        drop(app);
        project.close().unwrap();
    }
}

#[test]
fn failed_event_worker_does_not_skip_other_shutdown_cleanup() {
    let project = TestProject::new("failed-worker-cleanup");
    let app = project.application();
    app.initialize_sync(true).unwrap();
    execute(&app, "query_snippets", json!({})).unwrap();
    app.state()
        .unwrap()
        .index
        .as_mut()
        .unwrap()
        .events
        .replace_with_failed_worker_for_test();

    let error = app.stop_sync().unwrap_err();
    assert!(error.to_string().contains("Project event worker panicked"));
    assert!(app.state().unwrap().index.is_none());
    assert_eq!(app.status().unwrap()["phase"], "Stopped");
    assert_eq!(app.status().unwrap()["ready"], false);
    assert!(app.begin_operation().is_err());

    // Reopening proves the index worker and its cache lease were released even
    // though the watcher join reported a failure.
    let next = project.application();
    next.initialize_sync(true).unwrap();
    next.stop_sync().unwrap();
    app.stop_sync().unwrap();
    project.close().unwrap();
}

#[test]
fn stop_drains_accepted_external_changes_before_joining_the_index_worker() {
    use tantivy::schema::Value as _;
    let project = TestProject::new("watch-drain");
    let app = project.application();
    app.initialize_sync(true).unwrap();
    let original = execute(
        &app,
        "create_item",
        json!({
            "meta": {"type": "page", "title": "Before external change"},
            "content": {"type": "doc", "content": [{"type": "paragraph"}]}
        }),
    )
    .unwrap();
    let id = original["id"].as_str().unwrap();
    let path = app.index_path().unwrap();
    let MainSnapshot { index, reader, .. } =
        crate::indexing::service::service_index(&path).unwrap();
    crate::engine::project::write::write_item_input(
        &app.root,
        json!({
            "id": id, "meta": {"type": "page", "title": "External shutdown sentinel"},
            "content": original["content"]
        }),
        Some(original["version"].as_str().unwrap()),
        &app.activity_date().unwrap(),
    )
    .unwrap();
    app.state()
        .unwrap()
        .index
        .as_ref()
        .unwrap()
        .events
        .enqueue_for_test(Some(format!("items/{}/{id}/item.json", &id[..2])));
    app.stop_sync().unwrap();
    // Inspect the committed index directly, without startup reconciliation
    // concealing an event lost during stop.
    reader.reload().unwrap();
    let searcher = reader.searcher();
    let title = index.schema().get_field("title").unwrap();
    let hits = searcher
        .search(
            &tantivy::query::TermQuery::new(
                tantivy::Term::from_field_text(index.schema().get_field("id").unwrap(), id),
                tantivy::schema::IndexRecordOption::Basic,
            ),
            &tantivy::collector::TopDocs::with_limit(10).order_by_score(),
        )
        .unwrap();
    let titles = hits
        .into_iter()
        .filter_map(|(_, address)| {
            let document = searcher.doc::<tantivy::TantivyDocument>(address).unwrap();
            document
                .get_first(title)
                .and_then(|value| value.as_str())
                .map(str::to_owned)
        })
        .collect::<Vec<_>>();
    assert!(
        titles
            .iter()
            .any(|value| value == "External shutdown sentinel"),
        "{titles:?}"
    );
    assert!(!titles.iter().any(|value| value == "Before external change"));
    drop(searcher);
    drop(reader);
    drop(index);
    let next = project.application();
    next.initialize_sync(true).unwrap();
    next.stop_sync().unwrap();
    project.close().unwrap();
}

#[test]
fn disconnected_http_write_finishes_before_stop_releases_index_ownership() {
    let project = TestProject::new("disconnected-write");
    let app = Arc::new(BluinkApplication {
        inner: Arc::new(project.application()),
    });
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .worker_threads(2)
        .build()
        .unwrap();
    runtime.block_on(async {
        app.initialize(true).await.unwrap();
        let original = execute(&app.inner, "create_item", json!({
            "meta": {"type": "page", "title": "Before disconnect"},
            "content": {"type": "doc", "content": [{"type": "paragraph",
                "content": [{"type": "text", "text": "Original text"}]}]}
        })).unwrap();
        let id = original["id"].as_str().unwrap();
        // Hold the actual canonical item lock, rather than substituting a fake
        // HTTP action. The accepted update cannot commit until this is released.
        let item_lock = std::fs::OpenOptions::new().read(true).write(true)
            .open(app.inner.root.join(".cache/item-locks").join(format!("{id}.lock")))
            .unwrap();
        item_lock.lock_exclusive().unwrap();
        let address = app.start_http(Some(0)).await.unwrap();
        let body = json!({"id": id, "version": original["version"],
            "meta": {"title": "Persisted after disconnect"},
            "content": {"type": "doc", "content": [{"type": "paragraph",
                "content": [{"type": "text", "text": "Committed shutdown sentinel"}]}]}
        }).to_string();
        let mut client = TcpStream::connect((address.host.as_str(), u16::try_from(address.port).unwrap())).unwrap();
        write!(client, "POST /api/update_item HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}", body.len(), body).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            match app.inner.operations.try_write() {
                Err(std::sync::TryLockError::WouldBlock) => break,
                Ok(guard) => drop(guard),
                Err(error) => panic!("Operation guard failed: {error}"),
            }
            assert!(Instant::now() < deadline, "HTTP write was not admitted");
            tokio::task::yield_now().await;
        }
        client.shutdown(Shutdown::Both).unwrap();
        drop(client);
        let stopping = Arc::clone(&app);
        let stop = tokio::spawn(async move { stopping.stop().await });
        while !app.inner.stopping.load(Ordering::Acquire) {
            assert!(Instant::now() < deadline, "Stop was not admitted");
            tokio::task::yield_now().await;
        }
        assert!(!stop.is_finished(), "Stop abandoned the canonical write");
        assert!(app.inner.begin_operation().is_err());
        let contender = project.application();
        assert!(contender.initialize_sync(true).is_err(), "Cache ownership escaped during the write");
        drop(item_lock);
        tokio::time::timeout(Duration::from_secs(10), stop).await.unwrap().unwrap().unwrap();
        let stored = crate::engine::project::item::read_project_item(&app.inner.root, id).unwrap().unwrap();
        assert_eq!(stored["meta"]["title"], "Persisted after disconnect");
        assert_ne!(stored["revision"], original["version"]);
        assert_eq!(stored["content"]["content"][0]["content"][0]["text"], "Committed shutdown sentinel");
        // Reopening proves both the OS lease and writer thread were released,
        // and the accepted mutation is represented by the rebuilt projection.
        contender.initialize_sync(true).unwrap();
        let found = execute(&contender, "query_items", json!({"query": crate::query::document::test_query("sentinel")})).unwrap();
        assert_eq!(found["count"], 1);
        assert_eq!(found["items"][0]["id"], id);
        contender.stop_sync().unwrap();
        assert_eq!(app.inner.status().unwrap()["phase"], "Stopped");
    });
    project.close().unwrap();
}
