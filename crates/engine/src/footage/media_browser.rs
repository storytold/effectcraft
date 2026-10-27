//! Media Browser (Window ▸ Media Browser): browse for footage, keep favourite folders, read file
//! metadata, and import (`file.import`).
//!
//! What it browses is a [`Browser`]: the local file system on the desktop ([`FsBrowser`]); in the
//! web app, a virtual tree ([`VirtualTree`]) of the browser's storage and of folders the user
//! opened through the File System Access API, whose files are read only when imported
//! ([`Browser::fetch`]).
//!
//! Also the metadata the Metadata panel shows for project items and files (codec, size, rate,
//! duration, colour profile, dates).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use effectcraft_project::{Footage, FootageKind, Item, ItemKind, Project};

/// Media Browser state (in [`crate::EditorState`], serde for agents).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserState {
    /// The folder shown (`None` = the home folder).
    #[serde(default)]
    pub folder: Option<String>,
    /// Favourite folders.
    #[serde(default)]
    pub favorites: Vec<String>,
    /// Show only importable files.
    #[serde(default)]
    pub importable_only: bool,
}

/// Whether this build can browse the local file system ([`FsBrowser`]).
pub fn available() -> bool {
    !cfg!(target_arch = "wasm32")
}

/// Where the Media Browser browses.
pub trait Browser: Send + Sync {
    /// The folder shown first.
    fn home(&self) -> String;
    /// Places listed above the favourites: (name, folder). The default is Home.
    fn places(&self) -> Vec<(String, String)> {
        vec![("Home".into(), self.home())]
    }
    /// A folder's entries: folders first, then files, by name.
    fn list(&self, dir: &str, importable_only: bool) -> Result<Vec<Entry>, String>;
    /// The folder above `dir`.
    fn parent(&self, dir: &str) -> Option<String> {
        parent(dir)
    }
    /// Make `paths` readable by the importer. `false`: they are being fetched, and the host runs
    /// `mediaBrowser.import` with `params` again once they are (the web app reads opened folders'
    /// files on demand).
    fn fetch(&self, paths: &[String], params: &Value) -> bool {
        let _ = (paths, params);
        true
    }
    /// Extra toolbar actions: (id, label), run by `mediaBrowser.action`.
    fn actions(&self) -> Vec<(String, String)> {
        vec![]
    }
    fn action(&self, id: &str) -> Result<Value, String> {
        Err(format!("unknown Media Browser action `{id}`"))
    }
}

/// The local file system (desktop).
pub struct FsBrowser;

impl Browser for FsBrowser {
    fn home(&self) -> String {
        home()
    }
    fn list(&self, dir: &str, importable_only: bool) -> Result<Vec<Entry>, String> {
        list(dir, importable_only)
    }
}

impl crate::Session {
    /// What the Media Browser browses: [`crate::Session::browser`], else the local file system
    /// where this build has one.
    pub fn media_browser(&self) -> Option<std::sync::Arc<dyn Browser>> {
        self.browser.clone().or_else(|| available().then(|| std::sync::Arc::new(FsBrowser) as std::sync::Arc<dyn Browser>))
    }
}

/// One entry of a [`VirtualTree`].
#[derive(Clone, Debug, PartialEq)]
struct VNode {
    /// What importing it reads (`None`: a folder).
    import: Option<String>,
    size: u64,
    modified: Option<u64>,
}

/// A browsable tree of virtual paths (`/Folder/sub/file.mov`), each file standing for an
/// importable path (the web app's file table, or a file of an opened folder).
#[derive(Clone, Debug, Default)]
pub struct VirtualTree {
    nodes: BTreeMap<String, VNode>,
}

fn vparent(path: &str) -> Option<&str> {
    if path == "/" || path.is_empty() {
        return None;
    }
    let p = path.trim_end_matches('/');
    match p.rfind('/') {
        Some(0) => Some("/"),
        Some(i) => Some(&p[..i]),
        None => None,
    }
}

impl VirtualTree {
    /// Add a folder (and the folders above it).
    pub fn add_dir(&mut self, path: &str) {
        let mut cur = Some(path.trim_end_matches('/'));
        while let Some(p) = cur.filter(|p| !p.is_empty() && *p != "/") {
            self.nodes.entry(p.to_string()).or_insert(VNode { import: None, size: 0, modified: None });
            cur = vparent(p);
        }
    }

    /// Add a file shown at `path` that imports `import`.
    pub fn add_file(&mut self, path: &str, import: &str, size: u64, modified: Option<u64>) {
        if let Some(p) = vparent(path) {
            self.add_dir(p);
        }
        self.nodes.insert(path.to_string(), VNode { import: Some(import.to_string()), size, modified });
    }

    /// Remove `path` and everything under it.
    pub fn remove(&mut self, path: &str) {
        let pre = format!("{}/", path.trim_end_matches('/'));
        self.nodes.retain(|k, _| k != path && !k.starts_with(&pre));
    }

    pub fn is_dir(&self, path: &str) -> bool {
        path == "/" || self.nodes.get(path.trim_end_matches('/')).is_some_and(|n| n.import.is_none())
    }

    /// What a file shown at `path` imports.
    pub fn import_path(&self, path: &str) -> Option<&str> {
        self.nodes.get(path).and_then(|n| n.import.as_deref())
    }

    pub fn list(&self, dir: &str, importable_only: bool) -> Result<Vec<Entry>, String> {
        let dir = if dir.len() > 1 { dir.trim_end_matches('/') } else { dir };
        if !self.is_dir(dir) {
            return Err(format!("{dir}: no such folder"));
        }
        let mut out: Vec<Entry> = self
            .nodes
            .iter()
            .filter(|(k, _)| vparent(k) == Some(dir))
            .filter_map(|(k, n)| {
                let name = k.rsplit('/').next().unwrap_or(k).to_string();
                let kind = n.import.as_deref().and_then(kind_of);
                if importable_only && n.import.is_some() && kind.is_none() {
                    return None;
                }
                Some(Entry {
                    name,
                    // Folders browse by their virtual path; files import their own path.
                    path: n.import.clone().unwrap_or_else(|| k.clone()),
                    is_dir: n.import.is_none(),
                    size: n.size,
                    modified: n.modified,
                    kind,
                })
            })
            .collect();
        sort_entries(&mut out);
        Ok(out)
    }
}

/// One directory entry.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub size: u64,
    /// Seconds since the Unix epoch.
    pub modified: Option<u64>,
    /// Footage kind guessed from the extension (`video`, `image`, `audio`, `data`, `model`,
    /// `project`), `None` for other files and folders.
    pub kind: Option<&'static str>,
}

fn sort_entries(entries: &mut [Entry]) {
    entries.sort_by_cached_key(|e| (!e.is_dir, e.name.to_lowercase()));
}

const VIDEO: &[&str] = &["mp4", "m4v", "mov", "webm", "mkv", "avi", "mxf", "gif"];
const IMAGE: &[&str] = &["png", "jpg", "jpeg", "tif", "tiff", "exr", "bmp", "webp", "psd", "psb", "svg", "tga", "dpx", "cin", "hdr"];
const AUDIO: &[&str] = &["wav", "aif", "aiff", "mp3", "m4a", "aac", "flac", "ogg", "opus"];
const DATA: &[&str] = &["json", "csv", "tsv", "mgjson"];
const MODEL: &[&str] = &["gltf", "glb", "obj", "stl", "fbx"];

/// Kind of file from its extension.
pub fn kind_of(path: &str) -> Option<&'static str> {
    let ext = std::path::Path::new(path).extension()?.to_str()?.to_ascii_lowercase();
    let e = ext.as_str();
    if VIDEO.contains(&e) {
        Some("video")
    } else if IMAGE.contains(&e) {
        Some("image")
    } else if AUDIO.contains(&e) {
        Some("audio")
    } else if DATA.contains(&e) {
        Some("data")
    } else if MODEL.contains(&e) {
        Some("model")
    } else if matches!(e, "ecproj" | "ectemplate" | "json5") {
        Some("project")
    } else {
        None
    }
}

/// The user's home folder (or `/`).
pub fn home() -> String {
    std::env::var("HOME").or_else(|_| std::env::var("USERPROFILE")).unwrap_or_else(|_| "/".into())
}

fn unix(t: std::io::Result<std::time::SystemTime>) -> Option<u64> {
    t.ok()?.duration_since(std::time::UNIX_EPOCH).ok().map(|d| d.as_secs())
}

/// List a folder: folders first, then files, by name (hidden entries skipped).
pub fn list(dir: &str, importable_only: bool) -> Result<Vec<Entry>, String> {
    if !available() {
        return Err("the Media Browser needs the desktop app".into());
    }
    let rd = std::fs::read_dir(dir).map_err(|e| format!("{dir}: {e}"))?;
    let mut out: Vec<Entry> = rd
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            if name.starts_with('.') {
                return None;
            }
            let md = e.metadata().ok()?;
            let path = e.path().to_string_lossy().to_string();
            let kind = if md.is_dir() { None } else { kind_of(&path) };
            if importable_only && !md.is_dir() && kind.is_none() {
                return None;
            }
            Some(Entry { name, path, is_dir: md.is_dir(), size: if md.is_dir() { 0 } else { md.len() }, modified: unix(md.modified()), kind })
        })
        .collect();
    sort_entries(&mut out);
    Ok(out)
}

/// The parent folder of `dir`.
pub fn parent(dir: &str) -> Option<String> {
    std::path::Path::new(dir).parent().map(|p| p.to_string_lossy().to_string()).filter(|p| !p.is_empty())
}

/// Format seconds since the epoch as an ISO 8601 UTC date-time. Years far outside chrono's own
/// range are preserved (the old hand-rolled conversion kept them) rather than collapsing to the epoch.
pub fn iso_date(secs: u64) -> String {
    let (y, t) = utc_parts(secs);
    // `{:04}` rather than `%Y`, which writes a `+` before years past 9999.
    format!("{y:04}-{}", t.format("%m-%dT%H:%M:%SZ"))
}

/// Unix seconds → (proleptic Gregorian year, the same wall-clock instant moved into chrono's range
/// by whole 400-year cycles). The Gregorian calendar repeats every 400 years (146_097 days), so
/// month, day and time of day come from chrono and only the year is carried, which keeps every
/// input exact instead of collapsing out-of-range years to the epoch. `secs` is `u64`, so `days`
/// always fits an `i64`.
fn utc_parts(secs: u64) -> (i64, chrono::NaiveDateTime) {
    use chrono::Datelike;
    let (days, sod) = ((secs / 86_400) as i64, (secs % 86_400) as i64);
    let (cycles, rest) = (days.div_euclid(146_097), days.rem_euclid(146_097));
    // `rest + 719_163` (1970-01-01 is CE day 719_163) is always a valid CE day, and `sod` < 86_400.
    let date = i32::try_from(rest + 719_163).ok().and_then(chrono::NaiveDate::from_num_days_from_ce_opt);
    let time = u32::try_from(sod).ok().and_then(|s| chrono::NaiveTime::from_num_seconds_from_midnight_opt(s, 0));
    match (date, time) {
        (Some(d), Some(t)) => {
            let y = i128::from(d.year()) + i128::from(cycles) * 400;
            (i64::try_from(y).unwrap_or(i64::MAX), d.and_time(t))
        }
        _ => (1970, chrono::NaiveDateTime::default()), // unreachable
    }
}

/// File dates and size: (created, modified, size).
pub fn file_dates(path: &str) -> (Option<u64>, Option<u64>, Option<u64>) {
    if !available() || path.is_empty() {
        return (None, None, None);
    }
    match std::fs::metadata(path) {
        Ok(md) => (unix(md.created()), unix(md.modified()), Some(md.len())),
        Err(_) => (None, None, None),
    }
}

fn kind_name(k: FootageKind) -> &'static str {
    match k {
        FootageKind::Video => "video",
        FootageKind::Still => "still",
        FootageKind::Sequence => "sequence",
        FootageKind::Audio => "audio",
        FootageKind::Model => "model",
        FootageKind::Data => "data",
    }
}

/// Metadata of footage (the fields the Metadata panel lists).
pub fn footage_metadata(f: &Footage) -> Value {
    let (created, modified, size) = if f.missing { (None, None, None) } else { file_dates(f.sequence.first().map(String::as_str).unwrap_or(&f.path)) };
    json!({
        "path": f.path,
        "kind": kind_name(f.kind),
        "codec": f.codec,
        "width": f.width,
        "height": f.height,
        "pixelAspect": f.pixel_aspect,
        "frameRate": f.frame_rate.as_f64(),
        "duration": f.duration.seconds(),
        // Rounded like the media pool's frame count: Matroska stores the duration in whole
        // milliseconds, a hair short of the last frame at 30 fps (#1429).
        "frames": if f.has_video && f.kind != FootageKind::Still { Some(f.frame_rate.frame_at(f.frame_rate.snap_nearest(f.duration))) } else { None },
        "hasVideo": f.has_video,
        "hasAudio": f.has_audio,
        "alpha": format!("{:?}", f.alpha),
        "colorProfile": f.color_profile.map(|c| format!("{c:?}")).unwrap_or_else(|| "sRGB (default)".into()),
        "linearLight": f.linear_light,
        "preserveRgb": f.preserve_rgb,
        "fields": format!("{:?}", f.fields),
        "sequenceFiles": f.sequence.len(),
        "dataRows": f.data.as_ref().map(|d| d.lines().count().saturating_sub(1)),
        "created": created.map(iso_date),
        "modified": modified.map(iso_date),
        "fileSize": size,
        "missing": f.missing,
    })
}

/// Metadata of a project item.
pub fn item_metadata(p: &Project, it: &Item) -> Value {
    let mut v = json!({
        "id": it.id.0,
        "name": it.name,
        "type": it.type_name(),
        "comment": it.comment,
        "label": format!("{:?}", it.label),
    });
    let extra = match &it.kind {
        ItemKind::Footage(f) => footage_metadata(f),
        ItemKind::Comp(c) => json!({
            "width": c.width,
            "height": c.height,
            "pixelAspect": c.pixel_aspect,
            "frameRate": c.frame_rate.as_f64(),
            "duration": c.duration.seconds(),
            "layers": c.layers.len(),
            "workArea": [c.work_area.0.seconds(), c.work_area.1.seconds()],
        }),
        ItemKind::Solid(s) => json!({"width": s.width, "height": s.height, "color": s.color}),
        ItemKind::Folder => json!({"items": p.items.values().filter(|i| i.parent == Some(it.id)).count()}),
    };
    if let (Value::Object(a), Value::Object(b)) = (&mut v, extra) {
        a.extend(b);
    }
    if let Some(px) = &it.proxy {
        v["proxy"] = json!(px.footage.path);
    }
    v
}

/// Project-level metadata.
pub fn project_metadata(p: &Project, path: Option<&str>) -> Value {
    let st = &p.settings;
    let count = |f: &dyn Fn(&ItemKind) -> bool| p.items.values().filter(|i| f(&i.kind)).count();
    json!({
        "path": path,
        "comment": st.comment,
        "bitDepth": st.bit_depth.label(),
        "workingSpace": st.working_space.map(|c| format!("{c:?}")),
        "linearize": st.linearize,
        "timeDisplay": format!("{:?}", st.time_display),
        "audioSampleRate": st.audio_sample_rate,
        "items": p.items.len(),
        "compositions": count(&|k| matches!(k, ItemKind::Comp(_))),
        "footage": count(&|k| matches!(k, ItemKind::Footage(_))),
        "solids": count(&|k| matches!(k, ItemKind::Solid(_))),
        "folders": count(&|k| matches!(k, ItemKind::Folder)),
        "renderQueue": p.render_queue.len(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_a_folder() {
        let dir = std::env::temp_dir().join(format!("ec-mb-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("Shots")).unwrap();
        std::fs::write(dir.join("b.png"), b"x").unwrap();
        std::fs::write(dir.join("A.mov"), b"xy").unwrap();
        std::fs::write(dir.join("notes.txt"), b"xyz").unwrap();
        std::fs::write(dir.join(".hidden"), b"").unwrap();
        let d = dir.to_string_lossy().to_string();
        let all = list(&d, false).unwrap();
        let names: Vec<&str> = all.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["Shots", "A.mov", "b.png", "notes.txt"]);
        assert!(all[0].is_dir);
        assert_eq!(all[1].kind, Some("video"));
        assert_eq!(all[2].kind, Some("image"));
        assert_eq!(all[1].size, 2);
        let media = list(&d, true).unwrap();
        assert_eq!(media.len(), 3);
        assert_eq!(parent(&dir.join("Shots").to_string_lossy()).as_deref(), Some(d.as_str()));
        assert!(list(&dir.join("nope").to_string_lossy(), false).is_err());
        std::fs::write(dir.join("Ω.csv"), b"x").unwrap();
        std::fs::write(dir.join("作品.csv"), b"x").unwrap();
        let all = list(&d, false).unwrap();
        assert_eq!(all.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(), ["Shots", "A.mov", "b.png", "notes.txt", "Ω.csv", "作品.csv"]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn virtual_tree_lists_like_a_folder() {
        let mut t = VirtualTree::default();
        t.add_file("/Browser Storage/clip.mov", "/clip.mov", 10, Some(5));
        t.add_file("/Folders/Shoot/B/b.png", "/Folders/Shoot/B/b.png", 3, None);
        t.add_file("/Folders/Shoot/a.wav", "/Folders/Shoot/a.wav", 4, None);
        t.add_file("/Folders/Shoot/notes.txt", "/Folders/Shoot/notes.txt", 1, None);
        t.add_dir("/Folders/Empty");
        let names = |t: &VirtualTree, d: &str, only: bool| t.list(d, only).unwrap().into_iter().map(|e| e.name).collect::<Vec<_>>();
        assert_eq!(names(&t, "/", false), ["Browser Storage", "Folders"]);
        assert_eq!(names(&t, "/Folders", false), ["Empty", "Shoot"]);
        assert_eq!(names(&t, "/Folders/Shoot", false), ["B", "a.wav", "notes.txt"]);
        assert_eq!(names(&t, "/Folders/Shoot/", true), ["B", "a.wav"]);
        let e = &t.list("/Browser Storage", false).unwrap()[0];
        assert_eq!((e.path.as_str(), e.size, e.modified, e.kind, e.is_dir), ("/clip.mov", 10, Some(5), Some("video"), false));
        let d = &t.list("/Folders", false).unwrap()[1];
        assert_eq!((d.path.as_str(), d.is_dir), ("/Folders/Shoot", true));
        assert_eq!(t.import_path("/Browser Storage/clip.mov"), Some("/clip.mov"));
        assert!(t.list("/Folders/Shoot/a.wav", false).is_err(), "a file is not a folder");
        assert!(t.list("/Nope", false).is_err());
        t.remove("/Folders/Shoot");
        assert_eq!(names(&t, "/Folders", false), ["Empty"]);
        assert_eq!(vparent("/a"), Some("/"));
        assert_eq!(vparent("/"), None);
    }

    #[test]
    fn listings_keep_folders_first_and_stable_unicode_order() {
        let mut tree = VirtualTree::default();
        for name in ["作品.csv", "ä.csv", "Ä.csv", "İ.csv", "B.csv", "a.csv", "A.csv"] {
            tree.add_file(&format!("/Shoot/{name}"), &format!("/files/{name}"), 1, None);
        }
        for name in ["zFolder", "Alpha"] {
            tree.add_dir(&format!("/Shoot/{name}"));
        }
        let entries = tree.list("/Shoot", true).unwrap();
        assert_eq!(
            entries.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(),
            ["Alpha", "zFolder", "A.csv", "a.csv", "B.csv", "İ.csv", "Ä.csv", "ä.csv", "作品.csv"]
        );
        assert_eq!(entries[2].path, "/files/A.csv");
        let mut reversed = entries.iter().rev().cloned().collect::<Vec<_>>();
        sort_entries(&mut reversed);
        assert_eq!(
            reversed.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(),
            ["Alpha", "zFolder", "a.csv", "A.csv", "B.csv", "İ.csv", "ä.csv", "Ä.csv", "作品.csv"]
        );
    }

    #[test]
    #[ignore = "Media Browser filename sorting benchmark"]
    fn directory_sort_benchmark() {
        let entries: Vec<_> = (0..1000)
            .map(|i| Entry {
                name: format!("作品 {} {:04}.mov", if i % 2 == 0 { "Clip" } else { "clip" }, i * 7919 % 1000),
                path: format!("/Shoot/{i}.mov"),
                is_dir: i % 7 == 0,
                size: 1,
                modified: None,
                kind: Some("video"),
            })
            .collect();
        let mut before = Vec::new();
        let mut after = Vec::new();
        for _ in 0..5 {
            let mut old = entries.clone();
            let start = std::time::Instant::now();
            old.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
            before.push(start.elapsed());
            let mut new = entries.clone();
            let start = std::time::Instant::now();
            sort_entries(&mut new);
            after.push(start.elapsed());
            assert_eq!(old, new);
            std::hint::black_box(new);
        }
        before.sort();
        after.sort();
        println!("1,000 filename sort, median: {:?} -> {:?}", before[2], after[2]);
    }

    #[test]
    fn iso_dates() {
        assert_eq!(iso_date(0), "1970-01-01T00:00:00Z");
        assert_eq!(iso_date(1_700_000_000), "2023-11-14T22:13:20Z");
        assert_eq!(iso_date(951_782_400), "2000-02-29T00:00:00Z");
        // Years far outside chrono's own range are preserved (the old hand-rolled conversion kept
        // them) rather than collapsing to the epoch.
        assert_eq!(iso_date(253_402_300_800), "10000-01-01T00:00:00Z");
        assert_eq!(iso_date(10_000_000_000_000), "318857-05-20T17:46:40Z");
        assert_eq!(iso_date(u64::MAX), "584554051223-11-09T07:00:15Z");
    }
}
