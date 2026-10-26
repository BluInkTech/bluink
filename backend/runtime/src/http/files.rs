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
    file_decoded(root, Path::new(decoded.trim_start_matches('/')))
}

fn file_decoded(root: &Path, relative: &Path) -> Option<PathBuf> {
    if std::fs::symlink_metadata(root)
        .ok()?
        .file_type()
        .is_symlink()
    {
        return None;
    }
    let mut path = root.to_owned();
    for component in relative.components() {
        let Component::Normal(part) = component else {
            return None;
        };
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
    crate::engine::media_type::for_path(path)
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

/// A copy of an image asset at a listed width, made on Tokio's blocking pool
/// when missing. `None` serves the original.
async fn sized(
    record: &trace::Record,
    (width, owner, cache, original): (u32, String, PathBuf, PathBuf),
) -> Option<(File, &'static str)> {
    let started = std::time::Instant::now();
    let made = super::sized_images::prepare(original, cache, owner, width).await;
    if let Err(error) = &made {
        tracing::warn!(operation = "image_copy", %error,
            "Image copy could not be made; serving the original");
    }
    let made = made.ok().flatten();
    let copy = made?;
    if copy.made {
        record.image_copy(started.elapsed());
    }
    File::open(copy.path).ok().map(|file| (file, copy.mime))
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
    // `?w=` asks for an image asset at one of a few widths.
    let width = super::sized_images::requested_width(request.uri().query());
    let result = blocking(&state, record.clone(), move |application| {
        let _operation = application.begin_operation()?;
        let parts: Vec<_> = decoded.split('/').filter(|part| !part.is_empty()).collect();
        let options = &application.options;
        let mut owner = None;
        let (root, relative) = match parts.as_slice() {
            ["assets", id, name] if crate::engine::project::identity::is_item_id(id) => {
                let identity = crate::engine::project::identity::ItemId::try_from(*id)
                    .map_err(crate::application::error::invalid_request)?;
                owner = Some((*id).to_owned());
                (
                    Some(application.project_root().to_owned()),
                    identity.directory(Path::new("")).join("assets").join(name),
                )
            }
            ["fonts", rest @ ..] => (
                options.web_font_folder.as_ref().map(PathBuf::from),
                PathBuf::from(rest.join("/")),
            ),
            ["data", rest @ ..] => (
                options.data_dir.as_ref().map(PathBuf::from),
                PathBuf::from(rest.join("/")),
            ),
            _ => (
                options.public_dir.as_ref().map(PathBuf::from),
                PathBuf::from(decoded.trim_start_matches('/')),
            ),
        };
        // The path is already decoded. Do not decode it twice (e.g. %252e).
        let Some(path) = root.and_then(|root| file_decoded(&root, &relative)) else {
            return Ok(None);
        };
        let Some(mime) = mime(&path) else {
            return Ok(None);
        };
        // Sizing happens below, off the request workers that every RPC shares.
        let sizing = width
            .zip(owner)
            .zip(application.image_cache())
            .map(|((width, owner), cache)| (width, owner, cache, path.clone()));
        let Ok(file) = File::open(path) else {
            return Ok(None);
        };
        Ok(Some((file, mime, sizing)))
    })
    .await;
    match result {
        Ok(Some((file, mime, sizing))) => {
            let (file, mime) = match sizing {
                Some(sizing) => sized(&record, sizing).await.unwrap_or((file, mime)),
                None => (file, mime),
            };
            stream(file, mime, ()).unwrap_or_else(|error| error_response(&record, error))
        }
        Ok(None) => failure(StatusCode::NOT_FOUND, "ERR_NOT_FOUND"),
        Err(error) => error_response(&record, error),
    }
}
