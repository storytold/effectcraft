//! Settings ▸ Startup & Repair ▸ Window Graphics, and the launch guard behind it.
//!
//! Some graphics drivers crash EffectCraft as its window's device is created (an access
//! violation in an older Intel UHD driver with DirectX 12 / Vulkan, #243), or a moment after
//! its first frame, as the window is shown and its surface re-created (AMD's Vulkan driver for
//! a Radeon RX 6800M, Adrenalin 26.9.2: an access violation in amdvlk64.dll on every launch).
//! A crash in a driver can't be caught, so each launch leaves a marker in the settings folder
//! until its window has kept drawing for [`SETTLE`] (or the app quits normally before then). A
//! launch that finds the previous one's marker switches Window Graphics to OpenGL, which those
//! drivers run, and says so once the window is up. `WGPU_BACKEND` (wgpu's own override) still
//! wins over both. A separate guard file is locked while its launch runs, so a second copy opened
//! meanwhile isn't taken for a crash; and a window OpenGL can't open either puts Window Graphics
//! back to Automatic ([`opengl_failed`]) rather than failing every launch.

use std::fs::File;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use effectcraft_engine::prefs::{PREFS_FILE, Prefs};
use eframe::wgpu::Backends;

/// Left in the settings folder from a launch until its window has settled.
const MARKER: &str = "launch-pending";
const LOCK: &str = "launch.lock";

/// Keep this file in place: unlinking a locked file lets a second inode acquire a new lock.
fn acquire_guard(dir: &Path) -> Option<File> {
    let file = match std::fs::create_dir_all(dir)
        .and_then(|()| std::fs::OpenOptions::new().read(true).write(true).create(true).truncate(false).open(dir.join(LOCK)))
    {
        Ok(file) => file,
        Err(e) => {
            log::warn!("launch guard unavailable: {e}");
            return None;
        }
    };
    match file.try_lock() {
        Ok(()) => {}
        Err(std::fs::TryLockError::WouldBlock) => return None,
        Err(e) => {
            log::warn!("launch guard lock unavailable: {e}");
            return None;
        }
    }
    // Older versions lock the marker itself. Do not disturb an older live launch.
    if let Ok(marker) = File::open(dir.join(MARKER)) {
        match marker.try_lock() {
            Ok(()) => {}
            Err(std::fs::TryLockError::WouldBlock) => return None,
            Err(e) => {
                log::warn!("launch marker lock unavailable: {e}");
                return None;
            }
        }
    }
    Some(file)
}

/// How long the window keeps drawing after its first frame before the launch counts as made.
/// A driver that fails as the window is shown and its surface re-created does so within the
/// first second; a user quitting sooner than this is told the next launch uses OpenGL and can
/// switch back in Settings.
pub const SETTLE: Duration = Duration::from_secs(3);

/// This launch's window graphics.
#[derive(Debug, Default)]
pub struct Launch {
    /// The graphics APIs the window may use (`None`: eframe's default).
    pub backends: Option<Backends>,
    /// Switched to OpenGL because the previous launch stopped as its window opened: the message
    /// to show.
    pub notice: Option<String>,
    marker: Option<PathBuf>,
    /// Holds the separate guard file's lock until the launch settles (or the process ends).
    lock: Option<File>,
    /// When the window drew its first frame, until the launch settles.
    first_frame: Option<Instant>,
}

impl Launch {
    /// Read Window Graphics in the settings folder `dir`, switch it to OpenGL when the previous
    /// launch stopped as its window opened (where OpenGL exists: not macOS), and leave this
    /// launch's marker. `env_override`: `WGPU_BACKEND` is set.
    pub fn begin(dir: Option<&Path>, env_override: bool) -> Launch {
        let Some(dir) = dir.filter(|_| !env_override) else { return Launch::default() };
        let prefs_path = dir.join(PREFS_FILE);
        let mut prefs = std::fs::read_to_string(&prefs_path).map(|t| Prefs::from_json(&t)).unwrap_or_default();
        let marker = dir.join(MARKER);
        let Some(lock) = acquire_guard(dir) else {
            return Launch { backends: (prefs.startup.window_graphics == "gl").then_some(Backends::GL), ..Launch::default() };
        };
        // Read again while owning the guard: another launch may have just changed preferences.
        prefs = std::fs::read_to_string(&prefs_path).map(|t| Prefs::from_json(&t)).unwrap_or_default();
        let mut notice = None;
        if marker.exists() && prefs.startup.window_graphics == "auto" && cfg!(not(target_os = "macos")) {
            prefs.startup.window_graphics = "gl".into();
            match std::fs::create_dir_all(dir).and_then(|()| std::fs::write(&prefs_path, prefs.to_json())) {
                Ok(()) => log::warn!("the last launch stopped as its window opened: switching Window Graphics to OpenGL"),
                Err(e) => log::warn!("the last launch stopped as its window opened; saving Window Graphics failed: {e}"),
            }
            notice = Some(
                "EffectCraft's last launch stopped as its window opened, so it now draws with OpenGL \
                 (Settings ▸ Startup & Repair ▸ Window Graphics). Updating the graphics driver may let it use \
                 Automatic again."
                    .to_string(),
            );
        }
        let backends = (prefs.startup.window_graphics == "gl").then_some(Backends::GL);
        let marker = match File::create(&marker) {
            Ok(_) => Some(marker),
            Err(e) => {
                log::warn!("launch marker: {e}");
                None
            }
        };
        Launch { backends, notice, marker, lock: Some(lock), first_frame: None }
    }

    /// The window drew a frame. Until the launch has settled, returns how long that still takes
    /// (ask for a repaint then, so that an idle window settles too).
    pub fn frame_drawn(&mut self) -> Option<Duration> {
        self.frame_drawn_at(Instant::now())
    }

    fn frame_drawn_at(&mut self, now: Instant) -> Option<Duration> {
        self.marker.as_ref()?;
        let first = *self.first_frame.get_or_insert(now);
        let left = SETTLE.saturating_sub(now.saturating_duration_since(first));
        if left.is_zero() {
            self.settled();
            return None;
        }
        Some(left)
    }

    /// The app quits normally: however soon after opening, that's no crash.
    pub fn exited(&mut self) {
        self.settled();
    }

    /// The launch made it: release and remove its marker.
    fn settled(&mut self) {
        if let Some(m) = self.marker.take()
            && let Err(e) = std::fs::remove_file(&m)
        {
            log::warn!("launch marker: {e}");
        }
        self.lock = None;
    }
}

/// The window couldn't open with OpenGL (an error, not a crash): back to Automatic, and no marker.
pub fn opengl_failed(dir: Option<&Path>) {
    let Some(dir) = dir else { return };
    let Some(_lock) = acquire_guard(dir) else { return };
    let prefs_path = dir.join(PREFS_FILE);
    let mut prefs = std::fs::read_to_string(&prefs_path).map(|t| Prefs::from_json(&t)).unwrap_or_default();
    prefs.startup.window_graphics = "auto".into();
    if let Err(e) = std::fs::write(&prefs_path, prefs.to_json()) {
        log::warn!("saving Window Graphics failed: {e}");
    }
    let _ = std::fs::remove_file(dir.join(MARKER));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window_graphics(dir: &Path) -> String {
        Prefs::from_json(&std::fs::read_to_string(dir.join(PREFS_FILE)).unwrap_or_default()).startup.window_graphics
    }

    /// A launch that never drew switches the next one to OpenGL, which then stays (#243).
    #[test]
    fn a_launch_that_never_drew_switches_the_next_to_opengl() {
        let dir = std::env::temp_dir().join(format!("ec-launch-guard-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        // A first launch that draws: nothing changes. Its first frames don't settle it (a driver
        // may still crash as the window is shown); frames for `SETTLE` do.
        let mut l = Launch::begin(Some(&dir), false);
        assert_eq!((l.backends, l.notice.is_some()), (None, false));
        assert!(dir.join(MARKER).exists());
        let t0 = Instant::now();
        assert_eq!(l.frame_drawn_at(t0), Some(SETTLE));
        assert!(l.frame_drawn_at(t0 + SETTLE / 2).is_some_and(|left| left <= SETTLE / 2));
        assert!(dir.join(MARKER).exists());
        assert_eq!(l.frame_drawn_at(t0 + SETTLE), None);
        assert!(!dir.join(MARKER).exists());
        assert_eq!(l.frame_drawn_at(t0 + SETTLE), None, "settled stays settled");
        // A launch that crashes before drawing (it never calls `drawn`)…
        drop(Launch::begin(Some(&dir), false));
        // …makes the next one use OpenGL and say so, once.
        let mut l = Launch::begin(Some(&dir), false);
        if cfg!(target_os = "macos") {
            assert_eq!((l.backends, l.notice.is_some()), (None, false), "no OpenGL on macOS");
        } else {
            assert_eq!(l.backends, Some(Backends::GL));
            assert!(l.notice.is_some());
            assert_eq!(window_graphics(&dir), "gl", "saved in Settings");
            l.exited();
            let l = Launch::begin(Some(&dir), false);
            assert_eq!((l.backends, l.notice.is_some()), (Some(Backends::GL), false), "it stays, without the notice");
        }
        // `WGPU_BACKEND` wins, and leaves no marker.
        let _ = std::fs::remove_file(dir.join(MARKER));
        assert_eq!(Launch::begin(Some(&dir), true).backends, None);
        assert!(!dir.join(MARKER).exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A launch that stops moments after its first frame (AMD's Vulkan driver crashing as the
    /// window is shown and its surface re-created) switches the next one to OpenGL too; a
    /// normal quit in that time doesn't.
    #[test]
    fn a_launch_that_stops_after_its_first_frame_switches_the_next_to_opengl() {
        let dir = std::env::temp_dir().join(format!("ec-launch-guard-3-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut l = Launch::begin(Some(&dir), false);
        assert!(l.frame_drawn().is_some());
        drop(l);
        let l = Launch::begin(Some(&dir), false);
        if cfg!(target_os = "macos") {
            assert_eq!(l.backends, None, "no OpenGL on macOS");
        } else {
            assert_eq!(l.backends, Some(Backends::GL));
            assert!(l.notice.is_some());
        }
        drop(l);
        let _ = std::fs::remove_dir_all(&dir);
        // Quitting right after the window opened leaves no marker.
        let mut l = Launch::begin(Some(&dir), false);
        assert!(l.frame_drawn().is_some());
        l.exited();
        assert!(!dir.join(MARKER).exists());
        assert_eq!(Launch::begin(Some(&dir), false).backends, None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A second copy opened while the first is still opening isn't a crash; OpenGL that can't
    /// open a window either goes back to Automatic instead of failing every launch.
    #[test]
    fn a_second_copy_or_failed_opengl_doesnt_strand_window_graphics() {
        let dir = std::env::temp_dir().join(format!("ec-launch-guard-2-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let _first = Launch::begin(Some(&dir), false);
        assert_eq!(Launch::begin(Some(&dir), false).backends, None);
        assert_eq!(window_graphics(&dir), "auto");
        drop(_first);
        if cfg!(not(target_os = "macos")) {
            assert_eq!(Launch::begin(Some(&dir), false).backends, Some(Backends::GL), "a real crash still switches");
            opengl_failed(Some(&dir));
            assert_eq!(window_graphics(&dir), "auto");
            assert_eq!(Launch::begin(Some(&dir), false).backends, None);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A fresh settings folder for one test.
    fn fresh_dir(case: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ec-launch-guard-{case}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Window Graphics saved as OpenGL.
    fn save_opengl(dir: &Path) {
        let mut prefs = Prefs::default();
        prefs.startup.window_graphics = "gl".into();
        std::fs::write(dir.join(PREFS_FILE), prefs.to_json()).unwrap();
    }

    /// A second copy that quits normally while the first is still opening doesn't remove the first
    /// one's marker (#477).
    #[test]
    fn a_second_copy_quitting_leaves_the_running_launchs_marker() {
        let dir = fresh_dir("second-quits");
        let mut first = Launch::begin(Some(&dir), false);
        let mut second = Launch::begin(Some(&dir), false);
        second.exited();
        let kept = dir.join(MARKER).exists();
        first.exited();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(kept, "the second copy removed the running launch's marker");
    }

    /// A second copy whose OpenGL window fails doesn't remove a running launch's marker.
    #[test]
    fn failed_opengl_in_a_second_copy_leaves_the_running_launchs_marker() {
        let dir = fresh_dir("second-gl-fails");
        let mut first = Launch::begin(Some(&dir), false);
        let second = Launch::begin(Some(&dir), false);
        opengl_failed(Some(&dir));
        let kept = dir.join(MARKER).exists();
        drop(second);
        first.exited();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(kept, "OpenGL cleanup removed another running launch's marker");
    }

    /// With `WGPU_BACKEND` set, a stale marker and a saved OpenGL choice are left as they are.
    #[test]
    fn wgpu_backend_leaves_a_stale_marker_and_saved_opengl_alone() {
        let dir = fresh_dir("env-override");
        save_opengl(&dir);
        std::fs::write(dir.join(MARKER), b"stale-marker").unwrap();
        let before = std::fs::read(dir.join(PREFS_FILE)).unwrap();
        let mut l = Launch::begin(Some(&dir), true);
        assert_eq!(l.backends, None);
        assert!(l.notice.is_none());
        l.exited();
        assert!(dir.join(MARKER).exists());
        assert_eq!(std::fs::read(dir.join(PREFS_FILE)).unwrap(), before);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// After OpenGL fails to open a window, the next launch is back on Automatic, without a notice.
    #[test]
    fn failed_opengl_puts_the_next_launch_back_on_automatic() {
        let dir = fresh_dir("gl-recovers");
        save_opengl(&dir);
        let l = Launch::begin(Some(&dir), false);
        assert_eq!(l.backends, Some(Backends::GL));
        drop(l);
        opengl_failed(Some(&dir));
        assert!(!dir.join(MARKER).exists());
        assert_eq!(Prefs::from_json(&std::fs::read_to_string(dir.join(PREFS_FILE)).unwrap()).startup.window_graphics, "auto");
        let mut l = Launch::begin(Some(&dir), false);
        assert_eq!(l.backends, None);
        assert!(l.notice.is_none());
        l.exited();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The guard file is never removed or replaced: unlinking a locked file would let the next
    /// launch lock a new one while the old lock is still held.
    #[cfg(unix)]
    #[test]
    fn the_guard_file_stays_the_same_file_across_launches() {
        use std::os::unix::fs::MetadataExt;
        let dir = fresh_dir("guard-identity");
        let mut a = Launch::begin(Some(&dir), false);
        let before = std::fs::metadata(dir.join(LOCK)).unwrap();
        let t0 = Instant::now();
        assert_eq!(a.frame_drawn_at(t0), Some(SETTLE));
        assert_eq!(a.frame_drawn_at(t0 + SETTLE), None);
        assert!(!dir.join(MARKER).exists());
        let after = std::fs::metadata(dir.join(LOCK)).unwrap();
        assert_eq!((before.dev(), before.ino()), (after.dev(), after.ino()));
        let mut b = Launch::begin(Some(&dir), false);
        assert!(b.lock.is_some());
        let reused = std::fs::metadata(dir.join(LOCK)).unwrap();
        assert_eq!((before.dev(), before.ino()), (reused.dev(), reused.ino()));
        b.exited();
        let _ = std::fs::remove_dir_all(dir);
    }

    /// A second copy's frames and quit neither settle nor unlock the running launch.
    #[test]
    fn a_second_copys_frames_dont_settle_the_running_launch() {
        let dir = fresh_dir("second-frames");
        let mut a = Launch::begin(Some(&dir), false);
        std::fs::write(dir.join(MARKER), b"owner-marker").unwrap();
        let mut b = Launch::begin(Some(&dir), false);
        assert!(b.lock.is_none());
        assert!(b.marker.is_none());
        let t0 = Instant::now();
        assert_eq!(b.frame_drawn_at(t0), None);
        assert_eq!(b.frame_drawn_at(t0 + SETTLE), None);
        b.exited();
        assert_eq!(std::fs::read(dir.join(MARKER)).unwrap(), b"owner-marker");
        let probe = File::open(dir.join(LOCK)).unwrap();
        assert!(matches!(probe.try_lock(), Err(std::fs::TryLockError::WouldBlock)));
        a.exited();
        let _ = std::fs::remove_dir_all(dir);
    }

    /// Older versions lock the marker itself: while one runs, a new launch changes nothing, and
    /// once it has stopped without settling the next launch still switches to OpenGL.
    #[test]
    fn a_running_older_version_holding_the_marker_is_left_alone() {
        let dir = fresh_dir("older-version");
        std::fs::write(dir.join(MARKER), b"old-owner").unwrap();
        std::fs::write(dir.join(PREFS_FILE), Prefs::default().to_json()).unwrap();
        let old = File::open(dir.join(MARKER)).unwrap();
        old.try_lock().unwrap();
        let before = std::fs::read(dir.join(PREFS_FILE)).unwrap();
        let mut b = Launch::begin(Some(&dir), false);
        assert!(b.lock.is_none());
        assert!(b.marker.is_none());
        assert!(b.backends.is_none());
        assert!(b.notice.is_none());
        b.exited();
        opengl_failed(Some(&dir));
        assert_eq!(std::fs::read(dir.join(MARKER)).unwrap(), b"old-owner");
        assert_eq!(std::fs::read(dir.join(PREFS_FILE)).unwrap(), before);
        drop(old);
        let mut recovered = Launch::begin(Some(&dir), false);
        if cfg!(not(target_os = "macos")) {
            assert_eq!(recovered.backends, Some(Backends::GL));
        }
        recovered.exited();
        let _ = std::fs::remove_dir_all(dir);
    }

    /// A guard file that can't be opened leaves Window Graphics and the marker as they are.
    #[test]
    fn an_unusable_guard_file_leaves_settings_and_marker_alone() {
        let dir = fresh_dir("guard-unusable");
        std::fs::create_dir(dir.join(LOCK)).unwrap();
        std::fs::write(dir.join(MARKER), b"stale-marker").unwrap();
        std::fs::write(dir.join(PREFS_FILE), Prefs::default().to_json()).unwrap();
        let before = std::fs::read(dir.join(PREFS_FILE)).unwrap();
        let mut l = Launch::begin(Some(&dir), false);
        assert!(l.backends.is_none());
        assert!(l.notice.is_none());
        assert!(l.lock.is_none());
        assert!(l.marker.is_none());
        l.exited();
        opengl_failed(Some(&dir));
        assert_eq!(std::fs::read(dir.join(PREFS_FILE)).unwrap(), before);
        assert_eq!(std::fs::read(dir.join(MARKER)).unwrap(), b"stale-marker");
        let _ = std::fs::remove_dir_all(dir);
    }

    /// OpenGL cleanup from elsewhere waits until the running launch has let go; then it resets
    /// Window Graphics to Automatic and removes the marker.
    #[test]
    fn failed_opengl_cleanup_waits_for_the_running_launch() {
        let dir = fresh_dir("gl-cleanup-waits");
        save_opengl(&dir);
        let launch = Launch::begin(Some(&dir), false);
        assert_eq!(launch.backends, Some(Backends::GL));
        assert!(launch.lock.is_some());
        let before = std::fs::read(dir.join(PREFS_FILE)).unwrap();
        opengl_failed(Some(&dir));
        assert_eq!(std::fs::read(dir.join(PREFS_FILE)).unwrap(), before);
        assert!(dir.join(MARKER).exists(), "cleanup elsewhere can't remove a running launch's marker");
        drop(launch);
        opengl_failed(Some(&dir));
        let after = Prefs::from_json(&std::fs::read_to_string(dir.join(PREFS_FILE)).unwrap()).startup.window_graphics;
        assert_eq!(after, "auto", "once the launch has let go, failed OpenGL resets Window Graphics");
        assert!(!dir.join(MARKER).exists());
        let _ = std::fs::remove_dir_all(dir);
    }
}
