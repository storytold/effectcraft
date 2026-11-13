//! #3333: a footage frame uploads once per frame and its texture is shared by every layer and
//! effect input that shows it; the frame is bit-identical to drawing each layer from its own copy.

use std::collections::HashMap;
use std::sync::Mutex;

use super::*;
use crate::walk::SHARED_PICTURES;

/// Keeps decoded frames as the media pool does: the same frame (at the same size) of the same
/// footage is the same `Arc` while kept. Counts the frames it decodes.
#[derive(Default)]
struct Kept {
    frames: Mutex<HashMap<(ItemId, u32, u32, i64, Option<(u32, u32)>), Arc<Image>>>,
}

impl Kept {
    fn get(&self, item: ItemId, f: &Footage, t: Tick, size: Option<(u32, u32)>) -> Option<Arc<Image>> {
        let i = f.frame_rate.frame_at(t);
        let mut m = self.frames.lock().unwrap();
        let img = m.entry((item, f.width, f.height, i, size)).or_insert_with(|| {
            let full = pattern(item.0 as u32 + i as u32 * 7 + f.width, f.width, f.height);
            Arc::new(match size {
                Some((w, h)) => effectcraft_raster::resample(&full, w, h),
                None => full,
            })
        });
        Some(img.clone())
    }
}

impl FootageSource for Kept {
    fn frame(&self, item: ItemId, f: &Footage, t: Tick) -> Option<Arc<Image>> {
        self.get(item, f, t, None)
    }
    fn frame_at_size(&self, item: ItemId, f: &Footage, t: Tick, width: u32, height: u32) -> Option<Arc<Image>> {
        self.get(item, f, t, Some((width, height)))
    }
}

/// One footage item (a sequence: a new picture every frame) shown by `layers` layers, each
/// placed differently (the first fills the comp).
fn shared_scene(depth: BitDepth, layers: usize) -> (Scene, ItemId) {
    let mut s = Scene::new(depth);
    let f = Footage {
        path: "seq.exr".into(),
        kind: FootageKind::Sequence,
        width: 97,
        height: 61,
        pixel_aspect: 1.0,
        frame_rate: FrameRate::FPS_30,
        duration: Tick::from_seconds_f64(2.0),
        has_video: true,
        alpha: AlphaMode::Premultiplied,
        ..Default::default()
    };
    let fid = s.p.add_item("Sequence", Label::Lavender, None, ItemKind::Footage(f));
    for i in 0..layers {
        let mut l = build::layer(&mut s.p, &s.comp, "Sequence", LayerSource::Footage { item: fid }, (97, 61), None);
        if i > 0 {
            set(&mut l, "transform/rotation", Value::Scalar(11.0 * i as f64));
            set(&mut l, "transform/scale", Value::Vec3([70.0 + 9.0 * i as f64, 64.0, 100.0]));
            set(&mut l, "transform/position", v3(30.0 + 13.0 * i as f64, 25.0 + 4.0 * i as f64));
            set(&mut l, "transform/opacity", Value::Scalar(80.0));
            l.blend_mode = if i % 2 == 0 { BlendMode::Screen } else { BlendMode::Add };
        }
        s.push(l);
    }
    (s, fid)
}

/// The GPU frame with shared pictures on and off, and the number of uploads of each.
fn render_both(g: &crate::Gpu, s: &Scene, src: &Kept, opts: RenderOpts, t: Tick) -> (Image, Image, u64, u64) {
    let mut r = Renderer::new(&s.p, src, RenderOpts { backend: Backend::Gpu, ..opts });
    r.accel = Some(g);
    let before = g.ctx.transfer_stats().uploads;
    let on = g.render(&r, s.cid, t).expect("the GPU renders the frame");
    let uploads = g.ctx.transfer_stats().uploads - before;
    SHARED_PICTURES.store(false, std::sync::atomic::Ordering::Relaxed);
    let before = g.ctx.transfer_stats().uploads;
    let off = g.render(&r, s.cid, t);
    let copied = g.ctx.transfer_stats().uploads - before;
    SHARED_PICTURES.store(true, std::sync::atomic::Ordering::Relaxed);
    (on, off.expect("the GPU renders the frame from per-layer copies"), uploads, copied)
}

fn check_cpu(label: &str, s: &Scene, src: &Kept, opts: RenderOpts, t: Tick, gpu_img: &Image) {
    let cpu = Renderer::new(&s.p, src, RenderOpts { backend: Backend::Cpu, ..opts }).comp_frame_cpu(s.cid, t);
    let mut d = diff(&cpu, gpu_img, tolerance(s.p.settings.bit_depth));
    d.quantized = s.p.settings.bit_depth != BitDepth::Bpc32;
    check(label, Some(d), 0.0);
}

/// Three layers showing the same frame upload it once; a fourth that runs an effect on it
/// (resident on the GPU) reads the same texture as its input. Every frame of the sequence
/// uploads its own picture once.
#[test]
fn three_layers_of_one_footage_frame_upload_once() {
    let Some(g) = gpu() else { return };
    let src = Kept::default();
    let (mut s, fid) = shared_scene(BitDepth::Bpc32, 3);
    for k in 0..3 {
        let t = Tick::from_seconds_f64(k as f64 / 30.0);
        let (on, off, uploads, copied) = render_both(g, &s, &src, opts(), t);
        assert_eq!(uploads, 1, "frame {k}: three layers of one footage frame uploaded {uploads} times");
        assert_eq!(copied, 3, "frame {k}: per-layer copies upload once per layer");
        assert!(on.data == off.data, "frame {k}: the shared picture changed the frame");
        check_cpu(&format!("shared picture frame {k}"), &s, &src, opts(), t, &on);
    }
    // An effect input: the blur reads the shared texture. Alone, the blurred layer uploads
    // its picture (and whatever the blur itself uploads); the three plain layers showing the
    // same frame add no upload.
    let mut fx = build::layer(&mut s.p, &s.comp, "Blurred", LayerSource::Footage { item: fid }, (97, 61), None);
    set(&mut fx, "transform/position", v3(20.0, 20.0));
    s.effect(&mut fx, "ec.blur.gaussian", &[("blurriness", n(3.0))]);
    let (mut alone, _) = shared_scene(BitDepth::Bpc32, 0);
    alone.p = s.p.clone();
    alone.p.comp_mut(alone.cid).unwrap().layers = vec![fx.clone()].into();
    let (_, _, blur_uploads, _) = render_both(g, &alone, &src, opts(), Tick::from_seconds_f64(0.4));
    s.push(fx);
    let t = Tick::from_seconds_f64(0.5);
    let (on, off, uploads, _) = render_both(g, &s, &src, opts(), t);
    assert_eq!(uploads, blur_uploads, "three layers and an effect input of one footage frame uploaded {uploads} times, the effect layer alone {blur_uploads}");
    assert!(on.data == off.data, "the shared picture changed the frame with an effect input");
    check_cpu("shared picture with an effect input", &s, &src, opts(), t, &on);
}

/// The frame is bit-identical with shared pictures on and off wherever they apply or not: every
/// bit depth, colour management and linear blending, half resolution, a track matte, Preserve
/// Transparency, motion blur, masks, layer styles and a proxy.
#[test]
fn shared_pictures_do_not_change_the_frame() {
    let Some(g) = gpu() else { return };
    let src = Kept::default();
    let t = Tick::from_seconds_f64(0.2);
    for depth in [BitDepth::Bpc8, BitDepth::Bpc16, BitDepth::Bpc32] {
        let mut cases: Vec<(String, Scene, RenderOpts)> = vec![];
        cases.push(("plain".into(), shared_scene(depth, 3).0, opts()));
        cases.push(("half resolution".into(), shared_scene(depth, 3).0, RenderOpts { scale: 0.5, ..opts() }));
        for (ws, blend_linear) in [(Some(ColorSpace::Rec709), false), (None, true)] {
            let (mut s, _) = shared_scene(depth, 3);
            s.p.settings.working_space = ws;
            s.p.settings.blend_linear = blend_linear;
            cases.push((format!("colour {ws:?} linear blending {blend_linear}"), s, opts()));
        }
        {
            let (mut s, _) = shared_scene(depth, 3);
            let comp = s.p.comp_mut(s.cid).unwrap();
            comp.enable_motion_blur = true;
            let matte = comp.layers[0].id;
            comp.layers[1].track_matte = Some(TrackMatte { layer: matte, kind: MatteKind::Luma });
            comp.layers[2].preserve_transparency = true;
            comp.layers[2].switches.motion_blur = true;
            let pos = comp.layers[2].props.prop_mut("transform/position").unwrap();
            pos.keys = vec![Keyframe::new(Tick::ZERO, v3(10.0, 20.0)), Keyframe::new(Tick::from_seconds_f64(0.5), v3(90.0, 45.0))];
            s.comp.enable_motion_blur = true;
            cases.push(("matte, preserve transparency, motion blur".into(), s, opts()));
        }
        {
            let (mut s, _) = shared_scene(depth, 3);
            let mut next = s.p.next_id;
            let m = build::mask(&mut Ids(&mut next), "Mask 1", ShapePath::ellipse([40.0, 30.0], 60.0, 40.0), MaskMode::Add, [255, 255, 0]);
            let comp = s.p.comp_mut(s.cid).unwrap();
            comp.layers[0].props.sub_mut("masks").unwrap().children.push(m.into());
            effectcraft_project::styles::add_style(&mut comp.layers[1], &mut Ids(&mut next), "dropShadow", &s.comp.global_light, true).unwrap();
            s.p.next_id = next;
            cases.push(("mask and layer style".into(), s, opts()));
        }
        {
            let (mut s, fid) = shared_scene(depth, 3);
            let proxy = Footage { path: "proxy.png".into(), kind: FootageKind::Still, width: 48, height: 30, has_video: true, ..Default::default() };
            s.p.item_mut(fid).unwrap().proxy = Some(Box::new(effectcraft_project::Proxy { footage: proxy, enabled: true }));
            cases.push(("proxy".into(), s, opts()));
        }
        for (label, s, o) in cases {
            let (on, off, _, _) = render_both(g, &s, &src, o, t);
            assert!(on.data == off.data, "{label} {depth:?}: the shared picture changed the frame");
            check_cpu(&format!("{label} {depth:?}"), &s, &src, o, t, &on);
        }
    }
}

/// The picture cache keeps no texture of a picture the footage source dropped.
#[test]
fn picture_cache_drops_textures_of_dropped_pictures() {
    let Some(g) = gpu() else { return };
    let before = g.ctx.picture_entries();
    for i in 0..6 {
        let img = Arc::new(pattern(i, 64, 64));
        let up = g.ctx.upload_picture(&img).expect("upload");
        let again = g.ctx.upload_picture(&img).expect("upload");
        assert!(up.texture == again.texture, "the same picture uploaded twice");
        drop((up, again, img));
    }
    let kept = g.ctx.picture_entries();
    assert!(kept <= before + 1, "the picture cache kept {kept} textures of dropped pictures (had {before})");
}

/// Large uploads copy through reused staging buffers (rows padded to the copy alignment) with
/// the same pixels as `Queue::write_texture`; the buffers stay within their budget, and a
/// clear while one is in flight forgets it.
#[test]
fn staging_uploads_keep_pixels_and_reuse_buffers() {
    let Some(g) = gpu() else { return };
    let wait = || {
        let _ = g.ctx.poll_readbacks(wgpu::PollType::wait_indefinitely());
    };
    // 300 px: 4800-byte rows, padded to 4864.
    for (w, h) in [(300u32, 300u32), (512, 600)] {
        let before = g.ctx.upload_staging().copies();
        for i in 0..4 {
            let img = pattern(40 + i, w, h);
            let up = g.ctx.upload_image(&img).expect("upload");
            let mut e = crate::context::Enc::new(&g.ctx);
            let back = e.download(&up).expect("readback");
            assert!(back.data.iter().flatten().map(|v| v.to_bits()).eq(img.data.iter().flatten().map(|v| v.to_bits())), "{w}×{h} upload {i} changed pixels");
            drop((up, e));
            wait();
        }
        assert!(g.ctx.upload_staging().copies() - before >= 2, "{w}×{h}: no upload went through a reused staging buffer");
        assert!(g.ctx.upload_staging().bytes() <= 192 << 20);
    }
    // A clear with a copy in flight: nothing kept, and later uploads still work.
    let img = pattern(7, 300, 300);
    let up = g.ctx.upload_image(&img).expect("upload");
    g.ctx.upload_staging().clear();
    wait();
    assert_eq!(g.ctx.upload_staging().bytes(), 0, "a cleared staging kept its buffers");
    let mut e = crate::context::Enc::new(&g.ctx);
    assert!(e.download(&up).expect("readback").data == img.data);
    let again = g.ctx.upload_image(&img).expect("upload after a clear");
    assert!(e.download(&again).expect("readback").data == img.data);
}
