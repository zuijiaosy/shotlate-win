//! Image preparation for the models: BGR conversion, OpenCV-compatible bilinear resize,
//! RapidOCR's size rules, and tensor packing.

use crate::kit::image::RgbaImage;

/// 8-bit BGR pixels (the channel order the Paddle models were trained with), top-left origin.
#[derive(Clone)]
pub struct Bgr {
    pub width: usize,
    pub height: usize,
    pub data: Vec<u8>,
}

impl Bgr {
    pub fn filled(width: usize, height: usize, bgr: [u8; 3]) -> Bgr {
        let mut data = Vec::with_capacity(width * height * 3);
        for _ in 0..width * height {
            data.extend_from_slice(&bgr);
        }
        Bgr { width, height, data }
    }

    /// Transparent pixels are composited over white, so pins with alpha read like on screen.
    pub fn from_rgba(img: &RgbaImage) -> Bgr {
        let (w, h) = (img.width as usize, img.height as usize);
        let n = (w * h).min(img.data.len() / 4);
        let mut data = Vec::with_capacity(w * h * 3);
        for p in img.data.chunks_exact(4).take(n) {
            let a = p[3] as u32;
            let over = |c: u8| ((c as u32 * a + 255 * (255 - a) + 127) / 255) as u8;
            data.extend_from_slice(&[over(p[2]), over(p[1]), over(p[0])]);
        }
        data.resize(w * h * 3, 255);
        Bgr { width: w, height: h, data }
    }

    /// `x, y, w, h` must lie inside the image (callers clamp).
    pub fn crop(&self, x: usize, y: usize, w: usize, h: usize) -> Bgr {
        let mut data = Vec::with_capacity(w * h * 3);
        for row in y..y + h {
            let start = (row * self.width + x) * 3;
            data.extend_from_slice(&self.data[start..start + w * 3]);
        }
        Bgr { width: w, height: h, data }
    }

    /// Like `np.rot90`: 90° counter-clockwise, for vertical text columns.
    pub fn rot90(&self) -> Bgr {
        let (w, h) = (self.width, self.height);
        let mut data = vec![0u8; w * h * 3];
        // out[i][j] = in[j][w - 1 - i]; the output is h wide and w tall.
        for i in 0..w {
            for j in 0..h {
                let src = (j * w + (w - 1 - i)) * 3;
                let dst = (i * h + j) * 3;
                data[dst..dst + 3].copy_from_slice(&self.data[src..src + 3]);
            }
        }
        Bgr { width: h, height: w, data }
    }

    /// Adds `top` and `bottom` rows of black, like RapidOCR's letterbox.
    pub fn pad_vertical(&self, top: usize, bottom: usize) -> Bgr {
        let row = self.width * 3;
        let mut data = vec![0u8; row * (self.height + top + bottom)];
        data[row * top..row * (top + self.height)].copy_from_slice(&self.data);
        Bgr { width: self.width, height: self.height + top + bottom, data }
    }

    /// Bilinear resize with OpenCV's `INTER_LINEAR` sampling (half-pixel centers, edge clamp),
    /// so crops look to the model as they do under RapidOCR.
    pub fn resize(&self, width: usize, height: usize) -> Bgr {
        if width == self.width && height == self.height {
            return self.clone();
        }
        let xs = axis_weights(self.width, width);
        let ys = axis_weights(self.height, height);
        let mut data = vec![0u8; width * height * 3];
        let stride = self.width * 3;
        for (dy, &(y0, y1, fy)) in ys.iter().enumerate() {
            let r0 = &self.data[y0 * stride..y0 * stride + stride];
            let r1 = &self.data[y1 * stride..y1 * stride + stride];
            let out = &mut data[dy * width * 3..(dy + 1) * width * 3];
            for (dx, &(x0, x1, fx)) in xs.iter().enumerate() {
                for c in 0..3 {
                    let top = r0[x0 * 3 + c] as f32 * (1.0 - fx) + r0[x1 * 3 + c] as f32 * fx;
                    let bottom = r1[x0 * 3 + c] as f32 * (1.0 - fx) + r1[x1 * 3 + c] as f32 * fx;
                    out[dx * 3 + c] = (top * (1.0 - fy) + bottom * fy).round().clamp(0.0, 255.0) as u8;
                }
            }
        }
        Bgr { width, height, data }
    }

    /// Writes `(v / 255 - 0.5) / 0.5` planes (B, G, R) into an NCHW buffer of `plane_w` columns,
    /// leaving columns past `self.width` untouched (zero padding for rec).
    pub fn write_normalized(&self, out: &mut [f32], plane_w: usize, plane_h: usize) {
        let plane = plane_w * plane_h;
        for y in 0..self.height.min(plane_h) {
            for x in 0..self.width.min(plane_w) {
                let p = (y * self.width + x) * 3;
                for c in 0..3 {
                    out[c * plane + y * plane_w + x] = self.data[p + c] as f32 / 127.5 - 1.0;
                }
            }
        }
    }
}

/// Per output index: the two source indices and the weight of the second.
fn axis_weights(src: usize, dst: usize) -> Vec<(usize, usize, f32)> {
    let scale = src as f64 / dst as f64;
    (0..dst)
        .map(|d| {
            let f = (d as f64 + 0.5) * scale - 0.5;
            let mut i = f.floor();
            let mut t = f - i;
            if i < 0.0 {
                i = 0.0;
                t = 0.0;
            }
            let i = i as usize;
            if i + 1 >= src {
                (src - 1, src - 1, 0.0)
            } else {
                (i, i + 1, t as f32)
            }
        })
        .collect()
}

/// `int(round(v / 32) * 32)` with Python's banker's rounding, never below 32.
pub fn round32(v: f64) -> usize {
    (((v / 32.0).round_ties_even() * 32.0) as usize).max(32)
}

/// Global preprocess (RapidOCR `resize_image_within_bounds`): caps the long side at 2000 and
/// lifts the short side to 30. Returns the working size; `None` means keep the original.
pub fn global_size(width: usize, height: usize) -> Option<(usize, usize)> {
    const MAX_SIDE: f64 = 2000.0;
    const MIN_SIDE: f64 = 30.0;
    let (mut w, mut h) = (width, height);
    let mut changed = false;
    if w.max(h) as f64 > MAX_SIDE {
        let ratio = MAX_SIDE / w.max(h) as f64;
        w = round32((w as f64 * ratio).trunc());
        h = round32((h as f64 * ratio).trunc());
        changed = true;
    }
    if (w.min(h) as f64) < MIN_SIDE {
        let ratio = MIN_SIDE / w.min(h).max(1) as f64;
        w = round32((w as f64 * ratio).trunc());
        h = round32((h as f64 * ratio).trunc());
        changed = true;
    }
    changed.then_some((w, h))
}

/// RapidOCR's vertical padding for strips: rows of black added above and below when the image is
/// very short or wider than 8:1, so detection does not have to upscale a thin strip enormously.
pub fn vertical_padding(width: usize, height: usize) -> usize {
    const MIN_HEIGHT: usize = 30;
    const RATIO: f64 = 8.0;
    if height <= MIN_HEIGHT || width as f64 / height as f64 > RATIO {
        let new_h = ((width as f64 / RATIO) as usize).max(MIN_HEIGHT) * 2;
        new_h.abs_diff(height) / 2
    } else {
        0
    }
}

/// Detector input size (`limit_type: min`, `limit_side_len: 736`, multiples of 32). The long side
/// is additionally capped so a tall thin strip cannot blow up memory; RapidOCR has no such cap.
pub fn det_size(width: usize, height: usize) -> (usize, usize) {
    const LIMIT: f64 = 736.0;
    const MAX_SIDE: f64 = 4000.0;
    let (w, h) = (width.max(1) as f64, height.max(1) as f64);
    let mut ratio = if w.min(h) < LIMIT { LIMIT / w.min(h) } else { 1.0 };
    if w.max(h) * ratio > MAX_SIDE {
        ratio = MAX_SIDE / w.max(h);
    }
    (round32((w * ratio).trunc()), round32((h * ratio).trunc()))
}

/// Recognizer input widths. Each line pads up to the smallest that fits, so a handful of
/// optimized plans serve almost every line; 320 is RapidOCR's minimum width.
pub const REC_BUCKETS: [usize; 9] = [320, 480, 640, 800, 960, 1280, 1600, 1920, 2560];
pub const REC_HEIGHT: usize = 48;
/// Past the buckets, widths round up to this step; past `REC_MAX_WIDTH` lines are squeezed.
pub const REC_WIDE_STEP: usize = 1280;
pub const REC_MAX_WIDTH: usize = 10240;

/// (resized width, bucket width) for a crop of `w`×`h`.
pub fn rec_widths(w: usize, h: usize) -> (usize, usize) {
    let needed = ((REC_HEIGHT as f64 * w as f64 / h.max(1) as f64).ceil() as usize).max(1);
    let bucket = match REC_BUCKETS.iter().find(|&&b| b >= needed) {
        Some(&b) => b,
        None => needed.div_ceil(REC_WIDE_STEP).saturating_mul(REC_WIDE_STEP).min(REC_MAX_WIDTH),
    };
    (needed.min(bucket), bucket)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn det_sizes_follow_rapidocr() {
        // Short side lifted to 736, both sides rounded to 32.
        assert_eq!(det_size(1400, 560), (1856, 736));
        assert_eq!(det_size(1650, 346), (3520, 736));
        // Large enough: unchanged apart from rounding.
        assert_eq!(det_size(1920, 1080), (1920, 1088));
        // Tall strip: long side capped.
        let (w, h) = det_size(20, 2000);
        assert!(h <= 4000 && w >= 32, "{w}x{h}");
        assert_eq!(det_size(1, 1), (736, 736));
    }

    #[test]
    fn global_sizes() {
        assert_eq!(global_size(1400, 560), None);
        assert_eq!(global_size(3840, 2160), Some((1984, 1120)));
        assert_eq!(global_size(1, 1), Some((32, 32)));
        assert_eq!(global_size(100, 10), Some((288, 32)));
        assert_eq!(round32(16.0), 32);
        assert_eq!(round32(48.0), 64);
        assert_eq!(round32(80.0), 64);
    }

    #[test]
    fn padding_for_strips() {
        assert_eq!(vertical_padding(1400, 560), 0);
        // 1000x60 is wider than 8:1: pad to 250 rows in total.
        assert_eq!(vertical_padding(1000, 60), 95);
        assert_eq!(vertical_padding(100, 20), 20);
    }

    #[test]
    fn rec_buckets() {
        assert_eq!(rec_widths(100, 48), (100, 320));
        assert_eq!(rec_widths(500, 48), (500, 640));
        assert_eq!(rec_widths(3000, 48), (3000, 3840));
        assert_eq!(rec_widths(100_000, 10), (10240, 10240));
        assert_eq!(rec_widths(10, 0), (480, 480));
    }

    #[test]
    fn resize_matches_opencv_on_simple_cases() {
        let mut img = Bgr::filled(2, 1, [0, 0, 0]);
        img.data[3..6].copy_from_slice(&[200, 200, 200]);
        // Upscaling 2 -> 4: OpenCV gives 0, 50, 150, 200.
        let up = img.resize(4, 1);
        let row: Vec<u8> = up.data.chunks(3).map(|p| p[0]).collect();
        assert_eq!(row, vec![0, 50, 150, 200]);
        let same = Bgr::filled(3, 3, [7, 8, 9]).resize(1, 5);
        assert!(same.data.chunks(3).all(|p| p == [7, 8, 9]));
    }

    #[test]
    fn rotates_counter_clockwise() {
        // 2 wide, 1 tall: [A, B] -> column [B; A].
        let img = Bgr { width: 2, height: 1, data: vec![1, 1, 1, 2, 2, 2] };
        let r = img.rot90();
        assert_eq!((r.width, r.height), (1, 2));
        assert_eq!(r.data, vec![2, 2, 2, 1, 1, 1]);
    }

    #[test]
    fn rgba_over_white_as_bgr() {
        let img = RgbaImage { width: 2, height: 1, data: vec![10, 20, 30, 255, 0, 0, 0, 0] };
        let bgr = Bgr::from_rgba(&img);
        assert_eq!(bgr.data, vec![30, 20, 10, 255, 255, 255]);
    }
}
