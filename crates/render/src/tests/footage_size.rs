//! Footage at a reduced resolution: a source that resamples frames itself
//! ([`FootageSource::frame_at_size`], movies in `effectcraft-media`) renders what resampling the
//! full frame renders.

use std::sync::{Arc, Mutex};

use rayon::prelude::*;

use effectcraft_color::Label;
use effectcraft_project::build;
use effectcraft_project::{AlphaMode, BitDepth, Comp, Footage, FootageKind, ItemId, ItemKind, LayerSource, Project};
use effectcraft_time::{FrameRate, Tick};

use crate::{FootageSource, Image, RenderOpts, Renderer};

/// Footage with a textured frame; `sizes` (when set) resamples it itself and records the sizes
/// it was asked for.
struct Clip {
    sizes: Option<Mutex<Vec<(u32, u32)>>>,
}

impl FootageSource for Clip {
    fn frame(&self, _: ItemId, f: &Footage, _: Tick) -> Option<Arc<Image>> {
        let mut img = Image::new(f.width, f.height);
        img.rows_mut().for_each(|(y, row)| {
            for (x, p) in row.iter_mut().enumerate() {
                let s = (0.5 + 0.3 * (x as f32 * 0.37 + y as f32 * 0.11).sin()) * 0.9;
                *p = [s, 1.0 - s, s * 0.5, 1.0];
            }
        });
        Some(Arc::new(img))
    }
    fn frame_at_size(&self, item: ItemId, f: &Footage, t: Tick, w: u32, h: u32) -> Option<Arc<Image>> {
        let Some(sizes) = &self.sizes else { return self.frame(item, f, t) };
        sizes.lock().unwrap().push((w, h));
        Some(Arc::new(effectcraft_raster::resample(&*self.frame(item, f, t)?, w, h)))
    }
}

fn project(depth: BitDepth, invert_alpha: bool) -> (Project, ItemId) {
    let mut p = Project::default();
    p.settings.bit_depth = depth;
    let comp = Comp::new(96, 54, FrameRate::FPS_30, Tick::from_seconds_f64(1.0));
    let cid = p.add_item("Comp", Label::Sandstone, None, ItemKind::Comp(comp.clone().into()));
    let f = Footage {
        path: "clip.mp4".into(),
        kind: FootageKind::Video,
        width: 96,
        height: 54,
        pixel_aspect: 1.0,
        frame_rate: FrameRate::FPS_30,
        duration: Tick::from_seconds_f64(1.0),
        has_video: true,
        alpha: AlphaMode::Ignore,
        loop_count: 1,
        invert_alpha,
        ..Default::default()
    };
    let fid = p.add_item("clip", Label::Aqua, None, ItemKind::Footage(f));
    let l = build::layer(&mut p, &comp, "clip", LayerSource::Footage { item: fid }, (96, 54), None);
    p.comp_mut(cid).unwrap().layers.push(l);
    (p, cid)
}

#[test]
fn reduced_renders_take_footage_resampled_by_the_source() {
    for depth in [BitDepth::Bpc8, BitDepth::Bpc32] {
        let (p, cid) = project(depth, false);
        for (scale, asked) in [(1.0, None), (0.75, None), (0.5, Some((48, 27))), (1.0 / 3.0, Some((32, 18))), (0.25, Some((24, 14)))] {
            let opts = RenderOpts { scale, ..Default::default() };
            let plain = Renderer::new(&p, &Clip { sizes: None }, opts).comp_frame(cid, Tick::ZERO);
            let sized = Clip { sizes: Some(Mutex::default()) };
            let frame = Renderer::new(&p, &sized, opts).comp_frame(cid, Tick::ZERO);
            assert_eq!(frame, plain, "{depth:?} at {scale}");
            assert_eq!(sized.sizes.unwrap().into_inner().unwrap(), asked.into_iter().collect::<Vec<_>>(), "{depth:?} at {scale}");
        }
    }
    // Interpreted pixels (Invert Alpha) are interpreted at full size, then resampled.
    let (p, cid) = project(BitDepth::Bpc8, true);
    let sized = Clip { sizes: Some(Mutex::default()) };
    Renderer::new(&p, &sized, RenderOpts { scale: 0.5, ..Default::default() }).comp_frame(cid, Tick::ZERO);
    assert!(sized.sizes.unwrap().into_inner().unwrap().is_empty());
}

/// #1812: a still's layer is kept in the layer cache across times (its texture then uploads to
/// the GPU once, not every frame); a movie's is fetched again at every time.
#[test]
fn still_layers_are_cached_across_times_and_movies_are_not() {
    for (kind, fetches) in [(FootageKind::Still, 1), (FootageKind::Video, 3)] {
        let (mut p, cid) = project(BitDepth::Bpc8, false);
        let Some(LayerSource::Footage { item }) = p.comp(cid).map(|c| c.layers[0].source.clone()) else { panic!("footage layer") };
        if let Some(ItemKind::Footage(f)) = p.item_mut(item).map(|it| &mut it.kind) {
            f.kind = kind;
        }
        let cache = crate::LayerCache::default();
        let src = Clip { sizes: Some(Mutex::default()) };
        let opts = RenderOpts { scale: 0.5, ..Default::default() };
        for t in [0.0, 0.25, 0.5] {
            let t = Tick::from_seconds_f64(t);
            let mut r = Renderer::new(&p, &src, opts);
            r.cache = Some(&cache);
            let frame = r.comp_frame(cid, t);
            assert_eq!(frame, Renderer::new(&p, &src, opts).comp_frame(cid, t), "{kind:?} at {t:?}");
        }
        // (each uncached render above fetches once more)
        assert_eq!(src.sizes.unwrap().into_inner().unwrap().len(), fetches + 3, "{kind:?}");
    }
}

/// [`Clip`] that also keeps its frames as 8-bit levels, counting the times it's asked for them.
struct Clip8 {
    asked: std::sync::atomic::AtomicUsize,
}

impl FootageSource for Clip8 {
    fn frame(&self, item: ItemId, f: &Footage, t: Tick) -> Option<Arc<Image>> {
        let mut img = (*Clip { sizes: None }.frame(item, f, t)?).clone();
        // (partly transparent, premultiplied, as decoded with alpha)
        img.data.iter_mut().enumerate().for_each(|(i, p)| {
            let a = (i % 7) as f32 / 6.0;
            *p = [p[0] * a, p[1] * a, p[2] * a, a];
        });
        Some(Arc::new(img))
    }
    fn frame8(&self, item: ItemId, f: &Footage, t: Tick, _: crate::FloatCurve) -> Option<Arc<crate::disk_cache::Frame8>> {
        self.asked.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let img = self.frame(item, f, t)?;
        let rgba = img.data.iter().flat_map(|p| effectcraft_raster::quantize_px_u8(*p)).collect();
        Some(Arc::new(crate::disk_cache::Frame8 { width: img.width, height: img.height, rgba }))
    }
}

/// An 8 bpc render at Full takes plain footage as 8-bit levels and renders the same pixels to
/// the bit; other depths, reduced renders and interpreted pixels never ask for them.
#[test]
fn footage_kept_as_8_bit_levels_renders_the_same_pixels() {
    struct Float(Clip8);
    impl FootageSource for Float {
        fn frame(&self, item: ItemId, f: &Footage, t: Tick) -> Option<Arc<Image>> {
            self.0.frame(item, f, t)
        }
    }
    for (depth, invert, scale, used) in [
        (BitDepth::Bpc8, false, 1.0, true),
        (BitDepth::Bpc8, false, 0.75, true),
        (BitDepth::Bpc8, false, 0.5, false),
        (BitDepth::Bpc8, true, 1.0, false),
        (BitDepth::Bpc16, false, 1.0, false),
        (BitDepth::Bpc32, false, 1.0, false),
    ] {
        let (p, cid) = project(depth, invert);
        let opts = RenderOpts { scale, ..Default::default() };
        let q8 = Clip8 { asked: Default::default() };
        let frame = Renderer::new(&p, &q8, opts).comp_frame(cid, Tick::ZERO);
        let plain = Renderer::new(&p, &Float(Clip8 { asked: Default::default() }), opts).comp_frame(cid, Tick::ZERO);
        assert!(frame.data.iter().zip(&plain.data).all(|(a, b)| a.map(f32::to_bits) == b.map(f32::to_bits)), "{depth:?} {invert} {scale}");
        assert_eq!(q8.asked.into_inner() > 0, used, "{depth:?} {invert} {scale}");
    }
}

/// The 8-bit levels are read while the render records its reads: the movie is in the files the
/// frame (and a cached layer buffer) was made of. (Read outside, a buffer of a movie listed no
/// files and was still marked complete: changed on disk, it was served stale.)
#[test]
fn footage_kept_as_8_bit_levels_is_noted_as_read() {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    struct Noted {
        clip: Clip8,
        recording: AtomicBool,
        outside: AtomicUsize,
    }
    impl FootageSource for Noted {
        fn frame(&self, item: ItemId, f: &Footage, t: Tick) -> Option<Arc<Image>> {
            self.clip.frame(item, f, t)
        }
        fn frame8(&self, item: ItemId, f: &Footage, t: Tick, float: crate::FloatCurve) -> Option<Arc<crate::disk_cache::Frame8>> {
            if !self.recording.load(Ordering::SeqCst) {
                self.outside.fetch_add(1, Ordering::SeqCst);
            }
            self.clip.frame8(item, f, t, float)
        }
        // (as the media pool does: the files of what `f` reads go to `reads`)
        fn recording(&self, reads: &crate::reads::ReadLog, f: &mut dyn FnMut()) {
            reads.note(crate::reads::ReadFile::new("clip.mov", crate::reads::FileStamp::Native { len: 1, modified: Some(1) }));
            self.recording.store(true, Ordering::SeqCst);
            f();
            self.recording.store(false, Ordering::SeqCst);
        }
    }
    let (p, cid) = project(BitDepth::Bpc8, false);
    let src = Noted { clip: Clip8 { asked: Default::default() }, recording: AtomicBool::new(false), outside: AtomicUsize::new(0) };
    let log = crate::reads::ReadLog::new();
    let mut r = Renderer::new(&p, &src, RenderOpts::default());
    r.reads = Some(&log);
    r.comp_frame(cid, Tick::ZERO);
    assert!(src.clip.asked.load(Ordering::SeqCst) > 0, "the 8-bit levels were not asked for");
    assert_eq!(src.outside.load(Ordering::SeqCst), 0, "8-bit levels read outside the render's recording");
    assert!(log.files().files().iter().any(|f| &*f.path == "clip.mov"), "the movie is not in the frame's files");
}
