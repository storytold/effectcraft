//! CC RepeTile (#3626), against After Effects 2026's own read-back of the committed project
//! `tests/fixtures/cc-repetile-3626.aep` (generator: `tests/fixtures/tools/cc-repetile-3626.jsx`).
//! Every composition is the corpus base (1920 × 1080, 25 fps, 5 s, red solid `L`) with CC RepeTile
//! on `L`. After Effects read back:
//!
//! - `repetile-values`: Expand Right 120, Left 80, Down 40, Up 20, Tiling 4 (Unfold), Blend Borders 0.25
//! - `repetile-defaults`: every parameter at its default (Tiling 1, Repeat)
//! - `repetile-rosette`: Tiling 9 (Rosette), Expand Right keyed 0 at 1 s and 300 at 2 s
//! - `repetile-wide`: Expand Left 9838 (above EffectCraft's slider range, which stops at 4000)

use effectcraft_aep::import::{ImportOptions, ImportResult, import};
use effectcraft_keyframe::Value;
use effectcraft_project::{Layer, Project};
use effectcraft_time::Tick;

use crate::import::helpers::comp;

const REPETILE: &[u8] = include_bytes!("../../fixtures/cc-repetile-3626.aep");

/// The fixture imported into an empty project (losses allowed).
fn load() -> (Project, ImportResult) {
    let mut project = Project::default();
    let none = |_: &str| false;
    let r = import(&mut project, REPETILE, &ImportOptions { allow_losses: true, ..ImportOptions::new(&none) }).unwrap();
    (project, r)
}

/// Layer `L` of composition `name`.
fn layer<'p>(p: &'p Project, r: &ImportResult, name: &str) -> &'p Layer {
    let (_, c) = comp(p, r, name);
    c.layers.iter().find(|l| l.name == "L").unwrap_or_else(|| panic!("{name}: no layer L"))
}

/// The static value of parameter `id` of the layer's first effect.
fn value(l: &Layer, id: &str) -> Value {
    l.props.prop(&format!("effects/#1/{id}")).unwrap_or_else(|| panic!("{id}")).value.clone()
}

/// The report lines that name composition `name`.
fn losses(r: &ImportResult, name: &str) -> Vec<String> {
    r.report.losses.iter().flat_map(|l| l.details.iter()).filter(|d| d.starts_with(&format!("{name}:"))).cloned().collect()
}

#[test]
fn cc_repetile_imports_as_edge_tile_repeat() {
    let (p, r) = load();
    let l = layer(&p, &r, "repetile-values");
    assert_eq!(value(l, "expandRight"), Value::Scalar(120.0));
    assert_eq!(value(l, "expandLeft"), Value::Scalar(80.0));
    assert_eq!(value(l, "expandDown"), Value::Scalar(40.0));
    assert_eq!(value(l, "expandUp"), Value::Scalar(20.0));
    // After Effects' item 4, Unfold, is EffectCraft's item 1.
    assert_eq!(value(l, "tiling"), Value::Enum(1));
    // Blend Borders has no counterpart: reported, and nothing else is.
    let lost = losses(&r, "repetile-values");
    assert_eq!(lost.len(), 1, "{lost:?}");
    assert!(lost[0].contains("Blend Borders"), "{lost:?}");
}

#[test]
fn cc_repetile_defaults_import_without_losses() {
    let (p, r) = load();
    let l = layer(&p, &r, "repetile-defaults");
    for id in ["expandRight", "expandLeft", "expandDown", "expandUp"] {
        assert_eq!(value(l, id), Value::Scalar(0.0), "{id}");
    }
    assert_eq!(value(l, "tiling"), Value::Enum(0), "Repeat");
    assert!(losses(&r, "repetile-defaults").is_empty(), "{:?}", losses(&r, "repetile-defaults"));
}

#[test]
fn cc_repetile_tiling_without_counterpart_is_reported_and_keys_import() {
    let (p, r) = load();
    let l = layer(&p, &r, "repetile-rosette");
    let lost = losses(&r, "repetile-rosette");
    assert!(lost.iter().any(|d| d.contains("Tiling") && d.contains("'Rosette' has no counterpart")), "{lost:?}");
    assert_eq!(value(l, "tiling"), Value::Enum(0), "the default, Repeat");
    let keys = &l.props.prop("effects/#1/expandRight").unwrap().keys;
    let got: Vec<(Tick, Value)> = keys.iter().map(|k| (k.time, k.value.clone())).collect();
    assert_eq!(got, vec![(Tick::from_seconds_f64(1.0), Value::Scalar(0.0)), (Tick::from_seconds_f64(2.0), Value::Scalar(300.0))]);
}

#[test]
fn cc_repetile_keeps_an_expansion_beyond_the_slider_range() {
    let (p, r) = load();
    let l = layer(&p, &r, "repetile-wide");
    assert_eq!(value(l, "expandLeft"), Value::Scalar(9838.0));
    assert!(losses(&r, "repetile-wide").is_empty(), "{:?}", losses(&r, "repetile-wide"));
}
