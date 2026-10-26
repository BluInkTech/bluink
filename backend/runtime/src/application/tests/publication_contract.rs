// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! Regression coverage migrated from the obsolete Node Publication facade.
use super::*;
use crate::engine::publication::{OutsideLinks, PdfProfile, plan::PublicationFormat};
use crate::export;

fn write(app: &Application, input: Value) -> Value {
    let existing = input["id"]
        .as_str()
        .and_then(|id| crate::engine::project::item::read_project_item(&app.root, id).unwrap());
    let result = crate::engine::project::write::write_item_input_typed(
        &app.root,
        input,
        existing.as_ref().and_then(|item| item["revision"].as_str()),
        &app.activity_date().unwrap(),
    );
    mutations::committed(&app.root, result).unwrap()["item"].clone()
}

fn id(item: &Value) -> &str {
    item["id"].as_str().unwrap()
}

fn page(app: &Application, title: &str, tags: &[&str]) -> Value {
    write(
        app,
        json!({"meta": {"type": "page", "title": title}, "tags": tags,
        "content": {"type": "doc", "content": [{"type": "paragraph",
            "content": [{"type": "text", "text": title}]}]}}),
    )
}
#[expect(
    clippy::needless_pass_by_value,
    reason = "Test requests own fixture values just like decoded production requests."
)]
fn content(root: &Value, preset: &str, outline: Value) -> Value {
    json!({"kind": "publication", "preset": preset, "selectedOutput": "site",
    "rootPageId": id(root), "outline": outline, "selectionRules": [],
    "themeId": "classic", "targets": {
        "pdf": {"profile": "default", "outsideLinks": "error"},
        "html": {"outsideLinks": "error"},"docx": {"outsideLinks": "error"},"epub":{"outsideLinks": "error"},
        "site": {"layout": if preset == "blog" { "website" } else { "documentation" },
            "outsideLinks": "error"}
    }})
}

fn ready_index(app: &Application) -> String {
    app.initialize_sync(true).unwrap();
    let index = app.index_path().unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let status = service::index_service_status_typed(&index).unwrap();
        if status.state == "ready" && status.pending == 0 {
            return index;
        }
        assert_ne!(status.state, "error");
        assert!(
            Instant::now() < deadline,
            "Publication index did not settle"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn collection_searches_retain_their_captured_generation() {
    let project = TestProject::new("publication-query-generation");
    let app = project.application();
    app.initialize_sync(false).unwrap();
    let first = page(&app, "First collection page", &[]);
    let index = ready_index(&app);
    let (snapshot, cache) = service::cached_index(&index).unwrap();
    let query = crate::query::document::QueryDocument::active_pages();
    let before =
        crate::search::search_snapshot(&snapshot, &cache, &query, 100, 0, None, true).unwrap();
    assert_eq!(before.count, 1);

    page(&app, "Second collection page", &[]);
    ready_index(&app);
    let retained =
        crate::search::search_snapshot(&snapshot, &cache, &query, 100, 0, None, true).unwrap();
    assert_eq!(retained.count, 1);
    assert_eq!(retained.hits[0].id, id(&first));
    let current = crate::search::search_service_sync(index, query, 100, 0, None, true).unwrap();
    assert_eq!(current.count, 2);
    drop(snapshot);
    drop(cache);
    app.stop_sync().unwrap();
    project.close().unwrap();
}

#[test]
fn authored_order_theme_overrides_and_resolved_revisions_have_one_rust_owner() {
    let project = TestProject::new("publication-authored-contract");
    let app = project.application();
    app.initialize_sync(false).unwrap();
    let root = page(&app, "Overview", &[]);
    let first = page(&app, "Stars", &[]);
    let second = page(&app, "Planets", &[]);
    let settings = json!({"shared":{"root":{"fontSize":"13.2pt"}},"targets":{}});
    let mut source = content(
        &root,
        "book",
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
    let plan = export::plan_publication(&app.root, id(&publication), PublicationFormat::Pdf, None)
        .unwrap();
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
    assert!(app.state().unwrap().index.is_none());

    let resolved = export::resolve_inputs(&app.root, id(&publication), None).unwrap();
    write(
        &app,
        json!({"id": id(&publication), "meta": {"title": "Revised book"}}),
    );
    let error = export::plan_resolved(
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
    let fresh = export::plan_publication(&app.root, id(&publication), PublicationFormat::Pdf, None)
        .unwrap();
    assert_ne!(fresh.pages[2].revision, plan.pages[2].revision);
    app.stop_sync().unwrap();
    project.close().unwrap();
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "Keep each end-to-end scenario together so setup, failure checks and cleanup remain visible."
)]
fn query_collections_are_deterministic_and_stale_membership_cannot_render() {
    let project = TestProject::new("publication-collection-contract");
    let app = project.application();
    app.initialize_sync(false).unwrap();
    let root = page(&app, "Overview", &[]);
    let stars = page(&app, "Stars", &["astronomy"]);
    let planets = page(&app, "Planets", &["astronomy"]);
    let publication = write(
        &app,
        json!({"meta": {"type": "publication", "title": "Guide"},
        "content": content(&root, "documentation", json!([
            {"kind": "collection", "id": "00000000-0000-7000-8000-000000000711",
                "title": "Topics", "query": crate::query::document::test_query("+tag:astronomy")}
        ]))}),
    );
    let index = ready_index(&app);
    let plan = export::plan_publication(
        &app.root,
        id(&publication),
        PublicationFormat::Pdf,
        Some(&index),
    )
    .unwrap();
    let again = export::plan_publication(
        &app.root,
        id(&publication),
        PublicationFormat::Pdf,
        Some(&index),
    )
    .unwrap();
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
    ready_index(&app);
    let output = app.root.join("stale-collection.pdf");
    let error = export::render_publication(
        &app.root,
        &plan,
        &output,
        None,
        Some(&index),
        &app.export_resources(),
    )
    .unwrap_err();
    assert!(
        error.to_string().contains("Collection Topics changed"),
        "{error}"
    );
    assert!(!output.exists());
    let fresh = export::plan_publication(
        &app.root,
        id(&publication),
        PublicationFormat::Pdf,
        Some(&index),
    )
    .unwrap();
    assert_eq!(
        fresh.collections[0].page_ids,
        vec![id(&moon), id(&planets), id(&stars)]
    );

    let post = write(
        &app,
        json!({"meta": {"type": "page", "title": "Observing notes",
        "published_on": "2026-09-29"}, "content": {"type": "doc", "content": [{"type": "paragraph"}]}}),
    );
    let blog = write(
        &app,
        json!({"meta": {"type": "publication", "title": "Blog"},
        "content": content(&root, "blog", json!([
            {"kind": "collection", "id": "00000000-0000-7000-8000-000000000712", "title": "Posts",
                "query": crate::query::document::test_query("+has:published_on sort:published_on-desc"), "listingSlug": "posts", "pageSize": 20}
        ]))}),
    );
    let empty = write(
        &app,
        json!({"meta": {"type": "publication", "title": "Empty guide"},
        "content": content(&root, "documentation", json!([
            {"kind": "collection", "id": "00000000-0000-7000-8000-000000000713", "title": "Missing topics",
                "query": crate::query::document::test_query("+tag:no_results_expected"), "listingSlug": "topics"}
        ]))}),
    );
    ready_index(&app);
    let blog_plan =
        export::plan_publication(&app.root, id(&blog), PublicationFormat::Site, Some(&index))
            .unwrap();
    assert_eq!(blog_plan.collections[0].page_ids, vec![id(&post)]);
    let empty_plan =
        export::plan_publication(&app.root, id(&empty), PublicationFormat::Site, Some(&index))
            .unwrap();
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
    app.initialize_sync(false).unwrap();
    let pages = ["Root", "Second", "Third"].map(|title| page(&app, title, &[]));
    let unrelated = page(&app, "Outside the export", &[]);
    ready_index(&app);
    // A linked export must visit only reachable notes. A project-wide graph
    // capture would read this changed, unrelated file and fail the export.
    let unrelated_path = crate::engine::project::identity::ItemId::try_from(id(&unrelated))
        .unwrap()
        .path(&app.root);
    std::fs::write(unrelated_path, b"Unrelated malformed note").unwrap();
    for (index, source) in pages.iter().enumerate() {
        let target = &pages[(index + 1) % pages.len()];
        // Deliberately bypass notification: external edits must overlay the index.
        crate::engine::project::write::write_item_input_typed(&app.root,
            json!({"id": id(source), "meta": {}, "content": {"type": "doc", "content": [
                {"type": "paragraph", "content": [{"type": "text", "text": target["meta"]["title"],
                    "marks": [{"type": "link", "attrs": {"href": format!("/?note={}", id(target))}}]}]}
            ]}}), source["revision"].as_str(), &app.activity_date().unwrap()).unwrap();
    }
    let plan = export::plan_linked_pages(
        &app.root,
        id(&pages[0]),
        PublicationFormat::Site,
        "classic",
        OutsideLinks::Error,
        PdfProfile::Default,
    )
    .unwrap();
    assert_eq!(
        plan.pages
            .iter()
            .map(|page| page.id.as_str())
            .collect::<Vec<_>>(),
        pages.iter().map(id).collect::<Vec<_>>()
    );
    std::fs::remove_dir_all(
        app.root
            .join("items")
            .join(&id(&pages[2])[..2])
            .join(id(&pages[2])),
    )
    .unwrap();
    let error = export::plan_linked_pages(
        &app.root,
        id(&pages[0]),
        PublicationFormat::Site,
        "classic",
        OutsideLinks::Error,
        PdfProfile::Default,
    )
    .unwrap_err();
    assert!(
        error.to_string().contains("missing") || error.to_string().contains("does not exist"),
        "{error}"
    );
    let readable = export::plan_linked_pages(
        &app.root,
        id(&pages[0]),
        PublicationFormat::Site,
        "classic",
        OutsideLinks::TextOnly,
        PdfProfile::Default,
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
