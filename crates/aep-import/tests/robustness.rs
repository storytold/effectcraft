//! Damaged and hostile files produce an error or a report, never a panic.

mod common;

use common::fixture_bytes;
use effectcraft_aep::{AepFile, Body, Chunk, parse, write};
use effectcraft_aep_import::LossCategory;
use effectcraft_aep_import::{AepImportError, ImportOptions, import};
use effectcraft_keyframe::Value;
use effectcraft_project::{ItemKind, Project};

fn fixtures() -> Vec<String> {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../aep/tests/fixtures");
    let mut names: Vec<String> =
        std::fs::read_dir(dir).unwrap().filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().to_string()).filter(|n| n.ends_with(".aep")).collect();
    names.sort();
    assert!(names.len() > 100, "{} fixtures", names.len());
    names
}

/// Imports with and without allowing losses; both must return, and a result must be sane.
fn try_all(bytes: &[u8]) -> bool {
    let none = |_: &str| false;
    let mut ok = false;
    for allow in [false, true] {
        let mut project = Project::default();
        let opts = ImportOptions { allow_losses: allow, ..ImportOptions::new(&none) };
        match import(&mut project, bytes, &opts) {
            Ok(r) => {
                ok = true;
                check_sane(&project, r.comps.len());
            }
            Err(AepImportError::Refused(report)) => assert!(!report.is_lossless()),
            Err(_) => assert!(project.items.is_empty()),
        }
    }
    ok
}

/// What a consumer relies on: the project serialises, keyframes are in time order, parents exist.
fn check_sane(project: &Project, comps: usize) {
    serde_json::to_string(project).expect("a project that serialises");
    let mut seen = 0;
    for item in project.items.values() {
        if let ItemKind::Comp(c) = &item.kind {
            seen += 1;
            for l in &c.layers {
                if let Some(parent) = l.parent {
                    assert!(c.layer(parent).is_some());
                    assert_ne!(parent, l.id);
                }
                assert!(l.in_point <= l.out_point);
                // Effect parameters: finite values and keys in time order.
                for fx in l.props.sub("effects").into_iter().flat_map(|g| g.groups()) {
                    for p in fx.props() {
                        assert!(p.value.components().iter().all(|v| v.is_finite()), "{}: {:?}", p.match_id, p.value);
                        assert!(p.keys.windows(2).all(|w| w[0].time < w[1].time));
                        assert!(p.keys.iter().all(|k| k.value.components().iter().all(|v| v.is_finite())));
                    }
                }
                for path in ["transform/anchor", "transform/position", "transform/scale", "transform/rotation", "transform/opacity"] {
                    if let Some(p) = l.props.prop(path) {
                        assert!(p.keys.windows(2).all(|w| w[0].time < w[1].time));
                        assert!(p.value.components().iter().all(|v| v.is_finite()), "{path}: {:?}", p.value);
                        for k in &p.keys {
                            assert!(k.value.components().iter().all(|v| v.is_finite()));
                        }
                    }
                }
                check_masks(l);
                if let Some(Value::Text(t)) = l.props.prop("text/sourceText").map(|p| &p.value) {
                    // Runs cover the text; sizes, colours, leading and the box are usable.
                    assert_eq!(t.runs().iter().map(|r| r.len).sum::<usize>(), t.char_len());
                    assert!(t.runs().iter().all(|r| r.style.size.is_finite() && r.style.size > 0.0 && r.style.fill.iter().all(|c| c.is_finite())));
                    assert!(t.leading.is_none_or(f64::is_finite) && t.tracking.is_finite());
                    assert!(t.box_size.is_none_or(|b| b.iter().all(|v| v.is_finite() && *v >= 1.0)));
                }
            }
        }
    }
    assert!(seen >= comps);
}

/// Imported masks: finite paths with matching tangent lists, sorted keys, valid feather and opacity.
fn check_masks(l: &effectcraft_project::Layer) {
    let Some(masks) = l.masks() else { return };
    let path_ok = |v: &Value| match v {
        Value::Path(p) => {
            !p.vertices.is_empty()
                && p.in_tangents.len() == p.vertices.len()
                && p.out_tangents.len() == p.vertices.len()
                && p.vertices.iter().chain(&p.in_tangents).chain(&p.out_tangents).flatten().all(|c| c.is_finite())
        }
        _ => false,
    };
    for g in masks.groups() {
        assert!(matches!(g.kind, effectcraft_project::GroupKind::Mask { .. }));
        for id in ["path", "feather", "opacity", "expansion"] {
            let p = g.get(id).expect("mask property");
            assert!(p.keys.windows(2).all(|w| w[0].time < w[1].time), "{id}");
            for v in std::iter::once(&p.value).chain(p.keys.iter().map(|k| &k.value)) {
                match id {
                    "path" => assert!(path_ok(v), "{v:?}"),
                    "feather" => assert!(v.components().iter().all(|c| c.is_finite() && *c >= 0.0), "{v:?}"),
                    "opacity" => assert!(v.components().iter().all(|c| (0.0..=100.0).contains(c)), "{v:?}"),
                    _ => assert!(v.components().iter().all(|c| c.is_finite()), "{v:?}"),
                }
            }
        }
    }
}

#[test]
fn every_fixture_imports_or_reports() {
    for name in fixtures() {
        assert!(try_all(&fixture_bytes(&name)), "{name}");
    }
}

#[test]
fn truncated_files_never_panic() {
    for name in fixtures() {
        let bytes = fixture_bytes(&name);
        let n = bytes.len();
        for cut in (0..n).step_by((n / 40).max(1)).chain([n.saturating_sub(1), n.saturating_sub(2), 11, 12, 13, 100]) {
            try_all(&bytes[..cut.min(n)]);
        }
    }
}

/// A small deterministic generator (xorshift) for the byte flips.
fn next(state: &mut u64) -> u64 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    *state
}

#[test]
fn flipped_bytes_never_panic() {
    let mut state = 0x9e37_79b9_7f4a_7c15u64;
    for name in fixtures() {
        let original = fixture_bytes(&name);
        for round in 0..40 {
            let mut bytes = original.clone();
            let flips = 1 + round % 6;
            for _ in 0..flips {
                let at = (next(&mut state) % bytes.len() as u64) as usize;
                bytes[at] ^= 1 << (next(&mut state) % 8);
            }
            try_all(&bytes);
        }
        // Runs of 0xff / 0x00 over parts of the file, the classic size-field killers.
        for round in 0..12 {
            let mut bytes = original.clone();
            let at = (next(&mut state) % bytes.len() as u64) as usize;
            let len = 1 + (next(&mut state) % 64) as usize;
            let fill = if round % 2 == 0 { 0xff } else { 0x00 };
            for b in bytes.iter_mut().skip(at).take(len) {
                *b = fill;
            }
            try_all(&bytes);
        }
    }
}

fn visit(chunks: &mut [Chunk<'_>], f: &mut dyn FnMut(&mut Chunk<'_>)) {
    for c in chunks {
        f(c);
        visit(c.children_mut(), f);
    }
}

fn set(chunk: &mut Chunk<'_>, at: usize, bytes: &[u8]) {
    if let Body::Data(d) = &mut chunk.body {
        let d = d.to_mut();
        for (i, b) in bytes.iter().enumerate() {
            if let Some(slot) = d.get_mut(at + i) {
                *slot = *b;
            }
        }
    }
}

/// Rewrites a fixture with `edit` applied to every chunk.
fn mutate(name: &str, edit: &mut dyn FnMut(&mut Chunk<'_>)) -> Vec<u8> {
    let bytes = fixture_bytes(name);
    let mut file: AepFile<'_> = parse(&bytes).unwrap();
    visit(&mut file.chunks, edit);
    write(&file).unwrap()
}

const NAN: [u8; 8] = [0x7f, 0xf8, 0, 0, 0, 0, 0, 1];
const INF: [u8; 8] = [0x7f, 0xf0, 0, 0, 0, 0, 0, 0];
const HUGE: [u8; 8] = [0x7f, 0xef, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff];

#[test]
fn hostile_values_in_valid_structure() {
    let keyed =
        ["055-key-position-3key-inserted-t2.5.aep", "204-key-ease-asymmetric.aep", "205-key-opacity-ease-asymmetric.aep", "206-key-scale-2key-linear.aep"];
    let others = [
        "000-base.aep",
        "022-layer-parent-null.aep",
        "124-precomp-Pre.aep",
        "126-footage-png-layer.aep",
        "212-anchor-400x300-solid-100-50.aep",
        "091-mask-rect-props.aep",
        "093-mask-path-2keys.aep",
        "208-key-mask-feather-2key-linear.aep",
        "306-key-maskfeather-bezier-ease.aep",
        "328-mask-two-ellipse-rect.aep",
    ];
    for name in keyed.iter().chain(&others) {
        // Composition header: zero and extreme sizes, time base, durations.
        for (at, value) in [(4usize, 0u32), (8, 0), (4, u32::MAX), (8, u32::MAX), (44, u32::MAX), (28, u32::MAX), (36, 5), (172, u32::MAX)] {
            let bytes = mutate(name, &mut |c| {
                if c.is(b"cdta") {
                    set(c, at, &value.to_be_bytes());
                }
            });
            try_all(&bytes);
        }
        for (at, bytes2) in [(140usize, [0u8, 0]), (142, [0, 0]), (140, [0xff, 0xff]), (142, [0xff, 0xff]), (144, [0, 0])] {
            let bytes = mutate(name, &mut |c| {
                if c.is(b"cdta") {
                    set(c, at, &bytes2);
                }
            });
            try_all(&bytes);
        }
        // Layer header: stretch, times, source and parent links.
        for (at, value) in [
            (8usize, 0u32),
            (8, u32::MAX),
            (12, 0x8000_0000),
            (16, 0),
            (20, 0x7fff_ffff),
            (24, 0),
            (28, 0x8000_0000),
            (32, 0),
            (40, 0),
            (40, 27),
            (132, 27),
            (132, 29),
            (0, 0),
            (0, 29),
            (132, u32::MAX),
        ] {
            let bytes = mutate(name, &mut |c| {
                if c.is(b"ldta") {
                    set(c, at, &value.to_be_bytes());
                }
            });
            try_all(&bytes);
        }
        // Every double of every property: NaN, infinity, huge.
        for bad in [NAN, INF, HUGE] {
            for chunk_id in [b"cdat", b"ldat"] {
                let bytes = mutate(name, &mut |c| {
                    if c.is(chunk_id) {
                        let len = c.bytes().map_or(0, <[u8]>::len);
                        for at in (0..len).step_by(8) {
                            set(c, at, &bad);
                        }
                    }
                });
                try_all(&bytes);
            }
        }
        // Keyframe times and flags, interpolation bytes.
        for fill in [0xffu8, 0x7f, 0x03, 0x00] {
            let bytes = mutate(name, &mut |c| {
                if c.is(b"ldat") {
                    let len = c.bytes().map_or(0, <[u8]>::len);
                    for at in (0..len).step_by(8) {
                        set(c, at, &[fill; 4]);
                    }
                }
            });
            try_all(&bytes);
        }
        // Property headers (component counts, templates, time scale, expression bytes).
        for (at, value) in [(3usize, 0u8), (3, 255), (7, 0), (7, 1), (12, 0), (68, 1), (76, 1), (120, 1), (119, 1)] {
            let bytes = mutate(name, &mut |c| {
                if c.is(b"tdb4") {
                    set(c, at, &[value]);
                }
            });
            try_all(&bytes);
        }
        // Footage headers.
        for (at, value) in [(30usize, u32::MAX), (34, u32::MAX), (30, 0), (34, 0)] {
            let bytes = mutate(name, &mut |c| {
                if c.is(b"sspc") {
                    set(c, at, &value.to_be_bytes());
                }
            });
            try_all(&bytes);
        }
    }
}

/// Regression: a layer whose parent is itself, or a pair of layers that are each other's
/// parent, must not produce a parent loop in the project.
#[test]
fn parent_loops_are_dropped() {
    // 022: the child (id 27) has parent 29 (the null). Make the null's parent the child, and the
    // child its own parent.
    let looped = mutate("022-layer-parent-null.aep", &mut |c| {
        if c.is(b"ldta") {
            let id = c.bytes().and_then(|b| b.get(..4)).map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]));
            match id {
                Some(29) => set(c, 132, &27u32.to_be_bytes()),
                Some(27) => set(c, 132, &29u32.to_be_bytes()),
                _ => {}
            }
        }
    });
    let mut p = Project::default();
    let none = |_: &str| false;
    let opts = ImportOptions { allow_losses: true, ..ImportOptions::new(&none) };
    let r = import(&mut p, &looped, &opts).unwrap();
    assert_eq!(r.report.count(effectcraft_aep_import::LossCategory::ParentLinks), 2);
    let c = p.comp(r.comps[0]).unwrap();
    assert!(c.layers.iter().all(|l| l.parent.is_none()));

    let selfish = mutate("022-layer-parent-null.aep", &mut |c| {
        if c.is(b"ldta") && c.bytes().and_then(|b| b.get(..4)) == Some(&29u32.to_be_bytes()[..]) {
            set(c, 132, &29u32.to_be_bytes());
        }
    });
    let mut p = Project::default();
    let r = import(&mut p, &selfish, &opts).unwrap();
    assert_eq!(r.report.count(effectcraft_aep_import::LossCategory::ParentLinks), 1);
}

/// Regression: a precomposition that contains itself (or two comps containing each other) is cut
/// at the loop instead of building a cyclic project.
#[test]
fn precomposition_loops_are_cut() {
    // 124: comp C holds a layer of comp Pre (item id 37), and Pre holds the solid (id 26). Point
    // the layer in Pre at Pre itself, and the layer in C at C.
    let looped = mutate("124-precomp-Pre.aep", &mut |c| {
        if c.is(b"ldta") {
            let id = c.bytes().and_then(|b| b.get(..4)).map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]));
            if id == Some(39) {
                set(c, 40, &37u32.to_be_bytes());
            }
        }
    });
    let mut p = Project::default();
    let none = |_: &str| false;
    let opts = ImportOptions { allow_losses: true, ..ImportOptions::new(&none) };
    let r = import(&mut p, &looped, &opts).unwrap();
    assert_eq!(r.report.count(effectcraft_aep_import::LossCategory::UnsupportedLayers), 1, "{:?}", r.report.losses);
    // No composition uses itself, directly or through another.
    for id in &r.comps {
        let mut stack = vec![*id];
        let mut seen = std::collections::HashSet::new();
        while let Some(c) = stack.pop() {
            for l in &p.comp(c).unwrap().layers {
                if let effectcraft_project::LayerSource::Comp { item } = l.source {
                    assert_ne!(item, *id);
                    if seen.insert(item) {
                        stack.push(item);
                    }
                }
            }
        }
    }
}

/// Regression: a NaN or infinite static value used to reach the project as such.
#[test]
fn non_finite_static_values_fall_back_to_the_default() {
    let bytes = mutate("040-static-position-100-200.aep", &mut |c| {
        if c.is(b"cdat") {
            set(c, 0, &NAN);
            set(c, 8, &INF);
            set(c, 16, &HUGE);
        }
    });
    let mut p = Project::default();
    let none = |_: &str| false;
    let opts = ImportOptions { allow_losses: true, ..ImportOptions::new(&none) };
    let r = import(&mut p, &bytes, &opts).unwrap();
    assert!(r.report.count(effectcraft_aep_import::LossCategory::DamagedData) >= 1, "{:?}", r.report.losses);
    let layer = &p.comp(r.comps[0]).unwrap().layers[0];
    let pos = layer.props.prop("transform/position").unwrap();
    assert_eq!(pos.value, effectcraft_keyframe::Value::Vec3([960.0, 540.0, 0.0]));
}

fn import_allowing(bytes: &[u8]) -> (Project, effectcraft_aep_import::ImportResult) {
    let mut p = Project::default();
    let none = |_: &str| false;
    let opts = ImportOptions { allow_losses: true, ..ImportOptions::new(&none) };
    let r = import(&mut p, bytes, &opts).expect("import");
    check_sane(&p, r.comps.len());
    (p, r)
}

fn mask_count(p: &Project, r: &effectcraft_aep_import::ImportResult) -> usize {
    p.comp(r.comps[0]).unwrap().layers.iter().map(|l| l.masks().map_or(0, |m| m.groups().count())).sum()
}

/// Rewrites the point list of every mask path shape to `records` points of `point`.
fn with_points(name: &str, records: usize, point: [u8; 8]) -> Vec<u8> {
    mutate(name, &mut |c| {
        if c.is_list(b"shap") {
            for k in c.children_mut() {
                if k.is_list(b"list") {
                    for d in k.children_mut() {
                        if d.is(b"lhd3") {
                            set(d, 8, &u32::try_from(records).unwrap().to_be_bytes());
                        } else if d.is(b"ldat") {
                            d.body = Body::Data(std::borrow::Cow::Owned(point.repeat(records)));
                        }
                    }
                }
            }
        }
    })
}

/// Hostile mask paths: no vertices, a point count that is not a multiple of three, too many
/// vertices, NaN points and boxes, a malformed shape header. Each gives a report and the mask is
/// left out; the layer and the project stay valid.
#[test]
fn hostile_mask_paths_are_reported() {
    let half = 0.5f32.to_be_bytes();
    let mid = [half[0], half[1], half[2], half[3], half[0], half[1], half[2], half[3]];
    let nan = f32::NAN.to_be_bytes();
    let nan_point = [nan[0], nan[1], nan[2], nan[3], nan[0], nan[1], nan[2], nan[3]];
    let cases: [(usize, [u8; 8]); 5] = [(0, mid), (10, mid), (3 * 65_537, mid), (12, nan_point), (3, mid)];
    for (records, point) in cases {
        let bytes = with_points("090-mask-rect.aep", records, point);
        let (p, r) = import_allowing(&bytes);
        let lost = r.report.count(LossCategory::Masks);
        let kept = mask_count(&p, &r);
        assert_eq!(lost + kept, 1, "{records}: {:?}", r.report.losses);
        // Only the valid single-vertex path (3 records) survives.
        assert_eq!(kept, usize::from(records == 3), "{records}");
        assert_eq!(p.comp(r.comps[0]).unwrap().layers.len(), 1);
    }
    // NaN, infinity and huge numbers in the box.
    for bad in [f32::NAN, f32::INFINITY, f32::MAX] {
        let bytes = mutate("232-mask-ellipse.aep", &mut |c| {
            if c.is(b"shph") {
                for at in [4, 8, 12, 16] {
                    set(c, at, &bad.to_be_bytes());
                }
            }
        });
        let (p, r) = import_allowing(&bytes);
        assert_eq!(r.report.count(LossCategory::Masks) + mask_count(&p, &r), 1, "{bad}");
    }
    // A malformed shape header (too short).
    let bytes = mutate("090-mask-rect.aep", &mut |c| {
        if c.is(b"shph") {
            c.body = Body::Data(std::borrow::Cow::Owned(vec![0u8; 3]));
        }
    });
    let (p, r) = import_allowing(&bytes);
    assert_eq!((r.report.count(LossCategory::Masks), mask_count(&p, &r)), (1, 0));
}

/// An unknown mode is reported and becomes Add; a missing `mkif` body is damaged data.
#[test]
fn mask_modes_and_switches_from_hostile_mkif() {
    let bytes = mutate("234-mask-mode-intersect.aep", &mut |c| {
        if c.is(b"mkif") {
            set(c, 4, &99u32.to_be_bytes());
        }
    });
    let (p, r) = import_allowing(&bytes);
    assert_eq!(r.report.count(LossCategory::Masks), 1);
    let l = &p.comp(r.comps[0]).unwrap().layers[0];
    let g = l.masks().unwrap().groups().next().unwrap().clone();
    assert!(matches!(g.kind, effectcraft_project::GroupKind::Mask { mode: effectcraft_project::MaskMode::Add, .. }));

    let bytes = mutate("091-mask-rect-props.aep", &mut |c| {
        if c.is(b"mkif") {
            c.body = Body::Data(std::borrow::Cow::Owned(Vec::new()));
        }
    });
    let (p, r) = import_allowing(&bytes);
    assert_eq!(r.report.count(LossCategory::DamagedData), 1, "{:?}", r.report.losses);
    assert_eq!(mask_count(&p, &r), 1);
}

/// Path keys whose count does not match the stored shapes: the first shape is imported as a
/// static path and the mismatch is reported.
#[test]
fn path_keys_without_matching_shapes_fall_back_to_a_static_path() {
    let original = fixture_bytes("093-mask-path-2keys.aep");
    let mut file: AepFile<'_> = parse(&original).unwrap();
    visit(&mut file.chunks, &mut |c| {
        if c.is_list(b"omks")
            && let Body::List { children, .. } = &mut c.body
            && let Some(last) = children.iter().rposition(|k| k.is_list(b"shap"))
        {
            children.remove(last);
        }
    });
    let bytes = write(&file).unwrap();
    let (p, r) = import_allowing(&bytes);
    assert_eq!(r.report.count(LossCategory::DamagedData), 1, "{:?}", r.report.losses);
    let l = &p.comp(r.comps[0]).unwrap().layers[0];
    let g = l.masks().unwrap().groups().next().unwrap().clone();
    assert!(g.get("path").unwrap().keys.is_empty());
}

/// The effect fixtures: Tint, Gradient Ramp and Exposure, static and keyed, and a second instance.
const EFFECT_FIXTURES: [&str; 10] = [
    "356-fx-tint-default.aep",
    "363-fx-tint-amount-2key.aep",
    "364-fx-tint-white-2key.aep",
    "366-fx-blur-then-tint.aep",
    "377-fx-ramp-shape-radial.aep",
    "382-fx-ramp-start-2key.aep",
    "383-fx-ramp-on-100x100-start-10-20.aep",
    "395-fx-exposure-individual-red-1.aep",
    "396-fx-exposure-bypass-on.aep",
    "397-fx-exposure-master-2key.aep",
];

fn report_of(bytes: &[u8]) -> effectcraft_aep_import::ImportReport {
    effectcraft_aep_import::analyze(bytes, &|_| false).expect("a readable project")
}

#[test]
fn hostile_effect_parameter_records() {
    for name in EFFECT_FIXTURES {
        // Every parameter type in every pard, including unknown ones.
        for kind in [0u32, 2, 3, 4, 5, 6, 7, 9, 10, 12, 13, 14, 15, 16, u32::MAX] {
            let bytes = mutate(name, &mut |c| {
                if c.is(b"pard") {
                    set(c, 12, &kind.to_be_bytes());
                }
            });
            assert!(try_all(&bytes), "{name}: type {kind}");
        }
        // Defaults, popup counts, valid ranges and slider ranges: zero, negative, huge, NaN.
        for at in [56usize, 60, 62, 104, 108, 112, 116, 128] {
            for value in [0u32, 0x8000_0000, u32::MAX, 0x7fff_ffff, 0x7fc0_0000] {
                let bytes = mutate(name, &mut |c| {
                    if c.is(b"pard") {
                        set(c, at, &value.to_be_bytes());
                    }
                });
                assert!(try_all(&bytes), "{name}: pard@{at} = {value:#x}");
            }
        }
        // Values: non-finite, huge, and out-of-range popup indices.
        for bad in [NAN, INF, HUGE, 99.0f64.to_be_bytes(), (-3.0f64).to_be_bytes(), 0.0f64.to_be_bytes()] {
            for chunk_id in [b"cdat", b"ldat"] {
                let bytes = mutate(name, &mut |c| {
                    if c.is(chunk_id) {
                        let len = c.bytes().map_or(0, <[u8]>::len);
                        for at in (0..len).step_by(8) {
                            set(c, at, &bad);
                        }
                    }
                });
                assert!(try_all(&bytes), "{name}");
            }
        }
        // Truncated and empty pard bodies.
        for len in [0usize, 3, 13, 57, 129] {
            let bytes = mutate(name, &mut |c| {
                if c.is(b"pard")
                    && let Body::Data(d) = &mut c.body
                {
                    d.to_mut().truncate(len);
                }
            });
            assert!(try_all(&bytes), "{name}: pard cut to {len}");
        }
        // Component counts and templates of the value records.
        for (at, value) in [(3usize, 0u8), (3, 1), (3, 4), (3, 255), (7, 0), (7, 3)] {
            let bytes = mutate(name, &mut |c| {
                if c.is(b"tdb4") {
                    set(c, at, &[value]);
                }
            });
            assert!(try_all(&bytes), "{name}: tdb4@{at} = {value}");
        }
    }
}

#[test]
fn bogus_effect_structure_is_reported_not_imported_blindly() {
    // An unknown effect match name: reported by name.
    let bytes = mutate("356-fx-tint-default.aep", &mut |c| {
        if c.is(b"tdmn") && c.bytes().is_some_and(|b| b.starts_with(b"ADBE Tint\0")) {
            set(c, 0, b"XYZ Unknown\0");
        }
    });
    let r = report_of(&bytes);
    assert_eq!(r.imported.effects, 0);
    assert!(
        r.losses.iter().any(|l| l.category == effectcraft_aep_import::LossCategory::Effects && l.details.iter().any(|d| d.contains("XYZ Unknown"))),
        "{r:?}"
    );

    // No project-level definitions: the second Ramp (empty parT) cannot be read and is reported;
    // the first still arrives.
    let bytes = mutate("383-fx-ramp-on-100x100-start-10-20.aep", &mut |c| {
        if let Body::List { list_type, .. } = &mut c.body
            && list_type == b"EfdG"
        {
            *list_type = *b"XfdG";
        }
    });
    let r = report_of(&bytes);
    assert_eq!(r.imported.effects, 1, "{r:?}");
    assert_eq!(r.count(effectcraft_aep_import::LossCategory::Effects), 1, "{r:?}");
    assert!(try_all(&bytes));

    // A parT whose definitions were renamed (no pard at all) and an empty definition list.
    for id in [*b"Xard", *b"pard"] {
        let bytes = mutate("368-fx-ramp-default.aep", &mut |c| {
            if c.is(b"pard") {
                c.id = id;
                if let Body::Data(d) = &mut c.body {
                    d.to_mut().clear();
                }
            }
        });
        assert!(try_all(&bytes));
        let r = report_of(&bytes);
        assert!(r.imported.effects <= 1, "{r:?}");
    }

    // A wrong valid maximum is not guessed: the slider is reported.
    let bytes = mutate("380-fx-ramp-blend-25pct.aep", &mut |c| {
        if c.is(b"pard") && c.bytes().is_some_and(|b| b.get(16..35) == Some(b"Blend With Original")) {
            set(c, 128, &(100u32 << 16).to_be_bytes());
        }
    });
    let r = report_of(&bytes);
    assert!(r.losses.iter().any(|l| l.details.iter().any(|d| d.contains("Blend With Original") && d.contains("valid maximum"))), "{r:?}");

    // The same for a float slider (Exposure's Gamma Correction with another valid maximum).
    let bytes = mutate("394-fx-exposure-gamma-2.2.aep", &mut |c| {
        if c.is(b"pard") && c.bytes().is_some_and(|b| b.get(16..33) == Some(b"Gamma Correction\0")) {
            set(c, 108, &20f32.to_be_bytes());
        }
    });
    let r = report_of(&bytes);
    assert!(r.losses.iter().any(|l| l.details.iter().any(|d| d.contains("Gamma Correction") && d.contains("valid maximum"))), "{r:?}");
    assert_eq!(r.imported.effects, 1);

    // A group marker typed as a slider is reported, not read as a value.
    let bytes = mutate("389-fx-exposure-default.aep", &mut |c| {
        if c.is(b"pard") && c.bytes().is_some_and(|b| b.get(12..16) == Some(&13u32.to_be_bytes()[..]) && b.get(16..23) == Some(b"Master\0")) {
            set(c, 12, &10u32.to_be_bytes());
        }
    });
    assert!(try_all(&bytes));
    let r = report_of(&bytes);
    assert_eq!(r.imported.effects, 1, "{r:?}");
    assert!(r.count(effectcraft_aep_import::LossCategory::Effects) >= 1, "{r:?}");
}

#[test]
fn exposure_values_beyond_effectcrafts_slider_range_are_kept() {
    // After Effects accepts Exposure -100..100 stops (389); EffectCraft's own range is narrower.
    // A stored 50 is imported as stored (the effect renders it), not clamped and not reported.
    let bytes = mutate("391-fx-exposure-master-1.5.aep", &mut |c| {
        if c.is(b"cdat") && c.bytes().is_some_and(|b| b.get(0..8) == Some(&1.5f64.to_be_bytes()[..])) {
            set(c, 0, &50f64.to_be_bytes());
        }
    });
    let mut project = Project::default();
    let none = |_: &str| false;
    let r = import(&mut project, &bytes, &ImportOptions::new(&none)).expect("lossless");
    assert!(r.report.is_lossless(), "{:?}", r.report.losses);
    let mut found = None;
    for id in &r.comps {
        if let Some(item) = project.item(*id)
            && let ItemKind::Comp(c) = &item.kind
        {
            for l in &c.layers {
                if let Some(fx) = l.props.sub("effects").and_then(|g| g.groups().next()) {
                    found = fx.prop("master/exposure").map(|p| p.value.clone());
                }
            }
        }
    }
    assert_eq!(found, Some(effectcraft_keyframe::Value::Scalar(50.0)));
}

#[test]
fn huge_effect_lists_stay_bounded() {
    // The Tint's parT with its pard records repeated 2 000 times and the effect parade with 200
    // copies of the effect: the import finishes and every copy arrives or is reported.
    let bytes = fixture_bytes("356-fx-tint-default.aep");
    let mut file: AepFile<'_> = parse(&bytes).unwrap();
    fn grow(chunks: &mut [Chunk<'_>]) {
        for c in chunks {
            if let Body::List { list_type, children } = &mut c.body {
                if list_type == b"parT" {
                    let extra: Vec<Chunk<'_>> = children.iter().filter(|k| k.is(b"pard") || k.is(b"tdmn")).cloned().collect();
                    for _ in 0..2_000 / extra.len().max(1) {
                        children.extend(extra.iter().cloned());
                    }
                } else if list_type == b"tdgp" && children.iter().any(|k| k.is_list(b"sspc")) {
                    let pair: Vec<Chunk<'_>> = children.iter().filter(|k| k.is(b"tdmn") || k.is_list(b"sspc")).cloned().collect();
                    for _ in 0..200 {
                        children.extend(pair.iter().cloned());
                    }
                }
            }
            grow(c.children_mut());
        }
    }
    grow(&mut file.chunks);
    let bytes = write(&file).unwrap();
    assert!(try_all(&bytes));
    let r = report_of(&bytes);
    assert!(r.imported.effects >= 1, "{:?}", r.summary());
}

#[test]
fn huge_image_sequences_are_bounded() {
    // 603 with the last frame number at 99 999 (sspc u32 at 176) and its item copied 15 times:
    // 1.5 million file names would be created; the import expands at most a million in total and
    // links the first file of the others.
    let bytes = fixture_bytes("603-footage-exrseq-0101-as-layer.aep");
    let mut file: AepFile<'static> = parse(&bytes).expect("parse").into_owned();
    let fold = file.chunks.iter_mut().find(|c| c.is_list(b"Fold")).expect("Fold");
    let Body::List { children, .. } = &mut fold.body else { panic!("Fold") };
    let at = children
        .iter()
        .position(|c| c.is_list(b"Item") && c.child_list(b"Pin ").and_then(|p| p.child(b"sspc")).and_then(|s| s.bytes()).is_some_and(|b| &b[22..26] == b"oEXR"))
        .expect("sequence item");
    let mut item = children[at].clone();
    visit(std::slice::from_mut(&mut item), &mut |c| {
        if c.is(b"sspc") && c.bytes().is_some_and(|b| b.len() > 180 && &b[22..26] == b"oEXR") {
            set(c, 176, &99_999u32.to_be_bytes());
        }
    });
    for _ in 0..15 {
        children.insert(at, item.clone());
    }
    let out = write(&file).expect("write");
    let mut p = Project::default();
    let none = |_: &str| false;
    let r = import(&mut p, &out, &ImportOptions { allow_losses: true, ..ImportOptions::new(&none) }).expect("import");
    let files: usize = p.items.values().filter_map(|i| if let ItemKind::Footage(f) = &i.kind { Some(f.sequence.len()) } else { None }).sum();
    assert!(files <= 1_000_000, "{files}");
    assert!(r.report.count(LossCategory::DamagedData) > 0, "{:?}", r.report.losses);
}
