//! `--e2e <out-dir>`: an end-to-end run against the real app in the interactive session, driven with SendInput
//! like a person would: hotkey, drag, keys, Chinese input, copy, save, OCR, pin, settings. Every step is logged
//! as PASS / FAIL / INFO and screenshotted into `out-dir`, so a run inside a VM can be reviewed from the host.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Gdi::{BeginPaint, EndPaint, PAINTSTRUCT};
use windows::Win32::System::DataExchange::{CloseClipboard, GetClipboardData, IsClipboardFormatAvailable};
use windows::Win32::System::Memory::{GlobalLock, GlobalUnlock, GlobalSize};
use windows::Win32::System::Ole::CF_DIB;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    MAPVK_VK_TO_VSC, MapVirtualKeyW,
    INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT, KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP, MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_LEFTDOWN,
    MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MOVE, MOUSEINPUT, SendInput, VIRTUAL_KEY, VK_CONTROL, VK_ESCAPE, VK_LWIN, VK_MENU, VK_RETURN, VK_SHIFT, VK_SPACE,
};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{PCWSTR, w};

use super::{screen, util};

struct Run {
    out: PathBuf,
    /// Recorded demo frames and how long each stays on screen (seconds).
    frames: Vec<(String, f32)>,
    log: std::fs::File,
    failed: usize,
    width: i32,
    height: i32,
}

impl Run {
    fn line(&mut self, s: &str) {
        println!("{s}");
        let _ = writeln!(self.log, "{s}");
    }
    fn check(&mut self, ok: bool, what: &str) {
        if ok {
            self.line(&format!("PASS {what}"));
        } else {
            self.failed += 1;
            self.line(&format!("FAIL {what}"));
        }
    }
    fn info(&mut self, s: &str) {
        self.line(&format!("INFO {s}"));
    }
    /// One frame of the recorded demo, pointer included.
    fn frame(&mut self, hold: f32) {
        let name = format!("f{:03}.png", self.frames.len());
        if let Some(pix) = screen::monitors().first().and_then(|m| screen::grab_with_cursor(&m.rect)) {
            let _ = pix.save_png(self.out.join(&name));
            self.frames.push((name, hold));
        }
    }
    fn shot(&mut self, name: &str) {
        let monitors = screen::monitors();
        if let Some(pix) = monitors.first().and_then(|m| screen::grab(&m.rect)) {
            let _ = pix.save_png(self.out.join(format!("{name}.png")));
        }
    }
}

fn sleep(ms: u64) {
    std::thread::sleep(Duration::from_millis(ms));
}

fn find(class: PCWSTR, title: PCWSTR) -> Option<HWND> {
    unsafe { FindWindowW(class, title).ok() }.filter(|h| !h.0.is_null())
}

fn wait_for(timeout_ms: u64, mut f: impl FnMut() -> bool) -> bool {
    let start = Instant::now();
    while start.elapsed() < Duration::from_millis(timeout_ms) {
        if f() {
            return true;
        }
        sleep(100);
    }
    f()
}

fn class_of(hwnd: HWND) -> String {
    let mut buf = [0u16; 128];
    let n = unsafe { GetClassNameW(hwnd, &mut buf) } as usize;
    String::from_utf16_lossy(&buf[..n])
}

fn send(inputs: &[INPUT]) {
    unsafe {
        SendInput(inputs, std::mem::size_of::<INPUT>() as i32);
    }
}

fn key_input(vk: u16, up: bool) -> INPUT {
    // With the scan code too, like a real keyboard: IMEs look at it.
    let scan = unsafe { MapVirtualKeyW(vk as u32, MAPVK_VK_TO_VSC) } as u16;
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: VIRTUAL_KEY(vk), wScan: scan, dwFlags: if up { KEYEVENTF_KEYUP } else { KEYBD_EVENT_FLAGS(0) }, time: 0, dwExtraInfo: 0 } },
    }
}

/// Text of a window in another process (GetWindowText can't read other processes' controls).
fn window_text(h: HWND) -> String {
    let mut buf = vec![0u16; 8192];
    let n = unsafe { SendMessageW(h, WM_GETTEXT, Some(WPARAM(buf.len())), Some(LPARAM(buf.as_mut_ptr() as isize))) }.0 as usize;
    String::from_utf16_lossy(&buf[..n.min(buf.len())])
}

/// Presses `vk` with `modifiers` held.
fn chord(modifiers: &[u16], vk: u16) {
    let mut v: Vec<INPUT> = modifiers.iter().map(|m| key_input(*m, false)).collect();
    v.push(key_input(vk, false));
    v.push(key_input(vk, true));
    v.extend(modifiers.iter().rev().map(|m| key_input(*m, true)));
    send(&v);
    sleep(150);
}

fn key(vk: u16) {
    chord(&[], vk);
}

/// Types lowercase ASCII letters and digits as key presses (so an IME sees them).
fn type_keys(s: &str) {
    for c in s.chars() {
        let vk = c.to_ascii_uppercase() as u16;
        send(&[key_input(vk, false), key_input(vk, true)]);
        sleep(60);
    }
}

fn mouse(run: &Run, x: i32, y: i32, flags: windows::Win32::UI::Input::KeyboardAndMouse::MOUSE_EVENT_FLAGS) -> INPUT {
    // Absolute coordinates are 0–65535 across the primary monitor.
    // Pixel p maps to the centre of its normalized cell, so it rounds back to exactly p.
    let nx = ((x * 2 + 1) * 65536 / (run.width.max(1) * 2)).min(65535);
    let ny = ((y * 2 + 1) * 65536 / (run.height.max(1) * 2)).min(65535);
    INPUT { r#type: INPUT_MOUSE, Anonymous: INPUT_0 { mi: MOUSEINPUT { dx: nx, dy: ny, mouseData: 0, dwFlags: flags | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_MOVE, time: 0, dwExtraInfo: 0 } } }
}

fn move_to(run: &Run, x: i32, y: i32) {
    send(&[mouse(run, x, y, Default::default())]);
    sleep(40);
}

fn click(run: &Run, x: i32, y: i32) {
    move_to(run, x, y);
    send(&[mouse(run, x, y, MOUSEEVENTF_LEFTDOWN)]);
    sleep(60);
    send(&[mouse(run, x, y, MOUSEEVENTF_LEFTUP)]);
    sleep(150);
}

fn drag(run: &Run, from: (i32, i32), to: (i32, i32)) {
    move_to(run, from.0, from.1);
    send(&[mouse(run, from.0, from.1, MOUSEEVENTF_LEFTDOWN)]);
    sleep(60);
    for i in 1..=12 {
        let x = from.0 + (to.0 - from.0) * i / 12;
        let y = from.1 + (to.1 - from.1) * i / 12;
        move_to(run, x, y);
    }
    send(&[mouse(run, to.0, to.1, MOUSEEVENTF_LEFTUP)]);
    sleep(200);
}

/// The text on the clipboard, if any.
fn clipboard_text() -> Option<String> {
    unsafe {
        if !crate::win::clipboard::open(None) {
            return None;
        }
        let result = GetClipboardData(windows::Win32::System::Ole::CF_UNICODETEXT.0 as u32).ok().and_then(|h| {
            let hg = windows::Win32::Foundation::HGLOBAL(h.0);
            let p = GlobalLock(hg) as *const u16;
            if p.is_null() {
                return None;
            }
            let mut n = 0;
            while *p.add(n) != 0 {
                n += 1;
            }
            let s = String::from_utf16_lossy(std::slice::from_raw_parts(p, n));
            let _ = GlobalUnlock(hg);
            Some(s)
        });
        let _ = CloseClipboard();
        result
    }
}

/// Width and height of the image on the clipboard, if any.
fn clipboard_image_size() -> Option<(i32, i32)> {
    unsafe {
        if IsClipboardFormatAvailable(CF_DIB.0 as u32).is_err() {
            return None;
        }
        if !crate::win::clipboard::open(None) {
            return None;
        }
        let result = GetClipboardData(CF_DIB.0 as u32).ok().and_then(|h| {
            let hg = windows::Win32::Foundation::HGLOBAL(h.0);
            let p = GlobalLock(hg) as *const windows::Win32::Graphics::Gdi::BITMAPINFOHEADER;
            if p.is_null() {
                return None;
            }
            let size = ((*p).biWidth, (*p).biHeight.abs());
            let _ = GlobalUnlock(hg);
            Some(size)
        });
        let _ = CloseClipboard();
        result
    }
}

fn clipboard_image() -> Option<crate::kit::image::RgbaImage> {
    use crate::kit::image::RgbaImage;
    unsafe {
        if !super::clipboard::open(None) { return None; }
        let image = GetClipboardData(CF_DIB.0 as u32).ok().and_then(|h| {
            let hg = windows::Win32::Foundation::HGLOBAL(h.0);
            let p = GlobalLock(hg) as *const u8;
            if p.is_null() { return None; }
            let result = (|| {
                let size = GlobalSize(hg);
                let header = &*(p as *const windows::Win32::Graphics::Gdi::BITMAPINFOHEADER);
                if header.biBitCount != 32 || header.biWidth <= 0 || header.biHeight == 0 { return None; }
                let (w, h) = (header.biWidth as u32, header.biHeight.unsigned_abs());
                let bytes = w as usize * h as usize * 4;
                if size < header.biSize as usize + bytes { return None; }
                let data = std::slice::from_raw_parts(p.add(header.biSize as usize), bytes);
                let mut image = RgbaImage::filled(w, h, [255; 4]);
                for y in 0..h { for x in 0..w {
                    let source_y = if header.biHeight > 0 { h - y - 1 } else { y };
                    let i = (source_y as usize * w as usize + x as usize) * 4;
                    let j = (y as usize * w as usize + x as usize) * 4;
                    image.data[j..j+4].copy_from_slice(&[data[i+2], data[i+1], data[i], 255]);
                } }
                Some(image)
            })();
            let _ = GlobalUnlock(hg); result
        });
        let _ = CloseClipboard(); image
    }
}

fn overlay() -> Option<HWND> {
    find(w!("ShotlateOverlay"), PCWSTR::null())
}

/// Presses the capture shortcut the user configured (it may not be the default).
fn start_capture() {
    use crate::kit::settings::{MOD_ALT, MOD_CONTROL, MOD_SHIFT, MOD_WIN};
    let path = crate::app_paths::settings_file();
    let sc = std::fs::read(path)
        .ok()
        .and_then(|b| serde_json::from_slice::<crate::kit::settings::Settings>(&b).ok())
        .map(|s| s.capture_shortcut)
        .unwrap_or(crate::kit::settings::Shortcut::CAPTURE);
    let mut mods = Vec::new();
    for (bit, vk) in [(MOD_CONTROL, VK_CONTROL.0), (MOD_ALT, VK_MENU.0), (MOD_SHIFT, VK_SHIFT.0), (MOD_WIN, 0x5B)] {
        if sc.modifiers & bit != 0 {
            mods.push(vk);
        }
    }
    chord(&mods, sc.vk as u16);
}

/// Closes a message box of the app titled `title` with `button` (IDYES / IDNO / IDOK).
fn answer_dialog(title: &str, button: MESSAGEBOX_RESULT) -> bool {
    let t = util::wide(title);
    match find(w!("#32770"), PCWSTR(t.as_ptr())) {
        Some(dlg) => unsafe {
            let _ = PostMessageW(Some(dlg), WM_COMMAND, WPARAM(button.0 as usize), LPARAM(0));
            true
        },
        None => false,
    }
}

// A window showing the OCR test image at a known place, for OCR and pin tests.
static IMAGE: &[u8] = include_bytes!("../../tests/data/mixed.png");

unsafe extern "system" fn image_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if msg == WM_PAINT {
        let mut ps = PAINTSTRUCT::default();
        let hdc = unsafe { BeginPaint(hwnd, &mut ps) };
        if let Ok(img) = crate::kit::image::RgbaImage::decode_png(IMAGE) {
            if let Some(mut pix) = tiny_skia::Pixmap::new(img.width, img.height) {
                pix.data_mut().copy_from_slice(&img.data);
                util::blit(hdc, 0, 0, &pix);
            }
        }
        unsafe {
            let _ = EndPaint(hwnd, &ps);
        }
        return LRESULT(0);
    }
    unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
}

fn show_image_window(x: i32, y: i32) {
    std::thread::spawn(move || unsafe {
        let class = WNDCLASSEXW { cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32, lpfnWndProc: Some(image_proc), lpszClassName: w!("ShotlateE2EImage"), ..Default::default() };
        RegisterClassExW(&class);
        let Ok(hwnd) = CreateWindowExW(WS_EX_TOPMOST, w!("ShotlateE2EImage"), w!("OCR test image"), WS_POPUP | WS_VISIBLE, x, y, 1400, 560, None, None, None, None) else { return };
        let _ = SetForegroundWindow(hwnd);
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    });
}

pub fn run(out: &Path) -> i32 {
    // `--e2e <out> <section>` runs only that section (capture, save, ocr, pin, settings, translate).
    let only = std::env::args().nth(3);
    // `demo` (the README recording) only runs when asked for by name.
    let want = |name: &str| only.as_deref().is_none_or(|o| o == name);
    let _ = std::fs::create_dir_all(out);
    let Ok(log) = std::fs::File::create(out.join("e2e.log")) else { return 2 };
    let (width, height) = unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) };
    let mut run = Run { out: out.to_path_buf(), frames: Vec::new(), log, failed: 0, width, height };
    run.info(&format!("screen {width}x{height}, dpi {}", unsafe { windows::Win32::UI::HiDpi::GetDpiForSystem() }));

    // 1. The app: start it if it isn't running, and answer the first-launch question (download the models).
    if find(w!("ShotlateApp"), PCWSTR::null()).is_none() {
        let exe = std::env::current_exe().unwrap_or_default();
        // No inherited output handles: the scheduled task running this would otherwise stay "running" while the app lives.
        let null = std::process::Stdio::null;
        let _ = std::process::Command::new(exe).env("SHOTLATE_TRACE", "1").stdin(null()).stdout(null()).stderr(null()).spawn();
    }
    run.check(wait_for(15_000, || find(w!("ShotlateApp"), PCWSTR::null()).is_some()), "app starts and creates its window");
    sleep(1500);
    run.shot("00-started");
    if wait_for(5_000, || answer_dialog("欢迎使用 Shotlate", IDYES)) {
        run.info("first-launch dialog answered: download the models");
    }
    let models = crate::app_paths::models_dir();
    let downloaded = wait_for(240_000, || crate::ocr::models::is_ready(&models));
    run.check(downloaded, "recognition models download on first launch");
    sleep(1500);
    run.shot("01-after-download");

    // Shared by the sections: the selection used in capture / save, and the test image used by ocr / pin / translate.
    let (x0, y0, x1, y1) = (width / 8, height / 8, width * 5 / 8, height * 5 / 8);
    let (ix, iy) = (width / 10, height / 10);
    if only.as_deref() == Some("demo") {
        demo(&mut run);
        run.line(&format!("DONE {} failed", run.failed));
        return if run.failed > 0 { 1 } else { 0 };
    }
    if want("esc") {
    // 1b. Esc right after the shortcut, before the mouse has moved, cancels the capture. Another app
    // (Notepad, clicked into) holds the foreground first, as in real use.
    let _ = std::process::Command::new("notepad.exe").spawn();
    wait_for(10_000, || find(w!("Notepad"), PCWSTR::null()).is_some());
    sleep(1500);
    let notepad = find(w!("Notepad"), PCWSTR::null());
    for attempt in 1..=3 {
        if let Some(np) = notepad {
            let mut r = windows::Win32::Foundation::RECT::default();
            unsafe {
                let _ = GetWindowRect(np, &mut r);
            }
            click(&run, (r.left + r.right) / 2, (r.top + r.bottom) / 2);
            sleep(300);
            run.info(&format!("before the shortcut the foreground is {}", class_of(unsafe { GetForegroundWindow() })));
        }
        start_capture();
        let open = wait_for(5_000, || overlay().is_some());
        sleep(300);
        let fg = unsafe { GetForegroundWindow() };
        run.info(&format!("attempt {attempt}: overlay open {open}, foreground {}", class_of(fg)));
        // Attempt 2: Esc pressed before letting go of Alt (Alt+Esc is a system shortcut).
        if attempt == 2 {
            chord(&[VK_MENU.0], VK_ESCAPE.0);
            sleep(300);
            run.info(&format!("after Alt+Esc the foreground is {}", class_of(unsafe { GetForegroundWindow() })));
        }
        key(VK_ESCAPE.0);
        let closed = wait_for(2_000, || overlay().is_none());
        run.check(closed, &format!("Esc right after the shortcut cancels the capture (attempt {attempt})"));
        if !closed {
            run.shot(&format!("esc-failed-{attempt}"));
            key(VK_ESCAPE.0);
            key(VK_ESCAPE.0);
            wait_for(2_000, || overlay().is_none());
        }
        sleep(500);
    }
    if let Some(np) = notepad {
        unsafe {
            let _ = PostMessageW(Some(np), WM_CLOSE, WPARAM(0), LPARAM(0));
        }
        sleep(800);
    }
    }
    if want("capture") {
    // 2. Capture: hotkey, focus, drag, annotate, Chinese text, copy.
    start_capture();
    let open = wait_for(5_000, || overlay().is_some());
    run.check(open, "the capture shortcut opens the overlay");
    sleep(500);
    let fg = unsafe { GetForegroundWindow() };
    run.check(class_of(fg) == "ShotlateOverlay", &format!("overlay has keyboard focus (foreground is {})", class_of(fg)));
    move_to(&run, width / 3, height / 3);
    sleep(300);
    run.shot("02-overlay");
    drag(&run, (x0, y0), (x1, y1));
    run.shot("03-selected");
    key(0x31);
    drag(&run, (x0 + 40, y0 + 40), (x0 + 200, y0 + 140));
    key(0x32);
    drag(&run, (x1 - 60, y1 - 60), (x0 + 220, y0 + 150));
    run.shot("04-annotated");
    key(0x36);
    click(&run, x0 + 60, y1 - 80);
    type_keys("nihao");
    sleep(600);
    run.shot("05-ime-composing");
    key(VK_SPACE.0);
    sleep(600);
    let typed = overlay().and_then(|o| unsafe { FindWindowExW(Some(o), None, w!("EDIT"), PCWSTR::null()) }.ok()).map(window_text).unwrap_or_default();
    run.check(typed.contains('你'), &format!("Pinyin + Space commits Chinese into the text annotation ({typed:?})"));
    run.shot("05b-ime-committed");
    type_keys("shotlate");
    key(VK_RETURN.0);
    sleep(300);
    run.shot("06-text-typed");
    key(VK_ESCAPE.0);
    sleep(200);
    run.shot("07-text-committed");
    chord(&[VK_CONTROL.0], 0x43);
    let closed = wait_for(3_000, || overlay().is_none());
    run.check(closed, "Ctrl+C closes the overlay");
    sleep(200);
    run.shot("08-hud-copied");
    let size = clipboard_image_size();
    run.check(size == Some((x1 - x0, y1 - y0)), &format!("clipboard has the selection's image ({size:?}, expected {:?})", (x1 - x0, y1 - y0)));

    }
    if want("save") {
    // 3. Save with Ctrl+S.
    let downloads = super::app::downloads_dir();
    let before = std::fs::read_dir(&downloads).map(|d| d.count()).unwrap_or(0);
    start_capture();
    wait_for(5_000, || overlay().is_some());
    sleep(400);
    drag(&run, (x0, y0), (x0 + 300, y0 + 200));
    chord(&[VK_CONTROL.0], 0x53);
    let saved = wait_for(4_000, || std::fs::read_dir(&downloads).map(|d| d.count()).unwrap_or(0) > before);
    run.check(saved, &format!("Ctrl+S saves into {}", downloads.display()));
    sleep(300);
    run.shot("09-saved");

    }
    if want("ocr") || want("pin") || want("translate") {
        show_image_window(ix, iy);
        sleep(1500);
    }
    if want("ocr") {
    // 4. OCR on a window showing the test image.
    start_capture();
    wait_for(5_000, || overlay().is_some());
    sleep(400);
    drag(&run, (ix + 5, iy + 5), ((ix + 1100).min(width - 60), (iy + 360).min(height - 120)));
    key(0x58);
    let ocr_text = {
        let mut text = String::new();
        wait_for(30_000, || {
            let Some(o) = overlay() else { return false };
            let Ok(edit) = (unsafe { FindWindowExW(Some(o), None, w!("EDIT"), PCWSTR::null()) }) else { return false };
            text = window_text(edit);
            text.len() > 10
        });
        text
    };
    sleep(300);
    run.shot("10-ocr");
    run.check(ocr_text.contains("quick brown fox") && ocr_text.contains("截图"), "X recognizes the text into the OCR panel");
    run.info(&format!("OCR text: {}", ocr_text.replace("\r\n", " | ")));
    key(VK_ESCAPE.0);
    key(VK_ESCAPE.0);
    wait_for(2_000, || overlay().is_none());

    }
    if want("pin") {
    // 5. Pin with T, then close it with Esc.
    start_capture();
    wait_for(5_000, || overlay().is_some());
    sleep(400);
    drag(&run, (ix + 10, iy + 10), (ix + 500, iy + 200));
    key(0x54);
    let pinned = wait_for(3_000, || find(w!("ShotlatePin"), PCWSTR::null()).is_some());
    run.check(pinned, "T pins the selection");
    sleep(400);
    run.shot("11-pin");
    if let Some(pin) = find(w!("ShotlatePin"), PCWSTR::null()) {
        unsafe {
            let _ = SetForegroundWindow(pin);
        }
        // The pin recognizes its text in the background; then a double-click on "quick" selects the word.
        // Recognition is shared with the earlier sections and may still be busy, so retry for a while.
        let (qx, qy) = (ix + 105, iy + 161);
        let mut word = String::new();
        for _ in 0..6 {
            sleep(2_000);
            move_to(&run, qx, qy);
            for _ in 0..2 {
                send(&[mouse(&run, qx, qy, MOUSEEVENTF_LEFTDOWN)]);
                send(&[mouse(&run, qx, qy, MOUSEEVENTF_LEFTUP)]);
                sleep(60);
            }
            sleep(300);
            chord(&[VK_CONTROL.0], 0x43);
            sleep(300);
            word = clipboard_text().unwrap_or_default();
            if word == "quick" {
                break;
            }
        }
        run.shot("11b-pin-word-selected");
        run.check(word == "quick", &format!("double-click selects a word on a pin, Ctrl+C copies it ({word:?})"));
        chord(&[VK_CONTROL.0], 0x41);
        chord(&[VK_CONTROL.0], 0x43);
        sleep(300);
        let all = clipboard_text().unwrap_or_default();
        run.check(all.contains("quick brown fox") && all.contains('\n'), &format!("Ctrl+A then Ctrl+C copies all of the pin's text ({} chars)", all.chars().count()));
        run.shot("11c-pin-all-selected");
        key(VK_ESCAPE.0);
        run.check(find(w!("ShotlatePin"), PCWSTR::null()).is_some(), "first Esc only clears the text selection");
        key(VK_ESCAPE.0);
        run.check(wait_for(2_000, || find(w!("ShotlatePin"), PCWSTR::null()).is_none()), "Esc closes the pin");
    }

    }
    if want("settings") {
    // 6. Settings: every pane, clicked in the menu; then 测试连接 with a wrong key (kept for step 7).
    // The topmost test image would cover the settings; hide it until step 7.
    let image = find(w!("ShotlateE2EImage"), PCWSTR::null());
    if let Some(img) = image {
        unsafe {
            let _ = ShowWindow(img, SW_HIDE);
        }
    }
    if let Some(app) = find(w!("ShotlateApp"), PCWSTR::null()) {
        unsafe {
            let _ = PostMessageW(Some(app), WM_APP + 3, WPARAM(0), LPARAM(0));
        }
    }
    let settings = wait_for(3_000, || find(w!("ShotlateSettings"), PCWSTR::null()).is_some());
    run.check(settings, "settings window opens");
    if let Some(s) = find(w!("ShotlateSettings"), PCWSTR::null()) {
        sleep(500);
        let scale = unsafe { windows::Win32::UI::HiDpi::GetDpiForWindow(s) }.max(96) as f32 / 96.0;
        let at = |p: crate::kit::geom::Point| {
            let mut pt = windows::Win32::Foundation::POINT { x: (p.x * scale) as i32, y: (p.y * scale) as i32 };
            unsafe {
                let _ = windows::Win32::Graphics::Gdi::ClientToScreen(s, &mut pt);
            }
            (pt.x, pt.y)
        };
        use crate::ui::settings_view::{api_key_center, sidebar_center, test_button_center, engine_center};
        for pane in 0..4 {
            let (x, y) = at(sidebar_center(pane));
            click(&run, x, y);
            sleep(400);
            run.shot(&format!("13-settings-{pane}"));
        }
        let (x, y) = at(sidebar_center(2));
        click(&run, x, y);
        let (x, y) = at(engine_center(true));
        click(&run, x, y);
        let (x, y) = at(api_key_center());
        click(&run, x, y);
        chord(&[VK_CONTROL.0], 0x41);
        type_keys("sk0000test");
        let (x, y) = at(test_button_center());
        click(&run, x, y);
        sleep(8_000);
        run.shot("14-settings-test-connection");
        run.check(find(w!("ShotlateApp"), PCWSTR::null()).is_some(), "测试连接 with a wrong key reports it instead of crashing");
        unsafe {
            let _ = PostMessageW(Some(s), WM_CLOSE, WPARAM(0), LPARAM(0));
        }
    }

    if let Some(img) = image {
        unsafe {
            let _ = ShowWindow(img, SW_SHOWNA);
        }
        sleep(500);
    }

    }
    if want("translate") {
    // 7. Translation with a wrong key goes to the network and must come back as a message, never a crash.
    start_capture();
    wait_for(5_000, || overlay().is_some());
    sleep(400);
    drag(&run, (ix + 5, iy + 5), ((ix + 1100).min(width - 60), (iy + 360).min(height - 120)));
    key(0x59);
    sleep(8_000);
    run.shot("15-translate-wrong-key");
    run.check(find(w!("ShotlateApp"), PCWSTR::null()).is_some() && overlay().is_some(), "Y with a wrong key shows the error instead of crashing");
    key(VK_ESCAPE.0);
    key(VK_ESCAPE.0);
    wait_for(2_000, || overlay().is_none());

    // Leave no fake key behind.
    let _ = std::fs::remove_file(crate::app_paths::api_key_file());

    }
    if want("scroll") { scroll_test(&mut run); }
    run.line(&format!("DONE {} failed", run.failed));
    if run.failed > 0 { 1 } else { 0 }
}

/// Moves the pointer from `from` to `to` in `steps`, recording a frame at each step.
fn glide(run: &mut Run, from: (i32, i32), to: (i32, i32), steps: i32, down: bool) {
    move_to(run, from.0, from.1);
    if down {
        send(&[mouse(run, from.0, from.1, MOUSEEVENTF_LEFTDOWN)]);
        sleep(60);
    }
    for i in 1..=steps {
        let (x, y) = (from.0 + (to.0 - from.0) * i / steps, from.1 + (to.1 - from.1) * i / steps);
        move_to(run, x, y);
        sleep(30);
        run.frame(0.05);
    }
    if down {
        send(&[mouse(run, to.0, to.1, MOUSEEVENTF_LEFTUP)]);
        sleep(200);
    }
}

/// Types pinyin, one frame per key, then commits the first candidate.
fn type_pinyin(run: &mut Run, s: &str) {
    for c in s.chars() {
        type_keys(&c.to_string());
        sleep(80);
        run.frame(0.12);
    }
    key(VK_SPACE.0);
    sleep(400);
    run.frame(0.5);
}

// Shown in Notepad as the thing being captured.
const DEMO_TEXT: &str = "Shotlate for Windows\r\n\r\n按 Alt+Shift+A 开始截图，拖出选区后标注。\r\n按 X 识别文字，按 Y 翻译，按 T 贴到屏幕上。\r\nEnter 复制，Ctrl+S 保存到“下载”文件夹。\r\n\r\nScreenshots stay on your computer.\r\nOnly the recognized text is sent for translation.\r\n";

thread_local! { static SCROLL_Y: std::cell::Cell<i32> = const { std::cell::Cell::new(0) }; }
unsafe extern "system" fn scroll_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if msg == WM_MOUSEWHEEL {
        let delta = ((wp.0 >> 16) as u16 as i16) as i32;
        SCROLL_Y.with(|s| s.set((s.get() - delta / 120 * 60).clamp(0, 1800)));
        unsafe { let _ = windows::Win32::Graphics::Gdi::InvalidateRect(Some(hwnd), None, false); }
        return LRESULT(0);
    }
    if msg == WM_PAINT {
        use crate::kit::{color::Color, geom::{Point, Rect}};
        use crate::render::{canvas::Canvas, text::Weight};
        let mut ps = PAINTSTRUCT::default(); let hdc = unsafe { BeginPaint(hwnd, &mut ps) };
        if let Some(mut pix) = tiny_skia::Pixmap::new(560, 500) {
            let offset = SCROLL_Y.with(|s| s.get());
            let mut c = Canvas::new(pix.as_mut(), tiny_skia::Transform::identity());
            c.fill_rect(&Rect::new(0.0, 0.0, 560.0, 500.0), Color::white(1.0));
            c.save(); c.clip_rect(&Rect::new(0.0, 24.0, 560.0, 446.0));
            for n in 0..80 {
                let y = 24.0 + n as f32 * 32.0 - offset as f32;
                if y < -32.0 || y > 470.0 { continue; }
                c.fill_rect(&Rect::new(0.0, y, 560.0, 32.0), if n % 2 == 0 { Color::rgb(0.94, 0.97, 0.98) } else { Color::white(1.0) });
                c.text(&format!("{:02}  Shotlate · 滚动截图 · capture and translate", n + 1), Point::new(18.0, y + 6.0), 15.0, Weight::Regular, Color::rgb(0.12, 0.16, 0.2));
                c.fill_rect(&Rect::new(490.0, y + 8.0, (n * 17 % 45 + 5) as f32, 10.0), Color::rgb(0.18, 0.6, 0.38));
            }
            c.restore();
            c.fill_rect(&Rect::new(0.0, 0.0, 560.0, 24.0), Color::rgb(0.1, 0.38, 0.55));
            c.text("Shotlate 长截图演示", Point::new(16.0, 3.0), 14.0, Weight::Bold, Color::white(1.0));
            c.fill_rect(&Rect::new(0.0, 470.0, 560.0, 30.0), Color::rgb(0.2, 0.24, 0.27));
            c.text("固定底栏只保留一次", Point::new(16.0, 477.0), 13.0, Weight::Regular, Color::white(1.0));
            util::blit(hdc, 0, 0, &pix);
        }
        unsafe { let _ = EndPaint(hwnd, &ps); } return LRESULT(0);
    }
    if msg == WM_APP + 72 { SCROLL_Y.with(|s| s.set(0)); unsafe { let _ = windows::Win32::Graphics::Gdi::InvalidateRect(Some(hwnd), None, false); } return LRESULT(0); }
    unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
}
fn scroll_test(run: &mut Run) {
    std::thread::spawn(move || unsafe {
        let class = WNDCLASSEXW { cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32, lpfnWndProc: Some(scroll_proc), lpszClassName: w!("ShotlateScrollDemo"),
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(), ..Default::default() };
        RegisterClassExW(&class);
        let Ok(hwnd) = CreateWindowExW(WS_EX_TOPMOST, w!("ShotlateScrollDemo"), w!("长截图演示"), WS_POPUP | WS_VISIBLE, 30, 90, 560, 500, None, None, None, None) else { return };
        let _ = SetForegroundWindow(hwnd);
        let mut msg = MSG::default(); while GetMessageW(&mut msg, None, 0, 0).as_bool() { let _ = TranslateMessage(&msg); DispatchMessageW(&msg); }
    });
    run.check(wait_for(5000, || find(w!("ShotlateScrollDemo"), PCWSTR::null()).is_some()), "scroll demo opens");
    let capture = |run: &Run| {
        start_capture(); wait_for(5000, || overlay().is_some()); sleep(300);
        drag(run, (30, 90), (590, 590)); key(0x53);
        wait_for(5000, || find(w!("ShotlateScroll"), w!("Shotlate 长截图")).is_some()); sleep(400);
    };
    let state = || find(w!("ShotlateScroll"), w!("Shotlate 长截图")).map(|h| unsafe { SendMessageW(h, crate::win::scroll::TEST_STATE, Some(WPARAM(0)), Some(LPARAM(0))).0 }).unwrap_or(0);
    let button = |run: &Run, index: usize, finished: bool| {
        if let Some(hwnd) = find(w!("ShotlateScroll"), w!("Shotlate 长截图")) {
            let mut r = windows::Win32::Foundation::RECT::default(); unsafe { let _ = GetClientRect(hwnd, &mut r); }
            let scale = unsafe { windows::Win32::UI::HiDpi::GetDpiForWindow(hwnd) }.max(96) as f32 / 96.0;
            let center = crate::ui::scroll_view::buttons(r.right as f32 / scale, r.bottom as f32 / scale, finished)[index].0.center();
            let mut p = windows::Win32::Foundation::POINT { x: (center.x * scale) as i32, y: (center.y * scale) as i32 };
            unsafe { let _ = windows::Win32::Graphics::Gdi::ClientToScreen(hwnd, &mut p); }
            click(run, p.x, p.y);
        }
    };
    capture(run); move_to(run, 250, 300);
    for _ in 0..6 {
        let mut input = mouse(run, 250, 300, windows::Win32::UI::Input::KeyboardAndMouse::MOUSEEVENTF_WHEEL);
        input.Anonymous.mi.mouseData = (-120i32) as u32; send(&[input]); sleep(450);
    }
    run.check(state() / 2 > 700, "manual scrolling appends real desktop frames");
    run.shot("16-scroll-manual"); button(run, 1, false); sleep(1200);
    run.shot("17-scroll-result"); button(run, 0, true); sleep(300);
    run.check(clipboard_image_size().is_some_and(|(w, h)| w == 560 && h > 700), "finished long screenshot copies at full resolution");
    if let Some((w, h)) = clipboard_image_size() { run.info(&format!("long clipboard {w}x{h}")); }
    button(run, 1, true); sleep(300); button(run, 2, true); sleep(300);
    run.check(find(w!("ShotlatePin"), PCWSTR::null()).is_some(), "long screenshot can be pinned");
    if let Some(p) = find(w!("ShotlatePin"), PCWSTR::null()) { unsafe { let _ = PostMessageW(Some(p), WM_CLOSE, WPARAM(0), LPARAM(0)); } }
    button(run, 3, true); sleep(300);
    capture(run); key(VK_ESCAPE.0); sleep(300);
    run.check(find(w!("ShotlateScroll"), PCWSTR::null()).is_none(), "Esc cancels long capture while another app has focus");
    if let Some(target) = find(w!("ShotlateScrollDemo"), PCWSTR::null()) { unsafe { SendMessageW(target, WM_APP + 72, Some(WPARAM(0)), Some(LPARAM(0))); } }
    capture(run); button(run, 0, false); sleep(350); move_to(run, 950, 700); sleep(350);
    run.check(state() > 0 && state() % 2 == 0, "moving outside the region pauses automatic scrolling");
    button(run, 0, false);
    for _ in 0..65 { sleep(250); run.frame(0.25); if state() / 2 > 2100 && state() % 2 == 0 { break; } }
    run.shot("18-scroll-auto");
    run.check(state() / 2 > 2100 && state() % 2 == 0, "automatic scrolling reaches the bottom and finishes");
    button(run, 0, true); sleep(300);
    run.check(clipboard_image_size().is_some_and(|(_, h)| h >= 2200), "automatic result has the full page");
    if let Some(image) = clipboard_image() {
        if let Ok(bytes) = image.encode_png() { let _ = std::fs::write(run.out.join("scroll-long.png"), bytes); }
    }
    button(run, 3, true);
    let wide_rect = windows::Win32::Foundation::RECT { left: 30, top: 90, right: 950, bottom: 590 };
    let baseline = screen::grab(&wide_rect).map(|p| crate::kit::image::RgbaImage { width: p.width(), height: p.height(), data: p.take() });
    start_capture(); wait_for(5000, || overlay().is_some()); sleep(300);
    drag(run, (30, 90), (950, 590)); key(0x53);
    run.check(wait_for(5000, || state() / 2 == 500), "wide region starts with the panel inside the region");
    run.shot("19-scroll-wide"); button(run, 1, false); sleep(1000); button(run, 0, true); sleep(300);
    run.check(clipboard_image() == baseline, "overlapping control panel is excluded from captured pixels");
    button(run, 3, true);
    if let Some(target) = find(w!("ShotlateScrollDemo"), PCWSTR::null()) { unsafe { let _ = PostMessageW(Some(target), WM_CLOSE, WPARAM(0), LPARAM(0)); } }
}

/// `--e2e <out> demo`: records the README's capture flow on the real desktop, frame by frame, with an
/// ffconcat list (`frames.ffconcat`) for ffmpeg. Geometry below assumes the VM's 1024×768 screen.
fn demo(run: &mut Run) {
    let file = std::env::temp_dir().join("Shotlate 使用说明.txt");
    let mut bytes = vec![0xEF, 0xBB, 0xBF];
    bytes.extend_from_slice(DEMO_TEXT.as_bytes());
    let _ = std::fs::write(&file, bytes);
    chord(&[VK_LWIN.0], 0x44);
    sleep(800);
    let _ = std::process::Command::new("notepad.exe").arg(&file).spawn();
    let found = wait_for(10_000, || find(w!("Notepad"), PCWSTR::null()).is_some());
    run.check(found, "Notepad opens the demo text");
    sleep(1500);
    let (nx, ny, nw, nh) = (150, 90, 720, 470);
    if let Some(np) = find(w!("Notepad"), PCWSTR::null()) {
        unsafe {
            let _ = SetWindowPos(np, None, nx, ny, nw, nh, SWP_NOZORDER | SWP_SHOWWINDOW);
            let _ = SetForegroundWindow(np);
        }
    }
    sleep(500);
    for _ in 0..3 {
        chord(&[VK_CONTROL.0], 0xBB);
    }
    sleep(800);
    move_to(run, 940, 680);
    sleep(300);
    run.frame(1.0);

    // Capture: the pointer glides onto Notepad, which gets highlighted.
    start_capture();
    run.check(wait_for(5_000, || overlay().is_some()), "the capture shortcut opens the overlay");
    sleep(600);
    run.frame(0.4);
    glide(run, (940, 680), (nx + 420, ny + 300), 10, false);
    run.frame(1.2);

    // Drag a selection around the text.
    let (sx0, sy0, sx1, sy1) = (nx + 10, ny + 86, nx + nw - 30, ny + 306);
    glide(run, (sx0, sy0), (sx1, sy1), 14, true);
    run.frame(1.0);

    // Rectangle, arrow, then two numbers with captions.
    key(0x31);
    run.frame(0.3);
    glide(run, (nx + 42, ny + 140), (nx + 162, ny + 168), 8, true);
    run.frame(0.5);
    key(0x32);
    glide(run, (nx + 470, ny + 100), (nx + 172, ny + 150), 8, true);
    run.frame(0.5);
    key(0x37);
    click(run, nx + 436, ny + 179);
    run.frame(0.3);
    type_pinyin(run, "shibie");
    click(run, nx + 426, ny + 204);
    run.frame(0.3);
    type_pinyin(run, "baocun");
    key(VK_ESCAPE.0);
    sleep(300);
    run.frame(1.0);

    // X: recognized text in the editable panel.
    key(0x58);
    let recognized = wait_for(30_000, || overlay_edit_texts().iter().any(|t| t.contains("Windows")));
    run.check(recognized, "X shows the recognized text");
    sleep(300);
    run.frame(2.5);
    run.shot("demo-ocr");
    key(VK_ESCAPE.0);
    sleep(300);

    // T: pin it, then select text on the pin like in a text field.
    key(0x54);
    let pinned = wait_for(3_000, || find(w!("ShotlatePin"), PCWSTR::null()).is_some());
    run.check(pinned, "T pins the selection");
    sleep(500);
    run.frame(1.0);
    sleep(3_000);
    let (wx, wy) = (nx + 300, ny + 254);
    glide(run, (sx1 - 40, sy1 - 20), (wx, wy), 8, false);
    for _ in 0..2 {
        send(&[mouse(run, wx, wy, MOUSEEVENTF_LEFTDOWN)]);
        send(&[mouse(run, wx, wy, MOUSEEVENTF_LEFTUP)]);
        sleep(60);
    }
    sleep(300);
    run.frame(1.2);
    run.shot("demo-pin-word");
    glide(run, (nx + 22, ny + 145), (nx + 350, ny + 254), 10, true);
    run.frame(0.6);
    chord(&[VK_CONTROL.0], 0x43);
    sleep(200);
    run.frame(1.6);
    run.shot("demo-pin-copied");
    key(VK_ESCAPE.0);
    key(VK_ESCAPE.0);
    sleep(400);
    run.frame(1.0);

    // The settings window's panes, each cropped to the window (README's settings.gif).
    if let Some(app) = find(w!("ShotlateApp"), PCWSTR::null()) {
        unsafe {
            let _ = PostMessageW(Some(app), WM_APP + 3, WPARAM(0), LPARAM(0));
        }
    }
    run.check(wait_for(3_000, || find(w!("ShotlateSettings"), PCWSTR::null()).is_some()), "settings window opens");
    if let Some(s) = find(w!("ShotlateSettings"), PCWSTR::null()) {
        sleep(600);
        let scale = unsafe { windows::Win32::UI::HiDpi::GetDpiForWindow(s) }.max(96) as f32 / 96.0;
        let mut frame = windows::Win32::Foundation::RECT::default();
        unsafe {
            let _ = windows::Win32::Graphics::Dwm::DwmGetWindowAttribute(
                s,
                windows::Win32::Graphics::Dwm::DWMWA_EXTENDED_FRAME_BOUNDS,
                &mut frame as *mut _ as *mut core::ffi::c_void,
                std::mem::size_of::<windows::Win32::Foundation::RECT>() as u32,
            );
        }
        for pane in 0..4 {
            let p = crate::ui::settings_view::sidebar_center(pane);
            let mut pt = windows::Win32::Foundation::POINT { x: (p.x * scale) as i32, y: (p.y * scale) as i32 };
            unsafe {
                let _ = windows::Win32::Graphics::Gdi::ClientToScreen(s, &mut pt);
            }
            click(run, pt.x, pt.y);
            // Park the pointer outside the window so no hover state shows.
            move_to(run, frame.right + 40, frame.bottom + 20);
            sleep(500);
            if let Some(pix) = screen::grab(&frame) {
                let _ = pix.save_png(run.out.join(format!("settings-{pane}.png")));
            }
        }
        unsafe {
            let _ = PostMessageW(Some(s), WM_CLOSE, WPARAM(0), LPARAM(0));
        }
    }

    let mut list = String::from("ffconcat version 1.0\n");
    for (name, hold) in &run.frames {
        list.push_str(&format!("file {name}\nduration {hold}\n"));
    }
    if let Some((last, _)) = run.frames.last() {
        list.push_str(&format!("file {last}\n"));
    }
    let _ = std::fs::write(run.out.join("frames.ffconcat"), list);
    if let Some(np) = find(w!("Notepad"), PCWSTR::null()) {
        unsafe {
            let _ = PostMessageW(Some(np), WM_CLOSE, WPARAM(0), LPARAM(0));
        }
    }
}

/// Texts of the overlay's EDIT children (the hidden text input and the OCR panel).
fn overlay_edit_texts() -> Vec<String> {
    let Some(o) = overlay() else { return Vec::new() };
    let mut texts = Vec::new();
    let mut after: Option<HWND> = None;
    while let Ok(h) = unsafe { FindWindowExW(Some(o), after, w!("EDIT"), PCWSTR::null()) } {
        if h.0.is_null() {
            break;
        }
        texts.push(window_text(h));
        after = Some(h);
    }
    texts
}
