use super::*;
use crate::{Expression, LayerSource, Solid, build};
use effectcraft_time::{FrameRate, Tick};

fn names(comps: &[&str], footage: &[&str]) -> ExprReach {
    ExprReach::Names { comps: comps.iter().map(|s| s.to_string()).collect(), footage: footage.iter().map(|s| s.to_string()).collect() }
}

/// Expressions that read their own comp, or other items by literal names, are followed; every
/// way to reach items by a computed name or an id, or text the scanner can't read for sure,
/// reaches everything.
#[test]
fn expression_reach_follows_literal_names_and_refuses_the_rest() {
    let follows = [
        ("wiggle(2, 20)", names(&[], &[])),
        ("thisComp.layer(\"Comp 1 Layer\").transform.opacity * 2 / 3", names(&[], &[])),
        ("value + [10, 0]; value[0] / 2", names(&[], &[])),
        ("// comp(name) in a comment\n/* footage(x) */ value", names(&[], &[])),
        ("\"a string that says comp(x) and Layer\"", names(&[], &[])),
        ("comp(\"Other\").layer(1).transform.position", names(&["Other"], &[])),
        ("comp( 'A' ).width + comp(\"B\").height", names(&["A", "B"], &[])),
        ("footage(\"data.json\").sourceData.x", names(&[], &["data.json"])),
        ("x = a[0] <= b ? a[1] : 0; y = a[0] == 1", names(&[], &[])),
        ("thisLayer.source.layer(1).name", names(&[], &[])),
        ("if (a) x = 1; else x = 2; (a + b) / 2", names(&[], &[])),
        ("\"Ünïcödé name\"", names(&[], &[])),
        ("const f = (v) => v * 2; f(1)", names(&[], &[])),
    ];
    for (text, want) in follows {
        assert_eq!(expression_reach(text), want, "{text}");
    }
    let anything = [
        "comp(\"Shot \" + index).layer(1)",
        "comp(name)",
        "var n = \"Other\"; comp(n)",
        "comp(\"O\\\"ther\")",
        "x.comp(\"Other\")",
        "var comp = 1",
        "footage(thisComp.name)",
        "new Comp(7).layer(1)",
        "Layer(7, 3).name",
        "__h(__COMP, 7)",
        "o.__c = 7",
        "eval('1')",
        "Function('return 1')()",
        "this.x",
        "({}).constructor",
        "Object.getPrototypeOf(thisComp)",
        "JSON.parse(s)",
        "thisComp.layer.call(o, 1)",
        "o['_' + '_c']",
        "o[k] = 7",
        "o[k] ||= 7",
        "o[k] += 7",
        "[o[k]] = [7]",
        "({a: o[k]} = {a: 7})",
        "for (o[k] in x) {}",
        "({[k]: 7})",
        "({a: 1, [k]: 7})",
        "o = {'__c': 7}",
        "'\\x5f\\x5fc'",
        "'\\_\\_c'",
        "`${x}`",
        "c\\u006fmp('A')",
        "comp\u{0430}('A')",
        "/\"/.test(s); comp(n); \"",
        "return /x/",
        "if (a) /\"/.test(s)",
        "x = '",
        "/* comp(n)",
        "class X {}",
        "String.fromCharCode(95)",
    ];
    for text in anything {
        assert_eq!(expression_reach(text), ExprReach::Anything, "{text}");
    }
}

/// `a / b` divides after a name, a number, a string and a closing bracket or parenthesis.
#[test]
fn division_is_not_a_regular_expression() {
    for text in ["a / b", "2 / 3", "'a' / 2", "x[0] / 2", "(a + b) / 2", "f(x) / 2 / g(y)"] {
        assert_eq!(expression_reach(text), names(&[], &[]), "{text}");
    }
}

struct Fixture {
    p: Project,
    main: ItemId,
    other: ItemId,
    third: ItemId,
    unused: ItemId,
    solid: ItemId,
}

/// Comps "Main" (a solid), "Other" (a solid), "Third" (empty), an unused footage item.
fn fixture() -> Fixture {
    let mut p = Project::default();
    let comp = Comp::new(64, 64, FrameRate::FPS_30, Tick::from_seconds_f64(2.0));
    let solid =
        p.add_item("Solid", effectcraft_color::Label::None, None, ItemKind::Solid(Solid { color: [1.0, 0.0, 0.0], width: 64, height: 64, pixel_aspect: 1.0 }));
    let main = p.add_item("Main", effectcraft_color::Label::None, None, ItemKind::Comp(comp.clone().into()));
    let other = p.add_item("Other", effectcraft_color::Label::None, None, ItemKind::Comp(comp.clone().into()));
    let third = p.add_item("Third", effectcraft_color::Label::None, None, ItemKind::Comp(comp.clone().into()));
    let unused = p.add_item("unused.png", effectcraft_color::Label::None, None, ItemKind::Footage(crate::Footage { has_video: true, ..Default::default() }));
    for c in [main, other] {
        let l = build::layer(&mut p, &comp, "S", LayerSource::Solid { item: solid }, (64, 64), None);
        p.comp_mut(c).unwrap().layers.push(l);
    }
    Fixture { p, main, other, third, unused, solid }
}

fn set_expr(p: &mut Project, comp: ItemId, text: &str) {
    let l = p.comp_mut(comp).unwrap().layers.first_mut().unwrap();
    l.props.prop_mut("transform/opacity").unwrap().expr = Some(Expression { text: text.into(), enabled: true });
}

fn reach(p: &Project, comp: ItemId) -> PixelReach {
    p.pixel_reach(comp, &p.heard_layers())
}

#[test]
fn pixel_reach_is_what_the_comp_draws_and_what_its_expressions_name() {
    let mut f = fixture();
    let ids = |r: &PixelReach| r.items.clone().map(|s| s.into_iter().collect::<Vec<_>>());
    assert_eq!(reach(&f.p, f.main), PixelReach { items: Some([f.solid, f.main].into()), expressions: false, names: vec![] });
    // An expression reading only its own comp: what it draws, with expressions.
    set_expr(&mut f.p, f.main, "thisComp.layer(1).transform.opacity");
    let r = reach(&f.p, f.main);
    assert!(r.expressions);
    assert_eq!(ids(&r), Some(vec![f.solid, f.main]));
    // An expression in another comp doesn't reach this one.
    set_expr(&mut f.p, f.other, "wiggle(1, 1)");
    assert_eq!(ids(&reach(&f.p, f.main)), Some(vec![f.solid, f.main]));
    // A literal name: that comp and what it draws, with the name and the comps that have it.
    set_expr(&mut f.p, f.main, "comp(\"Other\").layer(1).transform.opacity");
    let r = reach(&f.p, f.main);
    assert_eq!(ids(&r), Some(vec![f.solid, f.main, f.other]));
    assert_eq!(r.names, vec![(true, "Other".to_string(), vec![f.other])]);
    // Another comp taking the name is part of it, whichever the lookup finds.
    f.p.item_mut(f.third).unwrap().name = "Other".into();
    let r = reach(&f.p, f.main);
    assert_eq!(r.names, vec![(true, "Other".to_string(), vec![f.other, f.third])]);
    assert!(r.items.unwrap().contains(&f.third));
    // A name no item has yet still counts (with no item).
    set_expr(&mut f.p, f.main, "footage(\"later.json\").sourceData");
    assert_eq!(reach(&f.p, f.main).names, vec![(false, "later.json".to_string(), vec![])]);
    // A computed name reaches everything.
    set_expr(&mut f.p, f.main, "comp(\"Ot\" + \"her\").width");
    assert_eq!(reach(&f.p, f.main), PixelReach { items: None, expressions: true, names: vec![] });
    // Footage reached as a layer's source brings the footage of its name (its data is read by
    // name).
    let comp = f.p.comp(f.main).unwrap().clone();
    let l = build::layer(&mut f.p, &comp, "U", LayerSource::Footage { item: f.unused }, (64, 64), None);
    f.p.comp_mut(f.main).unwrap().layers.push(l);
    let twin = f.p.add_item("unused.png", effectcraft_color::Label::None, None, ItemKind::Footage(crate::Footage::default()));
    set_expr(&mut f.p, f.main, "value");
    let r = reach(&f.p, f.main).items.unwrap();
    assert!(r.contains(&f.unused) && r.contains(&twin), "{r:?}");
}

/// A comp nested in the reach is scanned too: its expression's names count for the outer comp.
#[test]
fn nested_comps_bring_their_expressions_reach() {
    let mut f = fixture();
    let comp = f.p.comp(f.main).unwrap().clone();
    let pre = build::layer(&mut f.p, &comp, "Pre", LayerSource::Comp { item: f.other }, (64, 64), None);
    f.p.comp_mut(f.main).unwrap().layers.push(pre);
    set_expr(&mut f.p, f.other, "comp(\"Third\").width");
    let r = reach(&f.p, f.main);
    assert!(r.expressions && r.items.unwrap().contains(&f.third));
    // A cycle through names ends.
    let l = build::layer(&mut f.p, &comp, "S", LayerSource::Solid { item: f.solid }, (64, 64), None);
    f.p.comp_mut(f.third).unwrap().layers.push(l);
    set_expr(&mut f.p, f.third, "comp(\"Main\").width");
    assert!(reach(&f.p, f.main).items.unwrap().contains(&f.main));
}

/// A 3D model layer's item is reached like any other source: it is drawn, and an expression
/// reads its name and size through `layer.source` (a renamed model left the comp's disk key and
/// its old frames in place).
#[test]
fn model_sources_are_reached() {
    let mut f = fixture();
    let model = f.p.add_item(
        "chair.glb",
        effectcraft_color::Label::None,
        None,
        ItemKind::Footage(crate::Footage { kind: crate::FootageKind::Model, ..Default::default() }),
    );
    let comp = f.p.comp(f.main).unwrap().clone();
    let l = build::layer(&mut f.p, &comp, "Model", LayerSource::Model { item: model }, (64, 64), None);
    f.p.comp_mut(f.main).unwrap().layers.push(l);
    assert!(reach(&f.p, f.main).items.unwrap().contains(&model), "drawn");
    set_expr(&mut f.p, f.main, "thisComp.layer(\"Model\").source.name.length");
    let r = reach(&f.p, f.main);
    assert!(r.expressions && r.items.unwrap().contains(&model), "read by an expression");
}
