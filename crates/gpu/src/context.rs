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

/// Effect entry points in `kernels.wgsl`. Like the `fx_*` kernels, one that fails to build
/// (a driver's shader compiler rejecting it, #613) leaves its effects to the CPU instead of
/// turning the whole compositor off.
const EFFECT_ENTRIES: &[&str] = &["box_h", "box_v", "directional", "glow_bright", "glow_combine", "shadow_make", "shadow_combine", "pointwise"];

/// Entry points that also bind group 1 (four read-only storage buffers; see
/// [`Enc::dispatch_ext`]).
const EXT_ENTRIES: &[&str] = &["classic3d"];

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
            if self.escaped.load(std::sync::atomic::Ordering::Relaxed) {
                p.release(self.texture.clone());
            } else {
                p.release_owned(self.texture.clone(), self.owner);
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
    /// Reports when the submissions that may use an evicted texture are done.
    queue: Option<wgpu::Queue>,
    /// Textures released while the [`Enc`] that took them is alive, kept for that `Enc` only.
    /// Its later commands run after everything it recorded or submitted before (one queue,
    /// recording order), so it can overwrite them at once instead of allocating a new texture
    /// for every intermediate of a frame (a 4K frame holds dozens). Other encoders never see a
    /// working texture of another `Enc` before it is gone.
    owned: HashMap<u64, HashMap<(u32, u32), Vec<wgpu::Texture>>>,
    next_owner: u64,
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
                    dying.fetch_sub(evicted, Relaxed);
                }),
                None => {
                    dying.fetch_sub(evicted, Relaxed);
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
    /// Returns whether evicted textures still wait for their work-done callback.
    fn frame_end(&mut self) -> bool {
        self.frame += 1;
        let (frame, free) = (self.frame, &self.free);
        self.asked.retain(|k, f| frame - *f < STALE_FRAMES || free.contains_key(k));
        self.dying.load(std::sync::atomic::Ordering::Relaxed) > 0
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

    fn release_owned(&mut self, t: wgpu::Texture, owner: u64) {
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
    bgl: wgpu::BindGroupLayout,
    bgl_ext: wgpu::BindGroupLayout,
    pipelines: HashMap<&'static str, wgpu::ComputePipeline>,
    /// Effect kernels that failed to build, and why (their effects render on the CPU).
    unavailable: Vec<(&'static str, String)>,
    display_bgl: wgpu::BindGroupLayout,
    display: wgpu::ComputePipeline,
    /// The display texture's next mip level from the one above (`display.wgsl` `mip`).
    display_mip: wgpu::ComputePipeline,
    dummy: wgpu::Texture,
    dummy_tex: wgpu::TextureView,
    dummy_buf: wgpu::Buffer,
    uploads: Mutex<Uploads>,
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
    readbacks: Arc<crate::readback::Readbacks>,
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
fn init_resource<T>(_device: &wgpu::Device, _label: &str, create: impl FnOnce() -> T) -> Result<T, String> {
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
        let src = [
            include_str!("shaders/common.wgsl"),
            include_str!("shaders/kernels.wgsl"),
            include_str!("shaders/classic3d.wgsl"),
            include_str!("shaders/fx_color.wgsl"),
            include_str!("shaders/fx_distort.wgsl"),
            include_str!("shaders/fx_generate.wgsl"),
            include_str!("shaders/fx_key.wgsl"),
            include_str!("shaders/fx_stylize.wgsl"),
            include_str!("shaders/fx_noise.wgsl"),
            include_str!("shaders/fx_tone.wgsl"),
            include_str!("shaders/fx_warp.wgsl"),
            include_str!("shaders/fx_extra.wgsl"),
            include_str!("shaders/fx_depth.wgsl"),
            include_str!("shaders/fx_lut.wgsl"),
            include_str!("shaders/fx_sim.wgsl"),
            include_str!("shaders/fx_particles.wgsl"),
            include_str!("shaders/fx_vr.wgsl"),
            include_str!("shaders/fx_light.wgsl"),
            include_str!("shaders/fx_transition.wgsl"),
            include_str!("shaders/fx_text.wgsl"),
            include_str!("shaders/fx_time.wgsl"),
            include_str!("shaders/fx_pixel2.wgsl"),
            include_str!("shaders/fx_gen2.wgsl"),
            include_str!("shaders/sky.wgsl"),
        ]
        .concat();
        let module = init_resource(&device, "kernels shader module", || {
            device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("effectcraft kernels"), source: wgpu::ShaderSource::Wgsl(src.into()) })
        })?;
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
        let build = |e: &'static str, layout: &wgpu::PipelineLayout| {
            init_resource(&device, e, || {
                device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                    label: Some(e),
                    layout: Some(layout),
                    module: &module,
                    entry_point: Some(e),
                    compilation_options: Default::default(),
                    cache: None,
                })
            })
        };
        // The compositor's own kernels are required; an effect kernel that doesn't build is
        // left out, and its effects render on the CPU (see `Enc::take_missing`).
        let mut pipelines = ENTRIES
            .iter()
            .chain(crate::adv3d::SKY_KERNELS)
            .map(|e| (e, &layout))
            .chain(EXT_ENTRIES.iter().map(|e| (e, &layout_ext)))
            .map(|(e, layout)| Ok((*e, build(e, layout)?)))
            .collect::<Result<HashMap<_, _>, String>>()?;
        let mut unavailable = vec![];
        for e in EFFECT_ENTRIES
            .iter()
            .chain(crate::fx_color::KERNELS)
            .chain(crate::fx_distort::KERNELS)
            .chain(crate::fx_generate::KERNELS)
            .chain(crate::fx_key::KERNELS)
            .chain(crate::fx_stylize::KERNELS)
            .chain(crate::fx_noise::KERNELS)
            .chain(crate::fx_tone::KERNELS)
            .chain(crate::fx_warp::KERNELS)
            .chain(crate::fx_extra::KERNELS)
            .chain(crate::fx_depth::KERNELS)
            .chain(crate::fx_lut::KERNELS)
            .chain(crate::fx_sim::KERNELS)
            .chain(crate::fx_particles::KERNELS)
            .chain(crate::fx_vr::KERNELS)
            .chain(crate::fx_light::KERNELS)
            .chain(crate::fx_transition::KERNELS)
            .chain(crate::fx_text::KERNELS)
            .chain(crate::fx_time::KERNELS)
            .chain(crate::fx_pixel2::KERNELS)
            .chain(crate::fx_gen2::KERNELS)
        {
            match build(e, &layout) {
                Ok(p) => {
                    pipelines.insert(*e, p);
                }
                Err(why) => {
                    log::warn!("gpu {name}: kernel {e} unavailable, its effects render on the CPU: {why}");
                    unavailable.push((*e, why));
                }
            }
        }
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
        let pool = Arc::new(Mutex::new(Pool { queue: Some(queue.clone()), ..Default::default() }));
        Ok(GpuContext {
            device,
            queue,
            name,
            max_dim,
            bgl,
            bgl_ext,
            pipelines,
            unavailable,
            display_bgl,
            display,
            display_mip,
            dummy_tex,
            dummy,
            dummy_buf,
            uploads: Mutex::new(Uploads::default()),
            upload_budget: std::sync::atomic::AtomicUsize::new(UPLOAD_BUDGET),
            upload_full_warned: Default::default(),
            pool,
            staging: Default::default(),
            transfers: Default::default(),
            zeros: Default::default(),
            adv3d: std::sync::OnceLock::new(),
            adv3d_raster: adv3d_unsupported.is_none(),
            particles: Default::default(),
            particle_states: Default::default(),
            deferred: None,
            readbacks,
        })
    }

    /// Whether this context has observed a readback timeout or device failure.
    /// A successful check is not a guarantee against faults after it returns.
    /// Shared-device hosts must forward their device failures through the notifier.
    pub fn check_health(&self) -> Result<(), String> {
        self.readbacks.check()
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

    /// All device polling routes through this failure bridge.
    pub(crate) fn poll_readbacks(&self, kind: wgpu::PollType) -> Result<wgpu::PollStatus, String> {
        self.readbacks.check()?;
        self.device.poll(kind).map_err(|e| {
            let error = format!("GPU readback poll: {e}");
            self.readbacks.retire(error.clone());
            log::error!("{error}");
            error
        })
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

    /// A bounded native readback: polling and receiving share one deadline.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn read_buffer(&self, buffer: &wgpu::Buffer, row: usize, stride: usize, height: usize) -> Result<Vec<u8>, String> {
        let deadline = std::time::Instant::now() + crate::readback::TIMEOUT;
        let (tx, rx) = std::sync::mpsc::channel();
        self.map_readback(buffer, row, stride, height, move |r| {
            let _ = tx.send(r);
        });
        loop {
            self.readbacks.check()?;
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                let error = "GPU readback receive timed out; context retired".to_string();
                self.readbacks.retire(error.clone());
                return Err(error);
            }
            // Block until the GPU is done (bounded by the deadline: a timeout retires the
            // context) rather than polling and sleeping, which adds latency to every readback.
            self.poll_readbacks(wgpu::PollType::Wait { submission_index: None, timeout: Some(remaining) })?;
            // The callback may be running on another thread that polled the same device.
            match rx.recv_timeout(remaining.min(std::time::Duration::from_millis(1))) {
                Ok(result) => return result,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                Err(e) => return Err(format!("GPU readback completion channel: {e}")),
            }
        }
    }

    #[cfg(target_arch = "wasm32")]
    pub(crate) fn read_buffer(&self, _buffer: &wgpu::Buffer, _row: usize, _stride: usize, _height: usize) -> Result<Vec<u8>, String> {
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
            // Match eframe's backend override so CLI and headless acceptance tests
            // exercise the backend requested for the native viewer.
            desc.backends = wgpu::Backends::from_env().unwrap_or(desc.backends);
        }
        #[cfg(target_arch = "wasm32")]
        {
            desc.backends = wgpu::Backends::BROWSER_WEBGPU;
        }
        let instance = wgpu::Instance::new(desc);
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions { power_preference: wgpu::PowerPreference::HighPerformance, ..Default::default() })
            .await
            .map_err(|e| format!("no adapter: {e}"))?;
        let limits = adapter.limits();
        let required_limits = wgpu::Limits {
            max_texture_dimension_2d: limits.max_texture_dimension_2d.min(16384),
            max_buffer_size: limits.max_buffer_size,
            ..wgpu::Limits::default()
        };
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor { label: Some("effectcraft gpu"), required_limits, ..Default::default() })
            .await
            .map_err(|e| format!("no device: {e}"))?;
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
    /// Effect kernels the device couldn't build, with why: their effects render on the CPU.
    pub fn unavailable_kernels(&self) -> &[(&'static str, String)] {
        &self.unavailable
    }

    /// Leave kernel `entry` out, as if it had failed to build (tests of the CPU fallback).
    #[cfg(test)]
    pub(crate) fn drop_kernel(&mut self, entry: &'static str) {
        self.pipelines.remove(entry);
        self.unavailable.push((entry, "dropped by a test".into()));
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
        self.device.create_texture(&wgpu::TextureDescriptor {
            label: None,
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
        let reused = self.pool.lock().ok().and_then(|mut p| p.take_owned(owner, w, h).or_else(|| p.take(w, h)));
        let was_reused = reused.is_some();
        let texture = reused.unwrap_or_else(|| self.texture(w, h));
        let lease = Arc::new(Lease { texture: texture.clone(), pool: Arc::downgrade(&self.pool), owner, escaped: Default::default() });
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
    /// for [`STALE_FRAMES`] frames are evicted to make room for others). Also maintains the
    /// device without waiting, so textures evicted from the pool are freed and stop counting
    /// against its budget ([`Pool::dying`]) once their submissions are done.
    pub fn pool_frame_end(&self) {
        let dying = self.pool.lock().map(|mut p| p.frame_end()).unwrap_or_default();
        if dying {
            // A work-done callback waits for a submission; an empty one closes the wait.
            self.queue.submit([]);
        }
        let _ = self.poll_readbacks(wgpu::PollType::Poll);
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
        let g = self.image(img.width, img.height);
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo { texture: &g.texture, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            bytemuck::cast_slice(&img.data),
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(img.width * 16), rows_per_image: Some(img.height) },
            wgpu::Extent3d { width: img.width, height: img.height, depth_or_array_layers: 1 },
        );
        Some(g)
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
        if let Ok(mut p) = self.pool.lock() {
            p.clear();
        }
        if let Ok(mut s) = self.staging.lock() {
            s.clear();
        }
    }
}

/// Records GPU work for one render (a command encoder submitted at readback or the end).
pub(crate) struct Enc<'g> {
    pub g: &'g GpuContext,
    /// The open command encoder and its id in the texture pool.
    enc: Option<(wgpu::CommandEncoder, u64)>,
    pending: usize,
    /// A dispatch named a kernel the device doesn't have (see [`Enc::take_missing`]).
    missing: bool,
    /// This `Enc`'s id in the texture pool (textures it may reuse at once).
    owner: u64,
    /// `owner` is lent ([`Enc::lent`]) and outlives this `Enc`.
    lent: bool,
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
        Enc { g, enc: None, pending: 0, missing: false, owner, lent: false }
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
        Enc { g, enc: None, pending: 0, missing: false, owner, lent: true }
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
    pub fn dispatch(
        &mut self,
        entry: &str,
        p: &Params,
        src: &GpuImage,
        aux: Option<&GpuImage>,
        out: &GpuImage,
        data: Option<&wgpu::Buffer>,
        groups: (u32, u32),
    ) {
        self.dispatch_ext(entry, p, src, aux, out, data, groups, None);
    }

    /// [`Enc::dispatch`] for the kernels in `EXT_ENTRIES`, with their group 1 storage buffers.
    #[allow(clippy::too_many_arguments)]
    pub fn dispatch_ext(
        &mut self,
        entry: &str,
        p: &Params,
        src: &GpuImage,
        aux: Option<&GpuImage>,
        out: &GpuImage,
        data: Option<&wgpu::Buffer>,
        groups: (u32, u32),
        ext: Option<[&wgpu::Buffer; 4]>,
    ) {
        let g = self.g;
        let Some(pipe) = g.pipelines.get(entry) else {
            if !g.unavailable.iter().any(|(e, _)| *e == entry) {
                log::error!("gpu: no kernel {entry}");
            }
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
        self.dispatch(entry, p, src, aux, out, data, groups);
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

    /// Submit recorded work.
    pub fn submit(&mut self) {
        if let Some((enc, id)) = self.enc.take() {
            self.g.queue.submit([enc.finish()]);
            if let Ok(mut p) = self.g.pool.lock() {
                p.close(id);
            }
        }
        self.pending = 0;
    }

    /// Read an image back to the CPU (submits and waits). `None` where readback is impossible.
    pub fn download(&mut self, img: &GpuImage) -> Option<Image> {
        self.uses(img);
        let layout = crate::readback::Layout::new(img.width, img.height, 16, self.g.device.limits().max_buffer_size)
            .map_err(|e| log::error!("gpu image readback: {e}"))
            .ok()?;
        let mut data = Vec::new();
        data.try_reserve_exact(layout.len / 16).map_err(|e| log::error!("gpu image allocation: {e}")).ok()?;
        data.resize(layout.len / 16, [0.0; 4]);
        let mut out = Image { width: img.width, height: img.height, data };
        let dst: &mut [u8] = bytemuck::cast_slice_mut(&mut out.data);
        self.read_texture_into(&img.texture, img.width, img.height, 16, dst)?;
        Some(out)
    }

    /// Tightly packed texel bytes of a texture (`bpp` bytes per pixel).
    pub fn read_texture(&mut self, texture: &wgpu::Texture, w: u32, h: u32, bpp: u32) -> Option<Vec<u8>> {
        let layout =
            crate::readback::Layout::new(w, h, bpp, self.g.device.limits().max_buffer_size).map_err(|e| log::error!("gpu texture readback: {e}")).ok()?;
        let mut out = Vec::new();
        out.try_reserve_exact(layout.len).map_err(|e| log::error!("gpu readback allocation: {e}")).ok()?;
        out.resize(layout.len, 0);
        self.read_texture_into(texture, w, h, bpp, &mut out)?;
        Some(out)
    }

    /// [`Enc::read_texture`] into `out` (`w × bpp × h` bytes), through a reused staging buffer.
    fn read_texture_into(&mut self, texture: &wgpu::Texture, w: u32, h: u32, bpp: u32, out: &mut [u8]) -> Option<()> {
        if !self.g.can_wait() {
            return None;
        }
        self.g.transfers.count(false, out.len());
        let layout =
            crate::readback::Layout::new(w, h, bpp, self.g.device.limits().max_buffer_size).map_err(|e| log::error!("gpu texture readback: {e}")).ok()?;
        if out.len() != layout.len {
            log::error!("gpu readback destination size mismatch");
            return None;
        }
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
            wgpu::TexelCopyTextureInfo { texture, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            wgpu::TexelCopyBufferInfo { buffer: &buf, layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(padded), rows_per_image: Some(h) } },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
        self.submit();
        let bytes = self.g.read_buffer(&buf, row, padded as usize, h as usize).map_err(|e| log::error!("gpu texture readback: {e}")).ok()?;
        if bytes.len() != out.len() {
            log::error!("gpu texture readback size mismatch");
            return None;
        }
        out.copy_from_slice(&bytes);
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
        Some(())
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
