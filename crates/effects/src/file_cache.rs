//! Caches of what effects read from files named by a parameter (a LUT, an OCIO transform file, an
//! OCIO config), keyed by the parameter text and checked against the files each result was read
//! from (#3493, #3538).
//!
//! A cached result remembers every file its load read, with the file's size and modification time
//! (its stamp) taken before reading, including the files a nested cached load read (a config's
//! `FileTransform` LUTs) and files looked for but missing. Each lookup stamps those files again
//! (one `metadata` call each) and loads again once one moved, so a file edited, created, deleted or
//! repaired on disk is followed. Inline text reads no file and is its own content. An edit that keeps
//! both the size and the modification time can't be told apart.
//!
//! Every file read is also followed by [`check_lut_files`], and [`lut_generation`] moves when one is
//! found changed: the viewers and frame caches drop the frames made before.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::SystemTime;

/// A file's size and modification time when it was read (`None`: missing, or a file that could
/// not be described). A file whose stamp moved is read again.
pub(crate) type FileStamp = Option<(u64, Option<SystemTime>)>;

/// The files a load read and their stamps.
type Files = Vec<(String, FileStamp)>;

/// At most this many files are followed by [`check_lut_files`]; the least recently read go first.
const MAX_FOLLOWED_FILES: usize = 1024;

/// One cached result and what it was read from.
struct Entry<V> {
    files: Files,
    value: V,
    used: u64,
    /// The result's size estimate and the parameter text, counted against the cache's byte bound.
    bytes: usize,
}

/// Results by parameter text, at most `max_entries` and `max_bytes` of them, the least recently
/// used going first.
pub(crate) struct SourceCache<V> {
    map: HashMap<String, Entry<V>>,
    clock: u64,
    bytes: usize,
    max_entries: usize,
    max_bytes: usize,
}

impl<V> SourceCache<V> {
    pub(crate) fn new(max_entries: usize, max_bytes: usize) -> Self {
        SourceCache { map: HashMap::new(), clock: 0, bytes: 0, max_entries, max_bytes }
    }

    fn remove(&mut self, src: &str) {
        if let Some(e) = self.map.remove(src) {
            self.bytes = self.bytes.saturating_sub(e.bytes);
        }
    }

    /// Forget the results read from `path`.
    fn forget_file(&mut self, path: &str) {
        let stale: Vec<String> = self.map.iter().filter(|(_, e)| e.files.iter().any(|(p, _)| p == path)).map(|(k, _)| k.clone()).collect();
        for src in stale {
            self.remove(&src);
        }
    }

    /// Keep within the bounds, always the result for `keep`.
    fn evict(&mut self, keep: &str) {
        while self.map.len() > self.max_entries || (self.bytes > self.max_bytes && self.map.len() > 1) {
            let Some(oldest) = self.map.iter().filter(|(k, _)| k.as_str() != keep).min_by_key(|(_, e)| e.used).map(|(k, _)| k.clone()) else { break };
            self.remove(&oldest);
        }
    }

    /// The files the result cached for `src` was read from.
    pub(crate) fn files(&self, src: &str) -> Option<Vec<String>> {
        self.map.get(src).map(|e| e.files.iter().map(|(p, _)| p.clone()).collect())
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.map.len()
    }

    #[cfg(test)]
    pub(crate) fn contains(&self, src: &str) -> bool {
        self.map.contains_key(src)
    }

    #[cfg(test)]
    pub(crate) fn bytes(&self) -> (usize, usize, usize) {
        (self.bytes, self.map.values().map(|e| e.bytes).sum(), self.max_bytes)
    }
}

/// Lock a cache; a panic elsewhere while it was held leaves a usable map.
pub(crate) fn lock<V>(cache: &Mutex<SourceCache<V>>) -> MutexGuard<'_, SourceCache<V>> {
    cache.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A cache that registers with [`check_lut_files`] on first use.
pub(crate) struct Registered<V: 'static> {
    cache: OnceLock<Mutex<SourceCache<V>>>,
    max_entries: usize,
    max_bytes: usize,
}

impl<V: Clone + Send + 'static> Registered<V> {
    pub(crate) const fn new(max_entries: usize, max_bytes: usize) -> Self {
        Registered { cache: OnceLock::new(), max_entries, max_bytes }
    }

    pub(crate) fn get(&'static self) -> &'static Mutex<SourceCache<V>> {
        self.cache.get_or_init(|| {
            registry().push(self);
            Mutex::new(SourceCache::new(self.max_entries, self.max_bytes))
        })
    }
}

/// A cache [`check_lut_files`] tells to forget the results read from a changed file.
trait ForgetFile: Sync {
    fn forget_file(&self, path: &str);
    /// Add the files the cached results were read from to `out`.
    fn held_files(&self, out: &mut HashSet<String>);
}

impl<V: Send> ForgetFile for Registered<V> {
    fn forget_file(&self, path: &str) {
        if let Some(c) = self.cache.get() {
            lock(c).forget_file(path);
        }
    }

    fn held_files(&self, out: &mut HashSet<String>) {
        if let Some(c) = self.cache.get() {
            out.extend(lock(c).map.values().flat_map(|e| e.files.iter().map(|(p, _)| p.clone())));
        }
    }
}

fn registry() -> MutexGuard<'static, Vec<&'static dyn ForgetFile>> {
    static REGISTRY: OnceLock<Mutex<Vec<&'static dyn ForgetFile>>> = OnceLock::new();
    REGISTRY.get_or_init(Mutex::default).lock().unwrap_or_else(PoisonError::into_inner)
}

thread_local! {
    /// The loads running on this thread, innermost last: the files each read, and the bytes of
    /// the cached results it holds (a config holds the LUTs its `FileTransform`s loaded).
    static READING: RefCell<Vec<(Files, usize)>> = const { RefCell::new(Vec::new()) };
}

/// Note that the running load read (or looked for) `path`, stamped `stamp`.
pub(crate) fn note_file(path: &str, stamp: FileStamp) {
    READING.with(|r| {
        if let Ok(mut r) = r.try_borrow_mut()
            && let Some((top, _)) = r.last_mut()
            && !top.iter().any(|(p, _)| p == path)
        {
            top.push((path.to_string(), stamp));
        }
    });
}

/// Note that the running load holds a cached result of `bytes`.
fn note_bytes(bytes: usize) {
    READING.with(|r| {
        if let Ok(mut r) = r.try_borrow_mut()
            && let Some((_, held)) = r.last_mut()
        {
            *held = held.saturating_add(bytes);
        }
    });
}

/// Pops the files of a load when it ends, even by a panic.
struct Reading {
    popped: bool,
}

impl Reading {
    fn start() -> Reading {
        READING.with(|r| {
            if let Ok(mut r) = r.try_borrow_mut() {
                r.push((Vec::new(), 0));
            }
        });
        Reading { popped: false }
    }

    fn pop() -> Option<(Files, usize)> {
        READING.with(|r| r.try_borrow_mut().ok().and_then(|mut r| r.pop()))
    }

    /// The files the load read, and the bytes of the cached results it holds.
    fn finish(mut self) -> (Files, usize) {
        self.popped = true;
        Self::pop().unwrap_or_default()
    }
}

impl Drop for Reading {
    fn drop(&mut self) {
        if !self.popped {
            let _ = Self::pop();
        }
    }
}

/// The stamp of the file at `path` now: one `metadata` call.
pub(crate) fn file_stamp(path: &str) -> FileStamp {
    std::fs::metadata(path).ok().map(|md| (md.len(), md.modified().ok()))
}

/// Read the text file at `path` for the running load, stamped before reading (a file written
/// after it was stamped has a newer stamp, and is read again next time).
pub(crate) fn read(path: &str) -> std::io::Result<String> {
    note_file(path, file_stamp(path));
    std::fs::read_to_string(path)
}

/// Note that the running load looked for `path` and did not find it: it loads again once the file
/// appears.
pub(crate) fn note_missing(path: &str) {
    note_file(path, None);
}

/// The result cached for `src`, or `load`'s (and its size estimate in bytes), cached. `load` reads
/// files with [`read`] (and notes the ones it looked for with [`note_missing`]); no lock is held
/// while it runs or while the files are stamped.
pub(crate) fn cached<V: Clone>(cache: &Mutex<SourceCache<V>>, src: &str, load: impl FnOnce() -> (V, usize)) -> V {
    let files = lock(cache).map.get(src).map(|e| e.files.clone());
    let stale = files.is_some();
    if let Some(files) = files
        && files.iter().all(|(p, s)| file_stamp(p) == *s)
    {
        let mut c = lock(cache);
        c.clock += 1;
        let now = c.clock;
        if let Some(e) = c.map.get_mut(src)
            && e.files == files
        {
            e.used = now;
            let (value, bytes) = (e.value.clone(), e.bytes);
            drop(c);
            // (an enclosing load depends on these files too, and holds this result)
            for (p, s) in &files {
                note_file(p, *s);
            }
            note_bytes(bytes);
            return value;
        }
    }
    let reading = Reading::start();
    let (value, size) = load();
    let (files, held) = reading.finish();
    follow(&files, stale);
    for (p, s) in &files {
        note_file(p, *s);
    }
    // (what it holds counts against this cache's bound too: another cache letting go of it
    // frees nothing while this result keeps it)
    let bytes = size.saturating_add(held).saturating_add(src.len());
    note_bytes(bytes);
    let mut c = lock(cache);
    c.clock += 1;
    let used = c.clock;
    c.remove(src);
    c.map.insert(src.to_string(), Entry { files, value: value.clone(), used, bytes });
    c.bytes = c.bytes.saturating_add(bytes);
    c.evict(src);
    value
}

/// How often a file a cached load read was found changed on disk ([`lut_generation`]).
static GENERATION: AtomicU64 = AtomicU64::new(0);

/// The files followed by [`check_lut_files`]: the stamp each was last read with, and when.
#[derive(Default)]
struct Followed {
    files: HashMap<String, (FileStamp, u64)>,
    clock: u64,
}

fn followed() -> MutexGuard<'static, Followed> {
    static FOLLOWED: OnceLock<Mutex<Followed>> = OnceLock::new();
    FOLLOWED.get_or_init(Mutex::default).lock().unwrap_or_else(PoisonError::into_inner)
}

impl Followed {
    /// Note that `files` were read with these stamps; true when one has a different stamp than
    /// its last read's: a file changed on disk.
    fn read(&mut self, files: &Files) -> bool {
        let mut moved = false;
        for (path, stamp) in files {
            self.clock += 1;
            let now = self.clock;
            if self.files.insert(path.clone(), (*stamp, now)).is_some_and(|(old, _)| old != *stamp) {
                moved = true;
            }
        }
        moved
    }

    /// Keep at most [`MAX_FOLLOWED_FILES`], letting go of the least recently read first but never
    /// of a file in `held` (a cached result was read from it) or in `files` (just read).
    fn trim(&mut self, held: &HashSet<String>, files: &Files) {
        while self.files.len() > MAX_FOLLOWED_FILES {
            let Some(oldest) = self
                .files
                .iter()
                .filter(|(p, _)| !held.contains(p.as_str()) && !files.iter().any(|(f, _)| f == *p))
                .min_by_key(|(_, (_, used))| *used)
                .map(|(p, _)| p.clone())
            else {
                break;
            };
            self.files.remove(&oldest);
        }
    }
}

/// The files the registered caches' results were read from.
fn held_files() -> HashSet<String> {
    let caches: Vec<&'static dyn ForgetFile> = registry().clone();
    let mut out = HashSet::new();
    for c in caches {
        c.held_files(&mut out);
    }
    out
}

/// Note that `files` were read with these stamps. A file read with a different stamp than last
/// time, or a load replacing a result whose files moved (`stale`), means a file changed on disk,
/// and frames made before may show the old contents.
fn follow(files: &Files, stale: bool) {
    let (moved, over) = {
        let mut f = followed();
        (f.read(files), f.files.len() > MAX_FOLLOWED_FILES)
    };
    if over {
        // (gathered without the followed files' lock: no two of these locks are ever nested)
        let held = held_files();
        followed().trim(&held, files);
    }
    if moved || stale {
        GENERATION.fetch_add(1, Ordering::AcqRel);
    }
}

/// Changes each time a file a cached load read is found changed on disk (by a load itself or
/// [`check_lut_files`]): frames made before then may show the old contents.
pub fn lut_generation() -> u64 {
    GENERATION.load(Ordering::Acquire)
}

/// Look at every file read (one `metadata` call each) and forget the results read from the ones
/// that changed on disk since, so the next load reads them again. Returns [`lut_generation`].
pub fn check_lut_files() -> u64 {
    let files: Vec<(String, FileStamp)> = followed().files.iter().map(|(p, (s, _))| (p.clone(), *s)).collect();
    // (stamped without the lock: a file on a slow share must not hold up renders)
    let changed: Vec<(String, FileStamp, FileStamp)> =
        files.into_iter().filter_map(|(path, old)| Some(file_stamp(&path)).filter(|now| *now != old).map(|now| (path, old, now))).collect();
    let caches: Vec<&'static dyn ForgetFile> = registry().clone();
    for (path, old, now) in changed {
        {
            let mut f = followed();
            // (a render that read the file meanwhile has noted its new stamp already)
            match f.files.get_mut(&path) {
                Some((s, _)) if *s == old => *s = now,
                _ => continue,
            }
        }
        for c in &caches {
            c.forget_file(&path);
        }
        GENERATION.fetch_add(1, Ordering::AcqRel);
    }
    lut_generation()
}

#[cfg(test)]
pub(crate) fn followed_stamp(path: &str) -> Option<FileStamp> {
    followed().files.get(path).map(|(s, _)| *s)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Past [`MAX_FOLLOWED_FILES`], the file just read is still followed (a list of its own: the
    /// shared one's files belong to the other tests too).
    #[test]
    fn the_file_just_read_stays_followed_past_the_limit() {
        let mut f = Followed::default();
        let held: HashSet<String> = ["/followed/0.cube".to_string()].into();
        for i in 0..MAX_FOLLOWED_FILES + 8 {
            let files = vec![(format!("/followed/{i}.cube"), Some((1, None)))];
            assert!(!f.read(&files));
            f.trim(&held, &files);
            assert!(f.files.contains_key(&files[0].0), "{i}: the file just read");
            assert!(f.files.len() <= MAX_FOLLOWED_FILES);
        }
        assert!(f.files.contains_key("/followed/0.cube"), "a file a cached result was read from");
        assert!(!f.files.contains_key("/followed/1.cube"), "the least recently read other");
        assert!(f.read(&vec![("/followed/1030.cube".into(), Some((2, None)))]), "a moved stamp");
    }

    /// A nested cached load's files count for the enclosing load.
    #[test]
    fn an_enclosing_load_depends_on_a_nested_loads_files() {
        static INNER: Registered<u32> = Registered::new(8, 1 << 20);
        static OUTER: Registered<u32> = Registered::new(8, 1 << 20);
        let v = cached(OUTER.get(), "outer", || {
            note_missing("/nested/outer-own.cube");
            let inner = cached(INNER.get(), "inner", || {
                note_missing("/nested/inner.cube");
                (7, 100)
            });
            (inner, 0)
        });
        assert_eq!(v, 7);
        let files = lock(OUTER.get()).files("outer").unwrap();
        let held = |src: &str| lock(OUTER.get()).map.get(src).map(|e| e.bytes);
        // ("inner" is 5 bytes, its result 100: "outer" holds it, on a load and on a hit alike)
        assert_eq!(held("outer"), Some(100 + 5 + "outer".len()), "the result an enclosing load holds counts");
        assert_eq!(files, vec!["/nested/outer-own.cube".to_string(), "/nested/inner.cube".to_string()]);
        // A hit on the inner result still reports its files to a new enclosing load.
        let w = cached(OUTER.get(), "outer 2", || (cached(INNER.get(), "inner", || (0, 0)), 0));
        assert_eq!(w, 7);
        assert_eq!(lock(OUTER.get()).files("outer 2").unwrap(), vec!["/nested/inner.cube".to_string()]);
        assert_eq!(held("outer 2"), Some(100 + 5 + "outer 2".len()));
    }
}
