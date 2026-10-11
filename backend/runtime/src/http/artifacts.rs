// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use axum::{
    Extension,
    body::Bytes,
    extract::{Request, State},
    http::{HeaderValue, Method, StatusCode, header},
    response::{IntoResponse, Response},
};

use super::{HttpState, blocking, error_response, failure, files, trace};

const HTML_POLICY: &str = "sandbox; default-src 'none'; img-src data:; style-src 'unsafe-inline'; font-src 'self' data:; base-uri 'none'; form-action 'none'; frame-ancestors 'self'";
const SITE_POLICY: &str = "sandbox allow-scripts; default-src 'none'; img-src 'self' data:; script-src 'self'; style-src 'unsafe-inline'; font-src 'self' data:; base-uri 'none'; form-action 'none'; frame-ancestors 'self'";

struct Bundle(std::sync::Arc<Vec<u8>>);
impl AsRef<[u8]> for Bundle {
    fn as_ref(&self) -> &[u8] {
        self.0.as_slice()
    }
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
    if !crate::engine::project::item::is_item_id(id) {
        return failure(StatusCode::NOT_FOUND, "ERR_NOT_FOUND");
    }
    let id = id.to_owned();
    let path = path.to_owned();
    let download = request
        .uri()
        .query()
        .is_some_and(|query| query.split('&').any(|pair| pair == "download=1"));
    if path == "site.zip" {
        let result = blocking(&state, record.clone(), move |application| {
            application.export_site_zip(&id)
        })
        .await;
        return match result {
            Ok(Some(bundle)) => {
                let bytes = Bytes::from_owner(Bundle(bundle));
                (
                    [
                        (header::CONTENT_TYPE, "application/zip"),
                        (
                            header::CONTENT_DISPOSITION,
                            "attachment; filename=\"bluink-site.zip\"",
                        ),
                        (header::CACHE_CONTROL, "no-store"),
                    ],
                    bytes,
                )
                    .into_response()
            }
            Ok(None) => failure(StatusCode::NOT_FOUND, "ERR_NOT_FOUND"),
            Err(error) => error_response(&record, error),
        };
    }
    let result = blocking(&state, record.clone(), move |application| {
        application.open_export_artifact(&id, &path)
    })
    .await;
    match result {
        Ok(Some(artifact)) => {
            let name = artifact
                .path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("export");
            let disposition = format!(
                "{}; filename=\"{name}\"",
                if download { "attachment" } else { "inline" }
            );
            let Ok(disposition) = HeaderValue::from_str(&disposition) else {
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
