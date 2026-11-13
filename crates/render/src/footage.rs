use super::{Arc, AtomicBool, Footage, Image, ItemId, ItemKind, Ordering, Tick};
use crate::reads::ReadLog;

/// How a [`FootageSource`] hands over float footage (OpenEXR, float TIFF), whose values are
/// linear light: encoded with a curve, as the compositor takes footage, or as stored. Other
/// footage is decoded the same way whatever it says. Interpret Footage ▸ Preserve RGB always
/// reads the values as stored.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum FloatCurve {
    /// The sRGB curve (managed projects, and Working Gamma 2.2).
    #[default]
    Srgb,
    /// A pure 2.4 power curve (no working space, Working Gamma 2.4).
    Gamma24,
    /// The linear values as stored (Interpret As Linear Light encodes them afterwards).
    Linear,
}

/// Supplies decoded footage frames (implemented by the media layer).
pub trait FootageSource: Send + Sync {
    /// The frame of `item` at source time `t`, straight from the file (any size).
    fn frame(&self, item: ItemId, footage: &Footage, t: Tick) -> Option<Arc<Image>>;
    /// [`FootageSource::frame`] for a render that shows it at `width` × `height`, at most half its
    /// size: either that frame or, when the source can make it more cheaply than the whole frame
    /// (movies, see `effectcraft-media`), the frame already resampled to that size as
    /// [`effectcraft_raster::resample`] would.
    fn frame_at_size(&self, item: ItemId, footage: &Footage, t: Tick, _width: u32, _height: u32) -> Option<Arc<Image>> {
        self.frame(item, footage, t)
    }
    /// [`FootageSource::frame`] (`size: None`) or [`FootageSource::frame_at_size`] with float
    /// footage handed over as `float` says ([`FootageSource::frame`] uses [`FloatCurve::Srgb`]).
    /// Sources without float footage can leave the default, which ignores `float`.
    fn frame_with(&self, item: ItemId, footage: &Footage, t: Tick, size: Option<(u32, u32)>, float: FloatCurve) -> Option<Arc<Image>> {
        let _ = float;
        match size {
            Some((w, h)) => self.frame_at_size(item, footage, t, w, h),
            None => self.frame(item, footage, t),
        }
    }
    /// [`FootageSource::frame_with`]'s full-size frame as 8-bit levels: each pixel is
    /// [`effectcraft_raster::quantize_px_u8`] of the float pixel `frame_with` would give, so
    /// [`effectcraft_raster::expand_u8`] makes that frame quantised to 8 bpc, bit for bit, from a
    /// quarter of the memory (an 8 bpc render of 8-bit footage). `None` when the source has no
    /// cheaper way than the float frame; the renderer then takes that.
    fn frame8(&self, _item: ItemId, _footage: &Footage, _t: Tick, _float: FloatCurve) -> Option<Arc<crate::disk_cache::Frame8>> {
        None
    }
    /// `frames` interleaved stereo sample frames of `item`'s audio from source time `t` at
    /// `rate` Hz (`None` when unavailable). The first sample is `t.to_units_floor(rate)`;
    /// a sample boundary uses the earliest representable tick that selects that sample.
    fn audio(&self, _item: ItemId, _footage: &Footage, _t: Tick, _frames: usize, _rate: u32) -> Option<Vec<f32>> {
        None
    }
    /// Auxiliary 3D channels of `item` at source time `t` (multi-layer OpenEXR: depth, IDs,
    /// Cryptomatte, any named channel), with `scale` relative to the file's pixels. `None` when
    /// the footage has none.
    fn aux(&self, _item: ItemId, _footage: &Footage, _t: Tick) -> Option<Arc<effectcraft_raster::AuxChannels>> {
        None
    }
    /// The footage files (by [`Footage::path`]) whose channels effects of the project read
    /// through [`FootageSource::aux`] (EXtractoR, Cryptomatte and the other 3D Channel effects),
    /// replacing the set given before. A source that decodes a file's picture and its channels
    /// separately can decode both at once for these from the first frame on. Sources that don't
    /// ignore it.
    fn set_aux_footage(&self, _paths: &[String]) {}
    /// The parsed 3D model of a [`effectcraft_project::FootageKind::Model`] item (Advanced 3D).
    fn model(&self, _item: ItemId, _footage: &Footage) -> Option<Arc<effectcraft_model::Model>> {
        None
    }
    /// Set the decoded-frame cache budget in bytes (Settings ▸ Memory & CPU); sources without
    /// a cache ignore it.
    fn set_cache_budget(&self, _bytes: usize) {}
    /// Drop the decoded frames it keeps (Edit ▸ Purge ▸ All Memory / Image Cache Memory): the
    /// next reads decode again. Sources without a cache ignore it.
    fn purge(&self) {}
    /// The file at `path` changed on disk (Reload Footage): drop what it keeps of it (decoded
    /// frames, open decoders, parsed models), so the next reads see the new contents. Sources
    /// without a cache ignore it.
    fn forget(&self, _path: &str) {}
    /// Settings ▸ Disk ▸ Conformed Audio Folder: where decoded audio is kept between reads
    /// (`None` = off); sources that don't decode audio ignore it.
    fn set_conform_folder(&self, _folder: Option<std::path::PathBuf>) {}
    /// The decoded-frame cache budget in bytes, if the source has a cache.
    fn cache_budget(&self) -> Option<usize> {
        None
    }
    /// Vector footage (SVG, PDF, Illustrator, EPS: [`is_vector_footage`]) rasterised at `scale` ×
    /// its size as [`vector_fit`] draws it, for Continuously Rasterize. `None` when the footage
    /// is not vector or cannot be read.
    fn vector_frame(&self, _item: ItemId, _footage: &Footage, _scale: f64) -> Option<Arc<Image>> {
        None
    }
    /// Run `f`, which reads frames from this source, as work that can be called off: once
    /// `cancel` is set, long decodes it starts (a movie seeking back to a keyframe and decoding
    /// forward) stop early and their frames come back `None`. Sources that can't stop a decode
    /// just run `f`. See [`CancellableFootage`].
    fn cancellable(&self, _cancel: &Arc<AtomicBool>, f: &mut dyn FnMut()) {
        f()
    }
    /// Reads through this source have been called off: frames may have come back `None` that
    /// exist, so what was made from them must not be kept.
    fn called_off(&self) -> bool {
        false
    }
    /// Run `f`, which reads from this source on this thread, noting in `reads` every source file
    /// those reads use, with its identity as read (see [`crate::reads`]): the file a frame was
    /// decoded from, also when it comes from the source's own memory cache (without reading the
    /// file again). Sources that can't tell which files they read keep the default, which marks
    /// `reads` incomplete.
    fn recording(&self, reads: &ReadLog, f: &mut dyn FnMut()) {
        reads.mark_incomplete();
        f()
    }
    /// Why `footage` shows no picture although its file is there: a read of it failed because
    /// no decoder handles its codec, profile, chroma format or bit depth, or the stream is damaged
    /// (e.g. "H.264 (High 4:2:2) isn't supported by the built-in decoder: ..."). `None` while it
    /// decodes, and for sources that don't track failures. Stays set until the file is forgotten.
    fn problem(&self, _footage: &Footage) -> Option<String> {
        None
    }
    /// Missing files and [`Self::problem`] of what `item` shows: its proxy's while Use Proxy shows the proxy
    /// instead (the reason then says it is the proxy's), else the footage item's own. A
    /// composition has one only through its proxy.
    fn item_problem(&self, item: &effectcraft_project::Item) -> Option<String> {
        match item.proxy.as_deref().filter(|px| px.enabled && px.footage.has_video) {
            Some(px) => {
                missing_problem(&px.footage, "Use Set Proxy to locate the proxy file.").or_else(|| self.problem(&px.footage)).map(|why| format!("Proxy: {why}"))
            }
            None => match &item.kind {
                ItemKind::Footage(f) => missing_problem(f, "Use File ▸ Dependencies ▸ Relink Missing Footage or Replace Footage.").or_else(|| self.problem(f)),
                _ => None,
            },
        }
    }
}

/// A known offline source, without filesystem access while painting or querying.
fn missing_problem(f: &Footage, remedy: &str) -> Option<String> {
    (f.missing && !f.path.is_empty() && f.kind != effectcraft_project::FootageKind::Data).then(|| format!("Missing footage: {}. {remedy}", f.path))
}

/// Inventory warning, including an unused proxy or an original currently replaced by a proxy.
/// Viewer warnings instead use `item_problem`, which follows the active render source.
pub fn missing_item_problem(item: &effectcraft_project::Item) -> Option<String> {
    if let ItemKind::Footage(f) = &item.kind
        && let Some(why) = missing_problem(f, "Use File ▸ Dependencies ▸ Relink Missing Footage or Replace Footage.")
    {
        return Some(why);
    }
    item.proxy.as_ref().and_then(|p| missing_problem(&p.footage, "Use Set Proxy to locate the proxy file.")).map(|why| format!("Proxy: {why}"))
}

/// A render's view of a [`FootageSource`] whose frame reads can be called off with `cancel`
/// (a viewer frame nobody waits for any more, #329). Every read goes through
/// [`FootageSource::cancellable`] on its own, so reads made for this render on another thread
/// (a parallel loop) answer to this render's flag, and reads for other renders never do.
pub struct CancellableFootage<'a> {
    pub inner: &'a dyn FootageSource,
    pub cancel: Arc<AtomicBool>,
}

impl CancellableFootage<'_> {
    fn read<R>(&self, f: impl FnOnce() -> Option<R>) -> Option<R> {
        if self.cancel.load(Ordering::Relaxed) {
            return None;
        }
        let (mut f, mut out) = (Some(f), None);
        self.inner.cancellable(&self.cancel, &mut || out = f.take().and_then(|f| f()));
        out
    }
}

impl FootageSource for CancellableFootage<'_> {
    fn frame(&self, item: ItemId, footage: &Footage, t: Tick) -> Option<Arc<Image>> {
        self.read(|| self.inner.frame(item, footage, t))
    }
    fn frame_at_size(&self, item: ItemId, footage: &Footage, t: Tick, width: u32, height: u32) -> Option<Arc<Image>> {
        self.read(|| self.inner.frame_at_size(item, footage, t, width, height))
    }
    fn audio(&self, item: ItemId, footage: &Footage, t: Tick, frames: usize, rate: u32) -> Option<Vec<f32>> {
        self.inner.audio(item, footage, t, frames, rate)
    }
    fn aux(&self, item: ItemId, footage: &Footage, t: Tick) -> Option<Arc<effectcraft_raster::AuxChannels>> {
        self.inner.aux(item, footage, t)
    }
    fn set_aux_footage(&self, paths: &[String]) {
        self.inner.set_aux_footage(paths)
    }
    fn model(&self, item: ItemId, footage: &Footage) -> Option<Arc<effectcraft_model::Model>> {
        self.inner.model(item, footage)
    }
    fn set_cache_budget(&self, bytes: usize) {
        self.inner.set_cache_budget(bytes)
    }
    fn purge(&self) {
        self.inner.purge()
    }
    fn forget(&self, path: &str) {
        self.inner.forget(path)
    }
    fn set_conform_folder(&self, folder: Option<std::path::PathBuf>) {
        self.inner.set_conform_folder(folder)
    }
    fn cache_budget(&self) -> Option<usize> {
        self.inner.cache_budget()
    }
    fn vector_frame(&self, item: ItemId, footage: &Footage, scale: f64) -> Option<Arc<Image>> {
        self.inner.vector_frame(item, footage, scale)
    }
    fn cancellable(&self, cancel: &Arc<AtomicBool>, f: &mut dyn FnMut()) {
        self.inner.cancellable(cancel, f)
    }
    fn called_off(&self) -> bool {
        self.cancel.load(Ordering::Relaxed) || self.inner.called_off()
    }
    /// Runs `f` with the inner source, collecting the files it reads into `reads`.
    fn recording(&self, reads: &ReadLog, f: &mut dyn FnMut()) {
        self.inner.recording(reads, f)
    }
    fn problem(&self, footage: &Footage) -> Option<String> {
        self.inner.problem(footage)
    }
    fn frame_with(&self, item: ItemId, footage: &Footage, t: Tick, size: Option<(u32, u32)>, float: FloatCurve) -> Option<Arc<Image>> {
        self.read(|| self.inner.frame_with(item, footage, t, size, float))
    }
    fn frame8(&self, item: ItemId, footage: &Footage, t: Tick, float: FloatCurve) -> Option<Arc<crate::disk_cache::Frame8>> {
        self.read(|| self.inner.frame8(item, footage, t, float))
    }
}

/// Footage that can be rasterised at any scale: SVG, PDF, Illustrator and EPS files, and a
/// placed page of an After Effects project (its codec is not stored there).
pub fn is_vector_footage(f: &Footage) -> bool {
    matches!(f.codec.as_str(), "SVG" | "PDF" | "AI" | "EPS") || f.placed_page
}

/// How vector footage `f` is drawn at `scale` × its size: the image size, and the x and y scale
/// from document pixels (`doc` = the document's width and height, `own` = its pixel size, the
/// size rounded up) to image pixels.
///
/// Footage from an After Effects project ([`Footage::placed_page`]) fills its stored size, which
/// can round or measure the page differently: the layer's anchor and masks are measured against
/// that size, and the renderer resamples a reduced-resolution frame to it, so drawn at its own
/// size the art was scaled by the width ratio alone at full resolution (off against the masks)
/// and stretched to the stored aspect below 3/4 resolution (#3099). Other footage is drawn 1:1 at
/// the document's own size, as it always was.
pub fn vector_fit(f: &Footage, doc: [f64; 2], own: (u32, u32), scale: f64) -> ((u32, u32), [f64; 2]) {
    let scale = if scale.is_finite() && scale > 0.0 { scale } else { 1.0 };
    let side = |px: u32| (f64::from(px) * scale).ceil().clamp(1.0, f64::from(u32::MAX)) as u32;
    let size = (f.width, f.height);
    let fills = f.placed_page && size != own && size.0 > 0 && size.1 > 0 && doc.iter().all(|d| d.is_finite() && *d > 0.0);
    if !fills {
        return ((side(own.0), side(own.1)), [scale, scale]);
    }
    ((side(size.0), side(size.1)), [scale * f64::from(size.0) / doc[0], scale * f64::from(size.1) / doc[1]])
}

/// No footage available (renders footage layers as transparent).
pub struct NoFootage;

impl FootageSource for NoFootage {
    /// Always `None`: there is no footage.
    fn frame(&self, _: ItemId, _: &Footage, _: Tick) -> Option<Arc<Image>> {
        None
    }
    /// Reads no files.
    fn recording(&self, _: &ReadLog, f: &mut dyn FnMut()) {
        f()
    }
}
