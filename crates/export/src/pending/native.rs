use std::{
    fs::{self, File, Metadata},
    io,
    path::{Path, PathBuf},
    time::SystemTime,
};

// Coordinate this process's exports. Uncooperating external writers can still race
// the final version check; ordinary filesystem replacement is not compare-and-swap.
pub(super) static PUBLICATION: std::sync::Mutex<()> = std::sync::Mutex::new(());

pub(super) struct Staged {
    temporary: Option<tempfile::TempPath>,
    pub path: PathBuf,
    requested: PathBuf,
    original: serde_json::Value,
}

/// Inspect ordinary metadata for confirmation without opening the destination for writing.
pub(super) fn inspect(path: &str) -> io::Result<(String, serde_json::Value, bool)> {
    let requested = requested_path(path)?;
    let key = unicode(&requested)?.to_string();
    let resolved = resolve(&requested, false)?;
    let parent = resolved.parent().ok_or_else(|| io::Error::other("output has no parent directory"))?;
    let (_, anchor) = canonical_parent(parent)?;
    let state = readable_state(&resolved)?;
    let existing = !state.is_null();
    Ok((
        key,
        serde_json::json!({
            "resolved": unicode(&resolved)?, "state": state,
            "anchor": unicode(&anchor)?, "anchorPolicy": permissions_key(&fs::metadata(&anchor)?),
        }),
        existing,
    ))
}

pub(super) fn validate_approval(path: &str, authorization: Option<&serde_json::Value>) -> io::Result<()> {
    let requested = requested_path(path)?;
    let resolved = resolve(&requested, false)?;
    let Some(authorization) = authorization else {
        return match fs::symlink_metadata(&resolved) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
            Ok(_) => Err(io::Error::other(format!("overwrite confirmation required for {}", requested.display()))),
        };
    };
    let targets = crate::preflight::token_targets(authorization)?;
    let entry = targets.get(unicode(&requested)?).ok_or_else(|| io::Error::other(format!("export destination was not reviewed: {}", requested.display())))?;
    if entry.get("resolved").and_then(serde_json::Value::as_str) != Some(unicode(&resolved)?) || entry.get("state") != Some(&readable_state(&resolved)?) {
        return Err(io::Error::other(format!("export destination changed since preflight: {}", requested.display())));
    }
    let anchor = entry
        .get("anchor")
        .and_then(serde_json::Value::as_str)
        .filter(|path| path.len() <= 65_536)
        .ok_or_else(|| io::Error::other("invalid export authorization directory"))?;
    if entry.get("anchorPolicy") != Some(&permissions_key(&fs::metadata(anchor)?)) {
        return Err(io::Error::other("export directory policy changed since preflight"));
    }
    Ok(())
}

fn unicode(path: &Path) -> io::Result<&str> {
    path.to_str().filter(|path| path.len() <= 65_536).ok_or_else(|| io::Error::other("export preflight resolved path is non-Unicode or exceeds 65536 bytes"))
}

fn requested_path(path: &str) -> io::Result<PathBuf> {
    if path.len() > 65_536 {
        return Err(io::Error::other("export preflight path exceeds 65536 bytes"));
    }
    std::path::absolute(path)
}

fn readable_state(path: &Path) -> io::Result<serde_json::Value> {
    #[cfg(windows)]
    let file = {
        use std::os::windows::fs::OpenOptionsExt;
        match fs::OpenOptions::new().access_mode(0).open(path) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(serde_json::Value::Null),
            Err(error) => return Err(error),
        }
    };
    #[cfg(windows)]
    let metadata = file.metadata()?;
    #[cfg(not(windows))]
    let metadata = match fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(serde_json::Value::Null),
        Err(error) => return Err(error),
    };
    if !metadata.is_file() {
        return Err(io::Error::other("export destination must be a regular file"));
    }
    #[cfg(windows)]
    let identity = {
        let info = winapi_util::file::information(&file)?;
        (info.volume_serial_number(), info.file_index())
    };
    #[cfg(unix)]
    let identity = {
        use std::os::unix::fs::MetadataExt;
        (metadata.dev(), metadata.ino())
    };
    #[cfg(unix)]
    let change = {
        use std::os::unix::fs::MetadataExt;
        (metadata.ctime(), metadata.ctime_nsec())
    };
    #[cfg(not(unix))]
    let change = timestamp(metadata.created()?);
    Ok(serde_json::json!({
        "identity": identity, "change": change, "len": metadata.len(),
        "modified": timestamp(metadata.modified()?), "permissions": permissions_key(&metadata),
    }))
}

fn timestamp(time: SystemTime) -> (bool, u64, u32) {
    match time.duration_since(SystemTime::UNIX_EPOCH) {
        Ok(duration) => (false, duration.as_secs(), duration.subsec_nanos()),
        Err(error) => (true, error.duration().as_secs(), error.duration().subsec_nanos()),
    }
}

fn permissions_key(metadata: &Metadata) -> serde_json::Value {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        serde_json::json!(metadata.permissions().mode())
    }
    #[cfg(not(unix))]
    serde_json::json!(metadata.permissions().readonly())
}

impl Staged {
    /// Encode into a unique sibling created by tempfile; never truncate a final path.
    pub fn create(path: &str) -> io::Result<(Self, File)> {
        let requested = requested_path(path)?;
        let path = resolve(&requested, true)?;
        let original = readable_state(&path)?;
        let parent = path.parent().ok_or_else(|| io::Error::other("output has no parent directory"))?;
        let mut builder = tempfile::Builder::new();
        builder.prefix(".effectcraft-export-");
        #[cfg(unix)]
        let existing_permissions = {
            use std::os::unix::fs::PermissionsExt;
            let permissions = if original.is_null() { None } else { Some(fs::metadata(&path)?.permissions()) };
            builder.permissions(permissions.clone().unwrap_or_else(|| fs::Permissions::from_mode(0o666)));
            permissions
        };
        let (file, temporary) = builder.tempfile_in(parent)?.into_parts();
        #[cfg(unix)]
        if let Some(permissions) = existing_permissions {
            // Creation applies umask even to an existing mode. Retain that mode on the
            // owned staging file, including bits the current umask would remove.
            file.set_permissions(permissions)?;
        }
        let staged = Self { temporary: Some(temporary), path, requested, original };
        staged.validate()?;
        Ok((staged, file))
    }

    pub fn validate(&self) -> io::Result<()> {
        if resolve(&self.requested, false)? != self.path || readable_state(&self.path)? != self.original {
            return Err(io::Error::other(format!("export destination changed while rendering: {}", self.path.display())));
        }
        Ok(())
    }

    /// Check every completed staging file before the first sequence publication.
    pub fn prepare(&self) -> io::Result<()> {
        self.validate()?;
        let temporary = self.temporary.as_ref().ok_or_else(|| io::Error::other("export staging file already published"))?;
        if !fs::metadata(temporary)?.is_file() {
            return Err(io::Error::other("export staging path is not a regular file"));
        }
        Ok(())
    }

    /// Delegate same-directory publication and ordinary overwrite semantics to tempfile.
    pub fn publish_prepared(&mut self) -> io::Result<()> {
        self.validate()?;
        let temporary = self.temporary.take().ok_or_else(|| io::Error::other("export staging file already published"))?;
        let result = if self.original.is_null() { temporary.persist_noclobber(&self.path) } else { temporary.persist(&self.path) };
        if let Err(error) = result {
            self.temporary = Some(error.path);
            return Err(error.error);
        }
        Ok(())
    }

    /// Clean only the owned temporary file. Successful publication consumed it already.
    pub fn discard(self) -> io::Result<()> {
        if let Some(temporary) = self.temporary
            && let Err(error) = temporary.close()
            && error.kind() != io::ErrorKind::NotFound
        {
            return Err(io::Error::new(error.kind(), format!("cannot remove staging for {}: {error}", self.path.display())));
        }
        Ok(())
    }

    #[cfg(test)]
    fn publish(mut self) -> io::Result<()> {
        let _publication = PUBLICATION.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        self.prepare()?;
        let result = self.publish_prepared();
        self.discard()?;
        result
    }
}

/// Resolve leaf links explicitly, including dangling relative links. The OS resolves directory
/// links with its own loop checks; our leaf-link walk has a fixed bound.
fn resolve(requested: &Path, create_parents: bool) -> io::Result<PathBuf> {
    let mut path = requested.to_path_buf();
    for _ in 0..40 {
        let parent = path.parent().ok_or_else(|| io::Error::other("output has no parent directory"))?;
        if create_parents {
            fs::create_dir_all(parent)?;
        }
        let name = path.file_name().ok_or_else(|| io::Error::other("output has no filename"))?;
        path = canonical_parent(parent)?.0.join(name);
        match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                let target = fs::read_link(&path)?;
                path = if target.is_absolute() { target } else { path.parent().ok_or_else(|| io::Error::other("link has no parent"))?.join(target) };
            }
            // Include the leaf's actual on-disk spelling (not only its parent) in the
            // reviewed destination. Existing Windows case aliases must compare equal.
            Ok(_) => return path.canonicalize(),
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(path),
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::other("export destination has too many symbolic links"))
}

/// Canonicalize the existing ancestor, then normalize only its missing descendants.
/// A preflight can review a new folder without creating it; existing symlinks still
/// resolve through the OS before any lexical normalization of missing components.
fn canonical_parent(parent: &Path) -> io::Result<(PathBuf, PathBuf)> {
    let mut cursor = parent.to_path_buf();
    let mut missing = Vec::new();
    loop {
        match cursor.canonicalize() {
            Ok(anchor) => {
                let mut resolved = anchor.clone();
                for component in missing.iter().rev() {
                    if component == std::ffi::OsStr::new("..") {
                        resolved.pop();
                    } else if component != std::ffi::OsStr::new(".") {
                        resolved.push(component);
                    }
                }
                return Ok((resolved, anchor));
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                if missing.len() >= 1024 {
                    return Err(io::Error::other("export directory has too many missing components"));
                }
                let component = cursor.components().next_back().ok_or_else(|| io::Error::other("output has no existing ancestor"))?.as_os_str().to_os_string();
                if !cursor.pop() {
                    return Err(error);
                }
                missing.push(component);
            }
            Err(error) => return Err(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn stage(path: &Path, bytes: &[u8]) -> Staged {
        let (staged, mut file) = Staged::create(path.to_str().unwrap()).unwrap();
        file.write_all(bytes).unwrap();
        file.flush().unwrap();
        drop(file);
        staged
    }

    #[test]
    fn lifecycle_preserves_existing_bytes_until_publish_and_cleans_owned_files() {
        for existing in [false, true] {
            for action in ["drop", "discard", "publish"] {
                let dir = tempfile::tempdir().unwrap();
                let path = dir.path().join("movie.mov");
                if existing {
                    fs::write(&path, b"old complete movie").unwrap();
                }
                let staged = stage(&path, b"new complete movie");
                assert_eq!(fs::read(&path).ok(), existing.then(|| b"old complete movie".to_vec()));
                match action {
                    "publish" => staged.publish().unwrap(),
                    "discard" => staged.discard().unwrap(),
                    _ => drop(staged),
                }
                let expected = if action == "publish" { Some(b"new complete movie".to_vec()) } else { existing.then(|| b"old complete movie".to_vec()) };
                assert_eq!(fs::read(&path).ok(), expected);
                assert_eq!(fs::read_dir(dir.path()).unwrap().count(), usize::from(expected.is_some()));
            }
        }
    }

    #[test]
    fn changed_destination_is_preserved_and_staging_is_cleaned() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("movie.mov");
        for existing in [false, true] {
            if existing {
                fs::write(&path, b"original").unwrap();
            }
            let staged = stage(&path, b"replacement");
            fs::write(&path, b"external writer").unwrap();
            assert!(staged.publish().is_err());
            assert_eq!(fs::read(&path).unwrap(), b"external writer");
            assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
            fs::remove_file(&path).unwrap();
        }
    }

    #[test]
    fn hardlink_replacement_changes_only_the_requested_name() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("movie.mov");
        let alias = dir.path().join("alias.mov");
        fs::write(&path, b"original").unwrap();
        fs::hard_link(&path, &alias).unwrap();
        stage(&path, b"replacement").publish().unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"replacement");
        assert_eq!(fs::read(alias).unwrap(), b"original");
    }

    #[cfg(unix)]
    #[test]
    fn unix_publication_uses_normal_creation_mode_and_existing_mode() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let ordinary = dir.path().join("ordinary");
        drop(File::create(&ordinary).unwrap());
        let creation_mode = fs::metadata(&ordinary).unwrap().permissions().mode() & 0o777;
        let path = dir.path().join("movie.mov");
        stage(&path, b"new output").publish().unwrap();
        assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o777, creation_mode);
        fs::set_permissions(&path, fs::Permissions::from_mode(0o664)).unwrap();
        stage(&path, b"replacement").publish().unwrap();
        assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o664);
        assert_eq!(fs::read(&path).unwrap(), b"replacement");
    }

    #[cfg(windows)]
    #[test]
    fn library_replacement_error_preserves_locked_destination_and_cleans_staging() {
        use std::os::windows::fs::OpenOptionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("movie.mov");
        fs::write(&path, b"original").unwrap();
        let mut staged = stage(&path, b"replacement");
        // Metadata-only access remains possible, but the library cannot replace this
        // file while an ordinary reader denies delete sharing.
        let lock = fs::OpenOptions::new().read(true).share_mode(1).open(&path).unwrap();
        staged.prepare().unwrap();
        let error = staged.publish_prepared().unwrap_err();
        assert!(matches!(error.raw_os_error(), Some(5 | 32)), "{error}");
        staged.discard().unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"original");
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
        drop(lock);
        stage(&path, b"replacement").publish().unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn relative_and_dangling_symlinks_keep_the_link() {
        use std::os::unix::fs::symlink;
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target.mov");
        let link = dir.path().join("link.mov");
        symlink("target.mov", &link).unwrap();
        stage(&link, b"first").publish().unwrap();
        stage(&link, b"second").publish().unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"second");
        assert_eq!(fs::read_link(&link).unwrap(), Path::new("target.mov"));
    }
}
