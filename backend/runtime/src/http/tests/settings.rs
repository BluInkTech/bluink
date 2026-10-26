// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use super::support::*;
use axum::http::StatusCode;
use serde_json::{Value, json};

#[test]
fn reusable_theme_rpc_round_trips_revisions_and_assets_without_an_index() {
    let f = Fixture::new(false);
    let initial_items = crate::engine::project::item::list_project_item_ids(f.root.path()).unwrap();
    assert_eq!(
        f.call("list_publication_themes", json!({}))
            .as_array()
            .unwrap()
            .len(),
        2
    );
    let builtin = f.call(
        "read_publication_theme_package",
        json!({"themeId":"classic"}),
    );
    f.error(
        "delete_publication_theme",
        json!({"themeId":"classic","revision":builtin["summary"]["revision"]}),
        StatusCode::BAD_REQUEST,
    );
    let created = f.call("create_publication_theme", json!({"name":"Research 文学"}));
    let id = &created["summary"]["id"];
    let revision = &created["summary"]["revision"];
    let mut definition = created["definition"].clone();
    definition["root"]["fontSize"] = json!("14pt");
    let request = json!({"themeId":id,"revision":revision,"definition":definition});
    let validation = f.call("validate_publication_theme", request.clone());
    for target in ["html", "site", "pdf", "docx", "epub"] {
        assert_eq!(
            validation["resolved"][target]["rootFontSizePt"].as_f64(),
            Some(14.0)
        );
    }
    assert_eq!(
        f.call("read_publication_theme_package", json!({"themeId":id}))["summary"]["revision"],
        *revision
    );
    let updated = f.call("update_publication_theme", request.clone());
    assert_eq!(updated["definition"]["root"]["fontSize"], "14pt");
    assert!(
        f.error("update_publication_theme", request, StatusCode::BAD_REQUEST)
            .contains("Version conflict")
    );
    let duplicate = f.call("duplicate_publication_theme", json!({"themeId":"classic","revision":builtin["summary"]["revision"],"name":"Custom Classic"}));
    assert_ne!(duplicate["summary"]["id"], "classic");
    assert_eq!(duplicate["assets"], builtin["assets"]);
    f.error("update_publication_theme", json!({"themeId":"classic","revision":builtin["summary"]["revision"],"definition":builtin["definition"]}), StatusCode::BAD_REQUEST);
    assert_eq!(
        f.call(
            "delete_publication_theme",
            json!({"themeId":id,"revision":updated["summary"]["revision"]})
        ),
        json!({"deleted":true})
    );
    f.error(
        "read_publication_theme_package",
        json!({"themeId":id}),
        StatusCode::BAD_REQUEST,
    );
    assert_eq!(
        f.call("list_publication_themes", json!({}))
            .as_array()
            .unwrap()
            .len(),
        3
    );
    assert_eq!(
        crate::engine::project::item::list_project_item_ids(f.root.path()).unwrap(),
        initial_items
    );
}

#[test]
fn default_workspace_selection_preserves_normal_settings_and_workspace_integrity() {
    let f = Fixture::new(true);
    let main = f.call("read_workspace_context", json!({}))["defaultWorkspace"].clone();
    assert_eq!(main["meta"]["title"], "All notes");
    let mut content = main["content"].clone();
    let view = content["widgets"]
        .as_array()
        .unwrap()
        .iter()
        .position(|widget| widget["config"]["kind"] == "layout")
        .unwrap();
    content["widgets"][view]["config"]["mode"] = json!("table");
    let changed = f.call("update_item", json!({"id":main["id"],"version":main["version"],"meta":{"title":"Writing"},"content":content}));
    assert_eq!(
        f.call("read_workspace_context", json!({}))["defaultWorkspace"]["content"],
        content
    );
    f.error(
        "update_item",
        json!({"id":main["id"],"version":main["version"],"meta":{},"content":content}),
        StatusCode::BAD_REQUEST,
    );
    assert_ne!(changed["version"], main["version"]);
    f.error(
        "delete_item",
        json!({"id":main["id"]}),
        StatusCode::BAD_REQUEST,
    );
    let other = f.call(
        "create_item",
        json!({"meta":{"type":"workspace","title":"Research"},"content":{}}),
    );
    f.call("set_default_workspace", json!({"id":other["id"]}));
    f.call("delete_item", json!({"id":main["id"]}));
    assert_eq!(
        f.call("read_workspace_context", json!({}))["defaultWorkspace"]["id"],
        other["id"]
    );
    f.error(
        "delete_item",
        json!({"id":other["id"]}),
        StatusCode::BAD_REQUEST,
    );
    f.error(
        "set_default_workspace",
        json!({"id":main["id"]}),
        StatusCode::BAD_REQUEST,
    );
    let page = f.page("Page", "Words");
    f.error(
        "set_default_workspace",
        json!({"id":page["id"]}),
        StatusCode::BAD_REQUEST,
    );
    for name in [
        "read_default_view",
        "update_default_view",
        "read_default_workspace",
    ] {
        f.error(name, json!({}), StatusCode::NOT_FOUND);
    }
    let mut f = f;
    f.reopen(true);
    assert_eq!(
        f.call("read_workspace_context", json!({}))["defaultWorkspace"]["id"],
        other["id"]
    );
}

#[test]
fn removing_the_default_workspace_persists_its_explicit_replacement() {
    let mut f = Fixture::new(false);
    let main = f.call("read_workspace_context", json!({}))["defaultWorkspace"].clone();
    let other = f.call(
        "create_item",
        json!({"meta":{"type":"workspace","title":"Replacement"},"content":{}}),
    );
    f.error(
        "remove_workspace",
        json!({"id":main["id"],"version":main["version"]}),
        StatusCode::BAD_REQUEST,
    );
    let result = f.call(
        "remove_workspace",
        json!({"id":main["id"],"version":main["version"],"replacementId":other["id"]}),
    );
    assert_eq!(result["defaultWorkspace"], other);
    f.reopen(false);
    assert_eq!(f.call("read_workspace_context", json!({})), result);
    assert_eq!(
        f.call("read_item", json!({"id":main["id"]}))["meta"]["state"],
        "archived"
    );
    f.error(
        "remove_workspace",
        json!({"id":other["id"],"version":other["version"]}),
        StatusCode::BAD_REQUEST,
    );
}

#[test]
fn theme_inspection_supports_all_targets_without_an_index() {
    let f = Fixture::new(false);
    let mut hashes = std::collections::HashSet::new();
    for format in ["pdf", "html", "site"] {
        let theme = f.call(
            "read_publication_theme",
            json!({"themeId":"classic","format":format}),
        );
        assert_eq!(theme["id"], "classic");
        assert_eq!(theme["origin"], "builtin");
        assert_eq!(
            theme["resolved"]["styles"]["body"]["fontFamily"],
            "source-serif-4"
        );
        let hash = theme["sha256"].as_str().unwrap();
        assert_eq!(hash.len(), 64);
        assert!(hash.chars().all(|c| c.is_ascii_hexdigit()));
        hashes.insert(hash.to_owned());
    }
    assert_eq!(hashes.len(), 1);
    assert!(
        f.error(
            "read_publication_theme",
            json!({"themeId":"classic","format":"word"}),
            StatusCode::BAD_REQUEST
        )
        .contains("Choose PDF, Word, HTML, or site")
    );
    let status: Value = serde_json::from_str(&f.app.status().unwrap()).unwrap();
    assert!(status["index"].is_null());
}
