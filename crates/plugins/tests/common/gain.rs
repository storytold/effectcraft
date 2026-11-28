//! The EffectCraft v1 "gain" test plug-in, in WebAssembly text. The tests build it here, so no
//! binary plug-in is checked in.

#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, clippy::panic)]

/// The gain plug-in's manifest.
pub fn gain_manifest(id: &str) -> String {
    format!(
        r#"{{"api":1,"id":"{id}","name":"Gain {id}","category":"Test Plug-ins","version":"1.0","params":[{{"id":"gain","name":"Gain","type":"slider","default":0.5,"min":0,"max":4}},{{"id":"on","name":"Lift","type":"checkbox","default":false}},{{"id":"tint","name":"Tint","type":"color","default":[0,0,1,1]}}]}}"#
    )
}

/// A plug-in in WebAssembly text: multiplies RGB by `gain`, adds `lift` times `tint` when `on`.
pub fn gain_module(id: &str, render_body: &str) -> String {
    module_with(&gain_manifest(id), render_body)
}

/// The gain module with its own manifest JSON.
pub fn module_with(manifest: &str, render_body: &str) -> String {
    let manifest = manifest.replace('\n', " ");
    let manifest = manifest.as_str();
    let len = manifest.len();
    let escaped = manifest.replace('\\', "\\\\").replace('"', "\\\"");
    format!(
        r#"(module
  (memory (export "memory") 2)
  (data (i32.const 16) "{escaped}")
  (func (export "ec_api_version") (result i32) i32.const 1)
  (func (export "ec_manifest_ptr") (result i32) i32.const 16)
  (func (export "ec_manifest_len") (result i32) i32.const {len})
  (func (export "ec_alloc") (param $n i32) (result i32)
    (local $need i32)
    local.get $n
    i32.const 131071
    i32.add
    i32.const 16
    i32.shr_u
    memory.size
    i32.sub
    local.tee $need
    i32.const 0
    i32.gt_s
    if
      local.get $need
      memory.grow
      drop
    end
    i32.const 65536)
  (func (export "ec_render") (param $px i32) (param $w i32) (param $h i32) (param $params i32) (param $np i32) (param $t f64) (param $scale f64) (result i32)
    (local $i i32) (local $end i32) (local $g f32) (local $on f64)
    {render_body}
    local.get $params
    f64.load
    f32.demote_f64
    local.set $g
    local.get $params
    f64.load offset=8
    local.set $on
    local.get $px
    local.get $w
    local.get $h
    i32.mul
    i32.const 16
    i32.mul
    i32.add
    local.set $end
    local.get $px
    local.set $i
    block $done
      loop $l
        local.get $i
        local.get $end
        i32.ge_u
        br_if $done
        local.get $i
        local.get $i
        f32.load
        local.get $g
        f32.mul
        f32.store
        local.get $i
        local.get $i
        f32.load offset=4
        local.get $g
        f32.mul
        f32.store offset=4
        local.get $i
        local.get $i
        f32.load offset=8
        local.get $g
        f32.mul
        local.get $on
        local.get $params
        f64.load offset=32
        f64.mul
        f32.demote_f64
        f32.add
        f32.store offset=8
        local.get $i
        i32.const 16
        i32.add
        local.set $i
        br $l
      end
    end
    i32.const 0))"#
    )
}
