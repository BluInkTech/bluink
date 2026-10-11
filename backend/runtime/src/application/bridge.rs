// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use std::sync::Arc;
use std::sync::atomic::Ordering;

use napi_derive::napi;

use super::{Application, error};
use crate::error as bridge_error;

#[napi(object)]
#[derive(Clone)]
pub struct ApplicationOptions {
    pub project_root: String,
    pub timezone: Option<String>,
    pub cache_location: Option<String>,
    pub public_dir: Option<String>,
    pub data_dir: Option<String>,
    pub web_font_folder: Option<String>,
    pub pdf_font_folder: Option<String>,
    pub typst_binary: Option<String>,
}

#[napi(object)]
pub struct HttpAddress {
    pub host: String,
    pub port: u32,
    pub url: String,
}

/// Process-local application handle shared by the CLI and the Rust HTTP adapter.
#[napi]
pub struct BluinkApplication {
    pub(crate) inner: Arc<Application>,
}

#[napi]
impl BluinkApplication {
    #[napi]
    #[allow(
        clippy::significant_drop_tightening,
        reason = "Keep application state guarded throughout admission checks and related operations."
    )]
    pub fn notify_file_change(&self, filename: Option<String>) -> napi::Result<()> {
        self.inner
            .check_running()
            .map_err(bridge_error::native_error)?;
        let state = self.inner.state().map_err(bridge_error::native_error)?;
        let index = state.index.as_ref().ok_or_else(|| {
            bridge_error::native_error("Initialize the search index before subscribing to changes.")
        })?;
        index.events.notify(filename);
        Ok(())
    }

    #[napi]
    #[allow(
        clippy::significant_drop_tightening,
        reason = "Keep application state guarded throughout admission checks and related operations."
    )]
    pub fn report_watcher_error(&self) -> napi::Result<()> {
        let state = self.inner.state().map_err(bridge_error::native_error)?;
        if let Some(index) = state.index.as_ref() {
            index.events.report_failure();
        }
        Ok(())
    }

    #[napi(constructor)]
    pub fn new(options: ApplicationOptions) -> napi::Result<Self> {
        Ok(Self {
            inner: Arc::new(Application::new(options).map_err(bridge_error::native_error)?),
        })
    }

    #[napi]
    pub async fn initialize(&self, with_index: bool) -> napi::Result<String> {
        let application = Arc::clone(&self.inner);
        let result = tokio::task::spawn_blocking(move || application.initialize_sync(with_index))
            .await
            .map_err(bridge_error::native_error)?
            .map_err(bridge_error::native_error)?;
        serde_json::to_string(&result).map_err(bridge_error::native_error)
    }

    #[napi]
    pub fn status(&self) -> napi::Result<String> {
        serde_json::to_string(&self.inner.status().map_err(bridge_error::native_error)?)
            .map_err(bridge_error::native_error)
    }

    #[napi]
    pub async fn execute_command(&self, command: String, source: String) -> napi::Result<String> {
        let application = Arc::clone(&self.inner);
        tokio::task::spawn_blocking(move || {
            let request = super::request::decode(source.as_bytes())?;
            let result = application.execute_command(&command, request)?;
            serde_json::to_string(&result).map_err(error::internal_error)
        })
        .await
        .map_err(bridge_error::native_error)?
        .map_err(Into::into)
    }

    #[napi]
    pub async fn start_http(&self, port: Option<u32>) -> napi::Result<HttpAddress> {
        let address = self
            .inner
            .http
            .start(&self.inner, port.unwrap_or(9000))
            .await
            .map_err(bridge_error::native_error)?;
        let url = format!("http://{address}");
        crate::console::http_listening(&self.inner, &url);
        Ok(HttpAddress {
            host: address.ip().to_string(),
            port: u32::from(address.port()),
            url,
        })
    }

    #[napi]
    pub async fn stop_http(&self) -> napi::Result<()> {
        self.inner.http.stop().await.map_err(Into::into)
    }

    #[napi]
    pub async fn stop(&self) -> napi::Result<()> {
        self.inner.stopping.store(true, Ordering::Release);
        // Network admission ends before canonical operations and index workers drain.
        // Always finish project cleanup even when the listener failed.
        let http = self.inner.http.stop().await;
        let application = Arc::clone(&self.inner);
        let project = tokio::task::spawn_blocking(move || application.stop_sync())
            .await
            .map_err(bridge_error::native_error)?;
        project.map_err(bridge_error::native_error)?;
        http.map_err(Into::into)
    }
}
