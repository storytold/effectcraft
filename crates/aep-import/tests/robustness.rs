//! Damaged and hostile files produce an error or a report, never a panic.

mod common;

use common::fixture_bytes;
use effectcraft_aep::{AepFile, Body, Chunk, parse, write};
use effectcraft_aep_import::{AepImportError, ImportOptions, import};
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
                for path in ["transform/anchor", "transform/position", "transform/scale", "transform/rotation", "transform/opacity"] {
                    if let Some(p) = l.props.prop(path) {
                        assert!(p.keys.windows(2).all(|w| w[0].time < w[1].time));
                        assert!(p.value.components().iter().all(|v| v.is_finite()), "{path}: {:?}", p.value);
                        for k in &p.keys {
                            assert!(k.value.components().iter().all(|v| v.is_finite()));
                        }
                    }
                }
            }
        }
    }
    assert!(seen >= comps);
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
    let others = ["000-base.aep", "022-layer-parent-null.aep", "124-precomp-Pre.aep", "126-footage-png-layer.aep", "212-anchor-400x300-solid-100-50.aep"];
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
