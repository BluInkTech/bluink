// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use super::support::{Fixture, document};
use axum::http::StatusCode;
use serde_json::{Value, json};

#[test]
fn view_modes_are_cards_table_kanban_and_list_without_obsolete_aliases() {
    let f = Fixture::new(false);
    let workspace = f.call(
        "create_item",
        json!({"meta":{"type":"workspace","title":"Views"},"content":{}}),
    );
    let widgets = workspace["content"]["widgets"].as_array().unwrap();
    let pinned: Vec<_> = widgets
        .iter()
        .filter(|widget| widget["topBar"] == true)
        .map(|widget| widget["config"]["mode"].clone())
        .collect();
    assert_eq!(pinned, [json!("cards"), json!("table"), json!("kanban")]);
    let view = widgets
        .iter()
        .position(|widget| widget["config"]["kind"] == "layout")
        .unwrap();
    let change = |content: Value| json!({"id":workspace["id"],"version":workspace["version"],"meta":workspace["meta"],"content":content});
    for mode in ["dashboard", "calendar"] {
        let mut content = workspace["content"].clone();
        content["widgets"][view]["config"]["mode"] = json!(mode);
        f.error("update_item", change(content), StatusCode::BAD_REQUEST);
    }
    // Presentation and Home belong to widgets, never to the workspace itself.
    for (key, value) in [
        ("mode", json!("table")),
        ("layouts", json!([])),
        ("home", json!({"version": 1, "widgets": []})),
    ] {
        let mut content = workspace["content"].clone();
        content[key] = value;
        f.error("update_item", change(content), StatusCode::BAD_REQUEST);
    }
    assert_eq!(
        f.call("read_item", json!({"id":workspace["id"]})),
        workspace
    );
}

#[test]
fn workspace_pins_scope_tasks_activity_day_details_and_label_passages() {
    let f = Fixture::new(false);
    let main = f.call("read_workspace_context", json!({}))["defaultWorkspace"].clone();
    let other = f.call(
        "create_item",
        json!({"meta":{"type":"workspace","title":"Other"},"content":{}}),
    );
    let date = f.call("read_dashboard_analytics", json!({}))["days"]
        .as_array()
        .unwrap()
        .last()
        .unwrap()["date"]
        .clone();
    let mut notes = Vec::new();
    for (workspace, text) in [(&main, "one two"), (&other, "three four five")] {
        let mut content = document(text);
        content["content"][0]["attrs"] = json!({"labels":["source"]});
        let note = f.call(
            "create_item",
            json!({"meta":{"type":"page","title":text},"pins":[workspace["id"]],"content":content}),
        );
        f.call("mutate_note_task", json!({"noteId":note["id"],"version":note["version"],"change":{"kind":"create","text":text}}));
        notes.push(note);
    }
    f.initialize_index();
    let query = crate::query::document::test_query("+type:page +state:active +is:pinned");
    for (index, workspace) in [&main, &other].into_iter().enumerate() {
        let tasks = f.call(
            "query_note_tasks",
            json!({"query":query,"pinScopeId":workspace["id"],"status":"open"}),
        );
        assert_eq!(tasks["count"], 1);
        assert_eq!(tasks["items"][0]["noteId"], notes[index]["id"]);
        let activity = f.call(
            "read_dashboard_analytics",
            json!({"query":query,"pinScopeId":workspace["id"],"startDate":date,"endDate":date}),
        );
        assert_eq!(activity["summary"]["wordsAdded"], index + 2);
        let details = f.call(
            "read_dashboard_day_details",
            json!({"query":query,"pinScopeId":workspace["id"],"date":date}),
        );
        assert_eq!(details["items"].as_array().unwrap().len(), 1);
        assert_eq!(details["items"][0]["id"], notes[index]["id"]);
        let passages = f.call("query_label_passages", json!({"label":"source","query":query,"pinScopeId":workspace["id"],"limit":1,"offset":0}));
        assert_eq!(passages["count"], 1);
        assert_eq!(passages["items"][0]["itemId"], notes[index]["id"]);
    }
    let all_passages = f.call(
        "query_label_passages",
        json!({"label":"source","limit":1,"offset":1}),
    );
    assert_eq!(all_passages["count"], 2);
    assert_eq!(all_passages["items"].as_array().unwrap().len(), 1);
}

#[test]
fn query_pin_scope_can_preserve_membership_without_overriding_requested_order() {
    let f = Fixture::new(false);
    let main = f.call("read_workspace_context", json!({}))["defaultWorkspace"].clone();
    let other = f.call(
        "create_item",
        json!({"meta":{"type":"workspace","title":"Other"},"content":{}}),
    );
    let zulu = f.call("create_item", json!({"meta":{"type":"page","title":"Zulu"},"pins":[main["id"]],"content":document("Pinned to Main")}));
    let alpha = f.page("Alpha", "Not pinned");
    let bravo = f.call("create_item", json!({"meta":{"type":"page","title":"Bravo"},"pins":[other["id"]],"content":document("Pinned elsewhere")}));
    f.initialize_index();
    let request = json!({"query":crate::query::document::test_query("+type:page +state:active sort:title-asc"), "pinScopeId":main["id"],"limit":10});
    let priority = f.call("query_items", request.clone());
    assert_eq!(priority["items"][0]["id"], zulu["id"]);
    let mut ordered = request.clone();
    ordered["prioritizePins"] = json!(false);
    let found = f.call("query_items", ordered);
    assert_eq!(
        found["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| item["id"].clone())
            .collect::<Vec<_>>(),
        vec![alpha["id"].clone(), bravo["id"].clone(), zulu["id"].clone()]
    );
    assert!(found.get("pinnedCount").is_none());
    assert_eq!(
        f.call("query_items", request)["items"][0]["id"],
        zulu["id"],
        "Cache entries keep pin-priority modes separate"
    );
    for (workspace, expected) in [(&main, &zulu), (&other, &bravo)] {
        let found = f.call("query_items", json!({"query":crate::query::document::test_query("+type:page +state:active +is:pinned"),"pinScopeId":workspace["id"],"prioritizePins":false,"limit":10}));
        assert_eq!(found["count"], 1);
        assert_eq!(found["items"][0]["id"], expected["id"]);
    }
}

#[test]
fn workspace_note_candidates_are_bounded_and_require_an_active_workspace_without_an_index() {
    let f = Fixture::new(false);
    let main = f.call("read_workspace_context", json!({}))["defaultWorkspace"].clone();
    assert_eq!(
        f.call(
            "resolve_workspace_notes",
            json!({"workspaceId":main["id"], "ids":[]})
        ),
        json!([])
    );
    let valid_id = "00000000-0000-7000-8000-000000000501";
    for extra in [
        json!({"ids":vec![valid_id; 51]}),
        json!({"ids":["../invalid"]}),
        json!({"ids":[],"limit":0}),
        json!({"ids":[],"limit":51}),
        json!({"ids":[],"query":{}}),
    ] {
        let mut request = extra;
        request["workspaceId"] = main["id"].clone();
        f.error("resolve_workspace_notes", request, StatusCode::BAD_REQUEST);
    }
    let page = f.page("Page", "Only pages can be candidates.");
    let archived = f.call(
        "create_item",
        json!({"meta":{"type":"workspace","title":"Retired"},"content":{}}),
    );
    f.call("delete_item", json!({"id":archived["id"]}));
    for id in [&page["id"], &archived["id"], &json!(valid_id), &json!("")] {
        f.error(
            "resolve_workspace_notes",
            json!({"workspaceId":id,"ids":[]}),
            StatusCode::BAD_REQUEST,
        );
    }
}

#[test]
fn workspace_note_resolution_preserves_candidate_order_native_membership_and_pin_scope() {
    let mut f = Fixture::new(false);
    f.call(
        "create_property",
        json!({"key":"author","valueType":"text"}),
    );
    let workspace = f.call("create_item", json!({"meta":{"type":"workspace","title":"Research"},"content":{"query":crate::query::document::test_query("+type:page +state:active +author:Mary sort:title-asc")}}));
    let pins = f.call("create_item", json!({"meta":{"type":"workspace","title":"Pinned"},"content":{"query":crate::query::document::test_query("+is:pinned")}}));
    let first = f.call("create_item", json!({"meta":{"type":"page","title":"Alpha","author":"Mary"},"content":document("First candidate"),"pins":[workspace["id"]]}));
    let second = f.call("create_item", json!({"meta":{"type":"page","title":"Beta","author":"Mary"},"content":document("Second candidate"),"pins":[pins["id"]]}));
    let outside = f.call("create_item", json!({"meta":{"type":"page","title":"Outside","author":"John"},"content":document("Outside criteria despite its pin"),"pins":[workspace["id"]]}));
    let archived = f.call("create_item", json!({"meta":{"type":"page","state":"archived","title":"Archived","author":"Mary"},"content":document("Archived candidate")}));
    let removed = f.call("create_item", json!({"meta":{"type":"page","title":"Removed source","author":"Mary"},"content":document("Missing canonical record")}));
    f.initialize_index();
    std::fs::remove_file(f.item_path(&removed["id"])).unwrap();
    let candidates = json!([
        "00000000-0000-7000-8000-000000000501",
        removed["id"],
        outside["id"],
        second["id"],
        first["id"],
        second["id"],
        archived["id"],
        workspace["id"]
    ]);
    let found = f.call(
        "resolve_workspace_notes",
        json!({"workspaceId":workspace["id"],"ids":candidates}),
    );
    let ids = |items: &Value| {
        items
            .as_array()
            .unwrap()
            .iter()
            .map(|item| item["id"].clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(ids(&found), vec![second["id"].clone(), first["id"].clone()]);
    assert!(
        found
            .as_array()
            .unwrap()
            .iter()
            .all(|item| item["content"] == json!({}))
    );
    assert_eq!(f.call("read_item", json!({"id":first["id"]})), first);
    assert_eq!(
        ids(&f.call(
            "resolve_workspace_notes",
            json!({"workspaceId":workspace["id"],"ids":candidates,"limit":1})
        )),
        vec![second["id"].clone()]
    );
    assert_eq!(
        ids(&f.call(
            "resolve_workspace_notes",
            json!({"workspaceId":pins["id"],"ids":[first["id"],second["id"],outside["id"]]})
        )),
        vec![second["id"].clone()]
    );
    f.call(
        "update_item_meta",
        json!({"id":first["id"],"version":first["version"],"meta":{"author":"John"}}),
    );
    f.settled();
    assert_eq!(
        ids(&f.call(
            "resolve_workspace_notes",
            json!({"workspaceId":workspace["id"],"ids":candidates})
        )),
        vec![second["id"].clone()]
    );
    f.call("delete_item", json!({"id":second["id"]}));
    f.settled();
    f.reopen(true);
    assert_eq!(
        f.call(
            "resolve_workspace_notes",
            json!({"workspaceId":workspace["id"],"ids":candidates})
        ),
        json!([])
    );
}
