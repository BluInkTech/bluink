// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use super::support::*;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::{Value, json};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

pub(super) fn fixture() -> Fixture {
    Fixture::with_options(false, |options| {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
        options.web_font_folder =
            Some(root.join("fonts/assets/web").to_string_lossy().into_owned());
        options.pdf_font_folder =
            Some(root.join("fonts/assets/pdf").to_string_lossy().into_owned());
        let binary = if cfg!(target_os = "windows") {
            "windows-x64/typst.exe"
        } else if cfg!(target_os = "macos") {
            "macos-arm64/typst"
        } else {
            "linux-x64/typst"
        };
        options.typst_binary = Some(
            root.join("typst/assets")
                .join(binary)
                .to_string_lossy()
                .into_owned(),
        );
    })
}

pub(super) fn settled(f: &Fixture, job: &Value) -> Value {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let value = f.call(
            "read_export",
            json!({"jobId":job["jobId"],"includePlan":true}),
        );
        if !matches!(
            value["state"].as_str().unwrap(),
            "planning" | "queued" | "rendering"
        ) {
            return value;
        }
        assert!(Instant::now() < deadline, "Export did not settle: {value}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

pub(super) fn page_request(id: &Value, format: &str, theme: &str) -> Value {
    json!({"source":{"kind":"note","id":id},"format":format,"start":false,"themeId":theme,"outsideLinks":"text-only","profile":"default"})
}

fn pages(f: &Fixture) -> (Value, Value) {
    let chapter=f.call("create_item",json!({"meta":{"type":"page","title":"Chapter","lang":"en"},"tags":["fiction"],"content":document("Chapter body.")}));
    let home=f.call("create_item",json!({"meta":{"type":"page","title":"Home","lang":"en"},"tags":["fiction"],"content":{"type":"doc","content":[{"type":"paragraph","content":[{"type":"text","text":"Welcome home. "},{"type":"text","text":"Chapter","marks":[{"type":"link","attrs":{"href":format!("/?note={}",chapter["id"].as_str().unwrap())}}]}]}]}}));
    (home, chapter)
}

#[test]
fn figure_titles_persist_and_generate_escaped_html_and_real_pdf_through_axum() {
    let f = fixture();
    let page = f.call(
        "create_item",
        json!({"meta":{"type":"page","title":"Titled figures","lang":"en"},"content":document("")}),
    );
    let assets = f.item_path(&page["id"]).parent().unwrap().join("assets");
    std::fs::write(assets.join("diagram.svg"),r#"<svg xmlns="http://www.w3.org/2000/svg" width="128" height="128"><circle cx="64" cy="64" r="48" fill="blue"/></svg>"#).unwrap();
    let figure = |title: &str, body: Value| json!({"type":"figure","content":[{"type":"blockTitle","content":[{"type":"text","text":title}]},body]});
    let content = json!({"type":"doc","content":[
        figure("Code <script> & title",json!({"type":"code_block","attrs":{"language":"rust"},"content":[{"type":"text","text":"let value = \"<verbatim>\";"}]})),
        figure("Table & title",json!({"type":"table","attrs":{"columnWidthsPercent":[100]},"content":[{"type":"table_row","content":[{"type":"table_header","content":[{"type":"paragraph","content":[{"type":"text","text":"Cell"}]}]}]}]})),
        figure("Image <title>",json!({"type":"image","attrs":{"src":format!("/assets/{}/diagram.svg",page["id"].as_str().unwrap()),"alt":"A blue circle","width":128}}))
    ]});
    let saved=f.call("update_item",json!({"id":page["id"],"version":page["version"],"meta":page["meta"],"content":{"content":content}}));
    let read = f.call("read_item", json!({"id":page["id"]}));
    for block in read["content"]["content"]["content"].as_array().unwrap() {
        assert_eq!(block["type"], "figure");
        assert_eq!(block["content"][0]["type"], "blockTitle");
        assert!(block["content"][0].get("attrs").is_none());
    }
    assert!(f.error("update_item",json!({"id":page["id"],"version":saved["version"],"meta":saved["meta"],"content":{"content":{"type":"doc","content":[{"type":"code_block","attrs":{"title":"Obsolete"}}]}}}),StatusCode::BAD_REQUEST).contains("Unsupported code_block attribute: title"));
    assert_eq!(
        f.call("read_item", json!({"id":page["id"]}))["version"],
        saved["version"]
    );
    for format in ["html", "pdf"] {
        let job = f.call(
            "create_export",
            page_request(&page["id"], format, "classic"),
        );
        assert_eq!(settled(&f, &job)["state"], "prepared");
        f.call("start_export", job.clone());
        let ready = settled(&f, &job);
        assert_eq!(ready["state"], "ready", "{ready}");
        let (status, _, bytes) = f.request("GET", ready["artifactUrl"].as_str().unwrap(), "");
        assert_eq!(status, StatusCode::OK);
        if format == "pdf" {
            assert!(bytes.starts_with(b"%PDF-"));
        } else {
            let html = std::str::from_utf8(&bytes).unwrap();
            for expected in [
                "<figcaption>Code &lt;script&gt; &amp; title</figcaption>",
                "<caption>Table &amp; title</caption>",
                "<figcaption>Image &lt;title&gt;</figcaption>",
                "<code>let value = &quot;&lt;verbatim&gt;&quot;;</code>",
            ] {
                assert!(html.contains(expected), "Missing {expected}");
            }
        }
    }
}

#[test]
fn captured_export_plans_are_immutable_artifacts_download_and_stale_content_fails() {
    let f = fixture();
    let (_, chapter) = pages(&f);
    for format in ["html", "pdf"] {
        let input = page_request(&chapter["id"], format, "classic");
        let first = f.call("create_export", input.clone());
        let stale = f.call("create_export", input);
        assert_eq!(settled(&f, &stale)["state"], "prepared");
        let mut prepared = settled(&f, &first);
        assert_eq!(prepared["state"], "prepared");
        assert!(prepared["plan"].is_object());
        prepared["plan"]["value"]["title"] = json!("Client mutation");
        assert_eq!(
            f.call(
                "read_export",
                json!({"jobId":first["jobId"],"includePlan":true})
            )["plan"]["value"]["title"],
            "Chapter"
        );
        assert!(f.call("read_export", first.clone()).get("plan").is_none());
        f.call("start_export", first.clone());
        let ready = settled(&f, &first);
        assert_eq!(ready["state"], "ready", "{ready}");
        let path = ready["artifactUrl"].as_str().unwrap();
        let (status, headers, bytes) = f.request("GET", path, "");
        assert_eq!(status, StatusCode::OK);
        if format == "html" {
            assert!(
                headers["content-security-policy"]
                    .to_str()
                    .unwrap()
                    .starts_with("sandbox;")
            );
            assert!(
                std::str::from_utf8(&bytes)
                    .unwrap()
                    .contains("Chapter body.")
            );
        } else {
            assert!(bytes.starts_with(b"%PDF-"));
        }
        let (status, headers, download) = f.request("GET", &format!("{path}?download=1"), "");
        assert_eq!(status, StatusCode::OK);
        assert!(
            headers["content-disposition"]
                .to_str()
                .unwrap()
                .contains("attachment")
        );
        assert_eq!(download, bytes);
        assert_eq!(f.call("cancel_export", first), json!({"cancelled":false}));
        let current = f.call("read_item", json!({"id":chapter["id"]}));
        let changed=f.call("update_item_meta",json!({"id":chapter["id"],"version":current["version"],"meta":{"title":"Changed after capture"}}));
        f.call("start_export", stale.clone());
        let failed = settled(&f, &stale);
        assert_eq!(failed["state"], "failed");
        assert!(
            failed["error"]
                .as_str()
                .unwrap()
                .to_lowercase()
                .contains("changed")
        );
        assert_eq!(
            f.request(
                "GET",
                &format!("/api/export_artifact/{}", stale["jobId"].as_str().unwrap()),
                ""
            )
            .0,
            StatusCode::NOT_FOUND
        );
        f.call(
            "update_item_meta",
            json!({"id":chapter["id"],"version":changed["version"],"meta":{"title":"Chapter"}}),
        );
    }
}

fn preview_request(url: &str, path: &str, method: &str) -> Vec<u8> {
    let address = url.strip_prefix("http://").unwrap().trim_end_matches('/');
    let mut stream = std::net::TcpStream::connect(address).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    write!(
        stream,
        "{method} /{path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    let mut bytes = Vec::new();
    stream.read_to_end(&mut bytes).unwrap();
    bytes
}

#[test]
fn publications_and_linked_sites_share_jobs_zip_bundles_and_confined_preview_servers() {
    let f = fixture();
    let (home, chapter) = pages(&f);
    let publication=f.call("create_item",json!({"meta":{"type":"publication","title":"Fiction book"},"content":{"kind":"publication","preset":"book","selectedOutput":"html","rootPageId":home["id"],"outline":[{"kind":"collection","id":"00000000-0000-7000-8000-000000000603","title":"Fiction","query":"#fiction"}],"selectionRules":[],"themeId":"classic","targets":{"pdf":{"profile":"default","outsideLinks":"error"},"html":{"outsideLinks":"error"},"site":{"layout":"documentation","outsideLinks":"error"}}}}));
    f.initialize_index();
    let html=f.call("create_export",json!({"source":{"kind":"publication","id":publication["id"]},"format":"html","start":true}));
    let generated = settled(&f, &html);
    assert_eq!(generated["state"], "ready", "{generated}");
    assert_eq!(generated["summary"]["pages"], 2);
    assert_eq!(generated["summary"]["collections"], 1);
    for source in [
        json!({"kind":"publication","id":publication["id"]}),
        json!({"kind":"linked-pages","id":home["id"]}),
    ] {
        let input = if source["kind"] == "publication" {
            json!({"source":source,"format":"site","start":true})
        } else {
            json!({"source":source,"format":"site","start":true,"themeId":"classic","outsideLinks":"text-only","profile":"default"})
        };
        let site = f.call("create_export", input);
        let ready = settled(&f, &site);
        assert_eq!(ready["state"], "ready", "{ready}");
        assert_eq!(ready["summary"]["pages"], 2);
        let previews = f
            .runtime
            .block_on(futures_util::future::join_all((0..4).map(|_| {
                f.send_async(
                    Request::builder()
                        .method("POST")
                        .uri("/api/start_export_preview")
                        .body(Body::from(site.to_string()))
                        .unwrap(),
                )
            })));
        let urls: Vec<Value> = previews
            .into_iter()
            .map(|(status, _, bytes)| {
                assert_eq!(status, StatusCode::OK);
                serde_json::from_slice(&bytes).unwrap()
            })
            .collect();
        let url = urls[0]["url"].as_str().unwrap().to_owned();
        for preview in urls {
            assert_eq!(preview["url"], url);
        }
        let preview = preview_request(&url, "", "GET");
        let preview = String::from_utf8_lossy(&preview);
        assert!(preview.starts_with("HTTP/1.1 200"));
        assert!(preview.contains("Welcome home."));
        assert!(preview.contains("default-src 'none'"));
        let font = preview_request(
            &url,
            "fonts/ibm-plex-mono/ibm-plex-mono-400-normal.woff2",
            "GET",
        );
        assert!(String::from_utf8_lossy(&font).contains("font/woff2"));
        assert!(font.len() > 100);
        for path in ["%2e%2findex.html", "%5cindex.html", "%zz", "missing.css"] {
            assert!(preview_request(&url, path, "GET").starts_with(b"HTTP/1.1 404"));
            assert_eq!(
                f.request(
                    "GET",
                    &format!(
                        "/api/export_artifact/{}/{path}",
                        site["jobId"].as_str().unwrap()
                    ),
                    ""
                )
                .0,
                StatusCode::NOT_FOUND
            );
        }
        assert!(preview_request(&url, "", "POST").starts_with(b"HTTP/1.1 405"));
        let bundle = ready["bundleUrl"].as_str().unwrap();
        let (status, headers, zip) = f.request("GET", bundle, "");
        assert_eq!(status, StatusCode::OK);
        assert_eq!(headers["content-type"], "application/zip");
        assert!(zip.starts_with(b"PK\x03\x04"));
        assert_eq!(f.request("GET", bundle, "").2, zip);
        let (status, headers, body) = f.request("GET", ready["artifactUrl"].as_str().unwrap(), "");
        assert_eq!(status, StatusCode::OK);
        assert!(
            headers["content-security-policy"]
                .to_str()
                .unwrap()
                .contains("sandbox allow-scripts")
        );
        assert_eq!(headers["cross-origin-resource-policy"], "cross-origin");
        assert!(String::from_utf8_lossy(&body).contains("Welcome home."));
        assert_eq!(ready["plan"]["kind"], "publication");
        assert!(
            ready["plan"]["value"]["pages"]
                .as_array()
                .unwrap()
                .iter()
                .any(|p| p["id"] == chapter["id"])
        );
    }
}

fn copy_directory(source: &Path, target: &Path) {
    std::fs::create_dir_all(target).unwrap();
    for entry in std::fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        if entry.file_type().unwrap().is_dir() {
            copy_directory(&entry.path(), &target.join(entry.file_name()));
        } else {
            std::fs::copy(entry.path(), target.join(entry.file_name())).unwrap();
        }
    }
}

#[test]
fn theme_approval_and_cancellation_are_session_scoped_and_hash_exact() {
    let f = fixture();
    let (_, chapter) = pages(&f);
    let theme = f.root.path().join("themes/field-notes");
    copy_directory(
        &PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../examples/themes/field-notes"),
        &theme,
    );
    let input = page_request(&chapter["id"], "html", "field-notes");
    let first = f.call("create_export", input.clone());
    let blocked = settled(&f, &first);
    assert_eq!(blocked["state"], "awaiting_theme_approval");
    assert_eq!(blocked["themeTrust"]["trusted"], false);
    f.error(
        "approve_export_theme",
        json!({"jobId":first["jobId"],"hash":"wrong"}),
        StatusCode::INTERNAL_SERVER_ERROR,
    );
    assert_eq!(
        f.call(
            "approve_export_theme",
            json!({"jobId":first["jobId"],"hash":blocked["themeTrust"]["hash"]})
        ),
        json!({"trusted":true})
    );
    f.call("start_export", first.clone());
    assert_eq!(settled(&f, &first)["state"], "ready");
    let reused = f.call("create_export", input.clone());
    assert_eq!(settled(&f, &reused)["state"], "prepared");
    assert_eq!(
        f.call("cancel_export", reused.clone()),
        json!({"cancelled":true})
    );
    assert_eq!(f.call("cancel_export", reused), json!({"cancelled":false}));
    let macros = theme.join("html/macros.html");
    let text = std::fs::read_to_string(&macros).unwrap();
    std::fs::write(macros, format!("{text}\n")).unwrap();
    let changed = f.call("create_export", input);
    let changed = settled(&f, &changed);
    assert_eq!(changed["state"], "awaiting_theme_approval");
    assert_ne!(changed["themeTrust"]["hash"], blocked["themeTrust"]["hash"]);
}

#[test]
fn invalid_exports_and_failed_publications_do_not_block_standalone_note_planning() {
    let f = fixture();
    let (home, chapter) = pages(&f);
    let mut empty_theme = page_request(&home["id"], "pdf", "classic");
    empty_theme["themeId"] = json!("");
    for body in [
        Value::Null,
        json!([]),
        json!({}),
        json!({"source":{"kind":"note","id":"bad"},"format":"pdf","start":false}),
        empty_theme,
    ] {
        f.error("create_export", body, StatusCode::BAD_REQUEST);
    }
    let failed = f.call(
        "create_export",
        json!({"source":{"kind":"publication","id":home["id"]},"format":"html","start":true}),
    );
    assert_eq!(settled(&f, &failed)["state"], "failed");
    let note = f.call(
        "create_export",
        page_request(&chapter["id"], "html", "classic"),
    );
    assert_eq!(settled(&f, &note)["state"], "prepared");
    assert_eq!(
        f.call("cancel_export", json!({"jobId":"missing"})),
        json!({"cancelled":false})
    );
    f.error("read_export", json!({}), StatusCode::BAD_REQUEST);
    f.error(
        "read_export",
        json!({"jobId":"missing"}),
        StatusCode::INTERNAL_SERVER_ERROR,
    );
}
