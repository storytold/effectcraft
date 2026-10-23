//! Output files: the file system, or an in-memory [`Sink`] chosen per job (the web app, which
//! offers every written file as a download; tests).

use std::io::{BufWriter, Cursor, Seek, SeekFrom, Write};

use crate::{ExportError, Result, io};

/// Receives each finished output file (path as resolved, complete contents) instead of the file
/// system. Called once per file; image sequences call it from the render threads.
pub type Sink = dyn Fn(&str, Vec<u8>) + Send + Sync;

/// One output file being written.
pub(crate) enum Out<'a> {
    File(BufWriter<std::fs::File>),
    Mem { path: String, cur: Cursor<Vec<u8>>, sink: &'a Sink },
}

/// Create `path` (parent directories included) or an in-memory file for `sink`.
pub(crate) fn create<'a>(sink: Option<&'a Sink>, path: &str) -> Result<Out<'a>> {
    match sink {
        Some(sink) => Ok(Out::Mem { path: path.to_string(), cur: Cursor::new(Vec::new()), sink }),
        None => {
            if let Some(dir) = std::path::Path::new(path).parent().filter(|d| !d.as_os_str().is_empty()) {
                std::fs::create_dir_all(dir).map_err(io)?;
            }
            Ok(Out::File(BufWriter::new(std::fs::OpenOptions::new().write(true).create_new(true).open(path).map_err(io)?)))
        }
    }
}

/// Logs never replace a prior pathname. In particular, an existing log symlink or
/// hard link is not permission to modify its unrelated target. Sink delivery keeps
/// the requested callback name; native collisions receive a uniquely created sibling.
pub(crate) fn create_log<'a>(sink: Option<&'a Sink>, path: &str) -> Result<(Out<'a>, String)> {
    if sink.is_some() {
        return Ok((create(sink, path)?, path.to_string()));
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let requested = std::path::Path::new(path);
        let parent = requested.parent().filter(|path| !path.as_os_str().is_empty()).unwrap_or_else(|| std::path::Path::new("."));
        std::fs::create_dir_all(parent).map_err(io)?;
        match std::fs::OpenOptions::new().write(true).create_new(true).open(requested) {
            Ok(file) => return Ok((Out::File(BufWriter::new(file)), path.to_string())),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists || std::fs::symlink_metadata(requested).is_ok() => {}
            Err(error) => return Err(io(error)),
        }
        let stem = requested.file_stem().and_then(|name| name.to_str()).unwrap_or("RenderLog");
        let log = tempfile::Builder::new().prefix(&format!("{stem}-")).suffix(".txt").tempfile_in(parent).map_err(io)?;
        let (file, actual) = log.keep().map_err(io)?;
        let actual = actual.to_str().ok_or_else(|| io("non-Unicode render log path"))?.to_string();
        Ok((Out::File(BufWriter::new(file)), actual))
    }
    #[cfg(target_arch = "wasm32")]
    Err(ExportError::Unsupported("web render logs require an output sink".into()))
}

/// Whether `path` already exists (never for a sink: it has no memory of earlier jobs).
pub(crate) fn exists(sink: Option<&Sink>, path: &str) -> bool {
    sink.is_none() && std::path::Path::new(path).exists()
}

impl Out<'_> {
    /// Flush and close; a sink receives the contents now. Returns the file size.
    pub(crate) fn finish(self) -> Result<u64> {
        match self {
            Out::File(mut w) => {
                w.flush().map_err(io)?;
                let f = w.into_inner().map_err(|e| ExportError::Io(e.to_string()))?;
                Ok(f.metadata().map(|m| m.len()).unwrap_or(0))
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
            Out::File(w) => w.write(buf),
            Out::Mem { cur, .. } => cur.write(buf),
        }
    }
    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Out::File(w) => w.flush(),
            Out::Mem { .. } => Ok(()),
        }
    }
}

impl Seek for Out<'_> {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        match self {
            Out::File(w) => w.seek(pos),
            Out::Mem { cur, .. } => cur.seek(pos),
        }
    }
}

#[cfg(test)]
#[cfg(not(target_arch = "wasm32"))]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn log_creation_preserves_existing_files_directories_and_link_targets() {
        let kinds = if cfg!(unix) { vec!["file", "hardlink", "directory", "symlink", "dangling"] } else { vec!["file", "hardlink", "directory"] };
        for kind in kinds {
            let dir = tempfile::tempdir().unwrap();
            let requested = dir.path().join("Movie_RenderLog.txt");
            let unrelated = dir.path().join("unrelated.ecproj");
            let missing = dir.path().join("missing.ecproj");
            fs::write(&unrelated, b"unrelated project").unwrap();
            match kind {
                "file" => fs::write(&requested, b"previous log").unwrap(),
                "hardlink" => fs::hard_link(&unrelated, &requested).unwrap(),
                "directory" => fs::create_dir(&requested).unwrap(),
                #[cfg(unix)]
                "symlink" => std::os::unix::fs::symlink(&unrelated, &requested).unwrap(),
                #[cfg(unix)]
                "dangling" => std::os::unix::fs::symlink(&missing, &requested).unwrap(),
                _ => panic!("unknown test fixture"),
            }
            let original = fs::read(&requested).ok();
            let (mut log, actual) = create_log(None, requested.to_str().unwrap()).unwrap();
            log.write_all(b"new complete log").unwrap();
            log.finish().unwrap();
            assert_ne!(std::path::Path::new(&actual), requested);
            assert_eq!(fs::read(actual).unwrap(), b"new complete log");
            assert_eq!(fs::read(&requested).ok(), original);
            assert_eq!(fs::read(unrelated).unwrap(), b"unrelated project");
            assert!(!missing.exists());
            #[cfg(unix)]
            if kind == "symlink" || kind == "dangling" {
                assert!(fs::symlink_metadata(&requested).unwrap().file_type().is_symlink());
            }
        }
    }

    #[test]
    fn simultaneous_logs_get_distinct_paths_and_keep_their_own_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let requested = dir.path().join("Movie_RenderLog.txt");
        let barrier = std::sync::Barrier::new(8);
        let logs = std::thread::scope(|scope| {
            let threads: Vec<_> = (0..8)
                .map(|job| {
                    let requested = &requested;
                    let barrier = &barrier;
                    scope.spawn(move || {
                        barrier.wait();
                        let (mut log, actual) = create_log(None, requested.to_str().unwrap()).unwrap();
                        let bytes = format!("complete log from job {job}").into_bytes();
                        log.write_all(&bytes).unwrap();
                        log.finish().unwrap();
                        (actual, bytes)
                    })
                })
                .collect();
            threads.into_iter().map(|thread| thread.join().unwrap()).collect::<Vec<_>>()
        });
        let mut paths = std::collections::HashSet::new();
        for (path, bytes) in logs {
            assert_eq!(fs::read(&path).unwrap(), bytes);
            assert!(paths.insert(path));
        }
        assert_eq!(paths.len(), 8);
    }
}
