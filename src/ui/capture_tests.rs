//! Scenario tests for the capture overlay, the Rust counterpart of the macOS FeatureChecks.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use tiny_skia::Pixmap;

use super::*;
use crate::kit::annotation::Tool;
use crate::kit::color::PALETTE;

fn screen(w: u32, h: u32) -> Pixmap {
    let mut p = Pixmap::new(w, h).unwrap();
    // A vertical gradient, so crops and colors are position-dependent.
    let data = p.data_mut();
    for y in 0..h {
        for x in 0..w {
            let i = ((y * w + x) * 4) as usize;
            data[i] = (x * 255 / w) as u8;
            data[i + 1] = (y * 255 / h) as u8;
            data[i + 2] = 128;
            data[i + 3] = 255;
        }
    }
    p
}

fn view_with(scale: f32, windows: Vec<Rect>) -> CaptureView {
    let size = Size::new(800.0, 600.0);
    CaptureView::new("test".into(), screen((800.0 * scale) as u32, (600.0 * scale) as u32), size, windows, Theme { dark: false }, true, true)
}

fn view() -> CaptureView {
    view_with(1.0, vec![])
}

fn drag(v: &mut CaptureView, a: Point, b: Point) {
    v.mouse_move(a, Mods::NONE);
    v.mouse_down(a, MouseButton::Left, 1, Mods::NONE);
    for i in 1..=4 {
        let t = i as f32 / 4.0;
        v.mouse_move(Point::new(a.x + (b.x - a.x) * t, a.y + (b.y - a.y) * t), Mods::NONE);
    }
    v.mouse_up(b, Mods::NONE);
}

fn click(v: &mut CaptureView, p: Point) {
    v.mouse_move(p, Mods::NONE);
    v.mouse_down(p, MouseButton::Left, 1, Mods::NONE);
    v.mouse_up(p, Mods::NONE);
}

fn selected() -> CaptureView {
    let mut v = view();
    drag(&mut v, Point::new(100.0, 100.0), Point::new(500.0, 400.0));
    v.drain_effects();
    v
}

#[test]
fn drag_selects_and_shows_toolbar() {
    let mut v = view();
    drag(&mut v, Point::new(100.0, 100.0), Point::new(700.0, 400.0));
    assert_eq!(v.selection(), Some(Rect::new(100.0, 100.0, 600.0, 300.0)));
    let bar = v.toolbar_frame().expect("toolbar");
    assert!(bar.min_y() >= 400.0, "toolbar hangs below the selection");
    assert!((bar.max_x() - 700.0).abs() < 0.5, "and ends at its right edge");
    // Narrower than the toolbar: it stays on screen instead.
    let v = selected();
    assert!(v.toolbar_frame().unwrap().min_x() >= 4.0);
}

#[test]
fn tiny_drag_is_ignored_and_click_takes_window_or_screen() {
    let mut v = view_with(1.0, vec![Rect::new(50.0, 60.0, 200.0, 150.0)]);
    drag(&mut v, Point::new(100.0, 100.0), Point::new(101.0, 101.0));
    // Moving less than 2 pt doesn't count as a drag: it is a click on the window under the pointer.
    assert_eq!(v.selection(), Some(Rect::new(50.0, 60.0, 200.0, 150.0)));
    let mut v = view();
    click(&mut v, Point::new(300.0, 300.0));
    assert_eq!(v.selection(), Some(Rect::new(0.0, 0.0, 800.0, 600.0)));
}

#[test]
fn shift_drag_makes_a_square() {
    let mut v = view();
    let (a, b) = (Point::new(100.0, 100.0), Point::new(300.0, 180.0));
    v.mouse_move(a, Mods::NONE);
    v.mouse_down(a, MouseButton::Left, 1, Mods::NONE);
    v.mouse_move(Point::new(200.0, 150.0), Mods::SHIFT);
    v.mouse_move(b, Mods::SHIFT);
    v.mouse_up(b, Mods::SHIFT);
    let s = v.selection().unwrap();
    assert_eq!(s.width, s.height);
}

#[test]
fn tool_keys_draw_and_undo_redo() {
    let mut v = selected();
    v.key_down(Key::Char('1'), Mods::NONE);
    assert_eq!(v.tool(), Some(Tool::Rectangle));
    drag(&mut v, Point::new(150.0, 150.0), Point::new(250.0, 220.0));
    v.key_down(Key::Char('2'), Mods::NONE);
    drag(&mut v, Point::new(300.0, 300.0), Point::new(400.0, 250.0));
    assert_eq!(v.items().len(), 2);
    v.key_down(Key::Char('z'), Mods::CTRL);
    assert_eq!(v.items().len(), 1);
    v.key_down(Key::Char('z'), Mods::CTRL_SHIFT);
    assert_eq!(v.items().len(), 2);
    v.key_down(Key::Char('z'), Mods::CTRL);
    v.key_down(Key::Char('y'), Mods::CTRL);
    assert_eq!(v.items().len(), 2, "Ctrl+Y redoes too");
}

#[test]
fn clicking_an_annotation_selects_and_moves_it() {
    let mut v = selected();
    v.key_down(Key::Char('1'), Mods::NONE);
    drag(&mut v, Point::new(150.0, 150.0), Point::new(250.0, 220.0));
    v.key_down(Key::Escape, Mods::NONE); // deselect
    v.key_down(Key::Escape, Mods::NONE); // drop the tool
    assert_eq!(v.tool(), None);
    // Drag its edge.
    drag(&mut v, Point::new(150.0, 180.0), Point::new(170.0, 200.0));
    match &v.items()[0].shape {
        Shape::Rectangle(r) => assert_eq!((r.x, r.y), (170.0, 170.0)),
        s => panic!("{s:?}"),
    }
    // Arrow keys nudge the selected annotation; Delete removes it.
    v.key_down(Key::Right, Mods::SHIFT);
    match &v.items()[0].shape {
        Shape::Rectangle(r) => assert_eq!(r.x, 180.0),
        s => panic!("{s:?}"),
    }
    v.key_down(Key::Delete, Mods::NONE);
    assert!(v.items().is_empty());
}

#[test]
fn escape_steps_back_then_closes() {
    let mut v = selected();
    v.key_down(Key::Char('3'), Mods::NONE);
    v.key_down(Key::Escape, Mods::NONE);
    assert_eq!(v.tool(), None);
    assert!(v.drain_effects().is_empty());
    v.key_down(Key::Escape, Mods::NONE);
    assert!(matches!(v.drain_effects().as_slice(), [Effect::Close]));
}

#[test]
fn right_click_resets_an_untouched_selection() {
    let mut v = selected();
    v.mouse_down(Point::new(200.0, 200.0), MouseButton::Right, 1, Mods::NONE);
    assert!(!v.has_selection());
    v.mouse_down(Point::new(200.0, 200.0), MouseButton::Right, 1, Mods::NONE);
    assert!(matches!(v.drain_effects().as_slice(), [Effect::Close]));
}

#[test]
fn number_opens_a_caption_and_next_click_places_the_next() {
    let mut v = selected();
    v.key_down(Key::Char('7'), Mods::NONE);
    click(&mut v, Point::new(200.0, 200.0));
    assert!(v.is_editing_text());
    let effects = v.drain_effects();
    assert!(effects.iter().any(|e| matches!(e, Effect::TextInput(Some(TextInput { initial: Some(_), .. })))), "{effects:?}");
    v.text_changed("第一步", 9, 9);
    click(&mut v, Point::new(200.0, 260.0));
    // The first caption is kept and a second number is placed, again with a caption.
    assert!(v.is_editing_text());
    let texts = v.items().iter().filter(|i| matches!(i.shape, Shape::Text { .. })).count();
    let numbers = v.items().iter().filter(|i| matches!(i.shape, Shape::Number(_))).count();
    assert_eq!((texts, numbers), (1, 2));
    // Leaving a caption empty keeps just the number.
    v.key_down(Key::Escape, Mods::NONE);
    v.commit_text();
    assert_eq!(v.items().iter().filter(|i| matches!(i.shape, Shape::Text { .. })).count(), 1);
}

#[test]
fn text_annotation_commits_on_click_elsewhere() {
    let mut v = selected();
    v.key_down(Key::Char('6'), Mods::NONE);
    click(&mut v, Point::new(150.0, 150.0));
    v.text_changed("Hello\r\n世界", 13, 13);
    click(&mut v, Point::new(300.0, 300.0));
    assert!(!v.is_editing_text());
    match &v.items()[0].shape {
        Shape::Text { text, .. } => assert_eq!(text, "Hello\n世界"),
        s => panic!("{s:?}"),
    }
    assert!(v.drain_effects().iter().any(|e| matches!(e, Effect::TextInput(None))));
}

#[test]
fn style_bar_changes_the_selected_annotation_and_is_remembered() {
    let mut v = selected();
    v.key_down(Key::Char('2'), Mods::NONE);
    drag(&mut v, Point::new(150.0, 150.0), Point::new(300.0, 200.0));
    let green = v.style_button_center(StyleAction::Color(PALETTE[3])).expect("style bar");
    click(&mut v, green);
    assert!(v.items()[0].color.is_approximately(PALETTE[3]));
    assert!(settings::color_for(Tool::Arrow).is_approximately(PALETTE[3]), "remembered for the next arrow");
    // The wheel changes the size, clamped to the tool's range.
    for _ in 0..100 {
        v.wheel(1.0, Mods::NONE);
    }
    assert_eq!(v.items()[0].size, 40.0);
    v.key_down(Key::Char('z'), Mods::CTRL);
    // Wheel steps coalesce into one undo step (after the deadline passes).
    v.tick(Instant::now() + Duration::from_secs(1));
}

#[test]
fn hover_card_appears_after_the_delay_and_keys_can_be_swapped() {
    let mut v = selected();
    let arrow = v.toolbar_button_center(ToolbarAction::Tool(Tool::Arrow)).unwrap();
    v.mouse_move(arrow, Mods::NONE);
    assert!(!v.hover_card_visible());
    v.tick(Instant::now() + Duration::from_millis(300));
    assert!(!v.hover_card_visible(), "not before 500 ms");
    v.tick(Instant::now() + Duration::from_millis(600));
    assert!(v.hover_card_visible());
    // Click the key cap, press "x": arrow takes X and OCR takes arrow's old key 2.
    let card = v.card.as_ref().map(|c| v.card_view(c)).unwrap();
    let l = chrome::hover_card_layout(&card);
    let cap = l.keycap.offset(v.card_origin.x, v.card_origin.y).center();
    v.mouse_move(cap, Mods::NONE);
    v.mouse_down(cap, MouseButton::Left, 1, Mods::NONE);
    v.mouse_up(cap, Mods::NONE);
    v.key_down(Key::Char('x'), Mods::NONE);
    assert_eq!(toolbar::key_for("arrow"), "x");
    assert_eq!(toolbar::key_for("ocr"), "2");
    v.key_down(Key::Escape, Mods::NONE);
    v.key_down(Key::Char('x'), Mods::NONE);
    assert_eq!(v.tool(), Some(Tool::Arrow));
}

#[test]
fn ocr_flow_shows_the_panel_without_copying() {
    let mut v = view_with(2.0, vec![]);
    drag(&mut v, Point::new(100.0, 100.0), Point::new(300.0, 200.0));
    v.drain_effects();
    v.key_down(Key::Char('x'), Mods::NONE);
    let img = v.drain_effects().into_iter().find_map(|e| if let Effect::Recognize(i) = e { Some(i) } else { None }).expect("recognize");
    assert_eq!((img.width, img.height), (400, 200), "crop is at pixel density");
    v.recognition_finished(Ok(vec![("Hello".into(), Rect::new(20.0, 20.0, 100.0, 30.0)), ("world".into(), Rect::new(20.0, 52.0, 100.0, 30.0))]));
    assert_eq!(v.ocr_text(), Some("Hello\nworld"));
    let effects = v.drain_effects();
    assert!(effects.iter().any(|e| matches!(e, Effect::OcrPanel(Some(_)))));
    assert!(!effects.iter().any(|e| matches!(e, Effect::CopyText(_))), "OCR never copies by itself");
    v.key_down(Key::Escape, Mods::NONE);
    assert!(v.ocr_text().is_none());
    // A second X reuses the recognition.
    v.key_down(Key::Char('x'), Mods::NONE);
    assert!(!v.drain_effects().iter().any(|e| matches!(e, Effect::Recognize(_))));
    assert!(v.ocr_text().is_some());
}

#[test]
fn missing_models_or_key_are_explained() {
    let size = Size::new(400.0, 300.0);
    let mut v = CaptureView::new("t".into(), screen(400, 300), size, vec![], Theme { dark: false }, false, false);
    drag(&mut v, Point::new(10.0, 10.0), Point::new(200.0, 200.0));
    v.drain_effects();
    v.key_down(Key::Char('x'), Mods::NONE);
    assert!(v.drain_effects().iter().any(|e| matches!(e, Effect::ModelsMissing)));
    v.key_down(Key::Char('y'), Mods::NONE);
    assert!(v.toast_text().unwrap_or_default().contains("API Key"));
}

#[test]
fn translation_flow_toggles_between_original_and_translation() {
    let mut v = selected();
    v.key_down(Key::Char('y'), Mods::NONE);
    assert!(v.drain_effects().iter().any(|e| matches!(e, Effect::Recognize(_))));
    v.recognition_finished(Ok(vec![("Open the settings".into(), Rect::new(10.0, 10.0, 200.0, 20.0)), ("中文不用翻译".into(), Rect::new(10.0, 60.0, 200.0, 20.0))]));
    let items = v.drain_effects().into_iter().find_map(|e| if let Effect::Translate(i) = e { Some(i) } else { None }).expect("translate");
    assert_eq!(items.len(), 1, "only the foreign paragraph goes out");
    let mut map = HashMap::new();
    map.insert(items[0].id, "打开设置".to_string());
    v.translation_finished(Ok(map));
    assert!(matches!(v.translation, Translation::Shown(..)));
    v.key_down(Key::Char('y'), Mods::NONE);
    assert!(matches!(v.translation, Translation::Hidden(..)), "Y shows the original again");
    v.key_down(Key::Char('y'), Mods::NONE);
    assert!(matches!(v.translation, Translation::Shown(..)));
    // Undo with no annotations hides the translation.
    v.key_down(Key::Char('z'), Mods::CTRL);
    assert!(matches!(v.translation, Translation::Hidden(..)));
}

#[test]
fn translation_error_is_shown_and_retryable() {
    let mut v = selected();
    v.key_down(Key::Char('y'), Mods::NONE);
    v.recognition_finished(Ok(vec![("Open the settings".into(), Rect::new(10.0, 10.0, 200.0, 20.0))]));
    v.drain_effects();
    v.translation_finished(Err(TranslationError::Http { status: 402, message: "x".into() }));
    assert!(v.toast_text().unwrap_or_default().contains("余额不足"));
    v.key_down(Key::Char('y'), Mods::NONE);
    assert!(v.drain_effects().iter().any(|e| matches!(e, Effect::Translate(_))), "retry sends again");
}

#[test]
fn outputs_export_the_selection_at_pixel_size() {
    for (key, mods) in [(Key::Enter, Mods::NONE), (Key::Char('c'), Mods::CTRL), (Key::Char('s'), Mods::CTRL), (Key::Char('s'), Mods::CTRL_SHIFT), (Key::Char('t'), Mods::NONE)] {
        let mut v = view_with(2.0, vec![]);
        drag(&mut v, Point::new(100.0, 100.0), Point::new(300.0, 250.0));
        v.drain_effects();
        v.key_down(key, mods);
        let effects = v.drain_effects();
        let img = match effects.first() {
            Some(Effect::Copy(i) | Effect::Save(i, _) | Effect::SaveAs(i, _) | Effect::Pin(i, _)) => i.clone(),
            other => panic!("{key:?} {mods:?}: {other:?}"),
        };
        assert_eq!((img.width, img.height), (400, 300));
        // The top-left pixel is the screen's pixel at (200, 200).
        assert_eq!(img.pixel(0, 0)[0], (200 * 255 / 1600) as u8);
    }
}

#[test]
fn double_click_copies() {
    let mut v = selected();
    v.mouse_down(Point::new(200.0, 200.0), MouseButton::Left, 1, Mods::NONE);
    v.mouse_up(Point::new(200.0, 200.0), Mods::NONE);
    v.mouse_down(Point::new(200.0, 200.0), MouseButton::Left, 2, Mods::NONE);
    assert!(matches!(v.drain_effects().first(), Some(Effect::Copy(_))));
}

#[test]
fn toolbar_turns_vertical_without_room_below() {
    let mut v = view();
    drag(&mut v, Point::new(200.0, 300.0), Point::new(500.0, 595.0));
    assert!(v.toolbar_vertical());
    let bar = v.toolbar_frame().unwrap();
    assert!(bar.min_x() >= 500.0, "beside the selection, on the right");
}

#[test]
fn keyboard_selection_without_mouse() {
    let mut v = view();
    v.prime_cursor(Point::new(10.0, 10.0));
    v.key_down(Key::Right, Mods::SHIFT);
    assert!(matches!(v.drain_effects().first(), Some(Effect::WarpCursor(p)) if p.x == 20.0));
    v.key_down(Key::Enter, Mods::NONE);
    assert_eq!(v.selection(), Some(Rect::new(0.0, 0.0, 800.0, 600.0)));
    // Ctrl+arrow grows, Shift+arrow shrinks the edge; plain arrows move.
    v.key_down(Key::Left, Mods::SHIFT);
    assert_eq!(v.selection().unwrap().x, 1.0);
}

#[test]
fn restore_last_selection_with_r() {
    // Its own monitor name: the last selection is remembered process-wide, and other tests finish captures too.
    let size = Size::new(800.0, 600.0);
    let fresh = || CaptureView::new("restore-test".into(), screen(800, 600), size, vec![], Theme { dark: false }, true, true);
    let mut v = fresh();
    drag(&mut v, Point::new(100.0, 100.0), Point::new(500.0, 400.0));
    v.key_down(Key::Enter, Mods::NONE);
    let mut v2 = fresh();
    v2.key_down(Key::Char('r'), Mods::NONE);
    assert_eq!(v2.selection(), Some(Rect::new(100.0, 100.0, 400.0, 300.0)));
}

#[test]
fn render_and_dirty_regions() {
    let mut v = selected();
    assert!(v.take_dirty().is_some());
    assert!(v.take_dirty().is_none(), "nothing changed since");
    v.key_down(Key::Char('1'), Mods::NONE);
    let (x, y, w, h) = v.take_dirty().expect("tool change repaints");
    let pix = v.render_region(x, y, w, h).unwrap();
    assert_eq!((pix.width(), pix.height()), (w, h));
    assert!(v.render_full().is_some());
}

#[test]
fn ime_composition_shows_at_the_caret_until_committed() {
    let mut v = selected();
    v.key_down(Key::Char('6'), Mods::NONE);
    click(&mut v, Point::new(150.0, 150.0));
    v.text_changed("ab", 1, 1);
    let before = v.editor_frame().unwrap().width;
    v.composition_changed("nihao");
    assert!(v.editor_frame().unwrap().width > before, "composition widens the edited text");
    let caret_with = v.caret_rect().unwrap().x;
    v.composition_changed("");
    assert!(v.caret_rect().unwrap().x < caret_with);
    v.text_changed("a你好b", 7, 7);
    v.commit_text();
    match &v.items()[0].shape {
        Shape::Text { text, .. } => assert_eq!(text, "a你好b"),
        s => panic!("{s:?}"),
    }
}
