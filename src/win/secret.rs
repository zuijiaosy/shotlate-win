//! The translation API key, encrypted with DPAPI for the current Windows user, in %APPDATA%\Shotlate\api-key.
//! A debug build reads DEEPSEEK_API_KEY instead and never touches the real file, like the macOS dev build.

use windows::Win32::Foundation::{HLOCAL, LocalFree};
use windows::Win32::Security::Cryptography::{CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData, CryptUnprotectData};
use windows::core::PCWSTR;

use crate::app_paths;

fn is_dev_build() -> bool {
    cfg!(debug_assertions)
}

pub fn api_key() -> String {
    if is_dev_build() {
        return std::env::var("DEEPSEEK_API_KEY").unwrap_or_default();
    }
    std::fs::read(app_paths::api_key_file()).ok().and_then(|b| unprotect(&b)).map(|s| s.trim().to_string()).unwrap_or_default()
}

/// An empty key removes the file.
pub fn set_api_key(key: &str) {
    if is_dev_build() {
        return;
    }
    let path = app_paths::api_key_file();
    let key = key.trim();
    if key.is_empty() {
        let _ = std::fs::remove_file(path);
        return;
    }
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Some(bytes) = protect(key.as_bytes()) {
        let _ = std::fs::write(path, bytes);
    }
}

fn blob(data: &[u8]) -> CRYPT_INTEGER_BLOB {
    CRYPT_INTEGER_BLOB { cbData: data.len() as u32, pbData: data.as_ptr() as *mut u8 }
}

fn take(out: CRYPT_INTEGER_BLOB) -> Vec<u8> {
    let v = unsafe { std::slice::from_raw_parts(out.pbData, out.cbData as usize).to_vec() };
    unsafe {
        let _ = LocalFree(Some(HLOCAL(out.pbData as *mut _)));
    }
    v
}

fn protect(data: &[u8]) -> Option<Vec<u8>> {
    let input = blob(data);
    let mut out = CRYPT_INTEGER_BLOB::default();
    unsafe { CryptProtectData(&input, PCWSTR::null(), None, None, None, CRYPTPROTECT_UI_FORBIDDEN, &mut out).ok()? };
    Some(take(out))
}

fn unprotect(data: &[u8]) -> Option<String> {
    let input = blob(data);
    let mut out = CRYPT_INTEGER_BLOB::default();
    unsafe { CryptUnprotectData(&input, None, None, None, None, CRYPTPROTECT_UI_FORBIDDEN, &mut out).ok()? };
    String::from_utf8(take(out)).ok()
}

/// For the self-check: DPAPI protects and unprotects `sample` for this user.
pub fn roundtrip_check(sample: &str) -> bool {
    protect(sample.as_bytes()).and_then(|b| unprotect(&b)).as_deref() == Some(sample)
}
