//! Decides whether a message should pop up, and shows native notifications.
//! Windows uses real WinRT toasts (action buttons, hero images, sticky urgent toasts);
//! other platforms fall back to the Tauri notification plugin.

use crate::config::{Config, Subscription};
use crate::history::StoredMessage;
use chrono::{Local, NaiveTime};
use std::path::PathBuf;
use tauri::AppHandle;

#[allow(dead_code)]
pub struct Notif {
    pub msg_id: Option<String>,
    pub title: String,
    pub body: String,
    pub priority: u8,
    pub sound: bool,
    pub image: Option<PathBuf>,
    /// Labels of action buttons (index = action index in the message)
    pub buttons: Vec<String>,
}

fn parse_hm(s: &str) -> Option<NaiveTime> {
    NaiveTime::parse_from_str(s.trim(), "%H:%M").ok()
}

pub fn in_quiet_hours(cfg: &Config) -> bool {
    let q = &cfg.general.quiet_hours;
    if !q.enabled {
        return false;
    }
    let (Some(start), Some(end)) = (parse_hm(&q.start), parse_hm(&q.end)) else {
        return false;
    };
    let now = Local::now().time();
    if start <= end {
        now >= start && now < end
    } else {
        // wraps past midnight, e.g. 22:00 → 07:00
        now >= start || now < end
    }
}

pub fn should_notify(cfg: &Config, sub: &Subscription, m: &StoredMessage, paused: bool) -> bool {
    if paused || sub.muted || m.priority < sub.min_priority {
        return false;
    }
    if !sub.keywords.is_empty() {
        let hay = format!("{} {} {}", m.title, m.message, m.tags.join(" ")).to_lowercase();
        let hit = sub
            .keywords
            .iter()
            .map(|k| k.trim().to_lowercase())
            .filter(|k| !k.is_empty())
            .any(|k| hay.contains(&k));
        if !hit {
            return false;
        }
    }
    if in_quiet_hours(cfg) {
        return m.priority >= 5 && cfg.general.quiet_hours.allow_urgent;
    }
    true
}

pub fn from_message(sub: &Subscription, m: &StoredMessage, image: Option<PathBuf>) -> Notif {
    let label = sub.label();
    let mut title = if m.title.is_empty() {
        label.clone()
    } else {
        m.title.clone()
    };
    if !m.emojis.is_empty() {
        title = format!("{} {}", m.emojis, title);
    }
    let mut body = m.message.clone();
    if body.is_empty() {
        if let Some(a) = &m.attachment {
            body = format!("📎 {}", a.name);
        }
    }
    if !m.title.is_empty() {
        // keep the topic visible when the message has its own title
        body = format!("{body}\n— {label}");
    }
    Notif {
        msg_id: Some(m.id.clone()),
        title,
        body,
        priority: m.priority,
        sound: sub.sound,
        image,
        buttons: m.actions.iter().take(3).map(|a| a.label.clone()).collect(),
    }
}

#[cfg(windows)]
pub fn show(app: &AppHandle, n: Notif) {
    use tauri::Manager;
    use tauri_winrt_notification::{Duration, LoopableSound, Scenario, Sound, Toast};

    // Installed builds register the bundle identifier as the AppUserModelID.
    // Unpackaged dev builds borrow PowerShell's so toasts still appear.
    let app_id = if cfg!(debug_assertions) {
        Toast::POWERSHELL_APP_ID.to_string()
    } else {
        app.config().identifier.clone()
    };

    let mut t = Toast::new(&app_id).title(&n.title).text1(&n.body);
    if let Some(img) = &n.image {
        t = t.hero(img, "");
    }
    t = match n.priority {
        5 => t.scenario(Scenario::Reminder).duration(Duration::Long),
        4 => t.duration(Duration::Long),
        _ => t.duration(Duration::Short),
    };
    t = if !n.sound || n.priority <= 2 {
        t.sound(None)
    } else if n.priority == 5 {
        t.sound(Some(Sound::Single(LoopableSound::Alarm)))
    } else {
        t.sound(Some(Sound::Default))
    };
    for (i, label) in n.buttons.iter().enumerate() {
        t = t.add_button(label, &format!("a:{i}"));
    }
    if n.priority == 5 && n.buttons.is_empty() {
        // Reminder-style toasts only stay on screen if they have a button.
        t = t.add_button("Dismiss", "dismiss");
    }
    let handle = app.clone();
    let msg_id = n.msg_id.clone();
    t = t.on_activated(move |arg| {
        crate::on_notification_activated(&handle, msg_id.clone(), arg);
        Ok(())
    });
    if let Err(e) = t.show() {
        log::error!("toast failed: {e:?}");
    }
}

#[cfg(not(windows))]
pub fn show(app: &AppHandle, n: Notif) {
    use tauri_plugin_notification::NotificationExt;
    let mut b = app.notification().builder().title(&n.title).body(&n.body);
    if n.sound && n.priority > 2 {
        b = b.sound("default");
    }
    if let Err(e) = b.show() {
        log::error!("notification failed: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(prio: u8, text: &str) -> StoredMessage {
        StoredMessage { priority: prio, message: text.into(), ..Default::default() }
    }

    #[test]
    fn rules() {
        let cfg = Config::default();
        let mut sub = Subscription::default();
        assert!(should_notify(&cfg, &sub, &msg(3, "hi"), false));
        assert!(!should_notify(&cfg, &sub, &msg(3, "hi"), true), "paused");
        sub.min_priority = 4;
        assert!(!should_notify(&cfg, &sub, &msg(3, "hi"), false));
        assert!(should_notify(&cfg, &sub, &msg(5, "hi"), false));
        sub.min_priority = 1;
        sub.keywords = vec!["FAIL".into()];
        assert!(!should_notify(&cfg, &sub, &msg(3, "all good"), false));
        assert!(should_notify(&cfg, &sub, &msg(3, "backup failed"), false));
        sub.keywords.clear();
        sub.muted = true;
        assert!(!should_notify(&cfg, &sub, &msg(5, "x"), false));
    }

    #[test]
    fn quiet_hours_all_day() {
        let mut cfg = Config::default();
        cfg.general.quiet_hours.enabled = true;
        cfg.general.quiet_hours.start = "00:00".into();
        cfg.general.quiet_hours.end = "23:59".into();
        let sub = Subscription::default();
        // 23:59-00:00 is the only gap; ignore that minute
        if chrono::Local::now().format("%H:%M").to_string() != "23:59" {
            assert!(!should_notify(&cfg, &sub, &msg(4, "x"), false));
            assert!(should_notify(&cfg, &sub, &msg(5, "x"), false));
            cfg.general.quiet_hours.allow_urgent = false;
            assert!(!should_notify(&cfg, &sub, &msg(5, "x"), false));
        }
    }

    #[test]
    fn emoji_tags() {
        let (e, rest) = crate::emoji::split_tags(&["warning".into(), "truenas".into(), "+1".into()]);
        assert_eq!(e, "⚠️👍");
        assert_eq!(rest, vec!["truenas".to_string()]);
    }
}
