// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::engine::document::links::valid_external_url;
use crate::engine::project::asset::valid_filename;
use crate::engine::project::item::{is_item_id, read_project_item};
use crate::engine::project::snippets::read_snippet;
use crate::engine::publication::plan::{PlannedPage, PublicationFormat, PublicationPlan};
use crate::engine::publication::{OutsideLinks, PublicationContent};

#[derive(Debug, Deserialize, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub(crate) struct PlannedAsset {
    pub(crate) src: String,
    pub(crate) owner_id: String,
    pub(crate) filename: String,
    pub(crate) bytes: u64,
    pub(crate) sha256: String,
}

#[derive(Debug, Deserialize, Serialize)]
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

#[derive(Debug, Deserialize, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub(crate) struct PlannedSnippet {
    pub(crate) id: String,
    pub(crate) revision: String,
    pub(crate) paragraph: Value,
}

#[derive(Debug, Deserialize, Serialize)]
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
}

struct PendingLink {
    page_id: String,
    block_id: Option<String>,
    href: String,
}

struct PendingReference {
    id: String,
    page_id: String,
    block_id: Option<String>,
}

pub(crate) fn preflight_publication(
    root: &Path,
    plan: &mut PublicationPlan,
    content: &PublicationContent,
) -> Result<(), String> {
    let policy = match plan.format {
        PublicationFormat::Pdf => &content.targets.pdf.outside_links,
        PublicationFormat::Html => &content.targets.html.outside_links,
        PublicationFormat::Site => &content.targets.site.outside_links,
    };
    let result = preflight_pages(root, &plan.pages, policy)?;
    plan.assets = result.assets;
    plan.links = result.links;
    plan.snippets = result.snippets;
    Ok(())
}

pub(crate) fn preflight_pages(
    root: &Path,
    pages: &[PlannedPage],
    policy: &OutsideLinks,
) -> Result<PreflightResult, String> {
    let mut preflight = PagePreflight::default();
    for page in pages {
        let item = read_project_item(root, &page.id)?
            .ok_or_else(|| format!("Publication page {} disappeared during planning.", page.id))?;
        preflight.scan(root, page, &item)?;
    }
    preflight.finish(root, pages, policy)
}

/// Collect checks as pages are discovered, retaining only export metadata.
/// Cross-page links and snippets are resolved once membership is complete.
#[derive(Default)]
pub(crate) struct PagePreflight {
    anchors: HashMap<String, String>,
    assets_seen: HashSet<String>,
    assets: Vec<PlannedAsset>,
    pending_links: Vec<PendingLink>,
    pending_references: Vec<PendingReference>,
}

impl PagePreflight {
    pub(crate) fn scan(
        &mut self,
        root: &Path,
        page: &PlannedPage,
        item: &Value,
    ) -> Result<(), String> {
        let document = item["content"]
            .get("content")
            .filter(|value| value.is_object())
            .unwrap_or_else(|| &item["content"]);
        self.anchors
            .insert(format!("note-{}", page.id), page.id.clone());
        scan_node(
            root,
            document,
            page,
            None,
            &mut self.anchors,
            &mut self.assets_seen,
            &mut self.assets,
            &mut self.pending_links,
            &mut self.pending_references,
        )
    }

    pub(crate) fn finish(
        self,
        root: &Path,
        pages: &[PlannedPage],
        policy: &OutsideLinks,
    ) -> Result<PreflightResult, String> {
        let Self {
            anchors,
            assets,
            pending_links,
            pending_references,
            ..
        } = self;
        let mut pending_links = pending_links;
        let mut result = PreflightResult {
            assets,
            links: Vec::new(),
            snippets: Vec::new(),
        };
        let page_ids: HashSet<&str> = pages.iter().map(|page| page.id.as_str()).collect();
        let mut captured = HashSet::new();
        for reference in pending_references {
            let context = source_context(&reference.page_id, reference.block_id.as_deref());
            let record = if captured.insert(reference.id.clone()) {
                let record = read_snippet(root, &reference.id)?
                    .ok_or_else(|| format!("{context}: snippet {} is unresolved.", reference.id))?;
                if record["state"] != "active" {
                    return Err(format!("{context}: snippet {} is archived.", reference.id));
                }
                result.snippets.push(PlannedSnippet {
                    id: reference.id.clone(),
                    revision: record["revision"]
                        .as_str()
                        .ok_or("Snippet has no revision.")?
                        .to_owned(),
                    paragraph: record["paragraph"].clone(),
                });
                record["paragraph"].clone()
            } else {
                result
                    .snippets
                    .iter()
                    .find(|entry| entry.id == reference.id)
                    .ok_or("Snippet snapshot is missing.")?
                    .paragraph
                    .clone()
            };
            for child in record["content"].as_array().into_iter().flatten() {
                for mark in child["marks"].as_array().into_iter().flatten() {
                    if mark["type"] == "link" {
                        pending_links.push(PendingLink {
                            page_id: reference.page_id.clone(),
                            block_id: reference.block_id.clone(),
                            href: mark["attrs"]["href"]
                                .as_str()
                                .ok_or("Snippet link has no destination.")?
                                .to_owned(),
                        });
                    }
                }
            }
        }
        for link in pending_links {
            let context = source_context(&link.page_id, link.block_id.as_deref());
            let target_id = internal_page_id(&link.href);
            let (resolution, target_id, destination) = if let Some(id) = target_id {
                if page_ids.contains(id.as_str()) {
                    (LinkResolution::Included, Some(id), None)
                } else {
                    match policy {
                        OutsideLinks::Error => {
                            return Err(format!(
                                "{context}: linked page {id} is outside this Publication."
                            ));
                        }
                        // The destination is omitted from the output, so it is not
                        // an export dependency and may be missing from the project.
                        OutsideLinks::TextOnly => (LinkResolution::TextOnly, Some(id), None),
                    }
                }
            } else if let Some(anchor) = link.href.strip_prefix('#') {
                let owner = anchors.get(anchor).ok_or_else(|| {
                    format!("{context}: linked section #{anchor} is absent or excluded.")
                })?;
                (
                    LinkResolution::Anchor,
                    Some(owner.clone()),
                    Some(link.href.clone()),
                )
            } else if valid_external_url(&link.href) {
                (LinkResolution::External, None, Some(link.href.clone()))
            } else {
                return Err(format!(
                    "{context}: link cannot be exported: {}.",
                    link.href
                ));
            };
            result.links.push(PlannedLink {
                page_id: link.page_id,
                block_id: link.block_id,
                href: link.href,
                resolution,
                target_id,
                destination,
            });
        }
        Ok(result)
    }
}

#[allow(clippy::too_many_arguments)]
#[expect(
    clippy::too_many_lines,
    reason = "Keep the complete format or operation dispatch together for contract review."
)]
fn scan_node(
    root: &Path,
    node: &Value,
    page: &PlannedPage,
    inherited_block_id: Option<&str>,
    anchors: &mut HashMap<String, String>,
    assets_seen: &mut HashSet<String>,
    assets: &mut Vec<PlannedAsset>,
    links: &mut Vec<PendingLink>,
    references: &mut Vec<PendingReference>,
) -> Result<(), String> {
    let kind = node["type"].as_str().ok_or("Document node has no type.")?;
    let block_id = node["attrs"]["id"].as_str().or(inherited_block_id);
    if node["attrs"]["id"].as_str().is_some_and(|id| {
        page.excluded_block_ids
            .iter()
            .any(|excluded| excluded == id)
    }) {
        return Ok(());
    }
    let context = source_context(&page.id, block_id);
    if kind == "heading"
        && let Some(id) = block_id
        && anchors
            .insert(format!("block-{id}"), page.id.clone())
            .is_some_and(|owner| owner != page.id)
    {
        return Err(format!(
            "{context}: heading block ID {id} also belongs to another page."
        ));
    }
    if kind == "image" {
        let alt = node["attrs"]["alt"].as_str().unwrap_or("");
        if alt.trim().is_empty() {
            return Err(format!("{context}: image alt text must be nonempty."));
        }
        let src = node["attrs"]["src"].as_str().unwrap_or("");
        if assets_seen.insert(src.to_owned()) {
            assets.push(read_asset(root, src).map_err(|error| format!("{context}: {error}"))?);
        }
    }
    if kind == "embed" || (kind == "citation" && node["attrs"]["source"] == "snippet") {
        let id = node["attrs"]["snippetId"]
            .as_str()
            .ok_or_else(|| format!("{context}: snippet reference has no key."))?;
        references.push(PendingReference {
            id: id.to_owned(),
            page_id: page.id.clone(),
            block_id: block_id.map(str::to_owned),
        });
    }
    if kind == "field" {
        let key = node["attrs"]["key"]
            .as_str()
            .filter(|key| !key.is_empty())
            .ok_or_else(|| format!("{context}: field has no key."))?;
        let item = read_project_item(root, &page.id)?
            .ok_or_else(|| format!("{context}: field source page is missing."))?;
        let value = &item["meta"][key];
        if !value.is_string() && !value.is_number() && !value.is_boolean() {
            return Err(format!("{context}: field {key} has no scalar value."));
        }
    }
    if matches!(kind, "citation" | "footnote") && node["attrs"]["source"] != "snippet" {
        let paragraph = &node["attrs"]["paragraph"];
        crate::engine::project::snippets::validate_reference_paragraph(paragraph, false)
            .map_err(|error| format!("{context}: {error}"))?;
        scan_node(
            root,
            paragraph,
            page,
            block_id,
            anchors,
            assets_seen,
            assets,
            links,
            references,
        )?;
    }
    if let Some(marks) = node["marks"].as_array() {
        for mark in marks {
            if mark["type"] == "link" {
                let href = mark["attrs"]["href"].as_str().unwrap_or("");
                // External destinations do not depend on graph membership.
                // Reject them during discovery instead of after reading every
                // reachable document. Anchors still need the completed scan.
                if internal_page_id(href).is_none()
                    && !href.starts_with('#')
                    && !valid_external_url(href)
                {
                    return Err(format!("{context}: link cannot be exported: {href}."));
                }
                links.push(PendingLink {
                    page_id: page.id.clone(),
                    block_id: block_id.map(str::to_owned),
                    href: href.to_owned(),
                });
            }
        }
    }
    if let Some(children) = node["content"].as_array() {
        for child in children {
            scan_node(
                root,
                child,
                page,
                block_id,
                anchors,
                assets_seen,
                assets,
                links,
                references,
            )?;
        }
    }
    Ok(())
}

pub(crate) fn assert_dependencies_fresh(root: &Path, plan: &PublicationPlan) -> Result<(), String> {
    assert_preflight_fresh(root, &plan.assets, &plan.links, &plan.snippets)
}

pub(crate) fn assert_preflight_fresh(
    root: &Path,
    assets: &[PlannedAsset],
    _links: &[PlannedLink],
    snippets: &[PlannedSnippet],
) -> Result<(), String> {
    for asset in assets {
        let current = read_asset(root, &asset.src)?;
        if current.sha256 != asset.sha256 || current.bytes != asset.bytes {
            return Err(format!(
                "Asset {} changed after Publication planning.",
                asset.src
            ));
        }
    }
    for snippet in snippets {
        let current = read_snippet(root, &snippet.id)?;
        if current.as_ref().is_none_or(|record| {
            record["revision"].as_str() != Some(&snippet.revision)
                || record["paragraph"] != snippet.paragraph
        }) {
            return Err(format!(
                "Snippet {} changed or disappeared after Publication planning.",
                snippet.id
            ));
        }
    }
    Ok(())
}

fn read_asset(root: &Path, src: &str) -> Result<PlannedAsset, String> {
    let mut segments = src.split('/');
    if segments.next() != Some("") || segments.next() != Some("assets") {
        return Err(format!("Image source is not a project asset: {src}"));
    }
    let owner_id = segments
        .next()
        .ok_or_else(|| format!("Invalid asset URL: {src}"))?;
    let filename = segments
        .next()
        .ok_or_else(|| format!("Invalid asset URL: {src}"))?;
    if segments.next().is_some() || !is_item_id(owner_id) || !valid_filename(filename) {
        return Err(format!("Invalid asset URL: {src}"));
    }
    let path = root
        .join("items")
        .join(&owner_id[..2])
        .join(owner_id)
        .join("assets")
        .join(filename);
    reject_symlinks(root, &path)?;
    let mut file = File::open(&path).map_err(|_| format!("Image asset is missing: {src}"))?;
    let metadata = file
        .metadata()
        .map_err(|error| format!("Cannot inspect asset {src}: {error}"))?;
    if !metadata.is_file() {
        return Err(format!("Image asset is not a file: {src}"));
    }
    let mut digest = Sha256::new();
    let mut buffer = vec![0_u8; 65_536];
    let mut bytes = 0;
    loop {
        let count = file
            .read(&mut buffer)
            .map_err(|error| format!("Cannot read asset {src}: {error}"))?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
        bytes += count as u64;
    }
    Ok(PlannedAsset {
        src: src.to_owned(),
        owner_id: owner_id.to_owned(),
        filename: filename.to_owned(),
        bytes,
        sha256: super::digest_hex(&digest.finalize()),
    })
}

fn reject_symlinks(root: &Path, path: &Path) -> Result<(), String> {
    let relative = path
        .strip_prefix(root)
        .map_err(|_| "Asset path leaves the project root.")?;
    let mut part = PathBuf::from(root);
    for component in relative.components() {
        part.push(component);
        let metadata = fs::symlink_metadata(&part)
            .map_err(|_| format!("Missing asset path: {}", part.display()))?;
        if metadata.file_type().is_symlink() {
            return Err(format!(
                "Asset paths cannot use symbolic links: {}",
                part.display()
            ));
        }
    }
    Ok(())
}

pub(crate) fn internal_page_id(href: &str) -> Option<String> {
    let candidate = href
        .strip_prefix("/?note=")
        .or_else(|| href.strip_prefix("?note="))
        .unwrap_or(href);
    is_item_id(candidate).then(|| candidate.to_ascii_lowercase())
}

fn source_context(page_id: &str, block_id: Option<&str>) -> String {
    block_id.map_or_else(
        || format!("Page {page_id}"),
        |id| format!("Page {page_id}, block {id}"),
    )
}
