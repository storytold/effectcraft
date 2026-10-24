//! Effects of session S18 (711-723): Directional Blur, Radial Blur, CC Radial Blur, Venetian
//! Blinds, Channel Blur, Black & White, Tritone, Luma Key, Simple Choker, Matte Choker and
//! Transform import with the values they were made with.

mod common;

use common::{comp, load};
use effectcraft_keyframe::Value;
use effectcraft_project::PropGroup;

/// The effects of layer `layer`, and the import report's loss count.
fn effects(name: &str, layer: &str) -> (Vec<PropGroup>, usize, String) {
    let (p, r) = load(name);
    let (_, c) = comp(&p, &r, "C");
    let l = c.layers.iter().find(|l| l.name == layer).unwrap_or_else(|| panic!("{name}: no {layer}"));
    let fx = l.props.sub("effects").expect("effects");
    (fx.children.iter().filter_map(|n| n.as_group()).cloned().collect(), r.report.total_losses(), format!("{:?}", r.report.losses))
}

fn one(name: &str, layer: &str, id: &str) -> PropGroup {
    let (g, losses, detail) = effects(name, layer);
    assert_eq!(losses, 0, "{name}: {detail}");
    assert_eq!(g.len(), 1, "{name}");
    assert_eq!(g[0].match_id, id, "{name}");
    g[0].clone()
}

fn v(g: &PropGroup, id: &str) -> Value {
    g.prop(id).unwrap_or_else(|| panic!("no {id} in {}", g.match_id)).value.clone()
}

fn s(x: f64) -> Value {
    Value::Scalar(x)
}

fn near2(a: &Value, b: [f64; 2]) {
    let Value::Vec2(a) = a else { panic!("{a:?}") };
    assert!((a[0] - b[0]).abs() < 1e-3 && (a[1] - b[1]).abs() < 1e-3, "{a:?} != {b:?}");
}

fn near_color(a: &Value, b: [f64; 3]) {
    let Value::Color(a) = a else { panic!("{a:?}") };
    assert!(a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-3) && a[3] == 1.0, "{a:?} != {b:?}");
}

#[test]
fn the_eleven_defaults_import_lossless() {
    let (g, losses, detail) = effects("712-fx-eleven-effects-defaults-on-M.aep", "M");
    // CC Radial Blur's fresh Type is item 6, Scratch, which EffectCraft does not have. Set Matte
    // (the eleventh) is imported since its layer parameter is (905).
    assert_eq!(g.len(), 11, "{detail}");
    assert_eq!(losses, 1, "{detail}");
    assert!(detail.contains("Scratch"), "{detail}");
    let ids: Vec<&str> = g.iter().map(|e| e.match_id.as_str()).collect();
    assert_eq!(
        ids,
        [
            "ec.blur.radial",
            "ec.blur.ccradial",
            "ec.transition.venetian",
            "ec.blur.channel",
            "ec.color.blackwhite",
            "ec.color.tritone",
            "ec.key.luma",
            "ec.matte.simplechoker",
            "ec.matte.mattechoker",
            "ec.distort.transform",
            "ec.channel.setmatte"
        ]
    );
    // After Effects' defaults where they differ from EffectCraft's.
    assert_eq!(v(&g[2], "direction"), s(0.0));
    assert_eq!(v(&g[2], "width"), s(20.0));
    assert_eq!(v(&g[3], "repeatEdge"), Value::Bool(true));
    near_color(&v(&g[4], "tintColor"), [225.0 / 255.0, 211.0 / 255.0, 179.0 / 255.0]);
    near_color(&v(&g[5], "midtones"), [127.0 / 255.0, 100.0 / 255.0, 70.0 / 255.0]);
    assert_eq!(v(&g[8], "grayLevelSoftness1"), s(0.100006103515625 * 100.0));
    near2(&v(&g[9], "anchor"), [480.0, 270.0]);
}

#[test]
fn blurs_and_venetian_blinds() {
    let g = one("711-fx-directional-blur-30-40-on-M.aep", "M", "ec.blur.directional");
    assert_eq!((v(&g, "direction"), v(&g, "length")), (s(30.0), s(40.0)));
    let g = one("713-fx-radial-blur-zoom-20-on-M.aep", "M", "ec.blur.radial");
    assert_eq!((v(&g, "amount"), v(&g, "type"), v(&g, "antialiasing")), (s(20.0), Value::Enum(1), Value::Enum(1)));
    near2(&v(&g, "center"), [600.0, 300.0]);
    let g = one("714-fx-ccradial-type2-30-60-on-M.aep", "M", "ec.blur.ccradial");
    assert_eq!((v(&g, "type"), v(&g, "amount"), v(&g, "quality")), (Value::Enum(1), s(30.0), s(60.0)));
    near2(&v(&g, "center"), [400.0, 200.0]);
    let g = one("715-fx-venetian-40-30-50-5-on-M.aep", "M", "ec.transition.venetian");
    assert_eq!((v(&g, "completion"), v(&g, "direction"), v(&g, "width"), v(&g, "feather")), (s(40.0), s(30.0), s(50.0), s(5.0)));
    let g = one("716-fx-channel-blur-5-10-15-20-edge-horiz-on-M.aep", "M", "ec.blur.channel");
    assert_eq!((v(&g, "redBlurriness"), v(&g, "greenBlurriness"), v(&g, "blueBlurriness"), v(&g, "alphaBlurriness")), (s(5.0), s(10.0), s(15.0), s(20.0)));
    assert_eq!((v(&g, "repeatEdge"), v(&g, "dimensions")), (Value::Bool(true), Value::Enum(1)));
}

#[test]
fn colour_effects() {
    let g = one("717-fx-blackwhite-set-tint-on-L.aep", "L", "ec.color.blackwhite");
    assert_eq!(["reds", "yellows", "greens", "cyans", "blues", "magentas"].map(|id| v(&g, id)), [s(10.0), s(70.0), s(30.0), s(50.0), s(90.0), s(20.0)]);
    assert_eq!(v(&g, "tint"), Value::Bool(true));
    near_color(&v(&g, "tintColor"), [1.0, 0.5, 0.0]);
    let g = one("718-fx-tritone-set-blend25-on-L.aep", "L", "ec.color.tritone");
    near_color(&v(&g, "highlights"), [1.0, 1.0, 0.8]);
    near_color(&v(&g, "midtones"), [0.6, 0.2, 0.2]);
    near_color(&v(&g, "shadows"), [0.0, 0.0, 0.3]);
    assert_eq!(v(&g, "blend"), s(25.0));
}

#[test]
fn keys_and_mattes() {
    let g = one("719-fx-lumakey-darker-140-20-1-2.5-on-M.aep", "M", "ec.key.luma");
    assert_eq!(
        (v(&g, "keyType"), v(&g, "threshold"), v(&g, "tolerance"), v(&g, "edgeThin"), v(&g, "edgeFeather")),
        (Value::Enum(1), s(140.0), s(20.0), s(1.0), s(2.5))
    );
    let g = one("720-fx-simplechoker-3.5-on-M.aep", "M", "ec.matte.simplechoker");
    assert_eq!((v(&g, "view"), v(&g, "chokeMatte")), (Value::Enum(0), s(3.5)));
    assert_eq!(v(&one("721-fx-simplechoker-3.5-view-matte-on-M.aep", "M", "ec.matte.simplechoker"), "view"), Value::Enum(1));
    let g = one("722-fx-mattechoker-set-on-M.aep", "M", "ec.matte.mattechoker");
    assert_eq!(["geometricSoftness1", "choke1", "geometricSoftness2", "choke2", "iterations"].map(|id| v(&g, id)), [s(6.0), s(50.0), s(2.0), s(-10.0), s(3.0)]);
    // The gray level softnesses kept their defaults (AE refused the scripted values): 10 % and 100 %.
    assert_eq!((v(&g, "grayLevelSoftness1"), v(&g, "grayLevelSoftness2")), (s(0.100006103515625 * 100.0), s(100.0)));
}

#[test]
fn transform() {
    let g = one("723-fx-transform-set-on-M.aep", "M", "ec.distort.transform");
    near2(&v(&g, "anchor"), [400.0, 200.0]);
    near2(&v(&g, "position"), [500.0, 300.0]);
    assert_eq!(
        ["scaleHeight", "scaleWidth", "skew", "skewAxis", "rotation", "opacity", "shutterAngle"].map(|id| v(&g, id)),
        [s(80.0), s(120.0), s(10.0), s(20.0), s(30.0), s(70.0), s(90.0)]
    );
    assert_eq!((v(&g, "uniform"), v(&g, "useCompositionShutterAngle"), v(&g, "sampling")), (Value::Bool(false), Value::Bool(false), Value::Enum(1)));
}
