// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! Parameterized structured queries over a single reader transaction.
#[cfg(test)]
mod benchmarks;
mod compile;
mod dates;
mod execute;
mod facets;
mod order;
#[cfg(test)]
mod tests;

use super::{Error, Result, fields};
use crate::domain::project::query::{ConditionDiagnostic, QueryDocument};
use rusqlite::{Transaction, types::Value};
use std::collections::BTreeMap;

pub(crate) use dates::day_start;
#[cfg(test)]
pub(crate) use execute::matching_ids;
pub(crate) use execute::{SearchOptions, candidate_page_cards, ordered_ids, search};
pub(crate) use facets::{facet_counts, page_catalogue};

pub(crate) struct CompiledQuery {
    ctes: Vec<String>,
    filter: String,
    parameters: Vec<Value>,
    filter_parameters: usize,
    /// The header index used to count matches without ranking them.
    index: &'static str,
    from: String,
    ordering: Vec<String>,
}

struct Compiler {
    fields: BTreeMap<String, fields::Definition>,
    parameters: Vec<Value>,
    ctes: Vec<String>,
    rank_text: bool,
    positive_text: Vec<String>,
    pin_scope: Option<String>,
    timezone: chrono_tz::Tz,
    /// Only header predicates, including one type and one state equality.
    fixed_scope: bool,
}

impl Compiler {
    fn bind(&mut self, value: impl Into<Value>) -> String {
        self.parameters.push(value.into());
        format!("?{}", self.parameters.len())
    }

    fn relation(&mut self, kind: &str, key: Option<&str>, named: bool) -> String {
        let kind = self.bind(kind.to_owned());
        let target = key.map_or_else(String::new, |key| {
            let key = self.bind(key.to_owned());
            format!(" AND e.{}={key}", if named { "named_key" } else { "uuid" })
        });
        let namespace = if named {
            format!(" AND e.named_kind={kind}")
        } else {
            String::new()
        };
        format!(
            "i.id IN (SELECT r.source_id FROM relation r JOIN entity e ON e.id=r.target_id WHERE r.kind={kind}{namespace}{target})"
        )
    }
}

impl CompiledQuery {
    /// A joinable set of local item IDs, with its numbered bindings first.
    /// Consumers append their own bindings and keep this inside the same read
    /// transaction; no broad UUID list needs to cross the Rust/SQL boundary.
    pub(crate) fn selection(&self) -> (String, Vec<Value>) {
        (
            format!(
                "{}SELECT i.id FROM item i INDEXED BY item_query_header WHERE {}",
                self.with(),
                self.filter
            ),
            self.parameters[..self.filter_parameters].to_vec(),
        )
    }

    /// The selection with its parameters numbered after `offset` earlier ones,
    /// so a second selection can share a statement with the first.
    pub(crate) fn selection_after(&self, offset: usize) -> (String, Vec<Value>) {
        let (sql, parameters) = self.selection();
        let mut renumbered = String::with_capacity(sql.len());
        let mut rest = sql.as_str();
        // Every placeholder is a bound `?N`; values never appear in the SQL.
        while let Some(index) = rest.find('?') {
            renumbered.push_str(&rest[..=index]);
            rest = &rest[index + 1..];
            let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
            if let Ok(number) = rest[..digits].parse::<usize>() {
                renumbered.push_str(&(number + offset).to_string());
                rest = &rest[digits..];
            }
        }
        renumbered.push_str(rest);
        (renumbered, parameters)
    }

    fn with(&self) -> String {
        if self.ctes.is_empty() {
            String::new()
        } else {
            format!("WITH {} ", self.ctes.join(","))
        }
    }
}

/// A list scoped to one type and state in a visible order that one of the
/// (type, state, key, uuid) indexes stores reads only the requested page, and
/// its offsets skip index entries rather than sorting every match. Any other
/// filter or order seeks and scans local IDs through the thin header index.
fn page_index(compiler: &Compiler, ordering: &[String]) -> &'static str {
    match ordering {
        [key, uuid] if compiler.fixed_scope && uuid == "i.uuid ASC" => match key.as_str() {
            "i.modified_ms DESC" => "item_by_modified",
            "i.title_sort ASC" => "item_by_title",
            _ => "item_query_header",
        },
        _ => "item_query_header",
    }
}

fn diagnostic(position: usize, message: impl Into<String>) -> Error {
    Error::InvalidQuery(vec![ConditionDiagnostic {
        path: format!("/clauses/{position}/predicate/value"),
        code: "native".into(),
        message: message.into(),
        start: None,
        end: None,
    }])
}

fn read_count(row: &rusqlite::Row<'_>, column: usize) -> rusqlite::Result<usize> {
    usize::try_from(row.get::<_, i64>(column)?).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            column,
            rusqlite::types::Type::Integer,
            Box::new(error),
        )
    })
}

pub(crate) fn compile(
    tx: &Transaction<'_>,
    document: &QueryDocument,
    pin_scope: Option<&str>,
    timezone: chrono_tz::Tz,
) -> Result<CompiledQuery> {
    let definitions = fields::definitions(tx)?;
    let schema = crate::structured_query::schema::QuerySchema::from_properties(
        definitions
            .iter()
            .filter(|(key, _)| key.as_str() != "summary")
            .map(|(key, field)| (key.clone(), field.kind)),
    );
    document.validate(&schema).map_err(Error::InvalidQuery)?;
    let mut compiler = Compiler {
        fields: definitions,
        parameters: Vec::new(),
        ctes: Vec::new(),
        rank_text: document.sort.is_empty()
            && document.clauses.iter().any(|clause| {
                matches!(
                    clause.predicate,
                    crate::domain::project::query::Predicate::Text { .. }
                )
            }),
        positive_text: Vec::new(),
        pin_scope: pin_scope.map(str::to_owned),
        timezone,
        fixed_scope: true,
    };
    let filter = compiler.conditions(document)?;
    let filter_parameters = compiler.parameters.len();
    let (score, from) = if !compiler.rank_text || compiler.positive_text.is_empty() {
        ("0.0", String::new())
    } else {
        let occurrences = compiler
            .positive_text
            .iter()
            .map(|name| format!("SELECT item_id,score FROM {name}"))
            .collect::<Vec<_>>()
            .join(" UNION ALL ");
        compiler.ctes.push(format!("query_scores AS MATERIALIZED (SELECT item_id,-sum(score) AS score FROM ({occurrences}) GROUP BY item_id)"));
        // FTS5 estimates a tiny result set even for common words. Enumerating
        // grouped scores first prevents a correlated full CTE scan per page.
        (
            "scores.score",
            "query_scores scores CROSS JOIN item i INDEXED BY item_query_header ON i.id=scores.item_id".into(),
        )
    };
    let ordering = compiler.order(document, score);
    let index = page_index(&compiler, &ordering);
    let from = if from.is_empty() {
        format!("item i INDEXED BY {index}")
    } else {
        from
    };
    Ok(CompiledQuery {
        ctes: compiler.ctes,
        filter,
        parameters: compiler.parameters,
        filter_parameters,
        index,
        from,
        ordering,
    })
}
