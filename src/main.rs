// Release builds have no console window; debug builds keep one for logs.
// A misspelled or unimported Win32 constant in a `match` silently becomes a catch-all binding; make that an error.
#![deny(unreachable_patterns, non_snake_case)]
// Off Windows only the developer tools build; the parts they don't reach are used by `win`.
#![cfg_attr(not(windows), allow(dead_code))]
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app_paths;
mod dev;
mod kit;
mod ocr;
mod render;
mod ui;
#[cfg(windows)]
mod win;

fn main() {
    #[cfg(windows)]
    unsafe {
        use windows::Win32::UI::HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext};
        // Per-Monitor V2 before any window exists; the manifest declares it too.
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
    // Developer flags must come first on the command line; without one the tray app starts.
    if let Some(code) = dev::run_if_requested() {
        std::process::exit(code);
    }
    #[cfg(windows)]
    std::process::exit(win::app::run());
    #[cfg(not(windows))]
    {
        eprintln!("Shotlate for Windows runs on Windows. Here only the developer tools work, e.g. --ui-demo out/");
        std::process::exit(2);
    }
}
