//! Windows process inspection: child process enumeration and shell working directory.

use std::collections::HashMap;
use std::ffi::c_void;
use std::path::PathBuf;
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::System::Diagnostics::Debug::ReadProcessMemory;
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleA, GetProcAddress};
use windows_sys::Win32::System::Threading::{
    OpenProcess, PROCESS_QUERY_INFORMATION, PROCESS_VM_READ,
};

/// A snapshot of the process table: each pid's children and each pid's name.
fn process_table() -> (HashMap<u32, Vec<u32>>, HashMap<u32, String>) {
    let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
    let mut names: HashMap<u32, String> = HashMap::new();

    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snapshot == INVALID_HANDLE_VALUE {
            return (children, names);
        }

        let mut entry: PROCESSENTRY32W = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;

        if Process32FirstW(snapshot, &mut entry) != 0 {
            loop {
                let child_pid = entry.th32ProcessID;
                let parent_pid = entry.th32ParentProcessID;

                let len = entry
                    .szExeFile
                    .iter()
                    .position(|&c| c == 0)
                    .unwrap_or(entry.szExeFile.len());
                let exe_name = String::from_utf16_lossy(&entry.szExeFile[..len]);

                names.insert(child_pid, exe_name);
                children.entry(parent_pid).or_default().push(child_pid);

                if Process32NextW(snapshot, &mut entry) == 0 {
                    break;
                }
            }
        }
        CloseHandle(snapshot);
    }

    (children, names)
}

/// The pids running under `pid`, children and their descendants.
///
/// Unlike a name list, pids identify one process, so tests can assert that a
/// specific child was reaped without being confused by other processes that
/// share its executable name.
fn descendant_pids(children: &HashMap<u32, Vec<u32>>, pid: u32) -> Vec<u32> {
    let mut out = Vec::new();
    let mut stack = vec![pid];
    while let Some(p) = stack.pop() {
        let Some(kids) = children.get(&p) else {
            continue;
        };
        for &kid in kids {
            out.push(kid);
            stack.push(kid);
        }
    }
    out
}

/// The names of the processes still running under a shell, if any.
///
/// This provides the Windows equivalent of walking `/proc` on Linux:
/// a pane whose shell has living children (e.g. `npm`, `ssh.exe`, an editor, `cargo`)
/// is actively doing work, and closing the pane triggers a confirmation.
pub fn running_children(pid: u32) -> Vec<String> {
    let (children, names) = process_table();

    let mut out: Vec<String> = Vec::new();
    for kid in descendant_pids(&children, pid) {
        if let Some(name) = names.get(&kid)
            && !out.iter().any(|n| n == name)
        {
            out.push(name.clone());
        }
    }
    out
}

#[repr(C)]
struct ProcessBasicInformation {
    exit_status: i32,
    peb_base_address: *mut c_void,
    affinity_mask: usize,
    base_priority: i32,
    unique_process_id: usize,
    inherited_from_unique_process_id: usize,
}

#[repr(C)]
struct UnicodeString {
    length: u16,
    maximum_length: u16,
    buffer: *mut u16,
}

type FnNtQueryInformationProcess = unsafe extern "system" fn(
    process_handle: HANDLE,
    process_information_class: u32,
    process_information: *mut c_void,
    process_information_length: u32,
    return_length: *mut u32,
) -> i32;

/// The working directory of a shell process on Windows.
///
/// Queries the target process's PEB (Process Environment Block) to read
/// `RTL_USER_PROCESS_PARAMETERS.CurrentDirectory.DosPath`. This lets the editor's
/// file tree follow `cd` navigation inside terminal panes on Windows.
pub fn shell_cwd(pid: u32) -> Option<PathBuf> {
    unsafe {
        let ntdll = GetModuleHandleA(c"ntdll.dll".as_ptr().cast());
        if ntdll.is_null() {
            return None;
        }
        let nt_query_ptr = GetProcAddress(ntdll, c"NtQueryInformationProcess".as_ptr().cast());
        let nt_query: FnNtQueryInformationProcess = std::mem::transmute(nt_query_ptr?);

        let handle = OpenProcess(PROCESS_QUERY_INFORMATION | PROCESS_VM_READ, 0, pid);
        if handle.is_null() {
            return None;
        }

        let result = read_cwd_from_process(handle, nt_query);
        CloseHandle(handle);
        result
    }
}

unsafe fn read_cwd_from_process(
    handle: HANDLE,
    nt_query: FnNtQueryInformationProcess,
) -> Option<PathBuf> {
    // SAFETY: the handle is open for `PROCESS_QUERY_INFORMATION |
    // PROCESS_VM_READ`; every raw read is passed the exact size of its
    // destination, and the byte counts the API returns are checked before
    // the bytes are interpreted.
    unsafe {
        let mut pbi: ProcessBasicInformation = std::mem::zeroed();
        let mut return_len = 0u32;
        // ProcessBasicInformation = 0
        let status = nt_query(
            handle,
            0,
            &mut pbi as *mut _ as *mut c_void,
            std::mem::size_of::<ProcessBasicInformation>() as u32,
            &mut return_len,
        );
        if status != 0 || pbi.peb_base_address.is_null() {
            return None;
        }

        // In 64-bit Windows, ProcessParameters is at PEB + 0x20.
        // In 32-bit Windows, ProcessParameters is at PEB + 0x10.
        #[cfg(target_pointer_width = "64")]
        let process_params_offset = 0x20;
        #[cfg(target_pointer_width = "32")]
        let process_params_offset = 0x10;

        let mut process_params_ptr: *mut c_void = std::ptr::null_mut();
        let mut bytes_read = 0usize;
        let ok = ReadProcessMemory(
            handle,
            pbi.peb_base_address.add(process_params_offset),
            &mut process_params_ptr as *mut _ as *mut c_void,
            std::mem::size_of::<*mut c_void>(),
            &mut bytes_read,
        );
        if ok == 0 || process_params_ptr.is_null() {
            return None;
        }

        // In RTL_USER_PROCESS_PARAMETERS, CurrentDirectory.DosPath is at offset 0x38 on 64-bit.
        #[cfg(target_pointer_width = "64")]
        let current_dir_offset = 0x38;
        #[cfg(target_pointer_width = "32")]
        let current_dir_offset = 0x24;

        let mut dos_path: UnicodeString = std::mem::zeroed();
        let ok = ReadProcessMemory(
            handle,
            process_params_ptr.add(current_dir_offset),
            &mut dos_path as *mut _ as *mut c_void,
            std::mem::size_of::<UnicodeString>(),
            &mut bytes_read,
        );
        if ok == 0 || dos_path.buffer.is_null() || dos_path.length == 0 {
            return None;
        }

        let char_count = (dos_path.length / 2) as usize;
        if char_count == 0 || char_count > 4096 {
            return None;
        }

        let mut buf = vec![0u16; char_count];
        let ok = ReadProcessMemory(
            handle,
            dos_path.buffer as *const c_void,
            buf.as_mut_ptr() as *mut c_void,
            dos_path.length as usize,
            &mut bytes_read,
        );
        if ok == 0 {
            return None;
        }

        let path_str = String::from_utf16_lossy(&buf);
        let trimmed = path_str.trim_end_matches(['\0', ' ']);
        // A drive root ("C:\") must keep its separator: without it the path
        // would stop being absolute.
        if trimmed.len() == 2 && trimmed.ends_with(':') {
            return Some(PathBuf::from(format!("{trimmed}\\")));
        }
        if trimmed.is_empty() {
            None
        } else {
            Some(PathBuf::from(trimmed))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shell_directory_can_be_read_on_windows() {
        let cwd = shell_cwd(std::process::id());
        assert!(cwd.is_some(), "cwd should be readable for current process");
        let expected = std::env::current_dir().ok();
        // Canonicalize or strip trailing slashes for comparison
        let clean = |p: PathBuf| {
            p.to_string_lossy()
                .trim_end_matches(['\\', '/'])
                .to_ascii_lowercase()
        };
        assert_eq!(
            cwd.map(clean),
            expected.map(clean),
            "cwd should match std::env::current_dir"
        );
    }

    #[test]
    fn a_shell_with_a_running_child_says_so_on_windows() {
        let mut child = std::process::Command::new("powershell")
            .arg("-NoProfile")
            .arg("-Command")
            .arg("Start-Sleep -Seconds 30")
            .spawn()
            .expect("spawn sleep");

        let mut found = false;
        for _ in 0..20 {
            std::thread::sleep(std::time::Duration::from_millis(100));
            let (children, _) = process_table();
            if descendant_pids(&children, std::process::id()).contains(&child.id()) {
                found = true;
                break;
            }
        }
        assert!(found, "child powershell was detected");
        // The pane-close prompt matches by name, so the name must come back
        // for a live child. (The pid check above stays accurate even while
        // other tests run their own shells.)
        let names = running_children(std::process::id());
        assert!(
            names
                .iter()
                .any(|n| n.eq_ignore_ascii_case("powershell.exe")),
            "the live child's name is reported: {names:?}"
        );

        let _ = child.kill();
        let _ = child.wait();

        let mut gone = false;
        for _ in 0..20 {
            std::thread::sleep(std::time::Duration::from_millis(100));
            let (children, _) = process_table();
            if !descendant_pids(&children, std::process::id()).contains(&child.id()) {
                gone = true;
                break;
            }
        }
        assert!(gone, "reaped child is gone");
    }
}
