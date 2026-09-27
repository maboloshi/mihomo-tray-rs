//! User-interface strings.
//!
//! Every label, status line and error message lives here instead of at its call
//! site, so the UI language is a runtime choice rather than a property of the
//! source. `zh-CN` is built in and is the default.
//!
//! A tag that names a `lang/<tag>.yml` next to `tray.yml` is layered on top of a
//! built-in table, so a new language is one translated file and a key that file
//! omits falls back to the built-in string instead of breaking the build.
//!
//! Values are `Cow<'static, str>`: a built-in string stays borrowed (no
//! allocation, still a plain `&str` for `AppendMenuW`) while a translated one is
//! owned. Both live in the same table, so no call site has to care which it got.

use std::borrow::Cow;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use windows_sys::Win32::Globalization::GetUserDefaultUILanguage;

/// Declares the message table. The built-in tables, the file loader and the test
/// accessor are all generated from this one list, so a string cannot be defined
/// in one place and forgotten in another.
macro_rules! messages {
    ($($field:ident => $key:literal),* $(,)?) => {
        /// Every user-visible string. A field that contains a `{hole}` is a
        /// template and is filled by the matching render method below.
        #[derive(Debug, Clone)]
        pub struct Messages {
            $(pub $field: Cow<'static, str>,)*
        }

        impl Messages {
            /// Overwrite the fields `source` defines; keys it does not mention
            /// keep their current value. This is what makes a partially
            /// translated file fall back one string at a time.
            fn overlay(&mut self, source: &dyn Fn(&str) -> Option<String>) {
                $(
                    if let Some(value) = source($key) {
                        self.$field = Cow::Owned(value);
                    }
                )*
            }

            /// `(key, value)` in declaration order, for the tests that keep the
            /// built-in tables and the shipped template in sync.
            #[cfg(test)]
            pub fn entries(&self) -> Vec<(&'static str, &str)> {
                vec![$(($key, &*self.$field),)*]
            }
        }
    };
}

messages! {
    // Menu.
    menu_system_proxy => "menu.system_proxy",
    menu_mode => "menu.mode",
    menu_mode_rule => "menu.mode_rule",
    menu_mode_global => "menu.mode_global",
    menu_mode_direct => "menu.mode_direct",
    menu_tun => "menu.tun",
    menu_groups => "menu.groups",
    menu_group_label_tpl => "menu.group_label",
    menu_group_label_pinned_tpl => "menu.group_label_pinned",
    menu_unfix => "menu.unfix",
    menu_truncated => "menu.truncated",
    menu_empty => "menu.empty",
    menu_autostart => "menu.autostart",
    menu_reload => "menu.reload",
    menu_restart_admin => "menu.restart_admin",
    menu_exit => "menu.exit",
    menu_exit_stop_kernel => "menu.exit_stop_kernel",
    menu_exit_only => "menu.exit_only",

    // Status line and tooltip.
    status_running_tpl => "status.running",
    status_controller_unreachable => "status.controller_unreachable",
    status_stopped => "status.stopped",
    state_on => "state.on",
    state_off => "state.off",
    tooltip_kernel_tpl => "tooltip.kernel",
    tooltip_proxy_tun_tpl => "tooltip.proxy_tun",
    tooltip_port_tpl => "tooltip.port",

    // Errors. These reach the menu, the tooltip and the startup message box.
    error_kernel_not_found => "error.kernel_not_found",
    error_read_settings_tpl => "error.read_settings",
    error_window_create_tpl => "error.window_create",
    error_no_kernel_path => "error.no_kernel_path",
    error_no_matching_kernel => "error.no_matching_kernel",
    error_spawn_worker_tpl => "error.spawn_worker",
    error_controller_unreachable_tpl => "error.controller_unreachable",
    error_winhttp_open => "error.winhttp_open",
    error_request_failed => "error.request_failed",
    error_response_too_large => "error.response_too_large",
    error_http_request_failed => "error.http_request_failed",
    error_json_parse_tpl => "error.json_parse",
    error_start_process_tpl => "error.start_process",
    error_open_process_tpl => "error.open_process",
    error_kill_process_tpl => "error.kill_process",
    error_open_run_key_tpl => "error.open_run_key",
    error_current_exe_tpl => "error.current_exe",
    error_write_autostart_tpl => "error.write_autostart",
    error_delete_autostart_tpl => "error.delete_autostart",
    error_elevate_cancelled => "error.elevate_cancelled",
    error_message_window => "error.message_window",
    error_open_internet_settings_tpl => "error.open_internet_settings",
    error_set_proxy_enable_tpl => "error.set_proxy_enable",
    error_set_proxy_server_tpl => "error.set_proxy_server",
    error_set_proxy_override_tpl => "error.set_proxy_override",
}

impl Messages {
    /// Fill `{hole}` placeholders. An unknown hole stays visible on purpose: a
    /// translation that lost its placeholder should be obvious in the UI rather
    /// than silently drop the value it was meant to carry.
    fn fill(&self, template: &str, args: &[(&str, &str)]) -> String {
        let mut text = template.to_string();
        for (name, value) in args {
            text = text.replace(&format!("{{{name}}}"), value);
        }
        text
    }

    pub fn menu_group_label(&self, name: &str, kind: &str) -> String {
        self.fill(
            &self.menu_group_label_tpl,
            &[("name", name), ("kind", kind)],
        )
    }

    pub fn menu_group_label_pinned(&self, name: &str, kind: &str) -> String {
        self.fill(
            &self.menu_group_label_pinned_tpl,
            &[("name", name), ("kind", kind)],
        )
    }

    pub fn status_running(&self, mode: &str) -> String {
        self.fill(&self.status_running_tpl, &[("mode", mode)])
    }

    pub fn tooltip_kernel(&self, version: &str) -> String {
        self.fill(&self.tooltip_kernel_tpl, &[("version", version)])
    }

    pub fn tooltip_proxy_tun(&self, proxy: &str, tun: &str) -> String {
        self.fill(
            &self.tooltip_proxy_tun_tpl,
            &[("proxy", proxy), ("tun", tun)],
        )
    }

    pub fn tooltip_port(&self, port: u16) -> String {
        self.fill(&self.tooltip_port_tpl, &[("port", &port.to_string())])
    }

    pub fn error_read_settings(&self, path: &str, error: &str) -> String {
        self.fill(
            &self.error_read_settings_tpl,
            &[("path", path), ("error", error)],
        )
    }

    pub fn error_window_create(&self, error: &str) -> String {
        self.fill(&self.error_window_create_tpl, &[("error", error)])
    }

    pub fn error_spawn_worker(&self, error: &str) -> String {
        self.fill(&self.error_spawn_worker_tpl, &[("error", error)])
    }

    pub fn error_controller_unreachable(&self, address: &str, error: &str) -> String {
        self.fill(
            &self.error_controller_unreachable_tpl,
            &[("address", address), ("error", error)],
        )
    }

    pub fn error_json_parse(&self, error: &str) -> String {
        self.fill(&self.error_json_parse_tpl, &[("error", error)])
    }

    pub fn error_start_process(&self, path: &str, error: &str) -> String {
        self.fill(
            &self.error_start_process_tpl,
            &[("path", path), ("error", error)],
        )
    }

    pub fn error_open_process(&self, pid: u32) -> String {
        self.fill(&self.error_open_process_tpl, &[("pid", &pid.to_string())])
    }

    pub fn error_kill_process(&self, pid: u32) -> String {
        self.fill(&self.error_kill_process_tpl, &[("pid", &pid.to_string())])
    }

    pub fn error_open_run_key(&self, error: &str) -> String {
        self.fill(&self.error_open_run_key_tpl, &[("error", error)])
    }

    pub fn error_current_exe(&self, error: &str) -> String {
        self.fill(&self.error_current_exe_tpl, &[("error", error)])
    }

    pub fn error_write_autostart(&self, error: &str) -> String {
        self.fill(&self.error_write_autostart_tpl, &[("error", error)])
    }

    pub fn error_delete_autostart(&self, error: &str) -> String {
        self.fill(&self.error_delete_autostart_tpl, &[("error", error)])
    }

    pub fn error_open_internet_settings(&self, error: &str) -> String {
        self.fill(&self.error_open_internet_settings_tpl, &[("error", error)])
    }

    pub fn error_set_proxy_enable(&self, error: &str) -> String {
        self.fill(&self.error_set_proxy_enable_tpl, &[("error", error)])
    }

    pub fn error_set_proxy_server(&self, error: &str) -> String {
        self.fill(&self.error_set_proxy_server_tpl, &[("error", error)])
    }

    pub fn error_set_proxy_override(&self, error: &str) -> String {
        self.fill(&self.error_set_proxy_override_tpl, &[("error", error)])
    }
}

/// The built-in table. Strings here are the ones this code used to carry inline;
/// keep them byte-identical when editing, and edit `lang/en-US.yml` alongside.
impl Messages {
    fn zh_cn() -> Self {
        Self {
            menu_system_proxy: Cow::Borrowed("系统代理"),
            menu_mode: Cow::Borrowed("代理模式"),
            menu_mode_rule: Cow::Borrowed("Rule (规则)"),
            menu_mode_global: Cow::Borrowed("Global (全局)"),
            menu_mode_direct: Cow::Borrowed("Direct (直连)"),
            menu_tun: Cow::Borrowed("TUN 模式"),
            menu_groups: Cow::Borrowed("代理分组"),
            menu_group_label_tpl: Cow::Borrowed("{name} ({kind})"),
            menu_group_label_pinned_tpl: Cow::Borrowed("{name} ({kind} · 已固定)"),
            menu_unfix: Cow::Borrowed("自动（取消固定）"),
            menu_truncated: Cow::Borrowed("(项目过多，已省略)"),
            menu_empty: Cow::Borrowed("(空)"),
            menu_autostart: Cow::Borrowed("开机自启动"),
            menu_reload: Cow::Borrowed("重载配置"),
            menu_restart_admin: Cow::Borrowed("以管理员身份重启"),
            menu_exit: Cow::Borrowed("退出"),
            menu_exit_stop_kernel: Cow::Borrowed("退出并停止 Mihomo"),
            menu_exit_only: Cow::Borrowed("仅退出程序"),

            status_running_tpl: Cow::Borrowed("Mihomo 状态: 运行中 ({mode})"),
            status_controller_unreachable: Cow::Borrowed("Mihomo 状态: 内核运行中，控制器不可达"),
            status_stopped: Cow::Borrowed("Mihomo 状态: 未运行"),
            state_on: Cow::Borrowed("开"),
            state_off: Cow::Borrowed("关"),
            tooltip_kernel_tpl: Cow::Borrowed("内核: mihomo {version}"),
            tooltip_proxy_tun_tpl: Cow::Borrowed("系统代理: {proxy} · TUN: {tun}"),
            tooltip_port_tpl: Cow::Borrowed("端口: {port}"),

            error_kernel_not_found: Cow::Borrowed(
                "未找到 mihomo.exe，请在 tray.yml 中设置 mihomo.path",
            ),
            error_read_settings_tpl: Cow::Borrowed("读取 {path} 失败: {error}"),
            error_window_create_tpl: Cow::Borrowed("启动失败: {error}"),
            error_no_kernel_path: Cow::Borrowed("未找到 mihomo.exe 路径，无法确认要停止的进程"),
            error_no_matching_kernel: Cow::Borrowed("未找到与本程序配置匹配的 mihomo 进程"),
            error_spawn_worker_tpl: Cow::Borrowed("无法启动轮询线程: {error}"),
            error_controller_unreachable_tpl: Cow::Borrowed("控制器 {address} 不可达: {error}"),
            error_winhttp_open: Cow::Borrowed("WinHttpOpen 失败"),
            error_request_failed: Cow::Borrowed("请求失败"),
            error_response_too_large: Cow::Borrowed("响应过大（超过 8 MiB）"),
            error_http_request_failed: Cow::Borrowed("HTTP 请求失败"),
            error_json_parse_tpl: Cow::Borrowed("JSON 解析失败: {error}"),
            error_start_process_tpl: Cow::Borrowed("启动 {path} 失败: {error}"),
            error_open_process_tpl: Cow::Borrowed("无法打开进程 {pid}"),
            error_kill_process_tpl: Cow::Borrowed("结束进程 {pid} 失败"),
            error_open_run_key_tpl: Cow::Borrowed("打开 Run 键失败: {error}"),
            error_current_exe_tpl: Cow::Borrowed("获取程序路径失败: {error}"),
            error_write_autostart_tpl: Cow::Borrowed("写入自启动项失败: {error}"),
            error_delete_autostart_tpl: Cow::Borrowed("删除自启动项失败: {error}"),
            error_elevate_cancelled: Cow::Borrowed("提权启动被取消或失败"),
            error_message_window: Cow::Borrowed("创建消息窗口失败"),
            error_open_internet_settings_tpl: Cow::Borrowed("打开 Internet Settings 失败: {error}"),
            error_set_proxy_enable_tpl: Cow::Borrowed("设置 ProxyEnable 失败: {error}"),
            error_set_proxy_server_tpl: Cow::Borrowed("设置 ProxyServer 失败: {error}"),
            error_set_proxy_override_tpl: Cow::Borrowed("设置 ProxyOverride 失败: {error}"),
        }
    }
}

static MESSAGES: OnceLock<Messages> = OnceLock::new();

/// The active table. Without [`init`] (unit tests) this is the built-in default,
/// so no call site has to handle "no language chosen yet".
pub fn t() -> &'static Messages {
    MESSAGES.get_or_init(Messages::zh_cn)
}

/// Resolve and install the UI language.
///
/// Called first thing in `main`: code that runs later reports failures through
/// this table, so the language has to be settled before anything can fail.
/// Nothing selects the tag yet — it follows the Windows UI language, which is
/// exactly what a `ui.language: auto` setting would mean.
pub fn init() {
    let _ = MESSAGES.set(resolve(system_tag()));
}

fn resolve(tag: &str) -> Messages {
    match read_language_file(tag) {
        Some(translated) => {
            let mut messages = fallback();
            messages.overlay(&|key| translated.get(key).cloned());
            messages
        }
        None => builtin(tag),
    }
}

/// The table a language file is layered on: a translation may cover only part of
/// the UI, and every key it leaves out comes from here rather than showing up
/// blank. Deliberately independent of `tag`, so the same key is missing in the
/// same way whichever file is incomplete.
fn fallback() -> Messages {
    Messages::zh_cn()
}

/// The table compiled in for `tag`. An unknown tag falls back to the default, so
/// a typo in a language name degrades to a working UI instead of an empty one.
fn builtin(_tag: &str) -> Messages {
    Messages::zh_cn()
}

/// `auto`: follow the Windows UI language. Anything Chinese (simplified or
/// traditional) uses the default table, everything else English.
fn system_tag() -> &'static str {
    /// `LANG_CHINESE`, the primary language of the `LANGID`'s low ten bits.
    const LANG_CHINESE: u16 = 0x04;
    let language = unsafe { GetUserDefaultUILanguage() };
    if language & 0x3ff == LANG_CHINESE {
        "zh-CN"
    } else {
        "en-US"
    }
}

/// The directory scanned for `lang/<tag>.yml`: the one that holds `tray.yml`, so
/// the portable layout and `%APPDATA%` keep working without a second rule.
fn language_dir() -> PathBuf {
    crate::settings::settings_path()
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("lang")
}

/// Read `lang/<tag>.yml`. `None` means "no such file", which selects the
/// built-in table.
fn read_language_file(tag: &str) -> Option<HashMap<String, String>> {
    let text = std::fs::read_to_string(language_dir().join(format!("{tag}.yml"))).ok()?;
    Some(parse_language_file(&text))
}

/// Parse the deliberately tiny language-file format:
///
/// ```text
/// menu:                      # section, one per key prefix
///   system_proxy: 系统代理     # key = section + '.' + name
/// ```
///
/// A value is everything after the first `:`, trimmed, so a translation may
/// contain `#` and `:` freely; only a line whose first non-space character is
/// `#` is a comment. `strip_comment` (the `tray.yml` parser) is deliberately not
/// reused here: it treats a `#` after a space as a comment and an unmatched
/// quote as an open string, which silently truncates translations such as
/// `Proxy #1` and swallows the rest of the line after an apostrophe in `don't`.
fn parse_language_file(text: &str) -> HashMap<String, String> {
    let mut entries = HashMap::new();
    let mut section = String::new();
    for raw in text.lines() {
        let content = raw.trim();
        if content.is_empty() || content.starts_with('#') {
            continue;
        }
        let Some((key, value)) = content.split_once(':') else {
            continue;
        };
        let key = key.trim();
        if !raw.starts_with(char::is_whitespace) {
            section = key.to_string();
            continue;
        }
        let value = crate::settings::unquote(value);
        // An empty value is a mistake, not a translation; treating it as absent
        // keeps a blank line in the file from blanking a menu item.
        if !value.is_empty() {
            entries.insert(format!("{section}.{key}"), value);
        }
    }
    entries
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_table_has_no_empty_string() {
        for (key, value) in Messages::zh_cn().entries() {
            assert!(!value.trim().is_empty(), "{key} is empty");
        }
    }

    #[test]
    fn fill_replaces_every_hole() {
        let messages = Messages::zh_cn();
        assert_eq!(messages.tooltip_port(7890), "端口: 7890");
        assert_eq!(
            messages.menu_group_label("Auto", "URLTest"),
            "Auto (URLTest)"
        );
        assert_eq!(
            messages.menu_group_label_pinned("Auto", "URLTest"),
            "Auto (URLTest · 已固定)"
        );
        assert_eq!(
            messages.error_controller_unreachable("127.0.0.1:9090", "timeout"),
            "控制器 127.0.0.1:9090 不可达: timeout"
        );
    }

    #[test]
    fn language_file_keeps_hash_and_apostrophes() {
        let parsed = parse_language_file(
            "# comment\nmenu:\n  system_proxy: Proxy #1\n  empty: don't stop\nother:\n  k: a: b\n",
        );
        assert_eq!(parsed.get("menu.system_proxy").unwrap(), "Proxy #1");
        assert_eq!(parsed.get("menu.empty").unwrap(), "don't stop");
        // `# comment` on its own line is dropped, and only the first `:` splits.
        assert_eq!(parsed.get("other.k").unwrap(), "a: b");
        assert_eq!(parsed.len(), 3);
    }

    #[test]
    fn language_file_ignores_blank_values_and_junk() {
        // A value-less key behaves like a missing one, so the built-in table
        // still shows through instead of the item going blank.
        let parsed =
            parse_language_file("menu:\n  tun: TUN mode\n  reload:\n  groups: \"\"\nnope\n");
        assert_eq!(parsed.len(), 1);
        let mut messages = Messages::zh_cn();
        messages.overlay(&|key| parsed.get(key).cloned());
        assert_eq!(messages.menu_tun, "TUN mode");
        assert_eq!(messages.menu_reload, "重载配置");
        assert_eq!(messages.menu_groups, "代理分组");
    }

    #[test]
    fn overlay_keeps_keys_the_file_omits() {
        let mut messages = Messages::zh_cn();
        let file = parse_language_file("menu:\n  tun: TUN mode\n");
        messages.overlay(&|key| file.get(key).cloned());
        assert_eq!(messages.menu_tun, "TUN mode");
        assert_eq!(messages.menu_reload, "重载配置");
    }
}
