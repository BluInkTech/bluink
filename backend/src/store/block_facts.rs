// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use super::{Error, Result};
use crate::domain::document::{
    content::is_addressable_block,
    links::validate_http_links,
    projection::{DocumentProjection, project_document},
};
use crate::domain::publication::preflight::{PageFacts, document_facts};
use crate::records::PassageRecord;
use rusqlite::Transaction;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Deserialize, Serialize)]
pub(super) struct Facts {
    pub(crate) ids: Vec<String>,
    pub(crate) prose: DocumentProjection,
    pub(crate) export: PageFacts,
    pub(crate) passage_ids: Vec<String>,
    pub(crate) link_error: Option<String>,
    /// Records of a block prepared in this process; stored facts keep only IDs.
    #[serde(skip)]
    pub(crate) passages: Vec<PassageRecord>,
}

impl Facts {
    pub(crate) fn new(block: &Value) -> Self {
        let mut ids = Vec::new();
        collect_ids(block, &mut ids);
        let prose = project_document(block);
        let passages = if prose.labels.is_empty() {
            Vec::new()
        } else {
            crate::store::projections::passages::project("", "", "", &json!({"document":block}))
        };
        Self {
            ids,
            prose,
            export: document_facts(block),
            passage_ids: passages
                .iter()
                .map(|record| record.block_id.clone())
                .collect(),
            link_error: validate_http_links(block, None).err(),
            passages,
        }
    }
}

fn collect_ids(node: &Value, ids: &mut Vec<String>) {
    if node["type"].as_str().is_some_and(is_addressable_block)
        && let Some(id) = node["attrs"]["id"].as_str()
    {
        ids.push(id.to_owned());
    }
    for child in node["content"].as_array().into_iter().flatten() {
        collect_ids(child, ids);
    }
}

pub(super) fn read(tx: &Transaction<'_>, ids: &[i64]) -> Result<BTreeMap<String, Facts>> {
    let mut statement = tx.prepare_cached(
        "SELECT facts_json FROM block_fact WHERE block_id IN (SELECT value FROM json_each(?1))",
    )?;
    let mut facts = BTreeMap::new();
    for row in statement.query_map([serde_json::to_string(ids)?], |row| row.get::<_, String>(0))? {
        let fact: Facts = serde_json::from_str(&row?)?;
        let key = fact
            .ids
            .first()
            .ok_or_else(|| Error::Invalid("Stored block has no stable ID.".into()))?
            .clone();
        facts.insert(key, fact);
    }
    if facts.len() != ids.len() {
        return Err(Error::Invalid(
            "A document manifest has missing or duplicate block facts.".into(),
        ));
    }
    Ok(facts)
}

#[derive(Default)]
pub(super) struct Page {
    pub(crate) prose: DocumentProjection,
    pub(crate) export: PageFacts,
    pub(crate) passage_ids: Vec<String>,
    ids: BTreeSet<String>,
    links: BTreeSet<String>,
    images: BTreeSet<String>,
}

impl Page {
    /// A repeated nested editor ID needs canonical normalization and a full response.
    pub(crate) fn append(&mut self, block: &Facts) -> Result<bool> {
        if let Some(error) = &block.link_error {
            return Err(Error::Invalid(error.clone()));
        }
        if block
            .ids
            .iter()
            .any(|id| !self.ids.insert(id.to_ascii_lowercase()))
        {
            return Ok(false);
        }
        let prose = &block.prose;
        if !prose.text.is_empty() {
            if !self.prose.text.is_empty() {
                self.prose.text.push(' ');
            }
            self.prose.text.push_str(&prose.text);
        }
        self.prose.labels.extend_from_slice(&prose.labels);
        self.prose
            .snippet_citations
            .extend_from_slice(&prose.snippet_citations);
        self.prose
            .snippet_embeds
            .extend_from_slice(&prose.snippet_embeds);
        if self.prose.preview_image.is_none() {
            self.prose.preview_image.clone_from(&prose.preview_image);
        }
        for link in &prose.page_links {
            if self.links.insert(link.clone()) {
                self.prose.page_links.push(link.clone());
            }
        }
        self.passage_ids.extend_from_slice(&block.passage_ids);
        let source = &block.export;
        let target = &mut self.export;
        target.field_uses.extend(
            source
                .field_uses
                .iter()
                .map(|(position, usage)| (target.issues.len() + position, usage.clone())),
        );
        target.issues.extend_from_slice(&source.issues);
        target.link_count += source.link_count;
        target.words += source.words;
        target.headings.extend_from_slice(&source.headings);
        target.anchor_links.extend_from_slice(&source.anchor_links);
        target.snippets.extend_from_slice(&source.snippets);
        for image in &source.images {
            if self.images.insert(image.clone()) {
                target.images.push(image.clone());
            }
        }
        Ok(true)
    }

    pub(crate) fn finish(&mut self) {
        for keys in [
            &mut self.prose.labels,
            &mut self.prose.snippet_citations,
            &mut self.prose.snippet_embeds,
        ] {
            keys.sort();
            keys.dedup();
        }
    }
}
