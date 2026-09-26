//! Draws annotations and translations over the frozen screen. Shared by the on-screen overlay and the
//! exporter so the saved image matches what was on screen (ContentRenderer in the macOS version).

use std::cell::RefCell;
use std::collections::HashMap;

use tiny_skia::{FilterQuality, LineCap, LineJoin, Path, PathBuilder, Pixmap, Stroke, StrokeDash};

use super::canvas::{Canvas, oval_path, polyline_path, rect_path, rounded_rect_path, stroke};
use super::text::{self, Align, Weight};
use crate::kit::annotation::{
    AnnotationItem, ArrowHead, DashStyle, MAGNIFICATION, MosaicEffect, Shape, TextDecoration, TextMeasure, arrow_head_length, arrow_head_width,
    corner_radius,
};
use crate::kit::color::Color;
use crate::kit::geom::{Point, Rect};

/// One translated paragraph, laid out and ready to draw over the original text.
#[derive(Clone, Debug, PartialEq)]
pub struct TranslatedBlock {
    pub rect: Rect,
    pub text: String,
    pub font_size: f32,
    pub bold: bool,
    pub centered: bool,
    pub background: Color,
    pub foreground: Color,
}

/// Measures annotation text with the same fonts the renderer draws with.
pub struct FontMeasure;

impl TextMeasure for FontMeasure {
    fn text_size(&self, text: &str, size: f32, width: f32) -> (f32, f32) {
        text::measure(text, size, annotation_weight(), Some(width))
    }
}

/// Annotation text is medium weight on macOS; the bold face reads closer to it than the regular one.
pub fn annotation_weight() -> Weight {
    Weight::Bold
}

/// The frozen screenshot, its size in points, and lazily made mosaic versions of it.
pub struct Base {
    pub pixels: Pixmap,
    /// The screenshot's extent in points (origin 0,0).
    pub bounds: Rect,
    effects: RefCell<HashMap<MosaicEffect, Pixmap>>,
}

impl Base {
    pub fn new(pixels: Pixmap, bounds: Rect) -> Base {
        Base { pixels, bounds, effects: RefCell::new(HashMap::new()) }
    }

    /// Pixels per point.
    pub fn scale(&self) -> f32 {
        self.pixels.width() as f32 / self.bounds.width.max(1.0)
    }

    fn with_effect<R>(&self, effect: MosaicEffect, f: impl FnOnce(&Pixmap) -> R) -> R {
        let mut cache = self.effects.borrow_mut();
        let scale = self.scale();
        let pix = cache.entry(effect).or_insert_with(|| match effect {
            MosaicEffect::Pixelate => super::canvas::pixelate(&self.pixels, (9.0 * scale).max(8.0).round() as usize),
            MosaicEffect::Blur => super::canvas::blurred(&self.pixels, (9.0 * scale).round() as usize),
        });
        f(pix)
    }
}

pub struct ContentRenderer<'a> {
    pub base: &'a Base,
}

impl<'a> ContentRenderer<'a> {
    pub fn new(base: &'a Base) -> ContentRenderer<'a> {
        ContentRenderer { base }
    }

    pub fn draw_base(&self, c: &mut Canvas) {
        c.draw_image(self.base.pixels.as_ref(), &self.base.bounds, FilterQuality::Nearest, 1.0);
    }

    /// Base, then translations, then annotations (numbers counted in order).
    pub fn draw(&self, c: &mut Canvas, items: &[AnnotationItem], translation: &[TranslatedBlock]) {
        self.draw_base(c);
        self.draw_overlays(c, items, None, None, translation);
    }

    /// Translations go first so annotations stay on top of them.
    pub fn draw_overlays(&self, c: &mut Canvas, items: &[AnnotationItem], draft: Option<&AnnotationItem>, hidden: Option<u64>, translation: &[TranslatedBlock]) {
        for block in translation {
            draw_block(c, block);
        }
        let mut number = 0;
        for item in items {
            if matches!(item.shape, Shape::Number(_)) {
                number += 1;
            }
            if Some(item.id) != hidden {
                self.draw_item(c, item, number);
            }
        }
        if let Some(draft) = draft {
            if matches!(draft.shape, Shape::Number(_)) {
                number += 1;
            }
            self.draw_item(c, draft, number);
        }
    }

    pub fn draw_item(&self, c: &mut Canvas, item: &AnnotationItem, number: usize) {
        let color = item.color;
        let base_stroke = dashed(stroke(item.size), item.style.dash, item.size);
        match &item.shape {
            Shape::Rectangle(r) => {
                if item.style.rounded {
                    if let Some(p) = rounded_rect_path(r, corner_radius(r, item.size)) {
                        c.stroke_path(&p, color, &base_stroke);
                    }
                } else if let Some(p) = rect_path(r) {
                    let s = Stroke { line_join: LineJoin::Miter, ..base_stroke };
                    c.stroke_path(&p, color, &s);
                }
            }
            Shape::Arrow(a, b) => {
                if item.style.arrow_head == ArrowHead::Tapered && item.style.dash == DashStyle::Solid {
                    if let Some(p) = arrow_path(*a, *b, item.size) {
                        c.fill_path(&p, color);
                    }
                } else {
                    draw_arrow(c, &[*a, *b], item.style.arrow_head, item.size, color, &base_stroke);
                }
            }
            Shape::Pen(points) => {
                if let Some(p) = smooth_path(points) {
                    c.stroke_path(&p, color, &base_stroke);
                }
            }
            Shape::MosaicRect(r) => {
                c.save();
                c.clip_rect(r);
                self.draw_effect(c, item.effect);
                c.restore();
            }
            Shape::MosaicBrush(points) => {
                let pts = if points.len() == 1 { vec![points[0], points[0].offset(0.01, 0.0)] } else { points.clone() };
                if let Some(outline) = smooth_path(&pts).and_then(|p| p.stroke(&stroke(item.size), c.device_scale())) {
                    c.save();
                    c.clip_path(&outline);
                    self.draw_effect(c, item.effect);
                    c.restore();
                }
            }
            Shape::Text { text: s, origin, width } => {
                let layout = text::layout(s, item.size, annotation_weight(), Some(*width));
                match item.style.text {
                    TextDecoration::Plain => c.text_layout(&layout, *origin, color),
                    TextDecoration::Background => {
                        // A pill in the chosen color, with black or white text on it.
                        let box_ = item.bounds(&FontMeasure);
                        c.fill_rounded(&box_, (box_.height / 2.0).min(6.0), color);
                        c.text_layout(&layout, *origin, color.contrasting_text());
                    }
                    TextDecoration::Outline => {
                        let w = 2.0 * item.size * (30.0 / item.size).max(2.5) / 100.0;
                        c.text_layout_outlined(&layout, *origin, color, color.contrasting_text(), w * 2.0);
                    }
                }
            }
            Shape::Magnifier { source, target, radius } => {
                let lens = radius * MAGNIFICATION;
                let (dx, dy) = (target.x - source.x, target.y - source.y);
                let distance = dx.hypot(dy).max(0.001);
                let s = stroke(item.size);
                if distance > radius + lens {
                    let a = Point::new(source.x + dx / distance * radius, source.y + dy / distance * radius);
                    let b = Point::new(target.x - dx / distance * lens, target.y - dy / distance * lens);
                    if let Some(p) = polyline_path(&[a, b], false) {
                        c.stroke_path(&p, color, &s);
                    }
                }
                if let Some(p) = oval_path(&Rect::new(source.x - radius, source.y - radius, radius * 2.0, radius * 2.0)) {
                    c.stroke_path(&p, color, &s);
                }
                let lens_rect = Rect::new(target.x - lens, target.y - lens, lens * 2.0, lens * 2.0);
                if let Some(lens_path) = oval_path(&lens_rect) {
                    c.shadow(&lens_path, (0.0, 2.0), 6.0, Color::black(0.35));
                    c.save();
                    c.clip_path(&lens_path);
                    // Map the lens back onto the source circle, enlarged.
                    c.translate(target.x, target.y);
                    c.scale(MAGNIFICATION, MAGNIFICATION);
                    c.translate(-source.x, -source.y);
                    self.draw_base(c);
                    c.restore();
                    c.stroke_path(&lens_path, color, &s);
                }
            }
            Shape::Number(center) => {
                let d = item.size;
                let circle = Rect::new(center.x - d / 2.0, center.y - d / 2.0, d, d);
                if let Some(p) = oval_path(&circle) {
                    c.shadow(&p, (0.0, 1.0), 3.0, Color::black(0.3));
                    c.fill_path(&p, color);
                }
                c.stroke_oval(&circle.inset(0.75, 0.75), Color::white(0.9), (d / 16.0).max(1.5));
                let label = number.to_string();
                let size = d * if number >= 10 { 0.46 } else { 0.56 };
                let layout = text::layout(&label, size, Weight::Bold, None);
                let (ascent, _) = text::metrics(size, Weight::Bold);
                // Center the digits' cap height rather than the line box, which has room for descenders.
                let cap = ascent * 0.72;
                let origin = Point::new(center.x - layout.width / 2.0, center.y - ascent + cap / 2.0);
                c.text_layout(&layout, origin, color.contrasting_text());
            }
        }
    }

    fn draw_effect(&self, c: &mut Canvas, effect: MosaicEffect) {
        let bounds = self.base.bounds;
        self.base.with_effect(effect, |pix| c.draw_image(pix.as_ref(), &bounds, FilterQuality::Nearest, 1.0));
    }
}

fn dashed(mut s: Stroke, dash: DashStyle, size: f32) -> Stroke {
    match dash {
        DashStyle::Solid => {}
        DashStyle::Dashed => s.dash = StrokeDash::new(vec![(size * 3.0).max(4.0), (size * 2.0).max(3.0)], 0.0),
        DashStyle::Dotted => {
            // Zero-length dashes with round caps are dots.
            s.line_cap = LineCap::Round;
            s.dash = StrokeDash::new(vec![0.01, (size * 2.0).max(3.0)], 0.0);
        }
    }
    s
}

pub fn draw_block(c: &mut Canvas, block: &TranslatedBlock) {
    c.fill_rounded(&block.rect.inset(-2.0, -1.5), 2.0, block.background);
    let weight = if block.bold { Weight::Bold } else { Weight::Regular };
    let layout = text::layout(&block.text, block.font_size, weight, Some(block.rect.width))
        .with_align(if block.centered { Align::Center } else { Align::Left });
    let mut y = block.rect.y;
    if layout.height < block.rect.height {
        y += (block.rect.height - layout.height) / 2.0;
    }
    c.text_layout(&layout, Point::new(block.rect.x, y), block.foreground);
}

/// Quadratic curves through the midpoints of consecutive samples: smooth, but passes near every sample.
pub fn smooth_path(points: &[Point]) -> Option<Path> {
    let first = points.first()?;
    let mut pb = PathBuilder::new();
    pb.move_to(first.x, first.y);
    if points.len() == 1 {
        pb.line_to(first.x + 0.01, first.y);
        return pb.finish();
    }
    if points.len() <= 2 {
        for p in &points[1..] {
            pb.line_to(p.x, p.y);
        }
        return pb.finish();
    }
    for i in 1..points.len() - 1 {
        let mid = Point::new((points[i].x + points[i + 1].x) / 2.0, (points[i].y + points[i + 1].y) / 2.0);
        pb.quad_to(points[i].x, points[i].y, mid.x, mid.y);
    }
    let last = points[points.len() - 1];
    pb.line_to(last.x, last.y);
    pb.finish()
}

/// A stroked shaft with filled heads at the end (or both ends for `Double`), or open V heads for `Open`.
fn draw_arrow(c: &mut Canvas, points: &[Point], head: ArrowHead, size: f32, color: Color, shaft_stroke: &Stroke) {
    let Some(&tip) = points.last() else { return };
    let Some(&before_tip) = points[..points.len() - 1].iter().rev().find(|p| p.distance(tip) > 1.0) else { return };
    let trim = |end: Point, previous: Point| {
        // Stop the stroke inside the head, so the round cap doesn't poke out of the tip.
        let length = end.distance(previous);
        let h = arrow_head_length(size).min(length * 0.6) * 0.8;
        Point::new(end.x - (end.x - previous.x) / length * h, end.y - (end.y - previous.y) / length * h)
    };
    let mut shaft = points.to_vec();
    let n = shaft.len();
    if head != ArrowHead::Open {
        shaft[n - 1] = trim(tip, before_tip);
    }
    let start = points[0];
    let after_start = points[1..].iter().find(|p| p.distance(start) > 1.0).copied();
    if head == ArrowHead::Double {
        if let Some(a) = after_start {
            shaft[0] = trim(start, a);
        }
    }
    if let Some(p) = polyline_path(&shaft, false) {
        c.stroke_path(&p, color, shaft_stroke);
    }
    match head {
        ArrowHead::Tapered => c.fill_path(&arrow_head_path(before_tip, tip, size), color),
        ArrowHead::Double => {
            c.fill_path(&arrow_head_path(before_tip, tip, size), color);
            if let Some(a) = after_start {
                c.fill_path(&arrow_head_path(a, start, size), color);
            }
        }
        ArrowHead::Open => {
            let (dx, dy) = (tip.x - before_tip.x, tip.y - before_tip.y);
            let length = dx.hypot(dy).max(0.001);
            let (ux, uy) = (dx / length, dy / length);
            let arm = arrow_head_length(size).min(length * 0.6);
            let spread = arm * 0.6;
            let base = Point::new(tip.x - ux * arm, tip.y - uy * arm);
            let pts = [Point::new(base.x - uy * spread, base.y + ux * spread), tip, Point::new(base.x + uy * spread, base.y - ux * spread)];
            if let Some(p) = polyline_path(&pts, false) {
                c.stroke_path(&p, color, &stroke(size));
            }
        }
    }
}

/// A plain triangular head at `tip`, pointing away from `tail`.
fn arrow_head_path(tail: Point, tip: Point, size: f32) -> Path {
    let (dx, dy) = (tip.x - tail.x, tip.y - tail.y);
    let length = dx.hypot(dy).max(0.001);
    let (ux, uy) = (dx / length, dy / length);
    let head_length = arrow_head_length(size).min(length * 0.6);
    let head_half = (arrow_head_width(size) / 2.0).min(head_length * 0.7);
    let base = Point::new(tip.x - ux * head_length, tip.y - uy * head_length);
    let pts = [tip, Point::new(base.x - uy * head_half, base.y + ux * head_half), Point::new(base.x + uy * head_half, base.y - ux * head_half)];
    polyline_path(&pts, true).unwrap_or_else(|| PathBuilder::from_rect(tiny_skia::Rect::from_xywh(tip.x, tip.y, 0.1, 0.1).unwrap()))
}

/// A filled arrow whose shaft widens from the tail towards the head, like WeChat's and QQ's.
pub fn arrow_path(tail: Point, tip: Point, size: f32) -> Option<Path> {
    let (dx, dy) = (tip.x - tail.x, tip.y - tail.y);
    let length = dx.hypot(dy);
    if length <= 1.0 {
        return None;
    }
    let (ux, uy) = (dx / length, dy / length);
    let (nx, ny) = (-uy, ux);
    let head_length = arrow_head_length(size).min(length * 0.6);
    let head_half = (arrow_head_width(size) / 2.0).min(head_length * 0.7);
    let tail_half = (size * 0.15).max(0.5);
    let neck_half = (size * 0.6).max(1.5).min(head_half * 0.6);
    let base = Point::new(tip.x - ux * head_length, tip.y - uy * head_length);
    let p = |o: Point, half: f32| Point::new(o.x + nx * half, o.y + ny * half);
    polyline_path(&[p(tail, tail_half), p(base, neck_half), p(base, head_half), tip, p(base, -head_half), p(base, -neck_half), p(tail, -tail_half)], true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::annotation::{ItemStyle, MosaicEffect};
    use tiny_skia::Transform;

    fn item(shape: Shape, size: f32) -> AnnotationItem {
        AnnotationItem { id: 1, shape, color: Color::rgb(1.0, 0.0, 0.0), size, effect: MosaicEffect::Pixelate, style: ItemStyle::default() }
    }

    #[test]
    fn draws_every_shape_without_panicking() {
        let mut shot = Pixmap::new(200, 200).unwrap();
        shot.fill(tiny_skia::Color::from_rgba8(240, 240, 240, 255));
        let base = Base::new(shot, Rect::new(0.0, 0.0, 100.0, 100.0));
        let r = ContentRenderer::new(&base);
        let mut out = Pixmap::new(200, 200).unwrap();
        let mut c = Canvas::new(out.as_mut(), Transform::from_scale(2.0, 2.0));
        let items = vec![
            item(Shape::Rectangle(Rect::new(10.0, 10.0, 30.0, 20.0)), 4.0),
            item(Shape::Arrow(Point::new(10.0, 50.0), Point::new(60.0, 60.0)), 4.0),
            item(Shape::Pen(vec![Point::new(5.0, 5.0), Point::new(20.0, 30.0), Point::new(40.0, 10.0)]), 4.0),
            item(Shape::MosaicRect(Rect::new(50.0, 10.0, 30.0, 30.0)), 12.0),
            item(Shape::MosaicBrush(vec![Point::new(70.0, 70.0)]), 12.0),
            item(Shape::Text { text: "Hi 你好".into(), origin: Point::new(10.0, 70.0), width: 80.0 }, 14.0),
            item(Shape::Number(Point::new(80.0, 80.0)), 20.0),
            item(Shape::Magnifier { source: Point::new(30.0, 30.0), target: Point::new(60.0, 40.0), radius: 8.0 }, 2.0),
        ];
        r.draw(&mut c, &items, &[]);
        // Red appears somewhere.
        assert!(out.data().chunks(4).any(|p| p[0] > 200 && p[1] < 80));
    }
}
