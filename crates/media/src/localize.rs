//! The background footage localizer (#3271, Settings ▸ Disk ▸ Localize Network Footage, off by
//! default): footage on a network share is copied, file by file, to a local folder while the
//! user works, so later reads come from the local disk.
//!
//! - **Network footage**: UNC paths (`\\server\share\…`) and the locations the user lists
//!   (Settings ▸ Disk ▸ Network Footage Locations, such as a drive letter mapped to a share,
//!   `P:\`). No system call is made to tell a mapped drive apart.
//! - **Never on the playback path.** A read asks for a copy ([`request`]) and goes on reading
//!   the share; one background thread copies, one file at a time.
//! - **Used only when complete and verified.** A copy is made under a temporary name (`.part`),
//!   renamed when complete, and recorded beside it (`.src`: the source's path and identity when
//!   it was copied). It is read only while the source still has that identity and the copy its
//!   size ([`local_copy`]); a source that changed is read from the share (and copied again).
//! - **Readers keep naming the source.** Entries made of a localized file list the share's path
//!   and identity (see `ExrSource::Localized`), so caches are keyed the same either way.
//! - **Bounded, across sessions.** The copies take at most the size limit and leave the disk
//!   cache's reserve free on their disk ([`effectcraft_render::disk_cache::disk_reserve`]); the
//!   least recently used go first. The folder's copies are adopted when the localizer starts
//!   (and stale temporaries deleted), so they count towards the limit after a restart.
//! - **Visible.** [`status`] says how many files are local, their size, what is queued, what is
//!   being copied and which locations count as network.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Condvar, Mutex, PoisonError};

use effectcraft_render::reads::FileStamp;
use effectcraft_render::{LocalizeConfig, LocalizeReport};

/// A complete local copy.
struct Copy {
    local: PathBuf,
    /// The copy's size and the source's identity when it was copied.
    len: u64,
    source: FileStamp,
    last_use: u64,
}

#[derive(Default)]
struct State {
    config: Option<LocalizeConfig>,
    copies: HashMap<PathBuf, Copy>,
    queue: VecDeque<PathBuf>,
    queued: HashSet<PathBuf>,
    bytes: u64,
    clock: u64,
    /// The file being copied now and the setting ([`State::epoch`]) it is copied under: asked
    /// for again meanwhile it isn't queued a second time (unless the setting changed since).
    copying: Option<(PathBuf, u64)>,
    skipped: HashSet<PathBuf>,
    worker: bool,
    /// Bumped with each new setting: a copy started under another one is thrown away.
    epoch: u64,
    /// The free space seen last on the copies' disk and when (read at most every 30 s).
    space: Option<((u64, u64), std::time::Instant)>,
    /// The limit is held below the setting by free disk space.
    reserve_limited: bool,
    /// Folders being adopted now ([`configure`]): the worker starts no copy meanwhile (a copy
    /// already running is left alone by the scan, see [`adopt`]).
    adopting: u32,
}

/// At most this many files wait to be copied (older requests are dropped first).
const QUEUE_LIMIT: usize = 4096;

fn state() -> &'static (Mutex<State>, Condvar) {
    static STATE: std::sync::OnceLock<(Mutex<State>, Condvar)> = std::sync::OnceLock::new();
    STATE.get_or_init(Default::default)
}

fn lock() -> std::sync::MutexGuard<'static, State> {
    state().0.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The settings that decide what is copied where (the free-space reader aside).
fn same(a: Option<&LocalizeConfig>, b: Option<&LocalizeConfig>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => a.folder == b.folder && a.max_bytes == b.max_bytes && a.network == b.network,
        _ => false,
    }
}

/// Turn the localizer on with `config`, or off (`None`): the copies stay on disk but are no
/// longer read. A new folder's earlier copies are adopted (their records beside them), stale
/// temporaries deleted.
pub fn configure(config: Option<LocalizeConfig>) {
    // (the file system is read and written outside the lock: reads ask the localizer for copies
    // on every frame, and a slow folder must not hold them up)
    let (epoch, adopt_from, space_of) = {
        let mut st = lock();
        if same(st.config.as_ref(), config.as_ref()) {
            if let Some(c) = config {
                st.config = Some(c);
            }
            return;
        }
        st.epoch = st.epoch.wrapping_add(1);
        let new_folder = st.config.as_ref().map(|c| &c.folder) != config.as_ref().map(|c| &c.folder);
        st.queue.clear();
        st.queued.clear();
        st.skipped.clear();
        st.space = None;
        st.config = config;
        if new_folder {
            st.copies.clear();
            st.bytes = 0;
        }
        let adopt_from = st.config.as_ref().filter(|_| new_folder).map(|c| c.folder.clone());
        if adopt_from.is_some() {
            st.adopting = st.adopting.saturating_add(1);
        }
        // (the file being copied now keeps its files: its copy may land in this folder)
        let busy = st.copying.as_ref().map(|(p, _)| local_name(p));
        (st.epoch, adopt_from.map(|f| (f, busy)), st.config.as_ref().and_then(|c| c.disk_space.clone()))
    };
    // (ends the adoption also when the scan or the free-space reader panics)
    let guard = adopt_from.is_some().then_some(Adopting);
    let found = adopt_from.map(|(f, busy)| adopt(&f, busy.as_deref())).unwrap_or_default();
    let space = space_of.and_then(|read| read());
    drop(guard);
    let gone = {
        let mut st = lock();
        // (set again meanwhile: that setting's call does the work)
        if st.epoch != epoch {
            return;
        }
        for (original, copy) in found {
            // (a copy made meanwhile is the current one: never counted twice)
            if st.copies.contains_key(&original) {
                continue;
            }
            st.clock += 1;
            st.bytes = st.bytes.saturating_add(copy.len);
            let last_use = st.clock;
            st.copies.insert(original, Copy { last_use, ..copy });
        }
        if let Some(space) = space {
            st.space = Some((space, std::time::Instant::now()));
        }
        let max = limit(&mut st);
        evict_to(&mut st, max)
    };
    for local in gone {
        remove_copy(&local);
    }
}

/// The copies' record beside a copy.
fn record_path(local: &Path) -> PathBuf {
    let mut p = local.as_os_str().to_owned();
    p.push(".src");
    PathBuf::from(p)
}

/// A record's text: the source's path, size and modification time (ns, or `-`), the copy's size.
fn record(original: &Path, source: &FileStamp, len: u64) -> Option<String> {
    let FileStamp::Native { len: slen, modified } = source else { return None };
    let modified = modified.map_or("-".to_string(), |m| m.to_string());
    Some(format!("{}\n{slen}\n{modified}\n{len}\n", original.to_string_lossy()))
}

fn parse_record(text: &str) -> Option<(PathBuf, FileStamp, u64)> {
    let mut lines = text.lines();
    let original = PathBuf::from(lines.next()?);
    let slen = lines.next()?.parse().ok()?;
    let modified = match lines.next()? {
        "-" => None,
        m => Some(m.parse().ok()?),
    };
    let len = lines.next()?.parse().ok()?;
    Some((original, FileStamp::Native { len: slen, modified }, len))
}

/// Is `name` one of the localizer's own files (`<16 hex>_…`).
fn ours(name: &str) -> bool {
    name.len() > 17 && name.as_bytes().get(16) == Some(&b'_') && name.get(..16).is_some_and(|h| h.bytes().all(|b| b.is_ascii_hexdigit()))
}

/// Ends an adoption ([`State::adopting`]) when dropped.
struct Adopting;

impl Drop for Adopting {
    fn drop(&mut self) {
        let mut st = lock();
        st.adopting = st.adopting.saturating_sub(1);
        state().1.notify_all();
    }
}

/// The copies `folder` holds from an earlier session, by source; temporaries, records without
/// their copy and copies without a record are deleted (a copy renamed into place whose record
/// was never written, #3301 review: nothing else would ever delete it, and the size limit
/// didn't count it). Only the localizer's own files: other files in the folder are left alone,
/// and those of `busy`, the copy being made now. Runs without the localizer's lock.
fn adopt(folder: &Path, busy: Option<&str>) -> Vec<(PathBuf, Copy)> {
    #[cfg(test)]
    {
        let hook = BEFORE_ADOPT.lock().unwrap_or_else(PoisonError::into_inner).clone();
        if let Some(h) = hook {
            h(folder);
        }
    }
    let mut found = vec![];
    let Ok(dir) = std::fs::read_dir(folder) else { return found };
    let mut records = vec![];
    let mut copies = vec![];
    for e in dir.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        if !ours(&name) || busy.is_some_and(|b| name.starts_with(b)) {
            continue;
        }
        if name.ends_with(".part") {
            let _ = std::fs::remove_file(e.path());
        } else if name.ends_with(".src") {
            records.push(e.path());
        } else if e.file_type().is_ok_and(|t| t.is_file()) {
            copies.push(e.path());
        }
    }
    let recorded: std::collections::HashSet<PathBuf> = records.iter().map(|r| r.with_extension("")).collect();
    for copy in copies.iter().filter(|c| !recorded.contains(*c)) {
        let _ = std::fs::remove_file(copy);
    }
    for rec in records {
        let local = rec.with_extension("");
        let parsed = std::fs::read_to_string(&rec).ok().and_then(|t| parse_record(&t));
        let fine = parsed.as_ref().is_some_and(|(_, _, len)| std::fs::metadata(&local).is_ok_and(|m| m.is_file() && m.len() == *len));
        match parsed {
            Some((original, source, len)) if fine => found.push((original, Copy { local, len, source, last_use: 0 })),
            _ => {
                let _ = std::fs::remove_file(&rec);
                let _ = std::fs::remove_file(&local);
            }
        }
    }
    found
}

/// What the localizer is doing; `None` when it is off.
pub fn status() -> Option<LocalizeReport> {
    let st = lock();
    let c = st.config.as_ref()?;
    Some(LocalizeReport {
        files: st.copies.len(),
        bytes: st.bytes,
        max_bytes: c.max_bytes,
        queued: st.queue.len(),
        copying: st.copying.as_ref().map(|(p, _)| p.to_string_lossy().to_string()),
        skipped: st.skipped.len(),
        network: c.network.iter().map(|p| p.to_string_lossy().to_string()).collect(),
        reserve_limited: st.reserve_limited,
    })
}

/// Paths under these roots count as network footage (tests: a local folder standing in for a
/// share).
#[cfg(test)]
static NETWORK_ROOTS: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());

#[cfg(test)]
pub(crate) fn treat_as_network(root: &Path) {
    NETWORK_ROOTS.lock().unwrap_or_else(PoisonError::into_inner).push(root.to_path_buf());
}

/// `path` lies under `prefix` (separators and, on Windows, letter case don't matter).
fn under(path: &Path, prefix: &Path) -> bool {
    let norm = |p: &Path| {
        let s = p.to_string_lossy().replace('\\', "/");
        if cfg!(windows) { s.to_lowercase() } else { s }
    };
    let (p, pre) = (norm(path), norm(prefix));
    let pre = pre.trim_end_matches('/');
    !pre.is_empty() && (p == pre || p.starts_with(&format!("{pre}/")))
}

/// Is `path` network footage: a UNC path (`\\server\share\…`, `//server/share/…`), or under one
/// of the locations the localizer is given (`P:\`).
pub(crate) fn is_network(path: &Path) -> bool {
    #[cfg(test)]
    if NETWORK_ROOTS.lock().unwrap_or_else(PoisonError::into_inner).iter().any(|r| path.starts_with(r)) {
        return true;
    }
    let s = path.to_string_lossy();
    if s.starts_with("\\\\") || s.starts_with("//") {
        return true;
    }
    lock().config.as_ref().is_some_and(|c| c.network.iter().any(|n| under(path, n)))
}

/// Ask for a local copy of the network file `original` (nothing when the localizer is off, the
/// file is local, already copied, queued, being copied under this setting, or skipped).
/// Returns at once.
pub(crate) fn request(original: &Path) {
    if !is_network(original) {
        return;
    }
    let mut st = lock();
    let copying = st.copying.as_ref().is_some_and(|(p, epoch)| p == original && *epoch == st.epoch);
    if st.config.is_none() || copying || st.copies.contains_key(original) || st.queued.contains(original) || st.skipped.contains(original) {
        return;
    }
    if st.queue.len() >= QUEUE_LIMIT
        && let Some(old) = st.queue.pop_front()
    {
        st.queued.remove(&old);
    }
    st.queue.push_back(original.to_path_buf());
    st.queued.insert(original.to_path_buf());
    start_worker(&mut st);
    state().1.notify_all();
}

/// The complete local copy of `original` when its source still has identity `now` (its size and
/// modification time when it was copied) and the copy its size; `None` otherwise (read the
/// source). A copy whose source changed is forgotten (and copied again when asked for).
pub(crate) fn local_copy(original: &Path, now: &FileStamp) -> Option<PathBuf> {
    let (local, len, source) = {
        let st = lock();
        st.config.as_ref()?;
        let c = st.copies.get(original)?;
        (c.local.clone(), c.len, c.source)
    };
    // (the copy is looked at without the lock)
    let ok = source == *now && std::fs::metadata(&local).is_ok_and(|m| m.is_file() && m.len() == len);
    let mut st = lock();
    st.clock += 1;
    let clock = st.clock;
    // (the same copy as looked at: it may have been replaced or dropped meanwhile)
    let same = st.copies.get(original).is_some_and(|c| c.local == local && c.len == len && c.source == source);
    if !same {
        return None;
    }
    if ok {
        if let Some(c) = st.copies.get_mut(original) {
            c.last_use = clock;
        }
        return Some(local);
    }
    let gone = st.copies.remove(original);
    if let Some(g) = &gone {
        st.bytes = st.bytes.saturating_sub(g.len);
    }
    drop(st);
    if let Some(g) = gone {
        remove_copy(&g.local);
    }
    None
}

fn remove_copy(local: &Path) {
    let _ = std::fs::remove_file(local);
    let _ = std::fs::remove_file(record_path(local));
}

#[cfg(not(target_arch = "wasm32"))]
fn start_worker(st: &mut State) {
    if st.worker {
        return;
    }
    st.worker = std::thread::Builder::new().name("ec-localize".into()).spawn(worker).is_ok();
}

/// (no threads in the browser: nothing is copied there)
#[cfg(target_arch = "wasm32")]
fn start_worker(_st: &mut State) {}

/// The most the copies may take now: the setting, or less when free disk space holds them
/// below it (the disk cache's reserve stays free on their disk; free space is read at most
/// every 30 s).
fn limit(st: &mut State) -> u64 {
    let Some(c) = st.config.clone() else { return 0 };
    let fresh = st.space.is_some_and(|(_, at)| at.elapsed() < std::time::Duration::from_secs(30));
    if !fresh && let Some(read) = &c.disk_space {
        st.space = read().map(|s| (s, std::time::Instant::now()));
    }
    let cap = match st.space {
        Some(((total, available), _)) if total > 0 => st.bytes.saturating_add(available).saturating_sub(effectcraft_render::disk_cache::disk_reserve(total)),
        _ => u64::MAX,
    };
    st.reserve_limited = cap < c.max_bytes;
    c.max_bytes.min(cap)
}

/// Drop the least recently used copies until they take at most `max` bytes; their files, which
/// the caller deletes once it let go of the lock ([`remove_copy`]).
#[must_use]
fn evict_to(st: &mut State, max: u64) -> Vec<PathBuf> {
    let mut gone = vec![];
    while st.bytes > max {
        let Some(oldest) = st.copies.iter().min_by_key(|(_, c)| c.last_use).map(|(p, _)| p.clone()) else { break };
        if let Some(c) = st.copies.remove(&oldest) {
            st.bytes = st.bytes.saturating_sub(c.len);
            gone.push(c.local);
        }
    }
    gone
}

/// The copies' folder name of `original`: a hash of its path, then its file name.
fn local_name(original: &Path) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    original.hash(&mut h);
    let name = original.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    format!("{:016x}_{name}", h.finish())
}

/// The copying thread: one file at a time, until the queue is empty for a while.
#[cfg(not(target_arch = "wasm32"))]
fn worker() {
    let (m, cv) = state();
    loop {
        let (job, config, epoch, room) = {
            let mut st = m.lock().unwrap_or_else(PoisonError::into_inner);
            loop {
                if st.adopting == 0
                    && let (Some(p), Some(c)) = (st.queue.front().cloned(), st.config.clone())
                {
                    st.queue.pop_front();
                    st.queued.remove(&p);
                    st.copying = Some((p.clone(), st.epoch));
                    let room = limit(&mut st);
                    break (p, c, st.epoch, room);
                }
                let (g, timeout) = match cv.wait_timeout(st, std::time::Duration::from_secs(30)) {
                    Ok(r) => r,
                    Err(p) => p.into_inner(),
                };
                st = g;
                if timeout.timed_out() && st.queue.is_empty() {
                    st.worker = false;
                    return;
                }
            }
        };
        let copied = copy_file(&job, &config.folder, room);
        let mut st = m.lock().unwrap_or_else(PoisonError::into_inner);
        st.copying = None;
        let mut gone = vec![];
        match copied {
            Some((local, len, source)) if st.epoch == epoch => {
                if let Some(old) = st.copies.remove(&job) {
                    st.bytes = st.bytes.saturating_sub(old.len);
                }
                st.clock += 1;
                let last_use = st.clock;
                st.copies.insert(job, Copy { local, len, source, last_use });
                st.bytes = st.bytes.saturating_add(len);
                let max = limit(&mut st);
                gone = evict_to(&mut st, max);
            }
            // (made under an earlier setting: thrown away, unless the folder's adoption took it
            // as the current copy meanwhile)
            Some((local, _, _)) => {
                if !st.copies.get(&job).is_some_and(|c| c.local == local) {
                    gone.push(local);
                }
            }
            // (only under the setting it was asked for under: a new one asks again)
            None if st.epoch == epoch => {
                st.skipped.insert(job);
            }
            None => {}
        }
        drop(st);
        for local in gone {
            remove_copy(&local);
        }
    }
}

/// Copy `original` into `folder`: under a temporary name; then an earlier copy of it removed,
/// its record written beside the final name, and the temporary renamed when complete (a crash
/// in between leaves a record without its copy or a temporary, both deleted when the folder is
/// adopted; never a copy without a record, nor old bytes beside a new record); its
/// modification time set to the source's. The copy, its size and the
/// source's identity, or `None` (larger than `room`, unreadable, or changed while it was
/// copied).
#[cfg(not(target_arch = "wasm32"))]
fn copy_file(original: &Path, folder: &Path, room: u64) -> Option<(PathBuf, u64, FileStamp)> {
    #[cfg(test)]
    {
        let hook = BEFORE_COPY.lock().unwrap_or_else(PoisonError::into_inner).clone();
        if let Some(h) = hook {
            h(original);
        }
    }
    let meta = std::fs::metadata(original).ok().filter(std::fs::Metadata::is_file)?;
    if meta.len() > room {
        return None;
    }
    let before = FileStamp::of_metadata(&meta);
    std::fs::create_dir_all(folder).ok()?;
    let local = folder.join(local_name(original));
    let tmp = folder.join(format!("{}.part", local_name(original)));
    let copied = (|| -> std::io::Result<u64> {
        let mut from = std::fs::File::open(original)?;
        let mut to = std::fs::File::create(&tmp)?;
        let n = std::io::copy(&mut from, &mut to)?;
        to.sync_all()?;
        if let Ok(t) = meta.modified() {
            to.set_modified(t)?;
        }
        Ok(n)
    })();
    let after = std::fs::metadata(original).ok().map(|m| FileStamp::of_metadata(&m));
    let text = record(original, &before, meta.len());
    match (copied, text) {
        (Ok(n), Some(text)) if n == meta.len() && after.as_ref() == Some(&before) => {
            // An earlier copy of the source goes first (its record too): a new record must never
            // sit beside old bytes. One that can't be deleted (open elsewhere) stays, with its
            // own record, and this copy is given up.
            if local.exists() && std::fs::remove_file(&local).is_err() {
                let _ = std::fs::remove_file(&tmp);
                return None;
            }
            let _ = std::fs::remove_file(record_path(&local));
            if std::fs::write(record_path(&local), text).is_err() || std::fs::rename(&tmp, &local).is_err() {
                let _ = std::fs::remove_file(&tmp);
                remove_copy(&local);
                return None;
            }
            Some((local, n, before))
        }
        _ => {
            let _ = std::fs::remove_file(&tmp);
            None
        }
    }
}

/// Runs before each copy, with the source's path (tests).
#[cfg(test)]
type CopyHook = std::sync::Arc<dyn Fn(&Path) + Send + Sync>;
#[cfg(test)]
static BEFORE_COPY: Mutex<Option<CopyHook>> = Mutex::new(None);
/// Runs before a folder is adopted, with the folder (tests).
#[cfg(test)]
static BEFORE_ADOPT: Mutex<Option<CopyHook>> = Mutex::new(None);

/// The localizer state is process-wide: tests that use it take turns.
#[cfg(test)]
pub(crate) static TEST_SERIAL: Mutex<()> = Mutex::new(());

#[cfg(test)]
mod tests {
    use super::*;

    fn config(folder: &Path, max_bytes: u64, network: Vec<PathBuf>) -> Option<LocalizeConfig> {
        Some(LocalizeConfig { folder: folder.to_path_buf(), max_bytes, network, disk_space: None })
    }

    fn wait_for(f: impl Fn(&LocalizeReport) -> bool) -> LocalizeReport {
        let until = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            let s = status().unwrap_or_default();
            if f(&s) || std::time::Instant::now() > until {
                return s;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    /// A network file is copied in the background and read locally only while it matches its
    /// source; a source that changed is read from the share again; files over the limit are not
    /// copied; local files are never queued; a listed location counts as network; the folder's
    /// copies are adopted after a restart (stale temporaries deleted).
    #[test]
    fn network_files_are_copied_and_used_only_while_they_match() {
        let _serial = TEST_SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
        let root = std::env::temp_dir().join(format!("ec-3271-localize-{}", std::process::id()));
        let (share, local, mapped) = (root.join("share"), root.join("local"), root.join("mapped"));
        std::fs::create_dir_all(&share).unwrap();
        std::fs::create_dir_all(&mapped).unwrap();
        treat_as_network(&share);
        let file = share.join("plate_0001.exr");
        std::fs::write(&file, vec![7u8; 4096]).unwrap();
        let stamp = |p: &Path| FileStamp::of_metadata(&std::fs::metadata(p).unwrap());
        configure(config(&local, 1 << 20, vec![mapped.clone()]));
        assert!(is_network(&mapped.join("x.exr")), "a listed location");
        assert!(local_copy(&file, &stamp(&file)).is_none(), "not copied yet");
        request(&file);
        let s = wait_for(|s| s.files == 1);
        assert_eq!((s.files, s.bytes, s.queued), (1, 4096, 0), "{s:?}");
        let copy = local_copy(&file, &stamp(&file)).expect("a verified copy");
        assert!(copy.starts_with(&local));
        assert_eq!(std::fs::read(&copy).unwrap(), vec![7u8; 4096]);
        // A restart: the copy is adopted, a stale temporary deleted.
        let stale = local.join("0123456789abcdef_old.exr.part");
        std::fs::write(&stale, b"x").unwrap();
        configure(None);
        configure(config(&local, 1 << 20, vec![]));
        assert_eq!(status().unwrap().files, 1, "adopted");
        assert!(!stale.exists(), "temporaries deleted");
        assert!(local_copy(&file, &stamp(&file)).is_some());
        // The source changes: the copy is not read any more.
        std::fs::write(&file, vec![8u8; 5000]).unwrap();
        assert!(local_copy(&file, &stamp(&file)).is_none());
        // Over the limit: skipped.
        let big = share.join("big.exr");
        std::fs::write(&big, vec![1u8; 4096]).unwrap();
        configure(config(&local, 1000, vec![]));
        request(&big);
        let s = wait_for(|s| s.skipped == 1);
        assert_eq!((s.skipped, s.files), (1, 0), "{s:?}");
        // A local file is never queued.
        request(&root.join("elsewhere.exr"));
        assert_eq!(status().unwrap().queued, 0);
        configure(None);
        assert!(status().is_none());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// #3271 review: a file asked for again while it is being copied is not copied a second
    /// time, but asked for after the setting changed it is (the copy under the old setting is
    /// thrown away).
    #[test]
    fn a_file_being_copied_is_not_queued_again() {
        let _serial = TEST_SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
        // (dropped before `_serial`, also when an assertion fails)
        let _hook = HookGuard;
        let root = std::env::temp_dir().join(format!("ec-3271-localize-dup-{}", std::process::id()));
        let (share, local) = (root.join("share"), root.join("local"));
        std::fs::create_dir_all(&share).unwrap();
        treat_as_network(&share);
        let file = share.join("plate_0002.exr");
        std::fs::write(&file, vec![3u8; 2048]).unwrap();
        // Each copy waits until the test lets it go on.
        let copies = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let (go, wait) = std::sync::mpsc::channel::<()>();
        {
            let (copies, wait) = (copies.clone(), Mutex::new(wait));
            *BEFORE_COPY.lock().unwrap() = Some(std::sync::Arc::new(move |_: &Path| {
                copies.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let _ = wait.lock().unwrap().recv_timeout(std::time::Duration::from_secs(10));
            }));
        }
        let copies_now = || copies.load(std::sync::atomic::Ordering::SeqCst);
        configure(config(&local, 1 << 20, vec![]));
        request(&file);
        wait_for(|s| s.copying.is_some());
        request(&file);
        assert_eq!(status().unwrap().queued, 0, "not queued while it is copied");
        go.send(()).unwrap();
        wait_for(|s| s.files == 1 && s.copying.is_none());
        assert_eq!(copies_now(), 1, "copied once");
        // A new setting while a copy runs: asked for again, it is copied again.
        let file2 = share.join("plate_0003.exr");
        std::fs::write(&file2, vec![4u8; 1024]).unwrap();
        request(&file2);
        wait_for(|s| s.copying.is_some());
        configure(config(&local, 2 << 20, vec![]));
        request(&file2);
        assert_eq!(status().unwrap().queued, 1, "queued again under the new setting");
        go.send(()).unwrap();
        go.send(()).unwrap();
        let s = wait_for(|s| s.files == 2 && s.queued == 0 && s.copying.is_none());
        assert_eq!(s.files, 2, "{s:?}");
        assert_eq!(copies_now(), 3);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Takes the copy hook out and turns the localizer off when dropped (also when the test
    /// fails), so the next test doesn't inherit a hook that holds its copies.
    struct HookGuard;

    impl Drop for HookGuard {
        fn drop(&mut self) {
            *BEFORE_COPY.lock().unwrap_or_else(PoisonError::into_inner) = None;
            *BEFORE_ADOPT.lock().unwrap_or_else(PoisonError::into_inner) = None;
            configure(None);
        }
    }

    /// #3271 review: while a new folder's copies are adopted (outside the lock), the worker
    /// starts no copy: the scan can't delete its temporary or count a copy it makes twice.
    #[test]
    fn no_copy_starts_while_a_folder_is_adopted() {
        let _serial = TEST_SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
        let _hooks = HookGuard;
        let root = std::env::temp_dir().join(format!("ec-3271-localize-adopt-{}", std::process::id()));
        let (share, first, second) = (root.join("share"), root.join("first"), root.join("second"));
        std::fs::create_dir_all(&share).unwrap();
        treat_as_network(&share);
        let file = share.join("plate_0004.exr");
        std::fs::write(&file, vec![5u8; 1500]).unwrap();
        configure(config(&first, 1 << 20, vec![]));
        // The next folder's scan waits until the test lets it go on.
        let (entered, scanning) = std::sync::mpsc::channel::<()>();
        let (go, wait) = std::sync::mpsc::channel::<()>();
        {
            let (entered, wait) = (Mutex::new(entered), Mutex::new(wait));
            *BEFORE_ADOPT.lock().unwrap() = Some(std::sync::Arc::new(move |_: &Path| {
                let _ = entered.lock().unwrap().send(());
                let _ = wait.lock().unwrap().recv_timeout(std::time::Duration::from_secs(10));
            }));
        }
        let second_cfg = config(&second, 1 << 20, vec![]);
        let switching = std::thread::spawn(move || configure(second_cfg));
        scanning.recv_timeout(std::time::Duration::from_secs(10)).unwrap();
        request(&file);
        std::thread::sleep(std::time::Duration::from_millis(200));
        let s = status().unwrap();
        assert!(s.copying.is_none() && s.files == 0, "no copy while adopting: {s:?}");
        go.send(()).unwrap();
        switching.join().unwrap();
        let s = wait_for(|s| s.files == 1 && s.copying.is_none());
        assert_eq!((s.files, s.bytes), (1, 1500), "copied once, counted once: {s:?}");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// #3301 review: a copy renamed into place whose record was never written (a crash in
    /// between) is deleted when the folder is adopted: nothing else would delete it, and the
    /// size limit didn't count it. Other files in the folder and the copy being made stay.
    #[test]
    fn a_copy_without_its_record_is_deleted_when_adopted() {
        let _serial = TEST_SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
        let folder = std::env::temp_dir().join(format!("ec-3301-orphan-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        std::fs::create_dir_all(&folder).unwrap();
        let orphan = folder.join("0123456789abcdef_plate.exr");
        let busy = folder.join("fedcba9876543210_busy.exr");
        let other = folder.join("notes.txt");
        for f in [&orphan, &busy, &other] {
            std::fs::write(f, b"pixels").unwrap();
        }
        // A crash between writing a copy's record and renaming its temporary into place: both go.
        let mid = folder.join("abcdef0123456789_mid.exr");
        let (mid_rec, mid_part) = (record_path(&mid), folder.join("abcdef0123456789_mid.exr.part"));
        std::fs::write(&mid_rec, "/share/mid.exr\n6\n-\n6\n").unwrap();
        std::fs::write(&mid_part, b"pixels").unwrap();
        assert!(adopt(&folder, Some("fedcba9876543210_busy")).is_empty());
        assert!(!mid_rec.exists() && !mid_part.exists(), "a record without its copy and a temporary go");
        assert!(!orphan.exists(), "the copy without a record went");
        assert!(busy.exists(), "the copy being made stays");
        assert!(other.exists(), "not the localizer's: left alone");
        let _ = std::fs::remove_dir_all(&folder);
    }
}
