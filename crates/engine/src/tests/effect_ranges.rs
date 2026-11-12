//! Effect parameters take the whole valid range After Effects records for them (fx_defs in the
//! aep crate), end to end: `prop.set` keeps both ends, undo and redo restore them, and a saved
//! project reopens with them. The aep crate's
//! `effect_hard_ranges_cover_recorded_after_effects_ranges` checks the ranges themselves
//! against that table.

use effectcraft_project::{LayerId, Node, ParamUi, PropGroup};
use serde_json::{Value, json};

use crate::Session;

/// (issue, effect, parameter path, lowest, highest): the ends of the range the effect now accepts,
/// in EffectCraft's units: After Effects' valid range, or wider where EffectCraft already was.
/// At least one end of each is outside the range accepted before the issue.
#[rustfmt::skip]
const ROWS: &[(u32, &str, &str, f64, f64)] = &[
    (3487, "ec.color.levelsic", "rgb/rgbInBlack", -10000.0, 10000.0),
    (3488, "ec.color.levelsic", "rgb/rgbInWhite", -10000.0, 10000.0),
    (3489, "ec.color.levelsic", "rgb/rgbOutBlack", -10000.0, 10000.0),
    (3490, "ec.color.levelsic", "rgb/rgbOutWhite", -10000.0, 10000.0),
    (3491, "ec.color.levelsic", "rgb/rgbGamma", 0.0, 10.0),
    (3487, "ec.color.levelsic", "red/redInBlack", -10000.0, 10000.0),
    (3488, "ec.color.levelsic", "red/redInWhite", -10000.0, 10000.0),
    (3489, "ec.color.levelsic", "red/redOutBlack", -10000.0, 10000.0),
    (3490, "ec.color.levelsic", "red/redOutWhite", -10000.0, 10000.0),
    (3491, "ec.color.levelsic", "red/redGamma", 0.0, 10.0),
    (3487, "ec.color.levelsic", "green/greenInBlack", -10000.0, 10000.0),
    (3488, "ec.color.levelsic", "green/greenInWhite", -10000.0, 10000.0),
    (3489, "ec.color.levelsic", "green/greenOutBlack", -10000.0, 10000.0),
    (3490, "ec.color.levelsic", "green/greenOutWhite", -10000.0, 10000.0),
    (3491, "ec.color.levelsic", "green/greenGamma", 0.0, 10.0),
    (3487, "ec.color.levelsic", "blue/blueInBlack", -10000.0, 10000.0),
    (3488, "ec.color.levelsic", "blue/blueInWhite", -10000.0, 10000.0),
    (3489, "ec.color.levelsic", "blue/blueOutBlack", -10000.0, 10000.0),
    (3490, "ec.color.levelsic", "blue/blueOutWhite", -10000.0, 10000.0),
    (3491, "ec.color.levelsic", "blue/blueGamma", 0.0, 10.0),
    (3487, "ec.color.levelsic", "alpha/alphaInBlack", -10000.0, 10000.0),
    (3488, "ec.color.levelsic", "alpha/alphaInWhite", -10000.0, 10000.0),
    (3489, "ec.color.levelsic", "alpha/alphaOutBlack", -10000.0, 10000.0),
    (3490, "ec.color.levelsic", "alpha/alphaOutWhite", -10000.0, 10000.0),
    (3491, "ec.color.levelsic", "alpha/alphaGamma", 0.0, 10.0),
    (3478, "ec.generate.fourcolor", "blend", 1.0, 10000.0),
    (3479, "ec.generate.fourcolor", "jitter", 0.0, 500.0),
    (3483, "ec.noise.noisehls", "hue", 0.0, 1000.0),
    (3483, "ec.noise.noisehls", "lightness", 0.0, 1000.0),
    (3483, "ec.noise.noisehls", "saturation", 0.0, 1000.0),
    (3484, "ec.noise.noisehls", "grainSize", 0.0, 100.0),
    (3483, "ec.noise.noisehlsauto", "hue", 0.0, 1000.0),
    (3483, "ec.noise.noisehlsauto", "lightness", 0.0, 1000.0),
    (3483, "ec.noise.noisehlsauto", "saturation", 0.0, 1000.0),
    (3484, "ec.noise.noisehlsauto", "grainSize", 0.0, 100.0),
    (3480, "ec.color.changecolor", "hueTransform", -1800.0, 1800.0),
    (3476, "ec.generate.grid", "border", 0.0, 2000.0),
    (3472, "ec.transition.venetian", "width", 1.0, 32000.0),
    (3473, "ec.transition.venetian", "feather", 0.0, 32000.0),
    (3474, "ec.transition.radialwipe", "feather", 0.0, 32000.0),
    (3477, "ec.stylize.scatter", "amount", 0.0, 1000.0),
    (3475, "ec.generate.beam", "startThickness", 0.0, 4000.0),
    (3475, "ec.generate.beam", "endThickness", 0.0, 4000.0),
    (3471, "ec.distort.transform", "scaleHeight", -30000.0, 30000.0),
    (3471, "ec.distort.transform", "scaleWidth", -30000.0, 30000.0),
    (3470, "ec.distort.turbulentdisplace", "evolutionOptions/randomSeed", 0.0, 100000.0),
];

fn path(effect: &str, param: &str) -> String {
    format!("effects/{effect}/{param}")
}

fn read(s: &mut Session, layer: u64, effect: &str, param: &str) -> f64 {
    let got = s.execute("prop.get", json!({"layer": layer, "path": path(effect, param)})).unwrap();
    got["value"].as_f64().unwrap_or_else(|| panic!("{effect} {param}: {got}"))
}

fn set(s: &mut Session, layer: u64, effect: &str, param: &str, v: f64) -> Value {
    s.execute("prop.set", json!({"layer": layer, "path": path(effect, param), "value": v})).unwrap()
}

/// A composition with two solids, each with one instance of every effect in [`ROWS`].
fn session() -> (Session, u64, u64) {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Main", "width": 320, "height": 180, "frameRate": 30, "duration": 2})).unwrap();
    let mut layers = [0; 2];
    for l in &mut layers {
        *l = s.execute("layer.newSolid", json!({"color": "#808080", "width": 64, "height": 36})).unwrap()["layer"].as_u64().unwrap();
        let mut effects: Vec<&str> = ROWS.iter().map(|r| r.1).collect();
        effects.dedup();
        for e in effects {
            s.execute("effect.apply", json!({"layers": [*l], "effect": e})).unwrap();
        }
    }
    (s, layers[0], layers[1])
}

/// Both ends of every row survive `prop.set`, undo / redo, and a save and reopen.
#[test]
fn effect_parameters_keep_both_ends_of_the_after_effects_range() {
    let (mut s, low, high) = session();
    for &(issue, e, p, lo, hi) in ROWS {
        assert_eq!(set(&mut s, high, e, p, lo), json!(lo), "#{issue} {e} {p}: prop.set reply");
        assert_eq!(read(&mut s, high, e, p), lo, "#{issue} {e} {p}");
        set(&mut s, high, e, p, hi);
        assert_eq!(read(&mut s, high, e, p), hi, "#{issue} {e} {p}");
        assert!(s.undo());
        assert_eq!(read(&mut s, high, e, p), lo, "#{issue} {e} {p}: undo");
        assert!(s.redo());
        assert_eq!(read(&mut s, high, e, p), hi, "#{issue} {e} {p}: redo");
        set(&mut s, low, e, p, lo);
    }
    let dir = std::env::temp_dir().join(format!("ec-effect-ranges-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("ranges.ecproj").to_string_lossy().to_string();
    s.execute("file.save", json!({"path": file})).unwrap();
    let mut s2 = Session::default();
    s2.execute("file.open", json!({"path": file})).unwrap();
    let _ = std::fs::remove_dir_all(dir);
    for &(issue, e, p, lo, hi) in ROWS {
        assert_eq!(read(&mut s2, low, e, p), lo, "#{issue} {e} {p}: reopened");
        assert_eq!(read(&mut s2, high, e, p), hi, "#{issue} {e} {p}: reopened");
    }
}

/// A project saved while these ranges were narrower stored that range with each instance
/// (`ParamUi`) and values clamped to it: opening it takes the effect's current range and keeps
/// the stored values, so the new ends can be set.
#[test]
fn projects_saved_with_the_narrower_ranges_open_with_the_new_ones() {
    let (mut s, low, _) = session();
    let cid = s.active_comp_id().unwrap();
    let mut old = (*s.project).clone();
    let stored: Vec<f64> = ROWS.iter().map(|&(_, e, p, ..)| read(&mut s, low, e, p)).collect();
    {
        let effectcraft_project::ItemKind::Comp(comp) = &mut old.items.get_mut(&cid).unwrap().kind else { panic!("not a composition") };
        let l = std::sync::Arc::make_mut(comp).layers.find_mut(|l| l.id == LayerId(low)).unwrap();
        for (&(_, e, p, ..), v) in ROWS.iter().zip(&stored) {
            let g = l.props.sub_mut("effects").unwrap().children.iter_mut().find_map(|n| match n {
                Node::Group(g) if g.match_id == e => Some(g),
                _ => None,
            });
            let pr = prop_mut(g.unwrap(), p).unwrap_or_else(|| panic!("{e} {p}"));
            // The narrowest stored range that still holds the stored value.
            pr.ui = ParamUi::Slider { min: *v, max: *v, slider_min: *v, slider_max: *v, decimals: 1 };
        }
    }
    let dir = std::env::temp_dir().join(format!("ec-effect-ranges-old-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("old.ecproj").to_string_lossy().to_string();
    std::fs::write(&file, old.to_json()).unwrap();
    let mut s2 = Session::default();
    s2.execute("file.open", json!({"path": file})).unwrap();
    let _ = std::fs::remove_dir_all(dir);
    for (&(issue, e, p, lo, hi), v) in ROWS.iter().zip(&stored) {
        assert_eq!(read(&mut s2, low, e, p), *v, "#{issue} {e} {p}: the stored value is kept");
        let spec = effectcraft_effects::find(e).unwrap().params.iter().find(|q| q.id == p).unwrap();
        let uid = s2.execute("prop.get", json!({"layer": low, "path": path(e, p)})).unwrap()["uid"].as_u64().unwrap();
        let layer = s2.project.comp(cid).unwrap().layer(LayerId(low)).unwrap();
        assert_eq!(layer.props.find(uid).unwrap().ui, spec.ui, "#{issue} {e} {p}: the range is refreshed");
        set(&mut s2, low, e, p, lo);
        assert_eq!(read(&mut s2, low, e, p), lo, "#{issue} {e} {p}");
        set(&mut s2, low, e, p, hi);
        assert_eq!(read(&mut s2, low, e, p), hi, "#{issue} {e} {p}");
    }
}

/// The property at `path` (`group/param`) in an effect instance.
fn prop_mut<'a>(g: &'a mut PropGroup, path: &str) -> Option<&'a mut effectcraft_project::Property> {
    match path.split_once('/') {
        None => g.get_mut(path),
        Some((first, rest)) => prop_mut(g.sub_mut(first)?, rest),
    }
}
