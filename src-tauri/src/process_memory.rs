//! Process memory readout for the desktop diagnostics.
//!
//! The library accounting in dump_memory_stats tells us what the native backend
//! holds; the user's "3+ GB in Task Manager" also includes the WebView2/JS heap
//! and the OS allocation overhead, which the backend cannot see. This module
//! reports the *process* working set (current and peak) so QA and the user can
//! tell which side a peak comes from:
//!
//! * Windows: GetProcessMemoryInfo / PROCESS_MEMORY_COUNTERS (WorkingSetSize,
//!   PeakWorkingSetSize) through kernel32, declared here by hand so the app
//!   crate gains no new dependency (and Cargo.lock stays untouched).
//! * Linux: /proc/self/status (VmRSS / VmHWM).
//! * Everything else: both fields are None.
//!
//! The values are bytes; None means "the platform could not report it".

use serde::{Deserialize, Serialize};

/// Resident working set of the app process, in bytes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessMemory {
    /// Working set right now (None when the platform cannot report it).
    pub current_bytes: Option<u64>,
    /// Peak working set since the process started (None when unsupported).
    pub peak_bytes: Option<u64>,
}

/// Read the current process memory counters.
pub fn process_memory() -> ProcessMemory {
    imp::read()
}

#[cfg(windows)]
mod imp {
    use super::ProcessMemory;

    /// PROCESS_MEMORY_COUNTERS from psapi.h (the plain, not the _EX variant).
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct ProcessMemoryCounters {
        cb: u32,
        page_fault_count: u32,
        peak_working_set_size: usize,
        working_set_size: usize,
        quota_peak_paged_pool_usage: usize,
        quota_paged_pool_usage: usize,
        quota_peak_non_paged_pool_usage: usize,
        quota_non_paged_pool_usage: usize,
        pagefile_usage: usize,
        peak_pagefile_usage: usize,
    }

    impl ProcessMemoryCounters {
        fn zeroed() -> Self {
            // every field is a plain integer; all-zero is valid for this struct
            // (cb is set by the caller)
            Self {
                cb: 0,
                page_fault_count: 0,
                peak_working_set_size: 0,
                working_set_size: 0,
                quota_peak_paged_pool_usage: 0,
                quota_paged_pool_usage: 0,
                quota_peak_non_paged_pool_usage: 0,
                quota_non_paged_pool_usage: 0,
                pagefile_usage: 0,
                peak_pagefile_usage: 0,
            }
        }
    }

    // kernel32 exports both on Windows 7+ (K32GetProcessMemoryInfo is the
    // documented forwarder; declaring them here keeps the app dependency-free)
    #[link(name = "kernel32")]
    extern "system" {
        fn GetCurrentProcess() -> *mut core::ffi::c_void;
        fn K32GetProcessMemoryInfo(
            process: *mut core::ffi::c_void,
            counters: *mut ProcessMemoryCounters,
            cb: u32,
        ) -> i32;
    }

    pub fn read() -> ProcessMemory {
        let mut counters = ProcessMemoryCounters::zeroed();
        counters.cb = core::mem::size_of::<ProcessMemoryCounters>() as u32;
        // SAFETY: the handle is the pseudo handle for the current process and
        // counters points at a live, correctly sized PROCESS_MEMORY_COUNTERS.
        let ok =
            unsafe { K32GetProcessMemoryInfo(GetCurrentProcess(), &mut counters, counters.cb) };
        if ok == 0 {
            return ProcessMemory::default();
        }
        ProcessMemory {
            current_bytes: Some(counters.working_set_size as u64),
            peak_bytes: Some(counters.peak_working_set_size as u64),
        }
    }
}

#[cfg(target_os = "linux")]
mod imp {
    use super::ProcessMemory;

    /// Read a kB value out of /proc/self/status (VmRSS / VmHWM).
    fn status_bytes(field: &str) -> Option<u64> {
        let status = std::fs::read_to_string("/proc/self/status").ok()?;
        let line = status.lines().find(|line| line.starts_with(field))?;
        let kb: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
        Some(kb * 1024)
    }

    pub fn read() -> ProcessMemory {
        ProcessMemory {
            current_bytes: status_bytes("VmRSS:"),
            peak_bytes: status_bytes("VmHWM:"),
        }
    }
}

#[cfg(not(any(windows, target_os = "linux")))]
mod imp {
    use super::ProcessMemory;

    pub fn read() -> ProcessMemory {
        ProcessMemory::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn process_memory_reports_a_positive_working_set() {
        let memory = process_memory();
        #[cfg(any(windows, target_os = "linux"))]
        {
            let current = memory.current_bytes.expect("a current working set");
            assert!(current > 0, "working set is non-zero");
            let peak = memory.peak_bytes.expect("a peak working set");
            assert!(peak >= current, "peak {peak} < current {current}");
            eprintln!(
                "process memory: current {} bytes, peak {} bytes",
                current, peak
            );
        }
        #[cfg(not(any(windows, target_os = "linux")))]
        {
            assert_eq!(memory.current_bytes, None);
            assert_eq!(memory.peak_bytes, None);
        }
    }
}
