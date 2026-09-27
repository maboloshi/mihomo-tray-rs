//! Tray icons: drawn from raw pixels (no image crate, no asset files).
//!
//! Each state icon is a filled circle with a slightly darker outline and a light
//! centre dot, rendered with 4x supersampling so the shapes look smooth at the
//! small sizes the notification area uses.

use windows_sys::Win32::Foundation::HANDLE;
use windows_sys::Win32::Graphics::Gdi::{
    BI_BITFIELDS, BITMAPV5HEADER, CreateBitmap, CreateDIBSection, DIB_RGB_COLORS, DeleteObject,
    GetDC, HBITMAP, HDC, ReleaseDC,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateIconIndirect, DestroyIcon, GetSystemMetrics, HICON, ICONINFO, SM_CXSMICON,
};

const GRAY: [u8; 3] = [0x88, 0x88, 0x88];
const GREEN: [u8; 3] = [0x4C, 0xAF, 0x50];
const AMBER: [u8; 3] = [0xFF, 0x98, 0x00];
const BLUE: [u8; 3] = [0x21, 0x96, 0xF3];

/// What the tray icon shows. "The kernel is usable" and the two ways of taking
/// traffic over are three different things, so they get three different colours.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// The controller does not answer: the kernel is not usable.
    Unreachable,
    /// The kernel answers, and nothing is being taken over.
    Ready,
    /// The system proxy is on.
    SystemProxy,
    /// TUN is on.
    Tun,
}

pub struct Icons {
    pub gray: HICON,
    pub green: HICON,
    pub amber: HICON,
    pub blue: HICON,
}

impl Icons {
    pub fn new() -> Self {
        let size = unsafe { GetSystemMetrics(SM_CXSMICON) }.clamp(16, 64);
        Self {
            gray: make_icon(&render(GRAY, size as usize), size),
            green: make_icon(&render(GREEN, size as usize), size),
            amber: make_icon(&render(AMBER, size as usize), size),
            blue: make_icon(&render(BLUE, size as usize), size),
        }
    }

    /// TUN wins over the system proxy and both win over a plainly usable kernel,
    /// matching the classic tray behaviour.
    pub fn for_state(&self, state: State) -> HICON {
        match state {
            State::Tun => self.blue,
            State::SystemProxy => self.amber,
            State::Ready => self.green,
            State::Unreachable => self.gray,
        }
    }
}

impl Drop for Icons {
    fn drop(&mut self) {
        unsafe {
            DestroyIcon(self.gray);
            DestroyIcon(self.green);
            DestroyIcon(self.amber);
            DestroyIcon(self.blue);
        }
    }
}

/// BGRA pixels, alpha applied, 4x supersampled.
fn render(color: [u8; 3], size: usize) -> Vec<u8> {
    const SS: usize = 4;
    let big = size * SS;
    let center = (big as f32 - 1.0) / 2.0;
    let radius = big as f32 / 2.0 - SS as f32;
    let outline = radius - SS as f32 * 1.2;
    let dot = radius * 0.34;
    let dark = [color[0] * 3 / 5, color[1] * 3 / 5, color[2] * 3 / 5];

    // Premultiplied accumulation, so edges do not fade towards black.
    let mut acc = vec![0u32; size * size * 4];
    for y in 0..big {
        for x in 0..big {
            let dx = x as f32 - center;
            let dy = y as f32 - center;
            let dist = (dx * dx + dy * dy).sqrt();
            let (rgb, alpha) = if dist > radius {
                ([0u8; 3], 0u8)
            } else if dist > outline {
                (dark, 255u8)
            } else if dist < dot {
                (mix(color, [255, 255, 255], 0.72), 255)
            } else {
                (color, 255)
            };
            let idx = (y / SS * size + x / SS) * 4;
            let a = alpha as u32;
            acc[idx] += rgb[2] as u32 * a / 255; // B
            acc[idx + 1] += rgb[1] as u32 * a / 255; // G
            acc[idx + 2] += rgb[0] as u32 * a / 255; // R
            acc[idx + 3] += a;
        }
    }

    let per = (SS * SS) as u32;
    let mut out = vec![0u8; size * size * 4];
    for px in 0..size * size {
        let a = acc[px * 4 + 3] / per;
        if a == 0 {
            continue;
        }
        for c in 0..3 {
            // un-premultiply
            out[px * 4 + c] = (acc[px * 4 + c] / per * 255 / a).min(255) as u8;
        }
        out[px * 4 + 3] = a as u8;
    }
    out
}

fn mix(a: [u8; 3], b: [u8; 3], t: f32) -> [u8; 3] {
    let f = |x: u8, y: u8| (x as f32 * (1.0 - t) + y as f32 * t).round() as u8;
    [f(a[0], b[0]), f(a[1], b[1]), f(a[2], b[2])]
}

/// Build an `HICON` from BGRA pixels through a 32bpp DIB section.
fn make_icon(bgra: &[u8], size: i32) -> HICON {
    unsafe {
        let mut header: BITMAPV5HEADER = std::mem::zeroed();
        header.bV5Size = std::mem::size_of::<BITMAPV5HEADER>() as u32;
        header.bV5Width = size;
        header.bV5Height = -size; // top-down
        header.bV5Planes = 1;
        header.bV5BitCount = 32;
        header.bV5Compression = BI_BITFIELDS;
        header.bV5RedMask = 0x00FF_0000;
        header.bV5GreenMask = 0x0000_FF00;
        header.bV5BlueMask = 0x0000_00FF;
        header.bV5AlphaMask = 0xFF00_0000;

        let screen: HDC = GetDC(std::ptr::null_mut());
        let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
        let color_bitmap: HBITMAP = CreateDIBSection(
            screen,
            &header as *const BITMAPV5HEADER as *const _,
            DIB_RGB_COLORS,
            &mut bits,
            std::ptr::null_mut::<core::ffi::c_void>() as HANDLE,
            0,
        );
        if color_bitmap.is_null() || bits.is_null() {
            if !color_bitmap.is_null() {
                DeleteObject(color_bitmap);
            }
            ReleaseDC(std::ptr::null_mut(), screen);
            return std::ptr::null_mut();
        }
        std::ptr::copy_nonoverlapping(bgra.as_ptr(), bits as *mut u8, bgra.len());

        // `lpBits = NULL` would leave the AND mask uninitialised; a zeroed mask
        // is the documented equivalent of "fully opaque, alpha comes from the
        // colour bitmap".
        let stride = (size as usize).div_ceil(16) * 2;
        let mask_bits = vec![0u8; stride * size as usize];
        let mask_bitmap = CreateBitmap(
            size,
            size,
            1,
            1,
            mask_bits.as_ptr() as *const core::ffi::c_void,
        );
        let info = ICONINFO {
            fIcon: 1,
            xHotspot: 0,
            yHotspot: 0,
            hbmMask: mask_bitmap,
            hbmColor: color_bitmap,
        };
        let icon = CreateIconIndirect(&info);

        DeleteObject(color_bitmap);
        DeleteObject(mask_bitmap);
        ReleaseDC(std::ptr::null_mut(), screen);
        icon
    }
}
