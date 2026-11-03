// Shared helpers for both windows.
const TAURI = window.__TAURI__;
const invoke = (cmd, args) => TAURI.core.invoke(cmd, args);
const listen = (ev, fn) => TAURI.event.listen(ev, (e) => fn(e.payload));
const appWindow = TAURI.window.getCurrentWindow();

const $ = (sel, root = document) => root.querySelector(sel);
const $$ = (sel, root = document) => [...root.querySelectorAll(sel)];

function esc(s) {
  return String(s ?? "").replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]));
}

/** Escapes text and turns bare http(s) URLs into links. */
function linkify(s) {
  return esc(s).replace(/\bhttps?:\/\/[^\s<>"']+[^\s<>"'.,;:!?)\]]/g, (u) => `<a href="${u}" data-ext>${u}</a>`);
}

/** Small, safe Markdown subset: code blocks, inline code, bold, italics, links, lists, headings. */
function markdown(src) {
  const blocks = [];
  let s = String(src ?? "").replace(/```[^\n]*\n?([\s\S]*?)```/g, (_, code) => {
    blocks.push(`<pre>${esc(code.replace(/\n$/, ""))}</pre>`);
    return `\u0000${blocks.length - 1}\u0000`;
  });
  s = esc(s)
    .replace(/`([^`\n]+)`/g, "<code>$1</code>")
    .replace(/\*\*([^*\n]+)\*\*/g, "<strong>$1</strong>")
    .replace(/(^|[^*])\*([^*\n]+)\*/g, "$1<em>$2</em>")
    .replace(/\[([^\]\n]+)\]\((https?:\/\/[^)\s]+)\)/g, '<a href="$2" data-ext>$1</a>')
    .replace(/(^|[\s(])(https?:\/\/[^\s<]+[^\s<.,;:!?)])/g, '$1<a href="$2" data-ext>$2</a>');
  const out = [];
  for (const para of s.split(/\n{2,}/)) {
    const lines = para.split("\n");
    if (lines.every((l) => /^\s*[-*] /.test(l))) {
      out.push("<ul>" + lines.map((l) => `<li>${l.replace(/^\s*[-*] /, "")}</li>`).join("") + "</ul>");
    } else if (/^#{1,3} /.test(para)) {
      out.push(`<p><strong>${para.replace(/^#{1,3} /, "")}</strong></p>`);
    } else {
      out.push(`<p>${lines.join("<br>")}</p>`);
    }
  }
  return out.join("").replace(/\u0000(\d+)\u0000/g, (_, i) => blocks[+i]);
}

function fmtTime(sec) {
  return new Date(sec * 1000).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
}

function dayLabel(sec) {
  const d = new Date(sec * 1000);
  const today = new Date(); today.setHours(0, 0, 0, 0);
  const that = new Date(d); that.setHours(0, 0, 0, 0);
  const diff = Math.round((today - that) / 86400000);
  if (diff === 0) return "Today";
  if (diff === 1) return "Yesterday";
  if (diff < 7) return d.toLocaleDateString([], { weekday: "long" });
  return d.toLocaleDateString([], { day: "numeric", month: "long", year: today.getFullYear() === d.getFullYear() ? undefined : "numeric" });
}

function fmtSize(b) {
  if (!b) return "";
  if (b < 1024) return `${b} B`;
  if (b < 1048576) return `${(b / 1024).toFixed(0)} KB`;
  return `${(b / 1048576).toFixed(1)} MB`;
}

function applyAppearance(general) {
  const r = document.documentElement.style;
  r.setProperty("--accent", general.accent || "#7aa2f7");
  const mica = general.backdrop === "mica";
  // Glass: blurred background at a fixed 80% tint. Mica isn't see-through, so it gets a
  // light tint that lets the Mica colour show.
  r.setProperty("--tint", mica ? "0.25" : "0.80");
  // Windows drops the acrylic blur on inactive windows and shows flat grey instead,
  // so go solid while the window isn't focused.
  document.documentElement.classList.toggle("blur-mode", !mica);
}

appWindow.onFocusChanged(({ payload: focused }) => {
  document.documentElement.classList.toggle("inactive", !focused);
});

// ───────── right-click menu ─────────
// items: [{ label, action, danger?, disabled? } | "sep"]
function showMenu(x, y, items) {
  closeMenu();
  const menu = document.createElement("div");
  menu.className = "ctx";
  menu.setAttribute("role", "menu");
  for (const it of items) {
    if (it === "sep") {
      menu.appendChild(Object.assign(document.createElement("div"), { className: "ctx-sep" }));
      continue;
    }
    const b = document.createElement("button");
    b.type = "button";
    b.setAttribute("role", "menuitem");
    b.textContent = it.label;
    b.disabled = !!it.disabled;
    if (it.danger) b.classList.add("danger");
    b.addEventListener("click", () => {
      closeMenu();
      it.action();
    });
    menu.appendChild(b);
  }
  document.body.appendChild(menu);
  // keep it inside the window
  const r = menu.getBoundingClientRect();
  menu.style.left = `${Math.max(4, Math.min(x, innerWidth - r.width - 6))}px`;
  menu.style.top = `${Math.max(4, Math.min(y, innerHeight - r.height - 6))}px`;
  menu.querySelector("button:not(:disabled)")?.focus();
  menu.addEventListener("keydown", (e) => {
    const btns = [...menu.querySelectorAll("button:not(:disabled)")];
    const i = btns.indexOf(document.activeElement);
    if (e.key === "ArrowDown") { e.preventDefault(); btns[(i + 1) % btns.length]?.focus(); }
    if (e.key === "ArrowUp") { e.preventDefault(); btns[(i - 1 + btns.length) % btns.length]?.focus(); }
  });
}

function closeMenu() {
  document.querySelector(".ctx")?.remove();
}

document.addEventListener("mousedown", (e) => {
  if (!e.target.closest(".ctx")) closeMenu();
});
document.addEventListener("keydown", (e) => {
  if (e.key === "Escape") closeMenu();
});
window.addEventListener("blur", closeMenu);
window.addEventListener("resize", closeMenu);
document.addEventListener("scroll", closeMenu, true);

// No browser menu ("Reload", "Inspect", …) — except where copy/paste is useful.
document.addEventListener("contextmenu", (e) => {
  if (e.defaultPrevented) return;
  const editable = e.target.closest("input, textarea, [contenteditable]");
  const selected = window.getSelection().toString().length > 0;
  if (!editable && !selected) e.preventDefault();
});

let snackTimer;
let snackAction = null;
/** Toast. `action` = { label, run } adds a button (e.g. Undo) and keeps the toast up longer. */
function snack(text, isErr = false, action = null) {
  const el = $("#snack");
  if (!el) return;
  el.textContent = "";
  el.append(text);
  snackAction = action;
  if (action) {
    const b = document.createElement("button");
    b.type = "button";
    b.className = "snack-btn";
    b.textContent = action.label;
    b.addEventListener("click", () => runSnackAction());
    el.append(b);
  }
  el.classList.toggle("err", isErr);
  el.classList.toggle("has-action", !!action);
  el.classList.add("show");
  clearTimeout(snackTimer);
  snackTimer = setTimeout(hideSnack, action ? 6000 : isErr ? 4500 : 2200);
}

function hideSnack() {
  $("#snack")?.classList.remove("show", "has-action");
  snackAction = null;
}

function runSnackAction() {
  const a = snackAction;
  hideSnack();
  a?.run();
}

// Ctrl+Z triggers the toast's action (Undo) while it's showing
document.addEventListener("keydown", (e) => {
  if (e.ctrlKey && !e.shiftKey && e.key.toLowerCase() === "z" && snackAction && !e.target.closest("input, textarea")) {
    e.preventDefault();
    runSnackAction();
  }
});

// External links always open in the default browser.
document.addEventListener("click", (e) => {
  const a = e.target.closest("a[data-ext]");
  if (!a) return;
  e.preventDefault();
  invoke("open_url", { url: a.getAttribute("href") }).catch((err) => snack(String(err), true));
});

const PRIORITIES = [
  { v: 1, name: "Min" },
  { v: 2, name: "Low" },
  { v: 3, name: "Default" },
  { v: 4, name: "High" },
  { v: 5, name: "Urgent" },
];
// Ordered as a colour wheel so the picker reads as a spectrum.
const COLORS = [
  "#f7768e", "#ff5c8a", "#ff7a59", "#ff9e64", "#e0af68", "#f2d06b",
  "#c3e88d", "#9ece6a", "#4fd6be", "#2ac3de", "#7dcfff", "#7aa2f7",
  "#6f7cff", "#9d7cd8", "#bb9af7", "#e48bd9", "#c0caf5", "#a9b1d6",
];

/** Swatch row with all preset colours plus a custom colour picker. */
function swatchesHTML(id, current, label) {
  const cur = (current || "").toLowerCase();
  const isCustom = cur && !COLORS.includes(cur);
  return `<div class="swatches" id="${id}">
    ${COLORS.map((c) => `<button type="button" style="--c:${c}" data-c="${c}" class="${c === cur ? "on" : ""}" aria-label="${label} ${c}"></button>`).join("")}
    <label class="custom ${isCustom ? "on" : ""}" style="--c:${isCustom ? esc(cur) : "transparent"}" title="Pick any colour">
      <input type="color" value="${isCustom ? esc(cur) : "#7aa2f7"}" aria-label="Custom ${label.toLowerCase()}">
    </label>
  </div>`;
}

/** Wires a swatch row; onPick(colour) is called for presets and the custom picker. */
function bindSwatches(root, onPick) {
  if (!root) return;
  const custom = root.querySelector(".custom");
  const input = custom.querySelector("input");
  const select = (el) => root.querySelectorAll("button, .custom").forEach((x) => x.classList.toggle("on", x === el));
  root.addEventListener("click", (e) => {
    const b = e.target.closest("button[data-c]");
    if (!b) return;
    select(b);
    onPick(b.dataset.c);
  });
  input.addEventListener("input", () => {
    custom.style.setProperty("--c", input.value);
    select(custom);
    onPick(input.value);
  });
}
