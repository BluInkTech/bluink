// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use axum::http::{HeaderName, HeaderValue, Method, StatusCode, header, uri::Authority};
use axum::{extract::Request, middleware::Next, response::Response};

/// The listener binds loopback only, but a page elsewhere can still reach it:
/// a rebound DNS name makes that page same-origin unless Host is checked, and a
/// form or no-cors fetch sends a simple POST without any CORS preflight. Only
/// loopback names may read, and only loopback pages may change state.
pub(super) async fn loopback(request: Request, next: Next) -> Response {
    let headers = request.headers();
    let host = headers
        .get(header::HOST)
        .is_none_or(|value| value.to_str().is_ok_and(loopback_authority));
    let safe = matches!(*request.method(), Method::GET | Method::HEAD);
    // Browsers send Origin with every POST; other local clients may omit it.
    let origin = safe
        || headers.get(header::ORIGIN).is_none_or(|value| {
            value.to_str().is_ok_and(|origin| {
                origin
                    .strip_prefix("http://")
                    .or_else(|| origin.strip_prefix("https://"))
                    .is_some_and(loopback_authority)
            })
        });
    if host && origin {
        next.run(request).await
    } else {
        super::failure(StatusCode::FORBIDDEN, "ERR_FORBIDDEN_ORIGIN")
    }
}

fn loopback_authority(authority: &str) -> bool {
    authority.parse::<Authority>().is_ok_and(|authority| {
        let host = authority.host().to_ascii_lowercase();
        host == "localhost"
            || host.ends_with(".localhost")
            || host == "127.0.0.1"
            || host == "[::1]"
    })
}

pub(super) async fn security(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    for (name, value) in [
        (
            "content-security-policy",
            // Harper starts its packaged WebAssembly in a local blob worker.
            // Worker permission stays separate from the document's JS policy.
            "default-src 'self';base-uri 'self';font-src 'self' https: data:;form-action 'self';frame-ancestors 'self';img-src 'self' data:;object-src 'none';script-src 'self' 'wasm-unsafe-eval';script-src-attr 'none';worker-src 'self' blob:;style-src 'self' https: 'unsafe-inline';upgrade-insecure-requests",
        ),
        ("cross-origin-opener-policy", "same-origin"),
        ("cross-origin-resource-policy", "same-origin"),
        ("origin-agent-cluster", "?1"),
        ("referrer-policy", "no-referrer"),
        (
            "strict-transport-security",
            "max-age=15552000; includeSubDomains",
        ),
        ("x-content-type-options", "nosniff"),
        ("x-dns-prefetch-control", "off"),
        ("x-download-options", "noopen"),
        ("x-frame-options", "SAMEORIGIN"),
        ("x-permitted-cross-domain-policies", "none"),
    ] {
        response
            .headers_mut()
            .entry(HeaderName::from_static(name))
            .or_insert(HeaderValue::from_static(value));
    }
    response
}
