// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use super::support::*;
use crate::application::ApplicationOptions;
use crate::host::session::ApplicationHandle;
use serde_json::json;

#[test]
fn listener_admission_bind_failures_idempotent_stop_and_restart_are_owned_by_rust() {
    let f = Fixture::new(false);
    let root = tempfile::tempdir().unwrap();
    let uninitialized = ApplicationHandle::new(ApplicationOptions {
        workspace_template_path: concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/resources/workspace-starter.json"
        )
        .into(),
        project_root: root.path().to_string_lossy().into_owned(),
        user_data_dir: Some(
            std::path::PathBuf::from(root.path().to_string_lossy().into_owned())
                .join(".cache/test-user")
                .to_string_lossy()
                .into_owned(),
        ),
        timezone: Some("UTC".into()),
        public_dir: None,
        web_font_folder: None,
        pdf_font_folder: None,
        docx_font_folder: None,
        pdf_renderer: None,
    })
    .unwrap();
    assert!(
        f.runtime
            .block_on(uninitialized.start_http(Some(0)))
            .err()
            .unwrap()
            .to_string()
            .contains("Initialize the project")
    );
    f.runtime.block_on(uninitialized.stop()).unwrap();
    assert!(
        f.runtime
            .block_on(f.app.start_http(Some(65_536)))
            .err()
            .unwrap()
            .to_string()
            .contains("Choose a port")
    );
    let address = f.runtime.block_on(f.app.start_http(Some(0))).unwrap();
    assert_eq!(address.ip(), std::net::Ipv4Addr::LOCALHOST);
    assert!(address.port() > 0);
    assert!(
        f.runtime
            .block_on(f.app.start_http(Some(0)))
            .err()
            .unwrap()
            .to_string()
            .contains("already started")
    );
    let competing = Fixture::new(false);
    assert!(
        competing
            .runtime
            .block_on(competing.app.start_http(Some(u32::from(address.port()))))
            .is_err()
    );
    f.runtime.block_on(async {
        let (a, b) = futures_util::future::join(f.app.stop_http(), f.app.stop_http()).await;
        a.unwrap();
        b.unwrap();
    });
    let rebound = competing
        .runtime
        .block_on(competing.app.start_http(Some(u32::from(address.port()))))
        .unwrap();
    assert_eq!(rebound.port(), address.port());
    competing.runtime.block_on(competing.app.stop()).unwrap();
    f.runtime
        .block_on(f.app.start_http(Some(u32::from(address.port()))))
        .unwrap();
    f.runtime.block_on(async {
        let (a, b) = futures_util::future::join(f.app.stop(), f.app.stop()).await;
        a.unwrap();
        b.unwrap();
    });
    assert!(
        f.runtime
            .block_on(f.app.start_http(Some(0)))
            .err()
            .unwrap()
            .to_string()
            .contains("shutting down")
    );
}

#[test]
fn offline_font_stylesheets_are_served_from_the_configured_resource_folder() {
    let fonts = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../fonts/assets/web");
    let f = Fixture::with_options(false, |options| {
        options.web_font_folder = Some(fonts.to_string_lossy().into_owned());
    });
    let (status, headers, bytes) = f.request("GET", "/fonts/inter.css", "");
    assert_eq!(status, axum::http::StatusCode::OK);
    assert_eq!(headers["content-type"], "text/css; charset=utf-8");
    assert_eq!(
        bytes.as_ref(),
        std::fs::read(fonts.join("inter.css")).unwrap()
    );
    assert!(f.call("read_metadata_fields", json!({})).is_array());
}

#[test]
fn only_loopback_hosts_and_pages_reach_the_server() {
    use axum::body::Body;
    use axum::http::{Request, StatusCode, header};
    let f = Fixture::new(false);
    let send = |method: &str, path: &str, host: Option<&str>, origin: Option<&str>| {
        let mut request = Request::builder().method(method).uri(path);
        if let Some(host) = host {
            request = request.header(header::HOST, host);
        }
        if let Some(origin) = origin {
            request = request.header(header::ORIGIN, origin);
        }
        f.send(request.body(Body::from("{}")).unwrap()).0
    };
    let read = "/api/list_publication_themes";
    for host in [
        "127.0.0.1:9000",
        "localhost:5174",
        "[::1]:9000",
        "app.localhost",
    ] {
        assert_eq!(
            send("POST", read, Some(host), None),
            StatusCode::OK,
            "{host}"
        );
    }
    // A rebound DNS name would otherwise make the attacker's page same-origin.
    for host in [
        "evil.example:9000",
        "127.0.0.1.evil.example",
        "localhost.evil.example",
    ] {
        assert_eq!(
            send("GET", "/", Some(host), None),
            StatusCode::FORBIDDEN,
            "{host}"
        );
        assert_eq!(
            send("POST", read, Some(host), None),
            StatusCode::FORBIDDEN,
            "{host}"
        );
    }
    // Development pages on another loopback port may write; other sites may not,
    // even through a simple POST that skips the CORS preflight.
    let host = Some("127.0.0.1:9000");
    for origin in ["http://localhost:5174", "http://127.0.0.1:9000"] {
        assert_eq!(
            send("POST", read, host, Some(origin)),
            StatusCode::OK,
            "{origin}"
        );
    }
    for origin in [
        "https://evil.example",
        "null",
        "http://localhost.evil.example",
    ] {
        assert_eq!(
            send("POST", read, host, Some(origin)),
            StatusCode::FORBIDDEN,
            "{origin}"
        );
        assert_eq!(
            send("POST", "/api/upload_asset", host, Some(origin)),
            StatusCode::FORBIDDEN,
            "{origin}"
        );
    }
    // Cross-origin reads of static files stay governed by CORS, not this guard.
    assert_ne!(
        send("GET", "/", host, Some("https://evil.example")),
        StatusCode::FORBIDDEN
    );
}
