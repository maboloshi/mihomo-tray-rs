//! Handing a URL to the shell.
//!
//! The tray hosts no window and no web engine on purpose, so the kernel's
//! dashboard is opened in whatever browser the user has registered — a
//! dashboard may also be hosted elsewhere and only *talk* to this kernel.
//!
//! The call is `ShellExecuteW`, not `ShellExecuteExW`: there is no process to
//! wait for, and the entry point is the documented one for "open this with its
//! default handler". A machine with no handler reports an error code without
//! showing a system dialog, so the failure can be reported in the menu's own
//! error line like every other action.

use windows_sys::Win32::UI::Shell::ShellExecuteW;
use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

use crate::i18n;

/// `ShellExecuteW` reports success as "greater than 32"; anything at or below
/// that is one of its documented error codes.
const SHELL_EXECUTE_OK: isize = 32;

/// Open `url` with the default handler (a browser, for a http/https URL).
pub fn open_url(url: &str) -> Result<(), String> {
    let operation = wide("open");
    let target = wide(url);
    let code = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            operation.as_ptr(),
            target.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        )
    } as isize;
    if code <= SHELL_EXECUTE_OK {
        return Err(i18n::t().error_open_web_ui(&code.to_string()));
    }
    Ok(())
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}
