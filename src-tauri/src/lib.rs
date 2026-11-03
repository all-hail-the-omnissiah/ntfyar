mod config;
mod emoji;
mod history;
mod notify;
mod ntfy;
mod sync;
#[cfg(windows)]
mod swca;

use config::{secrets, AuthKind, Config, General, Server, Subscription};
use history::{History, StoredMessage};
use ntfy::{PublishRequest, ServerStatus, WireMessage};
use serde::Serialize;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder, Wry};
use tauri_plugin_autostart::ManagerExt as _;
use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};
use tauri_plugin_opener::OpenerExt;

const ICON_NORMAL: &[u8] = include_bytes!("../icons/tray.png");
const ICON_UNREAD: &[u8] = include_bytes!("../icons/tray-unread.png");
const ICON_ERROR: &[u8] = include_bytes!("../icons/tray-error.png");

pub struct AppState {
    pub config: Mutex<Config>,
    config_path: PathBuf,
    pub history: Mutex<History>,
    statuses: Mutex<HashMap<String, ServerStatus>>,
    tasks: Mutex<HashMap<String, tauri::async_runtime::JoinHandle<()>>>,
    paused: AtomicBool,
    pub http: reqwest::Client,
    cache_dir: PathBuf,
    tray_status: Mutex<Option<MenuItem<Wry>>>,
    tray_pause: Mutex<Option<CheckMenuItem<Wry>>>,
    /// One account sync at a time per app
    pub sync_lock: tokio::sync::Mutex<()>,
    /// server id → the account's private sync topic
    pub sync_topics: Mutex<HashMap<String, String>>,
}

/// Same order as COLORS in ui/common.js; used for topics imported from the account.
pub(crate) const TOPIC_COLORS: [&str; 18] = [
    "#f7768e", "#ff5c8a", "#ff7a59", "#ff9e64", "#e0af68", "#f2d06b", "#c3e88d", "#9ece6a",
    "#4fd6be", "#2ac3de", "#7dcfff", "#7aa2f7", "#6f7cff", "#9d7cd8", "#bb9af7", "#e48bd9",
    "#c0caf5", "#a9b1d6",
];

pub(crate) fn restart_server_pub(app: &AppHandle, server_id: &str) {
    restart_server(app, server_id);
}

pub(crate) fn refresh_tray(app: &AppHandle) {
    update_tray(app);
}

pub(crate) fn spawn_backfill(app: &AppHandle, sub_ids: Vec<String>) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        if let Err(e) = backfill(&app, sub_ids).await {
            log::warn!("backfill: {e}");
        }
    });
}

impl AppState {
    fn save_config(&self) -> Result<(), String> {
        self.config.lock().unwrap().save(&self.config_path)
    }
}

// ───────────────────────── connections ─────────────────────────

fn restart_server(app: &AppHandle, server_id: &str) {
    let st = app.state::<AppState>();
    if let Some(h) = st.tasks.lock().unwrap().remove(server_id) {
        h.abort();
    }
    let exists = st.config.lock().unwrap().server(server_id).is_some();
    if !exists {
        st.statuses.lock().unwrap().remove(server_id);
        emit_status(app);
        return;
    }
    let handle = tauri::async_runtime::spawn(ntfy::run_server(app.clone(), server_id.to_string()));
    st.tasks.lock().unwrap().insert(server_id.to_string(), handle);
}

fn restart_all(app: &AppHandle) {
    let ids: Vec<String> = {
        let st = app.state::<AppState>();
        let cfg = st.config.lock().unwrap();
        cfg.servers.iter().map(|s| s.id.clone()).collect()
    };
    for id in ids {
        restart_server(app, &id);
    }
}

pub(crate) fn set_status(app: &AppHandle, server_id: &str, status: ServerStatus) {
    let st = app.state::<AppState>();
    {
        let mut map = st.statuses.lock().unwrap();
        if map.get(server_id) == Some(&status) {
            return;
        }
        map.insert(server_id.to_string(), status);
    }
    emit_status(app);
}

fn emit_status(app: &AppHandle) {
    let statuses = app.state::<AppState>().statuses.lock().unwrap().clone();
    let _ = app.emit("status", &statuses);
    update_tray(app);
}

/// Called by the subscription loop for every batch of incoming messages.
pub(crate) fn handle_messages(app: &AppHandle, server_id: &str, msgs: Vec<WireMessage>, catchup: bool) {
    let st = app.state::<AppState>();
    // messages on the account's sync topic mean "topic list changed" — not for display
    let sync_topic = st.sync_topics.lock().unwrap().get(server_id).cloned();
    let (sync_events, msgs): (Vec<_>, Vec<_>) = msgs
        .into_iter()
        .partition(|m| sync_topic.as_deref() == Some(m.topic.as_str()));
    if !sync_events.is_empty() && !catchup {
        sync::spawn_sync(app, server_id.to_string());
    }
    let cfg = st.config.lock().unwrap().clone();
    let paused = st.paused.load(Ordering::Relaxed);
    let mut fresh: Vec<(Subscription, StoredMessage)> = Vec::new();
    {
        let mut hist = st.history.lock().unwrap();
        for w in msgs {
            let Some(sub) = cfg.sub_for(server_id, &w.topic) else {
                continue;
            };
            if w.id.is_empty() || hist.contains(&w.id) {
                continue;
            }
            let stored = ntfy::to_stored(server_id, &sub.id, w);
            if hist.insert(stored.clone(), cfg.general.history_limit) {
                fresh.push((sub.clone(), stored));
            }
        }
        if !fresh.is_empty() {
            hist.save();
        }
    }
    if fresh.is_empty() {
        return;
    }
    for (_, m) in &fresh {
        let _ = app.emit("message", m);
    }
    update_tray(app);

    let to_notify: Vec<_> = fresh
        .into_iter()
        .filter(|(sub, m)| notify::should_notify(&cfg, sub, m, paused))
        .collect();
    if to_notify.is_empty() {
        return;
    }
    if catchup {
        if !cfg.general.notify_missed {
            return;
        }
        if to_notify.len() > 3 {
            let mut topics: Vec<String> = to_notify.iter().map(|(s, _)| s.label()).collect();
            topics.sort();
            topics.dedup();
            notify::show(
                app,
                notify::Notif {
                    msg_id: None,
                    title: format!("📬 {} messages while you were away", to_notify.len()),
                    body: topics.join(" · "),
                    priority: to_notify.iter().map(|(_, m)| m.priority).max().unwrap_or(3).min(4),
                    sound: true,
                    image: None,
                    buttons: vec![],
                },
            );
            return;
        }
    }
    let server = cfg.server(server_id).cloned();
    for (sub, m) in to_notify {
        let app = app.clone();
        let server = server.clone();
        let http = st.http.clone();
        let cache = st.cache_dir.clone();
        tauri::async_runtime::spawn(async move {
            let image = match (&m.attachment, &server) {
                (Some(att), Some(srv)) => ntfy::cache_image(&http, srv, att, cache).await,
                _ => None,
            };
            notify::show(&app, notify::from_message(&sub, &m, image));
        });
    }
}

/// Toast clicked (`arg == None`) or one of its buttons pressed (`Some("a:<index>")`).
#[allow(dead_code)]
pub(crate) fn on_notification_activated(app: &AppHandle, msg_id: Option<String>, arg: Option<String>) {
    let Some(id) = msg_id else {
        show_main(app);
        return;
    };
    match arg.as_deref() {
        Some("dismiss") => {}
        Some(a) if a.starts_with("a:") => {
            if let Ok(i) = a[2..].parse::<usize>() {
                let app = app.clone();
                tauri::async_runtime::spawn(async move {
                    if let Err(e) = do_action(&app, &id, i).await {
                        notify_error(&app, &e);
                    }
                });
            }
        }
        _ => {
            let click = {
                let st = app.state::<AppState>();
                let h = st.history.lock().unwrap();
                h.get(&id).map(|m| m.click.clone()).unwrap_or_default()
            };
            mark_read_internal(app, Some(vec![id.clone()]), None);
            if click.starts_with("http://") || click.starts_with("https://") {
                let _ = app.opener().open_url(click, None::<&str>);
            } else {
                show_main(app);
                let _ = app.emit("focus-message", id);
            }
        }
    }
}

fn notify_error(app: &AppHandle, e: &str) {
    notify::show(
        app,
        notify::Notif {
            msg_id: None,
            title: "⚠️ Action failed".into(),
            body: e.to_string(),
            priority: 3,
            sound: false,
            image: None,
            buttons: vec![],
        },
    );
}

async fn do_action(app: &AppHandle, msg_id: &str, idx: usize) -> Result<String, String> {
    let (action, http) = {
        let st = app.state::<AppState>();
        let h = st.history.lock().unwrap();
        let m = h.get(msg_id).ok_or("Message not found")?;
        let a = m.actions.get(idx).cloned().ok_or("Action not found")?;
        (a, st.http.clone())
    };
    match action.action.as_str() {
        "view" => {
            if !(action.url.starts_with("http://") || action.url.starts_with("https://")) {
                return Err("Only http(s) links can be opened".into());
            }
            app.opener()
                .open_url(action.url.clone(), None::<&str>)
                .map_err(|e| e.to_string())?;
            Ok("Opened".into())
        }
        "http" => {
            ntfy::run_http_action(&http, &action).await?;
            Ok(format!("“{}” done", action.label))
        }
        other => Err(format!("Action type “{other}” isn't supported on desktop")),
    }
}

// ───────────────────────── windows & tray ─────────────────────────

/// Window background. `kind`:
/// - "none"    – no blur, plain see-through (opacity comes from the CSS tint)
/// - "blur"    – light blur of what's behind the window
/// - "acrylic" – strong blur with acrylic texture (like Tabby)
/// - "mica"    – Windows 11 Mica (tinted by the wallpaper, not see-through)
fn apply_backdrop(win: &WebviewWindow, general: &General) {
    let kind = general.backdrop.as_str();
    // No native shadow: on Windows it keeps a hidden frame that makes the window opaque.
    let _ = win.set_shadow(false);
    #[cfg(windows)]
    {
        use tauri::window::{Effect, EffectsBuilder};
        let w = win.clone();
        let kind = kind.to_string();
        // Window effects must be changed on the UI thread.
        let _ = win.run_on_main_thread(move || {
            // fully transparent window background (otherwise Windows can fill it opaque)
            let _ = w.set_background_color(Some(tauri::window::Color(0, 0, 0, 0)));
            let Ok(hwnd) = w.hwnd() else { return };
            let hwnd = hwnd.0 as isize;
            swca::window_frame(hwnd);
            if kind == "mica" {
                swca::dark_mode(hwnd);
                swca::set(hwnd, swca::DISABLED, 0);
                let cfg = EffectsBuilder::new().effect(Effect::MicaDark).build();
                if let Err(e) = w.set_effects(cfg) {
                    log::warn!("mica failed: {e}");
                }
            } else {
                // Not `set_effects(None)`: that sets the backdrop to NONE, which turns
                // the window opaque grey on Windows 11.
                // Glass is always blurred (older settings "none"/"blur" map here too)
                swca::apply_glass(hwnd, "acrylic");
            }
        });
    }
    #[cfg(not(windows))]
    let _ = (win, kind);
}

fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
    }
}

/// Builds the quick-publish window (hidden). Done once at startup so that opening it later
/// never has to create a webview — creating one from a synchronous command deadlocks on Windows.
fn create_publish_window(app: &AppHandle) -> Option<WebviewWindow> {
    let builder = WebviewWindowBuilder::new(app, "publish", WebviewUrl::App("publish.html".into()))
        .title("Quick publish")
        .inner_size(560.0, 420.0)
        .resizable(false)
        .decorations(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .center()
        .visible(false)
        .shadow(false)
        .transparent(true);
    match builder.build() {
        Ok(w) => {
            let general = app.state::<AppState>().config.lock().unwrap().general.clone();
            apply_backdrop(&w, &general);
            Some(w)
        }
        Err(e) => {
            log::error!("publish window: {e}");
            None
        }
    }
}

fn show_publish(app: &AppHandle) {
    let win = match app.get_webview_window("publish") {
        Some(w) => Some(w),
        None => create_publish_window(app),
    };
    if let Some(w) = win {
        let _ = w.center();
        let _ = w.show();
        let _ = w.set_focus();
        let _ = w.emit("publish-open", ());
    }
}

fn update_tray(app: &AppHandle) {
    let st = app.state::<AppState>();
    let unread = st.history.lock().unwrap().unread();
    let paused = st.paused.load(Ordering::Relaxed);
    let (problem, total, connected) = {
        let map = st.statuses.lock().unwrap();
        let problem = map
            .values()
            .any(|s| matches!(s, ServerStatus::Error(_) | ServerStatus::AuthError(_)));
        let connected = map.values().filter(|s| **s == ServerStatus::Connected).count();
        (problem, map.len(), connected)
    };
    let status_line = if total == 0 {
        "No servers set up".to_string()
    } else if problem {
        format!("⚠ {connected}/{total} servers connected")
    } else if connected == total {
        if total == 1 { "● Connected".into() } else { format!("● All {total} servers connected") }
    } else {
        "○ Connecting…".into()
    };
    let mut tooltip = format!("ntfyar — {status_line}");
    if unread > 0 {
        tooltip.push_str(&format!("\n{unread} unread"));
    }
    if paused {
        tooltip.push_str("\nNotifications paused");
    }
    if let Some(tray) = app.tray_by_id("main") {
        let bytes = if problem {
            ICON_ERROR
        } else if unread > 0 {
            ICON_UNREAD
        } else {
            ICON_NORMAL
        };
        if let Ok(img) = tauri::image::Image::from_bytes(bytes) {
            let _ = tray.set_icon(Some(img));
        }
        let _ = tray.set_tooltip(Some(tooltip));
    }
    if let Some(item) = st.tray_status.lock().unwrap().as_ref() {
        let _ = item.set_text(status_line);
    }
    if let Some(item) = st.tray_pause.lock().unwrap().as_ref() {
        let _ = item.set_checked(paused);
    }
    let _ = app.emit("unread", unread);
}

fn build_tray(app: &AppHandle) -> tauri::Result<()> {
    let status = MenuItem::with_id(app, "status", "Starting…", false, None::<&str>)?;
    let open = MenuItem::with_id(app, "open", "Open ntfyar", true, None::<&str>)?;
    let publish = MenuItem::with_id(app, "publish", "Quick publish…", true, None::<&str>)?;
    let readall = MenuItem::with_id(app, "readall", "Mark all as read", true, None::<&str>)?;
    let pause = CheckMenuItem::with_id(app, "pause", "Pause notifications", true, false, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let sep1 = PredefinedMenuItem::separator(app)?;
    let sep2 = PredefinedMenuItem::separator(app)?;
    let sep3 = PredefinedMenuItem::separator(app)?;
    let menu = Menu::with_items(
        app,
        &[&status, &sep1, &open, &publish, &readall, &sep2, &pause, &sep3, &quit],
    )?;
    {
        let st = app.state::<AppState>();
        *st.tray_status.lock().unwrap() = Some(status);
        *st.tray_pause.lock().unwrap() = Some(pause);
    }
    TrayIconBuilder::with_id("main")
        .icon(tauri::image::Image::from_bytes(ICON_NORMAL)?)
        .tooltip("ntfyar")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, ev| match ev.id().as_ref() {
            "open" => show_main(app),
            "publish" => show_publish(app),
            "readall" => mark_read_internal(app, None, None),
            "pause" => {
                let st = app.state::<AppState>();
                let now = !st.paused.load(Ordering::Relaxed);
                st.paused.store(now, Ordering::Relaxed);
                let _ = app.emit("paused", now);
                update_tray(app);
            }
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, ev| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = ev
            {
                let app = tray.app_handle();
                match app.get_webview_window("main") {
                    Some(w) if w.is_visible().unwrap_or(false) && w.is_focused().unwrap_or(false) => {
                        let _ = w.hide();
                    }
                    _ => show_main(app),
                }
            }
        })
        .build(app)?;
    Ok(())
}

fn register_hotkey(app: &AppHandle, combo: &str) -> Result<(), String> {
    let gs = app.global_shortcut();
    let _ = gs.unregister_all();
    if combo.trim().is_empty() {
        return Ok(());
    }
    gs.on_shortcut(combo.trim(), |app, _sc, ev| {
        if ev.state == ShortcutState::Pressed {
            show_publish(app);
        }
    })
    .map_err(|e| format!("Couldn't register shortcut “{combo}”: {e}"))
}

fn apply_autostart(app: &AppHandle, on: bool) {
    let al = app.autolaunch();
    let enabled = al.is_enabled().unwrap_or(false);
    let r = if on && !enabled {
        al.enable()
    } else if !on && enabled {
        al.disable()
    } else {
        Ok(())
    };
    if let Err(e) = r {
        log::warn!("autostart: {e}");
    }
}

fn mark_read_internal(app: &AppHandle, ids: Option<Vec<String>>, sub_id: Option<String>) {
    let st = app.state::<AppState>();
    {
        let mut h = st.history.lock().unwrap();
        h.mark_read(ids.as_deref(), sub_id.as_deref());
        h.save();
    }
    let _ = app.emit("history-changed", ());
    update_tray(app);
}

// ───────────────────────── commands ─────────────────────────

#[derive(Serialize)]
struct StateDto {
    config: Config,
    has_secret: HashMap<String, bool>,
    statuses: HashMap<String, ServerStatus>,
    paused: bool,
    platform: &'static str,
    version: &'static str,
}

#[tauri::command]
fn get_state(state: tauri::State<AppState>) -> StateDto {
    let config = state.config.lock().unwrap().clone();
    let has_secret = config
        .servers
        .iter()
        .map(|s| (s.id.clone(), s.auth != AuthKind::None && secrets::get(&s.id).is_some()))
        .collect();
    StateDto {
        config,
        has_secret,
        statuses: state.statuses.lock().unwrap().clone(),
        paused: state.paused.load(Ordering::Relaxed),
        platform: std::env::consts::OS,
        version: env!("CARGO_PKG_VERSION"),
    }
}

#[tauri::command]
fn get_history(state: tauri::State<AppState>) -> Vec<StoredMessage> {
    state.history.lock().unwrap().items.clone()
}

#[tauri::command]
fn mark_read(app: AppHandle, ids: Option<Vec<String>>, sub_id: Option<String>) {
    mark_read_internal(&app, ids, sub_id);
}

/// Deletes everything the app stores on this PC and goes back to first-start defaults.
/// The ntfy server and the account's topic list are not touched.
#[tauri::command]
async fn factory_reset(app: AppHandle) -> Result<(), String> {
    let st = app.state::<AppState>();
    // wait for a running account sync, and keep new ones out until we're done
    let _sync = st.sync_lock.lock().await;

    // stop all connections
    for (_, task) in st.tasks.lock().unwrap().drain() {
        task.abort();
    }
    st.statuses.lock().unwrap().clear();
    st.sync_topics.lock().unwrap().clear();
    st.paused.store(false, Ordering::Relaxed);

    // settings → defaults (written out, so the next start sees a clean config)
    let old = std::mem::take(&mut *st.config.lock().unwrap());
    for s in &old.servers {
        secrets::delete(&s.id);
    }
    secrets::wipe_file();
    st.save_config()?;

    {
        let mut h = st.history.lock().unwrap();
        h.clear();
        h.save();
    }
    let _ = std::fs::remove_dir_all(&st.cache_dir);

    let general = General::default();
    if let Err(e) = register_hotkey(&app, &general.publish_hotkey) {
        log::warn!("{e}");
    }
    apply_autostart(&app, general.autostart);
    for w in app.webview_windows().values() {
        apply_backdrop(w, &general);
    }
    update_tray(&app);
    let _ = app.emit("factory-reset", ());
    Ok(())
}

#[tauri::command]
fn set_dismissed(app: AppHandle, ids: Vec<String>, dismissed: bool, read: Option<bool>) {
    let st = app.state::<AppState>();
    {
        let mut h = st.history.lock().unwrap();
        h.set_dismissed(&ids, dismissed, read);
        h.save();
    }
    let _ = app.emit("history-changed", ());
    update_tray(&app);
}

#[tauri::command]
fn delete_messages(app: AppHandle, ids: Option<Vec<String>>, sub_id: Option<String>) {
    let st = app.state::<AppState>();
    {
        let mut h = st.history.lock().unwrap();
        h.delete(ids.as_deref(), sub_id.as_deref());
        h.save();
    }
    let _ = app.emit("history-changed", ());
    update_tray(&app);
}

fn normalize_url(url: &str) -> Result<String, String> {
    let mut u = url.trim().trim_end_matches('/').to_string();
    if u.is_empty() {
        return Err("Server URL is required".into());
    }
    if !u.contains("://") {
        u = format!("https://{u}");
    }
    if !(u.starts_with("http://") || u.starts_with("https://")) {
        return Err("URL must start with http:// or https://".into());
    }
    Ok(u)
}

#[tauri::command]
fn save_server(app: AppHandle, mut server: Server, secret: Option<String>) -> Result<Server, String> {
    server.url = normalize_url(&server.url)?;
    if server.name.trim().is_empty() {
        server.name = server
            .url
            .split("://")
            .nth(1)
            .unwrap_or(&server.url)
            .to_string();
    }
    if server.auth == AuthKind::Basic && server.username.trim().is_empty() {
        return Err("Username is required for password login".into());
    }
    if server.id.is_empty() {
        server.id = uuid::Uuid::new_v4().to_string();
    }
    match (&server.auth, secret.as_deref().map(str::trim)) {
        (AuthKind::None, _) => secrets::delete(&server.id),
        (_, Some(s)) if !s.is_empty() => secrets::set(&server.id, s)?,
        _ => {
            if secrets::get(&server.id).is_none() {
                return Err(match server.auth {
                    AuthKind::Token => "Access token is required".into(),
                    _ => "Password is required".into(),
                });
            }
        }
    }
    let st = app.state::<AppState>();
    {
        let mut cfg = st.config.lock().unwrap();
        match cfg.servers.iter_mut().find(|s| s.id == server.id) {
            Some(existing) => *existing = server.clone(),
            None => cfg.servers.push(server.clone()),
        }
    }
    st.save_config()?;
    restart_server(&app, &server.id);
    sync::spawn_sync(&app, server.id.clone());
    Ok(server)
}

#[tauri::command]
fn delete_server(app: AppHandle, id: String) -> Result<(), String> {
    let st = app.state::<AppState>();
    {
        let mut cfg = st.config.lock().unwrap();
        cfg.servers.retain(|s| s.id != id);
        cfg.subscriptions.retain(|s| s.server_id != id);
        cfg.pending_deletes.retain(|p| p.server_id != id);
    }
    st.save_config()?;
    secrets::delete(&id);
    {
        let mut h = st.history.lock().unwrap();
        h.delete_server(&id);
        h.save();
    }
    restart_server(&app, &id);
    let _ = app.emit("history-changed", ());
    update_tray(&app);
    Ok(())
}

#[tauri::command]
async fn test_server(
    app: AppHandle,
    mut server: Server,
    secret: Option<String>,
) -> Result<String, String> {
    server.url = normalize_url(&server.url)?;
    let (http, topic) = {
        let st = app.state::<AppState>();
        let cfg = st.config.lock().unwrap();
        let topic = cfg
            .subscriptions
            .iter()
            .find(|s| s.server_id == server.id)
            .map(|s| s.topic.clone());
        (st.http.clone(), topic)
    };
    let secret = secret.filter(|s| !s.trim().is_empty());
    ntfy::test(&http, &server, secret.as_deref(), topic.as_deref()).await
}

fn valid_topic(t: &str) -> bool {
    !t.is_empty()
        && t.len() <= 64
        && t.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

#[tauri::command]
fn save_subscription(app: AppHandle, mut sub: Subscription) -> Result<Subscription, String> {
    sub.topic = sub.topic.trim().to_string();
    if !valid_topic(&sub.topic) {
        return Err("Topic names may only contain letters, numbers, - and _ (max 64)".into());
    }
    sub.min_priority = sub.min_priority.clamp(1, 5);
    sub.keywords = sub
        .keywords
        .iter()
        .map(|k| k.trim().to_string())
        .filter(|k| !k.is_empty())
        .collect();
    let st = app.state::<AppState>();
    let old_server = {
        let mut cfg = st.config.lock().unwrap();
        if cfg.server(&sub.server_id).is_none() {
            return Err("Pick a server first".into());
        }
        if cfg
            .subscriptions
            .iter()
            .any(|s| s.id != sub.id && s.server_id == sub.server_id && s.topic == sub.topic)
        {
            return Err("You're already subscribed to that topic on this server".into());
        }
        if sub.id.is_empty() {
            sub.id = uuid::Uuid::new_v4().to_string();
        }
        let old = cfg.subscriptions.iter().find(|s| s.id == sub.id).cloned();
        match &old {
            Some(o) if o.topic != sub.topic || o.server_id != sub.server_id => {
                if o.synced {
                    cfg.pending_deletes.push(config::PendingDelete {
                        server_id: o.server_id.clone(),
                        topic: o.topic.clone(),
                    });
                }
                sub.synced = false;
            }
            Some(o) => sub.synced = o.synced,
            None => sub.synced = false,
        }
        match cfg.subscriptions.iter_mut().find(|s| s.id == sub.id) {
            Some(existing) => *existing = sub.clone(),
            None => cfg.subscriptions.push(sub.clone()),
        }
        old
    };
    st.save_config()?;
    // only reconnect when the topic set changed
    let topic_changed = old_server
        .as_ref()
        .map(|o| o.topic != sub.topic || o.server_id != sub.server_id)
        .unwrap_or(true);
    if topic_changed {
        if let Some(o) = &old_server {
            if o.server_id != sub.server_id {
                restart_server(&app, &o.server_id);
            }
        }
        restart_server(&app, &sub.server_id);
        // keep the ntfy account's topic list in step
        if let Some(o) = &old_server {
            if o.server_id != sub.server_id {
                sync::spawn_sync(&app, o.server_id.clone());
            }
        }
        sync::spawn_sync(&app, sub.server_id.clone());
        // pull in whatever the server still has cached for this topic
        let app2 = app.clone();
        let id = sub.id.clone();
        tauri::async_runtime::spawn(async move {
            if let Err(e) = backfill(&app2, vec![id]).await {
                log::warn!("backfill: {e}");
            }
        });
    }
    Ok(sub)
}

/// Imports messages the server still has cached for the given subscriptions.
/// They go straight into the history as read, without notifications.
async fn backfill(app: &AppHandle, sub_ids: Vec<String>) -> Result<usize, String> {
    let (jobs, http, limit) = {
        let st = app.state::<AppState>();
        let cfg = st.config.lock().unwrap();
        let jobs: Vec<(Subscription, Server)> = sub_ids
            .iter()
            .filter_map(|id| {
                let sub = cfg.sub(id)?.clone();
                let server = cfg.server(&sub.server_id)?.clone();
                Some((sub, server))
            })
            .collect();
        (jobs, st.http.clone(), cfg.general.history_limit)
    };
    let mut added = 0usize;
    let mut last_err = None;
    for (sub, server) in jobs {
        match ntfy::fetch_cached(&http, &server, &sub.topic).await {
            Ok(msgs) => {
                let st = app.state::<AppState>();
                let mut h = st.history.lock().unwrap();
                let before = added;
                for w in msgs {
                    if w.id.is_empty() || h.contains(&w.id) {
                        continue;
                    }
                    let mut m = ntfy::to_stored(&server.id, &sub.id, w);
                    m.read = true;
                    if h.insert(m, limit) {
                        added += 1;
                    }
                }
                if added > before {
                    h.save();
                }
            }
            Err(e) => last_err = Some(format!("{}: {e}", sub.label())),
        }
    }
    if added > 0 {
        let _ = app.emit("history-changed", ());
        update_tray(app);
    }
    match (added, last_err) {
        (0, Some(e)) => Err(e),
        _ => Ok(added),
    }
}

/// "Load older messages" button. `sub_id = None` means all topics.
#[tauri::command]
async fn fetch_older(app: AppHandle, sub_id: Option<String>) -> Result<usize, String> {
    let ids = match sub_id {
        Some(id) => vec![id],
        None => {
            let st = app.state::<AppState>();
            let cfg = st.config.lock().unwrap();
            cfg.subscriptions.iter().map(|s| s.id.clone()).collect()
        }
    };
    backfill(&app, ids).await
}

#[tauri::command]
fn delete_subscription(app: AppHandle, id: String, purge: bool) -> Result<(), String> {
    let st = app.state::<AppState>();
    let server_id = {
        let mut cfg = st.config.lock().unwrap();
        let sid = cfg.sub(&id).map(|s| s.server_id.clone());
        if let Some(s) = cfg.sub(&id).cloned() {
            if s.synced {
                cfg.pending_deletes.push(config::PendingDelete {
                    server_id: s.server_id.clone(),
                    topic: s.topic.clone(),
                });
            }
        }
        cfg.subscriptions.retain(|s| s.id != id);
        if cfg.general.default_publish_sub == id {
            cfg.general.default_publish_sub.clear();
        }
        sid
    };
    st.save_config()?;
    if purge {
        let mut h = st.history.lock().unwrap();
        h.delete(None, Some(&id));
        h.save();
    }
    if let Some(sid) = server_id {
        restart_server(&app, &sid);
        sync::spawn_sync(&app, sid);
    }
    let _ = app.emit("history-changed", ());
    update_tray(&app);
    Ok(())
}

#[tauri::command]
async fn save_general(app: AppHandle, general: General) -> Result<(), String> {
    let st = app.state::<AppState>();
    let old = st.config.lock().unwrap().general.clone();
    if general.publish_hotkey != old.publish_hotkey {
        if let Err(e) = register_hotkey(&app, &general.publish_hotkey) {
            let _ = register_hotkey(&app, &old.publish_hotkey);
            return Err(e);
        }
    }
    st.config.lock().unwrap().general = general.clone();
    st.save_config()?;
    if general.backdrop != old.backdrop {
        for w in app.webview_windows().values() {
            apply_backdrop(w, &general);
        }
    }
    if general.autostart != old.autostart {
        apply_autostart(&app, general.autostart);
    }
    let _ = app.emit("general-changed", &general);
    Ok(())
}

#[tauri::command]
fn set_paused(app: AppHandle, paused: bool) {
    app.state::<AppState>().paused.store(paused, Ordering::Relaxed);
    let _ = app.emit("paused", paused);
    update_tray(&app);
}

#[tauri::command]
async fn publish(app: AppHandle, req: PublishRequest) -> Result<(), String> {
    let (server, topic, http) = {
        let st = app.state::<AppState>();
        let cfg = st.config.lock().unwrap();
        let sub = cfg.sub(&req.sub_id).ok_or("Pick a topic")?;
        let server = cfg.server(&sub.server_id).ok_or("Server not found")?.clone();
        (server, sub.topic.clone(), st.http.clone())
    };
    if req.message.trim().is_empty() {
        return Err("Message is empty".into());
    }
    ntfy::publish(&http, &server, &topic, &req).await
}

#[tauri::command]
async fn run_action(app: AppHandle, msg_id: String, index: usize) -> Result<String, String> {
    do_action(&app, &msg_id, index).await
}

#[tauri::command]
fn open_url(app: AppHandle, url: String) -> Result<(), String> {
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err("Only http(s) links can be opened".into());
    }
    app.opener().open_url(url, None::<&str>).map_err(|e| e.to_string())
}

// async on purpose: synchronous commands run on the main thread on Windows,
// and touching windows from there can deadlock the webview.
#[tauri::command]
async fn open_publish(app: AppHandle) {
    show_publish(&app);
}

#[tauri::command]
async fn open_main(app: AppHandle, focus: Option<String>) {
    show_main(&app);
    if let Some(id) = focus {
        let _ = app.emit("focus-message", id);
    }
}

#[tauri::command]
fn quit(app: AppHandle) {
    app.exit(0);
}

// ───────────────────────── entry ─────────────────────────

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    #[allow(unused_mut)]
    let mut builder = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| show_main(app)))
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec!["--minimized"]),
        ));
    #[cfg(not(windows))]
    {
        builder = builder.plugin(tauri_plugin_notification::init());
    }

    builder
        .setup(|app| {
            let handle = app.handle().clone();
            let data_dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&data_dir)?;
            secrets::init(&data_dir);
            let config_path = data_dir.join("config.json");
            let config = Config::load(&config_path);
            let history = History::load(data_dir.join("history.json"));
            let cache_dir = app
                .path()
                .app_cache_dir()
                .unwrap_or_else(|_| data_dir.join("cache"))
                .join("images");
            let _ = std::fs::remove_dir_all(&cache_dir); // old toast images
            let general = config.general.clone();

            app.manage(AppState {
                config: Mutex::new(config),
                config_path,
                history: Mutex::new(history),
                statuses: Mutex::new(HashMap::new()),
                tasks: Mutex::new(HashMap::new()),
                paused: AtomicBool::new(false),
                http: ntfy::client(),
                cache_dir,
                tray_status: Mutex::new(None),
                tray_pause: Mutex::new(None),
                sync_lock: tokio::sync::Mutex::new(()),
                sync_topics: Mutex::new(HashMap::new()),
            });

            build_tray(&handle)?;
            if let Err(e) = register_hotkey(&handle, &general.publish_hotkey) {
                log::warn!("{e}");
            }
            apply_autostart(&handle, general.autostart);

            if let Some(main) = app.get_webview_window("main") {
                apply_backdrop(&main, &general);
                let started_hidden = std::env::args().any(|a| a == "--minimized");
                if !started_hidden {
                    let _ = main.show();
                    let _ = main.set_focus();
                }
            }

            create_publish_window(&handle);
            restart_all(&handle);
            update_tray(&handle);
            sync::spawn_sync_all(&handle);
            sync::start_periodic(&handle);
            Ok(())
        })
        .on_window_event(|window, event| {
            // Closing a window only hides it — the app lives in the tray.
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .invoke_handler(tauri::generate_handler![
            get_state,
            get_history,
            mark_read,
            delete_messages,
            set_dismissed,
            factory_reset,
            save_server,
            delete_server,
            test_server,
            save_subscription,
            delete_subscription,
            save_general,
            set_paused,
            publish,
            run_action,
            fetch_older,
            open_url,
            open_publish,
            open_main,
            quit
        ])
        .run(tauri::generate_context!())
        .expect("error while running ntfyar");
}
