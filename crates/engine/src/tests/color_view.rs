//! Viewer and colour completion (M7.7): View ▸ Switch 3D View ▸ Default, Split with New Locked
//! Viewer, Use Display Color Management, Simulate Output; Project Settings ▸ Color Engine,
//! ACES working spaces, HDR compand / tone mapping, Rec. 2100 output, Feet + Frames.

use effectcraft_project::{ColorEngine, ColorSpace, HdrMode, Project, TimeDisplayStyle};
use effectcraft_render::RenderOpts;
use effectcraft_render::three_d::View3D;
use effectcraft_time::Tick;
use serde_json::json;

use crate::Session;
use crate::viewer::{DisplayColor, SimProfile, Simulation};

fn session() -> Session {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "C", "width": 64, "height": 48, "frameRate": 24, "duration": 10})).unwrap();
    s
}

#[test]
fn default_3d_view_ignores_camera_layers() {
    let mut s = session();
    let cid = s.active_comp_id().unwrap();
    s.execute("layer.newCamera", json!({"position": [32, 24, -300], "poi": [32, 24, 0]})).unwrap();
    assert_eq!(s.execute("view.3d.default", json!({})).unwrap()["view"], "default");
    let vs = s.state.views3d.get(&cid).unwrap().clone();
    assert_eq!(vs.current, View3D::Default);
    // The comp's default camera: at −zoom looking at the centre, not the camera layer.
    let cam = s.view_camera(cid).unwrap();
    let zoom = effectcraft_geom::default_camera_zoom(64.0);
    assert!((cam.eye.z + zoom).abs() < 1e-9 && (cam.eye.x - 32.0).abs() < 1e-9 && !cam.ortho, "{:?}", cam.eye);
    // Orbitable like a custom view; Reset 3D View restores it; Last view goes back.
    s.execute("camera.orbit", json!({"yaw": 30})).unwrap();
    assert!((s.view_camera(cid).unwrap().eye.x - 32.0).abs() > 1.0);
    s.execute("view.reset3DView", json!({})).unwrap();
    assert!((s.view_camera(cid).unwrap().eye.x - 32.0).abs() < 1e-9);
    s.execute("view.3d.last", json!({})).unwrap();
    assert_eq!(s.state.views3d[&cid].current, View3D::ActiveCamera);
    assert_eq!(s.execute("view.set3DView", json!({"view": "Default"})).unwrap()["view"], "default");
}

#[test]
fn locked_viewer_keeps_its_comp() {
    let mut s = session();
    let a = s.active_comp_id().unwrap();
    s.execute("view.3d.top", json!({})).unwrap();
    assert!(!s.is_enabled("view.closeLockedViewer"));
    let r = s.execute("view.splitLockedViewer", json!({})).unwrap();
    assert_eq!(r, json!({"comp": a.0, "view": "top"}));
    // Another comp becomes active; the locked viewer stays on the first.
    s.execute("comp.new", json!({"name": "B"})).unwrap();
    assert_ne!(s.active_comp_id(), Some(a));
    assert_eq!(s.state.locked_viewer.unwrap().comp, a);
    let st = s.execute("view.displayColor", json!({})).unwrap();
    assert_eq!(st["lockedViewer"]["comp"], a.0);
    // Serde of the editor state; explicit comp and view.
    let back: crate::EditorState = serde_json::from_value(serde_json::to_value(&s.state).unwrap()).unwrap();
    assert_eq!(back.locked_viewer, s.state.locked_viewer);
    s.execute("view.splitLockedViewer", json!({"comp": a.0, "view": "custom2"})).unwrap();
    assert_eq!(s.state.locked_viewer.unwrap().view, View3D::Custom2);
    assert!(s.execute("view.splitLockedViewer", json!({"view": "sideways"})).is_err());
    assert_eq!(s.execute("view.closeLockedViewer", json!({})).unwrap()["closed"], true);
    assert!(s.state.locked_viewer.is_none());
}

fn px(r: u8, g: u8, b: u8) -> Vec<[u8; 4]> {
    vec![[r, g, b, 255]]
}

#[test]
fn display_color_management_and_simulation() {
    let mut s = session();
    // Unmanaged projects: nothing to convert.
    assert!(DisplayColor::of(&s).is_none());
    s.execute("file.projectSettings", json!({"workingSpace": "rec2020"})).unwrap();
    // Managed, sRGB display, no simulation: the frame is already display-referred.
    assert!(DisplayColor::of(&s).is_none());
    assert_eq!(crate::menus::checked(&s, "view.displayColorManagement", &json!({})), Some(true));
    // A Display P3 monitor: pure sRGB red becomes less saturated P3 numbers.
    s.execute("prefs.set", json!({"key": "previews.displayProfile", "value": "p3"})).unwrap();
    let mut p = px(255, 0, 0);
    DisplayColor::of(&s).unwrap().apply(&mut p);
    assert!((p[0][0] as i32 - 234).abs() <= 2 && (p[0][1] as i32 - 51).abs() <= 3 && (p[0][2] as i32 - 35).abs() <= 3, "{p:?}");
    // Display colour management off: the working space's (Rec. 2020) numbers go to the screen.
    s.execute("view.displayColorManagement", json!({})).unwrap();
    assert_eq!(crate::menus::checked(&s, "view.displayColorManagement", &json!({})), Some(false));
    let mut q = px(255, 0, 0);
    DisplayColor::of(&s).unwrap().apply(&mut q);
    assert!(q[0][0] < 230 && q[0][1] > 60, "raw Rec. 2020 numbers: {q:?}");
    s.execute("view.displayColorManagement", json!({"value": true})).unwrap();
    s.execute("prefs.set", json!({"key": "previews.displayProfile", "value": "srgb"})).unwrap();
    // Simulate Output ▸ Linear with Preserve RGB: linear numbers shown as-is (darker mid-grey).
    s.execute("view.simulateOutput", json!({"profile": "linear", "preserveRgb": true})).unwrap();
    let mut g = px(128, 128, 128);
    DisplayColor::of(&s).unwrap().apply(&mut g);
    assert!((g[0][0] as i32 - 55).abs() <= 2, "{g:?}");
    // Without Preserve RGB the round trip through 8-bit linear bands the shadows.
    s.execute("view.simulateOutput", json!({"profile": "linear"})).unwrap();
    let dc = DisplayColor::of(&s).unwrap();
    let levels: std::collections::BTreeSet<u8> = (0..32u8)
        .map(|v| {
            let mut p = px(v, v, v);
            dc.apply(&mut p);
            p[0][0]
        })
        .collect();
    assert!(levels.len() < 20, "banding: {levels:?}");
    assert_eq!(crate::menus::checked(&s, "view.simulateOutput", &json!({"profile": "linear"})), Some(true));
    // Rec. 709 simulation keeps in-gamut colours; legacy Mac gamma 1.8 with Preserve RGB darkens.
    s.execute("view.simulateOutput", json!({"profile": "rec709"})).unwrap();
    let mut c = px(200, 100, 50);
    DisplayColor::of(&s).unwrap().apply(&mut c);
    assert!((c[0][0] as i32 - 200).abs() <= 2 && (c[0][1] as i32 - 100).abs() <= 2, "{c:?}");
    s.execute("view.simulateOutput", json!({"profile": "mac18", "preserveRgb": true})).unwrap();
    let mut m = px(128, 128, 128);
    DisplayColor::of(&s).unwrap().apply(&mut m);
    assert!(m[0][0] < 120, "gamma 1.8 numbers on an sRGB display look darker: {m:?}");
    // Custom… uses (and sets) the custom profile.
    let r = s.execute("view.simulateOutput", json!({"profile": "custom", "space": "p3", "preserveRgb": false})).unwrap();
    assert_eq!(r["profile"], "p3");
    assert_eq!(s.state.viewer.custom_simulation, Simulation { profile: SimProfile::P3, preserve_rgb: false });
    assert_eq!(crate::menus::checked(&s, "view.simulateOutput", &json!({"profile": "custom"})), Some(true));
    s.execute("view.simulateOutput", json!({"profile": "none"})).unwrap();
    assert!(DisplayColor::of(&s).is_none());
    assert!(s.execute("view.simulateOutput", json!({"profile": "cmyk"})).is_err());
    // Every menu profile parses.
    for p in SimProfile::ALL {
        assert_eq!(SimProfile::parse(p.id()), Some(p));
    }
}

fn red_solid(s: &mut Session, hex: &str) {
    s.execute("layer.newSolid", json!({"color": hex, "width": 64, "height": 48})).unwrap();
}

#[test]
fn color_engine_aces_working_spaces_and_hdr() {
    let mut s = session();
    let cid = s.active_comp_id().unwrap();
    // OCIO engine: ACEScg working space and the tone-mapped display by default.
    s.execute("file.projectSettings", json!({"colorEngine": "ocio"})).unwrap();
    let st = s.project.settings.clone();
    assert_eq!((st.color_engine, st.working_space, st.hdr), (ColorEngine::Ocio, Some(ColorSpace::AcesCg), HdrMode::ToneMap));
    assert!(s.execute("file.projectSettings", json!({"workingSpace": "rec709"})).is_err(), "not in the OCIO config");
    s.execute("file.projectSettings", json!({"workingSpace": "aces2065"})).unwrap();
    assert!(s.execute("file.projectSettings", json!({"workingSpace": "rec2100pq"})).is_err(), "not a working space");
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(s.project.settings.working_space, Some(ColorSpace::AcesCg));
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(s.project.settings.color_engine, ColorEngine::Adobe);
    s.execute("edit.redo", json!({})).unwrap();
    assert_eq!(s.project.settings.color_engine, ColorEngine::Ocio);
    // 32 bpc, a solid of 4.0 (over-range) in ACEScg: tone mapped below white, not clipped.
    s.execute("file.projectSettings", json!({"bitDepth": 32, "hdr": "toneMap"})).unwrap();
    s.execute("layer.newSolid", json!({"color": "#ffffff", "width": 64, "height": 48})).unwrap();
    let lid = s.active_comp().unwrap().layers[0].id.0;
    s.execute("effect.apply", json!({"layers": [lid], "effect": "Exposure"})).unwrap();
    s.execute("prop.set", json!({"layer": lid, "path": "effects/#1/master/exposure", "value": 2.0})).unwrap();
    let mapped = s.render(cid, Tick::ZERO, RenderOpts::default()).data[0];
    assert!(mapped[0] > 0.5 && mapped[0] < 0.99, "tone mapped: {mapped:?}");
    s.execute("file.projectSettings", json!({"hdr": "clip"})).unwrap();
    let clipped = s.render(cid, Tick::ZERO, RenderOpts::default()).data[0];
    assert!(clipped[0] > 1.1, "32 bpc keeps over-range: {clipped:?}");
    s.execute("file.projectSettings", json!({"hdr": "compand"})).unwrap();
    let comp = s.render(cid, Tick::ZERO, RenderOpts::default()).data[0];
    assert!(comp[0] > 0.9 && comp[0] < 1.0, "companded: {comp:?}");
    // Rec. 2100 PQ output: HDR, no tone mapping: over-range white encodes above SDR white's 0.58.
    s.execute("file.projectSettings", json!({"outputSpace": "rec2100pq", "hdr": "toneMap"})).unwrap();
    let pq = s.render(cid, Tick::ZERO, RenderOpts::default()).data[0];
    assert!(pq[0] > 0.6 && pq[0] < 0.8, "PQ: {pq:?}");
    s.execute("file.projectSettings", json!({"outputSpace": "rec2100hlg"})).unwrap();
    let hlg = s.render(cid, Tick::ZERO, RenderOpts::default()).data[0];
    assert!(hlg[0] > 0.85, "HLG: {hlg:?}");
    assert!(s.execute("file.projectSettings", json!({"hdr": "glow"})).is_err());
    // Serde keeps the new settings; older files load with the defaults.
    let back: Project = serde_json::from_str(&serde_json::to_string(&*s.project).unwrap()).unwrap();
    assert_eq!(back.settings, s.project.settings);
    let mut v = serde_json::to_value(&s.project.settings).unwrap();
    for k in ["color_engine", "hdr", "output_space"] {
        v.as_object_mut().unwrap().remove(k);
    }
    let old: effectcraft_project::ProjectSettings = serde_json::from_value(v).unwrap();
    assert_eq!((old.color_engine, old.hdr, old.output_space), (ColorEngine::Adobe, HdrMode::Clip, None));
}

#[test]
fn aces_working_space_matches_the_ocio_config() {
    // ACEScg in the colour module and in the OCIO effects' built-in config agree.
    let mut s = session();
    s.execute("file.projectSettings", json!({"workingSpace": "acescg"})).unwrap();
    red_solid(&mut s, "#808080");
    let cid = s.active_comp_id().unwrap();
    // Authored ACEScg values (0.5 linear AP1 grey) → sRGB display.
    let out = s.render(cid, Tick::ZERO, RenderOpts::default()).data[0];
    let want = effectcraft_color::linear_to_srgb(128.0 / 255.0);
    assert!((out[0] - want).abs() < 0.01 && (out[1] - want).abs() < 0.01, "{out:?} vs {want}");
}

#[test]
fn feet_and_frames_display_and_entry() {
    let mut s = session();
    let comp = s.active_comp().unwrap().clone();
    s.execute("file.projectSettings", json!({"timeDisplay": "feet35"})).unwrap();
    assert_eq!(s.project.settings.time_display, TimeDisplayStyle::Feet35);
    let t = comp.frame_rate.tick_of(16 * 3 + 5);
    assert_eq!(crate::commands::time::display_time(&s, &comp, t), "0003+05");
    // Typed feet+frames in the time field.
    let r = s.execute("time.set", json!({"timecode": "2+10"})).unwrap();
    assert_eq!(r["frame"], 42);
    assert_eq!(r["display"], "0002+10");
    let r = s.execute("time.set", json!({"timecode": "+6"})).unwrap();
    assert_eq!(r["frame"], 48);
    s.execute("file.projectSettings", json!({"timeDisplay": "feet16"})).unwrap();
    assert_eq!(crate::commands::time::display_time(&s, &comp, comp.frame_rate.tick_of(95)), "0002+15");
    s.execute("file.projectSettings", json!({"timeDisplay": "frames"})).unwrap();
    assert_eq!(crate::commands::time::display_time(&s, &comp, comp.frame_rate.tick_of(95)), "00095");
    s.execute("file.projectSettings", json!({"timeDisplay": "timecode"})).unwrap();
    assert_eq!(crate::commands::time::display_time(&s, &comp, comp.frame_rate.tick_of(48)), "0:00:02:00");
    assert!(s.execute("file.projectSettings", json!({"timeDisplay": "furlongs"})).is_err());
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(s.project.settings.time_display, TimeDisplayStyle::Frames);
    assert_eq!(serde_json::to_value(TimeDisplayStyle::Feet16).unwrap(), json!("Feet16"));
}

/// View ▸ Simulate Output ▸ My Custom RGB…: typed-in primaries/white/gamma or an ICC profile,
/// kept in Settings.
#[test]
fn my_custom_rgb_simulation() {
    let mut s = session();
    s.config = Some(std::sync::Arc::new(crate::config::MemoryConfig::default()));
    s.execute("file.projectSettings", json!({"workingSpace": "srgb"})).unwrap();
    // The default definition: Rec. 709 primaries, D65, gamma 2.2.
    let d = crate::viewer::CustomRgb::default();
    assert_eq!((d.white, d.gamma), ([0.3127, 0.3290], 2.2));
    // Rec. 709 / D65 / gamma 1.8 behaves exactly like Legacy Macintosh RGB.
    s.execute("view.simulateOutput", json!({"profile": "mac18", "preserveRgb": true})).unwrap();
    let mut a = px(128, 90, 200);
    DisplayColor::of(&s).unwrap().apply(&mut a);
    let r = s
        .execute(
            "view.customRgb",
            json!({"name": "Old Mac", "red": [0.64, 0.33], "green": [0.30, 0.60], "blue": [0.15, 0.06], "white": [0.3127, 0.3290], "gamma": 1.8, "srgbCurve": false, "preserveRgb": true}),
        )
        .unwrap();
    assert_eq!((r["name"].as_str(), r["active"].as_bool()), (Some("Old Mac"), Some(true)));
    assert_eq!(s.state.viewer.simulation, Simulation { profile: SimProfile::MyCustom, preserve_rgb: true });
    let mut b = px(128, 90, 200);
    DisplayColor::of(&s).unwrap().apply(&mut b);
    assert_eq!(a, b);
    assert_eq!(crate::menus::checked(&s, "view.customRgb", &json!({})), Some(true));
    assert_eq!(crate::menus::checked(&s, "view.simulateOutput", &json!({"profile": "myCustom"})), Some(true));
    // Persisted in Settings.
    let saved = s.config.as_ref().unwrap().read(crate::prefs::PREFS_FILE).unwrap();
    assert!(saved.contains("\"customRgb\"") && saved.contains("Old Mac"), "{saved}");
    // Start from a built-in profile.
    s.execute("view.customRgb", json!({"from": "rec2020", "preserveRgb": false})).unwrap();
    assert_eq!(s.prefs.custom_rgb.red, [0.708, 0.292]);
    // An ICC profile: the primaries, white and curve come from the file.
    let p3 = [[0.680, 0.320], [0.265, 0.690], [0.150, 0.060]];
    let icc = effectcraft_color::icc::write_matrix_profile("Studio Monitor", p3, [0.3127, 0.3290], 2.4);
    let path = std::env::temp_dir().join(format!("ec-custom-{}.icc", std::process::id()));
    std::fs::write(&path, icc).unwrap();
    let r = s.execute("view.customRgb", json!({"icc": path.to_string_lossy()})).unwrap();
    let _ = std::fs::remove_file(&path);
    assert_eq!(r["name"], "Studio Monitor");
    let c = s.prefs.custom_rgb.clone();
    assert!((c.green[0] - 0.265).abs() < 1e-3 && (c.green[1] - 0.690).abs() < 1e-3, "{c:?}");
    assert!((c.gamma - 2.4).abs() < 0.01 && !c.srgb_curve);
    assert!(DisplayColor::of(&s).is_some());
    // The same path again keeps typed-in edits; an empty path forgets the profile.
    s.execute("view.customRgb", json!({"icc": c.icc, "gamma": 2.0})).unwrap();
    assert_eq!(s.prefs.custom_rgb.gamma, 2.0);
    s.execute("view.customRgb", json!({"icc": ""})).unwrap();
    assert!(s.prefs.custom_rgb.icc.is_empty());
    // Bad definitions are refused and change nothing.
    let before = s.prefs.custom_rgb.clone();
    assert!(s.execute("view.customRgb", json!({"red": [0.3, 0.6], "green": [0.3, 0.6]})).is_err());
    assert!(s.execute("view.customRgb", json!({"gamma": 0.0})).is_err());
    assert!(s.execute("view.customRgb", json!({"white": [1.2, 0.3]})).is_err());
    assert!(s.execute("view.customRgb", json!({"icc": "/no/such/profile.icc"})).is_err());
    assert_eq!(s.prefs.custom_rgb, before);
    // Define without simulating it.
    s.execute("view.simulateOutput", json!({"profile": "none"})).unwrap();
    s.execute("view.customRgb", json!({"reset": true, "apply": false})).unwrap();
    assert_eq!(s.state.viewer.simulation.profile, SimProfile::None);
    assert_eq!(s.prefs.custom_rgb, crate::viewer::CustomRgb::default());
    assert_eq!(s.execute("view.displayColor", json!({})).unwrap()["customRgb"]["gamma"], json!(2.2));
}

/// My Custom RGB from a LUT-based (`A2B0`) ICC profile: the viewer runs the profile's tables
/// (baked into a 3D LUT) and matches the same device described as a matrix/TRC profile.
#[test]
fn my_custom_rgb_lut_profiles() {
    use effectcraft_color::icc::{D50, LutKind, write_lut_profile};
    use effectcraft_color::space;
    let mut s = session();
    s.config = Some(std::sync::Arc::new(crate::config::MemoryConfig::default()));
    s.execute("file.projectSettings", json!({"workingSpace": "srgb"})).unwrap();
    // The device: P3 primaries, D65 white, gamma 2.4.
    let p3 = [[0.680, 0.320], [0.265, 0.690], [0.150, 0.060]];
    let d65 = [0.3127, 0.3290];
    let m = space::mul(&space::bradford(d65, D50), &space::rgb_to_xyz(p3, d65));
    let mi = space::invert(&m);
    let to = move |c: [f64; 3]| space::mul_vec(&m, c.map(|v| v.powf(2.4)));
    let from = move |x: [f64; 3]| space::mul_vec(&mi, x).map(|v| v.clamp(0.0, 1.0).powf(1.0 / 2.4));
    s.execute(
        "view.customRgb",
        json!({"name": "Matrix", "red": p3[0], "green": p3[1], "blue": p3[2], "white": d65, "gamma": 2.4, "srgbCurve": false, "preserveRgb": false}),
    )
    .unwrap();
    // (Inside both gamuts, away from the edges where a coarse B2A0 grid bends.)
    let colors = [px(128, 90, 200), px(90, 170, 110), px(225, 225, 225), px(60, 60, 60), px(200, 120, 100)];
    let reference: Vec<Vec<[u8; 4]>> = colors
        .iter()
        .map(|c| {
            let mut c = c.clone();
            DisplayColor::of(&s).unwrap().apply(&mut c);
            c
        })
        .collect();
    let dir = std::env::temp_dir();
    for (k, (kind, b2a)) in [(LutKind::Lut16, true), (LutKind::AToB, true), (LutKind::Lut8, true), (LutKind::Lut16, false)].into_iter().enumerate() {
        let icc = write_lut_profile("LUT Display", kind, 33, &to, if b2a { Some(&from) } else { None });
        let path = dir.join(format!("ec-lut-{}-{k}.icc", std::process::id()));
        std::fs::write(&path, &icc).unwrap();
        let r = s.execute("view.customRgb", json!({"icc": path.to_string_lossy(), "preserveRgb": false})).unwrap();
        let _ = std::fs::remove_file(&path);
        assert_eq!((r["name"].as_str(), r["lutProfile"].as_bool()), (Some("LUT Display"), Some(true)), "{r}");
        assert!(r.get("iccLut").is_none());
        // The numbers approximate the device.
        let c = s.prefs.custom_rgb.clone();
        assert!((c.red[0] - 0.680).abs() < 0.01 && (c.green[1] - 0.690).abs() < 0.01, "{kind:?} {c:?}");
        assert!((c.gamma - 2.4).abs() < 0.1, "{kind:?} {}", c.gamma);
        let dc = DisplayColor::of(&s).unwrap();
        assert!(dc.is_lut());
        let tol = if kind == LutKind::Lut8 { 6 } else { 4 };
        for (c, want) in colors.iter().zip(&reference) {
            let mut got = c.clone();
            dc.apply(&mut got);
            let d = (0..3).map(|i| (got[0][i] as i32 - want[0][i] as i32).abs()).max().unwrap();
            assert!(d <= tol, "{kind:?} b2a {b2a}: {c:?} → {got:?}, matrix {want:?}");
        }
    }
    // Kept in Settings (base64) and restored with the preferences.
    let saved = s.config.as_ref().unwrap().read(crate::prefs::PREFS_FILE).unwrap();
    assert!(saved.contains("\"iccLut\""));
    let back: crate::viewer::CustomRgb = serde_json::from_value(serde_json::from_str::<serde_json::Value>(&saved).unwrap()["customRgb"].clone()).unwrap();
    assert_eq!(back, s.prefs.custom_rgb);
    assert!(format!("{back:?}").len() < 400);
    // Typed-in numbers replace the tables.
    s.execute("view.customRgb", json!({"gamma": 2.2})).unwrap();
    assert!(s.prefs.custom_rgb.icc_lut.is_empty());
    assert!(!DisplayColor::of(&s).unwrap().is_lut());
}
