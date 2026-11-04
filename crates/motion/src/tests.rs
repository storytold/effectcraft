//! Import of synthetic Motion documents, keypoint/keyframe conversion and hostile input.

use effectcraft_keyframe::{Interp, Value};
use effectcraft_project::{Comp, Footage, FootageKind, ItemId, ItemKind, Layer, LayerSource, Project, Property};
use effectcraft_time::{FrameRate, TICKS_PER_SECOND, Tick};

use crate::curve::{Channel, MKey, Mapped, merge};
use crate::{ClipRef, clip_refs, import, parse_time, scene_rate};

fn none(_: &ClipRef) -> Option<Footage> {
    None
}

fn s(x: f64) -> Tick {
    Tick::from_seconds_f64(x)
}

/// A Motion 5 time at the 120000 timescale.
fn t(v: i64) -> String {
    format!("{v} 120000 1 0")
}

fn close(a: f64, b: f64, eps: f64) -> bool {
    (a - b).abs() <= eps
}

fn vec(v: &Value) -> Vec<f64> {
    v.components()
}

/// A document with `body` inside the scene after the settings (29.97 fps, 300 frames, 1920×1080).
fn doc(body: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE ozxmlscene>
<ozml version="5.2">
<factory id="3" uuid="615c4bf6406511d88802000a95af90f2"><description>Generator</description><manufacturer>Apple</manufacturer><version>1</version></factory>
<factory id="5" uuid="66fc0d6af6a911d6a7a7000393670732"><description>Image</description><manufacturer>Apple</manufacturer><version>1</version></factory>
<factory id="8" uuid="00000000000000000000000000000000"><description>Rig</description><manufacturer>Apple</manufacturer><version>1</version></factory>
<factory id="10" uuid="deca4859b16011d7a12d0003936f6f92"><description>ProPlugin Filter</description><manufacturer>Apple</manufacturer><version>1</version></factory>
<scene>
<sceneSettings>
  <width>1920</width><height>1080</height><duration>300</duration><frameRate>30</frameRate><NTSC>1</NTSC>
  <backgroundColor red="0.25" green="0.5" blue="0" alpha="1"/>
</sceneSettings>
{body}
<footage name="Media Layer" id="900">
  <clip name="Marina" id="901">
    <relativeURL>Media/Marina%20One.jpg</relativeURL>
    <pathURL>file://localhost/Users/someone/Movies/Media/Marina%20One.jpg</pathURL>
    <missingWidth>1280</missingWidth><missingHeight>720</missingHeight>
    <missingDuration>0.0333</missingDuration><creationDuration>1</creationDuration>
  </clip>
</footage>
</scene>
</ozml>"#
    )
}

fn solid_node(name: &str, timing: &str, props: &str) -> String {
    format!(
        r#"<scenenode name="{name}" id="11" factoryID="3" pluginUUID="C18E8B62-C24F-4F8D-8597-858675A40BBC" pluginName="Color Solid" version="5">
  {timing}
  <parameter name="Properties" id="1">{props}</parameter>
  <parameter name="Object" id="2"><parameter name="Color Solid" id="1"><parameter name="Color" id="1">
    <parameter name="Red" id="1" value="1"/><parameter name="Green" id="2" value="0.5"/><parameter name="Blue" id="3" value="0"/>
  </parameter></parameter></parameter>
</scenenode>"#
    )
}

fn image_node(name: &str, timing: &str, props: &str) -> String {
    format!(
        r#"<scenenode name="{name}" id="12" factoryID="5" version="5">
  {timing}
  <parameter name="Properties" id="1">
    {props}
    <parameter name="Media" id="324"><parameter name="Source Media" id="300" value="901"/></parameter>
  </parameter>
</scenenode>"#
    )
}

fn main_comp(p: &Project, id: ItemId) -> Comp {
    p.comp(id).cloned().unwrap()
}

fn tr(l: &Layer) -> &effectcraft_project::PropGroup {
    l.props.sub("transform").unwrap()
}

#[test]
fn times_parse_in_both_formats() {
    let r = FrameRate::FPS_29_97;
    assert_eq!(parse_time("120000 120000 1 0", r), Some(Tick(TICKS_PER_SECOND)));
    assert_eq!(parse_time("4004 120000 1 0", r), Some(r.frame_duration()));
    assert_eq!(parse_time("-4004 120000 1 0", r), Some(Tick(-r.frame_duration().0)));
    assert_eq!(parse_time("30", FrameRate::FPS_30), Some(Tick(TICKS_PER_SECOND)));
    assert_eq!(parse_time("1 0 1 0", r), None);
    assert_eq!(parse_time("x", r), None);
    assert_eq!(parse_time("99999999999999999999999999 1 1 0", r), None);
    assert_eq!(scene_rate(30.0, true), FrameRate::FPS_29_97);
    assert_eq!(scene_rate(24.0, true), FrameRate::FPS_23_976);
    assert_eq!(scene_rate(25.0, false), FrameRate::FPS_25);
}

#[test]
fn scene_settings_and_single_group_flatten() {
    let body = format!(
        r#"<layer name="Empty" id="2"><timing in="{}" out="{}" offset="0 1 1 0"/></layer>
<layer name="Group" id="3">{}{}<timing in="0 1 1 0" out="{}" offset="0 1 1 0"/></layer>"#,
        t(0),
        t(4004),
        solid_node("Top", "", ""),
        image_node("Photo", &format!(r#"<timing in="{}" out="{}" offset="{}"/>"#, t(40040), t(80080), t(40040)), ""),
        t(1197196),
    );
    let mut p = Project::default();
    let r = import(&mut p, doc(&body).as_bytes(), "Test", &none).unwrap();
    let c = main_comp(&p, r.comp);
    assert_eq!((c.width, c.height, c.frame_rate), (1920, 1080, FrameRate::FPS_29_97));
    assert_eq!(c.duration, FrameRate::FPS_29_97.tick_of(300));
    assert_eq!(c.background, [0.25, 0.5, 0.0]);
    // The empty group is dropped, the only group's contents are the comp's layers (top first).
    let names: Vec<&str> = c.layers.iter().map(|l| l.name.as_str()).collect();
    assert_eq!(names, ["Top", "Photo"]);
    let LayerSource::Solid { item } = c.layers[0].source else { panic!("solid expected") };
    let Some(ItemKind::Solid(sol)) = p.item(item).map(|i| &i.kind) else { panic!() };
    assert_eq!(sol.color, [1.0, 0.5, 0.0]);
    // Timing: out is Motion's last frame, start is the offset.
    let photo = &c.layers[1];
    let f = FrameRate::FPS_29_97.frame_duration();
    assert_eq!(photo.in_point, Tick(f.0 * 10));
    assert_eq!(photo.out_point, Tick(f.0 * 21));
    assert_eq!(photo.start_time, Tick(f.0 * 10));
    // Missing media becomes missing footage at the clip's size, with a warning.
    let LayerSource::Footage { item } = photo.source else { panic!("footage expected") };
    let Some(ItemKind::Footage(ft)) = p.item(item).map(|i| &i.kind) else { panic!() };
    assert!(ft.missing);
    assert_eq!((ft.width, ft.height, ft.kind), (1280, 720, FootageKind::Still));
    assert_eq!(ft.path, "/Users/someone/Movies/Media/Marina One.jpg");
    assert!(r.warnings.iter().any(|w| w.contains("Marina")), "{:?}", r.warnings);
    // Default transform: centred, 100 %, anchor at the layer centre.
    assert_eq!(vec(&tr(photo).get("position").unwrap().value), [960.0, 540.0, 0.0]);
    assert_eq!(vec(&tr(photo).get("anchor").unwrap().value), [640.0, 360.0, 0.0]);
    assert_eq!(vec(&tr(photo).get("scale").unwrap().value), [100.0, 100.0, 100.0]);
}

#[test]
fn resolved_media_is_used() {
    let body = format!(r#"<layer name="G" id="3">{}</layer>"#, image_node("Photo", "", ""));
    let resolve = |c: &ClipRef| -> Option<Footage> {
        assert_eq!(c.relative.as_deref(), Some("Media/Marina One.jpg"));
        Some(Footage { path: "/found/Marina One.jpg".into(), kind: FootageKind::Still, width: 640, height: 480, has_video: true, ..Default::default() })
    };
    let mut p = Project::default();
    let r = import(&mut p, doc(&body).as_bytes(), "Test", &resolve).unwrap();
    let c = main_comp(&p, r.comp);
    let LayerSource::Footage { item } = c.layers[0].source else { panic!() };
    let Some(ItemKind::Footage(ft)) = p.item(item).map(|i| &i.kind) else { panic!() };
    assert_eq!((ft.path.as_str(), ft.missing), ("/found/Marina One.jpg", false));
    assert_eq!(vec(&tr(&c.layers[0]).get("anchor").unwrap().value), [320.0, 240.0, 0.0]);
    assert_eq!(clip_refs(doc("").as_bytes()).unwrap().len(), 1);
}

#[test]
fn transform_maps_axes_units_and_keyframes() {
    let props = format!(
        r#"<parameter name="Transform" id="100">
  <parameter name="Position" id="101">
    <parameter name="X" id="1"><curve type="1" value="0"><numberOfKeypoints>2</numberOfKeypoints>
      <keypoint interpolation="1" flags="32"><time>{}</time><value>-100</value></keypoint>
      <keypoint interpolation="1" flags="32"><time>{}</time><value>100</value></keypoint>
    </curve></parameter>
    <parameter name="Y" id="2" value="50"/>
  </parameter>
  <parameter name="Rotation" id="109"><parameter name="Z" id="3" value="{}"/></parameter>
  <parameter name="Scale" id="105"><parameter name="X" id="1" value="0.5"/><parameter name="Y" id="2" value="2"/></parameter>
  <parameter name="Anchor Point" id="106"><parameter name="X" id="1" value="10"/><parameter name="Y" id="2" value="20"/></parameter>
</parameter>
<parameter name="Blending" id="200"><parameter name="Opacity" id="202"><curve type="1" value="1"><numberOfKeypoints>2</numberOfKeypoints>
  <keypoint interpolation="0" flags="32"><time>{}</time><value>1</value></keypoint>
  <keypoint flags="32"><time>{}</time><value>0.25</value></keypoint>
</curve></parameter></parameter>"#,
        t(0),
        t(120000),
        std::f64::consts::FRAC_PI_2,
        t(0),
        t(60000),
    );
    let body = format!(r#"<layer name="G" id="3">{}</layer>"#, solid_node("S", "", &props));
    let mut p = Project::default();
    let r = import(&mut p, doc(&body).as_bytes(), "Test", &none).unwrap();
    let c = main_comp(&p, r.comp);
    let g = tr(&c.layers[0]);
    // Animated position is separated: X keyframes, Y up becomes Y down.
    let px = g.get("positionX").unwrap();
    assert_eq!(px.keys.len(), 2);
    assert_eq!(px.keys[0].value, Value::Scalar(860.0));
    assert_eq!(px.keys[1].value, Value::Scalar(1060.0));
    assert_eq!(px.keys[1].time, s(1.0));
    assert_eq!(px.keys[0].out_interp, Interp::Linear);
    assert_eq!(g.get("positionY").unwrap().value, Value::Scalar(490.0));
    // 90° counter-clockwise is −90° in EffectCraft.
    assert!(close(g.get("rotation").unwrap().value.as_f64(), -90.0, 1e-9));
    assert_eq!(vec(&g.get("scale").unwrap().value), [50.0, 200.0, 100.0]);
    assert_eq!(vec(&g.get("anchor").unwrap().value), [970.0, 520.0, 0.0]);
    let op = g.get("opacity").unwrap();
    assert_eq!(op.keys[0].out_interp, Interp::Hold);
    assert_eq!(op.value_at(s(0.4)), Value::Scalar(100.0));
    assert_eq!(op.value_at(s(0.5)), Value::Scalar(25.0));
}

/// Bezier keypoints evaluate the same as the EffectCraft keyframes made from them.
#[test]
fn bezier_handles_become_matching_eases() {
    let ch = Channel {
        value: 0.0,
        keys: vec![
            MKey { t: 0.0, v: 0.0, code: None, tin: None, tout: Some((0.4, 0.0)) },
            MKey { t: 1.2, v: 10.0, code: None, tin: Some((-0.1, -3.0)), tout: Some((0.3, 0.0)) },
            MKey { t: 2.0, v: 4.0, code: Some(1), tin: Some((-0.2, 1.0)), tout: Some((0.1, 0.0)) },
            MKey { t: 3.0, v: 8.0, code: None, tin: Some((-0.3, 0.0)), tout: None },
        ],
    };
    for (scale, offset) in [(1.0, 0.0), (-100.0, 50.0)] {
        let (_, keys) = merge(&[Mapped { ch: &ch, scale, offset }], &|v| Value::Scalar(v[0]), false, &mut |_| {});
        let mut p = Property::new(1, "x", "X", Value::Scalar(0.0));
        p.keys = keys;
        for i in 0..=60 {
            let t = i as f64 * 0.05;
            let want = scale * ch.eval(t) + offset;
            let got = p.value_at(s(t)).as_f64();
            assert!(close(got, want, 1e-3 * scale.abs().max(1.0)), "t={t}: {got} vs {want}");
        }
    }
}

#[test]
fn channels_with_different_key_times_merge() {
    let x = Channel {
        value: 0.0,
        keys: vec![MKey { t: 0.0, v: 1.0, code: Some(1), tin: None, tout: None }, MKey { t: 1.0, v: 2.0, code: Some(1), tin: None, tout: None }],
    };
    let y = Channel { value: 0.0, keys: vec![MKey { t: 0.5, v: 3.0, code: Some(1), tin: None, tout: None }] };
    let z = Channel::constant(1.0);
    let mut warns = vec![];
    let chs = [Mapped { ch: &x, scale: 100.0, offset: 0.0 }, Mapped { ch: &y, scale: 100.0, offset: 0.0 }, Mapped { ch: &z, scale: 100.0, offset: 0.0 }];
    let (_, keys) = merge(&chs, &|v| Value::Vec3([v[0], v[1], v[2]]), false, &mut |w| warns.push(w));
    assert_eq!(keys.len(), 3);
    assert_eq!(vec(&keys[1].value), [150.0, 300.0, 100.0]);
    assert!(!warns.is_empty());
}

#[test]
fn group_filters_become_adjustment_layers() {
    let filters = format!(
        r#"<filter name="Soft" id="20" factoryID="10" pluginName="Gaussian Blur"><timing in="{}" out="{}" offset="0 1 1 0"/><parameter name="Amount" id="1" value="12"/></filter>
<filter name="Grain" id="21" factoryID="10" pluginName="Add Noise"><enabled>0</enabled><parameter name="Amount" id="1" value="2"/><parameter name="Monochrome" id="3" value="1"/></filter>
<filter name="Mystery" id="22" factoryID="10" pluginName="Kaleidoscope"/>
<behavior name="Wriggle" id="23" factoryID="7"/>"#,
        t(40040),
        t(80080)
    );
    let body = format!(r#"<layer name="G" id="3">{}{filters}</layer>"#, solid_node("S", "", ""));
    let mut p = Project::default();
    let r = import(&mut p, doc(&body).as_bytes(), "Test", &none).unwrap();
    let c = main_comp(&p, r.comp);
    // The behavior keeps the group from flattening: a precomp layer holds the contents.
    assert_eq!(c.layers.len(), 1);
    let LayerSource::Comp { item } = c.layers[0].source else { panic!("precomp expected") };
    let inner = main_comp(&p, item);
    let names: Vec<&str> = inner.layers.iter().map(|l| l.name.as_str()).collect();
    // Filters apply top to bottom, so the first filter is the lowest adjustment layer.
    assert_eq!(names, ["Grain", "Soft", "S"]);
    let soft = &inner.layers[1];
    assert!(soft.switches.adjustment);
    let f = FrameRate::FPS_29_97.frame_duration();
    assert_eq!((soft.in_point, soft.out_point), (Tick(f.0 * 10), Tick(f.0 * 21)));
    let fx = soft.props.sub("effects").unwrap();
    let blur = fx.children.iter().find_map(|n| if let effectcraft_project::Node::Group(g) = n { Some(g) } else { None }).unwrap();
    assert_eq!(blur.get("blurriness").unwrap().value, Value::Scalar(12.0));
    let grain = inner.layers[0].props.sub("effects").unwrap();
    let noise = grain.children.iter().find_map(|n| if let effectcraft_project::Node::Group(g) = n { Some(g) } else { None }).unwrap();
    assert!(!noise.enabled);
    assert_eq!(noise.get("amount").unwrap().value, Value::Scalar(50.0));
    assert_eq!(noise.get("color").unwrap().value, Value::Bool(false));
    assert!(r.warnings.iter().any(|w| w.contains("Kaleidoscope")), "{:?}", r.warnings);
    assert!(r.warnings.iter().any(|w| w.contains("Wriggle")), "{:?}", r.warnings);
}

#[test]
fn rigs_text_and_unknown_objects_warn() {
    let body = r#"<scenenode name="Rig" id="5" factoryID="8"/>
<layer name="G" id="3">
  <scenenode name="Title" id="6" factoryID="99"/>
</layer>"#;
    let mut p = Project::default();
    let r = import(&mut p, doc(body).as_bytes(), "Test", &none).unwrap();
    assert!(r.warnings.iter().any(|w| w.contains("rigs")), "{:?}", r.warnings);
    assert!(r.warnings.iter().any(|w| w.contains("unknown kind")), "{:?}", r.warnings);
}

#[test]
fn hostile_documents_are_errors_not_panics() {
    let mut p = Project::default();
    for bad in [&b"not xml"[..], b"<svg/>", b"<ozml/>", b"<ozml><scene/></ozml>", b"\xff\xfe<ozml/>", b""] {
        assert!(import(&mut p, bad, "X", &none).is_err());
    }
    // Absurd numbers and missing values fall back.
    let body = r#"<layer name="G" id="3"><scenenode name="S" id="11" factoryID="3" pluginName="Color Solid">
      <timing in="9223372036854775807 1 1 0" out="-1 0 1 0" offset="abc"/>
      <parameter name="Properties" id="1"><parameter name="Transform" id="100"><parameter name="Scale" id="105">
        <parameter name="X" id="1" value="NaN"/><parameter name="Y" id="2"><curve type="1"><keypoint><time>1e308</time><value>inf</value></keypoint></curve></parameter>
      </parameter></parameter></parameter></scenenode></layer>"#;
    let text = doc(body).replace("<width>1920</width>", "<width>99999999999</width>");
    let r = import(&mut p, text.as_bytes(), "X", &none).unwrap();
    assert!(main_comp(&p, r.comp).width <= 30_000);
    // Deep nesting stops at the depth limit.
    let deep = format!("{}{}{}", r#"<layer name="G" id="3">"#.repeat(200), solid_node("S", "", ""), "</layer>".repeat(200));
    let r = import(&mut p, doc(&deep).as_bytes(), "Deep", &none).unwrap();
    assert!(r.warnings.iter().any(|w| w.contains("too deeply")), "{:?}", r.warnings);
}

/// Templates rename filter instances ("Static", "Roll"): the plug-in UUID identifies them, and
/// the filter's Mix becomes the adjustment layer's opacity.
#[test]
fn renamed_filters_are_identified_by_uuid_and_mix_is_opacity() {
    let body = format!(
        r#"<layer name="G" id="3">{}
<filter name="Static" id="20" factoryID="10" pluginUUID="32ab5ee1-bacb-4b81-b44e-6d1e643c8d00" pluginName="Static">
  <parameter name="Static" id="3" value="4"/>
  <parameter name="Mix" id="10001"><curve type="1" value="1"><numberOfKeypoints>2</numberOfKeypoints>
    <keypoint interpolation="1" flags="32"><time>{}</time><value>0</value></keypoint>
    <keypoint interpolation="1" flags="32"><time>{}</time><value>1</value></keypoint>
  </curve></parameter>
</filter>
<behavior name="Wriggle" id="23" factoryID="7"/></layer>"#,
        solid_node("S", "", ""),
        t(0),
        t(120000)
    );
    let mut p = Project::default();
    let r = import(&mut p, doc(&body).as_bytes(), "Test", &none).unwrap();
    let c = main_comp(&p, r.comp);
    let LayerSource::Comp { item } = c.layers[0].source else { panic!() };
    let inner = main_comp(&p, item);
    let adj = &inner.layers[0];
    assert_eq!(adj.name, "Static");
    let fx: Vec<&str> = adj
        .props
        .sub("effects")
        .unwrap()
        .children
        .iter()
        .filter_map(|n| if let effectcraft_project::Node::Group(g) = n { Some(g.match_id.as_str()) } else { None })
        .collect();
    assert_eq!(fx, ["ec.distort.wavewarp", "ec.noise.noise", "ec.color.huesaturation"]);
    let op = tr(adj).get("opacity").unwrap();
    assert_eq!(op.value_at(s(0.0)), Value::Scalar(0.0));
    assert_eq!(op.value_at(s(0.5)), Value::Scalar(50.0));
    assert!(!r.warnings.iter().any(|w| w.contains("has no EffectCraft equivalent")), "{:?}", r.warnings);
}

#[test]
fn drop_zone_clips_with_bare_file_names_resolve_next_to_the_document() {
    let text = doc("").replace(
        "<relativeURL>Media/Marina%20One.jpg</relativeURL>\n    <pathURL>file://localhost/Users/someone/Movies/Media/Marina%20One.jpg</pathURL>",
        "<pathURL>Drop Zone Transition A.tiff</pathURL>",
    );
    let clips = clip_refs(text.as_bytes()).unwrap();
    assert_eq!(clips[0].path, None);
    assert_eq!(clips[0].relative.as_deref(), Some("Drop Zone Transition A.tiff"));
}

/// Regression: an absurd or tiny frame rate overflowed the NTSC numerator (a panic in debug
/// builds) or rounded to a zero rate.
#[test]
fn extreme_frame_rates_fall_back() {
    for (fps, ntsc) in [(1e300, true), (1e300, false), (1e-300, false), (0.0001, true), (f64::MAX, true)] {
        assert_eq!(scene_rate(fps, ntsc), FrameRate::FPS_30, "{fps} ntsc={ntsc}");
    }
    let text = doc("").replace("<frameRate>30</frameRate>", "<frameRate>1e300</frameRate>");
    let mut p = Project::default();
    let r = import(&mut p, text.as_bytes(), "X", &none).unwrap();
    assert_eq!(main_comp(&p, r.comp).frame_rate, FrameRate::FPS_30);
}

/// Regression: matching key times scanned every key, making a long curve quadratic to import
/// (about 11 s for this document before the fix; well under a second after).
#[test]
fn long_curves_import_in_linear_time() {
    let n = 100_000;
    let mut keys = String::with_capacity(n * 90);
    for i in 0..n {
        keys.push_str(&format!(r#"<keypoint interpolation="1"><time>{}</time><value>{}</value></keypoint>"#, t(i as i64 * 4004), i % 100));
    }
    let ch = |name: &str, id: u32| format!(r#"<parameter name="{name}" id="{id}"><curve type="1">{keys}</curve></parameter>"#);
    let props =
        format!(r#"<parameter name="Transform" id="100"><parameter name="Scale" id="105">{}{}{}</parameter></parameter>"#, ch("X", 1), ch("Y", 2), ch("Z", 3));
    let body = format!(r#"<layer name="G" id="3">{}</layer>"#, solid_node("S", "", &props));
    let text = doc(&body);
    let mut p = Project::default();
    let start = std::time::Instant::now();
    let r = import(&mut p, text.as_bytes(), "Long", &none).unwrap();
    let elapsed = start.elapsed();
    assert_eq!(tr(&main_comp(&p, r.comp).layers[0]).get("scale").unwrap().keys.len(), n);
    assert!(elapsed < std::time::Duration::from_secs(5), "{elapsed:?}");
}

/// `doc` with the Text and Shape factories Motion declares for those objects.
fn doc_with_text_and_shapes(body: &str) -> String {
    doc(body).replace(
        "<scene>",
        r#"<factory id="20" uuid="00000000000000000000000000000020"><description>Text</description><manufacturer>Apple</manufacturer><version>1</version></factory>
<factory id="21" uuid="00000000000000000000000000000021"><description>Shape</description><manufacturer>Apple</manufacturer><version>1</version></factory>
<scene>"#,
    )
}

/// A text object, a shape, a behavior and nested groups: the text and groups import, the shape
/// and the behavior are named in the warnings.
#[test]
fn text_shape_behavior_and_nested_groups() {
    let body = format!(
        r#"<layer name="Titles" id="3">
  <scenenode name="Title" id="13" factoryID="20"><text>Hello
Motion</text><style><font>Helvetica Neue</font><parameter name="Size" id="1" value="72"/></style>
    <behavior name="Throw" id="30" factoryID="7"/>
  </scenenode>
  <scenenode name="Circle" id="14" factoryID="21"/>
</layer>
<layer name="Outer" id="4"><layer name="Inner" id="5">{}</layer></layer>"#,
        solid_node("Fill", "", "")
    );
    let mut p = Project::default();
    let r = import(&mut p, doc_with_text_and_shapes(&body).as_bytes(), "Test", &none).unwrap();
    let w = &r.warnings;
    assert!(w.iter().any(|w| w.contains("Circle") && w.contains("shape objects are not imported")), "{w:?}");
    assert!(w.iter().any(|w| w.contains("Title") && w.contains("behavior `Throw` is not imported")), "{w:?}");
    // Two top-level groups: each a precomposition layer.
    let c = main_comp(&p, r.comp);
    let names: Vec<&str> = c.layers.iter().map(|l| l.name.as_str()).collect();
    assert_eq!(names, ["Titles", "Outer"]);
    let precomp = |l: &Layer| {
        let LayerSource::Comp { item } = l.source else { panic!("{} is not a precomp", l.name) };
        main_comp(&p, item)
    };
    // The text layer keeps its characters (line breaks as AE's \r), font and size.
    let titles = precomp(&c.layers[0]);
    assert_eq!(titles.layers.len(), 1, "the shape is skipped");
    let text = &titles.layers[0];
    assert_eq!(text.source, LayerSource::Text);
    let Value::Text(d) = &text.props.sub("text").unwrap().get("sourceText").unwrap().value else { panic!("text value") };
    assert_eq!((d.text.as_str(), d.font.as_str(), d.size), ("Hello\rMotion", "Helvetica Neue", 72.0));
    // Outer ▸ Inner ▸ Fill: a precomp inside a precomp.
    let inner = precomp(&precomp(&c.layers[1]).layers[0]);
    assert_eq!(inner.layers[0].name, "Fill");
}

/// Untrusted XML: entity declarations (the "billion laughs" bomb, external entities pointing at
/// local files) are refused before parsing; the bare DOCTYPE Motion writes is accepted.
#[test]
fn entity_declarations_are_refused() {
    let mut p = Project::default();
    let laughs = (1..10).map(|i| format!(r#"<!ENTITY l{i} "&l{0};&l{0};&l{0};&l{0};&l{0};&l{0};&l{0};&l{0};&l{0};&l{0};">"#, i - 1)).collect::<String>();
    let bomb = format!(r#"<?xml version="1.0"?><!DOCTYPE ozml [<!ENTITY l0 "lol">{laughs}]><ozml><scene><sceneSettings/><layer name="&l9;"/></scene></ozml>"#);
    let external =
        r#"<?xml version="1.0"?><!DOCTYPE ozml [<!ENTITY x SYSTEM "file:///etc/passwd">]><ozml><scene><sceneSettings/><layer name="&x;"/></scene></ozml>"#;
    for bad in [bomb.as_str(), external] {
        let e = import(&mut p, bad.as_bytes(), "X", &none).unwrap_err();
        assert!(e.to_string().contains("internal subset"), "{e}");
        assert!(clip_refs(bad.as_bytes()).is_err());
    }
    // An external DTD is never fetched: the document parses as if the DOCTYPE were bare.
    let system = doc("").replace("<!DOCTYPE ozxmlscene>", r#"<!DOCTYPE ozxmlscene SYSTEM "http://example.invalid/ozml.dtd">"#);
    assert!(import(&mut p, system.as_bytes(), "X", &none).is_ok());
    assert!(import(&mut p, doc("").as_bytes(), "X", &none).is_ok());
}
