// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use super::support::*;
use crate::application::{ApplicationOptions, BluinkApplication};
use serde_json::json;

#[test]
fn listener_admission_bind_failures_idempotent_stop_and_restart_are_owned_by_rust() {
    let f = Fixture::new(false);
    let root = tempfile::tempdir().unwrap();
    let uninitialized = BluinkApplication::new(ApplicationOptions {
        project_root: root.path().to_string_lossy().into_owned(),
        timezone: Some("UTC".into()),
        cache_location: None,
        public_dir: None,
        data_dir: None,
        web_font_folder: None,
        pdf_font_folder: None,
        typst_binary: None,
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
    assert_eq!(address.host, "127.0.0.1");
    assert!(address.port > 0);
    assert_eq!(address.url, format!("http://127.0.0.1:{}", address.port));
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
            .block_on(competing.app.start_http(Some(address.port)))
            .is_err()
    );
    f.runtime.block_on(async {
        let (a, b) = futures_util::future::join(f.app.stop_http(), f.app.stop_http()).await;
        a.unwrap();
        b.unwrap();
    });
    let rebound = competing
        .runtime
        .block_on(competing.app.start_http(Some(address.port)))
        .unwrap();
    assert_eq!(rebound.port, address.port);
    competing.runtime.block_on(competing.app.stop()).unwrap();
    f.runtime
        .block_on(f.app.start_http(Some(address.port)))
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
    assert_eq!(headers["content-type"], "text/css");
    assert_eq!(
        bytes.as_ref(),
        std::fs::read(fonts.join("inter.css")).unwrap()
    );
    assert!(f.call("read_metadata_fields", json!({})).is_array());
}
