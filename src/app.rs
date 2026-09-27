//! Application state and the worker thread that talks to the controller.
//!
//! The UI thread never performs HTTP: it renders from the shared snapshot and
//! posts commands to the worker, which refreshes the snapshot and pokes the UI
//! window when it is done.

use std::path::Path;
use std::sync::Arc;
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::time::Duration;

use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::WindowsAndMessaging::PostMessageW;

use crate::i18n;
use crate::icon::{Icons, State};
use crate::mihomo::{Client, discover, proc};
use crate::settings::Settings;
use crate::state::{self, KernelSlot, Shared, Snapshot};
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
    /// No `mihomo.exe` is left running.
    Gone,
    /// A kernel is still running that this process cannot end on its own: it runs
    /// with more rights, or it cannot even be recognised from here.
    NeedsAdmin,
    Refused(String),
}

pub struct App {
    pub settings: Settings,
    pub state: Shared,
    pub hwnd: HWND,
    pub icons: Icons,
    pub menu_open: bool,
    /// The kernel this program may start or replace, as the worker found it: the
    /// path is what "exit and stop mihomo" matches against, and the handle is the
    /// instance this program launched itself.
    kernel: KernelSlot,
    tx: Option<Sender<Command>>,
    fatal: Option<String>,
    /// `NIM_ADD` can fail before the shell is ready (e.g. at logon), so keep
    /// retrying from the poll loop until it sticks.
    icon_ready: bool,
}

impl App {
    pub fn new(settings: Settings, state: Shared, fatal: Option<String>) -> Self {
        Self {
            settings,
            state,
            hwnd: std::ptr::null_mut(),
            icons: Icons::new(),
            menu_open: false,
            kernel: state::kernel_slot(),
            tx: None,
            fatal,
            icon_ready: false,
        }
    }

    /// Register the tray icon and start the worker; call once the window exists.
    ///
    /// The icon is the thing the user waits for, so it comes first and nothing
    /// about the kernel runs on this thread: finding the kernel, starting it and
    /// waiting for it to answer are the worker's first job. The status note the
    /// worker publishes is what the tooltip and the menu show while that happens.
    pub fn start(&mut self) {
        self.refresh_ui();
        match spawn_worker(
            self.state.clone(),
            self.hwnd as isize,
            self.settings.clone(),
            Arc::clone(&self.kernel),
            self.fatal.take(),
        ) {
            Ok(tx) => self.tx = Some(tx),
            Err(error) => self.set_error(error),
        }
    }

    pub fn refresh_ui(&mut self) {
        let snapshot = state::read(&self.state);
        let icon = self.icons.for_state(icon_state(&snapshot));
        if self.icon_ready {
            win::update_icon(self.hwnd, icon, &tooltip(&snapshot));
        } else {
            self.icon_ready = win::add_icon(self.hwnd, icon, &tooltip(&snapshot));
        }
    }
    pub fn on_taskbar_created(&mut self) {
        let snapshot = state::read(&self.state);
        let icon = self.icons.for_state(icon_state(&snapshot));
        self.icon_ready = win::add_icon(self.hwnd, icon, &tooltip(&snapshot));
    }

    fn send(&self, command: Command) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(command);
        }
    }

    pub fn set_error(&self, message: impl Into<String>) {
        state::write_action_error(&self.state, message.into());
        post(self.hwnd as isize, win::WM_REFRESH);
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
                // Nothing is running any more: leave.
                StopResult::Gone => self.shutdown(),
                // A kernel is still running that this process has no rights over,
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
    ///
    /// Two things are never reported as success, because only the elevated helper
    /// can settle them: a kernel this process may not end (it runs with more
    /// rights), and one whose image path cannot even be read (it might be ours).
    /// A kernel whose image is readable and different is somebody else's, and is
    /// left alone rather than dragged into an elevation prompt.
    fn stop_kernel(&mut self) -> StopResult {
        let child = state::take_kernel_child(&self.kernel);
        // The kernel this program replaced is known by PID, and it is the one case
        // that needs no guessing: an elevated process cannot be recognised from
        // here by path, and this process has no rights over it either — only the
        // helper can end it.
        if let Some(pid) = state::read(&self.state).kernel_pid {
            if proc::list_mihomo().iter().any(|process| process.pid == pid) {
                return StopResult::NeedsAdmin;
            }
            // The kernel it replaced is gone: the recorded PID is stale.
            state::write_kernel_pid(&self.state, None);
        }
        // The discovered path is the only path this program knows: the kernel it
        // started itself came from it, and `find_kernel` prefers a real binary over
        // a scoop shim.
        if let Some(path) = state::kernel_path(&self.kernel) {
            match proc::stop_kernel(None, &path) {
                Err(error) => return StopResult::Refused(error),
                Ok(outcome) if outcome.denied > 0 => return StopResult::NeedsAdmin,
                Ok(_) => {}
            }
        }
        // The handle is only evidence while the process it points at is running:
        // after an elevated restart it refers to a kernel that is already gone.
        if let Some(mut child) = child {
            if matches!(child.try_wait(), Ok(None)) {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
        // Nothing of ours is left: anything still running either has a readable
        // image that is not ours, or was refused above.
        StopResult::Gone
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

/// How long a kernel this program started gets to answer the controller.
const KERNEL_STARTUP_BUDGET: Duration = Duration::from_secs(5);

/// Everything an action needs besides the command itself.
struct Worker<'a> {
    client: &'a Client,
    state: &'a Shared,
    hwnd: isize,
    /// The kernel this program may start or replace.
    kernel: &'a KernelSlot,
}

/// What bringing the kernel up left behind for the poll loop.
struct Startup {
    /// The controller to poll.
    client: Client,
    /// The kernel was started here, so it is worth waiting for it to answer.
    started_kernel: bool,
    /// Why the kernel could not be brought up, when it could not.
    error: Option<String>,
}

/// Bring the kernel up: say that it is being looked for, then look and start it.
///
/// This is the part of startup that talks to the outside world, and it runs on
/// the worker thread: the tray icon is registered before it, so a kernel that
/// needs seconds to come up no longer holds back what the user sees. The search
/// itself takes a moment on a machine where a dead address answers slowly, so it
/// says what it is doing before it starts — every later phase publishes a note of
/// its own, and the ones that do not (nothing found, a kernel already running) go
/// back to the status line and the error, which are the honest answer there.
fn startup(kernel: &KernelSlot, settings: &Settings, state: &Shared, hwnd: isize) -> Startup {
    show_note(
        state,
        hwnd,
        Some(i18n::t().status_looking_kernel.to_string()),
    );
    let startup = look_for_kernel(kernel, settings);
    if !startup.started_kernel {
        show_note(state, hwnd, None);
    }
    startup
}

/// The search itself, and the decision to start the kernel this program owns.
fn look_for_kernel(kernel: &KernelSlot, settings: &Settings) -> Startup {
    let path = discover::find_kernel(settings);
    // Recorded before anything runs: "exit and stop mihomo" matches the kernel it
    // may stop against this path, including one this program did not start.
    state::write_kernel_path(kernel, path.clone());
    let client = discover::find_controller(settings).unwrap_or_else(|| {
        Client::new("127.0.0.1:9090", "", settings.controller_timeout_ms)
            .expect("default controller address is always valid")
    });
    let mut startup = Startup {
        client,
        started_kernel: false,
        error: None,
    };
    if startup.client.alive() || !settings.mihomo_auto_start {
        return startup;
    }
    let Some(path) = path else {
        startup.error = Some(i18n::t().error_kernel_not_found.to_string());
        return startup;
    };
    // A kernel this program did not start is not replaced on a guess: its own
    // controller may simply not be up yet.
    if !proc::list_mihomo().is_empty() {
        return startup;
    }
    match proc::start(&path, &discover::launch_args(settings)) {
        Ok(child) => {
            state::write_kernel_child(kernel, child);
            startup.started_kernel = true;
        }
        Err(error) => startup.error = Some(error),
    }
    startup
}

/// Spawn the worker: it brings the kernel up, then keeps the snapshot fresh and
/// carries out the commands the UI thread posts.
fn spawn_worker(
    state: Shared,
    hwnd: isize,
    settings: Settings,
    kernel: KernelSlot,
    fatal: Option<String>,
) -> Result<Sender<Command>, String> {
    let (tx, rx) = mpsc::channel::<Command>();
    let poll = Duration::from_millis(settings.poll_interval_ms as u64);
    std::thread::Builder::new()
        .name("mihomo-poll".into())
        .stack_size(256 * 1024)
        .spawn(move || {
            let startup = startup(&kernel, &settings, &state, hwnd);
            let worker = Worker {
                client: &startup.client,
                state: &state,
                hwnd,
                kernel: &kernel,
            };
            // The kernel's own failure is the more specific one, so it wins over
            // a settings file that could not be read.
            let mut outcome: Outcome = startup.error.map(Some).or_else(|| fatal.map(Some));
            // Set while the kernel this program started has not answered yet: the
            // note stays up until it does, so a kernel that needs longer than the
            // budget does not look like a tray that never came up.
            let mut waiting_for_kernel = false;
            if startup.started_kernel {
                // Say what is going on before the bounded wait: the icon is
                // registered already, and this is what the user sees until the
                // kernel answers.
                state::write_kernel_started(&state, i18n::t().status_starting_kernel.to_string());
                post(hwnd, win::WM_REFRESH);
                if wait_for_controller(worker.client) {
                    show_note(&state, hwnd, None);
                } else {
                    show_note(
                        &state,
                        hwnd,
                        Some(i18n::t().status_kernel_silent.to_string()),
                    );
                    waiting_for_kernel = true;
                }
            }
            let mut version = String::new();
            loop {
                // Refresh first so the very first menu the user opens already has
                // real data, then wait either for a command or the poll interval.
                refresh(worker.client, &state, &mut version, outcome.take());
                post(hwnd, win::WM_REFRESH);
                if waiting_for_kernel {
                    // Over as soon as there is an answer — or as soon as the kernel
                    // is gone, in which case "still waiting" would be a lie.
                    let snapshot = state::read(&state);
                    if snapshot.controller_ok || !snapshot.kernel_running {
                        show_note(&state, hwnd, None);
                        waiting_for_kernel = false;
                    }
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
    let Some(pid) = our_kernel(worker) else {
        return Some(i18n::t().error_no_kernel_path.to_string());
    };
    let params = win::elevate::kernel_start_params(pid);

    // The helper blocks until it is done, and the UAC prompt is part of that, so
    // say what is happening before waiting; the note is cleared right after.
    show_note(
        worker.state,
        worker.hwnd,
        Some(i18n::t().status_elevating_kernel.to_string()),
    );
    let elevated = win::elevate::run_self_elevated(&params);
    show_note(worker.state, worker.hwnd, None);
    let code = match elevated {
        Ok(code) => code,
        Err(error) => return Some(error),
    };
    if code < 0 {
        return Some(helper_failure(code));
    }
    // A positive answer is the kernel the helper started: the tray cannot read an
    // elevated process's image path, so this PID is the identity it keeps — and
    // what "exit and stop mihomo" hands back to the helper later.
    state::write_kernel_pid(worker.state, (code > 0).then_some(code as u32));

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
    let Some(pid) = our_kernel(worker) else {
        return Some(i18n::t().error_no_kernel_path.to_string());
    };
    let params = win::elevate::kernel_stop_params(pid);

    show_note(
        worker.state,
        worker.hwnd,
        Some(i18n::t().status_elevating_stop.to_string()),
    );
    let elevated = win::elevate::run_self_elevated(&params);
    show_note(worker.state, worker.hwnd, None);
    match elevated {
        Err(error) => Some(error),
        // `HELPER_NOT_STOPPED` means the helper, which can see every kernel,
        // found none of ours: the process still running is somebody else's, and
        // the user asked to leave.
        Ok(proc::HELPER_OK | proc::HELPER_NOT_STOPPED) => {
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

/// The kernel this program is responsible for, out of everything that is running.
///
/// One answer, two purposes (replacing it for TUN, ending it): the identity is the
/// same either way, and a wrong answer would be equally wrong for both.
///
/// * the PID the elevated helper reported comes first — an elevated process's
///   image path cannot be read from here, so a PID is the only identity the tray
///   holds — and a PID that is no longer in the process list is stale;
/// * then the path this program knows, compared through `proc::same_image` so a
///   scoop junction and the versioned directory it points at count as one kernel,
///   and preferring a real binary over the shim that carries the same name;
/// * then a kernel whose image cannot be read at all, which is what an elevated
///   kernel replaced by an earlier TUN enable looks like after a tray restart.
///
/// A *readable* kernel with a different image is somebody else's and is never
/// guessed at: replacing or ending it would be worse than reporting that ours was
/// not found.
fn pick_kernel(
    running: &[proc::Process],
    recorded: Option<u32>,
    known: Option<&Path>,
) -> Option<u32> {
    let matches = |path: &Path, skip_shims: bool| {
        running
            .iter()
            .find(|process| {
                !process.denied
                    && !(skip_shims && proc::is_shim(&process.path))
                    && proc::same_image(&process.path, path)
            })
            .map(|process| process.pid)
    };
    recorded
        .filter(|pid| running.iter().any(|process| process.pid == *pid))
        .or_else(|| known.and_then(|path| matches(path, true).or_else(|| matches(path, false))))
        .or_else(|| {
            running
                .iter()
                .find(|process| process.denied)
                .map(|process| process.pid)
        })
}

/// The kernel this program is responsible for right now.
fn our_kernel(worker: &Worker) -> Option<u32> {
    let path = state::kernel_path(worker.kernel);
    pick_kernel(
        &proc::list_mihomo(),
        state::read(worker.state).kernel_pid,
        path.as_deref(),
    )
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

/// Wait, bounded, for a kernel this program just started to answer.
///
/// Returns whether it answered within the budget. The kernel is probed where its
/// own configuration says it listens — the client that was discovered — with
/// `Client::alive` asking its own short question, so one iteration cannot take
/// the configured request timeout while the kernel is still coming up.
fn wait_for_controller(client: &Client) -> bool {
    let deadline = std::time::Instant::now() + KERNEL_STARTUP_BUDGET;
    while std::time::Instant::now() < deadline {
        if client.alive() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    false
}

/// Poke the UI window with a message it handles on its own thread.
fn post(hwnd: isize, message: u32) {
    unsafe {
        PostMessageW(hwnd as HWND, message, 0, 0);
    }
}

/// Publish what the worker is doing right now, so a menu the user opens while it
/// blocked says something.
fn show_note(state: &Shared, hwnd: isize, message: Option<String>) {
    state::write_status_note(state, message);
    post(hwnd, win::WM_REFRESH);
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
        kernel_pid: previous.kernel_pid,
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

/// What the icon shows, from what the last refresh saw.
///
/// TUN beats the system proxy and both beat a kernel that only answers. A kernel
/// that does not answer beats everything: a registry entry claiming the system
/// proxy is on while nothing serves it is exactly what the user has to see.
fn icon_state(snapshot: &Snapshot) -> State {
    if !snapshot.controller_ok {
        State::Unreachable
    } else if snapshot.tun {
        State::Tun
    } else if snapshot.sysproxy {
        State::SystemProxy
    } else {
        State::Ready
    }
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

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn the_icon_shows_the_strongest_state() {
        let dead = Snapshot::default();
        assert_eq!(icon_state(&dead), State::Unreachable);

        let ready = Snapshot {
            controller_ok: true,
            ..Default::default()
        };
        assert_eq!(icon_state(&ready), State::Ready);

        let proxied = Snapshot {
            controller_ok: true,
            sysproxy: true,
            ..Default::default()
        };
        assert_eq!(icon_state(&proxied), State::SystemProxy);

        let tunnelled = Snapshot {
            controller_ok: true,
            sysproxy: true,
            tun: true,
            ..Default::default()
        };
        assert_eq!(icon_state(&tunnelled), State::Tun);

        // What the registry says proves nothing while the kernel is not answering.
        let lying = Snapshot {
            sysproxy: true,
            tun: true,
            ..Default::default()
        };
        assert_eq!(icon_state(&lying), State::Unreachable);
    }

    fn process(pid: u32, path: &str, denied: bool) -> proc::Process {
        proc::Process {
            pid,
            parent: 0,
            path: PathBuf::from(path),
            denied,
        }
    }

    const KERNEL: &str = r"D:\App\Scoop\apps\mihomo-v3\current\mihomo.exe";
    const STRANGER: &str = r"C:\other\mihomo.exe";
    const SHIM: &str = r"D:\App\Scoop\shims\mihomo.exe";

    #[test]
    fn the_kernel_is_picked_by_pid_then_by_path_then_by_being_unreadable() {
        let running = vec![process(1, KERNEL, false), process(2, STRANGER, false)];
        let ours = Some(Path::new(KERNEL));
        assert_eq!(pick_kernel(&running, None, ours), Some(1));

        // Nothing of ours is running: a readable stranger is never picked, not
        // even when it is the only kernel there is.
        let elsewhere = Some(Path::new(r"C:\nowhere\mihomo.exe"));
        assert_eq!(pick_kernel(&running, None, elsewhere), None);
        let single = vec![process(4, STRANGER, false)];
        assert_eq!(pick_kernel(&single, None, elsewhere), None);
        assert_eq!(pick_kernel(&single, None, None), None);

        // A kernel whose image cannot be read might be ours: that is what an
        // elevated one replaced by TUN looks like after a tray restart.
        let unreadable = vec![process(3, "", true)];
        assert_eq!(pick_kernel(&unreadable, None, elsewhere), Some(3));
        assert_eq!(pick_kernel(&unreadable, None, None), Some(3));

        // A launcher is used when the real kernel under it is not running.
        let launcher = vec![process(5, SHIM, false), process(6, STRANGER, false)];
        assert_eq!(pick_kernel(&launcher, None, Some(Path::new(SHIM))), Some(5));
    }

    #[test]
    fn the_kernel_the_helper_started_wins_by_pid() {
        let running = vec![process(1, KERNEL, false), process(2, STRANGER, false)];
        let ours = Some(Path::new(KERNEL));
        // The PID the helper reported is the kernel this program owns, even when
        // its image path points somewhere else entirely.
        assert_eq!(pick_kernel(&running, Some(2), ours), Some(2));
        // A PID that is no longer running is stale and must be ignored.
        assert_eq!(pick_kernel(&running, Some(99), ours), Some(1));
        assert_eq!(pick_kernel(&running, Some(0), ours), Some(1));
    }
}
