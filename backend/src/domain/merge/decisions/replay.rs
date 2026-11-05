// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk
use super::*;
use crate::domain::merge::validation::Snapshot;
use std::collections::BTreeMap;

/// Reapply recorded intent only to a freshly regenerated matching conflict.
/// The caller must not call this for unrelated edits merely carrying a tombstone.
pub(crate) fn replay(
    decision: &Value,
    current: &dyn Snapshot,
    config: &Value,
    values: &mut crate::domain::project::assemble::AuthoringValues<'_>,
) -> Result<BTreeMap<String, Value>, String> {
    let record = read(decision)?;
    let selection = record
        .resolution
        .as_ref()
        .map_or(Selection::Provisional, |resolution| resolution.selection);
    choose(&record.proposal, selection, current, config, values)
}

pub(super) fn choose(
    proposal: &Proposal,
    selection: Selection,
    current: &dyn Snapshot,
    config: &Value,
    values: &mut crate::domain::project::assemble::AuthoringValues<'_>,
) -> Result<BTreeMap<String, Value>, String> {
    let (proposal, selection) = proposal.effective(selection)?;
    let changes = match proposal {
        Proposal::Resolution(_) => return Err("Unresolved nested intent".into()),
        Proposal::File(_) => {
            return Err("File decisions must replay through the package coordinator".into());
        }
        Proposal::Item(proposal) => item_choice(proposal, selection, current, config, values)?,
        Proposal::Group(group) => group_choice(group, selection, current)?,
    };
    resolve::validate_choice(
        current,
        config,
        &changes,
        &changes.keys().cloned().collect(),
    )
    .map_err(error)?;
    Ok(changes)
}

fn item_choice(
    proposal: &Decision,
    selection: Selection,
    current: &dyn Snapshot,
    config: &Value,
    values: &mut crate::domain::project::assemble::AuthoringValues<'_>,
) -> Result<BTreeMap<String, Value>, String> {
    let mut proposal = proposal.clone();
    if proposal.location.0.is_empty() {
        let wanted = proposal.selected(match selection {
            Selection::Provisional => proposal.provisional,
            Selection::First => 0,
            Selection::Second => 1,
            Selection::Both => return Err("Keep both cannot replay a whole item".into()),
        })?;
        let before = current.read(&proposal.target)?;
        let actual = before
            .as_ref()
            .map(crate::domain::merge::item::semantic)
            .transpose()?;
        if Fragment::from(actual.as_ref()) == wanted {
            return Ok(BTreeMap::new());
        }
        if wanted == Fragment::Missing {
            return Err("Whole-item decision needs explicit deletion handling".into());
        }
        let timezone = config["analytics"]["timezone"]
            .as_str()
            .ok_or("Project has no timezone")?;
        let (restored, _) =
            super::whole::choose(&proposal, selection, current, timezone, values).map_err(error)?;
        let changes: BTreeMap<_, _> = restored
            .into_iter()
            .map(|value| (proposal.target.clone(), value))
            .collect();
        resolve::validate_choice(
            current,
            config,
            &changes,
            &changes.keys().cloned().collect(),
        )
        .map_err(error)?;
        return Ok(changes);
    }
    let before = current
        .read(&proposal.target)?
        .ok_or("Decision target disappeared during replay")?;
    let actual = super::patch::fragment(&before, &proposal.location.0).map_err(error)?;
    let wanted = match selection {
        Selection::Provisional => Some(proposal.selected(proposal.provisional)?),
        Selection::First => Some(proposal.selected(0)?),
        Selection::Second => Some(proposal.selected(1)?),
        Selection::Both => None,
    };
    if wanted.as_ref() == Some(&actual) {
        return Ok(BTreeMap::new());
    }
    let original = proposal.provisional;
    proposal.provisional = if actual == proposal.selected(0)? {
        0
    } else if actual == proposal.selected(1)? {
        1
    } else {
        return Err(
            "Decision replay encountered independently edited content; preserve both inputs".into(),
        );
    };
    let selection = if selection == Selection::Provisional {
        if original == 0 {
            Selection::First
        } else {
            Selection::Second
        }
    } else {
        selection
    };
    resolve::item_choice(&proposal, selection, current).map_err(error)
}

fn group_choice(
    group: &Group,
    selection: Selection,
    current: &dyn Snapshot,
) -> Result<BTreeMap<String, Value>, String> {
    let actual = crate::domain::merge::groups::patches(
        current,
        &group
            .provisional
            .iter()
            .map(|value| value.field.clone())
            .collect(),
    )?;
    let selected = group.selected(match selection {
        Selection::Provisional => None,
        Selection::First => Some(0),
        Selection::Second => Some(1),
        Selection::Both => return Err("Keep both is not a dependency choice".into()),
    })?;
    if actual == selected {
        return Ok(BTreeMap::new());
    }
    if actual != group.provisional
        && !group
            .alternatives
            .iter()
            .any(|alternative| alternative.fields == actual)
    {
        return Err(
            "Dependency replay encountered independently edited fields; preserve both inputs"
                .into(),
        );
    }
    let mut replay = group.clone();
    replay.provisional = actual;
    // Store the selected result explicitly because Provisional in the
    // captured proposal differs from the guard used for this replay.
    replay.alternatives[0].fields = selected;
    super::linked::choose(&replay, Selection::First, current).map_err(error)
}

fn error(error: resolve::ResolveError) -> String {
    match error {
        resolve::ResolveError::Stale(message)
        | resolve::ResolveError::Unsupported(message)
        | resolve::ResolveError::Invalid(message) => message,
    }
}
