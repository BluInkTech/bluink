// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk
use super::*;

fn application(root: &std::path::Path) -> Application {
    Application::new(crate::application::ApplicationOptions {
        project_root: root.join("project").to_string_lossy().into_owned(),
        workspace_template_path: concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/resources/workspace-starter.json"
        )
        .into(),
        user_data_dir: Some(root.join("user").to_string_lossy().into_owned()),
        timezone: Some("UTC".into()),
        public_dir: None,
        web_font_folder: None,
        pdf_font_folder: None,
        docx_font_folder: None,
        typst_binary: None,
    })
    .unwrap()
}

fn create(app: &Application) -> Value {
    let request = serde_json::from_value(json!({"meta":{"type":"page","title":"Base"},"content":{"document":{"type":"doc","content":[{"type":"paragraph","content":[{"type":"text","text":"First"}]},{"type":"paragraph","content":[{"type":"text","text":"Second"}]}]}}})).unwrap();
    serde_json::to_value(app.create_item(request).unwrap()).unwrap()
}

fn draft(page: &Value) -> Value {
    json!({"id":page["id"], "baseRevision":page["version"], "submissionId":uuid::Uuid::now_v7().to_string(), "device":"Test", "draft":{"kind":"replace","item":{"meta":page["meta"],"content":page["content"]}}})
}

fn save(app: &Application, request: &Value) -> Value {
    serde_json::to_value(
        app.merge_item_draft(serde_json::from_value(request.clone()).unwrap())
            .unwrap(),
    )
    .unwrap()
}

fn patch_draft(page: &Value, index: usize, text: &str) -> Value {
    let blocks = page["content"]["document"]["content"].as_array().unwrap();
    let mut block = blocks[index].clone();
    block["content"][0]["text"] = json!(text);
    let id = block["attrs"]["id"].as_str().unwrap().to_owned();
    let order: Vec<_> = blocks
        .iter()
        .map(|block| block["attrs"]["id"].clone())
        .collect();
    json!({"id":page["id"],"baseRevision":page["version"],"submissionId":uuid::Uuid::now_v7().to_string(),"device":"Test",
        "draft":{"kind":"documentPatch","meta":page["meta"],"content":{},"document":{"order":order,"blocks":{id:block}}}})
}

#[test]
fn patch_acknowledgement_retry_and_stale_merge_preserve_complete_intent() {
    let temp = tempfile::tempdir().unwrap();
    let app = application(temp.path());
    app.initialize_sync().unwrap();
    let page = create(&app);
    let left = patch_draft(&page, 0, "Edited first");
    let accepted = save(&app, &left);
    assert_eq!(accepted["kind"], "ack");
    assert!(accepted.get("item").is_none());
    let right = patch_draft(&page, 1, "Edited second");
    let merged = save(&app, &right);
    assert_eq!(merged["kind"], "reconciled");
    let blocks = &merged["item"]["content"]["document"]["content"];
    assert_eq!(blocks[0]["content"][0]["text"], "Edited first");
    assert_eq!(blocks[1]["content"][0]["text"], "Edited second");
    assert_eq!(save(&app, &left), accepted);
    app.stop_sync().unwrap();
    drop(app);
    temp.close().unwrap();
}

fn export_warnings(
    app: &Application,
    page: &str,
) -> Vec<crate::domain::publication::plan::DecisionWarning> {
    use crate::domain::publication::{OutsideLinks, PdfProfile, plan::PublicationFormat};
    let plan = crate::export_jobs::plan_page(
        &app.root,
        page,
        "classic",
        OutsideLinks::TextOnly,
        PublicationFormat::Html,
        PdfProfile::Default,
    )
    .unwrap();
    assert!(plan.decisions_checked);
    plan.warnings
}

#[test]
fn disjoint_drafts_merge_and_retry_returns_original_receipt_after_later_save() {
    let temp = tempfile::tempdir().unwrap();
    let app = application(temp.path());
    app.initialize_sync().unwrap();
    let page = create(&app);
    let mut left = draft(&page);
    left["draft"]["item"]["content"]["document"]["content"][0]["content"][0]["text"] =
        json!("Left paragraph");
    let saved_left = save(&app, &left);
    let mut right = draft(&page);
    right["draft"]["item"]["content"]["document"]["content"][1]["content"][0]["text"] =
        json!("Right paragraph");
    let saved_right = save(&app, &right);
    let blocks = &saved_right["item"]["content"]["document"]["content"];
    assert_eq!(blocks[0]["content"][0]["text"], "Left paragraph");
    assert_eq!(blocks[1]["content"][0]["text"], "Right paragraph");
    assert_eq!(save(&app, &left), saved_left);
    assert_eq!(
        crate::store::snapshots::read_project_item(&app.root, page["id"].as_str().unwrap())
            .unwrap()
            .unwrap()["revision"],
        saved_right["item"]["version"]
    );
    left["draft"]["item"]["meta"]["title"] = json!("Changed retry");
    assert!(
        app.merge_item_draft(serde_json::from_value(left).unwrap())
            .is_err()
    );
    app.stop_sync().unwrap();
    drop(app);
    temp.close().unwrap();
}

#[test]
fn overlapping_drafts_open_one_decision_and_resolution_preserves_other_paragraphs() {
    let temp = tempfile::tempdir().unwrap();
    let app = application(temp.path());
    app.initialize_sync().unwrap();
    let page = create(&app);
    let mut first = draft(&page);
    first["draft"]["item"]["content"]["document"]["content"][0]["content"][0]["text"] =
        json!("First choice");
    save(&app, &first);
    let mut second = draft(&page);
    second["draft"]["item"]["content"]["document"]["content"][0]["content"][0]["text"] =
        json!("Second choice");
    let saved = save(&app, &second);
    assert_eq!(saved["conflictIds"].as_array().unwrap().len(), 1);
    let id = saved["conflictIds"][0].as_str().unwrap().to_owned();
    let warnings = export_warnings(&app, page["id"].as_str().unwrap());
    assert_eq!(warnings.len(), 1);
    assert_eq!(warnings[0].decision_ids, vec![id.clone()]);
    let list = app
        .list_decisions(DecisionsRequest {
            target_id: Some(page["id"].as_str().unwrap().to_owned()),
        })
        .unwrap();
    assert_eq!(list.decisions.len(), 1);
    let preview = app
        .preview_decision(DecisionRequest { id: id.clone() })
        .unwrap();
    assert!(preview.allowed_selections.contains(&Selection::Both));
    let mut stale = preview.expected.clone();
    stale.decision_revision = "superseded-preview".into();
    assert!(matches!(
        app.resolve_decision(ResolveDecisionRequest {
            id: id.clone(),
            expected: stale,
            selection: Selection::Both,
            device: "Test".into()
        }),
        Err(super::super::error::ApplicationError::StaleDecision(_))
    ));
    let resolved = app
        .resolve_decision(ResolveDecisionRequest {
            id,
            expected: preview.expected,
            selection: Selection::Both,
            device: "Test".into(),
        })
        .unwrap();
    let item = serde_json::to_value(&resolved.items[0]).unwrap();
    let blocks = item["content"]["document"]["content"].as_array().unwrap();
    assert_eq!(blocks.len(), 3);
    assert_ne!(blocks[0]["attrs"]["id"], blocks[1]["attrs"]["id"]);
    assert_eq!(blocks[2]["content"][0]["text"], "Second");
    assert_eq!(export_warnings(&app, page["id"].as_str().unwrap()), []);
    assert!(
        app.list_decisions(DecisionsRequest { target_id: None })
            .unwrap()
            .decisions
            .is_empty()
    );
    app.stop_sync().unwrap();
    drop(app);
    temp.close().unwrap();
}

#[test]
fn unavailable_base_and_untrusted_identity_never_write() {
    let temp = tempfile::tempdir().unwrap();
    let app = application(temp.path());
    app.initialize_sync().unwrap();
    let page = create(&app);
    let mut request = draft(&page);
    request["baseRevision"] = json!(uuid::Uuid::now_v7().to_string());
    assert!(
        app.merge_item_draft(serde_json::from_value(request).unwrap())
            .unwrap_err()
            .to_string()
            .contains("base is unavailable")
    );
    let mut request = draft(&page);
    request["draft"]["item"]["id"] = json!(uuid::Uuid::now_v7().to_string());
    assert!(serde_json::from_value::<MergeDraftRequest>(request).is_err());
    assert_eq!(
        crate::store::snapshots::read_project_item(&app.root, page["id"].as_str().unwrap())
            .unwrap()
            .unwrap()["revision"],
        page["version"]
    );
    app.stop_sync().unwrap();
    drop(app);
    temp.close().unwrap();
}
