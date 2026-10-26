// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::path::Path;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;

use crate::engine::project::item::read_project_item;
use crate::engine::publication::article::{image_mime, publish_article};
use crate::engine::publication::html::{
    HtmlResources, HtmlState, escape_html, html_endnotes, render_html_document_in_scope,
};
use crate::engine::publication::plan::{
    PlannedPage, PublicationFormat, PublicationPlan, assert_plan_fresh,
    reading::{BookEntry, body_heading_level, entries},
};
use crate::engine::publication::preflight::LinkResolution;
use crate::engine::publication::source::{document, min_heading_level, resolved_fields};
use crate::engine::publication::theme::{HtmlShellContext, HtmlShellKind, render_html_shell};

const MAX_ASSET_BYTES: u64 = 64 * 1024 * 1024;
const MAX_HTML_BYTES: usize = 128 * 1024 * 1024;

/// Render the complete Publication in reading order as one offline HTML file.
/// The article shell is shared with single-note HTML; the body owns the
/// Publication outline and a contents list.
#[expect(
    clippy::too_many_lines,
    reason = "Keep the complete format or operation dispatch together for contract review."
)]
#[expect(
    clippy::expect_used,
    reason = "Formatting only strings and primitive values into String is infallible."
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
        let bytes = super::render::read_asset(root, asset)?;
        assets.insert(
            asset.src.clone(),
            format!("data:{mime};base64,{}", STANDARD.encode(bytes)),
        );
    }

    let snippets: HashMap<String, serde_json::Value> = plan
        .snippets
        .iter()
        .map(|snippet| (snippet.id.clone(), snippet.paragraph.clone()))
        .collect();
    let mut body = String::new();
    let mut citation_scope = HtmlState::default();
    let mut contents = Contents::default();
    let mut sections = Vec::new();
    let mut contents_position = None;
    let root_page = plan.pages.first().ok_or("Publication has no home page.")?;
    for entry in entries(plan)? {
        let depth = match &entry {
            BookEntry::Section { depth, .. } => *depth,
            BookEntry::Page { id, .. } => planned_page(plan, id)?.depth,
        };
        while sections.last().is_some_and(|level| *level >= depth) {
            sections.pop();
            body.push_str("</section>\n");
        }
        match entry {
            BookEntry::Section {
                id,
                title,
                number,
                depth,
            } => {
                let title = format!("{number}. {title}");
                let anchor = format!("section-{id}");
                contents.push(depth, &anchor, &title);
                let level = depth.clamp(1, 6);
                writeln!(
                    body,
                    "<section class=\"publication-section publication-chapter\" id=\"{}\"><h{level}>{}</h{level}>",
                    escape_html(&anchor),
                    escape_html(&title)
                )
                .expect("writing to a String cannot fail");
                sections.push(depth);
            }
            BookEntry::Page { id, number, intro } => {
                let page = planned_page(plan, &id)?;
                let first = id == plan.root_page_id;
                let title = if intro {
                    None
                } else if first {
                    Some(plan.title.clone())
                } else {
                    Some(format!(
                        "{}. {}",
                        number.ok_or("Chapter has no authored number.")?,
                        page.title
                    ))
                };
                if !first && let Some(title) = &title {
                    contents.push(page.depth, &format!("note-{id}"), title);
                }
                render_page(
                    page,
                    title.as_deref(),
                    plan,
                    root,
                    &assets,
                    &snippets,
                    &mut body,
                    &mut citation_scope,
                )?;
                if first {
                    contents_position = Some(body.len());
                }
            }
        }
    }
    for _ in sections {
        body.push_str("</section>\n");
    }
    body.insert_str(
        contents_position.ok_or("Publication has no home page.")?,
        &contents.finish(),
    );
    body.push_str(&html_endnotes(&citation_scope));
    let root_item = read_project_item(root, &root_page.id)
        .map_err(|error| error.to_string())?
        .ok_or("Publication home page disappeared.")?;
    let language = root_item["meta"]["lang"].as_str().unwrap_or("und");
    let html = render_html_shell(
        root,
        &plan.theme,
        HtmlShellKind::Article,
        HtmlShellContext {
            title: &plan.title,
            language,
            body: &body,
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

#[derive(Default)]
struct Contents {
    html: String,
    depth: usize,
}

impl Contents {
    #[expect(
        clippy::expect_used,
        reason = "Formatting only strings and primitive values into String is infallible."
    )]
    fn push(&mut self, depth: usize, anchor: &str, title: &str) {
        let depth = depth.clamp(1, self.depth + 1);
        if self.depth == 0 {
            self.html.push_str(
                "<nav class=\"publication-contents\" aria-label=\"Contents\"><h2>Contents</h2>",
            );
        }
        if depth > self.depth {
            self.html.push_str("<ol>");
        } else {
            self.html.push_str("</li>");
            for _ in depth..self.depth {
                self.html.push_str("</ol></li>");
            }
        }
        write!(
            self.html,
            "<li><a href=\"#{}\">{}</a>",
            escape_html(anchor),
            escape_html(title)
        )
        .expect("writing to a String cannot fail");
        self.depth = depth;
    }

    fn finish(mut self) -> String {
        if self.depth > 0 {
            for _ in 0..self.depth {
                self.html.push_str("</li></ol>");
            }
            self.html.push_str("</nav>\n");
        }
        self.html
    }
}

fn planned_page<'a>(plan: &'a PublicationPlan, id: &str) -> Result<&'a PlannedPage, String> {
    plan.pages
        .iter()
        .find(|page| page.id == id)
        .ok_or_else(|| format!("Outline page {id} was not captured."))
}

#[expect(
    clippy::too_many_arguments,
    reason = "Keep the explicit publication context together at the planning/rendering boundary."
)]
#[expect(
    clippy::expect_used,
    reason = "Formatting only strings and primitive values into String is infallible."
)]
fn render_page(
    page: &PlannedPage,
    title: Option<&str>,
    plan: &PublicationPlan,
    root: &Path,
    assets: &HashMap<String, String>,
    snippets: &HashMap<String, serde_json::Value>,

    body: &mut String,
    citation_scope: &mut HtmlState,
) -> Result<(), String> {
    let source = read_project_item(root, &page.id)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("Publication page {} disappeared.", page.id))?;
    let exclusions: HashSet<String> = page.excluded_block_ids.iter().cloned().collect();
    let offset = body_heading_level(page.depth)?
        - i16::try_from(min_heading_level(document(&source), &exclusions).unwrap_or(1))
            .map_err(|_| "Page heading level is too large.")?;
    let html = render_html_document_in_scope(
        document(&source),
        &HtmlResources {
            page_id: &page.id,
            links: &resolved_links(plan, &page.id)?,
            assets,
            fields: &resolved_fields(&source, document(&source))?,
            snippets,
            excluded_block_ids: &exclusions,
        },
        offset,
        citation_scope,
    )
    .map_err(|error| format!("Page {}: {error}", page.id))?;
    let chapter_class = if title.is_none() {
        ""
    } else if page.id == plan.root_page_id {
        " class=\"publication-chapter publication-chapter--first\""
    } else {
        " class=\"publication-chapter\""
    };
    write!(body, "<article id=\"note-{}\"{chapter_class}>", page.id)
        .expect("writing to a String cannot fail");
    if let Some(title) = title {
        let level = page.depth.clamp(1, 6);
        write!(
            body,
            "<header><h{level}>{}</h{level}></header>",
            escape_html(title)
        )
        .expect("writing to a String cannot fail");
    }
    write!(body, "\n{html}</article>\n").expect("writing to a String cannot fail");
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
            if page.id != plan.root_page_id {
                assert!(html.contains(&format!("href=\"#note-{}\"", page.id)));
            }
            assert!(html.contains(&format!("id=\"note-{}\"", page.id)));
        }
        assert!(html.contains("aria-label=\"Contents\""));
        let contents = html
            .split("aria-label=\"Contents\"")
            .nth(1)
            .unwrap()
            .split("</nav>")
            .next()
            .unwrap();
        assert!(contents.contains("1. Chapter one"));
        assert!(contents.contains("1.1. A deeper section"));
        assert!(!contents.contains(&plan.title));
        assert!(
            html.contains(
                "<h2 id=\"block-01a0e2a7-d131-757d-8cdc-b07ace421ea7\">Introduction</h2>"
            )
        );
        assert!(html.find("</article>").unwrap() < html.find("aria-label=\"Contents\"").unwrap());
    }

    #[test]
    fn reading_entries_share_section_intros_collections_and_page_deduplication() {
        use crate::engine::publication::plan::PlannedStructureNode;
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("test-fixtures/pdf-project");
        let mut plan = plan_publication(
            &root,
            "01a0e7f5-d342-7f23-bd3d-6e27064a16cd",
            PublicationFormat::Html,
        )
        .unwrap();
        let intro = plan.pages[1].id.clone();
        let chapter = plan.pages[2].id.clone();
        plan.pages[1].depth = 2;
        plan.pages[2].depth = 3;
        let collection = |id: &str| PlannedStructureNode::Collection {
            id: id.into(),
            title: "Collected".into(),
            page_ids: vec![chapter.clone()],
            show_pages_in_navigation: true,
            listing_slug: None,
            page_size: 20,
            feed: false,
            archives: false,
        };
        plan.structure = vec![PlannedStructureNode::Section {
            id: "part".into(),
            title: "Part <one>".into(),
            intro_page_id: Some(intro.clone()),
            children: vec![collection("first"), collection("second")],
        }];
        let html = render_publication_html(&root, &plan, false).unwrap();
        assert_eq!(html.matches("<article id=\"note-").count(), 3);
        assert!(html.contains("<h1>1. Part &lt;one&gt;</h1>"));
        assert!(html.contains("<h3>1.1.1. A deeper section</h3>"));
        assert!(!html.contains("<header><h2>Chapter one</h2>"));
        let contents = html
            .split("aria-label=\"Contents\"")
            .nth(1)
            .unwrap()
            .split("</nav>")
            .next()
            .unwrap();
        assert!(!contents.contains(&format!("#note-{intro}")));
        assert_eq!(contents.matches(&format!("#note-{chapter}")).count(), 1);
        assert!(contents.contains("#section-first"));
        assert!(contents.contains("#section-second"));
        assert_eq!(
            html.matches("<section class=\"publication-section publication-chapter\"")
                .count(),
            3
        );
        assert_eq!(
            html.matches("</section>").count(),
            html.matches("<section").count()
        );
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
            page_id: "test-page",
            links: &values,
            assets: &values,
            fields: &values,
            snippets: &snippets,
            excluded_block_ids: &excluded,
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
            page_id: "test-page",
            links: &links,
            assets: &values,
            fields: &values,
            snippets: &snippets,
            excluded_block_ids: &excluded,
        };
        let mut scope = TypstState::default();
        let first = render_typst_document_in_scope(&document, &resources, 0, &mut scope).unwrap();
        let second = render_typst_document_in_scope(&document, &resources, 0, &mut scope).unwrap();
        assert!(first.contains("A shared source"));
        assert!(!second.contains("A shared source"));
        assert!(second.contains("#footnote(label(\"reference-1\"))"));
    }
}
