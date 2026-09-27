//! Application state and the worker thread that talks to the controller.
//!
//! The UI thread never performs HTTP: it renders from the shared snapshot and
//! posts commands to the worker, which refreshes the snapshot and pokes the UI
//! window when it is done.

use std::path::PathBuf;
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
    /// Re-read everything (used after actions handled on the UI thread).
    Refresh,
}

pub struct App {
    pub settings: Settings,
    pub state: Shared,
    pub hwnd: HWND,
    pub icons: Icons,
    pub admin: bool,
    pub menu_open: bool,
    pub kernel_path: Option<PathBuf>,
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
            admin: win::elevate::is_admin(),
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

    /// Create the worker and register the tray icon; call once the window exists.
    pub fn start(&mut self, client: Client) {
        match spawn_worker(
            client,
            self.state.clone(),
            self.hwnd as isize,
            self.settings.clone(),
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
            Action::RestartAsAdmin => {
                // Hand the single-instance guard over before the elevated copy
                // starts, otherwise it would see the mutex as taken and exit
                // without a tray icon.
                crate::instance::release();
                match win::elevate::relaunch_as_admin() {
                    Ok(()) => self.shutdown(),
                    Err(error) => {
                        crate::instance::acquire();
                        self.set_error(error);
                    }
                }
            }
            Action::ExitStopKernel => {
                if let Err(error) = self.stop_kernel() {
                    self.set_error(error);
                }
                self.shutdown();
            }
            Action::ExitOnly => self.shutdown(),
        }
    }

    /// Stop only the kernel instance this program is responsible for. Fails
    /// closed: an unknown kernel path or an unknown process path is never
    /// treated as ours, so we cannot kill a mihomo the user started elsewhere.
    fn stop_kernel(&mut self) -> Result<(), String> {
        if let Some(mut child) = self.kernel.take() {
            let _ = child.kill();
            let _ = child.wait();
            return Ok(());
        }
        let Some(path) = self.kernel_path.clone() else {
            return Err(i18n::t().error_no_kernel_path.to_string());
        };
        let mut stopped = 0usize;
        for process in proc::list_mihomo() {
            if process.path.is_empty() {
                // OpenProcess was denied (e.g. an elevated instance): ownership
                // cannot be proven, so leave it alone.
                continue;
            }
            if process.path == path {
                proc::kill(process.pid)?;
                stopped += 1;
            }
        }
        if stopped == 0 {
            return Err(i18n::t().error_no_matching_kernel.to_string());
        }
        Ok(())
    }

    fn shutdown(&mut self) {
        win::remove_icon(self.hwnd);
        win::quit(self.hwnd);
    }
}

/// What the last user action did, if anything: `None` means "no new action",
/// `Some(None)` a success (which clears the previous error) and `Some(Some(_))`
/// a failure.
type Outcome = Option<Option<String>>;

fn spawn_worker(
    client: Client,
    state: Shared,
    hwnd: isize,
    settings: Settings,
    fatal: Option<String>,
) -> Result<Sender<Command>, String> {
    let (tx, rx) = mpsc::channel::<Command>();
    let poll = Duration::from_millis(settings.poll_interval_ms as u64);
    std::thread::Builder::new()
        .name("mihomo-poll".into())
        .stack_size(256 * 1024)
        .spawn(move || {
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
                    Ok(command) => outcome = Some(execute(&client, &command)),
                    Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => break,
                }
            }
        })
        .map(|_| tx)
        .map_err(|error| i18n::t().error_spawn_worker(&error.to_string()))
}

fn execute(client: &Client, command: &Command) -> Option<String> {
    let result = match command {
        Command::SetMode(mode) => client.set_mode(mode),
        Command::SetTun(enable) => client.set_tun(*enable),
        Command::Select { group, member } => client.select(group, member),
        Command::Unfix(group) => client.unfix(group),
        Command::Reload => client.reload(),
        Command::Refresh => Ok(()),
    };
    result.err()
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
