//! OCR lines grouped into paragraphs, the unit that gets translated (TextBlocks.swift).

use super::geom::Rect;

/// One recognized line, in the capture view's points (top-left origin).
#[derive(Clone, Debug, PartialEq)]
pub struct OcrLine {
    pub text: String,
    pub rect: Rect,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TextBlock {
    pub id: usize,
    pub lines: Vec<OcrLine>,
}

impl TextBlock {
    pub fn rect(&self) -> Rect {
        self.lines.iter().fold(Rect::NULL, |r, l| r.union(&l.rect))
    }

    pub fn line_height(&self) -> f32 {
        if self.lines.is_empty() {
            return 0.0;
        }
        self.lines.iter().map(|l| l.rect.height).sum::<f32>() / self.lines.len() as f32
    }

    /// Lines joined into one string. A trailing hyphen before a lowercase letter is a word break.
    pub fn text(&self) -> String {
        let mut result = String::new();
        for line in &self.lines {
            let t = line.text.trim_matches(|c: char| c == ' ' || c == '\t');
            if result.is_empty() {
                result = t.to_string();
            } else if result.ends_with('-') && t.chars().next().is_some_and(|c| c.is_lowercase()) {
                result.pop();
                result.push_str(t);
            } else {
                result.push(' ');
                result.push_str(t);
            }
        }
        result
    }
}

/// Groups lines into paragraphs: a line joins a block when it starts below the block's last line
/// with a small gap, has a similar height and a roughly aligned left edge.
pub fn group(lines: &[OcrLine]) -> Vec<TextBlock> {
    let mut sorted: Vec<&OcrLine> = lines.iter().collect();
    sorted.sort_by(|a, b| {
        if a.rect.min_y() != b.rect.min_y() {
            a.rect.min_y().total_cmp(&b.rect.min_y())
        } else {
            a.rect.min_x().total_cmp(&b.rect.min_x())
        }
    });
    let mut groups: Vec<Vec<OcrLine>> = Vec::new();
    for line in sorted {
        match groups.iter().rposition(|g| can_merge(&g[g.len() - 1], line)) {
            Some(i) => groups[i].push(line.clone()),
            None => groups.push(vec![line.clone()]),
        }
    }
    groups.into_iter().enumerate().map(|(id, lines)| TextBlock { id, lines }).collect()
}

fn can_merge(upper: &OcrLine, lower: &OcrLine) -> bool {
    let small = upper.rect.height.min(lower.rect.height);
    let large = upper.rect.height.max(lower.rect.height);
    if small <= 0.0 || large / small >= 1.4 {
        return false;
    }
    let gap = lower.rect.min_y() - upper.rect.max_y();
    if gap < -small * 0.3 || gap >= small * 0.8 {
        return false;
    }
    (upper.rect.min_x() - lower.rect.min_x()).abs() < small
}

/// Recognized text in reading order: one line per OCR line, paragraphs kept together.
pub fn plain_text(lines: &[OcrLine]) -> String {
    group(lines).iter().map(|b| b.lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>().join("\n")).collect::<Vec<_>>().join("\n")
}

/// Whether a block is worth sending for translation: it must contain Latin words, must not be mostly
/// CJK already, and must not be a bare URL, path, email or version number.
pub fn should_translate(text: &str) -> bool {
    let t = text.trim();
    let mut latin = 0;
    let mut cjk = 0;
    for c in t.chars() {
        match c as u32 {
            0x41..=0x5A | 0x61..=0x7A => latin += 1,
            0x3040..=0x30FF | 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xAC00..=0xD7AF => cjk += 1,
            _ => {}
        }
    }
    if latin < 2 || cjk >= latin {
        return false;
    }
    !(is_url(t) || is_path(t) || is_email(t) || is_version(t))
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

fn is_url(t: &str) -> bool {
    let lower = t.to_lowercase();
    (lower.starts_with("http://") || lower.starts_with("https://") || lower.starts_with("www.")) && !t.chars().any(char::is_whitespace)
}

/// `^[~/.]?[\w.-]*/[\w./-]*$`
fn is_path(t: &str) -> bool {
    let body = t.strip_prefix(['~', '/', '.']).unwrap_or(t);
    let Some(slash) = body.find('/') else { return false };
    body[..slash].chars().all(|c| is_word_char(c) || c == '.' || c == '-')
        && body[slash + 1..].chars().all(|c| is_word_char(c) || c == '.' || c == '/' || c == '-')
}

/// `^[\w.+-]+@[\w-]+\.[\w.]+$`
fn is_email(t: &str) -> bool {
    let Some((user, domain)) = t.split_once('@') else { return false };
    if user.is_empty() || !user.chars().all(|c| is_word_char(c) || ".+-".contains(c)) {
        return false;
    }
    let Some((host, rest)) = domain.split_once('.') else { return false };
    !host.is_empty() && host.chars().all(|c| is_word_char(c) || c == '-') && !rest.is_empty() && rest.chars().all(|c| is_word_char(c) || c == '.')
}

/// `^v?\d+(\.\d+)+[a-z]?$` (case-insensitive)
fn is_version(t: &str) -> bool {
    let lower = t.to_lowercase();
    let mut s = lower.strip_prefix('v').unwrap_or(&lower);
    if s.ends_with(|c: char| c.is_ascii_lowercase()) {
        s = &s[..s.len() - 1];
    }
    let parts: Vec<&str> = s.split('.').collect();
    parts.len() >= 2 && parts.iter().all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(text: &str, x: f32, y: f32, w: f32, h: f32) -> OcrLine {
        OcrLine { text: text.into(), rect: Rect::new(x, y, w, h) }
    }

    #[test]
    fn groups_paragraphs() {
        let lines = vec![
            line("The quick brown", 10.0, 10.0, 100.0, 12.0),
            line("fox jumps over", 10.0, 24.0, 90.0, 12.0),
            line("Heading", 10.0, 60.0, 80.0, 20.0),
            line("Side note", 300.0, 24.0, 60.0, 12.0),
        ];
        let blocks = group(&lines);
        assert_eq!(blocks.len(), 3);
        assert_eq!(blocks[0].text(), "The quick brown fox jumps over");
        assert_eq!(blocks[0].lines.len(), 2);
    }

    #[test]
    fn joins_hyphenated_words() {
        let b = TextBlock { id: 0, lines: vec![line("transla-", 0.0, 0.0, 10.0, 10.0), line("tion works", 0.0, 12.0, 10.0, 10.0)] };
        assert_eq!(b.text(), "translation works");
    }

    #[test]
    fn translation_filter() {
        assert!(should_translate("Save the file before closing"));
        assert!(!should_translate("保存文件后再关闭 OK"));
        assert!(!should_translate("https://example.com/path"));
        assert!(!should_translate("~/Downloads/file.png"));
        assert!(!should_translate("someone@example.com"));
        assert!(!should_translate("v1.2.3"));
        assert!(!should_translate("42"));
        assert!(should_translate("Version 2 is out"));
    }
}
