//! Auto-start through the per-user `Run` key (no elevation prompt).

use winreg::RegKey;
use winreg::enums::{HKEY_CURRENT_USER, KEY_READ, KEY_SET_VALUE};

use crate::i18n;

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const VALUE_NAME: &str = "mihomo-tray";

pub fn is_enabled() -> bool {
    RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey_with_flags(RUN_KEY, KEY_READ)
        .and_then(|key| key.get_value::<String, _>(VALUE_NAME))
        .is_ok()
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
