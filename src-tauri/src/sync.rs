//! Keeps the topic list in sync with the user's ntfy account (the same mechanism the ntfy
//! web app and Android app use). Only for servers where the app logs in.
//!
//! - `GET  /v1/account`               → `subscriptions: [{base_url, topic, display_name}]`, `sync_topic`
//! - `POST /v1/account/subscription`  → add `{base_url, topic, display_name}`
//! - `DELETE /v1/account/subscription` with `X-BaseURL` / `X-Topic` headers → remove
//! - The server publishes to the user's private `sync_topic` whenever the list changes,
//!   so we listen on it and re-sync right away.
//!
//! Each local subscription has a `synced` flag: "this topic existed in the account at the last
//! sync". A synced topic that disappears from the account was removed elsewhere, so it's removed
//! here too. An unsynced local topic (added here, or before sync existed) is uploaded instead.

use crate::config::{AuthKind, PendingDelete, Server, Subscription};
use crate::{ntfy, AppState};
use serde::Deserialize;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};

#[derive(Deserialize, Default)]
#[serde(default)]
struct Account {
    subscriptions: Vec<RemoteSub>,
    sync_topic: String,
}

#[derive(Deserialize, Default, Clone)]
#[serde(default)]
struct RemoteSub {
    base_url: String,
    topic: String,
    display_name: Option<String>,
}

fn norm(url: &str) -> String {
    url.trim().trim_end_matches('/').to_lowercase()
}

fn uses_account(server: &Server) -> bool {
    server.auth != AuthKind::None
}

/// The URL the server calls itself (its `base-url`), which is what the web app stores in the
/// account. It can differ from the address this PC uses (e.g. a LAN IP vs. the domain).
async fn canonical_base(http: &reqwest::Client, server: &Server) -> String {
    let fallback = server.base();
    let Ok(resp) = http
        .get(format!("{fallback}/config.js"))
        .timeout(Duration::from_secs(10))
        .send()
        .await
    else {
        return fallback;
    };
    let body = resp.text().await.unwrap_or_default();
    // config.js contains e.g. `base_url: "https://ntfy.example.com",`
    body.split("base_url:")
        .nth(1)
        .and_then(|rest| rest.split('"').nth(1))
        .map(|s| s.trim().trim_end_matches('/').to_string())
        .filter(|s| s.starts_with("http"))
        .unwrap_or(fallback)
}

async fn fetch_account(http: &reqwest::Client, server: &Server) -> Result<Account, String> {
    let auth = ntfy::auth_header(server, None);
    let mut req = http
        .get(format!("{}/v1/account", server.base()))
        .timeout(Duration::from_secs(20));
    if let Some(h) = &auth {
        req = req.header(reqwest::header::AUTHORIZATION, h);
    }
    let resp = req.send().await.map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("account request failed: HTTP {}", resp.status().as_u16()));
    }
    resp.json::<Account>().await.map_err(|e| e.to_string())
}

pub async fn remote_add(
    http: &reqwest::Client,
    server: &Server,
    base: &str,
    sub: &Subscription,
) -> Result<(), String> {
    let mut body = serde_json::json!({ "base_url": base, "topic": sub.topic });
    if !sub.display_name.trim().is_empty() {
        body["display_name"] = sub.display_name.trim().into();
    }
    let mut req = http
        .post(format!("{}/v1/account/subscription", server.base()))
        .json(&body)
        .timeout(Duration::from_secs(20));
    if let Some(h) = ntfy::auth_header(server, None) {
        req = req.header(reqwest::header::AUTHORIZATION, h);
    }
    let resp = req.send().await.map_err(|e| e.to_string())?;
    // 409 = already in the account, which is what we want anyway
    if resp.status().is_success() || resp.status().as_u16() == 409 {
        Ok(())
    } else {
        Err(format!("HTTP {}", resp.status().as_u16()))
    }
}

pub async fn remote_delete(
    http: &reqwest::Client,
    server: &Server,
    base: &str,
    topic: &str,
) -> Result<(), String> {
    let mut req = http
        .delete(format!("{}/v1/account/subscription", server.base()))
        .header("X-BaseURL", base)
        .header("X-Topic", topic)
        .timeout(Duration::from_secs(20));
    if let Some(h) = ntfy::auth_header(server, None) {
        req = req.header(reqwest::header::AUTHORIZATION, h);
    }
    let resp = req.send().await.map_err(|e| e.to_string())?;
    if resp.status().is_success() {
        Ok(())
    } else {
        Err(format!("HTTP {}", resp.status().as_u16()))
    }
}

pub fn spawn_sync(app: &AppHandle, server_id: String) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        if let Err(e) = sync_server(&app, &server_id).await {
            log::warn!("sync {server_id}: {e}");
        }
    });
}

pub fn spawn_sync_all(app: &AppHandle) {
    let ids: Vec<String> = {
        let st = app.state::<AppState>();
        let cfg = st.config.lock().unwrap();
        cfg.servers
            .iter()
            .filter(|s| uses_account(s))
            .map(|s| s.id.clone())
            .collect()
    };
    for id in ids {
        spawn_sync(app, id);
    }
}

/// Periodic safety net in case a sync event was missed.
pub fn start_periodic(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(300)).await;
            spawn_sync_all(&app);
        }
    });
}

pub async fn sync_server(app: &AppHandle, server_id: &str) -> Result<(), String> {
    let st = app.state::<AppState>();
    let _guard = st.sync_lock.lock().await;

    let Some(server) = st.config.lock().unwrap().server(server_id).cloned() else {
        return Ok(());
    };
    if !uses_account(&server) {
        return Ok(());
    }
    let http = st.http.clone();
    let base = canonical_base(&http, &server).await;

    // First send removals made in the app, otherwise the topics would be imported again.
    let pending: Vec<PendingDelete> = st
        .config
        .lock()
        .unwrap()
        .pending_deletes
        .iter()
        .filter(|p| p.server_id == server_id)
        .cloned()
        .collect();
    for p in &pending {
        remote_delete(&http, &server, &base, &p.topic).await?;
        st.config.lock().unwrap().pending_deletes.retain(|x| x != p);
    }
    if !pending.is_empty() {
        st.save_config()?;
    }

    let account = fetch_account(&http, &server).await?;
    let ours = [norm(&base), norm(&server.base())];
    let remote: Vec<RemoteSub> = account
        .subscriptions
        .into_iter()
        .filter(|r| ours.contains(&norm(&r.base_url)) && !r.topic.is_empty())
        .collect();

    // live sync events
    let topic_changed = {
        let mut map = st.sync_topics.lock().unwrap();
        let old = map.get(server_id).cloned().unwrap_or_default();
        if account.sync_topic.is_empty() {
            map.remove(server_id);
        } else {
            map.insert(server_id.to_string(), account.sync_topic.clone());
        }
        old != account.sync_topic
    };

    let mut added: Vec<String> = Vec::new();
    let mut removed: Vec<String> = Vec::new();
    let mut to_push: Vec<Subscription> = Vec::new();
    {
        let mut cfg = st.config.lock().unwrap();
        let used: Vec<String> = cfg.subscriptions.iter().map(|s| s.color.clone()).collect();
        let mut palette = crate::TOPIC_COLORS.iter().filter(|c| !used.iter().any(|u| u == *c));

        for r in &remote {
            if cfg.sub_for(server_id, &r.topic).is_none() {
                let sub = Subscription {
                    id: uuid::Uuid::new_v4().to_string(),
                    server_id: server_id.to_string(),
                    topic: r.topic.clone(),
                    display_name: r.display_name.clone().unwrap_or_default(),
                    color: palette.next().unwrap_or(&crate::TOPIC_COLORS[0]).to_string(),
                    synced: true,
                    ..Default::default()
                };
                added.push(sub.id.clone());
                cfg.subscriptions.push(sub);
            }
        }
        for sub in cfg.subscriptions.iter_mut().filter(|s| s.server_id == server_id) {
            if added.contains(&sub.id) {
                continue;
            }
            match remote.iter().find(|r| r.topic == sub.topic) {
                Some(r) => {
                    sub.synced = true;
                    if sub.display_name.trim().is_empty() {
                        if let Some(name) = r.display_name.clone().filter(|n| !n.trim().is_empty()) {
                            sub.display_name = name;
                        }
                    }
                }
                None if sub.synced => removed.push(sub.id.clone()),
                None => to_push.push(sub.clone()),
            }
        }
        cfg.subscriptions.retain(|s| !removed.contains(&s.id));
        if removed.contains(&cfg.general.default_publish_sub) {
            cfg.general.default_publish_sub.clear();
        }
    }
    st.save_config()?;

    // upload topics that only exist here
    let mut pushed = false;
    for sub in to_push {
        match remote_add(&http, &server, &base, &sub).await {
            Ok(()) => {
                let mut cfg = st.config.lock().unwrap();
                if let Some(s) = cfg.subscriptions.iter_mut().find(|s| s.id == sub.id) {
                    s.synced = true;
                    pushed = true;
                }
            }
            Err(e) => log::warn!("sync: couldn't add {} to account: {e}", sub.topic),
        }
    }
    if pushed {
        st.save_config()?;
    }

    if !removed.is_empty() {
        let mut h = st.history.lock().unwrap();
        for id in &removed {
            h.delete(None, Some(id));
        }
        h.save();
    }
    let changed = !added.is_empty() || !removed.is_empty();
    if changed || topic_changed {
        crate::restart_server_pub(app, server_id);
    }
    if changed {
        let _ = app.emit(
            "topics-synced",
            serde_json::json!({ "added": added.len(), "removed": removed.len() }),
        );
        let _ = app.emit("history-changed", ());
        crate::refresh_tray(app);
    }
    if !added.is_empty() {
        crate::spawn_backfill(app, added);
    }
    Ok(())
}
