//! Text animators and range selectors (579-594, 114) and the character and paragraph settings of
//! S17 (538-578, 595, 596).

mod common;

use common::{comp, load};
use effectcraft_aep_import::{ImportResult, LossCategory};
use effectcraft_keyframe::Value;
use effectcraft_keyframe::text_doc::{Composer, Direction, Justify, Kerning, TextDoc};
use effectcraft_project::{Layer, Project, PropGroup};
use effectcraft_time::{TICKS_PER_SECOND, Tick};

fn text_layer<'p>(p: &'p Project, r: &ImportResult) -> &'p Layer {
    let (_, c) = comp(p, r, "C");
    c.layers.iter().find(|l| l.name == "T").expect("text layer T")
}

fn animators(l: &Layer) -> Vec<&PropGroup> {
    l.props.group("text/animators").expect("animators").groups().collect()
}

fn scalar(g: &PropGroup, path: &str) -> f64 {
    match g.prop(path).unwrap_or_else(|| panic!("no {path}")).value {
        Value::Scalar(v) => v,
        ref v => panic!("{path}: {v:?}"),
    }
}

fn enumv(g: &PropGroup, path: &str) -> u32 {
    match g.prop(path).unwrap_or_else(|| panic!("no {path}")).value {
        Value::Enum(v) => v,
        ref v => panic!("{path}: {v:?}"),
    }
}

fn selector(a: &PropGroup) -> &PropGroup {
    a.sub("selectors").and_then(|s| s.groups().next()).expect("a selector")
}

#[test]
fn an_animator_with_position_imports_lossless() {
    let (p, r) = load("579-textanim-position-0-m50.aep");
    assert!(r.report.is_lossless(), "{:?}", r.report.losses);
    assert_eq!(r.report.imported.text_animators, 1);
    let l = text_layer(&p, &r);
    let a = animators(l);
    assert_eq!(a.len(), 1);
    assert_eq!(a[0].name, "Animator 1");
    assert_eq!(a[0].prop("properties/position").unwrap().value, Value::Vec3([0.0, -50.0, 0.0]));
    let s = selector(a[0]);
    assert_eq!(s.name, "Range Selector 1");
    assert_eq!((scalar(s, "start"), scalar(s, "end"), scalar(s, "offset")), (0.0, 100.0, 0.0));
}

#[test]
fn range_selector_settings_arrive() {
    let one = |name: &str| {
        let (p, r) = load(name);
        assert!(r.report.is_lossless(), "{name}: {:?}", r.report.losses);
        let l = text_layer(&p, &r);
        selector(animators(l)[0]).clone()
    };
    assert_eq!(scalar(&one("580-textanim-start-25.aep"), "start"), 25.0);
    assert_eq!(scalar(&one("581-textanim-end-75.aep"), "end"), 75.0);
    assert_eq!(scalar(&one("582-textanim-offset-10.aep"), "offset"), 10.0);
    // Units = Index: Start / End are characters (End = the 4 characters of HHHH).
    let s = one("583-textanim-units-index.aep");
    assert_eq!((enumv(&s, "advanced/units"), scalar(&s, "start"), scalar(&s, "end")), (1, 0.0, 4.0));
    assert_eq!(enumv(&one("584-textanim-basedon-3-words.aep"), "advanced/basedOn"), 2);
    assert_eq!(enumv(&one("585-textanim-mode-2-subtract.aep"), "advanced/mode"), 1);
    assert_eq!(scalar(&one("586-textanim-amount-50.aep"), "advanced/amount"), 50.0);
    assert_eq!(enumv(&one("587-textanim-shape-2-rampup.aep"), "advanced/shape"), 1);
    assert_eq!(scalar(&one("588-textanim-smoothness-50.aep"), "advanced/smoothness"), 50.0);
    assert_eq!(scalar(&one("589-textanim-easehigh-30.aep"), "advanced/easeHigh"), 30.0);
    assert_eq!(scalar(&one("590-textanim-easelow-40.aep"), "advanced/easeLow"), 40.0);
    let s = one("591-textanim-randomize-seed-7.aep");
    assert_eq!((s.prop("advanced/randomize").unwrap().value.clone(), scalar(&s, "advanced/randomSeed")), (Value::Bool(true), 7.0));
}

#[test]
fn every_mapped_animator_property_arrives() {
    let (p, r) = load("592-textanim-many-props.aep");
    assert!(r.report.is_lossless(), "{:?}", r.report.losses);
    let l = text_layer(&p, &r);
    let a = animators(l);
    assert_eq!(a.len(), 2);
    let m = a[1];
    assert_eq!(m.name, "Many");
    let v = |id: &str| m.prop(&format!("properties/{id}")).unwrap_or_else(|| panic!("no {id}")).value.clone();
    assert_eq!(v("anchor"), Value::Vec3([3.0, 4.0, 0.0]));
    assert_eq!(v("scale"), Value::Vec3([150.0, 120.0, 100.0]));
    assert_eq!((v("skew"), v("skewAxis"), v("rotation"), v("opacity")), (Value::Scalar(10.0), Value::Scalar(20.0), Value::Scalar(45.0), Value::Scalar(37.0)));
    assert_eq!(v("fillColor"), Value::Color([0.0, 1.0, 0.0, 1.0]));
    assert_eq!(v("strokeColor"), Value::Color([0.0, 0.0, 1.0, 1.0]));
    assert_eq!((v("strokeWidth"), v("tracking")), (Value::Scalar(3.0), Value::Scalar(20.0)));
    assert_eq!((v("lineSpacing"), v("blur")), (Value::Vec2([10.0, 20.0]), Value::Vec2([5.0, 6.0])));
    assert_eq!(v("trackingType"), Value::Enum(0));
}

#[test]
fn keyed_start_keeps_its_ease() {
    let (p, r) = load("594-textanim-position-start-keyed-ease.aep");
    assert!(r.report.is_lossless(), "{:?}", r.report.losses);
    let l = text_layer(&p, &r);
    let start = selector(animators(l)[0]).prop("start").unwrap();
    assert_eq!(start.keys.len(), 2);
    let at = |s: f64| match start.value_at(Tick((s * TICKS_PER_SECOND as f64) as i64)) {
        Value::Scalar(v) => v,
        v => panic!("{v:?}"),
    };
    assert_eq!((at(0.0), at(2.0)), (0.0, 100.0));
    // Easy ease: half way at 1 s, slower than linear at 0.5 s.
    assert!((at(1.0) - 50.0).abs() < 0.01, "{}", at(1.0));
    assert!(at(0.5) < 20.0, "{}", at(0.5));
}

#[test]
fn the_opacity_animator_of_114_imports() {
    let (p, r) = load("114-text-animator-opacity.aep");
    assert!(r.report.is_lossless(), "{:?}", r.report.losses);
    let (_, c) = comp(&p, &r, "C");
    let l = c.layers.iter().find(|l| l.props.group("text/animators").is_some_and(|a| a.groups().next().is_some())).expect("animated text");
    assert_eq!(animators(l)[0].prop("properties/opacity").unwrap().value, Value::Scalar(0.0));
}

fn doc(name: &str) -> (TextDoc, ImportResult) {
    let (p, r) = load(name);
    let l = text_layer(&p, &r);
    match &l.props.prop("text/sourceText").unwrap().value {
        Value::Text(d) => ((**d).clone(), r),
        v => panic!("{v:?}"),
    }
}

#[test]
fn character_settings_arrive() {
    let style = |name: &str| {
        let (d, r) = doc(name);
        assert!(r.report.is_lossless(), "{name}: {:?}", r.report.losses);
        d.style_at(0)
    };
    assert_eq!(style("539-text-hscale-0.9.aep").h_scale, 90.0);
    assert_eq!(style("540-text-vscale-1.5.aep").v_scale, 150.0);
    assert_eq!(style("541-text-baselineshift-41.aep").baseline_shift, 41.0);
    assert_eq!(style("542-text-autokern-none.aep").kerning, Kerning::Manual(0.0));
    assert_eq!(style("543-text-autokern-optical.aep").kerning, Kerning::Optical);
    assert_eq!(style("538-text-point-HHHH-normal-ref.aep").kerning, Kerning::Metrics);
    let s = style("544-text-applystroke-on.aep");
    assert!(s.apply_stroke && s.stroke_width == 1.0);
    assert_eq!(style("545-text-strokewidth-5.aep").stroke_width, 5.0);
    assert!(!style("546-text-applyfill-off.aep").apply_fill);
    assert_eq!(style("553-text-tsume-0.5.aep").tsume, 50.0);
    assert!(style("555-text-ligature-on.aep").ligatures);
    assert!(!style("538-text-point-HHHH-normal-ref.aep").ligatures);
    // Digit set Arabic lays out like the default (571) and is no loss.
    style("571-text-digitset-arabic.aep");
    let (d, r) = doc("596-text-combined-fill-over-stroke.aep");
    assert!(r.report.is_lossless() && !d.stroke_over_fill);
    let (d, _) = doc("595-text-stroke-scale-shift-combined.aep");
    let s = d.style_at(0);
    assert!(d.stroke_over_fill && s.apply_stroke);
    assert_eq!((s.stroke_width, s.stroke, s.h_scale, s.v_scale, s.baseline_shift), (5.0, [0.0, 0.0, 1.0, 1.0], 90.0, 150.0, 41.0));
}

#[test]
fn paragraph_settings_arrive() {
    let para = |name: &str| {
        let (d, r) = doc(name);
        assert!(r.report.is_lossless(), "{name}: {:?}", r.report.losses);
        d.para(0)
    };
    assert_eq!(para("560-text-firstlineindent-20.aep").indent_first, 20.0);
    assert_eq!(para("561-text-startindent-30.aep").indent_left, 30.0);
    assert_eq!(para("562-text-endindent-40.aep").indent_right, 40.0);
    assert_eq!(para("563-text-spacebefore-10.aep").space_before, 10.0);
    assert_eq!(para("564-text-spaceafter-12.aep").space_after, 12.0);
    assert!(para("558-text-hangingroman-on.aep").hanging_punctuation);
    assert_eq!(para("559-text-everyline-on.aep").composer, Composer::EveryLine);
    assert_eq!(para("538-text-point-HHHH-normal-ref.aep").composer, Composer::SingleLine);
    assert_eq!(para("569-text-direction-rtl.aep").direction, Direction::Rtl);
    // 569 stores "left" (0) in a right-to-left paragraph: After Effects lays it out flush right
    // (export test t7), so it arrives as visual right.
    assert_eq!(para("569-text-direction-rtl.aep").justify, Justify::Right);
    assert_eq!(para("556-text-autohyphenate-off.aep").justify, Justify::Left);
    assert_eq!(para("565-text-justify-fullleft.aep").justify, Justify::JustifyLastLeft);
    assert_eq!(para("566-text-justify-fullcenter.aep").justify, Justify::JustifyLastCenter);
    assert_eq!(para("567-text-justify-fullright.aep").justify, Justify::JustifyLastRight);
    assert_eq!(para("568-text-justify-fullall.aep").justify, Justify::JustifyAll);
    let (d, r) = doc("575-text-vertical-rtl.aep");
    assert!(d.vertical && r.report.is_lossless(), "{:?}", r.report.losses);
}

#[test]
fn settings_without_an_effectcraft_field_are_reported() {
    for (name, what) in [
        ("548-text-linejoin-round.aep", None),
        ("552-text-nobreak-on.aep", Some("No Break")),
        ("554-text-kerning-50.aep", Some("manual kerning")),
        ("572-text-leadingtype-japanese.aep", Some("Japanese leading")),
        ("578-text-digitset-hindi.aep", Some("digit set")),
    ] {
        let (_, r) = doc(name);
        match what {
            // The line join of a stroke that is not drawn carries nothing.
            None => assert!(r.report.is_lossless(), "{name}: {:?}", r.report.losses),
            Some(w) => {
                assert_eq!(r.report.count(LossCategory::TextLayers), 1, "{name}: {:?}", r.report.losses);
                assert!(r.report.losses.iter().any(|l| l.details.iter().any(|d| d.contains(w))), "{name}: {:?}", r.report.losses);
            }
        }
    }
}

#[test]
fn a_character_offset_animator_imports() {
    // 728: Character Offset 3; After Effects adds Character Alignment 1 and Character Range 1.
    let (p, r) = load("728-textanim-character-offset-3.aep");
    assert!(r.report.is_lossless(), "{:?}", r.report.losses);
    let l = text_layer(&p, &r);
    let a = animators(l)[0];
    assert_eq!(a.prop("properties/characterOffset").unwrap().value, Value::Scalar(3.0));
    assert_eq!(enumv(a, "properties/characterRange"), 0);
}
