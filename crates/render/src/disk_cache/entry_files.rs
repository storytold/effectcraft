//! The source files a frame entry on disk was made of (#3184): stored with the pixels, checked
//! when the entry is used. Only the files that frame read are looked at, never every file of the
//! project.
//!
//! Layout (little endian), before the frame's pixel payload:
//! `u32 count`, then per file `u32 path length`, the path (UTF-8), a tag byte and its fields:
//! `0` a file on disk (`u64 size`, `u8 has time`, `i128 time`: nanoseconds since the Unix epoch,
//! negative before it), `1` bytes from the browser's file table (`u64 size`, `u64 content
//! hash`), `2` a file that could not be read.

use std::sync::Arc;

use super::VirtualFileId;
use crate::reads::{FileStamp, MAX_READ_FILES, ReadFile, ReadFiles};

/// Longest path an entry stores (bytes): a longer one is not a path this app opened.
const MAX_PATH_BYTES: usize = 1 << 16;

/// A file's identity as a frame entry keeps it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntryStamp {
    /// Size and modification time (nanoseconds since the Unix epoch, negative before it) on
    /// disk.
    Native { len: u64, modified: Option<i128> },
    /// Size and content hash from the [`VirtualFileId`] hook (the browser's file table).
    Virtual { len: u64, hash: u64 },
    /// It could not be read when the frame was made: it still must not be readable.
    Missing,
}

/// One file of a frame entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EntryFile {
    pub path: Arc<str>,
    pub stamp: EntryStamp,
}

/// Every file a frame entry was made of.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EntryFiles(Vec<EntryFile>);

impl EntryFiles {
    /// The list to store for a frame that read `files`: `None` when the frame can't be checked
    /// against its files later (the list is incomplete, or a file from memory has no identity),
    /// so the frame must not be stored. Native stamps are the ones the files were read with,
    /// never a fresh look at the disk (a file changed during the render keeps the old one).
    pub fn of(files: &ReadFiles, virtual_id: Option<VirtualFileId<'_>>) -> Option<EntryFiles> {
        if !files.is_complete() {
            return None;
        }
        let mut out = Vec::with_capacity(files.len());
        for f in files.files() {
            let stamp = match f.stamp {
                FileStamp::Native { len, modified } => EntryStamp::Native { len, modified },
                FileStamp::Virtual => {
                    let (len, hash) = virtual_id.and_then(|id| id(&f.path))?;
                    EntryStamp::Virtual { len, hash }
                }
                FileStamp::Missing => EntryStamp::Missing,
            };
            out.push(EntryFile { path: f.path.clone(), stamp });
        }
        Some(EntryFiles(out))
    }

    /// The listed files.
    pub fn files(&self) -> &[EntryFile] {
        &self.0
    }

    /// The number of listed files.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether no files are listed.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The files as a render reports them (to keep with the frame in memory).
    pub fn to_read_files(&self) -> ReadFiles {
        self.0
            .iter()
            .map(|f| {
                let stamp = match f.stamp {
                    EntryStamp::Native { len, modified } => FileStamp::Native { len, modified },
                    EntryStamp::Virtual { .. } => FileStamp::Virtual,
                    EntryStamp::Missing => FileStamp::Missing,
                };
                ReadFile::new(f.path.clone(), stamp)
            })
            .collect()
    }

    /// Whether every file still is what the frame was made of: one `metadata` call per file
    /// (and the hook for files from memory).
    pub fn unchanged(&self, virtual_id: Option<VirtualFileId<'_>>) -> bool {
        self.0.iter().all(|f| file_unchanged(f, virtual_id))
    }

    /// Appends the list to `out` in little-endian binary form.
    pub(super) fn encode(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&(self.0.len() as u32).to_le_bytes());
        for f in &self.0 {
            out.extend_from_slice(&(f.path.len() as u32).to_le_bytes());
            out.extend_from_slice(f.path.as_bytes());
            match f.stamp {
                EntryStamp::Native { len, modified } => {
                    out.push(0);
                    out.extend_from_slice(&len.to_le_bytes());
                    out.push(u8::from(modified.is_some()));
                    out.extend_from_slice(&modified.unwrap_or(0).to_le_bytes());
                }
                EntryStamp::Virtual { len, hash } => {
                    out.push(1);
                    out.extend_from_slice(&len.to_le_bytes());
                    out.extend_from_slice(&hash.to_le_bytes());
                }
                EntryStamp::Missing => out.push(2),
            }
        }
    }

    /// The list at the start of `p` and what follows it; `None` when it is damaged.
    pub(super) fn decode(p: &[u8]) -> Option<(EntryFiles, &[u8])> {
        let mut r = Reader(p);
        let n = r.u32()? as usize;
        if n > MAX_READ_FILES {
            return None;
        }
        let mut out = Vec::with_capacity(n);
        for _ in 0..n {
            let len = r.u32()? as usize;
            if len > MAX_PATH_BYTES {
                return None;
            }
            let path: Arc<str> = std::str::from_utf8(r.take(len)?).ok()?.into();
            let stamp = match r.u8()? {
                0 => {
                    let len = r.u64()?;
                    let has = r.u8()?;
                    let t = r.i128()?;
                    match has {
                        0 => EntryStamp::Native { len, modified: None },
                        1 => EntryStamp::Native { len, modified: Some(t) },
                        _ => return None,
                    }
                }
                1 => EntryStamp::Virtual { len: r.u64()?, hash: r.u64()? },
                2 => EntryStamp::Missing,
                _ => return None,
            };
            out.push(EntryFile { path, stamp });
        }
        Some((EntryFiles(out), r.0))
    }
}

/// Whether the file still matches its stored stamp, by its native stamp or by its in-memory identity.
fn file_unchanged(f: &EntryFile, virtual_id: Option<VirtualFileId<'_>>) -> bool {
    let from_memory = || virtual_id.and_then(|id| id(&f.path));
    match f.stamp {
        EntryStamp::Native { len, modified } => FileStamp::native(&f.path) == Some(FileStamp::Native { len, modified }),
        EntryStamp::Virtual { len, hash } => from_memory() == Some((len, hash)),
        // (a folder at the path is still not a file to read)
        EntryStamp::Missing => !std::fs::metadata(&*f.path).is_ok_and(|m| m.is_file()) && from_memory().is_none(),
    }
}

/// Reads little-endian fields off the front of a byte slice.
struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    /// Takes the next `n` bytes, or `None` when fewer remain.
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let head = self.0.get(..n)?;
        self.0 = self.0.get(n..)?;
        Some(head)
    }
    /// Reads one byte.
    fn u8(&mut self) -> Option<u8> {
        self.take(1)?.first().copied()
    }
    /// Reads a little-endian `u32`.
    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }
    /// Reads a little-endian `u64`.
    fn u64(&mut self) -> Option<u64> {
        Some(u64::from_le_bytes(self.take(8)?.try_into().ok()?))
    }
    /// Reads a little-endian `i128`.
    fn i128(&mut self) -> Option<i128> {
        Some(i128::from_le_bytes(self.take(16)?.try_into().ok()?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A sample list of native, missing and virtual files for the tests.
    fn files() -> ReadFiles {
        [
            ReadFile::new("/a.png", FileStamp::Native { len: 3, modified: Some(9) }),
            ReadFile::new("/b.png", FileStamp::Native { len: 4, modified: None }),
            // Before 1970, and past where `u64` nanoseconds end (2554).
            ReadFile::new("/d.png", FileStamp::Native { len: 5, modified: Some(-86_400_000_000_000) }),
            ReadFile::new("/e.png", FileStamp::Native { len: 6, modified: Some(i128::from(u64::MAX) + 1) }),
            ReadFile::new("/c.png", FileStamp::Missing),
            ReadFile::new("/v.mov", FileStamp::Virtual),
        ]
        .into_iter()
        .collect()
    }

    #[test]
    fn lists_round_trip_and_damage_is_caught() {
        let id = |p: &str| (p == "/v.mov").then_some((8, 77));
        let list = EntryFiles::of(&files(), Some(&id)).unwrap();
        assert_eq!(list.files()[5].stamp, EntryStamp::Virtual { len: 8, hash: 77 });
        let mut bytes = vec![];
        list.encode(&mut bytes);
        bytes.extend_from_slice(b"rest");
        let (back, rest) = EntryFiles::decode(&bytes).unwrap();
        assert_eq!((back, rest), (list.clone(), &b"rest"[..]));
        assert_eq!(list.to_read_files(), files());
        // Every truncation is caught (never a panic, never a shorter list).
        let mut plain = vec![];
        list.encode(&mut plain);
        for n in 0..plain.len() {
            assert!(EntryFiles::decode(&plain[..n]).is_none(), "cut at {n}");
        }
        // Bad tags and counts.
        let mut bad = plain.clone();
        bad[0..4].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(EntryFiles::decode(&bad).is_none());
    }

    #[test]
    fn frames_that_cant_be_checked_are_not_stored() {
        let mut incomplete = files();
        incomplete.mark_incomplete();
        let id = |_: &str| Some((1, 1));
        assert!(EntryFiles::of(&incomplete, Some(&id)).is_none(), "an incomplete list");
        assert!(EntryFiles::of(&files(), None).is_none(), "a file from memory without the hook");
        assert!(EntryFiles::of(&ReadFiles::default(), None).is_some_and(|l| l.is_empty()), "a frame that reads no file");
    }

    #[test]
    fn the_check_looks_at_the_listed_files() {
        let dir = std::env::temp_dir().join(format!("effectcraft-entry-files-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("f.bin");
        std::fs::write(&p, b"abc").unwrap();
        let path = p.to_string_lossy().to_string();
        let gone = dir.join("gone.bin").to_string_lossy().to_string();
        let stamp = FileStamp::native(&path).unwrap();
        let read: ReadFiles = [ReadFile::new(path.as_str(), stamp), ReadFile::new(gone.as_str(), FileStamp::Missing)].into_iter().collect();
        let list = EntryFiles::of(&read, None).unwrap();
        assert!(list.unchanged(None));
        std::fs::write(&p, b"abcd").unwrap();
        assert!(!list.unchanged(None), "a new size");
        std::fs::write(&gone, b"x").unwrap();
        let list = EntryFiles::of(&[ReadFile::new(gone.as_str(), FileStamp::Missing)].into_iter().collect(), None).unwrap();
        assert!(!list.unchanged(None), "a missing file that is back");
        let v: ReadFiles = [ReadFile::new("/v", FileStamp::Virtual)].into_iter().collect();
        let one = |_: &str| Some((1, 1));
        let two = |_: &str| Some((1, 2));
        let list = EntryFiles::of(&v, Some(&one)).unwrap();
        assert!(list.unchanged(Some(&one)) && !list.unchanged(Some(&two)) && !list.unchanged(None));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
