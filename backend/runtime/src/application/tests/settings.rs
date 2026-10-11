// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use super::*;

#[test]
fn default_view_validation_preserves_configuration_and_reads_fresh_canonical_values() {
    let app = application("default-view-settings");
    app.initialize_sync(true).unwrap();
    let initial = crate::engine::project::config::read_project_config(&app.root)
        .unwrap()
        .unwrap();
    assert_eq!(
        execute(&app, "read_default_view", json!({})).unwrap(),
        json!({"query":"sort:modified-desc", "mode":"dashboard"})
    );
    let view = json!({"query":"#research sort:title-asc", "mode":"kanban", "kanban":{"groupBy":"meta.status", "columns":[{"id":"draft"}]}});
    assert_eq!(
        execute(&app, "update_default_view", json!({"content":view})).unwrap(),
        view
    );
    assert_eq!(execute(&app, "read_default_view", json!({})).unwrap(), view);
    for content in [
        Value::Null,
        json!([]),
        json!({}),
        json!({"query":4}),
        json!({"query":"+unknown_field:value"}),
    ] {
        assert!(matches!(
            execute(&app, "update_default_view", json!({"content":content})),
            Err(ApplicationError::InvalidRequest(_))
        ));
        assert_eq!(execute(&app, "read_default_view", json!({})).unwrap(), view);
    }
    let saved = crate::engine::project::config::read_project_config(&app.root)
        .unwrap()
        .unwrap();
    for key in ["id", "history", "analytics"] {
        assert_eq!(saved[key], initial[key]);
    }
    assert_eq!(
        std::fs::read_dir(app.root.join("items")).unwrap().count(),
        0
    );
    let mut external = saved;
    external["defaultView"] = json!({"query":"sort:modified-asc", "mode":"table"});
    std::fs::write(
        app.root.join("bluink.json"),
        serde_json::to_vec(&external).unwrap(),
    )
    .unwrap();
    assert_eq!(
        execute(&app, "read_default_view", json!({})).unwrap(),
        external["defaultView"]
    );
    std::fs::remove_file(app.root.join("bluink.json")).unwrap();
    assert!(matches!(
        execute(&app, "read_default_view", json!({})),
        Err(ApplicationError::Internal(_))
    ));
    assert!(matches!(
        execute(&app, "update_default_view", json!({"content":view})),
        Err(ApplicationError::Internal(_))
    ));
    app.stop_sync().unwrap();
    std::fs::remove_dir_all(&app.root).unwrap();
}

#[test]
fn theme_snapshots_support_all_targets_without_initializing_an_index() {
    let app = application("theme-settings");
    app.initialize_sync(false).unwrap();
    let mut hash = None;
    for format in ["pdf", "html", "site"] {
        let theme = execute(
            &app,
            "read_publication_theme",
            json!({"themeId":"classic", "format":format}),
        )
        .unwrap();
        assert_eq!(theme["origin"], "builtin");
        assert_eq!(theme["id"], "classic");
        assert_eq!(
            theme["settings"]["typography"]["bodyFamily"],
            "source-serif-4"
        );
        assert_eq!(theme["sha256"].as_str().unwrap().len(), 64);
        if let Some(previous) = &hash {
            assert_eq!(&theme["sha256"], previous);
        }
        hash = Some(theme["sha256"].clone());
    }
    assert!(matches!(
        execute(
            &app,
            "read_publication_theme",
            json!({"themeId":"classic", "format":"word"})
        ),
        Err(ApplicationError::InvalidRequest(_))
    ));
    // Keep theme-loading failures distinct from input validation at the adapter.
    assert!(matches!(
        execute(
            &app,
            "read_publication_theme",
            json!({"themeId":"missing", "format":"pdf"})
        ),
        Err(ApplicationError::Internal(_))
    ));
    assert!(app.state().unwrap().index.is_none());
    app.stop_sync().unwrap();
    std::fs::remove_dir_all(&app.root).unwrap();
}
