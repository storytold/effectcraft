//! The wasmi-backed [`EffectPlugin`] (WebAssembly ABI v1 and v2, see the crate docs).

use std::sync::Mutex;

use wasmi::{Config, Engine, Instance, Linker, Memory, Module, Store, StoreLimits, StoreLimitsBuilder, TypedFunc, WasmParams, WasmResults};

use crate::{EffectPlugin, MIN_WASM_API_VERSION, PluginFrame, PluginManifest, PluginParams, WASM_API_VERSION};
use effectcraft_effects::plugin::{PluginHost, PluginImage};

/// Instruction budget per frame: a fixed allowance plus some per pixel. Generous for per-pixel
/// work, but a runaway loop stops instead of hanging the render.
const FUEL_BASE: u64 = 50_000_000;
const FUEL_PER_PIXEL: u64 = 20_000;
/// The most linear memory a plug-in instance may have (a 4K float frame is ~130 MB).
const MAX_MEMORY: usize = 1 << 30;
/// The most elements one table of a plug-in may hold.
const MAX_TABLE_ELEMENTS: usize = 1 << 16;
/// The longest manifest read.
const MAX_MANIFEST: i32 = 1 << 20;

/// ABI 1: one buffer.
type RenderFn = TypedFunc<(i32, i32, i32, i32, i32, f64, f64), i32>;
/// ABI 2: the same call plus the previous and next frame buffers and their layer times.
type Render2Fn = TypedFunc<(i32, i32, i32, i32, i32, f64, f64, i32, f64, i32, f64, i32), i32>;
/// Optional `ec_padding(params, nparams, time, scale, layer_w, layer_h) -> pixels`.
type PaddingFn = TypedFunc<(i32, i32, f64, f64, f64, f64), i32>;

/// `ec_flags` bit 0: the render depends on `time` (the render cache keys the layer on it).
const FLAG_TIME_VARYING: i32 = 1;
/// `ec_flags` bit 1: an ABI 2 module reads the previous and next frames.
const FLAG_NEIGHBOURS: i32 = 2;
/// The most transparent pixels `ec_padding` may ask for per side.
const MAX_PADDING: i32 = 4096;

enum Renderer {
    V1(RenderFn),
    V2(Render2Fn),
}

/// One instantiated module (renders running on several threads each take their own).
struct Inst {
    store: Store<StoreLimits>,
    memory: Memory,
    alloc: TypedFunc<i32, i32>,
    render: Renderer,
    padding: Option<PaddingFn>,
}

/// An instance whose ABI version (and so its render export) is not selected yet.
struct Pending {
    store: Store<StoreLimits>,
    instance: Instance,
    memory: Memory,
    alloc: TypedFunc<i32, i32>,
}

impl Pending {
    /// A typed export by name, reporting a missing or mistyped export like [`missing`].
    fn func<P: WasmParams, R: WasmResults>(&self, name: &str) -> Result<TypedFunc<P, R>, String> {
        self.instance.get_typed_func::<P, R>(&self.store, name).map_err(|_| missing(name))
    }

    /// An optional export: `None` when absent, an error when it has the wrong signature.
    fn optional<P: WasmParams, R: WasmResults>(&self, name: &str) -> Result<Option<TypedFunc<P, R>>, String> {
        if self.instance.get_export(&self.store, name).is_none() {
            return Ok(None);
        }
        self.func(name).map(Some)
    }

    /// Select the render export for `abi`.
    fn finish(self, abi: u32) -> Result<Inst, String> {
        let render = match abi {
            1 => Renderer::V1(self.func("ec_render")?),
            2 => Renderer::V2(self.func("ec_render2")?),
            v => return Err(unsupported(v as i32)),
        };
        let padding = self.optional("ec_padding")?;
        Ok(Inst { store: self.store, memory: self.memory, alloc: self.alloc, render, padding })
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

/// Instantiate `module` in a fresh, limited store and read its memory and allocator.
fn begin(engine: &Engine, module: &Module) -> Result<Pending, String> {
    // One memory and a few small tables: a module declaring many of them would otherwise make
    // the host allocate and initialise each before an export or fuel check could refuse it.
    let limits = StoreLimitsBuilder::new().memory_size(MAX_MEMORY).memories(1).tables(4).table_elements(MAX_TABLE_ELEMENTS).instances(1).build();
    let mut store = Store::new(engine, limits);
    store.limiter(|l| l);
    store.set_fuel(FUEL_BASE).map_err(err)?;
    let linker = Linker::<StoreLimits>::new(engine);
    let instance = linker.instantiate_and_start(&mut store, module).map_err(|e| format!("instantiation failed: {e}"))?;
    let memory = instance.get_memory(&store, "memory").ok_or("the module exports no `memory`")?;
    let alloc = instance.get_typed_func::<i32, i32>(&store, "ec_alloc").map_err(|_| missing("ec_alloc"))?;
    Ok(Pending { store, instance, memory, alloc })
}

/// Instantiate `module` and select the render export for `abi`.
fn instantiate(engine: &Engine, module: &Module, abi: u32) -> Result<Inst, String> {
    begin(engine, module)?.finish(abi)
}

/// A loaded WebAssembly effect plug-in.
pub struct WasmPlugin {
    manifest: PluginManifest,
    source: String,
    /// The WebAssembly ABI the module implements (`ec_api_version`).
    abi: u32,
    /// `ec_flags` ([`FLAG_TIME_VARYING`], [`FLAG_NEIGHBOURS`]); without the export, both for
    /// ABI 2 and none for ABI 1.
    flags: i32,
    /// Whether the module exports `ec_padding`.
    pads: bool,
    engine: Engine,
    module: Module,
    /// Idle instances.
    pool: Mutex<Vec<Inst>>,
}

impl WasmPlugin {
    /// Load `bytes` and its manifest; errors on imports, a bad manifest or a missing export.
    pub fn new(bytes: &[u8], source: &str) -> Result<WasmPlugin, String> {
        let mut config = Config::default();
        config.consume_fuel(true);
        let engine = Engine::new(&config);
        let module = Module::new(&engine, bytes).map_err(|e| format!("not a valid WebAssembly module: {e}"))?;
        if let Some(i) = module.imports().next() {
            return Err(format!("plug-ins can't import anything (the module imports `{}.{}`)", i.module(), i.name()));
        }
        let mut pending = begin(&engine, &module)?;
        let version = pending.func::<(), i32>("ec_api_version")?;
        let abi = abi_of(version.call(&mut pending.store, ()).map_err(err)?)?;
        let flags = match pending.optional::<(), i32>("ec_flags")? {
            Some(f) => f.call(&mut pending.store, ()).map_err(err)?,
            None if abi >= 2 => FLAG_TIME_VARYING | FLAG_NEIGHBOURS,
            None => 0,
        };
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
        let inst = pending.finish(abi)?;
        let pads = inst.padding.is_some();
        Ok(WasmPlugin { manifest, source: source.to_string(), abi, flags, pads, engine, module, pool: Mutex::new(vec![inst]) })
    }

    /// Whether `ec_render2` gets the neighbouring frames.
    fn wants_neighbours(&self) -> bool {
        self.abi >= 2 && self.flags & FLAG_NEIGHBOURS != 0
    }

    /// An idle instance, or a fresh one when the pool is empty.
    fn take(&self) -> Result<Inst, String> {
        if let Some(i) = self.pool.lock().ok().and_then(|mut p| p.pop()) {
            return Ok(i);
        }
        instantiate(&self.engine, &self.module, self.abi)
    }

    /// Return a working instance to the pool.
    fn give_back(&self, inst: Inst) {
        if let Ok(mut p) = self.pool.lock() {
            p.push(inst);
        }
    }

    /// `ec_padding`, clamped to 0..=[`MAX_PADDING`]; 0 without the export.
    fn call_padding(&self, params: &PluginParams, time: f64, scale: f64, layer_size: [f64; 2]) -> Result<u32, String> {
        if !self.pads {
            return Ok(0);
        }
        let params_bytes = params.flat.len() * 8;
        let mut inst = self.take()?;
        let Some(f) = inst.padding else { return Ok(0) };
        inst.store.set_fuel(FUEL_BASE).map_err(err)?;
        let base = inst.alloc.call(&mut inst.store, params_bytes as i32).map_err(err)? as u32 as usize;
        let mem = inst.memory.data_mut(&mut inst.store);
        let dst = base.checked_add(params_bytes).and_then(|end| mem.get_mut(base..end)).ok_or("ec_alloc returned a buffer out of bounds")?;
        write_f64s(dst, params.flat);
        let pad = f
            .call(&mut inst.store, (base as i32, params.flat.len() as i32, time, scale, layer_size[0], layer_size[1]))
            .map_err(|e| format!("ec_padding trapped: {e}"))?;
        self.give_back(inst);
        Ok(pad.clamp(0, MAX_PADDING) as u32)
    }

    /// Parameters, the current frame and (for modules that read them) the previous and next
    /// frames in one buffer, then the render export. A module that reads the neighbours always
    /// gets both buffers (zeroed when a frame does not exist), so it can read them without a
    /// null check; `flags` says which hold real frames. Other modules get null pointers.
    fn render_with(
        &self,
        frame: &mut PluginFrame,
        params: &PluginParams,
        time: f64,
        prev: Option<(f64, &PluginImage)>,
        next: Option<(f64, &PluginImage)>,
    ) -> Result<(), String> {
        let n = frame.pixels.len();
        let params_bytes = params.flat.len() * 8;
        let neighbours = self.wants_neighbours();
        let frames = if neighbours { 3 } else { 1 };
        let frame_bytes = n.checked_mul(16).ok_or("frame too large")?;
        let total = frame_bytes
            .checked_mul(frames)
            .and_then(|b| b.checked_add(params_bytes))
            .filter(|t| *t <= i32::MAX as usize)
            .ok_or("frame too large for a 32-bit plug-in")?;
        let on_grid = |(t, img): (f64, &PluginImage)| (t, img.on_grid(frame.width, frame.height, frame.origin, frame.scale));
        let prev = prev.filter(|_| neighbours).map(on_grid).filter(|(_, px)| px.len() == n);
        let next = next.filter(|_| neighbours).map(on_grid).filter(|(_, px)| px.len() == n);
        let mut inst = self.take()?;
        inst.store.set_fuel(FUEL_BASE + FUEL_PER_PIXEL * (n * frames) as u64).map_err(err)?;
        let base = inst.alloc.call(&mut inst.store, total as i32).map_err(err)? as u32 as usize;
        // Straight into the module's memory, little-endian as WebAssembly memory is.
        let mem = inst.memory.data_mut(&mut inst.store);
        let buf = base.checked_add(total).and_then(|end| mem.get_mut(base..end)).ok_or("ec_alloc returned a buffer out of bounds")?;
        let (p, rest) = buf.split_at_mut_checked(params_bytes).ok_or("buffer too small")?;
        write_f64s(p, params.flat);
        let (cur, rest) = rest.split_at_mut_checked(frame_bytes).ok_or("buffer too small")?;
        write_pixels(cur, frame.pixels);
        if neighbours {
            let (a, b) = rest.split_at_mut_checked(frame_bytes).ok_or("buffer too small")?;
            write_neighbour(a, prev.as_ref());
            write_neighbour(b, next.as_ref());
        }
        // `base + total` is inside memory (at most 1 GiB), so these pointers fit an i32.
        let px = base + params_bytes;
        let (w, h, np) = (frame.width as i32, frame.height as i32, params.flat.len() as i32);
        let (name, r) = match &inst.render {
            Renderer::V1(f) => ("ec_render", f.call(&mut inst.store, (px as i32, w, h, base as i32, np, time, frame.scale))),
            Renderer::V2(f) => {
                let (prev_ptr, next_ptr) = if neighbours { ((px + frame_bytes) as i32, (px + frame_bytes * 2) as i32) } else { (0, 0) };
                let flags = i32::from(prev.is_some()) | (i32::from(next.is_some()) << 1);
                let prev_time = prev.as_ref().map_or(0.0, |(t, _)| *t);
                let next_time = next.as_ref().map_or(0.0, |(t, _)| *t);
                ("ec_render2", f.call(&mut inst.store, (px as i32, w, h, base as i32, np, time, frame.scale, prev_ptr, prev_time, next_ptr, next_time, flags)))
            }
        };
        match r.map_err(|e| format!("{name} trapped: {e}"))? {
            0 => {}
            code => return Err(format!("{name} returned error {code}")),
        }
        let out = inst.memory.data(&inst.store).get(px..px + frame_bytes).ok_or("ec_alloc returned a buffer out of bounds")?;
        read_pixels(out, frame.pixels);
        self.give_back(inst);
        Ok(())
    }
}

/// Write `f64`s little-endian into `dst`.
fn write_f64s(dst: &mut [u8], v: &[f64]) {
    for (d, x) in dst.as_chunks_mut::<8>().0.iter_mut().zip(v) {
        *d = x.to_le_bytes();
    }
}

/// Write pixels little-endian into `dst`.
fn write_pixels(dst: &mut [u8], px: &[[f32; 4]]) {
    for (d, c) in dst.as_chunks_mut::<4>().0.iter_mut().zip(px.as_flattened()) {
        *d = c.to_le_bytes();
    }
}

/// The neighbour's pixels, or zeros when there is no frame.
fn write_neighbour(dst: &mut [u8], neighbour: Option<&(f64, Vec<[f32; 4]>)>) {
    match neighbour {
        Some((_, px)) => write_pixels(dst, px),
        None => dst.fill(0),
    }
}

/// Read the little-endian pixels the module wrote back.
fn read_pixels(src: &[u8], pixels: &mut [[f32; 4]]) {
    for (c, s) in pixels.as_flattened_mut().iter_mut().zip(src.as_chunks::<4>().0) {
        *c = f32::from_le_bytes(*s);
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
        self.flags & (FLAG_TIME_VARYING | FLAG_NEIGHBOURS) != 0
    }

    fn padding(&self, params: &PluginParams, time: f64, scale: f64, layer_size: [f64; 2], _instance_key: u64) -> u32 {
        // A failing `ec_padding` pads nothing; the render reports the module's errors.
        self.call_padding(params, time, scale, layer_size).unwrap_or(0)
    }

    fn render(&self, frame: &mut PluginFrame, params: &PluginParams, time: f64) -> Result<(), String> {
        self.render_with(frame, params, time, None, None)
    }

    fn render_v2(&self, frame: &mut PluginFrame, params: &PluginParams, time: f64, host: &dyn PluginHost) -> Result<(), String> {
        if !self.wants_neighbours() {
            return self.render_with(frame, params, time, None, None);
        }
        let (prev_time, next_time) = neighbour_times(time, host.frame_rate());
        let prev = host.input_at(prev_time);
        let next = host.input_at(next_time);
        self.render_with(frame, params, time, prev.as_ref().map(|img| (prev_time, img)), next.as_ref().map(|img| (next_time, img)))
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
