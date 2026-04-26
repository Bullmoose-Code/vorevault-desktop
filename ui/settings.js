// Settings window bridge: invoke Tauri commands, listen for state events,
// re-render the whole DOM on each update. Uses window.__TAURI__ globals
// (withGlobalTauri = true in tauri.conf.json).

const { core: tCore, event: tEvent, opener: tOpener, dialog: tDialog } = window.__TAURI__;

const REPORT_URL = "https://github.com/Bullmoose-Code/vorevault-desktop/issues/new";

let autostartEnabled = false;
let signedIn = false;

async function loadAndRender() {
  let state, autostart;
  try {
    state = await tCore.invoke("get_state");
  } catch (e) {
    console.error("get_state failed", e);
    renderError();
    return;
  }
  try {
    autostart = await tCore.invoke("get_autostart");
  } catch (e) {
    console.warn("get_autostart failed", e);
    autostart = false;
  }
  autostartEnabled = !!autostart;
  signedIn = !!state.username;
  render(state);
}

function render(state) {
  renderAccount(state);
  renderFolder(state);
  renderAutostart();
  renderVersion(state);
}

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

function renderFolder(state) {
  const folder = document.getElementById("ctrl-folder");
  const row = document.getElementById("row-folder");
  folder.replaceChildren();
  if (!signedIn) {
    row.classList.add("disabled");
    const btn = mkBtn("—", "btn", null);
    btn.disabled = true;
    folder.appendChild(btn);
    return;
  }
  row.classList.remove("disabled");
  if (state.watch_folder_label) {
    const btn = mkBtn(state.watch_folder_label, "btn", onPickFolder);
    btn.title = state.watch_folder; // full path on hover
    folder.appendChild(btn);
  } else {
    folder.appendChild(mkBtn("choose a folder…", "btn btn-warn", onPickFolder));
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
    onToggleAutostart
  );
  auto.appendChild(btn);
}

function renderVersion(state) {
  document.getElementById("ctrl-version").textContent = "v" + state.version;
}

function renderError() {
  const root = document.getElementById("root");
  root.replaceChildren();
  const div = document.createElement("div");
  div.className = "error-banner";
  div.textContent = "VoreVault — couldn't load settings, please reopen.";
  root.appendChild(div);
}

function mkBtn(label, cls, onClick) {
  const b = document.createElement("button");
  b.className = cls;
  b.textContent = label;
  if (onClick) b.addEventListener("click", onClick);
  return b;
}

async function onSignIn() {
  try { await tCore.invoke("sign_in"); }
  catch (e) { console.error(e); }
}

async function onSignOut() {
  try { await tCore.invoke("sign_out"); }
  catch (e) { console.error(e); }
}

async function onPickFolder() {
  const picked = await tDialog.open({ directory: true, multiple: false });
  if (!picked) return;
  try {
    await tCore.invoke("change_watch_folder", { path: picked });
  } catch (e) {
    showFolderError(typeof e === "string" ? e : "couldn't change folder");
  }
}

function showFolderError(msg) {
  const folder = document.getElementById("ctrl-folder");
  const existing = folder.querySelector(".inline-error");
  if (existing) existing.remove();
  const err = document.createElement("span");
  err.className = "inline-error";
  err.textContent = msg;
  folder.appendChild(err);
}

async function onToggleAutostart() {
  const next = !autostartEnabled;
  try {
    await tCore.invoke("set_autostart", { enabled: next });
    autostartEnabled = next;
  } catch (e) {
    console.warn("set_autostart failed", e);
    try { autostartEnabled = await tCore.invoke("get_autostart"); }
    catch (_) {}
  }
  await loadAndRender();
}

document.getElementById("report-issue").addEventListener("click", async (e) => {
  e.preventDefault();
  try { await tOpener.openUrl(REPORT_URL); }
  catch (err) { console.warn("openUrl failed", err); }
});

// Re-render on backend state pushes.
tEvent.listen("settings:state-changed", (evt) => {
  signedIn = !!evt.payload.username;
  render(evt.payload);
});

// Initial paint.
loadAndRender();
