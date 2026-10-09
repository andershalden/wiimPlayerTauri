//! WiiM / LinkPlay control over UPnP SOAP (AVTransport + RenderingControl).

use crate::xml::{self, escape, flatten_soap, format_hms, parse_didl, parse_hms};
use serde::Serialize;
use std::collections::HashMap;
use std::time::Duration;

const AVT: &str = "urn:schemas-upnp-org:service:AVTransport:1";
const RC: &str = "urn:schemas-upnp-org:service:RenderingControl:1";

#[derive(Debug, Clone, Default, Serialize)]
pub struct TrackMeta {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub album_art: String,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct DeviceInfo {
    /// PLAYING, PAUSED_PLAYBACK, STOPPED, TRANSITIONING, NO_MEDIA_PRESENT …
    pub state: String,
    pub elapsed: u32,
    pub duration: u32,
    pub volume: u32,
    pub mute: bool,
    pub meta: TrackMeta,
}

#[derive(Debug, Clone, Serialize)]
pub struct Device {
    pub location: String,
    pub name: String,
    pub model: String,
    pub udn: String,
    #[serde(skip)]
    pub host: String,
    #[serde(skip)]
    av_transport: String,
    #[serde(skip)]
    rendering_control: String,
}

fn combine(base: &str, rel: &str) -> String {
    if rel.to_ascii_lowercase().starts_with("http") {
        return rel.to_string();
    }
    url::Url::parse(base)
        .and_then(|b| b.join(rel))
        .map(|u| u.to_string())
        .unwrap_or_else(|_| rel.to_string())
}

impl Device {
    /// Fetch and parse the UPnP description at `location`.
    pub async fn connect(http: &reqwest::Client, location: &str) -> Result<Device, String> {
        let body = http
            .get(location)
            .timeout(Duration::from_secs(4))
            .send()
            .await
            .and_then(|r| r.error_for_status())
            .map_err(|e| format!("description fetch failed: {e}"))?
            .text()
            .await
            .map_err(|e| e.to_string())?;
        let d = xml::parse_device_description(&body);
        let base = if d.url_base.is_empty() { location.to_string() } else { d.url_base.clone() };

        let mut av = String::new();
        let mut rc = String::new();
        for (ty, ctl) in &d.services {
            if ty == AVT {
                av = combine(&base, ctl);
            } else if ty == RC {
                rc = combine(&base, ctl);
            }
        }
        if av.is_empty() || rc.is_empty() {
            return Err("device lacks AVTransport/RenderingControl".into());
        }
        let host = url::Url::parse(location)
            .ok()
            .and_then(|u| u.host_str().map(str::to_string))
            .unwrap_or_default();
        Ok(Device {
            location: location.to_string(),
            name: if d.friendly_name.is_empty() { "WiiM".into() } else { d.friendly_name.clone() },
            model: d.model_name.clone(),
            udn: if d.udn.is_empty() { location.to_string() } else { d.udn.clone() },
            host,
            av_transport: av,
            rendering_control: rc,
        })
        .and_then(|dev| {
            let id = format!("{} {} {}", d.friendly_name, d.model_name, d.model_description)
                .to_lowercase();
            if id.contains("wiim") || id.contains("linkplay") {
                Ok(dev)
            } else {
                Err("not a WiiM device".into())
            }
        })
    }

    async fn soap(
        &self,
        http: &reqwest::Client,
        url: &str,
        urn: &str,
        action: &str,
        args: &[(&str, String)],
        raw: &[&str],
    ) -> Result<String, String> {
        let mut inner = String::new();
        for (k, v) in args {
            let val = if raw.contains(k) { v.clone() } else { escape(v) };
            inner.push_str(&format!("<{k}>{val}</{k}>"));
        }
        let envelope = format!(
            "<?xml version=\"1.0\" encoding=\"utf-8\"?>\
             <s:Envelope xmlns:s=\"http://schemas.xmlsoap.org/soap/envelope/\" \
             s:encodingStyle=\"http://schemas.xmlsoap.org/soap/encoding/\">\
             <s:Body><u:{action} xmlns:u=\"{urn}\">{inner}</u:{action}></s:Body></s:Envelope>"
        );
        let resp = http
            .post(url)
            .timeout(Duration::from_secs(5))
            .header("Content-Type", "text/xml; charset=\"utf-8\"")
            .header("SOAPACTION", format!("\"{urn}#{action}\""))
            .body(envelope)
            .send()
            .await
            .map_err(|e| format!("{action}: {e}"))?;
        let status = resp.status();
        let text = resp.text().await.map_err(|e| format!("{action}: {e}"))?;
        if !status.is_success() {
            return Err(format!("{action}: HTTP {status}"));
        }
        Ok(text)
    }

    async fn avt(&self, http: &reqwest::Client, action: &str, args: &[(&str, String)]) -> Result<HashMap<String, String>, String> {
        let mut a = vec![("InstanceID", "0".to_string())];
        a.extend_from_slice(args);
        Ok(flatten_soap(&self.soap(http, &self.av_transport, AVT, action, &a, &[]).await?))
    }

    async fn rc(&self, http: &reqwest::Client, action: &str, args: &[(&str, String)]) -> Result<HashMap<String, String>, String> {
        let mut a = vec![("InstanceID", "0".to_string()), ("Channel", "Master".to_string())];
        a.extend_from_slice(args);
        Ok(flatten_soap(&self.soap(http, &self.rendering_control, RC, action, &a, &[]).await?))
    }

    /// One poll: transport state, position/metadata, volume, mute — in parallel.
    /// Fails only if *every* call fails (device unreachable).
    pub async fn info(&self, http: &reqwest::Client) -> Result<DeviceInfo, String> {
        let (t, p, v, m) = tokio::join!(
            self.avt(http, "GetTransportInfo", &[]),
            self.avt(http, "GetPositionInfo", &[]),
            self.rc(http, "GetVolume", &[]),
            self.rc(http, "GetMute", &[]),
        );
        if t.is_err() && p.is_err() && v.is_err() && m.is_err() {
            return Err(t.unwrap_err());
        }
        let mut info = DeviceInfo { state: "UNKNOWN".into(), ..Default::default() };
        if let Ok(t) = t {
            info.state = t.get("CurrentTransportState").cloned().unwrap_or_else(|| "UNKNOWN".into());
        }
        if let Ok(p) = p {
            info.duration = parse_hms(p.get("TrackDuration").map(String::as_str).unwrap_or(""));
            info.elapsed = parse_hms(p.get("RelTime").map(String::as_str).unwrap_or(""));
            info.meta = parse_meta(p.get("TrackMetaData").map(String::as_str).unwrap_or(""));
        }
        if let Ok(v) = v {
            info.volume = v.get("CurrentVolume").and_then(|s| s.parse().ok()).unwrap_or(0);
        }
        if let Ok(m) = m {
            let s = m.get("CurrentMute").map(|s| s.to_lowercase()).unwrap_or_default();
            info.mute = s == "1" || s == "true";
        }
        Ok(info)
    }

    pub async fn play(&self, http: &reqwest::Client) -> Result<(), String> {
        self.avt(http, "Play", &[("Speed", "1".into())]).await.map(|_| ())
    }
    pub async fn pause(&self, http: &reqwest::Client) -> Result<(), String> {
        self.avt(http, "Pause", &[]).await.map(|_| ())
    }
    pub async fn previous(&self, http: &reqwest::Client) -> Result<(), String> {
        self.avt(http, "Previous", &[]).await.map(|_| ())
    }
    pub async fn next(&self, http: &reqwest::Client) -> Result<(), String> {
        self.avt(http, "Next", &[]).await.map(|_| ())
    }
    pub async fn seek(&self, http: &reqwest::Client, secs: u32) -> Result<(), String> {
        self.avt(http, "Seek", &[("Unit", "REL_TIME".into()), ("Target", format_hms(secs))])
            .await
            .map(|_| ())
    }
    pub async fn set_volume(&self, http: &reqwest::Client, v: u32) -> Result<(), String> {
        self.rc(http, "SetVolume", &[("DesiredVolume", v.min(100).to_string())]).await.map(|_| ())
    }
    pub async fn set_mute(&self, http: &reqwest::Client, mute: bool) -> Result<(), String> {
        self.rc(http, "SetMute", &[("DesiredMute", if mute { "1" } else { "0" }.into())])
            .await
            .map(|_| ())
    }

    /// `metadata` is DIDL-Lite XML (unescaped); it is escaped once here.
    pub async fn set_uri(&self, http: &reqwest::Client, uri: &str, metadata: &str) -> Result<(), String> {
        let args = [
            ("InstanceID", "0".to_string()),
            ("CurrentURI", uri.to_string()),
            ("CurrentURIMetaData", escape(metadata)),
        ];
        self.soap(http, &self.av_transport, AVT, "SetAVTransportURI", &args, &["CurrentURIMetaData"])
            .await
            .map(|_| ())
    }
}

fn parse_meta(raw: &str) -> TrackMeta {
    let raw = raw.trim();
    if raw.is_empty() || raw == "NOT_IMPLEMENTED" {
        return TrackMeta::default();
    }
    let didl = xml::unescape_again(raw);
    parse_didl(&didl)
        .into_iter()
        .next()
        .map(|e| TrackMeta { title: e.title, artist: e.artist, album: e.album, album_art: e.album_art })
        .unwrap_or_default()
}

/// DIDL-Lite for a single track, as plain (unescaped) XML.
pub fn build_didl(uri: &str, title: &str, artist: &str, album: &str, mime: &str, art: Option<&str>) -> String {
    let art_xml = art
        .map(|a| format!("<upnp:albumArtURI>{}</upnp:albumArtURI>", escape(a)))
        .unwrap_or_default();
    format!(
        "<DIDL-Lite xmlns=\"urn:schemas-upnp-org:metadata-1-0/DIDL-Lite/\" \
         xmlns:dc=\"http://purl.org/dc/elements/1.1/\" \
         xmlns:upnp=\"urn:schemas-upnp-org:metadata-1-0/upnp/\">\
         <item id=\"1\" parentID=\"0\" restricted=\"1\">\
         <dc:title>{}</dc:title><upnp:artist>{}</upnp:artist><upnp:album>{}</upnp:album>{}\
         <upnp:class>object.item.audioItem.musicTrack</upnp:class>\
         <res protocolInfo=\"http-get:*:{}:*\">{}</res></item></DIDL-Lite>",
        escape(if title.is_empty() { "Track" } else { title }),
        escape(artist),
        escape(album),
        art_xml,
        escape(mime),
        escape(uri),
    )
}
