// SPDX-License-Identifier: MPL-2.0

use super::*;
use crate::store::{Store, fields, fts, item, projections};
use crate::structured_query::fixtures::{criteria, field, tag};
use rusqlite::{
    Transaction,
    hooks::{AuthAction, AuthContext, Authorization},
};
use serde_json::{Value, json};
use uuid::Uuid;

fn database() -> (tempfile::TempDir, Store) {
    let root = tempfile::tempdir().unwrap();
    let store = Store::create(root.path(), Uuid::now_v7()).unwrap();
    (root, store)
}
#[expect(
    clippy::needless_pass_by_value,
    reason = "The fixture consumes inline JSON input and installs an independent canonical record."
)]
fn put(tx: &Transaction<'_>, input: Value) -> Value {
    let value = crate::store::commands::assemble_item(&input, None, "UTC").unwrap();
    let prepared = item::PreparedItem::new(&value).unwrap();
    let projection = projections::Prepared::new(&value).unwrap();
    let id = prepared.install(tx, item::ExpectedRevision::New).unwrap();
    fields::replace(tx, id, value["meta"].as_object().unwrap(), None).unwrap();
    projection.install(tx, id).unwrap();
    fts::Prepared::new(&projection.document)
        .install(tx, id)
        .unwrap();
    value
}
#[expect(
    clippy::needless_pass_by_value,
    reason = "Inline fixture values are assembled into one owned JSON document."
)]
fn page(title: &str, tags: Value, blocks: Value) -> Value {
    json!({"meta":{"type":"page","title":title},"tags":tags,"content":{"document":{"type":"doc","content":blocks}}})
}
#[expect(
    clippy::needless_pass_by_value,
    reason = "Inline fixture labels are assembled into one owned paragraph."
)]
fn paragraph(id: &str, text: &str, labels: Value) -> Value {
    json!({"type":"paragraph","attrs":{"id":id,"labels":labels},"content":[{"type":"text","text":text}]})
}
fn scope(
    document: Option<&crate::domain::project::query::QueryDocument>,
) -> collections::Scope<'_> {
    collections::Scope {
        document,
        pin_scope_id: None,
        timezone: chrono_tz::UTC,
        limit: 50,
        offset: 0,
    }
}

#[test]
fn metadata_reads_and_search_do_not_load_body_payloads() {
    let (_root, store) = database();
    let value = store
        .write(|tx| {
            Ok(put(
                tx,
                page(
                    "Visible title",
                    json!([]),
                    json!([paragraph("p", "Private prose", json!([]))]),
                ),
            ))
        })
        .unwrap();
    store
        .read(|tx| {
            tx.flush_prepared_statement_cache();
            tx.authorizer(Some(|context: AuthContext<'_>| match context.action {
                AuthAction::Read {
                    table_name: "body",
                    column_name: "content_json",
                    ..
                }
                | AuthAction::Read {
                    table_name: "block",
                    column_name: "document_json",
                    ..
                } => Authorization::Deny,
                _ => Authorization::Allow,
            }))?;
            let item = items::read_item(tx, value["id"].as_str().unwrap(), false)?;
            assert!(item.content.is_empty());
            let request =
                serde_json::from_value(json!({"query":{"version":1,"clauses":[],"sort":[]}}))
                    .unwrap();
            let result = items::query_items(tx, &request, chrono_tz::UTC)?;
            assert_eq!(result.count, 1);
            assert!(result.items[0].content.is_empty());
            tx.authorizer(None::<fn(AuthContext<'_>) -> Authorization>)?;
            let complete = items::read_item(tx, value["id"].as_str().unwrap(), true)?;
            assert_eq!(
                complete.content["document"]["content"][0]["content"][0]["text"],
                "Private prose"
            );
            Ok(())
        })
        .unwrap();
}

#[test]
fn tasks_scope_to_active_owners_and_passages_paginate_exact_label_matches() {
    use crate::domain::project::types::TaskStatus;
    let (_root, store) = database();
    store.write(|tx| {
        let first=put(tx,page("Target",json!(["chosen"]),json!([paragraph("a","First",json!(["quote","source"])),paragraph("b","Second",json!(["quote"]))])));
        let other=put(tx,page("Other",json!([]),json!([{"type":"paragraph","attrs":{"id":"p"}}])));
        for (owner,due,text) in [(&first,"2026-12-01","Later"),(&first,"2026-11-01","Earlier"),(&other,"2026-10-01","Unrelated")] {
            put(tx,json!({"meta":{"type":"task","title":text,"due_date":due},"content":{"noteId":owner["id"],"text":text}}));
        }
        let query=criteria([tag("chosen")]);
        let result=collections::tasks(tx,&scope(Some(&query)),None,None,TaskStatus::Open)?;
        assert_eq!(result.count,2);
        assert_eq!(result.items[0].text,"Earlier");
        // Task conditions combine with the page scope in one statement.
        let early:crate::domain::project::query::QueryDocument=serde_json::from_value(json!({"version":1,"clauses":[{"polarity":"required","predicate":{"kind":"field","field":"due_date","operator":"lt","value":"2026-11-15"}}],"sort":[]})).unwrap();
        let result=collections::tasks(tx,&scope(Some(&query)),None,Some(&early),TaskStatus::Open)?;
        assert_eq!(result.items.iter().map(|task|task.text.as_str()).collect::<Vec<_>>(),["Earlier"]);
        let mut selected=scope(Some(&query));selected.limit=1;selected.offset=1;
        let result=collections::passages(tx,&selected,"quote")?;
        assert_eq!(result.count,2);let last=first["content"]["document"]["content"].as_array().unwrap().iter().filter_map(|block|block["attrs"]["id"].as_str()).max().unwrap();
        assert_eq!(result.items[0].block_id,last);
        assert_eq!(result.items[0].note_title,"Target");
        tx.execute("UPDATE item SET header_json=json_set(header_json,'$.meta.state','archived') WHERE uuid=?1",[first["id"].as_str().unwrap()])?;
        assert_eq!(collections::tasks(tx,&scope(Some(&query)),None,None,TaskStatus::Open)?.count,0);
        assert_eq!(collections::passages(tx,&scope(None),"quote")?.count,0);
        Ok(())
    }).unwrap();
}

#[test]
fn snippet_search_is_literal_unicode_substring_and_usage_is_distinct_per_page() {
    let (_root, store) = database();
    store.write(|tx| {
        let key="source_quote";
        put(tx,json!({"id":crate::domain::project::named_items::id("snippet",key),"meta":{"type":"snippet"},"content":{"key":key,"paragraph":{"type":"paragraph","content":[{"type":"text","text":"A CAFÉ [literal] quotation"}]}}}));
        put(tx,page("Uses snippet",json!([]),json!([
            {"type":"paragraph","attrs":{"id":"p"},"content":[{"type":"citation","attrs":{"source":"snippet","snippetId":key}}]},
            {"type":"embed","attrs":{"id":"e","snippetId":key}}
        ])));
        let result=snippets::query(tx,"fé [literal]",50,0)?;
        assert_eq!(result.count,1);assert_eq!(result.items[0].usage_count,1);
        assert_eq!(snippets::query(tx,"f*",50,0)?.count,0);
        let usages=snippets::usages(tx,key)?;
        let value=serde_json::to_value(usages)?;
        assert_eq!(value.as_array().unwrap().len(),1);
        assert_eq!(value[0]["occurrences"].as_array().unwrap().len(),2);
        Ok(())
    }).unwrap();
}

#[test]
fn dashboard_and_calendar_retain_counts_and_author_date_offsets() {
    use crate::domain::project::types::CalendarGranularity;
    let (_root, store) = database();
    store.write(|tx| {
        let page=put(tx,page("Written",json!(["chosen"]),json!([{"type":"paragraph","attrs":{"id":"p"}}])));
        let id:i64=tx.query_row("SELECT id FROM item WHERE uuid=?1",[page["id"].as_str().unwrap()],|row|row.get(0))?;
        tx.execute("INSERT INTO activity_daily VALUES ('2026-11-01',?1,10,2,1,3),('2026-11-02',?1,4,1,1,0)",[id])?;
        tx.execute("UPDATE item SET header_json=json_set(header_json,'$.meta.written','2026-11-01T23:30:00-05:00') WHERE id=?1",[id])?;
        let query=criteria([tag("chosen")]);
        let selected=scope(Some(&query));
        let report=serde_json::to_value(analytics::dashboard(tx,&selected,Some("2026-11-01"),"2026-11-02")?)?;
        assert_eq!(report["summary"]["wordsAdded"],14);
        assert_eq!(report["summary"]["tasksCompleted"],3);
        assert_eq!(report["summary"]["currentStreak"],2);
        assert_eq!(report["days"][0]["notesTouched"],1);
        let calendar=calendar::buckets(tx,&selected,"meta.written","2026-11-01","2026-11-01",CalendarGranularity::Day)?;
        assert_eq!(calendar.buckets.len(),1);assert_eq!(calendar.buckets[0].date,"2026-11-01");
        let completed=calendar::buckets(tx,&selected,"completed","2026-11-01","2026-11-02",CalendarGranularity::Day)?;
        assert_eq!(completed.buckets[0].count,1);
        // 01:00Z on 31 October is still 30 October in Los Angeles, matching `created:2026-10-30`.
        tx.execute("UPDATE item SET created_ms=?2 WHERE id=?1",rusqlite::params![id,1_793_408_400_000_i64])?;
        let mut local=scope(Some(&query));local.timezone=chrono_tz::America::Los_Angeles;
        let created=calendar::buckets(tx,&local,"created","2026-10-01","2026-10-31",CalendarGranularity::Day)?;
        assert_eq!(created.buckets.len(),1);assert_eq!(created.buckets[0].date,"2026-10-30");
        let ids=crate::store::query::matching_ids(tx,&criteria([tag("chosen"), field("created", "2026-10-30")]),None,None,local.timezone)?;
        assert_eq!(ids.len(),1);
        Ok(())
    }).unwrap();
}
