//! Kernel process handling: detect what is running, start it hidden, stop the
//! process this program is responsible for, and — through the elevated helper —
//! replace it with one that has the rights TUN needs.

use std::ffi::OsStr;
use std::io;
use std::mem::size_of;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::ExitStatus;

use windows_sys::Win32::Foundation::{
    CloseHandle, GENERIC_READ, GENERIC_WRITE, GetLastError, HANDLE, INVALID_HANDLE_VALUE,
    LocalFree, TRUE, WAIT_FAILED, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::Environment::GetCommandLineW;
use windows_sys::Win32::System::Threading::{
    CREATE_NO_WINDOW, CreateProcessW, GetExitCodeProcess, INFINITE, OpenProcess,
    PROCESS_INFORMATION, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE, PROCESS_TERMINATE,
    QueryFullProcessImageNameW, STARTF_USESTDHANDLES, STARTUPINFOW, TerminateProcess,
    WaitForSingleObject,
};
use windows_sys::Win32::UI::Shell::CommandLineToArgvW;

use crate::i18n;

/// The kernel's file name: the one binary the discovery chain looks for, and the
/// only image a helper may start.
pub const KERNEL_EXE: &str = "mihomo.exe";
/// `OpenProcess` failing with this is how a higher-integrity process says "not
/// yours to look at"; anything else is a process that simply went away.
const ERROR_ACCESS_DENIED: u32 = 5;
/// `OpenProcess` failing with this means the PID is not a running process: the
/// one that was listed has exited in the meantime.
const ERROR_INVALID_PARAMETER: u32 = 87;
/// How long to wait for a terminated kernel to disappear. The replacement binds
/// the same ports right afterwards, so a half-dead listener must not linger.
const TERMINATE_WAIT_MS: u32 = 5_000;

/// Command-line switches that turn this executable into the elevated helper: one
/// for TUN (replace the kernel with an elevated one), one for stopping a kernel
/// the tray has no rights over, and one for replacing a kernel the tray has no
/// rights over. `main` handles them before the single-instance guard, so the
/// helper is a second process of this same binary.
///
/// Every switch takes the PID of the running `mihomo.exe` to work on first, and
/// reads the kernel's image out of that process (`docs/ROADMAP.md`, hard constraint
/// 9): a helper that could be told which image to start would be a general "run
/// this as administrator" primitive. The start helper also takes the arguments from
/// that process, which is the only faithful source for flags that were never in
/// `tray.yml` (an `-ext-ctl` on the command line, an unusual `-f`, and so on).
///
/// The replacing switch is the one exception to "the replacement is a copy of the
/// running kernel", which is exactly what it is for: the replacement's arguments
/// are handed in by the tray, computed from `tray.yml` the same way the kernel the
/// tray starts itself is — the elevated copy cannot compute them for itself,
/// because the settings file lives under the invoking account's `%APPDATA%` and
/// that is not necessarily visible to the account it runs as.
pub const KERNEL_START_SWITCH: &str = "--kernel-start-elevated";
pub const KERNEL_STOP_SWITCH: &str = "--kernel-stop-elevated";
pub const KERNEL_REPLACE_SWITCH: &str = "--kernel-replace-elevated";

/// Helper exit codes: the start helper answers with the PID of the kernel it
/// started, which is positive by construction, so every failure is negative.
///
/// The answer travels in the exit code because that is the one channel a process
/// launched through the UAC prompt has. A report file would have to be named by
/// the caller — which would hand any unprivileged process a way to make this
/// helper write a file with administrator rights.
pub const HELPER_OK: i32 = 0;
pub const HELPER_BAD_ARGS: i32 = -2;
pub const HELPER_NOT_STOPPED: i32 = -3;
pub const HELPER_NOT_STARTED: i32 = -4;
/// A `mihomo.exe` is running that even the helper may not stop.
pub const HELPER_DENIED: i32 = -5;
/// The kernel's own command line could not be read, so it cannot be restarted
/// the way it was running.
pub const HELPER_UNREADABLE: i32 = -6;

#[derive(Debug, Clone)]
pub struct Process {
    pub pid: u32,
    /// The process that started it. A scoop shim is called `mihomo.exe` as well,
    /// and the kernel it starts is its child, so this is what tells a launcher
    /// apart from the kernel it launched.
    pub parent: u32,
    pub path: PathBuf,
    /// The image path could not be read, so the process could not be told apart
    /// from the kernel this program is responsible for: a kernel with higher
    /// rights, or one whose path query was refused. Never touched from here.
    pub denied: bool,
}

/// Whether two image paths name the same file.
///
/// Comparing the strings is not enough on Windows, for two reasons that both
/// showed up on a real machine: paths are case-insensitive, and a scoop install
/// starts the kernel through its `apps\<app>\current\mihomo.exe` junction while
/// the running process reports the versioned directory it really lives in. The
/// comparison therefore resolves both sides (junctions, symlinks and `..`) and
/// falls back to the literal path when a file cannot be resolved — a kernel that
/// is already gone must still compare equal to the path recorded for it.
pub fn same_image(a: &Path, b: &Path) -> bool {
    let resolved = |path: &Path| std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let (a, b) = (resolved(a), resolved(b));
    a.to_string_lossy()
        .eq_ignore_ascii_case(&b.to_string_lossy())
}

/// Whether a path is a scoop shim: a launcher that carries the kernel's name
/// (`shims\mihomo.exe`) and starts the real binary as its child, so it is not the
/// kernel a caller is looking for.
pub fn is_shim(path: &Path) -> bool {
    path.parent()
        .and_then(Path::file_name)
        .is_some_and(|name| name.eq_ignore_ascii_case("shims"))
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
                        parent: entry.th32ParentProcessID,
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

/// The first buffer size tried for a process image path, in UTF-16 units. Large
/// enough for every ordinary path; the call site grows it if a longer one ever
/// appears.
const IMAGE_PATH_CHARS: u32 = 1024;
/// Upper bound for that growth: the longest path Windows itself accepts, so a
/// misbehaving call cannot turn this into an allocation loop.
const IMAGE_PATH_MAX_CHARS: u32 = 32 * 1024;

/// The image path of `pid`, and whether it could not be verified at all.
///
/// The elevated kernel is the case that matters: the limited-information handle
/// opens, but reading the image path is refused. Such a process is neither
/// provably ours nor provably somebody else's, so it is reported as unverified
/// instead of being silently ignored — this process can do nothing about it,
/// while the elevated helper can read the path and stop it if it matches.
///
/// Any failure counts as unverifiable, not just an access denial: a process whose
/// image cannot be named must never be filed under "somebody else's", because the
/// conclusion drawn from that is "nothing left to stop".
fn image_path(pid: u32) -> (PathBuf, bool) {
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return (PathBuf::new(), true);
        }
        // A fixed buffer would file a path longer than it as "unverifiable", and
        // that verdict later costs an extra UAC prompt. The call does not report
        // the length it wanted, so the buffer is grown until it fits: the first
        // attempt covers every ordinary path, and the loop exists for the rest.
        let path = read_image_path(handle, IMAGE_PATH_CHARS);
        CloseHandle(handle);
        path
    }
}

/// Read the image path of `handle` into a buffer of `first_size` UTF-16 units,
/// growing it while the call keeps failing.
///
/// The call does not distinguish "the buffer was too small" from "the process
/// refused to name its image". A local process opened with
/// `PROCESS_QUERY_LIMITED_INFORMATION` only fails the first way, and the caller
/// treats a final failure as unverifiable either way, so growing the buffer and
/// asking once more is the safe reading.
fn read_image_path(handle: HANDLE, first_size: u32) -> (PathBuf, bool) {
    let mut size = first_size.max(1);
    unsafe {
        loop {
            let mut buffer = vec![0u16; size as usize];
            let mut length = size;
            let ok = QueryFullProcessImageNameW(handle, 0, buffer.as_mut_ptr(), &mut length);
            if ok != 0 {
                // `length` counts the terminating NUL, which is not part of the path.
                let end = (length as usize).min(buffer.len());
                return (
                    PathBuf::from(String::from_utf16_lossy(&buffer[..end])),
                    false,
                );
            }
            if size >= IMAGE_PATH_MAX_CHARS {
                // Not a length problem: an elevated process, or a path past what
                // Windows itself accepts.
                return (PathBuf::new(), true);
            }
            size = size.saturating_mul(2).min(IMAGE_PATH_MAX_CHARS);
        }
    }
}

/// The kernel this program started, held by its process handle.
///
/// `std::process::Child` is the obvious type, but a `Child` only comes out of
/// `std::process::Command`, and that launcher carries an environment block, a
/// pipe relay and its own argument quoting into a program that starts exactly one
/// process. This type keeps the three things the program does with the kernel it
/// launched — is it still running, end it, wait for it — on the handle
/// `CreateProcessW` hands back.
#[derive(Debug)]
pub struct KernelChild {
    pid: u32,
    handle: HANDLE,
}

impl KernelChild {
    /// The PID: what the replacing helper answers with, so the tray learns which
    /// process it now owns.
    pub fn pid(&self) -> u32 {
        self.pid
    }

    /// `Ok(None)` while the kernel runs, its exit status once it is gone.
    pub fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        match unsafe { WaitForSingleObject(self.handle, 0) } {
            WAIT_TIMEOUT => Ok(None),
            WAIT_OBJECT_0 => self.exit_status().map(Some),
            _ => Err(io::Error::last_os_error()),
        }
    }

    /// End the kernel. A process that is already gone is a failure here, like it
    /// is for `std::process::Child`: the caller only kills what it just saw
    /// running.
    pub fn kill(&mut self) -> io::Result<()> {
        if unsafe { TerminateProcess(self.handle, 1) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    /// Wait for the kernel to disappear. The caller wants the process object gone
    /// (the replacement binds the same ports), not the exit code.
    pub fn wait(&mut self) -> io::Result<ExitStatus> {
        if unsafe { WaitForSingleObject(self.handle, INFINITE) } == WAIT_FAILED {
            return Err(io::Error::last_os_error());
        }
        self.exit_status()
    }

    fn exit_status(&self) -> io::Result<ExitStatus> {
        let mut code = 0u32;
        if unsafe { GetExitCodeProcess(self.handle, &mut code) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(ExitStatus::from_raw(code))
    }

    /// Take over a process somebody else started, for the tests that need a live
    /// stand-in for a kernel.
    #[cfg(test)]
    pub fn adopt(child: std::process::Child) -> Self {
        use std::os::windows::io::AsRawHandle;
        // The handle is taken over rather than copied: the child must not close it
        // on drop, because its new owner closes it.
        let child = std::mem::ManuallyDrop::new(child);
        Self {
            pid: child.id(),
            handle: child.as_raw_handle() as HANDLE,
        }
    }
}

impl Drop for KernelChild {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.handle);
        }
    }
}

// A process handle belongs to the process, not to the thread: the worker starts
// the kernel and the UI thread ends it, exactly like the `std::process::Child`
// this replaces.
unsafe impl Send for KernelChild {}

/// Start the kernel without a console window; the returned handle stays under our
/// control so "exit and stop mihomo" only affects the instance we launched.
///
/// The working directory is set to the kernel's own, because the kernel resolves
/// a relative path in its arguments against it, and inheriting this program's
/// would make that depend on how the tray itself was started (an Explorer launch
/// hands it whatever the shell had). The kernel's directory is at least one its
/// user knows.
pub fn start(exe: &Path, args: &[String]) -> Result<KernelChild, String> {
    unsafe { spawn(exe, args) }.map_err(|error| {
        i18n::t().error_start_process(&exe.display().to_string(), &error.to_string())
    })
}

unsafe fn spawn(exe: &Path, args: &[String]) -> io::Result<KernelChild> {
    let executable = nul_terminated(exe.as_os_str());
    let mut line = kernel_command_line(exe, args);
    let directory = exe.parent().map(|dir| nul_terminated(dir.as_os_str()));
    // The kernel has a hidden console of its own and nobody reads what it writes
    // there, so all three streams are the NUL device — `Stdio::null()` by hand.
    // Failing to open it is not fatal: without `STARTF_USESTDHANDLES` the kernel
    // simply starts with no standard handles at all.
    let inheritable = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: std::ptr::null_mut(),
        bInheritHandle: TRUE,
    };
    let device = nul_terminated(OsStr::new("NUL"));
    let null_device = unsafe {
        CreateFileW(
            device.as_ptr(),
            GENERIC_READ | GENERIC_WRITE,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            &inheritable,
            OPEN_EXISTING,
            0,
            std::ptr::null_mut(),
        )
    };
    let inherit = null_device != INVALID_HANDLE_VALUE;
    let mut startup: STARTUPINFOW = unsafe { std::mem::zeroed() };
    startup.cb = size_of::<STARTUPINFOW>() as u32;
    if inherit {
        startup.dwFlags = STARTF_USESTDHANDLES;
        startup.hStdInput = null_device;
        startup.hStdOutput = null_device;
        startup.hStdError = null_device;
    }
    let mut process: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
    let created = unsafe {
        CreateProcessW(
            executable.as_ptr(),
            line.as_mut_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            inherit as i32,
            CREATE_NO_WINDOW,
            std::ptr::null(),
            directory
                .as_ref()
                .map_or(std::ptr::null(), |dir| dir.as_ptr()),
            &startup,
            &mut process,
        )
    };
    if inherit {
        unsafe {
            CloseHandle(null_device);
        }
    }
    if created == 0 {
        return Err(io::Error::last_os_error());
    }
    unsafe {
        CloseHandle(process.hThread);
    }
    Ok(KernelChild {
        pid: process.dwProcessId,
        handle: process.hProcess,
    })
}

/// The command line for `exe` followed by `args`, NUL-terminated. `CreateProcessW`
/// takes one string where the argument boundaries are quoting, not separation.
fn kernel_command_line(exe: &Path, args: &[String]) -> Vec<u16> {
    let mut line = Vec::new();
    append_arg(&mut line, exe.as_os_str());
    for arg in args {
        line.push(' ' as u16);
        append_arg(&mut line, OsStr::new(arg));
    }
    line.push(0);
    line
}

/// Append one argument, quoted the way the C runtime reads a command line back
/// (the rules `CommandLineToArgvW` implements, which `std::process::Command`
/// follows too): an argument that is empty or holds a space, a tab or a quote is
/// wrapped in quotes, and backslashes that would otherwise sit in front of a
/// quote are doubled so they read as backslashes.
fn append_arg(line: &mut Vec<u16>, arg: &OsStr) {
    let quoted = arg.is_empty()
        || arg
            .encode_wide()
            .any(|unit| unit == ' ' as u16 || unit == '\t' as u16 || unit == '"' as u16);
    if !quoted {
        line.extend(arg.encode_wide());
        return;
    }
    line.push('"' as u16);
    let mut backslashes = 0usize;
    for unit in arg.encode_wide() {
        if unit == '\\' as u16 {
            backslashes += 1;
        } else if unit == '"' as u16 {
            line.extend(std::iter::repeat_n('\\' as u16, backslashes * 2 + 1));
            line.push(unit);
            backslashes = 0;
        } else {
            line.extend(std::iter::repeat_n('\\' as u16, backslashes));
            line.push(unit);
            backslashes = 0;
        }
    }
    line.extend(std::iter::repeat_n('\\' as u16, backslashes * 2));
    line.push('"' as u16);
}

/// `args` joined into one command line, quoted the way `CommandLineToArgvW` reads
/// it back.
///
/// This is how the replacing helper is handed the kernel's arguments:
/// `ShellExecuteExW` takes one string, so the argument boundaries have to survive
/// the trip out and back — including a path that ends in a backslash, which is the
/// case a hand-written `"…\"` quoting gets wrong.
pub fn quoted_args(args: &[String]) -> String {
    let mut line: Vec<u16> = Vec::new();
    for (index, arg) in args.iter().enumerate() {
        if index > 0 {
            line.push(' ' as u16);
        }
        append_arg(&mut line, OsStr::new(arg));
    }
    String::from_utf16_lossy(&line)
}

fn nul_terminated(value: &OsStr) -> Vec<u16> {
    value.encode_wide().chain(std::iter::once(0)).collect()
}

/// Why a process could not be terminated.
#[derive(Debug)]
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
            return match GetLastError() {
                ERROR_ACCESS_DENIED => Err(KillError::Denied),
                // The PID is not a running process any more: it exited between the
                // listing and this call, which is the outcome the caller wanted.
                // Reported as a failure it would abort the remaining kills and
                // block the kernel replacement that follows them.
                ERROR_INVALID_PARAMETER => Ok(()),
                _ => Err(KillError::Failed(i18n::t().error_open_process(pid))),
            };
        }
        let ok = TerminateProcess(handle, 0);
        // Read before anything else overwrites it.
        let termination_error = GetLastError();
        // A kernel that exited between the listing and here refuses termination
        // with the same access-denied the kernel uses for a process of higher
        // integrity, and only the process object itself tells the two apart: it is
        // signalled for a process that is gone, not for one that is out of reach.
        let exited = ok == 0 && WaitForSingleObject(handle, 0) == WAIT_OBJECT_0;
        if ok != 0 {
            WaitForSingleObject(handle, TERMINATE_WAIT_MS);
        }
        CloseHandle(handle);
        if ok == 0 && !exited {
            return Err(if termination_error == ERROR_ACCESS_DENIED {
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

/// The `mihomo.exe` processes that belong to the same family as `roots`.
///
/// A launcher runs in both directions. Scoop's shim is itself called
/// `mihomo.exe` and starts the real kernel as its child, so stopping only the
/// image the tray recorded leaves the kernel running — and stopping only the
/// kernel leaves the shim behind, waiting for it. Both are the same family, and
/// both have to go for the ports to be free.
///
/// Only processes already in `processes` are followed, so an unrelated shell that
/// happens to be a parent is never dragged in.
fn family(processes: &[Process], roots: &[u32]) -> Vec<u32> {
    let mut family: Vec<u32> = roots.to_vec();
    loop {
        let mut grew = false;
        for process in processes {
            if family.contains(&process.pid) {
                // Up to a `mihomo.exe` parent: that is the launcher.
                let mihomo_parent = process.parent != 0
                    && processes.iter().any(|other| other.pid == process.parent);
                if mihomo_parent && !family.contains(&process.parent) {
                    family.push(process.parent);
                    grew = true;
                }
            } else if family.contains(&process.parent) {
                // Down to everything a family member started.
                family.push(process.pid);
                grew = true;
            }
        }
        if !grew {
            break;
        }
    }
    family
}

/// Stop the kernel: every `mihomo.exe` whose image is `exe`, plus the family of
/// `root` when the caller knows which process it is responsible for.
///
/// The tray only has a path, the elevated helper has the PID it was handed —
/// which it resolved itself, so the path it passes here is the one it read from
/// that process. Both end up stopping a launcher and the kernel it launched. A
/// process whose path is unknown is counted as denied, never guessed at.
pub fn stop_kernel(root: Option<u32>, exe: &Path) -> Result<StopOutcome, String> {
    let processes = list_mihomo();
    let mut roots: Vec<u32> = Vec::new();
    if let Some(pid) = root {
        roots.push(pid);
    }
    roots.extend(
        processes
            .iter()
            .filter(|process| !process.denied && same_image(&process.path, exe))
            .map(|process| process.pid),
    );
    let family = family(&processes, &roots);

    let mut outcome = StopOutcome::default();
    for process in &processes {
        if !family.contains(&process.pid) {
            if process.denied {
                outcome.denied += 1;
            }
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
///
/// Success is answered with the PID of the new kernel: the caller cannot read an
/// elevated process's image path, so this is how it learns which process it now
/// owns.
pub fn start_kernel_elevated(args: &[String]) -> i32 {
    let Some(pid) = parse_pid(args) else {
        return HELPER_BAD_ARGS;
    };
    let Some(exe) = kernel_image(pid) else {
        return HELPER_BAD_ARGS;
    };
    // Read before stopping anything: the command line of a process that is gone
    // cannot be read any more. A kernel started without arguments has none to
    // carry over, which is `Some(&[])`, not an unreadable command line.
    let Some(kernel_args) =
        command_line(pid).and_then(|argv| argv.get(1..).map(<[String]>::to_vec))
    else {
        return HELPER_UNREADABLE;
    };
    match stop_kernel(Some(pid), &exe) {
        // A `mihomo.exe` that survives means the ports are still taken: starting a
        // second kernel would only produce two half-working ones.
        Ok(outcome) if outcome.denied > 0 => return HELPER_NOT_STOPPED,
        Ok(_) => {}
        Err(_) => return HELPER_NOT_STOPPED,
    }
    match start(&exe, &kernel_args) {
        // The handle is dropped once its PID is read: the kernel must outlive the
        // helper, and dropping the handle does not end the process.
        Ok(child) => i32::try_from(child.pid()).unwrap_or(HELPER_NOT_STARTED),
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
    match stop_kernel(Some(pid), &exe) {
        Ok(outcome) if outcome.stopped > 0 => HELPER_OK,
        // Told apart from "nothing matched", because only one of the two means
        // that asking with more rights could ever help.
        Ok(outcome) if outcome.denied > 0 => HELPER_DENIED,
        Ok(_) => HELPER_NOT_STOPPED,
        Err(_) => HELPER_NOT_STOPPED,
    }
}

/// Body of the replacing helper: `args` is `[kernel pid, kernel arguments…]`.
///
/// The counterpart of [`start_kernel_elevated`], for the case that motivated it
/// being different: the kernel that is running is not the one this program would
/// start, so the replacement cannot be a copy of it.
///
/// The image is not in the arguments: it is the running kernel's own image, read
/// from the process here, which is what keeps the helper from being a way to have
/// an arbitrary image started with administrator rights. The arguments *are* handed
/// in, because they are the ones the tray resolved from `tray.yml` — the
/// configuration this program would start a kernel with — and the elevated copy
/// cannot resolve them for itself: that settings file lives under the invoking
/// account's `%APPDATA%`, which the elevated account does not necessarily see.
///
/// Two processes are involved in the answer: the kernel that is ended and the one
/// that is started, and only the helper has the rights for either. Success is the
/// new kernel's PID, as in the start helper; nothing is started unless the old
/// kernel is really gone.
pub fn replace_kernel_elevated(args: &[String]) -> i32 {
    let Some(pid) = parse_pid(args) else {
        return HELPER_BAD_ARGS;
    };
    let Some(exe) = kernel_image(pid) else {
        return HELPER_BAD_ARGS;
    };
    match stop_kernel(Some(pid), &exe) {
        Ok(outcome) if outcome.denied > 0 => return HELPER_NOT_STOPPED,
        Ok(_) => {}
        Err(_) => return HELPER_NOT_STOPPED,
    }
    // A `mihomo.exe` that survived means the ports are still taken: starting the
    // replacement would produce two half-working kernels.
    if list_mihomo().iter().any(|process| process.pid == pid) {
        return HELPER_NOT_STOPPED;
    }
    match start(&exe, &args[1..]) {
        Ok(child) => i32::try_from(child.pid()).unwrap_or(HELPER_NOT_STARTED),
        Err(_) => HELPER_NOT_STARTED,
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
/// query: a kernel of the same user can be read from here, while one of higher
/// integrity cannot — which is why the elevated helper reads it for the kernel it
/// is about to replace. `None` is the honest answer in that case.
pub fn command_line(pid: u32) -> Option<Vec<String>> {
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

    use std::process::{Command, Stdio};

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
    fn a_process_image_path_is_read_whatever_its_length() {
        // Exercised on a process whose image really is there, because the failure
        // this guards against is precisely "could not be verified": without the
        // readable path a real kernel only some of these calls can name would be
        // filed as unverifiable and cost an extra UAC prompt.
        let mut child = std::process::Command::new("cmd")
            .args(["/c", "ping -n 30 127.0.0.1 > nul"])
            .stdout(std::process::Stdio::null())
            .spawn()
            .expect("start cmd.exe");
        let pid = child.id();
        let listed = (0..40).any(|_| {
            let (path, denied) = image_path(pid);
            if !denied && path.to_string_lossy().to_lowercase().ends_with("cmd.exe") {
                true
            } else {
                std::thread::sleep(std::time::Duration::from_millis(50));
                false
            }
        });
        let _ = child.kill();
        let _ = child.wait();
        assert!(listed, "cmd.exe (pid {pid}) could not be named");
    }

    #[test]
    fn an_image_path_longer_than_the_buffer_still_comes_back() {
        // `IMAGE_PATH_CHARS` is a starting size, not a limit: a path that does not
        // fit is asked for again with more room instead of being reported as
        // unverifiable.
        let handle =
            unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, std::process::id()) };
        assert!(!handle.is_null(), "cannot open this process");
        let path = read_image_path(handle, 4);
        unsafe { CloseHandle(handle) };
        assert!(!path.1, "the path was not read at all: {path:?}");
        assert!(path.0.is_file(), "{path:?} is not the test binary");
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

    #[test]
    fn paths_are_compared_the_way_windows_resolves_them() {
        let dir = std::env::temp_dir().join("mihomo-tray-test-same-image");
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        let file = dir.join("mihomo.exe");
        std::fs::write(&file, b"x").unwrap();

        // The same file spelled differently is the same kernel.
        let shouted = PathBuf::from(file.to_string_lossy().to_uppercase());
        assert!(same_image(&file, &shouted), "{file:?} vs {shouted:?}");
        // ... and so is a path that takes a detour through `..`.
        let detour = dir.join("sub").join("..").join("mihomo.exe");
        assert!(same_image(&file, &detour), "{file:?} vs {detour:?}");

        // A different image is a different kernel.
        let other = dir.join("other.exe");
        std::fs::write(&other, b"x").unwrap();
        assert!(!same_image(&file, &other));
        // A file that is already gone still equals the path recorded for it.
        let gone = dir.join("gone.exe");
        assert!(same_image(&gone, &gone));
        assert!(!same_image(&gone, &file));

        let _ = std::fs::remove_file(&file);
        let _ = std::fs::remove_file(&other);
    }

    fn process(pid: u32, parent: u32, path: &str, denied: bool) -> Process {
        Process {
            pid,
            parent,
            path: PathBuf::from(path),
            denied,
        }
    }

    #[test]
    fn a_launcher_and_the_kernel_it_started_are_one_family() {
        const SHIM: &str = r"D:\App\Scoop\shims\mihomo.exe";
        const KERNEL: &str = r"D:\App\Scoop\apps\mihomo-v3\current\mihomo.exe";
        let processes = vec![
            process(10, 999, SHIM, false),
            process(11, 10, KERNEL, false),
            process(12, 999, r"C:\other\mihomo.exe", false),
        ];
        // Handed the launcher, the kernel under it goes too — this is what the
        // elevated helper used to miss, leaving a kernel holding every port.
        assert_eq!(family(&processes, &[10]), vec![10, 11]);
        // Handed the kernel, the launcher above it comes along, so no shim is
        // left waiting for a child that is gone.
        assert_eq!(family(&processes, &[11]), vec![11, 10]);
        // An unrelated instance is never dragged in.
        assert!(!family(&processes, &[10]).contains(&12));
    }

    #[test]
    fn a_scoop_shim_is_recognised_as_a_launcher() {
        assert!(is_shim(Path::new(r"D:\App\Scoop\shims\mihomo.exe")));
        assert!(!is_shim(Path::new(
            r"D:\App\Scoop\apps\mihomo-v3\current\mihomo.exe"
        )));
        // Only a directory named `shims` is the launcher's; a name that merely
        // contains it is an ordinary kernel.
        assert!(!is_shim(Path::new(r"D:\App\Scoop\shims-backup\mihomo.exe")));
    }

    #[test]
    fn a_process_that_exited_is_already_stopped() {
        // The PID is released before this runs, which is exactly the race between
        // listing the kernels and terminating them: a gone process must count as
        // stopped, not as a failure that aborts the rest.
        let mut child = Command::new("cmd.exe")
            .args(["/c", "exit"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn a short-lived process");
        let pid = child.id();
        child.wait().expect("wait for it to exit");
        // The handle `Child` holds keeps the process object alive, and with it the
        // PID: only once it is closed is this PID as gone as the one a stale
        // listing hands to `kill`.
        drop(child);
        let outcome = kill(pid);
        assert!(
            matches!(outcome, Ok(())),
            "kill of an exited pid: {outcome:?}"
        );
    }

    #[test]
    fn an_argument_survives_the_round_trip_through_the_command_line() {
        // `CommandLineToArgvW` is the parser Windows itself uses, and the one the
        // helper reads a running kernel's arguments back with: what it returns has
        // to be what was handed in, or a restart hands the kernel different
        // arguments than the ones it is running with.
        let args = [
            "-d".to_string(),
            r"C:\Program Files\mihomo\data".to_string(),
            r#"--ext-ctl=127.0.0.1:9090"#.to_string(),
            r#"-f"C:\my config.yaml""#.to_string(),
            r"C:\trailing\".to_string(),
            "plain".to_string(),
            "空 格".to_string(),
            String::new(),
        ];
        let line = kernel_command_line(Path::new(r"C:\Program Files\mihomo\mihomo.exe"), &args);
        let parsed = parse_command_line(&String::from_utf16_lossy(&line[..line.len() - 1]));
        assert_eq!(parsed[0], r"C:\Program Files\mihomo\mihomo.exe");
        assert_eq!(parsed[1..], args);
    }

    #[test]
    fn the_helpers_arguments_survive_the_trip_through_shell_execute() {
        // The replacing helper is handed the kernel's whole argument list as one
        // string in `ShellExecuteExW`'s parameters, and reads it back with the same
        // parser: what it ends up starting has to be what the tray resolved. A path
        // that ends in a backslash is where hand-written quoting goes wrong.
        let args = [
            "-d".to_string(),
            r"C:\Program Files\mihomo\data".to_string(),
            "-f".to_string(),
            r"C:\trailing\".to_string(),
            "-secret=with \"quotes\"".to_string(),
            "空 格".to_string(),
            String::new(),
        ];
        let line = quoted_args(&args);
        assert_eq!(parse_command_line(&line), args, "{line}");
        // No argument list at all is still one: the helper's own switch and PID
        // stay the first two words of the parameters either way.
        assert_eq!(quoted_args(&[]), String::new());
    }

    #[test]
    fn the_kernel_is_started_without_a_console_and_can_be_waited_for() {
        // `start` is the only process launcher in the program, and it uses no
        // `std::process::Command`: this drives the `CreateProcessW` path itself,
        // quoting and working directory included.
        let root = std::env::var("SystemRoot").expect("SystemRoot");
        let cmd = PathBuf::from(root).join(r"System32\cmd.exe");
        let mut child =
            start(&cmd, &["/c".to_string(), "exit".to_string()]).expect("start cmd.exe");
        assert!(child.pid() > 0);
        child.wait().expect("wait for it to exit");
    }

    #[test]
    fn a_kernel_handle_reports_running_until_it_is_killed() {
        let child = Command::new("cmd.exe")
            .args(["/c", "ping -n 30 127.0.0.1 > nul"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn a long-lived process");
        let mut child = KernelChild::adopt(child);
        assert!(
            matches!(child.try_wait(), Ok(None)),
            "a live kernel is running"
        );
        child.kill().expect("end it");
        child.wait().expect("it has to disappear");
        assert!(
            matches!(child.try_wait(), Ok(Some(_))),
            "a killed kernel is gone"
        );
    }
}
