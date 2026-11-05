// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use crate::domain::document::links::export_url;
use crate::domain::project::{asset::valid_filename, identity::is_item_id};
use crate::domain::publication::{
    OutsideLinks, PublicationContent,
    plan::{PlannedPage, PublicationFormat, PublicationPlan},
    preflight::*,
};
use crate::store::snapshots::{read_project_item, read_snippet};
use rayon::prelude::*;
use serde_json::Value;
use std::{
    collections::{HashMap, HashSet},
    path::Path,
};

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
    let result = preflight_pages(
        root,
        &plan.pages,
        outside_policy(&content.targets, plan.format),
    )?;
    check_document_size(
        plan.format,
        result.words,
        "Choose fewer pages, or generate a website instead.",
    )?;
    if matches!(plan.format, PublicationFormat::Site) && content.targets.site.pdf_download {
        check_document_size(
            PublicationFormat::Pdf,
            result.words,
            "Turn off the PDF download, or choose fewer pages.",
        )?;
    }
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
    let mut words = 0;
    for page in pages {
        let item = read_project_item(root, &page.id)
            .map_err(|error| error.to_string())?
            .ok_or_else(|| format!("Publication page {} disappeared during planning.", page.id))?;
        words += document_words(crate::domain::publication::source::document(&item));
        preflight.scan(root, page, &item)?;
    }
    Ok(PreflightResult {
        words,
        ..preflight.finish(root, pages, policy)?
    })
}

/// Collect checks as pages are discovered, retaining only export metadata.
/// Cross-page links and snippets are resolved once membership is complete.
#[derive(Default)]
pub(crate) struct PagePreflight {
    anchors: HashMap<String, String>,
    assets: AssetScan,
    pending_links: Vec<PendingLink>,
    pending_references: Vec<PendingReference>,
}

/// Images seen by the scan, in order, and any hashed ahead of it.
#[derive(Default)]
struct AssetScan {
    seen: HashSet<String>,
    assets: Vec<PlannedAsset>,
    /// Hashes computed while the page was read on another thread.
    prepared: HashMap<String, PlannedAsset>,
}

/// Hashes a page's images ahead of its scan, so a reader thread can do this
/// I/O. An image that cannot be read is left for the scan to report.
pub(crate) fn prepare_page_assets(root: &Path, item: &Value) -> Vec<PlannedAsset> {
    fn sources<'a>(node: &'a Value, output: &mut Vec<&'a str>) {
        if node["type"] == "image"
            && let Some(src) = node["attrs"]["src"].as_str()
        {
            output.push(src);
        }
        if node["attrs"]["paragraph"].is_object() {
            sources(&node["attrs"]["paragraph"], output);
        }
        if let Some(children) = node["content"].as_array() {
            for child in children {
                sources(child, output);
            }
        }
    }
    let mut found = Vec::new();
    sources(
        crate::domain::publication::source::document(item),
        &mut found,
    );
    found.sort_unstable();
    found.dedup();
    found
        .into_iter()
        .filter_map(|src| read_asset(root, src).ok())
        .collect()
}

impl PagePreflight {
    /// Offers image hashes computed ahead of the scan; the scan uses each once.
    pub(crate) fn prepare(&mut self, assets: Vec<PlannedAsset>) {
        for asset in assets {
            if !self.assets.seen.contains(&asset.src) {
                self.assets.prepared.insert(asset.src.clone(), asset);
            }
        }
    }

    pub(crate) fn scan(
        &mut self,
        root: &Path,
        page: &PlannedPage,
        item: &Value,
    ) -> Result<(), String> {
        let document = crate::domain::publication::source::document(item);
        self.anchors
            .insert(format!("note-{}", page.id), page.id.clone());
        scan_node(
            root,
            document,
            &item["meta"],
            page,
            None,
            &mut self.anchors,
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
            assets: AssetScan { assets, .. },
            pending_links,
            pending_references,
            ..
        } = self;
        let mut pending_links = pending_links;
        let mut result = PreflightResult {
            assets,
            links: Vec::new(),
            snippets: Vec::new(),
            words: 0,
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
            } else if let Some(url) = export_url(&link.href) {
                (LinkResolution::External, None, Some(url))
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

/// What export needs from one page, gathered when the page is saved so
/// planning can answer without reading the document again.
#[expect(
    clippy::too_many_arguments,
    reason = "Keep the explicit publication context together at the planning/rendering boundary."
)]
fn scan_node(
    root: &Path,
    node: &Value,
    metadata: &Value,
    page: &PlannedPage,
    inherited_block_id: Option<&str>,
    anchors: &mut HashMap<String, String>,
    assets: &mut AssetScan,
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
    check_node(kind, node, metadata).map_err(|error| format!("{context}: {error}"))?;
    if kind == "image" {
        let src = node["attrs"]["src"].as_str().unwrap_or("");
        if assets.seen.insert(src.to_owned()) {
            let asset = match assets.prepared.remove(src) {
                Some(asset) => asset,
                None => read_asset(root, src).map_err(|error| format!("{context}: {error}"))?,
            };
            assets.assets.push(asset);
        }
    }
    if let Some(id) = snippet_reference(kind, node) {
        references.push(PendingReference {
            id: id.to_owned(),
            page_id: page.id.clone(),
            block_id: block_id.map(str::to_owned),
        });
    }
    if let Some(paragraph) = local_reference(kind, node) {
        scan_node(
            root, paragraph, metadata, page, block_id, anchors, assets, links, references,
        )?;
    }
    for href in link_hrefs(node) {
        // Reject a bad destination during discovery, before reading the rest
        // of a large graph. Anchors still need the completed scan.
        check_link(href).map_err(|error| format!("{context}: {error}"))?;
        links.push(PendingLink {
            page_id: page.id.clone(),
            block_id: block_id.map(str::to_owned),
            href: href.to_owned(),
        });
    }
    if let Some(children) = node["content"].as_array() {
        for child in children {
            scan_node(
                root, child, metadata, page, block_id, anchors, assets, links, references,
            )?;
        }
    }
    Ok(())
}

pub(crate) fn assert_dependencies_fresh(root: &Path, plan: &PublicationPlan) -> Result<(), String> {
    assert_preflight_fresh(root, &plan.assets, &plan.snippets)
}

pub(crate) fn assert_preflight_fresh(
    root: &Path,
    assets: &[PlannedAsset],
    snippets: &[PlannedSnippet],
) -> Result<(), String> {
    crate::export_jobs::parallel::exports(|| {
        assets.par_iter().try_for_each(|asset| {
            if asset.stamp.is_some() && asset_stamp(root, &asset.src)? == asset.stamp {
                return Ok(());
            }
            let current = read_asset(root, &asset.src)?;
            if current.sha256 != asset.sha256 || current.bytes != asset.bytes {
                return Err(format!(
                    "Asset {} changed after Publication planning.",
                    asset.src
                ));
            }
            Ok(())
        })
    })?;
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

/// An image source's owner, file name and path inside the project.
fn asset_mapping(root: &Path, src: &str) -> Result<Option<crate::store::assets::Object>, String> {
    let (owner, name) = asset_parts(src)?;
    let path = crate::store::assets::owner_path(owner, name).map_err(|error| error.to_string())?;
    crate::store::Store::shared(root)
        .and_then(|store| store.read(|tx| crate::store::assets::mapping(tx, &path)))
        .map_err(|error| error.to_string())
}

fn asset_parts(src: &str) -> Result<(&str, &str), String> {
    let mut parts = src
        .strip_prefix("/assets/")
        .ok_or_else(|| format!("Image source is not a project asset: {src}"))?
        .split('/');
    match (parts.next(), parts.next(), parts.next()) {
        (Some(owner), Some(name), None) if is_item_id(owner) && valid_filename(name) => {
            Ok((owner, name))
        }
        _ => Err(format!("Invalid asset URL: {src}")),
    }
}

fn asset_stamp(root: &Path, src: &str) -> Result<Option<String>, String> {
    Ok(asset_mapping(root, src)?.map(|object| object.oid))
}

fn read_asset(root: &Path, src: &str) -> Result<PlannedAsset, String> {
    let (owner, filename) = asset_parts(src)?;
    let object =
        asset_mapping(root, src)?.ok_or_else(|| format!("Image asset is missing: {src}"))?;
    object.available(root).map_err(|error| error.to_string())?;
    Ok(PlannedAsset {
        src: src.to_owned(),
        owner_id: owner.to_owned(),
        filename: filename.to_owned(),
        bytes: object.size,
        stamp: Some(object.oid.clone()),
        sha256: object.oid,
    })
}

#[cfg(test)]
mod facts_tests {
    use serde_json::json;

    use super::{FactIssue, FactLink, page_facts};

    const PAGE: &str = "01a0e2a0-0000-7000-8000-000000000002";

    #[test]
    fn page_facts_record_what_export_needs_and_every_standalone_issue() {
        let link = |href: &str| json!({"type":"text","text":"x","marks":[{"type":"link","attrs":{"href":href}}]});
        let document = json!({"type":"doc","content":[
            {"type":"heading","attrs":{"id":"h1","level":1},"content":[{"type":"text","text":"Title"}]},
            {"type":"paragraph","attrs":{"id":"p1"},"content":[
                link(&format!("/?note={PAGE}")),
                link("#block-h1"),
                link("https://example.com/"),
                link("not a link"),
                {"type":"citation","attrs":{"source":"snippet","snippetId":"shared"}},
                {"type":"footnote","attrs":{"paragraph":{"type":"paragraph","content":[link("#block-h1")]}}},
                {"type":"field","attrs":{"key":"missing"}}
            ]},
            {"type":"image","attrs":{"id":"i1","src":"/assets/x/a.png","alt":""}},
            {"type":"image","attrs":{"id":"i2","src":"/assets/x/a.png","alt":"Again"}},
            {"type":"embed","attrs":{"id":"e1","snippetId":"other"}}
        ]});
        let facts = page_facts(&document, &json!({"title":"Page"}));
        assert_eq!(facts.headings, ["h1"]);
        assert_eq!(facts.link_count, 5);
        let anchor = FactLink {
            target: "#block-h1".into(),
            block_id: Some("p1".into()),
        };
        assert_eq!(facts.anchor_links, [anchor.clone(), anchor]);
        assert_eq!(facts.images, ["/assets/x/a.png"]);
        assert_eq!(
            facts
                .snippets
                .iter()
                .map(|link| link.target.as_str())
                .collect::<Vec<_>>(),
            ["shared", "other"]
        );
        assert_eq!(
            facts.issues,
            [
                FactIssue {
                    block_id: Some("p1".into()),
                    message: "link cannot be exported: not a link.".into()
                },
                FactIssue {
                    block_id: Some("p1".into()),
                    message: "field missing has no scalar value.".into()
                },
                FactIssue {
                    block_id: Some("i1".into()),
                    message: "image alt text must be nonempty.".into()
                },
            ]
        );
    }

    #[test]
    fn a_clean_page_has_no_issues() {
        let document = json!({"type":"doc","content":[{"type":"paragraph","content":[
            {"type":"text","text":"Fine","marks":[{"type":"link","attrs":{"href":"https://example.com/"}}]}
        ]}]});
        let facts = page_facts(&document, &json!({}));
        assert_eq!(facts.issues, Vec::new());
        assert_eq!(facts.link_count, 1);
        assert_eq!(facts.words, 1);
    }

    #[test]
    fn words_count_footnotes_and_citations_too() {
        let document = json!({"type":"doc","content":[{"type":"paragraph","content":[
            {"type":"text","text":"Three plain words "},
            {"type":"footnote","attrs":{"paragraph":{"type":"paragraph","content":[
                {"type":"text","text":"a footnote's two"}
            ]}}}
        ]}]});
        assert_eq!(super::document_words(&document), 6);
    }

    #[test]
    fn single_file_exports_stop_at_the_word_limit_and_websites_do_not() {
        use super::{MAX_DOCUMENT_WORDS, check_document_size};
        use crate::domain::publication::plan::PublicationFormat;

        let remedy = "Choose fewer pages.";
        assert!(check_document_size(PublicationFormat::Pdf, MAX_DOCUMENT_WORDS, remedy).is_ok());
        assert_eq!(
            check_document_size(PublicationFormat::Docx, 2_345_678, remedy).unwrap_err(),
            "This export has 2,345,678 words, more than one Word document can hold (2,000,000 words). \
             Choose fewer pages."
        );
        assert!(check_document_size(PublicationFormat::Site, usize::MAX, remedy).is_ok());
    }
}
