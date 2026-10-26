// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

// Keep internal visibility explicit even though the containing modules are private.
#![allow(
    clippy::redundant_pub_crate,
    reason = "Keep internal visibility explicit in private modules."
)]
mod application;
mod calendar;
mod catalogue;
mod catalogue_usage;
mod console;
#[cfg(test)]
mod contracts;
mod diagnostics;
mod document_graph;
mod engine;
mod error;
mod export;
mod http;
mod indexing;
mod memory;
mod model;
mod parallel;
mod passages;
mod projection;
mod query;
mod relations;
mod reports;
mod search;
mod snippets;
mod tasks;

use napi_derive::napi;

// HTTP polling belongs on a small I/O pool; CPU work already has dedicated
// index workers and Rayon. Bound blocking work so request bursts cannot create
// hundreds of threads and their allocator arenas.
#[napi_derive::module_init]
#[expect(
    clippy::expect_used,
    reason = "N-API module initialization cannot return an error; failure to create the required runtime must stop initialization."
)]
fn initialize_async_runtime() {
    napi::bindgen_prelude::create_custom_tokio_runtime(
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .max_blocking_threads(2)
            .enable_all()
            .build()
            .expect("Create Bluink I/O runtime"),
    );
}

pub use application::{ApplicationOptions, BluinkApplication};
pub use console::{BluinkConsole, ConsoleAction, ConsoleOptions};
pub use diagnostics::{
    configure_native_diagnostics, flush_native_diagnostics, record_native_diagnostic,
    shutdown_native_diagnostics,
};

/// Embedded in every platform artifact so packaging can reject stale binaries
/// without having to execute a foreign architecture on the build machine.
///
/// The build mode distinguishes development addons from packageable releases.
#[napi]
#[must_use]
pub fn native_build_info() -> String {
    concat!(
        "BLUINK_NATIVE_BUILD_INFO_V1:",
        env!("BLUINK_NATIVE_BUILD_INFO"),
        ":END_BLUINK_NATIVE_BUILD_INFO"
    )
    .to_owned()
}
