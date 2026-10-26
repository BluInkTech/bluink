// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! Semantic query values. Editor identity and source ranges live outside them.
#[cfg(test)]
mod tests;

use std::collections::HashSet;

#[cfg(test)]
use serde::Serialize;

#[cfg(test)]
use super::Polarity;
#[cfg(test)]
use super::{QueryAst, QueryClause, QueryClauseKind};

pub(crate) use crate::engine::project::query::{
    Comparison, Condition, ConditionDiagnostic, Predicate, QueryDocument, QuerySort, SortDirection,
    StatePredicate, TextMode,
};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg(test)]
pub(crate) struct ConditionSource {
    pub(crate) path: String,
    pub(crate) start: usize,
    pub(crate) end: usize,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg(test)]
pub(crate) struct ParsedQuery {
    pub(crate) document: Option<QueryDocument>,
    pub(crate) diagnostics: Vec<ConditionDiagnostic>,
    pub(crate) source_map: Vec<ConditionSource>,
}

#[cfg(test)]
impl ParsedQuery {
    pub(crate) fn reject(&mut self, diagnostics: Vec<ConditionDiagnostic>) {
        self.document = None;
        for mut error in diagnostics {
            if let Some(source) = self.source_map.iter().find(|source| {
                error.path == source.path || error.path.starts_with(&format!("{}/", source.path))
            }) {
                error.start = Some(source.start);
                error.end = Some(source.end);
            }
            self.diagnostics.push(error);
        }
    }
}

#[cfg(test)]
impl Comparison {
    pub(crate) const fn symbol(self) -> &'static str {
        match self {
            Self::Match => "",
            Self::Gt => ">",
            Self::Gte => ">=",
            Self::Lt => "<",
            Self::Lte => "<=",
        }
    }

    #[cfg(test)]
    fn from_symbol(value: &str) -> Self {
        match value {
            ">" => Self::Gt,
            ">=" => Self::Gte,
            "<" => Self::Lt,
            "<=" => Self::Lte,
            _ => Self::Match,
        }
    }
}

#[cfg(test)]
impl StatePredicate {
    pub(crate) const fn key(self) -> &'static str {
        match self {
            Self::Archived => "archived",
            Self::Pinned => "pinned",
        }
    }
}

impl Condition {
    #[cfg(test)]
    fn from_clause(clause: &QueryClause) -> Self {
        let predicate = match &clause.kind {
            QueryClauseKind::Text { value, quoted } => Predicate::Text {
                value: value.clone(),
                mode: if *quoted {
                    TextMode::Phrase
                } else {
                    TextMode::Terms
                },
            },
            QueryClauseKind::Tag(key) => Predicate::Tag { key: key.clone() },
            QueryClauseKind::Label(key) => Predicate::Label { key: key.clone() },
            QueryClauseKind::Has(field) => Predicate::Has {
                field: field.clone(),
            },
            QueryClauseKind::Is(value) => Predicate::Is {
                value: if value == "pinned" {
                    StatePredicate::Pinned
                } else {
                    StatePredicate::Archived
                },
            },
            QueryClauseKind::Field {
                field,
                operator,
                value,
            } => Predicate::Field {
                field: field.clone(),
                operator: Comparison::from_symbol(operator),
                value: value.clone(),
            },
        };
        Self {
            polarity: clause.polarity,
            predicate,
        }
    }
}

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
    pub(crate) fn cache_identity(&self) -> crate::application::error::Result<String> {
        let semantic =
            serde_json::to_string(self).map_err(crate::application::error::internal_error)?;
        let relative = self.clauses.iter().any(|clause| matches!(&clause.predicate,
            Predicate::Field { field, value, .. } if matches!(field.as_str(), "created" | "modified" | "completed")
                && matches!(value.as_str(), "today" | "yesterday" | "last_7d")));
        // Native relative dates use UTC. A midnight must not reuse yesterday's
        // membership simply because no project record changed.
        Ok(if relative {
            format!("{semantic}:utc:{}", chrono::Utc::now().format("%Y-%m-%d"))
        } else {
            semantic
        })
    }

    #[cfg(test)]
    pub(crate) fn from_ast(ast: &QueryAst) -> Self {
        Self {
            version: 1,
            clauses: ast.clauses.iter().map(Condition::from_clause).collect(),
            sort: ast
                .sort
                .iter()
                .flatten()
                .map(|sort| QuerySort {
                    field: sort.field.clone(),
                    direction: if sort.descending {
                        SortDirection::Desc
                    } else {
                        SortDirection::Asc
                    },
                })
                .collect(),
        }
    }

    pub(crate) fn validate(
        &self,
        schema: &super::schema::QuerySchema,
    ) -> Result<(), Vec<ConditionDiagnostic>> {
        use crate::engine::project::types::MetadataFieldValueType as Type;
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
                        let relative =
                            matches!(field.as_str(), "created" | "modified" | "completed");
                        let valid = super::absolute_date_prefix(value).is_some()
                            || (relative
                                && matches!(value.as_str(), "today" | "yesterday" | "last_7d"))
                            || (kind != Type::Date
                                && (!relative || *operator != Comparison::Match)
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

    #[cfg(test)]
    pub(crate) fn format(&self) -> String {
        let mut parts = Vec::new();
        for condition in &self.clauses {
            let prefix = if condition.polarity == Polarity::Excluded {
                "-"
            } else {
                "+"
            };
            let value = match &condition.predicate {
                Predicate::Text { value, mode } => {
                    if *mode == TextMode::Phrase {
                        super::escaped_value(value)
                    } else {
                        value.clone()
                    }
                }
                Predicate::Tag { key } => format!("#{key}"),
                Predicate::Label { key } => format!("@{key}"),
                Predicate::Has { field } => format!("has:{field}"),
                Predicate::Is { value } => format!("is:{}", value.key()),
                Predicate::Field {
                    field,
                    operator: Comparison::Match,
                    value,
                } => format!("{field}:{}", super::escaped_value(value)),
                Predicate::Field {
                    field,
                    operator,
                    value,
                } => format!("{field}:{}{value}", operator.symbol()),
            };
            parts.push(format!("{prefix}{value}"));
        }
        if !self.sort.is_empty() {
            parts.push(format!(
                "sort:{}",
                self.sort
                    .iter()
                    .map(|sort| format!(
                        "{}-{}",
                        sort.field,
                        if sort.direction == SortDirection::Desc {
                            "desc"
                        } else {
                            "asc"
                        }
                    ))
                    .collect::<Vec<_>>()
                    .join(",")
            ));
        }
        parts.join(" ")
    }
}

#[cfg(test)]
pub(crate) fn parse(source: &str, fields: &HashSet<String>) -> ParsedQuery {
    let ast = match super::parse_bluink_query(source) {
        Ok(ast) => ast,
        Err(error) => {
            return ParsedQuery {
                document: None,
                diagnostics: vec![ConditionDiagnostic {
                    path: "/entry".into(),
                    code: error.code,
                    message: error.message,
                    start: Some(utf16_offset(source, error.span.start)),
                    end: Some(utf16_offset(source, error.span.end)),
                }],
                source_map: Vec::new(),
            };
        }
    };
    // Validate before converting enum values so unknown is: tokens cannot be
    // accidentally normalised into a different, valid predicate.
    if let Err(semantic) = super::validate_ast_semantics(&ast, fields) {
        let span = semantic.span.unwrap_or_else(|| super::QuerySpan {
            start: 0,
            end: source.len().max(1),
        });
        let mut error = diagnostic("/entry", semantic.message);
        error.start = Some(utf16_offset(source, span.start));
        error.end = Some(utf16_offset(source, span.end));
        return ParsedQuery {
            document: None,
            diagnostics: vec![error],
            source_map: Vec::new(),
        };
    }
    let document = QueryDocument::from_ast(&ast);
    let source_map = ast
        .clauses
        .iter()
        .enumerate()
        .map(|(index, clause)| ConditionSource {
            path: format!("/clauses/{index}"),
            start: utf16_offset(source, clause.span.start),
            end: utf16_offset(source, clause.span.end),
        })
        .collect();
    let diagnostics = document.validate_fields(fields).err().unwrap_or_default();
    let mut parsed = ParsedQuery {
        document: Some(document),
        diagnostics: Vec::new(),
        source_map,
    };
    if !diagnostics.is_empty() {
        parsed.reject(diagnostics);
    }
    parsed
}

#[cfg(test)]
fn utf16_offset(source: &str, byte: usize) -> usize {
    source
        .char_indices()
        .take_while(|(offset, _)| *offset < byte)
        .map(|(_, character)| character.len_utf16())
        .sum()
}

/// Test authoring shorthand only. Production RPCs and persisted definitions
/// accept structured documents exclusively; there is no production text-query API.
#[cfg(test)]
pub(crate) fn test_query(source: &str) -> QueryDocument {
    QueryDocument::from_ast(&super::parse_bluink_query(source).expect("Valid query fixture syntax"))
}
