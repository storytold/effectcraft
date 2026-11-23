//! The wasmtime-backed (Cranelift compiled) [`EffectPlugin`] (WebAssembly ABI v1 and v2, plus
//! optional multi-core row-band rendering via `ec_render_rows`; #3206).

use std::sync::{Mutex, OnceLock};

use rayon::prelude::*;
use wasmtime::{Config, Engine, Instance, Linker, Memory, Module, Store, StoreLimits, StoreLimitsBuilder, TypedFunc, WasmParams, WasmResults};

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
/// Minimum rows per band when splitting a frame across cores (`ec_render_rows`).
const MIN_ROWS_PER_BAND: u32 = 32;

/// ABI 1: one buffer.
type RenderFn = TypedFunc<(i32, i32, i32, i32, i32, f64, f64), i32>;
/// ABI 2: the same call plus the previous and next frame buffers and their layer times.
type Render2Fn = TypedFunc<(i32, i32, i32, i32, i32, f64, f64, i32, f64, i32, f64, i32), i32>;
/// Optional tiled row-band entry point (#3206):
/// `ec_render_rows(pixels_ptr, width, band_height, params_ptr, nparams, time, scale, y0, y1) -> i32`
/// where `pixels_ptr` holds rows `[band_start..band_end)` (`band_height` rows including overlap
/// `margin`) and `[y0..y1)` are the output rows within `pixels_ptr` (relative to `band_start`)
/// that this worker owns.
type RenderRowsFn = TypedFunc<(i32, i32, i32, i32, i32, f64, f64, i32, i32), i32>;

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
    render_rows: Option<RenderRowsFn>,
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
    fn func<P: WasmParams, R: WasmResults>(&mut self, name: &str) -> Result<TypedFunc<P, R>, String> {
        self.instance.get_typed_func::<P, R>(&mut self.store, name).map_err(|_| missing(name))
    }

    /// Select the render export for `abi` (and the optional `ec_render_rows` export).
    fn finish(mut self, abi: u32) -> Result<Inst, String> {
        let render = match abi {
            1 => Renderer::V1(self.instance.get_typed_func::<_, i32>(&mut self.store, "ec_render").map_err(|_| missing("ec_render"))?),
            2 => Renderer::V2(self.instance.get_typed_func::<_, i32>(&mut self.store, "ec_render2").map_err(|_| missing("ec_render2"))?),
            v => return Err(unsupported(v as i32)),
        };
        let render_rows = self.instance.get_typed_func::<_, i32>(&mut self.store, "ec_render_rows").ok();
        Ok(Inst { store: self.store, memory: self.memory, alloc: self.alloc, render, render_rows })
    }
}

/// One shared Cranelift engine for all WebAssembly effect plug-ins, configured with fuel
/// metering, SIMD (`v128`), and deterministic NaN canonicalisation (#3206).
fn shared_engine() -> Result<&'static Engine, String> {
    static ENGINE: OnceLock<Result<Engine, String>> = OnceLock::new();
    ENGINE
        .get_or_init(|| {
            let mut config = Config::new();
            config.consume_fuel(true);
            config.cranelift_nan_canonicalization(true);
            config.wasm_simd(true);
            config.wasm_relaxed_simd(false);
            Engine::new(&config).map_err(err)
        })
        .as_ref()
        .map_err(Clone::clone)
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

/// The error for a wasmtime or trap failure.
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
    let instance = linker.instantiate(&mut store, module).map_err(|e| format!("instantiation failed: {e}"))?;
    let memory = instance.get_memory(&mut store, "memory").ok_or("the module exports no `memory`")?;
    let alloc = instance.get_typed_func::<i32, i32>(&mut store, "ec_alloc").map_err(|_| missing("ec_alloc"))?;
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
    /// Whether the module exports `ec_render_rows` for parallel row-band rendering (#3206).
    has_render_rows: bool,
    engine: &'static Engine,
    module: Module,
    /// Idle instances.
    pool: Mutex<Vec<Inst>>,
}

impl WasmPlugin {
    /// Load `bytes` and its manifest; errors on imports, a bad manifest or a missing export.
    pub fn new(bytes: &[u8], source: &str) -> Result<WasmPlugin, String> {
        let engine = shared_engine()?;
        let module = Module::new(engine, bytes).map_err(|e| format!("not a valid WebAssembly module: {e}"))?;
        if let Some(i) = module.imports().next() {
            return Err(format!("plug-ins can't import anything (the module imports `{}.{}`)", i.module(), i.name()));
        }
        let mut pending = begin(engine, &module)?;
        let version = pending.func::<(), i32>("ec_api_version")?;
        let abi = abi_of(version.call(&mut pending.store, ()).map_err(err)?)?;
        let ptr_fn = pending.func::<(), i32>("ec_manifest_ptr")?;
        let ptr = ptr_fn.call(&mut pending.store, ()).map_err(err)?;
        let len_fn = pending.func::<(), i32>("ec_manifest_len")?;
        let len = len_fn.call(&mut pending.store, ()).map_err(err)?;
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
        let has_render_rows = inst.render_rows.is_some();
        Ok(WasmPlugin { manifest, source: source.to_string(), abi, has_render_rows, engine, module, pool: Mutex::new(vec![inst]) })
    }

    /// An idle instance, or a fresh one when the pool is empty.
    fn take(&self) -> Result<Inst, String> {
        if let Some(i) = self.pool.lock().ok().and_then(|mut p| p.pop()) {
            return Ok(i);
        }
        instantiate(self.engine, &self.module, self.abi)
    }

    /// Return a working instance to the pool.
    fn give_back(&self, inst: Inst) {
        if let Ok(mut p) = self.pool.lock() {
            p.push(inst);
        }
    }

    /// Parallel tiled row-band render for ABI 1 plug-ins exporting `ec_render_rows` (#3206).
    fn render_rows_parallel(&self, frame: &mut PluginFrame, params: &PluginParams, time: f64) -> Result<(), String> {
        let w = frame.width as usize;
        let h = frame.height;
        let threads = rayon::current_num_threads().max(1);
        let bands = threads.min((h / MIN_ROWS_PER_BAND).max(1) as usize);
        if bands <= 1 {
            return self.render_v1_single(frame, params, time);
        }
        let margin = self.manifest.margin.min(h);
        let src: Vec<[f32; 4]> = frame.pixels.to_vec();
        let rows_per = h.div_ceil(bands as u32);
        let ranges: Vec<(u32, u32)> = (0..bands)
            .map(|b| {
                let y0 = b as u32 * rows_per;
                let y1 = ((b as u32 + 1) * rows_per).min(h);
                (y0, y1)
            })
            .filter(|(y0, y1)| y0 < y1)
            .collect();
        let params_bytes = params.flat.len() * 8;
        let mut pbytes = Vec::with_capacity(params_bytes);
        for v in params.flat {
            pbytes.extend_from_slice(&v.to_le_bytes());
        }
        let scale = frame.scale;
        let results: Result<Vec<(u32, Vec<[f32; 4]>)>, String> = ranges
            .into_par_iter()
            .map(|(out_y0, out_y1)| {
                let band_start = out_y0.saturating_sub(margin);
                let band_end = (out_y1 + margin).min(h);
                let band_h = band_end - band_start;
                let band_px = w * band_h as usize;
                let total = params_bytes + band_px * 16;
                if total > i32::MAX as usize {
                    return Err("frame band too large for a 32-bit plug-in".into());
                }
                let mut inst = self.take()?;
                let Some(ref rows_fn) = inst.render_rows else {
                    self.give_back(inst);
                    return Err("missing ec_render_rows export".into());
                };
                inst.store.set_fuel(FUEL_BASE + FUEL_PER_PIXEL * band_px as u64).map_err(err)?;
                let base = inst.alloc.call(&mut inst.store, total as i32).map_err(err)? as u32 as usize;
                let mut buf = Vec::with_capacity(total);
                buf.extend_from_slice(&pbytes);
                push_pixels(&mut buf, &src[band_start as usize * w..band_end as usize * w]);
                inst.memory.write(&mut inst.store, base, &buf).map_err(|e| format!("ec_alloc returned a buffer out of bounds: {e}"))?;
                let px_ptr = (base + params_bytes) as i32;
                let local_y0 = (out_y0 - band_start) as i32;
                let local_y1 = (out_y1 - band_start) as i32;
                let r = rows_fn
                    .call(&mut inst.store, (px_ptr, w as i32, band_h as i32, base as i32, params.flat.len() as i32, time, scale, local_y0, local_y1))
                    .map_err(|e| format!("ec_render_rows trapped: {e}"));
                let out = match r {
                    Ok(0) => {
                        let out_rows = (out_y1 - out_y0) as usize;
                        let out_off = base + params_bytes + local_y0 as usize * w * 16;
                        let mut raw = vec![0u8; out_rows * w * 16];
                        inst.memory.read(&inst.store, out_off, &mut raw).map_err(err)?;
                        let mut band_out = vec![[0.0f32; 4]; out_rows * w];
                        read_pixels(&raw, &mut band_out);
                        Ok((out_y0, band_out))
                    }
                    Ok(code) => Err(format!("ec_render_rows returned error {code}")),
                    Err(e) => Err(e),
                };
                if out.is_ok() {
                    self.give_back(inst);
                }
                out
            })
            .collect();
        for (y0, band_out) in results? {
            let start = y0 as usize * w;
            frame.pixels[start..start + band_out.len()].copy_from_slice(&band_out);
        }
        Ok(())
    }

    /// ABI 1: parameters, then the pixels; `ec_render` (or parallel `ec_render_rows` when exported).
    fn render_v1(&self, frame: &mut PluginFrame, params: &PluginParams, time: f64) -> Result<(), String> {
        if self.has_render_rows && frame.height >= MIN_ROWS_PER_BAND * 2 {
            return self.render_rows_parallel(frame, params, time);
        }
        self.render_v1_single(frame, params, time)
    }

    /// Single-instance ABI 1 render via `ec_render`.
    fn render_v1_single(&self, frame: &mut PluginFrame, params: &PluginParams, time: f64) -> Result<(), String> {
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

    /// ABI 2: parameters, the current frame, then the previous and next frames; `ec_render2`.
    /// The neighbour buffers are always allocated (zeroed when absent), so a module can read
    /// both without a null check; `flags` says which hold real frames.
    fn render_v2(
        &self,
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
        let mut inst = self.take()?;
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
        let r = match r {
            Ok(0) => {
                let mut out = vec![0u8; frame_bytes];
                inst.memory.read(&inst.store, base + params_bytes, &mut out).map_err(err)?;
                read_pixels(&out, frame.pixels);
                Ok(())
            }
            Ok(code) => Err(format!("ec_render2 returned error {code}")),
            Err(e) => Err(e),
        };
        if r.is_ok() {
            self.give_back(inst);
        }
        r
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
        if self.abi == 1 { self.render_v1(frame, params, time) } else { self.render_v2(frame, params, time, None, None) }
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
        self.render_v2(frame, params, time, prev, next)
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
