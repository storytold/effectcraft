use effectcraft_keyframe::{Keyframe, ShapePath, Value};
use effectcraft_time::{FrameRate, Tick};

use crate::build::{self, Ids};
use crate::{Comp, ItemKind, LayerSource, MaskMode, Project, Solid};

fn project_with_layer() -> (Project, crate::ItemId) {
    let mut p = Project::default();
    let comp = Comp::new(1920, 1080, FrameRate::FPS_29_97, Tick::from_seconds_f64(10.0));
    let cid = p.add_item("Main", effectcraft_color::Label::Sandstone, None, ItemKind::Comp(comp.clone().into()));
    let sid = p.add_item(
        "Red Solid",
        effectcraft_color::Label::Red,
        None,
        ItemKind::Solid(Solid { color: [1.0, 0.0, 0.0], width: 1920, height: 1080, pixel_aspect: 1.0 }),
    );
    let layer = build::layer(&mut p, &comp, "Red Solid 1", LayerSource::Solid { item: sid }, (1920, 1080), None);
    p.comp_mut(cid).unwrap().layers.push(layer);
    (p, cid)
}

#[test]
fn paths_resolve() {
    let (p, cid) = project_with_layer();
    let l = &p.comp(cid).unwrap().layers[0];
    assert_eq!(l.props.prop("transform/position").unwrap().value, Value::Vec3([960.0, 540.0, 0.0]));
    assert_eq!(l.props.prop("Transform/Opacity").unwrap().value, Value::Scalar(100.0));
    assert_eq!(l.props.prop("transform/#7").unwrap().match_id, "rotation");
    let uid = l.props.prop("transform/scale").unwrap().uid;
    let path = l.props.path_of(uid).unwrap();
    assert_eq!(l.props.prop(&path).unwrap().match_id, "scale");
    assert_eq!(l.props.name_path_of(uid).unwrap(), "Transform/Scale");
}

#[test]
fn masks_and_occurrence_paths() {
    let (mut p, cid) = project_with_layer();
    let mut next = p.next_id;
    let m1 = build::mask(&mut Ids(&mut next), "Mask 1", ShapePath::rect([0.0, 0.0], 10.0, 10.0), MaskMode::Add, [255, 0, 0]);
    let m2 = build::mask(&mut Ids(&mut next), "Mask 2", ShapePath::ellipse([0.0, 0.0], 10.0, 10.0), MaskMode::Subtract, [0, 255, 0]);
    p.next_id = next;
    let l = &mut p.comp_mut(cid).unwrap().layers[0];
    let masks = l.props.sub_mut("masks").unwrap();
    masks.children.push(m1.into());
    masks.children.push(m2.into());
    assert_eq!(l.props.group("masks/mask#2").unwrap().name, "Mask 2");
    assert_eq!(l.props.group("masks/Mask 2").unwrap().name, "Mask 2");
    assert!(l.props.prop("masks/mask#2/feather").is_some());
}

#[test]
fn animate_and_roundtrip_json() {
    let (mut p, cid) = project_with_layer();
    {
        let l = &mut p.comp_mut(cid).unwrap().layers[0];
        let op = l.props.prop_mut("transform/opacity").unwrap();
        op.set_animated(true, Tick::ZERO);
        op.set_value_at(Tick::from_seconds_f64(1.0), Value::Scalar(0.0));
        assert_eq!(op.keys.len(), 2);
        assert_eq!(op.value_at(Tick::from_seconds_f64(0.5)), Value::Scalar(50.0));
    }
    let json = p.to_json();
    let q = Project::from_json(&json).unwrap();
    assert_eq!(p, q);
    let _ = Keyframe::new(Tick::ZERO, Value::Scalar(0.0));
}

#[test]
fn comp_resolution_is_saved_and_old_projects_load_with_auto() {
    use crate::Resolution;
    let (mut p, cid) = project_with_layer();
    // Auto (the default) isn't written, so older versions read the file as before.
    assert!(!p.to_file_json().unwrap().contains("\"resolution\""));
    p.comp_mut(cid).unwrap().resolution = Resolution::Custom(5);
    let q = Project::from_json(&p.to_file_json().unwrap()).unwrap();
    assert_eq!(q.comp(cid).unwrap().resolution, Resolution::Custom(5));
    p.comp_mut(cid).unwrap().resolution = Resolution::Half;
    let text = p.to_file_json().unwrap();
    assert_eq!(Project::from_json(&text).unwrap().comp(cid).unwrap().resolution, Resolution::Half);
    // A project saved before comps had a resolution opens with Auto.
    fn strip(v: &mut serde_json::Value) {
        match v {
            serde_json::Value::Object(m) => {
                m.remove("resolution");
                m.values_mut().for_each(strip);
            }
            serde_json::Value::Array(a) => a.iter_mut().for_each(strip),
            _ => {}
        }
    }
    let mut old: serde_json::Value = serde_json::from_str(&text).unwrap();
    strip(&mut old);
    assert_eq!(Project::from_json(&old.to_string()).unwrap().comp(cid).unwrap().resolution, Resolution::Auto);
    // It's not part of the comp's pixels.
    let (a, b) = (p.comp(cid).unwrap(), q.comp(cid).unwrap());
    assert!(a != b && a.same_pixels(b));
    assert_eq!(format!("{:?}", a.pixel_form()), format!("{:?}", b.pixel_form()));
}

#[test]
fn resolution_factors_and_scales() {
    use crate::Resolution;
    assert_eq!(Resolution::from_factor(1), Resolution::Full);
    assert_eq!(Resolution::from_factor(4), Resolution::Quarter);
    assert_eq!(Resolution::from_factor(0), Resolution::Full);
    assert_eq!(Resolution::from_factor(7), Resolution::Custom(7));
    assert_eq!(Resolution::from_factor(1000), Resolution::Custom(40));
    assert_eq!(Resolution::Custom(5).factor(), Some(5));
    assert_eq!(Resolution::Auto.factor(), None);
    // A hostile factor from a file renders within the Custom range.
    assert_eq!(Resolution::Custom(0).scale(1.0, 1.0), 1.0);
    assert_eq!(Resolution::Custom(255).scale(1.0, 1.0), 1.0 / 40.0);
    assert_eq!(Resolution::Third.scale(0.1, 1.0), 1.0 / 3.0);
}

/// Another layer of the same source, and a duplicated item, get a numbered name. Only a number set
/// off by a space is a counter; digits that end a word stay (`v-b1.mp4` gave `v-b1.mp 2`).
#[test]
fn numbered_names_keep_digits_that_end_a_word() {
    // (the name in use, the next layer's and the item copy's name)
    let rows = [
        ("v-b1.mp4", "v-b1.mp4 2"),
        ("clip2.mov", "clip2.mov 2"),
        ("shot 2", "shot 3"),
        ("a.b.c", "a.b.c 2"),
        ("Background", "Background 2"),
        ("Red Solid 1", "Red Solid 2"),
    ];
    for (name, next) in rows {
        let mut p = Project::default();
        let mut comp = Comp::new(64, 36, FrameRate::FPS_29_97, Tick::from_seconds_f64(1.0));
        let solid = Solid { color: [0.5; 3], width: 64, height: 36, pixel_aspect: 1.0 };
        let item = p.add_item(name, effectcraft_color::Label::Red, None, ItemKind::Solid(solid));
        let first = build::layer(&mut p, &comp, name, LayerSource::Solid { item }, (64, 36), None);
        assert_eq!(first.name, name, "the first layer of {name}");
        comp.layers.push(first);
        let second = build::layer(&mut p, &comp, name, LayerSource::Solid { item }, (64, 36), None);
        assert_eq!(second.name, next, "the second layer of {name}");
        assert_eq!(p.numbered_item_name(name, |_| false), next, "a copy of the item {name}");
    }
}

#[test]
fn unique_names_and_cycles() {
    let (p, cid) = project_with_layer();
    let c = p.comp(cid).unwrap();
    assert_eq!(c.unique_layer_name("Red Solid 1"), "Red Solid 2");
    assert_eq!(c.unique_layer_name("Shape Layer 1"), "Shape Layer 1");
    assert!(p.comp_contains(cid, cid));
}

#[test]
fn camera_and_light_layers_build() {
    let (mut p, cid) = project_with_layer();
    let comp = p.comp(cid).unwrap().clone();
    let cam = build::layer(&mut p, &comp, "Camera 1", LayerSource::Camera, (0, 0), None);
    assert!(cam.props.prop("cameraOptions/zoom").is_some());
    assert!(cam.props.prop("transform/poi").is_some());
    let light = build::layer(&mut p, &comp, "Light 1", LayerSource::Light { kind: crate::LightKind::Spot }, (0, 0), None);
    assert!(light.props.prop("lightOptions/coneAngle").is_some());
}

#[test]
fn same_content_sees_allocated_ids_and_comp_edits() {
    let (p, cid) = project_with_layer();
    let mut q = p.clone();
    assert!(q.same_content(&p));
    q.alloc();
    assert!(!q.same_content(&p), "an allocated id stays taken");
    let mut q = p.clone();
    // A comp copied on write but left equal is the same; a changed one is not.
    let _ = q.comp_mut(cid).unwrap();
    assert!(q.same_content(&p));
    q.comp_mut(cid).unwrap().layers[0].name = "Renamed".into();
    assert!(!q.same_content(&p));
    let mut r = p.clone();
    r.item_mut(cid).unwrap().name = "Other".into();
    assert!(!r.same_content(&p));
    let mut r = p.clone();
    r.settings.bit_depth = crate::BitDepth::Bpc16;
    assert!(!r.same_content(&p));
}

#[test]
fn project_files_name_their_version_and_always_read_back() {
    let (mut p, cid) = project_with_layer();
    let text = p.to_file_json().unwrap();
    assert_eq!(crate::saved_by(&text).as_deref(), Some(crate::APP_VERSION));
    assert!(text.trim_start().starts_with("{\n  \"savedBy\""), "the version comes first");
    assert_eq!(Project::from_json(&text).unwrap(), p);
    assert_eq!(crate::saved_by(&p.to_json()), None, "plain JSON (worker hand-offs) has none");
    assert_eq!(crate::saved_by(r#"{"savedBy": "999.0.0", "schema": 3}"#).as_deref(), Some("999.0.0"), "a newer schema still names its version");
    assert_eq!(crate::saved_by(r#"[2, {}, {}, 1]"#), None, "the array form has none");
    // JSON can't hold NaN: saving refuses instead of writing a file that can't be opened.
    p.comp_mut(cid).unwrap().layers[0].props.prop_mut("transform/opacity").unwrap().value = Value::Scalar(f64::NAN);
    let e = p.to_file_json().unwrap_err().to_string();
    assert!(e.contains("can't be saved"), "{e}");
    // Infinity too, in either profile: the check doesn't rely on reading the file back.
    p.comp_mut(cid).unwrap().layers[0].props.prop_mut("transform/opacity").unwrap().value = Value::Scalar(f64::INFINITY);
    let e = p.to_file_json().unwrap_err().to_string();
    assert!(e.contains("can't be saved"), "{e}");
    assert!(crate::is_newer_version("0.2.0", "0.1.9") && crate::is_newer_version("1.0.0-rc.1", "0.9.9"));
    assert!(!crate::is_newer_version("0.1.1", "0.1.1") && !crate::is_newer_version("0.1.0", "0.1.1"));
}

#[test]
fn project_files_refuse_newer_schemas() {
    let p = Project { schema: crate::SCHEMA_VERSION + 1, ..Project::default() };
    let expected = format!(
        "the project can't be saved: it holds a value a project file can't store (project schema {} is newer than this version supports ({}))",
        p.schema,
        crate::SCHEMA_VERSION
    );
    assert_eq!(p.to_file_json().unwrap_err().to_string(), expected);
    assert_eq!(p.to_file_json_with_view(&crate::SavedView::default(), false).unwrap_err().to_string(), expected);
}

#[test]
fn saved_files_reopen_in_pretty_and_compact_form() {
    let (p, _cid) = project_with_layer();
    assert_eq!(Project::from_json(&p.to_file_json().unwrap()).unwrap(), p, "the pretty project file");
    assert_eq!(Project::from_json(&p.to_json_compact()).unwrap(), p, "compact JSON");
}

/// A project file read back and saved again is the same text, indented and compact (Save Compact
/// Project Files), with its saved view.
#[test]
fn project_files_save_again_byte_for_byte() {
    let (p, cid) = project_with_layer();
    let view = crate::SavedView { active_comp: Some(cid), open_comps: vec![cid], comp_times: [(cid, effectcraft_time::Tick(4_237_833_600))].into() };
    for compact in [false, true] {
        let text = p.to_file_json_with_view(&view, compact).unwrap();
        let prefix = if compact { "{\"savedBy\":\"" } else { "{\n  \"savedBy\": \"" };
        assert!(text.starts_with(&format!("{prefix}{}\"", crate::APP_VERSION)), "compact {compact}: {text}");
        assert_eq!(text.contains('\n'), !compact);
        let again = Project::from_json(&text).unwrap().to_file_json_with_view(&crate::saved_view(&text), compact).unwrap();
        assert_eq!(again, text, "compact {compact}");
    }
}

#[test]
fn project_files_remember_the_open_comps() {
    let (p, cid) = project_with_layer();
    let view = crate::SavedView { active_comp: Some(cid), open_comps: vec![cid], comp_times: [(cid, effectcraft_time::Tick(4_237_833_600))].into() };
    let text = p.to_file_json_with_view(&view, false).unwrap();
    assert_eq!(crate::saved_view(&text), view);
    assert_eq!(Project::from_json(&text).unwrap(), p, "the view is not part of the project");
    // Files from before #748 have no view; a garbled one is ignored, not an error.
    assert_eq!(crate::saved_view(&p.to_file_json().unwrap()), crate::SavedView::default());
    let odd = text.replacen(&format!("\"activeComp\": {}", cid.0), "\"activeComp\": \"Main\"", 1);
    assert_eq!(crate::saved_view(&odd), crate::SavedView { active_comp: None, ..view.clone() });
    // Entries that aren't an id and a whole tick count are skipped; the rest are kept.
    let times = text.replacen("\"compTimes\": {", "\"compTimes\": {\"x\": 1, \"7\": \"a\", \"8\": 1.5, \"9\": -4,", 1);
    let expect = [(cid, effectcraft_time::Tick(4_237_833_600)), (crate::ItemId(9), effectcraft_time::Tick(-4))].into();
    assert_eq!(crate::saved_view(&times).comp_times, expect);
    let not_a_map = text.replacen("\"compTimes\": {", "\"compTimes\": [], \"unused\": {", 1);
    assert_eq!(crate::saved_view(&not_a_map), crate::SavedView { comp_times: Default::default(), ..view });
}

/// The Project panel's "used N times": layers in every comp that use the item directly, a precomp
/// counted by the comps it is nested in, and an unused item used by nothing.
#[test]
fn item_users_count_layers_in_every_comp() {
    let (mut p, main) = project_with_layer();
    let sid = p.find_by_name("Red Solid").unwrap().id;
    let comp = p.comp(main).unwrap().clone();
    let pre = p.add_item("A Precomp", effectcraft_color::Label::Sandstone, None, ItemKind::Comp(comp.clone().into()));
    let unused = p.add_item("Unused", effectcraft_color::Label::Red, None, ItemKind::Solid(Solid { color: [0.0; 3], width: 8, height: 8, pixel_aspect: 1.0 }));
    // Two more layers of the solid in the precomp (it starts with one, cloned from Main), and the
    // precomp nested in Main.
    for _ in 0..2 {
        let l = build::layer(&mut p, &comp, "Red Solid", LayerSource::Solid { item: sid }, (1920, 1080), None);
        p.comp_mut(pre).unwrap().layers.push(l);
    }
    let nest = build::layer(&mut p, &comp, "A Precomp", LayerSource::Comp { item: pre }, (1920, 1080), None);
    p.comp_mut(main).unwrap().layers.push(nest);
    // Ordered by comp name: "A Precomp" before "Main".
    assert_eq!(p.item_users(sid), vec![(pre, 3), (main, 1)]);
    assert_eq!(p.item_users(pre), vec![(main, 1)]);
    assert!(p.item_users(unused).is_empty());
    assert!(p.item_users(main).is_empty());
}

/// A project of `n` empty comps; `links` adds a precomp layer of comp `b` inside comp `a`
/// (indexes into the returned ids).
fn comp_graph(n: usize, links: &[(usize, usize)]) -> (Project, Vec<crate::ItemId>) {
    let mut p = Project::default();
    let ids: Vec<crate::ItemId> = (0..n)
        .map(|i| {
            p.add_item(
                &format!("C{i}"),
                effectcraft_color::Label::None,
                None,
                ItemKind::Comp(Comp::new(8, 8, FrameRate::FPS_25, Tick::from_seconds_f64(1.0)).into()),
            )
        })
        .collect();
    for &(a, b) in links {
        let outer = p.comp(ids[a]).unwrap().clone();
        let l = build::layer(&mut p, &outer, "pre", LayerSource::Comp { item: ids[b] }, (8, 8), None);
        let ItemKind::Comp(c) = &mut p.item_mut(ids[a]).unwrap().kind else { panic!("comp") };
        std::sync::Arc::make_mut(c).layers.push(l);
    }
    (p, ids)
}

#[test]
fn comp_contains_ends_on_cycles_long_chains_and_shared_precomps() {
    // A ⇄ B cycle, C unrelated: the walk used to recurse until the stack ran out (#1237).
    let (p, c) = comp_graph(3, &[(0, 1), (1, 0)]);
    assert!(!p.comp_contains(c[0], c[2]));
    assert!(p.comp_contains(c[0], c[1]) && p.comp_contains(c[1], c[0]));
    // A comp nesting itself.
    let (p, c) = comp_graph(2, &[(0, 0)]);
    assert!(!p.comp_contains(c[0], c[1]));
    assert!(p.comp_contains(c[0], c[0]));
    // A target reached through a cycle.
    let (p, c) = comp_graph(3, &[(0, 1), (1, 0), (1, 2)]);
    assert!(p.comp_contains(c[0], c[2]));
    assert!(!p.comp_contains(c[2], c[0]));
    // 40 levels of diamonds: 2^40 paths, each comp visited once.
    let mut links = vec![];
    for i in 0..40 {
        let (top, l, r, bottom) = (i * 3, i * 3 + 1, i * 3 + 2, i * 3 + 3);
        links.extend([(top, l), (top, r), (l, bottom), (r, bottom)]);
    }
    let (p, c) = comp_graph(121, &links);
    let t0 = std::time::Instant::now();
    assert!(p.comp_contains(c[0], c[120]));
    assert!(!p.comp_contains(c[120], c[0]));
    assert!(t0.elapsed() < std::time::Duration::from_secs(1), "{:?}", t0.elapsed());
    // A chain 20 000 comps deep (a deep recursion's stack frames).
    let n = 20_000;
    let (p, c) = comp_graph(n, &(0..n - 1).map(|i| (i, i + 1)).collect::<Vec<_>>());
    assert!(p.comp_contains(c[0], c[n - 1]));
    assert!(!p.comp_contains(c[n - 1], c[0]));
}

/// Interpret Footage ▸ Cineon Settings: the 10 Bit Black and White Points become the converted
/// points, Current Gamma 1 gives linear light, Highlight Rolloff 0 keeps over-range values and
/// a higher one ramps them off; the settings apply only to DPX and Cineon files, and projects
/// saved without them read the defaults.
#[test]
fn cineon_settings_convert_log_code_values() {
    use crate::{CineonSettings, CineonUnits, Footage};
    let code = |v: f32| v / 1023.0;
    let c = CineonSettings { log_conversion: true, rolloff: 0.0, ..Default::default() };
    assert!(c.convert(code(95.0)).abs() < 1e-6, "black point");
    assert!((c.convert(code(685.0)) - 1.0).abs() < 1e-5, "white point");
    // Code 400: (10^(-0.95) - 10^(-1.9667)) / (1 - 10^(-1.9667)) = 0.10251 linear, ^(1/1.7).
    assert!((c.convert(code(400.0)) - 0.26190).abs() < 1e-4, "{}", c.convert(code(400.0)));
    let linear = CineonSettings { gamma: 1.0, ..c };
    assert!((linear.convert(code(400.0)) - 0.10251).abs() < 1e-4, "{}", linear.convert(code(400.0)));
    assert!(c.convert(code(1023.0)) > 1.5, "over range kept: {}", c.convert(code(1023.0)));
    // The default rolloff (20) ramps the white point and above off, up to 1.
    let rolled = CineonSettings { rolloff: 20.0, ..c };
    let (w, mid, top) = (rolled.convert(code(685.0)), rolled.convert(code(720.0)), rolled.convert(code(1023.0)));
    assert!(w > 0.8 && w < mid && mid < top && top <= 1.0, "{w} {mid} {top}");
    let points = CineonSettings { converted_black: 64.0, converted_white: 940.0, ..c };
    assert!((points.convert(code(95.0)) - 64.0 / 1023.0).abs() < 1e-5);
    assert!((points.convert(code(685.0)) - 940.0 / 1023.0).abs() < 1e-5);
    // Units.
    assert!((CineonUnits::EightBit.from_ten_bit(1023.0) - 255.0).abs() < 1e-9);
    assert!((CineonUnits::Float.to_ten_bit(0.5) - 511.5).abs() < 1e-9);
    assert_eq!(CineonUnits::parse("8-bit"), Some(CineonUnits::EightBit));
    assert!(CineonUnits::ALL.iter().all(|u| CineonUnits::parse(u.id()) == Some(*u)));
    // DPX and Cineon files only, and only with Logarithmic Conversion on.
    let on = CineonSettings { log_conversion: true, ..Default::default() };
    let dpx = Footage { path: "/m/plate.0001.DPX".into(), cineon: on, ..Default::default() };
    assert!(dpx.cineon_applies());
    assert!(Footage { path: "/m/scan.cin".into(), cineon: on, ..Default::default() }.cineon_applies());
    assert!(!Footage { path: "/m/still.png".into(), cineon: on, ..Default::default() }.cineon_applies());
    assert!(!Footage { path: "/m/plate.dpx".into(), ..Default::default() }.cineon_applies());
    // Saved only when changed; read back, and absent in older projects.
    let plain = serde_json::to_value(Footage { path: "/m/a.dpx".into(), ..Default::default() }).unwrap();
    assert!(plain.get("cineon").is_none(), "{plain}");
    let saved = serde_json::to_string(&dpx).unwrap();
    assert_eq!(serde_json::from_str::<Footage>(&saved).unwrap().cineon, on);
    assert_eq!(serde_json::from_value::<Footage>(plain).unwrap().cineon, CineonSettings::default());
}

/// A Wiggle Paths 100 000 groups deep gets Spatial Phase on a 2 MiB stack.
#[test]
fn upgrading_deeply_nested_shape_groups_fits_a_small_stack() {
    let n = 100_000;
    let mut next = 1;
    let mut contents = build::shape_simple_op(&mut Ids(&mut next), "wiggle").unwrap();
    contents.children.retain(|c| c.match_id() != "spatialPhase");
    for _ in 0..n {
        let uid = Ids(&mut next).alloc();
        contents = crate::PropGroup::new(uid, "group", "Group 1").with(contents);
    }
    let h = std::thread::Builder::new()
        .stack_size(2 << 20)
        .spawn(move || {
            let added = build::upgrade_shape_contents(&mut Ids(&mut next), &mut contents);
            (added, contents)
        })
        .unwrap();
    let (added, mut g) = h.join().unwrap();
    assert!(added);
    // One level at a time: dropping the whole tree would recurse as deep as it is nested.
    let mut depth = 0;
    while g.match_id == "group" {
        let Some(crate::Node::Group(child)) = g.children.pop() else { panic!("a nested group") };
        g = child;
        depth += 1;
    }
    assert_eq!(depth, n);
    let ids: Vec<&str> = g.children.iter().map(|c| c.match_id()).collect();
    assert_eq!(ids, ["size", "detail", "points", "speed", "correlation", "phase", "spatialPhase", "seed"]);
}

/// Music in a precomp is heard, and so not silent, when a layer of a comp nesting it picks the
/// precomp layer (an Audio Waveform drawing the precomp's mix).
#[test]
fn music_in_a_precomp_a_waveform_picks_is_not_silent() {
    use crate::{Footage, FootageKind, GroupKind};
    let (mut p, inner) = project_with_layer();
    let song = p.add_item(
        "song.wav",
        effectcraft_color::Label::None,
        None,
        ItemKind::Footage(Footage { kind: FootageKind::Audio, has_video: false, has_audio: true, ..Default::default() }),
    );
    let comp = p.comp(inner).unwrap().clone();
    let music = build::layer(&mut p, &comp, "Music", LayerSource::Footage { item: song }, (1920, 1080), None);
    let music_id = music.id;
    p.comp_mut(inner).unwrap().layers.push(music);
    let outer = p.add_item("Outer", effectcraft_color::Label::None, None, ItemKind::Comp(comp.clone().into()));
    let pre = build::layer(&mut p, &comp, "Pre", LayerSource::Comp { item: inner }, (1920, 1080), None);
    let pre_id = pre.id;
    p.comp_mut(outer).unwrap().layers.push(pre);
    let silent = |p: &Project| p.silent_layers(inner, p.comp(inner).unwrap(), &p.heard_layers()).contains(&music_id);
    assert!(silent(&p), "nothing draws the precomp's audio");
    let mut n = 1_000_000;
    let mut ids = build::Ids(&mut n);
    let mut g = ids.group("waveform", "Audio Waveform");
    g.kind = GroupKind::Effect { effect: "ec.generate.audiowaveform".into() };
    g.children.push(ids.prop("audioLayer", "Audio Layer", Value::Layer(Some(pre_id.0))).into());
    let solid = p.comp(outer).unwrap().layers[0].id;
    p.comp_mut(outer).unwrap().layer_mut(solid).unwrap().props.group_mut("effects").unwrap().children.push(g.into());
    assert!(!silent(&p), "the outer comp's waveform draws the music");
}

/// Layers that can't change the picture: music, hidden layers nothing reads. A hidden matte is
/// read by the layer it mattes; nulls and cameras are never silent.
#[test]
fn silent_layers_are_music_and_unread_hidden_layers() {
    use crate::{Footage, FootageKind, LayerId, MatteKind, TrackMatte};
    let (mut p, cid) = project_with_layer();
    let comp = p.comp(cid).unwrap().clone();
    let song = p.add_item(
        "song.wav",
        effectcraft_color::Label::None,
        None,
        ItemKind::Footage(Footage { kind: FootageKind::Audio, has_video: false, has_audio: true, ..Default::default() }),
    );
    let add = |p: &mut Project, name: &str, source: LayerSource| {
        let l = build::layer(p, &comp, name, source, (1920, 1080), None);
        let id = l.id;
        p.comp_mut(cid).unwrap().layers.push(l);
        id
    };
    let music = add(&mut p, "Music", LayerSource::Footage { item: song });
    let solid = p.comp(cid).unwrap().layers[0].source.clone();
    let hidden = add(&mut p, "Hidden", solid.clone());
    let matte = add(&mut p, "Matte", solid);
    let null = add(&mut p, "Null", LayerSource::Null);
    let first = p.comp(cid).unwrap().layers[0].id;
    let c = p.comp_mut(cid).unwrap();
    for id in [hidden, matte, null] {
        c.layer_mut(id).unwrap().switches.video = false;
    }
    c.layer_mut(first).unwrap().track_matte = Some(TrackMatte { layer: matte, kind: MatteKind::Alpha });
    let c = p.comp(cid).unwrap();
    assert_eq!(p.silent_layers(cid, c, &p.heard_layers()).into_iter().collect::<Vec<LayerId>>(), vec![music, hidden]);
    // Solo hides every other layer, so a soloed music or hidden layer changes the picture.
    for (soloed, rest) in [(music, hidden), (hidden, music)] {
        let mut q = p.clone();
        q.comp_mut(cid).unwrap().layer_mut(soloed).unwrap().switches.solo = true;
        assert_eq!(q.silent_layers(cid, q.comp(cid).unwrap(), &q.heard_layers()).into_iter().collect::<Vec<LayerId>>(), vec![rest]);
    }
}

/// Plug-in effects read their input at other times (plug-in API 2), so a precomp under one is
/// re-rendered over its whole span; built-in effects only when they are on the list.
#[test]
fn plug_in_effects_read_other_frames() {
    assert!(crate::reads_other_frames("org.example.glow"));
    assert!(crate::reads_other_frames("ec.time.echo"));
    assert!(!crate::reads_other_frames("ec.blur.gaussian"));
}

/// An item whose parent is a composition (or an item that is not there) lists under no folder, and
/// the AEP export lost it without a word (review finding): opening such a file moves it to the top
/// level and says so; a parent that is a folder stays.
#[test]
fn an_item_inside_a_non_folder_moves_to_the_top_level_when_a_project_is_opened() {
    let mut p = Project::default();
    let folder = p.add_item("Folder", Default::default(), None, ItemKind::Folder);
    let comp = p.add_item(
        "Comp",
        Default::default(),
        None,
        ItemKind::Comp(std::sync::Arc::new(Comp::new(64, 36, FrameRate::new(25, 1), Tick::from_seconds_f64(1.0)))),
    );
    let inside_folder = p.add_item("Fine", Default::default(), Some(folder), ItemKind::Folder);
    let inside_comp = p.add_item("Inside comp", Default::default(), None, ItemKind::Folder);
    let orphan = p.add_item("Orphan", Default::default(), None, ItemKind::Folder);
    if let Some(i) = p.item_mut(inside_comp) {
        i.parent = Some(comp);
    }
    if let Some(i) = p.item_mut(orphan) {
        i.parent = Some(crate::ItemId(9_999));
    }
    let (q, notes) = Project::from_json_repaired(&p.to_json()).expect("opens");
    assert_eq!(q.item(inside_folder).and_then(|i| i.parent), Some(folder));
    assert_eq!(q.item(inside_comp).and_then(|i| i.parent), None);
    assert_eq!(q.item(orphan).and_then(|i| i.parent), None);
    assert_eq!(notes.len(), 2, "{notes:?}");
    assert!(notes.iter().all(|n| n.contains("not a folder")), "{notes:?}");
    // Nothing to repair in a sound project.
    assert!(Project::from_json_repaired(&q.to_json()).expect("opens").1.is_empty());
}

/// #3324: a layer of [`project_with_layer`] with `add` applied and three keys (at 1, 2 and 3 s)
/// with `values` on the property at `path`, and that layer with the middle key's value `mid`.
fn middle_key_edit(add: impl FnOnce(&mut Ids, &mut crate::Layer), path: &str, values: [Value; 3], mid: Value) -> (crate::Layer, crate::Layer) {
    let (mut p, cid) = project_with_layer();
    let mut l = p.comp(cid).unwrap().layers[0].clone();
    add(&mut Ids(&mut p.next_id), &mut l);
    l.props.prop_mut(path).unwrap_or_else(|| panic!("{path}")).keys =
        values.into_iter().zip([1.0, 2.0, 3.0]).map(|(v, s)| Keyframe::new(Tick::from_seconds_f64(s), v)).collect();
    let mut edited = l.clone();
    edited.props.prop_mut(path).unwrap().keys[1].value = mid;
    (l, edited)
}

/// The span of a middle-key edit: from the first key to the last (1 to 3 s).
fn first_to_last_key() -> Option<Vec<(Tick, Tick)>> {
    Some(vec![(Tick::from_seconds_f64(1.0), Tick::from_seconds_f64(3.0))])
}

/// An effect instance as `effects::instantiate` builds it: `params` under its group, or under a
/// `master` group (Exposure's `master/exposure`).
fn effect(ids: &mut Ids, id: &str, params: Vec<crate::Property>) -> crate::PropGroup {
    let mut fx = ids.group(id, id);
    fx.kind = crate::GroupKind::Effect { effect: id.into() };
    for p in params {
        fx.children.push(p.into());
    }
    fx
}

fn exposure(ids: &mut Ids) -> crate::PropGroup {
    let master = ids.group("master", "Master").with(ids.prop("exposure", "Exposure", Value::Scalar(0.0)));
    let mut fx = effect(ids, "ec.color.exposure", vec![]);
    fx.children.push(master.into());
    fx
}

fn add_effect(l: &mut crate::Layer, fx: crate::PropGroup) {
    l.props.sub_mut("effects").unwrap().children.push(fx.into());
}

fn scalars(v: [f64; 3]) -> [Value; 3] {
    v.map(Value::Scalar)
}

/// A key edit on an effect parameter (inside the effect's Master group) keeps the frames
/// outside its keys, as a Transform key edit does (#3324).
#[test]
fn an_effect_key_edit_reaches_only_its_keys() {
    let (a, b) = middle_key_edit(|ids, l| add_effect(l, exposure(ids)), "effects/#1/master/exposure", scalars([0.0, 1.0, 0.0]), Value::Scalar(2.0));
    assert_eq!(b.keyframe_spans(&a, true, false), first_to_last_key());
    assert_eq!(a.keyframe_spans(&b, true, false), first_to_last_key());
}

/// A key edit on a mask's path (a shape value) keeps the frames outside its keys (#3324).
#[test]
fn a_mask_path_key_edit_reaches_only_its_keys() {
    let path = |w: f64| Value::Path(ShapePath::rect([0.0, 0.0], w, 10.0));
    let (a, b) = middle_key_edit(
        |ids, l| {
            let m = build::mask(ids, "Mask 1", ShapePath::rect([0.0, 0.0], 10.0, 10.0), MaskMode::Add, [255, 0, 0]);
            l.props.sub_mut("masks").unwrap().children.push(m.into());
        },
        "masks/mask#1/path",
        [path(10.0), path(20.0), path(10.0)],
        path(40.0),
    );
    assert_eq!(b.keyframe_spans(&a, true, false), first_to_last_key());
}

/// A key edit on a shape's path inside a shape group keeps the frames outside its keys (#3324).
#[test]
fn a_shape_path_key_edit_reaches_only_its_keys() {
    let path = |w: f64| Value::Path(ShapePath::ellipse([0.0, 0.0], w, 10.0));
    let (a, b) = middle_key_edit(
        |ids, l| {
            let shape = build::shape_path(ids, ShapePath::ellipse([0.0, 0.0], 10.0, 10.0));
            let fill = build::shape_fill(ids, [1.0, 0.0, 0.0, 1.0]);
            let group = build::shape_group(ids, "Group 1", vec![shape, fill]);
            l.props.children.push(build::contents(ids).with(group).into());
        },
        "contents/group#1/contents/path#1/path",
        [path(10.0), path(20.0), path(10.0)],
        path(40.0),
    );
    assert_eq!(b.keyframe_spans(&a, true, false), first_to_last_key());
}

/// A key edit on a text animator's property keeps the frames outside its keys, and so does one
/// on Source Text (hold keys) (#3324).
#[test]
fn text_key_edits_reach_only_their_keys() {
    let text = |ids: &mut Ids, l: &mut crate::Layer| {
        let mut t = build::text(ids, effectcraft_keyframe::TextDoc::plain("Hello"));
        let opacity = build::text_anim_prop(ids, "opacity").unwrap();
        let animator = build::text_animator(ids, "Animator 1", vec![opacity]);
        t.sub_mut("animators").unwrap().children.push(animator.into());
        l.props.children.push(t.into());
    };
    let (a, b) = middle_key_edit(text, "text/animators/animator#1/properties/opacity", scalars([100.0, 0.0, 100.0]), Value::Scalar(50.0));
    assert_eq!(b.keyframe_spans(&a, true, false), first_to_last_key());
    let doc = |s: &str| Value::Text(Box::new(effectcraft_keyframe::TextDoc::plain(s)));
    let (a, b) = middle_key_edit(text, "text/sourceText", [doc("a"), doc("b"), doc("c")], doc("B"));
    assert_eq!(b.keyframe_spans(&a, true, false), first_to_last_key());
}

/// An expression anywhere on the layer can read the keyed property at other times: an effect key
/// edit then reaches the layer's whole span (#3324).
#[test]
fn an_expression_elsewhere_on_the_layer_keeps_the_whole_span() {
    let (a, b) = middle_key_edit(
        |ids, l| {
            add_effect(l, exposure(ids));
            l.props.prop_mut("transform/rotation").unwrap().expr = Some(crate::Expression { text: "time * 10".into(), enabled: true });
        },
        "effects/#1/master/exposure",
        scalars([0.0, 1.0, 0.0]),
        Value::Scalar(2.0),
    );
    assert_eq!(b.keyframe_spans(&a, true, false), None);
}

/// A change of the tree's structure reaches the layer's whole span: an effect added (even beside
/// a key edit), and a group header (an effect switched off) (#3324).
#[test]
fn an_effect_added_or_switched_off_keeps_the_whole_span() {
    let (a, b) = middle_key_edit(|ids, l| add_effect(l, exposure(ids)), "effects/#1/master/exposure", scalars([0.0, 1.0, 0.0]), Value::Scalar(2.0));
    let mut added = a.clone();
    add_effect(&mut added, effect(&mut Ids(&mut 10_000), "ec.channel.invert", vec![]));
    assert_eq!(added.keyframe_spans(&a, true, false), None);
    let mut both = b.clone();
    add_effect(&mut both, effect(&mut Ids(&mut 10_000), "ec.channel.invert", vec![]));
    assert_eq!(both.keyframe_spans(&a, true, false), None);
    let mut off = b.clone();
    off.props.group_mut("effects/#1").unwrap().enabled = false;
    assert_eq!(off.keyframe_spans(&a, true, false), None);
}

/// Effects that read their own parameters at other times (Write-on's dabs since the layer's
/// start, Radio Waves' birth values, Transform's own shutter) reach the layer's whole span with a
/// key edit of theirs, not with one of another effect on the layer (#3324).
#[test]
fn effects_reading_their_parameters_elsewhen_keep_the_whole_span() {
    let v2 = |x: f64| Value::Vec2([x, 0.0]);
    let write_on = |ids: &mut Ids| {
        let p = ids.prop("brushPosition", "Brush Position", v2(0.0));
        effect(ids, "ec.generate.writeon", vec![p])
    };
    let (a, b) = middle_key_edit(|ids, l| add_effect(l, write_on(ids)), "effects/#1/brushPosition", [v2(0.0), v2(5.0), v2(0.0)], v2(9.0));
    assert_eq!(b.keyframe_spans(&a, true, false), None);
    let radio = |ids: &mut Ids| {
        let p = ids.prop("producePoint", "Producer Point", v2(0.0));
        effect(ids, "ec.generate.radiowaves", vec![p])
    };
    let (a, b) = middle_key_edit(|ids, l| add_effect(l, radio(ids)), "effects/#1/producePoint", [v2(0.0), v2(5.0), v2(0.0)], v2(9.0));
    assert_eq!(b.keyframe_spans(&a, true, false), None);
    // Transform: its own Shutter Angle reaches past the comp's shutter, the comp's doesn't unless
    // the layer is stretched (Transform samples its shutter in layer time).
    for (comp_shutter, stretch, want) in [(false, 100.0, None), (true, 100.0, first_to_last_key()), (true, 200.0, None)] {
        let transform = |ids: &mut Ids, l: &mut crate::Layer| {
            let shutter = ids.prop("useCompositionShutterAngle", "Use Composition's Shutter Angle", Value::Bool(comp_shutter));
            let rotation = ids.prop("rotation", "Rotation", Value::Scalar(0.0));
            add_effect(l, effect(ids, "ec.distort.transform", vec![shutter, rotation]));
            l.stretch = stretch;
        };
        let (a, b) = middle_key_edit(transform, "effects/#1/rotation", scalars([0.0, 10.0, 0.0]), Value::Scalar(20.0));
        let want = want.map(|_| vec![(a.comp_time(Tick::from_seconds_f64(1.0)), a.comp_time(Tick::from_seconds_f64(3.0)))]);
        assert_eq!(b.keyframe_spans(&a, true, false), want, "comp shutter {comp_shutter}, stretch {stretch}");
    }
    // Another effect's key edit beside a Write-on.
    let (a, b) = middle_key_edit(
        |ids, l| {
            add_effect(l, write_on(ids));
            add_effect(l, exposure(ids));
        },
        "effects/#2/master/exposure",
        scalars([0.0, 1.0, 0.0]),
        Value::Scalar(2.0),
    );
    assert_eq!(b.keyframe_spans(&a, true, false), first_to_last_key());
}

/// The Essential Properties feed a nested comp, and audio something draws (a waveform reads a
/// stretch of it): a key edit there reaches the layer's whole span. Audio nothing draws doesn't
/// count at all (#3324).
#[test]
fn essential_properties_and_heard_audio_keep_the_whole_span() {
    let (a, b) = middle_key_edit(
        |ids, l| {
            let mut g = ids.group("essentialProperties", "Essential Properties").with(ids.prop("control1", "Size", Value::Scalar(0.0)));
            g.kind = crate::GroupKind::Essential { overridden: vec![] };
            l.props.children.push(g.into());
        },
        "essentialProperties/control1",
        scalars([0.0, 1.0, 0.0]),
        Value::Scalar(2.0),
    );
    assert_eq!(b.keyframe_spans(&a, true, false), None);
    let (a, b) = middle_key_edit(
        |ids, l| l.props.children.push(build::audio(ids).into()),
        "audio/levels",
        [Value::Vec2([0.0; 2]), Value::Vec2([-6.0; 2]), Value::Vec2([0.0; 2])],
        Value::Vec2([-12.0; 2]),
    );
    assert_eq!(b.keyframe_spans(&a, true, true), None, "heard");
    assert_eq!(b.keyframe_spans(&a, true, false), Some(vec![]), "not heard: the picture is the same");
}

/// A property tree nested deeper than the walk goes counts as changed everywhere (#3324).
#[test]
fn a_deeply_nested_key_edit_keeps_the_whole_span() {
    // 100 groups around a property keyed at 1, 2 and 3 s with `mid` in the middle (the same uids
    // each time).
    let deep = |mid: f64| {
        let mut n = 50_000;
        let ids = &mut Ids(&mut n);
        let mut x = ids.prop("x", "X", Value::Scalar(0.0));
        x.keys = [0.0, mid, 0.0].into_iter().zip([1.0, 2.0, 3.0]).map(|(v, s)| Keyframe::new(Tick::from_seconds_f64(s), Value::Scalar(v))).collect();
        let mut g = ids.group("g", "G").with(x);
        for _ in 0..100 {
            g = ids.group("g", "G").with(g);
        }
        g
    };
    let (p, cid) = project_with_layer();
    let (mut a, mut b) = (p.comp(cid).unwrap().layers[0].clone(), p.comp(cid).unwrap().layers[0].clone());
    a.props.children.push(deep(1.0).into());
    b.props.children.push(deep(2.0).into());
    assert_eq!(b.keyframe_spans(&a, true, false), None);
    // (a Transform key edit beside the same deep tree keeps its span)
    let (a, b) = middle_key_edit(|_, l| l.props.children.push(deep(1.0).into()), "transform/opacity", scalars([100.0, 0.0, 100.0]), Value::Scalar(50.0));
    assert_eq!(b.keyframe_spans(&a, true, false), first_to_last_key());
}

/// On a reversed layer (negative stretch) the first keys play last: an edit of the first key
/// reaches from the next keys' time to the out point, not from the in point (#3324).
#[test]
fn a_first_key_edit_on_a_reversed_layer_reaches_the_out_point() {
    // (a layer from 0 to 10 s reversed: start 10 s, so layer time t shows at 10 - t)
    let reversed = |_: &mut Ids, l: &mut crate::Layer| (l.stretch, l.start_time) = (-100.0, Tick::from_seconds_f64(10.0));
    let (a, _) = middle_key_edit(reversed, "transform/opacity", scalars([100.0, 0.0, 100.0]), Value::Scalar(0.0));
    let mut b = a.clone();
    b.props.prop_mut("transform/opacity").unwrap().keys[0].value = Value::Scalar(50.0);
    let spans = b.keyframe_spans(&a, true, false).unwrap();
    let (s, e) = spans[0];
    assert_eq!(spans.len(), 1);
    // Keys at layer 1, 2, 3 s show at 9, 8, 7 s; the first key's edit reaches 7 s (its second key
    // after) through the out point.
    assert_eq!((s, e), (Tick::from_seconds_f64(7.0), a.out_point), "{spans:?}");
}

/// The renderer reads some properties at other times: Position under Auto-Orient Along Path (a
/// still layer keeps its last direction of travel) and a model's animation clip (it plays from the
/// key that chose it). Their key edits reach the layer's whole span (#3324).
#[test]
fn properties_the_renderer_reads_elsewhen_keep_the_whole_span() {
    let v3 = |x: f64| Value::Vec3([x, 0.0, 0.0]);
    let (a, b) = middle_key_edit(|_, l| l.auto_orient = crate::AutoOrient::AlongPath, "transform/position", [v3(0.0), v3(5.0), v3(9.0)], v3(2.0));
    assert_eq!(b.keyframe_spans(&a, true, false), None);
    let (a, b) = middle_key_edit(|_, _| {}, "transform/position", [v3(0.0), v3(5.0), v3(9.0)], v3(2.0));
    assert_eq!(b.keyframe_spans(&a, true, false), first_to_last_key(), "without Auto-Orient");
    let (a, b) = middle_key_edit(
        |ids, l| {
            let g = ids.group("geometryOptions", "Geometry Options").with(ids.prop("animation", "Animation", Value::Enum(0)));
            l.props.children.push(g.into());
        },
        "geometryOptions/animation",
        [Value::Enum(0), Value::Enum(1), Value::Enum(1)],
        Value::Enum(0),
    );
    assert_eq!(b.keyframe_spans(&a, true, false), None);
}
