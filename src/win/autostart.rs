//! Auto-start through the per-user `Run` key (no elevation prompt).

use winreg::RegKey;
use winreg::enums::{HKEY_CURRENT_USER, KEY_READ, KEY_SET_VALUE};

use crate::i18n;

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const VALUE_NAME: &str = "mihomo-tray";

pub fn is_enabled() -> bool {
    let registered = RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey_with_flags(RUN_KEY, KEY_READ)
        .and_then(|key| key.get_value::<String, _>(VALUE_NAME))
        .ok();
    // A value that is there but names another file says nothing about *this*
    // program: the entry survives the executable being moved or replaced, and the
    // menu item then promises a start that will never happen.
    registered.is_some_and(|value| matches_current_exe(&value))
}

/// Whether a `Run` value is the one this executable would write: the quoted
/// current path, with `%NAME%` expanded.
///
/// `set` writes the quoted form, and the quoted form is the only one that survives
/// a path with spaces, so a comparison against it is exact enough. A value that
/// expands to something else — a moved program, or one installed over the old
/// location — counts as not enabled.
fn matches_current_exe(registered: &str) -> bool {
    let Ok(exe) = std::env::current_exe() else {
        // Without a path of our own nothing can be compared; reporting the entry
        // as ours keeps the menu item truthful about the registry.
        return true;
    };
    let quoted = format!("\"{}\"", exe.display());
    registered
        .trim()
        .eq_ignore_ascii_case(&crate::paths::expand(&quoted))
}

pub fn set(enabled: bool) -> Result<(), String> {
    let key = RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey_with_flags(RUN_KEY, KEY_READ | KEY_SET_VALUE)
        .map_err(|e| i18n::t().error_open_run_key(&e.to_string()))?;
    if enabled {
        let exe =
            std::env::current_exe().map_err(|e| i18n::t().error_current_exe(&e.to_string()))?;
        key.set_value(VALUE_NAME, &format!("\"{}\"", exe.display()))
            .map_err(|e| i18n::t().error_write_autostart(&e.to_string()))?;
    } else {
        match key.delete_value(VALUE_NAME) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(i18n::t().error_delete_autostart(&e.to_string())),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_current_executable_counts_as_registered() {
        let exe = std::env::current_exe().expect("test executable path");
        let ours = format!("\"{}\"", exe.display());
        assert!(matches_current_exe(&ours));
        // The spelling of a path is not part of its identity on Windows.
        assert!(matches_current_exe(&ours.to_uppercase()));
        // Trailing space is something the `Run` key tolerates in the values users
        // write by hand.
        assert!(matches_current_exe(&format!("{ours}  ")));

        // An entry left behind by a program that has since been moved names
        // another file, so this program is *not* set to start with Windows.
        assert!(!matches_current_exe(r#""D:\elsewhere\mihomo-tray.exe""#));
        assert!(!matches_current_exe(""));
    }
}
