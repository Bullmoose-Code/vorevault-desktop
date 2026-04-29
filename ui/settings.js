// Settings window: rules list + edit panel + global rows.
// Whole-DOM re-render on each state change. window.__TAURI__ globals
// (withGlobalTauri = true in tauri.conf.json).

const { core: tCore, event: tEvent, opener: tOpener, dialog: tDialog } = window.__TAURI__;

const REPORT_URL = "https://github.com/Bullmoose-Code/vorevault-desktop/issues/new";
const TAG_REGEX = /^[a-z0-9][a-z0-9-]{0,31}$/;

let autostartEnabled = false;
let signedIn = false;
let currentUpdaterState = { kind: "Idle", value: "" };

// Edit-panel state — set by edit/add flow, cleared on save/cancel.
let editing = null; // { id, path, vault_folder_id, vault_folder_label, tags } or null
let folderCache = null; // [{id, name, breadcrumb}] or null
let tagCache = null; // [{name, file_count}] or null

// ───── boot ─────

async function loadAndRender() {
  let state, autostart, updaterState;
  try {
    state = await tCore.invoke("get_state");
  } catch (e) {
    renderError("couldn't load settings: " + (e?.message || e));
    return;
  }
  try {
    autostart = await tCore.invoke("get_autostart");
  } catch (e) {
    autostart = false;
  }
  try {
    updaterState = await tCore.invoke("updater_get_state");
  } catch (e) {
    updaterState = { kind: "Error", value: e?.message || String(e) };
  }
  autostartEnabled = !!autostart;
  signedIn = !!state.username;
  currentUpdaterState = updaterState;
  render(state);
}

function render(state) {
  renderAccount(state);
  renderAutostart();
  renderVersion(state);
  renderUpdates(currentUpdaterState, state.version);
  renderRules(state.rules || []);
}

// ───── account / autostart / version / updates (unchanged behavior) ─────

function renderAccount(state) {
  const acct = document.getElementById("ctrl-account");
  acct.replaceChildren();
  if (state.username) {
    const name = document.createElement("span");
    name.className = "username";
    name.textContent = "@" + state.username;
    acct.appendChild(name);
    acct.appendChild(mkBtn("sign out", "btn btn-danger", onSignOut));
  } else {
    const txt = document.createElement("span");
    txt.className = "signed-out-text";
    txt.textContent = "not signed in";
    acct.appendChild(txt);
    acct.appendChild(mkBtn("sign in with Discord", "btn", onSignIn));
  }
}

function renderAutostart() {
  const auto = document.getElementById("ctrl-autostart");
  const row = document.getElementById("row-autostart");
  auto.replaceChildren();
  if (!signedIn) {
    row.classList.add("disabled");
    const btn = mkBtn("—", "btn", null);
    btn.disabled = true;
    auto.appendChild(btn);
    return;
  }
  row.classList.remove("disabled");
  const btn = mkBtn(
    autostartEnabled ? "on" : "off",
    autostartEnabled ? "btn btn-go" : "btn",
    onToggleAutostart,
  );
  auto.appendChild(btn);
}

function renderVersion(state) {
  document.getElementById("ctrl-version").textContent = "v" + state.version;
}

function renderUpdates(updaterState, currentVersion) {
  const ctrl = document.getElementById("ctrl-updates");
  if (!ctrl) return;
  ctrl.replaceChildren();

  const status = document.createElement("span");
  status.className = "updates-status";

  const kind = updaterState?.kind || "Idle";
  const value = updaterState?.value || "";

  let statusText;
  let isError = false;
  switch (kind) {
    case "Idle":
    case "UpToDate": statusText = `up to date · v${currentVersion}`; break;
    case "Checking": statusText = "checking…"; break;
    case "DownloadingUpdate": statusText = `downloading v${value} in background`; break;
    case "Ready": statusText = `update v${value} ready — restart to apply`; break;
    case "Error": statusText = `couldn't check (${value}) · retry`; isError = true; break;
    default: statusText = `unknown state: ${kind}`; isError = true;
  }
  if (isError) status.classList.add("error");
  status.textContent = statusText;
  ctrl.appendChild(status);

  const btnRow = document.createElement("span");
  btnRow.className = "updates-buttons";
  const checkEnabled = kind === "Idle" || kind === "UpToDate" || kind === "Error";
  const restartVisible = kind === "Ready";
  if (!restartVisible) {
    const checkBtn = mkBtn("check now", "btn", onCheckNow);
    if (!checkEnabled) checkBtn.disabled = true;
    btnRow.appendChild(checkBtn);
  } else {
    const restartBtn = mkBtn("restart now", "btn btn-go", onRestartNow);
    btnRow.appendChild(restartBtn);
  }
  ctrl.appendChild(btnRow);
}

// ───── rules list ─────

function renderRules(rules) {
  const list = document.getElementById("rules-list");
  const empty = document.getElementById("rules-empty");
  const addBtn = document.getElementById("add-rule-btn");
  list.replaceChildren();

  if (!signedIn) {
    empty.hidden = false;
    empty.textContent = "Sign in to configure watched folders.";
    addBtn.disabled = true;
    return;
  }
  addBtn.disabled = false;

  if (rules.length === 0) {
    empty.hidden = false;
    empty.textContent = "No watched folders. Add one to start uploading.";
    return;
  }
  empty.hidden = true;

  for (const r of rules) {
    list.appendChild(renderRuleCard(r));
  }
}

function renderRuleCard(rule) {
  const li = document.createElement("li");
  li.className = "rule-card";

  const path = document.createElement("span");
  path.className = "rule-path";
  path.textContent = rule.path;

  const dest = document.createElement("span");
  dest.className = "rule-dest";
  if (rule.vault_folder_id && rule.vault_folder_label) {
    dest.textContent = `→ ${rule.vault_folder_label}`;
  } else {
    dest.textContent = "→ My Files (default)";
  }

  const tags = document.createElement("span");
  tags.className = "rule-tags";
  if (rule.tags && rule.tags.length > 0) {
    tags.textContent = rule.tags.map((t) => "#" + t).join(" ");
  } else {
    tags.textContent = "no tags";
  }

  const actions = document.createElement("span");
  actions.className = "rule-actions";
  actions.appendChild(mkBtn("edit", "btn", () => startEditing(rule)));
  actions.appendChild(mkBtn("×", "btn btn-danger", () => onDeleteRule(rule.id)));

  li.appendChild(path);
  li.appendChild(dest);
  li.appendChild(tags);
  li.appendChild(actions);
  return li;
}

async function onDeleteRule(id) {
  try {
    await tCore.invoke("delete_rule", { id });
  } catch (e) {
    console.error("delete_rule failed:", e);
  }
}

// ───── edit panel ─────

function startEditing(rule) {
  editing = rule
    ? {
        id: rule.id,
        path: rule.path,
        vault_folder_id: rule.vault_folder_id ?? null,
        vault_folder_label: rule.vault_folder_label ?? null,
        tags: [...(rule.tags ?? [])],
      }
    : { id: crypto.randomUUID(), path: "", vault_folder_id: null, vault_folder_label: null, tags: [] };

  showEditPanel();
  paintEditPanel();
  // Pre-fetch folder + tag lists so the picker / autocomplete are responsive.
  refreshFolderCache();
  refreshTagCache();
}

function showEditPanel() {
  document.getElementById("rules-section").hidden = true;
  document.getElementById("edit-panel").hidden = false;
}

function hideEditPanel() {
  editing = null;
  document.getElementById("edit-panel").hidden = true;
  document.getElementById("rules-section").hidden = false;
  document.getElementById("edit-error").hidden = true;
  document.getElementById("edit-error").textContent = "";
}

function paintEditPanel() {
  if (!editing) return;
  document.getElementById("edit-path-display").textContent = editing.path || "(no folder picked yet)";
  paintSelectedDest();
  paintChips();
}

function paintSelectedDest() {
  const span = document.getElementById("edit-dest-selected");
  if (editing.vault_folder_id) {
    span.textContent = `selected: ${editing.vault_folder_label ?? "(unknown folder)"}`;
  } else {
    span.textContent = "selected: My Files (default)";
  }
}

function paintChips() {
  const wrap = document.getElementById("edit-chip-input");
  // Remove all existing chips, keeping the inline input field.
  for (const child of Array.from(wrap.children)) {
    if (!child.classList.contains("chip-text")) child.remove();
  }
  const input = document.getElementById("edit-tag-input");
  for (const t of editing.tags) {
    const chip = document.createElement("span");
    chip.className = "chip";
    chip.textContent = t;
    const x = document.createElement("button");
    x.type = "button";
    x.textContent = "×";
    x.addEventListener("click", () => {
      editing.tags = editing.tags.filter((tag) => tag !== t);
      paintChips();
    });
    chip.appendChild(x);
    wrap.insertBefore(chip, input);
  }
}

async function refreshFolderCache() {
  try {
    folderCache = await tCore.invoke("fetch_folders");
    paintDestResults();
  } catch (e) {
    console.error("fetch_folders failed:", e);
    folderCache = [];
  }
}

async function refreshTagCache() {
  try {
    tagCache = await tCore.invoke("fetch_tags");
  } catch (e) {
    console.error("fetch_tags failed:", e);
    tagCache = [];
  }
}

function paintDestResults() {
  const ul = document.getElementById("edit-dest-results");
  ul.replaceChildren();
  const search = document.getElementById("edit-dest-search").value.trim().toLowerCase();
  const list = folderCache ?? [];
  const matches = search
    ? list.filter((f) => f.breadcrumb.toLowerCase().includes(search))
    : list;

  // "no folder" default option.
  const defaultLi = document.createElement("li");
  defaultLi.className = "dest-default";
  defaultLi.textContent = "(no folder — upload to my home)";
  defaultLi.addEventListener("click", () => {
    editing.vault_folder_id = null;
    editing.vault_folder_label = null;
    paintSelectedDest();
    ul.replaceChildren();
  });
  ul.appendChild(defaultLi);

  for (const f of matches.slice(0, 50)) {
    const li = document.createElement("li");
    li.textContent = f.breadcrumb;
    li.addEventListener("click", () => {
      editing.vault_folder_id = f.id;
      editing.vault_folder_label = f.breadcrumb;
      paintSelectedDest();
      ul.replaceChildren();
    });
    ul.appendChild(li);
  }
}

let tagAutocompleteTimer = null;
function paintTagSuggestions() {
  if (tagAutocompleteTimer) clearTimeout(tagAutocompleteTimer);
  tagAutocompleteTimer = setTimeout(() => {
    const ul = document.getElementById("edit-tag-suggestions");
    ul.replaceChildren();
    if (!editing) return;
    const q = document.getElementById("edit-tag-input").value.trim().toLowerCase();
    if (!q) return;
    const list = tagCache ?? [];
    const matches = list
      .filter((t) => t.name.includes(q) && !editing.tags.includes(t.name))
      .sort((a, b) => b.file_count - a.file_count)
      .slice(0, 8);
    for (const t of matches) {
      const li = document.createElement("li");
      li.textContent = `#${t.name}  (${t.file_count})`;
      li.addEventListener("mousedown", (ev) => {
        // mousedown so it fires before the input loses focus.
        ev.preventDefault();
        commitTag(t.name);
      });
      ul.appendChild(li);
    }
  }, 100);
}

function commitTag(raw) {
  const norm = raw.trim().toLowerCase();
  if (!TAG_REGEX.test(norm)) {
    document.getElementById("edit-chip-input").classList.add("invalid");
    showEditError("invalid tag — lowercase letters/digits/hyphens, 1–32 chars, no leading hyphen");
    return false;
  }
  document.getElementById("edit-chip-input").classList.remove("invalid");
  document.getElementById("edit-error").hidden = true;
  if (!editing.tags.includes(norm)) {
    editing.tags.push(norm);
    paintChips();
  }
  document.getElementById("edit-tag-input").value = "";
  document.getElementById("edit-tag-suggestions").replaceChildren();
  return true;
}

function showEditError(msg) {
  const el = document.getElementById("edit-error");
  el.textContent = msg;
  el.hidden = false;
}

async function onPickEditFolder() {
  const picked = await tDialog.open({ directory: true, multiple: false });
  if (!picked) return;
  editing.path = picked;
  paintEditPanel();
}

async function onSaveEdit() {
  if (!editing.path) {
    showEditError("pick a folder first");
    return;
  }
  // Commit any pending text in the tag input — otherwise a typed-but-
  // uncommitted token is silently dropped on Save.
  const pending = document.getElementById("edit-tag-input").value.trim();
  if (pending && !commitTag(pending)) {
    // commitTag already showed an inline validation error.
    return;
  }
  try {
    await tCore.invoke("save_rule", { rule: {
      id: editing.id,
      path: editing.path,
      vault_folder_id: editing.vault_folder_id,
      vault_folder_label: editing.vault_folder_label,
      tags: editing.tags,
    }});
    hideEditPanel();
  } catch (e) {
    showEditError(typeof e === "string" ? e : "couldn't save rule");
  }
}

function onCancelEdit() {
  hideEditPanel();
}

// ───── helpers ─────

function mkBtn(label, cls, onClick) {
  const b = document.createElement("button");
  b.className = cls;
  b.textContent = label;
  if (onClick) b.addEventListener("click", onClick);
  return b;
}

function renderError(msg) {
  const root = document.getElementById("root");
  root.replaceChildren();
  const div = document.createElement("div");
  div.className = "error-banner";
  div.textContent = msg || "VoreVault — couldn't load settings, please reopen.";
  root.appendChild(div);
}

async function onCheckNow() { try { await tCore.invoke("updater_check_now"); } catch (e) { console.error(e); } }
async function onRestartNow() { try { await tCore.invoke("updater_install_and_restart"); } catch (e) { console.error(e); } }
async function onSignIn() { try { await tCore.invoke("sign_in"); } catch (e) { console.error(e); } }
async function onSignOut() { try { await tCore.invoke("sign_out"); } catch (e) { console.error(e); } }

async function onToggleAutostart() {
  const next = !autostartEnabled;
  try {
    await tCore.invoke("set_autostart", { enabled: next });
    autostartEnabled = next;
  } catch (e) {
    console.warn("set_autostart failed", e);
    try { autostartEnabled = await tCore.invoke("get_autostart"); } catch (_) {}
  }
  await loadAndRender();
}

// ───── event wiring ─────

document.getElementById("report-issue").addEventListener("click", async (e) => {
  e.preventDefault();
  try { await tOpener.openUrl(REPORT_URL); }
  catch (err) { console.warn("openUrl failed", err); }
});

document.getElementById("add-rule-btn").addEventListener("click", () => startEditing(null));
document.getElementById("edit-pick-folder-btn").addEventListener("click", onPickEditFolder);
document.getElementById("edit-save-btn").addEventListener("click", onSaveEdit);
document.getElementById("edit-cancel-btn").addEventListener("click", onCancelEdit);

document.getElementById("edit-dest-search").addEventListener("input", paintDestResults);
document.getElementById("edit-tag-input").addEventListener("input", paintTagSuggestions);
document.getElementById("edit-tag-input").addEventListener("keydown", (ev) => {
  if (!editing) return;
  if (ev.key === "Enter" || ev.key === ",") {
    ev.preventDefault();
    commitTag(ev.currentTarget.value);
  } else if (ev.key === "Backspace" && ev.currentTarget.value === "" && editing.tags.length > 0) {
    editing.tags.pop();
    paintChips();
  }
});

tEvent.listen("settings:state-changed", () => { loadAndRender(); });
tEvent.listen("updater:state-changed", () => { loadAndRender(); });

loadAndRender();
