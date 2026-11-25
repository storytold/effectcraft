//! App-owned nightly profile fences; no stable fallback, migration or process environment changes.

use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};

use effectcraft_host::app_dirs::{PORTABLE_DATA_DIR, portable_marker};

/// The bundle id of EffectCraft's own Nightly Flatpak (`packaging/linux/flatpak-bundle.sh`). Its
/// sandbox is a separate installation; every other Flatpak instance (the stable app's included)
/// would point the profile at another install's state.
pub const NIGHTLY_FLATPAK_ID: &str = "ai.storyteller.effectcraft.nightly";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Platform {
    Windows,
    Linux,
    Mac,
}

impl Platform {
    fn current() -> Result<Self, String> {
        match std::env::consts::OS {
            "windows" => Ok(Self::Windows),
            "linux" => Ok(Self::Linux),
            "macos" => Ok(Self::Mac),
            _ => Err("nightly-v1 is not defined for this operating system".into()),
        }
    }

    fn ignores_case(self) -> bool {
        self != Self::Linux
    }
}

fn absolute(path: PathBuf, label: &str) -> Result<PathBuf, String> {
    if !path.is_absolute() || path.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err(format!("{label} must be absolute and contain no parent traversal"));
    }
    Ok(path)
}

fn required_base(env: &impl Fn(&str) -> Option<OsString>, key: &str) -> Result<PathBuf, String> {
    let value = env(key).filter(|v| !v.is_empty()).ok_or_else(|| format!("cannot establish the stable {key} boundary"))?;
    absolute(PathBuf::from(value), key)
}

fn xdg_base(env: &impl Fn(&str) -> Option<OsString>, key: &str, fallback: &str) -> Result<PathBuf, String> {
    match env(key).filter(|v| !v.is_empty()) {
        Some(value) => absolute(PathBuf::from(value), key),
        None => Ok(required_base(env, "HOME")?.join(fallback)),
    }
}

/// Resolve existing ancestors before creating anything. A dangling link or inaccessible ancestor
/// is not a missing directory that may be replaced or followed later.
fn canonical_future(path: &Path) -> Result<PathBuf, String> {
    let mut parent = path;
    let mut missing = Vec::new();
    loop {
        match std::fs::symlink_metadata(parent) {
            Ok(_) => break,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                missing.push(parent.file_name().ok_or("the profile root has no existing ancestor")?.to_os_string());
                parent = parent.parent().ok_or("the profile root has no existing ancestor")?;
            }
            Err(error) => return Err(format!("cannot inspect the profile boundary: {error}")),
        }
    }
    let mut resolved = std::fs::canonicalize(parent).map_err(|e| format!("cannot resolve the profile boundary: {e}"))?;
    for part in missing.into_iter().rev() {
        resolved.push(part);
    }
    Ok(resolved)
}

fn overlaps(a: &Path, b: &Path, platform: Platform) -> bool {
    let parts = |p: &Path| {
        p.components()
            .map(|c| {
                let value = c.as_os_str().to_string_lossy().into_owned();
                if platform.ignores_case() { value.to_lowercase() } else { value }
            })
            .collect::<Vec<_>>()
    };
    let (a, b) = (parts(a), parts(b));
    a.starts_with(&b) || b.starts_with(&a)
}

fn installation_boundary(platform: Platform, exe: &Path) -> Result<PathBuf, String> {
    let parent = exe.parent().ok_or("cannot establish the installation boundary")?;
    if platform == Platform::Mac {
        // A profile in Resources is still inside the versioned install, not beside MacOS.
        if let Some(bundle) = exe.ancestors().find(|p| p.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("app"))) {
            return Ok(bundle.to_path_buf());
        }
    }
    Ok(parent.to_path_buf())
}

fn forbidden_roots(platform: Platform, env: &impl Fn(&str) -> Option<OsString>, exe: &Path) -> Result<Vec<PathBuf>, String> {
    let mut roots = match platform {
        Platform::Windows => {
            // Settings and crash recovery live in the roaming folder; the disk cache, media cache
            // and conformed audio in the local one (crates/render/src/disk_cache.rs).
            let mut roots =
                ["APPDATA", "LOCALAPPDATA"].into_iter().map(|key| required_base(env, key).map(|p| p.join("EffectCraft"))).collect::<Result<Vec<_>, _>>()?;
            // The MSI's default per-user and per-machine folders (install-policy.wxi). A custom
            // install folder is not known here; the executable's own folder is fenced below.
            roots.push(required_base(env, "LOCALAPPDATA")?.join("Programs").join("ArtCraft").join("EffectCraft"));
            for key in ["ProgramW6432", "ProgramFiles", "ProgramFiles(x86)"] {
                if let Some(base) = env(key).filter(|v| !v.is_empty()) {
                    roots.push(absolute(PathBuf::from(base), key)?.join("ArtCraft").join("EffectCraft"));
                }
            }
            roots
        }
        Platform::Linux => {
            let mut roots =
                [("XDG_CONFIG_HOME", ".config"), ("XDG_DATA_HOME", ".local/share"), ("XDG_CACHE_HOME", ".cache"), ("XDG_STATE_HOME", ".local/state")]
                    .into_iter()
                    .map(|(key, fallback)| xdg_base(env, key, fallback).map(|p| p.join("effectcraft")))
                    .collect::<Result<Vec<_>, _>>()?;
            roots.extend(["/usr/bin/effectcraft", "/usr/local/bin/effectcraft", "/opt/effectcraft"].map(PathBuf::from));
            // The stable Flatpak keeps its data in its own per-app folder.
            roots.push(required_base(env, "HOME")?.join(".var/app/ai.storyteller.effectcraft"));
            roots
        }
        Platform::Mac => {
            let home = required_base(env, "HOME")?;
            let library = home.join("Library");
            vec![
                library.join("Application Support/EffectCraft"),
                library.join("Caches/EffectCraft"),
                library.join("Caches/ai.storyteller.effectcraft"),
                library.join("Preferences/EffectCraft"),
                library.join("Preferences/ai.storyteller.effectcraft.plist"),
                library.join("Saved Application State/ai.storyteller.effectcraft.savedState"),
                home.join("Applications/EffectCraft.app"),
                PathBuf::from("/Applications/EffectCraft.app"),
            ]
        }
    };
    // The stable app's default presets folder (`presets_dir`: HOME, else USERPROFILE). A Nightly root
    // that contains it would share preset files with the stable app.
    if let Some(home) = env("HOME").filter(|v| !v.is_empty()).or_else(|| env("USERPROFILE").filter(|v| !v.is_empty())) {
        roots.push(absolute(PathBuf::from(home), "HOME")?.join("Documents").join("EffectCraft").join("Presets"));
    }
    roots.push(installation_boundary(platform, exe)?);
    roots.into_iter().map(|p| canonical_future(&p)).collect()
}

/// Every existing entry directly in the profile (settings, Plug-ins, Logs and so on) must resolve
/// inside it, so a link there cannot point Nightly's writes at another app's data.
fn entries_stay_inside(root: &Path) -> Result<(), String> {
    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(format!("cannot inspect the nightly profile: {error}")),
    };
    for entry in entries {
        let entry = entry.map_err(|e| format!("cannot inspect the nightly profile: {e}"))?;
        private_subdirectory(root, entry.file_name(), "profile entry")?;
    }
    Ok(())
}

fn private_subdirectory(root: &Path, name: impl AsRef<Path>, label: &str) -> Result<PathBuf, String> {
    let entry = canonical_future(&root.join(name))?;
    if entry == root || !entry.starts_with(root) {
        return Err(format!("the nightly {label} must stay inside its selected profile"));
    }
    Ok(entry)
}

/// Recognise the shared portable data namespace in both the supplied and resolved path.
/// This is not an inventory of arbitrary custom Stable overrides or a Launcher ownership proof.
fn is_portable_layout(path: &Path, platform: Platform) -> bool {
    // A folder under a portable copy belongs to that copy too. Above the chosen folder itself, a
    // marker only counts beside an EffectCraft executable, so an unrelated portable.txt higher up
    // (in a user folder or at a drive root) does not refuse the profile.
    path.ancestors().enumerate().any(|(depth, ancestor)| {
        portable_marker(ancestor).is_some() && (depth == 0 || ["effectcraft.exe", "effectcraft"].iter().any(|exe| ancestor.join(exe).is_file()))
    }) || path.components().any(|component| match component {
        Component::Normal(name) if platform.ignores_case() => name.to_string_lossy().eq_ignore_ascii_case(PORTABLE_DATA_DIR),
        Component::Normal(name) => name == PORTABLE_DATA_DIR,
        _ => false,
    })
}

/// The writable hook is the app's existing probe. Keeping it injected lets filesystem boundary
/// tests exercise the real policy without starting the app or changing global state.
pub fn validate(
    root: &Path,
    env: impl Fn(&str) -> Option<OsString>,
    exe: Option<&Path>,
    writable: impl FnOnce(&Path) -> std::io::Result<()>,
) -> Result<PathBuf, String> {
    validate_for(Platform::current()?, root, env, exe, writable)
}

fn validate_for(
    platform: Platform,
    root: &Path,
    env: impl Fn(&str) -> Option<OsString>,
    exe: Option<&Path>,
    writable: impl FnOnce(&Path) -> std::io::Result<()>,
) -> Result<PathBuf, String> {
    absolute(root.to_path_buf(), "the nightly profile root")?;
    if env("EFFECTCRAFT_CONFIG_DIR").is_some_and(|v| !v.is_empty()) {
        return Err("nightly-v1 cannot be combined with EFFECTCRAFT_CONFIG_DIR".into());
    }
    // The Nightly ships its own Flatpak, so that one sandbox may carry a profile. Any other
    // instance (the stable app's, or a foreign one) names another installation's state.
    if let Some(instance) = env("FLATPAK_ID").filter(|value| !value.is_empty())
        && instance.to_string_lossy() != NIGHTLY_FLATPAK_ID
    {
        return Err(format!("nightly-v1 accepts only the {NIGHTLY_FLATPAK_ID} Flatpak instance; launch the Nightly from its own package"));
    }
    let chosen = canonical_future(root)?;
    if is_portable_layout(root, platform) || is_portable_layout(&chosen, platform) {
        return Err("the nightly root must not use a portable EffectCraft installation or its EffectCraftData settings folder".into());
    }
    let exe = absolute(exe.ok_or("cannot establish the installation boundary")?.to_path_buf(), "the executable")?;
    let forbidden = forbidden_roots(platform, &env, &exe)?;
    if forbidden.iter().any(|stable| overlaps(&chosen, stable, platform)) {
        return Err("the nightly root must not overlap stable data or an installation".into());
    }
    // Existing aliases under the profile must not redirect any of the app's writes elsewhere.
    entries_stay_inside(&chosen)?;
    let mut create = std::fs::DirBuilder::new();
    create.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt as _;
        create.mode(0o700);
    }
    create.create(&chosen).map_err(|e| format!("cannot create the nightly profile: {e}"))?;
    let confirmed = std::fs::canonicalize(&chosen).map_err(|e| format!("cannot resolve the created nightly profile: {e}"))?;
    if confirmed != chosen || forbidden.iter().any(|stable| overlaps(&confirmed, stable, platform)) {
        return Err("the nightly profile changed while it was being validated".into());
    }
    entries_stay_inside(&confirmed)?;
    writable(&confirmed).map_err(|e| format!("the nightly profile is not writable: {e}"))?;
    Ok(confirmed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use effectcraft_host::app_dirs::PORTABLE_MARKERS;
    use std::collections::BTreeMap;
    use std::sync::atomic::{AtomicU64, Ordering};

    struct Fixture {
        root: PathBuf,
        env: BTreeMap<String, OsString>,
        exe: PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let root = loop {
                let candidate =
                    std::env::temp_dir().join(format!("effectcraft-profile-policy-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
                match std::fs::create_dir(&candidate) {
                    Ok(()) => break std::fs::canonicalize(candidate).unwrap(),
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                    Err(error) => panic!("cannot create owned fixture: {error}"),
                }
            };
            let home = root.join("home");
            let exe = root.join("install/effectcraft");
            std::fs::create_dir_all(&home).unwrap();
            std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
            std::fs::write(&exe, b"fixture executable, never run").unwrap();
            let env = [("HOME", home), ("APPDATA", root.join("roaming")), ("LOCALAPPDATA", root.join("local"))]
                .into_iter()
                .map(|(key, value)| (key.into(), value.into_os_string()))
                .collect();
            Self { root, env, exe }
        }

        fn get(&self, key: &str) -> Option<OsString> {
            self.env.get(key).cloned()
        }

        fn check(&self, platform: Platform, root: &Path) -> Result<PathBuf, String> {
            validate_for(
                platform,
                root,
                |key| self.get(key),
                Some(&self.exe),
                |dir| {
                    let probe = dir.join("exclusive-test-probe");
                    let file = std::fs::OpenOptions::new().write(true).create_new(true).open(&probe)?;
                    drop(file);
                    std::fs::remove_file(probe)
                },
            )
        }

        fn home(&self) -> PathBuf {
            PathBuf::from(self.get("HOME").unwrap())
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            // Only the directory successfully created by this fixture is owned for removal.
            std::fs::remove_dir_all(&self.root).unwrap();
        }
    }

    #[test]
    fn portable_settings_from_another_install_are_refused_before_any_probe() {
        let fixture = Fixture::new();
        let stable_install = fixture.root.join("stable portable install");
        let stable_data = stable_install.join(PORTABLE_DATA_DIR);
        std::fs::create_dir_all(&stable_data).unwrap();
        std::fs::write(stable_install.join(PORTABLE_MARKERS[0]), b"owned portable fixture").unwrap();
        std::fs::write(stable_data.join("settings.json"), b"stable settings must not change").unwrap();
        for platform in [Platform::Windows, Platform::Linux, Platform::Mac] {
            for candidate in [&stable_data, &stable_data.join("nested nightly")] {
                let probed = std::cell::Cell::new(false);
                let result = validate_for(
                    platform,
                    candidate,
                    |key| fixture.get(key),
                    Some(&fixture.exe),
                    |_| {
                        probed.set(true);
                        Ok(())
                    },
                );
                assert!(result.is_err(), "a different portable install is not a private Nightly profile");
                assert!(!probed.get(), "reject portable data before a writable probe");
                assert!(!stable_data.join("nested nightly").exists());
                assert_eq!(std::fs::read(stable_data.join("settings.json")).unwrap(), b"stable settings must not change");
            }
        }
        let private = fixture.root.join("EffectCraftData-backup-name/nightly");
        assert!(fixture.check(Platform::Linux, &private).is_ok(), "a merely similar name is not a portable settings component");
    }

    #[cfg(unix)]
    #[test]
    fn settings_aliases_cannot_redirect_to_stable_missing_or_profile_root_data() {
        let fixture = Fixture::new();
        let stable = fixture.home().join(".config/effectcraft/Plug-ins");
        std::fs::create_dir_all(&stable).unwrap();
        std::fs::write(stable.join("stable-plugin.wasm"), b"stable plugin bytes").unwrap();
        for (index, target) in [stable.clone(), fixture.root.join("missing target")].into_iter().enumerate() {
            let private = fixture.root.join(format!("nightly-{index}"));
            std::fs::create_dir(&private).unwrap();
            std::os::unix::fs::symlink(&target, private.join("Plug-ins")).unwrap();
            let probed = std::cell::Cell::new(false);
            let result = validate_for(
                Platform::Linux,
                &private,
                |key| fixture.get(key),
                Some(&fixture.exe),
                |_| {
                    probed.set(true);
                    Ok(())
                },
            );
            assert!(result.is_err(), "a profile entry must not escape the profile or follow a dangling alias");
            assert!(!probed.get());
        }
        let private = fixture.root.join("nightly-self");
        std::fs::create_dir(&private).unwrap();
        std::os::unix::fs::symlink(&private, private.join("Logs")).unwrap();
        assert!(fixture.check(Platform::Linux, &private).is_err(), "an entry cannot resolve to the profile itself");
        assert_eq!(std::fs::read(stable.join("stable-plugin.wasm")).unwrap(), b"stable plugin bytes");
    }

    #[cfg(unix)]
    #[test]
    fn an_alias_to_a_private_subfolder_keeps_the_profile_usable() {
        let fixture = Fixture::new();
        let private = fixture.root.join("nightly-private");
        let logs = private.join("private-logs");
        std::fs::create_dir_all(&logs).unwrap();
        std::os::unix::fs::symlink(&logs, private.join("Logs")).unwrap();
        assert!(fixture.check(Platform::Linux, &private).is_ok());
        assert_eq!(std::fs::canonicalize(private.join("Logs")).unwrap(), logs);
    }

    #[test]
    fn overlap_check_handles_ancestor_descendant_and_prefix_lookalikes() {
        let fixture = Fixture::new();
        let root = fixture.home().join("nightly/profiles/effectcraft");
        for platform in [Platform::Windows, Platform::Linux, Platform::Mac] {
            assert!(overlaps(&root, &root.join("Plug-ins"), platform));
            assert!(overlaps(&root.join("Plug-ins"), &root, platform));
            assert!(!overlaps(&root, &root.with_file_name("effectcraft-other"), platform));
        }
    }

    #[test]
    fn linux_fences_app_xdg_subtrees_but_allows_launcher_sibling() {
        let mut fixture = Fixture::new();
        for key in ["XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_CACHE_HOME", "XDG_STATE_HOME"] {
            fixture.env.insert(key.into(), fixture.root.join(key).into_os_string());
        }
        for key in ["XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_CACHE_HOME", "XDG_STATE_HOME"] {
            let stable = PathBuf::from(fixture.get(key).unwrap()).join("effectcraft");
            std::fs::create_dir_all(&stable).unwrap();
            std::fs::write(stable.join("stable-sentinel"), b"stable bytes").unwrap();
            for candidate in [stable.clone(), stable.join("nightly"), stable.parent().unwrap().to_path_buf()] {
                assert!(fixture.check(Platform::Linux, &candidate).unwrap_err().contains("overlap"));
            }
            assert_eq!(std::fs::read(stable.join("stable-sentinel")).unwrap(), b"stable bytes");
        }
        let data = PathBuf::from(fixture.get("XDG_DATA_HOME").unwrap());
        let nightly = data.join("ArtCraftLauncher/nightly/profiles/effectcraft");
        assert_eq!(fixture.check(Platform::Linux, &nightly).unwrap(), nightly);
        assert_eq!(std::fs::read_dir(nightly).unwrap().count(), 0);
    }

    #[test]
    fn mac_fences_only_app_library_subtrees_and_the_entire_bundle() {
        let mut fixture = Fixture::new();
        fixture.exe = fixture.root.join("install/EffectCraft Nightly.app/Contents/MacOS/EffectCraft");
        std::fs::create_dir_all(fixture.exe.parent().unwrap()).unwrap();
        std::fs::write(&fixture.exe, b"fixture executable, never run").unwrap();
        let library = fixture.home().join("Library");
        for suffix in [
            "Application Support/EffectCraft",
            "Caches/EffectCraft",
            "Caches/ai.storyteller.effectcraft",
            "Preferences/EffectCraft",
            "Saved Application State/ai.storyteller.effectcraft.savedState",
        ] {
            let stable = library.join(suffix);
            assert!(fixture.check(Platform::Mac, &stable.join("nightly")).unwrap_err().contains("overlap"));
            assert!(!stable.exists());
        }
        let inside_resources = fixture.exe.parent().unwrap().parent().unwrap().join("Resources/profile");
        assert!(fixture.check(Platform::Mac, &inside_resources).unwrap_err().contains("overlap"));
        assert!(!inside_resources.exists());
        let valid = library.join("Application Support/ArtCraftLauncher/nightly/profiles/effectcraft");
        assert_eq!(fixture.check(Platform::Mac, &valid).unwrap(), valid);
    }

    #[test]
    fn windows_roaming_local_and_install_fences_remain_active() {
        let fixture = Fixture::new();
        for key in ["APPDATA", "LOCALAPPDATA"] {
            let stable = PathBuf::from(fixture.get(key).unwrap()).join("EffectCraft");
            assert!(fixture.check(Platform::Windows, &stable.join("nightly")).unwrap_err().contains("overlap"));
            assert!(!stable.exists());
        }
        assert!(fixture.check(Platform::Windows, &fixture.exe.parent().unwrap().join("profile")).is_err());
        assert!(fixture.check(Platform::Windows, &fixture.root.join("Launcher/profiles/effectcraft")).is_ok());
    }

    #[test]
    fn windows_default_msi_install_folders_are_fenced() {
        let mut fixture = Fixture::new();
        let local = PathBuf::from(fixture.get("LOCALAPPDATA").unwrap());
        let per_user = local.join("Programs").join("ArtCraft").join("EffectCraft");
        assert!(fixture.check(Platform::Windows, &per_user.join("nightly")).unwrap_err().contains("overlap"));
        for key in ["ProgramW6432", "ProgramFiles", "ProgramFiles(x86)"] {
            let base = fixture.root.join(format!("machine {key}"));
            fixture.env.insert(key.into(), base.clone().into_os_string());
            let machine = base.join("ArtCraft").join("EffectCraft");
            assert!(fixture.check(Platform::Windows, &machine.join("nightly")).unwrap_err().contains("overlap"));
            assert!(!machine.exists());
        }
        let sibling = local.join("Programs").join("ArtCraft").join("ArtCraftLauncher").join("nightly").join("effectcraft");
        assert!(fixture.check(Platform::Windows, &sibling).is_ok(), "the Launcher's own Programs sibling is not EffectCraft's install");
    }

    #[test]
    fn folders_under_a_stable_portable_copy_are_refused_before_any_probe() {
        let fixture = Fixture::new();
        let portable = fixture.root.join("D/EC");
        std::fs::create_dir_all(&portable).unwrap();
        std::fs::write(portable.join(PORTABLE_MARKERS[1]), b"stable portable copy").unwrap();
        std::fs::write(portable.join("effectcraft.exe"), b"stable portable executable, never run").unwrap();
        for platform in [Platform::Windows, Platform::Linux, Platform::Mac] {
            for candidate in [portable.join("nightly"), portable.join("deeper/nightly")] {
                let probed = std::cell::Cell::new(false);
                let result = validate_for(
                    platform,
                    &candidate,
                    |key| fixture.get(key),
                    Some(&fixture.exe),
                    |_| {
                        probed.set(true);
                        Ok(())
                    },
                );
                assert!(result.unwrap_err().contains("portable"));
                assert!(!probed.get());
                assert!(!candidate.exists());
            }
        }
        // A marker that no EffectCraft executable sits beside is someone else's file.
        let unrelated = fixture.root.join("user folder");
        std::fs::create_dir_all(&unrelated).unwrap();
        std::fs::write(unrelated.join(PORTABLE_MARKERS[0]), b"another app's marker").unwrap();
        assert!(fixture.check(Platform::Windows, &unrelated.join("Launcher/nightly/effectcraft")).is_ok());
    }

    #[test]
    fn the_stable_presets_folder_is_fenced_on_every_platform() {
        let fixture = Fixture::new();
        let documents = fixture.home().join("Documents");
        for platform in [Platform::Windows, Platform::Linux, Platform::Mac] {
            for candidate in [documents.join("EffectCraft"), documents.join("EffectCraft/Presets"), documents.join("EffectCraft/nightly")] {
                assert!(fixture.check(platform, &candidate).unwrap_err().contains("overlap"), "{platform:?} {candidate:?}");
                assert!(!candidate.exists());
            }
        }
        assert!(fixture.check(Platform::Linux, &documents.join("EffectCraft Nightly")).is_ok(), "a sibling folder stays valid");
    }

    #[test]
    fn linux_fences_the_stable_flatpak_data_folder() {
        let fixture = Fixture::new();
        let flatpak = fixture.home().join(".var/app/ai.storyteller.effectcraft");
        for candidate in [flatpak.clone(), flatpak.join("data/nightly")] {
            assert!(fixture.check(Platform::Linux, &candidate).unwrap_err().contains("overlap"));
        }
        assert!(!flatpak.exists());
        assert!(fixture.check(Platform::Linux, &fixture.home().join(".var/app/ai.storyteller.launcher/nightly")).is_ok());
    }

    #[test]
    fn ambiguous_environment_and_non_native_sandbox_are_refused_before_creation() {
        let mut fixture = Fixture::new();
        let candidate = fixture.home().join("nightly/profiles/effectcraft");
        fixture.env.insert("XDG_CACHE_HOME".into(), "relative/cache".into());
        assert!(fixture.check(Platform::Linux, &candidate).unwrap_err().contains("absolute"));
        fixture.env.remove("XDG_CACHE_HOME");
        fixture.env.insert("EFFECTCRAFT_CONFIG_DIR".into(), fixture.home().join("custom-stable").into_os_string());
        assert!(fixture.check(Platform::Linux, &candidate).unwrap_err().contains("EFFECTCRAFT_CONFIG_DIR"));
        fixture.env.remove("EFFECTCRAFT_CONFIG_DIR");
        fixture.env.insert("FLATPAK_ID".into(), "ai.storyteller.effectcraft".into());
        assert!(fixture.check(Platform::Linux, &candidate).unwrap_err().contains("Flatpak"));
        fixture.env.remove("FLATPAK_ID");
        fixture.env.remove("HOME");
        assert!(fixture.check(Platform::Linux, &candidate).unwrap_err().contains("HOME"));
        assert!(!candidate.exists());
    }

    #[test]
    fn only_the_nightly_s_own_flatpak_instance_may_carry_a_profile() {
        // Inside packaging/linux/flatpak-bundle.sh's sandbox, $HOME is this app id's own folder, so
        // the launcher's profile is the Nightly's state and nothing else is reachable through it.
        let mut fixture = Fixture::new();
        fixture.env.insert("FLATPAK_ID".into(), NIGHTLY_FLATPAK_ID.into());
        let candidate = fixture.home().join(".local/share/effectcraft-nightly");
        assert_eq!(fixture.check(Platform::Linux, &candidate).unwrap(), std::fs::canonicalize(&candidate).unwrap());
        fixture.env.remove("FLATPAK_ID");
        for refused in ["ai.storyteller.effectcraft", "ai.storyteller.effectcraft.nightly-v1", "org.example.other"] {
            let mut other = Fixture::new();
            other.env.insert("FLATPAK_ID".into(), refused.into());
            let refused_root = other.home().join(".local/share/effectcraft-nightly");
            assert!(other.check(Platform::Linux, &refused_root).unwrap_err().contains("Flatpak"), "{refused}");
            assert!(!refused_root.exists());
        }
    }

    #[test]
    fn non_directory_and_failed_write_probe_never_fall_back_to_stable() {
        let fixture = Fixture::new();
        let file = fixture.home().join("not-a-directory");
        std::fs::write(&file, b"occupied").unwrap();
        assert!(fixture.check(Platform::Linux, &file).is_err());
        let refused = fixture.home().join("read-only-profile");
        assert!(
            validate_for(
                Platform::Linux,
                &refused,
                |key| fixture.get(key),
                Some(&fixture.exe),
                |_| { Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, "synthetic read-only profile")) }
            )
            .unwrap_err()
            .contains("not writable")
        );
        assert!(!fixture.home().join(".config/effectcraft").exists(), "no stable fallback was created");
    }

    #[test]
    fn created_profile_is_private_and_resolved() {
        let fixture = Fixture::new();
        let target = fixture.home().join("nightly/profiles/effectcraft");
        let resolved = fixture.check(Platform::Linux, &target).unwrap();
        assert_eq!(resolved, target);
        assert!(resolved.is_dir());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            assert_eq!(std::fs::metadata(&resolved).unwrap().permissions().mode() & 0o777, 0o700);
        }
    }
}
