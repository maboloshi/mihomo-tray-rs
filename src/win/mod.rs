//! Win32 glue: a hidden message window, the notification-area icon, the popup
//! menu plumbing, dark-mode opt-in and the message loop.
//!
//! `TrackPopupMenuEx` pumps messages, so the window procedure can be re-entered
//! while a menu is open. Every helper therefore works on a raw `*mut App` and
//! only creates short-lived references outside the pumping call.

pub mod autostart;
pub mod elevate;
pub mod menu;
pub mod proxy;

use std::sync::OnceLock;

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint,
};
use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress, LoadLibraryW};
use windows_sys::Win32::UI::Shell::{
    NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY, NIM_SETVERSION,
    NOTIFYICON_VERSION_4, NOTIFYICONDATAW, Shell_NotifyIconW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GWLP_USERDATA, GetCursorPos,
    GetMessageW, GetWindowLongPtrW, HICON, MSG, PostMessageW, PostQuitMessage, RegisterClassW,
    RegisterWindowMessageW, SetForegroundWindow, SetWindowLongPtrW, TPM_NONOTIFY, TPM_RETURNCMD,
    TPM_RIGHTBUTTON, TrackPopupMenuEx, TranslateMessage, WM_APP, WM_CONTEXTMENU, WM_DESTROY,
    WM_LBUTTONUP, WM_NULL, WM_RBUTTONUP, WNDCLASSW, WS_POPUP,
};

use crate::app::App;
use crate::settings::DarkMenu;

pub const WM_TRAY: u32 = WM_APP + 1;
pub const WM_REFRESH: u32 = WM_APP + 2;
const TRAY_ID: u32 = 1;

/// Create the (never shown) window that owns the tray icon and receives menu
/// commands. `app` must outlive the window.
pub fn create_message_window(app: *mut App) -> Result<HWND, String> {
    unsafe {
        let instance = GetModuleHandleW(std::ptr::null());
        let class_name = wide("MihomoTrayMessageWindow");
        let class = WNDCLASSW {
            style: 0,
            lpfnWndProc: Some(wndproc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: instance,
            hIcon: std::ptr::null_mut(),
            hCursor: std::ptr::null_mut(),
            hbrBackground: std::ptr::null_mut(),
            lpszMenuName: std::ptr::null(),
            lpszClassName: class_name.as_ptr(),
        };
        RegisterClassW(&class);
        let hwnd = CreateWindowExW(
            0,
            class_name.as_ptr(),
            class_name.as_ptr(),
            WS_POPUP,
            0,
            0,
            0,
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            instance,
            std::ptr::null(),
        );
        if hwnd.is_null() {
            return Err("创建消息窗口失败".into());
        }
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, app as isize);
        Ok(hwnd)
    }
}

pub fn add_icon(hwnd: HWND, icon: HICON, tooltip: &str) -> bool {
    unsafe {
        let mut data = icon_data(hwnd, icon, tooltip);
        if Shell_NotifyIconW(NIM_ADD, &data) == 0 {
            return false;
        }
        data.Anonymous.uVersion = NOTIFYICON_VERSION_4;
        if Shell_NotifyIconW(NIM_SETVERSION, &data) == 0 {
            // The window procedure always reads the version-4 message layout, so
            // an icon stuck on the legacy layout would never open the menu. Let
            // the caller retry from scratch.
            Shell_NotifyIconW(NIM_DELETE, &data);
            return false;
        }
        true
    }
}

pub fn update_icon(hwnd: HWND, icon: HICON, tooltip: &str) {
    unsafe {
        let data = icon_data(hwnd, icon, tooltip);
        Shell_NotifyIconW(NIM_MODIFY, &data);
    }
}

pub fn remove_icon(hwnd: HWND) {
    unsafe {
        let mut data: NOTIFYICONDATAW = std::mem::zeroed();
        data.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
        data.hWnd = hwnd;
        data.uID = TRAY_ID;
        Shell_NotifyIconW(NIM_DELETE, &data);
    }
}

fn icon_data(hwnd: HWND, icon: HICON, tooltip: &str) -> NOTIFYICONDATAW {
    let mut data: NOTIFYICONDATAW = unsafe { std::mem::zeroed() };
    data.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
    data.hWnd = hwnd;
    data.uID = TRAY_ID;
    data.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
    data.uCallbackMessage = WM_TRAY;
    data.hIcon = icon;
    // `szTip` is a fixed 128-unit buffer that the shell expects to be NUL
    // terminated, so truncate by UTF-16 units and leave room for the terminator.
    let tip: Vec<u16> = tooltip.encode_utf16().take(data.szTip.len() - 1).collect();
    data.szTip[..tip.len()].copy_from_slice(&tip);
    data
}

/// Dark menus need an undocumented (but stable and widely used) uxtheme opt-in.
/// When it is unavailable the shell simply keeps drawing a light menu.
pub fn apply_dark_mode(hwnd: HWND, preference: DarkMenu) {
    let dark = match preference {
        DarkMenu::Always => true,
        DarkMenu::Never => false,
        DarkMenu::Auto => apps_use_dark_theme(),
    };
    if !dark {
        return;
    }
    unsafe {
        let uxtheme = LoadLibraryW(wide("uxtheme.dll").as_ptr());
        if uxtheme.is_null() {
            return;
        }
        type SetPreferredAppMode = unsafe extern "system" fn(i32) -> i32;
        type AllowDarkModeForWindow = unsafe extern "system" fn(HWND, i32) -> i32;
        type SetWindowTheme = unsafe extern "system" fn(HWND, *const u16, *const u16) -> i32;

        // Prefer the exported name; fall back to the well-known ordinals only
        // when the name is missing (they are not part of the documented ABI).
        let set_mode_proc = GetProcAddress(uxtheme, c"SetPreferredAppMode".as_ptr() as *const u8)
            .or_else(|| GetProcAddress(uxtheme, 135 as *const u8));
        if let Some(proc) = set_mode_proc {
            let set_mode: SetPreferredAppMode = std::mem::transmute(proc);
            set_mode(2); // ForceDark
        }
        let allow_proc = GetProcAddress(uxtheme, c"AllowDarkModeForWindow".as_ptr() as *const u8)
            .or_else(|| GetProcAddress(uxtheme, 133 as *const u8));
        if let Some(proc) = allow_proc {
            let allow: AllowDarkModeForWindow = std::mem::transmute(proc);
            allow(hwnd, 1);
        }
        if let Some(proc) = GetProcAddress(uxtheme, c"SetWindowTheme".as_ptr() as *const u8) {
            let set_theme: SetWindowTheme = std::mem::transmute(proc);
            set_theme(hwnd, wide("DarkMode_Explorer").as_ptr(), std::ptr::null());
        }
    }
}

fn apps_use_dark_theme() -> bool {
    winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER)
        .open_subkey(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize")
        .and_then(|key| key.get_value::<u32, _>("AppsUseLightTheme"))
        .map(|light| light == 0)
        .unwrap_or(false)
}

pub fn run_message_loop() -> i32 {
    unsafe {
        let mut message: MSG = std::mem::zeroed();
        while GetMessageW(&mut message, std::ptr::null_mut(), 0, 0) > 0 {
            TranslateMessage(&message);
            DispatchMessageW(&message);
        }
        message.wParam as i32
    }
}

pub fn quit(hwnd: HWND) {
    unsafe {
        DestroyWindow(hwnd);
    }
}

/// Fatal startup errors have no tray icon yet, so they need a message box.
pub fn fatal(message: &str) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_OK, MessageBoxW};
    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            wide(message).as_ptr(),
            wide("mihomo-tray").as_ptr(),
            MB_OK | MB_ICONERROR,
        );
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        let app = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut App;
        if app.is_null() {
            return DefWindowProcW(hwnd, msg, wparam, lparam);
        }
        match msg {
            WM_TRAY => {
                on_tray_event(app, wparam, lparam);
                0
            }
            WM_REFRESH => {
                // Skip while a menu is open: the next poll covers it, and it keeps
                // the menu data consistent with what the user is looking at.
                if !(*app).menu_open {
                    (*app).refresh_ui();
                }
                0
            }
            WM_DESTROY => {
                // Idempotent, and the only place that is guaranteed to run for
                // every teardown path (logoff, Explorer shutdown, `quit`).
                remove_icon(hwnd);
                PostQuitMessage(0);
                0
            }
            other if other == taskbar_created() => {
                (*app).on_taskbar_created();
                0
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
}

fn on_tray_event(app: *mut App, wparam: WPARAM, lparam: LPARAM) {
    // NOTIFYICON_VERSION_4: the low word of lparam is the event, wparam packs x/y.
    let event = (lparam & 0xffff) as u32;
    if !matches!(event, WM_RBUTTONUP | WM_CONTEXTMENU | WM_LBUTTONUP) {
        return;
    }
    let x = (wparam & 0xffff) as i16 as i32;
    let y = ((wparam >> 16) & 0xffff) as i16 as i32;
    let (x, y) = if x < 0 || y < 0 {
        let mut point = POINT { x: 0, y: 0 };
        unsafe {
            GetCursorPos(&mut point);
        }
        (point.x, point.y)
    } else {
        (x, y)
    };
    show_context_menu(app, x, y)
}

fn show_context_menu(app: *mut App, x: i32, y: i32) {
    unsafe {
        if (*app).menu_open {
            return;
        }
        (*app).menu_open = true;

        let snapshot = crate::state::read(&(*app).state);
        let mut menu = menu::Menu::build(&snapshot, &(*app).settings, (*app).admin);
        let hwnd = (*app).hwnd;
        let (x, y) = clamp_to_work_area(x, y);

        SetForegroundWindow(hwnd);
        let selected = TrackPopupMenuEx(
            menu.handle,
            TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_NONOTIFY,
            x,
            y,
            hwnd,
            std::ptr::null(),
        );
        // The shell documentation recommends a benign message after the menu
        // closes, otherwise the next click on the icon can be swallowed.
        PostMessageW(hwnd, WM_NULL, 0, 0);

        let action = menu.action(selected as usize).cloned();
        menu.destroy();
        // Keep the guard set across `dispatch`: it performs Win32 calls that can
        // pump messages (ShellExecuteW, InternetSetOptionW), and without the guard
        // a pumped message could re-enter the window procedure and build a second
        // `&mut App` while this one is still live.
        if let Some(action) = action {
            (*app).dispatch(&action);
        }
        (*app).menu_open = false;
    }
}

fn clamp_to_work_area(x: i32, y: i32) -> (i32, i32) {
    unsafe {
        let monitor = MonitorFromPoint(POINT { x, y }, MONITOR_DEFAULTTONEAREST);
        let mut info: MONITORINFO = std::mem::zeroed();
        info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        if GetMonitorInfoW(monitor, &mut info) != 0 {
            let work = info.rcWork;
            return (
                x.clamp(work.left, (work.right - 1).max(work.left)),
                y.clamp(work.top, (work.bottom - 1).max(work.top)),
            );
        }
    }
    (x, y)
}

fn taskbar_created() -> u32 {
    static MESSAGE: OnceLock<u32> = OnceLock::new();
    *MESSAGE.get_or_init(|| unsafe { RegisterWindowMessageW(wide("TaskbarCreated").as_ptr()) })
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}
