//! What the browser keeps between visits: the in-memory side of the persistent store.
//!
//! The engine's storage traits are synchronous ([`ConfigStore`], [`FileOps`],
//! [`effectcraft_engine::Services`]); the browser's storage (the Origin Private File System, or
//! IndexedDB where OPFS can't write) is asynchronous. So the app works on a [`Mirror`]: every
//! stored entry is loaded into memory before the app starts, reads and writes hit the mirror
//! synchronously, and writes queue up as pending operations that the web host flushes to the
//! browser in the background (coalesced per key, in order).
//!
//! Keys are namespaced:
//!
//! | Key | What |
//! |---|---|
//! | `config/<name>` | settings (`prefs.json`), shortcut presets, the recovery sentinel, the session snapshot |
//! | `files<path>` | the virtual file table: imported media, saved projects, auto-saves (`/EffectCraft Auto-Save/…`) |
//!
//! Render outputs go into the file table unpersisted (they download instead).
//!
//! This module is plain Rust (no browser APIs) so it is unit-tested natively.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use effectcraft_engine::config::{ConfigStore, FileOps};

/// Prefix of configuration entries.
pub const CONFIG: &str = "config/";
/// Prefix of file-table entries (followed by the absolute path, `files/clip.mp4`).
pub const FILES: &str = "files";
/// The session snapshot (project + editor state), written while the app runs and restored
/// on the next visit.
pub const SNAPSHOT: &str = "session.ecproj";
/// Its metadata ([`SnapshotMeta`]).
pub const SNAPSHOT_META: &str = "session.json";

/// One stored blob.
#[derive(Clone, Debug)]
pub struct Entry {
    pub data: Arc<[u8]>,
    /// Milliseconds since the Unix epoch.
    pub modified: f64,
    /// Written to the browser's storage (render outputs are not).
    pub persist: bool,
    /// Which write this is: new with every write of the key, so a file replaced by another of
    /// the same size still reads as changed (workers are sent it again, #218).
    pub version: u64,
    /// FNV-1a of `data`, filled the first time a disk-cache key asks for it.
    hash: Option<u64>,
}

/// A queued write for the browser storage: `data` = `None` deletes the key.
#[derive(Clone, Debug, PartialEq)]
pub struct Pending {
    pub key: String,
    pub data: Option<Arc<[u8]>>,
    pub modified: f64,
}

/// The in-memory copy of the store plus the writes not yet flushed.
#[derive(Default)]
pub struct Mirror {
    entries: BTreeMap<String, Entry>,
    /// Keys changed since the last [`Mirror::take_pending`], in first-change order.
    dirty: Vec<String>,
    /// The last [`Entry::version`] given.
    version: u64,
}

impl Mirror {
    /// An entry read from the browser's storage at startup (nothing to flush).
    pub fn load(&mut self, key: &str, data: Arc<[u8]>, modified: f64) {
        let version = self.next_version();
        self.entries.insert(key.to_string(), Entry { data, modified, persist: true, version, hash: None });
    }

    pub fn put(&mut self, key: &str, data: Arc<[u8]>, modified: f64, persist: bool) {
        let was_persisted = self.entries.get(key).is_some_and(|e| e.persist);
        let version = self.next_version();
        self.entries.insert(key.to_string(), Entry { data, modified, persist, version, hash: None });
        if persist || was_persisted {
            self.touch(key);
        }
    }

    pub fn remove(&mut self, key: &str) -> bool {
        match self.entries.remove(key) {
            Some(e) => {
                if e.persist {
                    self.touch(key);
                }
                true
            }
            None => false,
        }
    }

    fn next_version(&mut self) -> u64 {
        self.version = self.version.wrapping_add(1);
        self.version
    }

    /// A write of `key` that failed: the next flush writes the key's latest state again (#215).
    pub fn retry(&mut self, key: &str) {
        self.touch(key);
    }

    fn touch(&mut self, key: &str) {
        if !self.dirty.iter().any(|k| k == key) {
            self.dirty.push(key.to_string());
        }
    }

    pub fn get(&self, key: &str) -> Option<&Entry> {
        self.entries.get(key)
    }

    /// Entries under `prefix`: (key, size, modified, persisted).
    pub fn list(&self, prefix: &str) -> Vec<(String, usize, f64, bool)> {
        self.entries
            .range(prefix.to_string()..)
            .take_while(|(k, _)| k.starts_with(prefix))
            .map(|(k, e)| (k.clone(), e.data.len(), e.modified, e.persist))
            .collect()
    }

    /// The writes to flush, one per changed key with its latest state (a put, or a delete when
    /// the key is gone or no longer persisted).
    pub fn take_pending(&mut self) -> Vec<Pending> {
        std::mem::take(&mut self.dirty)
            .into_iter()
            .map(|key| match self.entries.get(&key) {
                Some(e) if e.persist => Pending { data: Some(e.data.clone()), modified: e.modified, key },
                _ => Pending { key, data: None, modified: 0.0 },
            })
            .collect()
    }

    pub fn has_pending(&self) -> bool {
        !self.dirty.is_empty()
    }
}

/// The key of a file-table path (`/a/b.mov` → `files/a/b.mov`).
pub fn file_key(path: &str) -> String {
    if path.starts_with('/') { format!("{FILES}{path}") } else { format!("{FILES}/{path}") }
}

/// The path of a file-table key.
pub fn key_path(key: &str) -> Option<&str> {
    key.strip_prefix(FILES).filter(|p| p.starts_with('/'))
}

/// FNV-1a 64. Computed when a disk-cache key first needs the file, not on every write.
fn content_hash(data: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in data {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// The persistent store: a [`Mirror`] and a hook that wakes the flusher after changes.
pub struct Store {
    pub mirror: Mutex<Mirror>,
    /// Called after every change that needs flushing (the web host schedules a flush).
    pub on_change: OnceLock<fn()>,
    /// Clock in milliseconds since the Unix epoch (tests pin it).
    pub clock: OnceLock<fn() -> f64>,
    /// Bumped when a virtual file is written. Preview cache lookups key on it; they do not hash.
    media_epoch: Arc<AtomicU64>,
}

impl Default for Store {
    fn default() -> Self {
        Self { mirror: Mutex::default(), on_change: OnceLock::new(), clock: OnceLock::new(), media_epoch: Arc::new(AtomicU64::new(0)) }
    }
}

impl Store {
    pub fn lock(&self) -> std::sync::MutexGuard<'_, Mirror> {
        self.mirror.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn now(&self) -> f64 {
        self.clock.get().map_or(0.0, |f| f())
    }

    fn changed(&self) {
        if let Some(f) = self.on_change.get() {
            f();
        }
    }

    // ---- file table

    /// Store a file under `path` (persisted unless `persist` is false: render outputs).
    pub fn put_file(&self, path: &str, data: Arc<[u8]>, persist: bool) {
        let now = self.now();
        let key = file_key(path);
        let replaced = self.lock().get(&key).is_none_or(|e| !Arc::ptr_eq(&e.data, &data));
        self.lock().put(&key, data, now, persist);
        if replaced {
            self.media_epoch.fetch_add(1, Ordering::Relaxed);
        }
        if persist {
            self.changed();
        }
    }

    /// The generation preview cache lookups key on. It changes when [`Self::put_file`] writes
    /// a different blob, and it is not a content hash.
    pub fn media_epoch(&self) -> Arc<AtomicU64> {
        self.media_epoch.clone()
    }

    /// Size and content hash of the file under `path`. The hash is stored on the entry so a
    /// later key does not walk the bytes again.
    pub fn file_identity(&self, path: &str) -> Option<(u64, u64)> {
        let key = file_key(path);
        let mut mirror = self.lock();
        let entry = mirror.entries.get(&key)?;
        if let Some(hash) = entry.hash {
            return Some((entry.data.len() as u64, hash));
        }
        let hash = content_hash(&entry.data);
        let len = entry.data.len() as u64;
        if let Some(entry) = mirror.entries.get_mut(&key) {
            entry.hash = Some(hash);
        }
        Some((len, hash))
    }

    pub fn file(&self, path: &str) -> Option<Arc<[u8]>> {
        self.lock().get(&file_key(path)).map(|e| e.data.clone())
    }

    /// The file under `path` with its [`Entry::version`].
    pub fn file_versioned(&self, path: &str) -> Option<(Arc<[u8]>, u64)> {
        self.lock().get(&file_key(path)).map(|e| (e.data.clone(), e.version))
    }

    pub fn remove_file(&self, path: &str) -> bool {
        let r = self.lock().remove(&file_key(path));
        self.changed();
        r
    }

    /// Every file: (path, size, modified ms, persisted).
    pub fn files(&self) -> Vec<(String, usize, f64, bool)> {
        self.lock().list(FILES).into_iter().filter_map(|(k, n, m, p)| Some((key_path(&k)?.to_string(), n, m, p))).collect()
    }

    /// Stored (persisted) files by kind: `{projects, media, autoSaves}`, each `{count, bytes}`
    /// (the storage manager, `storage.info`).
    pub fn usage(&self) -> serde_json::Value {
        let mut out = serde_json::Map::new();
        for k in [FileKind::Project, FileKind::Media, FileKind::AutoSave] {
            let (mut count, mut bytes) = (0u64, 0u64);
            for (path, n, _, persisted) in self.files() {
                if persisted && FileKind::of(&path) == k {
                    count += 1;
                    bytes += n as u64;
                }
            }
            out.insert(k.name().into(), serde_json::json!({"count": count, "bytes": bytes}));
        }
        out.into()
    }

    /// Delete every stored file of a kind; returns (files, bytes) removed.
    pub fn clear_kind(&self, kind: FileKind) -> (u64, u64) {
        let (mut count, mut bytes) = (0u64, 0u64);
        for (path, n, _, persisted) in self.files() {
            if persisted && FileKind::of(&path) == kind {
                self.lock().remove(&file_key(&path));
                count += 1;
                bytes += n as u64;
            }
        }
        if count > 0 {
            self.changed();
        }
        (count, bytes)
    }
}

/// What a stored file is (Settings ▸ Disk ▸ Browser Storage).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileKind {
    /// A saved project (`.ecproj` outside the auto-save folder).
    Project,
    /// Imported footage and everything else.
    Media,
    /// Auto-saves (`/EffectCraft Auto-Save/…`, or `… auto-save N.ecproj`).
    AutoSave,
}

impl FileKind {
    pub fn of(path: &str) -> FileKind {
        let lower = path.to_ascii_lowercase();
        if !lower.ends_with(".ecproj") {
            return FileKind::Media;
        }
        let name = lower.rsplit('/').next().unwrap_or(&lower);
        if lower.contains("/effectcraft auto-save/") || name.contains(" auto-save ") { FileKind::AutoSave } else { FileKind::Project }
    }

    /// `storage.clear` / `storage.info` name.
    pub fn name(self) -> &'static str {
        match self {
            FileKind::Project => "projects",
            FileKind::Media => "media",
            FileKind::AutoSave => "autoSaves",
        }
    }

    pub fn from_name(n: &str) -> Option<FileKind> {
        [FileKind::Project, FileKind::Media, FileKind::AutoSave].into_iter().find(|k| k.name() == n)
    }
}

/// Settings and friends ([`ConfigStore`]) in the store; auto-saves ([`FileOps`]) in its file
/// table, under `/` (untitled projects auto-save to `/EffectCraft Auto-Save/`).
pub struct WebConfig {
    store: Arc<Store>,
    files: WebFiles,
}

impl WebConfig {
    pub fn new(store: Arc<Store>) -> WebConfig {
        WebConfig { files: WebFiles(store.clone()), store }
    }
}

/// [`FileOps`] on the store's file table.
pub struct WebFiles(pub Arc<Store>);

impl ConfigStore for WebConfig {
    fn read(&self, name: &str) -> Option<String> {
        let m = self.store.lock();
        m.get(&format!("{CONFIG}{name}")).map(|e| String::from_utf8_lossy(&e.data).into_owned())
    }
    fn write(&self, name: &str, data: &str) -> std::io::Result<()> {
        let now = self.store.now();
        self.store.lock().put(&format!("{CONFIG}{name}"), data.as_bytes().into(), now, true);
        self.store.changed();
        Ok(())
    }
    fn remove(&self, name: &str) -> std::io::Result<()> {
        self.store.lock().remove(&format!("{CONFIG}{name}"));
        self.store.changed();
        Ok(())
    }
    fn list(&self, dir: &str) -> Vec<String> {
        let pre = format!("{CONFIG}{}/", dir.trim_end_matches('/'));
        let mut v: Vec<String> = self
            .store
            .lock()
            .list(&pre)
            .into_iter()
            .filter_map(|(k, ..)| k.strip_prefix(&pre).filter(|n| !n.is_empty() && !n.contains('/')).map(str::to_string))
            .collect();
        v.sort();
        v
    }
    fn dir(&self) -> Option<PathBuf> {
        Some(PathBuf::from("/"))
    }
    fn files(&self) -> &dyn FileOps {
        &self.files
    }
}

fn path_str(p: &Path) -> String {
    p.to_string_lossy().replace('\\', "/")
}

impl FileOps for WebFiles {
    fn list(&self, dir: &Path) -> Vec<(String, std::time::SystemTime)> {
        let dir = path_str(dir);
        let dir = dir.trim_end_matches('/');
        self.0
            .files()
            .into_iter()
            .filter_map(|(p, _, m, _)| {
                let (parent, name) = p.rsplit_once('/')?;
                (parent == dir).then(|| (name.to_string(), std::time::UNIX_EPOCH + std::time::Duration::from_secs_f64(m.max(0.0) / 1000.0)))
            })
            .collect()
    }
    fn write(&self, path: &Path, data: &[u8]) -> std::io::Result<()> {
        self.0.put_file(&path_str(path), data.into(), true);
        Ok(())
    }
    fn remove(&self, path: &Path) -> std::io::Result<()> {
        if self.0.remove_file(&path_str(path)) { Ok(()) } else { Err(std::io::Error::new(std::io::ErrorKind::NotFound, path_str(path))) }
    }
    fn is_file(&self, path: &Path) -> bool {
        self.0.file(&path_str(path)).is_some()
    }
}

/// What the session snapshot records besides the project.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SnapshotMeta {
    /// The project's file (None = untitled).
    pub path: Option<String>,
    /// Unsaved changes when the snapshot was taken.
    pub dirty: bool,
    /// Viewer, timeline and selection state.
    pub state: Option<serde_json::Value>,
    /// Milliseconds since the Unix epoch.
    pub saved: f64,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> Arc<Store> {
        let s = Arc::new(Store::default());
        let _ = s.clock.set(|| 1_000.0);
        s
    }

    #[test]
    fn usage_by_kind_and_clearing() {
        let s = store();
        s.put_file("/a.ecproj", Arc::from(&b"proj"[..]), true);
        s.put_file("/clip.mp4", Arc::from(&b"0123456789"[..]), true);
        s.put_file("/still.png", Arc::from(&b"px"[..]), true);
        s.put_file("/EffectCraft Auto-Save/a auto-save 1.ecproj", Arc::from(&b"auto"[..]), true);
        s.put_file("/render.gif", Arc::from(&b"out"[..]), false);
        let u = s.usage();
        assert_eq!(u["projects"], serde_json::json!({"count": 1, "bytes": 4}));
        assert_eq!(u["media"], serde_json::json!({"count": 2, "bytes": 12}), "render outputs are not stored");
        assert_eq!(u["autoSaves"], serde_json::json!({"count": 1, "bytes": 4}));
        assert_eq!(FileKind::from_name("autoSaves"), Some(FileKind::AutoSave));
        assert_eq!(FileKind::of("/x/My auto-save 3.ecproj"), FileKind::AutoSave);
        s.lock().take_pending();
        assert_eq!(s.clear_kind(FileKind::Media), (2, 12));
        assert!(s.file("/clip.mp4").is_none() && s.file("/a.ecproj").is_some() && s.file("/render.gif").is_some());
        let deleted: Vec<String> = s.lock().take_pending().into_iter().filter(|p| p.data.is_none()).map(|p| p.key).collect();
        assert_eq!(deleted.len(), 2, "the deletions are flushed to browser storage");
        assert_eq!(s.usage()["media"]["count"], 0);
    }

    #[test]
    fn writes_coalesce_per_key_in_order() {
        let mut m = Mirror::default();
        m.put("config/a", Arc::from(&b"1"[..]), 1.0, true);
        m.put("files/x.png", Arc::from(&b"px"[..]), 2.0, true);
        m.put("config/a", Arc::from(&b"2"[..]), 3.0, true);
        let p = m.take_pending();
        assert_eq!(p.len(), 2);
        assert_eq!((p[0].key.as_str(), p[0].data.as_deref()), ("config/a", Some(&b"2"[..])));
        assert_eq!(p[0].modified, 3.0);
        assert_eq!(p[1].key, "files/x.png");
        assert!(!m.has_pending() && m.take_pending().is_empty());
        // Delete after put: one delete.
        m.put("files/y", Arc::from(&b"y"[..]), 4.0, true);
        m.remove("files/y");
        assert_eq!(m.take_pending(), vec![Pending { key: "files/y".into(), data: None, modified: 0.0 }]);
    }

    #[test]
    fn unpersisted_entries_never_flush_and_loaded_ones_are_clean() {
        let mut m = Mirror::default();
        m.load("files/old.mov", Arc::from(&b"m"[..]), 5.0);
        m.put("files/render.gif", Arc::from(&b"g"[..]), 6.0, false);
        assert!(!m.has_pending());
        assert!(!m.remove("files/none"));
        m.remove("files/render.gif");
        assert!(!m.has_pending());
        // Overwriting a persisted file with an unpersisted one deletes the stored copy.
        m.put("files/old.mov", Arc::from(&b"n"[..]), 7.0, false);
        assert_eq!(m.take_pending()[0].data, None);
        assert_eq!(m.list("files").len(), 1);
    }

    /// A file replaced by different bytes of the same size gets a new version, so workers are
    /// sent it again (#218).
    #[test]
    fn versions_change_with_every_write() {
        let s = store();
        s.put_file("/collision.png", Arc::from(&b"red!"[..]), true);
        let (_, a) = s.file_versioned("/collision.png").unwrap();
        s.put_file("/collision.png", Arc::from(&b"blue"[..]), true);
        let (data, b) = s.file_versioned("/collision.png").unwrap();
        assert_ne!(a, b);
        assert_eq!(&data[..], b"blue");
        assert_eq!(s.file_versioned("/collision.png").map(|f| f.1), Some(b), "reading doesn't change it");
        let epoch = s.media_epoch().load(Ordering::Relaxed);
        let id = s.file_identity("/collision.png").expect("identity");
        assert_eq!(id, (4, content_hash(b"blue")));
        assert_eq!(s.file_identity("/collision.png"), Some(id), "the hash is kept on the entry");
        assert!(epoch >= 2, "each new blob bumps the generation the preview keys on");
    }

    /// A write that failed is flushed again with the key's latest state (#215).
    #[test]
    fn failed_writes_retry() {
        let s = store();
        s.put_file("/collision.png", Arc::from(&b"blue"[..]), true);
        let mut m = s.lock();
        let p = m.take_pending();
        assert_eq!(p.len(), 1);
        assert!(!m.has_pending());
        m.retry(&p[0].key);
        assert_eq!(m.take_pending(), p, "the same write again");
        // A newer change wins over the failed one.
        m.retry("files/collision.png");
        m.put("files/collision.png", Arc::from(&b"gold"[..]), 2.0, true);
        let again = m.take_pending();
        assert_eq!((again.len(), again[0].data.as_deref()), (1, Some(&b"gold"[..])));
    }

    #[test]
    fn keys_and_paths_round_trip() {
        assert_eq!(file_key("/a b/c.mov"), "files/a b/c.mov");
        assert_eq!(file_key("c.mov"), "files/c.mov");
        assert_eq!(key_path("files/a b/c.mov"), Some("/a b/c.mov"));
        assert_eq!(key_path("config/prefs.json"), None);
        let s = store();
        s.put_file("/clip.mp4", Arc::from(&b"abc"[..]), true);
        s.put_file("/out.gif", Arc::from(&b"g"[..]), false);
        let f = s.files();
        assert_eq!(f, vec![("/clip.mp4".to_string(), 3, 1000.0, true), ("/out.gif".to_string(), 1, 1000.0, false)]);
        assert_eq!(s.file("/clip.mp4").as_deref(), Some(&b"abc"[..]));
    }

    #[test]
    fn config_store_and_file_ops() {
        let s = store();
        let c = WebConfig::new(s.clone());
        assert_eq!(c.read("prefs.json"), None);
        c.write("prefs.json", "{\"a\":1}").unwrap();
        assert_eq!(c.read("prefs.json").as_deref(), Some("{\"a\":1}"));
        c.remove("prefs.json").unwrap();
        assert_eq!(c.read("prefs.json"), None);
        // Config entries are not files.
        c.write("session.lock", "{}").unwrap();
        assert!(s.files().is_empty());
        let fs = c.files();
        fs.write(Path::new("/EffectCraft Auto-Save/P auto-save 1.ecproj"), b"{}").unwrap();
        fs.write(Path::new("/EffectCraft Auto-Save/P auto-save 2.ecproj"), b"{}").unwrap();
        fs.write(Path::new("/other.ecproj"), b"{}").unwrap();
        let mut names: Vec<String> = fs.list(Path::new("/EffectCraft Auto-Save")).into_iter().map(|e| e.0).collect();
        names.sort();
        assert_eq!(names, ["P auto-save 1.ecproj", "P auto-save 2.ecproj"]);
        assert!(fs.is_file(Path::new("/other.ecproj")));
        fs.remove(Path::new("/other.ecproj")).unwrap();
        assert!(!fs.is_file(Path::new("/other.ecproj")));
        assert!(fs.remove(Path::new("/other.ecproj")).is_err());
        let pending = s.lock().take_pending();
        let keys: Vec<&str> = pending.iter().map(|p| p.key.as_str()).collect();
        assert_eq!(
            keys,
            [
                "config/prefs.json",
                "config/session.lock",
                "files/EffectCraft Auto-Save/P auto-save 1.ecproj",
                "files/EffectCraft Auto-Save/P auto-save 2.ecproj",
                "files/other.ecproj"
            ]
        );
        assert_eq!(pending[0].data, None, "removed");
        assert_eq!(pending[4].data, None, "removed");
    }

    #[test]
    fn autosave_and_recovery_run_in_the_browser_store() {
        use effectcraft_engine::Session;
        use serde_json::json;
        let s = store();
        let mut a = Session { config: Some(Arc::new(WebConfig::new(s.clone()))), ..Default::default() };
        a.load_settings();
        assert!(a.begin_recovery().is_none());
        a.execute("file.newProject", json!({})).unwrap();
        a.execute("comp.new", json!({"name": "C"})).unwrap();
        // Untitled: the auto-save lands in the store's file table.
        let p = a.autosave_now().unwrap();
        assert_eq!(Path::new(&p), Path::new("/EffectCraft Auto-Save/Untitled Project auto-save 1.ecproj"));
        assert!(s.file(&path_str(Path::new(&p))).is_some());
        // The tab closes without a clean exit: the next visit offers the auto-save.
        let mut b = Session { config: Some(Arc::new(WebConfig::new(s.clone()))), ..Default::default() };
        b.load_settings();
        let r = b.begin_recovery().expect("recovery");
        assert!(r.autosave.is_some());
        // Increment and Save sees the store's files.
        let fs = WebConfig::new(s.clone());
        fs.files().write(Path::new("/A.ecproj"), b"{}").unwrap();
        let next = effectcraft_engine::autosave::increment_path("/A.ecproj", |p| fs.files().is_file(Path::new(p)));
        assert_eq!(next, "/A 2.ecproj");
    }

    #[test]
    fn snapshot_meta_round_trips() {
        let m = SnapshotMeta { path: Some("/p.ecproj".into()), dirty: true, state: Some(serde_json::json!({"activeComp": 3})), saved: 12.5 };
        let t = serde_json::to_string(&m).unwrap();
        assert_eq!(serde_json::from_str::<SnapshotMeta>(&t).unwrap(), m);
        assert_eq!(serde_json::from_str::<SnapshotMeta>("{}").unwrap(), SnapshotMeta::default());
    }
}
