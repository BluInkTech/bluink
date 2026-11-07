// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use crate::domain::publication::{
    page::PagePlan,
    preflight::PlannedAsset,
    source::{document, min_heading_level, parse_language, resolved_fields},
    typst::{HeadingOptions, TypstResources, heading, render_typst_document},
};
use crate::export_jobs::{
    page::assert_page_fresh,
    pdf::{PdfBundle, quote},
    theme::load_pdf_theme,
};
use crate::store::snapshots::read_project_item;
use std::{
    collections::{HashMap, HashSet},
    fmt::Write as _,
    path::Path,
};

#[expect(
    clippy::expect_used,
    reason = "Formatting only strings and primitive values into String is infallible."
)]
pub(crate) fn render_note_pdf(root: &Path, plan: &PagePlan) -> Result<PdfBundle, String> {
    if plan.format != crate::domain::publication::plan::PublicationFormat::Pdf {
        return Err("PDF rendering requires a PDF page plan.".into());
    }
    assert_page_fresh(root, plan)?;
    let mut files = load_pdf_theme(root, &plan.theme, &plan.title, "en")?;
    let item = read_project_item(root, &plan.page_id)
        .map_err(|error| error.to_string())?
        .ok_or("PDF source page disappeared.")?;
    let assets = resolved_assets(&plan.assets);
    let snippets: HashMap<String, serde_json::Value> = plan
        .snippets
        .iter()
        .map(|snippet| (snippet.id.clone(), snippet.paragraph.clone()))
        .collect();
    let language = item["meta"]["lang"].as_str().unwrap_or("en");
    let (lang, region) = parse_language(language)
        .ok_or_else(|| format!("Page {} has invalid language tag {language}.", plan.page_id))?;
    let mut content = String::from("#import \"theme-values.typ\": *\n");
    write!(
        content,
        "#set document(title: {})\n#set text(lang: {}, region: {})\n",
        quote(&plan.title),
        quote(&lang),
        region.as_deref().map_or_else(|| "none".to_owned(), quote),
    )
    .expect("writing to a String cannot fail");
    content.push_str(&heading(
        1,
        &format!("#text({})", quote(&plan.title)),
        HeadingOptions {
            anchor: Some(&format!("note-{}", plan.page_id)),
            ..HeadingOptions::default()
        },
    ));
    let page_line = content.bytes().filter(|byte| *byte == b'\n').count() + 1;
    let links = crate::domain::publication::render::native_links(&plan.links, &plan.page_id)?;
    let anchors = crate::domain::publication::render::linked_anchors(&plan.links);
    let fields = resolved_fields(&item, document(&item), &HashSet::new())?;
    let body = render_typst_document(
        document(&item),
        &TypstResources {
            page_id: &plan.page_id,
            links: &links,
            assets: &assets,
            fields: &fields,
            snippets: &snippets,
            excluded_block_ids: &HashSet::new(),
            anchors: &anchors,
            references_heading: &plan.theme.resolved.page.references_heading,
        },
        2 - i16::try_from(min_heading_level(document(&item), &HashSet::new()).unwrap_or(1))
            .map_err(|_| "Page heading level is too large.")?,
    )
    .map_err(|error| crate::domain::publication::render::on_page(&plan.page_id, error))?;
    content.push_str(&body);
    let content_path = "content.typ".to_owned();
    files.insert(content_path, content.into_bytes());
    for asset in &plan.assets {
        let bytes = crate::export_jobs::resources::read_asset(root, asset)?;
        files.insert(
            format!("assets/{}/{}", asset.owner_id, asset.filename),
            bytes,
        );
    }
    assert_page_fresh(root, plan)?;
    Ok(PdfBundle {
        shell_path: "document.typ".to_owned(),
        files,
        page_lines: vec![(page_line, plan.page_id.clone())],
    })
}

fn resolved_assets(assets: &[PlannedAsset]) -> HashMap<String, String> {
    assets
        .iter()
        .map(|asset| {
            (
                asset.src.clone(),
                format!("assets/{}/{}", asset.owner_id, asset.filename),
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use serde_json::{Value, json};

    use super::render_note_pdf;
    use crate::application::catalogue_writes::create_property;
    use crate::application::item_writes::write_asset;
    use crate::application::item_writes::write_item_input;
    use crate::domain::project::config::create_project_config;
    use crate::domain::publication::plan::PublicationFormat;
    use crate::domain::publication::{OutsideLinks, PdfProfile};
    use crate::export_jobs::page::assert_page_fresh;
    use crate::export_jobs::page::plan_page;
    use crate::export_jobs::pdf::publish_pdf;

    #[test]
    #[expect(
        clippy::too_many_lines,
        reason = "Keep each end-to-end scenario together so setup, failure checks and cleanup remain visible."
    )]
    fn renders_standalone_page_with_links_passages_assets_and_accessible_profile() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("project");
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("test-fixtures/pdf-project");
        let config: Value =
            serde_json::from_slice(&std::fs::read(fixture.join("bluink.json")).unwrap()).unwrap();
        create_project_config(&root, config).unwrap();
        crate::domain::project::properties::initialize(&root).unwrap();
        crate::application::snippets::write_snippet(
            &root,
            "excerpt",
            json!({"type":"paragraph","content":[{"type":"text","text":"Quoted source"}]}),
            None,
        )
        .unwrap();
        create_property(
            &crate::store::Store::shared(&root).unwrap(),
            &json!({"key":"source_url","valueType":"text"}),
        )
        .unwrap();
        let ids: Vec<_> = (0..3).map(|_| uuid::Uuid::now_v7().to_string()).collect();
        let src = format!("/assets/{}/image.png", ids[0]);
        write_item_input(&root, json!({"id":ids[0],"meta":{"type":"page","title":"Article <one>","lang":"en-GB"},
            "content":{"document": {"type":"doc","content":[
                {"type":"paragraph","content":[{"type":"text","text":"Outside note","marks":[{"type":"link","attrs":{"href":format!("/?note={}",ids[1])}}]}]},
                {"type":"heading","attrs":{"level":2},"content":[{"type":"text","text":"First section"}]},
                {"type":"image","attrs":{"src":src,"alt":"A small cover"}},
                {"type":"embed","attrs":{"snippetId":"excerpt"}}
            ]}}}), None, "2026-09-28").unwrap();
        write_item_input(&root, json!({"id":ids[1],"meta":{"type":"page","title":"Outside","source_url":"https://example.com/outside"},
            "content":{"document": {"type":"doc","content":[{"type":"paragraph","content":[{"type":"text","text":"Outside"}]}]}}}), None, "2026-09-28").unwrap();
        write_item_input(&root, json!({"id":ids[2],"meta":{"type":"page","title":"Passage","lang":"en"},
            "content":{"document": {"type":"doc","content":[{"type":"paragraph","attrs":{"labels":["excerpt"]},"content":[{"type":"text","text":"Quoted source"}]}]}}}),
            None, "2026-09-28").unwrap();
        write_asset(
            &root,
            &ids[0],
            "image.png",
            &std::fs::read(fixture.join("../one-pixel.png")).unwrap(),
        )
        .unwrap();
        assert!(
            plan_page(
                &root,
                &ids[0],
                "classic",
                OutsideLinks::Error,
                PublicationFormat::Pdf,
                PdfProfile::Default
            )
            .unwrap_err()
            .contains("outside this Publication")
        );
        let plan = plan_page(
            &root,
            &ids[0],
            "classic",
            OutsideLinks::TextOnly,
            PublicationFormat::Pdf,
            PdfProfile::Default,
        )
        .unwrap();
        let bundle = render_note_pdf(&root, &plan).unwrap();
        let source = String::from_utf8(bundle.files["content.typ"].clone()).unwrap();
        assert!(source.contains("#heading(level: 1)[#text(\"Article <one>\")]"));
        assert!(source.contains("#heading(level: 2)[#text(\"First section\")]"));
        assert!(!source.contains("https://example.com/outside"));
        assert!(source.contains("Quoted source"));
        let plain_plan = plan_page(
            &root,
            &ids[0],
            "classic",
            OutsideLinks::TextOnly,
            PublicationFormat::Pdf,
            PdfProfile::Default,
        )
        .unwrap();
        let plain_bundle = render_note_pdf(&root, &plain_plan).unwrap();
        let plain_source = String::from_utf8(plain_bundle.files["content.typ"].clone()).unwrap();
        assert!(plain_source.contains("Outside note"));
        assert!(!plain_source.contains("https://example.com/outside"));
        assert!(
            bundle
                .files
                .contains_key(&format!("assets/{}/image.png", ids[0]))
        );
        publish_pdf(
            bundle,
            plan.profile,
            &root.join("article.pdf"),
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("../fonts/assets/pdf"),
        )
        .unwrap();
        assert!(
            std::fs::read(root.join("article.pdf"))
                .unwrap()
                .starts_with(b"%PDF-")
        );
        let accessible = plan_page(
            &root,
            &ids[0],
            "classic",
            OutsideLinks::TextOnly,
            PublicationFormat::Pdf,
            PdfProfile::Accessible,
        )
        .unwrap();
        let accessible_bundle = render_note_pdf(&root, &accessible).unwrap();
        publish_pdf(
            accessible_bundle,
            accessible.profile,
            &root.join("accessible.pdf"),
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("../fonts/assets/pdf"),
        )
        .unwrap();
        assert!(
            std::fs::read(root.join("accessible.pdf"))
                .unwrap()
                .starts_with(b"%PDF-")
        );
        write_asset(&root, &ids[0], "image.png", b"changed").unwrap();
        assert!(
            assert_page_fresh(&root, &plan)
                .unwrap_err()
                .contains("changed")
        );
        temporary.close().unwrap();
    }

    #[test]
    fn project_pdf_theme_is_data_only_and_accessible_profile_remains_restricted() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("project");
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("test-fixtures/pdf-project");
        let config: Value =
            serde_json::from_slice(&std::fs::read(fixture.join("bluink.json")).unwrap()).unwrap();
        create_project_config(&root, config).unwrap();
        crate::domain::project::properties::initialize(&root).unwrap();
        let id = uuid::Uuid::now_v7().to_string();
        write_item_input(&root, json!({"id":id,"meta":{"type":"page","title":"Themed note","lang":"en"},
            "content":{"document": {"type":"doc","content":[{"type":"paragraph","content":[{"type":"text","text":"A paragraph"}]}]}}}),
            None, "2026-09-28").unwrap();
        let example = Path::new(env!("CARGO_MANIFEST_DIR")).join("../examples/themes/field-notes");
        crate::store::theme_packages::install_fixture(&root, "field-notes", &example);

        let plan = plan_page(
            &root,
            &id,
            "field-notes",
            OutsideLinks::Error,
            PublicationFormat::Pdf,
            PdfProfile::Default,
        )
        .unwrap();
        assert!(!plan.theme.requires_trust);
        let bundle = render_note_pdf(&root, &plan).unwrap();
        assert!(
            String::from_utf8(bundle.files["theme-values.typ"].clone())
                .unwrap()
                .contains("#f2f4f8")
        );
        assert!(
            plan_page(
                &root,
                &id,
                "field-notes",
                OutsideLinks::Error,
                PublicationFormat::Pdf,
                PdfProfile::Accessible
            )
            .unwrap_err()
            .contains("built-in themes only")
        );
        temporary.close().unwrap();
    }
}
