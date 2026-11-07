// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! The `bluink` executable.

// Every dependency belongs to the library; the executable only calls it.
#![allow(
    unused_crate_dependencies,
    reason = "The executable delegates to the library, which uses the dependencies."
)]

fn main() -> std::process::ExitCode {
    bluink::run()
}
