//! Native pre-render files stay private until the app can check the current project.

#[cfg(not(target_arch = "wasm32"))]
use crate::EngineError;
use crate::Session;

pub(super) const MAX_MOVIES: usize = 4096;

pub(super) struct Movie {
    pub path: String,
    #[cfg(not(target_arch = "wasm32"))]
    directory: tempfile::TempDir,
    #[cfg(not(target_arch = "wasm32"))]
    destination: std::path::PathBuf,
}

impl Movie {
    pub fn new(path: &str) -> Result<Self, String> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let path = std::path::Path::new(path);
            let parent = path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(std::path::Path::new("."));
            std::fs::create_dir_all(parent).map_err(|e| format!("cannot create pre-render folder {}: {e}", parent.display()))?;
            let directory =
                tempfile::Builder::new().prefix(".effectcraft-prerender-").tempdir_in(parent).map_err(|e| format!("cannot stage pre-render: {e}"))?;
            let name = path.file_name().ok_or_else(|| format!("pre-render has no file name: {}", path.display()))?;
            let staged = directory.path().join(name).to_string_lossy().into_owned();
            Ok(Self { path: staged, directory, destination: parent.to_path_buf() })
        }
        #[cfg(target_arch = "wasm32")]
        {
            // spawn_task is synchronous on WASM, so session mutations cannot interleave.
            Ok(Self { path: path.to_string() })
        }
    }
}

pub(super) fn commit(movies: Vec<Movie>, session: &Session) -> crate::Result<()> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        const FILES_PER_MOVIE: usize = 8;
        if movies.len() > MAX_MOVIES {
            return Err(EngineError::Other("too many staged pre-render movies".into()));
        }
        let mut files = Vec::new();
        let mut path_bytes = 0usize;
        for movie in &movies {
            let entries = std::fs::read_dir(movie.directory.path()).map_err(|e| EngineError::Other(format!("cannot read staged pre-render: {e}")))?;
            for (index, entry) in entries.take(FILES_PER_MOVIE + 1).enumerate() {
                if index == FILES_PER_MOVIE {
                    return Err(EngineError::Other("too many files in a staged pre-render".into()));
                }
                let entry = entry.map_err(|e| EngineError::Other(format!("cannot read staged pre-render entry: {e}")))?;
                let kind = entry.file_type().map_err(|e| EngineError::Other(format!("cannot inspect staged pre-render: {e}")))?;
                if !kind.is_file() {
                    return Err(EngineError::Other("a staged pre-render contains an unexpected directory or link".into()));
                }
                let source = entry.path();
                let destination = movie.destination.join(entry.file_name());
                path_bytes = path_bytes.saturating_add(source.as_os_str().len()).saturating_add(destination.as_os_str().len());
                if path_bytes > 16 * 1024 * 1024 {
                    return Err(EngineError::Other("staged pre-render paths exceed the 16 MiB limit".into()));
                }
                files.push((source, destination));
            }
        }
        // If a user imported a private staging file, leave the staging folders intact.
        let protected = session.protected_files();
        if let Some((source, _)) = files.iter().find(|(source, _)| protected.find(source).is_some()) {
            let source = source.display().to_string();
            for movie in movies {
                let _kept = movie.directory.keep();
            }
            return Err(EngineError::Other(format!("staged pre-render {source} is now used by the project; its temporary folder was kept")));
        }
        // Preflight every destination before replacing any output, including render logs.
        for (_, destination) in &files {
            if let Some(found) = protected.find(destination) {
                return Err(EngineError::Other(found.refusal(&destination.to_string_lossy(), crate::output_guard::Use::Export)));
            }
        }
        for (source, destination) in files {
            std::fs::rename(&source, &destination).map_err(|e| EngineError::Other(format!("cannot publish pre-render {}: {e}", destination.display())))?;
        }
    }
    #[cfg(target_arch = "wasm32")]
    let _ = (movies, session);
    Ok(())
}
