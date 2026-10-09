//! Tiny HTTP server that lets the speaker pull local files (with Range
//! support via tower-http) and lets the UI fetch embedded cover art.
//! Only files that were explicitly registered are reachable.

use axum::{
    extract::{Path, Request, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Router,
};
use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tower::ServiceExt;
use tower_http::services::ServeFile;

use crate::library;

pub struct FileServer {
    pub port: u16,
    paths: Mutex<Vec<PathBuf>>,
    index: Mutex<HashMap<PathBuf, u32>>,
    art: Mutex<HashMap<u32, Option<(Vec<u8>, String)>>>,
}

impl FileServer {
    pub async fn start() -> Result<Arc<FileServer>, String> {
        let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.map_err(|e| e.to_string())?;
        let port = listener.local_addr().map_err(|e| e.to_string())?.port();
        let fs = Arc::new(FileServer {
            port,
            paths: Mutex::new(Vec::new()),
            index: Mutex::new(HashMap::new()),
            art: Mutex::new(HashMap::new()),
        });
        let app = Router::new()
            .route("/f/{id}/{name}", get(serve_file))
            .route("/art/{id}", get(serve_art))
            .with_state(fs.clone());
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        Ok(fs)
    }

    /// Register a file and return its id (stable for the session).
    pub fn register(&self, path: &str) -> u32 {
        let p = PathBuf::from(path);
        let mut index = self.index.lock().unwrap();
        if let Some(id) = index.get(&p) {
            return *id;
        }
        let mut paths = self.paths.lock().unwrap();
        let id = paths.len() as u32;
        paths.push(p.clone());
        index.insert(p, id);
        id
    }

    pub fn file_url(&self, ip: &str, id: u32) -> String {
        let path = self.paths.lock().unwrap()[id as usize].clone();
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        format!("http://{ip}:{}/f/{id}/{}", self.port, utf8_percent_encode(&name, NON_ALPHANUMERIC))
    }

    pub fn art_url(&self, ip: &str, id: u32) -> String {
        format!("http://{ip}:{}/art/{id}", self.port)
    }

    fn path_of(&self, id: u32) -> Option<PathBuf> {
        self.paths.lock().unwrap().get(id as usize).cloned()
    }
}

async fn serve_file(State(fs): State<Arc<FileServer>>, Path((id, _name)): Path<(u32, String)>, req: Request) -> Response {
    let Some(path) = fs.path_of(id) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let mime: mime::Mime = library::mime_for(&path).parse().unwrap_or(mime::APPLICATION_OCTET_STREAM);
    match ServeFile::new_with_mime(path, &mime).oneshot(req).await {
        Ok(res) => res.into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

async fn serve_art(State(fs): State<Arc<FileServer>>, Path(id): Path<u32>) -> Response {
    let cached = fs.art.lock().unwrap().get(&id).cloned();
    let entry = match cached {
        Some(e) => e,
        None => {
            let Some(path) = fs.path_of(id) else {
                return StatusCode::NOT_FOUND.into_response();
            };
            let p = path.to_string_lossy().into_owned();
            let e = tokio::task::spawn_blocking(move || library::read_cover(&p)).await.ok().flatten();
            let mut cache = fs.art.lock().unwrap();
            if cache.len() > 64 {
                cache.clear();
            }
            cache.insert(id, e.clone());
            e
        }
    };
    match entry {
        Some((bytes, mime)) => ([(header::CONTENT_TYPE, mime), (header::CACHE_CONTROL, "max-age=3600".into())], bytes).into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}
