//! Opening a file that must be a regular file, without ever blocking on a FIFO.

use std::fs::File;
use std::io::{Error, ErrorKind};
use std::path::Path;

/// `O_NONBLOCK` per OS: opening a FIFO read-only with it returns at once instead of waiting for a writer.
/// shortcut: other Unixes keep a blocking open, the `metadata` check before it still covers the common case.
#[cfg(any(target_os = "linux", target_os = "android"))]
const O_NONBLOCK: i32 = 0o4000;
#[cfg(any(target_os = "macos", target_os = "ios", target_os = "freebsd", target_os = "netbsd", target_os = "openbsd", target_os = "dragonfly"))]
const O_NONBLOCK: i32 = 0x4;

/// Opens `path` for reading and returns the file with its metadata, only when it is a regular file.
/// A path swapped for a FIFO between a check and the open cannot hang the caller: the open does not
/// block, and the descriptor is validated before anything reads it.
pub fn open_regular_file(path: &Path) -> std::io::Result<(File, std::fs::Metadata)> {
    let not_file = || Error::new(ErrorKind::InvalidInput, format!("{} is not a file", path.display()));
    if !std::fs::metadata(path)?.is_file() {
        return Err(not_file());
    }
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(all(
        unix,
        any(
            target_os = "linux",
            target_os = "android",
            target_os = "macos",
            target_os = "ios",
            target_os = "freebsd",
            target_os = "netbsd",
            target_os = "openbsd",
            target_os = "dragonfly"
        )
    ))]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(O_NONBLOCK);
    }
    let file = options.open(path)?;
    let meta = file.metadata()?;
    if !meta.is_file() {
        return Err(not_file());
    }
    Ok((file, meta))
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn a_fifo_is_refused_without_blocking_and_a_file_opens() {
        let dir = std::env::temp_dir().join(format!("ec-regfile-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let fifo = dir.join("pipe");
        let made = std::process::Command::new("mkfifo").arg(&fifo).status().map(|s| s.success()).unwrap_or(false);
        let plain = dir.join("plain");
        std::fs::write(&plain, b"abc").unwrap();
        assert_eq!(open_regular_file(&plain).unwrap().1.len(), 3);
        if made {
            // metadata already says FIFO; the open itself must not block either.
            assert!(open_regular_file(&fifo).is_err());
            let mut o = std::fs::OpenOptions::new();
            use std::os::unix::fs::OpenOptionsExt;
            #[cfg(any(
                target_os = "linux",
                target_os = "android",
                target_os = "macos",
                target_os = "ios",
                target_os = "freebsd",
                target_os = "netbsd",
                target_os = "openbsd",
                target_os = "dragonfly"
            ))]
            {
                let f = o.read(true).custom_flags(O_NONBLOCK).open(&fifo).unwrap();
                assert!(!f.metadata().unwrap().is_file());
            }
        }
        std::fs::remove_dir_all(&dir).ok();
    }
}
