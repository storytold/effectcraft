//! Snapshot-based footage availability, shared by open-time checks and the disk watcher.
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use crate::{Services, Session};
use effectcraft_project::{Footage, FootageKind, ItemId, ItemKind, Project};

#[derive(Clone, PartialEq)]
pub(crate) struct Source {
    pub id: ItemId,
    pub proxy: bool,
    pub footage: Footage,
    paths: Vec<String>,
    folder: Option<String>,
}

impl Source {
    /// Find the same source settings in a newer project, ignoring only the disk-derived
    /// missing flag so an overlapping check cannot invalidate moved-source recovery.
    pub fn current<'a>(&self, project: &'a mut Project) -> Option<&'a mut Footage> {
        let item = project.item_mut(self.id)?;
        let f = if self.proxy {
            &mut item.proxy.as_mut()?.footage
        } else {
            let ItemKind::Footage(f) = &mut item.kind else { return None };
            f
        };
        // Availability is an observation, not a source edit. Another disk check must not
        // invalidate an open-time probe or recovery of a source moved with the project.
        let missing = std::mem::replace(&mut f.missing, self.footage.missing);
        let same = *f == self.footage;
        f.missing = missing;
        same.then_some(f)
    }
}

/// External originals and proxies that need availability checks. Embedded data and empty
/// paths remain usable without an external file and are excluded from this inventory.
fn entries(s: &Session) -> impl Iterator<Item = (ItemId, bool, &Footage)> {
    s.project.items.values().flat_map(|item| {
        let original = match &item.kind {
            ItemKind::Footage(f) => Some(f),
            _ => None,
        };
        original
            .map(|f| (false, f))
            .into_iter()
            .chain(item.proxy.as_ref().map(|p| (true, &p.footage)))
            .filter(|(_, f)| f.kind != FootageKind::Data && !f.path.is_empty())
            .map(move |(proxy, f)| (item.id, proxy, f))
    })
}

/// Check whether a snapshot can be reused without allocating new sequence path strings.
pub(crate) fn sources_match(s: &Session, sources: &[Source]) -> bool {
    let mut before = sources.iter();
    for (id, proxy, footage) in entries(s) {
        let Some(old) = before.next() else {
            return false;
        };
        if old.id != id || old.proxy != proxy || old.footage != *footage {
            return false;
        }
    }
    before.next().is_none()
}

/// Resolve accepted paths once for the worker. Refused document paths retain an empty probe
/// list, which reports missing without contacting the filesystem or a foreign share.
pub(crate) fn sources(s: &Session) -> Vec<Source> {
    let mut out = Vec::new();
    for (id, proxy, f) in entries(s) {
        let paths: Vec<String> = if s.stored_paths.check(f).is_ok() {
            std::iter::once(&f.path).chain(&f.sequence).filter_map(|p| s.stored_path(p).ok()?.to_str().map(str::to_string)).collect()
        } else {
            Vec::new()
        };
        let folder = paths
            .first()
            .and_then(|p| std::path::Path::new(p).parent())
            .filter(|dir| !dir.as_os_str().is_empty() && paths.iter().all(|p| std::path::Path::new(p).parent() == Some(*dir)))
            .and_then(|p| p.to_str())
            .map(str::to_string);
        out.push(Source { id, proxy, footage: f.clone(), paths, folder });
    }
    out
}

/// Each unique file is checked at most once per pass. Sequences are available while any frame
/// exists; when their common directory is offline, avoid probing thousands of absent frames.
pub(crate) fn scan(sources: &[Source], services: &dyn Services) -> Vec<bool> {
    let mut files = BTreeMap::new();
    let mut folders = BTreeMap::new();
    sources
        .iter()
        .map(|source| {
            let mut paths = source.paths.iter();
            let Some(first) = paths.next() else { return true };
            let present = |p: &String, files: &mut BTreeMap<String, bool>| *files.entry(p.clone()).or_insert_with(|| services.exists(p));
            if present(first, &mut files) {
                return false;
            }
            if let Some(dir) = &source.folder
                && !*folders.entry(dir.clone()).or_insert_with(|| services.is_dir(dir))
            {
                return true;
            }
            !paths.any(|p| present(p, &mut files))
        })
        .collect()
}

/// Maximum availability probes per automatic polling pass. Explicit checks may finish a full
/// scan; automatic monitoring resumes partial sequence scans and keeps the last proven status.
pub(crate) const PROBE_BUDGET: usize = 256;

struct Progress {
    next: usize,
    absent: usize,
    missing: bool,
}

#[derive(Default)]
pub(crate) struct Scanner {
    progress: Vec<Progress>,
    source: usize,
}

impl Scanner {
    /// Discard partial evidence after watched sources change, retaining their last known flags.
    pub fn reset(&mut self, sources: &[Source]) {
        self.progress = sources.iter().map(|s| Progress { next: 0, absent: 0, missing: s.footage.missing }).collect();
        self.source = 0;
    }

    /// Spend at most PROBE_BUDGET file/folder probes and return one status per source.
    /// Partial sequence scans retain the last proven status and resume on a later poll.
    pub fn poll(&mut self, sources: &[Source], services: &dyn Services) -> Vec<bool> {
        if self.progress.len() != sources.len() {
            self.reset(sources);
        }
        if sources.is_empty() {
            return Vec::new();
        }
        let mut remaining = PROBE_BUDGET;
        let mut visited = 0usize;
        let mut files = BTreeMap::new();
        let mut folders = BTreeMap::new();
        self.source %= sources.len();
        let mut continuation = false;
        while remaining > 0 {
            if visited >= sources.len() {
                if !self.progress.iter().any(|p| p.absent > 0) {
                    break;
                }
                visited = 0;
                continuation = true;
            }
            let Some(source) = sources.get(self.source) else {
                break;
            };
            let Some(progress) = self.progress.get_mut(self.source) else {
                break;
            };
            if continuation && progress.absent == 0 {
                visited = visited.saturating_add(1);
                self.source = self.source.saturating_add(1) % sources.len();
                continue;
            }
            let quota = (remaining / sources.len().saturating_sub(visited).max(1)).max(2).min(remaining);
            let mut used = 0usize;
            let mut checked_folder = false;
            if source.paths.is_empty() {
                progress.missing = true;
            }
            while used < quota && !source.paths.is_empty() {
                let Some(path) = source.paths.get(progress.next) else {
                    progress.next = 0;
                    break;
                };
                remaining = remaining.saturating_sub(1);
                used = used.saturating_add(1);
                if *files.entry(path.clone()).or_insert_with(|| services.exists(path)) {
                    progress.missing = false;
                    progress.absent = 0;
                    // Reuse this witness next time rather than scan absent earlier frames.
                    break;
                }
                progress.next = progress.next.saturating_add(1) % source.paths.len();
                progress.absent = progress.absent.saturating_add(1);
                if progress.absent >= source.paths.len() {
                    progress.missing = true;
                    progress.absent = 0;
                    break;
                }
                if !checked_folder
                    && used < quota
                    && let Some(folder) = &source.folder
                {
                    remaining = remaining.saturating_sub(1);
                    used = used.saturating_add(1);
                    checked_folder = true;
                    if !*folders.entry(folder.clone()).or_insert_with(|| services.is_dir(folder)) {
                        progress.missing = true;
                        progress.next = 0;
                        progress.absent = 0;
                        break;
                    }
                }
            }
            visited = visited.saturating_add(1);
            self.source = self.source.saturating_add(1) % sources.len();
        }
        self.progress.iter().map(|p| p.missing).collect()
    }
}

/// A disk observation prepared entirely by the existing worker. The UI only swaps a current
/// snapshot (the disk cache needs nothing: its entries check their own files when read, #3184).
pub(crate) struct Update {
    before: Arc<Project>,
    after: Arc<Project>,
    paths: BTreeSet<String>,
    newly_missing: usize,
}

/// Prepare a replacement only when availability changed. Project cloning belongs on the
/// worker; apply later rejects an outdated project snapshot.
pub(crate) fn prepare(project: &Arc<Project>, sources: &[Source], missing: &[bool]) -> Option<Update> {
    let mut after = project.clone();
    let mut paths = BTreeSet::new();
    let mut newly_missing = 0;
    for (source, missing) in sources.iter().zip(missing) {
        if source.footage.missing == *missing {
            continue;
        }
        let Some(current) = source.current(Arc::make_mut(&mut after)) else { continue };
        current.missing = *missing;
        newly_missing += usize::from(*missing);
        paths.insert(source.footage.path.clone());
        paths.extend(source.footage.sequence.iter().cloned());
    }
    if paths.is_empty() {
        return None;
    }
    Some(Update { before: project.clone(), after, paths, newly_missing })
}

impl Update {
    /// False means a newer project needs a fresh observation.
    pub fn apply(self, s: &mut Session) -> bool {
        if !Arc::ptr_eq(&s.project, &self.before) {
            return false;
        }
        let dirty = s.is_dirty();
        s.project = self.after;
        s.bump();
        if !dirty {
            s.mark_saved();
        }
        super::footage_reload::forget_media(s, &self.paths);
        if self.newly_missing > 0 {
            s.toast(format!(
                "{} footage source(s) offline. Use File ▸ Dependencies ▸ Relink Missing Footage, or Set Proxy for a missing proxy.",
                self.newly_missing
            ));
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use effectcraft_project::Proxy;
    use serde_json::json;
    use std::sync::{
        Mutex,
        atomic::{AtomicUsize, Ordering},
    };

    #[derive(Default)]
    struct Disk {
        files: Mutex<BTreeSet<String>>,
        looks: AtomicUsize,
        existing_dir: bool,
    }
    impl Disk {
        fn set(&self, paths: &[&str]) {
            *self.files.lock().unwrap() = paths.iter().map(|p| p.to_string()).collect();
        }
    }
    impl Services for Disk {
        fn read_file(&self, _: &str) -> std::io::Result<Vec<u8>> {
            Err(std::io::ErrorKind::NotFound.into())
        }
        fn write_file(&self, _: &str, _: &[u8]) -> std::io::Result<()> {
            Ok(())
        }
        fn exists(&self, path: &str) -> bool {
            self.looks.fetch_add(1, Ordering::Relaxed);
            self.files.lock().unwrap().contains(path)
        }
        fn is_dir(&self, path: &str) -> bool {
            if self.existing_dir {
                return true;
            }
            self.files.lock().unwrap().iter().any(|p| std::path::Path::new(p).parent() == Some(std::path::Path::new(path)))
        }
    }
    fn session(disk: Arc<Disk>) -> (Session, ItemId) {
        let mut s = Session { services: disk, ..Default::default() };
        s.prefs.import.auto_reload_footage = "off".into();
        let f = Footage { path: "/media/original.png".into(), width: 64, height: 32, has_video: true, ..Default::default() };
        let id = Arc::make_mut(&mut s.project).add_item("Plate", Default::default(), None, ItemKind::Footage(f.clone()));
        Arc::make_mut(&mut s.project).item_mut(id).unwrap().proxy =
            Some(Box::new(Proxy { footage: Footage { path: "/media/proxy.png".into(), ..f }, enabled: false }));
        s.mark_saved();
        (s, id)
    }
    fn check(s: &mut Session) {
        s.execute("footage.reloadChanged", json!({})).unwrap();
    }

    #[test]
    fn disconnect_and_reconnect_update_original_and_unused_proxy_without_edits() {
        let disk = Arc::new(Disk::default());
        disk.set(&["/media/original.png", "/media/proxy.png"]);
        let (mut s, id) = session(disk.clone());
        check(&mut s);
        assert!(!s.project.item(id).unwrap().has_missing_footage());
        disk.set(&["/media/proxy.png"]);
        check(&mut s);
        let sum = s.execute("project.summary", json!({})).unwrap();
        assert_eq!(sum["items"][0]["missing"], true);
        assert!(sum["items"][0]["problem"].as_str().unwrap().contains("/media/original.png"));
        disk.set(&["/media/original.png"]);
        check(&mut s);
        let item = s.project.item(id).unwrap();
        assert!(matches!(&item.kind, ItemKind::Footage(f) if !f.missing));
        assert!(item.proxy.as_ref().unwrap().footage.missing);
        disk.set(&["/media/original.png", "/media/proxy.png"]);
        check(&mut s);
        assert!(!s.project.item(id).unwrap().has_missing_footage());
        assert!(!s.is_dirty());
        assert!(s.history.undo.is_empty());
        let revision = s.revision;
        check(&mut s);
        assert_eq!(s.revision, revision, "unchanged observations are idempotent");
    }

    #[test]
    fn open_time_check_includes_proxies() {
        let disk = Arc::new(Disk::default());
        disk.set(&["/media/original.png"]);
        let (mut s, id) = session(disk);
        let r = s.execute("footage.check", json!({"wait": true})).unwrap();
        assert_eq!(r, json!({"checked": 2, "missing": 1, "probed": 0}));
        assert!(s.project.item(id).unwrap().proxy.as_ref().unwrap().footage.missing);
        assert!(!s.is_dirty());
    }

    #[test]
    fn stale_observation_cannot_mark_a_replacement_missing() {
        let (mut s, id) = session(Arc::new(Disk::default()));
        let old = sources(&s);
        let update = prepare(&s.project, &old, &[true, true]).unwrap();
        if let ItemKind::Footage(f) = &mut Arc::make_mut(&mut s.project).item_mut(id).unwrap().kind {
            f.path = "/new.png".into();
        }
        assert!(!update.apply(&mut s));
        assert!(matches!(&s.project.item(id).unwrap().kind, ItemKind::Footage(f) if !f.missing));
    }

    #[test]
    fn sequences_short_circuit_and_offline_folders_do_not_probe_every_frame() {
        let disk = Arc::new(Disk::default());
        disk.set(&["/media/frame1.png"]);
        let (mut s, id) = session(disk.clone());
        if let ItemKind::Footage(f) = &mut Arc::make_mut(&mut s.project).item_mut(id).unwrap().kind {
            f.kind = FootageKind::Sequence;
            f.path = "/media/frame0.png".into();
            f.sequence = (0..10_000).map(|n| format!("/media/frame{n}.png")).collect();
        }
        let source = sources(&s);
        assert_eq!(scan(&source, disk.as_ref()), [false, true], "a later frame keeps the sequence available");
        assert!(disk.looks.load(Ordering::Relaxed) < 5);
        disk.set(&[]);
        disk.looks.store(0, Ordering::Relaxed);
        assert_eq!(scan(&source, disk.as_ref()), [true, true]);
        assert_eq!(disk.looks.load(Ordering::Relaxed), 2, "one file per source, no scan of an unmounted folder");
    }

    /// An availability change does no disk-cache work (#3184): the cache stays attached and
    /// no file is looked at for it, on the worker or when the UI applies the observation (it
    /// used to stamp every footage file for a new layer salt).
    #[test]
    fn applying_an_availability_change_does_no_disk_cache_work() {
        let dir = std::env::temp_dir().join(format!("ec-availability-cache-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("plate.png");
        std::fs::write(&file, b"plate").unwrap();
        let mut s = Session::default();
        Arc::make_mut(&mut s.project).add_item(
            "Plate",
            Default::default(),
            None,
            ItemKind::Footage(Footage { path: file.to_string_lossy().into_owned(), has_video: true, missing: true, ..Default::default() }),
        );
        let cache = crate::render::disk_cache::DiskCache::open(dir.join("cache"), 1 << 20).unwrap();
        s.disk_cache = Some(cache.clone());
        s.layer_cache.set_disk(Some(cache.clone()));
        s.mark_saved();
        let update = prepare(&s.project, &sources(&s), &[false]).unwrap();
        std::fs::remove_file(file).unwrap();
        assert!(update.apply(&mut s));
        assert!(s.layer_cache.disk().is_some_and(|d| Arc::ptr_eq(&d, &cache)), "still attached");
        assert_eq!(cache.file_checks(), 0, "no file looked at for the disk cache");
        assert!(!s.is_dirty());
        drop(s);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn automatic_sequence_checks_are_bounded_and_resume_without_starving_other_sources() {
        let disk = Arc::new(Disk { existing_dir: true, ..Default::default() });
        let (mut s, id) = session(disk.clone());
        if let ItemKind::Footage(f) = &mut Arc::make_mut(&mut s.project).item_mut(id).unwrap().kind {
            f.kind = FootageKind::Sequence;
            f.path = "/media/frame0.png".into();
            f.sequence = (0..10_000).map(|n| format!("/media/frame{n}.png")).collect();
        }
        let sources = sources(&s);
        let mut scanner = Scanner::default();
        let mut status = Vec::new();
        for _ in 0..100 {
            disk.looks.store(0, Ordering::Relaxed);
            status = scanner.poll(&sources, disk.as_ref());
            assert!(disk.looks.load(Ordering::Relaxed) <= PROBE_BUDGET);
            assert!(status[1], "the single-file proxy is checked without waiting for the long sequence");
            if status[0] {
                break;
            }
        }
        assert_eq!(status, [true, true], "a complete absent cycle proves the sequence missing");
        disk.set(&["/media/frame9999.png"]);
        for _ in 0..100 {
            disk.looks.store(0, Ordering::Relaxed);
            status = scanner.poll(&sources, disk.as_ref());
            assert!(disk.looks.load(Ordering::Relaxed) <= PROBE_BUDGET);
            if !status[0] {
                break;
            }
        }
        assert_eq!(status, [false, true], "a late frame is eventually found");
        disk.looks.store(0, Ordering::Relaxed);
        assert_eq!(scanner.poll(&sources, disk.as_ref()), [false, true]);
        assert!(disk.looks.load(Ordering::Relaxed) <= 2, "keep the available late frame as a witness");
    }

    #[test]
    fn save_disconnect_undo_redo_and_reconciliation_return_to_clean_saved_state() {
        let disk = Arc::new(Disk::default());
        disk.set(&["/media/original.png", "/media/proxy.png"]);
        let (mut s, id) = session(disk.clone());
        s.edit("Rename", None, |project, _| {
            project.item_mut(id).unwrap().name = "Saved plate".into();
            Ok(())
        })
        .unwrap();
        s.mark_saved();
        let history = s.history.undo.clone();
        disk.set(&[]);
        check(&mut s);
        assert!(!s.is_dirty());
        assert!(s.undo());
        assert!(s.is_dirty());
        check(&mut s);
        assert!(s.redo());
        check(&mut s);
        assert!(!s.is_dirty(), "redo restores the saved observed snapshot retained by undo");
        assert_eq!(s.project.item(id).unwrap().name, "Saved plate");
        assert_eq!(s.history.undo.len(), history.len());
        assert_eq!(s.history.undo[0].0, history[0].0);
        assert!(s.history.redo.is_empty());
    }

    #[test]
    fn duplicate_sources_share_one_file_probe() {
        let disk = Arc::new(Disk::default());
        disk.set(&["/media/original.png"]);
        let (mut s, id) = session(disk.clone());
        Arc::make_mut(&mut s.project).item_mut(id).unwrap().proxy.as_mut().unwrap().footage.path = "/media/original.png".into();
        assert_eq!(scan(&sources(&s), disk.as_ref()), [false, false]);
        assert_eq!(disk.looks.load(Ordering::Relaxed), 1);
    }
}
