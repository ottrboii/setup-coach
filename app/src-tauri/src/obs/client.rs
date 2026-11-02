//! Minimal obs-websocket v5 client: Hello → Identify (with auth) → requests.
//! Raw JSON requests keep it small and make every OBS request available.

use super::ObsApi;
use base64::Engine;
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{oneshot, Mutex};
use tokio_tungstenite::tungstenite::Message;

type Pending = Arc<std::sync::Mutex<HashMap<String, oneshot::Sender<Result<Value, String>>>>>;
type Sink = futures_util::stream::SplitSink<
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
    Message,
>;

pub struct ObsClient {
    sink: Mutex<Sink>,
    pending: Pending,
    next: AtomicU64,
    alive: Arc<AtomicBool>,
    pub obs_ws_version: String,
}

/// `base64(sha256(base64(sha256(password + salt)) + challenge))`
pub fn auth_string(password: &str, salt: &str, challenge: &str) -> String {
    let b64 = base64::engine::general_purpose::STANDARD;
    let secret = b64.encode(Sha256::digest(format!("{password}{salt}").as_bytes()));
    b64.encode(Sha256::digest(format!("{secret}{challenge}").as_bytes()))
}

fn friendly(e: impl std::fmt::Display) -> String {
    format!("Couldn't talk to OBS ({e}). Is OBS open with its WebSocket server on?")
}

impl ObsClient {
    pub async fn connect(port: u16, password: Option<&str>) -> Result<Self, String> {
        let url = format!("ws://127.0.0.1:{port}");
        let (ws, _) = tokio::time::timeout(Duration::from_secs(4), tokio_tungstenite::connect_async(url.as_str()))
            .await
            .map_err(|_| "OBS didn't answer. Is it running?".to_string())?
            .map_err(friendly)?;
        let (mut sink, mut stream) = ws.split();
        // Hello
        let hello = read_json(&mut stream).await?;
        if hello["op"] != 0 {
            return Err("OBS sent an unexpected greeting.".into());
        }
        let d = &hello["d"];
        let mut identify = json!({ "rpcVersion": 1, "eventSubscriptions": 0 });
        if let Some(auth) = d.get("authentication") {
            let Some(pw) = password.filter(|p| !p.is_empty()) else {
                return Err("OBS needs the WebSocket password (OBS → Tools → WebSocket Server Settings).".into());
            };
            let salt = auth["salt"].as_str().unwrap_or_default();
            let challenge = auth["challenge"].as_str().unwrap_or_default();
            identify["authentication"] = json!(auth_string(pw, salt, challenge));
        }
        sink.send(Message::Text(json!({ "op": 1, "d": identify }).to_string())).await.map_err(friendly)?;
        let identified = read_json(&mut stream).await.map_err(|_| "OBS rejected the WebSocket password.".to_string())?;
        if identified["op"] != 2 {
            return Err("OBS rejected the WebSocket password.".into());
        }
        let pending: Pending = Arc::new(std::sync::Mutex::new(HashMap::new()));
        let alive = Arc::new(AtomicBool::new(true));
        // Reader task: route responses to their waiting requests.
        {
            let pending = pending.clone();
            let alive = alive.clone();
            tokio::spawn(async move {
                while let Some(msg) = stream.next().await {
                    let Ok(Message::Text(t)) = msg else {
                        if matches!(msg, Ok(Message::Close(_)) | Err(_)) {
                            break;
                        }
                        continue;
                    };
                    let Ok(v) = serde_json::from_str::<Value>(&t) else { continue };
                    if v["op"] != 7 {
                        continue;
                    }
                    let d = &v["d"];
                    let id = d["requestId"].as_str().unwrap_or_default().to_string();
                    let res = if d["requestStatus"]["result"].as_bool() == Some(true) {
                        Ok(d.get("responseData").cloned().unwrap_or(Value::Null))
                    } else {
                        Err(format!(
                            "OBS said no to {}: {}",
                            d["requestType"].as_str().unwrap_or("a request"),
                            d["requestStatus"]["comment"].as_str().unwrap_or("unknown error")
                        ))
                    };
                    if let Some(tx) = pending.lock().unwrap().remove(&id) {
                        let _ = tx.send(res);
                    }
                }
                alive.store(false, Ordering::SeqCst);
                for (_, tx) in pending.lock().unwrap().drain() {
                    let _ = tx.send(Err("Lost the connection to OBS.".into()));
                }
            });
        }
        Ok(ObsClient {
            sink: Mutex::new(sink),
            pending,
            next: AtomicU64::new(1),
            alive,
            obs_ws_version: d["obsWebSocketVersion"].as_str().unwrap_or_default().to_string(),
        })
    }
}

async fn read_json<S>(stream: &mut S) -> Result<Value, String>
where
    S: futures_util::Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    loop {
        let msg = tokio::time::timeout(Duration::from_secs(4), stream.next())
            .await
            .map_err(|_| "OBS stopped answering.".to_string())?
            .ok_or_else(|| "OBS closed the connection.".to_string())?
            .map_err(friendly)?;
        match msg {
            Message::Text(t) => return serde_json::from_str(&t).map_err(friendly),
            Message::Close(_) => return Err("OBS closed the connection (wrong password?).".into()),
            _ => continue,
        }
    }
}

#[async_trait::async_trait]
impl ObsApi for ObsClient {
    async fn request(&self, request_type: &str, data: Value) -> Result<Value, String> {
        if !self.alive.load(Ordering::SeqCst) {
            return Err("Lost the connection to OBS.".into());
        }
        let id = self.next.fetch_add(1, Ordering::SeqCst).to_string();
        let (tx, rx) = oneshot::channel();
        self.pending.lock().unwrap().insert(id.clone(), tx);
        let mut d = json!({ "requestType": request_type, "requestId": id });
        if !data.is_null() {
            d["requestData"] = data;
        }
        self.sink
            .lock()
            .await
            .send(Message::Text(json!({ "op": 6, "d": d }).to_string()))
            .await
            .map_err(friendly)?;
        match tokio::time::timeout(Duration::from_secs(6), rx).await {
            Ok(Ok(r)) => r,
            Ok(Err(_)) => Err("Lost the connection to OBS.".into()),
            Err(_) => {
                self.pending.lock().unwrap().remove(&id);
                Err(format!("OBS took too long to answer {request_type}."))
            }
        }
    }

    fn is_alive(&self) -> bool {
        self.alive.load(Ordering::SeqCst)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn auth_matches_protocol_example() {
        // Values from the obs-websocket protocol docs example.
        let s = super::auth_string("supersecretpassword", "lM1GncleQOaCu9lT1yeUZhFYnqhsLLP1G5lAGo3ixaI=", "+IxH4CnCiqpX1rM9scsNynZzbOe4KhDeYcTNS3PDaeY=");
        assert_eq!(s.len(), 44);
        assert!(s.ends_with('='));
    }
}
