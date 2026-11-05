// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! Regression coverage migrated from the obsolete Node Publication facade.

use super::*;
use crate::domain::publication::{OutsideLinks, PdfProfile, plan::PublicationFormat};
use crate::export_jobs;
use crate::structured_query::fixtures::{criteria, desc, field, has};

fn write(app: &Application, input: Value) -> Value {
    let existing = input["id"]
        .as_str()
        .and_then(|id| crate::store::snapshots::read_project_item(&app.root, id).unwrap());
    let result = crate::application::item_writes::write_item_input_typed(
        &app.database().unwrap(),
        input,
        existing.as_ref().and_then(|item| item["revision"].as_str()),
        &app.activity_date().unwrap(),
    );
    result.unwrap()["item"].clone()
}

fn id(item: &Value) -> &str {
    item["id"].as_str().unwrap()
}

fn page(app: &Application, title: &str, tags: &[&str]) -> Value {
    write(
        app,
        json!({"meta": {"type": "page", "title": title}, "tags": tags,
        "content": {"document": {"type": "doc", "content": [{"type": "paragraph",
            "content": [{"type": "text", "text": title}]}]}}}),
    )
}
#[expect(
    clippy::needless_pass_by_value,
    reason = "Test requests own fixture values just like decoded production requests."
)]
fn content(root: &Value, layout: &str, outline: Value) -> Value {
    json!({"kind": "publication", "selectedOutput": "site",
    "rootPageId": id(root), "outline": outline, "selectionRules": [],
    "themeId": "classic", "targets": {
        "pdf": {"profile": "default", "outsideLinks": "error"},
        "html": {"outsideLinks": "error"},"docx": {"outsideLinks": "error"},"epub":{"outsideLinks": "error"},
        "site": {"layout": layout,
            "outsideLinks": "error"}
    }})
}

#[test]
fn authored_order_theme_overrides_and_resolved_revisions_have_one_rust_owner() {
    let project = TestProject::new("publication-authored-contract");
    let app = project.application();
    app.initialize_sync().unwrap();
    let root = page(&app, "Overview", &[]);
    let first = page(&app, "Stars", &[]);
    let second = page(&app, "Planets", &[]);
    let settings = json!({"shared":{"root":{"fontSize":"13.2pt"}},"targets":{}});
    let mut source = content(
        &root,
        "documentation",
        json!([
            {"kind": "page", "pageId": id(&second), "children": [
                {"kind": "page", "pageId": id(&first), "children": []}
            ]}
        ]),
    );
    source["selectedOutput"] = json!("pdf");
    source["themeOverrides"] = serde_json::to_value(settings).unwrap();
    let publication = write(
        &app,
        json!({"meta": {"type": "publication", "title": "Book"},
        "content": source}),
    );
    let plan =
        export_jobs::plan_publication(&app.root, id(&publication), PublicationFormat::Pdf).unwrap();
    assert_eq!(plan.format, PublicationFormat::Pdf);
    assert_eq!(plan.theme.resolved.root_font_size_pt, 13.2);
    assert_eq!(plan.publication_revision, publication["revision"]);
    assert_eq!(
        plan.pages
            .iter()
            .map(|page| (page.id.as_str(), page.depth))
            .collect::<Vec<_>>(),
        vec![(id(&root), 0), (id(&second), 1), (id(&first), 2)]
    );
    assert!(app.database().is_ok());

    let resolved = export_jobs::resolve_inputs(&app.root, id(&publication)).unwrap();
    write(
        &app,
        json!({"id": id(&publication), "meta": {"title": "Revised book"}}),
    );
    let error = export_jobs::plan_resolved(
        &app.root,
        id(&publication),
        PublicationFormat::Pdf,
        &resolved,
    )
    .unwrap_err();
    assert!(
        error.to_string().contains("changed while resolving"),
        "{error}"
    );
    write(
        &app,
        json!({"id": id(&first), "meta": {"title": "Changed stars"}}),
    );
    let fresh =
        export_jobs::plan_publication(&app.root, id(&publication), PublicationFormat::Pdf).unwrap();
    assert_ne!(fresh.pages[2].revision, plan.pages[2].revision);
    app.stop_sync().unwrap();
    project.close().unwrap();
}

#[test]
fn query_collections_are_deterministic_and_stale_membership_cannot_render() {
    let project = TestProject::new("publication-collection-contract");
    let app = project.application();
    app.initialize_sync().unwrap();
    let root = page(&app, "Overview", &[]);
    let stars = page(&app, "Stars", &["astronomy"]);
    let planets = page(&app, "Planets", &["astronomy"]);
    let publication = write(
        &app,
        json!({"meta": {"type": "publication", "title": "Guide"},
        "content": content(&root, "documentation", json!([
            {"kind": "collection", "id": "00000000-0000-7000-8000-000000000711",
                "title": "Topics", "query": criteria([field("tag", "astronomy")])}
        ]))}),
    );
    let plan =
        export_jobs::plan_publication(&app.root, id(&publication), PublicationFormat::Pdf).unwrap();
    let again =
        export_jobs::plan_publication(&app.root, id(&publication), PublicationFormat::Pdf).unwrap();
    assert_eq!(
        serde_json::to_value(&plan).unwrap(),
        serde_json::to_value(again).unwrap()
    );
    assert_eq!(plan.collections[0].page_ids, vec![id(&planets), id(&stars)]);
    assert_eq!(
        plan.pages
            .iter()
            .map(|page| page.id.as_str())
            .collect::<Vec<_>>(),
        vec![id(&root), id(&planets), id(&stars)]
    );

    let moon = page(&app, "Moon", &["astronomy"]);
    let output = app.root.join("stale-collection.pdf");
    let error =
        export_jobs::render_publication(&app.root, &plan, &output, None, &app.export_resources())
            .unwrap_err();
    assert!(
        error.to_string().contains("Collection Topics changed"),
        "{error}"
    );
    assert!(!output.exists());
    let fresh =
        export_jobs::plan_publication(&app.root, id(&publication), PublicationFormat::Pdf).unwrap();
    assert_eq!(
        fresh.collections[0].page_ids,
        vec![id(&moon), id(&planets), id(&stars)]
    );

    let post = write(
        &app,
        json!({"meta": {"type": "page", "title": "Observing notes",
        "published_on": "2026-09-29"}, "content": {"document": {"type": "doc", "content": [{"type": "paragraph"}]}}}),
    );
    let blog = write(
        &app,
        json!({"meta": {"type": "publication", "title": "Blog"},
        "content": content(&root, "website", json!([
            {"kind": "collection", "id": "00000000-0000-7000-8000-000000000712", "title": "Posts",
                "query": criteria([has("published_on")]).sorted([desc("published_on")]), "listingSlug": "posts", "pageSize": 20}
        ]))}),
    );
    let empty = write(
        &app,
        json!({"meta": {"type": "publication", "title": "Empty guide"},
        "content": content(&root, "documentation", json!([
            {"kind": "collection", "id": "00000000-0000-7000-8000-000000000713", "title": "Missing topics",
                "query": criteria([field("tag", "no_results_expected")]), "listingSlug": "topics"}
        ]))}),
    );
    let blog_plan =
        export_jobs::plan_publication(&app.root, id(&blog), PublicationFormat::Site).unwrap();
    assert_eq!(blog_plan.collections[0].page_ids, vec![id(&post)]);
    let empty_plan =
        export_jobs::plan_publication(&app.root, id(&empty), PublicationFormat::Site).unwrap();
    assert_eq!(
        empty_plan.collections[0].page_ids,
        [] as [std::string::String; 0]
    );
    assert_eq!(
        empty_plan
            .pages
            .iter()
            .map(|page| page.id.as_str())
            .collect::<Vec<_>>(),
        vec![id(&root)]
    );
    app.stop_sync().unwrap();
    project.close().unwrap();
}

#[test]
fn linked_plans_overlay_unreported_canonical_edges_and_terminate_cycles() {
    let project = TestProject::new("publication-graph-contract");
    let app = project.application();
    app.initialize_sync().unwrap();
    let pages = ["Root", "Second", "Third"].map(|title| page(&app, title, &[]));
    page(&app, "Outside the export", &[]);
    for (index, source) in pages.iter().enumerate() {
        let target = &pages[(index + 1) % pages.len()];
        crate::application::item_writes::write_item_input_typed(&app.database().unwrap(),
            json!({"id": id(source), "meta": {}, "content": {"document": {"type": "doc", "content": [
                {"type": "paragraph", "content": [{"type": "text", "text": target["meta"]["title"],
                    "marks": [{"type": "link", "attrs": {"href": format!("/?note={}", id(target))}}]}]}
            ]}}}), source["revision"].as_str(), &app.activity_date().unwrap()).unwrap();
    }
    let plan = export_jobs::plan_linked_pages(
        &app.root,
        id(&pages[0]),
        PublicationFormat::Site,
        "classic",
        OutsideLinks::Error,
        PdfProfile::Default,
        crate::domain::publication::LinkDepth::Reachable,
        &mut |_, _| true,
    )
    .unwrap();
    assert_eq!(
        plan.pages
            .iter()
            .map(|page| page.id.as_str())
            .collect::<Vec<_>>(),
        pages.iter().map(id).collect::<Vec<_>>()
    );
    execute(&app, "delete_item", json!({"id":id(&pages[2])})).unwrap();
    let issues = export_jobs::plan_linked_pages(
        &app.root,
        id(&pages[0]),
        PublicationFormat::Site,
        "classic",
        OutsideLinks::Error,
        PdfProfile::Default,
        crate::domain::publication::LinkDepth::Reachable,
        &mut |_, _| true,
    )
    .unwrap()
    .issues;
    assert!(
        issues.iter().any(|issue| issue.message.contains("missing")),
        "{issues:?}"
    );
    let readable = export_jobs::plan_linked_pages(
        &app.root,
        id(&pages[0]),
        PublicationFormat::Site,
        "classic",
        OutsideLinks::TextOnly,
        PdfProfile::Default,
        crate::domain::publication::LinkDepth::Reachable,
        &mut |_, _| true,
    )
    .unwrap();
    assert_eq!(
        readable
            .pages
            .iter()
            .map(|page| page.id.as_str())
            .collect::<Vec<_>>(),
        pages[..2].iter().map(id).collect::<Vec<_>>()
    );
    app.stop_sync().unwrap();
    project.close().unwrap();
}
