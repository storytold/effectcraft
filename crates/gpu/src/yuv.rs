//! Export frames converted on the device to the H.264 encoder's input (8-bit BT.709 limited
//! range 4:2:0, see `shaders/yuv.wgsl`): a 4K frame reads back 12 MB of planes instead of
//! 133 MB of RGBA f32, and the CPU skips `to_rgba8_over` + `rgba_to_yuv420`.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use wgpu::util::DeviceExt;

use crate::context::{Enc, GpuImage};

pub(crate) struct YuvPipe {
    bgl: wgpu::BindGroupLayout,
    pipe: wgpu::ComputePipeline,
    /// Free staging buffers: each frame takes one and gets it back once it is unmapped.
    staging: Arc<Mutex<Vec<wgpu::Buffer>>>,
    /// Asynchronous frames whose readback hasn't finished ([`IN_FLIGHT`]).
    in_flight: Arc<AtomicUsize>,
}

/// Free staging buffers kept for reuse.
const RING: usize = 4;
/// Asynchronous frames in flight at most: each holds its rendered frame's textures until it
/// is submitted and its staging buffer until it is read (about 12 MB at 4K); past this the
/// next frame waits for the oldest readback.
/// Kept equal to the staging ring (`RING`) so a full set of in-flight frames never has to
/// allocate a fresh staging buffer.
pub(crate) const IN_FLIGHT: usize = RING;

impl YuvPipe {
    pub(crate) fn new(device: &wgpu::Device) -> YuvPipe {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("yuv420"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/yuv.wgsl").into()),
        });
        let storage =
            wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only: false }, has_dynamic_offset: false, min_binding_size: None };
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("yuv420"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry { binding: 2, visibility: wgpu::ShaderStages::COMPUTE, ty: storage, count: None },
            ],
        });
        let layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("yuv420"), bind_group_layouts: &[Some(&bgl)], immediate_size: 0 });
        let pipe = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("yuv420"),
            layout: Some(&layout),
            module: &module,
            entry_point: Some("yuv420"),
            compilation_options: Default::default(),
            cache: None,
        });
        YuvPipe { bgl, pipe, staging: Arc::new(Mutex::new(vec![])), in_flight: Arc::new(AtomicUsize::new(0)) }
    }
}

impl YuvPipe {
    fn take(&self, g: &crate::context::GpuContext, size: u64) -> wgpu::Buffer {
        let found = self.staging.lock().ok().and_then(|mut r| r.iter().position(|b| b.size() == size).map(|i| r.swap_remove(i)));
        found.unwrap_or_else(|| {
            g.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("yuv420 staging"),
                size,
                usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        })
    }

    /// Return an unmapped staging buffer to the ring.
    pub(crate) fn give(&self, b: wgpu::Buffer) {
        give(&self.staging, b);
    }

    #[cfg(test)]
    pub(crate) fn in_flight(&self) -> usize {
        self.in_flight.load(Ordering::SeqCst)
    }

    /// Reserve one of the [`IN_FLIGHT`] slots, or `None` when all are taken. The slot is
    /// freed when the returned guard drops (a declined frame) or its readback ends.
    pub(crate) fn reserve(&self) -> Option<Reserved> {
        self.in_flight.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| (n < IN_FLIGHT).then_some(n + 1)).ok()?;
        Some(Reserved(self.in_flight.clone()))
    }

    #[cfg(test)]
    pub(crate) fn kept(&self) -> usize {
        self.staging.lock().map(|r| r.len()).unwrap_or(0)
    }
}

/// An asynchronous frame's slot ([`YuvPipe::reserve`]), freed on drop.
pub(crate) struct Reserved(Arc<AtomicUsize>);

impl Drop for Reserved {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

fn give(ring: &Mutex<Vec<wgpu::Buffer>>, b: wgpu::Buffer) {
    if let Ok(mut r) = ring.lock()
        && r.len() < RING
    {
        r.push(b);
    }
}

/// The frame sizes the kernel handles: width a multiple of 8 (4 chroma samples per
/// invocation), height even.
pub(crate) fn supported(w: u32, h: u32) -> bool {
    w > 0 && h > 0 && w.is_multiple_of(8) && h.is_multiple_of(2)
}

/// Bytes of the planes: Y `w×h`, then U and V `w/2 × h/2`.
pub(crate) fn planes_len(w: u32, h: u32) -> u64 {
    w as u64 * h as u64 + 2 * ((w / 2) as u64 * (h / 2) as u64)
}

/// Record the conversion of `img` over `bg` into a `MAP_READ` staging buffer from the ring of
/// [`planes_len`] bytes and submit it. `None`: the size is not [`supported`].
pub(crate) fn record(e: &mut Enc, yp: &YuvPipe, img: &GpuImage, bg: [f32; 3]) -> Option<wgpu::Buffer> {
    let (w, h) = (img.width, img.height);
    if !supported(w, h) {
        return None;
    }
    let g = e.g;
    let cw = w / 2;
    let size = planes_len(w, h);
    let out = g.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("yuv420 out"),
        size,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let stg = yp.take(g, size);
    let mut params = [0u8; 32];
    // w * h fits in u32: both are at most the texture limit (16384).
    for (i, v) in [w, h, cw, w * h / 4].into_iter().enumerate() {
        params[i * 4..i * 4 + 4].copy_from_slice(&v.to_le_bytes());
    }
    for (i, v) in [bg[0], bg[1], bg[2], 1.0].into_iter().enumerate() {
        params[16 + i * 4..16 + i * 4 + 4].copy_from_slice(&v.to_le_bytes());
    }
    let ub =
        g.device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some("yuv420 params"), contents: &params, usage: wgpu::BufferUsages::UNIFORM });
    let view = img.texture.create_view(&Default::default());
    let group = g.device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("yuv420"),
        layout: &yp.bgl,
        entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: ub.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&view) },
            wgpu::BindGroupEntry { binding: 2, resource: out.as_entire_binding() },
        ],
    });
    {
        let enc = e.encoder();
        {
            let mut pass = enc.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("yuv420"), timestamp_writes: None });
            pass.set_pipeline(&yp.pipe);
            pass.set_bind_group(0, &group, &[]);
            pass.dispatch_workgroups((w / 8).div_ceil(8), (h / 2).div_ceil(8), 1);
        }
        enc.copy_buffer_to_buffer(&out, 0, &stg, 0, size);
    }
    e.submit();
    Some(stg)
}

/// Convert `img` over `bg` and read the planes back, waiting for the device.
pub(crate) fn convert(e: &mut Enc, yp: &YuvPipe, img: &GpuImage, bg: [f32; 3]) -> Option<Vec<u8>> {
    let stg = record(e, yp, img, bg)?;
    let n = planes_len(img.width, img.height) as usize;
    let r = e.g.read_buffer(&stg, n, n, 1).map_err(|err| log::error!(target: "effectcraft_gpu", "yuv420 readback: {err}")).ok();
    // `read_buffer` returns once the map settled or the context retired; a buffer that may
    // still be mapped or pending is not reused.
    if r.is_some() {
        yp.give(stg);
    }
    r
}

/// Map `stg` (`n` bytes, submitted by [`record`]) without waiting, holding `slot` until the
/// readback ends; the planes go to `done` from a later device poll, through the context's bounded readbacks (deadline, failure on
/// device loss), exactly once. `Err(done)`: the readback could not start, `done` untouched.
/// The buffer returns to the ring only once it is unmapped.
pub(crate) fn read_async(
    g: &crate::context::GpuContext,
    yp: &YuvPipe,
    stg: wgpu::Buffer,
    n: usize,
    slot: Reserved,
    done: effectcraft_render::YuvDone,
) -> Result<(), effectcraft_render::YuvDone> {
    let ring = yp.staging.clone();
    let b = stg.clone();
    // Kept reclaimable so a readback that can't start hands the caller's callback back.
    let cb = Arc::new(Mutex::new(Some((done, slot))));
    let c2 = cb.clone();
    let started = g.try_map_readback(
        &stg,
        n,
        n,
        1,
        move |r: Result<Vec<u8>, String>| {
            let taken = c2.lock().ok().and_then(|mut d| d.take());
            if let Some((d, slot)) = taken {
                // The slot frees before `done` runs: a `done` that submits again finds room.
                drop(slot);
                d(r.map_err(|e| log::error!(target: "effectcraft_gpu", "yuv420 readback: {e}")).ok());
            }
        },
        move || give(&ring, b),
    );
    match started {
        Ok(()) => Ok(()),
        Err((_, e)) => {
            log::warn!(target: "effectcraft_gpu", "yuv420 readback not started: {e}");
            let taken = cb.lock().ok().and_then(|mut d| d.take());
            taken.map_or(Ok(()), |(d, _slot)| Err(d))
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use effectcraft_raster::Image;

    /// The export's conversion (`to_rgba8_over` + `rgba_to_yuv420`), the reference the kernel
    /// is held to.
    pub(crate) fn cpu(img: &Image, bg: [f32; 3]) -> Vec<u8> {
        effectcraft_raster::yuv::yuv420_over(img, bg)
    }

    /// Premultiplied noise with some out-of-range values, opaque pixels, exact 8-bit levels and
    /// a few NaN and infinite components (the CPU writes 0 for NaN).
    pub(crate) fn noise(w: u32, h: u32, seed: u32) -> Image {
        let mut img = Image::new(w, h);
        let mut s = seed | 1;
        let mut rnd = || {
            s ^= s << 13;
            s ^= s >> 17;
            s ^= s << 5;
            s as f32 / u32::MAX as f32
        };
        for (i, p) in img.data.iter_mut().enumerate() {
            let a = if i % 7 == 0 { 1.0 } else { rnd() };
            *p = if i % 97 == 3 {
                [f32::NAN, 0.5, f32::INFINITY, a]
            } else if i % 101 == 5 {
                [0.2, 0.3, 0.4, f32::NAN]
            } else if i % 103 == 7 {
                [f32::NEG_INFINITY, 0.1, 0.9, f32::INFINITY]
            } else if i % 5 == 0 {
                let q = (i % 256) as f32 / 255.0;
                [q, 1.0 - q, q * 0.5, 1.0]
            } else {
                [rnd() * a * 1.1 - 0.02, rnd() * a, rnd() * a, a]
            };
        }
        img
    }

    /// The same bytes (the same kernel on the same device).
    pub(crate) fn assert_close_exact(got: &[u8], want: &[u8], what: &str) {
        assert!(got == want, "{what}: differs");
    }

    /// Check `got` against the CPU's `want`. Software adapters (llvmpipe, WARP) must match
    /// exactly. A hardware driver may contract multiply-adds or divide less exactly, which
    /// moves a value near a rounding boundary by one level (seen on an RTX 3090 under Vulkan):
    /// there every byte must be within one level and at most `budget` of them off (about 2–3×
    /// the rate measured on the 3090). A wrong formula, rounding mode or NaN rule fails either
    /// way on software adapters.
    pub(crate) fn assert_parity(g: &crate::Gpu, got: &[u8], want: &[u8], budget: f64, what: &str) {
        assert_eq!(got.len(), want.len(), "{what}: length");
        let diff: Vec<usize> = (0..got.len()).filter(|&i| got[i] != want[i]).collect();
        let max = diff.iter().map(|&i| (got[i] as i32 - want[i] as i32).abs()).max().unwrap_or(0);
        eprintln!("{what} on {}: {} of {} bytes differ (max {max})", g.ctx.name, diff.len(), got.len());
        if g.ctx.software {
            assert!(diff.is_empty(), "{what}: a software adapter must match exactly, first diffs at {:?}", &diff[..diff.len().min(10)]);
            return;
        }
        assert!(max <= 1, "{what}: a byte is {max} levels off, first diffs at {:?}", &diff[..diff.len().min(10)]);
        assert!(diff.len() as f64 <= budget * got.len() as f64, "{what}: {} of {} bytes off by one", diff.len(), got.len());
    }

    #[test]
    fn yuv420_matches_cpu() {
        let Some(g) = crate::tests::gpu() else { return };
        let yp = super::YuvPipe::new(&g.ctx.device);
        for (w, h, bg) in [(264u32, 150u32, [0.05, 0.1, 0.2]), (8, 2, [1.0, 1.0, 1.0]), (1928, 6, [0.0, 0.0, 0.0])] {
            let img = noise(w, h, w ^ (h << 16));
            let src = g.ctx.upload_image(&img).expect("upload");
            let got = super::convert(&mut crate::context::Enc::new(&g.ctx), &yp, &src, bg).expect("readback");
            assert_parity(g, &got, &cpu(&img, bg), 0.00005, &format!("noise {w}x{h}"));
        }
    }

    #[test]
    fn unsupported_sizes_are_declined() {
        assert!(super::supported(8, 2));
        for (w, h) in [(0, 2), (8, 0), (12, 2), (8, 3), (7, 4)] {
            assert!(!super::supported(w, h), "{w}x{h}");
        }
        assert_eq!(super::planes_len(8, 2), 16 + 2 * 4);
    }

    #[test]
    fn unsupported_sizes_are_not_converted() {
        let Some(g) = crate::tests::gpu() else { return };
        let yp = super::YuvPipe::new(&g.ctx.device);
        let src = g.ctx.upload_image(&noise(12, 4, 3)).expect("upload");
        assert!(super::convert(&mut crate::context::Enc::new(&g.ctx), &yp, &src, [0.0; 3]).is_none());
    }
}

#[cfg(test)]
mod accel_tests {
    use effectcraft_color::Label;
    use effectcraft_project::{BitDepth, Comp, ItemKind};
    use effectcraft_render::{Accelerator, Backend, RenderOpts, Renderer};
    use effectcraft_time::{FrameRate, Tick};

    use crate::tests::{Pattern, Scene, opts};

    pub(super) fn scene(w: u32, h: u32) -> Scene {
        let mut s = Scene::new(BitDepth::Bpc8);
        let comp = Comp::new(w, h, FrameRate::FPS_30, Tick::from_seconds_f64(2.0));
        s.cid = s.p.add_item("Comp yuv", Label::Sandstone, None, ItemKind::Comp(comp.clone().into()));
        s.comp = comp;
        let l = s.footage(w + 7, h + 3);
        s.push(l);
        let sol = s.solid([0.9, 0.2, 0.4], w / 2, h / 3);
        s.push(sol);
        s
    }

    /// The accelerator's YUV frame is the export's CPU conversion of the very frame
    /// `comp_frame` returns.
    #[test]
    fn comp_frame_yuv420_is_the_cpu_conversion_of_comp_frame() {
        let Some(g) = crate::tests::gpu() else { return };
        let s = scene(96, 60);
        let mut r = Renderer::new(&s.p, &Pattern, RenderOpts { backend: Backend::Gpu, ..opts() });
        r.accel = Some(g);
        let bg = [0.2, 0.3, 0.4];
        let rgba = g.comp_frame(&r, s.cid, Tick::ZERO).expect("rgba frame");
        let yuv = g.comp_frame_yuv420(&r, s.cid, Tick::ZERO, bg).expect("yuv frame");
        super::tests::assert_parity(g, &yuv, &super::tests::cpu(&rgba, bg), 0.006, "comp frame");
    }

    #[test]
    fn odd_sized_comps_fall_back_to_the_cpu() {
        let Some(g) = crate::tests::gpu() else { return };
        let s = scene(97, 61);
        let mut r = Renderer::new(&s.p, &Pattern, RenderOpts { backend: Backend::Gpu, ..opts() });
        r.accel = Some(g);
        assert!(g.comp_frame(&r, s.cid, Tick::ZERO).is_some());
        assert!(g.comp_frame_yuv420(&r, s.cid, Tick::ZERO, [0.0; 3]).is_none());
    }
}

#[cfg(test)]
mod async_tests {
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use effectcraft_render::{Accelerator, Backend, RenderOpts, Renderer, YuvDone};
    use effectcraft_time::Tick;

    use super::accel_tests::scene;
    use crate::tests::{Pattern, opts};

    type Got = Arc<Mutex<Vec<(usize, Option<Vec<u8>>)>>>;

    fn sink(got: &Got, i: usize) -> YuvDone {
        let got = got.clone();
        Box::new(move |b| got.lock().unwrap().push((i, b)))
    }

    fn drain(g: &crate::Gpu, got: &Got, n: usize) {
        for _ in 0..400 {
            if got.lock().unwrap().len() >= n {
                return;
            }
            g.poll_readbacks(Some(Duration::from_millis(50)));
        }
    }

    fn sorted(got: &Got) -> Vec<(usize, Option<Vec<u8>>)> {
        let mut v = std::mem::take(&mut *got.lock().unwrap());
        v.sort_by_key(|(i, _)| *i);
        v
    }

    /// Frames of two comps (different sizes and content) and backgrounds, many in flight: each
    /// callback runs once with its own frame's planes, equal to the synchronous conversion,
    /// and no more than `IN_FLIGHT` are ever pending.
    #[test]
    fn frames_in_flight_each_get_their_own_planes() {
        let Some(g) = crate::tests::gpu() else { return };
        let scenes = [scene(96, 60), scene(104, 40)];
        let rs: Vec<Renderer> = scenes
            .iter()
            .map(|s| {
                let mut r = Renderer::new(&s.p, &Pattern, RenderOpts { backend: Backend::Gpu, ..opts() });
                r.accel = Some(g);
                r
            })
            .collect();
        let frames: Vec<(usize, [f32; 3])> = (0..12).map(|i| (i % 2, [i as f32 / 12.0, 1.0 - i as f32 / 12.0, 0.5])).collect();
        let got: Got = Default::default();
        for (i, &(k, bg)) in frames.iter().enumerate() {
            assert!(g.comp_frame_yuv420_async(&rs[k], scenes[k].cid, Tick::ZERO, bg, sink(&got, i)).is_ok());
            let yp = g.ctx.yuv.get().expect("pipe");
            assert!(yp.in_flight() <= super::IN_FLIGHT, "{} in flight", yp.in_flight());
        }
        drain(g, &got, frames.len());
        let got = sorted(&got);
        assert_eq!(got.iter().map(|(i, _)| *i).collect::<Vec<_>>(), (0..frames.len()).collect::<Vec<_>>(), "each callback once");
        for ((i, planes), &(k, bg)) in got.into_iter().zip(&frames) {
            let want = g.comp_frame_yuv420(&rs[k], scenes[k].cid, Tick::ZERO, bg).expect("sync frame");
            super::tests::assert_close_exact(&planes.expect("readback"), &want, &format!("frame {i}"));
        }
        let yp = g.ctx.yuv.get().expect("pipe");
        assert_eq!(yp.in_flight(), 0);
        assert!((1..=super::RING).contains(&yp.kept()), "{}", yp.kept());
    }

    #[test]
    fn declined_async_frames_hand_the_callback_back_unused() {
        let Some(g) = crate::tests::gpu() else { return };
        let s = scene(97, 61);
        let mut r = Renderer::new(&s.p, &Pattern, RenderOpts { backend: Backend::Gpu, ..opts() });
        r.accel = Some(g);
        let got: Got = Default::default();
        let back = g.comp_frame_yuv420_async(&r, s.cid, Tick::ZERO, [0.0; 3], sink(&got, 0)).expect_err("declined");
        assert!(got.lock().unwrap().is_empty());
        back(None);
        assert_eq!(got.lock().unwrap().len(), 1);
    }

    /// Submit `n` conversions of uploaded frames, then start all their readbacks with no
    /// submit or poll in between (wgpu delivers map callbacks from either), so all are pending.
    fn submit(g: &crate::Gpu, n: usize, got: &Got) {
        let yp = g.ctx.yuv.get_or_init(|| super::YuvPipe::new(&g.ctx.device));
        let stgs: Vec<wgpu::Buffer> = (0..n)
            .map(|i| {
                let src = g.ctx.upload_image(&super::tests::noise(64, 32, i as u32 + 1)).expect("upload");
                super::record(&mut crate::context::Enc::new(&g.ctx), yp, &src, [0.0; 3]).expect("record")
            })
            .collect();
        for (i, stg) in stgs.into_iter().enumerate() {
            assert!(super::read_async(&g.ctx, yp, stg, super::planes_len(64, 32) as usize, yp.reserve().expect("slot"), sink(got, i)).is_ok());
        }
    }

    /// A device failure with several readbacks pending fails each of them once (`None`, never
    /// planes) and later frames are declined.
    #[test]
    fn a_device_failure_fails_every_pending_readback_once() {
        crate::tests::hold_gpu_lock();
        let g = crate::Gpu::headless().expect("no GPU adapter");
        let got: Got = Default::default();
        submit(&g, 3, &got);
        assert!(got.lock().unwrap().is_empty(), "nothing polled yet");
        g.ctx.device_failure_notifier()("test: device lost".into());
        assert!(!g.poll_readbacks(Some(Duration::from_millis(50))));
        drain(&g, &got, 3);
        let got = sorted(&got);
        assert_eq!(got.iter().map(|(i, _)| *i).collect::<Vec<_>>(), vec![0, 1, 2], "each callback once");
        assert!(got.iter().all(|(_, p)| p.is_none()), "no planes after the failure");
        assert_eq!(g.ctx.yuv.get().map(|yp| yp.in_flight()), Some(0));
        let s = scene(96, 60);
        let mut r = Renderer::new(&s.p, &Pattern, RenderOpts { backend: Backend::Gpu, ..opts() });
        r.accel = Some(&g);
        let late: Got = Default::default();
        assert!(g.comp_frame_yuv420_async(&r, s.cid, Tick::ZERO, [0.1; 3], sink(&late, 9)).is_err());
        // A readback that can't start (the context retired) hands the callback back unused.
        let yp = g.ctx.yuv.get().expect("pipe");
        let stg = g.ctx.device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: 64,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let back = super::read_async(&g.ctx, yp, stg, 64, yp.reserve().expect("slot"), sink(&late, 10)).expect_err("not started");
        assert!(late.lock().unwrap().is_empty());
        assert_eq!(yp.in_flight(), 0);
        back(None);
        assert_eq!(late.lock().unwrap().len(), 1);
    }

    /// Readbacks cancelled without retiring the context (a readback-mode switch) leave their
    /// staging buffers out of the ring until the map callback unmaps them, so the next frames
    /// never copy into a buffer that is still mapped or pending.
    #[test]
    fn cancelled_readbacks_return_their_buffers_only_once_unmapped() {
        crate::tests::hold_gpu_lock();
        let mut g = crate::Gpu::headless().expect("no GPU adapter");
        let got: Got = Default::default();
        submit(&g, 3, &got);
        std::sync::Arc::get_mut(&mut g.ctx).expect("sole owner").set_deferred(false);
        assert_eq!(got.lock().unwrap().len(), 3, "cancelled at once");
        assert!(got.lock().unwrap().iter().all(|(_, p)| p.is_none()));
        assert_eq!(g.ctx.yuv.get().map(|yp| yp.kept()), Some(0), "still pending maps: not reusable yet");
        for _ in 0..100 {
            if g.ctx.yuv.get().is_some_and(|yp| yp.kept() == 3) {
                break;
            }
            g.poll_readbacks(Some(Duration::from_millis(50)));
        }
        assert_eq!(g.ctx.yuv.get().map(|yp| yp.kept()), Some(3), "back once unmapped");
        let again: Got = Default::default();
        submit(&g, 3, &again);
        drain(&g, &again, 3);
        assert!(g.ctx.check_health().is_ok(), "reusing the buffers raised no device error");
        for (i, p) in sorted(&again) {
            let want = super::tests::cpu(&super::tests::noise(64, 32, i as u32 + 1), [0.0; 3]);
            super::tests::assert_close_exact(&p.expect("planes"), &want, &format!("frame {i}"));
        }
    }
}

#[cfg(test)]
mod tie_tests {
    use effectcraft_raster::Image;

    fn lum(r: u8, g: u8, b: u8) -> f32 {
        let (r, g, b) = (r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0);
        0.2126 * r + 0.7152 * g + 0.0722 * b
    }

    fn px(p: [u8; 3]) -> [f32; 4] {
        [p[0] as f32 / 255.0, p[1] as f32 / 255.0, p[2] as f32 / 255.0, 1.0]
    }

    /// Opaque pixels whose Y (or whose 2×2 block's U or V) lands exactly on .5 in the CPU's f32
    /// math: a kernel that rounds ties the other way moves most of them.
    #[test]
    fn rounding_ties_match_cpu() {
        let mut ties: Vec<[u8; 3]> = vec![];
        for r in 0..=255u8 {
            for g in 0..=255u8 {
                for b in 0..=255u8 {
                    let x = 16.0 + 219.0 * lum(r, g, b);
                    if x - x.floor() == 0.5 {
                        ties.push([r, g, b]);
                    }
                }
            }
        }
        assert!(!ties.is_empty());
        let mut blocks: Vec<[[u8; 3]; 4]> = vec![];
        let mut s = 12345u32;
        let mut rnd = || {
            s ^= s << 13;
            s ^= s >> 17;
            s ^= s << 5;
            (s >> 24) as u8
        };
        // Bounded so the search stays quick in debug builds (a few hundred blocks).
        for _ in 0..8_000_000 {
            let blk = [[rnd(), rnd(), rnd()], [rnd(), rnd(), rnd()], [rnd(), rnd(), rnd()], [rnd(), rnd(), rnd()]];
            let (mut a0, mut a1) = (0f32, 0f32);
            for p in blk {
                let l = lum(p[0], p[1], p[2]);
                a0 += (p[2] as f32 / 255.0 - l) / 1.8556;
                a1 += (p[0] as f32 / 255.0 - l) / 1.5748;
            }
            let u = 128.0 + 224.0 * a0 / 4.0;
            let v = 128.0 + 224.0 * a1 / 4.0;
            if u - u.floor() == 0.5 || v - v.floor() == 0.5 {
                blocks.push(blk);
            }
        }
        eprintln!("Y tie pixels: {}, chroma tie blocks: {}", ties.len(), blocks.len());
        assert!(!blocks.is_empty());
        let Some(g) = crate::tests::gpu() else { return };
        let yp = super::YuvPipe::new(&g.ctx.device);
        let w = 1024usize;
        let h = (ties.len().div_ceil(w) + 1) & !1;
        let mut img = Image::new(w as u32, h as u32);
        for (i, p) in img.data.iter_mut().enumerate() {
            *p = px(ties[i % ties.len()]);
        }
        let src = g.ctx.upload_image(&img).expect("upload");
        let got = super::convert(&mut crate::context::Enc::new(&g.ctx), &yp, &src, [0.0; 3]).expect("readback");
        super::tests::assert_parity(g, &got, &super::tests::cpu(&img, [0.0; 3]), 0.02, "Y ties");
        let cw = 512usize;
        let rows = blocks.len().div_ceil(cw);
        let mut img = Image::new((cw * 2) as u32, (rows * 2) as u32);
        for cy in 0..rows {
            for cx in 0..cw {
                let b = blocks[(cy * cw + cx) % blocks.len()];
                for (k, p) in b.iter().enumerate() {
                    let (x, y) = (cx * 2 + k % 2, cy * 2 + k / 2);
                    img.data[y * cw * 2 + x] = px(*p);
                }
            }
        }
        let src = g.ctx.upload_image(&img).expect("upload");
        let got = super::convert(&mut crate::context::Enc::new(&g.ctx), &yp, &src, [0.0; 3]).expect("readback");
        super::tests::assert_parity(g, &got, &super::tests::cpu(&img, [0.0; 3]), 0.06, "chroma ties");
    }
}

#[cfg(test)]
mod cap_and_poll_tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use effectcraft_render::{Accelerator, Backend, RenderOpts, Renderer};
    use effectcraft_time::Tick;

    /// Eight threads submitting at once never have more than `IN_FLIGHT` frames pending.
    #[test]
    fn in_flight_cap_holds_across_threads() {
        let Some(g) = crate::tests::gpu() else { return };
        let s = super::accel_tests::scene(96, 60);
        let max = AtomicUsize::new(0);
        let got = Arc::new(Mutex::new(0usize));
        std::thread::scope(|sc| {
            for t in 0..8 {
                let (s, max, got) = (&s, &max, got.clone());
                sc.spawn(move || {
                    let mut r = Renderer::new(&s.p, &crate::tests::Pattern, RenderOpts { backend: Backend::Gpu, ..crate::tests::opts() });
                    r.accel = Some(g);
                    for i in 0..6 {
                        let got = got.clone();
                        let ok = g.comp_frame_yuv420_async(
                            &r,
                            s.cid,
                            Tick::ZERO,
                            [t as f32 / 8.0, i as f32 / 6.0, 0.5],
                            Box::new(move |_| *got.lock().unwrap() += 1),
                        );
                        assert!(ok.is_ok());
                        max.fetch_max(g.ctx.yuv.get().unwrap().in_flight(), Ordering::SeqCst);
                    }
                });
            }
        });
        for _ in 0..400 {
            if *got.lock().unwrap() >= 48 {
                break;
            }
            g.poll_readbacks(Some(Duration::from_millis(50)));
        }
        eprintln!("max in flight observed: {} (cap {})", max.load(Ordering::SeqCst), super::IN_FLIGHT);
        assert_eq!(*got.lock().unwrap(), 48);
        assert!(max.load(Ordering::SeqCst) <= super::IN_FLIGHT, "cap exceeded: {}", max.load(Ordering::SeqCst));
        assert_eq!(g.ctx.yuv.get().unwrap().in_flight(), 0);
    }

    /// Submit enough work that a zero-length wait finds the queue busy.
    fn busy(g: &crate::Gpu) {
        let yp = g.ctx.yuv.get_or_init(|| super::YuvPipe::new(&g.ctx.device));
        let src = g.ctx.upload_image(&super::tests::noise(2048, 2048, 9)).expect("upload");
        for _ in 0..8 {
            let _ = super::record(&mut crate::context::Enc::new(&g.ctx), yp, &src, [0.0; 3]);
        }
    }

    /// `Gpu::wait` still treats work running past its deadline as a hung device (retires the
    /// context); only the asynchronous YUV poll reads a timed-out wait as a busy queue.
    #[test]
    fn wait_past_its_deadline_retires_but_the_yuv_poll_does_not() {
        crate::tests::hold_gpu_lock();
        let g = crate::Gpu::headless().expect("no GPU adapter");
        let mut busy_seen = false;
        for _ in 0..20 {
            busy(&g);
            let st = g.ctx.poll_readbacks_busy_ok(wgpu::PollType::Wait { submission_index: None, timeout: Some(Duration::ZERO) });
            assert!(st.is_ok() && g.ctx.check_health().is_ok(), "busy is not a failure");
            if matches!(st, Ok(wgpu::PollStatus::Poll)) {
                busy_seen = true;
                break;
            }
        }
        assert!(busy_seen, "the queue was never busy: the test proves nothing here");
        assert!(g.poll_readbacks(Some(Duration::from_secs(10))));
        let g = crate::Gpu::headless().expect("no GPU adapter");
        for _ in 0..20 {
            busy(&g);
            g.wait_for(Duration::ZERO);
            if g.ctx.check_health().is_err() {
                return;
            }
        }
        panic!("wait() past its deadline never retired the context");
    }
}
