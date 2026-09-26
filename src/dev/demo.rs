//! `--ui-demo [background.png] <out-dir>`: drives a capture overlay offscreen and writes one PNG per step,
//! so the UI can be reviewed without a Windows machine (like UIDemo.swift).

use std::collections::HashMap;
use std::path::Path;
use std::time::{Duration, Instant};

use tiny_skia::Pixmap;

use crate::kit::annotation::Tool;
use crate::kit::color::PALETTE;
use crate::kit::geom::{Point, Rect, Size};
use crate::kit::image::RgbaImage;
use crate::ui::capture::{CaptureView, Effect, Key, Mods, MouseButton};
use crate::ui::chrome::{StyleAction, Theme};
use crate::ui::toolbar::ToolbarAction;

/// A made-up desktop: a gradient with two "windows" holding real screenshots (or plain panels).
pub fn sample_screen(width: u32, height: u32, scale: f32, extra: &[RgbaImage]) -> Pixmap {
    let mut pix = Pixmap::new(width, height).expect("screen size");
    {
        let data = pix.data_mut();
        for y in 0..height {
            for x in 0..width {
                let i = ((y * width + x) * 4) as usize;
                let t = y as f32 / height as f32;
                let u = x as f32 / width as f32;
                data[i] = (40.0 + 60.0 * u) as u8;
                data[i + 1] = (90.0 + 80.0 * t) as u8;
                data[i + 2] = (160.0 + 60.0 * (1.0 - t)) as u8;
                data[i + 3] = 255;
            }
        }
    }
    let mut y = (80.0 * scale) as u32;
    for (n, img) in extra.iter().enumerate() {
        let x = ((60 + n as u32 * 140) as f32 * scale) as u32;
        paste(&mut pix, img, x, y);
        y += img.height + (40.0 * scale) as u32;
    }
    pix
}

fn paste(dst: &mut Pixmap, img: &RgbaImage, x: u32, y: u32) {
    let (dw, dh) = (dst.width(), dst.height());
    let data = dst.data_mut();
    for row in 0..img.height {
        let ty = y + row;
        if ty >= dh {
            break;
        }
        for col in 0..img.width {
            let tx = x + col;
            if tx >= dw {
                break;
            }
            let s = ((row * img.width + col) * 4) as usize;
            let d = ((ty * dw + tx) * 4) as usize;
            // Screenshots are opaque; store as premultiplied (same bytes).
            data[d..d + 3].copy_from_slice(&img.data[s..s + 3]);
            data[d + 3] = 255;
        }
    }
}

pub fn load_png(path: &Path) -> Option<RgbaImage> {
    RgbaImage::decode_png(&std::fs::read(path).ok()?).ok()
}

fn save(view: &CaptureView, dir: &Path, name: &str) {
    if let Some(pix) = view.render_full() {
        let path = dir.join(format!("{name}.png"));
        match pix.save_png(&path) {
            Ok(()) => println!("wrote {}", path.display()),
            Err(e) => eprintln!("failed to write {}: {e}", path.display()),
        }
    }
}

fn click(v: &mut CaptureView, p: Point) {
    v.mouse_move(p, Mods::NONE);
    v.mouse_down(p, MouseButton::Left, 1, Mods::NONE);
    v.mouse_up(p, Mods::NONE);
}

fn drag(v: &mut CaptureView, from: Point, to: Point, steps: usize) {
    v.mouse_move(from, Mods::NONE);
    v.mouse_down(from, MouseButton::Left, 1, Mods::NONE);
    for i in 1..=steps {
        let t = i as f32 / steps as f32;
        let p = Point::new(from.x + (to.x - from.x) * t, from.y + (to.y - from.y) * t);
        v.mouse_move(p, Mods::NONE);
    }
    v.mouse_up(to, Mods::NONE);
}

fn drag_path(v: &mut CaptureView, points: &[Point]) {
    v.mouse_move(points[0], Mods::NONE);
    v.mouse_down(points[0], MouseButton::Left, 1, Mods::NONE);
    for p in &points[1..] {
        v.mouse_move(*p, Mods::NONE);
    }
    v.mouse_up(*points.last().unwrap(), Mods::NONE);
}

/// Runs the scripted demo; returns false when a step didn't behave as expected.
pub fn run(background: Option<&Path>, out: &Path, scale: f32) -> bool {
    let _ = std::fs::create_dir_all(out);
    let size = Size::new(1280.0, 800.0);
    let (pw, ph) = ((size.width * scale) as u32, (size.height * scale) as u32);
    let shots: Vec<RgbaImage> = match background {
        Some(p) => load_png(p).into_iter().collect(),
        None => ["tests/data/memory.png", "tests/data/mixed.png"].iter().filter_map(|p| load_png(Path::new(p))).collect(),
    };
    let screen = sample_screen(pw, ph, scale, &shots);
    let windows: Vec<Rect> = shots
        .iter()
        .enumerate()
        .map(|(n, img)| {
            let x = (60 + n as u32 * 140) as f32;
            let y = 80.0 + shots[..n].iter().map(|s| s.height as f32 / scale + 40.0).sum::<f32>();
            Rect::new(x, y, img.width as f32 / scale, img.height as f32 / scale)
        })
        .collect();
    let mut v = CaptureView::new("demo".into(), screen, size, windows.clone(), Theme { dark: false }, true, true);
    let mut ok = true;
    let mut check = |cond: bool, what: &str| {
        if !cond {
            eprintln!("FAIL: {what}");
            ok = false;
        }
    };

    // 1. Pointer over a window: its frame is highlighted, magnifier follows.
    let over = windows.first().map_or(Point::new(300.0, 200.0), |w| Point::new(w.x + 40.0, w.y + 30.0));
    v.prime_cursor(over);
    save(&v, out, "01-hover-window");
    check(v.magnifier_visible(), "magnifier follows the pointer");

    // 2. Dragging a selection shows its size and the magnifier.
    let (a, b) = (Point::new(180.0, 150.0), Point::new(900.0, 520.0));
    v.mouse_move(a, Mods::NONE);
    v.mouse_down(a, MouseButton::Left, 1, Mods::NONE);
    for i in 1..=10 {
        let t = i as f32 / 10.0;
        v.mouse_move(Point::new(a.x + (b.x - a.x) * t, a.y + (b.y - a.y) * t), Mods::NONE);
    }
    save(&v, out, "02-selecting");
    v.mouse_up(b, Mods::NONE);
    check(v.selection() == Some(Rect::from_corners(a, b)), "drag selects a rect");
    save(&v, out, "03-selected");
    check(v.toolbar_frame().is_some(), "toolbar appears under the selection");

    // 3. Resting on a toolbar button shows its card after the delay.
    if let Some(c) = v.toolbar_button_center(ToolbarAction::Tool(Tool::Arrow)) {
        v.mouse_move(c, Mods::NONE);
        v.tick(Instant::now() + Duration::from_millis(600));
        check(v.hover_card_visible(), "hover card after 500 ms");
        save(&v, out, "04-hover-card");
        v.mouse_move(Point::new(500.0, 300.0), Mods::NONE);
        v.tick(Instant::now() + Duration::from_millis(900));
    }

    // 4. Rectangle (key 1), rounded + dashed.
    v.key_down(Key::Char('1'), Mods::NONE);
    check(v.tool() == Some(Tool::Rectangle), "key 1 picks the rectangle");
    drag(&mut v, Point::new(220.0, 190.0), Point::new(420.0, 290.0), 6);
    if let Some(p) = v.style_button_center(StyleAction::ToggleRounded) {
        click(&mut v, p);
    }
    save(&v, out, "05-rectangle");

    // 5. Arrow in green, then the pen.
    v.key_down(Key::Char('2'), Mods::NONE);
    if let Some(p) = v.style_button_center(StyleAction::Color(PALETTE[3])) {
        click(&mut v, p);
    }
    drag(&mut v, Point::new(560.0, 420.0), Point::new(440.0, 300.0), 6);
    v.key_down(Key::Char('3'), Mods::NONE);
    let pts: Vec<Point> = (0..30).map(|i| Point::new(600.0 + i as f32 * 8.0, 200.0 + (i as f32 * 0.5).sin() * 25.0)).collect();
    drag_path(&mut v, &pts);
    save(&v, out, "06-arrow-pen");

    // 6. Mosaic brush and blur rect, magnifier lens.
    v.key_down(Key::Char('4'), Mods::NONE);
    // Over the "Shotlate" row's text, so the pixelation shows.
    let pts: Vec<Point> = (0..16).map(|i| Point::new(190.0 + i as f32 * 8.0, 352.0 + (i % 3) as f32 * 3.0)).collect();
    drag_path(&mut v, &pts);
    v.key_down(Key::Char('5'), Mods::NONE);
    drag(&mut v, Point::new(700.0, 330.0), Point::new(730.0, 350.0), 5);
    save(&v, out, "07-mosaic-magnifier");

    // 7. Number with a caption, then a text annotation with a background.
    v.key_down(Key::Char('7'), Mods::NONE);
    click(&mut v, Point::new(250.0, 470.0));
    check(v.is_editing_text(), "a number opens its caption");
    v.text_changed("先点这里", 12, 12);
    save(&v, out, "08-number-caption");
    click(&mut v, Point::new(250.0, 500.0));
    v.text_changed("然后这里", 12, 12);
    v.key_down(Key::Escape, Mods::NONE);
    v.key_down(Key::Char('6'), Mods::NONE);
    click(&mut v, Point::new(600.0, 470.0));
    if let Some(p) = v.style_button_center(StyleAction::Text(crate::kit::annotation::TextDecoration::Background)) {
        click(&mut v, p);
    }
    v.text_changed("Shotlate 标注", 16, 16);
    save(&v, out, "09-text");
    v.key_down(Key::Escape, Mods::NONE);
    v.key_down(Key::Escape, Mods::NONE);
    v.key_down(Key::Escape, Mods::NONE);
    check(v.items().len() >= 9, "all annotations kept");

    // 8. OCR (recognition result simulated) and translation (translations simulated).
    v.key_down(Key::Char('x'), Mods::NONE);
    let recognize = v.drain_effects().into_iter().find_map(|e| if let Effect::Recognize(img) = e { Some(img) } else { None });
    check(recognize.is_some(), "X asks for recognition");
    let s = v.scale();
    let sel = v.selection().unwrap_or_default();
    let fake = |x: f32, y: f32, w: f32, h: f32, t: &str| (t.to_string(), Rect::new((x - sel.x) * s, (y - sel.y) * s, w * s, h * s));
    let lines = vec![
        fake(240.0, 330.0, 300.0, 18.0, "Open the settings to change the shortcut"),
        fake(240.0, 350.0, 280.0, 18.0, "and pick where screenshots are saved."),
        fake(620.0, 260.0, 160.0, 18.0, "Copy to clipboard"),
    ];
    v.recognition_finished(Ok(lines));
    check(v.ocr_text().is_some(), "OCR panel shows the text");
    save(&v, out, "10-ocr");
    v.key_down(Key::Escape, Mods::NONE);

    v.key_down(Key::Char('y'), Mods::NONE);
    let items = v.drain_effects().into_iter().find_map(|e| if let Effect::Translate(items) = e { Some(items) } else { None });
    check(items.is_some(), "Y asks for translation of the recognized paragraphs");
    save(&v, out, "11-translating");
    let map: HashMap<usize, String> = items
        .unwrap_or_default()
        .into_iter()
        .map(|i| {
            let t = match i.text.as_str() {
                t if t.starts_with("Open") => "打开设置可以修改快捷键，并选择截图保存的位置。".to_string(),
                "Copy to clipboard" => "复制到剪贴板".to_string(),
                other => format!("〔{other}〕"),
            };
            (i.id, t)
        })
        .collect();
    v.translation_finished(Ok(map));
    save(&v, out, "12-translated");

    // 9. Export: what copy / save would produce.
    match v.export_image() {
        Some(img) => {
            let path = out.join("13-export.png");
            let _ = std::fs::write(&path, img.encode_png().unwrap_or_default());
            println!("wrote {}", path.display());
            check(img.width == ((sel.width) * s).round() as u32, "export has the selection's pixel size");
        }
        None => check(false, "export"),
    }

    // 10. Selection at the bottom of the screen: the toolbar turns into a column beside it.
    let mut v2 = CaptureView::new("demo2".into(), sample_screen(pw, ph, scale, &shots), size, vec![], Theme { dark: true }, true, true);
    drag(&mut v2, Point::new(300.0, 420.0), Point::new(760.0, 790.0), 5);
    v2.key_down(Key::Char('2'), Mods::NONE);
    save(&v2, out, "14-vertical-toolbar-dark");
    check(v2.toolbar_vertical(), "toolbar goes vertical without room below");
    settings_frames(out);
    ok
}

/// `--bench-render`: how long full and partial overlay repaints take at 4K.
pub fn bench() {
    let size = Size::new(1920.0, 1080.0);
    let screen = sample_screen(3840, 2160, 2.0, &[]);
    let mut v = CaptureView::new("bench".into(), screen, size, vec![Rect::new(0.0, 0.0, 1920.0, 1040.0)], Theme { dark: false }, true, true);
    v.prime_cursor(Point::new(500.0, 400.0));
    let t = Instant::now();
    let _ = v.render_full();
    println!("full frame, hovering a full-screen window: {:?}", t.elapsed());
    let _ = v.take_dirty();
    let t = Instant::now();
    for i in 0..20 {
        v.mouse_move(Point::new(500.0 + i as f32 * 3.0, 400.0), Mods::NONE);
        if let Some((x, y, w, h)) = v.take_dirty() {
            let _ = v.render_region(x, y, w, h);
        }
    }
    println!("magnifier move (avg of 20): {:?}", t.elapsed() / 20);
    drag(&mut v, Point::new(300.0, 200.0), Point::new(1500.0, 900.0), 3);
    let _ = v.take_dirty();
    v.key_down(Key::Char('1'), Mods::NONE);
    let _ = v.take_dirty();
    let t = Instant::now();
    v.mouse_move(Point::new(400.0, 300.0), Mods::NONE);
    v.mouse_down(Point::new(400.0, 300.0), MouseButton::Left, 1, Mods::NONE);
    for i in 0..20 {
        v.mouse_move(Point::new(400.0 + i as f32 * 10.0, 300.0 + i as f32 * 5.0), Mods::NONE);
        if let Some((x, y, w, h)) = v.take_dirty() {
            let _ = v.render_region(x, y, w, h);
        }
    }
    println!("drawing a rectangle in a large selection (avg of 20): {:?}", t.elapsed() / 20);
}

/// The settings window's panes, light and dark, as PNGs (part of `--ui-demo`).
pub fn settings_frames(out: &Path) {
    use crate::kit::settings::{ImageFormat, Shortcut};
    use crate::ui::settings_view::{Models, Pane, SettingsState, SettingsView};
    for dark in [false, true] {
        let state = SettingsState {
            capture: Shortcut::CAPTURE,
            toggle_pins: Some(Shortcut::TOGGLE_PINS),
            capture_ok: true,
            toggle_ok: false,
            save_dir: "C:\\Users\\tester\\Downloads".into(),
            format: ImageFormat::Png,
            language: "简体中文".into(),
            testing: false,
            test_result: Some((true, "连接成功：截图并翻译到原位。".into())),
            login: true,
            auto_update: Some(true),
            models: if dark { Models::Downloading(0.42) } else { Models::Ready },
            models_mb: 23.0,
            models_dir: "C:\\Users\\tester\\AppData\\Local\\Shotlate\\models".into(),
            version: env!("CARGO_PKG_VERSION").into(),
        };
        let mut v = SettingsView::new(state, Theme { dark });
        for (i, pane) in Pane::ALL.iter().enumerate() {
            v.set_pane(*pane);
            if let Some(pix) = v.render(2.0) {
                let path = out.join(format!("20-settings-{}{}.png", i, if dark { "-dark" } else { "" }));
                let _ = pix.save_png(&path);
                println!("wrote {}", path.display());
            }
        }
    }
}
