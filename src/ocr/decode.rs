//! CTC greedy decoding of the recognizer output and per-character positions.

/// One kept CTC token: its class and the time step where it first appeared.
#[derive(Clone, Debug, PartialEq)]
pub struct Token {
    pub class: usize,
    pub step: usize,
    pub prob: f32,
}

/// Greedy CTC over the first `t_valid` of `t` steps of a `t`×`c` row-major probability matrix:
/// argmax per step, collapse repeats, drop blank (class 0).
pub fn ctc_greedy(probs: &[f32], t: usize, c: usize, t_valid: usize) -> Vec<Token> {
    let mut out = Vec::new();
    let mut prev = usize::MAX;
    for step in 0..t_valid.min(t) {
        let Some(row) = probs.get(step * c..(step + 1) * c) else { break };
        let (class, prob) = row
            .iter()
            .enumerate()
            .fold((0usize, f32::NEG_INFINITY), |best, (i, &p)| if p > best.1 { (i, p) } else { best });
        if class != 0 && class != prev {
            out.push(Token { class, step, prob });
        }
        prev = class;
    }
    out
}

/// Text, confidence and per-char x-ranges (in `0..width` units of the unpadded crop) of a line.
#[derive(Clone, Debug, PartialEq)]
pub struct Decoded {
    pub text: String,
    pub score: f32,
    pub char_ranges: Vec<(f32, f32)>,
}

/// `step_px`: crop pixels per time step. Leading and trailing whitespace tokens are dropped.
pub fn assemble(tokens: &[Token], classes: &[String], step_px: f32, width: f32) -> Decoded {
    let is_space = |t: &Token| classes.get(t.class).is_none_or(|s| s.trim().is_empty());
    let start = tokens.iter().position(|t| !is_space(t)).unwrap_or(tokens.len());
    let end = tokens.iter().rposition(|t| !is_space(t)).map_or(start, |i| i + 1);
    let tokens = &tokens[start..end];
    if tokens.is_empty() {
        return Decoded { text: String::new(), score: 0.0, char_ranges: Vec::new() };
    }
    // Confidence over all kept tokens, like RapidOCR (which does not trim).
    let score = tokens.iter().map(|t| t.prob).sum::<f32>() / tokens.len() as f32;
    let centers: Vec<f32> = tokens.iter().map(|t| ((t.step as f32 + 0.5) * step_px).clamp(0.0, width)).collect();
    let n = centers.len();
    // Boundaries halfway between neighbouring token centers; the outer edges mirror the first
    // and last gaps (a lone token gets the whole line).
    let mut edges = Vec::with_capacity(n + 1);
    if n == 1 {
        edges.extend([0.0, width]);
    } else {
        edges.push((centers[0] - (centers[1] - centers[0]) / 2.0).max(0.0));
        for i in 0..n - 1 {
            edges.push((centers[i] + centers[i + 1]) / 2.0);
        }
        edges.push((centers[n - 1] + (centers[n - 1] - centers[n - 2]) / 2.0).min(width));
    }
    let mut text = String::new();
    let mut char_ranges = Vec::new();
    for (i, t) in tokens.iter().enumerate() {
        let s = classes.get(t.class).map_or("", String::as_str);
        let k = s.chars().count().max(1) as f32;
        let (a, b) = (edges[i], edges[i + 1]);
        // A multi-char class (none in the PP-OCR dictionary, but cheap to handle) splits evenly.
        for (j, ch) in s.chars().enumerate() {
            text.push(ch);
            char_ranges.push((a + (b - a) * j as f32 / k, a + (b - a) * (j + 1) as f32 / k));
        }
    }
    Decoded { text, score, char_ranges }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn classes() -> Vec<String> {
        ["blank", "a", "b", "中", " "].iter().map(|s| s.to_string()).collect()
    }

    /// One-hot-ish rows: `ids[i]` gets `p`, the rest share what is left.
    fn probs(ids: &[usize], c: usize, p: f32) -> Vec<f32> {
        let mut out = vec![(1.0 - p) / (c - 1) as f32; ids.len() * c];
        for (i, &id) in ids.iter().enumerate() {
            out[i * c + id] = p;
        }
        out
    }

    #[test]
    fn collapses_repeats_and_blanks() {
        // a a _ a b b _ _ 中 | padding garbage beyond t_valid
        let ids = [1, 1, 0, 1, 2, 2, 0, 0, 3, 1, 2];
        let m = probs(&ids, 5, 0.9);
        let toks = ctc_greedy(&m, ids.len(), 5, 9);
        let got: Vec<(usize, usize)> = toks.iter().map(|t| (t.class, t.step)).collect();
        assert_eq!(got, vec![(1, 0), (1, 3), (2, 4), (3, 8)]);
        let d = assemble(&toks, &classes(), 8.0, 72.0);
        assert_eq!(d.text, "aab中");
        assert!((d.score - 0.9).abs() < 1e-6);
        assert_eq!(d.char_ranges.len(), 4);
        // Centers 4, 28, 36, 68: edges 0, 16, 32, 52, 72.
        assert_eq!(d.char_ranges, vec![(0.0, 16.0), (16.0, 32.0), (32.0, 52.0), (52.0, 72.0)]);
    }

    #[test]
    fn trims_spaces_and_handles_empty() {
        let ids = [4, 0, 1, 4, 2, 0, 4];
        let toks = ctc_greedy(&probs(&ids, 5, 0.8), ids.len(), 5, ids.len());
        let d = assemble(&toks, &classes(), 8.0, 56.0);
        assert_eq!(d.text, "a b");
        assert_eq!(d.char_ranges.len(), 3);
        let blank = ctc_greedy(&probs(&[0, 0, 4], 5, 0.9), 3, 5, 3);
        let d = assemble(&blank, &classes(), 8.0, 24.0);
        assert!(d.text.is_empty() && d.score == 0.0);
        assert!(ctc_greedy(&[], 0, 5, 3).is_empty());
    }

    #[test]
    fn single_char_spans_line() {
        let toks = vec![Token { class: 3, step: 2, prob: 0.7 }];
        let d = assemble(&toks, &classes(), 8.0, 30.0);
        assert_eq!(d.char_ranges, vec![(0.0, 30.0)]);
    }
}
