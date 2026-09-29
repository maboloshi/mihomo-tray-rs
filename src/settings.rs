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

#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    /// The kernel to start. Empty means "find it": the discovery chain looks beside
    /// this executable, then along `PATH` — where a scoop shim is followed to the
    /// binary it launches (`discover::find_kernel`). A kernel that is not called
    /// `mihomo.exe` has to be named here: nothing searches for another name.
    pub mihomo_path: String,
    /// mihomo's `-d`: the directory its configuration, cache and geodata live in.
    /// Empty means the directory of the configuration file the kernel brings along
    /// — or the kernel's own default, which mihomo documents as
    /// `%USERPROFILE%\.config\mihomo`.
    pub mihomo_home: String,
    pub mihomo_args: Vec<String>,
    /// mihomo's `-f`. Empty means `config.yaml` under the home directory above.
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
    /// The dashboard "open the web UI" points at. Empty means "the kernel's own
    /// `external-ui`, under the controller address" — a kernel without
    /// `external-ui` configured serves nothing there, hence the override.
    pub web_url: String,
    pub poll_interval_ms: u32,
    pub dark_menu: DarkMenu,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            mihomo_path: String::new(),
            mihomo_home: String::new(),
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
            web_url: String::new(),
            poll_interval_ms: 3000,
            dark_menu: DarkMenu::Auto,
        }
    }
}

/// The commented template written when there is no `tray.yml` anywhere, one file
/// per language the project ships.
///
/// Both samples are part of the source, so the file a user edits and the file
/// this program would write are literally the same text: the copy in the
/// repository cannot drift from the generated one. `language` is a primary
/// subtag (`zh`, `en`, …); anything else gets English, which is also the table
/// every message falls back to.
pub fn default_file(language: &str) -> &'static str {
    match language {
        "zh" => include_str!("../tray_Sample_zh.yml"),
        _ => include_str!("../tray_Sample_en.yml"),
    }
}

/// `tray.yml` next to the executable wins (portable layout), otherwise the file
/// lives in `%APPDATA%\mihomo-tray`.
///
/// The portable file has to carry something to count. Scoop's manifest creates an
/// empty one next to the executable, and an empty file would otherwise win over
/// the settings the user actually edits in `%APPDATA%` — every line of them
/// silently ignored, which is exactly what happened on a real machine.
pub fn settings_path() -> PathBuf {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let portable = dir.join("tray.yml");
            if has_content(&portable) {
                return portable;
            }
        }
    }
    if let Ok(appdata) = std::env::var("APPDATA") {
        return PathBuf::from(appdata).join("mihomo-tray").join("tray.yml");
    }
    PathBuf::from("tray.yml")
}

/// Whether `path` holds something a user could have meant as settings: anything
/// but whitespace, a BOM included. A file that cannot be read at all counts as
/// empty too, so an unreadable portable file does not shadow the `%APPDATA%` one.
fn has_content(path: &Path) -> bool {
    std::fs::read_to_string(path).is_ok_and(|text| !strip_bom(&text).trim().is_empty())
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
/// to edit. `text` is the sample for the active language — [`default_file`].
pub fn ensure_default_file(path: &Path, text: &str) {
    if path.exists() {
        return;
    }
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(path, text);
}

/// Upper bound on `groups.page_size`. A page is a submenu of the group's members,
/// and the entry budget caps how many of those survive anyway, so a larger number
/// buys nothing while an absurd one (`999999999`) would defeat the paging it was
/// meant to configure. `0` keeps its documented meaning: no paging.
const MAX_PAGE_SIZE: usize = 1000;

fn parse(text: &str) -> Settings {
    let mut s = Settings::default();
    let mut section = String::new();
    for raw in strip_bom(text).lines() {
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
            ("mihomo", "home") => s.mihomo_home = unquote(value),
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
            ("groups", "page_size") => {
                s.groups_page_size = value.parse().unwrap_or(0).min(MAX_PAGE_SIZE)
            }
            ("ui", "web_url") => s.web_url = unquote(value),
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

/// Drop a leading UTF-8 BOM, which a Windows editor may well have written.
///
/// Neither parser would otherwise recognise the first line: it would become a
/// section named `\u{feff}mihomo` (or `\u{feff}menu`) and every key under it
/// would be silently ignored — the whole file, effectively.
pub(crate) fn strip_bom(text: &str) -> &str {
    text.strip_prefix('\u{feff}').unwrap_or(text)
}

/// Drop a trailing `#` comment that is outside quotes. Shared with the mihomo
/// config reader so `secret: "tok#en"` survives there too.
///
/// A quote only opens where the same quote closes later in the line: an
/// apostrophe in an unquoted scalar (`don't`) is punctuation, not the start of a
/// quoted string, and treating it as one would swallow the comment after it.
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
                '\'' | '"' if line[i + c.len_utf8()..].contains(c) => quote = Some(c),
                '#' if prev.is_whitespace() => return &line[..i],
                _ => {}
            },
        }
        prev = c;
    }
    line
}

/// Split the items of an inline list on the commas that separate them, ignoring
/// the ones inside a quoted item (`["a,b"]` is one entry).
fn split_items(inner: &str) -> Vec<&str> {
    let mut items = Vec::new();
    let mut start = 0;
    let mut quote: Option<char> = None;
    for (i, c) in inner.char_indices() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => {}
            None => match c {
                '\'' | '"' if inner[i + c.len_utf8()..].contains(c) => quote = Some(c),
                ',' => {
                    items.push(&inner[start..i]);
                    start = i + 1;
                }
                _ => {}
            },
        }
    }
    items.push(&inner[start..]);
    items
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
    split_items(inner)
        .into_iter()
        .map(unquote)
        .filter(|item| !item.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_samples_describe_the_same_defaults() {
        // The samples differ only in comments, so they have to parse to the very
        // same settings: a translation that changed a value would otherwise
        // change how the program behaves with the UI language.
        let en = parse(default_file("en"));
        let zh = parse(default_file("zh"));
        assert_eq!(en, zh);
        assert_eq!(en, Settings::default());
        // An unsupported language still gets a usable file rather than nothing.
        assert_eq!(parse(default_file("ja")), en);
    }

    #[test]
    fn an_empty_portable_file_is_not_settings() {
        // Scoop's manifest creates an empty `tray.yml` next to the executable;
        // counting it as settings is what silently ignored the user's own file.
        let dir = std::env::temp_dir().join("mihomo-tray-test-settings-content");
        std::fs::create_dir_all(&dir).unwrap();
        let empty = dir.join("empty.yml");
        std::fs::write(&empty, "").unwrap();
        let blank = dir.join("blank.yml");
        std::fs::write(&blank, "\u{feff}\r\n  \n").unwrap();
        let real = dir.join("real.yml");
        std::fs::write(&real, "mixed-port: 7890\n").unwrap();
        let missing = dir.join("missing.yml");

        assert!(!has_content(&empty), "an empty file is not settings");
        assert!(!has_content(&blank), "whitespace is not settings");
        assert!(!has_content(&missing), "a missing file is not settings");
        assert!(has_content(&real));

        let _ = std::fs::remove_file(&empty);
        let _ = std::fs::remove_file(&blank);
        let _ = std::fs::remove_file(&real);
        let _ = std::fs::remove_dir(&dir);
    }

    #[test]
    fn parses_values_paths_and_cjk() {
        let text = "\
controller:
  address: 127.0.0.1:9098   # inline comment
  secret: \"tok#en\"
mihomo:
  path: \"D:\\App\\Scoop\\shims\\mihomo.exe\"
  home: \"%USERPROFILE%\\.config\\mihomo\"
  args: ['-m', '-ext-ui=C:\\ui']
proxy:
  bypass: [\"*.local\", 'example.com']
groups:
  page_size: 50
ui:
  dark_menu: always
  web_url: \"https://board.zash.run.place/#/setup?hostname={host}&port={port}\"
";
        let s = parse(text);
        assert_eq!(s.controller_address, "127.0.0.1:9098");
        assert_eq!(s.controller_secret, "tok#en");
        assert_eq!(s.mihomo_path, r"D:\App\Scoop\shims\mihomo.exe");
        assert_eq!(s.mihomo_home, r"%USERPROFILE%\.config\mihomo");
        assert_eq!(s.mihomo_args, vec!["-m", r"-ext-ui=C:\ui"]);
        assert_eq!(s.proxy_bypass, vec!["*.local", "example.com"]);
        assert_eq!(s.groups_page_size, 50);
        assert_eq!(s.dark_menu, DarkMenu::Always);
        // A `#` inside quotes is part of the URL, not a comment.
        assert_eq!(
            s.web_url,
            "https://board.zash.run.place/#/setup?hostname={host}&port={port}"
        );
    }

    #[test]
    fn missing_file_falls_back_to_defaults() {
        let (s, err) = load(Path::new(r"Z:\definitely\missing\tray.yml"));
        assert!(err.is_none());
        assert_eq!(s.poll_interval_ms, 3000);
    }

    #[test]
    fn a_bom_does_not_hide_the_first_section() {
        // A file saved by a Windows editor may start with a BOM; without skipping
        // it the first section is called `\u{feff}mihomo` and its keys are lost.
        let text = format!("\u{feff}{}", default_file("zh"));
        let s = parse(&text);
        assert_eq!(s.controller_timeout_ms, 2000);
        assert_eq!(s.poll_interval_ms, 3000);
    }

    #[test]
    fn an_apostrophe_does_not_hide_the_comment_behind_it() {
        let s = parse("controller:\n  secret: don't tell   # not part of the secret\n");
        assert_eq!(s.controller_secret, "don't tell");
        // A `#` with no space in front of it is still part of the value.
        let s = parse("controller:\n  secret: tok#en\n");
        assert_eq!(s.controller_secret, "tok#en");
    }

    #[test]
    fn a_comma_inside_quotes_stays_in_its_item() {
        let s = parse("proxy:\n  bypass: [\"a,b\", c]\n");
        assert_eq!(s.proxy_bypass, vec!["a,b", "c"]);
    }

    #[test]
    fn a_page_size_out_of_range_is_clamped() {
        // `0` is documented as "no paging" and stays exactly that.
        assert_eq!(parse("groups:\n  page_size: 0\n").groups_page_size, 0);
        assert_eq!(parse("groups:\n  page_size: 50\n").groups_page_size, 50);
        // The same bound the other intervals get: nothing pathological reaches the
        // code that splits a group into pages.
        assert_eq!(
            parse("groups:\n  page_size: 999999999\n").groups_page_size,
            MAX_PAGE_SIZE
        );
        // Not a number is not a page size; it keeps the documented default.
        assert_eq!(parse("groups:\n  page_size: many\n").groups_page_size, 0);
    }
}
