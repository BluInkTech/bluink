// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use crate::catalogue::FacetEntry;
use crate::domain::project::types::FacetKind;
use crate::records::TargetCount;
use crate::store::{Error, Result};
use rusqlite::{Transaction, params_from_iter, types::Value};

/// Selected hierarchical tags match descendants; returned keys remain exact.
pub(crate) fn facet_counts(
    tx: &Transaction<'_>,
    kind: &str,
    selected: &[String],
    source_type: Option<&str>,
) -> Result<Vec<TargetCount>> {
    let facet = match kind {
        "tag" => FacetKind::Tag,
        "label" => FacetKind::Label,
        _ => return Err(Error::Invalid("Facet kind must be tag or label.".into())),
    };
    if selected.is_empty() && source_type == Some("page") {
        return Ok(page_catalogue(tx, facet)?
            .into_iter()
            .filter(|row| row.usage_count > 0)
            .map(|row| TargetCount {
                tid: row.key,
                count: row.usage_count,
            })
            .collect());
    }
    let mut parameters = vec![Value::Text(kind.into())];
    let mut conditions = vec!["i.state='active'".to_owned()];
    let mut excluded = Vec::new();
    if let Some(source_type) = source_type {
        parameters.push(Value::Text(source_type.into()));
        conditions.push(format!("i.type=?{}", parameters.len()));
    }
    for key in selected {
        parameters.push(Value::Text(key.clone()));
        let key_parameter = parameters.len();
        let target = if kind == "tag" {
            parameters.push(Value::Text(format!("{key}/")));
            let start = parameters.len();
            parameters.push(Value::Text(format!("{key}0")));
            let end = parameters.len();
            format!(
                "(target.named_key=?{key_parameter} OR (target.named_key>=?{start} AND target.named_key<?{end}))"
            )
        } else {
            format!("target.named_key=?{key_parameter}")
        };
        conditions.push(format!("i.id IN (SELECT chosen.source_id FROM relation chosen WHERE chosen.kind=?1 AND chosen.target_id IN (SELECT target.id FROM entity target WHERE target.named_kind=?1 AND {target}))"));
        excluded.push(format!("e.named_key<>?{key_parameter}"));
    }
    // A selected facet already provides owner IDs. Seek those headers, then
    // count only the matching owners' outgoing edges through the source PK.
    let (candidate_index, related) = if selected.is_empty() {
        (
            "item_by_modified",
            "FROM relation other INDEXED BY relation_reverse WHERE other.kind=?1 AND other.source_id IN (SELECT id FROM candidates)",
        )
    } else {
        (
            "item_query_header",
            "FROM candidates c CROSS JOIN relation other WHERE other.source_id=c.id AND other.kind=?1",
        )
    };
    let sql = format!(
        "WITH candidates AS MATERIALIZED (SELECT i.id FROM item i INDEXED BY {candidate_index} WHERE {}),
         counts AS (SELECT other.target_id,count(*) AS total {related} GROUP BY other.target_id)
         SELECT e.named_key,counts.total FROM counts JOIN entity e ON e.id=counts.target_id {} ORDER BY e.named_key",
        conditions.join(" AND "),
        if excluded.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", excluded.join(" AND "))
        }
    );
    read_counts(tx, &sql, params_from_iter(parameters.iter()))
}

/// Merge the ordered streams of identity IDs and eligible page IDs. The
/// difference includes archived, non-page and missing owners. Subtract their
/// edges once from each target's complete reverse-index count.
pub(crate) fn page_catalogue(tx: &Transaction<'_>, kind: FacetKind) -> Result<Vec<FacetEntry>> {
    let sql = "
        WITH excluded(id) AS MATERIALIZED (
            SELECT id FROM entity
            EXCEPT SELECT id FROM item INDEXED BY item_query_header WHERE type='page' AND state='active'
            ORDER BY 1
        ), deductions AS MATERIALIZED (
            SELECT r.target_id,count(*) AS total
            FROM excluded x CROSS JOIN relation r ON r.source_id=x.id AND r.kind=?1
            GROUP BY r.target_id
        ), totals AS MATERIALIZED (
            SELECT e.id,e.named_key,
                (SELECT count(*) FROM relation r WHERE r.target_id=e.id AND r.kind=?1)
                    - coalesce(d.total,0) AS total
            FROM entity e LEFT JOIN deductions d ON d.target_id=e.id
            WHERE e.named_kind=?1
        )
        SELECT t.named_key,t.total,decl.id IS NULL FROM totals t
        LEFT JOIN item decl INDEXED BY item_query_header ON decl.id=t.id AND decl.state='active'
        WHERE t.total>0 OR decl.id IS NOT NULL ORDER BY t.named_key";
    Ok(tx
        .prepare_cached(sql)?
        .query_map([kind.as_str()], |row| {
            Ok(FacetEntry {
                kind,
                key: row.get(0)?,
                usage_count: super::read_count(row, 1)?,
                inferred: row.get(2)?,
            })
        })?
        .collect::<rusqlite::Result<_>>()?)
}

fn read_counts(
    tx: &Transaction<'_>,
    sql: &str,
    parameters: impl rusqlite::Params,
) -> Result<Vec<TargetCount>> {
    Ok(tx
        .prepare_cached(sql)?
        .query_map(parameters, |row| {
            Ok(TargetCount {
                tid: row.get(0)?,
                count: super::read_count(row, 1)?,
            })
        })?
        .collect::<rusqlite::Result<_>>()?)
}
