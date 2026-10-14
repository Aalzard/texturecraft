//! Client for the running app's control channel (`texturecraft --control <port>`): one JSON
//! request per line, replies matched by `id`. Keeps one connection, reconnects once on failure.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::sync::Mutex;

use crate::AutomationError;
use crate::budgets::MAX_BRIDGE_REPLY_BYTES;

type Conn = (BufReader<OwnedReadHalf>, OwnedWriteHalf);

pub struct BridgeClient {
    port: u16,
    token: String,
    conn: Mutex<Option<Conn>>,
    next_id: AtomicU64,
}

impl BridgeClient {
    /// A client for the app on `127.0.0.1:port` (loopback only, like the server).
    pub fn new(port: u16, token: String) -> BridgeClient {
        BridgeClient { port, token, conn: Mutex::new(None), next_id: AtomicU64::new(1) }
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    async fn connect(&self) -> Result<Conn, AutomationError> {
        let addr = ("127.0.0.1", self.port);
        let s = tokio::time::timeout(Duration::from_secs(5), TcpStream::connect(addr))
            .await
            .map_err(|_| AutomationError::Bridge(format!("timed out connecting to 127.0.0.1:{}", self.port)))?
            .map_err(|e| {
                AutomationError::Bridge(format!("cannot connect to 127.0.0.1:{} ({e}); start the app with `texturecraft --control {}`", self.port, self.port))
            })?;
        let (r, w) = s.into_split();
        let mut conn = (BufReader::new(r), w);
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        tokio::time::timeout(Duration::from_secs(5), exchange(&mut conn, id, "auth", &json!({"token": self.token})))
            .await
            .map_err(|_| AutomationError::Bridge("authentication timed out".into()))??
            .map_err(|e| AutomationError::Bridge(format!("authentication failed: {e}")))?;
        Ok(conn)
    }

    /// Call a control method; returns its `result` or the app's error.
    pub async fn call(&self, method: &str, params: Value) -> Result<Value, AutomationError> {
        let mut guard = self.conn.lock().await;
        for attempt in 0..2 {
            let mut conn = match guard.take() {
                Some(c) => c,
                None => self.connect().await?,
            };
            let id = self.next_id.fetch_add(1, Ordering::Relaxed);
            match tokio::time::timeout(Duration::from_secs(150), exchange(&mut conn, id, method, &params)).await {
                Ok(Ok(reply)) => {
                    // An oversized reply (`Bridge`) leaves unread bytes: drop that connection.
                    if !matches!(reply, Err(AutomationError::Bridge(_))) {
                        *guard = Some(conn);
                    }
                    return reply;
                }
                // A dropped connection (the app restarted) is retried once on a fresh one.
                Ok(Err(e)) if attempt == 1 => return Err(e),
                Ok(Err(_)) => {}
                Err(_) => return Err(AutomationError::Bridge(format!("`{method}` timed out"))),
            }
        }
        Err(AutomationError::Bridge(format!("`{method}` failed")))
    }
}

/// Outer `Err`: transport failure (retryable). Inner: the app's answer.
async fn exchange(conn: &mut Conn, id: u64, method: &str, params: &Value) -> Result<Result<Value, AutomationError>, AutomationError> {
    let io = |e: std::io::Error| AutomationError::Bridge(e.to_string());
    let line = format!("{}\n", json!({"id": id, "method": method, "params": params}));
    conn.1.write_all(line.as_bytes()).await.map_err(io)?;
    conn.1.flush().await.map_err(io)?;
    let mut buf = Vec::new();
    loop {
        buf.clear();
        let n = (&mut conn.0).take(MAX_BRIDGE_REPLY_BYTES as u64 + 1).read_until(b'\n', &mut buf).await.map_err(io)?;
        if n == 0 {
            return Err(AutomationError::Bridge("the app closed the connection".into()));
        }
        if n > MAX_BRIDGE_REPLY_BYTES {
            // Not retried: the request may have completed.
            return Ok(Err(AutomationError::Bridge(format!("reply over {MAX_BRIDGE_REPLY_BYTES} bytes; the operation may have completed"))));
        }
        let Ok(v) = serde_json::from_slice::<Value>(&buf) else { continue };
        if v.get("id").and_then(Value::as_u64) != Some(id) {
            continue; // a stale reply to an earlier, timed-out request
        }
        return Ok(if v.get("ok").and_then(Value::as_bool) == Some(true) {
            Ok(v.get("result").cloned().unwrap_or(Value::Null))
        } else {
            Err(AutomationError::App(v.get("error").and_then(Value::as_str).unwrap_or("unknown error").to_string()))
        });
    }
}
