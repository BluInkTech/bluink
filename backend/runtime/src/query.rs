// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

pub(crate) mod document;
mod exclusion;
pub(crate) mod schema;
pub(crate) mod sort;

use std::collections::HashSet;

use crate::application::error::Result;
use document::{Comparison, Predicate, SortDirection, StatePredicate, TextMode};
#[cfg(test)]
use pest::Parser;
#[cfg(test)]
use pest_derive::Parser;
#[cfg(test)]
use serde::Serialize;
use tantivy::query::{AllQuery, BooleanQuery, ExistsQuery, Occur, Query, QueryParser, TermQuery};
use tantivy::schema::IndexRecordOption;
use tantivy::{Index, Term};

use crate::application::error::{internal_error, invalid_request};
use crate::model::SortSpec;

#[cfg(test)]
#[derive(Parser)]
#[grammar = "runtime/src/query.pest"]
struct BluinkParser;

#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(test, derive(ts_rs::TS))]
pub(crate) struct QuerySpan {
    pub(crate) start: usize,
    pub(crate) end: usize,
}

#[cfg(test)]
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub(crate) struct QueryDiagnostic {
    pub(crate) code: String,
    pub(crate) message: String,
    pub(crate) severity: String,
    #[serde(flatten)]
    pub(crate) span: QuerySpan,
}

pub(crate) use crate::engine::project::query::Polarity;

#[cfg(test)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum QueryClauseKind {
    Text {
        value: String,
        quoted: bool,
    },
    Tag(String),
    Label(String),
    Has(String),
    Is(String),
    Field {
        field: String,
        operator: String,
        value: String,
    },
}

#[cfg(test)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct QueryClause {
    pub(crate) polarity: Polarity,
    pub(crate) kind: QueryClauseKind,
    pub(crate) span: QuerySpan,
}

#[cfg(test)]
#[derive(Debug, Clone)]
pub(crate) struct QueryAst {
    pub(crate) clauses: Vec<QueryClause>,
    pub(crate) sort: Option<Vec<SortSpec>>,
}
#[expect(
    clippy::needless_pass_by_value,
    reason = "The error adapter consumes the parser error passed by map_err."
)]
#[cfg(test)]
fn query_error(source: &str, error: pest::error::Error<Rule>) -> QueryDiagnostic {
    use pest::error::InputLocation;
    let (start, end) = match error.location {
        InputLocation::Pos(position) => (position, position.saturating_add(1)),
        InputLocation::Span((start, end)) => (start, end.max(start.saturating_add(1))),
    };
    QueryDiagnostic {
        code: "syntax".to_owned(),
        message: error.to_string(),
        severity: "error".to_owned(),
        span: QuerySpan {
            start,
            end: end.min(source.len()),
        },
    }
}

#[cfg(test)]
fn unquote(value: &str) -> String {
    if value.starts_with('"') && value.ends_with('"') && value.len() >= 2 {
        value[1..value.len() - 1]
            .replace(r#"\""#, "\"")
            .replace(r"\\", "\\")
    } else {
        value.to_owned()
    }
}

#[expect(
    clippy::too_many_lines,
    reason = "Keep the complete format or operation dispatch together for contract review."
)]
#[cfg(test)]
pub(crate) fn parse_bluink_query(source: &str) -> std::result::Result<QueryAst, QueryDiagnostic> {
    let pairs =
        BluinkParser::parse(Rule::query, source).map_err(|error| query_error(source, error))?;
    let mut clauses = Vec::new();
    let mut sort = None;
    for pair in pairs {
        for item in pair.into_inner() {
            match item.as_rule() {
                Rule::selection_clause => {
                    let span = QuerySpan {
                        start: item.as_span().start(),
                        end: item.as_span().end(),
                    };
                    let mut polarity = Polarity::Required;
                    let mut kind = None;
                    for part in item.into_inner() {
                        match part.as_rule() {
                            Rule::prefix => {
                                polarity = if part.as_str() == "-" {
                                    Polarity::Excluded
                                } else {
                                    Polarity::Required
                                }
                            }
                            Rule::tag => {
                                kind = Some(QueryClauseKind::Tag(part.as_str()[1..].to_owned()));
                            }
                            Rule::label => {
                                kind = Some(QueryClauseKind::Label(part.as_str()[1..].to_owned()));
                            }
                            Rule::has_clause => {
                                kind = Some(QueryClauseKind::Has(part.as_str()[4..].to_owned()));
                            }
                            Rule::is_clause => {
                                kind = Some(QueryClauseKind::Is(part.as_str()[3..].to_owned()));
                            }
                            Rule::field_clause => {
                                let mut field = String::new();
                                let mut operator = String::new();
                                let mut value = String::new();
                                for field_part in part.into_inner() {
                                    match field_part.as_rule() {
                                        Rule::identifier if field.is_empty() => {
                                            field_part.as_str().clone_into(&mut field);
                                        }
                                        Rule::value => {
                                            value = unquote(field_part.as_str());
                                            for candidate in [">=", "<=", ">", "<"] {
                                                if value.starts_with(candidate) {
                                                    candidate.clone_into(&mut operator);
                                                    value.drain(..candidate.len());
                                                    break;
                                                }
                                            }
                                        }
                                        _ => {}
                                    }
                                }
                                kind = Some(QueryClauseKind::Field {
                                    field,
                                    operator,
                                    value,
                                });
                            }
                            Rule::phrase => {
                                kind = Some(QueryClauseKind::Text {
                                    value: unquote(part.as_str()),
                                    quoted: true,
                                });
                            }
                            Rule::bare => {
                                kind = Some(QueryClauseKind::Text {
                                    value: part.as_str().to_owned(),
                                    quoted: false,
                                });
                            }
                            _ => {}
                        }
                    }
                    if let Some(kind) = kind {
                        clauses.push(QueryClause {
                            polarity,
                            kind,
                            span,
                        });
                    }
                }
                Rule::sort_clause => {
                    let value = item.as_str()[5..].trim();
                    sort = Some(parse_sort(value).map_err(|error| QueryDiagnostic {
                        code: "sort".to_owned(),
                        message: error,
                        severity: "error".to_owned(),
                        span: QuerySpan {
                            start: item.as_span().start(),
                            end: item.as_span().end(),
                        },
                    })?);
                }
                _ => {}
            }
        }
    }
    Ok(QueryAst { clauses, sort })
}

pub(crate) fn is_builtin_field(field: &str) -> bool {
    matches!(
        field,
        "type"
            | "state"
            | "tag"
            | "label"
            | "link"
            | "reference"
            | "title"
            | "body"
            | "id"
            | "created"
            | "modified"
            | "completed"
            | "published_on"
    )
}

pub(crate) fn is_sort_field(field: &str, property_fields: &HashSet<String>) -> bool {
    matches!(
        field,
        "created" | "modified" | "title" | "type" | "state" | "id" | "published_on"
    ) || property_fields.contains(field)
}

/// Validation used for editor diagnostics deliberately returns plain language.
/// N-API's `GenericFailure` display prefix is transport detail, not something a
/// writer should see in a query hint.
#[cfg(test)]
struct SemanticError {
    message: String,
    #[cfg(test)]
    span: Option<QuerySpan>,
}

#[cfg(test)]
impl std::fmt::Display for SemanticError {
    fn fmt(&self, output: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        output.write_str(&self.message)
    }
}

/// Test fixture syntax is checked before conversion only where an enum would lose
/// an invalid token. All domain semantics use the production document validator.
#[cfg(test)]
fn validate_ast_semantics(
    ast: &QueryAst,
    fields: &HashSet<String>,
) -> std::result::Result<(), SemanticError> {
    for clause in &ast.clauses {
        if let QueryClauseKind::Is(value) = &clause.kind
            && !matches!(value.as_str(), "archived" | "pinned")
        {
            return Err(SemanticError {
                message: format!("Unknown is: value `{value}`"),
                span: Some(clause.span),
            });
        }
    }
    document::QueryDocument::from_ast(ast)
        .validate_fields(fields)
        .map_err(|errors| {
            let error = &errors[0];
            let span = error
                .path
                .strip_prefix("/clauses/")
                .and_then(|path| path.split('/').next())
                .and_then(|index| index.parse::<usize>().ok())
                .and_then(|index| ast.clauses.get(index))
                .map(|clause| clause.span);
            SemanticError {
                message: error.message.clone(),
                span,
            }
        })
}

fn is_flat_facet_key(value: &str) -> bool {
    value.split('_').enumerate().all(|(index, segment)| {
        let mut characters = segment.chars();
        matches!(characters.next(), Some(character) if character.is_ascii_lowercase() || (index > 0 && character.is_ascii_digit()))
            && characters
                .all(|character| character.is_ascii_lowercase() || character.is_ascii_digit())
    })
}

fn is_tag_key(value: &str) -> bool {
    let segments = value.split('/').collect::<Vec<_>>();
    !segments.is_empty()
        && segments.iter().enumerate().all(|(index, segment)| {
            is_flat_facet_key(segment)
                || (index > 0
                    && segment.len() <= 4
                    && !segment.is_empty()
                    && segment.chars().all(|character| character.is_ascii_digit()))
        })
}
pub(crate) fn query_field(field: &str) -> Result<String> {
    match field {
        "type" => Ok("item_type".to_owned()),
        "state" => Ok("state".to_owned()),
        "tag" => Ok("tags".to_owned()),
        "label" => Ok("labels".to_owned()),
        "link" | "reference" => Ok("reference_targets".to_owned()),
        "title" | "body" | "id" | "created" | "modified" | "completed" => Ok(match field {
            "created" => "created_at".to_owned(),
            "modified" => "modified_at".to_owned(),
            "completed" => "metadata.completed_at".to_owned(),
            value => value.to_owned(),
        }),
        value
            if value
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || character == '_') =>
        {
            Ok(format!("metadata.{value}"))
        }
        _ => Err(invalid_request(format!("Unknown query field `{field}`"))),
    }
}

pub(crate) fn supports_presence(index: &Index, field: &str) -> bool {
    let Ok(canonical) = query_field(field) else {
        return false;
    };
    let schema = index.schema();
    schema
        .find_field(&canonical)
        .is_some_and(|(field, _)| schema.get_field_entry(field).is_fast())
}

#[cfg(test)]
fn parse_sort(value: &str) -> std::result::Result<Vec<SortSpec>, String> {
    let mut output = Vec::new();
    for entry in value.split(',') {
        let (field, direction) = entry
            .rsplit_once('-')
            .ok_or_else(|| "Sort entries must end in -asc or -desc".to_owned())?;
        if field.is_empty() || !matches!(direction, "asc" | "desc") {
            return Err("Sort entries must end in -asc or -desc".to_owned());
        }
        let canonical = match field {
            "created" | "modified" | "title" | "type" | "state" | "priority" | "id" => {
                field.to_owned()
            }
            value
                if value
                    .chars()
                    .all(|character| character.is_ascii_alphanumeric() || character == '_') =>
            {
                value.to_owned()
            }
            _ => return Err(format!("Unknown sort field `{field}`")),
        };
        if output.iter().any(|item: &SortSpec| item.field == canonical) {
            return Err(format!("Duplicate sort field `{field}`"));
        }
        output.push(SortSpec {
            field: canonical,
            descending: direction == "desc",
        });
    }
    if output.is_empty() {
        return Err("sort: requires at least one field".to_owned());
    }
    Ok(output)
}

fn date_prefix(value: &str) -> Option<String> {
    let value = match value {
        "today" => chrono::Utc::now().format("%Y-%m-%d").to_string(),
        "yesterday" => (chrono::Utc::now() - chrono::Duration::days(1))
            .format("%Y-%m-%d")
            .to_string(),
        "last_7d" => return None,
        value => value.to_owned(),
    };
    absolute_date_prefix(&value)
}

fn absolute_date_prefix(value: &str) -> Option<String> {
    let valid = matches!(value.len(), 4 | 7 | 10)
        && value
            .as_bytes()
            .iter()
            .enumerate()
            .all(|(index, byte)| match value.len() {
                4 => byte.is_ascii_digit(),
                7 => {
                    if index == 4 {
                        *byte == b'-'
                    } else {
                        byte.is_ascii_digit()
                    }
                }
                _ => {
                    if matches!(index, 4 | 7) {
                        *byte == b'-'
                    } else {
                        byte.is_ascii_digit()
                    }
                }
            });
    if !valid {
        return None;
    }
    let full = match value.len() {
        4 => format!("{value}-01-01"),
        7 => format!("{value}-01"),
        _ => value.to_owned(),
    };
    chrono::NaiveDate::parse_from_str(&full, "%Y-%m-%d").ok()?;
    Some(value.to_owned())
}

fn exact_field_query(index: &Index, field_name: &str, value: &str) -> Result<Box<dyn Query>> {
    let field = index
        .schema()
        .get_field(field_name)
        .map_err(internal_error)?;
    Ok(Box::new(TermQuery::new(
        Term::from_field_text(field, value),
        IndexRecordOption::Basic,
    )))
}

fn parse_isolated_query(index: &Index, source: &str) -> Result<Box<dyn Query>> {
    let schema = index.schema();
    let title = schema.get_field("title").map_err(internal_error)?;
    let body = schema.get_field("body").map_err(internal_error)?;
    let mut parser = QueryParser::for_index(index, vec![title, body]);
    parser.set_conjunction_by_default();
    parser.parse_query(source).map_err(invalid_request)
}

fn escaped_value(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', r"\\").replace('"', r#"\""#))
}

/// Compile the already-validated Bluink AST to Tantivy query objects. The
/// Tantivy parser remains deliberately confined to a single user text clause
/// or a single typed metadata predicate; it never receives the complete
/// Bluink expression or determines Boolean semantics.
#[cfg(test)]
pub(crate) fn compile_native_query(
    index: &Index,
    source: &str,
    property_fields: &schema::QuerySchema,
    pin_scope_id: Option<&str>,
) -> Result<(Box<dyn Query>, Vec<SortSpec>, bool)> {
    let ast =
        parse_bluink_query(source).map_err(|diagnostic| invalid_request(diagnostic.message))?;
    validate_ast_semantics(&ast, property_fields).map_err(invalid_request)?;
    compile_query_document(
        index,
        &document::QueryDocument::from_ast(&ast),
        property_fields,
        pin_scope_id,
    )
}

pub(crate) fn compile_query_document(
    index: &Index,
    document: &document::QueryDocument,
    property_fields: &schema::QuerySchema,
    pin_scope_id: Option<&str>,
) -> Result<(Box<dyn Query>, Vec<SortSpec>, bool)> {
    document
        .validate(property_fields)
        .map_err(crate::application::error::ApplicationError::InvalidQuery)?;
    let mut sorts: Vec<_> = document
        .sort
        .iter()
        .map(|sort| SortSpec {
            field: sort.field.clone(),
            descending: sort.direction == SortDirection::Desc,
        })
        .collect();
    let mut has_text = false;
    let mut clauses: Vec<(Occur, Box<dyn Query>)> = vec![(Occur::Must, Box::new(AllQuery))];

    for (position, clause) in document.clauses.iter().enumerate() {
        let occur = match clause.polarity {
            Polarity::Required => Occur::Must,
            Polarity::Excluded => Occur::MustNot,
        };
        let query = (|| -> Result<Box<dyn Query>> {
            Ok(match &clause.predicate {
                Predicate::Text { value, mode } => {
                    has_text = true;
                    let source = if *mode == TextMode::Phrase {
                        escaped_value(value)
                    } else {
                        value.clone()
                    };
                    parse_isolated_query(index, &source)?
                }
                Predicate::Tag { key } => exact_field_query(index, "tags", key)?,
                Predicate::Label { key } => exact_field_query(index, "labels", key)?,
                Predicate::Has { field } => {
                    if !supports_presence(index, field) {
                        return Err(invalid_request(format!(
                            "Presence checks are not supported for `{field}`."
                        )));
                    }
                    let field = query_field(field)?;
                    Box::new(ExistsQuery::new(field, true))
                }
                Predicate::Is { value } => match value {
                    StatePredicate::Archived => exact_field_query(index, "state", "archived")?,
                    StatePredicate::Pinned => match pin_scope_id {
                        Some(scope_id) => exact_field_query(index, "pins", scope_id)?,
                        None => Box::new(ExistsQuery::new("pins".to_owned(), false)),
                    },
                },
                Predicate::Field {
                    field,
                    operator,
                    value,
                } => compile_field(index, field, *operator, value)?,
            })
        })()
        .map_err(|error| match error {
            crate::application::error::ApplicationError::InvalidRequest(message) => {
                crate::application::error::ApplicationError::InvalidQuery(vec![
                    document::ConditionDiagnostic {
                        path: format!("/clauses/{position}/predicate/value"),
                        code: "native".into(),
                        message,
                        start: None,
                        end: None,
                    },
                ])
            }
            other => other,
        })?;
        clauses.push((occur, exclusion::guard(occur, query)));
    }
    if sorts.is_empty() && !has_text {
        sorts.push(SortSpec {
            field: "modified".into(),
            descending: true,
        });
    }
    if !sorts.is_empty() && !sorts.iter().any(|sort| sort.field == "id") {
        sorts.push(SortSpec {
            field: "id".into(),
            descending: false,
        });
    }
    let query: Box<dyn Query> = Box::new(BooleanQuery::new(clauses));
    Ok((query, sorts, has_text))
}

fn compile_field(
    index: &Index,
    field: &str,
    operator: Comparison,
    value: &str,
) -> Result<Box<dyn Query>> {
    let canonical = query_field(field)?;
    let date_field = matches!(field, "created" | "modified" | "completed");
    if operator != Comparison::Match {
        if field == "title" {
            // Compare complete titles with the same normalization used by sorting.
            return compile_range(index, "title_sort".into(), operator, &value.to_lowercase());
        }
        let date_value = if date_field {
            if value == "last_7d" {
                Some(
                    (chrono::Utc::now() - chrono::Duration::days(6))
                        .format("%Y-%m-%d")
                        .to_string(),
                )
            } else {
                date_prefix(value).or_else(|| {
                    chrono::DateTime::parse_from_rfc3339(value)
                        .ok()
                        .map(|instant| {
                            instant
                                .with_timezone(&chrono::Utc)
                                .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
                        })
                })
            }
        } else {
            None
        };
        if date_field && date_value.is_none() {
            return Err(invalid_request(
                "Date values must use YYYY, YYYY-MM, YYYY-MM-DD, today, yesterday, or last_7d",
            ));
        }
        let value = date_value.as_deref().unwrap_or(value);
        return compile_range(index, canonical, operator, value);
    }
    if date_field {
        if value.contains('/') {
            return Err(invalid_request(
                "Use YYYY-MM-DD for date values, not slash dates",
            ));
        }
        if value == "last_7d" {
            let start = (chrono::Utc::now() - chrono::Duration::days(6))
                .format("%Y-%m-%d")
                .to_string();
            return compile_range(index, canonical, Comparison::Gte, &start);
        }
        if let Some(prefix) = date_prefix(value) {
            // Every canonical date prefix ends in an ASCII digit. Incrementing
            // that digit gives the exclusive lexical end of its prefix range,
            // including December, leap days and the maximum four-digit year.
            let mut end = prefix.clone();
            let last = end
                .pop()
                .ok_or_else(|| internal_error("Empty date prefix"))?;
            end.push(char::from(
                u8::try_from(u32::from(last)).map_err(internal_error)? + 1,
            ));
            let schema = index.schema();
            let (native_field, path) = schema
                .find_field(&canonical)
                .ok_or_else(|| internal_error("Missing date field"))?;
            let term = |value: &str| {
                if let tantivy::schema::FieldType::JsonObject(options) =
                    schema.get_field_entry(native_field).field_type()
                {
                    let mut term = Term::from_field_json_path(
                        native_field,
                        path,
                        options.is_expand_dots_enabled(),
                    );
                    term.append_type_and_str(value);
                    term
                } else {
                    Term::from_field_text(native_field, value)
                }
            };
            // Native string bounds avoid JSON's numeric/date literal inference.
            return Ok(Box::new(tantivy::query::RangeQuery::new(
                std::ops::Bound::Included(term(&prefix)),
                std::ops::Bound::Excluded(term(&end)),
            )));
        }
        return Err(invalid_request(
            "Date values must use YYYY, YYYY-MM, YYYY-MM-DD, today, yesterday, or last_7d",
        ));
    }
    match canonical.as_str() {
        "item_type" | "state" | "tags" | "labels" | "reference_targets" | "id" => {
            exact_field_query(index, &canonical, value)
        }
        _ => parse_isolated_query(index, &format!("{canonical}:{}", escaped_value(value))),
    }
}

/// Pass bounds as data: spaces, quotes and punctuation are not query syntax.
fn compile_range(
    index: &Index,
    field: String,
    operator: Comparison,
    value: &str,
) -> Result<Box<dyn Query>> {
    use tantivy::query_grammar::{UserInputBound as Bound, UserInputLeaf};
    // Tantivy treats even an explicitly bounded "*" as infinity. Structured
    // values have no wildcard bounds, so preserve this literal as a native term.
    if value == "*" {
        use std::ops::Bound::{Excluded, Included, Unbounded};
        use tantivy::schema::FieldType;
        let schema = index.schema();
        let (native_field, json_path) = schema
            .find_field(&field)
            .ok_or_else(|| invalid_request(format!("Unknown query field `{field}`")))?;
        let term = match schema.get_field_entry(native_field).field_type() {
            FieldType::Str(_) => Term::from_field_text(native_field, value),
            FieldType::JsonObject(options) => {
                let mut term = Term::from_field_json_path(
                    native_field,
                    json_path,
                    options.is_expand_dots_enabled(),
                );
                term.append_type_and_str(value);
                term
            }
            _ => return Err(invalid_request("Choose a value matching the field type.")),
        };
        let (lower, upper) = match operator {
            Comparison::Gt => (Excluded(term), Unbounded),
            Comparison::Gte => (Included(term), Unbounded),
            Comparison::Lt => (Unbounded, Excluded(term)),
            Comparison::Lte => (Unbounded, Included(term)),
            Comparison::Match => {
                return Err(internal_error("A range requires a comparison operator."));
            }
        };
        return Ok(Box::new(tantivy::query::RangeQuery::new(lower, upper)));
    }
    let (lower, upper) = match operator {
        Comparison::Gt => (Bound::Exclusive(value.into()), Bound::Unbounded),
        Comparison::Gte => (Bound::Inclusive(value.into()), Bound::Unbounded),
        Comparison::Lt => (Bound::Unbounded, Bound::Exclusive(value.into())),
        Comparison::Lte => (Bound::Unbounded, Bound::Inclusive(value.into())),
        Comparison::Match => return Err(internal_error("A range requires a comparison operator.")),
    };
    QueryParser::for_index(index, Vec::new())
        .build_query_from_user_input_ast(
            UserInputLeaf::Range {
                field: Some(field),
                lower,
                upper,
            }
            .into(),
        )
        .map_err(invalid_request)
}

pub(crate) fn item_type_values() -> impl Iterator<Item = &'static str> {
    crate::engine::project::types::ItemType::VALUES
        .iter()
        .map(|(_, value)| *value)
}

#[cfg(test)]
mod tests {
    use super::parse_bluink_query;
    use super::sort::{compare_sort_values, search_hit_sort_value};
    fn parsed(source: &str) -> super::document::ParsedQuery {
        super::document::parse(source, &std::collections::HashSet::new())
    }
    use crate::model::SearchHit;
    use serde_json::json;

    #[test]
    fn parses_quoted_phrases_and_reports_spans() {
        let ast = parse_bluink_query(r#"+type:page "two words" -tag:draft"#).unwrap();
        assert_eq!(ast.clauses.len(), 3);
        assert!(parsed(r#"author:"unterminated"#).document.is_none());
    }

    #[test]
    fn parses_fields_ranges_exclusions_and_sorting() {
        let ast = parse_bluink_query(
            "+type:page -is:archived +publication_year:>=1950 sort:publication_year-asc,title-desc",
        )
        .unwrap();
        assert_eq!(ast.clauses.len(), 3);
        let sort = ast.sort.unwrap();
        assert_eq!(sort.len(), 2);
        assert_eq!(sort[0].field, "publication_year");
        assert!(!sort[0].descending);
        assert_eq!(sort[1].field, "title");
        assert!(sort[1].descending);
    }

    #[test]
    fn validates_facet_shorthand_and_negative_text_queries() {
        assert!(
            parsed("#research_methods #births/1900 @foreshadowing")
                .document
                .is_some()
        );
        assert!(parsed("@bibliography_0").document.is_some());
        assert!(parsed("#not-valid").document.is_none());
        assert!(parsed("@births/1900").document.is_none());
        assert!(parsed("-tag:draft").document.is_some());
        assert!(parsed(r#"research "exact phrase""#).document.is_some());
    }

    #[test]
    fn semantic_diagnostics_point_to_the_failing_clause() {
        for (source, clause) in [
            ("hello type:unknown", "type:unknown"),
            ("文学 state:missing", "state:missing"),
            ("#valid @births/1900", "@births/1900"),
            ("hello OR", "OR"),
            ("hello unknown_property:value", "unknown_property:value"),
        ] {
            let parsed = parsed(source);
            assert!(parsed.document.is_none(), "{source}");
            let error = &parsed.diagnostics[0];
            let start = source.rfind(clause).unwrap();
            assert_eq!(
                error.start,
                Some(source[..start].encode_utf16().count()),
                "{source}"
            );
            assert_eq!(error.end, Some(source.encode_utf16().count()), "{source}");
        }
    }

    #[test]
    fn reports_plain_language_semantic_diagnostics() {
        let analysis = parsed("unknown_property:value");
        assert!(analysis.document.is_none());
        let message = &analysis.diagnostics[0].message;
        assert_eq!(message, "Unknown query field `unknown_property`");
        assert!(!message.contains("Generic"));
    }

    #[test]
    fn accepts_last_sort_and_rejects_invalid_syntax() {
        for source in [
            "sort:title-asc +type:page",
            "+type:page sort:title-asc sort:id-asc",
        ] {
            let analysis = parsed(source);
            assert!(
                analysis.document.is_some(),
                "{source}: {:?}",
                analysis.diagnostics
            );
        }
        let ast = parse_bluink_query("sort:title-asc +type:page sort:modified-desc").unwrap();
        assert_eq!(ast.sort.unwrap()[0].field, "modified");
        for source in [
            "type:page OR tag:book",
            "sort:title-up",
            "+type:unknown",
            "+state:deleted",
        ] {
            assert!(parsed(source).document.is_none(), "{source}");
        }
    }

    #[test]
    fn typed_sorting_keeps_missing_values_last_in_both_directions() {
        let hit = SearchHit {
            id: "item".to_owned(),
            revision: "revision".to_owned(),
            created_at: String::new(),
            modified_at: String::new(),
            score: 0.0,
            preview: String::new(),
            preview_image: None,
            metadata: json!({ "rating": 2.5, "title": "Example" }),
            tags: Vec::new(),
        };
        assert_eq!(search_hit_sort_value(&hit, "rating"), Some(json!(2.5)));
        assert!(compare_sort_values(Some(json!(2)), Some(json!(10)), false).is_lt());
        assert!(compare_sort_values(Some(json!(2)), Some(json!(10)), true).is_gt());
        assert!(compare_sort_values(None, Some(json!(10)), false).is_gt());
        assert!(compare_sort_values(None, Some(json!(10)), true).is_gt());
    }
}

#[cfg(test)]
mod date_regressions {
    use super::*;
    #[test]
    fn completed_prefix_matches_json_dates_through_structured_conditions() {
        let schema = crate::indexing::index::build_schema();
        let index = Index::create_in_ram(schema.clone());
        let mut writer = crate::indexing::index::open_writer(&index).unwrap();
        for date in [
            "2024-05-01T12:00:00.000Z",
            "2024-05-31T23:59:59.000Z",
            "2024-06-01T00:00:00.000Z",
        ] {
            let mut document = tantivy::schema::TantivyDocument::default();
            document.add_text(schema.get_field("item_type").unwrap(), "page");
            document.add_object(
                schema.get_field("metadata").unwrap(),
                serde_json::from_value::<
                    std::collections::BTreeMap<String, tantivy::schema::OwnedValue>,
                >(serde_json::json!({"completed_at": date}))
                .unwrap(),
            );
            writer.add_document(document).unwrap();
        }
        writer.commit().unwrap();
        writer.wait_merging_threads().unwrap();
        let reader = crate::indexing::index::open_reader(&index).unwrap();
        for (prefix, count) in [("2024", 3), ("2024-05", 2), ("2024-05-31", 1), ("2023", 0)] {
            let document = serde_json::from_value(serde_json::json!({"version":1,"clauses":[{
                "polarity":"required","predicate":{"kind":"field","field":"completed","operator":"match","value":prefix}
            }],"sort":[]})).unwrap();
            let (query, _, _) =
                compile_query_document(&index, &document, &schema::QuerySchema::default(), None)
                    .unwrap();
            assert_eq!(
                reader
                    .searcher()
                    .search(query.as_ref(), &tantivy::collector::Count)
                    .unwrap(),
                count,
                "{prefix}"
            );
        }
    }
}
