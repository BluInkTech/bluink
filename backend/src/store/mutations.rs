// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! Canonical item groups and every derived row share one commit boundary.
#[cfg(test)]
mod tests;
use std::collections::BTreeSet;

use rusqlite::{OptionalExtension, Transaction, params};
use serde_json::Value;

use crate::domain::project::{candidate::NamedKind, identity::ItemId, types::ItemType};
use crate::store::changes::{ChangeDomain, ChangeOperation};

use super::{Error, Result, activity, changes, fields, fts, git, item, projections, relations};

pub(crate) struct PreparedChange {
    value: Value,
    item: item::PreparedItem,
    projection: projections::Prepared,
    fts: fts::Prepared,
    expected: Option<String>,
}

impl PreparedChange {
    pub(crate) fn new(value: Value, expected: Option<String>) -> Result<Self> {
        let item = item::PreparedItem::new(&value)?;
        let projection = projections::Prepared::new(&value)?;
        let fts = fts::Prepared::new(&projection.document);
        Ok(Self {
            value,
            item,
            projection,
            fts,
            expected,
        })
    }
}

pub(crate) struct Delete {
    pub(crate) id: String,
    pub(crate) expected: String,
}

/// A caller can add a receipt or pending Git ref operation before this transaction
/// commits. Initial imports pass no activity day: importing is not local writing.
pub(crate) fn apply(
    transaction: &Transaction<'_>,
    updates: &[PreparedChange],
    deletes: &[Delete],
    activity_day: Option<&str>,
) -> Result<i64> {
    apply_with_domains(
        transaction,
        updates,
        deletes,
        activity_day,
        &BTreeSet::new(),
    )
}

pub(crate) fn apply_with_domains(
    transaction: &Transaction<'_>,
    updates: &[PreparedChange],
    deletes: &[Delete],
    activity_day: Option<&str>,
    additional_domains: &BTreeSet<ChangeDomain>,
) -> Result<i64> {
    let mut seen = BTreeSet::new();
    for id in updates
        .iter()
        .map(|change| change.projection.document.id.as_str())
        .chain(deletes.iter().map(|change| change.id.as_str()))
    {
        if !seen.insert(id) {
            return Err(Error::Invalid(
                "An item occurs twice in one change.".to_owned(),
            ));
        }
    }
    if seen.is_empty() && additional_domains.is_empty() {
        return Ok(transaction.query_row(
            "SELECT generation FROM project_state WHERE singleton=1",
            [],
            |row| row.get(0),
        )?);
    }
    let mut domains = additional_domains.clone();
    if !seen.is_empty() {
        domains.insert(ChangeDomain::Items);
    }
    let mut pending = Vec::new();
    install_updates(
        transaction,
        updates,
        activity_day,
        &mut domains,
        &mut pending,
    )?;
    let archived: Vec<_> = updates
        .iter()
        .filter(|change| {
            change.projection.document.item_type == "page"
                && change.projection.document.state == "archived"
        })
        .map(|change| change.projection.document.id.as_str())
        .collect();
    let cascaded = archive_tasks(transaction, &archived)?;
    for (id, revision) in &cascaded {
        pending.push(changes::Change {
            id,
            item_type: ItemType::Task,
            operation: ChangeOperation::Upsert,
            revision: Some(revision),
        });
    }
    for deletion in deletes {
        let kind = remove(transaction, deletion)?;
        domains_for(kind, &mut domains);
        pending.push(changes::Change {
            id: &deletion.id,
            item_type: kind,
            operation: ChangeOperation::Delete,
            revision: None,
        });
    }
    for change in updates {
        super::theme_packages::require_reference(transaction, &change.value)?;
        relations::validate(transaction, &change.value, &change.projection.document)?;
        check_removed_declaration(transaction, &change.value)?;
        protect_default_workspace(
            transaction,
            &change.projection.document.id,
            change.value["meta"]["state"] == "active",
        )?;
    }
    // Deletion guards also run after the group, so deleting an owner and its
    // dependency together is allowed without exposing an intermediate state.
    for deletion in deletes {
        check_deleted_target(transaction, &deletion.id)?;
    }
    let generation = changes::publish(transaction, &domains, &pending)?;
    for change in pending {
        if let Some(revision) = change.revision {
            git::mark_item_dirty(transaction, change.id, revision, generation)?;
        } else {
            let path = ItemId::try_from(change.id)
                .map_err(Error::Invalid)?
                .path(std::path::Path::new(""));
            git::mark_deleted(
                transaction,
                &path.to_string_lossy().replace('\\', "/"),
                generation,
            )?;
        }
    }
    Ok(generation)
}

/// Retain the exact revision, then remove the item and its own derived rows.
fn remove(transaction: &Transaction<'_>, deletion: &Delete) -> Result<ItemType> {
    let previous = item::read_header(transaction, &deletion.id)?.ok_or(Error::NotFound)?;
    item::check_revision(
        Some(&previous),
        item::ExpectedRevision::Exact(&deletion.expected),
    )?;
    let memberships = item::memberships(transaction, previous.id)?;
    item::preserve(transaction, &previous, &memberships)?;
    let kind: ItemType = serde_json::from_value(previous.value["meta"]["type"].clone())?;
    fts::remove(transaction, previous.id)?;
    fields::undeclare(transaction, previous.id)?;
    transaction.execute("DELETE FROM relation WHERE source_id=?1", [previous.id])?;
    transaction.execute("DELETE FROM item WHERE id=?1", [previous.id])?;
    Ok(kind)
}

fn install_updates<'a>(
    transaction: &Transaction<'_>,
    updates: &'a [PreparedChange],
    activity_day: Option<&str>,
    domains: &mut BTreeSet<ChangeDomain>,
    pending: &mut Vec<changes::Change<'a>>,
) -> Result<()> {
    let mut previous = Vec::with_capacity(updates.len());
    let mut ids = Vec::with_capacity(updates.len());
    for change in updates {
        let document = &change.projection.document;
        // One header and membership read serves these checks and the install.
        let header = item::read_header(transaction, &document.id)?;
        let memberships = header
            .as_ref()
            .map(|header| item::memberships(transaction, header.id))
            .transpose()?;
        let existing = header.as_ref().zip(memberships.as_ref());
        let old = existing
            .map(|(header, memberships)| {
                item::assemble(
                    transaction,
                    header.value.clone(),
                    header.body,
                    memberships,
                    matches!(document.item_type.as_str(), "task" | "goal" | "property"),
                )
            })
            .transpose()?;
        if let Some(old) = &old {
            if old["meta"]["type"] != change.value["meta"]["type"] {
                return Err(Error::Invalid(
                    "An item's type cannot be changed.".to_owned(),
                ));
            }
            if old["createdAt"] != change.value["createdAt"]
                || old["revision"] == change.value["revision"]
            {
                return Err(Error::Invalid(
                    "A write must preserve creation time and use a fresh revision.".to_owned(),
                ));
            }
        }
        let expected = change
            .expected
            .as_deref()
            .map_or(item::ExpectedRevision::New, item::ExpectedRevision::Exact);
        ids.push(change.item.install_over(transaction, expected, existing)?);
        previous.push(old);
    }
    // Declarations in this group become authoritative before dependent values.
    for (change, id) in updates.iter().zip(&ids) {
        if change.projection.document.item_type == "property" {
            if change.projection.document.state == "active" {
                fields::declare(transaction, *id, &change.value["content"])?;
            } else {
                fields::undeclare(transaction, *id)?;
            }
        }
    }
    for ((change, id), previous) in updates.iter().zip(ids).zip(previous) {
        let document = &change.projection.document;
        let meta = document
            .metadata
            .as_object()
            .ok_or_else(|| Error::Invalid("Item metadata must be an object.".to_owned()))?;
        fields::replace(
            transaction,
            id,
            meta,
            previous
                .as_ref()
                .and_then(|previous| previous["meta"].as_object()),
        )?;
        change.projection.install(transaction, id)?;
        change.fts.install(transaction, id)?;
        let kind: ItemType = serde_json::from_value(change.value["meta"]["type"].clone())?;
        domains_for(kind, domains);
        if let Some(day) = activity_day {
            activity::record_write(transaction, previous.as_ref(), &change.value, day)?;
            domains.insert(ChangeDomain::Activity);
        }
        pending.push(changes::Change {
            id: &document.id,
            item_type: kind,
            operation: ChangeOperation::Upsert,
            revision: Some(&document.revision),
        });
    }
    Ok(())
}

fn domains_for(kind: ItemType, domains: &mut BTreeSet<ChangeDomain>) {
    match kind {
        ItemType::Tag | ItemType::Label | ItemType::Property => {
            domains.insert(ChangeDomain::Fields);
        }
        ItemType::Conflict => {
            domains.insert(ChangeDomain::Decisions);
        }
        ItemType::Setting => {
            domains.insert(ChangeDomain::Settings);
        }
        _ => {}
    }
}

fn check_removed_declaration(transaction: &Transaction<'_>, value: &Value) -> Result<()> {
    if value["meta"]["state"] != "active"
        && let Some(kind) = value["meta"]["type"].as_str().and_then(NamedKind::parse)
        && let Some(key) = value["content"]["key"].as_str()
        && relations::used(transaction, kind, key)?
    {
        return Err(Error::Invalid(format!(
            "{} {key} is still used by other items.",
            kind.as_str()
        )));
    }
    Ok(())
}

fn check_deleted_target(transaction: &Transaction<'_>, uuid: &str) -> Result<()> {
    protect_default_workspace(transaction, uuid, false)?;
    let named: Option<(String, String)> = transaction
        .query_row(
            "SELECT named_kind,named_key FROM entity WHERE uuid=?1 AND named_kind IS NOT NULL",
            [uuid],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    if let Some((kind, key)) = named
        && let Some(kind) = NamedKind::parse(&kind)
        && relations::used(transaction, kind, &key)?
    {
        return Err(Error::Invalid(format!(
            "{} {key} is still used by other items.",
            kind.as_str()
        )));
    }
    let protecting:bool=transaction.query_row(
        "SELECT EXISTS(SELECT 1 FROM entity e JOIN relation r ON r.target_id=e.id JOIN item i ON i.id=r.source_id
         WHERE e.uuid=?1 AND r.kind IN ('task_owner','pin','publication_member'))",[uuid],|row|row.get(0),
    )?;
    if protecting {
        return Err(Error::Invalid(
            "Item is still required by another item.".to_owned(),
        ));
    }
    Ok(())
}

pub(crate) fn active_tasks(
    transaction: &Transaction<'_>,
    owner: &str,
) -> Result<Vec<(String, String)>> {
    let mut select=transaction.prepare_cached(
            "SELECT child.uuid,child.revision FROM task_projection t JOIN item child ON child.id=t.item_id
             JOIN entity parent ON parent.id=t.note_id WHERE parent.uuid=?1 AND child.state='active' ORDER BY child.uuid",
        )?;
    Ok(select
        .query_map([owner], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?)
}

fn archive_tasks(transaction: &Transaction<'_>, owners: &[&str]) -> Result<Vec<(String, String)>> {
    let mut changed = Vec::new();
    for owner in owners {
        for (id, revision) in active_tasks(transaction, owner)? {
            let patch = serde_json::Map::from_iter([(
                "state".to_owned(),
                Value::String("archived".to_owned()),
            )]);
            let previous = item::read_header(transaction, &id)?.ok_or(Error::NotFound)?;
            let memberships = item::memberships(transaction, previous.id)?;
            let header =
                item::edit_metadata_from(transaction, &previous, &memberships, &revision, &patch)?;
            let revision = header.value["revision"]
                .as_str()
                .ok_or_else(|| Error::Invalid("Task has no revision.".to_owned()))?
                .to_owned();
            changed.push((id, revision));
        }
    }
    Ok(changed)
}

#[cfg(test)]
pub(crate) fn metadata(
    transaction: &Transaction<'_>,
    uuid: &str,
    expected: &str,
    patch: &serde_json::Map<String, Value>,
) -> Result<item::Header> {
    let previous = item::read_header(transaction, uuid)?.ok_or(Error::NotFound)?;
    let memberships = item::memberships(transaction, previous.id)?;
    metadata_from(transaction, uuid, &previous, &memberships, expected, patch)
}

/// Header-only writes retain body, passages and body FTS postings. Title has its
/// own occurrence in the shared FTS table and can therefore change independently.
/// `previous` and `memberships` are the current rows the caller read in this
/// transaction; the edit changes neither the body nor the memberships.
pub(crate) fn metadata_from(
    transaction: &Transaction<'_>,
    uuid: &str,
    previous: &item::Header,
    memberships: &Value,
    expected: &str,
    patch: &serde_json::Map<String, Value>,
) -> Result<item::Header> {
    let header = item::edit_metadata_from(transaction, previous, memberships, expected, patch)?;
    let meta = header.value["meta"]
        .as_object()
        .ok_or_else(|| Error::Invalid("Item metadata must be an object.".to_owned()))?;
    fields::replace(
        transaction,
        header.id,
        meta,
        previous.value["meta"].as_object(),
    )?;
    if header.value["meta"]["title"] != previous.value["meta"]["title"] {
        fts::update_title(
            transaction,
            header.id,
            header.value["meta"]["title"].as_str().unwrap_or_default(),
        )?;
    }
    let kind: ItemType = serde_json::from_value(header.value["meta"]["type"].clone())?;
    protect_default_workspace(transaction, uuid, header.value["meta"]["state"] == "active")?;
    if let Some(named) = NamedKind::parse(kind.as_str())
        && header.value["meta"]["state"] != "active"
    {
        let key: String = transaction.query_row(
            "SELECT named_key FROM entity WHERE id=?1",
            [header.id],
            |row| row.get(0),
        )?;
        if relations::used(transaction, named, &key)? {
            return Err(Error::Invalid(format!(
                "{} {key} is still used by other items.",
                kind.as_str()
            )));
        }
        if kind == ItemType::Property {
            fields::undeclare(transaction, header.id)?;
        }
    } else if kind == ItemType::Property && previous.value["meta"]["state"] != "active" {
        let declaration = super::body::read(transaction, header.body)?;
        fields::declare(transaction, header.id, &declaration)?;
    }
    let mut domains = BTreeSet::from([ChangeDomain::Items]);
    domains_for(kind, &mut domains);
    let cascaded = if kind == ItemType::Page && header.value["meta"]["state"] == "archived" {
        archive_tasks(transaction, &[uuid])?
    } else {
        Vec::new()
    };
    if kind == ItemType::Task {
        let owner:String=transaction.query_row("SELECT e.uuid FROM task_projection t JOIN entity e ON e.id=t.note_id WHERE t.item_id=?1",[header.id],|row|row.get(0))?;
        let parent = item::read_header(transaction, &owner)?;
        crate::domain::project::task::validate_owner(
            &header.value,
            parent.as_ref().map(|parent| &parent.value),
        )
        .map_err(Error::Invalid)?;
        let due = meta
            .get("due_date")
            .filter(|value| !value.is_null())
            .map(|value| {
                fields::time_value(
                    crate::domain::project::types::MetadataFieldValueType::Date,
                    value.as_str().ok_or_else(|| {
                        Error::Invalid("Task due date must be a string.".to_owned())
                    })?,
                )
            })
            .transpose()?;
        transaction.execute(
            "UPDATE task_projection SET due_day=?2 WHERE item_id=?1",
            params![header.id, due],
        )?;
    }
    let revision = header.value["revision"]
        .as_str()
        .ok_or_else(|| Error::Invalid("Item has no revision.".to_owned()))?;
    let mut pending = vec![changes::Change {
        id: uuid,
        item_type: kind,
        operation: ChangeOperation::Upsert,
        revision: Some(revision),
    }];
    for (id, revision) in &cascaded {
        pending.push(changes::Change {
            id,
            item_type: ItemType::Task,
            operation: ChangeOperation::Upsert,
            revision: Some(revision),
        });
    }
    let generation = changes::publish(transaction, &domains, &pending)?;
    git::mark_item_dirty(transaction, uuid, revision, generation)?;
    for (id, revision) in &cascaded {
        git::mark_item_dirty(transaction, id, revision, generation)?;
    }
    Ok(header)
}

fn protect_default_workspace(
    transaction: &Transaction<'_>,
    uuid: &str,
    active: bool,
) -> Result<()> {
    if !active && transaction.query_row::<bool,_,_>(
        "SELECT EXISTS(SELECT 1 FROM project_record WHERE path='bluink.json' AND json_extract(canonical_json,'$.defaultWorkspaceId')=?1)",[uuid],|row|row.get(0),
    )? {return Err(Error::Invalid("Choose another default workspace before removing this one.".to_owned()));}
    Ok(())
}
