//! Annotation model and geometry, ported from the macOS version's Annotation.swift.
//! Coordinates are logical points in the capture view (top-left origin).

use serde::{Deserialize, Serialize};

use super::color::Color;
use super::geom::{Point, Rect, distance_to_segment};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Tool {
    Rectangle,
    Arrow,
    Pen,
    Mosaic,
    Magnifier,
    Text,
    Number,
}

impl Tool {
    pub const ALL: [Tool; 7] = [Tool::Rectangle, Tool::Arrow, Tool::Pen, Tool::Mosaic, Tool::Magnifier, Tool::Text, Tool::Number];

    pub fn id(self) -> &'static str {
        match self {
            Tool::Rectangle => "rectangle",
            Tool::Arrow => "arrow",
            Tool::Pen => "pen",
            Tool::Mosaic => "mosaic",
            Tool::Magnifier => "magnifier",
            Tool::Text => "text",
            Tool::Number => "number",
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Tool::Rectangle => "矩形",
            Tool::Arrow => "箭头",
            Tool::Pen => "画笔",
            Tool::Mosaic => "马赛克",
            Tool::Magnifier => "放大镜",
            Tool::Text => "文字",
            Tool::Number => "序号",
        }
    }

    /// Single-key shortcut out of the box, 1 to 7 in toolbar order.
    pub fn default_key(self) -> &'static str {
        match self {
            Tool::Rectangle => "1",
            Tool::Arrow => "2",
            Tool::Pen => "3",
            Tool::Mosaic => "4",
            Tool::Magnifier => "5",
            Tool::Text => "6",
            Tool::Number => "7",
        }
    }

    /// What "size" means differs per tool: stroke width, brush width, font size or badge diameter.
    pub fn size_range(self) -> (f32, f32) {
        match self {
            Tool::Mosaic => (6.0, 120.0),
            Tool::Magnifier => (1.0, 10.0),
            Tool::Text => (10.0, 120.0),
            Tool::Number => (14.0, 80.0),
            _ => (1.0, 40.0),
        }
    }

    pub fn size_presets(self) -> [f32; 3] {
        match self {
            Tool::Mosaic => [12.0, 24.0, 48.0],
            Tool::Magnifier => [2.0, 3.0, 5.0],
            Tool::Text => [14.0, 20.0, 32.0],
            Tool::Number => [20.0, 26.0, 36.0],
            _ => [2.0, 4.0, 8.0],
        }
    }

    pub fn default_size(self) -> f32 {
        self.size_presets()[1]
    }

    /// Freehand tools always draw, even when the stroke starts on an existing annotation.
    pub fn is_freehand(self) -> bool {
        matches!(self, Tool::Pen | Tool::Mosaic)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MosaicMode {
    #[default]
    Brush,
    Rect,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MosaicEffect {
    #[default]
    Pixelate,
    Blur,
}

/// Stroke pattern for rectangles, arrows and the pen.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DashStyle {
    #[default]
    Solid,
    Dashed,
    Dotted,
}

/// Arrow look: the tapered filled arrow, a plain line with an open head, or heads at both ends.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ArrowHead {
    #[default]
    Tapered,
    Open,
    Double,
}

/// How text stands out from what is under it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TextDecoration {
    #[default]
    Plain,
    Background,
    Outline,
}

/// The optional looks an annotation can have beyond color and size.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ItemStyle {
    pub dash: DashStyle,
    pub arrow_head: ArrowHead,
    pub rounded: bool,
    pub text: TextDecoration,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Shape {
    Rectangle(Rect),
    Arrow(Point, Point),
    Pen(Vec<Point>),
    MosaicRect(Rect),
    MosaicBrush(Vec<Point>),
    /// Text origin is the top-left of the first line; lines wrap at `width`.
    Text { text: String, origin: Point, width: f32 },
    Number(Point),
    /// A circle of `radius` around `source`, shown enlarged in a lens centered at `target`.
    Magnifier { source: Point, target: Point, radius: f32 },
}

impl Shape {
    pub fn is_mosaic_brush(&self) -> bool {
        matches!(self, Shape::MosaicBrush(_))
    }
}

/// Measures wrapped annotation text; supplied by the renderer, which owns the fonts.
pub trait TextMeasure {
    /// Size of `text` at font `size`, wrapped at `width` (an empty string measures as one line).
    fn text_size(&self, text: &str, size: f32, width: f32) -> (f32, f32);
}

#[derive(Clone, Debug, PartialEq)]
pub struct AnnotationItem {
    pub id: u64,
    pub shape: Shape,
    pub color: Color,
    pub size: f32,
    pub effect: MosaicEffect,
    pub style: ItemStyle,
}

/// Resize handles of a rectangle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResizeHandle {
    TopLeft,
    Top,
    TopRight,
    Right,
    BottomRight,
    Bottom,
    BottomLeft,
    Left,
}

impl ResizeHandle {
    pub const ALL: [ResizeHandle; 8] = [
        ResizeHandle::TopLeft,
        ResizeHandle::Top,
        ResizeHandle::TopRight,
        ResizeHandle::Right,
        ResizeHandle::BottomRight,
        ResizeHandle::Bottom,
        ResizeHandle::BottomLeft,
        ResizeHandle::Left,
    ];

    pub fn point(self, r: &Rect) -> Point {
        match self {
            ResizeHandle::TopLeft => Point::new(r.min_x(), r.min_y()),
            ResizeHandle::Top => Point::new(r.mid_x(), r.min_y()),
            ResizeHandle::TopRight => Point::new(r.max_x(), r.min_y()),
            ResizeHandle::Right => Point::new(r.max_x(), r.mid_y()),
            ResizeHandle::BottomRight => Point::new(r.max_x(), r.max_y()),
            ResizeHandle::Bottom => Point::new(r.mid_x(), r.max_y()),
            ResizeHandle::BottomLeft => Point::new(r.min_x(), r.max_y()),
            ResizeHandle::Left => Point::new(r.min_x(), r.mid_y()),
        }
    }

    /// Moves the edges this handle controls by `d`. Dragging past the opposite edge flips the rect.
    pub fn resize(self, r: &Rect, d: Point) -> Rect {
        use ResizeHandle::*;
        let (mut min_x, mut max_x, mut min_y, mut max_y) = (r.min_x(), r.max_x(), r.min_y(), r.max_y());
        if matches!(self, TopLeft | Left | BottomLeft) {
            min_x += d.x;
        }
        if matches!(self, TopRight | Right | BottomRight) {
            max_x += d.x;
        }
        if matches!(self, TopLeft | Top | TopRight) {
            min_y += d.y;
        }
        if matches!(self, BottomLeft | Bottom | BottomRight) {
            max_y += d.y;
        }
        Rect::from_corners(Point::new(min_x, min_y), Point::new(max_x, max_y))
    }

    pub fn cursor(self) -> Cursor {
        match self {
            ResizeHandle::Left | ResizeHandle::Right => Cursor::ResizeLeftRight,
            ResizeHandle::Top | ResizeHandle::Bottom => Cursor::ResizeUpDown,
            ResizeHandle::TopLeft | ResizeHandle::BottomRight => Cursor::ResizeDiagonalDown,
            ResizeHandle::TopRight | ResizeHandle::BottomLeft => Cursor::ResizeDiagonalUp,
        }
    }
}

/// Mouse cursors the capture view asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cursor {
    Arrow,
    Crosshair,
    IBeam,
    OpenHand,
    ClosedHand,
    ResizeLeftRight,
    ResizeUpDown,
    /// Top-left to bottom-right.
    ResizeDiagonalDown,
    /// Bottom-left to top-right.
    ResizeDiagonalUp,
}

/// A draggable control point of an annotation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ItemHandle {
    Rect(ResizeHandle),
    Start,
    End,
}

pub const MAGNIFICATION: f32 = 2.0;

pub fn text_padding(size: f32) -> f32 {
    (size * 0.35).max(4.0)
}

pub fn corner_radius(r: &Rect, size: f32) -> f32 {
    (8.0 + size * 2.0).min(r.width.min(r.height) / 2.0)
}

pub fn arrow_head_width(size: f32) -> f32 {
    size * 3.0 + 10.0
}

pub fn arrow_head_length(size: f32) -> f32 {
    size * 3.0 + 12.0
}

/// Where the lens goes for a new magnifier: beside the circle, inside `bounds` if possible.
pub fn lens_center(source: Point, radius: f32, bounds: &Rect) -> Point {
    let lens = radius * MAGNIFICATION;
    let gap = radius * 0.6;
    let mut x = source.x + radius + gap + lens;
    if x + lens > bounds.max_x() {
        x = source.x - radius - gap - lens;
    }
    let y = source.y.max(bounds.min_y() + lens).min(bounds.max_y() - lens);
    Point::new(x, y)
}

impl AnnotationItem {
    pub fn tool(&self) -> Tool {
        match self.shape {
            Shape::Rectangle(_) => Tool::Rectangle,
            Shape::Arrow(..) => Tool::Arrow,
            Shape::Pen(_) => Tool::Pen,
            Shape::MosaicRect(_) | Shape::MosaicBrush(_) => Tool::Mosaic,
            Shape::Text { .. } => Tool::Text,
            Shape::Number(_) => Tool::Number,
            Shape::Magnifier { .. } => Tool::Magnifier,
        }
    }

    /// Visual bounds, including stroke width.
    pub fn bounds(&self, measure: &dyn TextMeasure) -> Rect {
        let pad = self.size / 2.0 + 1.0;
        match &self.shape {
            Shape::Rectangle(r) => r.inset(-pad, -pad),
            Shape::MosaicRect(r) => *r,
            Shape::Arrow(a, b) => {
                let head = arrow_head_width(self.size) / 2.0;
                Rect::from_corners(*a, *b).inset(-head, -head)
            }
            Shape::Pen(points) | Shape::MosaicBrush(points) => {
                points.iter().fold(Rect::NULL, |r, p| r.union_point(*p)).inset(-pad, -pad)
            }
            Shape::Text { text, origin, width } => {
                let (w, h) = measure.text_size(text, self.size, *width);
                let r = Rect::new(origin.x, origin.y, w.ceil(), h.ceil());
                if self.style.text == TextDecoration::Background {
                    let p = text_padding(self.size);
                    r.inset(-p, -p / 2.0)
                } else {
                    r
                }
            }
            Shape::Number(c) => Rect::new(c.x - self.size / 2.0, c.y - self.size / 2.0, self.size, self.size),
            Shape::Magnifier { source, target, radius } => {
                let lens = radius * MAGNIFICATION;
                Rect::new(source.x - radius, source.y - radius, radius * 2.0, radius * 2.0)
                    .union(&Rect::new(target.x - lens, target.y - lens, lens * 2.0, lens * 2.0))
                    .inset(-pad, -pad)
            }
        }
    }

    /// Whether a click at `p` lands on this annotation. Outlined shapes are hit on their stroke only,
    /// so a new shape can still be drawn inside an existing rectangle.
    pub fn contains(&self, p: Point, measure: &dyn TextMeasure) -> bool {
        let tolerance = (self.size / 2.0 + 3.0).max(5.0);
        match &self.shape {
            Shape::Rectangle(r) => {
                let outer = r.inset(-tolerance, -tolerance);
                let inner = r.inset(tolerance, tolerance);
                outer.contains(p) && (inner.is_empty() || !inner.contains(p))
            }
            Shape::MosaicRect(r) => r.contains(p),
            Shape::Arrow(a, b) => distance_to_segment(p, *a, *b) <= tolerance.max(arrow_head_width(self.size) / 2.0),
            Shape::Pen(points) | Shape::MosaicBrush(points) => {
                if points.len() == 1 {
                    return p.distance(points[0]) <= tolerance;
                }
                points.windows(2).any(|w| distance_to_segment(p, w[0], w[1]) <= tolerance)
            }
            Shape::Text { .. } => self.bounds(measure).inset(-4.0, -4.0).contains(p),
            Shape::Number(c) => p.distance(*c) <= self.size / 2.0 + 3.0,
            Shape::Magnifier { source, target, radius } => {
                p.distance(*target) <= radius * MAGNIFICATION + tolerance || (p.distance(*source) - radius).abs() <= tolerance
            }
        }
    }

    pub fn handles(&self) -> Vec<(ItemHandle, Point)> {
        match &self.shape {
            Shape::Rectangle(r) | Shape::MosaicRect(r) => ResizeHandle::ALL.iter().map(|h| (ItemHandle::Rect(*h), h.point(r))).collect(),
            Shape::Arrow(a, b) => vec![(ItemHandle::Start, *a), (ItemHandle::End, *b)],
            Shape::Magnifier { source, target, .. } => vec![(ItemHandle::Start, *source), (ItemHandle::End, *target)],
            _ => vec![],
        }
    }

    pub fn moved(&self, d: Point) -> AnnotationItem {
        let mut copy = self.clone();
        let m = |p: &Point| Point::new(p.x + d.x, p.y + d.y);
        copy.shape = match &self.shape {
            Shape::Rectangle(r) => Shape::Rectangle(r.offset(d.x, d.y)),
            Shape::MosaicRect(r) => Shape::MosaicRect(r.offset(d.x, d.y)),
            Shape::Arrow(a, b) => Shape::Arrow(m(a), m(b)),
            Shape::Pen(points) => Shape::Pen(points.iter().map(m).collect()),
            Shape::MosaicBrush(points) => Shape::MosaicBrush(points.iter().map(m).collect()),
            Shape::Text { text, origin, width } => Shape::Text { text: text.clone(), origin: m(origin), width: *width },
            Shape::Number(c) => Shape::Number(m(c)),
            Shape::Magnifier { source, target, radius } => Shape::Magnifier { source: m(source), target: m(target), radius: *radius },
        };
        copy
    }

    pub fn resized(&self, handle: ItemHandle, d: Point) -> AnnotationItem {
        let mut copy = self.clone();
        match (&self.shape, handle) {
            (Shape::Rectangle(r), ItemHandle::Rect(h)) => copy.shape = Shape::Rectangle(h.resize(r, d)),
            (Shape::MosaicRect(r), ItemHandle::Rect(h)) => copy.shape = Shape::MosaicRect(h.resize(r, d)),
            (Shape::Arrow(a, b), ItemHandle::Start) => copy.shape = Shape::Arrow(*a + d, *b),
            (Shape::Arrow(a, b), ItemHandle::End) => copy.shape = Shape::Arrow(*a, *b + d),
            (Shape::Magnifier { source, target, radius }, ItemHandle::Start) => {
                copy.shape = Shape::Magnifier { source: *source + d, target: *target, radius: *radius }
            }
            (Shape::Magnifier { source, target, radius }, ItemHandle::End) => {
                copy.shape = Shape::Magnifier { source: *source, target: *target + d, radius: *radius }
            }
            _ => {}
        }
        copy
    }

    pub fn is_meaningful(&self) -> bool {
        match &self.shape {
            Shape::Rectangle(r) | Shape::MosaicRect(r) => r.width >= 3.0 && r.height >= 3.0,
            Shape::Arrow(a, b) => a.distance(*b) >= 3.0,
            Shape::Pen(points) => points.len() >= 2,
            Shape::MosaicBrush(points) => !points.is_empty(),
            Shape::Text { text, .. } => !text.is_empty(),
            Shape::Number(_) => true,
            Shape::Magnifier { radius, .. } => *radius >= 4.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub struct FixedMeasure;
    impl TextMeasure for FixedMeasure {
        fn text_size(&self, text: &str, size: f32, _width: f32) -> (f32, f32) {
            (text.chars().count().max(1) as f32 * size * 0.6, size * 1.3)
        }
    }

    fn item(shape: Shape, size: f32) -> AnnotationItem {
        AnnotationItem { id: 1, shape, color: Color::rgb(1.0, 0.0, 0.0), size, effect: MosaicEffect::Pixelate, style: ItemStyle::default() }
    }

    #[test]
    fn rectangle_hit_on_stroke_only() {
        let r = item(Shape::Rectangle(Rect::new(0.0, 0.0, 100.0, 100.0)), 4.0);
        assert!(r.contains(Point::new(1.0, 50.0), &FixedMeasure));
        assert!(!r.contains(Point::new(50.0, 50.0), &FixedMeasure));
        assert!(!r.contains(Point::new(200.0, 50.0), &FixedMeasure));
    }

    #[test]
    fn resize_past_opposite_edge_flips() {
        let r = ResizeHandle::Left.resize(&Rect::new(10.0, 10.0, 20.0, 20.0), Point::new(30.0, 0.0));
        assert_eq!(r, Rect::new(30.0, 10.0, 10.0, 20.0));
    }

    #[test]
    fn lens_goes_left_when_no_room_right() {
        let bounds = Rect::new(0.0, 0.0, 200.0, 200.0);
        let c = lens_center(Point::new(180.0, 100.0), 10.0, &bounds);
        assert!(c.x < 180.0);
        let c = lens_center(Point::new(20.0, 100.0), 10.0, &bounds);
        assert!(c.x > 20.0);
    }

    #[test]
    fn moved_and_meaningful() {
        let a = item(Shape::Arrow(Point::new(0.0, 0.0), Point::new(1.0, 1.0)), 4.0);
        assert!(!a.is_meaningful());
        let m = a.moved(Point::new(5.0, 5.0));
        assert_eq!(m.shape, Shape::Arrow(Point::new(5.0, 5.0), Point::new(6.0, 6.0)));
        assert!(item(Shape::Number(Point::ZERO), 20.0).is_meaningful());
    }

    #[test]
    fn background_text_bounds_are_padded() {
        let mut t = item(Shape::Text { text: "ab".into(), origin: Point::new(10.0, 10.0), width: 300.0 }, 20.0);
        let plain = t.bounds(&FixedMeasure);
        t.style.text = TextDecoration::Background;
        let padded = t.bounds(&FixedMeasure);
        assert!(padded.width > plain.width && padded.min_x() < plain.min_x());
    }
}
