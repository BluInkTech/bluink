// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use super::*;

#[test]
fn canonical_writes_refresh_cached_facets_in_every_registered_project_index() {
    // This deliberately uses two indexes for one canonical project. Application
    // HTTP tests with one owned index cannot prove registry-wide invalidation.
    struct Indexes(Vec<String>);
    impl Drop for Indexes {
        fn drop(&mut self) {
            for path in self.0.drain(..) {
                crate::service::shutdown_index_service(path).unwrap();
            }
        }
    }
    fn ready(path: &str) {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let status = crate::service::index_service_status_typed(path).unwrap();
            assert_ne!(status.state, "error", "{:?}", status.error);
            if status.state == "ready" && status.pending == 0 {
                return;
            }
            assert!(Instant::now() < deadline, "Index did not commit: {path}");
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    fn facets(path: &str) -> Vec<(String, usize)> {
        crate::relations::facet_service_sync(
            path.to_owned(),
            "tags".to_owned(),
            vec![],
            Some("page".to_owned()),
        )
        .unwrap()
        .into_iter()
        .map(|entry| (entry.tid, entry.count))
        .collect()
    }

    let app = application("two-index-facets");
    app.initialize_sync(false).unwrap();
    let owner = execute(
        &app,
        "create_item",
        json!({"meta":{"type":"page", "title":"Indexed owner"}, "tags":["before"], "content":{"type":"doc", "content":[{"type":"paragraph"}]}}),
    )
    .unwrap();
    let mut indexes = Indexes(vec![]);
    for name in ["first", "second"] {
        let path = app
            .root
            .join(".cache")
            .join(name)
            .to_str()
            .unwrap()
            .to_owned();
        crate::service::start_index_service_typed(
            app.root.to_str().unwrap().to_owned(),
            path.clone(),
        )
        .unwrap();
        indexes.0.push(path);
    }
    for path in &indexes.0 {
        ready(path);
        assert_eq!(facets(path), vec![("before".to_owned(), 1)]);
    }
    execute(
        &app,
        "set_item_tags",
        json!({"id":owner["id"], "version":owner["version"], "tags":["after"]}),
    )
    .unwrap();
    for path in &indexes.0 {
        // Reading while the write is pending may cache the old generation.
        // Publication of the new readers must invalidate that cache as well.
        let _ = facets(path);
        ready(path);
        let result = crate::search::search_service_sync(
            path.to_owned(),
            "+tag:after".to_owned(),
            10,
            0,
            None,
        )
        .unwrap();
        assert_eq!(result.count, 1);
        assert_eq!(result.hits[0].id, owner["id"].as_str().unwrap());
        assert_eq!(facets(path), vec![("after".to_owned(), 1)]);
    }
    drop(indexes);
    app.stop_sync().unwrap();
    std::fs::remove_dir_all(&app.root).unwrap();
}

#[test]
fn catalogue_writes_share_validation_public_shapes_and_usage_guards_without_an_index() {
    let app = application("catalogue-writes");
    app.initialize_sync(false).unwrap();
    let property = execute(
        &app,
        "create_property",
        json!({"key":"review_stage", "valueType":"select", "options":["review", "draft", "draft"]}),
    )
    .unwrap();
    assert_eq!(
        property,
        json!({"key":"review_stage", "label":"Review stage", "valueType":"select", "source":"registered", "options":["draft", "review"]})
    );
    let updated = execute(
        &app,
        "add_property_options",
        json!({"key":"review_stage", "valueType":"select", "options":["ready", "draft"]}),
    )
    .unwrap();
    assert_eq!(updated["options"], json!(["draft", "ready", "review"]));
    for (operation, body) in [
        (
            "create_property",
            json!({"key":"review_stage", "valueType":"text"}),
        ),
        (
            "create_property",
            json!({"key":"Bad Key", "valueType":"text"}),
        ),
        (
            "create_property",
            json!({"key":"summary", "valueType":"text"}),
        ),
        (
            "add_property_options",
            json!({"key":"review_stage", "valueType":"select", "options":["Bad Option"]}),
        ),
        (
            "add_property_options",
            json!({"key":"missing", "valueType":"select", "options":[]}),
        ),
        ("remove_property", json!({"key":"missing"})),
        ("remove_property", json!({"key":"status"})),
        ("create_facet", json!({"kind":"unknown", "key":"unused"})),
        ("create_facet", json!({"kind":"label", "key":"Not valid"})),
        ("remove_facet", json!({"kind":"tag", "key":"missing"})),
    ] {
        assert!(
            matches!(
                execute(&app, operation, body),
                Err(ApplicationError::InvalidRequest(_))
            ),
            "{operation}"
        );
    }
    for (kind, key) in [("tag", "batch/references"), ("label", "review_batch")] {
        let facet = execute(&app, "create_facet", json!({"kind":kind, "key":key})).unwrap();
        assert_eq!(
            facet,
            json!({"kind":kind, "key":key, "usageCount":0, "inferred":false})
        );
        assert!(matches!(
            execute(&app, "create_facet", json!({"kind":kind, "key":key})),
            Err(ApplicationError::InvalidRequest(_))
        ));
    }
    execute(&app, "create_item", json!({"meta":{"type":"page", "title":"Catalogue usages", "review_stage":"ready"}, "tags":["batch/references"], "content":{"type":"doc", "content":[{"type":"paragraph", "attrs":{"labels":["review_batch"]}, "content":[{"type":"text", "text":"Usage fixture"}]}]}})).unwrap();
    for (operation, body) in [
        ("remove_property", json!({"key":"review_stage"})),
        (
            "remove_facet",
            json!({"kind":"tag", "key":"batch/references"}),
        ),
        (
            "remove_facet",
            json!({"kind":"label", "key":"review_batch"}),
        ),
    ] {
        assert!(matches!(
            execute(&app, operation, body),
            Err(ApplicationError::InvalidRequest(_))
        ));
    }
    execute(
        &app,
        "create_property",
        json!({"key":"unused", "valueType":"text"}),
    )
    .unwrap();
    assert_eq!(
        execute(&app, "remove_property", json!({"key":"unused"})).unwrap(),
        json!({"removed":true})
    );
    execute(&app, "create_facet", json!({"kind":"tag", "key":"unused"})).unwrap();
    assert_eq!(
        execute(&app, "remove_facet", json!({"kind":"tag", "key":"unused"})).unwrap(),
        json!({"removed":true})
    );
    assert!(app.state().unwrap().index.is_none());
    app.stop_sync().unwrap();
    std::fs::remove_dir_all(&app.root).unwrap();
}

#[test]
fn catalogue_mutations_refresh_schema_and_cached_facets_before_returning() {
    let app = application("catalogue-refresh");
    app.initialize_sync(true).unwrap();
    let analyze = || execute(&app, "analyze_query", json!({"query":"+batch_field:value"})).unwrap();
    assert_eq!(analyze()["valid"], false);
    execute(
        &app,
        "create_property",
        json!({"key":"batch_field", "valueType":"text"}),
    )
    .unwrap();
    assert_eq!(analyze()["valid"], true);
    execute(&app, "remove_property", json!({"key":"batch_field"})).unwrap();
    assert_eq!(analyze()["valid"], false);
    for kind in ["tag", "label"] {
        let facets = || execute(&app, "list_facets", json!({"kind":kind})).unwrap();
        assert_eq!(facets(), json!([]));
        execute(
            &app,
            "create_facet",
            json!({"kind":kind, "key":"batch_fixture"}),
        )
        .unwrap();
        assert_eq!(facets()[0]["key"], "batch_fixture");
        execute(
            &app,
            "remove_facet",
            json!({"kind":kind, "key":"batch_fixture"}),
        )
        .unwrap();
        assert_eq!(facets(), json!([]));
    }
    app.stop_sync().unwrap();
    std::fs::remove_dir_all(&app.root).unwrap();
}

#[test]
fn catalogue_filesystem_failures_are_internal_errors_not_invalid_user_requests() {
    let app = application("catalogue-io-failures");
    app.initialize_sync(false).unwrap();
    std::fs::rename(
        app.root.join("properties"),
        app.root.join("saved-properties"),
    )
    .unwrap();
    std::fs::write(app.root.join("properties"), "not a directory").unwrap();
    for (operation, body) in [
        (
            "create_property",
            json!({"key":"example", "valueType":"text"}),
        ),
        (
            "add_property_options",
            json!({"key":"status", "valueType":"select", "options":["ready"]}),
        ),
        ("remove_property", json!({"key":"example"})),
    ] {
        assert!(
            matches!(
                execute(&app, operation, body),
                Err(ApplicationError::Internal(_))
            ),
            "{operation}"
        );
    }
    std::fs::remove_file(app.root.join("properties")).unwrap();
    std::fs::rename(
        app.root.join("saved-properties"),
        app.root.join("properties"),
    )
    .unwrap();
    std::fs::write(app.root.join("facets/blocker"), "not a directory").unwrap();
    assert!(matches!(
        execute(
            &app,
            "create_facet",
            json!({"kind":"tag", "key":"blocker/child"})
        ),
        Err(ApplicationError::Internal(_))
    ));
    assert!(matches!(
        execute(
            &app,
            "remove_facet",
            json!({"kind":"tag", "key":"blocker/child"})
        ),
        Err(ApplicationError::Internal(_))
    ));
    app.stop_sync().unwrap();
    std::fs::remove_dir_all(&app.root).unwrap();
}
