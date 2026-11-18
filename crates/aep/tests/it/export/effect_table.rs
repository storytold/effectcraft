//! Effects written from the effect table without a corpus fixture: a new EffectCraft instance with
//! every mapped parameter changed is exported, and the export must carry the effect without a loss
//! and import back to the same values. Built in code, so these run without the corpus.

use crate::export::helpers::*;
use effectcraft_aep::container::effect_map::{Conv, by_effect};
use effectcraft_aep::export::LossCategory;
use effectcraft_aep::import::{ImportOptions, import};
use effectcraft_aep::{ParamKind, parse};
use effectcraft_keyframe::Value;
use effectcraft_project::{Node, ParamUi, Project, PropGroup, build};

/// HDR Highlight Compression, Echo, Linear Wipe, Offset and Color Stabilizer: imported since
/// #3110, written since their definitions were added to the export table.
const IMPORT_TABLE_EFFECTS: &[&str] =
    &["ec.utility.hdrcompression", "ec.time.echo", "ec.transition.linearwipe", "ec.distort.offset", "ec.color.colorstabilizer"];

/// Effects mapped in both directions from After Effects 26.5 projects (defaults and every parameter
/// changed).
const MAPPED_FROM_SAMPLES: &[&str] = &[
    "ec.distort.bulge",
    "ec.distort.twirl",
    "ec.distort.spherize",
    "ec.stylize.mosaic",
    "ec.stylize.posterize",
    "ec.stylize.threshold",
    "ec.stylize.findedges",
    "ec.distort.mirror",
    "ec.color.vibrance",
];

/// Every effect these tests cover.
fn covered() -> impl Iterator<Item = &'static &'static str> {
    IMPORT_TABLE_EFFECTS.iter().chain(MAPPED_FROM_SAMPLES)
}

/// A value other than `v` for a parameter of `ui` mapped as `conv` / `kind`, inside both
/// programs' ranges.
fn changed(v: &Value, ui: &ParamUi, conv: Conv, kind: ParamKind) -> Option<Value> {
    Some(match (conv, v) {
        (Conv::Color, _) => Value::Color([0.2, 0.4, 0.6, 1.0]),
        (Conv::Point, Value::Vec2(p)) => Value::Vec2([p[0] * 0.5 + 100.0, p[1] * 0.5 + 50.0]),
        (Conv::Angle, Value::Scalar(a)) => Value::Scalar(a + 33.0),
        (Conv::Checkbox, Value::Bool(b)) => Value::Bool(!b),
        (Conv::Popup { .. }, Value::Enum(i)) => {
            let ParamUi::Popup { options } = ui else { return None };
            // The next item that is not a separator.
            let n = options.len();
            let next = (1..n).map(|d| (*i as usize + d) % n).find(|&j| options.get(j).is_some_and(|o| o != "(-"))?;
            Value::Enum(u32::try_from(next).ok()?)
        }
        (Conv::Percent { .. } | Conv::Plain { .. } | Conv::Factor { .. }, Value::Scalar(x)) => {
            let ParamUi::Slider { slider_min, slider_max, .. } = ui else { return None };
            let mut y = slider_min + (slider_max - slider_min) * 0.37;
            if kind == ParamKind::IntSlider {
                y = y.round();
            }
            if (y - x).abs() < 1e-9 {
                y = if kind == ParamKind::IntSlider { y + 1.0 } else { y + (slider_max - slider_min) * 0.1 };
            }
            Value::Scalar(y)
        }
        _ => return None,
    })
}

/// `simple()` with a new instance of `id` whose mapped parameters are all changed; the changed
/// values by parameter id.
fn changed_instance(id: &str) -> (Project, Vec<(String, Value)>) {
    let (mut p, c, _) = simple();
    let spec = effectcraft_effects::find(id).unwrap_or_else(|| panic!("{id}: no such effect"));
    let map = by_effect(id).unwrap_or_else(|| panic!("{id}: not in the effect table"));
    let mut next = p.next_id;
    let mut g = effectcraft_effects::instantiate(spec, &mut build::Ids(&mut next), spec.name, [1920.0, 1080.0]);
    p.next_id = next;
    let mut set = Vec::new();
    for pm in map.params {
        let Some(ps) = spec.params.iter().find(|ps| ps.id == pm.target) else { continue };
        let Some(prop) = g.prop_mut(pm.target) else { continue };
        if let Some(v) = changed(&prop.value, &ps.ui, pm.conv, pm.kind) {
            prop.value = v.clone();
            set.push((pm.target.to_string(), v));
        }
    }
    assert!(!set.is_empty(), "{id}: no parameter changed");
    with_layer(&mut p, c, 0, |l| l.props.sub_mut("effects").expect("effects").children.push(Node::Group(g)));
    (p, set)
}

/// The first effect of the first layer of the first composition of `p`.
fn first_effect(p: &Project) -> Option<&PropGroup> {
    p.comps().flat_map(|(_, c)| c.layers.iter()).filter_map(|l| l.props.sub("effects")).flat_map(|e| e.children.iter()).find_map(|n| match n {
        Node::Group(g) => Some(g),
        _ => None,
    })
}

/// Equal up to the precision the file stores (16.16 fixed, float32).
fn close(a: &Value, b: &Value) -> bool {
    let (x, y) = (a.components(), b.components());
    x.len() == y.len() && x.iter().zip(&y).all(|(p, q)| (p - q).abs() <= 1e-3_f64.max(q.abs() * 1e-5))
}

#[test]
fn import_table_effects_are_written_and_read_back() {
    for id in covered() {
        let (p, set) = changed_instance(id);
        let r = export_all(&p);
        assert_eq!(r.report.exported.effects, 1, "{id}: {:?}", r.report.losses);
        assert_eq!(r.report.count(LossCategory::Effects), 0, "{id}: {:?}", r.report.losses);
        let file = parse(&r.bytes).expect("parse");
        assert_eq!(effectcraft_aep::write(&file).expect("write"), r.bytes, "{id}: the file writes back byte for byte");
        let mut q = Project::default();
        let none = |_: &str| false;
        import(&mut q, &r.bytes, &ImportOptions { allow_losses: true, ..ImportOptions::new(&none) }).expect("import");
        let g = first_effect(&q).unwrap_or_else(|| panic!("{id}: no effect after the round trip"));
        assert_eq!(g.kind, effectcraft_project::GroupKind::Effect { effect: (*id).to_string() });
        for (target, want) in &set {
            let got = &g.prop(target).unwrap_or_else(|| panic!("{id} {target}: missing")).value;
            assert!(close(got, want), "{id} {target}: wrote {want:?}, read back {got:?}");
        }
    }
}

/// A new instance (EffectCraft's defaults, which are not always After Effects': Color Stabilizer
/// starts at Brightness with its own sample points) comes back with the same values.
#[test]
fn import_table_effects_at_their_defaults_read_back() {
    for id in covered() {
        let (mut p, c, _) = simple();
        let spec = effectcraft_effects::find(id).expect("effect");
        let mut next = p.next_id;
        let g = effectcraft_effects::instantiate(spec, &mut build::Ids(&mut next), spec.name, [1920.0, 1080.0]);
        p.next_id = next;
        let before: Vec<(String, Value)> =
            by_effect(id).expect("mapped").params.iter().filter_map(|pm| g.prop(pm.target).map(|q| (pm.target.to_string(), q.value.clone()))).collect();
        with_layer(&mut p, c, 0, |l| l.props.sub_mut("effects").expect("effects").children.push(Node::Group(g)));
        let r = export_all(&p);
        assert_eq!(r.report.exported.effects, 1, "{id}: {:?}", r.report.losses);
        assert_eq!(r.report.count(LossCategory::Effects), 0, "{id}: {:?}", r.report.losses);
        let mut q = Project::default();
        let none = |_: &str| false;
        import(&mut q, &r.bytes, &ImportOptions { allow_losses: true, ..ImportOptions::new(&none) }).expect("import");
        let back = first_effect(&q).unwrap_or_else(|| panic!("{id}: no effect after the round trip"));
        for (target, want) in &before {
            let got = &back.prop(target).unwrap_or_else(|| panic!("{id} {target}: missing")).value;
            assert!(close(got, want), "{id} {target}: wrote {want:?}, read back {got:?}");
        }
    }
}

/// Color Stabilizer's instance carries the sequence data After Effects needs to render it (without
/// it After Effects 26.5 crashed rendering the exported layer); other effects carry none.
#[test]
fn color_stabilizer_carries_its_sequence_data() {
    for (id, want) in [("ec.color.colorstabilizer", Some(vec![0xde, 0xf1, 0x1c, 0xe4, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1])), ("ec.transition.linearwipe", None)]
    {
        let (p, _) = changed_instance(id);
        let r = export_all(&p);
        let file = parse(&r.bytes).expect("parse");
        let view = effectcraft_aep::Project::new(&file).expect("project");
        let fx = view.all_items().into_iter().find_map(|i| i.comp()).and_then(|c| c.layers().next()).map(|l| l.effects()).expect("effects");
        let e = fx.first().expect("one effect");
        let sdat = e.chunk().child(b"sdat").and_then(|c| c.bytes()).map(<[u8]>::to_vec);
        assert_eq!(sdat, want, "{id}");
    }
}
