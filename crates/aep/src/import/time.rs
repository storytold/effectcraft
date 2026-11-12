//! Exact conversion of composition time (units of the file's time scale, or rational seconds) to
//! [`Tick`]s.
//!
//! A time that falls on a frame of the composition becomes `FrameRate::tick_of(frame)`, so frames
//! stay frames at every rate (29.97 fps is `23976 / 800` in the file and `30000 / 1001` in the
//! project). Any other time is rounded to the nearest tick.

use effectcraft_time::{FrameRate, TICKS_PER_SECOND, Tick};

/// The composition's frame structure: the stored time base and the rate the project gets.
#[derive(Clone, Copy, Debug)]
pub(crate) struct TimeMap {
    /// The stored time scale (units per second).
    pub timescale: u32,
    /// The stored frame duration in units.
    pub frame_duration: u32,
    /// The project's frame rate.
    pub rate: FrameRate,
}

/// The project frame rate for a stored time base. The NTSC rates are stored as typed
/// (`29.97` is `23976 / 800`, `23.976` is `23976 / 1000`) and become the exact NTSC rates; every
/// other rate stays exactly the stored fraction.
pub(crate) fn frame_rate(timescale: u32, frame_duration: u32) -> Option<FrameRate> {
    if timescale == 0 || frame_duration == 0 {
        return None;
    }
    let exact = FrameRate::new(i64::from(timescale), i64::from(frame_duration));
    let fps = exact.as_f64();
    for ntsc in [FrameRate::FPS_23_976, FrameRate::FPS_29_97, FrameRate::FPS_59_94, FrameRate::FPS_119_88] {
        if (ntsc.as_f64() - fps).abs() < 0.0005 {
            return Some(ntsc);
        }
    }
    Some(exact)
}

impl TimeMap {
    /// Seconds `num / den` as a tick (zero for a zero denominator).
    pub fn ticks(&self, num: i64, den: i64) -> Tick {
        if den == 0 {
            return Tick::ZERO;
        }
        let (num, den) = (i128::from(num), i128::from(den));
        // Whole frames of the stored time base: num/den s * timescale / frame_duration.
        let scaled = num.saturating_mul(i128::from(self.timescale));
        let per_frame = den.saturating_mul(i128::from(self.frame_duration));
        if per_frame != 0 && scaled % per_frame == 0 {
            let frames = scaled / per_frame;
            if let Ok(frames) = i64::try_from(frames) {
                return clamp(self.rate.tick_of(frames).0.into());
            }
        }
        let n = num.saturating_mul(i128::from(TICKS_PER_SECOND));
        // Round half up on the signed value.
        let twice = n.saturating_mul(2).saturating_add(den);
        clamp(twice.div_euclid(den.saturating_mul(2)))
    }

    /// `units` of `scale` units per second.
    pub fn units(&self, units: i64, scale: u32) -> Tick {
        self.ticks(units, i64::from(scale))
    }
}

fn clamp(v: i128) -> Tick {
    Tick(i64::try_from(v).unwrap_or(if v < 0 { i64::MIN } else { i64::MAX }).clamp(Tick::MIN.0, Tick::MAX.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(timescale: u32, frame_duration: u32) -> TimeMap {
        TimeMap { timescale, frame_duration, rate: frame_rate(timescale, frame_duration).unwrap() }
    }

    #[test]
    fn ntsc_rates_are_exact() {
        assert_eq!(frame_rate(23976, 800), Some(FrameRate::FPS_29_97));
        assert_eq!(frame_rate(23976, 1000), Some(FrameRate::FPS_23_976));
        assert_eq!(frame_rate(25600, 1024), Some(FrameRate::new(25, 1)));
        assert_eq!(frame_rate(30720, 1024), Some(FrameRate::new(30, 1)));
        assert_eq!(frame_rate(0, 1), None);
        assert_eq!(frame_rate(1, 0), None);
    }

    #[test]
    fn frames_stay_frames() {
        let m = map(23976, 800);
        // 150 frames at 29.97 fps = 120000 units.
        assert_eq!(m.units(120_000, 23976), FrameRate::FPS_29_97.tick_of(150));
        assert_eq!(FrameRate::FPS_29_97.frame_at(m.units(120_000, 23976)), 150);
        // The same time as a reduced rational.
        assert_eq!(m.ticks(1001, 200), FrameRate::FPS_29_97.tick_of(150));
        let m25 = map(25600, 1024);
        assert_eq!(m25.units(25_600, 25_600), Tick(TICKS_PER_SECOND));
        assert_eq!(m25.ticks(1, 2), Tick(TICKS_PER_SECOND / 2));
    }

    #[test]
    fn off_frame_times_round_and_hostile_values_do_not_panic() {
        let m = map(25600, 1024);
        assert_eq!(m.ticks(1, 3).0, (TICKS_PER_SECOND as f64 / 3.0).round() as i64);
        assert_eq!(m.ticks(5, 0), Tick::ZERO);
        let _ = m.ticks(i64::MAX, 1);
        let _ = m.ticks(i64::MIN, 1);
        let _ = m.ticks(i64::MIN, i64::MAX);
        let _ = m.units(i64::MAX, u32::MAX);
        assert!(m.ticks(i64::MAX, 1) <= Tick::MAX);
    }
}
