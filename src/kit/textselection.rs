//! Selecting recognized text on a pinned picture with the mouse (TextSelection.swift).
//! All rects share the picture's pixel space.

use super::geom::{Point, Rect};
use super::textblocks::{self, OcrLine};

/// One recognized line with a box for every character, so a drag can select part of it.
#[derive(Clone, Debug, PartialEq)]
pub struct GlyphLine {
    pub text: String,
    pub rect: Rect,
    /// One box per `char` of `text`, left to right, each as tall as the line.
    pub boxes: Vec<Rect>,
    chars: Vec<char>,
}

impl GlyphLine {
    /// `spans` holds each character's x-range (from OCR), or None where there is none. Runs of equal
    /// spans are shared out evenly, and characters without one fill the gap between their neighbours.
    pub fn new(text: &str, rect: Rect, spans: &[Option<(f32, f32)>]) -> GlyphLine {
        let chars: Vec<char> = text.chars().collect();
        let n = chars.len();
        let usable = |s: &(f32, f32)| s.1 - s.0 > 0.01 && s.1 > rect.min_x() - rect.height && s.0 < rect.max_x() + rect.height;
        let mut xs: Vec<Option<(f32, f32)>> = if spans.len() == n { spans.iter().map(|s| s.filter(usable)).collect() } else { vec![None; n] };

        let share = |xs: &mut Vec<Option<(f32, f32)>>, span: (f32, f32), range: std::ops::Range<usize>| {
            let step = (span.1 - span.0) / range.len() as f32;
            for (k, i) in range.enumerate() {
                xs[i] = Some((span.0 + step * k as f32, span.0 + step * (k + 1) as f32));
            }
        };
        let mut i = 0;
        while i < n {
            let Some(span) = xs[i] else {
                i += 1;
                continue;
            };
            let mut j = i + 1;
            while j < n && xs[j].is_some_and(|o| (o.0 - span.0).abs() < 0.01 && (o.1 - span.1).abs() < 0.01) {
                j += 1;
            }
            if j - i > 1 {
                share(&mut xs, span, i..j);
            }
            i = j;
        }
        i = 0;
        while i < n {
            if xs[i].is_some() {
                i += 1;
                continue;
            }
            let mut j = i;
            while j < n && xs[j].is_none() {
                j += 1;
            }
            let low = if i > 0 { xs[i - 1].map_or(rect.min_x(), |s| s.1) } else { rect.min_x() };
            let high = if j < n { xs[j].map_or(rect.max_x(), |s| s.0) } else { rect.max_x() };
            share(&mut xs, (low, high.max(low)), i..j);
            i = j;
        }
        let boxes = xs.into_iter().map(|s| s.map_or(Rect::ZERO, |(a, b)| Rect::new(a, rect.y, (b - a).max(0.0), rect.height))).collect();
        GlyphLine { text: text.to_string(), rect, boxes, chars }
    }

    /// The caret offset closest to `x`: before the first character whose middle is right of it.
    fn caret(&self, x: f32) -> usize {
        self.boxes.iter().position(|b| x < b.mid_x()).unwrap_or(self.boxes.len())
    }

    /// The character under `x`, or the nearest one at either end.
    fn char_index(&self, x: f32) -> usize {
        self.boxes.iter().position(|b| x < b.max_x()).unwrap_or(self.boxes.len().saturating_sub(1)).min(self.boxes.len().saturating_sub(1))
    }
}

/// A caret between characters: `offset` characters into line `line`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TextPosition {
    pub line: usize,
    pub offset: usize,
}

/// A selected stretch of text; `anchor` stays where the drag started and `focus` follows the mouse.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TextSpan {
    pub anchor: TextPosition,
    pub focus: TextPosition,
}

impl TextSpan {
    pub fn start(&self) -> TextPosition {
        self.anchor.min(self.focus)
    }
    pub fn end(&self) -> TextPosition {
        self.anchor.max(self.focus)
    }
    pub fn is_empty(&self) -> bool {
        self.anchor == self.focus
    }
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct TextLayout {
    /// Lines in reading order, paragraph by paragraph, so a drag across lines selects what a reader would.
    pub lines: Vec<GlyphLine>,
}

fn is_cjk(c: char) -> bool {
    matches!(c as u32, 0x3040..=0x30FF | 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xAC00..=0xD7AF | 0xF900..=0xFAFF)
}

impl TextLayout {
    pub fn new(lines: Vec<GlyphLine>) -> TextLayout {
        let ocr: Vec<OcrLine> = lines.iter().map(|l| OcrLine { text: l.text.clone(), rect: l.rect }).collect();
        let mut unused: Vec<Option<GlyphLine>> = lines.into_iter().map(Some).collect();
        let mut ordered = Vec::new();
        for block in textblocks::group(&ocr) {
            for line in block.lines {
                if let Some(k) = unused.iter().position(|u| u.as_ref().is_some_and(|g| g.rect == line.rect && g.text == line.text)) {
                    if let Some(g) = unused[k].take() {
                        ordered.push(g);
                    }
                }
            }
        }
        TextLayout { lines: ordered.into_iter().filter(|l| !l.chars.is_empty()).collect() }
    }

    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    /// The caret under `p` when it is on a line (grown by `slop` so thin text is easy to hit), else None.
    pub fn hit_test(&self, p: Point, slop: f32) -> Option<TextPosition> {
        let index = (0..self.lines.len())
            .filter(|i| self.lines[*i].rect.inset(-slop, -slop).contains(p))
            .min_by(|a, b| (self.lines[*a].rect.mid_y() - p.y).abs().total_cmp(&(self.lines[*b].rect.mid_y() - p.y).abs()))?;
        Some(TextPosition { line: index, offset: self.lines[index].caret(p.x) })
    }

    /// The caret nearest to `p` anywhere, for extending a selection while the mouse leaves the text:
    /// the closest line vertically (so past a line's end means its end), then the closest horizontally.
    pub fn nearest(&self, p: Point) -> Option<TextPosition> {
        let distance = |r: &Rect| ((r.min_y() - p.y).max(0.0).max(p.y - r.max_y()), (r.min_x() - p.x).max(0.0).max(p.x - r.max_x()));
        let index = (0..self.lines.len()).min_by(|a, b| {
            let (da, db) = (distance(&self.lines[*a].rect), distance(&self.lines[*b].rect));
            da.0.total_cmp(&db.0).then(da.1.total_cmp(&db.1))
        })?;
        Some(TextPosition { line: index, offset: self.lines[index].caret(p.x) })
    }

    /// The word under `p`: a run of letters and digits, or two characters of a CJK run (a rough stand-in for
    /// dictionary word breaks), or else the single character there.
    pub fn word(&self, p: Point) -> Option<TextSpan> {
        let hit = self.hit_test(p, 3.0)?;
        let line = &self.lines[hit.line];
        let chars = &line.chars;
        let i = line.char_index(p.x);
        let c = *chars.get(i)?;
        let (start, end) = if is_cjk(c) {
            let mut s = i;
            while s > 0 && is_cjk(chars[s - 1]) {
                s -= 1;
            }
            let mut e = i + 1;
            while e < chars.len() && is_cjk(chars[e]) {
                e += 1;
            }
            let pair = s + (i - s) / 2 * 2;
            (pair, (pair + 2).min(e))
        } else if c.is_alphanumeric() || c == '_' {
            let word = |c: char| (c.is_alphanumeric() || c == '_') && !is_cjk(c);
            let mut s = i;
            while s > 0 && word(chars[s - 1]) {
                s -= 1;
            }
            let mut e = i + 1;
            while e < chars.len() && word(chars[e]) {
                e += 1;
            }
            (s, e)
        } else {
            (i, i + 1)
        };
        Some(TextSpan { anchor: TextPosition { line: hit.line, offset: start }, focus: TextPosition { line: hit.line, offset: end } })
    }

    /// The whole line under `p`.
    pub fn line_at(&self, p: Point) -> Option<TextSpan> {
        let hit = self.hit_test(p, 3.0)?;
        let n = self.lines[hit.line].chars.len();
        Some(TextSpan { anchor: TextPosition { line: hit.line, offset: 0 }, focus: TextPosition { line: hit.line, offset: n } })
    }

    fn spans(&self, range: &TextSpan) -> Vec<(usize, std::ops::Range<usize>)> {
        let (start, end) = (range.start(), range.end());
        if range.is_empty() || start.line >= self.lines.len() || end.line >= self.lines.len() {
            return vec![];
        }
        (start.line..=end.line)
            .map(|line| {
                let n = self.lines[line].chars.len();
                let lower = if line == start.line { start.offset.min(n) } else { 0 };
                let upper = if line == end.line { end.offset.min(n) } else { n };
                (line, lower..upper.max(lower))
            })
            .collect()
    }

    /// The highlight for `range`: one rect per line it touches.
    pub fn rects(&self, range: &TextSpan) -> Vec<Rect> {
        self.spans(range)
            .into_iter()
            .filter(|(_, r)| !r.is_empty())
            .map(|(line, r)| self.lines[line].boxes[r].iter().fold(Rect::NULL, |acc, b| acc.union(b)))
            .collect()
    }

    /// The selected text, one line of the picture per line of text.
    pub fn text(&self, range: &TextSpan) -> String {
        self.spans(range).into_iter().map(|(line, r)| self.lines[line].chars[r].iter().collect::<String>()).collect::<Vec<_>>().join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(text: &str, x: f32, y: f32) -> GlyphLine {
        let spans: Vec<Option<(f32, f32)>> = (0..text.chars().count()).map(|n| Some((x + n as f32 * 10.0, x + n as f32 * 10.0 + 10.0))).collect();
        GlyphLine::new(text, Rect::new(x, y, text.chars().count() as f32 * 10.0, 14.0), &spans)
    }

    #[test]
    fn shares_a_word_box_across_its_characters() {
        let word = Some((0.0, 40.0));
        let l = GlyphLine::new("abcd ef", Rect::new(0.0, 0.0, 70.0, 14.0), &[word, word, word, word, None, Some((50.0, 70.0)), Some((50.0, 70.0))]);
        assert_eq!(l.boxes.iter().map(|b| b.x).collect::<Vec<_>>(), vec![0.0, 10.0, 20.0, 30.0, 40.0, 50.0, 60.0]);
        assert_eq!(l.boxes[4].width, 10.0);
    }

    #[test]
    fn missing_boxes_fall_back_to_the_line() {
        let l = GlyphLine::new("abcd", Rect::new(100.0, 5.0, 40.0, 14.0), &[]);
        assert_eq!(l.boxes.iter().map(|b| b.x).collect::<Vec<_>>(), vec![100.0, 110.0, 120.0, 130.0]);
        assert!(l.boxes.iter().all(|b| b.y == 5.0 && b.height == 14.0));
    }

    #[test]
    fn hit_testing_and_dragging_past_a_line_end() {
        let layout = TextLayout::new(vec![line("hello", 0.0, 0.0), line("world", 0.0, 20.0)]);
        assert_eq!(layout.hit_test(Point::new(14.0, 7.0), 3.0), Some(TextPosition { line: 0, offset: 1 }));
        assert_eq!(layout.hit_test(Point::new(16.0, 7.0), 3.0), Some(TextPosition { line: 0, offset: 2 }));
        assert_eq!(layout.hit_test(Point::new(49.0, 27.0), 3.0), Some(TextPosition { line: 1, offset: 5 }));
        assert_eq!(layout.nearest(Point::new(90.0, 7.0)), Some(TextPosition { line: 0, offset: 5 }));
        let long = TextLayout::new(vec![line("a much longer line", 0.0, 0.0), line("short", 0.0, 20.0)]);
        assert_eq!(long.nearest(Point::new(120.0, 27.0)), Some(TextPosition { line: 1, offset: 5 }));
    }

    #[test]
    fn selection_text_in_reading_order() {
        // Given out of order: reading order puts "hello" first.
        let layout = TextLayout::new(vec![line("world", 0.0, 20.0), line("hello", 0.0, 0.0)]);
        let range = TextSpan { anchor: TextPosition { line: 1, offset: 2 }, focus: TextPosition { line: 0, offset: 3 } };
        assert_eq!(layout.text(&range), "lo\nwo");
        assert_eq!(layout.rects(&range), vec![Rect::new(30.0, 0.0, 20.0, 14.0), Rect::new(0.0, 20.0, 20.0, 14.0)]);
        assert_eq!(layout.text(&TextSpan { anchor: range.anchor, focus: range.anchor }), "");
    }

    #[test]
    fn double_click_selects_a_word() {
        let layout = TextLayout::new(vec![line("copy the text", 0.0, 0.0), line("直接选择文字", 0.0, 20.0)]);
        let word = layout.word(Point::new(65.0, 7.0)).unwrap();
        assert_eq!(layout.text(&word), "the");
        let cjk = layout.word(Point::new(25.0, 27.0)).unwrap();
        assert_eq!(layout.text(&cjk), "选择");
        let whole = layout.line_at(Point::new(25.0, 27.0)).unwrap();
        assert_eq!(layout.text(&whole), "直接选择文字");
    }
}
