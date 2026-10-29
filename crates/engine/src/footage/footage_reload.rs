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
//! were when the file was last seen. The desktop app takes those stamps every two seconds (and
//! when its window comes back to the front) on a worker thread, so a file on a slow network share
//! never holds up the interface ([`start_scan`], [`poll_scan`]); `footage.reloadChanged` takes them
//! on the spot (CLI, agents, tests). Like the footage check after File ▸ Open, an automatic reload
//! records the state of the disk: it is not an undo step and doesn't modify the project.

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

use effectcraft_project::{Footage, FootageKind, ItemId, ItemKind, SourceLayer};
use serde_json::{Value, json};

use crate::Session;

/// A file's size and modification time (see [`crate::Services::stamp`]).
pub type Stamp = (u64, u128);

/// Stamps taken on a worker thread, waiting for [`poll_scan`].
pub type Scan = Arc<Mutex<Option<Vec<(String, Option<Stamp>)>>>>;

/// The files footage items read: each item's file and every frame of an image sequence. Image
/// sequences count only with Automatically Reload Footage set to All Footage (After Effects'
/// default, Non-Sequence Footage, leaves them alone: thousands of frames on a share add up).
/// Items with a stored path that is not followed ([`crate::doc_path`]) are not watched.
fn watched(s: &Session) -> BTreeSet<String> {
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

/// Start stamping the watched files on a worker thread, unless a scan is still running (or the
/// build has no threads: the web app, whose footage lives in browser storage).
pub fn start_scan(s: &mut Session) {
    if s.footage_scan.is_some() || cfg!(target_arch = "wasm32") {
        return;
    }
    let paths = watched(s);
    if paths.is_empty() {
        return;
    }
    let services = s.services.clone();
    let slot: Scan = Arc::default();
    let out = slot.clone();
    let spawned = std::thread::Builder::new().name("footage-watch".into()).spawn(move || {
        let stamps = paths.into_iter().map(|p| (p.clone(), services.stamp(&p))).collect();
        *out.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(stamps);
    });
    if spawned.is_ok() {
        s.footage_scan = Some(slot);
    }
}

/// Apply a finished scan (`None` while there is none, or it is still running).
pub fn poll_scan(s: &mut Session) -> Option<crate::Result<Value>> {
    let stamps = s.footage_scan.as_ref()?.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take()?;
    s.footage_scan = None;
    Some(apply_stamps(s, stamps))
}
