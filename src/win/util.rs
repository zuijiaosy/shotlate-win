//! Small Win32 helpers: wide strings, DPI, pixel conversion, the UI-thread task queue.

use std::collections::VecDeque;
use std::sync::Mutex;

use tiny_skia::Pixmap;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::Graphics::Gdi::{BI_RGB, BITMAPINFO, BITMAPINFOHEADER, DIB_RGB_COLORS, HDC, SetDIBitsToDevice};
use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};
use windows::core::{PCWSTR, w};

pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

pub fn pcwstr(v: &[u16]) -> PCWSTR {
    PCWSTR(v.as_ptr())
}

pub fn from_wide(v: &[u16]) -> String {
    let end = v.iter().position(|c| *c == 0).unwrap_or(v.len());
    String::from_utf16_lossy(&v[..end])
}

/// Byte offset in `s` of UTF-16 index `u16_index` (EDIT controls count in UTF-16 units).
pub fn utf16_to_byte(s: &str, u16_index: usize) -> usize {
    let mut units = 0;
    for (byte, c) in s.char_indices() {
        if units >= u16_index {
            return byte;
        }
        units += c.len_utf16();
    }
    s.len()
}

pub fn loword(v: usize) -> u16 {
    (v & 0xFFFF) as u16
}

pub fn hiword(v: usize) -> u16 {
    ((v >> 16) & 0xFFFF) as u16
}

/// Signed client coordinates from a mouse message's LPARAM.
pub fn lparam_point(l: LPARAM) -> (i32, i32) {
    let v = l.0 as usize;
    (loword(v) as i16 as i32, hiword(v) as i16 as i32)
}

/// Whether the apps use the dark theme (Settings › Personalization › Colors).
pub fn is_dark_mode() -> bool {
    let mut value: u32 = 1;
    let mut size = std::mem::size_of::<u32>() as u32;
    let r = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"),
            w!("AppsUseLightTheme"),
            RRF_RT_REG_DWORD,
            None,
            Some(&mut value as *mut u32 as *mut _),
            Some(&mut size),
        )
    };
    r.is_ok() && value == 0
}

/// Draws a tiny-skia pixmap (premultiplied RGBA) at `x, y` of `hdc`. Opaque content only.
pub fn blit(hdc: HDC, x: i32, y: i32, pix: &Pixmap) {
    let (w, h) = (pix.width() as i32, pix.height() as i32);
    let mut bgra = pix.data().to_vec();
    for p in bgra.chunks_exact_mut(4) {
        p.swap(0, 2);
    }
    let info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w,
            // Negative height: rows top to bottom.
            biHeight: -h,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    unsafe {
        SetDIBitsToDevice(hdc, x, y, w as u32, h as u32, 0, 0, 0, h as u32, bgra.as_ptr() as *const _, &info, DIB_RGB_COLORS);
    }
}

// MARK: Tasks for the UI thread

/// Posted to the app window whenever a task is queued.
pub const WM_APP_TASK: u32 = WM_APP + 1;

type Task = Box<dyn FnOnce() + Send>;

static TASKS: Mutex<VecDeque<Task>> = Mutex::new(VecDeque::new());
static APP_HWND: Mutex<isize> = Mutex::new(0);

pub fn set_app_window(hwnd: HWND) {
    *APP_HWND.lock().unwrap_or_else(|e| e.into_inner()) = hwnd.0 as isize;
}

pub fn app_window() -> HWND {
    HWND(*APP_HWND.lock().unwrap_or_else(|e| e.into_inner()) as *mut _)
}

/// Runs `task` on the UI thread (from any thread), in the order queued.
pub fn run_on_ui(task: impl FnOnce() + Send + 'static) {
    TASKS.lock().unwrap_or_else(|e| e.into_inner()).push_back(Box::new(task));
    let hwnd = app_window();
    unsafe {
        let _ = PostMessageW(Some(hwnd), WM_APP_TASK, WPARAM(0), LPARAM(0));
    }
}

/// Runs the queued tasks; called by the app window on WM_APP_TASK.
pub fn drain_tasks() {
    loop {
        let task = TASKS.lock().unwrap_or_else(|e| e.into_inner()).pop_front();
        match task {
            Some(t) => t(),
            None => break,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf16_offsets() {
        let s = "a你😀b";
        assert_eq!(utf16_to_byte(s, 0), 0);
        assert_eq!(utf16_to_byte(s, 1), 1);
        assert_eq!(utf16_to_byte(s, 2), 4);
        assert_eq!(utf16_to_byte(s, 4), 8);
        assert_eq!(utf16_to_byte(s, 5), 9);
        assert_eq!(utf16_to_byte(s, 99), 9);
    }
}

/// Runs a background job; a panic inside it becomes an error message instead of a dead worker (or app).
pub fn guarded<T>(job: impl FnOnce() -> T + std::panic::UnwindSafe) -> Result<T, String> {
    std::panic::catch_unwind(job).map_err(|p| {
        let what = p.downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| p.downcast_ref::<String>().cloned()).unwrap_or_default();
        format!("内部错误：{what}")
    })
}

/// Writes panics to %APPDATA%\Shotlate\crash.log, so a crash report can say what happened.
pub fn install_crash_log() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let path = crate::app_paths::config_dir().join("crash.log");
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let backtrace = std::backtrace::Backtrace::force_capture();
        let line = format!("Shotlate {} (thread {:?}): {info}\n{backtrace}\n\n", env!("CARGO_PKG_VERSION"), std::thread::current().name());
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
            use std::io::Write as _;
            let _ = f.write_all(line.as_bytes());
        }
        default(info);
    }));
}

/// Appends a line to %TEMP%\shotlate-trace.log when SHOTLATE_TRACE is set (debugging on a test machine).
pub fn trace(line: impl FnOnce() -> String) {
    if std::env::var_os("SHOTLATE_TRACE").is_none() {
        return;
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(std::env::temp_dir().join("shotlate-trace.log")) {
        use std::io::Write as _;
        let _ = writeln!(f, "{}", line());
    }
}
