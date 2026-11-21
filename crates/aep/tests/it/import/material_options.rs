//! Material Options of a layer and the shadow settings of a light. After Effects 2026 writes a
//! property set to a value of its own as a record of state 1 holding the value as shown
//! (Casts Shadows and Accepts Shadows 0 Off, 1 On, 2 Only; Accepts Lights 0 when off; the
//! coefficients in percent; a light's Shadow Darkness in percent, Shadow Diffusion in pixels).
//! The tests write such records into the default-state records of `synthetic.aep` and of a
//! spot light case.

use effectcraft_aep::import::{ImportOptions, ImportResult, import};
use effectcraft_aep::{Body, Chunk, parse, write};
use effectcraft_keyframe::Value;
use effectcraft_project::{Layer, Project};

const SYNTHETIC: &[u8] = include_bytes!("../../fixtures/synthetic.aep");

fn import_all(bytes: &[u8]) -> (Project, ImportResult) {
    let mut project = Project::default();
    let none = |_: &str| false;
    let r = import(&mut project, bytes, &ImportOptions { allow_losses: true, ..ImportOptions::new(&none) }).unwrap();
    (project, r)
}

/// Writes `value` into every record of property `name` as a written (state 1) static value;
/// returns the new bytes and how many records were changed.
fn set(bytes: &[u8], values: &[(&str, f64)]) -> (Vec<u8>, usize) {
    fn walk(chunks: &mut [Chunk<'_>], values: &[(&str, f64)], n: &mut usize) {
        let mut pending: Option<f64> = None;
        for c in chunks.iter_mut() {
            if c.id == *b"tdmn" {
                let name = c.bytes().map(|b| b.split(|x| *x == 0).next().unwrap_or_default().to_vec()).unwrap_or_default();
                pending = values.iter().find(|(m, _)| m.as_bytes() == name.as_slice()).map(|(_, v)| *v);
                continue;
            }
            if let Some(v) = pending.take()
                && c.is_list(b"tdbs")
            {
                for r in c.children_mut() {
                    let Body::Data(d) = &mut r.body else { continue };
                    let d = d.to_mut();
                    match &r.id {
                        b"tdsb" => d.copy_from_slice(&1u32.to_be_bytes()),
                        b"cdat" => d[..8].copy_from_slice(&v.to_be_bytes()),
                        _ => {}
                    }
                }
                *n += 1;
            }
            walk(c.children_mut(), values, n);
        }
    }
    let mut file = parse(bytes).unwrap();
    let mut n = 0;
    walk(&mut file.chunks, values, &mut n);
    (write(&file).unwrap(), n)
}

/// Renames every record of property `from` to `to` (a match name is stored NUL-padded).
fn rename(bytes: &[u8], from: &str, to: &str) -> Vec<u8> {
    fn walk(chunks: &mut [Chunk<'_>], from: &str, to: &str) {
        for c in chunks.iter_mut() {
            if c.id == *b"tdmn"
                && let Body::Data(d) = &mut c.body
                && d.split(|x| *x == 0).next() == Some(from.as_bytes())
            {
                let d = d.to_mut();
                assert!(to.len() < d.len());
                d.fill(0);
                d[..to.len()].copy_from_slice(to.as_bytes());
            }
            walk(c.children_mut(), from, to);
        }
    }
    let mut file = parse(bytes).unwrap();
    walk(&mut file.chunks, from, to);
    write(&file).unwrap()
}

fn layers(p: &Project) -> Vec<Layer> {
    p.comps().flat_map(|(_, c)| c.layers.iter().filter(|l| l.props.sub("materialOptions").is_some()).cloned()).collect()
}

fn value(l: &Layer, path: &str) -> Value {
    l.props.prop(path).unwrap_or_else(|| panic!("{}: no {path}", l.name)).value.clone()
}

#[test]
fn material_options_are_imported() {
    let (bytes, n) = set(
        SYNTHETIC,
        &[
            ("ADBE Casts Shadows", 2.0),
            ("ADBE Light Transmission", 40.0),
            ("ADBE Accepts Shadows", 0.0),
            ("ADBE Accepts Lights", 0.0),
            ("ADBE Ambient Coefficient", 30.0),
            ("ADBE Diffuse Coefficient", 80.0),
            ("ADBE Specular Coefficient", 20.0),
            ("ADBE Shininess Coefficient", 40.0),
            ("ADBE Metal Coefficient", 0.0),
        ],
    );
    assert!(n >= 9, "{n} records written");
    let (p, r) = import_all(&bytes);
    let ls = layers(&p);
    assert!(!ls.is_empty());
    for l in &ls {
        assert_eq!(value(l, "materialOptions/castsShadows"), Value::Enum(2), "{}", l.name);
        assert_eq!(value(l, "materialOptions/lightTransmission"), Value::Scalar(40.0));
        assert_eq!(value(l, "materialOptions/acceptsShadows"), Value::Enum(0));
        assert_eq!(value(l, "materialOptions/acceptsLights"), Value::Bool(false));
        assert_eq!(value(l, "materialOptions/ambient"), Value::Scalar(30.0));
        assert_eq!(value(l, "materialOptions/diffuse"), Value::Scalar(80.0));
        assert_eq!(value(l, "materialOptions/specularIntensity"), Value::Scalar(20.0));
        assert_eq!(value(l, "materialOptions/specularShininess"), Value::Scalar(40.0));
        assert_eq!(value(l, "materialOptions/metal"), Value::Scalar(0.0));
    }
    // They were reported as not imported only in the general limitation line.
    let lost = r
        .report
        .losses
        .iter()
        .flat_map(|l| &l.details)
        .filter(|d| d.contains("Coefficient") || d.contains("Shadows") || d.contains("Light"))
        .collect::<Vec<_>>();
    assert!(lost.is_empty(), "{lost:?}");
    assert!(!r.report.limitations.iter().any(|l| l.contains("material options")), "{:?}", r.report.limitations);
}

#[test]
fn a_casts_shadows_value_out_of_range_keeps_the_default() {
    let (bytes, _) = set(SYNTHETIC, &[("ADBE Casts Shadows", 3.0)]);
    let (p, r) = import_all(&bytes);
    for l in &layers(&p) {
        assert_eq!(value(l, "materialOptions/castsShadows"), Value::Enum(0), "{}", l.name);
    }
    assert!(r.report.losses.iter().flat_map(|l| &l.details).any(|d| d.contains("ADBE Casts Shadows holds a value that is not valid")));
}

#[test]
fn the_shadow_darkness_and_diffusion_of_a_light_are_imported() {
    // A light stores Shadow Darkness and Diffusion only when they are set (lab file
    // `material_options`: 70 and 10, state 1). The case stores neither, so two of its
    // default-state records become them.
    let Some(case) = crate::common::try_fixture("254-light-spot-intensity-50.aep") else { return };
    let case = rename(&rename(&case, "ADBE Light Falloff Start", "ADBE Light Shadow Darkness"), "ADBE Light Falloff Distance", "ADBE Light Shadow Diffusion");
    let (bytes, n) = set(&case, &[("ADBE Light Shadow Darkness", 70.0), ("ADBE Light Shadow Diffusion", 10.0)]);
    assert!(n >= 2, "{n} records written");
    let (p, r) = import_all(&bytes);
    let light = p.comps().flat_map(|(_, c)| c.layers.iter()).find(|l| l.props.sub("lightOptions").is_some()).expect("a light");
    assert_eq!(value(light, "lightOptions/shadowDarkness"), Value::Scalar(70.0));
    assert_eq!(value(light, "lightOptions/shadowDiffusion"), Value::Scalar(10.0));
    let lost = r.report.losses.iter().flat_map(|l| &l.details).filter(|d| d.contains("Shadow")).collect::<Vec<_>>();
    assert!(lost.is_empty(), "{lost:?}");
}
