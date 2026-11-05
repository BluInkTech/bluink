// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use crate::domain::publication::{
    LinkDepth, OutlineNode, OutsideLinks, PdfProfile, PublicationContent,
    plan::{listings::*, *},
    preflight::outside_policy,
    source::document,
    theme::{ThemeOrigin, apply_theme_options, apply_theme_overrides},
    validate_publication,
};
use crate::export_jobs::{
    preflight::{
        PagePreflight, assert_dependencies_fresh, preflight_publication, prepare_page_assets,
    },
    theme::{assert_theme_fresh, capture_theme},
};
use crate::store::snapshots::read_project_item;
use rayon::prelude::*;
use serde_json::Value;
use std::{collections::HashMap, path::Path};

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
    let item = read_project_item(root, publication_id)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("Publication {publication_id} does not exist."))?;
    plan_publication_item(
        root,
        &item,
        format,
        resolved_collections,
        PublicationSourceKind::Publication,
    )
}

/// The PDF a website offers of its whole Publication. It follows the site's
/// outside-link policy, so the site's checks hold for its PDF too.
pub(crate) fn plan_site_pdf(
    root: &Path,
    publication_id: &str,
    resolved_collections: &HashMap<String, Vec<String>>,
) -> Result<PublicationPlan, String> {
    let mut item = read_project_item(root, publication_id)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("Publication {publication_id} does not exist."))?;
    let targets = &mut item["content"]["targets"];
    targets["pdf"] = serde_json::json!({
        "profile": "default",
        "outsideLinks": targets["site"]["outsideLinks"].clone(),
    });
    plan_publication_item(
        root,
        &item,
        PublicationFormat::Pdf,
        resolved_collections,
        PublicationSourceKind::Publication,
    )
}

#[tracing::instrument(level = "debug", skip_all, fields(format = ?format, outside_links = ?outside_links, profile = ?profile))]
#[expect(
    clippy::needless_pass_by_value,
    reason = "The operation consumes its input snapshot rather than exposing a borrowed transaction API."
)]
#[expect(
    clippy::too_many_arguments,
    reason = "Linked-page planning takes the export settings, its depth and a progress callback."
)]
pub(crate) fn plan_linked_pages(
    root: &Path,
    page_id: &str,
    format: PublicationFormat,
    theme_id: &str,
    outside_links: OutsideLinks,
    profile: PdfProfile,
    depth: LinkDepth,
    // Told the pages checked and discovered after each page; false stops planning.
    progress: &mut dyn FnMut(usize, usize) -> bool,
) -> Result<PublicationPlan, String> {
    let root_page = read_active_page(root, page_id)?;
    let content = validate_publication(&serde_json::json!({
        "kind": "publication",

        "selectedOutput": "site",
        "rootPageId": page_id,
        "outline": [],
        "selectionRules": [],
        "themeId": theme_id,
        "targets": {
            "pdf": {"profile": profile, "outsideLinks": outside_links},
            "html": {"outsideLinks": outside_links},"docx": {"outsideLinks": outside_links},"epub":{"outsideLinks": outside_links},
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
        targets: content.targets,
        unresolved: true,
        issues: Vec::new(),
        warnings: Vec::new(),
        decisions_checked: false,
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
    let stopped = || "Export cancelled.".to_owned();
    // Membership and order come from the index (or, without one, the files).
    // While pages are found, none is checked yet.
    let walked = crate::store::reads::document_graph::linked_pages(
        root,
        page_id,
        depth,
        &mut |_, found| progress(0, found),
    )?;
    let total = walked.pages.len();
    crate::domain::publication::preflight::check_document_size(
        format,
        walked.pages.iter().map(|page| page.facts.words).sum(),
        "Follow fewer links, or export a website instead.",
    )?;
    plan.issues = check_linked_facts(root, &walked, &outside_links)?;
    if format == PublicationFormat::Pdf && matches!(profile, PdfProfile::Accessible) {
        // The accessible profile checks languages and table headers in each document.
        crate::export_jobs::parallel::exports(|| {
            walked.pages.par_iter().try_for_each(|page| {
                let item = read_active_page(root, &page.id)?;
                require_accessible_source(&item, document(&item), &[])
            })
        })?;
    }
    drop(root_page);
    for page in walked.pages {
        let is_root = page.id == page_id;
        if !is_root {
            plan.structure.push(PlannedStructureNode::Page {
                page_id: page.id.clone(),
                show_in_navigation: true,
                slug: None,
                children: Vec::new(),
            });
        }
        plan.pages.push(PlannedPage {
            route: format!("pages/{}.html", page.id),
            id: page.id,
            revision: page.revision,
            title: page.title,
            published_on: page.published_on,
            tags: page.tags,
            depth: usize::from(!is_root),
            excluded_block_ids: Vec::new(),
            parent_id: (!is_root).then(|| page_id.to_owned()),
        });
    }
    if !progress(total, total) {
        return Err(stopped());
    }
    assign_routes(&mut plan)?;
    // Links, images and snippets are resolved when generation reads the pages.
    assert_plan_fresh(root, &plan)?;
    Ok(plan)
}

/// Checks a linked-pages export from each page's stored facts: problems a
/// page has on its own, heading IDs shared across pages, section links,
/// outside links under the strict policy, and the snippets pages use. Every
/// problem is listed, so the writer can fix them together.
fn check_linked_facts(
    root: &Path,
    walked: &crate::store::reads::document_graph::LinkedPages,
    outside_links: &OutsideLinks,
) -> Result<Vec<PlanIssue>, String> {
    let members: std::collections::HashSet<&str> =
        walked.pages.iter().map(|page| page.id.as_str()).collect();
    let unavailable: std::collections::HashSet<&str> =
        walked.unavailable.iter().map(String::as_str).collect();
    let mut issues = Vec::new();
    let mut anchors = HashMap::new();
    for page in &walked.pages {
        if page.title.trim().is_empty() {
            return Err(format!(
                "Item \"{}\" needs a title for publication.",
                page.id
            ));
        }
        let mut issue = |block_id: Option<&str>, message: String| {
            issues.push(PlanIssue {
                page_id: page.id.clone(),
                title: page.title.clone(),
                block_id: block_id.map(str::to_owned),
                message,
            });
        };
        for found in &page.facts.issues {
            issue(found.block_id.as_deref(), found.message.clone());
        }
        anchors.insert(format!("note-{}", page.id), page.id.clone());
        for heading in &page.facts.headings {
            if anchors
                .insert(format!("block-{heading}"), page.id.clone())
                .is_some_and(|owner| owner != page.id)
            {
                issue(
                    Some(heading),
                    format!("heading block ID {heading} also belongs to another page."),
                );
            }
        }
    }
    let mut snippets = HashMap::new();
    for page in &walked.pages {
        let mut issue = |block_id: Option<&str>, message: String| {
            issues.push(PlanIssue {
                page_id: page.id.clone(),
                title: page.title.clone(),
                block_id: block_id.map(str::to_owned),
                message,
            });
        };
        if matches!(outside_links, OutsideLinks::Error) {
            for id in page
                .links
                .iter()
                .filter(|id| !members.contains(id.as_str()))
            {
                issue(
                    None,
                    if unavailable.contains(id.as_str()) {
                        format!("linked page {id} is missing or is not an active page.")
                    } else {
                        format!("linked page {id} is outside this Publication.")
                    },
                );
            }
        }
        for link in &page.facts.anchor_links {
            let anchor = link.target.trim_start_matches('#');
            if !anchors.contains_key(anchor) {
                issue(
                    link.block_id.as_deref(),
                    format!("linked section #{anchor} is absent or excluded."),
                );
            }
        }
        for reference in &page.facts.snippets {
            let state = if let Some(state) = snippets.get(reference.target.as_str()) {
                *state
            } else {
                let record = crate::store::snapshots::read_snippet(root, &reference.target)?;
                let state = match record {
                    None => Some("is unresolved"),
                    Some(record) if record["state"] != "active" => Some("is archived"),
                    Some(_) => None,
                };
                snippets.insert(reference.target.as_str(), state);
                state
            };
            if let Some(state) = state {
                issue(
                    reference.block_id.as_deref(),
                    format!("snippet {} {state}.", reference.target),
                );
            }
        }
    }
    Ok(issues)
}

/// Pages read together while generation resolves a linked-pages plan.
const COMPLETION_BATCH: usize = 256;

/// Resolves a linked-pages plan's links, images and snippets as generation
/// starts. Each page is read once, in batches, and must still have the
/// revision it was planned with; the same scan as a Publication applies.
pub(crate) fn complete_linked_plan(
    root: &Path,
    plan: &PublicationPlan,
) -> Result<PublicationPlan, String> {
    if let Some(first) = plan.issues.first() {
        return Err(format!(
            "Fix this export's {} issue(s) first. In “{}”: {}",
            plan.issues.len(),
            first.title,
            first.message
        ));
    }
    let mut plan = plan.clone();
    let policy = outside_policy(&plan.targets, plan.format).clone();
    let mut preflight = PagePreflight::default();
    for batch in plan.pages.chunks(COMPLETION_BATCH) {
        let read = crate::export_jobs::parallel::exports(|| {
            batch
                .par_iter()
                .map(|page| {
                    let item = read_project_item(root, &page.id)
                        .map_err(|error| error.to_string())?
                        .ok_or_else(|| format!("Publication page {} is missing.", page.id))?;
                    if item["revision"].as_str() != Some(&page.revision) {
                        return Err(format!(
                            "Page {} changed during Publication planning.",
                            page.id
                        ));
                    }
                    let assets = prepare_page_assets(root, &item);
                    Ok((item, assets))
                })
                .collect::<Result<Vec<_>, String>>()
        })?;
        for (page, (item, assets)) in batch.iter().zip(read) {
            preflight.prepare(assets);
            preflight.scan(root, page, &item)?;
        }
    }
    let checked = preflight.finish(root, &plan.pages, &policy)?;
    plan.assets = checked.assets;
    plan.links = checked.links;
    plan.snippets = checked.snippets;
    plan.unresolved = false;
    Ok(plan)
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
    if let Some(settings) = &content.theme_overrides {
        apply_theme_overrides(&mut theme, settings)?;
    }
    apply_theme_options(&mut theme, &content.theme_options)?;
    if matches!(format, PublicationFormat::Pdf)
        && matches!(content.targets.pdf.profile, PdfProfile::Accessible)
        && (!matches!(theme.origin, ThemeOrigin::Builtin) || content.theme_overrides.is_some())
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
        unresolved: false,
        issues: Vec::new(),
        warnings: Vec::new(),
        decisions_checked: false,
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

pub(crate) fn assert_plan_fresh(root: &Path, plan: &PublicationPlan) -> Result<(), String> {
    let store = crate::store::Store::shared(root).map_err(|error| error.to_string())?;
    store
        .read(|tx| {
            let mut check = tx.prepare_cached(
                "SELECT EXISTS(SELECT 1 FROM item WHERE uuid=?1 AND revision=?2)",
            )?;
            for (id, revision) in
                std::iter::once((&plan.publication_id, &plan.publication_revision))
                    .chain(plan.pages.iter().map(|page| (&page.id, &page.revision)))
            {
                if !check.query_row([id, revision], |row| row.get::<_, bool>(0))? {
                    return Err(crate::store::Error::Invalid(format!(
                        "Item {id} changed during Publication planning."
                    )));
                }
            }
            Ok(())
        })
        .map_err(|error| error.to_string())?;
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
    let item = read_project_item(root, id)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("Publication page {id} is missing."))?;
    if item["meta"]["type"] != "page" || item["meta"]["state"] != "active" {
        return Err(format!("Publication page {id} must be an active page."));
    }
    title(&item)?;
    Ok(item)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use serde_json::{Value, json};

    use super::{
        PublicationFormat, PublicationSourceKind, assert_plan_fresh, plan_linked_pages,
        plan_publication, require_accessible_source,
    };
    use crate::application::catalogue_writes::create_property;
    use crate::application::item_writes::write_asset;
    use crate::application::item_writes::write_item_input;
    use crate::domain::project::config::create_project_config;
    use crate::domain::publication::{LinkDepth, OutsideLinks, PdfProfile};
    use crate::store::snapshots::read_project_item;

    #[test]
    fn linked_plans_check_facts_trust_file_stamps_and_resolve_at_generation() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("project");
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("test-fixtures/pdf-project");
        let config: Value =
            serde_json::from_slice(&std::fs::read(fixture.join("bluink.json")).unwrap()).unwrap();
        create_project_config(&root, config).unwrap();
        crate::domain::project::properties::initialize(&root).unwrap();
        let ids: Vec<_> = (0..2).map(|_| uuid::Uuid::now_v7().to_string()).collect();
        let write = |index: usize, body: Value, revision: Option<&str>| {
            write_item_input(
                &root,
                json!({"id": ids[index], "meta": {"type":"page","title": format!("Page {index}")},
                    "content": {"document": {"type":"doc","content":[{"type":"paragraph","content":body}]}}}),
                revision,
                "2026-10-27",
            )
            .unwrap()
        };
        write(
            0,
            json!([{"type":"text","text":"Next","marks":[{"type":"link","attrs":{"href":format!("/?note={}", ids[1])}}]}]),
            None,
        );
        let second = write(1, json!([{"type":"text","text":"End"}]), None);
        let plan_now = || {
            plan_linked_pages(
                &root,
                &ids[0],
                PublicationFormat::Site,
                "classic",
                OutsideLinks::TextOnly,
                PdfProfile::Default,
                LinkDepth::Direct,
                &mut |_, _| true,
            )
        };
        let plan = plan_now().unwrap();
        assert!(plan.links.is_empty() && plan.assets.is_empty());

        assert_plan_fresh(&root, &plan).unwrap();

        // Generation resolves the links; an edit after planning is refused.
        let completed = super::complete_linked_plan(&root, &plan).unwrap();
        assert_eq!(completed.links.len(), 1);
        write(
            1,
            json!([{"type":"text","text":"Edited"}]),
            second["item"]["revision"].as_str(),
        );
        assert!(
            assert_plan_fresh(&root, &plan)
                .unwrap_err()
                .contains("changed")
        );
        assert!(
            super::complete_linked_plan(&root, &plan)
                .unwrap_err()
                .contains("changed")
        );

        // A page's own problem, recorded in its facts, is listed as an issue.
        let current = read_project_item(&root, &ids[1]).unwrap().unwrap();
        write(
            1,
            json!([{"type":"text","text":"Bad","marks":[{"type":"link","attrs":{"href":"not a link"}}]}]),
            current["revision"].as_str(),
        );
        let issues = plan_now().unwrap().issues;
        assert!(
            issues.len() == 1
                && issues[0].title == "Page 1"
                && issues[0].message.contains("link cannot be exported"),
            "{issues:?}"
        );
        temporary.close().unwrap();
    }

    #[test]
    fn linked_pages_follow_links_to_the_chosen_depth_and_stop_when_asked() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("project");
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("test-fixtures/pdf-project");
        let config: Value =
            serde_json::from_slice(&std::fs::read(fixture.join("bluink.json")).unwrap()).unwrap();
        create_project_config(&root, config).unwrap();
        crate::domain::project::properties::initialize(&root).unwrap();
        // A chain: each page links to the next, and the last links nowhere.
        let ids: Vec<_> = (0..4).map(|_| uuid::Uuid::now_v7().to_string()).collect();
        for (index, id) in ids.iter().enumerate() {
            let text = ids.get(index + 1).map_or_else(
                || json!({"type":"text","text":"End"}),
                |next| json!({"type":"text","text":"Next","marks":[{"type":"link","attrs":{"href":format!("/?note={next}")}}]}),
            );
            write_item_input(
                &root,
                json!({
                    "id": id,
                    "meta": {"type":"page","title": format!("Page {index}")},
                    "content": {"document": {"type":"doc","content":[{"type":"paragraph","content":[text]}]}}
                }),
                None,
                "2026-10-27",
            )
            .unwrap();
        }
        let pages = |depth: LinkDepth| {
            let mut reports = Vec::new();
            let plan = plan_linked_pages(
                &root,
                &ids[0],
                PublicationFormat::Site,
                "classic",
                OutsideLinks::TextOnly,
                PdfProfile::Default,
                depth,
                &mut |checked, discovered| {
                    reports.push((checked, discovered));
                    true
                },
            )
            .unwrap();
            (plan.pages.len(), reports)
        };
        let (direct, reports) = pages(LinkDepth::Direct);
        assert_eq!(direct, 2);
        // Pages are found first (one report a level), then checked from their facts.
        assert_eq!(reports, vec![(0, 2), (0, 2), (2, 2)]);
        assert_eq!(pages(LinkDepth::TwoLevels).0, 3);
        assert_eq!(pages(LinkDepth::Reachable).0, 4);
        let stopped = plan_linked_pages(
            &root,
            &ids[0],
            PublicationFormat::Site,
            "classic",
            OutsideLinks::TextOnly,
            PdfProfile::Default,
            LinkDepth::Reachable,
            &mut |checked, _| checked < 2,
        )
        .unwrap_err();
        assert_eq!(stopped, "Export cancelled.");
    }

    #[test]
    fn a_linked_export_too_large_for_one_file_is_refused_before_generation() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("project");
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("test-fixtures/pdf-project");
        let config: Value =
            serde_json::from_slice(&std::fs::read(fixture.join("bluink.json")).unwrap()).unwrap();
        create_project_config(&root, config).unwrap();
        crate::domain::project::properties::initialize(&root).unwrap();
        // Two pages of 1,000,000 words each: either fits, together they do not.
        let ids: Vec<_> = (0..2).map(|_| uuid::Uuid::now_v7().to_string()).collect();
        let words = "word ".repeat(1_000_000);
        for (index, id) in ids.iter().enumerate() {
            let mut content = vec![json!({"type":"text","text":words})];
            if let Some(next) = ids.get(index + 1) {
                content.push(json!({"type":"text","text":"Next","marks":[{"type":"link","attrs":{"href":format!("/?note={next}")}}]}));
            }
            write_item_input(
                &root,
                json!({
                    "id": id,
                    "meta": {"type":"page","title": format!("Long page {index}")},
                    "content": {"document": {"type":"doc","content":[{"type":"paragraph","content":content}]}}
                }),
                None,
                "2026-10-28",
            )
            .unwrap();
        }
        let plan = |format| {
            plan_linked_pages(
                &root,
                &ids[0],
                format,
                "classic",
                OutsideLinks::TextOnly,
                PdfProfile::Default,
                LinkDepth::Direct,
                &mut |_, _| true,
            )
        };
        assert_eq!(
            plan(PublicationFormat::Pdf).unwrap_err(),
            "This export has 2,000,001 words, more than one PDF can hold (2,000,000 words). \
             Follow fewer links, or export a website instead."
        );
        assert!(plan(PublicationFormat::Epub).is_err());
        // A website writes each page separately.
        assert_eq!(plan(PublicationFormat::Site).unwrap().pages.len(), 2);
    }

    #[test]
    #[expect(
        clippy::too_many_lines,
        reason = "Keep each end-to-end scenario together so setup, failure checks and cleanup remain visible."
    )]
    fn linked_pages_follow_transitive_links_once_and_reject_missing_targets() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("project");
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("test-fixtures/pdf-project");
        let config: Value =
            serde_json::from_slice(&std::fs::read(fixture.join("bluink.json")).unwrap()).unwrap();
        create_project_config(&root, config).unwrap();
        crate::domain::project::properties::initialize(&root).unwrap();
        let ids: Vec<_> = (0..3).map(|_| uuid::Uuid::now_v7().to_string()).collect();
        for (index, target) in [1, 2, 0].into_iter().enumerate() {
            write_item_input(
                &root,
                json!({
                    "id": ids[index],
                    "meta": {"type":"page","title": format!("Page {index}")},
                    "content": {"document": {"type":"doc","content":[{"type":"paragraph","content":[{
                        "type":"text","text":format!("Page {target}"),
                        "marks":[{"type":"link","attrs":{"href":format!("/?note={}", ids[target])}}]
                    }]}]}}
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
            crate::domain::publication::LinkDepth::Reachable,
            &mut |_, _| true,
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
        // Links resolve when generation completes the plan.
        assert_eq!(plan.links.len(), 0);
        assert_eq!(
            super::complete_linked_plan(&root, &plan)
                .unwrap()
                .links
                .len(),
            3
        );
        let pdf = plan_linked_pages(
            &root,
            &ids[0],
            PublicationFormat::Pdf,
            "classic",
            OutsideLinks::Error,
            PdfProfile::Default,
            crate::domain::publication::LinkDepth::Reachable,
            &mut |_, _| true,
        )
        .unwrap();
        assert_eq!(pdf.pages.len(), 3);
        let missing = uuid::Uuid::now_v7().to_string();
        let current = read_project_item(&root, &ids[2]).unwrap().unwrap();
        write_item_input(&root, json!({
            "id": ids[2], "meta": {"type":"page","title":"Page 2"},
            "content": {"document": {"type":"doc","content":[{"type":"paragraph","content":[{
                "type":"text","text":"Missing", "marks":[{"type":"link","attrs":{"href":format!("/?note={missing}")}}]
            }]}]}}
        }), current["revision"].as_str(), "2026-09-28").unwrap();
        let issues = plan_linked_pages(
            &root,
            &ids[0],
            PublicationFormat::Site,
            "classic",
            OutsideLinks::Error,
            PdfProfile::Default,
            crate::domain::publication::LinkDepth::Reachable,
            &mut |_, _| true,
        )
        .unwrap()
        .issues;
        assert!(
            issues.iter().any(|issue| issue.message.contains("missing")),
            "{issues:?}"
        );
        for format in [PublicationFormat::Site, PublicationFormat::Pdf] {
            let plan = plan_linked_pages(
                &root,
                &ids[0],
                format,
                "classic",
                OutsideLinks::TextOnly,
                PdfProfile::Default,
                crate::domain::publication::LinkDepth::Reachable,
                &mut |_, _| true,
            )
            .unwrap();
            let plan = super::complete_linked_plan(&root, &plan).unwrap();
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
                        crate::domain::publication::preflight::LinkResolution::Included
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
                crate::domain::publication::preflight::LinkResolution::TextOnly
            ));
            assert_eq!(omitted.destination, None);
            assert_plan_fresh(&root, &plan).unwrap();
        }
        temporary.close().unwrap();
    }

    #[test]
    fn linked_discovery_keeps_breadth_first_order_and_checks_content_before_later_pages() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("project");
        crate::domain::project::config::open_or_create_project(&root, "UTC").unwrap();
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
                    "content":{"document": {"type":"doc","content":[{"type":"paragraph","content":children}]}}
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
                    crate::domain::publication::LinkDepth::Reachable,
                    &mut |_, _| true,
                )
                .unwrap();
                assert_eq!(
                    plan.pages.iter().map(|page| &page.id).collect::<Vec<_>>(),
                    ids.iter().collect::<Vec<_>>()
                );
                assert_eq!(plan.structure.len(), ids.len() - 1);
                assert!(plan.links.iter().all(|link| matches!(
                    link.resolution,
                    crate::domain::publication::preflight::LinkResolution::Included
                )));
                assert_plan_fresh(&root, &plan).unwrap();
            }
        }
        // Unsupported links in imported content must be reported even when a later target is unavailable.
        let store = crate::store::Store::shared(&root).unwrap();
        let mut first = read_project_item(&root, &ids[1]).unwrap().unwrap();
        first["meta"]["title"] = json!("Invalid destination");
        first["content"]["document"]["content"][0]["content"][0]["marks"][0]["attrs"]["href"] =
            json!("https://doi:10.21926/example");
        let revision = first["revision"].as_str().unwrap().to_owned();
        first["revision"] = json!(uuid::Uuid::now_v7().to_string());
        let imported = crate::store::mutations::PreparedChange::new(first, Some(revision)).unwrap();
        store
            .write(move |tx| crate::store::mutations::apply(tx, &[imported], &[], None))
            .unwrap();
        crate::store::commands::metadata(
            &store,
            &ids[6],
            None,
            serde_json::Map::from_iter([("state".to_owned(), json!("archived"))]),
            false,
        )
        .unwrap();
        drop(store);
        for format in [PublicationFormat::Pdf, PublicationFormat::Site] {
            let issues = plan_linked_pages(
                &root,
                &ids[0],
                format,
                "classic",
                OutsideLinks::TextOnly,
                PdfProfile::Default,
                crate::domain::publication::LinkDepth::Reachable,
                &mut |_, _| true,
            )
            .unwrap()
            .issues;
            assert!(
                issues
                    .iter()
                    .any(|issue| issue.title == "Invalid destination"
                        && issue.message.contains("link cannot be exported")),
                "{issues:?}"
            );
        }
        temporary.close().unwrap();
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
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("project");
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("test-fixtures/pdf-project");
        let config: Value =
            serde_json::from_slice(&std::fs::read(fixture.join("bluink.json")).unwrap()).unwrap();
        create_project_config(&root, config).unwrap();
        crate::domain::project::properties::initialize(&root).unwrap();
        let ids: Vec<_> = (0..4).map(|_| uuid::Uuid::now_v7().to_string()).collect();
        let omitted_block = uuid::Uuid::now_v7().to_string();
        let settings = json!({"shared":{"root":{"fontSize":"14.4pt"}},"targets":{}});
        for (index, id) in ids.iter().take(3).enumerate() {
            let paragraph = if index == 1 {
                json!({"type":"paragraph","attrs":{"id":omitted_block,"labels":["draft"]}})
            } else {
                json!({"type":"paragraph"})
            };
            write_item_input(
                &root,
                json!({"id": id, "meta":{"type":"page","title":format!("Page {index}")},
                "content":{"document": {"type":"doc","content":[paragraph]}}}),
                None,
                "2026-09-28",
            )
            .unwrap();
        }
        let publication = json!({
            "id": ids[3], "meta": {"type":"publication","title":"Astronomy"},
            "content": {"kind":"publication","selectedOutput":"pdf","rootPageId":ids[0],
                "outline":[{"kind":"page","pageId":ids[2],"children":[{"kind":"page","pageId":ids[1],"children":[]}]}],
                "selectionRules":[{"id":uuid::Uuid::now_v7().to_string(),"effect":"exclude",
                    "target":"block","matcher":{"kind":"block_label","value":"draft"}}],
                "themeId":"classic", "themeOverrides": settings,
                "targets":{"pdf":{"profile":"default","outsideLinks":"error"},
                    "html":{"outsideLinks":"error"},"docx":{"outsideLinks":"error"},"epub":{"outsideLinks":"error"},
                    "site":{"layout":"documentation","outsideLinks":"error"}}}
        });
        write_item_input(&root, publication, None, "2026-09-28").unwrap();
        let plan = plan_publication(&root, &ids[3], PublicationFormat::Pdf).unwrap();
        assert_eq!(plan.theme.resolved.root_font_size_pt, 14.4);
        assert_eq!(
            plan_publication(&root, &ids[3], PublicationFormat::Site)
                .unwrap()
                .theme
                .resolved
                .root_font_size_pt,
            14.4
        );
        let strict_id = uuid::Uuid::now_v7().to_string();
        write_item_input(
            &root,
            json!({"id":strict_id,"meta":{"type":"publication","title":"Strict book"},
                "content":{"kind":"publication","selectedOutput":"pdf","rootPageId":ids[0],
                "outline":[{"kind":"page","pageId":ids[1],"children":[]}],"selectionRules":[],
                "themeId":"classic","themeOverrides":plan.theme.overrides,
                "targets":{"pdf":{"profile":"accessible","outsideLinks":"error"},
                    "html":{"outsideLinks":"error"},"docx":{"outsideLinks":"error"},"epub":{"outsideLinks":"error"},
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
            json!({"id":ids[0],"meta":{},"content":{"document": image("")}}),
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
            json!({"id":ids[0],"meta":{},"content":{"document": image("Cover")}}),
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
            json!({"id":ids[0],"meta":{},"content":{"document": excluded_paragraph}}),
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
            json!({"id":ids[0],"meta":{},"content":{"document": unresolved_field}}),
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
            json!({"id":ids[0],"meta":{},"content":{"document": invalid_footnote}}),
            field_page["item"]["revision"].as_str(),
            "2026-09-28",
        )
        .unwrap_err();
        assert!(invalid_reference.contains("text"), "{invalid_reference}");
        temporary.close().unwrap();
    }

    #[test]
    fn resolves_outside_links_only_under_the_selected_format_policy() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("project");
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("test-fixtures/pdf-project");
        let config: Value =
            serde_json::from_slice(&std::fs::read(fixture.join("bluink.json")).unwrap()).unwrap();
        create_project_config(&root, config).unwrap();
        crate::domain::project::properties::initialize(&root).unwrap();
        create_property(
            &crate::store::Store::shared(&root).unwrap(),
            &json!({"key":"source_url","valueType":"text"}),
        )
        .unwrap();
        let ids: Vec<_> = (0..4).map(|_| uuid::Uuid::now_v7().to_string()).collect();
        let root_doc = json!({"type":"doc","content":[{"type":"paragraph","content":[
            {"type":"text","text":"Related article","marks":[{"type":"link",
                "attrs":{"href":format!("/?note={}",ids[2])}}]}
        ]}]});
        write_item_input(
            &root,
            json!({"id":ids[0],"meta":{"type":"page","title":"Start"},
            "content":{"document": root_doc}}),
            None,
            "2026-09-28",
        )
        .unwrap();
        write_item_input(
            &root,
            json!({"id":ids[1],"meta":{"type":"page","title":"Chapter"},
            "content":{"document": {"type":"doc","content":[{"type":"paragraph"}]}}}),
            None,
            "2026-09-28",
        )
        .unwrap();
        let outside = write_item_input(
            &root,
            json!({"id":ids[2],"meta":{"type":"page","title":"Outside",
            "source_url":"https://example.org/article"},
            "content":{"document": {"type":"doc","content":[{"type":"paragraph"}]}}}),
            None,
            "2026-09-28",
        )
        .unwrap();
        let mut content = json!({"kind":"publication","selectedOutput":"pdf","rootPageId":ids[0],
            "outline":[{"kind":"page","pageId":ids[1],"children":[]}],"selectionRules":[],"themeId":"classic",
            "targets":{"pdf":{"profile":"default","outsideLinks":"error"},
                "html":{"outsideLinks":"error"},"docx":{"outsideLinks":"error"},"epub":{"outsideLinks":"error"},
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
            json!({"id":ids[0],"meta":{},"content":{"document": broken_link}}),
            Some(&pdf_plan.pages[0].revision),
            "2026-09-28",
        )
        .unwrap();
        assert!(plan_publication(&root, &ids[3], PublicationFormat::Pdf).is_ok());
        temporary.close().unwrap();
    }

    #[test]
    fn captures_named_snippets_and_rejects_changed_or_missing_dependencies() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("project");
        crate::domain::project::config::open_or_create_project(&root, "UTC").unwrap();
        let paragraph =
            json!({"type":"paragraph","content":[{"type":"text","text":"Quoted source"}]});
        let snippet =
            crate::application::snippets::write_snippet(&root, "shared", paragraph.clone(), None)
                .unwrap();
        let id = uuid::Uuid::now_v7().to_string();
        write_item_input(
            &root,
            json!({"id":id,"meta":{"type":"page","title":"Owner"},
            "content":{"document": {"type":"doc","content":[{"type":"paragraph","content":[
                {"type":"citation","attrs":{"source":"snippet","snippetId":"shared"}},
                {"type":"citation","attrs":{"source":"snippet","snippetId":"shared"}}
            ]},{"type":"embed","attrs":{"snippetId":"shared"}}]}}}),
            None,
            "2026-10-01",
        )
        .unwrap();
        let linked = || {
            plan_linked_pages(
                &root,
                &id,
                PublicationFormat::Html,
                "classic",
                OutsideLinks::Error,
                PdfProfile::Default,
                crate::domain::publication::LinkDepth::Reachable,
                &mut |_, _| true,
            )
            .unwrap()
        };
        let plan = linked();
        assert_eq!(plan.pages.len(), 1);
        let plan = super::complete_linked_plan(&root, &plan).unwrap();
        assert_eq!(plan.snippets.len(), 1);
        assert_eq!(plan.snippets[0].id, "shared");
        assert_eq!(
            plan.snippets[0].revision,
            snippet["revision"].as_str().unwrap()
        );
        assert_eq!(plan.snippets[0].paragraph, paragraph);
        for _ in 0..2 {
            write_item_input(&root, json!({"meta":{"type":"page","title":"Classification only"},
                "content":{"document": {"type":"doc","content":[{"type":"paragraph","attrs":{"labels":["shared"]}}]}}}), None, "2026-10-01").unwrap();
        }
        assert_plan_fresh(&root, &plan).unwrap();
        crate::application::snippets::write_snippet(
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
        let refreshed = linked();
        let refreshed = super::complete_linked_plan(&root, &refreshed).unwrap();
        let db = rusqlite::Connection::open(root.join(crate::store::DATABASE_NAME)).unwrap();
        db.execute(
            "DELETE FROM item WHERE uuid=?1",
            [crate::domain::project::snippets::item_id("shared").unwrap()],
        )
        .unwrap();
        drop(db);
        assert!(
            assert_plan_fresh(&root, &refreshed)
                .unwrap_err()
                .contains("disappeared")
        );
        let unresolved = linked();
        assert!(unresolved.issues[0].message.contains("unresolved"));
        // A plan with issues does not generate.
        assert!(
            super::complete_linked_plan(&root, &unresolved)
                .unwrap_err()
                .contains("Fix this export")
        );
        temporary.close().unwrap();
    }
}
