//! Monitors, frozen screenshots (GDI BitBlt, no yellow border or permission prompt) and the windows on screen.
//! The process is Per-Monitor V2 DPI aware, so every coordinate here is in physical pixels.

use tiny_skia::Pixmap;
use windows::Win32::Foundation::{HWND, LPARAM, RECT};
use windows::Win32::Graphics::Dwm::{DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BitBlt, CAPTUREBLT, CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS, DeleteDC, DeleteObject,
    EnumDisplayMonitors, GetDC, GetMonitorInfoW, HDC, HGDIOBJ, HMONITOR, MONITORINFOEXW, ReleaseDC, SRCCOPY, SelectObject,
};
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetClassNameW, GetWindowLongW, GetWindowThreadProcessId, GWL_EXSTYLE, IsIconic, IsWindowVisible, WS_EX_TOOLWINDOW,
    WS_EX_TRANSPARENT,
};
use windows::core::BOOL;

use super::util::from_wide;

#[derive(Clone, Debug)]
pub struct Monitor {
    /// Device name, e.g. `\\.\DISPLAY1`; identifies the monitor for "restore last selection".
    pub name: String,
    pub rect: RECT,
    /// Pixels per point (DPI / 96).
    pub scale: f32,
}

pub fn monitors() -> Vec<Monitor> {
    unsafe extern "system" fn each(hmon: HMONITOR, _: HDC, _: *mut RECT, data: LPARAM) -> BOOL {
        let list = unsafe { &mut *(data.0 as *mut Vec<Monitor>) };
        let mut info = MONITORINFOEXW::default();
        info.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
        if unsafe { GetMonitorInfoW(hmon, &mut info.monitorInfo) }.as_bool() {
            let (mut dx, mut dy) = (96u32, 96u32);
            let _ = unsafe { GetDpiForMonitor(hmon, MDT_EFFECTIVE_DPI, &mut dx, &mut dy) };
            list.push(Monitor { name: from_wide(&info.szDevice), rect: info.monitorInfo.rcMonitor, scale: dx.max(96) as f32 / 96.0 });
        }
        BOOL(1)
    }
    let mut list: Vec<Monitor> = Vec::new();
    unsafe {
        let _ = EnumDisplayMonitors(None, None, Some(each), LPARAM(&mut list as *mut _ as isize));
    }
    list
}

/// The pixels of `r` (screen coordinates) as an opaque premultiplied pixmap.
pub fn grab(r: &RECT) -> Option<Pixmap> {
    let (w, h) = (r.right - r.left, r.bottom - r.top);
    if w <= 0 || h <= 0 {
        return None;
    }
    unsafe {
        let screen = GetDC(None);
        let mem = CreateCompatibleDC(Some(screen));
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -h,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
        let result = match CreateDIBSection(Some(mem), &info, DIB_RGB_COLORS, &mut bits, None, 0) {
            Ok(bmp) if !bits.is_null() => {
                let old = SelectObject(mem, HGDIOBJ(bmp.0));
                // CAPTUREBLT includes layered windows (tooltips, menus) in the copy.
                let ok = BitBlt(mem, 0, 0, w, h, Some(screen), r.left, r.top, SRCCOPY | CAPTUREBLT).is_ok();
                let pix = if ok {
                    let src = std::slice::from_raw_parts(bits as *const u8, (w * h * 4) as usize);
                    let mut pix = Pixmap::new(w as u32, h as u32);
                    if let Some(p) = &mut pix {
                        let d = p.data_mut();
                        for (i, px) in src.chunks_exact(4).enumerate() {
                            let o = i * 4;
                            d[o] = px[2];
                            d[o + 1] = px[1];
                            d[o + 2] = px[0];
                            d[o + 3] = 255;
                        }
                    }
                    pix
                } else {
                    None
                };
                SelectObject(mem, old);
                let _ = DeleteObject(HGDIOBJ(bmp.0));
                pix
            }
            _ => None,
        };
        let _ = DeleteDC(mem);
        ReleaseDC(None, screen);
        result
    }
}

/// Visible top-level windows front to back, as their visible frames (without the invisible resize border).
/// Our own windows, the desktop and cloaked (other virtual desktop, suspended UWP) windows are skipped.
pub fn window_rects() -> Vec<RECT> {
    unsafe extern "system" fn each(hwnd: HWND, data: LPARAM) -> BOOL {
        let list = unsafe { &mut *(data.0 as *mut Vec<RECT>) };
        unsafe {
            if !IsWindowVisible(hwnd).as_bool() || IsIconic(hwnd).as_bool() {
                return BOOL(1);
            }
            let ex = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
            if ex & WS_EX_TRANSPARENT.0 != 0 && ex & WS_EX_TOOLWINDOW.0 != 0 {
                return BOOL(1);
            }
            let mut pid = 0u32;
            GetWindowThreadProcessId(hwnd, Some(&mut pid));
            if pid == std::process::id() {
                return BOOL(1);
            }
            let mut cloaked = 0u32;
            if DwmGetWindowAttribute(hwnd, DWMWA_CLOAKED, &mut cloaked as *mut _ as *mut _, 4).is_ok() && cloaked != 0 {
                return BOOL(1);
            }
            let mut class = [0u16; 64];
            let n = GetClassNameW(hwnd, &mut class) as usize;
            let class = String::from_utf16_lossy(&class[..n]);
            if class == "Progman" || class == "WorkerW" {
                return BOOL(1);
            }
            let mut r = RECT::default();
            if DwmGetWindowAttribute(hwnd, DWMWA_EXTENDED_FRAME_BOUNDS, &mut r as *mut _ as *mut _, std::mem::size_of::<RECT>() as u32).is_err() {
                return BOOL(1);
            }
            if r.right - r.left >= 20 && r.bottom - r.top >= 20 {
                list.push(r);
            }
        }
        BOOL(1)
    }
    let mut list: Vec<RECT> = Vec::new();
    unsafe {
        let _ = EnumWindows(Some(each), LPARAM(&mut list as *mut _ as isize));
    }
    list
}
