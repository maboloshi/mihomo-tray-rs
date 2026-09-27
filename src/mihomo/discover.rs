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

/// The name mihomo looks for under `-d` when it is not told a file.
const CONFIG_FILE_NAME: &str = "config.yaml";

fn exe_dir() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()?
        .parent()
        .map(Path::to_path_buf)
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE").map(PathBuf::from)
}

/// Candidate `mihomo.exe` locations, highest priority first.
pub fn kernel_candidates(settings: &Settings) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    if !settings.mihomo_path.is_empty() {
        out.push(PathBuf::from(&settings.mihomo_path));
    }
    if let Some(running) = proc::list_mihomo()
        .into_iter()
        .find(|p| !p.path.as_os_str().is_empty() && !proc::is_shim(&p.path))
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
    let candidates: Vec<PathBuf> = kernel_candidates(settings)
        .into_iter()
        .filter(|p| p.is_file())
        .collect();
    // A scoop shim is a launcher, not the kernel: stopping it does not stop the
    // kernel it started, so a real binary is always preferred when one exists.
    candidates
        .iter()
        .find(|path| !proc::is_shim(path))
        .cloned()
        .or_else(|| candidates.into_iter().next())
}

/// Candidate configuration files, used only to read `external-controller`.
pub fn config_candidates(settings: &Settings) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    if !settings.mihomo_config.is_empty() {
        out.push(PathBuf::from(&settings.mihomo_config));
    }
    let file_name = std::env::var("CLASH_CONFIG_FILE").unwrap_or_else(|_| CONFIG_FILE_NAME.into());
    if let Ok(dir) = std::env::var("CLASH_HOME_DIR") {
        out.push(PathBuf::from(dir).join(&file_name));
    }
    if let Some(home) = home_dir() {
        out.push(home.join(".config").join("mihomo").join(&file_name));
    }
    if let Some(dir) = exe_dir() {
        out.push(dir.join(CONFIG_FILE_NAME));
    }
    out
}

/// The kernel's command line: the configured arguments, plus where its
/// configuration lives.
///
/// This is only for the kernel this program starts itself; a running kernel is
/// restarted from its own command line, which is the only source that is right by
/// construction. Two things are added, each only while the arguments do not name
/// it themselves:
///
/// * `-d <dir>` — mihomo's directory for the configuration, its cache and its
///   geodata. It matters because an elevated kernel may run under another account,
///   where `%USERPROFILE%` — and with it mihomo's own default directory — is not
///   the one the tray just read.
/// * `-f <file>` — a file that is not called `config.yaml` has to be named, and
///   the kernel would otherwise read another one than the tray did. That covers
///   `mihomo.config` as well as a `CLASH_CONFIG_FILE` name found under
///   `CLASH_HOME_DIR` or `~/.config/mihomo`.
pub fn launch_args(settings: &Settings) -> Vec<String> {
    let explicit = (!settings.mihomo_config.is_empty())
        .then(|| PathBuf::from(&settings.mihomo_config))
        .filter(|path| path.is_file());
    let file = explicit.clone().or_else(|| {
        config_candidates(settings)
            .into_iter()
            .find(|p| p.is_file())
    });
    let mut args = settings.mihomo_args.clone();
    let Some(file) = file else {
        return args;
    };
    if !names_flag(&args, "d") {
        if let Some(dir) = file.parent() {
            args.push("-d".to_string());
            args.push(dir.display().to_string());
        }
    }
    if needs_config_file(&file, explicit.is_some()) && !names_flag(&args, "f") {
        args.push("-f".to_string());
        args.push(file.display().to_string());
    }
    args
}

/// Whether the kernel has to be told which file to read: one named `config.yaml`
/// is what it looks for under `-d` anyway, anything else is not.
fn needs_config_file(file: &Path, explicit: bool) -> bool {
    explicit
        || !file
            .file_name()
            .is_some_and(|name| name.eq_ignore_ascii_case(CONFIG_FILE_NAME))
}

/// Whether the arguments already carry a Go-style flag: `-d`, `--d`, `-d=…`,
/// `--d=…`. mihomo has no long spellings of its own.
fn names_flag(args: &[String], flag: &str) -> bool {
    let pair = [format!("-{flag}"), format!("--{flag}")];
    args.iter().any(|arg| {
        pair.iter().any(|name| arg == name)
            || pair.iter().any(|name| arg.starts_with(&format!("{name}=")))
    })
}

/// Scan a mihomo configuration file for `external-controller` and `secret`.
///
/// Top level keys only: both are settings of the controller, while a nested
/// `secret:` (a proxy provider's, say) has nothing to do with reaching it.
pub fn controller_from_config(path: &Path) -> (Option<String>, Option<String>) {
    let Ok(text) = std::fs::read_to_string(path) else {
        return (None, None);
    };
    let mut address = None;
    let mut secret = None;
    for raw in crate::settings::strip_bom(&text).lines() {
        if raw.starts_with(char::is_whitespace) {
            continue;
        }
        let line = crate::settings::strip_comment(raw).trim();
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

    // Every candidate is probed at once, and the answers are read back in the
    // order the ports are listed in: probing them one after another means a port
    // that accepts a connection and then stalls — or one that takes seconds to
    // refuse — delays the kernel this program is about to start by its whole
    // timeout, for nothing.
    let secret = env_secret.as_deref().unwrap_or("");
    let candidates: Vec<Client> = DEFAULT_PORTS
        .iter()
        .filter_map(|port| {
            Client::new(
                &format!("127.0.0.1:{port}"),
                secret,
                settings.controller_timeout_ms.min(1000),
            )
        })
        .collect();
    let answered = std::thread::scope(|scope| {
        let probes: Vec<_> = candidates
            .iter()
            .map(|client| scope.spawn(|| client.alive()))
            .collect();
        probes
            .into_iter()
            .map(|probe| probe.join().unwrap_or(false))
            .collect::<Vec<bool>>()
    });
    if let Some(client) = candidates
        .iter()
        .zip(answered)
        .find(|(_, alive)| *alive)
        .map(|(client, _)| client)
    {
        return Some(client.clone());
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
    fn a_nested_secret_is_not_the_controllers() {
        let dir = std::env::temp_dir().join("mihomo-tray-test-cfg");
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("nested.yaml");
        std::fs::write(
            &file,
            "external-controller: 127.0.0.1:9099\nsecret: \"top\"\nproxy-providers:\n  p:\n    secret: \"nested\"\n",
        )
        .unwrap();
        let (address, secret) = controller_from_config(&file);
        assert_eq!(address.as_deref(), Some("127.0.0.1:9099"));
        assert_eq!(secret.as_deref(), Some("top"));
        let _ = std::fs::remove_file(&file);
    }

    #[test]
    fn a_non_default_config_name_is_named_to_the_kernel() {
        // The kernel looks for `config.yaml` under `-d` on its own, so any other
        // name — explicit or found through `CLASH_CONFIG_FILE` — has to be given.
        assert!(needs_config_file(
            Path::new(r"C:\mihomo\custom.yaml"),
            false
        ));
        assert!(needs_config_file(Path::new(r"C:\mihomo\config.yaml"), true));
        assert!(!needs_config_file(
            Path::new(r"C:\mihomo\config.yaml"),
            false
        ));
        assert!(!needs_config_file(
            Path::new(r"C:\mihomo\CONFIG.YAML"),
            false
        ));
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

    /// A scratch directory with a configuration file of a non-default name.
    fn scratch(file_name: &str) -> (PathBuf, PathBuf) {
        let dir = std::env::temp_dir().join("mihomo-tray-test-args");
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join(file_name);
        std::fs::write(&file, "mixed-port: 7890\n").unwrap();
        (dir, file)
    }

    #[test]
    fn an_explicit_config_file_is_named_with_its_directory() {
        let (dir, file) = scratch("custom.yaml");
        let settings = Settings {
            mihomo_config: file.display().to_string(),
            mihomo_args: vec!["-ext-ctl".to_string(), "127.0.0.1:9999".to_string()],
            ..Settings::default()
        };
        assert_eq!(
            launch_args(&settings),
            vec![
                "-ext-ctl".to_string(),
                "127.0.0.1:9999".to_string(),
                "-d".to_string(),
                dir.display().to_string(),
                "-f".to_string(),
                file.display().to_string(),
            ]
        );
        let _ = std::fs::remove_file(&file);
    }

    #[test]
    fn arguments_that_already_name_the_location_are_left_alone() {
        let (_, file) = scratch("already.yaml");
        for named in [
            vec!["-d", r"C:\elsewhere"],
            vec!["--d", r"C:\elsewhere"],
            vec!["-d=C:\\elsewhere"],
            vec!["--d=C:\\elsewhere"],
        ] {
            let mut args: Vec<String> = named.iter().map(|arg| arg.to_string()).collect();
            args.extend(["-f".to_string(), r"C:\elsewhere\config.yaml".to_string()]);
            let settings = Settings {
                mihomo_config: file.display().to_string(),
                mihomo_args: args.clone(),
                ..Settings::default()
            };
            assert_eq!(launch_args(&settings), args, "for {named:?}");
        }
        let _ = std::fs::remove_file(&file);
    }
}
