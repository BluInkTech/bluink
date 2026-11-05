// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use super::*;
use crate::host::session::ApplicationHandle;
use std::{
    io::{Read, Write},
    net::TcpStream,
};

const PAGE: &str = "00000000-0000-7000-8000-000000000601";
const TARGET: &str = "00000000-0000-7000-8000-000000000602";

fn fixture(project: &TestProject) -> Application {
    let mut app = project.application();
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
    app.options.web_font_folder =
        Some(root.join("fonts/assets/web").to_string_lossy().into_owned());
    app.options.docx_font_folder = Some(
        root.join("fonts/assets/docx")
            .to_string_lossy()
            .into_owned(),
    );
    app.initialize_sync().unwrap();
    for (id, title, content) in [
        (
            PAGE,
            "Home",
            json!([
                {"type": "paragraph", "content": [
                    {"type": "text", "text": "Welcome home. "},
                    {"type": "text", "text": "Chapter", "marks": [{"type": "link", "attrs": {"href": format!("/?note={TARGET}")}}]}
                ]}
            ]),
        ),
        (
            TARGET,
            "Chapter",
            json!([{"type": "paragraph", "content": [{"type": "text", "text": "Chapter body."}]}]),
        ),
    ] {
        crate::application::item_writes::write_item_input(&app.root,
            json!({"id": id, "meta": {"type": "page", "title": title, "lang": "en"}, "content": {"document": {"type": "doc", "content": content}}}),
            None, &app.activity_date().unwrap()).unwrap();
    }
    app
}
#[expect(
    clippy::needless_pass_by_value,
    reason = "Test requests own fixture values just like decoded production requests."
)]
fn run(app: &Application, operation: &str, request: Value) -> Value {
    let request = operations::Request::decode(operation, request.to_string().as_bytes()).unwrap();
    serde_json::to_value(app.execute(request).unwrap()).unwrap()
}

/// Linked-pages sources follow every reachable page here, as these scenarios expect.
fn source_json(kind: &str) -> Value {
    if kind == "linked-pages" {
        json!({"kind": kind, "id": PAGE, "depth": "reachable"})
    } else {
        json!({"kind": kind, "id": PAGE})
    }
}

fn create(app: &Application, source: &str, format: &str, start: bool, theme: &str) -> String {
    run(
        app,
        "create_export",
        json!({"source": source_json(source), "format": format,
        "start": start, "themeId": theme, "outsideLinks": "text-only", "profile": "default"}),
    )["jobId"]
        .as_str()
        .unwrap()
        .to_owned()
}

fn settled(app: &Application, id: &str) -> Value {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let value = run(
            app,
            "read_export",
            json!({"jobId": id, "includePlan": true}),
        );
        if !matches!(
            value["state"].as_str().unwrap(),
            "planning" | "queued" | "rendering"
        ) {
            return value;
        }
        assert!(Instant::now() < deadline, "Export did not settle: {value}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn word_artifact_leases_hold_shutdown_and_release_temporary_files() {
    let project = TestProject::new("word-download-lease");
    let app = Arc::new(fixture(&project));
    let id = create(&app, "note", "docx", true, "classic");
    assert_eq!(settled(&app, &id)["state"], "ready");
    let artifact = app.open_export_artifact(&id, "").unwrap().unwrap();
    let path = artifact.path.clone();
    assert_eq!(
        artifact.content_type,
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
    );
    assert!(path.exists());
    let stopping = Arc::clone(&app);
    let stopped = std::thread::spawn(move || stopping.stop_sync());
    let deadline = Instant::now() + Duration::from_secs(5);
    while !app.stopping.load(Ordering::Acquire) {
        assert!(Instant::now() < deadline);
        std::thread::yield_now();
    }
    assert!(!stopped.is_finished());
    assert!(path.exists());
    drop(artifact);
    stopped.join().unwrap().unwrap();
    assert!(!path.exists());
    assert!(app.open_export_artifact(&id, "").is_err());
    project.close().unwrap();
}

fn get(address: &str, path: &str) -> String {
    let mut stream = TcpStream::connect(address).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    let mut bytes = Vec::new();
    stream.read_to_end(&mut bytes).unwrap();
    String::from_utf8_lossy(&bytes).into_owned()
}

#[test]
fn typed_jobs_hold_immutable_plans_and_cleanup_html_outputs() {
    let project = TestProject::new("export-html");
    let app = fixture(&project);
    let id = create(&app, "note", "html", false, "classic");
    let mut status = settled(&app, &id);
    assert_eq!(status["state"], "prepared", "{status}");
    assert_eq!(status["plan"]["kind"], "page");
    status["plan"]["value"]["title"] = json!("Untrusted client mutation");
    assert_eq!(
        run(
            &app,
            "read_export",
            json!({"jobId": id, "includePlan": true})
        )["plan"]["value"]["title"],
        "Home"
    );
    assert!(
        run(&app, "read_export", json!({"jobId": id}))
            .get("plan")
            .is_none()
    );
    run(&app, "start_export", json!({"jobId": id}));
    let status = settled(&app, &id);
    assert_eq!(status["state"], "ready", "{status}");
    let artifact = app.export_artifact(&id, "").unwrap().unwrap();
    assert_eq!(artifact.content_type, "text/html; charset=utf-8");
    assert!(
        std::fs::read_to_string(&artifact.path)
            .unwrap()
            .contains("Welcome home.")
    );
    assert_eq!(
        run(&app, "cancel_export", json!({"jobId": id}))["cancelled"],
        false
    );
    assert!(app.database().is_ok());
    app.stop_sync().unwrap();
    assert!(!PathBuf::from(artifact.path).exists());
    project.close().unwrap();
}

#[test]
fn page_export_issues_read_what_a_save_recorded() {
    let project = TestProject::new("export-page-issues");
    let app = fixture(&project);
    assert_eq!(
        run(&app, "read_page_export_issues", json!({"id": PAGE})),
        json!([])
    );
    let page = run(&app, "read_item", json!({"id": PAGE}));
    run(
        &app,
        "update_item",
        json!({"id": PAGE, "version": page["version"], "meta": page["meta"], "content": {"document": {"type": "doc", "content": [
            {"type": "paragraph", "content": [{"type": "text", "text": "Bad", "marks": [{"type": "link", "attrs": {"href": "not a link"}}]}]}
        ]}}}),
    );
    let issues = run(&app, "read_page_export_issues", json!({"id": PAGE}));
    assert_eq!(issues.as_array().unwrap().len(), 1, "{issues}");
    assert_eq!(issues[0]["message"], "link cannot be exported: not a link.");
    assert!(issues[0]["blockId"].is_string());
    let missing = operations::Request::decode(
        "read_page_export_issues",
        json!({"id": "00000000-0000-7000-8000-00000000dead"})
            .to_string()
            .as_bytes(),
    )
    .unwrap();
    assert!(app.execute(missing).is_err());
}

#[test]
fn linked_jobs_report_the_links_generation_resolves() {
    let project = TestProject::new("export-linked-summary");
    let app = fixture(&project);
    // Planned only: links resolve when generation reads the pages.
    let prepared = create(&app, "linked-pages", "site", false, "classic");
    let status = settled(&app, &prepared);
    assert_eq!(status["state"], "prepared", "{status}");
    assert_eq!(status["summary"]["links"], 0);
    assert_eq!(status["plan"]["value"]["unresolved"], true);
    // Generated: the job keeps the resolved plan, and its summary counts it.
    let generated = create(&app, "linked-pages", "site", true, "classic");
    let status = settled(&app, &generated);
    assert_eq!(status["state"], "ready", "{status}");
    assert_eq!(status["summary"]["pages"], 2);
    assert_eq!(status["summary"]["links"], 1);
    assert!(
        status["plan"]["value"].get("unresolved").is_none(),
        "{status}"
    );
}

#[test]
#[expect(
    clippy::needless_collect,
    reason = "Collect all worker handles before joining so work remains concurrent."
)]
fn linked_site_preview_reuses_listener_and_shutdown_closes_connections() {
    let project = TestProject::new("export-preview");
    let app = Arc::new(fixture(&project));
    let id = create(&app, "linked-pages", "site", true, "classic");
    let status = settled(&app, &id);
    assert_eq!(status["state"], "ready", "{status}");
    assert_eq!(status["summary"]["pages"], 2);
    let urls = std::thread::scope(|scope| {
        let workers = (0..4)
            .map(|_| scope.spawn(|| run(&app, "start_export_preview", json!({"jobId": id}))))
            .collect::<Vec<_>>();
        workers
            .into_iter()
            .map(|worker| worker.join().unwrap()["url"].as_str().unwrap().to_owned())
            .collect::<Vec<_>>()
    });
    assert!(urls.iter().all(|url| url == &urls[0]));
    let address = urls[0]
        .strip_prefix("http://")
        .unwrap()
        .trim_end_matches('/');
    // A preview must stay alive beyond its shutdown-drain deadline.
    std::thread::sleep(Duration::from_millis(1100));
    let response = get(address, "/");
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    assert!(response.contains("content-type: text/html"));
    assert!(response.contains("content-security-policy: default-src 'none'"));
    assert!(response.contains("x-content-type-options: nosniff"));
    for path in [
        "/%2e%2e/index.html",
        "/%5cindex.html",
        "/%00index.html",
        "/%zz",
        "/missing.css",
    ] {
        assert!(get(address, path).starts_with("HTTP/1.1 404"), "{path}");
    }
    assert!(
        get(
            address,
            "/fonts/ibm-plex-mono/ibm-plex-mono-400-normal.woff2"
        )
        .contains("content-type: font/woff2")
    );
    let mut zip = app.export_site_zip(&id).unwrap().unwrap();
    let mut bytes = Vec::new();
    zip.file.read_to_end(&mut bytes).unwrap();
    assert!(bytes.starts_with(b"PK\x03\x04"));
    let mut again = app.export_site_zip(&id).unwrap().unwrap();
    let mut second = Vec::new();
    again.file.read_to_end(&mut second).unwrap();
    assert_eq!(bytes, second);
    // Close streams before testing shutdown, which waits for artifact leases.
    drop(zip);
    drop(again);
    assert!(app.export_artifact(&id, "../index.html").unwrap().is_none());
    let artifact = app.export_artifact(&id, "index.html").unwrap().unwrap();
    let mut connected = TcpStream::connect(address).unwrap();
    connected
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    // Leave a request incomplete so graceful drain has to force this connection closed.
    connected
        .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\n")
        .unwrap();
    app.stop_sync().unwrap();
    assert!(TcpStream::connect(address).is_err());
    let mut byte = [0];
    assert!(matches!(connected.read(&mut byte), Ok(0) | Err(_)));
    assert!(!PathBuf::from(artifact.path).exists());
    project.close().unwrap();
}

#[test]
fn stale_content_fails_generation_and_cancelled_jobs_cannot_restart() {
    let project = TestProject::new("export-stale");
    let app = fixture(&project);
    let id = create(&app, "note", "html", false, "classic");
    assert_eq!(settled(&app, &id)["state"], "prepared");
    let version = run(&app, "read_item", json!({"id": PAGE}))["version"].clone();
    app.execute(
        operations::Request::decode(
            "update_item_meta",
            json!({"id": PAGE, "version": version, "meta": {"title": "Changed source"}})
                .to_string()
                .as_bytes(),
        )
        .unwrap(),
    )
    .unwrap();
    run(&app, "start_export", json!({"jobId": id}));
    let status = settled(&app, &id);
    assert_eq!(status["state"], "failed", "{status}");
    assert!(status["error"].as_str().unwrap().contains("changed"));
    assert!(app.export_artifact(&id, "").unwrap().is_none());
    let id = create(&app, "note", "html", false, "classic");
    assert_eq!(settled(&app, &id)["state"], "prepared");
    assert_eq!(
        run(&app, "cancel_export", json!({"jobId": id}))["cancelled"],
        true
    );
    assert_eq!(
        run(&app, "cancel_export", json!({"jobId": id}))["cancelled"],
        false
    );
    assert!(
        app.execute(
            operations::Request::decode(
                "start_export",
                json!({"jobId": id}).to_string().as_bytes()
            )
            .unwrap()
        )
        .is_err()
    );
    app.stop_sync().unwrap();
    project.close().unwrap();
}

fn copy_theme(app: &Application) {
    let source = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../examples/themes/field-notes");
    crate::store::theme_packages::install_fixture(&app.root, "field-notes", &source);
}

#[test]
fn theme_approval_is_application_local_and_invalidated_by_theme_changes() {
    let first_project = TestProject::new("export-trust-first");
    let first = fixture(&first_project);
    let second_project = TestProject::new("export-trust-second");
    let second = fixture(&second_project);
    copy_theme(&first);
    copy_theme(&second);
    let id = create(&first, "note", "html", false, "field-notes");
    let status = settled(&first, &id);
    assert_eq!(status["state"], "awaiting_theme_approval", "{status}");
    let hash = status["themeTrust"]["hash"].as_str().unwrap();
    assert!(
        first
            .execute(
                operations::Request::decode(
                    "approve_export_theme",
                    json!({"jobId": id, "hash": "wrong"}).to_string().as_bytes()
                )
                .unwrap()
            )
            .is_err()
    );
    run(
        &first,
        "approve_export_theme",
        json!({"jobId": id, "hash": hash}),
    );
    assert_eq!(settled(&first, &id)["state"], "prepared");
    let repeated = create(&first, "note", "html", false, "field-notes");
    assert_eq!(settled(&first, &repeated)["state"], "prepared");
    let package = run(
        &first,
        "read_publication_theme_package",
        json!({"themeId":"field-notes"}),
    );
    let mut definition = package["definition"].clone();
    definition["root"]["fontSize"] = json!("14pt");
    run(
        &first,
        "update_publication_theme",
        json!({"themeId":"field-notes","revision":package["summary"]["revision"],"definition":definition}),
    );
    let restyled = create(&first, "note", "html", false, "field-notes");
    let data_status = settled(&first, &restyled);
    assert_eq!(data_status["state"], "prepared");
    assert_eq!(data_status["themeTrust"]["hash"], hash);
    run(&first, "start_export", json!({"jobId":repeated}));
    assert_eq!(settled(&first, &repeated)["state"], "failed");
    let isolated = create(&second, "note", "html", false, "field-notes");
    assert_eq!(
        settled(&second, &isolated)["state"],
        "awaiting_theme_approval"
    );
    let (stamp, mut files) =
        crate::store::theme_packages::load(&first.root, "field-notes").unwrap();
    files
        .get_mut("html/article.html")
        .unwrap()
        .extend_from_slice(b"\n<!-- changed theme -->");
    crate::store::theme_packages::save(&first.root, "field-notes", Some(stamp), None, &files)
        .unwrap();
    let changed = create(&first, "note", "html", false, "field-notes");
    assert_eq!(
        settled(&first, &changed)["state"],
        "awaiting_theme_approval"
    );
    run(&first, "start_export", json!({"jobId": id}));
    assert_eq!(settled(&first, &id)["state"], "failed");
    for app in [first, second] {
        app.stop_sync().unwrap();
    }
    first_project.close().unwrap();
    second_project.close().unwrap();
}

#[test]
fn dropping_an_application_on_an_async_thread_joins_its_preview() {
    let project = TestProject::new("export-drop");
    let app = fixture(&project);
    let id = create(&app, "linked-pages", "site", true, "classic");
    assert_eq!(settled(&app, &id)["state"], "ready");
    let url = run(&app, "start_export_preview", json!({"jobId": id}))["url"]
        .as_str()
        .unwrap()
        .to_owned();
    let artifact = app.export_artifact(&id, "index.html").unwrap().unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async move {
        drop(app);
    });
    let address = url.trim_start_matches("http://").trim_end_matches('/');
    assert!(TcpStream::connect(address).is_err());
    assert!(!PathBuf::from(artifact.path).exists());
    project.close().unwrap();
}

#[test]
fn interrupted_http_download_releases_its_artifact_before_application_stop_finishes() {
    let project = TestProject::new("export-interrupted-download");
    let app = fixture(&project);
    let id = create(&app, "linked-pages", "site", true, "classic");
    assert_eq!(settled(&app, &id)["state"], "ready");
    let artifact = app.export_artifact(&id, "index.html").unwrap().unwrap();
    let output = PathBuf::from(artifact.path);
    // A sparse artifact larger than socket buffers keeps the actual response
    // in flight until the client disconnects, without allocating a large file.
    std::fs::OpenOptions::new()
        .write(true)
        .open(&output)
        .unwrap()
        .set_len(64 * 1024 * 1024)
        .unwrap();
    let app = Arc::new(ApplicationHandle::from_application(app));
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .worker_threads(2)
        .build()
        .unwrap();
    runtime.block_on(async {
        let address = app.start_http(Some(0)).await.unwrap();
        let mut client = TcpStream::connect(address).unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        write!(
            client,
            "GET /api/export_artifact/{id}/index.html HTTP/1.1\r\nHost: localhost\r\n\r\n"
        )
        .unwrap();
        let mut headers = Vec::new();
        while !headers.ends_with(b"\r\n\r\n") {
            let mut byte = [0];
            client.read_exact(&mut byte).unwrap();
            headers.push(byte[0]);
            assert!(headers.len() < 8192);
        }
        assert!(
            String::from_utf8(headers)
                .unwrap()
                .starts_with("HTTP/1.1 200")
        );
        let stopping = Arc::clone(&app);
        let stop = tokio::spawn(async move { stopping.stop().await });
        let deadline = Instant::now() + Duration::from_secs(5);
        while !app.inner.stopping.load(Ordering::Acquire) {
            assert!(Instant::now() < deadline);
            tokio::task::yield_now().await;
        }
        assert!(!stop.is_finished());
        assert!(output.exists());
        client.shutdown(std::net::Shutdown::Both).unwrap();
        drop(client);
        tokio::time::timeout(Duration::from_secs(10), stop)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(!output.exists());
        assert!(TcpStream::connect(address).is_err());
        app.stop().await.unwrap();
    });
    project.close().unwrap();
}

#[cfg(unix)]
#[test]
#[expect(
    clippy::too_many_lines,
    reason = "Keep each end-to-end scenario together so setup, failure checks and cleanup remain visible."
)]
fn shutdown_stops_the_typst_child_and_cancels_queued_pdf_work() {
    use std::os::unix::fs::PermissionsExt;
    let project = TestProject::new("export-active-child");
    let mut app = fixture(&project);
    let resources = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
    let target = if cfg!(target_os = "macos") {
        "macos-arm64"
    } else {
        "linux-x64"
    };
    let typst = resources.join(format!("typst/assets/{target}/typst"));
    assert!(
        typst.is_file(),
        "Packaged Typst is required for this acceptance test"
    );
    app.options.pdf_font_folder = Some(
        resources
            .join("fonts/assets/pdf")
            .to_string_lossy()
            .into_owned(),
    );
    let control = tempfile::tempdir().unwrap();
    let started = control.path().join("started");
    let release = control.path().join("release");
    let finished = control.path().join("finished");
    let child_pid = control.path().join("pid");
    let wrapper = control.path().join("typst");
    let quote =
        |path: &std::path::Path| format!("'{}'", path.to_string_lossy().replace('\'', "'\\''"));
    // The wrapper provides a process barrier, then runs the actual packaged
    // engine. A deadline prevents a failed assertion leaving a child behind.
    std::fs::write(
        &wrapper,
        format!(
            r#"#!/bin/sh
if [ "$1" = '--version' ]; then exec {} "$@"; fi
for argument do output="$argument"; done
printf '%s\n' "$$" > {}
printf '%s\n' "$output" >> {}
attempt=0
while [ ! -f {} ]; do
  attempt=$((attempt + 1))
  if [ "$attempt" -ge 1000 ]; then exit 99; fi
  sleep 0.01
done
{} "$@"
result=$?
printf '%s\n' "$result" > {}
exit "$result"
"#,
            quote(&typst),
            quote(&child_pid),
            quote(&started),
            quote(&release),
            quote(&typst),
            quote(&finished)
        ),
    )
    .unwrap();
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
    app.options.typst_binary = Some(wrapper.to_string_lossy().into_owned());
    let app = Arc::new(app);
    let active = create(&app, "linked-pages", "pdf", true, "classic");
    let deadline = Instant::now() + Duration::from_secs(5);
    while !std::fs::read_to_string(&started).is_ok_and(|value| value.ends_with('\n')) {
        assert!(
            Instant::now() < deadline,
            "Typst did not reach the process barrier"
        );
        std::thread::yield_now();
    }
    let output = PathBuf::from(std::fs::read_to_string(&started).unwrap().trim());
    assert!(output.parent().unwrap().exists());
    assert_eq!(
        run(&app, "read_export", json!({"jobId": active}))["state"],
        "rendering"
    );
    let queued = create(&app, "linked-pages", "pdf", true, "classic");
    loop {
        let status = run(&app, "read_export", json!({"jobId": queued}));
        if status["state"] == "queued" {
            break;
        }
        assert_eq!(status["state"], "planning", "{status}");
        assert!(Instant::now() < deadline, "Second PDF did not queue");
        std::thread::yield_now();
    }
    let pid = std::fs::read_to_string(&child_pid).unwrap();
    let stopping = Instant::now();
    app.stop_sync().unwrap();
    assert!(
        stopping.elapsed() < Duration::from_secs(5),
        "Shutdown waited for the blocked compiler instead of cancelling it"
    );
    assert!(
        !std::process::Command::new("/bin/kill")
            .args(["-0", pid.trim()])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap()
            .success(),
        "Typst child was not reaped before shutdown returned"
    );
    assert!(!finished.exists(), "Cancelled compiler finished rendering");
    assert_eq!(
        std::fs::read_to_string(&started).unwrap().lines().count(),
        1,
        "Cancelled queued PDF started a child"
    );
    assert!(!output.parent().unwrap().exists());
    assert!(app.export_artifact(&active, "").is_err());
    project.close().unwrap();
}

#[test]
fn linked_pages_preserve_text_only_policy_and_fail_on_unresolved_content() {
    let project = TestProject::new("export-linked-policy");
    let app = fixture(&project);
    let strict = run(
        &app,
        "create_export",
        json!({"source": {"kind": "note", "id": PAGE}, "format": "html", "start": false,
        "themeId": "classic", "outsideLinks": "error", "profile": "default"}),
    )["jobId"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(settled(&app, &strict)["state"], "failed");
    let linked = create(&app, "linked-pages", "pdf", false, "classic");
    let status = settled(&app, &linked);
    assert_eq!(status["state"], "prepared", "{status}");
    assert_eq!(status["summary"]["pages"], 2);
    assert_eq!(
        status["plan"]["value"]["targets"]["pdf"]["outsideLinks"],
        "text-only"
    );
    let missing = "00000000-0000-7000-8000-000000000699";
    let page = run(&app, "read_item", json!({"id": PAGE}));
    run(
        &app,
        "update_item",
        json!({"id": PAGE, "version": page["version"], "meta": page["meta"], "content": {"document": {"type": "doc", "content": [
            {"type": "paragraph", "content": [{"type": "text", "text": "Unavailable", "marks": [{"type": "link", "attrs": {"href": format!("/?note={missing}")}}]}]}
        ]}}}),
    );
    let strict = run(
        &app,
        "create_export",
        json!({"source": source_json("linked-pages"), "format": "site", "start": false,
        "themeId": "classic", "outsideLinks": "error", "profile": "default"}),
    )["jobId"]
        .as_str()
        .unwrap()
        .to_owned();
    // The strict policy lists the missing page as an issue; the plan does not generate.
    let status = settled(&app, &strict);
    assert_eq!(status["state"], "prepared", "{status}");
    let issue = &status["plan"]["value"]["issues"][0];
    assert_eq!(issue["pageId"], PAGE);
    assert!(
        issue["message"].as_str().unwrap().contains("missing"),
        "{issue}"
    );
    let text_only = create(&app, "linked-pages", "site", false, "classic");
    assert_eq!(settled(&app, &text_only)["state"], "prepared");
    app.stop_sync().unwrap();
    project.close().unwrap();
}

#[test]
fn page_render_pins_retained_source_until_cancel_and_recovers_abandoned_leases() {
    let project = TestProject::new("export-snapshot-pins");
    let app = fixture(&project);
    let id = create(&app, "note", "html", false, "classic");
    assert_eq!(settled(&app, &id)["state"], "prepared");
    let store = app.database().unwrap();
    let version = run(&app, "read_item", json!({"id": PAGE}))["version"].clone();
    let pins = || {
        store
            .read(|tx| {
                Ok(tx.query_row::<i64, _, _>(
                    "SELECT count(*) FROM revision_pin p JOIN entity e ON e.id=p.item_id
         WHERE p.owner_kind='render' AND p.owner_id=?1 AND e.uuid=?2 AND p.revision_uuid=?3",
                    rusqlite::params![id, PAGE, version.as_str().unwrap()],
                    |row| row.get(0),
                )?)
            })
            .unwrap()
    };
    assert_eq!(pins(), 1);
    run(
        &app,
        "update_item_meta",
        json!({"id":PAGE, "version":version, "meta":{"title":"New heading"}}),
    );
    assert_eq!(pins(), 1);
    run(&app, "cancel_export", json!({"jobId":id}));
    assert_eq!(pins(), 0);
    store
        .write(|tx| {
            tx.execute(
                "INSERT INTO render_lease(id,expires_ms) VALUES('abandoned',0)",
                [],
            )?;
            Ok(())
        })
        .unwrap();
    crate::store::render_pins::maintain(&store, 1).unwrap();
    assert!(
        !store
            .read(|tx| Ok(tx.query_row::<bool, _, _>(
                "SELECT EXISTS(SELECT 1 FROM render_lease WHERE id='abandoned')",
                [],
                |row| row.get(0)
            )?))
            .unwrap()
    );
    app.stop_sync().unwrap();
}

#[test]
fn unrelated_save_during_render_planning_preserves_the_plan() {
    use crate::domain::publication::{
        plan::PublicationFormat,
        {OutsideLinks, PdfProfile},
    };
    use crate::export_jobs::model::Plan;
    use crate::store::render_pins::Guard;

    let project = TestProject::new("export-unrelated-save");
    let app = fixture(&project);
    let page = crate::export_jobs::page::plan_page(
        &app.root,
        PAGE,
        "classic",
        OutsideLinks::TextOnly,
        PublicationFormat::Html,
        PdfProfile::Default,
    )
    .unwrap();
    let revision = page.page_revision.clone();
    let plan = Plan::Page(Box::new(page));
    let version = run(&app, "read_item", json!({"id":TARGET}))["version"].clone();
    run(
        &app,
        "update_item_meta",
        json!({"id":TARGET,"version":version,"meta":{"title":"Separate edit"}}),
    );
    let store = app.database().unwrap();
    let owner = uuid::Uuid::now_v7().to_string();
    drop(Guard::hold(Arc::clone(&store), &owner, &plan).unwrap());
    run(
        &app,
        "update_item_meta",
        json!({"id":PAGE,"version":revision,"meta":{"title":"Changed export source"}}),
    );
    assert!(matches!(
        Guard::hold(Arc::clone(&store), &owner, &plan),
        Err(crate::store::Error::RevisionConflict)
    ));
    assert_eq!(
        store
            .read(|tx| Ok(tx.query_row::<i64, _, _>(
                "SELECT count(*) FROM render_lease WHERE id=?1",
                [&owner],
                |row| row.get(0)
            )?))
            .unwrap(),
        0
    );
    app.stop_sync().unwrap();
}
