//! File ▸ Import ▸ After Effects Project… through the command registry, with the synthetic
//! fixtures of `effectcraft-aep`.

use serde_json::json;

use crate::Session;

/// The test projects of `effectcraft-aep` and their cases.
#[path = "../../../aep/tests/it/common/mod.rs"]
mod common;

/// The path of a test project of `effectcraft-aep` (in the folder named by
/// `EFFECTCRAFT_AEP_FIXTURES`), or `None` after printing the skip line when it is missing. A case
/// of a combined project is written as a project of its own (see `common`), named like the case,
/// to a temporary folder.
fn fixture_path(name: &str) -> Option<String> {
    match common::locate(name)? {
        common::Source::File(path) => Some(path.to_string_lossy().to_string()),
        common::Source::Case { .. } => {
            // Written once per test process: tests running in parallel read the same case, and a
            // rewrite while another test reads it would hand that test a partial file.
            static WRITTEN: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, String>>> = std::sync::OnceLock::new();
            let mut written = WRITTEN.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner());
            if let Some(path) = written.get(name) {
                return Some(path.clone());
            }
            let bytes = common::try_fixture(name)?;
            let dir = std::env::temp_dir().join(format!("effectcraft-aep-cases-{}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            let path = dir.join(name);
            std::fs::write(&path, bytes).unwrap();
            let path = path.to_string_lossy().to_string();
            written.insert(name.to_string(), path.clone());
            Some(path)
        }
    }
}

/// The path of a whole file of the fixture folder, or `None` after printing the skip line.
fn file_path(name: &str) -> Option<String> {
    let path = common::dir().map(|d| d.join(name));
    if let Some(path) = path.filter(|p| p.is_file()) {
        Some(path.to_string_lossy().to_string())
    } else {
        common::skipped(name);
        None
    }
}
/// [`fixture_path`]; returns from the calling test when the file is missing.
macro_rules! fixture {
    ($name:expr) => {
        match fixture_path($name) {
            Some(path) => path,
            None => return,
        }
    };
}

#[test]
fn import_aep_reads_the_xml_form_like_the_binary_one() {
    // The pair holds cases 000, 058 (two Opacity keys), 060, 110 and 124 (with its precomposition),
    // saved by After Effects as .aep and right after as .aepx.
    let Some(xml) = file_path("aepx-pair.aepx") else { return };
    let Some(bin) = file_path("aepx-pair.aep") else { return };
    let mut s = Session::default();
    let x = s.execute_checked("file.importAep", json!({"path": xml})).unwrap();
    let mut t = Session::default();
    let b = t.execute_checked("file.importAep", json!({"path": bin})).unwrap();
    assert_eq!(x["report"], b["report"]);
    assert_eq!(x["report"]["imported"]["keyframes"], 2);
    let folder = effectcraft_project::ItemId(x["folder"].as_u64().unwrap());
    assert_eq!(s.project.item(folder).unwrap().name, "aepx-pair.aepx");
    let comps = |r: &serde_json::Value| r["comps"].as_array().unwrap().iter().map(|c| effectcraft_project::ItemId(c.as_u64().unwrap())).collect::<Vec<_>>();
    let (cx, cb) = (comps(&x), comps(&b));
    assert_eq!(cx.len(), 6);
    assert_eq!(cx.len(), cb.len());
    for (a, b) in cx.iter().zip(&cb) {
        assert_eq!(s.project.item(*a).unwrap().name, t.project.item(*b).unwrap().name);
        assert_eq!(s.project.comp(*a), t.project.comp(*b));
    }
}

#[test]
fn import_aep_creates_the_project_items_and_is_undoable() {
    let mut s = Session::default();
    let r = s.execute_checked("file.importAep", json!({"path": fixture!("055-key-position-3key-inserted-t2.5.aep")})).unwrap();
    assert_eq!(r["report"]["imported"]["compositions"], 1);
    assert_eq!(r["report"]["imported"]["keyframes"], 3);
    assert_eq!(r["report"]["losses"], json!([]));
    let cid = effectcraft_project::ItemId(r["comps"][0].as_u64().unwrap());
    // The wrapper folder is named after the file; the comp opens.
    let folder = effectcraft_project::ItemId(r["folder"].as_u64().unwrap());
    assert_eq!(s.project.item(folder).unwrap().name, "055-key-position-3key-inserted-t2.5.aep");
    assert_eq!(s.state.active_comp, Some(cid));
    let c = s.project.comp(cid).unwrap();
    assert_eq!((c.width, c.height, c.layers.len()), (1920, 1080, 1));
    let pos = c.layers[0].props.prop("transform/position").unwrap();
    assert_eq!(pos.keys.len(), 3);
    assert_eq!(pos.value_at(effectcraft_time::Tick(effectcraft_time::TICKS_PER_SECOND)), effectcraft_keyframe::Value::Vec3([100.0, 0.0, 0.0]));

    // One undo step removes everything the import added; redo brings it back.
    s.execute("edit.undo", json!({})).unwrap();
    assert!(s.project.items.is_empty());
    s.execute("edit.redo", json!({})).unwrap();
    assert!(s.project.comp(cid).is_some());
}

#[test]
fn losses_are_refused_unless_allowed() {
    let mut s = Session::default();
    let path = fixture!("077-fx-dropdown-default.aep");
    let err = s.execute_checked("file.importAep", json!({"path": path})).unwrap_err().to_string();
    assert!(err.contains("effects: 1") && err.contains("allowLosses"), "{err}");
    assert!(s.project.items.is_empty());
    assert!(s.history.undo.is_empty());

    // A dry run reports without importing.
    let r = s.execute_checked("file.importAep", json!({"path": path, "dryRun": true})).unwrap();
    assert_eq!(r["dryRun"], true);
    assert_eq!(r["report"]["losses"][0]["category"], "effects");
    assert_eq!(r["report"]["losses"][0]["count"], 1);
    assert!(s.project.items.is_empty());

    let r = s.execute_checked("file.importAep", json!({"path": path, "allowLosses": true})).unwrap();
    assert_eq!(r["report"]["losses"][0]["category"], "effects");
    assert!(!s.project.items.is_empty());
    s.execute("edit.undo", json!({})).unwrap();
    assert!(s.project.items.is_empty());
}

#[test]
fn bad_input_is_an_error_not_a_crash() {
    let mut s = Session::default();
    assert!(s.execute_checked("file.importAep", json!({})).is_err());
    assert!(s.execute_checked("file.importAep", json!({"path": "does/not/exist.aep"})).is_err());
    assert!(s.execute_checked("file.importAep", json!({"path": fixture!("000-base.aep"), "bogus": 1})).is_err());
    let dir = std::env::temp_dir().join(format!("effectcraft-aep-tests-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let junk = dir.join("junk.aep");
    std::fs::write(&junk, b"this is not an aep").unwrap();
    let err = s.execute_checked("file.importAep", json!({"path": junk.to_string_lossy()})).unwrap_err().to_string();
    assert!(err.contains("RIFX"), "{err}");
    let cut = dir.join("cut.aep");
    let bytes = std::fs::read(fixture!("000-base.aep")).unwrap();
    std::fs::write(&cut, &bytes[..bytes.len() / 2]).unwrap();
    assert!(s.execute_checked("file.importAep", json!({"path": cut.to_string_lossy(), "allowLosses": true})).is_err());
    assert!(s.project.items.is_empty());
}

#[test]
fn aep_menu_entry_exists() {
    let entries = crate::menus::entries();
    let find = |cmd: &str| entries.iter().find(|(_, e)| e.command == cmd).map(|(p, e)| (p.join(" > "), e.label.clone()));
    assert_eq!(find("file.importAep"), Some(("File > Import".into(), "After Effects Project...".into())));
}

#[test]
fn import_aep_takes_the_project_settings_into_an_empty_project_only() {
    use effectcraft_project::{BitDepth, TimeDisplayStyle};
    // The settings cases share files (one value per project): 621's file has 8 bits per channel,
    // working space None and frames (with 623 and 629); 632's and 633's file has 32 bits and feet
    // + frames 35mm.
    let mut s = Session::default();
    std::sync::Arc::make_mut(&mut s.project).settings.bit_depth = BitDepth::Bpc16;
    s.execute_checked("file.importAep", json!({"path": fixture!("621-settings-bpc-8.aep")})).unwrap();
    assert_eq!(s.project.settings.bit_depth, BitDepth::Bpc8);
    assert_eq!(s.project.settings.working_space, None);
    // A second file into a project with compositions leaves the settings alone ...
    s.execute_checked("file.importAep", json!({"path": fixture!("632-settings-linearize-and-blend-linear.aep")})).unwrap();
    assert_eq!((s.project.settings.bit_depth, s.project.settings.time_display), (BitDepth::Bpc8, TimeDisplayStyle::Frames));
    // ... unless asked; and undo restores them.
    s.execute_checked("file.importAep", json!({"path": fixture!("633-settings-feet-frames-35.aep"), "applySettings": true})).unwrap();
    assert_eq!(s.project.settings.time_display, TimeDisplayStyle::Feet35);
    assert_eq!(s.project.settings.bit_depth, BitDepth::Bpc32);
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(s.project.settings.time_display, TimeDisplayStyle::Frames);
    assert_eq!(s.project.settings.bit_depth, BitDepth::Bpc8);
}

/// File access that records every path it is asked about.
#[derive(Default)]
struct Recording(std::sync::Mutex<Vec<String>>);

impl crate::Services for Recording {
    fn read_file(&self, path: &str) -> std::io::Result<Vec<u8>> {
        self.0.lock().unwrap().push(path.to_string());
        std::fs::read(path)
    }
    fn write_file(&self, path: &str, data: &[u8]) -> std::io::Result<()> {
        std::fs::write(path, data)
    }
    fn exists(&self, path: &str) -> bool {
        self.0.lock().unwrap().push(path.to_string());
        std::path::Path::new(path).exists()
    }
    fn stamp(&self, path: &str) -> Option<(u64, u128)> {
        self.0.lock().unwrap().push(path.to_string());
        None
    }
}

/// `bytes` with the footage path of every file item (`Als2/alas`, key `fullpath`) set to `path`.
fn with_footage_path(bytes: &[u8], path: &str) -> Vec<u8> {
    fn set(chunks: &mut [effectcraft_aep::Chunk<'_>], value: &str, n: &mut usize) {
        for c in chunks {
            if c.id == *b"alas" {
                let json = c.bytes().unwrap().to_vec();
                let key = b"\"fullpath\":\"";
                let at = json.windows(key.len()).position(|w| w == key).unwrap() + key.len();
                let mut end = at;
                while json[end] != b'"' {
                    end += if json[end] == b'\\' { 2 } else { 1 };
                }
                let edited = [&json[..at], value.as_bytes(), &json[end..]].concat();
                *c = effectcraft_aep::Chunk::data(*b"alas", edited);
                *n += 1;
            } else if let effectcraft_aep::Body::List { children, .. } | effectcraft_aep::Body::Chunks(children) = &mut c.body {
                set(children, value, n);
            }
        }
    }
    let mut file = effectcraft_aep::parse(bytes).unwrap();
    let quoted = serde_json::to_string(path).unwrap();
    let mut n = 0;
    set(&mut file.chunks, &quoted[1..quoted.len() - 1], &mut n);
    assert!(n > 0, "no footage path");
    effectcraft_aep::write(&file).unwrap()
}

#[test]
fn stored_footage_paths_follow_the_document_path_policy() {
    // An .aep whose footage points to another computer or to a file beside it, imported from a
    // folder that is not the working directory: a foreign path is never looked at (not even by the
    // dry run) and arrives missing, the relative one is taken from the .aep's folder.
    use effectcraft_project::{Footage, ItemKind};
    let Some(bytes) = common::try_fixture("126-footage-png-layer.aep") else { return };
    let root = std::env::temp_dir().join(format!("ec-aep-doc-path-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let dir = root.join("job");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("clip.png"), b"x").unwrap();
    let local = root.to_string_lossy().into_owned();
    for (name, stored) in [("unc", r"\\192.0.2.1\share\x.png"), ("url", "file://192.0.2.1/s/x.png"), ("beside", "clip.png")] {
        let aep = dir.join(format!("{name}.aep"));
        std::fs::write(&aep, with_footage_path(&bytes, stored)).unwrap();
        let rec = std::sync::Arc::new(Recording::default());
        let mut s = Session { services: rec.clone(), ..Default::default() };
        s.execute("file.importAep", json!({"path": aep.to_string_lossy(), "allowLosses": true, "dryRun": true})).unwrap();
        s.execute("file.importAep", json!({"path": aep.to_string_lossy(), "allowLosses": true})).unwrap();
        let footage: Footage = s
            .project
            .items
            .values()
            .find_map(|i| match &i.kind {
                ItemKind::Footage(f) => Some(f.clone()),
                _ => None,
            })
            .unwrap_or_else(|| panic!("{name}: no footage"));
        let beside = name == "beside";
        assert_eq!(footage.missing, !beside, "{name}");
        if beside {
            assert_eq!(std::path::Path::new(&footage.path), dir.join("clip.png"));
        } else {
            // The session follows the imported path as stored in the .aep.
            assert!(s.stored_path(&footage.path).is_err(), "{name}");
        }
        let r = s.execute("footage.check", json!({"wait": true})).unwrap();
        assert_eq!(r["missing"].as_u64(), Some(u64::from(!beside)), "{name}: {r}");
        for p in rec.0.lock().unwrap().iter() {
            assert!(p.starts_with(&local), "{name}: looked at {p}");
        }
    }
    let _ = std::fs::remove_dir_all(&root);
}

/// An importer that finds sound in every video it probes.
struct WithSound;

impl crate::Importer for WithSound {
    fn probe(&self, path: &str) -> Result<effectcraft_project::Footage, String> {
        Ok(effectcraft_project::Footage {
            path: path.into(),
            kind: effectcraft_project::FootageKind::Video,
            has_video: true,
            has_audio: true,
            ..Default::default()
        })
    }
}

#[test]
fn a_video_with_sound_arrives_audible() {
    // The clip of the 645 project stores no sample rate (it has no sound there); the file next to
    // it is asked: with sound in it the imported footage has audio and its layer is part of the
    // mix; a missing file is left as the project file says.
    use effectcraft_project::{Footage, ItemKind};
    let Some(bytes) = common::try_fixture("645-footage-mp4-conform-30.aep") else { return };
    let root = std::env::temp_dir().join(format!("ec-aep-video-sound-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("clip.mp4"), b"x").unwrap();
    for (name, stored) in [("beside", "clip.mp4"), ("absent", "gone.mp4")] {
        let aep = root.join(format!("{name}.aep"));
        std::fs::write(&aep, with_footage_path(&bytes, stored)).unwrap();
        let mut s = Session { importer: Some(std::sync::Arc::new(WithSound)), ..Default::default() };
        s.execute("file.importAep", json!({"path": aep.to_string_lossy(), "allowLosses": true})).unwrap();
        let footage: Footage = s
            .project
            .items
            .values()
            .find_map(|i| match &i.kind {
                ItemKind::Footage(f) if f.kind == effectcraft_project::FootageKind::Video => Some(f.clone()),
                _ => None,
            })
            .unwrap_or_else(|| panic!("{name}: no video footage"));
        assert_eq!((footage.missing, footage.has_audio), (name == "absent", name == "beside"), "{name}");
        let audible = s.project.comps().any(|(id, _)| effectcraft_render::audio::comp_has_audio(&s.project, *id));
        assert_eq!(audible, name == "beside", "{name}: audible mix");
    }
    let _ = std::fs::remove_dir_all(&root);
}
