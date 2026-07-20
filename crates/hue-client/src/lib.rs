//! Philips Hue bridge client: mDNS discovery, pairing (CLIP v1 endpoint),
//! and CLIP v2 REST for lights / entertainment configurations.

use std::collections::HashSet;
use std::time::Duration;

use serde::{Deserialize, Serialize};

#[derive(Debug, thiserror::Error)]
pub enum HueError {
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("mdns error: {0}")]
    Mdns(String),
    #[error("bridge error: {0}")]
    Bridge(String),
    #[error("link button not pressed")]
    LinkButtonNotPressed,
    #[error("unexpected response: {0}")]
    Unexpected(String),
}

pub type Result<T> = std::result::Result<T, HueError>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveredBridge {
    pub ip: String,
    pub name: String,
    /// Bridge id from mDNS TXT record (lowercase hex), if present.
    pub bridge_id: Option<String>,
}

/// Browse `_hue._tcp.local.` for the given duration.
pub fn discover_bridges(timeout: Duration) -> Result<Vec<DiscoveredBridge>> {
    let daemon = mdns_sd::ServiceDaemon::new().map_err(|e| HueError::Mdns(e.to_string()))?;
    let receiver = daemon
        .browse("_hue._tcp.local.")
        .map_err(|e| HueError::Mdns(e.to_string()))?;

    let mut found = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let deadline = std::time::Instant::now() + timeout;
    loop {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() {
            break;
        }
        let Ok(event) = receiver.recv_timeout(remaining) else {
            break;
        };
        if let mdns_sd::ServiceEvent::ServiceResolved(info) = event {
            for addr in info.get_addresses() {
                let ip = addr.to_string();
                // Prefer IPv4; Hue entertainment streaming is IPv4-only in practice.
                if ip.contains(':') {
                    continue;
                }
                if seen.insert(ip.clone()) {
                    found.push(DiscoveredBridge {
                        ip,
                        name: info.get_fullname().to_string(),
                        bridge_id: info.get_property_val_str("bridgeid").map(|s| s.to_string()),
                    });
                }
            }
        }
    }
    let _ = daemon.shutdown();
    Ok(found)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairedBridge {
    pub ip: String,
    /// `hue-application-key` header value (a.k.a. username).
    pub app_key: String,
    /// 32-hex-char PSK for entertainment streaming.
    pub client_key: String,
}

fn insecure_client() -> Result<reqwest::Client> {
    // The bridge serves a certificate signed by Philips' private root CA
    // for an IP SAN; standard validation always fails, so it is disabled.
    Ok(reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .danger_accept_invalid_hostnames(true)
        .timeout(Duration::from_secs(10))
        .build()?)
}

/// Single pairing attempt. Returns `LinkButtonNotPressed` until the user
/// presses the physical button; poll this every second or two.
pub async fn pair(ip: &str, device_name: &str) -> Result<PairedBridge> {
    #[derive(Serialize)]
    struct PairReq<'a> {
        devicetype: &'a str,
        generateclientkey: bool,
    }
    let client = insecure_client()?;
    let resp: serde_json::Value = client
        .post(format!("https://{ip}/api"))
        .json(&PairReq {
            devicetype: device_name,
            generateclientkey: true,
        })
        .send()
        .await?
        .json()
        .await?;

    let first = resp
        .as_array()
        .and_then(|a| a.first())
        .ok_or_else(|| HueError::Unexpected(resp.to_string()))?;
    if let Some(err) = first.get("error") {
        let code = err.get("type").and_then(|v| v.as_i64()).unwrap_or(0);
        if code == 101 {
            return Err(HueError::LinkButtonNotPressed);
        }
        return Err(HueError::Bridge(err.to_string()));
    }
    let success = first
        .get("success")
        .ok_or_else(|| HueError::Unexpected(resp.to_string()))?;
    Ok(PairedBridge {
        ip: ip.to_string(),
        app_key: success
            .get("username")
            .and_then(|v| v.as_str())
            .ok_or_else(|| HueError::Unexpected("missing username".into()))?
            .to_string(),
        client_key: success
            .get("clientkey")
            .and_then(|v| v.as_str())
            .ok_or_else(|| HueError::Unexpected("missing clientkey".into()))?
            .to_string(),
    })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LightInfo {
    pub id: String,
    pub name: String,
    pub on: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntertainmentChannel {
    pub channel_id: u8,
    /// Position in the entertainment area, -1..1 per axis.
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntertainmentConfig {
    pub id: String,
    pub name: String,
    pub channels: Vec<EntertainmentChannel>,
}

pub struct HueClient {
    ip: String,
    app_key: String,
    http: reqwest::Client,
}

impl HueClient {
    pub fn new(bridge: &PairedBridge) -> Result<Self> {
        Ok(Self {
            ip: bridge.ip.clone(),
            app_key: bridge.app_key.clone(),
            http: insecure_client()?,
        })
    }

    fn url(&self, path: &str) -> String {
        format!("https://{}/clip/v2/{}", self.ip, path)
    }

    async fn get(&self, path: &str) -> Result<serde_json::Value> {
        let resp = self
            .http
            .get(self.url(path))
            .header("hue-application-key", &self.app_key)
            .send()
            .await?;
        let status = resp.status();
        let body: serde_json::Value = resp.json().await?;
        if !status.is_success() {
            return Err(HueError::Bridge(format!("{status}: {body}")));
        }
        Ok(body)
    }

    async fn put(&self, path: &str, body: serde_json::Value) -> Result<serde_json::Value> {
        let resp = self
            .http
            .put(self.url(path))
            .header("hue-application-key", &self.app_key)
            .json(&body)
            .send()
            .await?;
        let status = resp.status();
        let body: serde_json::Value = resp.json().await?;
        if !status.is_success() {
            return Err(HueError::Bridge(format!("{status}: {body}")));
        }
        Ok(body)
    }

    /// The DTLS PSK identity for entertainment streaming.
    ///
    /// Some bridge firmwares do not expose the `hue-application-id` header
    /// on `/auth/v2`; callers should fall back to the application key as
    /// PSK identity when this fails.
    pub async fn application_id(&self) -> Result<String> {
        let resp = self
            .http
            .get(format!("https://{}/auth/v2", self.ip))
            .header("hue-application-key", &self.app_key)
            .send()
            .await?;
        let status = resp.status();
        resp.headers()
            .get("hue-application-id")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string())
            .ok_or_else(|| {
                HueError::Unexpected(format!(
                    "missing hue-application-id header (GET /auth/v2 -> {status})"
                ))
            })
    }

    pub async fn lights(&self) -> Result<Vec<LightInfo>> {
        let body = self.get("resource/light").await?;
        let mut out = Vec::new();
        for item in body.get("data").and_then(|d| d.as_array()).into_iter().flatten() {
            out.push(LightInfo {
                id: item.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                name: item
                    .pointer("/metadata/name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("light")
                    .to_string(),
                on: item.pointer("/on/on").and_then(|v| v.as_bool()).unwrap_or(false),
            });
        }
        Ok(out)
    }

    pub async fn entertainment_configs(&self) -> Result<Vec<EntertainmentConfig>> {
        let body = self.get("resource/entertainment_configuration").await?;
        let mut out = Vec::new();
        for item in body.get("data").and_then(|d| d.as_array()).into_iter().flatten() {
            let mut channels = Vec::new();
            for ch in item.get("channels").and_then(|c| c.as_array()).into_iter().flatten() {
                channels.push(EntertainmentChannel {
                    channel_id: ch.get("channel_id").and_then(|v| v.as_u64()).unwrap_or(0) as u8,
                    x: ch.pointer("/position/x").and_then(|v| v.as_f64()).unwrap_or(0.0),
                    y: ch.pointer("/position/y").and_then(|v| v.as_f64()).unwrap_or(0.0),
                    z: ch.pointer("/position/z").and_then(|v| v.as_f64()).unwrap_or(0.0),
                });
            }
            out.push(EntertainmentConfig {
                id: item.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                name: item
                    .pointer("/metadata/name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("area")
                    .to_string(),
                channels,
            });
        }
        Ok(out)
    }

    /// Tell the bridge to open the entertainment UDP/DTLS port for us.
    /// Must be followed by a DTLS handshake within a few seconds.
    pub async fn start_streaming(&self, config_id: &str) -> Result<()> {
        self.put(
            &format!("resource/entertainment_configuration/{config_id}"),
            serde_json::json!({ "action": "start" }),
        )
        .await?;
        Ok(())
    }

    pub async fn stop_streaming(&self, config_id: &str) -> Result<()> {
        self.put(
            &format!("resource/entertainment_configuration/{config_id}"),
            serde_json::json!({ "action": "stop" }),
        )
        .await?;
        Ok(())
    }
}
