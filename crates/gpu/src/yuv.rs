//! Export frames converted on the device to the H.264 encoder's input (8-bit BT.709 limited
//! range 4:2:0, see `shaders/yuv.wgsl`): a 4K frame reads back 12 MB of planes instead of
//! 133 MB of RGBA f32, and the CPU skips `to_rgba8_over` + `rgba_to_yuv420`.

use wgpu::util::DeviceExt;

use crate::context::{Enc, GpuImage};

pub(crate) struct YuvPipe {
    bgl: wgpu::BindGroupLayout,
    pipe: wgpu::ComputePipeline,
}

impl YuvPipe {
    pub(crate) fn new(device: &wgpu::Device) -> Result<YuvPipe, String> {
        crate::context::init_resource(device, "yuv420", || {
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
            YuvPipe { bgl, pipe }
        })
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

/// Whether the [`planes_len`] storage and staging buffers for `w×h` fit within `limits`.
pub(crate) fn fits_limits(w: u32, h: u32, limits: &wgpu::Limits) -> bool {
    let size = planes_len(w, h);
    size <= limits.max_storage_buffer_binding_size && size <= limits.max_buffer_size
}

/// Record the conversion of `img` over `bg` into a new `MAP_READ` buffer of
/// [`planes_len`] bytes and submit it. `None`: the size is not [`supported`], exceeds the
/// device's buffer limits, or buffer allocation failed.
pub(crate) fn record(e: &mut Enc, yp: &YuvPipe, img: &GpuImage, bg: [f32; 3]) -> Option<wgpu::Buffer> {
    let (w, h) = (img.width, img.height);
    let g = e.g;
    if !supported(w, h) || !fits_limits(w, h, &g.device.limits()) {
        return None;
    }
    let cw = w / 2;
    let size = planes_len(w, h);
    let mut params = [0u8; 32];
    // w * h fits in u32: both are at most the texture limit (16384).
    for (i, v) in [w, h, cw, w * h / 4].into_iter().enumerate() {
        params[i * 4..i * 4 + 4].copy_from_slice(&v.to_le_bytes());
    }
    for (i, v) in [bg[0], bg[1], bg[2], 1.0].into_iter().enumerate() {
        params[16 + i * 4..16 + i * 4 + 4].copy_from_slice(&v.to_le_bytes());
    }
    let (out, stg, ub) = match crate::context::init_resource(&g.device, "yuv420 buffers", || {
        let out = g.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("yuv420 out"),
            size,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let stg = g.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("yuv420 staging"),
            size,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let ub = g.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("yuv420 params"),
            contents: &params,
            usage: wgpu::BufferUsages::UNIFORM,
        });
        (out, stg, ub)
    }) {
        Ok(buffers) => buffers,
        Err(err) => {
            log::warn!(target: "effectcraft_gpu", "yuv420 buffers: {err}");
            return None;
        }
    };
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
    e.g.read_buffer(&stg, n, n, 1).map_err(|err| log::error!(target: "effectcraft_gpu", "yuv420 readback: {err}")).ok()
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
        let yp = super::YuvPipe::new(&g.ctx.device).expect("yuv420 pipe");
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
    fn sizes_exceeding_buffer_limits_are_declined() {
        let def = wgpu::Limits::default();
        assert!(super::fits_limits(3840, 2160, &def));
        // 16384×16384 planes are 384 MiB, over default max_storage_buffer_binding_size (128 MiB)
        // and max_buffer_size (256 MiB).
        assert!(!super::fits_limits(16384, 16384, &def));
        let size = super::planes_len(64, 64);
        assert!(super::fits_limits(64, 64, &wgpu::Limits { max_storage_buffer_binding_size: size, max_buffer_size: size, ..def.clone() }));
        assert!(!super::fits_limits(64, 64, &wgpu::Limits { max_storage_buffer_binding_size: size - 1, max_buffer_size: size, ..def.clone() }));
        assert!(!super::fits_limits(64, 64, &wgpu::Limits { max_storage_buffer_binding_size: size, max_buffer_size: size - 1, ..def.clone() }));

        let Some(_) = crate::tests::gpu() else { return };
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter = pollster::block_on(instance.request_adapter(&Default::default())).expect("adapter");
        let required_limits = wgpu::Limits { max_storage_buffer_binding_size: 4096, max_buffer_size: 4096, ..def };
        let (device, queue) = pollster::block_on(adapter.request_device(&crate::context::device_descriptor(required_limits))).expect("limited device");
        let g = crate::Gpu::new(&adapter, device, queue).expect("limited Gpu");
        let yp = super::YuvPipe::new(&g.ctx.device).expect("yuv420 pipe");
        let src = g.ctx.upload_image(&noise(64, 64, 1)).expect("upload");
        assert!(super::convert(&mut crate::context::Enc::new(&g.ctx), &yp, &src, [0.0; 3]).is_none());
        assert!(g.ctx.check_health().is_ok());
    }

    #[test]
    fn unsupported_sizes_are_not_converted() {
        let Some(g) = crate::tests::gpu() else { return };
        let yp = super::YuvPipe::new(&g.ctx.device).expect("yuv420 pipe");
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

    fn scene(w: u32, h: u32) -> Scene {
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

    #[test]
    fn comp_frame_yuv420_falls_back_when_pipeline_init_failed() {
        let Some(_) = crate::tests::gpu() else { return };
        let g = crate::Gpu::try_headless().expect("headless Gpu");
        let _ = g.ctx.yuv.set(Err("simulated yuv420 init failure".into()));
        let s = scene(96, 60);
        let mut r = Renderer::new(&s.p, &Pattern, RenderOpts { backend: Backend::Gpu, ..opts() });
        r.accel = Some(&g);
        assert!(g.comp_frame_yuv420(&r, s.cid, Tick::ZERO, [0.0; 3]).is_none());
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
        let yp = super::YuvPipe::new(&g.ctx.device).expect("yuv420 pipe");
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
