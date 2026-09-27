//! Text layout and drawing with system fonts. Glyph outlines become tiny-skia paths, so text is
//! antialiased at any scale, can be stroked (outlined text) and looks the same on screen and in exports.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use ab_glyph::{Font, FontRef, GlyphId, OutlineCurve};
use tiny_skia::{Path, PathBuilder, Transform};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Weight {
    Regular,
    Bold,
}

/// Candidate font files per weight, tried in order for every character, so CJK, Latin and Korean all render.
fn candidates(weight: Weight) -> Vec<(&'static str, u32)> {
    if cfg!(windows) {
        let fonts = |name: &str| format!("{}\\Fonts\\{}", std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".into()), name);
        let leak = |s: String| -> &'static str { Box::leak(s.into_boxed_str()) };
        // Microsoft YaHei UI (index 1 of the collection) has the tighter UI line spacing.
        let primary = match weight {
            Weight::Regular => vec![(leak(fonts("msyh.ttc")), 1), (leak(fonts("msyh.ttf")), 0)],
            Weight::Bold => vec![(leak(fonts("msyhbd.ttc")), 1), (leak(fonts("msyhbd.ttf")), 0), (leak(fonts("msyh.ttc")), 1)],
        };
        let mut all = primary;
        all.extend([
            (leak(fonts(if weight == Weight::Bold { "segoeuib.ttf" } else { "segoeui.ttf" })), 0),
            (leak(fonts(if weight == Weight::Bold { "malgunbd.ttf" } else { "malgun.ttf" })), 0),
            (leak(fonts("YuGothM.ttc")), 0),
            (leak(fonts("seguisym.ttf")), 0),
            (leak(fonts("simsun.ttc")), 0),
            (leak(fonts("arial.ttf")), 0),
        ]);
        all
    } else if cfg!(target_os = "macos") {
        match weight {
            Weight::Regular => vec![
                ("/System/Library/Fonts/Hiragino Sans GB.ttc", 0),
                ("/System/Library/Fonts/STHeiti Light.ttc", 0),
                ("/System/Library/Fonts/AppleSDGothicNeo.ttc", 0),
                ("/System/Library/Fonts/Supplemental/Arial Unicode.ttf", 0),
                ("/System/Library/Fonts/Helvetica.ttc", 0),
            ],
            Weight::Bold => vec![
                ("/System/Library/Fonts/Hiragino Sans GB.ttc", 1),
                ("/System/Library/Fonts/STHeiti Medium.ttc", 0),
                ("/System/Library/Fonts/AppleSDGothicNeo.ttc", 6),
                ("/System/Library/Fonts/Supplemental/Arial Unicode.ttf", 0),
                ("/System/Library/Fonts/Helvetica.ttc", 1),
            ],
        }
    } else {
        vec![
            ("/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc", 0),
            ("/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf", 0),
        ]
    }
}

struct FontChain {
    fonts: Vec<FontRef<'static>>,
}

/// The bytes of a font file for the life of the process. On Windows the file is memory-mapped: the
/// fallback chain is several CJK fonts of 10–20 MB each, and mapping keeps them file-backed and
/// shared, with only the pages of glyphs actually drawn resident, instead of ~100 MB of private copies.
fn font_bytes(path: &str) -> Option<&'static [u8]> {
    #[cfg(windows)]
    {
        use windows::Win32::Foundation::{CloseHandle, GENERIC_READ};
        use windows::Win32::Storage::FileSystem::{CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, GetFileSizeEx, OPEN_EXISTING};
        use windows::Win32::System::Memory::{CreateFileMappingW, FILE_MAP_READ, MapViewOfFile, PAGE_READONLY};
        let wide: Vec<u16> = path.encode_utf16().chain(Some(0)).collect();
        unsafe {
            let file = CreateFileW(windows::core::PCWSTR(wide.as_ptr()), GENERIC_READ.0, FILE_SHARE_READ, None, OPEN_EXISTING, FILE_ATTRIBUTE_NORMAL, None).ok()?;
            let mut size = 0i64;
            let sized = GetFileSizeEx(file, &mut size).is_ok() && size > 0;
            let mapping = if sized { CreateFileMappingW(file, None, PAGE_READONLY, 0, 0, None).ok() } else { None };
            let _ = CloseHandle(file);
            let mapping = mapping?;
            // The view keeps the mapping alive; it is never unmapped (fonts live as long as the process).
            let view = MapViewOfFile(mapping, FILE_MAP_READ, 0, 0, 0);
            let _ = CloseHandle(mapping);
            if view.Value.is_null() {
                return None;
            }
            Some(std::slice::from_raw_parts(view.Value as *const u8, size as usize))
        }
    }
    #[cfg(not(windows))]
    {
        std::fs::read(path).ok().map(|d| &*Box::leak(d.into_boxed_slice()))
    }
}

impl FontChain {
    fn load(weight: Weight) -> FontChain {
        let mut fonts = Vec::new();
        for (path, index) in candidates(weight) {
            if let Some(data) = font_bytes(path) {
                if let Ok(font) = FontRef::try_from_slice_and_index(data, index).or_else(|_| FontRef::try_from_slice(data)) {
                    fonts.push(font);
                }
            }
        }
        FontChain { fonts }
    }

    /// The first font that has `c`, or the primary font (drawing its missing-glyph box).
    fn resolve(&self, c: char) -> Option<(usize, GlyphId)> {
        for (i, f) in self.fonts.iter().enumerate() {
            let id = f.glyph_id(c);
            if id.0 != 0 {
                return Some((i, id));
            }
        }
        self.fonts.first().map(|f| (0, f.glyph_id(c)))
    }
}

struct Fonts {
    regular: FontChain,
    bold: OnceLock<FontChain>,
    /// Glyph outlines at 1 unit per em, y down, cached per (weight, font, glyph).
    outlines: Mutex<HashMap<(Weight, usize, u16), Option<Path>>>,
}

static FONTS: OnceLock<Fonts> = OnceLock::new();

fn fonts() -> &'static Fonts {
    FONTS.get_or_init(|| Fonts { regular: FontChain::load(Weight::Regular), bold: OnceLock::new(), outlines: Mutex::new(HashMap::new()) })
}

/// Loads the regular fonts in the background, so the first capture doesn't wait on reading them.
pub fn preload() {
    std::thread::spawn(|| {
        let _ = fonts();
    });
}

fn chain(weight: Weight) -> &'static FontChain {
    let f = fonts();
    match weight {
        Weight::Regular => &f.regular,
        Weight::Bold => {
            let bold = f.bold.get_or_init(|| FontChain::load(Weight::Bold));
            if bold.fonts.is_empty() { &f.regular } else { bold }
        }
    }
}

pub fn fonts_available() -> bool {
    !fonts().regular.fonts.is_empty()
}

fn units_per_em(font: &FontRef<'static>) -> f32 {
    font.units_per_em().unwrap_or(1000.0)
}

fn glyph_path(weight: Weight, font_index: usize, id: GlyphId) -> Option<Path> {
    let key = (weight, font_index, id.0);
    if let Some(p) = fonts().outlines.lock().ok()?.get(&key) {
        return p.clone();
    }
    let font = &chain(weight).fonts[font_index];
    let upem = units_per_em(font);
    let path = font.outline(id).and_then(|outline| {
        let mut pb = PathBuilder::new();
        let mut last: Option<ab_glyph::Point> = None;
        let pt = |p: ab_glyph::Point| (p.x / upem, -p.y / upem);
        for curve in &outline.curves {
            let (start, end) = match curve {
                OutlineCurve::Line(a, b) => (*a, *b),
                OutlineCurve::Quad(a, _, b) => (*a, *b),
                OutlineCurve::Cubic(a, _, _, b) => (*a, *b),
            };
            if last.map_or(true, |l| (l.x - start.x).abs() > 1e-3 || (l.y - start.y).abs() > 1e-3) {
                if last.is_some() {
                    pb.close();
                }
                let (x, y) = pt(start);
                pb.move_to(x, y);
            }
            match curve {
                OutlineCurve::Line(_, b) => {
                    let (x, y) = pt(*b);
                    pb.line_to(x, y);
                }
                OutlineCurve::Quad(_, c, b) => {
                    let (cx, cy) = pt(*c);
                    let (x, y) = pt(*b);
                    pb.quad_to(cx, cy, x, y);
                }
                OutlineCurve::Cubic(_, c1, c2, b) => {
                    let (x1, y1) = pt(*c1);
                    let (x2, y2) = pt(*c2);
                    let (x, y) = pt(*b);
                    pb.cubic_to(x1, y1, x2, y2, x, y);
                }
            }
            last = Some(end);
        }
        if last.is_some() {
            pb.close();
        }
        pb.finish()
    });
    if let Ok(mut cache) = fonts().outlines.lock() {
        cache.insert(key, path.clone());
    }
    path
}

#[derive(Clone, Debug)]
pub struct Glyph {
    pub font: usize,
    pub id: GlyphId,
    /// Left edge of the glyph's advance box, in points from the line start.
    pub x: f32,
    pub advance: f32,
    /// Byte offset of the character in the laid-out string.
    pub byte: usize,
}

#[derive(Clone, Debug)]
pub struct Line {
    pub glyphs: Vec<Glyph>,
    pub width: f32,
    /// Top of the line box, from the layout's top.
    pub top: f32,
    /// Byte range of the text on this line (without the break character).
    pub start: usize,
    pub end: usize,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Align {
    Left,
    Center,
}

#[derive(Clone, Debug)]
pub struct TextLayout {
    pub lines: Vec<Line>,
    pub size: f32,
    pub weight: Weight,
    pub line_height: f32,
    pub ascent: f32,
    pub width: f32,
    pub height: f32,
    pub align: Align,
    pub max_width: Option<f32>,
}

/// Line metrics of the primary font at `size`: (ascent, line height).
pub fn metrics(size: f32, weight: Weight) -> (f32, f32) {
    match chain(weight).fonts.first() {
        Some(f) => {
            let upem = units_per_em(f);
            let ascent = f.ascent_unscaled() / upem * size;
            let descent = -f.descent_unscaled() / upem * size;
            // Line gap left out: annotation text should sit tight, like NSTextView's default line height.
            (ascent, (ascent + descent).max(size))
        }
        None => (size * 0.9, size * 1.2),
    }
}

fn is_cjk(c: char) -> bool {
    matches!(c as u32, 0x2E80..=0x9FFF | 0xAC00..=0xD7AF | 0xF900..=0xFAFF | 0xFF00..=0xFFEF)
}

/// Lays out `text` at `size` points, wrapping at `max_width` (on spaces, and between CJK characters;
/// a word longer than the line breaks anywhere). `\n` always breaks.
pub fn layout(text: &str, size: f32, weight: Weight, max_width: Option<f32>) -> TextLayout {
    let chain = chain(weight);
    let (ascent, line_height) = metrics(size, weight);
    let mut lines = Vec::new();
    let mut top = 0.0;

    for (para_start, para) in split_paragraphs(text) {
        // Characters of this paragraph with their advances.
        let mut chars: Vec<(usize, char, usize, GlyphId, f32)> = Vec::new();
        for (i, c) in para.char_indices() {
            let (fi, id) = chain.resolve(c).unwrap_or((0, GlyphId(0)));
            // No kerning: UI and annotation text is short, and CJK fonts rarely kern.
            let adv = match chain.fonts.get(fi) {
                Some(f) => f.h_advance_unscaled(id) / units_per_em(f) * size,
                None => size * 0.5,
            };
            chars.push((para_start + i, c, fi, id, adv));
        }

        let mut line_start = 0; // index into chars
        while line_start <= chars.len() {
            // Find the end of this line.
            let mut x = 0.0;
            let mut end = line_start;
            let mut last_break: Option<usize> = None; // index after which we may break
            while end < chars.len() {
                let (_, c, _, _, adv) = chars[end];
                if let Some(maxw) = max_width {
                    if x + adv > maxw + 0.01 && end > line_start && !c.is_whitespace() {
                        break;
                    }
                }
                x += adv;
                if c.is_whitespace() || is_cjk(c) || chars.get(end + 1).is_some_and(|n| is_cjk(n.1)) {
                    last_break = Some(end + 1);
                }
                end += 1;
            }
            if end < chars.len() {
                if let Some(b) = last_break.filter(|b| *b > line_start) {
                    end = b;
                }
            }
            let slice = &chars[line_start..end];
            let mut glyphs = Vec::with_capacity(slice.len());
            let mut gx = 0.0;
            for &(byte, _, fi, id, adv) in slice {
                glyphs.push(Glyph { font: fi, id, x: gx, advance: adv, byte });
                gx += adv;
            }
            // Trailing spaces don't count toward the width.
            let mut width = gx;
            for &(_, c, _, _, adv) in slice.iter().rev() {
                if c.is_whitespace() {
                    width -= adv;
                } else {
                    break;
                }
            }
            let start = slice.first().map_or(para_start + para.len(), |c| c.0);
            let endb = slice.last().map_or(start, |c| c.0 + c.1.len_utf8());
            lines.push(Line { glyphs, width: width.max(0.0), top, start, end: endb });
            top += line_height;
            if end >= chars.len() {
                break;
            }
            line_start = end;
            // Skip the spaces the line broke on.
            while line_start < chars.len() && chars[line_start].1 == ' ' {
                line_start += 1;
            }
            if line_start >= chars.len() {
                break;
            }
        }
    }
    let width = lines.iter().fold(0.0f32, |w, l| w.max(l.width));
    TextLayout { height: top, lines, size, weight, line_height, ascent, width, align: Align::Left, max_width }
}

fn split_paragraphs(text: &str) -> Vec<(usize, &str)> {
    let mut out = Vec::new();
    let mut start = 0;
    for (i, c) in text.char_indices() {
        if c == '\n' {
            out.push((start, &text[start..i]));
            start = i + 1;
        }
    }
    out.push((start, &text[start..]));
    out
}

/// Size of `text` wrapped at `width` (an empty string measures as one line, like NSString).
pub fn measure(text: &str, size: f32, weight: Weight, width: Option<f32>) -> (f32, f32) {
    let l = layout(if text.is_empty() { " " } else { text }, size, weight, width);
    (l.width, l.height)
}

impl TextLayout {
    pub fn with_align(mut self, align: Align) -> TextLayout {
        self.align = align;
        self
    }

    fn line_x(&self, line: &Line) -> f32 {
        match (self.align, self.max_width) {
            (Align::Center, Some(w)) => (w - line.width) / 2.0,
            (Align::Center, None) => (self.width - line.width) / 2.0,
            _ => 0.0,
        }
    }

    /// All glyph outlines as one path, positioned with the layout's top-left at `origin`.
    pub fn path(&self, origin: (f32, f32)) -> Option<Path> {
        let chain = chain(self.weight);
        let mut pb = PathBuilder::new();
        for line in &self.lines {
            let lx = self.line_x(line);
            for g in &line.glyphs {
                if chain.fonts.get(g.font).is_none() {
                    continue;
                }
                if let Some(p) = glyph_path(self.weight, g.font, g.id) {
                    let ts = Transform::from_row(self.size, 0.0, 0.0, self.size, origin.0 + lx + g.x, origin.1 + line.top + self.ascent);
                    if let Some(p) = p.transform(ts) {
                        pb.push_path(&p);
                    }
                }
            }
        }
        pb.finish()
    }

    /// The caret position (x, top) for byte offset `byte`, relative to the layout origin.
    pub fn caret(&self, byte: usize) -> (f32, f32) {
        for (i, line) in self.lines.iter().enumerate() {
            let last = i + 1 == self.lines.len();
            if byte <= line.end || last {
                let lx = self.line_x(line);
                // A caret at the start of the next line belongs there, not at the end of this one.
                if byte >= line.end && !last && self.lines[i + 1].start <= byte {
                    continue;
                }
                let x = line.glyphs.iter().find(|g| g.byte >= byte).map_or(line.width.max(line.glyphs.last().map_or(0.0, |g| g.x + g.advance)), |g| g.x);
                return (lx + x, line.top);
            }
        }
        (0.0, 0.0)
    }

    /// Highlight rects (x, top, width, height) for the byte range `start..end`.
    pub fn selection_rects(&self, start: usize, end: usize) -> Vec<(f32, f32, f32, f32)> {
        let mut out = Vec::new();
        if start >= end {
            return out;
        }
        for line in &self.lines {
            let lx = self.line_x(line);
            let xs: Vec<f32> = line.glyphs.iter().filter(|g| g.byte >= start && g.byte < end).flat_map(|g| [g.x, g.x + g.advance]).collect();
            if let (Some(a), Some(b)) = (xs.iter().cloned().reduce(f32::min), xs.iter().cloned().reduce(f32::max)) {
                out.push((lx + a, line.top, b - a, self.line_height));
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wraps_and_measures() {
        if !fonts_available() {
            eprintln!("no fonts on this machine; skipping");
            return;
        }
        let one = layout("Hello world", 20.0, Weight::Regular, None);
        assert_eq!(one.lines.len(), 1);
        let wrapped = layout("Hello world", 20.0, Weight::Regular, Some(one.width * 0.7));
        assert_eq!(wrapped.lines.len(), 2);
        assert!(wrapped.height > one.height);
        let cjk = layout("截图里的文字会在本机识别", 20.0, Weight::Regular, Some(60.0));
        assert!(cjk.lines.len() >= 3, "CJK wraps between characters");
        let newline = layout("a\nb", 20.0, Weight::Regular, None);
        assert_eq!(newline.lines.len(), 2);
        let empty = measure("", 20.0, Weight::Regular, None);
        assert!(empty.1 > 0.0);
        assert!(one.path((0.0, 0.0)).is_some());
    }

    #[test]
    fn caret_positions() {
        if !fonts_available() {
            return;
        }
        let l = layout("ab\ncd", 20.0, Weight::Regular, None);
        assert_eq!(l.caret(0), (0.0, 0.0));
        let (x, top) = l.caret(3);
        assert_eq!(x, 0.0);
        assert!(top > 0.0);
        let (x_end, _) = l.caret(5);
        assert!(x_end > 0.0);
    }
}
