//! Window transparency and blur on Windows.
//!
//! Uses the (undocumented but stable since Windows 10) user32 `SetWindowCompositionAttribute`,
//! the same API Tabby and most "acrylic" Electron apps use. It blurs whatever is behind the
//! window and stays see-through, unlike the Windows 11 system backdrop which is often drawn
//! opaque for custom frameless windows.
//!
//! Important: we never set the DWM system backdrop to "none" for the glass modes. On Windows 11
//! that switches off per-pixel transparency and DWM fills the window with its default (light)
//! background colour.
#![allow(non_snake_case, dead_code)]
use std::ffi::c_void;

pub const DISABLED: u32 = 0;
pub const BLUR: u32 = 3;
pub const ACRYLIC: u32 = 4;

#[repr(C)]
struct AccentPolicy {
    state: u32,
    flags: u32,
    color: u32,
    animation: u32,
}

#[repr(C)]
struct CompositionData {
    attrib: u32,
    data: *mut c_void,
    size: usize,
}

#[repr(C)]
struct DwmBlurBehind {
    flags: u32,
    enable: i32,
    region: isize,
    transition_on_maximized: i32,
}

type SetWindowCompositionAttribute = unsafe extern "system" fn(isize, *mut CompositionData) -> i32;

#[link(name = "kernel32")]
extern "system" {
    fn GetModuleHandleA(name: *const u8) -> isize;
    fn LoadLibraryA(name: *const u8) -> isize;
    fn GetProcAddress(module: isize, name: *const u8) -> *const c_void;
}

#[link(name = "dwmapi")]
extern "system" {
    fn DwmSetWindowAttribute(hwnd: isize, attr: u32, data: *const c_void, size: u32) -> i32;
    fn DwmEnableBlurBehindWindow(hwnd: isize, blur: *const DwmBlurBehind) -> i32;
}

#[link(name = "gdi32")]
extern "system" {
    fn CreateRectRgn(x1: i32, y1: i32, x2: i32, y2: i32) -> isize;
    fn DeleteObject(obj: isize) -> i32;
}

const DWMWA_USE_IMMERSIVE_DARK_MODE: u32 = 20;
const DWMWA_SYSTEMBACKDROP_TYPE: u32 = 38;
const DWMSBT_AUTO: u32 = 0;

/// `color` is 0xAABBGGRR. Acrylic needs a non-zero alpha or Windows ignores it.
pub fn set(hwnd: isize, state: u32, color: u32) -> bool {
    unsafe {
        let mut module = GetModuleHandleA(b"user32.dll\0".as_ptr());
        if module == 0 {
            module = LoadLibraryA(b"user32.dll\0".as_ptr());
        }
        if module == 0 {
            return false;
        }
        let f = GetProcAddress(module, b"SetWindowCompositionAttribute\0".as_ptr());
        if f.is_null() {
            return false;
        }
        let f: SetWindowCompositionAttribute = std::mem::transmute(f);
        let mut policy = AccentPolicy {
            state,
            flags: if state == ACRYLIC { 0 } else { 2 },
            color,
            animation: 0,
        };
        let mut data = CompositionData {
            attrib: 0x13, // WCA_ACCENT_POLICY
            data: &mut policy as *mut _ as *mut c_void,
            size: std::mem::size_of::<AccentPolicy>(),
        };
        f(hwnd, &mut data) != 0
    }
}

/// Dark window frame, so any fallback Windows draws is dark rather than white.
pub fn dark_mode(hwnd: isize) {
    let on: i32 = 1;
    unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE,
            &on as *const _ as *const c_void,
            4,
        );
    }
}

/// Removes Mica / system backdrop without disabling transparency (AUTO, not NONE).
pub fn reset_system_backdrop(hwnd: isize) {
    let v: u32 = DWMSBT_AUTO;
    unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_SYSTEMBACKDROP_TYPE,
            &v as *const _ as *const c_void,
            4,
        );
    }
}

/// Makes the window per-pixel transparent (what tao does when the window is created),
/// so the parts the page leaves transparent really show the desktop.
pub fn enable_transparency(hwnd: isize) {
    unsafe {
        let region = CreateRectRgn(0, 0, -1, -1);
        let bb = DwmBlurBehind {
            flags: 0x1 | 0x2, // DWM_BB_ENABLE | DWM_BB_BLURREGION
            enable: 1,
            region,
            transition_on_maximized: 0,
        };
        DwmEnableBlurBehindWindow(hwnd, &bb);
        DeleteObject(region);
    }
}

/// Applies one of "none" | "blur" | "acrylic" (glass modes). Mica is handled by Tauri.
/// `kind`: "none" = plain see-through, "acrylic" (or legacy "blur") = blur behind the window.
pub fn apply_glass(hwnd: isize, kind: &str) {
    dark_mode(hwnd);
    reset_system_backdrop(hwnd);
    set(hwnd, DISABLED, 0);
    enable_transparency(hwnd);
    if kind == "acrylic" || kind == "blur" {
        // Windows 11: the system's own acrylic backdrop. (The older SetWindowCompositionAttribute
        // blur turns this window opaque on current Windows 11 builds.)
        let transient: u32 = DWMSBT_TRANSIENTWINDOW;
        let hr = unsafe {
            DwmSetWindowAttribute(
                hwnd,
                DWMWA_SYSTEMBACKDROP_TYPE,
                &transient as *const _ as *const c_void,
                4,
            )
        };
        if hr != 0 {
            // Windows 10 / early Windows 11: no system backdrop, use the older acrylic
            set(hwnd, ACRYLIC, 0x011f1412);
        }
    }
}

const DWMSBT_TRANSIENTWINDOW: u32 = 3;

const DWMWA_WINDOW_CORNER_PREFERENCE: u32 = 33;
const DWMWA_BORDER_COLOR: u32 = 34;

/// Rounded Windows 11 corners and no 1px system border (Windows 11 22H2+, ignored elsewhere).
pub fn window_frame(hwnd: isize) {
    let round: u32 = 2; // DWMWCP_ROUND
    let no_border: u32 = 0xFFFF_FFFE; // DWMWA_COLOR_NONE
    unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            &round as *const _ as *const c_void,
            4,
        );
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_BORDER_COLOR,
            &no_border as *const _ as *const c_void,
            4,
        );
    }
}
