//! Markers (122, 123, 249, 347-349, 509, 510) and Time Remapping (319, 354) import with the
//! values the cases were made with.

mod common;

use common::{comp, load, try_import};
use effectcraft_color::Label;
use effectcraft_keyframe::{Interp, Value};
use effectcraft_project::{Marker, Project};
use effectcraft_time::{TICKS_PER_SECOND, Tick};

fn secs(s: f64) -> Tick {
    Tick((s * TICKS_PER_SECOND as f64).round() as i64)
}

fn lossless(name: &str) -> (Project, effectcraft_aep_import::ImportResult) {
    let mut p = Project::default();
    let r = try_import(&mut p, name, false).unwrap_or_else(|e| panic!("{name}: {e}"));
    (p, r)
}

fn layer_markers(name: &str, layer: &str) -> Vec<Marker> {
    let (p, r) = lossless(name);
    let (_, c) = comp(&p, &r, "C");
    c.layers.iter().find(|l| l.name == layer).expect("layer").markers.clone()
}

#[test]
fn composition_markers() {
    let (p, r) = lossless("122-marker-comp-1s.aep");
    let (_, c) = comp(&p, &r, "C");
    assert_eq!(c.markers.len(), 1);
    assert_eq!((c.markers[0].time, c.markers[0].comment.as_str(), c.markers[0].duration), (secs(1.0), "CompMarker", Tick::ZERO));
    assert_eq!(r.report.imported.markers, 1);

    // Two markers, one with a label and one with a duration of 0.5 s (300 / 600).
    let (p, r) = lossless("509-marker-comp-two.aep");
    let (_, c) = comp(&p, &r, "C");
    let m: Vec<_> = c.markers.iter().map(|m| (m.time, m.comment.as_str(), m.duration, m.label)).collect();
    assert_eq!(m, vec![(secs(1.0), "M1", Tick::ZERO, Label::Yellow), (secs(2.5), "M2", secs(0.5), Label::None)]);
}

#[test]
fn layer_markers_with_every_field() {
    let m = layer_markers("123-marker-layer-1s.aep", "L");
    assert_eq!((m.len(), m[0].time, m[0].comment.as_str()), (1, secs(1.0), "LayerMarker"));

    let m = &layer_markers("249-marker-layer-full.aep", "L")[0];
    assert_eq!(
        (m.time, m.duration, m.comment.as_str(), m.chapter.as_str(), m.url.as_str(), m.frame_target.as_str()),
        (secs(1.0), secs(1.0), "LM", "Chap", "http://a.example/u", "_blank")
    );
    let cue = m.cue_point.as_ref().expect("cue point");
    assert_eq!((cue.name.as_str(), cue.navigation, cue.params.len()), ("CueX", false, 0));
    assert_eq!(m.label, Label::None);

    assert_eq!(layer_markers("347-marker-layer-label3.aep", "L")[0].label, Label::Aqua);
    let n = &layer_markers("348-marker-layer-navigation-cue.aep", "L")[0];
    assert_eq!(n.cue_point.as_ref().map(|c| (c.name.as_str(), c.navigation)), Some(("CueN", true)));
    let e = &layer_markers("349-marker-layer-event-cue-params.aep", "L")[0];
    let cue = e.cue_point.as_ref().expect("cue");
    assert_eq!((cue.name.as_str(), cue.navigation), ("CueE", false));
    assert_eq!(cue.params, vec![("speaker".to_string(), "A".to_string()), ("n".to_string(), "2".to_string())]);
}

#[test]
fn a_marker_at_a_fractional_time_with_a_fractional_duration() {
    // 0.52 s is 13 312 / 25 600 (frame 13); 0.3 s is 180 / 600.
    let m = layer_markers("510-marker-layer-0.52-dur0.3.aep", "L");
    assert_eq!((m.len(), m[0].time, m[0].duration, m[0].comment.as_str()), (1, secs(0.52), secs(0.3), "LM"));
}

#[test]
fn time_remapping_becomes_the_time_remap_property() {
    for (name, layer, end) in [("319-precomp-timeremap.aep", "Pre", 5.0), ("354-footage-pngseq-timeremap.aep", "seq_001.png", 0.1)] {
        let (p, r) = load(name);
        let (_, c) = comp(&p, &r, "C");
        let l = c.layers.iter().find(|l| l.name == layer).unwrap_or_else(|| panic!("{name}: {:?}", c.layers.iter().map(|l| &l.name).collect::<Vec<_>>()));
        let tr = l.props.get("timeRemap").unwrap_or_else(|| panic!("{name}: no timeRemap"));
        let keys: Vec<_> = tr.keys.iter().map(|k| (k.time, k.value.clone(), k.in_interp, k.out_interp)).collect();
        assert_eq!(
            keys,
            vec![(Tick::ZERO, Value::Scalar(0.0), Interp::Linear, Interp::Linear), (secs(end), Value::Scalar(end), Interp::Linear, Interp::Linear)],
            "{name}"
        );
        assert_eq!(l.props.children.first().map(|n| n.match_id()), Some("timeRemap"), "{name}: first property");
        assert_eq!(r.report.imported.time_remaps, 1, "{name}");
        assert_eq!(r.report.count(effectcraft_aep_import::LossCategory::TimeRemapping), 0, "{name}");
    }
    // A layer without remapping gets no property.
    let (p, r) = load("000-base.aep");
    let (_, c) = comp(&p, &r, "C");
    assert!(c.layers[0].props.get("timeRemap").is_none());
}
