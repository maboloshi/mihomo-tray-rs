#![windows_subsystem = "windows"]

mod app;
mod i18n;
mod icon;
mod instance;
mod mihomo;
mod settings;
mod state;
mod win;

use std::time::Duration;

use app::App;
use mihomo::Client;

/// How long to wait for a kernel we just launched to answer the controller.
const KERNEL_STARTUP_BUDGET: Duration = Duration::from_secs(5);
/// Per-probe timeout while waiting, so the loop cannot stall for the full
/// configured controller timeout on every iteration.
const KERNEL_PROBE_TIMEOUT_MS: u32 = 500;

fn main() {
    // The elevated helper is a second process of this same executable: it runs
    // before the single-instance guard (which exists for the tray) and never
    // creates a window or an icon.
    if let Some(code) = run_kernel_helper() {
        std::process::exit(code);
    }

    if !instance::acquire() {
        return;
    }

    let settings_path = settings::settings_path();
    settings::ensure_default_file(&settings_path);
    // First, because everything below reports failures through the UI strings.
    i18n::init();
    let (settings, settings_error) = settings::load(&settings_path);

    let kernel_path = mihomo::discover::find_kernel(&settings);
    let client = mihomo::discover::find_controller(&settings);

    // Launch the kernel only when nothing is answering and the user allows it.
    let mut child = None;
    let mut fatal = settings_error.map(|error| {
        i18n::t().error_read_settings(&error.path.display().to_string(), &error.error.to_string())
    });
    if let Some(discovered) = client.as_ref() {
        if !discovered.alive() && settings.mihomo_auto_start {
            match &kernel_path {
                Some(path) if mihomo::proc::list_mihomo().is_empty() => {
                    let args = mihomo::discover::launch_args(&settings);
                    match mihomo::proc::start(path, &args) {
                        Ok(started) => {
                            child = Some(started);
                            wait_for_controller(&settings);
                        }
                        Err(error) => fatal = Some(error),
                    }
                }
                Some(_) => {}
                None => fatal = Some(i18n::t().error_kernel_not_found.to_string()),
            }
        }
    }
    let client = client.unwrap_or_else(|| {
        Client::new("127.0.0.1:9090", "", settings.controller_timeout_ms)
            .expect("default controller address is always valid")
    });

    // `App` deliberately lives for the whole process: the window procedure holds
    // a raw pointer to it, and the process exits as a unit.
    let shared = state::shared();
    let app = Box::into_raw(Box::new(App::new(
        settings.clone(),
        shared,
        kernel_path,
        fatal,
    )));

    unsafe {
        if let Some(child) = child {
            (*app).set_kernel(child);
        }
        match win::create_message_window(app) {
            Ok(hwnd) => (*app).hwnd = hwnd,
            Err(error) => {
                win::fatal(&i18n::t().error_window_create(&error));
                std::process::exit(1);
            }
        }
        win::apply_dark_mode((*app).hwnd, settings.dark_menu);
        (*app).start(client);
        win::run_message_loop();
    }
}

/// The two elevated-helper command lines `win::elevate` builds; every other
/// command line belongs to the tray.
fn run_kernel_helper() -> Option<i32> {
    let args = mihomo::proc::own_command_line();
    match args.get(1)?.as_str() {
        mihomo::proc::KERNEL_START_SWITCH => Some(mihomo::proc::start_kernel_elevated(&args[2..])),
        mihomo::proc::KERNEL_STOP_SWITCH => Some(mihomo::proc::stop_kernel_elevated(&args[2..])),
        _ => None,
    }
}

/// Poll the controller for a short, bounded time after starting the kernel.
fn wait_for_controller(settings: &settings::Settings) {
    let Some(probe) = Client::new("127.0.0.1:9090", "", KERNEL_PROBE_TIMEOUT_MS) else {
        return;
    };
    let deadline = std::time::Instant::now() + KERNEL_STARTUP_BUDGET;
    let addresses = {
        let mut list = vec![probe];
        if !settings.controller_address.is_empty() {
            if let Some(client) = Client::new(
                &settings.controller_address,
                &settings.controller_secret,
                KERNEL_PROBE_TIMEOUT_MS,
            ) {
                list.push(client);
            }
        }
        list
    };
    while std::time::Instant::now() < deadline {
        if addresses.iter().any(Client::alive) {
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}
