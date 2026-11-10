//! Protect files the project reads from render outputs (#1743).
//!
//! This guard is independent of replacement consent and export staging: it never authorizes
//! overwriting ordinary outputs or opens a second confirmation flow. Those belong to the
//! exporter. Existing aliases are compared by native file identity, including hard links.
//! Missing sources remain protected by their normalized path. Checks happen before dispatch;
//! filesystem changes between the check and the write require exporter-side revalidation.
//!
//! The commands that write to a path they are given (Save Frame As, Export Lottie, Save a Copy,
//! Collect Files, Export Timeline…) never replace one of those files either, with no flag:
//! they write through [`Session::write_output`], the one place that checks. Save and Save As
//! write the project's own file and do not.
//!
//! Nor does a render write one file twice. When two outputs of one job name the same file (two
//! items with one Output To, a duplicated item, output modules or segments that meet), the one
//! rendered later replaces what the earlier one just wrote, and nothing is on disk to say so when
//! the job starts. [`Session::duplicate_outputs`] lists them and the whole render is refused before
//! anything is written: no answer makes a render replacing its own output right. A numbered sequence is compared by the frames
//! it writes: two items of one pattern clash only where their frames do, and an item that skips
//! existing frames leaves the earlier one's alone. A new item's default output name steps aside
//! the same way it does for footage ([`Session::avoid_sources`]).
//!

use std::collections::{HashMap, HashSet};
#[cfg(not(target_arch = "wasm32"))]
use std::path::Component;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use effectcraft_project::render_queue::{RenderQueueItem, log_path, numbered_template, sequence_frame, sequence_parts, sequence_path};
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

/// What a file is written for, which words a refusal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Use {
    /// A Render Queue output (or a preview render): the fix is Output To.
    Render,
    /// Anything else written to a path: a frame, an export, a copy.
    Export,
}

/// Stands for the frame number in the name a numbered sequence is compared by: a character no
/// file is named with.
const FRAME: char = '\u{e000}';

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

    /// Why `path`, a file this reads, cannot be written for `used`.
    pub(crate) fn refusal(&self, path: &str, used: Use) -> String {
        match (used, self) {
            (Use::Render, Source::Footage(name)) => {
                format!("{path} is the footage “{name}” of this project, and a render never replaces footage; choose another Output To")
            }
            (Use::Render, Source::Proxy(name)) => format!("{path} is the proxy of “{name}”, and a render never replaces a proxy; choose another Output To"),
            (Use::Render, Source::Project) => format!("{path} is this project's own file, and a render never replaces it; choose another Output To"),
            (Use::Export, Source::Footage(name)) => {
                format!("{path} is the footage “{name}” of this project, and footage is never written over; choose another file")
            }
            (Use::Export, Source::Proxy(name)) => format!("{path} is the proxy of “{name}”, and a proxy is never written over; choose another file"),
            (Use::Export, Source::Project) => format!("{path} is this project's own file, which only Save writes; choose another file"),
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

/// What an existing regular file is known by on its volume (links and hard links to one file give one),
/// `None` when it does not exist, is not a regular file, or the volume has no such identity.
fn identity(path: &Path) -> Option<String> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        // Opening a FIFO can wait for a writer: only regular files are looked at.
        if !std::fs::metadata(path).ok()?.is_file() {
            return None;
        }
        same_file::Handle::from_path(path).ok().map(|handle| identity_hash(&handle).to_string())
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = path;
        None
    }
}

/// `path` made absolute, with `.` and `..` resolved by name alone (no file is looked at).
#[cfg(not(target_arch = "wasm32"))]
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
#[cfg(not(target_arch = "wasm32"))]
fn key(path: &Path) -> String {
    let parts: Vec<String> = lexical(path).components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
    fold(&parts.join("/"))
}

#[cfg(target_arch = "wasm32")]
fn key(path: &Path) -> String {
    browser_key(path)
}

/// Match the browser file table: one optional leading slash, with literal case and dot segments.
#[cfg(any(target_arch = "wasm32", test))]
fn browser_key(path: &Path) -> String {
    let path = path.to_string_lossy();
    if path.starts_with('/') { path.into_owned() } else { format!("/{path}") }
}

fn fold(name: &str) -> String {
    if IGNORES_CASE { name.to_lowercase() } else { name.to_string() }
}

/// Whether two paths are one file: the same name once resolved, or the same file on disk.
pub(crate) fn same_file(a: &Path, b: &Path) -> bool {
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

/// What an output writes, as far as a clash with another output goes.
#[derive(Clone, Debug, PartialEq)]
enum Target {
    /// One file: known by its name and, when it exists, by what it is on its volume.
    File { key: String, identity: Option<String> },
    /// The numbered files `first..end` of a sequence: `key` is its pattern, with the width the
    /// frame numbers are padded to.
    Frames { key: String, width: usize, first: i64, end: i64 },
}

impl Target {
    /// The file at `path`. `by_identity` also looks up what it is on its volume (a link to it
    /// is it); that asks the file system, so it is left out where only names matter.
    fn file(path: &Path, by_identity: bool) -> Target {
        let identity = if by_identity { identity(path) } else { None };
        Target::File { key: key(path), identity }
    }

    /// The `count` files of the sequence `path` (its frame-number pattern) from frame `first`.
    fn frames(path: &str, first: i64, count: u64) -> Target {
        let (before, width, after) = sequence_parts(path);
        let key = key(Path::new(&format!("{before}{FRAME}{after}")));
        Target::Frames { key, width, first, end: first.saturating_add(i64::try_from(count).unwrap_or(i64::MAX)) }
    }
}

/// One output of a job.
#[derive(Clone, Debug)]
struct Write {
    /// The queue item (`0`: one that is not in the queue yet).
    item: u64,
    /// The output path (a sequence's has its frame-number pattern).
    path: String,
    target: Target,
    /// Skip Existing Files: a frame that is there already is left alone.
    skips: bool,
}

/// What an output rendered later writes over.
struct Clash<'a> {
    /// The earlier output that writes it.
    earlier: &'a Write,
    /// How many files, and the first of them (its frame number, for a sequence).
    files: u64,
    first: i64,
}

/// The outputs of a job in the order they render, to find the one that writes what an earlier
/// one does.
#[derive(Default)]
struct Writes {
    all: Vec<Write>,
    path_bytes: usize,
    /// The first output to write a file, by name.
    files: HashMap<String, usize>,
    /// ... and by what the file is on its volume.
    identities: HashMap<String, usize>,
}

/// The common range whose formatted numbers agree. Once the number (including a minus sign)
/// fills the larger padding width, different widths produce the same file name.
fn common_frames(first: i64, end: i64, from: i64, to: i64, width: usize, other_width: usize) -> Option<(u64, i64)> {
    let (start, end) = (first.max(from), end.min(to));
    let count = |a: i64, b: i64| u64::try_from(b.saturating_sub(a)).unwrap_or(0);
    if width == other_width {
        return (end > start).then(|| (count(start, end), start));
    }
    let width = width.max(other_width);
    let power = |digits: usize| u32::try_from(digits).ok().and_then(|digits| 10i64.checked_pow(digits));
    let negative = power(width.saturating_sub(2)).map(|n| end.min((-n).saturating_add(1))).filter(|to| *to > start);
    let positive = power(width.saturating_sub(1)).map(|n| start.max(n)).filter(|from| *from < end);
    let files = negative.map_or(0, |to| count(start, to)).saturating_add(positive.map_or(0, |from| count(from, end)));
    (files > 0).then(|| (files, if negative.is_some() { start } else { positive.unwrap_or(start) }))
}

impl Writes {
    fn is_empty(&self) -> bool {
        self.all.is_empty()
    }

    /// The earliest output `later`, rendered after all of these, writes over.
    fn clash(&self, later: &Write) -> Option<Clash<'_>> {
        match &later.target {
            Target::File { key, identity } => {
                if let Some(at) = self.files.get(key).or_else(|| identity.as_ref().and_then(|id| self.identities.get(id))) {
                    return Some(Clash { earlier: self.all.get(*at)?, files: 1, first: 0 });
                }
                self.all.iter().find_map(|earlier| {
                    let Target::Frames { first, end, .. } = earlier.target else { return None };
                    let frame = sequence_frame(&crate::output_guard::key(Path::new(&earlier.path)), key)?;
                    (first..end).contains(&frame).then_some(Clash { earlier, files: 1, first: frame })
                })
            }
            // A frame that is there already is left alone, so a later output that skips existing
            // frames replaces none of an earlier one's.
            Target::Frames { .. } if later.skips => None,
            Target::Frames { key, width, first, end } => self.all.iter().find_map(|earlier| {
                let (files, start) = match &earlier.target {
                    Target::Frames { key: other, width: other_width, first: from, end: to } if key == other => {
                        common_frames(*first, *end, *from, *to, *width, *other_width)?
                    }
                    Target::File { key, .. } => {
                        let frame = sequence_frame(&crate::output_guard::key(Path::new(&later.path)), key)?;
                        if !(*first..*end).contains(&frame) {
                            return None;
                        }
                        (1, frame)
                    }
                    _ => return None,
                };
                Some(Clash { earlier, files, first: start })
            }),
        }
    }

    fn push(&mut self, write: Write) -> Result<(), String> {
        let path_bytes = self.path_bytes.saturating_add(write.path.len());
        if self.all.len() >= 65_536 || path_bytes > 16 * 1024 * 1024 {
            return Err("render output plan exceeds 65536 paths or 16 MiB of path names; split the queue".into());
        }
        self.path_bytes = path_bytes;
        let at = self.all.len();
        match &write.target {
            Target::File { key, identity } => {
                self.files.entry(key.clone()).or_insert(at);
                if let Some(identity) = identity {
                    self.identities.entry(identity.clone()).or_insert(at);
                }
            }
            Target::Frames { .. } => {}
        }
        self.all.push(write);
        Ok(())
    }
}

/// A queue item refused before any of its output modules are dispatched.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Refused {
    pub item: u64,
    pub reason: String,
}

/// An output that would write a file an earlier output of the same job writes.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct DuplicateOutput {
    /// The queue item that would write over it (the later one), its place in the queue (1-based)
    /// and its composition.
    pub item: u64,
    pub index: usize,
    pub comp: String,
    /// The file: a sequence's pattern, or the frame when it is one.
    pub path: String,
    /// How many files the two outputs have in common.
    pub files: u64,
    pub sequence: bool,
    /// The queue item whose output is written first, and its place and composition.
    pub with: u64,
    pub with_index: usize,
    pub with_comp: String,
}

impl DuplicateOutput {
    /// Why the render is refused, naming the two items.
    pub(crate) fn reason(&self) -> String {
        let label = |index: usize, comp: &str| format!("#{index} “{comp}”");
        let what = if self.sequence && self.files != 1 { format!("{} frames of {}", self.files, self.path) } else { self.path.clone() };
        let rule = "and a render never writes one file twice; choose another Output To for one of them";
        if self.item == self.with {
            format!("{} would write {what} twice (two of its output modules), {rule}", label(self.index, &self.comp))
        } else {
            format!("{} and {} would both write {what}, {rule}", label(self.with_index, &self.with_comp), label(self.index, &self.comp))
        }
    }
}

/// Why a render is refused when outputs of the job write one file twice: it would replace its own output.
pub(crate) fn duplicate_refusal(duplicates: &[DuplicateOutput]) -> Option<String> {
    let first = duplicates.first()?;
    let more = match duplicates.len().saturating_sub(1) {
        0 => String::new(),
        1 => "; 1 more output would write a file twice too".into(),
        n => format!("; {n} more outputs would write a file twice too"),
    };
    Some(format!("{}. Nothing was rendered{more}", first.reason()))
}

impl Session {
    /// The files the project reads.
    pub(crate) fn protected_files(&self) -> Protected {
        Protected::of(self)
    }

    /// What reads the file at `path`, if the project does.
    fn blocking(&self, path: &str) -> Option<Source> {
        self.protected_files().find(Path::new(path)).cloned()
    }

    /// `Err` when `path` is a file the project reads (footage, a proxy, the project file) and so
    /// is not for an export to replace. For callers that write the file themselves; call it
    /// before the work, not after it.
    pub fn refuse_export(&self, path: &str) -> crate::Result<()> {
        match self.blocking(path) {
            Some(source) => Err(crate::EngineError::Other(source.refusal(path, Use::Export))),
            None => Ok(()),
        }
    }

    /// Write `bytes` to `path` through the host's services, unless the project reads the file
    /// there ([`Session::refuse_export`]). Every command that writes to a path it is given
    /// writes through this; Save and Save As (the project's own file) do not.
    pub fn write_output(&self, path: &str, bytes: &[u8]) -> crate::Result<()> {
        self.write_output_with(&self.protected_files(), path, bytes)
    }

    /// [`Session::write_output`] against files already found ([`Session::protected_files`]),
    /// for a command that writes many.
    pub(crate) fn write_output_with(&self, protected: &Protected, path: &str, bytes: &[u8]) -> crate::Result<()> {
        if let Some(source) = protected.find(Path::new(path)) {
            return Err(crate::EngineError::Other(source.refusal(path, Use::Export)));
        }
        self.services.write_file(path, bytes).map_err(|e| crate::EngineError::Other(format!("cannot write {path}: {e}")))
    }

    /// `file.checkOutput`'s reply for `path`.
    pub(crate) fn output_status(&self, path: &str) -> Value {
        match self.blocking(path) {
            Some(source) => json!({"path": path, "blocked": true, "source": source.kind(), "reason": source.refusal(path, Use::Export)}),
            None => json!({"path": path, "blocked": false}),
        }
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
            let reason = match self.write_paths(item, path) {
                Ok(paths) => paths.iter().find_map(|path| match self.check_output(&protected, item, path) {
                    Ok(reason) => reason,
                    Err(reason) => Some(reason),
                }),
                Err(reason) => Some(reason),
            };
            if let Some(reason) = reason {
                seen.insert(item.id);
                refused.push(Refused { item: item.id, reason });
            }
        }
        refused
    }

    /// What the job writes more than once. An item in `refused` writes nothing, so it takes no part.
    pub(crate) fn duplicate_outputs(&self, items: &[Vec<(RenderQueueItem, String)>], refused: &[Refused]) -> Result<Vec<DuplicateOutput>, String> {
        let skip: HashSet<u64> = refused.iter().map(|r| r.item).collect();
        let mut writes = Writes::default();
        let mut duplicates = Vec::new();
        for (m, path) in items.iter().flatten().filter(|(m, _)| !skip.contains(&m.id)) {
            let paths = self.write_paths(m, path)?;
            // Alternative destinations of one output cannot collide with themselves.
            for path in &paths {
                let write = self.write_of(m, path, true);
                if let Some(clash) = writes.clash(&write) {
                    duplicates.push(self.duplicate_of(&write, &clash));
                    break;
                }
            }
            for path in paths {
                writes.push(self.write_of(m, &path, true))?;
            }
        }
        Ok(duplicates)
    }

    /// Every possible destination, since quota decisions can change during rendering.
    fn write_paths<'a>(&self, item: &RenderQueueItem, path: &'a str) -> Result<Vec<std::borrow::Cow<'a, str>>, String> {
        let overflow = &self.project.render_prefs.overflow_folders;
        const PATH_BYTES: usize = 1024 * 1024;
        if path.len() > PATH_BYTES {
            return Err("render output destinations exceed 1 MiB of path names".into());
        }
        let mut paths = vec![std::borrow::Cow::Borrowed(path)];
        let mut bytes = path.len();
        if item.settings.storage_overflow && self.storage_quota.is_some() {
            if overflow.len() > 256 {
                return Err("render output planning supports at most 256 storage overflow folders".into());
            }
            if let Some(name) = Path::new(path).file_name() {
                for folder in overflow {
                    if bytes.saturating_add(folder.len()).saturating_add(name.len()).saturating_add(1) > PATH_BYTES {
                        return Err("render output destinations exceed 1 MiB of path names".into());
                    }
                    let destination = Path::new(folder).join(name).to_string_lossy().into_owned();
                    bytes = bytes.saturating_add(destination.len());
                    if bytes > PATH_BYTES {
                        return Err("render output destinations exceed 1 MiB of path names".into());
                    }
                    paths.push(std::borrow::Cow::Owned(destination));
                }
            }
        }
        Ok(paths)
    }

    /// What `path`, an output of `m`, writes. `by_identity`: see [`Target::file`].
    fn write_of(&self, m: &RenderQueueItem, path: &str, by_identity: bool) -> Write {
        let target = if m.output.format.is_sequence() {
            let (first, count) = self.project.comp(m.comp).map_or((0, 0), |c| (m.settings.first_frame(c), m.settings.frame_count(c)));
            Target::frames(path, first, count)
        } else {
            Target::file(Path::new(path), by_identity)
        };
        let skips = m.settings.skip_existing && self.exporter.as_ref().is_some_and(|exporter| exporter.supports_skip_existing(&m.settings));
        Write { item: m.id, path: path.to_string(), target, skips }
    }

    /// The report of `later` writing over `clash`.
    fn duplicate_of(&self, later: &Write, clash: &Clash) -> DuplicateOutput {
        // Where a queue item stands, and what it renders.
        let place = |id: u64| {
            let index = self.project.render_queue.iter().position(|i| i.id == id);
            let comp = index.and_then(|k| self.project.render_queue.get(k)).and_then(|i| self.project.item(i.comp)).map(|i| i.name.clone());
            (index.map_or(0, |k| k + 1), comp.unwrap_or_default())
        };
        let sequence = matches!(later.target, Target::Frames { .. });
        // One shared frame is named by its file, more by the pattern.
        let path = if sequence && clash.files == 1 { sequence_path(&later.path, clash.first) } else { later.path.clone() };
        let (index, comp) = place(later.item);
        let (with_index, with_comp) = place(clash.earlier.item);
        DuplicateOutput { item: later.item, index, comp, path, files: clash.files, sequence, with: clash.earlier.item, with_index, with_comp }
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
            .find_map(|path| protected.find(path).map(|source| source.refusal(&path.to_string_lossy(), Use::Render))))
    }

    /// A new item's default output must not be a file the project reads, nor one another item of
    /// the queue writes (any of its output modules, whether or not it is ticked): `[compName]_1`,
    /// `[compName]_2`… take its place, as After Effects numbers a second item of the same
    /// composition. An output nothing is in the way of keeps the composition's own name; the name
    /// is stored in the item, so it stays as it is when the item it avoided goes away. One the
    /// caller chose is not touched: rendering it is refused instead ([`Session::duplicate_outputs`]).
    pub(crate) fn avoid_sources(&self, item: &mut RenderQueueItem) {
        // Native source protection stays with its filesystem guard; virtual web outputs still
        // need distinct queue names because the browser stores completed files by path.
        let protected = if cfg!(target_arch = "wasm32") { Protected::default() } else { self.protected_files() };
        let Ok(queue) = self.queue_writes() else { return };
        if protected.is_empty() && queue.is_empty() {
            return;
        }
        let template = item.output.output.clone();
        for n in 0..=MAX_NUMBER {
            item.output.output = if n == 0 { template.clone() } else { numbered_template(&template, n) };
            let Ok(outputs) = self.outputs_of(item, None) else { break };
            let mut in_the_way = false;
            for (module, path) in &outputs {
                let Ok(paths) = self.write_paths(module, path) else {
                    item.output.output = template;
                    return;
                };
                for destination in &paths {
                    match self.check_output(&protected, module, destination) {
                        Ok(Some(_)) => in_the_way = true,
                        Err(_) => {
                            item.output.output = template;
                            return;
                        }
                        Ok(None) => {}
                    }
                }
                in_the_way |= paths.iter().any(|path| queue.clash(&self.write_of(module, path, false)).is_some());
            }
            if !in_the_way {
                return;
            }
        }
        item.output.output = template;
    }

    /// What the items of the queue write (by name: a new item is not compared with what is on
    /// disk).
    fn queue_writes(&self) -> Result<Writes, String> {
        let mut writes = Writes::default();
        for outputs in self.project.render_queue.iter().filter_map(|it| self.outputs_of(it, None).ok()) {
            for (m, path) in &outputs {
                for path in self.write_paths(m, path)? {
                    writes.push(self.write_of(m, &path, false))?;
                }
            }
        }
        Ok(writes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Portable path checks protect virtual outputs too; they do not need native file identity.
    #[test]
    fn duplicate_queue_paths_are_reported_without_native_files() {
        let mut s = Session::default();
        s.execute("comp.new", json!({"name": "Main", "width": 16, "height": 16})).unwrap();
        for _ in 0..2 {
            s.execute("renderQueue.add", json!({"format": "h264", "output": "/virtual/shared.mp4"})).unwrap();
        }
        let (outputs, _) = s.queued_outputs(&effectcraft_project::render_queue::OutputFormat::ALL);
        let duplicates = s.duplicate_outputs(&outputs, &[]).unwrap();
        assert_eq!(duplicates.len(), 1);
        assert!(duplicates[0].path.ends_with("shared.mp4"));
        let refused = [Refused { item: duplicates[0].item, reason: "already refused".into() }];
        assert!(s.duplicate_outputs(&outputs, &refused).unwrap().is_empty());
    }

    /// An output of queue item `item` that is a file, known by its name alone.
    fn movie(item: u64, path: &str) -> Write {
        Write { item, path: path.into(), target: Target::file(Path::new(path), false), skips: false }
    }

    /// An output of queue item `item` that is `count` frames of a sequence from frame `first`.
    fn frames(item: u64, pattern: &str, first: i64, count: u64) -> Write {
        Write { item, path: pattern.into(), target: Target::frames(pattern, first, count), skips: false }
    }

    fn job_of(writes: impl IntoIterator<Item = Write>) -> Writes {
        let mut job = Writes::default();
        for write in writes {
            job.push(write).unwrap();
        }
        job
    }

    #[test]
    fn browser_output_names_match_the_virtual_file_table() {
        assert_eq!(browser_key(Path::new("renders/a.png")), browser_key(Path::new("/renders/a.png")));
        assert_ne!(browser_key(Path::new("renders/A.png")), browser_key(Path::new("renders/a.png")));
        assert_ne!(browser_key(Path::new("renders/../a.png")), browser_key(Path::new("a.png")));
        assert_ne!(browser_key(Path::new("//a.png")), browser_key(Path::new("/a.png")));
    }

    #[test]
    fn output_plan_path_budget_returns_an_error() {
        let mut job = Writes::default();
        let path = "x".repeat(1024 * 1024);
        for _ in 0..16 {
            job.push(movie(1, &path)).unwrap();
        }
        assert!(job.push(movie(2, "one-too-many")).is_err());
        assert_eq!(job.all.len(), 16);
    }

    /// The item an output replaces a file of, how many files, and the first.
    fn clash(job: &Writes, later: &Write) -> Option<(u64, u64, i64)> {
        job.clash(later).map(|c| (c.earlier.item, c.files, c.first))
    }

    #[test]
    fn a_file_is_replaced_by_the_output_that_has_its_name() {
        let job = job_of([movie(1, "out/a.mp4"), movie(2, "out/b.mp4"), movie(3, "out/b.mp4")]);
        assert_eq!(clash(&job, &movie(9, "out/a.mp4")), Some((1, 1, 0)));
        assert_eq!(clash(&job, &movie(9, "out/b.mp4")), Some((2, 1, 0)), "the first to write it is named");
        assert_eq!(clash(&job, &movie(9, "out/x/../a.mp4")), Some((1, 1, 0)), "dot segments are resolved");
        assert_eq!(clash(&job, &movie(9, "out/c.mp4")), None);
        assert_eq!(clash(&job, &movie(9, "other/a.mp4")), None);
        assert_eq!(clash(&job, &frames(9, "out/a.mp4", 0, 5)), None, "a sequence is not a movie");
        assert!(Writes::default().is_empty() && !job.is_empty());
    }

    #[test]
    fn two_names_of_one_file_on_disk_are_one_file() {
        let on_disk = |item: u64, name: &str, id: Option<&str>| Write {
            item,
            path: name.into(),
            target: Target::File { key: name.into(), identity: id.map(String::from) },
            skips: false,
        };
        let job = job_of([on_disk(1, "/a/link.mp4", Some("7:1")), on_disk(2, "/a/nothing-yet.mp4", None)]);
        assert_eq!(clash(&job, &on_disk(9, "/b/real.mp4", Some("7:1"))), Some((1, 1, 0)));
        assert_eq!(clash(&job, &on_disk(9, "/b/real.mp4", Some("7:2"))), None);
        assert_eq!(clash(&job, &on_disk(9, "/b/real.mp4", None)), None, "a file that is not there has no identity");
        assert_eq!(clash(&job, &on_disk(9, "/a/nothing-yet.mp4", None)), Some((2, 1, 0)), "its name is all there is");
    }

    #[test]
    fn sequences_clash_where_their_frames_do() {
        let job = job_of([frames(1, "a_[#####].png", 0, 5)]); // frames 0-4
        assert_eq!(clash(&job, &frames(2, "a_[#####].png", 5, 5)), None, "the five after it");
        assert_eq!(clash(&job, &frames(2, "a_[#####].png", -3, 3)), None, "the three before it");
        assert_eq!(clash(&job, &frames(2, "a_[#####].png", 4, 5)), Some((1, 1, 4)), "frame 4 only");
        assert_eq!(clash(&job, &frames(2, "a_[#####].png", 2, 1)), Some((1, 1, 2)));
        assert_eq!(clash(&job, &frames(2, "a_[#####].png", 1, 2)), Some((1, 2, 1)));
        assert_eq!(clash(&job, &frames(2, "a_[#####].png", -3, 100)), Some((1, 5, 0)), "all five");
        // The first of several that overlap is the one named.
        let job = job_of([frames(1, "a_[#####].png", 0, 3), frames(2, "a_[#####].png", 3, 3)]);
        assert_eq!(clash(&job, &frames(9, "a_[#####].png", 2, 3)), Some((1, 1, 2)));
        assert_eq!(clash(&job, &frames(9, "a_[#####].png", 3, 3)), Some((2, 3, 3)));
    }

    #[test]
    fn sequences_of_other_names_do_not_clash() {
        let job = job_of([frames(1, "out/a_[#####].png", 0, 5)]);
        for other in ["out/b_[#####].png", "out/a_[####].png", "out/a_[#####].jpg", "elsewhere/a_[#####].png", "out/a[#####].png", "out/a_[#####]/x.png"] {
            assert_eq!(clash(&job, &frames(2, other, 0, 5)), None, "{other}");
        }
        // One name, however it is spelled: the run with or without brackets, none at all, dot segments.
        for same in ["out/a_[#####].png", "out/a_#####.png", "out/a.png", "out/b/../a_[#####].png"] {
            assert_eq!(clash(&job, &frames(2, same, 0, 5)), Some((1, 5, 0)), "{same}");
        }
        // A frame number that starts a folder's name is not a file name's.
        let job = job_of([frames(1, "out/[##].png", 0, 5)]);
        assert_eq!(clash(&job, &frames(2, "out[##].png", 0, 5)), None);
        assert_eq!(clash(&job, &frames(2, "out/[##].png", 0, 5)), Some((1, 5, 0)));
    }

    #[test]
    fn padding_widths_can_name_the_same_frame_and_files_can_name_sequence_frames() {
        let job = job_of([frames(1, "out/a_[####].png", 9999, 3)]);
        assert_eq!(clash(&job, &frames(2, "out/a_[#####].png", 9999, 3)), Some((1, 2, 10000)));
        let job = job_of([frames(1, "out/a_[##].png", -12, 15)]);
        assert_eq!(clash(&job, &frames(2, "out/a_[###].png", -12, 15)), Some((1, 3, -12)));
        let job = job_of([frames(1, "out/a_[#####].png", 0, 5)]);
        assert_eq!(clash(&job, &movie(2, "out/a_00002.png")), Some((1, 1, 2)));
        let job = job_of([movie(1, "out/a_00002.png")]);
        assert_eq!(clash(&job, &frames(2, "out/a_[#####].png", 0, 5)), Some((1, 1, 2)));
        let mut skipping = frames(2, "out/a_[#####].png", 0, 5);
        skipping.skips = true;
        assert_eq!(clash(&job, &skipping), None);
    }

    /// Skip Existing Files leaves a frame that is there alone: it matters for the output that
    /// comes second, and for sequences only.
    #[test]
    fn an_output_that_skips_existing_frames_replaces_none() {
        let skipping = |mut write: Write| {
            write.skips = true;
            write
        };
        let job_of_one = job_of([frames(1, "a_[#####].png", 0, 5)]);
        assert_eq!(clash(&job_of_one, &skipping(frames(2, "a_[#####].png", 0, 5))), None);
        let job_of_one = job_of([skipping(frames(1, "a_[#####].png", 0, 5))]);
        assert_eq!(clash(&job_of_one, &frames(2, "a_[#####].png", 3, 5)), Some((1, 2, 3)), "the earlier one's frames are replaced");
        let job_of_one = job_of([movie(1, "a.mp4")]);
        assert_eq!(clash(&job_of_one, &skipping(movie(2, "a.mp4"))), Some((1, 1, 0)), "a movie is written whole");
    }

    #[test]
    fn frame_numbers_beyond_what_a_project_holds_do_not_panic() {
        let top = i64::MAX - 1;
        let job = job_of([frames(1, "a_[#####].png", top, u64::MAX), frames(2, "a_[#####].png", i64::MIN, u64::MAX)]);
        assert_eq!(clash(&job, &frames(9, "a_[#####].png", top, u64::MAX)), Some((1, 1, top)));
        assert_eq!(clash(&job, &frames(9, "a_[#####].png", -5, 3)), Some((2, 3, -5)));
        assert_eq!(clash(&job, &frames(9, "a_[#####].png", 0, 5)), None, "between the two");
        assert_eq!(clash(&job, &frames(9, "a_[#####].png", i64::MAX, 0)), None, "nothing written");
        assert_eq!(clash(&job, &frames(9, "a_[#####].png", i64::MIN, 0)), None);
    }

    #[test]
    fn the_reason_names_both_items_and_what_they_write() {
        let both = DuplicateOutput {
            item: 7,
            index: 3,
            comp: "Main".into(),
            path: "/o/Main.mp4".into(),
            files: 1,
            sequence: false,
            with: 4,
            with_index: 1,
            with_comp: "Intro".into(),
        };
        assert_eq!(
            both.reason(),
            "#1 “Intro” and #3 “Main” would both write /o/Main.mp4, and a render never writes one file twice; choose another Output To for one of them"
        );
        let sequence = DuplicateOutput { sequence: true, files: 12, path: "/o/Main_[#####].png".into(), ..both.clone() };
        assert!(sequence.reason().contains("would both write 12 frames of /o/Main_[#####].png,"), "{}", sequence.reason());
        let one = DuplicateOutput { files: 1, path: "/o/Main_00004.png".into(), ..sequence };
        assert!(one.reason().contains("would both write /o/Main_00004.png,"), "{}", one.reason());
        let itself = DuplicateOutput { with: 7, with_index: 3, with_comp: "Main".into(), ..both.clone() };
        assert!(itself.reason().starts_with("#3 “Main” would write /o/Main.mp4 twice (two of its output modules), and a render never"), "{}", itself.reason());

        let mut found = vec![];
        assert_eq!(duplicate_refusal(&found), None);
        found.push(both.clone());
        assert_eq!(duplicate_refusal(&found), Some(format!("{}. Nothing was rendered", both.reason())));
        found.push(both.clone());
        assert!(duplicate_refusal(&found).is_some_and(|r| r.ends_with("Nothing was rendered; 1 more output would write a file twice too")));
        found.push(both);
        assert!(duplicate_refusal(&found).is_some_and(|r| r.ends_with("Nothing was rendered; 2 more outputs would write a file twice too")));
    }

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
    #[cfg(not(target_arch = "wasm32"))]
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
