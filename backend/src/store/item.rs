// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! Small current headers and exact immutable revision references.
use rusqlite::{OptionalExtension, Transaction, params};
use serde_json::{Value, json};

use crate::domain::project::{item::validate_item, named_items};

use super::{Error, Result, body};

pub(crate) enum ExpectedRevision<'a> {
    New,
    Exact(&'a str),
}

pub(crate) struct PreparedItem {
    header: Value,
    body: body::PreparedBody,
    memberships: Value,
    named: Option<(String, String)>,
}

impl PreparedItem {
    /// Validate once at ingress and prepare immutable bytes outside the writer.
    pub(crate) fn new(value: &Value) -> Result<Self> {
        let id = text(value, "id")?;
        validate_item(value, id).map_err(Error::Invalid)?;
        let mut header = value.clone();
        let fields = header
            .as_object_mut()
            .ok_or_else(|| Error::Invalid("Item must be an object.".to_owned()))?;
        let content = fields
            .remove("content")
            .ok_or_else(|| Error::Invalid("Item has no content.".to_owned()))?;
        let mut memberships = json!({});
        for key in ["tags", "pins"] {
            if let Some(value) = fields.remove(key) {
                memberships[key] = value;
            }
        }
        let kind = text(&header["meta"], "type")?;
        let named = if named_items::is_named(kind) {
            Some((kind.to_owned(), text(&content, "key")?.to_owned()))
        } else {
            None
        };
        Ok(Self {
            header,
            body: body::PreparedBody::new(&content)?,
            memberships,
            named,
        })
    }

    /// Caller also applies projections, activity, receipts and feed before committing.
    pub(crate) fn install(
        &self,
        transaction: &Transaction<'_>,
        expected: ExpectedRevision<'_>,
    ) -> Result<i64> {
        let existing = read_header(transaction, text(&self.header, "id")?)?;
        let memberships = existing
            .as_ref()
            .map(|header| memberships(transaction, header.id))
            .transpose()?;
        self.install_over(
            transaction,
            expected,
            existing.as_ref().zip(memberships.as_ref()),
        )
    }

    /// `existing` is the current header and memberships, already read in this
    /// transaction by a caller that needs them for its own checks.
    pub(crate) fn install_over(
        &self,
        transaction: &Transaction<'_>,
        expected: ExpectedRevision<'_>,
        existing: Option<(&Header, &Value)>,
    ) -> Result<i64> {
        let uuid = text(&self.header, "id")?;
        check_revision(existing.map(|(header, _)| header), expected)?;
        let id = identity(
            transaction,
            uuid,
            self.named
                .as_ref()
                .map(|(kind, key)| (kind.as_str(), key.as_str())),
        )?;
        if let Some((header, memberships)) = existing {
            preserve(transaction, header, memberships)?;
        }
        let body = self
            .body
            .install_over(transaction, existing.map(|(header, _)| header.body))?;
        install_header(transaction, id, &self.header, body)?;
        if existing.map(|(_, memberships)| memberships) != Some(&self.memberships) {
            replace_memberships(transaction, id, &self.memberships)?;
        }
        Ok(id)
    }
}

pub(crate) struct Header {
    pub(crate) id: i64,
    pub(crate) value: Value,
    pub(crate) body: i64,
}

#[cfg(test)]
pub(crate) fn edit_metadata(
    transaction: &Transaction<'_>,
    uuid: &str,
    expected: &str,
    patch: &serde_json::Map<String, Value>,
) -> Result<Header> {
    let previous = read_header(transaction, uuid)?.ok_or(Error::NotFound)?;
    let memberships = memberships(transaction, previous.id)?;
    edit_metadata_from(transaction, &previous, &memberships, expected, patch)
}

/// A metadata edit validates and retains only the small header, never its body.
/// `previous` and `memberships` are the current rows the caller read in this
/// transaction.
pub(crate) fn edit_metadata_from(
    transaction: &Transaction<'_>,
    previous: &Header,
    memberships: &Value,
    expected: &str,
    patch: &serde_json::Map<String, Value>,
) -> Result<Header> {
    check_revision(Some(previous), ExpectedRevision::Exact(expected))?;
    let uuid = text(&previous.value, "id")?;
    let previous_kind = text(&previous.value["meta"], "type")?;
    if previous_kind == "conflict" {
        return Err(Error::Invalid(
            "Decisions must be changed through resolution.".to_owned(),
        ));
    }
    let mut metadata = previous.value["meta"]
        .as_object()
        .cloned()
        .ok_or_else(|| Error::Invalid("Item has invalid metadata.".to_owned()))?;
    metadata.extend(
        patch
            .iter()
            .map(|(key, value)| (key.clone(), value.clone())),
    );
    if metadata.get("type").and_then(Value::as_str) != Some(previous_kind) {
        return Err(Error::Invalid(
            "An item's type cannot be changed.".to_owned(),
        ));
    }
    serde_json::from_value::<crate::domain::project::types::ItemState>(
        metadata.get("state").cloned().unwrap_or(Value::Null),
    )?;
    crate::domain::project::item::validate_metadata(&metadata, uuid).map_err(Error::Invalid)?;
    preserve(transaction, previous, memberships)?;
    let mut header = Header {
        id: previous.id,
        value: previous.value.clone(),
        body: previous.body,
    };
    header.value["meta"] = Value::Object(metadata);
    header.value["revision"] = json!(uuid::Uuid::now_v7().to_string());
    header.value["modifiedAt"] =
        json!(chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true));
    install_header(transaction, header.id, &header.value, header.body)?;
    Ok(header)
}

/// The portable attachment paths an item owns, which leave with it on deletion.
pub(crate) fn owned_asset_paths(transaction: &Transaction<'_>, id: i64) -> Result<Vec<String>> {
    let mut statement = transaction
        .prepare_cached("SELECT path FROM asset_path WHERE owner_id=?1 ORDER BY path")?;
    let paths = statement
        .query_map([id], |row| row.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    Ok(paths)
}

pub(crate) fn read_header(transaction: &Transaction<'_>, uuid: &str) -> Result<Option<Header>> {
    let row = transaction
        .query_row(
            "SELECT id,header_json,body_id FROM item WHERE uuid = ?1",
            [uuid],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            },
        )
        .optional()?;
    row.map(|(id, value, body)| {
        Ok(Header {
            id,
            value: serde_json::from_str(&value)?,
            body,
        })
    })
    .transpose()
}

pub(crate) fn read(
    transaction: &Transaction<'_>,
    uuid: &str,
    with_content: bool,
) -> Result<Option<Value>> {
    read_header(transaction, uuid)?
        .map(|header| {
            let memberships = memberships(transaction, header.id)?;
            assemble(
                transaction,
                header.value,
                header.body,
                &memberships,
                with_content,
            )
        })
        .transpose()
}

pub(crate) fn read_revision(
    transaction: &Transaction<'_>,
    uuid: &str,
    revision: &str,
) -> Result<Option<Value>> {
    if let Some(current) = read_header(transaction, uuid)?
        && current.value["revision"].as_str() == Some(revision)
    {
        let memberships = memberships(transaction, current.id)?;
        return assemble(transaction, current.value, current.body, &memberships, true).map(Some);
    }
    let row = transaction.query_row(
        "SELECT r.header_json,r.body_id,r.memberships_json FROM revision r JOIN entity e ON e.id=r.item_id
         WHERE e.uuid=?1 AND r.revision_uuid=?2", params![uuid,revision], |row| Ok((row.get::<_,String>(0)?,row.get::<_,i64>(1)?,row.get::<_,String>(2)?)),
    ).optional()?;
    row.map(|(header, body, memberships)| {
        assemble(
            transaction,
            serde_json::from_str(&header)?,
            body,
            &serde_json::from_str(&memberships)?,
            true,
        )
    })
    .transpose()
}

pub(super) fn assemble(
    transaction: &Transaction<'_>,
    mut header: Value,
    body_id: i64,
    memberships: &Value,
    with_content: bool,
) -> Result<Value> {
    for key in ["tags", "pins"] {
        if let Some(value) = memberships.get(key) {
            header[key] = value.clone();
        }
    }
    if with_content {
        header["content"] = body::read(transaction, body_id)?;
    }
    Ok(header)
}

pub(crate) fn identity(
    transaction: &Transaction<'_>,
    uuid: &str,
    named: Option<(&str, &str)>,
) -> Result<i64> {
    let (kind, key) = named.map_or((None, None), |(kind, key)| (Some(kind), Some(key)));
    transaction.execute("INSERT INTO entity(uuid,named_kind,named_key) VALUES (?1,?2,?3) ON CONFLICT(uuid) DO NOTHING", params![uuid,kind,key])?;
    if let Some((kind, key)) = named {
        transaction.execute(
            "UPDATE entity SET named_kind=?2,named_key=?3 WHERE uuid=?1 AND named_kind IS NULL",
            params![uuid, kind, key],
        )?;
    }
    let (id, stored_kind, stored_key): (i64, Option<String>, Option<String>) = transaction
        .query_row(
            "SELECT id,named_kind,named_key FROM entity WHERE uuid=?1",
            [uuid],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )?;
    if named.is_some() && (stored_kind.as_deref() != kind || stored_key.as_deref() != key) {
        return Err(Error::Invalid("Named item identity changed.".to_owned()));
    }
    Ok(id)
}

pub(crate) fn check_revision(
    existing: Option<&Header>,
    expected: ExpectedRevision<'_>,
) -> Result<()> {
    match (existing, expected) {
        (None, ExpectedRevision::New) => Ok(()),
        (Some(header), ExpectedRevision::Exact(revision))
            if header.value["revision"].as_str() == Some(revision) =>
        {
            Ok(())
        }
        (None, ExpectedRevision::Exact(_)) => Err(Error::NotFound),
        _ => Err(Error::RevisionConflict),
    }
}

/// `memberships` must be the item's current relation set, as `memberships` reads it.
pub(crate) fn preserve(
    transaction: &Transaction<'_>,
    header: &Header,
    memberships: &Value,
) -> Result<()> {
    transaction.execute(
        "INSERT INTO revision(item_id,revision_uuid,header_json,body_id,memberships_json,superseded_ms) VALUES (?1,?2,?3,?4,?5,?6)",
        params![header.id,text(&header.value,"revision")?,serde_json::to_string(&header.value)?,header.body,serde_json::to_string(memberships)?,chrono::Utc::now().timestamp_millis()],
    )?;
    Ok(())
}

pub(crate) fn install_header(
    transaction: &Transaction<'_>,
    id: i64,
    header: &Value,
    body: i64,
) -> Result<()> {
    let (title, created, modified) = header_keys(header)?;
    transaction.execute(
        "INSERT INTO item(id,header_json,body_id,title_sort,created_ms,modified_ms) VALUES (?1,?2,?3,?4,?5,?6)
         ON CONFLICT(id) DO UPDATE SET header_json=excluded.header_json,body_id=excluded.body_id,title_sort=excluded.title_sort,created_ms=excluded.created_ms,modified_ms=excluded.modified_ms",
        params![id,serde_json::to_string(header)?,body,title,created,modified],
    )?;
    Ok(())
}

pub(super) fn header_keys(header: &Value) -> Result<(String, i64, i64)> {
    let title = super::normalize::sort_key(header["meta"]["title"].as_str().unwrap_or_default());
    Ok((
        title,
        instant(header, "createdAt")?,
        instant(header, "modifiedAt")?,
    ))
}

pub(super) fn memberships(transaction: &Transaction<'_>, id: i64) -> Result<Value> {
    let mut result = json!({});
    for (kind, key, column) in [("tag", "tags", "named_key"), ("pin", "pins", "uuid")] {
        // Identifiers are closed compile-time choices; user input is always bound.
        let mut statement = transaction.prepare_cached(&format!("SELECT e.{column} FROM relation r JOIN entity e ON e.id=r.target_id WHERE r.source_id=?1 AND r.kind=?2 ORDER BY e.{column}"))?;
        let values: Vec<String> = statement
            .query_map(params![id, kind], |row| row.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        if !values.is_empty() {
            result[key] = serde_json::to_value(values)?;
        }
    }
    Ok(result)
}

pub(super) fn replace_memberships(
    transaction: &Transaction<'_>,
    id: i64,
    memberships: &Value,
) -> Result<()> {
    transaction.execute(
        "DELETE FROM relation WHERE source_id=?1 AND kind IN ('tag','pin')",
        [id],
    )?;
    for (kind, key) in [("tag", "tags"), ("pin", "pins")] {
        if let Some(values) = memberships.get(key).and_then(Value::as_array) {
            for value in values {
                let value = value
                    .as_str()
                    .ok_or_else(|| Error::Invalid("Invalid authored membership.".to_owned()))?;
                let target = if kind == "tag" {
                    identity(
                        transaction,
                        &named_items::id(kind, value),
                        Some((kind, value)),
                    )?
                } else {
                    identity(transaction, value, None)?
                };
                transaction.execute("INSERT INTO relation(source_id,kind,target_id) VALUES (?1,?2,?3) ON CONFLICT DO NOTHING",params![id,kind,target])?;
            }
        }
    }
    Ok(())
}

fn instant(value: &Value, key: &str) -> Result<i64> {
    chrono::DateTime::parse_from_rfc3339(text(value, key)?)
        .map(|instant| instant.timestamp_millis())
        .map_err(|error| Error::Invalid(error.to_string()))
}

fn text<'a>(value: &'a Value, key: &str) -> Result<&'a str> {
    value
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| Error::Invalid(format!("Item has no {key}.")))
}
