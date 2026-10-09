//! SSDP discovery. Sends M-SEARCH twice (UDP is lossy) and collects unicast
//! replies for a short window. Never blocks the caller beyond that window.

use std::collections::HashMap;
use std::time::Duration;
use tokio::net::UdpSocket;
use tokio::time::Instant;

pub const RENDERER: &str = "urn:schemas-upnp-org:device:MediaRenderer:1";
pub const SERVER: &str = "urn:schemas-upnp-org:device:MediaServer:1";

/// Returns the distinct `LOCATION` URLs that answered.
pub async fn search(target: &str, window: Duration) -> Vec<String> {
    let Ok(sock) = UdpSocket::bind("0.0.0.0:0").await else {
        return vec![];
    };
    let msg = format!(
        "M-SEARCH * HTTP/1.1\r\nHOST: 239.255.255.250:1900\r\nMAN: \"ssdp:discover\"\r\nMX: 2\r\nST: {target}\r\n\r\n"
    );
    let dest = "239.255.255.250:1900";
    let _ = sock.send_to(msg.as_bytes(), dest).await;

    let deadline = Instant::now() + window;
    let mut resent = false;
    let mut found: HashMap<String, ()> = HashMap::new();
    let mut buf = vec![0u8; 8192];
    loop {
        let now = Instant::now();
        if now >= deadline {
            break;
        }
        if !resent && now + window / 2 >= deadline {
            let _ = sock.send_to(msg.as_bytes(), dest).await;
            resent = true;
        }
        let slice = (deadline - now).min(Duration::from_millis(500));
        match tokio::time::timeout(slice, sock.recv_from(&mut buf)).await {
            Ok(Ok((n, _))) => {
                let text = String::from_utf8_lossy(&buf[..n]);
                for line in text.lines().skip(1) {
                    if let Some((k, v)) = line.split_once(':') {
                        if k.trim().eq_ignore_ascii_case("location") {
                            found.insert(v.trim().to_string(), ());
                        }
                    }
                }
            }
            _ => {}
        }
    }
    found.into_keys().collect()
}
