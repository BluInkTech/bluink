// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

pub(crate) fn for_path(path: &std::path::Path) -> Option<&'static str> {
    Some(
        match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
            "ico" => "image/x-icon",
            "html" => "text/html; charset=utf-8",
            "js" => "text/javascript; charset=utf-8",
            "json" => "application/json; charset=utf-8",
            "xml" => "application/xml; charset=utf-8",
            "css" => "text/css; charset=utf-8",
            "txt" => "text/plain; charset=utf-8",
            "webmanifest" => "application/manifest+json",
            "wasm" => "application/wasm",
            "png" => "image/png",
            "jpg" | "jpeg" => "image/jpeg",
            "webp" => "image/webp",
            "gif" => "image/gif",
            "avif" => "image/avif",
            "svg" => "image/svg+xml",
            "pdf" => "application/pdf",
            "epub" => "application/epub+zip",
            "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
            "zip" => "application/zip",
            "ttf" => "font/ttf",
            "otf" => "font/otf",
            "woff" => "font/woff",
            "woff2" => "font/woff2",
            _ => return None,
        },
    )
}
