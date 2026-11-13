//! WebAssembly plug-ins: load, register, render through the effect registry, sandbox limits.

use effectcraft_effects::{Buf, EffectCtx, EffectEnv, Image};
use effectcraft_project::build::Ids;

use crate::load_wasm;

fn load_err(r: Result<&'static effectcraft_effects::EffectSpec, String>) -> String {
    match r {
        Err(e) => e,
        Ok(s) => panic!("unexpectedly loaded {}", s.id),
    }
}

/// The gain plug-in's manifest.
fn gain_manifest(id: &str) -> String {
    format!(
        r#"{{"api":1,"id":"{id}","name":"Gain {id}","category":"Test Plug-ins","version":"1.0","params":[{{"id":"gain","name":"Gain","type":"slider","default":0.5,"min":0,"max":4}},{{"id":"on","name":"Lift","type":"checkbox","default":false}},{{"id":"tint","name":"Tint","type":"color","default":[0,0,1,1]}}]}}"#
    )
}

/// A plug-in in WebAssembly text: multiplies RGB by `gain`, adds `lift` × `tint` when `on`.
fn gain_module(id: &str, render_body: &str) -> String {
    module_with(&gain_manifest(id), render_body)
}

/// The gain module with its own manifest JSON.
fn module_with(manifest: &str, render_body: &str) -> String {
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

fn render(spec: &effectcraft_effects::EffectSpec, gain: f64, on: bool) -> Image {
    let mut next = 1;
    let g = effectcraft_effects::instantiate(spec, &mut Ids(&mut next), spec.name, [4.0, 2.0]);
    let mut params = effectcraft_effects::flatten_params(&g, &mut |p| p.value.clone());
    params.values.insert("gain".into(), effectcraft_keyframe::Value::Scalar(gain));
    params.values.insert("on".into(), effectcraft_keyframe::Value::Bool(on));
    let ctx = EffectCtx { params: &params, time: 0.0, layer_size: [4.0, 2.0], seed: 1, adjustment: false, env: EffectEnv::default() };
    let buf = Buf { img: Image::filled(4, 2, [0.8, 0.6, 0.2, 1.0]), offset: [0.0; 2], scale: 1.0 };
    (spec.render)(&ctx, buf).img
}

#[test]
fn wasm_plugin_registers_and_renders() {
    let spec = load_wasm(gain_module("org.test.gain", "").as_bytes(), "gain.wat").unwrap();
    assert_eq!(spec.id, "org.test.gain");
    assert_eq!(spec.category, "Test Plug-ins");
    assert_eq!(spec.params.len(), 3);
    // Found like a built-in, listed under its own category.
    assert!(effectcraft_effects::find("org.test.gain").is_some());
    assert!(effectcraft_effects::lookup("Gain org.test.gain").is_some());
    assert!(effectcraft_effects::all().iter().any(|e| e.id == "org.test.gain"));
    assert!(effectcraft_effects::categories().contains(&"Test Plug-ins"));
    let img = render(spec, 0.5, false);
    assert_eq!(img.get(1, 1), [0.4, 0.3, 0.1, 1.0]);
    // The checkbox and colour parameters arrive flattened (tint blue = 1 → +1 on blue).
    let img = render(spec, 1.0, true);
    assert_eq!(img.get(3, 0), [0.8, 0.6, 1.2, 1.0]);
    // Deterministic, and safe to run on many threads at once (each takes an instance).
    let a: Vec<Image> = (0..8).map(|_| render(spec, 0.25, false)).collect();
    std::thread::scope(|sc| {
        let hs: Vec<_> = (0..4).map(|_| sc.spawn(|| render(spec, 0.25, false))).collect();
        for h in hs {
            assert_eq!(h.join().unwrap(), a[0]);
        }
    });
    assert!(a.iter().all(|i| *i == a[0]));
    // The same id can't be registered twice.
    assert!(load_err(load_wasm(gain_module("org.test.gain", "").as_bytes(), "again.wat")).contains("already registered"));
}

#[test]
fn wasm_plugins_are_sandboxed() {
    // A runaway loop runs out of fuel: the frame comes back unchanged instead of hanging.
    let spec = load_wasm(gain_module("org.test.spin", "(loop $spin br $spin)").as_bytes(), "spin.wat").unwrap();
    let img = render(spec, 0.5, false);
    assert_eq!(img.get(0, 0), [0.8, 0.6, 0.2, 1.0]);
    // No imports (files, clock, network…).
    let imports = r#"(module (import "env" "now" (func $now (result f64))) (memory (export "memory") 1))"#;
    assert!(load_err(load_wasm(imports.as_bytes(), "imports.wat")).contains("import"));
    // Garbage, an ABI this build doesn't implement, bad manifests. A module that claims ABI 2
    // must export `ec_render2` (this one still has only `ec_render`).
    assert!(load_wasm(b"\0asm nope", "bad.wasm").is_err());
    let v2 = gain_module("org.test.v2", "")
        .replace("(func (export \"ec_api_version\") (result i32) i32.const 1)", "(func (export \"ec_api_version\") (result i32) i32.const 2)");
    assert!(load_err(load_wasm(v2.as_bytes(), "v2.wat")).contains("ec_render2"));
    let v3 = gain_module("org.test.v3", "")
        .replace("(func (export \"ec_api_version\") (result i32) i32.const 1)", "(func (export \"ec_api_version\") (result i32) i32.const 3)");
    assert!(load_err(load_wasm(v3.as_bytes(), "v3.wat")).contains("ABI 3"));
    let reserved = gain_module("ec.blur.mine", "");
    assert!(load_err(load_wasm(reserved.as_bytes(), "reserved.wat")).contains("reserved"));
}

/// `base` with `from` replaced by `to` (which must occur).
fn edit(base: &str, from: &str, to: &str) -> String {
    assert!(base.contains(from), "{from}");
    base.replace(from, to)
}

#[test]
fn abi_violations_fail_to_load_with_reasons() {
    let base = gain_module("org.test.abi", "");
    // Missing exports.
    for export in ["ec_render", "ec_alloc", "ec_manifest_ptr", "ec_manifest_len", "ec_api_version"] {
        let m = edit(&base, &format!("(export \"{export}\")"), "");
        let e = load_err(load_wasm(m.as_bytes(), "x.wat"));
        assert!(e.contains(export), "{export}: {e}");
    }
    let m = edit(&base, "(memory (export \"memory\") 2)", "(memory 2)");
    assert!(load_err(load_wasm(m.as_bytes(), "x.wat")).contains("memory"));
    // Wrong signatures.
    let m = edit(&base, "(func (export \"ec_api_version\") (result i32) i32.const 1)", "(func (export \"ec_api_version\") (result f64) f64.const 1)");
    assert!(load_err(load_wasm(m.as_bytes(), "x.wat")).contains("ec_api_version"));
    let m = edit(&base, "(param $scale f64) (result i32)", "(param $scale f32) (result i32)");
    assert!(load_err(load_wasm(m.as_bytes(), "x.wat")).contains("ec_render"));
    // WebAssembly ABIs other than 1 and 2.
    for v in ["0", "-1", "3", "2147483647"] {
        let m = edit(
            &base,
            "(func (export \"ec_api_version\") (result i32) i32.const 1)",
            &format!("(func (export \"ec_api_version\") (result i32) i32.const {v})"),
        );
        assert!(load_err(load_wasm(m.as_bytes(), "x.wat")).contains(&format!("ABI {v}")), "{v}");
    }
    // Manifest out of bounds, negative or absurd lengths, not JSON.
    let ptr = "(func (export \"ec_manifest_ptr\") (result i32) i32.const 16)";
    let m = edit(&base, ptr, "(func (export \"ec_manifest_ptr\") (result i32) i32.const 200000)");
    assert!(load_err(load_wasm(m.as_bytes(), "x.wat")).contains("out of bounds"));
    let len = gain_manifest("org.test.abi").len();
    for bad in ["-5", "2000000000"] {
        let m = edit(&base, &format!("(result i32) i32.const {len})"), &format!("(result i32) i32.const {bad})"));
        assert!(load_err(load_wasm(m.as_bytes(), "x.wat")).contains("manifest length"), "{bad}");
    }
    let m = edit(&base, ptr, "(func (export \"ec_manifest_ptr\") (result i32) i32.const 4000)");
    assert!(load_err(load_wasm(m.as_bytes(), "x.wat")).contains("manifest JSON"));
    // Traps and runaway code while loading.
    let version = "(func (export \"ec_api_version\") (result i32) i32.const 1)";
    let m = edit(&base, version, "(func (export \"ec_api_version\") (result i32) unreachable)");
    assert!(load_wasm(m.as_bytes(), "x.wat").is_err());
    let m = edit(&base, version, "(func (export \"ec_api_version\") (result i32) (loop $l br $l) i32.const 1)");
    assert!(load_wasm(m.as_bytes(), "x.wat").is_err(), "fuel runs out in ec_api_version");
    let m = edit(&base, "(memory (export \"memory\") 2)", "(memory (export \"memory\") 2) (func $s (loop $l br $l)) (start $s)");
    assert!(load_err(load_wasm(m.as_bytes(), "x.wat")).contains("instantiation"), "fuel runs out in the start function");
    // Memory beyond the sandbox's limit.
    let m = edit(&base, "(memory (export \"memory\") 2)", "(memory (export \"memory\") 20000)");
    assert!(load_err(load_wasm(m.as_bytes(), "x.wat")).contains("instantiation"));
    // Not a module at all.
    for junk in [&b""[..], b"\0asm", b"\0asm\x01\0\0\0\xff\xff", b"(module", b"(module (func (export \"x\") unknown_op))"] {
        assert!(load_wasm(junk, "junk").is_err());
    }
}

#[test]
fn manifest_parameter_schemas_are_validated() {
    let p = |params: &str| format!(r#"{{"api":1,"id":"org.test.schema","name":"Schema","category":"Test Plug-ins","version":"1","params":[{params}]}}"#);
    let bad = [
        (r#"{"id":"a","name":"A","type":"checkbox"},{"id":"a","name":"B","type":"checkbox"}"#, "duplicate"),
        (r#"{"id":"a b","name":"A","type":"checkbox"}"#, "parameter id"),
        (r#"{"id":"","name":"A","type":"checkbox"}"#, "parameter id"),
        (r#"{"id":"p","name":"P","type":"popup","options":[],"default":0}"#, "popup"),
        (r#"{"id":"p","name":"P","type":"popup","options":["x"],"default":3}"#, "popup"),
        (r#"{"id":"s","name":"S","type":"slider","default":1,"min":5,"max":0}"#, "slider"),
        (r#"{"id":"s","name":"S","type":"slider","default":9,"min":0,"max":4}"#, "slider"),
        (r#"{"id":"s","name":"S","type":"slider","default":1,"min":0,"max":4,"sliderMin":3,"sliderMax":2}"#, "slider"),
        (r#"{"id":"s","name":"S","type":"wobble"}"#, "manifest JSON"),
        (r#"{"id":"s","name":"S","type":"slider"}"#, "manifest JSON"),
    ];
    for (params, want) in bad {
        let m = module_with(&p(params), "");
        let e = load_err(load_wasm(m.as_bytes(), "schema.wat"));
        assert!(e.contains(want), "{params}: {e}");
    }
    let many: Vec<String> = (0..300).map(|i| format!(r#"{{"id":"c{i}","name":"C","type":"checkbox"}}"#)).collect();
    let e = load_err(load_wasm(module_with(&p(&many.join(",")), "").as_bytes(), "many.wat"));
    assert!(e.contains("at most"), "{e}");
    // Missing name, another API in the manifest.
    let e = load_err(load_wasm(module_with(r#"{"api":1,"id":"org.test.noname","name":" ","category":"X","version":"1","params":[]}"#, "").as_bytes(), "x.wat"));
    assert!(e.contains("name"), "{e}");
    let e = load_err(load_wasm(module_with(r#"{"api":3,"id":"org.test.api3","name":"N","category":"X","version":"1","params":[]}"#, "").as_bytes(), "x.wat"));
    assert!(e.contains("API 3"), "{e}");
    // A valid schema loads.
    let ok = p(
        r#"{"id":"s","name":"S","type":"slider","default":2,"min":0,"max":4,"sliderMin":1,"sliderMax":3},{"id":"p","name":"P","type":"popup","options":["x","y"],"default":1}"#,
    );
    let spec = load_wasm(module_with(&ok, "").as_bytes(), "ok.wat").unwrap();
    assert_eq!(spec.params.len(), 2);
}

#[test]
fn render_failures_leave_the_frame_and_recover() {
    let orig = [0.8, 0.6, 0.2, 1.0];
    // ec_render traps (when time > 0.5), returns an error code, or ec_alloc hands back memory
    // out of bounds: the frame passes through unchanged, and the plug-in keeps working.
    let trap = load_wasm(gain_module("org.test.trap", "local.get $t f64.const 0.5 f64.gt if unreachable end").as_bytes(), "trap.wat").unwrap();
    let mut next = 1;
    let g = effectcraft_effects::instantiate(trap, &mut Ids(&mut next), trap.name, [4.0, 2.0]);
    let params = effectcraft_effects::flatten_params(&g, &mut |p| p.value.clone());
    let at = |time: f64| {
        let ctx = EffectCtx { params: &params, time, layer_size: [4.0, 2.0], seed: 1, adjustment: false, env: EffectEnv::default() };
        let buf = Buf { img: Image::filled(4, 2, orig), offset: [0.0; 2], scale: 1.0 };
        (trap.render)(&ctx, buf).img
    };
    assert_eq!(at(1.0).get(0, 0), orig, "trapped");
    assert_eq!(at(0.0).get(0, 0), [0.4, 0.3, 0.1, 1.0], "a fresh instance after the trap");
    assert_eq!(at(1.0).get(0, 0), orig);
    assert_eq!(at(0.0).get(1, 1), [0.4, 0.3, 0.1, 1.0]);
    let code = load_wasm(gain_module("org.test.code", "i32.const 7 return").as_bytes(), "code.wat").unwrap();
    assert_eq!(render(code, 0.5, false).get(0, 0), orig);
    let oob = edit(&gain_module("org.test.oob", ""), "    i32.const 65536)", "    i32.const -16)");
    let oob = load_wasm(oob.as_bytes(), "oob.wat").unwrap();
    assert_eq!(render(oob, 0.5, false).get(0, 0), orig);
    // Runaway rendering: out of fuel every time, never hangs.
    let spin = load_wasm(gain_module("org.test.spin2", "(loop $spin br $spin)").as_bytes(), "spin2.wat").unwrap();
    for _ in 0..3 {
        assert_eq!(render(spin, 0.5, false).get(0, 0), orig);
    }
}

#[test]
fn wasm_rendering_is_deterministic() {
    // Float edge cases (NaN from sqrt(−x), infinities from x / 0) come out bit-identical
    // across instances, threads and repeated runs.
    let body = "local.get $px local.get $px f32.load f32.const -1 f32.mul f32.sqrt f32.store \
                local.get $px local.get $px f32.load offset=4 f32.const 0 f32.div f32.store offset=4";
    let spec = load_wasm(gain_module("org.test.det", body).as_bytes(), "det.wat").unwrap();
    let bits = |img: &Image| -> Vec<u32> { (0..2).flat_map(|y| (0..4).map(move |x| (x, y))).flat_map(|(x, y)| img.get(x, y).map(f32::to_bits)).collect() };
    let first = bits(&render(spec, 0.37, true));
    let runs: Vec<Vec<u32>> = std::thread::scope(|sc| {
        let hs: Vec<_> = (0..6).map(|_| sc.spawn(|| bits(&render(spec, 0.37, true)))).collect();
        hs.into_iter().map(|h| h.join().unwrap()).collect()
    });
    for r in runs {
        assert_eq!(r, first);
    }
    // The same module loaded under another id renders the same bits.
    let twin = load_wasm(gain_module("org.test.det2", body).as_bytes(), "det2.wat").unwrap();
    assert_eq!(bits(&render(twin, 0.37, true)), first);
}

/// An ABI 2 module whose `ec_render2` writes what it got from the host into each pixel: red is
/// |next.red − prev.red|, green `flags / 8`, blue `prev_time`, alpha 1.
fn neighbour_module(id: &str) -> String {
    let manifest = format!(r#"{{"api":2,"id":"{id}","name":"Neighbours {id}","category":"Test Plug-ins","version":"1","params":[]}}"#);
    let len = manifest.len();
    let escaped = manifest.replace('\\', "\\\\").replace('"', "\\\"");
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
    (local $i i32) (local $n i32)
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
        local.get $px
        local.get $i
        i32.const 16
        i32.mul
        i32.add
        local.get $prev_t
        f32.demote_f64
        f32.store offset=8
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

fn neighbour_image(red: f32) -> effectcraft_effects::plugin::PluginImage {
    effectcraft_effects::plugin::PluginImage { width: 2, height: 1, pixels: vec![[red, 0.0, 0.0, 1.0]; 2], origin: [0.0; 2], scale: 1.0 }
}

/// A host with frames at two layer times.
struct Host {
    time: f64,
    fps: f64,
    prev: Option<(f64, effectcraft_effects::plugin::PluginImage)>,
    next: Option<(f64, effectcraft_effects::plugin::PluginImage)>,
}

impl effectcraft_effects::plugin::PluginHost for Host {
    fn input_at(&self, layer_time: f64) -> Option<effectcraft_effects::plugin::PluginImage> {
        if let Some((t, img)) = &self.prev
            && (t - layer_time).abs() < 1e-9
        {
            return Some(img.clone());
        }
        if let Some((t, img)) = &self.next
            && (t - layer_time).abs() < 1e-9
        {
            return Some(img.clone());
        }
        None
    }

    fn layer_input(&self, _param_id: &str, _layer_time: f64) -> Option<effectcraft_effects::plugin::PluginImage> {
        None
    }

    fn params_at(&self, _layer_time: f64) -> Option<(Vec<f64>, Vec<String>)> {
        None
    }

    fn frame_rate(&self) -> f64 {
        self.fps
    }

    fn time(&self) -> f64 {
        self.time
    }
}

#[test]
fn abi_v2_receives_neighbour_frames_and_times() {
    load_wasm(neighbour_module("org.test.neighbours").as_bytes(), "neighbours.wat").unwrap();
    let p = effectcraft_effects::plugin::plugin("org.test.neighbours").unwrap();
    assert!(effectcraft_effects::plugin::reads_other_times("org.test.neighbours"));
    assert!(effectcraft_effects::is_time_dependent("org.test.neighbours"));
    // The module has no parameters, so the flattened vector is empty.
    let params = effectcraft_effects::plugin::PluginParams { manifest: p.manifest(), flat: &[], texts: &[] };
    let mut px = vec![[0.0f32; 4]; 2];
    let host = Host { time: 0.5, fps: 4.0, prev: Some((0.25, neighbour_image(0.1))), next: Some((0.75, neighbour_image(0.6))) };
    let mut frame = effectcraft_effects::plugin::PluginFrame { width: 2, height: 1, pixels: &mut px, scale: 1.0, origin: [0.0; 2], layer_size: [2.0, 1.0] };
    p.render_v2(&mut frame, &params, 0.5, &host).unwrap();
    assert!((px[0][0] - 0.5).abs() < 1e-6, "|next − prev|: {px:?}");
    assert!((px[0][1] - 3.0 / 8.0).abs() < 1e-6, "both flags: {px:?}");
    assert!((px[0][2] - 0.25).abs() < 1e-6, "previous frame's time: {px:?}");
    assert_eq!(px[0][3], 1.0);
}

#[test]
fn abi_v2_without_a_host_gets_zeroed_neighbours() {
    let spec = load_wasm(neighbour_module("org.test.neighbours.empty").as_bytes(), "empty.wat").unwrap();
    let mut next_id = 1;
    let g = effectcraft_effects::instantiate(spec, &mut Ids(&mut next_id), spec.name, [2.0, 1.0]);
    let params = effectcraft_effects::flatten_params(&g, &mut |p| p.value.clone());
    let ctx = EffectCtx { params: &params, time: 0.5, layer_size: [2.0, 1.0], seed: 1, adjustment: false, env: EffectEnv::default() };
    let buf = Buf { img: Image::filled(2, 1, [0.5, 0.5, 0.5, 1.0]), offset: [0.0; 2], scale: 1.0 };
    // No host: both neighbour buffers are zeroed and no flag is set.
    assert_eq!((spec.render)(&ctx, buf).img.get(0, 0), [0.0, 0.0, 0.0, 1.0]);
    // ABI 1 plug-ins keep the time-independent behaviour.
    let gain = load_wasm(gain_module("org.test.neighbours.v1", "").as_bytes(), "v1.wat").unwrap();
    assert!(!effectcraft_effects::plugin::reads_other_times("org.test.neighbours.v1"));
    assert!(!effectcraft_effects::is_time_dependent("org.test.neighbours.v1"));
    assert!(gain.id.starts_with("org.test."));
}
