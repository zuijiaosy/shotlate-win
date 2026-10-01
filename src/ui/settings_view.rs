//! The settings window, drawn like the capture toolbar: a menu on the left (快捷键 / 保存 / 翻译 / 通用),
//! grouped rounded cards on the right. Platform-independent: the platform feeds mouse and key events, draws
//! what `render` produces, places real text boxes over `fields()`, and carries out the `SettingsEffect`s.
//! There is no save button; every change is an effect the platform writes through at once.

use tiny_skia::{Pixmap, Transform};

use super::chrome::Theme;
use crate::kit::color::{Color, SELECTION_BLUE};
use crate::kit::geom::{Point, Rect, Size};
use crate::kit::settings::{ImageFormat, MOD_ALT, MOD_CONTROL, MOD_SHIFT, MOD_WIN, Shortcut};
use crate::kit::translator::{LANGUAGES, TranslationEngine};
use crate::render::canvas::Canvas;
use crate::render::text::{self, Weight};

pub const WINDOW: Size = Size::new(720.0, 500.0);
const SIDEBAR: f32 = 184.0;
const CONTENT_X: f32 = 204.0;
const CONTENT_W: f32 = 496.0;
const ROW: f32 = 46.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pane {
    Shortcuts,
    Save,
    Translate,
    General,
}

impl Pane {
    pub const ALL: [Pane; 4] = [Pane::Shortcuts, Pane::Save, Pane::Translate, Pane::General];

    pub fn title(self) -> &'static str {
        match self {
            Pane::Shortcuts => "快捷键",
            Pane::Save => "保存",
            Pane::Translate => "翻译",
            Pane::General => "通用",
        }
    }
}

/// Text boxes the platform provides (real edit controls, for IME, selection and paste).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    ApiKey,
    BaseUrl,
    Model,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HotkeyTarget {
    Capture,
    TogglePins,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Models {
    Ready,
    Missing,
    /// 0–1.
    Downloading(f32),
}

/// What the settings show; the platform fills it from the stored settings and refreshes it.
#[derive(Clone, Debug, PartialEq)]
pub struct SettingsState {
    pub engine: TranslationEngine,
    pub api_key_present: bool,
    pub capture: Shortcut,
    pub toggle_pins: Option<Shortcut>,
    pub capture_ok: bool,
    pub toggle_ok: bool,
    pub save_dir: String,
    pub format: ImageFormat,
    pub language: String,
    pub testing: bool,
    /// (succeeded, message)
    pub test_result: Option<(bool, String)>,
    pub login: bool,
    /// None: automatic updates aren't available in this build.
    pub auto_update: Option<bool>,
    pub models: Models,
    pub models_mb: f32,
    pub models_dir: String,
    pub version: String,
}

#[derive(Clone, Debug, PartialEq)]
pub enum SettingsEffect {
    SetEngine(TranslationEngine),
    SetCapture(Shortcut),
    SetTogglePins(Option<Shortcut>),
    /// A shortcut is being recorded: the global ones must not fire meanwhile.
    PauseHotkeys,
    ResumeHotkeys,
    ChooseFolder,
    SetFormat(ImageFormat),
    SetLanguage(String),
    OpenUrl(String),
    TestConnection,
    ResetTranslation,
    SetLogin(bool),
    SetAutoUpdate(bool),
    /// Download, cancel or re-download the recognition models, depending on `Models`.
    ModelsButton,
    /// The visible text boxes changed (pane switch): place them at `fields()`.
    FieldsChanged,
    Beep,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Hit {
    Engine(usize),
    Pane(Pane),
    Recorder(HotkeyTarget),
    ClearPins,
    ChooseFolder,
    Format(ImageFormat),
    Language(usize),
    Link,
    Test,
    Reset,
    Login,
    AutoUpdate,
    Models,
}

enum Elem {
    Title(String, Point),
    Card(Rect),
    Divider(Rect),
    Label { text: String, at: Point, size: f32, weight: Weight, color: Tone },
    /// Wrapped text inside `rect`.
    Paragraph { text: String, rect: Rect, size: f32, color: Tone },
    Recorder { target: HotkeyTarget, rect: Rect, shortcut: Option<Shortcut>, recording: bool },
    Button { hit: Hit, rect: Rect, label: String, primary: bool, enabled: bool },
    Segmented { rect: Rect, options: Vec<String>, selected: usize, hit: fn(usize) -> Hit },
    Toggle { hit: Hit, rect: Rect, on: bool, enabled: bool },
    Field { field: Field, rect: Rect },
    Link { rect: Rect, text: String },
    Progress { rect: Rect, fraction: f32 },
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Tone {
    Primary,
    Secondary,
    Danger,
    Success,
}

pub struct SettingsView {
    pane: Pane,
    pub state: SettingsState,
    theme: Theme,
    hover: Option<Hit>,
    pressed: Option<Hit>,
    recording: Option<HotkeyTarget>,
    effects: Vec<SettingsEffect>,
}

pub const DEEPSEEK_URL: &str = "https://platform.deepseek.com";

impl SettingsView {
    pub fn new(state: SettingsState, theme: Theme) -> SettingsView {
        SettingsView { pane: Pane::Shortcuts, state, theme, hover: None, pressed: None, recording: None, effects: Vec::new() }
    }

    #[cfg(test)]
    pub fn pane(&self) -> Pane {
        self.pane
    }

    pub fn set_pane(&mut self, pane: Pane) {
        if self.pane != pane {
            self.stop_recording();
            self.pane = pane;
            self.effects.push(SettingsEffect::FieldsChanged);
        }
    }

    pub fn set_theme(&mut self, theme: Theme) {
        self.theme = theme;
    }

    pub fn theme(&self) -> Theme {
        self.theme
    }

    pub fn is_recording(&self) -> bool {
        self.recording.is_some()
    }

    pub fn drain_effects(&mut self) -> Vec<SettingsEffect> {
        std::mem::take(&mut self.effects)
    }

    /// Where the platform's text boxes go on the current pane (points), with the colors to give them.
    pub fn fields(&self) -> Vec<(Field, Rect)> {
        self.elements()
            .into_iter()
            .filter_map(|e| match e {
                Elem::Field { field, rect } => Some((field, rect.inset(10.0, 6.0))),
                _ => None,
            })
            .collect()
    }

    pub fn field_background(&self) -> Color {
        self.field_fill()
    }

    pub fn field_text(&self) -> Color {
        self.theme.label()
    }

    /// Whether the pointer at `p` is over something clickable (for a hand cursor).
    pub fn is_clickable(&self, p: Point) -> bool {
        self.hit(p).is_some()
    }

    // MARK: Events

    pub fn mouse_move(&mut self, p: Point) {
        self.hover = self.hit(p);
    }

    pub fn mouse_leave(&mut self) {
        self.hover = None;
    }

    pub fn mouse_down(&mut self, p: Point) {
        let hit = self.hit(p);
        // A click anywhere else stops waiting for a shortcut.
        if self.recording.is_some() && !matches!(hit, Some(Hit::Recorder(_))) {
            self.stop_recording();
        }
        self.pressed = hit;
        // The menu reacts on press, like a list.
        if let Some(Hit::Pane(pane)) = hit {
            self.set_pane(pane);
        }
    }

    pub fn mouse_up(&mut self, p: Point) {
        let pressed = self.pressed.take();
        if pressed.is_some() && pressed == self.hit(p) {
            self.activate(pressed.unwrap());
        }
    }

    /// A key while recording a shortcut: Windows virtual-key code and modifier state. Returns whether it was used.
    pub fn key_down(&mut self, vk: u32, ctrl: bool, alt: bool, shift: bool, win: bool) -> bool {
        let Some(target) = self.recording else { return false };
        const ESC: u32 = 0x1B;
        if vk == ESC {
            self.stop_recording();
            return true;
        }
        // Modifier keys alone: keep waiting.
        if matches!(vk, 0x10 | 0x11 | 0x12 | 0x5B | 0x5C | 0xA0..=0xA5) {
            return true;
        }
        let mut modifiers = 0;
        if ctrl {
            modifiers |= MOD_CONTROL;
        }
        if alt {
            modifiers |= MOD_ALT;
        }
        if shift {
            modifiers |= MOD_SHIFT;
        }
        if win {
            modifiers |= MOD_WIN;
        }
        let function_key = (0x70..=0x87).contains(&vk);
        // A plain key (or Shift+key) would swallow normal typing everywhere.
        if !function_key && (modifiers == 0 || modifiers == MOD_SHIFT) {
            self.effects.push(SettingsEffect::Beep);
            return true;
        }
        let shortcut = Shortcut::new(vk, modifiers);
        self.recording = None;
        match target {
            HotkeyTarget::Capture => {
                self.state.capture = shortcut.clone();
                self.effects.push(SettingsEffect::SetCapture(shortcut));
            }
            HotkeyTarget::TogglePins => {
                self.state.toggle_pins = Some(shortcut.clone());
                self.effects.push(SettingsEffect::SetTogglePins(Some(shortcut)));
            }
        }
        self.effects.push(SettingsEffect::ResumeHotkeys);
        true
    }

    pub fn stop_recording(&mut self) {
        if self.recording.take().is_some() {
            self.effects.push(SettingsEffect::ResumeHotkeys);
        }
    }

    fn activate(&mut self, hit: Hit) {
        match hit {
            Hit::Engine(i) => {
                if !self.state.testing {
                    self.state.engine = if i == 0 { TranslationEngine::Free } else { TranslationEngine::Llm };
                    self.state.test_result = None;
                    self.effects.push(SettingsEffect::SetEngine(self.state.engine));
                    self.effects.push(SettingsEffect::FieldsChanged);
                }
            }
            Hit::Pane(_) => {}
            Hit::Recorder(target) => {
                if self.recording == Some(target) {
                    self.stop_recording();
                } else {
                    let was = self.recording.replace(target);
                    if was.is_none() {
                        self.effects.push(SettingsEffect::PauseHotkeys);
                    }
                }
            }
            Hit::ClearPins => {
                self.state.toggle_pins = None;
                self.effects.push(SettingsEffect::SetTogglePins(None));
            }
            Hit::ChooseFolder => self.effects.push(SettingsEffect::ChooseFolder),
            Hit::Format(f) => {
                self.state.format = f;
                self.effects.push(SettingsEffect::SetFormat(f));
            }
            Hit::Language(i) => {
                if let Some(lang) = LANGUAGES.get(i) {
                    self.state.language = lang.to_string();
                    self.effects.push(SettingsEffect::SetLanguage(lang.to_string()));
                }
            }
            Hit::Link => self.effects.push(SettingsEffect::OpenUrl(DEEPSEEK_URL.into())),
            Hit::Test => {
                if !self.state.testing && (self.state.engine == TranslationEngine::Free || self.state.api_key_present) {
                    self.state.testing = true;
                    self.state.test_result = None;
                    self.effects.push(SettingsEffect::TestConnection);
                }
            }
            Hit::Reset => self.effects.push(SettingsEffect::ResetTranslation),
            Hit::Login => {
                self.state.login = !self.state.login;
                self.effects.push(SettingsEffect::SetLogin(self.state.login));
            }
            Hit::AutoUpdate => {
                if let Some(on) = self.state.auto_update {
                    self.state.auto_update = Some(!on);
                    self.effects.push(SettingsEffect::SetAutoUpdate(!on));
                }
            }
            Hit::Models => self.effects.push(SettingsEffect::ModelsButton),
        }
    }

    fn hit(&self, p: Point) -> Option<Hit> {
        for (i, pane) in Pane::ALL.iter().enumerate() {
            if sidebar_item(i).contains(p) {
                return Some(Hit::Pane(*pane));
            }
        }
        for e in self.elements() {
            match e {
                Elem::Recorder { target, rect, .. } if rect.contains(p) => return Some(Hit::Recorder(target)),
                Elem::Button { hit, rect, enabled: true, .. } if rect.contains(p) => return Some(hit),
                Elem::Toggle { hit, rect, enabled: true, .. } if rect.inset(-4.0, -4.0).contains(p) => return Some(hit),
                Elem::Link { rect, .. } if rect.contains(p) => return Some(Hit::Link),
                Elem::Segmented { rect, options, hit, .. } if rect.contains(p) => {
                    let w = rect.width / options.len() as f32;
                    let i = (((p.x - rect.x) / w) as usize).min(options.len() - 1);
                    return Some(hit(i));
                }
                _ => {}
            }
        }
        None
    }

    // MARK: Layout

    fn elements(&self) -> Vec<Elem> {
        let mut v = vec![Elem::Title(self.pane.title().into(), Point::new(CONTENT_X, 22.0))];
        let x = CONTENT_X;
        let w = CONTENT_W;
        let right = x + w - 16.0;
        let s = &self.state;
        let row_label = |v: &mut Vec<Elem>, text: &str, y: f32| {
            let (_, h) = text::measure(text, 13.0, Weight::Regular, None);
            v.push(Elem::Label { text: text.into(), at: Point::new(x + 16.0, y + (ROW - h) / 2.0), size: 13.0, weight: Weight::Regular, color: Tone::Primary });
        };
        let footer = |v: &mut Vec<Elem>, text: &str, y: f32| {
            v.push(Elem::Paragraph { text: text.into(), rect: Rect::new(x + 4.0, y, w - 8.0, 80.0), size: 12.0, color: Tone::Secondary });
        };
        match self.pane {
            Pane::Shortcuts => {
                let top = 64.0;
                v.push(Elem::Card(Rect::new(x, top, w, ROW * 2.0)));
                row_label(&mut v, "截图", top);
                let rec = Rect::new(right - 200.0, top + 8.0, 200.0, 30.0);
                v.push(Elem::Recorder { target: HotkeyTarget::Capture, rect: rec, shortcut: Some(s.capture.clone()), recording: self.recording == Some(HotkeyTarget::Capture) });
                if !s.capture_ok {
                    v.push(Elem::Label { text: "已被其他程序占用".into(), at: Point::new(rec.x - 116.0, top + 15.0), size: 12.0, weight: Weight::Regular, color: Tone::Danger });
                }
                v.push(Elem::Divider(Rect::new(x + 16.0, top + ROW, w - 16.0, 0.5)));
                let y = top + ROW;
                row_label(&mut v, "隐藏 / 显示全部贴图", y);
                let has = s.toggle_pins.is_some();
                let rec = Rect::new(right - 200.0 - if has { 62.0 } else { 0.0 }, y + 8.0, 200.0, 30.0);
                v.push(Elem::Recorder { target: HotkeyTarget::TogglePins, rect: rec, shortcut: s.toggle_pins.clone(), recording: self.recording == Some(HotkeyTarget::TogglePins) });
                if has {
                    v.push(Elem::Button { hit: Hit::ClearPins, rect: Rect::new(right - 54.0, y + 9.0, 54.0, 28.0), label: "清除".into(), primary: false, enabled: true });
                }
                if has && !s.toggle_ok {
                    v.push(Elem::Label { text: "已被占用".into(), at: Point::new(rec.x - 70.0, y + 15.0), size: 12.0, weight: Weight::Regular, color: Tone::Danger });
                }
                footer(
                    &mut v,
                    "全局快捷键，在任何应用里都能用。点一下按键框，再按下新的组合键（需要带 Ctrl、Alt 或 Win；F1–F12 可以单独用），Esc 取消。\n\n截图时各个工具的单键快捷键（1–7、X、Y、T），在工具栏按钮的悬停卡片上修改。",
                    top + ROW * 2.0 + 12.0,
                );
            }
            Pane::Save => {
                let top = 64.0;
                v.push(Elem::Card(Rect::new(x, top, w, ROW * 2.0)));
                row_label(&mut v, "保存位置", top);
                let button = Rect::new(right - 72.0, top + 9.0, 72.0, 28.0);
                v.push(Elem::Button { hit: Hit::ChooseFolder, rect: button, label: "选择…".into(), primary: false, enabled: true });
                let max = button.x - 12.0 - (x + 110.0);
                let path = elide_middle(&s.save_dir, 12.5, max);
                let (pw, ph) = text::measure(&path, 12.5, Weight::Regular, None);
                v.push(Elem::Label { text: path, at: Point::new(button.x - 12.0 - pw, top + (ROW - ph) / 2.0), size: 12.5, weight: Weight::Regular, color: Tone::Secondary });
                v.push(Elem::Divider(Rect::new(x + 16.0, top + ROW, w - 16.0, 0.5)));
                row_label(&mut v, "保存格式", top + ROW);
                v.push(Elem::Segmented {
                    rect: Rect::new(right - 150.0, top + ROW + 9.0, 150.0, 28.0),
                    options: vec!["PNG".into(), "JPG".into()],
                    selected: if s.format == ImageFormat::Png { 0 } else { 1 },
                    hit: |i| Hit::Format(if i == 0 { ImageFormat::Png } else { ImageFormat::Jpeg }),
                });
                footer(&mut v, "截图时按 Ctrl+S 直接保存到这里；Ctrl+Shift+S 另存为。默认是「下载」文件夹。", top + ROW * 2.0 + 12.0);
            }
            Pane::Translate => {
                let top = 64.0;
                let llm = s.engine == TranslationEngine::Llm;
                let rows = if llm { 5.0 } else { 2.0 };
                v.push(Elem::Card(Rect::new(x, top, w, ROW * rows)));
                row_label(&mut v, "翻译引擎", top);
                v.push(Elem::Segmented {
                    rect: Rect::new(right - 330.0, top + 9.0, 330.0, 28.0),
                    options: vec!["免费翻译".into(), "大模型".into()],
                    selected: usize::from(llm), hit: Hit::Engine,
                });
                row_label(&mut v, "译成", top + ROW);
                v.push(Elem::Segmented {
                    rect: Rect::new(right - 330.0, top + ROW + 9.0, 330.0, 28.0),
                    options: LANGUAGES.iter().map(|l| l.to_string()).collect(),
                    selected: LANGUAGES.iter().position(|l| *l == s.language).unwrap_or(0),
                    hit: Hit::Language,
                });
                if llm {
                    for (i, (label, field)) in [("API Key", Field::ApiKey), ("Base URL", Field::BaseUrl), ("模型", Field::Model)].into_iter().enumerate() {
                        let y = top + ROW * (i + 2) as f32;
                        row_label(&mut v, label, y);
                        v.push(Elem::Field { field, rect: Rect::new(right - 330.0, y + 8.0, 330.0, 30.0) });
                    }
                }
                let by = top + ROW * rows + 14.0;
                v.push(Elem::Button { hit: Hit::Test, rect: Rect::new(x, by, 96.0, 30.0),
                    label: if s.testing { "测试中…".into() } else { "测试连接".into() }, primary: true,
                    enabled: !s.testing && (!llm || s.api_key_present) });
                if llm {
                    v.push(Elem::Button { hit: Hit::Reset, rect: Rect::new(x + 106.0, by, 96.0, 30.0), label: "恢复默认".into(), primary: false, enabled: !s.testing });
                }
                if let Some((ok, msg)) = &s.test_result {
                    v.push(Elem::Paragraph { text: msg.clone(), rect: Rect::new(x + 4.0, by + 40.0, w - 8.0, 44.0),
                        size: 12.0, color: if *ok { Tone::Success } else { Tone::Danger } });
                }
                let fy = by + if s.test_result.is_some() { 92.0 } else { 44.0 };
                if llm {
                    let link = "DeepSeek 开放平台";
                    let (lw, lh) = text::measure(link, 12.0, Weight::Regular, None);
                    v.push(Elem::Link { rect: Rect::new(x + 4.0, fy, lw, lh), text: link.into() });
                    footer(&mut v, "支持 OpenAI 兼容接口，默认使用 DeepSeek。API Key 用 DPAPI 加密保存在本机。只发送识别出的文字，截图不会上传。", fy + 24.0);
                } else {
                    footer(&mut v, "腾讯交互翻译，无需 API Key。只发送识别出的文字，截图不会上传。免费服务暂时不可用时，可以切换到大模型。", fy);
                }
            }
            Pane::General => {
                let top = 64.0;
                v.push(Elem::Card(Rect::new(x, top, w, ROW * 2.0)));
                row_label(&mut v, "登录时启动 Shotlate", top);
                v.push(Elem::Toggle { hit: Hit::Login, rect: Rect::new(right - 40.0, top + 12.0, 40.0, 22.0), on: s.login, enabled: true });
                v.push(Elem::Divider(Rect::new(x + 16.0, top + ROW, w - 16.0, 0.5)));
                row_label(&mut v, "自动检查更新", top + ROW);
                let available = s.auto_update.is_some();
                v.push(Elem::Toggle { hit: Hit::AutoUpdate, rect: Rect::new(right - 40.0, top + ROW + 12.0, 40.0, 22.0), on: s.auto_update.unwrap_or(false), enabled: available });
                if !available {
                    v.push(Elem::Label { text: "这个版本不支持".into(), at: Point::new(right - 150.0, top + ROW + 15.0), size: 12.0, weight: Weight::Regular, color: Tone::Secondary });
                }

                let top2 = top + ROW * 2.0 + 14.0;
                let downloading = matches!(s.models, Models::Downloading(_));
                v.push(Elem::Card(Rect::new(x, top2, w, ROW + if downloading { 14.0 } else { 0.0 })));
                row_label(&mut v, "文字识别组件", top2);
                let (status, button) = match s.models {
                    Models::Ready => (format!("已下载（约 {:.0} MB）", s.models_mb), "重新下载"),
                    Models::Missing => (format!("未下载（约 {:.0} MB）", s.models_mb), "下载"),
                    Models::Downloading(f) => (format!("正在下载 {:.0}%", f * 100.0), "取消"),
                };
                let b = Rect::new(right - 84.0, top2 + 9.0, 84.0, 28.0);
                v.push(Elem::Button { hit: Hit::Models, rect: b, label: button.into(), primary: s.models == Models::Missing, enabled: true });
                let (sw, sh) = text::measure(&status, 12.5, Weight::Regular, None);
                v.push(Elem::Label {
                    text: status,
                    at: Point::new(b.x - 12.0 - sw, top2 + (ROW - sh) / 2.0),
                    size: 12.5,
                    weight: Weight::Regular,
                    color: if s.models == Models::Ready { Tone::Success } else { Tone::Secondary },
                });
                if let Models::Downloading(f) = s.models {
                    v.push(Elem::Progress { rect: Rect::new(x + 16.0, top2 + ROW, w - 32.0, 4.0), fraction: f });
                }
                footer(
                    &mut v,
                    &format!(
                        "识别文字和翻译用的离线识别模型（PP-OCRv6），只需下载一次，保存在\n{}\n\n当前版本 {}。有新版本时会提示，由你决定何时安装。",
                        s.models_dir, s.version
                    ),
                    top2 + ROW + if downloading { 14.0 } else { 0.0 } + 12.0,
                );
            }
        }
        v
    }

    // MARK: Drawing

    fn card_fill(&self) -> Color {
        if self.theme.dark { Color::rgb(0.17, 0.17, 0.18) } else { Color::white(1.0) }
    }

    fn window_fill(&self) -> Color {
        if self.theme.dark { Color::rgb(0.11, 0.11, 0.12) } else { Color::rgb(0.955, 0.955, 0.965) }
    }

    fn sidebar_fill(&self) -> Color {
        if self.theme.dark { Color::rgb(0.14, 0.14, 0.15) } else { Color::rgb(0.925, 0.925, 0.935) }
    }

    fn control_fill(&self) -> Color {
        if self.theme.dark { Color::rgb(0.24, 0.24, 0.26) } else { Color::rgb(0.93, 0.93, 0.94) }
    }

    fn field_fill(&self) -> Color {
        if self.theme.dark { Color::rgb(0.12, 0.12, 0.13) } else { Color::rgb(0.985, 0.985, 0.99) }
    }

    fn tone(&self, t: Tone) -> Color {
        match t {
            Tone::Primary => self.theme.label(),
            Tone::Secondary => self.theme.secondary(),
            Tone::Danger => Color::rgb(0.86, 0.2, 0.18),
            Tone::Success => Color::rgb(0.13, 0.6, 0.3),
        }
    }

    /// The whole window at `scale` pixels per point.
    pub fn render(&self, scale: f32) -> Option<Pixmap> {
        let (w, h) = ((WINDOW.width * scale).round() as u32, (WINDOW.height * scale).round() as u32);
        let mut pix = Pixmap::new(w, h)?;
        {
            let mut c = Canvas::new(pix.as_mut(), Transform::from_scale(scale, scale));
            c.fill_rect(&Rect::new(0.0, 0.0, WINDOW.width, WINDOW.height), self.window_fill());
            self.draw_sidebar(&mut c);
            for e in self.elements() {
                self.draw_elem(&mut c, &e);
            }
        }
        Some(pix)
    }

    fn draw_sidebar(&self, c: &mut Canvas) {
        c.fill_rect(&Rect::new(0.0, 0.0, SIDEBAR, WINDOW.height), self.sidebar_fill());
        c.fill_rect(&Rect::new(SIDEBAR - 0.5, 0.0, 0.5, WINDOW.height), self.theme.separator());
        c.text("Shotlate", Point::new(20.0, 20.0), 15.0, Weight::Bold, self.theme.label());
        for (i, pane) in Pane::ALL.iter().enumerate() {
            let r = sidebar_item(i);
            let selected = *pane == self.pane;
            if selected {
                c.fill_rounded(&r, 7.0, SELECTION_BLUE);
            } else if self.hover == Some(Hit::Pane(*pane)) {
                c.fill_rounded(&r, 7.0, self.theme.hover());
            }
            let color = if selected { Color::white(1.0) } else { self.theme.label() };
            draw_pane_icon(c, *pane, Point::new(r.x + 20.0, r.mid_y()), color);
            let (_, th) = text::measure(pane.title(), 13.5, Weight::Regular, None);
            c.text(pane.title(), Point::new(r.x + 38.0, r.mid_y() - th / 2.0), 13.5, if selected { Weight::Bold } else { Weight::Regular }, color);
        }
    }

    fn draw_elem(&self, c: &mut Canvas, e: &Elem) {
        let theme = &self.theme;
        match e {
            Elem::Title(t, at) => {
                c.text(t, *at, 20.0, Weight::Bold, theme.label());
            }
            Elem::Card(r) => {
                if let Some(p) = crate::render::canvas::rounded_rect_path(r, 10.0) {
                    c.shadow(&p, (0.0, 1.0), 3.0, Color::black(if theme.dark { 0.3 } else { 0.06 }));
                    c.fill_path(&p, self.card_fill());
                    c.stroke_path(&p, theme.separator(), &crate::render::canvas::stroke(0.5));
                }
            }
            Elem::Divider(r) => c.fill_rect(r, theme.separator()),
            Elem::Label { text, at, size, weight, color } => {
                c.text(text, *at, *size, *weight, self.tone(*color));
            }
            Elem::Paragraph { text, rect, size, color } => {
                let l = text::layout(text, *size, Weight::Regular, Some(rect.width));
                c.text_layout(&l, rect.origin(), self.tone(*color));
            }
            Elem::Recorder { target, rect, shortcut, recording } => {
                let hovered = self.hover == Some(Hit::Recorder(*target));
                c.fill_rounded(rect, 7.0, if hovered && !recording { theme.hover().with_alpha(0.12) } else { self.control_fill() });
                if *recording {
                    c.stroke_rounded(&rect.inset(0.75, 0.75), 7.0, SELECTION_BLUE, 1.5);
                    c.text_centered("按下新的组合键…（Esc 取消）", rect, 12.0, Weight::Regular, SELECTION_BLUE);
                } else {
                    match shortcut {
                        Some(sc) => self.draw_keycaps(c, rect, &sc.display()),
                        None => c.text_centered("点击设置", rect, 12.5, Weight::Regular, theme.secondary()),
                    }
                }
            }
            Elem::Button { hit, rect, label, primary, enabled } => {
                let hovered = self.hover == Some(*hit) && *enabled;
                let pressed = self.pressed == Some(*hit);
                let (fill, fg) = if *primary {
                    let base = if pressed { Color::rgb(0.1, 0.46, 0.8) } else if hovered { Color::rgb(0.2, 0.62, 0.97) } else { SELECTION_BLUE };
                    (base, Color::white(1.0))
                } else {
                    let base = if pressed || hovered { theme.label().with_alpha(if pressed { 0.16 } else { 0.11 }) } else { self.control_fill() };
                    (base, theme.label())
                };
                let fill = if *enabled { fill } else { fill.with_alpha(0.5) };
                c.fill_rounded(rect, 7.0, fill);
                c.text_centered(label, rect, 12.5, Weight::Regular, if *enabled { fg } else { fg.with_alpha(0.6) });
            }
            Elem::Segmented { rect, options, selected, hit } => {
                c.fill_rounded(rect, 7.0, self.control_fill());
                let w = rect.width / options.len() as f32;
                for (i, o) in options.iter().enumerate() {
                    let seg = Rect::new(rect.x + i as f32 * w, rect.y, w, rect.height).inset(2.0, 2.0);
                    if i == *selected {
                        if let Some(p) = crate::render::canvas::rounded_rect_path(&seg, 5.5) {
                            c.shadow(&p, (0.0, 1.0), 2.0, Color::black(0.12));
                            c.fill_path(&p, if theme.dark { Color::rgb(0.38, 0.38, 0.4) } else { Color::white(1.0) });
                        }
                    } else if self.hover == Some(hit(i)) {
                        c.fill_rounded(&seg, 5.5, theme.hover());
                    }
                    c.text_centered(o, &seg, 12.0, if i == *selected { Weight::Bold } else { Weight::Regular }, theme.label());
                }
            }
            Elem::Toggle { rect, on, enabled, .. } => {
                let track = if *on { SELECTION_BLUE } else if theme.dark { Color::rgb(0.32, 0.32, 0.34) } else { Color::rgb(0.82, 0.82, 0.84) };
                let alpha = if *enabled { 1.0 } else { 0.45 };
                c.fill_rounded(rect, rect.height / 2.0, track.with_alpha(alpha));
                let d = rect.height - 4.0;
                let kx = if *on { rect.max_x() - 2.0 - d } else { rect.x + 2.0 };
                let knob = Rect::new(kx, rect.y + 2.0, d, d);
                if let Some(p) = crate::render::canvas::oval_path(&knob) {
                    c.shadow(&p, (0.0, 1.0), 1.5, Color::black(0.2));
                    c.fill_path(&p, Color::white(alpha));
                }
            }
            Elem::Field { rect, .. } => {
                c.fill_rounded(rect, 7.0, self.field_fill());
                c.stroke_rounded(&rect.inset(0.5, 0.5), 7.0, theme.label().with_alpha(if theme.dark { 0.22 } else { 0.16 }), 1.0);
            }
            Elem::Link { rect, text } => {
                let color = if self.hover == Some(Hit::Link) { Color::rgb(0.1, 0.46, 0.8) } else { SELECTION_BLUE };
                c.text(text, rect.origin(), 12.0, Weight::Regular, color);
                c.fill_rect(&Rect::new(rect.x, rect.max_y() - 1.0, rect.width, 0.8), color.with_alpha(0.6));
            }
            Elem::Progress { rect, fraction } => {
                c.fill_rounded(rect, 2.0, self.control_fill());
                c.fill_rounded(&Rect::new(rect.x, rect.y, rect.width * fraction.clamp(0.0, 1.0), rect.height), 2.0, SELECTION_BLUE);
            }
        }
    }

    /// `Alt+Shift+A` as key caps centered in `rect`, like the capture toolbar's hover card.
    fn draw_keycaps(&self, c: &mut Canvas, rect: &Rect, display: &str) {
        let parts: Vec<&str> = display.split('+').collect();
        let widths: Vec<f32> = parts.iter().map(|p| text::measure(p, 12.0, Weight::Regular, None).0 + 14.0).map(|w| w.max(22.0)).collect();
        let total = widths.iter().sum::<f32>() + (parts.len() as f32 - 1.0) * 4.0;
        let mut x = rect.mid_x() - total / 2.0;
        for (p, w) in parts.iter().zip(widths) {
            let cap = Rect::new(x, rect.mid_y() - 10.0, w, 20.0);
            if let Some(path) = crate::render::canvas::rounded_rect_path(&cap, 4.0) {
                c.shadow(&path, (0.0, 1.0), 1.0, Color::black(0.15));
                c.fill_path(&path, if self.theme.dark { Color::rgb(0.36, 0.36, 0.38) } else { Color::white(1.0) });
            }
            c.text_centered(p, &cap, 12.0, Weight::Regular, self.theme.label());
            x += w + 4.0;
        }
    }
}

/// Where things are, for the end-to-end test that clicks them (points, window client coordinates).
pub fn sidebar_center(i: usize) -> Point {
    sidebar_item(i).center()
}

pub fn api_key_center() -> Point {
    Point::new(CONTENT_X + CONTENT_W - 16.0 - 165.0, 64.0 + ROW * 2.0 + 8.0 + 15.0)
}

pub fn engine_center(llm: bool) -> Point {
    Point::new(CONTENT_X + CONTENT_W - 16.0 - if llm { 82.5 } else { 247.5 }, 64.0 + 23.0)
}

pub fn test_button_center() -> Point {
    Point::new(CONTENT_X + 48.0, 64.0 + ROW * 5.0 + 14.0 + 15.0)
}

fn sidebar_item(i: usize) -> Rect {
    Rect::new(10.0, 58.0 + i as f32 * 38.0, SIDEBAR - 20.0, 32.0)
}

/// `text` with its middle replaced by "…" so it fits `max` points.
fn elide_middle(text: &str, size: f32, max: f32) -> String {
    if text::measure(text, size, Weight::Regular, None).0 <= max {
        return text.to_string();
    }
    let chars: Vec<char> = text.chars().collect();
    let mut keep = chars.len();
    while keep > 2 {
        keep -= 1;
        let head: String = chars[..keep / 2].iter().collect();
        let tail: String = chars[chars.len() - (keep - keep / 2)..].iter().collect();
        let s = format!("{head}…{tail}");
        if text::measure(&s, size, Weight::Regular, None).0 <= max {
            return s;
        }
    }
    "…".into()
}

/// Small line icons for the menu.
fn draw_pane_icon(c: &mut Canvas, pane: Pane, center: Point, color: Color) {
    let o = Point::new(center.x - 8.0, center.y - 8.0);
    let s = crate::render::canvas::stroke(1.4);
    let path = |pts: &[(f32, f32)], close: bool| {
        crate::render::canvas::polyline_path(&pts.iter().map(|(x, y)| Point::new(o.x + x, o.y + y)).collect::<Vec<_>>(), close)
    };
    match pane {
        Pane::Shortcuts => {
            c.stroke_rounded(&Rect::new(o.x + 1.0, o.y + 3.5, 14.0, 9.5), 2.0, color, 1.4);
            for (x, y) in [(4.0, 6.5), (7.0, 6.5), (10.0, 6.5), (13.0, 6.5)] {
                c.fill_rect(&Rect::new(o.x + x - 0.8, o.y + y - 0.8, 1.6, 1.6), color);
            }
            if let Some(p) = path(&[(5.0, 10.0), (11.0, 10.0)], false) {
                c.stroke_path(&p, color, &s);
            }
        }
        Pane::Save => {
            if let Some(p) = path(&[(2.0, 9.5), (2.0, 14.0), (14.0, 14.0), (14.0, 9.5)], false) {
                c.stroke_path(&p, color, &s);
            }
            if let Some(p) = path(&[(8.0, 1.5), (8.0, 10.0)], false) {
                c.stroke_path(&p, color, &s);
            }
            if let Some(p) = path(&[(4.5, 6.5), (8.0, 10.0), (11.5, 6.5)], false) {
                c.stroke_path(&p, color, &s);
            }
        }
        Pane::Translate => {
            c.text_centered("文", &Rect::new(o.x - 1.0, o.y - 1.0, 11.0, 11.0), 9.0, Weight::Regular, color);
            c.text_centered("A", &Rect::new(o.x + 7.0, o.y + 6.0, 10.0, 11.0), 9.5, Weight::Regular, color);
        }
        Pane::General => {
            c.stroke_oval(&Rect::new(o.x + 4.5, o.y + 4.5, 7.0, 7.0), color, 1.4);
            for k in 0..8 {
                let a = k as f32 * std::f32::consts::FRAC_PI_4;
                let (x1, y1) = (8.0 + 5.2 * a.cos(), 8.0 + 5.2 * a.sin());
                let (x2, y2) = (8.0 + 7.2 * a.cos(), 8.0 + 7.2 * a.sin());
                if let Some(p) = path(&[(x1, y1), (x2, y2)], false) {
                    c.stroke_path(&p, color, &crate::render::canvas::stroke(1.8));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> SettingsState {
        SettingsState {
            engine: TranslationEngine::Llm,
            api_key_present: true,
            capture: Shortcut::CAPTURE,
            toggle_pins: Some(Shortcut::TOGGLE_PINS),
            capture_ok: true,
            toggle_ok: true,
            save_dir: "C:\\Users\\tester\\Downloads".into(),
            format: ImageFormat::Png,
            language: "简体中文".into(),
            testing: false,
            test_result: None,
            login: false,
            auto_update: Some(true),
            models: Models::Missing,
            models_mb: 23.0,
            models_dir: "C:\\Users\\tester\\AppData\\Local\\Shotlate\\models".into(),
            version: "0.1.0".into(),
        }
    }

    fn click(v: &mut SettingsView, p: Point) {
        v.mouse_move(p);
        v.mouse_down(p);
        v.mouse_up(p);
    }

    fn center_of(v: &SettingsView, want: Hit) -> Point {
        for e in v.elements() {
            let r = match e {
                Elem::Recorder { target, rect, .. } if Hit::Recorder(target) == want => rect,
                Elem::Button { hit, rect, .. } if hit == want => rect,
                Elem::Toggle { hit, rect, .. } if hit == want => rect,
                Elem::Link { rect, .. } if want == Hit::Link => rect,
                _ => continue,
            };
            return r.center();
        }
        panic!("{want:?} not on this pane");
    }

    #[test]
    fn switching_panes_moves_the_text_boxes() {
        let mut v = SettingsView::new(state(), Theme { dark: false });
        assert!(v.fields().is_empty());
        click(&mut v, sidebar_item(2).center());
        assert_eq!(v.pane(), Pane::Translate);
        assert_eq!(v.fields().iter().map(|f| f.0).collect::<Vec<_>>(), vec![Field::ApiKey, Field::BaseUrl, Field::Model]);
        assert!(v.drain_effects().contains(&SettingsEffect::FieldsChanged));
    }

    #[test]
    fn free_engine_hides_secrets_and_tests_without_key() {
        let mut v = SettingsView::new(state(), Theme { dark: false });
        v.set_pane(Pane::Translate); v.state.api_key_present = false;
        click(&mut v, engine_center(false));
        assert_eq!(v.state.engine, TranslationEngine::Free);
        assert!(v.fields().is_empty());
        assert!(v.drain_effects().contains(&SettingsEffect::SetEngine(TranslationEngine::Free)));
        let at = center_of(&v, Hit::Test); click(&mut v, at);
        assert!(v.drain_effects().contains(&SettingsEffect::TestConnection));
    }

    #[test]
    fn recording_a_shortcut() {
        let mut v = SettingsView::new(state(), Theme { dark: false });
        let at = center_of(&v, Hit::Recorder(HotkeyTarget::Capture));
        click(&mut v, at);
        assert!(v.is_recording());
        assert_eq!(v.drain_effects(), vec![SettingsEffect::PauseHotkeys]);
        // A lone letter is refused, modifiers alone keep waiting.
        v.key_down(0x41, false, false, false, false);
        v.key_down(0x11, true, false, false, false);
        assert!(v.is_recording());
        v.key_down(0x51, true, true, false, false);
        assert!(!v.is_recording());
        let e = v.drain_effects();
        assert!(e.contains(&SettingsEffect::SetCapture(Shortcut::new(0x51, MOD_CONTROL | MOD_ALT))));
        assert!(e.contains(&SettingsEffect::ResumeHotkeys));
        assert_eq!(v.state.capture.display(), "Ctrl+Alt+Q");
        // Esc cancels.
        let at = center_of(&v, Hit::Recorder(HotkeyTarget::TogglePins));
        click(&mut v, at);
        v.key_down(0x1B, false, false, false, false);
        assert!(!v.is_recording());
    }

    #[test]
    fn toggles_segments_and_buttons() {
        let mut v = SettingsView::new(state(), Theme { dark: false });
        click(&mut v, sidebar_item(1).center());
        let seg = v.elements().into_iter().find_map(|e| if let Elem::Segmented { rect, .. } = e { Some(rect) } else { None }).unwrap();
        click(&mut v, Point::new(seg.max_x() - 10.0, seg.mid_y()));
        assert_eq!(v.state.format, ImageFormat::Jpeg);
        click(&mut v, sidebar_item(3).center());
        let at = center_of(&v, Hit::Login);
        click(&mut v, at);
        assert!(v.state.login);
        let at = center_of(&v, Hit::Models);
        click(&mut v, at);
        let e = v.drain_effects();
        assert!(e.contains(&SettingsEffect::SetFormat(ImageFormat::Jpeg)));
        assert!(e.contains(&SettingsEffect::SetLogin(true)));
        assert!(e.contains(&SettingsEffect::ModelsButton));
        click(&mut v, sidebar_item(2).center());
        let at = center_of(&v, Hit::Test);
        click(&mut v, at);
        assert!(v.state.testing);
        let at = center_of(&v, Hit::Link);
        click(&mut v, at);
        assert!(v.drain_effects().contains(&SettingsEffect::OpenUrl(DEEPSEEK_URL.into())));
    }

    #[test]
    fn renders_every_pane_in_both_themes() {
        for dark in [false, true] {
            let mut v = SettingsView::new(state(), Theme { dark });
            for pane in Pane::ALL {
                v.set_pane(pane);
                let p = v.render(1.5).unwrap();
                assert_eq!(p.width(), 1080);
            }
        }
    }

    #[test]
    fn long_paths_are_elided() {
        let long = "C:\\Users\\someone\\a very long folder name\\another folder\\and another one\\Screenshots";
        let e = elide_middle(long, 12.5, 200.0);
        assert!(e.contains('…'));
        assert!(e.ends_with("Screenshots") || e.ends_with('s'));
    }
}
