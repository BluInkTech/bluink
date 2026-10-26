// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! Bounded concurrency for filesystem reads.
pub(crate) fn files<T: Send>(work: impl FnOnce() -> Result<T, String> + Send) -> Result<T, String> {
    with_workers(2, "bluink-files", work)
}

/// Metadata probes are independent I/O operations; four workers keep scans
/// responsive without changing how any file stamp is obtained.
pub(crate) fn metadata<T: Send>(
    work: impl FnOnce() -> Result<T, String> + Send,
) -> Result<T, String> {
    with_workers(4, "bluink-metadata", work)
}

fn with_workers<T: Send>(
    limit: usize,
    name: &'static str,
    work: impl FnOnce() -> Result<T, String> + Send,
) -> Result<T, String> {
    // File batches are finite. Let their threads exit afterwards so allocator
    // thread caches from a large projection cannot remain alive while idle.
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(std::thread::available_parallelism().map_or(1, |count| count.get().min(limit)))
        .thread_name(move |index| format!("{name}-{index}"))
        .build()
        .map_err(|error| error.to_string())?;
    pool.install(work)
}
