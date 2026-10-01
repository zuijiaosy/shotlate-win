//! Gradient-weighted SAD matching and a replaceable tail, matching the macOS stitcher.

use super::image::RgbaImage;
use std::ops::Range;

const BANDS: usize = 48;
const TOLERANCE: i32 = 4;
const MAX_SCORE: f64 = 2.5;
pub const MAX_CAPTURE_BYTES: usize = 256 << 20;

pub fn height_budget(width: usize) -> usize {
    if width == 0 { 0 } else { 60_000.min(MAX_CAPTURE_BYTES / width / 4) }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StitchResult { Started, Appended(usize), Unchanged, ScrolledBack, NoOverlap, LimitReached }

struct Signatures { rows: usize, values: Vec<u8>, energy: Vec<usize> }

pub struct ScrollStitcher {
    pub max_height: usize,
    pub ignored_side_columns: usize,
    pub width: usize,
    frame_height: usize,
    chunks: Vec<Vec<u8>>,
    committed: usize,
    tail: Vec<u8>,
    edge: usize,
    first: Option<RgbaImage>,
    previous: Option<Signatures>,
}

impl ScrollStitcher {
    pub fn new(max_height: usize, ignored_side_columns: usize) -> Self {
        Self { max_height, ignored_side_columns, width: 0, frame_height: 0, chunks: Vec::new(), committed: 0,
            tail: Vec::new(), edge: 0, first: None, previous: None }
    }

    pub fn height(&self) -> usize { if self.width == 0 { 0 } else { self.committed + self.tail.len() / (self.width * 4) } }

    pub fn add(&mut self, frame: RgbaImage, hint: Option<i32>) -> StitchResult {
        let h = frame.height as usize;
        if frame.width == 0 || h < 16 || frame.data.len() != frame.width as usize * h * 4 { return StitchResult::NoOverlap; }
        if h > self.max_height { return StitchResult::LimitReached; }
        let sig = self.signatures(&frame);
        if self.width != frame.width as usize || self.frame_height != h || self.previous.is_none() {
            self.width = frame.width as usize;
            self.frame_height = h;
            self.chunks.clear(); self.committed = 0; self.edge = 0;
            self.tail = frame.data.clone(); self.first = Some(frame); self.previous = Some(sig);
            return StitchResult::Started;
        }
        let prev = self.previous.as_ref().unwrap();
        let same = (0..h).filter(|&y| rows_match(prev, y, &sig, y)).count();
        if same >= h - (h / 100).max(1) { return StitchResult::Unchanged; }
        let (top, bottom) = fixed_rows(prev, &sig);
        let band = top..h - bottom;
        if band.len() < 16 { return StitchResult::Unchanged; }
        let Some(d) = offset(prev, &sig, band.clone(), hint) else { return StitchResult::NoOverlap };
        if d == 0 { return StitchResult::Unchanged; }
        if d < 0 {
            let edge = self.edge + (-d) as usize;
            if self.first.is_none() && edge <= band.end {
                self.edge = edge; self.tail = rows(&frame, edge..h); self.previous = Some(sig);
            }
            return StitchResult::ScrolledBack;
        }
        let flush = band.start.max(band.end.saturating_sub(120.min(h / 8)));
        let committed = if self.first.is_some() { flush } else { self.committed };
        let edge = if self.first.is_some() { flush } else { self.edge };
        if edge < d as usize + band.start { return StitchResult::NoOverlap; }
        let start = edge - d as usize;
        let end = start.max(flush);
        if committed + end - start + h - end > self.max_height { return StitchResult::LimitReached; }
        if let Some(first) = self.first.take() { self.append(rows(&first, 0..flush)); }
        if end > start { self.append(rows(&frame, start..end)); }
        self.edge = end; self.tail = rows(&frame, end..h); self.previous = Some(sig);
        StitchResult::Appended(d as usize)
    }

    fn append(&mut self, data: Vec<u8>) {
        self.committed += data.len() / (self.width * 4);
        if let Some(last) = self.chunks.last_mut().filter(|c| c.len() + data.len() <= 8 << 20) {
            last.extend(data);
        } else { self.chunks.push(data); }
    }

    fn signatures(&self, frame: &RgbaImage) -> Signatures {
        let width = frame.width as usize;
        let side = if width >= 2 * self.ignored_side_columns + BANDS { self.ignored_side_columns } else { 0 };
        let columns = width - 2 * side;
        let mut sig = Signatures { rows: frame.height as usize, values: vec![0; frame.height as usize * BANDS], energy: vec![0; frame.height as usize] };
        for y in 0..sig.rows {
            let mut previous: Option<i32> = None;
            for k in 0..BANDS {
                let x0 = k * columns / BANDS;
                let x1 = (x0 + 1).max((k + 1) * columns / BANDS).min(columns);
                let sum: usize = (x0..x1).map(|x| {
                    let i = (y * width + side + x) * 4;
                    frame.data[i] as usize * 2 + frame.data[i + 1] as usize * 5 + frame.data[i + 2] as usize
                }).sum();
                let value = (sum / ((x1 - x0).max(1) * 8)) as u8;
                sig.values[y * BANDS + k] = value;
                if let Some(p) = previous { sig.energy[y] += (value as i32 - p).unsigned_abs() as usize; }
                previous = Some(value as i32);
            }
        }
        sig
    }

    pub fn row(&self, mut y: usize) -> &[u8] {
        if self.width == 0 { return &[]; }
        let stride = self.width * 4;
        for chunk in self.chunks.iter().chain(std::iter::once(&self.tail)) {
            let height = chunk.len() / stride;
            if y < height { return &chunk[y * stride..(y + 1) * stride]; }
            y -= height;
        }
        &[]
    }

    #[cfg(test)]
    pub fn image(&self) -> Option<RgbaImage> {
        if self.width == 0 || self.height() == 0 { return None; }
        let mut data = Vec::with_capacity(self.height() * self.width * 4);
        for chunk in self.chunks.iter().chain(std::iter::once(&self.tail)) { data.extend_from_slice(chunk); }
        Some(RgbaImage { width: self.width as u32, height: self.height() as u32, data })
    }

    pub fn into_image(mut self) -> Option<RgbaImage> {
        if self.width == 0 || self.height() == 0 { return None; }
        let height = self.height();
        self.first = None;
        let mut data = Vec::new();
        data.try_reserve_exact(height.checked_mul(self.width)?.checked_mul(4)?).ok()?;
        for chunk in self.chunks.into_iter().chain(std::iter::once(self.tail)) { data.extend_from_slice(&chunk); }
        Some(RgbaImage { width: self.width as u32, height: height as u32, data })
    }

    pub fn preview(&self, target_width: usize, max_height: usize) -> Option<RgbaImage> {
        if self.width == 0 || self.height() == 0 || target_width == 0 || max_height == 0 { return None; }
        let width = target_width.min(self.width);
        let scale = width as f64 / self.width as f64;
        let height = ((self.height() as f64 * scale) as usize).max(1).min(max_height);
        let start = self.height().saturating_sub((height as f64 / scale).ceil() as usize);
        let mut image = RgbaImage::filled(width as u32, height as u32, [255; 4]);
        for y in 0..height {
            let row = self.row((start + (y as f64 / scale) as usize).min(self.height() - 1));
            for x in 0..width {
                let sx = ((x as f64 / scale) as usize).min(self.width - 1);
                image.data[(y * width + x) * 4..(y * width + x + 1) * 4].copy_from_slice(&row[sx * 4..sx * 4 + 4]);
            }
        }
        Some(image)
    }
}

fn rows(frame: &RgbaImage, range: Range<usize>) -> Vec<u8> {
    frame.data[range.start * frame.width as usize * 4..range.end * frame.width as usize * 4].to_vec()
}
fn rows_match(a: &Signatures, i: usize, b: &Signatures, j: usize) -> bool {
    (0..BANDS).all(|k| (a.values[i * BANDS + k] as i32 - b.values[j * BANDS + k] as i32).abs() <= TOLERANCE)
}
fn fixed_rows(a: &Signatures, b: &Signatures) -> (usize, usize) {
    let fixed = |y: usize| (0..BANDS).filter(|&k| (a.values[y * BANDS + k] as i32 - b.values[y * BANDS + k] as i32).abs() > TOLERANCE).count() <= 2;
    let cap = a.rows / 3;
    let top = (0..cap).take_while(|&y| fixed(y)).count();
    let bottom = (0..cap).take_while(|&y| fixed(a.rows - 1 - y)).count();
    (top, bottom)
}
fn score(a: &Signatures, b: &Signatures, templates: &[usize], d: i32, band: Range<usize>) -> Option<f64> {
    let total: usize = templates.iter().map(|&y| a.energy[y] + 16).sum();
    let (mut weighted, mut weight, mut count) = (0u64, 0u64, 0);
    for &y in templates {
        let j = y as i32 - d;
        if j < band.start as i32 || j >= band.end as i32 { continue; }
        let diff: u64 = (0..BANDS).map(|k| (a.values[y * BANDS + k] as i32 - b.values[j as usize * BANDS + k] as i32).unsigned_abs() as u64).sum();
        let w = (a.energy[y] + 16) as u64;
        weighted += w * diff; weight += w; count += 1;
    }
    if count < 4 || weight * 5 < total as u64 { None } else { Some(weighted as f64 / (weight * BANDS as u64) as f64) }
}
fn sample(rows: &[usize], limit: usize) -> Vec<usize> {
    if rows.len() <= limit { rows.to_vec() } else { (0..limit).map(|i| rows[i * rows.len() / limit]).collect() }
}
fn offset(a: &Signatures, b: &Signatures, band: Range<usize>, hint: Option<i32>) -> Option<i32> {
    let detailed: Vec<_> = band.clone().filter(|&y| {
        let row = &a.values[y * BANDS..(y + 1) * BANDS];
        row.iter().max().unwrap() - row.iter().min().unwrap() >= 6
    }).collect();
    let templates = sample(&detailed, 192);
    if templates.len() < 4 || band.len() <= 8 { return None; }
    let sparse = sample(&templates, 48);
    let limit = band.len() as i32 - 8;
    let scores: Vec<_> = (-limit..=limit).map(|d| score(a, b, &sparse, d, band.clone()).unwrap_or(f64::INFINITY)).collect();
    let mut seeds: Vec<_> = (0..scores.len()).filter(|&i| scores[i] <= MAX_SCORE * 1.5 && (i == 0 || scores[i - 1] >= scores[i]) && (i + 1 == scores.len() || scores[i + 1] >= scores[i])).collect();
    seeds.sort_by(|&i, &j| scores[i].total_cmp(&scores[j]));
    let mut minima: Vec<(i32, f64)> = Vec::new();
    for i in seeds {
        let d = i as i32 - limit;
        if minima.len() >= 6 { break; }
        if minima.iter().any(|&(p, _)| (p - d).abs() <= 3) { continue; }
        if let Some(s) = score(a, b, &templates, d, band.clone()) { minima.push((d, s)); }
    }
    minima.retain(|&(_, s)| s <= MAX_SCORE);
    let best = minima.iter().map(|&(_, s)| s).min_by(f64::total_cmp)?;
    let mut ties: Vec<_> = minima.into_iter().filter(|&(_, s)| s <= best * 1.5 + 1.5).collect();
    if ties.len() == 1 { return Some(ties[0].0); }
    // Repeating rows cannot prove direction or distance; never invent a seam.
    let target = hint?;
    ties.sort_by_key(|&(d, _)| (d - target).abs());
    if ties.len() > 1 && (ties[0].0 - target).abs() + 3 >= (ties[1].0 - target).abs() { return None; }
    ties.first().map(|&(d, _)| d)
}

#[derive(Default, Debug)]
pub struct AutoScroll { pub running: bool, idle: u8, failures: u8, pub notches: i32, awaiting: Option<u64>, had_progress: bool }
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AutoAction { Wait, Continue, Recover(i32), Finish, Pause }
impl AutoScroll {
    pub fn start(&mut self) { self.running = true; self.idle = 0; self.failures = 0; self.notches = 2; self.awaiting = None; self.had_progress = false; }
    pub fn pause(&mut self) { self.running = false; self.awaiting = None; }
    pub fn await_frame(&mut self, serial: u64) { self.awaiting = Some(serial); }
    pub fn frame_processed(&mut self, serial: u64) {
        if self.awaiting.is_some_and(|required| serial >= required) { self.awaiting = None; }
    }
    pub fn settled(&mut self, progress: bool, lost: bool) -> AutoAction {
        if !self.running { return AutoAction::Pause; }
        if self.awaiting.is_some() { return AutoAction::Wait; }
        if lost {
            self.failures += 1;
            if self.failures >= 3 { self.pause(); return AutoAction::Pause; }
            let rollback = self.notches;
            self.notches = (self.notches - 1).max(1);
            return AutoAction::Recover(rollback);
        }
        self.failures = 0;
        self.had_progress |= progress;
        self.idle = if progress { 0 } else { self.idle + 1 };
        if self.idle >= 2 {
            self.pause(); if self.had_progress { AutoAction::Finish } else { AutoAction::Pause }
        } else { AutoAction::Continue }
    }
}

#[cfg(test)]
#[path = "scrollstitcher_tests.rs"]
mod tests;
