// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use super::*;

fn ready(path: &str) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let status = crate::indexing::service::index_service_status_typed(path).unwrap();
        assert_ne!(status.state, "error", "{:?}", status.error);
        if status.state == "ready" && status.pending == 0 {
            return;
        }
        assert!(Instant::now() < deadline, "Index did not commit: {path}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn query_picker_reads_use_the_schema_snapshot_and_refresh_property_options() {
    let project = TestProject::new("query-picker-snapshot");
    let app = Arc::new(project.application());
    app.initialize_sync(true).unwrap();
    ready(&app.index_path().unwrap());
    execute(
        &app,
        "create_property",
        json!({
            "key":"review_stage", "valueType":"select", "options":["draft"]
        }),
    )
    .unwrap();

    // Both reads must finish even while another writer owns the declarations
    // lock. This catches accidental marker reads without timing a disk cache.
    let lock = crate::engine::project::io::lock_file(
        &app.root.join(".cache/properties-catalogue.lock"),
        crate::engine::project::io::LockMode::Exclusive,
    )
    .unwrap();
    let worker_app = Arc::clone(&app);
    let (send, receive) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        let started = Instant::now();
        let metadata = execute(&worker_app, "read_metadata_fields", json!({}));
        let capabilities = execute(&worker_app, "read_query_capabilities", json!({}));
        send.send((metadata, capabilities, started.elapsed()))
            .unwrap();
    });
    let response = receive.recv_timeout(Duration::from_secs(2));
    drop(lock);
    worker.join().unwrap();
    let (metadata, capabilities, elapsed) =
        response.expect("Picker reads waited for property files");
    let metadata = metadata.unwrap();
    let capabilities = capabilities.unwrap();
    for fields in [&metadata, &capabilities["fields"]] {
        let field = fields
            .as_array()
            .unwrap()
            .iter()
            .find(|field| field["key"] == "review_stage")
            .unwrap();
        assert_eq!(field["options"], json!(["draft"]));
    }
    eprintln!("Metadata and query capabilities together: {elapsed:?}");

    execute(
        &app,
        "update_property_options",
        json!({
            "key":"review_stage", "valueType":"select", "options":["ready"]
        }),
    )
    .unwrap();
    let capabilities = execute(&app, "read_query_capabilities", json!({})).unwrap();
    assert_eq!(
        capabilities["fields"]
            .as_array()
            .unwrap()
            .iter()
            .find(|field| field["key"] == "review_stage")
            .unwrap()["options"],
        json!(["draft", "ready"])
    );

    // External catalogue edits use the same refresh path as the file watcher.
    let path =
        crate::engine::project::named_items::path(&app.root, "property", "review_stage").unwrap();
    let mut document: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    document["content"]["options"] = json!(["external"]);
    std::fs::write(&path, document.to_string()).unwrap();
    crate::indexing::service::refresh_query_fields(app.index_path().unwrap()).unwrap();
    let metadata = execute(&app, "read_metadata_fields", json!({})).unwrap();
    assert_eq!(
        metadata
            .as_array()
            .unwrap()
            .iter()
            .find(|field| field["key"] == "review_stage")
            .unwrap()["options"],
        json!(["external"])
    );

    execute(&app, "remove_property", json!({"key":"review_stage"})).unwrap();
    let capabilities = execute(&app, "read_query_capabilities", json!({})).unwrap();
    assert!(
        !capabilities["fields"]
            .as_array()
            .unwrap()
            .iter()
            .any(|field| field["key"] == "review_stage")
    );
    app.stop_sync().unwrap();
    project.close().unwrap();
}

#[test]
fn failed_schema_refresh_preserves_valid_fields_and_can_recover() {
    let project = TestProject::new("schema-refresh-recovery");
    let app = project.application();
    app.initialize_sync(true).unwrap();
    let index = app.index_path().unwrap();
    let before = crate::indexing::service::service_index(&index)
        .unwrap()
        .query_fields;
    assert!(before.contains("kind"));
    // Malformed external edits must not replace the last valid schema snapshot.
    let duplicate =
        crate::engine::project::named_items::path(&app.root, "property", "kind").unwrap();
    let original = std::fs::read(&duplicate).unwrap();
    std::fs::write(&duplicate, "invalid JSON").unwrap();
    assert!(crate::indexing::service::refresh_query_fields(index.clone()).is_err());
    let retained = crate::indexing::service::service_index(&index)
        .unwrap()
        .query_fields;
    assert_eq!(before.len(), retained.len());
    assert_eq!(before.value_type("kind"), retained.value_type("kind"));
    std::fs::write(duplicate, original).unwrap();
    crate::indexing::service::refresh_query_fields(index).unwrap();
    app.stop_sync().unwrap();
    project.close().unwrap();
}

#[test]
fn canonical_writes_refresh_facets_and_properties_in_every_registered_project_index() {
    // This deliberately uses two indexes for one canonical project. Application
    // HTTP tests with one owned index cannot prove registry-wide invalidation.
    struct Indexes(Vec<String>);
    impl Drop for Indexes {
        fn drop(&mut self) {
            for path in self.0.drain(..) {
                crate::indexing::service::shutdown_index_service(path).unwrap();
            }
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

    let project = TestProject::new("two-index-facets");
    let app = project.application();
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
        crate::indexing::service::start_index_service_typed(
            app.root.to_str().unwrap().to_owned(),
            &path,
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
        "create_property",
        json!({"key":"review_score","valueType":"number"}),
    )
    .unwrap();
    for path in &indexes.0 {
        assert_eq!(
            crate::indexing::service::service_index(path)
                .unwrap()
                .query_fields
                .value_type("review_score"),
            Some(crate::engine::project::types::MetadataFieldValueType::Number)
        );
    }
    execute(&app, "remove_property", json!({"key":"review_score"})).unwrap();
    for path in &indexes.0 {
        assert!(
            !crate::indexing::service::service_index(path)
                .unwrap()
                .query_fields
                .contains("review_score")
        );
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
            crate::query::document::test_query("+tag:after"),
            10,
            0,
            None,
            true,
        )
        .unwrap();
        assert_eq!(result.count, 1);
        assert_eq!(result.hits[0].id, owner["id"].as_str().unwrap());
        assert_eq!(facets(path), vec![("after".to_owned(), 1)]);
    }
    drop(indexes);
    app.stop_sync().unwrap();
    project.close().unwrap();
}

#[test]
fn catalogue_writes_share_validation_public_shapes_and_indexed_usage_guards() {
    let project = TestProject::new("catalogue-writes");
    let app = project.application();
    app.initialize_sync(true).unwrap();
    ready(&app.index_path().unwrap());
    let property = execute(
        &app,
        "create_property",
        json!({"key":"review_stage", "valueType":"select", "options":["review", "draft", "draft"]}),
    )
    .unwrap();
    assert_eq!(
        property,
        json!({"key":"review_stage", "valueType":"select", "source":"registered", "options":["draft", "review"]})
    );
    let updated = execute(
        &app,
        "update_property_options",
        json!({"key":"review_stage", "valueType":"select", "options":["ready", "draft"]}),
    )
    .unwrap();
    assert_eq!(updated["options"], json!(["draft", "ready", "review"]));
    assert_invalid_catalogue_requests(&app);
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
    assert!(app.state().unwrap().index.is_some());
    app.stop_sync().unwrap();
    project.close().unwrap();
}

fn assert_invalid_catalogue_requests(app: &Application) {
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
            "update_property_options",
            json!({"key":"review_stage", "valueType":"select", "options":["Bad Option"]}),
        ),
        (
            "update_property_options",
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
                execute(app, operation, body),
                Err(ApplicationError::InvalidRequest(_))
            ),
            "{operation}"
        );
    }
}

#[test]
fn declaration_filesystem_failures_are_internal_errors() {
    let project = TestProject::new("declaration-io-failures");
    let app = project.application();
    app.initialize_sync(false).unwrap();
    execute(
        &app,
        "create_property",
        json!({"key":"example", "valueType":"text"}),
    )
    .unwrap();
    let path = crate::engine::project::named_items::path(&app.root, "property", "example").unwrap();
    std::fs::write(&path, "invalid JSON").unwrap();
    assert!(matches!(
        execute(
            &app,
            "create_property",
            json!({"key":"example", "valueType":"text"})
        ),
        Err(ApplicationError::Internal(_))
    ));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "invalid JSON");
    execute(
        &app,
        "create_facet",
        json!({"kind":"tag","key":"blocker/child"}),
    )
    .unwrap();
    let path =
        crate::engine::project::named_items::path(&app.root, "tag", "blocker/child").unwrap();
    std::fs::write(&path, "invalid JSON").unwrap();
    assert!(matches!(
        execute(
            &app,
            "create_facet",
            json!({"kind":"tag", "key":"blocker/child"})
        ),
        Err(ApplicationError::Internal(_))
    ));
    assert_eq!(std::fs::read_to_string(path).unwrap(), "invalid JSON");
    app.stop_sync().unwrap();
    project.close().unwrap();
}

#[test]
fn removing_declarations_without_an_index_fails_without_scanning_items() {
    let project = TestProject::new("catalogue-no-index");
    let app = project.application();
    app.initialize_sync(false).unwrap();
    execute(
        &app,
        "create_property",
        json!({"key":"unused","valueType":"text"}),
    )
    .unwrap();
    for kind in ["tag", "label"] {
        execute(&app, "create_facet", json!({"kind":kind,"key":"unused"})).unwrap();
        let error = execute(&app, "remove_facet", json!({"kind":kind,"key":"unused"})).unwrap_err();
        assert!(matches!(error, ApplicationError::InvalidRequest(_)));
        assert!(error.to_string().contains("index"));
    }
    assert!(matches!(
        execute(&app, "remove_property", json!({"key":"unused"})),
        Err(ApplicationError::InvalidRequest(_))
    ));
    assert!(app.state().unwrap().index.is_none());
    app.stop_sync().unwrap();
    project.close().unwrap();
}

#[test]
fn templates_protect_what_they_use_without_counting_as_notes() {
    let project = TestProject::new("template-usages");
    let app = project.application();
    app.initialize_sync(true).unwrap();
    ready(&app.index_path().unwrap());
    execute(
        &app,
        "create_property",
        json!({"key":"reading_stage", "valueType":"select", "options":["to_read", "done"]}),
    )
    .unwrap();
    execute(&app, "create_facet", json!({"kind":"tag", "key":"reading"})).unwrap();
    execute(
        &app,
        "create_facet",
        json!({"kind":"label", "key":"verdict"}),
    )
    .unwrap();
    let template = execute(
        &app,
        "create_item",
        json!({
            "meta": {"type":"template", "title":"Book", "reading_stage":"to_read", "propertyKeys":["author"]},
            "tags": ["reading"],
            "content": {
                "content": {"type":"doc", "content":[{"type":"paragraph", "attrs":{"labels":["verdict"]}, "content":[{"type":"text", "text":"My verdict"}]}]}
            }
        }),
    )
    .unwrap();
    assert_eq!(template["meta"]["type"], "template");
    ready(&app.index_path().unwrap());
    // A template's values, tags and labels are in use, though no note uses them.
    for (operation, body) in [
        ("remove_property", json!({"key":"reading_stage"})),
        ("remove_facet", json!({"kind":"tag", "key":"reading"})),
        ("remove_facet", json!({"kind":"label", "key":"verdict"})),
    ] {
        assert!(
            matches!(
                execute(&app, operation, body.clone()),
                Err(ApplicationError::InvalidRequest(_))
            ),
            "{operation} {body} should be blocked by the template"
        );
    }
    // Counts are about notes, so the template is not one of the tag's uses.
    let facets = execute(&app, "list_facets", json!({"kind":"tag"})).unwrap();
    let reading = facets
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["key"] == "reading")
        .unwrap();
    assert_eq!(reading["usageCount"], 0);
    // Note queries ask for pages, so templates never appear among notes.
    let notes = execute(
        &app,
        "query_items",
        json!({"query":{"version":1, "clauses":[{"polarity":"required", "predicate":{"kind":"field", "field":"type", "operator":"match", "value":"page"}}], "sort":[]}, "limit":50}),
    )
    .unwrap();
    assert!(
        notes["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|item| item["meta"]["type"] == "page")
    );
    // Templates are text-only and follow page rules.
    for body in [
        json!({"meta":{"type":"template", "title":"Cover"}, "content":{"content":{"type":"doc", "content":[{"type":"image", "attrs":{"src":"/assets/01a0e2a0-0000-7000-8000-000000000001/cover.png"}}]}}}),
        json!({"meta":{"type":"template", "title":""}, "content":{"content":{"type":"doc", "content":[]}}}),
        json!({"meta":{"type":"template", "title":"Bad value", "reading_stage":"unknown"}, "content":{"content":{"type":"doc", "content":[]}}}),
    ] {
        assert!(
            execute(&app, "create_item", body.clone()).is_err(),
            "{body}"
        );
    }
}
