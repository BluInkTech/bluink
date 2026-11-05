// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! Canonical decision records and tombstones. These helpers prepare values; the
//! accepting coordinator must journal targets and decisions together and publish
//! their index entries as one change. No standalone decision-write RPC exists.

use super::{
    groups::Group,
    model::{Decision, Fragment, Source},
};
use crate::domain::project::{assemble::assemble_item, canonical_json, identity::ItemId};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(
    tag = "kind",
    content = "decision",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub(crate) enum Proposal {
    Item(Box<Decision>),
    Group(Box<Group>),
    File(Box<file::FileDecision>),
    Resolution(Box<competing::Competing>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(test, derive(ts_rs::TS))]
pub(crate) enum Selection {
    Provisional,
    First,
    Second,
    Both,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Resolution {
    pub(crate) selection: Selection,
    pub(crate) resolved_at: String,
    pub(crate) device: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Record {
    pub(crate) proposal: Proposal,
    /// None means open. Resolved records remain active canonical tombstones.
    pub(crate) resolution: Option<Resolution>,
}

impl Proposal {
    pub(crate) fn id(&self) -> &str {
        match self {
            Self::Item(d) => &d.id,
            Self::Group(d) => &d.id,
            Self::File(d) => &d.id,
            Self::Resolution(d) => &d.id,
        }
    }

    pub(crate) fn target(&self) -> &str {
        match self {
            Self::Item(d) => &d.target,
            Self::Group(d) => &d.target,
            Self::File(d) => &d.target,
            Self::Resolution(d) => &d.target,
        }
    }

    pub(crate) fn effective(&self, selection: Selection) -> Result<(&Self, Selection), String> {
        if let Self::Resolution(d) = self {
            d.original.effective(d.choice(selection)?)
        } else {
            Ok((self, selection))
        }
    }

    pub(crate) fn file_decision(&self) -> Option<&file::FileDecision> {
        match self {
            Self::File(d) => Some(d),
            Self::Resolution(d) => d.original.file_decision(),
            _ => None,
        }
    }

    pub(crate) fn selected_file(&self, selection: Selection) -> Result<Fragment, String> {
        let (proposal, selection) = self.effective(selection)?;
        proposal
            .file_decision()
            .ok_or("Not a file decision")?
            .selected(selection)
    }

    pub(crate) fn selected_item(
        &self,
        selection: Selection,
    ) -> Result<Option<(&Decision, Fragment)>, String> {
        let (proposal, selection) = self.effective(selection)?;
        let Self::Item(decision) = proposal else {
            return Ok(None);
        };
        let index = match selection {
            Selection::Provisional => decision.provisional,
            Selection::First => 0,
            Selection::Second => 1,
            Selection::Both => return Ok(None),
        };
        Ok(Some((decision, decision.selected(index)?)))
    }

    fn validate(&self) -> Result<(), String> {
        let (target, epoch, identity, sources) = match self {
            Self::Resolution(d) => {
                d.validate()?;
                (
                    &d.target,
                    &d.base_epoch,
                    d.identity()?,
                    [&d.alternatives[0].source, &d.alternatives[1].source],
                )
            }
            Self::Item(d) => {
                if d.provisional > 1 {
                    return Err("Invalid provisional alternative".into());
                }
                for segment in &d.location.0 {
                    match segment {
                        super::model::Segment::Entry(id) => {
                            ItemId::try_from(id.as_str())?;
                        }
                        super::model::Segment::Field(key) if key.is_empty() => {
                            return Err("Conflict location has an empty field".into());
                        }
                        super::model::Segment::Cell(cell) if cell.index >= cell.structure.len() => {
                            return Err("Cell location must include its row structure".into());
                        }
                        _ => {}
                    }
                }
                ordered(&d.alternatives[0].value, &d.alternatives[1].value)?;
                if d.alternatives[0].value == d.alternatives[1].value {
                    return Err("An item decision needs different alternatives".into());
                }
                if d.kind == super::model::DecisionKind::Position {
                    if !matches!(d.location.0.last(), Some(super::model::Segment::Field(key)) if key == "$order")
                    {
                        return Err("Ordering decisions must address an identity sequence".into());
                    }
                    d.selected(0)?;
                    d.selected(1)?;
                }
                (
                    &d.target,
                    &d.base_epoch,
                    d.identity()?,
                    [&d.alternatives[0].source, &d.alternatives[1].source],
                )
            }
            Self::File(d) => {
                d.validate()?;
                (
                    &d.target,
                    &d.base_epoch,
                    d.identity()?,
                    [&d.alternatives[0].source, &d.alternatives[1].source],
                )
            }
            Self::Group(d) => {
                validate_group(d)?;
                (
                    &d.target,
                    &d.base_epoch,
                    d.identity()?,
                    [&d.alternatives[0].source, &d.alternatives[1].source],
                )
            }
        };
        ItemId::try_from(target.as_str())?;
        if epoch.is_empty() || identity != self.id() {
            return Err("Conflict identity does not match its immutable alternatives".into());
        }
        for source in sources {
            validate_source(source)?;
        }
        super::model::Context {
            target: target.clone(),
            base_epoch: epoch.clone(),
            left: sources[0].clone(),
            right: sources[1].clone(),
        }
        .prefer_left()?;
        Ok(())
    }

    /// The semantic result, not the choice's name or device. Two devices can
    /// select the same result through different buttons and still agree.
    fn selected(&self, selection: Selection) -> Result<Value, String> {
        if selection == Selection::Both {
            return match self {
                Self::Item(d) if both::supported(d) => {
                    Ok(json!({"both": [d.selected(0)?, d.selected(1)?]}))
                }
                Self::File(d) => {
                    d.selected(Selection::Both)?;
                    Ok(
                        json!({"both": [d.selected(Selection::First)?, d.selected(Selection::Second)?]}),
                    )
                }
                Self::Item(_) | Self::Group(_) | Self::Resolution(_) => {
                    Err("Keep both is only available for document blocks".into())
                }
            };
        }
        match self {
            Self::Resolution(d) => return d.original.selected(d.choice(selection)?),
            Self::File(d) => serde_json::to_value(d.selected(selection)?),
            Self::Item(d) => serde_json::to_value(d.selected(match selection {
                Selection::Provisional => d.provisional,
                Selection::First => 0,
                Selection::Second => 1,
                Selection::Both => return Err("Unexpected Keep both selection".into()),
            })?),
            Self::Group(d) => serde_json::to_value(d.selected(match selection {
                Selection::Provisional => None,
                Selection::First => Some(0),
                Selection::Second => Some(1),
                Selection::Both => return Err("Keep both is not a dependency choice".into()),
            })?),
        }
        .map_err(|error| error.to_string())
    }

    fn selection_for(&self, wanted: &Value) -> Result<Selection, String> {
        [
            Selection::First,
            Selection::Second,
            Selection::Provisional,
            Selection::Both,
        ]
        .into_iter()
        .find(|choice| self.selected(*choice).as_ref() == Ok(wanted))
        .ok_or_else(|| "Resolution intent no longer matches its original decision".into())
    }
}

fn validate_group(d: &Group) -> Result<(), String> {
    if d.dependencies.is_empty() || d.policies.is_empty() || d.base.is_empty() {
        return Err("A dependency decision needs constraints and fields".into());
    }
    let fields: Vec<_> = d.base.iter().map(|value| &value.field).collect();
    if fields.windows(2).any(|pair| pair[0] >= pair[1]) {
        return Err("Dependency fields must be sorted and unique".into());
    }
    for field in &fields {
        ItemId::try_from(field.item.as_str())?;
    }
    for values in [
        &d.alternatives[0].fields,
        &d.alternatives[1].fields,
        &d.provisional,
    ] {
        if values.iter().map(|value| &value.field).collect::<Vec<_>>() != fields {
            return Err("Dependency alternatives must describe the same fields".into());
        }
    }
    ordered(&d.alternatives[0].fields, &d.alternatives[1].fields)?;
    if d.members
        .iter()
        .map(|member| &member.dependency)
        .collect::<Vec<_>>()
        != d.dependencies.iter().collect::<Vec<_>>()
    {
        return Err("Dependency membership must match its sorted constraints".into());
    }
    for member in &d.members {
        for owner in &member.owners {
            ItemId::try_from(owner.as_str())?;
        }
    }
    Ok(())
}

fn ordered<T: Serialize>(left: &T, right: &T) -> Result<(), String> {
    if canonical_json::to_vec(left).map_err(|e| e.to_string())?
        > canonical_json::to_vec(right).map_err(|e| e.to_string())?
    {
        return Err("Conflict alternatives are not in canonical order".into());
    }
    Ok(())
}

fn validate_source(source: &Source) -> Result<(), String> {
    let valid = match source {
        Source::Commit { id, device, .. } => !id.is_empty() && !device.is_empty(),
        Source::Draft {
            submission_id,
            device,
        } => !submission_id.is_empty() && !device.is_empty(),
        Source::Revision {
            revision,
            modified_at,
        } => {
            !revision.is_empty() && crate::domain::project::item::is_canonical_instant(modified_at)
        }
    };
    if valid {
        Ok(())
    } else {
        Err("Conflict provenance is incomplete".into())
    }
}

pub(crate) fn read(item: &Value) -> Result<Record, String> {
    let record: Record =
        serde_json::from_value(item["content"].clone()).map_err(|e| e.to_string())?;
    // Also rejects unknown nested proposal fields and duplicate set entries.
    if serde_json::to_value(&record).map_err(|e| e.to_string())? != item["content"] {
        return Err("Conflict content is not in its canonical shape".into());
    }
    record.proposal.validate()?;
    if let Some(resolution) = &record.resolution {
        record.proposal.selected(resolution.selection)?;
    }
    if let Some(resolution) = &record.resolution
        && (resolution.device.trim().is_empty()
            || !crate::domain::project::item::is_canonical_instant(&resolution.resolved_at))
    {
        return Err("Conflict resolution needs a device and canonical timestamp".into());
    }
    Ok(record)
}

/// Called by canonical item validation; do not recurse into that validator here.
pub(crate) fn validate_item(item: &Value, id: &str) -> Result<(), String> {
    if item["meta"] != json!({"type":"conflict", "state":"active"})
        || item.get("tags").is_some()
        || item.get("pins").is_some()
    {
        return Err(
            "Conflict records cannot be archived, tagged, pinned or given authored metadata".into(),
        );
    }
    if read(item)?.proposal.id() != id {
        return Err("Conflict envelope identity differs from its proposal".into());
    }
    Ok(())
}

/// Reuse an existing open decision or resolved tombstone byte-for-byte. This
/// must run before accepting a candidate, never after publishing its content.
pub(crate) fn prepare(
    proposal: Proposal,
    existing: Option<&Value>,
    timezone: &str,
    values: &mut crate::domain::project::assemble::AuthoringValues<'_>,
) -> Result<Value, String> {
    proposal.validate()?;
    if let Some(existing) = existing {
        crate::domain::project::item::validate_item(existing, proposal.id())?;
        if existing["meta"]["type"] != "conflict" {
            return Err("Conflict identity is already used by another item".into());
        }
        return Ok(existing.clone());
    }
    let id = proposal.id().to_owned();
    let item = assemble_item(
        &json!({"id":id, "meta":{"type":"conflict"},
        "content":Record { proposal, resolution:None }}),
        None,
        timezone,
        values,
    )?;
    crate::domain::project::item::validate_item(&item, &id)?;
    Ok(item)
}

/// Conflict records never enter ordinary field merging: a resolved tombstone
/// dominates an open copy, and absence cannot delete it. Competing resolutions
/// stop with both inputs intact until resolution-of-resolution decisions exist.
pub(super) fn merge(
    base: Option<&Value>,
    left: Option<&Value>,
    right: Option<&Value>,
) -> Result<Value, String> {
    let mut chosen: Option<(&Value, Record)> = None;
    for item in [base, left, right].into_iter().flatten() {
        if item["meta"]["type"] != "conflict" {
            return Err("Conflict item type cannot change".into());
        }
        let record = read(item)?;
        if let Some((previous, old)) = &chosen {
            if old.proposal.id() != record.proposal.id() {
                return Err("Different conflict identities".into());
            }
            match (&old.resolution, &record.resolution) {
                (Some(a), Some(b))
                    if old.proposal.selected(a.selection)?
                        != record.proposal.selected(b.selection)? =>
                {
                    return Err("Competing conflict resolutions require a new decision; preserve both inputs".into());
                }
                (Some(_), None) => continue,
                (None, Some(_)) => {}
                _ if canonical_json::to_vec(previous).map_err(|e| e.to_string())?
                    <= canonical_json::to_vec(item).map_err(|e| e.to_string())? =>
                {
                    continue;
                }
                _ => {}
            }
        }
        chosen = Some((item, record));
    }
    chosen
        .map(|(item, _)| item.clone())
        .ok_or_else(|| "No conflict record survives".into())
}

pub(crate) mod both;
mod competing;
pub(crate) use competing::merge_records;
pub(crate) mod file;
mod linked;
mod replay;
mod whole;
pub(crate) use replay::replay;
mod patch;
pub(crate) mod resolve;

#[cfg(test)]
mod tests;
