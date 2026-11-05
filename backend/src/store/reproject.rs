// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! Offline repair of disposable projections from the same sources as ordinary
//! writes. The caller publishes the new projector version in this transaction.

use super::{Error, Result, block_facts::Facts, body, changes, fields, fts, item, projections};
#[cfg(test)]
use crate::structured_query::fixtures::{criteria, greater_than};
use rusqlite::{Transaction, params};
use serde_json::Value;

pub(crate) fn rebuild(tx: &Transaction<'_>) -> Result<()> {
    // Tags and pins are authored edges. Every other relationship comes from
    // an item's content. Child rows (tokens, passage labels, decision targets)
    // are removed by the same foreign keys used during ordinary edits.
    tx.execute_batch(
        "DELETE FROM relation WHERE kind NOT IN ('tag','pin');
         DELETE FROM item_projection;
         DELETE FROM passage;
         DELETE FROM snippet_projection;
         DELETE FROM task_projection;
         DELETE FROM decision_projection;
         DELETE FROM field_presence;
         DELETE FROM field_option;
         DELETE FROM field_definition;
         DELETE FROM block_fact;
         INSERT INTO item_fts(item_fts) VALUES ('delete-all');",
    )?;
    visit(
        tx,
        "SELECT id FROM block WHERE id>?1 ORDER BY id LIMIT 128",
        |id| {
            let json: String = tx
                .prepare_cached("SELECT document_json FROM block WHERE id=?1")?
                .query_row([id], |row| row.get(0))?;
            let facts = Facts::new(&serde_json::from_str(&json)?);
            tx.prepare_cached("INSERT INTO block_fact(block_id,facts_json) VALUES (?1,?2)")?
                .execute(params![id, serde_json::to_string(&facts)?])?;
            Ok(())
        },
    )?;
    visit(
        tx,
        "SELECT id FROM item WHERE type='property' AND state='active' AND id>?1 ORDER BY id LIMIT 128",
        |id| {
            let body_id = tx
                .prepare_cached("SELECT body_id FROM item WHERE id=?1")?
                .query_row([id], |row| row.get(0))?;
            fields::declare(tx, id, &body::read(tx, body_id)?)
        },
    )?;
    visit(
        tx,
        "SELECT id FROM item WHERE id>?1 ORDER BY id LIMIT 128",
        |id| project(tx, id),
    )?;
    changes::reset_after_rebuild(tx)?;
    Ok(())
}

/// Keyset batches release the SELECT cursor before mutating derived keys. A
/// book-sized project holds at most one assembled item and one small ID batch.
fn visit(
    tx: &Transaction<'_>,
    sql: &str,
    mut process: impl FnMut(i64) -> Result<()>,
) -> Result<()> {
    let mut select = tx.prepare(sql)?;
    let mut last = 0;
    loop {
        let ids = select
            .query_map([last], |row| row.get(0))?
            .collect::<rusqlite::Result<Vec<i64>>>()?;
        let Some(&end) = ids.last() else {
            return Ok(());
        };
        for id in ids {
            process(id)?;
        }
        last = end;
    }
}

fn project(tx: &Transaction<'_>, id: i64) -> Result<()> {
    let (header, body): (String, i64) = tx
        .prepare_cached("SELECT header_json,body_id FROM item WHERE id=?1")?
        .query_row([id], |row| Ok((row.get(0)?, row.get(1)?)))?;
    let header: Value = serde_json::from_str(&header)?;
    let (title, created, modified) = item::header_keys(&header)?;
    tx.prepare_cached("UPDATE item SET title_sort=?2,created_ms=?3,modified_ms=?4 WHERE id=?1")?
        .execute(params![id, title, created, modified])?;
    let value = item::assemble(tx, header, body, &item::memberships(tx, id)?, true)?;
    let metadata = value["meta"]
        .as_object()
        .ok_or_else(|| Error::Invalid("Item has no metadata.".into()))?;
    fields::replace(tx, id, metadata, None)?;
    let projection = projections::Prepared::new(&value)?;
    projection.install(tx, id)?;
    fts::Prepared::new(&projection.document).install(tx, id)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::project::named_items;
    use crate::store::commands::assemble_item;
    use crate::store::{
        Store,
        mutations::{self, PreparedChange},
    };
    use serde_json::json;

    /// Includes canonical and private tables alongside disposable facts, with
    /// stable UUIDs/field keys rather than disposable dictionary row IDs.
    fn snapshot(tx: &Transaction<'_>) -> Result<Vec<Vec<String>>> {
        [
            "SELECT json_array(id,header_json,body_id,title_sort,created_ms,modified_ms) FROM item ORDER BY id",
            "SELECT json_array(id,content_json,hex(block_ids)) FROM body ORDER BY id",
            "SELECT json_array(revision_uuid,body_id,header_json,memberships_json) FROM revision ORDER BY revision_uuid",
            "SELECT json_array(block_id,facts_json) FROM block_fact ORDER BY block_id",
            "SELECT json_array(kind,source_id,target_id) FROM relation ORDER BY 1",
            "SELECT json_array(item_id,preview,word_count,export_facts_json,page_links_json) FROM item_projection ORDER BY item_id",
            "SELECT json_array(item_id,position,text,fragment_json) FROM passage ORDER BY item_id,position",
            "SELECT json_array(item_id,block_id,label_id) FROM passage_label ORDER BY 1",
            "SELECT json_array(f.key,v.item_id,v.text_value,v.number_value,v.time_value) FROM field_value v JOIN field_definition f ON f.id=v.field_id ORDER BY 1",
            "SELECT json_array(f.key,o.position,o.option_key) FROM field_option o JOIN field_definition f ON f.id=o.field_id ORDER BY 1",
            "SELECT json_array(term,doc,col,offset) FROM item_fts_terms ORDER BY term,doc,col,offset",
            "SELECT json_array(day,page_id,words_added,words_deleted,editing_events) FROM activity_daily ORDER BY day,page_id",
            "SELECT json_array(path,generation) FROM dirty_path ORDER BY path",
        ].iter().map(|sql| Ok(tx.prepare(sql)?.query_map([],|row|row.get(0))?.collect::<rusqlite::Result<_>>()?)).collect()
    }

    #[test]
    fn repairs_current_and_retained_facts_without_changing_author_data_or_history() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::create(directory.path(), uuid::Uuid::now_v7()).unwrap();
        let declaration = assemble_item(&json!({"id":named_items::id("property","stage"),"meta":{"type":"property","title":"Stage"},"content":{"key":"stage","valueType":"select","options":["review","draft"]}}),None,"UTC").unwrap();
        // Cross a batch boundary, then retain a block that only an old revision uses.
        let blocks = (0..130).map(|i| json!({"type":"paragraph","attrs":{"id":format!("block-{i}"),"labels":["quote"]},"content":[{"type":"text","text":format!("Chapter {i}")}]})).collect::<Vec<_>>();
        let page = assemble_item(&json!({"meta":{"type":"page","title":"A page","stage":"draft"},"tags":["research"],"content":{"document":{"type":"doc","content":blocks}}}),None,"UTC").unwrap();
        let mut content = page["content"].clone();
        content["document"]["content"][0]["content"][0]["text"] = json!("Revised chapter");
        let archived = assemble_item(
            &json!({"id":page["id"],"meta":{"state":"archived"},"content":content}),
            Some(&page),
            "UTC",
        )
        .unwrap();
        let records = [declaration, page.clone()]
            .into_iter()
            .map(|v| PreparedChange::new(v, None))
            .collect::<Result<Vec<_>>>()
            .unwrap();
        store
            .write(move |tx| mutations::apply(tx, &records, &[], Some("2026-11-01")))
            .unwrap();
        let updated = PreparedChange::new(
            archived,
            Some(page["revision"].as_str().unwrap().to_owned()),
        )
        .unwrap();
        store
            .write(move |tx| mutations::apply(tx, &[updated], &[], Some("2026-11-01")))
            .unwrap();
        let before = store.read(snapshot).unwrap();
        store
            .write(|tx| {
                tx.execute_batch(
                    "DELETE FROM block_fact; DELETE FROM item_projection; DELETE FROM field_option;
                DELETE FROM passage; DELETE FROM relation WHERE kind='label';
                UPDATE item SET title_sort='',created_ms=0,modified_ms=0;
                INSERT INTO item_fts(item_fts) VALUES ('delete-all')",
                )?;
                rebuild(tx)
            })
            .unwrap();
        assert_eq!(store.read(snapshot).unwrap(), before);
    }

    #[test]
    fn inferred_capabilities_and_search_survive_rebuild_and_declaration_reactivation() {
        use crate::domain::project::types::MetadataFieldValueType as Type;
        use crate::store::query::{self, SearchOptions};
        let directory = tempfile::tempdir().unwrap();
        let store = Store::create(directory.path(), uuid::Uuid::now_v7()).unwrap();
        let property_id = named_items::id("property", "event");
        let field=assemble_item(&json!({"id":property_id,"meta":{"type":"property","title":"Event"},"content":{"key":"event","valueType":"date"}}),None,"UTC").unwrap();
        let page = |meta: Value| {
            assemble_item(&json!({"meta":meta,"content":{"document":{"type":"doc","content":[{"type":"paragraph","attrs":{"id":"one"}}]}}}),None,"UTC").unwrap()
        };
        let owner = page(
            json!({"type":"page","title":"Owner","event":"2026-10-30","mixed":"hello","opposite":1,"flag":false,"number":0,"empty":[],"nothing":null}),
        );
        let second = page(
            json!({"type":"page","title":"Second","mixed":12,"opposite":"twelve","number":null}),
        );
        let owner_id = owner["id"].as_str().unwrap().to_owned();
        let records = [field, owner, second]
            .into_iter()
            .map(|v| PreparedChange::new(v, None))
            .collect::<Result<Vec<_>>>()
            .unwrap();
        store
            .write(move |tx| mutations::apply(tx, &records, &[], None))
            .unwrap();
        let change_state = |id: &str, state: &str| {
            let (id, state) = (id.to_owned(), state.to_owned());
            store
                .write(move |tx| {
                    let current = item::read_header(tx, &id)?.unwrap();
                    mutations::metadata(
                        tx,
                        &id,
                        current.value["revision"].as_str().unwrap(),
                        json!({"state":state}).as_object().unwrap(),
                    )?;
                    Ok(())
                })
                .unwrap();
        };
        change_state(&owner_id, "archived");
        change_state(&property_id, "archived");
        change_state(&owner_id, "active");
        let check = |tx: &Transaction<'_>, declared: bool| -> Result<()> {
            let fields = fields::definitions(tx)?;
            assert_eq!(fields["mixed"].kind, Type::Text);
            assert_eq!(fields["opposite"].kind, Type::Text);
            assert_eq!(fields["flag"].kind, Type::Checkbox);
            assert_eq!(fields["number"].kind, Type::Number);
            assert_eq!(fields["empty"].kind, Type::MultiSelect);
            assert_eq!(fields["nothing"].kind, Type::Text);
            assert_eq!(
                fields["event"].kind,
                if declared { Type::Date } else { Type::Text }
            );
            let search = |document: crate::structured_query::document::QueryDocument| {
                query::search(
                    tx,
                    &document,
                    &SearchOptions {
                        content: false,
                        limit: 10,
                        offset: 0,
                        pin_scope: None,
                        prioritize_pins: false,
                        timezone: chrono_tz::UTC,
                    },
                )
            };
            let result = search(criteria([crate::structured_query::fixtures::field(
                "event",
                "2026-10-30",
            )]))?;
            assert_eq!(
                result.items.iter().map(|hit| &hit.id).collect::<Vec<_>>(),
                [&owner_id]
            );
            if declared {
                assert_eq!(
                    search(criteria([greater_than("event", "2026-10-29")]))?
                        .items
                        .len(),
                    1
                );
            }
            Ok(())
        };
        for declared in [false, true] {
            if declared {
                change_state(&property_id, "active");
            }
            store.read(|tx| check(tx, declared)).unwrap();
            let before = store.read(snapshot).unwrap();
            store.write(rebuild).unwrap();
            store.read(|tx| check(tx, declared)).unwrap();
            assert_eq!(store.read(snapshot).unwrap(), before);
        }
    }
}
