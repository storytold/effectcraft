//! Control endpoints of the desktop app. Two listeners feed one request queue.
//!
//! The Craft Link socket (`effectcraft-<pid>` under the runtime dir, token handshake in craft-ipc)
//! is always on; [`start_with`] publishes it. `--control <port>` adds the localhost JSON-lines TCP
//! server below (one request per line, one reply per line). Loopback only, no token.
//!
//! The TCP server only ever reads requests: a line that is not a JSON object with a string
//! `method` gets one error reply and the connection is closed. Lines are bounded ([`MAX_LINE`])
//! and so is the number of open connections ([`MAX_CONNECTIONS`]); junk never reaches the UI
//! thread. See `docs/control-protocol.md`.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use craft_ipc::bulk::FrameTickets;
use craft_ipc::server::{Activate, ControlSink, Frames, Server, ServerConfig};
use effectcraft_ui_egui::ControlRequest;
use serde_json::{Value, json};

/// Longest accepted request line (bytes, without the newline). Requests are small JSON objects;
/// anything longer is refused and the connection closed.
pub const MAX_LINE: usize = 4 * 1024 * 1024;
/// Connections served at once; further ones get an error line and are closed.
pub const MAX_CONNECTIONS: usize = 16;

/// The running link server. The next [`start_with`] drops it. Drop joins threads and releases
/// the lock. It does not call [`Server::shutdown`].
static SERVER: Mutex<Option<Server>> = Mutex::new(None);

/// The Craft Link socket for `runtime` (the real runtime dir for `None`), plus the localhost TCP
/// server when `tcp_port` is set. Both feed the receiver that comes back. A failed start is
/// logged and nothing arrives from that listener.
pub fn start_with(runtime: Option<craft_ipc::RuntimeDir>, tcp_port: Option<u16>, ctx: egui::Context) -> Receiver<ControlRequest> {
    let (tx, rx) = channel::<ControlRequest>();
    start_link(runtime, tx.clone(), ctx.clone());
    if let Some(port) = tcp_port {
        start_tcp(port, tx, ctx);
    }
    rx
}

fn start_link(runtime: Option<craft_ipc::RuntimeDir>, tx: Sender<ControlRequest>, ctx: egui::Context) {
    // Release the previous lock before bind. Same pid cannot publish a second record while the
    // first server still holds `effectcraft-<pid>.lock`.
    *server_slot() = None;
    let sink: Arc<dyn ControlSink> = Arc::new(UiSink { tx, ctx: ctx.clone() });
    // `app.activate` runs on the UI thread, behind the same request channel as every other method.
    let activate: Arc<dyn Activate> = Arc::new(effectcraft_link::SinkActivate::new(Arc::clone(&sink)));
    // Frames are pulled from the newest snapshot the UI thread publishes (`live::tick`).
    let wake_ctx = ctx;
    let comp_frames = effectcraft_link::live::install(Arc::new(emit), Arc::new(move || wake_ctx.request_repaint()));
    let frames: Arc<dyn Frames> = Arc::new(FrameTickets::new(comp_frames));
    let config = ServerConfig {
        app: "effectcraft".to_string(),
        app_version: env!("CARGO_PKG_VERSION").to_string(),
        instance: None,
        runtime,
        tcp_port: None,
        sink,
        activate,
        frames,
    };
    match Server::start(config) {
        Ok(server) => *server_slot() = Some(server),
        Err(err) => log::error!("control server failed to start: {err}"),
    }
}

fn start_tcp(port: u16, tx: Sender<ControlRequest>, ctx: egui::Context) {
    let listener = match TcpListener::bind(("127.0.0.1", port)) {
        Ok(l) => l,
        Err(e) => {
            log::error!("control server failed to bind 127.0.0.1:{port}: {e}");
            return;
        }
    };
    log::info!("control server listening on 127.0.0.1:{port}");
    std::thread::spawn(move || accept_loop(listener, tx, ctx));
}

fn server_slot() -> std::sync::MutexGuard<'static, Option<Server>> {
    match SERVER.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

struct UiSink {
    tx: Sender<ControlRequest>,
    ctx: egui::Context,
}

impl ControlSink for UiSink {
    fn submit(&self, method: String, params: Value) -> Option<Receiver<Value>> {
        let (req, reply) = ControlRequest::new(method, params);
        self.tx.send(req).ok()?;
        self.ctx.request_repaint();
        Some(reply)
    }
}

/// Queue a push event on the running link server. Quiet when none runs.
fn emit(event: &str, data: Value) {
    if let Some(server) = server_slot().as_ref() {
        let _ = server.emit(event, data);
    }
}

fn accept_loop(listener: TcpListener, tx: Sender<ControlRequest>, ctx: egui::Context) {
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
        std::thread::spawn(move || {
            serve(stream, &tx, &ctx);
            open.fetch_sub(1, Ordering::SeqCst);
        });
    }
}

fn serve(stream: TcpStream, tx: &Sender<ControlRequest>, ctx: &egui::Context) {
    let Ok(read) = stream.try_clone() else { return };
    serve_lines(read, stream, |method, params| {
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

/// Serve requests from `read`, answering on `out`, until the peer closes, sends something that is
/// not a request, or `handle` returns `None` (app gone). `handle` gets `(method, params)` and
/// returns the reply object (the request's `id` is added here).
fn serve_lines<R: Read, W: Write>(read: R, mut out: W, mut handle: impl FnMut(String, Value) -> Option<Value>) {
    let mut reader = BufReader::new(read);
    let mut buf = Vec::new();
    loop {
        let reject = match read_bounded_line(&mut reader, &mut buf, MAX_LINE) {
            Ok(false) => return,
            Err(e) => e,
            Ok(true) => match parse_line(&buf) {
                Line::Blank => continue,
                Line::Reject(e) => e,
                Line::Request { id, method, params } => {
                    let Some(mut r) = handle(method, params) else { return };
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

    fn run(input: &[u8]) -> (Vec<Value>, Vec<String>) {
        let mut out = Vec::new();
        let mut calls = Vec::new();
        serve_lines(input, &mut out, |m, _p| {
            calls.push(m);
            Some(json!({"ok": true, "result": null}))
        });
        let replies = String::from_utf8(out).unwrap().lines().map(|l| serde_json::from_str::<Value>(l).unwrap()).collect();
        (replies, calls)
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
        // connection closed, so the JSON body is never executed.
        let http = b"POST / HTTP/1.1\r\nHost: 127.0.0.1:9877\r\nOrigin: https://attacker.example\r\nContent-Type: text/plain\r\n\r\n\
            {\"id\":7,\"method\":\"engine.execute\",\"params\":{\"command\":\"file.new\",\"params\":{}}}\n";
        let (r, calls) = run(http);
        assert!(calls.is_empty(), "nothing may run: {calls:?}");
        assert_eq!(r.len(), 1);
        assert_eq!(r[0]["ok"], false);
        assert!(r[0]["error"].as_str().unwrap().contains("closing the connection"));
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
            serve_lines(read, s, |_, _| Some(json!({"ok": true})));
        });
        let mut c = TcpStream::connect(addr).unwrap();
        c.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        c.write_all(b"{\"id\":7,\"method\":\"x\"}\nhello\n{\"method\":\"y\"}\n").unwrap();
        let mut text = String::new();
        c.read_to_string(&mut text).unwrap(); // returns at EOF: the server closed the connection
        server.join().unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2, "{text}");
        assert!(lines[0].contains("\"id\":7"));
        assert!(lines[1].contains("closing the connection"));
    }

    #[test]
    fn extra_connections_are_refused() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let addr = listener.local_addr().unwrap();
        let (tx, _rx) = channel::<ControlRequest>();
        std::thread::spawn(move || accept_loop(listener, tx, egui::Context::default()));
        // Idle connections are accepted in order and held open.
        let held: Vec<TcpStream> = (0..16).map(|_| TcpStream::connect(addr).unwrap()).collect();
        let mut extra = TcpStream::connect(addr).unwrap();
        extra.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let mut line = String::new();
        BufReader::new(&mut extra).read_line(&mut line).unwrap();
        assert!(line.contains("too many control connections"), "{line}");
        drop(held);
    }

    /// `Client::call` returns the result payload, or a `LinkError` for `ok: false`.
    /// Null means the reply was flat: a wrap would return the inner object, and a missing id
    /// would not match the request.
    #[test]
    fn link_socket_passes_finished_replies() {
        let pid = std::process::id();
        let tick = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
        let dir = std::path::PathBuf::from(format!("/tmp/ec-ctrl-{pid}-{tick:x}"));
        let _ = std::fs::remove_dir_all(&dir);
        let runtime = craft_ipc::RuntimeDir::at(&dir).unwrap();
        let _keep = RemoveDir(dir);
        let rx = start_with(Some(runtime.clone()), None, egui::Context::default());
        let worker = std::thread::spawn(move || {
            let first = rx.recv_timeout(Duration::from_secs(5)).expect("first request");
            first.reply.send(json!({"ok": true, "result": null})).expect("first reply");
            let second = rx.recv_timeout(Duration::from_secs(5)).expect("second request");
            second.reply.send(json!({"ok": false, "error": "missing command"})).expect("second reply");
        });
        let records = craft_ipc::Discovery::scan(&runtime).unwrap();
        assert_eq!(records.len(), 1, "{records:?}");
        assert_eq!(records[0].app, "effectcraft");
        let mut client = craft_ipc::client::Client::connect(&records[0]).unwrap();
        let first = client.call("engine.commands", json!({})).expect("first call");
        assert_eq!(first, Value::Null, "ok result must be null and not nested");
        match client.call("engine.commands", json!({})).expect_err("second call") {
            craft_ipc::LinkError::Invalid(text) => assert_eq!(text, "missing command"),
            other => panic!("expected the string error, got {other}"),
        }
        worker.join().expect("reply thread");
    }

    struct RemoveDir(std::path::PathBuf);

    impl Drop for RemoveDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}
