// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! Multipart is transport policy; canonical asset integrity belongs to core.
use axum::{
    Extension,
    extract::{FromRequest, Multipart, Request, State},
    http::{Method, StatusCode},
    response::Response,
};
use serde::Serialize;

use super::{HttpState, blocking, error_response, failure, trace};
use crate::application::error::{Result, invalid_request};

const LIMIT: usize = 10 * 1024 * 1024;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(test, derive(ts_rs::TS))]
pub(crate) struct Uploaded {
    src: String,
    filename: String,
    content_type: String,
    size: usize,
}

pub(super) async fn serve(
    State(state): State<HttpState>,
    Extension(record): Extension<trace::Record>,
    request: Request,
) -> Response {
    if request.method() != Method::POST {
        return failure(StatusCode::METHOD_NOT_ALLOWED, "ERR_RPC_INVALID_METHOD");
    }
    // Buffer only this bounded request before multipart parsing. This preserves
    // the total-body limit, including field headers and trailing multipart data,
    // instead of applying a per-file limit or trusting Content-Length.
    let (parts, body) = request.into_parts();
    let reading = record.reading();
    let bytes = axum::body::to_bytes(body, LIMIT).await;
    drop(reading);
    let bytes = match bytes {
        Ok(bytes) => bytes,
        Err(error) => {
            use std::error::Error;
            let message = if error.source().is_some_and(
                <dyn std::error::Error + 'static>::is::<http_body_util::LengthLimitError>,
            ) {
                "Image upload is larger than 10 MB."
            } else {
                "ERR_INVALID_UPLOAD"
            };
            return failure(StatusCode::BAD_REQUEST, message);
        }
    };
    let mut request = Request::from_parts(parts, axum::body::Body::from(bytes));
    axum::extract::DefaultBodyLimit::max(LIMIT).apply(&mut request);
    let Ok(multipart) = Multipart::from_request(request, &state).await else {
        return failure(StatusCode::BAD_REQUEST, "Expected multipart image upload.");
    };
    let parsed = parse(multipart).await;
    let (id, data, content_type, extension) = match parsed {
        Ok(parsed) => parsed,
        Err(error) => return error_response(&record, error),
    };
    let result = blocking(&state, record.clone(), move |application| {
        let _operation = application.begin_operation()?;
        let filename = format!("{}.{}", uuid::Uuid::new_v4(), extension);
        let src = crate::engine::project::asset::write_asset(
            application.project_root(),
            &id,
            &filename,
            &data,
        )?;
        Ok(Uploaded {
            src,
            filename,
            content_type,
            size: data.len(),
        })
    })
    .await;
    match result {
        Ok(value) => {
            let started = std::time::Instant::now();
            let encoded =
                serde_json::to_vec(&value).map_err(crate::application::error::internal_error);
            record.encode(started.elapsed());
            match encoded {
                Ok(bytes) => (
                    [(axum::http::header::CONTENT_TYPE, "application/json")],
                    bytes,
                )
                    .into_response(),
                Err(error) => error_response(&record, error),
            }
        }
        Err(error) => error_response(&record, error),
    }
}

use axum::response::IntoResponse;

async fn parse(
    mut multipart: Multipart,
) -> Result<(String, axum::body::Bytes, String, &'static str)> {
    let mut id = None;
    let mut image = None;
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|_| invalid_request("ERR_INVALID_UPLOAD"))?
    {
        match field.name() {
            Some("itemId") if id.is_none() => {
                id = Some(
                    field
                        .text()
                        .await
                        .map_err(|_| invalid_request("ERR_INVALID_UPLOAD"))?,
                );
            }
            Some("file") if image.is_none() => {
                let content_type = field
                    .content_type()
                    .unwrap_or("application/octet-stream")
                    .to_owned();
                let filename = field.file_name().unwrap_or("").to_owned();
                let extension = extension(&content_type, &filename)
                    .ok_or_else(|| invalid_request("Unsupported image type."))?;
                let bytes = field
                    .bytes()
                    .await
                    .map_err(|_| invalid_request("ERR_INVALID_UPLOAD"))?;
                image = Some((bytes, content_type, extension));
            }
            Some("itemId" | "file") => return Err(invalid_request("ERR_INVALID_UPLOAD")),
            _ => {
                field
                    .bytes()
                    .await
                    .map_err(|_| invalid_request("ERR_INVALID_UPLOAD"))?;
            }
        }
    }
    let (bytes, content_type, extension) = image
        .filter(|(bytes, _, _)| !bytes.is_empty())
        .ok_or_else(|| invalid_request("Missing image file."))?;
    let id = id
        .filter(|id| !id.is_empty())
        .ok_or_else(|| invalid_request("Missing item ID."))?;
    Ok((id, bytes, content_type, extension))
}

fn extension(mime: &str, filename: &str) -> Option<&'static str> {
    match mime.to_ascii_lowercase().as_str() {
        "image/png" => Some("png"),
        "image/jpeg" => Some("jpg"),
        "image/gif" => Some("gif"),
        "image/webp" => Some("webp"),
        // A raster filename must not authorize an explicitly declared SVG.
        "image/svg+xml" => None,
        _ => match filename.rsplit('.').next()?.to_ascii_lowercase().as_str() {
            "png" => Some("png"),
            "jpg" | "jpeg" => Some("jpg"),
            "gif" => Some("gif"),
            "webp" => Some("webp"),
            _ => None,
        },
    }
}
