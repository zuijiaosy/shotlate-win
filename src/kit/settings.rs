//! App settings in `%APPDATA%\Shotlate\settings.json`; every change is written through right away.
//! The API key is not here: it has its own encrypted file (see win::secret).
//! Without `init` (tests, dev tools) settings live in memory only and start from the defaults,
//! so scripted runs never touch the real configuration.

use std::collections::HashMap;
use std::path::PathBuf;
#[cfg(not(test))]
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use super::annotation::{ItemStyle, MosaicEffect, MosaicMode, Tool};
use super::color::{Color, PALETTE};
use super::translator::{DEFAULT_BASE_URL, DEFAULT_MODEL, DEFAULT_TARGET_LANGUAGE};

/// RegisterHotKey modifier bits.
pub const MOD_ALT: u32 = 0x1;
pub const MOD_CONTROL: u32 = 0x2;
pub const MOD_SHIFT: u32 = 0x4;
pub const MOD_WIN: u32 = 0x8;

/// A global shortcut: Windows virtual-key code plus RegisterHotKey modifiers.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Shortcut {
    pub vk: u32,
    pub modifiers: u32,
}

impl Shortcut {
    pub const fn new(vk: u32, modifiers: u32) -> Shortcut {
        Shortcut { vk, modifiers }
    }

    /// Alt+Shift+A: close to ⌥A on the Mac, while Alt+A is WeChat's and Ctrl+Alt+A QQ's screenshot key.
    pub const CAPTURE: Shortcut = Shortcut::new(0x41, MOD_ALT | MOD_SHIFT);
    pub const TOGGLE_PINS: Shortcut = Shortcut::new(0x48, MOD_ALT | MOD_SHIFT);

    /// `Ctrl+Alt+Shift+A`, the way Windows writes shortcuts.
    pub fn display(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        if self.modifiers & MOD_CONTROL != 0 {
            parts.push("Ctrl".into());
        }
        if self.modifiers & MOD_ALT != 0 {
            parts.push("Alt".into());
        }
        if self.modifiers & MOD_SHIFT != 0 {
            parts.push("Shift".into());
        }
        if self.modifiers & MOD_WIN != 0 {
            parts.push("Win".into());
        }
        parts.push(key_name(self.vk));
        parts.join("+")
    }
}

/// Human name of a virtual-key code.
pub fn key_name(vk: u32) -> String {
    match vk {
        0x30..=0x39 | 0x41..=0x5A => char::from_u32(vk).map(String::from).unwrap_or_default(),
        0x70..=0x87 => format!("F{}", vk - 0x6F),
        0x20 => "Space".into(),
        0x2C => "PrtScn".into(),
        0x60..=0x69 => format!("Num{}", vk - 0x60),
        0xBA => ";".into(),
        0xBB => "=".into(),
        0xBC => ",".into(),
        0xBD => "-".into(),
        0xBE => ".".into(),
        0xBF => "/".into(),
        0xC0 => "`".into(),
        0xDB => "[".into(),
        0xDC => "\\".into(),
        0xDD => "]".into(),
        0xDE => "'".into(),
        0x21 => "PageUp".into(),
        0x22 => "PageDown".into(),
        0x23 => "End".into(),
        0x24 => "Home".into(),
        0x2D => "Insert".into(),
        0x2E => "Delete".into(),
        _ => format!("Key{vk}"),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ImageFormat {
    #[default]
    Png,
    Jpeg,
}

impl ImageFormat {
    pub fn extension(self) -> &'static str {
        match self {
            ImageFormat::Png => "png",
            ImageFormat::Jpeg => "jpg",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Settings {
    pub capture_shortcut: Shortcut,
    /// None: the user cleared it.
    pub toggle_pins_shortcut: Option<Shortcut>,
    /// None: the Downloads folder.
    pub save_directory: Option<String>,
    pub image_format: ImageFormat,
    pub base_url: String,
    pub model: String,
    pub target_language: String,
    /// Toolbar single-key shortcuts the user changed; the rest keep their defaults.
    pub toolbar_keys: HashMap<String, String>,
    pub style_colors: HashMap<String, Color>,
    pub style_sizes: HashMap<String, f32>,
    pub style_options: HashMap<String, ItemStyle>,
    pub mosaic_mode: MosaicMode,
    pub mosaic_effect: MosaicEffect,
    /// The selection magnifier shows #RRGGBB (true) or "r, g, b".
    pub hex_color: bool,
    /// Whether the first-launch download prompt for the recognition models was shown.
    pub models_prompted: bool,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings {
            capture_shortcut: Shortcut::CAPTURE,
            toggle_pins_shortcut: Some(Shortcut::TOGGLE_PINS),
            save_directory: None,
            image_format: ImageFormat::Png,
            base_url: DEFAULT_BASE_URL.into(),
            model: DEFAULT_MODEL.into(),
            target_language: DEFAULT_TARGET_LANGUAGE.into(),
            toolbar_keys: HashMap::new(),
            style_colors: HashMap::new(),
            style_sizes: HashMap::new(),
            style_options: HashMap::new(),
            mosaic_mode: MosaicMode::Brush,
            mosaic_effect: MosaicEffect::Pixelate,
            hex_color: true,
            models_prompted: false,
        }
    }
}

struct Store {
    settings: Settings,
    path: Option<PathBuf>,
}

#[cfg(not(test))]
static STORE: Mutex<Option<Store>> = Mutex::new(None);

#[cfg(not(test))]
fn with_store<R>(f: impl FnOnce(&mut Store) -> R) -> R {
    let mut guard = STORE.lock().unwrap_or_else(|e| e.into_inner());
    let store = guard.get_or_insert_with(|| Store { settings: Settings::default(), path: None });
    f(store)
}

// Tests run in parallel threads; each gets its own settings so style memory and keys don't leak between them.
#[cfg(test)]
thread_local! {
    static STORE: std::cell::RefCell<Option<Store>> = const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
fn with_store<R>(f: impl FnOnce(&mut Store) -> R) -> R {
    STORE.with(|s| {
        let mut guard = s.borrow_mut();
        let store = guard.get_or_insert_with(|| Store { settings: Settings::default(), path: None });
        f(store)
    })
}

/// Loads `path` (a missing or broken file gives the defaults) and writes every later change to it.
pub fn init(path: PathBuf) {
    let settings = std::fs::read(&path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    with_store(|s| {
        s.settings = settings;
        s.path = Some(path);
    });
}

pub fn get() -> Settings {
    with_store(|s| s.settings.clone())
}

/// Changes the settings and saves them at once. Returns what `f` returned.
pub fn update<R>(f: impl FnOnce(&mut Settings) -> R) -> R {
    with_store(|s| {
        let before = s.settings.clone();
        let r = f(&mut s.settings);
        if s.settings != before {
            if let Some(path) = &s.path {
                save(path, &s.settings);
            }
        }
        r
    })
}

fn save(path: &PathBuf, settings: &Settings) {
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(bytes) = serde_json::to_vec_pretty(settings) {
        // Write then rename, so a crash mid-write never leaves a truncated file.
        let tmp = path.with_extension("json.tmp");
        if std::fs::write(&tmp, bytes).is_ok() {
            let _ = std::fs::rename(&tmp, path);
        }
    }
}

// Style memory: color, size and options per tool, remembered across captures and launches.

pub fn color_for(tool: Tool) -> Color {
    with_store(|s| s.settings.style_colors.get(tool.id()).copied()).unwrap_or(PALETTE[0])
}

pub fn size_for(tool: Tool) -> f32 {
    with_store(|s| s.settings.style_sizes.get(tool.id()).copied()).unwrap_or_else(|| tool.default_size())
}

pub fn style_for(tool: Tool) -> ItemStyle {
    with_store(|s| s.settings.style_options.get(tool.id()).copied()).unwrap_or_default()
}

pub fn set_color(tool: Tool, color: Color) {
    update(|s| {
        s.style_colors.insert(tool.id().into(), color);
    });
}

pub fn set_size(tool: Tool, size: f32) {
    update(|s| {
        s.style_sizes.insert(tool.id().into(), size);
    });
}

pub fn set_style(tool: Tool, style: ItemStyle) {
    update(|s| {
        s.style_options.insert(tool.id().into(), style);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shortcut_display() {
        assert_eq!(Shortcut::CAPTURE.display(), "Alt+Shift+A");
        assert_eq!(Shortcut::new(0x70, MOD_CONTROL).display(), "Ctrl+F1");
    }

    #[test]
    fn missing_fields_take_defaults() {
        let s: Settings = serde_json::from_str(r#"{"imageFormat":"jpeg","hexColor":false}"#).unwrap();
        assert_eq!(s.image_format, ImageFormat::Jpeg);
        assert!(!s.hex_color);
        assert_eq!(s.capture_shortcut, Shortcut::CAPTURE);
        assert_eq!(s.model, DEFAULT_MODEL);
    }

    #[test]
    fn round_trips_through_a_file() {
        let dir = std::env::temp_dir().join(format!("shotlate-settings-{}", std::process::id()));
        let path = dir.join("settings.json");
        let mut s = Settings::default();
        s.toolbar_keys.insert("ocr".into(), "q".into());
        s.style_colors.insert("arrow".into(), PALETTE[3]);
        save(&path, &s);
        let back: Settings = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(back, s);
        let _ = std::fs::remove_dir_all(dir);
    }
}
