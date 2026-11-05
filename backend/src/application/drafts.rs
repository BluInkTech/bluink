// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk
//! Revision-checked merge operations. Each plan is prepared from one SQLite
//! read snapshot; the writer transaction rechecks revisions or the project
//! generation, then commits items, projections, receipts and the change feed
//! together.
use super::{
    Application,
    error::{Result, internal_error, internal_message, invalid_request},
};
use crate::domain::merge::{
    decisions::{
        self, Proposal, Resolution, Selection,
        resolve::{self, Expected},
    },
    groups::Overlay,
    item::{materialize, merge_item},
    model::{Context, Source},
    validation::{Snapshot, Snapshots},
};
use crate::domain::project::{self, identity::ItemId, types::OpaqueJson};
use crate::records::items::{PublicItem, canonical_item};
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

mod live;
use crate::records::drafts::{self as request, MergeDraftRequest, MergeDraftResult};

#[cfg(test)]
mod tests;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub(crate) struct DecisionsRequest {
    #[cfg_attr(test, ts(optional))]
    target_id: Option<String>,
}

#[derive(Debug, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub(crate) struct DecisionsResult {
    decisions: Vec<PublicItem>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub(crate) struct DecisionRequest {
    pub(super) id: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(test, derive(ts_rs::TS))]
pub(crate) struct DecisionPreview {
    pub(super) decision: PublicItem,
    pub(super) current_targets: Vec<PublicItem>,
    pub(super) current_files: Vec<DecisionFile>,
    pub(super) allowed_selections: Vec<decisions::Selection>,
    pub(super) expected: Expected,
}

#[derive(Debug, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub(crate) struct DecisionFile {
    pub(super) path: String,
    pub(super) content: OpaqueJson,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub(crate) struct ResolveDecisionRequest {
    pub(super) id: String,
    pub(super) expected: Expected,
    pub(super) selection: Selection,
    pub(super) device: String,
}

#[derive(Debug, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub(crate) struct ResolveDecisionResult {
    pub(super) decision: PublicItem,
    pub(super) items: Vec<PublicItem>,
    pub(super) deleted_ids: Vec<String>,
}

#[expect(
    clippy::needless_pass_by_value,
    reason = "RPC dispatch owns decoded requests."
)]
impl Application {
    pub(crate) fn decision_count(&self) -> Result<usize> {
        self.database()?
            .read(|tx| {
                let count: i64 = tx.query_row(
                    "SELECT count(*) FROM decision_projection WHERE resolved=0",
                    [],
                    |row| row.get(0),
                )?;
                usize::try_from(count)
                    .map_err(|_| crate::store::Error::corrupt("Invalid decision count"))
            })
            .map_err(Into::into)
    }

    pub(crate) fn list_decisions(&self, request: DecisionsRequest) -> Result<DecisionsResult> {
        let _operation = self.begin_operation()?;
        if let Some(id) = &request.target_id {
            ItemId::try_from(id.as_str()).map_err(invalid_request)?;
        }
        let values=self.database()?.read(|tx| {
            let sql=if request.target_id.is_some(){"SELECT i.uuid FROM entity target JOIN decision_target dt ON dt.target_id=target.id JOIN item i ON i.id=dt.decision_id WHERE target.uuid=?1 ORDER BY i.uuid"}else{"SELECT i.uuid FROM decision_projection d JOIN item i ON i.id=d.item_id WHERE d.resolved=0 ORDER BY i.uuid"};
            let ids=tx.prepare(sql)?.query_map(rusqlite::params_from_iter(request.target_id.iter()),|row|row.get::<_,String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
            ids.iter().map(|id|crate::store::item::read(tx,id,true)?.ok_or(crate::store::Error::NotFound)).collect::<crate::store::Result<Vec<_>>>()
        })?;
        Ok(DecisionsResult {
            decisions: values
                .into_iter()
                .map(|value| Ok(canonical_item(value, true)?))
                .collect::<Result<_>>()?,
        })
    }

    pub(crate) fn preview_decision(&self, request: DecisionRequest) -> Result<DecisionPreview> {
        if self.is_file_decision(&request.id)? {
            return self.preview_file_decision(request);
        }
        let _operation = self.begin_operation()?;
        inspect(self.database()?.as_ref(), |tx| {
            let capture = live::Live(tx);
            let expected = resolve::preview(&request.id, &capture).map_err(resolve_error)?;
            let decision = capture
                .read(&request.id)
                .map_err(internal_message)?
                .ok_or_else(|| invalid_request("Decision no longer exists"))?;
            let mut current_targets = Vec::new();
            for (id, revision) in &expected.targets {
                if revision != "missing" {
                    current_targets.push(canonical_item(
                        capture
                            .read(id)
                            .map_err(internal_message)?
                            .ok_or_else(|| invalid_request("Decision target disappeared"))?,
                        true,
                    )?);
                }
            }
            let config = crate::store::project::read_config(tx)?;
            let allowed_selections = [
                decisions::Selection::Provisional,
                decisions::Selection::First,
                decisions::Selection::Second,
                decisions::Selection::Both,
            ]
            .into_iter()
            .filter(|selection| {
                crate::store::commands::with_authoring_values(|values| {
                    resolve::resolve(
                        &request.id,
                        &expected,
                        Resolution {
                            selection: *selection,
                            resolved_at: now(),
                            device: "preview".to_owned(),
                        },
                        &capture,
                        &config,
                        values,
                    )
                })
                .is_ok()
            })
            .collect();
            Ok(DecisionPreview {
                decision: canonical_item(decision, true)?,
                current_targets,
                current_files: Vec::new(),
                allowed_selections,
                expected,
            })
        })
    }

    pub(crate) fn resolve_decision(
        &self,
        request: ResolveDecisionRequest,
    ) -> Result<ResolveDecisionResult> {
        if self.is_file_decision(&request.id)? {
            return self.resolve_file_decision(request);
        }
        let _operation = self.begin_operation()?;
        let store = self.database()?;
        // The plan reads more items than it writes, so it needs an unchanged
        // project. Unrelated saves only prompt a fresh plan; a stale preview
        // still fails through the decision's expected revisions.
        let mut attempt = 0;
        let plan = loop {
            let (generation, delta, plan) = inspect(&store, |tx| {
                let capture = live::Live(tx);
                let generation = read_generation(tx)?;
                let config = crate::store::project::read_config(tx)?;
                let plan = crate::store::commands::with_authoring_values(|values| {
                    resolve::resolve(
                        &request.id,
                        &request.expected,
                        Resolution {
                            selection: request.selection,
                            device: request.device.clone(),
                            resolved_at: now(),
                        },
                        &capture,
                        &config,
                        values,
                    )
                })
                .map_err(resolve_error)?;
                let mut replacements = Vec::new();
                for value in plan.targets.values().chain(std::iter::once(&plan.decision)) {
                    let id = value["id"]
                        .as_str()
                        .ok_or_else(|| internal_message("Resolution target has no ID"))?;
                    let previous = crate::store::item::read_header(tx, id)?;
                    let expected = previous
                        .as_ref()
                        .and_then(|header| header.value["revision"].as_str());
                    if expected != value["revision"].as_str() {
                        replacements.push((value.clone(), expected.map(str::to_owned)));
                    }
                }
                let mut deletes = Vec::new();
                let mut assets = Vec::new();
                for id in &plan.deleted {
                    let header = crate::store::item::read_header(tx, id)?
                        .ok_or(crate::store::Error::NotFound)?;
                    // A deleted item's attachments go with it, as in an incoming delete.
                    let owned = crate::store::item::owned_asset_paths(tx, header.id)?;
                    assets.extend(owned.into_iter().map(|path| (path, None)));
                    deletes.push(crate::store::mutations::Delete {
                        id: id.clone(),
                        expected: header.value["revision"]
                            .as_str()
                            .ok_or_else(|| internal_message("Invalid revision"))?
                            .to_owned(),
                    });
                }
                Ok((
                    generation,
                    crate::store::portable::PreparedDelta::new(
                        replacements,
                        deletes,
                        Vec::new(),
                        assets,
                    )?,
                    plan,
                ))
            })?;
            match store.write(move |tx| delta.apply(tx, generation)) {
                Ok(_) => break plan,
                Err(crate::store::Error::RevisionConflict) if attempt < 2 => attempt += 1,
                Err(error) => {
                    return Err(match error {
                        crate::store::Error::RevisionConflict => {
                            super::error::ApplicationError::StaleDecision(
                                "The project changed. Preview the decision again.".into(),
                            )
                        }
                        other => other.into(),
                    });
                }
            }
        };
        Ok(ResolveDecisionResult {
            decision: canonical_item(plan.decision, true)?,
            items: plan
                .targets
                .into_values()
                .map(|value| Ok(canonical_item(value, true)?))
                .collect::<Result<_>>()?,
            deleted_ids: plan.deleted.into_iter().collect(),
        })
    }

    pub(crate) fn merge_item_draft(&self, request: MergeDraftRequest) -> Result<MergeDraftResult> {
        let _operation = self.begin_operation()?;
        for id in [&request.id, &request.base_revision, &request.submission_id] {
            ItemId::try_from(id.as_str()).map_err(invalid_request)?;
        }
        if request.device.trim().is_empty() || request.device.len() > 256 {
            return Err(invalid_request(
                "A draft needs a device name of up to 256 bytes.",
            ));
        }
        let hash = project::submissions::hash(&request).map_err(invalid_request)?;
        let store = self.database()?;
        // Identity, receipt, timezone and patch base share one read snapshot.
        let (local_id, replayed, timezone, base) = store.read(|tx| {
            let local_id: i64 = tx
                .query_row(
                    "SELECT id FROM entity WHERE uuid=?1",
                    [&request.id],
                    |row| row.get(0),
                )
                .optional()?
                .ok_or(crate::store::Error::NotFound)?;
            let replayed =
                crate::store::receipts::replay(tx, &submission(&request, local_id, &hash))?;
            if replayed.is_some() {
                return Ok((local_id, replayed, None, None));
            }
            let config = crate::store::project::read_config(tx)?;
            let timezone = config["analytics"]["timezone"].as_str().map(str::to_owned);
            let base = crate::store::draft::read_base(tx, &request)?;
            Ok((local_id, None, timezone, base))
        })?;
        if let Some(outcome) = replayed {
            return serde_json::from_value(outcome).map_err(internal_error);
        }
        let timezone = super::clock::timezone(
            timezone
                .as_deref()
                .ok_or_else(|| invalid_request("Project has no timezone."))?,
        )?;
        let day = super::clock::activity_date(chrono::Utc::now(), timezone);
        if let Some(prepared) =
            crate::store::draft::prepare(base, &request, timezone.name(), &hash)?
        {
            let activity_day = day.clone();
            if let Some(result) = store.write(move |tx| prepared.apply(tx, &activity_day))? {
                return serde_json::from_value(result).map_err(internal_error);
            }
        }
        // A concurrent save can cross the preparation boundary. Retry against its
        // committed revision, then let the client replay the same submission if busy.
        for _ in 0..3 {
            let prepared = inspect(&store, |tx| {
                prepare_reconciliation(tx, &request, local_id, timezone.name())
            })?;
            let receipt = OwnedSubmission {
                item: local_id,
                device: request.device.clone(),
                id: request.submission_id.clone(),
                hash: hash.clone(),
                base: request.base_revision.clone(),
            };
            let day = day.clone();
            let outcome = store.write(move |tx| {
                let key = receipt.key();
                if let Some(saved) = crate::store::receipts::replay(tx, &key)? {
                    return Ok(Some(saved));
                }
                let PreparedReconciliation {
                    generation,
                    changes,
                    accepted,
                    result,
                } = prepared;
                if read_generation(tx)? != generation {
                    return Ok(None);
                }
                if !crate::store::maintenance::base_is_admissible(
                    tx,
                    key.item,
                    key.base,
                    chrono::Utc::now().timestamp_millis(),
                )? {
                    return Err(crate::store::Error::Invalid(
                        "Draft base is unavailable; keep a recovery copy".into(),
                    ));
                }
                crate::store::mutations::apply(tx, &changes, &[], Some(&day))?;
                crate::store::receipts::complete(tx, &key, &accepted, &result)?;
                Ok(Some(result))
            })?;
            if let Some(outcome) = outcome {
                return serde_json::from_value(outcome).map_err(internal_error);
            }
        }
        Err(crate::store::Error::Busy.into())
    }
}

struct PreparedReconciliation {
    generation: i64,
    changes: Vec<crate::store::mutations::PreparedChange>,
    accepted: String,
    result: Value,
}

fn prepare_reconciliation(
    tx: &rusqlite::Transaction<'_>,
    request: &MergeDraftRequest,
    local_id: i64,
    timezone: &str,
) -> Result<PreparedReconciliation> {
    let admissible = crate::store::maintenance::base_is_admissible(
        tx,
        local_id,
        &request.base_revision,
        chrono::Utc::now().timestamp_millis(),
    )?;
    if !admissible {
        return Err(invalid_request(
            "Draft base is unavailable; keep a recovery copy",
        ));
    }
    let current =
        crate::store::item::read(tx, &request.id, true)?.ok_or(crate::store::Error::NotFound)?;
    let base = if current["revision"] == request.base_revision {
        current.clone()
    } else {
        crate::store::item::read_revision(tx, &request.id, &request.base_revision)?
            .ok_or_else(|| invalid_request("Draft base is unavailable; keep a recovery copy"))?
    };
    let config = crate::store::project::read_config(tx)?;
    let plan = prepare_draft(request, &base, &current, &live::Live(tx), &config, timezone)
        .map_err(invalid_request)?;
    let authored = request.draft.expand(&base).map_err(invalid_request)?;
    let unchanged = matches!(*request.draft, request::EditorDraft::DocumentPatch { .. })
        && ["meta", "content", "tags", "pins"]
            .iter()
            .all(|field| authored[field] == plan.next[field]);
    let item = canonical_item(plan.next, true)?;
    let accepted = item.version.clone();
    let result = if unchanged {
        MergeDraftResult::Ack {
            version: item.version,
            modified_at: item.modified_at,
            conflict_ids: plan.conflict_ids,
            submission_id: request.submission_id.clone(),
        }
    } else {
        MergeDraftResult::Reconciled {
            item: Box::new(item),
            conflict_ids: plan.conflict_ids,
            submission_id: request.submission_id.clone(),
        }
    };
    let mut changes = Vec::new();
    for (id, value) in plan.changed {
        let previous = crate::store::item::read_header(tx, &id)?;
        let expected = previous
            .as_ref()
            .and_then(|header| header.value["revision"].as_str());
        if expected != value["revision"].as_str() {
            changes.push(crate::store::mutations::PreparedChange::new(
                value,
                expected.map(str::to_owned),
            )?);
        }
    }
    Ok(PreparedReconciliation {
        generation: read_generation(tx)?,
        changes,
        accepted,
        result: serde_json::to_value(result).map_err(internal_error)?,
    })
}

fn inspect<T>(
    store: &crate::store::Store,
    action: impl FnOnce(&rusqlite::Transaction<'_>) -> Result<T>,
) -> Result<T> {
    store.read(|tx| Ok(action(tx)))?
}

fn read_generation(tx: &rusqlite::Transaction<'_>) -> crate::store::Result<i64> {
    Ok(tx.query_row(
        "SELECT generation FROM project_state WHERE singleton=1",
        [],
        |row| row.get(0),
    )?)
}
fn now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}
fn submission<'a>(
    request: &'a MergeDraftRequest,
    item: i64,
    hash: &'a str,
) -> crate::store::receipts::Submission<'a> {
    crate::store::receipts::Submission {
        item,
        device: &request.device,
        id: &request.submission_id,
        hash,
        base: &request.base_revision,
    }
}
struct OwnedSubmission {
    item: i64,
    device: String,
    id: String,
    hash: String,
    base: String,
}
impl OwnedSubmission {
    fn key(&self) -> crate::store::receipts::Submission<'_> {
        crate::store::receipts::Submission {
            item: self.item,
            device: &self.device,
            id: &self.id,
            hash: &self.hash,
            base: &self.base,
        }
    }
}

impl Application {
    /// Admitted like any other read, so it never reopens a store during
    /// shutdown. The guard is released before dispatch: both paths take their own.
    fn is_file_decision(&self, id: &str) -> Result<bool> {
        ItemId::try_from(id).map_err(invalid_request)?;
        let _operation = self.begin_operation()?;
        let Some(item) = self
            .database()?
            .read(|tx| crate::store::item::read(tx, id, true))?
        else {
            return Ok(false);
        };
        if item["meta"]["type"] != "conflict" {
            return Ok(false);
        }
        Ok(decisions::read(&item)
            .map_err(invalid_request)?
            .proposal
            .file_decision()
            .is_some())
    }
}

fn resolve_error(error: resolve::ResolveError) -> super::error::ApplicationError {
    match error {
        resolve::ResolveError::Stale(message) => {
            super::error::ApplicationError::StaleDecision(message)
        }
        resolve::ResolveError::Unsupported(message) | resolve::ResolveError::Invalid(message) => {
            invalid_request(message)
        }
    }
}

struct DraftPlan {
    changed: BTreeMap<String, Value>,
    next: Value,
    conflict_ids: Vec<String>,
}

fn draft_input(
    request: &MergeDraftRequest,
    base: &Value,
    current: &Value,
    timezone: &str,
) -> std::result::Result<Value, String> {
    if !matches!(current["meta"]["type"].as_str(), Some("page" | "template")) {
        return Err("Draft merge is available for pages and templates".into());
    }
    let mut input = request.draft.expand(base)?;
    input["id"] = json!(request.id);
    let draft = crate::store::commands::assemble_item(&input, Some(base), timezone)?;
    if draft["meta"]["type"] != current["meta"]["type"] {
        return Err("Draft cannot change its item type".into());
    }
    project::item::validate_item(&draft, &request.id)?;
    crate::domain::document::links::validate_http_links(
        crate::domain::document::document_root(&draft["content"]),
        None,
    )?;
    Ok(draft)
}

fn prepare_draft(
    request: &MergeDraftRequest,
    base: &Value,
    current: &Value,
    capture: &dyn Snapshot,
    config: &Value,
    timezone: &str,
) -> std::result::Result<DraftPlan, String> {
    crate::store::commands::with_authoring_values(|values| {
        let draft = draft_input(request, base, current, timezone)?;
        let context = Context {
            target: request.id.clone(),
            base_epoch: format!("revision:{}", request.base_revision),
            left: Source::Draft {
                submission_id: request.submission_id.clone(),
                device: request.device.clone(),
            },
            right: Source::Revision {
                revision: current["revision"]
                    .as_str()
                    .ok_or("Missing current revision")?
                    .to_owned(),
                modified_at: current["modifiedAt"]
                    .as_str()
                    .ok_or("Missing modification time")?
                    .to_owned(),
            },
        };
        let merged = merge_item(&context, Some(base), Some(&draft), Some(current))?;
        let next = materialize(merged.candidate, Some(current), timezone, values)?
            .ok_or("A draft cannot delete its target")?;
        let mut base_snapshot = Overlay::new(capture);
        base_snapshot.put(base.clone())?;
        let mut draft_snapshot = Overlay::new(capture);
        draft_snapshot.put(draft)?;
        let mut candidate = Overlay::new(capture);
        candidate.put(next.clone())?;
        let changed_ids = BTreeSet::from([request.id.clone()]);
        let repaired = crate::domain::merge::groups::repair(
            &context,
            config,
            &changed_ids,
            Snapshots {
                base: &base_snapshot,
                left: &draft_snapshot,
                right: capture,
                candidate: &candidate,
            },
        )?;
        let mut changed = BTreeMap::from([(request.id.clone(), next)]);
        for (id, value) in repaired.items {
            let before = capture.read(&id)?;
            let value = materialize(value, before.as_ref(), timezone, values)?
                .ok_or("A draft repair cannot delete an item")?;
            candidate.put(value.clone())?;
            changed.insert(id, value);
        }
        let proposals = merged
            .decisions
            .into_iter()
            .map(|d| Proposal::Item(Box::new(d)))
            .chain(
                repaired
                    .groups
                    .into_iter()
                    .map(|d| Proposal::Group(Box::new(d))),
            );
        let mut conflict_ids = Vec::new();
        for proposal in proposals {
            let id = proposal.id().to_owned();
            let existing = capture.read(&id)?;
            let decision = decisions::prepare(proposal, existing.as_ref(), timezone, values)?;
            if existing.is_some() {
                for (target, value) in decisions::replay(&decision, &candidate, config, values)? {
                    let before = capture.read(&target)?;
                    let value = materialize(
                        crate::domain::merge::item::ItemCandidate::Changed {
                            fields: crate::domain::merge::item::semantic(&value)?,
                            created_at: value["createdAt"]
                                .as_str()
                                .ok_or("Missing creation timestamp")?
                                .to_owned(),
                        },
                        before.as_ref(),
                        timezone,
                        values,
                    )?
                    .ok_or("Replayed item disappeared")?;
                    candidate.put(value.clone())?;
                    changed.insert(target, value);
                }
            }
            if decisions::read(&decision)?.resolution.is_none() {
                conflict_ids.push(id.clone());
            }
            changed.insert(id, decision);
        }
        let next = changed
            .get(&request.id)
            .cloned()
            .ok_or("Draft target missing from group")?;
        Ok(DraftPlan {
            changed,
            next,
            conflict_ids,
        })
    })
}
