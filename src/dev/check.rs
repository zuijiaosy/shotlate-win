//! `--check [name|all] [out-dir]`: self-checks that print PASS / FAIL / SKIP per check and exit non-zero on
//! any failure. The capture checks drive the real overlay state machine offscreen; on Windows the platform
//! pieces (screen grab, clipboard, DPAPI, hotkeys) are exercised too. Like the macOS `--check`.

use std::path::{Path, PathBuf};

enum Outcome {
    Pass,
    Fail(String),
    Skip(String),
}

type Check = fn(&Path) -> Outcome;

fn checks() -> Vec<(&'static str, Check)> {
    #[cfg_attr(not(windows), allow(unused_mut))]
    let mut v: Vec<(&'static str, Check)> = vec![
        ("fonts", fonts),
        ("capture", capture),
        ("export", export),
        ("settings", settings_roundtrip),
        ("ocr", ocr),
        ("translate-network", translate_network),
    ];
    #[cfg(windows)]
    v.extend(win_checks::all());
    v
}

pub fn run(name: &str, out: &Path) -> i32 {
    let _ = std::fs::create_dir_all(out);
    let mut failed = 0;
    let mut ran = 0;
    for (n, check) in checks() {
        if name != "all" && name != n {
            continue;
        }
        ran += 1;
        match check(out) {
            Outcome::Pass => println!("PASS {n}"),
            Outcome::Fail(why) => {
                failed += 1;
                println!("FAIL {n}: {why}");
            }
            Outcome::Skip(why) => println!("SKIP {n}: {why}"),
        }
    }
    if ran == 0 {
        eprintln!("no check named {name}; available: {}", checks().iter().map(|c| c.0).collect::<Vec<_>>().join(", "));
        return 2;
    }
    if failed > 0 { 1 } else { 0 }
}

fn fonts(_: &Path) -> Outcome {
    use crate::render::text::{self, Weight};
    if !text::fonts_available() {
        return Outcome::Fail("no system font could be loaded".into());
    }
    for (s, weight) in [("截图 Shotlate 1234", Weight::Regular), ("翻译到原位", Weight::Bold), ("日本語 한국어", Weight::Regular)] {
        let l = text::layout(s, 16.0, weight, None);
        if l.width <= 0.0 || l.path((0.0, 0.0)).is_none() {
            return Outcome::Fail(format!("can't draw {s:?}"));
        }
    }
    Outcome::Pass
}

fn capture(out: &Path) -> Outcome {
    // The scripted demo checks each step (selection, tools, card, OCR/translation flows, vertical toolbar).
    if super::demo::run(None, &out.join("ui-demo"), 1.0) { Outcome::Pass } else { Outcome::Fail("a UI demo step failed (see above)".into()) }
}

fn export(out: &Path) -> Outcome {
    use crate::kit::export;
    use crate::kit::image::RgbaImage;
    use crate::kit::settings::ImageFormat;
    let img = RgbaImage::filled(40, 30, [200, 30, 30, 255]);
    let dir = out.join("export");
    for format in [ImageFormat::Png, ImageFormat::Jpeg] {
        match export::save(&img, format, &dir, (2026, 1, 2, 3, 4, 5)) {
            Ok(p) if p.exists() => {}
            Ok(p) => return Outcome::Fail(format!("{} missing", p.display())),
            Err(e) => return Outcome::Fail(e),
        }
    }
    Outcome::Pass
}

fn settings_roundtrip(out: &Path) -> Outcome {
    use crate::kit::settings::Settings;
    let mut s = Settings::default();
    s.toolbar_keys.insert("ocr".into(), "q".into());
    let path = out.join("settings.json");
    let written = serde_json::to_vec_pretty(&s).map_err(|e| e.to_string()).and_then(|b| std::fs::write(&path, b).map_err(|e| e.to_string()));
    if let Err(e) = written {
        return Outcome::Fail(e);
    }
    match std::fs::read(&path).ok().and_then(|b| serde_json::from_slice::<Settings>(&b).ok()) {
        Some(back) if back == s => Outcome::Pass,
        _ => Outcome::Fail("settings didn't round-trip".into()),
    }
}

/// A real HTTPS request to the translation API with a fake key: must come back as "401", not crash
/// (a TLS setup that panics takes the whole app down in a release build).
fn translate_network(_: &Path) -> Outcome {
    use crate::kit::translator::{self, Item, TranslationConfig, TranslationError};
    let config = TranslationConfig {
        engine: crate::kit::translator::TranslationEngine::Llm,
        client_key: String::new(),
        base_url: translator::DEFAULT_BASE_URL.into(),
        model: translator::DEFAULT_MODEL.into(),
        api_key: "sk-shotlate-self-check".into(),
        target_language: translator::DEFAULT_TARGET_LANGUAGE.into(),
        timeout: std::time::Duration::from_secs(20),
    };
    match translator::translate(&[Item { id: 0, text: "Hello".into() }], &config) {
        Err(TranslationError::Http { status: 401, .. }) => Outcome::Pass,
        Err(TranslationError::Network(e)) => Outcome::Skip(format!("no network: {e}")),
        other => Outcome::Fail(format!("expected a 401, got {other:?}")),
    }
}

fn ocr(_: &Path) -> Outcome {
    let dir = PathBuf::from(std::env::var("SHOTLATE_MODELS").unwrap_or_else(|_| crate::app_paths::models_dir().display().to_string()));
    if !crate::ocr::models::is_ready(&dir) {
        return Outcome::Skip(format!("models not downloaded ({})", dir.display()));
    }
    // Embedded, so the check also runs where the repository isn't (a VM, CI artifacts).
    let Ok(img) = crate::kit::image::RgbaImage::decode_png(include_bytes!("../../tests/data/mixed.png")) else {
        return Outcome::Fail("embedded test image didn't decode".into());
    };
    let engine = match crate::ocr::OcrEngine::load(&dir) {
        Ok(e) => e,
        Err(e) => return Outcome::Fail(e.to_string()),
    };
    match engine.recognize(&img) {
        Ok(lines) if lines.iter().any(|l| l.text.contains("quick brown fox")) && lines.iter().any(|l| l.text.contains("截图")) => Outcome::Pass,
        Ok(lines) => Outcome::Fail(format!("unexpected text: {:?}", lines.iter().map(|l| &l.text).collect::<Vec<_>>())),
        Err(e) => Outcome::Fail(e.to_string()),
    }
}

#[cfg(windows)]
mod win_checks {
    use super::{Check, Outcome};
    use crate::win::{clipboard, screen, secret, util};
    use std::path::Path;
    use windows::Win32::System::DataExchange::{CloseClipboard, IsClipboardFormatAvailable};
    use windows::Win32::System::Ole::CF_DIB;
    use windows::Win32::UI::Input::KeyboardAndMouse::{MOD_ALT, MOD_CONTROL, MOD_SHIFT, RegisterHotKey, UnregisterHotKey, VK_F12};

    pub fn all() -> Vec<(&'static str, Check)> {
        vec![("screen", screen_grab), ("clipboard", clipboard_image), ("dpapi", dpapi), ("hotkey", hotkey)]
    }

    fn screen_grab(out: &Path) -> Outcome {
        let monitors = screen::monitors();
        let Some(m) = monitors.first() else { return Outcome::Skip("no monitor (headless session)".into()) };
        match screen::grab(&m.rect) {
            Some(pix) => {
                let _ = pix.save_png(out.join("screen.png"));
                if pix.width() as i32 == m.rect.right - m.rect.left { Outcome::Pass } else { Outcome::Fail("wrong size".into()) }
            }
            None => Outcome::Fail(format!("BitBlt failed for {}", m.name)),
        }
    }

    fn clipboard_image(_: &Path) -> Outcome {
        let img = crate::kit::image::RgbaImage::filled(8, 8, [10, 200, 10, 255]);
        if !clipboard::set_image(util::app_window(), &img) {
            return Outcome::Fail("couldn't write the clipboard".into());
        }
        let ok = unsafe {
            if !clipboard::open(None) {
                return Outcome::Fail("couldn't open the clipboard".into());
            }
            let ok = IsClipboardFormatAvailable(CF_DIB.0 as u32).is_ok();
            let _ = CloseClipboard();
            ok
        };
        if ok { Outcome::Pass } else { Outcome::Fail("no CF_DIB on the clipboard".into()) }
    }

    fn dpapi(_: &Path) -> Outcome {
        match secret::roundtrip_check("sk-test-密钥") {
            true => Outcome::Pass,
            false => Outcome::Fail("DPAPI protect/unprotect didn't round-trip".into()),
        }
    }

    fn hotkey(_: &Path) -> Outcome {
        // An unlikely combination, registered and released at once.
        let ok = unsafe { RegisterHotKey(None, 0xBEEF, MOD_CONTROL | MOD_ALT | MOD_SHIFT, VK_F12.0 as u32) };
        match ok {
            Ok(()) => {
                unsafe {
                    let _ = UnregisterHotKey(None, 0xBEEF);
                }
                Outcome::Pass
            }
            Err(e) => Outcome::Skip(format!("Ctrl+Alt+Shift+F12 is taken here ({e})")),
        }
    }
}
