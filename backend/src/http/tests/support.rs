// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use super::super::*;
use crate::application::ApplicationOptions;
use crate::host::session::ApplicationHandle;
use axum::body::{Body, Bytes};
use axum::http::HeaderMap;
use serde_json::{Value, json};
use tower::ServiceExt;

pub(super) struct Fixture {
    pub root: tempfile::TempDir,
    pub app: ApplicationHandle,
    pub runtime: tokio::runtime::Runtime,
    router: Router,
}

impl Fixture {
    pub(crate) fn new(background_sync: bool) -> Self {
        Self::with_options(background_sync, |_| {})
    }

    pub(crate) fn with_options(
        background_sync: bool,
        configure: impl FnOnce(&mut ApplicationOptions),
    ) -> Self {
        let root = tempfile::tempdir().unwrap();
        let mut options = ApplicationOptions {
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
            typst_binary: None,
        };
        configure(&mut options);
        let app = ApplicationHandle::new(options).unwrap();
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .worker_threads(2)
            .build()
            .unwrap();
        runtime.block_on(app.initialize(background_sync)).unwrap();
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
        let method = if name == "read_storage_status" {
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
            json!({"meta":{"type":"page","title":title},"content":page_content(text)}),
        )
    }

    pub(crate) fn reopen(&mut self, background_sync: bool) {
        self.runtime.block_on(self.app.stop()).unwrap();
        self.app = ApplicationHandle::new(self.app.inner.options.clone()).unwrap();
        self.runtime
            .block_on(self.app.initialize(background_sync))
            .unwrap();
        self.router = router(&self.app.inner, Arc::new(Semaphore::new(BLOCKING_CAPACITY)));
    }

    pub(crate) fn asset_directory(&self, id: &Value) -> std::path::PathBuf {
        crate::domain::project::identity::ItemId::try_from(id.as_str().unwrap())
            .unwrap()
            .asset_directory(self.root.path())
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.runtime.block_on(self.app.stop()).unwrap();
    }
}

/// A page's content holding one paragraph of text.
pub(super) fn page_content(text: &str) -> Value {
    if text.is_empty() {
        return json!({"document":{"type":"doc","content":[{"type":"paragraph"}]}});
    }
    json!({"document":{"type":"doc","content":[{"type":"paragraph","content":[{"type":"text","text":text}]}]}})
}
