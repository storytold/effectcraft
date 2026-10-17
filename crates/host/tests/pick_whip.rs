//! The pick whip's references read the property they were made from (#1072): an Alt pick whip's
//! (`byIndex`) select an effect's properties by index, `effect("Fractal Noise")(i)(j)`, and must
//! read the same value as the reference by name.
//!
//! (With Compact English off, a reference to another layer, `thisComp.layer("Other")("effects")…`,
//! doesn't evaluate yet, by name or by index: "not a callable function".)

use serde_json::{Value, json};

#[test]
fn index_references_read_the_same_values_as_names() {
    let mut s = effectcraft_host::session();
    s.execute("comp.new", json!({"name": "T", "width": 64, "height": 64, "duration": 1})).unwrap();
    let l = s.execute("layer.newSolid", json!({"name": "Reader", "color": "#ffffff"})).unwrap()["layer"].as_u64().unwrap();
    let o = s.execute("layer.newSolid", json!({"name": "Other", "color": "#00ff00"})).unwrap()["layer"].as_u64().unwrap();
    for fx in ["Fractal Noise", "Point Control"] {
        s.execute("effect.apply", json!({"layer": o, "effect": fx})).unwrap();
    }
    s.execute("prop.set", json!({"layer": o, "path": "effects/#1/transform/scale", "value": 77})).unwrap();
    s.execute("prop.set", json!({"layer": o, "path": "effects/#2/point", "value": [12, 34]})).unwrap();
    let evaluated = |s: &mut effectcraft_engine::Session, path: &str| -> Value {
        let v = s.execute("prop.get", json!({"layer": l, "path": path})).unwrap();
        assert!(v.get("expressionError").is_none(), "{v}");
        v["evaluated"].clone()
    };
    for (path, target, dimension, want) in
        [("transform/rotation", "effects/#1/transform/scale", None, json!(77.0)), ("transform/opacity", "effects/#2/point", Some(1), json!(34.0))]
    {
        let mut t = json!({"layer": o, "path": target});
        if let Some(d) = dimension {
            t["dimension"] = json!(d);
        }
        for by_index in [false, true] {
            let e = s.execute("prop.pickWhip", json!({"layer": l, "path": path, "target": t, "byIndex": by_index})).unwrap();
            assert_eq!(evaluated(&mut s, path), want, "by index {by_index}: {e}");
        }
    }
}
