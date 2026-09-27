//! Settings file (`tray.yml`) loading.
//!
//! Deliberately a small, documented subset of YAML: top level keys, one level of
//! nesting, `#` comments, single/double quoted scalars and inline `[a, b]` lists.
//! The file belongs to this program, so the subset is enough and saves both a
//! dependency and binary size.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DarkMenu {
    Auto,
    Always,
    Never,
}

#[derive(Debug, Clone)]
pub struct Settings {
    pub mihomo_path: String,
    pub mihomo_args: Vec<String>,
    pub mihomo_config: String,
    pub mihomo_auto_start: bool,
    pub controller_address: String,
    pub controller_secret: String,
    pub controller_timeout_ms: u32,
    pub proxy_bypass: Vec<String>,
    pub groups_order: Vec<String>,
    pub groups_include: Vec<String>,
    pub groups_exclude: Vec<String>,
    pub groups_page_size: usize,
    pub poll_interval_ms: u32,
    pub dark_menu: DarkMenu,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            mihomo_path: String::new(),
            mihomo_args: Vec::new(),
            mihomo_config: String::new(),
            mihomo_auto_start: true,
            controller_address: String::new(),
            controller_secret: String::new(),
            controller_timeout_ms: 2000,
            proxy_bypass: Vec::new(),
            groups_order: Vec::new(),
            groups_include: Vec::new(),
            groups_exclude: Vec::new(),
            groups_page_size: 0,
            poll_interval_ms: 3000,
            dark_menu: DarkMenu::Auto,
        }
    }
}

pub const DEFAULT_FILE: &str = "\
# mihomo-tray settings. Everything here is optional.
# This is a small YAML subset: top level keys, one nesting level, '#' comments,
# quoted scalars and inline [a, b] lists.

mihomo:
  path: \"\"            # empty = auto discover mihomo.exe
  args: []             # extra launch arguments, e.g. ['-d', 'C:\\mihomo']
  config: \"\"          # explicit config file path (also used to read external-controller)
  auto_start: true     # start the kernel on launch when it is not running

controller:
  address: \"\"         # e.g. 127.0.0.1:9090; empty = auto discover
  secret: \"\"
  timeout_ms: 2000

proxy:
  bypass: []           # extra ProxyOverride entries; '<local>' is always added

groups:
  order: []            # explicit group order; the rest is sorted case-insensitively
  include: []          # empty = every selector group
  exclude: []
  page_size: 0         # 0 = let the system scroll long menus; >0 = page long groups

ui:
  poll_interval_ms: 3000
  dark_menu: auto      # auto | always | never
";

/// `tray.yml` next to the executable wins (portable layout), otherwise the file
/// lives in `%APPDATA%\mihomo-tray`.
pub fn settings_path() -> PathBuf {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let portable = dir.join("tray.yml");
            if portable.exists() {
                return portable;
            }
        }
    }
    if let Ok(appdata) = std::env::var("APPDATA") {
        return PathBuf::from(appdata).join("mihomo-tray").join("tray.yml");
    }
    PathBuf::from("tray.yml")
}

/// A settings file that exists but could not be read. The message is rendered by
/// the caller, which is the only place that knows the active UI language.
#[derive(Debug)]
pub struct LoadError {
    pub path: PathBuf,
    pub error: std::io::Error,
}

/// Load the settings file, or defaults when it does not exist.
pub fn load(path: &Path) -> (Settings, Option<LoadError>) {
    match std::fs::read_to_string(path) {
        Ok(text) => (parse(&text), None),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (Settings::default(), None),
        Err(e) => (
            Settings::default(),
            Some(LoadError {
                path: path.to_path_buf(),
                error: e,
            }),
        ),
    }
}

/// Write the commented default file when it is missing, so users have something
/// to edit.
pub fn ensure_default_file(path: &Path) {
    if path.exists() {
        return;
    }
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(path, DEFAULT_FILE);
}

fn parse(text: &str) -> Settings {
    let mut s = Settings::default();
    let mut section = String::new();
    for raw in text.lines() {
        let line = strip_comment(raw);
        if line.trim().is_empty() {
            continue;
        }
        let indent = line.len() - line.trim_start().len();
        let content = line.trim();
        let Some((key, value)) = content.split_once(':') else {
            continue;
        };
        let key = key.trim();
        let value = value.trim();
        if indent == 0 {
            section = key.to_string();
            continue;
        }
        match (section.as_str(), key) {
            ("mihomo", "path") => s.mihomo_path = unquote(value),
            ("mihomo", "args") => s.mihomo_args = parse_list(value),
            ("mihomo", "config") => s.mihomo_config = unquote(value),
            ("mihomo", "auto_start") => s.mihomo_auto_start = parse_bool(value, true),
            ("controller", "address") => s.controller_address = unquote(value),
            ("controller", "secret") => s.controller_secret = unquote(value),
            ("controller", "timeout_ms") => {
                s.controller_timeout_ms = value.parse().unwrap_or(2000).clamp(200, 30_000)
            }
            ("proxy", "bypass") => s.proxy_bypass = parse_list(value),
            ("groups", "order") => s.groups_order = parse_list(value),
            ("groups", "include") => s.groups_include = parse_list(value),
            ("groups", "exclude") => s.groups_exclude = parse_list(value),
            ("groups", "page_size") => s.groups_page_size = value.parse().unwrap_or(0),
            ("ui", "poll_interval_ms") => {
                s.poll_interval_ms = value.parse().unwrap_or(3000).clamp(500, 60_000)
            }
            ("ui", "dark_menu") => {
                s.dark_menu = match unquote(value).to_ascii_lowercase().as_str() {
                    "always" => DarkMenu::Always,
                    "never" => DarkMenu::Never,
                    _ => DarkMenu::Auto,
                }
            }
            _ => {}
        }
    }
    s
}

/// Drop a trailing `#` comment that is outside quotes. Shared with the mihomo
/// config reader so `secret: "tok#en"` survives there too.
pub(crate) fn strip_comment(line: &str) -> &str {
    let mut quote: Option<char> = None;
    let mut prev = ' ';
    for (i, c) in line.char_indices() {
        match quote {
            Some(q) => {
                if c == q {
                    quote = None;
                }
            }
            None => match c {
                '\'' | '"' => quote = Some(c),
                '#' if prev.is_whitespace() => return &line[..i],
                _ => {}
            },
        }
        prev = c;
    }
    line
}

/// Strip one matching pair of surrounding quotes. Shared with the language-file
/// reader, which accepts the same quoted scalars.
pub(crate) fn unquote(v: &str) -> String {
    let v = v.trim();
    let bytes = v.as_bytes();
    if bytes.len() >= 2
        && ((bytes[0] == b'"' && bytes[bytes.len() - 1] == b'"')
            || (bytes[0] == b'\'' && bytes[bytes.len() - 1] == b'\''))
    {
        return v[1..v.len() - 1].to_string();
    }
    v.to_string()
}

fn parse_bool(v: &str, fallback: bool) -> bool {
    match unquote(v).to_ascii_lowercase().as_str() {
        "true" | "yes" | "on" | "1" => true,
        "false" | "no" | "off" | "0" => false,
        _ => fallback,
    }
}

/// Inline list: `[]`, `[a, b]`, `["a", "b"]`. Empty means "no entries".
fn parse_list(v: &str) -> Vec<String> {
    let v = v.trim();
    let inner = v.strip_prefix('[').and_then(|x| x.strip_suffix(']'));
    let Some(inner) = inner else {
        let one = unquote(v);
        return if one.is_empty() {
            Vec::new()
        } else {
            vec![one]
        };
    };
    inner
        .split(',')
        .map(unquote)
        .filter(|item| !item.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_default_file() {
        let s = parse(DEFAULT_FILE);
        assert_eq!(s.mihomo_path, "");
        assert!(s.mihomo_args.is_empty());
        assert!(s.mihomo_auto_start);
        assert_eq!(s.controller_timeout_ms, 2000);
        assert_eq!(s.poll_interval_ms, 3000);
        assert_eq!(s.groups_page_size, 0);
        assert_eq!(s.dark_menu, DarkMenu::Auto);
    }

    #[test]
    fn parses_values_paths_and_cjk() {
        let text = "\
controller:
  address: 127.0.0.1:9098   # inline comment
  secret: \"tok#en\"
mihomo:
  path: \"D:\\App\\Scoop\\shims\\mihomo.exe\"
  args: ['-d', \"C:\\Users\\沙漠之子\\.config\\mihomo\"]
proxy:
  bypass: [\"*.local\", 'example.com']
groups:
  page_size: 50
ui:
  dark_menu: always
";
        let s = parse(text);
        assert_eq!(s.controller_address, "127.0.0.1:9098");
        assert_eq!(s.controller_secret, "tok#en");
        assert_eq!(s.mihomo_path, r"D:\App\Scoop\shims\mihomo.exe");
        assert_eq!(
            s.mihomo_args,
            vec!["-d", r"C:\Users\沙漠之子\.config\mihomo"]
        );
        assert_eq!(s.proxy_bypass, vec!["*.local", "example.com"]);
        assert_eq!(s.groups_page_size, 50);
        assert_eq!(s.dark_menu, DarkMenu::Always);
    }

    #[test]
    fn missing_file_falls_back_to_defaults() {
        let (s, err) = load(Path::new(r"Z:\definitely\missing\tray.yml"));
        assert!(err.is_none());
        assert_eq!(s.poll_interval_ms, 3000);
    }
}
