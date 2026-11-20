//! Audio mixdown of a composition: every audible layer (footage with audio, precomps, and any
//! layer with a sound-generating effect such as Tone) summed with its Audio Levels (dB, per
//! channel), respecting in/out points, the Audio switch and solo. Layer audio effects
//! (Effect > Audio, see [`effectcraft_effects::audio_fx`]) run on each layer's audio before
//! its levels; Backwards plays the layer's [in, out) span reversed.
//!
//! Callers mix in short blocks (one video frame, an audio device buffer, or a second of export).
//! Effect parameters are evaluated once per block. Animated Audio Levels, and animated effects
//! that work sample by sample (Stereo Mixer), are evaluated on a grid of comp samples every
//! millisecond instead, so keyframed fades ramp smoothly whatever the block size. To make the
//! result independent of block boundaries, layer effects are run from a cleared state over a
//! pre-roll of earlier audio and only the block's tail is kept — so preview playback and export
//! produce identical samples.
//!
//! Time-stretched layers are resampled linearly, so pitch follows the speed. Time Remap uses the
//! same source-time evaluation as video, at audio sample resolution (keyframes and expressions),
//! and is resampled the same way; where the remap holds one source time the layer is silent, with
//! 2 ms fades at the edges so a hold does not click. A reversed layer
//! (negative stretch, Layer > Time > Time-Reverse Layer) plays its audio backwards at the same
//! speed: comp sample `k` plays the source just before layer time `layer_time(k)`, so at -100 %
//! with the start at the out point (After Effects' layout) it is the exact mirror of the
//! layer's span. Backwards on a reversed layer plays it forwards again.

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use effectcraft_effects::Params;
use effectcraft_effects::audio_fx;
use effectcraft_project::{Comp, GroupKind, ItemId, ItemKind, Layer, LayerSource, Node, Project};
use effectcraft_time::{TICKS_PER_SECOND, Tick};

use crate::{AudioExpression, EvalCtx, ExprHost, FootageSource};

const MAX_DEPTH: usize = 16;

/// Fastest a layer's audio plays (source samples per comp sample): 1 % stretch, the smallest
/// Time Stretch accepts. The mix plays the clamped stretch (`Layer::effective_stretch`, #3247),
/// so a layer is never faster; a faster speed would be silent instead of reading unbounded
/// source: a last-resort guard.
const MAX_SPEED: f64 = 100.0;

/// Most source sample frames one read asks a footage item or precomp for; a longer read is made
/// in pieces, so memory stays bounded however fast a layer plays and however deep stretched
/// precomps nest. The work is not bounded: each 1 % precomp level still reads 100 times its
/// span.
pub(crate) const MAX_READ: usize = 1 << 18;

/// The most effect preroll (history before a block) mixed, in seconds, across a layer's effects.
const MAX_PREROLL_SECONDS: f64 = 30.0;

/// The longest footage a waveform summarises (a day).
const MAX_PEAK_SECONDS: i64 = 24 * 3600;

/// `frames` stereo sample frames of `comp`'s mix from comp time `start` at `rate` Hz,
/// interleaved (L R L R …). Returns an error rather than partial audio when the work is excessive.
pub fn mix_comp(
    project: &Project,
    footage: &dyn FootageSource,
    expr: Option<&dyn ExprHost>,
    comp: ItemId,
    start: Tick,
    frames: usize,
    rate: u32,
) -> Result<Vec<f32>, AudioError> {
    mix_comp_with_control(&MixSources { project, footage, expr }, comp, start, frames, rate, &MixControl::default())
}

/// Why a mix produced no audio. A failure aborts the whole block: callers must not play or export
/// a partial mix.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AudioError {
    Cancelled,
    DeadlineExceeded,
    WorkLimit,
}
impl std::fmt::Display for AudioError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Cancelled => "audio mixing cancelled",
            Self::DeadlineExceeded => "audio mixing exceeded the browser preview time budget",
            Self::WorkLimit => "audio mixing exceeded its work limit; simplify time stretching, remapping, or nested compositions",
        })
    }
}
impl std::error::Error for AudioError {}

/// Controls shared by every layer, source read and nested composition in one mix call.
#[derive(Default)]
pub struct MixControl {
    /// Stop an in-flight mix, including expression evaluation when the host supports it.
    pub cancel: Option<Arc<AtomicBool>>,
    /// An absolute deadline shared across mixer calls made during one UI frame.
    /// Synchronous source reads and effects are checked at their call boundaries.
    pub deadline: Option<web_time::Instant>,
    /// Override the default allowance of [`WORK_PER_FRAME`] source or sample operations per
    /// output frame plus [`WORK_SLACK`]. One buffer is also capped at [`MAX_ALLOC_FRAMES`].
    pub max_work: Option<usize>,
    /// How long the mix's expressions may run in all (none by default). Only time spent
    /// evaluating expressions counts, not reading or decoding footage, so a slow first read
    /// of a long file cannot trip it.
    pub max_expression_time: Option<Duration>,
    /// Shared JavaScript yield allowance (1024 yields of up to 20,000 instructions by default).
    pub max_expression_yields: Option<usize>,
}

/// Source or sample operations one output frame may cost (all layers, nested mixes included).
/// A layer at the 1 % stretch limit reads 100 source frames per output frame, so two of them
/// nested (a 1 % layer in a 1 % precomp: 10⁴) fit; a third level, or a remap that jumps every few
/// samples, does not and fails instead of hanging.
const WORK_PER_FRAME: usize = 1 << 14;
/// Extra work every mix may do, so a one-frame call can still read a page.
const WORK_SLACK: usize = 65_536;
/// The most stereo frames one buffer holds: a block plus 30 s of effect pre-roll at 192 kHz.
const MAX_ALLOC_FRAMES: usize = 1 << 23;
/// Source frames a Time Remap read fetches at a time, aligned on absolute multiples of it so the
/// result does not depend on where a block starts.
const REMAP_PAGE: i64 = 4096;
/// Page fetches a mix may make per output frame (one per 64 frames, so a remap that jumps more
/// often than that fails) plus [`PAGE_SLACK`] for nested remaps and small blocks.
const FRAMES_PER_PAGE: usize = 64;
const PAGE_SLACK: usize = 64;
/// Fade at each edge of a Time Remap hold, in milliseconds.
const HOLD_FADE_MS: usize = 2;
/// Poll long sample loops in batches instead of reading the browser clock for every sample.
const CHECK_INTERVAL: usize = 256;

/// Shared interruption state for every expression in one mix, including nested evaluations.
/// Clones share the sticky resource failure, the time spent and the yield allowance, so a new
/// sample cannot reset them.
#[derive(Clone)]
pub struct AudioEvalControl {
    cancel: Option<Arc<AtomicBool>>,
    deadline: Option<web_time::Instant>,
    remaining: Rc<Cell<usize>>,
    failed: Rc<Cell<bool>>,
    spent: Rc<Cell<Duration>>,
    /// When the evaluation now running began (it counts towards the limit before it ends).
    #[cfg(not(target_arch = "wasm32"))]
    running: Rc<Cell<Option<web_time::Instant>>>,
    limit: Option<Duration>,
}
impl AudioEvalControl {
    fn new(control: &MixControl) -> Self {
        Self {
            cancel: control.cancel.clone(),
            deadline: control.deadline,
            remaining: Rc::new(Cell::new(control.max_expression_yields.unwrap_or(1024))),
            failed: Rc::new(Cell::new(false)),
            spent: Rc::new(Cell::new(Duration::ZERO)),
            #[cfg(not(target_arch = "wasm32"))]
            running: Rc::new(Cell::new(None)),
            limit: control.max_expression_time,
        }
    }
    /// Mark a runtime resource limit; ordinary expression errors still use keyframed fallback.
    pub fn fail(&self) {
        self.failed.set(true);
    }
    /// Whether the shared allowance (yields, expression time) has run out.
    pub fn exhausted(&self) -> bool {
        if self.limit.is_some_and(|limit| self.time_used() >= limit) {
            self.fail();
        }
        self.failed.get()
    }
    /// Time spent in expressions so far, including the evaluation now running.
    fn time_used(&self) -> Duration {
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(start) = self.running.get() {
            return self.spent.get().saturating_add(start.elapsed());
        }
        self.spent.get()
    }
    /// Check cancellation and resource limits without charging another instruction chunk.
    pub fn interrupted(&self) -> bool {
        self.cancel.as_ref().is_some_and(|c| c.load(Ordering::Relaxed)) || self.deadline_exceeded() || self.exhausted()
    }
    fn deadline_exceeded(&self) -> bool {
        self.deadline.is_some_and(|deadline| web_time::Instant::now() >= deadline)
    }
    /// Charge one budgeted JavaScript yield; this also bounds scripts on clockless WASM builds.
    pub fn yielded(&self) -> bool {
        if let Some(left) = self.remaining.get().checked_sub(1) {
            self.remaining.set(left);
        } else {
            self.fail();
        }
        self.interrupted()
    }
    /// Run one expression evaluation, counting its time (where there is a clock).
    fn timed<R>(&self, run: impl FnOnce() -> R) -> R {
        #[cfg(not(target_arch = "wasm32"))]
        {
            if self.running.get().is_some() {
                return run();
            }
            let start = web_time::Instant::now();
            self.running.set(Some(start));
            let out = run();
            self.running.set(None);
            self.spent.set(self.spent.get().saturating_add(start.elapsed()));
            out
        }
        #[cfg(target_arch = "wasm32")]
        run()
    }
}

/// What a mix may still do: cancellation, source and sample operations, page fetches and the
/// expressions' own allowance. A failure is sticky for the whole mix.
struct Budget<'a> {
    control: &'a MixControl,
    expressions: AudioEvalControl,
    remaining: Cell<usize>,
    pages: Cell<usize>,
    error: Cell<Option<AudioError>>,
}
impl Budget<'_> {
    fn fail(&self, error: AudioError) {
        if self.error.get().is_none() {
            self.error.set(Some(error));
        }
    }
    /// Whether the mix can go on (not cancelled, expressions within their allowance). Charges nothing.
    fn ok(&self, footage: &dyn FootageSource) -> bool {
        if self.error.get().is_some() {
            return false;
        }
        if footage.called_off() || self.control.cancel.as_ref().is_some_and(|c| c.load(Ordering::Relaxed)) {
            self.fail(AudioError::Cancelled);
            return false;
        }
        if self.expressions.deadline_exceeded() {
            self.fail(AudioError::DeadlineExceeded);
            return false;
        }
        if self.expressions.exhausted() {
            self.fail(AudioError::WorkLimit);
            return false;
        }
        true
    }
    /// Charge `frames` source or sample operations before doing them.
    fn charge(&self, frames: usize, footage: &dyn FootageSource) -> bool {
        if !self.ok(footage) {
            return false;
        }
        if frames > self.remaining.get() {
            self.fail(AudioError::WorkLimit);
            return false;
        }
        self.remaining.set(self.remaining.get() - frames);
        true
    }
    /// Allow one buffer of `frames` stereo frames.
    fn alloc(&self, frames: usize) -> bool {
        if frames > MAX_ALLOC_FRAMES {
            self.fail(AudioError::WorkLimit);
            return false;
        }
        true
    }
    /// Add `frames` operations to the allowance: work that is paid for elsewhere (effect pre-roll,
    /// a source page), so it does not eat into what the output block itself may cost.
    fn grant(&self, frames: usize) {
        // An explicit allowance is a hard cap: nothing is added to it.
        if self.control.max_work.is_none() {
            self.remaining.set(self.remaining.get().saturating_add(frames));
        }
    }
    /// One Time Remap page fetch.
    fn take_page(&self) -> bool {
        match self.pages.get().checked_sub(1) {
            Some(left) => {
                self.pages.set(left);
                true
            }
            None => {
                self.fail(AudioError::WorkLimit);
                false
            }
        }
    }
}

/// What a mix reads: the project, its footage and, optionally, the expression host.
#[derive(Clone, Copy)]
pub struct MixSources<'a> {
    pub project: &'a Project,
    pub footage: &'a dyn FootageSource,
    pub expr: Option<&'a dyn ExprHost>,
}

/// Mix with cancellation and a work budget; failures discard the entire output block.
pub fn mix_comp_with_control(
    sources: &MixSources<'_>,
    comp: ItemId,
    start: Tick,
    frames: usize,
    rate: u32,
    control: &MixControl,
) -> Result<Vec<f32>, AudioError> {
    let MixSources { project, footage, expr } = *sources;
    let cancellable = control.cancel.as_ref().map(|cancel| crate::CancellableFootage { inner: footage, cancel: cancel.clone() });
    let footage: &dyn FootageSource = cancellable.as_ref().map_or(footage, |source| source);
    let budget = Budget {
        control,
        expressions: AudioEvalControl::new(control),
        remaining: Cell::new(control.max_work.unwrap_or_else(|| frames.saturating_mul(WORK_PER_FRAME).saturating_add(WORK_SLACK))),
        pages: Cell::new(frames / FRAMES_PER_PAGE + PAGE_SLACK),
        error: Cell::new(None),
    };
    if !budget.alloc(frames) || !budget.ok(footage) {
        return Err(budget.error.get().unwrap_or(AudioError::WorkLimit));
    }
    let mut out = vec![0.0f32; frames * 2];
    mix_into(&AudioSources { project, footage, expr, rate, budget: &budget }, comp, start, &mut out, 0);
    budget.ok(footage);
    match budget.error.get() {
        Some(e) => Err(e),
        None => Ok(out),
    }
}

/// Whether the comp has anything audible (footage with audio, a sound generator, or a precomp
/// that has).
pub fn comp_has_audio(project: &Project, comp: ItemId) -> bool {
    // Breadth first, so each comp is looked at once and at its shallowest level: a comp reached
    // first through a long chain (past MAX_DEPTH) is still found by a shorter route, and a cycle
    // cannot hide a result (nothing is cached as "silent" while it is being explored).
    let mut seen = std::collections::HashSet::from([comp]);
    let mut level = vec![comp];
    for _ in 0..MAX_DEPTH {
        let mut next = vec![];
        for id in level {
            let Some(c) = project.comp(id) else { continue };
            for l in c.layers.iter().filter(|l| l.switches.audio) {
                match &l.source {
                    _ if generates(l) => return true,
                    LayerSource::Footage { item } if matches!(project.item(*item).map(|i| &i.kind), Some(ItemKind::Footage(f)) if f.has_audio && !f.missing) => {
                        return true;
                    }
                    LayerSource::Comp { item } if seen.insert(*item) => next.push(*item),
                    _ => {}
                }
            }
        }
        level = next;
    }
    false
}

/// Enabled audio effects of a layer (in stack order) with their effect ids.
fn audio_effect_groups(l: &Layer) -> impl Iterator<Item = (&str, &effectcraft_project::PropGroup)> {
    l.effects().filter(|_| l.switches.effects).into_iter().flat_map(|fx| fx.groups()).filter(|g| g.enabled).filter_map(|g| match &g.kind {
        GroupKind::Effect { effect } if audio_fx::is_audio_effect(effect) => Some((effect.as_str(), g)),
        _ => None,
    })
}

/// The layer makes sound on its own (Tone).
fn generates(l: &Layer) -> bool {
    audio_effect_groups(l).any(|(id, _)| audio_fx::generates_audio(id))
}

/// Does an enabled audio effect (Backwards) play the layer's [in, out) span reversed?
pub fn plays_backwards(l: &Layer) -> bool {
    audio_effect_groups(l).any(|(id, _)| audio_fx::reverses(id))
}

/// Does the layer contribute to the mix (Audio switch on and something to hear)?
pub fn audible(project: &Project, l: &Layer) -> bool {
    l.switches.audio
        && (generates(l)
            || match &l.source {
                LayerSource::Footage { item } => matches!(project.item(*item).map(|i| &i.kind), Some(ItemKind::Footage(f)) if f.has_audio),
                LayerSource::Comp { .. } => true,
                _ => false,
            })
}

/// Linear gain of an Audio Levels value in dB; -96 dB and below is silence.
pub fn db_to_gain(db: f64) -> f32 {
    if db <= -96.0 { 0.0 } else { 10f64.powf(db / 20.0) as f32 }
}

fn units_ceil(t: Tick, rate: u32) -> i64 {
    let n = t.0 as i128 * rate as i128;
    let d = TICKS_PER_SECOND as i128;
    (n.div_euclid(d) + i128::from(n.rem_euclid(d) != 0)) as i64
}

/// Samples between evaluations of animated levels and per-sample effects: one millisecond, on
/// a grid of comp sample indices so the result does not depend on where a block starts.
fn param_step(rate: u32) -> i64 {
    i64::from(rate / 1000).max(1)
}

/// Does any property of the group change over time?
fn animated(g: &effectcraft_project::PropGroup) -> bool {
    g.children.iter().any(|c| match c {
        Node::Prop(p) => p.is_animated() || p.has_expression(),
        Node::Group(sub) => animated(sub),
    })
}

/// The clock (seconds) a layer's audio effects run on at comp time `t` (oscillator phase): comp
/// time since the layer's start. It advances one sample per sample at any stretch, forwards or
/// reversed, so blocks join up; at 100 % it is the layer time.
fn fx_clock(l: &Layer, t: Tick) -> f64 {
    Tick(t.0.saturating_sub(l.start_time.0)).seconds()
}

fn sample_tick(i: i64, rate: u32) -> Tick {
    // Keep the requested sample when the rate does not divide the tick clock.
    let t = Tick::from_units(i, i64::from(rate));
    if t.to_units_floor(i64::from(rate)) < i { Tick(t.0.saturating_add(1)) } else { t }
}

struct MixEnv<'a> {
    project: &'a Project,
    footage: &'a dyn FootageSource,
    expr: Option<&'a dyn ExprHost>,
    comp_id: ItemId,
    comp: &'a Comp,
    rate: u32,
    depth: usize,
    budget: &'a Budget<'a>,
}

impl MixEnv<'_> {
    fn ctx(&self, t: Tick) -> EvalCtx<'_> {
        let mut ctx = EvalCtx::new(self.project, self.comp_id, self.comp, t);
        ctx.expr = self.expr;
        ctx
    }

    /// The layer's own (pre-effect) audio for comp sample indices `i0 .. i0 + n`, silent
    /// outside its [in, out) span. Comp sample index `k` maps to `in + out − 1 − k` when
    /// `reversed` (the Backwards effect).
    fn layer_raw(&self, l: &Layer, i0: i64, n: usize, reversed: bool, pre_roll: usize) -> Vec<f32> {
        let rate = self.rate;
        // Nothing is allocated, read or evaluated once the mix has failed or the buffer is too big.
        if !self.budget.ok(self.footage) || !self.budget.alloc(n) {
            return Vec::new();
        }
        let mut out = vec![0.0f32; n * 2];
        let (in_i, out_i) = (units_ceil(l.in_point, rate), units_ceil(l.out_point, rate));
        if reversed {
            // Mirror the clipped range, fetch it forwards, reverse.
            let a = i0.max(in_i);
            let b = (i0 + n as i64).min(out_i);
            if a >= b {
                return out;
            }
            let (ma, mb) = (in_i + out_i - b, in_i + out_i - a);
            let fwd = self.layer_raw(l, ma, (mb - ma) as usize, false, pre_roll);
            if fwd.len() != (mb - ma) as usize * 2 {
                return out;
            }
            for k in a..b {
                let j = (in_i + out_i - 1 - k - ma) as usize;
                let o = (k - i0) as usize;
                out[o * 2] = fwd[j * 2];
                out[o * 2 + 1] = fwd[j * 2 + 1];
            }
            return out;
        }
        let a = (in_i - i0).clamp(0, n as i64) as usize;
        let b = (out_i - i0).clamp(0, n as i64) as usize;
        if a >= b {
            return out;
        }
        // Time Remap replaces the stretch's source time: its value at each comp sample is the
        // source time to play.
        if l.props.get("timeRemap").is_some() {
            self.remapped_raw(l, i0, a, b, &mut out, pre_roll);
            return out;
        }
        // Within ±1 % to ±9900 % (#3247): a stretch of 1e-300 % set in memory read about
        // usize::MAX source samples for one block. A negative stretch plays backwards (#3239).
        let speed = 100.0 / l.effective_stretch();
        let aspeed = speed.abs();
        // The clamped stretch keeps the speed within MAX_SPEED; NaN is checked anyway. The
        // tolerance keeps exactly 1 % audible whatever the float noise in the stretch.
        if aspeed.is_nan() || aspeed > MAX_SPEED * (1.0 + 1e-9) {
            return out;
        }
        let backwards = speed < 0.0;
        // Comp samples per piece, so that one piece reads at most MAX_READ source frames (its
        // span, the fraction it starts past a source sample, and the sample after its end).
        let piece = (((MAX_READ - 4) as f64 / aspeed) as usize).clamp(1, b - a);
        let mut p0 = a;
        while p0 < b {
            let p1 = p0.saturating_add(piece).min(b);
            // Forwards a piece reads the source from the layer time of its first sample. Backwards
            // it reads from the layer time of its end and plays what it read last to first, so
            // comp sample `k` plays the source just before `layer_time(k)` — at -100 % the exact
            // mirror Backwards gives a forward layer.
            let origin = sample_tick(i0.saturating_add((if backwards { p1 } else { p0 }) as i64), rate);
            let Some(run) = self.source_run(l, l.layer_time(origin), p1 - p0, aspeed) else { return out };
            for (i, f) in run.as_chunks::<2>().0.iter().take(p1 - p0).enumerate() {
                let k = if backwards { (p1 - 1).saturating_sub(i) } else { p0 + i };
                if let Some(o) = out.get_mut(k * 2..k * 2 + 2) {
                    o.copy_from_slice(f);
                }
            }
            p0 = p1;
        }
        out
    }

    /// `m` sample frames of the layer's source audio from layer time `lt` on, played at `speed`
    /// (≥ 0) source samples per comp sample: blended linearly between source samples, or copied
    /// as they are at speed 1. `None` when the layer has no source audio.
    ///
    /// Sources deliver samples on their sample grid (from the sample at or before the time asked
    /// for), so a resampled run reads from that grid sample and starts `x` at `lt`'s exact
    /// position past it: a run then continues where the previous block or piece left off.
    fn source_run(&self, l: &Layer, lt: Tick, m: usize, speed: f64) -> Option<Vec<f32>> {
        let rate = self.rate;
        let same = (speed - 1.0).abs() < 1e-9;
        let s0 = lt.to_units_floor(i64::from(rate));
        let from = sample_tick(s0, rate);
        // `lt`'s position past sample s0, in samples (0 ≤ frac < 1); copies ignore it.
        let tps = TICKS_PER_SECOND as i128;
        let frac = if same { 0.0 } else { (i128::from(lt.0) * i128::from(rate)).rem_euclid(tps) as f64 / tps as f64 };
        // Resampled, one more source sample than the run covers: the last sample blends with it.
        let src_n = ((frac + m as f64 * speed).ceil() as usize).saturating_add(usize::from(!same)).max(1);
        if !self.budget.charge(src_n, self.footage) {
            return None;
        }
        let buf = match &l.source {
            LayerSource::Footage { item } => match self.project.item(*item).map(|i| &i.kind) {
                Some(ItemKind::Footage(f)) if f.has_audio => self.footage.audio(*item, f, from, src_n, rate).unwrap_or_else(|| vec![0.0; src_n * 2]),
                _ => return None,
            },
            LayerSource::Comp { item } => {
                if self.project.comp_contains(*item, self.comp_id) {
                    return None;
                }
                let mut sub = vec![0.0f32; src_n * 2];
                mix_into(
                    &AudioSources { project: self.project, footage: self.footage, expr: self.expr, rate, budget: self.budget },
                    *item,
                    from,
                    &mut sub,
                    self.depth + 1,
                );
                sub
            }
            _ => return None,
        };
        if !self.budget.ok(self.footage) {
            return None;
        }
        let mut out = vec![0.0f32; m * 2];
        for i in 0..m {
            if i.is_multiple_of(CHECK_INTERVAL) && !self.budget.ok(self.footage) {
                return None;
            }
            // The source sample at fractional index `x`, blended linearly with the next one unless the
            // layer plays at its own speed (then the samples are copied as they are).
            let x = frac + i as f64 * speed;
            let j = if same { i } else { (x as usize).min(src_n - 1) };
            if j * 2 + 1 >= buf.len() {
                break;
            }
            let k = if same || (j + 1) * 2 + 1 >= buf.len() { j } else { j + 1 };
            let f = if same { 0.0 } else { ((x - j as f64) as f32).clamp(0.0, 1.0) };
            out[i * 2] = buf[j * 2] + (buf[k * 2] - buf[j * 2]) * f;
            out[i * 2 + 1] = buf[j * 2 + 1] + (buf[k * 2 + 1] - buf[j * 2 + 1]) * f;
        }
        Some(out)
    }

    /// A property's value at comp time `t` under the mix's shared controls: expressions run through
    /// the host's audio entry point (cancellable, bounded in yields and time), and a resource
    /// failure ends the mix rather than falling back to keyframes for every later sample.
    fn value(&self, l: &Layer, prop: &effectcraft_project::Property, t: Tick) -> effectcraft_keyframe::Value {
        let base = prop.value_at(l.layer_time(t));
        if !prop.has_expression() {
            return base;
        }
        let Some(host) = self.expr else { return base };
        if !self.budget.charge(1, self.footage) {
            return base;
        }
        let ctx = self.ctx(t);
        let value = self.budget.expressions.timed(|| host.eval_audio(&ctx, l, prop, &base, &self.budget.expressions)).unwrap_or_else(|_| base.clone());
        // A resource failure inside the script surfaces here.
        self.budget.ok(self.footage);
        value
    }

    /// Effect parameters at comp time `t`, stopping as soon as the shared controls expire.
    fn params(&self, l: &Layer, g: &effectcraft_project::PropGroup, t: Tick) -> Params {
        let mut p = Params::default();
        for c in &g.children {
            if let Node::Prop(pr) = c {
                if !self.budget.ok(self.footage) {
                    break;
                }
                p.values.insert(pr.match_id.clone(), self.value(l, pr, t));
            }
        }
        p
    }

    /// The Time Remap's source time at comp sample `index`: its keyframes, then its expression
    /// (an exact scalar one is prepared once and run per sample, anything else through the host).
    fn remap_source(&self, l: &Layer, prop: &effectcraft_project::Property, prepared: Option<&dyn AudioExpression>, index: i64) -> Tick {
        let time = sample_tick(index, self.rate);
        let value = match (prepared, prop.value_at(l.layer_time(time))) {
            (Some(prepared), effectcraft_keyframe::Value::Scalar(v)) => effectcraft_keyframe::Value::Scalar(prepared.evaluate(time.seconds(), v).unwrap_or(v)),
            _ => self.value(l, prop, time),
        };
        Tick::from_seconds_f64(value.as_f64())
    }

    /// One page of source audio for Time Remap: `REMAP_PAGE` frames and the sample after them (for
    /// interpolation) from source sample `page`. A page is paid for by its own allowance, so what
    /// it costs does not depend on the block that asked for it; the number of pages is bounded.
    fn remap_page(&self, l: &Layer, page: i64) -> Option<Vec<f32>> {
        let frames = REMAP_PAGE as usize + 1;
        self.budget.grant(frames);
        if !self.budget.charge(frames, self.footage) {
            return None;
        }
        let start = sample_tick(page, self.rate);
        match &l.source {
            LayerSource::Footage { item } => match self.project.item(*item).map(|i| &i.kind) {
                Some(ItemKind::Footage(f)) if f.has_audio => Some(self.footage.audio(*item, f, start, frames, self.rate).unwrap_or_default()),
                _ => None,
            },
            LayerSource::Comp { item } => {
                if self.project.comp_contains(*item, self.comp_id) {
                    return None;
                }
                let mut sub = vec![0.0f32; frames * 2];
                let sources = AudioSources { project: self.project, footage: self.footage, expr: self.expr, rate: self.rate, budget: self.budget };
                mix_into(&sources, *item, start, &mut sub, self.depth + 1);
                Some(sub)
            }
            _ => None,
        }
    }

    /// Comp samples `a .. b` of a layer with Time Remap (comp sample `i0 + k` is output `k`): each
    /// plays the source at its remapped time, interpolated linearly, so the pitch follows the
    /// remap's speed (no pitch correction). Source audio is read in pages aligned on absolute
    /// multiples of `REMAP_PAGE`, so the samples do not depend on where the block starts. Where
    /// the remap holds one source time, no sound plays; 2 ms fades at each edge of a hold take
    /// away the click of stopping and starting.
    fn remapped_raw(&self, l: &Layer, i0: i64, a: usize, b: usize, out: &mut [f32], pre_roll: usize) {
        let Some(prop) = l.props.get("timeRemap") else { return };
        let prepared = prop.expr.as_ref().filter(|_| prop.has_expression()).and_then(|e| self.expr.and_then(|h| h.prepare_audio(&e.text)));
        let fade = (self.rate as usize * HOLD_FADE_MS / 1000).max(1);
        // Source times from one sample before the first output sample (to tell a hold from motion)
        // to a fade past the last (so the fade out of a hold that starts there is known).
        let count = b - a + 2 * fade + 1;
        let Some(first) = i0.checked_add(a as i64).and_then(|v| v.checked_sub(fade as i64 + 1)).filter(|v| v.checked_add(count as i64).is_some()) else {
            return;
        };
        if !self.budget.charge(count, self.footage) {
            return;
        }
        let mut times = Vec::with_capacity(count);
        for k in 0..count {
            if k.is_multiple_of(CHECK_INTERVAL) && !self.budget.ok(self.footage) {
                return;
            }
            times.push(self.remap_source(l, prop, prepared.as_deref(), first + k as i64));
        }
        // A hold is two successive samples inside the layer's [in, out) at one source time: before the
        // first key or past the last the remap is flat too, but there the layer plays no sound anyway.
        let (in_i, out_i) = (units_ceil(l.in_point, self.rate), units_ceil(l.out_point, self.rate));
        let hold = |k: usize| k > 0 && first + k as i64 > in_i && first + (k as i64) < out_i && times.get(k) == times.get(k - 1);
        // How many samples back / ahead the nearest held sample is (usize::MAX: none in range).
        let (mut since, mut until) = (vec![usize::MAX; count], vec![usize::MAX; count]);
        let mut last = None;
        for (k, d) in since.iter_mut().enumerate() {
            if k.is_multiple_of(CHECK_INTERVAL) && !self.budget.ok(self.footage) {
                return;
            }
            if hold(k) {
                last = Some(k);
            }
            *d = last.map_or(usize::MAX, |h| k - h);
        }
        let mut next = None;
        for (k, d) in until.iter_mut().enumerate().rev() {
            if k.is_multiple_of(CHECK_INTERVAL) && !self.budget.ok(self.footage) {
                return;
            }
            if hold(k) {
                next = Some(k);
            }
            *d = next.map_or(usize::MAX, |h| h - k);
        }
        let mut pages: Vec<(i64, Vec<f32>)> = Vec::new();
        let mut evict = 0;
        // A long effect history can walk more sequential pages than the output block's quota.
        // This allowance belongs to this layer only. Large jumps still use the shared quota.
        let mut history_pages = pre_roll.div_ceil(REMAP_PAGE as usize);
        let mut previous_page: Option<i64> = None;
        if !self.budget.charge(b - a, self.footage) {
            return;
        }
        for o in a..b {
            if (o - a).is_multiple_of(CHECK_INTERVAL) && !self.budget.ok(self.footage) {
                return;
            }
            let k = o - a + fade + 1;
            let gain = (since[k].min(until[k]) as f32 / fade as f32).min(1.0);
            if gain <= 0.0 {
                continue;
            }
            let Some(source) = times.get(k) else { continue };
            let numerator = i128::from(source.0) * i128::from(self.rate);
            let denominator = i128::from(TICKS_PER_SECOND);
            let Ok(sample) = i64::try_from(numerator.div_euclid(denominator)) else { continue };
            let Some(page) = sample.div_euclid(REMAP_PAGE).checked_mul(REMAP_PAGE) else { continue };
            let adjacent = previous_page.replace(page).is_some_and(|previous| page.abs_diff(previous) == REMAP_PAGE as u64);
            let slot = match pages.iter().position(|(p, _)| *p == page) {
                Some(slot) => slot,
                None => {
                    if adjacent && history_pages > 0 && self.budget.control.max_work.is_none() {
                        history_pages -= 1;
                    } else if !self.budget.take_page() {
                        return;
                    }
                    let Some(buf) = self.remap_page(l, page) else { return };
                    // Four pages (about 128 KiB) keep repeated jumps cheap.
                    if pages.len() < 4 {
                        pages.push((page, buf));
                        pages.len() - 1
                    } else {
                        pages[evict] = (page, buf);
                        let slot = evict;
                        evict = (evict + 1) % 4;
                        slot
                    }
                }
            };
            let Some((_, buf)) = pages.get(slot) else { return };
            let j = sample.rem_euclid(REMAP_PAGE) as usize * 2;
            let fraction = (numerator.rem_euclid(denominator) as f64 / denominator as f64) as f32;
            let Some(pair) = out.get_mut(o * 2..o * 2 + 2) else { return };
            for (c, v) in pair.iter_mut().enumerate() {
                let x = buf.get(j + c).copied().unwrap_or(0.0);
                let y = buf.get(j + 2 + c).copied().unwrap_or(0.0);
                *v = (x + (y - x) * fraction) * gain;
            }
        }
    }

    /// The layer's processed audio (effects applied, levels not) for comp sample indices
    /// `i0 .. i0 + n`.
    fn layer_audio(&self, l: &Layer, i0: i64, n: usize) -> Vec<f32> {
        let t = sample_tick(i0, self.rate);
        let mut fx: Vec<(&str, &effectcraft_project::PropGroup, Params)> = Vec::new();
        for (id, g) in audio_effect_groups(l) {
            let params = self.params(l, g, t);
            if !self.budget.ok(self.footage) {
                return Vec::new();
            }
            fx.push((id, g, params));
        }
        if fx.is_empty() {
            return self.layer_raw(l, i0, n, false, 0);
        }
        let reversed = plays_backwards(l);
        let pre_s: f64 = fx.iter().map(|(id, _, p)| audio_fx::preroll(id, p)).sum::<f64>().clamp(0.0, MAX_PREROLL_SECONDS);
        let pre = (pre_s * self.rate as f64).ceil() as usize;
        // The pre-roll's own source reads are not the block's cost: they are allowed on top of
        // what the block may use, at the speed the layer plays (never above MAX_SPEED).
        let speed = (100.0 / l.effective_stretch()).abs();
        let speed = if speed.is_finite() { speed.min(MAX_SPEED).ceil() as usize } else { 1 };
        self.budget.grant(pre.saturating_mul(speed.max(1)));
        let Some(total) = n.checked_add(pre) else {
            self.budget.fail(AudioError::WorkLimit);
            return Vec::new();
        };
        let b0 = i0 - pre as i64;
        let mut buf = self.layer_raw(l, b0, total, reversed, pre);
        if buf.len() < pre * 2 || !self.budget.ok(self.footage) {
            return Vec::new();
        }
        let start = fx_clock(l, sample_tick(b0, self.rate));
        for (id, g, p) in &fx {
            if !self.budget.ok(self.footage) {
                return Vec::new();
            }
            if audio_fx::per_sample(id) && animated(g) {
                self.process_stepped(l, id, g, &mut buf, b0);
            } else {
                audio_fx::process(id, p, &mut buf, self.rate, start);
            }
        }
        buf.drain(..pre * 2);
        buf
    }

    /// Run per-sample effect `id` over `buf` (comp samples from `b0`) in pieces of the parameter
    /// grid, each with the parameters at its first sample.
    fn process_stepped(&self, l: &Layer, id: &str, g: &effectcraft_project::PropGroup, buf: &mut [f32], b0: i64) {
        let step = param_step(self.rate);
        let frames = buf.len() / 2;
        let mut k = 0usize;
        while k < frames {
            if !self.budget.ok(self.footage) {
                return;
            }
            let i = b0.saturating_add(k as i64);
            let next = i.div_euclid(step).saturating_add(1).saturating_mul(step);
            let end = usize::try_from(next.saturating_sub(b0)).unwrap_or(frames).clamp(k + 1, frames);
            let t = sample_tick(i, self.rate);
            if let Some(piece) = buf.get_mut(k * 2..end * 2) {
                audio_fx::process(id, &self.params(l, g, t), piece, self.rate, fx_clock(l, t));
            }
            k = end;
        }
    }

    /// Linear gains (left, right) of the layer's Audio Levels for comp samples `i0 .. i0 + n`.
    /// Animated levels are evaluated on the parameter grid and interpolated in dB in between.
    fn level_gains(&self, l: &Layer, i0: i64, n: usize) -> Vec<[f32; 2]> {
        let Some(pr) = l.props.sub("audio").and_then(|g| g.get("levels")) else { return vec![[1.0, 1.0]; n] };
        let db = |i: i64| self.value(l, pr, sample_tick(i, self.rate)).as_vec2();
        let gain = |[a, b]: [f64; 2]| [db_to_gain(a), db_to_gain(b)];
        if !pr.is_animated() && !pr.has_expression() {
            return vec![gain(db(i0)); n];
        }
        let step = param_step(self.rate);
        let end = i0.saturating_add(n as i64);
        let mut out = Vec::with_capacity(n);
        let mut k = i0.div_euclid(step).saturating_mul(step);
        let mut lo = db(k);
        while k < end {
            if !self.budget.ok(self.footage) {
                return Vec::new();
            }
            let hi = db(k.saturating_add(step));
            for i in k.max(i0)..k.saturating_add(step).min(end) {
                let f = (i - k) as f64 / step as f64;
                out.push(gain([lo[0] + (hi[0] - lo[0]) * f, lo[1] + (hi[1] - lo[1]) * f]));
            }
            k = k.saturating_add(step);
            lo = hi;
        }
        out
    }
}

/// Shared inputs for composition audio mixing, including nested compositions.
struct AudioSources<'a> {
    project: &'a Project,
    footage: &'a dyn FootageSource,
    expr: Option<&'a dyn ExprHost>,
    rate: u32,
    /// Shared by the mix's layers and every nested composition it reads.
    budget: &'a Budget<'a>,
}

fn mix_into(sources: &AudioSources<'_>, comp_id: ItemId, start: Tick, out: &mut [f32], depth: usize) {
    let AudioSources { project, footage, expr, rate, budget } = *sources;
    let Some(comp) = project.comp(comp_id) else { return };
    if depth >= MAX_DEPTH || rate == 0 {
        return;
    }
    let env = MixEnv { project, footage, expr, comp_id, comp, rate, depth, budget };
    let frames = out.len() / 2;
    let any_solo = comp.layers.iter().any(|l| l.switches.solo && audible(project, l));
    let s0 = start.to_units_floor(rate as i64);
    for l in &comp.layers {
        if !budget.ok(footage) {
            return;
        }
        // The stretch the layer's time uses (#3247): 0 and NaN play at 100 %, as `layer_time`;
        // a negative one plays backwards (#3239).
        if !audible(project, l) || (any_solo && !l.switches.solo) {
            continue;
        }
        // Sample range of the block that falls inside the layer's [in, out).
        let a = (units_ceil(l.in_point, rate) - s0).clamp(0, frames as i64) as usize;
        let b = (units_ceil(l.out_point, rate) - s0).clamp(0, frames as i64) as usize;
        if a >= b {
            continue;
        }
        let gains = env.level_gains(l, s0 + a as i64, b - a);
        if gains.iter().all(|g| *g == [0.0, 0.0]) {
            continue;
        }
        let buf = env.layer_audio(l, s0 + a as i64, b - a);
        for (i, (f, g)) in buf.as_chunks::<2>().0.iter().zip(&gains).enumerate() {
            if i.is_multiple_of(CHECK_INTERVAL) && !budget.ok(footage) {
                return;
            }
            out[(a + i) * 2] += f[0] * g[0];
            out[(a + i) * 2 + 1] += f[1] * g[1];
        }
    }
}

/// A min/max peak summary of interleaved stereo audio for waveform drawing: one entry per
/// `bin` sample frames, `[min L, max L, min R, max R]`.
pub fn peak_summary(samples: &[f32], bin: usize) -> Vec<[f32; 4]> {
    let bin = bin.max(1);
    samples
        .chunks(bin * 2)
        .map(|c| {
            let mut s = [f32::MAX, f32::MIN, f32::MAX, f32::MIN];
            for f in c.as_chunks::<2>().0 {
                s[0] = s[0].min(f[0]);
                s[1] = s[1].max(f[0]);
                s[2] = s[2].min(f[1]);
                s[3] = s[3].max(f[1]);
            }
            if s[0] > s[1] { [0.0; 4] } else { s }
        })
        .collect()
}

/// Peak summary of a footage item's audio from source time 0 over `duration`, `bins_per_sec`
/// entries per second (rendered at a low internal rate, fetched in one-second chunks).
pub fn footage_peaks(footage: &dyn FootageSource, item: ItemId, f: &effectcraft_project::Footage, duration: Tick, bins_per_sec: u32) -> Vec<[f32; 4]> {
    let bins_per_sec = bins_per_sec.max(1);
    // Enough samples per bin to catch peaks of audible content, without decoding at full rate.
    let bin = (64usize).max(4000 / bins_per_sec as usize);
    let rate = bin as u32 * bins_per_sec;
    let chunk = bin * bins_per_sec as usize;
    // The stated duration comes from the project: at most a day is summarised, and the summary
    // ends where the audio does.
    let total = units_ceil(duration, rate).clamp(0, MAX_PEAK_SECONDS.saturating_mul(rate as i64)) as usize;
    let mut out = Vec::with_capacity(total / bin + 1);
    let mut i = 0usize;
    while i < total {
        let n = chunk.min(total - i);
        let Some(buf) = footage.audio(item, f, sample_tick(i as i64, rate), n, rate) else { break };
        out.extend(peak_summary(&buf, bin));
        i += n;
    }
    out
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use effectcraft_project::{Footage, FootageKind, build};
    use effectcraft_time::FrameRate;

    use super::*;
    use crate::Image;

    /// A 1 kHz-ish constant "tone": every sample is 0.25 (left) / -0.25 (right).
    struct Dc;
    impl FootageSource for Dc {
        fn frame(&self, _: ItemId, _: &Footage, _: Tick) -> Option<Arc<Image>> {
            None
        }
        fn audio(&self, _: ItemId, _: &Footage, start: Tick, frames: usize, _: u32) -> Option<Vec<f32>> {
            Some((0..frames).flat_map(|_| if start >= Tick::ZERO { [0.25, -0.25] } else { [0.0, 0.0] }).collect())
        }
    }

    #[test]
    fn audio_detection_answers_each_comp_once() {
        // Twenty levels, each comp holding two layers of the one below: 2^20 walks without a
        // memo, and all silent. A cycle is answered too.
        let mut p = Project::default();
        let mk = |p: &mut Project, layers: Vec<Layer>| {
            let mut c = Comp::new(8, 8, FrameRate::new(25, 1), Tick::from_seconds_f64(1.0));
            c.layers = layers.into();
            p.add_item("C", effectcraft_color::Label::Sandstone, None, ItemKind::Comp(c.into()))
        };
        let base = Comp::new(8, 8, FrameRate::new(25, 1), Tick::from_seconds_f64(1.0));
        let mut below = mk(&mut p, vec![]);
        for _ in 0..20 {
            let a = build::layer(&mut p, &base, "a", LayerSource::Comp { item: below }, (0, 0), None);
            let b = build::layer(&mut p, &base, "b", LayerSource::Comp { item: below }, (0, 0), None);
            below = mk(&mut p, vec![a, b]);
        }
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || tx.send(comp_has_audio(&p, below)).ok());
        assert!(!rx.recv_timeout(std::time::Duration::from_secs(20)).expect("detection finishes"));
    }

    #[test]
    fn audio_found_by_a_short_route_is_not_hidden_by_a_deep_one() {
        let mut p = Project::default();
        let base = Comp::new(8, 8, FrameRate::new(25, 1), Tick::from_seconds_f64(1.0));
        let mk = |p: &mut Project, layers: Vec<Layer>| {
            let mut c = base.clone();
            c.layers = layers.into();
            p.add_item("C", effectcraft_color::Label::Sandstone, None, ItemKind::Comp(c.into()))
        };
        let f = Footage { kind: FootageKind::Audio, has_audio: true, ..Default::default() };
        let fid = p.add_item("x.wav", effectcraft_color::Label::SeaFoam, None, ItemKind::Footage(f));
        let sound = build::layer(&mut p, &base, "s", LayerSource::Footage { item: fid }, (0, 0), None);
        let audible = mk(&mut p, vec![sound]);
        // A chain of MAX_DEPTH comps ends in `audible` one level past what is searched; the root
        // reaches `audible` directly as well, after the chain.
        let mut chain = audible;
        for _ in 0..MAX_DEPTH {
            let l = build::layer(&mut p, &base, "c", LayerSource::Comp { item: chain }, (0, 0), None);
            chain = mk(&mut p, vec![l]);
        }
        assert!(!comp_has_audio(&p, chain), "too deep to be found");
        let via_chain = build::layer(&mut p, &base, "chain", LayerSource::Comp { item: chain }, (0, 0), None);
        let direct = build::layer(&mut p, &base, "direct", LayerSource::Comp { item: audible }, (0, 0), None);
        let root = mk(&mut p, vec![via_chain, direct]);
        assert!(comp_has_audio(&p, root));
    }

    #[test]
    fn mixes_layers_with_levels_and_in_out() {
        let mut p = Project::default();
        let f = Footage {
            path: "x.wav".into(),
            kind: FootageKind::Audio,
            width: 0,
            height: 0,
            pixel_aspect: 1.0,
            frame_rate: FrameRate::new(25, 1),
            native_rate: None,
            duration: Tick::from_seconds_f64(10.0),
            has_video: false,
            has_audio: true,
            alpha: Default::default(),
            premul_color: [0.0; 3],
            loop_count: 1,
            codec: String::new(),
            missing: false,
            sequence: vec![],
            color_profile: None,
            ..Default::default()
        };
        let fid = p.add_item("x.wav", effectcraft_color::Label::SeaFoam, None, ItemKind::Footage(f));
        let comp = Comp::new(64, 64, FrameRate::new(25, 1), Tick::from_seconds_f64(2.0));
        let mut l1 = build::layer(&mut p, &comp, "a", LayerSource::Footage { item: fid }, (0, 0), None);
        l1.in_point = Tick::from_seconds_f64(1.0);
        let mut l2 = build::layer(&mut p, &comp, "b", LayerSource::Footage { item: fid }, (0, 0), None);
        if let Some(g) = l2.props.sub_mut("audio")
            && let Some(pr) = g.get_mut("levels")
        {
            pr.value = effectcraft_project::Value::Vec2([-6.0206, -96.0]);
        }
        let mut comp = comp;
        comp.layers = vec![l1, l2].into();
        let cid = p.add_item("C", effectcraft_color::Label::Sandstone, None, ItemKind::Comp(comp.into()));
        assert!(comp_has_audio(&p, cid));
        let rate = 1000;
        let m = mix_comp(&p, &Dc, None, cid, Tick::from_seconds_f64(0.5), 1000, rate).unwrap();
        // first half: only layer b (half gain left, muted right)
        assert!((m[0] - 0.125).abs() < 1e-3 && m[1].abs() < 1e-6, "{} {}", m[0], m[1]);
        // second half (≥ 1 s): a + b
        let i = 700 * 2;
        assert!((m[i] - 0.375).abs() < 1e-3 && (m[i + 1] + 0.25).abs() < 1e-3, "{} {}", m[i], m[i + 1]);
    }

    /// A 440 Hz tone, so a change in the mix shows in the samples.
    struct Tone;
    impl FootageSource for Tone {
        fn frame(&self, _: ItemId, _: &Footage, _: Tick) -> Option<Arc<Image>> {
            None
        }
        fn audio(&self, _: ItemId, _: &Footage, start: Tick, frames: usize, rate: u32) -> Option<Vec<f32>> {
            let t0 = start.seconds();
            Some(
                (0..frames)
                    .flat_map(|k| {
                        let v = (std::f64::consts::TAU * 440.0 * (t0 + k as f64 / rate as f64)).sin() as f32 * 0.5;
                        [v, v]
                    })
                    .collect(),
            )
        }
    }

    #[test]
    fn a_plain_precomp_mixes_the_same_audio_as_its_layers() {
        let mut p = Project::default();
        let f = Footage {
            path: "tone.wav".into(),
            kind: FootageKind::Audio,
            has_audio: true,
            frame_rate: FrameRate::new(25, 1),
            duration: Tick::from_seconds_f64(10.0),
            ..Default::default()
        };
        let fid = p.add_item("tone.wav", effectcraft_color::Label::SeaFoam, None, ItemKind::Footage(f));
        let base = Comp::new(64, 64, FrameRate::new(25, 1), Tick::from_seconds_f64(2.0));
        let tone = build::layer(&mut p, &base, "tone", LayerSource::Footage { item: fid }, (0, 0), None);
        let mut inner = base.clone();
        inner.layers = vec![tone].into();
        let inner_id = p.add_item("Inner", effectcraft_color::Label::Sandstone, None, ItemKind::Comp(inner.into()));
        let pre = build::layer(&mut p, &base, "pre", LayerSource::Comp { item: inner_id }, (64, 64), None);
        let mut outer = base.clone();
        outer.layers = vec![pre].into();
        let outer_id = p.add_item("Outer", effectcraft_color::Label::Sandstone, None, ItemKind::Comp(outer.into()));
        let rate = 8000;
        let t = Tick::from_seconds_f64(0.5);
        let direct = mix_comp(&p, &Tone, None, inner_id, t, 4000, rate).unwrap();
        let nested = mix_comp(&p, &Tone, None, outer_id, t, 4000, rate).unwrap();
        assert_eq!(direct.len(), nested.len());
        let worst = direct.iter().zip(&nested).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
        assert!(worst < 1e-6, "a plain precomp changes its audio by {worst}");
    }

    /// The error of a tone mixed at `stretch` percent against the exact tone, at `rate` Hz.
    fn stretch_error(stretch: f64, rate: u32) -> (f64, f64) {
        let mut p = Project::default();
        let f = Footage {
            path: "tone.wav".into(),
            kind: FootageKind::Audio,
            has_audio: true,
            frame_rate: FrameRate::new(25, 1),
            duration: Tick::from_seconds_f64(10.0),
            ..Default::default()
        };
        let fid = p.add_item("tone.wav", effectcraft_color::Label::SeaFoam, None, ItemKind::Footage(f));
        let base = Comp::new(64, 64, FrameRate::new(25, 1), Tick::from_seconds_f64(2.0));
        let mut tone = build::layer(&mut p, &base, "tone", LayerSource::Footage { item: fid }, (0, 0), None);
        tone.stretch = stretch;
        // A reversed layer as After Effects lays it out: over [0, 2) s, starting at its out point.
        let end = if stretch < 0.0 { 2.0 } else { 0.0 };
        if stretch < 0.0 {
            (tone.in_point, tone.out_point, tone.start_time) = (Tick::ZERO, Tick::from_seconds_f64(2.0), Tick::from_seconds_f64(2.0));
        }
        let mut comp = base.clone();
        comp.layers = vec![tone].into();
        let cid = p.add_item("C", effectcraft_color::Label::Sandstone, None, ItemKind::Comp(comp.into()));
        let n = 2000usize;
        let t = Tick::from_seconds_f64(0.5);
        let m = mix_comp(&p, &Tone, None, cid, t, n, rate).unwrap();
        let speed = 100.0 / stretch;
        let (mut worst, mut sq) = (0.0f64, 0.0f64);
        for i in 0..n {
            // The source plays `speed` times as fast as the comp: source time = speed * (comp time −
            // start). Reversed, a comp sample plays the source sample just before that time.
            let back = if stretch < 0.0 { speed / rate as f64 } else { 0.0 };
            let src = speed * (t.seconds() + i as f64 / rate as f64 - end) + back;
            let ideal = (std::f64::consts::TAU * 440.0 * src).sin() * 0.5;
            let e = (m[i * 2] as f64 - ideal).abs();
            worst = worst.max(e);
            sq += e * e;
        }
        (worst, (sq / n as f64).sqrt())
    }

    #[test]
    fn stretched_layers_resample_a_tone_with_small_error() {
        // Blending the two samples around each time: about 0.0002 at 48 kHz and 0.007 at 8 kHz. Copying
        // the nearest sample gave 0.025 and 0.15.
        for stretch in [70.0, 33.0] {
            let (worst, _) = stretch_error(stretch, 48_000);
            assert!(worst < 1e-3, "stretch {stretch}%: error {worst} at 48 kHz");
            let (worst, _) = stretch_error(stretch, 8_000);
            assert!(worst < 1e-2, "stretch {stretch}%: error {worst} at 8 kHz");
        }
    }

    /// A reversed layer plays its tone backwards with the same resampling quality as forwards
    /// (it was silent, #3239); at -100 % the samples are copied exactly.
    #[test]
    fn reversed_layers_resample_a_tone_with_small_error() {
        for stretch in [-70.0, -33.0] {
            let (worst, _) = stretch_error(stretch, 48_000);
            assert!(worst < 1e-3, "stretch {stretch}%: error {worst} at 48 kHz");
            let (worst, _) = stretch_error(stretch, 8_000);
            assert!(worst < 1e-2, "stretch {stretch}%: error {worst} at 8 kHz");
        }
        let (worst, _) = stretch_error(-100.0, 48_000);
        assert!(worst < 1e-5, "stretch -100%: error {worst}");
    }
}
