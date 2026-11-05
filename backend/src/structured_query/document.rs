// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! Semantic query values. Editor identity and source ranges live outside them.
#[cfg(test)]
mod tests;

use std::collections::HashSet;

pub(crate) use crate::domain::project::query::{
    Comparison, ConditionDiagnostic, Predicate, QueryDocument, QuerySort, SortDirection, TextMode,
};

fn diagnostic(path: impl Into<String>, message: impl Into<String>) -> ConditionDiagnostic {
    ConditionDiagnostic {
        path: path.into(),
        code: "semantic".into(),
        message: message.into(),
        start: None,
        end: None,
    }
}

fn has_entry_separator(value: &str) -> bool {
    value
        .chars()
        .any(|c| matches!(c, ' ' | '\t' | '\r' | '\n' | '"'))
}

impl QueryDocument {
    pub(crate) fn validate(
        &self,
        schema: &super::schema::QuerySchema,
    ) -> Result<(), Vec<ConditionDiagnostic>> {
        use crate::domain::project::types::MetadataFieldValueType as Type;
        let mut errors = self.validate_fields(schema).err().unwrap_or_default();
        for (index, condition) in self.clauses.iter().enumerate() {
            if let Predicate::Field {
                field,
                value,
                operator,
            } = &condition.predicate
            {
                let message = match schema.value_type(field) {
                    Some(Type::Number | Type::Rating)
                        if value.parse::<serde_json::Number>().is_err() =>
                    {
                        Some("Enter a number.")
                    }
                    Some(Type::Checkbox) if !matches!(value.as_str(), "true" | "false") => {
                        Some("Choose true or false.")
                    }
                    Some(kind @ (Type::Date | Type::DateTime | Type::Timestamp)) => {
                        let builtin =
                            matches!(field.as_str(), "created" | "modified" | "completed");
                        let valid = super::absolute_date_prefix(value).is_some()
                            || super::is_relative_date(value)
                            || (kind != Type::Date
                                && (!builtin || *operator != Comparison::Match)
                                && chrono::DateTime::parse_from_rfc3339(value).is_ok());
                        if valid {
                            None
                        } else {
                            Some(
                                "Enter a valid date: YYYY, YYYY-MM or YYYY-MM-DD. Use supported relative dates or an ISO timestamp only where available.",
                            )
                        }
                    }
                    _ => None,
                };
                if let Some(message) = message {
                    errors.push(diagnostic(
                        format!("/clauses/{index}/predicate/value"),
                        message,
                    ));
                }
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }

    pub(crate) fn validate_fields(
        &self,
        fields: &HashSet<String>,
    ) -> Result<(), Vec<ConditionDiagnostic>> {
        let mut diagnostics = Vec::new();
        if self.version != 1 {
            diagnostics.push(diagnostic("/version", "Choose query document version 1."));
        }
        for (index, condition) in self.clauses.iter().enumerate() {
            let path = format!("/clauses/{index}/predicate");
            let predicate = &condition.predicate;
            if let Predicate::Field { field, .. } | Predicate::Has { field } = predicate
                && !super::is_builtin_field(field)
                && !fields.contains(field)
            {
                diagnostics.push(diagnostic(&path, format!("Unknown query field `{field}`")));
            }
            let invalid = match predicate {
                Predicate::Tag { key } if !super::is_tag_key(key) => {
                    Some("Tags must be lowercase snake-case paths.")
                }
                Predicate::Label { key } if !super::is_flat_facet_key(key) => {
                    Some("Labels must be lowercase snake-case keys.")
                }
                Predicate::Text {
                    value,
                    mode: TextMode::Terms,
                } if value.is_empty()
                    || has_entry_separator(value)
                    || value.contains(':')
                    || value.starts_with(['+', '-'])
                    || value.contains(['(', ')'])
                    || ["AND", "OR", "NOT"]
                        .iter()
                        .any(|word| value.eq_ignore_ascii_case(word)) =>
                {
                    Some("Use one text term, or choose Phrase for literal text.")
                }
                Predicate::Text {
                    value,
                    mode: TextMode::Phrase,
                } if value.is_empty() => Some("Enter a phrase."),
                Predicate::Field { value, .. } if value.is_empty() => {
                    Some("A field condition requires a value.")
                }
                Predicate::Field { field, value, .. }
                    if field == "type" && !super::item_type_values().any(|kind| kind == value) =>
                {
                    Some("Unknown item type.")
                }
                Predicate::Field { field, value, .. }
                    if field == "state" && !["active", "archived"].contains(&value.as_str()) =>
                {
                    Some("Unknown item state.")
                }
                Predicate::Field { field, value, .. }
                    if field == "tag" && !super::is_tag_key(value) =>
                {
                    Some("Tags must be lowercase snake-case paths.")
                }
                Predicate::Field { field, value, .. }
                    if field == "label" && !super::is_flat_facet_key(value) =>
                {
                    Some("Labels must be lowercase snake-case keys.")
                }
                _ => None,
            };
            if let Some(message) = invalid {
                diagnostics.push(diagnostic(format!("{path}/value"), message));
            }
        }
        let mut sort_fields = HashSet::new();
        for (index, sort) in self.sort.iter().enumerate() {
            if !super::is_sort_field(&sort.field, fields) {
                diagnostics.push(diagnostic(
                    format!("/sort/{index}/field"),
                    format!("Unknown sort field `{}`", sort.field),
                ));
            }
            if !sort_fields.insert(&sort.field) {
                diagnostics.push(diagnostic(
                    format!("/sort/{index}/field"),
                    "Choose each sort field once.",
                ));
            }
        }
        if diagnostics.is_empty() {
            Ok(())
        } else {
            Err(diagnostics)
        }
    }
}
