// Main window: message history, topics, settings.

const S = {
  cfg: null,
  hasSecret: {},
  statuses: {},
  paused: false,
  platform: "",
  version: "",
  history: [],
  view: { kind: "all", subId: null }, // kind: all | sub | settings
  tab: "servers",                     // settings tab
  search: "",
  prio: 0,                            // 0 = all, else minimum priority
  unreadOnly: false,
  showDismissed: false,               // "Dismissed" filter: show only hidden messages
  editServer: null,                   // server object being edited (or {} for new)
  editSub: null,
};

const ICONS = {
  inbox: '<svg class="ico" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linejoin="round"><path d="M22 12h-6l-2 3h-4l-2-3H2"/><path d="M5.5 5h13L22 12v6a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2v-6z"/></svg>',
  bellOff: '<svg class="muted-ico" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M13.7 21a2 2 0 0 1-3.4 0M18.6 13A17.9 17.9 0 0 1 18 8M6.3 6.3A6 6 0 0 0 6 8c0 7-3 9-3 9h14M18 8a6 6 0 0 0-9.3-5M2 2l20 20"/></svg>',
  check: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.4"><path d="M20 6 9 17l-5-5"/></svg>',
  copy: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><rect x="9" y="9" width="13" height="13" rx="2"/><path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"/></svg>',
  trash: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M3 6h18M8 6V4h8v2M19 6l-1 14H6L5 6"/></svg>',
  checkAll: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2"><path d="M18 6 7 17l-5-5M22 10l-7.5 7.5L13 16"/></svg>',
  pause: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M13.7 21a2 2 0 0 1-3.4 0M18 8A6 6 0 0 0 6 8c0 7-3 9-3 9h18s-3-2-3-9"/></svg>',
  paused: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M13.7 21a2 2 0 0 1-3.4 0M18.6 13A17.9 17.9 0 0 1 18 8M6.3 6.3A6 6 0 0 0 6 8c0 7-3 9-3 9h14M18 8a6 6 0 0 0-9.3-5M2 2l20 20"/></svg>',
  download: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><path d="M12 3v12M7 10l5 5 5-5M5 21h14"/></svg>',
  x: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round"><path d="M18 6 6 18M6 6l12 12"/></svg>',
  restore: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M3 7v6h6"/><path d="M21 17a9 9 0 0 0-15-6.7L3 13"/></svg>',
  sweep: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M3 6h18M8 6V4h8v2M19 6l-1 14H6L5 6M10 11v6M14 11v6"/></svg>',
  file: '<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M21.4 11.1 12.2 20.3a6 6 0 0 1-8.5-8.5l9.2-9.2a4 4 0 0 1 5.7 5.7l-9.2 9.2a2 2 0 0 1-2.8-2.8l8.5-8.5"/></svg>',
};

// ───────────────────────── data ─────────────────────────

async function loadState() {
  const st = await invoke("get_state");
  S.cfg = st.config;
  S.hasSecret = st.has_secret;
  S.statuses = st.statuses;
  S.paused = st.paused;
  S.platform = st.platform;
  S.version = st.version;
  applyAppearance(S.cfg.general);
}

async function loadHistory() {
  S.history = await invoke("get_history");
}

const subById = (id) => S.cfg.subscriptions.find((s) => s.id === id);
const serverById = (id) => S.cfg.servers.find((s) => s.id === id);
const subLabel = (s) => (s?.display_name?.trim() ? s.display_name : s?.topic ?? "?");

function visibleMessages() {
  const q = S.search.trim().toLowerCase();
  return S.history.filter((m) => {
    if (S.view.kind === "sub" && m.sub_id !== S.view.subId) return false;
    if (!!m.dismissed !== S.showDismissed) return false;
    if (S.unreadOnly && m.read) return false;
    if (S.prio && m.priority < S.prio) return false;
    if (q) {
      const hay = `${m.title} ${m.message} ${m.topic} ${m.tags.join(" ")} ${subLabel(subById(m.sub_id))}`.toLowerCase();
      if (!hay.includes(q)) return false;
    }
    return true;
  });
}

// ───────────────────────── sidebar ─────────────────────────

function renderNav() {
  const unread = (subId) => S.history.filter((m) => !m.read && (!subId || m.sub_id === subId)).length;
  const total = unread();
  const isAll = S.view.kind === "all";
  let html = `
    <button class="nav-item ${isAll ? "active" : ""}" data-nav="all">
      ${ICONS.inbox}<span class="label">All messages</span>
      ${total ? `<span class="count">${total}</span>` : ""}
    </button>`;

  html += `<h4>Topics <button data-add-topic title="Subscribe to a topic" aria-label="Subscribe to a topic"><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round"><path d="M12 5v14M5 12h14"/></svg></button></h4>`;
  if (!S.cfg.subscriptions.length) {
    html += `<div class="conn" style="padding:4px 10px">No topics yet</div>`;
  }
  for (const sub of S.cfg.subscriptions) {
    const n = unread(sub.id);
    const active = S.view.kind === "sub" && S.view.subId === sub.id;
    html += `
      <button class="nav-item ${active ? "active" : ""}" data-nav="sub" data-id="${sub.id}" style="--c:${esc(sub.color)}">
        <span class="dot"></span>
        <span class="label">${esc(subLabel(sub))}</span>
        ${sub.muted ? ICONS.bellOff : ""}
        ${n ? `<span class="count">${n}</span>` : ""}
      </button>`;
  }
  $("#nav").innerHTML = html;
  $("#nav-settings").classList.toggle("active", S.view.kind === "settings");

  // connection status per server
  $("#conns").innerHTML = S.cfg.servers
    .map((srv) => {
      const st = S.statuses[srv.id] || { state: "connecting" };
      const text = {
        connected: "Connected",
        connecting: "Connecting…",
        idle: "No topics",
        error: st.detail || "Disconnected",
        auth_error: "Login rejected",
      }[st.state];
      return `<div class="conn" title="${esc(srv.url)} — ${esc(st.detail || text)}"><span class="led ${st.state}"></span><span class="label" style="overflow:hidden;text-overflow:ellipsis;white-space:nowrap">${esc(srv.name)} · ${esc(text)}</span></div>`;
    })
    .join("");
}

$("#nav").addEventListener("click", (e) => {
  if (e.target.closest("[data-add-topic]")) {
    openSettings("topics", { sub: newSub() });
    return;
  }
  const b = e.target.closest("[data-nav]");
  if (!b) return;
  if (onGeneralTab() && !okToLeave()) return;
  S.view = b.dataset.nav === "all" ? { kind: "all" } : { kind: "sub", subId: b.dataset.id };
  render();
});
$("#nav-settings").addEventListener("click", () => {
  const problem = Object.values(S.statuses).some((s) => s.state === "error" || s.state === "auth_error");
  openSettings(!S.cfg.servers.length || problem ? "servers" : "topics");
});
$("#conns").addEventListener("click", () => openSettings("servers"));
$("#open-publish").addEventListener("click", () => invoke("open_publish"));
$("#search").addEventListener("input", (e) => {
  if (onGeneralTab() && !okToLeave()) {
    e.target.value = "";
    return;
  }
  S.search = e.target.value;
  if (S.view.kind === "settings") S.view = { kind: "all" };
  renderMain();
});

// ───────────────────────── feed ─────────────────────────

function renderFeed() {
  const sub = S.view.kind === "sub" ? subById(S.view.subId) : null;
  const msgs = visibleMessages();
  const unreadHere = msgs.filter((m) => !m.read).length;
  $("#crumb").textContent = sub ? `/ ${subLabel(sub)}` : "";

  const prioChips = [
    [0, "All"], [3, "Default+"], [4, "High+"], [5, "Urgent"],
  ].map(([v, n]) => `<button class="chip ${S.prio === v ? "on" : ""}" data-prio="${v}">${n}</button>`).join("");

  let html = `
    <div class="toolbar">
      <div>
        <h1>${sub ? esc(subLabel(sub)) : "All messages"}</h1>
        <div class="sub">${sub ? `${esc(serverById(sub.server_id)?.name ?? "")}/${esc(sub.topic)}` : `${shownCount()} message${shownCount() === 1 ? "" : "s"}, ${S.history.filter((m) => !m.read && !m.dismissed).length} unread`}</div>
      </div>
      <div class="grow"></div>
      <div class="chips">${prioChips}<button class="chip ${S.unreadOnly ? "on" : ""}" data-unread>Unread</button><button class="chip ${S.showDismissed ? "on" : ""}" data-dismissed title="Messages you dismissed with ×">Dismissed</button></div>
      <button class="icon-btn" data-fetch title="Load older messages still stored on the server" aria-label="Load older messages">${ICONS.download}</button>
      <button class="icon-btn" data-readall title="Mark all as read" aria-label="Mark all as read" ${unreadHere ? "" : "disabled"}>${ICONS.checkAll}</button>
      <button class="icon-btn" data-clear title="${sub ? "Clear this topic's history" : "Clear all history"}" aria-label="Clear history">${ICONS.sweep}</button>
      <button class="icon-btn" data-pause title="${S.paused ? "Notifications are paused — click to resume" : "Pause notifications"}" aria-label="Pause notifications" style="${S.paused ? "color:var(--p4)" : ""}">${S.paused ? ICONS.paused : ICONS.pause}</button>
    </div>
    <div class="feed" id="feed">`;

  if (!msgs.length) {
    html += emptyState(sub);
  } else {
    let lastDay = "";
    for (const m of msgs) {
      const d = dayLabel(m.time);
      if (d !== lastDay) {
        html += `<div class="day">${d}</div>`;
        lastDay = d;
      }
      html += renderMsg(m);
    }
  }
  html += `</div>`;
  $("#main").innerHTML = html;
}

function emptyState(sub) {
  if (!S.cfg.servers.length) {
    return `<div class="empty">
      <div class="big">🔔</div>
      <h2>Connect your ntfy server</h2>
      <p>Add the address of your self-hosted ntfy server, then subscribe to the topics you want on this PC.</p>
      <p style="margin-top:12px"><button class="btn primary" data-go="servers">Add server</button></p>
    </div>`;
  }
  if (!S.cfg.subscriptions.length) {
    return `<div class="empty">
      <div class="big">📭</div>
      <h2>Subscribe to a topic</h2>
      <p>Use the same topic names you send to, for example <code>backups</code> or <code>proxmox</code>.</p>
      <p style="margin-top:12px"><button class="btn primary" data-go="topics">Subscribe to a topic</button></p>
    </div>`;
  }
  if (S.showDismissed && !S.search && !S.prio && !S.unreadOnly) {
    return `<div class="empty"><div class="big">🗂️</div><h2>No dismissed messages</h2><p>Messages you dismiss with × are kept here, so you can bring them back.</p></div>`;
  }
  if (S.search || S.prio || S.unreadOnly || S.showDismissed) {
    return `<div class="empty"><div class="big">🔎</div><h2>Nothing matches</h2><p>Try a different search or remove the filters.</p></div>`;
  }
  const srv = sub ? serverById(sub.server_id) : S.cfg.servers[0];
  const topic = sub ? sub.topic : S.cfg.subscriptions[0].topic;
  return `<div class="empty">
    <div class="big">✨</div>
    <h2>All quiet</h2>
    <p>New messages show up here and as Windows notifications. Send a test from any terminal:</p>
    <p style="margin-top:8px"><code>curl -d "Hello from the terminal" ${esc(srv?.url ?? "")}/${esc(topic)}</code></p>
  </div>`;
}

function renderMsg(m) {
  const sub = subById(m.sub_id);
  const color = sub?.color || "var(--accent)";
  const title = m.title || subLabel(sub);
  const body = m.markdown ? markdown(m.message) : linkify(m.message);
  let extras = m.tags.map((t) => `<span class="tag">${esc(t)}</span>`).join("");
  m.actions.forEach((a, i) => {
    extras += `<button class="act" data-action="${i}" title="${esc(a.action === "http" ? `${a.method || "POST"} ${a.url}` : a.url)}">${esc(a.label)}</button>`;
  });
  if (m.click) extras += `<a class="act" href="${esc(m.click)}" data-ext title="${esc(m.click)}">Open link</a>`;

  let attach = "";
  const att = m.attachment;
  if (att && att.url) {
    const expired = att.expires && att.expires * 1000 < Date.now();
    if ((att.type || "").startsWith("image/") && !expired) {
      attach = `<img class="attach-img" src="${esc(att.url)}" alt="${esc(att.name)}" loading="lazy" data-zoom />`;
    } else {
      extras += `<a class="act attach-file" href="${esc(att.url)}" data-ext>${ICONS.file}${esc(att.name)} <span style="color:var(--faint)">${fmtSize(att.size)}${expired ? " · expired" : ""}</span></a>`;
    }
  }
  const prioBadge = m.priority >= 4 ? `<span class="prio">${m.priority === 5 ? "URGENT" : "HIGH"}</span>` : "";
  return `
    <article class="msg p${m.priority} ${m.read ? "" : "unread"}" data-id="${esc(m.id)}" style="--c:${esc(color)}">
      <span class="edge"></span>
      <div class="head"><span class="title">${m.emojis ? `<span class="emoji">${m.emojis}</span>` : ""}${esc(title)}</span></div>
      <div class="meta">${prioBadge}${S.view.kind === "all" && m.title ? `<span class="topic">${esc(subLabel(sub))}</span>` : ""}<time title="${new Date(m.time * 1000).toLocaleString()}">${fmtTime(m.time)}</time></div>
      ${m.message ? `<div class="body ${m.markdown ? "md" : ""}">${body}</div>` : ""}
      ${attach}
      <div class="extras">${extras}</div>
      <div class="tools">
        ${m.read ? "" : `<button data-tool="read" title="Mark as read" aria-label="Mark as read">${ICONS.check}</button>`}
        <button data-tool="copy" title="Copy text" aria-label="Copy text">${ICONS.copy}</button>
        ${m.dismissed
          ? `<button data-tool="restore" title="Restore" aria-label="Restore">${ICONS.restore}</button>`
          : `<button data-tool="dismiss" title="Dismiss" aria-label="Dismiss">${ICONS.x}</button>`}
      </div>
    </article>`;
}

$("#main").addEventListener("click", async (e) => {
  const t = e.target;
  if (t.closest("[data-go]")) return openSettings(t.closest("[data-go]").dataset.go, t.closest("[data-go]").dataset.go === "topics" ? { sub: newSub() } : { server: {} });
  if (t.closest("[data-prio]")) { S.prio = +t.closest("[data-prio]").dataset.prio; return renderMain(); }
  if (t.closest("[data-unread]")) { S.unreadOnly = !S.unreadOnly; return renderMain(); }
  if (t.closest("[data-dismissed]")) { S.showDismissed = !S.showDismissed; return renderMain(); }
  if (t.closest("[data-readall]")) {
    return invoke("mark_read", { ids: null, subId: S.view.kind === "sub" ? S.view.subId : null });
  }
  if (t.closest("[data-clear]")) {
    const sub = S.view.kind === "sub" ? subById(S.view.subId) : null;
    if (!confirm(sub ? `Delete all messages in “${subLabel(sub)}”?` : "Delete the whole message history?")) return;
    return invoke("delete_messages", { ids: null, subId: sub ? sub.id : null });
  }
  if (t.closest("[data-fetch]")) {
    const btn = t.closest("[data-fetch]");
    btn.disabled = true;
    try {
      const n = await invoke("fetch_older", { subId: S.view.kind === "sub" ? S.view.subId : null });
      snack(n ? `Loaded ${n} older message${n === 1 ? "" : "s"}` : "No older messages on the server");
    } catch (err) {
      snack(String(err), true);
    }
    btn.disabled = false;
    return;
  }
  if (t.closest("[data-pause]")) return invoke("set_paused", { paused: !S.paused });
  if (t.closest("[data-zoom]")) {
    const box = document.createElement("div");
    box.className = "lightbox";
    box.innerHTML = `<img src="${esc(t.src)}" alt="">`;
    box.onclick = () => box.remove();
    document.body.appendChild(box);
    return;
  }

  const card = t.closest(".msg");
  if (!card) return;
  const id = card.dataset.id;
  const act = t.closest("[data-action]");
  if (act) {
    act.disabled = true;
    try {
      snack(await invoke("run_action", { msgId: id, index: +act.dataset.action }));
    } catch (err) {
      snack(String(err), true);
    }
    act.disabled = false;
    return;
  }
  const tool = t.closest("[data-tool]")?.dataset.tool;
  if (tool === "read") return invoke("mark_read", { ids: [id], subId: null });
  if (tool === "dismiss") return dismissMessages([id]);
  if (tool === "restore") return restoreMessages([id]);
  if (tool === "copy") {
    const m = S.history.find((x) => x.id === id);
    await navigator.clipboard.writeText([m.title, m.message].filter(Boolean).join("\n"));
    return snack("Copied");
  }
  // clicking a message marks it read (but not while selecting text)
  if (card.classList.contains("unread") && !window.getSelection().toString() && !t.closest("a")) {
    invoke("mark_read", { ids: [id], subId: null });
  }
});

// ───────────────────────── settings ─────────────────────────

function newSub() {
  const used = new Set(S.cfg.subscriptions.map((s) => s.color));
  return {
    id: "", server_id: S.cfg.servers[0]?.id ?? "", topic: "", display_name: "",
    color: COLORS.find((c) => !used.has(c)) || COLORS[0],
    muted: false, min_priority: 1, keywords: [], sound: true,
  };
}

function onGeneralTab() {
  return S.view.kind === "settings" && S.tab === "general";
}

function openSettings(tab, opts = {}) {
  if (onGeneralTab() && tab !== "general" && !okToLeave()) return;
  if (tab === "general" && !onGeneralTab()) S.draft = null; // start from the saved settings
  S.view = { kind: "settings" };
  S.tab = tab;
  S.editServer = opts.server ?? null;
  S.editSub = opts.sub ?? null;
  if (tab === "servers" && !S.cfg.servers.length && !S.editServer) S.editServer = {};
  render();
}

function renderSettings() {
  $("#crumb").textContent = "/ Settings";
  const tabs = [["servers", "Servers"], ["topics", "Topics"], ["general", "Appearance & behaviour"]];
  let html = `
    <div class="toolbar"><h1>Settings</h1></div>
    <div class="tabs">${tabs.map(([k, n]) => `<button class="tab ${S.tab === k ? "on" : ""}" data-tab="${k}">${n}</button>`).join("")}</div>
    <div class="settings" id="settings">`;
  if (S.tab === "servers") html += serversTab();
  if (S.tab === "topics") html += topicsTab();
  if (S.tab === "general") html += generalTab();
  html += `</div>`;
  $("#main").innerHTML = html;
  bindSettings();
}

function switchHTML(id, checked) {
  return `<label class="switch"><input type="checkbox" id="${id}" ${checked ? "checked" : ""}><span></span></label>`;
}

function serversTab() {
  const ed = S.editServer;
  let html = `<div class="section"><div class="list">`;
  for (const srv of S.cfg.servers) {
    const stObj = S.statuses[srv.id] ?? { state: "connecting" };
    const st = stObj.state;
    const problem = (st === "error" || st === "auth_error") ? `<div class="s" style="color:var(--p5)">${esc(stObj.detail || "")}</div>` : "";
    const login = { none: "No login", basic: `Logged in as ${srv.username}`, token: "Access token" }[srv.auth];
    html += `<div class="row clickable" data-edit-server="${srv.id}"><span class="led ${st}"></span>
      <div class="grow"><div class="t">${esc(srv.name)}</div><div class="s">${esc(srv.url)} · ${esc(login)}</div>${problem}</div></div>`;
  }
  html += `</div>`;
  if (!ed) html += `<div class="btns" style="margin-top:12px"><button class="btn" data-new-server>Add server</button></div>`;
  html += `</div>`;
  if (ed) html += serverForm(ed);
  return html;
}

function serverForm(srv) {
  const auth = srv.auth || "none";
  const saved = srv.id && S.hasSecret[srv.id];
  return `
  <div class="section">
    <h2>${srv.id ? "Edit server" : "Add server"}</h2>
    <div class="card" id="server-form">
      <div class="two">
        <div class="field"><label for="sv-url">Server URL</label><input type="url" id="sv-url" placeholder="https://ntfy.example.com" value="${esc(srv.url || "")}" spellcheck="false"></div>
        <div class="field"><label for="sv-name">Name</label><input type="text" id="sv-name" placeholder="Home server" value="${esc(srv.name || "")}"></div>
      </div>
      <div class="field">
        <span class="lbl">Login</span>
        <div class="seg" id="sv-auth">
          <button data-v="none" class="${auth === "none" ? "on" : ""}">None</button>
          <button data-v="basic" class="${auth === "basic" ? "on" : ""}">Username &amp; password</button>
          <button data-v="token" class="${auth === "token" ? "on" : ""}">Access token</button>
        </div>
        <span class="help">Only needed if your server is locked down. If you never typed a password into the ntfy app, choose None.</span>
      </div>
      <div class="two" id="sv-cred" style="${auth === "none" ? "display:none" : ""}">
        <div class="field" id="sv-user-wrap" style="${auth === "basic" ? "" : "display:none"}"><label for="sv-user">Username</label><input type="text" id="sv-user" value="${esc(srv.username || "")}" autocomplete="off" spellcheck="false"></div>
        <div class="field"><label for="sv-secret" id="sv-secret-lbl">${auth === "token" ? "Access token" : "Password"}</label>
          <input type="password" id="sv-secret" placeholder="${saved ? "Saved — leave empty to keep" : ""}" autocomplete="off">
        </div>
      </div>
      <div class="btns">
        <button class="btn primary" data-save-server>${srv.id ? "Save" : "Add server"}</button>
        <button class="btn" data-test-server>Test connection</button>
        <span class="result" id="sv-result"></span>
        <span class="grow"></span>
        ${srv.id ? `<button class="btn danger" data-delete-server>Remove</button>` : ""}
        ${S.cfg.servers.length ? `<button class="btn" data-cancel>Cancel</button>` : ""}
      </div>
    </div>
  </div>`;
}

function readServerForm() {
  const auth = $("#sv-auth .on").dataset.v;
  return {
    server: {
      id: S.editServer.id || "",
      name: $("#sv-name").value.trim(),
      url: $("#sv-url").value.trim(),
      auth,
      username: $("#sv-user").value.trim(),
    },
    secret: $("#sv-secret").value || null,
  };
}

function topicsTab() {
  if (!S.cfg.servers.length) {
    return `<div class="section"><p class="hint">Add a server first.</p><button class="btn primary" data-tab="servers">Add server</button></div>`;
  }
  let html = `<div class="section"><div class="list">`;
  for (const sub of S.cfg.subscriptions) {
    const rules = [];
    if (sub.muted) rules.push("muted");
    if (sub.min_priority > 1) rules.push(`${PRIORITIES[sub.min_priority - 1].name.toLowerCase()} priority and up`);
    if (sub.keywords.length) rules.push(`only “${sub.keywords.join("”, “")}”`);
    if (!sub.sound) rules.push("silent");
    html += `<div class="row clickable" data-edit-sub="${sub.id}" style="--c:${esc(sub.color)}"><span class="swatch"></span>
      <div class="grow"><div class="t">${esc(subLabel(sub))}</div><div class="s">${esc(serverById(sub.server_id)?.name ?? "?")}/${esc(sub.topic)}${rules.length ? " · " + esc(rules.join(", ")) : ""}</div></div></div>`;
  }
  html += `</div>`;
  if (!S.editSub) html += `<div class="btns" style="margin-top:12px"><button class="btn primary" data-new-sub>Subscribe to a topic</button></div>`;
  html += `</div>`;
  if (S.editSub) html += subForm(S.editSub);
  return html;
}

function subForm(sub) {
  return `
  <div class="section">
    <h2>${sub.id ? `Edit “${esc(subLabel(sub))}”` : "Subscribe to a topic"}</h2>
    <div class="card" id="sub-form">
      <div class="two">
        <div class="field"><label for="sb-topic">Topic</label><input type="text" id="sb-topic" value="${esc(sub.topic)}" placeholder="backups" spellcheck="false"></div>
        <div class="field"><label for="sb-server">Server</label><select id="sb-server">${S.cfg.servers.map((s) => `<option value="${s.id}" ${s.id === sub.server_id ? "selected" : ""}>${esc(s.name)}</option>`).join("")}</select></div>
      </div>
      <div class="two">
        <div class="field"><label for="sb-name">Display name <span style="color:var(--faint)">(optional)</span></label><input type="text" id="sb-name" value="${esc(sub.display_name)}" placeholder="Nightly backups"></div>
      </div>
      <div class="field"><span class="lbl">Colour</span>${swatchesHTML("sb-color", sub.color, "Colour")}</div>
      <div class="toggle-row"><div class="txt">Show notifications<small>When off, messages are still saved here, just without a pop-up.</small></div>${switchHTML("sb-notify", !sub.muted)}</div>
      <div class="toggle-row"><div class="txt">Play sound<small>Low and min priority messages are always silent.</small></div>${switchHTML("sb-sound", sub.sound)}</div>
      <div class="two">
        <div class="field"><label for="sb-prio">Notify for</label><select id="sb-prio">${PRIORITIES.map((p) => `<option value="${p.v}" ${p.v === sub.min_priority ? "selected" : ""}>${p.v === 1 ? "All priorities" : `${p.name} and above`}</option>`).join("")}</select></div>
        <div class="field"><label for="sb-kw">Only when the message contains</label><input type="text" id="sb-kw" value="${esc(sub.keywords.join(", "))}" placeholder="failed, error"><span class="help">Comma separated. Leave empty to get everything.</span></div>
      </div>
      <div class="btns">
        <button class="btn primary" data-save-sub>${sub.id ? "Save" : "Subscribe"}</button>
        <span class="result" id="sb-result"></span>
        <span class="grow"></span>
        ${sub.id ? `<button class="btn danger" data-delete-sub>Unsubscribe</button>` : ""}
        <button class="btn" data-cancel>Cancel</button>
      </div>
    </div>
  </div>`;
}

function generalTab() {
  if (!S.draft) S.draft = structuredClone(S.cfg.general);
  const g = S.draft;
  const isMica = g.backdrop === "mica";
  const q = g.quiet_hours;
  return `
  <div class="section">
    <h2>Look</h2>
    <div class="card">
      <div class="field"><span class="lbl">Window background</span>
        <div class="seg" id="g-mode">
          <button data-v="glass" class="${isMica ? "" : "on"}">Glass</button>
          <button data-v="mica" class="${isMica ? "on" : ""}">Mica</button>
        </div>
        <span class="help">Glass blurs what’s behind the window, like Tabby. While you’re in another app it turns solid, because Windows only blurs the active window. Mica isn’t see-through: it tints the window with your wallpaper colours and uses less GPU.</span>
      </div>
      <div class="field"><span class="lbl">Accent colour</span>${swatchesHTML("g-accent", g.accent, "Accent")}</div>
    </div>
  </div>
  <div class="section">
    <h2>Behaviour</h2>
    <div class="card">
      <div class="toggle-row"><div class="txt">Start with Windows<small>Starts quietly in the tray.</small></div>${switchHTML("g-autostart", g.autostart)}</div>
      <div class="toggle-row"><div class="txt">Notify about missed messages<small>After sleep or a lost connection. More than three are bundled into one notification.</small></div>${switchHTML("g-missed", g.notify_missed)}</div>
      <div class="toggle-row"><div class="txt">Keyboard shortcut for quick publish<small>Opens the quick publish window from anywhere in Windows.</small></div>${switchHTML("g-hotkey-on", g.publish_hotkey_enabled)}</div>
      <div class="field" id="g-hotkey-field" style="${g.publish_hotkey_enabled ? "" : "display:none"}"><label for="g-hotkey">Shortcut</label><input type="text" id="g-hotkey" class="hotkey" value="${esc(prettyHotkey(g.publish_hotkey))}" readonly><span class="help">Click the field, then press the keys you want.</span></div>
      <div class="field"><label for="g-limit">Messages to keep</label><input type="number" id="g-limit" min="50" max="20000" step="50" value="${g.history_limit}"></div>
    </div>
  </div>
  <div class="section">
    <h2>Quiet hours</h2>
    <div class="card">
      <div class="toggle-row"><div class="txt">Silence notifications at night<small>Messages still arrive and are kept in the history.</small></div>${switchHTML("g-quiet", q.enabled)}</div>
      <div class="two">
        <div class="field"><label for="g-qstart">From</label><input type="time" id="g-qstart" value="${esc(q.start)}"></div>
        <div class="field"><label for="g-qend">Until</label><input type="time" id="g-qend" value="${esc(q.end)}"></div>
      </div>
      <div class="toggle-row"><div class="txt">Let urgent messages through<small>Priority 5 still pops up during quiet hours.</small></div>${switchHTML("g-urgent", q.allow_urgent)}</div>
    </div>
  </div>
  <div class="section">
    <h2>Reset</h2>
    <p class="hint">Deletes everything ntfyar stores on this PC: servers and saved logins, topics, all messages and every setting. The app then starts over as if freshly installed. Your ntfy server and the topic list in your ntfy account aren’t touched, so the website and your phone stay as they are.</p>
    <div class="btns"><button class="btn danger" data-reset>Reset ntfyar…</button></div>
  </div>
  <div class="section">
    <p class="hint">ntfyar ${esc(S.version)}. Closing the window keeps it running in the tray. To quit, right-click the tray icon.</p>
  </div>
  <div class="savebar" id="g-savebar" hidden>
    <span class="grow">You have unsaved changes</span>
    <button class="btn btn-discard" data-discard>Discard changes</button>
    <button class="btn btn-save" data-save-general>Save changes</button>
  </div>`;
}

function prettyHotkey(raw) {
  if (!raw) return "None";
  return raw.replace("CommandOrControl", "Ctrl").replace(/\+/g, " + ");
}

// Appearance & behaviour edits go into a draft until "Save changes".
function isDirty() {
  return !!S.draft && JSON.stringify(S.draft) !== JSON.stringify(S.cfg.general);
}

function setDraft(patch) {
  Object.assign(S.draft, patch);
  // accent colour previews right away; it's put back if the changes are discarded
  applyAppearance({ ...S.cfg.general, accent: S.draft.accent });
  const bar = $("#g-savebar");
  if (bar) bar.hidden = !isDirty();
}

function discardDraft() {
  S.draft = null;
  applyAppearance(S.cfg.general);
}

async function saveDraft() {
  const btn = $("[data-save-general]");
  if (btn) btn.disabled = true;
  try {
    await invoke("save_general", { general: S.draft });
    S.cfg.general = structuredClone(S.draft);
    applyAppearance(S.cfg.general);
    $("#g-savebar").hidden = true;
    snack("Settings saved");
  } catch (err) {
    snack(String(err), true);
  }
  if (btn) btn.disabled = false;
}

/** Before leaving the settings page: ask if there are unsaved changes. */
function okToLeave() {
  if (!isDirty()) {
    S.draft = null;
    return true;
  }
  if (!confirm("You have unsaved changes in Appearance & behaviour. Discard them?")) return false;
  discardDraft();
  return true;
}

function bindSettings() {
  const root = $("#main");
  root.onclick = null;
  $$("[data-tab]", root).forEach((b) => (b.onclick = () => openSettings(b.dataset.tab)));
  $$("[data-edit-server]", root).forEach((r) => (r.onclick = () => openSettings("servers", { server: { ...serverById(r.dataset.editServer) } })));
  $$("[data-edit-sub]", root).forEach((r) => (r.onclick = () => openSettings("topics", { sub: structuredClone(subById(r.dataset.editSub)) })));
  $("[data-new-server]", root)?.addEventListener("click", () => openSettings("servers", { server: {} }));
  $("[data-new-sub]", root)?.addEventListener("click", () => openSettings("topics", { sub: newSub() }));
  $$("[data-cancel]", root).forEach((b) => (b.onclick = () => openSettings(S.tab)));
  $("[data-reset]", root)?.addEventListener("click", async (e) => {
    const ok = confirm(
      "Reset ntfyar?\n\nThis permanently deletes all servers, saved logins, topics, messages and settings on this PC. It can’t be undone.\n\nYour ntfy server and account aren’t affected."
    );
    if (!ok) return;
    e.target.disabled = true;
    try {
      await invoke("factory_reset");
    } catch (err) {
      snack(`Reset failed: ${err}`, true);
      e.target.disabled = false;
    }
  });

  // segmented controls
  $$(".seg", root).forEach((seg) =>
    seg.addEventListener("click", (e) => {
      const b = e.target.closest("button");
      if (!b) return;
      $$("button", seg).forEach((x) => x.classList.toggle("on", x === b));
      seg.dispatchEvent(new CustomEvent("change", { detail: b.dataset.v }));
    })
  );

  // server form
  $("#sv-auth")?.addEventListener("change", (e) => {
    const v = e.detail;
    $("#sv-cred").style.display = v === "none" ? "none" : "";
    $("#sv-user-wrap").style.display = v === "basic" ? "" : "none";
    $("#sv-secret-lbl").textContent = v === "token" ? "Access token" : "Password";
  });
  $("[data-test-server]", root)?.addEventListener("click", async (e) => {
    const r = $("#sv-result");
    const { server, secret } = readServerForm();
    e.target.disabled = true;
    r.className = "result"; r.textContent = "Testing…";
    try { r.textContent = await invoke("test_server", { server, secret }); r.className = "result ok"; }
    catch (err) { r.textContent = String(err); r.className = "result err"; }
    e.target.disabled = false;
  });
  $("[data-save-server]", root)?.addEventListener("click", async (e) => {
    const r = $("#sv-result");
    const { server, secret } = readServerForm();
    e.target.disabled = true;
    try {
      const saved = await invoke("save_server", { server, secret });
      await loadState();
      snack(`Saved ${saved.name}`);
      // first server → go straight to subscribing
      openSettings(S.cfg.subscriptions.length ? "servers" : "topics", S.cfg.subscriptions.length ? {} : { sub: newSub() });
    } catch (err) {
      r.textContent = String(err); r.className = "result err";
      e.target.disabled = false;
    }
  });
  $("[data-delete-server]", root)?.addEventListener("click", async () => {
    const srv = S.editServer;
    const n = S.cfg.subscriptions.filter((s) => s.server_id === srv.id).length;
    if (!confirm(`Remove “${srv.name}”${n ? ` and its ${n} topic${n > 1 ? "s" : ""}` : ""}? Its message history is deleted too.`)) return;
    await invoke("delete_server", { id: srv.id });
    await Promise.all([loadState(), loadHistory()]);
    S.view = { kind: "all" };
    openSettings("servers");
  });

  // subscription form
  bindSwatches($("#sb-color"), (c) => (S.editSub.color = c));
  $("[data-save-sub]", root)?.addEventListener("click", async (e) => {
    const sub = {
      ...S.editSub,
      topic: $("#sb-topic").value.trim(),
      server_id: $("#sb-server").value,
      display_name: $("#sb-name").value.trim(),
      color: S.editSub.color,
      muted: !$("#sb-notify").checked,
      sound: $("#sb-sound").checked,
      min_priority: +$("#sb-prio").value,
      keywords: $("#sb-kw").value.split(",").map((k) => k.trim()).filter(Boolean),
    };
    e.target.disabled = true;
    try {
      const saved = await invoke("save_subscription", { sub });
      await loadState();
      const isNew = !S.editSub.id;
      snack(isNew ? `Subscribed to ${saved.topic}` : "Saved");
      if (isNew) { S.view = { kind: "sub", subId: saved.id }; render(); }
      else openSettings("topics");
    } catch (err) {
      const r = $("#sb-result"); r.textContent = String(err); r.className = "result err";
      e.target.disabled = false;
    }
  });
  $("[data-delete-sub]", root)?.addEventListener("click", async () => {
    const sub = S.editSub;
    if (!confirm(`Unsubscribe from “${subLabel(sub)}” and delete its messages?`)) return;
    await invoke("delete_subscription", { id: sub.id, purge: true });
    await Promise.all([loadState(), loadHistory()]);
    openSettings("topics");
  });

  // general
  $("#g-mode")?.addEventListener("change", (e) => {
    const mica = e.detail === "mica";
    setDraft({ backdrop: mica ? "mica" : "acrylic" });
  });
  bindSwatches($("#g-accent"), (c) => setDraft({ accent: c }));
  $("#g-autostart")?.addEventListener("change", (e) => setDraft({ autostart: e.target.checked }));
  $("#g-missed")?.addEventListener("change", (e) => setDraft({ notify_missed: e.target.checked }));
  $("#g-limit")?.addEventListener("input", (e) => setDraft({ history_limit: Math.max(50, +e.target.value || 2000) }));
  $("#g-hotkey-on")?.addEventListener("change", (e) => {
    const on = e.target.checked;
    // switching it on with no shortcut set falls back to the default Ctrl + Alt + N
    const patch = { publish_hotkey_enabled: on };
    if (on && !S.draft.publish_hotkey) {
      patch.publish_hotkey = "CommandOrControl+Alt+N";
      $("#g-hotkey").value = prettyHotkey(patch.publish_hotkey);
    }
    $("#g-hotkey-field").style.display = on ? "" : "none";
    setDraft(patch);
  });
  $("[data-save-general]", root)?.addEventListener("click", saveDraft);
  $("[data-discard]", root)?.addEventListener("click", () => {
    discardDraft();
    renderSettings();
  });
  const quiet = () => setDraft({
    quiet_hours: { enabled: $("#g-quiet").checked, start: $("#g-qstart").value || "22:00", end: $("#g-qend").value || "07:00", allow_urgent: $("#g-urgent").checked },
  }, true);
  ["#g-quiet", "#g-qstart", "#g-qend", "#g-urgent"].forEach((s) => $(s)?.addEventListener("change", quiet));

  const hk = $("#g-hotkey");
  hk?.addEventListener("keydown", (e) => {
    e.preventDefault();
    if (["Control", "Alt", "Shift", "Meta"].includes(e.key)) return;
    let key = e.code.replace(/^Key/, "").replace(/^Digit/, "");
    if (e.code === "Space") key = "Space";
    if (!/^([A-Z0-9]|F\d{1,2}|Space)$/.test(key)) return;
    const mods = [];
    if (e.ctrlKey) mods.push("CommandOrControl");
    if (e.altKey) mods.push("Alt");
    if (e.shiftKey) mods.push("Shift");
    if (e.metaKey) mods.push("Super");
    if (!mods.length && !/^F\d/.test(key)) return snack("Use at least one of Ctrl, Alt or Shift", true);
    const raw = [...mods, key].join("+");
    hk.value = prettyHotkey(raw);
    setDraft({ publish_hotkey: raw });
  });
}

// ───────────────────────── render & events ─────────────────────────

function renderMain() {
  if (S.view.kind === "settings") renderSettings();
  else {
    $("#main").onclick = null;
    renderFeed();
  }
}

function render() {
  renderNav();
  renderMain();
}

let renderQueued = false;
function queueRender(full = true) {
  if (renderQueued) return;
  renderQueued = true;
  requestAnimationFrame(() => {
    renderQueued = false;
    renderNav();
    if (full && S.view.kind !== "settings") {
      const feed = $("#feed");
      const scroll = feed ? feed.scrollTop : 0;
      renderFeed();
      if ($("#feed")) $("#feed").scrollTop = scroll;
    }
  });
}

// ───────────────────────── dismiss / undo / delete ─────────────────────────

function shownCount() {
  return S.history.filter((m) => !m.dismissed).length;
}

// Dismissing several messages in a row collects them, so one Undo brings all back.
let undoBatch = [];
let undoTimer;

async function dismissMessages(ids) {
  const cards = ids.map((id) => $(`.msg[data-id="${CSS.escape(id)}"]`)).filter(Boolean);
  cards.forEach((c) => c.classList.add("leaving"));
  if (cards.length && !matchMedia("(prefers-reduced-motion: reduce)").matches) {
    await new Promise((r) => setTimeout(r, 150));
  }
  const prev = ids.map((id) => ({ id, read: !!S.history.find((m) => m.id === id)?.read }));
  await invoke("set_dismissed", { ids, dismissed: true, read: true });
  undoBatch.push(...prev);
  clearTimeout(undoTimer);
  undoTimer = setTimeout(() => (undoBatch = []), 6000);
  const n = undoBatch.length;
  snack(n === 1 ? "Message dismissed" : `${n} messages dismissed`, false, {
    label: "Undo",
    run: async () => {
      const batch = undoBatch;
      undoBatch = [];
      const wasUnread = batch.filter((b) => !b.read).map((b) => b.id);
      const wasRead = batch.filter((b) => b.read).map((b) => b.id);
      if (wasUnread.length) await invoke("set_dismissed", { ids: wasUnread, dismissed: false, read: false });
      if (wasRead.length) await invoke("set_dismissed", { ids: wasRead, dismissed: false, read: true });
    },
  });
}

async function restoreMessages(ids) {
  await invoke("set_dismissed", { ids, dismissed: false, read: null });
  snack(ids.length === 1 ? "Message restored" : `${ids.length} messages restored`);
}

function deleteForever(m) {
  if (!confirm("Delete this message permanently? This can’t be undone.")) return;
  invoke("delete_messages", { ids: [m.id], subId: null });
}

// ───────────────────────── right-click menus ─────────────────────────

async function confirmUnsubscribe(sub) {
  if (!confirm(`Unsubscribe from “${subLabel(sub)}” and delete its messages?`)) return;
  await invoke("delete_subscription", { id: sub.id, purge: true });
  await Promise.all([loadState(), loadHistory()]);
  if (S.view.kind === "sub" && S.view.subId === sub.id) S.view = { kind: "all" };
  render();
  snack(`Unsubscribed from ${sub.topic}`);
}

function topicMenu(sub, x, y) {
  const unread = S.history.filter((m) => m.sub_id === sub.id && !m.read).length;
  const count = S.history.filter((m) => m.sub_id === sub.id).length;
  showMenu(x, y, [
    { label: unread ? `Mark as read (${unread})` : "Mark as read", disabled: !unread,
      action: () => invoke("mark_read", { ids: null, subId: sub.id }) },
    { label: sub.muted ? "Unmute notifications" : "Mute notifications", action: async () => {
        try {
          await invoke("save_subscription", { sub: { ...sub, muted: !sub.muted } });
          await loadState();
          renderNav();
          snack(sub.muted ? `Notifications on for ${subLabel(sub)}` : `Muted ${subLabel(sub)}`);
        } catch (err) { snack(String(err), true); }
      } },
    { label: "Edit topic…", action: () => openSettings("topics", { sub: structuredClone(sub) }) },
    "sep",
    { label: "Clear messages", disabled: !count, action: () => {
        if (confirm(`Delete all messages in “${subLabel(sub)}”?`)) invoke("delete_messages", { ids: null, subId: sub.id });
      } },
    { label: "Unsubscribe", danger: true, action: () => confirmUnsubscribe(sub) },
  ]);
}

function messageMenu(m, x, y) {
  const link = m.click && /^https?:\/\//.test(m.click) ? m.click : "";
  showMenu(x, y, [
    { label: "Mark as read", disabled: m.read, action: () => invoke("mark_read", { ids: [m.id], subId: null }) },
    { label: "Copy text", action: async () => {
        await navigator.clipboard.writeText([m.title, m.message].filter(Boolean).join("\n"));
        snack("Copied");
      } },
    ...(link ? [{ label: "Open link", action: () => invoke("open_url", { url: link }).catch((e) => snack(String(e), true)) }] : []),
    "sep",
    m.dismissed
      ? { label: "Restore", action: () => restoreMessages([m.id]) }
      : { label: "Dismiss", action: () => dismissMessages([m.id]) },
    { label: "Delete permanently…", danger: true, action: () => deleteForever(m) },
  ]);
}

function bindContextMenus() {
  $("#nav").addEventListener("contextmenu", (e) => {
    const b = e.target.closest("[data-nav]");
    if (!b) return;
    e.preventDefault();
    if (b.dataset.nav === "all") {
      const unread = S.history.filter((m) => !m.read).length;
      showMenu(e.clientX, e.clientY, [
        { label: unread ? `Mark all as read (${unread})` : "Mark all as read", disabled: !unread,
          action: () => invoke("mark_read", { ids: null, subId: null }) },
      ]);
    } else {
      const sub = subById(b.dataset.id);
      if (sub) topicMenu(sub, e.clientX, e.clientY);
    }
  });
  $("#main").addEventListener("contextmenu", (e) => {
    const card = e.target.closest(".msg");
    // keep the normal copy menu when text is selected
    if (!card || window.getSelection().toString()) return;
    const m = S.history.find((x) => x.id === card.dataset.id);
    if (!m) return;
    e.preventDefault();
    messageMenu(m, e.clientX, e.clientY);
  });
}

function focusMessage(id) {
  if (onGeneralTab() && !okToLeave()) return;
  S.view = { kind: "all" };
  S.search = ""; S.prio = 0; S.unreadOnly = false; S.showDismissed = false;
  $("#search").value = "";
  render();
  const el = $(`.msg[data-id="${CSS.escape(id)}"]`);
  if (el) {
    el.scrollIntoView({ block: "center" });
    el.classList.add("flash");
  }
}

// window buttons
$("#win-min").onclick = () => appWindow.minimize();
$("#win-max").onclick = () => appWindow.toggleMaximize();
$("#win-close").onclick = () => appWindow.hide();

document.addEventListener("keydown", (e) => {
  if (e.ctrlKey && e.key.toLowerCase() === "f") { e.preventDefault(); $("#search").focus(); $("#search").select(); }
  if (e.ctrlKey && e.key.toLowerCase() === "n") { e.preventDefault(); invoke("open_publish"); }
  if (e.ctrlKey && e.key === ",") { e.preventDefault(); openSettings("general"); }
  if (e.key === "Escape" && document.activeElement === $("#search")) { $("#search").value = ""; S.search = ""; $("#search").blur(); renderMain(); }
});

(async function init() {
  await loadState();
  await loadHistory();
  render();

  listen("message", (m) => {
    if (S.history.some((x) => x.id === m.id)) return;
    S.history.unshift(m);
    S.history.sort((a, b) => b.time - a.time);
    queueRender(true);
  });
  listen("history-changed", async () => { await loadHistory(); queueRender(true); });
  listen("status", (st) => { S.statuses = st; renderNav(); });
  listen("paused", (p) => { S.paused = p; if (S.view.kind !== "settings") queueRender(true); });
  listen("general-changed", (g) => {
    S.cfg.general = g;
    // the publish window only changes the remembered topic; keep any unsaved edits here
    if (S.draft) S.draft.default_publish_sub = g.default_publish_sub;
    applyAppearance(S.draft ? { ...g, accent: S.draft.accent } : g);
  });
  listen("focus-message", (id) => focusMessage(id));
  // after a reset, start over with a clean page
  listen("factory-reset", () => location.reload());
  bindContextMenus();
  listen("topics-synced", async ({ added, removed }) => {
    await Promise.all([loadState(), loadHistory()]);
    if (S.view.kind === "sub" && !subById(S.view.subId)) S.view = { kind: "all" };
    // don't throw away a form the user is filling in
    if (S.view.kind === "settings" && (S.editSub || S.editServer)) renderNav();
    else render();
    const parts = [];
    if (added) parts.push(`${added} topic${added === 1 ? "" : "s"} added`);
    if (removed) parts.push(`${removed} removed`);
    snack(`Synced with your ntfy account: ${parts.join(", ")}`);
  });
})();
