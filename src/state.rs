//! Shared runtime state. The worker thread writes, the UI thread reads.

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
    /// Controller reachability, owned by the refresh loop: set when the
    /// controller stops answering and cleared as soon as it is back.
    pub controller_error: Option<String>,
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
        if self.controller_ok {
            let mode = if self.mode.is_empty() {
                "unknown"
            } else {
                &self.mode
            };
            format!("Mihomo 状态: 运行中 ({mode})")
        } else if self.kernel_running {
            "Mihomo 状态: 内核运行中，控制器不可达".to_string()
        } else {
            "Mihomo 状态: 未运行".to_string()
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
