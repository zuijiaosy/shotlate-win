//! One capture session: a topmost overlay window per monitor, each driving a `CaptureView`.
//! Win32 messages become view calls; the view's effects become clipboard writes, files, pins and so on.
//!
//! Re-entrancy: many Win32 calls send messages to our own windows synchronously (creating, destroying,
//! focusing, modal dialogs). The session is only borrowed for short view calls; effects run after the
//! borrow ends, and a window procedure that finds the session busy falls back to DefWindowProc.

use std::cell::{Cell, RefCell};
use std::sync::atomic::{AtomicIsize, AtomicU64, Ordering};
use std::time::Instant;

use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreateFontW, CreateSolidBrush, DEFAULT_CHARSET, DeleteObject, EndPaint, FW_NORMAL, HBRUSH, HDC, HFONT, HGDIOBJ, InvalidateRect,
    PAINTSTRUCT, SetBkColor, SetTextColor, CLEARTYPE_QUALITY, OUT_DEFAULT_PRECIS, CLIP_DEFAULT_PRECIS, FF_DONTCARE,
};
use windows::Win32::UI::Input::Ime::{
    CANDIDATEFORM, CFS_CANDIDATEPOS, CFS_FORCE_POSITION, COMPOSITIONFORM, HIMC, ImmAssociateContext, ImmGetContext, ImmReleaseContext,
    ImmSetCandidateWindow, ImmSetCompositionWindow, ImmGetCompositionStringW, GCS_COMPSTR,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetFocus, GetKeyState, ReleaseCapture, SetCapture, SetFocus, TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent, VK_BACK, VK_CONTROL, VK_DELETE,
    VK_DOWN, VK_ESCAPE, VK_F4, VK_LEFT, VK_MENU, VK_RETURN, VK_RIGHT, VK_SHIFT, VK_TAB, VK_UP,
};
use windows::Win32::System::Diagnostics::Debug::MessageBeep;
use windows::Win32::UI::Controls::{EM_GETSEL, EM_SETSEL, WM_MOUSELEAVE};
use windows::Win32::UI::Shell::{DefSubclassProc, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{PCWSTR, w};

use super::util::{self, lparam_point, wide};
use super::{app, clipboard, dialogs, hud, pin, screen};
use crate::kit::annotation::Cursor;
use crate::kit::geom::{Point, Rect, Size};
use crate::kit::settings;
use crate::ui::capture::{CaptureView, Effect, Key, Mods, MouseButton, TextInput};
use crate::ui::chrome::Theme;

const CLASS: PCWSTR = w!("ShotlateOverlay");
const TICK_TIMER: usize = 1;
const WM_APP_SYNC_EDIT: u32 = WM_APP + 20;
const WM_APP_COMMIT_TEXT: u32 = WM_APP + 21;
const WM_APP_OCR_ESCAPE: u32 = WM_APP + 22;
const WM_APP_HOOK_ESCAPE: u32 = WM_APP + 23;
const EDIT_ID: usize = 100;
const OCR_EDIT_ID: usize = 101;

struct Overlay {
    hwnd: HWND,
    view: CaptureView,
    origin: POINT,
    scale: f32,
    timer: bool,
    tracking: bool,
}

struct Session {
    id: u64,
    overlays: Vec<Overlay>,
    owner: Option<usize>,
    previous: HWND,
    text_edit: Option<(usize, HWND, HFONT)>,
    ocr_edit: Option<(usize, HWND, HFONT)>,
    dark_brush: Option<HBRUSH>,
}

thread_local! {
    static SESSION: RefCell<Option<Session>> = const { RefCell::new(None) };
}

static SESSION_IDS: AtomicU64 = AtomicU64::new(1);

pub fn register() {
    let class = WNDCLASSEXW {
        cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
        style: CS_DBLCLKS,
        lpfnWndProc: Some(proc),
        lpszClassName: CLASS,
        hCursor: unsafe { LoadCursorW(None, IDC_CROSS).unwrap_or_default() },
        ..Default::default()
    };
    unsafe {
        RegisterClassExW(&class);
    }
}

pub fn is_active() -> bool {
    SESSION.with(|s| s.try_borrow().map_or(true, |s| s.is_some()))
}

/// Freezes every monitor and opens the overlays.
pub fn begin() {
    if is_active() {
        return;
    }
    let previous = unsafe { GetForegroundWindow() };
    // Window frames first, before anything of ours appears on screen.
    let windows = screen::window_rects();
    let monitors = screen::monitors();
    let theme = Theme { dark: util::is_dark_mode() };
    let models_ready = app::models_ready();
    let api_key_present = !super::secret::api_key().is_empty();
    let mut overlays = Vec::new();
    // One rect per overlay, in the same order: a monitor whose grab failed gets neither.
    let mut rects: Vec<RECT> = Vec::new();
    for m in &monitors {
        let Some(pix) = screen::grab(&m.rect) else { continue };
        let size = Size::new(pix.width() as f32 / m.scale, pix.height() as f32 / m.scale);
        let local: Vec<Rect> = windows
            .iter()
            .filter_map(|r| {
                let v = Rect::new(
                    (r.left - m.rect.left) as f32 / m.scale,
                    (r.top - m.rect.top) as f32 / m.scale,
                    (r.right - r.left) as f32 / m.scale,
                    (r.bottom - r.top) as f32 / m.scale,
                );
                let c = v.intersection(&Rect::new(0.0, 0.0, size.width, size.height));
                (!c.is_null() && c.width >= 20.0 && c.height >= 20.0).then_some(c)
            })
            .collect();
        let view = CaptureView::new(m.name.clone(), pix, size, local, theme, models_ready, api_key_present);
        overlays.push(Overlay { hwnd: HWND::default(), view, origin: POINT { x: m.rect.left, y: m.rect.top }, scale: m.scale, timer: false, tracking: false });
        rects.push(m.rect);
    }
    if overlays.is_empty() {
        dialogs::warn(None, "截图失败", "没有拿到屏幕画面。");
        return;
    }
    SESSION.with(|s| {
        *s.borrow_mut() = Some(Session {
            id: SESSION_IDS.fetch_add(1, Ordering::Relaxed),
            overlays,
            owner: None,
            previous,
            text_edit: None,
            ocr_edit: None,
            dark_brush: None,
        })
    });
    // Create the windows outside the borrow: creation sends messages to `proc`.
    let mut hwnds = Vec::new();
    for r in &rects {
        let hwnd = unsafe {
            CreateWindowExW(WS_EX_TOPMOST | WS_EX_TOOLWINDOW, CLASS, w!("Shotlate"), WS_POPUP, r.left, r.top, r.right - r.left, r.bottom - r.top, None, None, None, None)
        };
        let hwnd = hwnd.unwrap_or_default();
        // Keys go straight to the overlay, not to an IME: "1" must pick a tool even with Chinese input on.
        unsafe {
            ImmAssociateContext(hwnd, HIMC::default());
        }
        hwnds.push(hwnd);
    }
    with(|s| {
        for (o, h) in s.overlays.iter_mut().zip(&hwnds) {
            o.hwnd = *h;
        }
    });
    let mut cursor = POINT::default();
    unsafe {
        let _ = GetCursorPos(&mut cursor);
    }
    let active = rects.iter().position(|r| cursor.x >= r.left && cursor.x < r.right && cursor.y >= r.top && cursor.y < r.bottom).unwrap_or(0);
    for (i, h) in hwnds.iter().enumerate() {
        unsafe {
            let _ = ShowWindow(*h, if i == active { SW_SHOW } else { SW_SHOWNA });
        }
    }
    if let Some(h) = hwnds.get(active) {
        unsafe {
            let _ = SetForegroundWindow(*h);
            let _ = SetFocus(Some(*h));
        }
        install_escape_hook(*h);
        let (local, scale) = with(|s| {
            let o = &s.overlays[active];
            (Point::new((cursor.x - o.origin.x) as f32 / o.scale, (cursor.y - o.origin.y) as f32 / o.scale), o.scale)
        })
        .unwrap_or((Point::ZERO, 1.0));
        let _ = scale;
        view_call(active, |v| v.prime_cursor(local));
    }
}

fn count() -> usize {
    with(|s| s.overlays.len()).unwrap_or(0)
}

/// Borrows the session for a short call; None when there is none or it is busy (re-entrant message).
fn with<R>(f: impl FnOnce(&mut Session) -> R) -> Option<R> {
    SESSION.with(|s| match s.try_borrow_mut() {
        Ok(mut guard) => guard.as_mut().map(f),
        Err(_) => None,
    })
}

fn index_of(hwnd: HWND) -> Option<usize> {
    with(|s| s.overlays.iter().position(|o| o.hwnd == hwnd)).flatten()
}

/// Calls the view of overlay `i`, then repaints what changed and carries out its effects.
fn view_call(i: usize, f: impl FnOnce(&mut CaptureView)) {
    let effects = with(|s| {
        let o = s.overlays.get_mut(i)?;
        f(&mut o.view);
        let effects = o.view.drain_effects();
        if o.view.has_selection() {
            s.owner = Some(i);
        } else if s.owner == Some(i) {
            s.owner = None;
        }
        Some(effects)
    })
    .flatten();
    refresh(i);
    for e in effects.unwrap_or_default() {
        apply(i, e);
    }
}

/// Invalidates the changed region and starts or stops the tick timer.
fn refresh(i: usize) {
    let Some((hwnd, dirty, wants, timer)) = with(|s| {
        let o = s.overlays.get_mut(i)?;
        let dirty = o.view.take_dirty();
        let wants = o.view.wants_ticks();
        let timer = o.timer;
        o.timer = wants;
        Some((o.hwnd, dirty, wants, timer))
    })
    .flatten() else {
        return;
    };
    unsafe {
        if let Some((x, y, w, h)) = dirty {
            let r = RECT { left: x, top: y, right: x + w as i32, bottom: y + h as i32 };
            let _ = InvalidateRect(Some(hwnd), Some(&r), false);
        }
        if wants && !timer {
            SetTimer(Some(hwnd), TICK_TIMER, 30, None);
        } else if !wants && timer {
            let _ = KillTimer(Some(hwnd), TICK_TIMER);
        }
    }
}

fn mods() -> Mods {
    let down = |vk: u16| unsafe { GetKeyState(vk as i32) } < 0;
    Mods { shift: down(VK_SHIFT.0), ctrl: down(VK_CONTROL.0), alt: down(VK_MENU.0) }
}

fn key_of(vk: u32) -> Key {
    match vk {
        v if v == VK_ESCAPE.0 as u32 => Key::Escape,
        v if v == VK_RETURN.0 as u32 => Key::Enter,
        v if v == VK_BACK.0 as u32 => Key::Backspace,
        v if v == VK_DELETE.0 as u32 => Key::Delete,
        v if v == VK_LEFT.0 as u32 => Key::Left,
        v if v == VK_RIGHT.0 as u32 => Key::Right,
        v if v == VK_UP.0 as u32 => Key::Up,
        v if v == VK_DOWN.0 as u32 => Key::Down,
        0x30..=0x39 => Key::Char(char::from_u32(vk).unwrap_or('0')),
        0x41..=0x5A => Key::Char(char::from_u32(vk + 32).unwrap_or('a')),
        // Numpad digits act like the top row.
        0x60..=0x69 => Key::Char(char::from_u32(vk - 0x60 + 0x30).unwrap_or('0')),
        _ => Key::Other,
    }
}

fn point_in(i: usize, l: LPARAM) -> Option<Point> {
    let (x, y) = lparam_point(l);
    with(|s| s.overlays.get(i).map(|o| Point::new(x as f32 / o.scale, y as f32 / o.scale))).flatten()
}

/// Whether overlay `i` may take input: no other monitor owns the selection.
fn can_interact(i: usize) -> bool {
    with(|s| s.owner.is_none_or(|o| o == i)).unwrap_or(false)
}

fn cursor_handle(c: Cursor) -> PCWSTR {
    match c {
        Cursor::Arrow => IDC_ARROW,
        Cursor::Crosshair => IDC_CROSS,
        Cursor::IBeam => IDC_IBEAM,
        Cursor::OpenHand | Cursor::ClosedHand => IDC_SIZEALL,
        Cursor::ResizeLeftRight => IDC_SIZEWE,
        Cursor::ResizeUpDown => IDC_SIZENS,
        Cursor::ResizeDiagonalDown => IDC_SIZENWSE,
        Cursor::ResizeDiagonalUp => IDC_SIZENESW,
    }
}

/// Shows the view's cursor. WM_SETCURSOR alone isn't enough: it isn't sent while the mouse is captured
/// (dragging), and it arrives before the move that changes the cursor.
fn set_cursor(i: usize) {
    let c = with(|s| s.overlays.get(i).map(|o| o.view.cursor())).flatten().unwrap_or(Cursor::Crosshair);
    unsafe {
        SetCursor(LoadCursorW(None, cursor_handle(c)).ok());
    }
}

unsafe extern "system" fn proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let Some(i) = index_of(hwnd) else {
        return unsafe { DefWindowProcW(hwnd, msg, wp, lp) };
    };
    match msg {
        WM_ERASEBKGND => return LRESULT(1),
        WM_PAINT => {
            paint(i, hwnd);
            return LRESULT(0);
        }
        WM_MOUSEMOVE => {
            let track = with(|s| s.overlays.get_mut(i).map(|o| !std::mem::replace(&mut o.tracking, true))).flatten().unwrap_or(false);
            if track {
                let mut t = TRACKMOUSEEVENT { cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: hwnd, dwHoverTime: 0 };
                unsafe {
                    let _ = TrackMouseEvent(&mut t);
                }
            }
            if let Some(p) = point_in(i, lp) {
                if can_interact(i) {
                    let m = mods();
                    view_call(i, |v| v.mouse_move(p, m));
                    set_cursor(i);
                } else {
                    view_call(i, |v| v.hide_magnifier());
                }
            }
            return LRESULT(0);
        }
        WM_MOUSELEAVE => {
            with(|s| s.overlays.get_mut(i).map(|o| o.tracking = false));
            view_call(i, |v| v.hide_magnifier());
            return LRESULT(0);
        }
        WM_LBUTTONDOWN | WM_LBUTTONDBLCLK | WM_RBUTTONDOWN => {
            if !can_interact(i) {
                return LRESULT(0);
            }
            unsafe {
                if msg != WM_RBUTTONDOWN {
                    SetCapture(hwnd);
                }
                // Typing a text annotation keeps the focus (clicking the style bar mustn't end it); the OCR text
                // box gives it back, so tool keys like "1" work again instead of typing into the panel.
                let focus = GetFocus();
                if focus != hwnd && !is_text_edit(focus) {
                    let _ = SetFocus(Some(hwnd));
                }
            }
            if let Some(p) = point_in(i, lp) {
                let (button, clicks) = match msg {
                    WM_RBUTTONDOWN => (MouseButton::Right, 1),
                    WM_LBUTTONDBLCLK => (MouseButton::Left, 2),
                    _ => (MouseButton::Left, 1),
                };
                let m = mods();
                view_call(i, |v| v.mouse_down(p, button, clicks, m));
                set_cursor(i);
            }
            return LRESULT(0);
        }
        WM_LBUTTONUP => {
            unsafe {
                let _ = ReleaseCapture();
            }
            if let Some(p) = point_in(i, lp) {
                let m = mods();
                view_call(i, |v| v.mouse_up(p, m));
                set_cursor(i);
            }
            return LRESULT(0);
        }
        WM_MOUSEWHEEL => {
            let delta = util::hiword(wp.0) as i16 as f32 / 120.0;
            let m = mods();
            view_call(target(i), |v| v.wheel(delta, m));
            return LRESULT(0);
        }
        WM_KEYDOWN | WM_SYSKEYDOWN => {
            let vk = wp.0 as u32;
            if msg == WM_SYSKEYDOWN && vk == VK_F4.0 as u32 {
                // Alt+F4: DefWindowProc turns it into WM_CLOSE, which ends the capture.
                return unsafe { DefWindowProcW(hwnd, msg, wp, lp) };
            }
            let t = target(i);
            if vk == VK_SHIFT.0 as u32 {
                let m = mods();
                view_call(t, |v| v.modifiers_changed(m));
                return LRESULT(0);
            }
            let (key, m) = (key_of(vk), mods());
            view_call(t, |v| v.key_down(key, m));
            return LRESULT(0);
        }
        WM_KEYUP | WM_SYSKEYUP => {
            if wp.0 as u32 == VK_SHIFT.0 as u32 {
                let m = mods();
                view_call(target(i), |v| v.modifiers_changed(m));
            }
            return LRESULT(0);
        }
        WM_SETCURSOR => {
            // Children (the OCR text box) ask their parent first; let them keep their I-beam.
            if wp.0 == hwnd.0 as usize && util::loword(lp.0 as usize) as u32 == HTCLIENT {
                set_cursor(i);
                return LRESULT(1);
            }
        }
        WM_MOUSEACTIVATE => {
            // Another monitor owns the selection: a click here must not take the focus (and a text box's typing)
            // away from it.
            if !can_interact(i) {
                return LRESULT(MA_NOACTIVATE as isize);
            }
        }
        // Alt+letter would otherwise beep (DefWindowProc looks for a menu mnemonic).
        WM_SYSCHAR => return LRESULT(0),
        WM_TIMER => {
            let now = Instant::now();
            view_call(i, |v| v.tick(now));
            return LRESULT(0);
        }
        WM_COMMAND => {
            let code = util::hiword(wp.0) as u32;
            let id = util::loword(wp.0) as usize;
            if code == EN_CHANGE {
                if id == EDIT_ID {
                    sync_text(i);
                } else if id == OCR_EDIT_ID {
                    let text = edit_text(HWND(lp.0 as *mut _)).replace("\r\n", "\n");
                    view_call(i, |v| v.ocr_text_changed(&text));
                }
            }
            return LRESULT(0);
        }
        WM_APP_SYNC_EDIT => {
            sync_text(i);
            return LRESULT(0);
        }
        WM_APP_COMMIT_TEXT => {
            view_call(i, |v| v.commit_text());
            return LRESULT(0);
        }
        WM_APP_OCR_ESCAPE => {
            view_call(i, |v| v.key_down(Key::Escape, Mods::NONE));
            return LRESULT(0);
        }
        WM_APP_HOOK_ESCAPE => {
            // Esc that reached us through the keyboard hook: the overlay had lost the keyboard.
            let t = target(i);
            view_call(t, |v| v.key_down(Key::Escape, Mods::NONE));
            // Still capturing (Esc only stepped back): try to take the keyboard back.
            if let Some(h) = with(|s| s.overlays.get(t).map(|o| o.hwnd)).flatten() {
                let ok = unsafe { SetForegroundWindow(h) }.as_bool();
                util::trace(|| format!("hook Esc; overlay back in front: {ok}"));
            }
            return LRESULT(0);
        }
        WM_CTLCOLOREDIT => {
            if util::is_dark_mode() {
                let hdc = HDC(wp.0 as *mut _);
                unsafe {
                    SetTextColor(hdc, COLORREF(0x00EE_EEEE));
                    SetBkColor(hdc, COLORREF(0x001F_1F1F));
                }
                let brush = with(|s| *s.dark_brush.get_or_insert_with(|| unsafe { CreateSolidBrush(COLORREF(0x001F_1F1F)) })).unwrap_or_default();
                return LRESULT(brush.0 as isize);
            }
        }
        WM_CLOSE => {
            end(None);
            return LRESULT(0);
        }
        _ => {}
    }
    unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
}

/// Keyboard and wheel go to the monitor that owns the selection, wherever the focus is.
fn target(i: usize) -> usize {
    with(|s| s.owner.unwrap_or(i)).unwrap_or(i)
}

fn is_text_edit(h: HWND) -> bool {
    with(|s| s.text_edit.is_some_and(|e| e.1 == h)).unwrap_or(false)
}

fn paint(i: usize, hwnd: HWND) {
    let mut ps = PAINTSTRUCT::default();
    let hdc = unsafe { BeginPaint(hwnd, &mut ps) };
    let r = ps.rcPaint;
    let (w, h) = (r.right - r.left, r.bottom - r.top);
    if w > 0 && h > 0 {
        let pix = SESSION.with(|s| s.try_borrow().ok().and_then(|s| s.as_ref().and_then(|s| s.overlays.get(i)).and_then(|o| o.view.render_region(r.left, r.top, w as u32, h as u32))));
        if let Some(pix) = pix {
            util::blit(hdc, r.left, r.top, &pix);
        }
    }
    unsafe {
        let _ = EndPaint(hwnd, &ps);
    }
}

// MARK: Text boxes

fn edit_text(h: HWND) -> String {
    let len = unsafe { GetWindowTextLengthW(h) };
    let mut buf = vec![0u16; len as usize + 1];
    unsafe {
        GetWindowTextW(h, &mut buf);
    }
    util::from_wide(&buf)
}

fn sync_text(i: usize) {
    let Some(edit) = with(|s| s.text_edit.map(|e| e.1)).flatten() else { return };
    let raw = edit_text(edit);
    let (mut start, mut end) = (0u32, 0u32);
    unsafe {
        SendMessageW(edit, EM_GETSEL, Some(WPARAM(&mut start as *mut u32 as usize)), Some(LPARAM(&mut end as *mut u32 as isize)));
    }
    // The EDIT counts line breaks as "\r\n"; the view gets "\n", so every "\r" before an offset shifts it by one.
    let offset = |u16_index: u32| {
        let byte = util::utf16_to_byte(&raw, u16_index as usize);
        byte - raw[..byte].matches('\r').count()
    };
    let (a, b) = (offset(start), offset(end));
    let text = raw.replace('\r', "");
    let composition = composition_string(edit);
    view_call(i, |v| {
        v.text_changed(&text, a, b);
        v.composition_changed(&composition);
    });
}

/// What the IME is composing in `edit` right now (empty when nothing is).
fn composition_string(edit: HWND) -> String {
    unsafe {
        let himc = ImmGetContext(edit);
        if himc.0.is_null() {
            return String::new();
        }
        let bytes = ImmGetCompositionStringW(himc, GCS_COMPSTR, None, 0);
        let mut out = String::new();
        if bytes > 0 {
            let mut buf = vec![0u16; bytes as usize / 2];
            let n = ImmGetCompositionStringW(himc, GCS_COMPSTR, Some(buf.as_mut_ptr() as *mut _), bytes as u32);
            if n > 0 {
                out = String::from_utf16_lossy(&buf[..n as usize / 2]);
            }
        }
        let _ = ImmReleaseContext(edit, himc);
        out
    }
}

fn ui_font(px: i32) -> HFONT {
    unsafe {
        CreateFontW(
            -px.max(8),
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

unsafe extern "system" fn text_subclass(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM, _id: usize, parent: usize) -> LRESULT {
    let parent = HWND(parent as *mut _);
    let ctrl = unsafe { GetKeyState(VK_CONTROL.0 as i32) } < 0;
    match msg {
        WM_KEYDOWN if wp.0 as u32 == VK_ESCAPE.0 as u32 || (wp.0 as u32 == VK_RETURN.0 as u32 && ctrl) => {
            unsafe {
                let _ = PostMessageW(Some(parent), WM_APP_COMMIT_TEXT, WPARAM(0), LPARAM(0));
            }
            return LRESULT(0);
        }
        // Swallow the characters of the keys handled above (no beep, no stray newline) and Tab.
        WM_CHAR if wp.0 == 0x1B || (wp.0 == 0x0A && ctrl) || wp.0 == 0x09 => return LRESULT(0),
        WM_KEYDOWN if wp.0 as u32 == VK_TAB.0 as u32 => return LRESULT(0),
        _ => {}
    }
    let r = unsafe { DefSubclassProc(hwnd, msg, wp, lp) };
    if matches!(msg, WM_KEYDOWN | WM_KEYUP | WM_CHAR | WM_LBUTTONUP | WM_IME_COMPOSITION | WM_IME_ENDCOMPOSITION) {
        unsafe {
            let _ = PostMessageW(Some(parent), WM_APP_SYNC_EDIT, WPARAM(0), LPARAM(0));
        }
    }
    // Not during a composition: moving the composition window then can end it.
    if matches!(msg, WM_SETFOCUS | WM_IME_STARTCOMPOSITION) {
        apply_ime(hwnd);
    }
    r
}

unsafe extern "system" fn ocr_subclass(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM, _id: usize, parent: usize) -> LRESULT {
    if msg == WM_KEYDOWN && wp.0 as u32 == VK_ESCAPE.0 as u32 {
        unsafe {
            let _ = PostMessageW(Some(HWND(parent as *mut _)), WM_APP_OCR_ESCAPE, WPARAM(0), LPARAM(0));
        }
        return LRESULT(0);
    }
    // Esc's character, and Ctrl+A's (0x01), which the EDIT would otherwise beep at or insert.
    if msg == WM_CHAR && (wp.0 == 0x1B || wp.0 == 0x01) {
        return LRESULT(0);
    }
    // Ctrl+A selects all (a multiline EDIT doesn't by default).
    if msg == WM_KEYDOWN && wp.0 == 0x41 && unsafe { GetKeyState(VK_CONTROL.0 as i32) } < 0 {
        unsafe {
            SendMessageW(hwnd, EM_SETSEL, Some(WPARAM(0)), Some(LPARAM(-1)));
        }
        return LRESULT(0);
    }
    unsafe { DefSubclassProc(hwnd, msg, wp, lp) }
}

thread_local! {
    /// Where the IME windows belong: the drawn caret in the hidden box's client coordinates, and the line height.
    static IME_CARET: Cell<(POINT, i32)> = const { Cell::new((POINT { x: 0, y: 0 }, 0)) };
}

fn place_ime(edit: HWND, caret_px: (i32, i32), line_px: i32) {
    // The box sits on the drawn caret, so IMEs that follow the box's own caret or text position (TSF ones such as
    // Microsoft Pinyin ignore ImmSetCompositionWindow) put their windows there. It's 2 px wide and nearly
    // transparent; ES_AUTOHSCROLL keeps its caret at its left edge.
    unsafe {
        let _ = SetWindowPos(edit, None, caret_px.0, caret_px.1, 2, line_px.max(8), SWP_NOZORDER | SWP_NOACTIVATE);
    }
    IME_CARET.with(|c| c.set((POINT { x: 0, y: 0 }, line_px)));
    apply_ime(edit);
}

/// Points IMM-based IMEs at the box's origin (TSF ones use the box's position, see `place_ime`).
fn apply_ime(edit: HWND) {
    let (pos, line_px) = IME_CARET.with(|c| c.get());
    unsafe {
        let himc = ImmGetContext(edit);
        if himc.0.is_null() {
            return;
        }
        let comp = COMPOSITIONFORM { dwStyle: CFS_FORCE_POSITION, ptCurrentPos: pos, ..Default::default() };
        let _ = ImmSetCompositionWindow(himc, &comp);
        let cand = CANDIDATEFORM { dwIndex: 0, dwStyle: CFS_CANDIDATEPOS, ptCurrentPos: POINT { x: pos.x, y: pos.y + line_px }, ..Default::default() };
        let _ = ImmSetCandidateWindow(himc, &cand);
        let _ = ImmReleaseContext(edit, himc);
    }
}

fn text_input(i: usize, input: Option<TextInput>) {
    let Some((hwnd, scale)) = with(|s| s.overlays.get(i).map(|o| (o.hwnd, o.scale))).flatten() else { return };
    match input {
        None => {
            if let Some((_, edit, font)) = with(|s| s.text_edit.take()).flatten() {
                unsafe {
                    if GetFocus() == edit {
                        let _ = SetFocus(Some(hwnd));
                    }
                    let _ = DestroyWindow(edit);
                    let _ = DeleteObject(HGDIOBJ(font.0));
                }
            }
        }
        Some(input) => {
            let existing = with(|s| s.text_edit.map(|e| e.1)).flatten();
            let edit = match existing {
                Some(e) => e,
                None => {
                    let font = ui_font((input.font_size * scale).round() as i32);
                    let style = WS_CHILD | WS_VISIBLE | WINDOW_STYLE((ES_MULTILINE | ES_AUTOVSCROLL | ES_AUTOHSCROLL | ES_WANTRETURN) as u32);
                    // A layered child (Windows 8+, declared in the manifest) can be made invisible yet keep focus and IME.
                    let Ok(edit) = (unsafe {
                        CreateWindowExW(WS_EX_LAYERED, w!("EDIT"), w!(""), style, 0, 0, 2, 20, Some(hwnd), Some(HMENU(EDIT_ID as *mut _)), None, None)
                    }) else {
                        return;
                    };
                    unsafe {
                        let _ = SetLayeredWindowAttributes(edit, COLORREF(0), 1, LWA_ALPHA);
                        SendMessageW(edit, WM_SETFONT, Some(WPARAM(font.0 as usize)), Some(LPARAM(0)));
                        let _ = SetWindowSubclass(edit, Some(text_subclass), 1, hwnd.0 as usize);
                    }
                    with(|s| s.text_edit = Some((i, edit, font)));
                    edit
                }
            };
            if let Some(initial) = &input.initial {
                let t = wide(&initial.replace('\n', "\r\n"));
                unsafe {
                    let _ = SetWindowTextW(edit, PCWSTR(t.as_ptr()));
                    SendMessageW(edit, EM_SETSEL, Some(WPARAM(t.len().saturating_sub(1))), Some(LPARAM(t.len().saturating_sub(1) as isize)));
                    let _ = SetFocus(Some(edit));
                }
            }
            let caret = ((input.caret.x * scale) as i32, (input.caret.y * scale) as i32);
            place_ime(edit, caret, (input.caret.height * scale) as i32);
        }
    }
}

fn ocr_panel(i: usize, panel: Option<(Rect, String)>) {
    let Some((hwnd, scale)) = with(|s| s.overlays.get(i).map(|o| (o.hwnd, o.scale))).flatten() else { return };
    match panel {
        None => {
            if let Some((_, edit, font)) = with(|s| s.ocr_edit.take()).flatten() {
                unsafe {
                    if GetFocus() == edit {
                        let _ = SetFocus(Some(hwnd));
                    }
                    let _ = DestroyWindow(edit);
                    let _ = DeleteObject(HGDIOBJ(font.0));
                }
            }
        }
        Some((rect, text)) => {
            // Inset a little so the drawn rounded background shows around the control.
            let r = rect.inset(5.0, 5.0).scaled(scale);
            let existing = with(|s| s.ocr_edit.map(|e| e.1)).flatten();
            match existing {
                Some(edit) => unsafe {
                    let _ = MoveWindow(edit, r.x as i32, r.y as i32, r.width as i32, r.height as i32, true);
                    // The view's text follows the box's edits, so it only differs after a new recognition.
                    if edit_text(edit).replace("\r\n", "\n") != text {
                        let t = wide(&text.replace('\n', "\r\n"));
                        let _ = SetWindowTextW(edit, PCWSTR(t.as_ptr()));
                    }
                },
                None => {
                    let font = ui_font((13.0 * scale).round() as i32);
                    let style = WS_CHILD | WS_VISIBLE | WS_VSCROLL | WINDOW_STYLE((ES_MULTILINE | ES_AUTOVSCROLL | ES_WANTRETURN | ES_NOHIDESEL) as u32);
                    let Ok(edit) = (unsafe {
                        CreateWindowExW(
                            WINDOW_EX_STYLE(0),
                            w!("EDIT"),
                            w!(""),
                            style,
                            r.x as i32,
                            r.y as i32,
                            r.width as i32,
                            r.height as i32,
                            Some(hwnd),
                            Some(HMENU(OCR_EDIT_ID as *mut _)),
                            None,
                            None,
                        )
                    }) else {
                        return;
                    };
                    let t = wide(&text.replace('\n', "\r\n"));
                    unsafe {
                        SendMessageW(edit, WM_SETFONT, Some(WPARAM(font.0 as usize)), Some(LPARAM(0)));
                        let _ = SetWindowTextW(edit, PCWSTR(t.as_ptr()));
                        let _ = SetWindowSubclass(edit, Some(ocr_subclass), 2, hwnd.0 as usize);
                    }
                    with(|s| s.ocr_edit = Some((i, edit, font)));
                }
            }
        }
    }
}

// MARK: Effects

fn session_id() -> Option<u64> {
    with(|s| s.id)
}

fn apply(i: usize, effect: Effect) {
    let Some(hwnd) = with(|s| s.overlays.get(i).map(|o| o.hwnd)).flatten() else { return };
    match effect {
        Effect::Close => end(None),
        Effect::Copy(img) => {
            let ok = clipboard::set_image(util::app_window(), &img);
            end(Some(if ok { "已复制到剪贴板".into() } else { "复制失败：剪贴板被其他程序占用".into() }));
        }
        Effect::Save(img, format) => {
            let dir = app::save_directory();
            match crate::kit::export::save(&img, format, &dir, app::local_time()) {
                Ok(path) => end(Some(format!("已保存到 {}", path.display()))),
                Err(e) => view_call(i, |v| v.show_message(&format!("保存失败：{e}"))),
            }
        }
        Effect::SaveAs(img, format) => {
            // The overlay steps aside while the dialog is up, and comes back if it is cancelled.
            let windows: Vec<HWND> = with(|s| s.overlays.iter().map(|o| o.hwnd).collect()).unwrap_or_default();
            for w in &windows {
                unsafe {
                    let _ = ShowWindow(*w, SW_HIDE);
                }
            }
            let name = crate::kit::export::default_file_name(format, app::local_time());
            match dialogs::save_as(None, &app::save_directory(), &name, format) {
                Some(path) => match crate::kit::export::encode(&img, format).and_then(|b| std::fs::write(&path, b).map_err(|e| e.to_string())) {
                    Ok(()) => end(Some(format!("已保存到 {}", path.display()))),
                    Err(e) => {
                        end(None);
                        dialogs::warn(None, "保存失败", &e);
                    }
                },
                None => {
                    for w in &windows {
                        unsafe {
                            let _ = ShowWindow(*w, SW_SHOWNA);
                        }
                    }
                    unsafe {
                        let _ = SetForegroundWindow(hwnd);
                        let _ = SetFocus(Some(hwnd));
                    }
                }
            }
        }
        Effect::Pin(img, rect) => {
            let Some((origin, scale)) = with(|s| s.overlays.get(i).map(|o| (o.origin, o.scale))).flatten() else { return };
            let screen = POINT { x: origin.x + (rect.x * scale).round() as i32, y: origin.y + (rect.y * scale).round() as i32 };
            end(None);
            pin::create(img, screen);
        }
        Effect::CopyText(text) => {
            clipboard::set_text(util::app_window(), &text);
        }
        Effect::CopyOcrPanelText => {
            if let Some(edit) = with(|s| s.ocr_edit.map(|e| e.1)).flatten() {
                clipboard::set_text(util::app_window(), &edit_text(edit));
            }
        }
        Effect::Recognize(img) => {
            let Some(id) = session_id() else { return };
            std::thread::spawn(move || {
                let lines = util::guarded(move || {
                    let result = app::recognize(&img);
                    result.map(|lines| lines.into_iter().map(|l| (l.text, l.rect)).collect::<Vec<_>>())
                })
                .and_then(|r| r);
                util::run_on_ui(move || {
                    if session_id() == Some(id) {
                        view_call(i, |v| v.recognition_finished(lines));
                    }
                });
            });
        }
        Effect::ModelsMissing => {
            app::offer_model_download(Some(hwnd));
        }
        Effect::Translate(items) => {
            let Some(id) = session_id() else { return };
            let config = app::translation_config();
            std::thread::spawn(move || {
                let result = util::guarded(|| app::translation_cache().translate(&items, &config, |missing| crate::kit::translator::translate(missing, &config)))
                    .unwrap_or_else(|e| Err(crate::kit::translator::TranslationError::BadResponse(e)));
                util::run_on_ui(move || {
                    if session_id() == Some(id) {
                        view_call(i, |v| v.translation_finished(result));
                    }
                });
            });
        }
        Effect::PickColor(current) => {
            if let Some(c) = dialogs::pick_color(hwnd, current) {
                view_call(i, |v| v.apply_custom_color(c));
            }
        }
        Effect::WarpCursor(p) => {
            if let Some((origin, scale)) = with(|s| s.overlays.get(i).map(|o| (o.origin, o.scale))).flatten() {
                unsafe {
                    let _ = SetCursorPos(origin.x + (p.x * scale).round() as i32, origin.y + (p.y * scale).round() as i32);
                }
            }
        }
        Effect::TextInput(input) => text_input(i, input),
        Effect::OcrPanel(panel) => ocr_panel(i, panel),
        Effect::Beep => unsafe {
            let _ = MessageBeep(MB_OK);
        },
    }
}

/// The recognition models finished downloading: open overlays may use them now.
pub fn models_became_ready() {
    let n = count();
    for i in 0..n {
        view_call(i, |v| {
            v.set_models_ready(true);
            v.show_message("文字识别组件已下载，可以识别和翻译了");
        });
    }
}

// MARK: Escape hook

/// The low-level keyboard hook, installed only while a capture is on screen.
static ESCAPE_HOOK: AtomicIsize = AtomicIsize::new(0);
/// The overlay that gets Esc from the hook.
static ESCAPE_TARGET: AtomicIsize = AtomicIsize::new(0);

/// Esc must always get out of a capture, even when the overlay has lost the keyboard: Alt+Esc (Esc
/// pressed before letting go of Alt in the Alt+A shortcut) is a system shortcut that never reaches the
/// app and sends the overlay behind the previous window. The hook catches Esc before the system does.
fn install_escape_hook(overlay: HWND) {
    ESCAPE_TARGET.store(overlay.0 as isize, Ordering::SeqCst);
    if ESCAPE_HOOK.load(Ordering::SeqCst) != 0 {
        return;
    }
    let module = unsafe { windows::Win32::System::LibraryLoader::GetModuleHandleW(None) }.ok();
    match unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(escape_hook), module.map(|m| m.into()), 0) } {
        Ok(h) => ESCAPE_HOOK.store(h.0 as isize, Ordering::SeqCst),
        Err(e) => util::trace(|| format!("keyboard hook failed: {e}")),
    }
}

fn remove_escape_hook() {
    ESCAPE_TARGET.store(0, Ordering::SeqCst);
    let h = ESCAPE_HOOK.swap(0, Ordering::SeqCst);
    if h != 0 {
        unsafe {
            let _ = UnhookWindowsHookEx(HHOOK(h as *mut _));
        }
    }
}

/// Runs for every key in the system while capturing, so it only looks at Esc and never blocks.
unsafe extern "system" fn escape_hook(code: i32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 && matches!(wp.0 as u32, WM_KEYDOWN | WM_SYSKEYDOWN) {
        let key = unsafe { &*(lp.0 as *const KBDLLHOOKSTRUCT) };
        let target = ESCAPE_TARGET.load(Ordering::SeqCst);
        if key.vkCode == VK_ESCAPE.0 as u32 && target != 0 {
            let alt = key.flags.0 & LLKHF_ALTDOWN.0 != 0;
            // Normal case: the overlay has the keyboard and handles Esc itself (text box, OCR panel…).
            let ours = {
                let fg = unsafe { GetForegroundWindow() };
                let mut buf = [0u16; 32];
                let n = unsafe { GetClassNameW(fg, &mut buf) } as usize;
                String::from_utf16_lossy(&buf[..n]) == "ShotlateOverlay"
            };
            if alt || !ours {
                unsafe {
                    let _ = PostMessageW(Some(HWND(target as *mut _)), WM_APP_HOOK_ESCAPE, WPARAM(0), LPARAM(0));
                }
                return LRESULT(1);
            }
        }
    }
    unsafe { CallNextHookEx(None, code, wp, lp) }
}

/// Closes the session, then shows `message` in the HUD.
pub fn end(message: Option<String>) {
    let Some(session) = SESSION.with(|s| s.try_borrow_mut().ok().and_then(|mut s| s.take())) else { return };
    remove_escape_hook();
    unsafe {
        for e in [session.text_edit, session.ocr_edit].into_iter().flatten() {
            let _ = DestroyWindow(e.1);
            let _ = DeleteObject(HGDIOBJ(e.2.0));
        }
        for o in &session.overlays {
            let _ = DestroyWindow(o.hwnd);
        }
        if let Some(b) = session.dark_brush {
            let _ = DeleteObject(HGDIOBJ(b.0));
        }
        // From the tray menu the previous foreground is our own hidden window; the system picks the next
        // window itself then, which is what the user was in before.
        let mut pid = 0u32;
        GetWindowThreadProcessId(session.previous, Some(&mut pid));
        if !session.previous.0.is_null() && pid != std::process::id() {
            let _ = SetForegroundWindow(session.previous);
        }
    }
    let _ = settings::get();
    if let Some(m) = message {
        hud::show(&m);
    }
}
