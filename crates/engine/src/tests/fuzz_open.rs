//! A project-file fuzzer (gaps G3, #779): File ▸ Open must turn any bytes into a project or an
//! error, never a panic, an abort or a hang (AGENTS.md "Never crash").
//!
//! Dependency-free, deterministic and seeded: seed projects (empty, and the demo cut down to one
//! layer at a time, each as `.ecproj` JSON and as `.ecprojx` XML) are mutated byte-wise (flips,
//! deletions, splices, truncation, interesting tokens, deep nesting) and structurally (a JSON
//! node replaced by an extreme number, another type or another subtree; members dropped,
//! elements duplicated), then opened through the real `file.open` path, followed by the queries
//! the app and agents run on a fresh project and a thumbnail-size render of the first frame.
//! Each input runs on its own thread under `catch_unwind` with a deadline, so a panic or a hang
//! is reported (with the iteration to reproduce it) instead of taking the test run down; a stack
//! overflow can't be caught and aborts the run, which fails it all the same.
//!
//! The default run is bounded (iterations and seconds) to stay fast in `cargo test`; a longer run:
//! `EFFECTCRAFT_FUZZ_ITERS=200000 EFFECTCRAFT_FUZZ_SECS=600 cargo test -p effectcraft-engine fuzz_`
//! (`EFFECTCRAFT_FUZZ_SEED` picks another sequence, `EFFECTCRAFT_FUZZ_START` the first iteration).
//! The `mutate` + `open_input` pair is what a cargo-fuzz target would call.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::{Services, Session};

const PATH: &str = "/fuzz/input.ecproj";

/// splitmix64: tiny, good enough for picking mutations.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    /// `0..n` (`0` when `n` is 0).
    fn below(&mut self, n: usize) -> usize {
        if n == 0 { 0 } else { (self.next() % n as u64) as usize }
    }
    fn chance(&mut self, one_in: usize) -> bool {
        self.below(one_in) == 0
    }
}

/// One file, served from memory; everything else is missing.
struct OneFile(Vec<u8>);

impl Services for OneFile {
    fn read_file(&self, path: &str) -> std::io::Result<Vec<u8>> {
        if path == PATH { Ok(self.0.clone()) } else { Err(std::io::Error::new(std::io::ErrorKind::NotFound, path.to_string())) }
    }
    fn write_file(&self, _: &str, _: &[u8]) -> std::io::Result<()> {
        Err(std::io::Error::other("the fuzzer writes nothing"))
    }
    fn exists(&self, path: &str) -> bool {
        path == PATH
    }
    fn stamp(&self, _: &str) -> Option<(u64, u128)> {
        None
    }
    fn list_dir(&self, _: &str) -> Vec<String> {
        vec![]
    }
    fn is_dir(&self, _: &str) -> bool {
        false
    }
}

/// Open `bytes` as a project file, then run what the app and agents read first: `Ok(true)` when
/// it opened, `Ok(false)` when it was refused. `Err` only for a crash: a panic in the open
/// itself, or one the command guard caught in a query.
fn open_input(bytes: Vec<u8>) -> Result<bool, String> {
    let mut s = Session { services: Arc::new(OneFile(bytes)), ..Default::default() };
    if crate::commands::open_for_tests(&mut s, &json!({ "path": PATH })).is_err() {
        return Ok(false); // refused: fine
    }
    let crashed = |r: crate::Result<Value>| match r {
        Err(e) if e.to_string().contains("internal error") => Err(e.to_string()),
        _ => Ok(()),
    };
    crashed(s.execute("project.summary", json!({})))?;
    let comps: Vec<_> = s.project.comps().map(|(id, c)| (id.0, c.layers.iter().map(|l| l.id.0).collect::<Vec<_>>())).collect();
    for (comp, layers) in comps.into_iter().take(4) {
        crashed(s.execute("comp.info", json!({ "comp": comp })))?;
        for layer in layers.into_iter().take(8) {
            crashed(s.execute("layer.tree", json!({ "comp": comp, "layer": layer, "depth": 3, "time": 1.0 })))?;
        }
    }
    // The first frame the viewer shows, at thumbnail size.
    if let Some(id) = s.state.active_comp
        && let Some(c) = s.project.comp(id)
    {
        let scale = (32.0 / f64::from(c.width.max(c.height).max(1))).min(1.0);
        effectcraft_render::render_frame(&s.project, id, effectcraft_time::Tick::from_seconds_f64(1.0), scale);
    }
    Ok(true)
}

/// Run one input on its own thread: a panic becomes its message, a hang a timeout.
fn run_guarded(bytes: Vec<u8>, deadline: Duration) -> Result<bool, String> {
    let (tx, rx) = std::sync::mpsc::channel();
    let spawned = std::thread::Builder::new().name("fuzz-open".into()).stack_size(8 << 20).spawn(move || {
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| open_input(bytes)));
        let r = match r {
            Ok(r) => r,
            Err(p) => {
                Err(format!("panic: {}", p.downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| p.downcast_ref::<String>().cloned()).unwrap_or_default()))
            }
        };
        let _ = tx.send(r);
    });
    if let Err(e) = spawned {
        return Err(format!("couldn't start the fuzz thread: {e}"));
    }
    rx.recv_timeout(deadline).unwrap_or_else(|_| Err(format!("hang: no result within {deadline:?}")))
}

/// Small projects covering what a project holds: the empty project, and the demo cut down to
/// one layer of one comp at a time (shape, text, solid, precomp, camera, light, null, effects,
/// masks, expressions; other comps keep their first layer), each as JSON and as XML. The whole
/// demo (700 kB) would make every input slow.
fn seeds() -> Vec<Vec<u8>> {
    let demo = crate::demo::demo_project();
    let mut projects = vec![effectcraft_project::Project::default()];
    for (cid, c) in demo.comps() {
        for i in 0..c.layers.len() {
            let mut p = demo.clone();
            for (id, item) in p.items.iter_mut() {
                if let effectcraft_project::ItemKind::Comp(c) = &mut item.kind {
                    let keep = if id == cid { i } else { 0 };
                    let layers = &mut Arc::make_mut(c).layers;
                    *layers = layers.get(keep).cloned().into_iter().collect();
                }
            }
            projects.push(p);
        }
    }
    let mut out = vec![];
    for p in projects {
        let json = p.to_file_json().unwrap();
        out.push(crate::xml_project::to_xml(&json).unwrap().into_bytes());
        out.push(json.into_bytes());
    }
    out
}

#[rustfmt::skip]
const TOKENS: &[&str] = &[
    "{", "}", "[", "]", "\"", ",", ":", "null", "true", "-1", "0", "1e308", "-1e308", "1e-320", "18446744073709551615",
    "-9223372036854775808", "4294967295", "4294967296", "\\u0000", "\\ud800", "<", ">", "</", "/>", "<arr>", "</arr>", "<obj>",
    "<m n=\"", "</m>", "<num>", "</num>", "<str>", "</str>", "<null/>", "<!--", "-->", "<?", "?>", "&", ";", "&#x110000;",
    "&#xD800;", "&#;", "&#x;", "&amp", "\u{feff}", "é", "\u{1F600}",
];

const NUMBERS: &[f64] = &[0.0, -0.0, 1.0, -1.0, 0.5, -0.5, 2.0, 1e-300, 1e15, 1e300, -1e300, 4294967295.0, 4294967296.0, 65536.0, 254016000000.0];
const INTEGERS: &[i64] = &[0, -1, 1, 2, 3, 255, 256, 65535, 2147483647, -2147483648, i64::MAX, i64::MIN, 9_007_199_254_740_993];

/// Every node of `v`, depth first (the root is 0).
fn count(v: &Value) -> usize {
    1 + match v {
        Value::Array(a) => a.iter().map(count).sum(),
        Value::Object(m) => m.values().map(count).sum(),
        _ => 0,
    }
}

/// The `k`-th node (depth first; the root is 0).
fn nth<'a>(v: &'a mut Value, k: &mut usize) -> Option<&'a mut Value> {
    if *k == 0 {
        return Some(v);
    }
    *k -= 1;
    match v {
        Value::Array(a) => a.iter_mut().find_map(|x| nth(x, k)),
        Value::Object(m) => m.values_mut().find_map(|x| nth(x, k)),
        _ => None,
    }
}

fn node_mut(v: &mut Value, k: usize) -> Option<&mut Value> {
    nth(v, &mut { k })
}

fn interesting(r: &mut Rng, around: &Value) -> Value {
    match r.below(7) {
        0 => json!(NUMBERS[r.below(NUMBERS.len())]),
        1 => json!(INTEGERS[r.below(INTEGERS.len())]),
        2 => match around.as_f64() {
            Some(x) => json!(x * [-1.0, 2.0, 1e6, 1e-6, 0.0][r.below(5)]),
            None => Value::Null,
        },
        3 => json!(["", "#999", "@0", "effects/#1", "../../x", "\u{0}", "transform/position", &"a".repeat(r.below(300))][r.below(8)]),
        4 => Value::Bool(r.chance(2)),
        5 => json!([]),
        _ => Value::Null,
    }
}

/// A structural mutation of a project's JSON (when the input still parses).
fn mutate_tree(r: &mut Rng, v: &mut Value) {
    let n = count(v);
    for _ in 0..1 + r.below(4) {
        let k = r.below(n.max(1));
        let donor = node_mut(v, r.below(n.max(1))).map(|d| d.clone()).unwrap_or(Value::Null);
        let Some(node) = node_mut(v, k) else { return };
        match r.below(6) {
            0 | 1 => *node = interesting(r, node),
            2 => *node = donor,
            3 => match node {
                Value::Object(m) if !m.is_empty() => {
                    let key = m.keys().nth(r.below(m.len())).cloned().unwrap_or_default();
                    m.remove(&key);
                }
                Value::Array(a) if !a.is_empty() => {
                    a.remove(r.below(a.len()));
                }
                _ => *node = Value::Null,
            },
            4 => {
                if let Value::Array(a) = node
                    && !a.is_empty()
                {
                    let i = r.below(a.len());
                    let x = a[i].clone();
                    a.insert(r.below(a.len() + 1), x);
                }
            }
            _ => {
                if let Value::Object(m) = node {
                    m.insert(["id", "parent", "source", "schema", "nextId", "layers", "items", "kind"][r.below(8)].into(), interesting(r, &Value::Null));
                }
            }
        }
    }
}

fn mutate_bytes(r: &mut Rng, b: &mut Vec<u8>) {
    for _ in 0..1 + r.below(3) {
        let len = b.len();
        match r.below(8) {
            0 if len > 0 => {
                let i = r.below(len);
                b[i] ^= 1 << r.below(8);
            }
            1 if len > 0 => {
                let i = r.below(len);
                b[i] = r.next() as u8;
            }
            2 if len > 0 => {
                let a = r.below(len);
                let e = (a + 1 + r.below(64)).min(len);
                b.drain(a..e);
            }
            3 if len > 0 => {
                let a = r.below(len);
                let e = (a + 1 + r.below(256)).min(len);
                let chunk = b[a..e].to_vec();
                let at = r.below(len + 1);
                b.splice(at..at, chunk);
            }
            4 => {
                let t = TOKENS[r.below(TOKENS.len())].as_bytes();
                let at = r.below(len + 1);
                b.splice(at..at, t.iter().copied());
            }
            5 if len > 0 => b.truncate(r.below(len)),
            6 => {
                // Deep nesting: what a recursive reader overflows its stack on.
                let (open, close) = [("[", "]"), ("<arr>", "</arr>"), ("<obj><m n=\"a\">", "</m></obj>")][r.below(3)];
                let depth = [200, 5_000, 200_000][r.below(3)];
                // Where the reader expects a value: after an opening token of the same kind.
                let opens: Vec<usize> = b.windows(open.len().min(5)).enumerate().filter(|(_, w)| *w == &open.as_bytes()[..w.len()]).map(|(i, _)| i).collect();
                let at = if opens.is_empty() { r.below(len + 1) } else { opens[r.below(opens.len())] };
                let mut nest = open.repeat(depth).into_bytes();
                nest.extend(close.repeat(if r.chance(2) { depth } else { 0 }).into_bytes());
                b.splice(at..at, nest);
            }
            _ => {}
        }
    }
}

/// One fuzz input from the seeds, decided by the iteration's own generator.
fn mutate(r: &mut Rng, seeds: &[Vec<u8>]) -> Vec<u8> {
    let mut b = seeds[r.below(seeds.len())].clone();
    if r.chance(2) {
        let text = String::from_utf8_lossy(&b).to_string();
        let json = if crate::xml_project::is_xml(&text) { crate::xml_project::from_xml(&text).ok() } else { Some(text) };
        if let Some(mut v) = json.and_then(|j| serde_json::from_str::<Value>(&j).ok()) {
            mutate_tree(r, &mut v);
            let j = v.to_string();
            b = if r.chance(3) { crate::xml_project::to_xml(&j).unwrap_or(j).into_bytes() } else { j.into_bytes() };
        }
    }
    if r.chance(3) || b.is_empty() {
        mutate_bytes(r, &mut b);
    }
    b
}

fn env_u64(name: &str, default: u64) -> u64 {
    std::env::var(name).ok().and_then(|v| v.trim().parse().ok()).unwrap_or(default)
}

#[test]
fn fuzz_file_open_never_crashes() {
    let seed = env_u64("EFFECTCRAFT_FUZZ_SEED", 0x00EC_F022);
    let start = env_u64("EFFECTCRAFT_FUZZ_START", 0);
    let iters = env_u64("EFFECTCRAFT_FUZZ_ITERS", 3_000);
    let budget = Duration::from_secs(env_u64("EFFECTCRAFT_FUZZ_SECS", 6));
    let seeds = seeds();
    // The unmutated seeds open.
    for s in &seeds {
        assert_eq!(run_guarded(s.clone(), Duration::from_secs(30)), Ok(true));
    }
    let began = Instant::now();
    let mut failures = vec![];
    let (mut ran, mut opened) = (0, 0);
    for i in start..start + iters {
        if began.elapsed() > budget {
            break;
        }
        let mut r = Rng(seed ^ i.wrapping_mul(0xD6E8_FEB8_6659_FD93));
        let input = mutate(&mut r, &seeds);
        ran += 1;
        match run_guarded(input.clone(), Duration::from_secs(10)) {
            Ok(o) => opened += usize::from(o),
            Err(e) => {
                let head = String::from_utf8_lossy(&input[..input.len().min(200)]).to_string();
                failures.push(format!("iteration {i} (seed {seed:#x}, {} bytes): {e}\n  input starts: {head:?}", input.len()));
                if failures.len() >= 5 {
                    break;
                }
            }
        }
    }
    assert!(failures.is_empty(), "File ▸ Open crashed on {} of {ran} fuzz inputs:\n{}", failures.len(), failures.join("\n"));
    eprintln!("fuzz_file_open: {ran} inputs ({opened} opened) in {:?}, no crashes", began.elapsed());
}
