// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! Small typed projections shared by filtering, sorting, presence and facets.
use std::collections::{BTreeMap, BTreeSet};

use rusqlite::{OptionalExtension, Transaction, params};
use serde_json::{Map, Value};

use super::{Error, Result, normalize};
use crate::domain::project::types::{FieldSource, MetadataFieldValueType as Type};

#[derive(Clone, Debug)]
pub(crate) struct Definition {
    pub(crate) id: i64,
    pub(crate) kind: Type,
    pub(crate) declared: bool,
}

pub(crate) fn definitions(tx: &Transaction<'_>) -> Result<BTreeMap<String, Definition>> {
    let mut statement = tx.prepare(
        "SELECT f.id,f.key,f.value_type,
            CASE WHEN f.declaration_id IS NULL THEN (SELECT min(observed_type) FROM field_presence WHERE field_id=f.id AND observed_type IS NOT NULL) END,
            CASE WHEN f.declaration_id IS NULL THEN (SELECT max(observed_type) FROM field_presence WHERE field_id=f.id AND observed_type IS NOT NULL) END
         FROM field_definition f
        WHERE f.declaration_id IS NOT NULL OR EXISTS(SELECT 1 FROM field_presence p
        JOIN item owner ON owner.id=p.item_id WHERE p.field_id=f.id AND owner.state='active')",
    )?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(1)?,
            row.get::<_, i64>(0)?,
            row.get::<_, Option<String>>(2)?,
            row.get::<_, Option<String>>(3)?,
            row.get::<_, Option<String>>(4)?,
        ))
    })?;
    rows.map(|row| {
        let (key, id, declared_kind, first, last) = row?;
        let declared = declared_kind.is_some();
        let inferred = if first == last {
            first.as_deref()
        } else {
            None
        };
        let kind = declared_kind.as_deref().or(inferred).unwrap_or("text");
        let kind = serde_json::from_value(Value::String(kind.to_owned()))?;
        Ok((key, Definition { id, kind, declared }))
    })
    .collect()
}

/// Declaration order changes replace only these small rows, never note values.
pub(crate) fn declare(tx: &Transaction<'_>, declaration_id: i64, content: &Value) -> Result<()> {
    crate::domain::project::properties::validate_content(content).map_err(Error::Invalid)?;
    let key = content["key"]
        .as_str()
        .ok_or_else(|| Error::Invalid("Property has no key.".into()))?;
    let kind = content["valueType"]
        .as_str()
        .ok_or_else(|| Error::Invalid("Property has no type.".into()))?;
    let old: Option<(i64, Option<String>)> = tx
        .query_row(
            "SELECT id,value_type FROM field_definition WHERE key=?1",
            [key],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    if let Some((id, previous)) = &old {
        let existing = tx
            .prepare_cached("SELECT option_key FROM field_option WHERE field_id=?1")?
            .query_map([id], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<BTreeSet<_>>>()?;
        let options = content["options"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect::<BTreeSet<_>>();
        let restricts_choices =
            !options.is_empty() && (existing.is_empty() || !existing.is_subset(&options));
        if previous.as_deref() != Some(kind) || restricts_choices {
            let declaration =
                crate::domain::project::properties::ValidatedDeclaration::read(content)
                    .map_err(Error::Invalid)?;
            let mut owners = tx.prepare("SELECT i.header_json FROM field_presence p JOIN item i ON i.id=p.item_id WHERE p.field_id=?1")?;
            let owners = owners.query_map([id], |row| row.get::<_, String>(0))?;
            for owner in owners {
                let header: Value = serde_json::from_str(&owner?)?;
                let value = &header["meta"][key];
                if !value.is_null() {
                    declaration.validate(value).map_err(Error::Invalid)?;
                }
            }
        }
    }
    tx.execute("INSERT INTO field_definition(key,value_type,declaration_id,normalization_version) VALUES(?1,?2,?3,?4)
        ON CONFLICT(key) DO UPDATE SET value_type=excluded.value_type,declaration_id=excluded.declaration_id,normalization_version=excluded.normalization_version",
        params![key,kind,declaration_id,normalize::VERSION])?;
    let id: i64 = tx.query_row(
        "SELECT id FROM field_definition WHERE key=?1",
        [key],
        |row| row.get(0),
    )?;
    tx.execute("DELETE FROM field_option WHERE field_id=?1", [id])?;
    let mut insert = tx.prepare_cached(
        "INSERT INTO field_option(field_id,option_key,position) VALUES(?1,?2,?3)",
    )?;
    for (position, option) in content["options"]
        .as_array()
        .into_iter()
        .flatten()
        .enumerate()
    {
        insert.execute(params![
            id,
            option.as_str(),
            i64::try_from(position).map_err(|_| Error::Invalid("Too many choices.".into()))?
        ])?;
    }
    Ok(())
}

pub(crate) fn undeclare(tx: &Transaction<'_>, declaration_id: i64) -> Result<()> {
    tx.execute("DELETE FROM field_option WHERE field_id IN (SELECT id FROM field_definition WHERE declaration_id=?1)",[declaration_id])?;
    tx.execute("DELETE FROM field_definition WHERE declaration_id=?1 AND NOT EXISTS (SELECT 1 FROM field_presence p WHERE p.field_id=field_definition.id)",[declaration_id])?;
    tx.execute(
        "UPDATE field_definition SET declaration_id=NULL,value_type=NULL WHERE declaration_id=?1",
        [declaration_id],
    )?;
    Ok(())
}

fn inferred(value: &Value) -> Type {
    match value {
        Value::Number(_) => Type::Number,
        Value::Bool(_) => Type::Checkbox,
        Value::Array(values) if values.iter().all(Value::is_string) => Type::MultiSelect,
        _ => Type::Text,
    }
}

fn field_id(tx: &Transaction<'_>, key: &str) -> Result<i64> {
    tx.execute("INSERT INTO field_definition(key,normalization_version) VALUES(?1,?2) ON CONFLICT(key) DO NOTHING", params![key,normalize::VERSION])?;
    Ok(tx.query_row(
        "SELECT id FROM field_definition WHERE key=?1",
        [key],
        |row| row.get(0),
    )?)
}

/// Changed keys only. The caller already validated the canonical metadata.
pub(crate) fn replace(
    tx: &Transaction<'_>,
    item_id: i64,
    metadata: &Map<String, Value>,
    previous: Option<&Map<String, Value>>,
) -> Result<()> {
    let keys: BTreeSet<_> = metadata
        .keys()
        .chain(previous.into_iter().flat_map(Map::keys))
        .collect();
    for key in keys {
        if matches!(key.as_str(), "type" | "state" | "title" | "propertyKeys")
            || previous.is_some_and(|old| old.get(key) == metadata.get(key))
        {
            continue;
        }
        let value = metadata.get(key);
        if let Some(value) = value {
            validate_field(tx, key, value)?;
        }
        let field = field_id(tx, key)?;
        tx.execute(
            "DELETE FROM field_presence WHERE item_id=?1 AND field_id=?2",
            params![item_id, field],
        )?;
        let Some(value) = value else {
            continue;
        };
        let state = if value.is_null() {
            "null"
        } else if !has_value(value) {
            "empty"
        } else {
            "value"
        };
        tx.execute(
            "INSERT INTO field_presence(item_id,field_id,value_state,observed_type) VALUES(?1,?2,?3,?4)",
            params![item_id, field, state, (!value.is_null()).then(||inferred(value).as_str())],
        )?;
        if state != "value" {
            continue;
        }
        let values = value
            .as_array()
            .map_or_else(|| vec![value], |values| values.iter().collect());
        let mut position = 0usize;
        for member in values {
            project_member(tx, item_id, field, member, &mut position)?;
        }
    }
    Ok(())
}

fn has_value(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Array(values) => values.iter().any(has_value),
        Value::Object(values) => values.values().any(has_value),
        _ => true,
    }
}

fn validate_field(tx: &Transaction<'_>, key: &str, value: &Value) -> Result<()> {
    if value.is_null() {
        return Ok(());
    }
    let body:Option<i64> = tx.prepare_cached("SELECT i.body_id FROM field_definition f JOIN item i ON i.id=f.declaration_id WHERE f.key=?1 AND i.state='active'")?
        .query_row([key],|row|row.get(0)).optional()?;
    if let Some(body) = body {
        let content: String = tx
            .prepare_cached("SELECT content_json FROM body WHERE id=?1")?
            .query_row([body], |row| row.get(0))?;
        let content = serde_json::from_str(&content)?;
        crate::domain::project::properties::ValidatedDeclaration::read(&content)
            .and_then(|declaration| declaration.validate(value))
            .map_err(Error::Invalid)?;
    }
    Ok(())
}

fn project_member(
    tx: &Transaction<'_>,
    item_id: i64,
    field: i64,
    value: &Value,
    position: &mut usize,
) -> Result<()> {
    let mut text = None;
    let mut choice_sort = None;
    let mut number = None;
    let mut time = None;
    match value {
        Value::String(value) => {
            text = Some(value.clone());
            let sort = normalize::sort_key(value);
            choice_sort = (sort != *value).then_some(sort);
            // A declared date/instant accepts these fixed-width portable forms.
            // Keep its source spelling so removing a declaration needs no rewrites.
            if value.as_bytes().get(4) == Some(&b'-') {
                time = match value.len() {
                    10 => time_value(Type::Date, value).ok(),
                    24 => time_value(Type::DateTime, value).ok(),
                    _ => None,
                };
            }
            let tokens = normalize::tokens(value);
            let mut end = *position;
            let mut insert = tx.prepare_cached("INSERT OR IGNORE INTO field_token(field_id,item_id,position,term) VALUES(?1,?2,?3,?4)")?;
            for token in tokens {
                let absolute = position.saturating_add(token.position);
                insert.execute(params![
                    field,
                    item_id,
                    i64::try_from(absolute)
                        .map_err(|_| Error::Invalid("Too many field tokens.".into()))?,
                    token.text
                ])?;
                end = absolute.saturating_add(1);
            }
            *position = end.saturating_add(1);
        }
        Value::Number(value) => number = value.as_f64(),
        Value::Bool(value) => number = Some(if *value { 1.0 } else { 0.0 }),
        _ => return Ok(()),
    }
    tx.execute("INSERT OR IGNORE INTO field_value(item_id,field_id,text_value,number_value,time_value,choice_sort) VALUES(?1,?2,?3,?4,?5,?6)", params![item_id,field,text,number,time,choice_sort])?;
    Ok(())
}

pub(crate) fn time_value(kind: Type, value: &str) -> Result<i64> {
    if kind == Type::Date {
        let date = chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d")
            .map_err(|error| Error::Invalid(error.to_string()))?;
        Ok(date
            .signed_duration_since(
                chrono::NaiveDate::from_ymd_opt(1970, 1, 1)
                    .ok_or_else(|| Error::Invalid("Invalid epoch.".into()))?,
            )
            .num_days())
    } else {
        chrono::DateTime::parse_from_rfc3339(value)
            .map(|date| date.timestamp_millis())
            .map_err(|error| Error::Invalid(error.to_string()))
    }
}

/// Capability data is small declaration data, independent of author document bodies.
pub(crate) fn schema(tx: &Transaction<'_>) -> Result<crate::structured_query::schema::QuerySchema> {
    let definitions = definitions(tx)?;
    let mut fields = vec![crate::catalogue::MetadataField {
        key: "summary".into(),
        revision: None,
        value_type: Type::Text,
        source: FieldSource::Builtin,
        system: None,
        options: None,
        option_appearance: None,
    }];
    for (key, definition) in definitions {
        if key == "summary" {
            continue;
        }
        let mut options = tx.prepare_cached(
            "SELECT option_key FROM field_option WHERE field_id=?1 ORDER BY position",
        )?;
        let options = options
            .query_map([definition.id], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let declaration = tx.query_row("SELECT i.revision,b.content_json FROM field_definition f JOIN item i ON i.id=f.declaration_id JOIN body b ON b.id=i.body_id WHERE f.id=?1",[definition.id],|row| Ok((row.get::<_,String>(0)?,row.get::<_,String>(1)?))).optional()?;
        let (revision, appearance) = if let Some((revision, content)) = declaration {
            let content: Value = serde_json::from_str(&content)?;
            (
                Some(revision),
                content
                    .get("optionAppearance")
                    .map(|appearance| serde_json::from_value(appearance.clone()))
                    .transpose()?,
            )
        } else {
            (None, None)
        };
        let system = crate::domain::project::config::is_required_builtin_property(&key);
        fields.push(crate::catalogue::MetadataField {
            key,
            revision,
            value_type: definition.kind,
            source: if system {
                FieldSource::Builtin
            } else if definition.declared {
                FieldSource::Registered
            } else {
                FieldSource::Inferred
            },
            system: system.then_some(true),
            options: (!options.is_empty()).then_some(options),
            option_appearance: appearance,
        });
    }
    Ok(crate::structured_query::schema::QuerySchema::from_metadata(
        fields,
    ))
}
