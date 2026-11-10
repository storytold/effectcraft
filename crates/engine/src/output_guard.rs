//! Protect files the project reads from render outputs (#1743).
//!
//! This guard is independent of replacement consent and export staging: it never authorizes
//! overwriting ordinary outputs or opens a second confirmation flow. Those belong to the
//! exporter. Paths are compared by name once resolved ([`key`]): made absolute, `.` and `..`
//! resolved, one separator, a volume's verbatim prefix dropped, and without case where volumes
//! ignore it. On Windows a drive letter whose root resolves elsewhere (a mapped network drive, a
//! `subst` drive) stands for that place where that can matter ([`Drives`]). No file is looked at
//! (#3328): a link to a source under another name is not recognised, which comparing file
//! identities did at the cost of opening every footage frame for every check. Checks happen
//! before dispatch; filesystem changes between the check and the write require exporter-side
//! revalidation.
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

/// The files the project reads: footage (every frame of a sequence), proxies and the project file,
/// by the name they are compared by ([`key`]). Built once per guard; checking a path against it
/// looks at no file (#3328: comparing file identities opened every footage frame for every
/// existing output, minutes for long sequences on a network share).
#[derive(Default)]
pub(crate) struct Protected {
    files: Vec<(PathBuf, Source)>,
    /// The files by [`key`], as written.
    by_path: HashMap<String, usize>,
    /// Which drive letters the files are on, for a mapped network drive and its UNC path being
    /// one place ([`Drives`]).
    #[cfg(windows)]
    drives: Drives,
}

/// The files the project reads, checked before a command writes to a path it was given
/// ([`Session::output_guard`]). One guard serves all the writes of one command.
pub struct OutputGuard(Protected);

impl OutputGuard {
    /// `Err` when `path` is a file the project reads (footage, a proxy, the project file).
    pub fn refuse(&self, path: &str) -> crate::Result<()> {
        match self.0.find(Path::new(path)) {
            Some(source) => Err(crate::EngineError::Other(source.refusal(path, Use::Export))),
            None => Ok(()),
        }
    }
}

impl Protected {
    pub(crate) fn of(s: &Session) -> Protected {
        s.output_guards.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
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
        let key = key(&path);
        #[cfg(windows)]
        self.drives.note(&key);
        self.by_path.entry(key).or_insert(self.files.len());
        self.files.push((path, source));
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    /// The source at this path, compared by name once resolved ([`key`]): no file is looked at.
    /// On Windows a drive letter is compared as the place its root resolves to where a path on
    /// another letter or a UNC path could name the same file ([`Drives`]).
    pub(crate) fn find(&self, path: &Path) -> Option<&Source> {
        let key = key(path);
        let at = self.by_path.get(&key).copied();
        #[cfg(windows)]
        let at = at.or_else(|| self.drives.find(&self.by_path, &key));
        at.and_then(|i| self.files.get(i)).map(|(_, source)| source)
    }
}

/// The drive letters of the protected files (#3330). A mapped network drive (`P:` for
/// `\\server\share`) or a `subst` drive is another name for a place: a path on a letter is
/// compared as the place its root resolves to ([`drive_root`]). That asks the file system (and
/// a disconnected drive can take long to answer), so it is done only where it can matter: an
/// output and protected files that are not all on one letter, or not all UNC paths. Then the
/// letters' roots come from a process-wide cache ([`resolved_root`]).
#[cfg(windows)]
#[derive(Default)]
struct Drives {
    /// The letters the files are on.
    letters: std::collections::BTreeSet<char>,
    /// A file isn't on a drive letter (a UNC path).
    other: bool,
    /// The files by their key with each letter resolved, made once a check needs it.
    resolved: std::sync::OnceLock<HashMap<String, usize>>,
}

#[cfg(windows)]
impl Drives {
    /// Whether the key changed what the files are on (a letter or a UNC path new to them).
    fn note(&mut self, key: &str) -> bool {
        match drive_letter(key) {
            Some(letter) => self.letters.insert(letter),
            None => !std::mem::replace(&mut self.other, true),
        }
    }

    /// [`Drives::note`] in a set that grows (queued outputs): what is resolved already stays
    /// valid unless the key brings a new letter or the first UNC path. `file` is the number of
    /// the output when the key is one file (a sequence's pattern is only noted). `true` when
    /// everything resolved has to be made again.
    fn note_at(&mut self, key: &str, file: Option<usize>) -> bool {
        let changed = self.note(key);
        if changed {
            self.resolved = std::sync::OnceLock::new();
        } else if let (Some(at), Some(resolved)) = (file, self.resolved.get_mut()) {
            let root = drive_letter(key).and_then(resolved_root);
            resolved.entry(with_root(key, root)).or_insert(at);
        }
        changed
    }

    /// `key` as the place it names when `resolving`, else as it is: keys of one place are equal
    /// when both are resolved. Whether to resolve is decided by the output asking
    /// ([`Drives::matters`]), for all the keys it is compared with: an earlier output on the
    /// letter it names is the one that makes it matter.
    fn place(&self, key: &str, resolving: bool) -> String {
        if resolving { with_root(key, drive_letter(key).and_then(resolved_root)) } else { key.to_string() }
    }

    /// Whether an output with `key` can name a file of a protected path under another root:
    /// it is on a letter and some file isn't on that letter, or it isn't on a letter and some
    /// file is.
    fn matters(&self, key: &str) -> bool {
        match drive_letter(key) {
            Some(letter) => self.other || self.letters.iter().any(|l| *l != letter),
            None => !self.letters.is_empty(),
        }
    }

    /// The file an output with `key` names once drive letters are resolved, where that can
    /// matter.
    fn find(&self, by_path: &HashMap<String, usize>, key: &str) -> Option<usize> {
        if !self.matters(key) {
            return None;
        }
        let resolved = self.resolved.get_or_init(|| {
            // (each letter's root once, not once per file)
            let roots: HashMap<char, Option<String>> = self.letters.iter().map(|l| (*l, resolved_root(*l))).collect();
            let mut resolved: HashMap<String, usize> = HashMap::with_capacity(by_path.len());
            for (k, i) in by_path {
                let root = drive_letter(k).and_then(|l| roots.get(&l).cloned().flatten());
                // (two files of one place: the first, as in `by_path`)
                resolved.entry(with_root(k, root)).and_modify(|j| *j = (*j).min(*i)).or_insert(*i);
            }
            resolved
        });
        let root = drive_letter(key).and_then(resolved_root);
        resolved.get(&with_root(key, root)).copied()
    }
}

/// The drive letter of `key` when it is a drive and its root (`x://…`; a drive-relative `x:foo`
/// is not mapped).
#[cfg(windows)]
fn drive_letter(key: &str) -> Option<char> {
    let mut chars = key.chars();
    let (Some(letter), Some(':'), Some('/'), Some('/')) = (chars.next(), chars.next(), chars.next(), chars.next()) else { return None };
    letter.is_ascii_lowercase().then_some(letter)
}

/// `key`, on a drive letter, with the drive's root as `root` (the [`key`] of what it resolves
/// to); as it is without one.
#[cfg(windows)]
fn with_root(key: &str, root: Option<String>) -> String {
    match (root, key.get(3..)) {
        // (`x:/` is the drive's root, the rest follows it)
        (Some(root), Some(rest)) => format!("{root}{rest}"),
        _ => key.to_string(),
    }
}

/// How long a drive root looked up stays known ([`resolved_root`]): a disconnected drive is
/// waited for at most once in this time.
#[cfg(windows)]
const ROOT_TTL: std::time::Duration = std::time::Duration::from_secs(60);

/// A drive root looked up: by whom (tests: each test its own), the letter, when, and the
/// [`key`] of what it resolved to (`None`: it couldn't be).
#[cfg(windows)]
type KnownRoot = (u64, char, std::time::Instant, Option<String>);

/// The [`key`] of what drive `letter`'s root resolves to, `None` when it can't be: looked up
/// at most once per [`ROOT_TTL`] in the process, failures too. The file system is asked
/// without the cache's lock held.
#[cfg(windows)]
fn resolved_root(letter: char) -> Option<String> {
    static KNOWN: std::sync::Mutex<Vec<KnownRoot>> = std::sync::Mutex::new(Vec::new());
    let lock = || KNOWN.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let (by, now) = (root_namespace(), root_clock());
    let fresh = |at: &std::time::Instant| now.saturating_duration_since(*at) < ROOT_TTL;
    if let Some((.., root)) = lock().iter().find(|(b, l, at, _)| *b == by && *l == letter && fresh(at)) {
        return root.clone();
    }
    let root = drive_root(letter).map(|r| key(&r));
    let mut known = lock();
    // (this caller's stale entries go, and this letter's; another's are its own to judge)
    known.retain(|(b, l, at, _)| *b != by || (*l != letter && fresh(at)));
    known.push((by, letter, now, root.clone()));
    root
}

/// What the root of drive `letter` (`a`..`z`) resolves to, `None` when it can't be (a
/// disconnected drive): a mapped network drive's `\\?\UNC\server\share\`, a `subst` drive's
/// folder, a local drive itself.
#[cfg(windows)]
fn drive_root(letter: char) -> Option<PathBuf> {
    #[cfg(test)]
    if let Some(root) = DRIVE_ROOT.with(|hook| hook.borrow_mut().as_mut().map(|h| h(letter))) {
        return root;
    }
    std::fs::canonicalize(format!("{letter}:\\")).ok()
}

/// The cache entries of this caller ([`resolved_root`]): one for the process, one per test.
#[cfg(windows)]
fn root_namespace() -> u64 {
    #[cfg(test)]
    return ROOT_NAMESPACE.with(std::cell::Cell::get);
    #[cfg(not(test))]
    0
}

/// Now, for [`resolved_root`] (tests move it on).
#[cfg(windows)]
fn root_clock() -> std::time::Instant {
    let now = std::time::Instant::now();
    #[cfg(test)]
    return ROOT_CLOCK_AHEAD.with(|ahead| now.checked_add(ahead.get()).unwrap_or(now));
    #[cfg(not(test))]
    now
}

/// Resolves drive roots in place of the file system on this thread (tests).
#[cfg(all(windows, test))]
type DriveHook = Box<dyn FnMut(char) -> Option<PathBuf>>;

#[cfg(all(windows, test))]
thread_local! {
    static DRIVE_ROOT: std::cell::RefCell<Option<DriveHook>> = const { std::cell::RefCell::new(None) };
    static ROOT_NAMESPACE: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    static ROOT_CLOCK_AHEAD: std::cell::Cell<std::time::Duration> = const { std::cell::Cell::new(std::time::Duration::ZERO) };
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

/// The name a path is compared by: absolute, resolved by name, with `/` between its parts, a
/// Windows verbatim prefix (`\\?\C:`, `\\?\UNC\server\share`) as the plain one, and without case
/// where volumes ignore it. No file is looked at.
#[cfg(not(target_arch = "wasm32"))]
fn key(path: &Path) -> String {
    use std::path::Prefix;
    let parts: Vec<String> = lexical(path)
        .components()
        .map(|c| match c {
            Component::Prefix(p) => match p.kind() {
                Prefix::Disk(d) | Prefix::VerbatimDisk(d) => format!("{}:", char::from(d)),
                Prefix::UNC(server, share) | Prefix::VerbatimUNC(server, share) => {
                    format!("/{}/{}", server.to_string_lossy(), share.to_string_lossy())
                }
                Prefix::DeviceNS(device) => format!("/./{}", device.to_string_lossy()),
                Prefix::Verbatim(name) => name.to_string_lossy().into_owned(),
            },
            // (the separator joining the parts)
            Component::RootDir => String::new(),
            other => other.as_os_str().to_string_lossy().into_owned(),
        })
        .collect();
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
    /// One file, known by its name.
    File { key: String },
    /// The numbered files `first..end` of a sequence: `key` is its pattern, with the width the
    /// frame numbers are padded to.
    Frames { key: String, width: usize, first: i64, end: i64 },
}

impl Target {
    /// The file at `path`.
    fn file(path: &Path) -> Target {
        Target::File { key: key(path) }
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
    /// Which drive letters the files and sequences are on: a mapped network drive and its UNC
    /// path name one place ([`Drives`]), as for the files the project reads. The roots come from
    /// a cache that keeps them 60 s ([`ROOT_TTL`]), so a queue built over a longer time may
    /// compare by a root that was looked up again meanwhile.
    #[cfg(windows)]
    drives: Drives,
    /// What each output names once its drive is resolved, made when a comparison first needs it
    /// and kept in step with the outputs ([`Writes::push`]): comparing outputs then takes no
    /// lookup, lock or allocation per output.
    #[cfg(windows)]
    places: std::sync::OnceLock<Vec<Place>>,
}

/// The place an output names, drive resolved ([`Writes::places`]).
#[cfg(windows)]
struct Place {
    /// The file's name, or the sequence's pattern as [`Target::Frames`] has it.
    target: String,
    /// A sequence's output path as a name (`sequence_frame` takes it); empty for a file.
    pattern: String,
}

#[cfg(windows)]
impl Place {
    /// The place `write` names, its keys resolved by `resolve`.
    fn of(write: &Write, resolve: impl Fn(&str) -> String) -> Place {
        match &write.target {
            Target::File { key } => Place { target: resolve(key), pattern: String::new() },
            Target::Frames { key, .. } => Place { target: resolve(key), pattern: resolve(&crate::output_guard::key(Path::new(&write.path))) },
        }
    }
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
    /// Whether the keys an output with `key` is compared with are resolved to the places they
    /// name ([`Drives::matters`]; never outside Windows).
    fn resolving(&self, key: &str) -> bool {
        #[cfg(windows)]
        return self.drives.matters(key);
        #[cfg(not(windows))]
        {
            let _ = key;
            false
        }
    }

    /// `key`, a file's name or a sequence's pattern, as the place it names when `resolving`.
    fn place(&self, key: &str, resolving: bool) -> String {
        #[cfg(windows)]
        return self.drives.place(key, resolving);
        #[cfg(not(windows))]
        {
            let _ = resolving;
            key.to_string()
        }
    }

    /// The places the earlier outputs name, when `resolving`.
    #[cfg(windows)]
    fn places(&self, resolving: bool) -> Option<&[Place]> {
        resolving.then(|| {
            self.places
                .get_or_init(|| {
                    // (each letter's root once)
                    let roots: HashMap<char, Option<String>> = self.drives.letters.iter().map(|l| (*l, resolved_root(*l))).collect();
                    let resolve = |key: &str| with_root(key, drive_letter(key).and_then(|l| roots.get(&l).cloned().flatten()));
                    self.all.iter().map(|write| Place::of(write, resolve)).collect()
                })
                .as_slice()
        })
    }

    /// The name of earlier output `at` (its target `key`) for comparing with a later one: its
    /// place when `resolving`.
    fn earlier_target<'a>(&'a self, at: usize, key: &'a str, resolving: bool) -> std::borrow::Cow<'a, str> {
        #[cfg(windows)]
        if resolving {
            return match self.places(true).and_then(|places| places.get(at)) {
                Some(place) => std::borrow::Cow::Borrowed(place.target.as_str()),
                None => std::borrow::Cow::Owned(self.place(key, true)),
            };
        }
        let _ = (at, resolving);
        std::borrow::Cow::Borrowed(key)
    }

    /// The sequence pattern of earlier output `at`, as a name: its place when `resolving`.
    fn earlier_pattern(&self, at: usize, earlier: &Write, resolving: bool) -> std::borrow::Cow<'_, str> {
        #[cfg(windows)]
        if resolving && let Some(place) = self.places(true).and_then(|places| places.get(at)) {
            return std::borrow::Cow::Borrowed(place.pattern.as_str());
        }
        let _ = at;
        std::borrow::Cow::Owned(self.place(&crate::output_guard::key(Path::new(&earlier.path)), resolving))
    }

    fn is_empty(&self) -> bool {
        self.all.is_empty()
    }

    /// The earliest output `later`, rendered after all of these, writes over.
    fn clash(&self, later: &Write) -> Option<Clash<'_>> {
        match &later.target {
            Target::File { key } => {
                let at = self.files.get(key).copied();
                #[cfg(windows)]
                let at = at.or_else(|| self.drives.find(&self.files, key));
                if let Some(at) = at {
                    return Some(Clash { earlier: self.all.get(at)?, files: 1, first: 0 });
                }
                let resolving = self.resolving(key);
                let key = self.place(key, resolving);
                self.all.iter().enumerate().find_map(|(at, earlier)| {
                    let Target::Frames { first, end, .. } = earlier.target else { return None };
                    let frame = sequence_frame(&self.earlier_pattern(at, earlier, resolving), &key)?;
                    (first..end).contains(&frame).then_some(Clash { earlier, files: 1, first: frame })
                })
            }
            // A frame that is there already is left alone, so a later output that skips existing
            // frames replaces none of an earlier one's.
            Target::Frames { .. } if later.skips => None,
            Target::Frames { key, width, first, end } => {
                let resolving = self.resolving(key);
                let (place, pattern) = (self.place(key, resolving), self.place(&crate::output_guard::key(Path::new(&later.path)), resolving));
                self.all.iter().enumerate().find_map(|(at, earlier)| {
                    let (files, start) = match &earlier.target {
                        Target::Frames { key: other, width: other_width, first: from, end: to } if place == self.earlier_target(at, other, resolving) => {
                            common_frames(*first, *end, *from, *to, *width, *other_width)?
                        }
                        Target::File { key } => {
                            let frame = sequence_frame(&pattern, &self.earlier_target(at, key, resolving))?;
                            if !(*first..*end).contains(&frame) {
                                return None;
                            }
                            (1, frame)
                        }
                        _ => return None,
                    };
                    Some(Clash { earlier, files, first: start })
                })
            }
        }
    }

    /// Keep the places in step with a new output, or drop them when what they were resolved
    /// with changed (a new letter, the first UNC path). Before it joins `all`: the places are
    /// made over `all`.
    #[cfg(windows)]
    fn note_place(&mut self, write: &Write, changed: bool) {
        if changed {
            self.places = std::sync::OnceLock::new();
        } else if let Some(places) = self.places.get_mut() {
            places.push(Place::of(write, |key| with_root(key, drive_letter(key).and_then(resolved_root))));
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
            // (`note_at` goes before the insert: it extends a resolved map by this output, and
            // starts a new one, from `files`, when a new letter or the first UNC path arrives)
            Target::File { key } => {
                #[cfg(windows)]
                let changed = self.drives.note_at(key, Some(at));
                #[cfg(windows)]
                self.note_place(&write, changed);
                self.files.entry(key.clone()).or_insert(at);
            }
            Target::Frames { key, .. } => {
                #[cfg(windows)]
                let changed = self.drives.note_at(key, None);
                #[cfg(windows)]
                self.note_place(&write, changed);
            }
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

    /// The files the project reads, to check the paths a command writes against
    /// ([`OutputGuard::refuse`], [`Session::write_output_guarded`]): a command that checks a path
    /// before its work and writes it after uses one guard for both.
    pub fn output_guard(&self) -> OutputGuard {
        OutputGuard(self.protected_files())
    }

    /// `Err` when `path` is a file the project reads (footage, a proxy, the project file) and so
    /// is not for an export to replace. For callers that write the file themselves; call it
    /// before the work, not after it.
    pub fn refuse_export(&self, path: &str) -> crate::Result<()> {
        self.output_guard().refuse(path)
    }

    /// Write `bytes` to `path` through the host's services, unless the project reads the file
    /// there ([`Session::refuse_export`]). Every command that writes to a path it is given
    /// writes through this; Save and Save As (the project's own file) do not.
    pub fn write_output(&self, path: &str, bytes: &[u8]) -> crate::Result<()> {
        self.write_output_with(&self.protected_files(), path, bytes)
    }

    /// [`Session::write_output`] checked against `guard` ([`Session::output_guard`]).
    pub fn write_output_guarded(&self, guard: &OutputGuard, path: &str, bytes: &[u8]) -> crate::Result<()> {
        self.write_output_with(&guard.0, path, bytes)
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
                let write = self.write_of(m, path);
                if let Some(clash) = writes.clash(&write) {
                    duplicates.push(self.duplicate_of(&write, &clash));
                    break;
                }
            }
            for path in paths {
                writes.push(self.write_of(m, &path))?;
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

    /// What `path`, an output of `m`, writes.
    fn write_of(&self, m: &RenderQueueItem, path: &str) -> Write {
        let target = if m.output.format.is_sequence() {
            let (first, count) = self.project.comp(m.comp).map_or((0, 0), |c| (m.settings.first_frame(c), m.settings.frame_count(c)));
            Target::frames(path, first, count)
        } else {
            Target::file(Path::new(path))
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
                in_the_way |= paths.iter().any(|path| queue.clash(&self.write_of(module, path)).is_some());
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
                    writes.push(self.write_of(m, &path))?;
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
        Write { item, path: path.into(), target: Target::file(Path::new(path)), skips: false }
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
    fn folder_numbered_sequences_are_never_partially_checked() {
        let pattern = std::env::temp_dir().join("frames[######]").join("plate.png").to_string_lossy().into_owned();
        let error = frames_on_disk(&pattern, 0, PROBE_LIMIT + 1).unwrap_err();
        assert!(error.contains("cannot safely check") && error.contains("100000"), "{error}");
        assert!(frames_on_disk(&pattern, 0, u64::MAX).is_err());
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

    /// A session reading a sequence of `count` frames `Clip_0000.exr`… in `dir`; their paths.
    #[cfg(not(target_arch = "wasm32"))]
    fn session_reading(dir: &Path, count: usize) -> (Session, Vec<String>) {
        let frames: Vec<String> = (0..count).map(|i| dir.join(format!("Clip_{i:04}.exr")).to_string_lossy().into_owned()).collect();
        let mut s = Session::default();
        let footage = Footage { path: frames[0].clone(), sequence: frames.clone(), has_video: true, ..Default::default() };
        Arc::make_mut(&mut s.project).add_item("plates", crate::color::Label::Aqua, None, ItemKind::Footage(footage));
        (s, frames)
    }

    /// #1743, #3328: a footage frame is refused as an output under every spelling of its path
    /// the volume reads as it: dot segments, the case where volumes ignore it, either separator
    /// and a verbatim prefix on Windows. The files need not exist: names are compared.
    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn a_footage_path_is_refused_under_every_spelling() {
        let dir = std::env::temp_dir().join("effectcraft-guard-spelling").join("plates");
        let (s, frames) = session_reading(&dir, 3);
        let frame = &frames[1];
        let refused = |path: &str| s.refuse_export(path).is_err();
        assert!(refused(frame));
        let dotted = dir.join("sub").join("..").join(".").join("Clip_0001.exr");
        assert!(refused(&dotted.to_string_lossy()), "dot segments");
        let cased = dir.parent().unwrap().join("PLATES").join("clip_0001.EXR");
        assert_eq!(refused(&cased.to_string_lossy()), IGNORES_CASE, "case, where the volume ignores it");
        if cfg!(windows) {
            assert!(refused(&frame.replace('\\', "/")), "forward slashes");
            let mixed = frame.replacen('\\', "/", 2);
            assert!(refused(&mixed), "mixed separators: {mixed}");
            let verbatim = format!(r"\\?\{frame}");
            assert!(refused(&verbatim), "verbatim prefix: {verbatim}");
        }
        assert!(!refused(&dir.join("Clip_0009.exr").to_string_lossy()), "another frame number");
        assert!(!refused(&dir.join("Clip_0001.png").to_string_lossy()), "another extension");
        let error = s.refuse_export(frame).unwrap_err().to_string();
        assert!(error.contains("plates") && error.contains("footage"), "{error}");
    }

    /// The verbatim and plain forms of a network path name one file.
    #[test]
    #[cfg(windows)]
    fn a_verbatim_unc_path_is_its_plain_form() {
        assert_eq!(key(Path::new(r"\\?\UNC\Server\Share\a\b.exr")), key(Path::new(r"\\server\share\a\b.exr")));
        assert_eq!(key(Path::new(r"\\?\C:\a\b.exr")), key(Path::new("c:/a/b.exr")));
        assert_ne!(key(Path::new(r"\\server\share\a\b.exr")), key(Path::new(r"\\server\other\a\b.exr")));
    }

    /// #3328: checking an existing output against 13,000 protected frames looks at no frame.
    /// The guard's only question to the file system is a drive's root ([`drive_root`], Windows):
    /// counted here, it is asked once, for the output's drive (the frames are on a UNC share),
    /// over both checks of one guard. Comparing identities opened every frame for every check.
    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn thirteen_thousand_protected_frames_are_checked_without_looking_at_a_file() {
        #[cfg(windows)]
        let lookups = drive_roots(&[]);
        let share = if cfg!(windows) { PathBuf::from(r"\\effectcraft-3328.invalid\share\seq") } else { PathBuf::from("/effectcraft-3328.invalid/share/seq") };
        let mut protected = Protected::default();
        for i in 0..13_000 {
            protected.add(share.join(format!("f_{i:05}.exr")), Source::Footage("plates".into()));
        }
        let guard = OutputGuard(protected);
        // (an output that exists: comparing identities opened every protected frame for it)
        let output = std::env::temp_dir().join(format!("effectcraft-guard-13000-{}.png", std::process::id()));
        std::fs::write(&output, b"an older render").unwrap();
        let output_text = output.to_string_lossy().into_owned();
        let first = guard.refuse(&output_text);
        let second = guard.refuse(&output_text);
        let _ = std::fs::remove_file(&output);
        // Outside Windows `Protected::find` touches no file system at all (no drive roots to
        // resolve), so only the Windows run can count lookups; elsewhere the checks must just pass.
        assert!(first.is_ok() && second.is_ok());
        assert!(guard.refuse(&share.join("f_06500.exr").to_string_lossy()).is_err());
        #[cfg(windows)]
        {
            assert_eq!(lookups.get(), 1, "the output drive's root, once; no frame");
            DRIVE_ROOT.with(|h| *h.borrow_mut() = None);
        }
    }

    /// Resolve drive roots on this thread with `roots` (letter, root; a letter not listed can't
    /// be resolved), counting the lookups.
    #[cfg(windows)]
    fn drive_roots(roots: &[(char, &'static str)]) -> std::rc::Rc<std::cell::Cell<usize>> {
        let roots: Vec<(char, &'static str)> = roots.to_vec();
        let count = std::rc::Rc::new(std::cell::Cell::new(0));
        let counted = count.clone();
        let hook: DriveHook = Box::new(move |letter| {
            counted.set(counted.get() + 1);
            roots.iter().find(|(l, _)| *l == letter).map(|(_, root)| PathBuf::from(root))
        });
        DRIVE_ROOT.with(|h| *h.borrow_mut() = Some(hook));
        ROOT_CLOCK_AHEAD.with(|a| a.set(std::time::Duration::ZERO));
        // (a cache of roots of its own: tests running at once don't see each other's)
        static NAMESPACES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        ROOT_NAMESPACE.with(|n| n.set(NAMESPACES.fetch_add(1, std::sync::atomic::Ordering::Relaxed)));
        count
    }

    #[cfg(windows)]
    fn guarding(paths: &[&str]) -> Protected {
        let mut protected = Protected::default();
        for path in paths {
            protected.add(PathBuf::from(path), Source::Footage("plates".into()));
        }
        protected
    }

    /// #3330: a mapped network drive and its UNC path are one place, both ways; a `subst` drive
    /// is its folder.
    #[test]
    #[cfg(windows)]
    fn a_mapped_drive_and_its_unc_path_are_one_place() {
        drive_roots(&[('p', r"\\?\UNC\server\share\"), ('s', r"\\?\C:\work\")]);
        let by_letter = guarding(&[r"P:\seq\f_0001.exr"]);
        assert!(by_letter.find(Path::new(r"\\server\share\seq\f_0001.exr")).is_some(), "footage by letter, output by UNC");
        assert!(by_letter.find(Path::new(r"\\SERVER\Share\seq\F_0001.EXR")).is_some(), "in another case");
        assert!(by_letter.find(Path::new(r"\\?\UNC\server\share\seq\f_0001.exr")).is_some(), "verbatim");
        assert!(by_letter.find(Path::new(r"\\server\share\seq\f_0002.exr")).is_none());
        assert!(guarding(&[r"P:\..\seq\f_0001.exr"]).find(Path::new(r"\\server\share\seq\f_0001.exr")).is_some(), "`..` stops at the root");
        let guard = OutputGuard(guarding(&[r"P:\seq\f_0001.exr"]));
        assert!(guard.refuse(r"\\server\share\seq\f_0001.exr").is_err(), "refused by the guard");
        assert!(by_letter.find(Path::new(r"\\server\other\seq\f_0001.exr")).is_none(), "another share");
        let by_unc = guarding(&[r"\\server\share\seq\f_0001.exr"]);
        assert!(by_unc.find(Path::new(r"p:\seq\f_0001.exr")).is_some(), "footage by UNC, output by letter");
        let in_subst = guarding(&[r"C:\work\seq\f_0001.exr"]);
        assert!(in_subst.find(Path::new(r"S:\seq\f_0001.exr")).is_some(), "a subst drive is its folder");
        DRIVE_ROOT.with(|h| *h.borrow_mut() = None);
    }

    /// A local drive keeps its letter, and so does one whose root can't be resolved (a
    /// disconnected drive): the check never fails because of it.
    #[test]
    #[cfg(windows)]
    fn a_local_or_unresolved_drive_keeps_its_letter() {
        drive_roots(&[('c', r"\\?\C:\")]);
        let local = guarding(&[r"C:\plates\f_0001.exr"]);
        assert!(local.find(Path::new(r"c:\plates\f_0001.exr")).is_some());
        assert!(local.find(Path::new(r"\\server\share\plates\f_0001.exr")).is_none());
        let unresolved = guarding(&[r"Q:\plates\f_0001.exr"]);
        assert!(unresolved.find(Path::new(r"Q:\plates\f_0001.exr")).is_some(), "the letter still matches");
        assert!(unresolved.find(Path::new(r"\\server\share\plates\f_0001.exr")).is_none());
        DRIVE_ROOT.with(|h| *h.borrow_mut() = None);
    }

    /// A guard resolves each drive letter once, not once per path.
    #[test]
    #[cfg(windows)]
    fn each_drive_letter_is_resolved_once_per_guard() {
        let count = drive_roots(&[('p', r"\\?\UNC\server\share\")]);
        let mut paths: Vec<String> = (0..1000).map(|i| format!(r"P:\seq\f_{i:04}.exr")).collect();
        paths.extend((0..500).map(|i| format!(r"Q:\other\f_{i:04}.exr")));
        let protected = guarding(&paths.iter().map(String::as_str).collect::<Vec<_>>());
        for _ in 0..3 {
            assert!(protected.find(Path::new(r"R:\render.png")).is_none());
            assert!(protected.find(Path::new(r"\\server\share\seq\f_0500.exr")).is_some());
        }
        assert_eq!(count.get(), 3, "P, Q and R once each");
        DRIVE_ROOT.with(|h| *h.borrow_mut() = None);
    }

    /// #3330: no drive root is looked up where it can't matter: footage and output on one
    /// letter, or all UNC paths (a disconnected drive is never waited for there).
    #[test]
    #[cfg(windows)]
    fn one_letter_or_only_unc_paths_look_up_no_root() {
        let count = drive_roots(&[('p', r"\\?\UNC\server\share\")]);
        let on_p = guarding(&[r"P:\seq\f_0001.exr", r"P:\seq\f_0002.exr"]);
        assert!(on_p.find(Path::new(r"P:\renders\out.png")).is_none());
        assert!(on_p.find(Path::new(r"p:\seq\F_0001.exr")).is_some());
        let unc = guarding(&[r"\\server\share\seq\f_0001.exr"]);
        assert!(unc.find(Path::new(r"\\server\share\renders\out.png")).is_none());
        assert!(unc.find(Path::new(r"\\server\share\seq\f_0001.exr")).is_some());
        assert_eq!(count.get(), 0);
        DRIVE_ROOT.with(|h| *h.borrow_mut() = None);
    }

    /// #3330: with a UNC path on one side, exactly the letters on the other are looked up.
    #[test]
    #[cfg(windows)]
    fn a_unc_path_on_one_side_looks_up_the_letters_of_the_other() {
        let count = drive_roots(&[('p', r"\\?\UNC\server\share\")]);
        let letters = guarding(&[r"P:\seq\f_0001.exr", r"Q:\seq\f_0001.exr"]);
        assert!(letters.find(Path::new(r"\\server\share\seq\f_0001.exr")).is_some());
        assert_eq!(count.get(), 2, "P and Q");
        let count = drive_roots(&[('p', r"\\?\UNC\server\share\")]);
        let unc = guarding(&[r"\\server\share\seq\f_0001.exr"]);
        assert!(unc.find(Path::new(r"P:\seq\f_0001.exr")).is_some());
        assert_eq!(count.get(), 1, "the output's P only");
        DRIVE_ROOT.with(|h| *h.borrow_mut() = None);
    }

    /// #3330: a root looked up serves later guards until it is a minute old, a failed lookup too.
    #[test]
    #[cfg(windows)]
    fn drive_roots_are_kept_for_a_minute() {
        let count = drive_roots(&[('p', r"\\?\UNC\server\share\")]);
        let check = || {
            let g = guarding(&[r"P:\seq\f_0001.exr", r"Q:\seq\f_0001.exr"]);
            assert!(g.find(Path::new(r"\\server\share\seq\f_0001.exr")).is_some());
        };
        check();
        assert_eq!(count.get(), 2, "P and Q (which can't be resolved)");
        ROOT_CLOCK_AHEAD.with(|a| a.set(ROOT_TTL - std::time::Duration::from_secs(1)));
        check();
        assert_eq!(count.get(), 2, "a second guard within the minute: from the cache, the failure too");
        ROOT_CLOCK_AHEAD.with(|a| a.set(ROOT_TTL + std::time::Duration::from_secs(1)));
        check();
        assert_eq!(count.get(), 4, "looked up again after it");
        DRIVE_ROOT.with(|h| *h.borrow_mut() = None);
        ROOT_CLOCK_AHEAD.with(|a| a.set(std::time::Duration::ZERO));
    }

    /// #3328: two queued movies naming one file, once by a mapped drive and once by its UNC path,
    /// collide, whichever is queued first; other places and letters don't.
    #[test]
    #[cfg(windows)]
    fn queued_outputs_collide_under_drive_and_unc_spellings() {
        let count = drive_roots(&[('p', r"\\?\UNC\server\share\"), ('q', r"\\?\UNC\server\other\")]);
        let by_letter = job_of([movie(1, r"P:\out\a.mp4")]);
        assert_eq!(clash(&by_letter, &movie(9, r"\\server\share\out\a.mp4")), Some((1, 1, 0)), "UNC after the letter");
        assert_eq!(clash(&by_letter, &movie(9, r"\\SERVER\Share\out\A.MP4")), Some((1, 1, 0)), "in another case");
        assert_eq!(clash(&by_letter, &movie(9, r"\\?\UNC\server\share\out\a.mp4")), Some((1, 1, 0)), "verbatim");
        assert_eq!(clash(&by_letter, &movie(9, r"\\server\share\out\b.mp4")), None, "another file");
        assert_eq!(clash(&by_letter, &movie(9, r"\\server\other\out\a.mp4")), None, "another share");
        assert_eq!(clash(&by_letter, &movie(9, r"Q:\out\a.mp4")), None, "another letter, another share");
        let by_unc = job_of([movie(1, r"\\server\share\out\a.mp4")]);
        assert_eq!(clash(&by_unc, &movie(9, r"p:\out\a.mp4")), Some((1, 1, 0)), "the letter after the UNC path");
        assert_eq!(clash(&by_unc, &movie(9, r"p:\out\b.mp4")), None);
        // Outputs that arrive after the first lookups still meet the earlier ones.
        let mut grown = job_of([movie(1, r"P:\out\a.mp4")]);
        assert_eq!(clash(&grown, &movie(9, r"P:\out\a.mp4")), Some((1, 1, 0)));
        assert_eq!(clash(&grown, &movie(9, r"\\server\share\out\a.mp4")), Some((1, 1, 0)));
        grown.push(movie(2, r"\\server\share\out\c.mp4")).unwrap();
        grown.push(movie(3, r"Q:\out\d.mp4")).unwrap();
        assert_eq!(clash(&grown, &movie(9, r"P:\out\c.mp4")), Some((2, 1, 0)));
        assert_eq!(clash(&grown, &movie(9, r"\\server\other\out\d.mp4")), Some((3, 1, 0)));
        assert!(count.get() <= 2, "only the letters P and Q are looked up");
        DRIVE_ROOT.with(|h| *h.borrow_mut() = None);
    }

    /// #3328: the same through the queue a render is refused from.
    #[test]
    #[cfg(windows)]
    fn a_queue_with_a_drive_and_a_unc_spelling_of_one_output_is_refused() {
        drive_roots(&[('p', r"\\?\UNC\server\share\")]);
        let mut s = Session::default();
        s.execute("comp.new", json!({"name": "Main", "width": 16, "height": 16})).unwrap();
        for path in [r"P:\out\shared.mp4", r"\\server\share\out\shared.mp4"] {
            s.execute("renderQueue.add", json!({"format": "h264", "output": path})).unwrap();
        }
        let (outputs, _) = s.queued_outputs(&effectcraft_project::render_queue::OutputFormat::ALL);
        assert_eq!(s.duplicate_outputs(&outputs, &[]).unwrap().len(), 1);
        DRIVE_ROOT.with(|h| *h.borrow_mut() = None);
    }

    /// Queued movies on one letter, or all on UNC paths, look up no drive root.
    #[test]
    #[cfg(windows)]
    fn queued_outputs_on_one_letter_look_up_no_root() {
        let count = drive_roots(&[('p', r"\\?\UNC\server\share\")]);
        let job = job_of([movie(1, r"P:\out\a.mp4"), movie(2, r"p:\out\b.mp4")]);
        assert_eq!(clash(&job, &movie(9, r"p:\OUT\a.mp4")), Some((1, 1, 0)));
        assert_eq!(clash(&job, &movie(9, r"P:\out\c.mp4")), None);
        let unc = job_of([movie(1, r"\\server\share\a.mp4")]);
        assert_eq!(clash(&unc, &movie(9, r"\\server\share\a.mp4")), Some((1, 1, 0)));
        assert_eq!(clash(&unc, &movie(9, r"\\server\share\b.mp4")), None);
        assert_eq!(count.get(), 0);
        DRIVE_ROOT.with(|h| *h.borrow_mut() = None);
    }

    /// #3328: sequences are compared by the place they name too: sequence and sequence, movie
    /// and sequence, sequence and movie, by drive letter against UNC path; ranges that don't
    /// overlap still don't collide.
    #[test]
    #[cfg(windows)]
    fn queued_sequences_collide_under_drive_and_unc_spellings() {
        let count = drive_roots(&[('p', r"\\?\UNC\server\share\"), ('q', r"\\?\UNC\server\other\")]);
        let letter = r"P:\out\f_[#####].png";
        let unc = r"\\server\share\out\f_[#####].png";
        // sequence, then sequence
        let job = job_of([frames(1, letter, 0, 10)]);
        assert_eq!(clash(&job, &frames(9, unc, 5, 10)), Some((1, 5, 5)), "frames 5..10 are shared");
        assert_eq!(clash(&job, &frames(9, unc, 10, 5)), None, "the next frames");
        assert_eq!(clash(&job, &frames(9, r"\\server\other\out\f_[#####].png", 0, 10)), None, "another share");
        assert_eq!(clash(&job, &frames(9, r"\\server\share\out\g_[#####].png", 0, 10)), None, "another name");
        assert_eq!(clash(&job, &frames(9, r"Q:\out\f_[#####].png", 0, 10)), None, "another letter, another share");
        let job = job_of([frames(1, unc, 0, 10)]);
        assert_eq!(clash(&job, &frames(9, letter, 9, 3)), Some((1, 1, 9)), "UNC first, the letter after");
        assert_eq!(clash(&job, &frames(9, letter, 10, 3)), None);
        // a movie that is a frame of an earlier sequence
        let job = job_of([frames(1, letter, 0, 10)]);
        assert_eq!(clash(&job, &movie(9, r"\\server\share\out\f_00003.png")), Some((1, 1, 3)), "file after sequence");
        assert_eq!(clash(&job, &movie(9, r"\\server\share\out\f_00010.png")), None, "beyond its frames");
        assert_eq!(clash(&job, &movie(9, r"\\server\other\out\f_00003.png")), None, "another share");
        // a sequence that has a frame of an earlier movie
        let job = job_of([movie(1, r"\\server\share\out\f_00003.png")]);
        assert_eq!(clash(&job, &frames(9, letter, 0, 10)), Some((1, 1, 3)), "sequence after file");
        assert_eq!(clash(&job, &frames(9, letter, 4, 10)), None, "its frames start after it");
        let job = job_of([movie(1, r"P:\out\f_00003.png")]);
        assert_eq!(clash(&job, &frames(9, unc, 0, 10)), Some((1, 1, 3)), "the other way round");
        assert!(count.get() <= 2, "P and Q only");
        DRIVE_ROOT.with(|h| *h.borrow_mut() = None);
    }

    /// A subst drive and the local letter of its folder name one place, and the roots' own
    /// separators don't double up.
    #[test]
    #[cfg(windows)]
    fn a_subst_drive_and_a_local_letter_name_one_place() {
        drive_roots(&[('x', r"\\?\C:\work"), ('y', r"\\?\C:\"), ('c', r"\\?\C:\")]);
        let job = job_of([movie(1, r"C:\work\out\a.mp4")]);
        assert_eq!(clash(&job, &movie(9, r"X:\out\a.mp4")), Some((1, 1, 0)), "X: is C:/work");
        assert_eq!(clash(&job, &movie(9, r"X:\out\b.mp4")), None);
        let job = job_of([movie(1, r"X:\out\a.mp4")]);
        assert_eq!(clash(&job, &movie(9, r"c:\WORK\out\a.mp4")), Some((1, 1, 0)), "the other way round");
        assert_eq!(clash(&job, &movie(9, r"c:\out\a.mp4")), None, "C: itself is not C:/work");
        let job = job_of([movie(1, r"C:\out\a.mp4")]);
        assert_eq!(clash(&job, &movie(9, r"Y:\out\a.mp4")), Some((1, 1, 0)), "Y: is C:/ itself");
        let job = job_of([frames(1, r"X:\out\f_[#####].png", 0, 5)]);
        assert_eq!(clash(&job, &frames(9, r"C:\work\out\f_[#####].png", 4, 5)), Some((1, 1, 4)));
        DRIVE_ROOT.with(|h| *h.borrow_mut() = None);
    }

    /// `..` segments and trailing separators don't hide a place, by letter or UNC path.
    #[test]
    #[cfg(windows)]
    fn dot_segments_and_trailing_separators_do_not_hide_a_place() {
        drive_roots(&[('p', r"\\?\UNC\server\share\")]);
        let job = job_of([movie(1, r"P:\out\..\out\a.mp4\")]);
        assert_eq!(clash(&job, &movie(9, r"\\server\share\out\a.mp4")), Some((1, 1, 0)));
        assert_eq!(clash(&job, &movie(9, r"\\server\share\x\..\out\.\a.mp4\")), Some((1, 1, 0)));
        assert_eq!(clash(&job, &movie(9, r"\\server\share\..\..\out\a.mp4")), Some((1, 1, 0)), "`..` stops at the share");
        assert_eq!(clash(&job, &movie(9, r"\\server\share\out\..\a.mp4")), None, "another folder");
        let job = job_of([movie(1, r"\\server\share\out\a.mp4\")]);
        assert_eq!(clash(&job, &movie(9, r"p:\out\sub\..\a.mp4")), Some((1, 1, 0)));
        assert_eq!(clash(&job, &movie(9, r"p:\..\out\a.mp4")), Some((1, 1, 0)), "`..` stops at the drive root");
        DRIVE_ROOT.with(|h| *h.borrow_mut() = None);
    }

    /// A letter whose root can't be resolved keeps its spelling: it collides with itself, not
    /// with a UNC path or another letter, and nothing fails.
    #[test]
    #[cfg(windows)]
    fn an_unresolved_letter_keeps_its_spelling_in_the_queue() {
        drive_roots(&[('p', r"\\?\UNC\server\share\")]);
        let job = job_of([movie(1, r"Q:\out\a.mp4"), frames(2, r"Q:\seq\f_[#####].png", 0, 5)]);
        assert_eq!(clash(&job, &movie(9, r"q:\OUT\a.mp4")), Some((1, 1, 0)), "the letter still matches");
        assert_eq!(clash(&job, &movie(9, r"\\server\share\out\a.mp4")), None, "not a UNC path");
        assert_eq!(clash(&job, &movie(9, r"P:\out\a.mp4")), None, "not another letter");
        assert_eq!(clash(&job, &movie(9, r"q:\seq\f_00002.png")), Some((2, 1, 2)));
        assert_eq!(clash(&job, &frames(9, r"Q:\seq\f_[#####].png", 4, 3)), Some((2, 1, 4)));
        assert_eq!(clash(&job, &frames(9, r"\\server\share\seq\f_[#####].png", 0, 5)), None);
        assert_eq!(clash(&job, &movie(9, r"\\server\share\seq\f_00002.png")), None);
        DRIVE_ROOT.with(|h| *h.borrow_mut() = None);
    }

    /// The first UNC path arriving after the resolved map was built (or the first output on a
    /// second letter) is compared with the outputs that were there before.
    #[test]
    #[cfg(windows)]
    fn a_late_first_unc_path_meets_the_earlier_letters() {
        drive_roots(&[('p', r"\\?\UNC\server\share\"), ('q', r"\\?\UNC\server\other\")]);
        let mut job = job_of([movie(1, r"P:\out\a.mp4"), movie(2, r"P:\out\b.mp4")]);
        assert_eq!(clash(&job, &movie(9, r"P:\out\b.mp4")), Some((2, 1, 0)), "one letter: by name");
        assert_eq!(clash(&job, &movie(9, r"\\server\share\out\b.mp4")), Some((2, 1, 0)), "the map is built");
        job.push(movie(3, r"\\server\share\out\c.mp4")).unwrap();
        assert_eq!(clash(&job, &movie(9, r"P:\out\c.mp4")), Some((3, 1, 0)), "the first UNC path is in it");
        assert_eq!(clash(&job, &movie(9, r"p:\out\a.mp4")), Some((1, 1, 0)));
        job.push(movie(4, r"p:\out\d.mp4")).unwrap();
        assert_eq!(clash(&job, &movie(9, r"\\server\share\out\d.mp4")), Some((4, 1, 0)), "added to a built map");
        job.push(movie(5, r"Q:\out\e.mp4")).unwrap();
        assert_eq!(clash(&job, &movie(9, r"\\server\other\out\e.mp4")), Some((5, 1, 0)), "a new letter");
        // a first sequence on a new place after the map was built
        job.push(frames(6, r"\\server\third\out\f_[#####].png", 0, 5)).unwrap();
        assert_eq!(clash(&job, &movie(9, r"\\server\third\out\f_00001.png")), Some((6, 1, 1)));
        assert_eq!(clash(&job, &movie(9, r"P:\out\d.mp4")), Some((4, 1, 0)));
        DRIVE_ROOT.with(|h| *h.borrow_mut() = None);
    }

    /// A queue of thousands of outputs, some by drive letter and some by UNC path, is built
    /// without looking a drive up per output (the resolver is asked once per letter) and without
    /// the comparisons costing a lock or an allocation per pair of outputs.
    #[test]
    #[cfg(windows)]
    fn a_big_mixed_queue_is_built_with_few_lookups() {
        let count = drive_roots(&[('p', r"\\?\UNC\server\share\")]);
        let started = std::time::Instant::now();
        let mut job = Writes::default();
        let mut first_clash = None;
        for i in 0..5000u64 {
            let path = if i % 2 == 0 { format!(r"P:\out\f{i}.mp4") } else { format!(r"\\server\share\out2\f{i}.mp4") };
            let write = if i % 50 == 0 { frames(i + 1, &format!(r"P:\seq{i}\f_[#####].png"), 0, 10) } else { movie(i + 1, &path) };
            if let Some(found) = clash(&job, &write) {
                first_clash = Some(found);
            }
            job.push(write).unwrap();
        }
        assert_eq!(first_clash, None, "all of them are different files");
        // The same files by the other spelling, and a frame of a sequence.
        assert_eq!(clash(&job, &movie(9, r"\\server\share\out\f2.mp4")), Some((3, 1, 0)));
        assert_eq!(clash(&job, &movie(9, r"P:\out2\f1.mp4")), Some((2, 1, 0)));
        assert_eq!(clash(&job, &movie(9, r"\\server\share\seq100\f_00004.png")), Some((101, 1, 4)));
        assert_eq!(clash(&job, &frames(9, r"\\server\share\seq100\f_[#####].png", 9, 5)), Some((101, 1, 9)));
        assert_eq!(clash(&job, &frames(9, r"\\server\share\seq100\f_[#####].png", 10, 5)), None);
        assert!(count.get() <= 2, "{} lookups", count.get());
        assert!(started.elapsed() < std::time::Duration::from_secs(2), "{:?}", started.elapsed());
        DRIVE_ROOT.with(|h| *h.borrow_mut() = None);
    }

    /// A sequence alone brings the first UNC path, or a new letter, after the resolved places
    /// were made; a file queried against it collides.
    #[test]
    #[cfg(windows)]
    fn a_sequence_alone_brings_a_new_place_after_the_places_were_made() {
        drive_roots(&[('p', r"\\?\UNC\server\share\"), ('q', r"\\?\UNC\server\other\")]);
        let mut job = job_of([movie(1, r"P:\out\a.mp4"), frames(2, r"P:\seq\f_[#####].png", 0, 5)]);
        // (a UNC file queried: the places and the map are made)
        assert_eq!(clash(&job, &movie(9, r"\\server\share\out\a.mp4")), Some((1, 1, 0)));
        assert_eq!(clash(&job, &movie(9, r"\\server\share\seq\f_00001.png")), Some((2, 1, 1)));
        // the first UNC path comes with a sequence
        job.push(frames(3, r"\\server\share\unc\f_[#####].png", 0, 5)).unwrap();
        assert_eq!(clash(&job, &movie(9, r"P:\unc\f_00002.png")), Some((3, 1, 2)), "a file by letter, the sequence by UNC");
        assert_eq!(clash(&job, &movie(9, r"P:\unc\f_00005.png")), None, "beyond its frames");
        // a new letter comes with a sequence
        job.push(frames(4, r"Q:\other\f_[#####].png", 0, 5)).unwrap();
        assert_eq!(clash(&job, &movie(9, r"\\server\other\other\f_00003.png")), Some((4, 1, 3)), "a file by UNC, the sequence by letter");
        assert_eq!(clash(&job, &movie(9, r"\\server\share\other\f_00003.png")), None, "another share");
        // places already made are extended by a sequence on a known place
        job.push(frames(5, r"p:\more\f_[#####].png", 0, 5)).unwrap();
        assert_eq!(clash(&job, &movie(9, r"\\server\share\more\f_00004.png")), Some((5, 1, 4)));
        assert_eq!(clash(&job, &frames(9, r"\\server\share\more\f_[#####].png", 4, 5)), Some((5, 1, 4)));
        DRIVE_ROOT.with(|h| *h.borrow_mut() = None);
    }

    /// With a real mapped network drive (any letter whose root resolves to a UNC path): a frame
    /// by letter is refused under its UNC path. Ignored: it needs such a drive (run it with
    /// `--ignored` where there is one).
    #[test]
    #[ignore]
    #[cfg(windows)]
    fn a_real_mapped_drive_is_its_unc_path() {
        let Some((letter, root)) = ('d'..='z').find_map(|l| drive_root(l).filter(|r| r.to_string_lossy().starts_with(r"\\?\UNC\")).map(|r| (l, r))) else {
            eprintln!("skipped: no mapped network drive here");
            return;
        };
        let unc = format!(r"\\{}", root.to_string_lossy().trim_start_matches(r"\\?\UNC\"));
        let protected = guarding(&[&format!(r"{letter}:\effectcraft-3330\f_0001.exr")]);
        let output = format!(r"{}\effectcraft-3330\f_0001.exr", unc.trim_end_matches('\\'));
        assert!(protected.find(Path::new(&output)).is_some(), "{letter}: is {unc}: {output}");
    }
}
