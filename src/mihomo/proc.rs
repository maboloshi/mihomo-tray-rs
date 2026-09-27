//! Kernel process handling: detect what is running, start it hidden, and stop
//! only the process this program is responsible for.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::Threading::{
    CREATE_NO_WINDOW, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_TERMINATE,
    QueryFullProcessImageNameW, TerminateProcess,
};

use crate::i18n;

const KERNEL_EXE: &str = "mihomo.exe";

#[derive(Debug, Clone)]
pub struct Process {
    pub pid: u32,
    pub path: PathBuf,
}

/// All running `mihomo.exe` processes, with their real image paths.
pub fn list_mihomo() -> Vec<Process> {
    let mut found = Vec::new();
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snapshot.is_null() || snapshot == (-1isize as HANDLE) {
            return found;
        }
        let mut entry: PROCESSENTRY32W = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        if Process32FirstW(snapshot, &mut entry) != 0 {
            loop {
                let name = String::from_utf16_lossy(&entry.szExeFile)
                    .trim_end_matches('\0')
                    .to_string();
                if name.eq_ignore_ascii_case(KERNEL_EXE) {
                    found.push(Process {
                        pid: entry.th32ProcessID,
                        path: image_path(entry.th32ProcessID).unwrap_or_default(),
                    });
                }
                if Process32NextW(snapshot, &mut entry) == 0 {
                    break;
                }
            }
        }
        CloseHandle(snapshot);
    }
    found
}

fn image_path(pid: u32) -> Option<PathBuf> {
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return None;
        }
        let mut buffer = [0u16; 520];
        let mut size = buffer.len() as u32;
        let ok = QueryFullProcessImageNameW(handle, 0, buffer.as_mut_ptr(), &mut size);
        CloseHandle(handle);
        if ok == 0 {
            return None;
        }
        Some(PathBuf::from(String::from_utf16_lossy(
            &buffer[..size as usize],
        )))
    }
}

/// Start the kernel without a console window; the returned child stays under our
/// control so "exit and stop mihomo" only affects the instance we launched.
pub fn start(exe: &Path, args: &[String]) -> Result<Child, String> {
    let mut command = Command::new(exe);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
        .spawn()
        .map_err(|e| i18n::t().error_start_process(&exe.display().to_string(), &e.to_string()))
}

pub fn kill(pid: u32) -> Result<(), String> {
    unsafe {
        let handle = OpenProcess(PROCESS_TERMINATE, 0, pid);
        if handle.is_null() {
            return Err(i18n::t().error_open_process(pid));
        }
        let ok = TerminateProcess(handle, 0);
        CloseHandle(handle);
        if ok == 0 {
            return Err(i18n::t().error_kill_process(pid));
        }
    }
    Ok(())
}
