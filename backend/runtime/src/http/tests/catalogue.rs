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
    assert_eq!(
        f.call("analyze_query", json!({"query":"+export_stage:ready"}))["valid"],
        false
    );
    let created = f.call(
        "create_property",
        json!({"key":"export_stage","valueType":"select","options":["review","draft","draft"]}),
    );
    assert_eq!(
        created,
        json!({"key":"export_stage","label":"Export stage","valueType":"select","source":"registered","options":["draft","review"]})
    );
    assert_eq!(
        f.call("analyze_query", json!({"query":"+export_stage:ready"}))["valid"],
        true
    );
    let updated = f.call(
        "add_property_options",
        json!({"key":"export_stage","valueType":"select","options":["ready"]}),
    );
    assert_eq!(updated["options"], json!(["draft", "ready", "review"]));
    assert!(
        f.call("read_metadata_fields", json!({}))
            .as_array()
            .unwrap()
            .contains(&updated)
    );
    assert_eq!(
        f.call("remove_property", json!({"key":"export_stage"})),
        json!({"removed":true})
    );
    assert_eq!(
        f.call("analyze_query", json!({"query":"+export_stage:ready"}))["valid"],
        false
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
            "add_property_options",
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
    let properties = f.root.path().join("properties");
    let backup = f.root.path().join("saved-properties");
    std::fs::rename(&properties, &backup).unwrap();
    std::fs::write(&properties, "not a directory").unwrap();
    f.error(
        "create_property",
        json!({"key":"storage_failure","valueType":"text"}),
        StatusCode::INTERNAL_SERVER_ERROR,
    );
    std::fs::remove_file(&properties).unwrap();
    std::fs::rename(&backup, &properties).unwrap();
    assert!(
        f.call("read_metadata_fields", json!({}))
            .as_array()
            .unwrap()
            .iter()
            .any(|field| field["key"] == "used_field")
    );
    let config = f.root.path().join("bluink.json");
    let backup = f.root.path().join("saved-bluink.json");
    let view = f.call("read_default_view", json!({}));
    std::fs::rename(&config, &backup).unwrap();
    f.error(
        "read_default_view",
        json!({}),
        StatusCode::INTERNAL_SERVER_ERROR,
    );
    f.error(
        "update_default_view",
        json!({"content":view}),
        StatusCode::INTERNAL_SERVER_ERROR,
    );
    std::fs::rename(backup, config).unwrap();
    assert_eq!(f.call("read_default_view", json!({})), view);
}
