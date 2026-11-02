//! GoXLR Utility client (open source replacement for the official GoXLR
//! App; github.com/GoXLR-on-Linux/goxlr-utility, MIT).
//!
//! Verified against the Utility's source:
//! - `daemon/src/servers/http_server.rs`: `POST /api/command` takes a JSON
//!   `DaemonRequest` and always answers 200 with a JSON `DaemonResponse`
//!   (errors as `{"Error": "..."}`); default port 14564 (`daemon/src/cli.rs`).
//! - `ipc/src/lib.rs`: `DaemonRequest::{Ping, GetStatus, Command(serial,
//!   GoXLRCommand)}`, serde's default external tagging, so e.g.
//!   `{"Command":["S2…",{"SetMicrophoneGain":["Dynamic",40]}]}`;
//!   `DaemonResponse::{Ok, Error(String), Status(DaemonStatus)}`.
//! - `GoXLRCommand::SetMicrophoneType(MicrophoneType)` and
//!   `SetMicrophoneGain(MicrophoneType, u16)`; `MicrophoneType` is
//!   `Dynamic | Condenser | Jack` (`types/src/lib.rs`); gain is 0–72 dB
//!   (`profile/src/microphone/mic_setup.rs`). Condenser mode = 48 V phantom.
//! - `ipc/src/device.rs`: `DaemonStatus.mixers: {serial: MixerStatus}`,
//!   `MixerStatus.mic_status: {mic_type, mic_gains: {Dynamic, Condenser,
//!   Jack}, noise_gate {enabled,…}, compressor {…}}`, `levels.deess`,
//!   `hardware.device_type: Full | Mini`.

use async_trait::async_trait;
use serde_json::{json, Value};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

pub const DEFAULT_PORT: u16 = 14564;
pub const GAIN_RANGE_DB: (f64, f64) = (0.0, 72.0);

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum MicType {
    Dynamic,
    Condenser,
    Jack,
}

impl MicType {
    pub fn as_str(self) -> &'static str {
        match self {
            MicType::Dynamic => "Dynamic",
            MicType::Condenser => "Condenser",
            MicType::Jack => "Jack",
        }
    }
    fn parse(s: &str) -> Option<Self> {
        match s {
            "Dynamic" => Some(MicType::Dynamic),
            "Condenser" => Some(MicType::Condenser),
            "Jack" => Some(MicType::Jack),
            _ => None,
        }
    }
}

/// What we read from one GoXLR.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct GoXlrDevice {
    pub serial: String,
    /// "Full" | "Mini" | "Unknown".
    pub device_type: String,
    pub mic_type: MicType,
    /// Gain for the current mic type (dB, 0–72).
    pub mic_gain_db: f64,
    pub gate_enabled: bool,
    pub compressor_threshold_db: Option<f64>,
    pub deess: Option<f64>,
}

impl GoXlrDevice {
    pub fn phantom_on(&self) -> bool {
        self.mic_type == MicType::Condenser
    }
}

/// Parse a `DaemonResponse::Status` body.
pub fn parse_status(v: &Value) -> Result<Vec<GoXlrDevice>, String> {
    if let Some(e) = v.get("Error").and_then(|e| e.as_str()) {
        return Err(e.to_string());
    }
    let st = v.get("Status").unwrap_or(v);
    let mixers = st.get("mixers").and_then(|m| m.as_object()).ok_or("GoXLR Utility didn't report any devices.")?;
    let mut out = Vec::new();
    for (serial, m) in mixers {
        let mic = &m["mic_status"];
        let Some(mic_type) = mic["mic_type"].as_str().and_then(MicType::parse) else { continue };
        let gain = mic["mic_gains"][mic_type.as_str()].as_f64().unwrap_or(0.0);
        out.push(GoXlrDevice {
            serial: serial.clone(),
            device_type: m["hardware"]["device_type"].as_str().unwrap_or("Unknown").to_string(),
            mic_type,
            mic_gain_db: gain,
            gate_enabled: mic["noise_gate"]["enabled"].as_bool().unwrap_or(false),
            compressor_threshold_db: mic["compressor"]["threshold"].as_f64(),
            deess: m["levels"]["deess"].as_f64(),
        });
    }
    out.sort_by(|a, b| a.serial.cmp(&b.serial));
    Ok(out)
}

pub fn cmd_set_mic_gain(mic_type: MicType, db: f64) -> Value {
    let g = db.round().clamp(GAIN_RANGE_DB.0, GAIN_RANGE_DB.1) as u16;
    json!({ "SetMicrophoneGain": [mic_type.as_str(), g] })
}

pub fn cmd_set_mic_type(mic_type: MicType) -> Value {
    json!({ "SetMicrophoneType": mic_type.as_str() })
}

/// The Utility's API (HTTP in the app, a fake in tests).
#[async_trait]
pub trait GoXlrApi: Send + Sync {
    /// Send a raw `DaemonRequest`, get the `DaemonResponse`.
    async fn request(&self, req: Value) -> Result<Value, String>;

    async fn devices(&self) -> Result<Vec<GoXlrDevice>, String> {
        parse_status(&self.request(json!("GetStatus")).await?)
    }

    async fn command(&self, serial: &str, cmd: Value) -> Result<(), String> {
        let r = self.request(json!({ "Command": [serial, cmd] })).await?;
        match r.get("Error").and_then(|e| e.as_str()) {
            Some(e) => Err(format!("GoXLR Utility: {e}")),
            None => Ok(()),
        }
    }
}

/// Talks to the GoXLR Utility daemon over its local HTTP API.
pub struct GoXlrHttp {
    pub port: u16,
}

impl GoXlrHttp {
    /// Connect if the daemon answers (and reports at least one GoXLR).
    pub async fn detect(port: u16) -> Option<Self> {
        let c = GoXlrHttp { port };
        match c.devices().await {
            Ok(d) if !d.is_empty() => Some(c),
            _ => None,
        }
    }
}

#[async_trait]
impl GoXlrApi for GoXlrHttp {
    async fn request(&self, req: Value) -> Result<Value, String> {
        let body = serde_json::to_vec(&req).map_err(|e| e.to_string())?;
        let fut = async {
            let mut s = TcpStream::connect(("127.0.0.1", self.port)).await.map_err(|_| "GoXLR Utility isn't running.".to_string())?;
            let head = format!(
                "POST /api/command HTTP/1.1\r\nHost: localhost:{}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                self.port,
                body.len()
            );
            s.write_all(head.as_bytes()).await.map_err(|e| e.to_string())?;
            s.write_all(&body).await.map_err(|e| e.to_string())?;
            let mut buf = Vec::new();
            s.read_to_end(&mut buf).await.map_err(|e| e.to_string())?;
            parse_http_json(&buf)
        };
        tokio::time::timeout(Duration::from_secs(3), fut).await.map_err(|_| "GoXLR Utility didn't answer.".to_string())?
    }
}

/// Body of an HTTP/1.1 response as JSON (Content-Length or chunked).
pub fn parse_http_json(raw: &[u8]) -> Result<Value, String> {
    let split = raw.windows(4).position(|w| w == b"\r\n\r\n").ok_or("Bad answer from GoXLR Utility.")?;
    let head = String::from_utf8_lossy(&raw[..split]).to_ascii_lowercase();
    if !head.starts_with("http/1.1 200") && !head.starts_with("http/1.0 200") {
        return Err(format!("GoXLR Utility answered: {}", head.lines().next().unwrap_or("")));
    }
    let mut body = raw[split + 4..].to_vec();
    if head.contains("transfer-encoding: chunked") {
        let mut out = Vec::new();
        let mut rest = &body[..];
        while let Some(eol) = rest.windows(2).position(|w| w == b"\r\n") {
            let size = usize::from_str_radix(String::from_utf8_lossy(&rest[..eol]).trim(), 16).map_err(|_| "Bad chunk from GoXLR Utility.")?;
            if size == 0 {
                break;
            }
            let start = eol + 2;
            out.extend_from_slice(rest.get(start..start + size).ok_or("Truncated answer from GoXLR Utility.")?);
            rest = rest.get(start + size + 2..).unwrap_or(&[]);
        }
        body = out;
    }
    serde_json::from_slice(&body).map_err(|e| format!("Bad answer from GoXLR Utility ({e})."))
}

/// In-memory GoXLR for tests and demos: applies gain / mic type commands.
#[derive(Default)]
pub struct FakeGoXlr {
    pub state: std::sync::Mutex<Vec<GoXlrDevice>>,
    pub commands: std::sync::Mutex<Vec<Value>>,
    /// Shared with a live mock mic so gain changes are audible.
    pub gain_link: Option<std::sync::Arc<std::sync::Mutex<f64>>>,
}

impl FakeGoXlr {
    pub fn with_device(serial: &str, mic_type: MicType, gain: f64) -> Self {
        FakeGoXlr {
            state: std::sync::Mutex::new(vec![GoXlrDevice {
                serial: serial.into(),
                device_type: "Full".into(),
                mic_type,
                mic_gain_db: gain,
                gate_enabled: true,
                compressor_threshold_db: Some(-20.0),
                deess: Some(0.0),
            }]),
            ..Default::default()
        }
    }
}

#[async_trait]
impl GoXlrApi for FakeGoXlr {
    async fn request(&self, req: Value) -> Result<Value, String> {
        if req == json!("GetStatus") {
            let st = self.state.lock().unwrap();
            let mixers: serde_json::Map<String, Value> = st
                .iter()
                .map(|d| {
                    (
                        d.serial.clone(),
                        json!({
                            "hardware": { "device_type": d.device_type, "serial_number": d.serial },
                            "mic_status": {
                                "mic_type": d.mic_type.as_str(),
                                "mic_gains": { "Dynamic": if d.mic_type == MicType::Dynamic { d.mic_gain_db } else { 40.0 },
                                               "Condenser": if d.mic_type == MicType::Condenser { d.mic_gain_db } else { 30.0 },
                                               "Jack": if d.mic_type == MicType::Jack { d.mic_gain_db } else { 20.0 } },
                                "noise_gate": { "enabled": d.gate_enabled },
                                "compressor": { "threshold": d.compressor_threshold_db }
                            },
                            "levels": { "deess": d.deess }
                        }),
                    )
                })
                .collect();
            return Ok(json!({ "Status": { "mixers": mixers } }));
        }
        self.commands.lock().unwrap().push(req.clone());
        let Some([serial, cmd]) = req.get("Command").and_then(|c| c.as_array()).map(|a| [a[0].clone(), a[1].clone()]) else {
            return Ok(json!({ "Error": "unsupported" }));
        };
        let mut st = self.state.lock().unwrap();
        let Some(d) = st.iter_mut().find(|d| Some(d.serial.as_str()) == serial.as_str()) else { return Ok(json!({ "Error": "no such device" })) };
        if let Some(a) = cmd.get("SetMicrophoneGain").and_then(|a| a.as_array()) {
            d.mic_type = a[0].as_str().and_then(MicType::parse).unwrap_or(d.mic_type);
            d.mic_gain_db = a[1].as_f64().unwrap_or(d.mic_gain_db);
            if let Some(l) = &self.gain_link {
                *l.lock().unwrap() = d.mic_gain_db;
            }
        } else if let Some(t) = cmd.get("SetMicrophoneType").and_then(|t| t.as_str()).and_then(MicType::parse) {
            d.mic_type = t;
        } else {
            return Ok(json!({ "Error": "unsupported command" }));
        }
        Ok(json!("Ok"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_json_matches_the_utility_ipc() {
        assert_eq!(json!({ "Command": ["S1", cmd_set_mic_gain(MicType::Dynamic, 41.6)] }).to_string(), r#"{"Command":["S1",{"SetMicrophoneGain":["Dynamic",42]}]}"#);
        assert_eq!(cmd_set_mic_type(MicType::Condenser).to_string(), r#"{"SetMicrophoneType":"Condenser"}"#);
        assert_eq!(cmd_set_mic_gain(MicType::Jack, 99.0)["SetMicrophoneGain"][1], 72);
    }

    #[test]
    fn parses_status_and_http() {
        let body = r#"{"Status":{"config":{},"mixers":{"S220202153DI7":{"hardware":{"device_type":"Mini"},"mic_status":{"mic_type":"Condenser","mic_gains":{"Dynamic":40,"Condenser":28,"Jack":0},"noise_gate":{"enabled":false},"compressor":{"threshold":-12}},"levels":{"deess":20}}}}}"#;
        let raw = format!("HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ntransfer-encoding: chunked\r\n\r\n{:x}\r\n{}\r\n0\r\n\r\n", body.len(), body);
        let v = parse_http_json(raw.as_bytes()).unwrap();
        let d = parse_status(&v).unwrap();
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].mic_type, MicType::Condenser);
        assert!(d[0].phantom_on());
        assert_eq!(d[0].mic_gain_db, 28.0);
        assert_eq!(d[0].device_type, "Mini");
        let raw2 = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{}", 17, r#"{"Error":"nope!"}"#);
        assert!(parse_status(&parse_http_json(raw2.as_bytes()).unwrap()).is_err());
    }

    #[tokio::test]
    async fn fake_applies_commands() {
        let f = FakeGoXlr::with_device("S1", MicType::Dynamic, 30.0);
        f.command("S1", cmd_set_mic_type(MicType::Condenser)).await.unwrap();
        f.command("S1", cmd_set_mic_gain(MicType::Condenser, 45.0)).await.unwrap();
        let d = f.devices().await.unwrap();
        assert_eq!(d[0].mic_type, MicType::Condenser);
        assert_eq!(d[0].mic_gain_db, 45.0);
        assert!(f.command("S9", cmd_set_mic_type(MicType::Dynamic)).await.is_err());
    }

    #[tokio::test]
    async fn http_client_round_trip() {
        // A one-shot fake daemon on a random port.
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = l.local_addr().unwrap().port();
        tokio::spawn(async move {
            let (mut s, _) = l.accept().await.unwrap();
            let mut buf = vec![0u8; 4096];
            let n = s.read(&mut buf).await.unwrap();
            let req = String::from_utf8_lossy(&buf[..n]).to_string();
            assert!(req.starts_with("POST /api/command HTTP/1.1"));
            assert!(req.ends_with("\"GetStatus\""));
            let body = r#"{"Status":{"mixers":{"S1":{"hardware":{"device_type":"Full"},"mic_status":{"mic_type":"Dynamic","mic_gains":{"Dynamic":51,"Condenser":0,"Jack":0},"noise_gate":{"enabled":true},"compressor":{"threshold":-20}},"levels":{"deess":0}}}}}"#;
            let resp = format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}", body.len(), body);
            s.write_all(resp.as_bytes()).await.unwrap();
        });
        let c = GoXlrHttp { port };
        let d = c.devices().await.unwrap();
        assert_eq!(d[0].mic_gain_db, 51.0);
    }
}
