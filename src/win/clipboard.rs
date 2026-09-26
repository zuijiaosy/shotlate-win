//! Clipboard: images as CF_DIB (what WeChat, QQ and Office read) plus "PNG" (what browsers and editors prefer),
//! and plain text.

use windows::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL, HWND};
use windows::Win32::Graphics::Gdi::{BI_RGB, BITMAPINFOHEADER};
use windows::Win32::System::DataExchange::{CloseClipboard, EmptyClipboard, OpenClipboard, RegisterClipboardFormatW, SetClipboardData};
use windows::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};
use windows::Win32::System::Ole::{CF_DIB, CF_UNICODETEXT};
use windows::core::w;

use crate::kit::image::RgbaImage;

/// Opens the clipboard, retrying briefly: another app may hold it for a moment.
pub fn open(owner: Option<HWND>) -> bool {
    for _ in 0..10 {
        if unsafe { OpenClipboard(owner) }.is_ok() {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    false
}

fn global(bytes: &[u8]) -> Option<HGLOBAL> {
    unsafe {
        let h = GlobalAlloc(GMEM_MOVEABLE, bytes.len()).ok()?;
        let p = GlobalLock(h) as *mut u8;
        if p.is_null() {
            let _ = GlobalFree(Some(h));
            return None;
        }
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), p, bytes.len());
        let _ = GlobalUnlock(h);
        Some(h)
    }
}

fn put(format: u32, bytes: &[u8]) -> bool {
    let Some(h) = global(bytes) else { return false };
    // On success the clipboard owns the memory; otherwise free it.
    if unsafe { SetClipboardData(format, Some(HANDLE(h.0))) }.is_err() {
        unsafe {
            let _ = GlobalFree(Some(h));
        }
        return false;
    }
    true
}

/// A packed DIB: BITMAPINFOHEADER + 32-bit BGRA rows, bottom-up.
fn dib(img: &RgbaImage) -> Vec<u8> {
    let header = BITMAPINFOHEADER {
        biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
        biWidth: img.width as i32,
        biHeight: img.height as i32,
        biPlanes: 1,
        biBitCount: 32,
        biCompression: BI_RGB.0,
        biSizeImage: img.width * img.height * 4,
        ..Default::default()
    };
    let mut out = Vec::with_capacity(std::mem::size_of::<BITMAPINFOHEADER>() + img.data.len());
    out.extend_from_slice(unsafe { std::slice::from_raw_parts(&header as *const _ as *const u8, std::mem::size_of::<BITMAPINFOHEADER>()) });
    let stride = img.width as usize * 4;
    for row in (0..img.height as usize).rev() {
        for p in img.data[row * stride..(row + 1) * stride].chunks_exact(4) {
            out.extend_from_slice(&[p[2], p[1], p[0], 255]);
        }
    }
    out
}

pub fn set_image(owner: HWND, img: &RgbaImage) -> bool {
    let png = img.encode_png().ok();
    let dib = dib(img);
    if !open(Some(owner)) {
        return false;
    }
    let ok = unsafe {
        let _ = EmptyClipboard();
        let mut ok = put(CF_DIB.0 as u32, &dib);
        if let Some(png) = png {
            let format = RegisterClipboardFormatW(w!("PNG"));
            if format != 0 {
                ok |= put(format, &png);
            }
        }
        ok
    };
    unsafe {
        let _ = CloseClipboard();
    }
    ok
}

pub fn set_text(owner: HWND, text: &str) -> bool {
    let mut units: Vec<u16> = text.replace("\r\n", "\n").replace('\n', "\r\n").encode_utf16().collect();
    units.push(0);
    let bytes = unsafe { std::slice::from_raw_parts(units.as_ptr() as *const u8, units.len() * 2) };
    if !open(Some(owner)) {
        return false;
    }
    let ok = unsafe {
        let _ = EmptyClipboard();
        put(CF_UNICODETEXT.0 as u32, bytes)
    };
    unsafe {
        let _ = CloseClipboard();
    }
    ok
}
