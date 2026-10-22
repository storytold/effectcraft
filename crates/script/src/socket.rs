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
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    use super::*;

    /// Streams are non-blocking: on Windows `shutdown` doesn't wake a call blocked on another
    /// handle of the socket, so reads and writes wait in short naps and notice `close` in between.
    enum Sock {
        Stream { s: TcpStream, buf: Vec<u8>, eof: bool },
        Listener(TcpListener),
    }

    /// One open socket. Reading, writing and polling lock only it, never the whole table, so a
    /// socket blocked on a slow peer doesn't hold up the others (#1248).
    struct Entry {
        sock: Mutex<Sock>,
        /// A second handle on a stream, so `close` can shut it down while another call is
        /// waiting to read or write it.
        wake: Option<TcpStream>,
        /// Set by `close`; a call waiting on the socket gives up at its next try.
        closed: AtomicBool,
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
        let entry = Arc::new(Entry { sock: Mutex::new(s), wake, closed: AtomicBool::new(false) });
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

    /// Forget socket `id`; a call waiting on it gives up at once.
    fn close(id: u32) {
        let entry = table().lock().unwrap_or_else(|e| e.into_inner()).socks.remove(&id);
        if let Some(e) = &entry {
            e.closed.store(true, Ordering::Release);
        }
        if let Some(w) = entry.as_ref().and_then(|e| e.wake.as_ref()) {
            let _ = w.shutdown(Shutdown::Both);
        }
    }

    /// The first and the longest nap of a call waiting on a stream that would block: short at
    /// first so replies come quickly, longer while the other side stays quiet.
    const NAP: (Duration, Duration) = (Duration::from_millis(1), Duration::from_millis(10));

    /// Nap before trying a stream that would block again; `false` once `end` has passed or the
    /// socket was closed.
    fn wait(end: Instant, closed: &AtomicBool, nap: &mut Duration) -> bool {
        let left = end.saturating_duration_since(Instant::now());
        if left.is_zero() || closed.load(Ordering::Acquire) {
            return false;
        }
        std::thread::sleep((*nap).min(left));
        *nap = nap.saturating_mul(2).min(NAP.1);
        true
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

    /// Fill a stream's buffer until `done(buf)`, the timeout, the end of the stream or `close`.
    fn fill(s: &mut TcpStream, buf: &mut Vec<u8>, eof: &mut bool, limit: Duration, closed: &AtomicBool, done: impl Fn(&[u8]) -> bool) {
        let end = Instant::now() + limit;
        let mut chunk = [0u8; 4096];
        let mut nap = NAP.0;
        while !done(buf) && !*eof {
            match s.read(&mut chunk) {
                Ok(0) => *eof = true,
                Ok(n) => {
                    buf.extend_from_slice(chunk.get(..n).unwrap_or_default());
                    nap = NAP.0;
                }
                Err(e) if e.kind() == ErrorKind::Interrupted => {}
                Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
                    if !wait(end, closed, &mut nap) {
                        break;
                    }
                }
                Err(_) => *eof = true,
            }
        }
    }

    /// Write all of `data` within `limit`: `WouldBlock` if the other side stops reading,
    /// `NotConnected` if the socket is closed meanwhile.
    fn send(s: &mut TcpStream, mut data: &[u8], limit: Duration, closed: &AtomicBool) -> std::io::Result<()> {
        let end = Instant::now() + limit;
        let mut nap = NAP.0;
        while !data.is_empty() {
            match s.write(data) {
                Ok(0) => return Err(ErrorKind::WriteZero.into()),
                Ok(n) => {
                    data = data.get(n..).unwrap_or_default();
                    nap = NAP.0;
                }
                Err(e) if e.kind() == ErrorKind::Interrupted => {}
                Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
                    if !wait(end, closed, &mut nap) {
                        let closed = closed.load(Ordering::Acquire);
                        return Err(if closed { std::io::Error::new(ErrorKind::NotConnected, "the socket was closed") } else { e });
                    }
                }
                Err(e) => return Err(e),
            }
        }
        s.flush()
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
                            if let Err(e) = s.set_nonblocking(true) {
                                return err(format!("cannot use the connection to {host}: {e}"));
                            }
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
                            if let Err(e) = s.set_nonblocking(true) {
                                return err(format!("cannot use the connection from {peer}: {e}"));
                            }
                            let host = peer.to_string();
                            json!({"id": add(Sock::Stream { s, buf: vec![], eof: false }), "host": host})
                        }
                        Err(_) => json!({"id": null}),
                    },
                    ("connected", Sock::Stream { s, buf, eof }) => {
                        if !*eof && buf.is_empty() {
                            // Peek (the stream doesn't block) to notice a closed peer.
                            let mut b = [0u8; 1];
                            match s.peek(&mut b) {
                                Ok(0) => *eof = true,
                                Err(e) if e.kind() != ErrorKind::WouldBlock => *eof = true,
                                _ => {}
                            }
                        }
                        json!({"value": !(*eof && buf.is_empty())})
                    }
                    ("connected", Sock::Listener(_)) => json!({"value": true}),
                    ("eof", Sock::Stream { buf, eof, .. }) => json!({"value": *eof && buf.is_empty()}),
                    ("read", Sock::Stream { s, buf, eof }) => {
                        let count = p.get("count").and_then(J::as_i64).unwrap_or(-1);
                        let limit = timeout(p);
                        if count < 0 {
                            fill(s, buf, eof, limit, &entry.closed, |_| false);
                            let all = std::mem::take(buf);
                            json!({"data": decode(&all, utf8)})
                        } else {
                            let n = count as usize;
                            fill(s, buf, eof, limit, &entry.closed, |b| b.len() >= n);
                            let take = n.min(buf.len());
                            let out: Vec<u8> = buf.drain(..take).collect();
                            json!({"data": decode(&out, utf8)})
                        }
                    }
                    ("readln", Sock::Stream { s, buf, eof }) => {
                        fill(s, buf, eof, timeout(p), &entry.closed, |b| b.contains(&b'\n'));
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
                        let data = p.get("data").and_then(J::as_str).unwrap_or("");
                        match send(s, &encode(data, utf8), limit, &entry.closed) {
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

    /// Write to `id` until a write times out, so the next one has to wait. Windows takes a whole
    /// write into its buffers while they have room, however big it is, so the first writes may
    /// go through.
    fn fill_up(id: u32) {
        let data = big();
        for _ in 0..8 {
            let r = run("write", id, &json!({"data": data, "timeout": 0.2}));
            if let Some(e) = r.get("error") {
                assert!(e.as_str().is_some_and(|e| e.contains("timed out")), "{r}");
                return;
            }
        }
        panic!("the writes to socket {id} never filled its buffers");
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
        within(Duration::from_secs(60), move || fill_up(id)).expect("the writes gave up");
        let data = big();
        let t0 = Instant::now();
        let r = within(Duration::from_secs(20), move || run("write", id, &json!({"data": data, "timeout": 1}))).expect("the write gave up");
        assert!(r["error"].as_str().is_some_and(|e| e.contains("timed out")), "{r}");
        assert!(t0.elapsed() >= Duration::from_millis(900) && t0.elapsed() < Duration::from_secs(15), "{:?}", t0.elapsed());
        run("close", id, &J::Null);
    }

    #[test]
    fn a_blocked_socket_doesnt_hold_up_the_others_and_close_interrupts_it() {
        let (host_a, _keep_a) = stalled_peer();
        let (host_b, _keep_b) = stalled_peer();
        let (a, b) = (open(&host_a), open(&host_b));
        // A write on A that would wait a minute.
        within(Duration::from_secs(60), move || fill_up(a)).expect("the writes gave up");
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

    #[test]
    fn close_ends_a_read_that_waits_for_a_quiet_peer() {
        // On Windows `shutdown` doesn't wake a blocked read: it waited out its timeout.
        let (host, _keep) = stalled_peer();
        let id = open(&host);
        let (tx, waiting) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(run("read", id, &json!({"count": 1, "timeout": 60})));
        });
        std::thread::sleep(Duration::from_millis(300));
        within(Duration::from_secs(3), move || run("close", id, &J::Null)).expect("close returned while the read waited");
        let r = waiting.recv_timeout(Duration::from_secs(5)).expect("the read ended when the socket was closed");
        assert_eq!(r["data"], "", "{r}");
    }
}
