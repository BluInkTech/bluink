// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use super::support::*;
use axum::http::StatusCode;
use serde_json::json;

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
            json!({"document":crate::query::document::test_query("+export_stage:ready")})
        )["valid"]
            .as_bool()
            .unwrap()
    );
    let created = f.call(
        "create_property",
        json!({"key":"export_stage","valueType":"select","options":["review","draft","draft"]}),
    );
    assert_eq!(
        created,
        json!({"key":"export_stage","valueType":"select","source":"registered","options":["draft","review"]})
    );
    assert!(
        f.call(
            "validate_query",
            json!({"document":crate::query::document::test_query("+export_stage:ready")})
        )["valid"]
            .as_bool()
            .unwrap()
    );
    let updated = f.call(
        "update_property_options",
        json!({"key":"export_stage","valueType":"select","options":["ready"]}),
    );
    assert_eq!(updated["options"], json!(["draft", "ready", "review"]));
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
            json!({"document":crate::query::document::test_query("+export_stage:ready")})
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
    f.call("create_item",json!({"meta":{"type":"page","title":"Usage guard","used_field":"value"},"tags":["used_tag"],"content":document("")}));
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
    let properties =
        crate::engine::project::named_items::path(f.root.path(), "property", "storage_failure")
            .unwrap();
    let parent = properties.parent().unwrap();
    std::fs::create_dir_all(parent.parent().unwrap()).unwrap();
    std::fs::write(parent, "not a directory").unwrap();
    f.error(
        "create_property",
        json!({"key":"storage_failure","valueType":"text"}),
        StatusCode::INTERNAL_SERVER_ERROR,
    );
    std::fs::remove_file(parent).unwrap();
    assert!(
        f.call("read_metadata_fields", json!({}))
            .as_array()
            .unwrap()
            .iter()
            .any(|field| field["key"] == "used_field")
    );
    let config = f.root.path().join("bluink.json");
    let backup = f.root.path().join("saved-bluink.json");
    let view = f.call("read_workspace_context", json!({}))["defaultWorkspace"].clone();
    std::fs::rename(&config, &backup).unwrap();
    f.error(
        "read_workspace_context",
        json!({}),
        StatusCode::INTERNAL_SERVER_ERROR,
    );
    f.error(
        "set_default_workspace",
        json!({"id":view["id"]}),
        StatusCode::INTERNAL_SERVER_ERROR,
    );
    std::fs::rename(backup, config).unwrap();
    assert_eq!(
        f.call("read_workspace_context", json!({}))["defaultWorkspace"],
        view
    );
}
