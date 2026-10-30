//! Import of documents written in code with FilmCraft's writers, and export → re-import.

use std::collections::HashMap;
use std::sync::Arc;

use effectcraft_keyframe::{Keyframe, Value};
use effectcraft_project::{Comp, Footage, FootageKind, ItemId, ItemKind, Layer, LayerSource, Project};
use effectcraft_time::{FrameRate, Tick};
use filmcraft_project as fp;

use crate::*;

const W: u32 = 640;
const H: u32 = 360;

fn secs(s: f64) -> filmcraft_time::Tick {
    filmcraft_time::Tick::from_seconds_f64(s)
}

fn t(s: f64) -> Tick {
    Tick::from_seconds_f64(s)
}

fn info(kind: filmcraft_media::MediaKind, video: bool, audio: bool) -> filmcraft_media::MediaInfo {
    filmcraft_media::MediaInfo {
        name: String::new(),
        kind,
        duration: secs(20.0),
        video: video.then(|| filmcraft_media::VideoStreamInfo {
            width: W,
            height: H,
            frame_rate: filmcraft_time::FrameRate { num: 25, den: 1 },
            par: (1, 1),
            codec: "png".into(),
            pixel_format: String::new(),
            color: Default::default(),
            has_alpha: false,
            bitrate: None,
        }),
        audio: audio.then(|| filmcraft_media::AudioStreamInfo { sample_rate: 48_000, channels: 2, codec: "pcm".into(), bits_per_sample: Some(16) }),
        container: String::new(),
        start_timecode: None,
        file_size: None,
    }
}

fn media(p: &mut fp::Project, name: &str, path: &str, kind: filmcraft_media::MediaKind, video: bool, audio: bool, bin: Option<fp::BinId>) -> fp::ItemId {
    let clip = fp::MediaClip {
        media: fp::MediaRef::File { path: path.into() },
        info: info(kind, video, audio),
        interpret: Default::default(),
        mark_in: None,
        mark_out: None,
        markers: vec![],
        offline: false,
        proxy: None,
        identity: None,
    };
    p.add_item(name, fp::Label::Iris, fp::ItemKind::Media(clip), bin)
}

fn rate() -> filmcraft_time::FrameRate {
    filmcraft_time::FrameRate { num: 25, den: 1 }
}

fn item(p: &mut fp::Project, it: fp::ItemId, kind: fp::TrackKind, start: f64, src: f64, dur: f64) -> fp::TrackItem {
    p.make_track_item(it, kind, secs(start), filmcraft_time::TimeRange::new(secs(src), secs(dur)), rate()).unwrap()
}

/// A Premiere-like edit: two clips with a cross dissolve on V1, a speed-changed, scaled and
/// faded clip on V2, a nested sequence on V3, and an audio clip at -6 dB on A1.
fn edit() -> (fp::Project, fp::ItemId) {
    let mut p = fp::Project::new("Edit");
    let bin = p.add_bin("Footage", None);
    let a = media(&mut p, "a.png", "/media/a.mov", filmcraft_media::MediaKind::Movie, true, false, Some(bin));
    let b = media(&mut p, "b.png", "/media/b.mov", filmcraft_media::MediaKind::Movie, true, false, Some(bin));
    let s = media(&mut p, "tone.wav", "/media/tone.wav", filmcraft_media::MediaKind::AudioOnly, false, true, Some(bin));
    let settings = fp::SequenceSettings { width: W, height: H, frame_rate: rate(), ..Default::default() };
    let nested = p.new_sequence("Nested", settings.clone(), 1, 0, None);
    let n_clip = item(&mut p, a, fp::TrackKind::Video, 0.0, 0.0, 2.0);
    p.sequence_mut(nested).unwrap().video_tracks[0].items.push(n_clip);
    let main = p.new_sequence("Main", settings, 3, 1, None);

    let ca = item(&mut p, a, fp::TrackKind::Video, 0.0, 0.0, 2.0);
    let cb = item(&mut p, b, fp::TrackKind::Video, 2.0, 1.0, 2.0);
    let tr = fp::Transition {
        id: fp::TransitionId(p.alloc_id()),
        effect: fp::find_effect("cross_dissolve").unwrap().instance(),
        start: secs(1.6),
        duration: secs(0.8),
        from: Some(ca.id),
        to: Some(cb.id),
        align: fp::TransitionAlign::CenterAtCut,
        reverse: false,
    };
    let mut cc = item(&mut p, b, fp::TrackKind::Video, 1.0, 2.0, 2.0);
    cc.speed = 2.0;
    let m = cc.effect_mut("motion").unwrap();
    m.params.insert("scale".into(), fp::Param::new(fp::ParamValue::Float(50.0)));
    m.params.insert("rotation".into(), fp::Param::new(fp::ParamValue::Float(30.0)));
    let mut pos = fp::Param::new(fp::ParamValue::Vec2(filmcraft_geom::Vec2 { x: 100.0, y: 100.0 }));
    pos.keyframes = vec![
        fp::Keyframe::new(secs(0.0), fp::ParamValue::Vec2(filmcraft_geom::Vec2 { x: 100.0, y: 100.0 })),
        fp::Keyframe::new(secs(1.0), fp::ParamValue::Vec2(filmcraft_geom::Vec2 { x: 300.0, y: 200.0 })),
    ];
    m.params.insert("position".into(), pos);
    cc.effect_mut("opacity").unwrap().params.insert("opacity".into(), fp::Param::new(fp::ParamValue::Float(50.0)));
    let cn = item(&mut p, nested, fp::TrackKind::Video, 4.0, 0.0, 2.0);
    let mut cs = item(&mut p, s, fp::TrackKind::Audio, 0.0, 0.0, 3.0);
    cs.effect_mut("volume").unwrap().params.insert("level".into(), fp::Param::new(fp::ParamValue::Float(-6.0)));
    let seq = p.sequence_mut(main).unwrap();
    seq.video_tracks[0].items = vec![ca, cb];
    seq.video_tracks[0].transitions = vec![tr];
    seq.video_tracks[1].items = vec![cc];
    seq.video_tracks[2].items = vec![cn];
    seq.audio_tracks[0].items = vec![cs];
    (p, main)
}

fn probe(path: &str) -> Option<Footage> {
    let audio = path.ends_with(".wav");
    Some(Footage {
        path: path.into(),
        kind: if audio { FootageKind::Audio } else { FootageKind::Video },
        width: if audio { 0 } else { W },
        height: if audio { 0 } else { H },
        frame_rate: FrameRate::new(25, 1),
        duration: t(20.0),
        has_video: !audio,
        has_audio: audio,
        ..Default::default()
    })
}

fn import_doc(bytes: &[u8], name: &str, probe_fn: &mut dyn FnMut(&str) -> Option<Footage>) -> (Project, ImportResult) {
    let mut p = Project::default();
    let r = import(&mut p, bytes, None, &ImportOptions { name: name.into(), ..Default::default() }, probe_fn).unwrap();
    (p, r)
}

fn layer<'a>(c: &'a Comp, name: &str) -> &'a Layer {
    c.layers.iter().find(|l| l.name == name).unwrap_or_else(|| panic!("no layer {name}: {:?}", c.layers.iter().map(|l| &l.name).collect::<Vec<_>>()))
}

fn near(a: Tick, b: Tick) -> bool {
    (a.0 - b.0).abs() <= Tick::from_seconds_f64(0.001).0
}

fn check_structure(p: &Project, r: &ImportResult, motion: bool) {
    let main = r.comps.iter().copied().find(|c| p.item(*c).unwrap().name == "Main").expect("Main comp");
    let c = p.comp(main).unwrap();
    assert_eq!((c.width, c.height), (W, H));
    assert_eq!(c.frame_rate, FrameRate::new(25, 1));
    assert!(near(c.duration, t(6.0)), "{:?}", c.duration.seconds());
    // V3 (nested) on top, then V2, then V1's later clip, then the earlier one; audio last.
    let names: Vec<&str> = c.layers.iter().map(|l| l.name.as_str()).collect();
    let pos = |n: &str| names.iter().position(|x| x.starts_with(n)).unwrap_or_else(|| panic!("{n} in {names:?}"));
    assert!(pos("Nested") < pos("b.png"), "{names:?}");
    let nested = layer(c, "Nested");
    assert!(matches!(nested.source, LayerSource::Comp { .. }));
    assert!(near(nested.in_point, t(4.0)) && near(nested.out_point, t(6.0)));
    // The faster clip: 200 % speed → 50 % stretch, source 2 s at its in point.
    let fast = c.layers.iter().find(|l| (l.stretch - 50.0).abs() < 1e-6).expect("stretched layer");
    assert!(near(fast.in_point, t(1.0)) && near(fast.out_point, t(3.0)));
    assert!(near(fast.layer_time(t(1.0)), t(2.0)), "{}", fast.layer_time(t(1.0)).seconds());
    // V1: b above a, overlapping over the dissolve; b fades in.
    let v1: Vec<&Layer> =
        c.layers.iter().filter(|l| (l.stretch - 100.0).abs() < 1e-6 && matches!(l.source, LayerSource::Footage { .. }) && l.switches.video).collect();
    assert_eq!(v1.len(), 2, "{names:?}");
    let (lb, la) = (v1[0], v1[1]);
    assert!(near(la.in_point, t(0.0)) && near(la.out_point, t(2.4)), "{:?}", la.out_point.seconds());
    assert!(near(lb.in_point, t(1.6)) && near(lb.out_point, t(4.0)), "{:?}", lb.in_point.seconds());
    assert!(near(lb.layer_time(t(2.0)), t(1.0)));
    let op = lb.transform().unwrap().get("opacity").unwrap();
    assert_eq!(op.keys.len(), 2);
    assert!(op.value_at(lb.layer_time(t(1.6))).as_f64() < 1e-6);
    assert!((op.value_at(lb.layer_time(t(2.0))).as_f64() - 50.0).abs() < 1.0);
    assert!((op.value_at(lb.layer_time(t(3.0))).as_f64() - 100.0).abs() < 1e-6);
    // Audio: an audio-only layer at -6 dB.
    let au = c.layers.iter().find(|l| !l.switches.video && l.switches.audio).expect("audio layer");
    assert!(near(au.out_point, t(3.0)));
    if motion {
        let lv = au.props.prop("audio/levels").unwrap().value.as_vec2();
        assert!((lv[0] + 6.0).abs() < 0.01, "{lv:?}");
        let tr = fast.transform().unwrap();
        assert_eq!(tr.get("scale").unwrap().value.as_vec3()[..2], [50.0, 50.0]);
        assert!((tr.get("rotation").unwrap().value.as_f64() - 30.0).abs() < 1e-6);
        assert!((tr.get("opacity").unwrap().value.as_f64() - 50.0).abs() < 1e-6);
        let pk = &tr.get("position").unwrap().keys;
        assert_eq!(pk.len(), 2);
        // Clip time 1 s = comp 2 s = layer time 4 s at 200 % speed.
        assert!(near(pk[1].time, fast.layer_time(t(2.0))));
        assert_eq!(pk[1].value.as_vec3()[..2], [300.0, 200.0]);
    }
    // Everything lands in the document's folder.
    assert!(p.item(r.folder).unwrap().is_folder());
    assert!(p.items.values().filter(|i| i.id != r.folder).all(|i| i.parent.is_some()));
}

/// A Premiere-style xmeml export with a bin, a master clip, Basic Motion and Opacity keyframes.
const PREMIERE_XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE xmeml>
<xmeml version="4">
  <project>
    <name>Promo</name>
    <children>
      <bin>
        <name>Shots</name>
        <children>
          <bin>
            <name>Day 1</name>
            <children>
              <clip id="masterclip-1">
                <name>wide.mov</name>
                <duration>250</duration>
                <rate><timebase>25</timebase><ntsc>FALSE</ntsc></rate>
                <media><video><track><clipitem id="clipitem-m1"><name>wide.mov</name>
                  <file id="file-1">
                    <name>wide.mov</name>
                    <pathurl>file://localhost/media/wide%20shot.mov</pathurl>
                    <rate><timebase>25</timebase><ntsc>FALSE</ntsc></rate>
                    <duration>250</duration>
                    <media><video><samplecharacteristics><width>640</width><height>360</height></samplecharacteristics></video></media>
                  </file>
                </clipitem></track></video></media>
              </clip>
            </children>
          </bin>
        </children>
      </bin>
      <sequence id="sequence-1">
        <name>Promo Cut</name>
        <duration>50</duration>
        <rate><timebase>25</timebase><ntsc>FALSE</ntsc></rate>
        <media>
          <video>
            <format><samplecharacteristics><rate><timebase>25</timebase></rate><width>640</width><height>360</height><pixelaspectratio>square</pixelaspectratio></samplecharacteristics></format>
            <track>
              <clipitem id="clipitem-1">
                <masterclipid>masterclip-1</masterclipid>
                <name>wide.mov</name>
                <enabled>TRUE</enabled>
                <rate><timebase>25</timebase><ntsc>FALSE</ntsc></rate>
                <start>0</start><end>50</end><in>25</in><out>75</out>
                <file id="file-1"/>
                <filter>
                  <effect>
                    <name>Basic Motion</name><effectid>basic</effectid><effectcategory>motion</effectcategory><effecttype>motion</effecttype><mediatype>video</mediatype>
                    <parameter><parameterid>scale</parameterid><name>Scale</name><value>150</value></parameter>
                    <parameter><parameterid>center</parameterid><name>Center</name><value><horiz>0.25</horiz><vert>0</vert></value></parameter>
                  </effect>
                </filter>
                <filter>
                  <effect>
                    <name>Opacity</name><effectid>opacity</effectid><effectcategory>motion</effectcategory><effecttype>motion</effecttype><mediatype>video</mediatype>
                    <parameter><parameterid>opacity</parameterid><name>Opacity</name>
                      <keyframe><when>0</when><value>0</value></keyframe>
                      <keyframe><when>25</when><value>100</value></keyframe>
                    </parameter>
                  </effect>
                </filter>
              </clipitem>
            </track>
          </video>
        </media>
      </sequence>
    </children>
  </project>
</xmeml>
"#;

#[test]
fn premiere_xml_bins_motion_and_opacity() {
    let mut paths = vec![];
    let (p, r) = import_doc(PREMIERE_XML.as_bytes(), "Promo.xml", &mut |path| {
        paths.push(path.to_string());
        probe(path)
    });
    assert_eq!(paths, vec!["/media/wide shot.mov".to_string()]);
    let shots = p.items.values().find(|i| i.name == "Shots").unwrap();
    assert_eq!(shots.parent, Some(r.folder));
    let day = p.items.values().find(|i| i.name == "Day 1").unwrap();
    assert_eq!(day.parent, Some(shots.id));
    assert!(p.items.values().any(|i| i.name == "wide.mov" && i.parent == Some(day.id)));
    let c = p.comp(r.comps[0]).unwrap();
    assert_eq!(p.item(r.comps[0]).unwrap().name, "Promo Cut");
    assert!(near(c.duration, t(2.0)));
    let l = &c.layers[0];
    assert!(near(l.layer_time(Tick::ZERO), t(1.0)), "source in 1 s");
    let tr = l.transform().unwrap();
    assert_eq!(tr.get("scale").unwrap().value.as_vec3()[..2], [150.0, 150.0]);
    // Center 0.25 of the width right of the frame centre.
    assert_eq!(tr.get("position").unwrap().value.as_vec3()[..2], [320.0 + 160.0, 180.0]);
    let op = tr.get("opacity").unwrap();
    assert_eq!(op.keys.len(), 2);
    assert!(near(op.keys[1].time, l.layer_time(t(1.0))));
    assert!((op.value_at(l.layer_time(t(0.5))).as_f64() - 50.0).abs() < 1e-6);
}

#[test]
fn imports_fcp7_xml_fcpxml_and_otio() {
    let (doc, main) = edit();
    for (fmt, motion) in [(fc::Format::Fcp7Xml, true), (fc::Format::Fcpxml, false), (fc::Format::Otio, false)] {
        let (bytes, _) = fc::export(&doc, main, fmt, &Default::default()).unwrap();
        let name = format!("Main.{}", fmt.extension());
        assert_eq!(TimelineFormat::detect(&bytes, Some(&name)).map(|f| f.fc()), Some(fmt));
        let (p, r) = import_doc(&bytes, &name, &mut probe);
        assert!(r.missing.is_empty(), "{fmt:?}: {:?}", r.missing);
        check_structure(&p, &r, motion);
    }
}

#[test]
fn imports_aaf_and_omf() {
    let (doc, main) = edit();
    for fmt in [fc::Format::Aaf, fc::Format::Omf] {
        let (bytes, _) = fc::export(&doc, main, fmt, &Default::default()).unwrap();
        let name = format!("Main.{}", fmt.extension());
        assert_eq!(TimelineFormat::detect(&bytes, Some(&name)).map(|f| f.fc()), Some(fmt));
        let (p, r) = import_doc(&bytes, &name, &mut probe);
        let c = p.comp(r.comps[0]).unwrap();
        if fmt == fc::Format::Aaf {
            assert_eq!((c.width, c.height), (W, H));
        }
        // Audio: an audio-only layer for the A1 clip (OMF carries the audio tracks only).
        let au = c.layers.iter().find(|l| !l.switches.video && l.switches.audio).expect("audio layer");
        assert!(near(au.in_point, Tick::ZERO) && near(au.out_point, t(3.0)));
        if fmt == fc::Format::Aaf {
            // The dissolve's clips overlap over the transition, the incoming one on top.
            let a = layer(c, "a.png");
            assert!(near(a.out_point, t(2.4)), "{}", a.out_point.seconds());
            let b = c.layers.iter().find(|l| near(l.in_point, t(1.6))).expect("incoming clip");
            assert!(c.index_of(b.id) < c.index_of(a.id));
            assert_eq!(b.transform().unwrap().get("opacity").unwrap().keys.len(), 2);
        }
    }
}

#[test]
fn missing_media_become_placeholders() {
    let (doc, main) = edit();
    let (bytes, _) = fc::export(&doc, main, fc::Format::Fcp7Xml, &Default::default()).unwrap();
    let (p, r) = import_doc(&bytes, "Main.xml", &mut |_| None);
    assert_eq!(r.missing.len(), 3, "{:?}", r.missing);
    let a = p.items.values().find(|i| i.name == "a.png").unwrap();
    match &a.kind {
        ItemKind::Footage(f) => assert!(f.missing && f.width == W && f.height == H && f.has_video),
        _ => panic!(),
    }
    check_structure(&p, &r, true);
}

#[test]
fn edl_imports_one_track() {
    let (doc, main) = edit();
    let (bytes, _) = fc::export(&doc, main, fc::Format::Edl, &Default::default()).unwrap();
    let mut p = Project::default();
    let r = import(&mut p, &bytes, None, &ImportOptions { name: "Main.edl".into(), edl_frame_rate: Some(25.0), ..Default::default() }, &mut probe).unwrap();
    assert_eq!(r.format, Some(TimelineFormat::Edl));
    let c = p.comp(r.comps[0]).unwrap();
    assert!(c.layers.len() >= 2);
}

#[test]
fn rejects_unknown_documents() {
    let mut p = Project::default();
    assert!(matches!(import(&mut p, b"hello", None, &ImportOptions::default(), &mut probe), Err(Error::UnknownFormat)));
}

// ---------------------------------------------------------------- export

fn footage_item(p: &mut Project, path: &str) -> ItemId {
    let f = probe(path).unwrap();
    p.add_item(path.rsplit('/').next().unwrap(), effectcraft_color::Label::Aqua, None, ItemKind::Footage(f))
}

fn comp_item(p: &mut Project, name: &str, c: Comp) -> ItemId {
    p.add_item(name, effectcraft_color::Label::Sandstone, None, ItemKind::Comp(Arc::new(c)))
}

/// A comp with a stretched, keyframed footage layer, a text layer, a precomp and an audio layer.
fn ae_project() -> (Project, ItemId) {
    let mut p = Project::default();
    let a = footage_item(&mut p, "/media/a.mov");
    let s = footage_item(&mut p, "/media/tone.wav");
    let mut inner = Comp::new(W, H, FrameRate::new(25, 1), t(4.0));
    let li = effectcraft_project::build::layer(&mut p, &inner, "inner a", LayerSource::Footage { item: a }, (W, H), None);
    inner.layers.push(li);
    let inner_id = comp_item(&mut p, "Inner", inner);
    let mut c = Comp::new(W, H, FrameRate::new(25, 1), t(5.0));
    let mut la = effectcraft_project::build::layer(&mut p, &c, "clip a", LayerSource::Footage { item: a }, (W, H), None);
    la.stretch = 200.0; // half speed
    la.start_time = t(-1.0); // layer time 1 s at comp 3 s… source 1 s at comp 1 s
    la.in_point = t(1.0);
    la.out_point = t(4.0);
    {
        let tr = la.transform_mut().unwrap();
        let pos = tr.get_mut("position").unwrap();
        pos.keys = vec![Keyframe::new(t(1.0), Value::Vec3([100.0, 120.0, 0.0])), Keyframe::new(t(2.0), Value::Vec3([500.0, 240.0, 0.0])).hold()];
        tr.get_mut("scale").unwrap().value = Value::Vec3([80.0, 80.0, 100.0]);
        tr.get_mut("rotation").unwrap().value = Value::Scalar(-15.0);
        tr.get_mut("opacity").unwrap().value = Value::Scalar(75.0);
    }
    let mut lt = effectcraft_project::build::layer(&mut p, &c, "Title", LayerSource::Text, (W, H), None);
    lt.in_point = t(0.6);
    lt.out_point = t(2.6);
    let mut lp = effectcraft_project::build::layer(&mut p, &c, "Inner", LayerSource::Comp { item: inner_id }, (W, H), None);
    lp.start_time = t(2.0);
    lp.in_point = t(2.0);
    lp.out_point = t(5.0);
    let mut ls = effectcraft_project::build::layer(&mut p, &c, "tone", LayerSource::Footage { item: s }, (0, 0), None);
    ls.out_point = t(3.0);
    ls.props.prop_mut("audio/levels").unwrap().value = Value::Vec2([-3.0, -3.0]);
    c.layers = vec![lt, lp, la, ls].into();
    let cid = comp_item(&mut p, "Main", c);
    (p, cid)
}

#[test]
fn plan_prerenders_rendered_only_layers() {
    let (p, cid) = ae_project();
    let opts = ExportOptions::default();
    let plan = plan_prerender(&p, cid, &opts);
    let c = p.comp(cid).unwrap();
    assert_eq!(plan, vec![(cid, layer(c, "Title").id)]);
    assert_eq!(prerender_reason(&p, cid, layer(c, "Title").id, &opts).as_deref(), Some("text layer"));
    let all = plan_prerender(&p, cid, &ExportOptions { prerender: PrerenderMode::All, ..Default::default() });
    assert_eq!(all.len(), 3, "audio-only layers are never rendered");
    assert!(plan_prerender(&p, cid, &ExportOptions { prerender: PrerenderMode::None, ..Default::default() }).is_empty());
}

#[test]
fn export_then_reimport_keeps_the_timeline() {
    let (p, cid) = ae_project();
    let c = p.comp(cid).unwrap();
    let title = layer(c, "Title").id;
    let mut pre = HashMap::new();
    pre.insert((cid, title), Prerendered { path: "/render/Title.mov".into(), width: W, height: H, frame_rate: FrameRate::new(25, 1), duration: t(2.0) });
    for format in [TimelineFormat::Fcp7Xml, TimelineFormat::Fcpxml, TimelineFormat::Otio] {
        let out = export(&p, cid, &ExportOptions { format, ..Default::default() }, &pre).unwrap();
        assert_eq!(out.prerendered, 1);
        assert_eq!(out.sequences, 2);
        assert_eq!(out.video_tracks, 4, "3 in Main + 1 in Inner");
        assert_eq!(out.audio_tracks, 1);
        let text = String::from_utf8(out.bytes.clone()).unwrap();
        assert!(text.contains("Title.mov"), "{format:?}");
        let (q, r) = import_doc(&out.bytes, &format!("Main.{}", format.extension()), &mut probe);
        let main = r.comps.iter().copied().find(|c| q.item(*c).unwrap().name == "Main").expect("Main");
        let m = q.comp(main).unwrap();
        assert_eq!(m.layers.len(), 4, "{format:?}: {:?}", m.layers.iter().map(|l| &l.name).collect::<Vec<_>>());
        // Stacking order kept: Title (pre-rendered) on top, then the precomp, then clip a.
        assert_eq!(m.layers[0].name, "Title");
        assert!(near(m.layers[0].in_point, t(0.6)) && near(m.layers[0].out_point, t(2.6)));
        assert!(matches!(m.layers[1].source, LayerSource::Comp { .. }), "{format:?}");
        assert!(near(m.layers[1].in_point, t(2.0)) && near(m.layers[1].layer_time(t(2.0)), Tick::ZERO));
        let la = &m.layers[2];
        assert!((la.stretch - 200.0).abs() < 1e-6, "{format:?} stretch {}", la.stretch);
        assert!(near(la.in_point, t(1.0)) && near(la.out_point, t(4.0)));
        let orig = layer(c, "clip a");
        assert!(near(la.layer_time(t(1.0)), orig.layer_time(t(1.0))), "{format:?} {}", la.layer_time(t(1.0)).seconds());
        assert!(!m.layers[3].switches.video && m.layers[3].switches.audio);
        if format == TimelineFormat::Fcp7Xml {
            let tr = la.transform().unwrap();
            assert_eq!(tr.get("scale").unwrap().value.as_vec3()[..2], [80.0, 80.0]);
            assert!((tr.get("rotation").unwrap().value.as_f64() + 15.0).abs() < 1e-6);
            assert!((tr.get("opacity").unwrap().value.as_f64() - 75.0).abs() < 1e-6);
            let pk = &tr.get("position").unwrap().keys;
            assert_eq!(pk.len(), 2);
            for (k, o) in pk.iter().zip(&tr_orig(orig).keys) {
                assert!(near(la.comp_time(k.time), orig.comp_time(o.time)), "{} vs {}", k.time.seconds(), o.time.seconds());
                assert_eq!(k.value.as_vec3()[..2], o.value.as_vec3()[..2]);
            }
            let lv = m.layers[3].props.prop("audio/levels").unwrap().value.as_vec2();
            assert!((lv[0] + 3.0).abs() < 0.01, "{lv:?}");
        }
    }
}

fn tr_orig(l: &Layer) -> &effectcraft_project::Property {
    l.transform().unwrap().get("position").unwrap()
}

/// #1894: XML timelines have no mute state, so a muted key must not be written as a keyframe
/// (the export would otherwise animate through it).
#[test]
fn export_skips_muted_keyframes() {
    let mut p = Project::default();
    let a = footage_item(&mut p, "/media/a.mov");
    let mut c = Comp::new(W, H, FrameRate::new(25, 1), t(4.0));
    let mut l = effectcraft_project::build::layer(&mut p, &c, "clip", LayerSource::Footage { item: a }, (W, H), None);
    {
        let pos = l.transform_mut().unwrap().get_mut("position").unwrap();
        pos.keys = vec![
            Keyframe::new(t(0.0), Value::Vec3([100.0, 120.0, 0.0])),
            Keyframe::new(t(1.0), Value::Vec3([500.0, 240.0, 0.0])),
            Keyframe::new(t(2.0), Value::Vec3([300.0, 60.0, 0.0])),
        ];
        pos.keys[1].muted = true;
    }
    c.layers.push(l);
    let cid = comp_item(&mut p, "Main", c);
    let out = export(&p, cid, &ExportOptions { format: TimelineFormat::Fcp7Xml, ..Default::default() }, &HashMap::new()).unwrap();
    let (q, r) = import_doc(&out.bytes, "Main.xml", &mut probe);
    let main = r.comps.iter().copied().find(|c| q.item(*c).unwrap().name == "Main").expect("Main");
    let pos = layer(q.comp(main).unwrap(), "clip").transform().unwrap().get("position").unwrap();
    assert_eq!(pos.keys.len(), 2, "the muted key was exported");
    assert_eq!(pos.keys[0].value.as_vec3()[0], 100.0);
    assert_eq!(pos.keys[1].value.as_vec3()[0], 300.0);
}

/// #1894: a muted non-uniform scale key must not force a uniform layer to pre-render; only the
/// active keys matter, and the static value when every key is muted.
#[test]
fn muted_non_uniform_scale_does_not_force_prerendering() {
    let mut p = Project::default();
    let a = footage_item(&mut p, "/media/a.mov");
    let mut c = Comp::new(W, H, FrameRate::new(25, 1), t(2.0));
    let mut l = effectcraft_project::build::layer(&mut p, &c, "clip", LayerSource::Footage { item: a }, (W, H), None);
    {
        let scale = l.transform_mut().unwrap().get_mut("scale").unwrap();
        scale.keys = vec![Keyframe::new(t(0.0), Value::Vec3([100.0, 100.0, 100.0])), Keyframe::new(t(1.0), Value::Vec3([50.0, 200.0, 100.0]))];
        scale.keys[1].muted = true;
    }
    let lid = l.id;
    c.layers.push(l);
    let cid = comp_item(&mut p, "Main", c);
    assert_eq!(prerender_reason(&p, cid, lid, &ExportOptions::default()), None);
    // With every key muted the static value drives evaluation, so a non-uniform static scale
    // still takes the flattened path.
    let scale = p.comp_mut(cid).unwrap().layer_mut(lid).unwrap().transform_mut().unwrap().get_mut("scale").unwrap();
    scale.keys[0].muted = true;
    scale.value = Value::Vec3([50.0, 200.0, 100.0]);
    assert_eq!(prerender_reason(&p, cid, lid, &ExportOptions::default()).as_deref(), Some("non-uniform scale"));
}

#[test]
fn export_without_prerender_leaves_text_out() {
    let (p, cid) = ae_project();
    let out = export(&p, cid, &ExportOptions { prerender: PrerenderMode::None, ..Default::default() }, &HashMap::new()).unwrap();
    assert!(out.warnings.iter().any(|w| w.contains("Title")), "{:?}", out.warnings);
    assert_eq!(out.prerendered, 0);
    assert_eq!(out.video_tracks, 3);
}

#[test]
fn formats_parse() {
    assert_eq!(TimelineFormat::parse("premiere"), Some(TimelineFormat::Fcp7Xml));
    assert_eq!(TimelineFormat::parse(".fcpxml"), Some(TimelineFormat::Fcpxml));
    assert_eq!(TimelineFormat::from_path("/a/b.otio"), Some(TimelineFormat::Otio));
    assert_eq!(TimelineFormat::Fcp7Xml.extension(), "xml");
    assert_eq!(par_fraction(0.9), (9, 10));
    assert_eq!(par_fraction(1.0), (1, 1));
}

#[test]
fn export_survives_a_precomp_layer_pointing_at_a_non_comp() {
    // A damaged project: the precomp layer's source is a footage item (or a deleted item).
    let (mut p, cid) = ae_project();
    let footage = p.items.iter().find(|(_, i)| matches!(i.kind, ItemKind::Footage(_))).map(|(id, _)| *id).unwrap();
    for bad in [footage, ItemId(9_999)] {
        let mut c = (*p.comp_arc(cid).unwrap()).clone();
        for l in &mut c.layers {
            if matches!(l.source, LayerSource::Comp { .. }) {
                l.source = LayerSource::Comp { item: bad };
            }
        }
        p.item_mut(cid).unwrap().kind = ItemKind::Comp(Arc::new(c));
        for format in [TimelineFormat::Fcp7Xml, TimelineFormat::Fcpxml, TimelineFormat::Otio] {
            let out = export(&p, cid, &ExportOptions { format, prerender: PrerenderMode::None, ..Default::default() }, &HashMap::new()).unwrap();
            assert_eq!(out.sequences, 1);
        }
    }
    assert!(matches!(export(&p, footage, &ExportOptions::default(), &HashMap::new()), Err(Error::NoComp(_))));
}

#[test]
fn xml_timelines_declaring_entities_are_refused() {
    // #2129: the timeline parser expands internal entities without a node limit.
    let dtd = "<!DOCTYPE x [<!ENTITY a \"aaaaaaaaaa\"><!ENTITY b \"&a;&a;&a;&a;&a;&a;&a;&a;&a;&a;\">]>";
    let docs = [
        (TimelineFormat::Fcp7Xml, format!("<?xml version=\"1.0\"?>{dtd}<xmeml version=\"5\">&b;</xmeml>")),
        (TimelineFormat::Fcpxml, format!("<?xml version=\"1.0\"?>{dtd}<fcpxml version=\"1.9\">&b;</fcpxml>")),
    ];
    for (format, doc) in docs {
        let mut p = Project::default();
        let r = import(&mut p, doc.as_bytes(), Some(format), &ImportOptions::default(), &mut |_| None);
        assert!(matches!(r, Err(Error::XmlEntities)), "{format:?}");
    }
}

#[test]
fn deeply_nested_documents_error_instead_of_overflowing_the_stack() {
    // A few hundred kilobytes nested 100 000 levels deep overflowed the parsers' stack.
    let n = 100_000;
    let docs = [
        (TimelineFormat::Fcp7Xml, format!("<?xml version=\"1.0\"?><xmeml version=\"5\">{}{}</xmeml>", "<a>".repeat(n), "</a>".repeat(n))),
        (TimelineFormat::Fcpxml, format!("<?xml version=\"1.0\"?><fcpxml version=\"1.9\">{}{}</fcpxml>", "<a>".repeat(n), "</a>".repeat(n))),
        (TimelineFormat::Otio, format!("{{\"OTIO_SCHEMA\":\"Timeline.1\",\"tracks\":{}{}}}", "[".repeat(n), "]".repeat(n))),
    ];
    for (format, doc) in docs {
        let mut p = Project::default();
        let r = import(&mut p, doc.as_bytes(), Some(format), &ImportOptions::default(), &mut |_| None);
        assert!(matches!(r, Err(Error::TooDeep(_))), "{format:?}");
    }
    assert_eq!(super::import::xml_depth(b"<a><!-- <b><b> --><c x='>'/><![CDATA[<d>]]><e></e></a>"), 2);
    assert_eq!(super::import::json_depth(br#"{"a": "[[[{{", "b": [[1], {"c": "\\\"["}]}"#), 3);
}
