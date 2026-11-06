//! Output files: the file system, or an in-memory [`Sink`] chosen per job (the web app, which
//! offers every written file as a download; tests).
//!
//! A file is written to a hidden temporary sibling and renamed over its destination only when
//! [`Out::finish`] succeeds, so a cancelled or failed export never truncates or deletes a file
//! that was already there (#1230).

use std::io::{BufWriter, Cursor, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::{ExportError, Result, io};

/// Receives each finished output file (path as resolved, complete contents) instead of the file
/// system. Called once per file; image sequences call it from the render threads.
pub type Sink = dyn Fn(&str, Vec<u8>) + Send + Sync;

/// One output file being written.
pub(crate) enum Out<'a> {
    File(FileOut),
    Mem { path: String, cur: Cursor<Vec<u8>>, sink: &'a Sink },
}

/// A file being written to `tmp`, published to `dest` by [`Out::finish`]. Dropped unfinished (an
/// error or a cancel), it removes `tmp` and leaves `dest` as it was.
pub(crate) struct FileOut {
    w: Option<BufWriter<std::fs::File>>,
    tmp: PathBuf,
    dest: PathBuf,
}

impl FileOut {
    fn writer(&mut self) -> std::io::Result<&mut BufWriter<std::fs::File>> {
        self.w.as_mut().ok_or_else(|| std::io::Error::other("output already finished"))
    }
}

impl Drop for FileOut {
    fn drop(&mut self) {
        if let Some(w) = self.w.take() {
            drop(w);
            let _ = std::fs::remove_file(&self.tmp);
        }
    }
}

/// A new, empty temporary file next to `dest` (same directory, so publishing is a rename on one
/// volume). `create_new` never opens a file that is already there.
fn temp_sibling(dest: &Path) -> Result<(std::fs::File, PathBuf)> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let dir = dest.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let name = dest.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let mut last = None;
    for _ in 0..16 {
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let tmp = dir.join(format!(".{name}.{}-{n}.part", std::process::id()));
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&tmp) {
            Ok(f) => return Ok((f, tmp)),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => last = Some(e),
            Err(e) => return Err(io(e)),
        }
    }
    Err(io(last.map_or_else(|| "no free temporary name".to_string(), |e| e.to_string())))
}

/// Create `path` (parent directories included) or an in-memory file for `sink`. On disk the
/// contents go to a temporary sibling until [`Out::finish`].
pub(crate) fn create<'a>(sink: Option<&'a Sink>, path: &str) -> Result<Out<'a>> {
    match sink {
        Some(sink) => Ok(Out::Mem { path: path.to_string(), cur: Cursor::new(Vec::new()), sink }),
        None => {
            let dest = PathBuf::from(path);
            if let Some(dir) = dest.parent().filter(|d| !d.as_os_str().is_empty()) {
                std::fs::create_dir_all(dir).map_err(io)?;
            }
            let (f, tmp) = temp_sibling(&dest)?;
            Ok(Out::File(FileOut { w: Some(BufWriter::new(f)), tmp, dest }))
        }
    }
}

/// Whether `path` already exists (never for a sink: it has no memory of earlier jobs).
pub(crate) fn exists(sink: Option<&Sink>, path: &str) -> bool {
    sink.is_none() && std::path::Path::new(path).exists()
}

impl Out<'_> {
    /// Flush and close; a sink receives the contents now. Returns the file size.
    pub(crate) fn finish(self) -> Result<u64> {
        match self {
            Out::File(mut fo) => {
                let Some(mut w) = fo.w.take() else { return Err(ExportError::Io("output already finished".into())) };
                // From here on `fo` no longer owns an open file; on failure remove the
                // temporary file ourselves and leave the destination alone.
                let publish = || -> Result<u64> {
                    w.flush().map_err(io)?;
                    let f = w.into_inner().map_err(|e| ExportError::Io(e.to_string()))?;
                    let n = f.metadata().map(|m| m.len()).unwrap_or(0);
                    // Closed before the rename (Windows can't rename an open file).
                    drop(f);
                    std::fs::rename(&fo.tmp, &fo.dest).map_err(|e| ExportError::Io(format!("cannot write {}: {e}", fo.dest.display())))?;
                    Ok(n)
                };
                let r = publish();
                if r.is_err() {
                    let _ = std::fs::remove_file(&fo.tmp);
                }
                r
            }
            Out::Mem { path, cur, sink } => {
                let data = cur.into_inner();
                let n = data.len() as u64;
                sink(&path, data);
                Ok(n)
            }
        }
    }
}

impl Write for Out<'_> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match self {
            Out::File(f) => f.writer()?.write(buf),
            Out::Mem { cur, .. } => cur.write(buf),
        }
    }
    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Out::File(f) => f.writer()?.flush(),
            Out::Mem { .. } => Ok(()),
        }
    }
}

impl Seek for Out<'_> {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        match self {
            Out::File(f) => f.writer()?.seek(pos),
            Out::Mem { cur, .. } => cur.seek(pos),
        }
    }
}
