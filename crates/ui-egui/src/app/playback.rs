use super::*;

impl EffectcraftApp {
    // ---------------------------------------------------------------- playback

    pub fn play(&mut self, now: f64) {
        let sc = self.session.prefs.preview.current;
        self.play_with(now, sc);
    }

    /// Start the preview of a Preview panel shortcut (its range, play-from, rate, skip,
    /// resolution, loop, caching, full screen and include options).
    pub fn play_with(&mut self, now: f64, sc: effectcraft_engine::preview::PreviewShortcut) {
        let Some(c) = self.session.active_comp_arc() else { return };
        let preset = self.session.prefs.preview.get(sc).clone();
        let plan = effectcraft_engine::preview::plan(&preset, &c, self.session.time());
        let fr = c.frame_rate;
        if plan.video {
            self.session.set_time(fr.tick_of(plan.first));
        }
        let restore_max = preset.full_screen.then_some(self.ui.maximized);
        let viewer = panels::viewers::active_panel(self);
        if preset.full_screen && self.ui.dock.is_docked(viewer) {
            self.ui.maximized = Some(viewer);
        }
        self.playback = Playback {
            playing: true,
            start_wall: now,
            start_time: fr.tick_of(plan.first),
            start_frame: plan.first,
            shown: 0,
            waiting: false,
            fps: plan.fps,
            shortcut: sc,
            plan: Some(plan),
            caching: plan.cache_first && plan.video,
            res: preset.resolution.factor(preset.custom_resolution),
            overlays: preset.include_overlays,
            layer_controls: preset.include_layer_controls,
            restore_max,
            audio_frame: (!plan.video).then_some(plan.first),
            audio_held: false,
            starting: false,
            starting_since: now,
            first_cached: None,
            shown_at: Default::default(),
        };
        if !self.playback.caching {
            if plan.video {
                let audible = self.audio_plays();
                self.playback.audio_held = audible;
                self.playback.starting = audible;
            } else {
                self.start_audio(fr.tick_of(plan.first));
            }
        }
    }

    /// The preview includes audio at the comp's frame rate and the comp has something audible.
    fn audio_plays(&self) -> bool {
        let (Some(pl), Some(cid)) = (self.playback.plan, self.session.active_comp_id()) else { return false };
        let Some(c) = self.session.project.comp(cid) else { return false };
        // A preview at another frame rate plays slower or faster than real time: silent.
        pl.audio && (pl.fps - c.frame_rate.as_f64()).abs() <= 0.01 && effectcraft_engine::render::audio::comp_has_audio(&self.session.project, cid)
    }

    /// Start audio preview from comp time `t` when [`Self::audio_plays`] and an output device
    /// opens.
    pub(super) fn start_audio(&mut self, t: Tick) {
        self.audio = None;
        self.meter.clipped = [false; 2];
        let (Some(pl), Some(cid)) = (self.playback.plan, self.session.active_comp_id()) else { return };
        let Some(c) = self.session.project.comp_arc(cid).filter(|_| self.audio_plays()) else { return };
        let out = audio::AudioOutput::from_prefs(&self.session.prefs);
        let Some(dev) = self.hooks.audio_device.as_ref().and_then(|f| f(&out)) else { return };
        let mix = self.session.prefs.audio.preview_sample_rate;
        let fr = c.frame_rate;
        match audio::AudioPlayback::start(dev, self.render_source(), cid, t, fr.tick_of(pl.start), fr.tick_of(pl.end + 1), pl.looping, mix) {
            Ok(a) => self.audio = Some(a),
            Err(e) => log::warn!("audio preview: {e}"),
        }
    }

    /// Audio scrubbing: play the frame of `comp` at `t` (see [`audio::AudioScrub`]), opening the
    /// output on first use. Silent comps and missing devices do nothing.
    pub fn scrub_audio(&mut self, comp: ItemId, t: Tick, now: f64) {
        if self.playback.playing || !effectcraft_engine::render::audio::comp_has_audio(&self.session.project, comp) {
            return;
        }
        if self.scrub.is_none() {
            let out = audio::AudioOutput::from_prefs(&self.session.prefs);
            let Some(dev) = self.hooks.audio_device.as_ref().and_then(|f| f(&out)) else { return };
            match audio::AudioScrub::open(dev, now) {
                Ok(s) => self.scrub = Some(s),
                Err(e) => {
                    log::warn!("audio scrubbing: {e}");
                    return;
                }
            }
        }
        let src = self.render_source();
        let mix = self.session.prefs.audio.preview_sample_rate;
        if let Some(s) = &mut self.scrub {
            s.play(&src, comp, t, mix, now);
        }
    }

    pub fn stop(&mut self) {
        let was = std::mem::replace(&mut self.playback.playing, false);
        self.audio = None;
        self.playback.caching = false;
        self.playback.starting = false;
        if !was {
            return;
        }
        if let Some(m) = self.playback.restore_max.take() {
            self.ui.maximized = m;
        }
        let Some(pl) = self.playback.plan else { return };
        let move_time = self.session.prefs.preview.get(self.playback.shortcut).move_time_to_preview_time;
        if !move_time {
            self.session.set_time(pl.origin);
        } else if let Some(f) = self.playback.audio_frame
            && let Some(c) = self.session.active_comp()
        {
            let t = c.frame_rate.tick_of(f);
            self.session.set_time(t);
        }
    }

    pub fn toggle_play(&mut self, now: f64) {
        let sc = self.session.prefs.preview.current;
        self.toggle_play_with(now, sc);
    }

    /// A preview shortcut pressed: start its preview, or stop the running one. While Cache
    /// Before Playback is still rendering, "If caching, play cached frames" plays what is
    /// cached so far instead of stopping.
    pub fn toggle_play_with(&mut self, now: f64, sc: effectcraft_engine::preview::PreviewShortcut) {
        if !self.playback.playing {
            self.play_with(now, sc);
            return;
        }
        if self.playback.caching && self.session.prefs.preview.get(self.playback.shortcut).play_cached_frames && self.play_cached(now) {
            return;
        }
        self.stop();
    }

    /// Cut the caching preview down to the frames cached from its first frame on and play them.
    fn play_cached(&mut self, now: f64) -> bool {
        let (Some(mut pl), Some(cid)) = (self.playback.plan, self.session.active_comp_id()) else { return false };
        let scale = self.viewer_shown.as_ref().map(|(_, k)| k.scale as f64 / 1000.0).unwrap_or(1.0);
        let series = self.frame_series(cid, scale);
        let mut last = None;
        let mut f = pl.first;
        while f <= pl.end && self.frames.is_cached(&FrameKey { frame: f, ..series }) {
            last = Some(f);
            f += pl.step;
        }
        let Some(last) = last else { return false };
        pl.end = last;
        pl.start = pl.first;
        self.playback.plan = Some(pl);
        self.playback.caching = false;
        self.playback.start_wall = now;
        self.playback.start_frame = pl.first;
        let t = self.session.active_comp().map(|c| c.frame_rate.tick_of(pl.first)).unwrap_or_default();
        self.start_audio(t);
        true
    }

    /// A preview plays with Video off in the Preview panel (audio only): the current time stays
    /// put, so the viewer and Timeline say so instead of looking stuck (#1118). The frame under
    /// the audible sample, when one is known.
    pub fn video_preview_off(&self) -> Option<Option<i64>> {
        let p = self.playback.plan.filter(|_| self.playback.playing)?;
        (!p.video).then_some(self.playback.audio_frame)
    }

    /// The viewer shows grids, guides and safe margins (hidden while a preview without
    /// Include Overlays plays).
    pub fn overlays_visible(&self) -> bool {
        !self.playback.playing || self.playback.overlays
    }

    /// The viewer shows layer handles and paths (View ▸ Show Layer Controls, and Include Layer
    /// Controls while a preview plays).
    pub fn layer_controls_visible(&self) -> bool {
        self.ui.viewer.show_layer_controls && (!self.playback.playing || self.playback.layer_controls)
    }
}
