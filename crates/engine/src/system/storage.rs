//! Browser storage management: what the web app keeps in the browser (projects, imported media,
//! auto-saves and settings in the Origin Private File System or IndexedDB; the disk cache's
//! frames in the Origin Private File System), how much of the origin's quota that uses, whether
//! the browser keeps it under storage pressure, and clearing it.
//!
//! The browser's storage APIs are asynchronous and the engine's commands are not, so a
//! [`StorageHost`] answers from its latest snapshot (refreshing it in the background) and
//! starts requests that finish later; the snapshot reports their outcome.
//!
//! Commands (agents, Settings ▸ Disk ▸ Browser Storage): `storage.info`, `storage.persist`,
//! `storage.clear {what}`. Without a host (the desktop) `storage.info` reports
//! `{available: false}` and the others fail.

use serde_json::{Value, json};

use crate::commands::{CommandSpec, str_p};
use crate::{Result, Session, cmd};

/// What can be cleared ([`StorageHost::clear`]).
pub const CLEARABLE: &[&str] = &["diskCache", "media", "projects", "autoSaves", "all"];

/// The browser's storage, as the web app manages it.
pub trait StorageHost: Send + Sync {
    /// The latest snapshot: `{backend, usage, quota, persisted, files: {count, bytes, media,
    /// projects, autoSaves}, diskCache: {enabled, entries, bytes, maxBytes, hits, misses, writes,
    /// evictions}}` (`usage` / `quota` from `navigator.storage.estimate()`, in bytes). Asks for
    /// a fresh one.
    fn info(&self) -> Value;
    /// Ask the browser to keep the storage under pressure (`navigator.storage.persist()`); the
    /// answer arrives in a later [`StorageHost::info`] (`persisted`).
    fn request_persist(&self) -> Value;
    /// Delete stored data: `diskCache` (cached frames), `media` (imported footage), `projects`
    /// (saved projects), `autoSaves`, or `all` of them (settings stay). Returns what was
    /// removed: `{cleared, entries, bytes}`.
    fn clear(&self, what: &str) -> std::result::Result<Value, String>;
}

fn host(s: &Session, cmd: &str) -> Result<std::sync::Arc<dyn StorageHost>> {
    s.storage.clone().ok_or_else(|| crate::commands::bad(cmd, "browser storage is only managed in the web app"))
}

fn info(s: &mut Session, _: &Value) -> Result<Value> {
    Ok(match &s.storage {
        Some(h) => {
            let mut v = h.info();
            if let Some(o) = v.as_object_mut() {
                o.insert("available".into(), json!(true));
            }
            v
        }
        None => json!({"available": false}),
    })
}

fn persist(s: &mut Session, _: &Value) -> Result<Value> {
    Ok(host(s, "storage.persist")?.request_persist())
}

fn clear(s: &mut Session, p: &Value) -> Result<Value> {
    let what = str_p(p, "what").unwrap_or("diskCache");
    if !CLEARABLE.contains(&what) {
        return Err(crate::commands::bad("storage.clear", format!("what: one of {}", CLEARABLE.join(", "))));
    }
    let h = host(s, "storage.clear")?;
    let r = h.clear(what).map_err(|e| crate::commands::bad("storage.clear", e))?;
    s.toast(format!("Cleared browser storage: {what}"));
    Ok(r)
}

fn has_host(s: &Session) -> std::result::Result<(), String> {
    if s.storage.is_some() { Ok(()) } else { Err("browser storage is only managed in the web app".into()) }
}

pub(crate) fn specs() -> Vec<CommandSpec> {
    vec![
        crate::query!(
            "storage.info",
            "Browser Storage",
            "{} → {available, backend, usage, quota, persisted, files: {count, bytes, media, projects, autoSaves}, diskCache: {enabled, entries, bytes, maxBytes, hits, misses, writes, evictions}}",
            info
        ),
        cmd!("storage.persist", "Request Persistent Storage", [], None, "{} → {requested, persisted}", has_host, persist),
        cmd!("storage.clear", "Clear Browser Storage", [], None, "{what: diskCache|media|projects|autoSaves|all} → {cleared, entries, bytes}", has_host, clear),
    ]
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;

    #[derive(Default)]
    struct Mock {
        cleared: Mutex<Vec<String>>,
        persisted: Mutex<bool>,
    }

    impl StorageHost for Mock {
        fn info(&self) -> Value {
            json!({"backend": "opfs", "usage": 1000, "quota": 100000, "persisted": *self.persisted.lock().unwrap(),
                   "diskCache": {"enabled": true, "entries": 3, "bytes": 300, "maxBytes": 1 << 30}})
        }
        fn request_persist(&self) -> Value {
            *self.persisted.lock().unwrap() = true;
            json!({"requested": true})
        }
        fn clear(&self, what: &str) -> std::result::Result<Value, String> {
            self.cleared.lock().unwrap().push(what.into());
            Ok(json!({"cleared": what, "entries": 3, "bytes": 300}))
        }
    }

    #[test]
    fn storage_commands_go_to_the_host() {
        let mut s = Session::default();
        assert_eq!(s.execute_checked("storage.info", json!({})).unwrap(), json!({"available": false}));
        assert!(s.execute_checked("storage.clear", json!({"what": "diskCache"})).is_err());
        let m = Arc::new(Mock::default());
        s.storage = Some(m.clone());
        let i = s.execute_checked("storage.info", json!({})).unwrap();
        assert_eq!((i["available"].clone(), i["backend"].clone(), i["persisted"].clone()), (json!(true), json!("opfs"), json!(false)));
        assert_eq!(s.execute_checked("storage.persist", json!({})).unwrap()["requested"], json!(true));
        assert_eq!(s.execute_checked("storage.info", json!({})).unwrap()["persisted"], json!(true));
        assert!(s.execute_checked("storage.clear", json!({"what": "everything"})).is_err());
        assert_eq!(s.execute_checked("storage.clear", json!({"what": "media"})).unwrap()["cleared"], json!("media"));
        // The disk cache's own commands use the browser's disk cache when there is no folder.
        let st = s.execute_checked("cache.diskStats", json!({})).unwrap();
        assert_eq!((st["enabled"].clone(), st["entries"].clone()), (json!(true), json!(3)));
        assert_eq!(s.execute_checked("edit.purge", json!({"what": "disk"})).unwrap()["diskEntries"], json!(3));
        assert_eq!(*m.cleared.lock().unwrap(), vec!["media".to_string(), "diskCache".to_string()]);
    }
}
