//! The per-thread boa runtime: one `Context` with the prelude loaded, a compiled-script cache,
//! the native functions, and the request/miss bookkeeping (see [`crate::host`]).

use std::cell::RefCell;
use std::collections::HashMap;
use std::mem::ManuallyDrop;

use boa_engine::object::builtins::JsArray;
use boa_engine::{Context, JsNativeError, JsObject, JsResult, JsString, JsValue, NativeFunction, Script, Source, js_string};
use effectcraft_keyframe::Value;

use crate::host::{Key, Req, Resp, secs};
use crate::noise;

const PRELUDE: &str = include_str!("prelude.js");
/// Message of the error thrown when a request isn't answered yet (never shown to users).
const PENDING: &str = "\u{1}effectcraft: pending host request";
const MAX_SCRIPTS: usize = 4096;
/// Loop iterations one script run may make (per function call): a run re-executes once per
/// property it reads, so this bounds the time of every pass.
pub const LOOP_LIMIT: u64 = 1_000_000;
/// The error of an evaluation that ran out of time.
pub const TIMED_OUT: &str = "Error: expression takes too long to evaluate";
/// The part of boa's stack-size limit error the runtime recovers from (see [`run`]).
const STACK_LIMIT: &str = "maximum stack size";

/// Memoized host answers and pending misses of one expression evaluation.
#[derive(Default)]
pub struct Frame {
    pub memo: HashMap<Req, Resp>,
    pub misses: Vec<Req>,
    /// What this frame evaluates: an evaluation that needs itself is a cycle.
    pub owner: Option<Owner>,
}

/// One expression evaluation: the property (comp, layer, uid), the comp time and the text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Owner {
    pub comp: u64,
    pub layer: u64,
    pub uid: u64,
    pub time: effectcraft_time::Tick,
    pub text: String,
}

thread_local! {
    /// Never dropped: boa's own thread-local heap may be torn down first at thread exit, and
    /// dropping GC handles after that aborts the process. The OS reclaims it with the thread.
    static RUNTIME: RefCell<Option<ManuallyDrop<Result<Runtime, String>>>> = const { RefCell::new(None) };
    /// Evaluation stack (nested when one expression reads another's value).
    pub static FRAMES: RefCell<Vec<Frame>> = const { RefCell::new(Vec::new()) };
    /// Script runs since the outermost evaluation on this thread began (nested ones included).
    pub static RUNS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(not(target_arch = "wasm32"))]
thread_local! {
    /// When the outermost evaluation on this thread began.
    pub static STARTED: std::cell::Cell<Option<std::time::Instant>> = const { std::cell::Cell::new(None) };
}

struct Runtime {
    ctx: Context,
    scripts: HashMap<String, Result<Script, String>>,
    begin: JsObject,
    finish: JsObject,
}

/// Per-evaluation inputs for `__begin`.
pub struct Begin<'a> {
    pub time: f64,
    pub value: &'a Resp,
    pub comp: u64,
    pub layer: u64,
    pub path: &'a str,
    pub uid: u64,
    pub index: f64,
    pub frame_duration: f64,
    /// Text Expression Selector `[textIndex, textTotal]` and `selectorValue` (percent).
    pub text_sel: [f64; 2],
    pub selector: [f64; 3],
}

/// Outcome of one script run.
pub enum Run {
    Done(Result<Out, String>),
    /// The script needs host answers first.
    Pending,
}

/// A script result, detached from the JS heap.
#[derive(Clone, Debug, PartialEq)]
pub enum Out {
    Undefined,
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Arr(Vec<Out>),
    Path {
        points: Vec<[f64; 2]>,
        ins: Vec<[f64; 2]>,
        outs: Vec<[f64; 2]>,
        closed: bool,
    },
    /// A text style object: JSON `{doc: TextDoc | null, ops: [[key, value, start, count], …]}`.
    Style(String),
    Object(String),
}

fn arg(args: &[JsValue], i: usize) -> JsValue {
    args.get(i).cloned().unwrap_or_default()
}
fn num(args: &[JsValue], i: usize, ctx: &mut Context) -> JsResult<f64> {
    arg(args, i).to_number(ctx)
}
fn id(args: &[JsValue], i: usize, ctx: &mut Context) -> JsResult<u64> {
    let v = num(args, i, ctx)?;
    Ok(if v.is_finite() && v >= 0.0 { v as u64 } else { u64::MAX })
}
fn string(args: &[JsValue], i: usize, ctx: &mut Context) -> JsResult<String> {
    Ok(arg(args, i).to_string(ctx)?.to_std_string_escaped())
}
fn key(args: &[JsValue], i: usize, ctx: &mut Context) -> JsResult<Key> {
    let v = arg(args, i);
    Ok(match v.as_number() {
        Some(n) => Key::Index(n as i64),
        None => Key::Name(v.to_string(ctx)?.to_std_string_escaped()),
    })
}

fn to_js(r: &Resp, ctx: &mut Context) -> JsValue {
    match r {
        Resp::Null => JsValue::null(),
        Resp::Bool(b) => JsValue::from(*b),
        Resp::Num(n) => JsValue::from(*n),
        Resp::Str(s) => JsValue::from(JsString::from(s.as_str())),
        Resp::List(items) => {
            let vals: Vec<JsValue> = items.iter().map(|i| to_js(i, ctx)).collect();
            JsArray::from_iter(vals, ctx).into()
        }
    }
}

/// `__h(op, …)`: answer a host request from the memo, or record a miss and abort.
fn native_host(_: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let op = num(args, 0, ctx)? as i64;
    let req = match op {
        0 => Req::CompInfo { comp: id(args, 1, ctx)? },
        1 => Req::CompByName { name: string(args, 1, ctx)? },
        2 => Req::LayerLookup { comp: id(args, 1, ctx)?, key: key(args, 2, ctx)? },
        3 => Req::LayerInfo { comp: id(args, 1, ctx)?, layer: id(args, 2, ctx)? },
        4 => Req::Child { comp: id(args, 1, ctx)?, layer: id(args, 2, ctx)?, group: string(args, 3, ctx)?, key: key(args, 4, ctx)? },
        5 => Req::PropInfo { comp: id(args, 1, ctx)?, layer: id(args, 2, ctx)?, path: string(args, 3, ctx)? },
        6 => Req::Value {
            comp: id(args, 1, ctx)?,
            layer: id(args, 2, ctx)?,
            path: string(args, 3, ctx)?,
            t: secs(num(args, 4, ctx)?),
            pre: arg(args, 5).to_boolean(),
        },
        7 => Req::Xform { comp: id(args, 1, ctx)?, layer: id(args, 2, ctx)?, t: secs(num(args, 3, ctx)?) },
        8 => Req::SourceRect { comp: id(args, 1, ctx)?, layer: id(args, 2, ctx)?, t: secs(num(args, 3, ctx)?), extents: arg(args, 4).to_boolean() },
        9 => {
            let l = num(args, 2, ctx)?;
            Req::Markers { comp: id(args, 1, ctx)?, layer: (l >= 0.0).then_some(l as u64) }
        }
        10 => Req::Doc {
            comp: id(args, 1, ctx)?,
            layer: id(args, 2, ctx)?,
            path: string(args, 3, ctx)?,
            t: secs(num(args, 4, ctx)?),
            pre: arg(args, 5).to_boolean(),
        },
        11 => Req::Footage { name: string(args, 1, ctx)? },
        12 => Req::FootageData { name: string(args, 1, ctx)? },
        13 => Req::Sample {
            comp: id(args, 1, ctx)?,
            layer: id(args, 2, ctx)?,
            x: secs(num(args, 3, ctx)?),
            y: secs(num(args, 4, ctx)?),
            rx: secs(num(args, 5, ctx)?),
            ry: secs(num(args, 6, ctx)?),
            post: arg(args, 7).to_boolean(),
            t: secs(num(args, 8, ctx)?),
        },
        14 => Req::Project,
        15 => Req::SourceInfo { comp: id(args, 1, ctx)?, layer: id(args, 2, ctx)? },
        _ => return Err(JsNativeError::typ().with_message("unknown host request").into()),
    };
    let found = FRAMES.with(|f| {
        let mut f = f.borrow_mut();
        let frame = f.last_mut()?;
        match frame.memo.get(&req) {
            Some(r) => Some(r.clone()),
            None => {
                frame.misses.push(req);
                None
            }
        }
    });
    match found {
        Some(r) => Ok(to_js(&r, ctx)),
        None => Err(JsNativeError::error().with_message(PENDING).into()),
    }
}

/// `__wig(layerIndex, uid, dims, freq, amp, octaves, ampMult, t)`.
fn native_wiggle(_: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let mut a = [0.0; 8];
    for (i, x) in a.iter_mut().enumerate() {
        *x = num(args, i, ctx)?;
    }
    let seed = noise::mix(&[a[0].to_bits(), a[1].to_bits()]);
    let dims = (a[2].max(1.0) as usize).min(16);
    let off = noise::wiggle(seed, dims, a[3], a[4], a[5], a[6], a[7]);
    let vals: Vec<JsValue> = off.into_iter().map(JsValue::from).collect();
    Ok(JsArray::from_iter(vals, ctx).into())
}

/// `__noise(x, y, z)`.
fn native_noise(_: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    Ok(JsValue::from(noise::noise3(num(args, 0, ctx)?, num(args, 1, ctx)?, num(args, 2, ctx)?)))
}

/// `__rnd(layerIndex, uid, userSeed, timeless, time, counter)`.
fn native_random(_: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let timeless = arg(args, 3).to_boolean();
    let t = if timeless { 0.0 } else { num(args, 4, ctx)? };
    let parts = [num(args, 0, ctx)?.to_bits(), num(args, 1, ctx)?.to_bits(), num(args, 2, ctx)?.to_bits(), t.to_bits(), num(args, 5, ctx)?.to_bits()];
    Ok(JsValue::from(noise::unit(noise::mix(&parts))))
}

impl Runtime {
    fn new() -> Result<Runtime, String> {
        let mut ctx = Context::default();
        let limits = ctx.runtime_limits_mut();
        limits.set_loop_iteration_limit(LOOP_LIMIT);
        limits.set_recursion_limit(400);
        let natives: [(JsString, usize, fn(&JsValue, &[JsValue], &mut Context) -> JsResult<JsValue>); 4] = [
            (js_string!("__h"), 6, native_host),
            (js_string!("__wig"), 8, native_wiggle),
            (js_string!("__noise"), 3, native_noise),
            (js_string!("__rnd"), 6, native_random),
        ];
        for (name, len, f) in natives {
            ctx.register_global_callable(name, len, NativeFunction::from_fn_ptr(f)).map_err(|e| e.to_string())?;
        }
        ctx.eval(Source::from_bytes(PRELUDE)).map_err(|e| format!("expression prelude: {e}"))?;
        let global = ctx.global_object();
        let func = |name: JsString, ctx: &mut Context| -> Result<JsObject, String> {
            global.get(name, ctx).ok().and_then(|v| v.as_object()).ok_or_else(|| "expression prelude: missing function".to_string())
        };
        let begin = func(js_string!("__begin"), &mut ctx)?;
        let finish = func(js_string!("__finish"), &mut ctx)?;
        Ok(Runtime { ctx, scripts: HashMap::new(), begin, finish })
    }

    fn script(&mut self, text: &str) -> Result<Script, String> {
        if let Some(s) = self.scripts.get(text) {
            return s.clone();
        }
        if self.scripts.len() >= MAX_SCRIPTS {
            self.scripts.clear();
        }
        // A block keeps `let`/`const` local to one evaluation; `{` shares line 1 with the code so
        // error line numbers match the user's text. Errors are rethrown from the script's own
        // frame: boa leaves the stack of the inner frames behind when an exception leaves the
        // script from a nested call (every host request does), and the thread's runtime would
        // fail every expression with "reached the maximum stack size" after a few hundred.
        let src = format!("try{{{}\n}}catch(__e){{throw __e}}", crate::rewrite::rewrite(text));
        let s = Script::parse(Source::from_bytes(src.as_bytes()), None, &mut self.ctx).map_err(|e| format_error(&e.to_string(), line_count(text)));
        self.scripts.insert(text.to_string(), s.clone());
        s
    }

    fn run(&mut self, text: &str, b: &Begin) -> Run {
        let script = match self.script(text) {
            Ok(s) => s,
            Err(e) => return Run::Done(Err(e)),
        };
        let ctx = &mut self.ctx;
        let args = [
            JsValue::from(b.time),
            to_js(b.value, ctx),
            JsValue::from(b.comp as f64),
            JsValue::from(b.layer as f64),
            JsValue::from(JsString::from(b.path)),
            JsValue::from(b.uid as f64),
            JsValue::from(b.index),
            JsValue::from(b.frame_duration),
            JsValue::from(b.text_sel[0]),
            JsValue::from(b.text_sel[1]),
            to_js(&crate::host::Resp::List(b.selector.iter().map(|x| crate::host::Resp::Num(*x)).collect()), ctx),
        ];
        if let Err(e) = self.begin.call(&JsValue::undefined(), &args, ctx) {
            return Run::Done(Err(format_error(&e.to_string(), line_count(text))));
        }
        let result = match evaluate_in_time(&script, ctx) {
            Some(r) => r.and_then(|v| self.finish.call(&JsValue::undefined(), &[v], ctx)),
            // Abandoned mid-run: the context is unusable now (see `run`).
            None => return Run::Done(Err(TIMED_OUT.into())),
        };
        let pending = FRAMES.with(|f| f.borrow().last().is_some_and(|fr| !fr.misses.is_empty()));
        if pending {
            return Run::Pending;
        }
        Run::Done(match result {
            Ok(v) => detach(&v, ctx, 0),
            Err(e) => Err(format_error(&e.to_string(), line_count(text))),
        })
    }
}

/// Run `script`, giving up (`None`) once the outermost evaluation is out of time
/// ([`crate::MAX_TIME`]). boa's budgeted evaluation yields every few thousand instructions of
/// the script and the functions it calls (not inside callbacks of native functions such as
/// `Array.prototype.map`, which stay bounded by the loop limit per call).
fn evaluate_in_time(script: &Script, ctx: &mut Context) -> Option<JsResult<JsValue>> {
    let mut run = std::pin::pin!(script.evaluate_async_with_budget(ctx, 20_000));
    let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
    loop {
        if let std::task::Poll::Ready(r) = run.as_mut().poll(&mut cx) {
            return Some(r);
        }
        if out_of_time() {
            return None;
        }
    }
}

#[cfg(test)]
thread_local! {
    /// Test hook: this thread's time budget (tests of the run-count budget lift it so that a
    /// loaded machine cannot reach the clock first).
    pub static TIME_LIMIT: std::cell::Cell<std::time::Duration> = const { std::cell::Cell::new(crate::MAX_TIME) };
}

#[cfg(not(target_arch = "wasm32"))]
fn max_time() -> std::time::Duration {
    #[cfg(test)]
    {
        TIME_LIMIT.with(|t| t.get())
    }
    #[cfg(not(test))]
    {
        crate::MAX_TIME
    }
}

/// Whether the outermost evaluation on this thread has used up [`crate::MAX_TIME`].
pub fn out_of_time() -> bool {
    #[cfg(not(target_arch = "wasm32"))]
    {
        STARTED.with(|s| s.get()).is_some_and(|t| t.elapsed() > max_time())
    }
    #[cfg(target_arch = "wasm32")]
    {
        false
    }
}

/// Copy a JS value out of the heap.
fn detach(v: &JsValue, ctx: &mut Context, depth: usize) -> Result<Out, String> {
    if v.is_undefined() {
        return Ok(Out::Undefined);
    }
    if v.is_null() {
        return Ok(Out::Null);
    }
    if let Some(b) = v.as_boolean() {
        return Ok(Out::Bool(b));
    }
    if let Some(n) = v.as_number() {
        return Ok(Out::Num(n));
    }
    if let Some(s) = v.as_string() {
        return Ok(Out::Str(s.to_std_string_escaped()));
    }
    let err = |e: boa_engine::JsError| format_error(&e.to_string(), usize::MAX);
    if let Some(o) = v.as_object() {
        if o.is_array() && depth < 4 {
            let arr = JsArray::from_object(o).map_err(err)?;
            let n = arr.length(ctx).map_err(err)?.min(1 << 16);
            let mut out = Vec::with_capacity(n as usize);
            for i in 0..n {
                out.push(detach(&arr.get(i, ctx).map_err(err)?, ctx, depth + 1)?);
            }
            return Ok(Out::Arr(out));
        }
        if o.get(js_string!("__style"), ctx).map_err(err)?.to_boolean() {
            let j = o.get(js_string!("__json"), ctx).map_err(err)?;
            return Ok(Out::Style(j.as_string().map(|s| s.to_std_string_escaped()).unwrap_or_default()));
        }
        if o.get(js_string!("__path"), ctx).map_err(err)?.to_boolean() {
            let pts = |name: JsString, ctx: &mut Context| -> Result<Vec<[f64; 2]>, String> {
                let v = o.get(name, ctx).map_err(err)?;
                Ok(match detach(&v, ctx, depth + 1)? {
                    Out::Arr(items) => items.iter().map(|p| out_vec(p, 2)).map(|c| [c[0], c[1]]).collect(),
                    _ => vec![],
                })
            };
            let points = pts(js_string!("__pts"), ctx)?;
            let ins = pts(js_string!("__in"), ctx)?;
            let outs = pts(js_string!("__out"), ctx)?;
            let closed = o.get(js_string!("__closed"), ctx).map_err(err)?.to_boolean();
            return Ok(Out::Path { points, ins, outs, closed });
        }
    }
    Ok(Out::Object(v.to_string(ctx).map(|s| s.to_std_string_escaped()).unwrap_or_default()))
}

fn out_vec(o: &Out, n: usize) -> Vec<f64> {
    let mut v = match o {
        Out::Arr(items) => items.iter().map(|i| if let Out::Num(x) = i { *x } else { f64::NAN }).collect(),
        Out::Num(x) => vec![*x],
        _ => vec![],
    };
    v.resize(n.max(v.len()), 0.0);
    v
}

/// Make boa's messages read like AE's: `Error at line N: message`.
fn format_error(msg: &str, max_line: usize) -> String {
    let msg = msg.trim();
    let first = msg.lines().next().unwrap_or("").trim();
    // The wrapper's closing brace sits one line below the user's text.
    let line = find_line(msg).map(|l| l.min(max_line.max(1)));
    // Drop the position boa appends to the first line (` at line 2, col 5`, ` (unknown at :2:11)`).
    let mut text = first.split(" at line ").next().unwrap_or(first).trim();
    if let Some(i) = text.rfind(" (")
        && text.ends_with(')')
        && text[i..].contains(" at ")
    {
        text = text[..i].trim();
    }
    // `throw new Error("x")` reads better as just the message.
    let text = text.strip_prefix("Error: ").unwrap_or(text);
    match line {
        Some(l) => format!("Error at line {l}: {text}"),
        None => format!("Error: {text}"),
    }
}

fn line_count(text: &str) -> usize {
    text.lines().count()
}

/// Line number from `at line N` (parser errors), else the user script's (`<main>`) backtrace
/// entry, else the first `:line:col` position.
fn find_line(msg: &str) -> Option<usize> {
    if let Some(i) = msg.find("at line ") {
        return msg[i + 8..].split(|c: char| !c.is_ascii_digit()).next()?.parse().ok();
    }
    if let Some(l) = msg.lines().find(|l| l.trim_start().starts_with("at <main>")).and_then(position_line) {
        return Some(l);
    }
    position_line(msg)
}

/// First `:line:col` in `msg`.
fn position_line(msg: &str) -> Option<usize> {
    let b = msg.as_bytes();
    for i in 0..b.len() {
        if b[i] != b':' {
            continue;
        }
        let digits = |from: usize| b[from..].iter().take_while(|c| c.is_ascii_digit()).count();
        let n = digits(i + 1);
        if n > 0 && b.get(i + 1 + n) == Some(&b':') && digits(i + 2 + n) > 0 {
            return msg[i + 1..i + 1 + n].parse().ok();
        }
    }
    None
}

/// Run `text` once on this thread's runtime.
///
/// boa leaves values on its VM stack when an exception unwinds through call frames, and every
/// pending host request does that (the request throws out of the object-model functions), so a
/// long-lived context eventually fails every run with "maximum stack size". When a run hits that
/// limit the context is replaced with a fresh one and the run retried once: each run starts from
/// `__begin` with memoized answers, so nothing carries over. A script that really overflows the
/// stack fails again on the fresh context and reports the error.
pub fn run(text: &str, b: &Begin) -> Run {
    RUNTIME.with(|r| {
        let mut r = r.borrow_mut();
        for fresh in [false, true] {
            let out = match &mut **r.get_or_insert_with(|| ManuallyDrop::new(Runtime::new())) {
                Ok(rt) => rt.run(text, b),
                Err(e) => return Run::Done(Err(e.clone())),
            };
            match out {
                Run::Done(Err(e)) if e == TIMED_OUT => {
                    // Abandoned mid-run: start the next evaluation on a fresh context.
                    if let Some(old) = r.take() {
                        drop(ManuallyDrop::into_inner(old));
                    }
                    return Run::Done(Err(e));
                }
                Run::Done(Err(e)) if !fresh && e.contains(STACK_LIMIT) => {
                    // Dropped here, mid-thread, where boa's heap is alive (unlike at thread exit).
                    if let Some(old) = r.take() {
                        drop(ManuallyDrop::into_inner(old));
                    }
                }
                out => return out,
            }
        }
        Run::Done(Err(format!("Error: {STACK_LIMIT}")))
    })
}

/// Compile `text` (cached) without running it.
pub fn check(text: &str) -> Result<(), String> {
    RUNTIME.with(|r| {
        let mut r = r.borrow_mut();
        match &mut **r.get_or_insert_with(|| ManuallyDrop::new(Runtime::new())) {
            Ok(rt) => rt.script(text).map(|_| ()),
            Err(e) => Err(e.clone()),
        }
    })
}

/// Convert a script result to the property's value kind.
pub fn to_value(out: &Out, current: &Value) -> Result<Value, String> {
    let finite = |x: f64| if x.is_finite() { Ok(x) } else { Err("Error: expression result is not a finite number".to_string()) };
    let numeric = |o: &Out| -> Option<f64> {
        match o {
            Out::Num(x) => Some(*x),
            Out::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
            Out::Arr(a) => a.first().and_then(|x| if let Out::Num(x) = x { Some(*x) } else { None }),
            _ => None,
        }
    };
    let truthy = |o: &Out| match o {
        Out::Undefined | Out::Null => false,
        Out::Bool(b) => *b,
        Out::Num(x) => *x != 0.0 && !x.is_nan(),
        Out::Str(s) => !s.is_empty(),
        _ => true,
    };
    let text = |o: &Out| -> String {
        match o {
            Out::Str(s) => s.clone(),
            Out::Num(x) => js_number(*x),
            Out::Bool(b) => b.to_string(),
            Out::Arr(a) => a.iter().map(|x| if let Out::Num(n) = x { js_number(*n) } else { String::new() }).collect::<Vec<_>>().join(","),
            Out::Undefined | Out::Null => String::new(),
            Out::Path { .. } => "[object Path]".into(),
            Out::Style(j) => {
                serde_json::from_str::<serde_json::Value>(j).ok().map(|v| styled_doc(&v, &effectcraft_keyframe::TextDoc::default()).text).unwrap_or_default()
            }
            Out::Object(s) => s.clone(),
        }
    };
    if matches!(out, Out::Undefined) && !matches!(current, Value::Text(_) | Value::Str(_)) {
        return Err("Error: expression result is undefined".into());
    }
    Ok(match current {
        Value::Scalar(_) => Value::Scalar(finite(numeric(out).ok_or_else(|| "Error: expression result must be a number".to_string())?)?),
        Value::Vec2(_) | Value::Vec3(_) | Value::Color(_) => {
            let Out::Arr(items) = out else {
                return Err(format!("Error: expression result must be of dimension {}, not 1", current.dims()));
            };
            let mut c = current.components();
            for (i, x) in items.iter().enumerate().take(c.len()) {
                c[i] = finite(numeric(x).ok_or_else(|| "Error: expression result array must contain numbers".to_string())?)?;
            }
            current.with_components(&c)
        }
        Value::Bool(_) => Value::Bool(truthy(out)),
        Value::Enum(_) => Value::Enum((finite(numeric(out).unwrap_or(1.0))? - 1.0).round().max(0.0) as u32),
        Value::Text(doc) => match out {
            Out::Style(j) => {
                let v: serde_json::Value = serde_json::from_str(j).map_err(|e| format!("Error: bad text style: {e}"))?;
                Value::Text(Box::new(styled_doc(&v, doc)))
            }
            _ => {
                let mut d = doc.clone();
                d.set_text(&text(out));
                Value::Text(d)
            }
        },
        Value::Str(_) => Value::Str(text(out)),
        Value::Path(_) => match out {
            Out::Path { points, ins, outs, closed } => {
                // A vertex that isn't a number (`[[0, 0], [[1, 2], 0]]`) comes through as NaN, which
                // the rasterizer must never see.
                if points.iter().chain(ins).chain(outs).flatten().any(|c| !c.is_finite()) {
                    return Err("Error: path points must be finite numbers".into());
                }
                let n = points.len();
                let fit = |v: &[[f64; 2]]| {
                    let mut v = v.to_vec();
                    v.resize(n, [0.0; 2]);
                    v
                };
                Value::Path(effectcraft_keyframe::ShapePath {
                    vertices: points.clone(),
                    in_tangents: fit(ins),
                    out_tangents: fit(outs),
                    closed: *closed,
                    feather: Vec::new(),
                })
            }
            _ => return Err("Error: expression result must be a path".into()),
        },
        Value::Layer(_) | Value::Gradient(_) => current.clone(),
    })
}

/// Apply a returned text style (`{doc, ops}`) to the property's own document `own`: the style's
/// source document (another layer's, or this one's) gives the formatting, then the setter calls
/// apply in order. Without `setText` the text stays `own`'s.
pub fn styled_doc(v: &serde_json::Value, own: &effectcraft_keyframe::TextDoc) -> effectcraft_keyframe::TextDoc {
    use effectcraft_keyframe::TextDoc;
    let mut d = own.clone();
    if let Some(src) = v.get("doc").filter(|s| !s.is_null()).and_then(|s| serde_json::from_value::<TextDoc>(s.clone()).ok()) {
        if src.text == own.text {
            // The same text (this layer's own style, or an identical one): every run carries over.
            d = TextDoc { box_size: own.box_size, box_pos: own.box_pos, vertical: own.vertical, ..src };
        } else {
            let first = src.style_at(0);
            d.runs.clear();
            d.apply_style_all(|s| *s = first.clone());
            let p0 = src.para(0);
            d.set_paras(vec![p0; d.para_count()]);
            d.stroke_over_fill = src.stroke_over_fill;
        }
    }
    for op in v.get("ops").and_then(|o| o.as_array()).into_iter().flatten() {
        let Some(key) = op.get(0).and_then(|k| k.as_str()) else { continue };
        let val = op.get(1).cloned().unwrap_or_default();
        let start = op.get(2).and_then(|x| x.as_i64()).unwrap_or(-1);
        let count = op.get(3).and_then(|x| x.as_i64()).unwrap_or(-1);
        let n = d.char_len();
        let range = (start >= 0).then(|| {
            let a = (start as usize).min(n);
            a..if count < 0 { n } else { (a + count as usize).min(n) }
        });
        if key == "text" {
            let s = val.as_str().unwrap_or_default();
            match range {
                Some(r) => d.replace_range(r, s, None),
                None => d.set_text(s),
            }
            continue;
        }
        let _ = d.set_attr(key, &val, range);
    }
    d
}

/// Number → string like JS (`3`, not `3.0`).
fn js_number(x: f64) -> String {
    if x.is_finite() && x == x.trunc() && x.abs() < 1e21 { format!("{}", x as i64) } else { format!("{x}") }
}
