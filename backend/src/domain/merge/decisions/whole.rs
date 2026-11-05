// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk
use super::{resolve::ResolveError, *};
use crate::domain::merge::{item::semantic, model::DecisionKind, validation::Snapshot};

/// Whole-item choices arise only from deletion against an edit. Confirmation
/// retains the exact current item; deletion still runs dependency validation.
pub(super) fn choose(
    proposal: &Decision,
    selection: Selection,
    current: &dyn Snapshot,
    timezone: &str,
    values: &mut crate::domain::project::assemble::AuthoringValues<'_>,
) -> Result<(Option<Value>, bool), ResolveError> {
    if proposal.kind != DecisionKind::DeleteEdit || selection == Selection::Both {
        return Err(ResolveError::Unsupported(
            "This whole-item choice is unavailable".into(),
        ));
    }
    let before = current
        .read(&proposal.target)
        .map_err(ResolveError::Invalid)?;
    let actual = before
        .as_ref()
        .map(semantic)
        .transpose()
        .map_err(ResolveError::Invalid)?;
    let actual = Fragment::from(actual.as_ref());
    if actual != proposal.selected(0).map_err(ResolveError::Invalid)?
        && actual != proposal.selected(1).map_err(ResolveError::Invalid)?
    {
        return Err(ResolveError::Stale(
            "Item changed since the deletion decision".into(),
        ));
    }
    let selected = proposal
        .selected(match selection {
            Selection::Provisional => proposal.provisional,
            Selection::First => 0,
            Selection::Second => 1,
            Selection::Both => {
                return Err(ResolveError::Unsupported(
                    "Keep both is not an item deletion choice".into(),
                ));
            }
        })
        .map_err(ResolveError::Invalid)?;
    if selected == actual {
        return Ok((None, false));
    }
    match selected {
        Fragment::Missing => Ok((None, before.is_some())),
        Fragment::Present(value) => Ok((
            Some(
                assemble_item(&value, before.as_ref(), timezone, values)
                    .map_err(ResolveError::Invalid)?,
            ),
            false,
        )),
    }
}
