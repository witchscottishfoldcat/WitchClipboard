#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::{
    borrow::Cow,
    collections::HashMap,
    io::Cursor,
    path::Path,
    sync::{
        atomic::{AtomicI64, AtomicIsize, AtomicU32, Ordering},
        mpsc, Arc, Mutex,
    },
    thread,
    time::Duration,
};

use arboard::{Clipboard, ImageData};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use serde_json::Value;
use sha2::{Digest, Sha256};
use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Emitter, Manager, State, WebviewUrl, WebviewWindow, WebviewWindowBuilder,
    PhysicalPosition, Rect, WindowEvent,
};
use tauri_plugin_global_shortcut::Shortcut;
use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};

mod classify;
mod cross_device;
mod crypto;
mod model;
mod platform;
mod settings;
mod storage;
mod webdav_sync;

use model::{ListQuery, ListResult, PasteOutcome, Stats};
use settings::SettingsStore;
use storage::{NewItem, SqliteStore};

const HIDDEN_RESTART_MARKER: &str = ".restart-hidden";

struct AppState {
    store: SqliteStore,
    settings: SettingsStore,
    clipboard_gate: Mutex<()>,
    target_hwnd: AtomicIsize,
    own_clipboard_sequence: AtomicU32,
    main_shortcut_id: AtomicU32,
    quick_shortcuts: Mutex<HashMap<u32, usize>>,
    hidden_at: AtomicI64,
    window_hidden_at: Mutex<HashMap<String, i64>>,
    cross_device: cross_device::CrossDevice,
    webdav: webdav_sync::WebDavSync,
    phone_events: Mutex<Option<mpsc::Receiver<cross_device::Incoming>>>,
}

impl AppState {
    fn open() -> Result<Self, String> {
        let data_dir = crypto::canonical_data_dir();
        let (phone_sender, phone_receiver) = mpsc::channel();
        let store = SqliteStore::open(&data_dir).map_err(|error| error.to_string())?;
        let webdav = webdav_sync::WebDavSync::new(&store);
        Ok(Self {
            store,
            settings: SettingsStore::load(&data_dir),
            clipboard_gate: Mutex::new(()),
            target_hwnd: AtomicIsize::new(0),
            own_clipboard_sequence: AtomicU32::new(0),
            main_shortcut_id: AtomicU32::new(0),
            quick_shortcuts: Mutex::new(HashMap::new()),
            hidden_at: AtomicI64::new(0),
            window_hidden_at: Mutex::new(HashMap::new()),
            cross_device: cross_device::CrossDevice::new(phone_sender, &data_dir),
            webdav,
            phone_events: Mutex::new(Some(phone_receiver)),
        })
    }
}

fn text_hash(text: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(text.as_bytes());
    format!("{:x}", hasher.finalize())
}

fn insert_text(
    state: &AppState,
    text: String,
    html: Option<String>,
    source_app: Option<String>,
) -> bool {
    if text.trim().is_empty() || text.len() > 1_000_000 {
        return false;
    }
    let hash = text_hash(&text);
    let added = state
        .store
        .add(NewItem {
            kind: "text".into(),
            text: Some(text.clone()),
            html,
            preview: classify::make_preview(&text, 160),
            auto_kind: classify::classify(&text).into(),
            hash,
            blob_name: None,
            thumb: None,
            width: None,
            height: None,
            bytes: text.len(),
            source_app,
        })
        .is_ok();
    if added {
        let settings = state.settings.get();
        let _ = state.store.prune(settings.max_items, settings.max_days);
    }
    added
}

fn insert_files(state: &AppState, paths: Vec<String>, source_app: Option<String>) -> bool {
    if paths.is_empty() {
        return false;
    }
    let text = paths.join("\n");
    let hash = text_hash(&format!("files:{}", paths.join("\0")));

    let first_name = Path::new(&paths[0])
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| paths[0].clone());
    let preview = if paths.len() == 1 {
        first_name
    } else {
        format!("{first_name} 等 {} 个文件", paths.len())
    };
    let bytes = paths
        .iter()
        .filter_map(|path| std::fs::metadata(path).ok())
        .filter(|metadata| metadata.is_file())
        .map(|metadata| metadata.len() as usize)
        .sum();

    let added = state
        .store
        .add(NewItem {
            kind: "files".into(),
            text: Some(text),
            html: None,
            preview,
            auto_kind: "path".into(),
            hash,
            blob_name: None,
            thumb: None,
            width: None,
            height: None,
            bytes,
            source_app,
        })
        .is_ok();
    if added {
        let settings = state.settings.get();
        let _ = state.store.prune(settings.max_items, settings.max_days);
    }
    added
}

fn capture_current_clipboard(state: &AppState) -> bool {
    let source_app = platform::foreground_exe();
    let settings = state.settings.get();
    if settings.skip_sensitive && platform::has_sensitive_marker() {
        return false;
    }
    if settings.skip_sensitive
        && source_app.as_ref().is_some_and(|exe| {
            settings.sensitive_apps.iter().any(|name| {
                !name.trim().is_empty() && exe.to_lowercase().contains(&name.trim().to_lowercase())
            })
        })
    {
        return false;
    }
    if let Some(paths) = platform::read_clipboard_files() {
        return insert_files(state, paths, source_app);
    }

    let Ok(mut clipboard) = Clipboard::new() else {
        return false;
    };
    let html = clipboard.get().html().ok();
    if let Ok(text) = clipboard.get_text() {
        return insert_text(state, text, html, source_app);
    }
    let Ok(image) = clipboard.get_image() else {
        return false;
    };
    let Some(rgba) = image::RgbaImage::from_raw(
        image.width as u32,
        image.height as u32,
        image.bytes.into_owned(),
    ) else {
        return false;
    };
    let mut png = Vec::new();
    if image::DynamicImage::ImageRgba8(rgba.clone())
        .write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png)
        .is_err()
    {
        return false;
    }
    let hash = text_hash_bytes(&png);
    let Ok(blob_name) = state.store.put_blob(&hash, &png) else {
        return false;
    };
    let thumb_image = image::imageops::thumbnail(&rgba, 220, 220);
    let mut thumb = Vec::new();
    let _ = image::DynamicImage::ImageRgba8(thumb_image)
        .write_to(&mut Cursor::new(&mut thumb), image::ImageFormat::Png);
    state
        .store
        .add(NewItem {
            kind: "image".into(),
            text: None,
            html: None,
            preview: format!("图片 {}×{}", image.width, image.height),
            auto_kind: "plain".into(),
            hash,
            blob_name: Some(blob_name),
            thumb: Some(thumb),
            width: Some(image.width as u32),
            height: Some(image.height as u32),
            bytes: png.len(),
            source_app,
        })
        .is_ok()
}

fn text_hash_bytes(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

fn start_text_monitor(app: AppHandle, state: Arc<AppState>) {
    let notifications = match platform::start_clipboard_notifications() {
        Ok(notifications) => notifications,
        Err(error) => {
            eprintln!("native clipboard listener unavailable: {error}");
            return;
        }
    };
    thread::spawn(move || {
        let mut last_sequence = platform::clipboard_sequence();
        while notifications.recv().is_ok() {
            // Own writes hold this gate until their sequence number is published. This removes
            // the race where WM_CLIPBOARDUPDATE can arrive before the writer marks it ignored.
            let _clipboard_guard = state
                .clipboard_gate
                .lock()
                .expect("clipboard gate lock poisoned");
            let sequence = platform::clipboard_sequence();
            if sequence == last_sequence {
                continue;
            }
            last_sequence = sequence;
            if sequence == state.own_clipboard_sequence.load(Ordering::Acquire) {
                continue;
            }
            if capture_current_clipboard(&state) {
                let _ = app.emit("witchcat://changed", ());
            }
        }
    });
}

fn epoch_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

fn remember_paste_target(app: &AppHandle) {
    let state = app.state::<Arc<AppState>>();
    if let Some(hwnd) = platform::foreground_window() {
        let belongs_to_this_process =
            platform::window_process_id(hwnd).is_some_and(|pid| pid == std::process::id());
        if !belongs_to_this_process {
            state.target_hwnd.store(hwnd, Ordering::Release);
        }
    }
}

fn hide_window(window: &WebviewWindow) {
    let hidden_at = epoch_ms();
    let state = window.state::<Arc<AppState>>();
    state.hidden_at.store(hidden_at, Ordering::Release);
    if let Ok(mut windows) = state.window_hidden_at.lock() {
        windows.insert(window.label().to_string(), hidden_at);
    }
    let handle = window.app_handle().clone();
    let label = window.label().to_string();
    thread::spawn(move || {
        thread::sleep(std::time::Duration::from_secs(60));
        let state = handle.state::<Arc<AppState>>();
        let still_idle = state
            .window_hidden_at
            .lock()
            .ok()
            .and_then(|windows| windows.get(&label).copied())
            == Some(hidden_at);
        if still_idle {
            if let Some(idle_window) = handle.get_webview_window(&label) {
                if !idle_window.is_visible().unwrap_or(false) {
                    let _ = idle_window.destroy();
                }
            }
            if let Ok(mut windows) = state.window_hidden_at.lock() {
                windows.remove(&label);
            }
            if !cfg!(debug_assertions)
                && !state.cross_device.status().running
                && ["main", "mini"].into_iter().all(|window_label| {
                    handle
                        .get_webview_window(window_label)
                        .is_none_or(|window| !window.is_visible().unwrap_or(false))
                })
                && std::fs::write(
                    state.store.data_dir().join(HIDDEN_RESTART_MARKER),
                    b"restart without webview",
                )
                .is_ok()
            {
                handle.request_restart();
            }
        }
    });
    let _ = window.hide();
}

fn watch_panel_window(window: &WebviewWindow) {
    let watched = window.clone();
    window.on_window_event(move |event| match event {
        WindowEvent::CloseRequested { api, .. } => {
            api.prevent_close();
            hide_window(&watched);
        }
        WindowEvent::Focused(true) => {
            if let Err(error) = platform::set_window_icons(&watched) {
                eprintln!("failed to refresh window icons: {error}");
            }
        }
        WindowEvent::Focused(false) if std::env::var_os("WCC_NO_AUTOHIDE").is_none() => {
            hide_window(&watched);
        }
        _ => {}
    });
}

fn position_mini_near_tray(window: &WebviewWindow, tray_rect: &Rect) {
    let scale_factor = window.scale_factor().unwrap_or(1.0);
    let tray_position = tray_rect.position.to_physical::<i32>(scale_factor);
    let tray_size = tray_rect.size.to_physical::<u32>(scale_factor);
    let Ok(window_size) = window.inner_size() else {
        return;
    };

    let monitor = window
        .available_monitors()
        .ok()
        .and_then(|monitors| {
            monitors.into_iter().find(|monitor| {
                let area = monitor.work_area();
                tray_position.x >= area.position.x
                    && tray_position.x < area.position.x + area.size.width as i32
                    && tray_position.y >= area.position.y
                    && tray_position.y < area.position.y + area.size.height as i32
            })
        })
        .or_else(|| window.current_monitor().ok().flatten());
    let Some(monitor) = monitor else {
        return;
    };

    let area = monitor.work_area();
    let margin = 8;
    let max_x = area.position.x + area.size.width as i32 - window_size.width as i32 - margin;
    let max_y = area.position.y + area.size.height as i32 - window_size.height as i32 - margin;
    let x = (tray_position.x + tray_size.width as i32 - window_size.width as i32)
        .clamp(area.position.x + margin, max_x);
    let y = (tray_position.y - window_size.height as i32 - margin)
        .clamp(area.position.y + margin, max_y);
    let _ = window.set_position(PhysicalPosition::new(x, y));
}

fn show_existing_window(app: &AppHandle, label: &str, tray_rect: Option<&Rect>) -> bool {
    let Some(window) = app.get_webview_window(label) else {
        return false;
    };
    if let Ok(mut windows) = app.state::<Arc<AppState>>().window_hidden_at.lock() {
        windows.remove(label);
    }
    remember_paste_target(app);
    for other in ["main", "mini"] {
        if other != label {
            if let Some(other_window) = app.get_webview_window(other) {
                hide_window(&other_window);
            }
        }
    }
    if label == "mini" {
        if let Some(tray_rect) = tray_rect {
            position_mini_near_tray(&window, tray_rect);
        }
    }
    let _ = window.show();
    let _ = window.set_focus();
    if let Err(error) = platform::set_window_icons(&window) {
        eprintln!("failed to set window icons: {error}");
    }
    let _ = app.emit("witchcat://panel-shown", ());
    let _ = app.emit("witchcat://changed", ());
    true
}

fn show_mini_window(app: &AppHandle, tray_rect: Option<Rect>) {
    if show_existing_window(app, "mini", tray_rect.as_ref()) {
        return;
    }
    remember_paste_target(app);
    let handle = app.clone();
    thread::spawn(move || {
        let built = WebviewWindowBuilder::new(
            &handle,
            "mini",
            WebviewUrl::App("index.html?mode=mini".into()),
        )
        .title("Witch Clipboard")
        .inner_size(280.0, 390.0)
        .decorations(false)
        .transparent(true)
        .shadow(false)
        .resizable(false)
        .skip_taskbar(true)
        .always_on_top(true)
        .visible(false)
        .build();
        match built {
            Ok(window) => {
                watch_panel_window(&window);
                if let Some(tray_rect) = tray_rect.as_ref() {
                    position_mini_near_tray(&window, tray_rect);
                } else {
                    let _ = window.center();
                }
                let _ = show_existing_window(&handle, "mini", tray_rect.as_ref());
            }
            Err(error) => eprintln!("failed to create mini panel: {error}"),
        }
    });
}

fn show_main_window(app: &AppHandle) {
    if show_existing_window(app, "main", None) {
        return;
    }
    remember_paste_target(app);
    let handle = app.clone();
    thread::spawn(move || {
        let built =
            WebviewWindowBuilder::new(&handle, "main", WebviewUrl::App("index.html".into()))
                .title("Witch Clipboard")
                .inner_size(820.0, 540.0)
                .min_inner_size(680.0, 440.0)
                .center()
                .decorations(false)
                .transparent(true)
                .shadow(false)
                .resizable(true)
                .skip_taskbar(true)
                .always_on_top(true)
                .visible(false)
                .build();
        match built {
            Ok(window) => {
                watch_panel_window(&window);
                let _ = show_existing_window(&handle, "main", None);
            }
            Err(error) => eprintln!("failed to create main panel: {error}"),
        }
    });
}

fn toggle_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        if window.is_visible().unwrap_or(false) {
            hide_window(&window);
            return;
        }
    }
    show_main_window(app);
}

fn toggle_tray_window(app: &AppHandle, tray_rect: Option<Rect>) {
    let state = app.state::<Arc<AppState>>();
    if epoch_ms() - state.hidden_at.load(Ordering::Acquire) < 400 {
        return;
    }
    let label = if state.settings.get().tray_opens_mini {
        "mini"
    } else {
        "main"
    };
    if let Some(window) = app.get_webview_window(label) {
        if window.is_visible().unwrap_or(false) {
            hide_window(&window);
            return;
        }
    }
    if label == "mini" {
        show_mini_window(app, tray_rect);
    } else {
        show_main_window(app);
    }
}

fn register_shortcuts(
    app: &AppHandle,
    state: &AppState,
    allow_fallback: bool,
) -> Result<(), String> {
    app.global_shortcut()
        .unregister_all()
        .map_err(|e| e.to_string())?;
    let settings = state.settings.get();
    let requested = settings
        .hotkey
        .parse::<Shortcut>()
        .map_err(|e| e.to_string())?;
    let main = match app.global_shortcut().register(requested) {
        Ok(()) => requested,
        Err(error) if allow_fallback => {
            eprintln!(
                "{} unavailable ({error}); using Ctrl+Alt+Shift+V",
                settings.hotkey
            );
            let fallback = "Ctrl+Alt+Shift+V"
                .parse::<Shortcut>()
                .map_err(|e| e.to_string())?;
            app.global_shortcut()
                .register(fallback)
                .map_err(|e| e.to_string())?;
            fallback
        }
        Err(error) => return Err(error.to_string()),
    };
    state.main_shortcut_id.store(main.id(), Ordering::Release);
    let mut quick = state
        .quick_shortcuts
        .lock()
        .map_err(|_| "quick shortcut lock poisoned".to_string())?;
    quick.clear();
    for index in 0..9 {
        let expression = format!("{}+{}", settings.quick_paste_modifiers, index + 1);
        if let Ok(shortcut) = expression.parse::<Shortcut>() {
            if app.global_shortcut().register(shortcut).is_ok() {
                quick.insert(shortcut.id(), index);
            }
        }
    }
    Ok(())
}

fn handle_shortcut(app: &AppHandle, shortcut: &Shortcut) {
    let state = app.state::<Arc<AppState>>();
    if shortcut.id() == state.main_shortcut_id.load(Ordering::Acquire) {
        toggle_main_window(app);
        return;
    }
    let index = state
        .quick_shortcuts
        .lock()
        .ok()
        .and_then(|map| map.get(&shortcut.id()).copied());
    let Some(index) = index else { return };
    if let Some(hwnd) = platform::foreground_window() {
        if !platform::window_process_id(hwnd).is_some_and(|pid| pid == std::process::id()) {
            state.target_hwnd.store(hwnd, Ordering::Release);
        }
    }
    let Ok(list) = state.store.list(&ListQuery {
        limit: Some(9),
        ..Default::default()
    }) else {
        return;
    };
    let Some(item) = list.items.get(index) else {
        return;
    };
    let id = item.id;
    let owned = state.inner().clone();
    let app = app.clone();
    thread::spawn(move || {
        if write_item(&owned, id).is_ok() {
            let target = owned.target_hwnd.load(Ordering::Acquire);
            let _ = platform::restore_and_paste(target);
            let _ = app.emit("witchcat://changed", ());
        }
    });
}

#[tauri::command]
fn clipboard_list(state: State<'_, Arc<AppState>>, query: ListQuery) -> Result<ListResult, String> {
    state.store.list(&query).map_err(|e| e.to_string())
}

#[tauri::command]
fn clipboard_stats(state: State<'_, Arc<AppState>>) -> Result<Stats, String> {
    state.store.stats().map_err(|e| e.to_string())
}

#[tauri::command]
fn clipboard_tags(state: State<'_, Arc<AppState>>) -> Result<Vec<String>, String> {
    state.store.tags().map_err(|e| e.to_string())
}

#[tauri::command]
fn clipboard_set_tags(state: State<'_, Arc<AppState>>, id: i64, tags: Vec<String>) {
    let _ = state.store.set_tags(id, &tags);
}

#[tauri::command]
fn clipboard_image(state: State<'_, Arc<AppState>>, id: i64) -> Result<Option<String>, String> {
    state
        .store
        .image_png(id)
        .map(|png| png.map(|data| format!("data:image/png;base64,{}", BASE64.encode(data))))
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn clipboard_related(
    state: State<'_, Arc<AppState>>,
    id: i64,
    limit: Option<usize>,
) -> Result<Vec<model::ClipItem>, String> {
    state
        .store
        .related(id, limit.unwrap_or(5))
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn get_settings(state: State<'_, Arc<AppState>>) -> model::Settings {
    state.settings.get()
}

#[tauri::command]
fn save_settings(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    patch: Value,
) -> Result<model::Settings, String> {
    let before = state.settings.get();
    let next = state.settings.save_patch(patch)?;
    if next.hotkey != before.hotkey || next.quick_paste_modifiers != before.quick_paste_modifiers {
        if let Err(error) = register_shortcuts(&app, &state, false) {
            let _ = state
                .settings
                .save_patch(serde_json::to_value(&before).map_err(|e| e.to_string())?);
            let _ = register_shortcuts(&app, &state, true);
            return Err(error);
        }
    }
    if next.auto_launch != before.auto_launch && !platform::set_auto_launch(next.auto_launch) {
        let _ = state
            .settings
            .save_patch(serde_json::to_value(&before).map_err(|e| e.to_string())?);
        return Err("开机自启设置失败".to_string());
    }
    if next.max_items != before.max_items || next.max_days != before.max_days {
        state
            .store
            .prune(next.max_items, next.max_days)
            .map_err(|error| error.to_string())?;
    }
    let _ = app.emit("witchcat://changed", ());
    Ok(next)
}

#[tauri::command]
fn security_info(state: State<'_, Arc<AppState>>) -> Value {
    serde_json::json!({
        "osProtected":state.store.is_os_protected(),"dbEncrypted":true,"nativeAvailable":true,
        "memoryFallback":false,"dataDir":state.store.data_dir().to_string_lossy()
    })
}

#[tauri::command]
fn hide_panel(window: WebviewWindow) {
    hide_window(&window);
}

#[tauri::command]
fn expand_panel(app: AppHandle, window: WebviewWindow) {
    hide_window(&window);
    show_main_window(&app);
}

#[tauri::command]
fn reveal_file(state: State<'_, Arc<AppState>>, id: i64) -> Result<(), String> {
    let item = state
        .store
        .get(id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "not-found".to_string())?;
    if item.kind != "files" {
        return Ok(());
    }
    let Some(first) = item.text.and_then(|v| v.lines().next().map(str::to_string)) else {
        return Ok(());
    };
    std::process::Command::new("explorer.exe")
        .arg(format!("/select,{first}"))
        .spawn()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn open_data_dir(state: State<'_, Arc<AppState>>) -> Result<(), String> {
    std::process::Command::new("explorer.exe")
        .arg(state.store.data_dir())
        .spawn()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn cross_device_start(state: State<'_, Arc<AppState>>) -> Result<cross_device::Status, String> {
    state.cross_device.start()?;
    if let Ok(mut clipboard) = Clipboard::new() {
        if let Ok(text) = clipboard.get_text() {
            let _ = state.cross_device.publish_text(text);
        } else if let Ok(image) = clipboard.get_image() {
            if let Some(rgba) = image::RgbaImage::from_raw(
                image.width as u32,
                image.height as u32,
                image.bytes.into_owned(),
            ) {
                let mut png = Vec::new();
                if image::DynamicImage::ImageRgba8(rgba)
                    .write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png)
                    .is_ok()
                {
                    let _ = state.cross_device.publish_image(png, "剪贴板图片".into());
                }
            }
        }
    }
    Ok(state.cross_device.status())
}
#[tauri::command]
fn cross_device_stop(state: State<'_, Arc<AppState>>) -> cross_device::Status {
    state.cross_device.stop()
}
#[tauri::command]
fn cross_device_status(state: State<'_, Arc<AppState>>) -> cross_device::Status {
    state.cross_device.status()
}
#[tauri::command]
fn cross_device_send(
    state: State<'_, Arc<AppState>>,
    id: i64,
) -> Result<cross_device::SendResult, String> {
    let item = state
        .store
        .get(id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "not-found".to_string())?;
    Ok(match item.kind.as_str() {
        "text" => state
            .cross_device
            .publish_text(item.text.unwrap_or_default()),
        "image" => state.cross_device.publish_image(
            state
                .store
                .image_png(id)
                .map_err(|e| e.to_string())?
                .unwrap_or_default(),
            item.preview,
        ),
        "files" => state.cross_device.publish_files(
            item.text
                .unwrap_or_default()
                .lines()
                .filter(|path| !path.is_empty())
                .map(str::to_string)
                .collect(),
        ),
        _ => cross_device::SendResult {
            ok: false,
            reason: Some("unsupported"),
        },
    })
}

#[tauri::command]
fn cross_device_approve(
    state: State<'_, Arc<AppState>>,
    device_id: String,
) -> Result<cross_device::Status, String> {
    state.cross_device.approve_device(&device_id)
}

#[tauri::command]
fn cross_device_reject(state: State<'_, Arc<AppState>>, device_id: String) -> cross_device::Status {
    state.cross_device.reject_device(&device_id)
}

#[tauri::command]
fn cross_device_cancel_transfer(
    state: State<'_, Arc<AppState>>,
    transfer_id: String,
) -> cross_device::Status {
    state.cross_device.cancel_transfer(&transfer_id)
}

#[tauri::command]
fn cross_device_retry_transfer(
    state: State<'_, Arc<AppState>>,
    transfer_id: String,
) -> cross_device::Status {
    state.cross_device.retry_transfer(&transfer_id)
}

#[tauri::command]
fn webdav_config(state: State<'_, Arc<AppState>>) -> Result<webdav_sync::PublicConfig, String> {
    state.webdav.config()
}

#[tauri::command]
fn webdav_save_config(
    state: State<'_, Arc<AppState>>,
    patch: webdav_sync::ConfigPatch,
) -> Result<webdav_sync::PublicConfig, String> {
    state.webdav.save_config(patch)
}

#[tauri::command]
fn webdav_copy_sync_key(state: State<'_, Arc<AppState>>) -> Result<(), String> {
    let sync_key = state.webdav.reveal_sync_key()?;
    let _clipboard_guard = state
        .clipboard_gate
        .lock()
        .map_err(|error| error.to_string())?;
    Clipboard::new()
        .and_then(|mut clipboard| clipboard.set_text(sync_key))
        .map_err(|error| error.to_string())?;
    state
        .own_clipboard_sequence
        .store(platform::clipboard_sequence(), Ordering::Release);
    Ok(())
}

#[tauri::command]
fn webdav_status(state: State<'_, Arc<AppState>>) -> webdav_sync::SyncStatus {
    state.webdav.status()
}

#[tauri::command]
async fn webdav_sync_now(
    state: State<'_, Arc<AppState>>,
) -> Result<webdav_sync::SyncStatus, String> {
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || state.webdav.sync_now(&state.store))
        .await
        .map_err(|error| error.to_string())?
}

#[tauri::command]
fn toggle_pin(app: AppHandle, state: State<'_, Arc<AppState>>, id: i64) -> Result<(), String> {
    state.store.toggle_pin(id).map_err(|error| error.to_string())?;
    let _ = app.emit("witchcat://changed", ());
    Ok(())
}

#[tauri::command]
fn remove_item(app: AppHandle, state: State<'_, Arc<AppState>>, id: i64) -> Result<(), String> {
    state.store.remove(id).map_err(|error| error.to_string())?;
    let _ = app.emit("witchcat://changed", ());
    Ok(())
}

#[tauri::command]
fn clear_all(state: State<'_, Arc<AppState>>) {
    let _ = state.store.clear_all();
}

fn write_item(state: &AppState, id: i64) -> Result<(), String> {
    let item = state
        .store
        .get(id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "not-found".to_string())?;
    let _clipboard_guard = state
        .clipboard_gate
        .lock()
        .map_err(|error| error.to_string())?;

    match (item.kind.as_str(), item.text) {
        ("files", Some(text)) => {
            let paths = text
                .lines()
                .filter(|path| !path.is_empty())
                .map(str::to_string)
                .collect::<Vec<_>>();
            if !platform::write_clipboard_files(&paths) {
                return Err("clipboard-write-failed".to_string());
            }
        }
        ("image", _) => {
            let png = state
                .store
                .image_png(id)
                .map_err(|e| e.to_string())?
                .ok_or_else(|| "not-found".to_string())?;
            let decoded = image::load_from_memory_with_format(&png, image::ImageFormat::Png)
                .map_err(|e| e.to_string())?
                .to_rgba8();
            let (width, height) = decoded.dimensions();
            Clipboard::new()
                .and_then(|mut clipboard| {
                    clipboard.set_image(ImageData {
                        width: width as usize,
                        height: height as usize,
                        bytes: Cow::Owned(decoded.into_raw()),
                    })
                })
                .map_err(|e| e.to_string())?;
        }
        (_, Some(text)) => {
            let mut clipboard = Clipboard::new().map_err(|error| error.to_string())?;
            if let Some(html) = item.html {
                clipboard
                    .set_html(html, Some(text))
                    .map_err(|error| error.to_string())?;
            } else {
                clipboard
                    .set_text(text)
                    .map_err(|error| error.to_string())?;
            }
        }
        _ => return Err("unsupported-kind".to_string()),
    }
    state
        .own_clipboard_sequence
        .store(platform::clipboard_sequence(), Ordering::Release);
    let _ = state.store.touch(id);
    Ok(())
}

#[tauri::command]
fn copy_item(state: State<'_, Arc<AppState>>, id: i64) -> Result<(), String> {
    write_item(&state, id)
}

#[tauri::command]
fn paste_item(window: WebviewWindow, state: State<'_, Arc<AppState>>, id: i64) -> PasteOutcome {
    if write_item(&state, id).is_err() {
        return PasteOutcome {
            ok: false,
            reason: Some("not-found"),
        };
    }

    let hidden = state.settings.get().hide_after_paste;
    if hidden {
        hide_window(&window);
    }
    let target = state.target_hwnd.load(Ordering::Acquire);
    match platform::restore_and_paste(target) {
        Ok(()) => PasteOutcome {
            ok: true,
            reason: None,
        },
        Err(reason) => {
            if hidden {
                let _ = window.show();
                let _ = window.set_focus();
            }
            PasteOutcome {
                ok: false,
                reason: Some(reason),
            }
        }
    }
}

fn main() {
    let hidden_marker = crypto::canonical_data_dir().join(HIDDEN_RESTART_MARKER);
    let start_hidden = std::env::args().any(|argument| argument == "--hidden")
        || std::fs::remove_file(hidden_marker).is_ok();
    let state = match AppState::open() {
        Ok(state) => Arc::new(state),
        Err(error) => {
            platform::show_fatal_error(&format!(
                "无法打开加密剪贴板历史。为避免数据损坏，应用没有创建空库，也没有覆盖原数据。\n\n数据目录：{}\n错误：{error}\n\n你仍可安装上一版 Electron 客户端回退。",
                crypto::canonical_data_dir().display()
            ));
            std::process::exit(2);
        }
    };
    let updater = option_env!("WCC_UPDATER_PUBLIC_KEY")
        .filter(|key| !key.trim().is_empty())
        .map_or_else(tauri_plugin_updater::Builder::new, |key| {
            tauri_plugin_updater::Builder::new().pubkey(key)
        });
    tauri::Builder::default()
        .manage(state)
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            show_main_window(app);
        }))
        .plugin(tauri_plugin_process::init())
        .plugin(updater.build())
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, shortcut, event| {
                    if event.state() == ShortcutState::Pressed {
                        handle_shortcut(app, shortcut);
                    }
                })
                .build(),
        )
        .invoke_handler(tauri::generate_handler![
            clipboard_list,
            clipboard_stats,
            clipboard_tags,
            clipboard_set_tags,
            toggle_pin,
            remove_item,
            clear_all,
            copy_item,
            paste_item,
            clipboard_image,
            clipboard_related,
            get_settings,
            save_settings,
            security_info,
            hide_panel,
            expand_panel,
            reveal_file,
            open_data_dir,
            cross_device_start,
            cross_device_stop,
            cross_device_status,
            cross_device_send,
            cross_device_approve,
            cross_device_reject,
            cross_device_cancel_transfer,
            cross_device_retry_transfer,
            webdav_config,
            webdav_save_config,
            webdav_copy_sync_key,
            webdav_status,
            webdav_sync_now,
        ])
        .setup(move |app| {
            let state = app.state::<Arc<AppState>>().inner().clone();
            register_shortcuts(app.handle(), &state, true).map_err(std::io::Error::other)?;

            let show = MenuItem::with_id(app, "show", "显示 / 隐藏", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show, &quit])?;
            TrayIconBuilder::new()
                .icon(
                    app.default_window_icon()
                        .expect("Witch Clipboard icon missing")
                        .clone(),
                )
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "show" => toggle_tray_window(app, None),
                    "quit" => app.exit(0),
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        rect,
                        ..
                    } = event
                    {
                        toggle_tray_window(tray.app_handle(), Some(rect));
                    }
                })
                .build(app)?;

            if !start_hidden {
                show_main_window(app.handle());
            }

            let state = app.state::<Arc<AppState>>().inner().clone();
            let phone = state
                .phone_events
                .lock()
                .expect("phone receiver lock poisoned")
                .take();
            if let Some(phone) = phone {
                let app_handle = app.handle().clone();
                let transfer_state = state.clone();
                thread::spawn(move || {
                    while let Ok(event) = phone.recv() {
                        match event {
                            cross_device::Incoming::Text(text) => {
                                if let Ok(_guard) = transfer_state.clipboard_gate.lock() {
                                    if let Ok(mut clipboard) = Clipboard::new() {
                                        if clipboard.set_text(&text).is_ok() {
                                            transfer_state.own_clipboard_sequence.store(
                                                platform::clipboard_sequence(),
                                                Ordering::Release,
                                            );
                                        }
                                    }
                                }
                                let _ = insert_text(
                                    &transfer_state,
                                    text,
                                    None,
                                    Some("局域网设备".to_string()),
                                );
                            }
                            cross_device::Incoming::Files(paths) => {
                                if let Ok(_guard) = transfer_state.clipboard_gate.lock() {
                                    if platform::write_clipboard_files(&paths) {
                                        transfer_state.own_clipboard_sequence.store(
                                            platform::clipboard_sequence(),
                                            Ordering::Release,
                                        );
                                    }
                                }
                                let _ = insert_files(
                                    &transfer_state,
                                    paths,
                                    Some("局域网设备".to_string()),
                                );
                            }
                        }
                        let _ = app_handle.emit("witchcat://changed", ());
                    }
                });
            }
            let sync_state = state.clone();
            thread::spawn(move || {
                thread::sleep(Duration::from_secs(30));
                loop {
                    if sync_state
                        .webdav
                        .config()
                        .is_ok_and(|config| config.enabled)
                    {
                        let _ = sync_state.webdav.sync_now(&sync_state.store);
                    }
                    thread::sleep(Duration::from_secs(5 * 60));
                }
            });
            start_text_monitor(app.handle().clone(), state);
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("failed to run Witch Clipboard");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_state(path: &Path) -> AppState {
        let store = SqliteStore::open(path).unwrap();
        let webdav = webdav_sync::WebDavSync::new(&store);
        AppState {
            store,
            settings: SettingsStore::load(path),
            clipboard_gate: Mutex::new(()),
            target_hwnd: AtomicIsize::new(0),
            own_clipboard_sequence: AtomicU32::new(0),
            main_shortcut_id: AtomicU32::new(0),
            quick_shortcuts: Mutex::new(HashMap::new()),
            hidden_at: AtomicI64::new(0),
            window_hidden_at: Mutex::new(HashMap::new()),
            cross_device: {
                let (tx, _) = mpsc::channel();
                cross_device::CrossDevice::new(tx, path)
            },
            webdav,
            phone_events: Mutex::new(None),
        }
    }

    #[test]
    fn preview_collapses_whitespace_and_limits_length() {
        let input = format!("  第一行\n\n{}  ", "长".repeat(200));
        let preview = classify::make_preview(&input, 160);
        assert_eq!(preview, "第一行");
    }

    #[test]
    fn duplicate_text_moves_to_front_without_growing_history() {
        let directory = tempfile::tempdir().unwrap();
        let state = test_state(directory.path());
        assert!(insert_text(&state, "first".to_string(), None, None));
        assert!(insert_text(&state, "second".to_string(), None, None));
        assert!(insert_text(&state, "first".to_string(), None, None));

        let items = state.store.list(&ListQuery::default()).unwrap().items;
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].text.as_deref(), Some("first"));
        assert_eq!(items[0].use_count, 1);
    }

    #[test]
    fn file_list_is_stored_as_a_real_file_item() {
        let directory = tempfile::tempdir().unwrap();
        let state = test_state(directory.path());
        assert!(insert_files(
            &state,
            vec!["C:\\one.txt".to_string(), "C:\\two.png".to_string()],
            Some("explorer.exe".to_string()),
        ));
        let items = state.store.list(&ListQuery::default()).unwrap().items;
        assert_eq!(items[0].kind, "files");
        assert_eq!(items[0].auto_kind, "path");
        assert_eq!(items[0].source_app.as_deref(), Some("explorer.exe"));
        assert!(items[0].preview.contains("2 个文件"));
    }

    /// This test intentionally mutates the real Windows clipboard. Keep it ignored and run it
    /// only through scripts/system-clipboard-e2e.ps1 after all clipboard managers are stopped.
    #[cfg(windows)]
    #[test]
    #[ignore = "mutates the real Windows clipboard"]
    fn windows_system_clipboard_pipeline_round_trip() {
        assert_eq!(
            std::env::var("WCC_SYSTEM_CLIPBOARD_E2E").as_deref(),
            Ok("1"),
            "run this test through scripts/system-clipboard-e2e.ps1"
        );

        let directory = tempfile::tempdir().unwrap();
        let state = test_state(directory.path());
        let original_text = Clipboard::new()
            .ok()
            .and_then(|mut clipboard| clipboard.get_text().ok());

        let html = "<p><strong>Witch Clipboard</strong> HTML E2E</p>";
        let alternative = "Witch Clipboard HTML E2E";
        Clipboard::new()
            .unwrap()
            .set_html(html, Some(alternative))
            .unwrap();
        assert!(capture_current_clipboard(&state));
        let html_item = state.store.list(&ListQuery::default()).unwrap().items[0].clone();
        assert_eq!(html_item.kind, "text");
        assert_eq!(html_item.text.as_deref(), Some(alternative));
        assert!(html_item
            .html
            .as_deref()
            .is_some_and(|value| value.contains(html)));
        write_item(&state, html_item.id).unwrap();
        let mut clipboard = Clipboard::new().unwrap();
        assert_eq!(clipboard.get_text().unwrap(), alternative);
        assert!(clipboard.get().html().unwrap().contains(html));

        let rgba = vec![255, 0, 0, 255, 0, 128, 255, 255];
        Clipboard::new()
            .unwrap()
            .set_image(ImageData {
                width: 2,
                height: 1,
                bytes: Cow::Owned(rgba.clone()),
            })
            .unwrap();
        assert!(capture_current_clipboard(&state));
        let image_item = state.store.list(&ListQuery::default()).unwrap().items[0].clone();
        assert_eq!(image_item.kind, "image");
        assert_eq!((image_item.width, image_item.height), (Some(2), Some(1)));
        assert!(state.store.image_png(image_item.id).unwrap().is_some());
        write_item(&state, image_item.id).unwrap();
        let image = Clipboard::new().unwrap().get_image().unwrap();
        assert_eq!((image.width, image.height), (2, 1));
        assert_eq!(image.bytes.as_ref(), rgba.as_slice());

        let first = directory.path().join("clipboard-e2e-one.txt");
        let second = directory.path().join("剪贴板-e2e-two.txt");
        std::fs::write(&first, "one").unwrap();
        std::fs::write(&second, "two").unwrap();
        let paths = vec![
            first.to_string_lossy().into_owned(),
            second.to_string_lossy().into_owned(),
        ];
        assert!(platform::write_clipboard_files(&paths));
        assert!(capture_current_clipboard(&state));
        let files_item = state.store.list(&ListQuery::default()).unwrap().items[0].clone();
        assert_eq!(files_item.kind, "files");
        assert_eq!(files_item.text.as_deref(), Some(paths.join("\n").as_str()));
        write_item(&state, files_item.id).unwrap();
        assert_eq!(platform::read_clipboard_files().unwrap(), paths);

        if let Some(text) = original_text {
            let _ = Clipboard::new().and_then(|mut clipboard| clipboard.set_text(text));
        }
    }
}
