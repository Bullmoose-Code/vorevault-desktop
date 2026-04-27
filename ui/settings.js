// Settings window bridge: invoke Tauri commands, listen for state events,
// re-render the whole DOM on each update. Uses window.__TAURI__ globals
// (withGlobalTauri = true in tauri.conf.json).

const { core: tCore, event: tEvent, opener: tOpener, dialog: tDialog } = window.__TAURI__;

const REPORT_URL = "https://github.com/Bullmoose-Code/vorevault-desktop/issues/new";

let autostartEnabled = false;
let signedIn = false;
let currentUpdaterState = { kind: "Idle", value: "" };

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
    autostart = { enabled: false, error: e?.message || String(e) };
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
  renderFolder(state);
  renderAutostart();
  renderVersion(state);
  renderUpdates(currentUpdaterState, state.version);
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

function renderError(msg) {
  const root = document.getElementById("root");
  root.replaceChildren();
  const div = document.createElement("div");
  div.className = "error-banner";
  div.textContent = msg || "VoreVault — couldn't load settings, please reopen.";
  root.appendChild(div);
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
    case "UpToDate":
      statusText = `up to date · v${currentVersion}`;
      break;
    case "Checking":
      statusText = "checking…";
      break;
    case "DownloadingUpdate":
      statusText = `downloading v${value} in background`;
      break;
    case "Ready":
      statusText = `update v${value} ready — restart to apply`;
      break;
    case "Error":
      statusText = `couldn't check (${value}) · retry`;
      isError = true;
      break;
    default:
      statusText = `unknown state: ${kind}`;
      isError = true;
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

async function onCheckNow() {
  try {
    await tCore.invoke("updater_check_now");
  } catch (e) {
    console.error("updater_check_now failed:", e);
  }
}

async function onRestartNow() {
  try {
    await tCore.invoke("updater_install_and_restart");
  } catch (e) {
    console.error("updater_install_and_restart failed:", e);
  }
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
tEvent.listen("settings:state-changed", () => {
  loadAndRender();
});

tEvent.listen("updater:state-changed", () => {
  loadAndRender();
});

// Initial paint.
loadAndRender();
