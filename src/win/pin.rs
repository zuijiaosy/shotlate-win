//! Screenshots floating above other windows (Pin.swift). Drag to move, wheel to zoom, Ctrl+wheel for opacity,
//! Esc to close, right-click for the menu.

use std::cell::RefCell;
use std::time::{Duration, Instant};

use tiny_skia::{FilterQuality, Pixmap, Transform};
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{BeginPaint, EndPaint, InvalidateRect, PAINTSTRUCT};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, ReleaseCapture, SetCapture, VK_CONTROL, VK_DOWN, VK_ESCAPE, VK_LEFT, VK_RIGHT, VK_SHIFT, VK_UP};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{PCWSTR, w};

use super::util::{self, lparam_point, wide};
use super::{app, clipboard, hud};
use crate::kit::color::{Color, SELECTION_BLUE};
use crate::kit::geom::{Point, Rect};
use crate::kit::image::RgbaImage;
use crate::kit::textselection::{GlyphLine, TextLayout, TextPosition, TextSpan};
use crate::render::canvas::Canvas;
use crate::ui::chrome;

const CLASS: PCWSTR = w!("ShotlatePin");
const FLASH_TIMER: usize = 1;

struct Pin {
    hwnd: HWND,
    image: RgbaImage,
    pixels: Pixmap,
    zoom: f32,
    opacity: f32,
    flash: Option<(String, Instant)>,
    drag: Option<(POINT, POINT)>,
    active: bool,
    /// The picture's recognized text (picture pixels), once recognized; None until then or without models.
    text: Option<TextLayout>,
    selection: Option<TextSpan>,
    selecting: bool,
    /// For counting double and triple clicks: when, where, how many.
    clicks: (Instant, POINT, u32),
}

struct Pins {
    pins: Vec<Pin>,
    hidden: bool,
}

thread_local! {
    static PINS: RefCell<Pins> = const { RefCell::new(Pins { pins: Vec::new(), hidden: false }) };
}

fn with<R>(f: impl FnOnce(&mut Pins) -> R) -> Option<R> {
    PINS.with(|p| p.try_borrow_mut().ok().map(|mut p| f(&mut p)))
}

fn with_pin<R>(hwnd: HWND, f: impl FnOnce(&mut Pin) -> R) -> Option<R> {
    with(|p| p.pins.iter_mut().find(|p| p.hwnd == hwnd).map(f)).flatten()
}

pub fn register() {
    let class = WNDCLASSEXW {
        cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
        style: CS_DROPSHADOW | CS_DBLCLKS,
        lpfnWndProc: Some(proc),
        lpszClassName: CLASS,
        hCursor: unsafe { LoadCursorW(None, IDC_SIZEALL).unwrap_or_default() },
        ..Default::default()
    };
    unsafe {
        RegisterClassExW(&class);
    }
}

pub fn has_pins() -> bool {
    with(|p| !p.pins.is_empty()).unwrap_or(false)
}

pub fn is_hiding_all() -> bool {
    with(|p| p.hidden).unwrap_or(false)
}

/// Floats `image` with its top-left at `origin` (screen pixels), at 100%.
pub fn create(image: RgbaImage, origin: POINT) {
    // A new pin while the others are hidden brings them back, so nothing stays hidden by surprise.
    if is_hiding_all() {
        toggle_hidden();
    }
    let mut pixels = match Pixmap::new(image.width.max(1), image.height.max(1)) {
        Some(p) => p,
        None => return,
    };
    // Opaque screenshot pixels: straight and premultiplied RGBA are the same bytes.
    let n = pixels.data().len().min(image.data.len());
    pixels.data_mut()[..n].copy_from_slice(&image.data[..n]);
    let (w, h) = (image.width as i32, image.height as i32);
    let hwnd = unsafe {
        CreateWindowExW(WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_LAYERED, CLASS, w!("Shotlate 贴图"), WS_POPUP, origin.x, origin.y, w, h, None, None, None, None)
    };
    let Ok(hwnd) = hwnd else { return };
    unsafe {
        let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), 255, LWA_ALPHA);
    }
    let for_text = image.clone();
    with(|p| {
        p.pins.push(Pin {
            hwnd,
            image,
            pixels,
            zoom: 1.0,
            opacity: 1.0,
            flash: None,
            drag: None,
            active: true,
            text: None,
            selection: None,
            selecting: false,
            clicks: (Instant::now(), POINT::default(), 0),
        })
    });
    unsafe {
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = SetForegroundWindow(hwnd);
    }
    prepare_text(hwnd, for_text);
}

/// Recognizes the pin's text in the background so it can be selected with the mouse, like on the Mac.
/// Silently skipped without the recognition models.
fn prepare_text(hwnd: HWND, image: RgbaImage) {
    if !app::models_ready() {
        return;
    }
    let id = hwnd.0 as isize;
    std::thread::spawn(move || {
        let layout = util::guarded(move || {
            let lines = app::recognize(&image).ok()?;
            let glyphs = lines
                .into_iter()
                .map(|l| {
                    let spans: Vec<Option<(f32, f32)>> = l.char_boxes.iter().map(|b| Some(*b)).collect();
                    GlyphLine::new(&l.text, l.rect, &spans)
                })
                .collect();
            Some(TextLayout::new(glyphs))
        })
        .ok()
        .flatten();
        util::run_on_ui(move || {
            util::trace(|| format!("pin text: {:?}", layout.as_ref().map(|l| l.lines.iter().map(|g| (g.text.clone(), g.rect)).collect::<Vec<_>>())));
            with_pin(HWND(id as *mut _), |p| p.text = layout.filter(|l| !l.is_empty()));
        });
    });
}

/// A client point (window pixels) in picture pixels.
fn picture_point(hwnd: HWND, x: i32, y: i32) -> Option<Point> {
    let mut client = RECT::default();
    unsafe {
        let _ = GetClientRect(hwnd, &mut client);
    }
    with_pin(hwnd, |p| {
        let sx = p.image.width as f32 / client.right.max(1) as f32;
        let sy = p.image.height as f32 / client.bottom.max(1) as f32;
        Point::new(x as f32 * sx, y as f32 * sy)
    })
}

fn selected_text(hwnd: HWND) -> Option<String> {
    with_pin(hwnd, |p| {
        let (layout, span) = (p.text.as_ref()?, p.selection?);
        let t = layout.text(&span);
        (!t.is_empty()).then_some(t)
    })
    .flatten()
}

fn copy_selected_text(hwnd: HWND) {
    if let Some(text) = selected_text(hwnd) {
        if clipboard::set_text(util::app_window(), &text) {
            flash(hwnd, "已复制文字");
        }
    }
}

fn repaint(hwnd: HWND) {
    unsafe {
        let _ = InvalidateRect(Some(hwnd), None, false);
    }
}

pub fn close_all() {
    let hwnds: Vec<HWND> = with(|p| {
        p.hidden = false;
        p.pins.drain(..).map(|p| p.hwnd).collect()
    })
    .unwrap_or_default();
    for h in hwnds {
        unsafe {
            let _ = DestroyWindow(h);
        }
    }
}

/// Hides every pin, or shows them again when hidden.
pub fn toggle_hidden() {
    let Some((hidden, hwnds)) = with(|p| {
        if p.pins.is_empty() {
            return None;
        }
        p.hidden = !p.hidden;
        Some((p.hidden, p.pins.iter().map(|p| p.hwnd).collect::<Vec<_>>()))
    })
    .flatten() else {
        hud::show("当前没有贴图");
        return;
    };
    for h in &hwnds {
        unsafe {
            let _ = ShowWindow(*h, if hidden { SW_HIDE } else { SW_SHOWNA });
        }
    }
    if hidden {
        hud::show(&format!("已隐藏 {} 张贴图，再按一次显示", hwnds.len()));
    }
}

fn close(hwnd: HWND) {
    with(|p| p.pins.retain(|p| p.hwnd != hwnd));
    unsafe {
        let _ = DestroyWindow(hwnd);
    }
}

fn flash(hwnd: HWND, text: &str) {
    with_pin(hwnd, |p| p.flash = Some((text.to_string(), Instant::now() + Duration::from_millis(1200))));
    unsafe {
        let _ = InvalidateRect(Some(hwnd), None, false);
        SetTimer(Some(hwnd), FLASH_TIMER, 1200, None);
    }
}

fn window_rect(hwnd: HWND) -> RECT {
    let mut r = RECT::default();
    unsafe {
        let _ = GetWindowRect(hwnd, &mut r);
    }
    r
}

/// Zooms to `zoom` (10%–800%) keeping `anchor` (screen pixels) fixed; defaults to the center.
fn set_zoom(hwnd: HWND, zoom: f32, anchor: Option<POINT>, show: bool) {
    let Some((w, h)) = with_pin(hwnd, |p| (p.image.width as f32, p.image.height as f32)) else { return };
    let zoom = zoom.clamp(0.1, 8.0);
    let r = window_rect(hwnd);
    let anchor = anchor.unwrap_or(POINT { x: (r.left + r.right) / 2, y: (r.top + r.bottom) / 2 });
    let rx = (anchor.x - r.left) as f32 / (r.right - r.left).max(1) as f32;
    let ry = (anchor.y - r.top) as f32 / (r.bottom - r.top).max(1) as f32;
    let (nw, nh) = ((w * zoom).round().max(8.0) as i32, (h * zoom).round().max(8.0) as i32);
    let (x, y) = (anchor.x - (rx * nw as f32) as i32, anchor.y - (ry * nh as f32) as i32);
    with_pin(hwnd, |p| p.zoom = zoom);
    unsafe {
        let _ = SetWindowPos(hwnd, None, x, y, nw, nh, SWP_NOZORDER | SWP_NOACTIVATE);
        let _ = InvalidateRect(Some(hwnd), None, false);
    }
    if show {
        flash(hwnd, &format!("{}%", (zoom * 100.0).round() as i32));
    }
}

fn set_opacity(hwnd: HWND, value: f32) {
    let v = value.clamp(0.2, 1.0);
    with_pin(hwnd, |p| p.opacity = v);
    unsafe {
        let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), (v * 255.0).round() as u8, LWA_ALPHA);
    }
    flash(hwnd, &format!("透明度 {}%", (v * 100.0).round() as i32));
}

fn copy_image(hwnd: HWND) {
    if let Some(img) = with_pin(hwnd, |p| p.image.clone()) {
        if clipboard::set_image(util::app_window(), &img) {
            flash(hwnd, "已复制");
        }
    }
}

fn save_image(hwnd: HWND) {
    let Some(img) = with_pin(hwnd, |p| p.image.clone()) else { return };
    let format = crate::kit::settings::get().image_format;
    match crate::kit::export::save(&img, format, &app::save_directory(), app::local_time()) {
        Ok(path) => {
            flash(hwnd, "已保存");
            hud::show(&format!("已保存到 {}", path.display()));
        }
        Err(_) => flash(hwnd, "保存失败"),
    }
}

fn paint(hwnd: HWND) {
    let mut ps = PAINTSTRUCT::default();
    let hdc = unsafe { BeginPaint(hwnd, &mut ps) };
    let mut client = RECT::default();
    unsafe {
        let _ = GetClientRect(hwnd, &mut client);
    }
    let (w, h) = (client.right.max(1) as u32, client.bottom.max(1) as u32);
    let frame = with_pin(hwnd, |p| {
        let mut out = Pixmap::new(w, h)?;
        let scale = monitor_scale(hwnd);
        {
            let mut c = Canvas::new(out.as_mut(), Transform::identity());
            let quality = if p.zoom >= 2.0 { FilterQuality::Nearest } else { FilterQuality::Bicubic };
            c.draw_image(p.pixels.as_ref(), &Rect::new(0.0, 0.0, w as f32, h as f32), quality, 1.0);
            if let (Some(layout), Some(span)) = (&p.text, &p.selection) {
                let (sx, sy) = (w as f32 / p.image.width.max(1) as f32, h as f32 / p.image.height.max(1) as f32);
                for r in layout.rects(span) {
                    c.fill_rect(&Rect::new(r.x * sx, r.y * sy, r.width * sx, r.height * sy), SELECTION_BLUE.with_alpha(0.3));
                }
            }
            let (color, width) = if p.active { (SELECTION_BLUE, 1.5 * scale) } else { (Color::gray(0.5, 0.5), 1.0) };
            c.stroke_rect(&Rect::new(width / 2.0, width / 2.0, w as f32 - width, h as f32 - width), color, width);
            if let Some((text, until)) = &p.flash {
                if Instant::now() < *until {
                    let mut c2 = Canvas::new(out.as_mut(), Transform::from_scale(scale, scale));
                    let max = (w as f32 / scale).max(120.0);
                    let (size, _) = chrome::toast_layout(text, max);
                    let o = Point::new((w as f32 / scale - size.width - 6.0).max(4.0), (h as f32 / scale - size.height - 6.0).max(4.0));
                    chrome::draw_toast(&mut c2, o, text, max, 1.0);
                }
            }
        }
        Some(out)
    })
    .flatten();
    if let Some(frame) = frame {
        util::blit(hdc, 0, 0, &frame);
    }
    unsafe {
        let _ = EndPaint(hwnd, &ps);
    }
}

fn monitor_scale(hwnd: HWND) -> f32 {
    let dpi = unsafe { windows::Win32::UI::HiDpi::GetDpiForWindow(hwnd) };
    dpi.max(96) as f32 / 96.0
}

const MENU_COPY: usize = 1;
const MENU_COPY_TEXT: usize = 5;
const MENU_SAVE: usize = 2;
const MENU_CLOSE: usize = 3;
const MENU_CLOSE_ALL: usize = 4;
const MENU_ZOOM: usize = 100;
const MENU_OPACITY: usize = 200;

fn show_menu(hwnd: HWND, at: POINT) {
    let (zoom, opacity) = with_pin(hwnd, |p| (p.zoom, p.opacity)).unwrap_or((1.0, 1.0));
    unsafe {
        let Ok(menu) = CreatePopupMenu() else { return };
        let add = |m: HMENU, id: usize, text: &str, checked: bool| {
            let t = wide(text);
            let flags = MF_STRING | if checked { MF_CHECKED } else { MF_UNCHECKED };
            let _ = AppendMenuW(m, flags, id, PCWSTR(t.as_ptr()));
        };
        if selected_text(hwnd).is_some() {
            add(menu, MENU_COPY_TEXT, "复制选中文字\tCtrl+C", false);
            add(menu, MENU_COPY, "复制图片", false);
        } else {
            add(menu, MENU_COPY, "复制\tCtrl+C", false);
        }
        add(menu, MENU_SAVE, "保存\tCtrl+S", false);
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
        if let Ok(zoom_menu) = CreatePopupMenu() {
            for pct in [25, 50, 100, 150, 200, 300] {
                add(zoom_menu, MENU_ZOOM + pct, &format!("{pct}%"), (zoom * 100.0 - pct as f32).abs() < 0.5);
            }
            let t = wide("缩放");
            let _ = AppendMenuW(menu, MF_POPUP, zoom_menu.0 as usize, PCWSTR(t.as_ptr()));
        }
        if let Ok(op_menu) = CreatePopupMenu() {
            for pct in [100, 80, 60, 40, 20] {
                add(op_menu, MENU_OPACITY + pct, &format!("{pct}%"), (opacity * 100.0 - pct as f32).abs() < 0.5);
            }
            let t = wide("透明度");
            let _ = AppendMenuW(menu, MF_POPUP, op_menu.0 as usize, PCWSTR(t.as_ptr()));
        }
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
        add(menu, MENU_CLOSE, "关闭\tEsc", false);
        add(menu, MENU_CLOSE_ALL, "关闭全部贴图", false);
        let _ = SetForegroundWindow(hwnd);
        let cmd = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_RIGHTBUTTON, at.x, at.y, None, hwnd, None).0 as usize;
        let _ = DestroyMenu(menu);
        match cmd {
            MENU_COPY => copy_image(hwnd),
            MENU_COPY_TEXT => copy_selected_text(hwnd),
            MENU_SAVE => save_image(hwnd),
            MENU_CLOSE => close(hwnd),
            MENU_CLOSE_ALL => close_all(),
            c if (MENU_ZOOM..MENU_ZOOM + 100).contains(&c) => set_zoom(hwnd, (c - MENU_ZOOM) as f32 / 100.0, None, true),
            c if (MENU_OPACITY..MENU_OPACITY + 101).contains(&c) => set_opacity(hwnd, (c - MENU_OPACITY) as f32 / 100.0),
            _ => {}
        }
    }
}

unsafe extern "system" fn proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let ctrl = unsafe { GetKeyState(VK_CONTROL.0 as i32) } < 0;
    let shift = unsafe { GetKeyState(VK_SHIFT.0 as i32) } < 0;
    match msg {
        WM_ERASEBKGND => return LRESULT(1),
        WM_PAINT => {
            paint(hwnd);
            return LRESULT(0);
        }
        WM_ACTIVATE => {
            let active = util::loword(wp.0) != 0;
            with_pin(hwnd, |p| p.active = active);
            unsafe {
                let _ = InvalidateRect(Some(hwnd), None, false);
            }
        }
        WM_LBUTTONDOWN | WM_LBUTTONDBLCLK => {
            let mut cursor = POINT::default();
            unsafe {
                let _ = GetCursorPos(&mut cursor);
                SetCapture(hwnd);
            }
            let (x, y) = lparam_point(lp);
            let at = picture_point(hwnd, x, y).unwrap_or_default();
            // Count clicks ourselves: Windows only reports doubles, and a triple click selects the line.
            let limit = Duration::from_millis(unsafe { windows::Win32::UI::Input::KeyboardAndMouse::GetDoubleClickTime() } as u64);
            let count = with_pin(hwnd, |p| {
                let (last, pos, n) = p.clicks;
                let near = (pos.x - cursor.x).abs() <= 4 && (pos.y - cursor.y).abs() <= 4;
                let n = if near && last.elapsed() <= limit { n + 1 } else { 1 };
                p.clicks = (Instant::now(), cursor, n);
                n
            })
            .unwrap_or(1);
            util::trace(|| format!("pin click {count} at {at:?} (msg {msg:#x}), text ready: {}", with_pin(hwnd, |p| p.text.is_some()).unwrap_or(false)));
            let handled = with_pin(hwnd, |p| {
                let layout = p.text.as_ref()?;
                if count >= 2 {
                    // Closing is Esc's job; double-click selects a word, triple-click the line.
                    let span = if count == 2 { layout.word(at) } else { layout.line_at(at) };
                    p.selection = span.or(p.selection);
                    return Some(());
                }
                // Pressing on text selects it, like a text field; anywhere else drags the pin.
                let position = layout.hit_test(at, 3.0)?;
                p.selection = Some(match (shift, p.selection) {
                    (true, Some(current)) => TextSpan { anchor: current.anchor, focus: position },
                    _ => TextSpan { anchor: position, focus: position },
                });
                p.selecting = true;
                Some(())
            })
            .flatten()
            .is_some();
            if !handled {
                let r = window_rect(hwnd);
                with_pin(hwnd, |p| {
                    p.selection = None;
                    p.drag = Some((cursor, POINT { x: r.left, y: r.top }));
                });
            }
            repaint(hwnd);
            return LRESULT(0);
        }
        WM_MOUSEMOVE => {
            if with_pin(hwnd, |p| p.selecting).unwrap_or(false) {
                let (x, y) = lparam_point(lp);
                if let Some(at) = picture_point(hwnd, x, y) {
                    with_pin(hwnd, |p| {
                        if let (Some(layout), Some(span)) = (&p.text, &mut p.selection) {
                            if let Some(pos) = layout.nearest(at) {
                                span.focus = pos;
                            }
                        }
                    });
                    repaint(hwnd);
                }
                return LRESULT(0);
            }
            if let Some(Some((start, origin))) = with_pin(hwnd, |p| p.drag) {
                let mut cursor = POINT::default();
                unsafe {
                    let _ = GetCursorPos(&mut cursor);
                    let _ = SetWindowPos(hwnd, None, origin.x + cursor.x - start.x, origin.y + cursor.y - start.y, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
                }
            }
            return LRESULT(0);
        }
        WM_LBUTTONUP => {
            with_pin(hwnd, |p| {
                p.drag = None;
                if p.selecting && p.selection.is_some_and(|s| s.is_empty()) {
                    p.selection = None;
                }
                p.selecting = false;
            });
            repaint(hwnd);
            unsafe {
                let _ = ReleaseCapture();
            }
            return LRESULT(0);
        }
        WM_SETCURSOR => {
            if util::loword(lp.0 as usize) as u32 == HTCLIENT {
                let mut pt = POINT::default();
                unsafe {
                    let _ = GetCursorPos(&mut pt);
                    let _ = windows::Win32::Graphics::Gdi::ScreenToClient(hwnd, &mut pt);
                }
                let over_text = picture_point(hwnd, pt.x, pt.y)
                    .and_then(|at| with_pin(hwnd, |p| p.text.as_ref().and_then(|l| l.hit_test(at, 3.0)).is_some()))
                    .unwrap_or(false);
                unsafe {
                    SetCursor(LoadCursorW(None, if over_text { IDC_IBEAM } else { IDC_SIZEALL }).ok());
                }
                return LRESULT(1);
            }
        }
        WM_MBUTTONDOWN => {
            // Middle click: back to 100%.
            set_zoom(hwnd, 1.0, None, true);
            return LRESULT(0);
        }
        WM_CONTEXTMENU => {
            let (mut x, mut y) = lparam_point(lp);
            if x == -1 && y == -1 {
                let r = window_rect(hwnd);
                (x, y) = (r.left + 10, r.top + 10);
            }
            show_menu(hwnd, POINT { x, y });
            return LRESULT(0);
        }
        WM_MOUSEWHEEL => {
            let steps = util::hiword(wp.0) as i16 as f32 / 120.0;
            if ctrl {
                let o = with_pin(hwnd, |p| p.opacity).unwrap_or(1.0);
                set_opacity(hwnd, o + steps * 0.05);
            } else {
                let z = with_pin(hwnd, |p| p.zoom).unwrap_or(1.0);
                let (x, y) = lparam_point(lp);
                set_zoom(hwnd, z * 1.1f32.powf(steps), Some(POINT { x, y }), true);
            }
            return LRESULT(0);
        }
        WM_KEYDOWN => {
            let vk = wp.0 as u32;
            let z = with_pin(hwnd, |p| p.zoom).unwrap_or(1.0);
            let step = if shift { 10 } else { 1 };
            match vk {
                // Esc first drops a text selection, then closes the pin.
                v if v == VK_ESCAPE.0 as u32 => {
                    if with_pin(hwnd, |p| p.selection.take().is_some()).unwrap_or(false) {
                        repaint(hwnd);
                    } else {
                        close(hwnd);
                    }
                }
                0x57 if ctrl => close(hwnd),
                0x43 if ctrl => {
                    if selected_text(hwnd).is_some() { copy_selected_text(hwnd) } else { copy_image(hwnd) }
                }
                // Ctrl+A selects all the recognized text.
                0x41 if ctrl => {
                    with_pin(hwnd, |p| {
                        if let Some(layout) = &p.text {
                            let last = layout.lines.len() - 1;
                            let end = layout.lines[last].boxes.len();
                            p.selection = Some(TextSpan { anchor: TextPosition { line: 0, offset: 0 }, focus: TextPosition { line: last, offset: end } });
                        }
                    });
                    repaint(hwnd);
                }
                0x53 if ctrl => save_image(hwnd),
                0xBB | 0x6B => set_zoom(hwnd, z * 1.1, None, true),
                0xBD | 0x6D => set_zoom(hwnd, z / 1.1, None, true),
                0x30 | 0x60 => set_zoom(hwnd, 1.0, None, true),
                v if v == VK_LEFT.0 as u32 || v == VK_RIGHT.0 as u32 || v == VK_UP.0 as u32 || v == VK_DOWN.0 as u32 => {
                    let (dx, dy) = match v {
                        v if v == VK_LEFT.0 as u32 => (-step, 0),
                        v if v == VK_RIGHT.0 as u32 => (step, 0),
                        v if v == VK_UP.0 as u32 => (0, -step),
                        _ => (0, step),
                    };
                    let r = window_rect(hwnd);
                    unsafe {
                        let _ = SetWindowPos(hwnd, None, r.left + dx, r.top + dy, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
                    }
                }
                _ => {}
            }
            return LRESULT(0);
        }
        WM_TIMER => {
            unsafe {
                let _ = KillTimer(Some(hwnd), FLASH_TIMER);
            }
            with_pin(hwnd, |p| p.flash = None);
            unsafe {
                let _ = InvalidateRect(Some(hwnd), None, false);
            }
            return LRESULT(0);
        }
        WM_DPICHANGED => {
            // Keep the picture's pixel size: pins show screenshot pixels 1:1 at 100%.
            return LRESULT(0);
        }
        _ => {}
    }
    unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
}
