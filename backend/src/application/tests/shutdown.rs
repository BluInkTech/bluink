// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use super::*;
use crate::host::session::ApplicationHandle;
use crate::structured_query::fixtures::{criteria, term};
use std::{
    io::Write,
    net::{Shutdown, TcpStream},
};

#[test]
fn poisoned_shutdown_gates_still_release_workers_and_projections() {
    for poisoned in ["lifecycle", "operations", "state"] {
        let project = TestProject::new("poisoned-shutdown-gate");
        let app = project.application();
        app.initialize_sync().unwrap();
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
            assert!(state.database.is_none());
            assert!(!state.ready);
            drop(state);
        } else {
            assert_eq!(app.status().unwrap()["phase"], "Stopped");
        }
        assert!(app.begin_operation().is_err());
        let next = project.application();
        next.initialize_sync().unwrap();
        next.stop_sync().unwrap();
        // Recovery permits cleanup only; it must not clear the poison.
        assert!(app.stop_sync().is_err());
        drop(app);
        project.close().unwrap();
    }
}

#[test]
fn disconnected_http_write_commits_before_shutdown_finishes() {
    let project = TestProject::new("disconnected-write");
    let app = Arc::new(ApplicationHandle::from_application(project.application()));
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .worker_threads(2)
        .build()
        .unwrap();
    runtime.block_on(async {
        app.initialize(true).await.unwrap();
        let original = execute(&app.inner, "create_item", json!({
            "meta": {"type": "page", "title": "Before disconnect"},
            "content": {"document": {"type": "doc", "content": [{"type": "paragraph",
                "content": [{"type": "text", "text": "Original text"}]}]}}
        })).unwrap();
        let id = original["id"].as_str().unwrap();
        // Hold a real SQLite writer transaction while the HTTP write is admitted.
        let item_lock=rusqlite::Connection::open(app.inner.database().unwrap().path()).unwrap();
        item_lock.execute_batch("BEGIN IMMEDIATE").unwrap();
        let address = app.start_http(Some(0)).await.unwrap();
        let body = json!({"id": id, "version": original["version"],
            "meta": {"title": "Persisted after disconnect"},
            "content": {"document": {"type": "doc", "content": [{"type": "paragraph",
                "content": [{"type": "text", "text": "Committed shutdown sentinel"}]}]}}
        }).to_string();
        let mut client = TcpStream::connect(address).unwrap();
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
        drop(item_lock);
        tokio::time::timeout(Duration::from_secs(10), stop).await.unwrap().unwrap().unwrap();
        let stored = crate::store::snapshots::read_project_item(&app.inner.root, id).unwrap().unwrap();
        assert_eq!(stored["meta"]["title"], "Persisted after disconnect");
        assert_ne!(stored["revision"], original["version"]);
        assert_eq!(stored["content"]["document"]["content"][0]["content"][0]["text"], "Committed shutdown sentinel");
        // Reopening proves both the OS lease and writer thread were released,
        // and the accepted mutation is represented by the rebuilt projection.
        contender.initialize_sync().unwrap();
        let found = execute(&contender, "query_items", json!({"query": criteria([term("sentinel")])})).unwrap();
        assert_eq!(found["count"], 1);
        assert_eq!(found["items"][0]["id"], id);
        contender.stop_sync().unwrap();
        assert_eq!(app.inner.status().unwrap()["phase"], "Stopped");
    });
    project.close().unwrap();
}
