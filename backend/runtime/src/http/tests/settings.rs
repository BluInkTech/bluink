// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use super::support::*;
use axum::http::StatusCode;
use serde_json::{Value, json};

#[test]
fn default_view_settings_preserve_saved_values_and_read_external_changes() {
    let f = Fixture::new(true);
    assert_eq!(
        f.call("read_default_view", json!({})),
        json!({"query":"sort:modified-desc","mode":"dashboard"})
    );
    let view = json!({"query":"#research sort:title-asc","mode":"kanban","kanban":{"groupBy":"meta.status","columns":[{"id":"draft"}]}});
    assert_eq!(f.call("update_default_view", json!({"content":view})), view);
    assert_eq!(f.call("read_default_view", json!({})), view);
    for content in [json!({"query":5}), json!({"query":"+unknown_field:value"})] {
        f.error(
            "update_default_view",
            json!({"content":content}),
            StatusCode::BAD_REQUEST,
        );
    }
    assert_eq!(f.call("read_default_view", json!({})), view);
    assert_eq!(f.call("read_items", json!({})), json!([]));
    let reopened =
        crate::application::BluinkApplication::new(crate::application::ApplicationOptions {
            project_root: f.root.path().to_string_lossy().into_owned(),
            timezone: Some("UTC".into()),
            cache_location: None,
            public_dir: None,
            data_dir: None,
            web_font_folder: None,
            pdf_font_folder: None,
            typst_binary: None,
        })
        .unwrap();
    f.runtime.block_on(reopened.initialize(false)).unwrap();
    let read = crate::application::operation::Request::decode("read_default_view", b"{}").unwrap();
    assert_eq!(
        serde_json::to_value(reopened.inner.execute(read).unwrap()).unwrap(),
        view
    );
    f.runtime.block_on(reopened.stop()).unwrap();
    let path = f.root.path().join("bluink.json");
    let mut config: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    config["defaultView"] = json!({"query":"sort:modified-asc","mode":"table"});
    std::fs::write(&path, config.to_string()).unwrap();
    assert_eq!(
        f.call("read_default_view", json!({})),
        config["defaultView"]
    );
    config["analytics"]["eventRetentionDays"] = json!(0);
    std::fs::write(&path, config.to_string()).unwrap();
    assert!(
        f.error(
            "read_default_view",
            json!({}),
            StatusCode::INTERNAL_SERVER_ERROR
        )
        .contains("invalid analytics settings")
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
            theme["settings"]["typography"]["bodyFamily"],
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
        .contains("Choose PDF, HTML, or site")
    );
    let status: Value = serde_json::from_str(&f.app.status().unwrap()).unwrap();
    assert!(status["index"].is_null());
}
