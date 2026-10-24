//! Batch 5 checks of fields decoded before (markers with several entries and fractional
//! durations, 509 and 510).

mod common;

use common::{first_comp, fixture, open};
use effectcraft_aep::Project;

#[test]
fn two_composition_markers_in_time_order_with_label_and_duration() {
    let bytes = fixture("509-marker-comp-two.aep");
    let file = open(&bytes);
    let project = Project::new(&file).expect("project");
    let m = first_comp(&project).markers();
    let got: Vec<_> = m.iter().map(|m| (m.time_units, m.comment.as_str(), m.duration_seconds, m.label)).collect();
    assert_eq!(got, vec![(25_600, "M1", Some(0.0), Some(2)), (64_000, "M2", Some(0.5), Some(0))]);
}

#[test]
fn a_fractional_marker_time_and_duration() {
    let bytes = fixture("510-marker-layer-0.52-dur0.3.aep");
    let file = open(&bytes);
    let project = Project::new(&file).expect("project");
    let comp = first_comp(&project);
    let layer = comp.layers().next().expect("L");
    let m = layer.markers();
    assert_eq!(m.len(), 1);
    // 0.52 s = 13 312 / 25 600; 0.3 s = 180 / 600.
    assert_eq!((m[0].time_units, m[0].comment.as_str()), (13_312, "LM"));
    assert!((m[0].duration_seconds.expect("duration") - 0.3).abs() < 1e-12);
}

/// The stored value of the property at `path` (match names) of the first layer.
fn shape_value(name: &str, path: &[&str]) -> Option<Vec<f64>> {
    let bytes = fixture(name);
    let file = open(&bytes);
    let project = Project::new(&file).expect("project");
    let comp = first_comp(&project);
    let layer = comp.layers().next().expect("S");
    match layer.properties()?.find(path)? {
        effectcraft_aep::PropNode::Property(p) => p.static_value(),
        _ => None,
    }
}

const G1: [&str; 2] = ["ADBE Root Vectors Group", "ADBE Vector Group"];

#[test]
fn shape_values_are_stored_as_shown() {
    let at = |rest: &[&'static str]| -> Vec<&'static str> { G1.iter().chain(rest.iter()).copied().collect() };
    assert_eq!(
        shape_value("451-shape-ellipse-400x300-fill.aep", &at(&["ADBE Vectors Group", "ADBE Vector Shape - Ellipse", "ADBE Vector Ellipse Size"])),
        Some(vec![400.0, 300.0])
    );
    assert_eq!(
        shape_value("451-shape-ellipse-400x300-fill.aep", &at(&["ADBE Vectors Group", "ADBE Vector Graphic - Fill", "ADBE Vector Fill Color"])),
        Some(vec![255.0, 0.0, 255.0, 0.0])
    );
    // Group scale in percent, not a fraction.
    assert_eq!(shape_value("457-shape-group-scale-50-80.aep", &at(&["ADBE Vector Transform Group", "ADBE Vector Scale"])), Some(vec![50.0, 80.0]));
    assert_eq!(
        shape_value("464-shape-stroke-opacity-50.aep", &at(&["ADBE Vectors Group", "ADBE Vector Graphic - Stroke", "ADBE Vector Stroke Opacity"])),
        Some(vec![50.0])
    );
    assert_eq!(shape_value("467-shape-trim-end-70.aep", &at(&["ADBE Vectors Group", "ADBE Vector Filter - Trim", "ADBE Vector Trim End"])), Some(vec![70.0]));
    assert_eq!(shape_value("469-shape-trim-type-2.aep", &at(&["ADBE Vectors Group", "ADBE Vector Filter - Trim", "ADBE Vector Trim Type"])), Some(vec![2.0]));
    assert_eq!(
        shape_value("471-shape-rect-roundness-30.aep", &at(&["ADBE Vectors Group", "ADBE Vector Shape - Rect", "ADBE Vector Rect Roundness"])),
        Some(vec![30.0])
    );
    assert_eq!(
        shape_value("473-shape-star-points-6.aep", &at(&["ADBE Vectors Group", "ADBE Vector Shape - Star", "ADBE Vector Star Points"])),
        Some(vec![6.0])
    );
    // The shape layer's own anchor: pixels as stored.
    assert_eq!(shape_value("462-shape-layer-anchor-10-20.aep", &["ADBE Transform Group", "ADBE Anchor Point"]), Some(vec![10.0, 20.0, 0.0]));
    // A new Trim Paths and every default item of 519 have no record.
    for item in
        ["ADBE Vector Shape - Rect", "ADBE Vector Shape - Ellipse", "ADBE Vector Shape - Star", "ADBE Vector Graphic - Fill", "ADBE Vector Filter - Trim"]
    {
        let bytes = fixture("519-shape-all-items-default.aep");
        let file = open(&bytes);
        let project = Project::new(&file).expect("project");
        let comp = first_comp(&project);
        let layer = comp.layers().next().expect("S");
        let Some(effectcraft_aep::PropNode::Group(g)) = layer.properties().and_then(|p| p.find(&at(&["ADBE Vectors Group", item]))) else { panic!("{item}") };
        assert!(g.entries().iter().all(|e| !matches!(e.node, effectcraft_aep::PropNode::Property(p) if p.flags() == Some(1))), "{item} has a value record");
    }
}

/// The baseline option of the first style run of the first text layer.
fn script_of(name: &str) -> effectcraft_aep::FontBaseline {
    let bytes = fixture(name);
    let file = open(&bytes);
    let project = Project::new(&file).expect("project");
    let comp = first_comp(&project);
    let layer = comp.layers().find(|l| l.text_document().is_some()).expect("a text layer");
    let doc = layer.text_document().expect("document");
    assert!(doc.undecoded_style_fields.is_empty(), "{name}: {:?}", doc.undecoded_style_fields);
    doc.style_runs.first().expect("a run").baseline
}

#[test]
fn superscript_and_subscript_are_style_slot_13() {
    use effectcraft_aep::FontBaseline;
    // The character state the corpus was made in is superscript (526, and 243, 517).
    assert_eq!(script_of("526-text-point-HHHH-default.aep"), FontBaseline::Superscript);
    assert_eq!(script_of("243-text-multiline-A-CR-B.aep"), FontBaseline::Superscript);
    assert_eq!(script_of("527-text-point-HHHH-baseline-normal.aep"), FontBaseline::Normal);
    assert_eq!(script_of("528-text-point-HHHH-baseline-subscript.aep"), FontBaseline::Subscript);
}

#[test]
fn a_wrapping_box_text() {
    let bytes = fixture("517-text-box-600x400-wrap.aep");
    let file = open(&bytes);
    let project = Project::new(&file).expect("project");
    let comp = first_comp(&project);
    let doc = comp.layers().find_map(|l| l.text_document()).expect("text");
    let b = doc.text_box.expect("box text");
    assert_eq!((b.x, b.y, b.width, b.height), (-300.0, -200.0, 600.0, 400.0));
    assert_eq!(doc.raw_text, "The quick brown fox jumps over the lazy dog and keeps running far away\r");
    assert_eq!(doc.style_runs[0].font_size, 100.0);
}

fn layer_l<T>(name: &str, f: impl Fn(&effectcraft_aep::Layer<'_, '_>) -> T) -> T {
    let bytes = fixture(name);
    let file = open(&bytes);
    let project = Project::new(&file).expect("project");
    let comp = first_comp(&project);
    let layer = comp.layers().next().expect("the solid L, the only layer");
    f(&layer)
}

#[test]
fn orientation_is_otst_otda() {
    let o = layer_l("488-layer-3d-orientation-10-20-30.aep", |l| l.orientation()).expect("orientation");
    assert_eq!((o.degrees, o.animated, o.has_expression), ([10.0, 20.0, 30.0], false, false));
    for name in ["485-layer-3d-on.aep", "486-layer-3d-rotx-30.aep", "487-layer-3d-roty-45.aep"] {
        assert_eq!(layer_l(name, |l| l.orientation()), None, "{name}: absent at the default");
    }
}
