//! Audio mixdown of a composition: every audible layer (footage with audio, precomps, and any
//! layer with a sound-generating effect such as Tone) summed with its Audio Levels (dB, per
//! channel), respecting in/out points, the Audio switch and solo. Layer audio effects
//! (Effect > Audio, see [`effectcraft_effects::audio_fx`]) run on each layer's audio before
//! its levels; Backwards plays the layer's [in, out) span reversed.
//!
//! Callers mix in short blocks (one video frame, or an audio device buffer). Effects and levels
//! are evaluated once per block. To make the result independent of block boundaries, layer
//! effects are run from a cleared state over a pre-roll of earlier audio and only the block's
//! tail is kept — so preview playback and export produce identical samples.
//!
//! Time-stretched and time-remapped audio is linearly resampled. Time Remap uses the same
//! source-time evaluation as video, at audio sample resolution; it does not preserve pitch.
//! Reversed (negative stretch) layers are silent for now.

use effectcraft_effects::Params;
use effectcraft_effects::audio_fx;
use effectcraft_project::{Comp, GroupKind, ItemId, ItemKind, Layer, LayerSource, Node, Project};
use effectcraft_time::{TICKS_PER_SECOND, Tick};

use crate::{EvalCtx, ExprHost, FootageSource};

const MAX_DEPTH: usize = 16;

/// `frames` stereo sample frames of `comp`'s mix from comp time `start` at `rate` Hz,
/// interleaved (L R L R …). Returns an error rather than partial audio when work is excessive.
pub fn mix_comp(
    project: &Project,
    footage: &dyn FootageSource,
    expr: Option<&dyn ExprHost>,
    comp: ItemId,
    start: Tick,
    frames: usize,
    rate: u32,
) -> Result<Vec<f32>, AudioError> {
    mix_comp_with_control(project, footage, expr, comp, start, frames, rate, &MixControl::default())
}

/// Failure aborts the entire block; callers must not play or export a partial mix.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AudioError {
    Cancelled,
    WorkLimit,
}
impl std::fmt::Display for AudioError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Cancelled => "audio mixing cancelled",
            Self::WorkLimit => "audio mixing exceeded its work limit; simplify time stretching, remapping, or nested compositions",
        })
    }
}
impl std::error::Error for AudioError {}

/// Controls shared by every layer, source read and nested composition in one mix call.
#[derive(Default)]
pub struct MixControl {
    /// Stop an in-flight mix, including expression evaluation when supported by the host.
    pub cancel: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    /// Override the default allowance of 1024 source/sample operations per output frame plus
    /// 65536 for page overhead. Individual allocations are also capped at 1048576 stereo frames.
    pub max_work: Option<usize>,
    /// Native wall-clock allowance for the whole mix (five seconds by default).
    pub max_duration: Option<std::time::Duration>,
    /// Shared JavaScript yield allowance (1024 yields of up to 20,000 instructions by default).
    pub max_expression_yields: Option<usize>,
}

/// Shared interruption state for every expression in one mix, including nested evaluations.
/// Clones share the sticky resource failure and yield allowance; a new sample cannot reset them.
#[derive(Clone)]
pub struct AudioEvalControl {
    cancel: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    remaining: std::rc::Rc<std::cell::Cell<usize>>,
    failed: std::rc::Rc<std::cell::Cell<bool>>,
    #[cfg(not(target_arch = "wasm32"))]
    deadline: Option<std::time::Instant>,
}
impl AudioEvalControl {
    /// Capture the deadline once, before any sample or nested mix starts.
    fn new(control: &MixControl) -> Self {
        Self {
            cancel: control.cancel.clone(),
            remaining: std::rc::Rc::new(std::cell::Cell::new(control.max_expression_yields.unwrap_or(1024))),
            failed: std::rc::Rc::new(std::cell::Cell::new(false)),
            #[cfg(not(target_arch = "wasm32"))]
            deadline: std::time::Instant::now().checked_add(control.max_duration.unwrap_or(std::time::Duration::from_secs(5))),
        }
    }
    /// Mark a runtime resource limit; ordinary expression errors still use keyframed fallback.
    pub fn fail(&self) {
        self.failed.set(true);
    }
    /// Whether the shared resource allowance has expired (also used after script completion).
    pub fn exhausted(&self) -> bool {
        #[cfg(not(target_arch = "wasm32"))]
        if self.deadline.is_some_and(|d| std::time::Instant::now() >= d) {
            self.fail();
        }
        self.failed.get()
    }
    /// Check cancellation and resource limits without charging another instruction chunk.
    pub fn interrupted(&self) -> bool {
        self.cancel.as_ref().is_some_and(|c| c.load(std::sync::atomic::Ordering::Relaxed)) || self.exhausted()
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
}

struct Budget<'a> {
    control: &'a MixControl,
    expressions: AudioEvalControl,
    remaining: std::cell::Cell<usize>,
    error: std::cell::Cell<Option<AudioError>>,
}
impl Budget<'_> {
    /// Charge before allocating, decoding or evaluating; a failure is sticky for the whole mix.
    fn charge(&self, frames: usize, footage: &dyn FootageSource) -> bool {
        if self.error.get().is_some() {
            return false;
        }
        if footage.called_off() || self.control.cancel.as_ref().is_some_and(|c| c.load(std::sync::atomic::Ordering::Relaxed)) {
            self.error.set(Some(AudioError::Cancelled));
            return false;
        }
        if self.expressions.exhausted() || frames > 1_048_576 || frames > self.remaining.get() {
            self.error.set(Some(AudioError::WorkLimit));
            return false;
        }
        self.remaining.set(self.remaining.get() - frames);
        true
    }
}

/// Mix with cancellation and a cumulative budget; failures discard the entire output block.
#[allow(clippy::too_many_arguments)]
pub fn mix_comp_with_control(
    project: &Project,
    footage: &dyn FootageSource,
    expr: Option<&dyn ExprHost>,
    comp: ItemId,
    start: Tick,
    frames: usize,
    rate: u32,
    control: &MixControl,
) -> Result<Vec<f32>, AudioError> {
    let cancellable = control.cancel.as_ref().map(|cancel| crate::CancellableFootage { inner: footage, cancel: cancel.clone() });
    let footage: &dyn FootageSource = cancellable.as_ref().map_or(footage, |source| source);
    let budget = Budget {
        control,
        expressions: AudioEvalControl::new(control),
        remaining: std::cell::Cell::new(control.max_work.unwrap_or(frames.saturating_mul(1024).saturating_add(65536))),
        error: std::cell::Cell::new(None),
    };
    if !budget.charge(frames, footage) {
        return Err(budget.error.get().unwrap_or(AudioError::WorkLimit));
    }
    let mut out = vec![0.0f32; frames * 2];
    mix_into(project, footage, expr, comp, start, rate, &mut out, 1.0, 1.0, 0, &budget);
    budget.charge(0, footage);
    match budget.error.get() {
        Some(e) => Err(e),
        None => Ok(out),
    }
}

/// Whether the comp has anything audible (footage with audio, a sound generator, or a precomp
/// that has).
pub fn comp_has_audio(project: &Project, comp: ItemId) -> bool {
    fn walk(project: &Project, comp: ItemId, depth: usize) -> bool {
        let Some(c) = project.comp(comp) else { return false };
        depth < MAX_DEPTH
            && c.layers.iter().any(|l| {
                l.switches.audio
                    && (generates(l)
                        || match &l.source {
                            LayerSource::Footage { item } => {
                                matches!(project.item(*item).map(|i| &i.kind), Some(ItemKind::Footage(f)) if f.has_audio && !f.missing)
                            }
                            LayerSource::Comp { item } => walk(project, *item, depth + 1),
                            _ => false,
                        })
            })
    }
    walk(project, comp, 0)
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

/// First sample index at or after a layer boundary.
fn units_ceil(t: Tick, rate: u32) -> i64 {
    let n = t.0 as i128 * rate as i128;
    let d = TICKS_PER_SECOND as i128;
    (n.div_euclid(d) + i128::from(n.rem_euclid(d) != 0)) as i64
}

/// Represent a sample without rounding it back into the preceding sample.
fn sample_tick(i: i64, rate: u32) -> Tick {
    // Keep the requested sample when the rate does not divide the tick clock.
    let t = Tick::from_units(i, i64::from(rate));
    if t.to_units_floor(i64::from(rate)) < i { Tick(t.0.saturating_add(1)) } else { t }
}

struct MixEnv<'a> {
    budget: &'a Budget<'a>,
    project: &'a Project,
    footage: &'a dyn FootageSource,
    expr: Option<&'a dyn ExprHost>,
    comp_id: ItemId,
    comp: &'a Comp,
    rate: u32,
    depth: usize,
}

impl MixEnv<'_> {
    /// Evaluation context at one composition sample.
    fn ctx(&self, t: Tick) -> EvalCtx<'_> {
        let mut ctx = EvalCtx::new(self.project, self.comp_id, self.comp, t);
        ctx.expr = self.expr;
        ctx
    }

    /// The layer's own (pre-effect) audio for comp sample indices `i0 .. i0 + n`, silent
    /// outside its [in, out) span. Comp sample index `k` maps to `in + out − 1 − k` when
    /// `reversed`.
    fn layer_raw(&self, l: &Layer, i0: i64, n: usize, reversed: bool) -> Vec<f32> {
        if !self.budget.charge(n, self.footage) {
            return Vec::new();
        }
        let rate = self.rate;
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
            let fwd = self.layer_raw(l, ma, (mb - ma) as usize, false);
            if !self.budget.charge(0, self.footage) {
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
        if l.props.get("timeRemap").is_some() {
            self.remapped_raw(l, i0, a, b, &mut out);
            return out;
        }
        let m = b - a;
        let speed = 100.0 / l.stretch;
        let src_n = ((m as f64 * speed).ceil() as usize).max(1);
        if src_n > 1_048_576 {
            // Paging removes the allocation limit, not the cumulative source-work allowance.
            if src_n > self.budget.remaining.get() {
                self.budget.error.set(Some(AudioError::WorkLimit));
                return out;
            }
            self.large_stretched_raw(l, i0, a, b, speed, src_n, &mut out);
            return out;
        }
        if !self.budget.charge(src_n, self.footage) {
            return out;
        }
        let lt = l.layer_time(sample_tick(i0 + a as i64, rate));
        let buf = match &l.source {
            LayerSource::Footage { item } => match self.project.item(*item).map(|i| &i.kind) {
                Some(ItemKind::Footage(f)) if f.has_audio => self.footage.audio(*item, f, lt, src_n, rate).unwrap_or_else(|| vec![0.0; src_n * 2]),
                _ => return out,
            },
            LayerSource::Comp { item } => {
                if self.project.comp_contains(*item, self.comp_id) {
                    return out;
                }
                let mut sub = vec![0.0f32; src_n * 2];
                mix_into(self.project, self.footage, self.expr, *item, lt, rate, &mut sub, 1.0, 1.0, self.depth + 1, self.budget);
                sub
            }
            _ => return out,
        };
        let same = (speed - 1.0).abs() < 1e-9;
        for i in 0..m {
            // The source sample at fractional index `x`, blended linearly with the next one unless the
            // layer plays at its own speed (then the samples are copied as they are).
            let x = i as f64 * speed;
            let j = if same { i } else { (x as usize).min(src_n - 1) };
            if j * 2 + 1 >= buf.len() {
                break;
            }
            let k = if same || (j + 1) * 2 + 1 >= buf.len() { j } else { j + 1 };
            let f = if same { 0.0 } else { ((x - j as f64) as f32).clamp(0.0, 1.0) };
            out[(a + i) * 2] = buf[j * 2] + (buf[k * 2] - buf[j * 2]) * f;
            out[(a + i) * 2 + 1] = buf[j * 2 + 1] + (buf[k * 2 + 1] - buf[j * 2 + 1]) * f;
        }
        out
    }

    /// Stream oversized stretched reads without allocating their whole source span. Each page
    /// includes the next sample for interpolation; nested mixes share the same work budget.
    #[allow(clippy::too_many_arguments)]
    fn large_stretched_raw(&self, l: &Layer, i0: i64, a: usize, b: usize, speed: f64, src_n: usize, out: &mut [f32]) {
        const PAGE: usize = 65536;
        let first = l.layer_time(sample_tick(i0 + a as i64, self.rate)).to_units_floor(i64::from(self.rate));
        let mut cached = None;
        let mut buf = Vec::new();
        for i in 0..b - a {
            if !self.budget.charge(0, self.footage) {
                return;
            }
            let x = i as f64 * speed;
            let j = (x as usize).min(src_n - 1);
            let page = j / PAGE * PAGE;
            if cached != Some(page) {
                let frames = (src_n - page).min(PAGE + 1);
                if !self.budget.charge(frames, self.footage) {
                    return;
                }
                let Some(index) = i64::try_from(page).ok().and_then(|p| first.checked_add(p)) else {
                    self.budget.error.set(Some(AudioError::WorkLimit));
                    return;
                };
                let start = sample_tick(index, self.rate);
                buf = match &l.source {
                    LayerSource::Footage { item } => match self.project.item(*item).map(|i| &i.kind) {
                        Some(ItemKind::Footage(f)) if f.has_audio => self.footage.audio(*item, f, start, frames, self.rate).unwrap_or_default(),
                        _ => return,
                    },
                    LayerSource::Comp { item } => {
                        if self.project.comp_contains(*item, self.comp_id) {
                            return;
                        }
                        let mut sub = vec![0.0; frames * 2];
                        mix_into(self.project, self.footage, self.expr, *item, start, self.rate, &mut sub, 1.0, 1.0, self.depth + 1, self.budget);
                        sub
                    }
                    _ => return,
                };
                cached = Some(page);
            }
            let offset = (j - page) * 2;
            let Some(pair) = buf.get(offset..offset + 2) else { return };
            let next = buf.get(offset + 2..offset + 4).unwrap_or(pair);
            let fraction = ((x - j as f64) as f32).clamp(0.0, 1.0);
            let Some(target) = out.get_mut((a + i) * 2..(a + i) * 2 + 2) else { return };
            for channel in 0..2 {
                target[channel] = pair[channel] + (next[channel] - pair[channel]) * fraction;
            }
        }
    }

    /// Fetch bounded source pages, rather than decoding once per output sample or allocating
    /// the entire span between arbitrarily distant remap keys. Page alignment is independent
    /// of the caller's buffer size, including the interpolation sample at each page boundary.
    fn remapped_raw(&self, l: &Layer, i0: i64, a: usize, b: usize, out: &mut [f32]) {
        const PAGE: i64 = 4096;
        let Some(prop) = l.props.get("timeRemap") else { return };
        let prepared = prop.expr.as_ref().filter(|_| prop.has_expression()).and_then(|e| self.expr.and_then(|h| h.prepare_audio(&e.text)));
        let mut pages: Vec<(i64, Vec<f32>)> = Vec::new();
        let mut next_page = 0;

        for i in a..b {
            if !self.budget.charge(1, self.footage) {
                return;
            }
            let Ok(offset) = i64::try_from(i) else { break };
            let Some(index) = i0.checked_add(offset) else { break };
            let time = sample_tick(index, self.rate);
            let source = if let Some(prepared) = &prepared {
                let base = prop.value_at(l.layer_time(time));
                if let effectcraft_keyframe::Value::Scalar(value) = base {
                    Tick::from_seconds_f64(prepared.evaluate(time.seconds(), value).unwrap_or(value))
                } else {
                    let value = self
                        .expr
                        .map(|h| h.eval_audio(&self.ctx(time), l, prop, &base, &self.budget.expressions).unwrap_or_else(|_| base.clone()))
                        .unwrap_or(base);
                    Tick::from_seconds_f64(value.as_f64())
                }
            } else {
                let base = prop.value_at(l.layer_time(time));
                let value = if prop.has_expression() {
                    self.expr.map(|h| h.eval_audio(&self.ctx(time), l, prop, &base, &self.budget.expressions).unwrap_or_else(|_| base.clone())).unwrap_or(base)
                } else {
                    base
                };
                Tick::from_seconds_f64(value.as_f64())
            };
            if !self.budget.charge(0, self.footage) {
                return;
            }
            let numerator = i128::from(source.0) * i128::from(self.rate);
            let denominator = i128::from(TICKS_PER_SECOND);
            let Ok(sample) = i64::try_from(numerator.div_euclid(denominator)) else { continue };
            let Some(page) = sample.div_euclid(PAGE).checked_mul(PAGE) else { continue };
            let slot = if let Some(slot) = pages.iter().position(|(p, _)| *p == page) {
                slot
            } else {
                let start = sample_tick(page, self.rate);
                let frames = PAGE as usize + 1;
                if !self.budget.charge(frames, self.footage) {
                    return;
                }
                let buf = match &l.source {
                    LayerSource::Footage { item } => match self.project.item(*item).map(|i| &i.kind) {
                        Some(ItemKind::Footage(f)) if f.has_audio => self.footage.audio(*item, f, start, frames, self.rate).unwrap_or_default(),
                        _ => return,
                    },
                    LayerSource::Comp { item } => {
                        if self.project.comp_contains(*item, self.comp_id) {
                            return;
                        }
                        let mut sub = vec![0.0; frames * 2];
                        mix_into(self.project, self.footage, self.expr, *item, start, self.rate, &mut sub, 1.0, 1.0, self.depth + 1, self.budget);
                        sub
                    }
                    _ => return,
                };
                // Four pages cap retained stereo samples at about 128 KiB while retaining repeated jumps.
                if pages.len() < 4 {
                    pages.push((page, buf));
                    pages.len() - 1
                } else {
                    let slot = next_page;
                    if let Some(p) = pages.get_mut(slot) {
                        *p = (page, buf);
                    }
                    next_page = (next_page + 1) % 4;
                    slot
                }
            };
            let Some((_, buf)) = pages.get(slot) else { return };
            let j = (sample.rem_euclid(PAGE) as usize) * 2;
            let fraction = (numerator.rem_euclid(denominator) as f64 / denominator as f64) as f32;
            let Some(pair) = out.get_mut(i.saturating_mul(2)..i.saturating_mul(2).saturating_add(2)) else { break };
            for (channel, output) in pair.iter_mut().enumerate() {
                let x = buf.get(j + channel).copied().unwrap_or(0.0);
                let y = buf.get(j + 2 + channel).copied().unwrap_or(0.0);
                *output = x + (y - x) * fraction;
            }
        }
    }

    /// Evaluate any audio property under the same controls as per-sample Time Remap.
    /// The host's scope also covers expressions reached through property references.
    fn value(&self, l: &Layer, prop: &effectcraft_project::Property, t: Tick) -> effectcraft_keyframe::Value {
        let base = prop.value_at(l.layer_time(t));
        if !self.budget.charge(0, self.footage) {
            return base;
        }
        let value = if prop.has_expression() {
            self.expr.map(|h| h.eval_audio(&self.ctx(t), l, prop, &base, &self.budget.expressions).unwrap_or_else(|_| base.clone())).unwrap_or(base)
        } else {
            base
        };
        self.budget.charge(0, self.footage);
        value
    }

    /// Effect parameters at comp time `t`, stopping as soon as shared controls expire.
    fn params(&self, l: &Layer, g: &effectcraft_project::PropGroup, t: Tick) -> Params {
        let mut p = Params::default();
        for c in &g.children {
            if !self.budget.charge(0, self.footage) {
                break;
            }
            if let Node::Prop(pr) = c {
                p.values.insert(pr.match_id.clone(), self.value(l, pr, t));
            }
        }
        p
    }

    /// The layer's processed audio (effects applied, levels not) for comp sample indices
    /// `i0 .. i0 + n`.
    fn layer_audio(&self, l: &Layer, i0: i64, n: usize) -> Vec<f32> {
        let t = sample_tick(i0, self.rate);
        let mut fx = Vec::new();
        for (id, g) in audio_effect_groups(l) {
            if !self.budget.charge(0, self.footage) {
                return Vec::new();
            }
            let params = self.params(l, g, t);
            if !self.budget.charge(0, self.footage) {
                return Vec::new();
            }
            fx.push((id, params));
        }
        if fx.is_empty() {
            return self.layer_raw(l, i0, n, false);
        }
        let reversed = fx.iter().any(|(id, _)| audio_fx::reverses(id));
        let pre_s: f64 = fx.iter().map(|(id, p)| audio_fx::preroll(id, p)).sum();
        let pre = (pre_s * self.rate as f64).ceil() as usize;
        let Some(total) = n.checked_add(pre).filter(|n| *n <= 1_048_576) else {
            self.budget.error.set(Some(AudioError::WorkLimit));
            return Vec::new();
        };
        let Some(first) = i0.checked_sub(pre as i64) else {
            self.budget.error.set(Some(AudioError::WorkLimit));
            return Vec::new();
        };
        let mut buf = self.layer_raw(l, first, total, reversed);
        if !self.budget.charge(0, self.footage) {
            return Vec::new();
        }
        let start = l.layer_time(sample_tick(i0 - pre as i64, self.rate)).seconds();
        for (id, p) in &fx {
            audio_fx::process(id, p, &mut buf, self.rate, start);
        }
        buf.drain(..pre * 2);
        buf
    }
}

/// Accumulate audible layers; nested calls share the root budget and cancellation state.
#[allow(clippy::too_many_arguments)]
fn mix_into(
    project: &Project,
    footage: &dyn FootageSource,
    expr: Option<&dyn ExprHost>,
    comp_id: ItemId,
    start: Tick,
    rate: u32,
    out: &mut [f32],
    gl: f32,
    gr: f32,
    depth: usize,
    budget: &Budget,
) {
    let Some(comp) = project.comp(comp_id) else { return };
    if depth >= MAX_DEPTH {
        budget.error.set(Some(AudioError::WorkLimit));
        return;
    }
    if rate == 0 {
        return;
    }
    let env = MixEnv { project, footage, expr, comp_id, comp, rate, depth, budget };
    let frames = out.len() / 2;
    let any_solo = comp.layers.iter().any(|l| l.switches.solo && audible(project, l));
    let s0 = start.to_units_floor(rate as i64);
    for l in &comp.layers {
        if !budget.charge(0, footage) {
            return;
        }
        if !audible(project, l) || (any_solo && !l.switches.solo) || l.stretch <= 0.0 {
            continue;
        }
        // Sample range of the block that falls inside the layer's [in, out).
        let a = (units_ceil(l.in_point, rate) - s0).clamp(0, frames as i64) as usize;
        let b = (units_ceil(l.out_point, rate) - s0).clamp(0, frames as i64) as usize;
        if a >= b {
            continue;
        }
        let t_a = sample_tick(s0 + a as i64, rate);
        let (ll, lr) = levels(&env, l, t_a);
        if !budget.charge(0, footage) {
            return;
        }
        let (ll, lr) = (ll * gl, lr * gr);
        if ll == 0.0 && lr == 0.0 {
            continue;
        }
        let buf = env.layer_audio(l, s0 + a as i64, b - a);
        for (i, f) in buf.as_chunks::<2>().0.iter().enumerate() {
            out[(a + i) * 2] += f[0] * ll;
            out[(a + i) * 2 + 1] += f[1] * lr;
        }
    }
}

/// Evaluate the layer gains at the start of its audible block.
fn levels(env: &MixEnv, l: &Layer, t: Tick) -> (f32, f32) {
    let Some(g) = l.props.sub("audio") else { return (1.0, 1.0) };
    let [a, b] = g.get("levels").map(|p| env.value(l, p, t).as_vec2()).unwrap_or([0.0, 0.0]);
    (db_to_gain(a), db_to_gain(b))
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
    let total = units_ceil(duration, rate).max(0) as usize;
    let mut out = Vec::with_capacity(total / bin + 1);
    let mut i = 0usize;
    while i < total {
        let n = chunk.min(total - i);
        let buf = footage.audio(item, f, sample_tick(i as i64, rate), n, rate).unwrap_or_else(|| vec![0.0; n * 2]);
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
        comp.layers = vec![l1, l2];
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
        inner.layers = vec![tone];
        let inner_id = p.add_item("Inner", effectcraft_color::Label::Sandstone, None, ItemKind::Comp(inner.into()));
        let pre = build::layer(&mut p, &base, "pre", LayerSource::Comp { item: inner_id }, (64, 64), None);
        let mut outer = base.clone();
        outer.layers = vec![pre];
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
        let mut comp = base.clone();
        comp.layers = vec![tone];
        let cid = p.add_item("C", effectcraft_color::Label::Sandstone, None, ItemKind::Comp(comp.into()));
        let n = 2000usize;
        let t = Tick::from_seconds_f64(0.5);
        let m = mix_comp(&p, &Tone, None, cid, t, n, rate).unwrap();
        let speed = 100.0 / stretch;
        let (mut worst, mut sq) = (0.0f64, 0.0f64);
        for i in 0..n {
            // The source plays `speed` times as fast as the comp: source time = speed * comp time.
            let ideal = (std::f64::consts::TAU * 440.0 * speed * (t.seconds() + i as f64 / rate as f64)).sin() * 0.5;
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
}
