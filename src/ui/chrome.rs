//! The capture overlay's panels: toolbar, hover card, style bar, size bar, toast, magnifier and the
//! OCR result panel. Layout math and drawing, ported from Chrome.swift / Magnifier.swift.

use tiny_skia::PathBuilder;

use super::icons::{self, Icon};
use super::toolbar::ToolbarAction;
use crate::kit::annotation::{ArrowHead, DashStyle, ItemStyle, MosaicEffect, MosaicMode, TextDecoration, Tool};
use crate::kit::color::{Color, PALETTE, SELECTION_BLUE};
use crate::kit::geom::{Point, Rect, Size};
use crate::render::canvas::{Canvas, rounded_rect_path};
use crate::render::text::{self, Weight};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Theme {
    pub dark: bool,
}

impl Theme {
    pub fn panel(&self) -> Color {
        if self.dark { Color::rgba(0.17, 0.17, 0.18, 0.98) } else { Color::rgba(0.97, 0.97, 0.97, 0.98) }
    }
    pub fn label(&self) -> Color {
        if self.dark { Color::gray(0.93, 1.0) } else { Color::rgba(0.11, 0.11, 0.12, 1.0) }
    }
    pub fn secondary(&self) -> Color {
        self.label().with_alpha(0.55)
    }
    pub fn separator(&self) -> Color {
        if self.dark { Color::white(0.14) } else { Color::black(0.12) }
    }
    pub fn hover(&self) -> Color {
        self.label().with_alpha(0.08)
    }
    pub fn text_background(&self) -> Color {
        if self.dark { Color::gray(0.12, 1.0) } else { Color::white(1.0) }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaretEdge {
    Top,
    Bottom,
    Left,
    Right,
}

impl CaretEdge {
    pub fn is_vertical(self) -> bool {
        matches!(self, CaretEdge::Top | CaretEdge::Bottom)
    }
}

pub const CARET: f32 = 6.0;

/// The panel body inside `frame`, excluding the caret.
pub fn content_rect(frame: &Rect, caret: Option<CaretEdge>) -> Rect {
    let mut r = *frame;
    match caret {
        Some(CaretEdge::Top) => {
            r.y += CARET;
            r.height -= CARET;
        }
        Some(CaretEdge::Bottom) => r.height -= CARET,
        Some(CaretEdge::Left) => {
            r.x += CARET;
            r.width -= CARET;
        }
        Some(CaretEdge::Right) => r.width -= CARET,
        None => {}
    }
    r
}

/// A rounded panel with a soft shadow and an optional caret pointing at what it belongs to.
/// `caret` offset is along the edge, relative to `frame`'s origin.
pub fn draw_panel(c: &mut Canvas, frame: &Rect, radius: f32, fill: Color, border: Option<Color>, caret: Option<(CaretEdge, f32)>, shadow: bool) {
    let body = content_rect(frame, caret.map(|c| c.0)).inset(0.5, 0.5);
    let Some(body_path) = rounded_rect_path(&body, radius) else { return };
    let mut pb = PathBuilder::new();
    pb.push_path(&body_path);
    if let Some((edge, offset)) = caret {
        let h = CARET;
        if edge.is_vertical() {
            let x = (frame.x + offset).clamp(body.min_x() + radius + 6.0, body.max_x() - radius - 6.0);
            let (base, tip) = if edge == CaretEdge::Top { (body.min_y() + 1.0, body.min_y() - h + 0.5) } else { (body.max_y() - 1.0, body.max_y() + h - 0.5) };
            pb.move_to(x - 7.0, base);
            pb.line_to(x, tip);
            pb.line_to(x + 7.0, base);
            pb.close();
        } else {
            let y = (frame.y + offset).clamp(body.min_y() + radius + 6.0, body.max_y() - radius - 6.0);
            let (base, tip) = if edge == CaretEdge::Left { (body.min_x() + 1.0, body.min_x() - h + 0.5) } else { (body.max_x() - 1.0, body.max_x() + h - 0.5) };
            pb.move_to(base, y - 7.0);
            pb.line_to(tip, y);
            pb.line_to(base, y + 7.0);
            pb.close();
        }
    }
    let Some(path) = pb.finish() else { return };
    if shadow {
        c.shadow(&path, (0.0, 3.0), 10.0, Color::black(0.28));
    }
    c.fill_path(&path, fill);
    if let Some(border) = border {
        c.stroke_path(&path, border, &crate::render::canvas::stroke(0.5));
    }
}

// MARK: Toolbar

pub const BUTTON: f32 = 32.0;
pub const BUTTON_GAP: f32 = 4.0;

pub fn toolbar_actions() -> Vec<ToolbarAction> {
    ToolbarAction::all()
}

pub fn toolbar_size(vertical: bool) -> Size {
    let n = toolbar_actions().len() as f32;
    let run = n * BUTTON + (n - 1.0) * BUTTON_GAP;
    if vertical { Size::new(BUTTON + 12.0, run + 16.0) } else { Size::new(run + 20.0, BUTTON + 12.0) }
}

/// Button `i`'s rect relative to the toolbar's origin.
pub fn toolbar_button(i: usize, vertical: bool) -> Rect {
    let step = i as f32 * (BUTTON + BUTTON_GAP);
    if vertical { Rect::new(6.0, 8.0 + step, BUTTON, BUTTON) } else { Rect::new(10.0 + step, 6.0, BUTTON, BUTTON) }
}

pub fn action_icon(action: ToolbarAction) -> Icon {
    match action {
        ToolbarAction::Tool(t) => Icon::Tool(t),
        ToolbarAction::Undo => Icon::Undo,
        ToolbarAction::Ocr => Icon::Ocr,
        ToolbarAction::Translate => Icon::Translate,
        ToolbarAction::Pin => Icon::Pin,
        ToolbarAction::Scroll => Icon::Scroll,
        ToolbarAction::Cancel => Icon::Cancel,
        ToolbarAction::Save => Icon::Save,
        ToolbarAction::Done => Icon::Done,
    }
}

pub struct ToolbarLook {
    pub origin: Point,
    pub vertical: bool,
    pub active_tool: Option<Tool>,
    pub translate_active: bool,
    pub can_undo: bool,
    pub hovered: Option<ToolbarAction>,
    pub pressed: Option<ToolbarAction>,
}

pub fn draw_toolbar(c: &mut Canvas, t: &ToolbarLook, theme: &Theme) {
    let size = toolbar_size(t.vertical);
    let frame = Rect::from_origin_size(t.origin, size);
    draw_panel(c, &frame, 9.0, theme.panel(), Some(theme.separator()), None, true);
    for (i, action) in toolbar_actions().into_iter().enumerate() {
        let r = toolbar_button(i, t.vertical).offset(t.origin.x, t.origin.y);
        let active = match action {
            ToolbarAction::Tool(tool) => t.active_tool == Some(tool),
            ToolbarAction::Translate => t.translate_active,
            _ => false,
        };
        let enabled = action != ToolbarAction::Undo || t.can_undo;
        if active {
            c.fill_rounded(&r.inset(1.0, 1.0), 6.0, SELECTION_BLUE.with_alpha(0.18));
        } else if enabled && (t.hovered == Some(action) || t.pressed == Some(action)) {
            c.fill_rounded(&r.inset(1.0, 1.0), 6.0, theme.hover());
        }
        let mut color = if active { SELECTION_BLUE } else if action == ToolbarAction::Done { SELECTION_BLUE } else { theme.label() };
        if !enabled {
            color = color.with_alpha(0.35);
        }
        icons::draw(c, action_icon(action), r.center(), color, theme.panel());
    }
}

// MARK: Hover card

pub struct HoverCard {
    pub title: String,
    pub shortcut: String,
    pub editable: bool,
    pub note: Option<String>,
    pub recording: bool,
}

pub struct HoverCardLayout {
    pub size: Size,
    pub title: Point,
    pub label: Point,
    pub keycap: Rect,
    pub note: Option<(Point, Vec<String>)>,
}

pub fn hover_card_layout(card: &HoverCard) -> HoverCardLayout {
    let pad = 12.0;
    let key_text = if card.recording { "按下新按键" } else { card.shortcut.as_str() };
    let (tw, th) = text::measure(&card.title, 13.0, Weight::Bold, None);
    let (sw, sh) = text::measure("快捷键", 12.0, Weight::Regular, None);
    let (kw, _) = text::measure(key_text, 12.0, Weight::Regular, None);
    let hint = (card.editable && !card.recording).then_some("点击按键可修改");
    let lines: Vec<String> = [if card.recording { Some("Esc 取消") } else { card.note.as_deref() }, hint].into_iter().flatten().map(String::from).collect();
    let title = Point::new(pad, 9.0);
    let row_y = title.y + th + 7.0;
    let key_size = Size::new((kw + 12.0).max(24.0), 20.0);
    let label = Point::new(pad, row_y + (key_size.height - sh) / 2.0);
    let keycap = Rect::new(label.x + sw + 8.0, row_y, key_size.width, key_size.height);
    let mut height = row_y + key_size.height + 10.0;
    let mut width = tw.max(keycap.max_x() - pad);
    let mut note = None;
    if !lines.is_empty() {
        let joined = lines.join("\n");
        let (nw, nh) = text::measure(&joined, 11.0, Weight::Regular, None);
        note = Some((Point::new(pad, height - 3.0), lines));
        height = height - 3.0 + nh + 9.0;
        width = width.max(nw);
    }
    HoverCardLayout { size: Size::new((width + pad * 2.0).ceil(), height.ceil()), title, label, keycap, note }
}

pub fn draw_hover_card(c: &mut Canvas, origin: Point, card: &HoverCard) {
    let l = hover_card_layout(card);
    let frame = Rect::from_origin_size(origin, l.size);
    draw_panel(c, &frame, 8.0, Color::gray(0.16, 0.96), None, None, true);
    let at = |p: Point| Point::new(origin.x + p.x, origin.y + p.y);
    c.text(&card.title, at(l.title), 13.0, Weight::Bold, Color::white(1.0));
    c.text("快捷键", at(l.label), 12.0, Weight::Regular, Color::white(0.7));
    let cap = l.keycap.offset(origin.x, origin.y);
    c.fill_rounded(&cap, 4.0, if card.recording { SELECTION_BLUE } else { Color::white(1.0) });
    let key_text = if card.recording { "按下新按键" } else { card.shortcut.as_str() };
    c.text_centered(key_text, &cap, 12.0, Weight::Regular, if card.recording { Color::white(1.0) } else { Color::black(1.0) });
    if let Some((p, lines)) = &l.note {
        c.text(&lines.join("\n"), at(*p), 11.0, Weight::Regular, Color::white(0.55));
    }
}

// MARK: Style bar

#[derive(Clone, Debug, PartialEq)]
pub struct StyleState {
    pub tool: Tool,
    pub color: Color,
    pub size: f32,
    pub mosaic_mode: MosaicMode,
    pub mosaic_effect: MosaicEffect,
    pub options: ItemStyle,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum StyleAction {
    Size(f32),
    Color(Color),
    CustomColor,
    MosaicMode(MosaicMode),
    MosaicEffect(MosaicEffect),
    Dash(DashStyle),
    ArrowHead(ArrowHead),
    ToggleRounded,
    Text(TextDecoration),
}

#[derive(Clone, Debug, PartialEq)]
pub enum StyleItem {
    Button { rect: Rect, action: StyleAction, icon: StyleIcon, active: bool, tip: &'static str },
    Separator(Rect),
    SizeLabel(Rect, String),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum StyleIcon {
    Icon(Icon),
    Swatch(Color),
    Rainbow,
}

/// The style bar's items relative to its content origin, and the content size.
pub fn style_items(s: &StyleState) -> (Vec<StyleItem>, Size) {
    let mut items = Vec::new();
    let mut x = 0.0;
    let height = if s.tool == Tool::Mosaic { 28.0 } else { 26.0 };
    let gap = 2.0;
    let mut button = |items: &mut Vec<StyleItem>, x: &mut f32, size: f32, action: StyleAction, icon: StyleIcon, active: bool, tip: &'static str| {
        items.push(StyleItem::Button { rect: Rect::new(*x, (height - size) / 2.0, size, size), action, icon, active, tip });
        *x += size + gap;
    };
    let separator = |items: &mut Vec<StyleItem>, x: &mut f32| {
        items.push(StyleItem::Separator(Rect::new(*x + 3.0, (height - 16.0) / 2.0, 1.0, 16.0)));
        *x += 7.0 + gap;
    };
    let sizes = |items: &mut Vec<StyleItem>, x: &mut f32, button: &mut dyn FnMut(&mut Vec<StyleItem>, &mut f32, f32, StyleAction, StyleIcon, bool, &'static str)| {
        for (i, value) in s.tool.size_presets().iter().enumerate() {
            let tip = ["小（滚轮可微调）", "中（滚轮可微调）", "大（滚轮可微调）"][i];
            button(items, x, 26.0, StyleAction::Size(*value), StyleIcon::Icon(Icon::SizeDot(i as u8)), (value - s.size).abs() < 0.5, tip);
        }
        items.push(StyleItem::SizeLabel(Rect::new(*x, 0.0, 28.0, height), format!("{}", s.size.round() as i32)));
        *x += 28.0 + gap;
    };
    if s.tool == Tool::Mosaic {
        for (mode, tip) in [(MosaicMode::Brush, "画笔涂抹"), (MosaicMode::Rect, "框选区域")] {
            button(&mut items, &mut x, 28.0, StyleAction::MosaicMode(mode), StyleIcon::Icon(Icon::MosaicMode(mode)), s.mosaic_mode == mode, tip);
        }
        separator(&mut items, &mut x);
        for (effect, tip) in [(MosaicEffect::Pixelate, "格子"), (MosaicEffect::Blur, "毛玻璃")] {
            button(&mut items, &mut x, 28.0, StyleAction::MosaicEffect(effect), StyleIcon::Icon(Icon::MosaicEffect(effect)), s.mosaic_effect == effect, tip);
        }
        if s.mosaic_mode == MosaicMode::Brush {
            separator(&mut items, &mut x);
            sizes(&mut items, &mut x, &mut button);
        }
    } else {
        sizes(&mut items, &mut x, &mut button);
        if s.tool == Tool::Arrow {
            separator(&mut items, &mut x);
            for (head, tip) in [(ArrowHead::Tapered, "实心箭头"), (ArrowHead::Open, "线条箭头"), (ArrowHead::Double, "双向箭头")] {
                button(&mut items, &mut x, 26.0, StyleAction::ArrowHead(head), StyleIcon::Icon(Icon::Arrow(head)), s.options.arrow_head == head, tip);
            }
        }
        if matches!(s.tool, Tool::Rectangle | Tool::Arrow | Tool::Pen) {
            separator(&mut items, &mut x);
            for (dash, tip) in [(DashStyle::Solid, "实线"), (DashStyle::Dashed, "虚线"), (DashStyle::Dotted, "点线")] {
                button(&mut items, &mut x, 26.0, StyleAction::Dash(dash), StyleIcon::Icon(Icon::Dash(dash)), s.options.dash == dash, tip);
            }
        }
        if s.tool == Tool::Rectangle {
            button(&mut items, &mut x, 26.0, StyleAction::ToggleRounded, StyleIcon::Icon(Icon::Rounded), s.options.rounded, "圆角矩形");
        }
        if s.tool == Tool::Text {
            separator(&mut items, &mut x);
            for (d, tip) in [(TextDecoration::Plain, "普通文字"), (TextDecoration::Background, "文字加底色"), (TextDecoration::Outline, "文字描边")] {
                button(&mut items, &mut x, 26.0, StyleAction::Text(d), StyleIcon::Icon(Icon::Text(d)), s.options.text == d, tip);
            }
        }
        separator(&mut items, &mut x);
        let mut matched = false;
        for color in PALETTE {
            let active = color.is_approximately(s.color);
            matched |= active;
            button(&mut items, &mut x, 24.0, StyleAction::Color(color), StyleIcon::Swatch(color), active, "颜色");
        }
        button(&mut items, &mut x, 24.0, StyleAction::CustomColor, StyleIcon::Rainbow, !matched, "自定义颜色");
    }
    (items, Size::new(x - gap, height))
}

/// Frame size of the style bar for `state`, with room for a caret on `caret` edge.
pub fn style_bar_size(s: &StyleState, caret: Option<CaretEdge>) -> Size {
    let (_, content) = style_items(s);
    let sideways = caret.is_some_and(|c| !c.is_vertical());
    let caret_size = if caret.is_some() { CARET } else { 0.0 };
    Size::new(content.width + 16.0 + if sideways { caret_size } else { 0.0 }, content.height + 8.0 + if sideways { 0.0 } else { caret_size })
}

/// Content origin of the style bar (inside padding and caret).
pub fn style_content_origin(frame: &Rect, caret: Option<CaretEdge>) -> Point {
    let body = content_rect(frame, caret);
    Point::new(body.x + 8.0, body.y + 4.0)
}

pub fn draw_style_bar(c: &mut Canvas, frame: &Rect, caret: (CaretEdge, f32), s: &StyleState, hovered: Option<usize>, theme: &Theme) {
    draw_panel(c, frame, 9.0, theme.panel(), Some(theme.separator()), Some(caret), true);
    let origin = style_content_origin(frame, Some(caret.0));
    let (items, _) = style_items(s);
    for (i, item) in items.iter().enumerate() {
        match item {
            StyleItem::Button { rect, icon, active, .. } => {
                let r = rect.offset(origin.x, origin.y);
                if *active {
                    c.fill_rounded(&r.inset(1.0, 1.0), 6.0, SELECTION_BLUE.with_alpha(0.18));
                } else if hovered == Some(i) {
                    c.fill_rounded(&r.inset(1.0, 1.0), 6.0, theme.hover());
                }
                let color = if *active { SELECTION_BLUE } else { theme.label() };
                match icon {
                    StyleIcon::Icon(icon) => icons::draw(c, *icon, r.center(), color, theme.panel()),
                    StyleIcon::Swatch(col) => icons::swatch(c, r.center(), *col),
                    StyleIcon::Rainbow => icons::rainbow_swatch(c, r.center()),
                }
            }
            StyleItem::Separator(r) => c.fill_rect(&r.offset(origin.x, origin.y), theme.separator()),
            StyleItem::SizeLabel(r, s) => c.text_centered(s, &r.offset(origin.x, origin.y), 11.0, Weight::Regular, theme.secondary()),
        }
    }
}

// MARK: Size bar, toast

pub fn size_text(size: Size) -> String {
    format!("{} × {}", size.width.round() as i32, size.height.round() as i32)
}

pub fn top_bar_size(size: Size) -> Size {
    let (w, _) = text::measure(&size_text(size), 12.0, Weight::Bold, None);
    Size::new(w.ceil() + 18.0, 24.0)
}

pub fn draw_top_bar(c: &mut Canvas, origin: Point, size: Size) {
    let s = top_bar_size(size);
    let r = Rect::from_origin_size(origin, s);
    c.fill_rounded(&r, 6.0, SELECTION_BLUE);
    c.text_centered(&size_text(size), &r, 12.0, Weight::Bold, Color::white(1.0));
}

pub fn toast_layout(message: &str, max_width: f32) -> (Size, crate::render::text::TextLayout) {
    let wrap = (max_width - 24.0).clamp(120.0, 380.0);
    let l = text::layout(message, 12.0, Weight::Bold, Some(wrap));
    (Size::new(l.width.ceil() + 24.0, l.height.ceil() + 14.0), l)
}

pub fn draw_toast(c: &mut Canvas, origin: Point, message: &str, max_width: f32, alpha: f32) {
    let (size, l) = toast_layout(message, max_width);
    let r = Rect::from_origin_size(origin, size);
    c.fill_rounded(&r, 7.0, Color::gray(0.1, 0.85 * alpha));
    c.text_layout(&l, Point::new(origin.x + 12.0, origin.y + 7.0), Color::white(alpha));
}

// MARK: Magnifier

pub const MAGNIFIER_CELL: f32 = 8.0;
pub const MAGNIFIER_CELLS: i32 = 15;
pub const MAGNIFIER_INFO: f32 = 50.0;

pub fn magnifier_size() -> Size {
    let side = MAGNIFIER_CELL * MAGNIFIER_CELLS as f32;
    Size::new(side, side + MAGNIFIER_INFO)
}

/// Where the magnifier sits for the cursor at `p`: below-right, flipped when it would leave `bounds`.
pub fn magnifier_origin(p: Point, bounds: &Rect) -> Point {
    let s = magnifier_size();
    let offset = 20.0;
    let mut o = Point::new(p.x + offset, p.y + offset);
    if o.x + s.width > bounds.max_x() - 4.0 {
        o.x = p.x - offset - s.width;
    }
    if o.y + s.height > bounds.max_y() - 4.0 {
        o.y = p.y - offset - s.height;
    }
    o
}

pub fn color_string(rgb: (u8, u8, u8), hex: bool) -> String {
    if hex { format!("#{:02X}{:02X}{:02X}", rgb.0, rgb.1, rgb.2) } else { format!("{}, {}, {}", rgb.0, rgb.1, rgb.2) }
}

/// Draws the zoomed pixels around `pixel` of `shot` (whose pixels are `pixels_per_point` per point).
pub fn draw_magnifier(c: &mut Canvas, origin: Point, shot: &tiny_skia::Pixmap, pixel: (i32, i32), rgb: (u8, u8, u8), top_text: &str, hex: bool) {
    let s = magnifier_size();
    let side = MAGNIFIER_CELL * MAGNIFIER_CELLS as f32;
    let frame = Rect::from_origin_size(origin, s);
    let Some(outer) = rounded_rect_path(&frame.inset(0.5, 0.5), 8.0) else { return };
    c.shadow(&outer, (0.0, 2.0), 8.0, Color::black(0.35));
    c.fill_path(&outer, Color::gray(0.12, 0.95));
    c.save();
    c.clip_path(&outer);
    c.fill_rect(&Rect::new(origin.x, origin.y, side, side), Color::black(1.0));
    // Zoomed pixels: sample each cell (cheap, 225 rects).
    let half = MAGNIFIER_CELLS / 2;
    for j in 0..MAGNIFIER_CELLS {
        for i in 0..MAGNIFIER_CELLS {
            let (x, y) = (pixel.0 - half + i, pixel.1 - half + j);
            if x < 0 || y < 0 || x >= shot.width() as i32 || y >= shot.height() as i32 {
                continue;
            }
            if let Some(p) = shot.pixel(x as u32, y as u32) {
                let col = p.demultiply();
                c.fill_rect(
                    &Rect::new(origin.x + i as f32 * MAGNIFIER_CELL, origin.y + j as f32 * MAGNIFIER_CELL, MAGNIFIER_CELL + 0.2, MAGNIFIER_CELL + 0.2),
                    Color::from_rgb8(col.red(), col.green(), col.blue()),
                );
            }
        }
    }
    let cell = MAGNIFIER_CELL;
    let center = half as f32 * cell;
    // Crosshair through the center row and column, a faint pixel grid, and the center pixel ringed.
    c.fill_rect(&Rect::new(origin.x, origin.y + center, side, cell), SELECTION_BLUE.with_alpha(0.28));
    c.fill_rect(&Rect::new(origin.x + center, origin.y, cell, side), SELECTION_BLUE.with_alpha(0.28));
    for i in 1..MAGNIFIER_CELLS {
        let v = i as f32 * cell;
        c.fill_rect(&Rect::new(origin.x + v, origin.y, 0.5, side), Color::white(0.07));
        c.fill_rect(&Rect::new(origin.x, origin.y + v, side, 0.5), Color::white(0.07));
    }
    let b = Rect::new(origin.x + center, origin.y + center, cell, cell);
    c.stroke_rect(&b.inset(-1.0, -1.0), Color::black(1.0), 1.5);
    c.stroke_rect(&b.inset(0.5, 0.5), Color::white(1.0), 1.0);
    c.fill_rect(&Rect::new(origin.x, origin.y + side, side, 0.5), Color::white(0.15));
    c.restore();

    c.text(top_text, Point::new(origin.x + 8.0, origin.y + side + 5.0), 10.5, Weight::Regular, Color::white(1.0));
    let swatch = Rect::new(origin.x + 8.0, origin.y + side + 22.0, 11.0, 11.0);
    c.fill_rounded(&swatch, 2.0, Color::from_rgb8(rgb.0, rgb.1, rgb.2));
    c.stroke_rounded(&swatch.inset(0.25, 0.25), 2.0, Color::white(0.5), 1.0);
    c.text(&color_string(rgb, hex), Point::new(origin.x + 24.0, origin.y + side + 20.0), 10.5, Weight::Regular, Color::white(1.0));
    c.text("C 复制  Shift 切换格式", Point::new(origin.x + 8.0, origin.y + side + 35.0), 9.5, Weight::Regular, Color::white(0.55));
    c.stroke_path(&outer, Color::white(0.25), &crate::render::canvas::stroke(1.0));
}

// MARK: OCR panel

pub const OCR_PANEL: Size = Size::new(340.0, 262.0);

/// Where the editable text goes, relative to the panel origin (the platform puts a real edit control there).
pub fn ocr_text_rect() -> Rect {
    Rect::new(12.0, 38.0, 316.0, 178.0)
}

pub fn ocr_copy_button() -> Rect {
    Rect::new(OCR_PANEL.width - 12.0 - 90.0, 224.0, 90.0, 28.0)
}

pub fn ocr_close_button() -> Rect {
    Rect::new(OCR_PANEL.width - 32.0, 9.0, 22.0, 22.0)
}

pub fn draw_ocr_panel(c: &mut Canvas, origin: Point, line_count: usize, text: Option<&str>, copied: bool, hovered: Option<u8>, theme: &Theme) {
    let frame = Rect::from_origin_size(origin, OCR_PANEL);
    draw_panel(c, &frame, 12.0, theme.panel(), Some(theme.separator()), None, true);
    c.text(&format!("识别结果 · {line_count} 行"), Point::new(origin.x + 14.0, origin.y + 11.0), 13.0, Weight::Bold, theme.label());
    let close = ocr_close_button().offset(origin.x, origin.y);
    if hovered == Some(0) {
        c.fill_rounded(&close.inset(1.0, 1.0), 6.0, theme.hover());
    }
    icons::draw(c, Icon::Close, close.center(), theme.label(), theme.panel());
    let area = ocr_text_rect().offset(origin.x, origin.y);
    c.fill_rounded(&area, 7.0, theme.text_background());
    // The platform's edit control covers this area; the drawn text is for offscreen renders.
    if let Some(t) = text {
        c.save();
        c.clip_rect(&area);
        let l = text::layout(t, 13.0, Weight::Regular, Some(area.width - 8.0));
        c.text_layout(&l, Point::new(area.x + 4.0, area.y + 6.0), theme.label());
        c.restore();
    }
    let b = ocr_copy_button().offset(origin.x, origin.y);
    c.fill_rounded(&b, 6.0, if hovered == Some(1) { SELECTION_BLUE.with_alpha(0.9) } else { SELECTION_BLUE });
    c.text_centered(if copied { "已复制 ✓" } else { "复制" }, &b, 13.0, Weight::Regular, Color::white(1.0));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toolbar_geometry() {
        let h = toolbar_size(false);
        let v = toolbar_size(true);
        assert_eq!(h.height, 44.0);
        assert_eq!(v.width, 44.0);
        let last = toolbar_button(toolbar_actions().len() - 1, false);
        assert_eq!(last.max_x() + 10.0, h.width);
    }

    #[test]
    fn style_items_per_tool() {
        let s = StyleState {
            tool: Tool::Arrow,
            color: PALETTE[0],
            size: 4.0,
            mosaic_mode: MosaicMode::Brush,
            mosaic_effect: MosaicEffect::Pixelate,
            options: ItemStyle::default(),
        };
        let (items, size) = style_items(&s);
        let buttons = items.iter().filter(|i| matches!(i, StyleItem::Button { .. })).count();
        // 3 sizes + 3 heads + 3 dashes + 8 colors + custom.
        assert_eq!(buttons, 18);
        assert!(size.width > 300.0);
        let mosaic = StyleState { tool: Tool::Mosaic, mosaic_mode: MosaicMode::Rect, ..s };
        let (items, _) = style_items(&mosaic);
        assert_eq!(items.iter().filter(|i| matches!(i, StyleItem::Button { .. })).count(), 4);
    }
}
