//! Eases that were read as linear before: Anchor Point (597, 730), an effect point and an effect
//! colour (598), mask paths (599, 700). EffectCraft's values between the keys are compared with
//! After Effects' read-backs (MANIFEST S17 / S19).

mod common;

use common::{comp, load};
use effectcraft_keyframe::Value;
use effectcraft_project::{Layer, Project};
use effectcraft_time::{TICKS_PER_SECOND, Tick};

fn at(s: f64) -> Tick {
    Tick((s * TICKS_PER_SECOND as f64).round() as i64)
}

fn layer<'p>(p: &'p Project, r: &effectcraft_aep_import::ImportResult, name: &str) -> &'p Layer {
    let (_, c) = comp(p, r, "C");
    c.layers.iter().find(|l| l.name == name).unwrap_or_else(|| panic!("no {name}"))
}

fn xy(v: &Value) -> [f64; 2] {
    match v {
        Value::Vec2(a) => *a,
        Value::Vec3(a) => [a[0], a[1]],
        v => panic!("{v:?}"),
    }
}

fn close(got: [f64; 2], want: [f64; 2], tol: f64, what: &str) {
    assert!((got[0] - want[0]).abs() <= tol && (got[1] - want[1]).abs() <= tol, "{what}: {got:?} vs After Effects {want:?}");
}

#[test]
fn anchor_point_ease_matches_after_effects() {
    for (name, l, want) in [
        ("597-key-anchor-ease-out50-40-in30-60.aep", "L", [[1004.4121, 562.2061], [1084.8353, 602.4177], [1137.1339, 628.5670]]),
        ("730-key-anchor-ease-on-400x800-solid.aep", "P", [[225.2343, 425.2343], [261.4341, 461.4341], [286.0550, 486.0550]]),
    ] {
        let (p, r) = load(name);
        assert!(r.report.is_lossless(), "{name}: {:?}", r.report.losses);
        let a = layer(&p, &r, l).props.prop("transform/anchor").unwrap();
        for (t, w) in [0.5, 1.0, 1.5].into_iter().zip(want) {
            close(xy(&a.value_at(at(t))), w, 0.5, &format!("{name} at {t} s"));
        }
    }
}

#[test]
fn effect_point_and_colour_ease_match_after_effects() {
    let (p, r) = load("598-key-ramp-point-ease-color-bezier.aep");
    assert!(r.report.is_lossless(), "{:?}", r.report.losses);
    let l = layer(&p, &r, "L");
    let fx = l.props.sub("effects").and_then(|e| e.groups().next()).expect("ramp");
    let start = fx.prop("start").unwrap();
    for (t, w) in [0.5, 1.0, 1.5].into_iter().zip([[178.4012, 239.2006], [353.0563, 326.5281], [462.9131, 381.4566]]) {
        close(xy(&start.value_at(at(t))), w, 0.5, &format!("Start of Ramp at {t} s"));
    }
    let colour = fx.prop("startColor").unwrap();
    for (t, red) in [(0.5, 0.75), (1.0, 0.5), (1.5, 0.25)] {
        let Value::Color(c) = colour.value_at(at(t)) else { panic!() };
        assert!((c[0] - red).abs() < 1e-3 && (c[2] - (1.0 - red)).abs() < 1e-3, "{t}: {c:?}");
    }
}

#[test]
fn mask_path_ease_matches_after_effects() {
    for (name, want) in [
        ("599-key-mask-path-ease-infl75.aep", [[849.1606, 434.5803], [760.0, 390.0], [670.8394, 345.4197]]),
        ("700-key-mask-path-ease-out20-in50.aep", [[803.9844, 411.9922], [727.6194, 373.8097], [676.9510, 348.4755]]),
    ] {
        let (p, r) = load(name);
        assert!(r.report.is_lossless(), "{name}: {:?}", r.report.losses);
        let l = layer(&p, &r, "L");
        let mask = l.props.sub("masks").and_then(|m| m.groups().next()).expect("mask");
        let path = mask.prop("path").unwrap();
        for (t, w) in [0.5, 1.0, 1.5].into_iter().zip(want) {
            let Value::Path(s) = path.value_at(at(t)) else { panic!() };
            let v = s.vertices.iter().copied().fold([f64::MAX, f64::MAX], |m, v| [m[0].min(v[0]), m[1].min(v[1])]);
            close(v, w, 0.5, &format!("{name} at {t} s"));
        }
    }
}
