//! A placed Illustrator file with masks under two parented Nulls (#3099), against After Effects
//! 2026's own read-back of the committed project `tests/fixtures/ai-mask-3099.aep` (generator:
//! `tests/fixtures/tools/ai-mask-3099.py` and `ai-mask-3099.jsx`). Most of these guard After
//! Effects' values the importer already read so (the drawing of the page is tested in
//! `effectcraft-media` and the host's `vector_import` tests); the Anchor Point of the null left
//! at its default (`[0, 0]`, not the centre) was wrong before #3099.
//!
//! The four `.ai` files are hand-written one-page PDFs with distinct page boxes. After Effects
//! sized them by their ArtBox (a, d), by the MediaBox rounded up (b: 300.6 × 120.4 → 301 × 121)
//! and ignored `/Rotate 90` (c). Composition `ai-mask` (1920 × 1080): `art` (a) and `artD` (d,
//! Continuously Rasterize) are children of Null `N2`, itself a child of Null `N1`; both carry two
//! rectangular masks; `artC` (c) is on its own.

use effectcraft_aep::import::{ImportOptions, import};
use effectcraft_keyframe::Value;
use effectcraft_project::{ItemKind, Layer, Project};

use crate::import::helpers::comp;

const AI_MASK: &[u8] = include_bytes!("../../fixtures/ai-mask-3099.aep");

/// The fixture imported into an empty project (losses allowed, no footage file present).
fn load() -> (Project, effectcraft_aep::import::ImportResult) {
    let mut project = Project::default();
    let none = |_: &str| false;
    let r = import(&mut project, AI_MASK, &ImportOptions { allow_losses: true, ..ImportOptions::new(&none) }).unwrap();
    (project, r)
}

/// The first two components of the static value of property `path` of `l`.
fn vec2(l: &Layer, path: &str) -> [f64; 2] {
    let v = l.props.prop(path).unwrap_or_else(|| panic!("{}: {path}", l.name)).value.as_vec2();
    [v[0], v[1]]
}

/// `a` and `b` agree within float32 noise.
fn near(a: [f64; 2], b: [f64; 2]) -> bool {
    (a[0] - b[0]).abs() < 1e-3 && (a[1] - b[1]).abs() < 1e-3
}

/// The four `.ai` items have After Effects' sizes and are placed pages.
#[test]
fn placed_ai_sizes_are_after_effects_sizes() {
    let (p, _) = load();
    let mut sizes: Vec<(String, (u32, u32))> = p
        .items
        .values()
        .filter_map(|i| match &i.kind {
            ItemKind::Footage(f) if f.path.ends_with(".ai") => {
                // Framed and drawn as After Effects places the page (#3099).
                assert!(f.placed_page, "{}", i.name);
                Some((i.name.clone(), (f.width, f.height)))
            }
            _ => None,
        })
        .collect();
    sizes.sort();
    assert_eq!(
        sizes,
        [
            ("i3099-a-boxes.ai".to_string(), (241, 121)),
            ("i3099-b-frac.ai".to_string(), (301, 121)),
            ("i3099-c-rot.ai".to_string(), (300, 100)),
            ("i3099-d-trim.ai".to_string(), (210, 120)),
        ]
    );
}

/// Parents, transforms (the default null anchor at `[0, 0]`) and both masks of each art layer
/// as After Effects read them back.
#[test]
fn masks_anchors_and_parents_are_after_effects_values() {
    let (p, r) = load();
    let (_, c) = comp(&p, &r, "ai-mask");
    let layer = |name: &str| c.layers.iter().find(|l| l.name == name).unwrap_or_else(|| panic!("no layer {name}"));
    let (art, art_d, art_c, n1, n2) = (layer("art"), layer("artD"), layer("artC"), layer("N1"), layer("N2"));
    // Parents: art, artD -> N2 -> N1.
    assert_eq!(art.parent, Some(n2.id));
    assert_eq!(art_d.parent, Some(n2.id));
    assert_eq!(n2.parent, Some(n1.id));
    assert_eq!(n1.parent, None);
    assert_eq!(art_c.parent, None);
    // Transforms as After Effects read them back (anchor = half the source size for footage).
    for (l, anchor, position, scale) in [
        (art, [120.5, 60.5], [0.0, 0.0], [100.0, 100.0]),
        (art_d, [105.0, 60.0], [300.0, 250.0], [100.0, 100.0]),
        (art_c, [150.0, 50.0], [1500.0, 800.0], [100.0, 100.0]),
        (n1, [0.0, 0.0], [700.0, 400.0], [80.0, 120.0]),
        (n2, [10.0, 20.0], [50.0, -30.0], [150.0, 150.0]),
    ] {
        assert!(near(vec2(l, "transform/anchor"), anchor), "{} anchor {:?}", l.name, vec2(l, "transform/anchor"));
        assert!(near(vec2(l, "transform/position"), position), "{} position {:?}", l.name, vec2(l, "transform/position"));
        assert!(near(vec2(l, "transform/scale"), scale), "{} scale {:?}", l.name, vec2(l, "transform/scale"));
    }
    match n1.props.prop("transform/rotation").map(|p| &p.value) {
        Some(Value::Scalar(v)) => assert!((v - 15.0).abs() < 1e-9, "N1 rotation {v}"),
        other => panic!("N1 rotation {other:?}"),
    }
    // Both art layers: M1 (40, 30)-(200, 90) and M2 (220, 100)-(350, 160) in layer pixels, the
    // second reaching past the layer, as After Effects read the vertices back.
    let rects = [[[40.0, 30.0], [200.0, 30.0], [200.0, 90.0], [40.0, 90.0]], [[220.0, 100.0], [350.0, 100.0], [350.0, 160.0], [220.0, 160.0]]];
    for l in [art, art_d] {
        let masks: Vec<_> = l.masks().expect("masks").groups().collect();
        assert_eq!(masks.len(), 2, "{}", l.name);
        for (m, want) in masks.iter().zip(&rects) {
            let Some(Value::Path(path)) = m.get("path").map(|p| &p.value) else { panic!("{}: {} has no path", l.name, m.name) };
            assert_eq!(path.vertices.len(), 4, "{}: {}", l.name, m.name);
            for (v, w) in path.vertices.iter().zip(want) {
                assert!(near(*v, *w), "{}: {} vertex {v:?} != {w:?}", l.name, m.name);
            }
        }
    }
}
