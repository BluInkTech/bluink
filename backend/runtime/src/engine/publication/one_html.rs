// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::fs;
use std::path::Path;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use sha2::{Digest, Sha256};

use crate::engine::project::item::read_project_item;
use crate::engine::publication::article::{image_mime, publish_article};
use crate::engine::publication::html::{
    HtmlResources, HtmlState, escape_html, html_endnotes, render_html_document_in_scope,
};
use crate::engine::publication::plan::{
    PlannedPage, PlannedStructureNode, PublicationFormat, PublicationPlan, assert_plan_fresh,
};
use crate::engine::publication::preflight::LinkResolution;
use crate::engine::publication::source::{document, min_heading_level, resolved_fields};
use crate::engine::publication::theme::{
    HtmlShellContext, HtmlShellKind, load_html_macros, render_html_shell,
};

const MAX_ASSET_BYTES: u64 = 64 * 1024 * 1024;
const MAX_HTML_BYTES: usize = 128 * 1024 * 1024;

/// Render the complete Publication in reading order as one offline HTML file.
/// The article shell is shared with single-note HTML; the body owns the
/// Publication outline and a contents list.
#[expect(
    clippy::too_many_lines,
    reason = "Keep the complete format or operation dispatch together for contract review."
)]
pub(crate) fn render_publication_html(
    root: &Path,
    plan: &PublicationPlan,
    trust_project_theme: bool,
) -> Result<String, String> {
    if !matches!(plan.format, PublicationFormat::Html) {
        return Err("A combined HTML renderer needs an HTML Publication plan.".to_owned());
    }
    assert_plan_fresh(root, plan)?;
    let macros = load_html_macros(
        root,
        &plan.theme,
        HtmlShellKind::Article,
        trust_project_theme,
    )?;
    let mut assets = HashMap::new();
    let mut total_asset_bytes = 0u64;
    for asset in &plan.assets {
        total_asset_bytes = total_asset_bytes
            .checked_add(asset.bytes)
            .ok_or("Publication image size total overflowed.")?;
        if total_asset_bytes > MAX_ASSET_BYTES {
            return Err("Publication images exceed the 64 MiB offline HTML limit.".to_owned());
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
        let digest: String = super::digest_hex(&Sha256::digest(&bytes));
        if bytes.len() as u64 != asset.bytes || digest != asset.sha256 {
            return Err(format!(
                "Image {} changed during HTML rendering.",
                asset.src
            ));
        }
        assets.insert(
            asset.src.clone(),
            format!("data:{mime};base64,{}", STANDARD.encode(bytes)),
        );
    }

    let mut sources = HashMap::new();
    for page in &plan.pages {
        sources.insert(
            page.id.clone(),
            read_project_item(root, &page.id)?
                .ok_or_else(|| format!("Publication page {} disappeared.", page.id))?,
        );
    }
    let snippets: HashMap<String, serde_json::Value> = plan
        .snippets
        .iter()
        .map(|snippet| (snippet.id.clone(), snippet.paragraph.clone()))
        .collect();
    let mut body = String::new();
    let mut citation_scope = HtmlState::default();
    let mut contents = String::from(
        "<nav class=\"publication-contents\" aria-label=\"Contents\"><h2>Contents</h2><ol>",
    );
    let mut rendered = HashSet::new();
    let root_page = plan.pages.first().ok_or("Publication has no home page.")?;
    write!(
        contents,
        "<li><a href=\"#note-{}\">{}</a></li>",
        root_page.id,
        escape_html(&root_page.title)
    )
    .expect("writing to a String cannot fail");
    render_page(
        root_page,
        plan,
        &sources,
        &assets,
        &snippets,
        &macros,
        &mut body,
        &mut citation_scope,
    )?;
    rendered.insert(root_page.id.as_str());
    render_outline(
        &plan.structure,
        plan,
        &sources,
        &assets,
        &snippets,
        &macros,
        &mut rendered,
        &mut contents,
        &mut body,
        &mut citation_scope,
    )?;
    contents.push_str("</ol></nav>\n");
    body.push_str(&html_endnotes(&citation_scope));
    if rendered.len() != plan.pages.len() {
        return Err("Publication outline does not account for every planned page.".to_owned());
    }
    let language = sources
        .get(&root_page.id)
        .and_then(|item| item["meta"]["lang"].as_str())
        .unwrap_or("und");
    let html = render_html_shell(
        root,
        &plan.theme,
        HtmlShellKind::Article,
        HtmlShellContext {
            title: &plan.title,
            language,
            body: &format!("{contents}{body}"),
            navigation: "",
            contents: "",
            role: "publication",
            page: serde_json::json!({"title":plan.title,"pageCount":plan.pages.len()}),
        },
        trust_project_theme,
    )?;
    if html.len() > MAX_HTML_BYTES {
        return Err("Publication exceeds the 128 MiB offline HTML limit.".to_owned());
    }
    assert_plan_fresh(root, plan)?;
    Ok(html)
}

pub(crate) fn publish_publication_html(html: &str, destination: &Path) -> Result<(), String> {
    publish_article(html, destination)
}

#[allow(clippy::too_many_arguments)]
#[expect(
    clippy::too_many_lines,
    reason = "Keep the complete format or operation dispatch together for contract review."
)]
fn render_outline<'a>(
    nodes: &[PlannedStructureNode],
    plan: &'a PublicationPlan,
    sources: &HashMap<String, serde_json::Value>,
    assets: &HashMap<String, String>,
    snippets: &HashMap<String, serde_json::Value>,
    macros: &crate::engine::publication::macros::RenderMacros,
    rendered: &mut HashSet<&'a str>,
    contents: &mut String,
    body: &mut String,
    citation_scope: &mut HtmlState,
) -> Result<(), String> {
    for node in nodes {
        match node {
            PlannedStructureNode::Page {
                page_id, children, ..
            } => {
                let page = planned_page(plan, page_id)?;
                write!(
                    contents,
                    "<li><a href=\"#note-{}\">{}</a>",
                    page.id,
                    escape_html(&page.title)
                )
                .expect("writing to a String cannot fail");
                if rendered.insert(page.id.as_str()) {
                    render_page(
                        page,
                        plan,
                        sources,
                        assets,
                        snippets,
                        macros,
                        body,
                        citation_scope,
                    )?;
                }
                if !children.is_empty() {
                    contents.push_str("<ol>");
                    render_outline(
                        children,
                        plan,
                        sources,
                        assets,
                        snippets,
                        macros,
                        rendered,
                        contents,
                        body,
                        citation_scope,
                    )?;
                    contents.push_str("</ol>");
                }
                contents.push_str("</li>");
            }
            PlannedStructureNode::Section {
                id,
                title,
                intro_page_id,
                children,
            } => {
                write!(
                    contents,
                    "<li><a href=\"#section-{id}\">{}</a>",
                    escape_html(title)
                )
                .expect("writing to a String cannot fail");
                writeln!(
                    body,
                    "<section class=\"publication-section\" id=\"section-{id}\"><h2>{}</h2>",
                    escape_html(title)
                )
                .expect("writing to a String cannot fail");
                contents.push_str("<ol>");
                if let Some(page_id) = intro_page_id {
                    let page = planned_page(plan, page_id)?;
                    write!(
                        contents,
                        "<li><a href=\"#note-{}\">{}</a></li>",
                        page.id,
                        escape_html(&page.title)
                    )
                    .expect("writing to a String cannot fail");
                    if rendered.insert(page.id.as_str()) {
                        render_page(
                            page,
                            plan,
                            sources,
                            assets,
                            snippets,
                            macros,
                            body,
                            citation_scope,
                        )?;
                    }
                }
                render_outline(
                    children,
                    plan,
                    sources,
                    assets,
                    snippets,
                    macros,
                    rendered,
                    contents,
                    body,
                    citation_scope,
                )?;
                contents.push_str("</ol></li>");
                body.push_str("</section>\n");
            }
            PlannedStructureNode::Collection {
                id,
                title,
                page_ids,
                ..
            } => {
                write!(
                    contents,
                    "<li><a href=\"#collection-{id}\">{}</a><ol>",
                    escape_html(title)
                )
                .expect("writing to a String cannot fail");
                writeln!(
                    body,
                    "<section class=\"publication-section\" id=\"collection-{id}\"><h2>{}</h2>",
                    escape_html(title)
                )
                .expect("writing to a String cannot fail");
                for page_id in page_ids {
                    let page = planned_page(plan, page_id)?;
                    write!(
                        contents,
                        "<li><a href=\"#note-{}\">{}</a></li>",
                        page.id,
                        escape_html(&page.title)
                    )
                    .expect("writing to a String cannot fail");
                    if rendered.insert(page.id.as_str()) {
                        render_page(
                            page,
                            plan,
                            sources,
                            assets,
                            snippets,
                            macros,
                            body,
                            citation_scope,
                        )?;
                    }
                }
                contents.push_str("</ol></li>");
                body.push_str("</section>\n");
            }
        }
    }
    Ok(())
}

fn planned_page<'a>(plan: &'a PublicationPlan, id: &str) -> Result<&'a PlannedPage, String> {
    plan.pages
        .iter()
        .find(|page| page.id == id)
        .ok_or_else(|| format!("Outline page {id} was not captured."))
}

#[allow(clippy::too_many_arguments)]
fn render_page(
    page: &PlannedPage,
    plan: &PublicationPlan,
    sources: &HashMap<String, serde_json::Value>,
    assets: &HashMap<String, String>,
    snippets: &HashMap<String, serde_json::Value>,
    macros: &crate::engine::publication::macros::RenderMacros,
    body: &mut String,
    citation_scope: &mut HtmlState,
) -> Result<(), String> {
    let source = sources
        .get(&page.id)
        .ok_or_else(|| format!("Publication page {} disappeared.", page.id))?;
    let exclusions: HashSet<String> = page.excluded_block_ids.iter().cloned().collect();
    let is_root = page.id == plan.root_page_id;
    let offset = (if is_root { 2 } else { 3 })
        - i16::try_from(min_heading_level(document(source), &exclusions).unwrap_or(1))
            .map_err(|_| "Page heading level is too large.")?;
    let html = render_html_document_in_scope(
        document(source),
        &HtmlResources {
            links: &resolved_links(plan, &page.id)?,
            assets,
            fields: &resolved_fields(source, document(source))?,
            snippets,
            excluded_block_ids: &exclusions,
            macros: Some(macros),
        },
        offset,
        citation_scope,
    )
    .map_err(|error| format!("Page {}: {error}", page.id))?;
    let heading = if is_root { "h1" } else { "h2" };
    write!(
        body,
        "<article id=\"note-{}\"><header><{heading}>{}</{heading}></header>\n{html}</article>\n",
        page.id,
        escape_html(if is_root { &plan.title } else { &page.title })
    )
    .expect("writing to a String cannot fail");
    Ok(())
}

fn resolved_links(
    plan: &PublicationPlan,
    source_id: &str,
) -> Result<HashMap<String, String>, String> {
    let mut links = HashMap::new();
    for link in plan.links.iter().filter(|link| link.page_id == source_id) {
        let target = match link.resolution {
            LinkResolution::Included => format!(
                "#note-{}",
                link.target_id
                    .as_deref()
                    .ok_or("Included link has no target ID.")?
            ),
            LinkResolution::TextOnly => String::new(),
            LinkResolution::Anchor | LinkResolution::External => link
                .destination
                .as_deref()
                .ok_or("Planned link has no destination.")?
                .to_owned(),
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

#[cfg(test)]
mod tests {
    use std::path::Path;

    use crate::engine::publication::plan::{PublicationFormat, plan_publication};

    use super::render_publication_html;

    #[test]
    fn renders_one_file_with_all_pages_and_internal_navigation() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("test-fixtures/pdf-project");
        let plan = plan_publication(
            &root,
            "01a0e7f5-d342-7f23-bd3d-6e27064a16cd",
            PublicationFormat::Html,
        )
        .unwrap();
        let html = render_publication_html(&root, &plan, false).unwrap();
        assert!(html.starts_with("<!doctype html>"));
        assert_eq!(
            html.matches("<article id=\"note-").count(),
            plan.pages.len()
        );
        for page in &plan.pages {
            assert!(html.contains(&format!("href=\"#note-{}\"", page.id)));
            assert!(html.contains(&format!("id=\"note-{}\"", page.id)));
        }
        assert!(html.contains("aria-label=\"Contents\""));
    }

    #[test]
    fn linked_citations_share_a_document_scope_and_sites_start_fresh() {
        use crate::engine::publication::html::{
            HtmlResources, HtmlState, html_endnotes, render_html_document,
            render_html_document_in_scope,
        };
        use crate::engine::publication::typst::{
            TypstResources, TypstState, render_typst_document_in_scope,
        };
        use serde_json::json;
        use std::collections::{HashMap, HashSet};
        let document = json!({"type":"doc","content":[{"type":"paragraph","content":[{"type":"citation","attrs":{"source":"snippet","snippetId":"shared"}}]}]});
        let snippets = HashMap::from([(
            "shared".to_owned(),
            json!({"type":"paragraph","content":[{"type":"text","text":"A shared source"}]}),
        )]);
        let values = HashMap::new();
        let excluded = HashSet::new();
        let resources = HtmlResources {
            links: &values,
            assets: &values,
            fields: &values,
            snippets: &snippets,
            excluded_block_ids: &excluded,
            macros: None,
        };
        let mut scope = HtmlState::default();
        let first = render_html_document_in_scope(&document, &resources, 0, &mut scope).unwrap();
        let second = render_html_document_in_scope(&document, &resources, 0, &mut scope).unwrap();
        let notes = html_endnotes(&scope);
        assert!(first.contains("fnref-1-1"));
        assert!(second.contains("fnref-1-2"));
        assert_eq!(notes.matches("A shared source").count(), 1);
        for _ in 0..2 {
            let page = render_html_document(&document, &resources, 0).unwrap();
            assert!(page.contains("fnref-1-1"));
            assert_eq!(page.matches("A shared source").count(), 1);
        }
        let links = HashMap::new();
        let resources = TypstResources {
            links: &links,
            assets: &values,
            fields: &values,
            snippets: &snippets,
            excluded_block_ids: &excluded,
            macros: None,
        };
        let mut scope = TypstState::default();
        let first = render_typst_document_in_scope(&document, &resources, 0, &mut scope).unwrap();
        let second = render_typst_document_in_scope(&document, &resources, 0, &mut scope).unwrap();
        assert!(first.contains("A shared source"));
        assert!(!second.contains("A shared source"));
        assert!(second.contains("#footnote(label(\"citation-1\"))"));
    }
}
