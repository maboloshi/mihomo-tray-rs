//! Application state and the worker thread that talks to the controller.
//!
//! The UI thread never performs HTTP: it renders from the shared snapshot and
//! posts commands to the worker, which refreshes the snapshot and pokes the UI
//! window when it is done.

use std::path::{Path, PathBuf};
use std::process::Child;
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::time::Duration;

use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::WindowsAndMessaging::PostMessageW;

use crate::i18n;
use crate::icon::Icons;
use crate::mihomo::{Client, proc};
use crate::settings::Settings;
use crate::state::{self, Shared, Snapshot};
use crate::win::{self, menu::Action};

#[derive(Debug, Clone)]
pub enum Command {
    SetMode(String),
    SetTun(bool),
    Select {
        group: String,
        member: String,
    },
    Unfix(String),
    Reload,
    /// Ask for the rights to stop a kernel that is not ours to end.
    StopKernelElevated,
    /// Re-read everything (used after actions handled on the UI thread).
    Refresh,
}

/// How "exit and stop mihomo" can end.
enum StopResult {
    Stopped,
    /// No process matched the known kernel path.
    Absent,
    /// A kernel is running that this process is not allowed to inspect.
    NeedsAdmin,
    Refused(String),
}

pub struct App {
    pub settings: Settings,
    pub state: Shared,
    pub hwnd: HWND,
    pub icons: Icons,
    pub menu_open: bool,
    pub kernel_path: Option<PathBuf>,
    /// The kernel this program started itself, so "exit and stop mihomo" can end
    /// it directly. Cleared once an elevated helper takes the kernel over.
    kernel: Option<Child>,
    tx: Option<Sender<Command>>,
    fatal: Option<String>,
    /// `NIM_ADD` can fail before the shell is ready (e.g. at logon), so keep
    /// retrying from the poll loop until it sticks.
    icon_ready: bool,
}

impl App {
    pub fn new(
        settings: Settings,
        state: Shared,
        kernel_path: Option<PathBuf>,
        fatal: Option<String>,
    ) -> Self {
        Self {
            settings,
            state,
            hwnd: std::ptr::null_mut(),
            icons: Icons::new(),
            menu_open: false,
            kernel_path,
            kernel: None,
            tx: None,
            fatal,
            icon_ready: false,
        }
    }

    pub fn set_kernel(&mut self, child: Child) {
        self.kernel = Some(child);
    }

    /// The kernel was replaced by the elevated helper, so the child handle is
    /// stale: from now on the kernel is only ever matched by path.
    pub fn forget_kernel(&mut self) {
        self.kernel = None;
    }

    /// Create the worker and register the tray icon; call once the window exists.
    pub fn start(&mut self, client: Client) {
        match spawn_worker(
            client,
            self.state.clone(),
            self.hwnd as isize,
            self.settings.clone(),
            self.kernel_path.clone(),
            self.fatal.take(),
        ) {
            Ok(tx) => self.tx = Some(tx),
            Err(error) => self.set_error(error),
        }
        let snapshot = state::read(&self.state);
        self.icon_ready = win::add_icon(
            self.hwnd,
            self.icons.for_state(snapshot.tun, self.proxying(&snapshot)),
            &tooltip(&snapshot),
        );
    }

    fn proxying(&self, snapshot: &Snapshot) -> bool {
        snapshot.sysproxy && snapshot.controller_ok
    }

    pub fn refresh_ui(&mut self) {
        let snapshot = state::read(&self.state);
        let icon = self.icons.for_state(snapshot.tun, self.proxying(&snapshot));
        if self.icon_ready {
            win::update_icon(self.hwnd, icon, &tooltip(&snapshot));
        } else {
            self.icon_ready = win::add_icon(self.hwnd, icon, &tooltip(&snapshot));
        }
    }
    pub fn on_taskbar_created(&mut self) {
        let snapshot = state::read(&self.state);
        let icon = self.icons.for_state(snapshot.tun, self.proxying(&snapshot));
        self.icon_ready = win::add_icon(self.hwnd, icon, &tooltip(&snapshot));
    }

    fn send(&self, command: Command) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(command);
        }
    }

    pub fn set_error(&self, message: impl Into<String>) {
        state::write_action_error(&self.state, message.into());
        unsafe {
            PostMessageW(self.hwnd, win::WM_REFRESH, 0, 0);
        }
    }

    pub fn dispatch(&mut self, action: &Action) {
        match action {
            Action::SetMode(mode) => self.send(Command::SetMode(mode.clone())),
            Action::ToggleTun => {
                let enable = !state::read(&self.state).tun;
                self.send(Command::SetTun(enable));
            }
            Action::Select { group, member } => self.send(Command::Select {
                group: group.clone(),
                member: member.clone(),
            }),
            Action::Unfix(group) => self.send(Command::Unfix(group.clone())),
            Action::Reload => self.send(Command::Reload),
            Action::ToggleSysProxy => {
                let snapshot = state::read(&self.state);
                let port = if snapshot.mixed_port > 0 {
                    snapshot.mixed_port
                } else {
                    7890
                };
                let result = if snapshot.sysproxy {
                    win::proxy::disable()
                } else {
                    win::proxy::enable(port, &self.settings.proxy_bypass)
                };
                if let Err(error) = result {
                    self.set_error(error);
                } else {
                    self.send(Command::Refresh);
                }
            }
            Action::ToggleAutostart => {
                let enabled = win::autostart::is_enabled();
                if let Err(error) = win::autostart::set(!enabled) {
                    self.set_error(error);
                } else {
                    self.send(Command::Refresh);
                }
            }
            Action::ExitStopKernel => match self.stop_kernel() {
                // Ours to end (or nothing to end): leave right away.
                StopResult::Stopped | StopResult::Absent => self.shutdown(),
                // An elevated kernel can only be stopped with the rights it has,
                // so the worker asks for them; the process stays alive until it
                // knows whether that worked.
                StopResult::NeedsAdmin => self.send(Command::StopKernelElevated),
                // Keeping the tray alive is the point: the user has to see why
                // nothing was stopped, and can still pick "exit only".
                StopResult::Refused(error) => self.set_error(error),
            },
            Action::ExitOnly => self.shutdown(),
        }
    }

    /// Stop only the kernel instance this program is responsible for. Fails
    /// closed: an unknown kernel path or an unknown process path is never
    /// treated as ours, so we cannot kill a mihomo the user started elsewhere.
    fn stop_kernel(&mut self) -> StopResult {
        let child = self.kernel.take();
        // The recorded target wins: it is the image path this program actually
        // started or replaced, while the discovered path is only a guess.
        let target = state::read(&self.state)
            .kernel_target
            .or_else(|| self.kernel_path.clone());
        let Some(path) = target else {
            // Without a known path the only process we can end is the one this
            // program started itself.
            return match child {
                Some(mut child) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    StopResult::Stopped
                }
                None => StopResult::Refused(i18n::t().error_no_kernel_path.to_string()),
            };
        };
        // Path matching covers both the kernel this program started and an
        // elevated one that replaced it; a stale child handle would otherwise
        // report success without stopping anything.
        match proc::stop_matching(&path) {
            Err(error) => StopResult::Refused(error),
            Ok(outcome) if outcome.stopped > 0 => StopResult::Stopped,
            // A kernel we are not allowed to end: ask for the rights.
            Ok(outcome) if outcome.denied > 0 => StopResult::NeedsAdmin,
            // Nothing matched by path: fall back to the handle we still hold.
            Ok(_) => match child {
                Some(mut child) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    StopResult::Stopped
                }
                None => StopResult::Absent,
            },
        }
    }

    pub fn shutdown(&mut self) {
        win::remove_icon(self.hwnd);
        win::quit(self.hwnd);
    }
}

/// What the last user action did, if anything: `None` means "no new action",
/// `Some(None)` a success (which clears the previous error) and `Some(Some(_))`
/// a failure.
type Outcome = Option<Option<String>>;

/// How long a just-restarted kernel may take to answer again.
const KERNEL_START_BUDGET: Duration = Duration::from_secs(10);

/// Everything an action needs besides the command itself.
struct Worker<'a> {
    client: &'a Client,
    state: &'a Shared,
    hwnd: isize,
    /// The kernel this program may start or replace.
    kernel_path: Option<&'a Path>,
}

fn spawn_worker(
    client: Client,
    state: Shared,
    hwnd: isize,
    settings: Settings,
    kernel_path: Option<PathBuf>,
    fatal: Option<String>,
) -> Result<Sender<Command>, String> {
    let (tx, rx) = mpsc::channel::<Command>();
    let poll = Duration::from_millis(settings.poll_interval_ms as u64);
    std::thread::Builder::new()
        .name("mihomo-poll".into())
        .stack_size(256 * 1024)
        .spawn(move || {
            let worker = Worker {
                client: &client,
                state: &state,
                hwnd,
                kernel_path: kernel_path.as_deref(),
            };
            let mut outcome: Outcome = fatal.map(Some);
            let mut version = String::new();
            loop {
                // Refresh first so the very first menu the user opens already has
                // real data, then wait either for a command or the poll interval.
                refresh(&client, &state, &mut version, outcome.take());
                unsafe {
                    PostMessageW(hwnd as HWND, win::WM_REFRESH, 0, 0);
                }
                match rx.recv_timeout(poll) {
                    Ok(command) => outcome = Some(execute(&worker, &command)),
                    Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => break,
                }
            }
        })
        .map(|_| tx)
        .map_err(|error| i18n::t().error_spawn_worker(&error.to_string()))
}

fn execute(worker: &Worker, command: &Command) -> Option<String> {
    let result = match command {
        Command::SetMode(mode) => worker.client.set_mode(mode),
        // TUN is not a plain request: see `set_tun`.
        Command::SetTun(enable) => return set_tun(worker, *enable),
        Command::Select { group, member } => worker.client.select(group, member),
        Command::Unfix(group) => worker.client.unfix(group),
        Command::Reload => worker.client.reload(),
        Command::StopKernelElevated => return stop_kernel_elevated(worker),
        Command::Refresh => Ok(()),
    };
    result.err()
}

/// Turn TUN on or off.
///
/// The controller answers `204` even when the kernel could not create the
/// adapter — it only writes a log line — so the request itself proves nothing
/// and the read-back decides. A kernel that answers while TUN stays off is a
/// kernel without administrator rights, which is the one thing TUN needs: it is
/// replaced by an elevated one (a second, short-lived process of this program,
/// started through the UAC prompt) and the request is repeated.
fn set_tun(worker: &Worker, enable: bool) -> Option<String> {
    if !enable {
        return worker.client.set_tun(false).err();
    }
    if let Err(error) = worker.client.set_tun(true) {
        return Some(error);
    }
    if read_tun(worker.client) == Some(true) {
        return None;
    }
    let Some(pid) = kernel_pid(worker) else {
        return Some(i18n::t().error_no_kernel_path.to_string());
    };
    let params = win::elevate::kernel_start_params(pid);

    // The helper blocks until it is done, and the UAC prompt is part of that, so
    // say what is happening before waiting; the note is cleared right after.
    show_note(worker, Some(i18n::t().status_elevating_kernel.to_string()));
    let elevated = win::elevate::run_self_elevated(&params);
    show_note(worker, None);
    // The prompt was cancelled, or it timed out while the helper was still
    // working: in the second case the kernel may already have been replaced, so
    // the child handle the UI thread holds is not worth keeping. From here on the
    // kernel is matched by PID and path instead.
    forget_child(worker);
    let code = match elevated {
        Ok(code) => code,
        Err(error) => return Some(error),
    };
    if code != proc::HELPER_OK {
        return Some(helper_failure(code));
    }

    wait_for_kernel(worker.client);
    if let Err(error) = worker.client.set_tun(true) {
        return Some(error);
    }
    match read_tun(worker.client) {
        Some(true) => None,
        _ => Some(i18n::t().error_tun_ineffective.to_string()),
    }
}

/// The TUN flag as the controller reports it, `None` while it cannot be read.
fn read_tun(client: &Client) -> Option<bool> {
    client.configs().ok().map(|(_, _, tun)| tun)
}

/// Ask for the rights to stop the kernel, and tell the UI thread to leave once it
/// is gone. The UAC prompt blocks this thread, exactly like the TUN restart.
fn stop_kernel_elevated(worker: &Worker) -> Option<String> {
    let Some(pid) = kernel_pid(worker) else {
        return Some(i18n::t().error_no_kernel_path.to_string());
    };
    let params = win::elevate::kernel_stop_params(pid);

    show_note(worker, Some(i18n::t().status_elevating_stop.to_string()));
    let elevated = win::elevate::run_self_elevated(&params);
    show_note(worker, None);
    match elevated {
        Err(error) => Some(error),
        Ok(proc::HELPER_OK) => {
            unsafe {
                PostMessageW(worker.hwnd as HWND, win::WM_EXIT, 0, 0);
            }
            None
        }
        Ok(code) => Some(helper_failure(code)),
    }
}

/// What a failed helper run means to the user. The helper names the reason, so it
/// does not have to be guessed from the exit code at every call site.
fn helper_failure(code: i32) -> String {
    let messages = i18n::t();
    match code {
        proc::HELPER_DENIED => messages.error_kernel_needs_admin.to_string(),
        proc::HELPER_UNREADABLE => messages.error_kernel_args.to_string(),
        proc::HELPER_NOT_STOPPED => messages.error_no_matching_kernel.to_string(),
        _ => messages.error_elevated_kernel_failed.to_string(),
    }
}

/// The PID of the kernel to work on: the image this program recorded first, then
/// the one discovery knows, then whatever is left running.
///
/// A PID is what survives the rights a tray does not have — the image path of an
/// elevated kernel cannot be read from here at all — which is exactly why the
/// elevated helper is handed one instead of a path.
fn kernel_pid(worker: &Worker) -> Option<u32> {
    let running = proc::list_mihomo();
    let matches = |path: &Path| {
        running
            .iter()
            .find(|process| !process.denied && process.path == path)
            .map(|process| process.pid)
    };
    let recorded = state::read(worker.state)
        .kernel_target
        .as_deref()
        .and_then(matches);
    recorded
        .or_else(|| worker.kernel_path.and_then(matches))
        .or_else(|| match running.len() {
            // Nothing matched by path: a single kernel is unambiguous, and among
            // several the higher-integrity one is the one holding the ports.
            1 => Some(running[0].pid),
            _ => running.iter().find(|process| process.denied).map(|p| p.pid),
        })
}

/// The child handle is only worth anything while the process this program started
/// is still the one running: an elevated helper replaces it.
fn forget_child(worker: &Worker) {
    unsafe {
        PostMessageW(worker.hwnd as HWND, win::WM_KERNEL_REPLACED, 0, 0);
    }
}

/// Wait, bounded, for a kernel that was just restarted to answer again.
fn wait_for_kernel(client: &Client) {
    let deadline = std::time::Instant::now() + KERNEL_START_BUDGET;
    while std::time::Instant::now() < deadline {
        if client.alive() {
            return;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// Publish what the worker is doing right now, so a menu the user opens while it
/// blocked says something.
fn show_note(worker: &Worker, message: Option<String>) {
    state::write_status_note(worker.state, message);
    unsafe {
        PostMessageW(worker.hwnd as HWND, win::WM_REFRESH, 0, 0);
    }
}

/// Rebuild the snapshot from the outside world.
///
/// Error ownership is split on purpose: the controller error is set and cleared
/// here (it must disappear once the controller answers again), while an action
/// error carries over until the next action replaces or clears it.
fn refresh(client: &Client, shared: &Shared, version: &mut String, outcome: Outcome) {
    let previous = state::read(shared);
    let mut snapshot = Snapshot {
        kernel_running: !proc::list_mihomo().is_empty(),
        sysproxy: win::proxy::is_enabled(),
        autostart: win::autostart::is_enabled(),
        action_error: previous.action_error,
        status_note: previous.status_note,
        kernel_target: previous.kernel_target,
        ..Default::default()
    };

    match client.configs() {
        Ok((mode, port, tun)) => {
            snapshot.controller_ok = true;
            snapshot.mode = mode;
            snapshot.mixed_port = port;
            snapshot.tun = tun;
            if version.is_empty() {
                // One extra request, only until it succeeds.
                *version = client.version().unwrap_or_default();
            }
            snapshot.version = version.clone();
            if let Ok(groups) = client.proxies() {
                snapshot.groups = groups;
            }
        }
        Err(error) => {
            snapshot.controller_ok = false;
            snapshot.controller_error =
                Some(i18n::t().error_controller_unreachable(&client.address(), &error));
        }
    }

    if let Some(result) = outcome {
        snapshot.action_error = result;
    }

    state::write(shared, snapshot);
}

fn tooltip(snapshot: &Snapshot) -> String {
    let messages = i18n::t();
    let mut text = String::from("mihomo-tray\n");
    text.push_str(&snapshot.status_line());
    if let Some(note) = snapshot.status_note.as_deref() {
        text.push_str(&format!("\n{note}"));
    }
    if snapshot.controller_ok {
        if !snapshot.version.is_empty() {
            text.push_str(&format!("\n{}", messages.tooltip_kernel(&snapshot.version)));
        }
        text.push_str(&format!(
            "\n{}",
            messages.tooltip_proxy_tun(
                if snapshot.sysproxy {
                    &messages.state_on
                } else {
                    &messages.state_off
                },
                if snapshot.tun {
                    &messages.state_on
                } else {
                    &messages.state_off
                }
            )
        ));
        if snapshot.mixed_port > 0 {
            text.push_str(&format!("\n{}", messages.tooltip_port(snapshot.mixed_port)));
        }
    }
    if let Some(error) = snapshot.error() {
        text.push_str(&format!("\n⚠ {error}"));
    }
    // `szTip` is 128 UTF-16 units *including* the terminator, so truncate by
    // code units rather than scalar values and keep one unit free.
    let units: Vec<u16> = text.encode_utf16().take(127).collect();
    String::from_utf16_lossy(&units)
}
