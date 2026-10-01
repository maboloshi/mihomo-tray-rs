//! Tray popup menu: a native `HMENU` rebuilt on every right click, so the data is
//! never stale. Long lists are capped with `MIM_MAXHEIGHT`, which makes the shell
//! add its own scroll arrows instead of clipping the menu at the screen edge.

use windows_sys::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, DestroyMenu, GetSystemMetrics, HMENU, MENUINFO, MF_CHECKED,
    MF_GRAYED, MF_POPUP, MF_SEPARATOR, MF_STRING, MIM_MAXHEIGHT, SM_CYSCREEN, SetMenuInfo,
};

use crate::i18n;
use crate::settings::Settings;
use crate::state::{Group, Snapshot};

const ID_BASE: usize = 100;
const MAX_DEPTH: usize = 4;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    ToggleSysProxy,
    SetMode(String),
    ToggleTun,
    Select {
        group: String,
        member: String,
    },
    /// Return a pinned `URLTest`/`Fallback` group to automatic selection.
    Unfix(String),
    ToggleAutostart,
    Reload,
    /// Drop every connection the kernel is proxying.
    CloseConnections,
    /// Restart the kernel process, keeping its own command line.
    RestartKernel,
    /// End the running kernel and start the one `tray.yml` describes.
    ForceRestartKernel,
    /// Measure node latency for a group (`GET /group/{name}/delay`).
    SpeedTest(String),
    /// Open the kernel's dashboard (or a hosted one pointed at it) in the
    /// browser. The tray itself has no settings window, so this is where the
    /// rest of mihomo is configured.
    OpenWebUi,
    ExitStopKernel,
    ExitOnly,
}

pub struct Menu {
    pub handle: HMENU,
    actions: Vec<Action>,
}

impl Menu {
    pub fn action(&self, id: usize) -> Option<&Action> {
        id.checked_sub(ID_BASE)
            .and_then(|index| self.actions.get(index))
    }

    pub fn build(snapshot: &Snapshot, settings: &Settings) -> Menu {
        let messages = i18n::t();
        let mut builder = Builder::default();
        let root = builder.new_menu();
        let ready = snapshot.controller_ok;

        builder.plain(root, &snapshot.status_line(), false);
        if let Some(note) = snapshot.status_note.as_deref() {
            builder.plain(root, &truncate(note, 90), false);
        }
        if let Some(error) = snapshot.error().map(str::to_string) {
            builder.plain(root, &truncate(&format!("⚠ {error}"), 90), false);
        }
        builder.separator(root);

        builder.checked(
            root,
            &messages.menu_system_proxy,
            Action::ToggleSysProxy,
            snapshot.sysproxy,
            true,
        );

        let mode_menu = builder.new_menu();
        for (label, mode) in [
            (&*messages.menu_mode_rule, "rule"),
            (&*messages.menu_mode_global, "global"),
            (&*messages.menu_mode_direct, "direct"),
        ] {
            builder.checked(
                mode_menu,
                label,
                Action::SetMode(mode.to_string()),
                snapshot.mode == mode,
                ready,
            );
        }
        builder.popup(root, &messages.menu_mode, mode_menu, ready, false);

        builder.checked(
            root,
            &messages.menu_tun,
            Action::ToggleTun,
            snapshot.tun,
            ready,
        );

        let groups_menu = builder.new_menu();
        let groups = ordered_groups(snapshot, settings);
        for group in &groups {
            let submenu = builder.new_menu();
            add_members(
                &mut builder,
                submenu,
                snapshot,
                group,
                0,
                &mut Vec::new(),
                settings,
            );
            builder.popup(groups_menu, &group_label(group), submenu, ready, false);
        }
        builder.popup(
            root,
            &messages.menu_groups,
            groups_menu,
            ready && !groups.is_empty(),
            false,
        );

        builder.separator(root);
        builder.checked(
            root,
            &messages.menu_autostart,
            Action::ToggleAutostart,
            snapshot.autostart,
            true,
        );
        // The URL points at the controller (or at a panel told about it), so
        // this entry follows the controller like every other action here.
        builder.item(root, &messages.menu_web_ui, Action::OpenWebUi, false, ready);

        // The one-off commands that act on the kernel process live one level down:
        // they are what a user reaches for when something is wrong, not part of the
        // switches the menu is opened for every day.
        let more_menu = builder.new_menu();
        builder.item(
            more_menu,
            &messages.menu_reload,
            Action::Reload,
            false,
            ready,
        );
        builder.item(
            more_menu,
            &messages.menu_close_connections,
            Action::CloseConnections,
            false,
            ready,
        );
        builder.item(
            more_menu,
            &messages.menu_restart_kernel,
            Action::RestartKernel,
            false,
            ready,
        );
        // Replacing the kernel is process work: it needs a kernel that is running,
        // not a controller to talk to.
        builder.item(
            more_menu,
            &messages.menu_force_restart_kernel,
            Action::ForceRestartKernel,
            false,
            snapshot.kernel_running,
        );
        builder.popup(
            root,
            &messages.menu_more,
            more_menu,
            ready || snapshot.kernel_running,
            false,
        );

        builder.separator(root);
        let exit_menu = builder.new_menu();
        builder.item(
            exit_menu,
            &messages.menu_exit_stop_kernel,
            Action::ExitStopKernel,
            false,
            true,
        );
        builder.item(
            exit_menu,
            &messages.menu_exit_only,
            Action::ExitOnly,
            false,
            true,
        );
        builder.popup(root, &messages.menu_exit, exit_menu, true, false);

        Menu {
            handle: root,
            actions: builder.actions,
        }
    }

    pub fn destroy(&mut self) {
        if !self.handle.is_null() {
            unsafe {
                DestroyMenu(self.handle);
            }
            self.handle = std::ptr::null_mut();
        }
    }
}

impl Drop for Menu {
    fn drop(&mut self) {
        self.destroy();
    }
}

fn ordered_groups<'a>(snapshot: &'a Snapshot, settings: &Settings) -> Vec<&'a Group> {
    let included = |name: &str| {
        (settings.groups_include.is_empty() || settings.groups_include.iter().any(|n| n == name))
            && !settings.groups_exclude.iter().any(|n| n == name)
    };
    let mut groups: Vec<&Group> = snapshot
        .groups
        .iter()
        .filter(|g| included(&g.name))
        .collect();
    // One comparator: configured order first (`groups.order`), then `GLOBAL` as
    // the conventional entry point, then alphabetical — `/proxies` is a Go map
    // and carries no configuration order of its own.
    groups.sort_by(|a, b| {
        let key = |g: &Group| {
            let position = settings
                .groups_order
                .iter()
                .position(|n| n == &g.name)
                .unwrap_or(usize::MAX);
            (position, g.name != "GLOBAL", g.name.to_lowercase())
        };
        key(a).cmp(&key(b))
    });
    groups
}

fn add_members(
    builder: &mut Builder,
    menu: HMENU,
    snapshot: &Snapshot,
    group: &Group,
    depth: usize,
    path: &mut Vec<String>,
    settings: &Settings,
) {
    // Measuring is a property of this group, so it belongs in the group's own
    // submenu rather than in a menu-wide entry that would have to guess which
    // group the user meant. Only there: a nested group is already listed under
    // "proxy groups" with a row of its own, and repeating this row at every
    // nesting level would spend the item budget on itself.
    let mut has_tools = false;
    if depth == 0 && snapshot.controller_ok && !group.members.is_empty() && !builder.out_of_budget()
    {
        builder.item(
            menu,
            &i18n::t().menu_speed_test,
            Action::SpeedTest(group.name.clone()),
            false,
            true,
        );
        has_tools = true;
    }
    // A pinned URLTest/Fallback stops choosing for itself, so the way back to
    // automatic selection has to be offered explicitly.
    if !group.fixed.is_empty() && !builder.out_of_budget() {
        builder.checked(
            menu,
            &i18n::t().menu_unfix,
            Action::Unfix(group.name.clone()),
            false,
            true,
        );
        has_tools = true;
    }
    if has_tools {
        builder.separator(menu);
    }
    // Latency only reorders a list that has been measured: mihomo's `all` array
    // is the order the configuration asked for, and reordering that because the
    // kernel has no `history` yet would be a change nobody asked for.
    let mut entries = member_entries(group);
    if entries.iter().any(|(_, delay)| delay.is_some()) {
        sort_by_delay(&mut entries);
    }
    let page_size = settings.groups_page_size;
    if page_size > 0 && entries.len() > page_size && depth == 0 {
        for (index, chunk) in entries.chunks(page_size).enumerate() {
            let first = index * page_size + 1;
            let last = first + chunk.len() - 1;
            let page = builder.new_menu();
            add_member_list(builder, page, snapshot, group, depth, path, settings, chunk);
            builder.popup(menu, &format!("{first}–{last}"), page, true, false);
        }
        return;
    }
    add_member_list(
        builder, menu, snapshot, group, depth, path, settings, &entries,
    );
}

/// The group's members paired with the latency `/proxies` reports for each,
/// index-aligned through [`Group::delays`].
fn member_entries(group: &Group) -> Vec<(String, Option<u32>)> {
    group
        .members
        .iter()
        .enumerate()
        .map(|(index, member)| (member.clone(), group.delays.get(index).copied().flatten()))
        .collect()
}

/// Where a member sorts. Measured nodes come first, fastest first; a node the
/// kernel never tested and one whose test timed out share the last place —
/// neither is a latency, so ordering one above the other would be invented, and
/// the order the kernel reported is kept among equals.
fn delay_key(delay: Option<u32>) -> u32 {
    match delay {
        Some(ms) if ms > 0 => ms,
        _ => u32::MAX,
    }
}

/// Stable insertion sort by [`delay_key`].
///
/// Hand-written rather than `slice::sort_by_key`, which would be the obvious
/// call: the standard library's stable sort is a large generic function that is
/// instantiated once per element type, and `ordered_groups` already pays for one
/// of those. A second instantiation measures around 4 KiB in a binary whose size
/// is part of its design (`docs/DESIGN.md` §2), while a group holds a handful of
/// nodes — the quadratic worst case here is a few thousand comparisons inside
/// the `MAX_ITEMS` menu budget. Equal keys keep their original relative order,
/// so an unmeasured tail stays exactly as mihomo reported it.
fn sort_by_delay(entries: &mut [(String, Option<u32>)]) {
    for index in 1..entries.len() {
        let mut position = index;
        while position > 0 && delay_key(entries[position].1) < delay_key(entries[position - 1].1) {
            entries.swap(position - 1, position);
            position -= 1;
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn add_member_list(
    builder: &mut Builder,
    menu: HMENU,
    snapshot: &Snapshot,
    group: &Group,
    depth: usize,
    path: &mut Vec<String>,
    settings: &Settings,
    members: &[(String, Option<u32>)],
) {
    if builder.out_of_budget() {
        builder.plain(menu, &i18n::t().menu_truncated, false);
        return;
    }
    for (member, delay) in members {
        if builder.out_of_budget() {
            builder.plain(menu, &i18n::t().menu_truncated, false);
            break;
        }
        let nested = depth < MAX_DEPTH
            && member != &group.name
            && !path.iter().any(|p| p == member)
            && snapshot.is_group(member);
        if nested {
            if let Some(child) = snapshot.group(member) {
                let submenu = builder.new_menu();
                path.push(member.clone());
                add_members(builder, submenu, snapshot, child, depth + 1, path, settings);
                path.pop();
                builder.popup(
                    menu,
                    &member_label(member, *delay),
                    submenu,
                    group.switchable,
                    member == &group.now,
                );
            }
        } else {
            builder.checked(
                menu,
                &member_label(member, *delay),
                Action::Select {
                    group: group.name.clone(),
                    member: member.clone(),
                },
                member == &group.now,
                group.switchable,
            );
        }
    }
    if members.is_empty() {
        builder.plain(menu, &i18n::t().menu_empty, false);
    }
}

/// Upper bound on menu entries per popup build. Mihomo groups reference each
/// other (GLOBAL lists every group), so without a cap the nested expansion grows
/// like G^4 and would freeze the UI thread right-clicking the tray.
const MAX_ITEMS: usize = 1500;

#[derive(Default)]
struct Builder {
    actions: Vec<Action>,
    items: usize,
}

impl Builder {
    fn out_of_budget(&self) -> bool {
        self.items >= MAX_ITEMS
    }

    fn new_menu(&self) -> HMENU {
        unsafe {
            let menu = CreatePopupMenu();
            let mut info: MENUINFO = std::mem::zeroed();
            info.cbSize = std::mem::size_of::<MENUINFO>() as u32;
            info.fMask = MIM_MAXHEIGHT;
            // Ask the shell to scroll instead of letting a tall menu run off the
            // screen; it draws the up/down arrows itself.
            info.cyMax = (GetSystemMetrics(SM_CYSCREEN) as u32 * 6 / 10).min(900);
            SetMenuInfo(menu, &info);
            menu
        }
    }

    fn separator(&mut self, menu: HMENU) {
        unsafe {
            AppendMenuW(menu, MF_SEPARATOR, 0, std::ptr::null());
        }
    }

    fn plain(&mut self, menu: HMENU, text: &str, enabled: bool) {
        self.items += 1;
        let text = wide(text);
        unsafe {
            AppendMenuW(
                menu,
                MF_STRING | if enabled { 0 } else { MF_GRAYED },
                0,
                text.as_ptr(),
            );
        }
    }

    fn item(&mut self, menu: HMENU, text: &str, action: Action, checked: bool, enabled: bool) {
        self.checked(menu, text, action, checked, enabled);
    }

    fn checked(&mut self, menu: HMENU, text: &str, action: Action, checked: bool, enabled: bool) {
        self.actions.push(action);
        self.items += 1;
        let id = ID_BASE + self.actions.len() - 1;
        let text = wide(text);
        let flags =
            MF_STRING | if checked { MF_CHECKED } else { 0 } | if enabled { 0 } else { MF_GRAYED };
        unsafe {
            AppendMenuW(menu, flags, id, text.as_ptr());
        }
    }

    /// A nested group can be the current selection of its parent, so submenu
    /// items carry the check mark too.
    fn popup(&mut self, parent: HMENU, text: &str, submenu: HMENU, enabled: bool, checked: bool) {
        self.items += 1;
        let text = wide(text);
        let flags =
            MF_POPUP | if enabled { 0 } else { MF_GRAYED } | if checked { MF_CHECKED } else { 0 };
        unsafe {
            AppendMenuW(parent, flags, submenu as usize, text.as_ptr());
        }
    }
}

/// Groups that are not plain selectors carry their type, so it is obvious why
/// their members behave the way they do. A pinned automatic group additionally
/// says so, because pinning stops the health check without changing anything
/// else about the group.
fn group_label(group: &Group) -> String {
    let name = menu_text(&group.name);
    if group.switchable && group.kind == "Selector" {
        return name;
    }
    let messages = i18n::t();
    if group.fixed.is_empty() {
        messages.menu_group_label(&name, &group.kind)
    } else {
        messages.menu_group_label_pinned(&name, &group.kind)
    }
}

/// A node as it appears in a group submenu: its name, plus the latency the kernel
/// last measured for it. A node that was never tested keeps a bare name, and one
/// whose test timed out says so instead of showing a plausible-looking `0ms`.
fn member_label(member: &str, delay: Option<u32>) -> String {
    let name = menu_text(member);
    match delay {
        Some(0) => i18n::t().menu_member_timeout(&name),
        Some(ms) => i18n::t().menu_member_delay(&name, ms),
        None => name,
    }
}

fn menu_text(s: &str) -> String {
    // `&` is a mnemonic marker inside a menu string, and control characters (a
    // NUL in particular) would truncate the visible label while the action still
    // uses the full name.
    s.chars()
        .map(|c| match c {
            '&' => "&&".to_string(),
            c if c.is_control() => " ".to_string(),
            c => c.to_string(),
        })
        .collect()
}

fn truncate(s: &str, limit: usize) -> String {
    if s.chars().count() <= limit {
        return s.to_string();
    }
    let mut out: String = s.chars().take(limit).collect();
    out.push('…');
    out
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetMenuItemCount, GetMenuState, GetMenuStringW, GetSubMenu, MF_BYCOMMAND, MF_BYPOSITION,
    };

    fn group(name: &str, switchable: bool, now: &str, members: &[&str]) -> Group {
        group_with_delays(name, switchable, now, members, &[])
    }

    /// A group whose members carry the latencies `/proxies` reported for them.
    /// `delays` is index-aligned with `members`; anything missing stays untested.
    fn group_with_delays(
        name: &str,
        switchable: bool,
        now: &str,
        members: &[&str],
        delays: &[Option<u32>],
    ) -> Group {
        let members: Vec<String> = members.iter().map(|m| m.to_string()).collect();
        let delays = (0..members.len())
            .map(|index| delays.get(index).copied().flatten())
            .collect();
        Group {
            name: name.to_string(),
            kind: if switchable { "Selector" } else { "URLTest" }.to_string(),
            switchable,
            now: now.to_string(),
            fixed: String::new(),
            members,
            delays,
        }
    }

    fn snapshot(groups: Vec<Group>) -> Snapshot {
        Snapshot {
            controller_ok: true,
            mode: "rule".to_string(),
            mixed_port: 7890,
            groups,
            ..Default::default()
        }
    }

    /// Menu item label by command id.
    fn label(menu: HMENU, id: usize) -> String {
        unsafe {
            let mut buffer = vec![0u16; 256];
            let len = GetMenuStringW(
                menu,
                id as u32,
                buffer.as_mut_ptr(),
                buffer.len() as i32,
                MF_BYCOMMAND,
            );
            String::from_utf16_lossy(&buffer[..len.max(0) as usize])
        }
    }

    /// Menu item label by position.
    fn label_at(menu: HMENU, position: i32) -> String {
        unsafe {
            let mut buffer = vec![0u16; 256];
            let len = GetMenuStringW(
                menu,
                position as u32,
                buffer.as_mut_ptr(),
                buffer.len() as i32,
                MF_BYPOSITION,
            );
            String::from_utf16_lossy(&buffer[..len.max(0) as usize])
        }
    }

    fn labels(menu: HMENU) -> Vec<String> {
        let count = unsafe { GetMenuItemCount(menu) };
        (0..count)
            .map(|position| label_at(menu, position))
            .collect()
    }

    fn find_submenu(menu: HMENU, name: &str) -> HMENU {
        let count = unsafe { GetMenuItemCount(menu) };
        for position in 0..count {
            if label_at(menu, position) == name {
                return unsafe { GetSubMenu(menu, position) };
            }
        }
        std::ptr::null_mut()
    }

    /// Position of a submenu item, which is how its own state (grayed, checked) is
    /// read: a popup carries no command id.
    fn submenu_position(menu: HMENU, name: &str) -> Option<i32> {
        let count = unsafe { GetMenuItemCount(menu) };
        (0..count).find(|position| label_at(menu, *position) == name)
    }

    /// Total number of entries in the whole menu tree.
    fn count_items(menu: HMENU) -> usize {
        let count = unsafe { GetMenuItemCount(menu) };
        let mut total = 0;
        for position in 0..count {
            total += 1;
            let submenu = unsafe { GetSubMenu(menu, position) };
            if !submenu.is_null() {
                total += count_items(submenu);
            }
        }
        total
    }

    #[test]
    fn groups_are_ordered_by_config_then_global_then_name() {
        let settings = Settings {
            groups_order: vec!["Zulu".to_string()],
            ..Settings::default()
        };
        let snapshot = snapshot(vec![
            group("Alpha", true, "DIRECT", &["DIRECT"]),
            group("GLOBAL", true, "DIRECT", &["DIRECT"]),
            group("Zulu", true, "DIRECT", &["DIRECT"]),
        ]);
        let menu = Menu::build(&snapshot, &settings);
        let groups = find_submenu(menu.handle, &i18n::t().menu_groups);
        assert!(!groups.is_null(), "groups submenu missing");
        assert_eq!(labels(groups), vec!["Zulu", "GLOBAL", "Alpha"]);
    }

    #[test]
    fn current_member_is_checked_and_readonly_members_are_grayed() {
        let snapshot = snapshot(vec![
            group("GLOBAL", true, "B", &["A", "B"]),
            group("Auto", false, "A", &["A", "B"]),
        ]);
        let menu = Menu::build(&snapshot, &Settings::default());
        let groups = find_submenu(menu.handle, &i18n::t().menu_groups);
        let global = find_submenu(groups, "GLOBAL");
        let auto = find_submenu(groups, &i18n::t().menu_group_label("Auto", "URLTest"));
        assert!(!global.is_null() && !auto.is_null());

        for (index, action) in menu.actions.iter().enumerate() {
            let Action::Select { group, member } = action else {
                continue;
            };
            let id = (ID_BASE + index) as u32;
            let submenu = if group == "GLOBAL" { global } else { auto };
            let state = unsafe { GetMenuState(submenu, id, MF_BYCOMMAND) };
            let now = snapshot.group(group).expect("group exists").now.clone();
            assert_eq!(
                label(submenu, ID_BASE + index),
                menu_text(member),
                "{group}/{member} label"
            );
            assert_eq!(
                state & MF_CHECKED != 0,
                *member == now,
                "{group}/{member} check state"
            );
            if group == "Auto" {
                assert!(
                    state & MF_GRAYED != 0,
                    "members of a read-only group must be grayed"
                );
            }
        }
    }

    #[test]
    fn pinned_auto_groups_are_switchable_and_offer_unpin() {
        let mut auto = group("Auto", true, "A", &["A", "B"]);
        auto.kind = "URLTest".to_string();
        auto.fixed = "A".to_string();
        let menu = Menu::build(&snapshot(vec![auto]), &Settings::default());
        let groups = find_submenu(menu.handle, &i18n::t().menu_groups);
        let pinned = i18n::t().menu_group_label_pinned("Auto", "URLTest");
        assert_eq!(labels(groups), vec![pinned.clone()]);

        let auto = find_submenu(groups, &pinned);
        assert!(!auto.is_null());
        // The group's own tools sit above its members: the speed test first, then
        // the way back to automatic selection for a pinned group.
        let entries = labels(auto);
        assert_eq!(entries[0], i18n::t().menu_speed_test.to_string());
        assert!(
            entries.contains(&i18n::t().menu_unfix.to_string()),
            "a pinned group offers unpinning: {entries:?}"
        );

        for (index, action) in menu.actions.iter().enumerate() {
            let id = ID_BASE + index;
            match action {
                Action::Unfix(group) => {
                    assert_eq!(group, "Auto");
                    assert_eq!(label(auto, id), i18n::t().menu_unfix.to_string());
                }
                Action::Select { group, member } => {
                    assert_eq!(group, "Auto");
                    let state = unsafe { GetMenuState(auto, id as u32, MF_BYCOMMAND) };
                    assert_eq!(
                        state & MF_GRAYED,
                        0,
                        "{member} of a URLTest group must be selectable"
                    );
                    assert_eq!(
                        state & MF_CHECKED != 0,
                        member == "A",
                        "{member} check state"
                    );
                }
                _ => {}
            }
        }
    }

    #[test]
    fn nested_groups_stop_at_the_item_budget() {
        // Five groups whose members reference each other: without the budget this
        // expands like G^4 on the UI thread during a right click.
        let names = ["G1", "G2", "G3", "G4", "G5"];
        let groups = names
            .iter()
            .map(|name| group(name, true, "missing", &names))
            .collect();
        let started = std::time::Instant::now();
        let menu = Menu::build(&snapshot(groups), &Settings::default());
        let elapsed = started.elapsed();
        let total = count_items(menu.handle);
        assert!(
            total <= MAX_ITEMS + 64,
            "menu grew to {total} entries (budget {MAX_ITEMS})"
        );
        assert!(elapsed.as_secs() < 2, "menu build took {elapsed:?}");
    }

    #[test]
    fn control_characters_and_ampersands_in_names_are_neutralised() {
        let snapshot = snapshot(vec![group("G&1", true, "a\nb", &["a\nb", "ok"])]);
        let menu = Menu::build(&snapshot, &Settings::default());
        let groups = find_submenu(menu.handle, &i18n::t().menu_groups);
        assert_eq!(label_at(groups, 0), "G&&1");
        let group_menu = find_submenu(groups, "G&&1");
        // The members keep the kernel's order, with the group's own tools above.
        let entries = labels(group_menu);
        assert!(
            entries.ends_with(&["a b".to_string(), "ok".to_string()]),
            "unexpected labels: {entries:?}"
        );
        // The action keeps the real name even though the label is sanitised.
        assert!(menu.actions.iter().any(|action| matches!(
            action,
            Action::Select { group, member } if group == "G&1" && member == "a\nb"
        )));
    }

    #[test]
    fn the_web_dashboard_entry_follows_the_controller_like_the_rest() {
        let messages = i18n::t();
        let ready = Menu::build(&snapshot(Vec::new()), &Settings::default());
        let index = ready
            .actions
            .iter()
            .position(|action| matches!(action, Action::OpenWebUi))
            .expect("the web dashboard entry exists");
        let id = ID_BASE + index;
        assert_eq!(
            label(ready.handle, id),
            messages.menu_web_ui.to_string(),
            "the entry sits in the root menu"
        );
        assert_eq!(
            unsafe { GetMenuState(ready.handle, id as u32, MF_BYCOMMAND) } & MF_GRAYED,
            0,
            "a reachable controller leaves the entry clickable"
        );

        // The URL points at the controller, so an unreachable one means there is
        // no dashboard to open.
        let mut offline = snapshot(Vec::new());
        offline.controller_ok = false;
        let offline = Menu::build(&offline, &Settings::default());
        assert_ne!(
            unsafe { GetMenuState(offline.handle, id as u32, MF_BYCOMMAND) } & MF_GRAYED,
            0,
            "without a controller the entry is grayed out"
        );
    }

    #[test]
    fn the_kernel_commands_live_in_the_more_submenu() {
        let messages = i18n::t();
        let menu = Menu::build(&snapshot(Vec::new()), &Settings::default());
        let more = find_submenu(menu.handle, &messages.menu_more);
        assert!(!more.is_null(), "more submenu missing");
        assert_eq!(
            labels(more),
            vec![
                messages.menu_reload.to_string(),
                messages.menu_close_connections.to_string(),
                messages.menu_restart_kernel.to_string(),
                messages.menu_force_restart_kernel.to_string(),
            ]
        );
        assert!(
            !labels(menu.handle).contains(&messages.menu_reload.to_string()),
            "reload config is a submenu entry now, not a root one"
        );

        // Reloading and closing connections need a controller, replacing the kernel
        // needs a kernel that is running: with neither, the submenu is out of reach.
        let mut offline = snapshot(Vec::new());
        offline.controller_ok = false;
        let offline = Menu::build(&offline, &Settings::default());
        let position = submenu_position(offline.handle, &messages.menu_more).expect("more submenu");
        let state = unsafe { GetMenuState(offline.handle, position as u32, MF_BYPOSITION) };
        assert_ne!(state & MF_GRAYED, 0, "no controller and no kernel");

        // A kernel that is running can be replaced without a controller, so the
        // submenu stays reachable and only the controller entries are grayed.
        let mut kernel_only = snapshot(Vec::new());
        kernel_only.controller_ok = false;
        kernel_only.kernel_running = true;
        let kernel_only = Menu::build(&kernel_only, &Settings::default());
        let position =
            submenu_position(kernel_only.handle, &messages.menu_more).expect("more submenu");
        let state = unsafe { GetMenuState(kernel_only.handle, position as u32, MF_BYPOSITION) };
        assert_eq!(state & MF_GRAYED, 0, "a kernel is there to be replaced");
        let more = find_submenu(kernel_only.handle, &messages.menu_more);
        for (index, action) in kernel_only.actions.iter().enumerate() {
            let id = (ID_BASE + index) as u32;
            let grayed = unsafe { GetMenuState(more, id, MF_BYCOMMAND) } & MF_GRAYED != 0;
            match action {
                Action::Reload | Action::CloseConnections | Action::RestartKernel => {
                    assert!(grayed, "{action:?}")
                }
                Action::ForceRestartKernel => assert!(!grayed, "{action:?}"),
                _ => {}
            }
        }
    }

    #[test]
    fn measured_members_carry_their_latency_and_sort_fastest_first() {
        let messages = i18n::t();
        // Deliberately listed slowest-first, and with one node never tested and
        // one that timed out.
        let group = group_with_delays(
            "Auto",
            true,
            "B",
            &["Slow", "Fast", "Untested", "Dead"],
            &[Some(400), Some(30), None, Some(0)],
        );
        let menu = Menu::build(&snapshot(vec![group]), &Settings::default());
        let groups = find_submenu(menu.handle, &i18n::t().menu_groups);
        let auto = find_submenu(groups, "Auto");
        let members: Vec<String> = labels(auto)
            .into_iter()
            .filter(|label| !label.is_empty() && label != &messages.menu_speed_test.to_string())
            .collect();
        assert_eq!(
            members,
            vec![
                messages.menu_member_delay("Fast", 30),
                messages.menu_member_delay("Slow", 400),
                // Neither a timeout nor an untested node is a latency, so they
                // share the last place and keep the order the kernel reported.
                "Untested".to_string(),
                messages.menu_member_timeout("Dead"),
            ],
            "members are ordered by measured latency, unmeasured last"
        );
    }

    #[test]
    fn sorting_by_delay_is_stable_and_puts_unmeasured_last() {
        let mut entries = vec![
            ("b".to_string(), None),
            ("c".to_string(), Some(0)),
            ("a".to_string(), Some(50)),
            ("d".to_string(), Some(10)),
            ("e".to_string(), None),
        ];
        sort_by_delay(&mut entries);
        let names: Vec<&str> = entries.iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(
            names,
            vec!["d", "a", "b", "c", "e"],
            "fastest first; timeouts and unmeasured keep their order behind the rest"
        );
    }

    #[test]
    fn an_untested_group_keeps_the_order_the_kernel_reported() {
        // mihomo's `all` array is the order the configuration asked for; without
        // measurements there is nothing that could justify reordering it.
        let menu = Menu::build(
            &snapshot(vec![group("Auto", true, "c", &["c", "a", "b"])]),
            &Settings::default(),
        );
        let groups = find_submenu(menu.handle, &i18n::t().menu_groups);
        let auto = find_submenu(groups, "Auto");
        let members: Vec<String> = labels(auto)
            .into_iter()
            .filter(|label| !label.is_empty() && label != &i18n::t().menu_speed_test.to_string())
            .collect();
        assert_eq!(members, vec!["c", "a", "b"]);
    }

    #[test]
    fn the_speed_test_entry_belongs_to_its_group_and_follows_the_controller() {
        let menu = Menu::build(
            &snapshot(vec![group("Auto", true, "A", &["A"])]),
            &Settings::default(),
        );
        let groups = find_submenu(menu.handle, &i18n::t().menu_groups);
        let auto = find_submenu(groups, "Auto");
        let index = menu
            .actions
            .iter()
            .position(|action| matches!(action, Action::SpeedTest(group) if group == "Auto"))
            .expect("the speed-test entry exists");
        let id = (ID_BASE + index) as u32;
        assert_eq!(
            label(auto, ID_BASE + index),
            i18n::t().menu_speed_test.to_string()
        );
        assert_eq!(
            unsafe { GetMenuState(auto, id, MF_BYCOMMAND) } & MF_GRAYED,
            0,
            "a reachable controller leaves the entry clickable"
        );

        // Testing a group is a controller call, so without one there is nothing
        // to ask.
        let mut offline = snapshot(vec![group("Auto", true, "A", &["A"])]);
        offline.controller_ok = false;
        let offline = Menu::build(&offline, &Settings::default());
        let index = offline
            .actions
            .iter()
            .position(|action| matches!(action, Action::SpeedTest(_)));
        assert!(
            index.is_none(),
            "an unreachable controller offers no speed test"
        );
    }

    #[test]
    fn disabled_status_item_has_no_action() {
        let mut snapshot = snapshot(Vec::new());
        snapshot.controller_ok = false;
        let menu = Menu::build(&snapshot, &Settings::default());
        assert_ne!(
            unsafe { GetMenuState(menu.handle, 0, MF_BYCOMMAND) },
            0xffff_ffff
        );
        // The status line is appended with id 0 and must map to no action.
        assert!(menu.action(0).is_none());
        assert_eq!(label_at(menu.handle, 0), snapshot.status_line());
    }
}
