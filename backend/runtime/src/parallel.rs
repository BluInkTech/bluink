// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! Bounded concurrency for filesystem reads, independent of CPU count.
pub(crate) fn files<T: Send>(work: impl FnOnce() -> Result<T, String> + Send) -> Result<T, String> {
    // File batches are finite. Let their threads exit afterwards so allocator
    // thread caches from a large projection cannot remain alive while idle.
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(std::thread::available_parallelism().map_or(1, |count| count.get().min(2)))
        .thread_name(|index| format!("bluink-files-{index}"))
        .build()
        .map_err(|error| error.to_string())?;
    pool.install(work)
}
