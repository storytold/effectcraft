//! A WAT guest that edits the document through `cmd.execute`, as a command and as a `before` hook.

use serde_json::{Value, json};

use super::EDIT;

/// The edit entry 0 runs. `comp.new` adds a comp, so the project shows whether the host call arrived.
pub const MADE: &str = "Plug-in A";

fn data(s: &str) -> String {
    s.bytes().map(|b| format!("\\{b:02x}")).collect()
}

/// A craft-abi 1 module. Entry 0 runs `cmd.execute comp.new` and answers `{"result":{"ok":true}}`,
/// which a command takes as its result and a hook takes as a pass.
fn wat() -> String {
    let request = format!(r#"{{"id":"comp.new","params":{{"name":"{MADE}","width":64,"height":36,"duration":1}}}}"#);
    let ok = r#"{"result":{"ok":true}}"#;
    format!(
        r#"(module
  (import "craft_host" "call" (func $host (param i32 i32 i32 i32 i32) (result i64)))
  (memory (export "memory") 4 16)
  (global $heap (mut i32) (i32.const 110000))
  (data (i32.const 64) "{}")
  (data (i32.const 320) "{}")
  (func $alloc (param $len i32) (param $align i32) (result i32)
    (local $p i32)
    (local.set $p (global.get $heap))
    (global.set $heap (i32.add (global.get $heap) (local.get $len)))
    (if (i32.gt_u (global.get $heap) (i32.const 250000)) (then (global.set $heap (local.get $p)) (return (i32.const 0))))
    (local.get $p))
  (func (export "craft_abi") (result i32) i32.const 1)
  (export "craft_alloc" (func $alloc))
  (func (export "craft_free") (param $p i32) (param $len i32) (param $align i32)
    (if (i32.eq (i32.add (local.get $p) (local.get $len)) (global.get $heap)) (then (global.set $heap (local.get $p)))))
  (func (export "craft_invoke") (param $e i32) (param $rp i32) (param $rl i32) (param $bp i32) (param $bl i32) (result i64)
    (local $out i32)
    (drop (call $host (i32.const 0x11) (i32.const 64) (i32.const {}) (i32.const 2048) (i32.const 1024)))
    (local.set $out (call $alloc (i32.const {}) (i32.const 1)))
    (memory.copy (local.get $out) (i32.const 320) (i32.const {}))
    (i64.or (i64.shl (i64.const {}) (i64.const 32)) (i64.extend_i32_u (local.get $out))))
)"#,
        data(&request),
        data(ok),
        request.len(),
        ok.len(),
        ok.len(),
        ok.len()
    )
}

fn package(contributions: Value) -> Vec<u8> {
    let manifest = json!({
        "schema": 1, "id": EDIT, "name": EDIT, "version": "1.0.0",
        "abi": { "craft": 1 },
        "requires": { "ops": ["cmd.execute"] },
        "permissions": { "required": ["commands.run", "commands.intercept"], "optional": [] },
        "limits": { "memory_mb": 4, "stack_kb": 128 },
        "contributions": contributions
    });
    super::package_from(&wat(), &manifest)
}

/// A command `make` (id `plugin.<EDIT>.make`) that adds a comp.
pub fn command_package() -> Vec<u8> {
    package(json!([{ "kind": "command", "id": "make", "entry": 0, "label": "make", "enabled": "always" }]))
}

/// A `before` hook on `globs` that adds a comp, then passes.
pub fn gate_package(globs: &[&str]) -> Vec<u8> {
    package(json!([{ "kind": "hook", "id": "gate", "entry": 0, "phase": "before", "match": globs, "instance": "session" }]))
}
