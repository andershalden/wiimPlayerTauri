//! Local music library: folder scan, tag reading, embedded cover art.

use lofty::config::ParseOptions;
use lofty::file::{AudioFile, TaggedFileExt};
use lofty::probe::Probe;
use lofty::tag::Accessor;
use serde::Serialize;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

pub const AUDIO_EXT: &[&str] = &[
    "mp3", "flac", "ogg", "opus", "aac", "m4a", "wav", "aiff", "aif", "wma", "ape", "alac",
];

#[derive(Debug, Clone, Serialize)]
pub struct Track {
    pub id: u32,
    pub path: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub track_no: u32,
    pub duration: u32,
}

pub fn is_audio(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| AUDIO_EXT.contains(&e.to_lowercase().as_str()))
        .unwrap_or(false)
}

pub fn mime_for(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()).map(str::to_lowercase).as_deref() {
        Some("mp3") => "audio/mpeg",
        Some("flac") => "audio/flac",
        Some("ogg") => "audio/ogg",
        Some("opus") => "audio/ogg",
        Some("aac") => "audio/aac",
        Some("m4a") | Some("alac") => "audio/mp4",
        Some("wav") => "audio/wav",
        Some("aiff") | Some("aif") => "audio/aiff",
        Some("wma") => "audio/x-ms-wma",
        Some("ape") => "audio/x-ape",
        _ => "application/octet-stream",
    }
}

/// Fast first pass: walk the folder, no tag reading. Zero-byte files are
/// skipped (the renderer accepts the URI but has nothing to play).
pub fn scan_folder(folder: &Path) -> Vec<Track> {
    let mut paths: Vec<PathBuf> = WalkDir::new(folder)
        .follow_links(true)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_file() && is_audio(e.path()))
        .filter(|e| e.metadata().map(|m| m.len() > 0).unwrap_or(false))
        .map(|e| e.into_path())
        .collect();
    paths.sort();
    paths
        .into_iter()
        .enumerate()
        .map(|(i, p)| Track {
            id: i as u32,
            title: p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(),
            path: p.to_string_lossy().into_owned(),
            artist: String::new(),
            album: String::new(),
            track_no: 0,
            duration: 0,
        })
        .collect()
}

/// Second pass: read tags + duration (no cover art, for speed).
pub fn read_tags(path: &str) -> Option<(String, String, String, u32, u32)> {
    let tagged = Probe::open(path)
        .ok()?
        .options(ParseOptions::new().read_cover_art(false))
        .read()
        .ok()?;
    let duration = tagged.properties().duration().as_secs() as u32;
    let tag = tagged.primary_tag().or_else(|| tagged.first_tag());
    let (title, artist, album, no) = match tag {
        Some(t) => (
            t.title().map(|s| s.into_owned()).unwrap_or_default(),
            t.artist().map(|s| s.into_owned()).unwrap_or_default(),
            t.album().map(|s| s.into_owned()).unwrap_or_default(),
            t.track().unwrap_or(0),
        ),
        None => Default::default(),
    };
    Some((title, artist, album, no, duration))
}

/// Embedded cover art (bytes, mime type), if any.
pub fn read_cover(path: &str) -> Option<(Vec<u8>, String)> {
    let tagged = lofty::read_from_path(path).ok()?;
    let tag = tagged.primary_tag().or_else(|| tagged.first_tag())?;
    let pic = tag.pictures().first()?;
    let mime = pic.mime_type().map(|m| m.as_str().to_string()).unwrap_or_else(|| "image/jpeg".into());
    Some((pic.data().to_vec(), mime))
}
