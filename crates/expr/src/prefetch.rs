//! What an expression read last time, to answer before its next run.
//!
//! Every host request a script makes before its answer is known costs a run (see
//! [`crate::host`]). Expressions read nearly the same things on every frame, so each evaluation
//! records the requests its final run made and the next evaluation of the same expression on
//! the same property answers them up front: from the second frame on, most expressions run
//! once. Requests at the evaluation's own time move to the new time; requests at other times
//! (`valueAtTime(time - d)`, `smooth`) can't be predicted exactly and are left to the run. A
//! prediction is only a hint: answering a request the script doesn't ask for costs time but
//! never changes a result, because answers depend only on the request.
//!
//! The logs are per thread and bounded. The property's own path (`@uid/@uid/…`, which the
//! object model and the requests use) is memoised per thread too, checked against the layer on
//! every use.

use std::cell::RefCell;
use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};

use effectcraft_project::{Layer, PropGroup, Property};

use crate::host::{Req, Secs};

/// Expressions remembered per thread (the map is cleared when full).
const MAX_LOGS: usize = 4096;
/// Longest request list worth remembering.
const MAX_LOG_LEN: usize = 4096;
/// Own paths remembered per thread.
const MAX_PATHS: usize = 16384;

/// An expression on a property: text (hashed; a collision only costs a wrong prediction), comp,
/// layer and property uid.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Key {
    text: u64,
    comp: u64,
    layer: u64,
    uid: u64,
}

impl Key {
    pub fn new(text: &str, comp: u64, layer: u64, uid: u64) -> Key {
        let mut h = DefaultHasher::new();
        text.hash(&mut h);
        Key { text: h.finish(), comp, layer, uid }
    }
}

struct Log {
    /// The evaluation's own comp time.
    t: Secs,
    reqs: Vec<Req>,
}

thread_local! {
    static LOGS: RefCell<HashMap<Key, Log>> = RefCell::new(HashMap::new());
    static PATHS: RefCell<HashMap<(u64, u64), (Vec<u64>, String)>> = RefCell::new(HashMap::new());
}

/// The requests to answer before evaluating `key` at comp time `now`.
pub fn predicted(key: &Key, now: Secs) -> Vec<Req> {
    LOGS.with(|l| {
        let l = l.borrow();
        let Some(log) = l.get(key) else { return vec![] };
        log.reqs
            .iter()
            .filter_map(|r| match r.time() {
                None => Some(r.clone()),
                Some(t) if t == log.t => Some(r.at(now)),
                Some(_) if log.t == now => Some(r.clone()),
                Some(_) => None,
            })
            .collect()
    })
}

/// Remember the requests the final run of `key` at comp time `t` made.
pub fn store(key: Key, t: Secs, reqs: Vec<Req>) {
    if reqs.len() > MAX_LOG_LEN {
        return;
    }
    LOGS.with(|l| {
        let mut l = l.borrow_mut();
        if l.len() >= MAX_LOGS && !l.contains_key(&key) {
            l.clear();
        }
        l.insert(key, Log { t, reqs });
    });
}

/// Whether the `@uid` chain leads from `g` to a property with the chain's last uid.
fn chain_ok(g: &PropGroup, chain: &[u64]) -> bool {
    let Some((last, init)) = chain.split_last() else { return false };
    let mut g = g;
    for u in init {
        match g.children.iter().find(|c| c.uid() == *u).and_then(|c| c.as_group()) {
            Some(next) => g = next,
            None => return false,
        }
    }
    g.children.iter().any(|c| c.uid() == *last && c.as_prop().is_some())
}

/// `layer.props.path_of(prop.uid)` (or `@uid` when the property isn't in the layer's tree),
/// memoised: the remembered path is used only while it still leads to the property.
pub fn own_path(layer: &Layer, prop: &Property) -> String {
    let key = (layer.id.0, prop.uid);
    let hit =
        PATHS.with(|m| m.borrow().get(&key).filter(|(chain, _)| chain.last() == Some(&prop.uid) && chain_ok(&layer.props, chain)).map(|(_, p)| p.clone()));
    if let Some(p) = hit {
        return p;
    }
    let Some(path) = layer.props.path_of(prop.uid) else { return format!("@{}", prop.uid) };
    let chain: Vec<u64> = path.split('/').filter_map(|s| s.strip_prefix('@')?.parse().ok()).collect();
    PATHS.with(|m| {
        let mut m = m.borrow_mut();
        if m.len() >= MAX_PATHS {
            m.clear();
        }
        m.insert(key, (chain, path.clone()));
    });
    path
}
