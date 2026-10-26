// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use axum::http::{HeaderName, HeaderValue};
use axum::{extract::Request, middleware::Next, response::Response};

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
