//! Talking to ntfy servers: streaming subscriptions with reconnect + catch-up,
//! publishing, connection tests and running `http` actions.

use crate::config::{secrets, AuthKind, Server};
use crate::history::{Action, Attachment, StoredMessage};
use crate::{emoji, AppState};
use base64::Engine;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use std::time::Duration;
use tauri::{AppHandle, Manager};

/// Wire format of one line from `/<topics>/json`.
#[derive(Debug, Deserialize, Default)]
#[serde(default)]
pub struct WireMessage {
    pub id: String,
    pub time: i64,
    pub event: String,
    pub topic: String,
    pub message: Option<String>,
    pub title: Option<String>,
    pub tags: Option<Vec<String>>,
    pub priority: Option<u8>,
    pub click: Option<String>,
    pub icon: Option<String>,
    pub actions: Option<Vec<Action>>,
    pub attachment: Option<Attachment>,
    pub content_type: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(tag = "state", content = "detail", rename_all = "snake_case")]
pub enum ServerStatus {
    Idle,
    Connecting,
    Connected,
    AuthError(String),
    Error(String),
}

pub fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent(concat!("ntfyar/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(15))
        .build()
        .expect("http client")
}

/// Builds the Authorization header value for a server, if it uses auth.
pub fn auth_header(server: &Server, secret_override: Option<&str>) -> Option<String> {
    let secret = match secret_override {
        Some(s) => Some(s.to_string()),
        None => secrets::get(&server.id),
    };
    match server.auth {
        AuthKind::None => None,
        AuthKind::Token => secret.map(|t| format!("Bearer {}", t.trim())),
        AuthKind::Basic => {
            let pass = secret.unwrap_or_default();
            let raw = format!("{}:{}", server.username, pass);
            Some(format!(
                "Basic {}",
                base64::engine::general_purpose::STANDARD.encode(raw)
            ))
        }
    }
}

fn with_auth(req: reqwest::RequestBuilder, auth: &Option<String>) -> reqwest::RequestBuilder {
    match auth {
        Some(h) => req.header(reqwest::header::AUTHORIZATION, h),
        None => req,
    }
}

pub fn to_stored(server_id: &str, sub_id: &str, w: WireMessage) -> StoredMessage {
    let tags = w.tags.unwrap_or_default();
    let (emojis, rest) = emoji::split_tags(&tags);
    StoredMessage {
        id: w.id,
        server_id: server_id.into(),
        sub_id: sub_id.into(),
        topic: w.topic,
        time: w.time,
        title: w.title.unwrap_or_default(),
        message: w.message.unwrap_or_default(),
        priority: w.priority.unwrap_or(3).clamp(1, 5),
        emojis,
        tags: rest,
        click: w.click.unwrap_or_default(),
        icon: w.icon.unwrap_or_default(),
        markdown: w.content_type.as_deref() == Some("text/markdown"),
        actions: w
            .actions
            .unwrap_or_default()
            .into_iter()
            .filter(|a| a.action == "view" || a.action == "http")
            .collect(),
        attachment: w.attachment,
        read: false,
        dismissed: false,
    }
}

fn parse_line(line: &[u8]) -> Option<WireMessage> {
    let s = std::str::from_utf8(line).ok()?.trim();
    if s.is_empty() {
        return None;
    }
    serde_json::from_str(s).ok()
}

/// What the subscription loop needs to know about one server, read fresh from config each attempt.
fn snapshot(app: &AppHandle, server_id: &str) -> Option<(Server, Vec<String>)> {
    let st = app.state::<AppState>();
    let cfg = st.config.lock().unwrap();
    let server = cfg.server(server_id)?.clone();
    let mut topics: Vec<String> = cfg
        .subscriptions
        .iter()
        .filter(|s| s.server_id == server_id && !s.topic.is_empty())
        .map(|s| s.topic.clone())
        .collect();
    // the account's private sync topic tells us when the topic list changed elsewhere
    if let Some(t) = st.sync_topics.lock().unwrap().get(server_id) {
        topics.push(t.clone());
    }
    topics.sort();
    topics.dedup();
    Some((server, topics))
}

/// Long-running task for one server. Exits when the server has no topics or was removed.
/// It is aborted and restarted by `AppState::restart_server` when config changes.
pub async fn run_server(app: AppHandle, server_id: String) {
    let http = app.state::<AppState>().http.clone();
    let mut backoff = 1u64;

    loop {
        let Some((server, topics)) = snapshot(&app, &server_id) else {
            return;
        };
        if topics.is_empty() {
            crate::set_status(&app, &server_id, ServerStatus::Idle);
            return;
        }
        crate::set_status(&app, &server_id, ServerStatus::Connecting);

        let auth = auth_header(&server, None);
        let topic_path = topics.join(",");
        let base = server.base();

        // 1) Catch up on anything we missed while offline / asleep.
        let newest = app
            .state::<AppState>()
            .history
            .lock()
            .unwrap()
            .newest_time(&server_id);
        let mut stream_since = chrono::Utc::now().timestamp().to_string();
        if let Some(since) = newest {
            let url = format!("{base}/{topic_path}/json?poll=1&since={since}");
            match with_auth(http.get(&url), &auth)
                .timeout(Duration::from_secs(30))
                .send()
                .await
            {
                Ok(resp) if resp.status().is_success() => {
                    if let Ok(body) = resp.bytes().await {
                        let msgs: Vec<WireMessage> = body
                            .split(|b| *b == b'\n')
                            .filter_map(parse_line)
                            .filter(|m| m.event == "message")
                            .collect();
                        if let Some(last) = msgs.iter().map(|m| m.time).max() {
                            stream_since = last.to_string();
                        }
                        crate::handle_messages(&app, &server_id, msgs, true);
                    }
                }
                Ok(resp) if resp.status().as_u16() == 401 || resp.status().as_u16() == 403 => {
                    crate::set_status(
                        &app,
                        &server_id,
                        ServerStatus::AuthError(format!(
                            "Server rejected the login ({}). Check username/password or token.",
                            resp.status().as_u16()
                        )),
                    );
                    tokio::time::sleep(Duration::from_secs(60)).await;
                    continue;
                }
                Ok(resp) => {
                    let msg = format!("HTTP {}", resp.status());
                    crate::set_status(&app, &server_id, ServerStatus::Error(msg));
                    sleep_backoff(&mut backoff).await;
                    continue;
                }
                Err(e) => {
                    crate::set_status(&app, &server_id, ServerStatus::Error(short_err(&e)));
                    sleep_backoff(&mut backoff).await;
                    continue;
                }
            }
        }

        // 2) Open the live stream.
        let url = format!("{base}/{topic_path}/json?since={stream_since}");
        let resp = match with_auth(http.get(&url), &auth).send().await {
            Ok(r) => r,
            Err(e) => {
                crate::set_status(&app, &server_id, ServerStatus::Error(short_err(&e)));
                sleep_backoff(&mut backoff).await;
                continue;
            }
        };
        let code = resp.status().as_u16();
        if code == 401 || code == 403 {
            crate::set_status(
                &app,
                &server_id,
                ServerStatus::AuthError(format!(
                    "Server rejected the login ({code}). Check username/password or token."
                )),
            );
            tokio::time::sleep(Duration::from_secs(60)).await;
            continue;
        }
        if !resp.status().is_success() {
            crate::set_status(&app, &server_id, ServerStatus::Error(format!("HTTP {code}")));
            sleep_backoff(&mut backoff).await;
            continue;
        }

        crate::set_status(&app, &server_id, ServerStatus::Connected);
        backoff = 1;
        // (re)connected: catch up on topic changes made elsewhere and send our pending ones
        crate::sync::spawn_sync(&app, server_id.clone());

        let mut stream = resp.bytes_stream();
        let mut buf: Vec<u8> = Vec::new();
        let err = loop {
            // ntfy sends a keepalive every ~45s; if nothing arrives for 2 min the
            // connection is dead (laptop slept, Wi-Fi changed, ...) → reconnect.
            match tokio::time::timeout(Duration::from_secs(120), stream.next()).await {
                Err(_) => break "Connection timed out".to_string(),
                Ok(None) => break "Connection closed by server".to_string(),
                Ok(Some(Err(e))) => break short_err(&e),
                Ok(Some(Ok(chunk))) => {
                    buf.extend_from_slice(&chunk);
                    while let Some(pos) = buf.iter().position(|b| *b == b'\n') {
                        let line: Vec<u8> = buf.drain(..=pos).collect();
                        if let Some(m) = parse_line(&line) {
                            if m.event == "message" {
                                crate::handle_messages(&app, &server_id, vec![m], false);
                            }
                        }
                    }
                }
            }
        };
        log::warn!("stream for {server_id} ended: {err}");
        crate::set_status(&app, &server_id, ServerStatus::Error(err));
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

async fn sleep_backoff(backoff: &mut u64) {
    tokio::time::sleep(Duration::from_secs(*backoff)).await;
    *backoff = (*backoff * 2).min(60);
}

fn short_err(e: &reqwest::Error) -> String {
    if e.is_connect() {
        "Can't reach server".into()
    } else if e.is_timeout() {
        "Timed out".into()
    } else {
        let s = e.to_string();
        s.chars().take(140).collect()
    }
}

/// Fetches every message the server still has cached for a topic (`since=all`).
/// How far back this goes depends on the server's `cache-duration` (ntfy default: 12h).
pub async fn fetch_cached(
    http: &reqwest::Client,
    server: &Server,
    topic: &str,
) -> Result<Vec<WireMessage>, String> {
    let auth = auth_header(server, None);
    let url = format!("{}/{topic}/json?poll=1&since=all", server.base());
    let resp = with_auth(http.get(&url), &auth)
        .timeout(Duration::from_secs(60))
        .send()
        .await
        .map_err(|e| short_err(&e))?;
    let code = resp.status().as_u16();
    if !resp.status().is_success() {
        let text = resp.text().await.unwrap_or_default();
        return Err(server_error(code, &text));
    }
    let body = resp.bytes().await.map_err(|e| short_err(&e))?;
    Ok(body
        .split(|b| *b == b'\n')
        .filter_map(parse_line)
        .filter(|m| m.event == "message")
        .collect())
}

#[derive(Debug, Deserialize, Serialize, Default)]
#[serde(default)]
pub struct PublishRequest {
    pub sub_id: String,
    pub message: String,
    pub title: String,
    pub priority: u8,
    pub tags: Vec<String>,
    pub click: String,
    /// e.g. "30m", "tomorrow, 10am"
    pub delay: String,
    pub markdown: bool,
}

pub async fn publish(
    http: &reqwest::Client,
    server: &Server,
    topic: &str,
    req: &PublishRequest,
) -> Result<(), String> {
    let mut body = serde_json::json!({ "topic": topic, "message": req.message });
    let o = body.as_object_mut().unwrap();
    if !req.title.trim().is_empty() {
        o.insert("title".into(), req.title.trim().into());
    }
    if (1..=5).contains(&req.priority) && req.priority != 3 {
        o.insert("priority".into(), req.priority.into());
    }
    if !req.tags.is_empty() {
        o.insert("tags".into(), req.tags.clone().into());
    }
    if !req.click.trim().is_empty() {
        o.insert("click".into(), req.click.trim().into());
    }
    if !req.delay.trim().is_empty() {
        o.insert("delay".into(), req.delay.trim().into());
    }
    if req.markdown {
        o.insert("markdown".into(), true.into());
    }
    let auth = auth_header(server, None);
    let resp = with_auth(http.post(server.base()), &auth)
        .json(&body)
        .timeout(Duration::from_secs(20))
        .send()
        .await
        .map_err(|e| short_err(&e))?;
    if resp.status().is_success() {
        Ok(())
    } else {
        let code = resp.status();
        let text = resp.text().await.unwrap_or_default();
        Err(server_error(code.as_u16(), &text))
    }
}

fn server_error(code: u16, body: &str) -> String {
    let detail = serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| v.get("error").and_then(|e| e.as_str()).map(String::from))
        .unwrap_or_else(|| body.chars().take(120).collect());
    match code {
        401 | 403 => format!("Not allowed ({code}): {detail}"),
        _ => format!("HTTP {code}: {detail}"),
    }
}

/// Checks the server is reachable and the credentials (if any) are accepted.
pub async fn test(
    http: &reqwest::Client,
    server: &Server,
    secret: Option<&str>,
    topic: Option<&str>,
) -> Result<String, String> {
    let base = server.base();
    if !(base.starts_with("http://") || base.starts_with("https://")) {
        return Err("URL must start with http:// or https://".into());
    }
    let health = http
        .get(format!("{base}/v1/health"))
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .map_err(|e| short_err(&e))?;
    if !health.status().is_success() {
        return Err(format!(
            "Server answered HTTP {} — is this really an ntfy server?",
            health.status().as_u16()
        ));
    }
    if server.auth == AuthKind::None {
        // Many servers allow anonymous health checks but lock the topics themselves.
        if let Some(topic) = topic {
            let r = http
                .get(format!("{base}/{topic}/json?poll=1&since=1m"))
                .timeout(Duration::from_secs(10))
                .send()
                .await
                .map_err(|e| short_err(&e))?;
            if matches!(r.status().as_u16(), 401 | 403) {
                return Err(format!(
                    "Server is reachable, but reading “{topic}” needs a login. Choose username & password or access token."
                ));
            }
        }
        return Ok("Connected. Server is reachable.".into());
    }
    let auth = auth_header(server, secret);
    let acc = with_auth(http.get(format!("{base}/v1/account")), &auth)
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .map_err(|e| short_err(&e))?;
    match acc.status().as_u16() {
        200 => {
            let v: serde_json::Value = acc.json().await.unwrap_or_default();
            let user = v
                .get("username")
                .and_then(|u| u.as_str())
                .unwrap_or("?")
                .to_string();
            Ok(format!("Connected and logged in as “{user}”."))
        }
        401 | 403 => Err("Server is reachable, but the login was rejected.".into()),
        c => Err(format!("Server is reachable, but /v1/account returned HTTP {c}.")),
    }
}

/// Runs an ntfy `http` action button.
pub async fn run_http_action(http: &reqwest::Client, a: &Action) -> Result<(), String> {
    let method = a
        .method
        .clone()
        .unwrap_or_else(|| "POST".into())
        .to_uppercase();
    let method = reqwest::Method::from_bytes(method.as_bytes()).map_err(|e| e.to_string())?;
    let mut req = http
        .request(method, &a.url)
        .timeout(Duration::from_secs(30));
    if let Some(h) = &a.headers {
        for (k, v) in h {
            req = req.header(k, v);
        }
    }
    if let Some(b) = &a.body {
        req = req.body(b.clone());
    }
    let resp = req.send().await.map_err(|e| short_err(&e))?;
    if resp.status().is_success() {
        Ok(())
    } else {
        Err(format!("Action returned HTTP {}", resp.status().as_u16()))
    }
}

/// Downloads an image attachment into the cache so it can be shown in the toast.
pub async fn cache_image(
    http: &reqwest::Client,
    server: &Server,
    att: &Attachment,
    dir: std::path::PathBuf,
) -> Option<std::path::PathBuf> {
    let mime = att.mime.clone().unwrap_or_default();
    if !mime.starts_with("image/") || att.size.unwrap_or(0) > 8_000_000 {
        return None;
    }
    let ext = mime.trim_start_matches("image/").replace("jpeg", "jpg");
    let ext: String = ext.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
    std::fs::create_dir_all(&dir).ok()?;
    let path = dir.join(format!("{}.{}", uuid::Uuid::new_v4(), ext));
    // Attachments on the same server may need the same credentials.
    let auth = if att.url.starts_with(&server.base()) {
        auth_header(server, None)
    } else {
        None
    };
    let resp = with_auth(http.get(&att.url), &auth)
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let bytes = resp.bytes().await.ok()?;
    std::fs::write(&path, &bytes).ok()?;
    Some(path)
}
