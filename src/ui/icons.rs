//! Toolbar and style-bar icons, drawn as vector paths (SF Symbols can't ship on Windows).
//! Each icon draws in a 16×16 box centered at `c`.

use tiny_skia::{LineCap, PathBuilder, Stroke, StrokeDash};

use crate::kit::annotation::{ArrowHead, DashStyle, MosaicEffect, MosaicMode, TextDecoration, Tool};
use crate::kit::color::Color;
use crate::kit::geom::{Point, Rect};
use crate::render::canvas::{Canvas, polyline_path, rounded_rect_path, stroke};
use crate::render::text::Weight;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Icon {
    Tool(Tool),
    Undo,
    Ocr,
    Translate,
    Pin,
    Cancel,
    Save,
    Done,
    Close,
    SizeDot(u8),
    Dash(DashStyle),
    Rounded,
    Arrow(ArrowHead),
    Text(TextDecoration),
    MosaicMode(MosaicMode),
    MosaicEffect(MosaicEffect),
}

const W: f32 = 1.35;

fn line_stroke() -> Stroke {
    stroke(W)
}

fn poly(c: &mut Canvas, pts: &[(f32, f32)], origin: Point, color: Color, close: bool) {
    let p: Vec<Point> = pts.iter().map(|(x, y)| Point::new(origin.x + x, origin.y + y)).collect();
    if let Some(path) = polyline_path(&p, close) {
        c.stroke_path(&path, color, &line_stroke());
    }
}

/// Draws `icon` centered at `center` in `color`. `background` is the panel color, for knockouts.
pub fn draw(c: &mut Canvas, icon: Icon, center: Point, color: Color, background: Color) {
    let o = Point::new(center.x - 8.0, center.y - 8.0);
    let at = |x: f32, y: f32| Point::new(o.x + x, o.y + y);
    match icon {
        Icon::Tool(Tool::Rectangle) => c.stroke_rounded(&Rect::new(o.x + 2.0, o.y + 2.5, 12.0, 11.0), 1.5, color, W),
        Icon::Tool(Tool::Arrow) => {
            poly(c, &[(3.0, 13.0), (13.0, 3.0)], o, color, false);
            poly(c, &[(6.5, 3.0), (13.0, 3.0), (13.0, 9.5)], o, color, false);
        }
        Icon::Tool(Tool::Pen) => {
            let mut pb = PathBuilder::new();
            pb.move_to(o.x + 2.0, o.y + 11.0);
            pb.cubic_to(o.x + 4.0, o.y + 3.0, o.x + 7.0, o.y + 3.0, o.x + 7.5, o.y + 8.0);
            pb.cubic_to(o.x + 8.0, o.y + 13.0, o.x + 11.5, o.y + 13.0, o.x + 14.0, o.y + 5.0);
            if let Some(p) = pb.finish() {
                c.stroke_path(&p, color, &line_stroke());
            }
        }
        Icon::Tool(Tool::Mosaic) => {
            let r = Rect::new(o.x + 2.0, o.y + 2.5, 12.0, 11.0);
            c.stroke_rounded(&r, 1.5, color, W);
            for (i, j) in [(0, 0), (1, 1), (2, 0), (0, 2), (2, 2)] {
                c.fill_rect(&Rect::new(r.x + 1.5 + i as f32 * 3.0, r.y + 1.5 + j as f32 * 2.7, 3.0, 2.7), color);
            }
        }
        Icon::Tool(Tool::Magnifier) => {
            c.stroke_oval(&Rect::new(o.x + 1.5, o.y + 1.5, 10.0, 10.0), color, W);
            poly(c, &[(10.0, 10.0), (14.5, 14.5)], o, color, false);
            poly(c, &[(6.5, 4.0), (6.5, 9.0)], o, color, false);
            poly(c, &[(4.0, 6.5), (9.0, 6.5)], o, color, false);
        }
        Icon::Tool(Tool::Text) => {
            c.text_centered("A", &Rect::new(o.x, o.y + 0.5, 11.0, 15.0), 12.0, Weight::Regular, color);
            poly(c, &[(13.0, 2.0), (13.0, 14.0)], o, color, false);
            poly(c, &[(11.5, 2.0), (14.5, 2.0)], o, color, false);
            poly(c, &[(11.5, 14.0), (14.5, 14.0)], o, color, false);
        }
        Icon::Tool(Tool::Number) => {
            c.stroke_oval(&Rect::new(o.x + 1.5, o.y + 1.5, 13.0, 13.0), color, W);
            c.text_centered("1", &Rect::new(o.x, o.y, 16.0, 16.0), 9.5, Weight::Bold, color);
        }
        Icon::Undo => {
            let mut pb = PathBuilder::new();
            pb.move_to(o.x + 5.0, o.y + 5.0);
            pb.line_to(o.x + 10.0, o.y + 5.0);
            pb.cubic_to(o.x + 16.0, o.y + 5.0, o.x + 16.0, o.y + 13.5, o.x + 10.0, o.y + 13.5);
            pb.line_to(o.x + 5.0, o.y + 13.5);
            if let Some(p) = pb.finish() {
                c.stroke_path(&p, color, &line_stroke());
            }
            poly(c, &[(7.5, 2.0), (4.5, 5.0), (7.5, 8.0)], o, color, false);
        }
        Icon::Ocr => {
            let r = Rect::new(o.x - 3.0, o.y + 2.5, 22.0, 11.0);
            c.fill_rounded(&r, 2.5, color);
            c.text_centered("OCR", &r, 8.0, Weight::Bold, background);
        }
        Icon::Translate => {
            c.text_centered("文", &Rect::new(o.x - 1.0, o.y - 1.0, 11.0, 11.0), 9.0, Weight::Regular, color);
            c.text_centered("A", &Rect::new(o.x + 7.0, o.y + 6.0, 10.0, 11.0), 9.5, Weight::Regular, color);
        }
        Icon::Pin => {
            // Push pin, tilted like SF Symbols' "pin".
            c.save();
            c.translate(center.x, center.y);
            c.scale(1.0, 1.0);
            let head = Rect::new(-3.5, -7.0, 7.0, 4.0);
            c.stroke_rounded(&head, 1.0, color, W);
            poly(c, &[(-2.5, -3.0), (-3.5, 2.0), (3.5, 2.0), (2.5, -3.0)], Point::ZERO, color, false);
            poly(c, &[(0.0, 2.0), (0.0, 7.5)], Point::ZERO, color, false);
            c.restore();
        }
        Icon::Cancel | Icon::Close => {
            let (a, b) = if icon == Icon::Close { (4.5, 11.5) } else { (3.5, 12.5) };
            poly(c, &[(a, a), (b, b)], o, color, false);
            poly(c, &[(b, a), (a, b)], o, color, false);
        }
        Icon::Save => {
            poly(c, &[(2.0, 9.5), (2.0, 14.0), (14.0, 14.0), (14.0, 9.5)], o, color, false);
            poly(c, &[(8.0, 1.5), (8.0, 10.0)], o, color, false);
            poly(c, &[(4.5, 6.5), (8.0, 10.0), (11.5, 6.5)], o, color, false);
        }
        Icon::Done => {
            if let Some(p) = polyline_path(&[at(2.5, 8.5), at(6.5, 12.5), at(14.0, 3.5)], false) {
                c.stroke_path(&p, color, &stroke(1.8));
            }
        }
        Icon::SizeDot(i) => {
            let d = [4.0, 7.0, 11.0][i.min(2) as usize];
            c.fill_oval(&Rect::new(center.x - d / 2.0, center.y - d / 2.0, d, d), color);
        }
        Icon::Dash(dash) => {
            let mut s = stroke(2.0);
            s.line_cap = LineCap::Butt;
            match dash {
                DashStyle::Solid => {}
                DashStyle::Dashed => s.dash = StrokeDash::new(vec![4.0, 2.5], 0.0),
                DashStyle::Dotted => {
                    s.line_cap = LineCap::Round;
                    s.dash = StrokeDash::new(vec![0.01, 3.5], 0.0);
                }
            }
            if let Some(p) = polyline_path(&[at(1.5, 8.0), at(14.5, 8.0)], false) {
                c.stroke_path(&p, color, &s);
            }
        }
        Icon::Rounded => c.stroke_rounded(&Rect::new(o.x + 2.0, o.y + 3.0, 12.0, 10.0), 4.0, color, 1.6),
        Icon::Arrow(head) => {
            let (a, b) = (at(2.5, 12.5), at(13.5, 3.5));
            if let Some(p) = polyline_path(&[a, b], false) {
                c.stroke_path(&p, color, &stroke(1.6));
            }
            let draw_head = |c: &mut Canvas, tip: Point, tail: Point, filled: bool| {
                let angle = (tip.y - tail.y).atan2(tip.x - tail.x);
                let p1 = Point::new(tip.x - 6.0 * (angle - 0.5).cos(), tip.y - 6.0 * (angle - 0.5).sin());
                let p2 = Point::new(tip.x - 6.0 * (angle + 0.5).cos(), tip.y - 6.0 * (angle + 0.5).sin());
                if let Some(p) = polyline_path(&[p1, tip, p2], filled) {
                    if filled { c.fill_path(&p, color) } else { c.stroke_path(&p, color, &stroke(1.6)) }
                }
            };
            draw_head(c, b, a, head != ArrowHead::Open);
            if head == ArrowHead::Double {
                draw_head(c, a, b, true);
            }
        }
        Icon::Text(decoration) => {
            let r = Rect::new(o.x + 0.5, o.y + 1.0, 15.0, 14.0);
            match decoration {
                TextDecoration::Plain => c.text_centered("A", &r, 12.0, Weight::Bold, color),
                TextDecoration::Background => {
                    c.fill_rounded(&r, 3.0, color);
                    c.text_centered("A", &r, 12.0, Weight::Bold, background);
                }
                TextDecoration::Outline => {
                    let l = crate::render::text::layout("A", 12.0, Weight::Bold, None);
                    let origin = Point::new(r.mid_x() - l.width / 2.0, r.mid_y() - l.height / 2.0);
                    if let Some(p) = l.path((origin.x, origin.y)) {
                        c.stroke_path(&p, color, &stroke(1.3));
                    }
                }
            }
        }
        Icon::MosaicMode(MosaicMode::Brush) => {
            // A paintbrush: handle and bristles.
            poly(c, &[(13.5, 2.5), (7.5, 8.5)], o, color, false);
            if let Some(p) = rounded_rect_path(&Rect::new(o.x + 3.0, o.y + 8.0, 5.0, 5.0), 2.0) {
                c.fill_path(&p, color);
            }
            poly(c, &[(3.0, 13.0), (1.5, 14.5)], o, color, false);
        }
        Icon::MosaicMode(MosaicMode::Rect) => c.dashed_rect(&Rect::new(o.x + 2.0, o.y + 3.0, 12.0, 10.0), color, 1.4, 2.5, 2.0),
        Icon::MosaicEffect(MosaicEffect::Pixelate) => {
            for i in 0..3 {
                for j in 0..3 {
                    c.fill_rect(&Rect::new(o.x + 2.5 + i as f32 * 4.0, o.y + 2.5 + j as f32 * 4.0, 3.2, 3.2), color);
                }
            }
        }
        Icon::MosaicEffect(MosaicEffect::Blur) => {
            // A drop.
            let mut pb = PathBuilder::new();
            pb.move_to(o.x + 8.0, o.y + 1.5);
            pb.cubic_to(o.x + 10.0, o.y + 5.0, o.x + 13.0, o.y + 7.5, o.x + 13.0, o.y + 10.0);
            pb.cubic_to(o.x + 13.0, o.y + 13.0, o.x + 10.5, o.y + 14.5, o.x + 8.0, o.y + 14.5);
            pb.cubic_to(o.x + 5.5, o.y + 14.5, o.x + 3.0, o.y + 13.0, o.x + 3.0, o.y + 10.0);
            pb.cubic_to(o.x + 3.0, o.y + 7.5, o.x + 6.0, o.y + 5.0, o.x + 8.0, o.y + 1.5);
            pb.close();
            if let Some(p) = pb.finish() {
                c.fill_path(&p, color);
            }
        }
    }
}

/// A color swatch: a filled circle with a faint edge.
pub fn swatch(c: &mut Canvas, center: Point, color: Color) {
    let r = Rect::new(center.x - 7.0, center.y - 7.0, 14.0, 14.0);
    c.fill_oval(&r, color.with_alpha(1.0));
    c.stroke_oval(&r, Color::gray(0.5, 0.5), 1.0);
}

/// The "custom color" swatch: a small color wheel.
pub fn rainbow_swatch(c: &mut Canvas, center: Point) {
    let colors = [
        Color::rgb(1.0, 0.23, 0.19),
        Color::rgb(1.0, 0.8, 0.0),
        Color::rgb(0.2, 0.78, 0.35),
        Color::rgb(0.0, 0.48, 1.0),
        Color::rgb(0.69, 0.32, 0.87),
    ];
    let radius = 7.0;
    let n = colors.len();
    for (i, color) in colors.iter().enumerate() {
        let a0 = i as f32 / n as f32 * std::f32::consts::TAU - std::f32::consts::FRAC_PI_2;
        let a1 = (i + 1) as f32 / n as f32 * std::f32::consts::TAU - std::f32::consts::FRAC_PI_2;
        let mut pb = PathBuilder::new();
        pb.move_to(center.x, center.y);
        let steps = 8;
        for s in 0..=steps {
            let a = a0 + (a1 - a0) * s as f32 / steps as f32;
            pb.line_to(center.x + radius * a.cos(), center.y + radius * a.sin());
        }
        pb.close();
        if let Some(p) = pb.finish() {
            c.fill_path(&p, *color);
        }
    }
}
