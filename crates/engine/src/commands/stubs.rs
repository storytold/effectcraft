//! Menu entries whose implementation is owned by another milestone (3D, render queue/export,
//! time remapping, pen/mask-vertex editing, motion tracking, layer styles…) or that have no
//! EffectCraft equivalent yet. They are registered so the menu bar matches After Effects, and are
//! always disabled. When a feature lands, delete its line here and register the real command
//! with the same id (the menu tree in `menus.rs` already points at it).

#[allow(unused_imports)]
use super::{CommandSpec, not_yet, not_yet_run};

#[allow(unused_macros)]
macro_rules! stub {
    ($id:literal, $label:literal, [$($m:literal),*], $params:literal) => {
        CommandSpec { id: $id, label: $label, menu: &[$($m),*], shortcut: None, alt_shortcuts: &[], params: $params, enabled: not_yet, run: not_yet_run, journal: true }
    };
    ($id:literal, $label:literal, [$($m:literal),*], $sc:literal, $params:literal) => {
        CommandSpec { id: $id, label: $label, menu: &[$($m),*], shortcut: Some($sc), alt_shortcuts: &[], params: $params, enabled: not_yet, run: not_yet_run, journal: true }
    };
}

pub fn specs() -> Vec<CommandSpec> {
    // Every After Effects menu entry has a real command now.
    vec![]
}
