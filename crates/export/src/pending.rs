//! Job-owned staging: failed exports never clean up a final pathname.

use crate::{Result, out};

#[cfg(not(target_arch = "wasm32"))]
mod native;

#[derive(Default)]
pub(crate) struct Pending {
    #[cfg(not(target_arch = "wasm32"))]
    files: std::sync::Mutex<Vec<native::Staged>>,
}

impl Pending {
    /// Open a sink directly or register a unique, private native staging file for this job.
    #[cfg(test)]
    pub fn create<'a>(&self, sink: Option<&'a out::Sink>, path: &str) -> Result<out::Out<'a>> {
        self.create_authorized(sink, path, None)
    }

    /// Bind native staging to the destinations reviewed by the host's confirmation.
    pub fn create_authorized<'a>(&self, sink: Option<&'a out::Sink>, path: &str, _authorization: Option<&serde_json::Value>) -> Result<out::Out<'a>> {
        if sink.is_some() {
            return out::create(sink, path);
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let mut files = self.files.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            native::validate_approval(path, _authorization).map_err(crate::io)?;
            let (staged, file) = native::Staged::create(path).map_err(crate::io)?;
            native::validate_approval(path, _authorization).map_err(crate::io)?;
            if files.iter().any(|other| other.path == staged.path) {
                return Err(crate::io(format!("duplicate export destination: {path}")));
            }
            files.push(staged);
            Ok(out::Out::File(std::io::BufWriter::new(file)))
        }
        #[cfg(target_arch = "wasm32")]
        Err(crate::ExportError::Unsupported("web exports require an output sink".into()))
    }

    /// Each rename publishes one complete file. A sequence is not an atomic transaction:
    /// previously published frames remain if a later publication fails.
    pub fn publish(&self) -> Result<()> {
        #[cfg(not(target_arch = "wasm32"))]
        self.publish_native(native::Staged::publish_prepared)?;
        Ok(())
    }

    /// Keep preparation, final validation, publication, and cleanup in one job-level
    /// critical section. The operation argument also permits deterministic I/O failures
    /// in regression tests without changing filesystem permissions or timing threads.
    #[cfg(not(target_arch = "wasm32"))]
    fn publish_native(&self, mut publish: impl FnMut(&mut native::Staged) -> std::io::Result<()>) -> Result<()> {
        let mut files = self.files.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let _publication = native::PUBLICATION.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        // Check completed staging files before changing any final pathname.
        for staged in files.iter() {
            staged.prepare().map_err(|error| crate::io(format!("export preparation failed for {} before publication: {error}", staged.path.display())))?;
        }
        for staged in files.iter() {
            staged.validate().map_err(crate::io)?;
        }
        let total = files.len();
        let mut published = 0_usize;
        while let Some(mut staged) = files.pop() {
            let result = publish(&mut staged);
            if result.is_ok() {
                published = published.saturating_add(1);
            }
            let path = staged.path.clone();
            let cleanup = staged.discard();
            let error = match (result, cleanup) {
                (Err(error), Err(cleanup)) => Some(format!("{error}; cleanup also failed: {cleanup}")),
                (Err(error), _) | (_, Err(error)) => Some(error.to_string()),
                _ => None,
            };
            if let Some(error) = error {
                let state = if published == total {
                    "all output files were published; staging cleanup failed"
                } else {
                    "already published files remain and the output set may be incomplete or contain mixed generations"
                };
                return Err(crate::io(format!(
                    "export publication/cleanup failed at {}: {error}; {published} of {total} files published; {state}",
                    path.display()
                )));
            }
        }
        Ok(())
    }

    /// Attempt cleanup of every pending file, returning the first cleanup failure.
    pub fn discard(&self) -> Result<()> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let files = std::mem::take(&mut *self.files.lock().unwrap_or_else(std::sync::PoisonError::into_inner));
            let mut failure = None;
            for staged in files {
                if let Err(error) = staged.discard() {
                    failure.get_or_insert(error);
                }
            }
            if let Some(error) = failure {
                return Err(crate::io(error));
            }
        }
        Ok(())
    }
}

/// Inspect a destination without creating it or its missing parent directories.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn inspect(path: &str) -> std::io::Result<(String, serde_json::Value, bool)> {
    native::inspect(path)
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn validate_approval(path: &str, authorization: Option<&serde_json::Value>) -> std::io::Result<()> {
    native::validate_approval(path, authorization)
}

#[cfg(test)]
#[cfg(not(target_arch = "wasm32"))]
mod tests {
    use super::*;
    use std::{fs, io::Write, path::Path};

    fn stage(pending: &Pending, path: &Path, bytes: &[u8]) {
        let mut writer = pending.create(None, path.to_str().unwrap()).unwrap();
        writer.write_all(bytes).unwrap();
        writer.flush().unwrap();
    }

    #[test]
    fn preparation_failure_publishes_no_frames() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("frame_0001.png");
        let b = dir.path().join("frame_0002.png");
        let pending = Pending::default();
        stage(&pending, &a, b"new 1");
        // Remove precisely the first job-owned staging entry in this empty fixture.
        let mut staging = fs::read_dir(dir.path()).unwrap().next().unwrap().unwrap().path();
        if staging.is_dir() {
            staging = fs::read_dir(staging).unwrap().next().unwrap().unwrap().path();
        }
        stage(&pending, &b, b"new 2");
        fs::remove_file(staging).unwrap();
        assert!(pending.publish().is_err());
        assert!(!a.exists());
        assert!(!b.exists());
        pending.discard().unwrap();
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[test]
    fn partial_publication_reports_count_and_retains_complete_files() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("frame_0001.png");
        let b = dir.path().join("frame_0002.png");
        let pending = Pending::default();
        stage(&pending, &a, b"new 1");
        stage(&pending, &b, b"new 2");
        let mut calls = 0;
        let error = pending
            .publish_native(|staged| {
                calls += 1;
                if calls == 2 {
                    return Err(std::io::Error::other("injected publication failure"));
                }
                staged.publish_prepared()
            })
            .unwrap_err()
            .to_string();
        assert!(error.contains("1 of 2 files published"), "{error}");
        assert!(error.contains("mixed generations"), "{error}");
        assert!(error.contains(&a.display().to_string()), "{error}");
        assert!(!a.exists());
        assert_eq!(fs::read(&b).unwrap(), b"new 2");
        pending.discard().unwrap();
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }
}
