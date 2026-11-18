//! Keys on an effect checkbox are imported. After Effects stores them as any one-component key:
//! 48-byte records with the value 0 or 1, both sides Hold (it holds every checkbox key; lab
//! keyed-param-kinds, Fractal Noise's Invert keyed off at 0 s and on at 1 s). Each effect is built
//! into `synthetic.aep` (`fx_inject`) in place of its Glow.

use crate::import::fx_inject::{Default, Fx, bytes_with, import_bytes, losses, other, popup};
use effectcraft_aep::effect_map::{FRACTAL_TYPE, NOISE_TYPE};
use effectcraft_aep::import::LossCategory;
use effectcraft_aep::{Body, Chunk, ParamKind as K, parse, write};
use effectcraft_keyframe::{Interp, Value};

/// Sets both interpolation bytes of every key of parameter `param` to Hold (3), as After Effects
/// writes checkbox keys; the number of records changed.
fn hold_keys(chunks: &mut [Chunk<'_>], param: &str) -> usize {
    let mut n = 0;
    for ch in chunks {
        let kids = ch.children_mut();
        let name = format!("{param}\0");
        if let Some(at) = kids.iter().position(|k| k.is(b"tdmn") && k.bytes().is_some_and(|b| b.starts_with(name.as_bytes())))
            && let Some(rec) = kids.get_mut(at + 1)
        {
            for list in rec.children_mut().iter_mut().filter(|k| k.is_list(b"list")) {
                for ldat in list.children_mut().iter_mut().filter(|k| k.is(b"ldat")) {
                    if let Body::Data(d) = &mut ldat.body {
                        for key in d.to_mut().chunks_mut(48) {
                            key[4] = 3;
                            key[5] = 3;
                            n += 1;
                        }
                    }
                }
            }
        }
        n += hold_keys(kids, param);
    }
    n
}

fn held(fx: &Fx, param: &str) -> Vec<u8> {
    let bytes = bytes_with(fx);
    let mut file = parse(&bytes).expect("parse");
    assert!(hold_keys(&mut file.chunks, param) >= 2);
    write(&file).expect("write")
}

fn keys(g: &effectcraft_project::PropGroup, id: &str) -> Vec<(Value, Interp, Interp)> {
    g.prop(id).expect(id).keys.iter().map(|k| (k.value.clone(), k.in_interp, k.out_interp)).collect()
}

#[test]
fn keys_on_a_mapped_checkbox_are_imported() {
    let fx = Fx {
        match_name: "ADBE Fractal Noise",
        name: "Fractal Noise",
        defs: vec![popup("Fractal Type", 1, FRACTAL_TYPE), popup("Noise Type", 3, NOISE_TYPE), other(K::Checkbox, "Invert", Default::Number(0.0))],
        values: vec![],
        data: vec![],
        keys: vec![(3, vec![(0, 0.0), (25_600, 1.0)])],
    };
    let (g, r) = import_bytes(&held(&fx, "ADBE Fractal Noise-0003"), "ec.noise.fractal");
    assert_eq!(losses(&r, LossCategory::Effects, "ADBE Fractal Noise"), Vec::<String>::new());
    assert_eq!(losses(&r, LossCategory::DamagedData, "ADBE Fractal Noise"), Vec::<String>::new());
    let g = g.expect("imported");
    assert_eq!(keys(&g, "invert"), [(Value::Bool(false), Interp::Hold, Interp::Hold), (Value::Bool(true), Interp::Hold, Interp::Hold)]);
}

#[test]
fn keys_on_a_placeholder_checkbox_are_kept() {
    let fx = Fx {
        match_name: "CC Cylinder",
        name: "CC Cylinder",
        defs: vec![other(K::Checkbox, "Wire", Default::Number(1.0))],
        values: vec![],
        data: vec![],
        keys: vec![(1, vec![(0, 1.0), (51_200, 0.0)])],
    };
    let (g, r) = import_bytes(&held(&fx, "CC Cylinder-0001"), "CC Cylinder");
    assert!(!losses(&r, LossCategory::Effects, "CC Cylinder").iter().any(|d| d.contains("keyframes")), "{:?}", r.report.losses);
    let g = g.expect("the placeholder");
    assert_eq!(keys(&g, "CC Cylinder-0001"), [(Value::Bool(true), Interp::Hold, Interp::Hold), (Value::Bool(false), Interp::Hold, Interp::Hold)]);
}
