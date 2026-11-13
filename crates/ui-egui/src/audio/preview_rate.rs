//! Preview speed changes composition time, not the output device's sample rate.
//! Keep the clock and the resampler on the same rational output-sample grid.

use effectcraft_engine::time::{FrameRate, TICKS_PER_SECOND, Tick};

use super::PlaySpan;

/// At most this many composition samples are mixed at once. At very high preview speeds,
/// skip samples that no output frame needs instead of allocating the entire source interval.
const SOURCE_PAGE: usize = 4096;

#[derive(Clone, Copy)]
pub(super) struct PreviewRate {
    rate: u32,
    speed: FrameRate,
    origin: Tick,
}

impl PreviewRate {
    pub(super) fn new(rate: u32, speed: f64, start: Tick) -> Result<Self, String> {
        // A billionth keeps the ratio of decimal preview rates and rational comp rates without
        // rounding it to an integer sample rate. Reject unrepresentable values, never mute them.
        let num = (speed * 1_000_000_000.0).round();
        if rate == 0 || !speed.is_finite() || speed <= 0.0 || num < 1.0 || num >= i64::MAX as f64 {
            return Err("The preview playback rate must be finite, positive and representable".into());
        }
        let speed = FrameRate::new(num as i64, 1_000_000_000);
        // Keep the existing 1x sample grid. At another speed, anchor the grid to the exact
        // requested frame rather than rounding a nonzero start backwards by an output sample.
        let origin = if speed.num == speed.den { Tick::ZERO } else { start };
        Ok(Self { rate, speed, origin })
    }

    pub(super) fn rate(self) -> u32 {
        self.rate
    }

    pub(super) fn normal(self) -> bool {
        self.speed.num == self.speed.den
    }

    pub(super) fn time(self, sample: i64) -> Tick {
        if self.normal() {
            // Preserve the legacy grid, including negative samples at unusual device rates.
            return super::sample_time(sample, self.rate);
        }
        self.origin + Tick::from_rational(sample, self.speed.num, i64::from(self.rate) * self.speed.den)
    }

    /// First output sample at or after a composition time. Ceiling keeps loop endpoints inside
    /// the work area. A fractional loop boundary is quantized by less than one output sample.
    fn sample_ceil(self, time: Tick) -> i64 {
        let n = i128::from((time - self.origin).0) * i128::from(self.rate) * i128::from(self.speed.den);
        let d = i128::from(TICKS_PER_SECOND) * i128::from(self.speed.num);
        let sample = n.div_euclid(d) + i128::from(n.rem_euclid(d) != 0);
        sample.clamp(i64::MIN as i128, i64::MAX as i128) as i64
    }

    pub(super) fn span(self, start: Tick, wa: Tick, wb: Tick, looping: bool) -> PlaySpan {
        if self.normal() {
            return PlaySpan::new(start, wa, wb, looping, self.rate);
        }
        let wa = self.sample_ceil(wa);
        PlaySpan { start: 0, wa, wb: self.sample_ceil(wb).max(wa.saturating_add(1)), looping }
    }

    /// Source sample and interpolation phase at an absolute output cursor. Computing this
    /// from the timeline (not by accumulating block lengths) makes partitioning immaterial.
    fn position(self, sample: i64, mix: u32) -> (i64, f32) {
        let n = i128::from(self.time(sample).0) * i128::from(mix);
        let d = i128::from(TICKS_PER_SECOND);
        (n.div_euclid(d) as i64, n.rem_euclid(d) as f32 / d as f32)
    }

    pub(super) fn mix(self, cursor: i64, n: usize, mix: u32, mut render: impl FnMut(Tick, usize) -> Vec<f32>) -> Vec<f32> {
        let mut out = Vec::with_capacity(n.saturating_mul(2));
        let mut page = Vec::new();
        let mut first = 0;
        for i in 0..n {
            let sample = cursor.saturating_add(i as i64);
            let (at, fraction) = self.position(sample, mix);
            let offset = usize::try_from(at.saturating_sub(first)).ok();
            if page.is_empty() || offset.is_none_or(|i| i >= (page.len() / 2).saturating_sub(1)) {
                first = at;
                let last = self.position(cursor.saturating_add(n.saturating_sub(1) as i64), mix).0;
                let next = self.position(sample.saturating_add(1), mix).0;
                let frames = if next.saturating_sub(first) >= SOURCE_PAGE as i64 - 1 {
                    // No following output sample can reuse this page. Read only this sample
                    // and its interpolation neighbor, rather than mixing an unused gap.
                    2
                } else {
                    last.saturating_sub(first).saturating_add(2).clamp(2, SOURCE_PAGE as i64) as usize
                };
                // mix_comp floors its starting sample. Add the fractional tick back so exotic
                // source rates start on `first`, not the preceding composition sample.
                let mut t = Tick::from_units(first, i64::from(mix));
                if t.to_units_floor(i64::from(mix)) < first {
                    t.0 = t.0.saturating_add(1);
                }
                page = render(t, frames);
            }
            let index = usize::try_from(at.saturating_sub(first)).unwrap_or(usize::MAX).saturating_mul(2);
            for channel in 0..2 {
                let a = page.get(index.saturating_add(channel)).copied().unwrap_or(0.0);
                let b = page.get(index.saturating_add(2 + channel)).copied().unwrap_or(a);
                out.push(a + (b - a) * fraction);
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::clock_sample;

    #[test]
    fn normal_speed_preserves_the_legacy_sample_grid() {
        for rate in [44_100, 48_000, 48_001] {
            let timeline = PreviewRate::new(rate, 1.0, Tick::from_seconds_f64(-0.3)).unwrap();
            for sample in [-48_001, -1, 0, 1, 48_001] {
                assert_eq!(timeline.time(sample), crate::audio::sample_time(sample, rate));
            }
        }
    }

    #[test]
    fn speed_clock_matches_slow_fast_and_fractional_previews() {
        for (comp, preview) in [(FrameRate::FPS_60, 30.0), (FrameRate::FPS_30, 60.0), (FrameRate::FPS_59_94, 29.97)] {
            let speed = preview / comp.as_f64();
            let start = comp.tick_of(17);
            let rate = PreviewRate::new(48_000, speed, start).unwrap();
            assert_eq!(rate.time(0), start, "the first frame stays exact");
            let time = rate.time(48_000);
            assert!(((time - start).seconds() - speed).abs() < 1e-9);
            assert_eq!(comp.frame_at(time), comp.frame_at(start + Tick::from_seconds_f64(speed)));
        }
    }

    #[test]
    fn speed_clock_subtracts_output_latency_and_wraps_inside_the_work_area() {
        for speed in [0.5, 2.0, 0.73] {
            let start = FrameRate::FPS_29_97.tick_of(19);
            let wa = FrameRate::FPS_29_97.tick_of(13);
            let wb = FrameRate::FPS_29_97.tick_of(43);
            let rate = PreviewRate::new(48_000, speed, start).unwrap();
            let span = rate.span(start, wa, wb, true);
            assert_eq!(rate.time(clock_sample(&span, 512, 1024)), start);
            assert!(rate.time(span.wb - 1) < wb);
            let wrapped = rate.time(clock_sample(&span, span.wb as u64 + 1024, 1024));
            assert!(wrapped >= wa && (wrapped - wa).seconds() < speed / 48_000.0);
            let once = PlaySpan { looping: false, ..span };
            assert!(rate.time(clock_sample(&once, u32::MAX as u64, 0)) < wb);
        }
    }

    fn ramp(t: Tick, n: usize, mix: u32) -> Vec<f32> {
        let first = t.to_units_floor(i64::from(mix));
        (0..n).flat_map(|i| [(first + i as i64) as f32 / 100_000.0, -(first + i as i64) as f32 / 100_000.0]).collect()
    }

    #[test]
    fn speed_mix_keeps_fractional_phase_across_uneven_blocks() {
        for speed in [0.5, 2.0, 0.73, 29.97 / FrameRate::FPS_59_94.as_f64()] {
            let start = FrameRate::FPS_29_97.tick_of(17);
            let rate = PreviewRate::new(48_000, speed, start).unwrap();
            for mix in [44_100, 48_000, 96_000] {
                let whole = rate.mix(0, 2048, mix, |t, n| ramp(t, n, mix));
                let split: Vec<_> = [(0, 321), (321, 107), (428, 1620)].into_iter().flat_map(|(at, n)| rate.mix(at, n, mix, |t, n| ramp(t, n, mix))).collect();
                assert_eq!(whole, split, "speed {speed}, source rate {mix}");
                for (i, frame) in whole.as_chunks::<2>().0.iter().enumerate() {
                    let expected = (start.seconds() + i as f64 * speed / 48_000.0) * mix as f64 / 100_000.0;
                    assert!((f64::from(frame[0]) - expected).abs() < 1e-6);
                    assert_eq!(frame[1], -frame[0]);
                }
            }
        }
    }

    #[test]
    fn extreme_speeds_keep_source_reads_bounded_without_changing_speed() {
        for speed in [0.00001, 99_900.0] {
            let rate = PreviewRate::new(8_000, speed, Tick::ZERO).unwrap();
            let mut rendered = 0;
            let out = rate.mix(0, 1024, 192_000, |_, n| {
                assert!(n <= SOURCE_PAGE);
                rendered += n;
                vec![0.25; n * 2]
            });
            assert!(rendered <= SOURCE_PAGE, "unused source intervals were mixed: {rendered} samples");
            assert_eq!(out, vec![0.25; 2048]);
            assert!((rate.time(8000).seconds() - speed).abs() < 1e-9);
        }
        for speed in [0.0, -1.0, f64::NAN, f64::INFINITY, f64::MAX] {
            assert!(PreviewRate::new(48_000, speed, Tick::ZERO).is_err());
        }
    }
}
