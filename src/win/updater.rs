//! Automatic updates through WinSparkle (the Windows sibling of Sparkle), loaded from WinSparkle.dll next to
//! the exe. The installer ships the DLL; development builds have none, and updating is then simply off.
//! Updates are verified with the same EdDSA public key as the macOS version's Sparkle feed.

use std::ffi::{CString, c_char, c_int};
use std::sync::OnceLock;

use windows::Win32::Foundation::HMODULE;
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows::core::{PCSTR, PCWSTR};

use super::util::wide;

pub const APPCAST_URL: &str = "https://github.com/zuijiaosy/shotlate-win/releases/latest/download/appcast.xml";
/// Same key as SUPublicEDKey in the macOS Info.plist.
pub const EDDSA_PUBLIC_KEY: &str = "RkEtvfkFSA2c5E/7aSKK3a8TgSGHQQcBdpE8IPBYvpg=";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

struct Sparkle {
    init: unsafe extern "C" fn(),
    cleanup: unsafe extern "C" fn(),
    check_with_ui: unsafe extern "C" fn(),
    set_automatic: unsafe extern "C" fn(c_int),
    get_automatic: unsafe extern "C" fn() -> c_int,
}

static SPARKLE: OnceLock<Option<Sparkle>> = OnceLock::new();

unsafe fn symbol<T>(lib: HMODULE, name: &str) -> Option<T> {
    let c = CString::new(name).ok()?;
    let f = unsafe { GetProcAddress(lib, PCSTR(c.as_ptr() as *const u8)) }?;
    Some(unsafe { std::mem::transmute_copy(&f) })
}

extern "C" fn shutdown_request() {
    // WinSparkle calls this from its own thread before running the installer.
    super::util::run_on_ui(super::app::quit);
}

fn load() -> Option<Sparkle> {
    let dir = std::env::current_exe().ok()?.parent()?.join("WinSparkle.dll");
    if !dir.exists() {
        return None;
    }
    let path = wide(&dir.display().to_string());
    unsafe {
        let lib = LoadLibraryW(PCWSTR(path.as_ptr())).ok()?;
        let set_url: unsafe extern "C" fn(*const c_char) = symbol(lib, "win_sparkle_set_appcast_url")?;
        let set_key: unsafe extern "C" fn(*const c_char) -> c_int = symbol(lib, "win_sparkle_set_eddsa_public_key")?;
        let set_details: unsafe extern "C" fn(*const u16, *const u16, *const u16) = symbol(lib, "win_sparkle_set_app_details")?;
        let set_shutdown: Option<unsafe extern "C" fn(extern "C" fn())> = symbol(lib, "win_sparkle_set_shutdown_request_callback");
        let url = CString::new(APPCAST_URL).ok()?;
        let key = CString::new(EDDSA_PUBLIC_KEY).ok()?;
        set_url(url.as_ptr());
        if set_key(key.as_ptr()) == 0 {
            return None;
        }
        let (company, app, version) = (wide("Shotlate"), wide("Shotlate"), wide(VERSION));
        set_details(company.as_ptr(), app.as_ptr(), version.as_ptr());
        if let Some(f) = set_shutdown {
            f(shutdown_request);
        }
        Some(Sparkle {
            init: symbol(lib, "win_sparkle_init")?,
            cleanup: symbol(lib, "win_sparkle_cleanup")?,
            check_with_ui: symbol(lib, "win_sparkle_check_update_with_ui")?,
            set_automatic: symbol(lib, "win_sparkle_set_automatic_check_for_updates")?,
            get_automatic: symbol(lib, "win_sparkle_get_automatic_check_for_updates")?,
        })
    }
}

fn sparkle() -> Option<&'static Sparkle> {
    SPARKLE.get_or_init(load).as_ref()
}

pub fn start() {
    if let Some(s) = sparkle() {
        unsafe { (s.init)() };
    }
}

pub fn is_available() -> bool {
    sparkle().is_some()
}

pub fn check_with_ui() {
    if let Some(s) = sparkle() {
        unsafe { (s.check_with_ui)() };
    }
}

pub fn automatic() -> bool {
    sparkle().is_some_and(|s| unsafe { (s.get_automatic)() } != 0)
}

pub fn set_automatic(on: bool) {
    if let Some(s) = sparkle() {
        unsafe { (s.set_automatic)(on as c_int) };
    }
}

pub fn cleanup() {
    if let Some(Some(s)) = SPARKLE.get() {
        unsafe { (s.cleanup)() };
    }
}
