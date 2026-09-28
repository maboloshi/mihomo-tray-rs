//! Where the kernel is, where its configuration is, and what that configuration
//! says about the external controller.
//!
//! Nothing here searches. The kernel binary is what `mihomo.path` names, the
//! configuration file is what `mihomo.config` or `mihomo.home` names (or mihomo's
//! own default when neither does), and the controller is what that file says.
//! Guessing is what once made this program talk to a kernel it was not looking
//! at, so a declaration that is missing or unusable is reported instead.

use std::path::{Path, PathBuf};

use super::api::Client;
use crate::i18n;
use crate::paths;
use crate::settings::Settings;

const DEFAULT_PORTS: [u16; 5] = [9090, 9091, 9097, 9098, 6170];

/// The name mihomo looks for under `-d` when it is not told a file.
const CONFIG_FILE_NAME: &str = "config.yaml";

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE").map(PathBuf::from)
}

/// mihomo's own configuration directory, mirrored exactly: `%USERPROFILE%\.config
/// \mihomo`, and `XDG_CONFIG_HOME` instead only when that directory does not
/// exist — mihomo tests the same condition, so the two agree on a machine that
/// sets the variable.
fn default_home() -> Option<PathBuf> {
    let home = home_dir()?.join(".config").join("mihomo");
    if home.exists() {
        return Some(home);
    }
    match std::env::var("XDG_CONFIG_HOME") {
        Ok(xdg) if !xdg.is_empty() => Some(PathBuf::from(xdg).join("mihomo")),
        _ => Some(home),
    }
}

/// The kernel to start: what `mihomo.path` names.
///
/// `Ok(None)` is "not configured" — an empty setting, which the caller reports as
/// such — while an error is a path that is configured and unusable. Both are
/// answered rather than guessed at: a search would start whichever `mihomo.exe`
/// it happened to find first.
pub fn find_kernel(settings: &Settings) -> Result<Option<PathBuf>, String> {
    if settings.mihomo_path.trim().is_empty() {
        return Ok(None);
    }
    paths::file("mihomo.path", &settings.mihomo_path).map(Some)
}

/// The configuration file `mihomo.config` declares, if it declares one.
fn declared_config(settings: &Settings) -> Result<Option<PathBuf>, String> {
    if settings.mihomo_config.trim().is_empty() {
        return Ok(None);
    }
    paths::absolute("mihomo.config", &settings.mihomo_config).map(Some)
}

/// The kernel's configuration directory: `mihomo.home`, else the directory of the
/// file `mihomo.config` names, else mihomo's own default.
///
/// `Ok(None)` means "not determinable" (`%USERPROFILE%` is not set), which leaves
/// the kernel to its own default rather than inventing one.
fn kernel_home(settings: &Settings) -> Result<Option<PathBuf>, String> {
    if !settings.mihomo_home.trim().is_empty() {
        return paths::absolute("mihomo.home", &settings.mihomo_home).map(Some);
    }
    if let Some(config) = declared_config(settings)? {
        return Ok(config.parent().map(Path::to_path_buf));
    }
    Ok(default_home())
}

/// The kernel's configuration file: `mihomo.config`, else `config.yaml` under
/// [`kernel_home`] — the rule mihomo itself applies to an empty `-f`.
fn kernel_config(settings: &Settings) -> Result<Option<PathBuf>, String> {
    match declared_config(settings)? {
        Some(config) => Ok(Some(config)),
        None => Ok(kernel_home(settings)?.map(|home| home.join(CONFIG_FILE_NAME))),
    }
}

/// The kernel's command line: what `tray.yml` declares, plus where its
/// configuration lives.
///
/// `-d` and `-f` are always built from the resolved absolute paths, because
/// mihomo resolves a relative one against the kernel's own working directory —
/// which for a kernel this program starts is inherited from the tray rather than
/// chosen by anyone. Pinning `-d` also matters for a kernel that ends up running
/// under another account (the elevated TUN replacement), where mihomo's own
/// default directory would be that account's rather than the one read here.
///
/// Everything else in `mihomo.args` is passed through as written, with `%NAME%`
/// expanded the way cmd would. An argument that restates one of the settings
/// owned by `tray.yml` is refused: the kernel would obey the argument while this
/// program read the setting, and the two would disagree without saying so.
pub fn launch_args(settings: &Settings) -> Result<Vec<String>, String> {
    for (flag, field) in [
        ("d", "mihomo.home"),
        ("f", "mihomo.config"),
        ("ext-ctl", "controller.address"),
        ("secret", "controller.secret"),
    ] {
        if names_flag(&settings.mihomo_args, flag) {
            return Err(i18n::t().error_flag_in_args(&format!("-{flag}"), field));
        }
    }
    let mut args: Vec<String> = settings
        .mihomo_args
        .iter()
        .map(|arg| paths::expand(arg))
        .collect();
    if let Some(home) = kernel_home(settings)? {
        args.push("-d".to_string());
        args.push(home.display().to_string());
    }
    if let Some(config) = kernel_config(settings)? {
        args.push("-f".to_string());
        args.push(config.display().to_string());
    }
    Ok(args)
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

    // The one configuration file `tray.yml` declares, or mihomo's own default: the
    // only file this program reads a controller address from.
    let mut from_file: Vec<(String, String)> = Vec::new();
    if let Ok(Some(path)) = kernel_config(settings) {
        if path.is_file() {
            let (address, secret) = controller_from_config(&path);
            if let Some(address) = address {
                from_file.push((
                    address,
                    secret.or_else(|| env_secret.clone()).unwrap_or_default(),
                ));
            }
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
    fn the_kernel_is_the_declared_one_and_nothing_else() {
        // Nothing declared is "not configured", not a search.
        assert!(find_kernel(&Settings::default()).unwrap().is_none());

        // A declared path that is not there is reported, not skipped: skipping is
        // what made a tray look like it could not find a kernel it was told about.
        let settings = Settings {
            mihomo_path: r"Z:\definitely\missing\mihomo.exe".into(),
            ..Settings::default()
        };
        let error = find_kernel(&settings).unwrap_err();
        assert!(error.contains("mihomo.path"), "{error}");

        // A relative path is refused before anything is opened.
        let settings = Settings {
            mihomo_path: r"scoop\apps\mihomo.exe".into(),
            ..Settings::default()
        };
        assert!(find_kernel(&settings).unwrap_err().contains("mihomo.path"));
    }

    #[test]
    fn a_declared_config_file_is_named_with_its_directory() {
        let (dir, file) = scratch("custom.yaml");
        let settings = Settings {
            mihomo_config: file.display().to_string(),
            mihomo_args: vec!["-m".to_string()],
            ..Settings::default()
        };
        assert_eq!(
            launch_args(&settings).unwrap(),
            vec![
                "-m".to_string(),
                "-d".to_string(),
                dir.display().to_string(),
                "-f".to_string(),
                file.display().to_string(),
            ]
        );
        let _ = std::fs::remove_file(&file);
    }

    #[test]
    fn a_declared_home_is_used_as_it_stands() {
        let (dir, file) = scratch("config.yaml");
        let settings = Settings {
            mihomo_home: dir.display().to_string(),
            ..Settings::default()
        };
        assert_eq!(
            launch_args(&settings).unwrap(),
            vec![
                "-d".to_string(),
                dir.display().to_string(),
                "-f".to_string(),
                file.display().to_string(),
            ]
        );
        let _ = std::fs::remove_file(&file);
    }

    #[test]
    fn arguments_may_not_restate_a_setting_we_own() {
        // Two spellings of one setting is how the kernel and this program end up
        // reading different files, so the argument is refused by name.
        let (_, file) = scratch("owned.yaml");
        for (flag, field) in [
            ("-d", "mihomo.home"),
            ("--d=X", "mihomo.home"),
            ("-f", "mihomo.config"),
            ("-f=Y", "mihomo.config"),
            ("-ext-ctl", "controller.address"),
            ("-secret", "controller.secret"),
        ] {
            let settings = Settings {
                mihomo_config: file.display().to_string(),
                mihomo_args: vec![flag.to_string()],
                ..Settings::default()
            };
            let error = launch_args(&settings).unwrap_err();
            // The report names the field to use instead, which is the useful half.
            assert!(error.contains(field), "{flag}: {error}");
        }
        // An argument that belongs to the kernel alone is passed through.
        let settings = Settings {
            mihomo_config: file.display().to_string(),
            mihomo_args: vec!["-ext-ui=C:\\ui".to_string()],
            ..Settings::default()
        };
        assert_eq!(launch_args(&settings).unwrap()[0], "-ext-ui=C:\\ui");
        let _ = std::fs::remove_file(&file);
    }
}
