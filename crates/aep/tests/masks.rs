//! Masks: mode, invert switch, feather / opacity / expansion, and the path shapes, checked against
//! the values the corpus was made with (its MANIFEST). The solid `L` is 1920x1080.

mod common;

use common::{first_comp, fixture, layer_with_id, open};
use effectcraft_aep::{KeyframeKind, Mask, MaskMode, PathVertex, Project, PropNode, Property, ShapePath, ValueSource};

const L: u32 = 0x1b;
const SIZE: (f64, f64) = (1920.0, 1080.0);

fn masks(name: &str) -> Vec<Mask> {
    let bytes = fixture(name);
    let file = open(&bytes);
    let project = Project::new(&file).unwrap();
    layer_with_id(&first_comp(&project), L).masks()
}

fn assert_points(vertices: &[PathVertex], expected: &[[f64; 2]]) {
    assert_eq!(vertices.len(), expected.len());
    for (v, e) in vertices.iter().zip(expected) {
        for (got, want) in v.position.iter().zip(e) {
            assert!((got - want).abs() < 1e-3, "{:?} != {e:?}", v.position);
        }
        // No tangents: both handles sit on the vertex.
        assert_eq!(v.in_handle, v.position);
        assert_eq!(v.out_handle, v.position);
    }
}

#[test]
fn no_masks_in_the_base() {
    assert!(masks("000-base.aep").is_empty());
}

#[test]
fn rectangular_mask_with_defaults() {
    // 090: rect size [800,400], centre [960,540]: "Mask 1", vertices [560,340] [1360,340] [1360,740] [560,740], mode Add.
    let m = masks("090-mask-rect.aep");
    assert_eq!(m.len(), 1);
    let m = &m[0];
    assert_eq!(m.name.as_deref(), Some("Mask 1"));
    assert_eq!((m.mode, m.inverted), (Some(MaskMode::Add), Some(false)));
    // Feather, opacity and expansion are absent at their defaults.
    assert_eq!((m.feather.value, m.feather.source), ([0.0, 0.0], ValueSource::Default));
    assert_eq!((m.opacity_percent.value, m.opacity_percent.source), (100.0, ValueSource::Default));
    assert_eq!((m.expansion.value, m.expansion.source), (0.0, ValueSource::Default));
    assert!(!m.path_animated);
    assert!(m.path_key_seconds.is_empty());
    assert_eq!(m.shapes.len(), 1);
    let s = &m.shapes[0];
    // The box is normalised to the layer size: 560/1920, 340/1080, 1360/1920, 740/1080.
    let want_box = [560.0 / 1920.0, 340.0 / 1080.0, 1360.0 / 1920.0, 740.0 / 1080.0];
    for (got, want) in s.bbox.iter().zip(want_box) {
        assert!((f64::from(*got) - want).abs() < 1e-6, "{:?}", s.bbox);
    }
    assert_eq!(s.closed_flag, 1);
    assert_eq!(s.vertex_count(), 4);
    // Three points per vertex, [vertex, out handle, next in handle], inside the box.
    let rect: [[f32; 2]; 12] =
        [[0.0, 0.0], [0.0, 0.0], [1.0, 0.0], [1.0, 0.0], [1.0, 0.0], [1.0, 1.0], [1.0, 1.0], [1.0, 1.0], [0.0, 1.0], [0.0, 1.0], [0.0, 1.0], [0.0, 0.0]];
    assert_eq!(s.points, rect);
    assert_points(&s.vertices(SIZE), &[[560.0, 340.0], [1360.0, 340.0], [1360.0, 740.0], [560.0, 740.0]]);
}

#[test]
fn mask_properties_mode_and_invert() {
    // 091: feather [0,0] -> [20,20], opacity 100 -> 50, mode Add -> Subtract, inverted, expansion 0 -> 5.
    let m = masks("091-mask-rect-props.aep");
    let m = &m[0];
    assert_eq!((m.mode, m.inverted), (Some(MaskMode::Subtract), Some(true)));
    assert_eq!((m.feather.value, m.feather.source), ([20.0, 20.0], ValueSource::Stored));
    assert_eq!((m.opacity_percent.value, m.opacity_percent.source), (50.0, ValueSource::Stored));
    assert_eq!((m.expansion.value, m.expansion.source), (5.0, ValueSource::Stored));
    // The path is unchanged from 090.
    assert_points(&m.shapes[0].vertices(SIZE), &[[560.0, 340.0], [1360.0, 340.0], [1360.0, 740.0], [560.0, 740.0]]);
}

#[test]
fn animated_mask_path_has_one_shape_per_key() {
    // 093: path keys at t=0 (rect 400x200: [760,440] [1160,440] [1160,640] [760,640]) and t=1 (rect 800x400).
    let m = masks("093-mask-path-2keys.aep");
    let m = &m[0];
    assert!(m.path_animated);
    assert_eq!(m.path_key_seconds, [0.0, 1.0]);
    // The path property keeps its keys in a list of 64-byte records (the shapes are in `shap`).
    let bytes = fixture("093-mask-path-2keys.aep");
    let file = open(&bytes);
    let project = Project::new(&file).unwrap();
    let layer = layer_with_id(&first_comp(&project), L);
    let Some(PropNode::Group(parade)) = layer.properties().unwrap().get("ADBE Mask Parade") else { panic!("mask parade") };
    let Some(PropNode::Group(atom)) = parade.get("ADBE Mask Atom") else { panic!("mask atom") };
    let Some(PropNode::Other(path)) = atom.get("ADBE Mask Shape") else { panic!("mask shape") };
    let prop = Property::from_chunk(path.child_list(b"tdbs").unwrap()).unwrap();
    let list = prop.keys().unwrap();
    assert_eq!((list.len(), list.header.record_size), (2, 64));
    assert!(list.keyframes().all(|k| k.kind() == KeyframeKind::MaskPath));
    assert_eq!(m.shapes.len(), 2);
    assert_points(&m.shapes[0].vertices(SIZE), &[[760.0, 440.0], [1160.0, 440.0], [1160.0, 640.0], [760.0, 640.0]]);
    assert_points(&m.shapes[1].vertices(SIZE), &[[560.0, 340.0], [1360.0, 340.0], [1360.0, 740.0], [560.0, 740.0]]);
}

#[test]
fn shape_path_maths_with_a_pixel_box_and_tangents() {
    // The documented point order [vertex i, out handle i, in handle i + 1] with a box in pixels (the
    // shape-layer convention): a triangle [0,-100] [100,100] [-100,100] in the box (-100,-100,100,100),
    // the first vertex with an out handle at [50,-100] and the last record the in handle of vertex 0.
    let p = ShapePath {
        bbox: [-100.0, -100.0, 100.0, 100.0],
        closed_flag: 1,
        open: false,
        points: vec![
            [0.5, 0.0],
            [0.75, 0.0],
            [1.0, 1.0], // vertex 0, out handle 0, in handle 1
            [1.0, 1.0],
            [1.0, 1.0],
            [0.0, 1.0], // vertex 1, out handle 1, in handle 2
            [0.0, 1.0],
            [0.0, 1.0],
            [0.25, 0.0], // vertex 2, out handle 2, in handle 0
        ],
    };
    let v = p.vertices((1.0, 1.0));
    assert_eq!(v.len(), 3);
    assert_eq!(v[0].position, [0.0, -100.0]);
    assert_eq!(v[0].out_handle, [50.0, -100.0]);
    assert_eq!(v[0].in_handle, [-50.0, -100.0]);
    assert_eq!(v[1].position, [100.0, 100.0]);
    assert_eq!(v[1].in_handle, [100.0, 100.0]);
    assert_eq!(v[2].position, [-100.0, 100.0]);
    assert_eq!(v[2].out_handle, [-100.0, 100.0]);
    assert!(ShapePath { bbox: [0.0; 4], closed_flag: 0, open: false, points: Vec::new() }.vertices((1.0, 1.0)).is_empty());
}

// ---------------------------------------------------------------------------- batch 2

fn near2(got: [f64; 2], want: [f64; 2], tol: f64) {
    assert!((got[0] - want[0]).abs() < tol && (got[1] - want[1]).abs() < tol, "{got:?} != {want:?}");
}

#[test]
fn mask_mode_ids() {
    // 090 Add 1, 091 Subtract 2, 234 Intersect 3, 235 Lighten 4, 236 Darken 5, 237 Difference 6.
    for (name, mode, id) in [
        ("090-mask-rect.aep", MaskMode::Add, 1),
        ("091-mask-rect-props.aep", MaskMode::Subtract, 2),
        ("234-mask-mode-intersect.aep", MaskMode::Intersect, 3),
        ("235-mask-mode-lighten.aep", MaskMode::Lighten, 4),
        ("236-mask-mode-darken.aep", MaskMode::Darken, 5),
        ("237-mask-mode-difference.aep", MaskMode::Difference, 6),
    ] {
        let m = masks(name);
        assert_eq!(m.len(), 1, "{name}");
        assert_eq!(m[0].mode, Some(mode), "{name}");
        assert_eq!(MaskMode::from_id(id), mode);
        assert_eq!(m[0].name.as_deref(), Some("Mask 1"));
        assert_eq!(m[0].shapes.len(), 1);
        assert!(!m[0].shapes[0].open);
    }
    assert_eq!(MaskMode::from_id(0), MaskMode::None);
    assert_eq!(MaskMode::from_id(9), MaskMode::Other(9));
}

#[test]
fn ellipse_has_tangents_of_0_5523_times_the_radius() {
    // 232: ellipse 800x400 at [960,540]: vertices [960,340] [1360,540] [960,740] [560,540]; in tangents
    // [-220.91,0] [0,-110.457] [220.91,0] [0,110.457]; out tangents the opposite ones; closed.
    let m = masks("232-mask-ellipse.aep");
    let shape = &m[0].shapes[0];
    assert!(!shape.open);
    assert_eq!(shape.vertex_count(), 4);
    let v = shape.vertices(SIZE);
    let vertices = [[960.0, 340.0], [1360.0, 540.0], [960.0, 740.0], [560.0, 540.0]];
    let in_tangents = [[-220.913_879, 0.0], [0.0, -110.456_97], [220.913_94, 0.0], [0.0, 110.456_97]];
    let out_tangents = [[220.913_94, 0.0], [0.0, 110.456_97], [-220.913_879, 0.0], [0.0, -110.456_97]];
    for i in 0..4 {
        near2(v[i].position, vertices[i], 1e-3);
        near2(v[i].in_tangent(), in_tangents[i], 1e-2);
        near2(v[i].out_tangent(), out_tangents[i], 1e-2);
        // Tangent = handle minus vertex.
        near2(v[i].in_handle, [v[i].position[0] + v[i].in_tangent()[0], v[i].position[1] + v[i].in_tangent()[1]], 1e-9);
    }
    // The box is the vertex box 560,340,1360,740 (normalised to the 1920x1080 layer).
    let [l, t, r, b] = shape.bbox.map(f64::from);
    near2([l * SIZE.0, t * SIZE.1], [560.0, 340.0], 1e-2);
    near2([r * SIZE.0, b * SIZE.1], [1360.0, 740.0], 1e-2);
}

#[test]
fn open_path_has_bit_8_in_the_shph_flags() {
    // 233: three vertices [500,300] [960,700] [1400,300], closed=false, no tangents.
    let m = masks("233-mask-open-path.aep");
    let shape = &m[0].shapes[0];
    assert!(shape.open);
    assert_eq!(shape.closed_flag, 1);
    assert_eq!(shape.vertex_count(), 3);
    assert_points(&shape.vertices(SIZE), &[[500.0, 300.0], [960.0, 700.0], [1400.0, 300.0]]);
    assert_eq!(m[0].mode, Some(MaskMode::Add));
    // The closed paths of the same session have the bit clear.
    for name in ["232-mask-ellipse.aep", "234-mask-mode-intersect.aep"] {
        assert!(!masks(name)[0].shapes[0].open, "{name}");
    }
}
