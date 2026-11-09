//! The wasmi-backed [`EffectPlugin`] (WebAssembly ABI v1 and v2, see the crate docs).

use std::sync::Mutex;

use wasmi::{Caller, Config, Engine, Extern, Instance, Linker, Memory, Module, Store, StoreLimits, StoreLimitsBuilder, TypedFunc, WasmParams, WasmResults};

use crate::{EffectPlugin, MIN_WASM_API_VERSION, PluginFrame, PluginManifest, PluginParams, WASM_API_VERSION};
use effectcraft_effects::plugin::{PluginHost, PluginImage};

/// Instruction budget per frame: a fixed allowance plus some per pixel. Generous for per-pixel
/// work, but a runaway loop stops instead of hanging the render.
const FUEL_BASE: u64 = 50_000_000;
const FUEL_PER_PIXEL: u64 = 20_000;
/// The most `source_at` calls one render may make. Each call renders another frame, so a
/// runaway effect stops instead of multiplying the render cost without bound.
const MAX_SOURCE_AT_CALLS: u32 = 64;
/// The most linear memory a plug-in instance may have (a 4K float frame is ~130 MB).
const MAX_MEMORY: usize = 1 << 30;
/// The longest manifest read.
const MAX_MANIFEST: i32 = 1 << 20;

/// Everything one `source_at` call may read during a render. It lives in the wasmi store, so
/// the host function gets the live `PluginHost` without a thread-local or a raw pointer.
struct SourceAtStore<'a> {
    limits: StoreLimits,
    host: &'a dyn PluginHost,
    width: u32,
    height: u32,
    origin: [f64; 2],
    scale: f64,
    calls: u32,
}

/// `ec.source_at(layer_time, effects, dst_ptr, dst_len) -> 0 ok, 1 unavailable, 2 bad buffer`.
/// Writes the effect's input at `layer_time` (first `effects` effects applied) into the
/// module's memory as premultiplied RGBA `f32` on the current frame's grid.
fn source_at_host<'a>(mut caller: Caller<'_, SourceAtStore<'a>>, layer_time: f64, effects: i32, dst_ptr: i32, dst_len: i32) -> i32 {
    if !layer_time.is_finite() || effects < 0 {
        return 1;
    }
    let (width, height, origin, scale, calls) = {
        let data = caller.data();
        (data.width, data.height, data.origin, data.scale, data.calls)
    };
    if calls >= MAX_SOURCE_AT_CALLS {
        return 1;
    }
    let (w, h) = (width as usize, height as usize);
    let Some(bytes) = w.checked_mul(h).and_then(|n| n.checked_mul(16)) else {
        return 2;
    };
    if dst_ptr < 0 || dst_len < 0 || (dst_len as usize) < bytes {
        return 2;
    }
    let start = dst_ptr as usize;
    let Some(end) = start.checked_add(bytes) else {
        return 2;
    };
    let Some(memory) = caller.get_export("memory").and_then(|e| match e {
        Extern::Memory(m) => Some(m),
        _ => None,
    }) else {
        return 1;
    };
    if end > memory.data_size(&caller) {
        return 2;
    }
    // Every call that reaches the host counts against the cap, including one that returns
    // unavailable. The cap, not the module's instruction fuel, bounds the host work.
    caller.data_mut().calls = calls + 1;
    let host: &'a dyn PluginHost = caller.data().host;
    let Some(image) = host.source_at(layer_time, effects as usize) else {
        return 1;
    };
    let pixels = image.on_grid(width, height, origin, scale);
    if pixels.len() != w * h {
        return 2;
    }
    let mut buf = Vec::with_capacity(bytes);
    for p in &pixels {
        for c in p {
            buf.extend_from_slice(&c.to_le_bytes());
        }
    }
    if memory.write(&mut caller, start, &buf).is_err() {
        return 2;
    }
    0
}

/// The source_at import for the temporary instance used to read the manifest at load time:
/// there is no render host yet, so every call is unavailable.
fn source_at_host_unavailable(_: Caller<'_, StoreLimits>, _: f64, _: i32, _: i32, _: i32) -> i32 {
    1
}

/// ABI 1: one buffer.
type RenderFn = TypedFunc<(i32, i32, i32, i32, i32, f64, f64), i32>;
/// ABI 2: the same call plus the previous and next frame buffers and their layer times.
type Render2Fn = TypedFunc<(i32, i32, i32, i32, i32, f64, f64, i32, f64, i32, f64, i32), i32>;

enum Renderer {
    V1(RenderFn),
    V2(Render2Fn),
}

/// One instantiated module (renders running on several threads each take their own).
struct Inst<T> {
    store: Store<T>,
    memory: Memory,
    alloc: TypedFunc<i32, i32>,
    render: Renderer,
}

/// An instance whose ABI version (and so its render export) is not selected yet.
struct Pending<T> {
    store: Store<T>,
    instance: Instance,
    memory: Memory,
    alloc: TypedFunc<i32, i32>,
}

impl<T> Pending<T> {
    /// A typed export by name, reporting a missing or mistyped export like [`missing`].
    fn func<P: WasmParams, R: WasmResults>(&self, name: &str) -> Result<TypedFunc<P, R>, String> {
        self.instance.get_typed_func::<P, R>(&self.store, name).map_err(|_| missing(name))
    }

    /// Select the render export for `abi`.
    fn finish(self, abi: u32) -> Result<Inst<T>, String> {
        let render = match abi {
            1 => Renderer::V1(self.instance.get_typed_func::<_, i32>(&self.store, "ec_render").map_err(|_| missing("ec_render"))?),
            2 => Renderer::V2(self.instance.get_typed_func::<_, i32>(&self.store, "ec_render2").map_err(|_| missing("ec_render2"))?),
            v => return Err(unsupported(v as i32)),
        };
        Ok(Inst { store: self.store, memory: self.memory, alloc: self.alloc, render })
    }
}

/// The error for a missing or mistyped export.
fn missing(name: &str) -> String {
    format!("the module exports no `{name}` (or with the wrong signature)")
}

/// The error for an ABI this build does not implement.
fn unsupported(v: i32) -> String {
    format!("the module implements WebAssembly ABI {v}, this build implements ABI {MIN_WASM_API_VERSION} to {WASM_API_VERSION}")
}

/// The module's `ec_api_version`, when this build implements it.
fn abi_of(v: i32) -> Result<u32, String> {
    match u32::try_from(v) {
        Ok(v) if (MIN_WASM_API_VERSION..=WASM_API_VERSION).contains(&v) => Ok(v),
        _ => Err(unsupported(v)),
    }
}

/// The error for a wasmi or trap failure.
fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

/// Finish instantiation: select exports and build the pending instance.
fn finish_begin<T>(mut store: Store<T>, linker: Linker<T>, module: &Module) -> Result<Pending<T>, String> {
    store.set_fuel(FUEL_BASE).map_err(err)?;
    let instance = linker.instantiate_and_start(&mut store, module).map_err(|e| format!("instantiation failed: {e}"))?;
    let memory = instance.get_memory(&store, "memory").ok_or("the module exports no `memory`")?;
    let alloc = instance.get_typed_func::<i32, i32>(&store, "ec_alloc").map_err(|_| missing("ec_alloc"))?;
    Ok(Pending { store, instance, memory, alloc })
}

/// Instantiate `module` in a fresh, limited store and read its memory and allocator.
/// `source_at` only resolves the import so the manifest can be read; such call returns 1,
/// because the real host arrives with [`instantiate_source_at`] at render time.
fn begin(engine: &Engine, module: &Module, source_at: bool) -> Result<Pending<StoreLimits>, String> {
    let mut store = Store::new(engine, StoreLimitsBuilder::new().memory_size(MAX_MEMORY).instances(1).build());
    store.limiter(|l| l);
    let mut linker = Linker::<StoreLimits>::new(engine);
    if source_at {
        linker.func_wrap("ec", "source_at", source_at_host_unavailable).map_err(err)?;
    }
    finish_begin(store, linker, module)
}

/// Instantiate `module` for one render, with the live host available to `ec.source_at`.
/// The store borrows the host, so these instances are created per render and never pooled.
fn instantiate_source_at<'a>(
    engine: &Engine,
    module: &Module,
    host: &'a dyn PluginHost,
    width: u32,
    height: u32,
    origin: [f64; 2],
    scale: f64,
) -> Result<Inst<SourceAtStore<'a>>, String> {
    let limits = StoreLimitsBuilder::new().memory_size(MAX_MEMORY).instances(1).build();
    let mut store = Store::new(engine, SourceAtStore { limits, host, width, height, origin, scale, calls: 0 });
    store.limiter(|s| &mut s.limits);
    let mut linker = Linker::<SourceAtStore<'a>>::new(engine);
    linker.func_wrap("ec", "source_at", source_at_host).map_err(err)?;
    finish_begin(store, linker, module)?.finish(2)
}

/// Instantiate `module` and select the render export for `abi`. A `source_at` module gets the
/// unavailable import here: this path has no render host, so its calls return 1.
fn instantiate(engine: &Engine, module: &Module, abi: u32, source_at: bool) -> Result<Inst<StoreLimits>, String> {
    begin(engine, module, source_at)?.finish(abi)
}

/// A loaded WebAssembly effect plug-in.
pub struct WasmPlugin {
    manifest: PluginManifest,
    source: String,
    /// The WebAssembly ABI the module implements (`ec_api_version`).
    abi: u32,
    /// The module imports `ec.source_at` (and its manifest declares it).
    source_at: bool,
    engine: Engine,
    module: Module,
    /// Idle instances.
    pool: Mutex<Vec<Inst<StoreLimits>>>,
}

impl WasmPlugin {
    /// Load `bytes` and its manifest; errors on imports, a bad manifest or a missing export.
    pub fn new(bytes: &[u8], source: &str) -> Result<WasmPlugin, String> {
        let mut config = Config::default();
        config.consume_fuel(true);
        let engine = Engine::new(&config);
        let module = Module::new(&engine, bytes).map_err(|e| format!("not a valid WebAssembly module: {e}"))?;
        let mut has_source_at = false;
        for i in module.imports() {
            if i.module() == "ec" && i.name() == "source_at" {
                if has_source_at {
                    return Err("the module imports `ec.source_at` more than once".into());
                }
                has_source_at = true;
            } else {
                return Err(format!("plug-ins can only import `ec.source_at` (the module imports `{}.{}`)", i.module(), i.name()));
            }
        }
        let mut pending = begin(&engine, &module, has_source_at)?;
        let version = pending.func::<(), i32>("ec_api_version")?;
        let abi = abi_of(version.call(&mut pending.store, ()).map_err(err)?)?;
        let ptr = pending.func::<(), i32>("ec_manifest_ptr")?.call(&mut pending.store, ()).map_err(err)?;
        let len = pending.func::<(), i32>("ec_manifest_len")?.call(&mut pending.store, ()).map_err(err)?;
        if !(0..=MAX_MANIFEST).contains(&len) {
            return Err(format!("bad manifest length {len}"));
        }
        let mut text = vec![0u8; len as usize];
        pending.memory.read(&pending.store, ptr as u32 as usize, &mut text).map_err(|e| format!("manifest out of bounds: {e}"))?;
        let manifest: PluginManifest = serde_json::from_slice(&text).map_err(|e| format!("bad manifest JSON: {e}"))?;
        manifest.validate()?;
        if let Some(p) = manifest.params.iter().find(|p| p.kind.needs_v2()) {
            return Err(format!("parameter `{}`: WebAssembly plug-ins use plug-in API 1 parameter types (no point3, layer, text or hidden)", p.id));
        }
        let declared = manifest.imports.iter().any(|i| i == "source_at");
        if declared != has_source_at {
            return Err(if declared {
                "the manifest declares `source_at` but the module does not import `ec.source_at`".into()
            } else {
                "the module imports `ec.source_at` but the manifest does not list it in `imports`".into()
            });
        }
        if has_source_at && abi < 2 {
            return Err("`source_at` needs WebAssembly ABI 2 (the module reports ABI 1)".into());
        }
        let inst = pending.finish(abi)?;
        Ok(WasmPlugin { manifest, source: source.to_string(), abi, source_at: has_source_at, engine, module, pool: Mutex::new(vec![inst]) })
    }

    /// An idle instance, or a fresh one when the pool is empty. `source_at` plug-ins do not use
    /// the pool: their store borrows the render host, so they are instantiated per render.
    fn take(&self) -> Result<Inst<StoreLimits>, String> {
        if let Some(i) = self.pool.lock().ok().and_then(|mut p| p.pop()) {
            return Ok(i);
        }
        instantiate(&self.engine, &self.module, self.abi, self.source_at)
    }

    /// Return a working instance to the pool.
    fn give_back(&self, inst: Inst<StoreLimits>) {
        if let Ok(mut p) = self.pool.lock() {
            p.push(inst);
        }
    }

    /// ABI 1: parameters, then the pixels; `ec_render`.
    fn render_v1(&self, frame: &mut PluginFrame, params: &PluginParams, time: f64) -> Result<(), String> {
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
        push_pixels(&mut buf, frame.pixels);
        inst.memory.write(&mut inst.store, base, &buf).map_err(|e| format!("ec_alloc returned a buffer out of bounds: {e}"))?;
        let px_ptr = (base + params_bytes) as i32;
        let r = match &inst.render {
            Renderer::V1(f) => f
                .call(&mut inst.store, (px_ptr, frame.width as i32, frame.height as i32, base as i32, params.flat.len() as i32, time, frame.scale))
                .map_err(|e| format!("ec_render trapped: {e}")),
            Renderer::V2(_) => return Err("the module implements WebAssembly ABI 1 but exports ec_render2".into()),
        };
        let r = match r {
            Ok(0) => {
                inst.memory.read(&inst.store, base + params_bytes, &mut buf[params_bytes..]).map_err(err)?;
                read_pixels(&buf[params_bytes..], frame.pixels);
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

    /// ABI 2 through the shared pool (plug-ins that do not use `source_at`).
    fn render_v2_buffers(
        &self,
        frame: &mut PluginFrame,
        params: &PluginParams,
        time: f64,
        prev: Option<(f64, &PluginImage)>,
        next: Option<(f64, &PluginImage)>,
    ) -> Result<(), String> {
        let mut inst = self.take()?;
        let r = render_v2_inst(&mut inst, frame, params, time, prev, next);
        if r.is_ok() {
            self.give_back(inst);
        }
        r
    }
}

/// ABI 2: parameters, the current frame, then the previous and next frames; `ec_render2`.
/// The neighbour buffers are always allocated (zeroed when absent), so a module can read
/// both without a null check; `flags` says which hold real frames.
fn render_v2_inst<T>(
    inst: &mut Inst<T>,
    frame: &mut PluginFrame,
    params: &PluginParams,
    time: f64,
    prev: Option<(f64, &PluginImage)>,
    next: Option<(f64, &PluginImage)>,
) -> Result<(), String> {
    let n = frame.pixels.len();
    let params_bytes = params.flat.len() * 8;
    let frame_bytes = n * 16;
    let total = params_bytes + frame_bytes * 3;
    if total > i32::MAX as usize {
        return Err("frame too large for a 32-bit plug-in".into());
    }
    let prev_px = prev.map(|(t, img)| (t, img.on_grid(frame.width, frame.height, frame.origin, frame.scale)));
    let next_px = next.map(|(t, img)| (t, img.on_grid(frame.width, frame.height, frame.origin, frame.scale)));
    let prev = prev_px.as_ref().filter(|(_, px)| px.len() == n);
    let next = next_px.as_ref().filter(|(_, px)| px.len() == n);
    inst.store.set_fuel(FUEL_BASE + FUEL_PER_PIXEL * (n as u64 * 3)).map_err(err)?;
    let base = inst.alloc.call(&mut inst.store, total as i32).map_err(err)? as u32 as usize;
    let mut buf: Vec<u8> = Vec::with_capacity(total);
    for v in params.flat {
        buf.extend_from_slice(&v.to_le_bytes());
    }
    push_pixels(&mut buf, frame.pixels);
    push_neighbour(&mut buf, prev, frame_bytes);
    push_neighbour(&mut buf, next, frame_bytes);
    inst.memory.write(&mut inst.store, base, &buf).map_err(|e| format!("ec_alloc returned a buffer out of bounds: {e}"))?;
    let px_ptr = (base + params_bytes) as i32;
    let prev_ptr = (base + params_bytes + frame_bytes) as i32;
    let next_ptr = (base + params_bytes + frame_bytes * 2) as i32;
    let flags = i32::from(prev.is_some()) | (i32::from(next.is_some()) << 1);
    let prev_time = prev.map_or(0.0, |(t, _)| *t);
    let next_time = next.map_or(0.0, |(t, _)| *t);
    let r = match &inst.render {
        Renderer::V2(f) => f
            .call(
                &mut inst.store,
                (
                    px_ptr,
                    frame.width as i32,
                    frame.height as i32,
                    base as i32,
                    params.flat.len() as i32,
                    time,
                    frame.scale,
                    prev_ptr,
                    prev_time,
                    next_ptr,
                    next_time,
                    flags,
                ),
            )
            .map_err(|e| format!("ec_render2 trapped: {e}")),
        Renderer::V1(_) => return Err("the module implements WebAssembly ABI 2 but exports ec_render".into()),
    };
    match r {
        Ok(0) => {
            let mut out = vec![0u8; frame_bytes];
            inst.memory.read(&inst.store, base + params_bytes, &mut out).map_err(err)?;
            read_pixels(&out, frame.pixels);
            Ok(())
        }
        Ok(code) => Err(format!("ec_render2 returned error {code}")),
        Err(e) => Err(e),
    }
}

/// Append pixels to the little-endian buffer the module reads.
fn push_pixels(buf: &mut Vec<u8>, px: &[[f32; 4]]) {
    for p in px {
        for c in p {
            buf.extend_from_slice(&c.to_le_bytes());
        }
    }
}

/// The neighbour's pixels, or `frame_bytes` zeroed bytes when there is no frame.
fn push_neighbour(buf: &mut Vec<u8>, neighbour: Option<&(f64, Vec<[f32; 4]>)>, frame_bytes: usize) {
    match neighbour {
        Some((_, px)) => push_pixels(buf, px),
        None => buf.resize(buf.len() + frame_bytes, 0),
    }
}

/// Read the little-endian pixels the module wrote back.
fn read_pixels(bytes: &[u8], pixels: &mut [[f32; 4]]) {
    for (px, b) in pixels.iter_mut().zip(bytes.as_chunks::<16>().0.iter()) {
        for (c, v) in px.iter_mut().zip(b.as_chunks::<4>().0.iter()) {
            *c = f32::from_le_bytes([v[0], v[1], v[2], v[3]]);
        }
    }
}

/// The layer times of the frames around `time` on a `fps` grid: the frame before and after it
/// at a frame boundary, the two frames that bracket it between frames.
fn neighbour_times(time: f64, fps: f64) -> (f64, f64) {
    if !fps.is_finite() || fps <= 0.0 {
        return (time - 1.0 / 30.0, time + 1.0 / 30.0);
    }
    let step = 1.0 / fps;
    let at = time * fps;
    // Frame times are not exact in floating point: at NTSC rates `time * fps` often lands a few
    // ulps above or below an integer. Treat a near-integer as a frame boundary, so the current
    // frame is never handed back as its own neighbour.
    if (at - at.round()).abs() < 1e-6 { (time - step, time + step) } else { (at.floor() / fps, at.ceil() / fps) }
}

impl EffectPlugin for WasmPlugin {
    fn manifest(&self) -> &PluginManifest {
        &self.manifest
    }

    fn source(&self) -> String {
        self.source.clone()
    }

    fn reads_other_times(&self) -> bool {
        self.abi >= 2
    }

    fn render(&self, frame: &mut PluginFrame, params: &PluginParams, time: f64) -> Result<(), String> {
        if self.abi == 1 { self.render_v1(frame, params, time) } else { self.render_v2_buffers(frame, params, time, None, None) }
    }

    fn render_v2(&self, frame: &mut PluginFrame, params: &PluginParams, time: f64, host: &dyn PluginHost) -> Result<(), String> {
        if self.abi == 1 {
            return self.render_v1(frame, params, time);
        }
        let (prev_time, next_time) = neighbour_times(time, host.frame_rate());
        let prev = host.input_at(prev_time);
        let next = host.input_at(next_time);
        let prev = prev.as_ref().map(|img| (prev_time, img));
        let next = next.as_ref().map(|img| (next_time, img));
        if self.source_at {
            // The store borrows `host`, so this instance serves exactly one render.
            let mut inst = instantiate_source_at(&self.engine, &self.module, host, frame.width, frame.height, frame.origin, frame.scale)?;
            render_v2_inst(&mut inst, frame, params, time, prev, next)
        } else {
            self.render_v2_buffers(frame, params, time, prev, next)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::neighbour_times;

    /// NTSC frame times do not multiply back exactly; every frame boundary must still get the
    /// frames either side of it, never the frame itself.
    #[test]
    fn frame_boundaries_get_their_neighbours_at_ntsc_rates() {
        let fps = 24000.0 / 1001.0;
        for n in 0..2400 {
            // 24000/1001 fps at 254 016 000 000 ticks/s, as `Tick` stores it.
            let time = (n * 10_594_584_000_i64) as f64 / 254_016_000_000.0;
            let (prev, next) = neighbour_times(time, fps);
            assert!((prev - (time - 1.0 / fps)).abs() < 1e-9, "frame {n}: prev {prev}, current {time}");
            assert!((next - (time + 1.0 / fps)).abs() < 1e-9, "frame {n}: next {next}, current {time}");
        }
    }
}
