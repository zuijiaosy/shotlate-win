//! A small drawing context over a tiny-skia pixmap: a current transform (points → pixels),
//! a clip mask stack, and the shapes the UI needs (rounded rects, soft shadows, text, images).

use tiny_skia::{
    FillRule, FilterQuality, LineCap, LineJoin, Mask, Paint, Path, PathBuilder, Pixmap, PixmapMut, PixmapPaint, PixmapRef, Stroke,
    StrokeDash, Transform,
};

use super::text::{self, TextLayout, Weight};
use crate::kit::color::Color;
use crate::kit::geom::{Point, Rect};

pub struct Canvas<'a> {
    pix: PixmapMut<'a>,
    ts: Transform,
    clip: Option<Mask>,
    stack: Vec<(Transform, Option<Mask>)>,
}

pub fn paint(color: Color) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color(color.to_skia());
    p.anti_alias = true;
    p
}

pub fn skia_rect(r: &Rect) -> Option<tiny_skia::Rect> {
    tiny_skia::Rect::from_xywh(r.x, r.y, r.width, r.height)
}

pub fn rect_path(r: &Rect) -> Option<Path> {
    skia_rect(r).map(PathBuilder::from_rect)
}

pub fn oval_path(r: &Rect) -> Option<Path> {
    skia_rect(r).and_then(PathBuilder::from_oval)
}

/// Rounded rect with circular corners (cubic approximation).
pub fn rounded_rect_path(r: &Rect, radius: f32) -> Option<Path> {
    let radius = radius.min(r.width / 2.0).min(r.height / 2.0).max(0.0);
    if radius <= 0.01 {
        return rect_path(r);
    }
    let k = 0.552_284_8 * radius;
    let (x0, y0, x1, y1) = (r.min_x(), r.min_y(), r.max_x(), r.max_y());
    let mut pb = PathBuilder::new();
    pb.move_to(x0 + radius, y0);
    pb.line_to(x1 - radius, y0);
    pb.cubic_to(x1 - radius + k, y0, x1, y0 + radius - k, x1, y0 + radius);
    pb.line_to(x1, y1 - radius);
    pb.cubic_to(x1, y1 - radius + k, x1 - radius + k, y1, x1 - radius, y1);
    pb.line_to(x0 + radius, y1);
    pb.cubic_to(x0 + radius - k, y1, x0, y1 - radius + k, x0, y1 - radius);
    pb.line_to(x0, y0 + radius);
    pb.cubic_to(x0, y0 + radius - k, x0 + radius - k, y0, x0 + radius, y0);
    pb.close();
    pb.finish()
}

pub fn polyline_path(points: &[Point], close: bool) -> Option<Path> {
    let mut pb = PathBuilder::new();
    let first = points.first()?;
    pb.move_to(first.x, first.y);
    for p in &points[1..] {
        pb.line_to(p.x, p.y);
    }
    if close {
        pb.close();
    }
    pb.finish()
}

pub fn stroke(width: f32) -> Stroke {
    Stroke { width, line_cap: LineCap::Round, line_join: LineJoin::Round, ..Stroke::default() }
}

impl<'a> Canvas<'a> {
    pub fn new(pix: PixmapMut<'a>, ts: Transform) -> Canvas<'a> {
        Canvas { pix, ts, clip: None, stack: Vec::new() }
    }




    /// Scale from the current coordinates to device pixels (x axis).
    pub fn device_scale(&self) -> f32 {
        (self.ts.sx * self.ts.sx + self.ts.ky * self.ts.ky).sqrt()
    }

    pub fn save(&mut self) {
        self.stack.push((self.ts, self.clip.clone()));
    }

    pub fn restore(&mut self) {
        if let Some((ts, clip)) = self.stack.pop() {
            self.ts = ts;
            self.clip = clip;
        }
    }

    pub fn translate(&mut self, dx: f32, dy: f32) {
        self.ts = self.ts.pre_translate(dx, dy);
    }

    pub fn scale(&mut self, sx: f32, sy: f32) {
        self.ts = self.ts.pre_scale(sx, sy);
    }

    pub fn clip_path(&mut self, path: &Path) {
        let (w, h) = (self.pix.width(), self.pix.height());
        match &mut self.clip {
            Some(mask) => mask.intersect_path(path, FillRule::Winding, true, self.ts),
            None => {
                if let Some(mut mask) = Mask::new(w, h) {
                    mask.fill_path(path, FillRule::Winding, true, self.ts);
                    self.clip = Some(mask);
                }
            }
        }
    }

    pub fn clip_rect(&mut self, r: &Rect) {
        match rect_path(r) {
            Some(p) => self.clip_path(&p),
            // An empty rect clips everything away.
            None => {
                if let Some(mask) = Mask::new(self.pix.width(), self.pix.height()) {
                    self.clip = Some(mask);
                }
            }
        }
    }

    pub fn fill_path(&mut self, path: &Path, color: Color) {
        self.pix.fill_path(path, &paint(color), FillRule::Winding, self.ts, self.clip.as_ref());
    }

    pub fn fill_path_even_odd(&mut self, path: &Path, color: Color) {
        self.pix.fill_path(path, &paint(color), FillRule::EvenOdd, self.ts, self.clip.as_ref());
    }

    pub fn stroke_path(&mut self, path: &Path, color: Color, stroke: &Stroke) {
        self.pix.stroke_path(path, &paint(color), stroke, self.ts, self.clip.as_ref());
    }

    pub fn fill_rect(&mut self, r: &Rect, color: Color) {
        if let Some(p) = rect_path(r) {
            self.fill_path(&p, color);
        }
    }

    pub fn stroke_rect(&mut self, r: &Rect, color: Color, width: f32) {
        if let Some(p) = rect_path(r) {
            let s = Stroke { width, line_join: LineJoin::Miter, ..Stroke::default() };
            self.stroke_path(&p, color, &s);
        }
    }

    pub fn fill_rounded(&mut self, r: &Rect, radius: f32, color: Color) {
        if let Some(p) = rounded_rect_path(r, radius) {
            self.fill_path(&p, color);
        }
    }

    pub fn stroke_rounded(&mut self, r: &Rect, radius: f32, color: Color, width: f32) {
        if let Some(p) = rounded_rect_path(r, radius) {
            self.stroke_path(&p, color, &stroke(width));
        }
    }

    pub fn fill_oval(&mut self, r: &Rect, color: Color) {
        if let Some(p) = oval_path(r) {
            self.fill_path(&p, color);
        }
    }

    pub fn stroke_oval(&mut self, r: &Rect, color: Color, width: f32) {
        if let Some(p) = oval_path(r) {
            self.stroke_path(&p, color, &stroke(width));
        }
    }


    /// Dashed rect outline, `dash` and `gap` in current units.
    pub fn dashed_rect(&mut self, r: &Rect, color: Color, width: f32, dash: f32, gap: f32) {
        if let Some(p) = rect_path(r) {
            let s = Stroke { width, dash: StrokeDash::new(vec![dash, gap], 0.0), ..Stroke::default() };
            self.stroke_path(&p, color, &s);
        }
    }

    /// Draws `image` (whose pixels cover `dest` in current coordinates).
    pub fn draw_image(&mut self, image: PixmapRef, dest: &Rect, quality: FilterQuality, opacity: f32) {
        if image.width() == 0 || image.height() == 0 || dest.is_empty() {
            return;
        }
        let ts = self.ts.pre_translate(dest.x, dest.y).pre_scale(dest.width / image.width() as f32, dest.height / image.height() as f32);
        let paint = PixmapPaint { opacity, quality, ..PixmapPaint::default() };
        self.pix.draw_pixmap(0, 0, image, &paint, ts, self.clip.as_ref());
    }

    /// Fills `path` with `color`, blurred by `blur` points and offset: a drop shadow.
    /// The blur runs on a small offscreen alpha mask around the path, so it stays cheap.
    pub fn shadow(&mut self, path: &Path, offset: (f32, f32), blur: f32, color: Color) {
        let ts = self.ts.post_translate(0.0, 0.0).pre_translate(offset.0, offset.1);
        let Some(device) = path.clone().transform(ts) else { return };
        let b = device.bounds();
        let radius = (blur * self.device_scale()).max(0.5);
        let pad = (radius * 2.0).ceil();
        let x0 = (b.left() - pad).floor();
        let y0 = (b.top() - pad).floor();
        let w = (b.width() + pad * 2.0).ceil() as u32 + 1;
        let h = (b.height() + pad * 2.0).ceil() as u32 + 1;
        if w > 8192 || h > 8192 {
            return;
        }
        let Some(mut tmp) = Pixmap::new(w, h) else { return };
        tmp.fill_path(&device, &paint(color.with_alpha(1.0)), FillRule::Winding, Transform::from_translate(-x0, -y0), None);
        box_blur_alpha(&mut tmp, (radius / 2.0).round().max(1.0) as usize);
        let pp = PixmapPaint { opacity: color.a.clamp(0.0, 1.0), ..PixmapPaint::default() };
        self.pix.draw_pixmap(x0 as i32, y0 as i32, tmp.as_ref(), &pp, Transform::identity(), self.clip.as_ref());
    }

    pub fn text_layout(&mut self, layout: &TextLayout, origin: Point, color: Color) {
        if let Some(p) = layout.path((origin.x, origin.y)) {
            self.fill_path(&p, color);
        }
    }

    /// Outlined text: a stroke of `outline_color` under the fill.
    pub fn text_layout_outlined(&mut self, layout: &TextLayout, origin: Point, color: Color, outline_color: Color, outline_width: f32) {
        if let Some(p) = layout.path((origin.x, origin.y)) {
            self.stroke_path(&p, outline_color, &Stroke { width: outline_width, line_join: LineJoin::Round, ..Stroke::default() });
            self.fill_path(&p, color);
        }
    }

    /// One line (or wrapped block) of UI text with its top-left at `origin`; returns its size.
    pub fn text(&mut self, s: &str, origin: Point, size: f32, weight: Weight, color: Color) -> (f32, f32) {
        let l = text::layout(s, size, weight, None);
        self.text_layout(&l, origin, color);
        (l.width, l.height)
    }

    /// UI text centered in `r`.
    pub fn text_centered(&mut self, s: &str, r: &Rect, size: f32, weight: Weight, color: Color) {
        let l = text::layout(s, size, weight, None);
        let origin = Point::new(r.mid_x() - l.width / 2.0, r.mid_y() - l.height / 2.0);
        self.text_layout(&l, origin, color);
    }
}

/// Three box-blur passes per axis on premultiplied pixels (all four channels), approximating a Gaussian.
pub fn box_blur_alpha(pix: &mut Pixmap, radius: usize) {
    let (w, h) = (pix.width() as usize, pix.height() as usize);
    if radius == 0 || w == 0 || h == 0 {
        return;
    }
    let data = pix.data_mut();
    let mut tmp = vec![0u8; data.len()];
    for _ in 0..3 {
        blur_pass(data, &mut tmp, w, h, radius, true);
        blur_pass(&tmp.clone(), data, w, h, radius, false);
    }
}

/// One horizontal (`horizontal`) or vertical running-sum box blur from `src` into `dst`.
fn blur_pass(src: &[u8], dst: &mut [u8], w: usize, h: usize, r: usize, horizontal: bool) {
    let (outer, inner) = if horizontal { (h, w) } else { (w, h) };
    let idx = |o: usize, i: usize| if horizontal { (o * w + i) * 4 } else { (i * w + o) * 4 };
    let window = (2 * r + 1) as u32;
    for o in 0..outer {
        let mut sum = [0u32; 4];
        // Edge pixels extend outward.
        for k in 0..=(2 * r) {
            let i = k.saturating_sub(r).min(inner - 1);
            let p = idx(o, i);
            for c in 0..4 {
                sum[c] += src[p + c] as u32;
            }
        }
        for i in 0..inner {
            let p = idx(o, i);
            for c in 0..4 {
                dst[p + c] = (sum[c] / window) as u8;
            }
            let add = idx(o, (i + r + 1).min(inner - 1));
            let sub = idx(o, i.saturating_sub(r));
            for c in 0..4 {
                sum[c] = sum[c] + src[add + c] as u32 - src[sub + c] as u32;
            }
        }
    }
}

/// The screenshot as square blocks of their average color.
pub fn pixelate(src: &Pixmap, block: usize) -> Pixmap {
    let (w, h) = (src.width() as usize, src.height() as usize);
    let mut out = src.clone();
    let block = block.max(1);
    let s = src.data();
    let d = out.data_mut();
    let mut by = 0;
    while by < h {
        let mut bx = 0;
        while bx < w {
            let (x1, y1) = ((bx + block).min(w), (by + block).min(h));
            let mut sum = [0u64; 4];
            for y in by..y1 {
                for x in bx..x1 {
                    let p = (y * w + x) * 4;
                    for c in 0..4 {
                        sum[c] += s[p + c] as u64;
                    }
                }
            }
            let n = ((x1 - bx) * (y1 - by)) as u64;
            let avg: Vec<u8> = sum.iter().map(|v| (v / n) as u8).collect();
            for y in by..y1 {
                for x in bx..x1 {
                    let p = (y * w + x) * 4;
                    d[p..p + 4].copy_from_slice(&avg);
                }
            }
            bx += block;
        }
        by += block;
    }
    out
}

/// A blurred copy of the screenshot ("frosted glass" mosaic).
pub fn blurred(src: &Pixmap, radius: usize) -> Pixmap {
    let mut out = src.clone();
    box_blur_alpha(&mut out, (radius / 2).max(1));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pixelate_averages_blocks() {
        let mut p = Pixmap::new(4, 2).unwrap();
        p.fill(tiny_skia::Color::BLACK);
        p.data_mut()[0..4].copy_from_slice(&[255, 255, 255, 255]);
        let out = pixelate(&p, 2);
        // Block of 4 pixels, one white: average 63.
        assert_eq!(out.data()[0], 63);
        assert_eq!(out.data()[4], 63);
        assert_eq!(out.data()[8], 0);
    }

    #[test]
    fn shadow_draws_something() {
        let mut p = Pixmap::new(60, 60).unwrap();
        let mut c = Canvas::new(p.as_mut(), Transform::identity());
        let path = rounded_rect_path(&Rect::new(10.0, 10.0, 30.0, 30.0), 6.0).unwrap();
        c.shadow(&path, (0.0, 3.0), 6.0, Color::black(0.5));
        assert!(p.data().iter().skip(3).step_by(4).any(|a| *a > 0));
    }
}
