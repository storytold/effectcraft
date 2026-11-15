//! Device, pipelines, textures, uploads and readback.

use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, Mutex, Weak};

use effectcraft_effects::Buf;
use effectcraft_raster::Image;

/// Compute entry points in `kernels.wgsl` (one pipeline each).
const ENTRIES: &[&str] = &[
    "warp_blend",
    "blend_full",
    "matte",
    "preserve",
    "knockout",
    "channel_mix",
    "quantize",
    "convert",
    "half",
    "adjust_mix",
    "bokeh_boost",
    "bokeh_prefix",
    "bokeh_gather",
    "fill",
];

/// Effect entry points in `kernels.wgsl`. Like the `fx_*` kernels, they are built on first use
/// (building all ~200 up front kept Chrome's GPU process busy past its watchdog, #1590, and
/// FXC took over a minute, #613), and one that fails to build (a driver's shader compiler
/// rejecting it) leaves its effects to the CPU instead of turning the whole compositor off.
const EFFECT_ENTRIES: &[&str] = &["box_h", "box_v", "directional", "glow_bright", "glow_combine", "shadow_make", "shadow_combine", "pointwise"];

/// Entry points that also bind group 1 (four read-only storage buffers; see
/// [`Enc::dispatch_ext`]). Like the effect kernels, one that fails to build (FXC on DirectX 12
/// can't unroll `classic3d`'s loops, #613) leaves its work to the CPU.
const EXT_ENTRIES: &[&str] = &["classic3d"];

/// What every kernel module starts with: the shared helpers and bindings, and `kernels.wgsl`
/// (the compositor's kernels, which declare `out` and helpers the effect files use).
const BASE_WGSL: [&str; 2] = [include_str!("shaders/common.wgsl"), include_str!("shaders/kernels.wgsl")];

/// The family whose module also gets the adapter's median implementation
/// ([`crate::fx_noise::MedianKernel::source`]), chosen when the context is created.
const MEDIAN_FAMILY: &str = "fx_noise";

/// Kernel families: each is compiled as a module of its own, `BASE_WGSL` plus its file and the
/// files it borrows functions from, built the first time one of its kernels is. A pipeline then
/// compiles at most ~120K characters of WGSL instead of all ~460K: on WebGPU each pipeline
/// compiles its module again, and the page's and its workers' devices doing that for every
/// kernel kept Chrome's GPU process busy past its watchdog (#1590). Family 0 is the base module.
const FAMILIES: &[(&str, &[&str], &[&str])] = &[
    ("kernels", &[], EFFECT_ENTRIES),
    ("fx_color", &[include_str!("shaders/fx_lut.wgsl"), include_str!("shaders/fx_color.wgsl")], crate::fx_color::KERNELS),
    ("fx_distort", &[include_str!("shaders/fx_distort.wgsl")], crate::fx_distort::KERNELS),
    ("fx_generate", &[include_str!("shaders/fx_generate.wgsl")], crate::fx_generate::KERNELS),
    ("fx_key", &[include_str!("shaders/fx_distort.wgsl"), include_str!("shaders/fx_key.wgsl")], crate::fx_key::KERNELS),
    ("fx_stylize", &[include_str!("shaders/fx_stylize.wgsl")], crate::fx_stylize::KERNELS),
    (MEDIAN_FAMILY, &[include_str!("shaders/fx_noise.wgsl")], crate::fx_noise::KERNELS),
    ("fx_tone", &[include_str!("shaders/fx_tone.wgsl")], crate::fx_tone::KERNELS),
    ("fx_warp", &[include_str!("shaders/fx_warp.wgsl")], crate::fx_warp::KERNELS),
    ("fx_extra", &[include_str!("shaders/fx_generate.wgsl"), include_str!("shaders/fx_extra.wgsl")], crate::fx_extra::KERNELS),
    ("fx_depth", &[include_str!("shaders/fx_depth.wgsl")], crate::fx_depth::KERNELS),
    ("fx_lut", &[include_str!("shaders/fx_lut.wgsl")], crate::fx_lut::KERNELS),
    ("fx_sim", &[include_str!("shaders/fx_sim.wgsl")], crate::fx_sim::KERNELS),
    (
        "fx_particles",
        &[include_str!("shaders/fx_stylize.wgsl"), include_str!("shaders/fx_sim.wgsl"), include_str!("shaders/fx_particles.wgsl")],
        crate::fx_particles::KERNELS,
    ),
    ("fx_vr", &[include_str!("shaders/fx_vr.wgsl")], crate::fx_vr::KERNELS),
    ("fx_light", &[include_str!("shaders/fx_light.wgsl")], crate::fx_light::KERNELS),
    ("fx_transition", &[include_str!("shaders/fx_transition.wgsl")], crate::fx_transition::KERNELS),
    ("fx_text", &[include_str!("shaders/fx_text.wgsl")], crate::fx_text::KERNELS),
    ("fx_time", &[include_str!("shaders/fx_time.wgsl")], crate::fx_time::KERNELS),
    ("fx_pixel2", &[include_str!("shaders/fx_stylize.wgsl"), include_str!("shaders/fx_pixel2.wgsl")], crate::fx_pixel2::KERNELS),
    ("fx_gen2", &[include_str!("shaders/fx_gen2.wgsl")], crate::fx_gen2::KERNELS),
    ("inpaint", &[include_str!("shaders/inpaint.wgsl")], crate::inpaint::KERNELS),
];

/// Module `label`: `BASE_WGSL` followed by `files`.
fn kernel_module(device: &wgpu::Device, label: &str, files: &[&str]) -> Result<wgpu::ShaderModule, String> {
    let src: String = BASE_WGSL.iter().chain(files).copied().collect();
    init_resource(device, &format!("{label} shader module"), || {
        device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some(label), source: wgpu::ShaderSource::Wgsl(src.into()) })
    })
}

/// Compute pipeline `entry` of `module`; a shader compiler rejecting it (FXC, #613) is an `Err`.
fn build_pipeline(
    device: &wgpu::Device,
    module: &wgpu::ShaderModule,
    layout: &wgpu::PipelineLayout,
    entry: &'static str,
) -> Result<wgpu::ComputePipeline, String> {
    init_resource(device, entry, || {
        device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some(entry),
            layout: Some(layout),
            module,
            entry_point: Some(entry),
            compilation_options: Default::default(),
            cache: None,
        })
    })
}

/// Effect kernels by entry point: their family (index in `FAMILIES`) and, once asked for, the
/// built pipeline or why it didn't build.
type EffectPipelines = HashMap<&'static str, (usize, std::sync::OnceLock<Result<wgpu::ComputePipeline, String>>)>;

/// Pixel format of every working texture: premultiplied RGBA, 32-bit float (32 bpc headroom;
/// 8/16 bpc are emulated by clamping and quantising, as on the CPU).
pub const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba32Float;

/// Uploaded layer buffers kept on the GPU by default (bytes; [`GpuContext::set_upload_budget`]).
const UPLOAD_BUDGET: usize = 1536 << 20;

/// Working textures kept for reuse once their last image is dropped (bytes).
const POOL_BUDGET: usize = 256 << 20;

/// Frames after which a size the pool was not asked for is stale: only stale sizes are
/// evicted to make room (see [`Pool::make_free`]).
///
/// The clock is coarse: it advances on every finished frame ([`GpuContext::pool_frame_end`])
/// from any thread, so with many frames in flight (an export renders up to 16 at once) this
/// means "not asked for during the last 2 completions", not "during 2 frames' rendering". A
/// size a slower in-flight frame still needs can go stale and be evicted. That costs a pool
/// miss (a new allocation), never a wrong pixel.
const STALE_FRAMES: u64 = 2;

/// The pool owner of uploaded textures: never registered by an [`Enc`] (their ids start at 1),
/// so an upload takes only from the shared free list ([`Pool::take`], textures no open encoder
/// can read) and gives its texture back through the guarded [`Pool::release`].
const UPLOAD_OWNER: u64 = 0;

/// Readback staging buffers kept for reuse across sizes (bytes).
const STAGING_BUDGET: u64 = 256 << 20;

/// A GPU image: premultiplied RGBA f32 in a texture (cheap to clone; immutable once written).
/// Working images come from the context's texture pool and go back to it when the last clone
/// is dropped.
#[derive(Clone, Debug)]
pub struct GpuImage {
    pub texture: wgpu::Texture,
    pub width: u32,
    pub height: u32,
    _lease: Option<Arc<Lease>>,
    /// Already clamped and quantised to this many levels (8/16 bpc), so quantising again is a
    /// no-op (see `ops::quantize`).
    pub(crate) levels: Option<f32>,
}

impl GpuImage {
    /// An image on a texture of its own (not pooled).
    pub fn new(texture: wgpu::Texture, width: u32, height: u32) -> GpuImage {
        GpuImage { texture, width, height, _lease: None, levels: None }
    }

    /// This image leaves the [`Enc`] that made it (read by an encoder that may stay open, see
    /// [`Enc::uses`]): when its last clone drops, its texture waits for every open encoder
    /// ([`Pool::release`]) instead of going back to that `Enc` for immediate reuse.
    pub(crate) fn escape(&self) {
        if let Some(l) = &self._lease {
            l.escaped.store(true, std::sync::atomic::Ordering::Relaxed);
        }
    }

    /// A pooled image of another `Enc` than `owner` (see [`GpuImage::escape`]).
    fn foreign_to(&self, owner: u64) -> bool {
        self._lease.as_ref().is_some_and(|l| l.owner != owner)
    }
}

/// Hands a pooled texture back to its [`Pool`] when the last [`GpuImage`] holding it drops.
struct Lease {
    texture: wgpu::Texture,
    pool: Weak<Mutex<Pool>>,
    /// The [`Enc`] that took it (see [`Pool::owned`]).
    owner: u64,
    /// Another [`Enc`] recorded work on it ([`Enc::uses`], [`GpuImage::escape`]):
    /// its owner can no longer reuse it at once, since that other encoder may still be open, so
    /// it is released through the guarded [`Pool::release`].
    escaped: std::sync::atomic::AtomicBool,
    /// The pool's [`Pool::generation`] when it was taken.
    generation: u64,
}

impl std::fmt::Debug for Lease {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Lease")
    }
}

impl Drop for Lease {
    fn drop(&mut self) {
        if let Some(pool) = self.pool.upgrade()
            && let Ok(mut p) = pool.lock()
        {
            // Always undo `pooled`'s leased count first: `Pool::discard` does not reset
            // `leased`, and a stale-generation lease must still clear its contribution.
            p.leased = p.leased.saturating_sub(tex_bytes(&self.texture));
            if !self.escaped.load(std::sync::atomic::Ordering::Relaxed) {
                p.release_owned(self.texture.clone(), self.owner, self.generation);
            } else if self.generation == p.generation {
                // Like `release_owned`, nothing taken before a failed frame comes back (#1319).
                p.release(self.texture.clone());
            }
        }
    }
}

/// Recycled working textures: allocating (and zero-initialising) a 1080p RGBA f32 texture
/// costs more than compositing a small layer into it, and a frame makes several per layer.
///
/// A released texture may still be read by commands recorded in an encoder that has not been
/// submitted yet. Reusing it in another encoder that is submitted first would overwrite it
/// before those reads run, so a released texture waits until every encoder that was open when
/// it was released has been submitted (or dropped).
#[derive(Default)]
pub(crate) struct Pool {
    /// Free textures by size, each with the [`Pool::stamp`] at which it was freed (oldest first).
    free: HashMap<(u32, u32), std::collections::VecDeque<(wgpu::Texture, u64)>>,
    /// Counts frees, so the free list evicts the oldest texture first.
    stamp: u64,
    /// Bytes the free list may hold (0 = [`POOL_BUDGET`]); tests lower it per context.
    budget: usize,
    /// (texture, newest encoder open at its release).
    released: Vec<(wgpu::Texture, u64)>,
    /// Encoders with recorded, unsubmitted work.
    open: BTreeSet<u64>,
    next: u64,
    bytes: usize,
    /// Bytes currently held by live [`Lease`]s.
    leased: usize,
    /// Working textures reused / newly allocated (diagnostics, tests).
    hits: u64,
    misses: u64,
    /// Free textures dropped to make room for newer ones (diagnostics, tests).
    evicted: u64,
    /// Output frames done so far ([`GpuContext::pool_frame_end`]).
    frame: u64,
    /// Per size, the [`Pool::frame`] in which it was last asked for (hit or miss).
    asked: HashMap<(u32, u32), u64>,
    /// Bytes of evicted textures whose GPU memory may not be freed yet: wgpu frees a dropped
    /// texture only once the submissions that used it are done. Counted against the budget
    /// for evicting, so evicting and reallocating cannot stack up freed-but-held memory.
    dying: Arc<std::sync::atomic::AtomicUsize>,
    /// The [`Pool::dying`] bytes left after a [`GpuContext::pool_frame_end`] wait that cleared
    /// none of them (`0` = not stalled). While `dying` does not drop below it, later frames only
    /// poll instead of waiting again (each wait could take up to [`crate::readback::TIMEOUT`]
    /// when the queue stopped completing work).
    stalled: usize,
    /// Number of empty-submission waits from [`GpuContext::pool_frame_end`] (diagnostics, tests).
    waits: u64,
    /// Reports when the submissions that may use an evicted texture are done.
    queue: Option<wgpu::Queue>,
    /// Textures released while the [`Enc`] that took them is alive, kept for that `Enc` only.
    /// Its later commands run after everything it recorded or submitted before (one queue,
    /// recording order), so it can overwrite them at once instead of allocating a new texture
    /// for every intermediate of a frame (a 4K frame holds dozens). Other encoders never see a
    /// working texture of another `Enc` before it is gone.
    owned: HashMap<u64, HashMap<(u32, u32), Vec<wgpu::Texture>>>,
    next_owner: u64,
    /// Raised by [`Pool::discard`]: a texture taken before it never comes back to the pool.
    generation: u64,
}

/// In-`Enc` texture reuse ([`Pool::owned`]); tests turn it off to compare.
pub(crate) static TEXTURE_REUSE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

/// Counts of uploads (CPU → GPU) and readbacks (GPU → CPU).
#[derive(Default)]
pub(crate) struct Transfers {
    ups: std::sync::atomic::AtomicU64,
    up_bytes: std::sync::atomic::AtomicU64,
    downs: std::sync::atomic::AtomicU64,
    down_bytes: std::sync::atomic::AtomicU64,
}

/// A snapshot of [`GpuContext::transfer_stats`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TransferStats {
    pub uploads: u64,
    pub upload_bytes: u64,
    pub readbacks: u64,
    pub readback_bytes: u64,
}

/// `WGPU_ADAPTER_NAME` trimmed; empty or whitespace-only counts as unset (an empty
/// substring would otherwise match the first adapter and skip the power preference).
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
fn wanted_adapter_name(var: Option<&str>) -> Option<&str> {
    var.map(str::trim).filter(|w| !w.is_empty())
}

/// Index of the first adapter whose name contains `want`, compared case-insensitively.
/// Only the name is matched, as in wgpu (the backend is chosen with `WGPU_BACKEND`).
/// `Err` lists each distinct adapter once as `name (backend)`, so a typo or an
/// OS/driver-specific name is easy to spot.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
fn match_adapter_name(want: &str, adapters: &[(String, String)]) -> Result<usize, String> {
    if adapters.is_empty() {
        return Err(format!("WGPU_ADAPTER_NAME={want:?}: no adapters on the enabled backends"));
    }
    let w = want.to_lowercase();
    adapters.iter().position(|(n, _)| n.to_lowercase().contains(&w)).ok_or_else(|| {
        let mut list: Vec<String> = Vec::new();
        for l in adapters.iter().map(|(n, b)| format!("{n} ({b})")) {
            if !list.contains(&l) {
                list.push(l);
            }
        }
        format!(
            "no adapter name matches WGPU_ADAPTER_NAME={want:?} (matched against the name only; \
             select a backend with WGPU_BACKEND); available: [{}]",
            list.join(", ")
        )
    })
}

/// The adapter named by `WGPU_ADAPTER_NAME` (case-insensitive substring, like wgpu's
/// `initialize_adapter_from_env`, which panics when none matches). `None` when unset or empty.
#[cfg(not(target_arch = "wasm32"))]
async fn adapter_by_name(instance: &wgpu::Instance) -> Option<Result<wgpu::Adapter, String>> {
    let var = std::env::var("WGPU_ADAPTER_NAME").ok();
    let want = wanted_adapter_name(var.as_deref())?;
    let mut all = instance.enumerate_adapters(wgpu::Backends::all()).await;
    let adapters: Vec<(String, String)> = all
        .iter()
        .map(|a| {
            let i = a.get_info();
            (i.name, format!("{:?}", i.backend))
        })
        .collect();
    Some(match_adapter_name(want, &adapters).map(|i| all.swap_remove(i)))
}
/// `WGPU_POWER_PREF` (`low` / `high` / `none`), default HighPerformance. wgpu ignores an
/// unknown value silently; warn so a typo is noticed.
#[cfg(not(target_arch = "wasm32"))]
fn power_pref_from_env() -> wgpu::PowerPreference {
    wgpu::PowerPreference::from_env().unwrap_or_else(|| {
        if let Ok(v) = std::env::var("WGPU_POWER_PREF") {
            log::warn!("gpu: ignoring WGPU_POWER_PREF={v:?} (expected low, high or none)");
        }
        wgpu::PowerPreference::HighPerformance
    })
}
#[cfg(target_arch = "wasm32")]
fn power_pref_from_env() -> wgpu::PowerPreference {
    wgpu::PowerPreference::HighPerformance
}
#[cfg(target_arch = "wasm32")]
async fn adapter_by_name(_: &wgpu::Instance) -> Option<Result<wgpu::Adapter, String>> {
    None
}

impl Transfers {
    fn count(&self, up: bool, bytes: usize) {
        use std::sync::atomic::Ordering::Relaxed;
        let (n, b) = if up { (&self.ups, &self.up_bytes) } else { (&self.downs, &self.down_bytes) };
        n.fetch_add(1, Relaxed);
        b.fetch_add(bytes as u64, Relaxed);
    }
}

fn tex_bytes(t: &wgpu::Texture) -> usize {
    t.width() as usize * t.height() as usize * 16
}

impl Pool {
    fn release(&mut self, t: wgpu::Texture) {
        match self.open.last() {
            Some(&newest) => self.released.push((t, newest)),
            None => self.make_free(t),
        }
    }

    fn budget(&self) -> usize {
        if self.budget == 0 { POOL_BUDGET } else { self.budget }
    }

    /// Keep `t` on the free list if it fits. Otherwise make room by evicting stale sizes, least
    /// recently freed first: sizes not asked for in the last [`STALE_FRAMES`] output frames (a
    /// blur bound or padded layer size that no longer occurs), so they age out instead of
    /// filling the budget for good. Sizes still in use are never evicted for a newcomer: a
    /// frame that frees more than the budget in a repeating order would otherwise evict, on
    /// every free, the texture the next call takes first (LRU on a cycle), and miss on every
    /// take. If nothing stale makes room, `t` is dropped (the kept set keeps hitting).
    ///
    /// Evicted textures count against the budget ([`Pool::dying`]) until the GPU work
    /// submitted so far is done. That throttles eviction to roughly one budget ahead of the
    /// GPU; it is not a hard bound (wgpu frees a dropped texture only when the device is
    /// maintained, which can be later than the work-done callback).
    fn make_free(&mut self, t: wgpu::Texture) {
        use std::sync::atomic::Ordering::Relaxed;
        let b = tex_bytes(&t);
        let budget = self.budget();
        if b > budget {
            return;
        }
        let mut evicted = 0;
        while self.bytes + b > budget {
            let frame = self.frame;
            let asked = &self.asked;
            let stale = |k: &(u32, u32)| asked.get(k).is_none_or(|f| frame.saturating_sub(*f) >= STALE_FRAMES);
            let Some(oldest) = self.free.iter().filter(|(k, _)| stale(k)).filter_map(|(k, q)| q.front().map(|(_, s)| (*s, *k))).min().map(|(_, k)| k) else {
                break;
            };
            let ob = oldest.0 as usize * oldest.1 as usize * 16;
            if self.dying.load(Relaxed) + evicted + ob > budget {
                break;
            }
            let Some(q) = self.free.get_mut(&oldest) else { break };
            if let Some((old, _)) = q.pop_front() {
                self.bytes -= tex_bytes(&old);
                evicted += ob;
                self.evicted += 1;
            }
            if q.is_empty() {
                self.free.remove(&oldest);
            }
        }
        if evicted > 0 {
            self.dying.fetch_add(evicted, Relaxed);
            let dying = self.dying.clone();
            match &self.queue {
                Some(q) => q.on_submitted_work_done(move || {
                    let _ = dying.try_update(Relaxed, Relaxed, |d| Some(d.saturating_sub(evicted)));
                }),
                None => {
                    let _ = dying.try_update(Relaxed, Relaxed, |d| Some(d.saturating_sub(evicted)));
                }
            }
        }
        if self.bytes + b > budget {
            return;
        }
        self.bytes += b;
        self.stamp += 1;
        self.free.entry((t.width(), t.height())).or_default().push_back((t, self.stamp));
    }

    /// An output frame is done: sizes not asked for since age towards [`STALE_FRAMES`].
    /// Returns the evicted bytes the caller should wait for: 0 when none still count, or when a
    /// previous wait cleared none and none has been freed since.
    fn frame_end(&mut self) -> usize {
        self.frame += 1;
        let (frame, free) = (self.frame, &self.free);
        self.asked.retain(|k, f| frame - *f < STALE_FRAMES || free.contains_key(k));
        let d = self.dying.load(std::sync::atomic::Ordering::Relaxed);
        if d == 0 {
            self.stalled = 0;
            return 0;
        }
        if self.stalled != 0 && d >= self.stalled {
            // No progress since the stalled wait: do not wait again.
            return 0;
        }
        self.stalled = 0;
        d
    }

    /// A [`GpuContext::pool_frame_end`] wait finished with `before` dying bytes still counted
    /// beforehand. A wait that freed nothing (timed out, or callbacks not run synchronously,
    /// e.g. on WebGPU where poll does not block) marks the pool stalled.
    fn waited(&mut self, before: usize) {
        self.waits += 1;
        let d = self.dying.load(std::sync::atomic::Ordering::Relaxed);
        if d >= before {
            self.stalled = d;
        }
    }

    fn open(&mut self) -> u64 {
        self.next += 1;
        self.open.insert(self.next);
        self.next
    }

    /// Encoder `id` was submitted or dropped: free what no open encoder can still read.
    fn close(&mut self, id: u64) {
        self.open.remove(&id);
        let oldest = self.open.first().copied().unwrap_or(u64::MAX);
        if self.released.iter().all(|(_, w)| *w >= oldest) {
            return;
        }
        let (ready, wait): (Vec<_>, Vec<_>) = std::mem::take(&mut self.released).into_iter().partition(|(_, w)| *w < oldest);
        self.released = wait;
        for (t, _) in ready {
            self.make_free(t);
        }
    }

    fn register(&mut self) -> u64 {
        self.next_owner += 1;
        if TEXTURE_REUSE.load(std::sync::atomic::Ordering::Relaxed) {
            self.owned.insert(self.next_owner, HashMap::new());
        }
        self.next_owner
    }

    /// `Enc` `owner` is gone: its kept textures go through the shared pool.
    fn unregister(&mut self, owner: u64) {
        if let Some(m) = self.owned.remove(&owner) {
            for t in m.into_values().flatten() {
                self.release(t);
            }
        }
    }

    fn release_owned(&mut self, t: wgpu::Texture, owner: u64, generation: u64) {
        if generation != self.generation {
            return;
        }
        if let Some(m) = self.owned.get_mut(&owner) {
            m.entry((t.width(), t.height())).or_default().push(t);
            return;
        }
        self.release(t);
    }

    /// An owner for [`Enc::lent`]. It is a fresh id each time, so a lease of an earlier lent
    /// `Enc` that drops later (an image that outlived its call) finds no owner and goes
    /// through [`Pool::release`], which waits for every open encoder.
    fn lend(&mut self) -> u64 {
        self.register()
    }

    /// [`Enc::lent`] is done with `owner` (its work submitted or discarded): its kept textures
    /// go to the shared free list at once. The free list has one [`POOL_BUDGET`] (only stale
    /// sizes are evicted), so finished calls no longer keep a budget each (16 frame threads
    /// kept up to 16 × `POOL_BUDGET`). The budget caps the free list only: textures waiting
    /// for open encoders ([`Pool::released`]) and those held by live `Enc`s are outside it.
    ///
    /// They skip [`Pool::release`]'s wait for open encoders: a kept texture's lease has
    /// dropped and only this `Enc` recorded work on it, which is already submitted or
    /// discarded, so later work cannot overtake it on the queue. That is enforced, not assumed:
    /// a lease read by another `Enc` is marked escaped
    /// ([`GpuImage::escape`]) and released through [`Pool::release`] whenever it drops, and a
    /// lease that drops after this call finds no owner and goes the same way.
    fn give_back(&mut self, owner: u64) {
        if let Some(m) = self.owned.remove(&owner) {
            for t in m.into_values().flatten() {
                self.make_free(t);
            }
        }
    }

    /// Moves `owner`'s returned working textures onto the shared free list (dropping any over
    /// budget) right after `owner`'s encoder has been submitted, while keeping `owner` registered.
    fn flush_owned(&mut self, owner: u64) {
        let Some(m) = self.owned.get_mut(&owner).map(std::mem::take) else { return };
        for t in m.into_values().flatten() {
            self.make_free(t);
        }
    }

    /// Bytes the pool will still keep alive (live `Lease`s, other owners' in-flight working sets,
    /// released textures still guarded by other open encoders, and up to `budget` on the shared
    /// free list) once `owner` flushes its returned textures and `closing_enc` submits.
    fn kept_after_flush(&self, owner: u64, closing_enc: Option<u64>) -> usize {
        let oldest = self.open.iter().copied().find(|&o| Some(o) != closing_enc).unwrap_or(u64::MAX);
        let mut ready = 0usize;
        let mut still_released = 0usize;
        for (t, w) in &self.released {
            let b = tex_bytes(t);
            if *w < oldest {
                ready += b;
            } else {
                still_released += b;
            }
        }
        let mut self_owned = 0usize;
        let mut other_owned = 0usize;
        for (&k, m) in &self.owned {
            let b: usize = m.values().flatten().map(tex_bytes).sum();
            if k == owner {
                self_owned += b;
            } else {
                other_owned += b;
            }
        }
        let free_after = self.bytes.max((self.bytes + ready + self_owned).min(self.budget()));
        self.leased + other_owned + still_released + free_after
    }

    /// Bytes of working textures kept for reuse: the free list, those waiting for open
    /// encoders and those kept by live `Enc`s.
    fn retained(&self) -> usize {
        let owned = self.owned.values().flat_map(|m| m.values().flatten());
        self.released.iter().map(|(t, _)| t).chain(owned).map(tex_bytes).fold(self.bytes, usize::saturating_add)
    }

    fn take_owned(&mut self, owner: u64, w: u32, h: u32) -> Option<wgpu::Texture> {
        let t = self.owned.get_mut(&owner)?.get_mut(&(w, h))?.pop()?;
        self.asked.insert((w, h), self.frame);
        self.hits += 1;
        Some(t)
    }

    fn take(&mut self, w: u32, h: u32) -> Option<wgpu::Texture> {
        self.asked.insert((w, h), self.frame);
        let q = self.free.get_mut(&(w, h));
        let t = q.and_then(|q| q.pop_back()).map(|(t, _)| t);
        if self.free.get(&(w, h)).is_some_and(|q| q.is_empty()) {
            self.free.remove(&(w, h));
        }
        match &t {
            Some(t) => {
                self.bytes -= tex_bytes(t);
                self.hits += 1;
            }
            None => self.misses += 1,
        }
        t
    }

    fn clear(&mut self) {
        self.free.clear();
        self.bytes = 0;
    }

    /// After a failed frame (#1319): drop every kept texture, the free ones and those waiting
    /// for an open encoder or kept by one, and the textures still out never return. The
    /// encoders' bookkeeping stays, so reuse remains ordered.
    fn discard(&mut self) {
        self.clear();
        self.released.clear();
        for kept in self.owned.values_mut() {
            kept.clear();
        }
        self.generation = self.generation.wrapping_add(1);
    }
}

/// Uniform parameter block shared by every kernel (see `shaders/common.wgsl`).
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Params {
    pub u: [[u32; 4]; 4],
    pub f: [[f32; 4]; 12],
}

impl Params {
    fn bytes(&self) -> Vec<u8> {
        let mut v = Vec::with_capacity(256);
        for r in &self.u {
            for x in r {
                v.extend_from_slice(&x.to_le_bytes());
            }
        }
        for r in &self.f {
            for x in r {
                v.extend_from_slice(&x.to_le_bytes());
            }
        }
        v
    }
}

struct Upload {
    buf: Weak<Buf>,
    img: GpuImage,
    bytes: usize,
    last_use: u64,
}

#[derive(Default)]
struct Uploads {
    map: HashMap<usize, Upload>,
    bytes: usize,
    clock: u64,
}

/// The device and everything compiled for it.
pub struct GpuContext {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub(crate) name: String,
    pub(crate) max_dim: u32,
    pub(crate) median_kernel: crate::fx_noise::MedianKernel,
    bgl: wgpu::BindGroupLayout,
    bgl_ext: wgpu::BindGroupLayout,
    /// Each of `FAMILIES`' modules, built with its first kernel; and their group 0 layout.
    modules: Vec<std::sync::OnceLock<Result<wgpu::ShaderModule, String>>>,
    layout: wgpu::PipelineLayout,
    /// The compositor's own kernels and Classic 3D (built up front).
    pipelines: HashMap<&'static str, wgpu::ComputePipeline>,
    /// Kernels built up front that failed to build, and why (Classic 3D renders on the CPU).
    unavailable: Vec<(&'static str, String)>,
    /// Effect kernels, built on first dispatch (`GpuContext::pipeline`); one that doesn't build
    /// leaves its effects to the CPU.
    effect_pipelines: EffectPipelines,
    display_bgl: wgpu::BindGroupLayout,
    display: wgpu::ComputePipeline,
    /// The display texture's next mip level from the one above (`display.wgsl` `mip`).
    display_mip: wgpu::ComputePipeline,
    dummy: wgpu::Texture,
    dummy_tex: wgpu::TextureView,
    dummy_buf: wgpu::Buffer,
    uploads: Mutex<Uploads>,
    /// Uploaded footage pictures, shared by the layers that show them (see [`crate::upload`]).
    pictures: Mutex<crate::upload::Pictures>,
    /// Reused staging buffers of large uploads (see [`crate::upload::Staging`]).
    upload_staging: crate::upload::Staging,
    upload_budget: std::sync::atomic::AtomicUsize,
    upload_full_warned: std::sync::atomic::AtomicBool,
    pool: Arc<Mutex<Pool>>,
    /// CPU ↔ GPU traffic (diagnostics: `bench --gpu`).
    pub(crate) transfers: Transfers,
    /// Readback staging buffers by size.
    staging: Mutex<HashMap<u64, Vec<wgpu::Buffer>>>,
    /// Transparent read-only images by size (see [`Enc::zeros`]).
    zeros: Mutex<HashMap<(u32, u32), GpuImage>>,
    /// Advanced 3D render pipelines (built on first use).
    pub(crate) adv3d: std::sync::OnceLock<crate::adv3d::Pipes>,
    /// The adapter can run the Advanced 3D rasteriser (`adv3d::raster_unsupported`); without
    /// it Advanced 3D scenes render on the CPU.
    pub(crate) adv3d_raster: bool,
    /// GPU particle pipeline (built on first use) and simulation checkpoints.
    pub(crate) particles: crate::particles::PipesCell,
    pub(crate) particle_states: crate::particles::StatesCell,
    /// Deferred readbacks (a browser worker's WebGPU device), see [`crate::deferred`].
    pub(crate) deferred: Option<Arc<crate::deferred::Deferred>>,
    /// The adapter is a software rasteriser (llvmpipe, WARP): its float maths follows the
    /// CPU's exactly, so the YUV tests hold it to exact parity.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) software: bool,
    /// Export frame → YUV 4:2:0 kernel (built on first use).
    pub(crate) yuv: std::sync::OnceLock<Result<crate::yuv::YuvPipe, String>>,
    readbacks: Arc<crate::readback::Readbacks>,
    /// Serializes `device.poll` across threads (`wgpu-core 30.0.1` can panic on concurrent
    /// `Device::maintain` when two callers poll the same device at once). Shared by every
    /// context on the same device.
    poll_lock: Arc<Mutex<()>>,
    memcap: crate::memcap::MemCap,
}

fn tex_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: false },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}

fn storage_tex_entry(binding: u32, format: wgpu::TextureFormat) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::StorageTexture { access: wgpu::StorageTextureAccess::WriteOnly, format, view_dimension: wgpu::TextureViewDimension::D2 },
        count: None,
    }
}

/// Contain synchronous native initialization errors without replacing the device owner's
/// handlers. Scopes are thread-local with wgpu's enabled `std` feature. The pinned native
/// wgpu-core backend returns an immediately-ready future from pop_error_scope, so these
/// drains do not poll or wait for unrelated submitted GPU work.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn init_resource<T>(device: &wgpu::Device, label: &str, create: impl FnOnce() -> T) -> Result<T, String> {
    let oom = device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
    let internal = device.push_error_scope(wgpu::ErrorFilter::Internal);
    let validation = device.push_error_scope(wgpu::ErrorFilter::Validation);
    // Some native drivers/wgpu paths can still unwind instead of reporting a scoped error.
    // Keep the last-resort guard inside the scopes, then drain every scope even on failure.
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(create));
    let validation_error = pollster::block_on(validation.pop());
    let internal_error = pollster::block_on(internal.pop());
    let oom_error = pollster::block_on(oom.pop());
    if let Some(error) = validation_error.or(internal_error).or(oom_error) {
        return Err(format!("GPU initialization ({label}): {error}"));
    }
    result.map_err(|payload| {
        let reason =
            payload.downcast_ref::<String>().map(String::as_str).or_else(|| payload.downcast_ref::<&str>().copied()).unwrap_or("native backend panicked");
        format!("GPU initialization ({label}): {reason}")
    })
}

/// Browser scope completion is asynchronous. Preserve the existing browser constructor;
/// never block its event loop or introduce a guard across an asynchronous boundary here.
#[cfg(target_arch = "wasm32")]
pub(crate) fn init_resource<T>(_device: &wgpu::Device, _label: &str, create: impl FnOnce() -> T) -> Result<T, String> {
    Ok(create())
}

/// A buffer holding `contents` (padded to 4 bytes), written through the queue. Unlike
/// `DeviceExt::create_buffer_init`, which maps the buffer at creation and panics when the
/// allocation failed (a driver reports it as an internal error, not out-of-memory: Adreno's
/// `ERROR_INITIALIZATION_FAILED`, #591), a failed allocation only yields an invalid buffer and
/// an error for the caller's error scope.
pub(crate) fn buffer_init(device: &wgpu::Device, queue: &wgpu::Queue, label: Option<&str>, contents: &[u8], usage: wgpu::BufferUsages) -> wgpu::Buffer {
    let align = wgpu::COPY_BUFFER_ALIGNMENT as usize;
    let padded = contents.len().div_ceil(align).saturating_mul(align);
    let buffer =
        device.create_buffer(&wgpu::BufferDescriptor { label, size: padded as u64, usage: usage | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
    if padded > 0 {
        if padded == contents.len() {
            queue.write_buffer(&buffer, 0, contents);
        } else {
            let mut data = contents.to_vec();
            data.resize(padded, 0);
            queue.write_buffer(&buffer, 0, &data);
        }
    }
    buffer
}

/// The descriptor for a device the compositor owns: `MemoryUsage` so freed textures give their
/// allocator blocks back instead of a few long-lived objects pinning large ones.
pub(crate) fn device_descriptor(required_limits: wgpu::Limits) -> wgpu::DeviceDescriptor<'static> {
    wgpu::DeviceDescriptor { label: Some("effectcraft gpu"), required_limits, memory_hints: wgpu::MemoryHints::MemoryUsage, ..Default::default() }
}

impl GpuContext {
    /// [`buffer_init`] on this context's device.
    pub(crate) fn buffer_init(&self, label: Option<&str>, contents: &[u8], usage: wgpu::BufferUsages) -> wgpu::Buffer {
        buffer_init(&self.device, &self.queue, label, contents, usage)
    }

    /// Build on an existing device (the desktop app shares egui-wgpu's). `Err` when the
    /// adapter cannot run the compositor (no compute shaders, e.g. WebGL2; no float storage
    /// textures) or the device was created with limits below what its pipelines bind.
    /// Preserves the host's device handlers. The host must forward failures using
    /// [`Self::device_failure_notifier`] for immediate readback cancellation.
    pub fn new(adapter: &wgpu::Adapter, device: wgpu::Device, queue: wgpu::Queue) -> Result<GpuContext, String> {
        let info = adapter.get_info();
        let median_kernel = crate::fx_noise::MedianKernel::for_adapter(info.backend, info.device_type);
        #[cfg(test)]
        let median_kernel = median_kernel.test_override()?;
        if !adapter.get_downlevel_capabilities().flags.contains(wgpu::DownlevelFlags::COMPUTE_SHADERS) {
            return Err(format!("{} ({:?}): no compute shaders", info.name, info.backend));
        }
        // OpenGL (Window Graphics ▸ OpenGL for drivers that crash otherwise, #243) translates the
        // kernels for tens of seconds before one fails validation: CPU compositing at once.
        if info.backend == wgpu::Backend::Gl {
            return Err(format!("{} (Gl): the GPU compositor needs DirectX 12, Vulkan, Metal or WebGPU", info.name));
        }
        // egui-wgpu asks for WebGL2's limits on GL (no storage buffers at all): building the
        // pipelines there would be a validation error, which release builds only log.
        let l = device.limits();
        let needs = [
            // `adv3d`'s post kernels bind 7 (its rasteriser's fragment stage 6).
            ("max_storage_buffers_per_shader_stage", l.max_storage_buffers_per_shader_stage, 7),
            ("max_storage_textures_per_shader_stage", l.max_storage_textures_per_shader_stage, 1),
            ("max_bind_groups", l.max_bind_groups, 2),
            ("max_compute_workgroup_size_x", l.max_compute_workgroup_size_x, 64),
            ("max_compute_workgroup_size_y", l.max_compute_workgroup_size_y, 16),
            ("max_compute_invocations_per_workgroup", l.max_compute_invocations_per_workgroup, 256),
            ("max_compute_workgroup_storage_size", l.max_compute_workgroup_storage_size, median_kernel.workgroup_storage()),
        ];
        if let Some((limit, have, need)) = needs.iter().find(|(_, have, need)| have < need) {
            return Err(format!("{} ({:?}): device limit {limit} is {have}, needs {need}", info.name, info.backend));
        }
        for f in [FORMAT, wgpu::TextureFormat::Rgba8Unorm] {
            if !adapter.get_texture_format_features(f).allowed_usages.contains(wgpu::TextureUsages::STORAGE_BINDING) {
                return Err(format!("{} ({:?}): {f:?} storage textures unsupported", info.name, info.backend));
            }
        }
        let readbacks = crate::readback::Readbacks::new()?;
        let name = format!("{} ({:?})", info.name, info.backend);
        let adv3d_unsupported = crate::adv3d::raster_unsupported(adapter);
        if let Some(why) = &adv3d_unsupported {
            log::info!("gpu {name}: Advanced 3D renders on the CPU ({why})");
        }
        let module = kernel_module(&device, "effectcraft kernels", &[])?;
        let bgl = init_resource(&device, "kernels bind-group layout", || {
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("effectcraft kernels"),
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::COMPUTE,
                        ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                        count: None,
                    },
                    tex_entry(1),
                    tex_entry(2),
                    storage_tex_entry(3, FORMAT),
                    wgpu::BindGroupLayoutEntry {
                        binding: 4,
                        visibility: wgpu::ShaderStages::COMPUTE,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Storage { read_only: true },
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                ],
            })
        })?;
        let layout = init_resource(&device, "kernels pipeline layout", || {
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("effectcraft kernels"),
                bind_group_layouts: &[Some(&bgl)],
                immediate_size: 0,
            })
        })?;
        let storage = |binding: u32| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only: true }, has_dynamic_offset: false, min_binding_size: None },
            count: None,
        };
        let bgl_ext = init_resource(&device, "extended kernels bind-group layout", || {
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("effectcraft kernels (ext)"),
                entries: &[storage(0), storage(1), storage(2), storage(3)],
            })
        })?;
        let layout_ext = init_resource(&device, "extended kernels pipeline layout", || {
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("effectcraft kernels (ext)"),
                bind_group_layouts: &[Some(&bgl), Some(&bgl_ext)],
                immediate_size: 0,
            })
        })?;
        // The compositor's own kernels are required and built now. A Classic 3D kernel that
        // doesn't build is left out (its 3D runs render on the CPU), and effect kernels are built
        // on first use (`GpuContext::pipeline`); one that doesn't build leaves its effects to the
        // CPU (see `Enc::take_missing`).
        let sky = kernel_module(&device, "effectcraft sky", &[include_str!("shaders/sky.wgsl")])?;
        let mut pipelines = ENTRIES
            .iter()
            .map(|e| (e, &module))
            .chain(crate::adv3d::SKY_KERNELS.iter().map(|e| (e, &sky)))
            .map(|(e, module)| Ok((*e, build_pipeline(&device, module, &layout, e)?)))
            .collect::<Result<HashMap<_, _>, String>>()?;
        let mut unavailable = vec![];
        let classic3d = kernel_module(&device, "effectcraft classic3d", &[include_str!("shaders/classic3d.wgsl")]);
        for e in EXT_ENTRIES {
            match classic3d.as_ref().map_err(Clone::clone).and_then(|m| build_pipeline(&device, m, &layout_ext, e)) {
                Ok(p) => {
                    pipelines.insert(*e, p);
                }
                Err(why) => {
                    log::warn!("gpu {name}: kernel {e} unavailable, its 3D layers render on the CPU: {why}");
                    unavailable.push((*e, why));
                }
            }
        }
        #[cfg(test)]
        {
            // The numerical probe (tests) calls `fx_stylize.wgsl` helpers.
            let probe = kernel_module(&device, "effectcraft math probe", &[include_str!("shaders/fx_stylize.wgsl"), crate::tests::MATH_PROBE_SHADER])?;
            pipelines.insert("math_probe", build_pipeline(&device, &probe, &layout, "math_probe")?);
        }
        let effect_pipelines =
            FAMILIES.iter().enumerate().flat_map(|(family, (_, _, kernels))| kernels.iter().map(move |e| (*e, (family, std::sync::OnceLock::new())))).collect();
        let modules = std::iter::once(std::sync::OnceLock::from(Ok(module))).chain(FAMILIES.iter().skip(1).map(|_| std::sync::OnceLock::new())).collect();
        let dmodule = init_resource(&device, "display shader module", || {
            device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("effectcraft display"),
                source: wgpu::ShaderSource::Wgsl(include_str!("shaders/display.wgsl").into()),
            })
        })?;
        let display_bgl = init_resource(&device, "display bind-group layout", || {
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("effectcraft display"),
                entries: &[tex_entry(1), storage_tex_entry(3, wgpu::TextureFormat::Rgba8Unorm)],
            })
        })?;
        let dlayout = init_resource(&device, "display pipeline layout", || {
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("effectcraft display"),
                bind_group_layouts: &[Some(&display_bgl)],
                immediate_size: 0,
            })
        })?;
        let display_pipeline = |entry: &'static str| {
            init_resource(&device, "display compute pipeline", || {
                device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                    label: Some(entry),
                    layout: Some(&dlayout),
                    module: &dmodule,
                    entry_point: Some(entry),
                    compilation_options: Default::default(),
                    cache: None,
                })
            })
        };
        let display = display_pipeline("display")?;
        let display_mip = display_pipeline("mip")?;
        let dummy = init_resource(&device, "dummy texture", || {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some("dummy"),
                size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: FORMAT,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            })
        })?;
        let dummy_buf =
            init_resource(&device, "dummy storage buffer", || buffer_init(&device, &queue, Some("dummy"), &[0u8; 16], wgpu::BufferUsages::STORAGE))?;
        let dummy_tex = init_resource(&device, "dummy texture view", || dummy.create_view(&Default::default()))?;
        let max_dim = device.limits().max_texture_dimension_2d;
        let memcap = crate::memcap::MemCap::new(&info);
        let pool_budget = if memcap.cap() > 0 { POOL_BUDGET.min((memcap.cap() / 4) as usize) } else { 0 };
        let pool = Arc::new(Mutex::new(Pool { queue: Some(queue.clone()), budget: pool_budget, ..Default::default() }));
        Ok(GpuContext {
            device,
            queue,
            name,
            max_dim,
            median_kernel,
            bgl,
            bgl_ext,
            modules,
            layout,
            pipelines,
            unavailable,
            effect_pipelines,
            display_bgl,
            display,
            display_mip,
            dummy_tex,
            dummy,
            dummy_buf,
            uploads: Mutex::new(Uploads::default()),
            pictures: Default::default(),
            upload_staging: Default::default(),
            upload_budget: std::sync::atomic::AtomicUsize::new(UPLOAD_BUDGET),
            upload_full_warned: Default::default(),
            pool,
            staging: Default::default(),
            transfers: Default::default(),
            zeros: Default::default(),
            adv3d: std::sync::OnceLock::new(),
            yuv: std::sync::OnceLock::new(),
            software: info.device_type == wgpu::DeviceType::Cpu,
            adv3d_raster: adv3d_unsupported.is_none(),
            particles: Default::default(),
            particle_states: Default::default(),
            deferred: None,
            readbacks,
            poll_lock: Default::default(),
            memcap,
        })
    }

    /// Whether this context has observed a readback timeout or device failure.
    /// A successful check is not a guarantee against faults after it returns.
    /// Shared-device hosts must forward their device failures through the notifier.
    pub fn check_health(&self) -> Result<(), String> {
        self.readbacks.check()
    }

    /// Export frame → YUV 4:2:0 kernel, initialized once and `None` when shader or pipeline creation fails.
    pub(crate) fn yuv_pipe(&self) -> Option<&crate::yuv::YuvPipe> {
        self.yuv
            .get_or_init(|| {
                crate::yuv::YuvPipe::new(&self.device).inspect_err(|e| {
                    log::warn!(target: "effectcraft_gpu", "yuv420 kernel unavailable: {e}");
                })
            })
            .as_ref()
            .ok()
    }

    /// A notification hook for the owner of a shared device's error/loss handlers.
    /// Invoke it with diagnostic context when the host observes a device failure.
    /// It retires this context and settles pending readbacks without replacing any
    /// host handler or keeping this context/device alive. It is harmless after drop.
    ///
    /// Shared-device construction does not automatically subscribe to device faults:
    /// without host forwarding, readbacks rely on poll errors and their deadlines.
    pub fn device_failure_notifier(&self) -> impl Fn(String) + Send + Sync + 'static {
        crate::readback::failure_notifier(&self.readbacks)
    }

    /// Maximum time a blocking poll holds `poll_lock` in one slice so concurrent readbacks are not
    /// starved behind a long `mem_gate` or readback wait.
    const POLL_SLICE: std::time::Duration = std::time::Duration::from_millis(20);

    /// All device polling routes through this failure bridge.
    pub(crate) fn poll_readbacks(&self, kind: wgpu::PollType) -> Result<wgpu::PollStatus, String> {
        self.readbacks.check()?;
        let res = {
            let _guard = match kind {
                wgpu::PollType::Poll => match self.poll_lock.try_lock() {
                    Ok(g) => g,
                    Err(std::sync::TryLockError::Poisoned(e)) => e.into_inner(),
                    Err(std::sync::TryLockError::WouldBlock) => return Ok(wgpu::PollStatus::Poll),
                },
                _ => self.poll_lock.lock().unwrap_or_else(|e| e.into_inner()),
            };
            self.device.poll(kind)
        };
        res.map_err(|e| {
            let error = format!("GPU readback poll: {e}");
            self.readbacks.retire(error.clone());
            log::error!("{error}");
            error
        })
    }

    /// Waits on `submission_index` up to `timeout` in [`Self::POLL_SLICE`] increments so wgpu
    /// frees resources dropped before that submission without holding `poll_lock` for seconds at a
    /// time. A timeout simply moves on without retiring the context or its readbacks; any other
    /// poll error retires the readbacks through the failure path.
    fn poll_wait(&self, submission_index: wgpu::SubmissionIndex, timeout: std::time::Duration) {
        if self.readbacks.check().is_err() {
            return;
        }
        let deadline = web_time::Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(web_time::Instant::now());
            let slice = remaining.min(Self::POLL_SLICE);
            let res = {
                let _guard = self.poll_lock.lock().unwrap_or_else(|e| e.into_inner());
                self.device.poll(wgpu::PollType::Wait { submission_index: Some(submission_index.clone()), timeout: Some(slice) })
            };
            let done = match res {
                Ok(_) => true,
                Err(wgpu::PollError::Timeout) => false,
                Err(e) => {
                    let error = format!("GPU poll wait: {e}");
                    self.readbacks.retire(error.clone());
                    log::error!("{error}");
                    true
                }
            };
            if done || remaining <= slice {
                break;
            }
        }
    }

    /// The memory budget (the memory cap: a conservative share of the device, see `memcap`),
    /// the allocator's peak, one frame's share of it and what the upload cache may still add.
    /// `None` where any is unknown: no allocator report (Metal, WebGPU, GL) or the cap is off.
    pub fn frame_memory(&self) -> Option<effectcraft_render::FrameMemory> {
        let budget = self.memcap.cap();
        let now = self.allocated_bytes();
        self.memcap.note(now);
        // The allocator's peak (sampled while frames render) holds a frame's working textures
        // at their most; what is gone again after the frame (and at least what the pool keeps
        // for the next) is one frame's share. Frames that had to wait for memory: no room.
        let peak = self.memcap.peak();
        let frame = if self.memcap.pressed() { budget } else { peak.saturating_sub(now).max(self.pool_retained_bytes() as u64) };
        // The upload cache may still fill up to its budget.
        let uploads = self.uploads.lock().map(|u| u.bytes).unwrap_or(0);
        let reserve = self.upload_budget().saturating_sub(uploads) as u64;
        (budget > 0 && peak > 0 && frame > 0).then_some(effectcraft_render::FrameMemory { budget, peak, frame, reserve })
    }

    /// Bytes held by wgpu's device allocator (`0` on backends that do not report).
    pub(crate) fn allocated_bytes(&self) -> u64 {
        self.device.generate_allocator_report().map_or(0, |r| r.total_allocated_bytes)
    }

    /// Bytes held in long-lived or still-referenced GPU caches/leases that submitting and waiting
    /// on `owner` (`closing_enc`) cannot free.
    fn kept_bytes(&self, owner: u64, closing_enc: Option<u64>) -> u64 {
        // (uploaded layer and footage textures are pooled: the pool's leases count them)
        let pool = self.pool.lock().map(|p| p.kept_after_flush(owner, closing_enc) as u64).unwrap_or(0);
        let staging: u64 = self.staging.lock().map(|s| s.iter().map(|(k, v)| k * v.len() as u64).sum()).unwrap_or(0);
        let zeros: u64 = self.zeros.lock().map(|z| z.keys().map(|&(w, h)| w as u64 * h as u64 * 16).sum()).unwrap_or(0);
        pool + staging + self.upload_staging.bytes() + zeros
    }

    #[cfg(test)]
    pub(crate) fn memcap_waits(&self) -> u64 {
        self.memcap.waits()
    }

    #[cfg(test)]
    pub(crate) fn memcap_warned(&self) -> bool {
        self.memcap.warned()
    }

    #[cfg(test)]
    pub(crate) fn set_memcap_wait_timeout(&self, timeout: std::time::Duration) {
        self.memcap.set_wait_timeout(timeout);
    }

    #[cfg(test)]
    pub(crate) fn lock_poll_for_test(&self) -> std::sync::MutexGuard<'_, ()> {
        self.poll_lock.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Start a bounded buffer mapping. Retired contexts reject new requests immediately.
    pub(crate) fn map_readback(
        &self,
        buffer: &wgpu::Buffer,
        row: usize,
        stride: usize,
        height: usize,
        done: impl FnOnce(Result<Vec<u8>, String>) + wgpu::WasmNotSend + 'static,
    ) {
        let ticket = match self.readbacks.start(done) {
            Ok(t) => t,
            Err(e) => {
                log::error!("gpu: {e}");
                return;
            }
        };
        if !ticket.active() {
            return;
        }
        let b = buffer.clone();
        buffer.map_async(wgpu::MapMode::Read, .., move |r| {
            if !ticket.active() {
                b.unmap();
                return;
            }
            let result = r.map_err(|e| format!("GPU buffer mapping: {e}")).and_then(|_| {
                let view = b.get_mapped_range(..).map_err(|e| format!("GPU mapped range: {e}"))?;
                crate::readback::packed(&view, row, stride, height)
            });
            b.unmap();
            ticket.complete(result);
        });
    }

    /// A bounded native readback: polling and receiving share one deadline, started once
    /// `poll_lock` is first acquired and polled in [`Self::POLL_SLICE`] slices so lock contention
    /// never consumes the readback deadline or holds the mutex across the whole timeout.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn read_buffer(&self, buffer: &wgpu::Buffer, row: usize, stride: usize, height: usize) -> Result<Vec<u8>, String> {
        self.read_buffer_after(buffer, None, row, stride, height)
    }

    /// [`Self::read_buffer`] waiting only for `submission` (the one that wrote `buffer`), not
    /// for every submission so far: another frame's GPU work submitted meanwhile doesn't hold
    /// this readback up (#3335). `None`: wait for all submitted work.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn read_buffer_after(
        &self,
        buffer: &wgpu::Buffer,
        submission: Option<wgpu::SubmissionIndex>,
        row: usize,
        stride: usize,
        height: usize,
    ) -> Result<Vec<u8>, String> {
        self.read_mapped(buffer, submission, |view| crate::readback::packed(view, row, stride, height))
    }

    /// Map `buffer`, wait for the mapping (only for `submission` when given) and hand the
    /// mapped bytes to `read`, on this thread, before unmapping: `read` copies the rows
    /// straight into their destination, once (#3335). The copy happens outside `poll_lock`, so
    /// it doesn't hold up other threads' readbacks the way copying inside the map callback
    /// (which runs during a device poll) would.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn read_mapped<R>(
        &self,
        buffer: &wgpu::Buffer,
        submission: Option<wgpu::SubmissionIndex>,
        read: impl FnOnce(&[u8]) -> Result<R, String>,
    ) -> Result<R, String> {
        let (tx, rx) = std::sync::mpsc::channel();
        let ticket = self.readbacks.start(move |r| {
            let _ = tx.send(r);
        })?;
        if !ticket.active() {
            return Err("GPU readback cancelled".into());
        }
        let b = buffer.clone();
        buffer.map_async(wgpu::MapMode::Read, .., move |r| {
            // Timed out or cancelled: nobody reads the mapping.
            if !ticket.active() {
                b.unmap();
                return;
            }
            // Mapped: the waiting thread reads the rows and unmaps (an empty vector = mapped).
            ticket.complete(r.map(|_| Vec::new()).map_err(|e| format!("GPU buffer mapping: {e}")));
        });
        // On any failure, unmap (harmless when not mapped; cancels a pending map) so a mapped
        // buffer can never be handed out again.
        let fail = |e: String| {
            buffer.unmap();
            e
        };
        self.wait_mapped(&rx, submission).map_err(fail)?.map_err(fail)?;
        let result = {
            let view = buffer.get_mapped_range(..).map_err(|e| format!("GPU mapped range: {e}"));
            view.and_then(|view| read(&view))
        };
        buffer.unmap();
        result
    }

    /// Poll until `rx` reports the mapping (see [`Self::read_mapped`]): polling and receiving
    /// share one deadline, started once `poll_lock` is first acquired and polled in
    /// [`Self::POLL_SLICE`] slices so lock contention never consumes the readback deadline or
    /// holds the mutex across the whole timeout. While another thread holds `poll_lock` it is
    /// polling the device, which delivers this mapping too: wait for it instead of queueing.
    #[cfg(not(target_arch = "wasm32"))]
    fn wait_mapped(
        &self,
        rx: &std::sync::mpsc::Receiver<Result<Vec<u8>, String>>,
        submission: Option<wgpu::SubmissionIndex>,
    ) -> Result<Result<Vec<u8>, String>, String> {
        let mut deadline = None;
        loop {
            self.readbacks.check()?;
            if let Ok(result) = rx.try_recv() {
                return Ok(result);
            }
            let (remaining, slice, poll_res) = {
                let _guard = match self.poll_lock.try_lock() {
                    Ok(g) => g,
                    Err(std::sync::TryLockError::Poisoned(e)) => e.into_inner(),
                    Err(std::sync::TryLockError::WouldBlock) => {
                        match rx.recv_timeout(std::time::Duration::from_millis(1)) {
                            Ok(result) => return Ok(result),
                            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                            Err(e) => return Err(format!("GPU readback completion channel: {e}")),
                        }
                        // The ticket's own deadline still bounds a wait that never gets the lock.
                        continue;
                    }
                };
                self.readbacks.check()?;
                let d = *deadline.get_or_insert_with(|| std::time::Instant::now() + crate::readback::TIMEOUT);
                let remaining = d.saturating_duration_since(std::time::Instant::now());
                if remaining.is_zero() {
                    (remaining, std::time::Duration::ZERO, Ok(wgpu::PollStatus::Poll))
                } else {
                    let slice = remaining.min(Self::POLL_SLICE);
                    let res = self.device.poll(wgpu::PollType::Wait { submission_index: submission.clone(), timeout: Some(slice) });
                    (remaining, slice, res)
                }
            };
            if let Ok(result) = rx.try_recv() {
                return Ok(result);
            }
            if remaining.is_zero() {
                let error = "GPU readback receive timed out; context retired".to_string();
                self.readbacks.retire(error.clone());
                return Err(error);
            }
            match poll_res {
                Ok(_) => {}
                Err(wgpu::PollError::Timeout) if remaining > slice => {}
                Err(e) => {
                    let error = format!("GPU readback poll: {e}");
                    self.readbacks.retire(error.clone());
                    log::error!("{error}");
                    return Err(error);
                }
            }
            // The callback may be running on another thread that polled the same device.
            match rx.recv_timeout(remaining.min(std::time::Duration::from_millis(1))) {
                Ok(result) => return Ok(result),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                Err(e) => return Err(format!("GPU readback completion channel: {e}")),
            }
        }
    }

    #[cfg(target_arch = "wasm32")]
    pub(crate) fn read_buffer(&self, _buffer: &wgpu::Buffer, _row: usize, _stride: usize, _height: usize) -> Result<Vec<u8>, String> {
        Err("synchronous GPU readback is unavailable in the browser".into())
    }

    #[cfg(target_arch = "wasm32")]
    pub(crate) fn read_buffer_after(
        &self,
        _buffer: &wgpu::Buffer,
        _submission: Option<wgpu::SubmissionIndex>,
        _row: usize,
        _stride: usize,
        _height: usize,
    ) -> Result<Vec<u8>, String> {
        Err("synchronous GPU readback is unavailable in the browser".into())
    }

    #[cfg(target_arch = "wasm32")]
    pub(crate) fn read_mapped<R>(
        &self,
        _buffer: &wgpu::Buffer,
        _submission: Option<wgpu::SubmissionIndex>,
        _read: impl FnOnce(&[u8]) -> Result<R, String>,
    ) -> Result<R, String> {
        Err("synchronous GPU readback is unavailable in the browser".into())
    }

    /// Switch to deferred readbacks (see [`crate::deferred`]): readbacks are never waited for.
    pub fn set_deferred(&mut self, on: bool) {
        self.readbacks.cancel_pending("GPU readback mode changed".into());
        if let Some(d) = self.deferred.take() {
            d.cancel();
        }
        self.deferred = on.then(Default::default);
    }

    pub fn deferred(&self) -> Option<&Arc<crate::deferred::Deferred>> {
        self.deferred.as_ref()
    }

    /// A device of its own on the best adapter, without blocking (a browser worker: WebGPU
    /// only; natively any backend). `Err` says why there is none.
    pub async fn request() -> Result<GpuContext, String> {
        #[allow(unused_mut)]
        let mut desc = wgpu::InstanceDescriptor::new_without_display_handle();
        #[cfg(not(target_arch = "wasm32"))]
        {
            // Honour the same wgpu env vars as the viewer (WGPU_BACKEND, WGPU_DX12_COMPILER,
            // WGPU_VALIDATION, WGPU_DEBUG, WGPU_POWER_PREF), plus WGPU_ADAPTER_NAME (CLI/headless
            // only for now; the viewer ignores it). Not full parity: the viewer also changes
            // instance flags and has a GL fallback.
            // Note: with_env() also resets memory_budget_thresholds to their default (unused here).
            desc = desc.with_env();
        }
        #[cfg(target_arch = "wasm32")]
        {
            desc.backends = wgpu::Backends::BROWSER_WEBGPU;
        }
        let instance = wgpu::Instance::new(desc);
        let (adapter, by_name) = match adapter_by_name(&instance).await {
            Some(a) => (a?, true),
            None => (
                instance
                    .request_adapter(&wgpu::RequestAdapterOptions { power_preference: power_pref_from_env(), ..Default::default() })
                    .await
                    .map_err(|e| format!("no adapter: {e}"))?,
                false,
            ),
        };
        let limits = adapter.limits();
        let required_limits = wgpu::Limits {
            max_texture_dimension_2d: limits.max_texture_dimension_2d.min(16384),
            max_buffer_size: limits.max_buffer_size,
            ..wgpu::Limits::default()
        };
        let (device, queue) = adapter.request_device(&device_descriptor(required_limits)).await.map_err(|e| match by_name {
            true => format!("no device on {} (picked by WGPU_ADAPTER_NAME): {e}", adapter.get_info().name),
            false => format!("no device: {e}"),
        })?;
        let context = GpuContext::new(&adapter, device, queue)?;
        // This path owns the device; install handlers only after construction succeeds.
        crate::readback::install(&context.device, &context.readbacks);
        Ok(context)
    }

    /// A device of its own on the best adapter (CLI, tests, benchmarks). `None` when no
    /// adapter qualifies.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn headless() -> Option<GpuContext> {
        pollster::block_on(GpuContext::request()).map_err(|e| log::info!("gpu: {e}")).ok()
    }

    /// Readbacks (GPU → CPU) are possible: waited for natively, or deferred (a browser
    /// worker, [`crate::deferred`]). Impossible on the browser's main thread.
    /// Kernels the device couldn't build so far, with why: their effects render on the CPU.
    /// Effect kernels are built on first use, so one not used yet isn't listed.
    pub fn unavailable_kernels(&self) -> Vec<(&'static str, String)> {
        let mut out: Vec<_> = self
            .unavailable
            .iter()
            .cloned()
            .chain(self.effect_pipelines.iter().filter_map(|(e, (_, cell))| match cell.get() {
                Some(Err(why)) => Some((*e, why.clone())),
                _ => None,
            }))
            .collect();
        out.sort_unstable_by_key(|(e, _)| *e);
        out
    }

    /// Whether kernel `entry` is available (an effect or Classic 3D kernel can be missing,
    /// #613); an effect kernel not built yet is built now.
    pub(crate) fn has_kernel(&self, entry: &str) -> bool {
        self.pipeline(entry).is_some()
    }

    /// Kernel `entry`'s pipeline: one built up front, or an effect kernel, built the first time
    /// it is asked for. `None` when the device doesn't have it.
    fn pipeline(&self, entry: &str) -> Option<&wgpu::ComputePipeline> {
        if let Some(p) = self.pipelines.get(entry) {
            return Some(p);
        }
        let Some((e, (family, cell))) = self.effect_pipelines.get_key_value(entry) else {
            if !self.unavailable.iter().any(|(e, _)| *e == entry) {
                log::error!("gpu: no kernel {entry}");
            }
            return None;
        };
        let built = cell.get_or_init(|| {
            let built = self.family_module(*family).and_then(|module| build_pipeline(&self.device, module, &self.layout, e));
            if let Err(why) = &built {
                log::warn!("gpu {}: kernel {e} unavailable, its effects render on the CPU: {why}", self.name);
            }
            built
        });
        built.as_ref().ok()
    }

    /// Family `family`'s module (`FAMILIES`), built the first time it is asked for.
    fn family_module(&self, family: usize) -> Result<&wgpu::ShaderModule, String> {
        let (Some((label, files, _)), Some(cell)) = (FAMILIES.get(family), self.modules.get(family)) else {
            return Err(format!("no kernel family {family}"));
        };
        cell.get_or_init(|| {
            let median = [self.median_kernel.source()];
            let extra: &[&str] = if *label == MEDIAN_FAMILY { &median } else { &[] };
            let files: Vec<&str> = files.iter().chain(extra).copied().collect();
            kernel_module(&self.device, label, &files)
        })
        .as_ref()
        .map_err(Clone::clone)
    }

    /// Build every effect kernel not built yet (tests; and a host that would rather pay the
    /// compile time up front than on first use).
    pub fn build_all_kernels(&self) {
        for e in self.effect_pipelines.keys() {
            let _ = self.pipeline(e);
        }
    }

    /// How many effect kernels have been built (tests).
    #[cfg(test)]
    pub(crate) fn effect_pipelines_built(&self) -> usize {
        self.effect_pipelines.values().filter(|(_, cell)| matches!(cell.get(), Some(Ok(_)))).count()
    }

    /// Leave kernel `entry` out of `pipelines`/`effect_pipelines`, as if it had failed to build.
    #[cfg(test)]
    fn drop_from(
        pipelines: &mut HashMap<&'static str, wgpu::ComputePipeline>,
        unavailable: &mut Vec<(&'static str, String)>,
        effects: &mut EffectPipelines,
        entry: &'static str,
    ) {
        let why = "dropped by a test".to_string();
        pipelines.remove(entry);
        match effects.get(entry).map(|(f, _)| *f) {
            Some(family) => {
                effects.insert(entry, (family, std::sync::OnceLock::from(Err(why))));
            }
            None => unavailable.push((entry, why)),
        }
    }

    /// Leave kernel `entry` out, as if it had failed to build (tests of the CPU fallback).
    #[cfg(test)]
    pub(crate) fn drop_kernel(&mut self, entry: &'static str) {
        Self::drop_from(&mut self.pipelines, &mut self.unavailable, &mut self.effect_pipelines, entry);
    }

    /// A copy of this context sharing the same [`wgpu::Device`] and [`wgpu::Queue`] with
    /// kernel `entry` left out, as if it had failed to build (tests of the CPU fallback
    /// without opening a second device on `llvmpipe`).
    #[cfg(test)]
    pub(crate) fn without_kernel(&self, entry: &'static str) -> GpuContext {
        let mut pipelines = self.pipelines.clone();
        let mut unavailable = self.unavailable.clone();
        let mut effect_pipelines = self.effect_pipelines.clone();
        Self::drop_from(&mut pipelines, &mut unavailable, &mut effect_pipelines, entry);
        GpuContext {
            device: self.device.clone(),
            queue: self.queue.clone(),
            name: self.name.clone(),
            max_dim: self.max_dim,
            median_kernel: self.median_kernel,
            bgl: self.bgl.clone(),
            bgl_ext: self.bgl_ext.clone(),
            modules: self.modules.clone(),
            layout: self.layout.clone(),
            pipelines,
            unavailable,
            effect_pipelines,
            display_bgl: self.display_bgl.clone(),
            display: self.display.clone(),
            display_mip: self.display_mip.clone(),
            dummy_tex: self.dummy_tex.clone(),
            dummy: self.dummy.clone(),
            dummy_buf: self.dummy_buf.clone(),
            uploads: Mutex::new(Uploads::default()),
            pictures: Default::default(),
            upload_staging: Default::default(),
            upload_budget: std::sync::atomic::AtomicUsize::new(UPLOAD_BUDGET),
            upload_full_warned: Default::default(),
            pool: self.pool.clone(),
            staging: Default::default(),
            transfers: Default::default(),
            zeros: Default::default(),
            adv3d: std::sync::OnceLock::new(),
            adv3d_raster: self.adv3d_raster,
            particles: Default::default(),
            particle_states: Default::default(),
            deferred: self.deferred.clone(),
            readbacks: self.readbacks.clone(),
            software: self.software,
            yuv: std::sync::OnceLock::new(),
            poll_lock: self.poll_lock.clone(),
            memcap: self.memcap.fresh(),
        }
    }

    /// Bytes of readback staging buffers kept for reuse (tests).
    #[cfg(test)]
    pub(crate) fn staging_bytes(&self) -> u64 {
        self.staging.lock().map(|s| s.iter().map(|(k, v)| k * v.len() as u64).sum()).unwrap_or(0)
    }

    pub fn can_readback(&self) -> bool {
        self.readbacks.check().is_ok() && (self.can_wait() || self.deferred.is_some())
    }

    /// Readbacks can be waited for (natively, unless deferred). Steps that read back without a
    /// key ([`crate::deferred`]) need this.
    pub fn can_wait(&self) -> bool {
        self.readbacks.check().is_ok() && cfg!(not(target_arch = "wasm32")) && self.deferred.is_none()
    }

    pub(crate) fn fits(&self, w: u32, h: u32) -> bool {
        w > 0 && h > 0 && w <= self.max_dim && h <= self.max_dim
    }

    /// A new (zeroed) texture of its own (not pooled).
    pub(crate) fn image(&self, w: u32, h: u32) -> GpuImage {
        GpuImage::new(self.texture(w, h), w, h)
    }

    fn texture(&self, w: u32, h: u32) -> wgpu::Texture {
        // Named with its size, so a GPU error about it says which texture failed (#1319).
        self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some(&format!("working image {w}×{h}")),
            size: wgpu::Extent3d { width: w.max(1), height: h.max(1), depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::COPY_SRC | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        })
    }

    /// A pooled working texture: (image, reused). A reused texture holds old pixels; a new
    /// one is zeroed.
    fn pooled(&self, w: u32, h: u32, owner: u64) -> (GpuImage, bool) {
        let (w, h) = (w.max(1), h.max(1));
        let bytes = w as usize * h as usize * 16;
        let (reused, generation) = self
            .pool
            .lock()
            .map(|mut p| {
                let t = p.take_owned(owner, w, h).or_else(|| p.take(w, h));
                p.leased += bytes;
                (t, p.generation)
            })
            .unwrap_or((None, u64::MAX));
        let was_reused = reused.is_some();
        let texture = reused.unwrap_or_else(|| self.texture(w, h));
        let lease = Arc::new(Lease { texture: texture.clone(), pool: Arc::downgrade(&self.pool), owner, escaped: Default::default(), generation });
        (GpuImage { texture, width: w, height: h, _lease: Some(lease), levels: None }, was_reused)
    }

    /// Bytes of uploaded layer textures the context keeps (default 1536 MB). An application that
    /// knows the device's memory can raise it (static layers of a big frame upload once) or lower
    /// it (a small GPU); the least recently used textures go first.
    pub fn set_upload_budget(&self, bytes: usize) {
        self.upload_budget.store(bytes.max(1 << 20), std::sync::atomic::Ordering::Relaxed);
    }

    pub fn upload_budget(&self) -> usize {
        self.upload_budget.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Uploads and readbacks so far (images; uploads of cached layers count once).
    pub fn transfer_stats(&self) -> TransferStats {
        use std::sync::atomic::Ordering::Relaxed;
        let t = &self.transfers;
        TransferStats {
            uploads: t.ups.load(Relaxed),
            upload_bytes: t.up_bytes.load(Relaxed),
            readbacks: t.downs.load(Relaxed),
            readback_bytes: t.down_bytes.load(Relaxed),
        }
    }

    /// Bytes of working textures the pool keeps for reuse (see [`GpuContext::trim_pool`]).
    pub fn pool_retained_bytes(&self) -> usize {
        self.pool.lock().map(|p| p.retained()).unwrap_or_default()
    }

    /// Drop the working textures kept for reuse (a render is done). Textures still waiting for
    /// an open encoder and those of live `Enc`s are unaffected.
    pub fn trim_pool(&self) {
        if let Ok(mut p) = self.pool.lock() {
            p.clear();
        }
    }

    /// An output frame is done: the pool ages the sizes not asked for since (only sizes unused
    /// for [`STALE_FRAMES`] frames are evicted to make room for others). When evicted textures
    /// still count ([`Pool::dying`]), waits once so their work-done callbacks can clear that
    /// count; if the wait frees none of them, later frames only poll until `dying` drops (a
    /// stopped queue must not re-wait [`crate::readback::TIMEOUT`] on every frame).
    pub fn pool_frame_end(&self) {
        let before = self.pool.lock().map(|mut p| p.frame_end()).unwrap_or_default();
        if before > 0 {
            // A work-done callback waits for a submission; an empty one closes the wait.
            let index = self.queue.submit([]);
            self.poll_wait(index, crate::readback::TIMEOUT);
            if let Ok(mut p) = self.pool.lock() {
                p.waited(before);
            }
        } else {
            let _ = self.poll_readbacks(wgpu::PollType::Poll);
        }
    }

    /// Bytes the pool's free list may keep (tests lower it on a context of their own).
    #[cfg(test)]
    pub(crate) fn set_pool_budget(&self, bytes: usize) {
        if let Ok(mut p) = self.pool.lock() {
            p.budget = bytes;
        }
    }

    /// Free textures evicted to make room for newer ones.
    #[cfg(test)]
    pub(crate) fn pool_evictions(&self) -> u64 {
        self.pool.lock().map(|p| p.evicted).unwrap_or_default()
    }

    /// Empty-submission waits from [`GpuContext::pool_frame_end`].
    #[cfg(test)]
    pub(crate) fn pool_waits(&self) -> u64 {
        self.pool.lock().map(|p| p.waits).unwrap_or_default()
    }

    /// Add or subtract phantom [`Pool::dying`] bytes without a work-done callback (tests that
    /// simulate a queue which stopped completing work).
    #[cfg(test)]
    pub(crate) fn pool_charge_dying(&self, bytes: isize) {
        use std::sync::atomic::Ordering::Relaxed;
        if let Ok(p) = self.pool.lock() {
            if bytes >= 0 {
                p.dying.fetch_add(bytes as usize, Relaxed);
            } else {
                let sub = (-bytes) as usize;
                let _ = p.dying.try_update(Relaxed, Relaxed, |d| Some(d.saturating_sub(sub)));
            }
        }
    }

    /// Texture pool counters: (working textures reused, newly allocated).
    pub fn pool_stats(&self) -> (u64, u64) {
        self.pool.lock().map(|p| (p.hits, p.misses)).unwrap_or_default()
    }

    /// Upload a CPU image (`None` when it does not fit the device).
    pub fn upload_image(&self, img: &Image) -> Option<GpuImage> {
        if !self.fits(img.width, img.height) {
            return None;
        }
        self.transfers.count(true, img.data.len() * 16);
        // A texture from the pool's shared free list: no open encoder can still read it, and the
        // copy below runs before anything recorded after this call.
        let (g, _) = self.pooled(img.width, img.height, UPLOAD_OWNER);
        if !self.upload_staging.write(&self.device, &self.queue, img, &g.texture) {
            self.queue.write_texture(
                wgpu::TexelCopyTextureInfo { texture: &g.texture, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
                bytemuck::cast_slice(&img.data),
                wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(img.width * 16), rows_per_image: Some(img.height) },
                wgpu::Extent3d { width: img.width, height: img.height, depth_or_array_layers: 1 },
            );
        }
        Some(g)
    }

    /// Upload a decoded footage picture once while the footage source keeps it: every layer
    /// and effect input showing the same frame shares the texture (`Renderer::footage_picture`).
    pub(crate) fn upload_picture(&self, img: &Arc<Image>) -> Option<GpuImage> {
        if let Some(t) = self.pictures.lock().ok().and_then(|mut p| p.get(img)) {
            return Some(t);
        }
        let tex = self.upload_image(img)?;
        if let Ok(mut p) = self.pictures.lock() {
            p.insert(img, tex.clone(), crate::upload::PICTURE_BUDGET.min(self.upload_budget() / 4));
        }
        Some(tex)
    }

    /// Entries in the footage picture cache.
    #[cfg(test)]
    pub(crate) fn picture_entries(&self) -> usize {
        self.pictures.lock().map(|p| p.len()).unwrap_or_default()
    }

    /// The upload staging buffers (tests).
    #[cfg(test)]
    pub(crate) fn upload_staging(&self) -> &crate::upload::Staging {
        &self.upload_staging
    }

    /// A render is done: drop the footage picture textures and the upload staging buffers
    /// (the next render's pictures upload again; their textures come from the pool).
    pub fn trim_uploads(&self) {
        if let Ok(mut p) = self.pictures.lock() {
            p.clear();
        }
        self.upload_staging.clear();
    }

    /// Entries in the layer upload cache.
    #[cfg(test)]
    pub(crate) fn upload_entries(&self) -> usize {
        self.uploads.lock().map(|u| u.map.len()).unwrap_or_default()
    }

    /// Upload a (layer cache) buffer, reusing the texture while the same `Arc` is alive: static
    /// layers upload once, not every frame.
    pub(crate) fn upload_buf(&self, buf: &Arc<Buf>) -> Option<GpuImage> {
        let key = Arc::as_ptr(buf) as usize;
        if let Ok(mut u) = self.uploads.lock() {
            u.clock += 1;
            let now = u.clock;
            if let Some(e) = u.map.get_mut(&key)
                && e.buf.upgrade().is_some_and(|b| Arc::ptr_eq(&b, buf))
            {
                e.last_use = now;
                return Some(e.img.clone());
            }
        }
        let img = self.upload_image(&buf.img)?;
        if let Ok(mut u) = self.uploads.lock() {
            let bytes = buf.img.data.len() * 16;
            let now = u.clock;
            if let Some(old) = u.map.insert(key, Upload { buf: Arc::downgrade(buf), img: img.clone(), bytes, last_use: now }) {
                u.bytes -= old.bytes;
            }
            u.bytes += bytes;
            // Drop textures of freed buffers at once (a layer whose buffer changes every frame
            // makes a new one each time, and nothing can hit the old one again), then the least
            // recently used over budget.
            let budget = self.upload_budget();
            let dead: Vec<usize> = u.map.iter().filter(|(_, e)| e.buf.strong_count() == 0).map(|(k, _)| *k).collect();
            for k in dead {
                if let Some(e) = u.map.remove(&k) {
                    u.bytes -= e.bytes;
                }
            }
            if u.bytes > budget && !self.upload_full_warned.swap(true, std::sync::atomic::Ordering::Relaxed) {
                log::warn!(
                    target: "effectcraft_gpu",
                    "GPU upload cache full ({} MB): evicting layer textures, which upload again when next used; a frame whose static layers exceed it re-uploads them every frame",
                    budget >> 20
                );
            }
            while u.bytes > budget {
                let Some(k) = u.map.iter().min_by_key(|(_, e)| e.last_use).map(|(k, _)| *k) else { break };
                if let Some(e) = u.map.remove(&k) {
                    u.bytes -= e.bytes;
                }
            }
        }
        Some(img)
    }

    /// Forget uploaded layer textures.
    pub fn clear_uploads(&self) {
        if let Ok(mut u) = self.uploads.lock() {
            u.map.clear();
            u.bytes = 0;
        }
        if let Ok(mut p) = self.pictures.lock() {
            p.clear();
        }
        self.upload_staging.clear();
        if let Ok(mut p) = self.pool.lock() {
            p.clear();
        }
        if let Ok(mut s) = self.staging.lock() {
            s.clear();
        }
    }

    /// After a failed frame: forget every texture and buffer kept for reuse, since any of them
    /// may be one the device rejected (#1319). Later frames allocate afresh.
    pub fn discard_kept(&self) {
        self.clear_uploads();
        if let Ok(mut p) = self.pool.lock() {
            p.discard();
        }
        if let Ok(mut z) = self.zeros.lock() {
            z.clear();
        }
        if let Ok(mut c) = self.particle_states.lock() {
            c.clear();
        }
    }
}

/// Records GPU work for one render (a command encoder submitted at readback or the end).
pub(crate) struct Enc<'g> {
    pub g: &'g GpuContext,
    /// The open command encoder and its id in the texture pool.
    enc: Option<(wgpu::CommandEncoder, u64)>,
    last_submit: Option<wgpu::SubmissionIndex>,
    pending: usize,
    /// A dispatch named a kernel the device doesn't have (see [`Enc::take_missing`]).
    missing: bool,
    /// This `Enc`'s id in the texture pool (textures it may reuse at once).
    owner: u64,
    /// `owner` is lent ([`Enc::lent`]) and outlives this `Enc`.
    lent: bool,
}

/// Images bound to a compute kernel's source, optional auxiliary and output slots.
pub(crate) struct KernelImages<'a> {
    pub src: &'a GpuImage,
    pub aux: Option<&'a GpuImage>,
    pub out: &'a GpuImage,
}

impl Drop for Enc<'_> {
    fn drop(&mut self) {
        // Unsubmitted work is discarded, so nothing recorded will read pooled textures.
        if let Ok(mut p) = self.g.pool.lock() {
            if let Some((_, id)) = self.enc.take() {
                p.close(id);
            }
            if self.lent {
                p.give_back(self.owner);
            } else {
                p.unregister(self.owner);
            }
        }
    }
}

impl<'g> Enc<'g> {
    pub fn new(g: &'g GpuContext) -> Enc<'g> {
        let owner = g.pool.lock().map(|mut p| p.register()).unwrap_or(0);
        Enc { g, enc: None, last_submit: None, pending: 0, missing: false, owner, lent: false }
    }

    /// An `Enc` for self-contained work (an effect chain): once it is dropped, its working
    /// textures can be reused at once ([`Pool::give_back`]). Its images stay with it, except a
    /// result another `Enc` records work on ([`Enc::uses`]), whose lease then releases through
    /// the guarded [`Pool::release`]. A plain [`Enc::new`] hands its
    /// textures back through the shared pool, where they wait for every open encoder, and a
    /// GPU frame keeps its encoder open while the CPU renders layers with GPU effects, so every
    /// effect's intermediates stayed allocated until the frame ended.
    pub(crate) fn lent(g: &'g GpuContext) -> Enc<'g> {
        let owner = g.pool.lock().map(|mut p| p.lend()).unwrap_or(0);
        Enc { g, enc: None, last_submit: None, pending: 0, missing: false, owner, lent: true }
    }

    /// `img` is used by this `Enc`: if another `Enc` made it, that one must not reuse it at once
    /// (this encoder may still be open when its lease drops), so mark it escaped.
    pub(crate) fn uses(&self, img: &GpuImage) {
        if img.foreign_to(self.owner) {
            img.escape();
        }
    }

    /// Whether a dispatch since the last call named a kernel the device doesn't have (one
    /// that failed to build): its output is wrong, and the effect must render on the CPU.
    pub(crate) fn take_missing(&mut self) -> bool {
        std::mem::take(&mut self.missing)
    }

    /// The command encoder, to record work that touches `imgs` (every pooled image the commands
    /// read or write; buffers and textures of the caller's own need not be named). The only
    /// way to record outside this module: naming the images marks foreign leases escaped
    /// ([`Enc::uses`]). Callers must name every pooled image the encoder's work uses, here or
    /// with separate [`Enc::uses`] calls (`record(&[])` plus `uses()` is allowed); this is a
    /// convention, not enforced, and skipping it reintroduces the reuse race it guards (#1441).
    pub(crate) fn record(&mut self, imgs: &[&GpuImage]) -> &mut wgpu::CommandEncoder {
        for img in imgs {
            self.uses(img);
        }
        self.encoder()
    }

    /// The raw encoder: within this module only, after [`Enc::uses`] on every pooled image.
    fn encoder(&mut self) -> &mut wgpu::CommandEncoder {
        let g = self.g;
        &mut self
            .enc
            .get_or_insert_with(|| {
                let id = g.pool.lock().map(|mut p| p.open()).unwrap_or(0);
                (g.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("effectcraft") }), id)
            })
            .0
    }

    /// A new zeroed image (pooled; a reused texture is cleared by a kernel).
    pub fn image(&mut self, w: u32, h: u32) -> GpuImage {
        let (img, reused) = self.g.pooled(w, h, self.owner);
        if reused {
            self.fill(&img, 0.0);
        }
        img
    }

    /// A new image for a kernel that writes every pixel of it (pooled; contents undefined).
    /// Under test it starts out as NaNs, so a kernel that skips pixels fails the CPU oracle.
    pub fn scratch(&mut self, w: u32, h: u32) -> GpuImage {
        let (img, _) = self.g.pooled(w, h, self.owner);
        if cfg!(test) {
            self.fill(&img, f32::NAN);
        }
        img
    }

    /// A transparent image to read from (shared per size; never written).
    pub fn zeros(&self, w: u32, h: u32) -> GpuImage {
        let (w, h) = (w.max(1), h.max(1));
        let Ok(mut z) = self.g.zeros.lock() else { return self.g.image(w, h) };
        if z.len() > 8 && !z.contains_key(&(w, h)) {
            z.clear();
        }
        z.entry((w, h)).or_insert_with(|| self.g.image(w, h)).clone()
    }

    /// Set every pixel of `img` to `v`.
    fn fill(&mut self, img: &GpuImage, v: f32) {
        let mut p = Params::default();
        p.f[0] = [v; 4];
        let dummy = GpuImage::new(self.g.dummy.clone(), 1, 1);
        self.pixels("fill", &p, &dummy, None, img, None);
    }

    /// Run kernel `entry` writing `out` (dispatched over `groups` workgroups).
    pub fn dispatch(&mut self, entry: &str, p: &Params, images: KernelImages<'_>, data: Option<&wgpu::Buffer>, groups: (u32, u32)) {
        self.dispatch_ext(entry, p, images, data, groups, None);
    }

    /// [`Enc::dispatch`] for the kernels in `EXT_ENTRIES`, with their group 1 storage buffers.
    pub fn dispatch_ext(
        &mut self,
        entry: &str,
        p: &Params,
        images: KernelImages<'_>,
        data: Option<&wgpu::Buffer>,
        groups: (u32, u32),
        ext: Option<[&wgpu::Buffer; 4]>,
    ) {
        let KernelImages { src, aux, out } = images;
        let g = self.g;
        let Some(pipe) = g.pipeline(entry) else {
            self.missing = true;
            return;
        };
        let ub = g.buffer_init(None, &p.bytes(), wgpu::BufferUsages::UNIFORM);
        let sv = src.texture.create_view(&Default::default());
        let av = aux.map(|a| a.texture.create_view(&Default::default()));
        let ov = out.texture.create_view(&Default::default());
        let bg = g.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &g.bgl,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: ub.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&sv) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(av.as_ref().unwrap_or(&g.dummy_tex)) },
                wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(&ov) },
                wgpu::BindGroupEntry { binding: 4, resource: data.unwrap_or(&g.dummy_buf).as_entire_binding() },
            ],
        });
        let bg_ext = ext.map(|b| {
            g.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: None,
                layout: &g.bgl_ext,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: b[0].as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 1, resource: b[1].as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 2, resource: b[2].as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 3, resource: b[3].as_entire_binding() },
                ],
            })
        });
        let imgs: Vec<&GpuImage> = [Some(src), aux, Some(out)].into_iter().flatten().collect();
        let enc = self.record(&imgs);
        {
            let mut pass = enc.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some(entry), timestamp_writes: None });
            pass.set_pipeline(pipe);
            pass.set_bind_group(0, &bg, &[]);
            if let Some(b) = &bg_ext {
                pass.set_bind_group(1, b, &[]);
            }
            pass.dispatch_workgroups(groups.0.max(1), groups.1.max(1), 1);
        }
        self.pending += 1;
        // Keep command buffers (and the transient textures they hold) bounded.
        if self.pending >= 256 {
            self.submit();
        }
    }

    /// Per-pixel kernel over `out`'s size (16×16 workgroups).
    pub fn pixels(&mut self, entry: &str, p: &Params, src: &GpuImage, aux: Option<&GpuImage>, out: &GpuImage, data: Option<&wgpu::Buffer>) {
        let groups = (out.width.div_ceil(16), out.height.div_ceil(16));
        self.dispatch(entry, p, KernelImages { src, aux, out }, data, groups);
    }

    /// Copy `src` into a larger zeroed image at (`x`, `y`) (Buf::pad).
    pub fn copy_into(&mut self, src: &GpuImage, dst: &GpuImage, x: u32, y: u32) {
        let enc = self.record(&[src, dst]);
        enc.copy_texture_to_texture(
            wgpu::TexelCopyTextureInfo { texture: &src.texture, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            wgpu::TexelCopyTextureInfo { texture: &dst.texture, mip_level: 0, origin: wgpu::Origin3d { x, y, z: 0 }, aspect: wgpu::TextureAspect::All },
            wgpu::Extent3d { width: src.width, height: src.height, depth_or_array_layers: 1 },
        );
    }

    /// A `w` × `h` image showing `src` with its pixel (`x`, `y`) at the top-left, transparent
    /// where `src` doesn't reach (Image::crop).
    pub fn crop(&mut self, src: &GpuImage, x: i64, y: i64, w: u32, h: u32) -> GpuImage {
        let dst = self.image(w, h);
        let span = |o: i64, n: u32, len: u32| {
            let (a, b) = (o.max(0), (o + n as i64).min(len as i64));
            (b > a).then(|| (a as u32, (a - o) as u32, (b - a) as u32))
        };
        let (Some((sx, dx, cw)), Some((sy, dy, ch))) = (span(x, w, src.width), span(y, h, src.height)) else { return dst };
        let enc = self.record(&[src, &dst]);
        enc.copy_texture_to_texture(
            wgpu::TexelCopyTextureInfo { texture: &src.texture, mip_level: 0, origin: wgpu::Origin3d { x: sx, y: sy, z: 0 }, aspect: wgpu::TextureAspect::All },
            wgpu::TexelCopyTextureInfo { texture: &dst.texture, mip_level: 0, origin: wgpu::Origin3d { x: dx, y: dy, z: 0 }, aspect: wgpu::TextureAspect::All },
            wgpu::Extent3d { width: cw, height: ch, depth_or_array_layers: 1 },
        );
        dst
    }

    /// The last submission of this encoder's work (what a readback after it waits for).
    pub(crate) fn last_submission(&self) -> Option<wgpu::SubmissionIndex> {
        self.last_submit.clone()
    }

    /// Submit recorded work.
    pub fn submit(&mut self) {
        if let Some((enc, id)) = self.enc.take() {
            self.last_submit = Some(self.g.queue.submit([enc.finish()]));
            if let Ok(mut p) = self.g.pool.lock() {
                p.close(id);
            }
        }
        self.pending = 0;
    }

    /// Submits and waits on this encoder's own submission when the device allocator is over the
    /// memory cap and enough of that memory is reclaimable, so wgpu frees textures dropped so far.
    pub fn mem_gate(&mut self) {
        if !self.g.memcap.due() {
            return;
        }
        let allocated = self.g.allocated_bytes();
        self.g.memcap.note(allocated);
        if allocated <= self.g.memcap.cap() {
            let _ = self.g.memcap.should_wait(allocated, 0);
            return;
        }
        let closing = self.enc.as_ref().map(|(_, id)| *id);
        let kept = self.g.kept_bytes(self.owner, closing);
        if !self.g.memcap.should_wait(allocated, kept) {
            return;
        }
        self.submit();
        if let Ok(mut p) = self.g.pool.lock() {
            p.flush_owned(self.owner);
        }
        let idx = self.last_submit.take().unwrap_or_else(|| self.g.queue.submit([]));
        self.g.poll_wait(idx, self.g.memcap.wait_timeout());
        self.g.memcap.record_wait(self.g.allocated_bytes(), self.g.kept_bytes(self.owner, None));
    }

    /// Read an image back to the CPU (submits and waits). `None` where readback is impossible.
    pub fn download(&mut self, img: &GpuImage) -> Option<Image> {
        self.uses(img);
        let layout = crate::readback::Layout::new(img.width, img.height, 16, self.g.device.limits().max_buffer_size)
            .map_err(|e| log::error!("gpu image readback: {e}"))
            .ok()?;
        // The mapped rows go straight into the image's pixels: one copy (#3335).
        let data: Vec<[f32; 4]> = self.read_texture_with(&img.texture, [0, 0, img.width, img.height], 16, crate::readback::rows::<[f32; 4]>)?;
        if data.len() != layout.len / 16 {
            log::error!("gpu image readback size mismatch");
            return None;
        }
        Some(Image { width: img.width, height: img.height, data })
    }

    /// Tightly packed texel bytes of a texture (`bpp` bytes per pixel).
    pub fn read_texture(&mut self, texture: &wgpu::Texture, w: u32, h: u32, bpp: u32) -> Option<Vec<u8>> {
        self.read_rect(texture, 0, 0, w, h, bpp)
    }

    /// [`Enc::read_texture`] of a sub-rectangle at (`x`, `y`): a fill only changes its hole, so
    /// only the hole's bounds need to cross back (`bytes_per_row` is still row-aligned inside).
    pub fn read_rect(&mut self, texture: &wgpu::Texture, x: u32, y: u32, w: u32, h: u32, bpp: u32) -> Option<Vec<u8>> {
        let layout =
            crate::readback::Layout::new(w, h, bpp, self.g.device.limits().max_buffer_size).map_err(|e| log::error!("gpu texture readback: {e}")).ok()?;
        let out: Vec<u8> = self.read_texture_with(texture, [x, y, w, h], bpp, crate::readback::rows::<u8>)?;
        if out.len() != layout.len {
            log::error!("gpu texture readback size mismatch");
            return None;
        }
        Some(out)
    }

    /// Read `rect` (`[x, y, w, h]`) of `texture` back through a reused staging buffer: `read`
    /// gets the mapped bytes, the row length, the padded row stride and the row count, and
    /// copies the rows into their destination itself (one copy, #3335). Waits only for this
    /// encoder's own submission, so frames rendering on other threads don't delay it.
    fn read_texture_with<R>(
        &mut self,
        texture: &wgpu::Texture,
        rect: [u32; 4],
        bpp: u32,
        read: impl FnOnce(&[u8], usize, usize, usize) -> Result<R, String>,
    ) -> Option<R> {
        let [x, y, w, h] = rect;
        if !self.g.can_wait() {
            return None;
        }
        let layout =
            crate::readback::Layout::new(w, h, bpp, self.g.device.limits().max_buffer_size).map_err(|e| log::error!("gpu texture readback: {e}")).ok()?;
        self.g.transfers.count(false, layout.len);
        let row = layout.row as usize;
        let padded = layout.stride;
        let size = layout.size;
        let buf = self.g.staging.lock().ok().and_then(|mut s| s.get_mut(&size).and_then(Vec::pop)).unwrap_or_else(|| {
            self.g.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("readback"),
                size,
                usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        });
        self.encoder().copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo { texture, mip_level: 0, origin: wgpu::Origin3d { x, y, z: 0 }, aspect: wgpu::TextureAspect::All },
            wgpu::TexelCopyBufferInfo { buffer: &buf, layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(padded), rows_per_image: Some(h) } },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
        self.submit();
        let out = self
            .g
            .read_mapped(&buf, self.last_submit.clone(), |view| read(view, row, padded as usize, h as usize))
            .map_err(|e| log::error!("gpu texture readback: {e}"))
            .ok()?;
        // Keep a few staging buffers per size (frames read back at the same size every time).
        // Layers whose bounds change every frame read back at a new size each time: drop the
        // other sizes once over budget, or mapped host memory grows for the whole render (#2081).
        if let Ok(mut s) = self.g.staging.lock() {
            let held: u64 = s.iter().map(|(k, v)| k.saturating_mul(v.len() as u64)).sum();
            if held.saturating_add(size) > STAGING_BUDGET {
                s.retain(|k, _| *k == size);
            }
            let v = s.entry(size).or_default();
            if v.len() < 2 && size <= STAGING_BUDGET {
                v.push(buf);
            }
        }
        Some(out)
    }

    /// [`Enc::read_texture`] without waiting for the GPU (the browser's main thread can't): `done`
    /// gets the bytes once the copy is mapped (from the browser's event loop on the web; from
    /// a later device poll natively).
    pub fn read_texture_async(&mut self, texture: &wgpu::Texture, w: u32, h: u32, bpp: u32, done: impl FnOnce(Option<Vec<u8>>) + wgpu::WasmNotSend + 'static) {
        if let Err(e) = self.g.readbacks.check() {
            log::error!("gpu readback: {e}");
            crate::readback::reject(done, e);
            return;
        }
        let layout = match crate::readback::Layout::new(w, h, bpp, self.g.device.limits().max_buffer_size) {
            Ok(layout) => layout,
            Err(e) => {
                log::error!("gpu texture readback: {e}");
                crate::readback::reject(done, e);
                return;
            }
        };
        let row = layout.row;
        let padded = layout.stride;
        let buf = self.g.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("readback-async"),
            size: layout.size,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.encoder().copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo { texture, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            wgpu::TexelCopyBufferInfo { buffer: &buf, layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(padded), rows_per_image: Some(h) } },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
        self.submit();
        self.g.map_readback(&buf, row as usize, padded as usize, h as usize, move |r| {
            done(r.map_err(|e| log::error!("gpu texture readback: {e}")).ok());
        });
        #[cfg(not(target_arch = "wasm32"))]
        let _ = self.g.poll_readbacks(wgpu::PollType::Poll);
    }

    /// A deferred readback of `img` (RGBA f32) under `key` (see [`crate::deferred`]): the bytes
    /// once they arrived (`Some(None)`: the readback failed), else `None` after starting the
    /// copy if needed and marking the pass missed.
    pub(crate) fn read_keyed(&mut self, img: &GpuImage, key: u128, offset: [f64; 2], scale: f64) -> Option<Option<crate::deferred::Readback>> {
        self.uses(img);
        use crate::deferred::Lookup;
        let d = self.g.deferred.clone()?;
        match d.lookup(key) {
            Lookup::Ready(r) => return Some(r),
            Lookup::InFlight => {}
            Lookup::Absent => {
                self.g.transfers.count(false, img.width as usize * img.height as usize * 16);
                let done = d.start(key, img.width, img.height, offset, scale);
                self.read_texture_async(&img.texture, img.width, img.height, 16, done);
            }
        }
        d.miss();
        None
    }

    /// A data buffer for kernels that read tables (curves).
    pub fn data(&self, v: &[f32]) -> wgpu::Buffer {
        let mut bytes = Vec::with_capacity(v.len().max(4) * 4);
        for x in v {
            bytes.extend_from_slice(&x.to_le_bytes());
        }
        self.bytes(bytes)
    }

    /// Copy an image into a storage buffer (RGBA f32 rows) for kernels that read a third image
    /// through `data`: (buffer, row length in pixels).
    pub fn image_rows(&mut self, img: &GpuImage) -> (wgpu::Buffer, u32) {
        self.uses(img);
        let row = (img.width * 16).div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let buf = self.g.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("image rows"),
            size: row as u64 * img.height as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.encoder().copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo { texture: &img.texture, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            wgpu::TexelCopyBufferInfo {
                buffer: &buf,
                layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(row), rows_per_image: Some(img.height) },
            },
            wgpu::Extent3d { width: img.width, height: img.height, depth_or_array_layers: 1 },
        );
        (buf, row / 16)
    }

    /// A read-only storage buffer holding `bytes` (padded to 16 bytes).
    pub fn bytes(&self, mut bytes: Vec<u8>) -> wgpu::Buffer {
        while bytes.len() < 16 || !bytes.len().is_multiple_of(4) {
            bytes.push(0);
        }
        self.g.buffer_init(Some("data"), &bytes, wgpu::BufferUsages::STORAGE)
    }

    /// Convert to the viewer's premultiplied RGBA8 texture (egui-wgpu's native texture format),
    /// with a full mip chain so the viewer minifies it without aliasing (#417).
    pub fn display(&mut self, img: &GpuImage) -> wgpu::Texture {
        self.uses(img);
        let g = self.g;
        let levels = mip_levels(img.width, img.height);
        let tex = g.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("viewer frame"),
            size: wgpu::Extent3d { width: img.width, height: img.height, depth_or_array_layers: 1 },
            mip_level_count: levels,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let level = |l: u32| tex.create_view(&wgpu::TextureViewDescriptor { base_mip_level: l, mip_level_count: Some(1), ..Default::default() });
        // Level 0 from the frame, then each level the 2 × 2 mean of the one above.
        let mut steps = vec![(&g.display, img.texture.create_view(&Default::default()), level(0), img.width, img.height)];
        for l in 1..levels {
            steps.push((&g.display_mip, level(l - 1), level(l), (img.width >> l).max(1), (img.height >> l).max(1)));
        }
        let enc = self.encoder();
        for (pipeline, src, dst, w, h) in &steps {
            let bg = g.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: None,
                layout: &g.display_bgl,
                entries: &[
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(src) },
                    wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(dst) },
                ],
            });
            let mut pass = enc.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("display"), timestamp_writes: None });
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, &bg, &[]);
            pass.dispatch_workgroups(w.div_ceil(16), h.div_ceil(16), 1);
        }
        tex
    }
}

/// Mip levels of a full chain for a `w × h` texture (down to 1 × 1).
pub(crate) fn mip_levels(w: u32, h: u32) -> u32 {
    u32::BITS - w.max(h).max(1).leading_zeros()
}

#[cfg(test)]
mod pool_stall_tests {
    use super::Pool;
    use std::sync::atomic::Ordering::Relaxed;

    #[test]
    fn frame_end_skips_wait_while_stalled_until_dying_drops() {
        let mut p = Pool::default();
        p.dying.store(100, Relaxed);
        assert_eq!(p.frame_end(), 100);
        p.waited(100);
        assert_eq!(p.stalled, 100);
        assert_eq!(p.waits, 1);
        assert_eq!(p.frame_end(), 0);
        p.dying.store(150, Relaxed);
        assert_eq!(p.frame_end(), 0);
        p.dying.store(50, Relaxed);
        assert_eq!(p.frame_end(), 50);
        assert_eq!(p.stalled, 0);
        p.dying.store(0, Relaxed);
        assert_eq!(p.frame_end(), 0);
    }
}

#[cfg(test)]
mod adapter_name_tests {
    use super::{match_adapter_name, wanted_adapter_name};

    fn adapters() -> Vec<(String, String)> {
        [("NVIDIA GeForce GTX 1070", "Vulkan"), ("Intel(R) UHD Graphics 630", "Dx12"), ("llvmpipe (LLVM 19.1.7, 256 bits)", "Gl")]
            .map(|(n, b)| (n.into(), b.into()))
            .to_vec()
    }

    #[test]
    fn matches_case_insensitive_substring() {
        assert_eq!(match_adapter_name("intel", &adapters()), Ok(1));
        assert_eq!(match_adapter_name("GTX 1070", &adapters()), Ok(0));
        assert_eq!(match_adapter_name("LLVMPIPE", &adapters()), Ok(2));
    }

    #[test]
    fn first_match_wins() {
        assert_eq!(match_adapter_name("i", &adapters()), Ok(0));
    }

    #[test]
    fn empty_or_blank_name_is_unset_and_value_is_trimmed() {
        assert_eq!(wanted_adapter_name(None), None);
        assert_eq!(wanted_adapter_name(Some("")), None);
        assert_eq!(wanted_adapter_name(Some("  ")), None);
        assert_eq!(wanted_adapter_name(Some(" intel ")), Some("intel"));
        assert_eq!(match_adapter_name(wanted_adapter_name(Some(" intel ")).unwrap(), &adapters()), Ok(1));
    }

    #[test]
    fn mismatch_lists_name_and_backend() {
        let e = match_adapter_name("NoSuch", &adapters()).unwrap_err();
        assert!(e.contains("\"NoSuch\""), "{e}");
        assert!(e.contains("Intel(R) UHD Graphics 630 (Dx12)"), "{e}");
        assert!(e.contains("NVIDIA GeForce GTX 1070 (Vulkan)"), "{e}");
    }

    #[test]
    fn backend_is_not_part_of_the_match_and_message_says_so() {
        let e = match_adapter_name("dx12", &adapters()).unwrap_err();
        assert!(e.contains("name only"), "{e}");
        assert!(e.contains("WGPU_BACKEND"), "{e}");
    }

    #[test]
    fn mismatch_lists_identical_name_and_backend_once() {
        let mut a = adapters();
        a.insert(2, ("Intel(R) UHD Graphics 630".into(), "Dx12".into()));
        a.push(("Intel(R) UHD Graphics 630".into(), "Vulkan".into()));
        let e = match_adapter_name("NoSuch", &a).unwrap_err();
        assert_eq!(e.matches("Intel(R) UHD Graphics 630 (Dx12)").count(), 1, "{e}");
        assert_eq!(e.matches("Intel(R) UHD Graphics 630 (Vulkan)").count(), 1, "{e}");
    }

    #[test]
    fn no_adapters_has_its_own_message() {
        let e = match_adapter_name("x", &[]).unwrap_err();
        assert!(e.contains("no adapters on the enabled backends"), "{e}");
    }
}
