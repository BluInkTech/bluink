// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk
use super::{candidate, decisions};
use crate::domain::merge::{
    decisions::{Proposal, Selection, both::renew_copy},
    model::Fragment,
};
use crate::domain::project::identity::ItemId;
use crate::versioning::{Result, failure};
use git2::{Oid, Repository};
use serde_json::Value;
use std::{collections::BTreeMap, path::Path};

/// Keep the displayed asset and insert copies of its authored image blocks for
/// the alternative. Stable copied IDs make replay idempotent across devices.
pub(super) fn keep_both(
    repo: &Repository,
    files: &mut BTreeMap<String, Oid>,
    proposal: &Proposal,
    choice: Selection,
    timezone: &str,
) -> Result<()> {
    let (effective, selection) = proposal.effective(choice)?;
    if selection != Selection::Both {
        return Ok(());
    }
    let file = effective
        .file_decision()
        .ok_or_else(|| failure("Expected asset decision"))?;
    if !file.path.starts_with("assets/") {
        return Err(failure("Keep both requires an owned asset"));
    }
    let parts: Vec<_> = file.path.split('/').collect();
    if parts.len() != 4 {
        return Err(failure("Invalid owned asset path"));
    }
    let owner = ItemId::try_from(parts[2])?;
    let extension = Path::new(parts[3])
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("bin");
    let filename = format!("{}.{extension}", file.id);
    let path = owner
        .asset_directory(Path::new(""))
        .join(&filename)
        .to_string_lossy()
        .replace('\\', "/");
    let other = &file.alternatives[1 - file.provisional].value;
    let Fragment::Present(_) = other else {
        return Err(failure(
            "The deleted alternative cannot be kept as another file",
        ));
    };
    if let Some(existing) = files.get(&path) {
        if decisions::fragment(repo, Some(*existing))? == *other {
            return Ok(());
        }
        return Err(failure(
            "The copied asset was edited after this decision; its current bytes were preserved",
        ));
    }
    decisions::apply(repo, files, &path, other)?;
    let Some(before) = candidate::read(repo, files, parts[2])? else {
        return Err(failure("The asset's owner disappeared"));
    };
    if !matches!(before["meta"]["type"].as_str(), Some("page" | "template")) {
        return Err(failure("Keep both requires a page asset"));
    }
    let mut after = before.clone();
    let blocks = after["content"]["document"]["content"]
        .as_array_mut()
        .ok_or_else(|| failure("Asset owner has no document"))?;
    let old = format!("/assets/{}/{name}", parts[2], name = parts[3]);
    let new = format!("/assets/{}/{filename}", parts[2]);
    let mut output = Vec::new();
    for block in std::mem::take(blocks) {
        let mut copy = block.clone();
        if replace_source(&mut copy, &old, &new) {
            renew_copy(&mut copy, &file.id)
                .map_err(|e| failure(format!("Cannot copy asset block: {e:?}")))?;
            output.push(block);
            output.push(copy);
        } else {
            output.push(block);
        }
    }
    *blocks = output;
    if after != before {
        let fields = crate::domain::merge::item::semantic(&after)?;
        let updated = crate::store::commands::with_authoring_values(|values| {
            crate::domain::merge::item::materialize(
                crate::domain::merge::item::ItemCandidate::Changed {
                    fields,
                    created_at: before["createdAt"]
                        .as_str()
                        .ok_or("Missing creation time")?
                        .into(),
                },
                Some(&before),
                timezone,
                values,
            )
        })?
        .ok_or_else(|| failure("Missing asset owner"))?;
        candidate::put(repo, files, &updated)?;
    }
    Ok(())
}

fn replace_source(node: &mut Value, old: &str, new: &str) -> bool {
    let mut changed = if node["attrs"]["src"] == old {
        node["attrs"]["src"] = Value::String(new.into());
        true
    } else {
        false
    };
    if let Some(children) = node.get_mut("content").and_then(Value::as_array_mut) {
        for child in children {
            changed |= replace_source(child, old, new);
        }
    }
    changed
}
