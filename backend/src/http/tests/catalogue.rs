// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use crate::structured_query::fixtures::{asc, criteria, field};

use super::support::*;
use axum::http::StatusCode;
use serde_json::json;

#[test]
fn removed_property_on_archived_pages_disappears_from_capabilities() {
    let f = Fixture::new(true);
    let key = "publication_stage";
    f.call(
        "create_property",
        json!({"key":key,"valueType":"select","options":["draft","ready"]}),
    );
    let pages = ["draft", "ready"].map(|value| {
        f.call(
            "create_item",
            json!({"meta":{"type":"page","title":value,(key):value},"content":page_content(value)}),
        )
    });
    f.call("delete_item", json!({"id":pages[0]["id"]}));
    f.error(
        "remove_property",
        json!({"key":key}),
        StatusCode::BAD_REQUEST,
    );
    f.call("delete_item", json!({"id":pages[1]["id"]}));
    f.call("remove_property", json!({"key":key}));
    let capabilities = f.call("read_query_capabilities", json!({}));
    assert!(
        capabilities["fields"]
            .as_array()
            .unwrap()
            .iter()
            .all(|field| field["key"] != key)
    );
    assert_eq!(
        f.call(
            "validate_query",
            json!({"document":criteria([field("publication_stage", "draft")])})
        )["valid"],
        false
    );
    assert_eq!(
        f.call("read_item", json!({"id":pages[0]["id"]}))["meta"][key],
        "draft"
    );
    let id = crate::domain::project::named_items::id("property", key);
    let archived = f.call("read_item", json!({"id":id}));
    f.call(
        "update_item_meta",
        json!({"id":id,"version":archived["version"],"meta":{"state":"active"}}),
    );
    let capabilities = f.call("read_query_capabilities", json!({}));
    assert!(
        capabilities["fields"]
            .as_array()
            .unwrap()
            .iter()
            .any(|field| field["key"] == key && field["valueType"] == "select")
    );
}

#[test]
fn authored_option_order_controls_search_and_reorder_does_not_rewrite_notes() {
    let f = Fixture::new(true);
    let declaration = f.call(
        "create_property",
        json!({"key":"stage","valueType":"select","options":["review","draft","final"]}),
    );
    let review = f.call("create_item", json!({"meta":{"type":"page","title":"Review","stage":"review"},"content":page_content("Review")}));
    let draft = f.call("create_item", json!({"meta":{"type":"page","title":"Draft","stage":"draft"},"content":page_content("Draft")}));
    let query = || {
        f.call(
            "query_items",
            json!({"query":criteria([field("type", "page")]).sorted([asc("stage")]),"limit":1}),
        )
    };
    assert_eq!(query()["items"][0]["id"], review["id"]);
    let updated = f.call("update_property_options", json!({"key":"stage","valueType":"select","optionOrder":["draft","final","review"],"expectedRevision":declaration["revision"]}));
    assert_ne!(updated["revision"], declaration["revision"]);
    assert_eq!(query()["items"][0]["id"], draft["id"]);
    assert_eq!(
        f.call("read_item", json!({"id":review["id"]}))["version"],
        review["version"]
    );
    assert_eq!(
        f.call("read_item", json!({"id":draft["id"]}))["version"],
        draft["version"]
    );
    f.error("update_property_options", json!({"key":"stage","valueType":"select","optionOrder":["review","draft","final"],"expectedRevision":declaration["revision"]}), StatusCode::BAD_REQUEST);
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "Keep each end-to-end scenario together so setup, failure checks and cleanup remain visible."
)]
fn catalogue_mutations_refresh_schema_and_facets_and_distinguish_storage_errors() {
    let f = Fixture::new(true);
    assert!(
        !f.call(
            "validate_query",
            json!({"document":criteria([field("export_stage", "ready")])})
        )["valid"]
            .as_bool()
            .unwrap()
    );
    let created = f.call(
        "create_property",
        json!({"key":"export_stage","valueType":"select","options":["review","draft"]}),
    );
    assert_eq!(
        created,
        json!({"key":"export_stage","valueType":"select","source":"registered","options":["review","draft"],"revision":created["revision"]})
    );
    assert!(
        f.call(
            "validate_query",
            json!({"document":criteria([field("export_stage", "ready")])})
        )["valid"]
            .as_bool()
            .unwrap()
    );
    let updated = f.call(
        "update_property_options",
        json!({"key":"export_stage","valueType":"select","options":["ready"]}),
    );
    assert_eq!(updated["options"], json!(["review", "draft", "ready"]));
    assert!(
        f.call("read_metadata_fields", json!({}))
            .as_array()
            .unwrap()
            .contains(&updated)
    );
    // Colours are saved with the property and reach every reader of its fields.
    let coloured = f.call(
        "update_property_options",
        json!({"key":"export_stage","valueType":"select",
            "optionAppearance":{"ready":{"color":"green"},"review":{"color":"amber"}}}),
    );
    assert_eq!(coloured["options"], updated["options"]);
    assert_eq!(
        coloured["optionAppearance"],
        json!({"ready":{"color":"green"},"review":{"color":"amber"}})
    );
    assert!(
        f.call("read_metadata_fields", json!({}))
            .as_array()
            .unwrap()
            .contains(&coloured)
    );
    for appearance in [
        json!({"ready":{"color":"magenta"}}),
        json!({"gone":{"color":"blue"}}),
    ] {
        f.error(
            "update_property_options",
            json!({"key":"export_stage","valueType":"select","optionAppearance":appearance}),
            StatusCode::BAD_REQUEST,
        );
    }
    assert_eq!(
        f.call("remove_property", json!({"key":"export_stage"})),
        json!({"removed":true})
    );
    assert!(
        !f.call(
            "validate_query",
            json!({"document":criteria([field("export_stage", "ready")])})
        )["valid"]
            .as_bool()
            .unwrap()
    );
    for kind in ["tag", "label"] {
        assert_eq!(f.call("list_facets", json!({"kind":kind})), json!([]));
        let facet = f.call(
            "create_facet",
            json!({"kind":kind,"key":"test_declaration"}),
        );
        assert_eq!(
            facet,
            json!({"kind":kind,"key":"test_declaration","usageCount":0,"inferred":false})
        );
        assert_eq!(f.call("list_facets", json!({"kind":kind})), json!([facet]));
        assert_eq!(
            f.call(
                "remove_facet",
                json!({"kind":kind,"key":"test_declaration"})
            ),
            json!({"removed":true})
        );
        assert_eq!(f.call("list_facets", json!({"kind":kind})), json!([]));
    }
    for (name, body) in [
        (
            "create_property",
            json!({"key":"summary","valueType":"text"}),
        ),
        (
            "create_property",
            json!({"key":"Bad Key","valueType":"text"}),
        ),
        (
            "update_property_options",
            json!({"key":"status","valueType":"select","options":["Bad Value"]}),
        ),
        ("remove_property", json!({"key":"missing"})),
        ("remove_property", json!({"key":"status"})),
        ("create_facet", json!({"kind":"unknown","key":"example"})),
        ("remove_facet", json!({"kind":"tag","key":"missing"})),
    ] {
        f.error(name, body, StatusCode::BAD_REQUEST);
    }
    f.call(
        "create_property",
        json!({"key":"used_field","valueType":"text"}),
    );
    f.call("create_facet", json!({"kind":"tag","key":"used_tag"}));
    f.call("create_item",json!({"meta":{"type":"page","title":"Usage guard","used_field":"value"},"tags":["used_tag"],"content":page_content("")}));
    f.error(
        "remove_property",
        json!({"key":"used_field"}),
        StatusCode::BAD_REQUEST,
    );
    f.error(
        "remove_facet",
        json!({"kind":"tag","key":"used_tag"}),
        StatusCode::BAD_REQUEST,
    );
    let store = crate::store::Store::shared(f.root.path()).unwrap();
    store.write(|tx|{tx.execute_batch("CREATE TRIGGER reject_property BEFORE INSERT ON item WHEN json_extract(NEW.header_json,'$.meta.type')='property' BEGIN SELECT RAISE(ABORT,'simulated storage fault'); END;")?;Ok(())}).unwrap();
    f.error(
        "create_property",
        json!({"key":"storage_failure","valueType":"text"}),
        StatusCode::INTERNAL_SERVER_ERROR,
    );
    store
        .write(|tx| {
            tx.execute_batch("DROP TRIGGER reject_property")?;
            Ok(())
        })
        .unwrap();
    assert!(
        f.call("read_metadata_fields", json!({}))
            .as_array()
            .unwrap()
            .iter()
            .any(|field| field["key"] == "used_field")
    );
    let view = f.call("read_workspace_context", json!({}))["defaultWorkspace"].clone();
    let config = store
        .write(|tx| {
            let old: String = tx.query_row(
                "SELECT canonical_json FROM project_record WHERE path='bluink.json'",
                [],
                |row| row.get(0),
            )?;
            tx.execute(
                "UPDATE project_record SET canonical_json='{}' WHERE path='bluink.json'",
                [],
            )?;
            Ok(old)
        })
        .unwrap();
    f.error(
        "read_workspace_context",
        json!({}),
        StatusCode::INTERNAL_SERVER_ERROR,
    );
    store
        .write(move |tx| {
            tx.execute(
                "UPDATE project_record SET canonical_json=?1 WHERE path='bluink.json'",
                [config],
            )?;
            Ok(())
        })
        .unwrap();
    assert_eq!(
        f.call("read_workspace_context", json!({}))["defaultWorkspace"],
        view
    );
}
