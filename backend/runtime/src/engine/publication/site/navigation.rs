// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use super::*;

pub(super) fn site_navigation(plan: &SitePlan<'_>, current_id: &str) -> Result<String, String> {
    site_navigation_at(plan, &page_file(plan, current_id), Some(current_id))
}

#[expect(
    clippy::expect_used,
    reason = "Formatting only strings and primitive values into String is infallible."
)]
pub(super) fn site_navigation_at(
    plan: &SitePlan<'_>,
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

pub(super) fn search_entry(
    plan: &SitePlan<'_>,
    page: &super::super::plan::PlannedPage,
    text: &str,
) -> Value {
    let search = format!("{} {text}", page.title)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();
    serde_json::json!({"title":page.title,"path":page_file(plan, &page.id),"search":search})
}

pub(super) fn search_script(entries: &[Value]) -> Result<String, String> {
    let index = serde_json::to_string(entries).map_err(|error| error.to_string())?;
    Ok(SEARCH_SCRIPT.replace("__BLUINK_SEARCH_INDEX__", &index))
}

#[expect(
    clippy::too_many_lines,
    reason = "Keep the complete format or operation dispatch together for contract review."
)]
#[expect(
    clippy::expect_used,
    reason = "Formatting only strings and primitive values into String is infallible."
)]
pub(super) fn write_navigation_children(
    plan: &SitePlan<'_>,
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
                    .page(page_id)
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
                        let page = plan.page(id).ok_or("Collection page was not planned.")?;
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
