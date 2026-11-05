// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! Captured publication values and deterministic layout rules.
pub(crate) mod listings;
pub(crate) mod reading;
use super::{
    PublicationContent, PublicationTargets, SelectionRule,
    preflight::{PlannedAsset, PlannedLink, PlannedSnippet},
    selection::excluded_block_ids,
    source::{document, parse_language},
    theme::ThemeSnapshot,
};
use listings::{revision, title};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(test, derive(ts_rs::TS))]
pub(crate) enum PublicationFormat {
    Pdf,
    Docx,
    Epub,
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
    pub(crate) const ALL: [Self; 5] = [Self::Pdf, Self::Docx, Self::Epub, Self::Html, Self::Site];

    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::Pdf => "pdf",
            Self::Docx => "docx",
            Self::Epub => "epub",
            Self::Html => "html",
            Self::Site => "site",
        }
    }
    pub(crate) fn parse(value: &str) -> Result<Self, String> {
        Self::ALL
            .into_iter()
            .find(|format| format.name() == value)
            .ok_or_else(|| {
                format!(
                    "Unsupported Publication format: {value}. Use pdf, docx, epub, html, or site."
                )
            })
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
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
    /// A linked-pages plan resolves its links, images and snippets when
    /// generation reads its pages; until then this is true and they are empty.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    #[cfg_attr(test, ts(as = "Option<bool>", optional))]
    pub(crate) unresolved: bool,
    /// Problems that stop this export, listed together so the writer can fix
    /// them all; a plan with issues does not generate.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[cfg_attr(test, ts(as = "Option<Vec<PlanIssue>>", optional))]
    pub(crate) issues: Vec<PlanIssue>,
    /// Non-blocking decisions on included content. Export uses its currently
    /// accepted provisional form, never the alternatives embedded in a decision.
    pub(crate) warnings: Vec<DecisionWarning>,
    pub(crate) decisions_checked: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub(crate) struct DecisionWarning {
    pub(crate) page_id: String,
    pub(crate) decision_ids: Vec<String>,
}

/// One problem in an export, and the page and block it is in.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub(crate) struct PlanIssue {
    pub(crate) page_id: String,
    pub(crate) title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub(crate) block_id: Option<String>,
    pub(crate) message: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
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

#[derive(Clone, Debug, Deserialize, Serialize)]
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

#[derive(Clone, Debug, Deserialize, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub(crate) struct PlannedCollection {
    pub(crate) id: String,
    pub(crate) title: String,
    pub(crate) query: crate::domain::project::query::QueryDocument,
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

#[derive(Clone, Debug, Deserialize, Serialize)]
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

pub(crate) fn planned_page(
    item: &Value,
    depth: usize,
    parent_id: Option<String>,
    content: &PublicationContent,
) -> Result<PlannedPage, String> {
    let document = document(item);
    Ok(PlannedPage {
        id: item["id"].as_str().ok_or("Page has no ID")?.to_owned(),
        revision: revision(item)?.to_owned(),
        title: title(item)?.to_owned(),
        route: if content.targets.site.layout
            == crate::domain::publication::SiteLayout::Documentation
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
