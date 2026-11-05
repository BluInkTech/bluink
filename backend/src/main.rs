// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! The Bluink executable: the local server, its console and one-shot
//! project commands.

// Keep internal visibility explicit even though the containing modules are private.
#![allow(
    clippy::redundant_pub_crate,
    reason = "Keep internal visibility explicit in private modules."
)]
mod allocator;
mod application;
mod catalogue;
mod console;
#[cfg(test)]
mod contracts;
mod diagnostics;
mod domain;
mod export_jobs;
mod host;
mod http;
mod records;
mod store;
mod structured_query;
mod versioning;

fn main() -> std::process::ExitCode {
    host::run()
}
