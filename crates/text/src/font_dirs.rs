//! The font folders of a Linux/BSD system, found the way the desktop finds them: the usual
//! folders, the XDG data folders (`$XDG_DATA_HOME/fonts`, `$XDG_DATA_DIRS/*/fonts`), the host font
//! folders a Flatpak sandbox exposes under `/run/host`, and the `<dir>` entries of the fontconfig
//! configuration (`/etc/fonts/fonts.conf` with its includes, `~/.config/fontconfig/fonts.conf`).
//!
//! The fontconfig files are read with a small tag scanner (no fontconfig library, no XML crate):
//! only `<dir>`, `<remap-dir>` and `<include>` matter here. The work is bounded: include depth,
//! number and size of the files read, and every file is read at most once.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// The environment the folders depend on, passed in so tests don't touch the process environment.
#[derive(Clone, Debug, Default)]
pub(crate) struct FontEnv {
    pub home: Option<PathBuf>,
    pub xdg_data_home: Option<PathBuf>,
    pub xdg_data_dirs: Option<std::ffi::OsString>,
    pub xdg_config_home: Option<PathBuf>,
    /// `FONTCONFIG_FILE`: the main configuration file instead of the standard ones.
    pub fontconfig_file: Option<PathBuf>,
    /// `FONTCONFIG_PATH`: ordered search folders, before the standard configuration folders.
    pub fontconfig_path: Option<std::ffi::OsString>,
    /// The main configuration files tried when `FONTCONFIG_FILE` is unset (the first that exists).
    pub fontconfig_roots: Vec<PathBuf>,
    /// Folders scanned if they exist (the usual system folders and the Flatpak host folders).
    pub fixed: Vec<PathBuf>,
}

impl FontEnv {
    pub fn from_process() -> Self {
        let var = |k: &str| std::env::var_os(k).filter(|v| !v.is_empty());
        Self {
            home: var("HOME").map(PathBuf::from),
            xdg_data_home: var("XDG_DATA_HOME").map(PathBuf::from),
            xdg_data_dirs: var("XDG_DATA_DIRS"),
            xdg_config_home: var("XDG_CONFIG_HOME").map(PathBuf::from),
            fontconfig_file: var("FONTCONFIG_FILE").map(PathBuf::from),
            fontconfig_path: var("FONTCONFIG_PATH"),
            fontconfig_roots: ["/etc/fonts/fonts.conf", "/usr/local/etc/fonts/fonts.conf"].map(PathBuf::from).to_vec(),
            fixed: [
                "/usr/share/fonts",
                "/usr/local/share/fonts",
                // Flatpak: the host's system, local and user fonts (no filesystem permission needed).
                "/run/host/fonts",
                "/run/host/local-fonts",
                "/run/host/user-fonts",
            ]
            .map(PathBuf::from)
            .to_vec(),
        }
    }

    fn data_home(&self) -> Option<PathBuf> {
        self.xdg_data_home.clone().filter(|p| p.is_absolute()).or_else(|| self.home.as_ref().map(|h| h.join(".local/share")))
    }

    fn config_home(&self) -> Option<PathBuf> {
        self.xdg_config_home.clone().filter(|p| p.is_absolute()).or_else(|| self.home.as_ref().map(|h| h.join(".config")))
    }
}

/// Every font folder for `env`, without duplicates, in a stable order. Folders that don't exist
/// are kept (the scan skips them).
pub(crate) fn unix_font_dirs(env: &FontEnv) -> Vec<PathBuf> {
    let mut dirs = env.fixed.clone();
    if let Some(h) = &env.home {
        dirs.push(h.join(".local/share/fonts"));
        dirs.push(h.join(".fonts"));
    }
    if let Some(d) = env.data_home() {
        dirs.push(d.join("fonts"));
    }
    let data_dirs = env.xdg_data_dirs.clone().unwrap_or_else(|| "/usr/local/share:/usr/share".into());
    dirs.extend(std::env::split_paths(&data_dirs).filter(|p| p.is_absolute()).map(|p| p.join("fonts")));
    dirs.extend(fontconfig_dirs(env));
    dedup_dirs(dirs)
}

/// Drop repeated folders (also the same folder reached through a symlink), keeping the first.
fn dedup_dirs(dirs: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut seen = HashSet::new();
    dirs.into_iter().filter(|d| seen.insert(std::fs::canonicalize(d).unwrap_or_else(|_| d.clone()))).collect()
}

const MAX_INCLUDE_DEPTH: u32 = 8;
const MAX_FILES: usize = 512;
const MAX_FILE_BYTES: u64 = 1 << 20;

/// The font folders named by the fontconfig configuration.
pub(crate) fn fontconfig_dirs(env: &FontEnv) -> Vec<PathBuf> {
    let mut st = Reader { env, out: Vec::new(), seen: HashSet::new() };
    let root = fontconfig_root(env);
    if let Some(root) = root {
        st.file(&root, 0);
    }
    // The user's own configuration, in case the system one doesn't include it.
    if let Some(c) = env.config_home() {
        st.include(&c.join("fontconfig/fonts.conf"), 0);
        st.include(&c.join("fontconfig/conf.d"), 0);
    }
    st.out
}

/// Select the main file as fontconfig does: absolute names bypass the search path;
/// relative names (or `fonts.conf`) use FONTCONFIG_PATH, then the standard folders.
fn fontconfig_root(env: &FontEnv) -> Option<PathBuf> {
    if let Some(file) = &env.fontconfig_file
        && file.is_absolute()
    {
        return Some(file.clone());
    }
    if env.fontconfig_file.is_none() && env.fontconfig_path.is_none() {
        return env.fontconfig_roots.iter().find(|p| readable_config(p)).cloned();
    }
    let file = env.fontconfig_file.as_deref().unwrap_or_else(|| Path::new("fonts.conf"));
    let mut dirs = Vec::new();
    if let Some(path) = &env.fontconfig_path {
        let mut paths = std::env::split_paths(path).peekable();
        while let Some(mut dir) = paths.next() {
            // Fontconfig ignores a final empty item; an interior empty item names `/`.
            if dir.as_os_str().is_empty() {
                if paths.peek().is_none() {
                    continue;
                }
                dir = PathBuf::from("/");
            }
            dirs.push(dir);
        }
    }
    dirs.extend(env.fontconfig_roots.iter().filter_map(|p| p.parent().map(Path::to_path_buf)));
    dirs.into_iter().map(|dir| dir.join(file)).find(|p| readable_config(p))
}

fn readable_config(path: &Path) -> bool {
    path.is_file() && std::fs::File::open(path).is_ok()
}

struct Reader<'a> {
    env: &'a FontEnv,
    out: Vec<PathBuf>,
    seen: HashSet<PathBuf>,
}

impl Reader<'_> {
    /// Read a configuration file or, for a folder, its `NN-name.conf` files in name order.
    fn include(&mut self, path: &Path, depth: u32) {
        if path.is_dir() {
            let Ok(rd) = std::fs::read_dir(path) else { return };
            let mut files: Vec<PathBuf> = rd
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.ends_with(".conf") && n.starts_with(|c: char| c.is_ascii_digit())))
                .collect();
            files.sort();
            for f in files {
                self.file(&f, depth);
            }
        } else {
            self.file(path, depth);
        }
    }

    fn file(&mut self, path: &Path, depth: u32) {
        if depth > MAX_INCLUDE_DEPTH || self.seen.len() >= MAX_FILES || !path.is_file() {
            return;
        }
        let key = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        if !self.seen.insert(key) {
            return;
        }
        if std::fs::metadata(path).map(|m| m.len() > MAX_FILE_BYTES).unwrap_or(true) {
            return;
        }
        let Ok(text) = std::fs::read_to_string(path) else { return };
        let base = path.parent().map(Path::to_path_buf).unwrap_or_default();
        for el in elements(&text) {
            match el.name {
                "dir" | "remap-dir" => {
                    if let Some(p) = self.resolve(&el, &base, false) {
                        self.out.push(p);
                    }
                }
                "include" => {
                    if let Some(p) = self.resolve(&el, &base, true) {
                        self.include(&p, depth.saturating_add(1));
                    }
                }
                _ => {}
            }
        }
    }

    /// The path an element names: `~` is the home folder, `prefix="xdg"` the XDG data (folders)
    /// or config (includes) home, `prefix="relative"` and plain relative paths the including
    /// file's folder, `prefix="default"`/`"cwd"` the working folder.
    fn resolve(&self, el: &Element, base: &Path, include: bool) -> Option<PathBuf> {
        let v = el.text.trim();
        if v.is_empty() {
            return None;
        }
        if let Some(rest) = v.strip_prefix('~') {
            return Some(self.env.home.as_ref()?.join(rest.trim_start_matches('/')));
        }
        let p = Path::new(v);
        if p.is_absolute() {
            return Some(p.to_path_buf());
        }
        match el.attr("prefix") {
            Some("xdg") => Some(if include { self.env.config_home()? } else { self.env.data_home()? }.join(p)),
            Some("default" | "cwd") => std::env::current_dir().ok().map(|c| c.join(p)),
            _ => Some(base.join(p)),
        }
    }
}

struct Element<'a> {
    name: &'a str,
    attrs: &'a str,
    text: String,
}

impl Element<'_> {
    fn attr(&self, key: &str) -> Option<&str> {
        let mut rest = self.attrs;
        while let Some(eq) = rest.find('=') {
            let k = rest.get(..eq)?.trim();
            let after = rest.get(eq + 1..)?.trim_start();
            let q = after.chars().next()?;
            if q != '"' && q != '\'' {
                return None;
            }
            let body = after.get(1..)?;
            let end = body.find(q)?;
            if k == key {
                return body.get(..end);
            }
            rest = body.get(end + 1..)?;
        }
        None
    }
}

/// The `<dir>`, `<remap-dir>` and `<include>` elements of a fontconfig file, comments skipped.
fn elements(src: &str) -> Vec<Element<'_>> {
    let mut out = Vec::new();
    let mut i = 0;
    while let Some(off) = src.get(i..).and_then(|s| s.find('<')) {
        let start = i + off;
        let Some(tail) = src.get(start..) else { break };
        if tail.starts_with("<!--") {
            match tail.find("-->") {
                Some(e) => {
                    i = start + e + 3;
                    continue;
                }
                None => break,
            }
        }
        let Some(close) = tail.find('>') else { break };
        let inner = tail.get(1..close).unwrap_or_default();
        i = start + close + 1;
        let name_len = inner.find(|c: char| c.is_whitespace() || c == '/').unwrap_or(inner.len());
        let name = inner.get(..name_len).unwrap_or_default();
        if !matches!(name, "dir" | "remap-dir" | "include") || inner.ends_with('/') {
            continue;
        }
        let attrs = inner.get(name_len..).unwrap_or_default();
        let end_tag = format!("</{name}>");
        let Some(body_len) = src.get(i..).and_then(|s| s.find(&end_tag)) else { break };
        let body = src.get(i..i + body_len).unwrap_or_default();
        i += body_len + end_tag.len();
        out.push(Element { name, attrs, text: decode_entities(body) });
    }
    out
}

fn decode_entities(s: &str) -> String {
    s.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'").replace("&amp;", "&")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("ec-font-dirs-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn env_in(root: &Path) -> FontEnv {
        FontEnv {
            home: Some(root.join("home")),
            fontconfig_roots: vec![root.join("etc/fonts/fonts.conf")],
            xdg_data_dirs: Some("/nonexistent-share".into()),
            ..Default::default()
        }
    }

    fn write(p: &Path, s: &str) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, s).unwrap();
    }

    #[test]
    fn fontconfig_dirs_follow_includes_prefixes_and_skip_the_rest() {
        let root = temp("conf");
        let etc = root.join("etc/fonts");
        write(
            &etc.join("fonts.conf"),
            r#"<?xml version="1.0"?>
<!DOCTYPE fontconfig SYSTEM "urn:fontconfig:fonts.dtd">
<fontconfig>
  <!-- <dir>/commented/out</dir> -->
  <dir>/sys/fonts</dir>
  <dir prefix="xdg">fonts</dir>
  <dir>~/.fonts</dir>
  <cachedir>/var/cache/fontconfig</cachedir>
  <cachedir prefix="xdg">fontconfig</cachedir>
  <dir prefix="relative">local-fonts</dir>
  <dir salt="x">/a&amp;b/fonts</dir>
  <include ignore_missing="yes">conf.d</include>
  <include ignore_missing="yes">missing.conf</include>
  <include ignore_missing="yes" prefix="xdg">fontconfig/fonts.conf</include>
  <include>fonts.conf</include>
</fontconfig>"#,
        );
        write(
            &etc.join("conf.d/50-flatpak.conf"),
            "<fontconfig><reset-dirs /><dir>/run/host/fonts</dir><include ignore_missing=\"yes\">/run/host/font-dirs.xml</include></fontconfig>",
        );
        write(&etc.join("conf.d/60-remap.conf"), "<fontconfig><remap-dir as-path=\"/usr/share/fonts\">/remapped/fonts</remap-dir></fontconfig>");
        write(&etc.join("conf.d/README"), "<dir>/not/a/conf/file</dir>");
        write(&etc.join("conf.d/nodigit.conf"), "<fontconfig><dir>/no/digit</dir></fontconfig>");
        write(&root.join("home/.config/fontconfig/fonts.conf"), "<fontconfig><dir>/user/conf/fonts</dir></fontconfig>");
        let dirs = fontconfig_dirs(&env_in(&root));
        let home = root.join("home");
        let want = vec![
            PathBuf::from("/sys/fonts"),
            home.join(".local/share/fonts"),
            home.join(".fonts"),
            etc.join("local-fonts"),
            PathBuf::from("/a&b/fonts"),
            PathBuf::from("/run/host/fonts"),
            PathBuf::from("/remapped/fonts"),
            PathBuf::from("/user/conf/fonts"),
        ];
        assert_eq!(dirs, want);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn include_cycles_and_unclosed_tags_terminate() {
        let root = temp("cycle");
        let etc = root.join("etc/fonts");
        write(&etc.join("fonts.conf"), "<fontconfig><dir>/one</dir><include>b.conf</include><include>fonts.conf</include></fontconfig>");
        write(&etc.join("b.conf"), "<fontconfig><include>fonts.conf</include><dir>/two</dir><dir>/unclosed");
        let dirs = fontconfig_dirs(&env_in(&root));
        assert_eq!(dirs, vec![PathBuf::from("/one"), PathBuf::from("/two")]);
        assert!(fontconfig_dirs(&FontEnv::default()).is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn xdg_and_flatpak_folders_are_listed_once() {
        let root = temp("xdg");
        let env = FontEnv {
            home: Some(root.join("home")),
            xdg_data_home: Some(root.join("data")),
            xdg_data_dirs: Some(std::env::join_paths([root.join("share"), "relative/ignored".into()]).unwrap()),
            fixed: vec![PathBuf::from("/run/host/fonts")],
            fontconfig_file: Some(root.join("fonts.conf")),
            ..Default::default()
        };
        write(&root.join("fonts.conf"), "<fontconfig><dir>/run/host/fonts</dir><dir prefix=\"xdg\">fonts</dir></fontconfig>");
        let dirs = unix_font_dirs(&env);
        assert_eq!(
            dirs,
            vec![
                PathBuf::from("/run/host/fonts"),
                root.join("home/.local/share/fonts"),
                root.join("home/.fonts"),
                root.join("data/fonts"),
                root.join("share/fonts"),
            ]
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A font in a folder that only the fontconfig configuration names (as in a Flatpak sandbox,
    /// where the host's fonts are under /run/host) is found by the scan.
    #[test]
    fn a_font_named_only_by_fontconfig_is_scanned() {
        use crate::fonts::FontSource;
        let root = temp("e2e");
        let host = root.join("run/host/fonts/TTF");
        std::fs::create_dir_all(&host).unwrap();
        std::fs::write(host.join("NOTOSERIF.TTF"), crate::fonts::NOTO_SERIF_REGULAR).unwrap();
        write(&root.join("etc/fonts/conf.d/50-flatpak.conf"), &format!("<fontconfig><dir>{}</dir></fontconfig>", root.join("run/host/fonts").display()));
        write(&root.join("etc/fonts/fonts.conf"), "<fontconfig><include ignore_missing=\"yes\">conf.d</include></fontconfig>");
        let src = crate::fonts::DirectorySource { dirs: unix_font_dirs(&env_in(&root)) };
        let faces = src.faces();
        assert!(faces.iter().any(|f| f.family == "Noto Serif" && f.origin == "system"), "{:?}", faces.iter().map(|f| &f.family).collect::<Vec<_>>());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn fontconfig_path_selects_the_first_existing_config_in_search_order() {
        let root = temp("config-search");
        let first = root.join("first");
        let second = root.join("second");
        write(&first.join("fonts.conf"), "<fontconfig><dir>/first/fonts</dir></fontconfig>");
        write(&second.join("fonts.conf"), "<fontconfig><dir>/second/fonts</dir></fontconfig>");
        let mut env = env_in(&root);
        env.fontconfig_path = Some(std::env::join_paths([root.join("missing"), first.clone(), second.clone()]).unwrap());
        assert_eq!(fontconfig_dirs(&env), [PathBuf::from("/first/fonts")]);
        env.fontconfig_path = Some(std::env::join_paths([second, first]).unwrap());
        assert_eq!(fontconfig_dirs(&env), [PathBuf::from("/second/fonts")]);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_relative_selected_config_uses_the_search_path_and_keeps_relative_includes() {
        let root = temp("selected-config");
        let config = root.join("config");
        write(&config.join("chosen.conf"), "<fontconfig><include>conf.d</include></fontconfig>");
        write(&config.join("conf.d/10-fonts.conf"), "<fontconfig><dir prefix=\"relative\">../../fonts</dir></fontconfig>");
        let mut env = env_in(&root);
        env.fontconfig_path = Some(config.clone().into_os_string());
        env.fontconfig_file = Some(PathBuf::from("chosen.conf"));
        assert_eq!(fontconfig_dirs(&env), [config.join("conf.d/../../fonts")]);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn an_absolute_selected_config_bypasses_the_search_path() {
        let root = temp("absolute-config");
        write(&root.join("search/fonts.conf"), "<fontconfig><dir>/wrong/fonts</dir></fontconfig>");
        write(&root.join("selected.conf"), "<fontconfig><dir>/selected/fonts</dir></fontconfig>");
        let mut env = env_in(&root);
        env.fontconfig_path = Some(root.join("search").into_os_string());
        env.fontconfig_file = Some(root.join("selected.conf"));
        assert_eq!(fontconfig_dirs(&env), [PathBuf::from("/selected/fonts")]);
        env.fontconfig_file = Some(root.join("missing.conf"));
        assert!(fontconfig_dirs(&env).is_empty(), "an explicit missing file does not pick another config");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn configuration_search_falls_back_to_standard_folders() {
        let root = temp("default-config");
        let config = root.join("etc/fonts");
        write(&config.join("fonts.conf"), "<fontconfig><dir>/default/fonts</dir></fontconfig>");
        write(&config.join("selected.conf"), "<fontconfig><dir>/selected/fonts</dir></fontconfig>");
        let mut env = env_in(&root);
        env.fontconfig_path = Some(root.join("missing").into_os_string());
        assert_eq!(fontconfig_dirs(&env), [PathBuf::from("/default/fonts")]);
        env.fontconfig_file = Some(PathBuf::from("selected.conf"));
        assert_eq!(fontconfig_dirs(&env), [PathBuf::from("/selected/fonts")]);
        env.fontconfig_path = None;
        assert_eq!(fontconfig_dirs(&env), [PathBuf::from("/selected/fonts")]);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[cfg(unix)]
    #[test]
    fn configuration_search_preserves_non_unicode_folder_names() {
        use std::os::unix::ffi::OsStrExt;
        let root = temp("non-unicode-config");
        let config = root.join(std::ffi::OsStr::from_bytes(b"config-\xff"));
        write(&config.join("fonts.conf"), "<fontconfig><dir>/configured/fonts</dir></fontconfig>");
        let mut env = env_in(&root);
        env.fontconfig_path = Some(config.into_os_string());
        assert_eq!(fontconfig_dirs(&env), [PathBuf::from("/configured/fonts")]);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[cfg(unix)]
    #[test]
    fn configuration_search_skips_unreadable_files() {
        use std::os::unix::fs::PermissionsExt;
        struct Cleanup {
            root: PathBuf,
            file: PathBuf,
            permissions: std::fs::Permissions,
        }
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = std::fs::set_permissions(&self.file, self.permissions.clone());
                let _ = std::fs::remove_dir_all(&self.root);
            }
        }
        let root = temp("unreadable-config");
        let first = root.join("first/fonts.conf");
        let second = root.join("second/fonts.conf");
        write(&first, "<fontconfig><dir>/unreadable/fonts</dir></fontconfig>");
        write(&second, "<fontconfig><dir>/readable/fonts</dir></fontconfig>");
        let _cleanup = Cleanup { root: root.clone(), file: first.clone(), permissions: std::fs::metadata(&first).unwrap().permissions() };
        std::fs::set_permissions(&first, std::fs::Permissions::from_mode(0o000)).unwrap();
        if std::fs::File::open(&first).is_ok() {
            eprintln!("skipped: this process can read mode-000 files");
            return;
        }
        let mut env = env_in(&root);
        env.fontconfig_roots = vec![first.clone(), second.clone()];
        env.fontconfig_path = Some(std::env::join_paths([first.parent().unwrap(), second.parent().unwrap()]).unwrap());
        assert_eq!(fontconfig_dirs(&env), [PathBuf::from("/readable/fonts")]);
        env.fontconfig_path = None;
        assert_eq!(fontconfig_dirs(&env), [PathBuf::from("/readable/fonts")]);
    }
}
