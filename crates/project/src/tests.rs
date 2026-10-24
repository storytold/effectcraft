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
    assert_eq!(p.to_file_json_with_view(&crate::SavedView::default()).unwrap_err().to_string(), expected);
}

#[test]
fn saved_files_reopen_in_pretty_and_compact_form() {
    let (p, _cid) = project_with_layer();
    assert_eq!(Project::from_json(&p.to_file_json().unwrap()).unwrap(), p, "the pretty project file");
    assert_eq!(Project::from_json(&p.to_json_compact()).unwrap(), p, "compact JSON");
}

#[test]
fn project_files_remember_the_open_comps() {
    let (p, cid) = project_with_layer();
    let view = crate::SavedView { active_comp: Some(cid), open_comps: vec![cid], comp_times: [(cid, effectcraft_time::Tick(4_237_833_600))].into() };
    let text = p.to_file_json_with_view(&view).unwrap();
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
