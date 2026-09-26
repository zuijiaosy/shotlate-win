//! sRGB colors with straight alpha, 0–1 components.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Color {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl Color {
    pub const fn rgb(r: f32, g: f32, b: f32) -> Color {
        Color { r, g, b, a: 1.0 }
    }

    pub const fn rgba(r: f32, g: f32, b: f32, a: f32) -> Color {
        Color { r, g, b, a }
    }

    pub const fn white(a: f32) -> Color {
        Color::rgba(1.0, 1.0, 1.0, a)
    }

    pub const fn black(a: f32) -> Color {
        Color::rgba(0.0, 0.0, 0.0, a)
    }

    pub const fn gray(level: f32, a: f32) -> Color {
        Color::rgba(level, level, level, a)
    }

    pub fn from_rgb8(r: u8, g: u8, b: u8) -> Color {
        Color::rgb(r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0)
    }

    pub fn with_alpha(self, a: f32) -> Color {
        Color { a, ..self }
    }

    pub fn to_rgb8(self) -> (u8, u8, u8) {
        let c = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        (c(self.r), c(self.g), c(self.b))
    }

    /// Relative luminance without gamma correction; good enough to pick black or white text.
    pub fn luminance(self) -> f32 {
        0.2126 * self.r + 0.7152 * self.g + 0.0722 * self.b
    }

    /// Black or white, whichever reads on this color.
    pub fn contrasting_text(self) -> Color {
        if self.luminance() > 0.6 { Color::black(1.0) } else { Color::white(1.0) }
    }

    pub fn is_approximately(self, other: Color) -> bool {
        (self.r - other.r).abs() < 0.01 && (self.g - other.g).abs() < 0.01 && (self.b - other.b).abs() < 0.01
    }

    pub fn to_skia(self) -> tiny_skia::Color {
        tiny_skia::Color::from_rgba(self.r.clamp(0.0, 1.0), self.g.clamp(0.0, 1.0), self.b.clamp(0.0, 1.0), self.a.clamp(0.0, 1.0))
            .unwrap_or(tiny_skia::Color::BLACK)
    }
}

/// Selection blue, matching the macOS version (and iShot's selection frame).
pub const SELECTION_BLUE: Color = Color::rgb(0.16, 0.58, 0.93);

/// The annotation colors offered in the style bar; the first is the default.
pub const PALETTE: [Color; 8] = [
    Color::rgb(0.96, 0.23, 0.19), // red
    Color::rgb(1.00, 0.55, 0.10), // orange
    Color::rgb(1.00, 0.80, 0.10), // yellow
    Color::rgb(0.20, 0.78, 0.35), // green
    Color::rgb(0.12, 0.56, 0.95), // blue
    Color::rgb(0.62, 0.32, 0.95), // purple
    Color::rgb(1.0, 1.0, 1.0),
    Color::rgb(0.0, 0.0, 0.0),
];
