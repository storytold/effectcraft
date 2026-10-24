//! The exported bytes are parsed back with `effectcraft-aep` and every exported field is compared
//! with the project it came from.

mod common;

use common::*;
use effectcraft_aep::{BlendMode as AepBlend, Interpolation, ItemKind as AepKind, PropNode, Quality as AepQuality, parse, write};
use effectcraft_aep_export::{AepExportError, ExportOptions, LossCategory, analyze, export};
use effectcraft_color::{BlendMode, Label};
use effectcraft_keyframe::{Ease, Interp, Value};
use effectcraft_project::{ItemKind, LayerSource, Project, Quality};
use effectcraft_time::FrameRate;

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-9
}

#[test]
fn simple_project_round_trips_through_the_container() {
    let (p, _, _) = simple();
    let r = export_all(&p);
    assert!(r.report.is_lossless(), "{:?}", r.report.losses);
    let file = parse(&r.bytes).expect("parse");
    // The container is consistent: writing it back gives the same bytes.
    assert_eq!(write(&file).expect("write"), r.bytes);
    let view = effectcraft_aep::Project::new(&file).expect("project");
    let items = view.all_items();
    let kinds: Vec<_> = items.iter().filter_map(|i| i.kind()).collect();
    assert_eq!(kinds.iter().filter(|k| **k == AepKind::Composition).count(), 1);
    assert_eq!(kinds.iter().filter(|k| **k == AepKind::Folder).count(), 1);
    assert_eq!(kinds.iter().filter(|k| **k == AepKind::Footage).count(), 1);
}

#[test]
fn composition_header_fields() {
    let mut p = Project::default();
    let c = add_comp(&mut p, "Flags", 800, 600, FrameRate::new(30, 1), 10.0, None);
    with_comp(&mut p, c, |cc| {
        cc.work_area = (secs(1.0), secs(3.0));
        cc.background = [0.5, 0.25, 1.0];
        cc.pixel_aspect = 0.9091;
        cc.shutter_angle = 90.0;
        cc.shutter_phase = -45.0;
        cc.display_start = secs(4.0);
        cc.enable_motion_blur = true;
    });
    let r = export_all(&p);
    let file = parse(&r.bytes).expect("parse");
    let view = effectcraft_aep::Project::new(&file).expect("project");
    let item = view.all_items().into_iter().find(|i| i.display_name().as_deref() == Some("Flags")).expect("comp");
    let h = item.comp().expect("comp").header().expect("header");
    assert_eq!((h.width, h.height), (800, 600));
    assert_eq!((h.time_base.frame_duration, h.time_base.timescale), (1024, 30720));
    assert_eq!(h.duration_units, 307_200);
    assert_eq!(h.work_area_start_units, 30_720);
    assert_eq!(h.work_area_end_units, Some(92_160));
    assert_eq!(h.background, [128, 64, 255]);
    assert_eq!((h.pixel_aspect.num, h.pixel_aspect.den), (10, 11));
    assert_eq!(h.shutter_angle, 90);
    assert_eq!(h.shutter_phase, Some(-45));
    assert!(h.motion_blur);
    assert_eq!(h.display_start.map(|d| (d.num, d.den)), Some((122_880, 30_720)));
    assert_eq!(h.frame_rate_fixed, 30 << 16);
}

#[test]
fn ntsc_and_odd_frame_rates_use_the_time_bases_after_effects_writes() {
    for (rate, fd, ts, fixed) in [
        (FrameRate::FPS_29_97, 800u32, 23_976u32, 1_964_114u32),
        (FrameRate::FPS_23_976, 1000, 23_976, 1_571_291),
        (FrameRate::new(24, 1), 1024, 24_576, 24 << 16),
        (FrameRate::new(12, 1), 2048, 24_576, 12 << 16),
        (FrameRate::new(60, 1), 512, 30_720, 60 << 16),
    ] {
        let mut p = Project::default();
        add_comp(&mut p, "R", 640, 360, rate, 2.0, None);
        let r = export_all(&p);
        let file = parse(&r.bytes).expect("parse");
        let view = effectcraft_aep::Project::new(&file).expect("project");
        let item = view.all_items().into_iter().find(|i| i.display_name().as_deref() == Some("R")).expect("comp");
        let h = item.comp().expect("c").header().expect("h");
        assert_eq!((h.time_base.frame_duration, h.time_base.timescale, h.frame_rate_fixed), (fd, ts, fixed), "{}", rate.label());
        let frames = (rate.as_f64() * 2.0).round() as u32;
        assert_eq!(h.duration_units, frames * fd);
    }
}

#[test]
fn layer_header_fields() {
    let mut p = Project::default();
    let c = add_comp(&mut p, "C", 800, 600, FrameRate::new(30, 1), 10.0, None);
    let s = add_solid(&mut p, "Sol", 800, 600, [0.1, 0.2, 0.3], None);
    for name in ["Bottom", "Mid", "Top"] {
        add_layer(&mut p, c, name, LayerSource::Solid { item: s }, (800, 600));
    }
    add_layer(&mut p, c, "Nul", LayerSource::Null, (100, 100));
    let bottom = p.comp(c).and_then(|cc| cc.layers.last().map(|l| l.id)).expect("bottom");
    with_layer(&mut p, c, 1, |l| {
        l.switches.adjustment = true;
        l.blend_mode = BlendMode::Multiply;
        l.label = Label::Lavender;
        l.switches.quality = Quality::Draft;
        l.parent = Some(bottom);
    });
    with_layer(&mut p, c, 2, |l| {
        l.switches.locked = true;
        l.switches.shy = true;
        l.switches.solo = true;
        l.blend_mode = BlendMode::Screen;
        l.start_time = secs(1.5);
        l.in_point = secs(2.0);
        l.out_point = secs(6.0);
        l.switches.video = false;
        l.switches.audio = false;
        l.switches.effects = false;
        l.switches.motion_blur = true;
        l.switches.guide = true;
    });
    let r = export_all(&p);
    assert!(r.report.is_lossless());
    let file = parse(&r.bytes).expect("parse");
    let view = effectcraft_aep::Project::new(&file).expect("project");
    let comp = view.all_items().into_iter().find(|i| i.comp().is_some()).expect("comp");
    let layers: Vec<_> = comp.comp().expect("c").layers().collect();
    assert_eq!(layers.len(), 4);
    let names: Vec<_> = layers.iter().map(|l| l.name().unwrap_or_default()).collect();
    assert_eq!(names, ["Nul", "Top", "Mid", "Bottom"]);
    let h: Vec<_> = layers.iter().map(|l| l.header().expect("header")).collect();
    // Null.
    assert!(h[0].is_null_object());
    assert!(h[0].source_id.is_some());
    // Adjustment layer "Top".
    assert!(h[1].is_adjustment());
    assert_eq!(h[1].blend_mode, AepBlend::Multiply);
    assert_eq!(h[1].label, 5);
    assert_eq!(h[1].quality, AepQuality::Draft);
    assert_eq!(h[1].parent_id, Some(h[3].id));
    // "Mid".
    assert!(h[2].is_locked() && h[2].is_shy() && h[2].is_solo());
    assert!(!h[2].is_video_enabled() && !h[2].is_audio_enabled() && !h[2].is_effects_enabled());
    assert!(h[2].is_motion_blur() && h[2].is_guide());
    assert_eq!(h[2].blend_mode, AepBlend::Screen);
    assert_eq!((h[2].start.num, h[2].start.den), (46_080, 30_720));
    assert_eq!(h[2].in_point_seconds(), Some(2.0));
    assert_eq!(h[2].out_point_seconds(), Some(6.0));
    assert_eq!(h[2].name_copy, "Mid");
    // Ids are unique.
    let mut ids: Vec<u32> = h.iter().map(|x| x.id).collect();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), 4);
}

#[test]
fn solids_and_items() {
    let mut p = Project::default();
    let f = p.add_item("Folder A", Label::None, None, ItemKind::Folder);
    add_comp(&mut p, "Comp 1", 100, 100, FrameRate::new(25, 1), 1.0, Some(f));
    add_solid(&mut p, "Blue", 640, 360, [0.0, 0.0, 1.0], Some(f));
    add_solid(&mut p, "Root solid", 31, 17, [1.0, 0.5, 0.25], None);
    let r = export_all(&p);
    let file = parse(&r.bytes).expect("parse");
    let view = effectcraft_aep::Project::new(&file).expect("project");
    let all = view.all_items();
    let folder = all.iter().find(|i| i.display_name().as_deref() == Some("Folder A")).expect("folder");
    let kids: Vec<_> = folder.children().collect();
    assert_eq!(kids.len(), 2);
    let blue = kids.iter().find(|i| i.kind() == Some(AepKind::Footage)).expect("blue");
    let f = blue.footage().expect("footage");
    assert!(f.is_solid);
    assert_eq!((f.width, f.height), (640, 360));
    assert_eq!(f.solid_rgb(), Some([0.0, 0.0, 1.0]));
    let solids = all.iter().find(|i| i.display_name().as_deref() == Some("Solids")).expect("Solids folder");
    let rs = solids.children().next().expect("root solid").footage().expect("f");
    assert_eq!((rs.width, rs.height), (31, 17));
    assert_eq!(rs.solid_rgb(), Some([1.0, 0.5, 0.25]));
}

fn transform_of(bytes: &[u8]) -> (effectcraft_aep::Transform, Vec<(String, Vec<(i32, Option<Interpolation>, Option<Interpolation>, Vec<f64>)>)>) {
    let file = parse(bytes).expect("parse");
    let view = effectcraft_aep::Project::new(&file).expect("project");
    let comp = view.all_items().into_iter().find(|i| i.comp().is_some()).expect("comp");
    let h = comp.comp().expect("c").header().expect("h");
    let layer = comp.comp().expect("c").layers().next().expect("layer");
    let tr = layer.transform(u32::from(h.width), u32::from(h.height)).expect("transform");
    let mut keys = Vec::new();
    if let Some(PropNode::Group(g)) = layer.properties().and_then(|p| p.get("ADBE Transform Group")) {
        for name in ["ADBE Anchor Point", "ADBE Position", "ADBE Scale", "ADBE Rotate Z", "ADBE Opacity"] {
            if let Some(PropNode::Property(pr)) = g.get(name)
                && let Some(list) = pr.keys()
            {
                let recs = list
                    .keyframes()
                    .map(|k| (k.time_units().unwrap_or(0), k.interpolation_in(), k.interpolation_out(), k.value().unwrap_or_default()))
                    .collect();
                keys.push((name.to_string(), recs));
            }
        }
    }
    (tr, keys)
}

#[test]
fn static_transform_values_use_the_file_units() {
    let mut p = Project::default();
    let c = add_comp(&mut p, "C", 1920, 1080, FrameRate::new(25, 1), 5.0, None);
    let s = add_solid(&mut p, "L", 400, 300, [1.0, 0.5, 0.0], None);
    add_layer(&mut p, c, "L", LayerSource::Solid { item: s }, (400, 300));
    with_layer(&mut p, c, 0, |l| {
        let tr = l.transform_mut().expect("tr");
        tr.get_mut("anchor").expect("a").value = Value::Vec3([100.0, 50.0, 0.0]);
        tr.get_mut("position").expect("p").value = Value::Vec3([300.0, 200.0, 0.0]);
        tr.get_mut("scale").expect("s").value = Value::Vec3([50.0, 60.0, 100.0]);
        tr.get_mut("rotation").expect("r").value = Value::Scalar(45.0);
        tr.get_mut("opacity").expect("o").value = Value::Scalar(25.0);
    });
    let r = export_all(&p);
    let (tr, keys) = transform_of(&r.bytes);
    assert!(keys.is_empty());
    let a = tr.anchor_px(400.0, 300.0);
    assert!(close(a[0], 100.0) && close(a[1], 50.0));
    assert_eq!(tr.anchor_fraction.value, [0.25, 50.0 / 300.0, 0.0]);
    assert_eq!(tr.position.value, [300.0, 200.0, 0.0]);
    assert_eq!(tr.scale_percent.value, [50.0, 60.0, 100.0]);
    assert!(close(tr.rotation_degrees.value, 45.0));
    assert!(close(tr.opacity_percent.value, 25.0));
}

#[test]
fn defaults_are_not_written() {
    let (p, _, _) = simple();
    let r = export_all(&p);
    let plain = export_all(&p);
    assert_eq!(r.bytes, plain.bytes);
    // A layer at the default values has no property record beyond the template's.
    let file = parse(&r.bytes).expect("parse");
    let view = effectcraft_aep::Project::new(&file).expect("project");
    let comp = view.all_items().into_iter().find(|i| i.comp().is_some()).expect("comp");
    let layer = comp.comp().expect("c").layers().next().expect("layer");
    let Some(PropNode::Group(g)) = layer.properties().and_then(|p| p.get("ADBE Transform Group")) else { panic!("no group") };
    for n in ["ADBE Anchor Point", "ADBE Position", "ADBE Scale", "ADBE Rotate Z", "ADBE Opacity"] {
        assert!(g.get(n).is_none(), "{n} must be absent at its default");
    }
}

#[test]
fn keyframes_with_interpolation_ease_and_expressions() {
    let mut p = Project::default();
    let c = add_comp(&mut p, "C", 1920, 1080, FrameRate::new(25, 1), 5.0, None);
    let s = add_solid(&mut p, "S", 400, 300, [0.2, 0.4, 0.6], None);
    add_layer(&mut p, c, "S", LayerSource::Solid { item: s }, (400, 300));
    with_layer(&mut p, c, 0, |l| {
        let tr = l.transform_mut().expect("tr");
        let o = tr.get_mut("opacity").expect("o");
        let mut k0 = key(0.0, Value::Scalar(0.0));
        let mut k1 = key(1.0, Value::Scalar(100.0));
        k0.out_interp = Interp::Bezier;
        k0.out_ease = vec![Ease { speed: 0.0, influence: 0.5 }];
        k1.in_interp = Interp::Bezier;
        k1.in_ease = vec![Ease { speed: 0.0, influence: 0.25 }];
        k1.out_interp = Interp::Hold;
        o.keys = vec![k0, k1, key(2.0, Value::Scalar(50.0))];
        let rot = tr.get_mut("rotation").expect("r");
        rot.value = Value::Scalar(10.0);
        rot.expr = Some(effectcraft_project::Expression { text: "time*20".into(), enabled: false });
    });
    let r = export_all(&p);
    assert_eq!(r.report.exported.keyframes, 3);
    assert_eq!(r.report.exported.expressions, 1);
    let (tr, keys) = transform_of(&r.bytes);
    let (_, opacity) = keys.iter().find(|(n, _)| n == "ADBE Opacity").expect("opacity keys");
    assert_eq!(opacity.len(), 3);
    assert_eq!(opacity[0].0, 0);
    assert_eq!(opacity[1].0, 25_600);
    assert_eq!(opacity[2].0, 51_200);
    assert_eq!(opacity[0].2, Some(Interpolation::Bezier));
    assert_eq!(opacity[1].1, Some(Interpolation::Bezier));
    assert_eq!(opacity[1].2, Some(Interpolation::Hold));
    assert_eq!(opacity[1].3, vec![1.0]);
    assert!(close(tr.rotation_degrees.value, 10.0));
    let file = parse(&r.bytes).expect("parse");
    let view = effectcraft_aep::Project::new(&file).expect("project");
    let comp = view.all_items().into_iter().find(|i| i.comp().is_some()).expect("comp");
    let layer = comp.comp().expect("c").layers().next().expect("layer");
    let Some(PropNode::Group(g)) = layer.properties().and_then(|p| p.get("ADBE Transform Group")) else { panic!("no group") };
    let Some(PropNode::Property(rot)) = g.get("ADBE Rotate Z") else { panic!("no rotation") };
    assert_eq!(rot.expression().as_deref(), Some("time*20"));
    assert_eq!(rot.expression_enabled(), Some(false));
}

#[test]
fn losses_are_reported_and_refused_unless_allowed() {
    let (mut p, c, _) = simple();
    let t = add_comp(&mut p, "T", 100, 100, FrameRate::new(25, 1), 1.0, None);
    add_layer(&mut p, t, "Par", LayerSource::Light { kind: effectcraft_project::LightKind::Parallel }, (100, 100));
    add_layer(&mut p, t, "Amb", LayerSource::Light { kind: effectcraft_project::LightKind::Ambient }, (100, 100));
    with_layer(&mut p, c, 0, |l| {
        l.blend_mode = BlendMode::Dissolve;
        l.switches.three_d = true;
        // A stretch that is not an exact small ratio is written unstretched.
        l.stretch = 100.0 * std::f64::consts::PI;
        l.transform_mut().expect("tr").get_mut("rotation").expect("r").keys = vec![key(0.0, Value::Scalar(0.0)), key(1.0, Value::Scalar(90.0))];
    });
    let rep = analyze(&p);
    assert_eq!(rep.count(LossCategory::Lights), 2);
    assert_eq!(rep.count(LossCategory::BlendModes), 1);
    // The 3D switch is written.
    assert_eq!(rep.count(LossCategory::ThreeD), 0);
    assert_eq!(rep.count(LossCategory::TimeStretch), 1);
    assert!(!rep.limitations.is_empty());
    match export(&p, &ExportOptions::default()) {
        Err(AepExportError::Refused(r)) => assert_eq!(*r, rep),
        other => panic!("expected a refusal, got {other:?}"),
    }
    let ok = export(&p, &ExportOptions { allow_losses: true }).expect("allowed");
    assert_eq!(ok.report, rep);
    // The report round-trips through JSON.
    let json = serde_json::to_string(&rep).expect("json");
    assert_eq!(serde_json::from_str::<effectcraft_aep_export::ExportReport>(&json).expect("back"), rep);
}

#[test]
fn the_exported_file_is_deterministic() {
    let (p, _, _) = simple();
    assert_eq!(export_all(&p).bytes, export_all(&p).bytes);
}

/// The user, host and domain names of the machine that runs the test, lower-cased. They are read from
/// the environment so that no name is written down in the repository; names shorter than three
/// characters are skipped because they would match by chance.
fn local_names() -> Vec<String> {
    ["USERNAME", "USER", "COMPUTERNAME", "HOSTNAME", "USERDOMAIN"]
        .into_iter()
        .filter_map(|key| std::env::var(key).ok())
        .map(|v| v.trim().to_lowercase())
        .filter(|v| v.len() >= 3)
        .collect()
}

/// Absolute drive-letter paths (`X:\…`, `X:/…`) in a lower-cased text, except the neutral
/// `c:\fixtures\` paths the fixtures use.
fn drive_paths(hay: &str) -> Vec<String> {
    let b = hay.as_bytes();
    let mut found = Vec::new();
    for i in 0..b.len().saturating_sub(3) {
        let starts = i == 0 || !b[i - 1].is_ascii_alphanumeric();
        if starts
            && b[i].is_ascii_alphabetic()
            && b[i + 1] == b':'
            && matches!(b[i + 2], b'\\' | b'/')
            && b[i + 3].is_ascii_alphanumeric()
            && !b[i..].starts_with(b"c:\\fixtures\\")
            && !b[i..].starts_with(b"c:/fixtures/")
        {
            found.push(String::from_utf8_lossy(&b[i..b.len().min(i + 16)]).into_owned());
        }
    }
    found
}

/// E-mail-like strings (`local@domain.tld`) in a lower-cased text.
fn email_like(hay: &str) -> Vec<String> {
    let b = hay.as_bytes();
    let local = |c: u8| c.is_ascii_alphanumeric() || b"._%+-".contains(&c);
    let domain = |c: u8| c.is_ascii_alphanumeric() || c == b'-' || c == b'.';
    let mut found = Vec::new();
    for at in (0..b.len()).filter(|&i| b[i] == b'@') {
        let mut s = at;
        while s > 0 && local(b[s - 1]) {
            s -= 1;
        }
        let mut e = at + 1;
        while e < b.len() && domain(b[e]) {
            e += 1;
        }
        while e > at + 1 && b[e - 1] == b'.' {
            e -= 1;
        }
        let host = &b[at + 1..e];
        let tld = host.iter().rposition(|&c| c == b'.').map(|dot| &host[dot + 1..]);
        let has_dot_inside = host.iter().position(|&c| c == b'.').is_some_and(|dot| dot > 0);
        if s < at && has_dot_inside && tld.is_some_and(|t| t.len() >= 2 && t.iter().all(u8::is_ascii_alphabetic)) {
            found.push(String::from_utf8_lossy(&b[s..e]).into_owned());
        }
    }
    found
}

#[test]
fn the_template_holds_no_local_data() {
    let base = include_bytes!("../template/base.aep");
    let protos = include_bytes!("../template/protos.aep");
    let masks = include_bytes!("../template/masks.aep");
    let effects = include_bytes!("../template/effects.aep");
    let footage = include_bytes!("../template/footage.aep");
    let text = include_bytes!("../template/text.aep");
    let objects = include_bytes!("../template/objects.aep");
    let names = local_names();
    let fixed = ["desktop-", "\\users\\", "/users/", "/home/"];
    for (name, bytes) in [
        ("base", &base[..]),
        ("protos", &protos[..]),
        ("masks", &masks[..]),
        ("effects", &effects[..]),
        ("footage", &footage[..]),
        ("text", &text[..]),
        ("objects", &objects[..]),
    ] {
        // Strings are searched as bytes and, with the NUL bytes removed, as UTF-16 text.
        let utf16: Vec<u8> = bytes.iter().copied().filter(|&c| c != 0).collect();
        for hay in [String::from_utf8_lossy(bytes).to_lowercase(), String::from_utf8_lossy(&utf16).to_lowercase()] {
            for needle in names.iter().map(String::as_str).chain(fixed) {
                assert!(!hay.contains(needle), "{name} contains a local name or path");
            }
            assert_eq!(drive_paths(&hay), Vec::<String>::new(), "{name} holds an absolute local path");
            assert_eq!(email_like(&hay), Vec::<String>::new(), "{name} holds an e-mail address");
        }
    }
}

#[test]
fn the_local_data_search_finds_what_it_looks_for() {
    assert_eq!(drive_paths("x d:\\work\\a.aep y"), vec!["d:\\work\\a.aep y".to_string()]);
    assert_eq!(drive_paths("q:/media/a.mov"), vec!["q:/media/a.mov".to_string()]);
    assert!(drive_paths("c:\\fixtures\\a.png and c:/fixtures/b.png").is_empty());
    assert!(drive_paths("http://example and 12:30 and ab:/x").is_empty());
    assert_eq!(email_like("by jane.doe@example.org."), vec!["jane.doe@example.org".to_string()]);
    assert!(email_like("@@ a@b x@.org y@host").is_empty());
}

#[test]
fn a_root_folder_named_solids_is_the_solids_folder() {
    let mut p = Project::default();
    let f = p.add_item("Solids", Label::None, None, ItemKind::Folder);
    let c = add_comp(&mut p, "C", 100, 100, FrameRate::new(25, 1), 1.0, None);
    let s = add_solid(&mut p, "Orange", 50, 50, [1.0, 0.5, 0.0], Some(f));
    add_layer(&mut p, c, "Orange", LayerSource::Solid { item: s }, (50, 50));
    add_layer(&mut p, c, "N", LayerSource::Null, (100, 100));
    let r = export_all(&p);
    let file = parse(&r.bytes).expect("parse");
    let view = effectcraft_aep::Project::new(&file).expect("project");
    let folders: Vec<_> = view.all_items().into_iter().filter(|i| i.kind() == Some(AepKind::Folder)).collect();
    assert_eq!(folders.len(), 1, "one Solids folder, not two");
    let kids: Vec<_> = folders[0].children().filter_map(|k| k.footage()).collect();
    assert_eq!(kids.len(), 2, "the solid and the hidden null solid");
    assert!(kids.iter().all(|k| k.is_solid));
}

#[test]
fn the_3d_switch_is_written() {
    // 015: ldta@38 bit 0x04.
    let (mut p, c, _) = simple();
    with_layer(&mut p, c, 0, |l| {
        l.switches.three_d = true;
        l.transform_mut().expect("tr").get_mut("position").expect("p").value = Value::Vec3([960.0, 540.0, 300.0]);
    });
    let r = export_all(&p);
    assert!(r.report.is_lossless(), "{:?}", r.report.losses);
    let file = parse(&r.bytes).expect("parse");
    let view = effectcraft_aep::Project::new(&file).expect("project");
    let layer = view.all_items().into_iter().find_map(|i| i.comp()).and_then(|c| c.layers().next()).expect("layer");
    assert!(layer.header().expect("header").is_three_d());
    let Some(PropNode::Group(g)) = layer.properties().and_then(|p| p.get("ADBE Transform Group")) else { panic!("transform") };
    let Some(PropNode::Property(pos)) = g.get("ADBE Position") else { panic!("position") };
    assert_eq!(pos.static_value().expect("value")[..3], [960.0, 540.0, 300.0]);
}

#[test]
fn time_stretch_is_the_exact_ratio_and_scales_the_time_scale() {
    // 018 (200 %: 2/1, 51 200), 316 (50 %: 1/2, 25 600), 317 (150 %: 3/2, 38 400).
    for (pct, num, den, ts) in [(200.0, 2i32, 1u32, 51_200u32), (50.0, 1, 2, 25_600), (150.0, 3, 2, 38_400)] {
        let (mut p, c, _) = simple();
        with_layer(&mut p, c, 0, |l| {
            l.stretch = pct;
            l.start_time = secs(1.0);
            l.in_point = secs(1.0);
            l.out_point = secs(1.0 + 2.0 * pct / 100.0);
            l.transform_mut().expect("tr").get_mut("opacity").expect("o").value = Value::Scalar(50.0);
        });
        let r = export_all(&p);
        assert!(r.report.is_lossless(), "{pct}: {:?}", r.report.losses);
        let file = parse(&r.bytes).expect("parse");
        let view = effectcraft_aep::Project::new(&file).expect("project");
        let layer = view.all_items().into_iter().find_map(|i| i.comp()).and_then(|c| c.layers().next()).expect("layer");
        let h = layer.header().expect("header");
        assert_eq!((h.stretch, h.stretch_den), (num, den), "{pct}");
        // Relative times in source time: the 2 s of source end 2 s after the start.
        assert_eq!((h.in_relative.num, h.out_relative.num), (0, 51_200), "{pct}");
        let Some(PropNode::Group(g)) = layer.properties().and_then(|p| p.get("ADBE Transform Group")) else { panic!("transform") };
        let Some(PropNode::Property(o)) = g.get("ADBE Opacity") else { panic!("opacity") };
        assert_eq!(o.time_scale(), Some(ts), "{pct}");
        // And back.
        let mut q = Project::default();
        let none = |_: &str| false;
        effectcraft_aep_import::import(
            &mut q,
            &r.bytes,
            &effectcraft_aep_import::ImportOptions { allow_losses: true, ..effectcraft_aep_import::ImportOptions::new(&none) },
        )
        .expect("import");
        let l = q.items.values().find_map(|i| if let ItemKind::Comp(c) = &i.kind { c.layers.first().cloned() } else { None }).expect("layer");
        assert!((l.stretch - pct).abs() < 1e-9 && l.out_point == secs(1.0 + 2.0 * pct / 100.0), "{pct}: {} {:?}", l.stretch, l.out_point);
    }
    // A ratio that is not small and exact is reported.
    let (mut p, c, _) = simple();
    with_layer(&mut p, c, 0, |l| l.stretch = 31.4159265);
    assert_eq!(export_all(&p).report.count(LossCategory::TimeStretch), 1);
}
