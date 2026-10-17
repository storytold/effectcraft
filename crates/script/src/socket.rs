//! `Socket`: TCP connections for scripts (the JavaScript Tools Guide's Socket object: `open`,
//! `listen`, `poll`, `read`, `readln`, `write`, `writeln`, `close`, `connected`, `eof`).
//!
//! Network access follows Preferences ▸ Scripting & Expressions ▸ Allow Scripts to Write Files
//! and Access Network: with it off, `new Socket()` throws. On the web there are no raw sockets:
//! `open` and `listen` return false (with `error` set).
//!
//! The JavaScript side holds only a handle; the streams live here, keyed by handle, so a script
//! window's handlers can keep using a socket opened earlier in the same script.

use boa_engine::{Context, JsResult, JsValue};
use serde_json::{Value as J, json};

use crate::runtime::{arg_string, js_str, throw, with_active};

/// `__sock(op, handle, paramsJson)` → result JSON (`{error}` on failure).
pub(crate) fn native_sock(_: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let op = arg_string(args, 0, ctx)?;
    let id = args.get(1).cloned().unwrap_or_default().to_number(ctx)? as u32;
    let params = arg_string(args, 2, ctx)?;
    let p: J = serde_json::from_str(&params).unwrap_or(J::Null);
    let allowed = with_active(|a| Ok(a.session.prefs.scripting.allow_scripts_write_files)).unwrap_or(false);
    if !allowed {
        return Err(throw(format!("scripts can't access the network unless {} is on", crate::runtime::GATE)));
    }
    if op == "check" {
        return Ok(js_str("{}"));
    }
    let r = imp::run(&op, id, &p);
    Ok(js_str(&r.to_string()))
}

fn err(e: impl std::fmt::Display) -> J {
    json!({"error": e.to_string()})
}

#[cfg(target_arch = "wasm32")]
mod imp {
    use super::*;
    pub(super) fn run(op: &str, _: u32, _: &J) -> J {
        match op {
            "connected" | "eof" => json!({"value": op == "eof"}),
            "close" => json!({}),
            _ => err("sockets are not available in the web version of EffectCraft"),
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
mod imp {
    use std::collections::BTreeMap;
    use std::io::{ErrorKind, Read, Write};
    use std::net::{Shutdown, TcpListener, TcpStream, ToSocketAddrs};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    use super::*;

    enum Sock {
        Stream { s: TcpStream, buf: Vec<u8>, eof: bool },
        Listener(TcpListener),
    }

    /// One open socket. Reading, writing and polling lock only it, never the whole table, so a
    /// socket blocked on a slow peer doesn't hold up the others (#1248).
    struct Entry {
        sock: Mutex<Sock>,
        /// A second handle on a stream, so `close` can shut it down while another call is
        /// blocked reading or writing it.
        wake: Option<TcpStream>,
    }

    struct Table {
        next: u32,
        socks: BTreeMap<u32, Arc<Entry>>,
    }

    fn table() -> &'static Mutex<Table> {
        static T: Mutex<Table> = Mutex::new(Table { next: 1, socks: BTreeMap::new() });
        &T
    }

    fn add(s: Sock) -> u32 {
        let wake = match &s {
            Sock::Stream { s, .. } => s.try_clone().ok(),
            Sock::Listener(_) => None,
        };
        let entry = Arc::new(Entry { sock: Mutex::new(s), wake });
        let mut t = table().lock().unwrap_or_else(|e| e.into_inner());
        let id = t.next;
        t.next += 1;
        t.socks.insert(id, entry);
        id
    }

    /// The socket `id`, if open. The table is locked only for the lookup.
    fn get(id: u32) -> Option<Arc<Entry>> {
        table().lock().unwrap_or_else(|e| e.into_inner()).socks.get(&id).cloned()
    }

    /// Forget socket `id`; a call blocked on it fails at once.
    fn close(id: u32) {
        let entry = table().lock().unwrap_or_else(|e| e.into_inner()).socks.remove(&id);
        if let Some(w) = entry.as_ref().and_then(|e| e.wake.as_ref()) {
            let _ = w.shutdown(Shutdown::Both);
        }
    }

    fn timeout(p: &J) -> Duration {
        Duration::from_secs_f64(p.get("timeout").and_then(J::as_f64).unwrap_or(10.0).clamp(0.0, 3600.0))
    }

    /// `host:port` (After Effects' form) → socket addresses.
    fn addr(host: &str) -> Result<Vec<std::net::SocketAddr>, String> {
        let a = host.to_socket_addrs().map_err(|e| format!("bad address `{host}` (use host:port): {e}"))?;
        Ok(a.collect())
    }

    /// Text ↔ bytes in the socket's encoding: UTF-8, else one byte per character (ASCII /
    /// BINARY, as in After Effects).
    fn encode(s: &str, utf8: bool) -> Vec<u8> {
        if utf8 { s.as_bytes().to_vec() } else { s.chars().map(|c| (c as u32).min(255) as u8).collect() }
    }
    fn decode(b: &[u8], utf8: bool) -> String {
        if utf8 { String::from_utf8_lossy(b).into_owned() } else { b.iter().map(|&c| c as char).collect() }
    }

    /// Fill a stream's buffer until `done(buf)` or the timeout / end of stream. Fails only when
    /// the read deadline can't be set.
    fn fill(s: &mut TcpStream, buf: &mut Vec<u8>, eof: &mut bool, limit: Duration, done: impl Fn(&[u8]) -> bool) -> std::io::Result<()> {
        let end = Instant::now() + limit;
        let mut chunk = [0u8; 4096];
        while !done(buf) && !*eof {
            let left = end.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            s.set_read_timeout(Some(left.max(Duration::from_millis(1))))?;
            match s.read(&mut chunk) {
                Ok(0) => *eof = true,
                Ok(n) => buf.extend_from_slice(&chunk[..n]),
                Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut | ErrorKind::Interrupted) => {
                    if e.kind() != ErrorKind::Interrupted {
                        break;
                    }
                }
                Err(_) => *eof = true,
            }
        }
        Ok(())
    }

    pub(super) fn run(op: &str, id: u32, p: &J) -> J {
        let utf8 = p.get("encoding").and_then(J::as_str).is_some_and(|e| e.eq_ignore_ascii_case("utf-8") || e.eq_ignore_ascii_case("utf8"));
        match op {
            "open" => {
                let host = p.get("host").and_then(J::as_str).unwrap_or("");
                let addrs = match addr(host) {
                    Ok(a) => a,
                    Err(e) => return err(e),
                };
                let limit = timeout(p).max(Duration::from_millis(10));
                let mut last = "no address".to_string();
                for a in addrs {
                    match TcpStream::connect_timeout(&a, limit) {
                        Ok(s) => {
                            let _ = s.set_nodelay(true);
                            return json!({"id": add(Sock::Stream { s, buf: vec![], eof: false })});
                        }
                        Err(e) => last = e.to_string(),
                    }
                }
                err(format!("cannot connect to {host}: {last}"))
            }
            "listen" => {
                let port = match p.get("port").and_then(J::as_f64).unwrap_or(0.0) {
                    v if v.fract() == 0.0 && (0.0..=65535.0).contains(&v) => v as u16,
                    v => return err(format!("bad port {v} (0 to 65535)")),
                };
                match TcpListener::bind(("127.0.0.1", port)).and_then(|l| l.set_nonblocking(true).map(|_| l)) {
                    Ok(l) => {
                        let port = l.local_addr().map(|a| a.port()).unwrap_or(port);
                        json!({"id": add(Sock::Listener(l)), "port": port})
                    }
                    Err(e) => err(format!("cannot listen on port {port}: {e}")),
                }
            }
            "close" => {
                close(id);
                json!({})
            }
            _ => {
                let Some(entry) = get(id) else {
                    return match op {
                        "connected" => json!({"value": false}),
                        "eof" => json!({"value": true}),
                        _ => err("the socket is not open"),
                    };
                };
                let mut sock = entry.sock.lock().unwrap_or_else(|e| e.into_inner());
                match (op, &mut *sock) {
                    ("poll", Sock::Listener(l)) => match l.accept() {
                        Ok((s, peer)) => {
                            if let Err(e) = s.set_nonblocking(false) {
                                return err(format!("cannot use the connection from {peer}: {e}"));
                            }
                            let host = peer.to_string();
                            json!({"id": add(Sock::Stream { s, buf: vec![], eof: false }), "host": host})
                        }
                        Err(_) => json!({"id": null}),
                    },
                    ("connected", Sock::Stream { s, buf, eof }) => {
                        if !*eof && buf.is_empty() {
                            // Peek without blocking to notice a closed peer.
                            let _ = s.set_nonblocking(true);
                            let mut b = [0u8; 1];
                            match s.peek(&mut b) {
                                Ok(0) => *eof = true,
                                Err(e) if e.kind() != ErrorKind::WouldBlock => *eof = true,
                                _ => {}
                            }
                            let _ = s.set_nonblocking(false);
                        }
                        json!({"value": !(*eof && buf.is_empty())})
                    }
                    ("connected", Sock::Listener(_)) => json!({"value": true}),
                    ("eof", Sock::Stream { buf, eof, .. }) => json!({"value": *eof && buf.is_empty()}),
                    ("read", Sock::Stream { s, buf, eof }) => {
                        let count = p.get("count").and_then(J::as_i64).unwrap_or(-1);
                        let limit = timeout(p);
                        if count < 0 {
                            if let Err(e) = fill(s, buf, eof, limit, |_| false) {
                                return err(format!("cannot set the read timeout: {e}"));
                            }
                            let all = std::mem::take(buf);
                            json!({"data": decode(&all, utf8)})
                        } else {
                            let n = count as usize;
                            if let Err(e) = fill(s, buf, eof, limit, |b| b.len() >= n) {
                                return err(format!("cannot set the read timeout: {e}"));
                            }
                            let take = n.min(buf.len());
                            let out: Vec<u8> = buf.drain(..take).collect();
                            json!({"data": decode(&out, utf8)})
                        }
                    }
                    ("readln", Sock::Stream { s, buf, eof }) => {
                        if let Err(e) = fill(s, buf, eof, timeout(p), |b| b.contains(&b'\n')) {
                            return err(format!("cannot set the read timeout: {e}"));
                        }
                        let line: Vec<u8> = match buf.iter().position(|&c| c == b'\n') {
                            Some(i) => {
                                let mut l: Vec<u8> = buf.drain(..=i).collect();
                                l.pop();
                                if l.last() == Some(&b'\r') {
                                    l.pop();
                                }
                                l
                            }
                            None => std::mem::take(buf),
                        };
                        json!({"data": decode(&line, utf8)})
                    }
                    ("write", Sock::Stream { s, .. }) => {
                        // A peer that stops reading fills the buffers: give up after `timeout`
                        // instead of blocking the script for good (#1248).
                        let limit = timeout(p).max(Duration::from_millis(1));
                        if let Err(e) = s.set_write_timeout(Some(limit)) {
                            return err(format!("cannot set the write timeout: {e}"));
                        }
                        let data = p.get("data").and_then(J::as_str).unwrap_or("");
                        match s.write_all(&encode(data, utf8)).and_then(|_| s.flush()) {
                            Ok(()) => json!({"ok": true}),
                            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => err(format!(
                                "write timed out after {} s: the other side isn't reading (some of the data may have been sent)",
                                limit.as_secs_f64()
                            )),
                            Err(e) => err(e),
                        }
                    }
                    ("host", Sock::Stream { s, .. }) => json!({"value": s.peer_addr().map(|a| a.to_string()).unwrap_or_default()}),
                    (op, _) => err(format!("`{op}` is not supported on this socket")),
                }
            }
        }
    }
}

#[cfg(test)]
#[cfg(not(target_arch = "wasm32"))]
mod tests {
    use std::net::{TcpListener, TcpStream};
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    use serde_json::{Value as J, json};

    use super::imp::run;

    /// A loopback peer that accepts one connection and never reads from it; returns its
    /// `host:port` and the accepted stream's keeper (drop it to let the connection go).
    fn stalled_peer() -> (String, mpsc::Sender<()>) {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let host = l.local_addr().unwrap().to_string();
        let (stop, wait) = mpsc::channel::<()>();
        std::thread::spawn(move || {
            let (_s, _): (TcpStream, _) = l.accept().unwrap();
            let _ = wait.recv_timeout(Duration::from_secs(120));
        });
        (host, stop)
    }

    fn open(host: &str) -> u32 {
        let r = run("open", 0, &json!({"host": host, "timeout": 5}));
        r["id"].as_u64().unwrap_or_else(|| panic!("open {host}: {r}")) as u32
    }

    /// More than the loopback buffers hold, so a peer that doesn't read stops the write.
    fn big() -> String {
        "x".repeat(64 * 1024 * 1024)
    }

    /// Run `f` on a thread; its result, or `None` if it takes longer than `limit`.
    fn within<T: Send + 'static>(limit: Duration, f: impl FnOnce() -> T + Send + 'static) -> Option<T> {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(f());
        });
        rx.recv_timeout(limit).ok()
    }

    #[test]
    fn a_write_to_a_peer_that_stops_reading_fails_after_the_timeout() {
        // #1248: it used to block for good.
        let (host, _keep) = stalled_peer();
        let id = open(&host);
        let data = big();
        let t0 = Instant::now();
        let r = within(Duration::from_secs(20), move || run("write", id, &json!({"data": data, "timeout": 1}))).expect("the write gave up");
        assert!(r["error"].as_str().is_some_and(|e| e.contains("timed out")), "{r}");
        assert!(t0.elapsed() < Duration::from_secs(15), "{:?}", t0.elapsed());
        run("close", id, &J::Null);
    }

    #[test]
    fn a_blocked_socket_doesnt_hold_up_the_others_and_close_interrupts_it() {
        let (host_a, _keep_a) = stalled_peer();
        let (host_b, _keep_b) = stalled_peer();
        let (a, b) = (open(&host_a), open(&host_b));
        // A write on A that would wait a minute.
        let data = big();
        let (tx, blocked) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(run("write", a, &json!({"data": data, "timeout": 60})));
        });
        std::thread::sleep(Duration::from_millis(500));
        // Socket B answers meanwhile (the table isn't locked by A's write).
        let r = within(Duration::from_secs(3), move || run("connected", b, &json!({}))).expect("socket B answered while A was blocked");
        assert_eq!(r["value"], true, "{r}");
        // Closing A returns at once and ends A's write.
        within(Duration::from_secs(3), move || run("close", a, &J::Null)).expect("close returned while A was blocked");
        let r = blocked.recv_timeout(Duration::from_secs(5)).expect("A's write ended when A was closed");
        assert!(r.get("error").is_some(), "{r}");
        run("close", b, &J::Null);
    }
}
