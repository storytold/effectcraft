//! Footage that changed on disk: File ▸ Reload Footage (Cmd+Alt+L) and Settings ▸ Import ▸
//! Reload Footage Changed on Disk.
//!
//! Reloading re-reads what the file decides (its size, a movie's duration, streams and frame
//! rate, the codec, a data file's text) and keeps what the project decided: the Photoshop layer
//! or PDF page an item shows and its interpretation (alpha, conformed frame rate, pixel aspect,
//! fields, colour). A Photoshop layer is found again by name when layers were added or removed
//! around it. The media layer then forgets the file and the layer cache is emptied, so the next
//! render reads the new pixels.
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
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
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
pub fn watched(s: &Session) -> BTreeSet<String> {
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

/// Size and position in the stack of Photoshop layer `l` in the file at `path` now: the layer
/// at its recorded index when the name still matches, else the first layer with its name.
fn psd_layer(s: &Session, path: &str, l: &SourceLayer) -> Option<(u32, Option<(u32, u32)>)> {
    let psd = effectcraft_psd::Psd::parse(s.services.read_file(path).ok()?).ok()?;
    let found = psd.layers.get(l.index as usize).filter(|x| x.name == l.name).or_else(|| psd.layers.iter().find(|x| x.name == l.name))?;
    let size = (!found.rect.is_empty()).then(|| (found.rect.width(), found.rect.height()));
    Some((u32::try_from(found.index).ok()?, size))
}

fn vector_size(s: &Session, f: &Footage) -> Option<(u32, u32)> {
    let bytes = s.services.read_file(&f.path).ok()?;
    let doc = crate::vector::vector_doc(&f.path, &bytes, f.layer.as_ref(), f.page)?.ok()?;
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
    forget(s, &paths);
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

/// Make the media layer and the layer cache let go of `paths`, so renders read them again.
fn forget(s: &mut Session, paths: &BTreeSet<String>) {
    for p in paths {
        s.footage.forget(p);
    }
    s.layer_cache.clear();
    // Reloaded files may keep their footage settings but not their size and time.
    s.refresh_disk_salt(true);
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
    forget(s, &changed);
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
    let now = watched(s).into_iter().map(|p| (p.clone(), services.stamp(&p))).collect();
    apply_stamps(s, now)
}

/// How often the watched files are stamped while the window is in front.
pub const FOCUSED_INTERVAL: Duration = Duration::from_secs(2);
/// How often they are stamped while another app has the front: a file saved there is noticed
/// as soon as the window comes back, so this only covers a second screen.
pub const UNFOCUSED_INTERVAL: Duration = Duration::from_secs(10);

enum Watch {
    Paths(BTreeSet<String>),
    Focused(bool),
}

/// The desktop app's footage watcher: a worker thread that stamps the watched files and hands
/// the stamps over ([`Watcher::take`]) only when one of them changed since it last looked,
/// waking the interface through the callback given to [`Watcher::start`]. The interface tells it
/// what to watch and whether the window is in front; nothing watched means the thread sleeps.
/// Dropping the watcher ends the thread.
pub struct Watcher {
    tx: Sender<Watch>,
    result: Arc<Mutex<Option<Stamps>>>,
    paths: BTreeSet<String>,
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
        let (tx, rx) = std::sync::mpsc::channel();
        let result: Arc<Mutex<Option<Stamps>>> = Arc::default();
        let out = Arc::clone(&result);
        std::thread::Builder::new().name("footage-watch".into()).spawn(move || watch(&rx, &*services, &out, &wake, focused, unfocused)).ok()?;
        Some(Watcher { tx, result, paths: BTreeSet::new(), focused: None })
    }

    /// The files to watch ([`watched`]); a changed set is stamped at once.
    pub fn set_watched(&mut self, paths: BTreeSet<String>) {
        if paths != self.paths {
            self.paths = paths.clone();
            let _ = self.tx.send(Watch::Paths(paths));
        }
    }

    /// Whether the window is in front; coming back to the front stamps the files at once.
    pub fn set_focused(&mut self, focused: bool) {
        if self.focused != Some(focused) {
            self.focused = Some(focused);
            let _ = self.tx.send(Watch::Focused(focused));
        }
    }

    /// Stamps the worker found changed (or the first ones for a new set of files), once; for
    /// [`apply_stamps`]. `None` while nothing changed.
    pub fn take(&self) -> Option<Stamps> {
        self.result.lock().unwrap_or_else(PoisonError::into_inner).take()
    }
}

/// The watcher's worker: wait for a message or the interval, stamp, report only changes.
fn watch(
    rx: &Receiver<Watch>,
    services: &dyn Services,
    out: &Mutex<Option<Stamps>>,
    wake: &(dyn Fn() + Send + Sync),
    focused_every: Duration,
    unfocused_every: Duration,
) {
    let mut paths = BTreeSet::new();
    let mut focused = true;
    let mut last: Option<Stamps> = None;
    loop {
        let msg = if paths.is_empty() {
            rx.recv().map_err(|_| RecvTimeoutError::Disconnected)
        } else {
            rx.recv_timeout(if focused { focused_every } else { unfocused_every })
        };
        match msg {
            Ok(Watch::Paths(p)) => {
                paths = p;
                // A new set is reported whole, so the session forgets files no longer watched.
                last = None;
            }
            Ok(Watch::Focused(f)) => {
                let regained = f && !focused;
                focused = f;
                if !regained {
                    continue;
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
        if paths.is_empty() {
            continue;
        }
        let stamps: Stamps = paths.iter().map(|p| (p.clone(), services.stamp(p))).collect();
        if last.as_ref() != Some(&stamps) {
            last = Some(stamps.clone());
            *out.lock().unwrap_or_else(PoisonError::into_inner) = Some(stamps);
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
                return s;
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
        w.set_watched(["/a.psd".to_string()].into());
        assert_eq!(wait_for(&w, &wakes, 1), vec![("/a.psd".to_string(), Some((10, 1)))]);
        let looked = disk.looks.load(Ordering::SeqCst);
        wait_until("it keeps stamping while the window is in front", || disk.looks.load(Ordering::SeqCst) > looked + 3);
        assert_eq!(wakes.load(Ordering::SeqCst), 1, "an unchanged file is no wakeup");
        assert!(w.take().is_none());
        disk.set("/a.psd", (12, 2));
        assert_eq!(wait_for(&w, &wakes, 2), vec![("/a.psd".to_string(), Some((12, 2)))]);
        assert_eq!(wakes.load(Ordering::SeqCst), 2);
    }

    /// Behind another app the files are stamped only at the slow interval; the window coming
    /// back to the front stamps them at once.
    #[test]
    fn behind_another_app_the_watcher_rests_until_the_window_is_back() {
        let disk = Arc::new(Disk::default());
        disk.set("/b.psd", (1, 1));
        let (mut w, wakes) = start(&disk, TICK, LONG).unwrap();
        w.set_focused(false);
        w.set_watched(["/b.psd".to_string()].into());
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
        w.set_watched(["/c.psd".to_string(), "/d.psd".to_string()].into());
        assert_eq!(wait_for(&w, &wakes, 1), vec![("/c.psd".to_string(), Some((1, 1))), ("/d.psd".to_string(), Some((2, 2)))]);
        w.set_watched(["/d.psd".to_string()].into());
        assert_eq!(wait_for(&w, &wakes, 2), vec![("/d.psd".to_string(), Some((2, 2)))], "the set is reported whole, so the session forgets /c.psd");
        w.set_watched(BTreeSet::new());
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
