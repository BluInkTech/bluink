// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! Exercise the actual renderer, input reader and restoration in an isolated PTY.
use std::io::{Read, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use portable_pty::{CommandBuilder, MasterPty, PtySize, native_pty_system};

use super::{BluinkConsole, ConsoleOptions};
use crate::application::{ApplicationOptions, BluinkApplication};

struct Session {
    master: Box<dyn MasterPty + Send>,
    child: Box<dyn portable_pty::Child + Send + Sync>,
    writer: Box<dyn Write + Send>,
    incoming: Receiver<Vec<u8>>,
    parser: vt100::Parser,
    transcript: Vec<u8>,
    answer_cursor_queries: bool,
    project: tempfile::TempDir,
}

impl Session {
    fn new(columns: u16, rows: u16, plain: bool, no_color: bool) -> Self {
        Self::launch(columns, rows, plain, no_color, None, None)
    }

    fn launch(
        columns: u16,
        rows: u16,
        plain: bool,
        no_color: bool,
        node: Option<&std::path::Path>,
        test_name: Option<&str>,
    ) -> Self {
        let project = tempfile::tempdir().unwrap();
        let pair = native_pty_system()
            .openpty(PtySize {
                rows,
                cols: columns,
                pixel_width: 0,
                pixel_height: 0,
            })
            .unwrap();
        let mut command = node.map_or_else(
            || {
                let mut command = CommandBuilder::new(std::env::current_exe().unwrap());
                command.args([
                    "--ignored",
                    "--exact",
                    test_name.unwrap_or("console::pty::terminal_child"),
                    "--nocapture",
                ]);
                command
            },
            |node| {
                use std::os::unix::fs::PermissionsExt;
                let bin = project.path().join("bin");
                std::fs::create_dir(&bin).unwrap();
                for name in ["xdg-open", "open"] {
                    let script = bin.join(name);
                    std::fs::write(&script, "#!/bin/sh\nprintf '%s\\n' \"$1\" > \"$BLUINK_BROWSER_TEST_OUTPUT\"\nexit 1\n").unwrap();
                    std::fs::set_permissions(script, std::fs::Permissions::from_mode(0o755)).unwrap();
                }
                let mut command = CommandBuilder::new(node);
                if let Some(preload) = std::env::var_os("BLUINK_CONSOLE_PRELOAD") {
                    command.arg("--import");
                    command.arg(preload);
                }
                command.arg(std::env::var_os("BLUINK_CONSOLE_CLI").map_or_else(
                    || std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("bluink"),
                    std::path::PathBuf::from,
                ));
                command.args(["server", "--port", "0", "--project"]);
                command.arg(project.path());
                let path = std::env::join_paths(std::iter::once(bin).chain(std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()))).unwrap();
                command.env("PATH", path);
                command.env("BLUINK_BROWSER_TEST_OUTPUT", project.path().join("browser-url.txt"));
                command
            },
        );
        command.env("BLUINK_CONSOLE_TEST_PROJECT", project.path());
        command.env(
            "BLUINK_CONSOLE_TEST_MODE",
            if plain { "plain" } else { "auto" },
        );
        command.env(
            "BLUINK_DIAGNOSTICS_FILE",
            project.path().join("diagnostics.jsonl"),
        );
        command.env("BLUINK_CACHE_LOCATION", "project");
        command.env(
            "BLUINK_LOG_LEVEL",
            if node.is_some() { "debug" } else { "info" },
        );
        command.env("CI", "0");
        command.env("TERM", "xterm-256color");
        if no_color {
            command.env("NO_COLOR", "1");
        } else {
            command.env_remove("NO_COLOR");
        }
        let child = pair.slave.spawn_command(command).unwrap();
        drop(pair.slave);
        let mut reader = pair.master.try_clone_reader().unwrap();
        let writer = pair.master.take_writer().unwrap();
        let (sender, incoming) = mpsc::channel();
        std::thread::spawn(move || {
            let mut buffer = [0; 4096];
            while let Ok(count) = reader.read(&mut buffer) {
                if count == 0 || sender.send(buffer[..count].to_vec()).is_err() {
                    break;
                }
            }
        });
        Self {
            master: pair.master,
            child,
            writer,
            incoming,
            parser: vt100::Parser::new(rows, columns, 2000),
            transcript: Vec::new(),
            answer_cursor_queries: true,
            project,
        }
    }

    fn pump(&mut self, timeout: Duration) -> bool {
        if let Ok(chunk) = self.incoming.recv_timeout(timeout) {
            for byte in chunk {
                self.parser.process(&[byte]);
                self.transcript.push(byte);
                // A real emulator answers cursor-position reports requested by
                // Crossterm. Respond even when a query crosses read boundaries.
                if self.answer_cursor_queries && self.transcript.ends_with(b"\x1b[6n") {
                    let (row, column) = self.parser.screen().cursor_position();
                    write!(self.writer, "\x1b[{};{}R", row + 1, column + 1).unwrap();
                    self.writer.flush().unwrap();
                }
            }
            true
        } else {
            false
        }
    }

    fn settle(&mut self) {
        // PTY writes can split in the middle of a draw. Continue servicing
        // cursor queries until the frame is quiet before inspecting/capturing.
        let deadline = Instant::now() + Duration::from_millis(300);
        while self.pump(Duration::from_millis(25)) && Instant::now() < deadline {}
    }

    fn wait_for(&mut self, expected: &str) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            self.pump(Duration::from_millis(50));
            if self.parser.screen().contents().contains(expected) {
                self.settle();
                if self.parser.screen().contents().contains(expected) {
                    return;
                }
            }
            if self.child.try_wait().unwrap().is_some() {
                break;
            }
        }
        panic!(
            "Missing {expected:?}; screen:\n{}\ntranscript tail:\n{}",
            self.parser.screen().contents(),
            String::from_utf8_lossy(
                &self.transcript[self.transcript.len().saturating_sub(4_000)..]
            )
        );
    }

    fn send(&mut self, keys: &[u8]) {
        self.writer.write_all(keys).unwrap();
        self.writer.flush().unwrap();
    }

    fn resize(&mut self, columns: u16, rows: u16) {
        self.parser.screen_mut().set_size(rows, columns);
        self.master
            .resize(PtySize {
                rows,
                cols: columns,
                pixel_width: 0,
                pixel_height: 0,
            })
            .unwrap();
    }

    fn capture(&mut self, name: &str) {
        self.settle();
        let Some(directory) = std::env::var_os("BLUINK_CONSOLE_CAPTURE_DIR") else {
            return;
        };
        let directory = std::path::PathBuf::from(directory);
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(
            directory.join(format!("{name}.txt")),
            self.parser.screen().contents(),
        )
        .unwrap();
        std::fs::write(directory.join(format!("{name}.ansi")), &self.transcript).unwrap();
        let (rows, columns) = self.parser.screen().size();
        let cells: Vec<_> = (0..rows).map(|row| (0..columns).map(|column| {
            let cell = self.parser.screen().cell(row, column).unwrap();
            serde_json::json!({"text":cell.contents(), "foreground":format!("{:?}", cell.fgcolor()), "background":format!("{:?}", cell.bgcolor()), "bold":cell.bold()})
        }).collect::<Vec<_>>()).collect();
        std::fs::write(
            directory.join(format!("{name}.json")),
            serde_json::to_vec_pretty(
                &serde_json::json!({"rows":rows,"columns":columns,"cells":cells}),
            )
            .unwrap(),
        )
        .unwrap();
    }

    fn quit(&mut self, keys: &[u8]) {
        self.finish(keys, "TERMINAL_SESSION_CLOSED");
    }

    fn finish(&mut self, keys: &[u8], marker: &str) {
        self.finish_outcome(keys, marker, true);
    }

    fn finish_outcome(&mut self, keys: &[u8], marker: &str, success: bool) {
        self.send(keys);
        self.wait_for(marker);
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                assert_eq!(status.success(), success);
                break;
            }
            assert!(Instant::now() < deadline, "Console child failed to exit");
            self.pump(Duration::from_millis(25));
        }
        self.settle();
        assert!(!self.parser.screen().hide_cursor(), "Cursor not restored");
        let termios = self.master.get_termios().unwrap();
        let flags = format!("{:?}", termios.local_flags);
        assert!(
            flags.contains("ICANON") && flags.contains("ECHO"),
            "Terminal not restored: {flags}"
        );
    }
}

#[test]
#[ignore = "requires BLUINK_CONSOLE_NODE, BLUINK_CONSOLE_PRELOAD and compiled CLI"]
fn compiled_cli_startup_failure_restores_the_terminal() {
    assert!(std::env::var_os("BLUINK_CONSOLE_PRELOAD").is_some());
    let node = std::env::var_os("BLUINK_CONSOLE_NODE").unwrap();
    let mut session = Session::launch(80, 24, false, true, Some(std::path::Path::new(&node)), None);
    session.wait_for("Starting");
    session.finish_outcome(b"", "Controlled startup failure", false);
    session.capture("compiled-cli-80x24-startup-failure");
}

/// Requires a freshly built addon and compiled Node CLI; invoked explicitly.
#[test]
#[ignore = "requires BLUINK_CONSOLE_NODE and the compiled CLI/native addon"]
fn compiled_cli_console_uses_host_rss_and_orderly_shutdown() {
    let node = std::env::var_os("BLUINK_CONSOLE_NODE")
        .expect("Set BLUINK_CONSOLE_NODE to the Node executable");
    let mut session = Session::launch(
        120,
        30,
        false,
        false,
        Some(std::path::Path::new(&node)),
        None,
    );
    session.wait_for("Running");
    session.wait_for("Memory ");
    assert!(!session.parser.screen().contents().contains("Memory —"));
    session.send(b"o");
    session.wait_for("Could not open the browser");
    session.wait_for("Errors 1");
    let launched = std::fs::read_to_string(session.project.path().join("browser-url.txt")).unwrap();
    assert!(launched.starts_with("http://127.0.0.1:"));
    session.send(b"i");
    session.wait_for("Node v");
    session.wait_for("Memory is process RSS: Node + Rust");
    session.wait_for(&format!(
        "Diagnostics {}",
        session.project.path().join("diagnostics.jsonl").display()
    ));
    session.capture("compiled-cli-120x30-info");
    session.send(b"\x1b");
    session.wait_for("q Stop");
    session.capture("compiled-cli-120x30-status");
    session.finish(b"q", "Bluink stopped");
    let captured =
        std::fs::read_to_string(session.project.path().join("diagnostics.jsonl")).unwrap();
    assert!(captured.contains("application.stop"));
}

impl Drop for Session {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
    }
}

#[test]
fn real_terminal_views_navigation_and_quit_work_at_all_supported_sizes() {
    for (columns, rows) in [(120, 30), (80, 24), (60, 12)] {
        let mut session = Session::new(columns, rows, false, false);
        session.wait_for("Running");
        session.wait_for("Errors 1");
        session.wait_for("Memory 184 MiB");
        session.wait_for("Warnings 1");
        session.capture(&format!("{columns}x{rows}-status"));
        session.send(b"l");
        session.wait_for("Logs ·");
        session.send(b"\x1b[A");
        session.capture(&format!("{columns}x{rows}-logs"));
        session.send(b"\r");
        session.wait_for("timestampMs");
        assert!(
            !session
                .parser
                .screen()
                .contents()
                .contains("private-fixture")
        );
        session.send(b"\x1b");
        session.send(b"e");
        session.wait_for("Errors ·");
        session.wait_for("fixture_error");
        session.capture(&format!("{columns}x{rows}-errors"));
        session.send(b"i");
        session.wait_for("Session information");
        session.send(b"\x1b[F");
        session.wait_for("entries evicted");
        session.capture(&format!("{columns}x{rows}-info"));
        session.send(b"?");
        session.wait_for("Keyboard shortcuts");
        session.capture(&format!("{columns}x{rows}-help"));
        session.send(b"\x1b");
        session.wait_for("q Stop");
        session.quit(b"q");
    }
}

#[test]
fn shutdown_restores_the_terminal_without_a_cursor_report() {
    for (columns, rows) in [(120, 30), (80, 24), (60, 12)] {
        // Isolate finalization from the application's ongoing log producer:
        // insert_before legitimately queries the cursor while rendering logs.
        let mut session = Session::launch(
            columns,
            rows,
            false,
            false,
            None,
            Some("console::terminal::tests::shutdown_child"),
        );
        session.wait_for("READY_FOR_SHUTDOWN");
        session.answer_cursor_queries = false;
        let shutdown_start = session.transcript.len();
        session.quit(b"q");
        assert!(
            !session.transcript[shutdown_start..]
                .windows(4)
                .any(|bytes| bytes == b"\x1b[6n")
        );
    }
}

#[test]
fn resize_fallback_and_ctrl_c_restore_the_terminal() {
    let mut session = Session::new(120, 30, false, true);
    session.wait_for("Running");
    session.resize(80, 24);
    // The previous 120-column footer is clipped after resizing. Wait until the
    // renderer has repositioned its counters before issuing another resize.
    session.wait_for("Errors 1");
    session.wait_for("q Stop");
    session.capture("resize-80x24");
    session.resize(50, 10);
    session.wait_for("switching to plain output");
    session.capture("resize-50x10-plain");
    session.quit(b"\x03");
}

#[test]
fn long_event_details_scroll_without_changing_the_selected_event() {
    let mut session = Session::new(60, 12, false, false);
    session.wait_for("Running");
    session.send(b"e");
    session.wait_for("fixture_error");
    session.send(b"\r");
    session.wait_for("timestampMs");
    session.send(b"\x1b[F");
    session.wait_for("durationMs");
    session.capture("60x12-details-scrolled");
    session.send(b"\x1b[H");
    session.wait_for("timestampMs");
    session.quit(b"q");
}

#[test]
fn no_color_retains_meaning_and_raw_ctrl_c_requests_shutdown() {
    let mut session = Session::new(80, 24, false, true);
    session.wait_for("Running");
    session.send(b"e");
    session.wait_for("fixture_error");
    assert!(
        !session
            .transcript
            .windows(5)
            .any(|window| window == b"\x1b[31m")
    );
    session.capture("80x24-no-color");
    session.quit(b"\x03");
}

/// Parent PTY tests explicitly invoke this helper in a fresh process.
#[test]
#[ignore = "PTY child process invoked by the parent acceptance tests"]
fn terminal_child() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        crate::diagnostics::configure_native_diagnostics().unwrap();
        let console = BluinkConsole::new(ConsoleOptions { cli_version: "26.10.1-alpha".to_owned(), node_version: "fixture-host".to_owned(), mode: Some(std::env::var("BLUINK_CONSOLE_TEST_MODE").unwrap()) }).unwrap();
        let application = BluinkApplication::new(ApplicationOptions {
            project_root: std::env::var("BLUINK_CONSOLE_TEST_PROJECT").unwrap(), timezone: Some("UTC".to_owned()), cache_location: Some("project".to_owned()), public_dir: None, data_dir: None, web_font_folder: None, pdf_font_folder: None, typst_binary: None,
        }).unwrap();
        console.attach_application(&application).unwrap();
        console.update_rss(Some(184.0 * 1_048_576.0)).unwrap();
        tokio::time::sleep(Duration::from_millis(200)).await;
        application.initialize(false).await.unwrap();
        application.start_http(Some(0)).await.unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let exiting = Arc::clone(&stop);
        let producer = std::thread::spawn(move || {
            for level in ["warn", "error"] {
                crate::diagnostics::record_native_diagnostic(serde_json::json!({"timestampMs":1,"level":level,"target":"bluink.fixture","name":format!("fixture_{level}"),"fields":{"error":"private-fixture","count":7},"spans":vec!["fixture.operation"; 40],"durationMs":1.0}).to_string()).unwrap();
            }
            let mut count = 0;
            while !exiting.load(Ordering::Acquire) {
                crate::diagnostics::record_native_diagnostic(serde_json::json!({"timestampMs":1,"level":"info","target":"bluink.fixture","name":"fixture_tick","fields":{"count":count}}).to_string()).unwrap();
                count += 1;
                std::thread::sleep(Duration::from_millis(150));
            }
        });
        // Node owns OS signals in production. The helper uses a safe Rust signal
        // flag only after a resize fallback returns the PTY to canonical mode.
        let mut signals = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt()).unwrap();
        loop {
            tokio::select! {
                action = console.next_action() => { if action.is_none_or(|action| action.kind == "shutdown") { break; } }
                _ = signals.recv() => break,
            }
        }
        stop.store(true, Ordering::Release);
        producer.join().unwrap();
        application.stop().await.unwrap();
        crate::diagnostics::shutdown_native_diagnostics().await.unwrap();
        console.stop().await.unwrap();
        console.stop().await.unwrap();
        assert!(console.next_action().await.is_none());
        println!("TERMINAL_SESSION_CLOSED");
    });
}
