//! Where settings, the API key and the recognition models live.
//! Windows: settings and key in %APPDATA%\Shotlate (roams), models in %LOCALAPPDATA%\Shotlate\models (large,
//! machine-specific). Elsewhere (dev tools and tests) a separate folder, never the real app's files.

use std::path::PathBuf;

fn env_dir(var: &str) -> Option<PathBuf> {
    std::env::var_os(var).map(PathBuf::from).filter(|p| p.is_absolute())
}

pub fn config_dir() -> PathBuf {
    if cfg!(windows) {
        env_dir("APPDATA").unwrap_or_else(std::env::temp_dir).join("Shotlate")
    } else {
        env_dir("HOME").unwrap_or_else(std::env::temp_dir).join(".shotlate-dev")
    }
}

pub fn models_dir() -> PathBuf {
    if cfg!(windows) {
        env_dir("LOCALAPPDATA").unwrap_or_else(config_dir).join("Shotlate").join("models")
    } else {
        config_dir().join("models")
    }
}

pub fn settings_file() -> PathBuf {
    config_dir().join("settings.json")
}

pub fn api_key_file() -> PathBuf {
    config_dir().join("api-key")
}
