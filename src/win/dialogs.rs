//! Standard dialogs: Save As, folder picker, color picker, message boxes.

use std::path::PathBuf;

use windows::Win32::Foundation::{COLORREF, HWND};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, CoTaskMemFree};
use windows::Win32::UI::Controls::Dialogs::{CC_FULLOPEN, CC_RGBINIT, CHOOSECOLORW, ChooseColorW};
use windows::Win32::UI::Shell::Common::COMDLG_FILTERSPEC;
use windows::Win32::UI::Shell::{
    FOS_FORCEFILESYSTEM, FOS_OVERWRITEPROMPT, FOS_PICKFOLDERS, FileOpenDialog, FileSaveDialog, IFileOpenDialog, IFileSaveDialog, IShellItem,
    SHCreateItemFromParsingName, SIGDN_FILESYSPATH,
};
use windows::Win32::UI::WindowsAndMessaging::{
    IDYES, MB_ICONINFORMATION, MB_ICONWARNING, MB_OK, MB_SETFOREGROUND, MB_TOPMOST, MB_YESNO, MessageBoxW,
};

use super::util::{pcwstr, wide};
use crate::kit::color::Color;
use crate::kit::settings::ImageFormat;

fn item_path(item: &IShellItem) -> Option<PathBuf> {
    unsafe {
        let p = item.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
        let s = p.to_string().ok();
        CoTaskMemFree(Some(p.0 as *const _));
        s.map(PathBuf::from)
    }
}

/// The Save As dialog, starting in `directory` with `file_name`. Returns the chosen path.
pub fn save_as(owner: Option<HWND>, directory: &std::path::Path, file_name: &str, format: ImageFormat) -> Option<PathBuf> {
    unsafe {
        let dialog: IFileSaveDialog = CoCreateInstance(&FileSaveDialog, None, CLSCTX_INPROC_SERVER).ok()?;
        let (name, spec) = match format {
            ImageFormat::Png => (wide("PNG 图片"), wide("*.png")),
            ImageFormat::Jpeg => (wide("JPG 图片"), wide("*.jpg;*.jpeg")),
        };
        let filters = [COMDLG_FILTERSPEC { pszName: pcwstr(&name), pszSpec: pcwstr(&spec) }];
        let _ = dialog.SetFileTypes(&filters);
        let ext = wide(format.extension());
        let _ = dialog.SetDefaultExtension(pcwstr(&ext));
        let file = wide(file_name);
        let _ = dialog.SetFileName(pcwstr(&file));
        let _ = dialog.SetOptions(FOS_OVERWRITEPROMPT | FOS_FORCEFILESYSTEM);
        let _ = std::fs::create_dir_all(directory);
        let dir = wide(&directory.display().to_string());
        if let Ok(folder) = SHCreateItemFromParsingName::<_, _, IShellItem>(pcwstr(&dir), None) {
            let _ = dialog.SetFolder(&folder);
        }
        dialog.Show(owner).ok()?;
        item_path(&dialog.GetResult().ok()?)
    }
}

/// A folder picker starting at `start`.
pub fn pick_folder(owner: Option<HWND>, start: &std::path::Path) -> Option<PathBuf> {
    unsafe {
        let dialog: IFileOpenDialog = CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER).ok()?;
        let _ = dialog.SetOptions(FOS_PICKFOLDERS | FOS_FORCEFILESYSTEM);
        let dir = wide(&start.display().to_string());
        if let Ok(folder) = SHCreateItemFromParsingName::<_, _, IShellItem>(pcwstr(&dir), None) {
            let _ = dialog.SetFolder(&folder);
        }
        dialog.Show(owner).ok()?;
        item_path(&dialog.GetResult().ok()?)
    }
}

thread_local! {
    static CUSTOM_COLORS: std::cell::RefCell<[COLORREF; 16]> = const { std::cell::RefCell::new([COLORREF(0x00FF_FFFF); 16]) };
}

/// The system color picker, starting at `current`.
pub fn pick_color(owner: HWND, current: Color) -> Option<Color> {
    let (r, g, b) = current.to_rgb8();
    // A copy, not a borrow held across the dialog's modal loop (which dispatches our messages).
    let mut custom = CUSTOM_COLORS.with(|c| *c.borrow());
    let mut cc = CHOOSECOLORW {
        lStructSize: std::mem::size_of::<CHOOSECOLORW>() as u32,
        hwndOwner: owner,
        rgbResult: COLORREF(r as u32 | (g as u32) << 8 | (b as u32) << 16),
        lpCustColors: custom.as_mut_ptr(),
        Flags: CC_FULLOPEN | CC_RGBINIT,
        ..Default::default()
    };
    let ok = unsafe { ChooseColorW(&mut cc) }.as_bool();
    CUSTOM_COLORS.with(|c| *c.borrow_mut() = custom);
    ok.then(|| {
        let v = cc.rgbResult.0;
        Color::from_rgb8((v & 0xFF) as u8, ((v >> 8) & 0xFF) as u8, ((v >> 16) & 0xFF) as u8)
    })
}

pub fn warn(owner: Option<HWND>, title: &str, text: &str) {
    let (t, m) = (wide(title), wide(text));
    unsafe {
        MessageBoxW(owner, pcwstr(&m), pcwstr(&t), MB_OK | MB_ICONWARNING | MB_SETFOREGROUND | MB_TOPMOST);
    }
}

pub fn ask(owner: Option<HWND>, title: &str, text: &str) -> bool {
    let (t, m) = (wide(title), wide(text));
    unsafe { MessageBoxW(owner, pcwstr(&m), pcwstr(&t), MB_YESNO | MB_ICONINFORMATION | MB_SETFOREGROUND | MB_TOPMOST) == IDYES }
}
