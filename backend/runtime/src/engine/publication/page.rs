// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! One standalone source contract, independent of its output serializer.
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::{
    OutsideLinks, PdfProfile,
    plan::{PlannedPage, PublicationFormat, require_accessible_source},
    preflight::{
        PlannedAsset, PlannedLink, PlannedSnippet, assert_preflight_fresh, preflight_pages,
    },
    source::document,
    theme::{ThemeOrigin, ThemeSnapshot, assert_theme_fresh, capture_theme},
};
use crate::engine::project::item::read_project_item;

#[derive(Debug, Deserialize, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub(crate) struct PagePlan {
    pub(crate) page_id: String,
    pub(crate) page_revision: String,
    pub(crate) title: String,
    pub(crate) format: PublicationFormat,
    pub(crate) profile: PdfProfile,
    pub(crate) outside_links: OutsideLinks,
    pub(crate) theme: ThemeSnapshot,
    pub(crate) assets: Vec<PlannedAsset>,
    pub(crate) links: Vec<PlannedLink>,
    pub(crate) snippets: Vec<PlannedSnippet>,
}

pub(crate) fn plan_page(
    root: &Path,
    page_id: &str,
    theme_id: &str,
    outside_links: OutsideLinks,
    format: PublicationFormat,
    profile: PdfProfile,
) -> Result<PagePlan, String> {
    if format == PublicationFormat::Site {
        return Err("A standalone page cannot generate a website.".into());
    }
    if format != PublicationFormat::Pdf && matches!(profile, PdfProfile::Accessible) {
        return Err("The accessible profile applies only to PDF.".into());
    }
    let page = read_project_item(root, page_id)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("Export source page {page_id} does not exist."))?;
    if page["meta"]["type"] != "page" || page["meta"]["state"] != "active" {
        return Err(format!("Export source {page_id} must be an active page."));
    }
    let title = page["meta"]["title"]
        .as_str()
        .filter(|title| !title.trim().is_empty())
        .ok_or("Export source page needs a title.")?
        .to_owned();
    let revision = page["revision"]
        .as_str()
        .ok_or("Export source page has no revision.")?
        .to_owned();
    let theme = capture_theme(root, theme_id, format)?;
    if matches!(profile, PdfProfile::Accessible) {
        if theme.origin != ThemeOrigin::Builtin {
            return Err("Accessible PDF currently supports reviewed built-in themes only.".into());
        }
        require_accessible_source(&page, document(&page), &[])?;
    }
    let source = PlannedPage {
        id: page_id.to_owned(),
        revision: revision.clone(),
        title: title.clone(),
        route: "index.html".into(),
        published_on: None,
        tags: Vec::new(),
        depth: 0,
        excluded_block_ids: Vec::new(),
        parent_id: None,
    };
    let preflight = preflight_pages(root, &[source], &outside_links)?;
    let plan = PagePlan {
        page_id: page_id.to_owned(),
        page_revision: revision,
        title,
        format,
        profile,
        outside_links,
        theme,
        assets: preflight.assets,
        links: preflight.links,
        snippets: preflight.snippets,
    };
    assert_page_fresh(root, &plan)?;
    Ok(plan)
}

pub(crate) fn assert_page_fresh(root: &Path, plan: &PagePlan) -> Result<(), String> {
    let current = read_project_item(root, &plan.page_id).map_err(|error| error.to_string())?;
    if current.as_ref().and_then(|page| page["revision"].as_str()) != Some(&plan.page_revision) {
        return Err(format!(
            "Export source page {} changed after planning.",
            plan.page_id
        ));
    }
    assert_theme_fresh(root, &plan.theme, plan.format)?;
    assert_preflight_fresh(root, &plan.assets, &plan.snippets)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::project::{
        asset::write_asset, config::create_project_config, snippets::write_snippet,
        write::write_item_input,
    };
    use serde_json::{Value, json};

    #[test]
    #[expect(
        clippy::too_many_lines,
        reason = "Keep cross-target planning, dependency changes and serializer rejection in one end-to-end scenario."
    )]
    fn targets_share_source_dependencies_and_reject_stale_assets_snippets_and_pages() {
        let folder = tempfile::tempdir().unwrap();
        let root = folder.path();
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("test-fixtures");
        let config = serde_json::from_slice(
            &std::fs::read(fixture.join("pdf-project/bluink.json")).unwrap(),
        )
        .unwrap();
        create_project_config(root, config).unwrap();
        crate::engine::project::properties::initialize(root).unwrap();
        let id = uuid::Uuid::now_v7().to_string();
        let paragraph =
            json!({"type":"paragraph","content":[{"type":"text","text":"Shared words"}]});
        write_snippet(root, "shared", paragraph.clone(), None).unwrap();
        write_item_input(root, json!({"id":id,"meta":{"type":"page","title":"Shared source","lang":"en"},"content":{"type":"doc","content":[
            {"type":"paragraph","content":[{"type":"text","text":"Self","marks":[{"type":"link","attrs":{"href":format!("/?note={id}")}}]}]},
            {"type":"embed","attrs":{"snippetId":"shared"}},
            {"type":"image","attrs":{"src":format!("/assets/{id}/image.png"),"alt":"Small image"}}
        ]}}), None, "2026-10-14").unwrap();
        let image = std::fs::read(fixture.join("one-pixel.png")).unwrap();
        write_asset(root, &id, "image.png", &image).unwrap();
        let plans: Vec<_> = [
            PublicationFormat::Html,
            PublicationFormat::Pdf,
            PublicationFormat::Docx,
            PublicationFormat::Epub,
        ]
        .into_iter()
        .map(|format| {
            plan_page(
                root,
                &id,
                "classic",
                OutsideLinks::Error,
                format,
                PdfProfile::Default,
            )
            .unwrap()
        })
        .collect();
        for plan in &plans {
            assert_eq!(plan.format, plan.theme.format);
            assert_page_fresh(root, plan).unwrap();
        }
        for field in [
            "pageId",
            "pageRevision",
            "title",
            "assets",
            "links",
            "snippets",
        ] {
            let html = serde_json::to_value(&plans[0]).unwrap();
            for target in plans.iter().skip(1) {
                let other = serde_json::to_value(target).unwrap();
                assert_eq!(html[field], other[field], "{field} {:?}", target.format);
            }
        }
        assert!(
            super::super::article::render_article(root, &plans[1], false)
                .unwrap_err()
                .contains("HTML page plan")
        );
        assert!(
            super::super::note_pdf::render_note_pdf(root, &plans[0])
                .err()
                .unwrap()
                .contains("PDF page plan")
        );
        write_asset(root, &id, "image.png", b"changed").unwrap();
        for plan in &plans {
            assert!(
                assert_page_fresh(root, plan)
                    .unwrap_err()
                    .contains("changed")
            );
        }
        write_asset(root, &id, "image.png", &image).unwrap();
        let snippet = crate::engine::project::snippets::read_snippet(root, "shared")
            .unwrap()
            .unwrap();
        write_snippet(
            root,
            "shared",
            json!({"type":"paragraph","content":[{"type":"text","text":"Changed snippet"}]}),
            snippet["revision"].as_str(),
        )
        .unwrap();
        for plan in &plans {
            assert!(
                assert_page_fresh(root, plan)
                    .unwrap_err()
                    .contains("changed")
            );
        }
        let snippet = crate::engine::project::snippets::read_snippet(root, "shared")
            .unwrap()
            .unwrap();
        write_snippet(root, "shared", paragraph, snippet["revision"].as_str()).unwrap();
        let page = read_project_item(root, &id).unwrap().unwrap();
        write_item_input(
            root,
            json!({"id":id,"meta":{"title":"Changed source"}}),
            page["revision"].as_str(),
            "2026-10-14",
        )
        .unwrap();
        for plan in &plans {
            assert!(
                assert_page_fresh(root, plan)
                    .unwrap_err()
                    .contains("source page")
            );
        }
        for (format, profile) in [
            (PublicationFormat::Site, PdfProfile::Default),
            (PublicationFormat::Html, PdfProfile::Accessible),
        ] {
            assert!(plan_page(root, &id, "classic", OutsideLinks::Error, format, profile).is_err());
        }
        let mut damaged: Value = serde_json::to_value(&plans[0]).unwrap();
        damaged["format"] = json!("pdf");
        let damaged: PagePlan = serde_json::from_value(damaged).unwrap();
        assert!(super::super::article::render_article(root, &damaged, false).is_err());
    }
}
