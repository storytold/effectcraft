//! The source files a render read: which footage files its pixels came from, each with its
//! identity (size and modification time) as it was when it was read.
//!
//! A [`FootageSource`](crate::FootageSource) that knows which files it opens notes them while
//! [`FootageSource::recording`](crate::FootageSource::recording) runs; the renderer collects them
//! in a [`ReadLog`] ([`Renderer::reads`](crate::Renderer::reads)). Buffers in the layer cache keep
//! the files they were made of, so a frame served from memory reports the same files as the
//! render that made it, without touching the disk.
//!
//! The list is what a cache entry on disk has to check before it is used: a handful of files per
//! frame, never every file of every footage item.

use std::sync::{Arc, Mutex, PoisonError};

/// The identity of a file as a render read it (frame and layer entries on disk store it, see
/// [`EntryFiles`](crate::disk_cache::EntryFiles)).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum FileStamp {
    /// A file on disk: its size in bytes and its modification time (nanoseconds since the Unix
    /// epoch, negative before it; `None` only where the file system has none).
    Native { len: u64, modified: Option<i128> },
    /// Bytes given from memory instead of the file system (the browser's file table): their
    /// identity is the size and content hash a
    /// [`VirtualFileId`](crate::disk_cache::VirtualFileId) gives for the path.
    Virtual,
    /// The file could not be read (missing, not a regular file, no permission). What was made
    /// without it changes once it can be read again.
    Missing,
}

impl FileStamp {
    /// The identity of a file from metadata the caller already has (of an open file).
    pub fn of_metadata(md: &std::fs::Metadata) -> FileStamp {
        FileStamp::Native { len: md.len(), modified: md.modified().ok().map(epoch_nanos) }
    }

    /// The identity of the file at `path` now: one `metadata` call. `None` when the file system
    /// can't describe it (missing, or a path served from memory).
    pub fn native(path: &str) -> Option<FileStamp> {
        std::fs::metadata(path).ok().map(|md| FileStamp::of_metadata(&md))
    }
}

/// `t` in nanoseconds since the Unix epoch, negative before it: every time a file system can
/// store fits (`i128` nanoseconds span far more than any `SystemTime`), saturating otherwise.
pub fn epoch_nanos(t: std::time::SystemTime) -> i128 {
    let nanos = |d: std::time::Duration| i128::try_from(d.as_nanos()).unwrap_or(i128::MAX);
    match t.duration_since(std::time::UNIX_EPOCH) {
        Ok(after) => nanos(after),
        Err(before) => nanos(before.duration()).saturating_neg(),
    }
}

/// One file a render read.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ReadFile {
    pub path: Arc<str>,
    pub stamp: FileStamp,
}

impl ReadFile {
    /// A file read with the given path and stamp.
    pub fn new(path: impl Into<Arc<str>>, stamp: FileStamp) -> ReadFile {
        ReadFile { path: path.into(), stamp }
    }
}

/// At most this many files are listed per render or cached buffer; more mark the list
/// incomplete (a comp reading more files than this for one frame is not checked file by file).
pub const MAX_READ_FILES: usize = 4096;

/// The files a render (or one cached buffer) read, each once, sorted by path.
///
/// Incomplete when it can't say for sure: a footage source that doesn't tell what it reads, a
/// buffer from a store that doesn't keep its files, more than [`MAX_READ_FILES`] files, or one
/// file read with two identities (it changed during the render). A cache entry whose list is
/// incomplete can't be checked against the files, so it must not outlive the session.
///
/// It also carries where its reads come from, which equality and hashing ignore: the drop
/// generation the earliest of them started at ([`ReadFiles::since`]), and whether a layer cache
/// key can't capture what it was made of ([`ReadFiles::is_unkeyed`]).
#[derive(Clone, Debug, Default)]
pub struct ReadFiles {
    files: Vec<ReadFile>,
    incomplete: bool,
    /// The disk cache's drop generation ([`DiskCache::drop_generation`]) when the earliest read
    /// behind the list started; `None` when nothing says (a list built by hand).
    ///
    /// [`DiskCache::drop_generation`]: crate::disk_cache::DiskCache::drop_generation
    since: Option<u64>,
    /// Made of values a layer's cache key doesn't capture (a text Expression Selector evaluates
    /// per character): such a buffer is not stored on disk.
    unkeyed: bool,
}

impl PartialEq for ReadFiles {
    /// The same files with the same identities (where the reads come from doesn't count).
    fn eq(&self, other: &Self) -> bool {
        self.files == other.files && self.incomplete == other.incomplete
    }
}

impl Eq for ReadFiles {}

impl std::hash::Hash for ReadFiles {
    /// Hashes what [`PartialEq`] compares.
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.files.hash(state);
        self.incomplete.hash(state);
    }
}

impl ReadFiles {
    /// Note a file read (once per path).
    pub fn add(&mut self, file: ReadFile) {
        match self.files.binary_search_by(|f| f.path.cmp(&file.path)) {
            Ok(i) => {
                if self.files.get(i).is_some_and(|f| f.stamp != file.stamp) {
                    self.incomplete = true;
                }
            }
            Err(i) => {
                if self.files.len() >= MAX_READ_FILES {
                    self.incomplete = true;
                } else {
                    self.files.insert(i, file);
                }
            }
        }
    }

    /// Note every file of `other` (whether it is incomplete or unkeyed, and its generation).
    pub fn extend(&mut self, other: &ReadFiles) {
        self.incomplete |= other.incomplete;
        self.unkeyed |= other.unkeyed;
        if let Some(g) = other.since {
            self.note_since(g);
        }
        for f in &other.files {
            self.add(f.clone());
        }
    }

    /// Some files read may be missing from the list (see [`ReadFiles`]).
    pub fn mark_incomplete(&mut self) {
        self.incomplete = true;
    }

    /// Whether the list names every file read.
    pub fn is_complete(&self) -> bool {
        !self.incomplete
    }

    /// The drop generation the earliest read behind the list started at (`None`: unknown). A
    /// buffer made of a file dropped after it holds that file as it was before the drop.
    pub fn since(&self) -> Option<u64> {
        self.since
    }

    /// A read behind the list started at drop generation `generation` (the earliest one counts).
    pub fn note_since(&mut self, generation: u64) {
        self.since = Some(self.since.map_or(generation, |s| s.min(generation)));
    }

    /// Made of values a layer cache key doesn't capture (see [`ReadFiles::is_unkeyed`]).
    pub fn mark_unkeyed(&mut self) {
        self.unkeyed = true;
    }

    /// Made of values a layer cache key doesn't capture: kept in memory, never stored on disk as
    /// a layer buffer. (A frame's key captures them: frame entries are not affected.)
    pub fn is_unkeyed(&self) -> bool {
        self.unkeyed
    }

    /// The files, sorted by path.
    pub fn files(&self) -> &[ReadFile] {
        &self.files
    }

    /// The identity `path` was read with.
    pub fn get(&self, path: &str) -> Option<FileStamp> {
        let i = self.files.binary_search_by(|f| (*f.path).cmp(path)).ok()?;
        self.files.get(i).map(|f| f.stamp)
    }

    /// The number of files in the list.
    pub fn len(&self) -> usize {
        self.files.len()
    }

    /// Whether the list holds no files.
    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }
}

impl FromIterator<ReadFile> for ReadFiles {
    /// Collects files into a list, adding each in turn.
    fn from_iter<I: IntoIterator<Item = ReadFile>>(iter: I) -> Self {
        let mut out = ReadFiles::default();
        for f in iter {
            out.add(f);
        }
        out
    }
}

/// Collects the files a render reads, from any thread (the layers of a frame render in
/// parallel).
#[derive(Debug, Default)]
pub struct ReadLog {
    files: Mutex<ReadFiles>,
}

impl ReadLog {
    /// An empty read log.
    pub fn new() -> ReadLog {
        ReadLog::default()
    }

    /// An empty read log for reads that start at drop generation `generation` (see
    /// [`ReadFiles::since`]).
    pub fn starting_at(generation: u64) -> ReadLog {
        let mut files = ReadFiles::default();
        files.note_since(generation);
        ReadLog { files: Mutex::new(files) }
    }

    /// Locks the file list, recovering from a poisoned lock.
    fn lock(&self) -> std::sync::MutexGuard<'_, ReadFiles> {
        self.files.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Note a file read.
    pub fn note(&self, file: ReadFile) {
        self.lock().add(file);
    }

    /// Note every file of `files` (and whether it is incomplete).
    pub fn note_all(&self, files: &ReadFiles) {
        if files.is_empty() && files.is_complete() && files.since.is_none() && !files.unkeyed {
            return;
        }
        self.lock().extend(files);
    }

    /// Some files read can't be listed (see [`ReadFiles`]).
    pub fn mark_incomplete(&self) {
        self.lock().mark_incomplete();
    }

    /// What is read is made of values a layer cache key doesn't capture
    /// ([`ReadFiles::is_unkeyed`]).
    pub fn mark_unkeyed(&self) {
        self.lock().mark_unkeyed();
    }

    /// The files noted so far.
    pub fn files(&self) -> ReadFiles {
        self.lock().clone()
    }

    /// The files noted.
    pub fn into_files(self) -> ReadFiles {
        self.files.into_inner().unwrap_or_else(PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A native file with the given length for the tests.
    fn file(path: &str, len: u64) -> ReadFile {
        ReadFile::new(path, FileStamp::Native { len, modified: Some(7) })
    }

    #[test]
    fn files_are_listed_once_sorted_by_path() {
        let files: ReadFiles = [file("/b", 1), file("/a", 2), file("/b", 1)].into_iter().collect();
        assert_eq!(files.files().iter().map(|f| &*f.path).collect::<Vec<_>>(), ["/a", "/b"]);
        assert!(files.is_complete());
        assert_eq!(files.get("/a"), Some(FileStamp::Native { len: 2, modified: Some(7) }));
        assert_eq!(files.get("/c"), None);
    }

    /// A file read twice with different identities changed during the render: the list can't
    /// vouch for what was made of it.
    #[test]
    fn a_file_read_with_two_identities_makes_the_list_incomplete() {
        let files: ReadFiles = [file("/a", 1), file("/a", 2)].into_iter().collect();
        assert_eq!(files.len(), 1);
        assert!(!files.is_complete());
    }

    /// Modification times keep their full value: before 1970 (not "no time") and past 2554
    /// (where `u64` nanoseconds end), so two such files never look alike.
    #[test]
    fn modification_times_are_kept_whole() {
        use std::time::{Duration, UNIX_EPOCH};
        let day = Duration::from_secs(86_400);
        assert_eq!(epoch_nanos(UNIX_EPOCH), 0);
        assert_eq!(epoch_nanos(UNIX_EPOCH - day), -86_400_000_000_000);
        let far = 600 * 365 * day;
        if let Some(t) = UNIX_EPOCH.checked_add(far) {
            assert_eq!(epoch_nanos(t), i128::try_from(far.as_nanos()).unwrap());
        }
        let dir = std::env::temp_dir().join(format!("effectcraft-stamp-1969-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (a, b) = (dir.join("a.bin"), dir.join("b.bin"));
        let mut stamps = vec![];
        for (p, back) in [(&a, day), (&b, 2 * day)] {
            std::fs::write(p, b"abc").unwrap();
            let f = std::fs::File::options().write(true).open(p).unwrap();
            if f.set_modified(UNIX_EPOCH - back).is_err() {
                // (a file system that can't store times before 1970)
                let _ = std::fs::remove_dir_all(&dir);
                return;
            }
            drop(f);
            stamps.push(FileStamp::native(&p.to_string_lossy()).unwrap());
        }
        assert!(matches!(stamps[0], FileStamp::Native { modified: Some(t), .. } if t < 0), "{:?}", stamps[0]);
        assert_ne!(stamps[0], stamps[1], "two files from before 1970 with the same size");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_list_is_bounded() {
        let files: ReadFiles = (0..MAX_READ_FILES + 3).map(|i| file(&format!("/f{i:06}"), 1)).collect();
        assert_eq!(files.len(), MAX_READ_FILES);
        assert!(!files.is_complete(), "files past the bound make the list incomplete, never silently short");
    }

    #[test]
    fn incompleteness_carries_over() {
        let mut part = ReadFiles::default();
        part.mark_incomplete();
        let log = ReadLog::new();
        log.note(file("/a", 1));
        log.note_all(&part);
        let all = log.into_files();
        assert_eq!(all.len(), 1);
        assert!(!all.is_complete());
    }
}
