//! Lays translated paragraphs over their originals: colors sampled from the screenshot, font size fitted
//! to each paragraph's box (TranslationLayout in the macOS version).

use std::collections::HashMap;

use super::content::TranslatedBlock;
use super::text::{self, Weight};
use crate::kit::colorsample;
use crate::kit::geom::Rect;
use crate::kit::image::RgbaImage;
use crate::kit::textblocks::TextBlock;

pub const MINIMUM_FONT_SIZE: f32 = 9.0;

/// `crop` holds the pixels of `selection` (points); block rects are in the same point space.
pub fn layout(blocks: &[TextBlock], translations: &HashMap<usize, String>, crop: &RgbaImage, selection: &Rect) -> Vec<TranslatedBlock> {
    if selection.width <= 0.0 {
        return vec![];
    }
    let scale = crop.width as f32 / selection.width;
    blocks
        .iter()
        .filter_map(|block| {
            let text = translations.get(&block.id)?;
            let rect = block.rect();
            let pixel_rect = Rect::new((rect.x - selection.x) * scale, (rect.y - selection.y) * scale, rect.width * scale, rect.height * scale);
            let colors = colorsample::sample(crop, &pixel_rect, ((scale * 2.0) as i64).max(2));
            // Bold stems are clearly thicker relative to the line height than regular ones.
            let stroke_ratio = colors.stroke_width / (block.line_height() * scale).max(1.0) as f64;
            let bold = stroke_ratio > 0.17;
            let (font_size, height) = fit(text, &rect, block.line_height(), bold);
            let mut draw_rect = rect;
            draw_rect.height = rect.height.max(height);
            Some(TranslatedBlock {
                rect: draw_rect,
                text: text.clone(),
                font_size,
                bold,
                centered: colors.is_centered(),
                background: colors.background.color(),
                foreground: colors.foreground.color(),
            })
        })
        .collect()
}

/// Starts near the original text size and shrinks by 0.5pt until the text fits the box, stopping at the
/// minimum size; the returned height may then exceed the box.
pub fn fit(s: &str, rect: &Rect, line_height: f32, bold: bool) -> (f32, f32) {
    let weight = if bold { Weight::Bold } else { Weight::Regular };
    let mut size = (line_height * 0.85).min(64.0).max(MINIMUM_FONT_SIZE);
    loop {
        let (_, height) = text::measure(s, size, weight, Some(rect.width));
        if height <= rect.height + line_height * 0.25 || size <= MINIMUM_FONT_SIZE {
            return (size, height.ceil());
        }
        size -= 0.5;
    }
}
