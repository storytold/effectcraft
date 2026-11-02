//! An expression fuzzer (gaps G3, #786): any expression text must evaluate to a value or an
//! error, never a panic, a stack overflow (which aborts the app), a hang or a runaway allocation
//! (AGENTS.md "Never crash").
//!
//! Dependency-free, deterministic and seeded: real After Effects-style expressions (`wiggle`,
//! `loopOut`, `valueAtTime`, `thisComp.layer(…)`, effects, masks, markers, text styles, paths…)
//! are mutated character-wise (flips, deletions, duplications, splices with other seeds,
//! truncation) and token-wise (punctuation, keywords and object-model names inserted, numbers
//! swapped for extreme ones, a token repeated into deep nesting or a long operator chain), then
//! syntax-checked, evaluated on a scalar, a vector, a text and a path property of the test comp,
//! read through another layer's expression and rendered in a small frame. Inputs run on a worker
//! thread with the app's default 2 MiB stack under `catch_unwind` and a deadline: a panic or a
//! hang is reported with the iteration that reproduces it instead of taking the run down, and a
//! stack overflow aborts the run, which fails it all the same (`EFFECTCRAFT_FUZZ_VERBOSE=1`
//! prints every input before it runs).
//!
//! The default run is bounded (iterations and seconds) to stay fast in `cargo test`; a longer run:
//! `EFFECTCRAFT_FUZZ_ITERS=200000 EFFECTCRAFT_FUZZ_SECS=600 cargo test -p effectcraft-expr fuzz_`
//! (`EFFECTCRAFT_FUZZ_SEED` picks another sequence, `EFFECTCRAFT_FUZZ_START` the first iteration).
//! The `mutate` + `eval_input` pair is what a cargo-fuzz target would call.

use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

use effectcraft_render::{NoFootage, RenderOpts, Renderer};

use super::{Fx, OPA, POS, ROT, fx, secs};
use crate::{Expressions, check_syntax, eval_property, eval_standalone};

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
    fn pick<'a, T>(&mut self, xs: &'a [T]) -> &'a T {
        &xs[self.below(xs.len())]
    }
}

/// Real expressions, in the shapes people write them (and the test comp can answer them).
#[rustfmt::skip]
const SEEDS: &[&str] = &[
    "time * 360",
    "value",
    "wiggle(2, 50)",
    "wiggle(5, 20, 3, 0.5, time * 2)",
    "temporalWiggle(2, 0.1)",
    "loopOut('cycle')",
    "loopOut('pingpong', 1)",
    "loopIn('offset')",
    "loopOutDuration('continue', 0.5)",
    "valueAtTime(time - 0.5)",
    "thisProperty.valueAtTime(time / 2) + velocityAtTime(time)[0]",
    "speedAtTime(0.5)",
    "smooth(0.2, 5)",
    "nearestKey(time).index",
    "numKeys > 1 ? key(numKeys).value : value",
    "thisComp.layer('A').transform.rotation.valueAtTime(0.25)",
    "thisComp.layer(\"B\").transform.position[0] + 10",
    "thisComp.layer(1).effect('Slider Control')('Slider') * 2",
    "effect(1).param(1) + effect(1)(1)",
    "comp('Other').layer(1).name",
    "comp('Other').width / thisComp.height",
    "thisLayer.index + thisComp.numLayers",
    "mask('Mask 1').maskOpacity",
    "mask('Mask 1').maskPath.points()[0]",
    "content('Rectangle 1').content('Rectangle Path 1').size[0]",
    "thisProperty.propertyGroup(1).propertyIndex * 10 + thisProperty.propertyGroup(1).numProperties",
    "effect(1)(1).propertyGroup(1).propertyGroup(2).name",
    "marker.numKeys > 0 ? marker.key(1).time : 0",
    "marker.nearestKey(time).comment",
    "thisComp.marker.key(1).time",
    "var r = sourceRectAtTime(time, false); [r.left + r.width / 2, r.top + r.height / 2]",
    "toComp([0, 0])",
    "fromWorld(toWorld(anchorPoint))",
    "lookAt([0, 0, 0], [100, 0, 100])",
    "value + [10, -5]",
    "[1, 2] * 2 - [0.5, 0.5] / 4",
    "x = value; x += [1, 0]; x",
    "linear(time, 0, 1, 0, 100)",
    "ease(time, 0, 2, [0, 0], [100, 50])",
    "easeIn(time, 0, 1, 0, 360) + easeOut(time, 1, 2, 0, 10)",
    "clamp(value * 2, 0, 100)",
    "length([0, 0], [3, 4]) + dot([1, 0], [0, 1]) + cross([1, 0, 0], [0, 1, 0])[2]",
    "normalize([3, 4])[0] * degreesToRadians(180)",
    "seedRandom(5, true); random(100)",
    "seedRandom(index, false); gaussRandom([10, 20])",
    "noise(time) * 100",
    "posterizeTime(12); wiggle(3, 30)",
    "framesToTime(timeToFrames(time) + 1)",
    "rgbToHsl([1, 0, 0, 1])[0] + hslToRgb([0.5, 1, 0.5, 1])[1]",
    "Math.sin(time * Math.PI * 2) * 50",
    "Math.round(value / 10) * 10",
    "var s = 0; for (var i = 0; i < 10; i++) { s += i; } s",
    "function f(a) { return a * 2 } f(value)",
    "var g = (a, b) => a - b; g(5, 3)",
    "if (time > 1) { 100 } else { 0 }",
    "try { thisComp.layer('Missing').name } catch (e) { 0 }",
    "'Frame ' + timeToFrames(time) + ' of ' + thisComp.duration",
    "text.sourceText + ' world'",
    "text.sourceText.style.setText('Hi there').setFontSize(20, 3, 5).setFauxBold(true, 0, 2)",
    "thisComp.layer('Title').text.sourceText.style.fontSize",
    "`${thisLayer.name}: ${Math.floor(time)}`",
    "createPath([[0, 0], [100, 0], [100, 100]], [], [], true)",
    "var p = thisProperty; p.pointOnPath(0.5)",
    "[thisComp.width, thisComp.height] / 2",
    "thisComp.frameDuration * 10",
    "inPoint + (outPoint - inPoint) * 0.5",
    "transform.opacity * 0.5 + [0][0]",
];

/// What gets inserted: syntax, keywords, object-model names and odd characters.
#[rustfmt::skip]
const TOKENS: &[&str] = &[
    "(", ")", "[", "]", "{", "}", ",", ";", ".", "?.", "?", ":", "=", "==", "=>", "+", "-", "*", "/", "%", "**", "++", "--",
    "+=", "-=", "*=", "/=", "!", "~", "&&", "||", "??", "...", "'", "\"", "`", "${", "\\", "/*", "*/", "//", "\n", "\r", "\t",
    "\u{0}", "\u{2028}", "\u{feff}", "é", "\u{1F600}", "\u{fffd}", " in ", " of ", " new ", " typeof ", " void ", " delete ",
    "function", "return ", "var ", "let ", "const ", "if", "else", "for", "while", "do", "switch", "case ", "break", "try",
    "catch", "finally", "throw ", "class ", "this", "null", "undefined", "NaN", "Infinity", "true", "-0", "0x", "1e", "1n",
    "/a+/g", "time", "value", "index", "thisComp", "thisLayer", "thisProperty", "layer(", "comp(", "effect(", "mask(",
    "content(", "key(", "marker", "valueAtTime(", "wiggle(", "loopOut(", "sourceRectAtTime(", "toComp(", "text.sourceText",
    ".style", ".setText(", ".propertyGroup(", ".propertyIndex", "createPath(", "seedRandom(", "random(", "linear(", "__h(", "__begin", "__S", "arguments",
];

/// Numbers swapped in for numeric literals.
#[rustfmt::skip]
const NUMBERS: &[&str] = &[
    "0", "-0", "1", "-1", "0.5", "2", "16", "17", "255", "256", "65536", "4294967295", "4294967296", "9007199254740993",
    "1e308", "-1e308", "1e-320", "1e400", "NaN", "Infinity", "-Infinity", "1e9", "1e15", "-2147483648", "0x7fffffff",
];

/// A fragment that nests (or chains) once per repetition: what overflows a recursive parser.
#[rustfmt::skip]
const NESTS: &[(&str, &str)] = &[
    ("(", ")"), ("[", "]"), ("{", "}"), ("-", ""), ("!", ""), ("~", ""), ("+", ""), ("++", ""), ("--", ""), ("typeof ", ""),
    ("new ", ""), ("await ", ""), ("void ", ""), ("delete ", ""), ("x**", ""), ("x+", ""), ("x*", ""), ("x=", ""),
    ("x?x:", ""), ("x&&", ""), ("x||", ""), ("x??", ""), ("x<", ""), ("x==", ""), ("x,", ""), ("()=>", ""), ("a=>", ""),
    ("x.", "y"), ("f(", ")"), ("x[", "]"), ("`${", "}`"), ("function(){return ", "}"), ("{a:", "}"), ("if(x)", ""),
    ("x.y", ""), ("f()", ""), ("x[0]", ""),
];

/// One mutated expression from the seeds, decided by the iteration's own generator.
fn mutate(r: &mut Rng) -> String {
    let mut s: Vec<char> = r.pick(SEEDS).chars().collect();
    if r.chance(4) {
        // Splice two expressions together.
        let other: Vec<char> = r.pick(SEEDS).chars().collect();
        let at = r.below(s.len() + 1);
        let glue: Vec<char> = r.pick(&[";\n", " + ", " * ", ", ", "(", "."]).chars().collect();
        s.splice(at..at, glue.into_iter().chain(other));
    }
    for _ in 0..1 + r.below(4) {
        let len = s.len();
        match r.below(10) {
            0 if len > 0 => {
                // A bit flip in the UTF-8 bytes (invalid sequences become U+FFFD).
                let mut b = s.iter().collect::<String>().into_bytes();
                let i = r.below(b.len());
                b[i] ^= 1 << r.below(8);
                s = String::from_utf8_lossy(&b).chars().collect();
            }
            1 if len > 0 => {
                let a = r.below(len);
                let e = (a + 1 + r.below(16)).min(len);
                s.drain(a..e);
            }
            2 if len > 0 => {
                let a = r.below(len);
                let e = (a + 1 + r.below(32)).min(len);
                let chunk = s[a..e].to_vec();
                let at = r.below(len + 1);
                s.splice(at..at, chunk);
            }
            3 | 4 => {
                let t: Vec<char> = r.pick(TOKENS).chars().collect();
                let at = r.below(len + 1);
                s.splice(at..at, t);
            }
            5 => {
                // An extreme number in place of a numeric literal (or anywhere).
                let starts: Vec<usize> = (0..len).filter(|&i| s[i].is_ascii_digit() && (i == 0 || !s[i - 1].is_ascii_alphanumeric())).collect();
                let n: Vec<char> = r.pick(NUMBERS).chars().collect();
                if starts.is_empty() {
                    let at = r.below(len + 1);
                    s.splice(at..at, n);
                } else {
                    let a = *r.pick(&starts);
                    let e = (a..len).find(|&i| !(s[i].is_ascii_alphanumeric() || s[i] == '.')).unwrap_or(len);
                    s.splice(a..e, n);
                }
            }
            6 => {
                // Deep nesting or a long chain, closed or not.
                let (open, close) = *r.pick(NESTS);
                let depth = *r.pick(&[8, 40, 300, 5_000, 100_000]);
                let at = r.below(len + 1);
                let mut nest: Vec<char> = open.repeat(depth).chars().collect();
                nest.push('1');
                if !r.chance(4) {
                    nest.extend(close.repeat(depth).chars());
                }
                s.splice(at..at, nest);
            }
            7 if len > 0 => s.truncate(r.below(len)),
            8 => {
                // A whole statement in front or behind.
                let other: Vec<char> = r.pick(SEEDS).chars().collect();
                if r.chance(2) {
                    s.extend(";\n".chars().chain(other));
                } else {
                    s.splice(0..0, other.into_iter().chain(";\n".chars()));
                }
            }
            _ => {}
        }
    }
    s.into_iter().collect()
}

/// Where an input is evaluated: a scalar, a vector, a text and a path property.
const TARGETS: &[(usize, &str)] = &[(0, ROT), (0, POS), (2, "text/sourceText"), (0, "masks/#1/path"), (3, ROT), (1, OPA)];
const TIMES: &[f64] = &[0.0, 0.5, 1.25, 4.0, -1.0, 1e6];

/// Syntax-check `text`, evaluate it on one property, read it from another layer's expression
/// and render a small frame with it. `Err` only for what must never happen: an `internal error`
/// (a panic caught by a guard).
fn eval_input(f: &mut Fx, text: &str, target: usize, t: f64) -> Result<(), String> {
    internal(&check_syntax(text))?;
    let layers = [f.a, f.b, f.title, f.shape];
    let (li, path) = TARGETS[target % TARGETS.len()];
    let layer = layers[li];
    internal(&eval_standalone(&f.p, f.cid, layer, path, text, t))?;
    // As the property's own expression, read by B's rotation and drawn by the renderer.
    f.set_expr(layer, path, text);
    f.set_expr(f.b, ROT, &format!("var v = thisComp.layer({}).transform.rotation; v + 1", li + 1));
    internal(&eval_property(&f.p, f.cid, layer, path, t))?;
    internal(&eval_property(&f.p, f.cid, f.b, ROT, t))?;
    let host = Expressions;
    let mut r = Renderer::new(&f.p, &NoFootage, RenderOpts { motion_blur: false, scale: 0.25, ..Default::default() });
    r.expr = Some(&host);
    r.comp_frame(f.cid, secs(t));
    // Leave the fixture as it was.
    let l = f.layer_mut(layer);
    if let Some(p) = l.props.prop_mut(path) {
        p.expr = None;
    }
    if let Some(p) = f.layer_mut(f.b).props.prop_mut(ROT) {
        p.expr = None;
    }
    Ok(())
}

fn internal<T>(r: &Result<T, String>) -> Result<(), String> {
    match r {
        Err(e) if e.contains("internal error") => Err(e.clone()),
        _ => Ok(()),
    }
}

struct Job {
    text: String,
    target: usize,
    t: f64,
}

/// A worker thread that keeps its expression runtime between inputs, like a render thread.
struct Worker {
    jobs: Sender<Job>,
    results: Receiver<Result<(), String>>,
}

impl Worker {
    fn start() -> Result<Worker, String> {
        let (jobs, rx) = channel::<Job>();
        let (tx, results) = channel();
        std::thread::Builder::new()
            .name("fuzz-expr".into())
            .stack_size(2 << 20)
            .spawn(move || {
                let mut f = fx();
                for job in rx {
                    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| eval_input(&mut f, &job.text, job.target, job.t)));
                    let r = r.unwrap_or_else(|p| {
                        Err(format!(
                            "panic: {}",
                            p.downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| p.downcast_ref::<String>().cloned()).unwrap_or_default()
                        ))
                    });
                    let failed = r.is_err();
                    if tx.send(r).is_err() || failed {
                        break; // after a panic the fixture and runtime may be half-updated: start over
                    }
                }
            })
            .map_err(|e| format!("couldn't start the fuzz thread: {e}"))?;
        Ok(Worker { jobs, results })
    }

    /// Run one input: a panic becomes its message, a hang a timeout (the worker is then dropped).
    fn run(&self, job: Job, deadline: Duration) -> Result<(), String> {
        self.jobs.send(job).map_err(|_| "the fuzz worker exited".to_string())?;
        self.results.recv_timeout(deadline).unwrap_or_else(|_| Err(format!("hang: no result within {deadline:?}")))
    }
}

fn env_u64(name: &str, default: u64) -> u64 {
    std::env::var(name).ok().and_then(|v| v.trim().parse().ok()).unwrap_or(default)
}

#[test]
fn fuzz_expressions_never_crash() {
    let seed = env_u64("EFFECTCRAFT_FUZZ_SEED", 0x00EC_E8E2);
    let start = env_u64("EFFECTCRAFT_FUZZ_START", 0);
    let iters = env_u64("EFFECTCRAFT_FUZZ_ITERS", 1_500);
    let budget = Duration::from_secs(env_u64("EFFECTCRAFT_FUZZ_SECS", 5));
    let verbose = env_u64("EFFECTCRAFT_FUZZ_VERBOSE", 0) != 0;
    // Every evaluation gives up after `MAX_TIME`, and an input is evaluated a handful of times.
    let deadline = crate::MAX_TIME * 24;
    let mut worker = Worker::start().unwrap();
    // The unmutated seeds evaluate everywhere without crashing.
    for (i, s) in SEEDS.iter().enumerate() {
        assert_eq!(worker.run(Job { text: s.to_string(), target: i, t: 0.5 }, deadline), Ok(()), "{s}");
    }
    let began = Instant::now();
    let mut failures = vec![];
    let mut ran = 0;
    for i in start..start + iters {
        if began.elapsed() > budget {
            break;
        }
        let mut r = Rng(seed ^ i.wrapping_mul(0xD6E8_FEB8_6659_FD93));
        let text = mutate(&mut r);
        let target = r.below(TARGETS.len());
        let t = *r.pick(TIMES);
        if verbose {
            eprintln!("iteration {i}: target {target}, t {t}, {} chars: {:?}", text.len(), text.chars().take(300).collect::<String>());
        }
        ran += 1;
        if let Err(e) = worker.run(Job { text: text.clone(), target, t }, deadline) {
            let head: String = text.chars().take(200).collect();
            failures.push(format!("iteration {i} (seed {seed:#x}, target {target}, t {t}, {} chars): {e}\n  input starts: {head:?}", text.len()));
            if failures.len() >= 5 {
                break;
            }
            worker = Worker::start().unwrap();
        }
    }
    assert!(failures.is_empty(), "expressions crashed on {} of {ran} fuzz inputs:\n{}", failures.len(), failures.join("\n"));
    eprintln!("fuzz_expressions: {ran} inputs in {:?}, no crashes", began.elapsed());
}
