//! Footage that changed on disk: File ▸ Reload Footage (Cmd+Alt+L) and Settings ▸ Import ▸
//! Reload Footage Changed on Disk.
//!
//! Reloading re-reads what the file decides (its size, a movie's duration, streams and frame
//! rate, the codec, a data file's text) and keeps what the project decided: the Photoshop layer
//! or PDF page an item shows and its interpretation (alpha, conformed frame rate, pixel aspect,
//! fields, colour). A Photoshop layer is found again by name when layers were added or removed
//! around it. The media layer then forgets the file and the layer cache is emptied, so the next
//! render reads the new pixels; File ▸ Reload Footage also deletes the disk cache entries made of
//! the file (#3184).
//!
//! The automatic part compares each footage file's size and modification time with what they
//! were when the file was last seen. The desktop app keeps a [`Watcher`]: a worker thread that
//! stamps the files every two seconds while the window is in front (every ten seconds while it
//! is not, and at once when it comes back to the front) and wakes the interface only when a stamp
//! changed, so a file on a slow network share never holds up the interface and an idle app never
//! redraws for footage that didn't change; `footage.reloadChanged` takes the stamps on the spot
//! (CLI, agents, tests). Like the footage check after File ▸ Open, an automatic reload records
//! the state of the disk: it is not an undo step and doesn't modify the project.

use std::collections::BTreeSet;

use super::availability::{self, Source};
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use effectcraft_project::{Footage, FootageKind, ItemId, ItemKind, SourceLayer};
use serde_json::{Value, json};

use crate::{Services, Session};

/// A file's size and modification time (see [`crate::Services::stamp`]).
pub type Stamp = (u64, u128);

/// The stamps of a set of files, in path order.
pub type Stamps = Vec<(String, Option<Stamp>)>;

/// The files footage items read: each item's file and every frame of an image sequence. Image
/// sequences count only with Automatically Reload Footage set to All Footage (After Effects'
/// default, Non-Sequence Footage, leaves them alone: thousands of frames on a share add up).
/// Items with a stored path that is not followed ([`crate::doc_path`]) are not watched.
fn reload_paths(s: &Session) -> BTreeSet<String> {
    let sequences = s.prefs.import.auto_reload_footage == "all";
    let mut paths = BTreeSet::new();
    for it in s.project.items.values() {
        if let ItemKind::Footage(f) = &it.kind {
            if (!f.sequence.is_empty() && !sequences) || s.stored_paths.check(f).is_err() {
                continue;
            }
            if !f.path.is_empty() {
                paths.insert(f.path.clone());
            }
            paths.extend(f.sequence.iter().cloned());
        }
    }
    paths
}

/// Automatic reload obeys Off; explicit Reload Changed still uses eligible non-sequence files.
fn automatic_paths(s: &Session) -> BTreeSet<String> {
    if s.prefs.import.auto_reload_footage == "off" { BTreeSet::new() } else { reload_paths(s) }
}

/// Embedded data is excluded from availability warnings, but its external file can still be
/// reloaded. Compare these paths without cloning sequences during unrelated project edits.
fn data_paths(project: &effectcraft_project::Project) -> impl Iterator<Item = (ItemId, &str, &[String])> {
    project.items.values().filter_map(|item| match &item.kind {
        ItemKind::Footage(f) if f.kind == FootageKind::Data => Some((item.id, f.path.as_str(), f.sequence.as_slice())),
        _ => None,
    })
}

/// Files and source identities watched by the single background disk worker.
#[derive(Clone, Default)]
pub struct Watched {
    reload: BTreeSet<String>,
    sources: Arc<Vec<Source>>,
    project: Option<Arc<effectcraft_project::Project>>,
    reload_mode: String,
    stored_paths: crate::doc_path::StoredPaths,
}

impl PartialEq for Watched {
    /// Compare watched inputs, excluding unrelated project edits whose latest Arc is refreshed
    /// separately without waking the worker or restarting partial sequence scans.
    fn eq(&self, other: &Self) -> bool {
        self.stored_paths == other.stored_paths
            && self.reload_mode == other.reload_mode
            && self.reload == other.reload
            && (Arc::ptr_eq(&self.sources, &other.sources) || self.sources == other.sources)
    }
}

/// Capture the automatic reload policy and opted-in availability sources for a worker.
/// Watcher::update reuses this source inventory when only unrelated project state changed.
pub fn watched(s: &Session) -> Watched {
    Watched {
        reload: automatic_paths(s),
        sources: Arc::new(if s.check_footage_on_open { availability::sources(s) } else { Vec::new() }),
        project: Some(s.project.clone()),
        reload_mode: s.prefs.import.auto_reload_footage.clone(),
        stored_paths: s.stored_paths.clone(),
    }
}

impl Watched {
    /// A worker may sleep indefinitely only when neither content nor availability is watched.
    fn is_empty(&self) -> bool {
        self.reload.is_empty() && self.sources.is_empty()
    }
    /// Complete the synchronous pass used by explicit commands. Automatic availability uses
    /// Scanner::poll instead, so long sequences cannot consume an unbounded polling pass.
    fn scan(&self, services: &dyn Services) -> Observation {
        Observation { stamps: self.reload.iter().map(|p| (p.clone(), services.stamp(p))).collect(), missing: availability::scan(&self.sources, services) }
    }
}

#[derive(Clone, PartialEq)]
struct Observation {
    stamps: Stamps,
    missing: Vec<bool>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ReloadPolicy {
    Automatic,
    Manual,
}

/// A complete disk observation paired with the sources that were actually checked.
pub struct Scan {
    watched: Watched,
    observation: Observation,
    update: Option<availability::Update>,
    policy: ReloadPolicy,
}

impl Scan {
    /// Prepare the availability change before the session applies the observation.
    /// Automatic callers construct this result on the worker; explicit commands run inline.
    fn new(watched: Watched, observation: Observation, policy: ReloadPolicy) -> Self {
        let update = watched.project.as_ref().and_then(|p| availability::prepare(p, &watched.sources, &observation.missing));
        Self { watched, observation, update, policy }
    }

    /// Apply a current observation, returning `stale: true` when its project needs rechecking.
    /// Availability-only scans preserve manual reload stamps; eligible content changes still
    /// follow the existing reload/probe path and return its contextual errors.
    pub fn apply(self, s: &mut Session) -> crate::Result<Value> {
        if let Some(update) = self.update
            && !update.apply(s)
        {
            return Ok(json!({"files": 0, "reloaded": 0, "stale": true}));
        }
        // Availability-only scans must preserve the baseline used by the manual command.
        // A scan captured before automatic reload was disabled must not reload stale paths.
        if self.policy == ReloadPolicy::Automatic && (self.watched.reload_mode == "off" || s.prefs.import.auto_reload_footage == "off") {
            return Ok(json!({"files": 0, "reloaded": 0}));
        }
        let current = match self.policy {
            ReloadPolicy::Automatic => automatic_paths(s),
            ReloadPolicy::Manual => reload_paths(s),
        };
        let stamps = self.observation.stamps.into_iter().filter(|(p, _)| current.contains(p)).collect();
        apply_stamps(s, stamps)
    }
}

/// Size and position in the stack of Photoshop layer `l` in the file at `path` now: the layer
/// at its recorded index when the name still matches, else the first layer with its name.
fn psd_layer(s: &Session, path: &str, l: &SourceLayer) -> Option<(u32, Option<(u32, u32)>)> {
    let psd = effectcraft_psd::Psd::parse(s.services.read_file(path).ok()?).ok()?;
    let found = psd.layers.get(l.index as usize).filter(|x| x.name == l.name).or_else(|| psd.layers.iter().find(|x| x.name == l.name))?;
    let size = (!found.rect.is_empty()).then(|| (found.rect.width(), found.rect.height()));
    Some((u32::try_from(found.index).ok()?, size))
}

/// The pixel size of vector footage `f` as its file is now (its page, framing and layer), or
/// `None` when the file cannot be read or shows nothing.
fn vector_size(s: &Session, f: &Footage) -> Option<(u32, u32)> {
    let bytes = s.services.read_file(&f.path).ok()?;
    let doc = crate::vector::footage_doc(f, &bytes)?.ok()?;
    (!doc.root.children.is_empty()).then(|| doc.pixel_size())
}

/// Footage `f` as its file is now, given the importer's fresh look at the file (`probed`).
fn refreshed(s: &Session, f: &Footage, probed: Footage) -> Footage {
    let mut nf = f.clone();
    nf.missing = false;
    // An image sequence picks up frames added to (or removed from) its run; its frame rate and
    // order are the interpretation's.
    if f.kind == FootageKind::Sequence {
        if probed.kind == FootageKind::Sequence {
            (nf.sequence, nf.width, nf.height) = (probed.sequence, probed.width, probed.height);
            nf.sync_sequence_duration();
        }
        return nf;
    }
    nf.codec = probed.codec;
    nf.has_video = probed.has_video;
    nf.has_audio = probed.has_audio;
    // The markers the file carries now (a WAV's cue points), for Update Markers From Source.
    nf.markers = probed.markers;
    if matches!(f.kind, FootageKind::Video | FootageKind::Audio) {
        nf.duration = probed.duration;
        // Interpret Footage ▸ Conform to frame rate stays; the file's own rate is refreshed.
        match f.native_rate {
            Some(_) => nf.native_rate = Some(probed.native_rate.unwrap_or(probed.frame_rate)),
            None => nf.frame_rate = probed.frame_rate,
        }
    }
    if f.kind == FootageKind::Data {
        nf.data = probed.data;
    }
    let (mut w, mut h) = (probed.width, probed.height);
    match &f.layer {
        // A smart object's size is its embedded file's, recorded at import.
        Some(l) if l.embedded.is_some() => (w, h) = (f.width, f.height),
        Some(_) if crate::vector::is_vector(f) => (w, h) = vector_size(s, f).unwrap_or((f.width, f.height)),
        Some(l) => match psd_layer(s, &f.path, l) {
            Some((index, size)) => {
                if let Some(nl) = nf.layer.as_mut() {
                    nl.index = index;
                }
                // Retain Layer Sizes: the layer's own bounds; otherwise the document's.
                if l.layer_size {
                    (w, h) = size.unwrap_or((f.width, f.height));
                }
            }
            // The layer is gone from the file: keep the item as it was.
            None => (w, h) = (f.width, f.height),
        },
        // A placed page of an After Effects project keeps the size After Effects stored: the
        // layers' anchors and masks are measured against it, and the page is drawn to fill it
        // (the probe would measure the page as a viewer frames it, #3099).
        None if f.placed_page => (w, h) = (f.width, f.height),
        // A later page of a PDF: the probe measures the first page.
        None if f.page != 0 => (w, h) = (f.width, f.height),
        None => {}
    }
    (nf.width, nf.height) = (w, h);
    nf
}

/// Reload footage items `ids` from their files (File ▸ Reload Footage): an undo step; items
/// whose file can't be read are marked missing. Returns how many items were reloaded.
pub fn reload_items(s: &mut Session, ids: &[ItemId]) -> crate::Result<usize> {
    Ok(reload(s, ids, true)?.0)
}

/// Reload items `ids`. `undoable` makes it an edit; otherwise it records the disk without
/// modifying the project, and an item whose file can't be read now (another app is still
/// writing it) is left as it was. Returns the number reloaded and the files that couldn't be read
/// (only without `undoable`).
fn reload(s: &mut Session, ids: &[ItemId], undoable: bool) -> crate::Result<(usize, BTreeSet<String>)> {
    let Some(importer) = s.importer.clone() else {
        return Err(crate::EngineError::Other("media import is not available in this build".into()));
    };
    let mut updates = vec![];
    let mut paths = BTreeSet::new();
    let mut failed = BTreeSet::new();
    for i in ids {
        if let Some(ItemKind::Footage(f)) = s.project.item(*i).map(|x| &x.kind) {
            // A sequence is probed the way it imports by default, so its run is found again; any
            // other item is its one file, not the run a numbered still sits in (#2074). A path
            // that is not followed is not read.
            let probed = s
                .stored_paths
                .check(f)
                .map_err(|(p, why)| crate::doc_path::refusal(&p, why))
                .and_then(|_| if f.kind == FootageKind::Sequence { s.probe_footage(&f.path) } else { importer.probe(&f.path) });
            let nf = match probed {
                Ok(p) => refreshed(s, f, p),
                Err(_) if undoable => Footage { missing: true, ..f.clone() },
                Err(_) => {
                    failed.insert(f.path.clone());
                    continue;
                }
            };
            paths.insert(f.path.clone());
            paths.extend(f.sequence.iter().chain(&nf.sequence).cloned());
            updates.push((*i, nf));
        }
    }
    let n = updates.len();
    apply_reloads(s, updates, undoable)?;
    forget_media(s, &paths);
    // An explicit reload also reads files again that kept their size and time (the check on hit
    // can't tell those apart): their entries in the disk cache go (#3184), when the cache is
    // switched on if it is off now. An automatic one only follows a changed stamp, which the
    // check catches.
    if undoable {
        match &s.disk_cache {
            Some(dc) => dc.drop_files(paths.iter().map(String::as_str)),
            None => s.pending_disk_drops.extend(paths),
        }
    }
    Ok((n, failed))
}

/// Puts reloaded footage into the project: an undoable edit, or (not undoable) a disk change
/// that leaves the project's dirty state as it was.
fn apply_reloads(s: &mut Session, updates: Vec<(ItemId, Footage)>, undoable: bool) -> crate::Result<()> {
    if undoable {
        s.edit("Reload Footage", None, |proj, _| {
            for (i, f) in updates {
                if let Some(it) = proj.item_mut(i) {
                    it.kind = ItemKind::Footage(f);
                }
            }
            Ok(())
        })?;
    } else {
        let p = Arc::make_mut(&mut s.project);
        let mut changed = false;
        for (i, f) in updates {
            if let Some(it) = p.item_mut(i)
                && !matches!(&it.kind, ItemKind::Footage(cur) if *cur == f)
            {
                it.kind = ItemKind::Footage(f);
                changed = true;
            }
        }
        if changed {
            let dirty = s.is_dirty();
            s.bump();
            if !dirty {
                // Only the disk changed: the project isn't modified.
                s.mark_saved();
            }
        }
    }
    Ok(())
}

/// Make the media layer and the layer cache let go of `paths`, so renders read them again. Disk
/// cache entries check the files they were made of when read (#3184); File ▸ Reload Footage
/// also drops the entries of its files ([`reload`]).
pub(crate) fn forget_media(s: &mut Session, paths: &BTreeSet<String>) {
    for p in paths {
        s.footage.forget(p);
    }
    s.layer_cache.clear();
    s.events.push(crate::Event::PurgeCaches);
}

/// Compare fresh stamps of the watched files with the ones last seen. A file seen for the first
/// time is only remembered; one whose stamp moved is reloaded: the items reading it get their
/// size and duration from it again and its pixels are read again. A file that can't be stamped
/// right now (an app saving by delete and rename) keeps its last stamp, and one that can't be
/// read (still being written) is tried again once it changes again; missing files are left to
/// the footage check. Returns `{files, reloaded}`.
pub fn apply_stamps(s: &mut Session, now: Vec<(String, Option<Stamp>)>) -> crate::Result<Value> {
    let before = std::mem::take(&mut s.footage_stamps);
    let mut changed = BTreeSet::new();
    // Only what is watched now is remembered (files no longer used are dropped).
    for (path, stamp) in now {
        match (before.get(&path), stamp) {
            (Some(old), None) => {
                s.footage_stamps.insert(path, *old);
            }
            (old, Some(stamp)) => {
                if old.is_some_and(|o| *o != stamp) {
                    changed.insert(path.clone());
                }
                s.footage_stamps.insert(path, stamp);
            }
            (None, None) => {}
        }
    }
    if changed.is_empty() {
        return Ok(json!({"files": 0, "reloaded": 0}));
    }
    let ids: Vec<ItemId> = s.project.items.values().filter(|i| matches!(&i.kind, ItemKind::Footage(f) if changed.contains(&f.path))).map(|i| i.id).collect();
    let (reloaded, failed) = if ids.is_empty() { (0, BTreeSet::new()) } else { reload(s, &ids, false)? };
    // Not readable (still being written, or not a file the importer reads): its new stamp is kept,
    // so it is tried again once it changes again, not on every look.
    for p in &failed {
        changed.remove(p);
    }
    if changed.is_empty() {
        return Ok(json!({"files": 0, "reloaded": 0}));
    }
    // Changed frames of image sequences (their items keep their metadata).
    forget_media(s, &changed);
    let files = changed.len();
    s.toast(if reloaded > 0 {
        format!("Reloaded {reloaded} footage item(s) changed on disk")
    } else {
        format!("Reloaded {files} footage file(s) changed on disk")
    });
    Ok(json!({"files": files, "reloaded": reloaded}))
}

/// `footage.reloadChanged`: stamp the watched files now and reload the ones that changed.
pub(crate) fn command(s: &mut Session, _: &Value) -> crate::Result<Value> {
    let services = s.services.clone();
    let watched = Watched {
        reload: reload_paths(s),
        sources: Arc::new(availability::sources(s)),
        project: Some(s.project.clone()),
        reload_mode: s.prefs.import.auto_reload_footage.clone(),
        stored_paths: s.stored_paths.clone(),
    };
    let observation = watched.scan(services.as_ref());
    Scan::new(watched, observation, ReloadPolicy::Manual).apply(s)
}

/// How often the watched files are stamped while the window is in front.
pub const FOCUSED_INTERVAL: Duration = Duration::from_secs(2);
/// How often they are stamped while another app has the front: a file saved there is noticed
/// as soon as the window comes back, so this only covers a second screen.
pub const UNFOCUSED_INTERVAL: Duration = Duration::from_secs(10);

#[derive(Default)]
struct Watch {
    paths: Watched,
    focused: bool,
    recheck: bool,
}

/// The desktop app's footage watcher: a worker thread that stamps the watched files and hands
/// the stamps over ([`Watcher::take`]) only when one of them changed since it last looked,
/// waking the interface through the callback given to [`Watcher::start`]. The interface tells it
/// what to watch and whether the window is in front; nothing watched means the thread sleeps.
/// Dropping the watcher ends the thread.
pub struct Watcher {
    tx: SyncSender<()>,
    pending: Arc<Mutex<Watch>>,
    result: Arc<Mutex<Option<Scan>>>,
    paths: Watched,
    focused: Option<bool>,
}

impl Watcher {
    /// Start the worker, stamping through `services`; `wake` runs on the worker whenever new
    /// stamps are ready to [`Watcher::take`]. `None` where there are no threads (the web app,
    /// whose footage lives in browser storage) or one couldn't be started.
    pub fn start(services: Arc<dyn Services>, wake: impl Fn() + Send + Sync + 'static) -> Option<Watcher> {
        Self::start_with(services, wake, FOCUSED_INTERVAL, UNFOCUSED_INTERVAL)
    }

    /// [`Watcher::start`] with its own stamping intervals (tests).
    pub fn start_with(services: Arc<dyn Services>, wake: impl Fn() + Send + Sync + 'static, focused: Duration, unfocused: Duration) -> Option<Watcher> {
        if cfg!(target_arch = "wasm32") {
            return None;
        }
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        let pending = Arc::new(Mutex::new(Watch { focused: true, ..Default::default() }));
        let input = pending.clone();
        let result: Arc<Mutex<Option<Scan>>> = Arc::default();
        let out = Arc::clone(&result);
        std::thread::Builder::new().name("footage-watch".into()).spawn(move || watch(&rx, &input, &*services, &out, &wake, focused, unfocused)).ok()?;
        Some(Watcher { tx, pending, result, paths: Watched::default(), focused: None })
    }

    /// Refresh the project snapshot after an edit without rebuilding sequence paths or waking
    /// the worker when the sources, path provenance and reload policy are unchanged.
    pub fn update(&mut self, s: &Session) {
        let same_sources = availability::sources_match(s, &self.paths.sources);
        let same_data = self.paths.project.as_ref().is_some_and(|before| data_paths(before).eq(data_paths(&s.project)));
        if same_sources && same_data && self.paths.stored_paths == s.stored_paths && self.paths.reload_mode == s.prefs.import.auto_reload_footage {
            let project = Some(s.project.clone());
            self.paths.project = project.clone();
            self.pending.lock().unwrap_or_else(PoisonError::into_inner).paths.project = project;
        } else {
            self.set_watched(watched(s));
        }
    }

    /// The files to watch ([`watched`]); a changed set is stamped at once.
    pub fn set_watched(&mut self, paths: Watched) {
        let changed = paths != self.paths;
        self.paths = paths.clone();
        self.pending.lock().unwrap_or_else(PoisonError::into_inner).paths = paths;
        if changed {
            let _ = self.tx.try_send(());
        }
    }

    /// An observation raced with an edit: refresh from the latest snapshot, through the same
    /// bounded queue. Unrelated edits update the snapshot without waking or rescanning.
    pub fn recheck(&self) {
        self.pending.lock().unwrap_or_else(PoisonError::into_inner).recheck = true;
        let _ = self.tx.try_send(());
    }

    /// Whether the window is in front; coming back to the front stamps the files at once.
    pub fn set_focused(&mut self, focused: bool) {
        if self.focused != Some(focused) {
            self.focused = Some(focused);
            self.pending.lock().unwrap_or_else(PoisonError::into_inner).focused = focused;
            let _ = self.tx.try_send(());
        }
    }

    /// Stamps the worker found changed (or the first ones for a new set of files), once; for
    /// [`apply_stamps`]. `None` while nothing changed.
    pub fn take(&self) -> Option<Scan> {
        self.result.lock().unwrap_or_else(PoisonError::into_inner).take().filter(|s| s.watched == self.paths)
    }
}

/// The watcher's worker: wait for a message or the interval, stamp, report only changes.
fn watch(
    rx: &Receiver<()>,
    pending: &Mutex<Watch>,
    services: &dyn Services,
    out: &Mutex<Option<Scan>>,
    wake: &(dyn Fn() + Send + Sync),
    focused_every: Duration,
    unfocused_every: Duration,
) {
    let mut paths = Watched::default();
    let mut focused = true;
    let mut last: Option<Observation> = None;
    let mut scanner = availability::Scanner::default();
    loop {
        let msg = if paths.is_empty() {
            rx.recv().map_err(|_| RecvTimeoutError::Disconnected)
        } else {
            rx.recv_timeout(if focused { focused_every } else { unfocused_every })
        };
        if matches!(msg, Err(RecvTimeoutError::Disconnected)) {
            return;
        }
        let mut next = pending.lock().unwrap_or_else(PoisonError::into_inner);
        let changed = next.paths != paths;
        let regained = next.focused && !focused;
        focused = next.focused;
        let recheck = std::mem::take(&mut next.recheck);
        paths = next.paths.clone();
        if changed {
            scanner.reset(&paths.sources);
        }
        if changed || recheck {
            last = None;
        }
        drop(next);
        if msg.is_ok() && !changed && !regained && !recheck {
            continue;
        }
        if paths.is_empty() {
            continue;
        }
        let stamps =
            Observation { stamps: paths.reload.iter().map(|p| (p.clone(), services.stamp(p))).collect(), missing: scanner.poll(&paths.sources, services) };
        if last.as_ref() != Some(&stamps) {
            last = Some(stamps.clone());
            *out.lock().unwrap_or_else(PoisonError::into_inner) = Some(Scan::new(paths.clone(), stamps, ReloadPolicy::Automatic));
            wake();
        }
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod watcher_tests {
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    /// Files whose stamps the test sets; counts how often they were stamped.
    #[derive(Default)]
    struct Disk {
        stamps: Mutex<HashMap<String, Stamp>>,
        looks: AtomicUsize,
    }
    impl Disk {
        fn set(&self, path: &str, stamp: Stamp) {
            self.stamps.lock().unwrap_or_else(PoisonError::into_inner).insert(path.into(), stamp);
        }
    }
    impl Services for Disk {
        fn read_file(&self, path: &str) -> std::io::Result<Vec<u8>> {
            Err(std::io::Error::new(std::io::ErrorKind::NotFound, path.to_string()))
        }
        fn write_file(&self, _: &str, _: &[u8]) -> std::io::Result<()> {
            Err(std::io::Error::other("read-only"))
        }
        fn exists(&self, path: &str) -> bool {
            self.stamp(path).is_some()
        }
        fn is_dir(&self, _: &str) -> bool {
            true
        }
        fn stamp(&self, path: &str) -> Option<Stamp> {
            self.looks.fetch_add(1, Ordering::SeqCst);
            self.stamps.lock().unwrap_or_else(PoisonError::into_inner).get(path).copied()
        }
    }

    const TICK: Duration = Duration::from_millis(20);
    const LONG: Duration = Duration::from_secs(60);

    fn wait_for(w: &Watcher, wakes: &AtomicUsize, n: usize) -> Stamps {
        let t0 = std::time::Instant::now();
        loop {
            if wakes.load(Ordering::SeqCst) >= n
                && let Some(s) = w.take()
            {
                return s.observation.stamps;
            }
            assert!(t0.elapsed() < Duration::from_secs(10), "no report {n} within 10 s");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// Wait (at most 10 s) until `ready`: a slow machine's worker gets there late, never early.
    fn wait_until(what: &str, mut ready: impl FnMut() -> bool) {
        let t0 = std::time::Instant::now();
        while !ready() {
            assert!(t0.elapsed() < Duration::from_secs(10), "{what} within 10 s");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn start(disk: &Arc<Disk>, focused: Duration, unfocused: Duration) -> Option<(Watcher, Arc<AtomicUsize>)> {
        let wakes = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&wakes);
        let w = Watcher::start_with(
            Arc::clone(disk) as Arc<dyn Services>,
            move || {
                counter.fetch_add(1, Ordering::SeqCst);
            },
            focused,
            unfocused,
        )?;
        Some((w, wakes))
    }

    /// The interface is woken for the first stamps of a set and for every change, and for
    /// nothing else: an unchanged file polled many times is no wakeup at all.
    #[test]
    fn the_watcher_wakes_the_interface_only_when_a_stamp_changes() {
        let disk = Arc::new(Disk::default());
        disk.set("/a.psd", (10, 1));
        let (mut w, wakes) = start(&disk, TICK, LONG).unwrap();
        w.set_focused(true);
        w.set_watched(Watched { reload: ["/a.psd".to_string()].into(), ..Default::default() });
        assert_eq!(wait_for(&w, &wakes, 1), vec![("/a.psd".to_string(), Some((10, 1)))]);
        let looked = disk.looks.load(Ordering::SeqCst);
        wait_until("it keeps stamping while the window is in front", || disk.looks.load(Ordering::SeqCst) > looked + 3);
        assert_eq!(wakes.load(Ordering::SeqCst), 1, "an unchanged file is no wakeup");
        assert!(w.take().is_none());
        disk.set("/a.psd", (12, 2));
        assert_eq!(wait_for(&w, &wakes, 2), vec![("/a.psd".to_string(), Some((12, 2)))]);
        assert_eq!(wakes.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn watcher_detects_disconnect_and_return_even_when_automatic_reload_is_off() {
        let disk = Arc::new(Disk::default());
        disk.set("/live.png", (1, 1));
        let mut s = Session { services: disk.clone(), check_footage_on_open: true, ..Default::default() };
        s.prefs.import.auto_reload_footage = "off".into();
        let id = Arc::make_mut(&mut s.project).add_item(
            "Live",
            Default::default(),
            None,
            ItemKind::Footage(Footage { path: "/live.png".into(), width: 64, height: 32, ..Default::default() }),
        );
        s.mark_saved();
        let (mut w, _) = start(&disk, TICK, TICK).unwrap();
        w.set_watched(watched(&s));
        disk.stamps.lock().unwrap().clear();
        wait_until("offline warning", || {
            if let Some(scan) = w.take() {
                scan.apply(&mut s).unwrap();
            }
            s.project.item(id).unwrap().has_missing_footage()
        });
        disk.set("/live.png", (1, 1));
        w.set_watched(watched(&s));
        wait_until("reconnected file", || {
            if let Some(scan) = w.take() {
                scan.apply(&mut s).unwrap();
            }
            !s.project.item(id).unwrap().has_missing_footage()
        });
        assert!(!s.is_dirty());
        assert!(s.history.undo.is_empty());
    }

    #[test]
    fn unrelated_edits_reuse_source_snapshots_without_waking_the_worker() {
        let disk = Arc::new(Disk::default());
        disk.set("/plate.png", (1, 1));
        let mut s = Session { services: disk.clone(), check_footage_on_open: true, ..Default::default() };
        s.prefs.import.auto_reload_footage = "off".into();
        let id = Arc::make_mut(&mut s.project).add_item(
            "Plate",
            Default::default(),
            None,
            ItemKind::Footage(Footage { path: "/plate.png".into(), ..Default::default() }),
        );
        let (mut w, wakes) = start(&disk, LONG, LONG).unwrap();
        w.update(&s);
        wait_for(&w, &wakes, 1);
        let sources = w.paths.sources.clone();
        let looks = disk.looks.load(Ordering::SeqCst);
        Arc::make_mut(&mut s.project).item_mut(id).unwrap().name = "Renamed".into();
        w.update(&s);
        assert!(Arc::ptr_eq(&sources, &w.paths.sources), "sequence snapshots are reused");
        assert!(Arc::ptr_eq(w.pending.lock().unwrap().paths.project.as_ref().unwrap(), &s.project));
        std::thread::sleep(TICK * 3);
        assert_eq!(disk.looks.load(Ordering::SeqCst), looks);
        assert_eq!(wakes.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn stale_availability_update_is_rechecked_against_the_latest_project() {
        let disk = Arc::new(Disk::default());
        let mut s = Session { services: disk.clone(), check_footage_on_open: true, ..Default::default() };
        s.prefs.import.auto_reload_footage = "off".into();
        let id = Arc::make_mut(&mut s.project).add_item(
            "Plate",
            Default::default(),
            None,
            ItemKind::Footage(Footage { path: "/gone.png".into(), ..Default::default() }),
        );
        let (mut w, _) = start(&disk, LONG, LONG).unwrap();
        w.update(&s);
        let mut scan = None;
        wait_until("first observation", || {
            scan = w.take();
            scan.is_some()
        });
        Arc::make_mut(&mut s.project).item_mut(id).unwrap().name = "Keep this edit".into();
        assert_eq!(scan.unwrap().apply(&mut s).unwrap()["stale"], true);
        w.update(&s);
        w.recheck();
        wait_until("fresh availability", || {
            if let Some(scan) = w.take() {
                scan.apply(&mut s).unwrap();
            }
            s.project.item(id).unwrap().has_missing_footage()
        });
        assert_eq!(s.project.item(id).unwrap().name, "Keep this edit");
    }

    #[test]
    fn manual_reload_with_automatic_reload_off_keeps_its_baseline_and_reloads_changed_data() {
        struct DataProbe(Arc<Disk>);
        impl crate::Importer for DataProbe {
            fn probe(&self, path: &str) -> std::result::Result<Footage, String> {
                let data = if self.0.stamp(path) == Some((2, 2)) { "updated" } else { "original" };
                Ok(Footage { path: path.into(), kind: FootageKind::Data, data: Some(data.into()), ..Default::default() })
            }
        }
        let disk = Arc::new(Disk::default());
        disk.set("/data.csv", (1, 1));
        let mut s = Session { services: disk.clone(), importer: Some(Arc::new(DataProbe(disk.clone()))), ..Default::default() };
        s.prefs.import.auto_reload_footage = "off".into();
        let id = Arc::make_mut(&mut s.project).add_item(
            "Data",
            Default::default(),
            None,
            ItemKind::Footage(Footage { path: "/data.csv".into(), kind: FootageKind::Data, data: Some("original".into()), ..Default::default() }),
        );
        assert_eq!(s.execute("footage.reloadChanged", json!({})).unwrap(), json!({"files": 0, "reloaded": 0}));
        let baseline = s.footage_stamps.clone();
        assert_eq!(baseline.get("/data.csv"), Some(&(1, 1)));
        let off = watched(&s);
        Scan::new(off.clone(), off.scan(disk.as_ref()), ReloadPolicy::Automatic).apply(&mut s).unwrap();
        assert_eq!(s.footage_stamps, baseline, "availability-only scans retain the manual baseline");
        s.prefs.import.auto_reload_footage = "nonSequence".into();
        disk.set("/data.csv", (2, 2));
        let old = watched(&s);
        let pending = Scan::new(old.clone(), old.scan(disk.as_ref()), ReloadPolicy::Automatic);
        s.prefs.import.auto_reload_footage = "off".into();
        assert_eq!(pending.apply(&mut s).unwrap(), json!({"files": 0, "reloaded": 0}));
        assert_eq!(s.footage_stamps, baseline, "a stale automatic result cannot reload or erase the baseline");
        assert_eq!(s.execute("footage.reloadChanged", json!({})).unwrap(), json!({"files": 1, "reloaded": 1}));
        assert!(matches!(&s.project.item(id).unwrap().kind, ItemKind::Footage(f) if f.data.as_deref() == Some("updated")));
        assert!(s.history.undo.is_empty());
    }

    #[test]
    fn watcher_updates_external_data_paths_after_import_relink_and_removal() {
        let disk = Arc::new(Disk::default());
        let mut s = Session { services: disk.clone(), check_footage_on_open: true, ..Default::default() };
        s.prefs.import.auto_reload_footage = "all".into();
        let (mut w, wakes) = start(&disk, LONG, LONG).unwrap();
        w.update(&s);
        disk.set("/first.csv", (1, 1));
        let id = Arc::make_mut(&mut s.project).add_item(
            "Data",
            Default::default(),
            None,
            ItemKind::Footage(Footage { path: "/first.csv".into(), kind: FootageKind::Data, ..Default::default() }),
        );
        w.update(&s);
        assert_eq!(wait_for(&w, &wakes, 1), vec![("/first.csv".into(), Some((1, 1)))]);
        disk.set("/second.csv", (2, 2));
        if let ItemKind::Footage(f) = &mut Arc::make_mut(&mut s.project).item_mut(id).unwrap().kind {
            f.path = "/second.csv".into();
        }
        w.update(&s);
        assert_eq!(wait_for(&w, &wakes, 2), vec![("/second.csv".into(), Some((2, 2)))]);
        Arc::make_mut(&mut s.project).items.remove(&id);
        w.update(&s);
        assert!(w.paths.reload.is_empty());
        assert!(w.pending.lock().unwrap().paths.reload.is_empty());
    }

    /// Behind another app the files are stamped only at the slow interval; the window coming
    /// back to the front stamps them at once.
    #[test]
    fn behind_another_app_the_watcher_rests_until_the_window_is_back() {
        let disk = Arc::new(Disk::default());
        disk.set("/b.psd", (1, 1));
        let (mut w, wakes) = start(&disk, TICK, LONG).unwrap();
        w.set_focused(false);
        w.set_watched(Watched { reload: ["/b.psd".to_string()].into(), ..Default::default() });
        wait_for(&w, &wakes, 1);
        let looked = disk.looks.load(Ordering::SeqCst);
        disk.set("/b.psd", (2, 2));
        std::thread::sleep(TICK * 15);
        assert_eq!(disk.looks.load(Ordering::SeqCst), looked, "no stamping at the front interval while behind");
        assert_eq!(wakes.load(Ordering::SeqCst), 1);
        w.set_focused(true);
        assert_eq!(wait_for(&w, &wakes, 2), vec![("/b.psd".to_string(), Some((2, 2)))]);
    }

    /// Nothing to watch: the worker sleeps, stamping nothing; a new set is stamped at once and
    /// reported whole, and dropping the watcher ends the worker.
    #[test]
    fn nothing_watched_is_nothing_stamped_and_a_new_set_is_reported_whole() {
        let disk = Arc::new(Disk::default());
        disk.set("/c.psd", (1, 1));
        disk.set("/d.psd", (2, 2));
        let (mut w, wakes) = start(&disk, TICK, TICK).unwrap();
        w.set_focused(true);
        std::thread::sleep(TICK * 10);
        assert_eq!(disk.looks.load(Ordering::SeqCst), 0);
        w.set_watched(Watched { reload: ["/c.psd".to_string(), "/d.psd".to_string()].into(), ..Default::default() });
        assert_eq!(wait_for(&w, &wakes, 1), vec![("/c.psd".to_string(), Some((1, 1))), ("/d.psd".to_string(), Some((2, 2)))]);
        w.set_watched(Watched { reload: ["/d.psd".to_string()].into(), ..Default::default() });
        assert_eq!(wait_for(&w, &wakes, 2), vec![("/d.psd".to_string(), Some((2, 2)))], "the set is reported whole, so the session forgets /c.psd");
        w.set_watched(Watched::default());
        // The worker may already be past its wait with the old set: that one round of stamps
        // (one file) can still come, and nothing after it.
        let looked = disk.looks.load(Ordering::SeqCst);
        std::thread::sleep(TICK * 10);
        assert!(disk.looks.load(Ordering::SeqCst) <= looked + 1, "nothing watched, nothing stamped");
        let result = Arc::downgrade(&w.result);
        drop(w);
        let t0 = std::time::Instant::now();
        while result.upgrade().is_some() {
            assert!(t0.elapsed() < Duration::from_secs(5), "the worker is still running");
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}
