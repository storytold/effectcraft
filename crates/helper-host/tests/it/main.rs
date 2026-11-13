//! The helper protocol end to end against a fake helper, written in safe Rust with
//! [`effectcraft_helper_host::serve`]. This binary is both sides: started with
//! `--effectcraft-helper` it is the helper, otherwise it runs the checks, starting itself as
//! the helper. (`harness = false`: libtest would reject the helper argument.)

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use effectcraft_effects::plugin::{
    EffectPlugin, ParamChange, PluginFrame, PluginHost, PluginImage, PluginManifest, PluginParam, PluginParamKind, PluginParams, plugin,
};
use effectcraft_helper_host::protocol::{HELPER_ARG, Reply, reply_line};
use effectcraft_helper_host::serve::{Provider, serve};

fn manifest(id: &str, params: Vec<PluginParam>) -> PluginManifest {
    PluginManifest {
        api: 2,
        id: id.into(),
        name: id.rsplit('.').next().unwrap_or(id).into(),
        category: "Helper Test".into(),
        version: "1".into(),
        author: String::new(),
        description: String::new(),
        params,
    }
}

fn amount() -> PluginParam {
    PluginParam {
        id: "amount".into(),
        name: "Amount".into(),
        group: String::new(),
        kind: PluginParamKind::Slider { default: 1.0, min: 0.0, max: 1.0, slider_min: None, slider_max: None, decimals: 2, integer: false },
    }
}

/// What each fake effect does.
#[derive(Clone, Copy, PartialEq)]
enum Kind {
    /// RGB → amount·(1 − RGB).
    Invert,
    /// Asks for 3 pixels of padding; fills the frame with its padding as red.
    Pad,
    /// Replaces the frame with the input one frame earlier, and puts `amount` from
    /// `params_at(0)` in alpha of pixel 0.
    Echo,
    /// Exits the process mid-render.
    Crash,
    /// Asks for the parameters before 0 (an answer far larger than a pipe holds), then never
    /// reads it or answers.
    Hang,
    /// Writes a long line of bytes that aren't UTF-8 to stderr, then more, then inverts.
    Noisy,
    /// Pool checks: marks itself active, waits until `amount` renders (this one included, in any
    /// process of this helper) are active at once, holds the frame for `hold_ms`, then finishes,
    /// or, with `exit`, exits the process.
    Meet { hold_ms: u64, exit: bool },
    /// Pool checks: asks for the input at another time (a callback) and finishes.
    Ask,
    /// Parameter-change checks: answers `param_changed` for `preset` (see [`preset_answer`]).
    Preset,
}

struct Fake {
    m: PluginManifest,
    kind: Kind,
    /// The parameters whose changes it wants to hear about.
    notify: Vec<String>,
}

impl Fake {
    fn new(m: PluginManifest, kind: Kind) -> Fake {
        Fake { m, kind, notify: vec![] }
    }
}

/// What the fake answers when `preset` changes to: 1, values made from what it was told
/// (amount halved; tint = time, layer width / 100, whether the instance is 7, 1); 2, values that
/// don't fit (only `link`, as a number, survives and amount 5 is clamped); 3, nothing for 10 s
/// (past the edit timeout); 4, it exits; 5, a reply that isn't `set_params`; else nothing.
fn preset_answer(change: &ParamChange) -> Result<Vec<(String, serde_json::Value)>, String> {
    use serde_json::json;
    if let Ok(dir) = pool_dir() {
        let _ = append(&dir, "changed.log", &format!("{} {}", std::process::id(), change.param));
    }
    let amount = change.values.get("amount").and_then(serde_json::Value::as_f64).unwrap_or(-1.0);
    match change.values.get("preset").and_then(serde_json::Value::as_u64) {
        Some(1) => Ok(vec![
            ("amount".into(), json!(amount * 0.5)),
            ("tint".into(), json!([change.time, change.layer_size[0] / 100.0, if change.instance_key == 7 { 1.0 } else { 0.0 }, 1.0])),
        ]),
        Some(2) => Ok(vec![
            ("nope".into(), json!(1)),
            ("tint".into(), json!([1.0])),
            ("preset".into(), json!(9)),
            ("link".into(), json!(1)),
            ("amount".into(), json!(5.0)),
            ("tint".into(), json!("red")),
        ]),
        Some(3) => {
            std::thread::sleep(Duration::from_secs(10));
            Ok(vec![])
        }
        Some(4) => std::process::exit(3),
        Some(5) => {
            let mut out = std::io::stdout();
            out.write_all(reply_line(&Reply::Pong)?.as_bytes()).and_then(|_| out.flush()).map_err(|e| e.to_string())?;
            std::thread::sleep(Duration::from_secs(10));
            Ok(vec![])
        }
        _ => Ok(vec![]),
    }
}

impl EffectPlugin for Fake {
    fn manifest(&self) -> &PluginManifest {
        &self.m
    }
    fn render(&self, _: &mut PluginFrame, _: &PluginParams, _: f64) -> Result<(), String> {
        Err("needs render_v2".into())
    }
    fn padding(&self, _: &PluginParams, _: f64, _: f64, _: [f64; 2], _: u64) -> u32 {
        if self.kind == Kind::Pad { 3 } else { 0 }
    }
    fn render_v2(&self, frame: &mut PluginFrame, params: &PluginParams, _time: f64, host: &dyn PluginHost) -> Result<(), String> {
        match self.kind {
            Kind::Invert => {
                let a = params.f("amount") as f32;
                for p in frame.pixels.iter_mut() {
                    for c in &mut p[..3] {
                        *c = a * (1.0 - *c);
                    }
                }
            }
            Kind::Pad => frame.pixels.iter_mut().for_each(|p| *p = [3.0, 0.0, 0.0, 1.0]),
            Kind::Echo => {
                let prev = host.input_at(host.time() - 1.0 / host.frame_rate()).ok_or("no input at the previous frame")?;
                if prev.pixels.len() != frame.pixels.len() {
                    return Err("previous frame has another size".into());
                }
                frame.pixels.copy_from_slice(&prev.pixels);
                let (flat, _) = host.params_at(0.0).ok_or("no params at 0")?;
                if let (Some(p), Some(a)) = (frame.pixels.first_mut(), flat.first()) {
                    p[3] = *a as f32;
                }
            }
            Kind::Meet { hold_ms, exit } => meet(params.f("amount") as usize, hold_ms, exit)?,
            Kind::Ask => {
                IN_ASK.store(true, Ordering::SeqCst);
                let _ = host.input_at(0.0);
                IN_ASK.store(false, Ordering::SeqCst);
            }
            Kind::Preset => {}
            Kind::Crash => std::process::exit(3),
            Kind::Hang => {
                let mut out = std::io::stdout();
                out.write_all(reply_line(&Reply::ParamsAt { time: -1.0 })?.as_bytes()).and_then(|_| out.flush()).map_err(|e| e.to_string())?;
                std::thread::sleep(Duration::from_secs(3600));
            }
            Kind::Noisy => {
                let mut err = std::io::stderr();
                let mut junk = vec![0xffu8; 1 << 20];
                junk.push(b'\n');
                err.write_all(&junk).map_err(|e| format!("stderr: {e}"))?;
                // EffectCraft must still be reading.
                std::thread::sleep(Duration::from_millis(300));
                err.write_all(b"still talking\n").map_err(|e| format!("stderr: {e}"))?;
                frame.pixels.iter_mut().for_each(|p| *p = [1.0, 0.0, 0.0, 1.0]);
            }
        }
        Ok(())
    }
    fn notify_params(&self) -> &[String] {
        &self.notify
    }
    fn param_changed(&self, change: &ParamChange) -> Result<Vec<(String, serde_json::Value)>, String> {
        if self.kind == Kind::Preset { preset_answer(change) } else { Ok(vec![]) }
    }
}

/// Set while a fake helper process waits for the answer to a callback.
static IN_ASK: AtomicBool = AtomicBool::new(false);

/// Where the pool checks' fake helpers keep their marks (`EC_FAKE_DIR`, set by the checks).
fn pool_dir() -> Result<PathBuf, String> {
    std::env::var_os("EC_FAKE_DIR").map(PathBuf::from).ok_or_else(|| "EC_FAKE_DIR is not set".to_string())
}

/// Create an empty mark file `name` in `dir`.
fn mark(dir: &Path, name: &str) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let p = dir.join(name);
    std::fs::write(&p, b"").map_err(|e| e.to_string())?;
    Ok(p)
}

fn count_files(dir: &Path, prefix: &str) -> usize {
    std::fs::read_dir(dir).map_or(0, |rd| rd.flatten().filter(|e| e.file_name().to_string_lossy().starts_with(prefix)).count())
}

/// Append one line to the log `name` in `dir` (one write: processes append at the same time).
fn append(dir: &Path, name: &str, line: &str) -> Result<(), String> {
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(dir.join(name)).map_err(|e| e.to_string())?;
    f.write_all(format!("{line}\n").as_bytes()).map_err(|e| e.to_string())
}

/// The lines of the log `name` in `dir`, split at the first space (process id, rest).
fn log_lines(dir: &Path, name: &str) -> Vec<(String, String)> {
    std::fs::read_to_string(dir.join(name))
        .unwrap_or_default()
        .lines()
        .map(|l| l.split_once(' ').map_or((l.to_string(), String::new()), |(a, b)| (a.to_string(), b.to_string())))
        .collect()
}

/// The processes that loaded `path` (`loads.log`), each with how often.
fn loads_of(dir: &Path, path: &str) -> std::collections::BTreeMap<String, usize> {
    let mut m = std::collections::BTreeMap::new();
    for (pid, p) in log_lines(dir, "loads.log") {
        if p == path {
            *m.entry(pid).or_insert(0) += 1;
        }
    }
    m
}

/// The paths a fake helper process has loaded, in order.
static LOADED: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// A destroyed instance that makes the pool's fake helper stop reading its input for a while.
const SLEEPY_INSTANCE: u64 = 666;

/// One render of [`Kind::Meet`]: `need` renders (this one included) are active at the same time,
/// or this fails after 15 s. The most renders seen at once is appended to `seen.log`.
fn meet(need: usize, hold_ms: u64, exit: bool) -> Result<(), String> {
    let dir = pool_dir()?;
    let mine = mark(&dir.join("marks"), &std::process::id().to_string())?;
    let (deadline, mut seen) = (Instant::now() + Duration::from_secs(15), 0);
    loop {
        seen = seen.max(count_files(&dir.join("marks"), ""));
        if seen >= need || Instant::now() > deadline {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    append(&dir, "seen.log", &seen.to_string())?;
    // What this process had loaded when it rendered.
    let loaded = LOADED.lock().map(|l| l.join(",")).unwrap_or_default();
    append(&dir, "renders.log", &format!("{} {loaded}", std::process::id()))?;
    std::thread::sleep(Duration::from_millis(hold_ms));
    let _ = std::fs::remove_file(&mine);
    if exit {
        append(&dir, "crashes.log", &std::process::id().to_string())?;
        std::process::exit(3);
    }
    if seen < need { Err(format!("only {seen} renders were active at once, wanted {need}")) } else { Ok(()) }
}

/// The fake helper of the pool checks: `max` processes, effects `org.test.<tag>.meet` (waits for
/// the other renders, holds 100 ms), `.slow` (holds 400 ms) and `.crash` (exits after 100 ms).
struct PoolProvider {
    tag: &'static str,
    max: u32,
}

impl PoolProvider {
    fn new(tag: &'static str, max: u32) -> PoolProvider {
        if let Ok(dir) = pool_dir() {
            // A process that dies while it starts up (before answering `hello`).
            if dir.join("die-on-start").exists() {
                std::process::exit(3);
            }
            let _ = mark(&dir.join("procs"), &std::process::id().to_string());
        }
        PoolProvider { tag, max }
    }
}

impl Provider for PoolProvider {
    fn name(&self) -> String {
        "Pool".into()
    }
    fn loads(&self) -> Vec<String> {
        vec!["pool".into()]
    }
    fn max_processes(&self) -> u32 {
        self.max
    }
    fn effects(&mut self) -> (Vec<Arc<dyn EffectPlugin>>, Vec<String>) {
        let kinds = [
            ("meet", Kind::Meet { hold_ms: 100, exit: false }),
            ("slow", Kind::Meet { hold_ms: 400, exit: false }),
            ("crash", Kind::Meet { hold_ms: 100, exit: true }),
            ("ask", Kind::Ask),
        ];
        let v: Vec<Arc<dyn EffectPlugin>> = kinds
            .into_iter()
            .map(|(n, kind)| Arc::new(Fake::new(manifest(&format!("org.test.{}.{n}", self.tag), vec![amount()]), kind)) as Arc<dyn EffectPlugin>)
            .collect();
        (v, vec![])
    }
    fn load(&mut self, path: &str) -> Result<(Vec<Arc<dyn EffectPlugin>>, Vec<String>), String> {
        let pid = std::process::id();
        let dir = pool_dir()?;
        // A load in the middle of a render's callback is a protocol error of EffectCraft's side.
        if IN_ASK.load(Ordering::SeqCst) {
            mark(&dir, &format!("load-during-render.{pid}"))?;
        }
        // While `refuse-z` exists, only the first process to load `z.pool` can.
        if path == "z.pool" && dir.join("refuse-z").exists() && std::fs::OpenOptions::new().write(true).create_new(true).open(dir.join("z-first")).is_err() {
            return Err("z.pool can't be loaded now".into());
        }
        append(&dir, "loads.log", &format!("{pid} {path}"))?;
        if let Ok(mut l) = LOADED.lock() {
            l.push(path.to_string());
        }
        Ok((vec![], vec![]))
    }
    fn destroy(&mut self, instance: u64) {
        if instance == SLEEPY_INSTANCE {
            std::thread::sleep(Duration::from_secs(6));
        }
        if let Ok(dir) = pool_dir()
            && instance < 1000
        {
            let _ = mark(&dir, &format!("destroyed.{}.{instance}", std::process::id()));
        }
    }
}

/// The fake helper of the parameter-change checks (`EC_FAKE_MODE` `notify`, or `old`: a helper
/// from before the extension, which announces no features): one process, effects
/// `org.test.<tag>.preset` (notifies on `preset`) and `.ask` (holds its process in a callback).
struct NotifyProvider {
    tag: &'static str,
}

impl Provider for NotifyProvider {
    fn name(&self) -> String {
        "Notify".into()
    }
    fn features(&self) -> Vec<String> {
        if self.tag == "old" { vec![] } else { vec![effectcraft_helper_host::protocol::FEATURE_PARAM_CHANGED.into()] }
    }
    fn effects(&mut self) -> (Vec<Arc<dyn EffectPlugin>>, Vec<String>) {
        let preset = PluginParam {
            id: "preset".into(),
            name: "Preset".into(),
            group: String::new(),
            kind: PluginParamKind::Popup { options: vec!["Custom".into(); 6], default: 0 },
        };
        let tint = PluginParam { id: "tint".into(), name: "Tint".into(), group: String::new(), kind: PluginParamKind::Color { default: [1.0; 4] } };
        let link = PluginParam { id: "link".into(), name: "Link".into(), group: String::new(), kind: PluginParamKind::Checkbox { default: false } };
        let mut fx = Fake::new(manifest(&format!("org.test.{}.preset", self.tag), vec![amount(), preset, tint, link]), Kind::Preset);
        // Unknown ids are dropped by EffectCraft.
        fx.notify = vec!["preset".into(), "no_such_param".into()];
        let ask = Fake::new(manifest(&format!("org.test.{}.ask", self.tag), vec![amount()]), Kind::Ask);
        (vec![Arc::new(fx), Arc::new(ask)], vec![])
    }
}

struct FakeProvider;

impl Provider for FakeProvider {
    fn name(&self) -> String {
        "Fake".into()
    }
    fn effects(&mut self) -> (Vec<Arc<dyn EffectPlugin>>, Vec<String>) {
        let v: Vec<Arc<dyn EffectPlugin>> =
            [("invert", Kind::Invert), ("pad", Kind::Pad), ("echo", Kind::Echo), ("crash", Kind::Crash), ("hang", Kind::Hang), ("noisy", Kind::Noisy)]
                .into_iter()
                .map(|(n, kind)| Arc::new(Fake::new(manifest(&format!("org.test.helper.{n}"), vec![amount()]), kind)) as Arc<dyn EffectPlugin>)
                .collect();
        (v, vec!["fake helper: a warning".into()])
    }
}

/// The host side of a render in these checks: the input at any time is a flat grey of that
/// time; with `nest`, asking for the input renders `invert` through the same helper first.
struct TestHost {
    w: u32,
    h: u32,
    nest: bool,
}

impl PluginHost for TestHost {
    fn input_at(&self, t: f64) -> Option<PluginImage> {
        let mut pixels = vec![[t as f32, t as f32, t as f32, 1.0]; (self.w * self.h) as usize];
        if self.nest {
            let inv = plugin("org.test.helper.invert")?;
            let m = inv.manifest().clone();
            let mut f =
                PluginFrame { width: self.w, height: self.h, pixels: &mut pixels, scale: 1.0, origin: [0.0; 2], layer_size: [self.w as f64, self.h as f64] };
            inv.render_v2(&mut f, &PluginParams { manifest: &m, flat: &[1.0], texts: &[] }, t, &TestHost { nest: false, ..*self }).ok()?;
        }
        Some(PluginImage { width: self.w, height: self.h, pixels, origin: [0.0; 2], scale: 1.0 })
    }
    fn layer_input(&self, _: &str, _: f64) -> Option<PluginImage> {
        None
    }
    fn params_at(&self, t: f64) -> Option<(Vec<f64>, Vec<String>)> {
        // Before 0: megabytes, far more than a pipe holds (see `Kind::Hang`).
        if t < 0.0 {
            return Some((vec![0.5; 1 << 20], vec![]));
        }
        Some((vec![0.25], vec![]))
    }
    fn frame_rate(&self) -> f64 {
        10.0
    }
    fn time(&self) -> f64 {
        1.0
    }
    fn instance_key(&self) -> u64 {
        7
    }
}

fn render(id: &str, pixels: &mut Vec<[f32; 4]>, w: u32, h: u32, amount: f64, nest: bool) -> Result<(), String> {
    let p = plugin(id).ok_or_else(|| format!("{id} is not registered"))?;
    let m = p.manifest().clone();
    let params = PluginParams { manifest: &m, flat: &[amount], texts: &[] };
    let host = TestHost { w, h, nest };
    let pad = p.padding(&params, 1.0, 1.0, [w as f64, h as f64], 7);
    let (w, h) = (w + 2 * pad, h + 2 * pad);
    pixels.resize((w * h) as usize, [0.0; 4]);
    let mut f = PluginFrame { width: w, height: h, pixels, scale: 1.0, origin: [pad as f64; 2], layer_size: [w as f64, h as f64] };
    p.render_v2(&mut f, &params, 1.0, &host)
}

fn check(name: &str, ok: bool, detail: impl std::fmt::Debug) -> bool {
    println!("{} {name}{}", if ok { "ok  " } else { "FAIL" }, if ok { String::new() } else { format!(": {detail:?}") });
    ok
}

fn run_checks() -> bool {
    let exe = match std::env::current_exe() {
        Ok(e) => e,
        Err(e) => return check("current exe", false, e),
    };
    let state = std::env::temp_dir().join(format!("ec-fake-helper-state-{}", std::process::id()));
    effectcraft_helper_host::blocklist::set_state_dir(state.clone());
    let mut all = true;

    // Discovery + describe.
    let r = effectcraft_helper_host::scan_paths(std::slice::from_ref(&exe), false);
    let loaded = r["loaded"].as_array().map_or(0, Vec::len);
    all &= check("scan registers the helper's effects", loaded == 6, &r);
    all &= check("helper warnings are reported", r["errors"].to_string().contains("a warning"), &r);
    all &= check(
        "a second scan doesn't start it twice",
        effectcraft_helper_host::scan_paths(std::slice::from_ref(&exe), false)["helpers"].as_array().is_some_and(Vec::is_empty),
        "",
    );
    let list = effectcraft_helper_host::list();
    all &= check("list names the helper", list["helpers"][0]["name"] == "Fake" && list["helpers"][0]["running"] == true, &list);

    // Plain render through the exchange file.
    let mut px = vec![[0.25, 0.5, 1.0, 1.0]; 6];
    let r = render("org.test.helper.invert", &mut px, 3, 2, 1.0, false);
    all &= check("invert renders", r.is_ok() && px.iter().all(|p| (p[0] - 0.75).abs() < 1e-6 && p[2].abs() < 1e-6 && p[3] == 1.0), (&r, &px));

    // Output on stderr that isn't text, in long lines, is drained, so the helper never blocks on it
    // or loses its stderr.
    let mut px = vec![[0.0; 4]; 4];
    let r = render("org.test.helper.noisy", &mut px, 2, 2, 1.0, false);
    all &= check("a helper's stderr is drained whatever it holds", r.is_ok() && px[0] == [1.0, 0.0, 0.0, 1.0], &r);

    // Padding.
    let mut px = vec![[0.0; 4]; 4];
    let r = render("org.test.helper.pad", &mut px, 2, 2, 1.0, false);
    all &= check("padding grows the frame", r.is_ok() && px.len() == 64 && px[0][0] == 3.0, (&r, px.len()));

    // Callbacks: the input at another time and parameters at another time.
    let mut px = vec![[0.0; 4]; 4];
    let r = render("org.test.helper.echo", &mut px, 2, 2, 1.0, false);
    all &= check("input_at / params_at round-trip", r.is_ok() && (px[1][0] - 0.9).abs() < 1e-6 && px[0][3] == 0.25, (&r, &px));

    // Nested: answering input_at renders another effect of the same helper.
    let mut px = vec![[0.0; 4]; 4];
    let r = render("org.test.helper.echo", &mut px, 2, 2, 1.0, true);
    all &= check("nested renders through one helper", r.is_ok() && (px[1][0] - 0.1).abs() < 1e-5, (&r, &px));

    // Several threads at once share the helper.
    let threads: Vec<_> = (0..4)
        .map(|i| {
            std::thread::spawn(move || {
                let mut px = vec![[0.0, 0.0, 0.0, 1.0]; 16];
                render("org.test.helper.invert", &mut px, 4, 4, 0.5, false).map(|_| (i, px[0][0]))
            })
        })
        .collect();
    let results: Vec<_> = threads.into_iter().map(|t| t.join().map_err(|_| "thread panicked".to_string()).and_then(|r| r)).collect();
    all &= check("concurrent renders", results.iter().all(|r| matches!(r, Ok((_, v)) if (*v - 0.5).abs() < 1e-6)), &results);

    // A crash mid-render is an error, the frame is untouched, and the helper comes back.
    let mut px = vec![[0.5; 4]; 4];
    let r = render("org.test.helper.crash", &mut px, 2, 2, 1.0, false);
    all &= check("a crashing helper is an error", r.as_ref().is_err_and(|e| e.contains("helper")) && px.iter().all(|p| *p == [0.5; 4]), &r);
    let mut px = vec![[0.0, 0.0, 0.0, 1.0]; 4];
    let r = render("org.test.helper.invert", &mut px, 2, 2, 1.0, false);
    all &= check("the helper restarts for the next render", r.is_ok() && px[0][0] == 1.0, &r);
    // A render that starts the stopped helper itself (no padding request first) succeeds.
    effectcraft_helper_host::stop_all();
    let r = plugin("org.test.helper.invert").ok_or("invert is not registered").and_then(|p| {
        let m = p.manifest().clone();
        let mut px = vec![[0.0, 0.0, 0.0, 1.0]; 4];
        let mut f = PluginFrame { width: 2, height: 2, pixels: &mut px, scale: 1.0, origin: [0.0; 2], layer_size: [2.0; 2] };
        let host = TestHost { w: 2, h: 2, nest: false };
        p.render_v2(&mut f, &PluginParams { manifest: &m, flat: &[1.0], texts: &[] }, 1.0, &host).map_err(|_| "render failed")?;
        Ok(px[0][0])
    });
    all &= check("a stopped helper restarts for a render without padding", r == Ok(1.0), r);

    // Effects that keep crashing it are turned off (and the list survives in the state dir).
    for _ in 0..3 {
        let _ = render("org.test.helper.crash", &mut vec![[0.0; 4]; 4], 2, 2, 1.0, false);
    }
    let r = render("org.test.helper.crash", &mut vec![[0.0; 4]; 4], 2, 2, 1.0, false);
    all &= check("repeat offenders are blocked", r.as_ref().is_err_and(|e| e.contains("turned off")), &r);
    all &= check("the block list is saved", std::fs::read_to_string(state.join("helper_blocklist.txt")).is_ok_and(|t| t.contains("org.test.helper.crash")), "");
    let r = effectcraft_helper_host::scan_paths(&[], true);
    all &= check("retry clears the block list", !effectcraft_helper_host::blocklist::is_blocked("org.test.helper.crash"), &r);

    // A hang is cut off by the timeout, also while EffectCraft is writing to a helper that
    // stopped reading.
    let t0 = std::time::Instant::now();
    let r = render("org.test.helper.hang", &mut vec![[0.0; 4]; 4], 2, 2, 1.0, false);
    all &= check(
        "a hanging helper times out",
        r.as_ref().is_err_and(|e| e.contains("did not answer")) && t0.elapsed() < Duration::from_secs(30),
        (&r, t0.elapsed()),
    );

    // The restarts above use up the helper's restarts for this minute, so it can't start again.
    // That is not the fault of the effect being rendered: it fails, but is not charged and never
    // turned off (it would be after CRASH_LIMIT charged failures).
    let starts: Vec<_> = (0..4).map(|_| render("org.test.helper.invert", &mut vec![[0.0; 4]; 4], 2, 2, 1.0, false)).collect();
    all &= check(
        "a helper that can't start doesn't block the effect",
        starts.iter().all(|r| r.as_ref().is_err_and(|e| e.contains("keeps stopping")))
            && !effectcraft_helper_host::blocklist::is_blocked("org.test.helper.invert"),
        &starts,
    );

    effectcraft_helper_host::stop_all();
    all &= pool_group("serial");
    all &= pool_group("pool");
    all &= pool_group("startup");
    all &= pool_group("notify");
    all &= pool_group("old");
    let _ = std::fs::remove_dir_all(&state);
    all
}

/// Render `id` (a pool check effect) on a 2x2 frame; `need` renders must be active at once.
fn render_pool_with(id: &str, need: f64, host: &dyn PluginHost) -> Result<(), String> {
    let p = plugin(id).ok_or_else(|| format!("{id} is not registered"))?;
    let m = p.manifest().clone();
    let mut px = vec![[0.5; 4]; 4];
    let mut f = PluginFrame { width: 2, height: 2, pixels: &mut px, scale: 1.0, origin: [0.0; 2], layer_size: [2.0; 2] };
    p.render_v2(&mut f, &PluginParams { manifest: &m, flat: &[need], texts: &[] }, 1.0, host)
}

fn render_pool(id: &str, need: f64) -> Result<(), String> {
    render_pool_with(id, need, &TestHost { w: 2, h: 2, nest: false })
}

/// Render each job (effect id, renders needed at once) on its own thread, all at once.
fn render_together(jobs: &[(&'static str, f64)]) -> Vec<Result<(), String>> {
    let threads: Vec<_> = jobs.iter().map(|&(id, need)| std::thread::spawn(move || render_pool(id, need))).collect();
    threads.into_iter().map(|t| t.join().unwrap_or_else(|_| Err("thread panicked".into()))).collect()
}

/// The host of a render that, when the helper asks for its input, tells the test, waits for it to
/// go on, and renders another effect of the same helper (a nested render) before answering.
struct GateHost {
    ready: Mutex<Sender<()>>,
    go: Mutex<Receiver<()>>,
    /// The effect of the nested render.
    nested: &'static str,
}

impl PluginHost for GateHost {
    fn input_at(&self, _: f64) -> Option<PluginImage> {
        let _ = self.ready.lock().ok()?.send(());
        self.go.lock().ok()?.recv_timeout(Duration::from_secs(30)).ok()?;
        render_pool(self.nested, 1.0).ok()?;
        Some(PluginImage { width: 2, height: 2, pixels: vec![[0.0; 4]; 4], origin: [0.0; 2], scale: 1.0 })
    }
    fn layer_input(&self, _: &str, _: f64) -> Option<PluginImage> {
        None
    }
    fn params_at(&self, _: f64) -> Option<(Vec<f64>, Vec<String>)> {
        None
    }
    fn frame_rate(&self) -> f64 {
        10.0
    }
    fn time(&self) -> f64 {
        1.0
    }
    fn instance_key(&self) -> u64 {
        9
    }
}

/// The most renders any process saw at once since `seen.log` was last removed.
fn seen_max(dir: &Path) -> usize {
    std::fs::read_to_string(dir.join("seen.log")).unwrap_or_default().lines().filter_map(|l| l.trim().parse().ok()).max().unwrap_or(0)
}

/// Forget what earlier checks saw, so a check reads only its own renders.
fn reset_seen(dir: &Path) {
    let _ = std::fs::remove_file(dir.join("seen.log"));
}

fn helper_entry() -> serde_json::Value {
    effectcraft_helper_host::list()["helpers"][0].clone()
}

/// Process pools: a helper that says `max_processes` gets that many processes, one render each.
/// Run in a child of the test (`--pool-checks`) with `EC_FAKE_MODE` (`pool`: max 4, `serial`: no
/// `max_processes`, `startup`: max 4, see [`startup_checks`]), `EC_FAKE_DIR` for the helpers' marks, and the helper program being this
/// very binary, which reads the same variables when started as the helper.
fn run_pool_checks(exe: &Path, mode: &str) -> bool {
    let mut all = true;
    // The pool checks wait for each other: more than the short timeout of the hang check.
    effectcraft_helper_host::client::set_render_timeout(Duration::from_secs(30));
    let Ok(dir) = pool_dir() else { return check("EC_FAKE_DIR", false, "not set") };
    let state = dir.join("state");
    effectcraft_helper_host::blocklist::set_state_dir(state);
    let r = effectcraft_helper_host::scan_paths(std::slice::from_ref(&exe.to_path_buf()), false);
    all &= check(&format!("{mode}: scan registers the helper's effects"), r["loaded"].as_array().map_or(0, Vec::len) == 4, &r);

    if mode == "serial" {
        // Without max_processes there is one process and the renders take turns.
        let rs = render_together(&[("org.test.serial.meet", 1.0); 3]);
        let procs = count_files(&dir.join("procs"), "");
        all &=
            check("without max_processes renders stay serial", rs.iter().all(Result::is_ok) && seen_max(&dir) == 1 && procs == 1, (&rs, seen_max(&dir), procs));
        let e = helper_entry();
        all &= check("list: one process, at most one", e["processes"] == 1 && e["max_processes"] == 1, &e);
        effectcraft_helper_host::stop_all();
        return all;
    }
    if mode == "startup" {
        return all & startup_checks(&dir);
    }

    // max_processes 4: four renders that only finish when all four are active at once. A serial
    // helper can't, so this proves they ran in parallel, in four processes.
    let rs = render_together(&[("org.test.pool.meet", 4.0); 4]);
    let procs = count_files(&dir.join("procs"), "");
    all &= check("four renders run at once in four processes", rs.iter().all(Result::is_ok) && seen_max(&dir) >= 4 && procs == 4, (&rs, seen_max(&dir), procs));
    let e = helper_entry();
    all &= check("list reports the pool", e["processes"] == 4 && e["max_processes"] == 4 && e["restarts"].as_u64().is_some_and(|r| r <= 1), &e);

    // More renders than processes wait for a free one; never more than four processes or renders.
    reset_seen(&dir);
    let rs = render_together(&[("org.test.pool.meet", 1.0); 12]);
    let procs = count_files(&dir.join("procs"), "");
    all &= check("never more processes than max_processes", rs.iter().all(Result::is_ok) && procs == 4, (&rs, procs));
    all &= check("never more renders at once than processes", (1..=4).contains(&seen_max(&dir)), seen_max(&dir));

    // load reaches every running process; destroy too.
    let r = effectcraft_helper_host::load_path("x.pool");
    all &= check("load is accepted", r.is_ok(), &r);
    let x = loads_of(&dir, "x.pool");
    all &= check("load reached all four processes", x.len() == 4 && x.values().all(|n| *n == 1), &x);
    // Loading the same path again (a folder with new plug-ins) reaches every process again.
    let r = effectcraft_helper_host::load_path("x.pool");
    let x = loads_of(&dir, "x.pool");
    all &= check("a repeated load reaches all four processes", r.is_ok() && x.len() == 4 && x.values().all(|n| *n == 2), (&r, &x));

    // A load a process refuses stays pending, and that process loads it before its next render.
    let _ = mark(&dir, "refuse-z");
    let r = effectcraft_helper_host::load_path("z.pool");
    let z = loads_of(&dir, "z.pool");
    all &= check("a load refused by the other processes", r.is_ok() && z.len() == 1, (&r, &z));
    let _ = std::fs::remove_file(dir.join("refuse-z"));
    let rs = render_together(&[("org.test.pool.meet", 4.0); 4]);
    let z = loads_of(&dir, "z.pool");
    all &= check("a refused load is tried again before the next render", rs.iter().all(Result::is_ok) && z.len() == 4, (&rs, &z));

    let told = effectcraft_helper_host::destroy_instance(42);
    let t0 = Instant::now();
    while count_files(&dir, "destroyed.") < 4 && t0.elapsed() < Duration::from_secs(10) {
        std::thread::sleep(Duration::from_millis(20));
    }
    all &= check("destroy reached all four processes", told == 4 && count_files(&dir, "destroyed.") == 4, (told, count_files(&dir, "destroyed.")));

    // destroy never waits for a process to read its input: these processes stop reading for a
    // while, and many more destroys than their pipes hold are still handed over at once. Renders
    // afterwards are in step (the writer's results for the destroys come first).
    let told = effectcraft_helper_host::destroy_instance(SLEEPY_INSTANCE);
    let t0 = Instant::now();
    let flood: usize = (0..20_000u64).map(|i| effectcraft_helper_host::destroy_instance(1000 + i)).sum();
    let took = t0.elapsed();
    all &= check("destroy doesn't wait for a helper that stopped reading", told == 4 && flood == 80_000 && took < Duration::from_secs(3), (told, flood, took));
    let rs = render_together(&[("org.test.pool.meet", 4.0); 4]);
    all &= check("renders after many destroys", rs.iter().all(Result::is_ok), &rs);

    // A crash in one process leaves the renders in the other three alone.
    let rs = render_together(&[("org.test.pool.slow", 4.0), ("org.test.pool.slow", 4.0), ("org.test.pool.slow", 4.0), ("org.test.pool.crash", 4.0)]);
    all &= check(
        "a crash in one process leaves the other renders intact",
        rs[..3].iter().all(Result::is_ok) && rs[3].as_ref().is_err_and(|e| e.contains("helper failed")),
        &rs,
    );
    let e = helper_entry();
    all &= check("only the crashed process is gone", e["processes"] == 3 && !effectcraft_helper_host::blocklist::is_blocked("org.test.pool.crash"), &e);

    // A process started later knows the loaded paths already.
    effectcraft_helper_host::stop_all();
    let r = render_pool("org.test.pool.meet", 1.0);
    all &= check("a stopped pool restarts for a render", r.is_ok(), &r);
    let x = loads_of(&dir, "x.pool");
    // The new process loads x.pool once, though it was loaded twice.
    all &= check("a process started later loads what was loaded", x.len() == 5 && x.values().filter(|n| **n == 1).count() == 1, &x);

    // A path loaded while a render waits for a callback is not sent into that conversation: the
    // nested render in the callback runs on the same process, which catches up on its next turn.
    let (ready_tx, ready_rx) = channel();
    let (go_tx, go_rx) = channel();
    let renderer = std::thread::spawn(move || {
        render_pool_with("org.test.pool.ask", 0.0, &GateHost { ready: Mutex::new(ready_tx), go: Mutex::new(go_rx), nested: "org.test.pool.meet" })
    });
    let waiting = ready_rx.recv_timeout(Duration::from_secs(30)).is_ok();
    let r = effectcraft_helper_host::load_path("y.pool");
    let _ = go_tx.send(());
    let rendered = renderer.join().unwrap_or_else(|_| Err("thread panicked".into()));
    all &= check("a load during a callback is accepted", waiting && r.is_ok() && rendered.is_ok(), (waiting, &r, &rendered));
    all &=
        check("a load never reaches a render waiting for a callback", count_files(&dir, "load-during-render.") == 0, count_files(&dir, "load-during-render."));
    // Four renders at once, so all four processes: each had loaded y.pool when it rendered.
    let _ = std::fs::remove_file(dir.join("renders.log"));
    let rs = render_together(&[("org.test.pool.meet", 4.0); 4]);
    let renders = log_lines(&dir, "renders.log");
    let pids: std::collections::BTreeSet<&String> = renders.iter().map(|(pid, _)| pid).collect();
    all &= check(
        "every process catches up on loads before it is used",
        rs.iter().all(Result::is_ok) && pids.len() == 4 && renders.iter().all(|(_, loaded)| loaded.split(',').any(|p| p == "y.pool")),
        (&rs, &renders),
    );

    // Crashes are counted per effect across the pool, not per process: three turn it off. Each
    // crash ends its process, so they are spread over three processes.
    for _ in 0..2 {
        let _ = render_pool("org.test.pool.crash", 1.0);
    }
    let crashed: std::collections::BTreeSet<String> = log_lines(&dir, "crashes.log").into_iter().map(|(pid, _)| pid).collect();
    let r = render_pool("org.test.pool.crash", 1.0);
    all &= check(
        "three crashes in different processes block the effect",
        crashed.len() == 3 && r.as_ref().is_err_and(|e| e.contains("turned off")),
        (&crashed, &r),
    );

    effectcraft_helper_host::stop_all();
    all
}

/// A fresh pool whose new processes die while they start: the first try of a slot other than the
/// first doesn't count as a restart, every later one does.
fn startup_checks(dir: &Path) -> bool {
    let mut all = true;
    let restarts = || helper_entry()["restarts"].as_u64().unwrap_or(u64::MAX);
    let before = restarts();
    // Hold the running process (slot 0) in a callback, so the renders below need another slot.
    let (ready_tx, ready_rx) = channel();
    let (go_tx, go_rx) = channel();
    let holder = std::thread::spawn(move || {
        render_pool_with("org.test.startup.ask", 0.0, &GateHost { ready: Mutex::new(ready_tx), go: Mutex::new(go_rx), nested: "org.test.startup.meet" })
    });
    let waiting = ready_rx.recv_timeout(Duration::from_secs(30)).is_ok();
    let _ = mark(dir, "die-on-start");
    let first = render_pool("org.test.startup.meet", 1.0);
    let after_first = restarts();
    let second = render_pool("org.test.startup.meet", 1.0);
    let after_second = restarts();
    let _ = std::fs::remove_file(dir.join("die-on-start"));
    let _ = go_tx.send(());
    let held = holder.join().unwrap_or_else(|_| Err("thread panicked".into()));
    all &= check(
        "a slot's first start is not a restart, even when it fails",
        waiting && first.is_err() && after_first == before,
        (waiting, &first, before, after_first),
    );
    all &= check("a slot's second start is a restart", second.is_err() && after_second == before + 1, (&second, before, after_second));
    all &= check("the held render finishes", held.is_ok(), &held);
    effectcraft_helper_host::stop_all();
    all
}

/// A [`ParamChange`] of `preset` to `preset` with amount 0.8, instance 7, at 1.5 s on a 300 px
/// wide layer.
fn change(preset: u64) -> ParamChange {
    let mut values = serde_json::Map::new();
    values.insert("amount".into(), serde_json::json!(0.8));
    values.insert("preset".into(), serde_json::json!(preset));
    values.insert("tint".into(), serde_json::json!([1.0, 1.0, 1.0, 1.0]));
    values.insert("link".into(), serde_json::json!(false));
    ParamChange { param: "preset".into(), instance_key: 7, time: 1.5, layer_size: [300.0, 200.0], values, texts: vec![] }
}

/// Parameter changes (#3663), run in a child of the test with `EC_FAKE_MODE` `notify` (a helper
/// that announces `param_changed`) or `old` (one that announces nothing).
fn run_notify_checks(exe: &Path, mode: &str) -> bool {
    let mut all = true;
    let Ok(dir) = pool_dir() else { return check("EC_FAKE_DIR", false, "not set") };
    effectcraft_helper_host::blocklist::set_state_dir(dir.join("state"));
    effectcraft_helper_host::client::set_edit_timeout(Duration::from_millis(1500));
    let r = effectcraft_helper_host::scan_paths(std::slice::from_ref(&exe.to_path_buf()), false);
    all &= check(&format!("{mode}: scan registers the helper's effects"), r["loaded"].as_array().map_or(0, Vec::len) == 2, &r);
    let Some(fx) = plugin(&format!("org.test.{mode}.preset")) else { return check("the preset effect is registered", false, &r) };
    let changed = || log_lines(&dir, "changed.log").len();

    if mode == "old" {
        // A helper without the feature: nothing to notify, and nothing is ever sent to it.
        let r = fx.param_changed(&change(1));
        all &= check("old helper: no notify_params", fx.notify_params().is_empty(), fx.notify_params());
        all &= check("old helper: param_changed is never sent", r == Ok(vec![]) && changed() == 0, (&r, changed()));
        all &= check("old helper: features are empty", helper_entry()["features"] == serde_json::json!([]), helper_entry());
        effectcraft_helper_host::stop_all();
        return all;
    }

    all &= check("notify_params: the valid ids", fx.notify_params() == ["preset".to_string()], fx.notify_params());
    all &= check("list shows the features", helper_entry()["features"] == serde_json::json!(["param_changed"]), helper_entry());

    // The round trip: what the helper was told comes back in its answer.
    let r = fx.param_changed(&change(1));
    let want = vec![("amount".to_string(), serde_json::json!(0.4)), ("tint".to_string(), serde_json::json!([1.5, 3.0, 1.0, 1.0]))];
    all &= check("param_changed round trip", r.as_ref() == Ok(&want) && changed() == 1, (&r, changed()));

    // Values that don't fit are dropped, entry by entry; slider values are clamped.
    let r = fx.param_changed(&change(2));
    let want = vec![("amount".to_string(), serde_json::json!(1.0)), ("link".to_string(), serde_json::json!(true))];
    all &= check("values that don't fit are ignored", r.as_ref() == Ok(&want), &r);

    // A parameter that isn't in notify_params is not sent.
    let mut other = change(1);
    other.param = "amount".into();
    let r = fx.param_changed(&other);
    all &= check("other parameters are not sent", r == Ok(vec![]) && changed() == 2, (&r, changed()));

    // A helper that doesn't answer in time: the edit goes on after the edit timeout, the helper
    // is stopped (its late answer would be out of step) and is not charged with a crash.
    let t0 = Instant::now();
    let r = fx.param_changed(&change(3));
    let took = t0.elapsed();
    all &= check(
        "a slow answer times out within the edit timeout",
        r.as_ref().is_err_and(|e| e.contains("did not answer")) && took < Duration::from_secs(5),
        (&r, took),
    );
    all &= check("the slow helper was stopped", helper_entry()["processes"] == 0, helper_entry());
    // Not running: an edit doesn't start the helper; a render does.
    let t0 = Instant::now();
    let r = fx.param_changed(&change(1));
    all &= check("an edit doesn't start a stopped helper", r.as_ref().is_err_and(|e| e.contains("not running")) && t0.elapsed() < Duration::from_secs(1), &r);
    let r = render_pool(&format!("org.test.{mode}.ask"), 1.0);
    all &= check("a render starts it again", r.is_ok(), &r);

    // A helper that exits, or answers something else, while asked: an error, not a crash count.
    let r = fx.param_changed(&change(4));
    all &= check(
        "a helper that exits while asked is an error",
        r.is_err() && !effectcraft_helper_host::blocklist::is_blocked(&format!("org.test.{mode}.preset")),
        &r,
    );
    let _ = render_pool(&format!("org.test.{mode}.ask"), 1.0);
    let r = fx.param_changed(&change(5));
    all &= check("a reply that isn't set_params is an error", r.as_ref().is_err_and(|e| e.contains("unexpected reply")), &r);
    let _ = render_pool(&format!("org.test.{mode}.ask"), 1.0);
    let r = fx.param_changed(&change(1));
    all &= check("and the next change is answered again", r.as_ref().is_ok_and(|v| v.len() == 2), &r);

    // The only process is busy in a render: the edit waits at most the edit timeout for it, and
    // never sends into that render's conversation.
    let (ready_tx, ready_rx) = channel();
    let (go_tx, go_rx) = channel();
    let ask = format!("org.test.{mode}.ask");
    let holder = std::thread::spawn(move || {
        render_pool_with(&ask, 0.0, &GateHost { ready: Mutex::new(ready_tx), go: Mutex::new(go_rx), nested: "org.test.notify.ask" })
    });
    let waiting = ready_rx.recv_timeout(Duration::from_secs(30)).is_ok();
    let before = changed();
    let t0 = Instant::now();
    let r = fx.param_changed(&change(1));
    let took = t0.elapsed();
    let _ = go_tx.send(());
    let held = holder.join().unwrap_or_else(|_| Err("thread panicked".into()));
    all &= check(
        "a busy helper holds an edit up for the edit timeout at most",
        waiting && r.as_ref().is_err_and(|e| e.contains("busy")) && took >= Duration::from_millis(1400) && took < Duration::from_secs(5) && changed() == before,
        (waiting, &r, took),
    );
    all &= check("the render it waited for finishes", held.is_ok(), &held);
    effectcraft_helper_host::stop_all();
    all
}

/// Run the pool checks of one mode in a child of this test, so its helpers (this same program)
/// see the mode in their environment.
fn pool_group(mode: &str) -> bool {
    let (Ok(exe), dir) = (std::env::current_exe(), std::env::temp_dir().join(format!("ec-fake-{mode}-{}", std::process::id()))) else {
        return check("current exe", false, "");
    };
    let _ = std::fs::remove_dir_all(&dir);
    if std::fs::create_dir_all(&dir).is_err() {
        return check("create the marks folder", false, &dir);
    }
    let status =
        std::process::Command::new(exe).arg(POOL_ARG).env("EC_FAKE_MODE", mode).env("EC_FAKE_DIR", &dir).env("EFFECTCRAFT_HELPER_MAX_PROCESSES", "4").status();
    let _ = std::fs::remove_dir_all(&dir);
    check(&format!("pool checks ({mode}) pass"), status.is_ok_and(|s| s.success()), "see the lines above")
}

/// Starts the pool checks of the mode in `EC_FAKE_MODE`.
const POOL_ARG: &str = "--pool-checks";

/// The one test this binary reports to test runners.
const TEST_NAME: &str = "helper_protocol";

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == HELPER_ARG) {
        // The pool checks start this program with the mode of the fake in the environment.
        let r = match std::env::var("EC_FAKE_MODE").as_deref() {
            Ok("pool") => serve(&mut PoolProvider::new("pool", 4)),
            Ok("serial") => serve(&mut PoolProvider::new("serial", 1)),
            Ok("startup") => serve(&mut PoolProvider::new("startup", 4)),
            Ok("notify") => serve(&mut NotifyProvider { tag: "notify" }),
            Ok("old") => serve(&mut NotifyProvider { tag: "old" }),
            _ => serve(&mut FakeProvider),
        };
        if let Err(e) = r {
            eprintln!("fake helper: {e}");
            std::process::exit(1);
        }
        return;
    }
    if args.iter().any(|a| a == POOL_ARG) {
        let (Ok(exe), Ok(mode)) = (std::env::current_exe(), std::env::var("EC_FAKE_MODE")) else { std::process::exit(2) };
        let ok = match mode.as_str() {
            "notify" | "old" => run_notify_checks(&exe, &mode),
            _ => run_pool_checks(&exe, &mode),
        };
        if !ok {
            std::process::exit(1);
        }
        return;
    }
    // The libtest listing protocol cargo-nextest uses (`--list --format terse`, with and without
    // `--ignored`): one `name: test` line, then nextest runs `<binary> helper_protocol --exact`.
    if args.iter().any(|a| a == "--list") {
        if !args.iter().any(|a| a == "--ignored") {
            println!("{TEST_NAME}: test");
        }
        return;
    }
    // Keep the hang check short.
    effectcraft_helper_host::client::set_render_timeout(Duration::from_secs(3));
    if !run_checks() {
        std::process::exit(1);
    }
}
