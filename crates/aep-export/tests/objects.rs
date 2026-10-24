//! Track mattes, markers, cameras, lights and shape layers: written from projects built in code
//! and read back with `effectcraft-aep`.

mod common;

use common::*;
use effectcraft_aep::parse;
use effectcraft_aep_export::LossCategory;
use effectcraft_project::{LayerSource, MatteKind, Project, TrackMatte};
use effectcraft_time::FrameRate;

/// `ldta` of every layer of the first composition, top first.
fn ldtas(bytes: &[u8]) -> Vec<Vec<u8>> {
    let file = parse(bytes).expect("parse");
    let view = effectcraft_aep::Project::new(&file).expect("project");
    let comp = view.all_items().into_iter().find_map(|i| i.comp()).expect("comp");
    comp.layers().map(|l| l.chunk().child(b"ldta").and_then(|c| c.bytes()).expect("ldta").to_vec()).collect()
}

fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_be_bytes(b[at..at + 4].try_into().expect("4"))
}

fn two_solids() -> (Project, effectcraft_project::ItemId) {
    let mut p = Project::default();
    let c = add_comp(&mut p, "C", 640, 360, FrameRate::new(25, 1), 2.0, None);
    let s = add_solid(&mut p, "S", 640, 360, [1.0, 0.0, 0.0], None);
    add_layer(&mut p, c, "Target", LayerSource::Solid { item: s }, (640, 360));
    add_layer(&mut p, c, "Matte", LayerSource::Solid { item: s }, (640, 360));
    (p, c)
}

#[test]
fn track_mattes_are_written_with_mode_and_layer() {
    // 403-406: the mode at ldta@107 (1 alpha, 2 alpha inverted, 3 luma, 4 luma inverted) and
    // the matte layer's id at ldta@160, on the target layer.
    for (kind, mode) in [(MatteKind::Alpha, 1u8), (MatteKind::AlphaInverted, 2), (MatteKind::Luma, 3), (MatteKind::LumaInverted, 4)] {
        let (mut p, c) = two_solids();
        let matte_id = p.comp(c).expect("c").layers[0].id;
        with_layer(&mut p, c, 1, |l| l.track_matte = Some(TrackMatte { layer: matte_id, kind }));
        let r = export_all(&p);
        assert!(r.report.is_lossless(), "{:?}", r.report.losses);
        let l = ldtas(&r.bytes);
        // Top: Matte, then Target.
        assert_eq!((l[0][107], u32_at(&l[0], 160)), (0, 0));
        assert_eq!(l[1][107], mode);
        assert_eq!(u32_at(&l[1], 160), u32_at(&l[0], 0), "the matte layer's id");
    }
    // A matte below the target (407) is written as well; one that is not exported is reported.
    let (mut p, c) = two_solids();
    let target = p.comp(c).expect("c").layers[1].id;
    with_layer(&mut p, c, 0, |l| l.track_matte = Some(TrackMatte { layer: target, kind: MatteKind::Alpha }));
    let l = ldtas(&export_all(&p).bytes);
    assert_eq!((l[0][107], u32_at(&l[0], 160)), (1, u32_at(&l[1], 0)));
    let (mut p, c) = two_solids();
    with_layer(&mut p, c, 1, |l| l.track_matte = Some(TrackMatte { layer: effectcraft_project::LayerId(987_654), kind: MatteKind::Luma }));
    let r = export_all(&p);
    assert_eq!(r.report.count(LossCategory::TrackMattes), 1);
    assert_eq!(ldtas(&r.bytes)[1][107], 0);
}

fn marker(t: f64, comment: &str) -> effectcraft_project::Marker {
    effectcraft_project::Marker { time: secs(t), comment: comment.into(), ..Default::default() }
}

#[test]
fn markers_are_written_like_after_effects_stores_them() {
    let (mut p, c) = two_solids();
    with_comp(&mut p, c, |cc| {
        cc.markers = vec![
            effectcraft_project::Marker { label: effectcraft_color::Label::Yellow, ..marker(1.0, "M1") },
            effectcraft_project::Marker { duration: secs(0.5), ..marker(2.5, "M2") },
        ];
    });
    with_layer(&mut p, c, 0, |l| {
        l.markers = vec![
            effectcraft_project::Marker {
                duration: secs(1.0),
                chapter: "Chap".into(),
                url: "http://a.example/u".into(),
                frame_target: "_blank".into(),
                cue_point: Some(effectcraft_project::CuePoint {
                    name: "CueX".into(),
                    navigation: false,
                    params: vec![("speaker".into(), "A".into()), ("n".into(), "2".into())],
                }),
                ..marker(1.0, "LM")
            },
            effectcraft_project::Marker {
                cue_point: Some(effectcraft_project::CuePoint { name: "CueN".into(), navigation: true, params: Vec::new() }),
                ..marker(0.52, "Nav")
            },
        ];
    });
    let r = export_all(&p);
    assert!(r.report.is_lossless(), "{:?}", r.report.losses);
    assert_eq!(r.report.exported.markers, 4);
    let file = parse(&r.bytes).expect("parse");
    assert_eq!(effectcraft_aep::write(&file).expect("write"), r.bytes);
    let view = effectcraft_aep::Project::new(&file).expect("project");
    let comp = view.all_items().into_iter().find_map(|i| i.comp()).expect("comp");
    let cm = comp.markers();
    assert_eq!(
        cm.iter().map(|m| (m.time_units, m.comment.as_str(), m.duration_seconds, m.label)).collect::<Vec<_>>(),
        vec![
            (25_600, "M1", Some(0.0), Some(effectcraft_color::Label::ALL.iter().position(|l| *l == effectcraft_color::Label::Yellow).expect("label") as u8)),
            // EffectCraft's default marker label is Red, index 1 (After Effects' own default is 0, None).
            (64_000, "M2", Some(0.5), Some(1)),
        ]
    );
    let layer = comp.layers().next().expect("layer");
    let lm = layer.markers();
    assert_eq!(lm.len(), 2);
    // In time order: Nav at 0.52 s (13 312 units, 510), LM at 1 s.
    assert_eq!(
        (lm[0].time_units, lm[0].comment.as_str(), lm[0].cue_type, lm[0].cue_point_name.as_str()),
        (13_312, "Nav", Some(effectcraft_aep::CueType::Navigation), "CueN")
    );
    let m = &lm[1];
    assert_eq!(
        (m.time_units, m.comment.as_str(), m.chapter.as_str(), m.url.as_str(), m.frame_target.as_str(), m.cue_point_name.as_str()),
        (25_600, "LM", "Chap", "http://a.example/u", "_blank", "CueX")
    );
    assert_eq!((m.duration_seconds, m.cue_type), (Some(1.0), Some(effectcraft_aep::CueType::Event)));
    assert_eq!(m.cue_params, vec![("speaker".to_string(), "A".to_string()), ("n".to_string(), "2".to_string())]);
    // A protected marker is reported (the switch is not decoded).
    with_layer(&mut p, c, 1, |l| l.markers = vec![effectcraft_project::Marker { protected: true, ..marker(0.2, "P") }]);
    assert_eq!(export_all(&p).report.count(LossCategory::Markers), 1);
}

#[test]
fn marker_fixtures_are_written_as_after_effects_wrote_them() {
    // 122 (composition), 123, 249, 347-349 (layer): the reader sees the same markers after an
    // import of the fixture, its markers put back from the reader's view (the importer does not
    // read markers yet), and an export.
    for name in [
        "122-marker-comp-1s.aep",
        "123-marker-layer-1s.aep",
        "249-marker-layer-full.aep",
        "347-marker-layer-label3.aep",
        "348-marker-layer-navigation-cue.aep",
        "349-marker-layer-event-cue-params.aep",
    ] {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../aep/tests/fixtures").join(name);
        let bytes = std::fs::read(&path).expect("fixture");
        let file = parse(&bytes).expect("parse");
        let view = effectcraft_aep::Project::new(&file).expect("project");
        let comp = view.all_items().into_iter().find_map(|i| i.comp()).expect("comp");
        let to_ec = |m: &effectcraft_aep::Marker| effectcraft_project::Marker {
            time: secs(m.seconds.unwrap_or(0.0)),
            duration: secs(m.duration_seconds.unwrap_or(0.0)),
            comment: m.comment.clone(),
            label: effectcraft_color::Label::ALL.get(usize::from(m.label.unwrap_or(0))).copied().unwrap_or_default(),
            chapter: m.chapter.clone(),
            url: m.url.clone(),
            frame_target: m.frame_target.clone(),
            cue_point: (!m.cue_point_name.is_empty() || m.cue_type == Some(effectcraft_aep::CueType::Navigation) || !m.cue_params.is_empty()).then(|| {
                effectcraft_project::CuePoint {
                    name: m.cue_point_name.clone(),
                    navigation: m.cue_type == Some(effectcraft_aep::CueType::Navigation),
                    params: m.cue_params.clone(),
                }
            }),
            protected: false,
        };
        let comp_markers: Vec<_> = comp.markers().iter().map(to_ec).collect();
        let layer_markers: Vec<_> = comp.layers().next().map(|l| l.markers().iter().map(to_ec).collect()).unwrap_or_default();
        let mut p = Project::default();
        let none = |_: &str| false;
        effectcraft_aep_import::import(
            &mut p,
            &bytes,
            &effectcraft_aep_import::ImportOptions { allow_losses: true, ..effectcraft_aep_import::ImportOptions::new(&none) },
        )
        .expect(name);
        let cid = p.comps().next().map(|(id, _)| *id).expect("comp");
        with_comp(&mut p, cid, |c| c.markers = comp_markers);
        with_layer(&mut p, cid, 0, |l| l.markers = layer_markers);
        let r = export_all(&p);
        let out = parse(&r.bytes).expect("parse");
        let oview = effectcraft_aep::Project::new(&out).expect("project");
        let ocomp = oview.all_items().into_iter().find_map(|i| i.comp()).expect("comp");
        assert_eq!(ocomp.markers(), comp.markers(), "{name}");
        assert_eq!(ocomp.layers().next().map(|l| l.markers()), comp.layers().next().map(|l| l.markers()), "{name}");
    }
}

fn with_props(p: &mut Project, c: effectcraft_project::ItemId, idx: usize, set: &[(&str, effectcraft_keyframe::Value)]) {
    with_layer(p, c, idx, |l| {
        for (path, v) in set {
            l.props.prop_mut(path).unwrap_or_else(|| panic!("{path}")).value = v.clone();
        }
    });
}

fn first_comp_layers(bytes: &[u8], mut f: impl FnMut(&effectcraft_aep::Layer<'_, '_>)) {
    let file = parse(bytes).expect("parse");
    assert_eq!(effectcraft_aep::write(&file).expect("write"), bytes);
    let view = effectcraft_aep::Project::new(&file).expect("project");
    let comp = view.all_items().into_iter().find_map(|i| i.comp()).expect("comp");
    for l in comp.layers() {
        f(&l);
    }
}

#[test]
fn cameras_are_written_with_position_point_of_interest_and_options() {
    use effectcraft_keyframe::Value;
    let mut p = Project::default();
    let c = add_comp(&mut p, "C", 1920, 1080, FrameRate::new(25, 1), 2.0, None);
    add_layer(&mut p, c, "Cam", LayerSource::Camera, (1920, 1080));
    with_props(
        &mut p,
        c,
        0,
        &[
            ("transform/position", Value::Vec3([100.0, 200.0, -1000.0])),
            ("transform/poi", Value::Vec3([960.0, 540.0, 0.0])),
            ("cameraOptions/dof", Value::Bool(true)),
            ("cameraOptions/focusDistance", Value::Scalar(800.0)),
            ("cameraOptions/aperture", Value::Scalar(30.0)),
            ("cameraOptions/zoom", Value::Scalar(1500.0)),
            ("cameraOptions/blurLevel", Value::Scalar(80.0)),
        ],
    );
    let r = export_all(&p);
    assert!(r.report.is_lossless(), "{:?}", r.report.losses);
    assert_eq!(r.report.exported.cameras, 1);
    first_comp_layers(&r.bytes, |l| {
        let h = l.header().expect("header");
        assert_eq!(h.layer_type, effectcraft_aep::LayerType::Camera);
        assert!(h.is_three_d() && h.is_auto_oriented(), "two-node camera");
        let pos = l.camera_position().expect("position");
        assert_eq!((pos.stored, pos.auto_state), ([100.0, 200.0, -1000.0], false));
        assert_eq!(l.camera_point_of_interest(), Some([960.0, 540.0, 0.0]));
        assert_eq!(l.camera_depth_of_field().map(|t| t.value), Some(true));
        assert_eq!(l.camera_focus_distance().map(|t| t.value), Some(800.0));
        assert_eq!(l.camera_aperture().map(|t| t.value), Some(30.0));
        assert_eq!(l.camera_zoom().map(|t| t.value), Some(1500.0));
    });
    // One-node (auto-orient off): bit 0x40 clear (251).
    with_layer(&mut p, c, 0, |l| l.auto_orient = effectcraft_project::AutoOrient::Off);
    first_comp_layers(&export_all(&p).bytes, |l| assert!(!l.header().expect("h").is_auto_oriented()));
}

#[test]
fn spot_and_point_lights_are_written_with_their_options() {
    use effectcraft_keyframe::Value;
    use effectcraft_project::LightKind;
    let mut p = Project::default();
    let c = add_comp(&mut p, "C", 1920, 1080, FrameRate::new(25, 1), 2.0, None);
    add_layer(&mut p, c, "Spot", LayerSource::Light { kind: LightKind::Spot }, (1920, 1080));
    add_layer(&mut p, c, "Point", LayerSource::Light { kind: LightKind::Point }, (1920, 1080));
    // Layer 0 is the point light (added last, on top), 1 the spot.
    with_props(
        &mut p,
        c,
        1,
        &[
            ("lightOptions/intensity", Value::Scalar(50.0)),
            ("lightOptions/color", Value::Color([1.0, 0.0, 0.0, 1.0])),
            ("lightOptions/coneAngle", Value::Scalar(60.0)),
            ("lightOptions/coneFeather", Value::Scalar(30.0)),
            ("transform/position", Value::Vec3([100.0, 50.0, -500.0])),
        ],
    );
    let r = export_all(&p);
    assert!(r.report.is_lossless(), "{:?}", r.report.losses);
    assert_eq!(r.report.exported.lights, 2);
    let mut seen = Vec::new();
    first_comp_layers(&r.bytes, |l| {
        let h = l.header().expect("header");
        assert_eq!(h.layer_type, effectcraft_aep::LayerType::Light);
        seen.push(h.light_type());
        match h.light_type() {
            Some(effectcraft_aep::LightType::Spot) => {
                assert_eq!(l.light_intensity().map(|t| t.value), Some(50.0));
                assert_eq!(l.light_color().map(|t| t.value), Some([1.0, 1.0, 0.0, 0.0]));
                assert_eq!(l.light_cone_angle().map(|t| t.value), Some(60.0));
                assert_eq!(l.light_cone_feather().map(|t| t.value), Some(30.0));
                assert_eq!(l.camera_position().map(|p| p.stored), Some([100.0, 50.0, -500.0]));
                assert_eq!(l.camera_point_of_interest(), Some([960.0, 540.0, 0.0]));
            }
            Some(effectcraft_aep::LightType::Point) => {
                assert_eq!(l.light_intensity().map(|t| t.value), Some(100.0));
            }
            other => panic!("{other:?}"),
        }
    });
    assert_eq!(seen.len(), 2);
    // Parallel and ambient lights, and unsampled options, are reported.
    let mut q = Project::default();
    let c = add_comp(&mut q, "C", 640, 360, FrameRate::new(25, 1), 1.0, None);
    add_layer(&mut q, c, "Par", LayerSource::Light { kind: LightKind::Parallel }, (640, 360));
    add_layer(&mut q, c, "Spot", LayerSource::Light { kind: LightKind::Spot }, (640, 360));
    with_props(&mut q, c, 0, &[("lightOptions/castsShadows", Value::Bool(true))]);
    assert_eq!(export_all(&q).report.count(LossCategory::Lights), 2);
}

#[test]
fn shape_layers_are_written_with_their_items() {
    use effectcraft_keyframe::{ShapePath, Value};
    use effectcraft_project::build;
    let mut p = Project::default();
    let c = add_comp(&mut p, "C", 1920, 1080, FrameRate::new(25, 1), 2.0, None);
    add_layer(&mut p, c, "S", LayerSource::Shape, (1920, 1080));
    let mut next = p.next_id;
    {
        let mut ids = build::Ids(&mut next);
        let mut rect = build::shape_rect(&mut ids, [400.0, 300.0], [10.0, -20.0], 15.0);
        rect.name = "Rectangle Path 1".into();
        let mut fill = build::shape_fill(&mut ids, [0.0, 1.0, 0.0, 1.0]);
        if let Some(r) = fill.get_mut("rule") {
            r.value = Value::Enum(1);
        }
        if let Some(o) = fill.get_mut("opacity") {
            o.value = Value::Scalar(50.0);
        }
        let mut stroke = build::shape_stroke(&mut ids, [1.0, 0.0, 0.0, 1.0], 6.0);
        if let Some(cap) = stroke.get_mut("cap") {
            cap.value = Value::Enum(1);
        }
        let mut g1 = build::shape_group(&mut ids, "Group 1", vec![rect, stroke, fill]);
        if let Some(t) = g1.sub_mut("transform") {
            t.get_mut("position").expect("pos").value = Value::Vec2([50.0, 50.0]);
            t.get_mut("scale").expect("scale").value = Value::Vec2([50.0, 80.0]);
            t.get_mut("rotation").expect("rot").value = Value::Scalar(30.0);
        }
        let ellipse = build::shape_ellipse(&mut ids, [500.0, 200.0], [100.0, 50.0]);
        let path = build::shape_path(
            &mut ids,
            ShapePath {
                vertices: vec![[0.0, -100.0], [100.0, 100.0], [-100.0, 100.0]],
                in_tangents: vec![[-10.0, -20.0], [30.0, 20.0], [-60.0, 10.0]],
                out_tangents: vec![[40.0, 0.0], [-20.0, 30.0], [0.0, -50.0]],
                closed: true,
                feather: Vec::new(),
            },
        );
        let fill2 = build::shape_fill(&mut ids, [0.0, 0.0, 1.0, 1.0]);
        let inner = build::shape_group(&mut ids, "Inner", vec![ellipse]);
        let g2 = build::shape_group(&mut ids, "Group 2", vec![inner, path, fill2]);
        let star = build::shape_star(&mut ids, true, 5.0, [0.0, 0.0], 100.0, 50.0);
        with_layer(&mut p, c, 0, |l| {
            let contents = l.props.sub_mut("contents").expect("contents");
            contents.children = vec![g1.into(), g2.into(), star.into()];
        });
    }
    p.next_id = next;
    let r = export_all(&p);
    // Everything is written, the polystar too.
    assert_eq!(r.report.count(LossCategory::ShapeLayers), 0, "{:?}", r.report.losses);
    assert_eq!(r.report.exported.shape_layers, 1);
    first_comp_layers(&r.bytes, |l| {
        assert_eq!(l.header().expect("h").layer_type, effectcraft_aep::LayerType::Shape);
        let groups = l.shape_groups();
        assert_eq!(groups.len(), 2, "{groups:?}");
        let names: Vec<_> = groups.iter().map(|g| g.name.clone()).collect();
        assert_eq!(names, vec![Some("Group 1".to_string()), Some("Group 2".to_string())]);
        let s = format!("{groups:?}");
        // The path box equals the one After Effects stored for the same path (240).
        for want in [
            "400.0, 300.0",
            "[10.0, -20.0]",
            "50.0, 50.0",
            "width: Some(Tracked { value: 6.0",
            "cap: Tracked { value: Round",
            "bbox: [-160.0, -120.0, 130.0, 130.0]",
        ] {
            assert!(s.contains(want), "{want} in {s}");
        }
    });
}

#[test]
fn hostile_objects_never_panic() {
    use effectcraft_keyframe::Value;
    use effectcraft_project::{LightKind, build};
    let mut p = Project::default();
    let c = add_comp(&mut p, "C", 640, 360, FrameRate::new(25, 1), 2.0, None);
    add_layer(&mut p, c, "S", LayerSource::Shape, (640, 360));
    let mut next = p.next_id;
    {
        let mut ids = build::Ids(&mut next);
        // 200 nested groups, a NaN rectangle, an empty path.
        let bad = build::shape_rect(&mut ids, [f64::NAN, 1e300], [0.0, 0.0], f64::INFINITY);
        let mut inner = build::shape_group(&mut ids, "deep", vec![bad]);
        for i in 0..200 {
            inner = build::shape_group(&mut ids, &format!("g{i}"), vec![inner]);
        }
        let empty = build::shape_path(&mut ids, effectcraft_keyframe::ShapePath::default());
        with_layer(&mut p, c, 0, |l| l.props.sub_mut("contents").expect("contents").children = vec![inner.into(), empty.into()]);
    }
    p.next_id = next;
    add_layer(&mut p, c, "Cam", LayerSource::Camera, (640, 360));
    with_props(&mut p, c, 0, &[("cameraOptions/zoom", Value::Scalar(f64::NAN)), ("transform/position", Value::Vec3([f64::INFINITY, 0.0, 0.0]))]);
    add_layer(&mut p, c, "L", LayerSource::Light { kind: LightKind::Spot }, (640, 360));
    with_props(&mut p, c, 0, &[("lightOptions/color", Value::Color([f64::NAN, 2.0, -1.0, 1.0])), ("lightOptions/intensity", Value::Scalar(-1e9))]);
    with_layer(&mut p, c, 0, |l| {
        l.markers = (0..500).map(|i| marker(f64::from(i) * 1e7, "x")).collect();
        l.track_matte = Some(TrackMatte { layer: l.id, kind: MatteKind::Alpha });
    });
    let r = export_all(&p);
    let file = parse(&r.bytes).expect("parse");
    assert_eq!(effectcraft_aep::write(&file).expect("write"), r.bytes);
    assert!(r.report.count(LossCategory::ShapeLayers) >= 2, "{:?}", r.report.losses);
    assert!(r.report.count(LossCategory::TrackMattes) == 1);
}

#[test]
fn camera_fixtures_come_back_without_a_loss() {
    // The exporter writes every camera option, Blur Level 100 too; importing the export must not
    // report it (it was reported as "not imported" before the importer and exporter lines met).
    use effectcraft_aep_import::{ImportOptions, import};
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../aep/tests/fixtures");
    let none = |_: &str| false;
    for name in ["251-camera-one-node.aep", "256-camera-two-node-default.aep", "342-camera-dof-800-30.aep", "537-camera-default-on-3d-rotx.aep"] {
        let bytes = std::fs::read(dir.join(name)).expect(name);
        let mut p = Project::default();
        let r1 = import(&mut p, &bytes, &ImportOptions { allow_losses: true, ..ImportOptions::new(&none) }).expect("import");
        assert!(r1.report.losses.is_empty(), "{name}: {:?}", r1.report.losses);
        let out = export_all(&p);
        assert!(out.report.is_lossless(), "{name}: {:?}", out.report.losses);
        let mut p2 = Project::default();
        let r2 = import(&mut p2, &out.bytes, &ImportOptions { allow_losses: true, ..ImportOptions::new(&none) }).expect("reimport");
        assert!(r2.report.losses.is_empty(), "{name}: {:?}", r2.report.losses);
        assert_eq!(r2.report.imported.cameras, 1, "{name}");
    }
}

/// The static value records below the shape contents of every layer, by (layer index, path of
/// item match names, record match name).
fn shape_records(bytes: &[u8]) -> std::collections::BTreeMap<(usize, String), effectcraft_aep::Chunk<'static>> {
    fn walk(
        cs: &[effectcraft_aep::Chunk<'_>],
        path: &str,
        li: usize,
        inside: bool,
        out: &mut std::collections::BTreeMap<(usize, String), effectcraft_aep::Chunk<'static>>,
    ) {
        let mut seen: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
        for (i, c) in cs.iter().enumerate() {
            let name = if c.is(b"tdmn") { c.bytes().map(|b| String::from_utf8_lossy(b.split(|x| *x == 0).next().unwrap_or(b"")).to_string()) } else { None };
            let Some(name) = name else { continue };
            let Some(node) = cs.get(i + 1) else { continue };
            let n = seen.entry(name.clone()).or_insert(0);
            let key = format!("{path}/{name}#{n}");
            *n += 1;
            let here = inside || name == "ADBE Root Vectors Group";
            if here && node.is_list(b"tdbs") && node.child(b"cdat").is_some() {
                out.insert((li, key.clone()), node.clone().into_owned());
            }
            if node.is_list(b"tdgp") {
                walk(node.children(), &key, li, here, out);
            }
        }
    }
    let f = parse(bytes).expect("parse");
    let mut out = std::collections::BTreeMap::new();
    let view = effectcraft_aep::Project::new(&f).expect("project");
    for item in view.all_items() {
        let Some(comp) = item.comp() else { continue };
        for (li, layer) in comp.layers().enumerate() {
            if let Some(g) = layer.chunk().child_list(b"tdgp") {
                walk(g.children(), "", li, false, &mut out);
            }
        }
    }
    out
}

#[test]
fn every_shape_value_record_is_written_as_after_effects_wrote_it() {
    // Every static record of a shape fixture comes back with After Effects' bytes, built from the
    // record of its property's type (R46).
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../aep/tests/fixtures");
    let mut names: Vec<String> = std::fs::read_dir(&dir)
        .expect("fixtures")
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|n| n.ends_with(".aep") && n.contains("shape"))
        .collect();
    names.sort();
    let mut compared = 0;
    for name in &names {
        let bytes = std::fs::read(dir.join(name)).expect("fixture");
        let mut p = Project::default();
        let none = |_: &str| false;
        let imported = effectcraft_aep_import::import(
            &mut p,
            &bytes,
            &effectcraft_aep_import::ImportOptions { allow_losses: true, ..effectcraft_aep_import::ImportOptions::new(&none) },
        )
        .expect("import");
        if imported.report.count(effectcraft_aep_import::LossCategory::ShapeLayers) > 0 {
            continue;
        }
        let r = export_all(&p);
        let (a, b) = (shape_records(&bytes), shape_records(&r.bytes));
        for (k, rec) in &a {
            // A record After Effects keeps for a value set to its default (239: Miter Limit 4)
            // is not needed: absent = default.
            let first = rec.child(b"cdat").and_then(|c| c.bytes()).and_then(|d| d.get(..8)).and_then(|b| b.try_into().ok()).map(f64::from_be_bytes);
            let explicit_default =
                [("ADBE Vector Stroke Miter Limit#0", 4.0), ("ADBE Vector Grad HiLite Angle#0", 0.0), ("ADBE Vector Grad HiLite Length#0", 0.0)]
                    .iter()
                    .any(|(n, v)| k.1.ends_with(n) && first == Some(*v));
            // A polygon has no inner radius or roundness in EffectCraft (After Effects keeps their
            // records, 331; a polygon ignores them).
            let polygon_inner = name.contains("polygon") && k.1.contains("ADBE Vector Star Inner");
            let Some(got) = b.get(k) else {
                assert!(explicit_default || polygon_inner, "{name}: {k:?} not written");
                continue;
            };
            assert_eq!(got, rec, "{name}: {k:?}");
            compared += 1;
        }
        for k in b.keys() {
            assert!(a.contains_key(k), "{name}: {k:?} written but After Effects has none");
        }
    }
    assert!(compared >= 40, "{compared}");
}

/// The keyed (or expression) records below the shape contents of the first layer: `(path,
/// tdb4, lhd3, ldat with spatial arc lengths zeroed, expression)`.
fn keyed_shape_records(bytes: &[u8]) -> Vec<(String, Vec<u8>, Vec<u8>, Vec<u8>, Option<Vec<u8>>)> {
    fn walk(cs: &[effectcraft_aep::Chunk<'_>], path: &str, inside: bool, out: &mut Vec<(String, Vec<u8>, Vec<u8>, Vec<u8>, Option<Vec<u8>>)>) {
        for (i, c) in cs.iter().enumerate() {
            let Some(name) = c.bytes().filter(|_| c.is(b"tdmn")).map(|b| String::from_utf8_lossy(b.split(|x| *x == 0).next().unwrap_or(b"")).to_string())
            else {
                continue;
            };
            let Some(node) = cs.get(i + 1) else { continue };
            let key = format!("{path}/{name}");
            let here = inside || name == "ADBE Root Vectors Group";
            if here && node.is_list(b"tdbs") {
                let list = node.child_list(b"list");
                let expr = node.child(b"Utf8").and_then(|u| u.bytes()).map(<[u8]>::to_vec);
                if list.is_some() || expr.is_some() {
                    let tdb4 = node.child(b"tdb4").and_then(|c| c.bytes()).unwrap_or_default().to_vec();
                    let lhd3 = list.and_then(|l| l.child(b"lhd3")).and_then(|c| c.bytes()).unwrap_or_default().to_vec();
                    let mut ldat = list.and_then(|l| l.child(b"ldat")).and_then(|c| c.bytes()).unwrap_or_default().to_vec();
                    let size = lhd3.get(16..20).map_or(0, |b| u32::from_be_bytes(b.try_into().unwrap()) as usize);
                    if size == 104 {
                        for r in ldat.chunks_mut(104) {
                            r[16..24].fill(0);
                            // Automatic spatial tangents: After Effects keeps 0 for two keys
                            // made by script (853), the exporter writes the tangents it computes
                            // (collinear: the same straight path, R14).
                            if r[8..12] == [0, 0, 0, 1] {
                                r[72..104].fill(0);
                            }
                        }
                    }
                    // The ease before the first key and after the last one is never used (the
                    // importer keeps none, the exporter writes 1/3 influence there).
                    if size == 48 && ldat.len() >= 96 {
                        ldat[16..32].fill(0);
                        let n = ldat.len();
                        ldat[n - 16..n].fill(0);
                    }
                    out.push((key.clone(), tdb4, lhd3, ldat, expr));
                }
            }
            if node.is_list(b"tdgp") {
                walk(node.children(), &key, here, out);
            }
        }
    }
    let f = parse(bytes).expect("parse");
    let view = effectcraft_aep::Project::new(&f).expect("project");
    let mut out = Vec::new();
    for item in view.all_items() {
        let Some(comp) = item.comp() else { continue };
        for layer in comp.layers() {
            if let Some(g) = layer.chunk().child_list(b"tdgp") {
                walk(g.children(), "", false, &mut out);
            }
        }
    }
    out
}

#[test]
fn keyed_shape_properties_are_written_as_after_effects_wrote_them() {
    // 851-857: Trim End linear and bezier keys, group Position (spatial), Fill Color, Trim
    // Offset with a hold key, an expression on Trim End, group Scale (two components).
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../aep/tests/fixtures");
    for name in [
        "851-shape-key-trim-end-0-100-linear.aep",
        "852-shape-key-trim-end-bezier-out0-60-in20-30.aep",
        "853-shape-key-group-position-0-200-100-linear.aep",
        "854-shape-key-fill-color-red-blue-linear.aep",
        "855-shape-key-trim-offset-0-90-hold.aep",
        "856-shape-trim-end-expression-time50.aep",
        "857-shape-key-group-scale-100-50-150-linear.aep",
    ] {
        let bytes = std::fs::read(dir.join(name)).expect("fixture");
        let mut p = Project::default();
        let none = |_: &str| false;
        effectcraft_aep_import::import(
            &mut p,
            &bytes,
            &effectcraft_aep_import::ImportOptions { allow_losses: true, ..effectcraft_aep_import::ImportOptions::new(&none) },
        )
        .expect("import");
        let r = export_all(&p);
        assert_eq!(r.report.count(LossCategory::ShapeLayers), 0, "{name}: {:?}", r.report.losses);
        let (a, b) = (keyed_shape_records(&bytes), keyed_shape_records(&r.bytes));
        assert_eq!(a.len(), 1, "{name}");
        assert_eq!(a, b, "{name}");
    }
}

#[test]
fn shape_operators_and_keys_built_in_code_come_back() {
    // Polystar, gradient fill / stroke, trim paths with keys, round corners, merge / offset paths
    // and a repeater with a transform: exported, imported again, the same values and keys.
    use effectcraft_keyframe::{Gradient, Keyframe, Value};
    use effectcraft_project::build;
    let mut p = Project::default();
    let c = add_comp(&mut p, "C", 1920, 1080, FrameRate::new(25, 1), 2.0, None);
    add_layer(&mut p, c, "S", LayerSource::Shape, (1920, 1080));
    let mut next = p.next_id;
    {
        let mut ids = build::Ids(&mut next);
        let rect = build::shape_rect(&mut ids, [400.0, 300.0], [0.0, 0.0], 0.0);
        let mut star = build::shape_star(&mut ids, true, 7.0, [10.0, 20.0], 120.0, 40.0);
        star.get_mut("rotation").unwrap().value = Value::Scalar(15.0);
        let mut gfill = build::shape_gradient_fill(&mut ids, true, [-50.0, 0.0], [60.0, 10.0], Gradient::default());
        gfill.get_mut("opacity").unwrap().value = Value::Scalar(70.0);
        let gstroke = build::shape_gradient_stroke(&mut ids, false, [0.0, 0.0], [100.0, 50.0], Gradient::default(), 5.0);
        let mut trim = build::shape_trim(&mut ids, 10.0, 90.0, 30.0);
        trim.get_mut("end").unwrap().keys = vec![
            key(0.0, Value::Scalar(0.0)),
            Keyframe { out_interp: effectcraft_keyframe::Interp::Hold, ..key(1.0, Value::Scalar(60.0)) },
            key(1.5, Value::Scalar(100.0)),
        ];
        let mut round = build::shape_simple_op(&mut ids, "round").unwrap();
        round.get_mut("radius").unwrap().value = Value::Scalar(25.0);
        let mut merge = build::shape_simple_op(&mut ids, "merge").unwrap();
        merge.get_mut("mode").unwrap().value = Value::Enum(2);
        let mut offset = build::shape_simple_op(&mut ids, "offset").unwrap();
        offset.get_mut("amount").unwrap().value = Value::Scalar(-5.0);
        offset.get_mut("join").unwrap().value = Value::Enum(1);
        let mut rep = build::shape_repeater(&mut ids, 4.0, [80.0, 0.0]);
        if let Some(t) = rep.sub_mut("transform") {
            t.get_mut("rotation").unwrap().value = Value::Scalar(12.0);
            t.get_mut("endOpacity").unwrap().value = Value::Scalar(30.0);
            t.get_mut("position").unwrap().keys = vec![key(0.0, Value::Vec2([0.0, 0.0])), key(1.0, Value::Vec2([80.0, 40.0]))];
        }
        let mut fill = build::shape_fill(&mut ids, [0.0, 1.0, 0.0, 1.0]);
        fill.get_mut("color").unwrap().keys = vec![key(0.0, Value::Color([1.0, 0.0, 0.0, 1.0])), key(1.0, Value::Color([0.0, 0.0, 1.0, 1.0]))];
        let g = build::shape_group(&mut ids, "Group 1", vec![rect, star, gfill, gstroke, trim, round, merge, offset, rep, fill]);
        with_layer(&mut p, c, 0, |l| l.props.sub_mut("contents").expect("contents").children = vec![g.into()]);
    }
    p.next_id = next;
    let r = export_all(&p);
    assert_eq!(r.report.count(LossCategory::ShapeLayers), 0, "{:?}", r.report.losses);
    let file = parse(&r.bytes).expect("parse");
    assert_eq!(effectcraft_aep::write(&file).expect("write"), r.bytes);
    let mut q = Project::default();
    let none = |_: &str| false;
    let back = effectcraft_aep_import::import(
        &mut q,
        &r.bytes,
        &effectcraft_aep_import::ImportOptions { allow_losses: true, ..effectcraft_aep_import::ImportOptions::new(&none) },
    )
    .expect("import");
    assert!(back.report.losses.is_empty(), "{:?}", back.report.losses);
    // Every property of the contents: value and key count (ids differ).
    let flat = |p: &Project| -> Vec<(String, String, usize)> {
        let mut out = Vec::new();
        for item in p.items.values() {
            let effectcraft_project::ItemKind::Comp(c) = &item.kind else { continue };
            for l in &c.layers {
                let Some(contents) = l.props.sub("contents") else { continue };
                let mut stack = vec![(contents, String::new())];
                while let Some((g, pre)) = stack.pop() {
                    for n in &g.children {
                        match n {
                            effectcraft_project::Node::Group(s) => stack.push((s, format!("{pre}{}/", s.match_id))),
                            // A keyed property's static value is its first key's in the file.
                            effectcraft_project::Node::Prop(pr) => out.push((
                                format!("{pre}{}", pr.match_id),
                                if pr.keys.is_empty() {
                                    format!("{:?}", pr.value)
                                } else {
                                    format!("{:?}", pr.keys.iter().map(|k| (k.time, k.value.clone(), k.out_interp)).collect::<Vec<_>>())
                                },
                                pr.keys.len(),
                            )),
                        }
                    }
                }
            }
        }
        out.sort();
        out
    };
    assert_eq!(flat(&p), flat(&q));
}

#[test]
fn hostile_shape_keys_never_panic() {
    use effectcraft_keyframe::Value;
    use effectcraft_project::build;
    let mut p = Project::default();
    let c = add_comp(&mut p, "C", 640, 360, FrameRate::new(25, 1), 2.0, None);
    add_layer(&mut p, c, "S", LayerSource::Shape, (640, 360));
    let mut next = p.next_id;
    {
        let mut ids = build::Ids(&mut next);
        let mut trim = build::shape_trim(&mut ids, 0.0, 100.0, 0.0);
        trim.get_mut("end").unwrap().keys = (0..20_000).map(|i| key(f64::from(i) * 0.0001, Value::Scalar(f64::from(i)))).collect();
        trim.get_mut("start").unwrap().keys = vec![key(0.0, Value::Scalar(f64::NAN)), key(-5.0, Value::Bool(true)), key(1e12, Value::Scalar(1.0))];
        trim.get_mut("mode").unwrap().keys = vec![key(0.0, Value::Enum(1)), key(1.0, Value::Enum(900))];
        let mut fill = build::shape_fill(&mut ids, [0.0, 1.0, 0.0, 1.0]);
        fill.get_mut("color").unwrap().keys = vec![key(0.0, Value::Color([f64::INFINITY, 0.0, 0.0, 1.0])), key(0.0, Value::Scalar(3.0))];
        let g = build::shape_group(&mut ids, "G", vec![trim, fill]);
        with_layer(&mut p, c, 0, |l| l.props.sub_mut("contents").expect("contents").children = vec![g.into()]);
    }
    p.next_id = next;
    let r = export_all(&p);
    let file = parse(&r.bytes).expect("parse");
    assert_eq!(effectcraft_aep::write(&file).expect("write"), r.bytes);
    assert!(r.report.count(LossCategory::InvalidValues) >= 1, "{:?}", r.report.losses);
}
