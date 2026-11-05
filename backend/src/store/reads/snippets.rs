// SPDX-License-Identifier: MPL-2.0
use crate::store::{Error, Result, item};
use crate::{
    domain::project::snippets as canonical,
    records::{SnippetHit, SnippetResults, SnippetUsage},
};
use rusqlite::{Transaction, params};
use serde_json::json;

pub(crate) fn query(
    tx: &Transaction<'_>,
    text: &str,
    limit: u32,
    offset: u32,
) -> Result<SnippetResults> {
    if !(1..=200).contains(&limit) {
        return Err(Error::Invalid(
            "Snippet query limit must be between 1 and 200.".into(),
        ));
    }
    let term = text.trim().to_lowercase().replace('\n', " ");
    let filter = "FROM snippet_projection s JOIN item i ON i.id=s.item_id JOIN entity e ON e.id=i.id WHERE i.type='snippet' AND i.state='active' AND (instr(e.named_key,?1)>0 OR instr(s.search_text,?1)>0)";
    let count: i64 = tx.query_row(&format!("SELECT count(*) {filter}"), [&term], |row| {
        row.get(0)
    })?;
    let count = usize::try_from(count).map_err(|error| Error::Invalid(error.to_string()))?;
    let mut statement=tx.prepare(&format!("SELECT i.uuid,(SELECT count(DISTINCT r.source_id) FROM relation r JOIN item owner ON owner.id=r.source_id WHERE r.target_id=i.id AND r.kind IN ('snippet_citation','snippet_embed') AND owner.type='page') {filter} ORDER BY e.named_key LIMIT ?2 OFFSET ?3"))?;
    let ids = statement
        .query_map(params![term, i64::from(limit), i64::from(offset)], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut items = Vec::new();
    for (id, usage) in ids {
        let value = item::read(tx, &id, true)?.ok_or(Error::NotFound)?;
        let record = canonical::record(&value).map_err(Error::Invalid)?;
        items.push(SnippetHit {
            record: serde_json::from_value(record)?,
            usage_count: usize::try_from(usage)
                .map_err(|error| Error::Invalid(error.to_string()))?,
        });
    }
    Ok(SnippetResults { items, count })
}

pub(crate) fn usages(tx: &Transaction<'_>, id: &str) -> Result<Vec<SnippetUsage>> {
    canonical::validate_snippet_id(id).map_err(Error::Invalid)?;
    let mut statement=tx.prepare("SELECT DISTINCT i.uuid FROM relation r JOIN entity e ON e.id=r.target_id JOIN item i ON i.id=r.source_id WHERE e.named_kind='snippet' AND e.named_key=?1 AND r.kind IN ('snippet_citation','snippet_embed') AND i.type='page' ORDER BY i.uuid")?;
    let ids = statement
        .query_map([id], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut usages = Vec::new();
    for uuid in ids {
        let item = item::read(tx, &uuid, true)?.ok_or(Error::NotFound)?;
        let occurrences = canonical::occurrences_in_item(&item, id);
        if occurrences.is_empty() {
            continue;
        }
        usages.push(serde_json::from_value(json!({"noteId":uuid,"noteTitle":item["meta"]["title"].as_str().unwrap_or("Untitled"),"revision":item["revision"],"occurrences":occurrences}))?);
    }
    Ok(usages)
}
