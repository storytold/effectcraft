//! Which files a render may write.
//!
//! A render never replaces a file the project reads (footage, a proxy, the project file itself:
//! [`Protected`]), and replaces any other file only when asked to (`overwrite`). Both are decided
//! from the paths the queue would render to, before anything is written
//! ([`Session::check_outputs`]): `renderQueue.checkOutputs` reports them, `renderQueue.render`
//! refuses on them, and the desktop app asks Replace or Cancel first (`panels::overwrite`).
//!
//! Files are compared as the file system sees them: through links, and without regard to case
//! where the volume ignores it (Windows, macOS). Footage that is not on disk is compared by name
//! alone. A hard link to footage under another name is the same file on disk, and is recognised by
//! the volume's own identity for it, asked only of an existing output of the size of some footage (a
//! render to a new file costs one look). Nothing stops a file from appearing between the check and
//! the write. The browser build downloads what it renders and has no files to replace, so it is not
//! checked.

use std::collections::{HashMap, HashSet};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use effectcraft_project::render_queue::{RenderQueueItem, log_path, numbered_template, sequence_frame, sequence_path};
use effectcraft_project::{Footage, ItemKind};
use serde_json::{Value, json};

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
    /// `footage`, `proxy` or `project`: the `source` of a blocked output.
    pub(crate) fn kind(&self) -> &'static str {
        match self {
            Source::Footage(_) => "footage",
            Source::Proxy(_) => "proxy",
            Source::Project => "project",
        }
    }

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
    /// Indexes into `files` by lower-cased file name: the only files another path can be the same
    /// as, short of a link.
    by_name: HashMap<String, Vec<usize>>,
    /// Indexes into `files` of the ones that exist, by size, made when first needed: a hard link has the size of
    /// the file it leads to, so only files of that size are asked whether they are the same file.
    #[cfg(not(target_arch = "wasm32"))]
    by_size: std::sync::OnceLock<HashMap<u64, Vec<usize>>>,
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
        let Some(name) = path.file_name().map(|n| n.to_string_lossy().to_lowercase()) else { return };
        self.by_name.entry(name).or_default().push(self.files.len());
        self.files.push((path, source));
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    /// What reads the file at `path`, if the project does. The same file under another spelling
    /// or through a link counts.
    pub(crate) fn find(&self, path: &Path) -> Option<&Source> {
        if self.is_empty() {
            return None;
        }
        let same = |candidate: &Path| self.named(candidate).find(|(known, _)| same_file(candidate, known)).map(|(_, source)| source);
        same(path)
            .or_else(|| {
                // A link is another name for what it leads to.
                let is_link = std::fs::symlink_metadata(path).ok()?.file_type().is_symlink();
                if is_link { same(&std::fs::canonicalize(path).ok()?) } else { None }
            })
            .or_else(|| self.alias_of(path))
    }

    /// What reads the file at `path` under another name: a hard link, which only the volume can tell is the same file.
    /// Only a file that exists is asked about, and only against the known files of its size.
    #[cfg(not(target_arch = "wasm32"))]
    fn alias_of(&self, path: &Path) -> Option<&Source> {
        let size = std::fs::metadata(path).ok().filter(|m| m.is_file())?.len();
        let by_size = self.by_size.get_or_init(|| {
            let mut sizes: HashMap<u64, Vec<usize>> = HashMap::new();
            for (i, (known, _)) in self.files.iter().enumerate() {
                if let Ok(meta) = std::fs::metadata(known)
                    && meta.is_file()
                {
                    sizes.entry(meta.len()).or_default().push(i);
                }
            }
            sizes
        });
        let candidates = by_size.get(&size)?;
        candidates.iter().filter_map(|i| self.files.get(*i)).find(|(known, _)| same_file::is_same_file(path, known).unwrap_or(false)).map(|(_, source)| source)
    }

    /// The browser build has no files to compare.
    #[cfg(target_arch = "wasm32")]
    fn alias_of(&self, _: &Path) -> Option<&Source> {
        None
    }

    /// The files whose name is `path`'s, whatever its case.
    fn named<'a>(&'a self, path: &Path) -> impl Iterator<Item = &'a (PathBuf, Source)> + 'a {
        let name = path.file_name().map(|n| n.to_string_lossy().to_lowercase());
        name.and_then(|n| self.by_name.get(&n)).into_iter().flatten().filter_map(|i| self.files.get(*i))
    }
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

/// What a file is known by on its volume, when it exists.
fn identity(path: &Path) -> Option<String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let meta = std::fs::metadata(path).ok()?;
        Some(format!("{}:{}", meta.dev(), meta.ino()))
    }
    #[cfg(not(unix))]
    {
        // The final path: links, junctions, short names and spelling resolved.
        std::fs::canonicalize(path).ok().map(|real| real.to_string_lossy().to_lowercase())
    }
}

/// Whether two paths are one file: the same name once resolved, or the same file on disk.
fn same_file(a: &Path, b: &Path) -> bool {
    key(a) == key(b) || matches!((identity(a), identity(b)), (Some(x), Some(y)) if x == y)
}

/// Whether a file (or a link, even one leading nowhere) is at `path`.
fn exists(path: &str) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|m| !m.is_dir())
}

/// The files of a numbered sequence (`count` frames from `first`) that are on disk: its folder is
/// read once, not asked about frame by frame.
fn frames_on_disk(path: &str, first: i64, count: u64) -> Vec<PathBuf> {
    let last = first.saturating_add(i64::try_from(count).unwrap_or(i64::MAX));
    let p = Path::new(path);
    let dir = p.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let name = p.file_name().map(|n| n.to_string_lossy().into_owned());
    let Some(name) = name.filter(|_| !dir.to_string_lossy().contains('#')) else {
        // The frame number is in a folder's name: every frame has a folder of its own.
        return (0..count.min(PROBE_LIMIT))
            .map(|k| sequence_path(path, first.saturating_add(i64::try_from(k).unwrap_or(i64::MAX))))
            .filter(|frame| exists(frame))
            .map(PathBuf::from)
            .collect();
    };
    let pattern = fold(&name);
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    entries
        .filter_map(Result::ok)
        .filter(|e| sequence_frame(&pattern, &fold(&e.file_name().to_string_lossy())).is_some_and(|frame| (first..last).contains(&frame)))
        .filter(|e| !e.file_type().is_ok_and(|t| t.is_dir()))
        .map(|e| e.path())
        .collect()
}

/// An output that would replace files that exist.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ExistingOutput {
    pub item: u64,
    /// The item's place in the queue (1-based).
    pub index: usize,
    pub comp: String,
    /// The output path (a sequence's has the frame number pattern).
    pub path: String,
    /// How many files it replaces (a sequence's frames that exist).
    pub files: u64,
    pub sequence: bool,
}

/// An output that would replace a file the project reads.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct BlockedOutput {
    pub item: u64,
    pub index: usize,
    pub comp: String,
    /// The file in the way (the frame of a sequence that is footage).
    pub path: String,
    /// [`Source::kind`].
    pub source: &'static str,
    pub reason: String,
}

/// What a set of outputs would do to the files on disk.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct OutputCheck {
    pub existing: Vec<ExistingOutput>,
    pub blocked: Vec<BlockedOutput>,
}

impl OutputCheck {
    /// `renderQueue.checkOutputs`' reply.
    pub(crate) fn to_json(&self) -> Value {
        let existing: Vec<Value> = self
            .existing
            .iter()
            .map(|e| json!({"item": e.item, "index": e.index, "comp": e.comp, "path": e.path, "files": e.files, "sequence": e.sequence}))
            .collect();
        let blocked: Vec<Value> = self
            .blocked
            .iter()
            .map(|b| json!({"item": b.item, "index": b.index, "comp": b.comp, "path": b.path, "source": b.source, "reason": b.reason}))
            .collect();
        json!({"existing": existing, "blocked": blocked})
    }

    /// Why a render that may not replace files is refused, when files exist.
    pub(crate) fn existing_refusal(&self) -> Option<String> {
        let first = self.existing.first()?;
        let files: u64 = self.existing.iter().map(|e| e.files).sum();
        let what = if self.existing.len() == 1 && !first.sequence {
            format!("{} already exists", first.path)
        } else {
            format!("{files} output files already exist (first: {})", first.path)
        };
        Some(format!("{what}. Nothing was rendered; pass \"overwrite\": true to replace existing output files"))
    }

    /// The items that cannot be rendered, once each, with the reason of their first blocked output.
    pub(crate) fn refusals(&self) -> Vec<crate::render_queue::Refused> {
        let mut seen = HashSet::new();
        self.blocked
            .iter()
            .filter(|b| seen.insert(b.item))
            .map(|b| crate::render_queue::Refused { item: b.item, path: b.path.clone(), reason: b.reason.clone() })
            .collect()
    }
}

impl Session {
    /// The files the project reads.
    pub(crate) fn protected_files(&self) -> Protected {
        Protected::of(self)
    }

    /// `Err` with the reason when `path` is a file the project reads.
    pub(crate) fn refuse_source(&self, path: &str) -> Result<(), String> {
        if cfg!(target_arch = "wasm32") {
            return Ok(());
        }
        match self.protected_files().find(Path::new(path)) {
            Some(source) => Err(source.refusal(path)),
            None => Ok(()),
        }
    }

    /// What rendering the queued items now would do to the files on disk.
    pub(crate) fn output_check(&self) -> OutputCheck {
        let formats = self.exporter.as_ref().map(|e| e.formats()).unwrap_or_default();
        let (items, _) = self.queued_outputs(&formats);
        self.check_outputs(&items)
    }

    /// What rendering `items` (the outputs [`Session::queued_outputs`] gives) would do to the
    /// files on disk.
    pub(crate) fn check_outputs(&self, items: &[Vec<(RenderQueueItem, String)>]) -> OutputCheck {
        let mut check = OutputCheck::default();
        if cfg!(target_arch = "wasm32") {
            return check;
        }
        let protected = self.protected_files();
        for (m, path) in items.iter().flatten() {
            self.check_output(&protected, m, path, &mut check);
        }
        check
    }

    fn check_output(&self, protected: &Protected, m: &RenderQueueItem, path: &str, check: &mut OutputCheck) {
        let comp = self.project.item(m.comp).map(|i| i.name.clone()).unwrap_or_default();
        let index = self.project.render_queue.iter().position(|i| i.id == m.id).map_or(0, |i| i + 1);
        let sequence = m.output.format.is_sequence();
        // The files this output replaces, if they are there. A sequence told to skip the frames
        // that exist leaves them alone.
        let there: Vec<PathBuf> = if !sequence {
            exists(path).then(|| PathBuf::from(path)).into_iter().collect()
        } else if m.settings.skip_existing {
            Vec::new()
        } else {
            self.project.comp(m.comp).map(|c| frames_on_disk(path, m.settings.first_frame(c), m.settings.frame_count(c))).unwrap_or_default()
        };
        // A movie is compared by name even when nothing is there yet (footage on a drive that is
        // not plugged in); so is the render log next to it.
        let log = log_path(path);
        let by_name = (!sequence).then_some(Path::new(path));
        let hit = there.iter().map(PathBuf::as_path).chain(by_name).chain(Some(Path::new(&log))).find_map(|p| protected.find(p).map(|source| (p, source)));
        if let Some((blocked, source)) = hit {
            let shown = blocked.to_string_lossy().into_owned();
            check.blocked.push(BlockedOutput { item: m.id, index, comp, source: source.kind(), reason: source.refusal(&shown), path: shown });
        } else if !there.is_empty() {
            check.existing.push(ExistingOutput { item: m.id, index, comp, path: path.to_string(), files: there.len() as u64, sequence });
        }
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
        for n in 0..=MAX_NUMBER {
            item.output.output = if n == 0 { template.clone() } else { numbered_template(&template, n) };
            let Ok(outputs) = self.outputs_of(item, None) else { break };
            let mut check = OutputCheck::default();
            for (m, path) in &outputs {
                self.check_output(&protected, m, path, &mut check);
            }
            if !check.blocked.is_empty() {
                continue;
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
        assert_eq!(names(frames_on_disk(&pattern, 0, 5)), ["a_00001.png", "a_00002.png"], "frames 0-4: not the 4-digit one, the jpg, the folder or frame 9");
        assert_eq!(names(frames_on_disk(&pattern, 2, 1)), ["a_00002.png"]);
        assert_eq!(names(frames_on_disk(&pattern, 9, 100)), ["a_00009.png"]);
        assert_eq!(names(frames_on_disk(&pattern, 0, u64::MAX)), ["a_00001.png", "a_00002.png", "a_00009.png"], "a hostile frame count is not walked");
        // A frame number in a folder's name: each frame has its own folder.
        std::fs::create_dir_all(dir.join("f02")).unwrap();
        std::fs::write(dir.join("f02").join("x.png"), b"x").unwrap();
        let in_folder = dir.join("f[##]").join("x.png").to_string_lossy().into_owned();
        assert_eq!(names(frames_on_disk(&in_folder, 0, 4)), ["x.png"]);
        // A folder that is not there holds nothing.
        assert!(frames_on_disk(&dir.join("none").join("a_[#####].png").to_string_lossy(), 0, 5).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
