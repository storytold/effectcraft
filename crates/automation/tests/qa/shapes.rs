//! Scenario 2 — a shape-layer logo: merge paths, offset paths, a gradient fill and a repeater
//! inside the mark, a trim-paths ring with eased keys (Keyframe Velocity), both parented to a
//! rotating null; exported to Lottie, re-imported, and compared frame by frame.

use serde_json::json;

use crate::harness::{Qa, centroid, coverage, mean_diff};

fn white(p: &image::Rgba<u8>) -> bool {
    p[0] > 200 && p[1] > 200 && p[2] > 200
}
fn bluish(p: &image::Rgba<u8>) -> bool {
    p[2] > 150 && p[0] < 120
}
fn yellowish(p: &image::Rgba<u8>) -> bool {
    p[0] > 150 && p[1] > 150 && p[2] < 110
}

#[test]
fn shape_logo_with_lottie_round_trip() {
    let mut qa = Qa::new("shapes");
    qa.exec("comp.new", json!({"name": "Logo", "width": 160, "height": 160, "frameRate": 12, "duration": 2}));
    qa.exec("layer.newNull", json!({"name": "Ctrl"}));

    // The mark: a square merged with a circle, grown by Offset Paths, gradient-filled, repeated.
    qa.exec("layer.newShape", json!({"kind": "rect", "size": [30, 30], "name": "Mark", "fill": "#ff0000", "position": [80, 80]}));
    let ell = qa.exec("layer.addShapeItem", json!({"layer": "Mark", "kind": "ellipse", "group": "contents/group"}));
    assert_eq!(ell["path"], "contents/group/contents/ellipse", "addShapeItem answers with the new item's path: {ell}");
    let base = ell["path"].as_str().unwrap().trim_end_matches("/ellipse").to_string();
    qa.tool("set_property", json!({"layer": "Mark", "path": format!("{base}/ellipse/position"), "value": [15, 15]}));
    qa.tool("set_property", json!({"layer": "Mark", "path": format!("{base}/ellipse/size"), "value": [24, 24]}));
    qa.exec("layer.addShapeItem", json!({"layer": "Mark", "kind": "merge", "group": "contents/group"}));
    let off = qa.exec("layer.addShapeItem", json!({"layer": "Mark", "kind": "offset", "group": "contents/group"}));
    qa.tool("set_property", json!({"layer": "Mark", "path": format!("{}/amount", off["path"].as_str().unwrap()), "value": 3}));
    let gf = qa.exec("layer.addShapeItem", json!({"layer": "Mark", "kind": "gfill", "group": "contents/group"}))["path"].as_str().unwrap().to_string();
    qa.tool("set_property", json!({"layer": "Mark", "path": format!("{base}/fill/opacity"), "value": 0}));
    // Gradients as plain JSON (a list of colour stops), not only the tagged form get returns.
    let g = qa.tool("set_property", json!({"layer": "Mark", "path": format!("{gf}/colors"), "value": ["#0080ff", "#ffff00"]}));
    assert_eq!(g["type"], "gradient");
    qa.tool("set_property", json!({"layer": "Mark", "path": format!("{gf}/start"), "value": [-20, 0]}));
    qa.tool("set_property", json!({"layer": "Mark", "path": format!("{gf}/end"), "value": [30, 0]}));
    let rep = qa.exec("layer.addShapeItem", json!({"layer": "Mark", "kind": "repeater"}))["path"].as_str().unwrap().to_string();
    qa.tool("set_property", json!({"layer": "Mark", "path": format!("{rep}/transform/position"), "value": [0, 0]}));
    qa.tool("set_property", json!({"layer": "Mark", "path": format!("{rep}/transform/rotation"), "value": 120}));

    // The ring: a stroke drawn on by Trim Paths, eased with Keyframe Velocity.
    qa.exec("layer.newShape", json!({"kind": "ellipse", "size": [120, 120], "name": "Ring", "strokeWidth": 5, "stroke": "#ffffff", "position": [80, 80]}));
    qa.tool("set_property", json!({"layer": "Ring", "path": "contents/group/contents/fill/opacity", "value": 0}));
    let trim = qa.exec("layer.addShapeItem", json!({"layer": "Ring", "kind": "trim"}))["path"].as_str().unwrap().to_string();
    let end = format!("{trim}/end");
    qa.tool("add_keyframe", json!({"layer": "Ring", "path": end, "keys": [{"time": 0, "value": 0}, {"time": 1, "value": 100}]}));
    let linear = qa.tool("get_property", json!({"layer": "Ring", "path": end, "time": 0.25}))["value"].as_f64().unwrap();
    assert!((linear - 25.0).abs() < 1e-6);
    // Keys selected by path (not only by uid), then Keyframe Velocity: speed 0, 33 % influence.
    assert_eq!(qa.exec("keys.select", json!({"keys": [{"layer": "Ring", "path": end, "time": 0}]})), json!(1));
    qa.exec("keys.velocity", json!({"outSpeed": 0, "outInfluence": 33.33}));
    assert_eq!(qa.exec("keys.select", json!({"keys": [{"layer": "Ring", "path": end, "time": 1}]})), json!(1));
    qa.exec("keys.velocity", json!({"inSpeed": 0, "inInfluence": 33.33}));
    let eased = qa.tool("get_property", json!({"layer": "Ring", "path": end, "time": 0.25}))["value"].as_f64().unwrap();
    assert!(eased < 20.0, "eased keys start slowly: {eased}");
    let mid = qa.tool("get_property", json!({"layer": "Ring", "path": end, "time": 0.5}))["value"].as_f64().unwrap();
    assert!((mid - 50.0).abs() < 1.0, "symmetric ease passes 50 % at the middle: {mid}");
    // A key reference that matches nothing is an error, not an empty selection.
    let e = qa.exec_err("keys.select", json!({"keys": [{"layer": "Ring", "path": "contents/nope", "time": 0}]}));
    assert!(e.contains("no such property"), "{e}");

    // Parent both to the null and spin it.
    qa.exec("layer.setParent", json!({"layers": ["Ring", "Mark"], "parent": "Ctrl"}));
    let e = qa.exec_err("layer.setParent", json!({"layers": ["Rnig"], "parent": "Ctrl"}));
    assert!(e.contains("no layer"), "a misspelt layer is reported: {e}");
    qa.tool("add_keyframe", json!({"layer": "Ctrl", "path": "transform/rotation", "keys": [{"time": 0, "value": 0}, {"time": 2, "value": 90}]}));

    let f0 = qa.frame(None, 0.0);
    let f05 = qa.frame(None, 0.5);
    let f15 = qa.frame(None, 1.5);
    let ring = |f: &image::RgbaImage| coverage(f, white);
    assert!(ring(&f0) < 0.002, "trim end 0 draws no ring: {}", ring(&f0));
    let (half, full) = (ring(&f05), ring(&f15));
    assert!(full > 0.03 && (half / full - 0.5).abs() < 0.1, "half the ring at 0.5 s: {half} vs {full}");
    assert!(coverage(&f15, bluish) > 0.005 && coverage(&f15, yellowish) > 0.005, "the gradient's two ends are both visible");
    // The repeater makes three copies around the layer's anchor (the comp centre): the mark's
    // centroid stays near the centre.
    let c = centroid(&f15, |p| bluish(p) || yellowish(p)).unwrap();
    assert!((c.0 - 80.0).abs() < 12.0 && (c.1 - 80.0).abs() < 12.0, "three copies balance around the centre: {c:?}");
    // The null's rotation turns the mark.
    let a = centroid(&f05, yellowish).unwrap();
    let b = centroid(&f15, yellowish).unwrap();
    assert!((a.0 - b.0).abs() + (a.1 - b.1).abs() > 2.0, "the parent's rotation moves the children: {a:?} {b:?}");

    // Lottie out and back in: the same frames.
    let path = qa.path("logo.json");
    let ex = qa.exec("file.exportLottie", json!({"comp": "Logo", "path": path}));
    assert!(ex["bytes"].as_u64().unwrap() > 500, "{ex}");
    let im = qa.exec("file.importLottie", json!({"path": path}));
    let lc = im["comp"].clone();
    assert!(lc.is_u64(), "{im}");
    for (t, orig) in [(0.0, &f0), (0.5, &f05), (1.5, &f15)] {
        let back = qa.frame(Some(lc.clone()), t);
        let d = mean_diff(orig, &back);
        assert!(d < 1.5, "Lottie round trip at {t} s differs by {d}");
    }
}
