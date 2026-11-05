// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk
use super::{resolve::ResolveError, *};
use crate::domain::merge::model::{DecisionKind, Segment, stable_id};
use std::collections::BTreeMap;

pub(super) fn supported(proposal: &Decision) -> bool {
    let path = &proposal.location.0;
    matches!(proposal.kind, DecisionKind::Block | DecisionKind::DeleteEdit)
        && (matches!(path.last(),Some(Segment::Entry(_)))
            || matches!(path.as_slice(), [.., Segment::Entry(_), Segment::Field(key)] if key == "$body"))
        && proposal.alternatives.iter().all(|alternative| matches!(&alternative.value, Fragment::Present(value) if value["type"].as_str().is_some()))
}

pub(super) fn choose(
    proposal: &Decision,
    before: &Value,
) -> Result<BTreeMap<String, Value>, ResolveError> {
    if !matches!(before["meta"]["type"].as_str(), Some("page" | "template")) || !supported(proposal)
    {
        return Err(ResolveError::Unsupported(
            "Keep both requires two document blocks".into(),
        ));
    }
    let mut location = proposal.location.0.clone();
    if matches!(location.last(), Some(Segment::Field(key)) if key == "$body") {
        location.pop();
    }
    let Some(Segment::Entry(id)) = location.pop() else {
        return Err(ResolveError::Unsupported(
            "Keep both requires an identified block in a sequence".into(),
        ));
    };
    let Fragment::Present(mut other) = proposal
        .selected(1 - proposal.provisional)
        .map_err(ResolveError::Invalid)?
    else {
        return Err(ResolveError::Unsupported(
            "A deleted alternative has no second block to insert".into(),
        ));
    };
    if other["type"].as_str().is_none() {
        return Err(ResolveError::Unsupported(
            "Keep both requires a document block".into(),
        ));
    }
    let Fragment::Present(mut sequence) = super::patch::fragment(before, &location)? else {
        return Err(ResolveError::Stale("Block container disappeared".into()));
    };
    let blocks = sequence
        .as_array_mut()
        .ok_or_else(|| ResolveError::Stale("Block container changed".into()))?;
    let index = blocks
        .iter()
        .position(|block| block["attrs"]["id"] == id)
        .ok_or_else(|| ResolveError::Stale("Block disappeared".into()))?;
    if other["type"] == "paragraph"
        && other["attrs"].get("labels").is_none()
        && let Some(labels) = blocks[index]["attrs"].get("labels")
    {
        other["attrs"]["labels"] = labels.clone();
    }
    renew_copy(&mut other, &proposal.id)?;
    // The deterministic copied identity is a durable receipt for this choice.
    // Replaying a tombstone must preserve later edits to the accepted copy.
    if blocks
        .iter()
        .any(|block| block["attrs"]["id"] == other["attrs"]["id"])
    {
        return Ok(BTreeMap::new());
    }
    blocks.insert(index + 1, other);
    let mut after = before.clone();
    super::patch::replace(&mut after, &location, &Fragment::Present(sequence))?;
    Ok(BTreeMap::from([(proposal.target.clone(), after)]))
}

pub(crate) fn renew_copy(node: &mut Value, decision: &str) -> Result<(), ResolveError> {
    let mut identities = BTreeMap::new();
    renew(node, decision, &mut identities)?;
    remap(node, &identities);
    Ok(())
}

fn renew(
    node: &mut Value,
    decision: &str,
    identities: &mut BTreeMap<String, String>,
) -> Result<(), ResolveError> {
    if let Some(id) = node["attrs"]["id"].as_str() {
        let next =
            stable_id(&("bluink:keep-both:v1", decision, id)).map_err(ResolveError::Invalid)?;
        identities.insert(id.to_owned(), next.clone());
        node["attrs"]["id"] = json!(next);
    }
    if let Some(children) = node.get_mut("content").and_then(Value::as_array_mut) {
        for child in children {
            renew(child, decision, identities)?;
        }
    }
    Ok(())
}

fn remap(value: &mut Value, identities: &BTreeMap<String, String>) {
    match value {
        Value::Object(fields) => {
            // Export anchors name a block as `#block-<id>`; keep either form.
            if let Some(href) = fields.get("href").and_then(Value::as_str)
                && let Some(target) = href.strip_prefix('#')
            {
                let (prefix, id) = target
                    .strip_prefix("block-")
                    .map_or(("", target), |id| ("block-", id));
                if let Some(next) = identities.get(id) {
                    fields.insert("href".into(), json!(format!("#{prefix}{next}")));
                }
            }
            for child in fields.values_mut() {
                remap(child, identities);
            }
        }
        Value::Array(children) => {
            for child in children {
                remap(child, identities);
            }
        }
        _ => {}
    }
}
