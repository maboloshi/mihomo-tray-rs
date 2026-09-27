//! Elevation: TUN needs administrator rights, the tray itself does not.
//!
//! The privileged work happens in a short-lived copy of this same executable
//! (`mihomo::proc::KERNEL_START_SWITCH` / `KERNEL_STOP_SWITCH`): it is handed the
//! PID of the running kernel, stops it, and starts a replacement from that
//! kernel's own image and command line — or only stops it. A cancelled UAC prompt
//! therefore leaves everything as it was: nothing has been killed yet at that
//! point. The PID is the only thing passed along, so a helper can never be talked
//! into starting an image of somebody else's choosing.
//!
//! Nothing privileged is ever driven from the tray process: it only launches the
//! helper and waits for it to exit.

use std::time::Duration;

use windows_sys::Win32::Foundation::CloseHandle;
use windows_sys::Win32::System::Threading::{GetExitCodeProcess, WaitForSingleObject};
use windows_sys::Win32::UI::Shell::{
    SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::SW_HIDE;

use crate::i18n;
use crate::mihomo::proc;

/// How long the helper may take. The UAC prompt itself is part of this, because
/// the helper process only starts running once the user has consented.
const HELPER_TIMEOUT: Duration = Duration::from_secs(120);
/// `WaitForSingleObject`'s timeout result, spelled out instead of imported.
const WAIT_TIMEOUT: u32 = 0x0000_0102;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// The helper's parameters for a kernel restart: the kernel's PID, nothing else.
///
/// No path and no arguments are passed: the helper reads both out of the running
/// kernel, which is the only way to restart it exactly as it was running — and
/// the only way to keep the helper from being a general elevated launcher.
pub fn kernel_start_params(pid: u32) -> String {
    format!("{} {pid}", proc::KERNEL_START_SWITCH)
}

/// The helper's parameters for stopping a kernel the tray has no rights over.
pub fn kernel_stop_params(pid: u32) -> String {
    format!("{} {pid}", proc::KERNEL_STOP_SWITCH)
}

/// Run this executable again through the UAC prompt and wait for it to finish.
/// The helper's exit code is returned as-is; `Err` means it never ran to a
/// result, which is what a cancelled prompt looks like.
pub fn run_self_elevated(params: &str) -> Result<i32, String> {
    let exe = std::env::current_exe().map_err(|e| i18n::t().error_current_exe(&e.to_string()))?;
    let operation = wide("runas");
    let file = wide(&exe.to_string_lossy());
    let parameters = wide(params);
    // A kernel started by the helper inherits the helper's working directory, so
    // hand it the one this tray runs in: relative paths inside the kernel's own
    // configuration are resolved against it.
    let directory = std::env::current_dir()
        .ok()
        .map(|dir| wide(&dir.to_string_lossy()));

    let mut info: SHELLEXECUTEINFOW = unsafe { std::mem::zeroed() };
    info.cbSize = std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
    // `NOASYNC` keeps the call synchronous on the worker thread, which pumps no
    // messages — the tray's UI thread must stay free.
    info.fMask = SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC;
    info.lpVerb = operation.as_ptr();
    info.lpFile = file.as_ptr();
    info.lpParameters = parameters.as_ptr();
    if let Some(directory) = &directory {
        info.lpDirectory = directory.as_ptr();
    }
    info.nShow = SW_HIDE;

    let started = unsafe { ShellExecuteExW(&mut info) };
    if started == 0 || info.hProcess.is_null() {
        // A cancelled prompt and a failed launch are the same thing to the user.
        return Err(i18n::t().error_elevate_cancelled.to_string());
    }

    let waited = unsafe { WaitForSingleObject(info.hProcess, HELPER_TIMEOUT.as_millis() as u32) };
    let mut code: u32 = 0;
    let read = unsafe { GetExitCodeProcess(info.hProcess, &mut code) };
    unsafe {
        CloseHandle(info.hProcess);
    }
    if waited == WAIT_TIMEOUT {
        return Err(i18n::t().error_elevate_timeout.to_string());
    }
    if read == 0 {
        return Err(i18n::t().error_elevate_cancelled.to_string());
    }
    Ok(code as i32)
}
