//! Small XML helpers shared by the WiiM (UPnP/SOAP) and DLNA code.
//! Everything works on namespace-stripped local names, like the old Qt code.

use quick_xml::events::{BytesStart, Event};
use quick_xml::Reader;
use std::collections::HashMap;

fn local(e: &BytesStart) -> String {
    String::from_utf8_lossy(e.local_name().as_ref()).into_owned()
}

fn attr(e: &BytesStart, name: &str) -> String {
    for a in e.attributes().flatten() {
        if a.key.local_name().as_ref() == name.as_bytes() {
            if let Ok(v) = a.unescape_value() {
                return v.into_owned();
            }
        }
    }
    String::new()
}

/// Flatten a SOAP response into `element name -> trimmed text` for every
/// non-empty leaf (the last occurrence wins). Entities are decoded.
pub fn flatten_soap(xml: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let mut reader = Reader::from_str(xml);
    let mut stack: Vec<(String, String)> = Vec::new();
    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => stack.push((local(&e), String::new())),
            Ok(Event::Text(t)) => {
                if let (Some(top), Ok(s)) = (stack.last_mut(), t.unescape()) {
                    top.1.push_str(&s);
                }
            }
            Ok(Event::CData(c)) => {
                if let Some(top) = stack.last_mut() {
                    top.1.push_str(&String::from_utf8_lossy(&c));
                }
            }
            Ok(Event::End(_)) => {
                if let Some((name, text)) = stack.pop() {
                    let text = text.trim();
                    if !text.is_empty() && name != "Envelope" && name != "Body" {
                        out.insert(name, text.to_string());
                    }
                }
            }
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }
    out
}

/// Decode the entity-escaped text one more time when a device double-encodes
/// its DIDL metadata (a no-op for already-decoded XML).
pub fn unescape_again(s: &str) -> String {
    if !s.contains("&lt;") {
        return s.to_string();
    }
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

/// One `<item>` or `<container>` of a DIDL-Lite document.
#[derive(Debug, Default, Clone)]
pub struct DidlEntry {
    pub is_container: bool,
    pub id: String,
    pub parent_id: String,
    pub child_count: u32,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub album_art: String,
    pub class: String,
    pub res_uri: String,
    pub protocol_info: String,
    pub duration: String,
}

pub fn parse_didl(xml: &str) -> Vec<DidlEntry> {
    let mut out = Vec::new();
    let mut reader = Reader::from_str(xml);
    let mut cur: Option<DidlEntry> = None;
    let mut creator = String::new();
    let mut field = String::new();
    let mut res_seen = false;
    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => {
                let name = local(&e);
                match name.as_str() {
                    "item" | "container" => {
                        cur = Some(DidlEntry {
                            is_container: name == "container",
                            id: attr(&e, "id"),
                            parent_id: attr(&e, "parentID"),
                            child_count: attr(&e, "childCount").parse().unwrap_or(0),
                            ..Default::default()
                        });
                        creator.clear();
                        res_seen = false;
                    }
                    "res" => {
                        // keep the first <res>; servers list transcodes after it
                        if let Some(c) = cur.as_mut() {
                            if !res_seen {
                                c.protocol_info = attr(&e, "protocolInfo");
                                c.duration = attr(&e, "duration");
                            }
                        }
                        field = if res_seen { String::new() } else { name };
                    }
                    _ => field = name,
                }
            }
            Ok(Event::Text(t)) => {
                if let (Some(c), Ok(s)) = (cur.as_mut(), t.unescape()) {
                    match field.as_str() {
                        "title" => c.title.push_str(&s),
                        "artist" => c.artist.push_str(&s),
                        "creator" => creator.push_str(&s),
                        "album" => c.album.push_str(&s),
                        "albumArtURI" => c.album_art.push_str(&s),
                        "class" => c.class.push_str(&s),
                        "res" => c.res_uri.push_str(s.trim()),
                        _ => {}
                    }
                }
            }
            Ok(Event::End(e)) => {
                let name = String::from_utf8_lossy(e.local_name().as_ref()).into_owned();
                if name == "res" {
                    res_seen = true;
                }
                field.clear();
                if name == "item" || name == "container" {
                    if let Some(mut c) = cur.take() {
                        if c.artist.is_empty() {
                            c.artist = std::mem::take(&mut creator);
                        }
                        out.push(c);
                    }
                }
            }
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }
    out
}

/// Pull the ContentDirectory (or any matching) control URL and friendly name
/// out of a UPnP device description.
#[derive(Debug, Default, Clone)]
pub struct DeviceDescription {
    pub url_base: String,
    pub friendly_name: String,
    pub model_name: String,
    pub model_description: String,
    pub udn: String,
    /// (serviceType, controlURL)
    pub services: Vec<(String, String)>,
}

pub fn parse_device_description(xml: &str) -> DeviceDescription {
    let mut d = DeviceDescription::default();
    let mut reader = Reader::from_str(xml);
    let mut stack: Vec<String> = Vec::new();
    let mut svc_type = String::new();
    let mut svc_ctl = String::new();
    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => {
                let n = local(&e);
                if n == "service" {
                    svc_type.clear();
                    svc_ctl.clear();
                }
                stack.push(n);
            }
            Ok(Event::Text(t)) => {
                let Ok(s) = t.unescape() else { continue };
                let s = s.trim();
                if s.is_empty() {
                    continue;
                }
                let Some(name) = stack.last() else { continue };
                // Only take identity fields from the root device, not embedded ones.
                let depth_device = stack.iter().filter(|x| *x == "device").count();
                let in_service = stack.iter().any(|x| x == "service");
                match name.as_str() {
                    "URLBase" => d.url_base = s.to_string(),
                    "friendlyName" if depth_device == 1 => {
                        d.friendly_name = s.to_string()
                    }
                    "modelName" if depth_device == 1 => d.model_name = s.to_string(),
                    "modelDescription" if depth_device == 1 => d.model_description = s.to_string(),
                    "UDN" if depth_device == 1 => d.udn = s.to_string(),
                    "serviceType" if in_service => svc_type = s.to_string(),
                    "controlURL" if in_service => svc_ctl = s.to_string(),
                    _ => {}
                }
            }
            Ok(Event::End(_)) => {
                if let Some(n) = stack.pop() {
                    if n == "service" && !svc_type.is_empty() && !svc_ctl.is_empty() {
                        d.services.push((svc_type.clone(), svc_ctl.clone()));
                    }
                }
            }
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }
    d
}

pub fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

pub fn parse_hms(v: &str) -> u32 {
    let parts: Vec<&str> = v.split(':').collect();
    if parts.len() != 3 {
        return 0;
    }
    let h: u32 = parts[0].parse().unwrap_or(0);
    let m: u32 = parts[1].parse().unwrap_or(0);
    // seconds may carry a fractional part ("0:03:25.000")
    let s: f64 = parts[2].parse().unwrap_or(0.0);
    h * 3600 + m * 60 + s as u32
}

pub fn format_hms(secs: u32) -> String {
    format!("{:02}:{:02}:{:02}", secs / 3600, secs % 3600 / 60, secs % 60)
}
