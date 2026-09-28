#![windows_subsystem = "windows"]

mod app;
mod i18n;
mod icon;
mod instance;
mod mihomo;
mod paths;
mod settings;
mod state;
mod win;

use app::App;

fn main() {
    // The elevated helper is a second process of this same executable: it runs
    // before the single-instance guard (which exists for the tray) and never
    // creates a window or an icon.
    if let Some(code) = run_kernel_helper() {
        std::process::exit(code);
    }

    // The language comes before the single-instance guard: a second launch is
    // answered with a message of its own, and that message has to be in the
    // user's language like everything else.
    i18n::init();

    if !instance::acquire() {
        // Silently leaving looks like "the program did not start" — on Windows 11
        // the icon of the running instance is usually hidden in the tray overflow,
        // so say that it is there instead.
        win::notice(&i18n::t().error_single_instance);
        return;
    }

    let settings_path = settings::settings_path();
    settings::ensure_default_file(&settings_path, settings::default_file(i18n::language()));
    let (settings, settings_error) = settings::load(&settings_path);
    let fatal = settings_error.map(|error| {
        i18n::t().error_read_settings(&error.path.display().to_string(), &error.error.to_string())
    });

    // `App` deliberately lives for the whole process: the window procedure holds
    // a raw pointer to it, and the process exits as a unit. Finding and starting
    // the kernel is not done here: the worker does that once the icon exists.
    let shared = state::shared();
    let app = Box::into_raw(Box::new(App::new(settings.clone(), shared, fatal)));

    unsafe {
        match win::create_message_window(app) {
            Ok(hwnd) => (*app).hwnd = hwnd,
            Err(error) => {
                win::fatal(&i18n::t().error_window_create(&error));
                std::process::exit(1);
            }
        }
        win::apply_dark_mode((*app).hwnd, settings.dark_menu);
        (*app).start();
        win::run_message_loop();
    }
}

/// The elevated-helper command lines `win::elevate` builds; every other command
/// line belongs to the tray.
fn run_kernel_helper() -> Option<i32> {
    let args = mihomo::proc::own_command_line();
    match args.get(1)?.as_str() {
        mihomo::proc::KERNEL_START_SWITCH => Some(mihomo::proc::start_kernel_elevated(&args[2..])),
        mihomo::proc::KERNEL_STOP_SWITCH => Some(mihomo::proc::stop_kernel_elevated(&args[2..])),
        mihomo::proc::KERNEL_REPLACE_SWITCH => {
            Some(mihomo::proc::replace_kernel_elevated(&args[2..]))
        }
        _ => None,
    }
}
