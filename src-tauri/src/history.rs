//! Message history kept in memory and mirrored to a JSON file.

use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Action {
    /// "view" | "http" | "broadcast" (broadcast is Android-only and ignored here)
    pub action: String,
    pub label: String,
    pub url: String,
    pub method: Option<String>,
    pub headers: Option<std::collections::HashMap<String, String>>,
    pub body: Option<String>,
    pub clear: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Attachment {
    pub name: String,
    #[serde(rename = "type")]
    pub mime: Option<String>,
    pub size: Option<u64>,
    pub expires: Option<i64>,
    pub url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct StoredMessage {
    pub id: String,
    pub server_id: String,
    pub sub_id: String,
    pub topic: String,
    /// Unix seconds
    pub time: i64,
    pub title: String,
    pub message: String,
    pub priority: u8,
    /// Emoji rendered from tags
    pub emojis: String,
    /// Tags that are not emojis
    pub tags: Vec<String>,
    pub click: String,
    pub icon: String,
    pub markdown: bool,
    pub actions: Vec<Action>,
    pub attachment: Option<Attachment>,
    pub read: bool,
    /// Hidden from the feed but kept (so it can be restored, and isn't re-imported).
    pub dismissed: bool,
}

pub struct History {
    path: PathBuf,
    pub items: Vec<StoredMessage>,
    ids: HashSet<String>,
}

impl History {
    pub fn load(path: PathBuf) -> History {
        let items: Vec<StoredMessage> = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        let ids = items.iter().map(|m| m.id.clone()).collect();
        History { path, items, ids }
    }

    pub fn contains(&self, id: &str) -> bool {
        self.ids.contains(id)
    }

    /// Inserts a message, keeping the list sorted newest-first and capped at `limit`.
    pub fn insert(&mut self, msg: StoredMessage, limit: usize) -> bool {
        if self.ids.contains(&msg.id) {
            return false;
        }
        self.ids.insert(msg.id.clone());
        let pos = self
            .items
            .iter()
            .position(|m| m.time <= msg.time)
            .unwrap_or(self.items.len());
        self.items.insert(pos, msg);
        let limit = limit.max(50);
        while self.items.len() > limit {
            if let Some(old) = self.items.pop() {
                self.ids.remove(&old.id);
            }
        }
        true
    }

    /// Newest message time for a server — used for `since=` catch-up after reconnects.
    pub fn newest_time(&self, server_id: &str) -> Option<i64> {
        self.items
            .iter()
            .filter(|m| m.server_id == server_id)
            .map(|m| m.time)
            .max()
    }

    pub fn unread(&self) -> usize {
        self.items.iter().filter(|m| !m.read).count()
    }

    pub fn mark_read(&mut self, ids: Option<&[String]>, sub_id: Option<&str>) {
        for m in self.items.iter_mut() {
            let hit = match (ids, sub_id) {
                (Some(ids), _) => ids.contains(&m.id),
                (None, Some(s)) => m.sub_id == s,
                (None, None) => true,
            };
            if hit {
                m.read = true;
            }
        }
    }

    /// Hides/restores messages. `read` optionally sets the read state at the same time
    /// (dismissing marks as read; undo puts back what it was).
    pub fn set_dismissed(&mut self, ids: &[String], dismissed: bool, read: Option<bool>) {
        for m in self.items.iter_mut().filter(|m| ids.contains(&m.id)) {
            m.dismissed = dismissed;
            if let Some(r) = read {
                m.read = r;
            }
        }
    }

    pub fn delete(&mut self, ids: Option<&[String]>, sub_id: Option<&str>) {
        self.items.retain(|m| match (ids, sub_id) {
            (Some(ids), _) => !ids.contains(&m.id),
            (None, Some(s)) => m.sub_id != s,
            (None, None) => false,
        });
        self.ids = self.items.iter().map(|m| m.id.clone()).collect();
    }

    pub fn clear(&mut self) {
        self.items.clear();
        self.ids.clear();
    }

    pub fn delete_server(&mut self, server_id: &str) {
        self.items.retain(|m| m.server_id != server_id);
        self.ids = self.items.iter().map(|m| m.id.clone()).collect();
    }

    pub fn get(&self, id: &str) -> Option<&StoredMessage> {
        self.items.iter().find(|m| m.id == id)
    }

    pub fn save(&self) {
        if let Some(dir) = self.path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let tmp = self.path.with_extension("json.tmp");
        if let Ok(data) = serde_json::to_string(&self.items) {
            if std::fs::write(&tmp, data).is_ok() {
                let _ = std::fs::rename(&tmp, &self.path);
            }
        }
    }
}
