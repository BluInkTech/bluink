// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use super::*;

fn paragraph(text: &str) -> Value {
    json!({"type":"paragraph","content":[{"type":"text","text":text}]})
}

#[test]
fn snippet_operations_preserve_identity_revisions_and_usage_guards() {
    {
        let project = TestProject::new("snippets");
        let app = project.application();
        app.initialize_sync().unwrap();
        assert_eq!(
            execute(&app, "read_snippet", json!({"id":"missing"})).unwrap(),
            Value::Null
        );
        let created = execute(
            &app,
            "create_snippet",
            json!({"id":"source","paragraph":paragraph("First")}),
        )
        .unwrap();
        assert_eq!(created["format"], "bluink.snippet");
        assert_eq!(created["formatVersion"], 1);
        assert_eq!(
            execute(&app, "read_snippet", json!({"id":"source"})).unwrap(),
            created
        );
        let updated = execute(&app, "update_snippet", json!({"id":"source","revision":created["revision"],"paragraph":paragraph("文学 [literal]")})).unwrap();
        assert_eq!(created["createdAt"], updated["createdAt"]);
        assert_ne!(created["revision"], updated["revision"]);
        for query in ["文学", "[literal]", "SOURCE"] {
            assert_eq!(
                execute(&app, "query_snippets", json!({"query":query})).unwrap(),
                json!({"count":1,"items":[{ "format": updated["format"], "formatVersion":1, "id":"source","state":"active","revision":updated["revision"],"createdAt":updated["createdAt"],"modifiedAt":updated["modifiedAt"],"paragraph":updated["paragraph"],"usageCount":0}]})
            );
        }
        assert_eq!(
            execute(&app, "query_snippets", json!({"query":".*"})).unwrap()["count"],
            0
        );
        assert_eq!(
            execute(&app, "query_snippets", json!({"limit":1,"offset":1})).unwrap()["items"],
            json!([])
        );
        for (operation, body) in [
            (
                "create_snippet",
                json!({"id":"source","paragraph":paragraph("Duplicate")}),
            ),
            (
                "update_snippet",
                json!({"id":"source","revision":created["revision"],"paragraph":paragraph("Stale")}),
            ),
            (
                "archive_snippet",
                json!({"id":"source","revision":created["revision"]}),
            ),
            ("query_snippets", json!({"limit":0})),
            ("query_snippets", json!({"limit":201})),
            ("read_snippet", json!({"id":"../escape"})),
            ("read_snippet_usages", json!({"id":"Bad Key"})),
            (
                "create_snippet",
                json!({"id":"invalid","paragraph":{"type":"heading"}}),
            ),
        ] {
            assert!(
                matches!(
                    execute(&app, operation, body),
                    Err(ApplicationError::InvalidRequest(_))
                ),
                "{operation}"
            );
        }
        let owner = execute(&app, "create_item", json!({"meta":{"type":"page","title":"Source owner"},"content":{"document": {"type":"doc","content":[{"type":"paragraph","content":[{"type":"citation","attrs":{"source":"snippet","snippetId":"source"}}]},{"type":"embed","attrs":{"snippetId":"source"}}]}}})).unwrap();
        let usages = execute(&app, "read_snippet_usages", json!({"id":"source"})).unwrap();
        assert_eq!(usages[0]["noteId"], owner["id"]);
        assert_eq!(usages[0]["noteTitle"], "Source owner");
        assert_eq!(usages[0]["occurrences"].as_array().unwrap().len(), 2);
        assert_eq!(
            execute(&app, "query_snippets", json!({})).unwrap()["items"][0]["usageCount"],
            1
        );
        assert!(
            matches!(execute(&app, "archive_snippet", json!({"id":"source","revision":updated["revision"]})), Err(ApplicationError::InvalidRequest(message)) if message.contains("used by"))
        );
        execute(&app, "update_item", json!({"id":owner["id"],"version":owner["version"],"meta":owner["meta"],"content":{"document": {"type":"doc","content":[{"type":"paragraph"}]}}})).unwrap();
        let archived = execute(
            &app,
            "archive_snippet",
            json!({"id":"source","revision":updated["revision"]}),
        )
        .unwrap();
        assert_eq!(archived["state"], "archived");
        assert_ne!(archived["revision"], updated["revision"]);
        assert_eq!(
            execute(&app, "query_snippets", json!({})).unwrap()["count"],
            0
        );
        app.stop_sync().unwrap();
        assert!(execute(&app, "query_snippets", json!({})).is_err());
        project.close().unwrap();
    }
}

#[test]
fn snippet_storage_damage_is_internal_and_failed_writes_leave_canonical_records_intact() {
    let project = TestProject::new("snippet-storage-errors");
    let app = project.application();
    app.initialize_sync().unwrap();
    let created = execute(
        &app,
        "create_snippet",
        json!({"id":"source","paragraph":paragraph("Original")}),
    )
    .unwrap();
    app.database().unwrap().write(|tx| {tx.execute_batch("CREATE TRIGGER reject_snippet_write BEFORE UPDATE ON item WHEN old.type='snippet' BEGIN SELECT RAISE(ABORT,'simulated storage failure'); END;")?;Ok(())}).unwrap();
    let result = execute(
        &app,
        "update_snippet",
        json!({"id":"source","revision":created["revision"],"paragraph":paragraph("Changed")}),
    );
    assert!(matches!(result, Err(ApplicationError::Internal(_))));
    assert_eq!(
        execute(&app, "read_snippet", json!({"id":"source"})).unwrap(),
        created
    );
    app.database()
        .unwrap()
        .write(|tx| {
            tx.execute_batch("DROP TRIGGER reject_snippet_write")?;
            Ok(())
        })
        .unwrap();
    app.stop_sync().unwrap();
    project.close().unwrap();
}
