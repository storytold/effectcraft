//! Sizes, frame rates and motion-blur samples stay within the project limits when a project is
//! opened and when a command sets them (#788).

use effectcraft_project::{ItemKind, limits};
use effectcraft_render::RenderOpts;
use effectcraft_time::{FrameRate, Tick};
use serde_json::{Value, json};

use crate::{Event, Session};

fn tmp(name: &str) -> String {
    let d = std::env::temp_dir().join(format!("effectcraft-limits-tests-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d.join(name).to_string_lossy().to_string()
}

/// The saved project with `edit` applied to the JSON of every item of kind `kind`.
fn tamper(text: &str, kind: &str, edit: impl Fn(&mut Value)) -> String {
    let mut v: Value = serde_json::from_str(text).unwrap();
    let items = v["items"].as_object_mut().unwrap();
    for it in items.values_mut().filter(|it| it["kind"]["type"] == kind) {
        edit(&mut it["kind"]);
    }
    serde_json::to_string(&v).unwrap()
}

/// A project whose comp is 4 billion × 0 px at 10^18 fps, -5 s long with 4 billion motion-blur
/// samples, holding a solid of 4 billion px.
fn hostile_project() -> String {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Main", "width": 64, "height": 32, "frameRate": 30, "duration": 2})).unwrap();
    s.execute("layer.newSolid", json!({"color": "#ff0000", "width": 64, "height": 2})).unwrap();
    let text = s.project.to_file_json().unwrap();
    let text = tamper(&text, "Comp", |c| {
        c["width"] = json!(4_000_000_000u64);
        c["height"] = json!(0);
        c["frame_rate"] = json!({"num": 1_000_000_000_000_000_000i64, "den": 1});
        c["duration"] = json!(-5);
        c["motion_blur_samples"] = json!(4_000_000_000u64);
    });
    tamper(&text, "Solid", |so| so["width"] = json!(4_000_000_000u64))
}

#[test]
fn a_hostile_project_opens_within_the_limits_and_says_what_changed() {
    let json_path = tmp("Hostile.ecproj");
    let text = hostile_project();
    std::fs::write(&json_path, &text).unwrap();
    let xml_path = tmp("Hostile.ecprojx");
    std::fs::write(&xml_path, crate::xml_project::to_xml(&text).unwrap()).unwrap();
    for path in [json_path, xml_path] {
        let mut s = Session::default();
        let r = s.execute("file.open", json!({"path": path})).unwrap();
        let repaired = r["repaired"].as_array().unwrap();
        assert!(repaired.iter().any(|n| n.as_str().unwrap().contains("4000000000 × 0 px → 30000 × 1 px")), "{path}: {repaired:?}");
        let toast = s.drain_events().into_iter().any(|e| matches!(e, Event::Toast { message, .. } if message.contains("outside EffectCraft's limits")));
        assert!(toast, "{path}");
        let (cid, c) = s.project.comps().next().map(|(id, c)| (*id, c.clone())).unwrap();
        assert_eq!((c.width, c.height), (limits::MAX_SIZE, 1));
        assert_eq!(c.frame_rate.as_f64(), limits::MAX_FPS);
        assert_eq!(c.duration, c.frame_rate.frame_duration(), "at least one frame");
        assert_eq!(c.work_area, (Tick::ZERO, c.duration));
        assert_eq!(c.motion_blur_samples, limits::MOTION_BLUR_SAMPLES.1);
        let solid = s.project.items.values().find_map(|i| if let ItemKind::Solid(so) = &i.kind { Some(so.clone()) } else { None }).unwrap();
        assert_eq!((solid.width, solid.height), (limits::MAX_SIZE, 2));
        // The comp renders at Full resolution, at its (capped) size.
        let img = s.render(cid, Tick::ZERO, RenderOpts { scale: 1.0, ..Default::default() });
        assert_eq!((img.width, img.height), (limits::MAX_SIZE, 1));
    }
}

#[test]
fn a_project_within_the_limits_opens_unchanged() {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Main", "width": 64, "height": 32, "frameRate": 24, "duration": 2})).unwrap();
    let text = s.project.to_file_json().unwrap();
    let (p, notes) = effectcraft_project::Project::from_json_repaired(&text).unwrap();
    assert!(notes.is_empty(), "{notes:?}");
    assert_eq!(p, *s.project);
}

#[test]
fn commands_cap_sizes_at_30000_px() {
    let mut s = Session::default();
    // 2^32 + 10 used to wrap to 10 px.
    s.execute("comp.new", json!({"name": "Main", "width": 4_294_967_306u64, "height": 32, "pixelAspect": 1e9})).unwrap();
    let c = s.active_comp().unwrap().clone();
    assert_eq!((c.width, c.height, c.pixel_aspect), (limits::MAX_SIZE, 32, limits::MAX_PIXEL_ASPECT));
    s.execute("comp.settings", json!({"width": 64, "height": 4_294_967_306u64})).unwrap();
    assert_eq!((s.active_comp().unwrap().width, s.active_comp().unwrap().height), (64, limits::MAX_SIZE));
    // New Solid: 0 and 4 billion px are brought to 1 and 30,000; the layer matches its solid.
    s.execute("layer.newSolid", json!({"width": 0, "height": 4_000_000_000u64})).unwrap();
    let solid = |s: &Session| s.project.items.values().find_map(|i| if let ItemKind::Solid(so) = &i.kind { Some(so.clone()) } else { None }).unwrap();
    assert_eq!((solid(&s).width, solid(&s).height), (1, limits::MAX_SIZE));
    // Layer Settings on the new (selected) solid layer.
    s.execute("layer.settings", json!({"width": 4_000_000_000u64, "height": 8})).unwrap();
    assert_eq!((solid(&s).width, solid(&s).height), (limits::MAX_SIZE, 8));
    s.execute("file.importSolid", json!({"width": 99_999_999, "height": 4})).unwrap();
    assert!(s.project.items.values().any(|i| matches!(&i.kind, ItemKind::Solid(so) if (so.width, so.height) == (limits::MAX_SIZE, 4))));
    // A placeholder can't ask for a huge frame or a zero frame rate either.
    s.execute("file.importPlaceholder", json!({"width": 4_000_000_000u64, "height": 8, "frameRate": 0})).unwrap();
    let f = s.project.items.values().find_map(|i| if let ItemKind::Footage(f) = &i.kind { Some(f.clone()) } else { None }).unwrap();
    assert_eq!((f.width, f.height, f.frame_rate), (limits::MAX_SIZE, 8, FrameRate::FPS_29_97));
}
