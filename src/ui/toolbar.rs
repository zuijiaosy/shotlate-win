//! Toolbar buttons and their single-key shortcuts (ToolbarAction / ToolbarKeys in Chrome.swift).

use crate::kit::annotation::Tool;
use crate::kit::settings;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolbarAction {
    Tool(Tool),
    Undo,
    Ocr,
    Translate,
    Pin,
    Cancel,
    Save,
    Done,
}

impl ToolbarAction {
    /// Every button, in toolbar order.
    pub fn all() -> Vec<ToolbarAction> {
        let mut v: Vec<ToolbarAction> = Tool::ALL.iter().map(|t| ToolbarAction::Tool(*t)).collect();
        v.extend([ToolbarAction::Undo, ToolbarAction::Ocr, ToolbarAction::Translate, ToolbarAction::Pin, ToolbarAction::Cancel, ToolbarAction::Save, ToolbarAction::Done]);
        v
    }

    /// Buttons with a single key can have it changed; undo, save, cancel and done keep the shortcuts everyone knows.
    pub fn key_id(self) -> Option<&'static str> {
        match self {
            ToolbarAction::Tool(t) => Some(t.id()),
            ToolbarAction::Ocr => Some("ocr"),
            ToolbarAction::Translate => Some("translate"),
            ToolbarAction::Pin => Some("pin"),
            _ => None,
        }
    }

    /// Shown in the hover card.
    pub fn shortcut(self) -> String {
        if let Some(id) = self.key_id() {
            return key_for(id).to_uppercase();
        }
        match self {
            ToolbarAction::Undo => "Ctrl+Z",
            ToolbarAction::Cancel => "Esc",
            ToolbarAction::Save => "Ctrl+S",
            ToolbarAction::Done => "Enter",
            _ => "",
        }
        .into()
    }

    pub fn title(self) -> &'static str {
        match self {
            ToolbarAction::Tool(t) => t.title(),
            ToolbarAction::Undo => "撤销",
            ToolbarAction::Ocr => "识别文字",
            ToolbarAction::Translate => "翻译到原位",
            ToolbarAction::Pin => "贴到屏幕上",
            ToolbarAction::Cancel => "退出截图",
            ToolbarAction::Save => "保存",
            ToolbarAction::Done => "复制到剪贴板",
        }
    }

    /// An extra line under the shortcut, for what isn't obvious from the title.
    pub fn note(self) -> Option<&'static str> {
        match self {
            ToolbarAction::Ocr => Some("结果可以编辑，再点复制"),
            ToolbarAction::Translate => Some("再按一次切换原文"),
            ToolbarAction::Save => Some("Ctrl+Shift+S 另存为"),
            ToolbarAction::Done => Some("也可以双击选区"),
            _ => None,
        }
    }

    /// Every button that has a single key, in toolbar order.
    pub fn keyed() -> Vec<ToolbarAction> {
        Self::all().into_iter().filter(|a| a.key_id().is_some()).collect()
    }

    /// The button an unmodified key press triggers.
    pub fn for_key(key: &str) -> Option<ToolbarAction> {
        Self::keyed().into_iter().find(|a| a.key_id().map(key_for).as_deref() == Some(key))
    }
}

/// Default single keys: tools 1–7, X OCR, Y translate, T pin.
pub fn default_key(id: &str) -> Option<&'static str> {
    if let Some(t) = Tool::ALL.iter().find(|t| t.id() == id) {
        return Some(t.default_key());
    }
    match id {
        "ocr" => Some("x"),
        "translate" => Some("y"),
        "pin" => Some("t"),
        _ => None,
    }
}

pub fn key_for(id: &str) -> String {
    settings::get().toolbar_keys.get(id).cloned().or_else(|| default_key(id).map(String::from)).unwrap_or_default()
}

/// Letters and digits, typed without modifiers.
pub fn is_allowed(key: &str) -> bool {
    key.chars().count() == 1 && key.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
}

/// Gives `key` to `id`. The button that had it takes `id`'s old key, so no two share one;
/// returns that button's id.
pub fn assign(key: &str, id: &str) -> Option<String> {
    let old = key_for(id);
    if key == old {
        return None;
    }
    let ids: Vec<&'static str> = ToolbarAction::keyed().into_iter().filter_map(|a| a.key_id()).collect();
    let other = ids.iter().find(|other| **other != id && key_for(other) == key).map(|s| s.to_string());
    settings::update(|s| {
        s.toolbar_keys.insert(id.into(), key.into());
        if let Some(other) = &other {
            s.toolbar_keys.insert(other.clone(), old.clone());
        }
    });
    other
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_swap() {
        // Settings are process-wide; this test owns the toolbar keys.
        settings::update(|s| s.toolbar_keys.clear());
        assert_eq!(ToolbarAction::for_key("1"), Some(ToolbarAction::Tool(Tool::Rectangle)));
        assert_eq!(ToolbarAction::for_key("x"), Some(ToolbarAction::Ocr));
        assert_eq!(ToolbarAction::Undo.shortcut(), "Ctrl+Z");
        let other = assign("x", "rectangle");
        assert_eq!(other.as_deref(), Some("ocr"));
        assert_eq!(key_for("rectangle"), "x");
        assert_eq!(key_for("ocr"), "1");
        assert_eq!(ToolbarAction::for_key("1"), Some(ToolbarAction::Ocr));
        assert!(is_allowed("q") && is_allowed("7") && !is_allowed("Q") && !is_allowed("ab"));
        settings::update(|s| s.toolbar_keys.clear());
    }
}
