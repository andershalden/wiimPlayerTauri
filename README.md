# WiiM Player

A lightweight desktop remote and music player for a **WiiM** speaker on KDE/Linux.
Built with Tauri 2: a Rust backend and a small dark HTML/JS interface (no frontend framework, no bundler).

It replaces the earlier Qt/QML version. The backend does all the networking asynchronously, so the UI stays responsive.

## Features

- Automatic discovery of your WiiM on the local network (SSDP), with a manual IP fallback
- Play, pause, previous, next, seek, volume and mute
- **Local library:** scans a music folder in the background, reads tags, and streams files to the speaker (with HTTP Range support)
- **Queue:** search, remove, shuffle, repeat off/all/one, and automatic advance to the next track
- **DLNA / UPnP media servers:** browse folders and add tracks or whole folders to the queue
- Cover art, shown in the app and sent to the speaker
- Tray icon: click to show/hide, with play/pause/next/previous and Quit in the menu. Closing the window hides it to the tray.
- Reconnects automatically if the speaker drops off the network

Shuffle and repeat are handled by the app, not by the speaker's own loop mode, because the speaker only ever sees one stream at a time.

## Install (AppImage)

Build it (see below), then:

```bash
./install.sh
```

This copies the AppImage to `~/.local/bin/WiiM-Player.AppImage`, installs the icon, and adds a **WiiM Player** entry under *Audio & Video* in the KDE launcher. No `sudo` is needed. Pass a path to install a specific AppImage: `./install.sh path/to/file.AppImage`.

## Usage

1. Start WiiM Player from the launcher. The sidebar shows the connection status at the bottom.
2. **Library:** your music folder defaults to `~/Music`. Change it in **Settings**. Double-click a track to play it, use the **+** button to queue it, or use **Play all** and **Add all** for the current search results.
3. **Network:** pick a DLNA server, browse into folders, and add tracks.
4. **Queue:** click a track to play it. Shuffle and repeat are at the top.

If your speaker isn't found automatically, enter its IP address in **Settings** and press *Save & reconnect*.

Settings are stored in `~/.config/wiimplayer/settings.json`.

## Building from source

### Requirements

- Rust (install with [rustup](https://rustup.rs))
- Node.js and npm (only for the Tauri CLI)
- System packages (Debian/Ubuntu/Kubuntu):

```bash
sudo apt install build-essential pkg-config libwebkit2gtk-4.1-dev libgtk-3-dev \
    libsoup-3.0-dev libjavascriptcoregtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev
```

The AppImage bundler downloads `linuxdeploy` on first use, so it needs internet access.

### Commands

```bash
npm install          # once: installs the Tauri CLI
npm run dev          # run in development mode
npm run build        # release build + AppImage
```

The AppImage ends up in `src-tauri/target/release/bundle/appimage/`. It is about 83 MB because it bundles the web engine.

## How it works

```
 UI (src/)  ──invoke / events──  Rust backend (src-tauri/src/)
 index.html                       core.rs        state, queue, polling, auto-advance
 style.css                        wiim.rs        UPnP SOAP control of the WiiM
 app.js                           ssdp.rs        device discovery
                                  dlna.rs        ContentDirectory browsing (paginated)
                                  library.rs     folder scan, tags, cover art
                                  fileserver.rs  HTTP server the speaker pulls files from
                                  xml.rs         XML / DIDL-Lite helpers
                                  lib.rs         Tauri commands and tray
```

- The backend polls the speaker about once per second and pushes `playback`, `queue` and `library` events to the UI. The UI interpolates the progress bar between polls.
- To play a local file, the app registers it with its built-in HTTP server and tells the speaker to fetch `http://<your-LAN-IP>:<port>/f/<id>/<name>`. Only files that were queued can be fetched, and cover art is served from the same server.
- Everything is local: no accounts, no cloud, no telemetry.

## Troubleshooting

- **Speaker not found:** make sure the computer and speaker are on the same network and subnet, and that your firewall allows UDP multicast replies (SSDP). Otherwise use the IP fallback in Settings.
- **A track won't play on the speaker:** the app shows a "not playing" message when the speaker rejects a stream. Some formats (for example APE, WMA) may not be supported by the speaker.
- **Local files stop mid-way or never start:** the speaker has to reach your PC on the LAN. Check that a firewall isn't blocking incoming connections to this app.
- **No tray icon:** the tray needs `libayatana-appindicator3` at runtime (installed on standard KDE systems).
- A `libayatana-appindicator is deprecated` warning in the terminal is harmless.

## Status

Version 0.1: tested against a single WiiM speaker, a local library of a few hundred files and one DLNA server. Not yet implemented: drag-to-reorder in the queue, multiple speakers, and a mini player.
