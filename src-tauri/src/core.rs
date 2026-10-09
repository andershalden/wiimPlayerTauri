//! Application core: device connection + polling, queue, library, DLNA.
//! All shared state lives in one mutex that is never held across an `.await`.

use crate::dlna::{DlnaItem, DlnaServer};
use crate::fileserver::FileServer;
use crate::library::{self, Track};
use crate::ssdp;
use crate::wiim::{self, Device, DeviceInfo};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter};
use tokio::sync::Notify;

// ── Public data types ─────────────────────────────────────────────────────────

#[derive(Serialize, Deserialize, Clone, Default)]
pub struct Settings {
    pub music_folder: Option<String>,
    pub wiim_ip: Option<String>,
}

#[derive(Serialize, Clone)]
pub struct QueueItem {
    pub uid: u32,
    /// "local" or "dlna"
    pub kind: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub duration: u32,
    /// local file path, or the DLNA stream URI
    pub source: String,
    pub mime: String,
    pub file_id: Option<u32>,
    pub art: String,
}

#[derive(Serialize, Clone, Default)]
pub struct Playback {
    pub connected: bool,
    pub device_name: String,
    pub status: String,
    pub error: String,
    pub state: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub art: String,
    pub elapsed: u32,
    pub duration: u32,
    pub volume: u32,
    pub mute: bool,
    pub current: Option<usize>,
    pub repeat: String,
    pub shuffle: bool,
    pub file_port: u16,
    pub scanning: bool,
}

#[derive(Serialize, Clone)]
pub struct QueueView {
    pub items: Vec<QueueItem>,
    pub current: Option<usize>,
}

#[derive(Serialize, Clone)]
pub struct Snapshot {
    pub playback: Playback,
    pub queue: QueueView,
    pub library: Vec<Track>,
    pub settings: Settings,
}

#[derive(Deserialize)]
pub struct DlnaPick {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub uri: String,
    pub mime: String,
    pub album_art: String,
    pub duration: u32,
}

#[derive(Serialize, Clone)]
pub struct DlnaServerView {
    pub name: String,
    pub location: String,
}

// ── Internal state ────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq)]
enum Repeat {
    Off,
    All,
    One,
}

impl Repeat {
    fn name(self) -> &'static str {
        match self {
            Repeat::Off => "off",
            Repeat::All => "all",
            Repeat::One => "one",
        }
    }
}

struct Tracking {
    /// We started the current track from the queue; watch for end/failure.
    armed: bool,
    played: bool,
    stopped_streak: u32,
    last_elapsed: u32,
    last_duration: u32,
    /// Ignore device state until this instant (track switch in flight).
    hold_until: Instant,
    started: Instant,
}

struct Inner {
    settings: Settings,
    device: Option<Arc<Device>>,
    info: DeviceInfo,
    connected: bool,
    status: String,
    error: String,
    poll_failures: u32,
    library: Vec<Track>,
    scanning: bool,
    scan_gen: u32,
    queue: Vec<QueueItem>,
    current: Option<usize>,
    repeat: Repeat,
    shuffle: bool,
    next_uid: u32,
    dlna: Vec<Arc<DlnaServer>>,
    tracking: Tracking,
}

pub struct Core {
    app: AppHandle,
    http: reqwest::Client,
    files: Arc<FileServer>,
    inner: Mutex<Inner>,
    wake: Notify,
}

fn settings_path() -> PathBuf {
    dirs::config_dir().unwrap_or_else(|| PathBuf::from(".")).join("wiimplayer").join("settings.json")
}

fn local_ip_towards(host: &str) -> String {
    std::net::UdpSocket::bind("0.0.0.0:0")
        .and_then(|s| {
            s.connect((host, 80))?;
            s.local_addr()
        })
        .map(|a| a.ip().to_string())
        .unwrap_or_else(|_| "127.0.0.1".into())
}

impl Core {
    pub async fn new(app: AppHandle) -> Result<Arc<Core>, String> {
        let files = FileServer::start().await?;
        let settings: Settings = std::fs::read_to_string(settings_path())
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(3))
            .pool_max_idle_per_host(2)
            .build()
            .map_err(|e| e.to_string())?;
        Ok(Arc::new(Core {
            app,
            http,
            files,
            wake: Notify::new(),
            inner: Mutex::new(Inner {
                settings,
                device: None,
                info: DeviceInfo::default(),
                connected: false,
                status: "Starting…".into(),
                error: String::new(),
                poll_failures: 0,
                library: vec![],
                scanning: false,
                scan_gen: 0,
                queue: vec![],
                current: None,
                repeat: Repeat::Off,
                shuffle: false,
                next_uid: 1,
                dlna: vec![],
                tracking: Tracking {
                    armed: false,
                    played: false,
                    stopped_streak: 0,
                    last_elapsed: 0,
                    last_duration: 0,
                    hold_until: Instant::now(),
                    started: Instant::now(),
                },
            }),
        }))
    }

    /// Kick off background work. Returns immediately.
    pub fn start(self: &Arc<Self>) {
        let c = self.clone();
        tauri::async_runtime::spawn(async move { c.poll_loop().await });
        self.rescan_library();
    }

    // ── Snapshots / events ────────────────────────────────────────────────────

    fn playback_of(&self, s: &Inner) -> Playback {
        let cur = s.current.and_then(|i| s.queue.get(i));
        let from_queue = s.tracking.armed && cur.is_some();
        let (title, artist, album, art) = match cur {
            Some(q) if from_queue => {
                let art = match q.file_id {
                    Some(id) => self.files.art_url("127.0.0.1", id),
                    None => q.art.clone(),
                };
                (q.title.clone(), q.artist.clone(), q.album.clone(), art)
            }
            _ => (
                s.info.meta.title.clone(),
                s.info.meta.artist.clone(),
                s.info.meta.album.clone(),
                s.info.meta.album_art.clone(),
            ),
        };
        // Device-reported text wins when it has something (covers external control).
        let m = &s.info.meta;
        Playback {
            connected: s.connected,
            device_name: s.device.as_ref().map(|d| d.name.clone()).unwrap_or_default(),
            status: s.status.clone(),
            error: s.error.clone(),
            state: s.info.state.clone(),
            title: if m.title.is_empty() { title } else { m.title.clone() },
            artist: if m.artist.is_empty() { artist } else { m.artist.clone() },
            album: if m.album.is_empty() { album } else { m.album.clone() },
            art,
            elapsed: s.info.elapsed,
            duration: s.info.duration.max(if from_queue && s.info.duration == 0 { cur.map(|q| q.duration).unwrap_or(0) } else { 0 }),
            volume: s.info.volume,
            mute: s.info.mute,
            current: s.current,
            repeat: s.repeat.name().into(),
            shuffle: s.shuffle,
            file_port: self.files.port,
            scanning: s.scanning,
        }
    }

    fn emit_playback(&self) {
        let pb = self.playback_of(&self.inner.lock().unwrap());
        let _ = self.app.emit("playback", pb);
    }

    fn emit_queue(&self) {
        let q = {
            let s = self.inner.lock().unwrap();
            QueueView { items: s.queue.clone(), current: s.current }
        };
        let _ = self.app.emit("queue", q);
        self.emit_playback();
    }

    fn emit_library(&self) {
        let lib = self.inner.lock().unwrap().library.clone();
        let _ = self.app.emit("library", lib);
    }

    pub fn snapshot(&self) -> Snapshot {
        let s = self.inner.lock().unwrap();
        Snapshot {
            playback: self.playback_of(&s),
            queue: QueueView { items: s.queue.clone(), current: s.current },
            library: s.library.clone(),
            settings: s.settings.clone(),
        }
    }

    fn set_status(&self, msg: impl Into<String>) {
        self.inner.lock().unwrap().status = msg.into();
        self.emit_playback();
    }

    fn set_error(&self, msg: impl Into<String>) {
        self.inner.lock().unwrap().error = msg.into();
        self.emit_playback();
    }

    fn save_settings(&self) {
        let s = self.inner.lock().unwrap().settings.clone();
        let p = settings_path();
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(p, serde_json::to_string_pretty(&s).unwrap_or_default());
    }

    // ── Device discovery + polling ────────────────────────────────────────────

    async fn discover_device(&self) -> Option<Arc<Device>> {
        self.set_status("Looking for WiiM…");
        let locations = ssdp::search(ssdp::RENDERER, Duration::from_secs(3)).await;
        let probes = locations.iter().map(|l| Device::connect(&self.http, l));
        let mut found = futures_util::future::join_all(probes).await.into_iter().flatten().next();

        if found.is_none() {
            let ip = self.inner.lock().unwrap().settings.wiim_ip.clone();
            if let Some(ip) = ip.filter(|s| !s.trim().is_empty()) {
                let loc = format!("http://{}:49152/description.xml", ip.trim());
                found = Device::connect(&self.http, &loc).await.ok();
            }
        }
        found.map(Arc::new)
    }

    async fn poll_loop(self: Arc<Self>) {
        let mut last_discovery: Option<Instant> = None;
        loop {
            let dev = self.inner.lock().unwrap().device.clone();
            match dev {
                None => {
                    if last_discovery.map_or(true, |t| t.elapsed() > Duration::from_secs(8)) {
                        last_discovery = Some(Instant::now());
                        match self.discover_device().await {
                            Some(d) => {
                                {
                                    let mut s = self.inner.lock().unwrap();
                                    s.status = format!("Connected to {}", d.name);
                                    s.error.clear();
                                    s.connected = true;
                                    s.poll_failures = 0;
                                    s.device = Some(d);
                                }
                                self.emit_playback();
                                continue;
                            }
                            None => {
                                {
                                    let mut s = self.inner.lock().unwrap();
                                    s.connected = false;
                                    s.status = "No WiiM found — retrying…".into();
                                }
                                self.emit_playback();
                            }
                        }
                    }
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
                Some(d) => {
                    match d.info(&self.http).await {
                        Ok(info) => self.apply_info(info),
                        Err(e) => {
                            let lost = {
                                let mut s = self.inner.lock().unwrap();
                                s.poll_failures += 1;
                                if s.poll_failures >= 3 {
                                    s.connected = false;
                                    s.device = None;
                                    s.status = "Connection lost — searching…".into();
                                    s.error = format!("Lost contact with WiiM: {e}");
                                    true
                                } else {
                                    false
                                }
                            };
                            if lost {
                                last_discovery = None;
                                self.emit_playback();
                            }
                        }
                    }
                    // sleep until next tick, or sooner if a command asked for a refresh
                    let _ = tokio::time::timeout(Duration::from_secs(1), self.wake.notified()).await;
                }
            }
        }
    }

    fn apply_info(self: &Arc<Self>, info: DeviceInfo) {
        let mut advance = false;
        {
            let mut s = self.inner.lock().unwrap();
            s.poll_failures = 0;
            s.connected = true;
            let name = s.device.as_ref().map(|d| d.name.clone()).unwrap_or_default();
            s.status = format!("Connected to {name}");

            let now = Instant::now();
            let state = info.state.clone();
            if s.tracking.armed && now >= s.tracking.hold_until {
                match state.as_str() {
                    "PLAYING" | "TRANSITIONING" => {
                        s.tracking.played = true;
                        s.tracking.stopped_streak = 0;
                        if state == "PLAYING" {
                            s.tracking.last_elapsed = info.elapsed;
                            s.tracking.last_duration = info.duration;
                        }
                    }
                    "STOPPED" | "NO_MEDIA_PRESENT" => {
                        if s.tracking.played {
                            let t = &s.tracking;
                            if t.last_duration > 0 && t.last_elapsed + 6 >= t.last_duration {
                                advance = true;
                            }
                            s.tracking.armed = false;
                        } else if now.duration_since(s.tracking.started) > Duration::from_secs(3) {
                            // Not playing after several polls: the device rejected the stream.
                            s.tracking.stopped_streak += 1;
                            if s.tracking.stopped_streak >= 3 {
                                let src = s.current.and_then(|i| s.queue.get(i)).map(|q| q.source.clone()).unwrap_or_default();
                                let ext = std::path::Path::new(&src)
                                    .extension()
                                    .map(|e| e.to_string_lossy().to_uppercase())
                                    .unwrap_or_else(|| "This".into());
                                s.error = format!("{ext} is not playing — the speaker may not support this format");
                                s.tracking.armed = false;
                            }
                        }
                    }
                    _ => {}
                }
            }
            s.info = info;
        }
        self.emit_playback();
        if advance {
            let c = self.clone();
            tauri::async_runtime::spawn(async move {
                c.step(true, true).await;
            });
        }
    }

    pub fn refresh_devices(self: &Arc<Self>) {
        let mut s = self.inner.lock().unwrap();
        s.device = None;
        s.connected = false;
        s.status = "Looking for WiiM…".into();
        drop(s);
        self.emit_playback();
    }

    pub fn set_wiim_ip(self: &Arc<Self>, ip: String) {
        self.inner.lock().unwrap().settings.wiim_ip = Some(ip).filter(|s| !s.trim().is_empty());
        self.save_settings();
        self.refresh_devices();
    }

    fn device(&self) -> Result<Arc<Device>, String> {
        self.inner.lock().unwrap().device.clone().ok_or_else(|| "No WiiM connected".to_string())
    }

    async fn report<T>(&self, what: &str, r: Result<T, String>) -> Option<T> {
        match r {
            Ok(v) => Some(v),
            Err(e) => {
                self.set_error(format!("{what} failed: {e}"));
                None
            }
        }
    }

    // ── Transport commands ────────────────────────────────────────────────────

    pub async fn play_pause(self: &Arc<Self>) {
        let (state, cur_some) = {
            let s = self.inner.lock().unwrap();
            (s.info.state.clone(), s.current)
        };
        match state.as_str() {
            "PLAYING" | "TRANSITIONING" => self.simple("Pause", |d, h| Box::pin(async move { d.pause(&h).await })).await,
            "STOPPED" | "NO_MEDIA_PRESENT" | "UNKNOWN" | "" if cur_some.is_some() => {
                self.play_index(cur_some.unwrap()).await
            }
            _ => self.simple("Play", |d, h| Box::pin(async move { d.play(&h).await })).await,
        }
    }

    async fn simple<F>(&self, what: &str, f: F)
    where
        F: FnOnce(Arc<Device>, reqwest::Client) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>> + Send>>,
    {
        let Ok(d) = self.device().map_err(|e| self.set_error(e)) else { return };
        let r = f(d, self.http.clone()).await;
        if self.report(what, r).await.is_some() {
            self.inner.lock().unwrap().error.clear();
        }
        self.wake.notify_one();
    }

    pub async fn next(self: &Arc<Self>) {
        if self.inner.lock().unwrap().queue.is_empty() {
            self.simple("Next", |d, h| Box::pin(async move { d.next(&h).await })).await;
        } else {
            self.step(true, false).await;
        }
    }

    pub async fn previous(self: &Arc<Self>) {
        let (empty, elapsed) = {
            let s = self.inner.lock().unwrap();
            (s.queue.is_empty(), s.info.elapsed)
        };
        if empty {
            self.simple("Previous", |d, h| Box::pin(async move { d.previous(&h).await })).await;
        } else if elapsed > 3 {
            self.seek(0).await;
        } else {
            self.step(false, false).await;
        }
    }

    /// Move to the next/previous queue entry and play it.
    async fn step(self: &Arc<Self>, forward: bool, auto: bool) {
        let target = {
            let s = self.inner.lock().unwrap();
            let len = s.queue.len();
            if len == 0 {
                None
            } else {
                let cur = s.current.unwrap_or(len - 1);
                if forward {
                    if auto && s.repeat == Repeat::One {
                        Some(cur)
                    } else if s.shuffle && len > 1 {
                        let mut n = fastrand::usize(..len - 1);
                        if n >= cur { n += 1 }
                        Some(n)
                    } else if cur + 1 < len {
                        Some(cur + 1)
                    } else if s.repeat == Repeat::All || !auto {
                        Some(0)
                    } else {
                        None
                    }
                } else {
                    Some(if cur == 0 { len - 1 } else { cur - 1 })
                }
            }
        };
        match target {
            Some(i) => self.play_index(i).await,
            None => self.set_status("End of queue"),
        }
    }

    pub async fn seek(&self, secs: u32) {
        self.simple("Seek", move |d, h| Box::pin(async move { d.seek(&h, secs).await })).await;
    }

    pub async fn set_volume(&self, v: u32) {
        {
            self.inner.lock().unwrap().info.volume = v.min(100);
        }
        self.simple("Volume", move |d, h| Box::pin(async move { d.set_volume(&h, v).await })).await;
    }

    pub async fn set_mute(&self, m: bool) {
        self.inner.lock().unwrap().info.mute = m;
        self.simple("Mute", move |d, h| Box::pin(async move { d.set_mute(&h, m).await })).await;
    }

    pub fn cycle_repeat(&self) {
        {
            let mut s = self.inner.lock().unwrap();
            s.repeat = match s.repeat {
                Repeat::Off => Repeat::All,
                Repeat::All => Repeat::One,
                Repeat::One => Repeat::Off,
            };
        }
        self.emit_playback();
    }

    pub fn toggle_shuffle(&self) {
        {
            let mut s = self.inner.lock().unwrap();
            s.shuffle = !s.shuffle;
        }
        self.emit_playback();
    }

    // ── Queue ─────────────────────────────────────────────────────────────────

    pub async fn play_index(self: &Arc<Self>, i: usize) {
        let (item, dev) = {
            let mut s = self.inner.lock().unwrap();
            let Some(item) = s.queue.get(i).cloned() else { return };
            let Some(dev) = s.device.clone() else {
                s.error = "No WiiM connected".into();
                drop(s);
                self.emit_playback();
                return;
            };
            s.current = Some(i);
            s.error.clear();
            s.status = format!("Loading: {}", item.title);
            s.tracking = Tracking {
                armed: true,
                played: false,
                stopped_streak: 0,
                last_elapsed: 0,
                last_duration: 0,
                hold_until: Instant::now() + Duration::from_secs(15),
                started: Instant::now(),
            };
            (item, dev)
        };
        self.emit_queue();

        let ip = local_ip_towards(&dev.host);
        let (url, art) = match item.file_id {
            Some(id) => (self.files.file_url(&ip, id), Some(self.files.art_url(&ip, id))),
            None => (item.source.clone(), Some(item.art.clone()).filter(|a| !a.is_empty())),
        };
        let meta = wiim::build_didl(&url, &item.title, &item.artist, &item.album, &item.mime, art.as_deref());

        let r = async {
            dev.set_uri(&self.http, &url, &meta).await?;
            // The speaker needs a moment to accept the URI before Play (MP3 race).
            tokio::time::sleep(Duration::from_millis(700)).await;
            dev.play(&self.http).await
        }
        .await;

        match r {
            Ok(()) => {
                let mut s = self.inner.lock().unwrap();
                s.tracking.hold_until = Instant::now() + Duration::from_millis(2500);
                s.tracking.started = Instant::now();
            }
            Err(e) => {
                let mut s = self.inner.lock().unwrap();
                s.tracking.armed = false;
                s.error = format!("Playback failed: {e}");
            }
        }
        self.emit_playback();
        self.wake.notify_one();
    }

    fn new_item(s: &mut Inner, mut it: QueueItem) -> QueueItem {
        it.uid = s.next_uid;
        s.next_uid += 1;
        it
    }

    pub async fn queue_add_library(self: &Arc<Self>, ids: Vec<u32>, replace: bool, play: bool) {
        let first = {
            let mut s = self.inner.lock().unwrap();
            if replace {
                s.queue.clear();
                s.current = None;
            }
            let first = s.queue.len();
            for id in ids {
                let Some(t) = s.library.get(id as usize).cloned() else { continue };
                let p = PathBuf::from(&t.path);
                let fid = self.files.register(&t.path);
                let item = Self::new_item(
                    &mut s,
                    QueueItem {
                        uid: 0,
                        kind: "local".into(),
                        title: t.title,
                        artist: t.artist,
                        album: t.album,
                        duration: t.duration,
                        source: t.path,
                        mime: library::mime_for(&p).into(),
                        file_id: Some(fid),
                        art: String::new(),
                    },
                );
                s.queue.push(item);
            }
            if s.current.is_none() && !s.queue.is_empty() {
                s.current = Some(0);
            }
            first
        };
        self.emit_queue();
        if play {
            self.play_index(first).await;
        }
    }

    pub async fn queue_add_dlna(self: &Arc<Self>, picks: Vec<DlnaPick>, replace: bool, play: bool) {
        let first = {
            let mut s = self.inner.lock().unwrap();
            if replace {
                s.queue.clear();
                s.current = None;
            }
            let first = s.queue.len();
            for p in picks {
                let item = Self::new_item(
                    &mut s,
                    QueueItem {
                        uid: 0,
                        kind: "dlna".into(),
                        title: p.title,
                        artist: p.artist,
                        album: p.album,
                        duration: p.duration,
                        source: p.uri,
                        mime: if p.mime.is_empty() { "audio/mpeg".into() } else { p.mime },
                        file_id: None,
                        art: p.album_art,
                    },
                );
                s.queue.push(item);
            }
            if s.current.is_none() && !s.queue.is_empty() {
                s.current = Some(0);
            }
            first
        };
        self.emit_queue();
        if play {
            self.play_index(first).await;
        }
    }

    pub fn queue_remove(&self, index: usize) {
        {
            let mut s = self.inner.lock().unwrap();
            if index >= s.queue.len() {
                return;
            }
            s.queue.remove(index);
            s.current = match s.current {
                _ if s.queue.is_empty() => None,
                Some(c) if index < c => Some(c - 1),
                Some(c) if c >= s.queue.len() => Some(s.queue.len() - 1),
                other => other,
            };
        }
        self.emit_queue();
    }

    pub fn queue_clear(&self) {
        {
            let mut s = self.inner.lock().unwrap();
            s.queue.clear();
            s.current = None;
            s.tracking.armed = false;
        }
        self.emit_queue();
    }

    // ── Library ───────────────────────────────────────────────────────────────

    pub fn set_music_folder(self: &Arc<Self>, path: String) {
        self.inner.lock().unwrap().settings.music_folder = Some(path);
        self.save_settings();
        self.rescan_library();
    }

    pub fn rescan_library(self: &Arc<Self>) {
        let (gen, folder) = {
            let mut s = self.inner.lock().unwrap();
            s.scan_gen += 1;
            s.scanning = true;
            let folder = s
                .settings
                .music_folder
                .clone()
                .map(PathBuf::from)
                .or_else(dirs::audio_dir);
            (s.scan_gen, folder)
        };
        self.emit_playback();
        let c = self.clone();
        tauri::async_runtime::spawn(async move {
            let Some(folder) = folder else {
                c.finish_scan(gen);
                return;
            };
            let tracks = tokio::task::spawn_blocking(move || library::scan_folder(&folder)).await.unwrap_or_default();
            {
                let mut s = c.inner.lock().unwrap();
                if s.scan_gen != gen {
                    return;
                }
                s.library = tracks.clone();
            }
            c.emit_library();

            // Tag pass in small batches so the list fills in progressively.
            for chunk in tracks.chunks(40) {
                let paths: Vec<(usize, String)> = chunk.iter().map(|t| (t.id as usize, t.path.clone())).collect();
                let tags = tokio::task::spawn_blocking(move || {
                    paths.into_iter().map(|(i, p)| (i, library::read_tags(&p))).collect::<Vec<_>>()
                })
                .await
                .unwrap_or_default();
                {
                    let mut s = c.inner.lock().unwrap();
                    if s.scan_gen != gen {
                        return;
                    }
                    for (i, t) in tags {
                        if let (Some((title, artist, album, no, dur)), Some(tr)) = (t, s.library.get_mut(i)) {
                            if !title.is_empty() { tr.title = title }
                            tr.artist = artist;
                            tr.album = album;
                            tr.track_no = no;
                            tr.duration = dur;
                        }
                    }
                }
                c.emit_library();
            }
            c.finish_scan(gen);
        });
    }

    fn finish_scan(&self, gen: u32) {
        {
            let mut s = self.inner.lock().unwrap();
            if s.scan_gen != gen {
                return;
            }
            s.scanning = false;
        }
        self.emit_playback();
    }

    // ── DLNA ──────────────────────────────────────────────────────────────────

    pub async fn dlna_discover(&self) -> Vec<DlnaServerView> {
        let locations = ssdp::search(ssdp::SERVER, Duration::from_secs(3)).await;
        let probes = locations.iter().map(|l| DlnaServer::connect(&self.http, l));
        let mut servers: Vec<Arc<DlnaServer>> =
            futures_util::future::join_all(probes).await.into_iter().flatten().map(Arc::new).collect();
        servers.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        let views = servers.iter().map(|s| DlnaServerView { name: s.name.clone(), location: s.location.clone() }).collect();
        self.inner.lock().unwrap().dlna = servers;
        views
    }

    pub async fn dlna_browse(&self, server: usize, object_id: String) -> Result<Vec<DlnaItem>, String> {
        let srv = self.inner.lock().unwrap().dlna.get(server).cloned().ok_or("Unknown server")?;
        srv.browse(&self.http, &object_id).await
    }
}
