//! The viewer's display pass on egui-wgpu's device (#2797): a rendered frame (half floats for 32
//! bpc projects, RGBA8 otherwise) goes through Show Channel, exposure, the pending HDR step and
//! the display colour (`effectcraft_engine::viewer::DisplayPass`) into one premultiplied RGBA8
//! texture with mips per viewer, which egui draws like the GPU compositor's frames (native
//! texture, the Viewer Zoom Quality sampler).
//!
//! This is not the compositor's `Gpu` (which may be missing): it needs only egui-wgpu's device.
//! Without one (no adapter, headless tests, the device retired) the same pass runs on the CPU
//! (`DisplayPass::pixel`). The shader (`display_gpu.wgsl`) repeats the CPU steps operation by
//! operation; the tests below hold it to them.

use std::sync::Arc;

use effectcraft_engine::viewer::{Channel, DisplayPass, PassColor, PassConv, PassCurve};
use eframe::egui_wgpu;
use eframe::wgpu;

use crate::frames::F16Frame;

const UNIFORM_BYTES: usize = 12 * 16;

/// A frame for the display pass.
#[derive(Clone)]
pub enum Source {
    F16(Arc<F16Frame>),
    Rgba8(Arc<egui::ColorImage>),
}

impl Source {
    fn same(&self, o: &Source) -> bool {
        match (self, o) {
            (Source::F16(a), Source::F16(b)) => Arc::ptr_eq(a, b),
            (Source::Rgba8(a), Source::Rgba8(b)) => Arc::ptr_eq(a, b),
            _ => false,
        }
    }

    pub fn size(&self) -> [usize; 2] {
        match self {
            Source::F16(f) => f.size(),
            Source::Rgba8(c) => c.size,
        }
    }

    /// A half-float frame past the texture limit goes through the CPU pass: the display
    /// conversion (exposure, HDR) comes before the averaging down, as with the 8-bit mips, and
    /// the GPU would average the scene-linear values first.
    pub fn needs_cpu_pass(&self, max_side: usize) -> bool {
        matches!(self, Source::F16(_)) && crate::frames::fit_factor(self.size(), max_side, 1) > 1
    }
}

/// The pass's uniform block (`Params` in the shader).
pub fn uniform(pass: &DisplayPass) -> [u8; UNIFORM_BYTES] {
    let channel = match pass.channel {
        Channel::Rgb => 0u32,
        Channel::Red => 1,
        Channel::Green => 2,
        Channel::Blue => 3,
        Channel::Alpha => 4,
        Channel::RgbStraight => 5,
    };
    let hdr = match pass.encoding.hdr {
        effectcraft_engine::project::HdrMode::Clip => 0u32,
        effectcraft_engine::project::HdrMode::Compand => 1,
        effectcraft_engine::project::HdrMode::ToneMap => 2,
    };
    let curve = pass.encoding.curve;
    let none = PassConv { from: PassCurve::Linear, matrix: None, to: PassCurve::Linear };
    let (kind, quantize, first, second, lut_n) = match &pass.color {
        None => (0u32, false, none, None, 2u32),
        Some(PassColor::Convs { first, quantize, second }) => (1, *quantize, *first, *second, 2),
        Some(PassColor::Lut(l)) => (2, false, none, None, l.size.max(2) as u32),
    };
    let s = second.unwrap_or(none);
    let mat = |m: Option<[[f32; 3]; 3]>| {
        let m = m.unwrap_or([[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]);
        m.map(|r| [r[0], r[1], r[2], 0.0])
    };
    let [x24, xm2, xm1] = effectcraft_engine::viewer::SHADER_EXPONENTS;
    let u = |v: u32| v.to_le_bytes();
    let f = |v: f32| v.to_le_bytes();
    let mut b = Vec::with_capacity(UNIFORM_BYTES);
    for v in [
        channel,
        pass.colorized as u32,
        curve.id(),
        hdr,
        kind,
        quantize as u32,
        second.is_some() as u32,
        lut_n,
        first.from.id(),
        first.to.id(),
        s.from.id(),
        s.to.id(),
    ] {
        b.extend(u(v));
    }
    for v in [
        pass.gain,
        curve.decode_param(),
        first.matrix.is_some() as u32 as f32,
        s.matrix.is_some() as u32 as f32,
        first.from.decode_param(),
        first.to.encode_param(),
        s.from.decode_param(),
        s.to.encode_param(),
        curve.encode_param(),
        x24,
        xm2,
        xm1,
    ] {
        b.extend(f(v));
    }
    for row in mat(first.matrix).into_iter().chain(mat(s.matrix)) {
        for v in row {
            b.extend(f(v));
        }
    }
    let mut out = [0u8; UNIFORM_BYTES];
    out.copy_from_slice(&b);
    out
}

/// Mip levels of a full chain for a `w` × `h` texture.
fn mip_levels(w: u32, h: u32) -> u32 {
    32 - w.max(h).max(1).leading_zeros()
}

/// The display pass's pipelines on one device (made once, kept with the app).
pub struct DisplayGpu {
    bake: wgpu::RenderPipeline,
    mip: wgpu::RenderPipeline,
    bake_layout: wgpu::BindGroupLayout,
    mip_layout: wgpu::BindGroupLayout,
    no_lut: wgpu::TextureView,
    max_side: u32,
}

impl DisplayGpu {
    /// The pipelines, or why the device would not make them (a shader a backend rejects): the
    /// viewer then runs the pass on the CPU instead of showing nothing.
    pub fn try_new(device: &wgpu::Device) -> Result<DisplayGpu, String> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let internal = device.push_error_scope(wgpu::ErrorFilter::Internal);
            let validation = device.push_error_scope(wgpu::ErrorFilter::Validation);
            let made = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| DisplayGpu::new(device)));
            let validation = pollster::block_on(validation.pop());
            let internal = pollster::block_on(internal.pop());
            if let Some(e) = validation.or(internal) {
                return Err(format!("the display pass's pipelines: {e}"));
            }
            made.map_err(|_| "the display pass's pipelines: the backend panicked".to_string())
        }
        #[cfg(target_arch = "wasm32")]
        {
            Ok(DisplayGpu::new(device))
        }
    }

    pub fn new(device: &wgpu::Device) -> DisplayGpu {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("viewer display pass"),
            source: wgpu::ShaderSource::Wgsl(include_str!("display_gpu.wgsl").into()),
        });
        let tex = |binding: u32, dim: wgpu::TextureViewDimension, filterable: bool| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable }, view_dimension: dim, multisampled: false },
            count: None,
        };
        let uniform = wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
            count: None,
        };
        let bake_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("viewer display pass"),
            entries: &[uniform, tex(1, wgpu::TextureViewDimension::D2, false), tex(2, wgpu::TextureViewDimension::D3, false)],
        });
        let mip_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("viewer display mips"),
            entries: &[tex(1, wgpu::TextureViewDimension::D2, false)],
        });
        let pipeline = |label: &str, layout: &wgpu::BindGroupLayout, entry: &str| {
            let pl =
                device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some(label), bind_group_layouts: &[Some(layout)], immediate_size: 0 });
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&pl),
                vertex: wgpu::VertexState { module: &module, entry_point: Some("vs"), buffers: &[], compilation_options: Default::default() },
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some(entry),
                    targets: &[Some(wgpu::ColorTargetState { format: wgpu::TextureFormat::Rgba8Unorm, blend: None, write_mask: wgpu::ColorWrites::ALL })],
                    compilation_options: Default::default(),
                }),
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                multiview_mask: None,
                cache: None,
            })
        };
        let no_lut = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("viewer display LUT (none)"),
            size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D3,
            format: wgpu::TextureFormat::Rgba32Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        DisplayGpu {
            bake: pipeline("viewer display pass", &bake_layout, "fs"),
            mip: pipeline("viewer display mips", &mip_layout, "mip"),
            bake_layout,
            mip_layout,
            no_lut: no_lut.create_view(&Default::default()),
            max_side: device.limits().max_texture_dimension_2d,
        }
    }

    /// The largest texture side the device takes.
    pub fn max_side(&self) -> usize {
        self.max_side as usize
    }

    /// `src` in a texture (half floats or RGBA8), averaged down by `k` first when `k > 1`
    /// (8-bit frames past the texture limit, #201; half-float ones never get here, see
    /// [`Source::needs_cpu_pass`]): `reuse` when it has the size and format (a playing
    /// preview's frames), else a new one.
    fn upload(&self, device: &wgpu::Device, queue: &wgpu::Queue, src: &Source, k: usize, reuse: Option<wgpu::Texture>) -> wgpu::Texture {
        let write = |w: u32, h: u32, format: wgpu::TextureFormat, bytes: &[u8]| {
            let (w, h) = (w.max(1), h.max(1));
            let fits = |t: &wgpu::Texture| t.width() == w && t.height() == h && t.format() == format;
            let tex = reuse.clone().filter(fits).unwrap_or_else(|| {
                device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("viewer frame (display pass input)"),
                    size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                    view_formats: &[],
                })
            });
            let px = if format == wgpu::TextureFormat::Rgba16Float { 8 } else { 4 };
            if bytes.len() as u64 >= w as u64 * h as u64 * px as u64 {
                queue.write_texture(
                    wgpu::TexelCopyTextureInfo { texture: &tex, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
                    bytes,
                    wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(w * px), rows_per_image: Some(h) },
                    wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
                );
            }
            tex
        };
        let f16 = wgpu::TextureFormat::Rgba16Float;
        let rgba8 = wgpu::TextureFormat::Rgba8Unorm;
        match src {
            Source::F16(f) => {
                debug_assert_eq!(k, 1, "half-float frames past the texture limit take the CPU pass");
                write(f.width, f.height, f16, bytemuck::cast_slice(f.bits()))
            }
            Source::Rgba8(c) if k > 1 => {
                let c = crate::frames::fit_texture_by((**c).clone(), usize::MAX, k);
                write(c.size[0] as u32, c.size[1] as u32, rgba8, c.as_raw())
            }
            Source::Rgba8(c) => write(c.size[0] as u32, c.size[1] as u32, rgba8, c.as_raw()),
        }
    }

    fn lut(&self, device: &wgpu::Device, queue: &wgpu::Queue, lut: &effectcraft_engine::color::icc::Lut3d) -> wgpu::Texture {
        let n = lut.size.max(2) as u32;
        let tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("viewer display LUT"),
            size: wgpu::Extent3d { width: n, height: n, depth_or_array_layers: n },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D3,
            format: wgpu::TextureFormat::Rgba32Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        // data[(r · n + g) · n + b]: x = b, y = g, z = r.
        let mut texels: Vec<f32> = Vec::with_capacity((n * n * n * 4) as usize);
        for i in 0..(n * n * n) as usize {
            let v = lut.data.get(i).copied().unwrap_or_default();
            texels.extend([v[0], v[1], v[2], 1.0]);
        }
        queue.write_texture(
            wgpu::TexelCopyTextureInfo { texture: &tex, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            bytemuck::cast_slice(&texels),
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(n * 16), rows_per_image: Some(n) },
            wgpu::Extent3d { width: n, height: n, depth_or_array_layers: n },
        );
        tex
    }

    /// The display texture for `input`: premultiplied RGBA8, a full mip chain.
    fn output(device: &wgpu::Device, w: u32, h: u32) -> wgpu::Texture {
        device.create_texture(&wgpu::TextureDescriptor {
            label: Some("viewer frame (displayed)"),
            size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            mip_level_count: mip_levels(w, h),
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        })
    }

    /// Run the pass from `input` into level 0 of `out`, then fill its mips.
    fn run(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        input: &wgpu::Texture,
        out: &wgpu::Texture,
        params: &[u8; UNIFORM_BYTES],
        lut: Option<&wgpu::Texture>,
    ) {
        use wgpu::util::DeviceExt as _;
        let buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("viewer display pass"),
            contents: params,
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let input_view = input.create_view(&Default::default());
        let lut_view = lut.map(|l| l.create_view(&Default::default()));
        let bake = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("viewer display pass"),
            layout: &self.bake_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: buf.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&input_view) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(lut_view.as_ref().unwrap_or(&self.no_lut)) },
            ],
        });
        let level = |l: u32| out.create_view(&wgpu::TextureViewDescriptor { base_mip_level: l, mip_level_count: Some(1), ..Default::default() });
        let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("viewer display pass") });
        let pass = |enc: &mut wgpu::CommandEncoder, target: &wgpu::TextureView, pipeline: &wgpu::RenderPipeline, bg: &wgpu::BindGroup| {
            let mut p = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("viewer display pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store },
                })],
                ..Default::default()
            });
            p.set_pipeline(pipeline);
            p.set_bind_group(0, bg, &[]);
            p.draw(0..3, 0..1);
        };
        pass(&mut enc, &level(0), &self.bake, &bake);
        for l in 1..out.mip_level_count() {
            let above = level(l - 1);
            let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("viewer display mips"),
                layout: &self.mip_layout,
                entries: &[wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&above) }],
            });
            pass(&mut enc, &level(l), &self.mip, &bg);
        }
        queue.submit([enc.finish()]);
    }
}

/// One viewer's displayed texture (the Composition viewer, a passive viewer, the snapshot): the
/// frame it was made from and the pass's uniform, so it runs again only when either changes.
#[derive(Default)]
pub struct Bake {
    source: Option<Source>,
    /// Averaged down by this for the texture limit (1: not).
    factor: usize,
    input: Option<wgpu::Texture>,
    output: Option<wgpu::Texture>,
    id: Option<egui::TextureId>,
    params: Option<[u8; UNIFORM_BYTES]>,
    lut: Option<(Arc<effectcraft_engine::color::icc::Lut3d>, wgpu::Texture)>,
    smooth: Option<bool>,
    /// How often the pass ran (tests).
    runs: u64,
}

impl Bake {
    /// The egui texture showing `src` through `pass` (sampled with the Viewer Zoom Quality's
    /// sampler for `smooth`), running the pass again when the frame or the pass changed.
    pub fn show(&mut self, rs: &egui_wgpu::RenderState, gpu: &DisplayGpu, src: &Source, pass: &DisplayPass, smooth: bool) -> Option<egui::TextureId> {
        if src.needs_cpu_pass(gpu.max_side()) {
            return None;
        }
        let (device, queue) = (&rs.device, &rs.queue);
        let params = uniform(pass);
        let new_source = !self.source.as_ref().is_some_and(|s| s.same(src));
        if new_source {
            self.factor = crate::frames::fit_factor(src.size(), gpu.max_side(), 1);
            self.input = Some(gpu.upload(device, queue, src, self.factor, self.input.take()));
            self.source = Some(src.clone());
        }
        let lut = match &pass.color {
            Some(PassColor::Lut(l)) => Some(l.clone()),
            _ => None,
        };
        // A new LUT (another profile, Preserve RGB, another display) has the same size, so the
        // uniform alone doesn't tell: it reruns the pass itself.
        let new_lut = match (&lut, &self.lut) {
            (Some(l), Some((have, _))) if Arc::ptr_eq(l, have) => false,
            (Some(l), _) => {
                self.lut = Some((l.clone(), gpu.lut(device, queue, l)));
                true
            }
            (None, had) => {
                let changed = had.is_some();
                self.lut = None;
                changed
            }
        };
        let input = self.input.as_ref()?;
        let (w, h) = (input.width(), input.height());
        let resized = self.output.as_ref().is_none_or(|o| o.width() != w || o.height() != h);
        if resized {
            self.output = Some(DisplayGpu::output(device, w, h));
        }
        let output = self.output.as_ref()?;
        if new_source || resized || new_lut || self.params != Some(params) {
            gpu.run(device, queue, input, output, &params, self.lut.as_ref().map(|(_, t)| t));
            self.params = Some(params);
            self.runs += 1;
        }
        let view = output.create_view(&Default::default());
        let sampler = || crate::panels::viewer::zoom_sampler(smooth);
        let mut ren = rs.renderer.write();
        let id = match self.id {
            Some(id) if !resized && self.smooth == Some(smooth) => id,
            Some(id) => {
                ren.update_egui_texture_from_wgpu_texture_with_sampler_options(device, &view, sampler(), id);
                id
            }
            None => ren.register_native_texture_with_sampler_options(device, &view, sampler()),
        };
        self.id = Some(id);
        self.smooth = Some(smooth);
        Some(id)
    }

    /// The texture egui draws, once [`Bake::show`] made it.
    pub fn id(&self) -> Option<egui::TextureId> {
        self.id
    }

    /// The frame shown (the one last given to [`Bake::show`]).
    pub fn source(&self) -> Option<&Source> {
        self.source.as_ref()
    }

    /// The texture coordinates of the frame in its texture (see `frames::fitted_uv`).
    pub fn uv(&self) -> egui::Rect {
        let size = self.source.as_ref().map_or([1, 1], Source::size);
        crate::frames::fitted_uv(size, self.factor.max(1))
    }

    /// Give the texture back to the renderer.
    pub fn free(&mut self, rs: Option<&egui_wgpu::RenderState>) {
        if let (Some(id), Some(rs)) = (self.id.take(), rs) {
            rs.renderer.write().free_texture(&id);
        }
        *self = Bake::default();
    }

    /// How often the pass ran for this slot.
    pub fn runs(&self) -> u64 {
        self.runs
    }

    /// The displayed texture (tests read it back).
    pub fn texture(&self) -> Option<&wgpu::Texture> {
        self.output.as_ref()
    }
}

#[cfg(test)]
mod tests;
