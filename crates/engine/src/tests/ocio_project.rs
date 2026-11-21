//! Project Settings ▸ Color ▸ OCIO Config: choosing a config, the studio default for new
//! projects, the `OCIO` environment variable's override, the working space from the config's
//! scene_linear role, and configs that can't be read.
//!
//! The configs are synthetic, written here: their matrices are derived from the working spaces'
//! published primaries (`ColorSpace::to_xyz`), and their names are deliberately unlike any real
//! config's, so a match can only be numerical.

use std::path::{Path, PathBuf};

use effectcraft_color::ColorSpace;
use effectcraft_color::space::{invert, mul};
use effectcraft_effects::ocio_config::Config;
use effectcraft_project::ColorEngine;
use serde_json::{Value, json};

use crate::{Event, Session, ocio_project};

fn tmp(name: &str) -> PathBuf {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let d = std::env::temp_dir().join(format!("ec-ocio-project-{}-{}-{name}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// The linear-light matrix from working space `ws` to ACES2065-1, as an OCIO 4×4 row list.
fn to_ap0(ws: ColorSpace) -> String {
    let m = mul(&invert(&ColorSpace::Aces2065.to_xyz()), &ws.to_xyz());
    let r = |i: usize| format!("{}, {}, {}, 0", m[i][0], m[i][1], m[i][2]);
    format!("[{}, {}, {}, 0, 0, 0, 1]", r(0), r(1), r(2))
}

/// A config whose scene_linear space is linear light in `ws`'s primaries, its reference
/// ACES2065-1; `interchange` adds the aces_interchange role.
fn config_text(ws: ColorSpace, interchange: bool) -> String {
    let role = if interchange { "  aces_interchange: Studio Archive\n" } else { "" };
    format!(
        "ocio_profile_version: 2\nname: synthetic test config\nroles:\n  scene_linear: Studio Working\n{role}  default: Studio Archive\n\
         colorspaces:\n  - !<ColorSpace>\n    name: Studio Archive\n  - !<ColorSpace>\n    name: Studio Working\n    \
         to_scene_reference: !<MatrixTransform> {{matrix: {}}}\n\
         displays:\n  Monitor A:\n    - !<View> {{name: Plain, colorspace: Studio Working}}\n\
         looks:\n  - !<Look>\n    name: Warm\n    process_space: Studio Working\n    transform: !<CDLTransform> {{slope: [1.1, 1, 0.9]}}\n",
        to_ap0(ws)
    )
}

fn write_config(dir: &Path, name: &str, ws: ColorSpace) -> String {
    let p = dir.join(name);
    std::fs::write(&p, config_text(ws, true)).unwrap();
    p.to_string_lossy().into_owned()
}

fn session() -> Session {
    let mut s = Session::default();
    s.execute("comp.new", json!({"width": 32, "height": 18, "duration": 1})).unwrap();
    s
}

fn toasts(s: &mut Session) -> Vec<String> {
    s.drain_events()
        .into_iter()
        .filter_map(|e| match e {
            Event::Toast { message, .. } => Some(message),
            _ => None,
        })
        .collect()
}

#[test]
fn scene_linear_is_identified_by_its_numbers_not_its_name() {
    let parse = |t: &str| Config::parse(t, Path::new(".")).unwrap();
    for ws in [ColorSpace::AcesCg, ColorSpace::Aces2065] {
        let (_, got) = ocio_project::resolve_working_space(&parse(&config_text(ws, true))).unwrap();
        assert_eq!(got, ws);
    }
    // Linear Rec. 709 (a Blender-style config): identified, and refused by name of its primaries.
    let e = ocio_project::resolve_working_space(&parse(&config_text(ColorSpace::Rec709, true))).unwrap_err();
    assert!(e.contains(ColorSpace::Rec709.label()) && e.contains(ColorSpace::AcesCg.label()), "{e}");
    // Without the aces_interchange role nothing anchors the numbers: refused, not guessed.
    let e = ocio_project::resolve_working_space(&parse(&config_text(ColorSpace::AcesCg, false))).unwrap_err();
    assert!(e.contains("aces_interchange"), "{e}");
}

#[test]
fn chosen_config_sets_the_working_space_and_survives_save_and_open() {
    let d = tmp("persist");
    let cfg = write_config(&d, "studio.ocio", ColorSpace::AcesCg);
    let mut s = session();
    s.execute("file.projectSettings", json!({"workingSpace": "rec709"})).unwrap();
    let r = s.execute("file.projectSettings", json!({"ocioConfig": cfg})).unwrap();
    let st = &s.project.settings;
    assert_eq!((st.color_engine, st.working_space, st.ocio_config.as_deref()), (ColorEngine::Ocio, Some(ColorSpace::AcesCg), Some(cfg.as_str())));
    assert_eq!(r["ocio"]["source"], "project");
    assert_eq!(r["ocio"]["looks"]["applied"], false, "listed, never presented as applied");
    assert!(r["ocio"]["looks"]["items"].as_array().is_some_and(|l| !l.is_empty()));
    // The form sends every field again: an unchanged config with another working space is refused.
    assert!(s.execute("file.projectSettings", json!({"ocioConfig": cfg, "workingSpace": "aces2065"})).is_err());
    let path = d.join("p.ecproj");
    s.execute("file.saveAs", json!({"path": path.to_string_lossy()})).unwrap();
    let mut o = Session::default();
    o.execute("file.open", json!({"path": path.to_string_lossy()})).unwrap();
    assert_eq!(o.project.settings, s.project.settings);
    // Clearing goes back to the built-in config.
    o.execute("file.projectSettings", json!({"ocioConfig": null})).unwrap();
    assert!(o.project.settings.ocio_config.is_none());
    let _ = std::fs::remove_dir_all(d);
}

#[test]
fn projects_without_a_config_save_and_load_as_before() {
    let d = tmp("old");
    let mut s = session();
    let path = d.join("p.ecproj");
    s.execute("file.saveAs", json!({"path": path.to_string_lossy()})).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(!text.contains("ocio"), "no new key in files without a config");
    let mut o = Session::default();
    o.execute("file.open", json!({"path": path.to_string_lossy()})).unwrap();
    assert!(o.project.settings.ocio_config.is_none());
    o.execute("file.save", json!({})).unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), text, "saved again byte for byte");
    let _ = std::fs::remove_dir_all(d);
}

#[test]
fn studio_default_seeds_new_projects_only() {
    let d = tmp("studio");
    let cfg = write_config(&d, "studio.ocio", ColorSpace::Aces2065);
    let mut s = session();
    let before = s.project.settings.clone();
    s.execute("prefs.set", json!({"key": "project.defaultOcioConfig", "value": cfg})).unwrap();
    assert_eq!(s.project.settings, before, "the open project keeps its settings");
    let r = s.execute("file.newProject", json!({})).unwrap();
    let st = &s.project.settings;
    assert_eq!((st.color_engine, st.working_space, st.ocio_config.as_deref()), (ColorEngine::Ocio, Some(ColorSpace::Aces2065), Some(cfg.as_str())));
    assert_eq!(r["ocio"]["source"], "studioDefault");
    assert!(!s.is_dirty(), "a new project is not modified by its defaults");
    // A studio default whose scene_linear isn't a working space leaves new projects as they were.
    let bad = write_config(&d, "linear709.ocio", ColorSpace::Rec709);
    s.execute("prefs.set", json!({"key": "project.defaultOcioConfig", "value": bad})).unwrap();
    toasts(&mut s);
    s.execute("file.newProject", json!({})).unwrap();
    assert_eq!(s.project.settings, effectcraft_project::ProjectSettings::default());
    assert!(!toasts(&mut s).is_empty(), "says why");
    let _ = std::fs::remove_dir_all(d);
}

#[test]
fn ocio_environment_variable_overrides_without_rewriting_the_project() {
    let d = tmp("env");
    let project_cfg = write_config(&d, "project.ocio", ColorSpace::AcesCg);
    let env_cfg = write_config(&d, "env.ocio", ColorSpace::Aces2065);
    let mut s = session();
    s.execute("file.projectSettings", json!({"ocioConfig": project_cfg})).unwrap();
    let path = d.join("p.ecproj");
    s.execute("file.saveAs", json!({"path": path.to_string_lossy()})).unwrap();
    let saved = s.project.settings.clone();

    let mut o = Session { ocio_env: Some(env_cfg.clone()), ..Default::default() };
    let r = o.execute("file.open", json!({"path": path.to_string_lossy()})).unwrap();
    assert_eq!(o.project.settings, saved, "opening never rewrites the project's colour settings");
    assert_eq!((r["ocio"]["source"].as_str(), r["ocio"]["path"].as_str()), (Some("env"), Some(env_cfg.as_str())));
    assert_eq!(r["ocio"]["workingSpace"], "aces2065");
    assert!(r["ocioNotice"].is_string(), "the mismatch with the saved working space is reported");
    // While the variable is set, the project's own choice is locked, as Nuke documents.
    assert!(o.execute("file.projectSettings", json!({"ocioConfig": null})).is_err());
    // A form that sends the same values again still applies its other fields.
    o.execute("file.projectSettings", json!({"ocioConfig": project_cfg, "bitDepth": 32})).unwrap();
    assert_eq!(o.project.settings.ocio_config, saved.ocio_config);
    // New projects use the variable's config too.
    o.execute("file.newProject", json!({})).unwrap();
    assert_eq!((o.project.settings.color_engine, o.project.settings.working_space), (ColorEngine::Ocio, Some(ColorSpace::Aces2065)));
    let _ = std::fs::remove_dir_all(d);
}

#[test]
fn unsupported_scene_linear_is_refused_and_changes_nothing() {
    let d = tmp("refuse");
    let cfg = write_config(&d, "linear709.ocio", ColorSpace::Rec709);
    let mut s = session();
    let before = s.project.settings.clone();
    let e = s.execute("file.projectSettings", json!({"ocioConfig": cfg})).unwrap_err().to_string();
    assert!(e.contains(ColorSpace::AcesCg.label()), "names what is supported: {e}");
    assert_eq!(s.project.settings, before);
    let _ = std::fs::remove_dir_all(d);
}

#[test]
fn a_missing_config_falls_back_without_failing() {
    let d = tmp("missing");
    let cfg = write_config(&d, "gone.ocio", ColorSpace::AcesCg);
    let mut s = session();
    s.execute("file.projectSettings", json!({"ocioConfig": cfg})).unwrap();
    let path = d.join("p.ecproj");
    s.execute("file.saveAs", json!({"path": path.to_string_lossy()})).unwrap();
    std::fs::remove_file(&cfg).unwrap();

    let mut o = Session::default();
    let r = o.execute("file.open", json!({"path": path.to_string_lossy()})).unwrap();
    assert_eq!(o.project.settings, s.project.settings, "the path is kept for when the file is back");
    assert_eq!((&r["ocio"]["loaded"], &r["ocio"]["fallback"]), (&json!(false), &json!("builtIn")));
    assert!(!toasts(&mut o).is_empty(), "the fallback is reported");
    // Other settings still change.
    o.execute("file.projectSettings", json!({"ocioConfig": cfg, "bitDepth": 16})).unwrap();
    assert_eq!(o.project.settings.bit_depth, effectcraft_project::BitDepth::Bpc16);
    // Choosing a missing file is kept too, on the built-in config, and says so.
    let other = d.join("not-there.ocio").to_string_lossy().into_owned();
    let r = o.execute("file.projectSettings", json!({"ocioConfig": other})).unwrap();
    assert!(r["notes"].as_array().is_some_and(|n| !n.is_empty()), "{r}");
    assert_eq!(r["ocio"]["loaded"], Value::Bool(false));
    let _ = std::fs::remove_dir_all(d);
}
