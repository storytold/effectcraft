//! Project file that changes on disk while the app is open: reload it into the live session
//! instead of asking for a restart.
//!
//! `Session` keeps the open project in an `Arc<Project>` in memory; only `file.open` / `file.revert`
//! read the file. The desktop app has no watcher on the project itself (the footage and LUT
//! watchers watch their own files), so an edit made by `effectcraft-cli` or an agent was invisible
//! until the window was reopened. [`Watcher`] stamps the project path on a worker thread and
//! wakes the interface once a change has stayed stable long enough to be a complete write.
//! It never reads or parses the project; the interface calls `file.revert`, which reuses the
//! ordinary open path and `replace_project`.

use crate::Services;
use std::sync::mpsc::{RecvTimeoutError, SyncSender, sync_channel};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

/// A project file's size and modification time, as reported by [`Services::stamp`].
pub type Stamp = (u64, u128);

/// How often the project file is stamped while the window is in front.
pub const FOCUSED_INTERVAL: Duration = Duration::from_secs(1);
/// How often while another app has the front.
pub const UNFOCUSED_INTERVAL: Duration = Duration::from_secs(5);
/// A write is atomic (temp file + rename) and may happen in more than one step: require the new
/// stamp to survive this long before reporting it.
pub const DEBOUNCE: Duration = Duration::from_millis(250);

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

#[derive(Default)]
struct State {
    path: Option<String>,
    stamp: Option<Stamp>,
    unverified: bool,
    focused: bool,
}

/// The worker that stamps one project file. Dropping the watcher ends the thread.
pub struct Watcher {
    _stop: SyncSender<()>,
    state: Arc<Mutex<State>>,
    result: Arc<Mutex<Option<(String, Stamp)>>>,
}

impl Watcher {
    /// Start the worker. `wake` runs on the worker when a change was seen. `None` where there are
    /// no threads (the web build) or a thread could not be started.
    pub fn start(services: Arc<dyn Services>, wake: impl Fn() + Send + 'static) -> Option<Watcher> {
        if cfg!(target_arch = "wasm32") {
            return None;
        }
        let (tx, rx) = sync_channel::<()>(1);
        let state = Arc::new(Mutex::new(State { focused: true, ..Default::default() }));
        let result = Arc::new(Mutex::new(None));
        let st = Arc::clone(&state);
        let res = Arc::clone(&result);
        std::thread::Builder::new()
            .name("project-watch".into())
            .spawn(move || {
                let mut interval = FOCUSED_INTERVAL;
                loop {
                    match rx.recv_timeout(interval) {
                        Ok(()) | Err(RecvTimeoutError::Timeout) => {}
                        Err(RecvTimeoutError::Disconnected) => break,
                    }
                    let (path, focused) = {
                        let s = lock(&st);
                        (s.path.clone(), s.focused)
                    };
                    interval = if focused { FOCUSED_INTERVAL } else { UNFOCUSED_INTERVAL };
                    let Some(path) = path else { continue };
                    let now = services.stamp(&path);
                    {
                        let mut s = lock(&st);
                        if s.path.as_ref() != Some(&path) {
                            continue;
                        }
                        match (s.stamp, now) {
                            // The first sighting is normally only a baseline. A save whose
                            // post-write check failed is different: the file on disk is not known
                            // to be ours, so the first stamp must reach the pending-change path.
                            (None, Some(stamp)) => {
                                s.stamp = Some(stamp);
                                if s.unverified {
                                    s.unverified = false;
                                    drop(s);
                                    *lock(&res) = Some((path.clone(), stamp));
                                    wake();
                                }
                            }
                            (Some(old), Some(new)) if old != new => {
                                drop(s);
                                std::thread::sleep(DEBOUNCE);
                                if services.stamp(&path) == Some(new) {
                                    // Publish the stamp together with the report. Keep the old
                                    // baseline while debouncing: a failed check or a stamp that
                                    // changed back must not swallow the original change.
                                    let mut s = lock(&st);
                                    if s.path.as_ref() == Some(&path) {
                                        s.stamp = Some(new);
                                    }
                                    drop(s);
                                    *lock(&res) = Some((path.clone(), new));
                                    wake();
                                }
                            }
                            // A missing or unreadable file leaves the baseline alone.
                            _ => {}
                        }
                    }
                }
            })
            .ok()?;
        Some(Watcher { _stop: tx, state, result })
    }

    /// Watch this project path with the stamp of the bytes the session loaded. Installing both
    /// under one lock closes the gap between opening a file and the worker's first observation.
    /// A new path starts a new baseline; an unchanged path keeps its baseline so external changes
    /// stay detectable.
    pub fn set_watched(&mut self, path: Option<String>, stamp: Option<Stamp>, unverified: bool) {
        let mut s = lock(&self.state);
        if s.path == path {
            return;
        }
        s.path = path;
        s.stamp = stamp;
        s.unverified = unverified;
        drop(s);
        let _ = self._stop.try_send(());
    }

    /// Whether the window is in front; coming back to the front stamps at once.
    pub fn set_focused(&mut self, focused: bool) {
        let mut s = lock(&self.state);
        if s.focused == focused {
            return;
        }
        s.focused = focused;
        drop(s);
        let _ = self._stop.try_send(());
    }

    /// The baseline the worker is comparing against (tests and diagnostics).
    pub fn baseline(&self) -> Option<Stamp> {
        lock(&self.state).stamp
    }

    /// The change found by the worker, once. The interface decides whether to reload it.
    pub fn take(&self) -> Option<(String, Stamp)> {
        lock(&self.result).take()
    }

    /// Record the stamp the session itself just wrote (open/save/reload) so it is not reported
    /// back as an external change.
    pub fn acknowledge(&self, stamp: Option<Stamp>) {
        let mut s = lock(&self.state);
        s.stamp = stamp;
        s.unverified = false;
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Disk {
        stamps: Mutex<std::collections::HashMap<String, Stamp>>,
        looks: AtomicUsize,
    }

    impl Services for Disk {
        fn read_file(&self, _: &str) -> std::io::Result<Vec<u8>> {
            Err(std::io::Error::new(std::io::ErrorKind::NotFound, "read-only test disk"))
        }
        fn write_file(&self, _: &str, _: &[u8]) -> std::io::Result<()> {
            Err(std::io::Error::other("read-only test disk"))
        }
        fn is_dir(&self, _: &str) -> bool {
            false
        }
        fn stamp(&self, path: &str) -> Option<Stamp> {
            self.looks.fetch_add(1, Ordering::SeqCst);
            lock(&self.stamps).get(path).copied()
        }
    }

    fn disk(stamp: Stamp) -> (Arc<Disk>, String) {
        let d = Arc::new(Disk { stamps: Mutex::new(Default::default()), looks: AtomicUsize::new(0) });
        let path = "C:/test/project.ecproj".to_string();
        lock(&d.stamps).insert(path.clone(), stamp);
        (d, path)
    }

    fn start(d: Arc<Disk>, wakes: Arc<AtomicUsize>) -> Watcher {
        Watcher::start(d, move || {
            wakes.fetch_add(1, Ordering::SeqCst);
        })
        .unwrap()
    }

    fn wait_for(what: &str, mut ready: impl FnMut() -> bool) {
        let t0 = std::time::Instant::now();
        while !ready() {
            assert!(t0.elapsed() < Duration::from_secs(10), "timed out: {what}");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn the_first_sighting_only_sets_the_baseline() {
        let (d, path) = disk((100, 1));
        let wakes = Arc::new(AtomicUsize::new(0));
        let mut w = start(d, Arc::clone(&wakes));
        w.set_watched(Some(path), None, false);
        wait_for("first baseline", || w.baseline().is_some());
        std::thread::sleep(Duration::from_millis(200));
        assert!(w.take().is_none(), "first sight must not be reported as an edit");
        assert_eq!(wakes.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn an_unverified_path_reports_its_first_stamp() {
        let (d, path) = disk((100, 1));
        let wakes = Arc::new(AtomicUsize::new(0));
        let mut w = start(d, Arc::clone(&wakes));
        w.set_watched(Some(path.clone()), None, true);
        wait_for("wake", || wakes.load(Ordering::SeqCst) >= 1);
        assert_eq!(w.take(), Some((path, (100, 1))));
    }

    #[test]
    fn an_external_write_wakes_and_reports_the_new_stamp() {
        let (d, path) = disk((100, 1));
        let wakes = Arc::new(AtomicUsize::new(0));
        let mut w = start(d.clone(), Arc::clone(&wakes));
        // The session's loaded stamp is installed synchronously with the path.
        w.set_watched(Some(path.clone()), Some((100, 1)), false);
        assert_eq!(w.baseline(), Some((100, 1)));
        // Simulate an atomic save: a new size and mtime.
        lock(&d.stamps).insert(path.clone(), (120, 2));
        wait_for("wake", || wakes.load(Ordering::SeqCst) >= 1);
        assert_eq!(w.take(), Some((path, (120, 2))));
    }

    #[test]
    fn acknowledging_a_reported_change_does_not_report_it_again() {
        let (d, path) = disk((100, 1));
        let wakes = Arc::new(AtomicUsize::new(0));
        let mut w = start(d.clone(), Arc::clone(&wakes));
        w.set_watched(Some(path.clone()), Some((100, 1)), false);
        assert_eq!(w.baseline(), Some((100, 1)));
        lock(&d.stamps).insert(path.clone(), (120, 2));
        wait_for("wake", || wakes.load(Ordering::SeqCst) >= 1);
        assert_eq!(w.take(), Some((path.clone(), (120, 2))));
        // The UI reloaded (or decided it was its own write) and tells the watcher the stamp it
        // now considers current. The same change must not be reported a second time.
        w.acknowledge(Some((120, 2)));
        std::thread::sleep(Duration::from_millis(300));
        assert!(w.take().is_none());
        assert_eq!(wakes.load(Ordering::SeqCst), 1);
    }
}
