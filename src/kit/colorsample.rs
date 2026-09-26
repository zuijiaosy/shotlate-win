//! Text and background colors of a text line in a screenshot, for drawing its translation in place
//! (ColorSampling.swift).

use super::color::Color;
use super::geom::Rect;
use super::image::RgbaImage;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb {
    pub const BLACK: Rgb = Rgb { r: 0, g: 0, b: 0 };
    pub const WHITE: Rgb = Rgb { r: 255, g: 255, b: 255 };

    pub fn luminance(self) -> f64 {
        (0.2126 * self.r as f64 + 0.7152 * self.g as f64 + 0.0722 * self.b as f64) / 255.0
    }

    fn distance(self, o: Rgb) -> i32 {
        (self.r as i32 - o.r as i32).abs() + (self.g as i32 - o.g as i32).abs() + (self.b as i32 - o.b as i32).abs()
    }

    pub fn color(self) -> Color {
        Color::from_rgb8(self.r, self.g, self.b)
    }
}

fn px(img: &RgbaImage, x: i64, y: i64) -> Rgb {
    let p = img.pixel(x as u32, y as u32);
    Rgb { r: p[0], g: p[1], b: p[2] }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SampledColors {
    pub background: Rgb,
    pub foreground: Rgb,
    /// Share of pixels inside the rect that belong to the text strokes.
    pub ink_coverage: f64,
    /// Average horizontal stroke thickness in pixels; relative to the line height it indicates weight.
    pub stroke_width: f64,
    /// Horizontal space to the edge of the background container, or None when it runs to the image edge.
    pub left_margin: Option<i64>,
    pub right_margin: Option<i64>,
}

impl SampledColors {
    /// Text that sits in the middle of its container (a button, a centered title) should stay centered.
    pub fn is_centered(&self) -> bool {
        match (self.left_margin, self.right_margin) {
            (Some(l), Some(r)) => (l - r).abs() <= 6.max((l + r) / 5),
            _ => false,
        }
    }
}

/// Integer pixel bounds (x0, y0, x1, y1) of `r` clamped to the image.
fn clamp(r: &Rect, img: &RgbaImage) -> (i64, i64, i64, i64) {
    let r = r.integral();
    let x0 = (r.min_x() as i64).clamp(0, img.width as i64);
    let y0 = (r.min_y() as i64).clamp(0, img.height as i64);
    let x1 = (r.max_x() as i64).clamp(0, img.width as i64);
    let y1 = (r.max_y() as i64).clamp(0, img.height as i64);
    (x0, y0, x1.max(x0), y1.max(y0))
}

/// Samples the text and background colors inside `rect` (pixel coordinates, top-left origin).
///
/// The background is the per-channel median of a ring of pixels just outside the rect, which is less
/// affected by anti-aliased glyph edges than an average. The foreground is the average of the pixels
/// inside the rect that differ most from it.
pub fn sample(img: &RgbaImage, rect: &Rect, ring: i64) -> SampledColors {
    let inner = clamp(rect, img);
    let outer = clamp(&rect.integral().inset(-(ring as f32), -(ring as f32)), img);
    let (ix0, iy0, ix1, iy1) = inner;
    if ix1 <= ix0 || iy1 <= iy0 {
        return SampledColors { background: Rgb::WHITE, foreground: Rgb::BLACK, ink_coverage: 0.0, stroke_width: 0.0, left_margin: None, right_margin: None };
    }
    let mut ring_pixels = Vec::new();
    for y in outer.1..outer.3 {
        for x in outer.0..outer.2 {
            if !(x >= ix0 && x < ix1 && y >= iy0 && y < iy1) {
                ring_pixels.push(px(img, x, y));
            }
        }
    }
    if ring_pixels.is_empty() {
        // The rect touches every edge of the image: fall back to its own border pixels.
        for x in ix0..ix1 {
            ring_pixels.push(px(img, x, iy0));
            ring_pixels.push(px(img, x, iy1 - 1));
        }
    }
    let background = median(&ring_pixels);

    let mut inside: Vec<(Rgb, i32)> = Vec::with_capacity(((ix1 - ix0) * (iy1 - iy0)) as usize);
    for y in iy0..iy1 {
        for x in ix0..ix1 {
            let p = px(img, x, y);
            inside.push((p, p.distance(background)));
        }
    }
    let stroke_width = average_ink_run(img, inner, background);
    let (left_margin, right_margin) = margins(img, inner, background);

    inside.sort_by(|a, b| b.1.cmp(&a.1));
    let ink_count = inside.iter().filter(|p| p.1 > 90).count();
    let top = &inside[..(inside.len() * 15 / 100).max(1)];
    let mut foreground = average(top.iter().map(|p| p.0));

    // Low contrast (blurry or empty rect): use black or white, whichever reads on the background.
    if (foreground.luminance() - background.luminance()).abs() < 0.3 {
        foreground = if background.luminance() > 0.5 { Rgb::BLACK } else { Rgb::WHITE };
    }
    SampledColors { background, foreground, ink_coverage: ink_count as f64 / inside.len() as f64, stroke_width, left_margin, right_margin }
}

/// Mean length of horizontal runs of "ink" pixels, i.e. the typical stem thickness.
fn average_ink_run(img: &RgbaImage, (x0, y0, x1, y1): (i64, i64, i64, i64), background: Rgb) -> f64 {
    let (mut runs, mut total) = (0, 0);
    for y in y0..y1 {
        let mut run = 0;
        for x in x0..x1 {
            if px(img, x, y).distance(background) > 150 {
                run += 1;
            } else if run > 0 {
                runs += 1;
                total += run;
                run = 0;
            }
        }
        if run > 0 {
            runs += 1;
            total += run;
        }
    }
    if runs == 0 { 0.0 } else { total as f64 / runs as f64 }
}

/// Scans left and right from the rect along its middle row until the background color ends.
fn margins(img: &RgbaImage, (x0, y0, x1, y1): (i64, i64, i64, i64), background: Rgb) -> (Option<i64>, Option<i64>) {
    let y = (y0 + y1) / 2;
    let scan = |start: i64, step: i64| {
        let mut x = start;
        let mut distance = 0;
        while x >= 0 && x < img.width as i64 {
            if px(img, x, y).distance(background) > 60 {
                return Some(distance);
            }
            x += step;
            distance += 1;
        }
        None
    };
    (scan(x0 - 1, -1), scan(x1, 1))
}

fn median(pixels: &[Rgb]) -> Rgb {
    if pixels.is_empty() {
        return Rgb::WHITE;
    }
    let mid = |mut v: Vec<u8>| {
        v.sort_unstable();
        v[v.len() / 2]
    };
    Rgb { r: mid(pixels.iter().map(|p| p.r).collect()), g: mid(pixels.iter().map(|p| p.g).collect()), b: mid(pixels.iter().map(|p| p.b).collect()) }
}

fn average(pixels: impl Iterator<Item = Rgb>) -> Rgb {
    let (mut r, mut g, mut b, mut n) = (0u64, 0u64, 0u64, 0u64);
    for p in pixels {
        r += p.r as u64;
        g += p.g as u64;
        b += p.b as u64;
        n += 1;
    }
    if n == 0 {
        return Rgb::BLACK;
    }
    Rgb { r: (r / n) as u8, g: (g / n) as u8, b: (b / n) as u8 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dark_text_on_light_background() {
        let mut img = RgbaImage::filled(40, 20, [250, 250, 250, 255]);
        // A "glyph": a dark bar inside the text rect.
        for y in 6..14 {
            for x in 12..16 {
                let i = ((y * 40 + x) * 4) as usize;
                img.data[i..i + 4].copy_from_slice(&[20, 20, 20, 255]);
            }
        }
        let s = sample(&img, &Rect::new(10.0, 5.0, 20.0, 10.0), 2);
        assert_eq!(s.background, Rgb { r: 250, g: 250, b: 250 });
        assert!(s.foreground.luminance() < 0.2);
        assert!((s.stroke_width - 4.0).abs() < 0.01);
    }

    #[test]
    fn centered_in_a_button() {
        let mut img = RgbaImage::filled(100, 20, [255, 255, 255, 255]);
        // Button background from x 20 to 80.
        for y in 0..20 {
            for x in 20..80 {
                let i = ((y * 100 + x) * 4) as usize;
                img.data[i..i + 4].copy_from_slice(&[0, 120, 215, 255]);
            }
        }
        let s = sample(&img, &Rect::new(35.0, 5.0, 30.0, 10.0), 2);
        assert!(s.is_centered());
    }
}
