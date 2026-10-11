// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use super::*;

pub(super) fn site_navigation(plan: &PublicationPlan, current_id: &str) -> Result<String, String> {
    site_navigation_at(plan, &page_file(plan, current_id), Some(current_id))
}

pub(super) fn site_navigation_at(
    plan: &PublicationPlan,
    current_route: &str,
    current_id: Option<&str>,
) -> Result<String, String> {
    let base = root_prefix(current_route);
    let mut nav = format!(
        "<form class=\"publication-search\" role=\"search\" data-publication-search data-base=\"{base}\"><label for=\"publication-search-input\">Search this publication</label><input id=\"publication-search-input\" type=\"search\" name=\"q\" autocomplete=\"off\"><div data-publication-search-results role=\"status\" aria-live=\"polite\"></div></form><script defer src=\"{base}search.js\"></script><ol class=\"publication-contents\"><li>"
    );
    write!(
        nav,
        "<a href=\"{}\"{}>{}</a>",
        relative_href(current_route, &page_file(plan, &plan.root_page_id)),
        if current_id == Some(plan.root_page_id.as_str()) {
            " aria-current=\"page\""
        } else {
            ""
        },
        escape_html(&plan.title)
    )
    .expect("writing to a String cannot fail");
    write_navigation_children(plan, current_route, current_id, &plan.structure, &mut nav)?;
    nav.push_str("</li></ol>");
    Ok(nav)
}

pub(super) fn search_script(
    plan: &PublicationPlan,
    page_items: &HashMap<&str, Value>,
) -> Result<String, String> {
    let mut entries = Vec::with_capacity(plan.pages.len());
    for page in &plan.pages {
        let item = page_items
            .get(page.id.as_str())
            .ok_or("Publication page was not captured for search.")?;
        let exclusions: HashSet<String> = page.excluded_block_ids.iter().cloned().collect();
        let fields = resolved_fields(item, document(item))?;
        let mut text = String::new();
        collect_search_text(document(item), &exclusions, &fields, &mut text)?;
        let search = format!("{} {text}", page.title)
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase();
        entries.push(serde_json::json!({
            "title": page.title,
            "path": page_file(plan, &page.id),
            "search": search,
        }));
    }
    let index = serde_json::to_string(&entries).map_err(|error| error.to_string())?;
    Ok(SEARCH_SCRIPT.replace("__BLUINK_SEARCH_INDEX__", &index))
}

pub(super) fn collect_search_text(
    node: &Value,
    exclusions: &HashSet<String>,
    fields: &HashMap<String, String>,
    output: &mut String,
) -> Result<(), String> {
    if node["attrs"]["id"]
        .as_str()
        .is_some_and(|id| exclusions.contains(id))
    {
        return Ok(());
    }
    match node["type"].as_str() {
        Some("text") => {
            output.push(' ');
            output.push_str(node["text"].as_str().unwrap_or(""));
        }
        Some("field") => {
            let key = node["attrs"]["key"]
                .as_str()
                .ok_or("Search field has no key.")?;
            output.push(' ');
            output.push_str(
                fields
                    .get(key)
                    .ok_or_else(|| format!("Search field {key} was not resolved."))?,
            );
        }
        _ => {
            for child in node["content"].as_array().into_iter().flatten() {
                collect_search_text(child, exclusions, fields, output)?;
            }
        }
    }
    Ok(())
}

pub(super) fn page_contents(
    document: &Value,
    exclusions: &HashSet<String>,
    fields: &HashMap<String, String>,
    heading_offset: i16,
) -> Result<String, String> {
    let mut entries = Vec::new();
    collect_page_headings(document, exclusions, fields, heading_offset, &mut entries)?;
    if entries.is_empty() {
        return Ok(String::new());
    }
    let mut html = String::from(
        "<nav class=\"publication-page-contents\" aria-label=\"On this page\"><h2>On this page</h2><ol>",
    );
    for (id, title, level) in entries {
        write!(html,
            "<li class=\"publication-page-contents__level-{level}\"><a href=\"#block-{}\">{}</a></li>",
            escape_html(&id),
            escape_html(&title)
        ).expect("writing to a String cannot fail");
    }
    html.push_str("</ol></nav>\n");
    Ok(html)
}

pub(super) fn collect_page_headings(
    node: &Value,
    exclusions: &HashSet<String>,
    fields: &HashMap<String, String>,
    heading_offset: i16,
    headings: &mut Vec<(String, String, i16)>,
) -> Result<(), String> {
    let attrs = &node["attrs"];
    if attrs["id"]
        .as_str()
        .is_some_and(|id| exclusions.contains(id))
    {
        return Ok(());
    }
    if node["type"] == "heading"
        && let Some(id) = attrs["id"].as_str()
    {
        let mut title = String::new();
        heading_text(node, fields, &mut title)?;
        let title = title.split_whitespace().collect::<Vec<_>>().join(" ");
        if !title.is_empty() {
            let level = i16::try_from(attrs["level"].as_u64().unwrap_or(1))
                .map_err(|_| "Heading level is too large.".to_owned())?
                + heading_offset;
            headings.push((id.to_owned(), title, level.clamp(1, 6)));
        }
    }
    for child in node["content"].as_array().into_iter().flatten() {
        collect_page_headings(child, exclusions, fields, heading_offset, headings)?;
    }
    Ok(())
}

pub(super) fn heading_text(
    node: &Value,
    fields: &HashMap<String, String>,
    output: &mut String,
) -> Result<(), String> {
    match node["type"].as_str() {
        Some("text") => output.push_str(node["text"].as_str().unwrap_or("")),
        Some("hard_break") => output.push(' '),
        Some("field") => {
            let key = node["attrs"]["key"]
                .as_str()
                .ok_or("Heading field has no key.")?;
            output.push_str(
                fields
                    .get(key)
                    .ok_or_else(|| format!("Heading field {key} was not resolved."))?,
            );
        }
        _ => {
            for child in node["content"].as_array().into_iter().flatten() {
                heading_text(child, fields, output)?;
            }
        }
    }
    Ok(())
}

#[expect(
    clippy::too_many_lines,
    reason = "Keep the complete format or operation dispatch together for contract review."
)]
pub(super) fn write_navigation_children(
    plan: &PublicationPlan,
    current_route: &str,
    current_id: Option<&str>,
    children: &[PlannedStructureNode],
    nav: &mut String,
) -> Result<(), String> {
    if children.is_empty() {
        return Ok(());
    }
    nav.push_str("<ol>");
    for node in children {
        match node {
            PlannedStructureNode::Page {
                page_id,
                show_in_navigation,
                children,
                ..
            } => {
                if !show_in_navigation {
                    continue;
                }
                let page = plan
                    .pages
                    .iter()
                    .find(|page| &page.id == page_id)
                    .ok_or("Navigation page was not planned.")?;
                write!(
                    nav,
                    "<li><a href=\"{}\"{}>{}</a>",
                    relative_href(current_route, &page_file(plan, page_id)),
                    if current_id == Some(page_id.as_str()) {
                        " aria-current=\"page\""
                    } else {
                        ""
                    },
                    escape_html(&page.title)
                )
                .expect("writing to a String cannot fail");
                write_navigation_children(plan, current_route, current_id, children, nav)?;
                nav.push_str("</li>");
            }
            PlannedStructureNode::Section {
                title,
                intro_page_id,
                children,
                ..
            } => {
                nav.push_str("<li>");
                if let Some(page_id) = intro_page_id {
                    write!(
                        nav,
                        "<a href=\"{}\"{}>{}</a>",
                        relative_href(current_route, &page_file(plan, page_id)),
                        if current_id == Some(page_id.as_str()) {
                            " aria-current=\"page\""
                        } else {
                            ""
                        },
                        escape_html(title)
                    )
                    .expect("writing to a String cannot fail");
                } else {
                    write!(nav, "<span>{}</span>", escape_html(title))
                        .expect("writing to a String cannot fail");
                }
                write_navigation_children(plan, current_route, current_id, children, nav)?;
                nav.push_str("</li>");
            }
            PlannedStructureNode::Collection {
                title,
                page_ids,
                show_pages_in_navigation,
                listing_slug,
                ..
            } => {
                nav.push_str("<li>");
                if let Some(slug) = listing_slug {
                    let route = format!("{slug}/index.html");
                    write!(
                        nav,
                        "<a href=\"{}\"{}>{}</a>",
                        relative_href(current_route, &route),
                        if current_route == route {
                            " aria-current=\"page\""
                        } else {
                            ""
                        },
                        escape_html(title)
                    )
                    .expect("writing to a String cannot fail");
                } else {
                    write!(nav, "<span>{}</span>", escape_html(title))
                        .expect("writing to a String cannot fail");
                }
                if *show_pages_in_navigation {
                    nav.push_str("<ol>");
                    for id in page_ids {
                        let page = plan
                            .pages
                            .iter()
                            .find(|page| &page.id == id)
                            .ok_or("Collection page was not planned.")?;
                        write!(
                            nav,
                            "<li><a href=\"{}\"{}>{}</a></li>",
                            relative_href(current_route, &page_file(plan, id)),
                            if current_id == Some(id.as_str()) {
                                " aria-current=\"page\""
                            } else {
                                ""
                            },
                            escape_html(&page.title)
                        )
                        .expect("writing to a String cannot fail");
                    }
                    nav.push_str("</ol>");
                }
                nav.push_str("</li>");
            }
        }
    }
    nav.push_str("</ol>");
    Ok(())
}
