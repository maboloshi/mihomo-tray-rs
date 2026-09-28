//! User-interface strings.
//!
//! Every label, status line and error message lives here instead of at its call
//! site, so the UI language is a runtime choice rather than a property of the
//! source. `zh-CN` is built in and is the default, `en-US` is built in as well.
//!
//! The language follows the Windows UI language and there is no setting for it:
//! the tag itself (`zh-CN`, `ja-JP`, …) names the `lang/<tag>.yml` file to look
//! for next to `tray.yml`. That file is layered on top of a built-in table, so a
//! new language is one translated file and a key the file omits falls back to a
//! built-in string instead of breaking the build.
//!
//! Values are `Cow<'static, str>`: a built-in string stays borrowed (no
//! allocation, still a plain `&str` for `AppendMenuW`) while a translated one is
//! owned. Both live in the same table, so no call site has to care which it got.

use std::borrow::Cow;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use windows_sys::Win32::Globalization::{GetUserDefaultUILanguage, LCIDToLocaleName};

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
    menu_web_ui => "menu.web_ui",
    menu_exit => "menu.exit",
    menu_exit_stop_kernel => "menu.exit_stop_kernel",
    menu_exit_only => "menu.exit_only",

    // Status line and tooltip.
    status_running_tpl => "status.running",
    status_controller_unreachable => "status.controller_unreachable",
    status_stopped => "status.stopped",
    status_looking_kernel => "status.looking_kernel",
    status_starting_kernel => "status.starting_kernel",
    status_kernel_silent => "status.kernel_silent",
    status_elevating_kernel => "status.elevating_kernel",
    status_elevating_stop => "status.elevating_stop",
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
    error_elevate_timeout => "error.elevate_timeout",
    error_elevated_kernel_failed => "error.elevated_kernel_failed",
    error_tun_ineffective => "error.tun_ineffective",
    error_kernel_needs_admin => "error.kernel_needs_admin",
    error_kernel_args => "error.kernel_args",
    error_message_window => "error.message_window",
    error_open_web_ui_tpl => "error.open_web_ui",
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

    pub fn error_open_web_ui(&self, code: &str) -> String {
        self.fill(&self.error_open_web_ui_tpl, &[("code", code)])
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

/// The built-in tables. Strings here are the ones this code used to carry
/// inline; treat every value as a released interface, not a scratch string.
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
            menu_web_ui: Cow::Borrowed("打开 Web 面板"),
            menu_exit: Cow::Borrowed("退出"),
            menu_exit_stop_kernel: Cow::Borrowed("退出并停止 Mihomo"),
            menu_exit_only: Cow::Borrowed("仅退出程序"),

            status_running_tpl: Cow::Borrowed("Mihomo 状态: 运行中 ({mode})"),
            status_controller_unreachable: Cow::Borrowed("Mihomo 状态: 内核运行中，控制器不可达"),
            status_stopped: Cow::Borrowed("Mihomo 状态: 未运行"),
            status_looking_kernel: Cow::Borrowed("正在查找内核…"),
            status_starting_kernel: Cow::Borrowed("正在启动内核…"),
            status_kernel_silent: Cow::Borrowed("内核尚未应答，仍在等待"),
            status_elevating_kernel: Cow::Borrowed("正在以管理员身份重启内核…"),
            status_elevating_stop: Cow::Borrowed("正在以管理员权限停止内核…"),
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
            error_elevate_timeout: Cow::Borrowed("等待管理员授权超时"),
            error_elevated_kernel_failed: Cow::Borrowed("以管理员权限启动内核失败"),
            error_tun_ineffective: Cow::Borrowed("TUN 未生效（通常需要管理员权限）"),
            error_kernel_needs_admin: Cow::Borrowed("内核以管理员权限运行，本程序无权停止它"),
            error_kernel_args: Cow::Borrowed("无法读取内核自己的启动参数，未重启内核"),
            error_message_window: Cow::Borrowed("创建消息窗口失败"),
            error_open_web_ui_tpl: Cow::Borrowed(
                "打开 Web 面板失败: 无法用默认浏览器打开（ShellExecute 返回 {code}）",
            ),
            error_open_internet_settings_tpl: Cow::Borrowed("打开 Internet Settings 失败: {error}"),
            error_set_proxy_enable_tpl: Cow::Borrowed("设置 ProxyEnable 失败: {error}"),
            error_set_proxy_server_tpl: Cow::Borrowed("设置 ProxyServer 失败: {error}"),
            error_set_proxy_override_tpl: Cow::Borrowed("设置 ProxyOverride 失败: {error}"),
        }
    }

    /// The built-in English table. `lang/en-US.yml` ships the same strings so a
    /// translator has a starting point; keep the two in step, the
    /// `shipped_template_matches_the_builtin_table` test fails otherwise.
    fn en_us() -> Self {
        Self {
            menu_system_proxy: Cow::Borrowed("System proxy"),
            menu_mode: Cow::Borrowed("Proxy mode"),
            menu_mode_rule: Cow::Borrowed("Rule"),
            menu_mode_global: Cow::Borrowed("Global"),
            menu_mode_direct: Cow::Borrowed("Direct"),
            menu_tun: Cow::Borrowed("TUN mode"),
            menu_groups: Cow::Borrowed("Proxy groups"),
            menu_group_label_tpl: Cow::Borrowed("{name} ({kind})"),
            menu_group_label_pinned_tpl: Cow::Borrowed("{name} ({kind} · pinned)"),
            menu_unfix: Cow::Borrowed("Automatic (unpin)"),
            menu_truncated: Cow::Borrowed("(too many items, omitted)"),
            menu_empty: Cow::Borrowed("(empty)"),
            menu_autostart: Cow::Borrowed("Start with Windows"),
            menu_reload: Cow::Borrowed("Reload config"),
            menu_web_ui: Cow::Borrowed("Open web dashboard"),
            menu_exit: Cow::Borrowed("Exit"),
            menu_exit_stop_kernel: Cow::Borrowed("Exit and stop Mihomo"),
            menu_exit_only: Cow::Borrowed("Exit only"),

            status_running_tpl: Cow::Borrowed("Mihomo status: running ({mode})"),
            status_controller_unreachable: Cow::Borrowed(
                "Mihomo status: kernel running, controller unreachable",
            ),
            status_stopped: Cow::Borrowed("Mihomo status: not running"),
            status_looking_kernel: Cow::Borrowed("Looking for the kernel…"),
            status_starting_kernel: Cow::Borrowed("Starting the kernel…"),
            status_kernel_silent: Cow::Borrowed("The kernel has not answered yet; still waiting"),
            status_elevating_kernel: Cow::Borrowed("Restarting the kernel as administrator…"),
            status_elevating_stop: Cow::Borrowed("Stopping the kernel as administrator…"),
            state_on: Cow::Borrowed("on"),
            state_off: Cow::Borrowed("off"),
            tooltip_kernel_tpl: Cow::Borrowed("Kernel: mihomo {version}"),
            tooltip_proxy_tun_tpl: Cow::Borrowed("System proxy: {proxy} · TUN: {tun}"),
            tooltip_port_tpl: Cow::Borrowed("Port: {port}"),

            error_kernel_not_found: Cow::Borrowed(
                "mihomo.exe not found; set mihomo.path in tray.yml",
            ),
            error_read_settings_tpl: Cow::Borrowed("Failed to read {path}: {error}"),
            error_window_create_tpl: Cow::Borrowed("Startup failed: {error}"),
            error_no_kernel_path: Cow::Borrowed(
                "No mihomo.exe path is known, cannot tell which process to stop",
            ),
            error_no_matching_kernel: Cow::Borrowed(
                "No mihomo process matches this program's configuration",
            ),
            error_spawn_worker_tpl: Cow::Borrowed("Failed to start the polling thread: {error}"),
            error_controller_unreachable_tpl: Cow::Borrowed(
                "Controller {address} unreachable: {error}",
            ),
            error_winhttp_open: Cow::Borrowed("WinHttpOpen failed"),
            error_request_failed: Cow::Borrowed("Request failed"),
            error_response_too_large: Cow::Borrowed("Response too large (over 8 MiB)"),
            error_http_request_failed: Cow::Borrowed("HTTP request failed"),
            error_json_parse_tpl: Cow::Borrowed("JSON parse failed: {error}"),
            error_start_process_tpl: Cow::Borrowed("Failed to start {path}: {error}"),
            error_open_process_tpl: Cow::Borrowed("Cannot open process {pid}"),
            error_kill_process_tpl: Cow::Borrowed("Failed to stop process {pid}"),
            error_open_run_key_tpl: Cow::Borrowed("Failed to open the Run key: {error}"),
            error_current_exe_tpl: Cow::Borrowed("Failed to get the program path: {error}"),
            error_write_autostart_tpl: Cow::Borrowed(
                "Failed to write the autostart entry: {error}",
            ),
            error_delete_autostart_tpl: Cow::Borrowed(
                "Failed to delete the autostart entry: {error}",
            ),
            error_elevate_cancelled: Cow::Borrowed("Elevation was cancelled or failed"),
            error_elevate_timeout: Cow::Borrowed("Timed out waiting for administrator consent"),
            error_elevated_kernel_failed: Cow::Borrowed(
                "Failed to start the kernel with administrator rights",
            ),
            error_tun_ineffective: Cow::Borrowed(
                "TUN did not take effect (usually needs administrator rights)",
            ),
            error_kernel_needs_admin: Cow::Borrowed(
                "The kernel runs with administrator rights and cannot be stopped by this program",
            ),
            error_kernel_args: Cow::Borrowed(
                "Could not read the kernel's own command line, so it was not restarted",
            ),
            error_message_window: Cow::Borrowed("Failed to create the message window"),
            error_open_web_ui_tpl: Cow::Borrowed(
                "Failed to open the web dashboard: could not launch the default browser (ShellExecute returned {code})",
            ),
            error_open_internet_settings_tpl: Cow::Borrowed(
                "Failed to open Internet Settings: {error}",
            ),
            error_set_proxy_enable_tpl: Cow::Borrowed("Failed to set ProxyEnable: {error}"),
            error_set_proxy_server_tpl: Cow::Borrowed("Failed to set ProxyServer: {error}"),
            error_set_proxy_override_tpl: Cow::Borrowed("Failed to set ProxyOverride: {error}"),
        }
    }
}

static MESSAGES: OnceLock<Messages> = OnceLock::new();
static LANGUAGE: OnceLock<String> = OnceLock::new();

/// The active table. Without [`init`] (unit tests) this is the built-in default,
/// so no call site has to handle "no language chosen yet".
pub fn t() -> &'static Messages {
    MESSAGES.get_or_init(Messages::zh_cn)
}

/// The primary subtag of the active UI language: `zh` or `en`, whichever table
/// the messages come from.
///
/// This is the one thing outside this module that depends on the language rather
/// than on a message: the sample `tray.yml` written on first run, which should be
/// in a language its reader understands. Without [`init`] it is the default
/// table's subtag, exactly like [`t`] is the default table.
pub fn language() -> &'static str {
    LANGUAGE.get().map_or(DEFAULT_LANGUAGE, String::as_str)
}

/// Resolve and install the UI language.
///
/// Called first thing in `main`: code that runs later reports failures through
/// this table, so the language has to be settled before anything can fail.
pub fn init() {
    let tag = system_tag();
    let _ = MESSAGES.set(resolve(&tag));
    let _ = LANGUAGE.set(primary(&tag).to_string());
}

fn resolve(tag: &str) -> Messages {
    resolve_in(&language_dir(), tag)
}

/// The testable core of [`resolve`]: the directory is a parameter so a test can
/// point at a scratch file instead of the real `tray.yml` directory.
fn resolve_in(dir: &Path, tag: &str) -> Messages {
    match read_language_file(dir, tag) {
        Some(translated) => {
            let mut messages = fallback();
            messages.overlay(&|key| lookup(&translated, key));
            messages
        }
        None => builtin(tag),
    }
}

/// The table a language file is layered on: a translation may cover only part of
/// the UI, and every key it leaves out comes from here rather than showing up
/// blank. Deliberately independent of `tag`, so a key is missing in the same way
/// whichever file is incomplete.
fn fallback() -> Messages {
    Messages::en_us()
}

/// The table compiled in for `tag`, matched on the primary language so `en-GB`
/// finds the English table. Anything unrecognised falls back to the default, so
/// an unsupported language degrades to a working UI instead of an empty one.
fn builtin(tag: &str) -> Messages {
    match primary(tag) {
        "en" => Messages::en_us(),
        _ => Messages::zh_cn(),
    }
}

/// The primary subtag of a BCP-47 tag: `en-GB` and `en_US` are both `en`.
fn primary(tag: &str) -> &str {
    tag.split(['-', '_']).next().unwrap_or_default()
}

/// The tag of the default table, and the answer when Windows cannot report a UI
/// language at all.
const DEFAULT_TAG: &str = "zh-CN";

/// The primary subtag of [`DEFAULT_TAG`], used before a language is installed.
const DEFAULT_LANGUAGE: &str = "zh";

/// The Windows UI language as a BCP-47 tag such as `zh-CN` or `ja-JP`. The tag
/// doubles as the language-file name, so a translation is picked up by naming it
/// after the language Windows already runs in.
fn system_tag() -> String {
    /// `LOCALE_NAME_MAX_LENGTH`, the documented maximum for a locale name.
    const LOCALE_NAME_MAX_LENGTH: usize = 85;
    let langid = unsafe { GetUserDefaultUILanguage() };
    // An LCID is a LANGID in the default sort order, and `SORT_DEFAULT` is zero.
    let mut buffer = [0u16; LOCALE_NAME_MAX_LENGTH];
    let len = unsafe {
        LCIDToLocaleName(
            u32::from(langid),
            buffer.as_mut_ptr(),
            buffer.len() as i32,
            0,
        )
    };
    if len <= 1 {
        return DEFAULT_TAG.to_string();
    }
    // `len` counts the terminating NUL, which is not part of the name.
    String::from_utf16_lossy(&buffer[..len as usize - 1])
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

/// Read `lang/<tag>.yml` from `dir`. `None` means "no such file", which selects
/// the built-in table.
fn read_language_file(dir: &Path, tag: &str) -> Option<Vec<(String, String)>> {
    let text = std::fs::read_to_string(dir.join(format!("{tag}.yml"))).ok()?;
    Some(parse_language_file(&text))
}

/// The value for `key`, or `None` when the file does not define it. Later lines
/// win, which is how an override added at the bottom of a file is read.
///
/// A linear scan is deliberate: the table has a few dozen entries and is read
/// once at startup, and a `HashMap` would drag hashing and table code into a
/// binary that is measured in kilobytes.
fn lookup(entries: &[(String, String)], key: &str) -> Option<String> {
    entries
        .iter()
        .rev()
        .find(|(name, _)| name == key)
        .map(|(_, value)| value.clone())
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
fn parse_language_file(text: &str) -> Vec<(String, String)> {
    let mut entries: Vec<(String, String)> = Vec::new();
    let mut section = String::new();
    for raw in crate::settings::strip_bom(text).lines() {
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
            entries.push((format!("{section}.{key}"), value));
        }
    }
    entries
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `{hole}` names in a template, in order.
    fn holes(text: &str) -> Vec<String> {
        let mut found = Vec::new();
        let mut rest = text;
        while let Some(start) = rest.find('{') {
            let Some(end) = rest[start..].find('}') else {
                break;
            };
            found.push(rest[start + 1..start + end].to_string());
            rest = &rest[start + end + 1..];
        }
        found
    }

    #[test]
    fn builtin_tables_agree_on_keys_and_holes() {
        let zh_messages = Messages::zh_cn();
        let en_messages = Messages::en_us();
        let zh = zh_messages.entries();
        let en = en_messages.entries();
        assert_eq!(zh.len(), en.len());
        for ((key, zh_value), (en_key, en_value)) in zh.iter().zip(&en) {
            assert_eq!(key, en_key);
            assert!(!zh_value.trim().is_empty(), "{key} is empty in zh-CN");
            assert!(!en_value.trim().is_empty(), "{key} is empty in en-US");
            assert_eq!(holes(zh_value), holes(en_value), "{key} holes differ");
        }
    }

    #[test]
    fn shipped_template_matches_the_builtin_table() {
        let template = parse_language_file(include_str!("../lang/en-US.yml"));
        let en_messages = Messages::en_us();
        let builtin = en_messages.entries();
        assert_eq!(template.len(), builtin.len());
        for (key, value) in builtin {
            assert_eq!(lookup(&template, key).as_deref(), Some(value), "{key}");
        }
    }

    #[test]
    fn system_tag_looks_like_a_language_tag() {
        // Exercises the Win32 call: a bad `LCID` leaves the buffer empty and the
        // tag would then never match any file name.
        let tag = system_tag();
        let mut chars = tag.chars();
        assert!(
            chars.next().is_some_and(|c| c.is_ascii_alphabetic())
                && chars.next().is_some_and(|c| c.is_ascii_alphabetic()),
            "{tag:?} is not a language tag"
        );
    }

    #[test]
    fn builtin_matches_the_language_not_the_region() {
        assert_eq!(builtin("en-GB").menu_exit.to_string(), "Exit");
        assert_eq!(builtin("en_US").menu_exit.to_string(), "Exit");
        assert_eq!(builtin("en").menu_exit.to_string(), "Exit");
        // Unsupported languages keep the default table rather than an empty UI.
        assert_eq!(builtin("ja-JP").menu_exit.to_string(), "退出");
        assert_eq!(builtin("zh-TW").menu_exit.to_string(), "退出");
    }

    #[test]
    fn the_language_is_the_primary_subtag() {
        assert_eq!(primary("zh-CN"), "zh");
        assert_eq!(primary("en_US"), "en");
        assert_eq!(primary("en"), "en");
        assert_eq!(primary(DEFAULT_TAG), DEFAULT_LANGUAGE);
        // Before `init` it mirrors `t()`: the built-in default table's language.
        assert_eq!(language(), DEFAULT_LANGUAGE);
    }

    #[test]
    fn a_language_file_is_layered_on_the_fallback() {
        let dir = std::env::temp_dir().join(format!("mihomo-tray-i18n-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create scratch language directory");
        let file = dir.join("ja-JP.yml");
        std::fs::write(&file, "# scratch\nmenu:\n  exit: 終了\n").expect("write scratch file");

        let messages = resolve_in(&dir, "ja-JP");
        assert_eq!(messages.menu_exit.to_string(), "終了");
        // Keys the file omits come from the fallback table, not from nothing.
        assert_eq!(messages.menu_reload.to_string(), "Reload config");
        // A tag with no file at all keeps the built-in default.
        assert_eq!(resolve_in(&dir, "de-DE").menu_exit.to_string(), "退出");

        let _ = std::fs::remove_file(&file);
        let _ = std::fs::remove_dir(&dir);
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
        assert_eq!(lookup(&parsed, "menu.system_proxy").unwrap(), "Proxy #1");
        assert_eq!(lookup(&parsed, "menu.empty").unwrap(), "don't stop");
        // `# comment` on its own line is dropped, and only the first `:` splits.
        assert_eq!(lookup(&parsed, "other.k").unwrap(), "a: b");
        assert_eq!(parsed.len(), 3);
    }

    #[test]
    fn a_bom_does_not_hide_the_first_section() {
        // Translators edit these files on Windows editors that may add a BOM;
        // without skipping it every key would carry it and no translation would
        // ever be found.
        let parsed = parse_language_file("\u{feff}menu:\n  exit: 終了\n");
        assert_eq!(lookup(&parsed, "menu.exit").as_deref(), Some("終了"));
    }

    #[test]
    fn language_file_ignores_blank_values_and_junk() {
        // A value-less key behaves like a missing one, so the built-in table
        // still shows through instead of the item going blank.
        let parsed =
            parse_language_file("menu:\n  tun: TUN mode\n  reload:\n  groups: \"\"\nnope\n");
        assert_eq!(parsed.len(), 1);
        let mut messages = Messages::zh_cn();
        messages.overlay(&|key| lookup(&parsed, key));
        assert_eq!(messages.menu_tun, "TUN mode");
        assert_eq!(messages.menu_reload, "重载配置");
        assert_eq!(messages.menu_groups, "代理分组");
    }

    #[test]
    fn overlay_keeps_keys_the_file_omits() {
        let mut messages = Messages::zh_cn();
        let file = parse_language_file("menu:\n  tun: TUN mode\n");
        messages.overlay(&|key| lookup(&file, key));
        assert_eq!(messages.menu_tun, "TUN mode");
        assert_eq!(messages.menu_reload, "重载配置");
    }
}
