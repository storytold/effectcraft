//! Effects (Tint, Gradient Ramp, Exposure): every effect fixture is imported and exported, and the
//! exported effects are parsed back with `effectcraft-aep` and compared with the ones After
//! Effects wrote, parameter by parameter (stored values, keys, definitions); projects edited in
//! code check the parameters no fixture changes, several instances, and what is reported.

mod common;

use common::*;
use effectcraft_aep::{AepFile, ParamValue, parse};
use effectcraft_aep_export::LossCategory;
use effectcraft_aep_import::{ImportOptions, import};
use effectcraft_keyframe::{Interp, Value};
use effectcraft_project::{ItemKind, Node, Project, PropGroup};

fn fixture(name: &str) -> Vec<u8> {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../aep/tests/fixtures");
    std::fs::read(dir.join(name)).expect(name)
}

fn fixtures_with_effects() -> Vec<String> {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../aep/tests/fixtures");
    let mut v: Vec<String> = std::fs::read_dir(&dir)
        .expect("fixtures")
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        // Every fixture with an effect of the table (the dropdown and layer controls are not in it).
        .filter(|n| {
            n.ends_with(".aep")
                && (n.contains("-fx-") || n.contains("key-fill") || n.contains("key-ramp") || n.contains("adj-ramp"))
                && !n.contains("dropdown")
                && !n.contains("layercontrol")
        })
        .collect();
    v.sort();
    v
}

fn import_bytes(bytes: &[u8]) -> Project {
    let mut p = Project::default();
    let none = |_: &str| false;
    let opts = ImportOptions { allow_losses: true, ..ImportOptions::new(&none) };
    import(&mut p, bytes, &opts).expect("import");
    p
}

/// One effect as the reader sees it: match name, instance name, and per parameter index the
/// stored value (None = default) and the keys `(time units, value)`.
#[derive(Debug, PartialEq)]
struct Seen {
    match_name: String,
    name: Option<String>,
    params: Vec<(usize, Option<ParamValue>, Vec<(i32, Vec<f64>, (u8, u8))>)>,
}

/// The effects of every layer, by (composition name, layer index).
fn effects_of(file: &AepFile<'_>) -> Vec<((String, usize), Vec<Seen>)> {
    let view = effectcraft_aep::Project::new(file).expect("project");
    let mut out = Vec::new();
    for item in view.all_items() {
        let Some(comp) = item.comp() else { continue };
        let cname = item.display_name().unwrap_or_default();
        for (i, layer) in comp.layers().enumerate() {
            let mut fx = Vec::new();
            // Effects outside the table are reported by the importer, not written.
            for e in layer.effects().into_iter().filter(|e| effectcraft_aep::effect_map::by_match_name(e.match_name()).is_some()) {
                let mut params = Vec::new();
                for p in view.effect_params(&e) {
                    let keys = p
                        .keys()
                        .map(|k| {
                            k.keyframes()
                                .map(|r| {
                                    let interp = (
                                        r.interpolation_in().map_or(0, |x| match x {
                                            effectcraft_aep::Interpolation::Linear => 1,
                                            effectcraft_aep::Interpolation::Bezier => 2,
                                            effectcraft_aep::Interpolation::Hold => 3,
                                            effectcraft_aep::Interpolation::Other(c) => c,
                                        }),
                                        r.interpolation_out().map_or(0, |x| match x {
                                            effectcraft_aep::Interpolation::Linear => 1,
                                            effectcraft_aep::Interpolation::Bezier => 2,
                                            effectcraft_aep::Interpolation::Hold => 3,
                                            effectcraft_aep::Interpolation::Other(c) => c,
                                        }),
                                    );
                                    (r.time_units().unwrap_or(-1), r.value().unwrap_or_default(), interp)
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    let value = if p.is_animated() { None } else { p.stored_value() };
                    params.push((p.index, value, keys));
                }
                fx.push(Seen { match_name: e.match_name().to_string(), name: e.display_name(), params });
            }
            if !fx.is_empty() {
                out.push(((cname.clone(), i), fx));
            }
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

fn close_value(a: &Option<ParamValue>, b: &Option<ParamValue>) -> bool {
    let comps = |v: &ParamValue| -> Vec<f64> {
        match v {
            ParamValue::Number(n) => vec![*n],
            ParamValue::Point(p) => p.to_vec(),
            ParamValue::Argb(c) => c.iter().skip(1).copied().collect(),
            ParamValue::Index(i) => vec![f64::from(*i)],
            ParamValue::Bool(b) => vec![if *b { 1.0 } else { 0.0 }],
            other => panic!("unexpected {other:?}"),
        }
    };
    match (a, b) {
        (None, None) => true,
        // The input layer names the owning layer by its id, which differs between the files.
        (Some(ParamValue::Layer(_)), Some(ParamValue::Layer(_))) => true,
        (Some(x), Some(y)) => comps(x).iter().zip(comps(y)).all(|(p, q)| (p - q).abs() < 1e-6),
        _ => false,
    }
}

#[test]
fn every_effect_fixture_is_written_as_after_effects_wrote_it() {
    let names = fixtures_with_effects();
    assert!(names.len() >= 20, "{names:?}");
    let mut compared = 0;
    for name in &names {
        let bytes = fixture(name);
        let original = parse(&bytes).expect("parse");
        let mut p = Project::default();
        let none = |_: &str| false;
        let imported = import(&mut p, &bytes, &ImportOptions { allow_losses: true, ..ImportOptions::new(&none) }).expect("import");
        let lossy: Vec<String> = imported.report.losses.iter().flat_map(|l| l.details.clone()).collect();
        let r = export_all(&p);
        assert_eq!(r.report.count(LossCategory::Effects), 0, "{name}: {:?}", r.report.losses);
        let exported = parse(&r.bytes).expect("parse export");
        let (a, b) = (effects_of(&original), effects_of(&exported));
        assert_eq!(a.len(), b.len(), "{name}: layers with effects");
        for ((ka, fa), (kb, fb)) in a.iter().zip(&b) {
            assert_eq!(ka, kb, "{name}");
            assert_eq!(fa.len(), fb.len(), "{name}: effect count");
            for (x, y) in fa.iter().zip(fb) {
                assert_eq!((&x.match_name, &x.name), (&y.match_name, &y.name), "{name}");
                // Written as the importer took it over (712: CC Radial Blur's Type Scratch has no
                // EffectCraft item, reported on import).
                if lossy.iter().any(|d| d.contains(&format!("({})", x.match_name))) {
                    continue;
                }
                assert_eq!(x.params.len(), y.params.len(), "{name}: parameters of {}", x.match_name);
                for (px, py) in x.params.iter().zip(&y.params) {
                    assert_eq!(px.0, py.0, "{name}");
                    assert!(close_value(&px.1, &py.1), "{name} {} #{}: {:?} vs {:?}", x.match_name, px.0, px.1, py.1);
                    assert_eq!(px.2.len(), py.2.len(), "{name} #{} keys", px.0);
                    // A colour key switched to bezier stores speed 0 / influence 0, which After
                    // Effects interpolates linearly (598): it is imported and written linear.
                    let colour = matches!(px.2.first().map(|k| k.1.len()), Some(4));
                    let lin = |i: (u8, u8)| if colour { (i.0.min(1), i.1.min(1)) } else { i };
                    for (kx, ky) in px.2.iter().zip(&py.2) {
                        assert_eq!((kx.0, lin(kx.2)), (ky.0, lin(ky.2)), "{name} #{} key", px.0);
                        assert!(kx.1.iter().zip(&ky.1).all(|(p, q)| (p - q).abs() < 1e-6), "{name} #{} key value {:?} {:?}", px.0, kx.1, ky.1);
                    }
                }
                compared += 1;
            }
        }
        // The input layer of every exported instance names the layer that owns it (320).
        let view = effectcraft_aep::Project::new(&exported).expect("project");
        for item in view.all_items() {
            let Some(comp) = item.comp() else { continue };
            for layer in comp.layers() {
                let id = layer.header().map(|h| h.id).expect("header");
                for e in layer.effects() {
                    let input = view.effect_params(&e).into_iter().find(|p| p.index == 0).and_then(|p| p.stored_value());
                    assert_eq!(input, Some(ParamValue::Layer(id)), "{name}: input layer of {}", e.match_name());
                }
            }
        }
        // The container is consistent and the result imports to the same effect values.
        assert_eq!(effectcraft_aep::write(&exported).expect("write"), r.bytes, "{name}");
        let q = import_bytes(&r.bytes);
        assert_eq!(effect_values(&p), effect_values(&q), "{name}: values after a second import");
    }
    assert!(compared >= 20, "{compared}");
}

/// Every effect parameter of the project: (comp, layer, effect, param path, value, key count).
fn effect_values(p: &Project) -> Vec<(String, String, String, String, Vec<i64>, usize)> {
    let mut out = Vec::new();
    for item in p.items.values() {
        let ItemKind::Comp(c) = &item.kind else { continue };
        for l in &c.layers {
            let Some(fx) = l.effects() else { continue };
            for g in fx.groups() {
                let mut stack: Vec<(&PropGroup, String)> = vec![(g, String::new())];
                while let Some((grp, pre)) = stack.pop() {
                    for n in &grp.children {
                        match n {
                            Node::Group(s) => stack.push((s, format!("{pre}{}/", s.match_id))),
                            Node::Prop(pr) => out.push((
                                item.name.clone(),
                                l.name.clone(),
                                g.name.clone(),
                                format!("{pre}{}", pr.match_id),
                                pr.value.components().iter().map(|v| (v * 1e6).round() as i64).collect(),
                                pr.keys.len(),
                            )),
                        }
                    }
                }
            }
        }
    }
    out.sort();
    out
}

/// The Tint of 356 imported, edited by `f`.
fn tint(f: impl FnOnce(&mut PropGroup)) -> Project {
    let mut p = import_bytes(&fixture("356-fx-tint-default.aep"));
    let id = p.items.values().find(|i| i.name == "C").map(|i| i.id).expect("C");
    with_layer(&mut p, id, 0, |l| {
        let g = l.props.sub_mut("effects").and_then(|e| e.children.iter_mut().find_map(|n| if let Node::Group(g) = n { Some(g) } else { None })).expect("tint");
        f(g);
    });
    p
}

#[test]
fn edited_parameters_and_keys_no_fixture_has() {
    let p = tint(|g| {
        g.get_mut("black").expect("black").value = Value::Color([0.25, 0.5, 0.75, 1.0]);
        g.get_mut("white").expect("white").keys = vec![key(0.0, Value::Color([1.0, 0.0, 0.0, 1.0])), key(1.0, Value::Color([0.0, 0.0, 1.0, 1.0]))];
        // A keyed property shows its first key's value.
        g.get_mut("white").expect("white").value = Value::Color([1.0, 0.0, 0.0, 1.0]);
        g.get_mut("amount").expect("amount").value = Value::Scalar(10.0);
        let mut k0 = key(0.0, Value::Scalar(10.0));
        k0.out_interp = Interp::Hold;
        g.get_mut("amount").expect("amount").keys = vec![k0, key(2.0, Value::Scalar(90.0))];
        g.name = "My Tint".into();
    });
    let r = export_all(&p);
    assert_eq!(r.report.exported.effects, 1);
    assert_eq!(r.report.count(LossCategory::Effects), 0, "{:?}", r.report.losses);
    let file = parse(&r.bytes).expect("parse");
    let seen = effects_of(&file);
    let fx = &seen.first().expect("layer").1[0];
    assert_eq!(fx.name.as_deref(), Some("My Tint"));
    // As After Effects stores a renamed instance (610): fnam keeps the effect's name.
    let view = effectcraft_aep::Project::new(&file).expect("project");
    let e = view.all_items().into_iter().find_map(|i| i.comp()).and_then(|c| c.layers().next()).map(|l| l.effects()).expect("effects");
    assert_eq!((e[0].name().as_deref(), e[0].user_name().as_deref()), (Some("Tint"), Some("My Tint")));
    let param = |i: usize| fx.params.iter().find(|p| p.0 == i).expect("param");
    match &param(1).1 {
        Some(ParamValue::Argb(c)) => assert!((c[1] - 0.25).abs() < 1e-6 && (c[2] - 0.5).abs() < 1e-6 && (c[3] - 0.75).abs() < 1e-6, "{c:?}"),
        other => panic!("{other:?}"),
    }
    let white = &param(2).2;
    assert_eq!(white.len(), 2);
    assert_eq!(white[1].0, 25_600);
    assert_eq!(white[1].1, vec![255.0, 0.0, 0.0, 255.0]);
    let amount = &param(3).2;
    assert_eq!(amount.iter().map(|k| (k.0, k.1[0], k.2)).collect::<Vec<_>>(), vec![(0, 10.0, (1, 3)), (51_200, 90.0, (1, 1))]);
    // And back.
    let q = import_bytes(&r.bytes);
    assert_eq!(effect_values(&p).into_iter().filter(|v| v.2 == "My Tint").count(), 3);
    assert_eq!(
        effect_values(&p)
            .into_iter()
            .map(|mut v| {
                v.1.clear();
                v
            })
            .collect::<Vec<_>>(),
        effect_values(&q)
            .into_iter()
            .map(|mut v| {
                v.1.clear();
                v
            })
            .collect::<Vec<_>>()
    );
}

#[test]
fn several_instances_share_one_definition_copy() {
    // Two Tints and a Ramp on one layer, a Ramp on a second layer.
    let mut p = import_bytes(&fixture("383-fx-ramp-on-100x100-start-10-20.aep"));
    let tint_src = import_bytes(&fixture("356-fx-tint-default.aep"));
    let tint_group = tint_src
        .items
        .values()
        .find_map(|i| if let ItemKind::Comp(c) = &i.kind { c.layers.first().and_then(|l| l.effects()).and_then(|e| e.groups().next().cloned()) } else { None })
        .expect("tint");
    let c = p.items.values().find(|i| i.name == "C").map(|i| i.id).expect("C");
    with_layer(&mut p, c, 0, |l| {
        let fx = l.props.sub_mut("effects").expect("effects");
        fx.children.push(Node::Group(tint_group.clone()));
        fx.children.push(Node::Group(tint_group));
    });
    let r = export_all(&p);
    assert_eq!(r.report.exported.effects, 4, "{:?}", r.report.losses);
    let file = parse(&r.bytes).expect("parse");
    let view = effectcraft_aep::Project::new(&file).expect("project");
    let mut full = Vec::new();
    for item in view.all_items() {
        let Some(comp) = item.comp() else { continue };
        for layer in comp.layers() {
            for e in layer.effects() {
                full.push((e.match_name().to_string(), e.has_own_definitions()));
            }
        }
    }
    // The first instance of each type in the file holds the definitions.
    assert_eq!(full.iter().filter(|(_, own)| *own).count(), 2, "{full:?}");
    assert_eq!(full.len(), 4);
    let defs: Vec<String> = view.effect_definitions().into_iter().map(|(n, _)| n).collect();
    assert_eq!(defs, vec!["ADBE Ramp".to_string(), "ADBE Tint".to_string()]);
    let pefl: Vec<String> =
        file.list(b"Pefl").expect("Pefl").children().iter().filter_map(|c| c.bytes()).map(|b| String::from_utf8_lossy(b).to_string()).collect();
    assert_eq!(pefl, vec!["ADBE Ramp".to_string(), "ADBE Tint".to_string()]);
    let efdc = file.list(b"EfdG").and_then(|g| g.child(b"EfDC")).and_then(|c| c.bytes()).expect("EfDC");
    assert_eq!(efdc, &2u32.to_le_bytes());
    // Every instance reads its parameters (borrowed definitions included) and imports again.
    let q = import_bytes(&r.bytes);
    assert_eq!(effect_values(&q).iter().map(|v| v.2.clone()).collect::<std::collections::BTreeSet<_>>().len(), 2);
    let n: usize = q
        .items
        .values()
        .filter_map(|i| {
            if let ItemKind::Comp(c) = &i.kind { Some(c.layers.iter().filter_map(|l| l.effects()).map(|e| e.groups().count()).sum::<usize>()) } else { None }
        })
        .sum();
    assert_eq!(n, 4);
}

#[test]
fn what_cannot_be_written_is_reported() {
    let p = tint(|g| {
        g.enabled = false;
        g.get_mut("amount").expect("amount").expr = Some(effectcraft_project::Expression { text: "50".into(), enabled: true });
    });
    let r = export_all(&p);
    // The switch is not decoded; the expression is written (534).
    assert_eq!(r.report.count(LossCategory::Effects), 1, "{:?}", r.report.losses);
    // An effect outside the table.
    let mut q = import_bytes(&fixture("356-fx-tint-default.aep"));
    let c = q.items.values().find(|i| i.name == "C").map(|i| i.id).expect("C");
    with_layer(&mut q, c, 0, |l| {
        if let Some(Node::Group(g)) = l.props.sub_mut("effects").and_then(|e| e.children.first_mut()) {
            g.kind = effectcraft_project::GroupKind::Effect { effect: "ec.distort.notmapped".into() };
        }
    });
    let r = export_all(&q);
    assert_eq!(r.report.count(LossCategory::Effects), 1);
    assert_eq!(r.report.exported.effects, 0);
    parse(&r.bytes).expect("still a valid file");
}

#[test]
fn hostile_effect_values_never_panic() {
    for v in [Value::Scalar(f64::NAN), Value::Scalar(1e300), Value::Color([f32::NAN as f64, 2.0, -1.0, 1.0]), Value::Bool(true), Value::Text(Box::default())] {
        let p = tint(|g| {
            g.get_mut("black").expect("black").value = v.clone();
            g.get_mut("amount").expect("amount").value = v.clone();
            g.get_mut("white").expect("white").keys = vec![key(0.0, v.clone()), key(0.0, v.clone()), key(1e12, v.clone())];
        });
        let r = export_all(&p);
        let file = parse(&r.bytes).expect("parse");
        assert_eq!(effectcraft_aep::write(&file).expect("write"), r.bytes);
        let _ = import_bytes(&r.bytes);
    }
}

/// The static value records (`tdbs` with a `cdat`) of the mapped parameters of every effect
/// instance, by (composition, layer index, effect index, record name).
fn static_records(file: &AepFile<'_>) -> std::collections::BTreeMap<(String, usize, usize, String), effectcraft_aep::Chunk<'static>> {
    let view = effectcraft_aep::Project::new(file).expect("project");
    let mut out = std::collections::BTreeMap::new();
    for item in view.all_items() {
        let Some(comp) = item.comp() else { continue };
        let cname = item.display_name().unwrap_or_default();
        for (li, layer) in comp.layers().enumerate() {
            for (ei, e) in layer.effects().into_iter().enumerate() {
                let Some(spec) = effectcraft_aep::effect_map::by_match_name(e.match_name()) else { continue };
                let Some(values) = e.chunk().child_list(b"tdgp") else { continue };
                let k = values.children();
                for (i, c) in k.iter().enumerate() {
                    let Some(name) =
                        c.bytes().filter(|_| c.is(b"tdmn")).map(|b| String::from_utf8_lossy(b.split(|x| *x == 0).next().unwrap_or(b"")).to_string())
                    else {
                        continue;
                    };
                    let Some(index) = name.rsplit('-').next().and_then(|s| s.parse::<usize>().ok()) else { continue };
                    if !spec.params.iter().any(|p| p.index == index) {
                        continue;
                    }
                    if let Some(rec) = k.get(i + 1).filter(|r| r.is_list(b"tdbs") && r.child(b"cdat").is_some()) {
                        let mut rec = rec.clone().into_owned();
                        // A layer parameter's `tdpi` is a layer id, which differs between the
                        // files (`layer_parameters_name_the_same_layers` checks the layers).
                        if let effectcraft_aep::Body::List { children, .. } = &mut rec.body {
                            for c in children.iter_mut().filter(|c| c.is(b"tdpi")) {
                                *c = effectcraft_aep::Chunk::data(*b"tdpi", vec![0; 4]);
                            }
                        }
                        out.insert((cname.clone(), li, ei, name), rec);
                    }
                }
            }
        }
    }
    out
}

#[test]
fn every_static_value_record_is_rebuilt_byte_for_byte() {
    // The records are built from one record per parameter type, the parameter's name and its
    // slider range (R45): every static record After Effects wrote for a mapped parameter of an
    // effect fixture comes back with the same bytes.
    let mut compared = 0;
    for name in fixtures_with_effects() {
        let bytes = fixture(&name);
        let original = parse(&bytes).expect("parse");
        let mut p = Project::default();
        let none = |_: &str| false;
        let imported = import(&mut p, &bytes, &ImportOptions { allow_losses: true, ..ImportOptions::new(&none) }).expect("import");
        // An effect the importer could not take over completely (712: CC Radial Blur's Type
        // Scratch has no EffectCraft item) is written as EffectCraft has it.
        let lossy: Vec<String> = imported.report.losses.iter().flat_map(|l| l.details.clone()).collect();
        let r = export_all(&p);
        let exported = parse(&r.bytes).expect("parse export");
        let skip = |k: &(String, usize, usize, String)| lossy.iter().any(|d| d.contains(k.3.rsplit_once('-').map_or("", |x| x.0)));
        let (mut a, mut b) = (static_records(&original), static_records(&exported));
        a.retain(|k, _| !skip(k));
        b.retain(|k, _| !skip(k));
        for (key, rec) in &a {
            let got = b.get(key).unwrap_or_else(|| panic!("{name}: {key:?} not written"));
            assert_eq!(got, rec, "{name}: {key:?}");
            compared += 1;
        }
        for key in b.keys() {
            assert!(a.contains_key(key), "{name}: {key:?} written but After Effects has none");
        }
    }
    assert!(compared >= 60, "{compared}");
}

#[test]
fn hue_saturation_and_levels_data_is_written_as_after_effects_wrote_it() {
    // Master Hue / Lightness (444, 446) and Levels' RGB input black, gamma, output white (512,
    // 514, 516) live in the effect's arbitrary data: the exported `aRbp` equals After Effects'.
    for (name, param) in [
        ("444-fx-huesat-hue-90.aep", "ADBE HUE SATURATION-0003"),
        ("446-fx-huesat-light-m30.aep", "ADBE HUE SATURATION-0003"),
        ("512-fx-levels-inblack-0.1.aep", "ADBE Easy Levels2-0002"),
        ("514-fx-levels-gamma-2.aep", "ADBE Easy Levels2-0002"),
        ("516-fx-levels-outwhite-0.9.aep", "ADBE Easy Levels2-0002"),
    ] {
        let bytes = fixture(name);
        let data = |b: &[u8]| -> Vec<u8> {
            let f = parse(b).expect("parse");
            let view = effectcraft_aep::Project::new(&f).expect("project");
            let mut out = Vec::new();
            for item in view.all_items() {
                let Some(comp) = item.comp() else { continue };
                for layer in comp.layers() {
                    for e in layer.effects() {
                        if let Some(d) = e.arbitrary_data(param) {
                            out = d.to_vec();
                        }
                    }
                }
            }
            out
        };
        let r = export_all(&import_bytes(&bytes));
        assert!(!data(&bytes).is_empty(), "{name}");
        assert_eq!(data(&r.bytes), data(&bytes), "{name}");
    }
}

/// The layer each layer parameter names, by layer index (comp, layer, effect,
/// parameter index, named layer or "None").
fn layer_params(file: &AepFile<'_>) -> Vec<(String, String, String, usize, String)> {
    let view = effectcraft_aep::Project::new(file).expect("project");
    let mut out = Vec::new();
    for item in view.all_items() {
        let Some(comp) = item.comp() else { continue };
        let cname = item.display_name().unwrap_or_default();
        let names: std::collections::HashMap<u32, String> = comp.layers().enumerate().filter_map(|(i, l)| Some((l.header()?.id, format!("#{i}")))).collect();
        for (i, layer) in comp.layers().enumerate() {
            let lname = format!("#{i}");
            for e in layer.effects() {
                for p in view.effect_params(&e).into_iter().filter(|p| p.index > 0 && p.kind == effectcraft_aep::ParamKind::Layer) {
                    let named = match p.stored_value() {
                        Some(ParamValue::Layer(0)) => "None".to_string(),
                        Some(ParamValue::Layer(id)) => names.get(&id).cloned().unwrap_or_else(|| format!("? {id}")),
                        other => format!("{other:?}"),
                    };
                    out.push((cname.clone(), lname.clone(), e.match_name().to_string(), p.index, named));
                }
            }
        }
    }
    out.sort();
    out
}

#[test]
fn layer_parameters_name_the_same_layers() {
    // 905: Compound Blur, Displacement Map and Set Matte name the grey solid R (904 E22-E24);
    // 220 / 320 / 321: Layer Control on another layer, on itself, None.
    let mut checked = 0;
    for name in
        ["905-fx-24-effects-set-one-comp.aep", "220-fx-layercontrol-to-layer1.aep", "320-fx-layercontrol-self-explicit.aep", "321-fx-layercontrol-none.aep"]
    {
        let bytes = fixture(name);
        let original = parse(&bytes).expect("parse");
        let p = import_bytes(&bytes);
        let r = export_all(&p);
        assert_eq!(r.report.count(LossCategory::Effects), 0, "{name}: {:?}", r.report.losses);
        let exported = parse(&r.bytes).expect("parse export");
        let (a, b) = (layer_params(&original), layer_params(&exported));
        assert!(!a.is_empty(), "{name}");
        assert_eq!(a, b, "{name}");
        checked += 1;
    }
    assert_eq!(checked, 4);
}

/// Sets every property with this match id (in every effect of every layer) to `v`.
fn set_all(p: &mut Project, id: &str, v: Value) -> usize {
    fn walk(g: &mut PropGroup, id: &str, v: &Value, n: &mut usize) {
        for c in &mut g.children {
            match c {
                Node::Prop(pr) if pr.match_id == id => {
                    pr.value = v.clone();
                    *n += 1;
                }
                Node::Group(sub) => walk(sub, id, v, n),
                _ => {}
            }
        }
    }
    let mut n = 0;
    let ids: Vec<_> = p.comps().map(|(i, _)| *i).collect();
    for cid in ids {
        let Some(mut c) = p.comp(cid).cloned() else { continue };
        for l in &mut c.layers {
            walk(&mut l.props, id, &v, &mut n);
        }
        if let Some(it) = p.item_mut(cid) {
            it.kind = ItemKind::Comp(std::sync::Arc::new(c));
        }
    }
    n
}

fn find_id<'a>(g: &'a PropGroup, id: &str) -> Option<&'a effectcraft_project::Property> {
    g.children.iter().find_map(|c| match c {
        Node::Prop(pr) if pr.match_id == id => Some(pr),
        Node::Group(sub) => find_id(sub, id),
        _ => None,
    })
}

#[test]
fn an_explicit_none_is_written_as_none() {
    // After Effects' None on Compound Blur / Displacement Map / Set Matte (`tdpi` 0) differs from
    // the owning layer (fx27 `layer-none`); it is EffectCraft's `NO_LAYER` both ways.
    let mut p = import_bytes(&fixture("905-fx-24-effects-set-one-comp.aep"));
    let none = Value::Layer(Some(effectcraft_project::NO_LAYER));
    let n: usize = ["blurLayer", "displacementMapLayer", "takeMatteFromLayer"].iter().map(|id| set_all(&mut p, id, none.clone())).sum();
    assert_eq!(n, 3);
    let r = export_all(&p);
    assert_eq!(r.report.count(LossCategory::Effects), 0, "{:?}", r.report.losses);
    let named: Vec<_> = layer_params(&parse(&r.bytes).expect("parse")).into_iter().filter(|x| x.2 != "ADBE Layer Control").map(|x| x.4).collect();
    assert_eq!(named, ["None", "None", "None"]);
    let q = import_bytes(&r.bytes);
    let mut back = 0;
    for (_, c) in q.comps() {
        for l in &c.layers {
            for id in ["blurLayer", "displacementMapLayer", "takeMatteFromLayer"] {
                if let Some(pr) = find_id(&l.props, id) {
                    assert_eq!(pr.value, none, "{id}");
                    back += 1;
                }
            }
        }
    }
    assert_eq!(back, 3);
}

#[test]
fn layer_control_on_a_named_layer_needs_no_source_choice() {
    // Layer Control has no source choice in After Effects: a companion other than Source (a
    // Layer Control made here defaults to Effects & Masks) is no loss.
    let mut p = import_bytes(&fixture("220-fx-layercontrol-to-layer1.aep"));
    assert_eq!(set_all(&mut p, "layerSource", Value::Enum(2)), 1);
    let r = export_all(&p);
    assert_eq!(r.report.count(LossCategory::Effects), 0, "{:?}", r.report.losses);
}

#[test]
fn unmapped_layer_parameters_keep_their_default() {
    // Timewarp's Matte Layer and Warp Layer are not in the table; their default is None (`tdpi`
    // 0), which the export keeps: the re-import reports nothing (529-533). Written as the owning
    // layer, the re-import reported both as "not imported".
    for name in ["529-fx-timewarp-default.aep", "531-fx-timewarp-speed-keyed-50-200.aep"] {
        let p = import_bytes(&fixture(name));
        let r = export_all(&p);
        let mut q = Project::default();
        let none = |_: &str| false;
        let back = import(&mut q, &r.bytes, &ImportOptions { allow_losses: true, ..ImportOptions::new(&none) }).expect("re-import");
        assert_eq!(back.report.count(effectcraft_aep_import::LossCategory::Effects), 0, "{name}: {:?}", back.report.losses);
    }
}

/// The arbitrary data of the first instance of `match_name` in a file.
fn arb(bytes: &[u8], match_name: &str, record: &str) -> Vec<u8> {
    let file = parse(bytes).expect("parse");
    let view = effectcraft_aep::Project::new(&file).expect("project");
    let comp = view.all_items().into_iter().find_map(|i| i.comp()).expect("comp");
    let fx = comp.layers().flat_map(|l| l.effects()).find(|e| e.match_name() == match_name).expect("effect");
    fx.arbitrary_data(record).expect("data").to_vec()
}

#[test]
fn curves_and_mesh_warp_are_written_as_after_effects_stores_them() {
    // Curves: the points and the tables rebuilt from them are After Effects' bytes (963, 964).
    for name in ["963-curves-red-mid-down-on-S.aep", "964-curves-rgb-s-curve-on-S.aep"] {
        let bytes = fixture(name);
        let r = export_all(&import_bytes(&bytes));
        assert_eq!(r.report.count(LossCategory::Effects), 0, "{name}: {:?}", r.report.losses);
        assert_eq!(arb(&r.bytes, "ADBE CurvesCustom", "ADBE CurvesCustom-0001"), arb(&bytes, "ADBE CurvesCustom", "ADBE CurvesCustom-0001"), "{name}");
    }
    // Mesh Warp (985): the same grid, every coordinate within 1e-5 of the layer.
    let bytes = fixture("985-meshwarp-3x3-vertex-moved-on-S.aep");
    let r = export_all(&import_bytes(&bytes));
    assert_eq!(r.report.count(LossCategory::Effects), 0, "{:?}", r.report.losses);
    let (a, b) = (arb(&r.bytes, "ADBE MESH WARP", "ADBE MESH WARP-0004"), arb(&bytes, "ADBE MESH WARP", "ADBE MESH WARP-0004"));
    let (ma, mb) = (effectcraft_aep::MeshData::parse(&a).expect("written"), effectcraft_aep::MeshData::parse(&b).expect("stored"));
    assert_eq!((ma.columns, ma.rows, ma.tail), (mb.columns, mb.rows, mb.tail));
    for (x, y) in ma.vertices.iter().zip(&mb.vertices) {
        for (p, q) in x.iter().zip(y) {
            assert!((p[0] - q[0]).abs() < 1e-5 && (p[1] - q[1]).abs() < 1e-5, "{p:?} vs {q:?}");
        }
    }
}

#[test]
fn mesh_handles_moved_on_their_own_survive_the_round_trip() {
    // 985 imported, then the right handle of vertex (2, 2) moved by 30 px on its own: the importer
    // keeps such moves as extra numbers of the vertex (EffectCraft draws a bilinear mesh).
    let mut p = import_bytes(&fixture("985-meshwarp-3x3-vertex-moved-on-S.aep"));
    let id: Vec<_> = p.comps().map(|(i, _)| *i).collect();
    let mut tokens = Vec::new();
    for cid in id {
        let Some(mut c) = p.comp(cid).cloned() else { continue };
        for l in &mut c.layers {
            fn walk(g: &mut PropGroup, tokens: &mut Vec<String>) {
                for n in &mut g.children {
                    match n {
                        Node::Prop(pr) if pr.match_id == "mesh" => {
                            if let Value::Str(s) = &pr.value {
                                let mut t: Vec<String> = s.split(' ').map(str::to_string).collect();
                                t[10] = format!("{},0,0,30,0,0,0,0,0", t[10]);
                                *tokens = t.clone();
                                pr.value = Value::Str(t.join(" "));
                            }
                        }
                        Node::Group(sub) => walk(sub, tokens),
                        _ => {}
                    }
                }
            }
            walk(&mut l.props, &mut tokens);
        }
        if let Some(it) = p.item_mut(cid) {
            it.kind = ItemKind::Comp(std::sync::Arc::new(c));
        }
    }
    assert_eq!(tokens.len(), 16);
    let r = export_all(&p);
    let data = arb(&r.bytes, "ADBE MESH WARP", "ADBE MESH WARP-0004");
    let m = effectcraft_aep::MeshData::parse(&data).expect("mesh");
    let d = effectcraft_aep::MeshData::default_vertex(3, 3, 2, 2);
    let v = m.vertices[10];
    // The right handle: default + the vertex's move + 30 px.
    assert!((v[2][0] - (d[2][0] + (132.0 + 30.0) / 1920.0)).abs() < 1e-5, "{:?}", v[2]);
    assert!((v[1][0] - (d[1][0] + 132.0 / 1920.0)).abs() < 1e-5, "{:?}", v[1]);
    // Re-imported, the vertex keeps the handle move.
    let q = import_bytes(&r.bytes);
    let mut found = false;
    for (_, c) in q.comps() {
        for l in &c.layers {
            if let Some(Value::Str(s)) = find_id(&l.props, "mesh").map(|p| p.value.clone()) {
                let t: Vec<&str> = s.split(' ').collect();
                let n: Vec<f64> = t[10].split(',').map(|x| x.parse().expect("number")).collect();
                assert_eq!(n.len(), 10, "{}", t[10]);
                assert!((n[4] - 30.0).abs() < 0.01 && (n[0] - 132.0).abs() < 0.01, "{n:?}");
                found = true;
            }
        }
    }
    assert!(found);
}
