//! Tray icons: drawn from raw pixels (no image crate, no asset files).
//!
//! Two pieces of information share one icon, and each owns its own geometry:
//! the **ring colour** is the takeover state (what the tray has always shown),
//! and the **centre dot** is the proxy mode the kernel is running. When the mode
//! is unknown the dot is simply not drawn, so the icon falls back to a plain
//! state-coloured circle.

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
/// `global`'s dot.
///
/// Three values were built and looked at on a real taskbar before this one; the
/// numbers below are what the choice has to answer to. The difficult case is
/// always the system-proxy (amber) ring, where the dot's own warmth competes
/// with the ring:
///
/// * `#BC5A52` — chroma 45, ~22 lightness below the amber: too soft, the two
///   warm colours flood together and the dot stops reading as a dot.
/// * `#B03A2E` / `#B53026` — chroma 58/65, ~31 below the amber: readable
///   everywhere, but duller than the red the tray is supposed to say.
/// * `#C00000` — chroma 84, the most vivid thing in the icon (the rings sit at
///   40–80 and `rule`'s light dot at 17), so it reads as stuck on.
///
/// `#E53935` is the one that was kept: chroma 78 and ~20 below the amber, so by
/// the amber-gap rule of thumb it is in the same band as the soft `#BC5A52` —
/// yet it was the one that looked right on the machine it runs on. The preview
/// sheets are renders, not the tray; the real taskbar wins. If a future change
/// to this value is ever judged by numbers alone, check the amber case by eye
/// first.
const RED: [u8; 3] = [0xE5, 0x39, 0x35];
const NEAR_BLACK: [u8; 3] = [0x21, 0x21, 0x21];
/// How far `rule`'s centre dot is brightened towards white. Unchanged from when
/// the dot carried no meaning at all.
const LIGHT_DOT: f32 = 0.72;

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

impl State {
    /// The row this state occupies in `Icons::icons`. Spelled out rather than
    /// taken from the enum's discriminant so reordering the variants cannot
    /// silently shuffle the table.
    fn index(self) -> usize {
        match self {
            State::Unreachable => 0,
            State::Ready => 1,
            State::SystemProxy => 2,
            State::Tun => 3,
        }
    }
}

/// The proxy mode the kernel is running, shown as the centre of the icon. This
/// is a second, orthogonal axis to [`State`]: any mode can be held while any of
/// the four takeover states is in effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Rule,
    Global,
    Direct,
    /// The controller is unreachable (or answered something unrecognised), so
    /// there is no mode to show.
    Unknown,
}

impl Mode {
    /// Parse the `mode` field mihomo reports (`GET /configs`).
    ///
    /// Only the three values the kernel itself can hold are accepted. An empty
    /// string — what a snapshot carries while the controller is unreachable —
    /// and any future or unexpected value both mean "no mode to show", which is
    /// safer than guessing one: a wrong centre is a wrong answer about which
    /// way traffic is going.
    pub fn from_str(mode: &str) -> Self {
        match mode {
            "rule" => Mode::Rule,
            "global" => Mode::Global,
            "direct" => Mode::Direct,
            _ => Mode::Unknown,
        }
    }

    /// The column this mode occupies in `Icons::icons`.
    fn index(self) -> usize {
        match self {
            Mode::Rule => 0,
            Mode::Global => 1,
            Mode::Direct => 2,
            Mode::Unknown => 3,
        }
    }

    /// The colour of the centre dot, or `None` for "draw no dot".
    ///
    /// It takes the ring colour because `rule` is the original light dot: the
    /// ring colour brightened towards white, which is what the icon looked like
    /// before modes were shown here at all. `global` and `direct` carry their
    /// meaning in their own colour and ignore the ring.
    fn core(self, fill: [u8; 3]) -> Option<[u8; 3]> {
        match self {
            Mode::Rule => Some(mix(fill, [0xFF, 0xFF, 0xFF], LIGHT_DOT)),
            Mode::Global => Some(RED),
            Mode::Direct => Some(NEAR_BLACK),
            Mode::Unknown => None,
        }
    }
}

/// The full icon set: every combination of the four states and the four mode
/// values, built once per pixel size.
///
/// All sixteen are pre-built because `Shell_NotifyIconW` is called with a ready
/// `HICON` on the UI thread, where drawing a circle and a DIB per refresh would
/// be work done on every poll tick.
pub struct Icons {
    /// `icons[mode.index()][state.index()]`, so [`Icons::for_state`] can walk a
    /// mode row directly.
    icons: [[HICON; 4]; 4],
    /// The pixel size these icons were drawn for. Kept so the set can be measured
    /// against the current notification area, which changes with the DPI of the
    /// primary display.
    size: i32,
}

impl Icons {
    pub fn new() -> Self {
        let size = system_icon_size();
        let mut pixels = vec![0u8; (size as usize).pow(2) * 4];
        // Built by walking the same indices `for_state` reads with, reusing one
        // pixel buffer: `render` writes every byte it has coverage for and clears
        // the alpha of the ones it does not.
        let icons = std::array::from_fn(|mode| {
            std::array::from_fn(|state| {
                let mode = Mode::index_inverse(mode);
                let state = State::index_inverse(state);
                let fill = state.fill();
                render(fill, mode.core(fill), size as usize, &mut pixels);
                make_icon(&pixels, size)
            })
        });
        Self { icons, size }
    }

    /// Whether the notification area now wants icons of another pixel size.
    /// `TaskbarCreated` is re-broadcast after a display change, so this is what
    /// tells a re-registration to draw the icons again instead of reusing the ones
    /// measured when the process started.
    pub fn refresh_size(&self) -> bool {
        system_icon_size() != self.size
    }

    /// The pixel size the current set was drawn for.
    #[cfg(test)]
    pub fn size(&self) -> i32 {
        self.size
    }

    /// TUN wins over the system proxy and both win over a plainly usable kernel,
    /// matching the classic tray behaviour. The mode is an independent axis and
    /// never changes that priority.
    pub fn for_state(&self, state: State, mode: Mode) -> HICON {
        self.icons[mode.index()][state.index()]
    }
}

impl State {
    /// The ring colour for this state.
    fn fill(self) -> [u8; 3] {
        match self {
            State::Unreachable => GRAY,
            State::Ready => GREEN,
            State::SystemProxy => AMBER,
            State::Tun => BLUE,
        }
    }

    /// The state at a column index, for building the table by index.
    fn index_inverse(index: usize) -> Self {
        match index {
            0 => State::Unreachable,
            1 => State::Ready,
            2 => State::SystemProxy,
            _ => State::Tun,
        }
    }
}

impl Mode {
    /// The mode at a row index, for building the table by index.
    fn index_inverse(index: usize) -> Self {
        match index {
            0 => Mode::Rule,
            1 => Mode::Global,
            2 => Mode::Direct,
            _ => Mode::Unknown,
        }
    }
}

/// The size the notification area draws icons at. Read through a function rather
/// than kept in a constant: it follows the DPI of the primary display.
fn system_icon_size() -> i32 {
    unsafe { GetSystemMetrics(SM_CXSMICON) }.clamp(16, 64)
}

impl Drop for Icons {
    fn drop(&mut self) {
        unsafe {
            for row in &self.icons {
                for icon in row {
                    DestroyIcon(*icon);
                }
            }
        }
    }
}

/// The centre dot's radius as a fraction of the ring radius.
///
/// This is the dot the icon has always had, about five device pixels across on
/// the 16 px icon the notification area uses at 100% DPI. The three mode colours
/// have to survive that size — which is the honest cost of not changing the
/// icon's shape. `rule` keeps the original light dot (the fill mixed 72% towards
/// white), so the everyday mode looks exactly as it did before modes were shown
/// here at all.
const CORE_SCALE: f32 = 0.34;

/// BGRA pixels, alpha applied, 4x supersampled.
///
/// `core` is the centre colour, or `None` to draw no centre at all.
///
/// Writes `out` for every pixel it has coverage for and zeroes the alpha of the
/// rest, so the same buffer can be handed in again and again.
fn render(fill: [u8; 3], core: Option<[u8; 3]>, size: usize, out: &mut [u8]) {
    const SS: usize = 4;
    let big = size * SS;
    let center = (big as f32 - 1.0) / 2.0;
    let radius = big as f32 / 2.0 - SS as f32;
    let outline = radius - SS as f32 * 1.2;
    let core_radius = radius * CORE_SCALE;
    // The outline is 60% of the fill colour. The arithmetic is widened to `u16`
    // because `0xFF * 3` does not fit in a `u8`, which would panic as soon as
    // overflow checks are on.
    let dark = [
        (fill[0] as u16 * 3 / 5) as u8,
        (fill[1] as u16 * 3 / 5) as u8,
        (fill[2] as u16 * 3 / 5) as u8,
    ];

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
            } else if let Some(core) = core
                && dist < core_radius
            {
                (core, 255)
            } else {
                (fill, 255)
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
    for px in 0..size * size {
        let a = acc[px * 4 + 3] / per;
        if a == 0 {
            // Nothing covered this pixel: clear it, so a reused buffer does not
            // keep the previous icon's colour under zero alpha.
            out[px * 4..px * 4 + 4].fill(0);
            continue;
        }
        for c in 0..3 {
            // un-premultiply
            out[px * 4 + c] = (acc[px * 4 + c] / per * 255 / a).min(255) as u8;
        }
        out[px * 4 + 3] = a as u8;
    }
}

/// Linear blend, `t = 0` gives `a` and `t = 1` gives `b`.
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Every state, for exhaustiveness in the tests below.
    const STATES: [State; 4] = [
        State::Unreachable,
        State::Ready,
        State::SystemProxy,
        State::Tun,
    ];
    /// Every mode, `Unknown` last to match the table's column order.
    const MODES: [Mode; 4] = [Mode::Rule, Mode::Global, Mode::Direct, Mode::Unknown];

    fn drawn(fill: [u8; 3], core: Option<[u8; 3]>, size: usize) -> Vec<u8> {
        let mut pixels = vec![0u8; size * size * 4];
        render(fill, core, size, &mut pixels);
        pixels
    }

    /// The BGRA pixel at `(x, y)`.
    fn px(pixels: &[u8], size: usize, x: usize, y: usize) -> (u8, u8, u8, u8) {
        let i = (y * size + x) * 4;
        (pixels[i + 2], pixels[i + 1], pixels[i], pixels[i + 3])
    }

    #[test]
    fn modes_are_parsed_from_what_the_kernel_reports() {
        assert_eq!(Mode::from_str("rule"), Mode::Rule);
        assert_eq!(Mode::from_str("global"), Mode::Global);
        assert_eq!(Mode::from_str("direct"), Mode::Direct);
    }

    #[test]
    fn anything_else_is_an_unknown_mode() {
        // The empty string is what a snapshot holds while the controller is
        // unreachable; the rest guard against a kernel that reports something
        // new, or reports one of the three with unexpected casing or padding.
        for value in ["", " ", "Rule", "GLOBAL", "Direct ", " script", "unknown"] {
            assert_eq!(Mode::from_str(value), Mode::Unknown, "{value:?}");
        }
    }

    #[test]
    fn each_mode_has_its_own_centre_colour() {
        let fill = State::Ready.fill();
        assert_eq!(Mode::Global.core(fill), Some(RED));
        assert_eq!(Mode::Direct.core(fill), Some(NEAR_BLACK));
        assert_eq!(Mode::Unknown.core(fill), None);
    }

    #[test]
    fn rule_keeps_the_original_light_dot() {
        // `rule` is what the tray showed before modes existed: `mix(fill, white,
        // 0.72)`, the exact expression the dot carried when it meant nothing.
        for state in STATES {
            let fill = state.fill();
            let expected = mix(fill, [0xFF, 0xFF, 0xFF], 0.72);
            assert_eq!(Mode::Rule.core(fill), Some(expected), "{state:?}");
            // ...and it is a light dot: every channel moved towards white.
            assert!(expected[0] >= fill[0] && expected[1] >= fill[1] && expected[2] >= fill[2]);
        }
        // Spot-check the ready state, whose light dot is the one users see most.
        assert_eq!(Mode::Rule.core(GREEN), Some([0xCD, 0xE9, 0xCE]));
    }

    #[test]
    fn the_centre_is_the_mode_colour() {
        for size in [16, 20, 24, 32, 64] {
            let center = size / 2;
            for mode in MODES {
                for state in STATES {
                    let fill = state.fill();
                    let pixels = drawn(fill, mode.core(fill), size);
                    let (r, g, b, a) = px(&pixels, size, center, center);
                    let expected = mode.core(fill).unwrap_or(fill);
                    assert_eq!(
                        (r, g, b, a),
                        (expected[0], expected[1], expected[2], 255),
                        "size {size}, {state:?}, {mode:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn an_unknown_mode_draws_no_centre() {
        for state in STATES {
            let with_none = drawn(state.fill(), None, 16);
            let unknown = drawn(state.fill(), Mode::Unknown.core(state.fill()), 16);
            assert_eq!(with_none, unknown);
            // ...and it cuts a hole in the ring only because the ring is meant
            // to be visible there: the centre is the fill colour, not something
            // else that happens to match.
            let (r, g, b, _) = px(&with_none, 16, 8, 8);
            assert_eq!([r, g, b], state.fill());
        }
    }

    #[test]
    fn the_centre_carries_the_mode_on_a_16px_icon() {
        // 0.34R on a 16 px icon is a dot about 2.4 device pixels across. That is
        // the whole budget the three colours get, so measure what it actually
        // covers rather than what it ideally would.
        let pixels = drawn(GRAY, Mode::Global.core(GRAY), 16);
        let coloured: Vec<usize> = (0..16 * 16)
            .filter(|i| {
                let (r, g, b, a) = px(&pixels, 16, i % 16, i / 16);
                a == 255 && [r, g, b] == RED
            })
            .collect();
        assert!(
            coloured.len() >= 9,
            "the dot covers only {} pixels",
            coloured.len()
        );
        // It is a dot, not a blob: the whole covered area stays well inside the
        // ring, leaving the takeover state readable all around it.
        for i in &coloured {
            let (x, y) = (i % 16, i / 16);
            assert!(
                (4..=11).contains(&x) && (4..=11).contains(&y),
                "the dot reached ({x}, {y})"
            );
        }
    }

    #[test]
    fn the_ring_still_says_which_state_the_kernel_is_in() {
        // A mode colour must never replace the ring: the takeover state stays
        // readable on every mode, which is what `docs/DESIGN` documents.
        let size = 16;
        for state in STATES {
            let fill = state.fill();
            let dark = [
                (fill[0] as u16 * 3 / 5) as u8,
                (fill[1] as u16 * 3 / 5) as u8,
                (fill[2] as u16 * 3 / 5) as u8,
            ];
            for mode in MODES {
                let pixels = drawn(fill, mode.core(fill), size);
                // A row through the middle, left of the centre: at x = 3 the
                // pixel is in the bare ring (between the centre dot and the
                // outline); at x = 1 it is the darkened outline itself. Neither
                // may pick up the mode colour.
                let ring = px(&pixels, size, 3, size / 2);
                assert_eq!([ring.0, ring.1, ring.2], fill, "{state:?}/{mode:?}");
                assert_eq!(ring.3, 255, "{state:?}/{mode:?}");
                let outline = px(&pixels, size, 1, size / 2);
                assert_eq!(
                    [outline.0, outline.1, outline.2],
                    dark,
                    "{state:?}/{mode:?}"
                );
            }
        }
    }

    #[test]
    fn the_indices_used_to_build_the_table_round_trip() {
        // `Icons::new` writes through `index_inverse` while `for_state` reads
        // through `index`; if the two ever disagree, a refresh would show the
        // wrong icon instead of failing.
        for (i, mode) in MODES.iter().enumerate() {
            assert_eq!(Mode::index_inverse(i), *mode);
            assert_eq!(mode.index(), i);
        }
        for (i, state) in STATES.iter().enumerate() {
            assert_eq!(State::index_inverse(i), *state);
            assert_eq!(state.index(), i);
        }
    }

    #[test]
    fn every_state_and_mode_pair_draws_a_different_icon() {
        let mut seen = std::collections::BTreeSet::new();
        for state in STATES {
            for mode in MODES {
                let fill = state.fill();
                let pixels = drawn(fill, mode.core(fill), 16);
                assert!(
                    pixels.iter().any(|byte| *byte != 0),
                    "{state:?}/{mode:?} rendered nothing"
                );
                assert!(
                    seen.insert(pixels),
                    "{state:?}/{mode:?} duplicates another combination"
                );
            }
        }
        assert_eq!(seen.len(), 16);
    }

    #[test]
    fn outline_arithmetic_stays_inside_a_byte() {
        // 0xFF * 3 does not fit in a `u8`; this is the regression guard for the
        // outline of the brightest fill.
        let pixels = drawn([0xFF, 0xFF, 0xFF], None, 16);
        let (r, g, b, a) = px(&pixels, 16, 1, 8);
        assert_eq!((r, g, b, a), (0x99, 0x99, 0x99, 255));
    }

    /// Write the actual `render()` output at every size the tray can ask for, as
    /// PNGs, so the icons can be judged at 100/125/150/200% DPI without a build
    /// or a registry change. Opt-in through an environment variable; the default
    /// `cargo test` writes nothing.
    ///
    /// `DSH_TRAY_DUMP=<dir> cargo test --release dumps_the_renders`
    #[test]
    fn dumps_the_renders() {
        let Ok(dir) = std::env::var("DSH_TRAY_DUMP") else {
            return;
        };
        let dir = std::path::PathBuf::from(dir);
        std::fs::create_dir_all(&dir).expect("create the dump directory");
        let mut written = 0;
        for size in [16usize, 20, 24, 32, 48, 64] {
            for state in STATES {
                for mode in MODES {
                    let pixels = drawn(state.fill(), mode.core(state.fill()), size);
                    let name = format!("{:?}_{:?}_{size}.png", state, mode).to_lowercase();
                    std::fs::write(dir.join(name), rgba_png(&pixels, size))
                        .expect("write the dump");
                    written += 1;
                }
            }
        }
        eprintln!("dumped {written} icons into {}", dir.display());
    }

    /// A minimal PNG writer for the dump above: BGRA in, RGBA out, no external
    /// crate and no compression (a stored deflate block), which is fine for the
    /// few hundred small images this writes.
    fn rgba_png(pixels: &[u8], size: usize) -> Vec<u8> {
        let mut raw = Vec::with_capacity(size * (size * 4 + 1));
        for y in 0..size {
            raw.push(0); // filter: none
            for x in 0..size {
                let i = (y * size + x) * 4;
                raw.extend_from_slice(&[pixels[i + 2], pixels[i + 1], pixels[i], pixels[i + 3]]);
            }
        }

        // A zlib stream of one stored block, so no compressor is needed.
        let mut zlib = vec![0x78, 0x01];
        let mut rest = raw.as_slice();
        loop {
            let take = rest.len().min(0xFFFF);
            let last = take == rest.len();
            zlib.push(if last { 1 } else { 0 });
            zlib.extend_from_slice(&(take as u16).to_le_bytes());
            zlib.extend_from_slice(&(!(take as u16)).to_le_bytes());
            zlib.extend_from_slice(&rest[..take]);
            rest = &rest[take..];
            if last {
                break;
            }
        }
        let mut adler_a: u32 = 1;
        let mut adler_b: u32 = 0;
        for byte in &raw {
            adler_a = (adler_a + *byte as u32) % 65521;
            adler_b = (adler_b + adler_a) % 65521;
        }
        zlib.extend_from_slice(&((adler_b << 16) | adler_a).to_be_bytes());

        let mut png = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(&(size as u32).to_be_bytes());
        ihdr.extend_from_slice(&(size as u32).to_be_bytes());
        ihdr.extend_from_slice(&[8, 6, 0, 0, 0]); // 8-bit RGBA
        png.extend_from_slice(&chunk(b"IHDR", &ihdr));
        png.extend_from_slice(&chunk(b"IDAT", &zlib));
        png.extend_from_slice(&chunk(b"IEND", &[]));
        png
    }

    fn chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(data.len() + 12);
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        out.extend_from_slice(kind);
        out.extend_from_slice(data);
        let mut crc = 0xFFFF_FFFFu32;
        for byte in kind.iter().chain(data) {
            crc ^= *byte as u32;
            for _ in 0..8 {
                crc = if crc & 1 != 0 {
                    0xEDB8_8320 ^ (crc >> 1)
                } else {
                    crc >> 1
                };
            }
        }
        out.extend_from_slice(&(crc ^ 0xFFFF_FFFF).to_be_bytes());
        out
    }

    #[test]
    fn icons_are_measured_against_the_current_notification_area() {
        let icons = Icons::new();
        // A fresh set was just measured, so the notification area cannot have
        // another size for it — unless the display changed between the two reads,
        // which no test can arrange.
        assert!(!icons.refresh_size());
        // The size is the one `GetSystemMetrics` reports, clamped to the range the
        // renderer is written for.
        assert!((16..=64).contains(&icons.size()));
        // Every combination has a handle, and they are all distinct.
        let mut seen = std::collections::BTreeSet::new();
        for state in STATES {
            for mode in MODES {
                let icon = icons.for_state(state, mode);
                assert!(!icon.is_null(), "{state:?}/{mode:?} has no handle");
                assert!(seen.insert(icon as usize), "{state:?}/{mode:?} is shared");
            }
        }
        assert_eq!(seen.len(), 16);
    }
}
