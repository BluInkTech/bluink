// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::engine::project::item::read_project_item;
use crate::engine::publication::OutsideLinks;
use crate::engine::publication::html::{HtmlResources, escape_html, render_html_document};
use crate::engine::publication::plan::PlannedPage;
use crate::engine::publication::preflight::{
    LinkResolution, PlannedAsset, PlannedLink, PlannedSnippet, assert_preflight_fresh,
    preflight_pages,
};
use crate::engine::publication::source::{document, min_heading_level, resolved_fields};
use crate::engine::publication::theme::{
    HtmlShellContext, HtmlShellKind, ThemeOrigin, ThemeSnapshot, capture_article_theme,
    load_html_macros, render_html_shell,
};

const MAX_ASSET_BYTES: u64 = 64 * 1024 * 1024;
const MAX_HTML_BYTES: usize = 128 * 1024 * 1024;

#[derive(Debug, Deserialize, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub(crate) struct ArticlePlan {
    pub(crate) page_id: String,
    pub(crate) page_revision: String,
    pub(crate) title: String,
    pub(crate) outside_links: OutsideLinks,
    pub(crate) theme: ThemeSnapshot,
    pub(crate) assets: Vec<PlannedAsset>,
    pub(crate) links: Vec<PlannedLink>,
    pub(crate) snippets: Vec<PlannedSnippet>,
}

pub(crate) fn plan_article(
    root: &Path,
    page_id: &str,
    theme_id: &str,
    outside_links: OutsideLinks,
) -> Result<ArticlePlan, String> {
    let page = read_project_item(root, page_id)?
        .ok_or_else(|| format!("Article page {page_id} does not exist."))?;
    if page["meta"]["type"] != "page" || page["meta"]["state"] != "active" {
        return Err(format!("Article source {page_id} must be an active page."));
    }
    let title = page["meta"]["title"]
        .as_str()
        .filter(|title| !title.trim().is_empty())
        .ok_or("Article page needs a title.")?
        .to_owned();
    let revision = page["revision"]
        .as_str()
        .ok_or("Article page has no revision.")?
        .to_owned();
    let theme = capture_article_theme(root, theme_id)?;
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
    let plan = ArticlePlan {
        page_id: page_id.to_owned(),
        page_revision: revision,
        title,
        outside_links,
        theme,
        assets: preflight.assets,
        links: preflight.links,
        snippets: preflight.snippets,
    };
    assert_article_fresh(root, &plan)?;
    Ok(plan)
}

pub(crate) fn assert_article_fresh(root: &Path, plan: &ArticlePlan) -> Result<(), String> {
    let current = read_project_item(root, &plan.page_id)?;
    if current.as_ref().and_then(|page| page["revision"].as_str()) != Some(&plan.page_revision) {
        return Err(format!(
            "Article page {} changed after planning.",
            plan.page_id
        ));
    }
    let theme = capture_article_theme(root, &plan.theme.id)?;
    if theme.sha256 != plan.theme.sha256
        || !matches!(
            (theme.origin, plan.theme.origin),
            (ThemeOrigin::Builtin, ThemeOrigin::Builtin)
                | (ThemeOrigin::Project, ThemeOrigin::Project)
        )
    {
        return Err(format!(
            "Article theme {} changed after planning.",
            plan.theme.id
        ));
    }
    assert_preflight_fresh(root, &plan.assets, &plan.links, &plan.snippets)
}

pub(crate) fn render_article(
    root: &Path,
    plan: &ArticlePlan,
    trust_project_theme: bool,
) -> Result<String, String> {
    assert_article_fresh(root, plan)?;
    let item = read_project_item(root, &plan.page_id)?.ok_or("Article source disappeared.")?;
    let mut asset_urls = HashMap::new();
    let mut total_asset_bytes = 0u64;
    for asset in &plan.assets {
        total_asset_bytes = total_asset_bytes
            .checked_add(asset.bytes)
            .ok_or("Article image size total overflowed.")?;
        if total_asset_bytes > MAX_ASSET_BYTES {
            return Err("Article images exceed the 64 MiB offline HTML limit.".to_owned());
        }
        let mime = image_mime(&asset.filename)
            .ok_or_else(|| format!("Unsupported offline HTML image: {}", asset.src))?;
        let path = root
            .join("items")
            .join(&asset.owner_id[..2])
            .join(&asset.owner_id)
            .join("assets")
            .join(&asset.filename);
        let bytes =
            fs::read(path).map_err(|error| format!("Cannot read image {}: {error}", asset.src))?;
        let hash: String = super::digest_hex(&Sha256::digest(&bytes));
        if bytes.len() as u64 != asset.bytes || hash != asset.sha256 {
            return Err(format!(
                "Image {} changed during HTML rendering.",
                asset.src
            ));
        }
        asset_urls.insert(
            asset.src.clone(),
            format!("data:{mime};base64,{}", STANDARD.encode(bytes)),
        );
    }
    let snippets: HashMap<String, serde_json::Value> = plan
        .snippets
        .iter()
        .map(|snippet| (snippet.id.clone(), snippet.paragraph.clone()))
        .collect();
    let links = resolved_links(plan, &plan.page_id)?;
    let fields = resolved_fields(&item, document(&item))?;
    let macros = load_html_macros(
        root,
        &plan.theme,
        HtmlShellKind::Article,
        trust_project_theme,
    )?;
    let heading_offset =
        2 - i16::try_from(min_heading_level(document(&item), &HashSet::new()).unwrap_or(1))
            .map_err(|_| "Page heading level is too large.")?;
    let body = render_html_document(
        document(&item),
        &HtmlResources {
            links: &links,
            assets: &asset_urls,
            fields: &fields,
            snippets: &snippets,
            excluded_block_ids: &HashSet::new(),
            macros: Some(&macros),
        },
        heading_offset,
    )
    .map_err(|error| format!("Page {}: {error}", plan.page_id))?;
    let article = format!(
        "<article id=\"note-{}\"><header><h1>{}</h1></header>\n{body}</article>\n",
        plan.page_id,
        escape_html(&plan.title)
    );
    let language = item["meta"]["lang"].as_str().unwrap_or("und");
    let html = render_html_shell(
        root,
        &plan.theme,
        HtmlShellKind::Article,
        HtmlShellContext {
            title: &plan.title,
            language,
            body: &article,
            navigation: "",
            contents: "",
            role: "article",
            page: serde_json::json!({"title":plan.title}),
        },
        trust_project_theme,
    )?;
    if html.len() > MAX_HTML_BYTES {
        return Err("Article exceeds the 128 MiB offline HTML limit.".to_owned());
    }
    assert_article_fresh(root, plan)?;
    Ok(html)
}

pub(crate) fn publish_article(html: &str, destination: &Path) -> Result<(), String> {
    match fs::symlink_metadata(destination) {
        Ok(_) => {
            return Err(format!(
                "HTML output already exists: {}",
                destination.display()
            ));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(format!("Cannot inspect HTML output: {error}")),
    }
    let parent = destination
        .parent()
        .filter(|path| path.is_dir())
        .ok_or("HTML output parent must be an existing folder.")?;
    let name = destination
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or("HTML output needs a filename.")?;
    if !name.to_ascii_lowercase().ends_with(".html") {
        return Err("HTML output filename must end in .html.".to_owned());
    }
    let stage = parent.join(format!(".{name}.bluink-{}.tmp", uuid::Uuid::now_v7()));
    fs::write(&stage, html).map_err(|error| format!("Cannot stage HTML output: {error}"))?;
    let result = fs::hard_link(&stage, destination)
        .map_err(|error| format!("Cannot publish HTML output: {error}"));
    let _ = fs::remove_file(&stage);
    result
}

fn resolved_links(plan: &ArticlePlan, source_id: &str) -> Result<HashMap<String, String>, String> {
    let mut links = HashMap::new();
    for link in plan.links.iter().filter(|link| link.page_id == source_id) {
        let target = match link.resolution {
            LinkResolution::Included => format!("#note-{}", plan.page_id),
            LinkResolution::Anchor => {
                if link.target_id.as_deref() != Some(plan.page_id.as_str()) {
                    return Err(format!(
                        "Linked section from page {source_id} is outside this article."
                    ));
                }
                link.destination
                    .as_deref()
                    .ok_or("Anchor has no destination.")?
                    .to_owned()
            }
            LinkResolution::External => link
                .destination
                .as_deref()
                .ok_or("Link has no destination.")?
                .to_owned(),
            LinkResolution::TextOnly => String::new(),
        };
        if links
            .insert(link.href.clone(), target.clone())
            .is_some_and(|prior| prior != target)
        {
            return Err(format!(
                "Source page {source_id} has conflicting link targets."
            ));
        }
    }
    Ok(links)
}

pub(crate) fn image_mime(filename: &str) -> Option<&'static str> {
    match Path::new(filename)
        .extension()?
        .to_str()?
        .to_ascii_lowercase()
        .as_str()
    {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "webp" => Some("image/webp"),
        "gif" => Some("image/gif"),
        "svg" => Some("image/svg+xml"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use serde_json::{Value, json};

    use super::{assert_article_fresh, plan_article, publish_article, render_article};
    use crate::engine::project::asset::write_asset;
    use crate::engine::project::config::create_project_config;
    use crate::engine::project::properties::create_property;
    use crate::engine::project::write::write_item_input;
    use crate::engine::publication::OutsideLinks;

    #[test]
    fn missing_internal_target_can_be_omitted_without_source_metadata() {
        let root =
            std::env::temp_dir().join(format!("bluink-missing-link-{}", uuid::Uuid::now_v7()));
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("test-fixtures/pdf-project");
        let config =
            serde_json::from_slice(&std::fs::read(fixture.join("bluink.json")).unwrap()).unwrap();
        create_project_config(&root, config).unwrap();
        std::fs::create_dir_all(root.join("properties")).unwrap();
        let source = uuid::Uuid::now_v7().to_string();
        let target = uuid::Uuid::now_v7().to_string();
        write_item_input(&root, json!({"id": source,"meta":{"type":"page","title":"Ordinary page"},
            "content":{"type":"doc","content":[{"type":"paragraph","content":[
                {"type":"text","text":"Keep these words","marks":[{"type":"link","attrs":{"href":format!("/?note={target}")}}]},
                {"type":"text","text":"Website","marks":[{"type":"link","attrs":{"href":"https://example.com/"}}]}
            ]}]}}), None, "2026-09-30").unwrap();
        let html_plan = plan_article(&root, &source, "classic", OutsideLinks::TextOnly).unwrap();
        let html = render_article(&root, &html_plan, false).unwrap();
        assert!(html.contains("Keep these words"));
        assert!(!html.contains(&target));
        assert!(html.contains("href=\"https://example.com/\""));
        assert!(
            plan_article(&root, &source, "classic", OutsideLinks::Error)
                .unwrap_err()
                .contains("outside")
        );
        let pdf_plan = crate::engine::publication::note_pdf::plan_note_pdf(
            &root,
            &source,
            "classic",
            crate::engine::publication::PdfProfile::Default,
            OutsideLinks::TextOnly,
        )
        .unwrap();
        let pdf =
            crate::engine::publication::note_pdf::render_note_pdf(&root, &pdf_plan, false).unwrap();
        let content = String::from_utf8(pdf.files["pdf/content.typ"].clone()).unwrap();
        assert!(content.contains("Keep these words"));
        assert!(!content.contains(&target));
        assert!(
            crate::engine::publication::note_pdf::plan_note_pdf(
                &root,
                &source,
                "classic",
                crate::engine::publication::PdfProfile::Default,
                OutsideLinks::Error
            )
            .unwrap_err()
            .contains("outside")
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    #[expect(
        clippy::too_many_lines,
        reason = "Keep each end-to-end scenario together so setup, failure checks and cleanup remain visible."
    )]
    fn renders_offline_article_from_resolved_sources() {
        let root = std::env::temp_dir().join(format!("bluink-article-{}", uuid::Uuid::now_v7()));
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("test-fixtures/pdf-project");
        let config: Value =
            serde_json::from_slice(&std::fs::read(fixture.join("bluink.json")).unwrap()).unwrap();
        create_project_config(&root, config).unwrap();
        crate::engine::project::snippets::write_snippet(
            &root,
            "excerpt",
            json!({"type":"paragraph","content":[{"type":"text","text":"Quoted & safe"}]}),
            None,
        )
        .unwrap();
        std::fs::create_dir_all(root.join("properties")).unwrap();
        create_property(&root, json!({"key":"source_url","valueType":"text"})).unwrap();
        let ids: Vec<_> = (0..3).map(|_| uuid::Uuid::now_v7().to_string()).collect();
        let image_src = format!("/assets/{}/cover.png", ids[0]);
        write_item_input(&root, json!({"id":ids[0],"meta":{"type":"page","title":"Article <one>","lang":"en"},
            "content":{"type":"doc","content":[
                {"type":"paragraph","content":[{"type":"text","text":"Outside note","marks":[{"type":"link","attrs":{"href":format!("/?note={}",ids[1])}}]}]},
                {"type":"heading","attrs":{"level":2},"content":[{"type":"text","text":"First section"}]},
                {"type":"image","attrs":{"src":image_src,"alt":"A small cover"}},
                {"type":"embed","attrs":{"snippetId":"excerpt"}}
            ]}}), None, "2026-09-28").unwrap();
        write_item_input(&root, json!({"id":ids[1],"meta":{"type":"page","title":"Outside","source_url":"https://example.com/outside"},
            "content":{"type":"doc","content":[{"type":"paragraph","content":[{"type":"text","text":"Outside"}]}]}}), None, "2026-09-28").unwrap();
        write_item_input(&root, json!({"id":ids[2],"meta":{"type":"page","title":"Passage"},
            "content":{"type":"doc","content":[{"type":"paragraph","attrs":{"labels":["excerpt"]},"content":[{"type":"text","text":"Quoted & safe"}]}]}}),
            None, "2026-09-28").unwrap();
        write_asset(
            &root,
            &ids[0],
            "cover.png",
            &std::fs::read(fixture.join("../one-pixel.png")).unwrap(),
        )
        .unwrap();
        let plan = plan_article(&root, &ids[0], "classic", OutsideLinks::TextOnly).unwrap();
        let html = render_article(&root, &plan, false).unwrap();
        assert!(html.contains("Article &lt;one&gt;"));
        assert!(html.contains("<h2"));
        assert!(!html.contains("https://example.com/outside"), "{html}");
        assert!(html.contains("data:image/png;base64,"));
        assert!(html.contains("Quoted &amp; safe"));
        assert!(html.contains("<html lang=\"en\">"));
        let plain_plan = plan_article(&root, &ids[0], "classic", OutsideLinks::TextOnly).unwrap();
        let plain_html = render_article(&root, &plain_plan, false).unwrap();
        assert!(plain_html.contains("Outside note"));
        assert!(!plain_html.contains("https://example.com/outside"));
        assert!(!plain_html.contains(&format!("/?note={}", ids[1])));
        let source_theme = Path::new(env!("CARGO_MANIFEST_DIR")).join("vendor/themes/classic");
        let project_theme = root.join("themes/custom");
        for directory in ["html", "site", "pdf"] {
            std::fs::create_dir_all(project_theme.join(directory)).unwrap();
        }
        for relative in [
            "html/article.html",
            "site/page.html",
            "site/listing.html",
            "pdf/document.typ",
        ] {
            std::fs::copy(source_theme.join(relative), project_theme.join(relative)).unwrap();
        }
        let manifest_path = project_theme.join("manifest.json");
        let mut manifest: Value =
            serde_json::from_slice(&std::fs::read(source_theme.join("manifest.json")).unwrap())
                .unwrap();
        manifest["id"] = json!("custom");
        manifest["macros"]["html"] = json!("html/macros.html");
        std::fs::write(&manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
        std::fs::write(
            project_theme.join("html/macros.html"),
            "{% macro paragraph(ctx, children) %}<p{{ ctx.attributes }} class=\"prose\">{{ children }}</p>{% endmacro %}",
        )
        .unwrap();
        let custom_plan = plan_article(&root, &ids[0], "custom", OutsideLinks::TextOnly).unwrap();
        assert!(
            render_article(&root, &custom_plan, false)
                .unwrap_err()
                .contains("requires explicit trust")
        );
        let custom_html = render_article(&root, &custom_plan, true).unwrap();
        assert!(custom_html.contains("class=\"prose\">"));
        assert!(custom_html.contains("Outside note"));
        assert!(custom_html.contains("Quoted &amp; safe</p>"));
        publish_article(&html, &root.join("article.html")).unwrap();
        assert!(
            std::fs::read_to_string(root.join("article.html"))
                .unwrap()
                .contains("Quoted &amp; safe")
        );
        assert!(
            publish_article(&html, &root.join("article.html"))
                .unwrap_err()
                .contains("already exists")
        );
        write_asset(&root, &ids[0], "cover.png", b"changed").unwrap();
        assert!(
            assert_article_fresh(&root, &plan)
                .unwrap_err()
                .contains("changed")
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
