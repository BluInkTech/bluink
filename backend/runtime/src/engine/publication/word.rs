// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! Production DOCX consumes the same captured sources and prepared events.
use super::{
    docx::{DocxBundle, DocxDocument},
    page::{PagePlan, assert_page_fresh},
    plan::{
        PlannedPage, PublicationFormat, PublicationPlan, assert_plan_fresh,
        reading::{BookEntry, body_heading_level, entries},
    },
    preflight::{PlannedAsset, PlannedLink, PlannedSnippet},
    render::{self, Formatter, ReferenceScope, Resources},
    source::{document, min_heading_level, resolved_fields},
};
use crate::engine::project::item::read_project_item;
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
    emit_page(
        &item,
        &plan.page_id,
        2,
        &[],
        &plan.links,
        &assets,
        &snippets,
        &mut ReferenceScope::default(),
        &mut writer,
    )?;
    let bundle = writer.finish().map_err(|error| error.to_string())?;
    assert_page_fresh(root, plan)?;
    Ok(bundle)
}

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
    let mut scope = ReferenceScope::default();
    for entry in entries(plan)? {
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
                let page: &PlannedPage = plan
                    .pages
                    .iter()
                    .find(|page| page.id == id)
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
                emit_page(
                    &item,
                    &id,
                    heading_level,
                    &page.excluded_block_ids,
                    &plan.links,
                    &assets,
                    &snippets,
                    &mut scope,
                    &mut writer,
                )?;
                if first {
                    writer.contents_page().map_err(|error| error.to_string())?;
                }
            }
        }
    }
    let bundle = writer.finish().map_err(|error| error.to_string())?;
    assert_plan_fresh(root, plan)?;
    Ok(bundle)
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
            .add_image_resource(&asset.src, render::read_asset(root, asset)?, fonts)
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
    links: &[PlannedLink],
    assets: &HashMap<String, String>,
    snippets: &HashMap<String, Value>,
    scope: &mut ReferenceScope,
    writer: &mut DocxDocument<'_>,
) -> Result<(), String> {
    let links = render::native_links(links, id)?;
    let excluded_block_ids: HashSet<_> = exclusions.iter().cloned().collect();
    let fields = resolved_fields(item, document(item))?;
    let offset = heading_level
        - i16::try_from(min_heading_level(document(item), &excluded_block_ids).unwrap_or(1))
            .map_err(|_| "Page heading level is too large")?;
    let prepared = render::prepare_page(
        id,
        document(item),
        &Resources {
            links: &links,
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
