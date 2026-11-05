// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use super::Destination;
use crate::domain::publication::preflight::{LinkResolution, PlannedLink};
use std::collections::{HashMap, HashSet, hash_map::Entry};
use url::Url;

/// Native document links share one closed set of anchors and external URLs.
/// The `block-…` labels any planned link points at, across every page.
/// Only these blocks need a label in the output.
pub(crate) fn linked_anchors(links: &[PlannedLink]) -> HashSet<String> {
    links
        .iter()
        .filter(|link| matches!(link.resolution, LinkResolution::Anchor))
        .filter_map(|link| link.destination.as_deref()?.strip_prefix('#'))
        .map(str::to_owned)
        .collect()
}

pub(crate) fn native_links(
    links: &[PlannedLink],
    source: &str,
) -> Result<HashMap<String, Destination>, String> {
    let mut output = HashMap::new();
    for link in links.iter().filter(|link| link.page_id == source) {
        insert_native_link(&mut output, link)?;
    }
    Ok(output)
}

/// Resolve each link once for book renderers instead of scanning the full
/// publication's links again for every source page.
pub(crate) fn native_links_by_page(
    links: &[PlannedLink],
) -> Result<HashMap<&str, HashMap<String, Destination>>, String> {
    let mut output = HashMap::new();
    for link in links {
        insert_native_link(output.entry(link.page_id.as_str()).or_default(), link)?;
    }
    Ok(output)
}

fn insert_native_link(
    output: &mut HashMap<String, Destination>,
    link: &PlannedLink,
) -> Result<(), String> {
    let destination = match link.resolution {
        LinkResolution::Included => Destination::Anchor(format!(
            "note-{}",
            link.target_id
                .as_deref()
                .ok_or("Included link has no target ID")?
        )),
        LinkResolution::Anchor => Destination::Anchor(
            link.destination
                .as_deref()
                .and_then(|value| value.strip_prefix('#'))
                .ok_or("Anchor has no fragment")?
                .into(),
        ),
        LinkResolution::External => Destination::Url(
            link.destination
                .clone()
                .ok_or("External link has no destination")?,
        ),
        LinkResolution::TextOnly => Destination::PlainText,
    };
    match output.entry(link.href.clone()) {
        Entry::Occupied(entry) if entry.get() != &destination => Err(format!(
            "Source page {} has conflicting link targets.",
            link.page_id
        )),
        Entry::Occupied(_) => Ok(()),
        Entry::Vacant(entry) => {
            entry.insert(destination);
            Ok(())
        }
    }
}

/// Output destinations are validated independently of their source spelling.
/// Relative site routes and embedded image data are legitimate planned resources.
pub(crate) fn safe_output_url(value: &str, image: bool) -> bool {
    if value.is_empty()
        || value.starts_with("//")
        || value.contains('\\')
        || value.chars().any(char::is_control)
    {
        return false;
    }
    if let Ok(url) = Url::parse(value) {
        return if image {
            (matches!(url.scheme(), "http" | "https") && url.has_host())
                || [
                    "data:image/png;base64,",
                    "data:image/jpeg;base64,",
                    "data:image/webp;base64,",
                    "data:image/gif;base64,",
                    "data:image/svg+xml;base64,",
                ]
                .iter()
                .any(|prefix| {
                    value.strip_prefix(prefix).is_some_and(|data| {
                        !data.is_empty()
                            && data.bytes().all(|byte| {
                                byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'/' | b'=')
                            })
                    })
                })
        } else {
            (matches!(url.scheme(), "http" | "https") && url.has_host())
                || (url.scheme() == "mailto" && !url.path().is_empty())
        };
    }
    !value
        .split('/')
        .next()
        .is_some_and(|part| part.contains(':'))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn external(page: &str, destination: &str) -> PlannedLink {
        PlannedLink {
            page_id: page.into(),
            block_id: None,
            href: "same source spelling".into(),
            resolution: LinkResolution::External,
            target_id: None,
            destination: Some(destination.into()),
        }
    }

    #[test]
    fn book_links_keep_destinations_scoped_to_the_source_page() {
        let links = [
            external("one", "https://one.test"),
            external("two", "https://two.test"),
            external("one", "https://one.test"),
        ];
        let grouped = native_links_by_page(&links).unwrap();
        for page in ["one", "two"] {
            assert_eq!(grouped[page], native_links(&links, page).unwrap());
        }
        assert_ne!(grouped["one"], grouped["two"]);
        assert!(native_links(&links, "empty").unwrap().is_empty());
    }

    #[test]
    fn book_links_reject_conflicts_and_incomplete_destinations() {
        let links = [
            external("one", "https://one.test"),
            external("one", "https://two.test"),
        ];
        assert_eq!(
            native_links_by_page(&links).unwrap_err(),
            native_links(&links, "one").unwrap_err()
        );
        for resolution in [
            LinkResolution::Included,
            LinkResolution::Anchor,
            LinkResolution::External,
        ] {
            let mut link = external("one", "https://one.test");
            link.resolution = resolution;
            link.destination = None;
            assert!(native_links_by_page(&[link]).is_err());
        }
    }
}
