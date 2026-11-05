// SPDX-License-Identifier: MPL-2.0
use crate::store::{Error, Result, query};
use crate::{
    domain::project::{query::QueryDocument, types::TaskStatus},
    records::{PassageRecord, ResultPage, TaskRecord},
};
use rusqlite::{Transaction, params_from_iter, types::Value};
use std::fmt::Write as _;

pub(crate) struct Scope<'a> {
    pub(crate) document: Option<&'a QueryDocument>,
    pub(crate) pin_scope_id: Option<&'a str>,
    pub(crate) timezone: chrono_tz::Tz,
    pub(crate) limit: u32,
    pub(crate) offset: u32,
}

impl Scope<'_> {
    pub(super) fn selected(&self, tx: &Transaction<'_>) -> Result<Option<(String, Vec<Value>)>> {
        self.document
            .filter(|doc| !doc.is_empty())
            .map(|doc| {
                query::compile(tx, doc, self.pin_scope_id, self.timezone)
                    .map(|compiled| compiled.selection())
            })
            .transpose()
    }
    fn validate(&self) -> Result<()> {
        if !(1..=200).contains(&self.limit) {
            return Err(Error::Invalid("Choose a page size from 1 to 200.".into()));
        }
        Ok(())
    }
}

pub(crate) fn tasks(
    tx: &Transaction<'_>,
    scope: &Scope<'_>,
    note_id: Option<&str>,
    task_query: Option<&QueryDocument>,
    status: TaskStatus,
) -> Result<ResultPage<TaskRecord>> {
    scope.validate()?;
    if note_id.is_some_and(|id| !crate::domain::project::identity::is_item_id(id)) {
        return Err(Error::Invalid("Invalid note ID.".into()));
    }
    let mut values = Vec::new();
    let mut filter =
        String::from("i.type='task' AND i.state='active' AND n.type='page' AND n.state='active'");
    if let Some(id) = note_id {
        values.push(Value::Text(id.to_owned()));
        filter.push_str(" AND n.uuid=?1");
    } else if let Some((selection, params)) = scope.selected(tx)? {
        values = params;
        let _ = write!(filter, " AND n.id IN ({selection})");
    }
    // Conditions on the tasks themselves, such as a context or a start date.
    if let Some(document) = task_query.filter(|document| !document.is_empty()) {
        let (selection, params) = query::compile(tx, document, scope.pin_scope_id, scope.timezone)?
            .selection_after(values.len());
        values.extend(params);
        let _ = write!(filter, " AND i.id IN ({selection})");
    }
    let completed = status.as_str() == "completed";
    filter.push_str(if completed {
        " AND t.completed_ms IS NOT NULL"
    } else {
        " AND t.completed_ms IS NULL"
    });
    let from = "task_projection t JOIN item i ON i.id=t.item_id JOIN item n ON n.id=t.note_id";
    let count = count(
        tx,
        &format!("SELECT count(*) FROM {from} WHERE {filter}"),
        &values,
    )?;
    let order = if completed {
        "t.completed_ms DESC,i.uuid ASC"
    } else {
        "(t.due_day IS NULL) ASC,t.due_day ASC,i.modified_ms DESC,i.uuid ASC"
    };
    let pagination = page(&mut values, scope);
    let sql = format!(
        "SELECT n.uuid,i.uuid,i.revision,n.title,t.text,json_extract(i.header_json,'$.createdAt'),json_extract(b.content_json,'$.completedAt'),json_extract(i.header_json,'$.meta.due_date'),json_extract(i.header_json,'$.modifiedAt'),i.header_json->'$.meta' FROM {from} JOIN body b ON b.id=i.body_id WHERE {filter} ORDER BY {order} {pagination}"
    );
    let items = tx
        .prepare(&sql)?
        .query_map(params_from_iter(values.iter()), |row| {
            Ok(TaskRecord {
                note_id: row.get(0)?,
                task_id: row.get(1)?,
                revision: row.get(2)?,
                note_title: row.get(3)?,
                text: row.get(4)?,
                created_at: row.get(5)?,
                completed_at: row.get(6)?,
                due_date: row.get(7)?,
                modified_at: row.get(8)?,
                properties: task_properties(&row.get::<_, String>(9)?),
            })
        })?
        .collect::<rusqlite::Result<_>>()?;
    Ok(ResultPage { items, count })
}

/// A task's own property values: its metadata without the fields every task has.
fn task_properties(meta: &str) -> serde_json::Map<String, serde_json::Value> {
    let mut properties: serde_json::Map<String, serde_json::Value> =
        serde_json::from_str(meta).unwrap_or_default();
    for key in ["type", "title", "state", "due_date"] {
        properties.remove(key);
    }
    properties
}

pub(crate) fn passages(
    tx: &Transaction<'_>,
    scope: &Scope<'_>,
    label: &str,
) -> Result<ResultPage<PassageRecord>> {
    scope.validate()?;
    if label.trim().is_empty() {
        return Err(Error::Invalid("Choose a paragraph label.".into()));
    }
    let (selection, mut values) = scope.selected(tx)?.map_or_else(
        || (String::new(), Vec::new()),
        |(sql, params)| (format!(" AND i.id IN ({sql})"), params),
    );
    values.push(Value::Text(label.to_owned()));
    // Archived pages keep their passages so restoring them reads no body.
    let filter = format!(
        "i.type='page' AND i.state='active' AND e.named_kind='label' AND e.named_key=?{}{selection}",
        values.len()
    );
    let from = "passage p JOIN item i ON i.id=p.item_id JOIN passage_label pl ON pl.item_id=p.item_id AND pl.block_id=p.block_id JOIN entity e ON e.id=pl.label_id";
    let count = count(
        tx,
        &format!("SELECT count(*) FROM {from} WHERE {filter}"),
        &values,
    )?;
    let pagination = page(&mut values, scope);
    let sql=format!("SELECT i.uuid,p.block_id,i.revision,i.title,p.text,p.fragment_json,p.truncated,
        (SELECT json_group_array(named_key) FROM (SELECT e2.named_key FROM passage_label l JOIN entity e2 ON e2.id=l.label_id WHERE l.item_id=p.item_id AND l.block_id=p.block_id ORDER BY e2.named_key))
        FROM {from} WHERE {filter} ORDER BY i.uuid,p.block_id {pagination}");
    let mut statement = tx.prepare(&sql)?;
    let mut rows = statement.query(params_from_iter(values.iter()))?;
    let mut items = Vec::new();
    while let Some(row) = rows.next()? {
        items.push(PassageRecord {
            item_id: row.get(0)?,
            block_id: row.get(1)?,
            revision: row.get(2)?,
            note_title: row.get(3)?,
            text: row.get(4)?,
            fragment: serde_json::from_str(&row.get::<_, String>(5)?)?,
            truncated: row.get(6)?,
            labels: serde_json::from_str(&row.get::<_, String>(7)?)?,
        });
    }
    Ok(ResultPage { items, count })
}

fn page(values: &mut Vec<Value>, scope: &Scope<'_>) -> String {
    values.push(Value::Integer(i64::from(scope.limit)));
    let limit = values.len();
    values.push(Value::Integer(i64::from(scope.offset)));
    format!("LIMIT ?{limit} OFFSET ?{}", values.len())
}

fn count(tx: &Transaction<'_>, sql: &str, values: &[Value]) -> Result<usize> {
    let count: i64 = tx.query_row(sql, params_from_iter(values.iter()), |row| row.get(0))?;
    usize::try_from(count).map_err(|error| Error::Invalid(error.to_string()))
}
