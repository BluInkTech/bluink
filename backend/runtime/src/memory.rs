// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! Return unused allocator pages after bursts of native work.

#[cfg(all(target_os = "linux", target_env = "gnu"))]
pub(crate) fn release_unused() {
    // SAFETY: glibc documents malloc_trim as MT-Safe. It accepts a size, has no
    // pointer preconditions, and releases only unallocated whole heap pages.
    // This deliberately excludes other platforms and allocators. The exception
    // is confined to this one platform call; the crate still denies unsafe code.
    #[allow(
        unsafe_code,
        reason = "The audited glibc malloc_trim call has no pointer preconditions and is MT-Safe."
    )]
    unsafe {
        libc::malloc_trim(0);
    }
}

#[cfg(not(all(target_os = "linux", target_env = "gnu")))]
pub(crate) const fn release_unused() {}
