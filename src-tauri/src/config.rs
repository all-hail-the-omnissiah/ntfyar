//! Persistent settings: servers, topic subscriptions and general preferences.
//! Secrets (passwords / tokens) are NOT stored here; they live in the OS keychain
//! (Windows Credential Manager) — see `secrets`.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum AuthKind {
    None,
    Basic,
    Token,
}

impl Default for AuthKind {
    fn default() -> Self {
        AuthKind::None
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Server {
    pub id: String,
    pub name: String,
    /// Base URL, e.g. https://ntfy.example.com
    pub url: String,
    pub auth: AuthKind,
    /// Only used for Basic auth.
    pub username: String,
}

impl Default for Server {
    fn default() -> Self {
        Server {
            id: String::new(),
            name: String::new(),
            url: String::new(),
            auth: AuthKind::None,
            username: String::new(),
        }
    }
}

impl Server {
    pub fn base(&self) -> String {
        self.url.trim().trim_end_matches('/').to_string()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Subscription {
    pub id: String,
    pub server_id: String,
    pub topic: String,
    /// Optional friendly name shown in the UI instead of the topic.
    pub display_name: String,
    /// Accent colour for this topic in the UI (CSS colour).
    pub color: String,
    /// Never show a desktop notification (messages still land in history).
    pub muted: bool,
    /// Only notify for messages with at least this priority (1-5).
    pub min_priority: u8,
    /// If non-empty, only notify when the title/message/tags contain one of these words.
    pub keywords: Vec<String>,
    /// Play a sound for this topic's notifications.
    pub sound: bool,
    /// Present in the ntfy account at the last sync (see sync.rs).
    pub synced: bool,
}

impl Default for Subscription {
    fn default() -> Self {
        Subscription {
            id: String::new(),
            server_id: String::new(),
            topic: String::new(),
            display_name: String::new(),
            color: "#7aa2f7".into(),
            muted: false,
            min_priority: 1,
            keywords: vec![],
            sound: true,
            synced: false,
        }
    }
}

impl Subscription {
    pub fn label(&self) -> String {
        if self.display_name.trim().is_empty() {
            self.topic.clone()
        } else {
            self.display_name.clone()
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct QuietHours {
    pub enabled: bool,
    /// "HH:MM" local time
    pub start: String,
    pub end: String,
    /// Priority 5 (urgent) messages still notify during quiet hours.
    pub allow_urgent: bool,
}

impl Default for QuietHours {
    fn default() -> Self {
        QuietHours {
            enabled: false,
            start: "22:00".into(),
            end: "07:00".into(),
            allow_urgent: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct General {
    /// "acrylic" | "mica" | "none"
    pub backdrop: String,
    /// UI accent colour.
    pub accent: String,
    /// Background tint opacity 0..100 (how see-through the window is).
    pub opacity: u8,
    pub autostart: bool,
    /// Global shortcut that opens the quick-publish window, e.g. "CommandOrControl+Alt+N".
    pub publish_hotkey: String,
    pub quiet_hours: QuietHours,
    /// Show notifications for messages fetched while the app was offline.
    pub notify_missed: bool,
    /// Maximum number of messages kept in history.
    pub history_limit: usize,
    /// Topic used by default in quick publish (subscription id).
    pub default_publish_sub: String,
}

impl Default for General {
    fn default() -> Self {
        General {
            backdrop: "acrylic".into(),
            accent: "#7aa2f7".into(),
            opacity: 80,
            autostart: true,
            publish_hotkey: "CommandOrControl+Alt+N".into(),
            quiet_hours: QuietHours::default(),
            notify_missed: true,
            history_limit: 2000,
            default_publish_sub: String::new(),
        }
    }
}

/// A topic unsubscribed here that still has to be removed from the ntfy account.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(default)]
pub struct PendingDelete {
    pub server_id: String,
    pub topic: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Config {
    pub servers: Vec<Server>,
    pub subscriptions: Vec<Subscription>,
    pub general: General,
    pub pending_deletes: Vec<PendingDelete>,
}

impl Config {
    pub fn load(path: &PathBuf) -> Config {
        match std::fs::read_to_string(path) {
            Ok(s) => serde_json::from_str(&s).unwrap_or_else(|e| {
                log::error!("config parse error, using defaults: {e}");
                // keep a copy of the broken file so nothing is lost
                let _ = std::fs::copy(path, path.with_extension("json.broken"));
                Config::default()
            }),
            Err(_) => Config::default(),
        }
    }

    pub fn save(&self, path: &PathBuf) -> Result<(), String> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let tmp = path.with_extension("json.tmp");
        let data = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(&tmp, data).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, path).map_err(|e| e.to_string())
    }

    pub fn server(&self, id: &str) -> Option<&Server> {
        self.servers.iter().find(|s| s.id == id)
    }

    pub fn sub(&self, id: &str) -> Option<&Subscription> {
        self.subscriptions.iter().find(|s| s.id == id)
    }

    pub fn sub_for(&self, server_id: &str, topic: &str) -> Option<&Subscription> {
        self.subscriptions
            .iter()
            .find(|s| s.server_id == server_id && s.topic == topic)
    }
}

/// Credentials in the OS keychain (Windows Credential Manager), keyed by
/// server id. If the keychain is unavailable (some Linux setups), they fall back to a
/// `secrets.json` file in the app data folder that only the current user can read.
pub mod secrets {
    use std::collections::HashMap;
    use std::path::PathBuf;
    use std::sync::OnceLock;

    const SERVICE: &str = "ntfyar";
    static FALLBACK: OnceLock<PathBuf> = OnceLock::new();

    pub fn init(data_dir: &std::path::Path) {
        let _ = FALLBACK.set(data_dir.join("secrets.json"));
    }

    fn entry(server_id: &str) -> Result<keyring::Entry, String> {
        keyring::Entry::new(SERVICE, server_id).map_err(|e| e.to_string())
    }

    fn read_file() -> HashMap<String, String> {
        FALLBACK
            .get()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    fn write_file(map: &HashMap<String, String>) -> Result<(), String> {
        let p = FALLBACK.get().ok_or("no data dir")?;
        if map.is_empty() {
            let _ = std::fs::remove_file(p);
            return Ok(());
        }
        std::fs::write(p, serde_json::to_string(map).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o600));
        }
        Ok(())
    }

    pub fn get(server_id: &str) -> Option<String> {
        if let Some(s) = entry(server_id).ok().and_then(|e| e.get_password().ok()) {
            return Some(s);
        }
        read_file().get(server_id).cloned()
    }

    pub fn set(server_id: &str, secret: &str) -> Result<(), String> {
        let keychain = entry(server_id).and_then(|e| {
            e.set_password(secret).map_err(|e| e.to_string())?;
            // make sure it can actually be read back
            e.get_password().map(|_| ()).map_err(|e| e.to_string())
        });
        let mut map = read_file();
        match keychain {
            Ok(()) => {
                if map.remove(server_id).is_some() {
                    write_file(&map)?;
                }
                Ok(())
            }
            Err(e) => {
                log::warn!("keychain unavailable ({e}), storing secret in app data");
                map.insert(server_id.to_string(), secret.to_string());
                write_file(&map)
            }
        }
    }

    /// Removes every saved secret from the fallback file (used by the factory reset).
    pub fn wipe_file() {
        if let Some(p) = FALLBACK.get() {
            let _ = std::fs::remove_file(p);
        }
    }

    pub fn delete(server_id: &str) {
        if let Ok(e) = entry(server_id) {
            let _ = e.delete_credential();
        }
        let mut map = read_file();
        if map.remove(server_id).is_some() {
            let _ = write_file(&map);
        }
    }
}
