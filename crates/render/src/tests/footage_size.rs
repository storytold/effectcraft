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

/// #3271: an OpenEXR sequence's layer is cached per frame (keyed by its source time): the same
/// frame again is served from the layer cache, another frame has another key. Other sequences
/// stay in the media pool only.
#[test]
fn exr_sequence_layers_are_cached_per_frame() {
    for (ext, fetches) in [("exr", 1), ("png", 2)] {
        let (mut p, cid) = project(BitDepth::Bpc8, false);
        let Some(LayerSource::Footage { item }) = p.comp(cid).map(|c| c.layers[0].source.clone()) else { panic!("footage layer") };
        if let Some(ItemKind::Footage(f)) = p.item_mut(item).map(|it| &mut it.kind) {
            f.kind = FootageKind::Sequence;
            f.sequence = (1..=30).map(|i| format!("/shot/plate_{i:04}.{ext}")).collect();
            f.path = f.sequence[0].clone();
        }
        let key = |t: f64| {
            let c = p.comp(cid).unwrap();
            crate::cache::layer_key(&crate::EvalCtx::new(&p, cid, c, Tick::from_seconds_f64(t)), &c.layers[0], 0.5, false, false)
        };
        assert_eq!(key(0.0).is_some(), ext == "exr", "{ext}");
        if ext == "exr" {
            assert_ne!(key(0.0), key(0.1), "another frame, another key");
            assert_eq!(key(0.0), key(0.01), "the same frame");
        }
        let cache = crate::LayerCache::default();
        let src = Clip { sizes: Some(Mutex::default()) };
        let opts = RenderOpts { scale: 0.5, ..Default::default() };
        for _ in 0..2 {
            let mut r = Renderer::new(&p, &src, opts);
            r.cache = Some(&cache);
            r.comp_frame(cid, Tick::ZERO);
        }
        assert_eq!(src.sizes.unwrap().into_inner().unwrap().len(), fetches, "{ext}");
    }
}

/// #3271 review: with a viewer region of interest, a still from a source that reads whole frames
/// anyway (not by rows) is asked for whole and stays in the layer cache: a second render reads
/// nothing. A source that reads rows is asked for only the rows the region shows.
#[test]
fn a_source_that_reads_whole_frames_keeps_the_layer_cached_under_a_region() {
    struct Counting {
        reads: Mutex<Vec<Option<(i64, i64)>>>,
        by_rows: bool,
    }
    impl FootageSource for Counting {
        fn frame(&self, item: ItemId, f: &Footage, t: Tick) -> Option<Arc<Image>> {
            self.reads.lock().unwrap().push(None);
            Clip { sizes: None }.frame(item, f, t)
        }
        fn frame_rows(&self, item: ItemId, f: &Footage, t: Tick, _: Option<(u32, u32)>, _: crate::FloatCurve, rows: (i64, i64)) -> Option<Arc<Image>> {
            self.reads.lock().unwrap().push(Some(rows));
            Clip { sizes: None }.frame(item, f, t)
        }
        fn reads_rows(&self, _: &Footage, _: Tick) -> bool {
            self.by_rows
        }
    }
    let (mut p, cid) = project(BitDepth::Bpc8, false);
    let Some(LayerSource::Footage { item }) = p.comp(cid).map(|c| c.layers[0].source.clone()) else { panic!("footage layer") };
    if let Some(ItemKind::Footage(f)) = p.item_mut(item).map(|it| &mut it.kind) {
        f.kind = FootageKind::Still;
        f.path = "/still.png".into();
    }
    let opts = RenderOpts { roi: Some([0.0, 0.0, 96.0, 10.0]), ..Default::default() };
    for by_rows in [false, true] {
        let src = Counting { reads: Mutex::default(), by_rows };
        let cache = crate::LayerCache::default();
        for _ in 0..2 {
            let mut r = Renderer::new(&p, &src, opts);
            r.cache = Some(&cache);
            r.comp_frame(cid, Tick::ZERO);
        }
        let reads = src.reads.into_inner().unwrap();
        if by_rows {
            assert_eq!(reads.len(), 2, "by rows, each render: {reads:?}");
            assert!(reads.iter().all(|r| r.is_some_and(|(y0, y1)| y0 == 0 && y1 < 54)), "{reads:?}");
        } else {
            assert_eq!(reads, [None], "whole, once: the layer cache keeps it");
        }
    }
}
