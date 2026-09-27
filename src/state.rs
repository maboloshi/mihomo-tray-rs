//! Shared runtime state. The worker thread writes, the UI thread reads.

use std::path::PathBuf;
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
    /// The exact image path of the kernel this program started or replaced last.
    /// Stopping uses it first, because that is the only path that is certainly
    /// the kernel's own — the discovered path is a fallback.
    pub kernel_target: Option<PathBuf>,
    /// The kernel the elevated helper started, as reported through its exit code.
    /// An elevated process's image path cannot be read from here, so a PID is the
    /// only identity the tray can hold on to.
    pub kernel_pid: Option<u32>,
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

/// Remember which kernel image this program started or replaced, so stopping it
/// does not have to guess between the configured and the running path.
pub fn write_kernel_target(state: &Shared, path: Option<PathBuf>) {
    state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .kernel_target = path;
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
