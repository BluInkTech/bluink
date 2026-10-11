// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use super::*;
use fs4::fs_std::FileExt;
use std::io::Write;
use std::net::{Shutdown, TcpStream};

#[test]
fn stop_drains_accepted_external_changes_before_joining_the_index_worker() {
    use tantivy::schema::Value as _;
    let app = application("watch-drain");
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
    let (index, reader, _) = crate::service::service_index(&path).unwrap();
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
        .notify(Some(format!("items/{}/{id}/item.json", &id[..2])));
    app.stop_sync().unwrap();
    // Inspect the committed index directly, without startup reconciliation
    // concealing an event lost during stop.
    reader.reload().unwrap();
    let searcher = reader.searcher();
    let title = index.schema().get_field("title").unwrap();
    let hits = searcher
        .search(
            &tantivy::query::AllQuery,
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
    let next = application("watch-drain");
    next.initialize_sync(true).unwrap();
    next.stop_sync().unwrap();
    std::fs::remove_dir_all(&app.root).unwrap();
}

#[test]
fn disconnected_http_write_finishes_before_stop_releases_index_ownership() {
    let app = Arc::new(BluinkApplication {
        inner: Arc::new(application("disconnected-write")),
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
        let contender = application("disconnected-write");
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
        let found = execute(&contender, "query_items", json!({"query": "sentinel"})).unwrap();
        assert_eq!(found["count"], 1);
        assert_eq!(found["items"][0]["id"], id);
        contender.stop_sync().unwrap();
        assert_eq!(app.inner.status().unwrap()["phase"], "Stopped");
        std::fs::remove_dir_all(&app.inner.root).unwrap();
    });
}
