//! Where the kernel is, where its configuration is, and what that configuration
//! says about the external controller.
//!
//! The kernel binary is what `mihomo.path` names and otherwise the first hit of
//! the discovery chain: this executable's own directory — `mihomo.exe` beside the
//! tray, or in `bin\`/`core\`, which is how a bundled release is packed — then
//! `PATH`, then scoop. mihomo itself searches for nothing: it is told `-d`/`-f` or
//! falls back to its own directory, so a binary found here is always launched with
//! absolute `-d` and `-f`, which is what keeps the kernel and this program reading
//! the same files.
//!
//! A declared `mihomo.path` is answered as it stands: a path that is set and
//! unusable is reported, never quietly replaced by whichever `mihomo.exe` the chain
//! would have reached instead. The configuration file is what `mihomo.config` or
//! `mihomo.home` names, else the one the kernel brings along — beside the tray that
//! bundles it, else beside the kernel itself — else mihomo's own default. The
//! controller is what the kernel itself says: its command line, its environment and
//! that file, in mihomo's own order, with `tray.yml` only as the last resort.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use super::api::{Client, ClientError};
use super::proc;
use crate::i18n;
use crate::paths;
use crate::settings::Settings;

/// The name mihomo looks for under `-d` when it is not told a file.
const CONFIG_FILE_NAME: &str = "config.yaml";

/// A controller address and secret, as one source states them. `None` is "this
/// source says nothing about it", which is what keeps an empty value from
/// overwriting the next source in the chain — mihomo's own rule for `-ext-ctl`
/// and `-secret`.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct ControllerSettings {
    address: Option<String>,
    secret: Option<String>,
}

/// What a mihomo configuration file says about its controller.
#[derive(Debug, Default, PartialEq, Eq)]
struct ControllerFile {
    settings: ControllerSettings,
    /// The file names a controller of another family — `external-controller-tls`,
    /// `-unix` or `-pipe` — which is why a kernel that serves a controller may
    /// still have none this program can reach.
    other_family: bool,
}

/// A resolved controller, before it becomes a client.
#[derive(Debug, PartialEq, Eq)]
struct Controller {
    address: String,
    secret: String,
}

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

/// The kernel this program would start, and how it was named.
///
/// The two are told apart because the status line says which binary the chain
/// picked: a declaration is the user's own doing and needs no announcement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kernel {
    /// `tray.yml mihomo.path` named it.
    Declared(PathBuf),
    /// The discovery chain found it.
    Discovered(PathBuf),
}

impl Kernel {
    pub fn path(&self) -> &Path {
        match self {
            Kernel::Declared(path) | Kernel::Discovered(path) => path,
        }
    }
}

/// The kernel to start: what `mihomo.path` names, else the first hit of the
/// discovery chain.
///
/// `Ok(None)` is "the chain found none", which the caller reports as not found —
/// while an error is a declaration that is configured and unusable. The two are
/// not interchangeable: a declared path that is missing is a mistake to fix, not a
/// reason to start whichever `mihomo.exe` happens to be around.
pub fn find_kernel(settings: &Settings) -> Result<Option<Kernel>, String> {
    find_kernel_among(settings, &kernel_candidates())
}

/// The rule above, with the chain handed in — the caller produces it, so a test
/// does not depend on where this machine keeps its kernels.
fn find_kernel_among(
    settings: &Settings,
    candidates: &[PathBuf],
) -> Result<Option<Kernel>, String> {
    if !settings.mihomo_path.trim().is_empty() {
        return paths::file("mihomo.path", &settings.mihomo_path)
            .map(|path| Some(Kernel::Declared(path)));
    }
    Ok(found_kernel(candidates))
}

/// The first candidate that is a file, preferring a real binary over a scoop shim:
/// the shim carries the kernel's name but is a launcher, so a kernel started from
/// it reports an image the shim does not have.
fn found_kernel(candidates: &[PathBuf]) -> Option<Kernel> {
    let files: Vec<&PathBuf> = candidates.iter().filter(|path| path.is_file()).collect();
    files
        .iter()
        .find(|path| !proc::is_shim(path))
        .or_else(|| files.first())
        .map(|path| Kernel::Discovered((*path).clone()))
}

/// This executable's own directory: where a bundled kernel lives.
fn exe_dir() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()?
        .parent()
        .map(Path::to_path_buf)
}

/// The candidate kernels, highest priority first: beside the tray (or in `bin\`,
/// `core\`), then every `PATH` entry, then scoop.
///
/// Nothing here looks at a running process: a kernel this program did not start is
/// not silently adopted. "Force restart" is the one action that replaces such a
/// kernel, and it asks first.
fn kernel_candidates() -> Vec<PathBuf> {
    kernel_candidates_for(
        exe_dir().as_deref(),
        std::env::var_os("PATH").as_deref(),
        std::env::var_os("SCOOP").map(PathBuf::from).as_deref(),
        home_dir().as_deref(),
    )
}

/// The chain itself, with every place handed in. `PATH` is taken as the raw
/// variable so a test can describe one without touching this machine's.
fn kernel_candidates_for(
    exe_dir: Option<&Path>,
    path_var: Option<&OsStr>,
    scoop: Option<&Path>,
    home: Option<&Path>,
) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(dir) = exe_dir {
        out.push(dir.join(proc::KERNEL_EXE));
        out.push(dir.join("bin").join(proc::KERNEL_EXE));
        out.push(dir.join("core").join(proc::KERNEL_EXE));
    }
    if let Some(path_var) = path_var {
        for entry in std::env::split_paths(path_var) {
            out.push(entry.join(proc::KERNEL_EXE));
        }
    }
    if let Some(scoop) = scoop {
        scoop_candidates(scoop, &mut out);
        out.push(scoop.join("shims").join(proc::KERNEL_EXE));
    }
    if let Some(home) = home {
        out.push(home.join("scoop").join("shims").join(proc::KERNEL_EXE));
    }
    out
}

/// `%SCOOP%\apps\mihomo*\current\mihomo.exe`: scoop keeps every version in a
/// directory of its own and points `current` at the installed one. Sorted, so two
/// installs of the same name do not make the answer depend on the file system.
fn scoop_candidates(scoop: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(scoop.join("apps")) else {
        return;
    };
    let mut found: Vec<PathBuf> = entries
        .flatten()
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .to_ascii_lowercase()
                .starts_with("mihomo")
        })
        .map(|entry| entry.path().join("current").join(proc::KERNEL_EXE))
        .collect();
    found.sort();
    out.extend(found);
}

/// The configuration file `mihomo.config` declares, if it declares one.
fn declared_config(settings: &Settings) -> Result<Option<PathBuf>, String> {
    if settings.mihomo_config.trim().is_empty() {
        return Ok(None);
    }
    paths::absolute("mihomo.config", &settings.mihomo_config).map(Some)
}

/// The configuration directory `mihomo.home` declares, if it declares one.
fn declared_home(settings: &Settings) -> Result<Option<PathBuf>, String> {
    if settings.mihomo_home.trim().is_empty() {
        return Ok(None);
    }
    paths::absolute("mihomo.home", &settings.mihomo_home).map(Some)
}

/// The kernel's configuration file: `mihomo.config`, else `config.yaml` under
/// `mihomo.home`, else the file the kernel brings along, else `config.yaml` under
/// mihomo's own default directory — the rule mihomo itself applies to an empty
/// `-f`.
fn kernel_config(settings: &Settings, kernel: Option<&Path>) -> Result<Option<PathBuf>, String> {
    kernel_config_with(settings, kernel, exe_dir().as_deref(), default_home())
}

/// The rule itself, with every place handed in: a test can describe a bundle, a
/// kernel beside it and a home directory without touching this machine's.
fn kernel_config_with(
    settings: &Settings,
    kernel: Option<&Path>,
    exe_dir: Option<&Path>,
    home: Option<PathBuf>,
) -> Result<Option<PathBuf>, String> {
    if let Some(config) = declared_config(settings)? {
        return Ok(Some(config));
    }
    if let Some(home) = declared_home(settings)? {
        return Ok(Some(home.join(CONFIG_FILE_NAME)));
    }
    if let Some(config) = bundled_config(kernel, exe_dir) {
        return Ok(Some(config));
    }
    Ok(home.map(|home| home.join(CONFIG_FILE_NAME)))
}

/// The kernel's configuration directory: `mihomo.home`, else the directory of the
/// file its configuration came from.
///
/// `Ok(None)` means "not determinable" (`%USERPROFILE%` is not set), which leaves
/// the kernel to its own default rather than inventing one.
fn kernel_home(settings: &Settings, kernel: Option<&Path>) -> Result<Option<PathBuf>, String> {
    if let Some(home) = declared_home(settings)? {
        return Ok(Some(home));
    }
    Ok(kernel_config(settings, kernel)?.and_then(|config| config.parent().map(Path::to_path_buf)))
}

/// The configuration file a kernel brings along: `<root>\config.yaml` when the
/// kernel sits in this executable's own directory tree — the root of a bundle that
/// is `mihomo-tray.exe` with `mihomo.exe` beside it, or in `bin\`/`core\` — and
/// otherwise `config.yaml` beside the kernel itself.
///
/// Only a file that is really there counts: this is a candidate, not a claim, and
/// mihomo's own default stays the answer when no such file exists.
fn bundled_config(kernel: Option<&Path>, exe_dir: Option<&Path>) -> Option<PathBuf> {
    let dir = kernel?.parent()?;
    if let Some(root) = exe_dir {
        if is_beside(dir, root) {
            let config = root.join(CONFIG_FILE_NAME);
            if config.is_file() {
                return Some(config);
            }
        }
    }
    let config = dir.join(CONFIG_FILE_NAME);
    config.is_file().then_some(config)
}

/// Whether `dir` is `root` itself or one directory below it — the layouts a bundle
/// uses. The comparison goes through [`proc::same_image`], so a junction, a
/// different case and a short name all count as the same directory.
fn is_beside(dir: &Path, root: &Path) -> bool {
    proc::same_image(dir, root)
        || dir
            .parent()
            .is_some_and(|parent| proc::same_image(parent, root))
}

/// The kernel's command line: what `tray.yml` declares, plus where its
/// configuration lives. `kernel` is the binary this program would start, which the
/// configuration candidates are resolved against — the same one the caller records
/// for "exit and stop mihomo", so the two cannot disagree.
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
pub fn launch_args(settings: &Settings, kernel: Option<&Path>) -> Result<Vec<String>, String> {
    launch_args_with(settings, kernel, override_environment())
}

/// The command line itself, with the controller environment handed in — the
/// caller reads it, so a test does not depend on this machine's variables.
fn launch_args_with(
    settings: &Settings,
    kernel: Option<&Path>,
    env: ControllerSettings,
) -> Result<Vec<String>, String> {
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
    if let Some(home) = kernel_home(settings, kernel)? {
        args.push("-d".to_string());
        args.push(home.display().to_string());
    }
    if let Some(config) = kernel_config(settings, kernel)? {
        args.push("-f".to_string());
        args.push(config.display().to_string());
    }
    args.extend(override_args(&env));
    Ok(args)
}

/// The `CLASH_OVERRIDE_*` variables written out as kernel arguments: `-ext-ctl`
/// and `-secret`.
///
/// Those variables are the defaults of exactly these flags, so writing them out
/// is the same instruction to the kernel — but a written-out value is the one that
/// survives. The elevated helper restarts the kernel from its old command line,
/// while nothing that goes through UAC inherits this program's environment
/// (measured: the helper sees neither `CLASH_HOME_DIR` nor `CLASH_OVERRIDE_*`).
/// Deciding the value here is what keeps a TUN-elevated kernel on the controller
/// this program resolved.
fn override_args(env: &ControllerSettings) -> Vec<String> {
    let mut args = Vec::new();
    if let Some(address) = &env.address {
        args.push("-ext-ctl".to_string());
        args.push(address.clone());
    }
    if let Some(secret) = &env.secret {
        args.push("-secret".to_string());
        args.push(secret.clone());
    }
    args
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
fn controller_from_config(path: &Path) -> std::io::Result<ControllerFile> {
    let text = std::fs::read_to_string(path)?;
    let mut file = ControllerFile::default();
    for raw in crate::settings::strip_bom(&text).lines() {
        if raw.starts_with(char::is_whitespace) {
            continue;
        }
        let line = crate::settings::strip_comment(raw).trim();
        if let Some((key, value)) = line.split_once(':') {
            let value = value.trim().trim_matches(|c| c == '"' || c == '\'');
            if value.is_empty() {
                continue;
            }
            match key.trim() {
                "external-controller" => file.settings.address = Some(value.to_string()),
                "secret" => file.settings.secret = Some(value.to_string()),
                "external-controller-tls"
                | "external-controller-unix"
                | "external-controller-pipe" => file.other_family = true,
                _ => {}
            }
        }
    }
    Ok(file)
}

/// The controller this program talks to, resolved the way mihomo resolves it.
///
/// mihomo's own precedence, replayed rather than reinvented: `-ext-ctl` and
/// `-secret` — whose flag defaults are the `CLASH_OVERRIDE_*` environment
/// variables — win when they are non-empty, and the configuration file decides
/// otherwise, because mihomo only applies the option when it is not empty. The
/// `controller` section of `tray.yml` has the last word, and only where mihomo has
/// none: a kernel with no controller of this family, or a configuration that
/// cannot be read at all (`-config <base64>`, an age-encrypted file, a file that
/// is not there).
///
/// The command line read here belongs to the running kernel, never to this
/// program's own arguments — those are refused when they restate one of these
/// settings, so the two cannot disagree. The environment is this process's, which
/// is what a kernel started from here inherits; a kernel started elsewhere may
/// have been given another one, and that is the one case this cannot see.
pub fn find_controller(settings: &Settings, kernel: Option<&Path>) -> Result<Client, String> {
    find_controller_with(settings, kernel, override_environment())
}

/// The resolution itself, with the controller environment handed in — the caller
/// reads it, so a test does not depend on this machine's variables.
fn find_controller_with(
    settings: &Settings,
    kernel: Option<&Path>,
    from_env: ControllerSettings,
) -> Result<Client, String> {
    let config = kernel_config(settings, kernel)?;
    let args = kernel_arguments(config.as_deref());
    let file = config.as_deref().map(controller_from_config);

    let from_argv = ControllerSettings {
        address: flag_value(args.as_deref().unwrap_or_default(), "ext-ctl"),
        secret: flag_value(args.as_deref().unwrap_or_default(), "secret"),
    };
    let from_file = match &file {
        Some(Ok(file)) => file.settings.clone(),
        _ => ControllerSettings::default(),
    };
    let declared = ControllerSettings {
        address: non_empty(&settings.controller_address),
        secret: non_empty(&settings.controller_secret),
    };

    let Some(controller) = resolve_controller(from_argv, from_env, from_file, declared) else {
        return Err(unresolved(config.as_deref(), file.as_ref()));
    };
    Client::new(
        &controller.address,
        &controller.secret,
        settings.controller_timeout_ms,
    )
    .map_err(|error| match error {
        // Two different things to fix, so they get two different messages: a typo,
        // or a scheme this program does not speak.
        ClientError::TlsUnsupported => i18n::t().error_controller_tls(&controller.address),
        ClientError::NotAnAddress => i18n::t().error_controller_invalid(&controller.address),
    })
}

/// The chain itself, with every source handed in. mihomo's precedence first, and
/// `tray.yml` only where mihomo says nothing — per setting, so an address from one
/// source and a secret from another stay possible, which is what "the kernel sets
/// no secret" means.
fn resolve_controller(
    argv: ControllerSettings,
    env: ControllerSettings,
    file: ControllerSettings,
    declared: ControllerSettings,
) -> Option<Controller> {
    let address = argv
        .address
        .or(env.address)
        .or(file.address)
        .or(declared.address)?;
    let secret = argv
        .secret
        .or(env.secret)
        .or(file.secret)
        .or(declared.secret)
        .unwrap_or_default();
    Some(Controller { address, secret })
}

/// Why no controller could be resolved, named as precisely as the sources allow.
fn unresolved(config: Option<&Path>, file: Option<&std::io::Result<ControllerFile>>) -> String {
    let messages = i18n::t();
    let Some(path) = config else {
        return messages.error_controller_unset.to_string();
    };
    let path = path.display().to_string();
    match file {
        Some(Err(error)) => messages.error_config_unreadable(&path, &error.to_string()),
        Some(Ok(file)) if file.other_family => messages.error_controller_other_family(&path),
        _ => messages.error_controller_missing(&path),
    }
}

/// A `CLASH_OVERRIDE_*` variable, as mihomo reads it: an empty one is not a value.
fn env_setting(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
}

/// The `CLASH_OVERRIDE_*` variables: mihomo makes them the defaults of `-ext-ctl`
/// and `-secret`, which is why they belong to the same slot in the chain rather
/// than to a source of their own.
fn override_environment() -> ControllerSettings {
    ControllerSettings {
        address: env_setting("CLASH_OVERRIDE_EXTERNAL_CONTROLLER"),
        secret: env_setting("CLASH_OVERRIDE_SECRET"),
    }
}

/// A setting from `tray.yml`, where empty means "not set" rather than "the empty
/// string".
fn non_empty(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_string())
}

/// The command line of the kernel this resolution is about.
///
/// The kernel that was told to read the configuration file this program reads is
/// the one being talked about; a lone `mihomo.exe` leaves nothing to choose
/// between; anything else has no answer here, because reading a stranger's command
/// line could as well report another kernel's controller.
fn kernel_arguments(wanted: Option<&Path>) -> Option<Vec<String>> {
    let processes = proc::list_mihomo();
    kernel_arguments_from(&processes, wanted, proc::command_line)
}

/// The rule above, over a process list with the reader handed in, so a test can
/// describe kernels without starting any.
fn kernel_arguments_from(
    processes: &[proc::Process],
    wanted: Option<&Path>,
    args_of: fn(u32) -> Option<Vec<String>>,
) -> Option<Vec<String>> {
    let readable = |process: &proc::Process| -> Option<Vec<String>> {
        // A kernel of higher integrity is neither ours to read nor to describe.
        if process.denied {
            return None;
        }
        args_of(process.pid)
    };
    if let Some(wanted) = wanted {
        for process in processes {
            if let Some(args) = readable(process) {
                if argv_config(&args).is_some_and(|path| same_file(&path, wanted)) {
                    return Some(args);
                }
            }
        }
    }
    match processes {
        [only] => readable(only),
        _ => None,
    }
}

/// The configuration file a kernel's own command line names, resolved the way
/// mihomo resolves it: `-f` when given, else `config.yaml` under `-d` or under the
/// default directory.
///
/// `None` when that cannot be known — a relative path, which mihomo resolves
/// against the kernel's own working directory, and this program cannot read
/// another process's working directory.
fn argv_config(args: &[String]) -> Option<PathBuf> {
    if let Some(file) = flag_value(args, "f") {
        let path = PathBuf::from(file);
        return path.is_absolute().then_some(path);
    }
    match flag_value(args, "d") {
        Some(dir) => {
            let dir = PathBuf::from(dir);
            dir.is_absolute().then(|| dir.join(CONFIG_FILE_NAME))
        }
        None => default_home().map(|home| home.join(CONFIG_FILE_NAME)),
    }
}

/// A Go-style flag value out of an argument list: `-f x`, `-f=x`, `--f=x` — the
/// spellings mihomo's own parser accepts, with the last occurrence winning, which
/// is also what its parser does.
fn flag_value(args: &[String], flag: &str) -> Option<String> {
    let short = format!("-{flag}");
    let long = format!("--{flag}");
    let mut found = None;
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        if arg == &short || arg == &long {
            found = args
                .get(index + 1)
                .cloned()
                .filter(|value| !value.is_empty());
            index += 2;
            continue;
        }
        if let Some(value) = arg
            .strip_prefix(&format!("{short}="))
            .or_else(|| arg.strip_prefix(&format!("{long}=")))
        {
            found = (!value.is_empty()).then(|| value.to_string());
        }
        index += 1;
    }
    found
}

/// Whether two paths name the same file, compared the way Windows compares them.
/// Neither is resolved: the file is allowed not to exist yet.
fn same_file(a: &Path, b: &Path) -> bool {
    a.to_string_lossy()
        .eq_ignore_ascii_case(&b.to_string_lossy())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scratch configuration file holding `text`.
    ///
    /// The name carries a counter: tests run in parallel threads, and two of them
    /// writing one path would let a reader see a truncated file — `fs::write`
    /// truncates first — which is a test artefact, not something to debug twice.
    fn config_file(name: &str, text: &str) -> PathBuf {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join("mihomo-tray-test-cfg");
        std::fs::create_dir_all(&dir).unwrap();
        let serial = NEXT.fetch_add(1, Ordering::Relaxed);
        let file = dir.join(format!("{serial}-{name}"));
        std::fs::write(&file, text).unwrap();
        file
    }

    /// The settings one source states, for the chain test below.
    fn source(address: Option<&str>, secret: Option<&str>) -> ControllerSettings {
        ControllerSettings {
            address: address.map(str::to_string),
            secret: secret.map(str::to_string),
        }
    }

    #[test]
    fn reads_controller_settings_from_a_config_file() {
        let file = config_file(
            "controller.yaml",
            "mixed-port: 7890\nexternal-controller: 127.0.0.1:9099 # comment\nsecret: \"tok\"\ntun:\n  enable: true\n",
        );
        let file = controller_from_config(&file).unwrap();
        assert_eq!(file.settings.address.as_deref(), Some("127.0.0.1:9099"));
        assert_eq!(file.settings.secret.as_deref(), Some("tok"));
        assert!(!file.other_family);
    }

    #[test]
    fn a_nested_secret_is_not_the_controllers() {
        let path = config_file(
            "nested.yaml",
            "external-controller: 127.0.0.1:9099\nsecret: \"top\"\nproxy-providers:\n  p:\n    secret: \"nested\"\n",
        );
        let file = controller_from_config(&path).unwrap();
        assert_eq!(file.settings.address.as_deref(), Some("127.0.0.1:9099"));
        assert_eq!(file.settings.secret.as_deref(), Some("top"));
    }

    #[test]
    fn a_controller_of_another_family_is_noted() {
        // It is why a kernel with a controller can still have none this program
        // reaches, which is a different report from "no controller at all".
        let path = config_file(
            "only-pipe.yaml",
            "external-controller-pipe: \\\\.\\pipe\\mihomo\n",
        );
        let file = controller_from_config(&path).unwrap();
        assert!(file.other_family);
        assert!(file.settings.address.is_none());
    }

    #[test]
    fn a_config_file_that_cannot_be_read_is_an_error() {
        let missing = std::env::temp_dir()
            .join("mihomo-tray-test-cfg")
            .join("not-there.yaml");
        assert!(controller_from_config(&missing).is_err());
    }

    #[test]
    fn the_chain_is_mihomos_and_tray_yml_comes_last() {
        let argv = source(Some("127.0.0.1:2222"), Some("argv"));
        let env = source(Some("127.0.0.1:3333"), Some("env"));
        let file = source(Some("127.0.0.1:4444"), Some("file"));
        let declared = source(Some("127.0.0.1:1111"), Some("declared"));
        let nothing = ControllerSettings::default();

        let resolve = |argv: &ControllerSettings,
                       env: &ControllerSettings,
                       file: &ControllerSettings,
                       declared: &ControllerSettings| {
            resolve_controller(argv.clone(), env.clone(), file.clone(), declared.clone())
        };

        // mihomo's own order, one source at a time.
        assert_eq!(
            resolve(&argv, &env, &file, &declared).unwrap().address,
            "127.0.0.1:2222"
        );
        assert_eq!(
            resolve(&nothing, &env, &file, &declared).unwrap().address,
            "127.0.0.1:3333"
        );
        assert_eq!(
            resolve(&nothing, &nothing, &file, &declared)
                .unwrap()
                .address,
            "127.0.0.1:4444"
        );
        // `tray.yml` only where mihomo says nothing.
        let last = resolve(&nothing, &nothing, &nothing, &declared).unwrap();
        assert_eq!(last.address, "127.0.0.1:1111");
        assert_eq!(last.secret, "declared");
        assert!(resolve(&nothing, &nothing, &nothing, &nothing).is_none());

        // A source that says nothing about the secret leaves it to the next one,
        // which is what "the kernel sets no secret" looks like.
        let split = resolve(
            &source(Some("127.0.0.1:2222"), None),
            &nothing,
            &file,
            &declared,
        )
        .unwrap();
        assert_eq!(split.address, "127.0.0.1:2222");
        assert_eq!(split.secret, "file");
    }

    #[test]
    fn a_flag_value_is_read_the_way_go_reads_it() {
        let args = |items: &[&str]| {
            items
                .iter()
                .map(|item| item.to_string())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            flag_value(&args(&["-f", r"C:\a.yaml"]), "f").as_deref(),
            Some(r"C:\a.yaml")
        );
        assert_eq!(
            flag_value(&args(&[r"-f=C:\b.yaml"]), "f").as_deref(),
            Some(r"C:\b.yaml")
        );
        assert_eq!(
            flag_value(&args(&[r"--f=C:\c.yaml"]), "f").as_deref(),
            Some(r"C:\c.yaml")
        );
        // The last occurrence wins, exactly as it does in mihomo's own parser.
        assert_eq!(
            flag_value(&args(&["-f", r"C:\a.yaml", "-f", r"C:\b.yaml"]), "f").as_deref(),
            Some(r"C:\b.yaml")
        );
        // A flag that is absent, or one with no value, says nothing.
        assert_eq!(flag_value(&args(&["-m"]), "f"), None);
        assert_eq!(flag_value(&args(&["-f"]), "f"), None);
    }

    #[test]
    fn the_config_file_of_a_command_line_follows_mihomos_rule() {
        let args = |items: &[&str]| {
            items
                .iter()
                .map(|item| item.to_string())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            argv_config(&args(&["-f", r"C:\a\custom.yaml"])),
            Some(PathBuf::from(r"C:\a\custom.yaml"))
        );
        assert_eq!(
            argv_config(&args(&["-d", r"C:\a"])),
            Some(PathBuf::from(r"C:\a\config.yaml"))
        );
        // A relative path would be resolved against the kernel's working
        // directory, which this program cannot read: "unknown" is the answer.
        assert_eq!(argv_config(&args(&["-f", "custom.yaml"])), None);
        assert_eq!(argv_config(&args(&["-d", "relative"])), None);
    }

    #[test]
    fn only_a_kernel_that_reads_our_file_is_asked() {
        let kernel = |pid: u32, denied: bool| proc::Process {
            pid,
            parent: 0,
            path: PathBuf::from(r"C:\mihomo\mihomo.exe"),
            denied,
        };
        let ours = PathBuf::from(r"C:\mihomo\config.yaml");
        let args_of = |pid: u32| match pid {
            1 => Some(vec!["-f".to_string(), r"C:\other\config.yaml".to_string()]),
            2 => Some(vec!["-f".to_string(), r"C:\mihomo\config.yaml".to_string()]),
            _ => None,
        };

        // The kernel reading the file this program reads is the one being asked,
        // even when another kernel is listed first.
        let both = [kernel(1, false), kernel(2, false)];
        assert_eq!(
            kernel_arguments_from(&both, Some(&ours), args_of).unwrap()[1],
            r"C:\mihomo\config.yaml"
        );
        // A lone kernel leaves nothing to choose between.
        assert!(kernel_arguments_from(&[kernel(1, false)], None, args_of).is_some());
        // Several, none of them reading our file: no answer rather than a guess.
        assert!(
            kernel_arguments_from(&[kernel(1, false), kernel(3, false)], Some(&ours), args_of)
                .is_none()
        );
        // A kernel of higher integrity is not read at all.
        assert!(kernel_arguments_from(&[kernel(2, true)], Some(&ours), args_of).is_none());
    }

    #[test]
    fn tray_yml_is_used_when_the_kernel_says_nothing() {
        let path = config_file("no-controller.yaml", "mixed-port: 7890\n");
        let settings = Settings {
            mihomo_config: path.display().to_string(),
            controller_address: "127.0.0.1:1234".into(),
            controller_secret: "tok".into(),
            ..Settings::default()
        };
        let client = find_controller_with(&settings, None, ControllerSettings::default()).unwrap();
        assert_eq!(client.address(), "127.0.0.1:1234");
        assert_eq!(client.secret, "tok");
    }

    #[test]
    fn a_kernel_without_a_controller_is_reported() {
        let path = config_file("plain.yaml", "mixed-port: 7890\n");
        let settings = Settings {
            mihomo_config: path.display().to_string(),
            ..Settings::default()
        };
        let error =
            find_controller_with(&settings, None, ControllerSettings::default()).unwrap_err();
        assert!(error.contains("external-controller"), "{error}");
    }

    #[test]
    fn a_controller_this_program_cannot_speak_to_is_reported_as_such() {
        let path = config_file(
            "only-pipe.yaml",
            "external-controller-pipe: \\\\.\\pipe\\mihomo\n",
        );
        let settings = Settings {
            mihomo_config: path.display().to_string(),
            ..Settings::default()
        };
        let error =
            find_controller_with(&settings, None, ControllerSettings::default()).unwrap_err();
        assert!(error.contains("TLS/unix/pipe"), "{error}");
    }

    /// An `https://` address names a controller this program has no client for.
    /// The refusal is what keeps the scheme from being dropped and the request
    /// from going out in plaintext.
    #[test]
    fn an_https_controller_is_refused_with_its_own_reason() {
        let path = config_file("tls.yaml", "external-controller: https://127.0.0.1:9090\n");
        let settings = Settings {
            mihomo_config: path.display().to_string(),
            ..Settings::default()
        };
        let error =
            find_controller_with(&settings, None, ControllerSettings::default()).unwrap_err();
        assert!(error.contains("https"), "{error}");
        assert!(!error.contains("host:port"), "{error}");
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
    fn the_kernel_is_the_declared_one_or_the_first_the_chain_finds() {
        // An empty chain is "not found", which the caller reports as such.
        assert!(
            find_kernel_among(&Settings::default(), &[])
                .unwrap()
                .is_none()
        );

        // A declared path that is not there is reported, not skipped: skipping is
        // what made a tray look like it could not find a kernel it was told about.
        let settings = Settings {
            mihomo_path: r"Z:\definitely\missing\mihomo.exe".into(),
            ..Settings::default()
        };
        let error = find_kernel_among(&settings, &[]).unwrap_err();
        assert!(error.contains("mihomo.path"), "{error}");

        // A relative path is refused before anything is opened.
        let settings = Settings {
            mihomo_path: r"scoop\apps\mihomo.exe".into(),
            ..Settings::default()
        };
        assert!(
            find_kernel_among(&settings, &[])
                .unwrap_err()
                .contains("mihomo.path")
        );
    }

    /// A chain of existing files, in the order the tests hand them in.
    fn chain(dir: &Path, names: &[&str]) -> Vec<PathBuf> {
        std::fs::create_dir_all(dir).unwrap();
        names
            .iter()
            .map(|name| {
                let file = dir.join(name);
                std::fs::write(&file, "").unwrap();
                file
            })
            .collect()
    }

    /// A file in a `shims` directory, which is what a scoop shim looks like: a
    /// launcher that carries the kernel's name.
    fn shim(dir: &Path) -> PathBuf {
        let dir = dir.join("shims");
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join(proc::KERNEL_EXE);
        std::fs::write(&file, "").unwrap();
        file
    }

    #[test]
    fn the_chain_takes_the_first_hit_and_prefers_a_real_binary() {
        let dir = std::env::temp_dir().join("mihomo-tray-test-chain");
        let _ = std::fs::remove_dir_all(&dir);
        let found = chain(&dir, &["mihomo.exe", "second.exe"]);
        let (beside, second) = (&found[0], &found[1]);
        let launcher = shim(&dir);
        assert!(proc::is_shim(&launcher));

        // The first existing candidate wins.
        assert_eq!(
            found_kernel(&[beside.clone(), second.clone()]),
            Some(Kernel::Discovered(beside.clone()))
        );
        // A candidate that is not there is skipped, not reported: the chain is a
        // search, and only the whole chain coming up empty is "not found".
        assert_eq!(
            found_kernel(&[dir.join("absent.exe"), second.clone()]),
            Some(Kernel::Discovered(second.clone()))
        );
        // A scoop shim is a launcher with the kernel's name, so a real binary later
        // in the chain is preferred — a kernel started from the shim reports an
        // image the shim does not have.
        assert_eq!(
            found_kernel(&[launcher.clone(), second.clone()]),
            Some(Kernel::Discovered(second.clone()))
        );
        // A shim is still better than nothing.
        assert_eq!(
            found_kernel(std::slice::from_ref(&launcher)),
            Some(Kernel::Discovered(launcher.clone()))
        );
        assert_eq!(found_kernel(&[]), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_declared_kernel_is_not_searched_for() {
        let dir = std::env::temp_dir().join("mihomo-tray-test-declared");
        let found = chain(&dir, &["declared.exe", "found.exe"]);
        let settings = Settings {
            mihomo_path: found[0].display().to_string(),
            ..Settings::default()
        };
        // The declaration is the answer even when the chain would find another
        // one: this program starts what it was told to start.
        assert_eq!(
            find_kernel_among(&settings, &found).unwrap(),
            Some(Kernel::Declared(found[0].clone()))
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_chain_looks_beside_the_tray_then_along_path_then_in_scoop() {
        let path_var = std::env::join_paths([r"C:\first", r"C:\second"]).unwrap();
        let candidates = kernel_candidates_for(
            Some(Path::new(r"C:\tray")),
            Some(&path_var),
            Some(Path::new(r"C:\scoop")),
            Some(Path::new(r"C:\Users\someone")),
        );
        assert_eq!(
            candidates,
            vec![
                PathBuf::from(r"C:\tray\mihomo.exe"),
                PathBuf::from(r"C:\tray\bin\mihomo.exe"),
                PathBuf::from(r"C:\tray\core\mihomo.exe"),
                PathBuf::from(r"C:\first\mihomo.exe"),
                PathBuf::from(r"C:\second\mihomo.exe"),
                PathBuf::from(r"C:\scoop\shims\mihomo.exe"),
                PathBuf::from(r"C:\Users\someone\scoop\shims\mihomo.exe"),
            ]
        );
    }

    #[test]
    fn scoop_installs_are_found_through_their_current_link() {
        let root = std::env::temp_dir().join("mihomo-tray-test-scoop");
        let _ = std::fs::remove_dir_all(&root);
        let installed = root
            .join("apps")
            .join("mihomo-v3")
            .join("current")
            .join(proc::KERNEL_EXE);
        std::fs::create_dir_all(installed.parent().unwrap()).unwrap();
        std::fs::create_dir_all(root.join("apps").join("other-app")).unwrap();
        std::fs::write(&installed, "").unwrap();

        let candidates = kernel_candidates_for(None, None, Some(&root), None);
        // A real installed version comes before the shim that launches it, and an
        // application that merely starts with `mihomo` is not mistaken for one.
        assert_eq!(candidates.first(), Some(&installed));
        assert_eq!(
            candidates.last(),
            Some(&root.join("shims").join(proc::KERNEL_EXE))
        );
        assert!(
            !candidates
                .iter()
                .any(|candidate| candidate.to_string_lossy().contains("other-app"))
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_kernel_in_the_bundle_root_brings_the_root_configuration() {
        let root = std::env::temp_dir().join("mihomo-tray-test-bundle");
        let _ = std::fs::remove_dir_all(&root);
        let bin = root.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let kernel = bin.join(proc::KERNEL_EXE);
        std::fs::write(&kernel, "").unwrap();
        let home = root.join("default-home");
        std::fs::create_dir_all(&home).unwrap();
        let default_config = home.join("config.yaml");
        std::fs::write(&default_config, "").unwrap();

        // `mihomo.exe` in `bin\` with the configuration at the root of the bundle:
        // the root wins, and `-d` becomes that root.
        let root_config = root.join("config.yaml");
        std::fs::write(&root_config, "").unwrap();
        assert_eq!(
            kernel_config_with(
                &Settings::default(),
                Some(&kernel),
                Some(&root),
                Some(home.clone())
            )
            .unwrap(),
            Some(root_config.clone())
        );

        // Without one at the root, the file beside the kernel is the candidate.
        std::fs::remove_file(&root_config).unwrap();
        let beside = bin.join("config.yaml");
        std::fs::write(&beside, "").unwrap();
        assert_eq!(
            kernel_config_with(
                &Settings::default(),
                Some(&kernel),
                Some(&root),
                Some(home.clone())
            )
            .unwrap(),
            Some(beside.clone())
        );

        // Neither: mihomo's own default, exactly as if the kernel had not been
        // found at all.
        std::fs::remove_file(&beside).unwrap();
        assert_eq!(
            kernel_config_with(
                &Settings::default(),
                Some(&kernel),
                Some(&root),
                Some(home.clone())
            )
            .unwrap(),
            Some(default_config.clone())
        );

        // A kernel elsewhere on the machine takes its own directory, never the
        // tray's.
        let elsewhere = std::env::temp_dir()
            .join("mihomo-tray-test-bundle-elsewhere")
            .join("mihomo.exe");
        std::fs::create_dir_all(elsewhere.parent().unwrap()).unwrap();
        std::fs::write(&elsewhere, "").unwrap();
        assert_eq!(
            kernel_config_with(
                &Settings::default(),
                Some(&elsewhere),
                Some(&root),
                Some(home.clone())
            )
            .unwrap(),
            Some(default_config.clone())
        );

        // `mihomo.home` and `mihomo.config` still have the last word over all of
        // it: the candidates are for an undeclared layout only.
        let declared = Settings {
            mihomo_home: home.display().to_string(),
            ..Settings::default()
        };
        assert_eq!(
            kernel_config_with(&declared, Some(&kernel), Some(&root), Some(home.clone())).unwrap(),
            Some(default_config.clone())
        );
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(elsewhere.parent().unwrap());
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
            launch_args_with(&settings, None, ControllerSettings::default()).unwrap(),
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
            launch_args_with(&settings, None, ControllerSettings::default()).unwrap(),
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
    fn the_controller_environment_is_written_onto_the_command_line() {
        // `CLASH_OVERRIDE_*` is the default of `-ext-ctl`/`-secret`, so writing it
        // out says the same thing to the kernel — and being written out is what
        // makes it survive the elevated restart, which inherits no environment.
        let (dir, file) = scratch("env.yaml");
        let settings = Settings {
            mihomo_config: file.display().to_string(),
            ..Settings::default()
        };
        assert_eq!(
            launch_args_with(
                &settings,
                None,
                source(Some("127.0.0.1:9095"), Some("testsecret")),
            )
            .unwrap(),
            vec![
                "-d".to_string(),
                dir.display().to_string(),
                "-f".to_string(),
                file.display().to_string(),
                "-ext-ctl".to_string(),
                "127.0.0.1:9095".to_string(),
                "-secret".to_string(),
                "testsecret".to_string(),
            ]
        );
        // Each variable stands on its own, and an unset one adds nothing.
        assert_eq!(
            override_args(&source(None, Some("tok"))),
            vec!["-secret".to_string(), "tok".to_string()]
        );
        assert_eq!(override_args(&source(None, None)), Vec::<String>::new());
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
            let error = launch_args(&settings, None).unwrap_err();
            // The report names the field to use instead, which is the useful half.
            assert!(error.contains(field), "{flag}: {error}");
        }
        // An argument that belongs to the kernel alone is passed through.
        let settings = Settings {
            mihomo_config: file.display().to_string(),
            mihomo_args: vec!["-ext-ui=C:\\ui".to_string()],
            ..Settings::default()
        };
        assert_eq!(launch_args(&settings, None).unwrap()[0], "-ext-ui=C:\\ui");
        let _ = std::fs::remove_file(&file);
    }
}
