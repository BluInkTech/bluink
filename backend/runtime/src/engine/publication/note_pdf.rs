// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::engine::project::item::read_project_item;
use crate::engine::publication::pdf::{PdfBundle, bundle_sibling, quote};
use crate::engine::publication::plan::{PlannedPage, PublicationFormat, require_accessible_source};
use crate::engine::publication::preflight::{
    LinkResolution, PlannedAsset, PlannedLink, PlannedSnippet, assert_preflight_fresh,
    preflight_pages,
};
use crate::engine::publication::source::{
    document, min_heading_level, parse_language, resolved_fields,
};
use crate::engine::publication::style::typst_values;
use crate::engine::publication::theme::{
    ThemeOrigin, ThemeSnapshot, assert_theme_fresh, capture_theme, load_pdf_theme,
};
use crate::engine::publication::typst::{TypstLink, TypstResources, render_typst_document};
use crate::engine::publication::{OutsideLinks, PdfProfile};

#[derive(Debug, Deserialize, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub(crate) struct NotePdfPlan {
    pub(crate) page_id: String,
    pub(crate) page_revision: String,
    pub(crate) title: String,
    pub(crate) profile: PdfProfile,
    pub(crate) outside_links: OutsideLinks,
    pub(crate) theme: ThemeSnapshot,
    pub(crate) assets: Vec<PlannedAsset>,
    pub(crate) links: Vec<PlannedLink>,
    pub(crate) snippets: Vec<PlannedSnippet>,
}

pub(crate) fn plan_note_pdf(
    root: &Path,
    page_id: &str,
    theme_id: &str,
    profile: PdfProfile,
    outside_links: OutsideLinks,
) -> Result<NotePdfPlan, String> {
    let page = read_project_item(root, page_id)?
        .ok_or_else(|| format!("PDF source page {page_id} does not exist."))?;
    if page["meta"]["type"] != "page" || page["meta"]["state"] != "active" {
        return Err(format!("PDF source {page_id} must be an active page."));
    }
    let title = page["meta"]["title"]
        .as_str()
        .filter(|title| !title.trim().is_empty())
        .ok_or("PDF source page needs a title.")?
        .to_owned();
    let revision = page["revision"]
        .as_str()
        .ok_or("PDF source page has no revision.")?
        .to_owned();
    let theme = capture_theme(root, theme_id, PublicationFormat::Pdf)?;
    if matches!(profile, PdfProfile::Accessible) && !matches!(theme.origin, ThemeOrigin::Builtin) {
        return Err("Accessible PDF currently supports reviewed built-in themes only.".to_owned());
    }
    let source = PlannedPage {
        id: page_id.to_owned(),
        revision: revision.clone(),
        title: title.clone(),
        route: "index.html".to_owned(),
        published_on: None,
        tags: Vec::new(),
        depth: 0,
        excluded_block_ids: Vec::new(),
        parent_id: None,
    };
    let preflight = preflight_pages(root, &[source], &outside_links)?;
    let plan = NotePdfPlan {
        page_id: page_id.to_owned(),
        page_revision: revision,
        title,
        profile,
        outside_links,
        theme,
        assets: preflight.assets,
        links: preflight.links,
        snippets: preflight.snippets,
    };
    if matches!(profile, PdfProfile::Accessible) {
        require_accessible_source(&page, document(&page), &[])?;
    }
    assert_note_pdf_fresh(root, &plan)?;
    Ok(plan)
}

pub(crate) fn assert_note_pdf_fresh(root: &Path, plan: &NotePdfPlan) -> Result<(), String> {
    let current = read_project_item(root, &plan.page_id)?;
    if current.as_ref().and_then(|page| page["revision"].as_str()) != Some(&plan.page_revision) {
        return Err(format!(
            "PDF source page {} changed after planning.",
            plan.page_id
        ));
    }
    assert_theme_fresh(root, &plan.theme, PublicationFormat::Pdf)?;
    assert_preflight_fresh(root, &plan.assets, &plan.links, &plan.snippets)
}

pub(crate) fn render_note_pdf(
    root: &Path,
    plan: &NotePdfPlan,
    trust_project_theme: bool,
) -> Result<PdfBundle, String> {
    assert_note_pdf_fresh(root, plan)?;
    let theme = load_pdf_theme(root, &plan.theme, trust_project_theme)?;
    let mut files = theme.files;
    let item = read_project_item(root, &plan.page_id)?.ok_or("PDF source page disappeared.")?;
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
        quote(&plan.title)?,
        quote(&lang)?,
        region
            .as_deref()
            .map(quote)
            .transpose()?
            .unwrap_or_else(|| "none".to_owned()),
    )
    .expect("writing to a String cannot fail");
    write!(
        content,
        "#heading(level: 1)[#text({})] <note-{}>\n\n",
        quote(&plan.title)?,
        plan.page_id
    )
    .expect("writing to a String cannot fail");
    let page_line = content.bytes().filter(|byte| *byte == b'\n').count() + 1;
    let links = resolved_links(plan, &plan.page_id)?;
    let fields = resolved_fields(&item, document(&item))?;
    let body = render_typst_document(
        document(&item),
        &TypstResources {
            links: &links,
            assets: &assets,
            fields: &fields,
            snippets: &snippets,
            excluded_block_ids: &HashSet::new(),
            macros: Some(&theme.macros),
        },
        2 - i16::try_from(min_heading_level(document(&item), &HashSet::new()).unwrap_or(1))
            .map_err(|_| "Page heading level is too large.")?,
    )
    .map_err(|error| format!("Page {}: {error}", plan.page_id))?;
    content.push_str(&body);
    let values_path = bundle_sibling(&theme.shell_path, "theme-values.typ");
    let content_path = bundle_sibling(&theme.shell_path, "content.typ");
    if files.contains_key(&values_path) || files.contains_key(&content_path) {
        return Err("Theme cannot replace generated PDF files.".to_owned());
    }
    files.insert(
        values_path,
        typst_values(&plan.theme.settings)?.into_bytes(),
    );
    files.insert(content_path, content.into_bytes());
    for asset in &plan.assets {
        let path = root
            .join("items")
            .join(&asset.owner_id[..2])
            .join(&asset.owner_id)
            .join("assets")
            .join(&asset.filename);
        let bytes =
            fs::read(&path).map_err(|error| format!("Cannot read asset {}: {error}", asset.src))?;
        let digest = Sha256::digest(&bytes);
        let hash: String = super::digest_hex(&digest);
        if bytes.len() as u64 != asset.bytes || hash != asset.sha256 {
            return Err(format!("Asset {} changed during PDF rendering.", asset.src));
        }
        let relative = bundle_sibling(
            &theme.shell_path,
            &format!("assets/{}/{}", asset.owner_id, asset.filename),
        );
        files.insert(relative, bytes);
    }
    assert_note_pdf_fresh(root, plan)?;
    Ok(PdfBundle {
        shell_path: theme.shell_path,
        files,
        page_lines: vec![(page_line, plan.page_id.clone())],
    })
}

fn resolved_links(
    plan: &NotePdfPlan,
    source_id: &str,
) -> Result<HashMap<String, TypstLink>, String> {
    let mut links = HashMap::new();
    for link in plan.links.iter().filter(|link| link.page_id == source_id) {
        let destination = match link.resolution {
            LinkResolution::Included => {
                let target = link
                    .target_id
                    .as_deref()
                    .ok_or("Included link has no target ID.")?;
                if target != plan.page_id {
                    return Err("Included PDF link points outside the source page.".to_owned());
                }
                TypstLink::Label(format!("note-{target}"))
            }
            LinkResolution::Anchor => {
                if link.target_id.as_deref() != Some(plan.page_id.as_str()) {
                    return Err(format!(
                        "Passage page {source_id} has an anchor outside this PDF."
                    ));
                }
                let anchor = link
                    .destination
                    .as_deref()
                    .ok_or("Anchor has no destination.")?
                    .strip_prefix('#')
                    .ok_or("Anchor has no fragment.")?;
                TypstLink::Label(anchor.to_owned())
            }
            LinkResolution::External => TypstLink::Url(
                link.destination
                    .as_deref()
                    .ok_or("Link has no destination.")?
                    .to_owned(),
            ),
            LinkResolution::TextOnly => TypstLink::PlainText,
        };
        if links
            .insert(link.href.clone(), destination.clone())
            .is_some_and(|prior| prior != destination)
        {
            return Err(format!(
                "Source page {source_id} has conflicting link targets."
            ));
        }
    }
    Ok(links)
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

    use super::{assert_note_pdf_fresh, plan_note_pdf, render_note_pdf};
    use crate::engine::project::asset::write_asset;
    use crate::engine::project::config::create_project_config;
    use crate::engine::project::properties::create_property;
    use crate::engine::project::write::write_item_input;
    use crate::engine::publication::pdf::publish_pdf;
    use crate::engine::publication::{OutsideLinks, PdfProfile};

    #[test]
    #[expect(
        clippy::too_many_lines,
        reason = "Keep each end-to-end scenario together so setup, failure checks and cleanup remain visible."
    )]
    fn renders_standalone_page_with_links_passages_assets_and_accessible_profile() {
        let binary = if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
            "linux-x64/typst"
        } else if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
            "windows-x64/typst.exe"
        } else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
            "macos-arm64/typst"
        } else {
            return;
        };
        let typst = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../typst/assets")
            .join(binary);
        if !typst.is_file() {
            return;
        }
        let root = std::env::temp_dir().join(format!("bluink-note-pdf-{}", uuid::Uuid::now_v7()));
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("test-fixtures/pdf-project");
        let config: Value =
            serde_json::from_slice(&std::fs::read(fixture.join("bluink.json")).unwrap()).unwrap();
        create_project_config(&root, config).unwrap();
        crate::engine::project::snippets::write_snippet(
            &root,
            "excerpt",
            json!({"type":"paragraph","content":[{"type":"text","text":"Quoted source"}]}),
            None,
        )
        .unwrap();
        std::fs::create_dir_all(root.join("properties")).unwrap();
        create_property(&root, json!({"key":"source_url","valueType":"text"})).unwrap();
        let ids: Vec<_> = (0..3).map(|_| uuid::Uuid::now_v7().to_string()).collect();
        let src = format!("/assets/{}/image.png", ids[0]);
        write_item_input(&root, json!({"id":ids[0],"meta":{"type":"page","title":"Article <one>","lang":"en-GB"},
            "content":{"type":"doc","content":[
                {"type":"paragraph","content":[{"type":"text","text":"Outside note","marks":[{"type":"link","attrs":{"href":format!("/?note={}",ids[1])}}]}]},
                {"type":"heading","attrs":{"level":2},"content":[{"type":"text","text":"First section"}]},
                {"type":"image","attrs":{"src":src,"alt":"A small cover"}},
                {"type":"embed","attrs":{"snippetId":"excerpt"}}
            ]}}), None, "2026-09-28").unwrap();
        write_item_input(&root, json!({"id":ids[1],"meta":{"type":"page","title":"Outside","source_url":"https://example.com/outside"},
            "content":{"type":"doc","content":[{"type":"paragraph","content":[{"type":"text","text":"Outside"}]}]}}), None, "2026-09-28").unwrap();
        write_item_input(&root, json!({"id":ids[2],"meta":{"type":"page","title":"Passage","lang":"en"},
            "content":{"type":"doc","content":[{"type":"paragraph","attrs":{"labels":["excerpt"]},"content":[{"type":"text","text":"Quoted source"}]}]}}),
            None, "2026-09-28").unwrap();
        write_asset(
            &root,
            &ids[0],
            "image.png",
            &std::fs::read(fixture.join("../one-pixel.png")).unwrap(),
        )
        .unwrap();
        assert!(
            plan_note_pdf(
                &root,
                &ids[0],
                "classic",
                PdfProfile::Default,
                OutsideLinks::Error
            )
            .unwrap_err()
            .contains("outside this Publication")
        );
        let plan = plan_note_pdf(
            &root,
            &ids[0],
            "classic",
            PdfProfile::Default,
            OutsideLinks::TextOnly,
        )
        .unwrap();
        let bundle = render_note_pdf(&root, &plan, false).unwrap();
        let source = String::from_utf8(bundle.files["pdf/content.typ"].clone()).unwrap();
        assert!(source.contains("#heading(level: 1)[#text(\"Article <one>\")]"));
        assert!(source.contains("#heading(level: 2)[#text(\"First section\")]"));
        assert!(!source.contains("https://example.com/outside"));
        assert!(source.contains("Quoted source"));
        let plain_plan = plan_note_pdf(
            &root,
            &ids[0],
            "classic",
            PdfProfile::Default,
            OutsideLinks::TextOnly,
        )
        .unwrap();
        let plain_bundle = render_note_pdf(&root, &plain_plan, false).unwrap();
        let plain_source =
            String::from_utf8(plain_bundle.files["pdf/content.typ"].clone()).unwrap();
        assert!(plain_source.contains("Outside note"));
        assert!(!plain_source.contains("https://example.com/outside"));
        assert!(
            bundle
                .files
                .contains_key(&format!("pdf/assets/{}/image.png", ids[0]))
        );
        publish_pdf(
            &bundle,
            plan.profile,
            &root.join("article.pdf"),
            &typst,
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("../fonts/assets/pdf"),
        )
        .unwrap();
        assert!(
            std::fs::read(root.join("article.pdf"))
                .unwrap()
                .starts_with(b"%PDF-")
        );
        let accessible = plan_note_pdf(
            &root,
            &ids[0],
            "classic",
            PdfProfile::Accessible,
            OutsideLinks::TextOnly,
        )
        .unwrap();
        let accessible_bundle = render_note_pdf(&root, &accessible, false).unwrap();
        publish_pdf(
            &accessible_bundle,
            accessible.profile,
            &root.join("accessible.pdf"),
            &typst,
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
            assert_note_pdf_fresh(&root, &plan)
                .unwrap_err()
                .contains("changed")
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn project_theme_requires_trust_and_cannot_claim_accessible_profile() {
        let root = std::env::temp_dir().join(format!("bluink-note-theme-{}", uuid::Uuid::now_v7()));
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("test-fixtures/pdf-project");
        let config: Value =
            serde_json::from_slice(&std::fs::read(fixture.join("bluink.json")).unwrap()).unwrap();
        create_project_config(&root, config).unwrap();
        std::fs::create_dir_all(root.join("properties")).unwrap();
        let id = uuid::Uuid::now_v7().to_string();
        write_item_input(&root, json!({"id":id,"meta":{"type":"page","title":"Themed note","lang":"en"},
            "content":{"type":"doc","content":[{"type":"paragraph","content":[{"type":"text","text":"A paragraph"}]}]}}),
            None, "2026-09-28").unwrap();
        let example = Path::new(env!("CARGO_MANIFEST_DIR")).join("../examples/themes/field-notes");
        let installed = root.join("themes/field-notes");
        for directory in ["html", "site", "pdf"] {
            std::fs::create_dir_all(installed.join(directory)).unwrap();
        }
        for relative in [
            "manifest.json",
            "html/article.html",
            "html/macros.html",
            "site/page.html",
            "pdf/document.typ",
            "pdf/macros.j2",
        ] {
            std::fs::copy(example.join(relative), installed.join(relative)).unwrap();
        }
        let plan = plan_note_pdf(
            &root,
            &id,
            "field-notes",
            PdfProfile::Default,
            OutsideLinks::Error,
        )
        .unwrap();
        assert!(
            render_note_pdf(&root, &plan, false)
                .err()
                .unwrap()
                .contains("trust")
        );
        let bundle = render_note_pdf(&root, &plan, true).unwrap();
        assert!(
            String::from_utf8(bundle.files["pdf/content.typ"].clone())
                .unwrap()
                .contains("#block(inset: 6pt, fill: surface)[")
        );
        assert!(
            plan_note_pdf(
                &root,
                &id,
                "field-notes",
                PdfProfile::Accessible,
                OutsideLinks::Error,
            )
            .unwrap_err()
            .contains("built-in themes only")
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
