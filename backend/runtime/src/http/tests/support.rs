// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use super::super::*;
use crate::application::{ApplicationOptions, BluinkApplication};
use axum::body::{Body, Bytes};
use axum::http::HeaderMap;
use serde_json::{Value, json};
use tower::ServiceExt;

pub(super) struct Fixture {
    pub root: tempfile::TempDir,
    pub app: BluinkApplication,
    pub runtime: tokio::runtime::Runtime,
    router: Router,
}

impl Fixture {
    pub(crate) fn new(index: bool) -> Self {
        Self::with_options(index, |_| {})
    }

    pub(crate) fn with_options(
        index: bool,
        configure: impl FnOnce(&mut ApplicationOptions),
    ) -> Self {
        let root = tempfile::tempdir().unwrap();
        let mut options = ApplicationOptions {
            project_root: root.path().to_string_lossy().into_owned(),
            timezone: Some("UTC".into()),
            cache_location: Some("project".into()),
            public_dir: None,
            data_dir: None,
            web_font_folder: None,
            pdf_font_folder: None,
            typst_binary: None,
        };
        configure(&mut options);
        let app = BluinkApplication::new(options).unwrap();
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .worker_threads(2)
            .build()
            .unwrap();
        runtime.block_on(app.initialize(index)).unwrap();
        let router = router(&app.inner, Arc::new(Semaphore::new(BLOCKING_CAPACITY)));
        Self {
            root,
            app,
            runtime,
            router,
        }
    }

    pub(crate) fn request(
        &self,
        method: &str,
        path: &str,
        body: impl Into<Body>,
    ) -> (StatusCode, HeaderMap, Bytes) {
        let request = Request::builder()
            .method(method)
            .uri(path)
            .body(body.into())
            .unwrap();
        self.send(request)
    }

    pub(crate) fn send(&self, request: Request) -> (StatusCode, HeaderMap, Bytes) {
        self.runtime.block_on(self.send_async(request))
    }

    pub(crate) async fn send_async(&self, request: Request) -> (StatusCode, HeaderMap, Bytes) {
        let response = self.router.clone().oneshot(request).await.unwrap();
        let (parts, body) = response.into_parts();
        (
            parts.status,
            parts.headers,
            axum::body::to_bytes(body, usize::MAX).await.unwrap(),
        )
    }
    #[expect(
        clippy::needless_pass_by_value,
        reason = "Test requests own fixture values just like decoded production requests."
    )]
    pub(crate) fn call(&self, name: &str, body: Value) -> Value {
        let method = if name == "cache_status" {
            "GET"
        } else {
            "POST"
        };
        let (status, headers, bytes) =
            self.request(method, &format!("/api/{name}"), body.to_string());
        assert_eq!(
            status,
            StatusCode::OK,
            "{name}: {}",
            String::from_utf8_lossy(&bytes)
        );
        assert_eq!(headers[header::CONTENT_TYPE], "application/json");
        serde_json::from_slice(&bytes).unwrap()
    }
    #[expect(
        clippy::needless_pass_by_value,
        reason = "Test requests own fixture values just like decoded production requests."
    )]
    pub(crate) fn error(&self, name: &str, body: Value, expected: StatusCode) -> String {
        let (status, _, bytes) = self.request("POST", &format!("/api/{name}"), body.to_string());
        assert_eq!(
            status,
            expected,
            "{name}: {}",
            String::from_utf8_lossy(&bytes)
        );
        serde_json::from_slice::<Value>(&bytes).unwrap()["error"]
            .as_str()
            .unwrap()
            .to_owned()
    }

    pub(crate) fn page(&self, title: &str, text: &str) -> Value {
        self.call(
            "create_item",
            json!({"meta":{"type":"page","title":title},"content":document(text)}),
        )
    }

    pub(crate) fn initialize_index(&self) {
        self.runtime.block_on(self.app.initialize(true)).unwrap();
    }

    pub(crate) fn reopen(&mut self, index: bool) {
        self.runtime.block_on(self.app.stop()).unwrap();
        self.app = BluinkApplication::new(ApplicationOptions {
            project_root: self.root.path().to_string_lossy().into_owned(),
            timezone: Some("UTC".into()),
            cache_location: Some("project".into()),
            public_dir: None,
            data_dir: None,
            web_font_folder: None,
            pdf_font_folder: None,
            typst_binary: None,
        })
        .unwrap();
        self.runtime.block_on(self.app.initialize(index)).unwrap();
        self.router = router(&self.app.inner, Arc::new(Semaphore::new(BLOCKING_CAPACITY)));
    }

    pub(crate) fn item_path(&self, id: &Value) -> std::path::PathBuf {
        let id = id.as_str().unwrap();
        self.root
            .path()
            .join(format!("items/{}/{id}/item.json", &id[..2]))
    }

    pub(crate) fn settled(&self) {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let status = self.call("cache_status", json!({}));
            if status["pending"] == 0 && status["structuralState"] == "ready" {
                return;
            }
            assert!(Instant::now() < deadline, "index did not settle: {status}");
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.runtime.block_on(self.app.stop()).unwrap();
    }
}

pub(super) fn document(text: &str) -> Value {
    if text.is_empty() {
        return json!({"type":"doc","content":[{"type":"paragraph"}]});
    }
    json!({"type":"doc","content":[{"type":"paragraph","content":[{"type":"text","text":text}]}]})
}
