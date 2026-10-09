"use strict";
const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const pickDir = (opts) => window.__TAURI__.dialog.open(opts);

const $ = (id) => document.getElementById(id);
const esc = (s) => String(s ?? "").replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]));
const fmt = (s) => { s = Math.max(0, Math.floor(s || 0)); return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`; };

const ICON = {
  play: '<svg viewBox="0 0 24 24"><path d="M8 5v14l11-7z"/></svg>',
  pause: '<svg viewBox="0 0 24 24"><path d="M6 5h4v14H6zm8 0h4v14h-4z"/></svg>',
  plus: '<svg viewBox="0 0 24 24"><path d="M19 13h-6v6h-2v-6H5v-2h6V5h2v6h6z"/></svg>',
  close: '<svg viewBox="0 0 24 24"><path d="M19 6.4 17.6 5 12 10.6 6.4 5 5 6.4 10.6 12 5 17.6 6.4 19 12 13.4 17.6 19 19 17.6 13.4 12z"/></svg>',
  folder: '<svg viewBox="0 0 24 24"><path d="M10 4H4a2 2 0 0 0-2 2v12a2 2 0 0 0 2 2h16a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-8z"/></svg>',
  shuffle: '<svg viewBox="0 0 24 24"><path d="M10.6 9.2 5.4 4 4 5.4l5.2 5.2zM14.5 4l2.0 2-12.5 12.6 1.4 1.4L17.9 7.4l2.1 2.1V4zm.5 9.4-1.4 1.4 3.3 3.3L14.9 20H20v-5.1l-2 2z"/></svg>',
  repeat: '<svg viewBox="0 0 24 24"><path d="M7 7h10v3l4-4-4-4v3H5v6h2zm10 10H7v-3l-4 4 4 4v-3h12v-6h-2z"/></svg>',
  volume: '<svg viewBox="0 0 24 24"><path d="M3 9v6h4l5 5V4L7 9zm13.5 3A4.5 4.5 0 0 0 14 8v8a4.5 4.5 0 0 0 2.5-4z"/></svg>',
  muted: '<svg viewBox="0 0 24 24"><path d="M16.5 12A4.5 4.5 0 0 0 14 8v2.2l2.5 2.5zM19 12c0 .9-.2 1.8-.6 2.5l1.5 1.5A9 9 0 0 0 21 12a9 9 0 0 0-7-8.8v2.1c2.9.9 5 3.6 5 6.7zM4.3 3 3 4.3 7.7 9H3v6h4l5 5v-6.7l4.3 4.3a8.8 8.8 0 0 1-2.3 1.4v2.1a11 11 0 0 0 3.9-2L19.7 21 21 19.7l-9-9zM12 4 9.9 6.1 12 8.2z"/></svg>',
};

const S = {
  pb: null, pbAt: 0,
  queue: { items: [], current: null },
  lib: [], sort: { key: "artist", dir: 1 }, libView: [],
  libMode: (() => { try { return localStorage.getItem("libMode") || "tracks"; } catch { return "tracks"; } })(),
  tree: null, folder: null,
  dlna: { servers: [], server: -1, stack: [], items: [] },
  seeking: false,
  artSrc: { now: "", bar: "" },
};

async function call(cmd, args) {
  try { return await invoke(cmd, args); }
  catch (e) { showError(String(e)); return undefined; }
}
function showError(msg) {
  const el = $("error");
  el.textContent = msg; el.hidden = !msg;
}

// ── navigation ────────────────────────────────────────────────────────────────
function showView(name) {
  document.querySelectorAll(".view").forEach((v) => v.classList.toggle("active", v.id === `view-${name}`));
  document.querySelectorAll("#nav button").forEach((b) => b.classList.toggle("active", b.dataset.view === name));
  if (name === "network" && S.dlna.servers.length === 0 && !S.dlna.searched) discoverDlna();
}
$("nav").addEventListener("click", (e) => { const b = e.target.closest("button[data-view]"); if (b) showView(b.dataset.view); });

// ── player bar / now playing ──────────────────────────────────────────────────
const STATE_LABEL = { PLAYING: "Playing", PAUSED_PLAYBACK: "Paused", STOPPED: "Stopped", TRANSITIONING: "Loading…", NO_MEDIA_PRESENT: "Idle" };
const isPlaying = (pb) => pb && (pb.state === "PLAYING" || pb.state === "TRANSITIONING");

function setArt(which, url) {
  if (S.artSrc[which] === url) return;
  S.artSrc[which] = url;
  const img = $(which === "now" ? "now-art" : "bar-art");
  img.hidden = true;
  if (!url) { img.removeAttribute("src"); return; }
  img.onload = () => { img.hidden = false; };
  img.onerror = () => { img.hidden = true; };
  img.src = url;
}
function setText(id, text) { const el = $(id); if (el.textContent !== text) el.textContent = text; }
function setFill(input) {
  const max = Number(input.max) || 1;
  input.style.setProperty("--fill", `${(Number(input.value) / max) * 100}%`);
}

function renderPlayback(pb) {
  const prevCur = S.pb?.current;
  S.pb = pb; S.pbAt = performance.now();

  $("conn-dot").classList.toggle("on", pb.connected);
  setText("conn-text", pb.connected ? pb.device_name || "Connected" : pb.status || "Not connected");
  setText("set-device", pb.connected ? `Connected to ${pb.device_name}` : pb.status || "");
  showError(pb.error);

  const title = pb.title || "Nothing playing";
  setText("now-title", title); setText("bar-title", title);
  setText("now-artist", pb.artist); setText("bar-artist", pb.artist);
  setText("now-album", pb.album);
  setText("now-state", STATE_LABEL[pb.state] || (pb.connected ? "Idle" : "Disconnected"));
  setArt("now", pb.art); setArt("bar", pb.art);

  $("btn-play").innerHTML = isPlaying(pb) ? ICON.pause : ICON.play;
  $("btn-mute").innerHTML = pb.mute || pb.volume === 0 ? ICON.muted : ICON.volume;
  if (document.activeElement !== $("volume")) { $("volume").value = pb.volume; setFill($("volume")); }
  setText("vol-val", String(pb.volume));

  $("seek").max = pb.duration || 0;
  tick();

  $("btn-shuffle").innerHTML = `${ICON.shuffle} Shuffle`;
  $("btn-shuffle").classList.toggle("on", pb.shuffle);
  $("btn-repeat").innerHTML = `${ICON.repeat} Repeat${pb.repeat === "one" ? " one" : pb.repeat === "all" ? " all" : ""}`;
  $("btn-repeat").classList.toggle("on", pb.repeat !== "off");
  setText("lib-status", pb.scanning ? "Scanning…" : "");

  if (prevCur !== pb.current) markCurrent();
}

// interpolate progress between 1 s polls
function tick() {
  const pb = S.pb; if (!pb || S.seeking) return;
  let el = pb.elapsed;
  if (pb.state === "PLAYING") el += (performance.now() - S.pbAt) / 1000;
  if (pb.duration) el = Math.min(el, pb.duration);
  $("seek").value = el; setFill($("seek"));
  setText("t-elapsed", fmt(el)); setText("t-total", fmt(pb.duration));
}
setInterval(tick, 250);

$("btn-play").onclick = () => call("play_pause");
$("btn-next").onclick = () => call("next_track");
$("btn-prev").onclick = () => call("previous_track");
$("btn-mute").onclick = () => call("set_mute", { mute: !(S.pb && S.pb.mute) });
$("btn-shuffle").onclick = () => call("toggle_shuffle");
$("btn-repeat").onclick = () => call("cycle_repeat");
$("btn-clear").onclick = () => call("queue_clear");

const seekEl = $("seek");
seekEl.addEventListener("input", () => { S.seeking = true; setFill(seekEl); setText("t-elapsed", fmt(seekEl.value)); });
seekEl.addEventListener("change", async () => { await call("seek", { secs: Math.floor(seekEl.value) }); S.seeking = false; if (S.pb) { S.pb.elapsed = Number(seekEl.value); S.pbAt = performance.now(); } });
let volTimer = 0;
$("volume").addEventListener("input", (e) => {
  setFill(e.target); setText("vol-val", e.target.value);
  clearTimeout(volTimer);
  volTimer = setTimeout(() => call("set_volume", { volume: Number(e.target.value) }), 80);
});

// ── library ───────────────────────────────────────────────────────────────────
const LIB_CAP = 1000;
const norm = (s) => (s || "").toLowerCase();

function computeLibView() {
  const q = norm($("lib-filter").value).trim();
  let v = S.lib;
  if (q) v = v.filter((t) => norm(t.title).includes(q) || norm(t.artist).includes(q) || norm(t.album).includes(q));
  const { key, dir } = S.sort;
  const cmp = key === "duration"
    ? (a, b) => (a.duration - b.duration) * dir
    : (a, b) => (norm(a[key]).localeCompare(norm(b[key])) || norm(a.album).localeCompare(norm(b.album)) || a.track_no - b.track_no || norm(a.title).localeCompare(norm(b.title))) * dir;
  S.libView = v.slice().sort(cmp);
}

const trackRowHtml = (t, n) => `
    <div class="row" data-id="${t.id}">
      <span class="c-n">${n}</span>
      <span class="c-title" title="${esc(t.title)}">${esc(t.title)}</span>
      <span class="c-artist">${esc(t.artist)}</span>
      <span class="c-album">${esc(t.album)}</span>
      <span class="c-dur">${t.duration ? fmt(t.duration) : ""}</span>
      <span class="c-act"><button data-act="play" title="Play now">${ICON.play}</button><button data-act="add" title="Add to queue">${ICON.plus}</button></span>
    </div>`;

function renderLibrary() {
  $("lib-count").textContent = S.lib.length ? String(S.lib.length) : "";
  const folders = S.libMode === "folders";
  $("lib-head").hidden = folders;
  $("lib-crumbs").hidden = !folders;
  document.querySelectorAll("#lib-mode button").forEach((b) => b.classList.toggle("on", b.dataset.mode === S.libMode));
  if (folders) return renderFolders();

  computeLibView();
  document.querySelectorAll("#lib-head [data-sort]").forEach((b) => b.classList.toggle("sorted", b.dataset.sort === S.sort.key));
  const list = $("lib-list");
  if (!S.lib.length) {
    list.innerHTML = `<div class="empty">${S.pb?.scanning ? "Scanning your music folder…" : "No music found. Choose a folder in Settings."}</div>`;
    return;
  }
  const shown = S.libView.slice(0, LIB_CAP);
  list.innerHTML = shown.map((t, i) => trackRowHtml(t, i + 1)).join("") +
    (S.libView.length > LIB_CAP ? `<div class="note">Showing first ${LIB_CAP} of ${S.libView.length} — refine your search.</div>` : "") +
    (shown.length === 0 ? `<div class="empty">No matches.</div>` : "");
}

// ── folder view ──
const dirOf = (p) => p.slice(0, p.lastIndexOf("/")) || "/";
const byName = (a, b) => a.name.localeCompare(b.name, undefined, { numeric: true, sensitivity: "base" });

function buildTree() {
  const dirs = new Map();
  const node = (p) => {
    let d = dirs.get(p);
    if (!d) { d = { path: p, name: p.slice(p.lastIndexOf("/") + 1) || "/", subdirs: new Set(), tracks: [], count: 0 }; dirs.set(p, d); }
    return d;
  };
  let root = null;
  for (const t of S.lib) {
    const d = dirOf(t.path);
    if (root === null) root = d;
    else while (root !== "/" && d !== root && !d.startsWith(root + "/")) root = dirOf(root);
  }
  if (root === null) return { root: "/", dirs };
  for (const t of S.lib) {
    const d = dirOf(t.path);
    node(d).tracks.push(t);
    for (let cur = d; ; ) {
      node(cur).count++;
      if (cur === root) break;
      const parent = dirOf(cur);
      node(parent).subdirs.add(cur);
      cur = parent;
    }
  }
  return { root, dirs };
}

// album order: by track number when every file has one, otherwise by file name
function ordered(tracks) {
  const t = tracks.slice();
  const numbered = t.length > 0 && t.every((x) => x.track_no > 0);
  return t.sort((a, b) => (numbered ? a.track_no - b.track_no : 0) || a.path.localeCompare(b.path, undefined, { numeric: true }));
}
function collect(path) {
  const out = [];
  (function walk(p) {
    const n = S.tree.dirs.get(p); if (!n) return;
    out.push(...ordered(n.tracks));
    [...n.subdirs].map((d) => S.tree.dirs.get(d)).sort(byName).forEach((d) => walk(d.path));
  })(path);
  return out;
}
const folderTracks = () => (S.tree && S.tree.dirs.has(S.folder) ? collect(S.folder) : []);

function renderFolders() {
  const list = $("lib-list");
  if (!S.lib.length) {
    $("lib-crumbs").innerHTML = "";
    list.innerHTML = `<div class="empty">${S.pb?.scanning ? "Scanning your music folder…" : "No music found. Choose a folder in Settings."}</div>`;
    return;
  }
  if (!S.tree) S.tree = buildTree();
  const { root, dirs } = S.tree;
  if (!S.folder || !dirs.has(S.folder)) S.folder = root;

  const trail = [];
  for (let p = S.folder; ; p = dirOf(p)) { trail.unshift(p); if (p === root || p === "/") break; }
  $("lib-crumbs").innerHTML = trail.map((p, i) =>
    `<button data-path="${esc(p)}">${esc(dirs.get(p)?.name || p)}</button>${i < trail.length - 1 ? "<span>/</span>" : ""}`).join("");

  const cur = dirs.get(S.folder);
  const q = norm($("lib-filter").value).trim();
  const subs = [...cur.subdirs].map((p) => dirs.get(p)).sort(byName).filter((d) => !q || norm(d.name).includes(q));
  const tracks = ordered(cur.tracks).filter((t) => !q || norm(t.title).includes(q) || norm(t.artist).includes(q) || norm(t.album).includes(q));

  const dirRows = subs.map((d) => `
    <div class="row dir" data-dir="${esc(d.path)}">
      <span class="c-n">${ICON.folder}</span>
      <span class="c-title" title="${esc(d.name)}">${esc(d.name)}</span>
      <span class="c-artist">${d.count} track${d.count === 1 ? "" : "s"}</span>
      <span class="c-album"></span><span class="c-dur"></span>
      <span class="c-act"><button data-act="play" title="Play folder now">${ICON.play}</button><button data-act="add" title="Add folder to queue">${ICON.plus}</button></span>
    </div>`).join("");
  const shown = tracks.slice(0, LIB_CAP);
  list.innerHTML = dirRows + shown.map((t, i) => trackRowHtml(t, i + 1)).join("") +
    (tracks.length > LIB_CAP ? `<div class="note">Showing first ${LIB_CAP} of ${tracks.length} tracks.</div>` : "") +
    (!subs.length && !shown.length ? `<div class="empty">${q ? "No matches." : "This folder is empty."}</div>` : "");
}

function setLibMode(mode) {
  S.libMode = mode;
  try { localStorage.setItem("libMode", mode); } catch {}
  renderLibrary();
}
$("lib-mode").addEventListener("click", (e) => { const b = e.target.closest("button[data-mode]"); if (b) setLibMode(b.dataset.mode); });
$("lib-crumbs").addEventListener("click", (e) => {
  const b = e.target.closest("button[data-path]"); if (!b) return;
  S.folder = b.dataset.path; $("lib-filter").value = ""; renderLibrary();
});

$("lib-list").addEventListener("click", (e) => {
  const row = e.target.closest(".row"); if (!row) return;
  if (row.dataset.dir !== undefined) {
    const act = e.target.closest("button")?.dataset.act;
    if (!act) { S.folder = row.dataset.dir; $("lib-filter").value = ""; renderLibrary(); $("lib-list").scrollTop = 0; return; }
    const ids = collect(row.dataset.dir).map((t) => t.id);
    if (ids.length) call("queue_add_library", { ids, replace: false, play: act === "play" });
    return;
  }
  const id = Number(row.dataset.id);
  const act = e.target.closest("button")?.dataset.act;
  if (act === "add") call("queue_add_library", { ids: [id], replace: false, play: false });
  else if (act === "play") call("queue_add_library", { ids: [id], replace: false, play: true });
});
$("lib-list").addEventListener("dblclick", (e) => {
  const row = e.target.closest(".row"); if (!row || row.dataset.dir !== undefined || e.target.closest("button")) return;
  call("queue_add_library", { ids: [Number(row.dataset.id)], replace: false, play: true });
});
$("lib-head").addEventListener("click", (e) => {
  const b = e.target.closest("[data-sort]"); if (!b) return;
  const key = b.dataset.sort;
  S.sort = { key, dir: S.sort.key === key ? -S.sort.dir : 1 };
  renderLibrary();
});
let libTimer = 0;
$("lib-filter").addEventListener("input", () => { clearTimeout(libTimer); libTimer = setTimeout(renderLibrary, 120); });
const libIds = () => (S.libMode === "folders" ? folderTracks() : S.libView).map((t) => t.id);
$("lib-addall").onclick = () => { const ids = libIds(); if (ids.length) call("queue_add_library", { ids, replace: false, play: false }); };
$("lib-playall").onclick = () => { const ids = libIds(); if (ids.length) call("queue_add_library", { ids, replace: true, play: true }); };

// ── queue ─────────────────────────────────────────────────────────────────────
function renderQueue() {
  const { items, current } = S.queue;
  $("queue-count").textContent = items.length ? String(items.length) : "";
  const q = norm($("queue-filter").value).trim();
  const list = $("queue-list");
  if (!items.length) { list.innerHTML = `<div class="empty">The queue is empty. Add songs from Library or Network.</div>`; return; }
  const rows = [];
  items.forEach((t, i) => {
    if (q && !(norm(t.title).includes(q) || norm(t.artist).includes(q) || norm(t.album).includes(q))) return;
    rows.push(`
    <div class="row queue-row${i === current ? " current" : ""}" data-i="${i}">
      <span class="c-n">${i + 1}</span>
      <span class="c-title" title="${esc(t.title)}">${esc(t.title)}</span>
      <span class="c-artist">${esc(t.artist)}</span>
      <span class="c-album">${esc(t.album)}</span>
      <span class="c-dur">${t.duration ? fmt(t.duration) : ""}</span>
      <span class="c-act"><button data-act="remove" title="Remove">${ICON.close}</button></span>
    </div>`);
  });
  list.innerHTML = rows.join("") || `<div class="empty">No matches.</div>`;
}
function markCurrent() {
  const cur = S.pb?.current ?? S.queue.current;
  document.querySelectorAll("#queue-list .row").forEach((r) => r.classList.toggle("current", Number(r.dataset.i) === cur));
}
$("queue-list").addEventListener("click", (e) => {
  const row = e.target.closest(".row"); if (!row) return;
  const i = Number(row.dataset.i);
  if (e.target.closest("button[data-act=remove]")) call("queue_remove", { index: i });
  else call("queue_play", { index: i });
});
let qTimer = 0;
$("queue-filter").addEventListener("input", () => { clearTimeout(qTimer); qTimer = setTimeout(renderQueue, 120); });

// ── network (DLNA) ────────────────────────────────────────────────────────────
async function discoverDlna() {
  const sel = $("dlna-server");
  S.dlna.searched = true;
  sel.innerHTML = `<option>Searching…</option>`;
  const servers = (await call("dlna_discover")) || [];
  S.dlna.servers = servers;
  sel.innerHTML = servers.length
    ? servers.map((s, i) => `<option value="${i}">${esc(s.name)}</option>`).join("")
    : `<option value="-1">No media servers found</option>`;
  if (servers.length) openServer(0); else { S.dlna.items = []; S.dlna.stack = []; renderDlna(); }
}
async function openServer(i) {
  S.dlna.server = i;
  S.dlna.stack = [{ id: "0", title: S.dlna.servers[i].name }];
  await browse();
}
async function browse() {
  const top = S.dlna.stack[S.dlna.stack.length - 1];
  $("dlna-list").innerHTML = `<div class="empty">Loading…</div>`;
  const items = await call("dlna_browse", { server: S.dlna.server, objectId: top.id });
  S.dlna.items = items || [];
  $("dlna-filter").value = "";
  renderDlna();
}
function dlnaView() {
  const q = norm($("dlna-filter").value).trim();
  return S.dlna.items.filter((it) => !q || norm(it.title).includes(q) || norm(it.artist).includes(q));
}
function renderDlna() {
  $("dlna-crumbs").innerHTML = S.dlna.stack.map((c, i) => `<button data-i="${i}">${esc(c.title)}</button>${i < S.dlna.stack.length - 1 ? "<span>/</span>" : ""}`).join("");
  const view = dlnaView();
  const list = $("dlna-list");
  if (!view.length) { list.innerHTML = `<div class="empty">${S.dlna.servers.length ? "Nothing here." : "No DLNA media server found on the network."}</div>`; return; }
  list.innerHTML = view.map((it) => it.is_container
    ? `<div class="row folder" data-id="${esc(it.id)}"><span class="c-n">${ICON.folder}</span><span class="c-title">${esc(it.title)}</span><span class="c-dur">${it.child_count || ""}</span></div>`
    : `<div class="row" data-id="${esc(it.id)}">
        <span class="c-n"></span><span class="c-title" title="${esc(it.title)}">${esc(it.title)}</span>
        <span class="c-artist">${esc(it.artist)}</span><span class="c-album">${esc(it.album)}</span>
        <span class="c-dur">${it.duration ? fmt(it.duration) : ""}</span>
        <span class="c-act"><button data-act="play" title="Play now">${ICON.play}</button><button data-act="add" title="Add to queue">${ICON.plus}</button></span>
      </div>`).join("");
}
const pick = (it) => ({ title: it.title, artist: it.artist, album: it.album, uri: it.uri, mime: it.mime, album_art: it.album_art, duration: it.duration });
const dlnaTracks = () => dlnaView().filter((it) => !it.is_container);

$("dlna-list").addEventListener("click", (e) => {
  const row = e.target.closest(".row"); if (!row) return;
  const it = S.dlna.items.find((x) => x.id === row.dataset.id); if (!it) return;
  if (it.is_container) { S.dlna.stack.push({ id: it.id, title: it.title }); browse(); return; }
  const act = e.target.closest("button")?.dataset.act;
  if (act === "add") call("queue_add_dlna", { picks: [pick(it)], replace: false, play: false });
  else if (act === "play") call("queue_add_dlna", { picks: [pick(it)], replace: false, play: true });
});
$("dlna-list").addEventListener("dblclick", (e) => {
  const row = e.target.closest(".row"); if (!row || e.target.closest("button")) return;
  const it = S.dlna.items.find((x) => x.id === row.dataset.id);
  if (it && !it.is_container) call("queue_add_dlna", { picks: [pick(it)], replace: false, play: true });
});
$("dlna-crumbs").addEventListener("click", (e) => {
  const b = e.target.closest("button[data-i]"); if (!b) return;
  S.dlna.stack = S.dlna.stack.slice(0, Number(b.dataset.i) + 1); browse();
});
$("dlna-server").addEventListener("change", (e) => { const i = Number(e.target.value); if (i >= 0) openServer(i); });
$("dlna-refresh").onclick = discoverDlna;
let dTimer = 0;
$("dlna-filter").addEventListener("input", () => { clearTimeout(dTimer); dTimer = setTimeout(renderDlna, 120); });
$("dlna-addall").onclick = () => { const p = dlnaTracks().map(pick); if (p.length) call("queue_add_dlna", { picks: p, replace: false, play: false }); };
$("dlna-playall").onclick = () => { const p = dlnaTracks().map(pick); if (p.length) call("queue_add_dlna", { picks: p, replace: true, play: true }); };

// ── settings ──────────────────────────────────────────────────────────────────
function renderSettings(s) {
  $("set-folder").value = s.music_folder || "(default: ~/Music)";
  if (document.activeElement !== $("set-ip")) $("set-ip").value = s.wiim_ip || "";
}
$("set-choose").onclick = async () => {
  const dir = await pickDir({ directory: true, multiple: false, title: "Choose your music folder" });
  if (dir) { await call("set_music_folder", { path: dir }); $("set-folder").value = dir; showView("library"); }
};
$("set-rescan").onclick = () => call("rescan_library");
$("set-ip-save").onclick = () => call("set_wiim_ip", { ip: $("set-ip").value.trim() });
$("set-rediscover").onclick = () => call("rediscover_device");

// ── boot ──────────────────────────────────────────────────────────────────────
(async function init() {
  await listen("playback", (e) => renderPlayback(e.payload));
  await listen("queue", (e) => { S.queue = e.payload; renderQueue(); });
  await listen("library", (e) => { S.lib = e.payload; S.tree = null; renderLibrary(); });
  const snap = await call("snapshot");
  if (!snap) return;
  S.queue = snap.queue; S.lib = snap.library;
  renderSettings(snap.settings);
  renderPlayback(snap.playback);
  renderQueue(); renderLibrary();
})();
