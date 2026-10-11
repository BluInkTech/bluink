// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use super::support::*;
use axum::body::{Body, Bytes};
use axum::http::Request;
use futures_util::StreamExt;
use serde_json::{Value, json};
use std::io::Write;
use std::time::{Duration, Instant};

type Expected = Vec<(String, u16, usize)>;

fn call(f: &Fixture, expected: &mut Expected, name: &str, body: &str, method: &str) -> Value {
    let (status, _, bytes) = f.request(method, &format!("/api/{name}"), body.to_owned());
    expected.push((
        if name.starts_with("private-") {
            "unknown"
        } else {
            name
        }
        .to_owned(),
        status.as_u16(),
        bytes.len(),
    ));
    serde_json::from_slice(&bytes).unwrap()
}

// Run only in a fresh test process: subscriber, thresholds and sink are process-wide.
#[test]
#[ignore = "invoked by the three diagnostic contract tests with isolated environment"]
fn capture_child() {
    let directory =
        std::path::PathBuf::from(std::env::var_os("BLUINK_HTTP_CAPTURE_CHILD").unwrap());
    crate::diagnostics::configure_native_diagnostics().unwrap();
    crate::diagnostics::configure_native_diagnostics().unwrap();
    let f = Fixture::with_options(false, |options| {
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
        options.web_font_folder =
            Some(root.join("fonts/assets/web").to_string_lossy().into_owned());
        options.pdf_font_folder =
            Some(root.join("fonts/assets/pdf").to_string_lossy().into_owned());
        options.typst_binary = Some(
            directory
                .join("private-missing-typst")
                .to_string_lossy()
                .into_owned(),
        );
    });
    let mut expected = Vec::new();
    let item=call(&f,&mut expected,"create_item",&json!({"meta":{"type":"page","title":"private-document-title"},"content":document("private-authored-prose")}).to_string(),"POST");
    let read = json!({"id":item["id"]}).to_string();
    let request = |name: &str, body: &str| {
        Request::builder()
            .method("POST")
            .uri(format!("/api/{name}"))
            .body(Body::from(body.to_owned()))
            .unwrap()
    };
    let (a, b) = f.runtime.block_on(futures_util::future::join(
        f.send_async(request("read_metadata_fields", "{}")),
        f.send_async(request("read_item", &read)),
    ));
    expected.push(("read_metadata_fields".into(), a.0.as_u16(), a.2.len()));
    expected.push(("read_item".into(), b.0.as_u16(), b.2.len()));
    for (name, body, method) in [
        ("read_item", "{}", "POST"),
        ("read_item", "{", "POST"),
        ("private-path-query-secret", "{}", "POST"),
        ("read_item", "", "GET"),
        ("read_item", &" ".repeat(1_048_577), "POST"),
    ] {
        call(&f, &mut expected, name, body, method);
    }
    let read = Bytes::from(read);
    let first = read.slice(..1);
    let rest = read.slice(1..);
    let body = futures_util::stream::once(async move { Ok::<_, std::io::Error>(first) }).chain(
        futures_util::stream::once(async move {
            tokio::time::sleep(Duration::from_millis(80)).await;
            Ok::<_, std::io::Error>(rest)
        }),
    );
    let (status, _, bytes) = f.request("POST", "/api/read_item", Body::from_stream(body));
    expected.push(("read_item".into(), status.as_u16(), bytes.len()));
    let address = f.runtime.block_on(f.app.start_http(Some(0))).unwrap();
    let mut socket =
        std::net::TcpStream::connect(("127.0.0.1", u16::try_from(address.port).unwrap())).unwrap();
    socket
        .write_all(
            b"POST /api/read_item HTTP/1.1\r\nHost: localhost\r\nContent-Length: 100\r\n\r\n{",
        )
        .unwrap();
    std::thread::sleep(Duration::from_millis(80));
    drop(socket);
    let mut outcomes = Vec::new();
    for format in ["html", "pdf"] {
        let job=call(&f,&mut expected,"create_export",&json!({"source":{"kind":"note","id":item["id"]},"format":format,"start":true,"themeId":"classic","outsideLinks":"text-only","profile":"default"}).to_string(),"POST");
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let status = call(&f, &mut expected, "read_export", &job.to_string(), "POST");
            if matches!(status["state"].as_str().unwrap(), "ready" | "failed") {
                outcomes.push(status["state"].clone());
                break;
            }
            assert!(Instant::now() < deadline, "{status}");
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    let filename = f.item_path(&item["id"]);
    let original = std::fs::read(&filename).unwrap();
    std::fs::write(&filename, "private-invalid-json").unwrap();
    call(
        &f,
        &mut expected,
        "read_item",
        &json!({"id":item["id"]}).to_string(),
        "POST",
    );
    std::fs::write(filename, original).unwrap();
    stop_preserving_shared_sink(&f, &mut expected, address.port);
    crate::diagnostics::record_native_diagnostic(json!({"timestampMs":1,"level":"info","target":"bluink.host","name":"host_finished","fields":{"phase":"cleanup"}}).to_string()).unwrap();
    let statistics: Value = serde_json::from_str(
        &f.runtime
            .block_on(crate::diagnostics::shutdown_native_diagnostics())
            .unwrap(),
    )
    .unwrap();
    std::fs::write(directory.join("expected.json"),json!({"expected":expected,"outcomes":outcomes,"statistics":statistics,"projectRoot":f.root.path()}).to_string()).unwrap();
}

fn stop_preserving_shared_sink(f: &Fixture, expected: &mut Expected, port: u32) {
    // An application owns its listener, but not the process-wide diagnostic sink.
    let remaining = Fixture::new(false);
    let remaining_address = remaining
        .runtime
        .block_on(remaining.app.start_http(Some(0)))
        .unwrap();
    f.runtime.block_on(f.app.stop()).unwrap();
    assert!(std::net::TcpStream::connect(("127.0.0.1", u16::try_from(port).unwrap())).is_err());
    assert!(
        std::net::TcpStream::connect(("127.0.0.1", u16::try_from(remaining_address.port).unwrap()))
            .is_ok()
    );
    call(&remaining, expected, "read_metadata_fields", "{}", "POST");
    remaining
        .runtime
        .block_on(crate::diagnostics::flush_native_diagnostics())
        .unwrap();
    call(&remaining, expected, "read_metadata_fields", "{}", "POST");
    remaining.runtime.block_on(remaining.app.stop()).unwrap();
}

#[expect(
    clippy::too_many_lines,
    reason = "Keep each end-to-end scenario together so setup, failure checks and cleanup remain visible."
)]
fn capture(threshold: &str) {
    let directory = tempfile::tempdir().unwrap();
    let filename = directory.path().join("capture.jsonl");
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "http::tests::diagnostics::capture_child",
            "--ignored",
            "--nocapture",
        ])
        .env("BLUINK_HTTP_CAPTURE_CHILD", directory.path())
        .env("BLUINK_DIAGNOSTICS_FILE", &filename)
        .env("BLUINK_LOG_LEVEL", "debug")
        .env("BLUINK_RPC_SLOW_MS", threshold)
        .env("NO_COLOR", "1")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let result: Value =
        serde_json::from_slice(&std::fs::read(directory.path().join("expected.json")).unwrap())
            .unwrap();
    assert_eq!(result["outcomes"], json!(["ready", "failed"]));
    assert_eq!(
        result["statistics"],
        json!({"droppedEvents":0,"writeFailures":0})
    );
    let capture = std::fs::read_to_string(filename).unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!format!("{capture}{stderr}").contains("private-"));
    assert!(!capture.contains(result["projectRoot"].as_str().unwrap()));
    assert!(!stderr.contains('\u{1b}'));
    let events = capture
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(events.last().unwrap()["name"], "host_finished");
    let first_stop = events
        .iter()
        .position(|event| event["name"] == "application.stop")
        .unwrap();
    assert_eq!(
        events[first_stop + 1..]
            .iter()
            .filter(|event| event["name"] == "request_complete"
                && event["fields"]["operation"] == "read_metadata_fields"
                && event["fields"]["status"] == 200)
            .count(),
        2
    );
    let completed = events
        .iter()
        .filter(|event| event["name"] == "request_complete")
        .collect::<Vec<_>>();
    assert_eq!(
        completed.len(),
        result["expected"].as_array().unwrap().len() + 1
    );
    let interrupted = completed
        .iter()
        .filter(|event| {
            matches!(event["fields"]["status"].as_u64(), Some(400 | 499))
                && event["fields"]["body_read_ms"].as_f64().unwrap() >= 50.0
        })
        .collect::<Vec<_>>();
    assert_eq!(interrupted.len(), 1);
    let ids = completed
        .iter()
        .map(|event| event["fields"]["request_seq"].as_u64().unwrap())
        .collect::<std::collections::HashSet<_>>();
    assert_eq!(ids.len(), completed.len());
    let mut actual = completed
        .iter()
        .filter(|event| event["fields"]["request_seq"] != interrupted[0]["fields"]["request_seq"])
        .map(|event| {
            json!([
                event["fields"]["operation"],
                event["fields"]["status"],
                event["fields"]["response_bytes"]
            ])
            .to_string()
        })
        .collect::<Vec<_>>();
    actual.sort();
    let mut expected = result["expected"]
        .as_array()
        .unwrap()
        .iter()
        .map(Value::to_string)
        .collect::<Vec<_>>();
    expected.sort();
    assert_eq!(actual, expected);
    for event in &completed {
        for field in [
            "duration_ms",
            "body_read_ms",
            "decode_ms",
            "worker_queue_ms",
            "admission_ms",
            "readiness_ms",
            "execute_ms",
            "encode_ms",
        ] {
            assert!(
                event["fields"][field].as_f64().is_some_and(|n| n >= 0.0),
                "{field}: {event}"
            );
        }
        let level = if event["fields"]["status"].as_u64().unwrap() >= 500 {
            "error"
        } else if threshold == "0" {
            "warn"
        } else {
            "debug"
        };
        assert_eq!(event["level"], level);
    }
    assert!(
        completed.iter().any(
            |event| event["fields"]["body_read_ms"].as_f64().unwrap() >= 50.0
                && event["fields"]["execute_ms"].as_f64().unwrap()
                    < event["fields"]["body_read_ms"].as_f64().unwrap()
        )
    );
    assert!(
        completed
            .iter()
            .any(|event| event["fields"]["status"] == 500
                && event["message"] == "HTTP request failed")
    );
    let failure = events
        .iter()
        .find(|event| event["name"] == "export_failed")
        .unwrap();
    assert_eq!(failure["message"], "Export generation failed");
    assert!(failure["fields"]["export_seq"].is_number());
    assert!(
        completed
            .iter()
            .any(|event| event["fields"]["operation"] == "create_export"
                && event["fields"]["request_seq"] == failure["fields"]["request_seq"])
    );
    assert!(
        failure["spans"]
            .as_array()
            .unwrap()
            .contains(&json!("export.job"))
    );
    for phase in ["planning", "rendering", "cleanup"] {
        assert!(events.iter().any(|event| event["name"] == "export_phase"
            && event["fields"]["phase"] == phase
            && event["fields"]["duration_ms"].is_number()));
    }
    assert!(
        events
            .iter()
            .any(|event| event["name"] == "export_queued" && event["fields"]["capacity"] == 1)
    );
    assert!(
        events
            .iter()
            .any(|event| event["name"] == "export_dequeued"
                && event["fields"]["queue_ms"].is_number())
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| event["name"] == "invalid_slow_threshold")
            .count(),
        usize::from(threshold == "-1")
    );
}

#[test]
fn ordinary_requests_export_context_privacy_and_flush_are_captured_by_rust() {
    capture("100000");
}
#[test]
fn slow_requests_have_warning_severity() {
    capture("0");
}
#[test]
fn invalid_threshold_falls_back_once() {
    capture("-1");
}
