//! DLNA / UPnP MediaServer browsing (ContentDirectory:1).

use crate::xml::{self, escape, flatten_soap, parse_didl};
use serde::Serialize;
use std::time::Duration;

const CD: &str = "urn:schemas-upnp-org:service:ContentDirectory:1";
const PAGE: u32 = 200;

#[derive(Debug, Clone, Serialize)]
pub struct DlnaServer {
    pub name: String,
    pub location: String,
    #[serde(skip)]
    pub control_url: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct DlnaItem {
    pub id: String,
    pub parent_id: String,
    pub title: String,
    pub is_container: bool,
    pub child_count: u32,
    pub uri: String,
    pub mime: String,
    pub artist: String,
    pub album: String,
    pub album_art: String,
    pub duration: u32,
}

impl DlnaServer {
    pub async fn connect(http: &reqwest::Client, location: &str) -> Result<DlnaServer, String> {
        let body = http
            .get(location)
            .timeout(Duration::from_secs(4))
            .send()
            .await
            .and_then(|r| r.error_for_status())
            .map_err(|e| e.to_string())?
            .text()
            .await
            .map_err(|e| e.to_string())?;
        let d = xml::parse_device_description(&body);
        let ctl = d
            .services
            .iter()
            .find(|(t, _)| t.to_lowercase().contains("contentdirectory"))
            .map(|(_, c)| c.clone())
            .ok_or("no ContentDirectory service")?;
        let base = if d.url_base.is_empty() { location.to_string() } else { d.url_base };
        let control_url = url::Url::parse(&base)
            .and_then(|b| b.join(&ctl))
            .map_err(|e| e.to_string())?
            .to_string();
        let name = if d.friendly_name.is_empty() {
            url::Url::parse(location).ok().and_then(|u| u.host_str().map(str::to_string)).unwrap_or_default()
        } else {
            d.friendly_name
        };
        Ok(DlnaServer { name, location: location.to_string(), control_url })
    }

    /// Browse all direct children of `object_id`, following pagination.
    pub async fn browse(&self, http: &reqwest::Client, object_id: &str) -> Result<Vec<DlnaItem>, String> {
        let mut all = Vec::new();
        let mut start = 0u32;
        loop {
            let envelope = format!(
                "<?xml version=\"1.0\" encoding=\"utf-8\"?>\
                 <s:Envelope xmlns:s=\"http://schemas.xmlsoap.org/soap/envelope/\"><s:Body>\
                 <u:Browse xmlns:u=\"{CD}\"><ObjectID>{}</ObjectID>\
                 <BrowseFlag>BrowseDirectChildren</BrowseFlag><Filter>*</Filter>\
                 <StartingIndex>{start}</StartingIndex><RequestedCount>{PAGE}</RequestedCount>\
                 <SortCriteria></SortCriteria></u:Browse></s:Body></s:Envelope>",
                escape(object_id)
            );
            let resp = http
                .post(&self.control_url)
                .timeout(Duration::from_secs(10))
                .header("Content-Type", "text/xml; charset=\"utf-8\"")
                .header("SOAPACTION", format!("\"{CD}#Browse\""))
                .body(envelope)
                .send()
                .await
                .map_err(|e| format!("browse failed: {e}"))?;
            let status = resp.status();
            let text = resp.text().await.map_err(|e| e.to_string())?;
            if !status.is_success() {
                return Err(format!("browse failed: HTTP {status}"));
            }
            let f = flatten_soap(&text);
            let returned: u32 = f.get("NumberReturned").and_then(|s| s.parse().ok()).unwrap_or(0);
            let total: u32 = f.get("TotalMatches").and_then(|s| s.parse().ok()).unwrap_or(0);
            let didl = f.get("Result").cloned().unwrap_or_default();

            for e in parse_didl(&didl) {
                if e.is_container {
                    if !e.title.is_empty() {
                        all.push(DlnaItem {
                            id: e.id, parent_id: e.parent_id, title: e.title, is_container: true,
                            child_count: e.child_count, uri: String::new(), mime: String::new(),
                            artist: String::new(), album: String::new(), album_art: String::new(), duration: 0,
                        });
                    }
                } else {
                    let audio = e.protocol_info.to_lowercase().contains("audio/")
                        || e.class.to_lowercase().starts_with("object.item.audioitem");
                    if audio && !e.res_uri.is_empty() {
                        let mime = e.protocol_info.split(':').nth(2).unwrap_or("audio/mpeg").to_string();
                        all.push(DlnaItem {
                            id: e.id, parent_id: e.parent_id, title: e.title, is_container: false,
                            child_count: 0, uri: e.res_uri, mime, artist: e.artist, album: e.album,
                            album_art: e.album_art, duration: xml::parse_hms(&e.duration),
                        });
                    }
                }
            }

            start += returned;
            if returned == 0 || start >= total {
                break;
            }
        }
        Ok(all)
    }
}
