//! Kernel process handling: detect what is running, start it hidden, stop the
//! process this program is responsible for, and — through the elevated helper —
//! replace it with one that has the rights TUN needs.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, HANDLE, LocalFree};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::Environment::GetCommandLineW;
use windows_sys::Win32::System::Threading::{
    CREATE_NO_WINDOW, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE,
    PROCESS_TERMINATE, QueryFullProcessImageNameW, TerminateProcess, WaitForSingleObject,
};
use windows_sys::Win32::UI::Shell::CommandLineToArgvW;

use crate::i18n;

const KERNEL_EXE: &str = "mihomo.exe";
/// `OpenProcess` failing with this is how a higher-integrity process says "not
/// yours to look at"; anything else is a process that simply went away.
const ERROR_ACCESS_DENIED: u32 = 5;
/// How long to wait for a terminated kernel to disappear. The replacement binds
/// the same ports right afterwards, so a half-dead listener must not linger.
const TERMINATE_WAIT_MS: u32 = 5_000;

/// Command-line switches that turn this executable into the elevated helper: one
/// for TUN (replace the kernel with an elevated one), one for stopping a kernel
/// the tray has no rights over. `main` handles them before the single-instance
/// guard, so the helper is a second process of this same binary.
///
/// Both switches take exactly one argument, the PID of the running `mihomo.exe`
/// to work on. The image and the arguments of that kernel are read out of the
/// process itself, never handed in on the command line: a helper that could be
/// told which image to start would be a general "run this as administrator"
/// primitive, and a kernel's own command line is also the only faithful source
/// for flags that were never in `tray.yml` (an `-ext-ctl` on the command line,
/// an unusual `-f`, and so on).
pub const KERNEL_START_SWITCH: &str = "--kernel-start-elevated";
pub const KERNEL_STOP_SWITCH: &str = "--kernel-stop-elevated";

/// Helper exit codes. The tray only needs "0 or not", but naming them keeps the
/// helper readable.
pub const HELPER_OK: i32 = 0;
pub const HELPER_BAD_ARGS: i32 = 2;
pub const HELPER_NOT_STOPPED: i32 = 3;
pub const HELPER_NOT_STARTED: i32 = 4;
/// A `mihomo.exe` is running that even the helper may not stop.
pub const HELPER_DENIED: i32 = 5;
/// The kernel's own command line could not be read, so it cannot be restarted
/// the way it was running.
pub const HELPER_UNREADABLE: i32 = 6;

#[derive(Debug, Clone)]
pub struct Process {
    pub pid: u32,
    pub path: PathBuf,
    /// The image path could not be read, so the process could not be told apart
    /// from the kernel this program is responsible for: a kernel with higher
    /// rights, or one whose path query was refused. Never touched from here.
    pub denied: bool,
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
                    let (path, denied) = image_path(entry.th32ProcessID);
                    found.push(Process {
                        pid: entry.th32ProcessID,
                        path,
                        denied,
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

/// The image path of `pid`, and whether it could not be verified at all.
///
/// The elevated kernel is the case that matters: the limited-information handle
/// opens, but reading the image path is refused. Such a process is neither
/// provably ours nor provably somebody else's, so it is reported as unverified
/// instead of being silently ignored — this process can do nothing about it,
/// while the elevated helper can read the path and stop it if it matches.
fn image_path(pid: u32) -> (PathBuf, bool) {
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return (PathBuf::new(), GetLastError() == ERROR_ACCESS_DENIED);
        }
        let mut buffer = [0u16; 520];
        let mut size = buffer.len() as u32;
        let ok = QueryFullProcessImageNameW(handle, 0, buffer.as_mut_ptr(), &mut size);
        CloseHandle(handle);
        if ok == 0 {
            return (PathBuf::new(), true);
        }
        (
            PathBuf::from(String::from_utf16_lossy(&buffer[..size as usize])),
            false,
        )
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

/// Why a process could not be terminated.
enum KillError {
    /// The process runs with higher rights than this one.
    Denied,
    Failed(String),
}

/// Terminate `pid` and wait for it to disappear. A refusal is told apart from a
/// failure because only the former can be retried with the elevated helper.
fn kill(pid: u32) -> Result<(), KillError> {
    unsafe {
        let handle = OpenProcess(PROCESS_TERMINATE | PROCESS_SYNCHRONIZE, 0, pid);
        if handle.is_null() {
            return Err(if GetLastError() == ERROR_ACCESS_DENIED {
                KillError::Denied
            } else {
                KillError::Failed(i18n::t().error_open_process(pid))
            });
        }
        let ok = TerminateProcess(handle, 0);
        let denied = ok == 0 && GetLastError() == ERROR_ACCESS_DENIED;
        if ok != 0 {
            WaitForSingleObject(handle, TERMINATE_WAIT_MS);
        }
        CloseHandle(handle);
        if ok == 0 {
            return Err(if denied {
                KillError::Denied
            } else {
                KillError::Failed(i18n::t().error_kill_process(pid))
            });
        }
    }
    Ok(())
}

/// What a stop attempt found.
#[derive(Debug, Default, Clone, Copy)]
pub struct StopOutcome {
    pub stopped: usize,
    /// `mihomo.exe` processes that could not be stopped because they run with
    /// higher rights: either their image path was unreadable, or terminating
    /// them was refused. Both mean "not ours to end from here".
    pub denied: usize,
}

/// Stop every `mihomo.exe` running from `exe`. Nothing else is touched: a
/// process whose path is unknown is counted, never guessed at.
pub fn stop_matching(exe: &Path) -> Result<StopOutcome, String> {
    let mut outcome = StopOutcome::default();
    for process in list_mihomo() {
        if process.denied {
            outcome.denied += 1;
            continue;
        }
        if process.path != exe {
            continue;
        }
        match kill(process.pid) {
            Ok(()) => outcome.stopped += 1,
            // Being refused is exactly the case the elevated helper exists for,
            // so it is a status to report, not a failure.
            Err(KillError::Denied) => outcome.denied += 1,
            Err(KillError::Failed(error)) => return Err(error),
        }
    }
    Ok(outcome)
}

/// Body of the elevated helper: `args` is `[kernel pid]`.
///
/// The running kernel has to go first — the replacement binds the same ports —
/// and it is stopped by a process that has the rights to do so. Starting the
/// kernel is the last step, so a failure there leaves the machine without a
/// kernel rather than with two of them. The replacement is started exactly the
/// way the kernel it replaces was running, its arguments included.
pub fn start_kernel_elevated(args: &[String]) -> i32 {
    let Some(pid) = parse_pid(args) else {
        return HELPER_BAD_ARGS;
    };
    let Some(exe) = kernel_image(pid) else {
        return HELPER_BAD_ARGS;
    };
    // Read before stopping anything: the command line of a process that is gone
    // cannot be read any more.
    let Some(kernel_args) = command_line(pid).map(|argv| argv[1..].to_vec()) else {
        return HELPER_UNREADABLE;
    };
    match stop_matching(&exe) {
        // A `mihomo.exe` that survives means the ports are still taken: starting a
        // second kernel would only produce two half-working ones.
        Ok(outcome) if outcome.denied > 0 => return HELPER_NOT_STOPPED,
        Ok(_) => {}
        Err(_) => return HELPER_NOT_STOPPED,
    }
    match start(&exe, &kernel_args) {
        // The child is deliberately dropped: it must outlive the helper, and an
        // `std::process::Child` does not kill on drop.
        Ok(_) => HELPER_OK,
        Err(_) => HELPER_NOT_STARTED,
    }
}

/// Body of the stop-only helper: `args` is `[kernel pid]`. "Exit and stop mihomo"
/// uses it when the kernel was elevated by an earlier TUN enable, which leaves
/// the tray without any rights over it.
pub fn stop_kernel_elevated(args: &[String]) -> i32 {
    let Some(pid) = parse_pid(args) else {
        return HELPER_BAD_ARGS;
    };
    let Some(exe) = kernel_image(pid) else {
        return HELPER_BAD_ARGS;
    };
    match stop_matching(&exe) {
        Ok(outcome) if outcome.stopped > 0 => HELPER_OK,
        // Told apart from "nothing matched", because only one of the two means
        // that asking with more rights could ever help.
        Ok(outcome) if outcome.denied > 0 => HELPER_DENIED,
        Ok(_) => HELPER_NOT_STOPPED,
        Err(_) => HELPER_NOT_STOPPED,
    }
}

/// The single helper argument: the PID of the kernel to work on.
fn parse_pid(args: &[String]) -> Option<u32> {
    let pid = args.first()?.parse::<u32>().ok()?;
    (pid != 0).then_some(pid)
}

/// The image of `pid`, but only while that process is a `mihomo.exe`.
///
/// The name check is what keeps the helper from becoming a way to launch an
/// arbitrary image with administrator rights: the path is taken from the process
/// itself, and only a kernel is accepted.
fn kernel_image(pid: u32) -> Option<PathBuf> {
    let (path, denied) = image_path(pid);
    (!denied && !path.as_os_str().is_empty() && is_kernel_name(&path)).then_some(path)
}

fn is_kernel_name(path: &Path) -> bool {
    path.file_name()
        .is_some_and(|name| name.eq_ignore_ascii_case(KERNEL_EXE))
}

/// `argv` of `pid`, exactly as that process received it.
///
/// `NtQueryInformationProcess(ProcessCommandLineInformation)` copies the string
/// into the caller's buffer, so no PEB walking — and no assumption about the
/// bitness of the target — is needed. It wants the same rights as any other
/// query, which is why this lives in the helper: the tray cannot read the kernel
/// it is about to replace, and the helper can.
fn command_line(pid: u32) -> Option<Vec<String>> {
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return None;
        }
        let mut buffer = vec![0u8; 8 * 1024];
        let mut needed: u32 = 0;
        let mut status = NtQueryInformationProcess(
            handle,
            PROCESS_COMMAND_LINE_INFORMATION,
            buffer.as_mut_ptr() as *mut core::ffi::c_void,
            buffer.len() as u32,
            &mut needed,
        );
        if status == STATUS_BUFFER_TOO_SMALL || status == STATUS_INFO_LENGTH_MISMATCH {
            // The kernel reports how much it needs; one retry is enough because
            // the size only grows while the command line itself changes.
            if needed as usize > buffer.len() {
                buffer.resize(needed as usize, 0);
                status = NtQueryInformationProcess(
                    handle,
                    PROCESS_COMMAND_LINE_INFORMATION,
                    buffer.as_mut_ptr() as *mut core::ffi::c_void,
                    buffer.len() as u32,
                    &mut needed,
                );
            }
        }
        CloseHandle(handle);
        if status < 0 {
            return None;
        }
        // The bytes were written by the kernel into a byte buffer, so the header
        // has to be read without assuming an alignment the buffer never promised.
        let header = core::ptr::read_unaligned(buffer.as_ptr() as *const UnicodeString);
        let length = header.length as usize;
        let text = header.buffer;
        if text.is_null() || length == 0 {
            return None;
        }
        let units = std::slice::from_raw_parts(text, length / 2);
        Some(parse_command_line(&String::from_utf16_lossy(units)))
    }
}

/// This process's own command line, parsed the way Windows parsed it.
pub fn own_command_line() -> Vec<String> {
    unsafe { parse_command_line(&from_wide(GetCommandLineW())) }
}

/// Parse a command line with `CommandLineToArgvW`, the parser Windows itself
/// uses, so what the helper sees is what the kernel saw.
pub fn parse_command_line(line: &str) -> Vec<String> {
    let buffer: Vec<u16> = line.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        let mut count = 0i32;
        let argv = CommandLineToArgvW(buffer.as_ptr(), &mut count);
        if argv.is_null() {
            return Vec::new();
        }
        let mut out = Vec::with_capacity(count.max(0) as usize);
        for index in 0..count.max(0) as isize {
            let entry = *argv.offset(index);
            if entry.is_null() {
                break;
            }
            let mut len = 0isize;
            while *entry.offset(len) != 0 {
                len += 1;
            }
            out.push(String::from_utf16_lossy(std::slice::from_raw_parts(
                entry,
                len as usize,
            )));
        }
        LocalFree(argv as *mut core::ffi::c_void);
        out
    }
}

unsafe fn from_wide(pointer: *const u16) -> String {
    unsafe {
        let mut len = 0isize;
        while *pointer.offset(len) != 0 {
            len += 1;
        }
        String::from_utf16_lossy(std::slice::from_raw_parts(pointer, len as usize))
    }
}

/// `UNICODE_STRING`, as `NtQueryInformationProcess` writes it.
#[repr(C)]
struct UnicodeString {
    length: u16,
    maximum_length: u16,
    buffer: *mut u16,
}

/// `ProcessCommandLineInformation`; the string comes back in the caller's own
/// buffer, so no remote memory has to be read.
const PROCESS_COMMAND_LINE_INFORMATION: u32 = 60;
const STATUS_INFO_LENGTH_MISMATCH: i32 = 0xC000_0004u32 as i32;
const STATUS_BUFFER_TOO_SMALL: i32 = 0xC000_0023u32 as i32;

#[link(name = "ntdll")]
unsafe extern "system" {
    fn NtQueryInformationProcess(
        handle: HANDLE,
        class: u32,
        info: *mut core::ffi::c_void,
        length: u32,
        returned: *mut u32,
    ) -> i32;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helper_takes_a_pid_and_nothing_else() {
        assert_eq!(parse_pid(&["4242".to_string()]), Some(4242));
        assert_eq!(parse_pid(&[]), None);
        assert_eq!(parse_pid(&["0".to_string()]), None);
        assert_eq!(parse_pid(&[r"C:\mihomo.exe".to_string()]), None);
        // Extra arguments are ignored: only the first one is the PID.
        assert_eq!(parse_pid(&["7".to_string(), "8".to_string()]), Some(7));
    }

    #[test]
    fn only_images_named_like_the_kernel_are_accepted() {
        assert!(is_kernel_name(Path::new(r"C:\anywhere\mihomo.exe")));
        assert!(is_kernel_name(Path::new(r"C:\anywhere\MIHOMO.EXE")));
        assert!(!is_kernel_name(Path::new(r"C:\anywhere\mihomo.exe.bak")));
        assert!(!is_kernel_name(Path::new(r"C:\anywhere\notmihomo.exe")));
        // This test binary is not the kernel, so the helper must refuse it even
        // though it is a running process with a readable command line.
        assert_eq!(kernel_image(std::process::id()), None);
    }

    #[test]
    fn a_running_process_command_line_can_be_read() {
        let argv = command_line(std::process::id()).expect("own command line");
        assert!(!argv.is_empty(), "argv was empty");
        let exe = PathBuf::from(&argv[0]);
        assert!(exe.is_file(), "argv[0] was not an image path: {exe:?}");
    }

    #[test]
    fn command_lines_are_parsed_the_way_windows_parses_them() {
        assert_eq!(
            parse_command_line(r#""C:\Program Files\mihomo\mihomo.exe" -d "C:\odd path" plain"#),
            vec![
                r"C:\Program Files\mihomo\mihomo.exe",
                "-d",
                r"C:\odd path",
                "plain"
            ]
        );
        // Whatever Windows makes of an empty line, it never yields a PID — which
        // is all the helper needs from it.
        let empty = parse_command_line("");
        assert!(empty.len() <= 1, "unexpected {empty:?}");
        assert!(parse_pid(&empty).is_none());
    }
}
