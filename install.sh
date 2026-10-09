#!/usr/bin/env bash
# Installs the WiiM Player AppImage into the KDE application menu (no sudo).
# Usage: ./install.sh [path/to/WiiM Player_*.AppImage]
set -euo pipefail

cd "$(dirname "$0")"
SRC="${1:-}"
if [[ -z "$SRC" ]]; then
    SRC="$(ls -t src-tauri/target/release/bundle/appimage/*.AppImage 2>/dev/null | head -n1 || true)"
fi
if [[ -z "$SRC" || ! -f "$SRC" ]]; then
    echo "No AppImage found. Build one first:  npm run build" >&2
    exit 1
fi

BIN="$HOME/.local/bin"
APPS="$HOME/.local/share/applications"
ICONS="$HOME/.local/share/icons/hicolor"
mkdir -p "$BIN" "$APPS" "$ICONS/scalable/apps" "$ICONS/128x128/apps"

install -m 755 "$SRC" "$BIN/WiiM-Player.AppImage"
install -m 644 app-icon.svg "$ICONS/scalable/apps/wiimplayer.svg"
install -m 644 src-tauri/icons/128x128.png "$ICONS/128x128/apps/wiimplayer.png"

cat > "$APPS/wiimplayer.desktop" <<DESKTOP
[Desktop Entry]
Type=Application
Name=WiiM Player
GenericName=Music Player
Comment=Control your WiiM speaker and play local or DLNA music
Exec=$BIN/WiiM-Player.AppImage
Icon=wiimplayer
Terminal=false
Categories=AudioVideo;Audio;Player;
StartupWMClass=wiimplayer
DESKTOP

update-desktop-database "$APPS" 2>/dev/null || true
gtk-update-icon-cache -q -t "$ICONS" 2>/dev/null || true
(kbuildsycoca6 >/dev/null 2>&1 || kbuildsycoca5 >/dev/null 2>&1) || true

echo "Installed. 'WiiM Player' should appear in the launcher under Audio & Video."
case ":$PATH:" in *":$BIN:"*) ;; *) echo "Note: $BIN is not on your PATH." ;; esac
