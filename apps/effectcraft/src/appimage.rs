//! Desktop integration for the AppImage. Wayland compositors (GNOME, KDE…) show a window's icon
//! from the desktop entry named by its app ID (`ai.storyteller.effectcraft.desktop`). The .deb,
//! .rpm and Flatpak packages install one; an AppImage installs nothing, so its window got the
//! generic icon. Started from an AppImage (`$APPIMAGE`), the app adds a hidden entry
//! (`NoDisplay=true`: no menu item) pointing at it and its icon under the user's data folder
//! (`$XDG_DATA_HOME`, else `~/.local/share`). Nothing is written when another entry already
//! provides the ID (an installed package, or the user's own), and an entry it wrote is kept up to
//! date when the AppImage moves. Failures are only logged: the icon is cosmetic.

use std::path::{Path, PathBuf};

pub const APP_ID: &str = "ai.storyteller.effectcraft";

/// Marks entries this module wrote (it never touches others).
const MARKER: &str = "X-VisualabEffects-AppImage=true";

/// A path as one `Exec` argument (desktop entry spec: quoted, with `"`, `` ` ``, `$` and `\`
/// escaped; `%` doubled so it isn't a field code).
pub fn exec_arg(path: &str) -> String {
    let mut s = String::with_capacity(path.len() + 2);
    s.push('"');
    for c in path.chars() {
        // The quoting escape is a backslash, which the key file's string syntax escapes again:
        // `"` is written `\\"`, a backslash `\\\\`.
        match c {
            '"' | '`' | '$' => {
                s.push_str("\\\\");
                s.push(c);
            }
            '\\' => s.push_str("\\\\\\\\"),
            '%' => s.push_str("%%"),
            '\n' | '\r' => {}
            c => s.push(c),
        }
    }
    s.push('"');
    s
}

/// The hidden desktop entry for the AppImage at `appimage`.
pub fn entry(appimage: &str) -> String {
    format!(
        "[Desktop Entry]\nType=Application\nName=Visualab Effects\nComment=Motion graphics and visual effects compositor\nExec={} %F\nIcon={APP_ID}\nTerminal=false\nStartupWMClass={APP_ID}\nNoDisplay=true\n{MARKER}\n",
        exec_arg(appimage)
    )
}

/// Write the entry and `icon` (a 256 px PNG) under `data_home`, unless one of `data_dirs` (or
/// `data_home` itself, with an entry we didn't write) already has the ID's entry. Returns whether
/// anything was written.
pub fn integrate(appimage: &str, data_home: &Path, data_dirs: &[PathBuf], icon: &[u8]) -> std::io::Result<bool> {
    let name = format!("{APP_ID}.desktop");
    if data_dirs.iter().any(|d| d.join("applications").join(&name).exists()) {
        return Ok(false);
    }
    let path = data_home.join("applications").join(&name);
    let text = entry(appimage);
    let icon_path = data_home.join("icons/hicolor/256x256/apps").join(format!("{APP_ID}.png"));
    match std::fs::read_to_string(&path) {
        Ok(old) if !old.contains(MARKER) => return Ok(false),
        Ok(old) if old == text && icon_path.exists() => return Ok(false),
        _ => {}
    }
    write_atomic(&icon_path, icon)?;
    write_atomic(&path, text.as_bytes())?;
    Ok(true)
}

fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("part");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)
}

/// [`integrate`] for this process: when started from an AppImage, with the XDG data folders of
/// the environment.
pub fn integrate_from_env(icon: &[u8]) {
    let Some(appimage) = std::env::var_os("APPIMAGE").map(|a| a.to_string_lossy().into_owned()) else { return };
    let env_path = |k: &str| std::env::var_os(k).filter(|v| !v.is_empty()).map(PathBuf::from);
    let Some(data_home) = env_path("XDG_DATA_HOME").or_else(|| env_path("HOME").map(|h| h.join(".local/share"))) else { return };
    let dirs = std::env::var("XDG_DATA_DIRS").ok().filter(|v| !v.is_empty()).unwrap_or_else(|| "/usr/local/share:/usr/share".into());
    let data_dirs: Vec<PathBuf> = dirs.split(':').filter(|d| !d.is_empty()).map(PathBuf::from).collect();
    match integrate(&appimage, &data_home, &data_dirs, icon) {
        Ok(true) => log::info!("AppImage: desktop entry and icon written to {}", data_home.display()),
        Ok(false) => {}
        Err(e) => log::warn!("AppImage: could not write the desktop entry: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exec_arguments_are_quoted_and_escaped() {
        assert_eq!(exec_arg("/home/a/VisualabEffects.AppImage"), "\"/home/a/VisualabEffects.AppImage\"");
        assert_eq!(exec_arg("/a b/$x\"y`z\\w%"), "\"/a b/\\\\$x\\\\\"y\\\\`z\\\\\\\\w%%\"");
        let e = entry("/opt/VisualabEffects.AppImage");
        assert!(e.contains("Exec=\"/opt/VisualabEffects.AppImage\" %F\n") && e.contains("NoDisplay=true\n") && e.contains("Icon=ai.storyteller.effectcraft\n"));
    }

    #[test]
    fn writes_once_follows_moves_and_leaves_other_entries_alone() {
        let root = std::env::temp_dir().join(format!("ec-appimage-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (home, system) = (root.join("home"), root.join("system"));
        let entry_path = home.join("applications/ai.storyteller.effectcraft.desktop");
        let icon_path = home.join("icons/hicolor/256x256/apps/ai.storyteller.effectcraft.png");
        // First start: entry and icon written; again: nothing to do.
        assert!(integrate("/a/VisualabEffects.AppImage", &home, std::slice::from_ref(&system), b"png").unwrap());
        assert_eq!(std::fs::read(&icon_path).unwrap(), b"png");
        assert!(std::fs::read_to_string(&entry_path).unwrap().contains("Exec=\"/a/VisualabEffects.AppImage\""));
        assert!(!integrate("/a/VisualabEffects.AppImage", &home, std::slice::from_ref(&system), b"png").unwrap());
        // The AppImage moved: the entry follows it.
        assert!(integrate("/b/VisualabEffects.AppImage", &home, std::slice::from_ref(&system), b"png").unwrap());
        assert!(std::fs::read_to_string(&entry_path).unwrap().contains("Exec=\"/b/VisualabEffects.AppImage\""));
        // The user's own entry is never replaced.
        std::fs::write(&entry_path, "[Desktop Entry]\nName=Mine\n").unwrap();
        assert!(!integrate("/c/VisualabEffects.AppImage", &home, std::slice::from_ref(&system), b"png").unwrap());
        assert_eq!(std::fs::read_to_string(&entry_path).unwrap(), "[Desktop Entry]\nName=Mine\n");
        // An installed package provides the entry: nothing is written.
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(system.join("applications")).unwrap();
        std::fs::write(system.join("applications/ai.storyteller.effectcraft.desktop"), "x").unwrap();
        assert!(!integrate("/a/VisualabEffects.AppImage", &home, std::slice::from_ref(&system), b"png").unwrap());
        assert!(!entry_path.exists());
        let _ = std::fs::remove_dir_all(&root);
    }
}
