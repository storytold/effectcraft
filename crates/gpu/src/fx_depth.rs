//! GPU effects, 3D Channel family (kernels in `shaders/fx_depth.wgsl`, every entry point
//! prefixed `fxd_`): 3D Channel Extract, Cryptomatte, Depth Matte, Depth of Field, EXtractoR,
//! Fog 3D, ID Matte and IDentifier.
//!
//! The layer's auxiliary channels ([`effectcraft_raster::AuxChannels`]) go to the GPU as an
//! extra texture at the aux resolution: the planes an effect reads are packed into its four
//! channels (alpha = 1 marks "inside the aux image"). `fxd_lookup` resamples it nearest-neighbour
//! onto the buffer grid with the CPU's exact index arithmetic (layer position × aux scale,
//! floored), and the effect kernels work on that buffer-sized plane. Cryptomatte uploads its
//! finished output instead of the raw ranks (#3334): the matte as one R32F plane (the Colors
//! display: its RGBA pixels), made on the CPU from the unscaled channels at source resolution,
//! box-filtered below render scale 1 and kept in [`matte_cache`]. Fog 3D's Gradient Layer is
//! rendered by the host on the CPU (it is another layer) and uploaded.

use std::sync::Arc;

use effectcraft_effects::{Buf, CryptoOutput, CryptoPlane, EffectCtx};
use effectcraft_keyframe::Value;
use effectcraft_raster::channels3d::BACKGROUND_DEPTH;
use effectcraft_raster::{AuxChannels, Image};

use crate::context::{Enc, GpuImage, Params};
use crate::effects::{GBuf, gaussian_blur};

pub(crate) mod matte_cache;

/// Compute entry points in `fx_depth.wgsl`.
pub(crate) const KERNELS: &[&str] = &["fxd_lookup", "fxd_point", "fxd_dof_f", "fxd_dof_level", "fxd_avg"];

/// Effect ids implemented here.
pub(crate) const IDS: &[&str] = &[
    "ec.3d.channelextract",
    "ec.3d.cryptomatte",
    "ec.3d.depthmatte",
    "ec.3d.depthoffield",
    "ec.3d.extractor",
    "ec.3d.fog3d",
    "ec.3d.idmatte",
    "ec.3d.identifier",
];

/// Run effect `id` (one of [`IDS`]); `None` = this parameter combination runs on the CPU.
pub(crate) fn apply(e: &mut Enc, id: &str, ctx: &EffectCtx, b: GBuf) -> Option<GBuf> {
    let aux = ctx.env.host.and_then(|h| h.aux());
    if id == "ec.3d.extractor" {
        return extractor(e, ctx, b, aux.as_deref());
    }
    let Some(a) = aux else { return Some(b) };
    match id {
        "ec.3d.channelextract" => channel_extract(e, ctx, b, &a),
        "ec.3d.cryptomatte" => cryptomatte(e, ctx, b, &a),
        "ec.3d.depthmatte" => depth_matte(e, ctx, b, &a),
        "ec.3d.depthoffield" => depth_of_field(e, ctx, b, &a),
        "ec.3d.fog3d" => fog(e, ctx, b, &a),
        "ec.3d.idmatte" => id_matte(e, ctx, b, &a),
        "ec.3d.identifier" => identifier(e, ctx, b, &a),
        _ => None,
    }
}

fn op(code: u32) -> Params {
    let mut p = Params::default();
    p.u[0][0] = code;
    p
}

/// Pack up to four aux planes (aux resolution; `None` planes are 0, alpha defaults to 1 = inside)
/// and upload them.
fn aux_texture(e: &mut Enc, a: &AuxChannels, planes: [Option<&[f32]>; 4]) -> Option<GpuImage> {
    let n = (a.width * a.height) as usize;
    if planes.iter().flatten().any(|p| p.len() < n) {
        return None;
    }
    let mut img = Image::new(a.width, a.height);
    for (i, px) in img.data.iter_mut().enumerate() {
        *px = [0, 1, 2, 3].map(|k| planes[k].map_or(if k == 3 { 1.0 } else { 0.0 }, |p| p[i]));
    }
    e.g.upload_image(&img)
}

/// Resample an aux texture (`tex_scale` texels per layer pixel) onto the buffer grid (nearest;
/// `fill` outside the aux image), sampling each buffer pixel at sub-pixel position `at`.
fn lookup(e: &mut Enc, b: &GBuf, tex_scale: f64, tex: &GpuImage, at: [f64; 2], fill: [f32; 4]) -> GpuImage {
    let inv = 1.0 / b.scale.max(1e-9);
    let mut p = Params::default();
    p.f[0] = [b.offset[0] as f32, b.offset[1] as f32, inv as f32, tex_scale as f32];
    p.f[1] = [at[0] as f32, at[1] as f32, 0.0, 0.0];
    p.f[2] = fill;
    let out = e.scratch(b.img.width, b.img.height);
    e.pixels("fxd_lookup", &p, tex, None, &out, None);
    out
}

fn point(e: &mut Enc, p: &Params, b: GBuf, plane: &GpuImage) -> Option<GBuf> {
    let out = e.scratch(b.img.width, b.img.height);
    e.pixels("fxd_point", p, &b.img, Some(plane), &out, None);
    Some(GBuf { img: out, ..b })
}

// ---------------------------------------------------------------- 3D Channel Extract

fn channel_extract(e: &mut Enc, ctx: &EffectCtx, b: GBuf, a: &AuxChannels) -> Option<GBuf> {
    let pr = ctx.params;
    let get3 = |n: [&[&str]; 3]| -> Option<[&[f32]; 3]> { Some([a.find(n[0])?, a.find(n[1])?, a.find(n[2])?]) };
    let u = ["UV.U", "U", "uv.x"];
    let v = ["UV.V", "V", "uv.y"];
    // (planes, mode): mode 0 grey (lv), 1 UV (lv on R, G; B = 0), 2 normals (lv of ×0.5 + 0.5),
    // 3 RGB (lv), 4 unclamped RGB (no mapping).
    let (planes, mode): ([&[f32]; 3], u32) = match pr.e("channel") {
        0 => (a.depth().map(|z| [z, z, z])?, 0),
        1 => (a.object_id().map(|c| [c, c, c])?, 0),
        7 => (a.material_id().map(|c| [c, c, c])?, 0),
        4 => (a.coverage().map(|c| [c, c, c])?, 0),
        2 => (get3([&u, &v, &["UV.W", "W", "uv.z"]]).or_else(|| Some([a.find(&u)?, a.find(&v)?, a.find(&v)?]))?, 1),
        3 => (get3([&["N.X", "Normal.X", "normals.x"], &["N.Y", "Normal.Y", "normals.y"], &["N.Z", "Normal.Z", "normals.z"]])?, 2),
        5 => (get3([&["BG.R", "Background.R"], &["BG.G", "Background.G"], &["BG.B", "Background.B"]])?, 3),
        _ => (get3([&["R"], &["G"], &["B"]])?, 4),
    };
    let tex = aux_texture(e, a, [Some(planes[0]), Some(planes[1]), Some(planes[2]), None])?;
    let mut p = op(0);
    p.u[0] = [0, mode, pr.b("clampOutput") as u32, pr.b("invertDepthMap") as u32];
    p.f[0] = [pr.f("blackPoint") as f32, pr.f("whitePoint") as f32, 0.0, 0.0];
    if !pr.b("antialias") {
        let plane = lookup(e, &b, a.scale, &tex, [0.5, 0.5], [0.0; 4]);
        return point(e, &p, b, &plane);
    }
    // Anti-alias: four sub-pixel samples, summed in order and scaled once (as the CPU does).
    let (w, h) = (b.img.width, b.img.height);
    let mut acc: Option<GpuImage> = None;
    for s in [[0.25, 0.25], [0.75, 0.25], [0.25, 0.75], [0.75, 0.75]] {
        let plane = lookup(e, &b, a.scale, &tex, s, [0.0; 4]);
        let one = e.scratch(w, h);
        e.pixels("fxd_point", &p, &b.img, Some(&plane), &one, None);
        acc = Some(match acc {
            None => one,
            Some(prev) => {
                let out = e.scratch(w, h);
                e.pixels("fxd_avg", &op(0), &prev, Some(&one), &out, None);
                out
            }
        });
    }
    let acc = acc?;
    let out = e.scratch(w, h);
    e.pixels("fxd_avg", &op(1), &acc, None, &out, None);
    Some(GBuf { img: out, ..b })
}

// ---------------------------------------------------------------- Depth Matte / Fog 3D

fn depth_plane(e: &mut Enc, b: &GBuf, a: &AuxChannels) -> Option<GpuImage> {
    let z = a.depth()?;
    let tex = aux_texture(e, a, [Some(z), None, None, None])?;
    Some(lookup(e, b, a.scale, &tex, [0.5, 0.5], [BACKGROUND_DEPTH, 0.0, 0.0, 0.0]))
}

fn depth_matte(e: &mut Enc, ctx: &EffectCtx, b: GBuf, a: &AuxChannels) -> Option<GBuf> {
    let Some(plane) = depth_plane(e, &b, a) else { return if a.depth().is_none() { Some(b) } else { None } };
    let mut p = op(1);
    p.u[0][1] = ctx.params.b("invert") as u32;
    p.f[0] = [ctx.params.f("depth") as f32, ctx.params.f("feather").max(0.0) as f32, 0.0, 0.0];
    point(e, &p, b, &plane)
}

fn fog(e: &mut Enc, ctx: &EffectCtx, b: GBuf, a: &AuxChannels) -> Option<GBuf> {
    let pr = ctx.params;
    let Some(plane) = depth_plane(e, &b, a) else { return if a.depth().is_none() { Some(b) } else { None } };
    let contrib = (pr.f("layerContribution") / 100.0) as f32;
    let grad = if contrib > 0.0 && matches!(pr.get("gradientLayer"), Some(Value::Layer(Some(_)))) {
        ctx.layer_param("gradientLayer", true).map(|o| {
            let cpu = Buf { img: Image::new(b.img.width, b.img.height), offset: b.offset, scale: b.scale };
            effectcraft_effects::util::fit_layer(ctx, &cpu, &o, true)
        })
    } else {
        None
    };
    let grad = match grad {
        Some(g) => Some(e.g.upload_image(&g)?),
        None => None,
    };
    let fc = pr.color("fogColor");
    let mut p = op(2);
    p.u[0] = [2, pr.b("foggyBackground") as u32, grad.is_some() as u32, 0];
    p.f[0] = [pr.f("fogStartDepth") as f32, pr.f("fogEndDepth") as f32, (pr.f("fogOpacity") / 100.0) as f32, (pr.f("scatteringDensity") / 100.0) as f32];
    p.f[1] = [fc[0], fc[1], fc[2], contrib];
    p.f[2][0] = BACKGROUND_DEPTH;
    let Some(g) = grad else { return point(e, &p, b, &plane) };
    // The gradient's factor joins the depth plane (G) first: kernels read two images.
    let mut q = op(3);
    q.f[0][0] = contrib;
    let merged = e.scratch(b.img.width, b.img.height);
    e.pixels("fxd_point", &q, &g, Some(&plane), &merged, None);
    point(e, &p, b, &merged)
}

// ---------------------------------------------------------------- Depth of Field

fn depth_of_field(e: &mut Enc, ctx: &EffectCtx, b: GBuf, a: &AuxChannels) -> Option<GBuf> {
    let pr = ctx.params;
    if a.depth().is_none() {
        return Some(b);
    }
    let max_r = (pr.f("maximumRadius").max(0.0) * b.scale) as f32;
    if max_r <= 0.0 {
        return Some(b);
    }
    let plane = depth_plane(e, &b, a)?;
    const LEVELS: usize = 5;
    let (w, h) = (b.img.width, b.img.height);
    let stack: Vec<GpuImage> = (0..LEVELS)
        .map(|k| {
            let r = max_r * k as f32 / (LEVELS - 1) as f32;
            if r < 0.3 { b.img.clone() } else { gaussian_blur(e, &b.img, (r / 2.0) as f64, (r / 2.0) as f64, true) }
        })
        .collect();
    // Per-pixel stack position, then each level pair writes its own pixels.
    let mut p = Params::default();
    let gamma = 2f32.powf((50.0 - pr.f("focalBias") as f32) / 25.0);
    p.f[0] = [pr.f("focalPlane") as f32, pr.f("focalPlaneThickness").max(0.0) as f32, max_r, gamma];
    p.u[0][0] = LEVELS as u32;
    let fpl = e.scratch(w, h);
    e.pixels("fxd_dof_f", &p, &plane, None, &fpl, None);
    let (rows, row_len) = e.image_rows(&fpl);
    let out = e.scratch(w, h);
    for k in 0..LEVELS - 1 {
        let mut q = Params::default();
        q.u[0] = [k as u32, row_len, LEVELS as u32, 0];
        e.pixels("fxd_dof_level", &q, &stack[k], Some(&stack[k + 1]), &out, Some(&rows));
    }
    Some(GBuf { img: out, ..b })
}

// ---------------------------------------------------------------- ID Matte / IDentifier

fn id_plane(a: &AuxChannels, which: u32) -> Option<&[f32]> {
    if which == 1 { a.material_id() } else { a.object_id() }
}

fn id_matte(e: &mut Enc, ctx: &EffectCtx, b: GBuf, a: &AuxChannels) -> Option<GBuf> {
    let pr = ctx.params;
    let Some(ids) = id_plane(a, pr.e("auxChannel")) else { return Some(b) };
    let cov = if pr.b("useCoverage") { a.coverage() } else { None };
    let tex = aux_texture(e, a, [Some(ids), cov, None, None])?;
    let plane = lookup(e, &b, a.scale, &tex, [0.5, 0.5], [0.0; 4]);
    let (w, h) = (b.img.width, b.img.height);
    let mut p = op(4);
    p.u[0][1] = cov.is_some() as u32;
    p.f[0][0] = pr.f("idSelection").round() as f32;
    let mut m = e.scratch(w, h);
    e.pixels("fxd_point", &p, &plane, None, &m, None);
    let feather = pr.f("feather").max(0.0) * b.scale;
    if feather > 0.0 {
        m = gaussian_blur(e, &m, feather * 0.5, feather * 0.5, true);
    }
    let mut p = op(5);
    p.u[0][1] = pr.b("invert") as u32;
    point(e, &p, b, &m)
}

fn identifier(e: &mut Enc, ctx: &EffectCtx, b: GBuf, a: &AuxChannels) -> Option<GBuf> {
    let pr = ctx.params;
    let Some(ids) = id_plane(a, pr.e("channelType")) else { return Some(b) };
    let max_id = ids.iter().cloned().fold(1.0f32, f32::max);
    let tex = aux_texture(e, a, [Some(ids), None, None, None])?;
    let plane = lookup(e, &b, a.scale, &tex, [0.5, 0.5], [0.0; 4]);
    let mut p = op(6);
    p.u[0][1] = pr.e("display");
    p.f[0] = [pr.f("id").round() as f32, max_id, 0.0, 0.0];
    point(e, &p, b, &plane)
}

// ---------------------------------------------------------------- Cryptomatte

fn cryptomatte(e: &mut Enc, ctx: &EffectCtx, b: GBuf, a: &Arc<AuxChannels>) -> Option<GBuf> {
    let pr = ctx.params;
    let Some(layer) = effectcraft_effects::crypto_layer_named(a, pr.s("layerName"), pr.e("layer") as usize) else { return Some(b) };
    let manifest = a.manifest(&layer);
    let sel = effectcraft_effects::selection_hashes(pr.s("selection"), manifest);
    // The finished output, made on the CPU from the unscaled ID and coverage channels at source
    // resolution and box-filtered below render scale 1 (#3334): the matte alone for Matte,
    // Preview and Matte Only, the finished pixels for Colors. Kept per footage frame, layer,
    // selection and output, so a frame shown again neither makes nor reads the ranks again.
    // `tests::fx_depth::cryptomatte_every_display_matches_the_cpu` guards this against the shader.
    let output = CryptoOutput::for_display(pr.e("display"), pr.b("matteOnly"));
    let finished = matte_cache::plane(a, &layer, &sel, output, b.scale);
    let tex = upload_plane(e, &finished)?;
    let plane = lookup(e, &b, finished.scale, &tex, [0.5, 0.5], [0.0; 4]);
    let mut p = op(7);
    p.u[0] = [7, pr.e("display"), pr.b("matteOnly") as u32, pr.b(effectcraft_effects::MATTE_ONLY_IN_ALPHA) as u32];
    point(e, &p, b, &plane)
}

/// Upload a finished Cryptomatte plane: the matte as one R32F channel (four bytes a pixel, not
/// the sixteen of a working image), the Colors display's pixels as RGBA. `None` when it does not
/// fit the device (the effect then runs on the CPU).
fn upload_plane(e: &Enc, p: &CryptoPlane) -> Option<GpuImage> {
    let g = e.g;
    if !g.fits(p.width, p.height) {
        return None;
    }
    let (format, bpp) = match p.output {
        CryptoOutput::Matte => (wgpu::TextureFormat::R32Float, 4),
        CryptoOutput::Colors => (wgpu::TextureFormat::Rgba32Float, 16),
    };
    let n = (p.width as usize).checked_mul(p.height as usize)?.checked_mul(p.output.channels())?;
    if p.data.len() != n {
        return None;
    }
    let size = wgpu::Extent3d { width: p.width, height: p.height, depth_or_array_layers: 1 };
    let texture = g.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("Cryptomatte plane"),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    g.queue.write_texture(
        wgpu::TexelCopyTextureInfo { texture: &texture, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
        bytemuck::cast_slice(&p.data),
        wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(p.width.checked_mul(bpp)?), rows_per_image: Some(p.height) },
        size,
    );
    #[cfg(test)]
    PLANE_UPLOADS.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push((p.output, p.width, p.height, p.bytes()));
    Some(GpuImage::new(texture, p.width, p.height))
}

/// The Cryptomatte planes uploaded so far (output, width, height, bytes; tests).
#[cfg(test)]
pub(crate) static PLANE_UPLOADS: std::sync::Mutex<Vec<(CryptoOutput, u32, u32, usize)>> = std::sync::Mutex::new(Vec::new());

// ---------------------------------------------------------------- EXtractoR

fn extractor(e: &mut Enc, ctx: &EffectCtx, b: GBuf, a: Option<&AuxChannels>) -> Option<GBuf> {
    let pr = ctx.params;
    let names = [pr.s("red"), pr.s("green"), pr.s("blue"), pr.s("alpha")];
    let (bp, wp) = (pr.f("blackPoint") as f32, pr.f("whitePoint") as f32);
    let unmult = pr.b("unMult");
    // `(copy)`: the input's own channel of the same position, never an aux channel.
    let own = |k: usize, n: &str| effectcraft_effects::extractor_own_channel(k, n).and_then(|c| u32::try_from(c).ok());
    let src_ch: Vec<Option<&[f32]>> = names.iter().map(|n| a.and_then(|a| effectcraft_effects::extractor_aux_channel(a, n))).collect();
    if src_ch.iter().all(Option::is_none) && bp == 0.0 && wp == 1.0 && !unmult && names.iter().enumerate().all(|(k, n)| own(k, n) == Some(k as u32)) {
        return Some(b);
    }
    // Per output channel: 0..3 = the layer's own channel, 4 = aux, 5 = default (0, alpha 1).
    let sources: [u32; 4] = std::array::from_fn(|k| if src_ch[k].is_some() { 4 } else { own(k, names[k]).unwrap_or(5) });
    let plane = match a {
        Some(a) if src_ch.iter().any(Option::is_some) => {
            let tex = aux_texture(e, a, [src_ch[0], src_ch[1], src_ch[2], src_ch[3]])?;
            Some(lookup(e, &b, a.scale, &tex, [0.5, 0.5], [0.0; 4]))
        }
        _ => None,
    };
    let mut p = op(8);
    p.u[0] = [8, unmult as u32, pr.b("clip") as u32, 0];
    p.u[1] = sources;
    p.f[0] = [bp, wp, 0.0, 0.0];
    let out = e.scratch(b.img.width, b.img.height);
    e.pixels("fxd_point", &p, &b.img, plane.as_ref(), &out, None);
    Some(GBuf { img: out, ..b })
}
