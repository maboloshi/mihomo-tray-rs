//! Windows system proxy (WinINet per-user settings).
//!
//! The mihomo core never touches these registry values, so the tray owns them.
//! Writing alone is not enough: WinINet has to be told to re-read them, which is
//! the classic reason "the proxy was set but nothing changed".

use windows_sys::Win32::Networking::WinInet::{
    INTERNET_OPTION_REFRESH, INTERNET_OPTION_SETTINGS_CHANGED, InternetSetOptionW,
};
use winreg::RegKey;
use winreg::enums::{HKEY_CURRENT_USER, KEY_READ, KEY_SET_VALUE};

use crate::i18n;

const INTERNET_SETTINGS: &str = r"Software\Microsoft\Windows\CurrentVersion\Internet Settings";

fn read() -> Result<RegKey, String> {
    RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey_with_flags(INTERNET_SETTINGS, KEY_READ | KEY_SET_VALUE)
        .map_err(|e| i18n::t().error_open_internet_settings(&e.to_string()))
}

pub fn is_enabled() -> bool {
    RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey_with_flags(INTERNET_SETTINGS, KEY_READ)
        .and_then(|key| key.get_value::<u32, _>("ProxyEnable"))
        .map(|value| value == 1)
        .unwrap_or(false)
}

pub fn enable(port: u16, extra_bypass: &[String]) -> Result<(), String> {
    let key = read()?;
    key.set_value("ProxyEnable", &1u32)
        .map_err(|e| i18n::t().error_set_proxy_enable(&e.to_string()))?;
    key.set_value("ProxyServer", &format!("127.0.0.1:{port}"))
        .map_err(|e| i18n::t().error_set_proxy_server(&e.to_string()))?;

    // Keep a bypass list the user already curated; only seed it when empty.
    let existing: String = key.get_value("ProxyOverride").unwrap_or_default();
    if existing.trim().is_empty() {
        let mut entries: Vec<String> = extra_bypass
            .iter()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        entries.push("<local>".to_string());
        key.set_value("ProxyOverride", &entries.join(";"))
            .map_err(|e| i18n::t().error_set_proxy_override(&e.to_string()))?;
    }
    refresh();
    Ok(())
}

pub fn disable() -> Result<(), String> {
    let key = read()?;
    key.set_value("ProxyEnable", &0u32)
        .map_err(|e| i18n::t().error_set_proxy_enable(&e.to_string()))?;
    refresh();
    Ok(())
}

fn refresh() {
    unsafe {
        InternetSetOptionW(
            std::ptr::null(),
            INTERNET_OPTION_SETTINGS_CHANGED,
            std::ptr::null(),
            0,
        );
        InternetSetOptionW(
            std::ptr::null(),
            INTERNET_OPTION_REFRESH,
            std::ptr::null(),
            0,
        );
    }
}
