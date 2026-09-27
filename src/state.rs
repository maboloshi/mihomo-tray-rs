//! Shared runtime state. The worker thread writes, the UI thread reads.

use std::path::PathBuf;
use std::process::Child;
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Default)]
pub struct Group {
    pub name: String,
    pub kind: String,
    /// Only the group types whose adapter implements mihomo's `SelectAble` can
    /// be switched: `Selector`, `URLTest` and `Fallback`.
    pub switchable: bool,
    pub now: String,
    /// `/proxies` `fixed`: the member a `URLTest`/`Fallback` group was pinned
    /// to, empty while the group keeps choosing for itself. `Selector` groups
    /// never report it.
    pub fixed: String,
    pub members: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    /// Controller answered `GET /`.
    pub controller_ok: bool,
    pub version: String,
    /// `rule` | `global` | `direct`
    pub mode: String,
    pub mixed_port: u16,
    /// Effective TUN state, read back from `GET /configs`.
    pub tun: bool,
    pub sysproxy: bool,
    pub autostart: bool,
    pub kernel_running: bool,
    pub groups: Vec<Group>,
    /// Last failed action (mode/TUN/select/reload, registry writes). Survives
    /// refreshes until the next action.
    pub action_error: Option<String>,
    /// What the worker is busy with right now, while it blocks on something the
    /// user should see (the UAC prompt of an elevated kernel restart). Cleared as
    /// soon as the action is over.
    pub status_note: Option<String>,
    /// Controller reachability, owned by the refresh loop: set when the
    /// controller stops answering and cleared as soon as it is back.
    pub controller_error: Option<String>,
    /// The kernel the elevated helper started, as reported through its exit code.
    /// An elevated process's image path cannot be read from here, so a PID is the
    /// only identity the tray can hold on to.
    pub kernel_pid: Option<u32>,
    /// The dashboard "open the web UI" opens, resolved against the controller this
    /// worker found. Resolved on the worker thread because that is where the
    /// controller's address lives; the UI thread only opens what it is handed.
    pub web_ui_url: String,
}

impl Snapshot {
    pub fn group(&self, name: &str) -> Option<&Group> {
        self.groups.iter().find(|g| g.name == name)
    }

    pub fn is_group(&self, name: &str) -> bool {
        self.group(name).is_some()
    }

    /// The message to show in the tooltip/menu. Reachability wins, because it
    /// usually explains the failed action too.
    pub fn error(&self) -> Option<&str> {
        self.controller_error
            .as_deref()
            .or(self.action_error.as_deref())
    }

    /// One line summary used for the disabled status item and the tooltip.
    pub fn status_line(&self) -> String {
        let messages = crate::i18n::t();
        if self.controller_ok {
            messages.status_running(if self.mode.is_empty() {
                "unknown"
            } else {
                &self.mode
            })
        } else if self.kernel_running {
            messages.status_controller_unreachable.to_string()
        } else {
            messages.status_stopped.to_string()
        }
    }
}

pub type Shared = Arc<Mutex<Snapshot>>;

pub fn shared() -> Shared {
    Arc::new(Mutex::new(Snapshot::default()))
}

pub fn read(state: &Shared) -> Snapshot {
    // A poisoned lock still holds the last good snapshot, which is strictly
    // better than pretending the core is not running.
    state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone()
}

pub fn write(state: &Shared, snapshot: Snapshot) {
    *state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = snapshot;
}

/// Record a failure that happened on the UI thread (registry writes, shell
/// calls). It is kept until the next action replaces or clears it.
pub fn write_action_error(state: &Shared, message: String) {
    state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .action_error = Some(message);
}

/// Show (or clear) what the worker is doing right now. Unlike an error this is
/// not a result: the next refresh keeps it only until the worker clears it.
pub fn write_status_note(state: &Shared, note: Option<String>) {
    state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .status_note = note;
}

/// Remember the kernel the elevated helper started, by PID: after a replacement
/// the tray cannot read that process's image path, and the PID is what the helper
/// reported back.
pub fn write_kernel_pid(state: &Shared, pid: Option<u32>) {
    state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .kernel_pid = pid;
}

/// The kernel was started here and has not answered the controller yet: the
/// status line says so and `note` explains the wait. The first real refresh
/// replaces all of it.
pub fn write_kernel_started(state: &Shared, note: String) {
    let mut snapshot = state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    snapshot.kernel_running = true;
    snapshot.status_note = Some(note);
}

/// The kernel this program may start or replace: where it was found, and the
/// handle of the instance this program launched itself.
///
/// The worker fills it while it brings the kernel up, and reads the path back for
/// the actions that must know which kernel is ours; the UI thread reads the path
/// and claims the handle when the user asks to exit and stop mihomo. It cannot
/// live in `Snapshot`, which is cloned on every read.
#[derive(Debug, Default)]
pub struct Kernel {
    path: Option<PathBuf>,
    child: Option<Child>,
}

pub type KernelSlot = Arc<Mutex<Kernel>>;

pub fn kernel_slot() -> KernelSlot {
    Arc::new(Mutex::new(Kernel::default()))
}

/// A poisoned lock still holds the kernel this program knows about, which is
/// strictly better than pretending nothing was started.
fn lock(slot: &KernelSlot) -> std::sync::MutexGuard<'_, Kernel> {
    slot.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Record where the kernel was found, before anything is started: the exit path
/// matches the kernel it may stop against this path, including a kernel this
/// program did not start.
pub fn write_kernel_path(slot: &KernelSlot, path: Option<PathBuf>) {
    lock(slot).path = path;
}

/// Record the handle of the kernel this program launched.
pub fn write_kernel_child(slot: &KernelSlot, child: Child) {
    lock(slot).child = Some(child);
}

/// Where the kernel was found.
pub fn kernel_path(slot: &KernelSlot) -> Option<PathBuf> {
    lock(slot).path.clone()
}

/// Claim the handle of the kernel this program launched.
pub fn take_kernel_child(slot: &KernelSlot) -> Option<Child> {
    lock(slot).child.take()
}
