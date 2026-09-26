//! `--e2e <out-dir>`: an end-to-end run against the real app in the interactive session, driven with SendInput
//! like a person would: hotkey, drag, keys, Chinese input, copy, save, OCR, pin, settings. Every step is logged
//! as PASS / FAIL / INFO and screenshotted into `out-dir`, so a run inside a VM can be reviewed from the host.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Gdi::{BeginPaint, EndPaint, PAINTSTRUCT};
use windows::Win32::System::DataExchange::{CloseClipboard, GetClipboardData, IsClipboardFormatAvailable};
use windows::Win32::System::Memory::{GlobalLock, GlobalUnlock};
use windows::Win32::System::Ole::CF_DIB;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    MAPVK_VK_TO_VSC, MapVirtualKeyW,
    INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT, KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP, MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_LEFTDOWN,
    MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MOVE, MOUSEINPUT, SendInput, VIRTUAL_KEY, VK_CONTROL, VK_ESCAPE, VK_MENU, VK_RETURN, VK_SHIFT, VK_SPACE,
};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{PCWSTR, w};

use super::{screen, util};

struct Run {
    out: PathBuf,
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
    let want = |name: &str| only.as_deref().is_none_or(|o| o == name);
    let _ = std::fs::create_dir_all(out);
    let Ok(log) = std::fs::File::create(out.join("e2e.log")) else { return 2 };
    let (width, height) = unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) };
    let mut run = Run { out: out.to_path_buf(), log, failed: 0, width, height };
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
        use crate::ui::settings_view::{api_key_center, sidebar_center, test_button_center};
        for pane in 0..4 {
            let (x, y) = at(sidebar_center(pane));
            click(&run, x, y);
            sleep(400);
            run.shot(&format!("13-settings-{pane}"));
        }
        let (x, y) = at(sidebar_center(2));
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
    run.line(&format!("DONE {} failed", run.failed));
    if run.failed > 0 { 1 } else { 0 }
}
