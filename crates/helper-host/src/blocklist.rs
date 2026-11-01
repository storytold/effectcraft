//! Effects that keep taking their helper down are turned off.
//!
//! Each time a render through a helper ends with the helper crashing or hanging, the effect is
//! charged; at [`CRASH_LIMIT`] it is blocked: its renders fail at once with a message instead of
//! restarting the helper again. Blocks are kept in `helper_blocklist.txt` in the state folder
//! ([`set_state_dir`]), one effect id per line, so they last across launches until a scan with
//! `retry` ([`clear`]).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use crate::client::lock;

/// Crashes after which an effect is blocked.
pub const CRASH_LIMIT: u32 = 3;

const FILE: &str = "helper_blocklist.txt";

#[derive(Default)]
struct State {
    dir: Option<PathBuf>,
    crashes: HashMap<String, u32>,
    blocked: Vec<String>,
    loaded: bool,
}

fn state() -> &'static Mutex<State> {
    static S: OnceLock<Mutex<State>> = OnceLock::new();
    S.get_or_init(Default::default)
}

/// Where the block list lives (EffectCraft's settings folder); helpers get it as
/// `EFFECTCRAFT_HELPER_STATE_DIR` for their own records.
pub fn set_state_dir(dir: PathBuf) {
    let mut s = lock(state());
    s.dir = Some(dir);
    s.loaded = false;
}

pub fn state_dir() -> Option<PathBuf> {
    lock(state()).dir.clone()
}

fn load(s: &mut State) {
    if s.loaded {
        return;
    }
    s.loaded = true;
    if let Some(text) = s.dir.as_ref().and_then(|d| std::fs::read_to_string(d.join(FILE)).ok()) {
        for id in text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')) {
            if !s.blocked.iter().any(|b| b == id) {
                s.blocked.push(id.to_string());
            }
        }
    }
}

fn save(s: &State) {
    let Some(dir) = &s.dir else { return };
    let mut text =
        String::from("# Effects turned off because they kept stopping their effect helper.\n# Delete a line (or scan with retry) to try one again.\n");
    for id in &s.blocked {
        text.push_str(id);
        text.push('\n');
    }
    if let Err(e) = std::fs::create_dir_all(dir).and_then(|_| std::fs::write(dir.join(FILE), text)) {
        log::warn!("can't save the helper block list: {e}");
    }
}

pub fn is_blocked(id: &str) -> bool {
    let mut s = lock(state());
    load(&mut s);
    s.blocked.iter().any(|b| b == id)
}

/// Charge `id` with a helper crash; returns whether it is now blocked.
pub fn note_crash(id: &str) -> bool {
    let mut s = lock(state());
    load(&mut s);
    let n = {
        let n = s.crashes.entry(id.to_string()).or_insert(0);
        *n = n.saturating_add(1);
        *n
    };
    if n >= CRASH_LIMIT && !s.blocked.iter().any(|b| b == id) {
        s.blocked.push(id.to_string());
        save(&s);
    }
    n >= CRASH_LIMIT
}

/// The blocked effect ids.
pub fn blocked() -> Vec<String> {
    let mut s = lock(state());
    load(&mut s);
    s.blocked.clone()
}

/// Forget every block and crash count.
pub fn clear() {
    let mut s = lock(state());
    load(&mut s);
    s.blocked.clear();
    s.crashes.clear();
    save(&s);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effects_are_blocked_after_repeated_crashes() {
        let id = "org.test.blocklist-unit";
        for _ in 1..CRASH_LIMIT {
            assert!(!note_crash(id));
        }
        assert!(note_crash(id));
        assert!(is_blocked(id) && blocked().iter().any(|b| b == id));
        clear();
        assert!(!is_blocked(id));
    }
}
