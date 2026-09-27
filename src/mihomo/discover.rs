//! Locating the kernel executable and the external controller.
//!
//! Three different things have three different sources, so they are resolved
//! separately: the `mihomo.exe` path, the configuration file, and the controller
//! address. Nothing here assumes the config file is where the running core reads
//! it from — probing is the last resort precisely because the address is often
//! injected through `-ext-ctl` or an environment variable.

use std::path::{Path, PathBuf};

use super::api::Client;
use super::proc;
use crate::settings::Settings;

const DEFAULT_PORTS: [u16; 5] = [9090, 9091, 9097, 9098, 6170];

fn exe_dir() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()?
        .parent()
        .map(Path::to_path_buf)
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE").map(PathBuf::from)
}

fn is_shim(path: &Path) -> bool {
    path.to_string_lossy()
        .to_ascii_lowercase()
        .contains(r"scoop\shims")
}

/// Candidate `mihomo.exe` locations, highest priority first.
pub fn kernel_candidates(settings: &Settings) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    if !settings.mihomo_path.is_empty() {
        out.push(PathBuf::from(&settings.mihomo_path));
    }
    if let Some(running) = proc::list_mihomo()
        .into_iter()
        .find(|p| !is_shim(&p.path))
        .map(|p| p.path)
    {
        out.push(running);
    }
    if let Some(dir) = exe_dir() {
        out.push(dir.join("mihomo.exe"));
        out.push(dir.join("bin").join("mihomo.exe"));
        out.push(dir.join("core").join("mihomo.exe"));
    }
    if let Some(path) = std::env::var_os("PATH") {
        for entry in std::env::split_paths(&path) {
            out.push(entry.join("mihomo.exe"));
        }
    }
    if let Some(scoop) = std::env::var_os("SCOOP") {
        let root = PathBuf::from(scoop);
        out.push(root.join("shims").join("mihomo.exe"));
        if let Ok(apps) = std::fs::read_dir(root.join("apps")) {
            for app in apps.flatten() {
                if app
                    .file_name()
                    .to_string_lossy()
                    .to_ascii_lowercase()
                    .starts_with("mihomo")
                {
                    out.push(app.path().join("current").join("mihomo.exe"));
                }
            }
        }
    }
    if let Some(home) = home_dir() {
        out.push(home.join("scoop").join("shims").join("mihomo.exe"));
    }
    out
}

pub fn find_kernel(settings: &Settings) -> Option<PathBuf> {
    kernel_candidates(settings)
        .into_iter()
        .find(|p| p.is_file())
}

/// Candidate configuration files, used only to read `external-controller`.
pub fn config_candidates(settings: &Settings) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    if !settings.mihomo_config.is_empty() {
        out.push(PathBuf::from(&settings.mihomo_config));
    }
    let file_name = std::env::var("CLASH_CONFIG_FILE").unwrap_or_else(|_| "config.yaml".into());
    if let Ok(dir) = std::env::var("CLASH_HOME_DIR") {
        out.push(PathBuf::from(dir).join(&file_name));
    }
    if let Some(home) = home_dir() {
        out.push(home.join(".config").join("mihomo").join(&file_name));
    }
    if let Some(dir) = exe_dir() {
        out.push(dir.join("config.yaml"));
    }
    out
}

/// Scan a mihomo configuration file for `external-controller` and `secret`.
pub fn controller_from_config(path: &Path) -> (Option<String>, Option<String>) {
    let Ok(text) = std::fs::read_to_string(path) else {
        return (None, None);
    };
    let mut address = None;
    let mut secret = None;
    for line in text.lines() {
        let line = crate::settings::strip_comment(line).trim();
        if let Some((key, value)) = line.split_once(':') {
            let value = value.trim().trim_matches(|c| c == '"' || c == '\'');
            match key.trim() {
                "external-controller" if !value.is_empty() => address = Some(value.to_string()),
                "secret" if !value.is_empty() => secret = Some(value.to_string()),
                _ => {}
            }
        }
    }
    (address, secret)
}

/// Resolve the controller client: explicit setting, environment override,
/// configuration file, then a port probe.
pub fn find_controller(settings: &Settings) -> Option<Client> {
    if !settings.controller_address.is_empty() {
        return Client::new(
            &settings.controller_address,
            &settings.controller_secret,
            settings.controller_timeout_ms,
        );
    }

    let env_address = std::env::var("CLASH_OVERRIDE_EXTERNAL_CONTROLLER").ok();
    let env_secret = std::env::var("CLASH_OVERRIDE_SECRET").ok();

    if let Some(address) = env_address.as_deref().filter(|a| !a.is_empty()) {
        if let Some(client) = Client::new(
            address,
            env_secret.as_deref().unwrap_or(""),
            settings.controller_timeout_ms,
        ) {
            return Some(client);
        }
    }

    let mut from_file: Vec<(String, String)> = Vec::new();
    for path in config_candidates(settings) {
        if !path.is_file() {
            continue;
        }
        let (address, secret) = controller_from_config(&path);
        if let Some(address) = address {
            from_file.push((
                address,
                secret.or_else(|| env_secret.clone()).unwrap_or_default(),
            ));
        }
    }
    for (address, secret) in &from_file {
        if let Some(client) = Client::new(address, secret, settings.controller_timeout_ms) {
            if client.alive() {
                return Some(client);
            }
        }
    }

    for port in DEFAULT_PORTS {
        if let Some(client) = Client::new(
            &format!("127.0.0.1:{port}"),
            env_secret.as_deref().unwrap_or(""),
            settings.controller_timeout_ms.min(1000),
        ) {
            if client.alive() {
                return Some(client);
            }
        }
    }

    // Nothing answered: fall back to the first configured candidate so the UI can
    // report a controller error instead of silently doing nothing.
    from_file
        .into_iter()
        .next()
        .and_then(|(address, secret)| {
            Client::new(&address, &secret, settings.controller_timeout_ms)
        })
        .or_else(|| Client::new("127.0.0.1:9090", "", settings.controller_timeout_ms))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_controller_from_a_config_file() {
        let dir = std::env::temp_dir().join("mihomo-tray-test-cfg");
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("config.yaml");
        std::fs::write(
            &file,
            "mixed-port: 7890\nexternal-controller: 127.0.0.1:9099 # comment\nsecret: \"tok\"\ntun:\n  enable: true\n",
        )
        .unwrap();
        let (address, secret) = controller_from_config(&file);
        assert_eq!(address.as_deref(), Some("127.0.0.1:9099"));
        assert_eq!(secret.as_deref(), Some("tok"));
        let _ = std::fs::remove_file(&file);
    }

    #[test]
    fn explicit_setting_wins() {
        let settings = Settings {
            controller_address: "127.0.0.1:1234".into(),
            ..Settings::default()
        };
        let client = find_controller(&settings).unwrap();
        assert_eq!(client.address(), "127.0.0.1:1234");
    }
}
