//! Localhost JSON-lines control server (one request per line, one reply per line). Loopback only.
//!
//! The server only ever reads requests: a line that is not a JSON object with a string `method`
//! (an HTTP request line from a browser's cross-origin `fetch`, a stray `nc`, binary junk) gets
//! one error reply and the connection is closed, so nothing after it is executed. Lines are
//! bounded ([`MAX_LINE`]) and so is the number of open connections ([`MAX_CONNECTIONS`]); junk
//! never reaches the UI thread. See `docs/control-protocol.md`.
//!
//! Loopback is not a security boundary (other users, containers, sandboxed apps), and the channel
//! can run scripts, so every connection must first authenticate (#2122): the app writes a fresh
//! per-launch token to `control-<port>.token` in the settings folder, readable by the user only,
//! and the first request must be `auth` with that token ([`effectcraft_automation::control_auth`]).
//! Anything else is answered with an error and the connection closed before it runs.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

use effectcraft_automation::control_auth;
use effectcraft_ui_egui::ControlRequest;
use serde_json::{Value, json};

/// Longest accepted request line (bytes, without the newline). Requests are small JSON objects;
/// anything longer is refused and the connection closed.
pub const MAX_LINE: usize = 4 * 1024 * 1024;
/// Connections served at once; further ones get an error line and are closed.
pub const MAX_CONNECTIONS: usize = 16;
/// Time from accepting a connection to its `auth` request (as in PhotoCraft). A connection that
/// hasn't authenticated by then is closed, so idle or slow-drip peers can't hold every slot.
pub const AUTH_DEADLINE: Duration = Duration::from_secs(10);

pub fn start(port: u16, ctx: egui::Context) -> Receiver<ControlRequest> {
    let (tx, rx) = channel::<ControlRequest>();
    let listener = match TcpListener::bind(("127.0.0.1", port)) {
        Ok(l) => l,
        Err(e) => {
            log::error!("control server failed to bind 127.0.0.1:{port}: {e}");
            return rx;
        }
    };
    // Only once the port is ours: a second instance that failed to bind mustn't replace the
    // running one's token.
    let token = match publish_token(port) {
        Ok(token) => token,
        Err(e) => {
            log::error!("control server not started: {e}");
            return rx;
        }
    };
    log::info!("control server listening on 127.0.0.1:{port}");
    std::thread::spawn(move || accept_loop(listener, tx, ctx, token.into(), AUTH_DEADLINE));
    rx
}

/// Generate this launch's token and write it where clients look for it. Never logged.
fn publish_token(port: u16) -> Result<String, String> {
    let path = control_auth::token_file_path_from_env(port, effectcraft_host::config_dir().as_deref()).map_err(|e| e.to_string())?;
    let token = control_auth::generate_token().map_err(|e| e.to_string())?;
    control_auth::write_token_file(&path, &token).map_err(|e| e.to_string())?;
    log::info!("control token file: {}", path.display());
    Ok(token)
}

fn accept_loop(listener: TcpListener, tx: Sender<ControlRequest>, ctx: egui::Context, token: Arc<str>, auth_deadline: Duration) {
    let open = Arc::new(AtomicUsize::new(0));
    for mut stream in listener.incoming().flatten() {
        if open.fetch_add(1, Ordering::SeqCst) >= MAX_CONNECTIONS {
            open.fetch_sub(1, Ordering::SeqCst);
            let _ = writeln!(stream, "{}", json!({"ok": false, "error": "too many control connections"}));
            continue;
        }
        let tx = tx.clone();
        let ctx = ctx.clone();
        let open = Arc::clone(&open);
        let token = Arc::clone(&token);
        std::thread::spawn(move || {
            serve(stream, &token, auth_deadline, &tx, &ctx);
            open.fetch_sub(1, Ordering::SeqCst);
        });
    }
}

fn serve(stream: TcpStream, token: &str, auth_deadline: Duration, tx: &Sender<ControlRequest>, ctx: &egui::Context) {
    let Ok(read) = stream.try_clone() else { return };
    serve_lines(read, stream, token, auth_deadline, |method, params| {
        let (req, rrx) = ControlRequest::new(method, params);
        tx.send(req).ok()?;
        ctx.request_repaint();
        Some(rrx.recv_timeout(Duration::from_secs(60)).unwrap_or_else(|_| json!({"ok": false, "error": "timeout"})))
    });
}

/// One parsed line.
enum Line {
    Request {
        id: Value,
        method: String,
        params: Value,
    },
    Blank,
    /// Not a request: reply with this error and close the connection.
    Reject(String),
}

fn parse_line(bytes: &[u8]) -> Line {
    let Ok(text) = std::str::from_utf8(bytes) else {
        return Line::Reject("not a control request (invalid UTF-8); closing the connection".into());
    };
    let text = text.trim();
    if text.is_empty() {
        return Line::Blank;
    }
    let msg = match serde_json::from_str::<Value>(text) {
        Ok(v) => v,
        Err(e) => return Line::Reject(format!("bad JSON ({e}); expected one JSON request object per line; closing the connection")),
    };
    let Some(obj) = msg.as_object() else {
        return Line::Reject("not a control request (expected a JSON object); closing the connection".into());
    };
    let Some(method) = obj.get("method").and_then(Value::as_str) else {
        return Line::Reject("not a control request (missing string `method`); closing the connection".into());
    };
    Line::Request { id: obj.get("id").cloned().unwrap_or(Value::Null), method: method.to_string(), params: obj.get("params").cloned().unwrap_or(json!({})) }
}

/// Read one `\n`-terminated line of at most `max` bytes into `buf` (newline stripped).
/// `Ok(false)` at end of stream; `Err` when the line is too long or the read fails.
fn read_bounded_line(reader: &mut impl BufRead, buf: &mut Vec<u8>, max: usize) -> Result<bool, String> {
    buf.clear();
    // `take` bounds the bytes buffered for one line: a peer can't make us allocate more than `max`.
    let limit = u64::try_from(max).unwrap_or(u64::MAX).saturating_add(1);
    let n = reader.take(limit).read_until(b'\n', buf).map_err(|e| e.to_string())?;
    if n == 0 {
        return Ok(false);
    }
    if buf.last() == Some(&b'\n') {
        buf.pop();
        if buf.last() == Some(&b'\r') {
            buf.pop();
        }
        return Ok(true);
    }
    if buf.len() > max {
        return Err(format!("request line longer than {max} bytes; closing the connection"));
    }
    // Last line without a newline, then end of stream.
    Ok(true)
}

/// A source whose blocking reads can be bounded: the socket, or an in-memory buffer in tests.
trait ReadTimeout {
    fn set_read_timeout(&self, timeout: Option<Duration>) -> std::io::Result<()>;
}

impl ReadTimeout for TcpStream {
    fn set_read_timeout(&self, timeout: Option<Duration>) -> std::io::Result<()> {
        TcpStream::set_read_timeout(self, timeout)
    }
}

impl ReadTimeout for &[u8] {
    fn set_read_timeout(&self, _: Option<Duration>) -> std::io::Result<()> {
        Ok(())
    }
}

/// Reads that fail once the connection's auth deadline has passed. The deadline is fixed when the
/// connection is accepted and checked before every read, with the socket's read timeout set to
/// what is left, so neither silence nor a byte-at-a-time drip can extend it.
struct AuthDeadlineRead<R> {
    inner: R,
    /// `None` once authenticated.
    deadline: Option<Instant>,
    limit: Duration,
}

impl<R: ReadTimeout> AuthDeadlineRead<R> {
    fn new(inner: R, limit: Duration) -> Self {
        // A limit too large to represent expires at once rather than never.
        let now = Instant::now();
        Self { inner, deadline: Some(now.checked_add(limit).unwrap_or(now)), limit }
    }

    /// The connection authenticated: no deadline from here on.
    fn authenticated(&mut self) -> std::io::Result<()> {
        self.deadline = None;
        self.inner.set_read_timeout(None)
    }

    fn expired(&self) -> std::io::Error {
        let secs = self.limit.as_secs_f64();
        std::io::Error::new(std::io::ErrorKind::TimedOut, format!("not authenticated within {secs} s of connecting; closing the connection"))
    }
}

impl<R: Read + ReadTimeout> Read for AuthDeadlineRead<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let Some(deadline) = self.deadline else { return self.inner.read(buf) };
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(self.expired());
        }
        self.inner.set_read_timeout(Some(left))?;
        match self.inner.read(buf) {
            // The socket timeout is what was left of the deadline.
            Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => Err(self.expired()),
            other => other,
        }
    }
}

/// The reply to anything but a valid `auth` on a connection that hasn't authenticated.
const AUTH_REQUIRED: &str = "authentication required: the first request must be {\"method\":\"auth\",\"params\":{\"token\":…}} \
     with the token from control-<port>.token in the EffectCraft settings folder, or from the file EFFECTCRAFT_CONTROL_TOKEN_FILE names; \
     closing the connection";

/// What happens to a request, given whether the connection has authenticated (#2122).
enum Gate {
    /// Authenticated: hand it to the app.
    Dispatch,
    /// Answered here (`auth`), without reaching the app.
    Reply(Value),
    /// Not authenticated: answer with this error and close the connection.
    Close(&'static str),
}

/// Only an `auth` request carrying `token` opens a connection; until then nothing is dispatched.
fn gate(authenticated: &mut bool, method: &str, params: &Value, token: &str) -> Gate {
    let is_auth = method == control_auth::AUTH_METHOD;
    if *authenticated {
        return if is_auth { Gate::Reply(json!({"ok": false, "error": "already authenticated"})) } else { Gate::Dispatch };
    }
    if is_auth && control_auth::auth_params_match(params, token) {
        *authenticated = true;
        return Gate::Reply(json!({"ok": true, "result": {"authenticated": true}}));
    }
    Gate::Close(AUTH_REQUIRED)
}

/// Serve requests from `read`, answering on `out`, until the peer closes, sends something that is
/// not a request, fails to authenticate with `token` within `auth_deadline` of connecting, or
/// `handle` returns `None` (app gone). `handle` gets `(method, params)` and returns the reply
/// object (the request's `id` is added here).
fn serve_lines<R: Read + ReadTimeout, W: Write>(
    read: R,
    mut out: W,
    token: &str,
    auth_deadline: Duration,
    mut handle: impl FnMut(String, Value) -> Option<Value>,
) {
    let mut reader = BufReader::new(AuthDeadlineRead::new(read, auth_deadline));
    let mut buf = Vec::new();
    let mut authenticated = false;
    loop {
        let reject = match read_bounded_line(&mut reader, &mut buf, MAX_LINE) {
            Ok(false) => return,
            Err(e) => e,
            Ok(true) => match parse_line(&buf) {
                Line::Blank => continue,
                Line::Reject(e) => e,
                Line::Request { id, method, params } => {
                    let was_authenticated = authenticated;
                    let reply = match gate(&mut authenticated, &method, &params, token) {
                        Gate::Dispatch => handle(method, params),
                        Gate::Reply(r) => {
                            if authenticated && !was_authenticated && reader.get_mut().authenticated().is_err() {
                                return;
                            }
                            Some(r)
                        }
                        Gate::Close(e) => {
                            let _ = writeln!(out, "{}", json!({"id": id, "ok": false, "error": e})).and_then(|()| out.flush());
                            return;
                        }
                    };
                    let Some(mut r) = reply else { return };
                    if let Some(o) = r.as_object_mut() {
                        o.insert("id".into(), id);
                    }
                    if writeln!(out, "{r}").and_then(|()| out.flush()).is_err() {
                        return;
                    }
                    continue;
                }
            },
        };
        let _ = writeln!(out, "{}", json!({"ok": false, "error": reject})).and_then(|()| out.flush());
        return;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use effectcraft_automation::{BridgeClient, TokenSource};

    const TOKEN: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    const EXECUTE: &str = r#"{"id":1,"method":"engine.execute","params":{"command":"file.new","params":{}}}"#;

    fn auth_line(token: &str) -> String {
        format!("{}\n", json!({"id": "auth", "method": "auth", "params": {"token": token}}))
    }

    /// Serve `input` as it is (no `auth` added); the replies and the methods that reached the app.
    fn run_raw(input: &[u8]) -> (Vec<Value>, Vec<String>) {
        let mut out = Vec::new();
        let mut calls = Vec::new();
        serve_lines(input, &mut out, TOKEN, AUTH_DEADLINE, |m, _p| {
            calls.push(m);
            Some(json!({"ok": true, "result": null}))
        });
        let replies = String::from_utf8(out).unwrap().lines().map(|l| serde_json::from_str::<Value>(l).unwrap()).collect();
        (replies, calls)
    }

    /// Serve `input` on an authenticated connection; the `auth` reply is checked and left out.
    fn run(input: &[u8]) -> (Vec<Value>, Vec<String>) {
        let mut full = auth_line(TOKEN).into_bytes();
        full.extend_from_slice(input);
        let (mut r, calls) = run_raw(&full);
        assert_eq!(r.remove(0), json!({"id": "auth", "ok": true, "result": {"authenticated": true}}));
        (r, calls)
    }

    /// #2122: without `auth` first, `engine.execute` never reaches the app and the connection closes.
    #[test]
    fn unauthenticated_execute_is_refused() {
        let (r, calls) = run_raw(format!("{EXECUTE}\n{EXECUTE}\n").as_bytes());
        assert!(calls.is_empty(), "ran without authenticating: {calls:?}");
        assert_eq!(r.len(), 1, "one error, then the connection is closed: {r:?}");
        assert_eq!((r[0]["id"].clone(), r[0]["ok"].clone()), (json!(1), json!(false)));
        assert!(r[0]["error"].as_str().unwrap().starts_with("authentication required"), "{r:?}");
    }

    #[test]
    fn wrong_or_missing_token_is_refused() {
        let wrong = "f".repeat(64);
        for first in [
            auth_line(&wrong),
            auth_line(&TOKEN[..63]),
            auth_line(""),
            "{\"method\":\"auth\"}\n".to_string(),
            "{\"method\":\"auth\",\"params\":{\"token\":5}}\n".to_string(),
        ] {
            let (r, calls) = run_raw(format!("{first}{EXECUTE}\n{}", auth_line(TOKEN)).as_bytes());
            assert!(calls.is_empty(), "{first}: {calls:?}");
            assert_eq!(r.len(), 1, "{first}: closed after the refusal: {r:?}");
            assert_eq!(r[0]["ok"], false);
            assert!(r[0]["error"].as_str().unwrap().contains("authentication required"));
        }
    }

    #[test]
    fn the_right_token_opens_the_connection() {
        let (r, calls) = run(format!("{EXECUTE}\n{}{{\"id\":2,\"method\":\"b\"}}\n", auth_line(TOKEN)).as_bytes());
        assert_eq!(calls, ["engine.execute", "b"], "a repeated `auth` is answered here, not dispatched");
        assert_eq!(r.len(), 3);
        assert_eq!((r[0]["id"].clone(), r[0]["ok"].clone()), (json!(1), json!(true)));
        assert_eq!(r[1], json!({"id": "auth", "ok": false, "error": "already authenticated"}));
        assert_eq!(r[2]["id"], 2);
        // Hex letters match in either case.
        let (r, calls) = run_raw(format!("{}{EXECUTE}\n", auth_line(&TOKEN.to_ascii_uppercase())).as_bytes());
        assert_eq!((r.len(), calls.len()), (2, 1));
    }

    #[test]
    fn requests_are_answered_in_order_with_ids() {
        let (r, calls) = run(b"{\"id\":1,\"method\":\"a\"}\n\n{\"id\":2,\"method\":\"b\",\"params\":{}}\r\n{\"method\":\"c\"}");
        assert_eq!(calls, ["a", "b", "c"]);
        assert_eq!(r.len(), 3);
        assert_eq!(r[0]["id"], 1);
        assert_eq!(r[1]["id"], 2);
        assert_eq!(r[2]["id"], Value::Null);
    }

    #[test]
    fn http_request_closes_before_the_body_runs() {
        // A browser's cross-origin `fetch` POST: the request line is rejected and the
        // connection closed, so the JSON body is never executed, authenticated or not.
        let http = b"POST / HTTP/1.1\r\nHost: 127.0.0.1:9877\r\nOrigin: https://attacker.example\r\nContent-Type: text/plain\r\n\r\n\
            {\"id\":7,\"method\":\"engine.execute\",\"params\":{\"command\":\"file.new\",\"params\":{}}}\n";
        for (r, calls) in [run(http), run_raw(http)] {
            assert!(calls.is_empty(), "nothing may run: {calls:?}");
            assert_eq!(r.len(), 1);
            assert_eq!(r[0]["ok"], false);
            assert!(r[0]["error"].as_str().unwrap().contains("closing the connection"));
        }
    }

    #[test]
    fn non_request_json_closes_the_connection() {
        for junk in [
            &b"[1,2]\n{\"method\":\"a\"}\n"[..],
            b"{\"id\":1}\n{\"method\":\"a\"}\n",
            b"{\"method\":5}\n{\"method\":\"a\"}\n",
            b"\xff\xfe\n{\"method\":\"a\"}\n",
        ] {
            let (r, calls) = run(junk);
            assert!(calls.is_empty(), "{calls:?}");
            assert_eq!(r.len(), 1);
            assert_eq!(r[0]["ok"], false);
        }
    }

    #[test]
    fn a_request_before_junk_still_runs() {
        let (r, calls) = run(b"{\"method\":\"a\"}\nGET / HTTP/1.1\n{\"method\":\"b\"}\n");
        assert_eq!(calls, ["a"]);
        assert_eq!(r.len(), 2);
        assert_eq!(r[1]["ok"], false);
    }

    #[test]
    fn overlong_line_is_refused() {
        // One byte over the limit, then a valid request: neither runs.
        let mut input = vec![b'x'; MAX_LINE + 1];
        input.extend_from_slice(b"\n{\"method\":\"a\"}\n");
        let (r, calls) = run(&input);
        assert!(calls.is_empty(), "{calls:?}");
        assert_eq!(r.len(), 1);
        assert!(r[0]["error"].as_str().unwrap().contains("longer than"));
    }

    #[test]
    fn line_at_the_limit_is_accepted() {
        let mut buf = Vec::new();
        let line = vec![b'a'; 10];
        let mut input = line.clone();
        input.push(b'\n');
        assert_eq!(read_bounded_line(&mut &input[..], &mut buf, 10), Ok(true));
        assert_eq!(buf, line);
        assert!(read_bounded_line(&mut &[b'a'; 11][..], &mut buf, 10).is_err());
    }

    #[test]
    fn tcp_connection_is_closed_after_junk() {
        // End to end over a real loopback socket (the transport the app uses).
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let addr = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (s, _) = listener.accept().unwrap();
            let read = s.try_clone().unwrap();
            serve_lines(read, s, TOKEN, AUTH_DEADLINE, |_, _| Some(json!({"ok": true})));
        });
        let mut c = TcpStream::connect(addr).unwrap();
        c.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        c.write_all(format!("{}{{\"id\":7,\"method\":\"x\"}}\nhello\n{{\"method\":\"y\"}}\n", auth_line(TOKEN)).as_bytes()).unwrap();
        let mut text = String::new();
        c.read_to_string(&mut text).unwrap(); // returns at EOF: the server closed the connection
        server.join().unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 3, "{text}");
        assert!(lines[1].contains("\"id\":7"));
        assert!(lines[2].contains("closing the connection"));
    }

    /// The bridge client (`effectcraft-cli --bridge`, MCP bridge) reads the token file the app
    /// writes and authenticates against the real handshake; a stale token is refused.
    #[test]
    fn bridge_client_authenticates_with_the_token_file() {
        let dir = std::env::temp_dir().join(format!("ec-control-server-auth-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let token = control_auth::generate_token().unwrap();
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let path = control_auth::token_file_path(port, None, Some(&dir)).unwrap();
        control_auth::write_token_file(&path, &token).unwrap();
        let served = token.clone();
        let server = std::thread::spawn(move || {
            for _ in 0..2 {
                let (s, _) = listener.accept().unwrap();
                let read = s.try_clone().unwrap();
                serve_lines(read, s, &served, AUTH_DEADLINE, |m, _| Some(json!({"ok": true, "result": {"method": m}})));
            }
        });
        // Explicit inputs: an inherited EFFECTCRAFT_CONTROL_TOKEN(_FILE) must not pick the token.
        let mut client = BridgeClient::new(&port.to_string(), TokenSource::from_inputs(&port.to_string(), None, None, Some(&dir))).unwrap();
        assert_eq!(client.call("engine.execute", json!({})).unwrap()["method"].as_str(), Some("engine.execute"));
        drop(client);
        let mut stale = BridgeClient::new(&port.to_string(), TokenSource::Token(control_auth::generate_token().unwrap())).unwrap();
        let error = stale.call("engine.execute", json!({})).unwrap_err().to_string();
        assert!(error.contains("authentication required"), "{error}");
        server.join().unwrap();
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn extra_connections_are_refused() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let addr = listener.local_addr().unwrap();
        let (tx, _rx) = channel::<ControlRequest>();
        std::thread::spawn(move || accept_loop(listener, tx, egui::Context::default(), TOKEN.into(), AUTH_DEADLINE));
        // Idle connections are accepted in order and held open.
        let held: Vec<TcpStream> = (0..16).map(|_| TcpStream::connect(addr).unwrap()).collect();
        let mut extra = TcpStream::connect(addr).unwrap();
        extra.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let mut line = String::new();
        BufReader::new(&mut extra).read_line(&mut line).unwrap();
        assert!(line.contains("too many control connections"), "{line}");
        drop(held);
    }

    /// Connections that don't authenticate in time are closed and free their slots, whether they
    /// stay silent or drip a byte at a time.
    #[test]
    fn unauthenticated_connections_are_closed_at_the_deadline() {
        let deadline = Duration::from_millis(300);
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let addr = listener.local_addr().unwrap();
        let (tx, _rx) = channel::<ControlRequest>();
        std::thread::spawn(move || accept_loop(listener, tx, egui::Context::default(), TOKEN.into(), deadline));
        let start = Instant::now();
        let idle: Vec<TcpStream> = (0..MAX_CONNECTIONS - 1).map(|_| TcpStream::connect(addr).unwrap()).collect();
        let drip = TcpStream::connect(addr).unwrap();
        let mut writer = drip.try_clone().unwrap();
        std::thread::spawn(move || {
            while start.elapsed() < Duration::from_secs(5) {
                let _ = writer.write_all(b" ");
                std::thread::sleep(Duration::from_millis(50));
            }
        });
        for c in &idle {
            c.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            let mut text = String::new();
            (&mut &*c).read_to_string(&mut text).unwrap(); // EOF: the server closed it
            assert!(text.contains("not authenticated within 0.3 s"), "{text}");
        }
        drip.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        // Closed (EOF or reset, as unread drip bytes may make the close a reset), not timed out.
        let mut text = String::new();
        if let Err(e) = (&mut &drip).read_to_string(&mut text) {
            assert!(!matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut), "still open: {e}");
        }
        assert!(start.elapsed() < Duration::from_secs(3), "the drip extended the deadline: {:?}", start.elapsed());
        // The slots are free again: a new client authenticates.
        let mut reply = String::new();
        for _ in 0..50 {
            let mut c = TcpStream::connect(addr).unwrap();
            c.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            c.write_all(auth_line(TOKEN).as_bytes()).unwrap();
            reply.clear();
            BufReader::new(&mut c).read_line(&mut reply).unwrap();
            if !reply.contains("too many") {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(reply.contains("\"authenticated\":true"), "{reply}");
    }

    /// Once authenticated, the deadline no longer applies.
    #[test]
    fn authenticated_connections_outlive_the_deadline() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let addr = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (s, _) = listener.accept().unwrap();
            let read = s.try_clone().unwrap();
            serve_lines(read, s, TOKEN, Duration::from_millis(200), |_, _| Some(json!({"ok": true})));
        });
        let mut c = TcpStream::connect(addr).unwrap();
        c.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        c.write_all(auth_line(TOKEN).as_bytes()).unwrap();
        std::thread::sleep(Duration::from_millis(500));
        c.write_all(b"{\"id\":2,\"method\":\"x\"}\n").unwrap();
        c.shutdown(std::net::Shutdown::Write).unwrap();
        let mut text = String::new();
        c.read_to_string(&mut text).unwrap();
        server.join().unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2, "{text}");
        assert!(lines[1].contains("\"id\":2") && lines[1].contains("\"ok\":true"), "{text}");
    }
}
