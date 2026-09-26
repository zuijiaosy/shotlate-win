//! DB (differentiable binarization) postprocess: probability map -> axis-aligned text boxes.
//! Mirrors RapidOCR's `DBPostProcess` but uses bounding boxes instead of `minAreaRect`,
//! since screen text is horizontal.

const THRESH: f32 = 0.3;
const BOX_THRESH: f32 = 0.5;
const MAX_CANDIDATES: usize = 1000;
const UNCLIP_RATIO: f32 = 1.6;
const MIN_SIZE: f32 = 3.0;

/// A box in destination pixels: `x0..x1`, `y0..y1` (exclusive ends, like a crop).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DetBox {
    pub x0: usize,
    pub y0: usize,
    pub x1: usize,
    pub y1: usize,
    pub score: f32,
}

/// Inclusive pixel bounds of one connected component.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Component {
    pub x0: usize,
    pub y0: usize,
    pub x1: usize,
    pub y1: usize,
}

/// Binarizes and dilates with OpenCV's 2x2 kernel (anchor at 1,1: a pixel is set when it or its
/// left, upper or upper-left neighbour is set).
pub fn binarize(pred: &[f32], w: usize, h: usize) -> Vec<u8> {
    let raw: Vec<bool> = pred.iter().map(|&p| p > THRESH).collect();
    let mut mask = vec![0u8; w * h];
    for y in 0..h {
        for x in 0..w {
            let at = |xx: usize, yy: usize| raw[yy * w + xx];
            let on = at(x, y) || (x > 0 && at(x - 1, y)) || (y > 0 && at(x, y - 1)) || (x > 0 && y > 0 && at(x - 1, y - 1));
            mask[y * w + x] = on as u8;
        }
    }
    mask
}

/// 8-connected components of `mask`, in raster order of their first pixel.
pub fn components(mask: &[u8], w: usize, h: usize, limit: usize) -> Vec<Component> {
    let mut seen = vec![false; w * h];
    let mut out = Vec::new();
    let mut stack = Vec::new();
    for start in 0..w * h {
        if mask[start] == 0 || seen[start] {
            continue;
        }
        if out.len() >= limit {
            break;
        }
        seen[start] = true;
        stack.push(start);
        let mut c = Component { x0: start % w, y0: start / w, x1: start % w, y1: start / w };
        while let Some(i) = stack.pop() {
            let (x, y) = (i % w, i / w);
            c.x0 = c.x0.min(x);
            c.x1 = c.x1.max(x);
            c.y0 = c.y0.min(y);
            c.y1 = c.y1.max(y);
            for ny in y.saturating_sub(1)..=(y + 1).min(h - 1) {
                for nx in x.saturating_sub(1)..=(x + 1).min(w - 1) {
                    let j = ny * w + nx;
                    if mask[j] != 0 && !seen[j] {
                        seen[j] = true;
                        stack.push(j);
                    }
                }
            }
        }
        out.push(c);
    }
    out
}

/// Boxes from a `w`×`h` probability map, scaled to a `dest_w`×`dest_h` image.
pub fn boxes_from_map(pred: &[f32], w: usize, h: usize, dest_w: usize, dest_h: usize) -> Vec<DetBox> {
    if w == 0 || h == 0 || pred.len() < w * h || dest_w == 0 || dest_h == 0 {
        return Vec::new();
    }
    let mask = binarize(pred, w, h);
    let mut boxes = Vec::new();
    for c in components(&mask, w, h, MAX_CANDIDATES) {
        // Contour points are pixel centers, so a one-pixel-wide component has zero width.
        let (bw, bh) = ((c.x1 - c.x0) as f32, (c.y1 - c.y0) as f32);
        if bw.min(bh) < MIN_SIZE {
            continue;
        }
        let score = box_score(pred, w, &c);
        if score < BOX_THRESH {
            continue;
        }
        // Unclip: offsetting a rectangle by d grows each side by d.
        let d = bw * bh * UNCLIP_RATIO / (2.0 * (bw + bh));
        if bw.min(bh) + 2.0 * d < MIN_SIZE + 2.0 {
            continue;
        }
        let sx = dest_w as f32 / w as f32;
        let sy = dest_h as f32 / h as f32;
        // Round, clip to the image, then clip to the last pixel like `clip_det_res`.
        let map = |v: f32, s: f32, dest: usize| -> usize { ((v * s).round().clamp(0.0, dest as f32) as usize).min(dest - 1) };
        let b = DetBox {
            x0: map(c.x0 as f32 - d, sx, dest_w),
            y0: map(c.y0 as f32 - d, sy, dest_h),
            x1: map(c.x1 as f32 + d, sx, dest_w),
            y1: map(c.y1 as f32 + d, sy, dest_h),
            score,
        };
        if b.x1 - b.x0 <= 3 || b.y1 - b.y0 <= 3 {
            continue;
        }
        boxes.push(b);
    }
    boxes
}

/// Mean probability over the component's bounding box (`box_score_fast` fills the whole box).
fn box_score(pred: &[f32], w: usize, c: &Component) -> f32 {
    let mut sum = 0.0f64;
    for y in c.y0..=c.y1 {
        sum += pred[y * w + c.x0..=y * w + c.x1].iter().map(|&p| p as f64).sum::<f64>();
    }
    let n = ((c.x1 - c.x0 + 1) * (c.y1 - c.y0 + 1)) as f64;
    (sum / n) as f32
}

/// Reading order like RapidOCR's `sorted_boxes`: stable sort by top; a top within `row_gap` of the
/// previous one continues the same row; rows are ordered left to right.
pub fn reading_order(tops_lefts: &[(f32, f32)], row_gap: f32) -> Vec<usize> {
    let mut by_y: Vec<usize> = (0..tops_lefts.len()).collect();
    by_y.sort_by(|&a, &b| tops_lefts[a].0.total_cmp(&tops_lefts[b].0));
    let mut row = 0usize;
    let mut keyed: Vec<(usize, f32, usize)> = Vec::with_capacity(by_y.len());
    for (k, &i) in by_y.iter().enumerate() {
        if k > 0 && tops_lefts[i].0 - tops_lefts[by_y[k - 1]].0 >= row_gap {
            row += 1;
        }
        keyed.push((row, tops_lefts[i].1, i));
    }
    keyed.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.total_cmp(&b.1)));
    keyed.into_iter().map(|(_, _, i)| i).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map_with(w: usize, h: usize, rects: &[(usize, usize, usize, usize, f32)]) -> Vec<f32> {
        let mut m = vec![0.0; w * h];
        for &(x0, y0, x1, y1, p) in rects {
            for y in y0..y1 {
                for x in x0..x1 {
                    m[y * w + x] = p;
                }
            }
        }
        m
    }

    #[test]
    fn dilation_grows_right_and_down() {
        let mut pred = vec![0.0; 9];
        pred[4] = 0.9;
        let mask = binarize(&pred, 3, 3);
        assert_eq!(mask, vec![0, 0, 0, 0, 1, 1, 0, 1, 1]);
    }

    #[test]
    fn eight_connected_components() {
        #[rustfmt::skip]
        let mask = vec![
            1, 0, 0, 0,
            0, 1, 0, 1,
            0, 0, 0, 1,
        ];
        let cs = components(&mask, 4, 3, 10);
        assert_eq!(cs, vec![Component { x0: 0, y0: 0, x1: 1, y1: 1 }, Component { x0: 3, y0: 1, x1: 3, y1: 2 }]);
        assert_eq!(components(&mask, 4, 3, 1).len(), 1);
    }

    #[test]
    fn boxes_are_unclipped_scored_and_filtered() {
        let (w, h) = (100, 40);
        // A strong line, a weak blob, and a speck too small to keep.
        let pred = map_with(w, h, &[(10, 10, 60, 20, 0.9), (70, 10, 90, 30, 0.35), (95, 2, 97, 4, 0.9)]);
        let boxes = boxes_from_map(&pred, w, h, 200, 80);
        assert_eq!(boxes.len(), 1, "{boxes:?}");
        let b = boxes[0];
        // Dilated component spans x 10..=60, y 10..=20 (inclusive); d = 50*10*1.6/120 = 6.67.
        assert_eq!((b.x0, b.y0, b.x1, b.y1), (7, 7, 133, 53));
        assert!(b.score > 0.8);
    }

    #[test]
    fn empty_and_degenerate_maps() {
        assert!(boxes_from_map(&[], 0, 0, 10, 10).is_empty());
        assert!(boxes_from_map(&[0.9], 1, 1, 10, 10).is_empty());
        assert!(boxes_from_map(&vec![0.9; 64], 8, 8, 0, 0).is_empty());
    }

    #[test]
    fn rows_then_columns() {
        // Two rows; the second box of row one sits 4 px lower but stays in the row.
        let items = [(50.0, 200.0), (10.0, 300.0), (14.0, 20.0), (52.0, 5.0)];
        assert_eq!(reading_order(&items, 10.0), vec![2, 1, 3, 0]);
        assert!(reading_order(&[], 10.0).is_empty());
    }
}
