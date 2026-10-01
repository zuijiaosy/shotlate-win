//! The Windows app: tray, hotkeys, capture overlays, pins, settings. Everything that isn't Win32 plumbing
//! lives in `kit`, `render` and `ui`, where it is tested on any OS.

pub mod app;
pub mod autostart;
pub mod clipboard;
pub mod dialogs;
pub mod e2e;
pub mod hud;
pub mod layered;
pub mod overlay;
pub mod pin;
pub mod screen;
pub mod secret;
pub mod settings_window;
pub mod updater;
pub mod util;
pub mod scroll;
