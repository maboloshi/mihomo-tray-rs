//! Handing a URL or a file to the shell.
//!
//! The tray hosts no window and no web engine on purpose, so the kernel's
//! dashboard is opened in whatever browser the user has registered — a
//! dashboard may also be hosted elsewhere and only *talk* to this kernel. The
//! same reasoning covers the two configuration files the menu offers to open:
//! the tray has no settings window, so an editor the user already configured is
//! the honest place to edit them.
//!
//! The call is `ShellExecuteW`, not `ShellExecuteExW`: there is no process to
//! wait for, and the entry point is the documented one for "open this with its
//! default handler". A machine with no handler reports an error code without
//! showing a system dialog, so the failure can be reported in the menu's own
//! error line like every other action.

use std::path::Path;

use windows_sys::Win32::UI::Shell::ShellExecuteW;
use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

use crate::i18n;

/// `ShellExecuteW` reports success as "greater than 32"; anything at or below
/// that is one of its documented error codes.
const SHELL_EXECUTE_OK: isize = 32;

/// Open `url` with the default handler (a browser, for a http/https URL).
pub fn open_url(url: &str) -> Result<(), String> {
    execute(url).map_err(|code| i18n::t().error_open_web_ui(&code.to_string()))
}

/// Open `path` with its default handler.
///
/// A path that does not exist yet falls back to the directory it would live in:
/// the kernel's `config.yaml` is a candidate mihomo resolves, not a file that
/// has to exist, and a machine where the kernel never ran has no such file to
/// hand to an editor. Opening the directory is still "the default handler for
/// this path", and it is the only answer that lets the user create the file.
/// When even that directory is missing there is nothing to open, and the shell
/// reports it like any other failure.
pub fn open_path(path: &Path) -> Result<(), String> {
    let target = if path.exists() {
        path
    } else {
        match path.parent().filter(|dir| dir.is_dir()) {
            Some(dir) => dir,
            None => path,
        }
    };
    execute(&target.display().to_string())
        .map_err(|code| i18n::t().error_open_file(&path.display().to_string(), &code.to_string()))
}

/// `ShellExecuteW("open", target, …)`, with its raw error code.
fn execute(target: &str) -> Result<(), isize> {
    let operation = wide("open");
    let target = wide(target);
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
        return Err(code);
    }
    Ok(())
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}
