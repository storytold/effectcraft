//! The wasmi-backed [`EffectPlugin`] (plug-in API v1 ABI, see the crate docs).

use std::sync::Mutex;

use wasmi::{Config, Engine, Instance, Linker, Memory, Module, Store, StoreLimits, StoreLimitsBuilder, TypedFunc, WasmParams, WasmResults};

use crate::{EffectPlugin, PluginFrame, PluginManifest, PluginParams};

/// The WebAssembly ABI version (`ec_api_version`); manifests may say API 1 or 2, but a module
/// can only use the API 1 parameter types.
const WASM_API_VERSION: i32 = 1;
/// Instruction budget per frame: a fixed allowance plus some per pixel. Generous for per-pixel
/// work, but a runaway loop stops instead of hanging the render.
const FUEL_BASE: u64 = 50_000_000;
const FUEL_PER_PIXEL: u64 = 20_000;
/// The most linear memory a plug-in instance may have (a 4K float frame is ~130 MB).
const MAX_MEMORY: usize = 1 << 30;
/// The longest manifest read.
const MAX_MANIFEST: i32 = 1 << 20;

type RenderFn = TypedFunc<(i32, i32, i32, i32, i32, f64, f64), i32>;

/// One instantiated module (renders running on several threads each take their own).
struct Inst {
    store: Store<StoreLimits>,
    instance: Instance,
    memory: Memory,
    alloc: TypedFunc<i32, i32>,
    render: RenderFn,
}

impl Inst {
    fn func<P: WasmParams, R: WasmResults>(&self, name: &str) -> Result<TypedFunc<P, R>, String> {
        self.instance.get_typed_func::<P, R>(&self.store, name).map_err(|_| missing(name))
    }
}

fn missing(name: &str) -> String {
    format!("the module exports no `{name}` (or with the wrong signature)")
}

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

fn instantiate(engine: &Engine, module: &Module) -> Result<Inst, String> {
    let mut store = Store::new(engine, StoreLimitsBuilder::new().memory_size(MAX_MEMORY).instances(1).build());
    store.limiter(|l| l);
    store.set_fuel(FUEL_BASE).map_err(err)?;
    let linker = Linker::<StoreLimits>::new(engine);
    let instance = linker.instantiate_and_start(&mut store, module).map_err(|e| format!("instantiation failed: {e}"))?;
    let memory = instance.get_memory(&store, "memory").ok_or("the module exports no `memory`")?;
    let alloc = instance.get_typed_func::<i32, i32>(&store, "ec_alloc").map_err(|_| missing("ec_alloc"))?;
    let render = instance.get_typed_func::<(i32, i32, i32, i32, i32, f64, f64), i32>(&store, "ec_render").map_err(|_| missing("ec_render"))?;
    Ok(Inst { store, instance, memory, alloc, render })
}

/// A loaded WebAssembly effect plug-in.
pub struct WasmPlugin {
    manifest: PluginManifest,
    source: String,
    engine: Engine,
    module: Module,
    /// Idle instances.
    pool: Mutex<Vec<Inst>>,
}

impl WasmPlugin {
    pub fn new(bytes: &[u8], source: &str) -> Result<WasmPlugin, String> {
        let mut config = Config::default();
        config.consume_fuel(true);
        let engine = Engine::new(&config);
        let module = Module::new(&engine, bytes).map_err(|e| format!("not a valid WebAssembly module: {e}"))?;
        if let Some(i) = module.imports().next() {
            return Err(format!("plug-ins can't import anything (the module imports `{}.{}`)", i.module(), i.name()));
        }
        let mut inst = instantiate(&engine, &module)?;
        let version = inst.func::<(), i32>("ec_api_version")?;
        let v = version.call(&mut inst.store, ()).map_err(err)?;
        if v != WASM_API_VERSION {
            return Err(format!("the module implements plug-in API {v}, this build implements API {WASM_API_VERSION} for WebAssembly"));
        }
        let ptr = inst.func::<(), i32>("ec_manifest_ptr")?.call(&mut inst.store, ()).map_err(err)?;
        let len = inst.func::<(), i32>("ec_manifest_len")?.call(&mut inst.store, ()).map_err(err)?;
        if !(0..=MAX_MANIFEST).contains(&len) {
            return Err(format!("bad manifest length {len}"));
        }
        let mut text = vec![0u8; len as usize];
        inst.memory.read(&inst.store, ptr as u32 as usize, &mut text).map_err(|e| format!("manifest out of bounds: {e}"))?;
        let manifest: PluginManifest = serde_json::from_slice(&text).map_err(|e| format!("bad manifest JSON: {e}"))?;
        manifest.validate()?;
        if let Some(p) = manifest.params.iter().find(|p| p.kind.needs_v2()) {
            return Err(format!("parameter `{}`: WebAssembly plug-ins use plug-in API 1 parameter types (no point3, layer, text or hidden)", p.id));
        }
        Ok(WasmPlugin { manifest, source: source.to_string(), engine, module, pool: Mutex::new(vec![inst]) })
    }

    fn take(&self) -> Result<Inst, String> {
        if let Some(i) = self.pool.lock().ok().and_then(|mut p| p.pop()) {
            return Ok(i);
        }
        instantiate(&self.engine, &self.module)
    }

    fn give_back(&self, inst: Inst) {
        if let Ok(mut p) = self.pool.lock() {
            p.push(inst);
        }
    }
}

impl EffectPlugin for WasmPlugin {
    fn manifest(&self) -> &PluginManifest {
        &self.manifest
    }

    fn source(&self) -> String {
        self.source.clone()
    }

    fn render(&self, frame: &mut PluginFrame, params: &PluginParams, time: f64) -> Result<(), String> {
        let n = frame.pixels.len();
        let params_bytes = params.flat.len() * 8;
        let total = params_bytes + n * 16;
        if total > i32::MAX as usize {
            return Err("frame too large for a 32-bit plug-in".into());
        }
        let mut inst = self.take()?;
        inst.store.set_fuel(FUEL_BASE + FUEL_PER_PIXEL * n as u64).map_err(err)?;
        let base = inst.alloc.call(&mut inst.store, total as i32).map_err(err)? as u32 as usize;
        // Parameters, then pixels (little-endian, as WebAssembly memory is).
        let mut buf: Vec<u8> = Vec::with_capacity(total);
        for v in params.flat {
            buf.extend_from_slice(&v.to_le_bytes());
        }
        for px in frame.pixels.iter() {
            for c in px {
                buf.extend_from_slice(&c.to_le_bytes());
            }
        }
        inst.memory.write(&mut inst.store, base, &buf).map_err(|e| format!("ec_alloc returned a buffer out of bounds: {e}"))?;
        let px_ptr = (base + params_bytes) as i32;
        let r = inst
            .render
            .call(&mut inst.store, (px_ptr, frame.width as i32, frame.height as i32, base as i32, params.flat.len() as i32, time, frame.scale))
            .map_err(|e| format!("ec_render trapped: {e}"));
        let r = match r {
            Ok(0) => {
                inst.memory.read(&inst.store, base + params_bytes, &mut buf[params_bytes..]).map_err(err)?;
                for (px, b) in frame.pixels.iter_mut().zip(buf[params_bytes..].as_chunks::<16>().0.iter()) {
                    for (c, v) in px.iter_mut().zip(b.as_chunks::<4>().0.iter()) {
                        *c = f32::from_le_bytes([v[0], v[1], v[2], v[3]]);
                    }
                }
                Ok(())
            }
            Ok(code) => Err(format!("ec_render returned error {code}")),
            Err(e) => Err(e),
        };
        if r.is_ok() {
            self.give_back(inst);
        }
        r
    }
}
