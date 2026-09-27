//! Single-instance guard: a named mutex held for the lifetime of the process.
//!
//! The elevated helper is the same executable, so `main` answers it before this
//! guard is taken: a second process is expected there, not a second tray.

use std::sync::atomic::{AtomicIsize, Ordering};

use windows_sys::Win32::Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, GetLastError};
use windows_sys::Win32::System::Threading::CreateMutexW;

static HANDLE: AtomicIsize = AtomicIsize::new(0);

const MUTEX_NAME: &str = "mihomo-tray-single-instance";

/// `true` when this process owns the guard, `false` when another one already
/// holds it (the caller should exit quietly).
pub fn acquire() -> bool {
    if HANDLE.load(Ordering::SeqCst) != 0 {
        return true;
    }
    let name: Vec<u16> = MUTEX_NAME
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    unsafe {
        let handle = CreateMutexW(std::ptr::null(), 0, name.as_ptr());
        if handle.is_null() {
            // Cannot even query the guard: better to keep running than to swallow
            // the launch.
            return true;
        }
        if GetLastError() == ERROR_ALREADY_EXISTS {
            CloseHandle(handle);
            return false;
        }
        HANDLE.store(handle as isize, Ordering::SeqCst);
    }
    true
}
