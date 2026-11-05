// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk
use crate::{
    domain::merge::{
        decisions::{self, Proposal, Selection},
        model::{Context, Fragment},
    },
    versioning::{Result, failure, repository},
};
use git2::{Oid, Repository};
use serde_json::{Map, Value};
use std::collections::BTreeSet;

mod hunks;
#[cfg(test)]
mod tests;

pub(super) fn merge(
    repo: &Repository,
    context: &Context,
    path: &str,
    base: Option<Oid>,
    left: Option<Oid>,
    right: Option<Oid>,
) -> Result<(Option<Oid>, Vec<Proposal>)> {
    let text = |id: Option<Oid>| -> Result<Option<String>> {
        id.map(|id| {
            String::from_utf8(repo.find_blob(id)?.content().to_vec())
                .map_err(|_| failure("Portable files must use UTF-8 or LFS pointers"))
        })
        .transpose()
    };
    let (b, l, r) = (text(base)?, text(left)?, text(right)?);
    if repository::tracked(path) != Some(repository::Kind::Binary)
        && let (Some(b), Some(l), Some(r)) = (b.as_deref(), l.as_deref(), r.as_deref())
    {
        if path == "bluink.json" || path.ends_with("/manifest.json") {
            let mut proposals = Vec::new();
            let selected = json_merge(
                context,
                path,
                &mut Vec::new(),
                [
                    Some(&serde_json::from_str(b)?),
                    Some(&serde_json::from_str(l)?),
                    Some(&serde_json::from_str(r)?),
                ],
                &mut proposals,
            )?
            .ok_or_else(|| failure("JSON root disappeared"))?;
            return Ok((
                Some(repo.blob(&serde_json::to_vec_pretty(&selected)?)?),
                proposals,
            ));
        }
        let (selected, proposals) = hunks::merge(context, path, b, l, r)?;
        return Ok((Some(repo.blob(selected.as_bytes())?), proposals));
    }
    let proposal =
        decisions::file::propose(context, path, b.as_deref(), l.as_deref(), r.as_deref())?;
    let oid = match proposal.selected_file(Selection::Provisional)? {
        Fragment::Missing => None,
        Fragment::Present(value) => Some(
            repo.blob(
                value
                    .as_str()
                    .ok_or_else(|| failure("Expected file text"))?
                    .as_bytes(),
            )?,
        ),
    };
    Ok((oid, vec![proposal]))
}

fn json_merge(
    context: &Context,
    path: &str,
    keys: &mut Vec<String>,
    values: [Option<&Value>; 3],
    proposals: &mut Vec<Proposal>,
) -> Result<Option<Value>> {
    let [base, left, right] = values;
    if left == right || right == base {
        return Ok(left.cloned());
    }
    if left == base {
        return Ok(right.cloned());
    }
    if let (Some(Value::Object(l)), Some(Value::Object(r))) = (left, right) {
        let base = base.and_then(Value::as_object);
        let fields: BTreeSet<_> = l
            .keys()
            .chain(r.keys())
            .chain(base.into_iter().flat_map(|map| map.keys()))
            .collect();
        let mut result = Map::new();
        for field in fields {
            keys.push(field.clone());
            if let Some(value) = json_merge(
                context,
                path,
                keys,
                [
                    base.and_then(|map| map.get(field)),
                    l.get(field),
                    r.get(field),
                ],
                proposals,
            )? {
                result.insert(field.clone(), value);
            }
            keys.pop();
        }
        return Ok(Some(Value::Object(result)));
    }
    let proposal = decisions::file::propose_scoped(
        context,
        path,
        decisions::file::FileScope::Json { keys: keys.clone() },
        Fragment::from(base),
        Fragment::from(left),
        Fragment::from(right),
    )?;
    let selected = proposal.selected_file(Selection::Provisional)?;
    proposals.push(proposal);
    Ok(match selected {
        Fragment::Missing => None,
        Fragment::Present(value) => Some(value),
    })
}
