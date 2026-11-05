// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use serde_json::Value;
use url::Url;

/// Syntax checks only. Private hosts and unreachable servers are valid;
/// neither saving nor exporting performs a network request.
pub(crate) fn valid_external_url(value: &str) -> bool {
    export_url(value).is_some()
}

/// The address every export writes for an external link: the parsed URL with
/// any remaining backslash percent-encoded. Browsers already read a path
/// backslash as a slash; one in a query or fragment would otherwise reach the
/// output raw, which renderers refuse. Checking and generation share this, so
/// a link that passes the checks always exports.
pub(crate) fn export_url(value: &str) -> Option<String> {
    let url = Url::parse(value).ok()?;
    ((matches!(url.scheme(), "http" | "https") && url.has_host())
        || (url.scheme() == "mailto" && !url.path().is_empty()))
    .then(|| url.as_str().replace('\\', "%5C"))
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

#[cfg(test)]
mod tests {
    use super::export_url;

    #[test]
    fn exports_write_the_parsed_address_with_no_raw_backslash() {
        // A real Wikipedia citation: the backslashes are part of its query.
        assert_eq!(
            export_url(
                r"http://www.encyclopediaofukraine.com/display.asp?linkPath=pages\Z\A\ZaporizhiaThe.htm"
            )
            .as_deref(),
            Some(
                "http://www.encyclopediaofukraine.com/display.asp?linkPath=pages%5CZ%5CA%5CZaporizhiaThe.htm"
            )
        );
        // Browsers read a backslash in a web address's path as a slash.
        assert_eq!(
            export_url(r"https://example.com/a\b").as_deref(),
            Some("https://example.com/a/b")
        );
        assert_eq!(
            export_url("mailto:writer@example.com").as_deref(),
            Some("mailto:writer@example.com")
        );
        for invalid in ["not a link", "javascript:alert(1)", "https://", "mailto:"] {
            assert_eq!(export_url(invalid), None, "{invalid}");
        }
    }
}
