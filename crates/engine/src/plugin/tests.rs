//! The shared plug-in runtime in EffectCraft: command plug-ins written in WebAssembly text run
//! real commands as one undo step, read-only access and refused commands are enforced, a failed
//! run leaves the project as it was, and bundles install, list and remove through the store.

use std::sync::atomic::{AtomicUsize, Ordering};

use craft_plugin::bundle::Bundle;
use serde_json::{Value, json};

use super::*;

/// A WAT string literal of `s`.
fn lit(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// How a test module ends after its calls.
#[derive(Clone, Copy, PartialEq)]
enum End {
    /// Returns 0.
    Ok,
    /// Traps (`unreachable`).
    Trap,
    /// Loops forever.
    Spin,
}

/// A command plug-in module that calls `calls` in order; a refused or failed call ends the run
/// with the host's reply as its error.
fn module(calls: &[(&str, Value)], end: End) -> Vec<u8> {
    let mut data = String::new();
    let mut body = String::new();
    let mut at = 1024;
    for (cmd, params) in calls {
        let params = params.to_string();
        let (c, p) = (at, at + cmd.len());
        at = p + params.len() + 16;
        data.push_str(&format!("  (data (i32.const {c}) \"{}\")\n  (data (i32.const {p}) \"{}\")\n", lit(cmd), lit(&params)));
        body.push_str(&format!(
            "    (if (call $call (i32.const {c}) (i32.const {}) (i32.const {p}) (i32.const {}))\n      (then\n        (local.set $n (call $reply_len))\n        (drop (call $reply_read (i32.const 32768) (local.get $n)))\n        (call $set_output (i32.const 32768) (local.get $n))\n        (return (i32.const 1))))\n",
            cmd.len(),
            params.len()
        ));
    }
    let tail = match end {
        End::Ok => "(i32.const 0)",
        End::Trap => "unreachable",
        End::Spin => "(loop $l (br $l)) (i32.const 0)",
    };
    let wat = format!(
        "(module
  (import \"craft\" \"call\" (func $call (param i32 i32 i32 i32) (result i32)))
  (import \"craft\" \"reply_len\" (func $reply_len (result i32)))
  (import \"craft\" \"reply_read\" (func $reply_read (param i32 i32) (result i32)))
  (import \"craft\" \"set_output\" (func $set_output (param i32 i32)))
  (memory (export \"memory\") 1)
{data}  (func (export \"craft_run\") (param i32) (result i32) (local $n i32)
{body}    {tail}))"
    );
    wat::parse_str(&wat).unwrap()
}

/// A bundle of `wasm` with a manifest for EffectCraft (`access`: read or write).
fn bundle(id: &str, access: &str, wasm: Vec<u8>) -> Vec<u8> {
    let manifest = json!({
        "id": id, "name": "Solids", "version": "1.0.0", "main": "plugin.wasm",
        "apps": {"effectcraft": {"api": 6}},
        "permissions": {"access": access},
        "commands": [{"id": "run", "label": "Run"}],
    });
    Bundle { manifest: manifest.to_string(), wasm, assets: vec![] }.encode()
}

/// A session with one composition and its `Plug-ins` folder in `dir`.
fn session(dir: &std::path::Path) -> Session {
    let mut s = Session { plugins_dir: Some(dir.join("Plug-ins")), ..Default::default() };
    s.execute("comp.new", json!({"name": "C", "width": 64, "height": 64})).unwrap();
    s
}

fn layers(s: &Session) -> usize {
    s.active_comp().map_or(0, |c| c.layers.len())
}

/// Install `bytes` from a file in `dir`.
fn install_file(s: &mut Session, dir: &std::path::Path, bytes: &[u8]) -> Value {
    let path = dir.join("p.craftplugin");
    std::fs::write(&path, bytes).unwrap();
    s.execute("plugin.install", json!({"path": path})).unwrap()
}

const SOLID: &str = "layer.newSolid";

#[test]
fn a_write_plugin_runs_commands_as_one_undo_step() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = session(dir.path());
    let wasm = module(&[(SOLID, json!({"name": "A", "color": "#ff0000"})), (SOLID, json!({"name": "B", "color": "#00ff00"}))], End::Ok);
    install_file(&mut s, dir.path(), &bundle("org.test.solids", "write", wasm));
    let undo = s.history.undo.len();
    let r = s.execute("plugin.run", json!({"id": "org.test.solids"})).unwrap();
    assert_eq!(r["calls"], 2, "{r}");
    assert_eq!(layers(&s), 2);
    assert_eq!(s.history.undo.len(), undo + 1, "one undo step");
    assert_eq!(s.history.undo.last().map(|(l, _)| l.as_str()), Some("Solids"));
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(layers(&s), 0, "one Undo takes the whole run back");
}

#[test]
fn a_read_plugin_that_edits_is_put_back_and_refused() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = session(dir.path());
    install_file(&mut s, dir.path(), &bundle("org.test.reader", "read", module(&[(SOLID, json!({"name": "A"}))], End::Ok)));
    let (undo, revision) = (s.history.undo.len(), s.revision);
    let e = s.execute("plugin.run", json!({"id": "org.test.reader"})).unwrap_err().to_string();
    assert!(e.contains("may only read"), "{e}");
    assert_eq!(layers(&s), 0);
    assert_eq!(s.history.undo.len(), undo);
    assert!(s.journal.iter().all(|(id, _)| id != SOLID), "nothing of the call is journaled");
    // Reading is allowed.
    install_file(&mut s, dir.path(), &bundle("org.test.reader", "read", module(&[("comp.info", json!({}))], End::Ok)));
    s.execute("plugin.run", json!({"id": "org.test.reader"})).unwrap();
    assert!(s.revision >= revision);
    assert_eq!(s.history.undo.len(), undo);
}

#[test]
fn refused_commands_are_refused() {
    let m = craft_plugin::Manifest::parse(&json!({"id": "a.b", "name": "P", "main": "p.wasm", "apps": {"effectcraft": {"api": 6}}}).to_string()).unwrap();
    for id in [
        "file.save",
        "edit.undo",
        "prefs.set",
        "script.run",
        "engine.batch",
        "library.install",
        "renderQueue.render",
        "plugin.run",
        "edit.paste",
        "effect.helpers.load",
    ] {
        assert!(refusal(&m, id, &json!({})).is_some(), "{id}");
    }
    assert!(refusal(&m, "anim.applyPreset", &json!({"path": "/tmp/x.ecpreset"})).is_some_and(|w| w.contains("can't name files")));
    assert!(refusal(&m, "layer.newSolid", &json!({"nested": {"file": "/etc/passwd"}})).is_some());
    for ok in [("prop.set", json!({"path": "transform/opacity", "value": 50})), (SOLID, json!({})), ("anim.applyPreset", json!({"preset": {}}))] {
        assert_eq!(refusal(&m, ok.0, &ok.1), None, "{}", ok.0);
    }
    // End to end: the plug-in gets the refusal, and the run fails with nothing changed.
    let dir = tempfile::tempdir().unwrap();
    let mut s = session(dir.path());
    install_file(&mut s, dir.path(), &bundle("org.test.saver", "write", module(&[(SOLID, json!({})), ("file.save", json!({}))], End::Ok)));
    let e = s.execute("plugin.run", json!({"id": "org.test.saver"})).unwrap_err().to_string();
    assert!(e.contains("isn't available to plugins"), "{e}");
    assert_eq!(layers(&s), 0, "the solid made before the refusal is rolled back");
}

#[test]
fn a_command_that_opens_a_dialog_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = session(dir.path());
    install_file(&mut s, dir.path(), &bundle("org.test.dialog", "write", module(&[("effect.manage", json!({}))], End::Ok)));
    s.drain_events();
    let e = s.execute("plugin.run", json!({"id": "org.test.dialog"})).unwrap_err().to_string();
    assert!(e.contains("opens a dialog"), "{e}");
    assert!(!s.drain_events().iter().any(|e| matches!(e, Event::Frontend { .. })), "the dialog request is dropped");
}

#[test]
fn a_trapping_or_endless_plugin_leaves_the_project_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = session(dir.path());
    for (end, why) in [(End::Trap, "crashed"), (End::Spin, "instruction budget")] {
        install_file(&mut s, dir.path(), &bundle("org.test.bad", "write", module(&[(SOLID, json!({}))], end)));
        let (undo, project) = (s.history.undo.len(), s.project.clone());
        let e = s.execute("plugin.run", json!({"id": "org.test.bad", "fuel": 200_000})).unwrap_err().to_string();
        assert!(e.contains(why), "{e}");
        assert_eq!(layers(&s), 0);
        assert!(Arc::ptr_eq(&project, &s.project), "the project is the one before the run");
        assert_eq!(s.history.undo.len(), undo);
    }
}

#[test]
fn install_list_and_remove_through_the_store() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = session(dir.path());
    let bytes = bundle("org.test.solids", "write", module(&[], End::Ok));
    let info = install_file(&mut s, dir.path(), &bytes);
    assert_eq!(info["installed"], true);
    assert_eq!(info["stored"], true);
    assert_eq!(info["sha256"], craft_plugin::sha256_hex(&bytes));
    let folder = dir.path().join("Plug-ins").join(craft_plugin::store::folder_name("org.test.solids"));
    assert!(folder.join("source.json").is_file());
    // A new session reads it from the folder.
    let mut again = session(dir.path());
    let listed = again.execute("plugin.list", json!({})).unwrap();
    assert_eq!(listed["plugins"][0]["id"], "org.test.solids", "{listed}");
    assert_eq!(listed["plugins"][0]["source"]["type"], "file");
    // The Effect ▸ Plug-ins menu lists its command.
    let (entries, _) = crate::menus::dynamic(&again, "plugins", &Default::default());
    assert_eq!(entries.first().map(|e| e.command.as_str()), Some("plugin.run"));
    again.execute("plugin.remove", json!({"id": "org.test.solids"})).unwrap();
    assert!(!folder.exists());
    assert_eq!(again.execute("plugin.list", json!({})).unwrap()["plugins"], json!([]));
    assert!(again.execute("plugin.remove", json!({"id": "org.test.solids"})).is_err());
}

#[test]
fn review_then_install_the_reviewed_bundle() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = session(dir.path());
    let bytes = bundle("org.test.solids", "write", module(&[], End::Ok));
    let data = effectcraft_track::roto::rle::base64_encode(&bytes);
    let r = s.execute("plugin.install", json!({"data": data, "review": true})).unwrap();
    assert_eq!(r["installed"], false);
    assert_eq!(r["permissions"]["access"], "write", "{r}");
    assert_eq!(r["inCatalog"], Value::Null);
    assert_eq!(s.execute("plugin.list", json!({})).unwrap()["plugins"], json!([]), "nothing is written before the install");
    let wrong = s.execute("plugin.install", json!({"data": data, "sha256": "0".repeat(64)})).unwrap_err().to_string();
    assert!(wrong.contains("review it again"), "{wrong}");
    let ok = s.execute("plugin.install", json!({"data": data, "sha256": r["sha256"]})).unwrap();
    assert_eq!(ok["installed"], true);
}

#[test]
fn a_plugin_for_another_app_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = session(dir.path());
    let manifest = json!({"id": "org.test.fig", "name": "Fig", "version": "1.0.0", "main": "p.wasm", "apps": {"figcraft": {"api": 6}}});
    let bytes = Bundle { manifest: manifest.to_string(), wasm: module(&[], End::Ok), assets: vec![] }.encode();
    let path = dir.path().join("fig.craftplugin");
    std::fs::write(&path, bytes).unwrap();
    let e = s.execute("plugin.install", json!({"path": path})).unwrap_err().to_string();
    assert!(e.contains("figcraft"), "{e}");
}

#[test]
fn installs_from_a_local_catalog() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = session(dir.path());
    let bytes = bundle("org.test.solids", "write", module(&[], End::Ok));
    let file = dir.path().join("solids.craftplugin");
    std::fs::write(&file, &bytes).unwrap();
    let url = |p: &std::path::Path| format!("file://{}", p.display());
    let index = json!({"schema": 1, "packages": [{
        "id": "org.test.solids", "name": "Solids", "kind": "command",
        "versions": [{"version": "1.0.0", "apps": {"effectcraft": {"api": 6}}, "url": url(&file), "sha256": craft_plugin::sha256_hex(&bytes), "size": bytes.len()}]
    }]});
    let catalog = dir.path().join("index.json");
    std::fs::write(&catalog, index.to_string()).unwrap();
    s.execute("plugin.catalog.add", json!({"url": url(&catalog)})).unwrap();
    let listed = s.execute("plugin.catalog.list", json!({})).unwrap();
    assert_eq!(listed["packages"][0]["id"], "org.test.solids", "{listed}");
    let r = s.execute("plugin.install", json!({"catalog": url(&catalog), "id": "org.test.solids"})).unwrap();
    assert_eq!(r["source"]["type"], "catalog");
    assert_eq!(r["inCatalog"], true);
    assert!(s.execute("plugin.catalog.add", json!({"url": "https://example.com/index.json"})).is_err());
}

#[test]
fn settings_are_bounded() {
    let mut all = BTreeMap::new();
    set_setting(&mut all, "p", "k", Some("{\"a\": 1}")).unwrap();
    assert_eq!(all["p"]["k"], json!({"a": 1}));
    assert!(set_setting(&mut all, "p", &"k".repeat(MAX_SETTING_KEY + 1), Some("1")).is_err());
    assert!(set_setting(&mut all, "p", "big", Some(&format!("\"{}\"", "x".repeat(MAX_SETTINGS_BYTES)))).is_err());
    assert!(set_setting(&mut all, "p", "bad", Some("not json")).is_err());
    set_setting(&mut all, "p", "k", None).unwrap();
    assert!(!all.contains_key("p"));
}

static LOADED: AtomicUsize = AtomicUsize::new(0);

fn fake_loader(_: &[u8], _: &str) -> std::result::Result<Value, String> {
    LOADED.fetch_add(1, Ordering::SeqCst);
    Ok(json!({"id": "org.test.glow"}))
}

type Frames = Vec<Vec<[f32; 4]>>;

fn fake_previewer(_: &[u8], _: u32, _: u32, _: &[[f32; 4]], _: &[f64]) -> std::result::Result<(Value, Frames), String> {
    Ok((json!({"id": "org.test.glow"}), vec![]))
}

#[test]
fn effect_plugins_install_from_a_bundle_and_load_through_the_host() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = session(dir.path());
    s.plugin_loader = Some(fake_loader);
    s.plugin_previewer = Some(fake_previewer);
    let manifest =
        json!({"id": "org.test.glow", "name": "Glow", "version": "1.0.0", "main": "glow.wasm", "kind": "effect", "apps": {"effectcraft": {"api": 2}}});
    let bytes = Bundle { manifest: manifest.to_string(), wasm: b"\0asm".to_vec(), assets: vec![] }.encode();
    let before = LOADED.load(Ordering::SeqCst);
    let r = install_file(&mut s, dir.path(), &bytes);
    assert_eq!(r["kind"], "effect");
    assert_eq!(LOADED.load(Ordering::SeqCst), before + 1, "the effect is registered at once");
    let e = s.execute("plugin.run", json!({"id": "org.test.glow"})).unwrap_err().to_string();
    assert!(e.contains("effect plug-in"), "{e}");
    // An effect ABI this build doesn't load is refused.
    let newer = json!({"id": "org.test.glow", "name": "Glow", "version": "2.0.0", "main": "glow.wasm", "kind": "effect", "apps": {"effectcraft": {"api": 99}}});
    let bytes = Bundle { manifest: newer.to_string(), wasm: b"\0asm".to_vec(), assets: vec![] }.encode();
    let path = dir.path().join("newer.craftplugin");
    std::fs::write(&path, bytes).unwrap();
    assert!(s.execute("plugin.install", json!({"path": path})).is_err());
    // Removing it applies at the next start: its effect stays registered.
    assert_eq!(s.execute("plugin.remove", json!({"id": "org.test.glow"})).unwrap()["restartNeeded"], true);
}

#[test]
fn every_plugin_command_documents_its_params() {
    for id in ["plugin.list", "plugin.install", "plugin.update", "plugin.remove", "plugin.run", "plugin.catalog.list", "plugin.catalog.add"] {
        let spec = crate::commands::find(id).unwrap();
        assert!(crate::commands::accepted_params(spec.params).is_some(), "{id}");
    }
    let keys = crate::commands::accepted_params(crate::commands::find("plugin.install").unwrap().params).unwrap();
    for k in ["path", "data", "name", "repo", "catalog", "id", "version", "review", "sha256"] {
        assert!(keys.iter().any(|x| x == k), "{k}: {keys:?}");
    }
}
