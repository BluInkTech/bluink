// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! A clean editor submission reuses its exact base manifest. The shared merge
//! path handles stale bases and edits requiring normalization or group repair.
mod install;
#[cfg(test)]
mod tests;

use super::{
    Error, Result,
    block_facts::{self, Facts, Page},
    body::{self, PreparedBlock},
    item, normalize,
};
use crate::domain::project::assemble;
use crate::records::PassageRecord;
use crate::records::drafts::{DocumentPatch, EditorDraft, MergeDraftRequest};
use rusqlite::Transaction;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

pub(crate) struct Prepared {
    id: String,
    base: String,
    device: String,
    submission: String,
    hash: String,
    header: Value,
    memberships: Value,
    body: BodyChange,
}

enum BodyChange {
    Keep,
    Fields { outer: String, order: Vec<String> },
    Blocks(Box<BodyEdit>),
}

struct BodyEdit {
    order: Vec<String>,
    outer: String,
    changed: BTreeMap<String, PreparedBlock>,
    page: Page,
    passages: BTreeMap<String, PassageRecord>,
    fts: Option<String>,
    edges_changed: bool,
    /// Snippet keys absent from the base. Base references are guarded by the
    /// snippet's own archive and delete checks, so only these need the writer.
    added_snippets: BTreeSet<String>,
}

pub(crate) struct Base {
    header: item::Header,
    memberships: Value,
    outer: Value,
    order: Vec<String>,
    facts: BTreeMap<String, Facts>,
}

/// Validation and block preparation run outside any transaction; the writer
/// rechecks the base revision before installing the result.
pub(crate) fn prepare(
    base: Option<Base>,
    request: &MergeDraftRequest,
    timezone: &str,
    request_hash: &str,
) -> Result<Option<Prepared>> {
    let EditorDraft::DocumentPatch {
        meta,
        tags,
        pins,
        content,
        document,
    } = request.draft.as_ref()
    else {
        return Ok(None);
    };
    if document.order.is_empty() {
        return Err(Error::Invalid(
            "A document needs at least one block.".into(),
        ));
    }
    if content.contains_key("document") {
        return Err(Error::Invalid(
            "Document patch content cannot contain document.".into(),
        ));
    }
    if document
        .attrs
        .as_ref()
        .is_some_and(|attrs| !attrs.is_empty())
    {
        return Ok(None);
    }
    let Some(base) = base else {
        return Ok(None);
    };
    let mut header = base.header.value.clone();
    let metadata = serde_json::to_value(meta)?;
    if header["meta"]["type"] != metadata["type"]
        || header["meta"]["state"] != metadata["state"]
        || header["meta"]
            .as_object()
            .is_some_and(|old| old.keys().any(|key| metadata.get(key).is_none()))
    {
        return Ok(None);
    }
    header["meta"] = metadata;
    crate::domain::project::item::validate_metadata(
        header["meta"]
            .as_object()
            .ok_or_else(|| Error::Invalid("Draft metadata must be an object.".into()))?,
        &request.id,
    )
    .map_err(Error::Invalid)?;
    let Some(memberships) =
        submitted_memberships(&base.memberships, tags.as_deref(), pins.as_deref())?
    else {
        return Ok(None);
    };
    let mut outer = serde_json::to_value(content)?;
    outer["document"] = json!({"type":"doc"});
    if let Some(attrs) = &document.attrs {
        outer["document"]["attrs"] = json!(attrs);
    }
    crate::domain::project::item::validate_page_fields(&outer, &request.id)
        .map_err(Error::Invalid)?;
    if header["meta"]["type"] == "template" {
        crate::domain::project::template::validate(
            &json!({"meta":header["meta"],"content":outer}),
            &request.id,
        )
        .map_err(Error::Invalid)?;
    }
    header["revision"] = json!(uuid::Uuid::now_v7().to_string());
    header["modifiedAt"] =
        json!(chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true));
    header["analytics"]["timezone"] = json!(timezone);
    let body = if document.blocks.is_empty() && base.order == document.order {
        if base.outer == outer {
            BodyChange::Keep
        } else {
            BodyChange::Fields {
                outer: serde_json::to_string(&outer)?,
                order: document.order.clone(),
            }
        }
    } else {
        let Some(body) = prepare_body(&base, document, &outer, &mut header)? else {
            return Ok(None);
        };
        BodyChange::Blocks(Box::new(body))
    };
    Ok(Some(Prepared {
        id: request.id.clone(),
        base: request.base_revision.clone(),
        device: request.device.clone(),
        submission: request.submission_id.clone(),
        hash: request_hash.to_owned(),
        header,
        memberships,
        body,
    }))
}

fn prepare_body(
    base: &Base,
    document: &DocumentPatch,
    outer: &Value,
    header: &mut Value,
) -> Result<Option<BodyEdit>> {
    let mut page = Page::default();
    let mut changed = BTreeMap::new();
    let mut passages = BTreeMap::new();
    let mut seen = BTreeSet::new();
    let mut text_changed = base.order != document.order;
    let mut edges_changed = false;
    for key in &document.order {
        if key.is_empty() || !seen.insert(key) {
            return Err(Error::Invalid("Duplicate or empty patch block ID.".into()));
        }
        if let Some(node) = document.blocks.get(key) {
            if node.0["attrs"]["id"].as_str() != Some(key) {
                return Err(Error::Invalid(
                    "Document patch ID differs from its key.".into(),
                ));
            }
            let content = json!({"document":{"type":"doc","content":[node.0]}});
            if crate::domain::document::validation::validate_canonical_document(
                &content["document"],
            )
            .is_err()
            {
                return Ok(None);
            }
            if header["meta"]["type"] == "template" {
                crate::domain::project::template::validate(
                    &json!({"meta":header["meta"],"content":content}),
                    header["id"].as_str().unwrap_or_default(),
                )
                .map_err(Error::Invalid)?;
            }
            let mut block = PreparedBlock::new(&node.0)?;
            text_changed |= base
                .facts
                .get(key)
                .is_none_or(|old| old.prose.text != block.facts.prose.text);
            edges_changed |= !same_edges(base.facts.get(key), Some(&block.facts));
            if header["meta"]["type"] == "page" {
                for record in std::mem::take(&mut block.facts.passages) {
                    passages.insert(record.block_id.clone(), record);
                }
            }
            if !page.append(&block.facts)? {
                return Ok(None);
            }
            changed.insert(key.clone(), block);
        } else {
            let old = base.facts.get(key).ok_or_else(|| {
                Error::Invalid("Document patch references an unknown block.".into())
            })?;
            if !page.append(old)? {
                return Ok(None);
            }
        }
    }
    if changed.len() != document.blocks.len() {
        return Err(Error::Invalid(
            "Document patch contains an unused block.".into(),
        ));
    }
    edges_changed |= base
        .facts
        .iter()
        .any(|(key, facts)| !seen.contains(key) && !same_edges(Some(facts), None));
    page.finish();
    let previous_snippets: BTreeSet<_> = base
        .facts
        .values()
        .flat_map(|facts| {
            facts
                .prose
                .snippet_citations
                .iter()
                .chain(&facts.prose.snippet_embeds)
        })
        .collect();
    let added_snippets = page
        .prose
        .snippet_citations
        .iter()
        .chain(&page.prose.snippet_embeds)
        .filter(|key| !previous_snippets.contains(key))
        .cloned()
        .collect();
    let text = &page.prose.text;
    header["analytics"]["current"] =
        json!({"contentHash":assemble::hash_text(text),"wordCount":assemble::count_words(text)});
    Ok(Some(BodyEdit {
        order: document.order.clone(),
        outer: serde_json::to_string(outer)?,
        fts: text_changed.then(|| normalize::fts_text(text)),
        changed,
        page,
        passages,
        edges_changed,
        added_snippets,
    }))
}

fn same_edges(left: Option<&Facts>, right: Option<&Facts>) -> bool {
    fn edges(facts: Option<&Facts>) -> [&[String]; 4] {
        facts.map_or([&[][..]; 4], |facts| {
            [
                &facts.prose.labels,
                &facts.prose.page_links,
                &facts.prose.snippet_embeds,
                &facts.prose.snippet_citations,
            ]
        })
    }
    edges(left) == edges(right)
}

/// Read the exact base a document patch may reuse. Callers share this snapshot
/// with their other pre-writer reads.
pub(crate) fn read_base(tx: &Transaction<'_>, request: &MergeDraftRequest) -> Result<Option<Base>> {
    let EditorDraft::DocumentPatch { document, .. } = request.draft.as_ref() else {
        return Ok(None);
    };
    if document
        .attrs
        .as_ref()
        .is_some_and(|attrs| !attrs.is_empty())
    {
        return Ok(None);
    }
    let header = item::read_header(tx, &request.id)?.ok_or(Error::NotFound)?;
    if header.value["revision"] != request.base_revision
        || !matches!(
            header.value["meta"]["type"].as_str(),
            Some("page" | "template")
        )
    {
        return Ok(None);
    }
    let layout = body::layout(tx, header.body)?;
    let order = layout.keys;
    let outer = serde_json::from_str(&layout.outer)?;
    let facts = if !document.blocks.is_empty() || order != document.order {
        block_facts::read(tx, &layout.ids)?
    } else {
        BTreeMap::new()
    };
    let memberships = item::memberships(tx, header.id)?;
    Ok(Some(Base {
        header,
        memberships,
        outer,
        order,
        facts,
    }))
}

fn submitted_memberships(
    current: &Value,
    tags: Option<&[String]>,
    pins: Option<&[String]>,
) -> Result<Option<Value>> {
    let mut memberships = json!({});
    for (key, values) in [("tags", tags), ("pins", pins)] {
        if let Some(values) = values {
            if values.is_empty() || values.windows(2).any(|pair| pair[0] >= pair[1]) {
                return Ok(None);
            }
            memberships[key] = json!(values);
        } else if current.get(key).is_some() {
            return Ok(None);
        }
    }
    crate::domain::project::item::validate_memberships(&memberships).map_err(Error::Invalid)?;
    Ok(Some(memberships))
}
