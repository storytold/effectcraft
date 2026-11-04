//! The `startup` key of `./effectcraft-config.json`: which project the page opens on load.
//!
//! | Value | On load |
//! |---|---|
//! | `"last"` (and absent) | the last visit's project; the demo project on a first visit |
//! | `"empty"` | the last visit's project; a blank project on a first visit, never the demo |
//! | `"demo"` | the demo project, whatever the last visit left |
//!
//! The `?empty` and `?demo` URL flags still win. Anything else (a missing key, another type, an
//! unknown value, a config that is not JSON) is the default, `"last"`.

use serde_json::Value;

/// What the page opens on load.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Startup {
    #[default]
    Last,
    Empty,
    Demo,
}

/// The largest config file read (bytes).
pub const CONFIG_MAX: usize = 64 * 1024;

/// The `startup` key of a config file's bytes.
pub fn parse(bytes: &[u8]) -> Startup {
    if bytes.len() > CONFIG_MAX {
        return Startup::default();
    }
    let Ok(v) = serde_json::from_slice::<Value>(bytes) else { return Startup::default() };
    match v.get("startup").and_then(Value::as_str) {
        Some("empty") => Startup::Empty,
        Some("demo") => Startup::Demo,
        _ => Startup::Last,
    }
}

impl Startup {
    /// Whether the last visit's project is restored (off for the demo).
    pub fn restores(self) -> bool {
        self != Startup::Demo
    }

    /// Whether the session snapshot keeps being saved (off for the demo, so it never replaces the
    /// project the last visit left).
    pub fn keeps_session(self) -> bool {
        self != Startup::Demo
    }

    /// Whether the demo project opens when nothing was restored (off for a blank start).
    pub fn demo_when_nothing_restored(self) -> bool {
        self != Startup::Empty
    }
}
