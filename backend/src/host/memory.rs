// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! Resident memory for the console status line. `None` when unavailable.

#[cfg(windows)]
pub(super) fn resident_bytes() -> Option<u64> {
    use windows_sys::Win32::System::ProcessStatus::{
        GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS,
    };
    use windows_sys::Win32::System::Threading::GetCurrentProcess;
    let size = u32::try_from(std::mem::size_of::<PROCESS_MEMORY_COUNTERS>()).ok()?;
    let mut counters = PROCESS_MEMORY_COUNTERS {
        cb: size,
        PageFaultCount: 0,
        PeakWorkingSetSize: 0,
        WorkingSetSize: 0,
        QuotaPeakPagedPoolUsage: 0,
        QuotaPagedPoolUsage: 0,
        QuotaPeakNonPagedPoolUsage: 0,
        QuotaNonPagedPoolUsage: 0,
        PagefileUsage: 0,
        PeakPagefileUsage: 0,
    };
    #[expect(
        unsafe_code,
        reason = "GetCurrentProcess returns a constant pseudo-handle and has no preconditions."
    )]
    // SAFETY: The pseudo-handle needs no closing and is valid for this process.
    let process = unsafe { GetCurrentProcess() };
    #[expect(
        unsafe_code,
        reason = "The Windows query writes exactly `cb` bytes into an owned, initialized structure."
    )]
    // SAFETY: The output structure lives through the call with its true size in `cb`.
    let succeeded = unsafe { GetProcessMemoryInfo(process, &raw mut counters, size) };
    (succeeded != 0).then_some(counters.WorkingSetSize as u64)
}

#[cfg(target_os = "linux")]
pub(super) fn resident_bytes() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let kilobytes = status
        .lines()
        .find_map(|line| line.strip_prefix("VmRSS:"))?
        .trim()
        .strip_suffix("kB")?
        .trim()
        .parse::<u64>()
        .ok()?;
    kilobytes.checked_mul(1024)
}

#[cfg(target_os = "macos")]
pub(super) fn resident_bytes() -> Option<u64> {
    let mut information = std::mem::MaybeUninit::<libc::proc_taskinfo>::uninit();
    let size = i32::try_from(std::mem::size_of::<libc::proc_taskinfo>()).ok()?;
    let process = i32::try_from(std::process::id()).ok()?;
    #[expect(
        unsafe_code,
        reason = "proc_pidinfo writes at most `size` bytes into owned storage for this process."
    )]
    // SAFETY: The buffer is exactly one proc_taskinfo and outlives the call;
    // the kernel reports how many bytes it wrote.
    let written = unsafe {
        libc::proc_pidinfo(
            process,
            libc::PROC_PIDTASKINFO,
            0,
            information.as_mut_ptr().cast(),
            size,
        )
    };
    if written != size {
        return None;
    }
    #[expect(
        unsafe_code,
        reason = "The successful call above initialized the whole structure."
    )]
    // SAFETY: `written == size` proves the kernel filled the structure.
    let information = unsafe { information.assume_init() };
    Some(information.pti_resident_size)
}

#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
pub(super) const fn resident_bytes() -> Option<u64> {
    None
}

#[cfg(test)]
mod tests {
    #[test]
    fn this_process_reports_resident_memory() {
        assert!(super::resident_bytes().is_some_and(|bytes| bytes > 1_048_576));
    }
}
