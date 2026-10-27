//! Effect plug-ins in a full session: a Rust plug-in registered through the trait and the
//! example WebAssembly plug-in (examples/plugins/posterize-bands) loaded with
//! `effect.plugins.load`, both applied by id like built-ins and rendered.

use std::sync::Arc;

use effectcraft_engine::Session;
use effectcraft_engine::effects::plugin::{EffectPlugin, PluginFrame, PluginManifest, PluginParam, PluginParamKind, PluginParams, register_plugin};
use effectcraft_engine::time::Tick;
use serde_json::json;

struct Swap {
    m: PluginManifest,
}

impl EffectPlugin for Swap {
    fn manifest(&self) -> &PluginManifest {
        &self.m
    }
    fn render(&self, frame: &mut PluginFrame, params: &PluginParams, _time: f64) -> Result<(), String> {
        if params.b("on") {
            for p in frame.pixels.iter_mut() {
                p.swap(0, 2);
            }
        }
        Ok(())
    }
}

fn setup() -> (Session, u64) {
    let mut s = effectcraft_host::session();
    s.execute("comp.new", json!({"name": "P", "width": 40, "height": 20, "duration": 1})).unwrap();
    let l = s.execute("layer.newSolid", json!({"color": [1.0, 0.5, 0.0], "width": 40, "height": 20})).unwrap()["layer"].as_u64().unwrap();
    (s, l)
}

fn px(s: &Session) -> [f32; 4] {
    let cid = s.active_comp_id().unwrap();
    s.render(cid, Tick::ZERO, Default::default()).get(20, 10)
}

#[test]
fn rust_plugins_apply_by_id() {
    let m = PluginManifest {
        api: 1,
        id: "org.test.swap".into(),
        name: "Swap Red Blue".into(),
        category: "Channel".into(),
        version: "1".into(),
        author: String::new(),
        description: String::new(),
        params: vec![PluginParam { id: "on".into(), name: "On".into(), kind: PluginParamKind::Checkbox { default: true } }],
    };
    register_plugin(Arc::new(Swap { m })).unwrap();
    let (mut s, l) = setup();
    s.execute("effect.apply", json!({"layer": l, "effect": "org.test.swap"})).unwrap();
    let p = px(&s);
    assert!(p[0] < 0.01 && (p[1] - 0.5).abs() < 0.01 && p[2] > 0.99, "{p:?}");
    // Its parameters are ordinary properties: set (and animate) them like any effect's.
    s.execute("prop.set", json!({"layer": l, "path": "effects/#1/on", "value": false})).unwrap();
    let p = px(&s);
    assert!(p[0] > 0.99 && p[2] < 0.01, "{p:?}");
    let list = s.execute("effect.plugins.list", json!({})).unwrap();
    assert!(list["plugins"].as_array().unwrap().iter().any(|p| p["id"] == "org.test.swap" && p["category"] == "Channel"));
    assert_eq!(list["api"], 1);
    assert!(s.execute("effect.list", json!({"filter": "swap red"})).unwrap().as_array().unwrap().len() == 1);
}

/// Build the example plug-in (skipped when the wasm32 target isn't installed).
fn build_example() -> Option<std::path::PathBuf> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/plugins/posterize-bands");
    let target = std::env::var_os("CARGO_TARGET_DIR").map(std::path::PathBuf::from).unwrap_or_else(|| root.join("../../../target")).join("plugin-example");
    let target = if target.is_absolute() { target } else { root.join("../../..").join(target) };
    let st = std::process::Command::new(env!("CARGO"))
        .current_dir(&root)
        .args(["build", "--release", "--target", "wasm32-unknown-unknown", "--target-dir"])
        .arg(&target)
        .status()
        .ok()?;
    if !st.success() {
        eprintln!("skipping: the example plug-in didn't build (rustup target add wasm32-unknown-unknown)");
        return None;
    }
    Some(target.join("wasm32-unknown-unknown/release/posterize_bands.wasm"))
}

#[test]
fn example_wasm_plugin_loads_and_renders() {
    let Some(wasm) = build_example() else { return };
    let (mut s, l) = setup();
    let r = s.execute("effect.plugins.load", json!({"path": wasm.to_string_lossy()})).unwrap();
    assert_eq!(r["loaded"][0]["id"], "org.effectcraft.example.posterize-bands", "{r}");
    s.execute("effect.apply", json!({"layer": l, "effect": "Posterize Bands"})).unwrap();
    s.execute("prop.set", json!({"layer": l, "path": "effects/#1/mix", "value": 0})).unwrap();
    s.execute("prop.set", json!({"layer": l, "path": "effects/#1/levels", "value": 2})).unwrap();
    // Luminance of (1, .5, 0) is ~0.57: with 2 levels it bands to 1, scaling the colour by 1/y
    // (8 bpc: red clips at 1).
    let p = px(&s);
    let y = 0.2126 + 0.7152 * 0.5;
    assert!((p[1] - 0.5 / y as f32).abs() < 0.01 && p[0] > 0.99 && p[2] < 0.01, "{p:?}");
    // Invert: band 0.
    s.execute("prop.set", json!({"layer": l, "path": "effects/#1/invert", "value": true})).unwrap();
    assert_eq!(px(&s)[..3], [0.0, 0.0, 0.0]);
    let list = s.execute("effect.plugins.list", json!({})).unwrap();
    assert!(list["plugins"].as_array().unwrap().iter().any(|p| p["source"].as_str().unwrap_or("").ends_with(".wasm")));
}

/// A parameterless plug-in module (WebAssembly text): `render` is `ec_render`'s body.
fn wat_plugin(id: &str, api: i32, render: &str) -> String {
    let manifest = format!(r#"{{"api":1,"id":"{id}","name":"Host {id}","category":"Test Plug-ins","version":"1","params":[]}}"#);
    let len = manifest.len();
    let escaped = manifest.replace('"', "\\\"");
    format!(
        r#"(module
  (memory (export "memory") 2)
  (data (i32.const 16) "{escaped}")
  (func (export "ec_api_version") (result i32) i32.const {api})
  (func (export "ec_manifest_ptr") (result i32) i32.const 16)
  (func (export "ec_manifest_len") (result i32) i32.const {len})
  (func (export "ec_alloc") (param i32) (result i32) i32.const 1024)
  (func (export "ec_render") (param i32 i32 i32 i32 i32 f64 f64) (result i32) {render}))"#
    )
}

#[test]
fn plugin_loading_reports_bad_modules_and_survives_failing_renders() {
    let dir = std::env::temp_dir().join(format!("effectcraft-host-plugins-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let files = [
        ("a-good.wasm", wat_plugin("org.test.host.good", 1, "i32.const 0").into_bytes()),
        ("b-garbage.wasm", b"\0asm\x01\0\0\0garbage".to_vec()),
        ("c-v2.wasm", wat_plugin("org.test.host.v2", 2, "i32.const 0").into_bytes()),
        ("d-empty.wasm", vec![]),
        ("e-trap.wasm", wat_plugin("org.test.host.trap", 1, "unreachable").into_bytes()),
        ("notes.txt", b"not a plug-in".to_vec()),
    ];
    for (name, bytes) in &files {
        std::fs::write(dir.join(name), bytes).unwrap();
    }
    let (mut s, l) = setup();
    let r = s.execute("effect.plugins.load", json!({"folder": dir.to_string_lossy()})).unwrap();
    let ids: Vec<&str> = r["loaded"].as_array().unwrap().iter().filter_map(|v| v["id"].as_str()).collect();
    assert_eq!(ids, ["org.test.host.good", "org.test.host.trap"], "{r}");
    let errs: Vec<String> = r["errors"].as_array().unwrap().iter().map(|e| e["path"].as_str().unwrap().to_string()).collect();
    assert_eq!(errs.len(), 3, "{r}");
    assert!(errs.iter().all(|p| !p.ends_with(".txt")));
    assert!(r["errors"].to_string().contains("API 2"), "{r}");
    // A single file: errors come back as command errors; loading twice is refused.
    let good = dir.join("a-good.wasm").to_string_lossy().to_string();
    assert!(s.execute("effect.plugins.load", json!({"path": good})).unwrap_err().to_string().contains("already registered"));
    assert!(s.execute("effect.plugins.load", json!({"path": dir.join("missing.wasm").to_string_lossy()})).is_err());
    assert!(s.execute("effect.plugins.load", json!({"folder": dir.join("missing").to_string_lossy()})).is_err());
    assert!(s.execute("effect.plugins.load", json!({})).is_err());
    // A plug-in that traps on every frame leaves the layer as it was.
    let before = px(&s);
    s.execute("effect.apply", json!({"layer": l, "effect": "org.test.host.trap"})).unwrap();
    assert_eq!(px(&s), before);
    s.execute("effect.apply", json!({"layer": l, "effect": "org.test.host.good"})).unwrap();
    assert_eq!(px(&s), before);
    let _ = std::fs::remove_dir_all(&dir);
}
