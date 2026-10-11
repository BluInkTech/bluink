// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

pub(crate) mod article;
pub(crate) mod fonts;
pub(crate) mod html;
pub(crate) mod macros;
pub(crate) mod note_pdf;
pub(crate) mod one_html;
pub(crate) mod pdf;
pub(crate) mod plan;
pub(crate) mod preflight;
pub(crate) mod request;
pub(crate) mod selection;
pub(crate) mod site;
pub(crate) mod source;
pub(crate) mod style;
pub(crate) mod theme;
pub(crate) mod typst;

use std::collections::{BTreeMap, HashSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use url::Url;

use crate::engine::project::item::{is_item_id, is_tag_key};
use crate::engine::project::properties::is_snake_case;
use crate::engine::publication::theme::validate_theme_override;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct PublicationContent {
    pub(crate) kind: PublicationKind,
    pub(crate) preset: PublicationPreset,
    pub(crate) selected_output: PublicationOutput,
    pub(crate) root_page_id: String,
    pub(crate) outline: Vec<OutlineNode>,
    pub(crate) selection_rules: Vec<SelectionRule>,
    pub(crate) theme_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub(crate) theme_settings: Option<theme::ThemeSettingsOverride>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    #[cfg_attr(test, ts(as = "BTreeMap<String, theme::ThemeOptionValue>"))]
    pub(crate) theme_options: BTreeMap<String, Value>,
    pub(crate) targets: PublicationTargets,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub(crate) enum PublicationKind {
    Publication,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub(crate) enum PublicationPreset {
    Book,
    Documentation,
    Blog,
    Website,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub(crate) enum PublicationOutput {
    Pdf,
    Html,
    Site,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[cfg_attr(test, derive(ts_rs::TS))]
pub(crate) enum OutlineNode {
    Page {
        #[serde(rename = "pageId")]
        page_id: String,
        children: Vec<Self>,
        #[serde(rename = "showInNavigation", default = "default_true")]
        #[cfg_attr(test, ts(as = "Option<bool>", optional))]
        show_in_navigation: bool,
        #[serde(default)]
        #[cfg_attr(test, ts(optional = nullable))]
        slug: Option<String>,
    },
    Section {
        id: String,
        title: String,
        #[serde(rename = "introPageId", default)]
        #[cfg_attr(test, ts(optional = nullable))]
        intro_page_id: Option<String>,
        children: Vec<Self>,
    },
    Collection {
        id: String,
        title: String,
        query: String,
        #[serde(rename = "showPagesInNavigation", default = "default_true")]
        #[cfg_attr(test, ts(as = "Option<bool>", optional))]
        show_pages_in_navigation: bool,
        #[serde(rename = "listingSlug", default)]
        #[cfg_attr(test, ts(optional = nullable))]
        listing_slug: Option<String>,
        #[serde(rename = "pageSize", default = "default_page_size")]
        #[cfg_attr(test, ts(as = "Option<u16>", optional))]
        page_size: u16,
        #[serde(default)]
        #[cfg_attr(test, ts(as = "Option<bool>", optional))]
        feed: bool,
        #[serde(default)]
        #[cfg_attr(test, ts(as = "Option<bool>", optional))]
        archives: bool,
    },
}

const fn default_true() -> bool {
    true
}
const fn default_page_size() -> u16 {
    20
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SelectionRule {
    pub(crate) id: String,
    pub(crate) effect: SelectionEffect,
    pub(crate) target: SelectionTarget,
    pub(crate) matcher: SelectionMatcher,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub(crate) enum SelectionEffect {
    Include,
    Exclude,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub(crate) enum SelectionTarget {
    Page,
    Block,
    Section,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SelectionMatcher {
    pub(crate) kind: SelectionMatcherKind,
    pub(crate) value: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub(crate) enum SelectionMatcherKind {
    BlockType,
    BlockLabel,
    PageTag,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct PublicationTargets {
    pub(crate) pdf: PdfTarget,
    pub(crate) html: HtmlTarget,
    pub(crate) site: SiteTarget,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct HtmlTarget {
    pub(crate) outside_links: OutsideLinks,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct PdfTarget {
    pub(crate) profile: PdfProfile,
    pub(crate) outside_links: OutsideLinks,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub(crate) enum PdfProfile {
    Default,
    Accessible,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "kebab-case")]
pub(crate) enum OutsideLinks {
    Error,
    TextOnly,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SiteTarget {
    pub(crate) layout: SiteLayout,
    pub(crate) outside_links: OutsideLinks,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub(crate) base_url: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub(crate) enum SiteLayout {
    Documentation,
    Website,
}

pub(crate) fn validate_publication(content: &Value) -> Result<PublicationContent, String> {
    let publication: PublicationContent = serde_json::from_value(content.clone())
        .map_err(|error| format!("Invalid Publication content: {error}"))?;
    if !is_item_id(&publication.root_page_id) {
        return Err("Publication rootPageId must be a page UUID.".to_owned());
    }
    if !is_theme_id(&publication.theme_id) {
        return Err("Publication themeId must be a lowercase hyphenated ID.".to_owned());
    }
    if let Some(settings) = &publication.theme_settings {
        validate_theme_override(
            &serde_json::to_value(settings).map_err(|error| error.to_string())?,
        )?;
    }
    let mut pages = HashSet::from([publication.root_page_id.as_str()]);
    let mut nodes = HashSet::new();
    for node in &publication.outline {
        validate_outline_node(node, &mut pages, &mut nodes)?;
    }
    let mut rules = HashSet::new();
    for rule in &publication.selection_rules {
        if !is_item_id(&rule.id) || !rules.insert(rule.id.as_str()) {
            return Err("Publication selection rules need unique UUIDs.".to_owned());
        }
        match (&rule.target, &rule.matcher.kind) {
            (SelectionTarget::Page, SelectionMatcherKind::PageTag)
                if is_tag_key(&rule.matcher.value) => {}
            (
                SelectionTarget::Block | SelectionTarget::Section,
                SelectionMatcherKind::BlockLabel,
            ) if is_snake_case(&rule.matcher.value) => {}
            (SelectionTarget::Block, SelectionMatcherKind::BlockType)
                if is_block_type(&rule.matcher.value) => {}
            (SelectionTarget::Section, SelectionMatcherKind::BlockType)
                if rule.matcher.value == "heading" => {}
            _ => {
                return Err(format!(
                    "Publication rule {} has an invalid target or matcher.",
                    rule.id
                ));
            }
        }
    }
    if let Some(base_url) = &publication.targets.site.base_url {
        let valid = Url::parse(base_url).is_ok_and(|url| {
            matches!(url.scheme(), "http" | "https")
                && url.has_host()
                && url.username().is_empty()
                && url.password().is_none()
                && url.query().is_none()
                && url.fragment().is_none()
                && base_url.ends_with('/')
        });
        if !valid {
            return Err("Publication site baseUrl must be an absolute HTTP(S) URL ending in /, without credentials, query, or fragment.".to_owned());
        }
    }
    Ok(publication)
}

fn validate_outline_node<'a>(
    node: &'a OutlineNode,
    pages: &mut HashSet<&'a str>,
    nodes: &mut HashSet<&'a str>,
) -> Result<(), String> {
    let children = match node {
        OutlineNode::Page {
            page_id,
            children,
            slug,
            ..
        } => {
            register_page(page_id, pages)?;
            if let Some(slug) = slug {
                validate_slug(slug)?;
            }
            children
        }
        OutlineNode::Section {
            id,
            title,
            intro_page_id,
            children,
        } => {
            register_node(id, nodes)?;
            if title.trim().is_empty() || title.len() > 160 {
                return Err(
                    "Publication sections need a title of at most 160 characters.".to_owned(),
                );
            }
            if let Some(page_id) = intro_page_id {
                register_page(page_id, pages)?;
            }
            children
        }
        OutlineNode::Collection {
            id,
            title,
            query,
            listing_slug,
            page_size,
            ..
        } => {
            register_node(id, nodes)?;
            if title.trim().is_empty() || title.len() > 160 || query.trim().is_empty() {
                return Err("Publication collections need a title and query.".to_owned());
            }
            if let Some(slug) = listing_slug {
                validate_slug(slug)?;
            }
            if !(1..=100).contains(page_size) {
                return Err("Publication collection pageSize must be between 1 and 100.".to_owned());
            }
            return Ok(());
        }
    };
    for child in children {
        validate_outline_node(child, pages, nodes)?;
    }
    Ok(())
}

fn register_page<'a>(page_id: &'a str, pages: &mut HashSet<&'a str>) -> Result<(), String> {
    if !is_item_id(page_id) {
        return Err("Publication pageId must be a page UUID.".to_owned());
    }
    if !pages.insert(page_id) {
        return Err(format!(
            "Page {page_id} appears more than once in the Publication."
        ));
    }
    Ok(())
}

fn register_node<'a>(id: &'a str, nodes: &mut HashSet<&'a str>) -> Result<(), String> {
    if !is_item_id(id) || !nodes.insert(id) {
        return Err("Publication sections and collections need distinct UUIDs.".to_owned());
    }
    Ok(())
}

fn validate_slug(slug: &str) -> Result<(), String> {
    if slug.is_empty()
        || slug.len() > 80
        || !slug
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        || slug.starts_with('-')
        || slug.ends_with('-')
        || slug.contains("--")
    {
        return Err(
            "Publication slugs must be lowercase words separated by single hyphens.".to_owned(),
        );
    }
    Ok(())
}

fn is_theme_id(value: &str) -> bool {
    let mut segments = value.split('-');
    segments.next().is_some_and(valid_theme_segment) && segments.all(valid_theme_segment)
}

fn valid_theme_segment(segment: &str) -> bool {
    !segment.is_empty()
        && segment
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_lowercase())
        && segment
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
}

fn is_block_type(value: &str) -> bool {
    matches!(
        value,
        "paragraph"
            | "heading"
            | "code_block"
            | "horizontal_rule"
            | "blockquote"
            | "list"
            | "listItem"
            | "image"
            | "box"
            | "boxHeading"
            | "figure"
            | "table"
            | "table_row"
            | "table_cell"
            | "table_header"
            | "embed"
    )
}

/// Encode a digest without allocating a temporary String for each byte.
fn digest_hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut hex = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(hex, "{byte:02x}").expect("writing to a String cannot fail");
    }
    hex
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{OutlineNode, validate_publication};

    fn valid() -> serde_json::Value {
        json!({
            "kind": "publication", "preset": "book", "selectedOutput": "pdf",
            "rootPageId": "01a0e2a0-0000-7000-8000-000000000001",
            "outline": [{"kind":"page","pageId":"01a0e2a0-0000-7000-8000-000000000002", "children": [
                {"kind":"page","pageId":"01a0e2a0-0000-7000-8000-000000000003", "children": []}
            ]}],
            "selectionRules": [{"id":"01a0e2a0-0000-7000-8000-000000000004", "effect":"exclude",
                "target":"block", "matcher":{"kind":"block_label", "value":"draft_note"}}],
            "themeId": "classic",
            "targets": {"pdf":{"profile":"default","outsideLinks":"error"},
                "html":{"outsideLinks":"error"},
                "site":{"layout":"documentation","outsideLinks":"text-only"}}
        })
    }

    #[test]
    fn accepts_nested_pages_and_explicit_rules() {
        let content = valid();
        let parsed = validate_publication(&content).unwrap();
        assert!(
            matches!(&parsed.outline[0], OutlineNode::Page { children, .. } if children.len() == 1)
        );
        assert_eq!(parsed.root_page_id, content["rootPageId"]);
    }

    #[test]
    fn stores_an_explicit_output_independent_of_preset() {
        for preset in ["book", "documentation", "blog", "website"] {
            for output in ["pdf", "html", "site"] {
                let mut content = valid();
                content["preset"] = json!(preset);
                content["selectedOutput"] = json!(output);
                let parsed = validate_publication(&content).unwrap();
                assert_eq!(
                    serde_json::to_value(parsed.selected_output).unwrap(),
                    json!(output)
                );
            }
        }
        let mut missing = valid();
        missing.as_object_mut().unwrap().remove("selectedOutput");
        assert!(
            validate_publication(&missing)
                .unwrap_err()
                .contains("selectedOutput")
        );
    }

    #[test]
    fn rejects_duplicate_pages_and_invalid_rule_combinations() {
        let mut duplicate = valid();
        duplicate["outline"][0]["children"][0]["pageId"] = duplicate["rootPageId"].clone();
        assert!(
            validate_publication(&duplicate)
                .unwrap_err()
                .contains("more than once")
        );
        let mut invalid = valid();
        invalid["selectionRules"][0]["target"] = json!("page");
        assert!(
            validate_publication(&invalid)
                .unwrap_err()
                .contains("invalid target")
        );
        invalid["selectionRules"][0]["target"] = json!("block");
        invalid["selectionRules"][0]["matcher"]["kind"] = json!("page_tag");
        assert!(validate_publication(&invalid).is_err());
        let mut invalid_base = valid();
        invalid_base["targets"]["site"]["baseUrl"] = json!("https://example.org/docs");
        assert!(
            validate_publication(&invalid_base)
                .unwrap_err()
                .contains("baseUrl")
        );
    }

    #[test]
    fn validates_shared_theme_settings_on_the_publication() {
        let mut content = valid();
        content["themeSettings"] = json!({"typography":{"bodyScale":1.2}});
        assert_eq!(
            serde_json::to_value(
                validate_publication(&content)
                    .unwrap()
                    .theme_settings
                    .unwrap()
            )
            .unwrap()["typography"]["bodyScale"],
            json!(1.2)
        );
        content["themeSettings"]["unknown"] = json!({"ink":"#ffffff"});
        assert!(validate_publication(&content).is_err());
    }
}
