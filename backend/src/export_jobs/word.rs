// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! Production DOCX consumes the same captured sources and prepared events.
use crate::domain::publication::{
    page::PagePlan,
    plan::{
        PlannedPage, PublicationFormat, PublicationPlan,
        reading::{BookEntry, body_heading_level, entries},
    },
    preflight::{PlannedAsset, PlannedSnippet},
    render::{self, Destination, Formatter, ReferenceScope, Resources},
    source::{document, min_heading_level, resolved_fields},
    theme_format::ReferencesAt,
};
use crate::export_jobs::{
    docx::{DocxBundle, DocxDocument},
    page::assert_page_fresh,
    plan::assert_plan_fresh,
};
use crate::store::snapshots::read_project_item;
use serde_json::Value;
use std::{
    collections::{HashMap, HashSet},
    path::Path,
};

pub(crate) fn render_page(
    root: &Path,
    plan: &PagePlan,
    fonts: &Path,
    svg_fonts: Option<&Path>,
) -> Result<DocxBundle, String> {
    if plan.format != PublicationFormat::Docx {
        return Err("Word rendering requires a DOCX page plan.".into());
    }
    assert_page_fresh(root, plan)?;
    let item = read_project_item(root, &plan.page_id)
        .map_err(|error| error.to_string())?
        .ok_or("Word source page disappeared")?;
    let language = item["meta"]["lang"].as_str().unwrap_or("und");
    let mut writer = DocxDocument::new(&plan.theme.resolved, &plan.title, language, fonts)
        .map_err(|error| error.to_string())?;
    let assets = install_assets(root, &plan.assets, &mut writer, svg_fonts)?;
    writer
        .chapter(&plan.title, 1, &format!("note-{}", plan.page_id), true)
        .map_err(|error| error.to_string())?;
    let snippets = snippets(&plan.snippets);
    let mut scope = ReferenceScope::default();
    let links = render::native_links(&plan.links, &plan.page_id)?;
    emit_page(
        &item,
        &plan.page_id,
        2,
        &[],
        &links,
        &assets,
        &snippets,
        &mut scope,
        &mut writer,
    )?;
    emit_references(
        &mut scope,
        &plan.page_id,
        Some((&plan.theme.resolved.page.references_heading, 2)),
        &mut writer,
    )?;
    let bundle = writer.finish().map_err(|error| error.to_string())?;
    assert_page_fresh(root, plan)?;
    Ok(bundle)
}

#[expect(
    clippy::too_many_lines,
    reason = "One loop places every reading-order entry with its section break and its sources."
)]
pub(crate) fn render_publication(
    root: &Path,
    plan: &PublicationPlan,
    fonts: &Path,
    svg_fonts: Option<&Path>,
) -> Result<DocxBundle, String> {
    if plan.format != PublicationFormat::Docx {
        return Err("Word rendering requires a DOCX Publication plan.".into());
    }
    assert_plan_fresh(root, plan)?;
    let root_item = read_project_item(root, &plan.root_page_id)
        .map_err(|error| error.to_string())?
        .ok_or("Publication root page disappeared")?;
    let language = root_item["meta"]["lang"].as_str().unwrap_or("und");
    let mut writer = DocxDocument::new(&plan.theme.resolved, &plan.title, language, fonts)
        .map_err(|error| error.to_string())?;
    let assets = install_assets(root, &plan.assets, &mut writer, svg_fonts)?;
    let snippets = snippets(&plan.snippets);
    let links_by_page = render::native_links_by_page(&plan.links)?;
    let empty_links = HashMap::new();
    let pages: HashMap<_, _> = plan
        .pages
        .iter()
        .map(|page| (page.id.as_str(), page))
        .collect();
    let page_style = &plan.theme.resolved.page;
    let heading = page_style.references_heading.as_str();
    // A book listing its sources at its end numbers them straight through.
    let mut book_scope =
        (page_style.references_at == ReferencesAt::Book).then(ReferenceScope::default);
    // Each Bluink page is its own Word section, so its footnotes number
    // from one; a section heading opens the section of the page it introduces.
    let mut page_open = false;
    for entry in entries(plan)? {
        if page_open {
            writer.end_section().map_err(|error| error.to_string())?;
            page_open = false;
        }
        match entry {
            BookEntry::Section {
                id,
                title,
                number,
                depth,
            } => {
                writer
                    .set_content_language(language)
                    .map_err(|error| error.to_string())?;
                writer
                    .chapter(
                        &format!("{number}. {title}"),
                        u16::try_from(depth).map_err(|_| "Publication outline is too deep")?,
                        &format!("section-{id}"),
                        false,
                    )
                    .map_err(|error| error.to_string())?;
            }
            BookEntry::Page { id, number, intro } => {
                let page: &PlannedPage = pages
                    .get(id.as_str())
                    .copied()
                    .ok_or("Reading-order page was not captured")?;
                let item = read_project_item(root, &id)
                    .map_err(|error| error.to_string())?
                    .ok_or_else(|| format!("Publication page {id} disappeared"))?;
                writer
                    .set_content_language(item["meta"]["lang"].as_str().unwrap_or("und"))
                    .map_err(|error| error.to_string())?;
                let first = id == plan.root_page_id;
                let anchor = format!("note-{id}");
                if intro {
                    writer
                        .page_anchor(&anchor)
                        .map_err(|error| error.to_string())?;
                } else {
                    let title = if first {
                        plan.title.clone()
                    } else {
                        format!(
                            "{}. {}",
                            number.as_deref().ok_or("Chapter has no authored number")?,
                            page.title
                        )
                    };
                    writer
                        .chapter(
                            &title,
                            u16::try_from(page.depth.max(1))
                                .map_err(|_| "Publication outline is too deep")?,
                            &anchor,
                            first,
                        )
                        .map_err(|error| error.to_string())?;
                }
                let heading_level = body_heading_level(page.depth)?;
                let mut chapter_scope = ReferenceScope::named(&id);
                emit_page(
                    &item,
                    &id,
                    heading_level,
                    &page.excluded_block_ids,
                    links_by_page.get(id.as_str()).unwrap_or(&empty_links),
                    &assets,
                    &snippets,
                    book_scope.as_mut().unwrap_or(&mut chapter_scope),
                    &mut writer,
                )?;
                // Each chapter ends with the sources it cites, unless the book lists them.
                if book_scope.is_none() {
                    let level =
                        u16::try_from(heading_level).map_err(|_| "Heading level is invalid")?;
                    emit_references(&mut chapter_scope, &id, Some((heading, level)), &mut writer)?;
                }
                if first {
                    writer.contents_page().map_err(|error| error.to_string())?;
                }
                page_open = true;
            }
        }
    }
    if let Some(scope) = &mut book_scope
        && let Some(list) = scope.take_list(&plan.root_page_id, None)
    {
        // The book's sources close it as a chapter of their own in the contents.
        writer
            .chapter(heading, 1, "references", false)
            .map_err(|error| error.to_string())?;
        render::walk(&list, &mut writer).map_err(|error| error.to_string())?;
    }
    let bundle = writer.finish().map_err(|error| error.to_string())?;
    assert_plan_fresh(root, plan)?;
    Ok(bundle)
}

/// The sources a scope has gathered, written after the text that cited them.
fn emit_references(
    scope: &mut ReferenceScope,
    page_id: &str,
    heading: Option<(&str, u16)>,
    writer: &mut DocxDocument<'_>,
) -> Result<(), String> {
    scope.take_list(page_id, heading).map_or(Ok(()), |list| {
        render::walk(&list, writer).map_err(|error| error.to_string())
    })
}

fn snippets(captured: &[PlannedSnippet]) -> HashMap<String, Value> {
    captured
        .iter()
        .map(|snippet| (snippet.id.clone(), snippet.paragraph.clone()))
        .collect()
}

fn install_assets(
    root: &Path,
    captured: &[PlannedAsset],
    writer: &mut DocxDocument<'_>,
    fonts: Option<&Path>,
) -> Result<HashMap<String, String>, String> {
    for asset in captured {
        writer
            .add_image_resource(
                &asset.src,
                crate::export_jobs::resources::read_asset(root, asset)?,
                fonts,
            )
            .map_err(|error| format!("Asset {}: {error}", asset.src))?;
    }
    Ok(captured
        .iter()
        .map(|asset| (asset.src.clone(), asset.src.clone()))
        .collect())
}

#[expect(
    clippy::too_many_arguments,
    reason = "Pass captured resources explicitly into the shared preparer without giving the formatter source access."
)]
fn emit_page(
    item: &Value,
    id: &str,
    heading_level: i16,
    exclusions: &[String],
    links: &HashMap<String, Destination>,
    assets: &HashMap<String, String>,
    snippets: &HashMap<String, Value>,
    scope: &mut ReferenceScope,
    writer: &mut DocxDocument<'_>,
) -> Result<(), String> {
    let excluded_block_ids: HashSet<_> = exclusions.iter().cloned().collect();
    let fields = resolved_fields(item, document(item), &excluded_block_ids)?;
    let offset = heading_level
        - i16::try_from(min_heading_level(document(item), &excluded_block_ids).unwrap_or(1))
            .map_err(|_| "Page heading level is too large")?;
    let prepared = render::prepare_page(
        id,
        document(item),
        &Resources {
            links,
            assets,
            fields: &fields,
            snippets,
            excluded_block_ids: &excluded_block_ids,
        },
        offset,
        scope,
    )
    .map_err(|error| error.to_string())?;
    render::walk(&prepared, writer).map_err(|error| error.to_string())
}
