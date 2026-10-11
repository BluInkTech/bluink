// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

pub(crate) mod sort;

use std::collections::HashSet;

use crate::application::error::Result;
use pest::Parser;
use pest_derive::Parser;
use serde::Serialize;
use tantivy::query::{
    AllQuery, BooleanQuery, ExistsQuery, Occur, Query, QueryParser, RegexQuery, TermQuery,
};
use tantivy::schema::IndexRecordOption;
use tantivy::{Index, Term};

use crate::application::error::{internal_error, invalid_request};
use crate::model::SortSpec;

#[derive(Parser)]
#[grammar = "runtime/src/query.pest"]
struct BluinkParser;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(test, derive(ts_rs::TS))]
pub(crate) struct QuerySpan {
    pub(crate) start: usize,
    pub(crate) end: usize,
}

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

#[derive(Debug, Clone, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub(crate) struct QueryAnalysis {
    pub(crate) language_version: u8,
    pub(crate) valid: bool,
    pub(crate) diagnostics: Vec<QueryDiagnostic>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Polarity {
    Required,
    Excluded,
}

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct QueryClause {
    pub(crate) polarity: Polarity,
    pub(crate) kind: QueryClauseKind,
    pub(crate) span: QuerySpan,
}

#[derive(Debug, Clone)]
pub(crate) struct QueryAst {
    pub(crate) clauses: Vec<QueryClause>,
    pub(crate) sort: Option<Vec<SortSpec>>,
}
#[expect(
    clippy::needless_pass_by_value,
    reason = "The error adapter consumes the parser error passed by map_err."
)]
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

fn is_builtin_field(field: &str) -> bool {
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

/// Validation used for editor diagnostics deliberately returns plain language.
/// N-API's `GenericFailure` display prefix is transport detail, not something a
/// writer should see in a query hint.
fn validate_schema_fields(
    ast: &QueryAst,
    property_fields: &HashSet<String>,
) -> std::result::Result<(), String> {
    for clause in &ast.clauses {
        let field = match &clause.kind {
            QueryClauseKind::Field { field, .. } | QueryClauseKind::Has(field) => {
                Some(field.as_str())
            }
            _ => None,
        };
        if let Some(field) = field
            && !is_builtin_field(field)
            && !property_fields.contains(field)
        {
            return Err(format!("Unknown query field `{field}`"));
        }
    }
    if let Some(sorts) = &ast.sort {
        for sort in sorts {
            if !matches!(
                sort.field.as_str(),
                "created" | "modified" | "title" | "type" | "state" | "id" | "published_on"
            ) && !property_fields.contains(&sort.field)
            {
                return Err(format!("Unknown sort field `{}`", sort.field));
            }
        }
    }
    Ok(())
}

fn validate_ast_semantics(
    ast: &QueryAst,
    property_fields: &HashSet<String>,
) -> std::result::Result<(), String> {
    validate_schema_fields(ast, property_fields)?;
    for clause in &ast.clauses {
        match &clause.kind {
            QueryClauseKind::Tag(key) => {
                if !is_tag_key(key) {
                    return Err("Tags after # must be lowercase snake-case paths".to_owned());
                }
            }
            QueryClauseKind::Label(key) => {
                if !is_flat_facet_key(key) {
                    return Err("Labels after @ must be lowercase snake-case keys".to_owned());
                }
            }
            QueryClauseKind::Is(value) if !matches!(value.as_str(), "archived" | "pinned") => {
                return Err(format!("Unknown is: value `{value}`"));
            }
            QueryClauseKind::Field { field, value, .. } => {
                if value.is_empty() {
                    return Err("A field clause requires a value".to_owned());
                }
                if field == "type"
                    && ![
                        "page",
                        "workspace",
                        "widget",
                        "query",
                        "task",
                        "goal",
                        "setting",
                        "publication",
                    ]
                    .contains(&value.as_str())
                {
                    return Err(format!("Unknown type value `{value}`"));
                }
                if field == "state" && !["active", "archived"].contains(&value.as_str()) {
                    return Err(format!("Unknown state value `{value}`"));
                }
                if field == "tag" && !is_tag_key(value) {
                    return Err("Tags must be lowercase snake-case paths".to_owned());
                }
                if field == "label" && !is_flat_facet_key(value) {
                    return Err("Labels must be lowercase snake-case keys".to_owned());
                }
            }
            QueryClauseKind::Text { value, .. }
                if value.eq_ignore_ascii_case("AND")
                    || value.eq_ignore_ascii_case("OR")
                    || value.eq_ignore_ascii_case("NOT")
                    || value.contains('(')
                    || value.contains(')') =>
            {
                return Err("Boolean operators and nesting are not supported yet".to_owned());
            }
            _ => {}
        }
    }
    Ok(())
}

fn semantic_span(ast: &QueryAst, error: &str, source_len: usize) -> QuerySpan {
    let field = error
        .strip_prefix("Unknown query field `")
        .or_else(|| error.strip_prefix("Unknown type value `"))
        .or_else(|| error.strip_prefix("Unknown state value `"))
        .and_then(|value| value.strip_suffix('`'));
    if let Some(field) = field
        && let Some(clause) = ast.clauses.iter().find(|clause| match &clause.kind {
            QueryClauseKind::Field {
                field: candidate, ..
            }
            | QueryClauseKind::Has(candidate) => candidate == field,
            _ => false,
        })
    {
        return clause.span;
    }
    if let Some(clause) = ast.clauses.iter().find(|clause| {
        matches!(
            &clause.kind,
            QueryClauseKind::Tag(_) | QueryClauseKind::Label(_) | QueryClauseKind::Text { .. }
        ) && (error.contains("Tags") || error.contains("Labels") || error.contains("Boolean"))
    }) {
        return clause.span;
    }
    QuerySpan {
        start: 0,
        end: source_len.max(1),
    }
}

pub(crate) fn analyze_bluink_query_with_schema(
    source: &str,
    property_fields: &HashSet<String>,
) -> QueryAnalysis {
    match parse_bluink_query(source) {
        Ok(ast) => match validate_ast_semantics(&ast, property_fields) {
            Ok(()) => QueryAnalysis {
                language_version: 1,
                valid: true,
                diagnostics: Vec::new(),
            },
            Err(error) => QueryAnalysis {
                language_version: 1,
                valid: false,
                diagnostics: vec![QueryDiagnostic {
                    code: "semantic".to_owned(),
                    message: error.clone(),
                    severity: "error".to_owned(),
                    span: semantic_span(&ast, &error, source.len()),
                }],
            },
        },
        Err(diagnostic) => QueryAnalysis {
            language_version: 1,
            valid: false,
            diagnostics: vec![diagnostic],
        },
    }
}

#[cfg(test)]
pub(crate) fn analyze_bluink_query(source: &str) -> QueryAnalysis {
    analyze_bluink_query_with_schema(source, &HashSet::new())
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
    if !output.iter().any(|item| item.field == "id") {
        output.push(SortSpec {
            field: "id".to_owned(),
            descending: false,
        });
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
    let valid = matches!(value.len(), 4 | 7 | 10)
        && value
            .as_bytes()
            .iter()
            .enumerate()
            .all(|(index, byte)| match value.len() {
                4 => byte.is_ascii_digit(),
                7 => matches!(index, 4) || byte.is_ascii_digit(),
                _ => matches!(index, 4 | 7) || byte.is_ascii_digit(),
            });
    valid.then_some(value)
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
pub(crate) fn compile_native_query(
    index: &Index,
    source: &str,
    property_fields: &HashSet<String>,
    pin_scope_id: Option<&str>,
) -> Result<(Box<dyn Query>, Vec<SortSpec>, bool)> {
    // The legacy string compiler is retained only by existing fixtures. Runtime
    // execution starts from the AST and composes Query objects directly.
    let ast =
        parse_bluink_query(source).map_err(|diagnostic| invalid_request(diagnostic.message))?;
    validate_ast_semantics(&ast, property_fields).map_err(invalid_request)?;
    let mut sorts = ast.sort.clone().unwrap_or_default();
    let mut has_text = false;
    let mut clauses: Vec<(Occur, Box<dyn Query>)> = Vec::new();
    let mut has_required_clause = false;

    for clause in ast.clauses {
        let occur = match clause.polarity {
            Polarity::Required => {
                has_required_clause = true;
                Occur::Must
            }
            Polarity::Excluded => Occur::MustNot,
        };
        let query: Box<dyn Query> = match clause.kind {
            QueryClauseKind::Text { value, quoted } => {
                has_text = true;
                let source = if quoted { escaped_value(&value) } else { value };
                parse_isolated_query(index, &source)?
            }
            QueryClauseKind::Tag(value) => exact_field_query(index, "tags", &value)?,
            QueryClauseKind::Label(value) => exact_field_query(index, "labels", &value)?,
            QueryClauseKind::Has(field) => {
                let field = query_field(&field)?;
                Box::new(ExistsQuery::new(field, true))
            }
            QueryClauseKind::Is(value) => match value.as_str() {
                "archived" => exact_field_query(index, "state", "archived")?,
                "pinned" => match pin_scope_id {
                    Some(scope_id) => exact_field_query(index, "pins", scope_id)?,
                    None => Box::new(ExistsQuery::new("pins".to_owned(), false)),
                },
                _ => unreachable!("semantic validation handles is: values"),
            },
            QueryClauseKind::Field {
                field,
                operator,
                value,
            } => {
                let canonical = query_field(&field)?;
                if operator.is_empty() {
                    if matches!(field.as_str(), "created" | "modified" | "completed") {
                        if value.contains('/') {
                            return Err(invalid_request(
                                "Use YYYY-MM-DD for date values, not slash dates",
                            ));
                        }
                        if value == "last_7d" {
                            let start = (chrono::Utc::now() - chrono::Duration::days(6))
                                .format("%Y-%m-%d")
                                .to_string();
                            parse_isolated_query(index, &format!("{canonical}:>={start}"))?
                        } else if let Some(prefix) = date_prefix(&value) {
                            let field = index
                                .schema()
                                .get_field(&canonical)
                                .map_err(internal_error)?;
                            Box::new(
                                RegexQuery::from_pattern(&format!("{prefix}.*"), field)
                                    .map_err(internal_error)?,
                            )
                        } else {
                            return Err(invalid_request(
                                "Date values must use YYYY, YYYY-MM, YYYY-MM-DD, today, yesterday, or last_7d",
                            ));
                        }
                    } else {
                        match canonical.as_str() {
                            "item_type" | "state" | "tags" | "labels" | "reference_targets"
                            | "id" => exact_field_query(index, &canonical, &value)?,
                            _ => parse_isolated_query(
                                index,
                                &format!("{canonical}:{}", escaped_value(&value)),
                            )?,
                        }
                    }
                } else {
                    // Tantivy owns the field-type-specific range encoding for
                    // this one predicate; conjunction, negation, and sorting
                    // remain owned by Bluink's AST compiler.
                    parse_isolated_query(index, &format!("{canonical}:{operator}{value}"))?
                }
            }
        };
        clauses.push((occur, query));
    }
    if sorts.is_empty() && !has_text {
        sorts = parse_sort("modified-desc").map_err(internal_error)?;
    }
    if !has_required_clause && !clauses.is_empty() {
        clauses.insert(0, (Occur::Must, Box::new(AllQuery)));
    }
    let query: Box<dyn Query> = if clauses.is_empty() {
        Box::new(AllQuery)
    } else {
        Box::new(BooleanQuery::new(clauses))
    };
    Ok((query, sorts, has_text))
}

#[cfg(test)]
mod tests {
    use super::sort::{compare_sort_values, search_hit_sort_value};
    use super::{analyze_bluink_query, parse_bluink_query};
    use crate::model::SearchHit;
    use serde_json::json;

    #[test]
    fn parses_quoted_phrases_and_reports_spans() {
        let ast = parse_bluink_query(r#"+type:page "two words" -tag:draft"#).unwrap();
        assert_eq!(ast.clauses.len(), 3);
        assert!(!analyze_bluink_query(r#"author:"unterminated"#).valid);
    }

    #[test]
    fn parses_fields_ranges_exclusions_and_sorting() {
        let ast = parse_bluink_query(
            "+type:page -is:archived +publication_year:>=1950 sort:publication_year-asc,title-desc",
        )
        .unwrap();
        assert_eq!(ast.clauses.len(), 3);
        let sort = ast.sort.unwrap();
        assert_eq!(sort.len(), 3);
        assert_eq!(sort[0].field, "publication_year");
        assert!(!sort[0].descending);
        assert_eq!(sort[1].field, "title");
        assert!(sort[1].descending);
        assert_eq!(sort[2].field, "id");
    }

    #[test]
    fn validates_facet_shorthand_and_negative_text_queries() {
        assert!(analyze_bluink_query("#research_methods #births/1900 @foreshadowing").valid);
        assert!(analyze_bluink_query("@bibliography_0").valid);
        assert!(!analyze_bluink_query("#not-valid").valid);
        assert!(!analyze_bluink_query("@births/1900").valid);
        assert!(analyze_bluink_query("-tag:draft").valid);
        assert!(analyze_bluink_query(r#"research "exact phrase""#).valid);
    }

    #[test]
    fn reports_plain_language_semantic_diagnostics() {
        let analysis = analyze_bluink_query("unknown_property:value");
        assert!(!analysis.valid);
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
            let analysis = analyze_bluink_query(source);
            assert!(analysis.valid, "{source}: {:?}", analysis.diagnostics);
        }
        let ast = parse_bluink_query("sort:title-asc +type:page sort:modified-desc").unwrap();
        assert_eq!(ast.sort.unwrap()[0].field, "modified");
        for source in [
            "type:page OR tag:book",
            "sort:title-up",
            "+type:unknown",
            "+state:deleted",
        ] {
            assert!(!analyze_bluink_query(source).valid, "{source}");
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
