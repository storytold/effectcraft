//! The Cryptomatte eyedropper on real OpenEXR footage (#473): `effect.pickCryptomatte` reads the
//! file's Cryptomatte ranks and manifest, edits the Selection, and the rendered matte follows,
//! including undo.

use effectcraft_engine::Session;
use effectcraft_engine::time::Tick;
use effectcraft_raster::channels3d::{crypto_float, crypto_hash};
use serde_json::json;

const RIGHT: &str = "Right, \"Box\"";

/// An 8 × 4 EXR: white RGBA, and a CryptoObject layer with `Left` in columns before `split` and
/// the comma-and-quotes name from there, both named in its manifest.
fn crypto_exr(tag: &str, split: usize) -> String {
    use exr::prelude::*;
    let (w, h) = (8usize, 4usize);
    let ids: Vec<f32> = (0..w * h).map(|i| crypto_float(crypto_hash(if i % w < split { "Left" } else { RIGHT }))).collect();
    let ch = |n: &str, v: Vec<f32>| AnyChannel::new(n, FlatSamples::F32(v));
    let one = || vec![1.0f32; w * h];
    let channels = AnyChannels::sort(
        vec![ch("R", one()), ch("G", one()), ch("B", one()), ch("A", one()), ch("CryptoObject00.R", ids), ch("CryptoObject00.G", one())].into(),
    );
    let mut layer = Layer::new((w, h), LayerAttributes::default(), Encoding::FAST_LOSSLESS, channels);
    let manifest = json!({"Left": format!("{:08x}", crypto_hash("Left")), RIGHT: format!("{:08x}", crypto_hash(RIGHT))}).to_string();
    for (k, v) in [("name", "CryptoObject".to_string()), ("manifest", manifest)] {
        layer.attributes.other.insert(Text::from(format!("cryptomatte/f834d0a/{k}").as_str()), AttributeValue::Text(Text::from(v.as_str())));
    }
    // A folder of its own: numbered siblings would import as an image sequence.
    let dir = std::env::temp_dir().join(format!("ec-crypto-pick-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("crypto.exr");
    Image::from_layer(layer).write().to_file(&path).unwrap();
    path.to_string_lossy().to_string()
}

/// A session with the EXR as a layer of a 2 s comp, under a Cryptomatte effect.
fn footage(tag: &str, split: usize) -> (Session, String) {
    let path = crypto_exr(tag, split);
    let mut s = effectcraft_host::session();
    let r = s.execute("file.import", json!({"paths": [path]})).unwrap();
    assert_eq!(r["errors"], json!([]), "{r}");
    s.execute("comp.new", json!({"name": "Crypto", "width": 8, "height": 4, "frameRate": 24, "duration": 2})).unwrap();
    s.execute("layer.addItem", json!({"item": r["items"][0]})).unwrap();
    s.execute_checked("effect.apply", json!({"effect": "Cryptomatte"})).unwrap();
    (s, path)
}

/// The matte's alpha over the left and right halves.
fn matte(s: &Session) -> [f32; 2] {
    let img = s.render(s.active_comp_id().unwrap(), Tick::ZERO, Default::default());
    [img.get(1, 1)[3], img.get(6, 2)[3]]
}

#[test]
fn picking_on_a_cryptomatte_exr_edits_the_selection_and_the_matte() {
    let (mut s, path) = footage("edit", 4);
    s.execute_checked("prop.set", json!({"path": "effects/#1/display", "value": 1})).unwrap();
    assert_eq!(matte(&s), [0.0, 0.0], "nothing selected");
    let pick = |s: &mut Session, x: f64, mode: &str| s.execute_checked("effect.pickCryptomatte", json!({"effect": 1, "x": x, "y": 1, "mode": mode})).unwrap();

    assert_eq!(pick(&mut s, 1.0, "replace")["selection"], "Left");
    assert_eq!(matte(&s), [1.0, 0.0]);
    let r = pick(&mut s, 6.0, "add");
    assert_eq!((r["name"].as_str(), r["layer"].as_str()), (Some(RIGHT), Some("CryptoObject")), "{r}");
    assert_eq!(r["selection"], r#"Left, "Right, \"Box\"""#);
    assert_eq!(matte(&s), [1.0, 1.0], "the quoted name selects the right half");
    pick(&mut s, 2.0, "remove");
    assert_eq!(matte(&s), [0.0, 1.0]);
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(matte(&s), [1.0, 1.0]);
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(matte(&s), [1.0, 0.0]);
    let _ = std::fs::remove_dir_all(std::path::Path::new(&path).parent().unwrap());
}

/// At half resolution a preview pixel shows the aux texel at its centre (layer x 3 for preview
/// pixel 1): `scale` makes the pick read that one, so what is clicked is what lights up.
#[test]
fn a_pick_at_preview_resolution_reads_the_pixel_the_preview_shows() {
    let (mut s, path) = footage("half", 3);
    s.execute_checked("prop.set", json!({"path": "effects/#1/matteOnly", "value": true})).unwrap();
    let r = s.execute_checked("effect.pickCryptomatte", json!({"effect": 1, "x": 2.5, "y": 1.5, "scale": 0.5})).unwrap();
    assert_eq!(r["name"], RIGHT, "{r}");
    let opts = effectcraft_engine::render::RenderOpts { scale: 0.5, ..Default::default() };
    assert_eq!(s.render(s.active_comp_id().unwrap(), Tick::ZERO, opts).get(1, 0)[3], 1.0);
    // At full resolution the same point is Left's pixel.
    let r = s.execute_checked("effect.cryptomatteAt", json!({"effect": 1, "x": 2.5, "y": 1.5})).unwrap();
    assert_eq!(r["name"], "Left");
    let _ = std::fs::remove_dir_all(std::path::Path::new(&path).parent().unwrap());
}

/// A keyframed Selection picked at `time` is edited at that time, not at the current time.
#[test]
fn a_keyframed_selection_is_edited_where_it_was_read() {
    let (mut s, path) = footage("key", 4);
    for t in [0.0, 1.0] {
        s.execute_checked("prop.addKey", json!({"path": "effects/#1/selection", "time": t, "value": format!("<{:08x}>", crypto_hash(RIGHT))})).unwrap();
    }
    let r = s.execute_checked("effect.pickCryptomatte", json!({"effect": 1, "x": 0.5, "y": 1.5, "mode": "add", "time": 1.0})).unwrap();
    assert_eq!((r["changed"].as_bool(), r["selected"].as_bool()), (Some(true), Some(true)), "{r}");
    let sel = |s: &mut Session, t: f64| {
        let r = s.execute("effect.cryptomatteAt", json!({"effect": 1, "x": 0.5, "y": 1.5, "time": t})).unwrap();
        r["selected"].as_bool()
    };
    assert_eq!((sel(&mut s, 0.0), sel(&mut s, 1.0)), (Some(false), Some(true)), "only the key at 1 s gains Left");
    let _ = std::fs::remove_dir_all(std::path::Path::new(&path).parent().unwrap());
}

/// A precomp whose 3D layer an expression moves: the pick finds it where the render keys it
/// (the pick's aux pass runs the session's expressions).
#[test]
fn a_pick_on_a_precomp_follows_expressions() {
    let mut s = effectcraft_host::session();
    let inner = s.execute("comp.new", json!({"name": "Inner", "width": 40, "height": 20, "frameRate": 24, "duration": 1})).unwrap()["comp"].clone();
    let l = s.execute("layer.newSolid", json!({"name": "Ball", "width": 6, "height": 6})).unwrap()["layer"].clone();
    s.execute("layer.setSwitch", json!({"layers": [l], "switch": "threeD", "value": true})).unwrap();
    s.execute_checked("prop.setExpression", json!({"layer": l, "path": "transform/position", "expression": "[33, 10, 0]"})).unwrap();
    s.execute("comp.new", json!({"name": "Outer", "width": 40, "height": 20, "frameRate": 24, "duration": 1})).unwrap();
    s.execute("layer.addItem", json!({"item": inner})).unwrap();
    s.execute_checked("effect.apply", json!({"effect": "Cryptomatte"})).unwrap();
    s.execute_checked("prop.set", json!({"path": "effects/#1/matteOnly", "value": true})).unwrap();
    let r = s.execute_checked("effect.pickCryptomatte", json!({"effect": 1, "x": 33.5, "y": 10.5})).unwrap();
    assert_eq!(r["selection"], "Ball", "{r}");
    let img = s.render(s.active_comp_id().unwrap(), Tick::ZERO, Default::default());
    assert_eq!((img.get(33, 10)[3], img.get(20, 10)[3]), (1.0, 0.0));
    assert_eq!(s.execute("effect.cryptomatteAt", json!({"effect": 1, "x": 20.5, "y": 10.5})).unwrap(), serde_json::Value::Null);
}
