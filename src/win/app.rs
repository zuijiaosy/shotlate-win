//! The tray app: a hidden window that owns the tray icon and global hotkeys, runs tasks for worker threads,
//! manages the OCR engine and the model download, and pumps messages.

use std::cell::RefCell;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS, HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoTaskMemFree};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::SystemInformation::GetLocalTime;
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::Controls::{ICC_HOTKEY_CLASS, ICC_LINK_CLASS, ICC_STANDARD_CLASSES, INITCOMMONCONTROLSEX, InitCommonControlsEx};
use windows::Win32::UI::Input::KeyboardAndMouse::{HOT_KEY_MODIFIERS, MOD_NOREPEAT, RegisterHotKey, UnregisterHotKey};
use windows::Win32::UI::Shell::{
    FOLDERID_Downloads, KNOWN_FOLDER_FLAG, NIF_ICON, NIF_MESSAGE, NIF_SHOWTIP, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY, NIM_SETVERSION, NOTIFYICON_VERSION_4,
    NOTIFYICONDATAW, SHGetKnownFolderPath, Shell_NotifyIconW,
};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{PCWSTR, w};

use super::util::{self, wide};
use super::{dialogs, hud, overlay, pin, secret, settings_window, updater};
use crate::app_paths;
use crate::kit::settings::{self, Shortcut};
use crate::kit::translator::{TranslationCache, TranslationConfig};
use crate::kit::image::RgbaImage;
use crate::ocr::{self, OcrEngine, RecognizedLine};

const CLASS: PCWSTR = w!("ShotlateApp");
const WM_APP_TRAY: u32 = WM_APP + 2;
/// A second launch asks the running copy to open its settings.
const WM_APP_SHOW_SETTINGS: u32 = WM_APP + 3;
const HOTKEY_CAPTURE: i32 = 1;
const HOTKEY_TOGGLE_PINS: i32 = 2;

const MENU_CAPTURE: usize = 1;
const MENU_TOGGLE_PINS: usize = 2;
const MENU_CLOSE_PINS: usize = 3;
const MENU_SETTINGS: usize = 4;
const MENU_MODELS: usize = 5;
const MENU_UPDATES: usize = 6;
const MENU_QUIT: usize = 7;

struct State {
    hwnd: HWND,
    taskbar_created: u32,
    capture_ok: bool,
    toggle_ok: bool,
}

thread_local! {
    static STATE: RefCell<State> = const { RefCell::new(State { hwnd: HWND(std::ptr::null_mut()), taskbar_created: 0, capture_ok: true, toggle_ok: true }) };
}

// MARK: OCR engine and models

static ENGINE: Mutex<Option<Arc<OcrEngine>>> = Mutex::new(None);
static DOWNLOADING: AtomicBool = AtomicBool::new(false);
static CANCEL: AtomicBool = AtomicBool::new(false);
/// Download progress (done, total) for the settings window.
pub static PROGRESS: Mutex<Option<(u64, u64)>> = Mutex::new(None);
static CACHE: OnceLock<TranslationCache> = OnceLock::new();

pub fn models_ready() -> bool {
    ocr::models::is_ready(&app_paths::models_dir())
}

pub fn is_downloading() -> bool {
    DOWNLOADING.load(Ordering::Relaxed)
}

/// How long the engine stays loaded after its last use. Its optimized plans take 100–200 MB while
/// loading and building them again costs well under a second next to the recognition itself, so it
/// is not kept around (nor warmed up at startup). See AGENTS.md.
const ENGINE_IDLE: Duration = Duration::from_secs(60);
/// Bumped when a recognition starts and ends; a pending release only goes ahead if it hasn't moved.
static ENGINE_USES: AtomicU64 = AtomicU64::new(0);

/// The engine, loading it on first use.
fn ocr_engine() -> Result<Arc<OcrEngine>, String> {
    let mut guard = ENGINE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(e) = guard.as_ref() {
        return Ok(e.clone());
    }
    let engine = Arc::new(OcrEngine::load(&app_paths::models_dir()).map_err(|e| e.to_string())?);
    *guard = Some(engine.clone());
    Ok(engine)
}

/// Recognizes `image`, loading the models if needed (call from a worker thread). The engine is
/// released once it has been idle for `ENGINE_IDLE`.
pub fn recognize(image: &RgbaImage) -> Result<Vec<RecognizedLine>, String> {
    // Schedules the release on drop, so a panicking recognition doesn't pin the engine in memory.
    struct ReleaseLater;
    impl Drop for ReleaseLater {
        fn drop(&mut self) {
            let mark = ENGINE_USES.fetch_add(1, Ordering::SeqCst) + 1;
            std::thread::spawn(move || {
                std::thread::sleep(ENGINE_IDLE);
                let mut guard = ENGINE.lock().unwrap_or_else(|e| e.into_inner());
                if ENGINE_USES.load(Ordering::SeqCst) == mark && guard.take().is_some() {
                    util::trace(|| "OCR engine released after idle".into());
                }
            });
        }
    }
    ENGINE_USES.fetch_add(1, Ordering::SeqCst);
    let _release = ReleaseLater;
    ocr_engine().and_then(|engine| engine.recognize(image).map_err(|e| e.to_string()))
}

/// Asks whether to download the recognition models (about 23 MB) and starts the download.
pub fn offer_model_download(owner: Option<HWND>) {
    if is_downloading() {
        hud::show("正在下载文字识别组件…");
        return;
    }
    let mb = ocr::models::TOTAL_BYTES as f64 / 1_000_000.0;
    let text = format!("识别文字和翻译需要先下载文字识别组件（约 {mb:.0} MB），只需下载一次。\n\n现在下载吗？不下载也能正常截图、标注、复制和保存。");
    if dialogs::ask(owner, "下载文字识别组件", &text) {
        start_model_download();
    }
}

pub fn start_model_download() {
    if DOWNLOADING.swap(true, Ordering::SeqCst) {
        return;
    }
    CANCEL.store(false, Ordering::SeqCst);
    *PROGRESS.lock().unwrap_or_else(|e| e.into_inner()) = Some((0, ocr::models::TOTAL_BYTES));
    settings_window::refresh_models();
    std::thread::spawn(|| {
        let dir = app_paths::models_dir();
        let mut last = Instant::now();
        let mut progress = |done: u64, total: u64| {
            *PROGRESS.lock().unwrap_or_else(|e| e.into_inner()) = Some((done, total));
            if last.elapsed() > Duration::from_millis(250) {
                last = Instant::now();
                util::run_on_ui(settings_window::refresh_models);
            }
        };
        let result = ocr::models::download(&dir, &mut progress, &CANCEL);
        DOWNLOADING.store(false, Ordering::SeqCst);
        *PROGRESS.lock().unwrap_or_else(|e| e.into_inner()) = None;
        util::run_on_ui(move || {
            settings_window::refresh_models();
            match result {
                Ok(()) => {
                    hud::show("文字识别组件已下载");
                    overlay::models_became_ready();
                }
                Err(e) => dialogs::warn(None, "下载失败", &format!("{e}\n\n可以稍后在托盘图标 → 设置 → 通用里重新下载。")),
            }
        });
    });
}

pub fn cancel_model_download() {
    CANCEL.store(true, Ordering::SeqCst);
}

// MARK: Translation, saving

pub fn translation_config() -> TranslationConfig {
    let s = settings::get();
    TranslationConfig { base_url: s.base_url, model: s.model, api_key: secret::api_key(), target_language: s.target_language, timeout: Duration::from_secs(20) }
}

pub fn translation_cache() -> &'static TranslationCache {
    CACHE.get_or_init(TranslationCache::default)
}

pub fn downloads_dir() -> PathBuf {
    unsafe {
        if let Ok(p) = SHGetKnownFolderPath(&FOLDERID_Downloads, KNOWN_FOLDER_FLAG(0), None) {
            let s = p.to_string().unwrap_or_default();
            CoTaskMemFree(Some(p.0 as *const _));
            if !s.is_empty() {
                return PathBuf::from(s);
            }
        }
    }
    std::env::var_os("USERPROFILE").map(|h| PathBuf::from(h).join("Downloads")).unwrap_or_else(std::env::temp_dir)
}

pub fn save_directory() -> PathBuf {
    settings::get().save_directory.map(PathBuf::from).unwrap_or_else(downloads_dir)
}

pub fn local_time() -> (u16, u16, u16, u16, u16, u16) {
    let t = unsafe { GetLocalTime() };
    (t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond)
}

// MARK: Hotkeys

fn register_one(hwnd: HWND, id: i32, shortcut: Option<&Shortcut>) -> bool {
    unsafe {
        let _ = UnregisterHotKey(Some(hwnd), id);
    }
    let Some(s) = shortcut else { return true };
    unsafe { RegisterHotKey(Some(hwnd), id, HOT_KEY_MODIFIERS(s.modifiers) | MOD_NOREPEAT, s.vk).is_ok() }
}

/// Registers the global shortcuts from the settings; remembers which ones another app already took.
pub fn register_hotkeys() {
    let hwnd = util::app_window();
    let s = settings::get();
    let capture_ok = register_one(hwnd, HOTKEY_CAPTURE, Some(&s.capture_shortcut));
    let toggle_ok = register_one(hwnd, HOTKEY_TOGGLE_PINS, s.toggle_pins_shortcut.as_ref());
    STATE.with(|st| {
        if let Ok(mut st) = st.try_borrow_mut() {
            st.capture_ok = capture_ok;
            st.toggle_ok = toggle_ok;
        }
    });
}

/// While a shortcut is being typed into the settings, the old one must not fire.
pub fn pause_hotkeys() {
    let hwnd = util::app_window();
    unsafe {
        let _ = UnregisterHotKey(Some(hwnd), HOTKEY_CAPTURE);
        let _ = UnregisterHotKey(Some(hwnd), HOTKEY_TOGGLE_PINS);
    }
}

pub fn hotkey_status() -> (bool, bool) {
    STATE.with(|st| st.try_borrow().map(|s| (s.capture_ok, s.toggle_ok)).unwrap_or((true, true)))
}

// MARK: Tray

fn tray_data(hwnd: HWND) -> NOTIFYICONDATAW {
    let mut data = NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: 1,
        // NOTIFYICON_VERSION_4 hides the standard tooltip unless NIF_SHOWTIP is set.
        uFlags: NIF_ICON | NIF_MESSAGE | NIF_TIP | NIF_SHOWTIP,
        uCallbackMessage: WM_APP_TRAY,
        hIcon: app_icon(true),
        ..Default::default()
    };
    let tip = wide(&format!("Shotlate · 截图 {}", settings::get().capture_shortcut.display()));
    let n = tip.len().min(data.szTip.len() - 1);
    data.szTip[..n].copy_from_slice(&tip[..n]);
    data
}

fn app_icon(small: bool) -> HICON {
    unsafe {
        let instance = GetModuleHandleW(None).unwrap_or_default();
        let size = if small { GetSystemMetrics(SM_CXSMICON) } else { GetSystemMetrics(SM_CXICON) };
        // Shared: the tray data is rebuilt on every tooltip change, and a shared icon needs no DestroyIcon.
        LoadImageW(Some(instance.into()), PCWSTR(1 as *const u16), IMAGE_ICON, size, size, LR_DEFAULTCOLOR | LR_SHARED)
            .map(|h| HICON(h.0))
            .or_else(|_| LoadIconW(None, IDI_APPLICATION))
            .unwrap_or_default()
    }
}

fn add_tray(hwnd: HWND) {
    let mut data = tray_data(hwnd);
    unsafe {
        let _ = Shell_NotifyIconW(NIM_ADD, &data);
        data.Anonymous.uVersion = NOTIFYICON_VERSION_4;
        let _ = Shell_NotifyIconW(NIM_SETVERSION, &data);
    }
}

pub fn refresh_tray_tip() {
    let data = tray_data(util::app_window());
    unsafe {
        let _ = Shell_NotifyIconW(NIM_MODIFY, &data);
    }
}

fn show_tray_menu(hwnd: HWND) {
    let s = settings::get();
    let (capture_ok, toggle_ok) = hotkey_status();
    let label = |title: &str, shortcut: Option<&Shortcut>, ok: bool| match shortcut {
        Some(sc) if ok => format!("{title}\t{}", sc.display()),
        Some(sc) => format!("{title}（快捷键 {} 已被占用）", sc.display()),
        None => title.to_string(),
    };
    unsafe {
        let Ok(menu) = CreatePopupMenu() else { return };
        let add = |id: usize, text: &str, enabled: bool| {
            let t = wide(text);
            let flags = MF_STRING | if enabled { MF_ENABLED } else { MF_GRAYED };
            let _ = AppendMenuW(menu, flags, id, PCWSTR(t.as_ptr()));
        };
        let sep = || {
            let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
        };
        add(MENU_CAPTURE, &label("截图", Some(&s.capture_shortcut), capture_ok), true);
        sep();
        let has_pins = pin::has_pins();
        let toggle_title = if pin::is_hiding_all() { "显示全部贴图" } else { "隐藏全部贴图" };
        add(MENU_TOGGLE_PINS, &label(toggle_title, s.toggle_pins_shortcut.as_ref(), toggle_ok), has_pins);
        add(MENU_CLOSE_PINS, "关闭全部贴图", has_pins);
        sep();
        add(MENU_SETTINGS, "设置…", true);
        if !models_ready() {
            add(MENU_MODELS, if is_downloading() { "正在下载文字识别组件…" } else { "下载文字识别组件…" }, !is_downloading());
        }
        if updater::is_available() {
            add(MENU_UPDATES, "检查更新…", true);
        }
        sep();
        add(MENU_QUIT, "退出 Shotlate", true);
        let mut p = POINT::default();
        let _ = GetCursorPos(&mut p);
        // Without this the menu doesn't close when clicking elsewhere (documented TrackPopupMenu quirk).
        let _ = SetForegroundWindow(hwnd);
        let cmd = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_RIGHTBUTTON, p.x, p.y, None, hwnd, None).0 as usize;
        let _ = PostMessageW(Some(hwnd), WM_NULL, WPARAM(0), LPARAM(0));
        let _ = DestroyMenu(menu);
        match cmd {
            // Let the menu fade before the screen freezes.
            MENU_CAPTURE => {
                SetTimer(Some(hwnd), 7, 200, None);
            }
            MENU_TOGGLE_PINS => pin::toggle_hidden(),
            MENU_CLOSE_PINS => pin::close_all(),
            MENU_SETTINGS => settings_window::show(),
            MENU_MODELS => offer_model_download(None),
            MENU_UPDATES => updater::check_with_ui(),
            MENU_QUIT => {
                let _ = DestroyWindow(hwnd);
            }
            _ => {}
        }
    }
}

unsafe extern "system" fn proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let taskbar_created = STATE.with(|s| s.try_borrow().map(|s| s.taskbar_created).unwrap_or(0));
    match msg {
        util::WM_APP_TASK => {
            util::drain_tasks();
            return LRESULT(0);
        }
        WM_HOTKEY => {
            match wp.0 as i32 {
                HOTKEY_CAPTURE => overlay::begin(),
                HOTKEY_TOGGLE_PINS => pin::toggle_hidden(),
                _ => {}
            }
            return LRESULT(0);
        }
        WM_APP_TRAY => {
            // NOTIFYICON_VERSION_4: the event is in the low word of lParam. A right click sends WM_RBUTTONUP
            // and then WM_CONTEXTMENU (the keyboard only the latter); handling both would open the menu twice.
            match util::loword(lp.0 as usize) as u32 {
                WM_LBUTTONUP => overlay::begin(),
                WM_CONTEXTMENU => show_tray_menu(hwnd),
                _ => {}
            }
            return LRESULT(0);
        }
        WM_APP_SHOW_SETTINGS => {
            settings_window::show();
            return LRESULT(0);
        }
        WM_TIMER if wp.0 == 7 => {
            unsafe {
                let _ = KillTimer(Some(hwnd), 7);
            }
            overlay::begin();
            return LRESULT(0);
        }
        WM_DESTROY => {
            let data = tray_data(hwnd);
            unsafe {
                let _ = Shell_NotifyIconW(NIM_DELETE, &data);
            }
            overlay::end(None);
            pin::close_all();
            updater::cleanup();
            unsafe { PostQuitMessage(0) };
            return LRESULT(0);
        }
        m if m == taskbar_created && m != 0 => {
            // Explorer restarted: the tray icon has to be added again.
            add_tray(hwnd);
            return LRESULT(0);
        }
        _ => {}
    }
    unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
}

/// Asks the WinSparkle installer to close us before updating.
pub fn quit() {
    unsafe {
        let _ = PostMessageW(Some(util::app_window()), WM_CLOSE, WPARAM(0), LPARAM(0));
    }
}

/// Starts the tray app and runs until it quits. Returns the process exit code.
pub fn run() -> i32 {
    unsafe {
        // Only one copy: a second launch opens the running copy's settings instead.
        let _mutex = CreateMutexW(None, true, w!("Local\\Shotlate.SingleInstance"));
        if GetLastError() == ERROR_ALREADY_EXISTS {
            if let Ok(existing) = FindWindowW(CLASS, PCWSTR::null()) {
                // We hold the foreground right (the user just launched us); pass it on so the settings come to the front.
                let mut pid = 0u32;
                GetWindowThreadProcessId(existing, Some(&mut pid));
                let _ = AllowSetForegroundWindow(pid);
                let _ = PostMessageW(Some(existing), WM_APP_SHOW_SETTINGS, WPARAM(0), LPARAM(0));
            }
            return 0;
        }
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        // SysLink and the hotkey control in Settings aren't registered until this is called.
        let icc = INITCOMMONCONTROLSEX {
            dwSize: std::mem::size_of::<INITCOMMONCONTROLSEX>() as u32,
            dwICC: ICC_STANDARD_CLASSES | ICC_HOTKEY_CLASS | ICC_LINK_CLASS,
        };
        let _ = InitCommonControlsEx(&icc);
    }
    util::install_crash_log();
    settings::init(app_paths::settings_file());
    crate::render::text::preload();

    let class = WNDCLASSEXW {
        cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
        lpfnWndProc: Some(proc),
        lpszClassName: CLASS,
        ..Default::default()
    };
    unsafe {
        RegisterClassExW(&class);
    }
    overlay::register();
    pin::register();
    hud::register();
    settings_window::register();

    // A hidden top-level window (not message-only: those don't get TaskbarCreated).
    let Ok(hwnd) = (unsafe { CreateWindowExW(WS_EX_TOOLWINDOW, CLASS, w!("Shotlate"), WS_POPUP, 0, 0, 0, 0, None, None, None, None) }) else {
        return 1;
    };
    util::set_app_window(hwnd);
    let taskbar_created = unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) };
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        s.hwnd = hwnd;
        s.taskbar_created = taskbar_created;
    });
    add_tray(hwnd);
    register_hotkeys();
    updater::start();
    first_launch();

    let mut msg = MSG::default();
    unsafe {
        // GetMessage returns -1 on error, which `as_bool` would treat as a message forever.
        while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
            if settings_window::is_dialog_message(&msg) {
                continue;
            }
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    0
}

/// Once: say where Shotlate lives and offer the recognition models.
fn first_launch() {
    if settings::get().models_prompted {
        return;
    }
    settings::update(|s| s.models_prompted = true);
    let shortcut = settings::get().capture_shortcut.display();
    if models_ready() {
        hud::show(&format!("Shotlate 已在托盘运行 · 按 {shortcut} 截图"));
        return;
    }
    util::run_on_ui(move || {
        let mb = ocr::models::TOTAL_BYTES as f64 / 1_000_000.0;
        let text = format!(
            "Shotlate 已在任务栏右下角的托盘里运行，按 {shortcut} 截图。\n\n识别文字和翻译需要先下载文字识别组件（约 {mb:.0} MB），只需下载一次。现在下载吗？\n\n不下载也能正常截图、标注、复制、保存和贴图，以后可以在 设置 → 通用 里下载。"
        );
        if dialogs::ask(None, "欢迎使用 Shotlate", &text) {
            start_model_download();
        }
    });
}
