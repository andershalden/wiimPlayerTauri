mod core;
mod dlna;
mod fileserver;
mod library;
mod ssdp;
mod wiim;
mod xml;

use crate::core::{Core, DlnaPick, DlnaServerView, Snapshot};
use crate::dlna::DlnaItem;
use std::sync::Arc;
use tauri::{
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Manager, State, WindowEvent,
};

type Cx<'a> = State<'a, Arc<Core>>;

#[tauri::command]
fn snapshot(core: Cx) -> Snapshot {
    core.snapshot()
}

#[tauri::command]
async fn play_pause(core: Cx<'_>) -> Result<(), String> {
    core.play_pause().await;
    Ok(())
}
#[tauri::command]
async fn next_track(core: Cx<'_>) -> Result<(), String> {
    core.next().await;
    Ok(())
}
#[tauri::command]
async fn previous_track(core: Cx<'_>) -> Result<(), String> {
    core.previous().await;
    Ok(())
}
#[tauri::command]
async fn seek(core: Cx<'_>, secs: u32) -> Result<(), String> {
    core.seek(secs).await;
    Ok(())
}
#[tauri::command]
async fn set_volume(core: Cx<'_>, volume: u32) -> Result<(), String> {
    core.set_volume(volume).await;
    Ok(())
}
#[tauri::command]
async fn set_mute(core: Cx<'_>, mute: bool) -> Result<(), String> {
    core.set_mute(mute).await;
    Ok(())
}
#[tauri::command]
fn cycle_repeat(core: Cx) {
    core.cycle_repeat();
}
#[tauri::command]
fn toggle_shuffle(core: Cx) {
    core.toggle_shuffle();
}

#[tauri::command]
async fn queue_play(core: Cx<'_>, index: usize) -> Result<(), String> {
    core.play_index(index).await;
    Ok(())
}
#[tauri::command]
fn queue_remove(core: Cx, index: usize) {
    core.queue_remove(index);
}
#[tauri::command]
fn queue_clear(core: Cx) {
    core.queue_clear();
}
#[tauri::command]
async fn queue_add_library(core: Cx<'_>, ids: Vec<u32>, replace: bool, play: bool) -> Result<(), String> {
    core.queue_add_library(ids, replace, play).await;
    Ok(())
}
#[tauri::command]
async fn queue_add_dlna(core: Cx<'_>, picks: Vec<DlnaPick>, replace: bool, play: bool) -> Result<(), String> {
    core.queue_add_dlna(picks, replace, play).await;
    Ok(())
}

#[tauri::command]
fn set_music_folder(core: Cx, path: String) {
    core.set_music_folder(path);
}
#[tauri::command]
fn rescan_library(core: Cx) {
    core.rescan_library();
}
#[tauri::command]
fn set_wiim_ip(core: Cx, ip: String) {
    core.set_wiim_ip(ip);
}
#[tauri::command]
fn rediscover_device(core: Cx) {
    core.refresh_devices();
}

#[tauri::command]
async fn dlna_discover(core: Cx<'_>) -> Result<Vec<DlnaServerView>, String> {
    Ok(core.dlna_discover().await)
}
#[tauri::command]
async fn dlna_browse(core: Cx<'_>, server: usize, object_id: String) -> Result<Vec<DlnaItem>, String> {
    core.dlna_browse(server, object_id).await
}

fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

fn toggle_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        if w.is_visible().unwrap_or(false) && w.is_focused().unwrap_or(false) {
            let _ = w.hide();
        } else {
            show_main(app);
        }
    }
}

fn build_tray(app: &AppHandle) -> tauri::Result<()> {
    let show = MenuItem::with_id(app, "show", "Show WiiM Player", true, None::<&str>)?;
    let prev = MenuItem::with_id(app, "prev", "Previous", true, None::<&str>)?;
    let pp = MenuItem::with_id(app, "pp", "Play / Pause", true, None::<&str>)?;
    let next = MenuItem::with_id(app, "next", "Next", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let sep1 = PredefinedMenuItem::separator(app)?;
    let sep2 = PredefinedMenuItem::separator(app)?;
    let menu = Menu::with_items(app, &[&show, &sep1, &prev, &pp, &next, &sep2, &quit])?;

    TrayIconBuilder::with_id("main")
        .icon(app.default_window_icon().cloned().expect("window icon"))
        .tooltip("WiiM Player")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, ev| {
            let core = app.state::<Arc<Core>>().inner().clone();
            match ev.id.as_ref() {
                "show" => show_main(app),
                "quit" => app.exit(0),
                "pp" => {
                    tauri::async_runtime::spawn(async move { core.play_pause().await });
                }
                "next" => {
                    tauri::async_runtime::spawn(async move { core.next().await });
                }
                "prev" => {
                    tauri::async_runtime::spawn(async move { core.previous().await });
                }
                _ => {}
            }
        })
        .on_tray_icon_event(|tray, ev| {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = ev {
                toggle_main(tray.app_handle());
            }
        })
        .build(app)?;
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let handle = app.handle().clone();
            // The core is built on the async runtime; block just long enough to bind the file server.
            let core = tauri::async_runtime::block_on(Core::new(handle.clone()))?;
            app.manage(core.clone());
            build_tray(&handle)?;
            core.start();
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.show();
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            // Closing the window hides it to the tray; use the tray menu to quit.
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .invoke_handler(tauri::generate_handler![
            snapshot,
            play_pause,
            next_track,
            previous_track,
            seek,
            set_volume,
            set_mute,
            cycle_repeat,
            toggle_shuffle,
            queue_play,
            queue_remove,
            queue_clear,
            queue_add_library,
            queue_add_dlna,
            set_music_folder,
            rescan_library,
            set_wiim_ip,
            rediscover_device,
            dlna_discover,
            dlna_browse,
        ])
        .run(tauri::generate_context!())
        .expect("error while running WiiM Player");
}
