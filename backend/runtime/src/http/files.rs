// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! Confined file lookup and streamed HTTP delivery, shared with site previews.
use std::fs::File;
use std::path::{Component, Path, PathBuf};

use axum::{
    Extension,
    body::Body,
    extract::{Request, State},
    http::{Method, StatusCode, header},
    response::{IntoResponse, Response},
};
use std::io::Read;

use super::{HttpState, blocking, error_response, failure, trace};
use crate::application::error::Result;

pub(crate) fn decoded(source: &str) -> Option<String> {
    let source = source.split('?').next()?;
    let bytes = source.as_bytes();
    for (index, byte) in bytes.iter().enumerate() {
        if *byte == b'%'
            && !bytes
                .get(index + 1..index + 3)
                .is_some_and(|pair| pair.iter().all(u8::is_ascii_hexdigit))
        {
            return None;
        }
    }
    let decoded = percent_encoding::percent_decode_str(source)
        .decode_utf8()
        .ok()?;
    if decoded.contains(['\\', '\0', ':']) {
        return None;
    }
    if decoded
        .split('/')
        .filter(|part| !part.is_empty())
        .any(|part| {
            let mut components = Path::new(part).components();
            !matches!(components.next(), Some(Component::Normal(_))) || components.next().is_some()
        })
    {
        return None;
    }
    Some(decoded.into_owned())
}

pub(crate) fn file(root: &Path, source: &str) -> Option<PathBuf> {
    let decoded = decoded(source)?;
    file_decoded(root, &decoded)
}

fn file_decoded(root: &Path, relative: &str) -> Option<PathBuf> {
    if std::fs::symlink_metadata(root)
        .ok()?
        .file_type()
        .is_symlink()
    {
        return None;
    }
    let mut path = root.to_owned();
    for part in relative.split('/').filter(|part| !part.is_empty()) {
        path.push(part);
        if std::fs::symlink_metadata(&path)
            .ok()?
            .file_type()
            .is_symlink()
        {
            return None;
        }
    }
    if std::fs::symlink_metadata(&path).ok()?.is_dir() {
        path.push("index.html");
    }
    let metadata = std::fs::symlink_metadata(&path).ok()?;
    (metadata.is_file() && !metadata.file_type().is_symlink()).then_some(path)
}

fn mime(path: &Path) -> Option<&'static str> {
    Some(match path.extension()?.to_str()? {
        "ico" => "image/x-icon",
        "html" => "text/html",
        "js" => "text/javascript",
        "json" => "application/json",
        "webmanifest" => "application/manifest+json",
        "wasm" => "application/wasm",
        "css" => "text/css",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        "pdf" => "application/pdf",
        "ttf" => "application/x-font-ttf",
        "woff2" => "font/woff2",
        "txt" => "text/plain",
        _ => return None,
    })
}

pub(crate) fn stream<G: Send + 'static>(
    file: File,
    mime: &'static str,
    guard: G,
) -> Result<Response> {
    static READS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(8);
    // The file closes before its lease, including when an interrupted response
    // leaves a blocking read in flight. A guard outside tokio::fs::File would
    // be dropped while Tokio's internal blocking read still owns that handle.
    struct OwnedFile<G> {
        file: File,
        _guard: G,
    }
    let owned = OwnedFile {
        file,
        _guard: guard,
    };
    let size = owned.file.metadata()?.len();
    let stream = futures_util::stream::try_unfold(owned, |mut owned| async move {
        let permit = READS.acquire().await.map_err(std::io::Error::other)?;
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let mut bytes = vec![0; 64 * 1024];
            let read = owned.file.read(&mut bytes)?;
            if read == 0 {
                return Ok::<_, std::io::Error>(None);
            }
            bytes.truncate(read);
            Ok(Some((axum::body::Bytes::from(bytes), owned)))
        })
        .await
        .map_err(std::io::Error::other)?
    });
    Ok((
        [
            (header::CONTENT_TYPE, mime),
            (header::CONTENT_LENGTH, &size.to_string()),
        ],
        Body::from_stream(stream),
    )
        .into_response())
}

pub(super) async fn serve(
    State(state): State<HttpState>,
    Extension(record): Extension<trace::Record>,
    request: Request,
) -> Response {
    if request.method() != Method::GET {
        return failure(StatusCode::NOT_FOUND, "ERR_NOT_FOUND");
    }
    let source = request.uri().path().to_owned();
    if source.starts_with("/api") {
        return failure(StatusCode::NOT_FOUND, "ERR_RPC_ENDPOINT_NOT_FOUND");
    }
    let Some(decoded) = decoded(&source) else {
        return failure(StatusCode::BAD_REQUEST, "ERR_NOT_FOUND");
    };
    let result = blocking(&state, record.clone(), move |application| {
        let _operation = application.begin_operation()?;
        let parts: Vec<_> = decoded.split('/').filter(|part| !part.is_empty()).collect();
        let options = &application.options;
        let (root, relative) = match parts.as_slice() {
            ["assets", id, name] if crate::engine::project::item::is_item_id(id) => (
                Some(application.project_root().to_owned()),
                format!("items/{}/{id}/assets/{name}", id[..2].to_ascii_lowercase()),
            ),
            ["fonts", rest @ ..] => (
                options.web_font_folder.as_ref().map(PathBuf::from),
                rest.join("/"),
            ),
            ["data", rest @ ..] => (options.data_dir.as_ref().map(PathBuf::from), rest.join("/")),
            _ => (
                options.public_dir.as_ref().map(PathBuf::from),
                decoded.trim_start_matches('/').to_owned(),
            ),
        };
        // The path is already decoded. Do not decode it twice (e.g. %252e).
        let Some(path) = root.and_then(|root| file_decoded(&root, &relative)) else {
            return Ok(None);
        };
        let Some(mime) = mime(&path) else {
            return Ok(None);
        };
        let Ok(file) = File::open(path) else {
            return Ok(None);
        };
        Ok(Some((file, mime)))
    })
    .await;
    match result {
        Ok(Some((file, mime))) => {
            stream(file, mime, ()).unwrap_or_else(|error| error_response(&record, error))
        }
        Ok(None) => failure(StatusCode::NOT_FOUND, "ERR_NOT_FOUND"),
        Err(error) => error_response(&record, error),
    }
}
