//! # effectcraft-helper-host (L3)
//!
//! **External effect helpers**: separate programs that offer effects to EffectCraft over a small,
//! versioned protocol ([`protocol`], `docs/external-helpers.md`, ADR
//! `docs/adr/0001-external-effect-helpers.md`). EffectCraft finds installed helpers
//! ([`find_helpers`]), starts each as a child process, lists its effects and registers every one
//! as an ordinary plug-in effect (plug-in API 2), so the UI, the property tree, keyframes,
//! scripting and the command surface need nothing new. Rendering sends the frame to the helper
//! through a shared exchange file ([`frames`]) and answers its questions (the input at another
//! time, another layer, parameters at another time) while it works.
//!
//! The point is containment: whatever native code a helper runs (an OpenFX plug-in host, for
//! example) runs in **its** process. If it crashes, hangs or talks nonsense, EffectCraft stops
//! it, shows the error on the effect, leaves the frame as it was and starts the helper again
//! for the next render; an effect that keeps taking its helper down is turned off
//! ([`blocklist`]). This crate is safe Rust (`unsafe_code = "forbid"`, like the rest of the
//! workspace), loads no native code and opens no network connection.
//!
//! [`serve`] is the helper side, for helper programs written in Rust.
//!
//! Native only: on wasm32 the crate is empty (there are no processes to start).

#![cfg(not(target_arch = "wasm32"))]

pub mod blocklist;
pub mod client;
pub mod frames;
pub mod protocol;
pub mod serve;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use effectcraft_effects::plugin::{plugin, register_plugin};
use serde_json::{Value, json};

use crate::client::{Helper, HelperEffect, lock};
use crate::protocol::RemoteEffect;

/// Helper program names start with this (`effectcraft-helper-ofx`, plus `.exe` on Windows).
pub const PROGRAM_PREFIX: &str = "effectcraft-helper-";

/// The running helpers.
fn helpers() -> &'static Mutex<Vec<Arc<Helper>>> {
    static H: OnceLock<Mutex<Vec<Arc<Helper>>>> = OnceLock::new();
    H.get_or_init(Default::default)
}

/// Where helpers are looked for: `helpers/` next to the executable, `helpers/` in the settings
/// folder (`config_dir`), and every entry of `EFFECTCRAFT_HELPER_PATH` (a path list of helper
/// programs or folders).
pub fn default_search_paths(config_dir: Option<&Path>) -> Vec<PathBuf> {
    let mut v = vec![];
    if let Some(dir) = std::env::current_exe().ok().and_then(|e| e.parent().map(Path::to_path_buf)) {
        v.push(dir.join("helpers"));
        // macOS app bundles: Contents/MacOS/effectcraft → Contents/Helpers.
        if cfg!(target_os = "macos")
            && let Some(contents) = dir.parent()
        {
            v.push(contents.join("Helpers"));
        }
    }
    if let Some(c) = config_dir {
        v.push(c.join("helpers"));
    }
    if let Some(p) = std::env::var_os("EFFECTCRAFT_HELPER_PATH") {
        v.extend(std::env::split_paths(&p).filter(|p| !p.as_os_str().is_empty()));
    }
    v
}

/// Whether a file name is a helper program's.
pub fn is_helper_name(name: &str) -> bool {
    let stem = if cfg!(windows) {
        match name.len().checked_sub(4).and_then(|i| name.get(i..).map(|ext| (i, ext))) {
            Some((i, ext)) if ext.eq_ignore_ascii_case(".exe") => name.get(..i).unwrap_or(""),
            _ => return false,
        }
    } else {
        name
    };
    stem.len() > PROGRAM_PREFIX.len() && stem.starts_with(PROGRAM_PREFIX) && !stem.contains('.')
}

/// The helper programs in `paths`: files named `effectcraft-helper-*` in each folder and its
/// direct subfolders, and paths that are themselves such files (any name is accepted for an
/// explicit file). Sorted, without duplicates.
pub fn find_helpers(paths: &[PathBuf]) -> Vec<PathBuf> {
    fn named(p: &Path) -> bool {
        p.is_file() && p.file_name().and_then(|n| n.to_str()).is_some_and(is_helper_name)
    }
    let mut out = vec![];
    for p in paths {
        if p.is_file() {
            out.push(p.clone());
            continue;
        }
        let Ok(rd) = std::fs::read_dir(p) else { continue };
        for e in rd.flatten() {
            let path = e.path();
            if named(&path) {
                out.push(path);
            } else if path.is_dir()
                && let Ok(sub) = std::fs::read_dir(&path)
            {
                out.extend(sub.flatten().map(|e| e.path()).filter(|p| named(p)));
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

/// Register a helper's effects. Effects already registered (an earlier scan) are kept as they
/// are; blocked effects are reported, not registered.
fn register(helper: &Arc<Helper>, effects: Vec<RemoteEffect>, errors: &mut Vec<String>) -> Vec<Value> {
    let mut loaded = vec![];
    let mut added = 0;
    for e in effects {
        let id = e.manifest.id.clone();
        if blocklist::is_blocked(&id) {
            errors.push(format!("{id}: turned off because it kept stopping its helper (scan with retry to try again)"));
            continue;
        }
        if plugin(&id).is_some() {
            continue;
        }
        let (name, category) = (e.manifest.name.clone(), e.manifest.category.clone());
        match register_plugin(Arc::new(HelperEffect::new(helper.clone(), e))) {
            Ok(_) => {
                added += 1;
                loaded.push(json!({"id": id, "name": name, "category": category, "helper": helper.program().to_string_lossy()}));
            }
            Err(err) => errors.push(format!("{id}: {err}")),
        }
    }
    helper.add_effects(added);
    loaded
}

/// Start every helper found in `paths` that isn't running yet and register its effects.
/// `retry` forgets the block list first. Returns `{paths, helpers, loaded, errors}`.
pub fn scan_paths(paths: &[PathBuf], retry: bool) -> Value {
    if retry {
        blocklist::clear();
    }
    let (mut started, mut loaded, mut errors) = (vec![], vec![], vec![]);
    for program in find_helpers(paths) {
        if lock(helpers()).iter().any(|h| h.program() == program) {
            continue;
        }
        match Helper::start(&program) {
            Ok((h, effects, errs)) => {
                errors.extend(errs.into_iter().map(|e| format!("{}: {e}", program.display())));
                loaded.extend(register(&h, effects, &mut errors));
                started.push(json!(program.to_string_lossy()));
                lock(helpers()).push(h);
            }
            Err(e) => errors.push(e),
        }
    }
    json!({
        "paths": paths.iter().map(|p| p.to_string_lossy()).collect::<Vec<_>>(),
        "helpers": started,
        "loaded": loaded,
        "errors": errors,
    })
}

/// Load a file or folder through the helpers that accept its extension (a folder goes to every
/// helper that loads anything). Returns `{loaded, errors}`.
pub fn load_path(path: &str) -> Result<Value, String> {
    let p = Path::new(path);
    let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("").to_string();
    // `.ofx.bundle` folders and the like: match on any dotted part of the name.
    let parts: Vec<String> = p.file_name().and_then(|n| n.to_str()).map(|n| n.split('.').skip(1).map(str::to_string).collect()).unwrap_or_default();
    let list: Vec<Arc<Helper>> = lock(helpers()).clone();
    let takers: Vec<Arc<Helper>> = list
        .into_iter()
        .filter(|h| {
            let s = h.status();
            if p.is_dir() && parts.is_empty() { !s.loads.is_empty() } else { h.loads(&ext) || parts.iter().any(|x| h.loads(x)) }
        })
        .collect();
    if takers.is_empty() {
        return Err(format!("no effect helper loads `{path}` (installed helpers: Effect ▸ Scan for Effect Helpers)"));
    }
    let (mut loaded, mut errors) = (vec![], vec![]);
    for h in takers {
        match h.load(path) {
            Ok((effects, errs)) => {
                errors.extend(errs);
                loaded.extend(register(&h, effects, &mut errors));
            }
            Err(e) => errors.push(e),
        }
    }
    if loaded.is_empty() && !errors.is_empty() {
        return Err(errors.join("; "));
    }
    Ok(json!({"loaded": loaded, "errors": errors}))
}

/// The helpers and their state: `{helpers: [{program, name, version, loads, running, effects,
/// restarts, error}], blocked}`.
pub fn list() -> Value {
    let helpers: Vec<Value> = lock(helpers())
        .iter()
        .map(|h| {
            let s = h.status();
            json!({
                "program": s.program.to_string_lossy(),
                "name": s.name,
                "version": s.version,
                "loads": s.loads,
                "running": s.running,
                "effects": s.effects,
                "restarts": s.restarts,
                "error": s.last_error,
            })
        })
        .collect();
    json!({"helpers": helpers, "blocked": blocklist::blocked()})
}

/// Stop every helper process (they start again when needed). Called when the app quits;
/// helpers also exit on their own when EffectCraft's end of the pipe closes.
pub fn stop_all() {
    for h in lock(helpers()).iter() {
        h.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helper_programs_are_recognised_by_name() {
        if cfg!(windows) {
            assert!(is_helper_name("effectcraft-helper-ofx.exe"));
            assert!(!is_helper_name("effectcraft-helper-ofx"));
        } else {
            assert!(is_helper_name("effectcraft-helper-ofx"));
            assert!(!is_helper_name("effectcraft-helper-ofx.pdb"));
        }
        assert!(!is_helper_name("effectcraft-helper-"));
        assert!(!is_helper_name("effectcraft"));
    }

    #[test]
    fn missing_folders_find_nothing_and_loading_without_helpers_is_an_error() {
        assert!(find_helpers(&[PathBuf::from("/no/such/folder")]).is_empty());
        assert!(load_path("/no/such/thing.xyz").is_err());
    }
}
