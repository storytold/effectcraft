//! Expression results shared between evaluations.
//!
//! A controller slider read by 100 layers used to have its own expression run 100 times per
//! frame (and per motion-blur sample): each evaluation memoised only its own host answers. This
//! memo keeps, per thread, the result of each successful evaluation together with every host
//! answer its final run read. An evaluation with the same inputs (script, property, comp time,
//! keyframed value, `__begin` arguments) first answers those recorded requests and, when every
//! answer is bit-for-bit the one recorded, returns the recorded result without running the
//! script. Expressions are pure functions of their inputs and host answers (see
//! [`crate::host`]): given the same answers a run makes the same requests and returns the same
//! result, so a hit is exact and no edit can make it stale. The answers are resolved at the
//! evaluation's own nesting depth, as a run would, so reads cut short by
//! [`crate::MAX_DEPTH`] are re-checked too.
//!
//! What a hit saves is the script run: resolving the answers is work the prefetch does anyway.
//! Scripts that read something besides their answers (`Math.random`, `Date`) are never stored.
//! The memo is per thread (frames render in parallel without sharing it), bounded, and cleared
//! when full.

use std::cell::RefCell;
use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};

use crate::host::{Req, Resp, Secs};
use crate::runtime::{Begin, Out};

/// Results remembered per thread.
const MAX_ENTRIES: usize = 8192;
/// Approximate bytes of recorded answers per thread (all entries together).
const MAX_BYTES: usize = 32 << 20;
/// Most answers, and approximate bytes of answers, worth recording for one result.
const MAX_DEPS: usize = 4096;
const MAX_ENTRY_BYTES: usize = 1 << 20;

/// The inputs of one evaluation that hash cheaply (the script text is hashed; the entry keeps
/// the full text and the rest of [`Begin`] to compare).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Key {
    text: u64,
    comp: u64,
    layer: u64,
    uid: u64,
    time: Secs,
    index: Secs,
    frame_duration: Secs,
    text_sel: [Secs; 2],
    selector: [Secs; 3],
}

impl Key {
    pub fn new(text: &str, b: &Begin) -> Key {
        let mut h = DefaultHasher::new();
        text.hash(&mut h);
        let bits = f64::to_bits;
        Key {
            text: h.finish(),
            comp: b.comp,
            layer: b.layer,
            uid: b.uid,
            time: bits(b.time),
            index: bits(b.index),
            frame_duration: bits(b.frame_duration),
            text_sel: b.text_sel.map(bits),
            selector: b.selector.map(bits),
        }
    }
}

struct Entry {
    text: Box<str>,
    path: Box<str>,
    value: Resp,
    /// What the final run read, in order, with the answers it got.
    reqs: Vec<Req>,
    resps: Vec<Resp>,
    out: Out,
    bytes: usize,
}

#[derive(Default)]
struct Memo {
    entries: HashMap<Key, Entry>,
    bytes: usize,
}

/// Approximate heap size of an answer.
fn weight(r: &Resp) -> usize {
    16 + match r {
        Resp::Str(s) => s.len(),
        Resp::List(items) => items.iter().map(weight).fold(0, usize::saturating_add),
        _ => 0,
    }
}

thread_local! {
    static MEMO: RefCell<Memo> = RefCell::new(Memo::default());
    /// Tests turn the memo off to compute the oracle.
    #[cfg(test)]
    static OFF: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

#[cfg(test)]
fn off() -> bool {
    OFF.with(std::cell::Cell::get)
}
#[cfg(not(test))]
fn off() -> bool {
    false
}

/// Turn the memo off (or back on) on this thread.
#[cfg(test)]
pub fn set_off(v: bool) {
    OFF.with(|o| o.set(v));
}

/// Whether a script's result may be remembered: it reads nothing but its inputs and answers. A
/// cheap first check by text; a run that reaches `Math.random` or `Date` any other way is caught
/// as it calls them (`Frame::impure`).
pub fn cacheable(text: &str) -> bool {
    !off() && !text.contains("Math.random") && !text.contains("Date")
}

/// Bit-for-bit equality (`-0` ≠ `0`, and `NaN` equals itself), so a hit never changes a result
/// a script could tell apart.
pub fn same(a: &Resp, b: &Resp) -> bool {
    match (a, b) {
        (Resp::Null, Resp::Null) => true,
        (Resp::Bool(x), Resp::Bool(y)) => x == y,
        (Resp::Num(x), Resp::Num(y)) => x.to_bits() == y.to_bits(),
        (Resp::Str(x), Resp::Str(y)) => x == y,
        (Resp::List(x), Resp::List(y)) => x.len() == y.len() && x.iter().zip(y).all(|(a, b)| same(a, b)),
        _ => false,
    }
}

/// The requests a remembered evaluation with these inputs read, to answer before [`check`].
pub fn recorded(key: &Key, text: &str, b: &Begin) -> Option<Vec<Req>> {
    if off() {
        return None;
    }
    MEMO.with(|m| {
        let m = m.borrow();
        let e = m.entries.get(key)?;
        (*e.text == *text && *e.path == *b.path && same(&e.value, b.value)).then(|| e.reqs.clone())
    })
}

/// The remembered result when `answered(req, resp)` holds for every recorded request and its
/// recorded answer (it must not evaluate anything: answer the requests first).
pub fn check(key: &Key, answered: impl Fn(&Req, &Resp) -> bool) -> Option<Out> {
    MEMO.with(|m| {
        let m = m.borrow();
        let e = m.entries.get(key)?;
        e.reqs.iter().zip(&e.resps).all(|(q, r)| answered(q, r)).then(|| e.out.clone())
    })
}

/// Remember a successful evaluation and what its final run read.
pub fn store(key: Key, text: &str, b: &Begin, reqs: Vec<Req>, resps: Vec<Resp>, out: Out) {
    if reqs.len() != resps.len() || reqs.len() > MAX_DEPS || !cacheable(text) {
        return;
    }
    let bytes = resps.iter().map(weight).fold(weight(b.value), usize::saturating_add);
    if bytes > MAX_ENTRY_BYTES {
        return;
    }
    MEMO.with(|m| {
        let mut m = m.borrow_mut();
        if let Some(old) = m.entries.remove(&key) {
            m.bytes = m.bytes.saturating_sub(old.bytes);
        }
        if m.entries.len() >= MAX_ENTRIES || m.bytes.saturating_add(bytes) > MAX_BYTES {
            m.entries.clear();
            m.bytes = 0;
        }
        m.bytes = m.bytes.saturating_add(bytes);
        m.entries.insert(key, Entry { text: text.into(), path: b.path.into(), value: b.value.clone(), reqs, resps, out, bytes });
    });
}
