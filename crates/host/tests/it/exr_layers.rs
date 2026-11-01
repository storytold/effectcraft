//! Multi-layer OpenEXR footage and EXtractoR in a full session (#295): a file whose colour is
//! only in named layers (as Blender and Nuke write them) imports, `layer.channels` lists its
//! layers and channels, and EXtractoR shows the chosen ones.

use effectcraft_engine::Session;
use effectcraft_engine::time::Tick;
use serde_json::{Value, json};

/// An 8×4 EXR with layers `diffuse` (orange), `spec` (blue) and `depth` (Z = 0.25), and no
/// unnamed RGB layer.
fn layered_exr() -> String {
    use exr::prelude::*;
    let (w, h) = (8usize, 4usize);
    let ch = |n: &str, v: f32| AnyChannel::new(n, FlatSamples::F32(vec![v; w * h]));
    let channels = AnyChannels::sort(
        vec![ch("diffuse.R", 1.0), ch("diffuse.G", 0.5), ch("diffuse.B", 0.0), ch("spec.R", 0.0), ch("spec.G", 0.0), ch("spec.B", 1.0), ch("depth.Z", 0.25)]
            .into(),
    );
    let image = Image::from_layer(Layer::new((w, h), LayerAttributes::default(), Encoding::FAST_LOSSLESS, channels));
    let path = std::env::temp_dir().join(format!("ec-exr-layers-{}.exr", std::process::id()));
    image.write().to_file(&path).unwrap();
    path.to_string_lossy().to_string()
}

fn px(s: &Session) -> [f32; 4] {
    let cid = s.active_comp_id().unwrap();
    s.render(cid, Tick::ZERO, Default::default()).get(4, 2)
}

fn near(a: [f32; 4], b: [f32; 3]) -> bool {
    (0..3).all(|k| (a[k] - b[k]).abs() < 0.02)
}

#[test]
fn extractor_shows_the_chosen_layer_and_channels_of_a_layered_exr() {
    let path = layered_exr();
    let mut s = effectcraft_host::session();
    let r = s.execute("file.import", json!({"paths": [path]})).unwrap();
    assert_eq!(r["errors"], json!([]), "the file imports: {r}");
    s.execute("comp.new", json!({"name": "EXR", "width": 8, "height": 4, "frameRate": 24, "duration": 1})).unwrap();
    let layer = s.execute("layer.addItem", json!({"item": r["items"][0]})).unwrap()["layer"].clone();
    // The picture is the colour layer (diffuse: orange).
    assert!(near(px(&s), [1.0, 0.735, 0.0]), "{:?}", px(&s));

    let ch = s.execute("layer.channels", json!({"layer": layer})).unwrap();
    let names: Vec<&str> = ch["layers"].as_array().unwrap().iter().filter_map(|l| l["name"].as_str()).collect();
    assert_eq!(names, ["depth", "diffuse", "spec"], "{ch}");
    assert_eq!(ch["layers"][2]["rgba"], json!(["spec.R", "spec.G", "spec.B", ""]));
    assert_eq!(ch["layers"][0]["rgba"], json!(["depth.Z", "depth.Z", "depth.Z", ""]), "one channel fills red, green and blue");
    assert!(ch["channels"].as_array().unwrap().contains(&Value::from("depth.Z")));

    // EXtractoR on the spec layer: blue.
    s.execute("effect.apply", json!({"layers": [layer], "effect": "EXtractoR"})).unwrap();
    for (param, v) in ["red", "green", "blue", "alpha"].iter().zip(["spec.R", "spec.G", "spec.B", ""]) {
        s.execute("prop.set", json!({"layer": layer, "path": format!("effects/#1/{param}"), "value": v})).unwrap();
    }
    assert!(near(px(&s), [0.0, 0.0, 1.0]), "{:?}", px(&s));
    // Depth in all three: grey at its value.
    for param in ["red", "green", "blue"] {
        s.execute("prop.set", json!({"layer": layer, "path": format!("effects/#1/{param}"), "value": "depth.Z"})).unwrap();
    }
    assert!(near(px(&s), [0.25, 0.25, 0.25]), "{:?}", px(&s));
    // A layer that isn't OpenEXR has no channels.
    let solid = s.execute("layer.newSolid", json!({"color": "#808080"})).unwrap()["layer"].clone();
    assert_eq!(s.execute("layer.channels", json!({"layer": solid})).unwrap()["layers"], json!([]));
    let _ = std::fs::remove_file(path);
}
