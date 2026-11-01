//! EffectCraft's side of a helper: the child process, its pipes and frame exchange file, and
//! the [`EffectPlugin`] each remote effect is registered as.
//!
//! Nothing a helper does can take EffectCraft down: a helper that exits, hangs past its
//! timeout or answers nonsense is stopped, the render that hit it fails with a message (shown on
//! the effect, the frame is left as it was), and the next render starts the helper again. An
//! effect that took its helper down [`crate::blocklist::CRASH_LIMIT`] times is blocked.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::ThreadId;
use std::time::{Duration, Instant};

use effectcraft_effects::plugin::{EffectPlugin, PluginFrame, PluginHost, PluginImage, PluginManifest, PluginParams};

use crate::blocklist;
use crate::frames::Exchange;
use crate::protocol::{HELPER_ARG, ImageRef, MAX_LINE, PROTOCOL_VERSION, RemoteEffect, RenderRequest, Reply, Request, parse_reply, request_line};

/// How deep renders through one helper may nest (an effect asking for its input, which another
/// effect of the same helper renders, ...).
const MAX_DEPTH: u32 = 16;
/// Restarts allowed per minute before a helper is left stopped until the next scan.
const MAX_RESTARTS_PER_MINUTE: usize = 5;
/// A helper idle for longer than this is pinged before it is used again.
const HEARTBEAT_AFTER: Duration = Duration::from_secs(30);
const HEARTBEAT_TIMEOUT: Duration = Duration::from_secs(10);

/// [`set_render_timeout`] (milliseconds; 0 = not set).
static RENDER_TIMEOUT_MS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Override the render timeout (tests; `EFFECTCRAFT_HELPER_TIMEOUT` still wins).
pub fn set_render_timeout(t: Duration) {
    RENDER_TIMEOUT_MS.store(u64::try_from(t.as_millis()).unwrap_or(u64::MAX), std::sync::atomic::Ordering::Relaxed);
}

/// Timeout for one render step, `EFFECTCRAFT_HELPER_TIMEOUT` seconds (default 120: heavy
/// plug-ins take a while on their first frame).
fn render_timeout() -> Duration {
    if let Some(s) = env_secs("EFFECTCRAFT_HELPER_TIMEOUT") {
        return Duration::from_secs(s);
    }
    match RENDER_TIMEOUT_MS.load(std::sync::atomic::Ordering::Relaxed) {
        0 => Duration::from_secs(120),
        ms => Duration::from_millis(ms),
    }
}

/// Timeout for start-up, `describe` and `load`, `EFFECTCRAFT_HELPER_LOAD_TIMEOUT` seconds
/// (default 600: scanning large plug-in suites is slow).
fn load_timeout() -> Duration {
    Duration::from_secs(env_secs("EFFECTCRAFT_HELPER_LOAD_TIMEOUT").unwrap_or(600))
}

fn env_secs(k: &str) -> Option<u64> {
    std::env::var(k).ok()?.trim().parse().ok().filter(|s| *s > 0)
}

pub(crate) fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// What the reader thread passes on.
enum Line {
    Reply(Result<Reply, String>),
    Closed,
}

/// A running helper process.
struct Conn {
    child: Child,
    stdin: ChildStdin,
    rx: Receiver<Line>,
    exchange: Exchange,
    /// The first free byte of the exchange file (images are stacked, see [`Helper::render`]).
    top: u64,
    last_used: Instant,
    generation: u64,
}

impl Conn {
    fn send(&mut self, r: &Request) -> Result<(), String> {
        let line = request_line(r)?;
        self.stdin.write_all(line.as_bytes()).and_then(|_| self.stdin.flush()).map_err(|e| format!("can't write to the helper: {e}"))
    }

    fn recv(&mut self, timeout: Duration) -> Result<Reply, Failure> {
        match self.rx.recv_timeout(timeout) {
            Ok(Line::Reply(Ok(r))) => Ok(r),
            Ok(Line::Reply(Err(e))) => Err(Failure::Broken(e)),
            Ok(Line::Closed) | Err(RecvTimeoutError::Disconnected) => {
                // Give the process a moment to finish exiting so its status can be reported.
                std::thread::sleep(Duration::from_millis(50));
                let status = match self.child.try_wait() {
                    Ok(Some(s)) => s.to_string(),
                    _ => "closed its output".into(),
                };
                Err(Failure::Broken(format!("the helper stopped ({status})")))
            }
            Err(RecvTimeoutError::Timeout) => Err(Failure::Broken(format!("the helper did not answer within {} s", timeout.as_secs()))),
        }
    }

    fn call(&mut self, r: &Request, timeout: Duration) -> Result<Reply, Failure> {
        self.send(r).map_err(Failure::Broken)?;
        self.recv(timeout)
    }

    /// Reserve room for an image of `width × height` at the top of the exchange file.
    fn alloc(&mut self, width: u32, height: u32, origin: [f64; 2], scale: f64) -> Result<ImageRef, String> {
        let at = ImageRef { offset: self.top, width, height, origin, scale };
        let bytes = at.bytes().ok_or_else(|| format!("image {width}×{height} is too large for a helper"))?;
        // Keep images 64-byte aligned for helpers that map the file.
        self.top = self.top.checked_add(bytes.div_ceil(64).saturating_mul(64)).ok_or("frame exchange file too large")?;
        Ok(at)
    }

    fn stop(mut self) {
        let _ = self.send(&Request::Shutdown);
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Why a request failed.
enum Failure {
    /// The helper answered with an error and is fine.
    Refused(String),
    /// The helper crashed, hung or broke the protocol: it is stopped.
    Broken(String),
}

/// Whose turn it is to talk to the helper: one thread at a time, re-entered by that thread for
/// nested renders.
#[derive(Default)]
struct Turn {
    owner: Option<ThreadId>,
    depth: u32,
}

struct TurnGuard<'a>(&'a Helper);

impl Drop for TurnGuard<'_> {
    fn drop(&mut self) {
        let mut t = lock(&self.0.turn);
        t.depth = t.depth.saturating_sub(1);
        if t.depth == 0 {
            t.owner = None;
            self.0.turn_cv.notify_all();
        }
    }
}

/// What [`Helper::status`] reports.
#[derive(Clone, Debug, Default)]
pub struct HelperStatus {
    pub program: PathBuf,
    pub name: String,
    pub version: String,
    pub loads: Vec<String>,
    pub running: bool,
    pub effects: usize,
    pub restarts: usize,
    pub last_error: Option<String>,
}

/// One helper program and, while it runs, its process.
pub struct Helper {
    program: PathBuf,
    info: Mutex<HelperStatus>,
    conn: Mutex<Option<Conn>>,
    turn: Mutex<Turn>,
    turn_cv: Condvar,
    /// Paths loaded with [`Helper::load`], loaded again after a restart.
    loaded: Mutex<Vec<String>>,
    restarts: Mutex<Vec<Instant>>,
    generation: Mutex<u64>,
}

impl Helper {
    /// Start a helper and list its effects.
    pub fn start(program: &Path) -> Result<(Arc<Helper>, Vec<RemoteEffect>, Vec<String>), String> {
        let h = Arc::new(Helper {
            program: program.to_path_buf(),
            info: Mutex::new(HelperStatus { program: program.to_path_buf(), ..Default::default() }),
            conn: Mutex::new(None),
            turn: Mutex::new(Turn::default()),
            turn_cv: Condvar::new(),
            loaded: Mutex::new(vec![]),
            restarts: Mutex::new(vec![]),
            generation: Mutex::new(0),
        });
        let (effects, errors) = {
            let _turn = h.take_turn()?;
            // A helper that crashes while listing its effects (one bad plug-in) is given a
            // second and third chance: helpers remember what crashed them and skip it.
            let mut last = String::new();
            let mut out = None;
            for _ in 0..3 {
                match h.with_conn(|c| c.call(&Request::Describe, load_timeout())) {
                    Ok(Reply::Effects { effects, errors }) => {
                        out = Some((effects, errors));
                        break;
                    }
                    Ok(other) => last = format!("unexpected reply to describe: {other:?}"),
                    Err(Failure::Refused(e) | Failure::Broken(e)) => last = e,
                }
            }
            out.ok_or_else(|| format!("{}: {last}", program.display()))?
        };
        lock(&h.info).effects = effects.len();
        Ok((h, effects, errors))
    }

    pub fn program(&self) -> &Path {
        &self.program
    }

    pub fn status(&self) -> HelperStatus {
        let mut s = lock(&self.info).clone();
        s.running = lock(&self.conn).is_some();
        s.restarts = lock(&self.restarts).len();
        s
    }

    pub fn loads(&self, ext: &str) -> bool {
        lock(&self.info).loads.iter().any(|e| e.eq_ignore_ascii_case(ext))
    }

    pub(crate) fn add_effects(&self, n: usize) {
        let mut i = lock(&self.info);
        i.effects = i.effects.saturating_add(n);
    }

    /// Load effects from a path through the helper ([`Request::Load`]).
    pub fn load(&self, path: &str) -> Result<(Vec<RemoteEffect>, Vec<String>), String> {
        let _turn = self.take_turn()?;
        let r = self.with_conn(|c| c.call(&Request::Load { path: path.to_string() }, load_timeout()));
        match r {
            Ok(Reply::Effects { effects, errors }) => {
                let mut l = lock(&self.loaded);
                if !l.iter().any(|p| p == path) {
                    l.push(path.to_string());
                }
                Ok((effects, errors))
            }
            Ok(Reply::Error { message }) => Err(message),
            Ok(other) => Err(format!("unexpected reply to load: {other:?}")),
            Err(Failure::Refused(e) | Failure::Broken(e)) => Err(e),
        }
    }

    /// Stop the process (it starts again when next needed).
    pub fn stop(&self) {
        if let Some(c) = lock(&self.conn).take() {
            c.stop();
        }
    }

    fn take_turn(&self) -> Result<TurnGuard<'_>, String> {
        let me = std::thread::current().id();
        let mut t = lock(&self.turn);
        loop {
            match t.owner {
                None => {
                    t.owner = Some(me);
                    t.depth = 1;
                    break;
                }
                Some(o) if o == me => {
                    if t.depth >= MAX_DEPTH {
                        return Err("effects through this helper are nested too deeply".into());
                    }
                    t.depth += 1;
                    break;
                }
                Some(_) => t = self.turn_cv.wait(t).unwrap_or_else(PoisonError::into_inner),
            }
        }
        Ok(TurnGuard(self))
    }

    /// Run `f` on the running process, starting it first if needed. A [`Failure::Broken`]
    /// stops the process.
    fn with_conn<T>(&self, f: impl FnOnce(&mut Conn) -> Result<T, Failure>) -> Result<T, Failure> {
        let mut g = lock(&self.conn);
        if g.is_none() {
            *g = Some(self.spawn().map_err(|e| self.not_started(e))?);
        }
        let Some(c) = g.as_mut() else { return Err(Failure::Broken("the helper is not running".into())) };
        if c.last_used.elapsed() > HEARTBEAT_AFTER && !matches!(c.call(&Request::Ping, HEARTBEAT_TIMEOUT), Ok(Reply::Pong)) {
            if let Some(dead) = g.take() {
                dead.stop();
            }
            self.note_error("the helper stopped answering; restarting it");
            *g = Some(self.spawn().map_err(|e| self.not_started(e))?);
        }
        let Some(c) = g.as_mut() else { return Err(Failure::Broken("the helper is not running".into())) };
        let r = f(c);
        c.last_used = Instant::now();
        if let Err(Failure::Broken(e)) = &r {
            self.note_error(e);
            if let Some(dead) = g.take() {
                dead.stop();
            }
        }
        r
    }

    /// A helper that could not be started (not installed any more, restarting too often, no
    /// frame exchange file): reported, but not charged to the effect that asked for it, since
    /// nothing it did stopped the helper ([`Failure::Refused`], not [`Failure::Broken`]).
    fn not_started(&self, e: String) -> Failure {
        self.note_error(&e);
        Failure::Refused(e)
    }

    fn note_error(&self, e: &str) {
        log::warn!("helper {}: {e}", self.program.display());
        lock(&self.info).last_error = Some(e.to_string());
    }

    fn spawn(&self) -> Result<Conn, String> {
        {
            let mut r = lock(&self.restarts);
            r.retain(|t| t.elapsed() < Duration::from_secs(60));
            if r.len() >= MAX_RESTARTS_PER_MINUTE {
                return Err("the helper keeps stopping; it stays off until the next scan (Effect ▸ Scan for Effect Helpers)".into());
            }
            r.push(Instant::now());
        }
        let exchange = Exchange::create().map_err(|e| format!("can't create the frame exchange file: {e}"))?;
        let mut cmd = Command::new(&self.program);
        cmd.arg(HELPER_ARG).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
        if let Some(dir) = blocklist::state_dir() {
            cmd.env("EFFECTCRAFT_HELPER_STATE_DIR", dir);
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            // CREATE_NO_WINDOW: no console window per helper.
            cmd.creation_flags(0x0800_0000);
        }
        let mut child = cmd.spawn().map_err(|e| format!("can't start {}: {e}", self.program.display()))?;
        let (Some(stdin), Some(stdout), stderr) = (child.stdin.take(), child.stdout.take(), child.stderr.take()) else {
            let _ = child.kill();
            return Err("the helper's pipes are unavailable".into());
        };
        let (tx, rx) = channel();
        let label = self.program.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let l2 = label.clone();
        let spawned = std::thread::Builder::new().name(format!("helper-out {label}")).spawn(move || {
            let mut r = BufReader::new(stdout);
            loop {
                let mut buf = vec![];
                match (&mut r).take(MAX_LINE).read_until(b'\n', &mut buf) {
                    Ok(0) | Err(_) => {
                        let _ = tx.send(Line::Closed);
                        return;
                    }
                    Ok(_) => {
                        let line = String::from_utf8_lossy(&buf);
                        match parse_reply(&line) {
                            Some(reply) => {
                                if tx.send(Line::Reply(reply)).is_err() {
                                    return;
                                }
                            }
                            None => log::info!("helper {l2}: {}", line.trim_end()),
                        }
                    }
                }
            }
        });
        if let Err(e) = spawned {
            let _ = child.kill();
            return Err(format!("can't start a reader thread: {e}"));
        }
        if let Some(err) = stderr {
            let _ = std::thread::Builder::new().name(format!("helper-err {label}")).spawn(move || {
                for line in BufReader::new(err).lines().map_while(Result::ok) {
                    log::info!("helper {label}: {line}");
                }
            });
        }
        let generation = {
            let mut g = lock(&self.generation);
            *g = g.saturating_add(1);
            *g
        };
        let mut c = Conn { child, stdin, rx, exchange, top: 0, last_used: Instant::now(), generation };
        let frames = c.exchange.path().to_string_lossy().into_owned();
        match c.call(&Request::Hello { version: PROTOCOL_VERSION, frames }, load_timeout()) {
            Ok(Reply::Hello { version, name, helper_version, loads }) if version == PROTOCOL_VERSION => {
                let mut i = lock(&self.info);
                (i.name, i.version, i.loads) = (name, helper_version, loads);
            }
            Ok(Reply::Hello { version, .. }) => {
                c.stop();
                return Err(format!("the helper speaks protocol {version}; this EffectCraft speaks {PROTOCOL_VERSION}"));
            }
            Ok(Reply::Error { message }) => {
                c.stop();
                return Err(message);
            }
            Ok(other) => {
                c.stop();
                return Err(format!("unexpected reply to hello: {other:?}"));
            }
            Err(Failure::Refused(e) | Failure::Broken(e)) => {
                c.stop();
                return Err(e);
            }
        }
        c.exchange.unlink();
        // After a restart the new process knows no effects yet: list them again (the first
        // start lists them in `Helper::start`), then load what the user loaded before.
        if generation > 1 && !matches!(c.call(&Request::Describe, load_timeout()), Ok(Reply::Effects { .. })) {
            c.stop();
            return Err("the restarted helper could not list its effects".into());
        }
        for path in lock(&self.loaded).clone() {
            if !matches!(c.call(&Request::Load { path: path.clone() }, load_timeout()), Ok(Reply::Effects { .. })) {
                log::warn!("helper {}: reloading {path} failed", self.program.display());
            }
        }
        Ok(c)
    }

    fn padding(&self, effect: &str, params: &PluginParams, time: f64, scale: f64, layer_size: [f64; 2], instance: u64) -> Result<u32, String> {
        let _turn = self.take_turn()?;
        let req = Request::Padding { effect: effect.to_string(), instance, time, scale, layer_size, flat: params.flat.to_vec(), texts: params.texts.to_vec() };
        match self.with_conn(|c| c.call(&req, render_timeout())) {
            Ok(Reply::Padding { pixels }) => Ok(pixels),
            Ok(Reply::Error { message }) => Err(message),
            Ok(other) => Err(format!("unexpected reply to padding: {other:?}")),
            Err(Failure::Refused(e) | Failure::Broken(e)) => Err(e),
        }
    }

    /// Render one frame through the helper. Images live on a stack in the exchange file: the
    /// frame first, then whatever the helper asks for, freed again when the render ends (nested
    /// renders stack above).
    fn render(&self, effect: &str, frame: &mut PluginFrame, params: &PluginParams, time: f64, host: &dyn PluginHost) -> Result<(), Failure> {
        let _turn = self.take_turn().map_err(Failure::Refused)?;
        // The generation of the process this render runs on: taken inside `with_conn`, which
        // may start the process (after a crash or `stop`), so it is never that of a dead one.
        let (at, base, started) = self.with_conn(|c| {
            let base = c.top;
            let at = c.alloc(frame.width, frame.height, frame.origin, frame.scale).map_err(Failure::Refused)?;
            c.exchange.write(&at, frame.pixels).map_err(Failure::Broken)?;
            Ok((at, base, c.generation))
        })?;
        let req = Request::Render(RenderRequest {
            effect: effect.to_string(),
            instance: host.instance_key(),
            time,
            frame: at,
            layer_size: frame.layer_size,
            flat: params.flat.to_vec(),
            texts: params.texts.to_vec(),
            frame_rate: host.frame_rate(),
            frame_range: host.frame_range(),
        });
        let result = self.converse(req, frame, &at, host, started);
        // Free this render's images (if the same process is still running).
        if let Some(c) = lock(&self.conn).as_mut()
            && c.generation == started
        {
            c.top = base;
        }
        result
    }

    fn converse(&self, req: Request, frame: &mut PluginFrame, at: &ImageRef, host: &dyn PluginHost, started: u64) -> Result<(), Failure> {
        let gone = || Failure::Broken("the helper restarted during the render".into());
        let mut next = req;
        loop {
            let reply = self.with_conn(|c| if c.generation == started { c.call(&next, render_timeout()) } else { Err(gone()) })?;
            next = match reply {
                Reply::Done => {
                    return self
                        .with_conn(|c| if c.generation == started { c.exchange.read_into(at, frame.pixels).map_err(Failure::Broken) } else { Err(gone()) });
                }
                Reply::Error { message } => return Err(Failure::Refused(message)),
                // Host callbacks run without holding the process, so they may render through
                // this helper again (on this thread: the turn is re-entered).
                Reply::InputAt { time } => self.send_image(host.input_at(time), started)?,
                Reply::LayerInput { param, time } => self.send_image(host.layer_input(&param, time), started)?,
                Reply::ParamsAt { time } => Request::Params { values: host.params_at(time) },
                other => return Err(Failure::Broken(format!("unexpected reply during a render: {other:?}"))),
            };
        }
    }

    /// Put a callback's image in the exchange file and make the answer.
    fn send_image(&self, img: Option<PluginImage>, started: u64) -> Result<Request, Failure> {
        let Some(img) = img.filter(|i| u64::from(i.width).saturating_mul(u64::from(i.height)) == i.pixels.len() as u64) else {
            return Ok(Request::Image { image: None });
        };
        self.with_conn(|c| {
            if c.generation != started {
                return Err(Failure::Broken("the helper restarted during the render".into()));
            }
            let at = c.alloc(img.width, img.height, img.origin, img.scale).map_err(Failure::Refused)?;
            c.exchange.write(&at, &img.pixels).map_err(Failure::Broken)?;
            Ok(Request::Image { image: Some(at) })
        })
    }
}

impl Drop for Helper {
    fn drop(&mut self) {
        self.stop();
    }
}

/// A helper's effect, registered like any plug-in effect.
pub struct HelperEffect {
    helper: Arc<Helper>,
    manifest: PluginManifest,
    padding: bool,
}

impl HelperEffect {
    pub fn new(helper: Arc<Helper>, remote: RemoteEffect) -> HelperEffect {
        HelperEffect { helper, manifest: remote.manifest, padding: remote.padding }
    }
}

/// A host for [`EffectPlugin::render`] without services.
struct NoHost;

impl PluginHost for NoHost {
    fn input_at(&self, _: f64) -> Option<PluginImage> {
        None
    }
    fn layer_input(&self, _: &str, _: f64) -> Option<PluginImage> {
        None
    }
    fn params_at(&self, _: f64) -> Option<(Vec<f64>, Vec<String>)> {
        None
    }
    fn frame_rate(&self) -> f64 {
        30.0
    }
    fn time(&self) -> f64 {
        0.0
    }
}

impl EffectPlugin for HelperEffect {
    fn manifest(&self) -> &PluginManifest {
        &self.manifest
    }

    fn render(&self, frame: &mut PluginFrame, params: &PluginParams, time: f64) -> Result<(), String> {
        self.render_v2(frame, params, time, &NoHost)
    }

    fn source(&self) -> String {
        format!("helper: {}", self.helper.program.display())
    }

    fn padding(&self, params: &PluginParams, time: f64, scale: f64, layer_size: [f64; 2], instance_key: u64) -> u32 {
        if !self.padding || blocklist::is_blocked(&self.manifest.id) {
            return 0;
        }
        match self.helper.padding(&self.manifest.id, params, time, scale, layer_size, instance_key) {
            Ok(p) => p,
            Err(e) => {
                log::warn!("helper effect `{}`: padding: {e}", self.manifest.id);
                0
            }
        }
    }

    fn render_v2(&self, frame: &mut PluginFrame, params: &PluginParams, time: f64, host: &dyn PluginHost) -> Result<(), String> {
        let id = &self.manifest.id;
        if blocklist::is_blocked(id) {
            return Err(format!(
                "{} is turned off: it stopped its helper {} times (Effect ▸ Scan for Effect Helpers with retry turns it back on)",
                self.manifest.name,
                blocklist::CRASH_LIMIT
            ));
        }
        match self.helper.render(id, frame, params, time, host) {
            Ok(()) => Ok(()),
            Err(Failure::Refused(e)) => Err(e),
            Err(Failure::Broken(e)) => {
                blocklist::note_crash(id);
                Err(format!("{}: the effect helper failed: {e}", self.manifest.name))
            }
        }
    }
}
