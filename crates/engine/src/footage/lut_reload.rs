//! LUT files that change on disk (#3493).
//!
//! A render follows a LUT file by itself: [`effectcraft_effects::load_lut`] stamps the file and
//! reads it again once it changed, and a layer's cache key holds the stamp. What the desktop app
//! keeps of earlier renders, the frames of its viewer and RAM preview, is dropped when
//! [`effectcraft_effects::lut_generation`] moves. The [`Watcher`] looks at the LUT files in use
//! every two seconds, so an edit shows without anything rendering first, and wakes the interface
//! only when one changed.

use std::sync::mpsc::{RecvTimeoutError, SyncSender};
use std::time::Duration;

/// Moves each time a LUT file in use was found changed on disk: frames rendered before may show
/// the old LUT ([`effectcraft_effects::lut_generation`]).
pub fn generation() -> u64 {
    effectcraft_effects::lut_generation()
}

/// Stamp the LUT files in use now ([`effectcraft_effects::check_lut_files`]): the ones that
/// changed are read again by the next render. Returns [`generation`].
pub fn check() -> u64 {
    effectcraft_effects::check_lut_files()
}

/// How often the LUT files in use are stamped.
pub const INTERVAL: Duration = Duration::from_secs(2);

/// A worker thread that stamps the LUT files in use ([`effectcraft_effects::check_lut_files`])
/// and runs `wake` when one changed. Dropping the watcher ends the thread.
pub struct Watcher {
    _stop: SyncSender<()>,
}

impl Watcher {
    /// Start the worker. `None` where there are no threads (the web app, whose LUTs don't come
    /// from files) or one couldn't be started.
    pub fn start(wake: impl Fn() + Send + 'static) -> Option<Watcher> {
        Self::start_with(wake, INTERVAL)
    }

    /// [`Watcher::start`] stamping every `every` (tests).
    pub fn start_with(wake: impl Fn() + Send + 'static, every: Duration) -> Option<Watcher> {
        if cfg!(target_arch = "wasm32") {
            return None;
        }
        let (tx, rx) = std::sync::mpsc::sync_channel::<()>(1);
        std::thread::Builder::new()
            .name("lut-watch".into())
            .spawn(move || {
                let mut seen = effectcraft_effects::lut_generation();
                // (ends once the watcher, the only sender, is dropped)
                while let Err(RecvTimeoutError::Timeout) = rx.recv_timeout(every) {
                    let now = check();
                    if now != seen {
                        seen = now;
                        wake();
                    }
                }
            })
            .ok()?;
        Some(Watcher { _stop: tx })
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;

    /// A LUT file edited on disk wakes the interface without a render loading it.
    #[test]
    fn an_edited_lut_file_wakes_the_interface() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("watched.cube");
        std::fs::write(&path, "LUT_1D_SIZE 2\n0 0 0\n1 1 1\n").unwrap();
        let src = path.to_string_lossy().into_owned();
        assert!(effectcraft_effects::load_lut(&src).is_some());
        let before = effectcraft_effects::lut_generation();
        let (tx, rx) = std::sync::mpsc::channel();
        let _watcher = Watcher::start_with(
            move || {
                let _ = tx.send(());
            },
            Duration::from_millis(10),
        )
        .unwrap();
        std::fs::write(&path, "LUT_1D_SIZE 3\n0 0 0\n0.5 0.5 0.5\n1 1 1\n").unwrap();
        rx.recv_timeout(Duration::from_secs(10)).unwrap();
        assert!(effectcraft_effects::lut_generation() > before);
    }
}
