//! The display pass on a real device (any wgpu adapter; CI runs lavapipe), held to its CPU
//! reference `DisplayPass::pixel`. Skips without an adapter.

use std::sync::{Arc, Mutex};

use effectcraft_engine::project::HdrMode;
use effectcraft_engine::render::Image;
use effectcraft_engine::viewer::{Channel, DisplayColor, DisplayPass, FrameEncoding, PassCurve, SimProfile, Simulation};
use eframe::egui_wgpu;
use eframe::wgpu;

use super::{DisplayGpu, Source};
use crate::frames::F16Frame;

/// One device at a time (tests run in parallel).
static DEVICE: Mutex<()> = Mutex::new(());

struct Dev {
    instance: wgpu::Instance,
    adapter: wgpu::Adapter,
    device: wgpu::Device,
    queue: wgpu::Queue,
    /// A software rasterizer (lavapipe / llvmpipe, as CI has): its pow / exp / division round
    /// differently again, so exact comparisons allow 1 LSB on a few bytes there.
    cpu: bool,
}

fn device() -> Option<Dev> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle().with_env());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default())).ok()?;
    let (device, queue) =
        pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor { label: Some("display pass tests"), ..Default::default() })).ok()?;
    let info = adapter.get_info();
    eprintln!("display pass tests on {:?} ({:?})", info.name, info.device_type);
    // (macOS CI runners' Metal is a virtual "Apple Paravirtual device": it rounds like a software one)
    let cpu = info.device_type == wgpu::DeviceType::Cpu || info.name.contains("Paravirtual");
    Some(Dev { cpu, instance, adapter, device, queue })
}

/// `compare`'s result is exact, or (software rasterizers) at most 1 LSB on at most 1 % of bytes.
fn exact_enough(d: &Dev, (n, max): (usize, i32), bytes: usize) -> bool {
    (n, max) == (0, 0) || d.cpu && max <= 1 && n * 100 <= bytes
}

/// egui-wgpu's render state on the test device, as eframe hands it to the app.
fn render_state(d: &Dev) -> egui_wgpu::RenderState {
    let renderer = egui_wgpu::Renderer::new(&d.device, wgpu::TextureFormat::Rgba8Unorm, egui_wgpu::RendererOptions::default());
    egui_wgpu::RenderState {
        adapter: d.adapter.clone(),
        available_adapters: vec![d.adapter.clone()],
        instance: d.instance.clone(),
        device: d.device.clone(),
        queue: d.queue.clone(),
        target_format: wgpu::TextureFormat::Rgba8Unorm,
        renderer: Arc::new(egui::mutex::RwLock::new(renderer)),
    }
}

/// A 17³ LUT that mixes channels and bends the curve (as an ICC profile's device does).
fn test_lut(k: f64) -> Arc<effectcraft_engine::color::icc::Lut3d> {
    Arc::new(effectcraft_engine::color::icc::Lut3d::bake(17, |c| {
        [c[0].powf(1.3) * 0.9 + c[1] * 0.1, c[1] * 0.8 + c[2] * 0.2, (c[2] * 0.7 + c[0] * 0.3).sqrt() * k]
    }))
}

fn read(d: &Dev, tex: &wgpu::Texture, level: u32) -> (Vec<[u8; 4]>, [u32; 2]) {
    let (w, h) = ((tex.width() >> level).max(1), (tex.height() >> level).max(1));
    let stride = (w * 4).div_ceil(256) * 256;
    let buf = d.device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: (stride * h) as u64,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut e = d.device.create_command_encoder(&Default::default());
    e.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo { texture: tex, mip_level: level, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
        wgpu::TexelCopyBufferInfo { buffer: &buf, layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(stride), rows_per_image: Some(h) } },
        wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
    );
    d.queue.submit([e.finish()]);
    buf.slice(..).map_async(wgpu::MapMode::Read, |r| r.expect("map"));
    d.device.poll(wgpu::PollType::wait_indefinitely()).expect("poll");
    let data = buf.slice(..).get_mapped_range().expect("mapped").to_vec();
    let mut out = Vec::with_capacity((w * h) as usize);
    for y in 0..h {
        let row = &data[(y * stride) as usize..][..(w * 4) as usize];
        out.extend(row.as_chunks::<4>().0.iter().copied());
    }
    (out, [w, h])
}

/// The pass on the device: level 0 of the displayed texture, and the texture.
fn bake(d: &Dev, gpu: &DisplayGpu, src: &Source, pass: &DisplayPass) -> (Vec<[u8; 4]>, wgpu::Texture) {
    let k = crate::frames::fit_factor(src.size(), gpu.max_side(), 1);
    let input = gpu.upload(&d.device, &d.queue, src, k, None);
    let out = DisplayGpu::output(&d.device, input.width(), input.height());
    let lut = match &pass.color {
        Some(effectcraft_engine::viewer::PassColor::Lut(l)) => Some(gpu.lut(&d.device, &d.queue, l)),
        _ => None,
    };
    gpu.run(&d.device, &d.queue, &input, &out, &super::uniform(pass), lut.as_ref());
    (read(d, &out, 0).0, out)
}

fn frame(px: &[[f32; 4]], w: u32, encoding: FrameEncoding) -> Arc<F16Frame> {
    let mut img = Image::new(w, px.len() as u32 / w);
    img.data.copy_from_slice(px);
    Arc::new(F16Frame::from_image(&img, encoding))
}

/// GPU output against the reference on the frame's own (half-float) values: the number of
/// bytes that differ and the largest difference.
fn compare(got: &[[u8; 4]], f: &F16Frame, pass: &DisplayPass) -> (usize, i32) {
    let (mut n, mut max) = (0, 0);
    for (i, g) in got.iter().enumerate() {
        let want = pass.pixel(f.pixel(i).expect("pixel"));
        for c in 0..4 {
            let d = (g[c] as i32 - want[c] as i32).abs();
            if d > 0 {
                n += 1;
                max = max.max(d);
            }
        }
    }
    (n, max)
}

const SRGB: FrameEncoding = FrameEncoding { curve: PassCurve::Srgb, hdr: HdrMode::Clip };

fn grey(l: f32) -> [f32; 4] {
    let e = PassCurve::Srgb.encode(l);
    [e, e, e, 1.0]
}

/// The issue's values (4.0 at -2 EV is white, 0.5 at +1 EV is white) and the HDR golden table,
/// bit for bit on the GPU.
#[test]
fn hdr_values_on_the_gpu() {
    let _g = DEVICE.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some(d) = device() else { return eprintln!("no wgpu adapter: skipping") };
    let gpu = DisplayGpu::new(&d.device);
    let ls = [0.25f32, 0.5, 1.0, 2.0, 4.0, 16.0];
    let f = frame(&ls.map(grey), 6, SRGB);
    let want: [[u8; 6]; 5] = [
        [137, 188, 255, 255, 255, 255],
        [99, 137, 188, 255, 255, 255],
        [71, 99, 137, 188, 255, 255],
        [34, 49, 71, 99, 137, 255],
        [188, 255, 255, 255, 255, 255],
    ];
    for (stops, w) in [0.0f32, -1.0, -2.0, -4.0, 1.0].into_iter().zip(want) {
        let pass = DisplayPass::new(Channel::Rgb, false, stops, SRGB, None);
        let (got, _) = bake(&d, &gpu, &Source::F16(f.clone()), &pass);
        let r: Vec<u8> = got.iter().map(|p| p[0]).collect();
        assert!(r.iter().zip(w).all(|(g, w)| *g == w || d.cpu && g.abs_diff(w) <= 1), "{stops} EV: {r:?} vs {w:?}");
        let c = compare(&got, &f, &pass);
        assert!(c == (0, 0) || d.cpu && c.1 <= 1, "{stops} EV: {c:?}");
    }
}

/// Exposure 0 on half floats: bit for bit the reference, and the bytes the 8-bit viewer showed
/// for values in 0..1 (at least 97.9 % equal, never more than 1 apart: the half-float storage).
#[test]
fn exposure_zero_parity_on_the_gpu() {
    let _g = DEVICE.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some(d) = device() else { return eprintln!("no wgpu adapter: skipping") };
    let gpu = DisplayGpu::new(&d.device);
    let pass = DisplayPass::new(Channel::Rgb, false, 0.0, SRGB, None);
    for alpha in [1.0f32, 0.5] {
        let img = crate::frames::tests::parity_frame(alpha);
        let f = Arc::new(F16Frame::from_image(&img, SRGB));
        let (got, _) = bake(&d, &gpu, &Source::F16(f.clone()), &pass);
        let c = compare(&got, &f, &pass);
        assert!(exact_enough(&d, c, got.len() * 4), "alpha {alpha}: {c:?}");
        let old = crate::frames::to_color_image(&img);
        let (mut same, mut max) = (0, 0);
        for (g, o) in got.iter().zip(&old.pixels) {
            let o = o.to_array();
            for c in 0..3 {
                same += (g[c] == o[c]) as usize;
                max = max.max((g[c] as i32 - o[c] as i32).abs());
            }
            assert_eq!(g[3], o[3]);
        }
        let share = same as f64 / (got.len() * 3) as f64;
        eprintln!("GPU, alpha {alpha}: {:.2} % of colour bytes as before, max {max}", share * 100.0);
        assert!(share >= 0.979 && max <= 1, "alpha {alpha}: {share}, max {max}");
    }
    // The issue's two values, exactly.
    let f = frame(&[grey(4.0), grey(0.5)], 2, SRGB);
    let (m2, _) = bake(&d, &gpu, &Source::F16(f.clone()), &DisplayPass::new(Channel::Rgb, false, -2.0, SRGB, None));
    let (p1, _) = bake(&d, &gpu, &Source::F16(f), &DisplayPass::new(Channel::Rgb, false, 1.0, SRGB, None));
    let white = |p: [u8; 4]| p.iter().all(|c| *c == 255 || d.cpu && *c >= 254);
    assert!(white(m2[0]) && white(p1[1]), "{:?} {:?}", m2[0], p1[1]);
}

/// Every pass setting the viewer has, against the reference on the same half-float input:
/// exposure, HDR step (after exposure), Show Channel, curves and display colour.
#[test]
fn the_shader_is_the_reference() {
    let _g = DEVICE.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some(d) = device() else { return eprintln!("no wgpu adapter: skipping") };
    let gpu = DisplayGpu::new(&d.device);
    // 64 × 64: colours 0..2 in every channel, alpha 1, 0.5 and a few others, some over range.
    let px: Vec<[f32; 4]> = (0..64 * 64)
        .map(|i| {
            let (x, y) = (i % 64, i / 64);
            let a = [1.0, 0.5, 0.25, 0.8, 1.0, 1.0, 0.0, 1.0][y % 8];
            // Row 5: negative values too (32 bpc frames can hold them).
            let lo = if y % 8 == 5 { 0.75 } else { 0.0 };
            let v = |k: usize| ((x * 7 + y * k) % 64) as f32 / 32.0 - lo;
            [v(3) * a, v(5) * a, v(11) * a, a]
        })
        .collect();
    let dcs = [
        None,
        DisplayColor::new(
            Some(effectcraft_engine::project::ColorSpace::Srgb),
            false,
            effectcraft_engine::project::ColorSpace::Srgb,
            true,
            effectcraft_engine::project::ColorSpace::DisplayP3,
            Simulation::default(),
            &Default::default(),
        ),
        DisplayColor::new(
            Some(effectcraft_engine::project::ColorSpace::Srgb),
            false,
            effectcraft_engine::project::ColorSpace::Srgb,
            true,
            effectcraft_engine::project::ColorSpace::Srgb,
            Simulation { profile: SimProfile::Rec709, preserve_rgb: false },
            &Default::default(),
        ),
        DisplayColor::new(
            Some(effectcraft_engine::project::ColorSpace::Srgb),
            false,
            effectcraft_engine::project::ColorSpace::Srgb,
            true,
            effectcraft_engine::project::ColorSpace::DisplayP3,
            Simulation { profile: SimProfile::Mac18, preserve_rgb: true },
            &Default::default(),
        ),
        // A LUT-based My Custom RGB profile.
        Some(DisplayColor::from_lut(test_lut(1.0))),
    ];
    let (mut report, mut off, mut total) = (vec![], 0usize, 0usize);
    for curve in [PassCurve::Srgb, PassCurve::Gamma(2.4), PassCurve::Linear, PassCurve::Pq, PassCurve::Hlg] {
        for hdr in [HdrMode::Clip, HdrMode::Compand, HdrMode::ToneMap] {
            let enc = FrameEncoding { curve, hdr };
            let f = frame(&px, 64, enc);
            for stops in [0.0f32, 1.0, -2.0, 2.5] {
                for (ci, dc) in dcs.iter().enumerate() {
                    for (channel, colorized) in
                        [(Channel::Rgb, false), (Channel::RgbStraight, false), (Channel::Red, false), (Channel::Green, true), (Channel::Alpha, false)]
                    {
                        if ci > 0 && !matches!(channel, Channel::Rgb) {
                            continue;
                        }
                        let pass = DisplayPass::new(channel, colorized, stops, enc, dc.as_ref());
                        let (got, _) = bake(&d, &gpu, &Source::F16(f.clone()), &pass);
                        let (n, max) = compare(&got, &f, &pass);
                        // The device's pow / exp / ln / division are not the CPU's to the last
                        // bit: a value on a rounding tie can land either side of it.
                        // PQ's exponent of 78.8 magnifies those last bits most.
                        // Software rasterizers (CI's lavapipe) differ more.
                        let share = if matches!(curve, PassCurve::Pq | PassCurve::Hlg) { 5 } else { 1 } * if d.cpu { 5 } else { 1 };
                        assert!(max <= 1 && n * 100 <= got.len() * 4 * share, "{curve:?} {hdr:?} {stops} EV {channel:?} dc {ci}: {n} bytes off, max {max}");
                        off += n;
                        total += got.len() * 4;
                        if n > 0 {
                            report.push(format!("{curve:?} {hdr:?} {stops} {channel:?} {ci}: {n}"));
                        }
                    }
                }
            }
        }
    }
    eprintln!("{off} of {total} bytes 1 LSB off the reference: {report:?}");
}

/// 8-bit frames go through the same pass (exposure before the display colour there too), equal
/// to the CPU pass on their bytes.
#[test]
fn rgba8_frames_match_the_cpu_pass() {
    let _g = DEVICE.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some(d) = device() else { return eprintln!("no wgpu adapter: skipping") };
    let gpu = DisplayGpu::new(&d.device);
    let px: Vec<egui::Color32> = (0..256 * 16)
        .map(|i| {
            let (x, y) = (i % 256, i / 256);
            let a = [255u8, 128, 64, 200][y % 4];
            let c = |k: usize| ((x * k + y) % 256) as u32 * a as u32 / 255;
            egui::Color32::from_rgba_premultiplied(c(1) as u8, c(3) as u8, c(7) as u8, a)
        })
        .collect();
    let img = Arc::new(egui::ColorImage::new([256, 16], px));
    let dc = DisplayColor::new(
        Some(effectcraft_engine::project::ColorSpace::Srgb),
        false,
        effectcraft_engine::project::ColorSpace::Srgb,
        true,
        effectcraft_engine::project::ColorSpace::DisplayP3,
        Simulation::default(),
        &Default::default(),
    );
    for stops in [0.0f32, 1.0, -1.5] {
        for dc in [None, dc.as_ref()] {
            let pass = DisplayPass::new(Channel::Rgb, false, stops, SRGB, dc);
            let (got, _) = bake(&d, &gpu, &Source::Rgba8(img.clone()), &pass);
            let mut want: Vec<[u8; 4]> = img.pixels.iter().map(|c| c.to_array()).collect();
            pass.apply_rgba8(&mut want);
            let off = got.iter().zip(&want).flat_map(|(g, w)| (0..4).map(move |c| (g[c] as i32 - w[c] as i32).abs())).filter(|d| *d > 0).collect::<Vec<_>>();
            eprintln!("8-bit, {stops} EV, dc {}: {} bytes 1 LSB off", dc.is_some(), off.len());
            assert!(off.iter().all(|d| *d <= 1) && off.len() * 100 <= got.len() * 4, "{stops} EV, dc {}: {off:?}", dc.is_some());
            if stops == 0.0 && dc.is_none() {
                assert_eq!(got, want, "nothing to change: the frame's bytes");
            }
        }
    }
}

/// The mips: each texel the rounded mean of the 2 × 2 texels above it, and at the edge of an
/// odd-sized level above also of its last column / row, so no frame pixel is dropped.
#[test]
fn mips_average_the_level_above() {
    let _g = DEVICE.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some(d) = device() else { return eprintln!("no wgpu adapter: skipping") };
    let gpu = DisplayGpu::new(&d.device);
    let px: Vec<[f32; 4]> = (0..37 * 21).map(|i| [(i % 37) as f32 / 36.0, (i / 37) as f32 / 20.0, 0.3, 1.0]).collect();
    let f = frame(&px, 37, SRGB);
    let (_, tex) = bake(&d, &gpu, &Source::F16(f), &DisplayPass::new(Channel::Rgb, false, 0.5, SRGB, None));
    assert_eq!(tex.mip_level_count(), 6);
    for l in 1..tex.mip_level_count() {
        let (above, [aw, ah]) = read(&d, &tex, l - 1);
        let (got, [w, h]) = read(&d, &tex, l);
        for y in 0..h {
            for x in 0..w {
                let (x0, y0) = ((x * 2).min(aw - 1), (y * 2).min(ah - 1));
                let x1 = if x == w - 1 { aw } else { (x * 2 + 2).min(aw) };
                let y1 = if y == h - 1 { ah } else { (y * 2 + 2).min(ah) };
                let n = ((x1 - x0) * (y1 - y0)) as f32;
                let want: [u8; 4] = std::array::from_fn(|c| {
                    let s: f32 =
                        (y0..y1).flat_map(|yy| (x0..x1).map(move |xx| (xx, yy))).map(|(xx, yy)| above[(yy * aw + xx) as usize][c] as f32 / 255.0).sum();
                    (s / n * 255.0 + 0.5).floor() as u8
                });
                let g = got[(y * w + x) as usize];
                assert!((0..4).all(|c| (g[c] as i32 - want[c] as i32).abs() <= 1), "level {l} ({x}, {y}): {g:?} vs {want:?}");
            }
        }
    }
}

/// One egui frame through egui_wgpu's renderer into an offscreen target (eframe minus the
/// swapchain): RGBA8 of the framebuffer.
fn render_egui(
    d: &Dev,
    renderer: &mut egui_wgpu::Renderer,
    ctx: &egui::Context,
    size_pts: [f32; 2],
    ppp: f32,
    ui_fn: &mut dyn FnMut(&mut egui::Ui),
) -> (Vec<[u8; 4]>, [u32; 2]) {
    ctx.set_pixels_per_point(ppp);
    let raw = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(size_pts[0], size_pts[1]))), ..Default::default() };
    let mut first = ctx.run_ui(raw.clone(), |ui| ui_fn(ui));
    let mut out = ctx.run_ui(raw, |ui| ui_fn(ui));
    for o in [&mut first, &mut out] {
        for (id, delta) in o.textures_delta.set.iter().flat_map(|(id, v)| v.iter().map(move |d| (id, d))) {
            renderer.update_texture(&d.device, &d.queue, *id, delta);
        }
        for id in &o.textures_delta.free {
            renderer.free_texture(id);
        }
        o.textures_delta.clear();
    }
    let jobs = ctx.tessellate(out.shapes, out.pixels_per_point);
    let size = [(size_pts[0] * ppp).round() as u32, (size_pts[1] * ppp).round() as u32];
    let sd = egui_wgpu::ScreenDescriptor { size_in_pixels: size, pixels_per_point: out.pixels_per_point };
    let target = d.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("framebuffer"),
        size: wgpu::Extent3d { width: size[0], height: size[1], depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = target.create_view(&Default::default());
    let mut enc = d.device.create_command_encoder(&Default::default());
    let user = renderer.update_buffers(&d.device, &d.queue, &mut enc, &jobs, &sd);
    {
        let pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("egui"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store },
            })],
            ..Default::default()
        });
        let mut pass = pass.forget_lifetime();
        renderer.render(&mut pass, &jobs, &sd);
    }
    d.queue.submit(user.into_iter().chain([enc.finish()]));
    (read(d, &target, 0).0, size)
}

/// The displayed texture drawn by egui as the viewer draws it (native texture, the Faster
/// sampler: nearest): at 2 pixels per point, zoomed 8× and 13.37× with the frame past every
/// edge of a clip rect, each screen pixel shows the texel under its centre; and over egui's
/// checkerboard, half-transparent pixels blend premultiplied.
#[test]
fn the_displayed_texture_lands_where_the_viewer_draws_it() {
    let _g = DEVICE.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some(d) = device() else { return eprintln!("no wgpu adapter: skipping") };
    let gpu = DisplayGpu::new(&d.device);
    let mut renderer =
        egui_wgpu::Renderer::new(&d.device, wgpu::TextureFormat::Rgba8Unorm, egui_wgpu::RendererOptions { dithering: false, ..Default::default() });
    let ctx = egui::Context::default();
    let (tw, th) = (64u32, 32u32);
    let px: Vec<[f32; 4]> = (0..tw * th).map(|i| [(i % tw) as f32 / 63.0, (i / tw) as f32 / 31.0, 1.0, 1.0]).collect();
    let f = frame(&px, tw, SRGB);
    let pass = DisplayPass::new(Channel::Rgb, false, -1.0, SRGB, None);
    let (texels, tex) = bake(&d, &gpu, &Source::F16(f), &pass);
    let id =
        renderer.register_native_texture_with_sampler_options(&d.device, &tex.create_view(&Default::default()), crate::panels::viewer::zoom_sampler(false));
    let ppp = 2.0f32;
    let clip = egui::Rect::from_min_max(egui::pos2(50.0, 40.0), egui::pos2(350.0, 260.0));
    for (zoom, origin) in [(2.0f32, egui::pos2(80.0, 60.0)), (8.0, egui::pos2(-100.0, -60.0)), (13.37, egui::pos2(-151.3, -77.7))] {
        let rect = egui::Rect::from_min_size(origin, egui::vec2(tw as f32 * zoom, th as f32 * zoom));
        let mut ui_fn = |ui: &mut egui::Ui| {
            let p = ui.painter().with_clip_rect(clip);
            p.image(id, rect, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), egui::Color32::WHITE);
        };
        let (out, sz) = render_egui(&d, &mut renderer, &ctx, [400.0, 300.0], ppp, &mut ui_fn);
        let (mut bad, mut drawn) = (0, 0);
        for y in 0..sz[1] {
            for x in 0..sz[0] {
                let (cx, cy) = ((x as f32 + 0.5) / ppp, (y as f32 + 0.5) / ppp);
                let (u, v) = ((cx - rect.min.x) / zoom, (cy - rect.min.y) / zoom);
                // Centres within a hair of a texel or clip edge can go either way.
                let near = |t: f32| (t - t.round()).abs() < 0.01;
                if near(u)
                    || near(v)
                    || near(cx * ppp - clip.min.x * ppp)
                    || !clip.contains(egui::pos2(cx, cy))
                    || u < 0.0
                    || v < 0.0
                    || u >= tw as f32
                    || v >= th as f32
                {
                    continue;
                }
                drawn += 1;
                let want = texels[(v as u32 * tw + u as u32) as usize];
                let got = out[(y * sz[0] + x) as usize];
                if (0..4).any(|c| (got[c] as i32 - want[c] as i32).abs() > 1) {
                    bad += 1;
                }
            }
        }
        assert!(drawn > 1000, "{drawn}");
        assert_eq!(bad, 0, "zoom {zoom}, origin {origin:?}: {bad} of {drawn} pixels wrong");
    }
    renderer.free_texture(&id);
    // Alpha 0.5 over a checkerboard.
    let src = [0.5f32, 0.25, 0.0, 0.5];
    let f = frame(&[src; 256], 16, SRGB);
    let (texels, tex) = bake(&d, &gpu, &Source::F16(f), &DisplayPass::new(Channel::Rgb, false, 0.0, SRGB, None));
    let id =
        renderer.register_native_texture_with_sampler_options(&d.device, &tex.create_view(&Default::default()), crate::panels::viewer::zoom_sampler(false));
    let (c1, c2) = (egui::Color32::from_gray(0x66), egui::Color32::from_gray(0x99));
    let mut ui_fn = |ui: &mut egui::Ui| {
        let p = ui.painter();
        for j in 0..8 {
            for i in 0..8 {
                let r = egui::Rect::from_min_size(egui::pos2(i as f32 * 8.0, j as f32 * 8.0), egui::vec2(8.0, 8.0));
                p.rect_filled(r, 0.0, if (i + j) % 2 == 0 { c1 } else { c2 });
            }
        }
        p.image(
            id,
            egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(64.0, 64.0)),
            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            egui::Color32::WHITE,
        );
    };
    let (out, sz) = render_egui(&d, &mut renderer, &ctx, [64.0, 64.0], 2.0, &mut ui_fn);
    let s = texels[0];
    let mut bad = 0;
    for y in 0..sz[1] {
        for x in 0..sz[0] {
            let (i, j) = (x / 16, y / 16);
            let dst = if (i + j) % 2 == 0 { 0x66 } else { 0x99 } as f32;
            let want: [u8; 3] = std::array::from_fn(|c| (s[c] as f32 + dst * (1.0 - s[3] as f32 / 255.0)).round() as u8);
            let got = out[(y * sz[0] + x) as usize];
            if (0..3).any(|c| (got[c] as i32 - want[c] as i32).abs() > 1) {
                bad += 1;
            }
        }
    }
    assert_eq!(bad, 0, "alpha over the checkerboard: {bad} pixels off");
}

/// Timings at 4K (`cargo test -- --ignored display_pass_timings --nocapture`): the f32 → half
/// conversion of a frame, today's f32 → RGBA8 conversion, and the display pass.
#[test]
#[ignore = "timing, not a check"]
fn display_pass_timings() {
    let _g = DEVICE.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let img = Image::filled(3840, 2160, [0.7, 1.9, 0.2, 1.0]);
    let time = |f: &mut dyn FnMut()| {
        let mut v: Vec<f64> = (0..9)
            .map(|_| {
                let t = std::time::Instant::now();
                f();
                t.elapsed().as_secs_f64() * 1000.0
            })
            .collect();
        v.sort_by(f64::total_cmp);
        v[4]
    };
    let f16 = time(&mut || drop(F16Frame::from_image(&img, SRGB)));
    let rgba8 = time(&mut || drop(crate::frames::to_color_image(&img)));
    eprintln!("4K: f32 → f16 {f16:.2} ms, f32 → RGBA8 {rgba8:.2} ms (medians of 9)");
    let Some(d) = device() else { return };
    let gpu = DisplayGpu::new(&d.device);
    let f = Arc::new(F16Frame::from_image(&img, SRGB));
    let input = gpu.upload(&d.device, &d.queue, &Source::F16(f), 1, None);
    let out = DisplayGpu::output(&d.device, 3840, 2160);
    let params = super::uniform(&DisplayPass::new(Channel::Rgb, false, -1.0, FrameEncoding { curve: PassCurve::Srgb, hdr: HdrMode::ToneMap }, None));
    let pass = time(&mut || {
        gpu.run(&d.device, &d.queue, &input, &out, &params, None);
        d.device.poll(wgpu::PollType::wait_indefinitely()).expect("poll");
    });
    let f = Arc::new(F16Frame::from_image(&img, SRGB));
    let mut kept = Some(gpu.upload(&d.device, &d.queue, &Source::F16(f.clone()), 1, None));
    let upload = time(&mut || {
        kept = Some(gpu.upload(&d.device, &d.queue, &Source::F16(f.clone()), 1, kept.take()));
        d.device.poll(wgpu::PollType::wait_indefinitely()).expect("poll");
    });
    let cpu = time(&mut || drop(f.to_color_image(&DisplayPass::new(Channel::Rgb, false, -1.0, SRGB, None))));
    eprintln!("4K: half-float upload into the kept texture {upload:.2} ms; the pass on the CPU (no device) {cpu:.2} ms");
    eprintln!("4K: display pass with mips, submit to idle {pass:.2} ms");
}

/// `Bake::show` on egui-wgpu's render state: it runs the pass again when the frame, the pass's
/// settings or the LUT change (a new LUT of the same size included: another ICC profile,
/// Preserve RGB, another display), and only then; the texture id stays.
#[test]
fn bakes_run_again_when_frame_pass_or_lut_change() {
    let _g = DEVICE.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some(d) = device() else { return eprintln!("no wgpu adapter: skipping") };
    let gpu = DisplayGpu::new(&d.device);
    let rs = render_state(&d);
    let px: Vec<[f32; 4]> = (0..16).map(|i| [i as f32 / 15.0, 0.4, 0.7, 1.0]).collect();
    let f = frame(&px, 4, SRGB);
    let lut = |k: f64| DisplayColor::from_lut(test_lut(k));
    let (l1, l2) = (lut(1.0), lut(0.5));
    let pass = |stops: f32, dc: &DisplayColor| DisplayPass::new(Channel::Rgb, false, stops, SRGB, Some(dc));
    let mut b = super::Bake::default();
    let id = b.show(&rs, &gpu, &Source::F16(f.clone()), &pass(0.0, &l1), true).unwrap();
    let first = read(&d, b.texture().unwrap(), 0).0;
    assert_eq!(b.runs(), 1);
    b.show(&rs, &gpu, &Source::F16(f.clone()), &pass(0.0, &l1), true);
    assert_eq!(b.runs(), 1, "nothing changed");
    assert_eq!(b.show(&rs, &gpu, &Source::F16(f.clone()), &pass(0.0, &l2), true), Some(id));
    assert_eq!(b.runs(), 2, "a new LUT of the same size");
    let second = read(&d, b.texture().unwrap(), 0).0;
    assert_ne!(first, second);
    let want: Vec<[u8; 4]> = (0..16).map(|i| pass(0.0, &l2).pixel(f.pixel(i).unwrap())).collect();
    assert!(second.iter().zip(&want).all(|(g, w)| (0..4).all(|c| g[c].abs_diff(w[c]) <= 1)), "{second:?} vs {want:?}");
    b.show(&rs, &gpu, &Source::F16(f.clone()), &pass(-1.0, &l2), true);
    assert_eq!(b.runs(), 3, "another exposure");
    b.show(&rs, &gpu, &Source::F16(frame(&px, 4, SRGB)), &pass(-1.0, &l2), true);
    assert_eq!(b.runs(), 4, "another frame");
    b.free(Some(&rs));
    assert!(b.id().is_none());
}
