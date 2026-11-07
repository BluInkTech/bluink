// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! The Node addon. Node only loads Bluink and waits: `run` starts the same host
//! the executable runs on a thread of its own, and `shutdown` forwards the
//! signals Node owns. No application logic lives here.

use napi::bindgen_prelude::Object;
use napi::{Env, Error, Result, Status};
use napi_derive::napi;

#[napi(object)]
pub struct RunOptions {
    /// Command-line arguments, without the program name.
    pub argv: Vec<String>,
    /// The installed `bluink` package the addon was loaded for.
    pub package_root: Option<String>,
    /// The launcher script Node ran, which a PDF renderer child runs again.
    pub launcher: Option<String>,
}

/// Starts Bluink and resolves with its exit code once it finishes.
#[napi]
pub fn run(env: &Env, options: RunOptions) -> Result<Object<'_>> {
    let (deferred, promise) = env.create_deferred()?;
    std::thread::Builder::new()
        .name("bluink".to_owned())
        .spawn(move || {
            let code = bluink::run_embedded(bluink::Launch {
                arguments: options.argv,
                package_root: options.package_root.map(Into::into),
                launcher: options.launcher.map(Into::into),
            });
            deferred.resolve(move |_| Ok(u32::from(code)));
        })
        .map_err(|error| Error::new(Status::GenericFailure, error.to_string()))?;
    Ok(promise)
}

/// Requests the graceful stop that Ctrl+C requests in the executable.
#[napi]
pub fn shutdown() {
    bluink::request_shutdown();
}
