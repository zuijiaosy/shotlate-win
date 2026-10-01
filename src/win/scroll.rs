//! Live region capture. The worker owns stitching; UI state is borrowed only for short calls.
use std::cell::RefCell;
use std::sync::{Arc, Condvar, Mutex};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use tiny_skia::Pixmap;
use windows::core::{PCWSTR, w};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{BeginPaint, EndPaint, InvalidateRect, PAINTSTRUCT};
use windows::Win32::Graphics::Dwm::DwmFlush;
use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, RegisterHotKey, UnregisterHotKey, HOT_KEY_MODIFIERS, SendInput, INPUT, INPUT_0, INPUT_MOUSE, MOUSEINPUT, MOUSEEVENTF_WHEEL, VK_CONTROL, VK_ESCAPE};
use windows::Win32::UI::WindowsAndMessaging::*;
use crate::kit::image::RgbaImage;
use crate::kit::scrollstitcher::{AutoAction, AutoScroll, ScrollStitcher, StitchResult, height_budget};
use crate::kit::geom::{Point, Rect};
use crate::ui::scroll_view;
use super::{app, clipboard, dialogs, hud, layered, pin, screen, util};

const CLASS: PCWSTR = w!("ShotlateScroll");
const BORDER: PCWSTR = w!("ShotlateScrollBorder");
static IDS: AtomicU64 = AtomicU64::new(1);
pub const TEST_STATE: u32 = WM_APP + 71;
#[derive(Default)]
struct Work { frame: Option<Frame>, finish: bool, cancel: bool }
struct Frame { image: RgbaImage, serial: u64, captured: Instant }
type Shared = Arc<(Mutex<Work>, Condvar)>;
struct Capture {
    id: u64, hwnd: HWND, border: Vec<HWND>, rect: RECT, target: HWND, scale: f32,
    shared: Shared, preview: Option<Pixmap>, height: usize, status: String,
    auto: AutoScroll, sent: Option<Instant>, changed: Instant, progress: bool, lost: bool, recovering: bool,
    width: f32, panel_height: f32,
    panel_rect: RECT, hides_panel: bool, scroll_point: POINT,
    next_frame: u64, step_serial: u64, processed_at: Option<Instant>,
}
struct ResultView { hwnd: HWND, image: Arc<RgbaImage>, pixels: Pixmap, scale: f32, width: f32, height: f32, offset: f32, status: String }
thread_local! {
    static CAPTURE: RefCell<Option<Capture>> = const { RefCell::new(None) };
    static RESULT: RefCell<Option<ResultView>> = const { RefCell::new(None) };
}
fn with<R>(f: impl FnOnce(&mut Capture) -> R) -> Option<R> { CAPTURE.with(|s| s.try_borrow_mut().ok()?.as_mut().map(f)) }
fn with_result<R>(f: impl FnOnce(&mut ResultView) -> R) -> Option<R> { RESULT.with(|s| s.try_borrow_mut().ok()?.as_mut().map(f)) }
pub fn is_active() -> bool { CAPTURE.with(|s| s.borrow().is_some()) }
pub fn register() {
    unsafe {
        let class = WNDCLASSEXW { cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32, lpfnWndProc: Some(proc), lpszClassName: CLASS,
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(), ..Default::default() };
        RegisterClassExW(&class);
        let border = WNDCLASSEXW { lpszClassName: BORDER, lpfnWndProc: Some(border_proc), ..class };
        RegisterClassExW(&border);
    }
}
unsafe extern "system" fn border_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if msg == WM_NCHITTEST { return LRESULT(HTTRANSPARENT as isize); }
    unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
}
fn pixels(image: RgbaImage) -> Option<Pixmap> { Pixmap::from_vec(image.data, tiny_skia::IntSize::from_wh(image.width, image.height)?) }
fn repaint(hwnd: HWND) { unsafe { let _ = InvalidateRect(Some(hwnd), None, false); } }
fn inside(r: &RECT, p: POINT) -> bool { p.x >= r.left && p.x < r.right && p.y >= r.top && p.y < r.bottom }

pub fn begin(rect: RECT, scale: f32) {
    if is_active() { return; }
    let monitors = screen::monitors();
    let Some(monitor) = monitors.iter().find(|m| inside(&m.rect, POINT { x: rect.left, y: rect.top })) else { return };
    let region = Rect::new(rect.left as f32, rect.top as f32, (rect.right - rect.left) as f32, (rect.bottom - rect.top) as f32);
    let bounds = Rect::new(monitor.rect.left as f32, monitor.rect.top as f32, (monitor.rect.right - monitor.rect.left) as f32, (monitor.rect.bottom - monitor.rect.top) as f32);
    let panel = scroll_view::panel_rect(region, bounds, scale);
    let (x, y, panel_w, panel_h) = (panel.x.round() as i32, panel.y.round() as i32, panel.width.round() as i32, panel.height.round() as i32);
    let panel_rect = RECT { left: x, top: y, right: x + panel_w, bottom: y + panel_h };
    let hides_panel = !panel.intersection(&region).is_empty();
    let center = POINT { x: (rect.left + rect.right) / 2, y: (rect.top + rect.bottom) / 2 };
    let target = unsafe { GetAncestor(WindowFromPoint(center), GA_ROOT) };
    let scroll_point = [center, POINT { x: rect.left + 8, y: rect.top + 8 }, POINT { x: rect.right - 8, y: rect.top + 8 },
        POINT { x: rect.left + 8, y: rect.bottom - 8 }].into_iter().find(|&p| !inside(&panel_rect, p)).unwrap_or(center);
    let Ok(hwnd) = (unsafe { CreateWindowExW(WS_EX_TOPMOST | WS_EX_TOOLWINDOW, CLASS, w!("Shotlate 长截图"), WS_POPUP | WS_BORDER, x, y, panel_w, panel_h, None, None, None, None) }) else { return };
    let id = IDS.fetch_add(1, Ordering::Relaxed);
    let shared: Shared = Arc::new((Mutex::new(Work::default()), Condvar::new()));
    let border = create_border(rect, scale);
    CAPTURE.with(|s| *s.borrow_mut() = Some(Capture { id, hwnd, border, rect, target, scale, shared: shared.clone(), preview: None,
        height: 0, status: "在框内滚动，或点击自动滚动".into(), auto: AutoScroll::default(), sent: None, changed: Instant::now(),
        progress: false, lost: false, recovering: false, width: (panel_w - 2) as f32 / scale, panel_height: (panel_h - 2) as f32 / scale,
        panel_rect, hides_panel, scroll_point, next_frame: 0, step_serial: 0, processed_at: None }));
    let preview_width = ((260.0 - 24.0) * scale) as usize;
    let preview_height = (panel_h as f32 - 146.0 * scale).max(1.0) as usize;
    std::thread::spawn(move || {
        let outcome = util::guarded(|| {
            let mut stitcher = ScrollStitcher::new(height_budget((rect.right - rect.left) as usize), (16.0 * scale) as usize);
            loop {
                let (frame, finishing, cancel) = {
                    let (lock, wake) = &*shared;
                    let mut work = lock.lock().unwrap_or_else(|e| e.into_inner());
                    while work.frame.is_none() && !work.finish && !work.cancel { work = wake.wait(work).unwrap_or_else(|e| e.into_inner()); }
                    (work.frame.take(), work.finish, work.cancel)
                };
                if cancel { return; }
                if let Some(frame) = frame {
                    let Frame { image, serial, captured } = frame;
                    let result = stitcher.add(image, None);
                    let preview = stitcher.preview(preview_width, preview_height).and_then(pixels);
                    let height = stitcher.height();
                    util::run_on_ui(move || {
                        let hwnd = with(|s| {
                            if s.id != id { return None; }
                            s.height = height; s.preview = preview;
                            s.processed_at = Some(captured);
                            s.auto.frame_processed(serial);
                            if serial >= s.step_serial && matches!(result, StitchResult::Appended(_)) { s.progress = true; }
                            s.lost = result == StitchResult::NoOverlap;
                            if result != StitchResult::Unchanged { s.changed = captured; }
                            if result != StitchResult::Unchanged {
                                s.status = if s.lost { "跟丢了：往回滚一点，接上后继续".into() } else { format!("{} px · {}", height, if s.auto.running { "自动滚动中" } else { "继续滚动，或点完成" }) };
                            }
                            Some(s.hwnd)
                        }).flatten();
                        if let Some(hwnd) = hwnd { repaint(hwnd); }
                        if result == StitchResult::LimitReached && with(|s| s.id == id).unwrap_or(false) { finish(false); }
                    });
                }
                if finishing {
                    let image = stitcher.into_image();
                    util::run_on_ui(move || {
                        if with(|s| s.id == id).unwrap_or(false) {
                            teardown();
                            if let Some(image) = image { show_result(image, scale); } else { hud::show("没有取得长截图画面"); }
                        }
                    });
                    return;
                }
            }
        });
        if let Err(message) = outcome { util::run_on_ui(move || { if with(|s| s.id == id).unwrap_or(false) { teardown(); hud::show(&format!("长截图失败：{message}")); } }); }
    });
    unsafe { let _ = RegisterHotKey(Some(hwnd), 70, HOT_KEY_MODIFIERS(0), VK_ESCAPE.0 as u32); let _ = ShowWindow(hwnd, SW_SHOWNA); SetTimer(Some(hwnd), 1, 50, None); }
    sample();
}

fn create_border(r: RECT, scale: f32) -> Vec<HWND> {
    let n = (2.0 * scale).round().max(1.0) as i32;
    let regions = [(r.left - n, r.top - n, r.right - r.left + 2 * n, n), (r.left - n, r.bottom, r.right - r.left + 2 * n, n),
        (r.left - n, r.top, n, r.bottom - r.top), (r.right, r.top, n, r.bottom - r.top)];
    regions.into_iter().filter_map(|(x, y, w, h)| unsafe {
        let hwnd = CreateWindowExW(WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            BORDER, w!(""), WS_POPUP, x, y, w, h, None, None, None, None).ok()?;
        let mut pix = Pixmap::new(w as u32, h as u32)?;
        for py in 0..h { for px in 0..w { if (px + py) / (4 * n) % 2 == 0 {
            let i = (py * w + px) as usize * 4; pix.data_mut()[i..i+4].copy_from_slice(&[40, 150, 240, 255]);
        } } }
        layered::update(hwnd, &pix, POINT { x, y }); let _ = ShowWindow(hwnd, SW_SHOWNA); Some(hwnd)
    }).collect()
}
fn sample() {
    let Some((rect, shared, hwnd, hide, serial)) = with(|s| { s.next_frame += 1; (s.rect, s.shared.clone(), s.hwnd, s.hides_panel, s.next_frame) }) else { return };
    if hide { unsafe { let _ = ShowWindow(hwnd, SW_HIDE); let _ = DwmFlush(); } }
    let frame = screen::grab(&rect);
    let captured = Instant::now();
    if hide { unsafe { let _ = ShowWindow(hwnd, SW_SHOWNA); } }
    if let Some(frame) = frame {
        let (lock, wake) = &*shared;
        let mut work = lock.lock().unwrap_or_else(|e| e.into_inner());
        if !work.finish && !work.cancel {
            let (width, height) = (frame.width(), frame.height());
            work.frame = Some(Frame { image: RgbaImage { width, height, data: frame.take() }, serial, captured }); wake.notify_one();
        }
    } else { with(|s| { s.auto.pause(); s.status = "取帧失败，请取消后重新截图".into(); }); }
}
fn teardown() {
    let s = CAPTURE.with(|s| s.borrow_mut().take());
    if let Some(s) = s {
        { let (lock, wake) = &*s.shared; lock.lock().unwrap_or_else(|e| e.into_inner()).cancel = true; wake.notify_one(); }
        unsafe { let _ = UnregisterHotKey(Some(s.hwnd), 70); let _ = KillTimer(Some(s.hwnd), 1); for hwnd in s.border { let _ = DestroyWindow(hwnd); } let _ = DestroyWindow(s.hwnd); }
    }
}
fn finish(cancel: bool) {
    if cancel { teardown(); return; }
    sample();
    if let Some((hwnd, shared)) = with(|s| { s.auto.pause(); s.status = "正在生成长截图…".into(); (s.hwnd, s.shared.clone()) }) {
        unsafe { let _ = KillTimer(Some(hwnd), 1); }
        let (lock, wake) = &*shared; lock.lock().unwrap_or_else(|e| e.into_inner()).finish = true; wake.notify_one(); repaint(hwnd);
    }
}
fn wheel(notches: i32) -> bool {
    let input = INPUT { r#type: INPUT_MOUSE, Anonymous: INPUT_0 { mi: MOUSEINPUT { mouseData: (notches * 120) as u32, dwFlags: MOUSEEVENTF_WHEEL, ..Default::default() } } };
    unsafe { SendInput(&[input], std::mem::size_of::<INPUT>() as i32) == 1 }
}
fn step(notches: i32) {
    with(|s| { s.progress = false; s.sent = Some(Instant::now()); s.step_serial = s.next_frame + 1; s.auto.await_frame(s.step_serial); });
    if !wheel(notches) { with(|s| { s.auto.pause(); s.sent = None; s.status = "无法滚动目标窗口，请手动滚动".into(); }); }
}
fn toggle_auto() {
    let action = with(|s| {
        if s.auto.running { s.auto.pause(); s.sent = None; s.status = "已暂停，可以手动滚动".into(); return None; }
        s.auto.start(); s.recovering = false;
        Some((s.scroll_point, s.target, s.auto.notches))
    }).flatten();
    if let Some((point, target, n)) = action {
        if !unsafe { SetForegroundWindow(target) }.as_bool() {
            with(|s| { s.auto.pause(); s.status = "无法激活目标窗口，请先点选目标内容".into(); }); return;
        }
        unsafe { let _ = SetCursorPos(point.x, point.y); }
        step(-n);
    }
}
fn tick() {
    sample();
    let mut cursor = POINT::default(); unsafe { let _ = GetCursorPos(&mut cursor); }
    let action = with(|s| {
        if !s.auto.running { return None; }
        if !inside(&s.rect, cursor) || inside(&s.panel_rect, cursor) || !unsafe { IsWindow(Some(s.target)) }.as_bool() || unsafe { GetForegroundWindow() } != s.target {
            s.auto.pause(); s.sent = None; s.status = "已暂停：鼠标离开选区或目标窗口已切换".into(); return None;
        }
        let sent = s.sent?;
        // Time alone cannot prove stagnation: require a processed post-settle frame.
        if !s.processed_at.is_some_and(|at| at >= sent + Duration::from_millis(300)) { return None; }
        if sent.elapsed() < Duration::from_millis(300) || (s.changed.elapsed() < Duration::from_millis(250) && sent.elapsed() < Duration::from_millis(1200)) { return None; }
        if s.recovering { s.recovering = false; return Some(AutoAction::Continue); }
        Some(s.auto.settled(s.progress, s.lost))
    }).flatten();
    match action {
        Some(AutoAction::Continue) => { if let Some(n) = with(|s| s.auto.notches) { step(-n); } }
        Some(AutoAction::Recover(n)) => { with(|s| s.recovering = true); step(n); }
        Some(AutoAction::Finish) => finish(false),
        Some(AutoAction::Pause) => { with(|s| { s.sent = None; s.status = "未能确认滚动进展，请手动滚动或重新选择区域".into(); }); }
        Some(AutoAction::Wait) | None => {}
    }
    if let Some(hwnd) = with(|s| s.hwnd) { repaint(hwnd); }
}

fn show_result(image: RgbaImage, scale: f32) {
    close_result();
    let Some(monitor) = screen::monitors().into_iter().next() else { return };
    let width = 760.0_f32.min((monitor.rect.right - monitor.rect.left) as f32 / scale - 40.0);
    let height = 640.0_f32.min((monitor.rect.bottom - monitor.rect.top) as f32 / scale - 80.0);
    let Some(pixels) = image.thumbnail(1024, 8192).and_then(pixels) else { hud::show("长截图太大，无法打开预览"); return };
    let Ok(hwnd) = (unsafe { CreateWindowExW(WS_EX_TOOLWINDOW | WS_EX_TOPMOST, CLASS, w!("Shotlate 长截图"), WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU,
        monitor.rect.left + 20, monitor.rect.top + 20, (width * scale) as i32, (height * scale) as i32, None, None, None, None) }) else { return };
    let mut client = RECT::default(); unsafe { let _ = GetClientRect(hwnd, &mut client); }
    let status = format!("{} × {} px", image.width, image.height);
    RESULT.with(|s| *s.borrow_mut() = Some(ResultView { hwnd, image: Arc::new(image), pixels, scale, width: client.right as f32 / scale, height: client.bottom as f32 / scale, offset: 0.0, status }));
    unsafe { let _ = ShowWindow(hwnd, SW_SHOW); let _ = SetForegroundWindow(hwnd); }
}
fn close_result() {
    if let Some(s) = RESULT.with(|s| s.borrow_mut().take()) { unsafe { let _ = DestroyWindow(s.hwnd); } }
}
fn result_action(index: usize) {
    if index >= 3 { close_result(); return; }
    let Some((image, hwnd)) = with_result(|s| (Arc::clone(&s.image), s.hwnd)) else { return };
    match index {
        0 => { if clipboard::set_image(util::app_window(), &image) { hud::show("已复制长截图"); } else { hud::show("复制失败，请重试"); } }
        1 => {
            let format = crate::kit::settings::get().image_format;
            match crate::kit::export::save(&image, format, &app::save_directory(), app::local_time()) {
                Ok(path) => hud::show(&format!("已保存到 {}", path.display())),
                Err(e) => dialogs::warn(Some(hwnd), "保存失败", &e),
            }
        }
        2 => {
            let monitor = screen::monitors().into_iter().next();
            if let Some(m) = monitor { pin::create_fitted(image.as_ref().clone(), POINT { x: m.rect.left + 40, y: m.rect.top + 40 }, (m.rect.right - m.rect.left - 80) as f32, (m.rect.bottom - m.rect.top - 80) as f32); }
        }
        _ => close_result(),
    }
}
fn paint(hwnd: HWND) {
    let mut ps = PAINTSTRUCT::default(); let hdc = unsafe { BeginPaint(hwnd, &mut ps) };
    let dark = util::is_dark_mode();
    let frame = with(|s| if s.hwnd == hwnd { scroll_view::render(s.width, s.panel_height, s.scale, s.preview.as_ref(), &s.status, s.auto.running, false, 0.0, dark) } else { None }).flatten()
        .or_else(|| with_result(|s| if s.hwnd == hwnd { scroll_view::render(s.width, s.height, s.scale, Some(&s.pixels), &s.status, false, true, s.offset, dark) } else { None }).flatten());
    if let Some(frame) = frame { util::blit(hdc, 0, 0, &frame); }
    unsafe { let _ = EndPaint(hwnd, &ps); }
}
unsafe extern "system" fn proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let capture = with(|s| s.hwnd == hwnd).unwrap_or(false);
    match msg {
        TEST_STATE => return LRESULT(if capture { with(|s| s.height * 2 + usize::from(s.auto.running)).unwrap_or(0) } else { with_result(|s| s.image.height as usize * 2).unwrap_or(0) } as isize),
        WM_ERASEBKGND => return LRESULT(1),
        WM_PAINT => { paint(hwnd); return LRESULT(0); }
        WM_TIMER if capture => { tick(); return LRESULT(0); }
        WM_HOTKEY if capture => { finish(true); return LRESULT(0); }
        WM_CLOSE => { if capture { finish(true); } else { close_result(); } return LRESULT(0); }
        WM_LBUTTONUP => {
            let (x, y) = util::lparam_point(lp);
            let hit = if capture {
                with(|s| scroll_view::buttons(s.width, s.panel_height, false).iter().position(|(r, _)| r.contains(Point::new(x as f32 / s.scale, y as f32 / s.scale)))).flatten()
            } else {
                with_result(|s| scroll_view::buttons(s.width, s.height, true).iter().position(|(r, _)| r.contains(Point::new(x as f32 / s.scale, y as f32 / s.scale)))).flatten()
            };
            if let Some(index) = hit { if capture { match index { 0 => toggle_auto(), 1 => finish(false), _ => finish(true) } } else { result_action(index); } }
            repaint(hwnd); return LRESULT(0);
        }
        WM_MOUSEWHEEL if !capture => {
            let delta = ((wp.0 >> 16) as u16 as i16) as f32;
            with_result(|s| {
                let image_h = (s.width - 24.0) * s.image.height as f32 / s.image.width as f32;
                s.offset = (s.offset - delta / 120.0 * 60.0).clamp(0.0, (image_h - (s.height - 144.0)).max(0.0));
            }); repaint(hwnd); return LRESULT(0);
        }
        WM_KEYDOWN => {
            let ctrl = unsafe { GetKeyState(VK_CONTROL.0 as i32) } < 0;
            if wp.0 == VK_ESCAPE.0 as usize { if capture { finish(true); } else { close_result(); } }
            else if capture && wp.0 == 13 { finish(false); }
            else if !capture && ctrl { match wp.0 { 0x43 => result_action(0), 0x53 => result_action(1), 0x54 => result_action(2), _ => {} } }
            return LRESULT(0);
        }
        _ => {}
    }
    unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
}
