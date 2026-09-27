//! Offline text recognition with PaddleOCR models (PP-OCRv6) run by tract. See AGENTS.md.
//!
//! The pipeline follows RapidOCR 3.9.2 defaults, simplified for axis-aligned screen text:
//! global resize -> DB text detection -> crop each box -> CTC recognition. tract optimizes a
//! model for one concrete input shape at a time (symbolic shapes fail for det and run ~3x slower
//! for rec), so the parsed graphs are kept and optimized plans are cached per shape.

// Wired into the capture UI separately; until then most of the API has no caller.
#![allow(dead_code)]

mod decode;
mod detect;
pub mod models;
mod prep;

use std::collections::HashMap;
use std::fmt;
use std::path::Path;
use std::sync::{Arc, Mutex};

use tract_onnx::prelude::*;

use crate::kit::geom::Rect;
use crate::kit::image::RgbaImage;
use prep::{Bgr, REC_BUCKETS, REC_HEIGHT};

type Plan = Arc<TypedRunnableModel>;
type CropResult = Result<Option<decode::Decoded>, OcrError>;

/// Detector plans kept; each screenshot size needs its own, so this is a small LRU.
const DET_PLAN_CACHE: usize = 4;
/// Plans for widths past the fixed buckets, kept alongside them.
const REC_EXTRA_PLANS: usize = 3;
/// RapidOCR's `text_score`.
const TEXT_SCORE: f32 = 0.5;
/// Boxes whose tops differ by less than this (original pixels) share a row.
const ROW_GAP: f32 = 10.0;
/// Detector size prepared by `warm_up`: a full 1080p screen (short side kept, rounded to 32).
const WARM_DET: (usize, usize) = (1920, 1088);
/// Recognition threads; lines are short jobs, more threads mostly add contention.
const MAX_REC_THREADS: usize = 4;

#[derive(Clone, Debug, PartialEq)]
pub struct RecognizedLine {
    pub text: String,
    /// Pixels of the input image, axis-aligned.
    pub rect: Rect,
    pub score: f32,
    /// Per `char` of `text`, its x-range in image pixels, left to right. Estimated from CTC time
    /// steps, so good enough for selecting text, not for exact glyph bounds. Vertical (rotated)
    /// lines give every char the full line width.
    pub char_boxes: Vec<(f32, f32)>,
}

#[derive(Debug)]
pub enum OcrError {
    /// Model files are missing or incomplete; the UI offers to download them.
    ModelsMissing,
    ModelCorrupt(String),
    Inference(String),
}

impl fmt::Display for OcrError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OcrError::ModelsMissing => write!(f, "识别组件未下载"),
            OcrError::ModelCorrupt(detail) => write!(f, "识别模型损坏：{detail}"),
            OcrError::Inference(detail) => write!(f, "文字识别失败：{detail}"),
        }
    }
}

impl std::error::Error for OcrError {}

fn inference(e: impl fmt::Display) -> OcrError {
    OcrError::Inference(e.to_string())
}

pub struct OcrEngine {
    det: InferenceModel,
    rec: InferenceModel,
    /// `["blank"] + dictionary + [" "]`, indexed by the recognizer's class.
    classes: Vec<String>,
    /// Most recently used last.
    det_plans: Mutex<Vec<((usize, usize), Plan)>>,
    /// Keyed by padded input width.
    rec_plans: Mutex<HashMap<usize, Plan>>,
}

impl OcrEngine {
    /// Parses both models (about half a second); plans are built lazily or by `warm_up`.
    pub fn load(model_dir: &Path) -> Result<OcrEngine, OcrError> {
        if !models::is_ready(model_dir) {
            return Err(OcrError::ModelsMissing);
        }
        // The models carry DynamicDimension annotations tract cannot analyse; the real shapes
        // come from the input facts set per plan.
        let onnx = tract_onnx::onnx().with_ignore_value_info(true).with_ignore_output_shapes(true);
        let corrupt = |name: &str, e: TractError| OcrError::ModelCorrupt(format!("{name}（{e}）"));

        let det = onnx.model_for_path(model_dir.join(models::DET_MODEL)).map_err(|e| corrupt(models::DET_MODEL, e))?;
        let rec_proto = onnx.proto_model_for_path(model_dir.join(models::REC_MODEL)).map_err(|e| corrupt(models::REC_MODEL, e))?;
        let dict = rec_proto
            .metadata_props
            .iter()
            .find(|p| p.key == "character")
            .map(|p| p.value.clone())
            .ok_or_else(|| OcrError::ModelCorrupt(format!("{} 缺少字符表", models::REC_MODEL)))?;
        let rec = onnx.model_for_proto_model(&rec_proto).map_err(|e| corrupt(models::REC_MODEL, e))?;
        drop(rec_proto);

        let mut classes = Vec::with_capacity(dict.len() / 3 + 2);
        classes.push("blank".to_string());
        classes.extend(dict.split('\n').map(|s| s.trim_end_matches('\r').to_string()));
        classes.push(" ".to_string());
        Ok(OcrEngine { det, rec, classes, det_plans: Mutex::new(Vec::new()), rec_plans: Mutex::new(HashMap::new()) })
    }

    /// Builds the plans most recognitions need (detector for a full 1080p screen, recognizer
    /// widths 320/640/960) and runs each once. Meant for a background thread at startup.
    pub fn warm_up(&self) {
        let (w, h) = WARM_DET;
        if let (Ok(plan), Ok(input)) = (self.det_plan(w, h), Tensor::zero::<f32>(&[1, 3, h, w])) {
            let _ = run(&plan, input);
        }
        for width in [320, 640, 960] {
            if let (Ok(plan), Ok(input)) = (self.rec_plan(width), Tensor::zero::<f32>(&[1, 3, REC_HEIGHT, width])) {
                let _ = run(&plan, input);
            }
        }
    }

    /// Builds (without running) the detector plan for a `w`×`h` input; for measurements.
    pub fn prepare_det(&self, w: usize, h: usize) -> Result<(), OcrError> {
        self.det_plan(w, h).map(|_| ())
    }

    /// Builds (without running) the recognizer plan for input width `width`; for measurements.
    pub fn prepare_rec(&self, width: usize) -> Result<(), OcrError> {
        self.rec_plan(width).map(|_| ())
    }

    /// Lines in reading order (top to bottom, then left to right), in pixels of `image`.
    pub fn recognize(&self, image: &RgbaImage) -> Result<Vec<RecognizedLine>, OcrError> {
        let (ori_w, ori_h) = (image.width as usize, image.height as usize);
        if ori_w == 0 || ori_h == 0 || image.data.len() < ori_w * ori_h * 4 {
            return Ok(Vec::new());
        }
        let mut img = Bgr::from_rgba(image);
        let (mut ratio_w, mut ratio_h) = (1.0f32, 1.0f32);
        if let Some((w, h)) = prep::global_size(ori_w, ori_h) {
            img = img.resize(w, h);
            ratio_w = ori_w as f32 / w as f32;
            ratio_h = ori_h as f32 / h as f32;
        }
        let pad = prep::vertical_padding(img.width, img.height);
        if pad > 0 {
            img = img.pad_vertical(pad, pad);
        }

        let boxes = self.detect(&img)?;
        // Crops are cut up front so recognition can fan out over threads.
        let crops: Vec<(Bgr, bool)> = boxes
            .iter()
            .map(|b| {
                let crop = img.crop(b.x0, b.y0, b.x1 - b.x0, b.y1 - b.y0);
                let vertical = crop.height as f32 / crop.width as f32 >= 1.5;
                (if vertical { crop.rot90() } else { crop }, vertical)
            })
            .collect();
        let decoded = self.recognize_crops(&crops)?;

        let mut lines = Vec::with_capacity(boxes.len());
        for ((b, (_, vertical)), decoded) in boxes.iter().zip(&crops).zip(decoded) {
            let Some(decoded) = decoded else { continue };
            if decoded.score < TEXT_SCORE || decoded.text.trim().is_empty() {
                continue;
            }
            // Back to the original image: undo padding and the global resize, then clamp.
            let to_x = |x: f32| (x * ratio_w).clamp(0.0, ori_w as f32);
            let to_y = |y: f32| ((y - pad as f32) * ratio_h).clamp(0.0, ori_h as f32);
            let (x0, x1) = (to_x(b.x0 as f32), to_x(b.x1 as f32));
            let (y0, y1) = (to_y(b.y0 as f32), to_y(b.y1 as f32));
            let char_boxes = if *vertical {
                vec![(x0, x1); decoded.text.chars().count()]
            } else {
                decoded.char_ranges.iter().map(|&(a, e)| (to_x(b.x0 as f32 + a), to_x(b.x0 as f32 + e))).collect()
            };
            lines.push(RecognizedLine { text: decoded.text, rect: Rect::new(x0, y0, x1 - x0, y1 - y0), score: decoded.score, char_boxes });
        }

        let keys: Vec<(f32, f32)> = lines.iter().map(|l| (l.rect.y, l.rect.x)).collect();
        let mut slots: Vec<Option<RecognizedLine>> = lines.into_iter().map(Some).collect();
        Ok(detect::reading_order(&keys, ROW_GAP).into_iter().filter_map(|i| slots[i].take()).collect())
    }

    /// Text boxes in pixels of `img`.
    fn detect(&self, img: &Bgr) -> Result<Vec<detect::DetBox>, OcrError> {
        let (w, h) = prep::det_size(img.width, img.height);
        let resized = img.resize(w, h);
        let mut input = vec![0f32; 3 * w * h];
        resized.write_normalized(&mut input, w, h);
        let input = Tensor::from_shape(&[1, 3, h, w], &input).map_err(inference)?;
        let plan = self.det_plan(w, h)?;
        let out = run(&plan, input)?;
        let shape = out.shape();
        if shape.len() != 4 || shape[0] != 1 || shape[1] != 1 {
            return Err(OcrError::ModelCorrupt(format!("{} 输出形状 {shape:?}", models::DET_MODEL)));
        }
        let (map_h, map_w) = (shape[2], shape[3]);
        let view = out.try_as_plain_ram().map_err(inference)?;
        let pred = view.as_slice::<f32>().map_err(inference)?;
        Ok(detect::boxes_from_map(pred, map_w, map_h, img.width, img.height))
    }

    /// Recognizes crops on a few threads (tract runs a plan on one core). Missing plans are
    /// built first, in parallel too, so no two threads optimize the same width.
    fn recognize_crops(&self, crops: &[(Bgr, bool)]) -> Result<Vec<Option<decode::Decoded>>, OcrError> {
        let threads = std::thread::available_parallelism().map_or(1, |n| n.get()).clamp(1, MAX_REC_THREADS).min(crops.len());
        if threads <= 1 {
            return crops.iter().map(|(c, _)| self.recognize_crop(c)).collect();
        }
        let mut widths: Vec<usize> = crops.iter().map(|(c, _)| prep::rec_widths(c.width, c.height).1).collect();
        widths.sort_unstable();
        widths.dedup();
        let missing: Vec<usize> = {
            let cache = self.rec_plans.lock().map_err(inference)?;
            widths.into_iter().filter(|w| !cache.contains_key(w)).collect()
        };
        std::thread::scope(|s| {
            let jobs: Vec<_> = missing.iter().map(|&w| s.spawn(move || self.rec_plan(w).map(|_| ()))).collect();
            jobs.into_iter().try_for_each(|j| j.join().unwrap_or_else(|_| Err(inference("线程异常"))))
        })?;

        // Widest first, handed out through a shared counter, so threads finish together.
        let mut order: Vec<usize> = (0..crops.len()).collect();
        order.sort_by_key(|&i| std::cmp::Reverse(crops[i].0.width * REC_HEIGHT / crops[i].0.height.max(1)));
        let next = std::sync::atomic::AtomicUsize::new(0);
        let results: Vec<Vec<(usize, CropResult)>> = std::thread::scope(|s| {
            let workers: Vec<_> = (0..threads)
                .map(|_| {
                    s.spawn(|| {
                        let mut out = Vec::new();
                        loop {
                            let k = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                            let Some(&i) = order.get(k) else { break };
                            out.push((i, self.recognize_crop(&crops[i].0)));
                        }
                        out
                    })
                })
                .collect();
            workers.into_iter().map(|w| w.join().unwrap_or_default()).collect()
        });
        let mut slots: Vec<Option<Option<decode::Decoded>>> = vec![None; crops.len()];
        for (i, r) in results.into_iter().flatten() {
            slots[i] = Some(r?);
        }
        // A worker that panicked leaves holes; treat them as failures rather than dropping lines silently.
        slots.into_iter().map(|s| s.ok_or_else(|| inference("线程异常"))).collect()
    }

    /// Recognizes one horizontal crop; `None` for crops too small to read.
    fn recognize_crop(&self, crop: &Bgr) -> CropResult {
        if crop.width == 0 || crop.height == 0 {
            return Ok(None);
        }
        let (resized_w, bucket) = prep::rec_widths(crop.width, crop.height);
        let resized = crop.resize(resized_w, REC_HEIGHT);
        let mut input = vec![0f32; 3 * REC_HEIGHT * bucket];
        resized.write_normalized(&mut input, bucket, REC_HEIGHT);
        let input = Tensor::from_shape(&[1, 3, REC_HEIGHT, bucket], &input).map_err(inference)?;
        let plan = self.rec_plan(bucket)?;
        let out = run(&plan, input)?;
        let shape = out.shape();
        if shape.len() != 3 || shape[0] != 1 || shape[2] != self.classes.len() {
            return Err(OcrError::ModelCorrupt(format!(
                "{} 输出形状 {shape:?}，字符表 {} 项",
                models::REC_MODEL,
                self.classes.len()
            )));
        }
        let (t, c) = (shape[1], shape[2]);
        let view = out.try_as_plain_ram().map_err(inference)?;
        let probs = view.as_slice::<f32>().map_err(inference)?;
        // Steps over the zero padding decode to junk; only read the ones covering the text.
        let t_valid = ((t * resized_w) as f64 / bucket as f64).ceil() as usize;
        let tokens = decode::ctc_greedy(probs, t, c, t_valid);
        // Time step -> crop pixels: steps are `bucket / t` input columns wide.
        let step_px = bucket as f32 / t as f32 * crop.width as f32 / resized_w as f32;
        Ok(Some(decode::assemble(&tokens, &self.classes, step_px, crop.width as f32)))
    }

    fn det_plan(&self, w: usize, h: usize) -> Result<Plan, OcrError> {
        if let Some(plan) = {
            let mut cache = self.det_plans.lock().map_err(inference)?;
            cache.iter().position(|(k, _)| *k == (w, h)).map(|i| {
                let entry = cache.remove(i);
                let plan = entry.1.clone();
                cache.push(entry);
                plan
            })
        } {
            return Ok(plan);
        }
        // Optimizing takes ~150 ms; done outside the lock so other shapes are not blocked.
        let plan = build_plan(&self.det, [1, 3, h, w], models::DET_MODEL)?;
        let mut cache = self.det_plans.lock().map_err(inference)?;
        if !cache.iter().any(|(k, _)| *k == (w, h)) {
            if cache.len() >= DET_PLAN_CACHE {
                cache.remove(0);
            }
            cache.push(((w, h), plan.clone()));
        }
        Ok(plan)
    }

    fn rec_plan(&self, width: usize) -> Result<Plan, OcrError> {
        if let Some(plan) = self.rec_plans.lock().map_err(inference)?.get(&width) {
            return Ok(plan.clone());
        }
        let plan = build_plan(&self.rec, [1, 3, REC_HEIGHT, width], models::REC_MODEL)?;
        let mut cache = self.rec_plans.lock().map_err(inference)?;
        // Very wide lines are rare: keep only a few of their plans around.
        if !REC_BUCKETS.contains(&width) {
            let extras: Vec<usize> = cache.keys().copied().filter(|k| !REC_BUCKETS.contains(k)).collect();
            if extras.len() >= REC_EXTRA_PLANS {
                for k in extras {
                    cache.remove(&k);
                }
            }
        }
        cache.insert(width, plan.clone());
        Ok(plan)
    }
}

fn build_plan(model: &InferenceModel, shape: [usize; 4], name: &str) -> Result<Plan, OcrError> {
    model
        .clone()
        .with_input_fact(0, f32::fact(shape).into())
        .and_then(|m| m.into_optimized())
        .and_then(|m| m.into_runnable())
        .map_err(|e| OcrError::ModelCorrupt(format!("{name}（{e}）")))
}

fn run(plan: &Plan, input: Tensor) -> Result<Tensor, OcrError> {
    let mut out = plan.run(tvec!(input.into())).map_err(inference)?;
    if out.is_empty() {
        return Err(inference("模型没有输出"));
    }
    Ok(out.swap_remove(0).into_tensor())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::time::Instant;

    fn assert_send_sync<T: Send + Sync>() {}

    #[test]
    fn engine_is_shareable() {
        assert_send_sync::<OcrEngine>();
    }

    #[test]
    fn missing_models_are_reported() {
        let dir = std::env::temp_dir().join("shotlate-no-models-here");
        match OcrEngine::load(&dir) {
            Err(e @ OcrError::ModelsMissing) => assert_eq!(e.to_string(), "识别组件未下载"),
            other => panic!("unexpected {:?}", other.map(|_| ())),
        }
    }

    fn models_dir() -> Option<PathBuf> {
        let dir = PathBuf::from(std::env::var_os("SHOTLATE_MODELS")?);
        if models::is_ready(&dir) {
            Some(dir)
        } else {
            println!("note: SHOTLATE_MODELS={} has no complete models; skipping", dir.display());
            None
        }
    }

    fn engine() -> Option<Arc<OcrEngine>> {
        use std::sync::OnceLock;
        static ENGINE: OnceLock<Option<Arc<OcrEngine>>> = OnceLock::new();
        ENGINE
            .get_or_init(|| {
                let Some(dir) = models_dir() else {
                    println!("note: SHOTLATE_MODELS not set; skipping model tests");
                    return None;
                };
                let t = Instant::now();
                let engine = OcrEngine::load(&dir).expect("load models");
                println!("load: {:?}", t.elapsed());
                let t = Instant::now();
                engine.warm_up();
                println!("warm-up: {:?}", t.elapsed());
                Some(Arc::new(engine))
            })
            .clone()
    }

    fn fixture(name: &str) -> RgbaImage {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data").join(name);
        RgbaImage::decode_png(&std::fs::read(path).expect("read fixture")).expect("decode fixture")
    }

    /// Whitespace dropped, full-width punctuation folded to ASCII, so spacing and 。/. style
    /// differences between the ground truth and the model do not count as errors.
    fn normalize(s: &str) -> Vec<char> {
        s.chars()
            .filter(|c| !c.is_whitespace())
            .map(|c| match c {
                '，' => ',',
                '。' => '.',
                '：' => ':',
                '；' => ';',
                '？' => '?',
                '！' => '!',
                '（' => '(',
                '）' => ')',
                '～' => '~',
                '“' | '”' => '"',
                '‘' | '’' => '\'',
                '・' | '•' => '·',
                c if ('\u{FF01}'..='\u{FF5E}').contains(&c) => char::from_u32(c as u32 - 0xFEE0).unwrap_or(c),
                c => c,
            })
            .collect()
    }

    fn similarity(a: &str, b: &str) -> f32 {
        let (a, b) = (normalize(a), normalize(b));
        if a.is_empty() && b.is_empty() {
            return 1.0;
        }
        let mut prev: Vec<usize> = (0..=b.len()).collect();
        for (i, ca) in a.iter().enumerate() {
            let mut cur = vec![i + 1; b.len() + 1];
            for (j, cb) in b.iter().enumerate() {
                cur[j + 1] = (prev[j] + (ca != cb) as usize).min(prev[j + 1] + 1).min(cur[j] + 1);
            }
            prev = cur;
        }
        1.0 - prev[b.len()] as f32 / a.len().max(b.len()) as f32
    }

    fn timed(engine: &OcrEngine, img: &RgbaImage, label: &str) -> Vec<RecognizedLine> {
        let t = Instant::now();
        let lines = engine.recognize(img).expect("recognize");
        println!("{label} first: {:?}", t.elapsed());
        let t = Instant::now();
        let again = engine.recognize(img).expect("recognize");
        println!("{label} again: {:?}", t.elapsed());
        assert_eq!(lines, again, "deterministic");
        for l in &lines {
            println!("  {:.2} [{:.0},{:.0} {:.0}x{:.0}] {}", l.score, l.rect.x, l.rect.y, l.rect.width, l.rect.height, l.text);
        }
        lines
    }

    fn check_geometry(lines: &[RecognizedLine], img: &RgbaImage) {
        for l in lines {
            assert!(l.rect.x >= 0.0 && l.rect.y >= 0.0);
            assert!(l.rect.max_x() <= img.width as f32 && l.rect.max_y() <= img.height as f32);
            assert_eq!(l.char_boxes.len(), l.text.chars().count(), "{}", l.text);
            for w in l.char_boxes.windows(2) {
                assert!(w[0].0 <= w[1].0 && w[0].1 <= w[1].1 + 0.01, "{:?} in {}", l.char_boxes, l.text);
            }
            for &(a, b) in &l.char_boxes {
                assert!(a >= l.rect.x - 0.01 && b <= l.rect.max_x() + 0.01 && a <= b);
            }
        }
    }

    #[test]
    fn recognizes_mixed_text() {
        let Some(engine) = engine() else { return };
        let img = fixture("mixed.png");
        let lines = timed(&engine, &img, "mixed.png");
        check_geometry(&lines, &img);
        let truth = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/mixed.txt")).expect("truth");
        let truth: Vec<&str> = truth.lines().filter(|l| !l.trim().is_empty()).collect();
        let mut hits = 0;
        for want in &truth {
            let best = lines.iter().map(|l| similarity(want, &l.text)).fold(0.0, f32::max);
            println!("  {best:.3} <- {want}");
            if best >= 0.9 {
                hits += 1;
            }
        }
        assert!(hits >= 7, "only {hits} of {} lines recognized", truth.len());
        // Reading order: the ground truth lines come out in order.
        let firsts: Vec<usize> = truth
            .iter()
            .filter_map(|want| lines.iter().position(|l| similarity(want, &l.text) >= 0.9))
            .collect();
        assert!(firsts.windows(2).all(|w| w[0] < w[1]), "order {firsts:?}");
    }

    #[test]
    fn recognizes_activity_monitor() {
        let Some(engine) = engine() else { return };
        let img = fixture("memory.png");
        let lines = timed(&engine, &img, "memory.png");
        check_geometry(&lines, &img);
        let all: Vec<String> = lines.iter().map(|l| l.text.replace(' ', "")).collect();
        for want in ["活动监视器", "WeChatHelper", "Shotlate", "52.9MB", "28720"] {
            assert!(all.iter().any(|t| t.contains(want)), "missing {want}");
        }
        // "Shotlate" should sit left of "50.0 MB" on the same row.
        let find = |s: &str| lines.iter().find(|l| l.text.replace(' ', "").contains(s)).map(|l| l.rect);
        if let (Some(a), Some(b)) = (find("Shotlate"), find("50.0MB")) {
            assert!(a.max_x() < b.x && (a.mid_y() - b.mid_y()).abs() < 10.0);
        }
    }

    #[test]
    fn odd_sizes_do_not_panic() {
        let Some(engine) = engine() else { return };
        for (w, h) in [(1, 1), (0, 0), (5, 3000), (3000, 5), (64, 64)] {
            let img = RgbaImage::filled(w, h, [255, 255, 255, 255]);
            let lines = engine.recognize(&img).expect("recognize");
            assert!(lines.is_empty(), "{w}x{h}: {lines:?}");
        }
        // A thin strip cut from real text still reads.
        let img = fixture("memory.png");
        if let Some(strip) = img.crop(80, 160, 400, 40) {
            let lines = engine.recognize(&strip).expect("recognize strip");
            println!("strip: {:?}", lines.iter().map(|l| &l.text).collect::<Vec<_>>());
            assert!(lines.iter().any(|l| l.text.contains("WeChat")));
        }
        let bad = RgbaImage { width: 10, height: 10, data: vec![0; 7] };
        assert!(engine.recognize(&bad).expect("short buffer").is_empty());
    }
}
