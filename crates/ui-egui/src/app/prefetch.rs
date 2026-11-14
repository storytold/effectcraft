use super::{EffectcraftApp, FrameKey, FrameWindow, ItemId, PlaybackPass, frames};

/// Seconds without a project change before paused prefetch resumes (see
/// [`EffectcraftApp::editing`]).
const EDIT_QUIET: f64 = 0.3;

/// Longest a preview with sound holds its first frame, once that frame is cached, waiting for the
/// frames ahead (seconds, #947). A frame ahead that never gets cached mustn't hold it for ever:
/// the sound starts from the first frame anyway, and the preview goes on as a stalled one does.
const STARTUP_WAIT: f64 = 2.0;

/// Longest a preview with sound holds its first frame in all (seconds, #947). The first frame
/// itself can take a while (a heavy comp, a busy machine), and [`STARTUP_WAIT`] counts only from
/// when it is cached; but one that never gets cached (larger than the RAM preview, which the size
/// estimate can't always tell) mustn't hold it for ever. The preview then goes on as a stalled
/// one does: its sound starts once the frames are cached.
const STARTUP_LIMIT: f64 = 10.0;

/// How many frames from the first a preview with sound waits for before it starts (#947): the
/// prefetch's `ahead`, but no more than its `window` requests from the current frame (offsets
/// `0..window`), and no more than fit in the RAM preview `budget` (90 %, as for the prefetch) at
/// `frame_bytes`, the size frames really render at; at least the current frame, which the viewer
/// always requests. Waiting for a frame that is never requested, or that can't stay cached with
/// the others, would hold the first frame for ever.
fn startup_frames(ahead: i64, window: i64, budget: usize, frame_bytes: usize) -> i64 {
    let fit = (budget as f64 * 0.9 / frame_bytes.max(1) as f64) as i64;
    ahead.min(window).min(fit).max(1)
}

/// Whether frame `f` comes after the current frame `cur` and before the sound's frame `target` in
/// play order. A looping preview whose sound is at a lower frame than the picture has wrapped
/// to the loop's start, so the frames up to the end and then from the start count; otherwise
/// (and always without a loop) only the frames strictly between them do.
fn between_picture_and_sound(cur: i64, f: i64, target: i64, looping: bool) -> bool {
    if target >= cur || !looping { cur < f && f < target } else { f > cur || f < target }
}

/// How long the pace takes to follow the rate frames arrive at (seconds, the time constant of
/// its smoothing): short enough to follow a change, long enough to play through a batch of
/// frames finishing together.
const PACE_TAU: f64 = 0.5;

/// The rate a preview plays at when frames arrive slower than its frame rate: proportional to
/// the cached frames ahead (`lead` of `target`), so it settles where the clock takes frames as
/// fast as they arrive and the lead stays put; smoothed over [`PACE_TAU`] (`dt` seconds since
/// the last pass). Never above the frame rate, never under an eighth of it (slower reads as
/// stopped: the clock holds instead). No frame ahead: the previous pace, the clock holds anyway.
fn pace_for(prev: f64, lead: i64, target: i64, fps: f64, dt: f64) -> f64 {
    if lead <= 0 || target <= 0 || fps <= 0.0 {
        return prev;
    }
    let raw = (fps * lead as f64 / target as f64).clamp(fps / 8.0, fps);
    let k = 1.0 - (-dt.max(0.0) / PACE_TAU).exp();
    prev + (raw - prev) * k
}

/// How many frames after the current one the pace measures its lead over: twice the prefetch
/// depth `ahead`, but only frames the RAM preview can keep cached together with the current one.
/// `prefetch_playback` requests offsets `0..fit_ahead` from the current frame (the current frame
/// and `fit_ahead - 1` after it), so a lead counted over more frames than that could never fill:
/// a preview that renders every frame at once would still be slowed down to a fraction of its
/// frame rate. 0 when not even one frame after the current one is prefetched: the pace can't
/// measure anything, and the clock runs in real time (holding on frames that aren't cached).
fn pace_horizon(ahead: i64, fit_ahead: i64) -> i64 {
    (ahead * 2).max(1).min(fit_ahead - 1).max(0)
}

/// Cached frames (`cached`, sorted) among the next `max` frames `after(1..=max)` plays, and how
/// many frames it plays (`after` is `None` past the end). Counted, not the consecutive run: one
/// late frame (a GPU warm-up frame, a heavy one) with frames cached beyond it holds the clock
/// for a moment when it gets there, rather than slowing the whole preview to a crawl.
fn lead_of(after: impl Fn(i64) -> Option<i64>, cached: &[i64], max: i64) -> (i64, i64) {
    let ahead: Vec<i64> = (1..=max).map_while(after).collect();
    (ahead.iter().filter(|f| cached.binary_search(f).is_ok()).count() as i64, ahead.len() as i64)
}

impl PlaybackPass<'_> {
    /// The frame `k` played frames after `from` (wrapping as the preview loops), `None` once a
    /// preview that doesn't loop has reached its end.
    fn frame_after(&self, from: i64, k: i64) -> Option<i64> {
        match self.plan {
            Some(p) => p.frame_after(from, k),
            None => {
                let FrameWindow { first: wa, last: wb } = self.window;
                let f = from + k;
                if f <= wb {
                    Some(f)
                } else if self.looping {
                    Some(wa + (f - wa).rem_euclid((wb - wa + 1).max(1)))
                } else {
                    None
                }
            }
        }
    }
}

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
        // While the sound drives a video preview, frames are rendered and kept ahead of what it
        // plays rather than of the picture, which drops frames behind it when rendering can't
        // keep up (#2473).
        let lead = if video { self.sound_frame(fr, wa, wb, plan.map_or(1, |p| p.step)).unwrap_or(cur) } else { cur };
        let ahead = self.prefetch_ahead(ctx, now);
        let queued = self.frames.inflight();
        let series = self.frame_series(cid, scale);
        // The preview's position orders prefetch and eviction (#595).
        let playhead = plan.filter(|p| p.video).map(|p| frames::Playhead { series, cur: lead, start: p.start, end: p.end, step: p.step, looping: p.looping });
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
        self.prefetch_playback(&PlaybackPass { cur: lead, ..pass }, fit_ahead);
        self.wake_silent_preview();
        self.pump_meters(ctx, now);
        if !self.playback.playing {
            self.cache_when_idle(ctx, &c, &series, now, input_active, (wa, wb));
            return;
        }
        let fit = self.frames_that_fit(&c, scale);
        if self.hold_first_frame(&pass, fit) {
            return;
        }
        self.start_held_audio(&pass, fit);
        if self.advance_audio_clock(&pass, step) {
            return;
        }
        self.advance_frame_clock(&pass, step, fit_ahead);
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
            // An edit while playing (mute, unmute, solo, levels): the sound follows it (#2104).
            if a.project().is_some_and(|p| !std::sync::Arc::ptr_eq(&p, &self.session.project)) {
                a.follow_project(self.session.project.clone());
            }
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

    /// Starting a preview with sound: hold the first frame until the frames ahead are cached, then
    /// start the sound from that same frame (see `Playback::starting`). Without an output device
    /// the picture plays on its own clock from it. How many frames ahead: only ones
    /// `prefetch_playback` requests and that fit in the RAM preview at the size they really render
    /// (an Extended Viewer frame is larger than the comp), so the wait always ends. Returns true
    /// while it holds the first frame.
    fn hold_first_frame(&mut self, pass: &PlaybackPass<'_>, fit: i64) -> bool {
        let PlaybackPass { ctx, comp, scale, now, plan, series, cur, ahead, frame_rate: fr, .. } = *pass;
        if !self.playback.starting {
            return false;
        }
        let Some(p) = plan else { return false };
        self.playback.start_wall = now;
        self.playback.start_frame = cur;
        // `prefetch_playback` requests every uncached frame of offsets 0..window from here.
        let window = (ahead * 3).min(fit - 1);
        let need = self.frames.get(&FrameKey { frame: cur, ..series }).map(|img| startup_frames(ahead, window, self.frames.budget(), img.bytes()));
        if need.is_some() && self.playback.first_cached.is_none() {
            self.playback.first_cached = Some(now);
        }
        // Don't wait for frames the RAM preview can't hold (not even one frame fits), for the
        // frames ahead longer than STARTUP_WAIT once the first is in, nor longer than
        // STARTUP_LIMIT in all (a frame that never gets cached).
        let give_up = (self.frames.budget() as f64) < self.frame_bytes(comp, scale)
            || self.playback.first_cached.is_some_and(|t| now - t >= STARTUP_WAIT)
            || now - self.playback.starting_since >= STARTUP_LIMIT;
        if !give_up && !need.is_some_and(|n| self.cached_ahead(&p, &series, cur, n)) {
            self.playback.waiting = true;
            ctx.request_repaint();
            return true;
        }
        self.playback.starting = false;
        self.playback.waiting = false;
        // Not on a frame that isn't cached: the picture couldn't show where the sound starts. The
        // preview goes on silently, and the sound starts once frames are cached.
        if need.is_some() {
            self.playback.audio_held = false;
            self.start_audio(fr.tick_of(cur));
        }
        false
    }

    /// A silent preview whose project was edited so that it has sound now (a layer unmuted while
    /// playing, #2104): video previews wait for cached frames; audio-only previews start from
    /// their moving audio frame, not the stationary timeline. Checked once per project edit.
    fn wake_silent_preview(&mut self) {
        let pb = &self.playback;
        if self.audio.is_some() || !pb.playing || pb.caching || pb.starting || pb.audio_held {
            return;
        }
        if pb.silent_project.as_ref().is_some_and(|p| std::sync::Arc::ptr_eq(p, &self.session.project)) {
            return;
        }
        self.playback.silent_project = Some(self.session.project.clone());
        if !self.audio_plays() {
            return;
        }
        if self.playback.plan.is_some_and(|p| p.video) {
            self.playback.audio_held = true;
        } else if let Some(c) = self.session.active_comp() {
            let frame = self.playback.audio_frame.unwrap_or(self.playback.start_frame);
            self.start_audio(c.frame_rate.tick_of(frame));
        }
    }

    /// Held sound starts once the frames ahead are cached (see `Playback::audio_held`): the
    /// prefetch's `ahead` when the silent preview keeps real time, else the rest of the preview.
    fn start_held_audio(&mut self, pass: &PlaybackPass<'_>, fit: i64) {
        let PlaybackPass { now, plan, series, cur, ahead, frame_rate: fr, .. } = *pass;
        if !self.playback.audio_held {
            return;
        }
        let Some(p) = plan else { return };
        // No more than the prefetch requests: offsets below `fit - 1` (`advance_playback`).
        let need = ahead.min(fit - 1).max(1);
        if (self.playback.keeps_up(now) && self.cached_ahead(&p, &series, cur, need)) || self.cached_ahead(&p, &series, cur, fit) {
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
        let target = snap(fr.frame_at(a.clock_time()).clamp(wa, wb));
        if !video {
            self.playback.audio_frame = Some(target);
        } else if self.frames.is_cached(&FrameKey { frame: target, ..series }) {
            self.playback.waiting = false;
            if target != cur {
                self.session.set_time(fr.tick_of(target));
                self.playback.frame_shown(now);
            }
        } else {
            // Rendering can't keep up: the sound plays on and the picture drops frames, showing
            // the latest one rendered up to the sound, while `advance_playback` renders ahead of
            // the sound (#2473). Stopping the sound instead and starting it again once frames were
            // cached chopped it into bursts that played as distortion (#3260).
            self.playback.waiting = true;
            if let Some(f) = self.latest_cached_before(pass, target).filter(|&f| f != cur) {
                self.session.set_time(fr.tick_of(f));
                self.playback.frame_shown(now);
            }
            // Dropping frames: a lower resolution may keep up (the rate shown says so).
            self.step_down(now, pass.scale);
        }
        ctx.request_repaint();
        true
    }

    /// The frame under the audible sample while the sound drives the preview, kept in the
    /// preview's range and on its frame step; `None` without sound.
    fn sound_frame(&self, fr: effectcraft_engine::time::FrameRate, wa: i64, wb: i64, step: i64) -> Option<i64> {
        let a = self.audio.as_ref()?;
        let step = step.max(1);
        Some(wa + (fr.frame_at(a.clock_time()).clamp(wa, wb) - wa).div_euclid(step) * step)
    }

    /// The last cached frame after the current one and before `target`, in play order: what a
    /// preview behind its sound shows (`None` when no such frame is cached). The walk stops at
    /// the first frame that isn't before the sound's, which it can step over without meeting
    /// (a picture off the plan's frame step) or never reach (a sound behind the picture).
    fn latest_cached_before(&self, pass: &PlaybackPass<'_>, target: i64) -> Option<i64> {
        let PlaybackPass { plan, series, cur, window, looping, .. } = *pass;
        let cached = self.frames.cached_frames(&series);
        let mut best = None;
        for k in 1..=(window.last - window.first + 1).max(1) {
            let Some(f) = plan.map_or(Some(cur + k), |p| p.frame_after(cur, k)) else { break };
            if !between_picture_and_sound(cur, f, target, looping) {
                break;
            }
            if cached.binary_search(&f).is_ok() {
                best = Some(f);
            }
        }
        best
    }

    /// The wall clock advances the preview when no audio device drives it, and shows the target
    /// frame (or holds when it is not cached yet). While frames arrive slower than the frame
    /// rate it runs at their pace ([`pace_for`]): every frame shows, evenly, as After Effects
    /// plays a preview it hasn't cached yet; real time again once the frames ahead are cached.
    fn advance_frame_clock(&mut self, pass: &PlaybackPass<'_>, step: i64, fit_ahead: i64) {
        let PlaybackPass { ctx, now, plan, video, frame_rate: fr, cur, series, .. } = *pass;
        let FrameWindow { last: wb, .. } = pass.window;
        // The plan carries the rate the preview was started with (and `play_with` copies it to
        // `Playback::fps`). Deriving the frame from the plan keeps a logic-only tick correct even
        // when a host/test supplies only the plan; a stale duplicate cannot hold playback open.
        let fps = plan.map_or(self.playback.fps, |p| p.fps);
        let dt = self.playback.paced_at.map_or(0.0, |t| (now - t).max(0.0));
        self.playback.paced_at = Some(now);
        if video {
            // The lead counts out of twice the prefetch depth: the prefetch keeps that many
            // frames coming (or the frames left before the end), so a full lead means rendering
            // keeps up. No more than the RAM preview holds ([`pace_horizon`]).
            let horizon = pace_horizon(pass.ahead, fit_ahead);
            let prev = if self.playback.pace > 0.0 { self.playback.pace } else { fps };
            let pace = if horizon > 0 {
                let (lead, target) = self.cached_lead(pass, horizon);
                pace_for(prev, lead, target, fps, dt)
            } else {
                fps
            };
            self.playback.pace = pace;
            if pace < fps * 0.8 {
                self.step_down(now, pass.scale);
            }
            if pace < fps * 0.98 {
                self.advance_paced(pass, pace, dt);
                return;
            }
            if self.playback.pace_acc.take().is_some() {
                // Back to real time from here.
                self.playback.start_wall = now;
                self.playback.start_frame = cur;
            }
        }
        let elapsed = now - self.playback.start_wall;
        let n = ((elapsed * fps) / step as f64).floor() as i64;
        let target = pass.frame_after(self.playback.start_frame, n);
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
            self.step_down(now, pass.scale);
            let next = pass.frame_after(cur, 1).unwrap_or(cur);
            if self.frames.is_cached(&FrameKey { frame: next, ..series }) {
                self.session.set_time(fr.tick_of(next));
            }
            self.playback.start_wall = now;
            self.playback.start_frame = fr.frame_at(self.session.time());
        }
        ctx.request_repaint();
    }

    /// The paced clock: it owes `pace` played frames a second (plus what the last passes left
    /// over) and shows them one by one as far as they are cached; on a frame that isn't it holds
    /// (owing one frame, so that frame shows the moment it arrives), and at the end of a preview
    /// that doesn't loop it stops.
    fn advance_paced(&mut self, pass: &PlaybackPass<'_>, pace: f64, dt: f64) {
        let PlaybackPass { ctx, now, frame_rate: fr, cur, series, .. } = *pass;
        let step = pass.plan.map_or(1, |p| p.step).max(1) as f64;
        let mut owed = self.playback.pace_acc.unwrap_or(0.0) + dt * pace / step;
        let mut f = cur;
        while owed >= 1.0 {
            match pass.frame_after(f, 1) {
                Some(n) if self.frames.is_cached(&FrameKey { frame: n, ..series }) => {
                    f = n;
                    owed -= 1.0;
                }
                Some(_) => {
                    owed = 1.0;
                    break;
                }
                None => {
                    self.session.set_time(fr.tick_of(pass.window.last));
                    self.stop();
                    return;
                }
            }
        }
        self.playback.waiting = owed >= 1.0;
        if f != cur {
            self.session.set_time(fr.tick_of(f));
            self.playback.frame_shown(now);
        }
        self.playback.pace_acc = Some(owed);
        ctx.request_repaint();
    }

    /// Cached frames among the next `max` in play order, and how many of those there are (fewer
    /// near the end of a preview that doesn't loop: its cached tail is a full lead). See
    /// [`lead_of`].
    fn cached_lead(&self, pass: &PlaybackPass<'_>, max: i64) -> (i64, i64) {
        lead_of(|k| pass.frame_after(pass.cur, k), &self.frames.cached_frames(&pass.series), max)
    }

    /// Settings ▸ Previews ▸ Lower Resolution When Playback Can't Keep Up: a preview at
    /// Resolution Auto plays under 80 % of its frame rate (its pace, or stalled on a frame that
    /// isn't cached) two seconds after it started or last stepped down (the first frames at a
    /// new scale, and the GPU's warm-up frames, take a moment), so it plays on at half `scale`,
    /// no lower than the Adaptive Resolution Limit. An explicit Resolution is kept.
    // ponytail: it never steps back up during a play, and frames cached at the higher scale aren't
    // shown once it stepped down; add both if loops over a partly cached comp need them.
    fn step_down(&mut self, now: f64, scale: f64) {
        let pb = &self.playback;
        let since = pb.adaptive.map_or(pb.starting_since, |(_, t)| t);
        let step = pb.plan.map_or(1, |p| p.step).max(1) as f64;
        let lower = scale * 0.5;
        if !self.session.prefs.previews.adaptive_playback
            || pb.res.is_some()
            || !self.viewer_res().is_auto()
            || now - since < 2.0
            // (a rate from before a stall: nothing shown for a second isn't keeping up)
            || (pb.shown_at.back().is_some_and(|t| now - t <= 1.0) && pb.achieved_fps().is_some_and(|f| f * step >= pb.fps * 0.8))
            || lower < self.session.prefs.adaptive_limit() - 1e-9
        {
            return;
        }
        self.playback.adaptive = Some((lower, now));
        self.playback.shown_at.clear();
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
        // Frames an edit kept in RAM are on disk under the comp's previous content only.
        // Again soon while frames wait to be written (a refused write is retried then).
        #[cfg(not(target_arch = "wasm32"))]
        if self.frames.persist_missing(&self.render_source(), series, &self.frame_opts(ItemId(series.comp), series.scale as f64 / 1000.0)) {
            ctx.request_repaint_after(std::time::Duration::from_millis(500));
        }
        // An empty work area has nothing to cache.
        let n = wb - wa + 1;
        if n < 1 {
            return;
        }
        let cur = comp.frame_rate.frame_at(self.session.time()).clamp(wa, wb);
        // The viewer's exact factor (1/3, not the key's 0.333): it shows these frames.
        let scale = frames::render_scale(series.scale, comp.resolution.factor());
        let fit = self.frames_that_fit(comp, scale).min(n);
        // One thread each (`Frames::request_background`), so every frame thread takes one.
        let slots = self.frames_parallelism().max(1);
        let mut queued = self.frames.inflight();
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
            self.request_frame_background(ItemId(series.comp), f, scale);
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
        ((self.frames.budget() as f64 * 0.9 / self.frame_bytes(comp, scale)) as i64).max(1)
    }

    /// Bytes of one comp-sized viewer frame at `scale` (Extended Viewer frames are larger; 32 bpc
    /// frames are half floats, twice the bytes of 8-bit ones).
    fn frame_bytes(&self, comp: &effectcraft_engine::project::Comp, scale: f64) -> f64 {
        let side = |n: u32| (n as f64 * scale).ceil().max(1.0);
        side(comp.width) * side(comp.height) * frames::frame_pixel_bytes(&self.session.project.settings) as f64
    }

    /// Frames rendered at once by a preview or idle caching: the lanes (in the browser the frame
    /// workers; without them frames render one at a time on the UI thread, `Frames::pump`).
    fn frames_parallelism(&self) -> usize {
        self.frames.lanes()
    }
}

#[cfg(test)]
mod pace_tests {
    use super::{lead_of, pace_for, pace_horizon};

    /// The lead is measured over no more frames than the RAM preview keeps ahead of the current
    /// one, so a full cache plays in real time. (It counted out of twice the prefetch depth: with
    /// room for 4 frames, at most 2 after the current one were ever cached, and a preview that
    /// rendered at once played at a quarter of its frame rate.)
    #[test]
    fn the_pace_horizon_fits_in_the_ram_preview() {
        // A roomy budget: twice the prefetch depth.
        assert_eq!(pace_horizon(6, 400), 12);
        // Room for 4 frames: the prefetch requests offsets 0..3, so 2 after the current one.
        assert_eq!(pace_horizon(6, 3), 2);
        let (lead, target) = lead_of(|k| Some(10 + k), &[10, 11, 12], pace_horizon(6, 3));
        assert!((pace_for(30.0, lead, target, 30.0, 10.0) - 30.0).abs() < 1e-6, "a full cache plays in real time");
        // Room for the current frame only (or nothing): no lead to measure.
        assert_eq!(pace_horizon(6, 1), 0);
        assert_eq!(pace_horizon(6, 0), 0);
        // No prefetch depth: still the next frame.
        assert_eq!(pace_horizon(0, 400), 1);
    }

    /// The pace follows the cached lead: real time with a full lead, a third of it with a third,
    /// never under an eighth, unchanged with nothing ahead, and smoothed over time.
    #[test]
    fn pace_follows_the_cached_lead() {
        // Settled (a long dt): proportional to the lead.
        assert!((pace_for(30.0, 12, 12, 30.0, 10.0) - 30.0).abs() < 1e-6);
        assert!((pace_for(30.0, 4, 12, 30.0, 10.0) - 10.0).abs() < 1e-6);
        assert!((pace_for(30.0, 1, 100, 30.0, 10.0) - 3.75).abs() < 1e-6, "never under an eighth");
        assert_eq!(pace_for(17.0, 0, 12, 30.0, 10.0), 17.0, "no frame ahead: unchanged");
        // Smoothing: after one time constant it has gone 63 % of the way.
        let p = pace_for(30.0, 4, 12, 30.0, 0.5);
        assert!((p - (30.0 - 20.0 * (1.0 - (-1.0f64).exp()))).abs() < 1e-6);
        assert_eq!(pace_for(30.0, 4, 12, 30.0, 0.0), 30.0, "no time passed: unchanged");
        assert!(pace_for(10.0, 12, 12, 30.0, 0.5) > 10.0, "and up again as the lead fills");
    }

    /// Near the end of a preview that doesn't loop, the lead counts out of the frames left: a
    /// cached tail plays in real time. (It counted out of the full depth, and a cached preview
    /// slowed down over its last frames, a short one for its whole length.)
    #[test]
    fn a_cached_end_is_a_full_lead() {
        let cached: Vec<i64> = (0..=20).collect();
        let to_20 = |cur: i64| move |k: i64| Some(cur + k).filter(|f| *f <= 20);
        assert_eq!(lead_of(to_20(5), &cached, 12), (12, 12));
        assert_eq!(lead_of(to_20(16), &cached, 12), (4, 4));
        assert_eq!(lead_of(to_20(20), &cached, 12), (0, 0));
        let (lead, target) = lead_of(to_20(16), &cached, 12);
        assert!((pace_for(30.0, lead, target, 30.0, 10.0) - 30.0).abs() < 1e-6);
        // Frames left uncached still count against it.
        assert_eq!(lead_of(to_20(16), &[17, 19], 12), (2, 4));
        // Looping: the walk wraps, always `max` frames.
        assert_eq!(lead_of(|k| Some((18 + k) % 21), &cached, 12), (12, 12));
    }
}

#[cfg(test)]
mod startup_tests {
    use super::{between_picture_and_sound, startup_frames};

    /// The frames a preview behind its sound may show are only those before the sound's frame in
    /// play order, so the picture is never ahead of the sound (#2473).
    #[test]
    fn a_dropped_frame_preview_never_shows_past_the_sound() {
        // Sound ahead of the picture: only the frames between them.
        assert!(between_picture_and_sound(10, 12, 14, false));
        assert!(!between_picture_and_sound(10, 14, 14, false), "not the sound's own frame");
        assert!(!between_picture_and_sound(10, 15, 14, true), "nor one past it, stepped over off the grid");
        assert!(!between_picture_and_sound(10, 10, 14, true), "nor the current frame");
        // A looping preview whose sound wrapped to the start: the end of the loop, then its start.
        assert!(between_picture_and_sound(90, 95, 3, true));
        assert!(between_picture_and_sound(90, 1, 3, true));
        assert!(!between_picture_and_sound(90, 3, 3, true));
        assert!(!between_picture_and_sound(90, 50, 3, true), "the wrapped walk stops before reaching the picture again");
        // Without a loop, a sound behind the picture allows nothing ahead of it.
        assert!(!between_picture_and_sound(90, 95, 3, false));
        assert!(!between_picture_and_sound(90, 1, 3, false));
        // The sound on the picture's frame: nothing.
        assert!(!between_picture_and_sound(10, 11, 10, true));
    }

    /// The frames a preview with sound waits for are always requested and fit together (#947).
    #[test]
    fn startup_waits_only_for_frames_it_requests_and_can_keep() {
        // A 64×36 frame: 9216 bytes. A 46 080-byte budget fits 4 of them (90 %); the prefetch
        // window then requests offsets 0..3, so the start needs those 3, not 4 it never asks for.
        let frame = 64 * 36 * 4;
        let fit = 4;
        let window = (32i64 * 3).min(fit - 1);
        assert_eq!(startup_frames(32, window, 46_080, frame), 3);
        // A roomy budget: the prefetch's `ahead` frames.
        assert_eq!(startup_frames(8, 24, 100_000_000, frame), 8);
        // Extended Viewer: a 160×120 comp renders 320×248 frames. 952 320 bytes fit 11 comp-sized
        // frames but only 2 real ones (90 %): wait for 2, not 4.
        assert_eq!(startup_frames(4, 11, 952_320, 320 * 248 * 4), 2);
        // Nothing fits, or no window: still the current frame, never 0.
        assert_eq!(startup_frames(8, 0, 0, frame), 1);
        // A zero frame size counts as 1 byte: no division by zero.
        assert_eq!(startup_frames(8, 24, 10, 0), 8);
    }
}
