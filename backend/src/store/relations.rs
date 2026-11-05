// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! Relationship decisions use one canonical SQL snapshot, with no file candidates.
use std::collections::BTreeSet;

use rusqlite::{Transaction, params};
use serde_json::Value;

use crate::domain::project::candidate::{Catalogue, Dependency, NamedKind, publication_targets};
use crate::domain::project::properties::ValidatedDeclaration;
use crate::domain::project::types::{ItemState, ItemType};
use crate::store::projections::document::SearchDocument;

use super::{Error, Result, body, item};

/// Recheck current dependency states after all members of the change are installed.
pub(crate) fn validate(
    transaction: &Transaction<'_>,
    value: &Value,
    document: &SearchDocument,
) -> Result<()> {
    let mut dependencies = BTreeSet::new();
    for (key, value) in value["meta"]
        .as_object()
        .ok_or_else(|| Error::Invalid("Item metadata must be an object.".to_owned()))?
    {
        if !value.is_null() {
            dependencies.insert(Dependency::named(NamedKind::Property, key));
        }
    }
    for key in &document.pins {
        dependencies.insert(Dependency::Item(key.clone()));
    }
    for key in document
        .snippet_citation_targets
        .iter()
        .chain(&document.snippet_embed_targets)
    {
        dependencies.insert(Dependency::named(NamedKind::Snippet, key));
    }
    if let Some(task) = &document.task {
        dependencies.insert(Dependency::Item(task.note_id.clone()));
    }
    dependencies.extend(publication_targets(value).into_iter().map(Dependency::Item));
    validate_dependencies(transaction, value, &dependencies)
}

pub(crate) fn validate_dependencies(
    transaction: &Transaction<'_>,
    value: &Value,
    dependencies: &BTreeSet<Dependency>,
) -> Result<()> {
    let mut catalogue = Catalogue::default();
    // Free tags and labels have no target state for the catalogue to check.
    let ids: BTreeSet<_> = dependencies
        .iter()
        .filter(|dependency| {
            !matches!(
                dependency,
                Dependency::Named {
                    kind: NamedKind::Tag | NamedKind::Label,
                    ..
                }
            )
        })
        .map(Dependency::target_id)
        .collect();
    for id in ids {
        if let Some(header) = item::read_header(transaction, &id)? {
            let kind: ItemType = serde_json::from_value(header.value["meta"]["type"].clone())?;
            let state: ItemState = serde_json::from_value(header.value["meta"]["state"].clone())?;
            let content = if kind == ItemType::Property && state == ItemState::Active {
                Some(body::read(transaction, header.body)?)
            } else {
                None
            };
            let declaration = content
                .as_ref()
                .map(|content| {
                    let key = content["key"]
                        .as_str()
                        .ok_or_else(|| Error::Invalid("Property has no key.".to_owned()))?;
                    Ok::<_, Error>((
                        key,
                        ValidatedDeclaration::read(content).map_err(Error::Invalid)?,
                    ))
                })
                .transpose()?;
            catalogue
                .observe_summary(&id, kind, state, declaration)
                .map_err(Error::Invalid)?;
        }
    }
    if let Some(issue) = catalogue
        .check_captured(value, dependencies)
        .map_err(Error::Invalid)?
        .first()
    {
        return Err(Error::Invalid(issue.to_string()));
    }
    Ok(())
}

/// The usage policy includes templates and archived snippet owners exactly as before.
pub(crate) fn used(transaction: &Transaction<'_>, kind: NamedKind, key: &str) -> Result<bool> {
    if kind == NamedKind::Property {
        return Ok(transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM field_definition f JOIN field_presence p ON p.field_id=f.id
             JOIN item i ON i.id=p.item_id WHERE f.key=?1 AND p.value_state!='null' AND i.state='active')",
            [key],|row|row.get(0),
        )?);
    }
    let target = crate::domain::project::named_items::id(kind.as_str(), key);
    let (first, second) = match kind {
        NamedKind::Tag => ("tag", "tag"),
        NamedKind::Label => ("label", "label"),
        NamedKind::Snippet => ("snippet_citation", "snippet_embed"),
        NamedKind::Property => {
            return Err(Error::Invalid(
                "Property uses typed projections.".to_owned(),
            ));
        }
    };
    let documents_only = kind != NamedKind::Tag;
    let active_only = !kind.protect_archived_owners();
    Ok(transaction.query_row(
        "SELECT EXISTS(SELECT 1 FROM entity e JOIN relation r ON r.target_id=e.id JOIN item i ON i.id=r.source_id
         WHERE e.uuid=?1 AND r.kind IN (?2,?3) AND (?4=0 OR i.state='active') AND (?5=0 OR i.type IN ('page','template')))",
        params![target,first,second,active_only,documents_only],|row|row.get(0),
    )?)
}
