// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use super::exports::{fixture, settled};
use axum::http::{StatusCode, header};
use serde_json::{Value, json};

fn request(record: &Value, format: &str, start: bool) -> Value {
    let mut definition = record["definition"].clone();
    definition["root"]["fontSize"] = json!("13pt");
    json!({"themeId":record["summary"]["id"],"revision":record["summary"]["revision"],"definition":definition,"format":format,"start":start})
}

#[test]
fn draft_previews_use_every_production_exporter_without_saving_or_indexing() {
    let f = fixture();
    let initial_items = crate::engine::project::item::list_project_item_ids(f.root.path()).unwrap();
    let builtin = f.call(
        "read_publication_theme_package",
        json!({"themeId":"classic"}),
    );
    for format in ["html", "site", "pdf", "docx", "epub"] {
        let job = f.call("preview_publication_theme", request(&builtin, format, true));
        let status = settled(&f, &job);
        assert_eq!(status["state"], "ready", "{status}");
        assert_eq!(status["themePreview"]["themeId"], "classic");
        assert_eq!(
            status["themePreview"]["revision"],
            builtin["summary"]["revision"]
        );
        assert_ne!(
            status["themePreview"]["draftSha256"],
            builtin["summary"]["revision"]
        );
        assert!(status.get("themeTrust").is_none());
        assert_eq!(status["summary"]["pages"], 3);
        assert_eq!(
            status["plan"]["value"]["theme"]["resolved"]["rootFontSizePt"].as_f64(),
            Some(13.0)
        );
        let (code, headers, bytes) = f.request("GET", status["artifactUrl"].as_str().unwrap(), "");
        assert_eq!(code, StatusCode::OK);
        match format {
            "pdf" => {
                assert_eq!(headers[header::CONTENT_TYPE], "application/pdf");
                assert!(bytes.starts_with(b"%PDF-"));
            }
            "epub" => {
                assert_eq!(headers[header::CONTENT_TYPE], "application/epub+zip");
                let parts = crate::engine::publication::docx::tests::parts(&bytes);
                assert!(parts.contains_key("EPUB/book.opf"));
            }
            "docx" => {
                assert_word_sample(&bytes, &headers);
            }
            _ => {
                let html = std::str::from_utf8(&bytes).unwrap();
                assert!(html.contains("Write once, publish anywhere"));
                assert!(html.contains(if format == "html" {
                    "data:font/woff2;base64,"
                } else {
                    "url('fonts/"
                }));
            }
        }
        if format == "site" {
            let route = status["plan"]["value"]["pages"]
                .as_array()
                .unwrap()
                .iter()
                .find(|page| page["title"] == "Typography and structure")
                .unwrap()["route"]
                .as_str()
                .unwrap();
            let (_, _, chapter) = f.request(
                "GET",
                &format!(
                    "/api/export_artifact/{}/{route}",
                    job["jobId"].as_str().unwrap()
                ),
                "",
            );
            assert!(
                std::str::from_utf8(&chapter)
                    .unwrap()
                    .contains("Heading six")
            );
        }
        assert_eq!(
            f.call("cancel_export", json!({"jobId":job["jobId"]})),
            json!({"cancelled":true})
        );
        assert_eq!(settled(&f, &job)["state"], "cancelled");
        assert_eq!(
            f.request("GET", status["artifactUrl"].as_str().unwrap(), "")
                .0,
            StatusCode::NOT_FOUND
        );
    }
    assert_eq!(
        f.call(
            "read_publication_theme_package",
            json!({"themeId":"classic"})
        ),
        builtin
    );
    assert_eq!(
        crate::engine::project::item::list_project_item_ids(f.root.path()).unwrap(),
        initial_items
    );
    assert!(!f.root.path().join("themes").exists());
}

fn assert_word_sample(bytes: &[u8], headers: &axum::http::HeaderMap) {
    assert_eq!(
        headers[header::CONTENT_TYPE],
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
    );
    assert!(
        headers[header::CONTENT_DISPOSITION]
            .to_str()
            .unwrap()
            .starts_with("attachment;")
    );
    let parts = crate::engine::publication::docx::tests::parts(bytes);
    let document = std::str::from_utf8(&parts["word/document.xml"]).unwrap();
    assert!(document.contains("Heading six"));
    assert!(document.contains("w:vMerge"));
    assert_eq!(
        std::str::from_utf8(&parts["word/footnotes.xml"])
            .unwrap()
            .matches("A shared reference.")
            .count(),
        1
    );
    assert!(parts.keys().any(|path| {
        std::path::Path::new(path)
            .extension()
            .is_some_and(|extension| extension == "odttf")
    }));
}

#[test]
fn preview_approval_uses_original_web_identity_and_rejects_stale_sources() {
    let f = fixture();
    let builtin = f.call(
        "read_publication_theme_package",
        json!({"themeId":"classic"}),
    );
    let custom = f.call(
        "duplicate_publication_theme",
        json!({"themeId":"classic","revision":builtin["summary"]["revision"],"name":"Custom"}),
    );
    let draft = request(&custom, "html", false);
    let validation = f.call("validate_publication_theme", json!({"themeId":draft["themeId"],"revision":draft["revision"],"definition":draft["definition"]}));
    let job = f.call("preview_publication_theme", draft.clone());
    let pending = settled(&f, &job);
    assert_eq!(pending["state"], "awaiting_theme_approval");
    assert_eq!(pending["themeTrust"]["hash"], validation["executionSha256"]);
    assert!(
        pending["themeTrust"]["root"]
            .as_str()
            .unwrap()
            .contains(custom["summary"]["id"].as_str().unwrap())
    );
    f.error(
        "approve_export_theme",
        json!({"jobId":job["jobId"],"hash":pending["plan"]["value"]["theme"]["executionSha256"]}),
        StatusCode::INTERNAL_SERVER_ERROR,
    );
    f.call(
        "approve_export_theme",
        json!({"jobId":job["jobId"],"hash":pending["themeTrust"]["hash"]}),
    );
    let repeated = f.call("preview_publication_theme", draft.clone());
    assert_eq!(settled(&f, &repeated)["state"], "prepared");
    let mut web_changed = draft.clone();
    web_changed["definition"]["web"]["controls"][0]["label"] = json!("Changed navigation");
    let changed_job = f.call("preview_publication_theme", web_changed);
    assert_eq!(
        settled(&f, &changed_job)["state"],
        "awaiting_theme_approval"
    );
    f.call("update_publication_theme", json!({"themeId":draft["themeId"],"revision":draft["revision"],"definition":draft["definition"]}));
    f.error("preview_publication_theme", draft, StatusCode::BAD_REQUEST);
    f.call("start_export", json!({"jobId":repeated["jobId"]}));
    let stale = settled(&f, &repeated);
    assert_eq!(stale["state"], "failed");
    assert!(
        stale["error"]
            .as_str()
            .unwrap()
            .contains("changed after preview")
    );
    assert!(stale.get("artifactUrl").is_none());
    assert!(f.error("approve_export_theme", json!({"jobId":changed_job["jobId"],"hash":settled(&f, &changed_job)["themeTrust"]["hash"]}), StatusCode::INTERNAL_SERVER_ERROR).contains("changed after preview"));
}

#[test]
fn invalid_drafts_and_stale_revisions_do_not_create_canonical_records() {
    let f = fixture();
    let initial_items = crate::engine::project::item::list_project_item_ids(f.root.path()).unwrap();
    let builtin = f.call(
        "read_publication_theme_package",
        json!({"themeId":"classic"}),
    );
    for (field, value) in [
        ("revision", json!("stale")),
        ("format", json!("mobi")),
        ("definition", json!({})),
    ] {
        let mut invalid = request(&builtin, "html", true);
        invalid[field] = value;
        f.error(
            "preview_publication_theme",
            invalid,
            StatusCode::BAD_REQUEST,
        );
    }
    let mut unsafe_path = request(&builtin, "html", true);
    unsafe_path["definition"]["web"]["html"] = json!("../outside.html");
    f.error(
        "preview_publication_theme",
        unsafe_path,
        StatusCode::BAD_REQUEST,
    );
    assert_eq!(
        f.call(
            "read_publication_theme_package",
            json!({"themeId":"classic"})
        ),
        builtin
    );
    assert_eq!(
        crate::engine::project::item::list_project_item_ids(f.root.path()).unwrap(),
        initial_items
    );
}
