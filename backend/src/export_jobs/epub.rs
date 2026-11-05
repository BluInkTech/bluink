// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! Reflowable EPUB 3. Shared preparation/traversal and theme values feed XHTML;
//! chapter boundaries, navigation and OCF packaging belong to this exporter.
mod package;
#[cfg(test)]
mod tests;

use crate::domain::publication::{
    html::{self, HtmlResources, HtmlState, escape_html},
    page::PagePlan,
    plan::{
        PublicationFormat, PublicationPlan,
        reading::{BookEntry, entries},
    },
    preflight::{LinkResolution, PlannedAsset, PlannedLink, PlannedSnippet},
    source::{document, min_heading_level, parse_language, resolved_fields},
    theme_format::{ReferencesAt, ResolvedTheme, css},
};
use crate::export_jobs::{media::Asset, page::assert_page_fresh};
use crate::store::snapshots::read_project_item;
use package::{EPUB, NAMESPACE, Package, XHTML};
use serde_json::Value;
use std::{
    collections::{HashMap, HashSet},
    path::Path,
};

/// The chapter a book lists its sources in, when it lists them at its end.
const REFERENCES_FILE: &str = "references.xhtml";

pub(crate) struct EpubBundle {
    pub(crate) bytes: Vec<u8>,
}
struct Chapter {
    path: String,
    title: String,
    depth: usize,
}
struct Book {
    package: Package,
    chapters: Vec<Chapter>,
    assets: HashMap<String, String>,
    snippets: HashMap<String, Value>,
    /// The heading above a list of sources.
    references_heading: String,
    /// When the book lists its sources at its end, the notes every chapter shares.
    shared_notes: Option<HtmlState>,
}

impl Book {
    fn new(
        root: &Path,
        theme: &ResolvedTheme,
        captured: &[PlannedAsset],
        snippets: &[PlannedSnippet],
        web_fonts: &Path,
        svg_fonts: Option<&Path>,
    ) -> Result<Self, String> {
        let mut package = Package::default();
        let (font_css, font_assets) = crate::export_jobs::fonts::resources(
            theme.fonts.iter().map(String::as_str),
            web_fonts,
        )?;
        // The generated font stylesheet uses relative URLs; retain them intact.
        package.add("fonts/fonts.css", "text/css", font_css.into_bytes())?;
        for (path, bytes) in font_assets {
            package.add(&format!("fonts/{path}"), "font/woff2", bytes)?;
        }
        for id in &theme.fonts {
            let license = std::fs::read(web_fonts.join("licenses").join(format!("{id}.txt")))
                .map_err(|error| error.to_string())?;
            package.add(&format!("fonts/licenses/{id}.txt"), "text/plain", license)?;
        }
        package.add(
            "styles.css",
            "text/css",
            css::epub_stylesheet(theme)?.into_bytes(),
        )?;
        let mut assets = HashMap::new();
        let mut svg_cache = crate::export_jobs::media::SvgFonts::default();
        for source in captured {
            let image = Asset::parse_with_fonts(
                crate::export_jobs::resources::read_asset(root, source)?,
                svg_fonts,
                &mut svg_cache,
            )?;
            // PNG/JPEG are portable across readers. SVG and WebP use the same
            // offline raster fallback as Word, with no remote resource loading.
            let part = image.primary;
            assets.insert(source.src.clone(), format!("../{}", part.path));
            package.add(&part.path, &part.kind, part.bytes)?;
        }
        Ok(Self {
            package,
            chapters: Vec::new(),
            references_heading: theme.page.references_heading.clone(),
            shared_notes: None,
            assets,
            snippets: snippets
                .iter()
                .map(|s| (s.id.clone(), s.paragraph.clone()))
                .collect(),
        })
    }

    fn chapter(&mut self, chapter: Chapter, language: &str, body: &str) -> Result<(), String> {
        let title = escape_html(&chapter.title);
        let language = escape_html(language);
        let xml = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<html xmlns=\"{NAMESPACE}\" xmlns:epub=\"{EPUB}\" xml:lang=\"{language}\" lang=\"{language}\"><head><title>{title}</title><link rel=\"stylesheet\" type=\"text/css\" href=\"../styles.css\" /><link rel=\"stylesheet\" type=\"text/css\" href=\"../fonts/fonts.css\" /></head><body class=\"publication-content\"><h1>{title}</h1>{body}</body></html>"
        );
        crate::domain::xml::valid_characters(&xml)?;
        self.package.add(&chapter.path, XHTML, xml.into_bytes())?;
        self.chapters.push(chapter);
        Ok(())
    }

    fn page(
        &mut self,
        item: &Value,
        chapter: Chapter,
        id: &str,
        links: &[PlannedLink],
        exclusions: &[String],
    ) -> Result<(), String> {
        let links = chapter_links(links, id)?;
        let excluded_block_ids: HashSet<_> = exclusions.iter().cloned().collect();
        let fields = resolved_fields(item, document(item), &excluded_block_ids)?;
        let offset =
            2 - i16::try_from(min_heading_level(document(item), &excluded_block_ids).unwrap_or(1))
                .map_err(|_| "Page heading level is too large")?;
        let resources = HtmlResources {
            page_id: id,
            references_heading: &self.references_heading,
            links: &links,
            assets: &self.assets,
            fields: &fields,
            snippets: &self.snippets,
            excluded_block_ids: &excluded_block_ids,
        };
        // A chapter ends with its footnotes, and with its sources unless the
        // book lists them in a chapter of their own.
        let body = if let Some(notes) = &mut self.shared_notes {
            let mut body =
                html::render_html_document_in_scope(document(item), &resources, offset, notes)?;
            body.push_str(&html::html_footnotes(notes));
            crate::domain::xml::valid_characters(&body)?;
            body
        } else {
            html::render_xhtml_document(document(item), &resources, offset)?
        };
        self.chapter(chapter, &language(item)?, &body)
    }

    fn finish(mut self, id: &str, title: &str, language: &str) -> Result<EpubBundle, String> {
        if let Some(mut notes) = self.shared_notes.take() {
            let list = html::html_references(&mut notes, id, None)?;
            if !list.is_empty() {
                let heading = self.references_heading.clone();
                self.chapter(
                    Chapter {
                        path: format!("chapters/{REFERENCES_FILE}"),
                        title: heading,
                        depth: 1,
                    },
                    language,
                    &list,
                )?;
            }
        }
        self.package.add(
            "nav.xhtml",
            XHTML,
            package::navigation(title, language, &self.chapters)?,
        )?;
        Ok(EpubBundle {
            bytes: self.package.finish(
                &format!("urn:uuid:{id}"),
                title,
                language,
                &self.chapters,
            )?,
        })
    }
}

fn language(item: &Value) -> Result<String, String> {
    let value = item["meta"]["lang"].as_str().unwrap_or("und");
    let (lang, region) =
        parse_language(value).ok_or_else(|| format!("Invalid EPUB language: {value}"))?;
    Ok(region.map_or_else(|| lang.clone(), |region| format!("{lang}-{region}")))
}

fn chapter_links(links: &[PlannedLink], source: &str) -> Result<HashMap<String, String>, String> {
    links
        .iter()
        .filter(|link| link.page_id == source)
        .map(|link| {
            let target = match link.resolution {
                LinkResolution::Included => format!(
                    "{}.xhtml",
                    link.target_id.as_deref().ok_or("EPUB link has no target")?
                ),
                LinkResolution::Anchor => format!(
                    "{}.xhtml{}",
                    link.target_id
                        .as_deref()
                        .ok_or("EPUB anchor has no owner")?,
                    link.destination
                        .as_deref()
                        .ok_or("EPUB anchor has no fragment")?
                ),
                LinkResolution::External => link
                    .destination
                    .clone()
                    .ok_or("External link has no destination")?,
                LinkResolution::TextOnly => String::new(),
            };
            Ok((link.href.clone(), target))
        })
        .collect()
}

pub(crate) fn render_page(
    root: &Path,
    plan: &PagePlan,
    web_fonts: &Path,
    svg_fonts: Option<&Path>,
) -> Result<EpubBundle, String> {
    if plan.format != PublicationFormat::Epub {
        return Err("EPUB rendering requires an EPUB page plan".into());
    }
    assert_page_fresh(root, plan)?;
    let item = read_project_item(root, &plan.page_id)
        .map_err(|error| error.to_string())?
        .ok_or("EPUB source page disappeared")?;
    let mut book = Book::new(
        root,
        &plan.theme.resolved,
        &plan.assets,
        &plan.snippets,
        web_fonts,
        svg_fonts,
    )?;
    book.page(
        &item,
        Chapter {
            path: format!("chapters/{}.xhtml", plan.page_id),
            title: plan.title.clone(),
            depth: 1,
        },
        &plan.page_id,
        &plan.links,
        &[],
    )?;
    let bundle = book.finish(&plan.page_id, &plan.title, &language(&item)?)?;
    assert_page_fresh(root, plan)?;
    Ok(bundle)
}

pub(crate) fn render_publication(
    root: &Path,
    plan: &PublicationPlan,
    web_fonts: &Path,
    svg_fonts: Option<&Path>,
) -> Result<EpubBundle, String> {
    if plan.format != PublicationFormat::Epub {
        return Err("EPUB rendering requires an EPUB publication plan".into());
    }
    crate::export_jobs::plan::assert_plan_fresh(root, plan)?;
    let root_item = read_project_item(root, &plan.root_page_id)
        .map_err(|error| error.to_string())?
        .ok_or("EPUB root page disappeared")?;
    let language = language(&root_item)?;
    let mut book = Book::new(
        root,
        &plan.theme.resolved,
        &plan.assets,
        &plan.snippets,
        web_fonts,
        svg_fonts,
    )?;
    if plan.theme.resolved.page.references_at == ReferencesAt::Book {
        // Citations link into the closing references chapter beside them.
        book.shared_notes = Some(HtmlState::default().xhtml().listed_in(REFERENCES_FILE));
    }
    for entry in entries(plan)? {
        match entry {
            BookEntry::Section {
                id,
                title,
                number,
                depth,
            } => book.chapter(
                Chapter {
                    path: format!("chapters/section-{id}.xhtml"),
                    title: format!("{number}. {title}"),
                    depth,
                },
                &language,
                "",
            )?,
            BookEntry::Page { id, number, intro } => {
                let page = plan
                    .pages
                    .iter()
                    .find(|page| page.id == id)
                    .ok_or("EPUB reading-order page was not captured")?;
                let item = read_project_item(root, &id)
                    .map_err(|error| error.to_string())?
                    .ok_or("EPUB chapter disappeared")?;
                let title = if id == plan.root_page_id {
                    plan.title.clone()
                } else {
                    number.map_or_else(
                        || page.title.clone(),
                        |number| format!("{number}. {}", page.title),
                    )
                };
                book.page(
                    &item,
                    Chapter {
                        path: format!("chapters/{id}.xhtml"),
                        title,
                        depth: page.depth.max(1) + usize::from(intro),
                    },
                    &id,
                    &plan.links,
                    &page.excluded_block_ids,
                )?;
            }
        }
    }
    let bundle = book.finish(&plan.publication_id, &plan.title, &language)?;
    crate::export_jobs::plan::assert_plan_fresh(root, plan)?;
    Ok(bundle)
}

pub(crate) fn publish(bundle: &EpubBundle, destination: &Path) -> Result<(), String> {
    crate::export_jobs::output::publish_file(&bundle.bytes, destination, "EPUB", "epub")
}
