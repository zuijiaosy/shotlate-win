//! A short message at the bottom of the screen, used after the overlay has closed ("已复制到剪贴板").

use std::cell::Cell;

use tiny_skia::{Pixmap, Transform};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GetCursorPos, KillTimer, RegisterClassExW, SW_SHOWNOACTIVATE, SetTimer, ShowWindow,
    WM_TIMER, WNDCLASSEXW, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
};
use windows::core::w;

use super::screen;
use crate::kit::color::Color;
use crate::kit::geom::{Point, Rect};
use crate::render::canvas::Canvas;
use crate::render::text::{self, Weight};

thread_local! {
    static CURRENT: Cell<isize> = const { Cell::new(0) };
}

unsafe extern "system" fn proc(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    if msg == WM_TIMER {
        unsafe {
            let _ = KillTimer(Some(hwnd), 1);
            let _ = DestroyWindow(hwnd);
        }
        CURRENT.with(|c| {
            if c.get() == hwnd.0 as isize {
                c.set(0);
            }
        });
        return LRESULT(0);
    }
    unsafe { DefWindowProcW(hwnd, msg, w, l) }
}

pub fn register() {
    let class = WNDCLASSEXW {
        cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
        lpfnWndProc: Some(proc),
        lpszClassName: w!("ShotlateHud"),
        ..Default::default()
    };
    unsafe {
        RegisterClassExW(&class);
    }
}

/// Shows `message` for 1.6 s on the monitor under the pointer, replacing any message still showing.
pub fn show(message: &str) {
    CURRENT.with(|c| {
        let old = c.replace(0);
        if old != 0 {
            unsafe {
                let _ = DestroyWindow(HWND(old as *mut _));
            }
        }
    });
    let mut cursor = POINT::default();
    unsafe {
        let _ = GetCursorPos(&mut cursor);
    }
    let monitors = screen::monitors();
    let Some(m) = monitors
        .iter()
        .find(|m| cursor.x >= m.rect.left && cursor.x < m.rect.right && cursor.y >= m.rect.top && cursor.y < m.rect.bottom)
        .or(monitors.first())
    else {
        return;
    };
    let s = m.scale;
    let monitor_width = (m.rect.right - m.rect.left) as f32 / s;
    let l = text::layout(message, 13.0, Weight::Bold, Some(monitor_width - 72.0));
    let width = (l.width + 32.0).min(monitor_width - 40.0);
    let height = l.height.max(20.0) + 16.0;
    let (pw, ph) = ((width * s).ceil() as u32, (height * s).ceil() as u32);
    let Some(mut pix) = Pixmap::new(pw, ph) else { return };
    {
        let mut c = Canvas::new(pix.as_mut(), Transform::from_scale(s, s));
        c.fill_rounded(&Rect::new(0.0, 0.0, width, height), 10.0, Color::black(0.78));
        c.text_layout(&l, Point::new(16.0, (height - l.height) / 2.0), Color::white(1.0));
    }
    let x = m.rect.left + ((m.rect.right - m.rect.left) - pw as i32) / 2;
    let y = m.rect.bottom - (120.0 * s) as i32 - ph as i32;
    unsafe {
        let Ok(hwnd) = CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_TRANSPARENT | WS_EX_NOACTIVATE,
            w!("ShotlateHud"),
            w!(""),
            WS_POPUP,
            x,
            y,
            pw as i32,
            ph as i32,
            None,
            None,
            None,
            None,
        ) else {
            return;
        };
        super::layered::update(hwnd, &pix, POINT { x, y });
        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        SetTimer(Some(hwnd), 1, 1600, None);
        CURRENT.with(|c| c.set(hwnd.0 as isize));
    }
}
