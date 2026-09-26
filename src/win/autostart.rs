//! "登录时启动": a value under HKCU\Software\Microsoft\Windows\CurrentVersion\Run.

use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_READ, KEY_SET_VALUE, REG_SZ, RRF_RT_REG_SZ, RegCloseKey, RegDeleteValueW, RegGetValueW, RegOpenKeyExW,
    RegSetValueExW,
};
use windows::core::w;

use super::util::wide;

const RUN: windows::core::PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
const NAME: windows::core::PCWSTR = w!("Shotlate");

pub fn is_enabled() -> bool {
    unsafe { RegGetValueW(HKEY_CURRENT_USER, RUN, NAME, RRF_RT_REG_SZ, None, None, None).is_ok() }
}

pub fn set_enabled(on: bool) -> Result<(), String> {
    // Before opening the key, so an error here doesn't leak it.
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let mut key = HKEY::default();
    let access = KEY_SET_VALUE | KEY_READ;
    let r = unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, RUN, None, access, &mut key) };
    if r.is_err() {
        return Err(format!("无法打开启动项注册表（{}）", r.0));
    }
    let result = if on {
        let value = wide(&format!("\"{}\"", exe.display()));
        let bytes = unsafe { std::slice::from_raw_parts(value.as_ptr() as *const u8, value.len() * 2) };
        let r = unsafe { RegSetValueExW(key, NAME, None, REG_SZ, Some(bytes)) };
        if r.is_ok() { Ok(()) } else { Err(format!("写入启动项失败（{}）", r.0)) }
    } else {
        let r = unsafe { RegDeleteValueW(key, NAME) };
        if r.is_ok() || r.0 == 2 { Ok(()) } else { Err(format!("删除启动项失败（{}）", r.0)) }
    };
    unsafe {
        let _ = RegCloseKey(key);
    }
    result
}
