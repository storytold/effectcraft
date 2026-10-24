//! Parameter defaults in the form After Effects writes when it saves a project it opened (the
//! re-saves of export tests e1 and e2): point defaults in percent of the layer instead of
//! fractions, and a popup default slot of 0. Simulated by patching our own fixtures.

use effectcraft_aep_import::{ImportOptions, import};
use effectcraft_keyframe::Value;
use effectcraft_project::{ItemKind, Project, PropGroup};

fn fixture(name: &str) -> Vec<u8> {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../aep/tests/fixtures");
    std::fs::read(dir.join(name)).expect(name)
}

/// Rewrites `pard@56..` of every parameter definition named `name` (the 32-byte name at @16).
fn patch_default(bytes: &mut [u8], name: &str, value: &[u8]) -> usize {
    let mut n = 0;
    let pat = name.as_bytes();
    let mut i = 0;
    while i + pat.len() < bytes.len() {
        if &bytes[i..i + pat.len()] == pat && i >= 16 && bytes[i + pat.len()] == 0 && &bytes[i - 24..i - 20] == b"pard" {
            let at = i - 16 + 56;
            bytes[at..at + value.len()].copy_from_slice(value);
            n += 1;
        }
        i += 1;
    }
    n
}

fn effects(bytes: &[u8]) -> (Vec<PropGroup>, String) {
    let mut p = Project::default();
    let none = |_: &str| false;
    let r = import(&mut p, bytes, &ImportOptions { allow_losses: true, ..ImportOptions::new(&none) }).expect("import");
    let mut out = Vec::new();
    for item in p.items.values() {
        if let ItemKind::Comp(c) = &item.kind {
            for l in &c.layers {
                if let Some(fx) = l.effects() {
                    out.extend(fx.groups().cloned());
                }
            }
        }
    }
    (out, format!("{:?}", r.report.losses))
}

fn fixed(v: f64) -> [u8; 4] {
    ((v * 65536.0) as i32).to_be_bytes()
}

#[test]
fn point_defaults_in_percent_are_read_as_fractions() {
    // 368: Gradient Ramp at its defaults on a 1920 x 1080 layer; the re-saved form stores Start
    // 50 / 0 and End 50 / 100.
    let mut bytes = fixture("368-fx-ramp-default.aep");
    let start = [fixed(50.0), fixed(0.0)].concat();
    let end = [fixed(50.0), fixed(100.0)].concat();
    assert!(patch_default(&mut bytes, "Start of Ramp", &start) >= 1);
    assert!(patch_default(&mut bytes, "End of Ramp", &end) >= 1);
    let (fx, losses) = effects(&bytes);
    assert_eq!(fx[0].prop("start").unwrap().value, Value::Vec2([960.0, 0.0]), "{losses}");
    assert_eq!(fx[0].prop("end").unwrap().value, Value::Vec2([960.0, 1080.0]));
    // The fraction form of the fresh instance still reads the same.
    let (fx, _) = effects(&fixture("368-fx-ramp-default.aep"));
    assert_eq!(fx[0].prop("end").unwrap().value, Value::Vec2([960.0, 1080.0]));
}

#[test]
fn a_popup_default_slot_of_zero_is_the_fresh_default() {
    // 712: Luma Key at its defaults (Key Out Darker); the re-saved form leaves 0 in the slot.
    let mut bytes = fixture("712-fx-eleven-effects-defaults-on-M.aep");
    assert!(patch_default(&mut bytes, "Key Type", &0u32.to_be_bytes()) >= 1);
    let (fx, losses) = effects(&bytes);
    let luma = fx.iter().find(|g| g.match_id == "ec.key.luma").expect("luma key");
    let (fresh, _) = effects(&fixture("712-fx-eleven-effects-defaults-on-M.aep"));
    let want = fresh.iter().find(|g| g.match_id == "ec.key.luma").unwrap().prop("keyType").unwrap().value.clone();
    assert_eq!(luma.prop("keyType").unwrap().value, want);
    assert!(!losses.contains("Key Type"), "{losses}");
}
