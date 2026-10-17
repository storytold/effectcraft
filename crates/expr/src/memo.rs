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
//! The memo is per thread (frames render in parallel without sharing it), bounded, and clears
//! recorded results when full while reusing bounded table storage.

use std::cell::RefCell;
use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};

use crate::host::{Req, Resp, Secs};
use crate::runtime::{Begin, Out};

/// Results remembered per thread.
const MAX_ENTRIES: usize = 65_536;
/// Estimated retained entry data and table storage per thread (allocator RSS can differ).
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
    /// High-water table capacity: removing entries can reduce capacity() without freeing buckets.
    table_capacity: usize,
}

// Include allocation overhead and spare capacity, not just visible string/list lengths.
fn allocation(bytes: usize) -> usize {
    if bytes == 0 { 0 } else { bytes.saturating_add(16) }
}

fn vector_heap<T>(capacity: usize) -> usize {
    allocation(capacity.saturating_mul(std::mem::size_of::<T>()))
}

fn response_heap(r: &Resp) -> usize {
    match r {
        Resp::Str(s) => allocation(s.capacity()),
        Resp::List(items) => items.iter().map(response_heap).fold(vector_heap::<Resp>(items.capacity()), usize::saturating_add),
        _ => 0,
    }
}

fn request_heap(r: &Req) -> usize {
    let key_heap = |key: &crate::host::Key| match key {
        crate::host::Key::Name(s) => allocation(s.capacity()),
        crate::host::Key::Index(_) => 0,
    };
    match r {
        Req::CompByName { name } | Req::Footage { name } | Req::FootageData { name } => allocation(name.capacity()),
        Req::LayerLookup { key, .. } => key_heap(key),
        Req::Child { group, key, .. } => allocation(group.capacity()).saturating_add(key_heap(key)),
        Req::PropInfo { path, .. } | Req::Keys { path, .. } | Req::Value { path, .. } | Req::Doc { path, .. } => allocation(path.capacity()),
        _ => 0,
    }
}

fn output_heap(out: &Out) -> usize {
    match out {
        Out::Str(s) | Out::Style(s) | Out::Object(s) => allocation(s.capacity()),
        Out::Arr(items) => items.iter().map(output_heap).fold(vector_heap::<Out>(items.capacity()), usize::saturating_add),
        Out::Path { points, ins, outs, .. } => {
            [points.capacity(), ins.capacity(), outs.capacity()].into_iter().map(vector_heap::<[f64; 2]>).fold(0, usize::saturating_add)
        }
        _ => 0,
    }
}

fn entry_heap(text: &str, b: &Begin, reqs: &Vec<Req>, resps: &Vec<Resp>, out: &Out) -> usize {
    [
        allocation(text.len()),
        allocation(b.path.len()),
        response_heap(b.value),
        vector_heap::<Req>(reqs.capacity()),
        vector_heap::<Resp>(resps.capacity()),
        output_heap(out),
    ]
    .into_iter()
    .chain(reqs.iter().map(request_heap))
    .chain(resps.iter().map(response_heap))
    .fold(0, usize::saturating_add)
}

/// Leave 50% headroom for spare buckets, control bytes and allocation overhead.
fn table_bytes(capacity: usize) -> usize {
    allocation(capacity.saturating_mul(std::mem::size_of::<(Key, Entry)>()).saturating_mul(3) / 2)
}

thread_local! {
    static MEMO: RefCell<Memo> = RefCell::new(Memo::default());
    #[cfg(test)]
    static RESETS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    #[cfg(test)]
    static GROWTHS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
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
    let bytes = entry_heap(text, b, &reqs, &resps, &out);
    if bytes.saturating_add(std::mem::size_of::<(Key, Entry)>()) > MAX_ENTRY_BYTES {
        return;
    }
    MEMO.with(|m| {
        let mut m = m.borrow_mut();
        if let Some(old) = m.entries.remove(&key) {
            m.bytes = m.bytes.saturating_sub(old.bytes);
        }
        m.table_capacity = m.table_capacity.max(m.entries.capacity());
        // Current capacity guarantees insertion room; high-water capacity accounts for buckets.
        let next_capacity = if m.entries.len() >= m.entries.capacity() { m.table_capacity.max(4).saturating_mul(2) } else { m.table_capacity };
        if m.entries.len() >= MAX_ENTRIES || table_bytes(next_capacity).saturating_add(m.bytes).saturating_add(bytes) > MAX_BYTES {
            #[cfg(test)]
            RESETS.with(|n| n.set(n.get() + 1));
            m.entries.clear();
            m.bytes = 0;
            if table_bytes(m.table_capacity).saturating_add(bytes) > MAX_BYTES {
                *m = Memo::default();
            }
        }
        m.bytes = m.bytes.saturating_add(bytes);
        #[cfg(test)]
        let before_capacity = m.table_capacity;
        m.entries.insert(key, Entry { text: text.into(), path: b.path.into(), value: b.value.clone(), reqs, resps, out, bytes });
        m.table_capacity = m.table_capacity.max(m.entries.capacity());
        #[cfg(test)]
        if m.table_capacity > before_capacity {
            GROWTHS.with(|n| n.set(n.get() + 1));
        }
        if table_bytes(m.table_capacity).saturating_add(m.bytes) > MAX_BYTES {
            #[cfg(test)]
            RESETS.with(|n| n.set(n.get() + 1));
            *m = Memo::default();
        }
    });
}

#[cfg(test)]
pub(crate) fn footprint() -> (usize, usize, usize, usize, usize) {
    MEMO.with(|m| {
        let m = m.borrow();
        (m.entries.len(), m.bytes + table_bytes(m.table_capacity), m.table_capacity, RESETS.with(std::cell::Cell::get), GROWTHS.with(std::cell::Cell::get))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn begin(value: &Resp, time: f64) -> Begin<'_> {
        Begin {
            time,
            value,
            comp: 1,
            layer: 2,
            path: "transform/rotation",
            uid: 3,
            index: 0.0,
            frame_duration: 1.0 / 30.0,
            text_sel: [1.0; 2],
            selector: [100.0; 3],
        }
    }

    #[test]
    fn graph_sized_small_results_fit_without_discarding_earlier_samples() {
        MEMO.with(|m| *m.borrow_mut() = Memo::default());
        let value = Resp::Num(0.0);
        let text = "Math.sin(time * 3) * 90";
        for i in 0..28_800 {
            let b = begin(&value, i as f64 / 1440.0);
            store(Key::new(text, &b), text, &b, vec![], vec![], Out::Num(i as f64));
        }
        MEMO.with(|m| {
            let m = m.borrow();
            assert_eq!(m.entries.len(), 28_800);
            assert!(m.bytes + table_bytes(m.table_capacity) <= MAX_BYTES);
        });
        for i in [0, 8192, 28_799] {
            let b = begin(&value, i as f64 / 1440.0);
            let key = Key::new(text, &b);
            assert_eq!(recorded(&key, text, &b), Some(vec![]));
            assert_eq!(check(&key, |_, _| true), Some(Out::Num(i as f64)));
        }
    }

    #[test]
    fn oversized_results_and_request_names_are_not_retained() {
        MEMO.with(|m| *m.borrow_mut() = Memo::default());
        let value = Resp::Num(0.0);
        let b = begin(&value, 0.0);
        let key = Key::new("value", &b);
        for out in [
            Out::Str("x".repeat(MAX_ENTRY_BYTES)),
            Out::Arr(vec![Out::Str("x".repeat(MAX_ENTRY_BYTES))]),
            Out::Path { points: vec![[0.0; 2]; MAX_ENTRY_BYTES / 32], ins: vec![[0.0; 2]; MAX_ENTRY_BYTES / 32], outs: vec![], closed: false },
        ] {
            store(key, "value", &b, vec![], vec![], out);
            assert!(recorded(&key, "value", &b).is_none());
        }
        let req = Req::Child { comp: 1, layer: 2, group: "x".repeat(MAX_ENTRY_BYTES / 2), key: crate::host::Key::Name("x".repeat(MAX_ENTRY_BYTES / 2)) };
        store(key, "value", &b, vec![req], vec![Resp::Num(1.0)], Out::Num(1.0));
        assert!(recorded(&key, "value", &b).is_none());
    }

    #[test]
    fn memory_budget_clear_reuses_reserved_table_without_reallocation() {
        MEMO.with(|m| *m.borrow_mut() = Memo::default());
        let value = Resp::Num(0.0);
        let mut previous_count = 0;
        let mut previous_capacity = 0;
        let clears = RESETS.with(std::cell::Cell::get);
        for i in 0..2000 {
            let b = begin(&value, i as f64);
            let key = Key::new("value", &b);
            store(key, "value", &b, vec![], vec![], Out::Str("x".repeat(64 * 1024)));
            MEMO.with(|m| {
                let m = m.borrow();
                assert!(m.bytes + table_bytes(m.table_capacity) <= MAX_BYTES);
                assert!(m.entries.contains_key(&key));
                if m.entries.len() < previous_count {
                    assert_eq!(m.table_capacity, previous_capacity);
                }
                previous_count = m.entries.len();
                previous_capacity = m.table_capacity;
            });
        }
        assert!(RESETS.with(std::cell::Cell::get) > clears);
        assert!(recorded(&Key::new("value", &begin(&value, 0.0)), "value", &begin(&value, 0.0)).is_none());
    }

    #[test]
    fn replacing_results_updates_retained_bytes_without_accumulating_old_payloads() {
        MEMO.with(|m| *m.borrow_mut() = Memo::default());
        let value = Resp::Num(0.0);
        let b = begin(&value, 0.0);
        let key = Key::new("value", &b);
        for out in [Out::Str("a".repeat(100_000)), Out::Str("b".repeat(200_000)), Out::Num(3.0)] {
            let expected = entry_heap("value", &b, &vec![], &vec![], &out);
            store(key, "value", &b, vec![], vec![], out.clone());
            MEMO.with(|m| {
                let m = m.borrow();
                assert_eq!(m.entries.len(), 1);
                assert_eq!(m.bytes, expected);
            });
            assert_eq!(check(&key, |_, _| true), Some(out));
        }
    }
}
