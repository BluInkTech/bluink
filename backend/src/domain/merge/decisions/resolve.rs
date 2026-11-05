// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use super::*;
use crate::domain::merge::validation::{self, Snapshot, Snapshots};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub(crate) struct Expected {
    pub(crate) decision_revision: String,
    /// Complete preview read set, including unchanged dependency owners.
    pub(crate) targets: BTreeMap<String, String>,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ResolveError {
    Stale(String),
    Unsupported(String),
    Invalid(String),
}

/// Publish these complete replacements and the decision in one transaction.
/// The accepting coordinator must recheck the captured database generation
/// before applying the plan. No writes occur here.
#[derive(Debug)]
pub(crate) struct ResolutionPlan {
    pub(crate) decision: Value,
    pub(crate) targets: BTreeMap<String, Value>,
    pub(crate) deleted: BTreeSet<String>,
}

fn decision(id: &str, current: &dyn Snapshot) -> Result<(Value, Record), ResolveError> {
    let item = current
        .read(id)
        .map_err(ResolveError::Invalid)?
        .ok_or_else(|| ResolveError::Stale("Decision no longer exists".into()))?;
    crate::domain::project::item::validate_item(&item, id).map_err(ResolveError::Invalid)?;
    let record = read(&item).map_err(ResolveError::Invalid)?;
    if record.resolution.is_some() {
        return Err(ResolveError::Stale("Decision is already resolved".into()));
    }
    Ok((item, record))
}

pub(crate) fn preview(id: &str, current: &dyn Snapshot) -> Result<Expected, ResolveError> {
    let (item, record) = decision(id, current)?;
    capture(&item, &record, current)
}

fn capture(
    item: &Value,
    record: &Record,
    current: &dyn Snapshot,
) -> Result<Expected, ResolveError> {
    let (effective, _) = record
        .proposal
        .effective(Selection::Provisional)
        .map_err(ResolveError::Invalid)?;
    let mut ids = match effective {
        Proposal::Item(proposal) => BTreeSet::from([proposal.target.clone()]),
        Proposal::Group(group) => super::linked::targets(group, current)?,
        Proposal::File(_) => {
            return Err(ResolveError::Unsupported(
                "File decisions require a package preview".into(),
            ));
        }
        Proposal::Resolution(_) => {
            return Err(ResolveError::Invalid("Unresolved nested intent".into()));
        }
    };
    let mut proposal = &record.proposal;
    while let Proposal::Resolution(intent) = proposal {
        ids.insert(intent.original.id().to_owned());
        proposal = &intent.original;
    }
    let mut targets = BTreeMap::new();
    for id in ids {
        let target = current.read(&id).map_err(ResolveError::Invalid)?;
        let Some(target) = target else {
            if matches!(effective, Proposal::Item(proposal) if proposal.target == id && proposal.location.0.is_empty())
            {
                targets.insert(id, "missing".into());
                continue;
            }
            return Err(ResolveError::Stale(
                "Decision target no longer exists".into(),
            ));
        };
        crate::domain::project::item::validate_item(&target, &id).map_err(ResolveError::Invalid)?;
        targets.insert(
            id,
            target["revision"]
                .as_str()
                .ok_or_else(|| ResolveError::Invalid("Missing target revision".into()))?
                .to_owned(),
        );
    }
    Ok(Expected {
        decision_revision: item["revision"]
            .as_str()
            .ok_or_else(|| ResolveError::Invalid("Missing decision revision".into()))?
            .to_owned(),
        targets,
    })
}

pub(crate) fn resolve(
    id: &str,
    expected: &Expected,
    resolution: Resolution,
    current: &dyn Snapshot,
    config: &Value,
    values: &mut crate::domain::project::assemble::AuthoringValues<'_>,
) -> Result<ResolutionPlan, ResolveError> {
    let (decision, mut record) = decision(id, current)?;
    if capture(&decision, &record, current)? != *expected {
        return Err(ResolveError::Stale(
            "Decision or dependency revisions changed since preview".into(),
        ));
    }
    let mut deleted = BTreeSet::new();
    let timezone = config["analytics"]["timezone"]
        .as_str()
        .ok_or_else(|| ResolveError::Invalid("Project has no timezone".into()))?;
    let (effective, selection) = record
        .proposal
        .effective(resolution.selection)
        .map_err(ResolveError::Invalid)?;
    let mut changes = match effective {
        Proposal::Item(proposal) if proposal.location.0.is_empty() => {
            let (restored, remove) =
                super::whole::choose(proposal, selection, current, timezone, values)?;
            if remove {
                deleted.insert(proposal.target.clone());
            }
            restored
                .into_iter()
                .map(|value| (proposal.target.clone(), value))
                .collect()
        }
        Proposal::Item(_) | Proposal::Group(_)
            if matches!(&record.proposal, Proposal::Resolution(_)) =>
        {
            super::replay::choose(
                &record.proposal,
                resolution.selection,
                current,
                config,
                values,
            )
            .map_err(ResolveError::Stale)?
        }
        Proposal::Item(proposal) => item_choice(proposal, selection, current)?,
        Proposal::Group(group) => super::linked::choose(group, selection, current)?,
        Proposal::File(_) => {
            return Err(ResolveError::Unsupported(
                "File decisions require a package transaction".into(),
            ));
        }
        Proposal::Resolution(_) => {
            return Err(ResolveError::Invalid("Unresolved nested intent".into()));
        }
    };
    apply_intents(&record.proposal, &resolution, current, &mut changes)?;
    validate_changes(
        current,
        config,
        &changes,
        &expected.targets.keys().cloned().collect(),
        &deleted,
    )?;
    let mut targets = BTreeMap::new();
    for (id, after) in changes {
        let before = current.read(&id).map_err(ResolveError::Invalid)?;
        let value = if let Some(before) = before {
            assemble_target(&before, &after, timezone, values)?
        } else {
            Some(after)
        };
        if let Some(value) = value {
            targets.insert(id, value);
        }
    }
    record.resolution = Some(resolution);
    let decision = assemble_item(
        &json!({"id":id,"meta":{},"content":record}),
        Some(&decision),
        timezone,
        values,
    )
    .map_err(ResolveError::Invalid)?;
    crate::domain::project::item::validate_item(&decision, id).map_err(ResolveError::Invalid)?;
    Ok(ResolutionPlan {
        decision,
        targets,
        deleted,
    })
}

pub(crate) fn apply_intents(
    proposal: &Proposal,
    resolution: &Resolution,
    current: &dyn Snapshot,
    changes: &mut BTreeMap<String, Value>,
) -> Result<(), ResolveError> {
    if let Proposal::Resolution(intent) = proposal {
        let selection = intent
            .choice(resolution.selection)
            .map_err(ResolveError::Invalid)?;
        let mut original = current
            .read(intent.original.id())
            .map_err(ResolveError::Invalid)?
            .ok_or_else(|| ResolveError::Stale("Original decision disappeared".into()))?;
        let mut record = read(&original).map_err(ResolveError::Invalid)?;
        // Regeneration can choose a different provisional side. Translate the
        // recorded outcome to this tombstone before assigning its resolution.
        let wanted = intent
            .original
            .selected(selection)
            .map_err(ResolveError::Invalid)?;
        let choice = record
            .proposal
            .selection_for(&wanted)
            .map_err(ResolveError::Invalid)?;
        let resolved = Resolution {
            selection: choice,
            resolved_at: resolution.resolved_at.clone(),
            device: resolution.device.clone(),
        };
        record.resolution = Some(resolved.clone());
        original["content"] =
            serde_json::to_value(&record).map_err(|e| ResolveError::Invalid(e.to_string()))?;
        changes.insert(intent.original.id().to_owned(), original);
        apply_intents(&record.proposal, &resolved, current, changes)?;
    }
    Ok(())
}

pub(super) fn item_choice(
    proposal: &Decision,
    selection: Selection,
    current: &dyn Snapshot,
) -> Result<BTreeMap<String, Value>, ResolveError> {
    if proposal.location.0.is_empty() {
        return Err(ResolveError::Unsupported(
            "Whole-item deletion requires its own policy".into(),
        ));
    }
    let before = current
        .read(&proposal.target)
        .map_err(ResolveError::Invalid)?
        .ok_or_else(|| ResolveError::Stale("Target disappeared".into()))?;
    let widget_order = before["meta"]["type"] == "workspace"
        && proposal.kind == crate::domain::merge::model::DecisionKind::Position
        && proposal.location
            == crate::domain::merge::model::Location::default()
                .field("content")
                .field("widgets")
                .field("$order");
    if !matches!(
        before["meta"]["type"].as_str(),
        Some(
            "page" | "template" | "task" | "tag" | "label" | "property" | "snippet" | "publication"
        )
    ) && !widget_order
    {
        return Err(ResolveError::Unsupported(
            "This target requires package-level resolution validation".into(),
        ));
    }
    if super::patch::fragment(&before, &proposal.location.0)?
        != proposal
            .selected(proposal.provisional)
            .map_err(ResolveError::Invalid)?
    {
        return Err(ResolveError::Stale(
            "Decision fragment changed; refresh its preview".into(),
        ));
    }
    if selection == Selection::Both {
        return super::both::choose(proposal, &before);
    }
    let selected = proposal
        .selected(match selection {
            Selection::Provisional => proposal.provisional,
            Selection::First => 0,
            Selection::Second => 1,
            Selection::Both => {
                return Err(ResolveError::Unsupported("Keep both needs a block".into()));
            }
        })
        .map_err(ResolveError::Invalid)?;
    let mut after = before.clone();
    super::patch::replace(&mut after, &proposal.location.0, &selected)?;
    Ok(if after == before {
        BTreeMap::new()
    } else {
        BTreeMap::from([(proposal.target.clone(), after)])
    })
}

pub(super) fn validate_choice(
    current: &dyn Snapshot,
    config: &Value,
    changes: &BTreeMap<String, Value>,
    inspected: &BTreeSet<String>,
) -> Result<(), ResolveError> {
    validate_changes(current, config, changes, inspected, &BTreeSet::new())
}

fn validate_changes(
    current: &dyn Snapshot,
    config: &Value,
    changes: &BTreeMap<String, Value>,
    inspected: &BTreeSet<String>,
    deleted: &BTreeSet<String>,
) -> Result<(), ResolveError> {
    let mut overlay = crate::domain::merge::groups::Overlay::new(current);
    for id in deleted {
        overlay.remove(id).map_err(ResolveError::Invalid)?;
    }
    for (id, after) in changes {
        crate::domain::project::item::validate_item(after, id).map_err(ResolveError::Invalid)?;
        if matches!(after["meta"]["type"].as_str(), Some("page" | "template")) {
            crate::domain::document::links::validate_http_links(
                crate::domain::document::document_root(&after["content"]),
                None,
            )
            .map_err(ResolveError::Invalid)?;
        }
        overlay.put(after.clone()).map_err(ResolveError::Invalid)?;
    }
    let report = validation::validate_affected(
        config,
        inspected,
        Snapshots {
            base: current,
            left: current,
            right: current,
            candidate: &overlay,
        },
    )
    .map_err(ResolveError::Invalid)?;
    if !report.violations.is_empty() {
        return Err(ResolveError::Stale(
            "Choice violates current relationships; refresh the dependency preview".into(),
        ));
    }
    Ok(())
}

fn assemble_target(
    before: &Value,
    after: &Value,
    timezone: &str,
    values: &mut crate::domain::project::assemble::AuthoringValues<'_>,
) -> Result<Option<Value>, ResolveError> {
    Ok(if before == after {
        None
    } else {
        let mut target =
            assemble_item(after, Some(before), timezone, values).map_err(ResolveError::Invalid)?;
        // assemble_item accepts a partial edit; this is an exact field patch.
        // Preserve explicit removal rather than reintroducing old metadata.
        target["meta"] = after["meta"].clone();
        for key in ["tags", "pins"] {
            if after.get(key).is_none() {
                target
                    .as_object_mut()
                    .ok_or_else(|| ResolveError::Invalid("Invalid target envelope".into()))?
                    .remove(key);
            }
        }
        crate::domain::project::item::validate_item(
            &target,
            after["id"]
                .as_str()
                .ok_or_else(|| ResolveError::Invalid("Target has no identity".into()))?,
        )
        .map_err(ResolveError::Invalid)?;
        Some(target)
    })
}
