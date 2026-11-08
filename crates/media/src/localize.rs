//! The background footage localizer (#3271, Settings ▸ Disk ▸ Localize Network Footage, off by
//! default): footage on a network share is copied, file by file, to a local folder while the
//! user works, so later reads come from the local disk.
//!
//! - **Never on the playback path.** A read asks for a copy ([`request`]) and goes on reading
//!   the share; one background thread copies, one file at a time.
//! - **Used only when complete and verified.** A copy is made under a temporary name, renamed
//!   when complete, and its size and the source's identity (size and modification time) as they
//!   were when it was copied are recorded. It is read only while the source still has that
//!   identity and the copy that size ([`local_copy`]); a source that changed is read from the
//!   share (and copied again).
//! - **Readers keep naming the source.** Entries made of a localized file list the share's path
//!   and identity (see `ExrSource::Localized`), so caches are keyed the same either way.
//! - **Bounded.** The copies take at most the size limit; the least recently used go first.
//! - **Visible.** [`status`] says how many files are local, their size, what is queued and what
//!   is being copied.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Condvar, Mutex, PoisonError};

use effectcraft_render::reads::FileStamp;

/// Where copies go and how much they may take.
#[derive(Clone, Debug, PartialEq)]
pub struct LocalizeSettings {
    pub folder: PathBuf,
    pub max_bytes: u64,
}

/// What the localizer is doing (Settings ▸ Disk shows it).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LocalizeStatus {
    /// Complete local copies and their bytes.
    pub files: usize,
    pub bytes: u64,
    pub max_bytes: u64,
    /// Files waiting to be copied.
    pub queued: usize,
    /// The file being copied now.
    pub copying: Option<String>,
    /// Files that were too large for the limit, or failed to copy.
    pub skipped: usize,
}

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
    settings: Option<LocalizeSettings>,
    copies: HashMap<PathBuf, Copy>,
    queue: VecDeque<PathBuf>,
    queued: HashSet<PathBuf>,
    bytes: u64,
    clock: u64,
    copying: Option<PathBuf>,
    skipped: HashSet<PathBuf>,
    worker: bool,
    /// Bumped with each new setting: a copy started under another one is thrown away.
    epoch: u64,
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

/// Turn the localizer on with `settings`, or off (`None`): the copies made so far stay on disk
/// but are no longer read; a new folder starts empty.
pub fn configure(settings: Option<LocalizeSettings>) {
    let mut st = lock();
    if st.settings == settings {
        return;
    }
    st.epoch = st.epoch.wrapping_add(1);
    if st.settings.as_ref().map(|s| &s.folder) != settings.as_ref().map(|s| &s.folder) {
        st.copies.clear();
        st.bytes = 0;
    }
    st.queue.clear();
    st.queued.clear();
    st.skipped.clear();
    st.settings = settings;
    if let Some(max) = st.settings.as_ref().map(|s| s.max_bytes) {
        evict_to(&mut st, max);
    }
}

/// What the localizer is doing; `None` when it is off.
pub fn status() -> Option<LocalizeStatus> {
    let st = lock();
    let s = st.settings.as_ref()?;
    Some(LocalizeStatus {
        files: st.copies.len(),
        bytes: st.bytes,
        max_bytes: s.max_bytes,
        queued: st.queue.len(),
        copying: st.copying.as_ref().map(|p| p.to_string_lossy().to_string()),
        skipped: st.skipped.len(),
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

/// Is `path` on a network share: a UNC path (`\\server\share\…`, `//server/share/…`). Drive
/// letters mapped to a share are not told apart from local disks.
pub(crate) fn is_network(path: &Path) -> bool {
    #[cfg(test)]
    if NETWORK_ROOTS.lock().unwrap_or_else(PoisonError::into_inner).iter().any(|r| path.starts_with(r)) {
        return true;
    }
    let s = path.to_string_lossy();
    s.starts_with("\\\\") || s.starts_with("//")
}

/// Ask for a local copy of the network file `original` (nothing when the localizer is off, the
/// file is local, already copied, queued or skipped). Returns at once.
pub(crate) fn request(original: &Path) {
    if !is_network(original) {
        return;
    }
    let mut st = lock();
    if st.settings.is_none() || st.copies.contains_key(original) || st.queued.contains(original) || st.skipped.contains(original) {
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
    let mut st = lock();
    st.settings.as_ref()?;
    st.clock += 1;
    let clock = st.clock;
    let c = st.copies.get_mut(original)?;
    let ok = c.source == *now && std::fs::metadata(&c.local).is_ok_and(|m| m.is_file() && m.len() == c.len);
    if ok {
        c.last_use = clock;
        return Some(c.local.clone());
    }
    if let Some(gone) = st.copies.remove(original) {
        st.bytes = st.bytes.saturating_sub(gone.len);
        let _ = std::fs::remove_file(&gone.local);
    }
    None
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

/// Drop the least recently used copies until they take at most `max` bytes.
fn evict_to(st: &mut State, max: u64) {
    while st.bytes > max {
        let Some(oldest) = st.copies.iter().min_by_key(|(_, c)| c.last_use).map(|(p, _)| p.clone()) else { break };
        if let Some(c) = st.copies.remove(&oldest) {
            st.bytes = st.bytes.saturating_sub(c.len);
            let _ = std::fs::remove_file(&c.local);
        }
    }
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
        let (job, settings, epoch) = {
            let mut st = m.lock().unwrap_or_else(PoisonError::into_inner);
            loop {
                if let (Some(p), Some(s)) = (st.queue.front().cloned(), st.settings.clone()) {
                    st.queue.pop_front();
                    st.queued.remove(&p);
                    st.copying = Some(p.clone());
                    break (p, s, st.epoch);
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
        let copied = copy_file(&job, &settings);
        let mut st = m.lock().unwrap_or_else(PoisonError::into_inner);
        st.copying = None;
        match copied {
            Some((local, len, source)) if st.epoch == epoch => {
                if let Some(old) = st.copies.remove(&job) {
                    st.bytes = st.bytes.saturating_sub(old.len);
                }
                st.clock += 1;
                let last_use = st.clock;
                st.copies.insert(job, Copy { local, len, source, last_use });
                st.bytes = st.bytes.saturating_add(len);
                evict_to(&mut st, settings.max_bytes);
            }
            Some((local, _, _)) => {
                let _ = std::fs::remove_file(local);
            }
            None => {
                st.skipped.insert(job);
            }
        }
    }
}

/// Copy `original` into the folder of `settings`: under a temporary name, then renamed when
/// complete; its modification time set to the source's. The copy, its size and the source's
/// identity, or `None` (too large for the limit, unreadable, or changed while it was copied).
#[cfg(not(target_arch = "wasm32"))]
fn copy_file(original: &Path, settings: &LocalizeSettings) -> Option<(PathBuf, u64, FileStamp)> {
    let meta = std::fs::metadata(original).ok().filter(std::fs::Metadata::is_file)?;
    if meta.len() > settings.max_bytes {
        return None;
    }
    let before = FileStamp::of_metadata(&meta);
    std::fs::create_dir_all(&settings.folder).ok()?;
    let local = settings.folder.join(local_name(original));
    let tmp = settings.folder.join(format!("{}.part", local_name(original)));
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
    match copied {
        Ok(n) if n == meta.len() && after.as_ref() == Some(&before) && std::fs::rename(&tmp, &local).is_ok() => Some((local, n, before)),
        _ => {
            let _ = std::fs::remove_file(&tmp);
            None
        }
    }
}

/// The localizer state is process-wide: tests that use it take turns.
#[cfg(test)]
pub(crate) static TEST_SERIAL: Mutex<()> = Mutex::new(());

#[cfg(test)]
mod tests {
    use super::*;

    fn wait_for(f: impl Fn(&LocalizeStatus) -> bool) -> LocalizeStatus {
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
    /// copied; local files are never queued.
    #[test]
    fn network_files_are_copied_and_used_only_while_they_match() {
        let _serial = TEST_SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
        let root = std::env::temp_dir().join(format!("ec-3271-localize-{}", std::process::id()));
        let (share, local) = (root.join("share"), root.join("local"));
        std::fs::create_dir_all(&share).unwrap();
        treat_as_network(&share);
        let file = share.join("plate_0001.exr");
        std::fs::write(&file, vec![7u8; 4096]).unwrap();
        let stamp = |p: &Path| FileStamp::of_metadata(&std::fs::metadata(p).unwrap());
        configure(Some(LocalizeSettings { folder: local.clone(), max_bytes: 1 << 20 }));
        assert!(local_copy(&file, &stamp(&file)).is_none(), "not copied yet");
        request(&file);
        let s = wait_for(|s| s.files == 1);
        assert_eq!((s.files, s.bytes, s.queued), (1, 4096, 0), "{s:?}");
        let copy = local_copy(&file, &stamp(&file)).expect("a verified copy");
        assert!(copy.starts_with(&local));
        assert_eq!(std::fs::read(&copy).unwrap(), vec![7u8; 4096]);
        // The source changes: the copy is not read any more.
        std::fs::write(&file, vec![8u8; 5000]).unwrap();
        assert!(local_copy(&file, &stamp(&file)).is_none());
        // Over the limit: skipped.
        let big = share.join("big.exr");
        std::fs::write(&big, vec![1u8; 4096]).unwrap();
        configure(Some(LocalizeSettings { folder: local.clone(), max_bytes: 1000 }));
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
}
