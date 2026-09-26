//! The settings window: a host for `ui::settings_view`, which draws everything like the capture toolbar.
//! Only the three text boxes (API Key, Base URL, model) are real EDIT controls, borderless, sitting in the
//! drawn fields, so typing, IME, selection and paste behave natively. Every change is written at once.

use std::cell::RefCell;

use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{DWMWA_USE_IMMERSIVE_DARK_MODE, DwmSetWindowAttribute};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, CreateFontW, CreateSolidBrush, DEFAULT_CHARSET, DeleteObject, EndPaint, FF_DONTCARE,
    FW_NORMAL, HBRUSH, HDC, HFONT, HGDIOBJ, InvalidateRect, OUT_DEFAULT_PRECIS, PAINTSTRUCT, ScreenToClient, SetBkColor, SetTextColor,
};
use windows::Win32::System::Diagnostics::Debug::MessageBeep;
use windows::Win32::UI::Controls::{EM_SETSEL, WM_MOUSELEAVE};
use windows::Win32::UI::HiDpi::{AdjustWindowRectExForDpi, GetDpiForWindow};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetFocus, GetKeyState, ReleaseCapture, SetCapture, SetFocus, TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent, VK_CONTROL, VK_LWIN, VK_MENU,
    VK_RWIN, VK_SHIFT, VK_TAB,
};
use windows::Win32::UI::Shell::{DefSubclassProc, SetWindowSubclass, ShellExecuteW};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{PCWSTR, w};

use super::util::{self, lparam_point, wide};
use super::{app, autostart, dialogs, secret, updater};
use crate::kit::color::Color;
use crate::kit::geom::Point;
use crate::kit::settings;
use crate::kit::translator::{self, DEFAULT_BASE_URL, DEFAULT_MODEL, DEFAULT_TARGET_LANGUAGE, Item};
use crate::ui::chrome::Theme;
use crate::ui::settings_view::{Field, Models, SettingsEffect, SettingsState, SettingsView, WINDOW};

const CLASS: PCWSTR = w!("ShotlateSettings");
const FIELD_IDS: [(Field, i32); 3] = [(Field::ApiKey, 40), (Field::BaseUrl, 42), (Field::Model, 43)];

struct Host {
    hwnd: HWND,
    view: SettingsView,
    edits: Vec<(Field, HWND)>,
    font: HFONT,
    brush: HBRUSH,
    scale: f32,
    tracking: bool,
    /// Set while filling the text boxes from the settings, so their change notifications are ignored.
    loading: bool,
}

thread_local! {
    static HOST: RefCell<Option<Host>> = const { RefCell::new(None) };
}

fn with<R>(f: impl FnOnce(&mut Host) -> R) -> Option<R> {
    HOST.with(|h| h.try_borrow_mut().ok().and_then(|mut h| h.as_mut().map(f)))
}

pub fn register() {
    let class = WNDCLASSEXW {
        cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
        lpfnWndProc: Some(proc),
        lpszClassName: CLASS,
        hCursor: unsafe { LoadCursorW(None, IDC_ARROW).unwrap_or_default() },
        // The app icon (resource 1) in the title bar and Alt+Tab.
        hIcon: unsafe {
            let instance = windows::Win32::System::LibraryLoader::GetModuleHandleW(None).unwrap_or_default();
            LoadIconW(Some(instance.into()), PCWSTR(1 as *const u16)).unwrap_or_default()
        },
        ..Default::default()
    };
    unsafe {
        RegisterClassExW(&class);
    }
}

/// The settings window draws its own controls; keyboard navigation is handled by the text boxes themselves.
pub fn is_dialog_message(_msg: &MSG) -> bool {
    false
}

fn models_state() -> Models {
    if app::is_downloading() {
        let p = *app::PROGRESS.lock().unwrap_or_else(|e| e.into_inner());
        Models::Downloading(p.map_or(0.0, |(d, t)| if t > 0 { d as f32 / t as f32 } else { 0.0 }))
    } else if app::models_ready() {
        Models::Ready
    } else {
        Models::Missing
    }
}

fn load_state() -> SettingsState {
    let s = settings::get();
    let (capture_ok, toggle_ok) = app::hotkey_status();
    SettingsState {
        capture: s.capture_shortcut,
        toggle_pins: s.toggle_pins_shortcut,
        capture_ok,
        toggle_ok,
        save_dir: app::save_directory().display().to_string(),
        format: s.image_format,
        language: s.target_language,
        testing: false,
        test_result: None,
        login: autostart::is_enabled(),
        auto_update: updater::is_available().then(updater::automatic),
        models: models_state(),
        models_mb: crate::ocr::models::TOTAL_BYTES as f32 / 1e6,
        models_dir: crate::app_paths::models_dir().display().to_string(),
        version: updater::VERSION.into(),
    }
}

fn colorref(c: Color) -> COLORREF {
    let (r, g, b) = c.to_rgb8();
    COLORREF(r as u32 | (g as u32) << 8 | (b as u32) << 16)
}

fn make_font(scale: f32) -> HFONT {
    unsafe {
        CreateFontW(
            -((13.0 * scale).round() as i32),
            0,
            0,
            0,
            FW_NORMAL.0 as i32,
            0,
            0,
            0,
            DEFAULT_CHARSET,
            OUT_DEFAULT_PRECIS,
            CLIP_DEFAULT_PRECIS,
            CLEARTYPE_QUALITY,
            FF_DONTCARE.0 as u32,
            w!("Microsoft YaHei UI"),
        )
    }
}

fn apply_title_bar_theme(hwnd: HWND, dark: bool) {
    let value: i32 = dark as i32;
    unsafe {
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE, &value as *const i32 as *const _, 4);
    }
}

/// Opens the settings window, or brings it to the front.
pub fn show() {
    if let Some(hwnd) = with(|h| h.hwnd) {
        unsafe {
            let _ = ShowWindow(hwnd, SW_SHOWNORMAL);
            let _ = SetForegroundWindow(hwnd);
        }
        return;
    }
    let style = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX;
    let Ok(hwnd) = (unsafe { CreateWindowExW(WINDOW_EX_STYLE(0), CLASS, w!("Shotlate 设置"), style, CW_USEDEFAULT, CW_USEDEFAULT, 720, 500, None, None, None, None) }) else {
        return;
    };
    let dark = util::is_dark_mode();
    apply_title_bar_theme(hwnd, dark);
    let scale = unsafe { GetDpiForWindow(hwnd) }.max(96) as f32 / 96.0;
    let view = SettingsView::new(load_state(), Theme { dark });
    let brush = unsafe { CreateSolidBrush(colorref(view.field_background())) };
    HOST.with(|h| *h.borrow_mut() = Some(Host { hwnd, view, edits: Vec::new(), font: make_font(scale), brush, scale, tracking: false, loading: true }));

    // The three text boxes: created once, shown on the translation pane.
    for (field, id) in FIELD_IDS {
        let password = if field == Field::ApiKey { ES_PASSWORD } else { 0 };
        let style = WS_CHILD | WS_TABSTOP | WINDOW_STYLE((ES_AUTOHSCROLL | password) as u32);
        if let Ok(edit) = unsafe { CreateWindowExW(WINDOW_EX_STYLE(0), w!("EDIT"), w!(""), style, 0, 0, 10, 10, Some(hwnd), Some(HMENU(id as isize as *mut _)), None, None) } {
            let font = with(|h| h.font).unwrap_or_default();
            unsafe {
                SendMessageW(edit, WM_SETFONT, Some(WPARAM(font.0 as usize)), Some(LPARAM(1)));
                let _ = SetWindowSubclass(edit, Some(edit_subclass), id as usize, 0);
            }
            with(|h| h.edits.push((field, edit)));
        }
    }
    let s = settings::get();
    set_edit(Field::ApiKey, &secret::api_key());
    set_edit(Field::BaseUrl, &s.base_url);
    set_edit(Field::Model, &s.model);
    with(|h| h.loading = false);

    resize_to_scale(hwnd, scale, None);
    layout_edits();
    unsafe {
        let _ = ShowWindow(hwnd, SW_SHOWNORMAL);
        let _ = SetForegroundWindow(hwnd);
    }
}

/// Sizes the window so its client area is the view at `scale`; centered on the screen, or at `suggested`.
fn resize_to_scale(hwnd: HWND, scale: f32, suggested: Option<&RECT>) {
    let (cw, ch) = ((WINDOW.width * scale).round() as i32, (WINDOW.height * scale).round() as i32);
    let mut r = RECT { left: 0, top: 0, right: cw, bottom: ch };
    unsafe {
        let style = WINDOW_STYLE(GetWindowLongW(hwnd, GWL_STYLE) as u32);
        let _ = AdjustWindowRectExForDpi(&mut r, style, false, WINDOW_EX_STYLE(0), GetDpiForWindow(hwnd));
        let (w, h) = (r.right - r.left, r.bottom - r.top);
        let (x, y) = match suggested {
            Some(s) => (s.left, s.top),
            None => ((GetSystemMetrics(SM_CXSCREEN) - w) / 2, (GetSystemMetrics(SM_CYSCREEN) - h) / 2),
        };
        let _ = SetWindowPos(hwnd, None, x, y, w, h, SWP_NOZORDER | SWP_NOACTIVATE);
    }
}

fn set_edit(field: Field, text: &str) {
    if let Some(edit) = with(|h| h.edits.iter().find(|e| e.0 == field).map(|e| e.1)).flatten() {
        let t = wide(text);
        unsafe {
            let _ = SetWindowTextW(edit, PCWSTR(t.as_ptr()));
        }
    }
}

fn edit_text(edit: HWND) -> String {
    let len = unsafe { GetWindowTextLengthW(edit) };
    let mut buf = vec![0u16; len as usize + 1];
    unsafe {
        GetWindowTextW(edit, &mut buf);
    }
    util::from_wide(&buf)
}

/// Puts the text boxes of the current pane over their drawn fields and hides the others.
fn layout_edits() {
    let Some((edits, fields, scale, font, parent)) = with(|h| (h.edits.clone(), h.view.fields(), h.scale, h.font, h.hwnd)) else { return };
    for (field, edit) in edits {
        match fields.iter().find(|f| f.0 == field) {
            Some((_, r)) => {
                let line = (18.0 * scale).round() as i32;
                // Single-line EDITs draw from the top; center the line in the field.
                let y = (r.y * scale + (r.height * scale - line as f32) / 2.0).round() as i32;
                unsafe {
                    SendMessageW(edit, WM_SETFONT, Some(WPARAM(font.0 as usize)), Some(LPARAM(0)));
                    let _ = SetWindowPos(edit, None, (r.x * scale).round() as i32, y, (r.width * scale).round() as i32, line, SWP_NOZORDER | SWP_NOACTIVATE | SWP_SHOWWINDOW);
                }
            }
            None => unsafe {
                if GetFocus() == edit {
                    let _ = SetFocus(Some(parent));
                }
                let _ = ShowWindow(edit, SW_HIDE);
            },
        }
    }
}

fn invalidate() {
    if let Some(hwnd) = with(|h| h.hwnd) {
        unsafe {
            let _ = InvalidateRect(Some(hwnd), None, false);
        }
    }
}

/// Updates the recognition-model line (download progress comes from a worker thread).
pub fn refresh_models() {
    if with(|h| h.view.state.models = models_state()).is_some() {
        invalidate();
    }
}

fn refresh_hotkey_status() {
    let (capture_ok, toggle_ok) = app::hotkey_status();
    with(|h| {
        h.view.state.capture_ok = capture_ok;
        h.view.state.toggle_ok = toggle_ok;
    });
}

/// Calls the view, repaints, and carries out its effects outside the borrow (they may open dialogs).
fn view_call(f: impl FnOnce(&mut SettingsView)) {
    let effects = with(|h| {
        f(&mut h.view);
        h.view.drain_effects()
    })
    .unwrap_or_default();
    invalidate();
    for e in effects {
        apply(e);
    }
}

fn apply(effect: SettingsEffect) {
    let owner = with(|h| h.hwnd);
    match effect {
        SettingsEffect::SetCapture(sc) => {
            settings::update(|s| s.capture_shortcut = sc);
            app::register_hotkeys();
            app::refresh_tray_tip();
            refresh_hotkey_status();
        }
        SettingsEffect::SetTogglePins(sc) => {
            settings::update(|s| s.toggle_pins_shortcut = sc);
            app::register_hotkeys();
            refresh_hotkey_status();
        }
        SettingsEffect::PauseHotkeys => {
            app::pause_hotkeys();
            // Keys must reach the window, not a text box.
            if let Some(hwnd) = owner {
                unsafe {
                    let _ = SetFocus(Some(hwnd));
                }
            }
        }
        SettingsEffect::ResumeHotkeys => {
            app::register_hotkeys();
            refresh_hotkey_status();
        }
        SettingsEffect::ChooseFolder => {
            if let Some(dir) = dialogs::pick_folder(owner, &app::save_directory()) {
                settings::update(|s| s.save_directory = Some(dir.display().to_string()));
                with(|h| h.view.state.save_dir = dir.display().to_string());
            }
        }
        SettingsEffect::SetFormat(f) => settings::update(|s| s.image_format = f),
        SettingsEffect::SetLanguage(l) => settings::update(|s| s.target_language = l),
        SettingsEffect::OpenUrl(url) => {
            let u = wide(&url);
            unsafe {
                ShellExecuteW(None, w!("open"), PCWSTR(u.as_ptr()), PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL);
            }
        }
        SettingsEffect::TestConnection => {
            let config = app::translation_config();
            std::thread::spawn(move || {
                let items = [Item { id: 0, text: "Take a screenshot and translate it in place.".into() }];
                let result = util::guarded(|| translator::translate(&items, &config)).unwrap_or_else(|e| Err(translator::TranslationError::BadResponse(e)));
                let (ok, text) = match result {
                    Ok(r) => (true, r.get(&0).map_or("连接成功，但没有返回译文".to_string(), |t| format!("连接成功：{t}"))),
                    Err(e) => (false, format!("失败：{e}")),
                };
                util::run_on_ui(move || {
                    with(|h| {
                        h.view.state.testing = false;
                        h.view.state.test_result = Some((ok, text));
                    });
                    invalidate();
                });
            });
        }
        SettingsEffect::ResetTranslation => {
            settings::update(|s| {
                s.base_url = DEFAULT_BASE_URL.into();
                s.model = DEFAULT_MODEL.into();
                s.target_language = DEFAULT_TARGET_LANGUAGE.into();
            });
            with(|h| h.view.state.language = DEFAULT_TARGET_LANGUAGE.into());
            set_edit(Field::BaseUrl, DEFAULT_BASE_URL);
            set_edit(Field::Model, DEFAULT_MODEL);
        }
        SettingsEffect::SetLogin(on) => {
            if let Err(e) = autostart::set_enabled(on) {
                dialogs::warn(owner, "设置开机启动失败", &e);
                with(|h| h.view.state.login = autostart::is_enabled());
            }
        }
        SettingsEffect::SetAutoUpdate(on) => updater::set_automatic(on),
        SettingsEffect::ModelsButton => {
            if app::is_downloading() {
                app::cancel_model_download();
            } else {
                if app::models_ready() {
                    // Re-download: remove the files so every one is fetched and verified again.
                    for f in crate::ocr::models::FILES.iter() {
                        let _ = std::fs::remove_file(crate::app_paths::models_dir().join(f.name));
                    }
                }
                app::start_model_download();
            }
            refresh_models();
        }
        SettingsEffect::FieldsChanged => layout_edits(),
        SettingsEffect::Beep => unsafe {
            let _ = MessageBeep(MB_OK);
        },
    }
    invalidate();
}

fn edit_changed(id: i32, edit: HWND) {
    if with(|h| h.loading).unwrap_or(true) {
        return;
    }
    let text = edit_text(edit);
    match FIELD_IDS.iter().find(|f| f.1 == id).map(|f| f.0) {
        Some(Field::ApiKey) => secret::set_api_key(&text),
        Some(Field::BaseUrl) => {
            let v = text.trim().to_string();
            settings::update(|s| s.base_url = if v.is_empty() { DEFAULT_BASE_URL.into() } else { v });
        }
        Some(Field::Model) => {
            let v = text.trim().to_string();
            settings::update(|s| s.model = if v.is_empty() { DEFAULT_MODEL.into() } else { v });
        }
        None => {}
    }
}

/// Tab and Shift+Tab move between the text boxes of the pane.
unsafe extern "system" fn edit_subclass(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM, _id: usize, _: usize) -> LRESULT {
    if msg == WM_KEYDOWN && wp.0 as u32 == VK_TAB.0 as u32 {
        let back = unsafe { GetKeyState(VK_SHIFT.0 as i32) } < 0;
        let visible: Vec<HWND> = with(|h| h.edits.iter().map(|e| e.1).filter(|e| unsafe { IsWindowVisible(*e) }.as_bool()).collect()).unwrap_or_default();
        if let Some(i) = visible.iter().position(|e| *e == hwnd) {
            let n = visible.len();
            let next = visible[if back { (i + n - 1) % n } else { (i + 1) % n }];
            unsafe {
                let _ = SetFocus(Some(next));
                SendMessageW(next, EM_SETSEL, Some(WPARAM(0)), Some(LPARAM(-1)));
            }
        }
        return LRESULT(0);
    }
    if msg == WM_CHAR && wp.0 == 0x09 {
        return LRESULT(0);
    }
    unsafe { DefSubclassProc(hwnd, msg, wp, lp) }
}

fn point(lp: LPARAM) -> Point {
    let (x, y) = lparam_point(lp);
    let scale = with(|h| h.scale).unwrap_or(1.0);
    Point::new(x as f32 / scale, y as f32 / scale)
}

fn is_recording() -> bool {
    with(|h| h.view.is_recording()).unwrap_or(false)
}

unsafe extern "system" fn proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_ERASEBKGND => return LRESULT(1),
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            let hdc = unsafe { BeginPaint(hwnd, &mut ps) };
            let frame = HOST.with(|h| h.try_borrow().ok().and_then(|h| h.as_ref().and_then(|h| h.view.render(h.scale))));
            if let Some(frame) = frame {
                util::blit(hdc, 0, 0, &frame);
            }
            unsafe {
                let _ = EndPaint(hwnd, &ps);
            }
            return LRESULT(0);
        }
        WM_MOUSEMOVE => {
            let track = with(|h| !std::mem::replace(&mut h.tracking, true)).unwrap_or(false);
            if track {
                let mut t = TRACKMOUSEEVENT { cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: hwnd, dwHoverTime: 0 };
                unsafe {
                    let _ = TrackMouseEvent(&mut t);
                }
            }
            let p = point(lp);
            view_call(|v| v.mouse_move(p));
            return LRESULT(0);
        }
        WM_MOUSELEAVE => {
            with(|h| h.tracking = false);
            view_call(|v| v.mouse_leave());
            return LRESULT(0);
        }
        WM_LBUTTONDOWN => {
            unsafe {
                // Clicking the window (not a text box) takes the keyboard back, e.g. for recording a shortcut.
                let _ = SetFocus(Some(hwnd));
                SetCapture(hwnd);
            }
            let p = point(lp);
            view_call(|v| v.mouse_down(p));
            return LRESULT(0);
        }
        WM_LBUTTONUP => {
            unsafe {
                let _ = ReleaseCapture();
            }
            let p = point(lp);
            view_call(|v| v.mouse_up(p));
            return LRESULT(0);
        }
        WM_SETCURSOR => {
            if util::loword(lp.0 as usize) as u32 == HTCLIENT && HWND(wp.0 as *mut _) == hwnd {
                let mut pt = POINT::default();
                unsafe {
                    let _ = GetCursorPos(&mut pt);
                    let _ = ScreenToClient(hwnd, &mut pt);
                }
                let scale = with(|h| h.scale).unwrap_or(1.0);
                let clickable = with(|h| h.view.is_clickable(Point::new(pt.x as f32 / scale, pt.y as f32 / scale))).unwrap_or(false);
                unsafe {
                    SetCursor(LoadCursorW(None, if clickable { IDC_HAND } else { IDC_ARROW }).ok());
                }
                return LRESULT(1);
            }
        }
        WM_KEYDOWN | WM_SYSKEYDOWN => {
            if is_recording() {
                let down = |vk: u16| unsafe { GetKeyState(vk as i32) } < 0;
                let (ctrl, alt, shift, win) = (down(VK_CONTROL.0), down(VK_MENU.0), down(VK_SHIFT.0), down(VK_LWIN.0) || down(VK_RWIN.0));
                let vk = wp.0 as u32;
                view_call(|v| {
                    v.key_down(vk, ctrl, alt, shift, win);
                });
                return LRESULT(0);
            }
        }
        // Alt+key while recording would beep or open the system menu.
        WM_SYSCHAR | WM_CHAR if is_recording() => return LRESULT(0),
        WM_COMMAND => {
            let code = util::hiword(wp.0) as u32;
            if code == EN_CHANGE {
                edit_changed(util::loword(wp.0) as i32, HWND(lp.0 as *mut _));
            }
            return LRESULT(0);
        }
        WM_CTLCOLOREDIT => {
            let hdc = HDC(wp.0 as *mut _);
            if let Some((bg, fg, brush)) = with(|h| (h.view.field_background(), h.view.field_text(), h.brush)) {
                unsafe {
                    SetTextColor(hdc, colorref(fg));
                    SetBkColor(hdc, colorref(bg));
                }
                return LRESULT(brush.0 as isize);
            }
        }
        WM_SETTINGCHANGE => {
            // The light/dark app setting changed.
            let dark = util::is_dark_mode();
            let changed = with(|h| {
                if h.view.theme().dark == dark {
                    return false;
                }
                h.view.set_theme(Theme { dark });
                let old = std::mem::replace(&mut h.brush, unsafe { CreateSolidBrush(colorref(h.view.field_background())) });
                unsafe {
                    let _ = DeleteObject(HGDIOBJ(old.0));
                }
                true
            })
            .unwrap_or(false);
            if changed {
                apply_title_bar_theme(hwnd, dark);
                invalidate();
                let edits: Vec<HWND> = with(|h| h.edits.iter().map(|e| e.1).collect()).unwrap_or_default();
                for e in edits {
                    unsafe {
                        let _ = InvalidateRect(Some(e), None, true);
                    }
                }
            }
        }
        WM_DPICHANGED => {
            let scale = util::hiword(wp.0).max(96) as f32 / 96.0;
            let font = make_font(scale);
            let old = with(|h| {
                h.scale = scale;
                std::mem::replace(&mut h.font, font)
            });
            if let Some(old) = old {
                unsafe {
                    let _ = DeleteObject(HGDIOBJ(old.0));
                }
            }
            let suggested = unsafe { *(lp.0 as *const RECT) };
            resize_to_scale(hwnd, scale, Some(&suggested));
            layout_edits();
            invalidate();
            return LRESULT(0);
        }
        WM_CLOSE => {
            unsafe {
                let _ = DestroyWindow(hwnd);
            }
            return LRESULT(0);
        }
        WM_DESTROY => {
            let recording = is_recording();
            if let Some(host) = HOST.with(|h| h.try_borrow_mut().ok().and_then(|mut h| h.take())) {
                unsafe {
                    let _ = DeleteObject(HGDIOBJ(host.font.0));
                    let _ = DeleteObject(HGDIOBJ(host.brush.0));
                }
            }
            // A shortcut being recorded had paused the hotkeys.
            if recording {
                app::register_hotkeys();
            }
            return LRESULT(0);
        }
        _ => {}
    }
    unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
}
