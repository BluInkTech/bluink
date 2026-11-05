// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use super::*;

#[test]
fn query_picker_reads_committed_schema_during_another_write_and_refreshes_options() {
    let project = TestProject::new("query-picker-snapshot");
    let app = project.application();
    app.initialize_sync().unwrap();
    execute(
        &app,
        "create_property",
        json!({"key":"review_stage","valueType":"select","options":["draft"]}),
    )
    .unwrap();
    let database = app.database().unwrap();
    let (entered, waiting) = std::sync::mpsc::channel();
    let (release, hold) = std::sync::mpsc::channel();
    let writer = std::thread::spawn(move || {
        database
            .write(move |_tx| {
                entered.send(()).unwrap();
                hold.recv_timeout(std::time::Duration::from_secs(5))
                    .unwrap();
                Ok(())
            })
            .unwrap();
    });
    waiting
        .recv_timeout(std::time::Duration::from_secs(5))
        .unwrap();
    let metadata = execute(&app, "read_metadata_fields", json!({})).unwrap();
    let capabilities = execute(&app, "read_query_capabilities", json!({})).unwrap();
    release.send(()).unwrap();
    writer.join().unwrap();
    for fields in [&metadata, &capabilities["fields"]] {
        let field = fields
            .as_array()
            .unwrap()
            .iter()
            .find(|field| field["key"] == "review_stage")
            .unwrap();
        assert_eq!(field["options"], json!(["draft"]));
    }
    execute(
        &app,
        "update_property_options",
        json!({"key":"review_stage","valueType":"select","options":["ready"]}),
    )
    .unwrap();
    let fields = execute(&app, "read_metadata_fields", json!({})).unwrap();
    assert_eq!(
        fields
            .as_array()
            .unwrap()
            .iter()
            .find(|field| field["key"] == "review_stage")
            .unwrap()["options"],
        json!(["draft", "ready"])
    );
    app.stop_sync().unwrap();
    project.close().unwrap();
}

#[test]
fn independent_sqlite_reader_sees_committed_facet_updates_immediately() {
    let project = TestProject::new("committed-facets");
    let app = project.application();
    app.initialize_sync().unwrap();
    let reader = crate::store::Store::open(&app.root).unwrap();
    let owner=execute(&app,"create_item",json!({"meta":{"type":"page","title":"Owner"},"tags":["before"],"content":{"document":{"type":"doc","content":[{"type":"paragraph"}]}}})).unwrap();
    let facets = || {
        reader
            .read(|tx| crate::store::query::facet_counts(tx, "tag", &[], Some("page")))
            .unwrap()
            .into_iter()
            .map(|entry| (entry.tid, entry.count))
            .collect::<Vec<_>>()
    };
    assert_eq!(facets(), vec![("before".to_owned(), 1)]);
    execute(
        &app,
        "set_item_tags",
        json!({"id":owner["id"],"version":owner["version"],"tags":["after"]}),
    )
    .unwrap();
    assert_eq!(facets(), vec![("after".to_owned(), 1)]);
    drop(reader);
    app.stop_sync().unwrap();
    project.close().unwrap();
}

#[test]
fn catalogue_writes_share_validation_public_shapes_and_indexed_usage_guards() {
    let project = TestProject::new("catalogue-writes");
    let app = project.application();
    app.initialize_sync().unwrap();
    let property = execute(
        &app,
        "create_property",
        json!({"key":"review_stage", "valueType":"select", "options":["review", "draft"]}),
    )
    .unwrap();
    assert_eq!(
        property,
        json!({"key":"review_stage", "valueType":"select", "source":"registered", "options":["review", "draft"], "revision":property["revision"]})
    );
    let updated = execute(
        &app,
        "update_property_options",
        json!({"key":"review_stage", "valueType":"select", "options":["ready", "draft"]}),
    )
    .unwrap();
    assert_eq!(updated["options"], json!(["review", "draft", "ready"]));
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
    execute(&app, "create_item", json!({"meta":{"type":"page", "title":"Catalogue usages", "review_stage":"ready"}, "tags":["batch/references"], "content":{"document": {"type":"doc", "content":[{"type":"paragraph", "attrs":{"labels":["review_batch"]}, "content":[{"type":"text", "text":"Usage fixture"}]}]}}})).unwrap();
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
fn unused_declarations_can_be_removed_immediately() {
    let project = TestProject::new("catalogue-remove-unused");
    let app = project.application();
    app.initialize_sync().unwrap();
    execute(
        &app,
        "create_property",
        json!({"key":"unused","valueType":"text"}),
    )
    .unwrap();
    for kind in ["tag", "label"] {
        execute(&app, "create_facet", json!({"kind":kind,"key":"unused"})).unwrap();
        execute(&app, "remove_facet", json!({"kind":kind,"key":"unused"})).unwrap();
        let facets = execute(&app, "list_facets", json!({"kind":kind})).unwrap();
        assert!(
            facets
                .as_array()
                .unwrap()
                .iter()
                .all(|facet| facet["key"] != "unused")
        );
    }
    execute(&app, "remove_property", json!({"key":"unused"})).unwrap();
    app.stop_sync().unwrap();
    project.close().unwrap();
}

#[test]
fn templates_protect_what_they_use_without_counting_as_notes() {
    let project = TestProject::new("template-usages");
    let app = project.application();
    app.initialize_sync().unwrap();
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
                "document": {"type":"doc", "content":[{"type":"paragraph", "attrs":{"labels":["verdict"]}, "content":[{"type":"text", "text":"My verdict"}]}]}
            }
        }),
    )
    .unwrap();
    assert_eq!(template["meta"]["type"], "template");
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
        json!({"meta":{"type":"template", "title":"Cover"}, "content":{"document":{"type":"doc", "content":[{"type":"image", "attrs":{"src":"/assets/01a0e2a0-0000-7000-8000-000000000001/cover.png"}}]}}}),
        json!({"meta":{"type":"template", "title":""}, "content":{"document":{"type":"doc", "content":[]}}}),
        json!({"meta":{"type":"template", "title":"Bad value", "reading_stage":"unknown"}, "content":{"document":{"type":"doc", "content":[]}}}),
    ] {
        assert!(
            execute(&app, "create_item", body.clone()).is_err(),
            "{body}"
        );
    }
}
