// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use serde_json::Value;
use url::Url;

/// Syntax checks only. Private hosts and unreachable servers are valid;
/// neither saving nor exporting performs a network request.
pub(crate) fn valid_external_url(value: &str) -> bool {
    Url::parse(value).is_ok_and(|url| {
        (matches!(url.scheme(), "http" | "https") && url.has_host())
            || (url.scheme() == "mailto" && !url.path().is_empty())
    })
}

/// Reject malformed HTTP destinations before committing a page. Membership,
/// anchors, assets and export-specific policies are checked by the planner.
pub(crate) fn validate_http_links(
    node: &Value,
    inherited_block: Option<&str>,
) -> Result<(), String> {
    let block = node["attrs"]["id"].as_str().or(inherited_block);
    if let Some(marks) = node["marks"].as_array() {
        for mark in marks {
            if mark["type"] == "link"
                && let Some(href) = mark["attrs"]["href"].as_str()
                && href.split(':').next().is_some_and(|scheme| {
                    scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https")
                })
                && !valid_external_url(href)
            {
                let context = block.map_or_else(|| "Page".to_owned(), |id| format!("Block {id}"));
                return Err(format!("{context}: invalid HTTP link URL: {href}."));
            }
        }
    }
    if let Some(children) = node["content"].as_array() {
        for child in children {
            validate_http_links(child, block)?;
        }
    }
    if let Some(paragraph) = node["attrs"].get("paragraph") {
        validate_http_links(paragraph, block)?;
    }
    Ok(())
}
