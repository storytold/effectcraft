use super::{Arc, AtomicBool, Footage, Image, ItemId, ItemKind, Ordering, Tick};

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
    /// Vector footage (SVG) rasterised at `scale` × its pixel size, for Continuously Rasterize.
    /// `None` when the footage is not vector or cannot be read.
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
    /// Why `footage` shows no picture although its file is there: a read of it failed because
    /// no decoder handles its codec, profile, chroma format or bit depth, or the stream is damaged
    /// (e.g. "H.264 (High 4:2:2) isn't supported by the built-in decoder: ..."). `None` while it
    /// decodes, and for sources that don't track failures. Stays set until the file is forgotten.
    fn problem(&self, _footage: &Footage) -> Option<String> {
        None
    }
    /// [`Self::problem`] of what `item` shows: its proxy's while Use Proxy shows the proxy
    /// instead (the reason then says it is the proxy's), else the footage item's own. A
    /// composition has one only through its proxy.
    fn item_problem(&self, item: &effectcraft_project::Item) -> Option<String> {
        match item.proxy.as_deref().filter(|px| px.enabled && px.footage.has_video) {
            Some(px) => self.problem(&px.footage).map(|why| format!("Proxy: {why}")),
            None => match &item.kind {
                ItemKind::Footage(f) => self.problem(f),
                _ => None,
            },
        }
    }
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
    fn problem(&self, footage: &Footage) -> Option<String> {
        self.inner.problem(footage)
    }
    fn frame_with(&self, item: ItemId, footage: &Footage, t: Tick, size: Option<(u32, u32)>, float: FloatCurve) -> Option<Arc<Image>> {
        self.read(|| self.inner.frame_with(item, footage, t, size, float))
    }
}

/// Footage that can be rasterised at any scale (SVG).
pub fn is_vector_footage(f: &Footage) -> bool {
    matches!(f.codec.as_str(), "SVG" | "PDF" | "AI" | "EPS")
}

/// No footage available (renders footage layers as transparent).
pub struct NoFootage;

impl FootageSource for NoFootage {
    fn frame(&self, _: ItemId, _: &Footage, _: Tick) -> Option<Arc<Image>> {
        None
    }
}
