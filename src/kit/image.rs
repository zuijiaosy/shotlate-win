//! Plain RGBA8 images (straight alpha, top-left origin) and PNG / JPEG encoding.

use std::io::Cursor;

#[derive(Clone, PartialEq, Eq)]
pub struct RgbaImage {
    pub width: u32,
    pub height: u32,
    /// `width * height * 4` bytes, rows top to bottom.
    pub data: Vec<u8>,
}

impl std::fmt::Debug for RgbaImage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "RgbaImage({}x{})", self.width, self.height)
    }
}

impl RgbaImage {
    pub fn thumbnail(&self, max_width: u32, max_height: u32) -> Option<RgbaImage> {
        if self.width == 0 || self.height == 0 || max_width == 0 || max_height == 0 { return None; }
        let scale = (max_width as f64 / self.width as f64).min(max_height as f64 / self.height as f64).min(1.0);
        let width = (self.width as f64 * scale).round().max(1.0) as u32;
        let height = (self.height as f64 * scale).round().max(1.0) as u32;
        let mut data = Vec::new();
        data.try_reserve_exact(width as usize * height as usize * 4).ok()?;
        for y in 0..height { for x in 0..width {
            let sx = (x as u64 * self.width as u64 / width as u64) as u32;
            let sy = (y as u64 * self.height as u64 / height as u64) as u32;
            data.extend_from_slice(&self.pixel(sx, sy));
        } }
        Some(RgbaImage { width, height, data })
    }

    pub fn filled(width: u32, height: u32, rgba: [u8; 4]) -> RgbaImage {
        let mut data = Vec::with_capacity(width as usize * height as usize * 4);
        for _ in 0..(width as usize * height as usize) {
            data.extend_from_slice(&rgba);
        }
        RgbaImage { width, height, data }
    }

    pub fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        let i = (y as usize * self.width as usize + x as usize) * 4;
        [self.data[i], self.data[i + 1], self.data[i + 2], self.data[i + 3]]
    }

    /// The pixels of `x, y, w, h`, clamped to the image. Returns None when nothing is left.
    pub fn crop(&self, x: i64, y: i64, w: i64, h: i64) -> Option<RgbaImage> {
        let x0 = x.clamp(0, self.width as i64);
        let y0 = y.clamp(0, self.height as i64);
        let x1 = (x + w).clamp(0, self.width as i64);
        let y1 = (y + h).clamp(0, self.height as i64);
        if x1 <= x0 || y1 <= y0 {
            return None;
        }
        let (cw, ch) = ((x1 - x0) as usize, (y1 - y0) as usize);
        let mut data = Vec::with_capacity(cw * ch * 4);
        let stride = self.width as usize * 4;
        for row in y0 as usize..y1 as usize {
            let start = row * stride + x0 as usize * 4;
            data.extend_from_slice(&self.data[start..start + cw * 4]);
        }
        Some(RgbaImage { width: cw as u32, height: ch as u32, data })
    }

    pub fn encode_png(&self) -> Result<Vec<u8>, String> {
        let mut out = Vec::new();
        {
            let mut encoder = png::Encoder::new(Cursor::new(&mut out), self.width, self.height);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            // Screenshots are large and mostly flat: fast compression is nearly as small and much quicker.
            encoder.set_compression(png::Compression::Fast);
            let mut writer = encoder.write_header().map_err(|e| e.to_string())?;
            writer.write_image_data(&self.data).map_err(|e| e.to_string())?;
        }
        Ok(out)
    }

    pub fn encode_jpeg(&self, quality: u8) -> Result<Vec<u8>, String> {
        let mut out = Vec::new();
        let encoder = jpeg_encoder::Encoder::new(&mut out, quality);
        let rgb: Vec<u8> = self.data.chunks_exact(4).flat_map(|p| [p[0], p[1], p[2]]).collect();
        encoder
            .encode(&rgb, self.width as u16, self.height as u16, jpeg_encoder::ColorType::Rgb)
            .map_err(|e| e.to_string())?;
        Ok(out)
    }

    pub fn decode_png(bytes: &[u8]) -> Result<RgbaImage, String> {
        let decoder = png::Decoder::new(Cursor::new(bytes));
        let mut reader = decoder.read_info().map_err(|e| e.to_string())?;
        let size = reader.output_buffer_size().ok_or("PNG too large")?;
        let mut buf = vec![0; size];
        let info = reader.next_frame(&mut buf).map_err(|e| e.to_string())?;
        buf.truncate(info.buffer_size());
        let (w, h) = (info.width, info.height);
        let data = match (info.color_type, info.bit_depth) {
            (png::ColorType::Rgba, png::BitDepth::Eight) => buf,
            (png::ColorType::Rgb, png::BitDepth::Eight) => buf.chunks_exact(3).flat_map(|p| [p[0], p[1], p[2], 255]).collect(),
            (png::ColorType::Grayscale, png::BitDepth::Eight) => buf.iter().flat_map(|&g| [g, g, g, 255]).collect(),
            (png::ColorType::GrayscaleAlpha, png::BitDepth::Eight) => buf.chunks_exact(2).flat_map(|p| [p[0], p[0], p[0], p[1]]).collect(),
            (ct, bd) => return Err(format!("unsupported PNG format {ct:?} {bd:?}")),
        };
        Ok(RgbaImage { width: w, height: h, data })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn png_round_trip() {
        let mut img = RgbaImage::filled(3, 2, [10, 20, 30, 255]);
        img.data[4..8].copy_from_slice(&[200, 100, 50, 255]);
        let back = RgbaImage::decode_png(&img.encode_png().unwrap()).unwrap();
        assert_eq!(back, img);
    }

    #[test] fn thumbnail_is_bounded_and_preserves_source() {
        let image = RgbaImage::filled(40, 600, [10, 20, 30, 255]);
        let preview = image.thumbnail(20, 60).unwrap();
        assert_eq!((preview.width, preview.height), (4, 60));
        assert_eq!(preview.pixel(0, 0), [10, 20, 30, 255]);
        assert_eq!(image.data.len(), 40 * 600 * 4);
        assert!(image.thumbnail(0, 60).is_none());
    }

    #[test]
    fn crop_clamps() {
        let img = RgbaImage::filled(10, 10, [1, 2, 3, 255]);
        let c = img.crop(-5, 8, 10, 10).unwrap();
        assert_eq!((c.width, c.height), (5, 2));
        assert!(img.crop(20, 0, 5, 5).is_none());
    }

    #[test]
    fn jpeg_encodes() {
        let img = RgbaImage::filled(16, 16, [255, 0, 0, 255]);
        let bytes = img.encode_jpeg(90).unwrap();
        assert_eq!(&bytes[..2], &[0xFF, 0xD8]);
    }
}
