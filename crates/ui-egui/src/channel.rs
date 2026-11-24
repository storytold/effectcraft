//! The launch channel: stable EffectCraft, or the isolated Nightly flavor the ArtCraft Launcher
//! starts from its own profile. Selected once at startup, before any window or saved state.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU8, Ordering};

/// Matches the `.desktop` file and hicolor icon name, so Wayland docks pick up the icon.
pub const APP_ID: &str = "ai.storyteller.effectcraft";

/// The Nightly window's own app id: a separate taskbar entry and Wayland class from stable. It is
/// not the package/bundle identifier (`ai.storyteller.effectcraft.nightly`), which names the
/// archive and the macOS bundle.
pub const NIGHTLY_APP_ID: &str = "ai.storyteller.effectcraft.nightly-v1";

/// Immutable startup identity. The desktop host selects it before any UI or service is created.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LaunchIdentity {
    Stable,
    NightlyV1,
}

static IDENTITY: AtomicU8 = AtomicU8::new(0);

pub fn select_identity(identity: LaunchIdentity) -> Result<(), String> {
    let value = match identity {
        LaunchIdentity::Stable => 1,
        LaunchIdentity::NightlyV1 => 2,
    };
    IDENTITY.compare_exchange(0, value, Ordering::AcqRel, Ordering::Acquire).map(|_| ()).map_err(|_| "the window launch identity was already selected".into())
}

pub fn identity() -> LaunchIdentity {
    if IDENTITY.load(Ordering::Acquire) == 2 { LaunchIdentity::NightlyV1 } else { LaunchIdentity::Stable }
}

pub const fn id_for(identity: LaunchIdentity) -> &'static str {
    match identity {
        LaunchIdentity::Stable => APP_ID,
        LaunchIdentity::NightlyV1 => NIGHTLY_APP_ID,
    }
}

pub fn app_id() -> &'static str {
    id_for(identity())
}

/// The product name this launch shows: the window title, About and the version line.
pub const fn name_for(identity: LaunchIdentity) -> &'static str {
    match identity {
        LaunchIdentity::Stable => "EffectCraft",
        LaunchIdentity::NightlyV1 => "EffectCraft Nightly",
    }
}

/// Whether features that belong to a stable installation (the updater, the control channel, agent
/// automation) may run. A Nightly session is managed by ArtCraft Launcher.
pub const fn stable_services_for(identity: LaunchIdentity) -> bool {
    matches!(identity, LaunchIdentity::Stable)
}

pub fn stable_services_allowed() -> bool {
    stable_services_for(identity())
}

/// The Nightly profile's own user-preset folder, installed once by the desktop host before the UI
/// exists. A stable session installs nothing, so its `EC_PRESETS_DIR`/Documents rule is unchanged.
static PROFILE_PRESETS: OnceLock<PathBuf> = OnceLock::new();

pub fn set_profile_presets(dir: PathBuf) {
    let _ = PROFILE_PRESETS.set(dir);
}

pub fn profile_presets() -> Option<&'static Path> {
    PROFILE_PRESETS.get().map(PathBuf::as_path)
}

/// A builder for the window, carrying the channel's app id.
pub fn builder() -> egui::ViewportBuilder {
    egui::ViewportBuilder::default().with_app_id(app_id())
}

/// The exact build identity, set once by the desktop host from its build script: for Nightly the
/// prepared feed version (`X.Y.Z-nightly.YYYYMMDD.run.attempt.gsha`) and the source commit.
struct BuildIdentity {
    version: &'static str,
    commit: Option<&'static str>,
}

static BUILD: OnceLock<BuildIdentity> = OnceLock::new();

pub fn set_build_identity(version: &'static str, commit: Option<&'static str>) {
    let _ = BUILD.set(BuildIdentity { version, commit });
}

/// The version this binary was built as. Before the host sets it, the Cargo version.
pub fn version() -> &'static str {
    BUILD.get().map_or(env!("CARGO_PKG_VERSION"), |build| build.version)
}

/// The source commit baked into a release or Nightly build, when the packager provided one.
pub fn commit() -> Option<&'static str> {
    BUILD.get().and_then(|build| build.commit)
}

/// The channel label a Nightly build appends to its version line.
pub fn channel_label() -> Option<&'static str> {
    match identity() {
        LaunchIdentity::Stable => None,
        LaunchIdentity::NightlyV1 => Some("nightly-v1"),
    }
}

/// The extra About line a Nightly build shows: the channel and the baked source commit, and where
/// a problem is reported. Stable shows nothing extra.
pub fn nightly_about_line() -> Option<String> {
    channel_label()?;
    let commit = commit().map(|sha| format!("  •  commit {}", &sha[..sha.len().min(12)])).unwrap_or_default();
    Some(format!("Nightly channel, managed by ArtCraft Launcher{commit}  •  Report a problem: {}", effectcraft_engine::links::ISSUES))
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_nightly_identity_is_distinct_and_never_enables_stable_services() {
        assert_eq!(super::id_for(super::LaunchIdentity::Stable), super::APP_ID);
        assert_ne!(super::id_for(super::LaunchIdentity::NightlyV1), super::APP_ID);
        assert!(super::stable_services_for(super::LaunchIdentity::Stable));
        assert!(!super::stable_services_for(super::LaunchIdentity::NightlyV1));
        assert_eq!(super::name_for(super::LaunchIdentity::Stable), "EffectCraft");
        assert_eq!(super::name_for(super::LaunchIdentity::NightlyV1), "EffectCraft Nightly");
    }

    #[test]
    fn the_window_carries_the_channel_app_id() {
        // The process-global identity is unset here, so this is the stable id.
        assert_eq!(super::builder().app_id.as_deref(), Some(super::APP_ID));
    }

    #[test]
    fn a_stable_session_has_no_channel_and_no_about_extra() {
        assert!(super::channel_label().is_none());
        assert!(super::nightly_about_line().is_none());
        assert_eq!(super::version(), env!("CARGO_PKG_VERSION"));
        assert_eq!(super::commit(), None, "an unset build identity has no invented commit");
    }
}
