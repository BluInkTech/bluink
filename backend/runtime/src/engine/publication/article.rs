// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use std::collections::{HashMap, HashSet};

use std::path::Path;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;

use crate::engine::project::item::read_project_item;
use crate::engine::publication::html::{HtmlResources, escape_html, render_html_document};
use crate::engine::publication::preflight::LinkResolution;
use crate::engine::publication::source::{document, min_heading_level, resolved_fields};
use crate::engine::publication::theme::{HtmlShellContext, HtmlShellKind, render_html_shell};

use super::page::{PagePlan, assert_page_fresh};

const MAX_ASSET_BYTES: u64 = 64 * 1024 * 1024;
const MAX_HTML_BYTES: usize = 128 * 1024 * 1024;

pub(crate) fn render_article(
    root: &Path,
    plan: &PagePlan,
    trust_project_theme: bool,
) -> Result<String, String> {
    if plan.format != super::plan::PublicationFormat::Html {
        return Err("HTML rendering requires an HTML page plan.".into());
    }
    assert_page_fresh(root, plan)?;
    let item = read_project_item(root, &plan.page_id)
        .map_err(|error| error.to_string())?
        .ok_or("Article source disappeared.")?;
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
        let bytes = super::render::read_asset(root, asset)?;
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
    let heading_offset =
        2 - i16::try_from(min_heading_level(document(&item), &HashSet::new()).unwrap_or(1))
            .map_err(|_| "Page heading level is too large.")?;
    let body = render_html_document(
        document(&item),
        &HtmlResources {
            page_id: &plan.page_id,
            links: &links,
            assets: &asset_urls,
            fields: &fields,
            snippets: &snippets,
            excluded_block_ids: &HashSet::new(),
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
    assert_page_fresh(root, plan)?;
    Ok(html)
}

pub(crate) fn publish_article(html: &str, destination: &Path) -> Result<(), String> {
    super::output::publish_file(html.as_bytes(), destination, "HTML", "html")
}

fn resolved_links(plan: &PagePlan, source_id: &str) -> Result<HashMap<String, String>, String> {
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
    crate::engine::media_type::for_path(Path::new(filename)).filter(|mime| {
        matches!(
            *mime,
            "image/png" | "image/jpeg" | "image/webp" | "image/gif" | "image/svg+xml"
        )
    })
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use serde_json::{Value, json};

    use super::{publish_article, render_article};
    use crate::engine::project::asset::write_asset;
    use crate::engine::project::config::create_project_config;
    use crate::engine::project::properties::create_property;
    use crate::engine::project::write::write_item_input;
    use crate::engine::publication::page::{assert_page_fresh, plan_page};
    use crate::engine::publication::plan::PublicationFormat;
    use crate::engine::publication::{OutsideLinks, PdfProfile};

    #[test]
    fn missing_internal_target_can_be_omitted_without_source_metadata() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("project");
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("test-fixtures/pdf-project");
        let config =
            serde_json::from_slice(&std::fs::read(fixture.join("bluink.json")).unwrap()).unwrap();
        create_project_config(&root, config).unwrap();
        crate::engine::project::properties::initialize(&root).unwrap();
        let source = uuid::Uuid::now_v7().to_string();
        let target = uuid::Uuid::now_v7().to_string();
        write_item_input(&root, json!({"id": source,"meta":{"type":"page","title":"Ordinary page"},
            "content":{"type":"doc","content":[{"type":"paragraph","content":[
                {"type":"text","text":"Keep these words","marks":[{"type":"link","attrs":{"href":format!("/?note={target}")}}]},
                {"type":"text","text":"Website","marks":[{"type":"link","attrs":{"href":"https://example.com/"}}]}
            ]}]}}), None, "2026-09-30").unwrap();
        let html_plan = plan_page(
            &root,
            &source,
            "classic",
            OutsideLinks::TextOnly,
            PublicationFormat::Html,
            PdfProfile::Default,
        )
        .unwrap();
        let html = render_article(&root, &html_plan, false).unwrap();
        assert!(html.contains("Keep these words"));
        assert!(!html.contains(&target));
        assert!(html.contains("href=\"https://example.com/\""));
        assert!(
            plan_page(
                &root,
                &source,
                "classic",
                OutsideLinks::Error,
                PublicationFormat::Html,
                PdfProfile::Default
            )
            .unwrap_err()
            .contains("outside")
        );
        let pdf_plan = plan_page(
            &root,
            &source,
            "classic",
            OutsideLinks::TextOnly,
            PublicationFormat::Pdf,
            PdfProfile::Default,
        )
        .unwrap();
        let pdf = crate::engine::publication::note_pdf::render_note_pdf(&root, &pdf_plan).unwrap();
        let content = String::from_utf8(pdf.files["content.typ"].clone()).unwrap();
        assert!(content.contains("Keep these words"));
        assert!(!content.contains(&target));
        assert!(
            plan_page(
                &root,
                &source,
                "classic",
                OutsideLinks::Error,
                PublicationFormat::Pdf,
                PdfProfile::Default
            )
            .unwrap_err()
            .contains("outside")
        );
        temporary.close().unwrap();
    }

    #[test]
    #[expect(
        clippy::too_many_lines,
        reason = "Keep the complete offline article scenario, stale dependencies and trust checks together."
    )]
    fn renders_offline_article_from_resolved_sources() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("project");
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("test-fixtures/pdf-project");
        let config: Value =
            serde_json::from_slice(&std::fs::read(fixture.join("bluink.json")).unwrap()).unwrap();
        create_project_config(&root, config).unwrap();
        crate::engine::project::properties::initialize(&root).unwrap();
        crate::engine::project::snippets::write_snippet(
            &root,
            "excerpt",
            json!({"type":"paragraph","content":[{"type":"text","text":"Quoted & safe"}]}),
            None,
        )
        .unwrap();
        crate::engine::project::properties::initialize(&root).unwrap();
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
        let plan = plan_page(
            &root,
            &ids[0],
            "classic",
            OutsideLinks::TextOnly,
            PublicationFormat::Html,
            PdfProfile::Default,
        )
        .unwrap();
        let html = render_article(&root, &plan, false).unwrap();
        assert!(html.contains("Article &lt;one&gt;"));
        assert!(html.contains("<h2"));
        assert!(!html.contains("https://example.com/outside"), "{html}");
        assert!(html.contains("data:image/png;base64,"));
        assert!(html.contains("Quoted &amp; safe"));
        assert!(html.contains("<html lang=\"en\">"));
        let plain_plan = plan_page(
            &root,
            &ids[0],
            "classic",
            OutsideLinks::TextOnly,
            PublicationFormat::Html,
            PdfProfile::Default,
        )
        .unwrap();
        let plain_html = render_article(&root, &plain_plan, false).unwrap();
        assert!(plain_html.contains("Outside note"));
        assert!(!plain_html.contains("https://example.com/outside"));
        assert!(!plain_html.contains(&format!("/?note={}", ids[1])));
        let source_theme = Path::new(env!("CARGO_MANIFEST_DIR")).join("vendor/themes/classic");
        let project_theme = root.join("themes/custom");
        for directory in ["html", "site"] {
            std::fs::create_dir_all(project_theme.join(directory)).unwrap();
        }
        for relative in ["html/article.html", "site/page.html", "site/listing.html"] {
            std::fs::copy(source_theme.join(relative), project_theme.join(relative)).unwrap();
        }
        let manifest_path = project_theme.join("manifest.json");
        let mut manifest: Value =
            serde_json::from_slice(&std::fs::read(source_theme.join("manifest.json")).unwrap())
                .unwrap();
        manifest["id"] = json!("custom");
        std::fs::write(&manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
        let shell_path = project_theme.join("html/article.html");
        let shell = std::fs::read_to_string(&shell_path).unwrap();
        std::fs::write(
            &shell_path,
            shell.replace("publication--classic", "publication--custom"),
        )
        .unwrap();
        let custom_plan = plan_page(
            &root,
            &ids[0],
            "custom",
            OutsideLinks::TextOnly,
            PublicationFormat::Html,
            PdfProfile::Default,
        )
        .unwrap();
        assert!(
            render_article(&root, &custom_plan, false)
                .unwrap_err()
                .contains("requires explicit trust")
        );
        let custom_html = render_article(&root, &custom_plan, true).unwrap();
        assert!(custom_html.contains("publication--custom"));
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
            assert_page_fresh(&root, &plan)
                .unwrap_err()
                .contains("changed")
        );
        temporary.close().unwrap();
    }
}
