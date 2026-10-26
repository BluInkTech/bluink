// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use axum::{
    Router,
    extract::{Request, State},
    http::{Method, StatusCode, header},
    response::{IntoResponse, Response},
    routing::any,
};
use tokio::sync::oneshot;

use crate::application::error::{Result, internal_error};
pub(crate) use crate::http::files::file;

pub(crate) struct Preview {
    pub(crate) url: String,
    shutdown: oneshot::Sender<()>,
    thread: std::thread::JoinHandle<std::io::Result<()>>,
}

impl Preview {
    /// One owned runtime per preview listener, never per request. Its thread
    /// can be joined from application Drop without nesting the host runtime.
    pub(crate) fn start(root: PathBuf) -> Result<Self> {
        let listener = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        let address = listener.local_addr()?;
        listener.set_nonblocking(true)?;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let (shutdown, stopped) = oneshot::channel();
        let router = Router::new().fallback(any(serve_file)).with_state(root);
        let thread = std::thread::Builder::new()
            .name("bluink-export-preview".to_owned())
            .spawn(move || {
                runtime.block_on(async move {
                    let listener = tokio::net::TcpListener::from_std(listener)?;
                    let (draining, drain) = oneshot::channel();
                    let server = tokio::spawn(async move {
                        axum::serve(listener, router)
                            .with_graceful_shutdown(async {
                                let _ = drain.await;
                            })
                            .await
                    });
                    let _ = stopped.await;
                    let _ = draining.send(());
                    match tokio::time::timeout(Duration::from_secs(1), server).await {
                        Ok(result) => result.map_err(std::io::Error::other)?,
                        Err(_) => Ok(()),
                    }
                })
                // Dropping this owned runtime closes remaining connection tasks and
                // waits for any accepted blocking file reads before join returns.
            })?;
        Ok(Self {
            url: format!("http://{address}/"),
            shutdown,
            thread,
        })
    }

    pub(crate) fn stop(self) -> Result<()> {
        let Self {
            shutdown, thread, ..
        } = self;
        let _ = shutdown.send(());
        thread
            .join()
            .map_err(|_| internal_error("Website preview thread panicked."))?
            .map_err(internal_error)
    }
}

pub(crate) fn content_type(path: &Path) -> &'static str {
    crate::engine::media_type::for_path(path).unwrap_or("application/octet-stream")
}

async fn serve_file(State(root): State<PathBuf>, request: Request) -> Response {
    if request.method() != Method::GET {
        return StatusCode::METHOD_NOT_ALLOWED.into_response();
    }
    let path = request.uri().to_string();
    let result = tokio::task::spawn_blocking(move || {
        let file = file(&root, &path)?;
        Some((content_type(&file), std::fs::File::open(file).ok()?))
    })
    .await;
    match result {
        Ok(Some((mime, file))) => crate::http::files::stream(file, mime, ()).map_or_else(|_| (StatusCode::NOT_FOUND, "Page not found").into_response(), |mut response| {
                response.headers_mut().insert(header::CONTENT_SECURITY_POLICY, axum::http::HeaderValue::from_static("default-src 'none'; img-src 'self' data:; script-src 'self'; style-src 'unsafe-inline'; font-src 'self' data:; base-uri 'none'; form-action 'none'"));
                response.headers_mut().insert(
                    header::X_CONTENT_TYPE_OPTIONS,
                    axum::http::HeaderValue::from_static("nosniff"),
                );
                response
            }),
        _ => (StatusCode::NOT_FOUND, "Page not found").into_response(),
    }
}
