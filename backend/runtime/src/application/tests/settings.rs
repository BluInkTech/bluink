// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use super::*;

#[test]
fn removing_workspaces_requires_a_fresh_revision_and_a_valid_default_replacement() {
    let project = TestProject::new("remove-workspace");
    let app = project.application();
    app.initialize_sync(false).unwrap();
    let context = execute(&app, "read_workspace_context", json!({})).unwrap();
    let main = &context["defaultWorkspace"];
    let other = execute(
        &app,
        "create_item",
        json!({"meta":{"type":"workspace","title":"Other"},"content":{}}),
    )
    .unwrap();
    let page = execute(&app, "create_item", json!({"meta":{"type":"page","title":"Retained note"},"content":{"content":{"type":"doc","content":[{"type":"paragraph"}]}}})).unwrap();
    for replacement in [
        Value::Null,
        main["id"].clone(),
        page["id"].clone(),
        json!("missing"),
    ] {
        assert!(
            execute(
                &app,
                "remove_workspace",
                json!({"id":main["id"],"version":main["version"],"replacementId":replacement})
            )
            .is_err()
        );
        assert_eq!(
            execute(&app, "read_workspace_context", json!({})).unwrap(),
            context
        );
    }
    let changed = execute(&app,"update_item",json!({"id":main["id"],"version":main["version"],"meta":{"title":"Renamed"},"content":main["content"]})).unwrap();
    assert!(
        execute(
            &app,
            "remove_workspace",
            json!({"id":main["id"],"version":main["version"],"replacementId":other["id"]})
        )
        .is_err()
    );
    assert_eq!(
        execute(&app, "read_workspace_context", json!({})).unwrap()["defaultWorkspace"],
        changed
    );
    let removed = execute(
        &app,
        "remove_workspace",
        json!({"id":changed["id"],"version":changed["version"],"replacementId":other["id"]}),
    )
    .unwrap();
    assert_eq!(removed["defaultWorkspace"], other);
    let archived = execute(&app, "read_item", json!({"id":main["id"]})).unwrap();
    assert_eq!(archived["meta"]["state"], "archived");
    assert_eq!(archived["content"], changed["content"]);
    assert_eq!(
        execute(&app, "read_item", json!({"id":page["id"]})).unwrap(),
        page
    );
    assert!(
        execute(
            &app,
            "remove_workspace",
            json!({"id":other["id"],"version":other["version"]})
        )
        .is_err()
    );
    assert_eq!(
        execute(&app, "read_workspace_context", json!({})).unwrap(),
        removed
    );
    app.stop_sync().unwrap();
    project.close().unwrap();
}

#[test]
fn workspace_context_reports_fresh_default_identity_and_settings_without_an_index() {
    let project = TestProject::new("workspace-context");
    let app = project.application();
    app.initialize_sync(false).unwrap();
    let first = execute(&app, "read_workspace_context", json!({})).unwrap();
    let config = crate::engine::project::config::read_project_config(&app.root)
        .unwrap()
        .unwrap();
    assert_eq!(first["projectId"], config["id"]);
    assert_eq!(
        first["defaultWorkspace"]["id"],
        config["defaultWorkspaceId"]
    );
    assert_eq!(first["defaultWorkspace"]["meta"]["title"], "All notes");
    let other = execute(&app, "create_item", json!({"meta":{"type":"workspace","title":"Research","summary":"Research workspace"},"content":{}})).unwrap();
    execute(&app, "set_default_workspace", json!({"id":other["id"]})).unwrap();
    let fresh = execute(&app, "read_workspace_context", json!({})).unwrap();
    assert_eq!(fresh["projectId"], first["projectId"]);
    assert_eq!(fresh["defaultWorkspace"], other);
    let changed = execute(&app, "update_item", json!({"id":other["id"],"version":other["version"],"meta":{"title":"Writing","summary":"Continue here"},"content":other["content"]})).unwrap();
    assert_eq!(
        execute(&app, "read_workspace_context", json!({})).unwrap()["defaultWorkspace"],
        changed
    );
    assert!(app.state().unwrap().index.is_none());
    std::fs::remove_file(app.root.join("bluink.json")).unwrap();
    assert!(matches!(
        execute(&app, "read_workspace_context", json!({})),
        Err(ApplicationError::Internal(_))
    ));
    app.stop_sync().unwrap();
    project.close().unwrap();
}

#[test]
fn default_workspace_uses_normal_versioned_settings_and_reads_fresh_canonical_values() {
    let project = TestProject::new("default-workspace-settings");
    let app = project.application();
    app.initialize_sync(true).unwrap();
    let initial = crate::engine::project::config::read_project_config(&app.root)
        .unwrap()
        .unwrap();
    let main =
        execute(&app, "read_workspace_context", json!({})).unwrap()["defaultWorkspace"].clone();
    assert_eq!(main["id"], initial["defaultWorkspaceId"]);
    assert_eq!(main["meta"]["title"], "All notes");
    let mut content = main["content"].clone();
    let view = content["widgets"]
        .as_array()
        .unwrap()
        .iter()
        .position(|widget| widget["config"]["kind"] == "layout")
        .unwrap();
    assert_eq!(content["widgets"][view]["config"]["mode"], "cards");
    content["widgets"][view]["config"]["mode"] = json!("table");
    content["query"] = json!(crate::query::document::test_query(
        "#research sort:title-asc"
    ));
    let updated = execute(&app, "update_item", json!({"id":main["id"],"version":main["version"],"meta":{"title":"Writing"},"content":content})).unwrap();
    let fresh =
        execute(&app, "read_workspace_context", json!({})).unwrap()["defaultWorkspace"].clone();
    assert_eq!(fresh["content"], content);
    assert_eq!(fresh["meta"]["title"], "Writing");
    assert_ne!(updated["version"], main["version"]);
    assert!(
        execute(
            &app,
            "update_item",
            json!({"id":main["id"],"version":main["version"],"meta":{},"content":content})
        )
        .is_err()
    );
    let other = execute(
        &app,
        "create_item",
        json!({"meta":{"type":"workspace","title":"Research"},"content":{}}),
    )
    .unwrap();
    execute(&app, "set_default_workspace", json!({"id":other["id"]})).unwrap();
    assert_eq!(
        execute(&app, "read_workspace_context", json!({})).unwrap()["defaultWorkspace"]["id"],
        other["id"]
    );
    let saved = crate::engine::project::config::read_project_config(&app.root)
        .unwrap()
        .unwrap();
    for key in ["id", "history", "analytics"] {
        assert_eq!(saved[key], initial[key]);
    }
    let mut external = saved;
    external["defaultWorkspaceId"] = main["id"].clone();
    std::fs::write(
        app.root.join("bluink.json"),
        serde_json::to_vec(&external).unwrap(),
    )
    .unwrap();
    assert_eq!(
        execute(&app, "read_workspace_context", json!({})).unwrap()["defaultWorkspace"]["id"],
        main["id"]
    );
    std::fs::remove_file(app.root.join("bluink.json")).unwrap();
    assert!(matches!(
        execute(&app, "read_workspace_context", json!({})),
        Err(ApplicationError::Internal(_))
    ));
    app.stop_sync().unwrap();
    project.close().unwrap();
}

#[test]
fn theme_snapshots_support_all_targets_without_initializing_an_index() {
    let project = TestProject::new("theme-settings");
    let app = project.application();
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
            theme["resolved"]["styles"]["body"]["fontFamily"],
            "source-serif-4"
        );
        assert_eq!(theme["sha256"].as_str().unwrap().len(), 64);
        if let Some(previous) = &hash {
            assert_eq!(&theme["sha256"], previous);
        }
        hash = Some(theme["sha256"].clone());
    }
    let theme = execute(&app, "read_publication_theme", json!({"themeId":"classic", "format":"html",
        "overrides":{"shared":{"root":{"fontSize":"15pt"}},"targets":{"html":{"page":{"measureCharacters":65}}}}
    })).unwrap();
    assert_eq!(theme["resolved"]["rootFontSizePt"], 15.0);
    assert_eq!(theme["resolved"]["page"]["measureCharacters"], 65);
    let invalid_word = execute(&app, "read_publication_theme", json!({"themeId":"classic", "format":"html",
        "overrides":{"shared":{},"targets":{"docx":{"styles":{"body":{"fontSize":{"ref":"missing-word-token"}}}}}}
    })).unwrap_err();
    assert!(
        matches!(invalid_word, ApplicationError::InvalidRequest(ref message) if message.starts_with("Word:") && message.contains("missing-word-token"))
    );
    assert!(matches!(
        execute(
            &app,
            "read_publication_theme",
            json!({"themeId":"classic", "format":"html",
                "overrides":{"shared":{"root":{"fontSize":"1rem"}},"targets":{}}
            })
        ),
        Err(ApplicationError::InvalidRequest(_))
    ));
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
    project.close().unwrap();
}
