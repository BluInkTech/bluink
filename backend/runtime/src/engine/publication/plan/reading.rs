// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use super::{PlannedStructureNode, PublicationPlan};
use std::collections::HashSet;

/// Cover titles and chapter titles precede the authored page headings. Root
/// pages have outline depth zero but their first body heading is still level 2.
pub(crate) fn body_heading_level(depth: usize) -> Result<i16, String> {
    i16::try_from(depth)
        .ok()
        .and_then(|depth| depth.checked_add(1))
        .map(|level| level.max(2))
        .ok_or_else(|| "Publication outline is too deep.".into())
}

pub(crate) enum BookEntry {
    Section {
        id: String,
        title: String,
        number: String,
        depth: usize,
    },
    Page {
        id: String,
        number: Option<String>,
        intro: bool,
    },
}

pub(crate) fn collect_book_entries(
    nodes: &[PlannedStructureNode],
    depth: usize,
    counts: &mut Vec<usize>,
    rendered_page_ids: &mut HashSet<String>,
    output: &mut Vec<BookEntry>,
) {
    for node in nodes {
        counts.truncate(depth);
        if counts.len() < depth {
            counts.resize(depth, 0);
        }
        counts[depth - 1] += 1;
        let number = counts
            .iter()
            .map(usize::to_string)
            .collect::<Vec<_>>()
            .join(".");
        match node {
            PlannedStructureNode::Page {
                page_id, children, ..
            } => {
                if rendered_page_ids.insert(page_id.clone()) {
                    output.push(BookEntry::Page {
                        id: page_id.clone(),
                        number: Some(number),
                        intro: false,
                    });
                }
                collect_book_entries(children, depth + 1, counts, rendered_page_ids, output);
            }
            PlannedStructureNode::Section {
                id,
                title,
                intro_page_id,
                children,
            } => {
                output.push(BookEntry::Section {
                    id: id.clone(),
                    title: title.clone(),
                    number,
                    depth,
                });
                if let Some(page_id) = intro_page_id
                    && rendered_page_ids.insert(page_id.clone())
                {
                    output.push(BookEntry::Page {
                        id: page_id.clone(),
                        number: None,
                        intro: true,
                    });
                }
                collect_book_entries(children, depth + 1, counts, rendered_page_ids, output);
            }
            PlannedStructureNode::Collection {
                id,
                title,
                page_ids,
                ..
            } => {
                output.push(BookEntry::Section {
                    id: id.clone(),
                    title: title.clone(),
                    number,
                    depth,
                });
                for (index, page_id) in page_ids.iter().enumerate() {
                    if rendered_page_ids.insert(page_id.clone()) {
                        output.push(BookEntry::Page {
                            id: page_id.clone(),
                            number: Some(format!(
                                "{}.{}",
                                counts
                                    .iter()
                                    .map(usize::to_string)
                                    .collect::<Vec<_>>()
                                    .join("."),
                                index + 1
                            )),
                            intro: false,
                        });
                    }
                }
            }
        }
    }
}

pub(crate) fn entries(plan: &PublicationPlan) -> Result<Vec<BookEntry>, String> {
    let mut output = vec![BookEntry::Page {
        id: plan.root_page_id.clone(),
        number: None,
        intro: false,
    }];
    let mut rendered = HashSet::from([plan.root_page_id.clone()]);
    collect_book_entries(
        &plan.structure,
        1,
        &mut Vec::new(),
        &mut rendered,
        &mut output,
    );
    if rendered.len() != plan.pages.len()
        || plan.pages.iter().any(|page| !rendered.contains(&page.id))
    {
        return Err("Publication outline does not account for every planned page.".into());
    }
    Ok(output)
}
