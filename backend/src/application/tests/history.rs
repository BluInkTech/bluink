// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk
use super::*;

fn create_page(app: &Application) -> Value {
    execute(
        app,
        "create_item",
        json!({
            "meta": {"type": "page", "title": "First draft"},
            "content": {"document": {"type": "doc", "content": [{"type": "paragraph"}]}}
        }),
    )
    .unwrap()
}

fn preview_request(app: &Application, page: &Value, checkpoint: &Value) -> Value {
    let preview = execute(
        app,
        "preview_history_restore",
        json!({
            "commitId": checkpoint["head"], "itemId": page["id"]
        }),
    )
    .unwrap();
    json!({"commitId": checkpoint["head"], "itemId": page["id"],
        "expectedHead": preview["expectedHead"], "expectedGeneration": preview["expectedGeneration"]})
}

#[test]
fn both_applications_see_the_restored_revision_and_acknowledged_checkpoint() {
    let project = TestProject::new("history-restore");
    let server = project.application();
    let cli = project.application();
    server.initialize_sync().unwrap();
    cli.initialize_sync().unwrap();
    let page = create_page(&server);
    let historical = execute(&server, "checkpoint_history", json!({})).unwrap();
    execute(
        &server,
        "update_item_meta",
        json!({
            "id":page["id"], "version": page["version"], "meta": {"title":"Changed"}
        }),
    )
    .unwrap();
    let request = preview_request(&cli, &page, &historical);
    let status = execute(&cli, "restore_history", request).unwrap();
    let restored = execute(&server, "read_item", json!({"id":page["id"]})).unwrap();
    assert_eq!(restored["meta"]["title"], "First draft");
    assert_ne!(restored["version"], page["version"]);
    assert_ne!(status["head"], historical["head"]);
    assert_eq!(status["dirty"], false);
    assert_eq!(
        execute(&server, "read_history_status", json!({})).unwrap()["head"],
        status["head"]
    );
    server.stop_sync().unwrap();
    cli.stop_sync().unwrap();
    project.close().unwrap();
}

#[test]
fn a_save_after_the_restore_preview_survives_the_stale_request() {
    let project = TestProject::new("history-stale-preview");
    let app = project.application();
    app.initialize_sync().unwrap();
    let page = create_page(&app);
    let checkpoint = execute(&app, "checkpoint_history", json!({})).unwrap();
    let request = preview_request(&app, &page, &checkpoint);
    execute(
        &app,
        "update_item_meta",
        json!({
            "id": page["id"], "version": page["version"], "meta": {"title":"Keep this save"}
        }),
    )
    .unwrap();
    assert!(execute(&app, "restore_history", request).is_err());
    assert_eq!(
        execute(&app, "read_item", json!({"id":page["id"]})).unwrap()["meta"]["title"],
        "Keep this save"
    );
    app.stop_sync().unwrap();
    project.close().unwrap();
}
