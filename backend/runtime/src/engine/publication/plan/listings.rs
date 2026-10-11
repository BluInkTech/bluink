// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use super::*;

pub(super) fn assign_routes(plan: &mut PublicationPlan) -> Result<(), String> {
    fn explicit(nodes: &[PlannedStructureNode], output: &mut HashMap<String, String>) {
        for node in nodes {
            match node {
                PlannedStructureNode::Page {
                    page_id,
                    slug,
                    children,
                    ..
                } => {
                    if let Some(slug) = slug {
                        output.insert(page_id.clone(), format!("{slug}/index.html"));
                    }
                    explicit(children, output);
                }
                PlannedStructureNode::Section { children, .. } => explicit(children, output),
                PlannedStructureNode::Collection { .. } => {}
            }
        }
    }
    let mut overrides = HashMap::new();
    explicit(&plan.structure, &mut overrides);
    let mut routes = std::collections::HashSet::new();
    for page in &mut plan.pages {
        if page.id == plan.root_page_id {
            "index.html".clone_into(&mut page.route);
        } else if let Some(route) = overrides.remove(&page.id) {
            page.route = route;
        }
        if !routes.insert(page.route.clone()) {
            return Err(format!(
                "Publication pages have a route collision at {}.",
                page.route
            ));
        }
    }
    Ok(())
}

#[expect(
    clippy::too_many_lines,
    reason = "Keep the complete format or operation dispatch together for contract review."
)]
pub(super) fn plan_listings(plan: &mut PublicationPlan) -> Result<(), String> {
    use std::collections::BTreeMap;
    fn add_pages(
        listings: &mut Vec<PlannedListing>,
        routes: &mut std::collections::HashSet<String>,
        collection_id: &str,
        base: &str,
        title: &str,
        ids: &[String],
        page_size: usize,
    ) -> Result<(), String> {
        let total_pages = ids.len().div_ceil(page_size).max(1);
        for index in 0..total_pages {
            let route = if index == 0 {
                format!("{base}/index.html")
            } else {
                format!("{base}/page/{}/index.html", index + 1)
            };
            if !routes.insert(route.clone()) {
                return Err(format!(
                    "Publication pages have a route collision at {route}."
                ));
            }
            listings.push(PlannedListing {
                route,
                base_path: base.to_owned(),
                title: title.to_owned(),
                collection_id: collection_id.to_owned(),
                page_ids: ids
                    .iter()
                    .skip(index * page_size)
                    .take(page_size)
                    .cloned()
                    .collect(),
                page_number: index + 1,
                total_pages,
            });
        }
        Ok(())
    }
    let mut routes = plan
        .pages
        .iter()
        .map(|page| page.route.clone())
        .collect::<std::collections::HashSet<_>>();
    let mut listings = Vec::new();
    for collection in &plan.collections {
        let Some(slug) = &collection.listing_slug else {
            continue;
        };
        let size = usize::from(collection.page_size);
        add_pages(
            &mut listings,
            &mut routes,
            &collection.id,
            slug,
            &collection.title,
            &collection.page_ids,
            size,
        )?;
        if collection.feed {
            let route = format!("{slug}/feed.xml");
            if !routes.insert(route.clone()) {
                return Err(format!(
                    "Publication pages have a route collision at {route}."
                ));
            }
        }
        if !collection.archives {
            continue;
        }
        let mut by_year: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let mut by_month: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let mut by_tag: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let mut dated = collection.page_ids.clone();
        dated.sort_by(|a, b| {
            let left = plan
                .pages
                .iter()
                .find(|page| &page.id == a)
                .and_then(|page| page.published_on.as_deref())
                .unwrap_or("");
            let right = plan
                .pages
                .iter()
                .find(|page| &page.id == b)
                .and_then(|page| page.published_on.as_deref())
                .unwrap_or("");
            right.cmp(left).then_with(|| a.cmp(b))
        });
        for id in dated {
            let page = plan
                .pages
                .iter()
                .find(|page| page.id == id)
                .ok_or("Archive page was not planned.")?;
            let date = page
                .published_on
                .as_deref()
                .ok_or("Archive page has no publication date.")?;
            by_year
                .entry(date[..4].to_owned())
                .or_default()
                .push(id.clone());
            by_month
                .entry(date[..7].to_owned())
                .or_default()
                .push(id.clone());
            for tag in &page.tags {
                by_tag.entry(tag.clone()).or_default().push(id.clone());
            }
        }
        for (year, ids) in by_year {
            add_pages(
                &mut listings,
                &mut routes,
                &collection.id,
                &format!("{slug}/archive/{year}"),
                &format!("{} · {year}", collection.title),
                &ids,
                size,
            )?;
        }
        for (month, ids) in by_month {
            add_pages(
                &mut listings,
                &mut routes,
                &collection.id,
                &format!("{slug}/archive/{}/{}", &month[..4], &month[5..7]),
                &format!("{} · {month}", collection.title),
                &ids,
                size,
            )?;
        }
        for (tag, ids) in by_tag {
            add_pages(
                &mut listings,
                &mut routes,
                &collection.id,
                &format!("{slug}/tag/{tag}"),
                &format!("{} · {tag}", collection.title),
                &ids,
                size,
            )?;
        }
    }
    plan.listings = listings;
    Ok(())
}

pub(super) fn revision(item: &Value) -> Result<&str, String> {
    item["revision"]
        .as_str()
        .ok_or_else(|| "Item has no revision".to_owned())
}

pub(super) fn title(item: &Value) -> Result<&str, String> {
    item["meta"]["title"]
        .as_str()
        .filter(|title| !title.trim().is_empty())
        .ok_or_else(|| format!("Item {} needs a title for publication.", item["id"]))
}

pub(super) fn include_page(page: &Value, content: &PublicationContent) -> bool {
    let mut included = true;
    for rule in &content.selection_rules {
        if !matches!(rule.target, SelectionTarget::Page)
            || !matches!(rule.matcher.kind, SelectionMatcherKind::PageTag)
        {
            continue;
        }
        let matches = page["tags"].as_array().is_some_and(|tags| {
            tags.iter()
                .any(|tag| tag.as_str() == Some(&rule.matcher.value))
        });
        if matches {
            included = matches!(rule.effect, SelectionEffect::Include);
        }
    }
    included
}
