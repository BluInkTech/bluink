// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use axum::{
    Extension,
    extract::{Request, State},
    http::{HeaderValue, Method, StatusCode, header},
    response::Response,
};

use super::{HttpState, blocking, error_response, failure, files, trace};

const HTML_POLICY: &str = "sandbox; default-src 'none'; img-src data:; style-src 'unsafe-inline'; font-src 'self' data:; base-uri 'none'; form-action 'none'; frame-ancestors 'self'";
const SITE_POLICY: &str = "sandbox allow-scripts; default-src 'none'; img-src 'self' data:; script-src 'self'; style-src 'unsafe-inline'; font-src 'self' data:; base-uri 'none'; form-action 'none'; frame-ancestors 'self'";

fn disposition(path: &std::path::Path, download: bool) -> Option<HeaderValue> {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("export");
    let attachment = download
        || path.extension().is_some_and(|extension| {
            extension.eq_ignore_ascii_case("docx")
                || extension.eq_ignore_ascii_case("epub")
                || extension.eq_ignore_ascii_case("zip")
        });
    HeaderValue::from_str(&format!(
        "{}; filename=\"{name}\"",
        if attachment { "attachment" } else { "inline" }
    ))
    .ok()
}

pub(super) async fn serve(
    State(state): State<HttpState>,
    Extension(record): Extension<trace::Record>,
    request: Request,
) -> Response {
    if request.method() != Method::GET {
        return failure(StatusCode::METHOD_NOT_ALLOWED, "ERR_RPC_INVALID_METHOD");
    }
    let Some(source) = request.uri().path().strip_prefix("/api/export_artifact/") else {
        return failure(StatusCode::NOT_FOUND, "ERR_NOT_FOUND");
    };
    let (id, path) = source.split_once('/').unwrap_or((source, ""));
    if !crate::engine::project::identity::is_item_id(id) {
        return failure(StatusCode::NOT_FOUND, "ERR_NOT_FOUND");
    }
    let id = id.to_owned();
    let path = path.to_owned();
    let download = request
        .uri()
        .query()
        .is_some_and(|query| query.split('&').any(|pair| pair == "download=1"));
    let result = blocking(&state, record.clone(), move |application| {
        if path == "site.zip" {
            application.export_site_zip(&id)
        } else {
            application.open_export_artifact(&id, &path)
        }
    })
    .await;
    match result {
        Ok(Some(artifact)) => {
            let Some(disposition) = disposition(&artifact.path, download) else {
                return failure(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "The request could not be completed.",
                );
            };
            let policy =
                artifact
                    .content_type
                    .starts_with("text/html")
                    .then_some(if artifact.site {
                        SITE_POLICY
                    } else {
                        HTML_POLICY
                    });
            let sandbox_font = artifact.site && artifact.content_type.starts_with("font/");
            match files::stream(artifact.file, artifact.content_type, artifact.lease) {
                Ok(mut response) => {
                    response
                        .headers_mut()
                        .insert(header::CONTENT_DISPOSITION, disposition);
                    response
                        .headers_mut()
                        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
                    response.headers_mut().insert(
                        "cross-origin-resource-policy",
                        HeaderValue::from_static("cross-origin"),
                    );
                    // A sandboxed site preview has an opaque origin. Font
                    // requests therefore need CORS even on the app's own host.
                    if sandbox_font {
                        response.headers_mut().insert(
                            header::ACCESS_CONTROL_ALLOW_ORIGIN,
                            HeaderValue::from_static("*"),
                        );
                    }
                    if let Some(policy) = policy {
                        response.headers_mut().insert(
                            header::CONTENT_SECURITY_POLICY,
                            HeaderValue::from_static(policy),
                        );
                    }
                    response
                }
                Err(error) => error_response(&record, error),
            }
        }
        Ok(None) => failure(StatusCode::NOT_FOUND, "ERR_NOT_FOUND"),
        Err(error) => error_response(&record, error),
    }
}
