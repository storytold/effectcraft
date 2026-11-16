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
        params: vec![PluginParam { id: "on".into(), name: "On".into(), kind: PluginParamKind::Checkbox { default: true }, group: String::new() }],
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
    assert_eq!(list["api"], 2);
    assert!(s.execute("effect.list", json!({"filter": "swap red"})).unwrap().as_array().unwrap().len() == 1);
}

/// Fills the frame with its `ramp` gradient's first colour stop, read from the JSON text.
struct FirstStop {
    m: PluginManifest,
}

impl EffectPlugin for FirstStop {
    fn manifest(&self) -> &PluginManifest {
        &self.m
    }
    fn render(&self, frame: &mut PluginFrame, params: &PluginParams, _time: f64) -> Result<(), String> {
        let g: serde_json::Value = serde_json::from_str(params.text("ramp")).map_err(|e| e.to_string())?;
        if g["opacities"].as_array().is_none_or(Vec::is_empty) {
            return Err("no opacity stops".into());
        }
        let c = &g["colors"][0][1];
        let c = [0, 1, 2, 3].map(|i| c[i].as_f64().unwrap_or(-1.0) as f32);
        for p in frame.pixels.iter_mut() {
            *p = c;
        }
        Ok(())
    }
}

fn gradient_manifest(id: &str, default: Option<serde_json::Value>) -> PluginManifest {
    PluginManifest {
        api: 2,
        id: id.into(),
        name: format!("First Stop {id}"),
        category: "Generate".into(),
        version: "1".into(),
        author: String::new(),
        description: String::new(),
        params: vec![PluginParam { id: "ramp".into(), name: "Ramp".into(), kind: PluginParamKind::Gradient { default }, group: String::new() }],
    }
}

/// A gradient parameter is a gradient property (default, set and keyframed like a shape's) that
/// reaches the plug-in as JSON; a default that isn't a gradient is refused.
#[test]
fn plugins_take_gradient_parameters() {
    register_plugin(Arc::new(FirstStop { m: gradient_manifest("org.test.firststop", Some(json!(["#ff0000", "#0000ff"]))) })).unwrap();
    let (mut s, l) = setup();
    s.execute("effect.apply", json!({"layer": l, "effect": "org.test.firststop"})).unwrap();
    let p = px(&s);
    assert!(p[0] > 0.99 && p[1] < 0.01 && p[2] < 0.01, "{p:?}");
    let v = s.execute("prop.get", json!({"layer": l, "path": "effects/#1/ramp"})).unwrap();
    assert!(v.to_string().contains("colors"), "{v}");
    s.execute("prop.set", json!({"layer": l, "path": "effects/#1/ramp", "value": [[0.0, "#00ff00"], [1.0, "#ffffff"]]})).unwrap();
    let p = px(&s);
    assert!(p[0] < 0.01 && p[1] > 0.99 && p[2] < 0.01, "{p:?}");
    // keyframed: a key at 0 s (green) and 1 s (blue), halfway between at 0.5 s
    s.execute("prop.addKey", json!({"layer": l, "path": "effects/#1/ramp", "time": 0.0, "value": ["#00ff00", "#ffffff"]})).unwrap();
    s.execute("prop.addKey", json!({"layer": l, "path": "effects/#1/ramp", "time": 1.0, "value": ["#0000ff", "#ffffff"]})).unwrap();
    let cid = s.active_comp_id().unwrap();
    let p = s.render(cid, Tick::from_seconds_f64(0.5), Default::default()).get(20, 10);
    assert!((p[1] - 0.5).abs() < 0.02 && (p[2] - 0.5).abs() < 0.02, "{p:?}");
    let bad = register_plugin(Arc::new(FirstStop { m: gradient_manifest("org.test.badramp", Some(json!(["#ff0000"]))) })).err();
    assert!(bad.as_deref().is_some_and(|e| e.contains("gradient `ramp`")), "{bad:?}");
    // stops outside 0–1 are refused too
    let out = register_plugin(Arc::new(FirstStop { m: gradient_manifest("org.test.outramp", Some(json!([[-0.5, "#ff0000"], [1.5, "#0000ff"]]))) })).err();
    assert!(out.as_deref().is_some_and(|e| e.contains("gradient `ramp`")), "{out:?}");
    // no default: white to black
    register_plugin(Arc::new(FirstStop { m: gradient_manifest("org.test.plainramp", None) })).unwrap();
}

/// Records the parameter digest each render sees.
struct Digests {
    m: PluginManifest,
}

static SEEN: std::sync::Mutex<Vec<Option<u64>>> = std::sync::Mutex::new(Vec::new());

impl EffectPlugin for Digests {
    fn manifest(&self) -> &PluginManifest {
        &self.m
    }
    fn render(&self, _frame: &mut PluginFrame, _params: &PluginParams, _time: f64) -> Result<(), String> {
        Ok(())
    }
    fn render_v2(
        &self,
        _frame: &mut PluginFrame,
        _params: &PluginParams,
        _time: f64,
        host: &dyn effectcraft_engine::effects::plugin::PluginHost,
    ) -> Result<(), String> {
        SEEN.lock().unwrap().push(host.params_digest());
        Ok(())
    }
    // (every frame renders: no cached frame hides a call)
    fn reads_other_times(&self) -> bool {
        true
    }
}

/// A plug-in sees a digest of its parameter data: the same on every frame while nothing is
/// edited, different after an edit, and absent while a parameter has an expression (helpers then
/// re-read their parameter history).
#[test]
fn plugins_see_a_params_digest() {
    let m = PluginManifest {
        api: 2,
        id: "org.test.digest".into(),
        name: "Digest".into(),
        category: "Utility".into(),
        version: "1".into(),
        author: String::new(),
        description: String::new(),
        params: vec![PluginParam {
            id: "amount".into(),
            name: "Amount".into(),
            group: String::new(),
            kind: PluginParamKind::Slider { default: 1.0, min: 0.0, max: 10.0, slider_min: None, slider_max: None, decimals: 2, integer: false },
        }],
    };
    register_plugin(Arc::new(Digests { m })).unwrap();
    let (mut s, l) = setup();
    s.execute("effect.apply", json!({"layer": l, "effect": "org.test.digest"})).unwrap();
    let cid = s.active_comp_id().unwrap();
    let at = |s: &Session, t: f64| {
        let n = SEEN.lock().unwrap().len();
        s.render(cid, Tick::from_seconds_f64(t), Default::default());
        let seen = SEEN.lock().unwrap();
        assert!(seen.len() > n, "the render at {t} called the plug-in");
        *seen.last().unwrap()
    };
    let first = at(&s, 0.0);
    assert!(first.is_some());
    assert_eq!(at(&s, 0.5), first, "the same digest while nothing is edited");
    s.execute("prop.set", json!({"layer": l, "path": "effects/#1/amount", "value": 2.0})).unwrap();
    let edited = at(&s, 0.6);
    assert!(edited.is_some() && edited != first);
    assert_eq!(at(&s, 0.7), edited);
    // At another render scale `params_at` flattens points differently: so does the digest.
    let n = SEEN.lock().unwrap().len();
    s.render(cid, Tick::from_seconds_f64(0.7), effectcraft_engine::render::RenderOpts { scale: 0.5, ..Default::default() });
    let half = *SEEN.lock().unwrap().get(n).expect("the half-resolution render called the plug-in");
    assert!(half.is_some() && half != edited, "{half:?} vs {edited:?}");
    s.execute("prop.setExpression", json!({"layer": l, "path": "effects/#1/amount", "expression": "time"})).unwrap();
    assert_eq!(at(&s, 0.8), None);
}

/// Build the example plug-in (skipped when the wasm32 target isn't installed).
fn build_example() -> Option<std::path::PathBuf> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/plugins/posterize-bands");
    let workspace = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let assigned = std::env::var_os("CARGO_TARGET_DIR");
    let target = assigned.as_ref().map(std::path::PathBuf::from).unwrap_or_else(|| workspace.join("target"));
    let target = if target.is_absolute() { target } else { workspace.join(target) };
    // The plug-in is part of this test's build. Inherit the assigned target and
    // compiler guards rather than creating a second target at its root.
    let mut command = std::process::Command::new(env!("CARGO"));
    command.current_dir(&workspace).args(["build", "--release", "--target", "wasm32-unknown-unknown", "--manifest-path"]).arg(root.join("Cargo.toml"));
    if assigned.is_none() {
        command.env("CARGO_TARGET_DIR", &target);
    }
    let st = command.status().ok()?;
    if !st.success() {
        eprintln!("skipping: the example plug-in didn't build (rustup target add wasm32-unknown-unknown)");
        return None;
    }
    Some(target.join("wasm32-unknown-unknown/release/posterize_bands.wasm"))
}

/// A host session whose settings and Plug-ins folder are both in `config`: installs must never
/// reach the real settings folder the host session points its Plug-ins folder at.
fn install_session(config: &std::path::Path) -> Session {
    let mut s = effectcraft_host::session();
    s.config = Some(Arc::new(effectcraft_engine::config::DirConfig::new(config)));
    s.plugins_dir = Some(config.join("Plug-ins"));
    s
}

#[test]
fn plugin_bundle_install_copies_wasm_and_installs_panels() {
    let config = tempfile::TempDir::new().unwrap();
    let config = config.path();
    let plug_dir = config.join("Plug-ins");
    let bundle = tempfile::TempDir::new().unwrap();
    let bundle = bundle.path();
    std::fs::write(bundle.join("a-good.wasm"), wat_plugin("org.test.host.install", 1, "i32.const 0")).unwrap();
    std::fs::write(bundle.join("My Panel.jsx"), "1".as_bytes()).unwrap();
    std::fs::write(bundle.join("notes.txt"), b"not a plug-in").unwrap();

    let mut s = install_session(config);
    let r = s.execute("effect.plugins.install", json!({"folder": bundle.to_string_lossy()})).unwrap();
    assert_eq!(r["installed"].as_array().unwrap().len(), 1, "{r}");
    assert_eq!(r["errors"].as_array().unwrap().len(), 0, "{r}");
    assert_eq!(r["panels"].as_array().unwrap()[0], json!("My Panel.jsx"), "{r}");
    // The plug-in is on disk where the start-up scan looks, and the panel is listable like an
    // installed one.
    assert!(plug_dir.join("a-good.wasm").is_file(), "{r}");
    assert!(s.config.as_ref().unwrap().read("Scripts/ScriptUI Panels/My Panel.jsx").as_deref() == Some("1"), "{r}");
    // The effect is usable in this session already.
    s.execute("comp.new", json!({"name": "I", "width": 40, "height": 20, "duration": 1})).unwrap();
    let l = s.execute("layer.newSolid", json!({"color": [1.0, 0.5, 0.0], "width": 40, "height": 20})).unwrap()["layer"].as_u64().unwrap();
    s.execute("effect.apply", json!({"layer": l, "effect": "Host org.test.host.install"})).unwrap();

    // A load failure rolls the copy back: nothing left behind to fail every start.
    let bad = tempfile::TempDir::new().unwrap();
    let bad = bad.path();
    std::fs::write(bad.join("b-garbage.wasm"), b"\0asm\x01\0\0\0garbage").unwrap();
    let r2 = s.execute("effect.plugins.install", json!({"folder": bad.to_string_lossy()})).unwrap();
    assert_eq!(r2["errors"].as_array().unwrap().len(), 1, "{r2}");
    assert!(r2["installed"].as_array().unwrap().is_empty(), "{r2}");
    assert!(!plug_dir.join("b-garbage.wasm").exists(), "{r2}");

    // A single-file install: non-wasm paths are refused, and good files land on disk.
    assert!(s.execute("effect.plugins.install", json!({"path": bundle.join("My Panel.jsx").to_string_lossy()})).is_err());
    // A single-file success: the file lands in the Plug-ins folder and is reported installed.
    let fresh = bundle.join("fresh.wasm");
    std::fs::write(&fresh, wat_plugin("org.test.host.fresh", 1, "i32.const 0")).unwrap();
    let r3 = s.execute("effect.plugins.install", json!({"path": fresh.to_string_lossy()})).unwrap();
    assert_eq!(r3["installed"].as_array().unwrap()[0], json!("fresh.wasm"), "{r3}");
    assert!(plug_dir.join("fresh.wasm").is_file(), "{r3}");
    // Again from `config`: an already-registered (in-session) id fails as a duplicate.
    let again = config.join("keep.wasm");
    std::fs::copy(bundle.join("a-good.wasm"), &again).unwrap();
    assert!(s.execute("effect.plugins.install", json!({"path": again.to_string_lossy()})).is_err());
}

#[test]
fn plugin_install_never_replaces_an_installed_plugin() {
    let config = tempfile::TempDir::new().unwrap();
    let config = config.path();
    let plug_dir = config.join("Plug-ins");
    let src = tempfile::TempDir::new().unwrap();
    let src = src.path();
    std::fs::write(src.join("a-good.wasm"), wat_plugin("org.test.host.replace", 1, "i32.const 0")).unwrap();

    let mut s = install_session(config);
    s.execute("effect.plugins.install", json!({"path": src.join("a-good.wasm").to_string_lossy()})).unwrap();
    let installed = std::fs::read(plug_dir.join("a-good.wasm")).unwrap();

    // Same file name, a different plug-in: installing over the existing one must be refused,
    // not quietly swap it — and the first plug-in's bytes on disk stay exactly what they were.
    let attack = tempfile::TempDir::new().unwrap();
    let attack = attack.path();
    std::fs::write(attack.join("a-good.wasm"), wat_plugin("org.test.host.hack", 1, "i32.const 0")).unwrap();
    let r = s.execute("effect.plugins.install", json!({"folder": attack.to_string_lossy()})).unwrap();
    assert!(r["installed"].as_array().unwrap().is_empty(), "{r}");
    let errs: Vec<String> = r["errors"].as_array().unwrap().iter().map(|e| e["error"].as_str().unwrap().to_string()).collect();
    assert!(errs.iter().any(|e| e.contains("already installed")), "{r}");
    assert_eq!(std::fs::read(plug_dir.join("a-good.wasm")).unwrap(), installed);

    // The same through a single-file install: a command error, disk untouched.
    assert!(s.execute("effect.plugins.install", json!({"path": attack.join("a-good.wasm").to_string_lossy()})).is_err());
    assert_eq!(std::fs::read(plug_dir.join("a-good.wasm")).unwrap(), installed);

    // A ScriptUI panel is code too: a bundle must not replace an installed one silently either.
    let p1 = tempfile::TempDir::new().unwrap();
    std::fs::write(p1.path().join("My Panel.jsx"), "1").unwrap();
    s.execute("effect.plugins.install", json!({"folder": p1.path().to_string_lossy()})).unwrap();
    let p2 = tempfile::TempDir::new().unwrap();
    std::fs::write(p2.path().join("My Panel.jsx"), "2").unwrap();
    let r = s.execute("effect.plugins.install", json!({"folder": p2.path().to_string_lossy()})).unwrap();
    assert!(r["panels"].as_array().unwrap().is_empty(), "{r}");
    let errs: Vec<String> = r["errors"].as_array().unwrap().iter().map(|e| e["error"].as_str().unwrap().to_string()).collect();
    assert!(errs.iter().any(|e| e.contains("already installed as a ScriptUI panel")), "{r}");
    assert_eq!(s.config.as_ref().unwrap().read("Scripts/ScriptUI Panels/My Panel.jsx").as_deref(), Some("1"));
}

/// A single install of a hidden file is an error, not a success that installed nothing; a panel
/// whose name differs from an installed one only in case is refused (on Windows and macOS writing
/// it would replace that panel); an installed plug-in's source is its installed copy.
#[test]
fn plugin_install_refuses_hidden_files_and_case_variant_panels() {
    let config = tempfile::TempDir::new().unwrap();
    let config = config.path();
    let src = tempfile::TempDir::new().unwrap();
    let src = src.path();
    let mut s = install_session(config);

    let hidden = src.join(".hidden.wasm");
    std::fs::write(&hidden, wat_plugin("org.test.host.hidden", 1, "i32.const 0")).unwrap();
    assert!(s.execute("effect.plugins.install", json!({"path": hidden.to_string_lossy()})).is_err());
    assert!(!config.join("Plug-ins").join(".hidden.wasm").exists());

    let p1 = tempfile::TempDir::new().unwrap();
    std::fs::write(p1.path().join("Case Panel.jsx"), "1").unwrap();
    s.execute("effect.plugins.install", json!({"folder": p1.path().to_string_lossy()})).unwrap();
    let p2 = tempfile::TempDir::new().unwrap();
    std::fs::write(p2.path().join("case panel.jsx"), "2").unwrap();
    let r = s.execute("effect.plugins.install", json!({"folder": p2.path().to_string_lossy()})).unwrap();
    assert!(r["panels"].as_array().unwrap().is_empty(), "{r}");
    assert_eq!(r["errors"].as_array().unwrap().len(), 1, "{r}");
    assert_eq!(s.config.as_ref().unwrap().read("Scripts/ScriptUI Panels/Case Panel.jsx").as_deref(), Some("1"));

    let good = src.join("sourced.wasm");
    std::fs::write(&good, wat_plugin("org.test.host.sourced", 1, "i32.const 0")).unwrap();
    s.execute("effect.plugins.install", json!({"path": good.to_string_lossy()})).unwrap();
    let list = s.execute("effect.plugins.list", json!({})).unwrap();
    let entry = list["plugins"].as_array().unwrap().iter().find(|p| p["id"] == "org.test.host.sourced").cloned().unwrap();
    assert_eq!(entry["source"], json!(config.join("Plug-ins").join("sourced.wasm").to_string_lossy()), "{entry}");
}

/// A huge file is refused before it is read into memory and copied into the folder the app loads
/// at every start.
#[test]
fn plugin_install_refuses_an_oversized_file() {
    let config = tempfile::TempDir::new().unwrap();
    let src = tempfile::TempDir::new().unwrap();
    let big = src.path().join("big.wasm");
    // Sparse on every platform we build for: no 64 MiB written, just the length.
    std::fs::File::create(&big).unwrap().set_len((64 << 20) + 1).unwrap();

    let mut s = install_session(config.path());
    let Err(e) = s.execute("effect.plugins.install", json!({"path": big.to_string_lossy()})) else { panic!("an oversized plug-in was installed") };
    assert!(e.to_string().contains("byte limit"), "{e}");
    assert!(!config.path().join("Plug-ins/big.wasm").exists(), "{e}");
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
pub(super) fn wat_plugin(id: &str, api: i32, render: &str) -> String {
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
    let guard = tempfile::tempdir().unwrap();
    let dir = guard.path().to_path_buf();
    let files = [
        ("a-good.wasm", wat_plugin("org.test.host.good", 1, "i32.const 0").into_bytes()),
        ("b-garbage.wasm", b"\0asm\x01\0\0\0garbage".to_vec()),
        ("c-v3.wasm", wat_plugin("org.test.host.v3", 3, "i32.const 0").into_bytes()),
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
    assert!(r["errors"].to_string().contains("ABI 3"), "{r}");
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

/// An ABI 2 plug-in (WebAssembly text) that shows the neighbouring frames it got: per pixel it
/// writes |next.red − prev.red| into red, `flags / 8` into green, `prev_time` into blue and 1
/// into alpha, so a render test can read the frames and their times back from the image.
fn wat_neighbour_plugin(id: &str) -> String {
    let manifest = format!(r#"{{"api":2,"id":"{id}","name":"Host {id}","category":"Test Plug-ins","version":"1","params":[]}}"#);
    let len = manifest.len();
    let escaped = manifest.replace('"', "\\\"");
    format!(
        r#"(module
  (memory (export "memory") 2)
  (data (i32.const 16) "{escaped}")
  (func (export "ec_api_version") (result i32) i32.const 2)
  (func (export "ec_manifest_ptr") (result i32) i32.const 16)
  (func (export "ec_manifest_len") (result i32) i32.const {len})
  (func (export "ec_alloc") (param i32) (result i32) i32.const 1024)
  (func (export "ec_render2")
    (param $px i32) (param $w i32) (param $h i32) (param $params i32) (param $np i32)
    (param $t f64) (param $scale f64)
    (param $prev i32) (param $prev_t f64) (param $next i32) (param $next_t f64) (param $flags i32)
    (result i32)
    (local $i i32)
    (local $n i32)
    local.get $w
    local.get $h
    i32.mul
    local.set $n
    block $done
      loop $per_pixel
        local.get $i
        local.get $n
        i32.ge_u
        br_if $done
        ;; red = |next.red - prev.red|
        local.get $px
        local.get $i
        i32.const 16
        i32.mul
        i32.add
        local.get $next
        local.get $i
        i32.const 16
        i32.mul
        i32.add
        f32.load
        local.get $prev
        local.get $i
        i32.const 16
        i32.mul
        i32.add
        f32.load
        f32.sub
        f32.abs
        f32.store
        ;; green = flags / 8
        local.get $px
        local.get $i
        i32.const 16
        i32.mul
        i32.add
        local.get $flags
        f32.convert_i32_s
        f32.const 8
        f32.div
        f32.store offset=4
        ;; blue = prev_time
        local.get $px
        local.get $i
        i32.const 16
        i32.mul
        i32.add
        local.get $prev_t
        f32.demote_f64
        f32.store offset=8
        ;; alpha = 1
        local.get $px
        local.get $i
        i32.const 16
        i32.mul
        i32.add
        f32.const 1
        f32.store offset=12
        local.get $i
        i32.const 1
        i32.add
        local.set $i
        br $per_pixel
      end
    end
    i32.const 0))"#
    )
}

/// The neighbouring frames are the layer's own pixels at other times: here a precomp whose
/// nested solid fades from 0 to 100 % over one second, at 4 fps. At 0.5 s the neighbours are
/// 0.25 s and 0.75 s, so red (the current frame) was 0.5 and |next − prev| is 0.5.
#[test]
fn abi_v2_plugins_render_with_real_neighbour_frames() {
    let mut s = effectcraft_host::session();
    let inner = s.execute("comp.new", json!({"name": "Inner", "width": 40, "height": 20, "frameRate": 4, "duration": 1})).unwrap()["comp"].as_u64().unwrap();
    let solid = s.execute("layer.newSolid", json!({"color": [1.0, 0.0, 0.0], "width": 40, "height": 20})).unwrap()["layer"].as_u64().unwrap();
    s.execute("prop.addKey", json!({"layer": solid, "path": "transform/opacity", "time": 0.0, "value": 0})).unwrap();
    s.execute("prop.addKey", json!({"layer": solid, "path": "transform/opacity", "time": 1.0, "value": 100})).unwrap();
    let outer = s.execute("comp.new", json!({"name": "Outer", "width": 40, "height": 20, "frameRate": 4, "duration": 1})).unwrap()["comp"].as_u64().unwrap();
    let layer = s.execute("layer.addItem", json!({"comp": outer, "item": inner, "time": 0})).unwrap()["layer"].as_u64().unwrap();
    let outer = effectcraft_engine::project::ItemId(outer);

    let dir = tempfile::TempDir::new().unwrap();
    let wasm = dir.path().join("neighbours.wasm");
    std::fs::write(&wasm, wat_neighbour_plugin("org.test.host.neighbours")).unwrap();
    let r = s.execute("effect.plugins.load", json!({"path": wasm.to_string_lossy()})).unwrap();
    assert_eq!(r["loaded"][0]["id"], "org.test.host.neighbours", "{r}");
    s.execute("effect.apply", json!({"layer": layer, "effect": "org.test.host.neighbours"})).unwrap();

    let p = s.render(outer, Tick::from_seconds_f64(0.5), Default::default()).get(20, 10);
    assert!((p[0] - 0.5).abs() < 0.02, "precomp neighbours: {p:?}");
    // Both neighbours exist, the previous one is the 0.25 s frame, and the frame is opaque.
    assert!((p[1] - 3.0 / 8.0).abs() < 0.01, "flags: {p:?}");
    assert!((p[2] - 0.25).abs() < 0.01, "prev time: {p:?}");
    assert_eq!(p[3], 1.0, "{p:?}");

    // At the very start of the layer only the next frame exists (the previous one is zeroed);
    // solids are filled with a quantised colour, hence the 8-bit step.
    let p = s.render(outer, Tick::ZERO, Default::default()).get(20, 10);
    assert!((p[0] - 0.25).abs() < 0.01, "the next frame's red, with no previous frame: {p:?}");
    assert!((p[1] - 2.0 / 8.0).abs() < 0.01, "only the next frame is valid: {p:?}");
}

/// Records the comp camera it is handed.
struct CameraProbe {
    m: PluginManifest,
    seen: Arc<std::sync::Mutex<Option<effectcraft_engine::effects::plugin::CompCamera>>>,
}

impl EffectPlugin for CameraProbe {
    fn manifest(&self) -> &PluginManifest {
        &self.m
    }
    fn render(&self, _frame: &mut PluginFrame, _params: &PluginParams, _time: f64) -> Result<(), String> {
        Ok(())
    }
    fn render_v2(
        &self,
        _frame: &mut PluginFrame,
        _params: &PluginParams,
        _time: f64,
        host: &dyn effectcraft_engine::effects::plugin::PluginHost,
    ) -> Result<(), String> {
        *self.seen.lock().unwrap() = host.comp_camera();
        Ok(())
    }
}

/// `comp_camera`'s W is the depth in front of the camera (what `zoom / W` scales by), for a 2D
/// layer and a 3D one alike: a 3D layer's own projection would otherwise leave W = 1 on it.
#[test]
fn comp_camera_w_is_the_depth_for_2d_and_3d_layers() {
    let seen = Arc::new(std::sync::Mutex::new(None));
    let m = PluginManifest {
        api: 2,
        id: "org.test.cameraprobe".into(),
        name: "Camera Probe".into(),
        category: "Test Plug-ins".into(),
        version: "1".into(),
        author: String::new(),
        description: String::new(),
        params: vec![],
    };
    register_plugin(Arc::new(CameraProbe { m, seen: seen.clone() })).unwrap();
    let (mut s, l) = setup();
    s.execute("effect.apply", json!({"layer": l, "effect": "org.test.cameraprobe"})).unwrap();
    // W and where the layer centre (20, 10) shows, in the layer's pixels
    let mut n = 0;
    let mut look = |s: &Session| {
        n += 1;
        let cid = s.active_comp_id().unwrap();
        let _ = s.render(cid, Tick::ZERO, Default::default());
        let c = seen.lock().unwrap().take().unwrap_or_else(|| panic!("no camera handed over (step {n})"));
        let at = |r: [f64; 4]| r[0] * 20.0 + r[1] * 10.0 + r[3];
        let w = at(c.matrix[2]);
        (w, c.zoom, at(c.matrix[0]) / w, at(c.matrix[1]) / w)
    };
    let (w, zoom, x, y) = look(&s);
    assert!(zoom > 0.0 && (w - zoom).abs() < 1e-6 * zoom, "2D: the default camera is zoom away: {w} vs {zoom}");
    assert!((x - 20.0).abs() < 1e-6 && (y - 10.0).abs() < 1e-6, "{x}, {y}");
    s.execute("layer.setSwitch", json!({"layers": [l], "switch": "threeD", "value": true})).unwrap();
    let (w, _, x, y) = look(&s);
    assert!((w - zoom).abs() < 1e-6 * zoom, "3D at z = 0: {w} vs {zoom}");
    assert!((x - 20.0).abs() < 1e-6 && (y - 10.0).abs() < 1e-6, "{x}, {y}");
    s.execute("prop.set", json!({"layer": l, "path": "transform/position", "value": [20.0, 10.0, zoom]})).unwrap();
    let (w, _, x, y) = look(&s);
    assert!((w - 2.0 * zoom).abs() < 1e-6 * zoom, "3D, one zoom further back: {w} vs {}", 2.0 * zoom);
    assert!((x - 20.0).abs() < 1e-6 && (y - 10.0).abs() < 1e-6, "{x}, {y}");
}

/// A generator: ignores its input, scribbles green over the whole frame, draws a red square at
/// x 10..14, y 5..9 and says only that square is its result. Notes the first pixel it was given.
struct Dot {
    m: PluginManifest,
}

static DOT_SAW: std::sync::Mutex<Vec<[f32; 4]>> = std::sync::Mutex::new(vec![]);

impl EffectPlugin for Dot {
    fn manifest(&self) -> &PluginManifest {
        &self.m
    }
    fn render(&self, _: &mut PluginFrame, _: &PluginParams, _: f64) -> Result<(), String> {
        Err("needs render_bounded".into())
    }
    fn ignores_input(&self) -> bool {
        true
    }
    fn render_bounded(
        &self,
        frame: &mut PluginFrame,
        _: &PluginParams,
        _: f64,
        _: &dyn effectcraft_engine::effects::plugin::PluginHost,
    ) -> Result<Option<[u32; 4]>, String> {
        DOT_SAW.lock().unwrap().push(frame.pixels[0]);
        let w = frame.width as usize;
        for (i, p) in frame.pixels.iter_mut().enumerate() {
            let (x, y) = (i % w, i / w);
            *p = if (10..14).contains(&x) && (5..9).contains(&y) { [1.0, 0.0, 0.0, 1.0] } else { [0.0, 1.0, 0.0, 1.0] };
        }
        Ok(Some([10, 5, 14, 9]))
    }
}

fn manifest(id: &str, name: &str) -> PluginManifest {
    PluginManifest {
        api: 2,
        id: id.into(),
        name: name.into(),
        category: "Generate".into(),
        version: "1".into(),
        author: String::new(),
        description: String::new(),
        params: vec![PluginParam { id: "on".into(), name: "On".into(), kind: PluginParamKind::Checkbox { default: true }, group: String::new() }],
    }
}

/// A generator plug-in that says where it drew: alone on a solid it gets a blank buffer (the
/// solid isn't filled for it) and only its square shows; with an effect after it the rest is
/// cleared before that effect runs; after another effect it sees the solid.
#[test]
fn bounded_generators_keep_only_what_they_drew() {
    register_plugin(Arc::new(Dot { m: manifest("org.test.dot", "Dot") })).unwrap();
    register_plugin(Arc::new(Swap { m: manifest("org.test.swap2", "Swap Again") })).unwrap();
    let cid_px = |s: &Session, x: i64, y: i64| s.render(s.active_comp_id().unwrap(), Tick::ZERO, Default::default()).get(x, y);
    let saw = || *DOT_SAW.lock().unwrap().last().unwrap();

    let (mut s, l) = setup();
    s.execute("effect.apply", json!({"layer": l, "effect": "org.test.dot"})).unwrap();
    assert_eq!(cid_px(&s, 11, 6), [1.0, 0.0, 0.0, 1.0], "its square");
    assert_eq!(cid_px(&s, 20, 15), [0.0; 4], "nothing outside it");
    assert_eq!(cid_px(&s, 5, 6), [0.0; 4], "nor beside it, where the grid-aligned cut starts");
    assert_eq!(saw(), [0.0; 4], "a blank buffer, not the orange solid");

    // an effect after it sees the square on transparency
    s.execute("effect.apply", json!({"layer": l, "effect": "org.test.swap2"})).unwrap();
    assert_eq!(cid_px(&s, 11, 6), [0.0, 0.0, 1.0, 1.0], "its square, swapped");
    assert_eq!(cid_px(&s, 20, 15), [0.0; 4], "cleared outside it");

    // CC Composite after it reads the original layer, so the solid is filled for it
    let (mut s, l) = setup();
    s.execute("effect.apply", json!({"layer": l, "effect": "org.test.dot"})).unwrap();
    s.execute("effect.apply", json!({"layer": l, "effect": "ec.channel.cccomposite"})).unwrap();
    let _ = cid_px(&s, 0, 0);
    assert_eq!(saw()[3], 1.0, "the solid CC Composite needs, not a blank buffer: {:?}", saw());

    // after another effect it gets the layer as that effect left it
    let (mut s, l) = setup();
    s.execute("effect.apply", json!({"layer": l, "effect": "org.test.swap2"})).unwrap();
    s.execute("effect.apply", json!({"layer": l, "effect": "org.test.dot"})).unwrap();
    assert_eq!(cid_px(&s, 20, 15), [0.0; 4]);
    let p = saw();
    assert!(p[2] > 0.99 && (p[1] - 0.5).abs() < 0.01 && p[3] == 1.0, "the swapped solid: {p:?}");
}

/// A generator (it ignores its input) that always fails.
struct Broken {
    m: PluginManifest,
}

impl EffectPlugin for Broken {
    fn manifest(&self) -> &PluginManifest {
        &self.m
    }
    fn render(&self, _: &mut PluginFrame, _: &PluginParams, _: f64) -> Result<(), String> {
        Err("broken".into())
    }
    fn ignores_input(&self) -> bool {
        true
    }
}

/// A failing generator first on a solid passes the solid through, as any failed effect passes
/// its input, although the solid isn't filled for a generator that works.
#[test]
fn a_failed_generator_shows_the_solid() {
    register_plugin(Arc::new(Broken { m: manifest("org.test.broken", "Broken") })).unwrap();
    let (mut s, l) = setup();
    s.execute("effect.apply", json!({"layer": l, "effect": "org.test.broken"})).unwrap();
    let p = px(&s);
    assert!(p[0] > 0.99 && (p[1] - 0.5).abs() < 0.01 && p[2] < 0.01 && p[3] == 1.0, "the orange solid: {p:?}");
}
