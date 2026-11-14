//! #3087: an effect with no EffectCraft counterpart (a third-party plug-in, a pseudo effect of
//! expression controls) arrives as a placeholder holding its parameters, so expressions that read
//! them keep working; it renders nothing and is still reported. Each effect is built into
//! `synthetic.aep` (`fx_inject`) in place of its Glow, on the 8 x 8 sequence layer.

use crate::import::fx_inject::{Default, Fx, bytes_with, import_bytes, losses, other, popup, run, slider, v};
use effectcraft_aep::import::LossCategory;
use effectcraft_aep::{Body, Chunk, ParamKind as K, parse, write};
use effectcraft_keyframe::Value;
use effectcraft_project::{GroupKind, ParamUi, PropGroup};

/// A third-party effect with one parameter of each kept kind, inside a twirl-down group.
fn third_party(values: Vec<(usize, f64)>, keys: Vec<(usize, Vec<(i32, f64)>)>) -> Fx {
    Fx {
        match_name: "CC Cylinder",
        name: "CC Cylinder",
        defs: vec![
            other(K::GroupStart, "Light", Default::None),
            slider(K::FloatSlider, "Radius (%)", 30.0, (0.0, 30000.0)),
            other(K::GroupEnd, "", Default::None),
            other(K::Angle, "Rotation X", Default::Number(12.5)),
            other(K::Checkbox, "Wire", Default::Number(1.0)),
            other(K::Color, "Light Color", Default::Color([255, 51, 102, 204])),
            other(K::Point, "Position", Default::Point(0.25, 0.5)),
            popup("Render", 1, &["Full", "(-", "Outside", "Inside"]),
            slider(K::IntSlider, "Count", 3.0, (0.0, 50.0)),
        ],
        values,
        data: vec![],
        keys,
    }
}

fn placeholder_of(fx: &Fx) -> (PropGroup, effectcraft_aep::import::ImportResult) {
    let (g, r) = run(fx, fx.match_name);
    (g.expect("the placeholder"), r)
}

#[test]
fn an_effect_without_a_counterpart_is_kept_with_its_values() {
    // Radius 37.5 and Render "Outside" (3: the separator counts) stored, the rest at defaults.
    let (g, r) = placeholder_of(&third_party(vec![(2, 37.5), (8, 3.0)], vec![]));
    assert_eq!(g.kind, GroupKind::Effect { effect: "CC Cylinder".into() });
    assert_eq!(g.name, "CC Cylinder");
    let names: Vec<(&str, &str)> = g.props().map(|p| (p.match_id.as_str(), p.name.as_str())).collect();
    assert_eq!(
        names,
        [
            ("CC Cylinder-0002", "Radius (%)"),
            ("CC Cylinder-0004", "Rotation X"),
            ("CC Cylinder-0005", "Wire"),
            ("CC Cylinder-0006", "Light Color"),
            ("CC Cylinder-0007", "Position"),
            ("CC Cylinder-0008", "Render"),
            ("CC Cylinder-0009", "Count"),
        ],
        "flat, in order, without the group markers"
    );
    assert_eq!(v(&g, "CC Cylinder-0002"), Value::Scalar(37.5));
    assert_eq!(v(&g, "CC Cylinder-0004"), Value::Scalar(12.5));
    assert_eq!(v(&g, "CC Cylinder-0005"), Value::Bool(true));
    let Value::Color(c) = v(&g, "CC Cylinder-0006") else { panic!("a colour") };
    assert!(c.iter().zip([0.2, 0.4, 0.8, 1.0]).all(|(a, b)| (a - b).abs() < 1e-6), "{c:?}");
    assert_eq!(v(&g, "CC Cylinder-0007"), Value::Vec2([2.0, 4.0]), "layer pixels");
    assert_eq!(v(&g, "CC Cylinder-0008"), Value::Enum(2), "item 3 of 4, 0-based");
    assert_eq!(v(&g, "CC Cylinder-0009"), Value::Scalar(3.0));
    let render = g.prop("CC Cylinder-0008").unwrap();
    assert_eq!(render.ui, ParamUi::Popup { options: ["Full", "(-", "Outside", "Inside"].map(String::from).to_vec() });
    // Still reported, once, and counted apart from the imported effects.
    let found = losses(&r, LossCategory::Effects, "CC Cylinder");
    assert!(found.len() == 1 && found[0].ends_with("has the effect 'CC Cylinder' (CC Cylinder), kept without rendering"), "{found:?}");
    assert_eq!((r.report.imported.placeholder_effects, r.report.imported.effects), (1, 1), "the placeholder and the Levels");
}

#[test]
fn keys_and_expressions_of_a_placeholder_are_kept() {
    let fx = third_party(vec![(4, 12.5)], vec![(2, vec![(0, 10.0), (25, 20.0)])]);
    let bytes = with_expression(&bytes_with(&fx), "CC Cylinder-0004", "time * 90");
    let (g, _) = import_bytes(&bytes, "CC Cylinder");
    let g = g.unwrap();
    let radius = g.prop("CC Cylinder-0002").unwrap();
    assert_eq!(radius.keys.iter().map(|k| k.value.clone()).collect::<Vec<_>>(), [Value::Scalar(10.0), Value::Scalar(20.0)]);
    let angle = g.prop("CC Cylinder-0004").unwrap();
    assert_eq!(angle.expr.as_ref().map(|e| (e.text.as_str(), e.enabled)), Some(("time * 90", true)));
}

/// A pseudo effect's parameter match names (`Pseudo/0.25-0001`) are no property path segment:
/// its parameters are `param<index>`, found by their names.
#[test]
fn a_pseudo_effect_is_kept_with_its_parameter_names() {
    let fx = Fx {
        match_name: "Pseudo/0.25",
        name: "Settings",
        defs: vec![slider(K::FloatSlider, "Speed", 1.0, (0.0, 100.0)), popup("Mode", 2, &["A", "B"])],
        values: vec![(1, 4.0)],
        data: vec![],
        keys: vec![],
    };
    let (g, r) = placeholder_of(&fx);
    assert_eq!(g.prop("param1").map(|p| (p.name.as_str(), p.value.clone())), Some(("Speed", Value::Scalar(4.0))));
    assert_eq!(g.prop("param2").map(|p| (p.name.as_str(), p.value.clone())), Some(("Mode", Value::Enum(1))));
    assert_eq!(losses(&r, LossCategory::Effects, "Pseudo/0.25").len(), 1);
    // The effect id stays the match name (Find Missing Effects, expressions); the group's match id
    // is one path segment, so a match path (keyframe paste, pick whip) finds its parameters again.
    assert_eq!(g.kind, GroupKind::Effect { effect: "Pseudo/0.25".into() });
    let speed = g.prop("param1").map(|p| p.uid);
    let layer = PropGroup::new(0, "layer", "Layer").with(PropGroup::new(0, "effects", "Effects").with(g));
    let path = speed.and_then(|u| layer.match_path_of(u));
    assert_eq!(path.as_deref().and_then(|p| layer.prop(p)).map(|p| p.name.as_str()), Some("Speed"), "{path:?}");
}

/// A float slider keeps its valid range and slider range, so Effect Controls drags and clamps it
/// as After Effects does; a range that is not a range stays a plain number.
#[test]
fn a_float_slider_keeps_its_range() {
    let fx = Fx {
        match_name: "Pseudo/r",
        name: "Rig",
        defs: vec![slider(K::FloatSlider, "Speed", 1.0, (-10.0, 100.0)), slider(K::FloatSlider, "Odd", 1.0, (5.0, 1.0))],
        values: vec![],
        data: vec![],
        keys: vec![],
    };
    let (g, _) = placeholder_of(&fx);
    let ui = |id: &str| g.prop(id).map(|p| p.ui.clone());
    assert_eq!(ui("param1"), Some(ParamUi::Slider { min: -10.0, max: 100.0, slider_min: -10.0, slider_max: 100.0, decimals: 2 }));
    assert_eq!(ui("param2"), Some(ParamUi::Number));
}

/// A popup whose definition is borrowed (the project copy) and has no value record: its default
/// slot holds another copy's selection, so the first item is used and that is reported.
#[test]
fn a_borrowed_popup_without_a_record_takes_the_first_item_and_is_reported() {
    let bytes = without_instance_definitions(&bytes_with(&third_party(vec![], vec![])));
    let (g, r) = import_bytes(&bytes, "CC Cylinder");
    assert_eq!(v(&g.unwrap(), "CC Cylinder-0008"), Value::Enum(0));
    let found = losses(&r, LossCategory::Effects, "CC Cylinder");
    assert!(found.iter().any(|d| d.contains("Render") && d.contains("default item is not known")), "{found:?}");
}

/// A popup default slot of 0 (no item; After Effects' save of a project it opened) takes the first
/// item and is reported, not read as damaged data.
#[test]
fn a_popup_default_of_no_item_takes_the_first_item() {
    let fx = Fx { match_name: "Some Fx", name: "Some", defs: vec![popup("Mode", 0, &["A", "B"])], values: vec![], data: vec![], keys: vec![] };
    let (g, r) = placeholder_of(&fx);
    assert_eq!(v(&g, "Some Fx-0001"), Value::Enum(0));
    let found = losses(&r, LossCategory::Effects, "Some Fx");
    assert!(found.iter().any(|d| d.contains("default item is not stored")), "{found:?}");
    assert!(losses(&r, LossCategory::DamagedData, "Some Fx").is_empty());
}

/// A popup past the 1024 items kept: the cut is reported, and a selection among the cut items
/// takes the first item and is reported as such, not as damaged data.
#[test]
fn a_popup_past_the_items_kept_is_reported() {
    let items: &'static [&'static str] = Box::leak((0..1100).map(|i| &*Box::leak(format!("I{i}").into_boxed_str())).collect::<Vec<_>>().into_boxed_slice());
    let fx = Fx { match_name: "Long Fx", name: "Long", defs: vec![popup("Pick", 1, items)], values: vec![(1, 1050.0)], data: vec![], keys: vec![] };
    let (g, r) = placeholder_of(&fx);
    let pick = g.prop("Long Fx-0001").unwrap();
    assert_eq!(pick.value, Value::Enum(0));
    assert!(matches!(&pick.ui, ParamUi::Popup { options } if options.len() == 1024));
    let found = losses(&r, LossCategory::Effects, "Long Fx");
    assert!(found.iter().any(|d| d.contains("past the first 1024")), "{found:?}");
    assert!(found.iter().any(|d| d.contains("item 1050 is past the items kept")), "{found:?}");
    assert!(losses(&r, LossCategory::DamagedData, "Long Fx").is_empty());
}

/// Many large popups stay within the import's budget: what is beyond is reported, not kept.
#[test]
fn a_hostile_placeholder_stays_bounded() {
    let items: &'static [&'static str] = Box::leak((0..1000).map(|i| &*Box::leak(format!("I{i}").into_boxed_str())).collect::<Vec<_>>().into_boxed_slice());
    let fx = Fx { match_name: "Big Fx", name: "Big", defs: (0..300).map(|_| popup("P", 1, items)).collect(), values: vec![], data: vec![], keys: vec![] };
    let (g, r) = placeholder_of(&fx);
    let kept = g.props().count();
    assert!(kept > 0 && kept < 300, "{kept}");
    assert!(g.props().map(|p| if let ParamUi::Popup { options } = &p.ui { options.len() + 1 } else { 1 }).sum::<usize>() <= 200_000);
    assert!(losses(&r, LossCategory::Effects, "Big Fx").iter().any(|d| d.contains("placeholder parameters")));
}

/// Two instances of an effect whose parameters fill the import's budget: the first keeps what fits
/// and stops at the first parameter past it; the second is reported without a placeholder (its
/// shared definitions are not read again).
#[test]
fn an_effect_past_the_spent_budget_gets_no_placeholder() {
    let items: &'static [&'static str] = Box::leak((0..1000).map(|i| &*Box::leak(format!("I{i}").into_boxed_str())).collect::<Vec<_>>().into_boxed_slice());
    let fx = Fx { match_name: "Big Fx", name: "Big", defs: (0..300).map(|_| popup("P", 1, items)).collect(), values: vec![], data: vec![], keys: vec![] };
    let bytes = twice(&bytes_with(&fx), "Big Fx");
    let mut project = effectcraft_project::Project::default();
    let none = |_: &str| false;
    let r = effectcraft_aep::import::import(
        &mut project,
        &bytes,
        &effectcraft_aep::import::ImportOptions { allow_losses: true, ..effectcraft_aep::import::ImportOptions::new(&none) },
    )
    .unwrap();
    let placeholders: Vec<usize> = project
        .comps()
        .flat_map(|(_, c)| c.layers.iter())
        .filter_map(|l| l.props.sub("effects"))
        .flat_map(|fx| fx.children.iter().filter_map(|n| n.as_group()).filter(|g| g.match_id == "Big Fx").map(|g| g.props().count()).collect::<Vec<_>>())
        .collect();
    // 199 popups of 1000 items (1001 each) fit in 200 000; the 200th stops the first instance.
    assert_eq!(placeholders, [199]);
    let found = losses(&r, LossCategory::Effects, "Big Fx");
    assert_eq!(found.iter().filter(|d| d.contains("and the ones after it not kept")).count(), 1, "{found:?}");
    assert!(found.iter().any(|d| d.contains("not kept (past the import's placeholder parameters)")), "{found:?}");
    assert!(found.iter().any(|d| d.contains("kept without rendering") && !d.contains("more like it")), "the kept instance alone: {found:?}");
}

/// Parameters a placeholder does not keep (markers, arbitrary data) still spend the budget: a
/// definition of very many of them, shared by many instances, is not walked again for each one.
#[test]
fn parameters_not_kept_still_spend_the_budget() {
    let defs = (0..200_000).map(|i| other(if i % 2 == 0 { K::NoData } else { K::ArbitraryData }, "X", Default::None)).collect();
    let fx = Fx { match_name: "Odd Fx", name: "Odd", defs, values: vec![], data: vec![], keys: vec![] };
    let (_, r) = import_bytes(&twice(&bytes_with(&fx), "Odd Fx"), "Odd Fx");
    let found = losses(&r, LossCategory::Effects, "Odd Fx");
    assert!(found.iter().any(|d| d.contains("not kept (past the import's placeholder parameters)")), "{found:?}");
}

/// Long popup item names spend the budget by their length, so the names kept stay bounded.
#[test]
fn long_popup_item_names_spend_the_budget() {
    let long = "x".repeat(1600);
    let items: &'static [&'static str] =
        Box::leak((0..1024).map(|i| &*Box::leak(format!("{long}{i}").into_boxed_str())).collect::<Vec<_>>().into_boxed_slice());
    let fx = Fx { match_name: "Long Fx", name: "Long", defs: vec![popup("A", 1, items), popup("B", 1, items)], values: vec![], data: vec![], keys: vec![] };
    let (g, r) = placeholder_of(&fx);
    assert_eq!(g.props().count(), 1);
    assert!(losses(&r, LossCategory::Effects, "Long Fx").iter().any(|d| d.contains("placeholder parameters")));
}

/// `bytes` with the layer's instance of `match_name` (its `tdmn` and `sspc`) repeated after it.
fn twice(bytes: &[u8], match_name: &str) -> Vec<u8> {
    let mut file = parse(bytes).unwrap();
    let mut done = 0;
    edit(&mut file.chunks, false, &mut |c, in_project_copy| {
        let Body::List { children, .. } = &mut c.body else { return };
        if in_project_copy {
            return;
        }
        let Some(at) = children.iter().position(|k| k.is(b"tdmn") && k.bytes().is_some_and(|b| b.split(|x| *x == 0).next() == Some(match_name.as_bytes())))
        else {
            return;
        };
        let pair: Vec<Chunk<'_>> = children[at..at + 2].to_vec();
        children.splice(at + 2..at + 2, pair);
        done += 1;
    });
    assert_eq!(done, 1, "one instance");
    write(&file).unwrap()
}

/// `bytes` with the expression `text` on the parameter record `param` (enabled: `tdb4` byte 120
/// 1, docs/aep-format.md 3.6).
fn with_expression(bytes: &[u8], param: &str, text: &str) -> Vec<u8> {
    let mut file = parse(bytes).unwrap();
    let mut done = 0;
    edit(&mut file.chunks, false, &mut |c, _| {
        let Body::List { children, .. } = &mut c.body else { return };
        let Some(at) = children.iter().position(|k| k.is(b"tdmn") && k.bytes().is_some_and(|b| b.split(|x| *x == 0).next() == Some(param.as_bytes()))) else {
            return;
        };
        let Some(Body::List { children: record, .. }) = children.get_mut(at + 1).map(|k| &mut k.body) else { return };
        for k in record.iter_mut().filter(|k| k.is(b"tdb4")) {
            if let Body::Data(d) = &mut k.body
                && let Some(b) = d.to_mut().get_mut(120)
            {
                *b = 1;
            }
        }
        record.push(Chunk::data(*b"Utf8", text.as_bytes().to_vec()));
        done += 1;
    });
    assert!(done >= 1);
    write(&file).unwrap()
}

/// `bytes` with the layer's instance of `CC Cylinder` stripped of its own definitions (an empty
/// `parT`): it borrows the project copy's (`EfdG`).
fn without_instance_definitions(bytes: &[u8]) -> Vec<u8> {
    let mut file = parse(bytes).unwrap();
    let mut done = 0;
    edit(&mut file.chunks, false, &mut |c, in_project_copy| {
        let Body::List { children, .. } = &mut c.body else { return };
        if in_project_copy {
            return;
        }
        let Some(at) = children.iter().position(|k| k.is(b"tdmn") && k.bytes().is_some_and(|b| b.split(|x| *x == 0).next() == Some(&b"CC Cylinder"[..])))
        else {
            return;
        };
        let Some(sspc) = children.get_mut(at + 1).filter(|k| k.is_list(b"sspc")) else { return };
        for k in sspc.children_mut() {
            if let Body::List { list_type, children } = &mut k.body
                && list_type == b"parT"
            {
                children.clear();
                done += 1;
            }
        }
    });
    assert_eq!(done, 1, "one instance");
    write(&file).unwrap()
}

/// Calls `f` on every chunk with whether it is inside the project's effect definitions (`EfdG`).
fn edit(chunks: &mut [Chunk<'_>], in_efdg: bool, f: &mut dyn FnMut(&mut Chunk<'_>, bool)) {
    for c in chunks {
        let inside = in_efdg || c.is_list(b"EfdG");
        f(c, inside);
        edit(c.children_mut(), inside, f);
    }
}
