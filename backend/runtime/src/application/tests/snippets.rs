// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use super::*;

fn paragraph(text: &str) -> Value {
    json!({"type":"paragraph","content":[{"type":"text","text":text}]})
}

#[test]
fn snippet_operations_preserve_identity_revisions_and_usage_guards_with_or_without_index() {
    for indexed in [false, true] {
        let app = application(if indexed {
            "snippets-indexed"
        } else {
            "snippets-direct"
        });
        app.initialize_sync(indexed).unwrap();
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
        let owner = execute(&app, "create_item", json!({"meta":{"type":"page","title":"Source owner"},"content":{"type":"doc","content":[{"type":"paragraph","content":[{"type":"citation","attrs":{"source":"snippet","snippetId":"source"}}]},{"type":"embed","attrs":{"snippetId":"source"}}]}})).unwrap();
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
        execute(&app, "update_item", json!({"id":owner["id"],"version":owner["version"],"meta":owner["meta"],"content":{"type":"doc","content":[{"type":"paragraph"}]}})).unwrap();
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
        assert_eq!(app.state().unwrap().index.is_some(), indexed);
        app.stop_sync().unwrap();
        assert!(execute(&app, "query_snippets", json!({})).is_err());
        std::fs::remove_dir_all(&app.root).unwrap();
    }
}

#[test]
fn snippet_storage_damage_is_internal_and_failed_writes_leave_canonical_records_intact() {
    let app = application("snippet-storage-errors");
    app.initialize_sync(false).unwrap();
    let created = execute(
        &app,
        "create_snippet",
        json!({"id":"source","paragraph":paragraph("Original")}),
    )
    .unwrap();
    let file = app.root.join("snippets/snippet_source.json");
    let bytes = std::fs::read(&file).unwrap();
    std::fs::write(&file, b"{").unwrap();
    for (operation, body) in [
        ("read_snippet", json!({"id":"source"})),
        (
            "update_snippet",
            json!({"id":"source","revision":created["revision"],"paragraph":paragraph("Changed")}),
        ),
        (
            "archive_snippet",
            json!({"id":"source","revision":created["revision"]}),
        ),
        ("query_snippets", json!({})),
    ] {
        assert!(
            matches!(
                execute(&app, operation, body),
                Err(ApplicationError::Internal(_))
            ),
            "{operation}"
        );
        assert_eq!(std::fs::read(&file).unwrap(), b"{");
    }
    std::fs::write(&file, bytes).unwrap();
    assert_eq!(
        execute(&app, "read_snippet", json!({"id":"source"})).unwrap(),
        created
    );
    let config = app.root.join("bluink.json");
    std::fs::remove_file(config).unwrap();
    assert!(matches!(
        execute(
            &app,
            "create_snippet",
            json!({"id":"another","paragraph":paragraph("Missing project")})
        ),
        Err(ApplicationError::Internal(_))
    ));
    app.stop_sync().unwrap();
    std::fs::remove_dir_all(&app.root).unwrap();
}
