use super::{EngineError, Image, ItemId, RenderOpts, Renderer, Result, Session, ThumbnailJob, Tick, Value, commands};

impl Session {
    /// Render a comp frame with the session's footage + expression hosts.
    pub fn render(&self, comp: ItemId, t: Tick, opts: RenderOpts) -> Image {
        self.renderer(opts).comp_frame(comp, t)
    }

    /// The renderer [`Session::render`] uses: the session's footage, expressions, layer cache and
    /// accelerator, and the render prefs (nested switches, draft shadows) over `opts`.
    pub fn renderer(&self, opts: RenderOpts) -> Renderer<'_> {
        let mut r = Renderer::new(&self.project, self.footage.as_ref(), self.render_opts(opts));
        r.expr = self.expr.as_deref();
        r.cache = Some(&self.layer_cache);
        r.accel = self.accel.as_deref();
        r
    }

    /// `opts` with the render prefs (nested switches, draft shadows), as [`Session::renderer`].
    pub fn render_opts(&self, opts: RenderOpts) -> RenderOpts {
        RenderOpts { nested_switches: self.prefs.general.switches_affect_nested_comps, draft_shadows: self.prefs.three_d.realtime_shadows, ..opts }
    }

    /// Whether `effect.cryptomatteAt` with params `p` answers from the kept probe (a few array
    /// reads; `probeOnly` queries answer `pending` until then). Changes nothing: ask it every
    /// frame, and start [`Session::crypto_probe_job`] only when a build will actually run.
    pub fn crypto_probe_ready(&self, p: &Value) -> Result<bool> {
        commands::crypto_probe_ready(self, p)
    }

    /// For the Cryptomatte eyedropper: `None` when `effect.cryptomatteAt` with params `p` answers
    /// from the kept probe, else a job that builds that probe (it may render a precomp's ID pass)
    /// off the UI thread, for the next query to use. Asking for a job supersedes a build in
    /// flight for another probe (it then stores nothing): ask only to run it.
    pub fn crypto_probe_job(&self, p: &Value) -> Result<Option<commands::CryptoProbeJob>> {
        commands::crypto_probe_job(self, p)
    }

    /// The coverage of Cryptomatte ID `hash` (as `effect.cryptomatteAt` reports it, parsed) over
    /// the layer of params `p`, at most `max_side` samples on the long side, from the kept probe:
    /// `None` until [`Session::crypto_probe_job`] has built it.
    pub fn crypto_coverage(&self, p: &Value, hash: u32, max_side: usize) -> Option<commands::CryptoCoverage> {
        commands::crypto_coverage(self, p, hash, max_side)
    }

    /// Let go of the kept Cryptomatte probe (and its aux channels), and cancel a build in
    /// flight: the eyedropper was disarmed. Edits, footage changes and Open / New do it too.
    pub fn drop_crypto_probe(&self) {
        if let Ok(mut slot) = self.crypto_probe.lock() {
            slot.clear();
        }
    }

    /// Whether a Cryptomatte probe is kept, and how many were built so far (for tests).
    #[doc(hidden)]
    pub fn crypto_probe_stats(&self) -> (bool, u64) {
        self.crypto_probe.lock().map_or((false, 0), |s| (s.kept(), s.builds().1))
    }

    /// How many probe jobs ran so far, built or cancelled before they began (for tests: each
    /// is a thread in the eyedropper).
    #[doc(hidden)]
    pub fn crypto_probe_jobs(&self) -> u64 {
        self.crypto_probe.lock().map_or(0, |s| s.builds().0)
    }

    /// Render a comp frame (comp time `t`) as opaque 8-bit RGBA over the comp background, scaled
    /// so the longest side is at most `max_side` pixels (0 = full size). Returns (width, height, rgba).
    pub fn render_rgba8(&self, comp: ItemId, t: Tick, max_side: u32) -> Result<(u32, u32, Vec<u8>)> {
        let c = self.project.comp(comp).ok_or(EngineError::NoComp)?;
        let long = c.width.max(c.height).max(1) as f64;
        let scale = if max_side == 0 { 1.0 } else { (max_side as f64 / long).min(1.0) };
        let img = self.render(comp, t, RenderOpts { scale, backend: effectcraft_render::Backend::Auto, ..Default::default() });
        Ok((img.width, img.height, img.to_rgba8_over(c.background)))
    }

    /// A Project panel thumbnail of an item (a comp at `t`, footage at its first frame), longest
    /// side at most `max_side`, as a job that owns everything it reads: frontends run it off the
    /// UI thread (a big comp takes far longer than a frame). `None` for items without pixels.
    pub fn thumbnail_job(&self, item: ItemId, t: Tick, max_side: u32) -> Option<ThumbnailJob> {
        let it = self.project.item(item)?;
        let footage = self.footage.clone();
        match &it.kind {
            effectcraft_project::ItemKind::Comp(c) => {
                let (project, expr, cache) = (self.project.clone(), self.expr.clone(), self.layer_cache.clone());
                let long = c.width.max(c.height).max(1) as f64;
                let scale = if max_side == 0 { 1.0 } else { (max_side as f64 / long).min(1.0) };
                let opts = RenderOpts {
                    scale,
                    nested_switches: self.prefs.general.switches_affect_nested_comps,
                    draft_shadows: self.prefs.three_d.realtime_shadows,
                    ..Default::default()
                };
                let bg = c.background;
                Some(Box::new(move || {
                    let mut r = Renderer::new(&project, footage.as_ref(), opts);
                    r.expr = expr.as_deref();
                    r.cache = Some(&cache);
                    let img = r.comp_frame(item, t);
                    Some((img.width, img.height, img.to_rgba8_over(bg)))
                }))
            }
            effectcraft_project::ItemKind::Footage(f) if f.has_video => {
                let f = f.clone();
                Some(Box::new(move || {
                    // Vector footage is drawn at the thumbnail's size: its declared size can be a
                    // 16384² canvas, gigabytes of pixels to point-sample down to a few dozen.
                    let scale = (max_side.max(1) as f64 / f.width.max(f.height).max(1) as f64).min(1.0);
                    let src = footage.vector_frame(item, &f, scale).or_else(|| footage.frame(item, &f, Tick(0)))?;
                    let img = effectcraft_render::cineon_frame(src, &f);
                    // Point-sample down to at most `max_side` on the long side (a drawn size one
                    // pixel over it, from rounding, is shrunk by that pixel, not halved).
                    let (long, max) = (u64::from(img.width.max(img.height)), u64::from(max_side.max(1)));
                    let fit = |side: u32| if long <= max { side } else { ((u64::from(side) * max + long / 2) / long).max(1) as u32 };
                    let (w, h) = (fit(img.width), fit(img.height));
                    let full = img.to_rgba8();
                    let mut px = Vec::with_capacity((w * h * 4) as usize);
                    for y in 0..h {
                        for x in 0..w {
                            let (sx, sy) = (u64::from(x) * u64::from(img.width) / u64::from(w), u64::from(y) * u64::from(img.height) / u64::from(h));
                            let i = ((sy * u64::from(img.width) + sx) * 4) as usize;
                            px.extend_from_slice(&full[i..i + 4]);
                        }
                    }
                    Some((w, h, px))
                }))
            }
            _ => None,
        }
    }

    /// [`Session::render_rgba8`], or with `transparent` the frame's own straight alpha (what a
    /// render with RGB + Alpha channels writes) instead of compositing over the background.
    pub fn render_rgba8_alpha(&self, comp: ItemId, t: Tick, max_side: u32, transparent: bool) -> Result<(u32, u32, Vec<u8>)> {
        if !transparent {
            return self.render_rgba8(comp, t, max_side);
        }
        let c = self.project.comp(comp).ok_or(EngineError::NoComp)?;
        let long = c.width.max(c.height).max(1) as f64;
        let scale = if max_side == 0 { 1.0 } else { (max_side as f64 / long).min(1.0) };
        let img = self.render(comp, t, RenderOpts { scale, backend: effectcraft_render::Backend::Auto, ..Default::default() });
        Ok((img.width, img.height, img.to_rgba8()))
    }

    /// Make the media layer and the layer cache let go of what they decoded from `footage`'s
    /// files (it keys frames by path, so an overwritten file would keep showing its old pixels).
    pub(crate) fn forget_decoded(&self, footage: &effectcraft_project::Footage) {
        for p in std::iter::once(&footage.path).chain(&footage.sequence) {
            self.footage.forget(p);
        }
        self.layer_cache.clear();
        self.drop_crypto_probe();
    }

    /// Tell the footage source which files effects read the channels of ([`aux_footage`]), so
    /// it can decode their pictures and channels at once from the first frame (#474). Called with
    /// every project change ([`Session::bump`]); the set is only sent when it changed, or after
    /// [`Session::replace_project`] cleared what was sent.
    pub(crate) fn refresh_aux_footage(&mut self) {
        let paths = aux_footage(&self.project);
        if self.aux_footage.as_ref() != Some(&paths) {
            self.footage.set_aux_footage(&paths);
            self.aux_footage = Some(paths);
        }
    }
}

/// The files (each footage item's [`effectcraft_project::Footage::path`], sorted, once each) of
/// the footage layers that have an enabled effect reading their channels
/// ([`effectcraft_effects::reads_aux`]), with the layer's Effects switch on. Several items can
/// use one file: it stays in while any of them is read.
pub(crate) fn aux_footage(project: &effectcraft_project::Project) -> Vec<String> {
    use effectcraft_project::{GroupKind, ItemKind, LayerSource};
    let mut paths: Vec<String> = project
        .comps()
        .flat_map(|(_, c)| c.layers.iter())
        .filter(|l| l.switches.effects)
        .filter(|l| {
            l.effects()
                .is_some_and(|fx| fx.groups().any(|g| g.enabled && matches!(&g.kind, GroupKind::Effect { effect } if effectcraft_effects::reads_aux(effect))))
        })
        .filter_map(|l| match &l.source {
            LayerSource::Footage { item } => match &project.item(*item)?.kind {
                ItemKind::Footage(f) if !f.path.is_empty() => Some(f.path.clone()),
                _ => None,
            },
            _ => None,
        })
        .collect();
    paths.sort_unstable();
    paths.dedup();
    paths
}
