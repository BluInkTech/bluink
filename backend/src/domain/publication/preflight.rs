// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! Prose facts, export constraints and captured resource descriptions.
use super::{OutsideLinks, plan::PublicationFormat};
use crate::domain::document::links::valid_external_url;
use crate::domain::project::identity::is_item_id;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashSet;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub(crate) struct PlannedAsset {
    pub(crate) src: String,
    pub(crate) owner_id: String,
    pub(crate) filename: String,
    pub(crate) bytes: u64,
    pub(crate) sha256: String,
    /// The image file's stamp when it was hashed.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub(crate) stamp: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub(crate) struct PlannedLink {
    pub(crate) page_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub(crate) block_id: Option<String>,
    pub(crate) href: String,
    pub(crate) resolution: LinkResolution,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub(crate) target_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub(crate) destination: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub(crate) struct PlannedSnippet {
    pub(crate) id: String,
    pub(crate) revision: String,
    pub(crate) paragraph: Value,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub(crate) enum LinkResolution {
    Included,
    TextOnly,
    External,
    Anchor,
}

pub(crate) struct PreflightResult {
    pub(crate) assets: Vec<PlannedAsset>,
    pub(crate) links: Vec<PlannedLink>,
    pub(crate) snippets: Vec<PlannedSnippet>,
    /// Words across every scanned page, to check the export's size.
    pub(crate) words: usize,
}

/// The most words one PDF, Word, EPUB or single web page may hold, sized for
/// archival books. Typst lays out a whole PDF in memory, measured at up to
/// 0.71 GB per 100,000 words on link-dense Wikipedia pages, so a book at this
/// limit (about 5,600 PDF pages) needs about 14 GB. A website writes each page
/// separately and has no limit.
pub(crate) const MAX_DOCUMENT_WORDS: usize = 2_000_000;

/// Refuses a single-file export too large to generate, before any work.
/// The remedy is the sentence telling the writer what they can do instead.
pub(crate) fn check_document_size(
    format: PublicationFormat,
    words: usize,
    remedy: &str,
) -> Result<(), String> {
    let kind = match format {
        PublicationFormat::Pdf => "PDF",
        PublicationFormat::Docx => "Word document",
        PublicationFormat::Epub => "EPUB",
        PublicationFormat::Html => "web page",
        PublicationFormat::Site => return Ok(()),
    };
    if words <= MAX_DOCUMENT_WORDS {
        return Ok(());
    }
    Err(format!(
        "This export has {} words, more than one {kind} can hold ({} words). {remedy}",
        thousands(words),
        thousands(MAX_DOCUMENT_WORDS)
    ))
}

fn thousands(value: usize) -> String {
    let digits = value.to_string();
    let mut grouped = String::new();
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    grouped
}

/// Words in a document's text, footnotes and citations included, counted
/// as the page's analytics count them.
pub(crate) fn document_words(node: &Value) -> usize {
    node["text"]
        .as_str()
        .map_or(0, crate::domain::project::assemble::count_words)
        + node["attrs"].get("paragraph").map_or(0, document_words)
        + node["content"]
            .as_array()
            .into_iter()
            .flatten()
            .map(document_words)
            .sum::<usize>()
}

/// The outside-link policy a format's target sets.
pub(crate) const fn outside_policy(
    targets: &crate::domain::publication::PublicationTargets,
    format: PublicationFormat,
) -> &OutsideLinks {
    match format {
        PublicationFormat::Pdf => &targets.pdf.outside_links,
        PublicationFormat::Docx => &targets.docx.outside_links,
        PublicationFormat::Epub => &targets.epub.outside_links,
        PublicationFormat::Html => &targets.html.outside_links,
        PublicationFormat::Site => &targets.site.outside_links,
    }
}

pub(crate) fn internal_page_id(href: &str) -> Option<String> {
    let candidate = href
        .strip_prefix("/?note=")
        .or_else(|| href.strip_prefix("?note="))
        .unwrap_or(href);
    is_item_id(candidate).then(|| candidate.to_ascii_lowercase())
}

pub(crate) fn source_context(page_id: &str, block_id: Option<&str>) -> String {
    block_id.map_or_else(
        || format!("Page {page_id}"),
        |id| format!("Page {page_id}, block {id}"),
    )
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PageFacts {
    /// Heading block IDs, the targets of `#block-<id>` links.
    pub(crate) headings: Vec<String>,
    /// Links to a section (`#…`), checked once membership is known.
    pub(crate) anchor_links: Vec<FactLink>,
    /// Every link mark, for the plan's summary.
    pub(crate) link_count: usize,
    /// Image sources in document order, without repeats.
    pub(crate) images: Vec<String>,
    /// Snippet citations and embeds, in document order.
    pub(crate) snippets: Vec<FactLink>,
    /// Problems that stop this page exporting wherever it is exported.
    pub(crate) issues: Vec<FactIssue>,
    /// A field's location among structural issues, for resolving the current header.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) field_uses: Vec<(usize, FactLink)>,
    /// Words in the page, so a linked export's size is known before generation.
    pub(crate) words: usize,
}

/// A link or reference and the block it sits in.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FactLink {
    pub(crate) target: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) block_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub(crate) struct FactIssue {
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub(crate) block_id: Option<String>,
    pub(crate) message: String,
}

/// Collects a page's export facts with the same checks the export scan
/// applies, so a page saved without issues passes them at export too.
#[cfg(test)]
pub(crate) fn page_facts(document: &Value, metadata: &Value) -> PageFacts {
    document_facts(document).with_metadata(metadata)
}

impl PageFacts {
    pub(crate) fn with_metadata(mut self, metadata: &Value) -> Self {
        if self.field_uses.is_empty() {
            return self;
        }
        let mut issues = std::mem::take(&mut self.issues).into_iter();
        let mut emitted = 0;
        for (position, usage) in std::mem::take(&mut self.field_uses) {
            self.issues.extend(issues.by_ref().take(position - emitted));
            emitted = position;
            if let Err(message) = check_field(&usage.target, metadata) {
                self.issues.push(FactIssue {
                    block_id: usage.block_id,
                    message,
                });
            }
        }
        self.issues.extend(issues);
        self
    }
}

/// Facts determined by prose alone. Metadata edits reuse these and the ordered links.
pub(crate) fn document_facts(document: &Value) -> PageFacts {
    fn visit(
        node: &Value,
        inherited_block_id: Option<&str>,
        images: &mut HashSet<String>,
        facts: &mut PageFacts,
    ) {
        let block_id = node["attrs"]["id"].as_str().or(inherited_block_id);
        let owned_block = || block_id.map(str::to_owned);
        let Some(kind) = node["type"].as_str() else {
            facts.issues.push(FactIssue {
                block_id: owned_block(),
                message: "Document node has no type.".to_owned(),
            });
            return;
        };
        if kind == "field"
            && let Some(key) = node["attrs"]["key"].as_str().filter(|key| !key.is_empty())
        {
            facts.field_uses.push((
                facts.issues.len(),
                FactLink {
                    target: key.to_owned(),
                    block_id: owned_block(),
                },
            ));
        } else if let Err(message) = check_node(kind, node, &Value::Null) {
            facts.issues.push(FactIssue {
                block_id: owned_block(),
                message,
            });
        }
        if kind == "heading"
            && let Some(id) = block_id
        {
            facts.headings.push(id.to_owned());
        }
        if kind == "image"
            && let Some(src) = node["attrs"]["src"].as_str()
            && images.insert(src.to_owned())
        {
            facts.images.push(src.to_owned());
        }
        if let Some(id) = snippet_reference(kind, node) {
            facts.snippets.push(FactLink {
                target: id.to_owned(),
                block_id: owned_block(),
            });
        }
        for href in link_hrefs(node) {
            facts.link_count += 1;
            if let Err(message) = check_link(href) {
                facts.issues.push(FactIssue {
                    block_id: owned_block(),
                    message,
                });
            } else if href.starts_with('#') {
                facts.anchor_links.push(FactLink {
                    target: href.to_owned(),
                    block_id: owned_block(),
                });
            }
        }
        if let Some(paragraph) = local_reference(kind, node) {
            visit(paragraph, block_id, images, facts);
        }
        for child in node["content"].as_array().into_iter().flatten() {
            visit(child, block_id, images, facts);
        }
    }
    let mut facts = PageFacts {
        words: document_words(document),
        ..PageFacts::default()
    };
    visit(document, None, &mut HashSet::new(), &mut facts);
    facts
}

fn check_field(key: &str, metadata: &Value) -> Result<(), String> {
    match &metadata[key] {
        Value::String(_) | Value::Number(_) | Value::Bool(_) => Ok(()),
        _ => Err(format!("field {key} has no scalar value.")),
    }
}

/// Checks a node by itself: image alt text, snippet keys, field values and
/// local reference paragraphs. Shared by the export scan and page facts.
pub(crate) fn check_node(kind: &str, node: &Value, metadata: &Value) -> Result<(), String> {
    if kind == "image"
        && node["attrs"]["alt"]
            .as_str()
            .unwrap_or("")
            .trim()
            .is_empty()
    {
        return Err("image alt text must be nonempty.".to_owned());
    }
    if (kind == "embed" || (kind == "citation" && node["attrs"]["source"] == "snippet"))
        && node["attrs"]["snippetId"].as_str().is_none()
    {
        return Err("snippet reference has no key.".to_owned());
    }
    if kind == "field" {
        let key = node["attrs"]["key"]
            .as_str()
            .filter(|key| !key.is_empty())
            .ok_or("field has no key.")?;
        check_field(key, metadata)?;
    }
    if matches!(kind, "citation" | "footnote") && node["attrs"]["source"] != "snippet" {
        crate::domain::project::snippets::validate_reference_paragraph(
            &node["attrs"]["paragraph"],
            false,
        )?;
    }
    Ok(())
}

/// The snippet a citation or embed uses, if it names one.
pub(crate) fn snippet_reference<'a>(kind: &str, node: &'a Value) -> Option<&'a str> {
    (kind == "embed" || (kind == "citation" && node["attrs"]["source"] == "snippet"))
        .then(|| node["attrs"]["snippetId"].as_str())
        .flatten()
}

/// A local citation or footnote's own valid paragraph, scanned like the page.
pub(crate) fn local_reference<'a>(kind: &str, node: &'a Value) -> Option<&'a Value> {
    (matches!(kind, "citation" | "footnote") && node["attrs"]["source"] != "snippet")
        .then(|| &node["attrs"]["paragraph"])
        .filter(|paragraph| {
            crate::domain::project::snippets::validate_reference_paragraph(paragraph, false).is_ok()
        })
}

pub(crate) fn link_hrefs(node: &Value) -> impl Iterator<Item = &str> {
    node["marks"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|mark| mark["type"] == "link")
        .map(|mark| mark["attrs"]["href"].as_str().unwrap_or(""))
}

/// External destinations do not depend on graph membership, so a link that is
/// neither a page, a section nor a valid web address fails on its own.
pub(crate) fn check_link(href: &str) -> Result<(), String> {
    if internal_page_id(href).is_none() && !href.starts_with('#') && !valid_external_url(href) {
        return Err(format!("link cannot be exported: {href}."));
    }
    Ok(())
}
