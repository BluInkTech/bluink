// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use super::{BodyChange, BodyEdit, Prepared};
use crate::domain::{
    document::projection::bounded_preview,
    project::{
        candidate::{Dependency, NamedKind},
        types::ItemType,
    },
};
use crate::store::changes::{ChangeDomain, ChangeOperation};
use crate::store::{
    Error, Result, activity, body, changes, fields, fts, git, item, projections, receipts,
    relations,
};
use rusqlite::{Transaction, params};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

impl Prepared {
    pub(crate) fn apply(self, tx: &Transaction<'_>, activity_day: &str) -> Result<Option<Value>> {
        let current = item::read_header(tx, &self.id)?.ok_or(Error::NotFound)?;
        let submission = receipts::Submission {
            item: current.id,
            device: &self.device,
            id: &self.submission,
            hash: &self.hash,
            base: &self.base,
        };
        if let Some(outcome) = receipts::replay(tx, &submission)? {
            return Ok(Some(outcome));
        }
        if current.value["revision"] != self.base {
            return Ok(None);
        }
        let previous_memberships = item::memberships(tx, current.id)?;
        if !self.valid_dependencies(tx, &previous_memberships)? {
            return Ok(None);
        }
        let next_body = match &self.body {
            BodyChange::Keep => current.body,
            BodyChange::Fields { order, outer } => {
                body::patch(tx, current.body, outer, order, &BTreeMap::new())?
            }
            BodyChange::Blocks(edit) => {
                body::patch(tx, current.body, &edit.outer, &edit.order, &edit.changed)?
            }
        };
        if next_body == current.body
            && current.value["meta"] == self.header["meta"]
            && previous_memberships == self.memberships
        {
            return acknowledgement(tx, &submission, &current.value).map(Some);
        }
        item::preserve(tx, &current, &previous_memberships)?;
        let meta = self.header["meta"]
            .as_object()
            .ok_or_else(|| Error::Invalid("Invalid draft metadata.".into()))?;
        fields::replace(tx, current.id, meta, current.value["meta"].as_object())?;
        item::install_header(tx, current.id, &self.header, next_body)?;
        if previous_memberships != self.memberships {
            item::replace_memberships(tx, current.id, &self.memberships)?;
        }
        if current.value["meta"]["title"] != self.header["meta"]["title"] {
            fts::update_title(
                tx,
                current.id,
                self.header["meta"]["title"].as_str().unwrap_or_default(),
            )?;
        }
        let kind: ItemType = serde_json::from_value(self.header["meta"]["type"].clone())?;
        if let BodyChange::Blocks(edit) = &self.body {
            edit.install(tx, current.id, &self.header, kind)?;
        }
        let revision = self.header["revision"]
            .as_str()
            .ok_or_else(|| Error::Invalid("Draft has no accepted revision.".into()))?;
        let mut domains = BTreeSet::from([ChangeDomain::Items]);
        if kind == ItemType::Page
            && current.value["analytics"]["current"]["contentHash"]
                != self.header["analytics"]["current"]["contentHash"]
        {
            activity::record_write(tx, Some(&current.value), &self.header, activity_day)?;
            domains.insert(ChangeDomain::Activity);
        }
        let generation = changes::publish(
            tx,
            &domains,
            &[changes::Change {
                id: &self.id,
                item_type: kind,
                operation: ChangeOperation::Upsert,
                revision: Some(revision),
            }],
        )?;
        git::mark_item_dirty(tx, &self.id, revision, generation)?;
        acknowledgement(tx, &submission, &self.header).map(Some)
    }

    /// The base equals the current revision here, so only what the draft adds
    /// needs a live check. Retained pins and snippet references are protected
    /// by their targets' delete and archive guards, tags have no target state,
    /// and `fields::replace` validates each changed property value.
    fn valid_dependencies(
        &self,
        tx: &Transaction<'_>,
        previous_memberships: &Value,
    ) -> Result<bool> {
        if previous_memberships["pins"] != self.memberships["pins"] {
            crate::store::commands::validate_new_pins(
                tx,
                Some(previous_memberships),
                &self.memberships,
            )?;
        }
        let BodyChange::Blocks(edit) = &self.body else {
            return Ok(true);
        };
        if edit.added_snippets.is_empty() {
            return Ok(true);
        }
        let dependencies = edit
            .added_snippets
            .iter()
            .map(|key| Dependency::named(NamedKind::Snippet, key))
            .collect();
        match relations::validate_dependencies(tx, &self.header, &dependencies) {
            Ok(()) => Ok(true),
            Err(Error::Invalid(_)) => Ok(false),
            Err(error) => Err(error),
        }
    }
}

fn acknowledgement(
    tx: &Transaction<'_>,
    submission: &receipts::Submission<'_>,
    header: &Value,
) -> Result<Value> {
    let revision = header["revision"]
        .as_str()
        .ok_or_else(|| Error::Invalid("Invalid revision.".into()))?;
    let outcome = json!({"kind":"ack","version":revision,"modifiedAt":header["modifiedAt"],"conflictIds":[],"submissionId":submission.id});
    receipts::complete(tx, submission, revision, &outcome)?;
    Ok(outcome)
}

impl BodyEdit {
    fn install(&self, tx: &Transaction<'_>, id: i64, header: &Value, kind: ItemType) -> Result<()> {
        let prose = &self.page.prose;
        let stats = &header["analytics"]["current"];
        tx.execute("UPDATE item_projection SET preview=?2,preview_image=?3,analyzed_text_hash=?4,word_count=?5,export_facts_json=?6,page_links_json=?7 WHERE item_id=?1",params![id,bounded_preview(&prose.text),prose.preview_image,stats["contentHash"].as_str(),stats["wordCount"].as_i64(),serde_json::to_string(&self.page.export)?,serde_json::to_string(&prose.page_links)?])?;
        if let Some(postings) = &self.fts {
            fts::install_body(tx, id, postings)?;
        }
        if self.edges_changed {
            projections::document_edges(
                tx,
                id,
                &prose.labels,
                &prose.page_links,
                &prose.snippet_citations,
                &prose.snippet_embeds,
            )?;
        }
        if kind == ItemType::Page {
            self.update_passages(tx, id)?;
        }
        Ok(())
    }

    fn update_passages(&self, tx: &Transaction<'_>, id: i64) -> Result<()> {
        tx.execute("DELETE FROM passage WHERE item_id=?1 AND block_id NOT IN (SELECT value FROM json_each(?2))",params![id,serde_json::to_string(&self.page.passage_ids)?])?;
        for (position, key) in self.page.passage_ids.iter().enumerate() {
            if let Some(passage) = self.passages.get(key) {
                tx.execute(
                    "DELETE FROM passage WHERE item_id=?1 AND block_id=?2",
                    params![id, key],
                )?;
                projections::write_passage(tx, id, position, passage)?;
            } else {
                let position =
                    i64::try_from(position).map_err(|error| Error::Invalid(error.to_string()))?;
                tx.execute("UPDATE passage SET position=?3 WHERE item_id=?1 AND block_id=?2 AND position<>?3",params![id,key,position])?;
            }
        }
        Ok(())
    }
}
