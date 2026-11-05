// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! Derived document facts and relationships, published inside the canonical write.

pub(crate) mod document;
pub(crate) mod passages;

use std::collections::BTreeSet;

use rusqlite::{Transaction, params};
use serde_json::Value;

use crate::domain::merge::decisions::{self, Proposal};
use crate::domain::project::named_items;
use crate::store::projections::document::{SearchDocument, project_item};

use super::{Error, Result, item};

pub(crate) struct Prepared {
    pub(crate) document: SearchDocument,
    content_hash: String,
    words: i64,
    decision: Option<decisions::Record>,
    publication_targets: BTreeSet<String>,
    snippet_search: Option<String>,
}

impl Prepared {
    /// The canonical ingress validator has already checked this envelope.
    pub(crate) fn new(value: &Value) -> Result<Self> {
        let id = value["id"]
            .as_str()
            .ok_or_else(|| Error::Invalid("Item has no ID.".to_owned()))?;
        let document = project_item(value, id).map_err(Error::Invalid)?;
        let content_hash = value["analytics"]["current"]["contentHash"]
            .as_str()
            .ok_or_else(|| Error::Invalid("Item has no analyzed content hash.".to_owned()))?
            .to_owned();
        let words = value["analytics"]["current"]["wordCount"]
            .as_i64()
            .filter(|words| *words >= 0)
            .ok_or_else(|| Error::Invalid("Item has no valid word count.".to_owned()))?;
        let decision = (document.item_type == "conflict")
            .then(|| decisions::read(value).map_err(Error::Invalid))
            .transpose()?;
        let snippet_search = document.snippet.as_ref().map(snippet_search_text);
        Ok(Self {
            snippet_search,
            document,
            content_hash,
            words,
            decision,
            publication_targets: crate::domain::project::candidate::publication_targets(value),
        })
    }

    pub(crate) fn install(&self, transaction: &Transaction<'_>, id: i64) -> Result<()> {
        let document = &self.document;
        transaction.execute(
            "INSERT INTO item_projection(item_id,preview,preview_image,analyzed_text_hash,word_count,export_facts_json,page_links_json)
             VALUES (?1,?2,?3,?4,?5,?6,?7) ON CONFLICT(item_id) DO UPDATE SET preview=excluded.preview,
             preview_image=excluded.preview_image,analyzed_text_hash=excluded.analyzed_text_hash,
             word_count=excluded.word_count,export_facts_json=excluded.export_facts_json,page_links_json=excluded.page_links_json",
            params![id,document.preview,document.preview_image,self.content_hash,self.words,document.export_facts,serde_json::to_string(&document.page_links)?],
        )?;
        relationships(transaction, id, document)?;
        transaction.execute(
            "DELETE FROM relation WHERE source_id=?1 AND kind='publication_member'",
            [id],
        )?;
        for target in &self.publication_targets {
            edge(
                transaction,
                id,
                "publication_member",
                item::identity(transaction, target, None)?,
            )?;
        }
        passages(transaction, id, document)?;
        task(transaction, id, document)?;
        install_snippet(transaction, id, self.snippet_search.as_deref())?;
        decision(transaction, id, document, self.decision.as_ref())?;
        Ok(())
    }
}

fn relationships(transaction: &Transaction<'_>, id: i64, document: &SearchDocument) -> Result<()> {
    document_edges(
        transaction,
        id,
        &document.labels,
        &document.reference_targets,
        &document.snippet_citation_targets,
        &document.snippet_embed_targets,
    )?;
    transaction.execute(
        "DELETE FROM relation WHERE source_id=?1 AND kind='task_owner'",
        [id],
    )?;
    if let Some(task) = &document.task {
        edge(
            transaction,
            id,
            "task_owner",
            item::identity(transaction, &task.note_id, None)?,
        )?;
    }
    Ok(())
}

fn edge(transaction: &Transaction<'_>, source: i64, kind: &str, target: i64) -> Result<()> {
    transaction.execute(
        "INSERT INTO relation(source_id,kind,target_id) VALUES (?1,?2,?3) ON CONFLICT DO NOTHING",
        params![source, kind, target],
    )?;
    Ok(())
}

fn passages(transaction: &Transaction<'_>, id: i64, document: &SearchDocument) -> Result<()> {
    transaction.execute("DELETE FROM passage WHERE item_id=?1", [id])?;
    for (position, passage) in document.passages.iter().enumerate() {
        write_passage(transaction, id, position, passage)?;
    }
    Ok(())
}

fn task(transaction: &Transaction<'_>, id: i64, document: &SearchDocument) -> Result<()> {
    transaction.execute("DELETE FROM task_projection WHERE item_id=?1", [id])?;
    if let Some(task) = &document.task {
        let note_id = item::identity(transaction, &task.note_id, None)?;
        let completed = task
            .completed_at
            .as_deref()
            .map(parse_instant)
            .transpose()?;
        let due = task
            .due_date
            .as_deref()
            .map(|date| {
                chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d")
                    .map(|date| {
                        date.signed_duration_since(chrono::DateTime::UNIX_EPOCH.date_naive())
                            .num_days()
                    })
                    .map_err(|error| Error::Invalid(error.to_string()))
            })
            .transpose()?;
        transaction.execute("INSERT INTO task_projection(item_id,note_id,text,completed_ms,due_day) VALUES (?1,?2,?3,?4,?5)",params![id,note_id,task.text,completed,due])?;
    }
    Ok(())
}

fn decision(
    transaction: &Transaction<'_>,
    id: i64,
    document: &SearchDocument,
    record: Option<&decisions::Record>,
) -> Result<()> {
    transaction.execute("DELETE FROM decision_projection WHERE item_id=?1", [id])?;
    if let Some(record) = record {
        let kind = match record.proposal {
            Proposal::Item(_) => "item",
            Proposal::Group(_) => "group",
            Proposal::File(_) => "file",
            Proposal::Resolution(_) => "resolution",
        };
        transaction.execute("INSERT INTO decision_projection(item_id,kind,target_key,resolved,created_ms) VALUES (?1,?2,?3,?4,?5)",params![id,kind,record.proposal.target(),record.resolution.is_some(),parse_instant(&document.created_at)?])?;
        if record.resolution.is_none() {
            let mut targets = BTreeSet::from([record.proposal.target()]);
            if let Proposal::Group(group) = &record.proposal {
                targets.extend(
                    group
                        .provisional
                        .iter()
                        .map(|field| field.field.item.as_str()),
                );
            }
            for target in targets {
                if crate::domain::project::identity::is_item_id(target) {
                    let target = item::identity(transaction, target, None)?;
                    transaction.execute(
                        "INSERT INTO decision_target(decision_id,target_id) VALUES (?1,?2)",
                        params![id, target],
                    )?;
                }
            }
        }
    }
    Ok(())
}

fn parse_instant(value: &str) -> Result<i64> {
    chrono::DateTime::parse_from_rfc3339(value)
        .map(|value| value.timestamp_millis())
        .map_err(|error| Error::Invalid(error.to_string()))
}

pub(super) fn document_edges(
    transaction: &Transaction<'_>,
    id: i64,
    labels: &[String],
    references: &[String],
    citations: &[String],
    embeds: &[String],
) -> Result<()> {
    transaction.execute("DELETE FROM relation WHERE source_id=?1 AND kind IN ('label','reference','snippet_citation','snippet_embed')",[id])?;
    for (kind, values, named) in [
        ("label", labels, Some("label")),
        ("reference", references, None),
        ("snippet_citation", citations, Some("snippet")),
        ("snippet_embed", embeds, Some("snippet")),
    ] {
        for value in values {
            let target = if let Some(named) = named {
                item::identity(
                    transaction,
                    &named_items::id(named, value),
                    Some((named, value)),
                )?
            } else {
                item::identity(transaction, value, None)?
            };
            edge(transaction, id, kind, target)?;
        }
    }
    Ok(())
}

pub(super) fn write_passage(
    transaction: &Transaction<'_>,
    id: i64,
    position: usize,
    passage: &crate::records::PassageRecord,
) -> Result<()> {
    transaction.execute(
            "INSERT INTO passage(item_id,block_id,position,text,fragment_json,truncated) VALUES (?1,?2,?3,?4,?5,?6)",
            params![id,passage.block_id,i64::try_from(position).map_err(|error|Error::Invalid(error.to_string()))?,passage.text,serde_json::to_string(&passage.fragment)?,passage.truncated],
        )?;
    for key in &passage.labels {
        let target = item::identity(
            transaction,
            &named_items::id("label", key),
            Some(("label", key)),
        )?;
        transaction.execute(
            "INSERT INTO passage_label(item_id,block_id,label_id) VALUES (?1,?2,?3)",
            params![id, passage.block_id, target],
        )?;
    }

    Ok(())
}

fn snippet_search_text(record: &Value) -> String {
    record["paragraph"]["content"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|node| node["text"].as_str())
        .collect::<String>()
        .to_lowercase()
        .replace('\n', " ")
}

fn install_snippet(tx: &Transaction<'_>, id: i64, text: Option<&str>) -> Result<()> {
    if let Some(text) = text {
        tx.execute("INSERT INTO snippet_projection(item_id,search_text) VALUES (?1,?2) ON CONFLICT(item_id) DO UPDATE SET search_text=excluded.search_text",params![id,text])?;
    } else {
        tx.execute("DELETE FROM snippet_projection WHERE item_id=?1", [id])?;
    }
    Ok(())
}
