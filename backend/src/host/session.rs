// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use std::sync::Arc;

use crate::application::{Application, ApplicationOptions, error};

/// Process-local application handle shared by the CLI and the HTTP adapter.
pub(crate) struct ApplicationHandle {
    pub(crate) inner: Arc<Application>,
    http: crate::http::Runtime,
}

impl ApplicationHandle {
    #[cfg(test)]
    pub(crate) fn from_application(application: Application) -> Self {
        Self {
            inner: Arc::new(application),
            http: crate::http::Runtime::default(),
        }
    }

    pub(crate) fn new(options: ApplicationOptions) -> anyhow::Result<Self> {
        Ok(Self {
            inner: Arc::new(Application::new(options)?),
            http: crate::http::Runtime::default(),
        })
    }

    pub(crate) async fn initialize(&self, background_sync: bool) -> anyhow::Result<()> {
        let application = Arc::clone(&self.inner);
        tokio::task::spawn_blocking(move || {
            application.initialize_sync()?;
            if background_sync {
                application.start_automatic_sync()?;
            }
            Ok::<_, error::ApplicationError>(())
        })
        .await??;
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn status(&self) -> anyhow::Result<String> {
        Ok(serde_json::to_string(&self.inner.status()?)?)
    }

    /// Run one CLI application command and render its result for stdout:
    /// text as is, structured results as indented JSON.
    pub(crate) async fn run_command(
        &self,
        command: &str,
        request: crate::application::commands::CommandRequest,
    ) -> anyhow::Result<String> {
        let application = Arc::clone(&self.inner);
        let command = command.to_owned();
        tokio::task::spawn_blocking(move || {
            let result = serde_json::to_value(application.execute_command(&command, request)?)
                .map_err(error::internal_error)?;
            Ok::<_, error::ApplicationError>(match &result["value"] {
                serde_json::Value::String(text) if result["kind"] == "text" => text.clone(),
                value => serde_json::to_string_pretty(value).map_err(error::internal_error)?,
            })
        })
        .await?
        .map_err(Into::into)
    }

    pub(crate) async fn start_http(
        &self,
        port: Option<u32>,
    ) -> anyhow::Result<std::net::SocketAddr> {
        let address = self.http.start(&self.inner, port.unwrap_or(9000)).await?;
        crate::console::http_listening(&self.inner, &format!("http://{address}"));
        Ok(address)
    }

    #[cfg(test)]
    pub(crate) async fn stop_http(&self) -> anyhow::Result<()> {
        self.http.stop().await.map_err(Into::into)
    }

    pub(crate) async fn stop(&self) -> anyhow::Result<()> {
        self.inner.begin_shutdown();
        self.inner.cancel_owned_sync();
        // Network admission ends before in-flight operations drain and the store closes.
        // Always finish project cleanup even when the listener failed.
        let http = self.http.stop().await;
        let application = Arc::clone(&self.inner);
        let project = tokio::task::spawn_blocking(move || application.stop_sync()).await?;
        project?;
        http.map_err(Into::into)
    }
}
