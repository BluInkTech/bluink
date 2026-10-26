// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use super::*;

const SETUPS: [(&str, &str); 5] = [
    (
        "PARA",
        include_str!("../../../../vendor/workspace-setups/para.json"),
    ),
    (
        "GTD",
        include_str!("../../../../vendor/workspace-setups/gtd.json"),
    ),
    (
        "Writing studio",
        include_str!("../../../../vendor/workspace-setups/writing-studio.json"),
    ),
    (
        "Research notes",
        include_str!("../../../../vendor/workspace-setups/research-notes.json"),
    ),
    (
        "Content calendar",
        include_str!("../../../../vendor/workspace-setups/content-calendar.json"),
    ),
];

fn setup(name: &str) -> Value {
    serde_json::from_str(SETUPS.iter().find(|(setup, _)| *setup == name).unwrap().1).unwrap()
}

fn templates(app: &Application) -> Vec<Value> {
    let found = execute(
        app,
        "query_items",
        json!({"query": {"version": 1, "clauses": [{"polarity": "required", "predicate": {"kind": "field", "field": "type", "operator": "match", "value": "template"}}], "sort": []}, "limit": 500}),
    )
    .unwrap();
    found["items"].as_array().unwrap().clone()
}

#[test]
fn shipped_setups_create_their_properties_templates_and_workspace_together() {
    let project = TestProject::new("workspace-setups");
    let app = project.application();
    app.initialize_sync(true).unwrap();
    for (name, _) in SETUPS {
        let source = setup(name);
        let workspace = execute(
            &app,
            "create_workspace_from_setup",
            json!({"setup": source, "title": format!("My {name}")}),
        )
        .unwrap_or_else(|error| panic!("{name}: {error}"));
        assert_eq!(workspace["meta"]["title"], format!("My {name}"));
        assert_eq!(workspace["meta"]["type"], "workspace");
        let content = &workspace["content"];
        assert_eq!(
            content["widgets"].as_array().unwrap().len(),
            source["workspace"]["widgets"].as_array().unwrap().len()
        );
        // Offers name the new templates by ID, in the setup's order.
        let offered = content["templates"]["offered"].as_array().unwrap();
        assert_eq!(
            offered.len(),
            source["workspace"]["templates"]["offered"]
                .as_array()
                .unwrap()
                .len()
        );
        for id in offered {
            let template = execute(&app, "read_item", json!({"id": id})).unwrap();
            assert_eq!(template["meta"]["type"], "template", "{name}");
        }
        assert!(offered.contains(&content["templates"]["default"]));
    }
    let fields = execute(&app, "read_metadata_fields", json!({})).unwrap();
    for key in [
        "para",
        "stage",
        "area",
        "context",
        "draft_stage",
        "research_status",
        "content_status",
        "channel",
    ] {
        assert!(
            fields
                .as_array()
                .unwrap()
                .iter()
                .any(|field| field["key"] == key),
            "{key} is declared"
        );
    }
}

#[test]
fn setups_reuse_existing_properties_and_name_templates_in_create_buttons() {
    let project = TestProject::new("workspace-setup-reuse");
    let app = project.application();
    app.initialize_sync(true).unwrap();
    execute(
        &app,
        "create_property",
        json!({"key": "area", "valueType": "select", "options": ["finances", "health", "home", "learning", "writing", "work"]}),
    )
    .unwrap();
    let source = setup("PARA");
    let first = execute(
        &app,
        "create_workspace_from_setup",
        json!({"setup": source, "title": "Second brain"}),
    )
    .unwrap();
    // The project's own area, with its extra option, is reused, never replaced.
    let fields = execute(&app, "read_metadata_fields", json!({})).unwrap();
    let area = fields
        .as_array()
        .unwrap()
        .iter()
        .find(|field| field["key"] == "area")
        .unwrap();
    assert!(area["options"].as_array().unwrap().contains(&json!("work")));
    let buttons = first["content"]["widgets"]
        .as_array()
        .unwrap()
        .iter()
        .find(|widget| widget["config"]["kind"] == "create_buttons")
        .unwrap();
    let ids = buttons["config"]["templateIds"].as_array().unwrap();
    // The setup's buttons name its templates, now by their new IDs.
    assert_eq!(
        ids,
        first["content"]["templates"]["offered"].as_array().unwrap()
    );
    // A second workspace from the same setup gets its own templates.
    execute(
        &app,
        "create_workspace_from_setup",
        json!({"setup": setup("PARA"), "title": "Another brain"}),
    )
    .unwrap();
    assert_eq!(templates(&app).len(), 6);
}

#[test]
fn a_refused_setup_writes_nothing() {
    let project = TestProject::new("workspace-setup-refused");
    let app = project.application();
    app.initialize_sync(true).unwrap();
    let before = templates(&app).len();
    let mut built_in = setup("PARA");
    built_in["properties"]
        .as_array_mut()
        .unwrap()
        .push(json!({"key": "due_date", "valueType": "date"}));
    let mut bad_value = setup("PARA");
    bad_value["templates"][0]["values"]["para"] = json!("someday");
    let mut unknown_key = setup("PARA");
    unknown_key["workspace"]["templates"]["offered"] = json!(["project", "nowhere"]);
    for (setup, message) in [
        (built_in, "due_date is built in"),
        (bad_value, "Property para must use a declared option"),
        (
            unknown_key,
            "template nowhere, which the setup does not include",
        ),
    ] {
        let error = execute(
            &app,
            "create_workspace_from_setup",
            json!({"setup": setup, "title": "Refused"}),
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains(message), "{error}");
    }
    assert_eq!(templates(&app).len(), before);
    let fields = execute(&app, "read_metadata_fields", json!({})).unwrap();
    assert!(
        !fields
            .as_array()
            .unwrap()
            .iter()
            .any(|field| field["key"] == "para")
    );
    let error = execute(
        &app,
        "create_workspace_from_setup",
        json!({"setup": {"format": "something-else"}, "title": "Nope"}),
    )
    .unwrap_err()
    .to_string();
    assert!(error.contains("not a valid workspace setup"), "{error}");
}
