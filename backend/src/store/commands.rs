// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! Ordinary authoring prepares content outside the writer and rechecks its base.
use super::{Error, Result, Store, item, mutations, project};
use rusqlite::Transaction;
use serde_json::{Map, Value};

pub(crate) enum MembershipChange {
    Tags(Vec<String>),
    Pin { scope: String, pinned: bool },
}

pub(crate) fn archive_named(store: &Store, kind: &str, key: &str) -> Result<()> {
    let id = crate::domain::project::named_items::id(kind, key);
    let previous = store
        .read(|tx| item::read_header(tx, &id))?
        .filter(|header| header.value["meta"]["state"] == "active")
        .ok_or(Error::NotFound)?;
    let revision = previous.value["revision"]
        .as_str()
        .ok_or_else(|| Error::corrupt("Declaration has no revision."))?;
    metadata(
        store,
        &id,
        Some(revision),
        Map::from_iter([("state".into(), serde_json::json!("archived"))]),
        false,
    )?;
    Ok(())
}

pub(crate) fn memberships(
    store: &Store,
    id: &str,
    expected: &str,
    change: MembershipChange,
    with_content: bool,
) -> Result<Value> {
    ItemId::try_from(id).map_err(Error::Invalid)?;
    let id = id.to_owned();
    let expected = expected.to_owned();
    store.write(move |tx| {
        let previous = item::read_header(tx,&id)?.ok_or(Error::NotFound)?;
        item::check_revision(Some(&previous),item::ExpectedRevision::Exact(&expected))?;
        let current = item::memberships(tx,previous.id)?;
        let mut memberships = current.clone();
        match change {
            MembershipChange::Tags(mut tags) => {
                for tag in &tags { crate::domain::project::facets::validate_content("tag",&serde_json::json!({"key":tag})).map_err(Error::Invalid)?; }
                tags.sort_unstable(); tags.dedup();
                memberships["tags"] = serde_json::json!(tags);
            }
            MembershipChange::Pin {scope,pinned} => {
                ItemId::try_from(scope.as_str()).map_err(Error::Invalid)?;
                let mut pins = memberships["pins"].as_array().cloned().unwrap_or_default();
                pins.retain(|value|value.as_str()!=Some(scope.as_str()));
                if pinned {
                    let active: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM item WHERE uuid=?1 AND type='workspace' AND state='active')",[&scope],|row|row.get(0))?;
                    if !active { return Err(Error::Invalid("Pins can only target an active workspace.".to_owned())); }
                    pins.push(serde_json::json!(scope));
                }
                pins.sort_by(|left,right|left.as_str().cmp(&right.as_str()));
                memberships["pins"] = Value::Array(pins);
            }
        }
        // A true no-op keeps its revision and produces no history work.
        if memberships == current {
            return item::assemble(tx,previous.value,previous.body,&current,with_content);
        }
        // This retains the old memberships in the exact revision before installing
        // their replacement; both the header and relation set share one commit.
        let header = mutations::metadata_from(tx,&id,&previous,&current,&expected,&Map::new())?;
        item::replace_memberships(tx,header.id,&memberships)?;
        // Stored memberships omit emptied kinds; return them as a read would.
        let stored = item::memberships(tx,header.id)?;
        item::assemble(tx,header.value,header.body,&stored,with_content)
    })
}
use crate::domain::project::{config, identity::ItemId};

pub(crate) fn write(
    store: &Store,
    input: Value,
    expected: Option<&str>,
    activity_day: &str,
) -> Result<Value> {
    let id = input["id"].as_str();
    if let Some(id) = id {
        ItemId::try_from(id).map_err(Error::Invalid)?;
    }
    let (previous, config) = store.read(|tx| {
        Ok((
            id.map(|id| item::read(tx, id, true)).transpose()?.flatten(),
            project::read_config(tx)?,
        ))
    })?;
    if previous
        .as_ref()
        .and_then(|value| value["revision"].as_str())
        != expected
    {
        return Err(Error::RevisionConflict);
    }
    let timezone = config["analytics"]["timezone"]
        .as_str()
        .ok_or_else(|| Error::Invalid("Project has no timezone.".to_owned()))?;
    let next = assemble_owned(input, previous.as_ref(), timezone).map_err(Error::Invalid)?;
    validate_content(previous.as_ref(), &next)?;
    let prepared = mutations::PreparedChange::new(next.clone(), expected.map(str::to_owned))?;
    let activity_day = activity_day.to_owned();
    store.write(move |tx| {
        validate_state(tx, previous.as_ref(), &next)?;
        mutations::apply(tx, &[prepared], &[], Some(&activity_day))?;
        Ok(next)
    })
}

pub(crate) fn metadata(
    store: &Store,
    id: &str,
    expected: Option<&str>,
    patch: Map<String, Value>,
    with_content: bool,
) -> Result<Value> {
    ItemId::try_from(id).map_err(Error::Invalid)?;
    let id = id.to_owned();
    let expected = expected.map(str::to_owned);
    store.write(move |tx| {
        // One header and membership read serves validation, the edit and the response.
        let header = item::read_header(tx, &id)?.ok_or(Error::NotFound)?;
        if expected
            .as_ref()
            .is_some_and(|revision| header.value["revision"].as_str() != Some(revision.as_str()))
        {
            return Err(Error::RevisionConflict);
        }
        let needs_content = matches!(
            header.value["meta"]["type"].as_str(),
            Some("goal" | "property" | "snippet")
        );
        let memberships = item::memberships(tx, header.id)?;
        let previous = item::assemble(
            tx,
            header.value.clone(),
            header.body,
            &memberships,
            needs_content,
        )?;
        let mut next = previous.clone();
        next["meta"]
            .as_object_mut()
            .ok_or_else(|| Error::Invalid("Item has no metadata.".to_owned()))?
            .extend(patch.clone());
        // A true no-op keeps its revision and produces no history work.
        if next["meta"] == previous["meta"] {
            return if with_content == needs_content {
                Ok(previous)
            } else {
                item::assemble(tx, header.value, header.body, &memberships, with_content)
            };
        }
        validate_content(Some(&previous), &next)?;
        validate_state(tx, Some(&previous), &next)?;
        let revision = header.value["revision"]
            .as_str()
            .ok_or_else(|| Error::Invalid("Item has no revision.".to_owned()))?
            .to_owned();
        let edited = mutations::metadata_from(tx, &id, &header, &memberships, &revision, &patch)?;
        item::assemble(tx, edited.value, edited.body, &memberships, with_content)
    })
}

pub(crate) fn validate_content(previous: Option<&Value>, next: &Value) -> Result<()> {
    let kind = next["meta"]["type"].as_str();
    if kind == Some("conflict") || previous.is_some_and(|value| value["meta"]["type"] == "conflict")
    {
        return Err(Error::Invalid(
            "Decisions are managed by the merge coordinator.".to_owned(),
        ));
    }
    if matches!(kind, Some("page" | "template")) && next.get("content").is_some() {
        crate::domain::document::links::validate_http_links(
            crate::domain::document::document_root(&next["content"]),
            None,
        )
        .map_err(Error::Invalid)?;
    }
    if let Some(previous) = previous {
        if kind == Some("goal") && previous["content"] != next["content"] {
            return Err(Error::Invalid(
                "Writing goal dates and targets cannot change. End this goal and create another."
                    .to_owned(),
            ));
        }
        if kind == Some("snippet") && previous["meta"]["state"] == "archived" {
            return Err(Error::Invalid(format!(
                "Snippet {} is archived.",
                next["content"]["key"].as_str().unwrap_or_default()
            )));
        }
        if kind == Some("property") && previous["meta"]["state"] == "active" {
            if previous["content"]["valueType"] != next["content"]["valueType"] {
                return Err(Error::Invalid(
                    "A property's value type cannot change.".to_owned(),
                ));
            }
            for option in previous["content"]["options"]
                .as_array()
                .into_iter()
                .flatten()
            {
                if !next["content"]["options"]
                    .as_array()
                    .is_some_and(|options| options.contains(option))
                {
                    return Err(Error::Invalid(
                        "Existing property options cannot be removed.".to_owned(),
                    ));
                }
            }
        }
    }
    if kind == Some("property")
        && next["meta"]["state"] != "active"
        && next["content"]["key"]
            .as_str()
            .is_some_and(config::is_required_builtin_property)
    {
        return Err(Error::Invalid(
            "Required built-in properties cannot be archived.".to_owned(),
        ));
    }
    Ok(())
}

pub(crate) fn validate_state(
    tx: &Transaction<'_>,
    previous: Option<&Value>,
    next: &Value,
) -> Result<()> {
    validate_new_pins(tx, previous, next)?;
    if next["meta"]["type"] == "goal"
        && next["meta"]["state"] == "active"
        && !previous.is_some_and(|value| value["meta"]["state"] == "active")
    {
        validate_goal(tx, next)?;
    }
    Ok(())
}

pub(super) fn validate_new_pins(
    tx: &Transaction<'_>,
    previous: Option<&Value>,
    next: &Value,
) -> Result<()> {
    for pin in next["pins"].as_array().into_iter().flatten() {
        if previous.is_some_and(|value| {
            value["pins"]
                .as_array()
                .is_some_and(|pins| pins.contains(pin))
        }) {
            continue;
        }
        let id = pin
            .as_str()
            .ok_or_else(|| Error::Invalid("Pin must identify a workspace.".to_owned()))?;
        let active: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM item WHERE uuid=?1 AND type='workspace' AND state='active')",[id],|row|row.get(0))?;
        if !active {
            return Err(Error::Invalid(
                "Pins can only target an active workspace.".to_owned(),
            ));
        }
    }
    Ok(())
}

fn validate_goal(tx: &Transaction<'_>, next: &Value) -> Result<()> {
    let content = &next["content"];
    if content["scope"] == "view" {
        let id = content["viewId"]
            .as_str()
            .ok_or_else(|| Error::Invalid("Choose a saved workspace.".to_owned()))?;
        let workspace = item::read(tx, id, true)?.ok_or(Error::NotFound)?;
        if workspace["meta"]["type"] != "workspace"
            || workspace["meta"]["state"] != "active"
            || workspace["content"]["query"] != content["query"]
        {
            return Err(Error::Invalid(
                "Writing goals need an active workspace with the captured query.".to_owned(),
            ));
        }
    }
    let overlap: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM item i JOIN body b ON b.id=i.body_id
        WHERE i.type='goal' AND i.state='active' AND i.uuid!=?1
        AND json_extract(b.content_json,'$.scope')=?2
        AND (?2!='view' OR json_extract(b.content_json,'$.viewId')=?3)
        AND json_extract(b.content_json,'$.startDate')<=?4
        AND json_extract(b.content_json,'$.endDate')>=?5)",
        rusqlite::params![
            next["id"].as_str(),
            content["scope"].as_str(),
            content["viewId"].as_str(),
            content["endDate"].as_str(),
            content["startDate"].as_str()
        ],
        |row| row.get(0),
    )?;
    if overlap {
        return Err(Error::Invalid(
            "This scope already has a writing goal for those dates.".to_owned(),
        ));
    }
    Ok(())
}

/// Capture effects once, then supply them explicitly to authoring calculations.
pub(crate) fn with_authoring_values<T, E>(
    prepare: impl FnOnce(
        &mut crate::domain::project::assemble::AuthoringValues<'_>,
    ) -> std::result::Result<T, E>,
) -> std::result::Result<T, E> {
    let timestamp = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    let mut ids = std::iter::repeat_with(|| uuid::Uuid::now_v7().to_string());
    prepare(&mut crate::domain::project::assemble::AuthoringValues {
        timestamp: &timestamp,
        ids: &mut ids,
    })
}

pub(crate) fn assemble_item(
    input: &Value,
    existing: Option<&Value>,
    timezone: &str,
) -> std::result::Result<Value, String> {
    assemble_owned(input.clone(), existing, timezone)
}

pub(crate) fn assemble_owned(
    input: Value,
    existing: Option<&Value>,
    timezone: &str,
) -> std::result::Result<Value, String> {
    with_authoring_values(|values| {
        crate::domain::project::assemble::assemble_owned_item(input, existing, timezone, values)
    })
}

/// Validate and commit a keyed paragraph through the ordinary canonical writer.
pub(crate) fn write_snippet(
    store: &Store,
    key: &str,
    paragraph: Value,
    expected_revision: Option<&str>,
) -> Result<Value> {
    use crate::domain::project::snippets;
    let id = snippets::item_id(key).map_err(Error::Invalid)?;
    snippets::validate_reference_paragraph(&paragraph, true).map_err(Error::Invalid)?;
    if expected_revision.is_none() && store.read(|tx| item::read_header(tx, &id))?.is_some() {
        return Err(Error::Invalid(format!("Snippet {key} already exists.")));
    }
    let mut input =
        serde_json::json!({"id":id,"meta":{"type":"snippet","title":key},"content":{"key":key}});
    input["content"]["paragraph"] = paragraph;
    let item = write(
        store,
        input,
        expected_revision,
        &chrono::Utc::now().date_naive().to_string(),
    )?;
    snippets::record(&item).map_err(Error::corrupt)
}
