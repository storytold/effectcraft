//! The bounds every project holds to (#788). After Effects limits compositions, solids and
//! imported images to 30,000 × 30,000 pixels; the Composition Settings commands already used that
//! limit, a frame-rate range and a motion-blur sample range. A project file is input like any
//! other: [`Project::from_json_repaired`](crate::Project::from_json_repaired) brings what it reads
//! into these bounds (and says what it changed) and [`Project::add_item`](crate::Project::add_item)
//! does the same for every new item, so a corrupt or hostile file can't make the renderer allocate
//! a frame of billions of pixels or step through zero-length frames.

use std::fmt::Display;
use std::sync::Arc;

use effectcraft_time::FrameRate;

use crate::{Comp, Footage, Item, ItemKind, Solid};

/// The largest width or height of a composition, solid or footage frame, in pixels.
pub const MAX_SIZE: u32 = 30_000;
/// The slowest frame rate, in frames per second.
pub const MIN_FPS: f64 = 0.001;
/// The fastest frame rate, in frames per second.
pub const MAX_FPS: f64 = 1000.0;
/// The narrowest pixel aspect ratio.
pub const MIN_PIXEL_ASPECT: f64 = 0.1;
/// The widest pixel aspect ratio.
pub const MAX_PIXEL_ASPECT: f64 = 10.0;
/// Composition Settings ▸ Advanced ▸ Samples Per Frame.
pub const MOTION_BLUR_SAMPLES: (u32, u32) = (2, 64);
/// Composition Settings ▸ Advanced ▸ Adaptive Sample Limit.
pub const ADAPTIVE_SAMPLE_LIMIT: (u32, u32) = (16, 256);
/// The smallest time stretch, in percent, either way (After Effects sets a stretch between 0 and
/// 1 % to 1 %, between -1 % and 0 to -1 %; `layer.timeStretch` refuses less).
pub const MIN_STRETCH: f64 = 1.0;
/// The largest time stretch, in percent, either way (After Effects' `Layer.stretch` range is
/// -9900 to 9900).
pub const MAX_STRETCH: f64 = 9900.0;

/// A layer's time stretch from [`MIN_STRETCH`] to [`MAX_STRETCH`] % either way, its sign (a
/// reversed layer) kept; 100 % for 0 or NaN. Layer and comp time divide by the stretch, and the
/// audio mix reads `100 / stretch` source samples per sample, so a stretch of 1e-300 % or NaN
/// from a damaged file must not reach them (#3247).
pub fn stretch(v: f64) -> f64 {
    if v.is_nan() || v == 0.0 {
        return 100.0;
    }
    // Infinity clamps to the largest stretch with its sign.
    v.signum() * v.abs().clamp(MIN_STRETCH, MAX_STRETCH)
}

/// A stretch for a note: a plain number in a readable range, scientific notation beyond it (a
/// plain 1e-300 would print 300 digits).
fn percent(v: f64) -> String {
    if v == 0.0 || (1e-3..1e7).contains(&v.abs()) { format!("{v} %") } else { format!("{v:e} %") }
}

/// A width or height from 1 to [`MAX_SIZE`].
pub fn size(v: u64) -> u32 {
    // In range after the clamp, so the cast can't truncate.
    v.clamp(1, u64::from(MAX_SIZE)) as u32
}

/// [`size`] of a measured length (rounded; 1 for NaN).
pub fn size_f64(v: f64) -> u32 {
    if v.is_finite() { v.round().clamp(1.0, f64::from(MAX_SIZE)) as u32 } else { 1 }
}

/// A pixel aspect ratio from [`MIN_PIXEL_ASPECT`] to [`MAX_PIXEL_ASPECT`] (square for a value
/// that isn't a positive number).
pub fn pixel_aspect(v: f64) -> f64 {
    if v.is_finite() && v > 0.0 { v.clamp(MIN_PIXEL_ASPECT, MAX_PIXEL_ASPECT) } else { 1.0 }
}

/// A frame rate from [`MIN_FPS`] to [`MAX_FPS`]; `fallback` for a zero or negative rate.
pub fn frame_rate(r: FrameRate, fallback: FrameRate) -> FrameRate {
    if r.num <= 0 || r.den <= 0 {
        return fallback;
    }
    let fps = r.as_f64();
    if (MIN_FPS..=MAX_FPS).contains(&fps) { r } else { FrameRate::from_f64(fps.clamp(MIN_FPS, MAX_FPS)) }
}

/// Records one corrected value.
fn note(notes: &mut Vec<String>, item: &str, what: &str, from: impl Display, to: impl Display) {
    notes.push(format!("{item}: {what} {from} → {to}"));
}

fn fps(r: FrameRate) -> String {
    format!("{}/{} fps", r.num, r.den)
}

/// Brings a read item (and its proxy) within the limits; one line per corrected value is added
/// to `notes`.
pub(crate) fn repair_item(item: &mut Item, notes: &mut Vec<String>) {
    repair_kind(&item.name, &mut item.kind, notes);
    if let Some(proxy) = &mut item.proxy {
        repair_footage(&format!("{} (proxy)", item.name), &mut proxy.footage, notes);
    }
}

/// Brings `kind` within the limits; one line per corrected value is added to `notes`.
pub(crate) fn repair_kind(name: &str, kind: &mut ItemKind, notes: &mut Vec<String>) {
    match kind {
        ItemKind::Comp(c) => {
            if let Some(fixed) = repair_comp(name, c, notes) {
                *c = Arc::new(fixed);
            }
        }
        ItemKind::Solid(s) => repair_solid(name, s, notes),
        ItemKind::Footage(f) => repair_footage(name, f, notes),
        ItemKind::Folder => {}
    }
}

/// The comp with its settings within the limits, or `None` when they already are (so a
/// composition shared with undo snapshots is only copied when something changes).
fn repair_comp(name: &str, c: &Comp, notes: &mut Vec<String>) -> Option<Comp> {
    let width = c.width.clamp(1, MAX_SIZE);
    let height = c.height.clamp(1, MAX_SIZE);
    let pa = pixel_aspect(c.pixel_aspect);
    let rate = frame_rate(c.frame_rate, FrameRate::FPS_29_97);
    // At least one frame long.
    let duration = c.duration.max(rate.frame_duration());
    let samples = c.motion_blur_samples.clamp(MOTION_BLUR_SAMPLES.0, MOTION_BLUR_SAMPLES.1);
    let adaptive = c.motion_blur_adaptive_limit.clamp(ADAPTIVE_SAMPLE_LIMIT.0, ADAPTIVE_SAMPLE_LIMIT.1);
    let stretch_ok = |l: &crate::Layer| stretch(l.stretch).to_bits() == l.stretch.to_bits();
    let unchanged = (width, height, rate, duration, samples, adaptive)
        == (c.width, c.height, c.frame_rate, c.duration, c.motion_blur_samples, c.motion_blur_adaptive_limit)
        && pa.to_bits() == c.pixel_aspect.to_bits()
        && c.layers.iter().all(stretch_ok);
    if unchanged {
        return None;
    }
    let mut n = c.clone();
    if (width, height) != (c.width, c.height) {
        note(notes, name, "size", format_args!("{} × {} px", c.width, c.height), format_args!("{width} × {height} px"));
    }
    if pa.to_bits() != c.pixel_aspect.to_bits() {
        note(notes, name, "pixel aspect ratio", c.pixel_aspect, pa);
    }
    if rate != c.frame_rate {
        note(notes, name, "frame rate", fps(c.frame_rate), fps(rate));
    }
    if duration != c.duration {
        note(notes, name, "duration", format_args!("{:.3} s", c.duration.seconds()), format_args!("{:.3} s", duration.seconds()));
        n.work_area = (effectcraft_time::Tick::ZERO, duration);
    }
    if samples != c.motion_blur_samples {
        note(notes, name, "motion blur samples per frame", c.motion_blur_samples, samples);
    }
    if adaptive != c.motion_blur_adaptive_limit {
        note(notes, name, "adaptive sample limit", c.motion_blur_adaptive_limit, adaptive);
    }
    (n.width, n.height, n.pixel_aspect, n.frame_rate, n.duration) = (width, height, pa, rate, duration);
    (n.motion_blur_samples, n.motion_blur_adaptive_limit) = (samples, adaptive);
    // Only the layers out of range are copied (the others stay shared).
    for l in n.layers.iter_mut_where(|l| !stretch_ok(l)) {
        let fixed = stretch(l.stretch);
        note(notes, name, &format!("layer '{}' time stretch", l.name), percent(l.stretch), percent(fixed));
        l.stretch = fixed;
    }
    Some(n)
}

fn repair_solid(name: &str, s: &mut Solid, notes: &mut Vec<String>) {
    let (w, h) = (s.width.clamp(1, MAX_SIZE), s.height.clamp(1, MAX_SIZE));
    if (w, h) != (s.width, s.height) {
        note(notes, name, "size", format_args!("{} × {} px", s.width, s.height), format_args!("{w} × {h} px"));
        (s.width, s.height) = (w, h);
    }
    let pa = pixel_aspect(s.pixel_aspect);
    if pa.to_bits() != s.pixel_aspect.to_bits() {
        note(notes, name, "pixel aspect ratio", s.pixel_aspect, pa);
        s.pixel_aspect = pa;
    }
}

fn repair_footage(name: &str, f: &mut Footage, notes: &mut Vec<String>) {
    // Audio-only footage has no frame (0 × 0).
    let (w, h) = (f.width.min(MAX_SIZE), f.height.min(MAX_SIZE));
    if (w, h) != (f.width, f.height) {
        note(notes, name, "size", format_args!("{} × {} px", f.width, f.height), format_args!("{w} × {h} px"));
        (f.width, f.height) = (w, h);
    }
    let pa = pixel_aspect(f.pixel_aspect);
    if pa.to_bits() != f.pixel_aspect.to_bits() {
        note(notes, name, "pixel aspect ratio", f.pixel_aspect, pa);
        f.pixel_aspect = pa;
    }
    let rate = frame_rate(f.frame_rate, FrameRate::FPS_30);
    if rate != f.frame_rate {
        note(notes, name, "frame rate", fps(f.frame_rate), fps(rate));
        f.frame_rate = rate;
    }
    if let Some(native) = f.native_rate {
        let rate = frame_rate(native, FrameRate::FPS_30);
        if rate != native {
            note(notes, name, "native frame rate", fps(native), fps(rate));
            f.native_rate = Some(rate);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use effectcraft_time::Tick;

    #[test]
    fn helpers_clamp_and_refuse_nonsense() {
        assert_eq!(size(0), 1);
        assert_eq!(size(u64::from(u32::MAX) + 5), MAX_SIZE);
        assert_eq!(size_f64(f64::NAN), 1);
        assert_eq!(size_f64(1e12), MAX_SIZE);
        assert_eq!(pixel_aspect(0.0), 1.0);
        assert_eq!(pixel_aspect(-2.0), 1.0);
        assert_eq!(pixel_aspect(1e9), MAX_PIXEL_ASPECT);
        assert_eq!(frame_rate(FrameRate { num: 0, den: 1 }, FrameRate::FPS_25), FrameRate::FPS_25);
        assert_eq!(frame_rate(FrameRate { num: 24, den: -1 }, FrameRate::FPS_25), FrameRate::FPS_25);
        assert_eq!(frame_rate(FrameRate::new(1_000_000_000_000_000_000, 1), FrameRate::FPS_25).as_f64(), MAX_FPS);
        assert_eq!(frame_rate(FrameRate::FPS_29_97, FrameRate::FPS_25), FrameRate::FPS_29_97);
    }

    #[test]
    fn add_item_and_reading_a_file_hold_footage_to_the_limits() {
        let mut p = crate::Project::default();
        let wild = Footage { width: u32::MAX, height: 0, pixel_aspect: 0.0, frame_rate: FrameRate { num: 1, den: 0 }, ..Default::default() };
        let id = p.add_item("Clip", effectcraft_color::Label::None, None, ItemKind::Footage(wild));
        let ItemKind::Footage(f) = &p.items[&id].kind else { panic!("footage") };
        assert_eq!((f.width, f.height, f.pixel_aspect, f.frame_rate), (MAX_SIZE, 0, 1.0, FrameRate::FPS_30));
        // The same values in a file are corrected, and each correction is reported.
        let mut v: serde_json::Value = serde_json::from_str(&p.to_json()).unwrap();
        let kind = &mut v["items"][id.0.to_string()]["kind"];
        kind["width"] = serde_json::json!(4_000_000_000u64);
        kind["native_rate"] = serde_json::json!({"num": -24, "den": 1});
        let mut proxy = kind.clone();
        proxy["frame_rate"] = serde_json::json!({"num": 0, "den": 0});
        v["items"][id.0.to_string()]["proxy"] = serde_json::json!({"footage": proxy});
        let (back, notes) = crate::Project::from_json_repaired(&v.to_string()).unwrap();
        let ItemKind::Footage(b) = &back.items[&id].kind else { panic!("footage") };
        assert_eq!((b.width, b.native_rate), (MAX_SIZE, Some(FrameRate::FPS_30)));
        assert_eq!(
            notes,
            [
                "Clip: size 4000000000 × 0 px → 30000 × 0 px",
                "Clip: native frame rate -24/1 fps → 30/1 fps",
                "Clip (proxy): size 4000000000 × 0 px → 30000 × 0 px",
                "Clip (proxy): frame rate 0/0 fps → 30/1 fps",
                "Clip (proxy): native frame rate -24/1 fps → 30/1 fps",
            ]
        );
    }

    #[test]
    fn a_stretch_is_held_to_one_to_9900_percent_with_its_sign() {
        assert_eq!(stretch(f64::NAN), 100.0);
        assert_eq!(stretch(0.0), 100.0);
        assert_eq!(stretch(-0.0), 100.0);
        assert_eq!(stretch(1e-300), MIN_STRETCH);
        assert_eq!(stretch(-1e-300), -MIN_STRETCH);
        assert_eq!(stretch(0.5), MIN_STRETCH);
        assert_eq!(stretch(1e300), MAX_STRETCH);
        assert_eq!(stretch(f64::INFINITY), MAX_STRETCH);
        assert_eq!(stretch(f64::NEG_INFINITY), -MAX_STRETCH);
        for v in [1.0, -1.0, 33.3, 100.0, -100.0, 200.0, 9900.0, -9900.0] {
            assert_eq!(stretch(v).to_bits(), v.to_bits(), "{v} is in range");
        }
    }

    /// A layer whose stretch got past loading (0, NaN, 1e-300 %, set in memory) still maps time
    /// through a stretch in range: before, 0 and 1e-300 % saturated every layer time to the end of
    /// time and NaN collapsed it to 0 (#3247).
    #[test]
    fn layer_and_comp_time_use_a_stretch_in_range() {
        let mut p = crate::Project::default();
        let comp = Comp::new(64, 64, FrameRate::FPS_25, Tick::from_seconds_f64(10.0));
        let mut l = crate::build::layer(&mut p, &comp, "L", crate::LayerSource::Camera, (64, 64), None);
        l.start_time = Tick::from_seconds_f64(1.0);
        let at = |s: f64| Tick::from_seconds_f64(s);
        for (bad, effective) in [(0.0, 100.0), (f64::NAN, 100.0), (1e-300, 1.0), (-1e-300, -1.0), (1e300, 9900.0), (f64::NEG_INFINITY, -9900.0)] {
            l.stretch = bad;
            assert_eq!(l.effective_stretch(), effective, "{bad}");
            // One comp second after the start is 100 / stretch layer seconds.
            let want = Tick((at(1.0).0 as f64 * 100.0 / effective) as i64);
            assert_eq!(l.layer_time(at(2.0)), want, "{bad}");
            // And back, within the rounding of the two conversions.
            assert!((l.comp_time(l.layer_time(at(2.0))) - at(2.0)).abs() <= Tick(100), "{bad}");
        }
    }

    #[test]
    fn a_stretch_out_of_range_in_a_file_is_corrected_and_reported() {
        let mut p = crate::Project::default();
        let comp = Comp::new(64, 64, FrameRate::FPS_25, Tick::from_seconds_f64(2.0));
        let mut c = comp.clone();
        for (name, s) in [("tiny", 1e-300), ("reversed tiny", -1e-300), ("zero", 0.0), ("huge", 1e300), ("reversed huge", -5e4), ("fine", -50.0)] {
            let mut l = crate::build::layer(&mut p, &comp, name, crate::LayerSource::Camera, (64, 64), None);
            l.stretch = s;
            c.layers.push(l);
        }
        // Added through `add_item`, the comp is held to the limits too: write the values back in.
        let id = p.add_item("Comp", effectcraft_color::Label::None, None, ItemKind::Comp(Arc::new(c.clone())));
        assert!(p.comp(id).unwrap().layers.iter().all(|l| l.stretch.abs() >= MIN_STRETCH && l.stretch.abs() <= MAX_STRETCH));
        let mut v: serde_json::Value = serde_json::from_str(&p.to_json()).unwrap();
        for (k, l) in c.layers.iter().enumerate() {
            v["items"][id.0.to_string()]["kind"]["layers"][k]["stretch"] = serde_json::json!(l.stretch);
        }
        let (back, notes) = crate::Project::from_json_repaired(&v.to_string()).unwrap();
        let got: Vec<f64> = back.comp(id).unwrap().layers.iter().map(|l| l.stretch).collect();
        assert_eq!(got, [1.0, -1.0, 100.0, 9900.0, -9900.0, -50.0]);
        assert_eq!(
            notes,
            [
                "Comp: layer 'tiny' time stretch 1e-300 % → 1 %",
                "Comp: layer 'reversed tiny' time stretch -1e-300 % → -1 %",
                "Comp: layer 'zero' time stretch 0 % → 100 %",
                "Comp: layer 'huge' time stretch 1e300 % → 9900 %",
                "Comp: layer 'reversed huge' time stretch -50000 % → -9900 %",
            ]
        );
        // A NaN stretch is written as null (an auto-save): the file opens, at 100 %, and says so.
        // Before, the whole project was refused.
        v["items"][id.0.to_string()]["kind"]["layers"][0]["stretch"] = serde_json::Value::Null;
        let (back, notes) = crate::Project::from_json_repaired(&v.to_string()).unwrap();
        assert_eq!(back.comp(id).unwrap().layers.iter().next().unwrap().stretch, 100.0);
        assert_eq!(notes.first().map(String::as_str), Some("Comp: layer 'tiny' time stretch NaN % → 100 %"));
        let mut nan = p.clone();
        nan.comp_mut(id).unwrap().layers.iter_mut().for_each(|l| l.stretch = f64::NAN);
        let (back, _) = crate::Project::from_json_repaired(&nan.to_json_compact()).unwrap();
        assert!(back.comp(id).unwrap().layers.iter().all(|l| l.stretch == 100.0));
    }

    #[test]
    fn a_comp_in_range_is_not_copied() {
        let c = Comp::new(1920, 1080, FrameRate::FPS_24, Tick::from_seconds_f64(2.0));
        let mut notes = vec![];
        assert!(repair_comp("Comp", &c, &mut notes).is_none());
        assert!(notes.is_empty());
    }
}
