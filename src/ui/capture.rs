//! One monitor's capture overlay: dimming, selection, annotations, magnifier, toolbar, style bar, OCR and
//! translation. A port of CaptureView.swift as a platform-independent state machine: the platform feeds it
//! input events, draws what `render` produces, and carries out the `Effect`s it asks for.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use tiny_skia::{Pixmap, Transform};

use super::chrome::{self, CaretEdge, HoverCard, StyleAction, StyleItem, StyleState, Theme, ToolbarLook};
use super::toolbar::{self, ToolbarAction};
use crate::kit::annotation::{
    AnnotationItem, Cursor, ItemHandle, ItemStyle, MosaicEffect, MosaicMode, ResizeHandle, Shape, TextDecoration, Tool, lens_center,
};
use crate::kit::color::{Color, SELECTION_BLUE};
use crate::kit::geom::{Point, Rect, Size};
use crate::kit::image::RgbaImage;
use crate::kit::settings::{self, ImageFormat};
use crate::kit::textblocks::{self, OcrLine, TextBlock};
use crate::kit::translator::{Item, TranslationError};
use crate::render::canvas::Canvas;
use crate::render::content::{Base, ContentRenderer, FontMeasure, TranslatedBlock, annotation_weight};
use crate::render::text;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Mods {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
}

impl Mods {
    pub const NONE: Mods = Mods { shift: false, ctrl: false, alt: false };
    pub const SHIFT: Mods = Mods { shift: true, ctrl: false, alt: false };
    pub const CTRL: Mods = Mods { shift: false, ctrl: true, alt: false };
    pub const CTRL_SHIFT: Mods = Mods { shift: true, ctrl: true, alt: false };

    pub fn is_empty(&self) -> bool {
        !self.shift && !self.ctrl && !self.alt
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Right,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Escape,
    Enter,
    Backspace,
    Delete,
    Left,
    Right,
    Up,
    Down,
    /// A letter (lowercase) or digit, from the physical key, whatever the keyboard layout or IME.
    Char(char),
    Other,
}

/// Where the hidden text box that receives typing (and the IME) should be.
#[derive(Clone, Debug, PartialEq)]
pub struct TextInput {
    /// Caret rect in view points, for placing the IME composition window.
    pub caret: Rect,
    /// Text to load into the edit box when it starts, with the caret at the end.
    pub initial: Option<String>,
    pub font_size: f32,
}

/// Something the platform has to do for the view.
#[derive(Debug)]
pub enum Effect {
    /// End the capture without output.
    Close,
    /// Copy the image and close; the platform shows "已复制到剪贴板".
    Copy(RgbaImage),
    /// Save into the configured folder and close; on failure the platform calls `show_message` instead.
    Save(RgbaImage, ImageFormat),
    SaveAs(RgbaImage, ImageFormat),
    /// Float the image above other windows where the selection (view points) was, and close.
    Pin(RgbaImage, Rect),
    CopyText(String),
    /// Copy what's in the OCR panel's edit box (the user may have changed it).
    CopyOcrPanelText,
    /// Recognize `image` (the selection's pixels) and call `recognition_finished`.
    Recognize(RgbaImage),
    /// The recognition models aren't downloaded yet.
    ModelsMissing,
    /// Translate and call `translation_finished`.
    Translate(Vec<Item>),
    /// Open a color picker and call `apply_custom_color`.
    PickColor(Color),
    WarpCursor(Point),
    /// Show (Some) or move the hidden text box, or remove it (None).
    TextInput(Option<TextInput>),
    /// Show the OCR panel's editable text at this rect (view points) with this text, or hide it.
    OcrPanel(Option<(Rect, String)>),
    Beep,
}

#[derive(Clone, Debug)]
enum Drag {
    None,
    Selecting(Point),
    Moving(Point, Rect),
    Resizing(ResizeHandle, Rect, Point),
    Drawing(Point),
    MovingItem(Point, AnnotationItem),
    ResizingItem(ItemHandle, AnnotationItem, Point),
    /// A press on a toolbar button, fired on release over the same button.
    Toolbar(ToolbarAction),
    /// A press inside a panel: swallowed.
    Chrome,
}

#[derive(Clone, Debug)]
enum Translation {
    None,
    Loading,
    Shown(Vec<TranslatedBlock>, Rect),
    Hidden(Vec<TranslatedBlock>, Rect),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Purpose {
    Ocr,
    Translate,
}

struct TextEdit {
    origin: Point,
    wrap: f32,
    text: String,
    /// Selection in bytes; caret at `end`.
    start: usize,
    end: usize,
    editing_id: Option<u64>,
    color: Color,
    size: f32,
    style: ItemStyle,
    /// What the IME is composing (e.g. pinyin), not yet part of `text`; shown underlined at the caret.
    composition: String,
}

struct Toast {
    text: String,
    until: Option<Instant>,
}

struct OcrPanel {
    text: String,
    line_count: usize,
    copied_until: Option<Instant>,
    origin: Point,
}

struct Card {
    action: ToolbarAction,
    recording: bool,
    note: Option<String>,
}

/// Remembered per monitor for "R: restore last selection".
static LAST_SELECTION: Mutex<Option<HashMap<String, Rect>>> = Mutex::new(None);

fn last_selection(id: &str) -> Option<Rect> {
    LAST_SELECTION.lock().ok()?.as_ref()?.get(id).copied()
}

fn set_last_selection(id: &str, r: Rect) {
    if let Ok(mut m) = LAST_SELECTION.lock() {
        m.get_or_insert_with(HashMap::new).insert(id.to_string(), r);
    }
}

const CARD_DELAY: Duration = Duration::from_millis(500);
const CARD_HIDE: Duration = Duration::from_millis(400);
const COALESCE: Duration = Duration::from_millis(600);

pub struct CaptureView {
    pub monitor: String,
    base: Base,
    window_rects: Vec<Rect>,
    theme: Theme,
    models_ready: bool,
    api_key_present: bool,

    has_selection: bool,
    selection: Rect,
    hover_rect: Option<Rect>,
    drag: Drag,
    mouse_down_point: Point,
    did_drag: bool,
    shift_down: bool,
    mouse: Point,

    items: Vec<AnnotationItem>,
    next_id: u64,
    selected_id: Option<u64>,
    hovered_id: Option<u64>,
    draft: Option<AnnotationItem>,
    undo_stack: Vec<Vec<AnnotationItem>>,
    redo_stack: Vec<Vec<AnnotationItem>>,
    change_start: Option<Vec<AnnotationItem>>,
    coalesce_deadline: Option<Instant>,
    scroll_accumulator: f32,
    tool: Option<Tool>,
    text_edit: Option<TextEdit>,
    captioned_number_id: Option<u64>,

    recognition: Option<(Rect, Vec<OcrLine>)>,
    translation: Translation,
    busy: Option<Purpose>,
    pending_blocks: Vec<TextBlock>,
    pending_rect: Rect,
    shimmer_start: Instant,
    ocr_boxes_visible: bool,

    toolbar_origin: Point,
    toolbar_vertical: bool,
    toolbar_side: Option<CaretEdge>,
    hovered_button: Option<ToolbarAction>,
    hover_since: Option<Instant>,
    hide_card_at: Option<Instant>,
    card: Option<Card>,
    card_origin: Point,
    style_frame: Option<(Rect, CaretEdge, f32)>,
    hovered_style: Option<usize>,
    top_bar: Option<Point>,
    toast: Option<Toast>,
    ocr_panel: Option<OcrPanel>,
    ocr_hover: Option<u8>,
    magnifier: Option<(Point, Option<String>)>,

    cursor: Cursor,
    effects: Vec<Effect>,
    /// Chrome and focus rects drawn in the last frame, so the next repaint also clears where they were.
    last_frame: Rect,
    dirty: bool,
}

impl CaptureView {
    /// `pixels` is the frozen screen at full resolution; `size` is the monitor in points (pixels / scale).
    pub fn new(monitor: String, pixels: Pixmap, size: Size, window_rects: Vec<Rect>, theme: Theme, models_ready: bool, api_key_present: bool) -> CaptureView {
        let bounds = Rect::new(0.0, 0.0, size.width, size.height);
        CaptureView {
            monitor,
            base: Base::new(pixels, bounds),
            window_rects,
            theme,
            models_ready,
            api_key_present,
            has_selection: false,
            selection: Rect::ZERO,
            hover_rect: None,
            drag: Drag::None,
            mouse_down_point: Point::ZERO,
            did_drag: false,
            shift_down: false,
            mouse: Point::ZERO,
            items: Vec::new(),
            next_id: 1,
            selected_id: None,
            hovered_id: None,
            draft: None,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            change_start: None,
            coalesce_deadline: None,
            scroll_accumulator: 0.0,
            tool: None,
            text_edit: None,
            captioned_number_id: None,
            recognition: None,
            translation: Translation::None,
            busy: None,
            pending_blocks: Vec::new(),
            pending_rect: Rect::NULL,
            shimmer_start: Instant::now(),
            ocr_boxes_visible: false,
            toolbar_origin: Point::ZERO,
            toolbar_vertical: false,
            toolbar_side: None,
            hovered_button: None,
            hover_since: None,
            hide_card_at: None,
            card: None,
            card_origin: Point::ZERO,
            style_frame: None,
            hovered_style: None,
            top_bar: None,
            toast: None,
            ocr_panel: None,
            ocr_hover: None,
            magnifier: None,
            cursor: Cursor::Crosshair,
            effects: Vec::new(),
            last_frame: Rect::NULL,
            dirty: true,
        }
    }

    // MARK: Accessors

    pub fn bounds(&self) -> Rect {
        self.base.bounds
    }

    /// Pixels per point.
    pub fn scale(&self) -> f32 {
        self.base.scale()
    }

    pub fn has_selection(&self) -> bool {
        self.has_selection
    }

    pub fn selection(&self) -> Option<Rect> {
        self.has_selection.then_some(self.selection)
    }

    pub fn items(&self) -> &[AnnotationItem] {
        &self.items
    }

    pub fn tool(&self) -> Option<Tool> {
        self.tool
    }

    pub fn cursor(&self) -> Cursor {
        self.cursor
    }

    #[cfg(test)]
    pub fn toast_text(&self) -> Option<&str> {
        self.toast.as_ref().map(|t| t.text.as_str())
    }

    pub fn ocr_text(&self) -> Option<&str> {
        self.ocr_panel.as_ref().map(|p| p.text.as_str())
    }

    pub fn is_editing_text(&self) -> bool {
        self.text_edit.is_some()
    }

    pub fn hover_card_visible(&self) -> bool {
        self.card.is_some()
    }

    pub fn magnifier_visible(&self) -> bool {
        self.magnifier.is_some()
    }

    pub fn toolbar_frame(&self) -> Option<Rect> {
        self.toolbar_visible().then(|| Rect::from_origin_size(self.toolbar_origin, chrome::toolbar_size(self.toolbar_vertical)))
    }

    pub fn toolbar_vertical(&self) -> bool {
        self.toolbar_vertical
    }

    /// Center of the toolbar button for `action`, in view points (for tests and demos).
    pub fn toolbar_button_center(&self, action: ToolbarAction) -> Option<Point> {
        let i = chrome::toolbar_actions().iter().position(|a| *a == action)?;
        Some(chrome::toolbar_button(i, self.toolbar_vertical).offset(self.toolbar_origin.x, self.toolbar_origin.y).center())
    }

    /// Center of the style bar item that performs `action`, in view points.
    pub fn style_button_center(&self, action: StyleAction) -> Option<Point> {
        let (frame, edge, _) = self.style_frame?;
        let origin = chrome::style_content_origin(&frame, Some(edge));
        let state = self.current_style()?;
        chrome::style_items(&state).0.into_iter().find_map(|i| match i {
            StyleItem::Button { rect, action: a, .. } if a == action => Some(rect.offset(origin.x, origin.y).center()),
            _ => None,
        })
    }

    pub fn drain_effects(&mut self) -> Vec<Effect> {
        std::mem::take(&mut self.effects)
    }

    fn measure(&self) -> FontMeasure {
        FontMeasure
    }

    fn item(&self, id: Option<u64>) -> Option<&AnnotationItem> {
        let id = id?;
        self.items.iter().find(|i| i.id == id)
    }

    fn selected_index(&self) -> Option<usize> {
        let id = self.selected_id?;
        self.items.iter().position(|i| i.id == id)
    }

    fn new_id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }

    fn changed(&mut self) {
        self.dirty = true;
    }

    fn is_selecting(&self) -> bool {
        matches!(self.drag, Drag::Selecting(_)) && self.did_drag
    }

    fn is_adjusting_selection(&self) -> bool {
        matches!(self.drag, Drag::Moving(..) | Drag::Resizing(..))
    }

    fn focus_rect(&self) -> Option<Rect> {
        if self.has_selection || self.is_selecting() {
            return (self.selection.width > 0.0).then_some(self.selection);
        }
        self.hover_rect
    }

    fn visible_translation(&self) -> &[TranslatedBlock] {
        match &self.translation {
            Translation::Shown(blocks, _) => blocks,
            _ => &[],
        }
    }

    fn toolbar_visible(&self) -> bool {
        self.has_selection && !self.is_adjusting_selection()
    }

    fn clamp_to_bounds(&self, p: Point) -> Point {
        let b = self.bounds();
        Point::new(p.x.clamp(b.min_x(), b.max_x() - 0.01), p.y.clamp(b.min_y(), b.max_y() - 0.01))
    }

    fn clamp_to_selection(&self, p: Point) -> Point {
        let s = self.selection;
        Point::new(p.x.clamp(s.min_x(), s.max_x()), p.y.clamp(s.min_y(), s.max_y()))
    }

    // MARK: Toast

    pub fn show_message(&mut self, text: &str) {
        self.show_toast(text, Some(Duration::from_millis(2500)));
    }

    fn show_toast(&mut self, text: &str, duration: Option<Duration>) {
        self.toast = Some(Toast { text: text.to_string(), until: duration.map(|d| Instant::now() + d) });
        self.changed();
    }

    fn hide_toast(&mut self) {
        if self.toast.take().is_some() {
            self.changed();
        }
    }

    // MARK: Timers

    /// Whether anything is waiting on time (toasts, the hover card, shimmer, undo coalescing).
    pub fn wants_ticks(&self) -> bool {
        self.toast.as_ref().is_some_and(|t| t.until.is_some())
            || self.hover_since.is_some()
            || self.hide_card_at.is_some()
            || self.coalesce_deadline.is_some()
            || matches!(self.translation, Translation::Loading) && !self.pending_blocks.is_empty()
            || self.ocr_panel.as_ref().is_some_and(|p| p.copied_until.is_some())
    }

    pub fn tick(&mut self, now: Instant) {
        if self.toast.as_ref().and_then(|t| t.until).is_some_and(|u| now >= u) {
            self.hide_toast();
        }
        if let Some(since) = self.hover_since {
            if now >= since + CARD_DELAY {
                self.hover_since = None;
                if let Some(action) = self.hovered_button {
                    self.show_card(action);
                }
            }
        }
        if self.hide_card_at.is_some_and(|t| now >= t) {
            self.hide_card_at = None;
            if self.card.as_ref().is_some_and(|c| !c.recording) {
                self.card = None;
                self.changed();
            }
        }
        if self.coalesce_deadline.is_some_and(|t| now >= t) {
            self.end_change();
        }
        if matches!(self.translation, Translation::Loading) && !self.pending_blocks.is_empty() {
            self.changed();
        }
        if let Some(p) = &mut self.ocr_panel {
            if p.copied_until.is_some_and(|t| now >= t) {
                p.copied_until = None;
                self.dirty = true;
            }
        }
    }

    // MARK: Undo

    fn begin_change(&mut self) {
        if self.change_start.is_none() {
            self.change_start = Some(self.items.clone());
        }
    }

    fn end_change(&mut self) {
        self.coalesce_deadline = None;
        if let Some(start) = self.change_start.take() {
            if start != self.items {
                self.undo_stack.push(start);
                if self.undo_stack.len() > 200 {
                    self.undo_stack.remove(0);
                }
                self.redo_stack.clear();
            }
        }
        self.changed();
    }

    /// Groups rapid changes (scrolling, arrow nudges, color picking) into one undo step.
    fn coalesced_change(&mut self, body: impl FnOnce(&mut CaptureView)) {
        self.begin_change();
        body(self);
        self.coalesce_deadline = Some(Instant::now() + COALESCE);
    }

    fn mutate(&mut self, body: impl FnOnce(&mut CaptureView)) {
        self.begin_change();
        body(self);
        self.end_change();
    }

    fn can_undo(&self) -> bool {
        !self.undo_stack.is_empty() || matches!(self.translation, Translation::Shown(..))
    }

    pub fn undo(&mut self) {
        self.commit_text();
        self.end_change();
        if let Some(previous) = self.undo_stack.pop() {
            self.redo_stack.push(std::mem::replace(&mut self.items, previous));
            if self.item(self.selected_id).is_none() {
                self.selected_id = None;
            }
        } else if let Translation::Shown(blocks, rect) = std::mem::replace(&mut self.translation, Translation::None) {
            self.translation = Translation::Hidden(blocks, rect);
        }
        self.layout_chrome();
    }

    pub fn redo(&mut self) {
        self.commit_text();
        self.end_change();
        let Some(next) = self.redo_stack.pop() else { return };
        self.undo_stack.push(std::mem::replace(&mut self.items, next));
        if self.item(self.selected_id).is_none() {
            self.selected_id = None;
        }
        self.layout_chrome();
    }

    // MARK: Hit testing

    fn hit_item(&self, p: Point) -> Option<AnnotationItem> {
        if !self.selection.inset(-6.0, -6.0).contains(p) {
            return None;
        }
        // A freehand tool always paints, so strokes can be layered without selecting what is underneath.
        if self.tool.is_some_and(|t| t.is_freehand()) {
            return None;
        }
        let editing = self.text_edit.as_ref().and_then(|e| e.editing_id);
        self.items.iter().rev().find(|i| Some(i.id) != editing && i.contains(p, &self.measure())).cloned()
    }

    fn item_handle(&self, p: Point) -> Option<ItemHandle> {
        let item = self.item(self.selected_id)?;
        item.handles().into_iter().find(|(_, h)| (h.x - p.x).abs() <= 7.0 && (h.y - p.y).abs() <= 7.0).map(|h| h.0)
    }

    fn selection_handle(&self, p: Point) -> Option<ResizeHandle> {
        ResizeHandle::ALL.into_iter().find(|h| {
            let q = h.point(&self.selection);
            (q.x - p.x).abs() <= 8.0 && (q.y - p.y).abs() <= 8.0
        })
    }

    fn hover_target(&self, p: Point) -> Option<Rect> {
        self.window_rects.iter().find(|r| r.contains(p)).copied()
    }

    fn select(&mut self, id: Option<u64>) {
        if self.selected_id != id {
            self.selected_id = id;
            self.layout_chrome();
        }
    }

    fn replace_item(&mut self, item: AnnotationItem) {
        if let Some(i) = self.items.iter().position(|x| x.id == item.id) {
            self.items[i] = item;
            self.changed();
        }
    }

    // MARK: Mouse

    /// Positions the magnifier and hover state for the pointer at `p`, e.g. when the overlay appears.
    pub fn prime_cursor(&mut self, p: Point) {
        self.mouse = self.clamp_to_bounds(p);
        self.handle_mouse_moved(self.mouse);
    }

    /// The pointer left this monitor while another one owns the selection.
    pub fn hide_magnifier(&mut self) {
        if self.magnifier.take().is_some() {
            self.changed();
        }
    }

    pub fn mouse_move(&mut self, p: Point, mods: Mods) {
        let p = self.clamp_to_bounds(p);
        self.mouse = p;
        match self.drag {
            Drag::None | Drag::Toolbar(_) | Drag::Chrome => self.handle_mouse_moved(p),
            _ => self.mouse_dragged(p, mods),
        }
    }

    fn chrome_hover(&mut self, p: Point) -> bool {
        // Hover card first: it floats over everything.
        if let Some(card) = &self.card {
            let size = chrome::hover_card_layout(&self.card_view(card)).size;
            if Rect::from_origin_size(self.card_origin, size).contains(p) {
                self.hide_card_at = None;
                self.hovered_button = None;
                self.hover_since = None;
                self.cursor = Cursor::Arrow;
                return true;
            }
        }
        let mut over_chrome = false;
        let button = self.toolbar_frame().filter(|f| f.contains(p)).and_then(|_| {
            chrome::toolbar_actions().into_iter().enumerate().find_map(|(i, a)| {
                chrome::toolbar_button(i, self.toolbar_vertical).offset(self.toolbar_origin.x, self.toolbar_origin.y).contains(p).then_some(a)
            })
        });
        if self.toolbar_frame().is_some_and(|f| f.contains(p)) {
            over_chrome = true;
        }
        if button != self.hovered_button {
            self.hovered_button = button;
            self.changed();
            match button {
                Some(action) => {
                    self.hide_card_at = None;
                    if self.card.is_some() {
                        // Once a card is up, moving to the next button switches it at once.
                        self.show_card(action);
                    } else {
                        self.hover_since = Some(Instant::now());
                    }
                }
                None => {
                    self.hover_since = None;
                    if self.card.as_ref().is_some_and(|c| !c.recording) {
                        self.hide_card_at = Some(Instant::now() + CARD_HIDE);
                    }
                }
            }
        }
        let style_hover = self.style_frame.and_then(|(frame, edge, _)| {
            if !frame.contains(p) {
                return None;
            }
            over_chrome = true;
            let origin = chrome::style_content_origin(&frame, Some(edge));
            let state = self.current_style()?;
            chrome::style_items(&state).0.iter().position(|i| matches!(i, StyleItem::Button { rect, .. } if rect.offset(origin.x, origin.y).contains(p)))
        });
        if style_hover != self.hovered_style {
            self.hovered_style = style_hover;
            self.changed();
        }
        let ocr_hover = self.ocr_panel.as_ref().and_then(|panel| {
            let frame = Rect::from_origin_size(panel.origin, chrome::OCR_PANEL);
            if !frame.contains(p) {
                return None;
            }
            over_chrome = true;
            if chrome::ocr_close_button().offset(panel.origin.x, panel.origin.y).contains(p) {
                Some(0)
            } else if chrome::ocr_copy_button().offset(panel.origin.x, panel.origin.y).contains(p) {
                Some(1)
            } else {
                Some(2)
            }
        });
        if ocr_hover != self.ocr_hover {
            self.ocr_hover = ocr_hover;
            self.changed();
        }
        if over_chrome {
            self.cursor = Cursor::Arrow;
        }
        over_chrome
    }

    fn handle_mouse_moved(&mut self, p: Point) {
        if self.chrome_hover(p) {
            if self.magnifier.take().is_some() {
                self.changed();
            }
            if self.hovered_id.take().is_some() {
                self.changed();
            }
            return;
        }
        if !self.has_selection {
            let hover = self.hover_target(p);
            if hover != self.hover_rect {
                self.hover_rect = hover;
                self.layout_chrome();
            }
            self.show_magnifier(p, None);
            self.cursor = Cursor::Crosshair;
            return;
        }
        let hovered = if self.text_edit.is_none() { self.hit_item(p).map(|i| i.id) } else { None };
        if hovered != self.hovered_id {
            self.hovered_id = hovered;
            self.changed();
        }
        self.cursor = if let Some(handle) = self.item_handle(p) {
            match handle {
                ItemHandle::Rect(h) => h.cursor(),
                _ => Cursor::Crosshair,
            }
        } else if hovered.is_some() {
            Cursor::OpenHand
        } else if let Some(h) = self.selection_handle(p) {
            h.cursor()
        } else if self.selection.contains(p) {
            match self.tool {
                Some(Tool::Text) => Cursor::IBeam,
                Some(_) => Cursor::Crosshair,
                None => Cursor::OpenHand,
            }
        } else {
            Cursor::Arrow
        };
    }

    fn show_magnifier(&mut self, p: Point, size_text: Option<String>) {
        self.magnifier = Some((p, size_text));
        self.changed();
    }

    fn size_text(r: &Rect) -> String {
        chrome::size_text(r.size())
    }

    /// Handles a press. `clicks` is 2 for a double-click.
    pub fn mouse_down(&mut self, p: Point, button: MouseButton, clicks: u32, mods: Mods) {
        let p = self.clamp_to_bounds(p);
        match button {
            MouseButton::Right => return self.right_mouse_down(),
            MouseButton::Left => {}
        }
        if self.chrome_mouse_down(p) {
            return;
        }
        self.mouse_down_point = p;
        self.did_drag = false;
        if self.text_edit.is_some() {
            let was_caption = self.captioned_number_id.is_some();
            self.commit_text();
            // With the number tool, the click that ends a caption also places the next number.
            if !(was_caption && self.tool == Some(Tool::Number)) {
                return;
            }
            self.select(None);
        }

        if !self.has_selection {
            self.drag = Drag::Selecting(p);
            return;
        }

        let hit = self.hit_item(p);
        if clicks == 2 {
            if let Some(hit) = &hit {
                if matches!(hit.shape, Shape::Text { .. }) {
                    let hit = hit.clone();
                    self.begin_text_editing(None, Some(&hit));
                    return;
                }
            }
            if hit.is_none() && self.tool.is_none() && self.selection.contains(p) {
                self.finish(Output::Copy);
                return;
            }
        }
        if let (Some(item), Some(handle)) = (self.item(self.selected_id).cloned(), self.item_handle(p)) {
            self.begin_change();
            self.drag = Drag::ResizingItem(handle, item, p);
            return;
        }
        if hit.is_none() {
            if let Some(handle) = self.selection_handle(p) {
                self.drag = Drag::Resizing(handle, self.selection, p);
                return;
            }
        }
        if let Some(hit) = hit {
            self.select(Some(hit.id));
            self.begin_change();
            self.drag = Drag::MovingItem(p, hit);
            self.cursor = Cursor::ClosedHand;
            return;
        }
        self.select(None);
        let _ = mods;
        if let (Some(tool), true) = (self.tool, self.selection.contains(p)) {
            self.begin_annotation(tool, p);
        } else if self.selection.contains(p) {
            self.drag = Drag::Moving(p, self.selection);
            self.cursor = Cursor::ClosedHand;
        } else if self.items.is_empty() && matches!(self.translation, Translation::None) {
            // Dragging outside an untouched selection starts a new one.
            self.reset_selection();
            self.drag = Drag::Selecting(p);
        }
    }

    /// Presses on panels: buttons act, everything else is swallowed. Returns whether the press was on chrome.
    fn chrome_mouse_down(&mut self, p: Point) -> bool {
        if let Some(card) = &self.card {
            let view = self.card_view(card);
            let l = chrome::hover_card_layout(&view);
            if Rect::from_origin_size(self.card_origin, l.size).contains(p) {
                let editable = card.action.key_id().is_some();
                if editable && !card.recording && l.keycap.offset(self.card_origin.x, self.card_origin.y).inset(-4.0, -4.0).contains(p) {
                    if let Some(c) = &mut self.card {
                        c.recording = true;
                    }
                    self.hide_card_at = None;
                    self.changed();
                }
                self.drag = Drag::Chrome;
                return true;
            }
        }
        // A click anywhere else stops waiting for a key.
        self.stop_recording();
        if let Some(frame) = self.toolbar_frame() {
            if frame.contains(p) {
                self.drag = match self.hovered_button.or_else(|| self.button_at(p)) {
                    Some(action) => Drag::Toolbar(action),
                    None => Drag::Chrome,
                };
                self.changed();
                return true;
            }
        }
        if let Some((frame, edge, _)) = self.style_frame {
            if frame.contains(p) {
                let origin = chrome::style_content_origin(&frame, Some(edge));
                if let Some(state) = self.current_style() {
                    let action = chrome::style_items(&state).0.into_iter().find_map(|i| match i {
                        StyleItem::Button { rect, action, .. } if rect.offset(origin.x, origin.y).contains(p) => Some(action),
                        _ => None,
                    });
                    if let Some(action) = action {
                        self.apply_style(action, false);
                    }
                }
                self.drag = Drag::Chrome;
                return true;
            }
        }
        if let Some(panel) = &self.ocr_panel {
            let origin = panel.origin;
            if Rect::from_origin_size(origin, chrome::OCR_PANEL).contains(p) {
                if chrome::ocr_close_button().offset(origin.x, origin.y).contains(p) {
                    self.close_ocr_panel();
                } else if chrome::ocr_copy_button().offset(origin.x, origin.y).contains(p) {
                    self.effects.push(Effect::CopyOcrPanelText);
                    if let Some(panel) = &mut self.ocr_panel {
                        panel.copied_until = Some(Instant::now() + Duration::from_millis(1500));
                    }
                    self.changed();
                }
                self.drag = Drag::Chrome;
                return true;
            }
        }
        false
    }

    fn button_at(&self, p: Point) -> Option<ToolbarAction> {
        chrome::toolbar_actions().into_iter().enumerate().find_map(|(i, a)| {
            chrome::toolbar_button(i, self.toolbar_vertical).offset(self.toolbar_origin.x, self.toolbar_origin.y).contains(p).then_some(a)
        })
    }

    fn mouse_dragged(&mut self, p: Point, mods: Mods) {
        if !self.did_drag && p.distance(self.mouse_down_point) > 2.0 {
            self.did_drag = true;
        }
        match self.drag.clone() {
            Drag::None | Drag::Toolbar(_) | Drag::Chrome => {}
            Drag::Selecting(start) => {
                if !self.did_drag {
                    return;
                }
                let mut end = p;
                if mods.shift {
                    let side = (p.x - start.x).abs().max((p.y - start.y).abs());
                    end = self.clamp_to_bounds(Point::new(
                        start.x + if p.x >= start.x { side } else { -side },
                        start.y + if p.y >= start.y { side } else { -side },
                    ));
                }
                self.selection = Rect::from_corners(start, end);
                self.hover_rect = None;
                let text = Self::size_text(&self.selection);
                self.show_magnifier(end, Some(text));
                self.layout_chrome();
            }
            Drag::Moving(start, original) => {
                let b = self.bounds();
                let mut r = original.offset(p.x - start.x, p.y - start.y);
                r.x = r.x.clamp(0.0, (b.width - r.width).max(0.0));
                r.y = r.y.clamp(0.0, (b.height - r.height).max(0.0));
                self.selection = r;
                self.layout_chrome();
            }
            Drag::Resizing(handle, original, start) => {
                self.selection = handle.resize(&original, p - start).intersection(&self.bounds());
                if self.selection.is_null() {
                    self.selection = Rect::new(p.x, p.y, 0.0, 0.0);
                }
                let text = Self::size_text(&self.selection);
                self.show_magnifier(p, Some(text));
                self.layout_chrome();
            }
            Drag::Drawing(start) => {
                let q = self.clamp_to_selection(p);
                self.update_draft(start, q, mods.shift);
                self.changed();
            }
            Drag::MovingItem(start, original) => self.replace_item(original.moved(p - start)),
            Drag::ResizingItem(handle, original, start) => self.replace_item(original.resized(handle, p - start)),
        }
    }

    pub fn mouse_up(&mut self, p: Point, _mods: Mods) {
        let p = self.clamp_to_bounds(p);
        let finished = std::mem::replace(&mut self.drag, Drag::None);
        match finished {
            Drag::None | Drag::Chrome => {}
            Drag::Toolbar(action) => {
                self.changed();
                if self.button_at(p) == Some(action) {
                    self.handle_action(action);
                }
            }
            Drag::Selecting(_) => {
                if self.did_drag {
                    if self.selection.width < 4.0 || self.selection.height < 4.0 {
                        self.selection = Rect::ZERO;
                        self.layout_chrome();
                        return;
                    }
                } else {
                    // A click picks the window under the cursor, or the whole screen.
                    self.selection = self.hover_rect.map(|r| r.intersection(&self.bounds())).filter(|r| !r.is_null()).unwrap_or(self.bounds());
                }
                self.commit_selection();
            }
            Drag::Moving(..) | Drag::Resizing(..) => {
                if self.selection.width < 4.0 || self.selection.height < 4.0 {
                    self.selection = Rect::new(self.selection.x, self.selection.y, self.selection.width.max(4.0), self.selection.height.max(4.0));
                }
                self.magnifier = None;
                self.layout_chrome();
            }
            Drag::Drawing(_) => {
                if let Some(draft) = self.draft.take() {
                    if draft.is_meaningful() {
                        let id = draft.id;
                        self.mutate(|v| v.items.push(draft));
                        self.selected_id = Some(id);
                    }
                }
                self.layout_chrome();
            }
            Drag::MovingItem(..) | Drag::ResizingItem(..) => {
                self.end_change();
                self.handle_mouse_moved(p);
            }
        }
    }

    fn right_mouse_down(&mut self) {
        if self.text_edit.is_some() {
            self.commit_text();
        } else if !self.has_selection {
            self.effects.push(Effect::Close);
        } else if self.selected_id.is_some() {
            self.select(None);
        } else if self.tool.is_some() {
            self.set_tool(None);
        } else if self.items.is_empty() && matches!(self.translation, Translation::None) {
            self.reset_selection();
            let m = self.mouse;
            self.handle_mouse_moved(m);
        }
    }

    /// Wheel `lines` (one notch = 1, negative = toward the user) changes the size of the tool or selected annotation.
    pub fn wheel(&mut self, lines: f32, _mods: Mods) {
        if !self.has_selection {
            return;
        }
        let Some(tool) = (if self.text_edit.is_some() { Some(Tool::Text) } else { self.item(self.selected_id).map(|i| i.tool()).or(self.tool) }) else { return };
        if tool == Tool::Mosaic {
            let brush = self.item(self.selected_id).map_or(settings::get().mosaic_mode == MosaicMode::Brush, |i| i.shape.is_mosaic_brush());
            if !brush {
                return;
            }
        }
        self.scroll_accumulator += lines;
        if self.scroll_accumulator.abs() < 1.0 {
            return;
        }
        let steps = self.scroll_accumulator.trunc();
        self.scroll_accumulator -= steps;
        let unit = if matches!(tool, Tool::Mosaic | Tool::Text | Tool::Number) { 2.0 } else { 1.0 };
        let current = self.current_style().map_or(tool.default_size(), |s| s.size);
        self.apply_style(StyleAction::Size(current + steps * unit), true);
    }

    fn commit_selection(&mut self) {
        self.has_selection = true;
        self.hover_rect = None;
        self.magnifier = None;
        self.layout_chrome();
    }

    fn reset_selection(&mut self) {
        self.commit_text();
        self.busy = None;
        self.has_selection = false;
        self.selection = Rect::ZERO;
        self.tool = None;
        self.items.clear();
        self.undo_stack.clear();
        self.redo_stack.clear();
        self.selected_id = None;
        self.hovered_id = None;
        self.recognition = None;
        self.translation = Translation::None;
        self.pending_blocks.clear();
        self.close_ocr_panel();
        self.toast = None;
        self.layout_chrome();
    }

    // MARK: Annotations

    fn set_tool(&mut self, tool: Option<Tool>) {
        self.commit_text();
        self.tool = tool;
        self.select(None);
        self.layout_chrome();
    }

    fn begin_annotation(&mut self, tool: Tool, p: Point) {
        let color = settings::color_for(tool);
        let size = settings::size_for(tool);
        let style = settings::style_for(tool);
        let id = self.new_id();
        let new = |shape: Shape| AnnotationItem { id, shape, color, size, effect: MosaicEffect::Pixelate, style };
        match tool {
            Tool::Text => self.begin_text_editing(Some(p), None),
            Tool::Number => {
                let item = new(Shape::Number(p));
                let item_id = item.id;
                self.mutate(|v| v.items.push(item));
                // A number is usually followed by its explanation, so start typing right beside it.
                self.begin_text_editing(Some(Point::new(p.x + size / 2.0 + 6.0, p.y)), None);
                self.captioned_number_id = Some(item_id);
            }
            Tool::Pen => {
                self.draft = Some(new(Shape::Pen(vec![p])));
                self.drag = Drag::Drawing(p);
            }
            Tool::Magnifier => {
                self.draft = Some(AnnotationItem { style: ItemStyle::default(), ..new(Shape::Magnifier { source: p, target: p, radius: 0.0 }) });
                self.drag = Drag::Drawing(p);
            }
            Tool::Mosaic => {
                let s = settings::get();
                let shape = if s.mosaic_mode == MosaicMode::Brush { Shape::MosaicBrush(vec![p]) } else { Shape::MosaicRect(Rect::new(p.x, p.y, 0.0, 0.0)) };
                self.draft = Some(AnnotationItem { effect: s.mosaic_effect, style: ItemStyle::default(), ..new(shape) });
                self.drag = Drag::Drawing(p);
                self.changed();
            }
            Tool::Rectangle => {
                self.draft = Some(new(Shape::Rectangle(Rect::new(p.x, p.y, 0.0, 0.0))));
                self.drag = Drag::Drawing(p);
            }
            Tool::Arrow => {
                self.draft = Some(new(Shape::Arrow(p, p)));
                self.drag = Drag::Drawing(p);
            }
        }
    }

    fn update_draft(&mut self, start: Point, p: Point, shift: bool) {
        let selection = self.selection;
        let Some(item) = &mut self.draft else { return };
        match &mut item.shape {
            Shape::Pen(points) => {
                if shift {
                    let first = points[0];
                    *points = vec![first, p];
                } else if points.last().is_some_and(|l| l.distance(p) >= 1.0) {
                    points.push(p);
                }
            }
            Shape::MosaicBrush(points) => {
                if points.last().is_some_and(|l| l.distance(p) >= 1.0) {
                    points.push(p);
                }
            }
            Shape::Rectangle(r) | Shape::MosaicRect(r) => {
                let mut end = p;
                if shift {
                    let side = (p.x - start.x).abs().max((p.y - start.y).abs());
                    end = Point::new(start.x + if p.x >= start.x { side } else { -side }, start.y + if p.y >= start.y { side } else { -side });
                }
                *r = Rect::from_corners(start, end);
            }
            Shape::Arrow(a, b) => {
                let mut end = p;
                if shift {
                    // Snap to 45° steps.
                    let step = std::f32::consts::FRAC_PI_4;
                    let angle = ((p.y - start.y).atan2(p.x - start.x) / step).round() * step;
                    let length = p.distance(start);
                    end = Point::new(start.x + angle.cos() * length, start.y + angle.sin() * length);
                }
                *a = start;
                *b = end;
            }
            Shape::Magnifier { source, target, radius } => {
                let r = p.distance(start);
                *source = start;
                *radius = r;
                *target = lens_center(start, r, &selection);
            }
            Shape::Text { .. } | Shape::Number(_) => {}
        }
    }

    fn delete_selected_item(&mut self) {
        let Some(id) = self.selected_id else { return };
        self.mutate(|v| v.items.retain(|i| i.id != id));
        self.selected_id = None;
        self.hovered_id = None;
        self.layout_chrome();
    }

    // MARK: Text

    fn begin_text_editing(&mut self, at: Option<Point>, existing: Option<&AnnotationItem>) {
        self.commit_text();
        self.begin_change();
        let mut origin = at.unwrap_or(Point::ZERO);
        let mut text = String::new();
        let (color, size, style, editing_id);
        if let Some(Shape::Text { text: t, origin: o, .. }) = existing.map(|e| &e.shape) {
            let e = existing.unwrap();
            origin = *o;
            text = t.clone();
            color = e.color;
            size = e.size;
            style = e.style;
            editing_id = Some(e.id);
            self.selected_id = None;
        } else {
            color = settings::color_for(Tool::Text);
            size = settings::size_for(Tool::Text);
            style = settings::style_for(Tool::Text);
            editing_id = None;
            origin.y -= size * 0.6;
        }
        let wrap = (self.selection.max_x() - origin.x - 6.0).max(60.0);
        let len = text.len();
        self.text_edit = Some(TextEdit { origin, wrap, text: text.clone(), start: len, end: len, editing_id, color, size, style, composition: String::new() });
        let caret = self.caret_rect().unwrap_or(Rect::ZERO);
        self.effects.push(Effect::TextInput(Some(TextInput { caret, initial: Some(text), font_size: size })));
        self.layout_chrome();
    }

    /// Text and selection (byte offsets) from the platform's edit box.
    pub fn text_changed(&mut self, text: &str, start: usize, end: usize) {
        let Some(e) = &mut self.text_edit else { return };
        e.text = text.replace("\r\n", "\n");
        e.start = start.min(e.text.len());
        e.end = end.min(e.text.len());
        self.emit_text_input();
        self.changed();
    }

    /// Tells the platform where the caret is now (for the IME window) and the font size.
    fn emit_text_input(&mut self) {
        let Some(size) = self.text_edit.as_ref().map(|e| e.size) else { return };
        let caret = self.caret_rect().unwrap_or(Rect::ZERO);
        self.effects.push(Effect::TextInput(Some(TextInput { caret, initial: None, font_size: size })));
    }

    /// The IME's uncommitted composition string (empty when none).
    pub fn composition_changed(&mut self, composition: &str) {
        let Some(e) = &mut self.text_edit else { return };
        if e.composition != composition {
            e.composition = composition.to_string();
            self.changed();
        }
    }

    /// The text as shown while editing: the composition sits at the caret. Returns it and the composition's byte range.
    fn display_text(e: &TextEdit) -> (String, usize, usize) {
        let at = e.end.min(e.text.len());
        let mut s = String::with_capacity(e.text.len() + e.composition.len());
        s.push_str(&e.text[..at]);
        s.push_str(&e.composition);
        s.push_str(&e.text[at..]);
        (s, at, at + e.composition.len())
    }

    /// Where the text being edited is drawn: origin and wrapped size, like the macOS editor view's frame.
    fn editor_frame(&self) -> Option<Rect> {
        let e = self.text_edit.as_ref()?;
        let (shown, _, _) = Self::display_text(e);
        let (w, h) = text::measure(&shown, e.size, annotation_weight(), Some(e.wrap));
        let (_, line_height) = text::metrics(e.size, annotation_weight());
        Some(Rect::new(e.origin.x, e.origin.y, (w + 2.0).max(24.0).min(e.wrap), h.max(line_height)))
    }

    fn caret_rect(&self) -> Option<Rect> {
        let e = self.text_edit.as_ref()?;
        let (shown, _, comp_end) = Self::display_text(e);
        let l = text::layout(&shown, e.size, annotation_weight(), Some(e.wrap));
        let (x, top) = l.caret(comp_end);
        Some(Rect::new(e.origin.x + x, e.origin.y + top, 1.5, l.line_height))
    }

    /// Ends text editing (Esc, Ctrl+Enter, or a click elsewhere): keeps the text as an annotation.
    pub fn commit_text(&mut self) {
        let Some(e) = self.text_edit.take() else { return };
        let text = e.text.trim().to_string();
        let shape = Shape::Text { text: text.clone(), origin: e.origin, width: e.wrap };
        if let Some(id) = e.editing_id {
            if let Some(i) = self.items.iter().position(|x| x.id == id) {
                if text.is_empty() {
                    self.items.remove(i);
                } else {
                    let item = &mut self.items[i];
                    item.shape = shape;
                    item.color = e.color;
                    item.size = e.size;
                    item.style = e.style;
                    self.selected_id = Some(id);
                }
            }
        } else if !text.is_empty() {
            let id = self.new_id();
            self.items.push(AnnotationItem { id, shape, color: e.color, size: e.size, effect: MosaicEffect::Pixelate, style: e.style });
            self.selected_id = Some(id);
        }
        if let Some(id) = self.captioned_number_id.take() {
            if text.is_empty() && self.items.iter().any(|i| i.id == id) {
                self.selected_id = Some(id);
            }
        }
        self.effects.push(Effect::TextInput(None));
        self.end_change();
        self.layout_chrome();
    }

    // MARK: Style

    fn current_style(&self) -> Option<StyleState> {
        let s = settings::get();
        if let Some(e) = &self.text_edit {
            return Some(StyleState { tool: Tool::Text, color: e.color, size: e.size, mosaic_mode: s.mosaic_mode, mosaic_effect: s.mosaic_effect, options: e.style });
        }
        if let Some(item) = self.item(self.selected_id) {
            return Some(StyleState {
                tool: item.tool(),
                color: item.color,
                size: item.size,
                mosaic_mode: if item.shape.is_mosaic_brush() { MosaicMode::Brush } else { MosaicMode::Rect },
                mosaic_effect: item.effect,
                options: item.style,
            });
        }
        let tool = self.tool?;
        Some(StyleState {
            tool,
            color: settings::color_for(tool),
            size: settings::size_for(tool),
            mosaic_mode: s.mosaic_mode,
            mosaic_effect: s.mosaic_effect,
            options: settings::style_for(tool),
        })
    }

    /// The color picker's result.
    pub fn apply_custom_color(&mut self, color: Color) {
        self.apply_style(StyleAction::Color(color), true);
    }

    pub fn apply_style(&mut self, action: StyleAction, coalesce: bool) {
        if action == StyleAction::CustomColor {
            let current = self.current_style().map_or(settings::color_for(self.tool.unwrap_or(Tool::Rectangle)), |s| s.color);
            self.effects.push(Effect::PickColor(current));
            return;
        }
        let Some(style_tool) = (if self.text_edit.is_some() { Some(Tool::Text) } else { self.item(self.selected_id).map(|i| i.tool()).or(self.tool) }) else { return };
        let clamped = match action {
            StyleAction::Size(v) => {
                let (lo, hi) = style_tool.size_range();
                Some(v.clamp(lo, hi))
            }
            _ => None,
        };
        // Remember the choice for the next annotation of this kind.
        match action {
            StyleAction::Size(_) => settings::set_size(style_tool, clamped.unwrap_or(0.0)),
            StyleAction::Color(c) => settings::set_color(style_tool, c),
            StyleAction::MosaicMode(m) => settings::update(|s| s.mosaic_mode = m),
            StyleAction::MosaicEffect(e) => settings::update(|s| s.mosaic_effect = e),
            _ => {}
        }
        let mut new_options = None;
        if let Some(mut options) = self.current_style().map(|s| s.options) {
            let changed = match action {
                StyleAction::Dash(d) => {
                    options.dash = d;
                    true
                }
                StyleAction::ArrowHead(h) => {
                    options.arrow_head = h;
                    true
                }
                StyleAction::ToggleRounded => {
                    options.rounded = !options.rounded;
                    true
                }
                StyleAction::Text(t) => {
                    options.text = t;
                    true
                }
                _ => false,
            };
            if changed {
                settings::set_style(style_tool, options);
                new_options = Some(options);
            }
        }

        if let Some(e) = &mut self.text_edit {
            if let Some(size) = clamped {
                e.size = size;
            }
            if let StyleAction::Color(c) = action {
                e.color = c;
            }
            if let Some(o) = new_options {
                e.style = o;
            }
            self.emit_text_input();
        } else if let Some(index) = self.selected_index() {
            let change = move |v: &mut CaptureView| {
                let mut item = v.items[index].clone();
                if let Some(size) = clamped {
                    item.size = size;
                }
                if let StyleAction::Color(c) = action {
                    item.color = c;
                }
                if let StyleAction::MosaicEffect(e) = action {
                    item.effect = e;
                }
                if let Some(o) = new_options {
                    item.style = o;
                }
                v.replace_item(item);
            };
            if coalesce { self.coalesced_change(change) } else { self.mutate(change) }
        }
        self.layout_chrome();
    }

    // MARK: Keyboard

    /// Handles a key press on the overlay (text typing goes to the platform's edit box instead).
    pub fn key_down(&mut self, key: Key, mods: Mods) {
        if self.card.as_ref().is_some_and(|c| c.recording) {
            return self.record_key(key, mods);
        }
        if key == Key::Escape {
            return self.handle_escape();
        }
        if self.text_edit.is_some() {
            if key == Key::Enter && mods.ctrl {
                self.commit_text();
            }
            return;
        }
        let arrow = match key {
            Key::Left => Some(Point::new(-1.0, 0.0)),
            Key::Right => Some(Point::new(1.0, 0.0)),
            Key::Up => Some(Point::new(0.0, -1.0)),
            Key::Down => Some(Point::new(0.0, 1.0)),
            _ => None,
        };
        if !self.has_selection {
            if let Some(d) = arrow {
                let step = if mods.shift { 10.0 } else { 1.0 };
                let target = self.clamp_to_bounds(Point::new(self.mouse.x + d.x * step, self.mouse.y + d.y * step));
                self.effects.push(Effect::WarpCursor(target));
                self.mouse = target;
                self.handle_mouse_moved(target);
            } else if key == Key::Enter {
                self.selection = self.hover_rect.map(|r| r.intersection(&self.bounds())).filter(|r| !r.is_null()).unwrap_or(self.bounds());
                self.commit_selection();
            } else if mods.is_empty() && key == Key::Char('r') {
                self.restore_last_selection();
            } else if mods.is_empty() && key == Key::Char('c') {
                self.copy_pixel_color();
            }
            return;
        }
        match key {
            Key::Enter => return self.finish(Output::Copy),
            Key::Backspace | Key::Delete => return self.delete_selected_item(),
            _ => {}
        }
        if let Some(d) = arrow {
            return self.handle_arrow(d, mods);
        }
        if mods == Mods::CTRL {
            match key {
                Key::Char('z') => self.undo(),
                Key::Char('y') => self.redo(),
                Key::Char('c') => self.finish(Output::Copy),
                Key::Char('s') => self.finish(Output::Save),
                Key::Char('t') => self.handle_action(ToolbarAction::Pin),
                _ => {}
            }
            return;
        }
        if mods == Mods::CTRL_SHIFT {
            match key {
                Key::Char('z') => self.redo(),
                Key::Char('s') => self.finish(Output::SaveAs),
                _ => {}
            }
            return;
        }
        if !mods.is_empty() {
            return;
        }
        if let Key::Char(c) = key {
            if let Some(action) = ToolbarAction::for_key(&c.to_string()) {
                self.handle_action(action);
            }
        }
    }

    /// Shift presses toggle the magnifier's color format.
    pub fn modifiers_changed(&mut self, mods: Mods) {
        if mods.shift && !self.shift_down && self.magnifier.is_some() && matches!(self.drag, Drag::None) {
            settings::update(|s| s.hex_color = !s.hex_color);
            self.changed();
        }
        self.shift_down = mods.shift;
    }

    /// Esc steps back one level at a time; it only closes the capture when there is nothing left to back out of.
    fn handle_escape(&mut self) {
        if self.text_edit.is_some() {
            self.commit_text();
        } else if self.ocr_panel.is_some() {
            self.close_ocr_panel();
        } else if self.selected_id.is_some() {
            self.select(None);
        } else if self.tool.is_some() {
            self.set_tool(None);
        } else {
            self.effects.push(Effect::Close);
        }
    }

    /// Arrow keys: nudge the selected annotation, or move / expand (Ctrl) / shrink (Shift) the selection by 1pt.
    fn handle_arrow(&mut self, d: Point, mods: Mods) {
        if let Some(index) = self.selected_index() {
            let step = if mods.shift { 10.0 } else { 1.0 };
            self.coalesced_change(|v| {
                let moved = v.items[index].moved(Point::new(d.x * step, d.y * step));
                v.replace_item(moved);
            });
            return;
        }
        let b = self.bounds();
        if mods.is_empty() {
            let mut r = self.selection.offset(d.x, d.y);
            r.x = r.x.clamp(0.0, (b.width - r.width).max(0.0));
            r.y = r.y.clamp(0.0, (b.height - r.height).max(0.0));
            self.selection = r;
        } else if mods == Mods::CTRL || mods == Mods::SHIFT {
            // Ctrl pushes the edge in the arrow's direction outward; Shift pulls that same edge inward.
            let outward = if mods == Mods::CTRL { 1.0 } else { -1.0 };
            let s = self.selection;
            let (mut min_x, mut max_x, mut min_y, mut max_y) = (s.min_x(), s.max_x(), s.min_y(), s.max_y());
            if d.x < 0.0 {
                min_x -= outward;
            }
            if d.x > 0.0 {
                max_x += outward;
            }
            if d.y < 0.0 {
                min_y -= outward;
            }
            if d.y > 0.0 {
                max_y += outward;
            }
            let r = Rect::new(min_x, min_y, max_x - min_x, max_y - min_y).intersection(&b);
            if r.is_null() || r.width < 4.0 || r.height < 4.0 {
                return;
            }
            self.selection = r;
        } else {
            return;
        }
        self.layout_chrome();
    }

    fn restore_last_selection(&mut self) {
        match last_selection(&self.monitor).map(|r| r.intersection(&self.bounds())) {
            Some(r) if !r.is_null() && r.width >= 4.0 && r.height >= 4.0 => {
                self.selection = r;
                self.commit_selection();
            }
            _ => self.show_toast("还没有上一次的选区", Some(Duration::from_millis(2500))),
        }
    }

    fn pixel_at(&self, p: Point) -> ((i32, i32), (u8, u8, u8)) {
        let s = self.scale();
        let pix = &self.base.pixels;
        let x = ((p.x * s) as i32).clamp(0, pix.width() as i32 - 1);
        let y = ((p.y * s) as i32).clamp(0, pix.height() as i32 - 1);
        let c = pix.pixel(x as u32, y as u32).map(|c| c.demultiply()).map_or((0, 0, 0), |c| (c.red(), c.green(), c.blue()));
        ((x, y), c)
    }

    fn copy_pixel_color(&mut self) {
        let (_, rgb) = self.pixel_at(self.mouse);
        let value = chrome::color_string(rgb, settings::get().hex_color);
        self.effects.push(Effect::CopyText(value.clone()));
        self.show_toast(&format!("已复制颜色 {value}"), Some(Duration::from_millis(1500)));
    }

    // MARK: Hover card

    fn card_view(&self, card: &Card) -> HoverCard {
        HoverCard {
            title: card.action.title().to_string(),
            shortcut: card.action.shortcut(),
            editable: card.action.key_id().is_some(),
            note: card.note.clone().or_else(|| card.action.note().map(String::from)),
            recording: card.recording,
        }
    }

    fn show_card(&mut self, action: ToolbarAction) {
        self.card = Some(Card { action, recording: false, note: None });
        self.hide_card_at = None;
        self.place_card();
        self.changed();
    }

    /// Above the button for a row (below without room); for a column, on whichever side has room.
    fn place_card(&mut self) {
        let Some(card) = &self.card else { return };
        let Some(center) = self.toolbar_button_center(card.action) else { return };
        let size = chrome::hover_card_layout(&self.card_view(card)).size;
        let bounds = self.bounds();
        let frame = Rect::from_origin_size(self.toolbar_origin, chrome::toolbar_size(self.toolbar_vertical));
        let b = Rect::new(center.x - 16.0, center.y - 16.0, 32.0, 32.0);
        let mut o = if self.toolbar_vertical {
            let mut o = Point::new(frame.max_x() + 8.0, b.mid_y() - size.height / 2.0);
            if o.x + size.width > bounds.max_x() - 4.0 {
                o.x = frame.min_x() - 8.0 - size.width;
            }
            o
        } else {
            let mut o = Point::new(b.mid_x() - size.width / 2.0, frame.min_y() - 8.0 - size.height);
            if o.y < 4.0 {
                o.y = frame.max_y() + 8.0;
            }
            o
        };
        o.x = o.x.clamp(4.0, (bounds.max_x() - size.width - 4.0).max(4.0));
        o.y = o.y.clamp(4.0, (bounds.max_y() - size.height - 4.0).max(4.0));
        self.card_origin = o;
    }

    fn stop_recording(&mut self) {
        if let Some(c) = &mut self.card {
            if c.recording {
                c.recording = false;
                self.hide_card_at = Some(Instant::now() + CARD_HIDE);
                self.changed();
            }
        }
    }

    fn record_key(&mut self, key: Key, mods: Mods) {
        if key == Key::Escape {
            return self.stop_recording();
        }
        let typed = match key {
            Key::Char(c) if !mods.ctrl && !mods.alt => c.to_string(),
            _ => String::new(),
        };
        if !toolbar::is_allowed(&typed) {
            self.effects.push(Effect::Beep);
            return;
        }
        let Some(card) = &self.card else { return };
        let action = card.action;
        let Some(id) = action.key_id() else { return };
        let other = toolbar::assign(&typed, id);
        let swapped = other.and_then(|o| ToolbarAction::keyed().into_iter().find(|a| a.key_id() == Some(o.as_str())));
        self.card = Some(Card {
            action,
            recording: false,
            note: swapped.map(|s| format!("已和「{}」互换，它现在是 {}", s.title(), s.shortcut())),
        });
        self.place_card();
        self.changed();
    }

    // MARK: Layout

    fn layout_chrome(&mut self) {
        self.changed();
        let bounds = self.bounds();
        let size_rect = if self.has_selection || self.is_selecting() { (self.selection.width > 0.0).then_some(self.selection) } else { self.hover_rect };
        self.top_bar = size_rect.map(|r| {
            let s = chrome::top_bar_size(r.size());
            let mut top = Point::new(r.min_x(), r.min_y() - s.height - 6.0);
            if top.y < 4.0 {
                top.y = r.min_y() + 6.0;
            }
            top.x = top.x.clamp(4.0, (bounds.max_x() - s.width - 4.0).max(4.0));
            top
        });
        if !self.has_selection {
            self.ocr_panel = None;
        }
        let style = self.current_style();
        if !self.toolbar_visible() {
            self.style_frame = None;
            self.card = None;
            self.hovered_button = None;
            self.hover_since = None;
            return;
        }
        let sel = self.selection;
        let top_bar_top = self.top_bar.map_or(sel.min_y(), |t| t.y);
        // Under the selection; without room there, a column beside it (right, then left); failing both,
        // a row above it, then a column inside its right edge, then a row inside its bottom edge.
        let flat = chrome::toolbar_size(false);
        let column = chrome::toolbar_size(true);
        let mut bar = Point::new(sel.max_x() - flat.width, sel.max_y() + 8.0);
        let mut below = true;
        let mut side = None;
        let mut inside = false;
        if bar.y + flat.height > bounds.max_y() - 4.0 {
            let above_top = if top_bar_top < sel.min_y() { top_bar_top } else { sel.min_y() };
            if column.height <= bounds.height - 8.0 {
                if sel.max_x() + 8.0 + column.width <= bounds.max_x() - 4.0 {
                    side = Some(CaretEdge::Right);
                } else if sel.min_x() - 8.0 - column.width >= 4.0 {
                    side = Some(CaretEdge::Left);
                } else if above_top - flat.height - 8.0 < 4.0 && sel.width > column.width * 3.0 {
                    side = Some(CaretEdge::Right);
                    inside = true;
                }
            }
            if let Some(side) = side {
                bar.x = if inside {
                    sel.max_x() - 8.0 - column.width
                } else if side == CaretEdge::Right {
                    sel.max_x() + 8.0
                } else {
                    sel.min_x() - 8.0 - column.width
                };
                // Bottom-aligned with the selection, like the horizontal bar hangs off its bottom-right corner.
                bar.y = (sel.max_y() - column.height).max(4.0).min(bounds.max_y() - column.height - 4.0);
            } else {
                bar.y = above_top - flat.height - 8.0;
                below = false;
                if bar.y < 4.0 {
                    bar.y = sel.max_y() - flat.height - 8.0;
                    below = true;
                }
            }
        }
        self.toolbar_vertical = side.is_some();
        self.toolbar_side = side;
        let size = chrome::toolbar_size(self.toolbar_vertical);
        if side.is_none() {
            bar.x = bar.x.clamp(4.0, (bounds.max_x() - size.width - 4.0).max(4.0));
        }
        if bar != self.toolbar_origin {
            // Any move of the toolbar drops the card.
            self.card = None;
            self.hover_since = None;
        }
        self.toolbar_origin = bar;

        self.style_frame = style.as_ref().map(|state| {
            let icon = self
                .toolbar_button_center(ToolbarAction::Tool(state.tool))
                .map(|c| Point::new(c.x - bar.x, c.y - bar.y))
                .unwrap_or(Point::new(size.width / 2.0, size.height / 2.0));
            if let Some(side) = side {
                // Beside the column, on the selection side, with the caret pointing at the tool.
                let edge = if side == CaretEdge::Right { CaretEdge::Right } else { CaretEdge::Left };
                let s = chrome::style_bar_size(state, Some(edge));
                let x = if side == CaretEdge::Right { bar.x - s.width - 4.0 } else { bar.x + size.width + 4.0 };
                let anchor = bar.y + icon.y;
                let y = (anchor - s.height / 2.0).max(4.0).min(bounds.max_y() - s.height - 4.0);
                let x = x.clamp(4.0, (bounds.max_x() - s.width - 4.0).max(4.0));
                (Rect::new(x, y, s.width, s.height), edge, anchor - y)
            } else {
                // Next to the toolbar with a caret pointing at the tool it configures.
                let mut style_below = below;
                let expected = 42.0;
                if style_below && bar.y + size.height + 4.0 + expected > bounds.max_y() - 4.0 {
                    style_below = false;
                }
                if !style_below && bar.y - 4.0 - expected < 4.0 {
                    style_below = true;
                }
                let edge = if style_below { CaretEdge::Top } else { CaretEdge::Bottom };
                let s = chrome::style_bar_size(state, Some(edge));
                let anchor = bar.x + icon.x;
                let x = (anchor - 26.0).max(4.0).min(bounds.max_x() - s.width - 4.0);
                let y = if style_below { bar.y + size.height + 4.0 } else { bar.y - s.height - 4.0 };
                (Rect::new(x, y, s.width, s.height), edge, anchor - x)
            }
        });

        // The OCR panel goes beside the selection, clear of a toolbar column there.
        if self.ocr_panel.is_some() {
            let panel = chrome::OCR_PANEL;
            let right = if side == Some(CaretEdge::Right) && !inside { bar.x + size.width } else { sel.max_x() };
            let left = if side == Some(CaretEdge::Left) { bar.x } else { sel.min_x() };
            let mut o = Point::new(right + 10.0, sel.min_y());
            if o.x + panel.width > bounds.max_x() - 4.0 {
                o.x = left - panel.width - 10.0;
            }
            if o.x < 4.0 {
                o.x = bounds.max_x() - panel.width - 4.0;
            }
            o.y = o.y.max(4.0).min(bounds.max_y() - panel.height - 4.0);
            let moved = self.ocr_panel.as_ref().is_some_and(|p| p.origin != o);
            if let Some(p) = &mut self.ocr_panel {
                p.origin = o;
            }
            if moved {
                self.emit_ocr_panel();
            }
        }
        if self.card.is_some() {
            self.place_card();
        }
    }

    fn emit_ocr_panel(&mut self) {
        let Some(p) = &self.ocr_panel else { return };
        let rect = chrome::ocr_text_rect().offset(p.origin.x, p.origin.y);
        self.effects.push(Effect::OcrPanel(Some((rect, p.text.clone()))));
    }

    fn toast_origin(&self, size: Size) -> Point {
        let b = self.bounds();
        let anchor = if self.has_selection { self.selection } else { Rect::new(b.mid_x(), 40.0, 0.0, 0.0) };
        Point::new((anchor.mid_x() - size.width / 2.0).max(4.0).min(b.max_x() - size.width - 4.0), (anchor.min_y() + 10.0).max(4.0))
    }

    fn toast_max_width(&self) -> f32 {
        if self.has_selection { self.selection.width.max(220.0) } else { 400.0 }
    }

    // MARK: Actions

    fn handle_action(&mut self, action: ToolbarAction) {
        self.commit_text();
        match action {
            ToolbarAction::Tool(t) => self.set_tool(if self.tool == Some(t) { None } else { Some(t) }),
            ToolbarAction::Undo => self.undo(),
            ToolbarAction::Ocr => self.run_ocr(),
            ToolbarAction::Translate => self.run_translation(),
            ToolbarAction::Pin => self.finish(Output::Pin),
            ToolbarAction::Cancel => self.effects.push(Effect::Close),
            ToolbarAction::Save => self.finish(if self.shift_down { Output::SaveAs } else { Output::Save }),
            ToolbarAction::Done => self.finish(Output::Copy),
        }
    }

    /// The selection's pixels.
    fn crop_selection(&self) -> Option<RgbaImage> {
        let s = self.scale();
        let r = self.selection.scaled(s).integral();
        let pix = &self.base.pixels;
        let data: Vec<u8> = pix.data().to_vec();
        let full = RgbaImage { width: pix.width(), height: pix.height(), data };
        full.crop(r.x as i64, r.y as i64, r.width as i64, r.height as i64)
    }

    fn start_recognition(&mut self, purpose: Purpose) -> bool {
        if let Some((rect, _)) = &self.recognition {
            if *rect == self.selection {
                return false;
            }
        }
        if !self.models_ready {
            self.show_toast("文字识别组件还没有下载", Some(Duration::from_millis(3000)));
            self.effects.push(Effect::ModelsMissing);
            return true;
        }
        let Some(crop) = self.crop_selection() else { return true };
        self.busy = Some(purpose);
        self.pending_rect = self.selection;
        self.show_toast("正在识别文字…", None);
        self.effects.push(Effect::Recognize(crop));
        true
    }

    /// The models finished downloading while the overlay is open.
    pub fn set_models_ready(&mut self, ready: bool) {
        self.models_ready = ready;
    }

    /// Recognizes the selection's text and shows it in an editable panel beside the selection.
    fn run_ocr(&mut self) {
        if self.busy.is_some() {
            return;
        }
        if !self.start_recognition(Purpose::Ocr) {
            self.show_ocr_result();
        }
    }

    fn show_ocr_result(&mut self) {
        let Some((_, lines)) = &self.recognition else { return };
        let text = textblocks::plain_text(lines);
        if text.is_empty() {
            self.show_toast("没有识别到文字", Some(Duration::from_millis(2500)));
            return;
        }
        let line_count = lines.len();
        self.ocr_panel = Some(OcrPanel { text, line_count, copied_until: None, origin: Point::new(-10_000.0, 0.0) });
        self.ocr_boxes_visible = true;
        self.hide_toast();
        self.layout_chrome();
    }

    fn close_ocr_panel(&mut self) {
        if self.ocr_panel.take().is_some() {
            self.effects.push(Effect::OcrPanel(None));
        }
        self.ocr_boxes_visible = false;
        self.changed();
    }

    /// The user edited the OCR panel's text.
    pub fn ocr_text_changed(&mut self, text: &str) {
        if let Some(p) = &mut self.ocr_panel {
            p.text = text.to_string();
        }
    }

    /// Lines in the recognized image's pixels, or an error message.
    pub fn recognition_finished(&mut self, result: Result<Vec<(String, Rect)>, String>) {
        let Some(purpose) = self.busy.take() else { return };
        if self.pending_rect != self.selection {
            self.show_toast("选区已改变，请重新识别", Some(Duration::from_millis(2500)));
            return;
        }
        let lines = match result {
            Ok(lines) => lines,
            Err(e) => {
                self.translation = Translation::None;
                self.show_toast(&format!("文字识别失败：{e}"), Some(Duration::from_millis(4000)));
                return;
            }
        };
        let s = self.scale();
        let origin = self.selection.origin();
        let lines: Vec<OcrLine> = lines
            .into_iter()
            .map(|(text, r)| OcrLine { text, rect: Rect::new(origin.x + r.x / s, origin.y + r.y / s, r.width / s, r.height / s) })
            .collect();
        self.recognition = Some((self.selection, lines));
        match purpose {
            Purpose::Ocr => self.show_ocr_result(),
            Purpose::Translate => self.continue_translation(),
        }
    }

    fn run_translation(&mut self) {
        match &self.translation {
            Translation::Loading => return,
            Translation::Shown(blocks, rect) if *rect == self.selection => {
                self.translation = Translation::Hidden(blocks.clone(), *rect);
                self.show_toast("显示原文", Some(Duration::from_millis(1000)));
                return self.layout_chrome();
            }
            Translation::Hidden(blocks, rect) if *rect == self.selection => {
                self.translation = Translation::Shown(blocks.clone(), *rect);
                self.show_toast("显示译文", Some(Duration::from_millis(1000)));
                return self.layout_chrome();
            }
            _ => {}
        }
        if !self.api_key_present {
            self.show_toast("还没有填写 API Key。请按 Esc 退出截图，在托盘图标 → 设置 → 翻译 中填写（不翻译可以不填）。", Some(Duration::from_millis(5000)));
            return;
        }
        if self.busy.is_some() {
            return;
        }
        self.translation = Translation::Loading;
        if !self.start_recognition(Purpose::Translate) {
            self.continue_translation();
        } else if self.busy.is_none() {
            // Recognition couldn't start (models missing).
            self.translation = Translation::None;
        }
    }

    fn continue_translation(&mut self) {
        let Some((_, lines)) = &self.recognition else { return };
        let blocks: Vec<TextBlock> = textblocks::group(lines).into_iter().filter(|b| textblocks::should_translate(&b.text())).collect();
        if blocks.is_empty() {
            self.translation = Translation::None;
            self.show_toast("没有找到需要翻译的外文", Some(Duration::from_millis(2500)));
            return;
        }
        self.translation = Translation::Loading;
        self.shimmer_start = Instant::now();
        self.show_toast(&format!("正在翻译 {} 段…", blocks.len()), None);
        let items = blocks.iter().map(|b| Item { id: b.id, text: b.text() }).collect();
        self.pending_blocks = blocks;
        self.pending_rect = self.selection;
        self.busy = Some(Purpose::Translate);
        self.effects.push(Effect::Translate(items));
    }

    pub fn translation_finished(&mut self, result: Result<HashMap<usize, String>, TranslationError>) {
        if self.busy != Some(Purpose::Translate) {
            return;
        }
        self.busy = None;
        let blocks = std::mem::take(&mut self.pending_blocks);
        match result {
            Err(e) => {
                self.translation = Translation::None;
                let message = match e {
                    TranslationError::MissingApiKey => "还没有填写 API Key。请按 Esc 退出截图，在托盘图标 → 设置 → 翻译 中填写。".to_string(),
                    e => format!("翻译失败：{e}\n再按一次翻译按钮可以重试。"),
                };
                self.show_toast(&message, Some(Duration::from_millis(6000)));
            }
            Ok(translations) => {
                let rect = self.pending_rect;
                let (Some(crop), true) = (self.crop_selection(), rect == self.selection) else {
                    self.translation = Translation::None;
                    self.show_toast("选区已改变，请重新翻译", Some(Duration::from_millis(2500)));
                    return;
                };
                let laid_out = crate::render::translation::layout(&blocks, &translations, &crop, &rect);
                let missing = blocks.len() - laid_out.len();
                let count = laid_out.len();
                self.translation = Translation::Shown(laid_out, rect);
                if missing > 0 {
                    self.show_toast(&format!("已翻译 {count} 段，{missing} 段没有返回译文"), Some(Duration::from_millis(3000)));
                } else {
                    self.show_toast(&format!("已翻译 {count} 段 · Y 切换原文"), Some(Duration::from_millis(2000)));
                }
            }
        }
        self.layout_chrome();
    }

    // MARK: Output

    /// The image copy / save would produce right now: the selection with annotations and visible translation.
    pub fn export_image(&self) -> Option<RgbaImage> {
        let s = self.scale();
        let sel = self.selection;
        let w = (sel.width * s).round() as u32;
        let h = (sel.height * s).round() as u32;
        let mut pix = Pixmap::new(w.max(1), h.max(1))?;
        {
            let ts = Transform::from_scale(s, s).pre_translate(-sel.x, -sel.y);
            let mut c = Canvas::new(pix.as_mut(), ts);
            ContentRenderer::new(&self.base).draw(&mut c, &self.items, self.visible_translation());
        }
        let data = pix.take_demultiplied();
        Some(RgbaImage { width: w.max(1), height: h.max(1), data })
    }

    fn finish(&mut self, output: Output) {
        self.commit_text();
        self.end_change();
        if !self.has_selection || self.selection.width < 1.0 || self.selection.height < 1.0 {
            return;
        }
        set_last_selection(&self.monitor, self.selection);
        let Some(image) = self.export_image() else {
            self.show_toast("导出图片失败", Some(Duration::from_millis(2500)));
            return;
        };
        let format = settings::get().image_format;
        self.effects.push(match output {
            Output::Copy => Effect::Copy(image),
            Output::Save => Effect::Save(image, format),
            Output::SaveAs => Effect::SaveAs(image, format),
            Output::Pin => Effect::Pin(image, self.selection),
        });
    }

    // MARK: Rendering

    /// Rects of everything drawn over the dimmed screen, for repainting only what changed.
    fn frame_rects(&self) -> Rect {
        let mut r = self.focus_rect().map_or(Rect::NULL, |f| f.inset(-14.0, -14.0));
        if let Some(t) = self.top_bar {
            let s = chrome::top_bar_size(self.focus_rect().map_or(Size::default(), |f| f.size()));
            r = r.union(&Rect::from_origin_size(t, s).inset(-4.0, -4.0));
        }
        if let Some(f) = self.toolbar_frame() {
            r = r.union(&f.inset(-16.0, -16.0));
        }
        if let Some((f, ..)) = self.style_frame {
            r = r.union(&f.inset(-16.0, -16.0));
        }
        if let Some(card) = &self.card {
            let s = chrome::hover_card_layout(&self.card_view(card)).size;
            r = r.union(&Rect::from_origin_size(self.card_origin, s).inset(-16.0, -16.0));
        }
        if let Some(t) = &self.toast {
            let (s, _) = chrome::toast_layout(&t.text, self.toast_max_width());
            r = r.union(&Rect::from_origin_size(self.toast_origin(s), s).inset(-2.0, -2.0));
        }
        if let Some((p, _)) = &self.magnifier {
            let o = chrome::magnifier_origin(*p, &self.bounds());
            r = r.union(&Rect::from_origin_size(o, chrome::magnifier_size()).inset(-16.0, -16.0));
        }
        if let Some(p) = &self.ocr_panel {
            r = r.union(&Rect::from_origin_size(p.origin, chrome::OCR_PANEL).inset(-16.0, -16.0));
        }
        // Annotations can stick out of the selection while being dragged (they are clipped, but handles aren't).
        for item in &self.items {
            r = r.union(&item.bounds(&self.measure()).inset(-12.0, -12.0));
        }
        if let Some(f) = self.editor_frame() {
            r = r.union(&f.inset(-8.0, -8.0));
        }
        r
    }

    /// The pixel rect to repaint since the last call, or None when nothing changed.
    pub fn take_dirty(&mut self) -> Option<(i32, i32, u32, u32)> {
        if !self.dirty {
            return None;
        }
        self.dirty = false;
        let now = self.frame_rects();
        let union = now.union(&self.last_frame);
        self.last_frame = now;
        if union.is_null() {
            return None;
        }
        let s = self.scale();
        let r = union.scaled(s).integral().intersection(&Rect::new(0.0, 0.0, self.base.pixels.width() as f32, self.base.pixels.height() as f32));
        if r.is_empty() {
            return None;
        }
        Some((r.x as i32, r.y as i32, r.width as u32, r.height as u32))
    }

    /// Renders the pixel rect `x, y, w, h` of the overlay (premultiplied RGBA).
    pub fn render_region(&self, x: i32, y: i32, w: u32, h: u32) -> Option<Pixmap> {
        let mut pix = Pixmap::new(w.max(1), h.max(1))?;
        self.render_into(&mut pix, x, y);
        Some(pix)
    }

    pub fn render_full(&self) -> Option<Pixmap> {
        self.render_region(0, 0, self.base.pixels.width(), self.base.pixels.height())
    }

    fn render_into(&self, pix: &mut Pixmap, ox: i32, oy: i32) {
        let s = self.scale();
        // Frozen screen: copy the pixels straight across (no resampling).
        copy_region(&self.base.pixels, pix, ox, oy);
        let ts = Transform::from_translate(-ox as f32, -oy as f32).pre_scale(s, s);
        let mut c = Canvas::new(pix.as_mut(), ts);
        let focus = self.focus_rect();

        // Dim everything but the focus rect.
        let bounds = self.bounds();
        let mut pb = tiny_skia::PathBuilder::new();
        if let Some(r) = crate::render::canvas::skia_rect(&bounds.inset(-2.0, -2.0)) {
            pb.push_rect(r);
        }
        if let Some(r) = focus.and_then(|f| crate::render::canvas::skia_rect(&f)) {
            pb.push_rect(r);
        }
        if let Some(p) = pb.finish() {
            c.fill_path_even_odd(&p, Color::black(0.4));
        }

        if let Some(focus) = focus {
            if self.has_selection {
                c.save();
                c.clip_rect(&focus);
                let editing = self.text_edit.as_ref().and_then(|e| e.editing_id);
                ContentRenderer::new(&self.base).draw_overlays(&mut c, &self.items, self.draft.as_ref(), editing, self.visible_translation());
                if self.ocr_boxes_visible {
                    if let Some((_, lines)) = &self.recognition {
                        for line in lines {
                            let b = line.rect.inset(-2.0, -1.0);
                            c.fill_rounded(&b, 3.0, SELECTION_BLUE.with_alpha(0.14));
                            c.stroke_rounded(&b, 3.0, SELECTION_BLUE.with_alpha(0.55), 1.0);
                        }
                    }
                }
                if matches!(self.translation, Translation::Loading) {
                    let t = self.shimmer_start.elapsed().as_secs_f32();
                    for (i, block) in self.pending_blocks.iter().enumerate() {
                        let wave = 0.5 + 0.5 * (t * 5.0 - i as f32 * 0.6).sin();
                        c.fill_rounded(&block.rect().inset(-2.0, -1.5), 3.0, SELECTION_BLUE.with_alpha(0.10 + 0.16 * wave));
                    }
                }
                self.draw_text_editor(&mut c);
                c.restore();
                self.draw_item_decorations(&mut c);
            } else if self.draft.is_some() {
                // Nothing to draw: annotations need a selection.
            }
            c.stroke_rect(&focus.inset(-0.75, -0.75), SELECTION_BLUE, if self.has_selection || self.is_selecting() { 1.5 } else { 2.5 });
            if self.has_selection {
                for h in ResizeHandle::ALL {
                    let p = h.point(&focus);
                    let sq = Rect::new(p.x - 3.5, p.y - 3.5, 7.0, 7.0);
                    c.fill_rect(&sq, SELECTION_BLUE);
                    c.stroke_rect(&sq.inset(0.5, 0.5), Color::white(1.0), 1.0);
                }
            }
        }

        if let (Some(t), Some(f)) = (self.top_bar, focus) {
            chrome::draw_top_bar(&mut c, t, f.size());
        }
        if self.toolbar_visible() {
            chrome::draw_toolbar(
                &mut c,
                &ToolbarLook {
                    origin: self.toolbar_origin,
                    vertical: self.toolbar_vertical,
                    active_tool: self.tool,
                    translate_active: matches!(self.translation, Translation::Shown(..) | Translation::Loading),
                    can_undo: self.can_undo(),
                    hovered: self.hovered_button,
                    pressed: match self.drag {
                        Drag::Toolbar(a) => Some(a),
                        _ => None,
                    },
                },
                &self.theme,
            );
        }
        if let (Some((frame, edge, offset)), Some(state)) = (self.style_frame, self.current_style()) {
            chrome::draw_style_bar(&mut c, &frame, (edge, offset), &state, self.hovered_style, &self.theme);
        }
        if let Some(panel) = &self.ocr_panel {
            chrome::draw_ocr_panel(&mut c, panel.origin, panel.line_count, Some(&panel.text), panel.copied_until.is_some(), self.ocr_hover, &self.theme);
        }
        if let Some((p, size_text)) = &self.magnifier {
            let (pixel, rgb) = self.pixel_at(*p);
            let o = chrome::magnifier_origin(*p, &bounds);
            let top = size_text.clone().unwrap_or_else(|| format!("{}, {}", p.x.round() as i32, p.y.round() as i32));
            chrome::draw_magnifier(&mut c, o, &self.base.pixels, pixel, rgb, &top, settings::get().hex_color);
        }
        if let Some(card) = &self.card {
            chrome::draw_hover_card(&mut c, self.card_origin, &self.card_view(card));
        }
        if let Some(t) = &self.toast {
            let (size, _) = chrome::toast_layout(&t.text, self.toast_max_width());
            chrome::draw_toast(&mut c, self.toast_origin(size), &t.text, self.toast_max_width(), 1.0);
        }
    }

    fn draw_text_editor(&self, c: &mut Canvas) {
        let Some(e) = &self.text_edit else { return };
        let (shown, comp_start, comp_end) = Self::display_text(e);
        let l = text::layout(&shown, e.size, annotation_weight(), Some(e.wrap));
        // Selection highlight, then the text as it will look, then the IME's composition underline and the caret.
        let (a, b) = (e.start.min(e.end), e.start.max(e.end));
        if e.composition.is_empty() {
            for (x, y, w, h) in l.selection_rects(a, b) {
                c.fill_rect(&Rect::new(e.origin.x + x, e.origin.y + y, w, h), SELECTION_BLUE.with_alpha(0.3));
            }
        }
        let item = AnnotationItem {
            id: 0,
            shape: Shape::Text { text: shown.clone(), origin: e.origin, width: e.wrap },
            color: e.color,
            size: e.size,
            effect: MosaicEffect::Pixelate,
            style: e.style,
        };
        for (x, y, w, h) in l.selection_rects(comp_start, comp_end) {
            c.fill_rect(&Rect::new(e.origin.x + x, e.origin.y + y + h - 2.0, w, 1.5), e.color);
        }
        if !shown.is_empty() {
            if e.style.text == TextDecoration::Background || e.style.text == TextDecoration::Outline {
                ContentRenderer::new(&self.base).draw_item(c, &item, 0);
            } else {
                c.text_layout(&l, e.origin, e.color);
            }
        }
        if let Some(caret) = self.caret_rect() {
            c.fill_rect(&caret, e.color);
        }
    }

    fn draw_item_decorations(&self, c: &mut Canvas) {
        if let Some(frame) = self.editor_frame() {
            dashed_rect(c, &frame.inset(-5.0, -3.0), 1.0);
        }
        let editing = self.text_edit.as_ref().and_then(|e| e.editing_id);
        if let Some(h) = self.item(self.hovered_id) {
            if Some(h.id) != self.selected_id && Some(h.id) != editing {
                dashed_rect(c, &h.bounds(&self.measure()).inset(-3.0, -3.0), 0.5);
            }
        }
        let Some(item) = self.item(self.selected_id) else { return };
        if Some(item.id) == editing {
            return;
        }
        let handles = item.handles();
        if handles.is_empty() || item.tool() == Tool::Mosaic {
            dashed_rect(c, &item.bounds(&self.measure()).inset(-3.0, -3.0), 1.0);
        }
        for (_, p) in handles {
            let dot = Rect::new(p.x - 4.5, p.y - 4.5, 9.0, 9.0);
            c.fill_oval(&dot, Color::white(1.0));
            c.stroke_oval(&dot, SELECTION_BLUE, 1.5);
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Output {
    Copy,
    Save,
    SaveAs,
    Pin,
}

fn dashed_rect(c: &mut Canvas, r: &Rect, alpha: f32) {
    c.stroke_rect(r, Color::white(0.8 * alpha), 1.0);
    c.dashed_rect(r, SELECTION_BLUE.with_alpha(alpha), 1.0, 4.0, 3.0);
}

/// Copies the pixels of `src` at offset (`ox`, `oy`) into `dst` (same pixel format).
fn copy_region(src: &Pixmap, dst: &mut Pixmap, ox: i32, oy: i32) {
    let (sw, sh) = (src.width() as i32, src.height() as i32);
    let (dw, dh) = (dst.width() as i32, dst.height() as i32);
    let x0 = ox.max(0);
    let x1 = (ox + dw).min(sw);
    if x1 <= x0 {
        return;
    }
    let s = src.data();
    let d = dst.data_mut();
    for dy in 0..dh {
        let sy = oy + dy;
        if sy < 0 || sy >= sh {
            continue;
        }
        let s_start = ((sy * sw + x0) * 4) as usize;
        let d_start = ((dy * dw + (x0 - ox)) * 4) as usize;
        let len = ((x1 - x0) * 4) as usize;
        d[d_start..d_start + len].copy_from_slice(&s[s_start..s_start + len]);
    }
}

#[cfg(test)]
#[path = "capture_tests.rs"]
mod tests;
