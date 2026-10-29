use super::*;

impl EffectcraftApp {
    /// The size of the viewer frame's texture (CPU frames; fitted to the GPU's texture limit).
    pub fn viewer_texture_size(&self) -> Option<[usize; 2]> {
        self.viewer_tex.as_ref().map(|(t, _, _)| t.size())
    }

    /// The viewer's current pixels as 8-bit premultiplied RGBA, reading a GPU frame back the
    /// first time something asks for it.
    ///
    /// The browser can't wait for the GPU: there the first call starts an asynchronous readback
    /// and returns `None`; a later call (the next frame) has the pixels.
    pub fn viewer_pixels(&mut self) -> Option<Arc<egui::ColorImage>> {
        let to_image = |bytes: Vec<u8>, w: u32, h: u32| {
            let px = bytes.as_chunks::<4>().0.iter().map(|c| egui::Color32::from_rgba_premultiplied(c[0], c[1], c[2], c[3])).collect();
            Arc::new(egui::ColorImage::new([w as usize, h as usize], px))
        };
        if self.viewer_image.is_none()
            && let (Some((_, key, f)), Some(g)) = (&self.viewer_native, &self.gpu)
            && self.viewer_shown.as_ref().map(|s| s.1) == Some(*key)
        {
            if cfg!(target_arch = "wasm32") {
                let ready = self.viewer_readback.1.lock().ok().and_then(|mut r| r.take_if(|r| r.0 == *key));
                if let Some((_, img)) = ready {
                    self.viewer_image = Some(img);
                } else if self.viewer_readback.0 != Some(*key) {
                    self.viewer_readback.0 = Some(*key);
                    let (slot, k, (w, h)) = (self.viewer_readback.1.clone(), *key, (f.width, f.height));
                    let ctx = self.frames.context();
                    g.read_display_async(f, move |bytes| {
                        if let (Some(b), Ok(mut s)) = (bytes, slot.lock()) {
                            *s = Some((k, to_image(b, w, h)));
                        }
                        if let Some(c) = ctx {
                            c.request_repaint();
                        }
                    });
                }
            } else if let Some(bytes) = g.read_display(f) {
                self.viewer_image = Some(to_image(bytes, f.width, f.height));
            }
        }
        self.viewer_image.clone()
    }

    /// The viewer's Resolution: the active composition's (saved with the project).
    pub fn viewer_res(&self) -> state::Resolution {
        self.session.active_comp().map(|c| c.resolution).unwrap_or_default()
    }

    /// The render scale used by the viewer right now.
    pub fn viewer_scale(&self, zoom: f32, ppp: f32) -> f64 {
        // The preview's Resolution (when not Auto) while it plays.
        if self.playback.playing
            && let Some(f) = self.playback.res
        {
            return state::Resolution::from_factor(f.into()).scale(zoom, ppp);
        }
        // Auto follows the zoom alike while playing and paused, so frames cached while scrubbing
        // play back, and frames cached by a preview stay on screen when it stops.
        let res = self.viewer_res();
        let full = res.scale(zoom, ppp);
        // Fast Previews ▸ Adaptive Resolution / Fast Draft: lower resolution while dragging.
        let (_, k) = self.session.state.viewer.fast_previews.render(self.ui.viewer.interacting || self.ui.viewer.property_interacting);
        if k < 1.0 && res.is_auto() {
            return full.min((full * 0.5).max(self.session.prefs.adaptive_limit()));
        }
        full
    }

    /// The cache key of viewer frame `frame` of `comp` at `scale` (see [`Self::frame_series`]).
    pub fn frame_key(&self, comp: ItemId, frame: i64, scale: f64) -> FrameKey {
        FrameKey { frame, ..self.frame_series(comp, scale) }
    }

    /// The key of frame 0 of `comp`'s viewer frames at `scale` (the content, view and render
    /// options are the same for every frame: loops compute this once).
    pub fn frame_series(&self, comp: ItemId, scale: f64) -> FrameKey {
        FrameKey {
            revision: self.session.revision,
            content: self.frames.content_of(&self.session.project, self.session.revision, comp),
            comp: comp.0,
            frame: 0,
            scale: (scale * 1000.0).round() as u32,
            view: self.view_hash(comp),
            opts: frames::opts_hash(&self.frame_opts(comp, scale)),
        }
    }

    /// The frame series the viewer of `comp` shows now (its scale, view and options, for the
    /// comp's current content): what the cache bars count.
    pub fn shown_series(&self, comp: ItemId) -> FrameKey {
        let scale = self.viewer_shown.as_ref().filter(|(_, k)| k.comp == comp.0).map_or(1000, |(_, k)| k.scale);
        self.frame_series(comp, scale as f64 / 1000.0)
    }

    /// Hash of the comp viewer's 3D view camera (0 for the active camera view).
    pub fn view_hash(&self, comp: ItemId) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        // The region of interest changes what is rendered too.
        let roi = self.viewer_region(comp);
        if let Some(r) = roi {
            r.map(f64::to_bits).hash(&mut h);
        }
        let Some(cam) = self.session.view_camera(comp) else { return if roi.is_some() { h.finish() | 1 } else { 0 } };
        for row in cam.view.0 {
            for v in row {
                v.to_bits().hash(&mut h);
            }
        }
        cam.zoom.to_bits().hash(&mut h);
        cam.ortho.hash(&mut h);
        h.finish() | 1
    }

    /// The comp-space region viewer frames of `comp` cover: the region of interest, else the
    /// Extended Viewer's area (frame plus pasteboard), else `None` (the whole frame).
    pub fn viewer_region(&self, comp: ItemId) -> Option<[f64; 4]> {
        if self.session.active_comp_id() != Some(comp) {
            return None;
        }
        self.session.state.region_of_interest.or(self.ui.viewer.extended)
    }

    /// Request a prefetch frame render (no-op if cached/in flight).
    pub fn request_frame(&self, comp: ItemId, frame: i64, scale: f64) {
        self.request_frame_with(comp, frame, scale, frames::Frames::request);
    }

    /// Request the frame on screen: rendered before any queued prefetch.
    pub fn request_frame_urgent(&self, comp: ItemId, frame: i64, scale: f64) {
        self.request_frame_with(comp, frame, scale, frames::Frames::request_urgent);
    }

    /// Request a frame nobody waits for (Cache Frames When Idle): rendered on one thread
    /// (`Frames::request_background`).
    pub fn request_frame_background(&self, comp: ItemId, frame: i64, scale: f64) {
        self.request_frame_with(comp, frame, scale, frames::Frames::request_background);
    }

    /// Render options of viewer frames of `comp` at `scale`.
    pub fn frame_opts(&self, comp: ItemId, scale: f64) -> RenderOpts {
        let (draft, _) = self.session.state.viewer.fast_previews.render(self.ui.viewer.interacting || self.ui.viewer.property_interacting);
        let roi = self.viewer_region(comp);
        RenderOpts {
            scale,
            motion_blur: true,
            guides: true,
            draft: self.ui.viewer.fast_preview || draft,
            view: self.session.view_camera(comp),
            roi,
            backend: effectcraft_engine::render::Backend::Auto,
            nested_switches: self.session.prefs.general.switches_affect_nested_comps,
            draft_shadows: self.session.prefs.three_d.realtime_shadows,
            proxy: Default::default(),
        }
    }

    fn request_frame_with(
        &self,
        comp: ItemId,
        frame: i64,
        scale: f64,
        request: fn(&frames::Frames, &frames::RenderSource, FrameKey, ItemId, Tick, RenderOpts),
    ) {
        let Some(c) = self.session.project.comp(comp) else { return };
        let key = self.frame_key(comp, frame, scale);
        let t = c.frame_rate.tick_of(frame);
        let opts = self.frame_opts(comp, scale);
        request(&self.frames, &self.render_source(), key, comp, t, opts);
    }
}
