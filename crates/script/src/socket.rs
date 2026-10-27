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

    enum Sock {
        Stream { s: TcpStream, buf: Vec<u8>, eof: bool },
        Listener(TcpListener),
    }

    /// One open socket. Reading, writing and polling lock only it, never the whole table, so a
    /// socket blocked on a slow peer doesn't hold up the others (#1248).
    struct Entry {
        sock: Mutex<Sock>,
        /// A second handle on a stream, so `close` can wake a blocked read.
        wake: Option<TcpStream>,
        /// Closed or unusable after a mode error. Writes poll it because Windows sends may not
        /// be interrupted by shutdown alone; queued operations recheck it after locking.
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
        let mut t = table().lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let id = t.next;
        t.next += 1;
        t.socks.insert(id, entry);
        id
    }

    /// The socket `id`, if open. The table is locked only for the lookup.
    fn get(id: u32) -> Option<Arc<Entry>> {
        table().lock().unwrap_or_else(std::sync::PoisonError::into_inner).socks.get(&id).cloned()
    }

    /// Forget socket `id`; a call blocked on it fails at once.
    fn close(id: u32) {
        let entry = table().lock().unwrap_or_else(std::sync::PoisonError::into_inner).socks.remove(&id);
        if let Some(entry) = entry {
            entry.closed.store(true, Ordering::Release);
            if let Some(w) = &entry.wake {
                let _ = w.shutdown(Shutdown::Both);
            }
        }
    }

    /// A test-only handle for establishing backpressure before exercising the public operations.
    #[cfg(test)]
    pub(super) fn test_stream(id: u32) -> TcpStream {
        let entry = get(id).unwrap();
        let sock = entry.sock.lock().unwrap();
        let Sock::Stream { s, .. } = &*sock else { panic!("expected a stream") };
        s.try_clone().unwrap()
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

    /// A failed mode transition leaves the stream unusable; reject queued operations and wake I/O.
    fn change_mode(s: &TcpStream, entry: &Entry, nonblocking: bool, mode: &impl Fn(&TcpStream, bool) -> std::io::Result<()>) -> std::io::Result<()> {
        mode(s, nonblocking).map_err(|e| {
            entry.closed.store(true, Ordering::Release);
            let _ = s.shutdown(Shutdown::Both);
            let action = if nonblocking { "enable nonblocking mode" } else { "restore blocking mode" };
            std::io::Error::other(format!("socket is unusable: cannot {action}: {e}"))
        })
    }

    /// Write with one deadline and an explicit close signal. On Windows, shutdown alone does
    /// not reliably interrupt a blocking send, so never leave a send blocked inside the OS.
    /// The caller holds this socket's lock; restore blocking mode or retire the connection.
    fn write(s: &mut TcpStream, data: &[u8], limit: Duration, entry: &Entry, mode: &impl Fn(&TcpStream, bool) -> std::io::Result<()>) -> std::io::Result<()> {
        change_mode(s, entry, true, mode)?;
        let end = Instant::now() + limit;
        let result = (|| {
            let mut pending = data;
            while !pending.is_empty() {
                if entry.closed.load(Ordering::Acquire) {
                    return Err(std::io::Error::new(ErrorKind::ConnectionAborted, "the socket was closed"));
                }
                let left = end.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    return Err(std::io::Error::from(ErrorKind::TimedOut));
                }
                match s.write(pending) {
                    Ok(0) => return Err(std::io::Error::from(ErrorKind::WriteZero)),
                    Ok(n) => pending = pending.get(n..).ok_or_else(|| std::io::Error::from(ErrorKind::InvalidData))?,
                    Err(e) if e.kind() == ErrorKind::WouldBlock => std::thread::sleep(left.min(Duration::from_millis(5))),
                    Err(e) if e.kind() == ErrorKind::Interrupted => continue,
                    Err(e) => return Err(e),
                }
            }
            Ok(())
        })();
        match (result, change_mode(s, entry, false, mode)) {
            (Err(write), Err(restore)) => Err(std::io::Error::other(format!("{restore}; write also failed: {write}"))),
            (result, Ok(())) => result,
            (Ok(()), Err(restore)) => Err(restore),
        }
    }

    /// Dispatch an authorized operation without holding the socket table during I/O.
    pub(super) fn run(op: &str, id: u32, p: &J) -> J {
        run_with_mode(op, id, p, TcpStream::set_nonblocking)
    }

    /// Dispatch with a mode setter so exceptional OS failures can be injected per operation.
    pub(super) fn run_with_mode(op: &str, id: u32, p: &J, mode: impl Fn(&TcpStream, bool) -> std::io::Result<()>) -> J {
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
                let mut sock = entry.sock.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                // Recheck after locking: close or a failed mode transition may have retired it
                // while this caller waited with an Arc obtained from the table.
                if entry.closed.load(Ordering::Acquire) {
                    return match op {
                        "connected" => json!({"value": false}),
                        "eof" => json!({"value": true}),
                        _ => err("the socket is closed or unusable"),
                    };
                }
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
                            if let Err(e) = change_mode(s, &entry, true, &mode) {
                                return err(e);
                            }
                            let mut b = [0u8; 1];
                            match s.peek(&mut b) {
                                Ok(0) => *eof = true,
                                Err(e) if e.kind() != ErrorKind::WouldBlock => *eof = true,
                                _ => {}
                            }
                            if let Err(e) = change_mode(s, &entry, false, &mode) {
                                return err(e);
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
                        let data = p.get("data").and_then(J::as_str).unwrap_or("");
                        match write(s, &encode(data, utf8), limit, &entry, &mode) {
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
    use std::io::{ErrorKind, Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    use serde_json::{Value as J, json};

    use super::imp::run;

    /// A loopback peer that accepts one connection and never reads from it; returns its
    /// `host:port` and the accepted stream's keeper (drop it to let the connection go).
    fn stalled_peer() -> (String, mpsc::Sender<()>) {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        socket2::SockRef::from(&l).set_recv_buffer_size(4096).unwrap();
        let host = l.local_addr().unwrap().to_string();
        let (stop, wait) = mpsc::channel::<()>();
        std::thread::spawn(move || {
            let (_s, _): (TcpStream, _) = l.accept().unwrap();
            let _ = wait.recv_timeout(Duration::from_secs(120));
        });
        (host, stop)
    }

    /// Open a test connection through the production socket dispatcher.
    fn open(host: &str) -> u32 {
        let r = run("open", 0, &json!({"host": host, "timeout": 5}));
        r["id"].as_u64().unwrap_or_else(|| panic!("open {host}: {r}")) as u32
    }

    /// Fill bounded socket buffers nonblocking, rather than assuming a 64 MiB write stalls.
    /// In particular, Windows may accept that entire write with its default send buffering.
    fn backpressure(id: u32) {
        let mut stream = super::imp::test_stream(id);
        socket2::SockRef::from(&stream).set_send_buffer_size(4096).unwrap();
        stream.set_nonblocking(true).unwrap();
        let start = Instant::now();
        let chunk = [b'x'; 4096];
        let mut sent = 0;
        loop {
            assert!(sent < 16 * 1024 * 1024 && start.elapsed() < Duration::from_secs(5), "could not establish backpressure");
            match stream.write(&chunk) {
                Ok(0) => panic!("peer closed while establishing backpressure"),
                Ok(n) => sent += n,
                Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == ErrorKind::Interrupted => continue,
                Err(e) => panic!("establishing backpressure: {e}"),
            }
        }
        stream.set_nonblocking(false).unwrap();
    }

    /// Run `f` on a thread; its result, or `None` if it takes longer than `limit`.
    fn within<T: Send + 'static>(limit: Duration, f: impl FnOnce() -> T + Send + 'static) -> Option<T> {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(f());
        });
        rx.recv_timeout(limit).ok()
    }

    /// A bounded write times out even when the peer never drains its receive buffer.
    #[test]
    fn a_write_to_a_peer_that_stops_reading_fails_after_the_timeout() {
        // #1248: it used to block for good.
        let (host, _keep) = stalled_peer();
        let id = open(&host);
        backpressure(id);
        let data = "x".repeat(64 * 1024);
        let t0 = Instant::now();
        let r = within(Duration::from_secs(20), move || run("write", id, &json!({"data": data, "timeout": 1}))).expect("the write gave up");
        assert!(r["error"].as_str().is_some_and(|e| e.contains("timed out")), "{r}");
        assert!(t0.elapsed() >= Duration::from_millis(500) && t0.elapsed() < Duration::from_secs(15), "{:?}", t0.elapsed());
        run("close", id, &J::Null);
    }

    /// Closing a stalled stream interrupts its writer without blocking another socket.
    #[test]
    fn a_blocked_socket_doesnt_hold_up_the_others_and_close_interrupts_it() {
        let (host_a, _keep_a) = stalled_peer();
        let (host_b, _keep_b) = stalled_peer();
        let (a, b) = (open(&host_a), open(&host_b));
        // A write on A that would wait a minute.
        backpressure(a);
        let data = "x".repeat(64 * 1024);
        let (tx, blocked) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(run("write", a, &json!({"data": data, "timeout": 60})));
        });
        std::thread::sleep(Duration::from_millis(500));
        assert!(matches!(blocked.try_recv(), Err(mpsc::TryRecvError::Empty)), "A must still be writing before close");
        // Socket B answers meanwhile (the table isn't locked by A's write).
        let r = within(Duration::from_secs(3), move || run("connected", b, &json!({}))).expect("socket B answered while A was blocked");
        assert_eq!(r["value"], true, "{r}");
        // Closing A returns at once and ends A's write.
        within(Duration::from_secs(3), move || run("close", a, &J::Null)).expect("close returned while A was blocked");
        let r = blocked.recv_timeout(Duration::from_secs(5)).expect("A's write ended when A was closed");
        assert!(r.get("error").is_some(), "{r}");
        run("close", b, &J::Null);
    }

    /// Failed mode changes retire the stream; subsequent I/O fails instead of using it.
    #[test]
    fn mode_restoration_failure_retires_streams() {
        for (op, fail_nonblocking) in [("write", false), ("connected", false), ("write", true), ("connected", true)] {
            let (host, _keep) = stalled_peer();
            let id = open(&host);
            let result = super::imp::run_with_mode(op, id, &json!({"data": "hello"}), |s, nonblocking| {
                if nonblocking == fail_nonblocking { Err(std::io::Error::other("injected mode failure")) } else { s.set_nonblocking(nonblocking) }
            });
            assert!(result["error"].as_str().is_some_and(|e| e.contains("injected mode failure")), "{op}: {result}");
            assert_eq!(run("connected", id, &J::Null)["value"], false, "{op}: broken socket remains connected");
            assert_eq!(run("eof", id, &J::Null)["value"], true);
            for operation in ["read", "readln", "write"] {
                let r = run(operation, id, &json!({"count": 1, "timeout": 0.01, "data": "x"}));
                assert!(r.get("error").is_some(), "{operation} must reject the unusable stream: {r}");
            }
            run("close", id, &J::Null);
            run("close", id, &J::Null);
        }
    }

    /// A preceding write error cannot hide the failure that makes the stream unusable.
    #[test]
    fn restoration_failure_preserves_the_write_error_too() {
        let (host, _keep) = stalled_peer();
        let id = open(&host);
        let result = super::imp::run_with_mode("write", id, &json!({"data": "hello"}), |s, nonblocking| {
            if nonblocking {
                s.set_nonblocking(true)?;
                s.shutdown(std::net::Shutdown::Write)
            } else {
                Err(std::io::Error::other("injected restore failure"))
            }
        });
        let error = result["error"].as_str().unwrap();
        assert!(error.contains("injected restore failure") && error.contains("write also failed"), "{error}");
        assert_eq!(run("connected", id, &J::Null)["value"], false);
        run("close", id, &J::Null);
    }

    /// Both outgoing and accepted sockets deliver complete data, then return to blocking reads.
    #[test]
    fn writes_deliver_all_data_and_restore_blocking_reads() {
        for accepted in [false, true] {
            let (id, mut peer) = if accepted {
                let listener = run("listen", 0, &json!({"port": 0}));
                let listener_id = listener["id"].as_u64().unwrap() as u32;
                let peer = TcpStream::connect(("127.0.0.1", listener["port"].as_u64().unwrap() as u16)).unwrap();
                let start = Instant::now();
                let id = loop {
                    if let Some(id) = run("poll", listener_id, &J::Null)["id"].as_u64() {
                        break id as u32;
                    }
                    assert!(start.elapsed() < Duration::from_secs(5));
                    std::thread::sleep(Duration::from_millis(1));
                };
                run("close", listener_id, &J::Null);
                (id, peer)
            } else {
                let listener = TcpListener::bind("127.0.0.1:0").unwrap();
                let id = open(&listener.local_addr().unwrap().to_string());
                (id, listener.accept().unwrap().0)
            };
            socket2::SockRef::from(&super::imp::test_stream(id)).set_send_buffer_size(4096).unwrap();
            peer.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            let reader = std::thread::spawn(move || {
                let mut data = vec![0; 256 * 1024];
                peer.read_exact(&mut data).unwrap();
                assert!(data.iter().all(|b| *b == b'x'));
                // A read on a socket accidentally left nonblocking would return before this.
                std::thread::sleep(Duration::from_millis(100));
                peer.write_all(b"ack").unwrap();
            });
            let r = within(Duration::from_secs(10), move || run("write", id, &json!({"data": "x".repeat(256 * 1024), "timeout": 5}))).unwrap();
            assert_eq!(r["ok"], true, "{r}");
            let r = run("read", id, &json!({"count": 3, "timeout": 2}));
            assert_eq!(r["data"], "ack", "{r}");
            run("close", id, &J::Null);
            reader.join().unwrap();
        }
    }
}
