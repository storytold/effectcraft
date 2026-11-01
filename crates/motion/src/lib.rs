//! Apple Motion (OZML) import and export.
//!
//! Motion projects (`.motn`) and the templates Final Cut Pro uses (titles `.moti`, transitions
//! `.motr`, effects `.moef`, generators `.mogn`) are XML documents in Motion's OZML structure.
//! This crate is implemented from Apple's public *Motion XML Guide* (element and channel
//! reference, curve and keypoint enumerations) and from observing files Motion writes; no Motion
//! code or assets were used.
//!
//! **Import** ([`import`]) turns the scene into a new composition. Each group (`layer` / `group`
//! element) becomes a nested composition the size of the scene; image objects become footage
//! layers (the caller resolves the clip files, see [`clip_refs`]), Color Solid generators become
//! solids and text objects become text layers. Transform (position, rotation, scale, anchor
//! point) and opacity come across with their keyframes (constant, linear and Bezier keypoints;
//! Bezier tangents become speed/influence ease). Filters applied to a group become adjustment
//! layers at the top of the group's composition, active for the filter's own time range; a few
//! filters have an equivalent effect (see `filters.rs`). Behaviors, rigs, cameras, lights,
//! shapes, emitters and replicators are listed in [`ImportResult::warnings`].
//!
//! **Export** ([`export_comp`]) writes a composition as a Motion project: nested compositions
//! become groups, solids become Color Solid generators, footage becomes image objects (with the
//! clip in the media list), and transforms and opacity keep their keyframes. Anything Motion
//! cannot take from us is listed in [`ExportResult::warnings`].
//!
//! Coordinates: Motion puts the origin at the centre of the canvas with Y up, angles are radians
//! counter-clockwise, scale and opacity are fractions. EffectCraft uses top-left pixels with Y
//! down, degrees clockwise and percentages. Keypoint times are seconds (`value timescale flags
//! epoch` in Motion 5, frames in older files) relative to the object's start, which is
//! EffectCraft's layer time.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

mod curve;
mod export;
mod filters;
mod import;
mod xml;

use effectcraft_time::{FrameRate, TICKS_PER_SECOND, Tick};

pub use export::{ExportResult, export_comp};
pub use import::{ClipRef, ImportResult, clip_refs, import};

#[derive(Debug, thiserror::Error)]
pub enum MotionError {
    #[error("not a Motion document: {0}")]
    Parse(String),
    #[error("{0}")]
    Invalid(String),
}

/// File extensions of Motion documents (projects and Final Cut Pro templates).
pub const EXTENSIONS: &[&str] = &["motn", "motr", "moti", "moef", "mogn"];

/// The timescale of the times we write (exact for 23.976, 24, 25, 29.97, 30, 50, 59.94 and 60).
const TIMESCALE: i128 = 120_000;

/// Deepest group nesting read or written (guards hostile or cyclic documents).
const MAX_DEPTH: usize = 64;

/// A Motion time: `value timescale flags epoch` (seconds = value / timescale), or a plain frame
/// number in files from Motion 4 and earlier.
pub(crate) fn parse_time(s: &str, rate: FrameRate) -> Option<Tick> {
    let mut it = s.split_whitespace();
    let first = it.next()?;
    match it.next() {
        Some(ts) => {
            let v: i128 = first.parse().ok()?;
            let ts: i128 = ts.parse().ok()?;
            if ts <= 0 {
                return None;
            }
            let t = v.checked_mul(TICKS_PER_SECOND as i128)?.checked_div(ts)?;
            i64::try_from(t).ok().map(Tick)
        }
        None => {
            let f: f64 = first.parse().ok()?;
            f.is_finite().then(|| Tick::from_seconds_f64(f * rate.den as f64 / rate.num.max(1) as f64))
        }
    }
}

/// A time as Motion 5 writes it.
pub(crate) fn fmt_time(t: Tick) -> String {
    let v = (t.0 as i128 * TIMESCALE + TICKS_PER_SECOND as i128 / 2).div_euclid(TICKS_PER_SECOND as i128);
    format!("{v} {TIMESCALE} 1 0")
}

/// The scene frame rate from `frameRate` and the `NTSC` flag (30 + NTSC = 29.97).
pub(crate) fn scene_rate(fps: f64, ntsc: bool) -> FrameRate {
    // Motion scenes run at 1 to a few hundred fps; anything else is corrupt or hostile (and
    // would overflow the NTSC numerator or round to a zero rate).
    if !fps.is_finite() || !(1.0..=1000.0).contains(&fps) {
        return FrameRate::FPS_30;
    }
    let whole = fps.round();
    if ntsc && (fps - whole).abs() < 1e-6 && whole >= 1.0 {
        return FrameRate::new(whole as i64 * 1000, 1001);
    }
    FrameRate::from_f64(fps)
}

/// `frameRate` and `NTSC` of a rate (29.97 → 30 + NTSC).
pub(crate) fn rate_fields(rate: FrameRate) -> (f64, bool) {
    if rate.den == 1001 && rate.num % 1000 == 0 { ((rate.num / 1000) as f64, true) } else { (rate.as_f64(), false) }
}

#[cfg(test)]
mod tests;
