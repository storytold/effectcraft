//! The Advanced 3D rasteriser on wgpu: a render pipeline with a reversed-Z depth buffer
//! (`Depth32Float`, Greater), the scene's materials, textures, lights and shadow maps in
//! storage buffers, and `advanced3d.wgsl`'s physically based fragment shader. Opaque triangles
//! are drawn with depth writes, then the sorted transparent ones blended over (premultiplied).
//! Colour (`Rgba16Float`) and camera depth (`R32Uint` holding f32 bits) stay on the GPU for `adv3d.wgsl`'s
//! compute kernels, which run the rest of `effectcraft_render::three_d::adv::render_prepared`:
//! the 2×2 supersampling resolve, the motion-blur sub-sample average (nearest depth), the
//! depth-based iris depth of field (the Classic 3D bokeh kernel's row spans and prefix-sum
//! gather, highlight boost, progressive blend between blur levels), the sRGB encode and the
//! premultiplied "over" onto the compositor's canvas. Only the depth of field reads anything
//! back (its 8-byte radius range, to choose the blur levels as the CPU does).
//!
//! Wireframe-quality layers draw their outlines here too ([`wireframe`]).

use effectcraft_render::three_d::Dof;
use effectcraft_render::three_d::adv::{Prepared, Rendered, Scene, Target};
use effectcraft_render::three_d::bokeh;

use crate::context::{Enc, GpuContext, GpuImage};

/// Pipelines (built on first use).
pub(crate) struct Pipes {
    /// (opaque, transparent); `None` when the adapter can't rasterise ([`raster_unsupported`]):
    /// scenes then render on the CPU.
    raster: Option<(wgpu::BindGroupLayout, wgpu::RenderPipeline, wgpu::RenderPipeline)>,
    post: Post,
}

fn pipes(g: &GpuContext) -> &Pipes {
    g.adv3d.get_or_init(|| Pipes::new(&g.device, g.adv3d_raster))
}

const COLOR: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
/// Camera depth as the bits of an f32: integer targets render on every backend, while `R32Float`
/// needs `EXT_color_buffer_float` on GLES (absent on Mesa's llvmpipe, for one).
const DEPTH_OUT: wgpu::TextureFormat = wgpu::TextureFormat::R32Uint;
/// [`DEPTH_OUT`]'s clear value: the bits of −1.0 ("nothing drawn").
const NO_DEPTH: f64 = 0xBF80_0000u32 as f64;
const DEPTH: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
const VERTEX_SIZE: u64 = 52;

/// Why `adapter` can't run the rasteriser, or `None` when it can. Building the pipelines anyway
/// would be a validation error, which release builds only log (and then draw nothing), so the
/// caller keeps the scenes on the CPU instead.
pub(crate) fn raster_unsupported(adapter: &wgpu::Adapter) -> Option<String> {
    raster_capability_reason(adapter.get_downlevel_capabilities().flags, |format| adapter.get_texture_format_features(format))
}

fn raster_capability_reason(flags: wgpu::DownlevelFlags, features: impl Fn(wgpu::TextureFormat) -> wgpu::TextureFormatFeatures) -> Option<String> {
    use wgpu::TextureFormatFeatureFlags as F;
    use wgpu::TextureUsages as U;
    if !flags.contains(wgpu::DownlevelFlags::FRAGMENT_STORAGE) {
        return Some("no fragment storage buffers".into());
    }
    let targets = U::RENDER_ATTACHMENT | U::TEXTURE_BINDING | U::COPY_SRC;
    let needs = [(COLOR, targets, F::BLENDABLE), (DEPTH_OUT, targets, F::empty()), (DEPTH, U::RENDER_ATTACHMENT, F::empty())];
    for (format, usages, required_flags) in needs {
        let f = features(format);
        if !f.allowed_usages.contains(usages) || !f.flags.contains(required_flags) {
            return Some(format!("{format:?} lacks raster usages {usages:?} or flags {required_flags:?}"));
        }
    }
    // The transparent pipeline blends colour and masks the depth target off.
    if !flags.contains(wgpu::DownlevelFlags::INDEPENDENT_BLEND) {
        return Some("no independent blending".into());
    }
    None
}

/// Bound one allocation by its actual byte stride, without imposing a global image limit.
fn buffer_bytes(count: u64, stride: u64, limit: u64) -> Option<u64> {
    let size = count.checked_mul(stride)?;
    (size > 0 && size <= limit && usize::try_from(size).is_ok()).then_some(size)
}

/// Sizes of exactly the arrays serialized by pack, including its empty storage padding.
/// Checking these before serialization bounds host packing as well as the GPU uploads.
fn packed_bytes(counts: [u64; 6], uniform_limit: u64, storage_limit: u64, buffer_limit: u64) -> Option<[u64; 7]> {
    let mut sizes = [0; 7];
    sizes[0] = buffer_bytes(40, 4, uniform_limit.min(buffer_limit))?;
    for (index, (count, stride)) in counts.into_iter().zip([96, 16, 16, 80, 80, 4]).enumerate() {
        let size = count.checked_mul(stride)?.max(16);
        sizes[index + 1] = buffer_bytes(size, 1, storage_limit.min(buffer_limit))?;
    }
    Some(sizes)
}

#[cfg(test)]
mod capability_tests {
    use super::*;

    fn supported(format: wgpu::TextureFormat) -> wgpu::TextureFormatFeatures {
        let rt = wgpu::TextureUsages::RENDER_ATTACHMENT;
        wgpu::TextureFormatFeatures {
            allowed_usages: if format == DEPTH { rt } else { rt | wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_SRC },
            flags: if format == COLOR { wgpu::TextureFormatFeatureFlags::BLENDABLE } else { wgpu::TextureFormatFeatureFlags::empty() },
        }
    }

    fn downlevel() -> wgpu::DownlevelFlags {
        wgpu::DownlevelFlags::FRAGMENT_STORAGE | wgpu::DownlevelFlags::INDEPENDENT_BLEND
    }

    #[test]
    fn matching_raster_capabilities_are_accepted_without_optional_features() {
        assert!(raster_capability_reason(downlevel(), supported).is_none());
    }

    #[test]
    fn fragment_storage_and_independent_blending_are_required() {
        for flag in [wgpu::DownlevelFlags::FRAGMENT_STORAGE, wgpu::DownlevelFlags::INDEPENDENT_BLEND] {
            assert!(raster_capability_reason(downlevel() - flag, supported).is_some());
        }
    }

    #[test]
    fn color_and_integer_depth_require_their_actual_texture_usages() {
        for target in [COLOR, DEPTH_OUT] {
            for usage in [wgpu::TextureUsages::RENDER_ATTACHMENT, wgpu::TextureUsages::TEXTURE_BINDING, wgpu::TextureUsages::COPY_SRC] {
                assert!(
                    raster_capability_reason(downlevel(), |format| {
                        let mut features = supported(format);
                        if format == target {
                            features.allowed_usages.remove(usage);
                        }
                        features
                    })
                    .is_some()
                );
            }
        }
    }

    #[test]
    fn color_blending_and_depth_attachment_are_required() {
        for target in [COLOR, DEPTH] {
            assert!(
                raster_capability_reason(downlevel(), |format| {
                    let mut features = supported(format);
                    if format == target {
                        if target == COLOR {
                            features.flags.remove(wgpu::TextureFormatFeatureFlags::BLENDABLE);
                        } else {
                            features.allowed_usages.remove(wgpu::TextureUsages::RENDER_ATTACHMENT);
                        }
                    }
                    features
                })
                .is_some()
            );
        }
    }

    #[test]
    fn byte_guards_use_each_buffer_stride_and_reject_overflow() {
        assert_eq!(buffer_bytes(2, VERTEX_SIZE, 104), Some(104));
        assert_eq!(buffer_bytes(2, VERTEX_SIZE, 103), None);
        assert_eq!(buffer_bytes(26, 4, 104), Some(104));
        assert_eq!(buffer_bytes(27, 4, 104), None);
        assert_eq!(buffer_bytes(0, 4, 104), None);
        assert_eq!(buffer_bytes(u64::MAX, 4, u64::MAX), None);
        assert_eq!(buffer_bytes(1, 16, 15), None);
    }

    #[test]
    fn packed_arrays_preflight_exact_strides_and_empty_padding() {
        assert_eq!(packed_bytes([0; 6], 160, 16, 160), Some([160, 16, 16, 16, 16, 16, 16]));
        assert_eq!(packed_bytes([2; 6], 160, 192, 192), Some([160, 192, 32, 32, 160, 160, 16]));
        assert!(packed_bytes([0; 6], 159, 16, 160).is_none());
        assert!(packed_bytes([0; 6], 160, 15, 160).is_none());
        assert!(packed_bytes([2; 6], 160, 192, 191).is_none());
        for index in 0..6 {
            let mut counts = [0; 6];
            counts[index] = u64::MAX;
            assert!(packed_bytes(counts, u64::MAX, u64::MAX, u64::MAX).is_none());
        }
    }

    #[test]
    fn predicted_sizes_match_the_real_scene_serializer() {
        use effectcraft_render::three_d::adv::{Light, Material, ShadowMap, TexInfo};
        let scene = Scene {
            materials: vec![Material::default()],
            textures: vec![TexInfo { offset: 0, width: 1, height: 1, wrap_u: 0, wrap_v: 0 }],
            texels: vec![[0.0; 4]],
            lights: vec![Light {
                kind: 0,
                pos: [0.0; 3],
                dir: [0.0; 3],
                color: [0.0; 3],
                cos_inner: 0.0,
                cos_outer: 0.0,
                falloff: 0,
                radius: 0.0,
                falloff_distance: 0.0,
                shadow: -1,
                shadow_darkness: 0.0,
                shadow_diffusion: 0.0,
            }],
            shadows: vec![ShadowMap { view: [[0.0; 4]; 3], focal: 0.0, ortho: false, size: 1, offset: 0, bias: 0.0, radius: 0.0 }],
            shadow_texels: vec![0.0],
            ..Scene::default()
        };
        for (scene, counts) in [(Scene::default(), [0; 6]), (scene, [1; 6])] {
            let actual = pack(&scene).map(|bytes| bytes.len() as u64);
            assert_eq!(Some(actual), packed_bytes(counts, 160, 160, 160));
        }
    }
}

fn storage(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only: true }, has_dynamic_offset: false, min_binding_size: None },
        count: None,
    }
}

impl Pipes {
    fn new(device: &wgpu::Device, raster: bool) -> Pipes {
        let raster = raster.then(|| {
            let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("effectcraft advanced 3d"),
                source: wgpu::ShaderSource::Wgsl(include_str!("shaders/advanced3d.wgsl").into()),
            });
            let mut entries = vec![wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                count: None,
            }];
            entries.extend((1..=6).map(storage));
            let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some("advanced 3d"), entries: &entries });
            let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("advanced 3d"),
                bind_group_layouts: &[Some(&bgl)],
                immediate_size: 0,
            });
            let attrs = [
                wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x3, offset: 0, shader_location: 0 },
                wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x3, offset: 12, shader_location: 1 },
                wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x2, offset: 24, shader_location: 2 },
                wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x4, offset: 32, shader_location: 3 },
                wgpu::VertexAttribute { format: wgpu::VertexFormat::Uint32, offset: 48, shader_location: 4 },
            ];
            let make = |transparent: bool| {
                let blend = transparent.then_some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING);
                device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                    label: Some(if transparent { "advanced 3d transparent" } else { "advanced 3d opaque" }),
                    layout: Some(&layout),
                    vertex: wgpu::VertexState {
                        module: &module,
                        entry_point: Some("vs"),
                        compilation_options: Default::default(),
                        buffers: &[Some(wgpu::VertexBufferLayout { array_stride: VERTEX_SIZE, step_mode: wgpu::VertexStepMode::Vertex, attributes: &attrs })],
                    },
                    primitive: wgpu::PrimitiveState {
                        topology: wgpu::PrimitiveTopology::TriangleList,
                        front_face: wgpu::FrontFace::Ccw,
                        cull_mode: None,
                        ..Default::default()
                    },
                    depth_stencil: Some(wgpu::DepthStencilState {
                        format: DEPTH,
                        depth_write_enabled: Some(!transparent),
                        depth_compare: Some(wgpu::CompareFunction::Greater),
                        stencil: Default::default(),
                        bias: Default::default(),
                    }),
                    multisample: Default::default(),
                    fragment: Some(wgpu::FragmentState {
                        module: &module,
                        entry_point: Some("fs"),
                        compilation_options: Default::default(),
                        targets: &[
                            Some(wgpu::ColorTargetState { format: COLOR, blend, write_mask: wgpu::ColorWrites::ALL }),
                            Some(wgpu::ColorTargetState {
                                format: DEPTH_OUT,
                                blend: None,
                                write_mask: if transparent { wgpu::ColorWrites::empty() } else { wgpu::ColorWrites::ALL },
                            }),
                        ],
                    }),
                    multiview_mask: None,
                    cache: None,
                })
            };
            let opaque = make(false);
            let transparent = make(true);
            (bgl, opaque, transparent)
        });
        Pipes { raster, post: Post::new(device) }
    }
}

fn f32s(v: impl IntoIterator<Item = f32>) -> Vec<u8> {
    let mut b: Vec<u8> = v.into_iter().flat_map(f32::to_le_bytes).collect();
    while b.len() < 16 {
        b.push(0);
    }
    b
}

/// Pack the scene's arrays as the shader's storage buffers.
fn pack(s: &Scene) -> [Vec<u8>; 7] {
    let mut globals: Vec<f32> = vec![];
    // Column-major clip matrix.
    for c in 0..4 {
        for r in 0..4 {
            globals.push(s.clip[r][c]);
        }
    }
    globals.extend(s.view[2]);
    globals.extend([s.eye[0], s.eye[1], s.eye[2], s.ortho as u32 as f32]);
    globals.extend([s.cam_fwd[0], s.cam_fwd[1], s.cam_fwd[2], s.lights.len() as f32]);
    globals.extend([s.ambient[0], s.ambient[1], s.ambient[2], s.env.is_some() as u32 as f32]);
    let e = s.env.unwrap_or(effectcraft_render::three_d::adv::EnvInfo { radiance: 0, mips: 1, irradiance: 0, intensity: 0.0, rotation: 0.0 });
    globals.extend([e.radiance as f32, e.mips as f32, e.irradiance as f32, e.intensity]);
    let lit = !(s.lights.is_empty() && s.env.is_none() && s.ambient == [0.0; 3]);
    globals.extend([e.rotation, lit as u32 as f32, 0.0, 0.0]);
    let mats = s.materials.iter().flat_map(|m| {
        let flags = (m.double_sided as u32)
            | (m.unlit as u32) << 1
            | (m.accepts_lights as u32) << 2
            | (m.receives_shadows as u32) << 3
            | (m.shadows_only as u32) << 4
            | (m.alpha_mode << 8);
        [
            m.base[0],
            m.base[1],
            m.base[2],
            m.base[3],
            m.emissive[0],
            m.emissive[1],
            m.emissive[2],
            m.metallic,
            m.roughness,
            m.normal_scale,
            m.occlusion_strength,
            m.alpha_cutoff,
            m.tex_base as f32,
            m.tex_mr as f32,
            m.tex_normal as f32,
            m.tex_occlusion as f32,
            m.tex_emissive as f32,
            flags as f32,
            m.diffuse_k,
            m.specular_k,
            m.ambient_k,
            m.opacity,
            0.0,
            0.0,
        ]
    });
    let mut tex: Vec<u8> = s.textures.iter().flat_map(|t| [t.offset, t.width, t.height, t.wrap_u | t.wrap_v << 8]).flat_map(u32::to_le_bytes).collect();
    while tex.len() < 16 {
        tex.push(0);
    }
    let texels = f32s(s.texels.iter().flatten().copied());
    let lights = s.lights.iter().flat_map(|l| {
        [
            l.pos[0],
            l.pos[1],
            l.pos[2],
            l.kind as f32,
            l.dir[0],
            l.dir[1],
            l.dir[2],
            l.cos_outer,
            l.color[0],
            l.color[1],
            l.color[2],
            l.cos_inner,
            l.falloff as f32,
            l.radius,
            l.falloff_distance,
            l.shadow as f32,
            l.shadow_darkness,
            0.0,
            0.0,
            0.0,
        ]
    });
    let shadows = s.shadows.iter().flat_map(|m| {
        let mut v: Vec<f32> = m.view.iter().flatten().copied().collect();
        v.extend([m.focal, m.ortho as u32 as f32, f32::from_bits(m.size), f32::from_bits(m.offset), m.bias, m.radius, 0.0, 0.0]);
        v
    });
    [f32s(globals), f32s(mats), tex, texels, f32s(lights), f32s(shadows), f32s(s.shadow_texels.iter().copied())]
}

fn half_to_f32(h: u16) -> f32 {
    let s = if h & 0x8000 != 0 { -1.0 } else { 1.0 };
    let e = ((h >> 10) & 0x1f) as i32;
    let m = (h & 0x3ff) as f32;
    match e {
        0 => s * m * 2f32.powi(-24),
        31 => {
            if m == 0.0 {
                s * f32::INFINITY
            } else {
                f32::NAN
            }
        }
        _ => s * (1.0 + m / 1024.0) * 2f32.powi(e - 15),
    }
}

/// Record the render pass of one scene into `e`: (colour `Rgba16Float`, camera depth `R32Uint`
/// with −1 where nothing was drawn) at the scene's raster size. `None` when the device can't
/// (render targets, size or buffer limits).
fn raster_into(e: &mut Enc, s: &Scene) -> Option<(wgpu::Texture, wgpu::Texture)> {
    let g = e.g;
    if !g.adv3d_raster || !g.fits(s.width, s.height) || s.indices.is_empty() {
        return None;
    }
    let limits = g.device.limits();
    buffer_bytes(u64::try_from(s.vertices.len()).ok()?, VERTEX_SIZE, limits.max_buffer_size)?;
    buffer_bytes(u64::try_from(s.indices.len()).ok()?, 4, limits.max_buffer_size)?;
    let n = u32::try_from(s.indices.len()).ok()?;
    if s.opaque_count > n {
        return None;
    }
    let counts = [s.materials.len(), s.textures.len(), s.texels.len(), s.lights.len(), s.shadows.len(), s.shadow_texels.len()];
    let mut checked_counts = [0; 6];
    for (out, count) in checked_counts.iter_mut().zip(counts) {
        *out = u64::try_from(count).ok()?;
    }
    let expected_sizes = packed_bytes(checked_counts, limits.max_uniform_buffer_binding_size, limits.max_storage_buffer_binding_size, limits.max_buffer_size)?;
    let bufs = pack(s);
    if bufs.iter().zip(expected_sizes).any(|(bytes, expected)| bytes.len() as u64 != expected) {
        return None;
    }
    let p = pipes(g);
    let Some((bgl, opaque, transparent)) = &p.raster else { return None };
    let dev = &g.device;
    let vb: Vec<u8> = s
        .vertices
        .iter()
        .flat_map(|v| {
            let mut b = Vec::with_capacity(VERTEX_SIZE as usize);
            for x in v.pos.iter().chain(&v.normal).chain(&v.uv).chain(&v.tangent) {
                b.extend_from_slice(&x.to_le_bytes());
            }
            b.extend_from_slice(&v.material.to_le_bytes());
            b
        })
        .collect();
    let vbuf = g.buffer_init(Some("adv3d vertices"), &vb, wgpu::BufferUsages::VERTEX);
    let ib: Vec<u8> = s.indices.iter().flat_map(|i| i.to_le_bytes()).collect();
    let ibuf = g.buffer_init(Some("adv3d indices"), &ib, wgpu::BufferUsages::INDEX);
    let ub = g.buffer_init(Some("adv3d globals"), &bufs[0], wgpu::BufferUsages::UNIFORM);
    let sbs: Vec<wgpu::Buffer> = bufs[1..].iter().map(|b| g.buffer_init(Some("adv3d data"), b, wgpu::BufferUsages::STORAGE)).collect();
    let mut entries = vec![wgpu::BindGroupEntry { binding: 0, resource: ub.as_entire_binding() }];
    for (i, b) in sbs.iter().enumerate() {
        entries.push(wgpu::BindGroupEntry { binding: i as u32 + 1, resource: b.as_entire_binding() });
    }
    let bg = dev.create_bind_group(&wgpu::BindGroupDescriptor { label: Some("adv3d"), layout: bgl, entries: &entries });
    let tex = |format: wgpu::TextureFormat, usage: wgpu::TextureUsages| {
        dev.create_texture(&wgpu::TextureDescriptor {
            label: Some("adv3d target"),
            size: wgpu::Extent3d { width: s.width, height: s.height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage,
            view_formats: &[],
        })
    };
    let rt = wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC | wgpu::TextureUsages::TEXTURE_BINDING;
    let color = tex(COLOR, rt);
    let zout = tex(DEPTH_OUT, rt);
    let depth = tex(DEPTH, wgpu::TextureUsages::RENDER_ATTACHMENT);
    let (cv, zv, dv) = (color.create_view(&Default::default()), zout.create_view(&Default::default()), depth.create_view(&Default::default()));
    let enc = e.record(&[]);
    {
        let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("adv3d"),
            color_attachments: &[
                Some(wgpu::RenderPassColorAttachment {
                    view: &cv,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store },
                }),
                Some(wgpu::RenderPassColorAttachment {
                    view: &zv,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color { r: NO_DEPTH, g: 0.0, b: 0.0, a: 0.0 }), store: wgpu::StoreOp::Store },
                }),
            ],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &dv,
                depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(0.0), store: wgpu::StoreOp::Store }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_bind_group(0, &bg, &[]);
        pass.set_vertex_buffer(0, vbuf.slice(..));
        pass.set_index_buffer(ibuf.slice(..), wgpu::IndexFormat::Uint32);
        if s.opaque_count > 0 {
            pass.set_pipeline(opaque);
            pass.draw_indexed(0..s.opaque_count, 0, 0..1);
        }
        if n > s.opaque_count {
            pass.set_pipeline(transparent);
            pass.draw_indexed(s.opaque_count..n, 0, 0..1);
        }
    }
    Some((color, zout))
}

/// Rasterise a scene and read it back ([`effectcraft_render::Accelerator::raster_3d`]). `None`
/// when the device can't (no readback, size or buffer limits).
///
/// With deferred readbacks (a browser worker) both targets are read back under the scene's key
/// ([`crate::deferred`]): the first pass starts them and gets an empty target (the pass misses).
pub(crate) fn render(g: &GpuContext, s: &Scene) -> Option<Target> {
    if !g.can_readback() {
        return None;
    }
    let mut e = Enc::new(g);
    let (cb, zb) = match g.deferred.clone() {
        Some(d) => {
            use crate::deferred::Lookup::*;
            let mut k = crate::deferred::Key::new(4);
            key_scene(&mut k, s);
            let kc = k.finish();
            let kz = kc ^ 1;
            match (d.lookup(kc), d.lookup(kz)) {
                (Ready(c), Ready(z)) => (c?.bytes, z?.bytes),
                (Absent, Absent) => {
                    let (color, zout) = raster_into(&mut e, s)?;
                    e.read_texture_async(&color, s.width, s.height, 8, d.start(kc, s.width, s.height, [0.0; 2], 1.0));
                    e.read_texture_async(&zout, s.width, s.height, 4, d.start(kz, s.width, s.height, [0.0; 2], 1.0));
                    d.miss();
                    return Some(empty_target(s.width, s.height));
                }
                _ => {
                    d.miss();
                    return Some(empty_target(s.width, s.height));
                }
            }
        }
        None => {
            let (color, zout) = raster_into(&mut e, s)?;
            (e.read_texture(&color, s.width, s.height, 8)?, e.read_texture(&zout, s.width, s.height, 4)?)
        }
    };
    if cb.len() != (s.width * s.height * 8) as usize || zb.len() != (s.width * s.height * 4) as usize {
        return None;
    }
    let color = cb.as_chunks::<8>().0.iter().map(|c| [0, 1, 2, 3].map(|k| half_to_f32(u16::from_le_bytes([c[2 * k], c[2 * k + 1]])))).collect();
    let depth = zb
        .as_chunks::<4>()
        .0
        .iter()
        .map(|c| {
            let z = f32::from_le_bytes([c[0], c[1], c[2], c[3]]);
            if z < 0.0 { f32::INFINITY } else { z }
        })
        .collect();
    Some(Target { width: s.width, height: s.height, color, depth })
}

/// A placeholder target (a pass whose readbacks are in flight).
fn empty_target(w: u32, h: u32) -> Target {
    let n = (w * h) as usize;
    Target { width: w, height: h, color: vec![[0.0; 4]; n], depth: vec![f32::INFINITY; n] }
}

/// Everything that determines a scene's pixels, for deferred readback keys: the packed
/// buffers, the geometry, the target size.
fn key_scene(k: &mut crate::deferred::Key, s: &Scene) {
    k.u64(s.width as u64);
    k.u64(s.height as u64);
    k.u64(s.ssaa as u64);
    k.u64(s.opaque_count as u64);
    for b in pack(s) {
        k.u64(b.len() as u64);
        k.bytes(&b);
    }
    let mut v = Vec::with_capacity(s.vertices.len() * VERTEX_SIZE as usize);
    for x in &s.vertices {
        for f in x.pos.iter().chain(&x.normal).chain(&x.uv).chain(&x.tangent) {
            v.extend_from_slice(&f.to_le_bytes());
        }
        v.extend_from_slice(&x.material.to_le_bytes());
    }
    k.bytes(&v);
    k.bytes(&s.indices.iter().flat_map(|i| i.to_le_bytes()).collect::<Vec<u8>>());
}

// ---------------------------------------------------------------- after the rasteriser

/// Compute kernels of `adv3d.wgsl` (resolve, motion-blur accumulation, depth of field,
/// encoding, compositing, wireframes) with their shared bind group layout and the placeholder
/// resources bound to the slots a kernel doesn't use.
pub(crate) struct Post {
    bgl: wgpu::BindGroupLayout,
    kernels: Vec<(&'static str, wgpu::ComputePipeline)>,
    /// Placeholders for bindings 3–9 (distinct buffers: writable bindings must not alias).
    dummy_bufs: Vec<wgpu::Buffer>,
    dummy_color: wgpu::TextureView,
    dummy_z: wgpu::TextureView,
    dummy_out: wgpu::TextureView,
}

const KERNELS: &[&str] = &["resolve", "radius_of", "boost", "prefix_rows", "gather", "finish", "wire", "occlude"];

impl Post {
    fn new(device: &wgpu::Device) -> Post {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("effectcraft advanced 3d post"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/adv3d.wgsl").into()),
        });
        let tex = |binding, sample_type| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Texture { sample_type, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false },
            count: None,
        };
        let buf = |binding, read_only| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only }, has_dynamic_offset: false, min_binding_size: None },
            count: None,
        };
        let entries = [
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                count: None,
            },
            tex(1, wgpu::TextureSampleType::Float { filterable: false }),
            tex(2, wgpu::TextureSampleType::Uint),
            buf(3, false),
            buf(4, false),
            buf(5, false),
            buf(6, true),
            buf(7, false),
            buf(8, false),
            buf(9, false),
            wgpu::BindGroupLayoutEntry {
                binding: 10,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::StorageTexture {
                    access: wgpu::StorageTextureAccess::WriteOnly,
                    format: crate::context::FORMAT,
                    view_dimension: wgpu::TextureViewDimension::D2,
                },
                count: None,
            },
        ];
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some("advanced 3d post"), entries: &entries });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("advanced 3d post"),
            bind_group_layouts: &[Some(&bgl)],
            immediate_size: 0,
        });
        let kernels = KERNELS
            .iter()
            .map(|k| {
                let p = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                    label: Some(k),
                    layout: Some(&layout),
                    module: &module,
                    entry_point: Some(k),
                    compilation_options: Default::default(),
                    cache: None,
                });
                (*k, p)
            })
            .collect();
        let dummy_bufs = (3..=9)
            .map(|_| {
                device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("adv3d dummy"),
                    size: 16,
                    usage: wgpu::BufferUsages::STORAGE,
                    mapped_at_creation: false,
                })
            })
            .collect();
        let tex1 = |format, usage| {
            device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some("adv3d dummy"),
                    size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage,
                    view_formats: &[],
                })
                .create_view(&Default::default())
        };
        Post {
            bgl,
            kernels,
            dummy_bufs,
            dummy_color: tex1(COLOR, wgpu::TextureUsages::TEXTURE_BINDING),
            dummy_z: tex1(DEPTH_OUT, wgpu::TextureUsages::TEXTURE_BINDING),
            dummy_out: tex1(crate::context::FORMAT, wgpu::TextureUsages::STORAGE_BINDING),
        }
    }
}

/// The resources of one post kernel dispatch (unset slots get placeholders).
#[derive(Default)]
struct Binds<'b> {
    color: Option<&'b wgpu::TextureView>,
    z: Option<&'b wgpu::TextureView>,
    /// Bindings 3–9: acc, depth, src, table, radius, range, prefix.
    bufs: [Option<&'b wgpu::Buffer>; 7],
    out: Option<&'b wgpu::TextureView>,
}

/// Uniform block of the post kernels (`Post` in `adv3d.wgsl`).
fn params(u0: [u32; 4], f0: [f32; 4]) -> Vec<u8> {
    let mut v: Vec<u8> = u0.iter().flat_map(|x| x.to_le_bytes()).collect();
    v.extend([0u8; 16]);
    v.extend(f0.iter().flat_map(|x| x.to_le_bytes()));
    v.extend([0u8; 16]);
    v
}

fn dispatch(e: &mut Enc, kernel: &str, u0: [u32; 4], f0: [f32; 4], b: Binds, groups: (u32, u32)) {
    let g = e.g;
    let post = &pipes(g).post;
    let Some((_, pipe)) = post.kernels.iter().find(|(k, _)| *k == kernel) else { return };
    let ub = g.buffer_init(None, &params(u0, f0), wgpu::BufferUsages::UNIFORM);
    let mut entries = vec![
        wgpu::BindGroupEntry { binding: 0, resource: ub.as_entire_binding() },
        wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(b.color.unwrap_or(&post.dummy_color)) },
        wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(b.z.unwrap_or(&post.dummy_z)) },
    ];
    for (i, buf) in b.bufs.iter().enumerate() {
        entries.push(wgpu::BindGroupEntry { binding: 3 + i as u32, resource: buf.unwrap_or(&post.dummy_bufs[i]).as_entire_binding() });
    }
    entries.push(wgpu::BindGroupEntry { binding: 10, resource: wgpu::BindingResource::TextureView(b.out.unwrap_or(&post.dummy_out)) });
    let bg = g.device.create_bind_group(&wgpu::BindGroupDescriptor { label: Some(kernel), layout: &post.bgl, entries: &entries });
    let enc = e.record(&[]);
    let mut pass = enc.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some(kernel), timestamp_writes: None });
    pass.set_pipeline(pipe);
    pass.set_bind_group(0, &bg, &[]);
    pass.dispatch_workgroups(groups.0.max(1), groups.1.max(1), 1);
}

fn pixel_groups(w: u32, h: u32) -> (u32, u32) {
    (w.div_ceil(16), h.div_ceil(16))
}

fn storage_buf(g: &GpuContext, size: u64, label: &str) -> wgpu::Buffer {
    g.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size: size.max(16),
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

/// Read a storage buffer back (submits and waits).
fn read_buffer(e: &mut Enc, buf: &wgpu::Buffer, size: u64) -> Option<Vec<u8>> {
    if !e.g.can_wait() {
        return None;
    }
    buffer_bytes(size, 1, e.g.device.limits().max_buffer_size)?;
    let row = usize::try_from(size).ok()?;
    if size > buf.size() || !size.is_multiple_of(wgpu::COPY_BUFFER_ALIGNMENT) {
        return None;
    }
    let rb = e.g.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("adv3d readback"),
        size,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    e.record(&[]).copy_buffer_to_buffer(buf, 0, &rb, 0, size);
    e.submit();
    let out = e.g.read_buffer(&rb, row, row, 1).map_err(|e| log::error!("gpu 3d readback: {e}")).ok()?;
    Some(out)
}

/// A resolved run on the GPU: premultiplied linear (or working-space) colour and camera depth
/// (`BIG` where nothing was drawn), one entry per output pixel.
pub(crate) struct Resolved {
    acc: wgpu::Buffer,
    depth: wgpu::Buffer,
    width: u32,
    height: u32,
}

/// The rasteriser's "nothing drawn" depth on the GPU (∞ on the CPU).
const BIG: f32 = 3.0e38;

/// Rasterise every motion-blur sub-sample of a prepared run, resolve and average them, then
/// apply the depth of field (`render_prepared`'s steps). `None` = can't run here (sizes,
/// limits, or a depth of field without readback); `Some(None)` = nothing drawn.
pub(crate) fn resolve_run(e: &mut Enc, prep: &Prepared) -> Option<Option<Resolved>> {
    let (w, h) = prep.out;
    let g = e.g;
    if !g.fits(w, h) || (prep.dof.is_some() && !g.can_wait()) {
        return None;
    }
    let px = w as u64 * h as u64;
    let limits = g.device.limits();
    let max = limits.max_storage_buffer_binding_size.min(limits.max_buffer_size);
    let acc_bytes = buffer_bytes(px, 16, max)?;
    let depth_bytes = buffer_bytes(px, 4, max)?;
    let acc = storage_buf(g, acc_bytes, "adv3d acc");
    let depth = storage_buf(g, depth_bytes, "adv3d depth");
    let n = prep.samples.max(1);
    let weight = if n <= 1 { 1.0 } else { 1.0 / n as f32 };
    let mut first = true;
    for i in 0..n {
        let s = prep.scene(i);
        if s.indices.is_empty() {
            continue;
        }
        if Some(s.width) != w.checked_mul(s.ssaa.max(1)) || Some(s.height) != h.checked_mul(s.ssaa.max(1)) {
            return None;
        }
        let (color, zout) = raster_into(e, &s)?;
        let (cv, zv) = (color.create_view(&Default::default()), zout.create_view(&Default::default()));
        let b = Binds { color: Some(&cv), z: Some(&zv), bufs: [Some(&acc), Some(&depth), None, None, None, None, None], out: None };
        dispatch(e, "resolve", [w, h, s.ssaa.max(1), first as u32], [weight, 0.0, 0.0, 0.0], b, pixel_groups(w, h));
        first = false;
        // One sub-sample's targets alive at a time.
        e.submit();
    }
    if first {
        return Some(None);
    }
    if let Some(dof) = &prep.dof {
        depth_of_field(e, &acc, &depth, (w, h), dof, prep.scale)?;
    }
    Some(Some(Resolved { acc, depth, width: w, height: h }))
}

/// `adv::depth_of_field` on the resolved buffers: radii, highlights, prefix sums, the iris
/// gather at the levels around each pixel's radius. The result replaces `acc`.
fn depth_of_field(e: &mut Enc, acc: &wgpu::Buffer, depth: &wgpu::Buffer, (w, h): (u32, u32), dof: &Dof, scale: f64) -> Option<()> {
    let g = e.g;
    let px = w as u64 * h as u64;
    let limits = g.device.limits();
    let max = limits.max_storage_buffer_binding_size.min(limits.max_buffer_size);
    let prefix_bytes = buffer_bytes((u64::from(w) + 1).checked_mul(u64::from(h))?, 16, max)?;
    buffer_bytes(1, 16, max)?;
    let radius = storage_buf(g, px * 4, "adv3d radius");
    let mut init = Vec::with_capacity(16);
    for v in [f32::INFINITY.to_bits(), 0, 0, 0] {
        init.extend_from_slice(&v.to_le_bytes());
    }
    let range = g.buffer_init(Some("adv3d radius range"), &init, wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC);
    let f0 = [dof.focus as f32, dof.aperture as f32, dof.blur_level as f32, scale as f32];
    let b = Binds { bufs: [None, Some(depth), None, None, Some(&radius), Some(&range), None], ..Default::default() };
    dispatch(e, "radius_of", [w, h, 0, 0], f0, b, pixel_groups(w, h));
    let rb = read_buffer(e, &range, 16)?;
    let lo = f32::from_bits(u32::from_le_bytes([rb[0], rb[1], rb[2], rb[3]]));
    let hi = f32::from_bits(u32::from_le_bytes([rb[4], rb[5], rb[6], rb[7]]));
    if hi < 0.5 {
        return Some(());
    }
    let Some(levels) = bokeh::blur_levels(&[lo, hi]) else { return Some(()) };
    // Level headers, then every level's weighted spans.
    let mut heads: Vec<[f32; 4]> = vec![];
    let mut spans: Vec<[f32; 4]> = vec![];
    for &r in &levels {
        match bokeh::kernel_spans(&dof.iris, r as f64) {
            Some((kernels, norm)) => {
                let first = levels.len() + spans.len();
                for (wt, sp) in &kernels {
                    spans.extend(sp.iter().map(|&(dy, x0, x1)| [dy as f32, x0 as f32, x1 as f32, *wt]));
                }
                let count = levels.len() + spans.len() - first;
                heads.push([first as f32, count as f32, 1.0 / norm, 0.0]);
            }
            None => heads.push([0.0, 0.0, 0.0, 1.0]),
        }
    }
    let table: Vec<u8> = heads.iter().chain(&spans).flatten().flat_map(|x| x.to_le_bytes()).collect();
    if table.len() as u64 > max {
        return None;
    }
    let table = e.bytes(table);
    let src = storage_buf(g, px * 16, "adv3d dof source");
    let prefix = storage_buf(g, prefix_bytes, "adv3d prefix");
    let hl = dof.highlight;
    let b = Binds { bufs: [Some(acc), None, Some(&src), None, Some(&radius), None, None], ..Default::default() };
    dispatch(e, "boost", [w, h, 0, 0], [hl.gain as f32, hl.threshold as f32, hl.saturation as f32, 0.0], b, pixel_groups(w, h));
    let b = Binds { bufs: [None, None, Some(&src), None, None, None, Some(&prefix)], ..Default::default() };
    dispatch(e, "prefix_rows", [w, h, 0, 0], [0.0; 4], b, (h.div_ceil(64), 1));
    let b = Binds { bufs: [Some(acc), None, Some(&src), Some(&table), Some(&radius), None, Some(&prefix)], ..Default::default() };
    dispatch(e, "gather", [w, h, levels.len() as u32, 0], [lo, hi, 0.0, 0.0], b, pixel_groups(w, h));
    e.submit();
    Some(())
}

/// The resolved run as an image in the canvas's encoding, composited over `canvas` when given.
pub(crate) fn finish(e: &mut Enc, r: &Resolved, encode: bool, canvas: Option<&GpuImage>) -> GpuImage {
    canvas.inspect(|c| e.uses(c));
    let out = e.image(r.width, r.height);
    let ov = out.texture.create_view(&Default::default());
    let cv = canvas.map(|c| c.texture.create_view(&Default::default()));
    let b = Binds { color: cv.as_ref(), bufs: [Some(&r.acc), None, None, None, None, None, None], out: Some(&ov), ..Default::default() };
    dispatch(e, "finish", [r.width, r.height, encode as u32, canvas.is_some() as u32], [0.0; 4], b, pixel_groups(r.width, r.height));
    out
}

/// [`effectcraft_render::Accelerator::render_3d`]: the whole prepared run on the GPU, read
/// back.
pub(crate) fn render_prepared(g: &GpuContext, prep: &Prepared) -> Option<Rendered> {
    if !g.can_readback() {
        return None;
    }
    if let Some(d) = g.deferred.clone() {
        return render_prepared_deferred(g, &d, prep);
    }
    let mut e = Enc::new(g);
    let Some(r) = resolve_run(&mut e, prep)? else { return Some(None) };
    let img = finish(&mut e, &r, !prep.linear, None);
    let img = e.download(&img)?;
    let px = r.width as u64 * r.height as u64;
    let db = read_buffer(&mut e, &r.depth, px * 4)?;
    let depth = db
        .as_chunks::<4>()
        .0
        .iter()
        .map(|c| {
            let z = f32::from_le_bytes([c[0], c[1], c[2], c[3]]);
            if z >= BIG * 0.5 { f32::INFINITY } else { z }
        })
        .collect();
    Some(Some((img, depth)))
}

/// [`render_prepared`] with deferred readbacks: the image and the depth are read back under the
/// run's key (every sub-sample's scene, the output size and encoding). A depth of field needs a
/// readback in the middle of the run: such runs go to the CPU path (whose scenes still
/// rasterise here, [`render`]).
fn render_prepared_deferred(g: &GpuContext, d: &std::sync::Arc<crate::deferred::Deferred>, prep: &Prepared) -> Option<Rendered> {
    use crate::deferred::Lookup::*;
    if prep.dof.is_some() {
        return None;
    }
    let (w, h) = prep.out;
    let size = buffer_bytes(u64::from(w).checked_mul(u64::from(h))?, 4, g.device.limits().max_buffer_size)?;
    let row = usize::try_from(size).ok()?;
    let mut k = crate::deferred::Key::new(5);
    k.u64(w as u64);
    k.u64(h as u64);
    k.u64(prep.samples as u64);
    k.f64(prep.scale);
    k.u64(prep.linear as u64);
    let mut any = false;
    for i in 0..prep.samples.max(1) {
        let s = prep.scene(i);
        any |= !s.indices.is_empty();
        key_scene(&mut k, &s);
    }
    if !any {
        return Some(None);
    }
    let ki = k.finish();
    let kz = ki ^ 1;
    match (d.lookup(ki), d.lookup(kz)) {
        (Ready(a), Ready(b)) => {
            let (a, b) = (a?, b?);
            let mut img = effectcraft_render::Image::new(a.width, a.height);
            let dst: &mut [u8] = bytemuck::cast_slice_mut(&mut img.data);
            if dst.len() != a.bytes.len() || b.bytes.len() != dst.len() / 4 {
                return None;
            }
            dst.copy_from_slice(&a.bytes);
            let depth = b
                .bytes
                .as_chunks::<4>()
                .0
                .iter()
                .map(|c| {
                    let z = f32::from_le_bytes([c[0], c[1], c[2], c[3]]);
                    if z >= BIG * 0.5 { f32::INFINITY } else { z }
                })
                .collect();
            return Some(Some((img, depth)));
        }
        (Absent, Absent) => {}
        _ => {
            d.miss();
            return Some(None);
        }
    }
    let mut e = Enc::new(g);
    let Some(r) = resolve_run(&mut e, prep)? else { return Some(None) };
    let img = finish(&mut e, &r, !prep.linear, None);
    let _ = e.read_keyed(&img, ki, [0.0; 2], 1.0);
    let rb = g.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("adv3d depth readback"),
        size,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    e.record(&[]).copy_buffer_to_buffer(&r.depth, 0, &rb, 0, size);
    e.submit();
    let done = d.start(kz, w, h, [0.0; 2], 1.0);
    g.map_readback(&rb, row, row, 1, move |r| done(r.map_err(|e| log::error!("gpu depth readback: {e}")).ok()));
    #[cfg(not(target_arch = "wasm32"))]
    let _ = g.poll_readbacks(wgpu::PollType::Poll);
    d.miss();
    Some(None)
}

/// Draw a prepared run over the GPU canvas (the compositor's walk; no readback unless the
/// depth of field needs its radius range). `None` = render it on the CPU.
pub(crate) fn draw_run(e: &mut Enc, prep: &Prepared, canvas: &GpuImage) -> Option<GpuImage> {
    if (canvas.width, canvas.height) != prep.out {
        return None;
    }
    match resolve_run(e, prep)? {
        None => Some(canvas.clone()),
        Some(r) => Some(finish(e, &r, !prep.linear, Some(canvas))),
    }
}

/// A layer rendered on its own (`iso`), finished, and hidden where the run's main scene is
/// nearer (`adv::draw_run`'s 2D compositing path).
pub(crate) fn occluded(e: &mut Enc, iso: &Resolved, main: &Resolved, encode: bool) -> GpuImage {
    let img = finish(e, iso, encode, None);
    let (w, h) = (iso.width, iso.height);
    let out = e.image(w, h);
    let (iv, ov) = (img.texture.create_view(&Default::default()), out.texture.create_view(&Default::default()));
    let b =
        Binds { color: Some(&iv), bufs: [None, Some(&iso.depth), Some(&main.acc), None, Some(&main.depth), None, None], out: Some(&ov), ..Default::default() };
    dispatch(e, "occlude", [w, h, 0, 0], [0.0; 4], b, pixel_groups(w, h));
    out
}

/// Kernels of `sky.wgsl` (the shared compute module).
pub(crate) const SKY_KERNELS: &[&str] = &["adv_sky"];

/// An Environment Light Background layer drawn over `canvas` (`three_d::compose::draw_sky`).
pub(crate) fn sky(e: &mut Enc, sky: &effectcraft_render::three_d::SkyDraw, canvas: &GpuImage) -> Option<GpuImage> {
    let img = e.g.upload_buf(&sky.buf)?;
    let f3 = |v: [f64; 3], w: f64| [v[0] as f32, v[1] as f32, v[2] as f32, w as f32];
    let mut p = crate::context::Params::default();
    p.f[0] = f3(sky.fwd, sky.zoom);
    p.f[1] = f3(sky.right, sky.ortho as u32 as f64);
    p.f[2] = [sky.down[0] as f32, sky.down[1] as f32, sky.down[2] as f32, sky.rotation];
    let s = sky.scale;
    p.f[3] = [(sky.roi.0 / s - sky.view.0 / 2.0) as f32, (sky.roi.1 / s - sky.view.1 / 2.0) as f32, (1.0 / s) as f32, sky.opacity];
    p.f[4] = [img.width as f32, img.height as f32, 0.0, 0.0];
    let out = e.scratch(canvas.width, canvas.height);
    e.pixels("adv_sky", &p, canvas, Some(&img), &out, None);
    Some(out)
}

/// Wireframe-quality layer outlines: `canvas` with `pixels` set to opaque white
/// ([`effectcraft_render::Renderer::wireframe_pixels`]).
pub(crate) fn wireframe(e: &mut Enc, canvas: &GpuImage, pixels: &[(u32, u32)]) -> GpuImage {
    e.uses(canvas);
    let out = e.image(canvas.width, canvas.height);
    e.copy_into(canvas, &out, 0, 0);
    if pixels.is_empty() {
        return out;
    }
    let table: Vec<u8> = pixels.iter().flat_map(|&(x, y)| [x as f32, y as f32, 0.0, 0.0]).flat_map(f32::to_le_bytes).collect();
    let table = e.bytes(table);
    let ov = out.texture.create_view(&Default::default());
    let b = Binds { bufs: [None, None, None, Some(&table), None, None, None], out: Some(&ov), ..Default::default() };
    let n = pixels.len() as u32;
    dispatch(e, "wire", [n, 0, 0, 0], [0.0; 4], b, (n.div_ceil(64), 1));
    out
}
