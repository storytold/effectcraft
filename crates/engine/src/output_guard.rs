//! Protect files the project reads from render outputs (#1743).
//!
//! This guard is independent of replacement consent and export staging: it never authorizes
//! overwriting ordinary outputs or opens a second confirmation flow. Those belong to the
//! exporter. Existing aliases are compared by native file identity, including hard links.
//! Missing sources remain protected by their normalized path. Checks happen before dispatch;
//! filesystem changes between the check and the write require exporter-side revalidation.

use std::collections::{HashMap, HashSet};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use effectcraft_project::render_queue::{RenderQueueItem, log_path, numbered_template, sequence_frame, sequence_path};
use effectcraft_project::{Footage, ItemKind};

use crate::Session;

/// Volumes on Windows and macOS ignore case unless they were set up not to.
const IGNORES_CASE: bool = cfg!(any(windows, target_os = "macos"));

/// How many frames are looked at one by one for a sequence whose folder cannot be read at once
/// (its frame number is in a folder's name).
const PROBE_LIMIT: u64 = 100_000;

/// How far a default output name is numbered (`_1` … `_1000`) before it is left to the check at
/// render time.
const MAX_NUMBER: u32 = 1_000;

/// What reads a file the project depends on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Source {
    /// A footage item, by name.
    Footage(Arc<str>),
    /// An item's proxy, by the item's name.
    Proxy(Arc<str>),
    /// The project file.
    Project,
}

impl Source {
    /// Why `path`, a file this reads, cannot be a render's output.
    pub(crate) fn refusal(&self, path: &str) -> String {
        match self {
            Source::Footage(name) => format!("{path} is the footage “{name}” of this project, and a render never replaces footage; choose another Output To"),
            Source::Proxy(name) => format!("{path} is the proxy of “{name}”, and a render never replaces a proxy; choose another Output To"),
            Source::Project => format!("{path} is this project's own file, and a render never replaces it; choose another Output To"),
        }
    }
}

/// The files the project reads: footage (every frame of a sequence), proxies and the project file.
#[derive(Default)]
pub(crate) struct Protected {
    files: Vec<(PathBuf, Source)>,
    by_path: HashMap<String, usize>,
    /// Hashes of native identities, built once. Handles are closed after indexing so long
    /// sequences do not exhaust the process's file descriptors. Hash hits are verified with
    /// native handle equality; a hash collision cannot make another file a protected source.
    #[cfg(not(target_arch = "wasm32"))]
    by_identity: std::sync::OnceLock<HashMap<u64, Vec<usize>>>,
    #[cfg(all(test, not(target_arch = "wasm32")))]
    identity_reads: std::sync::atomic::AtomicUsize,
}

impl Protected {
    pub(crate) fn of(s: &Session) -> Protected {
        let mut found = Protected::default();
        for item in s.project.items.values() {
            let name: Arc<str> = item.name.as_str().into();
            if let ItemKind::Footage(f) = &item.kind {
                found.add_footage(s, f, &Source::Footage(name.clone()));
            }
            if let Some(proxy) = &item.proxy {
                found.add_footage(s, &proxy.footage, &Source::Proxy(name));
            }
        }
        if let Some(path) = s.path.as_deref().filter(|p| !p.is_empty()) {
            found.add(PathBuf::from(path), Source::Project);
        }
        found
    }

    fn add_footage(&mut self, s: &Session, f: &Footage, source: &Source) {
        for stored in std::iter::once(&f.path).chain(&f.sequence).filter(|p| !p.is_empty()) {
            // The file the media layer opens for the path the project stores.
            let path = s.stored_path(stored).unwrap_or_else(|_| PathBuf::from(stored));
            self.add(path, source.clone());
        }
    }

    fn add(&mut self, path: PathBuf, source: Source) {
        self.by_path.entry(key(&path)).or_insert(self.files.len());
        self.files.push((path, source));
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    /// The source at this normalized path or native file identity. A new output only needs
    /// its path lookup and one failed filesystem probe; existing outputs build the index once.
    pub(crate) fn find(&self, path: &Path) -> Option<&Source> {
        self.by_path.get(&key(path)).and_then(|i| self.files.get(*i)).map(|(_, source)| source).or_else(|| self.alias_of(path))
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn handle(&self, path: &Path) -> Option<same_file::Handle> {
        #[cfg(test)]
        self.identity_reads.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        // Opening a FIFO can wait for a writer. Only regular files can be protected media;
        // preserve the old size-indexed lookup's exclusion of special files and directories.
        if !std::fs::metadata(path).ok()?.is_file() {
            return None;
        }
        same_file::Handle::from_path(path).ok()
    }

    /// Hashes narrow the search, while native handle equality also checks any hash collisions.
    #[cfg(not(target_arch = "wasm32"))]
    fn alias_of(&self, path: &Path) -> Option<&Source> {
        let output = self.handle(path)?;
        let indexed = self.by_identity.get_or_init(|| {
            let mut ids: HashMap<u64, Vec<usize>> = HashMap::new();
            for (i, (known, _)) in self.files.iter().enumerate() {
                if let Some(handle) = self.handle(known) {
                    ids.entry(identity_hash(&handle)).or_default().push(i);
                }
            }
            ids
        });
        indexed.get(&identity_hash(&output))?.iter().find_map(|i| {
            let (known, source) = self.files.get(*i)?;
            (self.handle(known)? == output).then_some(source)
        })
    }

    #[cfg(target_arch = "wasm32")]
    fn alias_of(&self, _: &Path) -> Option<&Source> {
        None
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn identity_hash(handle: &same_file::Handle) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    handle.hash(&mut hash);
    hash.finish()
}

/// `path` made absolute, with `.` and `..` resolved by name alone (no file is looked at).
fn lexical(path: &Path) -> PathBuf {
    let absolute = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let mut out = PathBuf::new();
    for part in absolute.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => match out.components().next_back() {
                Some(Component::Normal(_)) => {
                    out.pop();
                }
                Some(Component::RootDir | Component::Prefix(_)) => {}
                _ => out.push(".."),
            },
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// The name a path is compared by: absolute, resolved by name, without case where volumes ignore it.
fn key(path: &Path) -> String {
    let parts: Vec<String> = lexical(path).components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
    fold(&parts.join("/"))
}

fn fold(name: &str) -> String {
    if IGNORES_CASE { name.to_lowercase() } else { name.to_string() }
}

#[cfg(test)]
fn same_file(a: &Path, b: &Path) -> bool {
    if key(a) == key(b) {
        return true;
    }
    #[cfg(not(target_arch = "wasm32"))]
    return same_file::is_same_file(a, b).unwrap_or(false);
    #[cfg(target_arch = "wasm32")]
    false
}

/// Whether a file (or a link, even one leading nowhere) is at `path`.
fn exists(path: &str) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|m| !m.is_dir())
}

/// The files of a numbered sequence (`count` frames from `first`) that are on disk: its folder is
/// read once, not asked about frame by frame.
fn frames_on_disk(path: &str, first: i64, count: u64) -> Result<Vec<PathBuf>, String> {
    let last = first.saturating_add(i64::try_from(count).unwrap_or(i64::MAX));
    let p = Path::new(path);
    let dir = p.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let name = p.file_name().map(|n| n.to_string_lossy().into_owned());
    let Some(name) = name.filter(|_| !dir.to_string_lossy().contains('#')) else {
        // The frame number is in a folder's name: every frame has a folder of its own.
        if count > PROBE_LIMIT {
            return Err(format!(
                "cannot safely check {count} output frames with a number in a folder name (limit {PROBE_LIMIT}); choose a fixed output folder or a shorter range"
            ));
        }
        return Ok((0..count)
            .map(|k| sequence_path(path, first.saturating_add(i64::try_from(k).unwrap_or(i64::MAX))))
            .filter(|frame| exists(frame))
            .map(PathBuf::from)
            .collect());
    };
    let pattern = fold(&name);
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(format!("cannot check render output folder {}: {e}", dir.display())),
    };
    Ok(entries
        .filter_map(Result::ok)
        .filter(|e| sequence_frame(&pattern, &fold(&e.file_name().to_string_lossy())).is_some_and(|frame| (first..last).contains(&frame)))
        .filter(|e| !e.file_type().is_ok_and(|t| t.is_dir()))
        .map(|e| e.path())
        .collect())
}

/// A queue item refused before any of its output modules are dispatched.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Refused {
    pub item: u64,
    pub reason: String,
}

impl Session {
    /// The files the project reads.
    pub(crate) fn protected_files(&self) -> Protected {
        Protected::of(self)
    }

    /// Reject protected outputs and ranges that cannot be checked completely. Ordinary-file
    /// replacement consent is left to the exporter, independently of this source guard.
    pub(crate) fn check_outputs(&self, items: &[Vec<(RenderQueueItem, String)>]) -> Vec<Refused> {
        if cfg!(target_arch = "wasm32") {
            return Vec::new();
        }
        let protected = self.protected_files();
        let mut seen = HashSet::new();
        let mut refused = Vec::new();
        for (item, path) in items.iter().flatten() {
            if seen.contains(&item.id) {
                continue;
            }
            let reason = match self.check_output(&protected, item, path) {
                Ok(reason) => reason,
                Err(reason) => Some(reason),
            };
            if let Some(reason) = reason {
                seen.insert(item.id);
                refused.push(Refused { item: item.id, reason });
            }
        }
        refused
    }

    fn check_output(&self, protected: &Protected, item: &RenderQueueItem, path: &str) -> Result<Option<String>, String> {
        if protected.is_empty() {
            return Ok(None);
        }
        let sequence = item.output.format.is_sequence();
        // Skip Existing Files leaves existing frames untouched, but not the render log.
        let frames = if sequence && !item.settings.skip_existing {
            match self.project.comp(item.comp) {
                Some(comp) => frames_on_disk(path, item.settings.first_frame(comp), item.settings.frame_count(comp))?,
                None => Vec::new(),
            }
        } else {
            Vec::new()
        };
        let log = log_path(path);
        let movie = (!sequence).then_some(Path::new(path));
        Ok(frames
            .iter()
            .map(PathBuf::as_path)
            .chain(movie)
            .chain(Some(Path::new(&log)))
            .find_map(|path| protected.find(path).map(|source| source.refusal(&path.to_string_lossy()))))
    }

    /// A new item's default output must not be a file the project reads: `[compName]_1`,
    /// `[compName]_2`… take its place (and avoid the queue's other outputs), as After Effects
    /// numbers a second item of the same composition. An output nothing is in the way of keeps
    /// the composition's own name, and one the caller chose is not touched: rendering it is
    /// refused instead.
    pub(crate) fn avoid_sources(&self, item: &mut RenderQueueItem) {
        if cfg!(target_arch = "wasm32") {
            return;
        }
        let protected = self.protected_files();
        if protected.is_empty() {
            return;
        }
        let template = item.output.output.clone();
        let mut taken: Option<HashSet<String>> = None;
        'candidate: for n in 0..=MAX_NUMBER {
            item.output.output = if n == 0 { template.clone() } else { numbered_template(&template, n) };
            let Ok(outputs) = self.outputs_of(item, None) else { break };
            for (module, path) in &outputs {
                match self.check_output(&protected, module, path) {
                    Ok(Some(_)) => continue 'candidate,
                    Err(_) => break 'candidate,
                    Ok(None) => {}
                }
            }
            // Only a name that had to move is kept clear of the other items.
            let others = if n == 0 { None } else { Some(taken.get_or_insert_with(|| self.queue_output_keys())) };
            if others.is_none_or(|others| !outputs.iter().any(|(_, path)| others.contains(&key(Path::new(path))))) {
                return;
            }
        }
        item.output.output = template;
    }

    /// [`key`] of every output of the queue's items.
    fn queue_output_keys(&self) -> HashSet<String> {
        self.project.render_queue.iter().filter_map(|it| self.outputs_of(it, None).ok()).flatten().map(|(_, path)| key(Path::new(&path))).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn unrelated_same_size_frames_are_indexed_once() {
        let dir = std::env::temp_dir().join(format!("effectcraft-identity-index-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut protected = Protected::default();
        for i in 0..128 {
            let source = dir.join(format!("source{i}")).join("plate.png");
            let output = dir.join(format!("output{i}")).join("plate.png");
            for path in [&source, &output] {
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                std::fs::write(path, b"same-size frame").unwrap();
            }
            protected.add(source, Source::Footage(format!("plate{i}").into()));
        }
        for i in 0..128 {
            assert!(protected.find(&dir.join(format!("output{i}")).join("plate.png")).is_none());
        }
        assert_eq!(protected.identity_reads.load(std::sync::atomic::Ordering::Relaxed), 256, "one source-index pass plus one lookup per output");
        let alias = dir.join("renamed.png");
        std::fs::hard_link(dir.join("source64").join("plate.png"), &alias).unwrap();
        assert_eq!(protected.find(&alias), Some(&Source::Footage("plate64".into())));
        assert_eq!(protected.identity_reads.load(std::sync::atomic::Ordering::Relaxed), 258, "native equality verifies a matching identity");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn folder_numbered_sequences_are_never_partially_checked() {
        let pattern = std::env::temp_dir().join("frames[######]").join("plate.png").to_string_lossy().into_owned();
        let error = frames_on_disk(&pattern, 0, PROBE_LIMIT + 1).unwrap_err();
        assert!(error.contains("cannot safely check") && error.contains("100000"), "{error}");
        assert!(frames_on_disk(&pattern, 0, u64::MAX).is_err());
    }

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn non_file_outputs_do_not_build_the_identity_index() {
        let protected = Protected::default();
        assert!(protected.find(&std::env::temp_dir()).is_none());
        assert!(protected.by_identity.get().is_none());
    }

    #[test]
    fn paths_compare_by_name_once_resolved() {
        let here = std::env::current_dir().unwrap();
        assert_eq!(key(Path::new("a/b/../c.mp4")), key(Path::new("a/c.mp4")));
        assert_eq!(key(Path::new("./a/c.mp4")), key(&here.join("a/c.mp4")));
        assert_ne!(key(Path::new("a/c.mp4")), key(Path::new("b/c.mp4")));
        assert_eq!(key(Path::new(&format!("{}x", "../".repeat(64)))), key(Path::new(&format!("{}x", "../".repeat(80)))), "past the root stays at the root");
        assert_eq!(IGNORES_CASE, key(Path::new("A/C.MP4")) == key(Path::new("a/c.mp4")));
    }

    #[test]
    fn a_file_is_the_same_file_under_every_spelling_the_volume_allows() {
        let dir = std::env::temp_dir().join(format!("effectcraft-output-guard-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("clip.mov"), b"x").unwrap();
        std::fs::write(dir.join("other.mov"), b"y").unwrap();
        assert!(same_file(&dir.join("clip.mov"), &dir.join("sub").join("..").join("clip.mov")));
        assert!(!same_file(&dir.join("clip.mov"), &dir.join("other.mov")));
        // Nothing there yet: only the name decides.
        assert!(same_file(&dir.join("gone.mov"), &dir.join("sub").join("..").join("gone.mov")));
        assert!(!same_file(&dir.join("gone.mov"), &dir.join("clip.mov")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_sequences_folder_is_read_once_for_its_frames() {
        let dir = std::env::temp_dir().join(format!("effectcraft-output-frames-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("a_00003.png")).unwrap(); // a folder is not a frame
        for name in ["a_00001.png", "a_00002.png", "a_0004.png", "a_00004.jpg", "b_00002.png", "a_00009.png"] {
            std::fs::write(dir.join(name), b"x").unwrap();
        }
        let pattern = dir.join("a_[#####].png").to_string_lossy().into_owned();
        let names = |frames: Vec<PathBuf>| {
            let mut names: Vec<String> = frames.iter().filter_map(|f| f.file_name()).map(|n| n.to_string_lossy().into_owned()).collect();
            names.sort();
            names
        };
        assert_eq!(
            names(frames_on_disk(&pattern, 0, 5).unwrap()),
            ["a_00001.png", "a_00002.png"],
            "frames 0-4: not the 4-digit one, the jpg, the folder or frame 9"
        );
        assert_eq!(names(frames_on_disk(&pattern, 2, 1).unwrap()), ["a_00002.png"]);
        assert_eq!(names(frames_on_disk(&pattern, 9, 100).unwrap()), ["a_00009.png"]);
        assert_eq!(names(frames_on_disk(&pattern, 0, u64::MAX).unwrap()), ["a_00001.png", "a_00002.png", "a_00009.png"], "a hostile frame count is not walked");
        // A frame number in a folder's name: each frame has its own folder.
        std::fs::create_dir_all(dir.join("f02")).unwrap();
        std::fs::write(dir.join("f02").join("x.png"), b"x").unwrap();
        let in_folder = dir.join("f[##]").join("x.png").to_string_lossy().into_owned();
        assert_eq!(names(frames_on_disk(&in_folder, 0, 4).unwrap()), ["x.png"]);
        // A folder that is not there holds nothing.
        assert!(frames_on_disk(&dir.join("none").join("a_[#####].png").to_string_lossy(), 0, 5).unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
