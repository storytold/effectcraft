use super::{EffectcraftApp, FrameKey, FrameWindow, ItemId, PlaybackPass, audio, frames};

/// Seconds without a project change before paused prefetch resumes (see
/// [`EffectcraftApp::editing`]).
const EDIT_QUIET: f64 = 0.3;

impl EffectcraftApp {
    /// RAM preview: advance at the preview's frame rate when frames are cached; otherwise hold
    /// and keep rendering ahead (the green cache bar fills, then playback runs in real time).
    pub(super) fn advance_playback(&mut self, ctx: &egui::Context, scale: f64, now: f64, input_active: bool, cid: ItemId) {
        let Some(c) = self.session.project.comp_arc(cid) else { return };
        let fr = c.frame_rate;
        let plan = self.playback.plan.filter(|_| self.playback.playing);
        let (wa, wb) = match plan {
            Some(p) => (p.start, p.end),
            None => (fr.frame_at(c.work_area.0), fr.frame_at(c.work_area.1 - c.frame_duration())),
        };
        let looping = plan.is_none_or(|p| p.looping);
        let video = plan.is_none_or(|p| p.video);
        // Prefetch ahead.
        let cur = fr.frame_at(self.session.time());
        let ahead = self.prefetch_ahead(ctx, now);
        let queued = self.frames.inflight();
        let series = self.frame_series(cid, scale);
        // The preview's position orders prefetch and eviction (#595).
        let playhead = plan.filter(|p| p.video).map(|p| frames::Playhead { series, cur, start: p.start, end: p.end, step: p.step, looping: p.looping });
        self.frames.set_playhead(playhead);
        let pass = PlaybackPass {
            ctx,
            comp: &c,
            cid,
            scale,
            now,
            frame_rate: fr,
            series,
            plan,
            window: FrameWindow { first: wa, last: wb },
            cur,
            video,
            looping,
            ahead,
            queued,
        };
        if self.cache_preview(&pass) {
            return;
        }
        let step = plan.map_or(1, |p| p.step);
        let fit_ahead = self.frames_that_fit(&c, scale) - 1;
        self.prefetch_playback(&pass, fit_ahead);
        self.pump_meters(ctx, now);
        if !self.playback.playing {
            self.cache_when_idle(ctx, &c, &series, now, input_active, (wa, wb));
            return;
        }
        let fit = self.frames_that_fit(&c, scale);
        self.start_held_audio(&pass, fit);
        if self.advance_audio_clock(&pass, step) {
            return;
        }
        self.advance_frame_clock(&pass, step);
    }

    /// While paused (scrubbing, editing) the viewer's frame comes first: prefetch only a few
    /// frames, only once it is done, and not while edits keep coming (a layer dragged in the
    /// viewer): those frames would be stale at the next step and would hold the CPU and GPU
    /// the next viewer frame needs.
    fn prefetch_ahead(&mut self, ctx: &egui::Context, now: f64) -> i64 {
        if self.playback.playing {
            (self.frames_parallelism() * 2).max(4) as i64
        } else if self.frames.urgent_pending() {
            0
        } else if let Some(wait) = self.editing(now) {
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(wait));
            0
        } else {
            // Keep workers available for a newly edited viewer frame. Running obsolete
            // background renders cannot be preempted by the priority queue.
            self.frames_parallelism().saturating_sub(1).min(2) as i64
        }
    }

    /// Cache Before Playback: the range in play order, then start the clock. A range larger than
    /// the RAM preview budget is cut to the frames that fit (caching more would evict the first
    /// ones and never finish). Returns true when this pass handled the preview.
    fn cache_preview(&mut self, pass: &PlaybackPass<'_>) -> bool {
        let PlaybackPass { ctx, comp, cid, scale, now, frame_rate: fr, series, plan, ahead, mut queued, .. } = *pass;
        if !self.playback.caching {
            return false;
        }
        let Some(p) = plan else { return false };
        let mut all = true;
        let n = (p.end - p.start) / p.step + 1;
        let fit = self.frames_that_fit(comp, scale);
        if n > fit {
            let mut cut = p;
            (cut.start, cut.end) = (p.first, (p.first + (fit - 1) * p.step).min(p.end));
            self.playback.plan = Some(cut);
            self.toast = Some((format!("Preview cut to {fit} of {n} frames: no more fit in the RAM preview (Settings ▸ Memory & CPU)"), now));
            ctx.request_repaint();
            return true;
        }
        let order: Vec<i64> = (0..n).filter_map(|k| p.frame_after(p.first, k)).collect();
        for f in order {
            if self.frames.is_cached(&FrameKey { frame: f, ..series }) {
                continue;
            }
            all = false;
            if queued < ahead as usize {
                self.request_frame(cid, f, scale);
                queued += 1;
            }
        }
        self.playback.waiting = !all;
        if all {
            self.playback.caching = false;
            self.playback.start_wall = now;
            self.playback.start_frame = p.first;
            self.session.set_time(fr.tick_of(p.first));
            self.start_audio(fr.tick_of(p.first));
        }
        ctx.request_repaint();
        true
    }

    /// Never further ahead than the RAM preview holds: those frames would evict the ones the
    /// preview shows first, or be evicted before it gets there (#595). The window starts at the
    /// first uncached frame ahead: after a loop or a restart the frames just ahead are cached,
    /// and a window from the playhead left the workers idle until it came near the end of the
    /// cached run. It stays ahead × 3 frames wide: workers spread over the whole preview render
    /// frames with different layers at once (on the GPU, an atlas each: #1812).
    fn prefetch_playback(&self, pass: &PlaybackPass<'_>, fit_ahead: i64) {
        let PlaybackPass { cid, scale, plan, cur, ahead, series, mut queued, video, .. } = *pass;
        let FrameWindow { first: wa, last: wb } = pass.window;
        let frame = |k: i64| match plan {
            Some(p) => p.frame_after(cur, k),
            None => {
                let f = cur + k;
                Some(if f > wb { wa + (f - wb - 1).rem_euclid((wb - wa + 1).max(1)) } else { f })
            }
        };
        let cached = self.frames.cached_frames(&series);
        let uncached = |f: i64| cached.binary_search(&f).is_err();
        let first = match plan.filter(|_| self.playback.playing) {
            Some(p) => (0..fit_ahead.min((p.end - p.start) / p.step.max(1) + 1))
                .map_while(|k| frame(k).map(|f| (k, f)))
                .find(|&(_, f)| uncached(f))
                .map_or(0, |(k, _)| k),
            None => 0,
        };
        for k in first..(first + ahead * 3).min(fit_ahead) {
            if queued >= ahead as usize || !video {
                break;
            }
            let Some(f) = frame(k) else { break };
            if uncached(f) {
                self.request_frame(cid, f, scale);
                queued += 1;
            }
        }
    }

    /// Advance the audio or scrub output and feed its latest peaks to the level meter; an idle scrub is closed after
    /// one second. With no source but an active meter, drive the meter to silence and request a repaint.
    fn pump_meters(&mut self, ctx: &egui::Context, now: f64) {
        if let Some(a) = &mut self.audio {
            a.pump();
            self.meter.update(a.feed.take_peaks(), now);
        } else if let Some(s) = &mut self.scrub {
            // Scrubbing: the meters follow the snippets; an idle second closes the output.
            s.pump();
            self.meter.update(s.feed.take_peaks(), now);
            if now - s.last_used > 1.0 {
                self.scrub = None;
            }
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        } else if self.meter.active() {
            self.meter.update([0.0; 2], now);
            ctx.request_repaint();
        }
    }

    /// Held sound starts once the frames ahead are cached (see `Playback::audio_held`): the
    /// prefetch window when the silent preview keeps real time, else the rest of the preview.
    fn start_held_audio(&mut self, pass: &PlaybackPass<'_>, fit: i64) {
        let PlaybackPass { now, plan, series, cur, ahead, frame_rate: fr, .. } = *pass;
        if !self.playback.audio_held {
            return;
        }
        let Some(p) = plan else { return };
        if (self.playback.keeps_up(now) && self.cached_ahead(&p, &series, cur, ahead.min(fit))) || self.cached_ahead(&p, &series, cur, fit) {
            self.playback.audio_held = false;
            self.playback.start_wall = now;
            self.playback.start_frame = cur;
            self.start_audio(fr.tick_of(cur));
        }
    }

    /// Audio clock drives playback: show the frame under the audible sample. Returns true when
    /// the audio path handled this pass (including stopping at the end).
    fn advance_audio_clock(&mut self, pass: &PlaybackPass<'_>, step: i64) -> bool {
        let PlaybackPass { ctx, now, series, video, frame_rate: fr, cur, .. } = *pass;
        let FrameWindow { first: wa, last: wb } = pass.window;
        let Some(a) = &self.audio else { return false };
        if a.finished() {
            if video {
                self.session.set_time(fr.tick_of(wb));
            } else {
                self.playback.audio_frame = Some(wb);
            }
            self.stop();
            return true;
        }
        let snap = |f: i64| wa + (f - wa).div_euclid(step) * step;
        let target = snap(audio::frame_of_sample(a.clock(), a.rate, fr).clamp(wa, wb));
        if !video {
            self.playback.audio_frame = Some(target);
        } else if self.frames.is_cached(&FrameKey { frame: target, ..series }) {
            self.playback.waiting = false;
            if target != cur {
                self.session.set_time(fr.tick_of(target));
                self.playback.frame_shown(now);
            }
        } else {
            // Rendering can't keep up: rather than skip frames, the sound stops and every
            // frame shows as it renders until the frames ahead are cached again.
            self.audio = None;
            self.playback.audio_held = true;
            self.playback.waiting = true;
            // The rate measured before the stall no longer says it keeps up.
            self.playback.shown_at.clear();
            self.playback.start_wall = now;
            self.playback.start_frame = cur;
        }
        ctx.request_repaint();
        true
    }

    /// The wall clock advances the preview when no audio device drives it, and shows the target
    /// frame (or holds when it is not cached yet).
    fn advance_frame_clock(&mut self, pass: &PlaybackPass<'_>, step: i64) {
        let PlaybackPass { ctx, now, plan, video, looping, frame_rate: fr, cur, series, .. } = *pass;
        let FrameWindow { first: wa, last: wb } = pass.window;
        let elapsed = now - self.playback.start_wall;
        // The plan carries the rate the preview was started with (and `play_with` copies it to
        // `Playback::fps`). Deriving the frame from the plan keeps a logic-only tick correct even
        // when a host/test supplies only the plan; a stale duplicate cannot hold playback open.
        let fps = plan.map_or(self.playback.fps, |p| p.fps);
        let n = ((elapsed * fps) / step as f64).floor() as i64;
        let target = match plan {
            Some(p) => p.frame_after(self.playback.start_frame, n),
            None => {
                let t = self.playback.start_frame + n;
                if t <= wb {
                    Some(t)
                } else if looping {
                    Some(wa + (t - wa).rem_euclid((wb - wa + 1).max(1)))
                } else {
                    None
                }
            }
        };
        let Some(target) = target else {
            if video {
                self.session.set_time(fr.tick_of(wb));
            } else {
                self.playback.audio_frame = Some(wb);
            }
            self.stop();
            return;
        };
        if !video {
            // Audio-only preview without an audio device: the clock runs, the time stays.
            self.playback.audio_frame = Some(target);
            ctx.request_repaint();
            return;
        }
        if self.frames.is_cached(&FrameKey { frame: target, ..series }) {
            self.playback.waiting = false;
            if target != cur {
                self.session.set_time(fr.tick_of(target));
                self.playback.frame_shown(now);
            }
        } else {
            // Not cached yet: hold the clock at the current frame (cache first, then play).
            self.playback.waiting = true;
            let next = plan.and_then(|p| p.frame_after(cur, 1)).unwrap_or(if cur + 1 > wb { wa } else { cur + 1 });
            if self.frames.is_cached(&FrameKey { frame: next, ..series }) {
                self.session.set_time(fr.tick_of(next));
            }
            self.playback.start_wall = now;
            self.playback.start_frame = fr.frame_at(self.session.time());
        }
        ctx.request_repaint();
    }

    /// While the project keeps changing (an edit within [`EDIT_QUIET`] seconds), how long until
    /// it counts as settled.
    fn editing(&mut self, now: f64) -> Option<f64> {
        if self.last_edit.1 != self.session.revision {
            self.last_edit = (now, self.session.revision);
        }
        let left = EDIT_QUIET - (now - self.last_edit.0);
        (left > 0.0).then_some(left)
    }

    /// Composition ▸ Preview ▸ Cache Frames When Idle: after a second without input or edits,
    /// render the work area's frames (from the current time on, then from its start) in the
    /// background, a few at a time, until it is cached or the RAM preview budget is full.
    fn cache_when_idle(
        &mut self,
        ctx: &egui::Context,
        comp: &effectcraft_engine::project::Comp,
        series: &FrameKey,
        now: f64,
        input_active: bool,
        (wa, wb): (i64, i64),
    ) {
        let active = input_active;
        if active || self.last_activity.1 != self.session.revision {
            self.last_activity = (now, self.session.revision);
        }
        if !self.ui.cache_when_idle || self.frames.urgent_pending() {
            return;
        }
        let quiet = now - self.last_activity.0;
        if quiet < 1.0 {
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(1.0 - quiet));
            return;
        }
        // An empty work area has nothing to cache.
        let n = wb - wa + 1;
        if n < 1 {
            return;
        }
        let cur = comp.frame_rate.frame_at(self.session.time()).clamp(wa, wb);
        let fit = self.frames_that_fit(comp, series.scale as f64 / 1000.0).min(n);
        let slots = (self.frames_parallelism() / 2).max(1);
        let mut queued = self.frames.inflight();
        let scale = series.scale as f64 / 1000.0;
        let mut pending = false;
        for k in 0..fit {
            let f = wa + (cur - wa + k).rem_euclid(n);
            if self.frames.is_cached(&FrameKey { frame: f, ..*series }) {
                continue;
            }
            pending = true;
            if queued >= slots {
                break;
            }
            self.request_frame(ItemId(series.comp), f, scale);
            queued += 1;
        }
        if pending {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
    }

    /// How many viewer frames of `comp` at `scale` the RAM preview budget holds (90 % of it,
    /// at least one).
    /// The frames of `plan` from `cur` to its end, or as many as fit in the RAM preview, are
    /// cached: sound can play from `cur` in real time.
    fn cached_ahead(&self, plan: &effectcraft_engine::preview::PreviewPlan, series: &FrameKey, cur: i64, fit: i64) -> bool {
        let cached = self.frames.cached_frames(series);
        (0..fit).map_while(|k| plan.frame_after(cur, k)).take_while(|f| *f >= cur).all(|f| cached.binary_search(&f).is_ok())
    }

    fn frames_that_fit(&self, comp: &effectcraft_engine::project::Comp, scale: f64) -> i64 {
        let side = |n: u32| (n as f64 * scale).ceil().max(1.0);
        let frame = side(comp.width) * side(comp.height) * 4.0;
        ((self.frames.budget() as f64 * 0.9 / frame) as i64).max(1)
    }

    fn frames_parallelism(&self) -> usize {
        if cfg!(target_arch = "wasm32") {
            // frame workers render one frame each; without them frames render one at a time
            // on the UI thread (`Frames::pump`)
            return self.frames.remote_slots().max(1);
        }
        self.frames.parallelism()
    }
}
