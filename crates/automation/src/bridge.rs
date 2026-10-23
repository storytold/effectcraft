//! Blocking client for the desktop app's JSON-lines control channel (`docs/control-protocol.md`).
//! One persistent loopback connection. Only explicitly read-only queries can be resent.

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

use serde_json::{Value, json};

use crate::control_auth::{TokenSource, auth_request};
use crate::{Error, Result};

/// Slightly longer than the app's own 60 s per-request timeout, so its error reply wins.
const READ_TIMEOUT: Duration = Duration::from_secs(65);
/// How long the app may take to answer `auth` (it doesn't wait on the UI thread).
const AUTH_TIMEOUT: Duration = Duration::from_secs(10);

pub struct BridgeClient {
    addr: String,
    token: TokenSource,
    conn: Option<(BufReader<TcpStream>, TcpStream)>,
    next_id: u64,
}

impl BridgeClient {
    /// `addr`: a port (`9877`) or `host:port` on loopback (`127.0.0.1`, `localhost`, `[::1]`).
    /// Every connection starts with `auth`, the token read from `token` at that moment (#2122).
    pub fn new(addr: &str, token: TokenSource) -> Result<Self> {
        let addr = if addr.parse::<u16>().is_ok() { format!("127.0.0.1:{addr}") } else { addr.to_string() };
        let host = addr.rsplit_once(':').map(|(h, _)| h).unwrap_or(&addr);
        if !matches!(host, "127.0.0.1" | "localhost" | "[::1]" | "::1") {
            return Err(Error::BadArgs(format!("bridge address must be loopback (PORT or 127.0.0.1:PORT), got `{addr}`")));
        }
        Ok(Self { addr, token, conn: None, next_id: 1 })
    }

    pub fn addr(&self) -> &str {
        &self.addr
    }

    fn connect(&self) -> Result<(BufReader<TcpStream>, TcpStream)> {
        let sa = self
            .addr
            .to_socket_addrs()
            .map_err(|e| Error::Bridge(format!("{}: {e}", self.addr)))?
            .next()
            .ok_or_else(|| Error::Bridge(format!("cannot resolve {}", self.addr)))?;
        let s = TcpStream::connect_timeout(&sa, Duration::from_secs(5))
            .map_err(|e| Error::Bridge(format!("cannot connect to {} ({e}); start the app with `effectcraft --control <port>`", self.addr)))?;
        s.set_nodelay(true).ok();
        let r = s.try_clone().map_err(|e| Error::Bridge(e.to_string()))?;
        let mut conn = (BufReader::new(r), s);
        // Read the token now, not at construction: the app writes a new one each launch.
        let token = self.token.token()?;
        authenticate(&mut conn, &token)?;
        conn.1.set_read_timeout(Some(READ_TIMEOUT)).ok();
        Ok(conn)
    }

    /// Call a control method; returns the reply's `result`, or the app's `error` as [`Error::App`].
    pub fn call(&mut self, method: &str, params: Value) -> Result<Value> {
        let retry_read =
            matches!(method, "engine.commands" | "ui.menu.list" | "ui.inspect" | "ui.elements" | "ui.viewer.locate" | "ui.viewer.hit" | "ui.timeline.locate")
                || (method == "ui.playback" && params["action"] == "status");
        let unknown = |error| Error::Bridge(format!("`{method}` request outcome is unknown: {error}; inspect the app before retrying"));
        let mut last = Error::Bridge("not attempted".into());
        for attempt in 0..2 {
            if self.conn.is_none() {
                self.conn = Some(self.connect()?);
            }
            let id = self.next_id;
            self.next_id = self.next_id.checked_add(1).ok_or_else(|| Error::Bridge("control request id space exhausted".into()))?;
            let Some((r, w)) = self.conn.as_mut() else { continue };
            let line = format!("{}\n", json!({"id": id, "method": method, "params": params}));
            // `Ok(Err(_))`: transmission/reply failed (the app may have applied the request);
            // `Err(_)`: a timeout or bad reply (never resent: the app may have run the request).
            let res = (|| -> Result<std::result::Result<Value, Error>> {
                if let Err(e) = w.write_all(line.as_bytes()) {
                    return Ok(Err(Error::Bridge(e.to_string())));
                }
                let mut buf = String::new();
                match r.read_line(&mut buf) {
                    Ok(0) => return Ok(Err(Error::Bridge("connection closed by the app".into()))),
                    Ok(_) => {}
                    Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {
                        return Err(Error::Bridge(format!("no reply to `{method}` within {}s (is the app window running?)", READ_TIMEOUT.as_secs())));
                    }
                    Err(e) => return Ok(Err(Error::Bridge(e.to_string()))),
                }
                serde_json::from_str(&buf).map(Ok).map_err(|e| Error::Bridge(format!("bad reply: {e}")))
            })();
            match res {
                Ok(Ok(v)) if v.get("ok").and_then(Value::as_bool) == Some(true) => return Ok(v.get("result").cloned().unwrap_or(Value::Null)),
                Ok(Ok(v)) => return Err(Error::App(v.get("error").and_then(Value::as_str).unwrap_or("error").to_string())),
                Ok(Err(e)) => {
                    self.conn = None;
                    if !retry_read {
                        return Err(unknown(e));
                    }
                    // A read-only query can be repeated after a stale connection or lost reply.
                    last = e;
                    if attempt == 1 {
                        break;
                    }
                }
                Err(e) => {
                    // The stream may still deliver the late reply: start fresh next call.
                    self.conn = None;
                    return Err(if retry_read { e } else { unknown(e) });
                }
            }
        }
        Err(last)
    }
}

/// Send `auth` on a fresh connection and check the app accepted it. Nothing else has been sent,
/// so a failure here never leaves a request's outcome unknown.
fn authenticate((r, w): &mut (BufReader<TcpStream>, TcpStream), token: &str) -> Result<()> {
    w.set_read_timeout(Some(AUTH_TIMEOUT)).ok();
    let failed = |e: String| Error::Bridge(format!("control channel authentication failed: {e}"));
    w.write_all(format!("{}\n", auth_request(token)).as_bytes()).map_err(|e| failed(e.to_string()))?;
    let mut buf = String::new();
    match r.read_line(&mut buf) {
        Ok(0) => return Err(failed("connection closed by the app".into())),
        Ok(_) => {}
        Err(e) => return Err(failed(e.to_string())),
    }
    let reply: Value = serde_json::from_str(&buf).map_err(|e| failed(format!("bad reply: {e}")))?;
    if reply.get("ok").and_then(Value::as_bool) == Some(true) {
        return Ok(());
    }
    let why = reply.get("error").and_then(Value::as_str).unwrap_or("refused");
    Err(failed(format!("the app refused the control token: {why}")))
}
