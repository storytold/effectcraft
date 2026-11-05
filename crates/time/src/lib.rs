//! Exact media time for EffectCraft (shared design with FilmCraft).
//!
//! All time is an integer number of [`Tick`]s at [`TICKS_PER_SECOND`] = 254 016 000 000/s. That
//! rate divides evenly into every broadcast frame duration (23.976, 24, 25, 29.97, 30, 48, 50,
//! 59.94, 60, 120 …) and every common audio sample duration (8 k … 192 kHz, including the 44.1 k
//! family), so edits, frame math and audio alignment never drift.
//!
//! Timecode (SMPTE drop/non-drop, frames, feet+frames, samples) is only a *display* of ticks.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use serde::{Deserialize, Serialize};
use std::fmt;
use std::ops::{Add, AddAssign, Neg, Sub, SubAssign};

/// Ticks per second.
pub const TICKS_PER_SECOND: i64 = 254_016_000_000;

/// A point or duration in time, in ticks.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Tick(pub i64);

impl Tick {
    pub const ZERO: Tick = Tick(0);
    pub const MAX: Tick = Tick(i64::MAX / 4);
    pub const MIN: Tick = Tick(i64::MIN / 4);

    /// Seconds to ticks, clamped to [`Tick::MIN`]..=[`Tick::MAX`] (NaN is zero), so times from
    /// files, users and agents can't overflow later arithmetic.
    pub fn from_seconds_f64(s: f64) -> Tick {
        let t = (s * TICKS_PER_SECOND as f64).round();
        // `as` saturates (NaN becomes 0); then keep within the documented range.
        Tick((t as i64).clamp(Tick::MIN.0, Tick::MAX.0))
    }
    pub fn seconds(self) -> f64 {
        self.0 as f64 / TICKS_PER_SECOND as f64
    }
    /// Exact conversion from a count of `units` at `per_second` (e.g. samples at 48 000).
    pub fn from_units(units: i64, per_second: i64) -> Tick {
        Tick(clamp128((units as i128 * TICKS_PER_SECOND as i128) / nonzero(per_second)))
    }
    /// Floor conversion to a count of units at `per_second`.
    pub fn to_units_floor(self, per_second: i64) -> i64 {
        clamp128((self.0 as i128 * per_second as i128).div_euclid(TICKS_PER_SECOND as i128))
    }
    /// Conversion from a rational timestamp `pts * num / den` seconds (container timebases).
    pub fn from_rational(pts: i64, num: i64, den: i64) -> Tick {
        let t = (pts as i128).saturating_mul(num as i128).saturating_mul(TICKS_PER_SECOND as i128);
        Tick(clamp128(t.div_euclid(nonzero(den))))
    }
    /// Inverse of [`Tick::from_rational`], floored to the timebase.
    pub fn to_rational_floor(self, num: i64, den: i64) -> i64 {
        clamp128((self.0 as i128 * den as i128).div_euclid(nonzero(num) * TICKS_PER_SECOND as i128))
    }
    pub fn abs(self) -> Tick {
        Tick(self.0.saturating_abs())
    }
    pub fn min(self, o: Tick) -> Tick {
        Tick(self.0.min(o.0))
    }
    pub fn max(self, o: Tick) -> Tick {
        Tick(self.0.max(o.0))
    }
    pub fn clamp(self, lo: Tick, hi: Tick) -> Tick {
        Tick(self.0.clamp(lo.0, hi.0))
    }
    /// Scale by a rational factor (`num/den`), flooring.
    pub fn mul_ratio(self, num: i64, den: i64) -> Tick {
        Tick(clamp128((self.0 as i128 * num as i128).div_euclid(nonzero(den))))
    }
}

/// A divisor that is never zero (an invalid zero rate or timebase from a file divides by 1
/// instead of panicking).
fn nonzero(v: i64) -> i128 {
    if v == 0 { 1 } else { v as i128 }
}

/// An `i128` intermediate as `i64`, saturating instead of wrapping.
fn clamp128(v: i128) -> i64 {
    v.clamp(i64::MIN as i128, i64::MAX as i128) as i64
}

impl Add for Tick {
    type Output = Tick;
    fn add(self, o: Tick) -> Tick {
        Tick(self.0.saturating_add(o.0))
    }
}
impl Sub for Tick {
    type Output = Tick;
    fn sub(self, o: Tick) -> Tick {
        Tick(self.0.saturating_sub(o.0))
    }
}
impl Neg for Tick {
    type Output = Tick;
    fn neg(self) -> Tick {
        Tick(self.0.saturating_neg())
    }
}
impl AddAssign for Tick {
    fn add_assign(&mut self, o: Tick) {
        self.0 = self.0.saturating_add(o.0);
    }
}
impl SubAssign for Tick {
    fn sub_assign(&mut self, o: Tick) {
        self.0 = self.0.saturating_sub(o.0);
    }
}

/// A half-open time range `[start, start + duration)`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TimeRange {
    pub start: Tick,
    pub duration: Tick,
}

impl TimeRange {
    pub fn new(start: Tick, duration: Tick) -> Self {
        Self { start, duration }
    }
    pub fn from_bounds(start: Tick, end: Tick) -> Self {
        Self { start, duration: end - start }
    }
    pub fn end(&self) -> Tick {
        self.start + self.duration
    }
    pub fn contains(&self, t: Tick) -> bool {
        t >= self.start && t < self.end()
    }
    pub fn overlaps(&self, o: &TimeRange) -> bool {
        self.start < o.end() && o.start < self.end()
    }
    pub fn intersect(&self, o: &TimeRange) -> Option<TimeRange> {
        let s = self.start.max(o.start);
        let e = self.end().min(o.end());
        (e > s).then(|| TimeRange::from_bounds(s, e))
    }
    pub fn is_empty(&self) -> bool {
        self.duration.0 <= 0
    }
}

/// A frame rate as an exact rational (`num / den` frames per second).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FrameRate {
    pub num: i64,
    pub den: i64,
}

impl Default for FrameRate {
    fn default() -> Self {
        FrameRate::FPS_23_976
    }
}

impl FrameRate {
    pub const FPS_23_976: FrameRate = FrameRate { num: 24000, den: 1001 };
    pub const FPS_24: FrameRate = FrameRate { num: 24, den: 1 };
    pub const FPS_25: FrameRate = FrameRate { num: 25, den: 1 };
    pub const FPS_29_97: FrameRate = FrameRate { num: 30000, den: 1001 };
    pub const FPS_30: FrameRate = FrameRate { num: 30, den: 1 };
    pub const FPS_48: FrameRate = FrameRate { num: 48, den: 1 };
    pub const FPS_50: FrameRate = FrameRate { num: 50, den: 1 };
    pub const FPS_59_94: FrameRate = FrameRate { num: 60000, den: 1001 };
    pub const FPS_60: FrameRate = FrameRate { num: 60, den: 1 };
    pub const FPS_119_88: FrameRate = FrameRate { num: 120000, den: 1001 };
    pub const FPS_120: FrameRate = FrameRate { num: 120, den: 1 };

    /// Rates offered in sequence settings (common composition presets).
    pub const COMMON: [FrameRate; 11] = [
        Self::FPS_23_976,
        Self::FPS_24,
        Self::FPS_25,
        Self::FPS_29_97,
        Self::FPS_30,
        Self::FPS_48,
        Self::FPS_50,
        Self::FPS_59_94,
        Self::FPS_60,
        Self::FPS_119_88,
        Self::FPS_120,
    ];

    pub fn new(num: i64, den: i64) -> Self {
        // Magnitudes can be 2^63 for i64::MIN; keep them unsigned and divide in i128.
        let g = gcd(num.unsigned_abs(), den.unsigned_abs()).max(1) as i128;
        FrameRate { num: (num as i128 / g) as i64, den: (den as i128 / g) as i64 }
    }

    /// Closest standard rate for a float (e.g. from a container's average rate).
    pub fn from_f64(fps: f64) -> Self {
        for r in Self::COMMON {
            if (r.as_f64() - fps).abs() < 0.005 {
                return r;
            }
        }
        let num = (fps * 1000.0).round() as i64;
        // A tiny positive rate must not round to the invalid 0/1 rate.
        FrameRate::new(if fps > 0.0 { num.max(1) } else { num }, 1000)
    }

    pub fn as_f64(self) -> f64 {
        self.num as f64 / self.den as f64
    }

    /// Exact duration of one frame (rounded down only for exotic rates).
    pub fn frame_duration(self) -> Tick {
        Tick(clamp128((TICKS_PER_SECOND as i128 * self.den as i128) / nonzero(self.num)))
    }

    /// Index of the frame containing `t` (floor).
    pub fn frame_at(self, t: Tick) -> i64 {
        clamp128((t.0 as i128 * self.num as i128).div_euclid(TICKS_PER_SECOND as i128 * nonzero(self.den)))
    }

    /// First tick belonging to frame `f` (ceiling at a fractional tick boundary).
    /// Common broadcast rates have exact integer boundaries. For other rates, flooring would
    /// put the returned tick in frame `f - 1`, since [`Self::frame_at`] floors its frame index.
    pub fn tick_of(self, f: i64) -> Tick {
        let n = (f as i128).saturating_mul(TICKS_PER_SECOND as i128).saturating_mul(self.den as i128);
        let d = nonzero(self.num);
        // The only signed division overflow (MIN / -1) has a positive result beyond i128.
        let q = n.checked_div_euclid(d).unwrap_or(i128::MAX);
        let r = n.checked_rem_euclid(d).unwrap_or(0);
        Tick(clamp128(if d > 0 && r != 0 { q.saturating_add(1) } else { q }))
    }

    /// Snap `t` down to a frame boundary.
    pub fn snap(self, t: Tick) -> Tick {
        self.tick_of(self.frame_at(t))
    }

    /// Snap `t` to the nearest frame boundary.
    pub fn snap_nearest(self, t: Tick) -> Tick {
        let a = self.snap(t);
        let b = self.tick_of(self.frame_at(t).saturating_add(1));
        if (t - a) <= (b - t) { a } else { b }
    }

    /// Timecode base (frames counted per timecode second): 30 for 29.97, 24 for 23.976.
    pub fn timecode_base(self) -> i64 {
        if self.num <= 0 || self.den <= 0 {
            return 1;
        }
        clamp128((i128::from(self.num) + i128::from(self.den) - 1) / i128::from(self.den)).max(1)
    }

    /// NTSC (x/1001) rates can use drop-frame timecode.
    pub fn is_ntsc(self) -> bool {
        self.den == 1001
    }

    /// Whether drop-frame counting applies (29.97 / 59.94 / 119.88).
    pub fn supports_drop_frame(self) -> bool {
        self.is_ntsc() && self.timecode_base() % 30 == 0
    }

    pub fn label(self) -> String {
        if self.den == 1 {
            format!("{}", self.num)
        } else {
            let v = self.as_f64();
            let s = format!("{v:.3}");
            let s = s.trim_end_matches('0').trim_end_matches('.');
            s.to_string()
        }
    }
}

impl fmt::Display for FrameRate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} fps", self.label())
    }
}

fn gcd(mut a: u64, mut b: u64) -> u64 {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a
}

/// How time is displayed (Timecode, Feet+Frames 16mm/35mm, Frames, Audio Samples).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TimeDisplay {
    #[default]
    Timecode,
    Frames,
    Feet16,
    Feet35,
    AudioSamples,
    Seconds,
}

impl TimeDisplay {
    pub const ALL: [TimeDisplay; 6] =
        [TimeDisplay::Timecode, TimeDisplay::Feet35, TimeDisplay::Feet16, TimeDisplay::Frames, TimeDisplay::AudioSamples, TimeDisplay::Seconds];
    pub fn label(self) -> &'static str {
        match self {
            TimeDisplay::Timecode => "Timecode",
            TimeDisplay::Frames => "Frames",
            TimeDisplay::Feet16 => "Feet + Frames 16mm",
            TimeDisplay::Feet35 => "Feet + Frames 35mm",
            TimeDisplay::AudioSamples => "Audio Samples",
            TimeDisplay::Seconds => "Seconds",
        }
    }
}

/// Drop-frame parameters: frames dropped per minute and nominal base.
fn df_params(rate: FrameRate) -> (i64, i64) {
    let base = rate.timecode_base();
    (base / 15, base) // 30 → 2, 60 → 4, 120 → 8
}

/// Convert a frame count to SMPTE fields `(negative, h, m, s, f)`.
pub fn frames_to_fields(frame: i64, rate: FrameRate, drop_frame: bool) -> (bool, i64, i64, i64, i64) {
    let neg = frame < 0;
    let mut n = frame.saturating_abs();
    let (drop, base) = df_params(rate);
    if drop_frame && rate.supports_drop_frame() {
        let per_min = base.saturating_mul(60) - drop;
        let per_10 = per_min.saturating_mul(10) + drop;
        let d = n / per_10;
        let m = n % per_10;
        n = n.saturating_add((drop * 9).saturating_mul(d));
        if m > drop {
            n = n.saturating_add(drop * ((m - drop) / per_min));
        }
    }
    let f = n % base;
    let s = (n / base) % 60;
    let mi = (n / base.saturating_mul(60)) % 60;
    let h = n / base.saturating_mul(3600);
    (neg, h, mi, s, f)
}

/// Convert SMPTE fields back to a frame count.
pub fn fields_to_frames(h: i64, m: i64, s: i64, f: i64, rate: FrameRate, drop_frame: bool) -> i64 {
    let (drop, base) = df_params(rate);
    let secs = h.saturating_mul(3600).saturating_add(m.saturating_mul(60)).saturating_add(s);
    let mut n = secs.saturating_mul(base).saturating_add(f);
    if drop_frame && rate.supports_drop_frame() {
        let total_min = h.saturating_mul(60).saturating_add(m);
        n = n.saturating_sub(drop.saturating_mul(total_min - total_min / 10));
    }
    n
}

/// Format a frame count as SMPTE timecode (`HH:MM:SS:FF`, or `HH;MM;SS;FF` for drop-frame).
pub fn format_timecode_frames(frame: i64, rate: FrameRate, drop_frame: bool) -> String {
    let df = drop_frame && rate.supports_drop_frame();
    let (neg, h, m, s, f) = frames_to_fields(frame, rate, df);
    let sep = if df { ';' } else { ':' };
    let fw = if rate.timecode_base() >= 100 { 3 } else { 2 };
    format!("{}{h:02}{sep}{m:02}{sep}{s:02}{sep}{f:0fw$}", if neg { "-" } else { "" })
}

/// Format a frame count the way After Effects displays the current time: hours not padded
/// (`0:00:02:15`, or `0;00;02;15` for drop-frame).
pub fn format_timecode_ae(frame: i64, rate: FrameRate, drop_frame: bool) -> String {
    let df = drop_frame && rate.supports_drop_frame();
    let (neg, h, m, s, f) = frames_to_fields(frame, rate, df);
    let sep = if df { ';' } else { ':' };
    let fw = if rate.timecode_base() >= 100 { 3 } else { 2 };
    format!("{}{h}{sep}{m:02}{sep}{s:02}{sep}{f:0fw$}", if neg { "-" } else { "" })
}

/// Format a tick according to a display mode.
pub fn format_time(t: Tick, rate: FrameRate, drop_frame: bool, display: TimeDisplay, sample_rate: i64) -> String {
    let frame = rate.frame_at(t);
    match display {
        TimeDisplay::Timecode => format_timecode_frames(frame, rate, drop_frame),
        TimeDisplay::Frames => format!("{frame}"),
        TimeDisplay::Feet35 | TimeDisplay::Feet16 => {
            let per_ft = if display == TimeDisplay::Feet35 { 16 } else { 40 };
            let neg = frame < 0;
            let a = frame.unsigned_abs();
            format!("{}{}+{:02}", if neg { "-" } else { "" }, a / per_ft, a % per_ft)
        }
        TimeDisplay::AudioSamples => {
            let sign = if t.0 < 0 { "-" } else { "" };
            let abs = t.0.saturating_abs();
            let secs = abs.div_euclid(TICKS_PER_SECOND);
            let rem = Tick(abs.rem_euclid(TICKS_PER_SECOND)).to_units_floor(sample_rate);
            format!("{sign}{:02}:{:02}:{:02}:{rem:05}", secs / 3600, (secs / 60) % 60, secs % 60)
        }
        TimeDisplay::Seconds => {
            let s = t.seconds();
            // A value that rounds to zero must not print a sign ("-0.000").
            format!("{:.3}", if (s * 1000.0).round() == 0.0 { 0.0 } else { s })
        }
    }
}

/// Error from [`parse_timecode`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError(pub String);

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for ParseError {}

/// Parse user-typed timecode into a frame count, the way NLE timecode fields do:
/// - `01:02:03:04`, `01;02;03;04`, `1.2.3.4` (any of `:;.,` separators),
/// - bare digits are read right-aligned as `HHMMSSFF` (`1000` = 00:00:10:00),
/// - a leading `+`/`-` makes the value relative to `current` (`+15` = 15 frames later, `-1.00` = 1 s earlier).
pub fn parse_timecode(input: &str, rate: FrameRate, drop_frame: bool, current: i64) -> Result<i64, ParseError> {
    let s = input.trim();
    if s.is_empty() {
        return Err(ParseError("empty timecode".into()));
    }
    let (rel, body) = match s.as_bytes()[0] {
        b'+' => (Some(1), &s[1..]),
        b'-' => (Some(-1), &s[1..]),
        _ => (None, s),
    };
    let parts: Vec<&str> = body.split([':', ';', '.', ',']).collect();
    let nums: Vec<i64> = if parts.len() == 1 {
        let digits = parts[0];
        if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
            return Err(ParseError(format!("not a timecode: `{input}`")));
        }
        // Right-aligned pairs: "12345" -> [1, 23, 45]; relative bare numbers are frames.
        if rel.is_some() {
            vec![digits.parse().map_err(|_| ParseError("number too large".into()))?]
        } else {
            let mut v = Vec::new();
            let b = digits.as_bytes();
            let mut end = b.len();
            while end > 0 {
                let start = end.saturating_sub(2);
                v.push(digits[start..end].parse::<i64>().unwrap_or(0));
                end = start;
            }
            v.reverse();
            v
        }
    } else {
        parts
            .iter()
            .map(|p| if p.is_empty() { Ok(0) } else { p.parse::<i64>().map_err(|_| ParseError(format!("bad field `{p}`"))) })
            .collect::<Result<_, _>>()?
    };
    if nums.len() > 4 {
        return Err(ParseError("too many timecode fields".into()));
    }
    let mut f4 = [0i64; 4];
    let off = 4 - nums.len();
    f4[off..].copy_from_slice(&nums);
    let [h, m, sec, fr] = f4;
    // Overflowing fields (e.g. 90 frames) are allowed, as in common NLE timecode fields.
    let base = rate.timecode_base();
    let too_large = || ParseError("timecode too large".into());
    let frames = if nums.len() == 1 && rel.is_some() {
        fr
    } else if drop_frame && rate.supports_drop_frame() && m < 60 && sec < 60 && fr < base {
        if h > i64::MAX / base.saturating_mul(3600) {
            return Err(too_large());
        }
        fields_to_frames(h, m, sec, fr, rate, true)
    } else {
        h.checked_mul(3600)
            .and_then(|v| v.checked_add(m.checked_mul(60)?))
            .and_then(|v| v.checked_add(sec))
            .and_then(|v| v.checked_mul(base))
            .and_then(|v| v.checked_add(fr))
            .ok_or_else(too_large)?
    };
    Ok(match rel {
        Some(-1) => current.saturating_sub(frames),
        Some(_) => current.saturating_add(frames),
        None => frames,
    })
}

/// Format a frame count as Feet + Frames (film footage counting) with `per_foot` frames per
/// foot (35 mm film: 16, 16 mm: 40): `0012+07`. Negative counts get a leading `-`.
pub fn format_feet_frames(frame: i64, per_foot: i64) -> String {
    let pf = per_foot.max(1);
    let sign = if frame < 0 { "-" } else { "" };
    let a = frame.unsigned_abs();
    let pf = pf as u64;
    let w = if pf > 100 { 3 } else { 2 };
    format!("{sign}{:04}+{:0w$}", a / pf, a % pf)
}

/// Parse a Feet + Frames field into a frame count: `12+07` (feet + frames; frames may overflow
/// a foot), `-1+04`, a bare frame count (`250`), or `+n` / `-n` frames relative to `current`.
pub fn parse_feet_frames(input: &str, per_foot: i64, current: i64) -> Result<i64, ParseError> {
    let s = input.trim();
    let bad = || ParseError(format!("not feet+frames: `{input}`"));
    let num = |t: &str| -> Result<i64, ParseError> {
        let t = t.trim();
        if t.is_empty() {
            return Ok(0);
        }
        if !t.bytes().all(|b| b.is_ascii_digit()) {
            return Err(bad());
        }
        t.parse::<i64>().map_err(|_| ParseError("number too large".into()))
    };
    if s.is_empty() {
        return Err(ParseError("empty feet+frames".into()));
    }
    let (sign, body) = match s.as_bytes()[0] {
        b'-' => (-1, &s[1..]),
        b'+' => (1, &s[1..]),
        _ => (0, s),
    };
    match body.rfind('+') {
        Some(i) => {
            let (feet, frames) = (num(&body[..i])?, num(&body[i + 1..])?);
            let v = feet.checked_mul(per_foot.max(1)).and_then(|v| v.checked_add(frames)).ok_or_else(|| ParseError("number too large".into()))?;
            // A sign before feet+frames is the sign of the count, not a relative move.
            Ok(if sign < 0 { -v } else { v })
        }
        None => {
            let v = num(body)?;
            Ok(match sign {
                0 => v,
                k => current.saturating_add(k * v),
            })
        }
    }
}

/// Common audio sample rates offered in sequence settings.
pub const SAMPLE_RATES: [i64; 6] = [32_000, 44_100, 48_000, 88_200, 96_000, 192_000];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_f64_tiny_positive_fps_is_nonzero() {
        let r = FrameRate::from_f64(0.0001);
        assert!(r.num > 0 && r.den > 0, "got {r:?}");
    }

    #[test]
    fn seconds_display_has_no_negative_zero() {
        for t in [Tick(-1), Tick(0), Tick(-100)] {
            assert_eq!(format_time(t, FrameRate::FPS_30, false, TimeDisplay::Seconds, 48000), "0.000");
        }
        let one = Tick(-TICKS_PER_SECOND);
        assert_eq!(format_time(one, FrameRate::FPS_30, false, TimeDisplay::Seconds, 48000), "-1.000");
    }

    #[test]
    fn frame_rate_constructors_accept_extreme_numbers() {
        assert_eq!(FrameRate::new(i64::MIN, 1), FrameRate { num: i64::MIN, den: 1 });
        assert_eq!(FrameRate::new(i64::MIN, i64::MIN), FrameRate { num: -1, den: -1 });
        assert_eq!(FrameRate::new(0, i64::MIN), FrameRate { num: 0, den: -1 });
        assert_eq!(FrameRate::new(i64::MIN, 0), FrameRate { num: -1, den: 0 });
        assert_eq!(FrameRate::new(0, 0), FrameRate { num: 0, den: 0 });
        assert_eq!(FrameRate::from_f64(f64::NAN), FrameRate { num: 0, den: 1 });
        for fps in [f64::NEG_INFINITY, -1e308, -9.2e18, f64::INFINITY, 1e308] {
            assert_eq!(FrameRate::from_f64(fps), FrameRate::new((fps * 1000.0).round() as i64, 1000));
        }
    }

    #[test]
    fn hostile_numbers_never_overflow() {
        // `layer.timing {"delta": -1e308}` over the control channel overflowed `snap_nearest`;
        // a timecode field holding `99999999999999:00:00:00` overflowed the parser.
        let r = FrameRate::FPS_29_97;
        for s in [1e308, -1e308, f64::INFINITY, f64::NEG_INFINITY, f64::NAN, 9.2e18] {
            let t = Tick::from_seconds_f64(s);
            assert!(t >= Tick::MIN && t <= Tick::MAX);
            let _ = r.snap_nearest(t) + t - t;
            let _ = format_time(t, r, true, TimeDisplay::Timecode, 48_000);
            let _ = format_time(t, r, false, TimeDisplay::Feet16, 48_000);
        }
        assert_eq!(Tick::from_seconds_f64(f64::NAN), Tick::ZERO);
        assert_eq!(Tick(i64::MAX) + Tick(1), Tick(i64::MAX));
        assert_eq!(-Tick(i64::MIN), Tick(i64::MAX));
        let _ = format_timecode_frames(i64::MIN, r, true);
        let _ = format_feet_frames(i64::MIN, 16);
        assert!(parse_timecode("99999999999999:00:00:00", r, false, 0).is_err());
        assert!(parse_timecode("9223372036854775807:59:59:29", r, true, 0).is_err());
        assert!(parse_timecode("+9223372036854775807", r, false, i64::MAX).is_ok());
        assert!(parse_feet_frames("9223372036854775807+1", 16, 0).is_err());
        assert!(parse_feet_frames("+9223372036854775807", 16, 1).is_ok());
        // A zero rate or timebase (from a damaged file) doesn't divide by zero.
        let z = FrameRate { num: 0, den: 0 };
        let _ = (z.frame_duration(), z.frame_at(Tick(5)), z.tick_of(3), z.snap_nearest(Tick(7)), z.timecode_base());
        let _ = (Tick::from_units(5, 0), Tick::from_rational(1, 1, 0), Tick(5).to_rational_floor(0, 1), Tick(5).mul_ratio(1, 0));
        let huge = FrameRate { num: i64::MAX, den: 1 };
        let _ = format_timecode_frames(i64::MAX, huge, false);
    }
    #[test]
    fn hostile_rates_format_and_parse_without_panicking() {
        for rate in [
            FrameRate::new(i64::MIN, -1),
            FrameRate { num: 1, den: i64::MIN },
            FrameRate::new(i64::MIN, i64::MIN),
            FrameRate { num: 0, den: 0 },
            FrameRate { num: -30, den: 1 },
        ] {
            assert_eq!(rate.timecode_base(), 1);
            assert_eq!(format_timecode_frames(0, rate, false), "00:00:00:00");
            assert_eq!(parse_timecode("0:0:0:0", rate, false, 0).unwrap(), 0);
        }
        for rate in [FrameRate { num: i64::MAX, den: 1 }, FrameRate { num: i64::MAX, den: 1001 }] {
            let expected = ((i128::from(rate.num) + i128::from(rate.den) - 1) / i128::from(rate.den)) as i64;
            assert_eq!(rate.timecode_base(), expected);
        }
    }

    #[test]
    fn negative_relative_min_field_saturates_in_the_requested_direction() {
        assert_eq!(parse_timecode("-0:0:0:-9223372036854775808", FrameRate::FPS_30, false, 0).unwrap(), i64::MAX);
        assert_eq!(parse_timecode("+0:0:0:-9223372036854775808", FrameRate::FPS_30, false, 0).unwrap(), i64::MIN);
        assert_eq!(parse_timecode("-1", FrameRate::FPS_30, false, i64::MIN).unwrap(), i64::MIN);
    }

    #[test]
    fn common_rate_timecode_roundtrips_keep_their_existing_behavior() {
        for rate in FrameRate::COMMON {
            for frame in [-10_000, -1, 0, 1, 10_000] {
                let text = format_timecode_frames(frame, rate, false);
                assert_eq!(parse_timecode(&text, rate, false, 0).unwrap(), frame, "{rate}: {text}");
            }
        }
    }

    use proptest::prelude::*;

    #[test]
    fn every_common_rate_is_exact() {
        for r in FrameRate::COMMON {
            let d = r.frame_duration();
            assert_eq!(d.0 as i128 * r.num as i128, TICKS_PER_SECOND as i128 * r.den as i128, "{r}");
        }
        for sr in SAMPLE_RATES.iter().chain(&[8000, 11025, 16000, 22050, 176_400]) {
            assert_eq!(TICKS_PER_SECOND % sr, 0, "{sr}");
        }
    }

    #[test]
    fn fractional_tick_frame_boundaries_belong_to_the_requested_frame() {
        let rate = FrameRate::from_f64(12.345);
        assert_eq!(rate, FrameRate::new(2469, 200));
        assert_eq!(rate.tick_of(1), Tick(20_576_427_704));
        assert_eq!(rate.tick_of(-1), Tick(-20_576_427_703));
        for frame in [-10_000, -2, -1, 0, 1, 2, 10_000] {
            let start = rate.tick_of(frame);
            assert_eq!(rate.frame_at(start), frame);
            assert_eq!(rate.frame_at(start - Tick(1)), frame - 1);
            assert_eq!(rate.snap(start), start);
            assert_eq!(rate.snap_nearest(start), start);
            let next = rate.tick_of(frame + 1);
            assert_eq!(rate.frame_at(next - Tick(1)), frame);
        }
    }

    #[test]
    fn common_rate_boundaries_keep_their_exact_ticks() {
        for rate in FrameRate::COMMON {
            for frame in [-10_000, -1, 0, 1, 10_000] {
                let expected = frame as i128 * TICKS_PER_SECOND as i128 * rate.den as i128 / rate.num as i128;
                assert_eq!(rate.tick_of(frame), Tick(expected as i64), "{rate}, frame {frame}");
                assert_eq!(rate.frame_at(rate.tick_of(frame) - Tick(1)), frame - 1);
            }
        }
    }

    #[test]
    fn extreme_frame_boundaries_saturate_without_overflowing() {
        for rate in [
            FrameRate { num: 1, den: i64::MAX },
            FrameRate { num: -1, den: i64::MAX },
            FrameRate { num: i64::MAX, den: i64::MAX },
            FrameRate { num: 0, den: i64::MAX },
        ] {
            for frame in [i64::MIN, i64::MAX] {
                let _ = rate.tick_of(frame);
            }
        }
        assert_eq!(FrameRate { num: 1, den: i64::MAX }.tick_of(i64::MAX), Tick(i64::MAX));
        assert_eq!(FrameRate { num: 1, den: i64::MAX }.tick_of(i64::MIN), Tick(i64::MIN));
    }

    #[test]
    fn drop_frame_known_values() {
        let r = FrameRate::FPS_29_97;
        assert_eq!(format_timecode_frames(0, r, true), "00;00;00;00");
        assert_eq!(format_timecode_frames(1799, r, true), "00;00;59;29");
        assert_eq!(format_timecode_frames(1800, r, true), "00;01;00;02");
        assert_eq!(format_timecode_frames(17982, r, true), "00;10;00;00");
        assert_eq!(format_timecode_frames(107892, r, true), "01;00;00;00");
        assert_eq!(format_timecode_frames(1800, r, false), "00:01:00:00");
        let r60 = FrameRate::FPS_59_94;
        assert_eq!(format_timecode_frames(3600, r60, true), "00;01;00;04");
    }

    #[test]
    fn parse_forms() {
        let r = FrameRate::FPS_25;
        assert_eq!(parse_timecode("00:00:10:00", r, false, 0).unwrap(), 250);
        assert_eq!(parse_timecode("1000", r, false, 0).unwrap(), 250);
        assert_eq!(parse_timecode("1.00", r, false, 0).unwrap(), 25);
        assert_eq!(parse_timecode("+15", r, false, 100).unwrap(), 115);
        assert_eq!(parse_timecode("-1.00", r, false, 100).unwrap(), 75);
        assert_eq!(parse_timecode("00;01;00;02", FrameRate::FPS_29_97, true, 0).unwrap(), 1800);
        assert!(parse_timecode("abc", r, false, 0).is_err());
    }

    #[test]
    fn display_modes() {
        let r = FrameRate::FPS_24;
        let t = r.tick_of(40);
        assert_eq!(format_time(t, r, false, TimeDisplay::Feet35, 48000), "2+08");
        assert_eq!(format_time(t, r, false, TimeDisplay::Feet16, 48000), "1+00");
        assert_eq!(format_time(t, r, false, TimeDisplay::Frames, 48000), "40");
        assert_eq!(format_time(Tick::from_units(48_001, 48_000), r, false, TimeDisplay::AudioSamples, 48000), "00:00:01:00001");
    }

    #[test]
    fn neg_samples() {
        let r = FrameRate::FPS_30;
        let fmt = |s: f64| format_time(Tick::from_seconds_f64(s), r, false, TimeDisplay::AudioSamples, 48000);
        assert_eq!(fmt(-0.5), "-00:00:00:24000");
        assert_eq!(fmt(-3700.0), "-01:01:40:00000");
    }

    #[test]
    fn rational_conversions() {
        // 90 kHz MPEG timebase
        let t = Tick::from_rational(90_000, 1, 90_000);
        assert_eq!(t.0, TICKS_PER_SECOND);
        assert_eq!(t.to_rational_floor(1, 90_000), 90_000);
        assert_eq!(FrameRate::from_f64(29.97), FrameRate::FPS_29_97);
        assert_eq!(FrameRate::FPS_23_976.label(), "23.976");
    }

    proptest! {
        #[test]
        fn frame_rate_normalization_preserves_exact_ratio(num in any::<i64>(), den in any::<i64>()) {
            let rate = FrameRate::new(num, den);
            prop_assert_eq!(num as i128 * rate.den as i128, den as i128 * rate.num as i128);
            prop_assert_eq!(FrameRate::new(rate.num, rate.den), rate);
        }

        #[test]
        fn frame_tick_roundtrip(f in -1_000_000i64..10_000_000, ri in 0usize..11) {
            let r = FrameRate::COMMON[ri];
            prop_assert_eq!(r.frame_at(r.tick_of(f)), f);
            prop_assert_eq!(r.frame_at(r.tick_of(f) + r.frame_duration() - Tick(1)), f);
        }

        #[test]
        fn arbitrary_frame_boundaries_roundtrip_and_interior_ticks_stay_in_their_frame(
            frame in -1_000_000i64..1_000_000,
            milli_fps in 1_000i64..1_000_000,
            phase in 1i64..1_000,
        ) {
            let rate = FrameRate::new(milli_fps, 1000);
            let start = rate.tick_of(frame);
            let next = rate.tick_of(frame + 1);
            prop_assert_eq!(rate.frame_at(start), frame);
            prop_assert_eq!(rate.frame_at(start - Tick(1)), frame - 1);
            prop_assert_eq!(rate.frame_at(next - Tick(1)), frame);
            let interior = start + (next - start).mul_ratio(phase, 1000);
            prop_assert!(interior > start && interior < next);
            prop_assert_eq!(rate.frame_at(interior), frame);
            prop_assert_eq!(rate.snap(interior), start);
        }

        #[test]
        fn drop_frame_bijection(f in 0i64..(24 * 107_892)) {
            let r = FrameRate::FPS_29_97;
            let (_, h, m, s, fr) = frames_to_fields(f, r, true);
            // dropped labels never appear
            prop_assert!(!(s == 0 && fr < 2 && m % 10 != 0));
            prop_assert_eq!(fields_to_frames(h, m, s, fr, r, true), f);
            let txt = format_timecode_frames(f, r, true);
            prop_assert_eq!(parse_timecode(&txt, r, true, 0).unwrap(), f);
        }

        #[test]
        fn ndf_parse_roundtrip(f in 0i64..10_000_000, ri in 0usize..11) {
            let r = FrameRate::COMMON[ri];
            let txt = format_timecode_frames(f, r, false);
            prop_assert_eq!(parse_timecode(&txt, r, false, 0).unwrap(), f);
        }
    }

    #[test]
    fn feet_and_frames() {
        // 35 mm: 16 frames per foot; 16 mm: 40.
        assert_eq!(format_feet_frames(0, 16), "0000+00");
        assert_eq!(format_feet_frames(16 * 12 + 7, 16), "0012+07");
        assert_eq!(format_feet_frames(95, 40), "0002+15");
        assert_eq!(format_feet_frames(-20, 16), "-0001+04");
        assert_eq!(parse_feet_frames("12+07", 16, 0), Ok(199));
        assert_eq!(parse_feet_frames("0012+07", 16, 0), Ok(199));
        assert_eq!(parse_feet_frames("2+15", 40, 0), Ok(95));
        assert_eq!(parse_feet_frames("-1+04", 16, 0), Ok(-20));
        assert_eq!(parse_feet_frames("1+20", 16, 0), Ok(36), "frames may overflow a foot");
        assert_eq!(parse_feet_frames("250", 16, 0), Ok(250));
        assert_eq!(parse_feet_frames("+10", 16, 100), Ok(110));
        assert_eq!(parse_feet_frames("-10", 16, 100), Ok(90));
        assert_eq!(parse_feet_frames("+", 16, 5), Ok(5));
        assert!(parse_feet_frames("1:2", 16, 0).is_err());
        assert!(parse_feet_frames("", 16, 0).is_err());
        for f in [0, 1, 15, 16, 17, 1234, 99_999] {
            for pf in [16, 40] {
                assert_eq!(parse_feet_frames(&format_feet_frames(f, pf), pf, 0), Ok(f));
            }
        }
    }
}
