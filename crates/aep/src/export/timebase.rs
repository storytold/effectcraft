//! The composition time base: the pair (frame duration, time scale) After Effects stores for a
//! frame rate, and conversions between [`Tick`]s and time units.
//!
//! Evidence (generated compositions saved by After Effects, see `docs/aep-export.md`): the time
//! scale is 23 976 for the NTSC rates (frame duration 1000, 800, 400, 200 for 23.976, 29.97,
//! 59.94, 119.88); for every other rate the frame duration is the smallest power of two for which
//! `rate x frame duration >= 24 576` (25 fps: 1024 and 25 600; 12 fps: 2048 and 24 576; 1 fps:
//! 32 768 and 32 768; 240 fps: 128 and 30 720).

use effectcraft_time::{FrameRate, TICKS_PER_SECOND, Tick};

/// Frame duration and time scale in units; one frame lasts `frame_duration` units and a second
/// has `timescale` units.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TimeBase {
    pub frame_duration: u32,
    pub timescale: u32,
}

/// The 16.16 fixed-point rate After Effects stores next to the time base (a float32 rate).
pub(crate) fn fixed_rate(rate: FrameRate) -> u32 {
    let fps = ntsc_typed(rate).unwrap_or(rate.as_f64() as f32);
    let v = (f64::from(fps) * 65536.0).round();
    if v.is_finite() && v >= 0.0 && v <= f64::from(u32::MAX) { v as u32 } else { 0 }
}

/// The typed value of an NTSC rate (23.976, 29.97, 59.94, 119.88), which After Effects stores as
/// a float32 of exactly these decimals.
pub(crate) fn ntsc_typed(rate: FrameRate) -> Option<f32> {
    [(FrameRate::FPS_23_976, 23.976f32), (FrameRate::FPS_29_97, 29.97), (FrameRate::FPS_59_94, 59.94), (FrameRate::FPS_119_88, 119.88)]
        .into_iter()
        .find(|(r, _)| *r == rate)
        .map(|(_, v)| v)
}

/// The time base for `rate`, or `None` for a rate After Effects cannot hold (non-positive, or not
/// representable with a frame duration that is a power of two).
pub(crate) fn time_base(rate: FrameRate) -> Option<TimeBase> {
    if rate.num <= 0 || rate.den <= 0 {
        return None;
    }
    if let Some(v) = ntsc_typed(rate) {
        let fd = if v < 24.0 {
            1000
        } else if v < 30.0 {
            800
        } else if v < 60.0 {
            400
        } else {
            200
        };
        return Some(TimeBase { frame_duration: fd, timescale: 23976 });
    }
    let (num, den) = (i128::from(rate.num), i128::from(rate.den));
    let mut fd: i128 = 1;
    while fd <= 65536 {
        let ts = num.checked_mul(fd)?;
        if ts % den == 0 {
            let ts = ts / den;
            if ts >= 24576 {
                return Some(TimeBase { frame_duration: u32::try_from(fd).ok()?, timescale: u32::try_from(ts).ok()? });
            }
        }
        fd *= 2;
    }
    None
}

impl TimeBase {
    /// `tick` in time units, rounded to the nearest unit; the second value tells whether it was
    /// not exact.
    pub fn units(&self, tick: Tick) -> (i64, bool) {
        let n = i128::from(tick.0) * i128::from(self.timescale);
        let d = i128::from(TICKS_PER_SECOND);
        let q = n.div_euclid(d);
        let r = n.rem_euclid(d);
        let rounded = if r * 2 >= d { q + 1 } else { q };
        (i64::try_from(rounded).unwrap_or(if rounded < 0 { i64::MIN } else { i64::MAX }), r != 0)
    }

    /// Whole frames of `tick` (nearest).
    pub fn frames(&self, tick: Tick, rate: FrameRate) -> i64 {
        let n = i128::from(tick.0) * i128::from(rate.num);
        let d = i128::from(TICKS_PER_SECOND) * i128::from(rate.den.max(1));
        let q = n.div_euclid(d);
        let r = n.rem_euclid(d);
        let f = if r * 2 >= d { q + 1 } else { q };
        i64::try_from(f).unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rates_match_after_effects() {
        let tb = |n, d| time_base(FrameRate::new(n, d)).map(|t| (t.frame_duration, t.timescale));
        assert_eq!(tb(25, 1), Some((1024, 25600)));
        assert_eq!(tb(30, 1), Some((1024, 30720)));
        assert_eq!(tb(24, 1), Some((1024, 24576)));
        assert_eq!(tb(12, 1), Some((2048, 24576)));
        assert_eq!(tb(25, 2), Some((2048, 25600)));
        assert_eq!(tb(15, 1), Some((2048, 30720)));
        assert_eq!(tb(48, 1), Some((512, 24576)));
        assert_eq!(tb(50, 1), Some((512, 25600)));
        assert_eq!(tb(60, 1), Some((512, 30720)));
        assert_eq!(tb(240, 1), Some((128, 30720)));
        assert_eq!(tb(1, 1), Some((32768, 32768)));
        assert_eq!(tb(30000, 1001), Some((800, 23976)));
        assert_eq!(tb(24000, 1001), Some((1000, 23976)));
        assert_eq!(tb(60000, 1001), Some((400, 23976)));
        assert_eq!(tb(120000, 1001), Some((200, 23976)));
        assert_eq!(tb(0, 1), None);
        assert_eq!(tb(1, 0), None);
    }

    #[test]
    fn fixed_point_rates() {
        assert_eq!(fixed_rate(FrameRate::new(25, 1)), 0x19_0000);
        assert_eq!(fixed_rate(FrameRate::FPS_29_97), 1_964_114);
        assert_eq!(fixed_rate(FrameRate::FPS_23_976), 1_571_291);
        assert_eq!(fixed_rate(FrameRate::new(12, 1)), 0xc0000);
    }

    #[test]
    fn units_round_to_the_nearest() {
        let tb = TimeBase { frame_duration: 1024, timescale: 25600 };
        assert_eq!(tb.units(Tick(TICKS_PER_SECOND)), (25600, false));
        assert_eq!(tb.units(Tick(TICKS_PER_SECOND / 3)).0, 8533);
        assert!(tb.units(Tick(TICKS_PER_SECOND / 3)).1);
        assert_eq!(tb.frames(Tick(TICKS_PER_SECOND * 2), FrameRate::new(25, 1)), 50);
    }
}
