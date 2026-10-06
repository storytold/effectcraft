//! The EffectCraft GPU compositor (Mercury GPU Acceleration's counterpart), on wgpu compute
//! shaders: Metal, Vulkan, Direct3D 12 and WebGPU.
//!
//! The CPU [`Renderer`] stays the reference and keeps rendering layer *content* (sources, masks,
//! CPU effects, layer styles) into its layer cache. [`Gpu`] composites: it uploads the cached
//! layer buffers (once per buffer), applies the layer transforms with the CPU's sampling
//! (nearest / bilinear / bicubic, minification pre-filter), motion-blur sub-samples, track
//! mattes, Preserve Transparency, layer styles' passes, all 38 blend modes and the 8/16 bpc
//! clamping and quantisation, and converts colour spaces. Classic 3D runs composite here too
//! (`classic3d`: per-pixel fragment sort, lights, ray-cast shadows), and adjustment layers run
//! their effect stacks on the GPU-resident comp. Wireframe-quality outlines are drawn here too.
//!
//! Advanced 3D comps rasterise on a render pipeline (`advanced3d.wgsl`: depth buffer, PBR,
//! image-based light, shadow maps) and finish in compute kernels (`adv3d.wgsl`: supersampling
//! resolve, motion-blur sub-samples, iris depth of field, encoding, compositing), see
//! [`Accelerator::render_3d`] and `Renderer::prepare_adv_run`. Runs with layers on the 2D
//! compositing path (blend modes, track mattes, Preserve Transparency:
//! `Renderer::split_adv_run`) render each such layer alone, hide it behind the nearer main
//! scene and composite it through the 2D path here too; environment backgrounds (Classic and
//! Advanced 3D) draw their sky in a kernel (`Renderer::sky_draw`).
//!
//! GPU effects ([`effectcraft_effects::GPU_EFFECTS`]) run as compute kernels with the CPU
//! effect's exact steps (padding, box-blur radii, parameter conversions); chains of them are
//! uploaded and read back once. Each family lives in its own module with its own WGSL file
//! (`fx_color`, `fx_depth`, `fx_distort`, `fx_extra`, `fx_gen2`, `fx_generate`, `fx_key`,
//! `fx_light`, `fx_lut`, `fx_noise`, `fx_particles`, `fx_pixel2`, `fx_sim`, `fx_stylize`,
//! `fx_text`, `fx_time`, `fx_tone`, `fx_transition`, `fx_vr`, `fx_warp`); settings a kernel
//! cannot match fall back to the CPU (`catalog::gpu_supported`, or `None` from the family's
//! `apply`).
//! The 3D Channel effects read the layer's aux channels as an extra texture (`fx_depth`), the
//! colour-management effects interpret colour programs built next to the CPU effects
//! (`fx_lut`), simulations keep their state on the CPU and rasterise its per-frame plan here
//! (`fx_sim`, `fx_particles`), and so do the geometry generators (`fx_gen2`: bolts, strokes,
//! glyphs, audio marks, waves).
//!
//! Working textures come from a pool (`context::Pool`): a frame allocates several full-frame
//! RGBA f32 images per layer, and creating and zeroing those cost more than compositing a small
//! layer. A released texture is reused once every encoder that could still read it has been
//! submitted. 8/16 bpc quantisation after each layer is fused into that layer's composite.
//! [`Backend::Auto`](effectcraft_render::Backend) renders each comp on whichever compositor
//! measured faster for it ([`effectcraft_render::AutoPick`], kept by [`Gpu`]): a light comp
//! that the CPU composites in a millisecond stays there rather than paying for a full-frame
//! readback.
//!
//! GPU particles (`particles`): the stepped particle effects hand their simulation to
//! [`effectcraft_effects::psim::ParticleSim`], implemented here with one invocation per particle
//! and GPU-resident checkpoints.
//!
//! Plug a [`Gpu`] into [`Renderer::accel`] (it implements [`Accelerator`]); renders then use it
//! when [`RenderOpts::backend`](effectcraft_render::RenderOpts) asks for it. The viewer can
//! skip readback entirely with [`Gpu::render_display`], which leaves an RGBA8 texture for
//! egui-wgpu to draw.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

mod adv3d;
mod bokeh;
mod classic3d;
mod context;
pub mod deferred;
mod effects;
mod fx_color;
mod fx_depth;
mod fx_distort;
mod fx_extra;
mod fx_gen2;
mod fx_generate;
mod fx_key;
mod fx_light;
mod fx_lut;
mod fx_noise;
mod fx_particles;
mod fx_pixel2;
mod fx_sim;
mod fx_stylize;
mod fx_text;
mod fx_time;
mod fx_tone;
mod fx_transition;
mod fx_vr;
mod fx_warp;
mod ops;
mod particles;
mod walk;

use std::sync::Arc;

pub use context::{GpuContext, GpuImage, TransferStats};
use effectcraft_effects::Buf;
use effectcraft_project::ItemId;
use effectcraft_raster::Image;
use effectcraft_render::{Accelerator, FxStep, Renderer};
use effectcraft_time::Tick;
pub use wgpu;

use crate::context::Enc;

/// The GPU compositor (cheap to clone; one device shared by all clones).
#[derive(Clone)]
pub struct Gpu {
    ctx: Arc<GpuContext>,
    /// Backend::Auto's per-comp CPU / GPU timings.
    auto: Arc<effectcraft_render::AutoPick>,
}

/// A viewer frame left on the GPU: premultiplied RGBA8 (`wgpu::TextureFormat::Rgba8Unorm`),
/// ready for `egui_wgpu::Renderer::register_native_texture`.
#[derive(Clone, Debug)]
pub struct DisplayFrame {
    pub texture: wgpu::Texture,
    pub width: u32,
    pub height: u32,
}

impl Gpu {
    /// On a device of its own (CLI, tests, benchmarks); `None` without a usable adapter.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn headless() -> Option<Gpu> {
        GpuContext::headless().map(Gpu::from_context)
    }

    /// On an existing device (the desktop app shares egui-wgpu's).
    pub fn new(adapter: &wgpu::Adapter, device: wgpu::Device, queue: wgpu::Queue) -> Result<Gpu, String> {
        GpuContext::new(adapter, device, queue).map(Gpu::from_context)
    }

    pub fn from_context(ctx: GpuContext) -> Gpu {
        Gpu { ctx: Arc::new(ctx), auto: Default::default() }
    }

    /// A device of its own without blocking, with deferred readbacks (a browser worker, see
    /// [`deferred`]); natively the same device works too (tests). `Err` says why there is none.
    pub async fn request_deferred() -> Result<Gpu, String> {
        let mut ctx = GpuContext::request().await?;
        ctx.set_deferred(true);
        Ok(Gpu::from_context(ctx))
    }

    /// [`Gpu::request_deferred`] blocking (tests of the browser worker's path, natively).
    #[cfg(not(target_arch = "wasm32"))]
    pub fn headless_deferred() -> Option<Gpu> {
        pollster::block_on(Gpu::request_deferred()).map_err(|e| log::info!("gpu: {e}")).ok()
    }

    /// The deferred readback state, when readbacks are deferred.
    pub fn deferred(&self) -> Option<&Arc<deferred::Deferred>> {
        self.ctx.deferred()
    }

    /// Resolves once no deferred readback is in flight (immediately without deferred readbacks).
    pub async fn settled(&self) {
        if let Some(d) = self.ctx.deferred() {
            d.settled().await;
        }
    }

    /// Deliver finished readbacks without blocking (native; the browser delivers them itself).
    pub fn poll(&self) {
        #[cfg(not(target_arch = "wasm32"))]
        let _ = self.ctx.device.poll(wgpu::PollType::Poll);
    }

    /// `{passes, readbacks, inFlight}` of deferred rendering (diagnostics).
    pub fn deferred_stats(&self) -> Option<(u64, u64, usize)> {
        let d = self.ctx.deferred()?;
        Some((d.passes.load(std::sync::atomic::Ordering::Relaxed), d.readbacks.load(std::sync::atomic::Ordering::Relaxed), d.in_flight()))
    }

    pub fn context(&self) -> &GpuContext {
        &self.ctx
    }

    pub fn device(&self) -> &wgpu::Device {
        &self.ctx.device
    }

    /// Render a top-level comp frame and leave it on the GPU as a display texture (no
    /// readback unless a CPU fallback step needs one). `None` = render on the CPU.
    pub fn render_display(&self, r: &Renderer, comp: ItemId, t: Tick) -> Option<DisplayFrame> {
        let mut e = Enc::new(&self.ctx);
        let img = walk::render(&mut e, r, comp, t)?;
        let texture = e.display(&img);
        e.submit();
        Some(DisplayFrame { texture, width: img.width, height: img.height })
    }

    /// Read a display frame back as premultiplied RGBA8 (Info panel sampling, eyedroppers).
    pub fn read_display(&self, f: &DisplayFrame) -> Option<Vec<u8>> {
        Enc::new(&self.ctx).read_texture(&f.texture, f.width, f.height, 4)
    }

    /// [`Gpu::read_display`] without blocking (the browser's main thread): `done` gets the bytes
    /// once the GPU has them.
    pub fn read_display_async(&self, f: &DisplayFrame, done: impl FnOnce(Option<Vec<u8>>) + wgpu::WasmNotSend + 'static) {
        Enc::new(&self.ctx).read_texture_async(&f.texture, f.width, f.height, 4, done);
    }

    /// Render a frame on the GPU and read it back (`None` = not handled).
    pub fn render(&self, r: &Renderer, comp: ItemId, t: Tick) -> Option<Image> {
        if !self.ctx.can_readback() {
            return None;
        }
        if let Some(d) = self.ctx.deferred.clone() {
            return self.render_deferred(&d, r, comp, t);
        }
        let mut e = Enc::new(&self.ctx);
        let img = walk::render(&mut e, r, comp, t)?;
        e.download(&img)
    }

    /// [`Gpu::render`] with deferred readbacks: the frame's readback is keyed by comp, time and
    /// render options; until it arrives (or while layer steps still miss) the pass gets a
    /// placeholder.
    fn render_deferred(&self, d: &Arc<deferred::Deferred>, r: &Renderer, comp: ItemId, t: Tick) -> Option<Image> {
        let mut k = deferred::Key::new(2);
        k.u64(comp.0);
        k.u64(t.0 as u64);
        k.debug(&r.opts);
        let key = k.finish();
        let image = |rb: deferred::Readback| -> Option<Image> {
            let mut img = Image::new(rb.width, rb.height);
            let dst: &mut [u8] = bytemuck::cast_slice_mut(&mut img.data);
            (dst.len() == rb.bytes.len()).then(|| dst.copy_from_slice(&rb.bytes))?;
            Some(img)
        };
        match d.lookup(key) {
            deferred::Lookup::Ready(rb) => return rb.and_then(image),
            deferred::Lookup::InFlight => {
                d.miss();
                return Some(Image::new(1, 1));
            }
            deferred::Lookup::Absent => {}
        }
        let misses = d.misses();
        let mut e = Enc::new(&self.ctx);
        let img = walk::render(&mut e, r, comp, t)?;
        if d.misses() != misses {
            // Layer steps are still in flight: this pass's canvas is not the frame.
            return Some(Image::new(1, 1));
        }
        match e.read_keyed(&img, key, [0.0; 2], 1.0) {
            Some(rb) => rb.and_then(image),
            None => Some(Image::new(1, 1)),
        }
    }

    /// Run GPU work `f` on this thread, catching the device running out of video memory
    /// meanwhile (#106). `Err` (why) means the result is incomplete: a texture that failed to
    /// allocate holds nothing. The uploaded layers and pooled textures are then freed, so the
    /// next render has room. The browser reports it to the device's error handler instead.
    pub fn within_memory<T>(&self, f: impl FnOnce() -> T) -> Result<T, String> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let scope = self.ctx.device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
            let out = f();
            match pollster::block_on(scope.pop()) {
                Some(e) => {
                    self.ctx.clear_uploads();
                    Err(e.to_string())
                }
                None => Ok(out),
            }
        }
        #[cfg(target_arch = "wasm32")]
        Ok(f())
    }

    /// Wait for all submitted GPU work (benchmarks).
    pub fn wait(&self) {
        let _ = self.ctx.device.poll(wgpu::PollType::wait_indefinitely());
    }
}

impl Accelerator for Gpu {
    fn name(&self) -> String {
        self.ctx.name.clone()
    }

    fn comp_frame(&self, r: &Renderer, comp: ItemId, t: Tick) -> Option<Image> {
        self.render(r, comp, t)
    }

    fn supports_effect(&self, id: &str) -> bool {
        self.ctx.can_readback() && effects::supports(id)
    }

    fn effects(&self, chain: &[FxStep], buf: &Buf, levels: Option<f32>) -> Option<Buf> {
        effects::run_chain(&mut Enc::new(&self.ctx), chain, buf, levels)
    }

    fn raster_3d(&self, scene: &effectcraft_render::three_d::adv::Scene) -> Option<effectcraft_render::three_d::adv::Target> {
        adv3d::render(&self.ctx, scene)
    }

    fn render_3d(&self, run: &effectcraft_render::three_d::adv::Prepared) -> Option<effectcraft_render::three_d::adv::Rendered> {
        adv3d::render_prepared(&self.ctx, run)
    }

    fn particles(&self) -> Option<&dyn effectcraft_effects::psim::ParticleSim> {
        self.ctx.can_readback().then_some(self as &dyn effectcraft_effects::psim::ParticleSim)
    }

    fn auto_pick(&self) -> Option<&effectcraft_render::AutoPick> {
        Some(&self.auto)
    }

    fn frame_begin(&self) {
        if let Some(d) = self.ctx.deferred() {
            d.frame_begin();
        }
    }

    fn pass_begin(&self) {
        if let Some(d) = self.ctx.deferred() {
            d.pass_begin();
        }
    }

    fn pass_missed(&self) -> bool {
        self.ctx.deferred().is_some_and(|d| d.missed())
    }

    fn miss_gate(&self) -> Option<Arc<std::sync::atomic::AtomicBool>> {
        self.ctx.deferred().map(|d| d.gate())
    }

    fn settle(&self) -> Option<std::pin::Pin<Box<dyn std::future::Future<Output = ()> + '_>>> {
        let d = self.ctx.deferred()?.clone();
        // Natively the device delivers the readbacks when polled: wait for them here.
        #[cfg(not(target_arch = "wasm32"))]
        if d.in_flight() > 0 {
            let _ = self.ctx.device.poll(wgpu::PollType::wait_indefinitely());
        }
        Some(Box::pin(async move { d.settled().await }))
    }
}

impl effectcraft_effects::psim::ParticleSim for Gpu {
    fn simulate(&self, req: &effectcraft_effects::psim::SimRequest) -> Option<Vec<effectcraft_effects::psim::SimParticle>> {
        particles::simulate(&self.ctx, req)
    }
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_3d;
#[cfg(test)]
mod tests_adjust;
#[cfg(test)]
mod tests_adv3d;
#[cfg(test)]
mod tests_deferred;
#[cfg(test)]
mod tests_deferred_jobs;
#[cfg(test)]
mod tests_fx_color;
#[cfg(test)]
mod tests_fx_depth;
#[cfg(test)]
mod tests_fx_distort;
#[cfg(test)]
mod tests_fx_extra;
#[cfg(test)]
mod tests_fx_gen2;
#[cfg(test)]
mod tests_fx_generate;
#[cfg(test)]
mod tests_fx_key;
#[cfg(test)]
mod tests_fx_light;
#[cfg(test)]
mod tests_fx_lut;
#[cfg(test)]
mod tests_fx_noise;
#[cfg(test)]
mod tests_fx_particles;
#[cfg(test)]
mod tests_fx_pixel2;
#[cfg(test)]
mod tests_fx_sim;
#[cfg(test)]
mod tests_fx_stylize;
#[cfg(test)]
mod tests_fx_text;
#[cfg(test)]
mod tests_fx_time;
#[cfg(test)]
mod tests_fx_tone;
#[cfg(test)]
mod tests_fx_transition;
#[cfg(test)]
mod tests_fx_vr;
#[cfg(test)]
mod tests_fx_warp;
#[cfg(test)]
mod tests_particles;
#[cfg(all(test, target_os = "windows", target_arch = "x86_64", target_env = "msvc"))]
mod tests_static_dxc;
