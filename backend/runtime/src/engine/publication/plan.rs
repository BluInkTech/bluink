// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

mod listings;

use listings::*;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::engine::project::item::read_project_item;
use crate::engine::publication::preflight::{
    PagePreflight, PlannedAsset, PlannedLink, PlannedSnippet, assert_dependencies_fresh,
    internal_page_id, preflight_publication,
};
use crate::engine::publication::selection::excluded_block_ids;
use crate::engine::publication::source::{document, parse_language};
use crate::engine::publication::theme::{
    ThemeOrigin, ThemeSnapshot, apply_theme_options, apply_theme_settings, assert_theme_fresh,
    capture_theme,
};
use crate::engine::publication::{
    OutlineNode, OutsideLinks, PdfProfile, PublicationContent, PublicationTargets, SelectionEffect,
    SelectionMatcherKind, SelectionRule, SelectionTarget, validate_publication,
};

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(test, derive(ts_rs::TS))]
pub(crate) enum PublicationFormat {
    Pdf,
    Html,
    Site,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub(crate) enum PublicationSourceKind {
    Publication,
    LinkedPages,
}

impl PublicationFormat {
    pub(crate) fn parse(value: &str) -> Result<Self, String> {
        match value {
            "pdf" => Ok(Self::Pdf),
            "html" => Ok(Self::Html),
            "site" => Ok(Self::Site),
            _ => Err(format!(
                "Unsupported Publication format: {value}. Use pdf, html, or site."
            )),
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub(crate) struct PublicationPlan {
    pub(crate) source_kind: PublicationSourceKind,
    pub(crate) publication_id: String,
    pub(crate) publication_revision: String,
    pub(crate) title: String,
    pub(crate) format: PublicationFormat,
    pub(crate) theme_id: String,
    pub(crate) theme: ThemeSnapshot,
    pub(crate) root_page_id: String,
    pub(crate) structure: Vec<PlannedStructureNode>,
    pub(crate) pages: Vec<PlannedPage>,
    pub(crate) collections: Vec<PlannedCollection>,
    pub(crate) listings: Vec<PlannedListing>,
    pub(crate) assets: Vec<PlannedAsset>,
    pub(crate) links: Vec<PlannedLink>,
    pub(crate) snippets: Vec<PlannedSnippet>,
    pub(crate) selection_rules: Vec<SelectionRule>,
    pub(crate) targets: PublicationTargets,
}

#[derive(Debug, Deserialize, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub(crate) struct PlannedListing {
    pub(crate) route: String,
    pub(crate) base_path: String,
    pub(crate) title: String,
    pub(crate) collection_id: String,
    pub(crate) page_ids: Vec<String>,
    pub(crate) page_number: usize,
    pub(crate) total_pages: usize,
}

#[derive(Debug, Deserialize, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum PlannedStructureNode {
    Page {
        #[serde(rename = "pageId")]
        page_id: String,
        #[serde(rename = "showInNavigation")]
        show_in_navigation: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        #[cfg_attr(test, ts(optional))]
        slug: Option<String>,
        children: Vec<Self>,
    },
    Section {
        id: String,
        title: String,
        #[serde(rename = "introPageId", skip_serializing_if = "Option::is_none")]
        #[cfg_attr(test, ts(optional))]
        intro_page_id: Option<String>,
        children: Vec<Self>,
    },
    Collection {
        id: String,
        title: String,
        #[serde(rename = "pageIds")]
        page_ids: Vec<String>,
        #[serde(rename = "showPagesInNavigation")]
        show_pages_in_navigation: bool,
        #[serde(rename = "listingSlug", skip_serializing_if = "Option::is_none")]
        #[cfg_attr(test, ts(optional))]
        listing_slug: Option<String>,
        #[serde(rename = "pageSize")]
        page_size: u16,
        feed: bool,
        archives: bool,
    },
}

#[derive(Debug, Deserialize, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub(crate) struct PlannedCollection {
    pub(crate) id: String,
    pub(crate) title: String,
    pub(crate) query: String,
    /// Raw query membership before selection rules. Used to detect a stale
    /// dynamic collection without repeating the complete Publication plan.
    pub(crate) resolved_ids: Vec<String>,
    pub(crate) page_ids: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub(crate) listing_slug: Option<String>,
    pub(crate) page_size: u16,
    pub(crate) feed: bool,
    pub(crate) archives: bool,
}

#[derive(Debug, Deserialize, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub(crate) struct PlannedPage {
    pub(crate) id: String,
    pub(crate) revision: String,
    pub(crate) title: String,
    pub(crate) route: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub(crate) published_on: Option<String>,
    pub(crate) tags: Vec<String>,
    pub(crate) depth: usize,
    pub(crate) excluded_block_ids: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub(crate) parent_id: Option<String>,
}

#[cfg(test)]
pub(crate) fn plan_publication(
    root: &Path,
    publication_id: &str,
    format: PublicationFormat,
) -> Result<PublicationPlan, String> {
    plan_publication_resolved(root, publication_id, format, &HashMap::new())
}

#[tracing::instrument(level = "debug", skip_all, fields(format = ?format, collection_count = resolved_collections.len()))]
pub(crate) fn plan_publication_resolved(
    root: &Path,
    publication_id: &str,
    format: PublicationFormat,
    resolved_collections: &HashMap<String, Vec<String>>,
) -> Result<PublicationPlan, String> {
    let item = read_project_item(root, publication_id)?
        .ok_or_else(|| format!("Publication {publication_id} does not exist."))?;
    plan_publication_item(
        root,
        &item,
        format,
        resolved_collections,
        PublicationSourceKind::Publication,
    )
}

/// Build a Publication plan from the transitive internal-link graph without
/// creating a saved Publication item. Discovery is breadth first in document
/// order, so a cycle never duplicates a page and repeated plans are stable.
#[cfg(test)]
pub(crate) fn plan_linked_pages(
    root: &Path,
    page_id: &str,
    format: PublicationFormat,
    theme_id: &str,
    outside_links: OutsideLinks,
    profile: PdfProfile,
) -> Result<PublicationPlan, String> {
    plan_linked_pages_with_graph(
        root,
        page_id,
        format,
        theme_id,
        outside_links,
        profile,
        None,
    )
}

#[allow(clippy::too_many_arguments)]
#[tracing::instrument(level = "debug", skip_all, fields(format = ?format, outside_links = ?outside_links, profile = ?profile, has_cached_graph = graph.is_some()))]
#[expect(
    clippy::too_many_lines,
    reason = "Keep the complete format or operation dispatch together for contract review."
)]
#[expect(
    clippy::needless_pass_by_value,
    reason = "The operation consumes its input snapshot rather than exposing a borrowed transaction API."
)]
#[expect(
    clippy::needless_collect,
    reason = "Collect all worker handles before joining so work remains concurrent."
)]
pub(crate) fn plan_linked_pages_with_graph(
    root: &Path,
    page_id: &str,
    format: PublicationFormat,
    theme_id: &str,
    outside_links: OutsideLinks,
    profile: PdfProfile,
    graph: Option<&HashMap<String, Vec<String>>>,
) -> Result<PublicationPlan, String> {
    let root_page = read_active_page(root, page_id)?;
    let content = validate_publication(&serde_json::json!({
        "kind": "publication",
        "preset": "documentation",
        "selectedOutput": "site",
        "rootPageId": page_id,
        "outline": [],
        "selectionRules": [],
        "themeId": theme_id,
        "targets": {
            "pdf": {"profile": profile, "outsideLinks": outside_links},
            "html": {"outsideLinks": outside_links},
            "site": {"layout": "documentation", "outsideLinks": outside_links},
        },
    }))?;
    let theme = capture_theme(root, theme_id, format)?;
    let mut plan = PublicationPlan {
        source_kind: PublicationSourceKind::LinkedPages,
        publication_id: page_id.to_owned(),
        publication_revision: revision(&root_page)?.to_owned(),
        title: title(&root_page)?.to_owned(),
        format,
        theme_id: theme_id.to_owned(),
        theme,
        root_page_id: page_id.to_owned(),
        structure: Vec::new(),
        pages: Vec::new(),
        collections: Vec::new(),
        listings: Vec::new(),
        assets: Vec::new(),
        links: Vec::new(),
        snippets: Vec::new(),
        selection_rules: Vec::new(),
        targets: content.targets.clone(),
    };
    if format == PublicationFormat::Pdf
        && matches!(profile, PdfProfile::Accessible)
        && !matches!(plan.theme.origin, ThemeOrigin::Builtin)
    {
        return Err(
            "Accessible PDF currently supports unmodified reviewed built-in themes only."
                .to_owned(),
        );
    }
    let mut seen = HashSet::from([page_id.to_owned()]);
    let mut queue = VecDeque::new();
    let mut preflight = PagePreflight::default();
    let capture = |page: Value,
                   plan: &mut PublicationPlan,
                   queue: &mut VecDeque<String>,
                   seen: &mut HashSet<String>,
                   preflight: &mut PagePreflight|
     -> Result<(), String> {
        let id = page["id"].as_str().ok_or("Linked page has no ID.")?;
        let is_root = id == page_id;
        let source = planned_page(
            &page,
            usize::from(!is_root),
            (!is_root).then(|| page_id.to_owned()),
            &content,
        )?;
        // Validate content now, before discovering the rest of a potentially
        // project-sized graph. Do not read or parse the same document again.
        preflight.scan(root, &source, &page).map_err(|error| {
            format!(
                "While checking “{}” ({} pages checked, {} discovered): {error}",
                source.title,
                plan.pages.len(),
                seen.len()
            )
        })?;
        if format == PublicationFormat::Pdf && matches!(profile, PdfProfile::Accessible) {
            require_accessible_source(&page, document(&page), &[])?;
        }
        let mut links = Vec::new();
        collect_internal_links(document(&page), &mut links);
        // A canonical edit racing the snapshot must still discover its new
        // targets. Preserve authored ordering while checking the snapshot set.
        if let Some(indexed) = graph.and_then(|graph| graph.get(id)) {
            let authored: HashSet<_> = links.iter().collect();
            let indexed_set: HashSet<_> = indexed.iter().collect();
            if authored == indexed_set {
                let order: HashMap<_, _> = links
                    .iter()
                    .enumerate()
                    .map(|(position, target)| (target.clone(), position))
                    .collect();
                links.clone_from(indexed);
                links.sort_by_key(|target| order[target]);
            }
        }
        for target in links {
            if seen.insert(target.clone()) {
                queue.push_back(target);
            }
        }
        if !is_root {
            plan.structure.push(PlannedStructureNode::Page {
                page_id: id.to_owned(),
                show_in_navigation: true,
                slug: None,
                children: Vec::new(),
            });
        }
        plan.pages.push(source);
        Ok(())
    };
    capture(root_page, &mut plan, &mut queue, &mut seen, &mut preflight)?;
    let workers = std::thread::available_parallelism()
        .map_or(1, usize::from)
        .min(4);
    loop {
        let ids: Vec<_> = std::mem::take(&mut queue).into_iter().collect();
        if ids.is_empty() {
            break;
        }
        let loaded = std::thread::scope(|scope| {
            let handles: Vec<_> = ids
                .chunks(ids.len().div_ceil(workers))
                .map(|ids| {
                    let policy = &outside_links;
                    scope.spawn(move || {
                        ids.iter()
                            .map(|id| read_linked_page(root, id, policy))
                            .collect::<Vec<_>>()
                    })
                })
                .collect();
            handles
                .into_iter()
                .map(|handle| {
                    handle
                        .join()
                        .map_err(|_| "Linked-page reader failed.".to_owned())
                        .map_err(|error| vec![Err(error)])
                })
                .flat_map(|result| result.unwrap_or_else(|errors| errors))
                .collect::<Vec<_>>()
        });
        for page in loaded {
            if let Some(page) = page? {
                capture(page, &mut plan, &mut queue, &mut seen, &mut preflight)?;
            }
        }
    }
    assign_routes(&mut plan)?;
    let checked = preflight.finish(root, &plan.pages, &outside_links)?;
    plan.assets = checked.assets;
    plan.links = checked.links;
    plan.snippets = checked.snippets;
    assert_plan_fresh(root, &plan)?;
    Ok(plan)
}

fn collect_internal_links(node: &Value, output: &mut Vec<String>) {
    if let Some(marks) = node["marks"].as_array() {
        for mark in marks {
            if mark["type"] == "link"
                && let Some(href) = mark["attrs"]["href"].as_str()
                && let Some(id) = internal_page_id(href)
            {
                output.push(id);
            }
        }
    }
    if let Some(children) = node["content"].as_array() {
        for child in children {
            collect_internal_links(child, output);
        }
    }
    if node["attrs"]["paragraph"].is_object() {
        collect_internal_links(&node["attrs"]["paragraph"], output);
    }
}

fn plan_publication_item(
    root: &Path,
    item: &Value,
    format: PublicationFormat,
    resolved_collections: &HashMap<String, Vec<String>>,
    source_kind: PublicationSourceKind,
) -> Result<PublicationPlan, String> {
    let publication_id = item["id"].as_str().ok_or("Publication has no ID.")?;
    if item["meta"]["type"] != "publication" || item["meta"]["state"] != "active" {
        return Err(format!(
            "Item {publication_id} is not an active Publication."
        ));
    }
    let content = validate_publication(&item["content"])?;
    let mut theme = capture_theme(root, &content.theme_id, format)?;
    if let Some(settings) = &content.theme_settings {
        apply_theme_settings(
            &mut theme,
            &serde_json::to_value(settings).map_err(|error| error.to_string())?,
        )?;
    }
    apply_theme_options(&mut theme, &content.theme_options)?;
    if matches!(format, PublicationFormat::Pdf)
        && matches!(content.targets.pdf.profile, PdfProfile::Accessible)
        && (!matches!(theme.origin, ThemeOrigin::Builtin) || content.theme_settings.is_some())
    {
        return Err(
            "Accessible PDF currently supports unmodified reviewed built-in themes only."
                .to_owned(),
        );
    }
    let mut plan = PublicationPlan {
        source_kind,
        publication_id: publication_id.to_owned(),
        publication_revision: revision(item)?.to_owned(),
        title: title(item)?.to_owned(),
        format,
        theme_id: content.theme_id.clone(),
        theme,
        root_page_id: content.root_page_id.clone(),
        structure: Vec::new(),
        pages: Vec::new(),
        collections: Vec::new(),
        listings: Vec::new(),
        assets: Vec::new(),
        links: Vec::new(),
        snippets: Vec::new(),
        selection_rules: content.selection_rules.clone(),
        targets: content.targets.clone(),
    };
    let root_page = read_active_page(root, &content.root_page_id)?;
    if !include_page(&root_page, &content) {
        return Err("The Publication's root page is excluded by a selection rule.".to_owned());
    }
    plan.pages
        .push(planned_page(&root_page, 0, None, &content)?);
    for node in &content.outline {
        if let Some(node) = capture_outline(
            root,
            node,
            (&content.root_page_id, 1),
            &content,
            resolved_collections,
            &mut plan.pages,
            &mut plan.collections,
        )? {
            plan.structure.push(node);
        }
    }
    if matches!(format, PublicationFormat::Site)
        && plan.collections.iter().any(|collection| collection.feed)
        && content.targets.site.base_url.is_none()
    {
        return Err("A site address is required before generating a blog feed.".to_owned());
    }
    assign_routes(&mut plan)?;
    if matches!(format, PublicationFormat::Site) {
        plan_listings(&mut plan)?;
    }
    preflight_publication(root, &mut plan, &content)?;
    if matches!(format, PublicationFormat::Pdf)
        && matches!(content.targets.pdf.profile, PdfProfile::Accessible)
    {
        for page in &plan.pages {
            let item = read_active_page(root, &page.id)?;
            require_accessible_source(&item, document(&item), &page.excluded_block_ids)?;
        }
    }
    // External edits may bypass the application's item locks. Recheck every
    // captured revision before handing this plan to a renderer or CLI caller.
    assert_plan_fresh(root, &plan)?;
    Ok(plan)
}

pub(crate) fn require_accessible_source(
    item: &Value,
    node: &Value,
    exclusions: &[String],
) -> Result<(), String> {
    let id = item["id"].as_str().unwrap_or("unknown");
    if item["meta"]["lang"]
        .as_str()
        .and_then(parse_language)
        .is_none()
    {
        return Err(format!("Accessible PDF requires a language on page {id}."));
    }
    require_table_headers(node, exclusions, id)
}

fn require_table_headers(node: &Value, exclusions: &[String], id: &str) -> Result<(), String> {
    if node["attrs"]["id"]
        .as_str()
        .is_some_and(|block_id| exclusions.iter().any(|excluded| excluded == block_id))
    {
        return Ok(());
    }
    if node["type"] == "table" {
        let cells = node["content"]
            .as_array()
            .and_then(|rows| rows.first())
            .and_then(|row| row["content"].as_array());
        if !cells.is_some_and(|cells| {
            !cells.is_empty() && cells.iter().all(|cell| cell["type"] == "table_header")
        }) {
            return Err(format!(
                "Accessible PDF requires first-row header cells in page {id}."
            ));
        }
    }
    if let Some(children) = node["content"].as_array() {
        for child in children {
            require_table_headers(child, exclusions, id)?;
        }
    }
    if let Some(fragment) = node["attrs"]["paragraph"].as_object() {
        require_table_headers(&Value::Object(fragment.clone()), exclusions, id)?;
    }
    Ok(())
}

pub(crate) fn assert_plan_fresh(root: &Path, plan: &PublicationPlan) -> Result<(), String> {
    let current = read_project_item(root, &plan.publication_id)?;
    if current.as_ref().and_then(|item| item["revision"].as_str())
        != Some(&plan.publication_revision)
    {
        return Err(format!(
            "Publication {} changed during planning.",
            plan.publication_id
        ));
    }
    for page in &plan.pages {
        let current = read_project_item(root, &page.id)?;
        if current.as_ref().and_then(|item| item["revision"].as_str()) != Some(&page.revision) {
            return Err(format!(
                "Page {} changed during Publication planning.",
                page.id
            ));
        }
    }
    assert_theme_fresh(root, &plan.theme, plan.format)?;
    assert_dependencies_fresh(root, plan)?;
    Ok(())
}

#[expect(
    clippy::too_many_lines,
    reason = "Keep the complete format or operation dispatch together for contract review."
)]
fn capture_outline(
    root: &Path,
    node: &OutlineNode,
    position: (&str, usize),
    content: &PublicationContent,
    resolved_collections: &HashMap<String, Vec<String>>,
    pages: &mut Vec<PlannedPage>,
    collections: &mut Vec<PlannedCollection>,
) -> Result<Option<PlannedStructureNode>, String> {
    let (parent_id, depth) = position;
    match node {
        OutlineNode::Page {
            page_id,
            children,
            show_in_navigation,
            slug,
        } => {
            let item = read_active_page(root, page_id)?;
            if !include_page(&item, content) {
                return Ok(None);
            }
            pages.push(planned_page(
                &item,
                depth,
                Some(parent_id.to_owned()),
                content,
            )?);
            let mut planned_children = Vec::new();
            for child in children {
                if let Some(planned) = capture_outline(
                    root,
                    child,
                    (page_id, depth + 1),
                    content,
                    resolved_collections,
                    pages,
                    collections,
                )? {
                    planned_children.push(planned);
                }
            }
            Ok(Some(PlannedStructureNode::Page {
                page_id: page_id.clone(),
                show_in_navigation: *show_in_navigation,
                slug: slug.clone(),
                children: planned_children,
            }))
        }
        OutlineNode::Section {
            id,
            title,
            intro_page_id,
            children,
        } => {
            let mut included_intro = None;
            if let Some(page_id) = intro_page_id {
                let item = read_active_page(root, page_id)?;
                if include_page(&item, content) {
                    pages.push(planned_page(&item, depth + 1, Some(id.clone()), content)?);
                    included_intro = Some(page_id.clone());
                }
            }
            let mut planned_children = Vec::new();
            for child in children {
                if let Some(planned) = capture_outline(
                    root,
                    child,
                    (id, depth + 1),
                    content,
                    resolved_collections,
                    pages,
                    collections,
                )? {
                    planned_children.push(planned);
                }
            }
            Ok(Some(PlannedStructureNode::Section {
                id: id.clone(),
                title: title.clone(),
                intro_page_id: included_intro,
                children: planned_children,
            }))
        }
        OutlineNode::Collection {
            id,
            title,
            query,
            show_pages_in_navigation,
            listing_slug,
            page_size,
            feed,
            archives,
        } => {
            let ids = resolved_collections
                .get(id)
                .ok_or("Collection queries need to be resolved before planning.")?;
            let mut included = Vec::new();
            for page_id in ids {
                let item = read_active_page(root, page_id)?;
                if !include_page(&item, content) {
                    continue;
                }
                if pages.iter().any(|page| page.id == *page_id) {
                    included.push(page_id.clone());
                    continue;
                }
                pages.push(planned_page(&item, depth + 1, Some(id.clone()), content)?);
                included.push(page_id.clone());
            }
            if (*feed || *archives) && listing_slug.is_none() {
                return Err("Blog collections need a listing address.".to_owned());
            }
            if *feed || *archives {
                for page_id in &included {
                    let page = pages
                        .iter()
                        .find(|page| &page.id == page_id)
                        .ok_or("Blog page was not planned.")?;
                    if page.published_on.is_none() {
                        return Err(format!("Blog page {} needs a publication date.", page.id));
                    }
                }
            }
            collections.push(PlannedCollection {
                id: id.clone(),
                title: title.clone(),
                query: query.clone(),
                resolved_ids: ids.clone(),
                page_ids: included,
                listing_slug: listing_slug.clone(),
                page_size: *page_size,
                feed: *feed,
                archives: *archives,
            });
            Ok(Some(PlannedStructureNode::Collection {
                id: id.clone(),
                title: title.clone(),
                page_ids: collections
                    .last()
                    .ok_or("Missing planned collection")?
                    .page_ids
                    .clone(),
                show_pages_in_navigation: *show_pages_in_navigation,
                listing_slug: listing_slug.clone(),
                page_size: *page_size,
                feed: *feed,
                archives: *archives,
            }))
        }
    }
}

fn read_active_page(root: &Path, id: &str) -> Result<Value, String> {
    let item =
        read_project_item(root, id)?.ok_or_else(|| format!("Publication page {id} is missing."))?;
    if item["meta"]["type"] != "page" || item["meta"]["state"] != "active" {
        return Err(format!("Publication page {id} must be an active page."));
    }
    title(&item)?;
    Ok(item)
}

fn read_linked_page(root: &Path, id: &str, policy: &OutsideLinks) -> Result<Option<Value>, String> {
    let item = read_project_item(root, id)?;
    let active = item
        .as_ref()
        .is_some_and(|item| item["meta"]["type"] == "page" && item["meta"]["state"] == "active");
    if !active {
        return if matches!(policy, OutsideLinks::TextOnly) {
            Ok(None)
        } else if item.is_none() {
            Err(format!("Publication page {id} is missing."))
        } else {
            Err(format!("Publication page {id} must be an active page."))
        };
    }
    if let Some(item) = &item {
        title(item)?;
    }
    Ok(item)
}

fn planned_page(
    item: &Value,
    depth: usize,
    parent_id: Option<String>,
    content: &PublicationContent,
) -> Result<PlannedPage, String> {
    let document = item["content"]
        .get("content")
        .filter(|value| value.is_object())
        .unwrap_or_else(|| &item["content"]);
    Ok(PlannedPage {
        id: item["id"].as_str().ok_or("Page has no ID")?.to_owned(),
        revision: revision(item)?.to_owned(),
        title: title(item)?.to_owned(),
        route: if content.targets.site.layout
            == crate::engine::publication::SiteLayout::Documentation
        {
            format!(
                "pages/{}.html",
                item["id"].as_str().ok_or("Page has no ID")?
            )
        } else {
            format!(
                "pages/{}/index.html",
                item["meta"]["web_slug"]
                    .as_str()
                    .unwrap_or(item["id"].as_str().ok_or("Page has no ID")?)
            )
        },
        published_on: item["meta"]["published_on"].as_str().map(str::to_owned),
        tags: item["tags"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect(),
        depth,
        excluded_block_ids: excluded_block_ids(document, &content.selection_rules)?,
        parent_id,
    })
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use serde_json::{Value, json};

    use super::{
        PublicationFormat, PublicationSourceKind, assert_plan_fresh, plan_linked_pages,
        plan_publication, require_accessible_source,
    };
    use crate::engine::project::asset::write_asset;
    use crate::engine::project::config::create_project_config;
    use crate::engine::project::item::read_project_item;
    use crate::engine::project::properties::create_property;
    use crate::engine::project::write::write_item_input;
    use crate::engine::publication::{OutsideLinks, PdfProfile};

    #[test]
    #[expect(
        clippy::too_many_lines,
        reason = "Keep each end-to-end scenario together so setup, failure checks and cleanup remain visible."
    )]
    fn linked_pages_follow_transitive_links_once_and_reject_missing_targets() {
        let root =
            std::env::temp_dir().join(format!("bluink-linked-plan-{}", uuid::Uuid::now_v7()));
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("test-fixtures/pdf-project");
        let config: Value =
            serde_json::from_slice(&std::fs::read(fixture.join("bluink.json")).unwrap()).unwrap();
        create_project_config(&root, config).unwrap();
        std::fs::create_dir_all(root.join("properties")).unwrap();
        let ids: Vec<_> = (0..3).map(|_| uuid::Uuid::now_v7().to_string()).collect();
        for (index, target) in [1, 2, 0].into_iter().enumerate() {
            write_item_input(
                &root,
                json!({
                    "id": ids[index],
                    "meta": {"type":"page","title": format!("Page {index}")},
                    "content": {"type":"doc","content":[{"type":"paragraph","content":[{
                        "type":"text","text":format!("Page {target}"),
                        "marks":[{"type":"link","attrs":{"href":format!("/?note={}", ids[target])}}]
                    }]}]}
                }),
                None,
                "2026-09-28",
            )
            .unwrap();
        }
        let plan = plan_linked_pages(
            &root,
            &ids[0],
            PublicationFormat::Site,
            "classic",
            OutsideLinks::Error,
            PdfProfile::Default,
        )
        .unwrap();
        assert!(matches!(
            plan.source_kind,
            PublicationSourceKind::LinkedPages
        ));
        assert_eq!(
            plan.pages.iter().map(|page| &page.id).collect::<Vec<_>>(),
            ids.iter().collect::<Vec<_>>()
        );
        assert_eq!(plan.links.len(), 3);
        let pdf = plan_linked_pages(
            &root,
            &ids[0],
            PublicationFormat::Pdf,
            "classic",
            OutsideLinks::Error,
            PdfProfile::Default,
        )
        .unwrap();
        assert_eq!(pdf.pages.len(), 3);
        let missing = uuid::Uuid::now_v7().to_string();
        let current = read_project_item(&root, &ids[2]).unwrap().unwrap();
        write_item_input(&root, json!({
            "id": ids[2], "meta": {"type":"page","title":"Page 2"},
            "content": {"type":"doc","content":[{"type":"paragraph","content":[{
                "type":"text","text":"Missing", "marks":[{"type":"link","attrs":{"href":format!("/?note={missing}")}}]
            }]}]}
        }), current["revision"].as_str(), "2026-09-28").unwrap();
        let error = plan_linked_pages(
            &root,
            &ids[0],
            PublicationFormat::Site,
            "classic",
            OutsideLinks::Error,
            PdfProfile::Default,
        )
        .unwrap_err();
        assert!(
            error.contains("missing") || error.contains("does not exist"),
            "{error}"
        );
        for format in [PublicationFormat::Site, PublicationFormat::Pdf] {
            let plan = plan_linked_pages(
                &root,
                &ids[0],
                format,
                "classic",
                OutsideLinks::TextOnly,
                PdfProfile::Default,
            )
            .unwrap();
            assert_eq!(plan.pages.len(), 3);
            assert!(matches!(
                plan.targets.pdf.outside_links,
                OutsideLinks::TextOnly
            ));
            assert!(matches!(
                plan.targets.site.outside_links,
                OutsideLinks::TextOnly
            ));
            assert_eq!(
                plan.links
                    .iter()
                    .filter(|link| matches!(
                        link.resolution,
                        crate::engine::publication::preflight::LinkResolution::Included
                    ))
                    .count(),
                2
            );
            let omitted = plan
                .links
                .iter()
                .find(|link| link.target_id.as_deref() == Some(&missing))
                .unwrap();
            assert!(matches!(
                omitted.resolution,
                crate::engine::publication::preflight::LinkResolution::TextOnly
            ));
            assert_eq!(omitted.destination, None);
            assert_plan_fresh(&root, &plan).unwrap();
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn linked_discovery_keeps_breadth_first_order_and_checks_content_before_later_pages() {
        let root =
            std::env::temp_dir().join(format!("bluink-linked-batch-{}", uuid::Uuid::now_v7()));
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("test-fixtures/pdf-project");
        let config =
            serde_json::from_slice(&std::fs::read(fixture.join("bluink.json")).unwrap()).unwrap();
        create_project_config(&root, config).unwrap();
        std::fs::create_dir_all(root.join("properties")).unwrap();
        let ids: Vec<_> = (0..12).map(|_| uuid::Uuid::now_v7().to_string()).collect();
        for (index, id) in ids.iter().enumerate() {
            let targets: Vec<_> = if index == 0 {
                (1..7).collect()
            } else if index < 6 {
                vec![index + 6, 0]
            } else {
                vec![0]
            };
            let children: Vec<_> = targets
                .iter()
                .map(|target| {
                    json!({
                        "type":"text", "text":format!("Page {target}"),
                        "marks":[{"type":"link","attrs":{"href":format!("/?note={}",ids[*target])}}]
                    })
                })
                .collect();
            write_item_input(
                &root,
                json!({
                    "id":id,"meta":{"type":"page","title":format!("Page {index}")},
                    "content":{"type":"doc","content":[{"type":"paragraph","content":children}]}
                }),
                None,
                "2026-09-30",
            )
            .unwrap();
        }
        for format in [PublicationFormat::Pdf, PublicationFormat::Site] {
            for _ in 0..2 {
                let plan = plan_linked_pages(
                    &root,
                    &ids[0],
                    format,
                    "classic",
                    OutsideLinks::Error,
                    PdfProfile::Default,
                )
                .unwrap();
                assert_eq!(
                    plan.pages.iter().map(|page| &page.id).collect::<Vec<_>>(),
                    ids.iter().collect::<Vec<_>>()
                );
                assert_eq!(plan.structure.len(), ids.len() - 1);
                assert!(plan.links.iter().all(|link| matches!(
                    link.resolution,
                    crate::engine::publication::preflight::LinkResolution::Included
                )));
                assert_plan_fresh(&root, &plan).unwrap();
            }
        }
        // Simulate an external file edit bypassing save-time syntax checks.
        let mut first = read_project_item(&root, &ids[1]).unwrap().unwrap();
        first["meta"]["title"] = json!("Invalid destination");
        first["content"]["content"][0]["content"][0]["marks"][0]["attrs"]["href"] =
            json!("https://doi:10.21926/example");
        std::fs::write(
            root.join("items")
                .join(&ids[1][..2])
                .join(&ids[1])
                .join("item.json"),
            serde_json::to_vec(&first).unwrap(),
        )
        .unwrap();
        // The later page is unreadable. The first content error must win,
        // proving we did not finish graph discovery before validating it.
        std::fs::remove_file(
            root.join("items")
                .join(&ids[6][..2])
                .join(&ids[6])
                .join("item.json"),
        )
        .unwrap();
        for format in [PublicationFormat::Pdf, PublicationFormat::Site] {
            let error = plan_linked_pages(
                &root,
                &ids[0],
                format,
                "classic",
                OutsideLinks::TextOnly,
                PdfProfile::Default,
            )
            .unwrap_err();
            assert!(
                error.contains("Invalid destination") && error.contains("link cannot be exported"),
                "{error}"
            );
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn accessible_source_requires_language_and_table_headers() {
        let page = json!({"id":"01a0e2a0-0000-7000-8000-000000000001","meta":{"lang":"en-GB"}});
        let table = json!({"type":"doc","content":[{"type":"table","content":[
            {"type":"table_row","content":[{"type":"table_cell","content":[{"type":"paragraph","content":[{"type":"text","text":"Value"}]}]}]}
        ]}]});
        assert!(
            require_accessible_source(&page, &table, &[])
                .unwrap_err()
                .contains("header")
        );
        let mut header = table;
        header["content"][0]["content"][0]["content"][0]["type"] = json!("table_header");
        require_accessible_source(&page, &header, &[]).unwrap();
        let missing = json!({"id":page["id"],"meta":{}});
        assert!(
            require_accessible_source(&missing, &header, &[])
                .unwrap_err()
                .contains("language")
        );
    }

    #[test]
    #[expect(
        clippy::too_many_lines,
        reason = "Keep each end-to-end scenario together so setup, failure checks and cleanup remain visible."
    )]
    fn captures_authored_order_and_rejects_stale_or_missing_pages() {
        let root =
            std::env::temp_dir().join(format!("bluink-publication-plan-{}", uuid::Uuid::now_v7()));
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("test-fixtures/pdf-project");
        let config: Value =
            serde_json::from_slice(&std::fs::read(fixture.join("bluink.json")).unwrap()).unwrap();
        create_project_config(&root, config).unwrap();
        std::fs::create_dir_all(root.join("properties")).unwrap();
        let ids: Vec<_> = (0..4).map(|_| uuid::Uuid::now_v7().to_string()).collect();
        let omitted_block = uuid::Uuid::now_v7().to_string();
        let mut settings: Value = serde_json::from_str(include_str!(
            "../../../../vendor/themes/classic/manifest.json"
        ))
        .unwrap();
        settings = settings["settings"].clone();
        settings["typography"]["bodyScale"] = json!(1.2);
        for (index, id) in ids.iter().take(3).enumerate() {
            let paragraph = if index == 1 {
                json!({"type":"paragraph","attrs":{"id":omitted_block,"labels":["draft"]}})
            } else {
                json!({"type":"paragraph"})
            };
            write_item_input(
                &root,
                json!({"id": id, "meta":{"type":"page","title":format!("Page {index}")},
                "content":{"type":"doc","content":[paragraph]}}),
                None,
                "2026-09-28",
            )
            .unwrap();
        }
        let publication = json!({
            "id": ids[3], "meta": {"type":"publication","title":"Astronomy"},
            "content": {"kind":"publication","preset":"book","selectedOutput":"pdf","rootPageId":ids[0],
                "outline":[{"kind":"page","pageId":ids[2],"children":[{"kind":"page","pageId":ids[1],"children":[]}]}],
                "selectionRules":[{"id":uuid::Uuid::now_v7().to_string(),"effect":"exclude",
                    "target":"block","matcher":{"kind":"block_label","value":"draft"}}],
                "themeId":"classic", "themeSettings": settings,
                "targets":{"pdf":{"profile":"default","outsideLinks":"error"},
                    "html":{"outsideLinks":"error"},
                    "site":{"layout":"documentation","outsideLinks":"error"}}}
        });
        write_item_input(&root, publication, None, "2026-09-28").unwrap();
        let plan = plan_publication(&root, &ids[3], PublicationFormat::Pdf).unwrap();
        assert_eq!(plan.theme.settings.typography.body_scale, 1.2);
        assert_eq!(
            plan_publication(&root, &ids[3], PublicationFormat::Site)
                .unwrap()
                .theme
                .settings
                .typography
                .body_scale,
            1.2
        );
        let strict_id = uuid::Uuid::now_v7().to_string();
        write_item_input(
            &root,
            json!({"id":strict_id,"meta":{"type":"publication","title":"Strict book"},
                "content":{"kind":"publication","preset":"book","selectedOutput":"pdf","rootPageId":ids[0],
                "outline":[{"kind":"page","pageId":ids[1],"children":[]}],"selectionRules":[],
                "themeId":"classic","themeSettings":plan.theme.settings,
                "targets":{"pdf":{"profile":"accessible","outsideLinks":"error"},
                    "html":{"outsideLinks":"error"},
                    "site":{"layout":"documentation","outsideLinks":"error"}}}}),
            None,
            "2026-09-28",
        )
        .unwrap();
        assert!(
            plan_publication(&root, &strict_id, PublicationFormat::Pdf)
                .unwrap_err()
                .contains("unmodified reviewed built-in")
        );
        assert_eq!(
            plan.pages
                .iter()
                .map(|page| page.id.as_str())
                .collect::<Vec<_>>(),
            vec![ids[0].as_str(), ids[2].as_str(), ids[1].as_str()]
        );
        assert_eq!(
            plan.pages.iter().map(|page| page.depth).collect::<Vec<_>>(),
            vec![0, 1, 2]
        );
        assert_eq!(plan.pages[2].excluded_block_ids, [omitted_block]);
        let prior = plan.pages[1].revision.clone();
        write_item_input(
            &root,
            json!({"id":ids[2],"meta":{"title":"Changed"}}),
            Some(&prior),
            "2026-09-28",
        )
        .unwrap();
        assert!(
            assert_plan_fresh(&root, &plan)
                .unwrap_err()
                .contains("changed")
        );
        let root_revision = plan.pages[0].revision.as_str();
        let asset_url = format!("/assets/{}/cover.png", ids[0]);
        let image = |alt: &str| {
            json!({"type":"doc","content":[{"type":"image",
            "attrs":{"src":asset_url,"alt":alt}}]})
        };
        let bad_alt = write_item_input(
            &root,
            json!({"id":ids[0],"meta":{},"content":image("")}),
            Some(root_revision),
            "2026-09-28",
        )
        .unwrap();
        assert!(
            plan_publication(&root, &ids[3], PublicationFormat::Pdf)
                .unwrap_err()
                .contains("alt text")
        );
        let valid_image = write_item_input(
            &root,
            json!({"id":ids[0],"meta":{},"content":image("Cover")}),
            bad_alt["item"]["revision"].as_str(),
            "2026-09-28",
        )
        .unwrap();
        let missing_asset = plan_publication(&root, &ids[3], PublicationFormat::Pdf).unwrap_err();
        assert!(
            missing_asset.to_lowercase().contains("missing"),
            "{missing_asset}"
        );
        write_asset(&root, &ids[0], "cover.png", b"first version").unwrap();
        let asset_plan = plan_publication(&root, &ids[3], PublicationFormat::Pdf).unwrap();
        assert_eq!(asset_plan.assets.len(), 1);
        assert_eq!(asset_plan.assets[0].bytes, 13);
        assert_eq!(
            valid_image["item"]["revision"],
            asset_plan.pages[0].revision
        );
        write_asset(&root, &ids[0], "cover.png", b"changed asset").unwrap();
        assert!(
            assert_plan_fresh(&root, &asset_plan)
                .unwrap_err()
                .contains("changed")
        );
        let excluded_paragraph = json!({"type":"doc","content":[{"type":"paragraph","attrs":{"labels":["draft"]},
            "content":[{"type":"field","attrs":{"key":"missing"}}]}]});
        write_item_input(
            &root,
            json!({"id":ids[0],"meta":{},"content":excluded_paragraph}),
            Some(&asset_plan.pages[0].revision),
            "2026-09-28",
        )
        .unwrap();
        let filtered = plan_publication(&root, &ids[3], PublicationFormat::Pdf).unwrap();
        assert!(filtered.assets.is_empty());
        assert_eq!(filtered.pages[0].excluded_block_ids.len(), 1);
        let unresolved_field = json!({"type":"doc","content":[{"type":"paragraph","content":[
            {"type":"field","attrs":{"key":"missing"}}
        ]}]});
        let field_page = write_item_input(
            &root,
            json!({"id":ids[0],"meta":{},"content":unresolved_field}),
            Some(&filtered.pages[0].revision),
            "2026-09-28",
        )
        .unwrap();
        assert!(
            plan_publication(&root, &ids[3], PublicationFormat::Pdf)
                .unwrap_err()
                .contains("no scalar value")
        );
        let invalid_footnote = json!({"type":"doc","content":[{"type":"paragraph","content":[
            {"type":"footnote","attrs":{"paragraph":{"type":"paragraph","content":[
                {"type":"image","attrs":{"src":format!("/assets/{}/absent.png",ids[0]),"alt":"Missing image"}}
            ]}}}
        ]}]});
        let invalid_reference = write_item_input(
            &root,
            json!({"id":ids[0],"meta":{},"content":invalid_footnote}),
            field_page["item"]["revision"].as_str(),
            "2026-09-28",
        )
        .unwrap_err();
        assert!(invalid_reference.contains("text"), "{invalid_reference}");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn resolves_outside_links_only_under_the_selected_format_policy() {
        let root =
            std::env::temp_dir().join(format!("bluink-publication-links-{}", uuid::Uuid::now_v7()));
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("test-fixtures/pdf-project");
        let config: Value =
            serde_json::from_slice(&std::fs::read(fixture.join("bluink.json")).unwrap()).unwrap();
        create_project_config(&root, config).unwrap();
        std::fs::create_dir_all(root.join("properties")).unwrap();
        create_property(&root, json!({"key":"source_url","valueType":"text"})).unwrap();
        let ids: Vec<_> = (0..4).map(|_| uuid::Uuid::now_v7().to_string()).collect();
        let root_doc = json!({"type":"doc","content":[{"type":"paragraph","content":[
            {"type":"text","text":"Related article","marks":[{"type":"link",
                "attrs":{"href":format!("/?note={}",ids[2])}}]}
        ]}]});
        write_item_input(
            &root,
            json!({"id":ids[0],"meta":{"type":"page","title":"Start"},
            "content":root_doc}),
            None,
            "2026-09-28",
        )
        .unwrap();
        write_item_input(
            &root,
            json!({"id":ids[1],"meta":{"type":"page","title":"Chapter"},
            "content":{"type":"doc","content":[{"type":"paragraph"}]}}),
            None,
            "2026-09-28",
        )
        .unwrap();
        let outside = write_item_input(
            &root,
            json!({"id":ids[2],"meta":{"type":"page","title":"Outside",
            "source_url":"https://example.org/article"},
            "content":{"type":"doc","content":[{"type":"paragraph"}]}}),
            None,
            "2026-09-28",
        )
        .unwrap();
        let mut content = json!({"kind":"publication","preset":"book","selectedOutput":"pdf","rootPageId":ids[0],
            "outline":[{"kind":"page","pageId":ids[1],"children":[]}],"selectionRules":[],"themeId":"classic",
            "targets":{"pdf":{"profile":"default","outsideLinks":"error"},
                "html":{"outsideLinks":"error"},
                "site":{"layout":"documentation","outsideLinks":"text-only"}}});
        let publication = write_item_input(
            &root,
            json!({"id":ids[3],"meta":{"type":"publication","title":"Book"},
            "content":content}),
            None,
            "2026-09-28",
        )
        .unwrap();
        assert!(
            plan_publication(&root, &ids[3], PublicationFormat::Pdf)
                .unwrap_err()
                .contains("outside")
        );
        let site_plan = plan_publication(&root, &ids[3], PublicationFormat::Site).unwrap();
        assert_eq!(site_plan.links.len(), 1);
        assert_eq!(site_plan.links[0].destination.as_deref(), None);
        content["targets"]["pdf"]["outsideLinks"] = json!("text-only");
        write_item_input(
            &root,
            json!({"id":ids[3],"meta":{},"content":content}),
            publication["item"]["revision"].as_str(),
            "2026-09-28",
        )
        .unwrap();
        let pdf_plan = plan_publication(&root, &ids[3], PublicationFormat::Pdf).unwrap();
        write_item_input(
            &root,
            json!({"id":ids[2],"meta":{"source_url":"https://example.org/new"}}),
            outside["item"]["revision"].as_str(),
            "2026-09-28",
        )
        .unwrap();
        assert!(assert_plan_fresh(&root, &pdf_plan).is_ok());
        let missing_id = uuid::Uuid::now_v7().to_string();
        let broken_link = json!({"type":"doc","content":[{"type":"paragraph","content":[
            {"type":"text","text":"Missing page","marks":[{"type":"link",
                "attrs":{"href":format!("/?note={missing_id}")}}]}
        ]}]});
        write_item_input(
            &root,
            json!({"id":ids[0],"meta":{},"content":broken_link}),
            Some(&pdf_plan.pages[0].revision),
            "2026-09-28",
        )
        .unwrap();
        assert!(plan_publication(&root, &ids[3], PublicationFormat::Pdf).is_ok());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn captures_named_snippets_and_rejects_changed_or_missing_dependencies() {
        let root =
            std::env::temp_dir().join(format!("bluink-snippet-plan-{}", uuid::Uuid::now_v7()));
        crate::engine::project::config::open_or_create_project(&root, "UTC").unwrap();
        let paragraph =
            json!({"type":"paragraph","content":[{"type":"text","text":"Quoted source"}]});
        let snippet = crate::engine::project::snippets::write_snippet(
            &root,
            "shared",
            paragraph.clone(),
            None,
        )
        .unwrap();
        let id = uuid::Uuid::now_v7().to_string();
        write_item_input(
            &root,
            json!({"id":id,"meta":{"type":"page","title":"Owner"},
            "content":{"type":"doc","content":[{"type":"paragraph","content":[
                {"type":"citation","attrs":{"source":"snippet","snippetId":"shared"}},
                {"type":"citation","attrs":{"source":"snippet","snippetId":"shared"}}
            ]},{"type":"embed","attrs":{"snippetId":"shared"}}]}}),
            None,
            "2026-10-01",
        )
        .unwrap();
        let plan = plan_linked_pages(
            &root,
            &id,
            PublicationFormat::Html,
            "classic",
            OutsideLinks::Error,
            PdfProfile::Default,
        )
        .unwrap();
        assert_eq!(plan.pages.len(), 1);
        assert_eq!(plan.snippets.len(), 1);
        assert_eq!(plan.snippets[0].id, "shared");
        assert_eq!(
            plan.snippets[0].revision,
            snippet["revision"].as_str().unwrap()
        );
        assert_eq!(plan.snippets[0].paragraph, paragraph);
        for _ in 0..2 {
            write_item_input(&root, json!({"meta":{"type":"page","title":"Classification only"},
                "content":{"type":"doc","content":[{"type":"paragraph","attrs":{"labels":["shared"]}}]}}), None, "2026-10-01").unwrap();
        }
        assert_plan_fresh(&root, &plan).unwrap();
        crate::engine::project::snippets::write_snippet(
            &root,
            "shared",
            json!({"type":"paragraph","content":[{"type":"text","text":"Changed source"}]}),
            snippet["revision"].as_str(),
        )
        .unwrap();
        assert!(
            assert_plan_fresh(&root, &plan)
                .unwrap_err()
                .contains("changed")
        );
        let refreshed = plan_linked_pages(
            &root,
            &id,
            PublicationFormat::Html,
            "classic",
            OutsideLinks::Error,
            PdfProfile::Default,
        )
        .unwrap();
        std::fs::remove_file(root.join("snippets/snippet_shared.json")).unwrap();
        assert!(
            assert_plan_fresh(&root, &refreshed)
                .unwrap_err()
                .contains("disappeared")
        );
        assert!(
            plan_linked_pages(
                &root,
                &id,
                PublicationFormat::Html,
                "classic",
                OutsideLinks::Error,
                PdfProfile::Default
            )
            .unwrap_err()
            .contains("unresolved")
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
