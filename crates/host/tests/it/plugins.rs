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
