//! Windows system facts, from the Win32 counters.
//!
//! Windows has no `/proc`, so the same numbers come from `kernel32`/`psapi`.
//! The environment already carries the CPU description and the logical
//! processor count, so only memory, uptime and the process counters need a
//! call.

use super::gib;
use windows_sys::Win32::Foundation::FILETIME;
use windows_sys::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
use windows_sys::Win32::System::SystemInformation::{
    GetTickCount64, GlobalMemoryStatusEx, MEMORYSTATUSEX,
};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};

/// The machine's hostname.
pub(crate) fn hostname() -> String {
    std::env::var("COMPUTERNAME").unwrap_or_else(|_| "localhost".to_owned())
}

/// `Windows_NT (x86_64)`.
///
/// The build number lives in the registry, which is more machinery than a
/// one-line summary needs.
pub(crate) fn os_line() -> String {
    let flavour = std::env::var("OS").unwrap_or_else(|_| "Windows".to_owned());
    format!("{flavour} ({})", std::env::consts::ARCH)
}

/// `AMD64 Family 25 Model 116 Stepping 1, AuthenticAMD (12 threads)`.
pub(crate) fn cpu_line() -> String {
    let model = std::env::var("PROCESSOR_IDENTIFIER").unwrap_or_else(|_| "unknown CPU".to_owned());
    let threads = std::env::var("NUMBER_OF_PROCESSORS")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(1);
    format!("{model} ({threads} threads)")
}

/// Physical memory status, from `GlobalMemoryStatusEx`.
fn memory_status() -> Option<MEMORYSTATUSEX> {
    // SAFETY: the struct is zeroed and `dwLength` is set to its own size, which
    // is what the API requires.
    let mut status: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
    status.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
    let ok = unsafe { GlobalMemoryStatusEx(&mut status) };
    (ok != 0).then_some(status)
}

/// Total physical memory in KiB.
pub(crate) fn total_memory_kib() -> Option<u64> {
    Some(memory_status()?.ullTotalPhys / 1024)
}

/// `1.7 GiB / 15.6 GiB`, from `GlobalMemoryStatusEx`.
pub(crate) fn memory_line() -> String {
    let Some(status) = memory_status() else {
        return "unknown".to_owned();
    };
    let total = status.ullTotalPhys / 1024;
    let available = status.ullAvailPhys / 1024;
    format!("{} / {}", gib(total - available), gib(total))
}

/// `3h 12m`, from `GetTickCount64` (milliseconds since boot).
pub(crate) fn uptime_line() -> String {
    // SAFETY: no arguments, no failure mode.
    let seconds = unsafe { GetTickCount64() } / 1000;
    let (h, m) = (seconds / 3600, (seconds % 3600) / 60);
    if h > 0 {
        format!("{h}h {m}m")
    } else {
        format!("{m}m")
    }
}

/// The process's working set in KiB, from `GetProcessMemoryInfo`.
pub(crate) fn process_rss_kib() -> Option<u64> {
    // SAFETY: the struct is zeroed and its size is passed to the API.
    let mut counters: PROCESS_MEMORY_COUNTERS = unsafe { std::mem::zeroed() };
    let ok = unsafe {
        GetProcessMemoryInfo(
            GetCurrentProcess(),
            &mut counters,
            std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32,
        )
    };
    (ok != 0).then_some(counters.WorkingSetSize as u64 / 1024)
}

/// Threads in this process.
///
/// A count needs a Toolhelp32 snapshot; the CPU line reads fine without it.
pub(crate) fn process_threads() -> Option<u64> {
    None
}

/// CPU time this process has used, in clock ticks (kernel + user).
pub(crate) fn process_cpu_ticks() -> Option<u64> {
    // SAFETY: all four out-parameters are zeroed FILETIMEs.
    let (mut creation, mut exit, mut kernel, mut user): (FILETIME, FILETIME, FILETIME, FILETIME) =
        unsafe { std::mem::zeroed() };
    let ok = unsafe {
        GetProcessTimes(
            GetCurrentProcess(),
            &mut creation,
            &mut exit,
            &mut kernel,
            &mut user,
        )
    };
    if ok == 0 {
        return None;
    }
    // FILETIME counts 100 ns units; one USER_HZ tick is 10 ms, so 100_000 units.
    let ticks =
        |ft: FILETIME| (((ft.dwHighDateTime as u64) << 32) | ft.dwLowDateTime as u64) / 100_000;
    Some(ticks(kernel) + ticks(user))
}
