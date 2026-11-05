// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! The `bluink` executable driven from outside, as a user or supervisor would:
//! arguments and output, plain-mode lifecycle, startup failures and the
//! one-shot commands. Cargo builds the executable under test.

#![allow(
    unused_crate_dependencies,
    reason = "Integration tests use only the dependencies they exercise."
)]
#![allow(
    clippy::unwrap_used,
    clippy::panic,
    reason = "Test helpers fail loudly, like the tests that call them."
)]

use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Output, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

const EXECUTABLE: &str = env!("CARGO_BIN_EXE_bluink");

fn home(directory: &Path) -> PathBuf {
    directory.join(".test-user-home")
}

/// The caller's environment without colour, with user state inside the test.
fn prepare(command: &mut Command, home: &Path) {
    command
        .env_remove("FORCE_COLOR")
        .env("NO_COLOR", "1")
        .env("HOME", home)
        .env("USERPROFILE", home)
        .env("BLUINK_LOG_LEVEL", "error");
}

struct Run {
    code: i32,
    stdout: String,
    stderr: String,
}

fn run(cwd: &Path, home: &Path, args: &[&str]) -> Run {
    let mut command = Command::new(EXECUTABLE);
    prepare(&mut command, home);
    let Output {
        status,
        stdout,
        stderr,
    } = command
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    Run {
        code: status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&stdout).into_owned(),
        stderr: String::from_utf8_lossy(&stderr).into_owned(),
    }
}

fn succeed(cwd: &Path, home: &Path, args: &[&str]) -> String {
    let result = run(cwd, home, args);
    assert_eq!(result.code, 0, "{args:?}: {}", result.stderr);
    result.stdout
}

fn json(cwd: &Path, home: &Path, args: &[&str]) -> Value {
    serde_json::from_str(&succeed(cwd, home, args)).unwrap()
}

/// A running executable whose output is collected; plain servers stop
/// gracefully on a `stop` line.
struct Server {
    child: Child,
    stdin: Option<ChildStdin>,
    output: Arc<Mutex<String>>,
}

impl Server {
    fn launch(cwd: &Path, home: &Path, args: &[&str], environment: &[(&str, &Path)]) -> Self {
        let mut command = Command::new(EXECUTABLE);
        prepare(&mut command, home);
        command.env("BLUINK_LOG_LEVEL", "debug");
        for (name, value) in environment {
            command.env(name, value);
        }
        let mut child = command
            .args(args)
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let output = Arc::new(Mutex::new(String::new()));
        let collect = |stream: Box<dyn Read + Send>| {
            let output = Arc::clone(&output);
            std::thread::spawn(move || {
                for line in BufReader::new(stream).lines().map_while(Result::ok) {
                    let mut output = output.lock().unwrap();
                    output.push_str(&line);
                    output.push('\n');
                }
            });
        };
        collect(Box::new(child.stdout.take().unwrap()));
        collect(Box::new(child.stderr.take().unwrap()));
        Self {
            stdin: child.stdin.take(),
            child,
            output,
        }
    }

    fn output(&self) -> String {
        self.output.lock().unwrap().clone()
    }

    fn ready(&mut self) -> String {
        let pattern = regex::Regex::new(r"Server started at (http://127\.0\.0\.1:\d+)").unwrap();
        let deadline = Instant::now() + Duration::from_secs(30);
        while Instant::now() < deadline {
            if let Some(found) = pattern.captures(&self.output()) {
                return found[1].to_owned();
            }
            assert!(
                self.child.try_wait().unwrap().is_none(),
                "Bluink exited before listening: {}",
                self.output()
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("Bluink did not start: {}", self.output());
    }

    fn stop(&mut self, times: usize) {
        let stdin = self.stdin.as_mut().unwrap();
        for _ in 0..times {
            stdin.write_all(b"stop\n").unwrap();
        }
        stdin.flush().unwrap();
    }

    fn exit_code(&mut self) -> i32 {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                // Let the readers drain the final lines.
                std::thread::sleep(Duration::from_millis(50));
                return status.code().unwrap_or(-1);
            }
            assert!(
                Instant::now() < deadline,
                "Bluink did not exit: {}",
                self.output()
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

fn plain_server(project: &Path, home: &Path) -> Server {
    // The server creates a missing project, so start beside it rather than in it.
    Server::launch(
        project.parent().unwrap(),
        home,
        &[
            "server",
            "--no-tui",
            "--port",
            "0",
            "--project",
            project.to_str().unwrap(),
        ],
        &[],
    )
}

/// Run `operation` against a plain server for `project`, then stop it cleanly.
fn with_server<T>(project: &Path, home: &Path, operation: impl FnOnce(&str) -> T) -> T {
    let mut server = plain_server(project, home);
    let result = operation(&server.ready());
    server.stop(1);
    assert_eq!(server.exit_code(), 0, "{}", server.output());
    result
}

fn rpc(url: &str, operation: &str, body: &Value) -> Value {
    let response = reqwest::blocking::Client::new()
        .post(format!("{url}/api/{operation}"))
        .body(body.to_string())
        .send()
        .unwrap();
    let status = response.status();
    let value: Value = response.json().unwrap();
    assert_eq!(status, 200, "{operation}: {value}");
    value
}

fn page(title: &str, text: &str) -> Value {
    let paragraph = if text.is_empty() {
        json!({"type": "paragraph"})
    } else {
        json!({"type": "paragraph", "content": [{"type": "text", "text": text}]})
    };
    json!({
        "meta": {"type": "page", "title": title},
        "content": {"document": {"type": "doc", "content": [paragraph]}},
    })
}

/// Seed pages through Rust validation over the real HTTP API.
fn seed(project: &Path, home: &Path, pages: &[(&str, &str)]) -> Vec<String> {
    with_server(project, home, |url| {
        pages
            .iter()
            .map(|(title, text)| {
                rpc(url, "create_item", &page(title, text))["id"]
                    .as_str()
                    .unwrap()
                    .to_owned()
            })
            .collect()
    })
}

fn path(value: &Path) -> &str {
    value.to_str().unwrap()
}

/// A project that reopens cleanly was released and left consistent.
fn reopens(project: &Path, home: &Path) {
    succeed(project, home, &["history", "--project", path(project)]);
}

#[test]
fn help_version_and_argument_errors_need_no_project_or_resources() {
    let directory = tempfile::tempdir().unwrap();
    let (root, home) = (directory.path(), &home(directory.path()));
    for name in ["server", "export", "publication"] {
        let help = run(root, home, &[name, "--help"]);
        assert_eq!(help.code, 0, "{}", help.stderr);
        assert!(help.stdout.contains(&format!("Usage: bluink {name}")));
        assert_eq!(help.stderr, "");
        assert!(!help.stdout.contains('\u{1b}'));
    }
    let version = run(root, home, &["--version"]);
    assert!(
        regex::Regex::new(r"^@bluink/cli v\S+\r?\n$")
            .unwrap()
            .is_match(&version.stdout)
    );
    let identity: Value = serde_json::from_str(&succeed(root, home, &["build-info"])).unwrap();
    assert!(identity["sourceDigest"].is_string());
    let unknown = run(root, home, &["unknown-command"]);
    assert_eq!(unknown.code, 1);
    assert!(
        unknown
            .stderr
            .contains("Unknown command: 'unknown-command'")
    );
    assert!(unknown.stdout.contains("Usage:"));
    let option = run(root, home, &["server", "--bogus"]);
    assert_eq!(option.code, 1);
    assert!(option.stderr.contains("Unknown option '--bogus'"));
}

#[test]
fn plain_mode_reports_readiness_and_serves_the_saved_page() {
    let directory = tempfile::tempdir().unwrap();
    let (root, home) = (directory.path(), &home(directory.path()));
    let project = root.join("project");
    let ids = seed(&project, home, &[("Progress fixture", "")]);
    let mut server = plain_server(&project, home);
    let url = server.ready();
    assert!(!server.output().contains('\u{1b}'));
    let item = rpc(&url, "read_item", &json!({"id": ids[0]}));
    assert_eq!(item["meta"]["title"], "Progress fixture");
    server.stop(1);
    assert_eq!(server.exit_code(), 0, "{}", server.output());
    assert!(
        server
            .output()
            .contains("Bluink has stopped. You can close this window.")
    );
}

#[test]
fn stop_requests_coalesce_and_diagnostics_flush_before_release() {
    let directory = tempfile::tempdir().unwrap();
    let (root, home) = (directory.path(), &home(directory.path()));
    let diagnostics = root.join("diagnostics.jsonl");
    let mut server = Server::launch(
        root,
        home,
        &["server", "--port", "0"],
        &[("BLUINK_DIAGNOSTICS_FILE", &diagnostics)],
    );
    let url = server.ready();
    let response = reqwest::blocking::get(format!("{url}/api/read_storage_status")).unwrap();
    assert_eq!(response.status(), 200);
    server.stop(2);
    assert_eq!(server.exit_code(), 0, "{}", server.output());
    assert_eq!(server.output().matches("shutdown_requested").count(), 1);
    let captured = std::fs::read_to_string(&diagnostics).unwrap();
    assert!(captured.contains("read_storage_status"));
    assert!(captured.contains("shutdown_requested"));
    assert!(
        captured
            .lines()
            .any(|line| serde_json::from_str::<Value>(line).unwrap()["name"] == "application.stop"),
        "Final application cleanup must reach capture before exit"
    );
    reopens(root, home);
}

#[cfg(unix)]
#[test]
fn signals_stop_the_server_gracefully() {
    let directory = tempfile::tempdir().unwrap();
    let (root, home) = (directory.path(), &home(directory.path()));
    let mut server = plain_server(&root.join("project"), home);
    server.ready();
    for signal in ["TERM", "INT"] {
        Command::new("kill")
            .args([&format!("-{signal}"), &server.child.id().to_string()])
            .status()
            .unwrap();
    }
    assert_eq!(server.exit_code(), 0, "{}", server.output());
    reopens(&root.join("project"), home);
}

#[test]
fn a_stop_during_startup_drains_the_project_once() {
    let directory = tempfile::tempdir().unwrap();
    let (root, home) = (directory.path(), &home(directory.path()));
    let project = root.join("project");
    let mut server = plain_server(&project, home);
    server.stop(1);
    assert_eq!(server.exit_code(), 0, "{}", server.output());
    // The stop may land before the project exists; a later start must work.
    with_server(&project, home, |_| {});
}

#[test]
fn a_second_launch_reuses_the_running_bluink_for_the_same_project() {
    let directory = tempfile::tempdir().unwrap();
    let (root, home) = (directory.path(), &home(directory.path()));
    let project = root.join("project");
    let mut running = plain_server(&project, home);
    let url = running.ready();
    let port = url.rsplit(':').next().unwrap();
    let again = run(
        root,
        home,
        &[
            "server",
            "--no-open",
            "--port",
            port,
            "--project",
            path(&project),
        ],
    );
    assert_eq!(again.code, 0, "{}", again.stderr);
    assert!(
        again
            .stdout
            .contains("Bluink is already running for this project")
    );
    // A different project on the same port is a real conflict.
    let other = run(
        root,
        home,
        &[
            "server",
            "--no-open",
            "--port",
            port,
            "--project",
            path(&root.join("other")),
        ],
    );
    assert_eq!(other.code, 1);
    assert!(
        other
            .stderr
            .contains(&format!("Port {port} is already in use"))
    );
    running.stop(1);
    assert_eq!(running.exit_code(), 0, "{}", running.output());
}

#[test]
fn startup_failures_release_the_project_and_explain_a_taken_port() {
    let directory = tempfile::tempdir().unwrap();
    let (root, home) = (directory.path(), &home(directory.path()));
    let blocker = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = blocker.local_addr().unwrap().port().to_string();
    let collision = run(root, home, &["server", "--no-tui", "--port", &port]);
    assert_eq!(collision.code, 1);
    assert!(
        collision
            .stderr
            .contains(&format!("Port {port} is already in use"))
    );
    assert!(
        collision
            .stderr
            .contains("Or choose another port with --port.")
    );
    reopens(root, home);
    drop(blocker);
    let file = root.join("not-a-folder");
    std::fs::write(&file, "not a project directory").unwrap();
    let invalid = run(
        root,
        home,
        &[
            "server",
            "--no-tui",
            "--port",
            "0",
            "--project",
            path(&file),
        ],
    );
    assert_eq!(invalid.code, 1);
    assert!(!format!("{}{}", invalid.stdout, invalid.stderr).contains("Server started"));
    assert_ne!(invalid.stderr, "");
}

fn fixture(project: &Path, home: &Path) -> (String, String) {
    let ids = seed(
        project,
        home,
        &[
            ("Home 文学", "Home 文学 body <safe> & complete."),
            ("Chapter", "Chapter body <safe> & complete."),
        ],
    );
    (ids[0].clone(), ids[1].clone())
}

#[test]
fn rebuild_projections_publishes_searchable_results_to_an_open_server() {
    let directory = tempfile::tempdir().unwrap();
    let (root, home) = (directory.path(), &home(directory.path()));
    let project = root.join("project");
    let (_, chapter) = fixture(&project, home);
    let mut server = plain_server(&project, home);
    let url = server.ready();
    let saved = rpc(&url, "read_item", &json!({"id": chapter}));
    let cursor = rpc(&url, "read_changes", &json!({}))["cursor"].clone();
    // The project defaults to the current directory, as for backup and export.
    let output = succeed(&project, home, &["rebuild-projections"]);
    let changes = rpc(&url, "read_changes", &json!({"cursor": cursor}));
    assert_eq!(
        output.trim(),
        format!(
            "Rebuilt query projections at generation {}.",
            changes["cursor"]["generation"].as_str().unwrap()
        )
    );
    assert!(
        changes["batches"]
            .as_array()
            .unwrap()
            .iter()
            .any(|batch| batch["reset"] == true)
    );
    assert_eq!(rpc(&url, "read_item", &json!({"id": chapter})), saved);
    let matches = rpc(
        &url,
        "query_items",
        &json!({"query": {"version": 1, "sort": [], "clauses": [{
            "polarity": "required",
            "predicate": {"kind": "text", "value": "chapter", "mode": "terms"},
        }]}}),
    );
    assert!(
        matches["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["id"] == chapter)
    );
    server.stop(1);
    assert_eq!(server.exit_code(), 0, "{}", server.output());
}

#[test]
fn history_restore_and_sync_share_reviewed_local_project_operations() {
    let directory = tempfile::tempdir().unwrap();
    let (root, home) = (directory.path(), &home(directory.path()));
    let project = root.join("project");
    fixture(&project, home);
    let history = json(
        root,
        home,
        &["history", "--project", path(&project), "--limit", "1"],
    );
    assert_eq!(history["entries"].as_array().unwrap().len(), 1);
    let checkpoint = history["entries"][0]["id"].as_str().unwrap().to_owned();
    let restore = [
        "restore",
        "--project",
        path(&project),
        "--checkpoint",
        &checkpoint,
    ];
    let extra = seed(
        &project,
        home,
        &[("Later writing", "Preserved by a checkpoint.")],
    );
    let preview = json(root, home, &restore);
    assert!(
        preview["changedItems"]
            .as_array()
            .unwrap()
            .iter()
            .any(|change| change["id"] == extra[0])
    );
    seed(
        &project,
        home,
        &[("Newer than the preview", "Preserved by a checkpoint.")],
    );
    let token = preview["reviewToken"].as_str().unwrap();
    let stale = run(
        root,
        home,
        &[&restore[..], &["--confirm", "--review-token", token]].concat(),
    );
    assert_ne!(stale.code, 0);
    assert!(stale.stderr.to_lowercase().contains("preview changed"));
    let fresh = json(root, home, &restore);
    let token = fresh["reviewToken"].as_str().unwrap();
    json(
        root,
        home,
        &[&restore[..], &["--confirm", "--review-token", token]].concat(),
    );
    let origin = root.join("writing.git");
    let sync = ["sync", "--project", path(&project)];
    assert!(json(root, home, &[&sync[..], &["--status"]].concat())["origin"].is_null());
    json(
        root,
        home,
        &[&sync[..], &["--origin", path(&origin), "--create"]].concat(),
    );
    assert_eq!(json(root, home, &sync)["state"], "synced");
    let clone = root.join("clone");
    let cloned = json(
        root,
        home,
        &[
            "sync",
            "--open-from",
            path(&origin),
            "--destination",
            path(&clone),
        ],
    );
    assert_eq!(cloned["path"], path(&clone));
    assert_ne!(
        json(root, home, &["history", "--project", path(&clone)])["entries"],
        json!([])
    );
    assert!(json(root, home, &[&sync[..], &["--disconnect"]].concat())["origin"].is_null());
}

#[test]
fn export_forwards_flags_resolves_packaged_pdf_resources_and_separates_errors() {
    let directory = tempfile::tempdir().unwrap();
    let (root, home) = (directory.path(), &home(directory.path()));
    let project = root.join("project");
    let (page, _) = fixture(&project, home);
    let checked = run(
        root,
        home,
        &[
            "export",
            "html",
            "--project",
            path(&project),
            "--note",
            &page,
            "--check",
        ],
    );
    assert_eq!(checked.code, 0, "{}", checked.stderr);
    assert_eq!(
        serde_json::from_str::<Value>(&checked.stdout).unwrap()["pageId"],
        page.as_str()
    );
    assert_eq!(checked.stderr, "");
    let generated = succeed(
        root,
        home,
        &[
            "export",
            "pdf",
            "--project",
            path(&project),
            "--note",
            &page,
            "--out",
            "./note.pdf",
        ],
    );
    assert_eq!(
        Path::new(generated.trim()).canonicalize().unwrap(),
        root.join("note.pdf").canonicalize().unwrap()
    );
    assert!(
        std::fs::read(root.join("note.pdf"))
            .unwrap()
            .starts_with(b"%PDF-")
    );
    let invalid = run(root, home, &["export", "html", "--unknown-option"]);
    assert_eq!(invalid.code, 1);
    assert_eq!(invalid.stdout, "");
    assert!(invalid.stderr.contains("Unknown option"));
}

#[test]
fn publication_create_inspect_plan_and_generate_return_json_results() {
    let directory = tempfile::tempdir().unwrap();
    let (root, home) = (directory.path(), &home(directory.path()));
    let project = root.join("project");
    let (page, chapter) = fixture(&project, home);
    std::fs::write(
        root.join("book.json"),
        json!({
            "title": "CLI book",
            "content": {
                "kind": "publication",
                "selectedOutput": "site",
                "rootPageId": page,
                "outline": [{"kind": "page", "pageId": chapter, "children": []}],
                "selectionRules": [],
                "themeId": "classic",
                "targets": {
                    "pdf": {"profile": "default", "outsideLinks": "error"},
                    "html": {"outsideLinks": "error"},
                    "docx": {"outsideLinks": "error"},
                    "epub": {"outsideLinks": "error"},
                    "site": {"layout": "documentation", "outsideLinks": "error"},
                },
            },
        })
        .to_string(),
    )
    .unwrap();
    let project = path(&project);
    let created = succeed(
        root,
        home,
        &[
            "publication",
            "create",
            "--project",
            project,
            "--input",
            "./book.json",
        ],
    );
    let id = created.trim();
    assert!(regex::Regex::new(r"^[0-9a-f-]{36}$").unwrap().is_match(id));
    let inspected = json(
        root,
        home,
        &["publication", "inspect", "--project", project, "--id", id],
    );
    assert_eq!(inspected["meta"]["title"], "CLI book");
    let planned = json(
        root,
        home,
        &[
            "publication",
            "plan",
            "--project",
            project,
            "--id",
            id,
            "--format",
            "site",
        ],
    );
    assert_eq!(planned["pages"].as_array().unwrap().len(), 2);
    let generated = json(
        root,
        home,
        &[
            "publication",
            "generate",
            "--project",
            project,
            "--id",
            id,
            "--format",
            "site",
            "--output",
            "./site",
        ],
    );
    assert_eq!(
        Path::new(generated["output"].as_str().unwrap())
            .canonicalize()
            .unwrap(),
        root.join("site").canonicalize().unwrap()
    );
    assert_ne!(std::fs::read(root.join("site/index.html")).unwrap(), b"");
}

#[test]
fn portable_export_import_and_full_backup_restore_preserve_uncheckpointed_saves() {
    let directory = tempfile::tempdir().unwrap();
    let (root, home) = (directory.path(), &home(directory.path()));
    let source = root.join("source");
    fixture(&source, home);
    let (portable, backup) = (root.join("portable"), root.join("backup"));
    let (imported, restored) = (root.join("imported"), root.join("restored"));
    // Export and back up while the writer is open, before any checkpoint.
    let id = with_server(&source, home, |url| {
        let page = rpc(url, "create_item", &page("Uncheckpointed page", ""));
        let exported = json(
            root,
            home,
            &[
                "export-project",
                "--project",
                path(&source),
                "--destination",
                path(&portable),
            ],
        );
        assert!(exported["items"].as_u64().unwrap() >= 3);
        assert!(exported["snapshotBytes"].as_u64().unwrap() > 0);
        let backed_up = json(
            root,
            home,
            &[
                "backup",
                "--project",
                path(&source),
                "--destination",
                path(&backup),
            ],
        );
        assert!(backed_up["snapshot"]["projectId"].is_string());
        page["id"].as_str().unwrap().to_owned()
    });
    let mut entries: Vec<_> = std::fs::read_dir(&portable)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    entries.sort();
    assert_eq!(entries, ["bluink.json", "items"]);
    succeed(
        root,
        home,
        &[
            "import",
            "--source",
            path(&portable),
            "--destination",
            path(&imported),
        ],
    );
    succeed(
        root,
        home,
        &[
            "restore-backup",
            "--source",
            path(&backup),
            "--destination",
            path(&restored),
        ],
    );
    for project in [&imported, &restored] {
        let title = with_server(project, home, |url| {
            rpc(url, "read_item", &json!({"id": id}))["meta"]["title"].clone()
        });
        assert_eq!(title, "Uncheckpointed page");
    }
    let duplicate = run(
        root,
        home,
        &[
            "import",
            "--source",
            path(&portable),
            "--destination",
            path(&imported),
        ],
    );
    assert_ne!(duplicate.code, 0);
}
