//! Output files: timestamped names like the macOS version, never overwriting an existing file.

use std::path::{Path, PathBuf};

use super::image::RgbaImage;
use super::settings::ImageFormat;

/// `Shotlate 2026-09-25 15.30.00.png` for the local time (year, month, day, hour, minute, second).
pub fn default_file_name(format: ImageFormat, t: (u16, u16, u16, u16, u16, u16)) -> String {
    format!("Shotlate {:04}-{:02}-{:02} {:02}.{:02}.{:02}.{}", t.0, t.1, t.2, t.3, t.4, t.5, format.extension())
}

/// `name` in `dir`, or `name 2`, `name 3`… when taken.
pub fn unique_path(dir: &Path, name: &str) -> PathBuf {
    let first = dir.join(name);
    if !first.exists() {
        return first;
    }
    let (base, ext) = match name.rsplit_once('.') {
        Some((b, e)) => (b.to_string(), format!(".{e}")),
        None => (name.to_string(), String::new()),
    };
    (2..).map(|n| dir.join(format!("{base} {n}{ext}"))).find(|p| !p.exists()).unwrap_or(first)
}

pub fn encode(img: &RgbaImage, format: ImageFormat) -> Result<Vec<u8>, String> {
    match format {
        ImageFormat::Png => img.encode_png(),
        ImageFormat::Jpeg => img.encode_jpeg(90),
    }
}

/// Writes into `dir` (created if needed) under a fresh timestamped name; returns the file.
pub fn save(img: &RgbaImage, format: ImageFormat, dir: &Path, time: (u16, u16, u16, u16, u16, u16)) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let path = unique_path(dir, &default_file_name(format, time));
    std::fs::write(&path, encode(img, format)?).map_err(|e| e.to_string())?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_uniqueness() {
        assert_eq!(default_file_name(ImageFormat::Png, (2026, 9, 5, 7, 3, 9)), "Shotlate 2026-09-05 07.03.09.png");
        let dir = std::env::temp_dir().join(format!("shotlate-export-{}", std::process::id()));
        let img = RgbaImage::filled(2, 2, [1, 2, 3, 255]);
        let t = (2026, 1, 1, 0, 0, 0);
        let a = save(&img, ImageFormat::Png, &dir, t).unwrap();
        let b = save(&img, ImageFormat::Png, &dir, t).unwrap();
        assert_ne!(a, b);
        assert!(b.file_name().unwrap().to_string_lossy().ends_with(" 2.png"));
        let j = save(&img, ImageFormat::Jpeg, &dir, t).unwrap();
        assert!(j.extension().unwrap() == "jpg");
        let _ = std::fs::remove_dir_all(dir);
    }
}
