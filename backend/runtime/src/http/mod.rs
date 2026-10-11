// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! Product HTTP transport: requests, files and responses remain entirely in Rust.
mod artifacts;
pub(crate) mod files;
mod headers;
#[cfg(test)]
mod tests;
mod trace;
pub(crate) mod uploads;

use std::net::{Ipv4Addr, SocketAddr};
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

use axum::extract::{Request, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::any;
use axum::{Extension, Router, middleware};
use tokio::net::TcpListener;
use tokio::sync::{Mutex, Semaphore, oneshot};
use tokio::task::JoinHandle;

use crate::application::Application;
use crate::application::error::{ApplicationError, Result, internal_error, invalid_request};
use crate::application::operation;

const DEFAULT_BODY_BYTES: usize = 1_048_576;
const LARGE_BODY_BYTES: usize = 16 * DEFAULT_BODY_BYTES;
const DRAIN_TIMEOUT: Duration = Duration::from_secs(30);
// Backpressure before entering Tokio's blocking pool, shared by RPC and file IO.
// Export rendering retains its separate process-wide PDF/web capacity lanes.
const BLOCKING_CAPACITY: usize = 8;

#[derive(Clone)]
struct HttpState {
    application: Weak<Application>,
    work: Arc<Semaphore>,
}

struct Server {
    work: Arc<Semaphore>,
    shutdown: oneshot::Sender<()>,
    task: JoinHandle<std::io::Result<()>>,
}

#[derive(Default)]
pub(crate) struct Runtime {
    server: Mutex<Option<Server>>,
}

impl Runtime {
    #[expect(
        clippy::significant_drop_tightening,
        reason = "The listener guard serializes start and stop through completion."
    )]
    pub(crate) async fn start(
        &self,
        application: &Arc<Application>,
        port: u32,
    ) -> Result<SocketAddr> {
        let port =
            u16::try_from(port).map_err(|_| invalid_request("Choose a port from 0 to 65535."))?;
        let mut server = self.server.lock().await;
        {
            let _operation = application.begin_operation()?;
        }
        if server.is_some() {
            return Err(invalid_request("The HTTP server is already started."));
        }
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, port)).await?;
        let address = listener.local_addr()?;
        // Stop can be requested while bind awaits. Do not publish a listener then.
        application.check_running()?;
        let (shutdown, stopped) = oneshot::channel();
        let work = Arc::new(Semaphore::new(BLOCKING_CAPACITY));
        let router = router(application, Arc::clone(&work));
        let task = tokio::spawn(async move {
            axum::serve(listener, router)
                .with_graceful_shutdown(async {
                    let _ = stopped.await;
                })
                .await
        });
        *server = Some(Server {
            work,
            shutdown,
            task,
        });
        Ok(address)
    }
    #[expect(
        clippy::significant_drop_tightening,
        reason = "The listener guard serializes start and stop through completion."
    )]
    pub(crate) async fn stop(&self) -> Result<()> {
        // Serialize stop/start and concurrent stops through the full network drain.
        let mut server = self.server.lock().await;
        if let Some(Server {
            shutdown,
            mut task,
            work,
        }) = server.take()
        {
            work.close();
            let _ = shutdown.send(());
            if let Ok(result) = tokio::time::timeout(DRAIN_TIMEOUT, &mut task).await {
                result.map_err(internal_error)?.map_err(internal_error)?;
            } else {
                tracing::warn!(target: "bluink.http", "HTTP connection drain timed out");
                task.abort();
                let _ = task.await;
                // Blocking writes still hold application operation guards.
                // Application shutdown waits for them after closing connections.
            }
        }
        Ok(())
    }
}

// Both the listener and in-process HTTP tests use this complete production router.
fn router(application: &Arc<Application>, work: Arc<Semaphore>) -> Router {
    Router::new()
        .route("/api/upload_asset", any(uploads::serve))
        .route("/api/upload_asset/", any(uploads::serve))
        .route("/api/export_artifact/{id}", any(artifacts::serve))
        .route("/api/export_artifact/{id}/", any(artifacts::serve))
        .route("/api/export_artifact/{id}/{*path}", any(artifacts::serve))
        .route("/api/{operation}", any(rpc))
        .fallback(files::serve)
        .layer(middleware::from_fn(headers::security))
        .layer(middleware::from_fn(trace::request))
        // A weak reference prevents a listener/application ownership cycle.
        .with_state(HttpState {
            application: Arc::downgrade(application),
            work,
        })
}

impl Drop for Runtime {
    fn drop(&mut self) {
        if let Some(server) = self.server.get_mut().take() {
            server.work.close();
            let _ = server.shutdown.send(());
            server.task.abort();
        }
    }
}

async fn rpc(
    State(state): State<HttpState>,
    Extension(record): Extension<trace::Record>,
    request: Request,
) -> Response {
    // Operation names are fixed ASCII route keys. Preserve raw matching and
    // JSON errors rather than letting Path reject/percent-decode unknown keys.
    let operation = request
        .uri()
        .path()
        .strip_prefix("/api/")
        .unwrap_or_default()
        .to_owned();
    let Some(method) = operation::Request::http_method(&operation) else {
        return rpc_failure(&record, StatusCode::NOT_FOUND, "ERR_RPC_ENDPOINT_NOT_FOUND");
    };
    if request.method().as_str() != method {
        return rpc_failure(
            &record,
            StatusCode::METHOD_NOT_ALLOWED,
            "ERR_RPC_INVALID_METHOD",
        );
    }
    let source = if method == "GET" {
        axum::body::Bytes::from_static(b"{}")
    } else {
        let limit = if matches!(operation.as_str(), "create_item" | "update_item") {
            LARGE_BODY_BYTES
        } else {
            DEFAULT_BODY_BYTES
        };
        let reading = record.reading();
        let body = axum::body::to_bytes(request.into_body(), limit).await;
        drop(reading);
        match body {
            Ok(bytes) => bytes,
            Err(error) => {
                use std::error::Error;
                let oversized = error.source().is_some_and(
                    <dyn std::error::Error + 'static>::is::<http_body_util::LengthLimitError>,
                );
                return if oversized {
                    rpc_failure(
                        &record,
                        StatusCode::PAYLOAD_TOO_LARGE,
                        "ERR_PAYLOAD_TOO_LARGE",
                    )
                } else {
                    rpc_failure(&record, StatusCode::BAD_REQUEST, "ERR_INVALID_JSON")
                };
            }
        }
    };
    let worker_record = record.clone();
    let result = worker(&state, record.clone(), move |application| {
        let started = Instant::now();
        let request = operation::Request::decode(&operation, &source);
        worker_record.decode(started.elapsed());
        let request = request?;
        let started = Instant::now();
        let (result, waits) = crate::diagnostics::waits::capture(|| application.execute(request));
        worker_record.execution(started.elapsed(), waits);
        let value = result?;
        let started = Instant::now();
        let encoded = serde_json::to_vec(&value).map_err(internal_error);
        worker_record.encode(started.elapsed());
        encoded
    })
    .await;
    match result {
        Ok(bytes) => ([(header::CONTENT_TYPE, "application/json")], bytes).into_response(),
        Err(error) => error_response(&record, error),
    }
}

async fn worker<T: Send + 'static>(
    state: &HttpState,
    record: trace::Record,
    action: impl FnOnce(Arc<Application>) -> Result<T> + Send + 'static,
) -> Result<T> {
    let queued = Instant::now();
    let permit = Arc::clone(&state.work)
        .acquire_owned()
        .await
        .map_err(|_| invalid_request("The application is shutting down."))?;
    let application = state
        .application
        .upgrade()
        .ok_or_else(|| invalid_request("The application is shutting down."))?;
    let span = tracing::Span::current();
    tokio::task::spawn_blocking(move || {
        // Keep admission occupied even if a client drops its request future.
        let _permit = permit;
        span.in_scope(|| {
            record.queue(queued.elapsed());
            action(application)
        })
    })
    .await
    .map_err(|_| internal_error("The request could not be completed."))?
}

async fn blocking<T: Send + 'static>(
    state: &HttpState,
    record: trace::Record,
    action: impl FnOnce(Arc<Application>) -> Result<T> + Send + 'static,
) -> Result<T> {
    let measured = record.clone();
    worker(state, record, move |application| {
        let started = Instant::now();
        let (result, waits) = crate::diagnostics::waits::capture(|| action(application));
        measured.execution(started.elapsed(), waits);
        result
    })
    .await
}

fn error_response(record: &trace::Record, error: ApplicationError) -> Response {
    match error {
        ApplicationError::InvalidRequest(message) => {
            let status = if message == "The application is shutting down." {
                StatusCode::SERVICE_UNAVAILABLE
            } else {
                StatusCode::BAD_REQUEST
            };
            rpc_failure(record, status, &message)
        }
        ApplicationError::Internal(message) => {
            rpc_failure(record, StatusCode::INTERNAL_SERVER_ERROR, &message)
        }
    }
}

fn rpc_failure(record: &trace::Record, status: StatusCode, message: &str) -> Response {
    let started = Instant::now();
    let response = failure(status, message);
    record.encode(started.elapsed());
    response
}

fn failure(status: StatusCode, message: &str) -> Response {
    (
        status,
        [(header::CONTENT_TYPE, "application/json")],
        serde_json::json!({"error": message}).to_string(),
    )
        .into_response()
}
