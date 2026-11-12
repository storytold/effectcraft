//! Text effects (Numbers, Timecode) and the obsolete generators Basic Text, Path Text and
//! Lightning.
//!
//! Effects sit below the text engine in the crate layering, so text here is drawn with a small
//! built-in single-stroke font (original work: capitals, digits and common punctuation laid out
//! on a 4 × 6 unit grid; lowercase letters use the capitals) and rasterised as anti-aliased round
//! strokes.

use std::f64::consts::PI;

use effectcraft_keyframe::Value;
use effectcraft_project::ParamUi;
use effectcraft_raster::{Image, Px};
use rayon::prelude::*;

use crate::generate3::{Seg, mask_px, poly_segs, raster_segs};
use crate::util::{Plane, gauss_plane, hash1, poly_length, poly_point_at};
use crate::{Buf, EffectCtx, EffectSpec, col, num, p, popup, slider};

fn spec(id: &'static str, name: &'static str, category: &'static str, params: Vec<crate::ParamSpec>, render: crate::RenderFn) -> EffectSpec {
    EffectSpec { id, name, category, params, render, gpu: false, float: true }
}

fn pct() -> ParamUi {
    slider(0.0, 100.0, 0.0, 100.0, 1)
}

// ---------------------------------------------------------------- stroke font

/// Glyph strokes on a grid x ∈ [0, 4], y ∈ [0, 6] (y down, baseline at 6): polylines separated
/// by `;`, points by spaces. A single point is a dot.
fn glyph_src(c: char) -> &'static str {
    match c.to_ascii_uppercase() {
        '0' => "1,0 3,0 4,1 4,5 3,6 1,6 0,5 0,1 1,0; 3.6,0.6 0.4,5.4",
        '1' => "1,1 2,0 2,6; 1,6 3,6",
        '2' => "0,1 1,0 3,0 4,1 4,2 0,6 4,6",
        '3' => "0,0 4,0 2,2.5 3,2.5 4,3.5 4,5 3,6 1,6 0,5",
        '4' => "3,6 3,0 0,4 4,4",
        '5' => "4,0 0,0 0,2.5 3,2.5 4,3.5 4,5 3,6 0,6",
        '6' => "3.5,0 1,0 0,1 0,5 1,6 3,6 4,5 4,3.5 3,2.5 0,2.5",
        '7' => "0,0 4,0 1.5,6",
        '8' => "1,0 3,0 4,1 4,2 3,3 1,3 0,2 0,1 1,0; 1,3 0,4 0,5 1,6 3,6 4,5 4,4 3,3",
        '9' => "4,3.5 1,3.5 0,2.5 0,1 1,0 3,0 4,1 4,5 3,6 0.5,6",
        'A' => "0,6 2,0 4,6; 0.7,4 3.3,4",
        'B' => "0,0 0,6 3,6 4,5 4,4 3,3 0,3; 0,0 3,0 4,1 4,2 3,3",
        'C' => "4,1 3,0 1,0 0,1 0,5 1,6 3,6 4,5",
        'D' => "0,0 0,6 2.5,6 4,4.5 4,1.5 2.5,0 0,0",
        'E' => "4,0 0,0 0,6 4,6; 0,3 3,3",
        'F' => "4,0 0,0 0,6; 0,3 3,3",
        'G' => "4,1 3,0 1,0 0,1 0,5 1,6 3,6 4,5 4,3.5 2.5,3.5",
        'H' => "0,0 0,6; 4,0 4,6; 0,3 4,3",
        'I' => "1,0 3,0; 2,0 2,6; 1,6 3,6",
        'J' => "4,0 4,5 3,6 1,6 0,5",
        'K' => "0,0 0,6; 4,0 0,4; 1.3,3 4,6",
        'L' => "0,0 0,6 4,6",
        'M' => "0,6 0,0 2,3 4,0 4,6",
        'N' => "0,6 0,0 4,6 4,0",
        'O' => "1,0 3,0 4,1 4,5 3,6 1,6 0,5 0,1 1,0",
        'P' => "0,6 0,0 3,0 4,1 4,2 3,3 0,3",
        'Q' => "1,0 3,0 4,1 4,5 3,6 1,6 0,5 0,1 1,0; 2.5,4.5 4,6.3",
        'R' => "0,6 0,0 3,0 4,1 4,2 3,3 0,3; 2,3 4,6",
        'S' => "4,1 3,0 1,0 0,1 0,2 1,3 3,3 4,4 4,5 3,6 1,6 0,5",
        'T' => "0,0 4,0; 2,0 2,6",
        'U' => "0,0 0,5 1,6 3,6 4,5 4,0",
        'V' => "0,0 2,6 4,0",
        'W' => "0,0 1,6 2,2 3,6 4,0",
        'X' => "0,0 4,6; 4,0 0,6",
        'Y' => "0,0 2,3 4,0; 2,3 2,6",
        'Z' => "0,0 4,0 0,6 4,6",
        ':' => "2,2; 2,5",
        ';' => "2,2; 2,5 1.5,6.8",
        '.' => "2,6",
        ',' => "2,5.6 1.5,6.8",
        '-' => "1,3.5 3,3.5",
        '+' => "0.5,3.5 3.5,3.5; 2,2 2,5",
        '/' => "4,0 0,6",
        '(' => "2.5,-0.3 1.5,1.2 1.5,4.8 2.5,6.3",
        ')' => "1.5,-0.3 2.5,1.2 2.5,4.8 1.5,6.3",
        '\'' => "2,0 2,1.5",
        '"' => "1.4,0 1.4,1.5; 2.6,0 2.6,1.5",
        '!' => "2,0 2,4.2; 2,6",
        '?' => "0,1 1,0 3,0 4,1 4,2 2,3.5 2,4.4; 2,6",
        '%' => "0,6 4,0; 0.6,0.6; 3.4,5.4",
        '=' => "0.5,2.5 3.5,2.5; 0.5,4.5 3.5,4.5",
        '#' => "1.3,0.5 1,5.5; 3,0.5 2.7,5.5; 0,2 4,2; 0,4 4,4",
        '&' => "4,6 1,2 1,0.8 1.8,0 2.8,0.8 2.8,1.8 0,4 0,5.2 1,6 2.5,6 4,4",
        '*' => "2,1 2,4; 0.7,1.8 3.3,3.2; 3.3,1.8 0.7,3.2",
        '_' => "0,6.5 4,6.5",
        '<' => "4,1 0,3.5 4,6",
        '>' => "0,1 4,3.5 0,6",
        '@' => "3,4 3,2 1.8,2 1.2,3 1.8,4 3,4 4,3.5 4,1 3,0 1,0 0,1 0,5 1,6 3.5,6",
        '$' => "4,1 3,0.5 1,0.5 0,1.5 1,3 3,3 4,4.5 3,5.5 1,5.5 0,5; 2,-0.3 2,6.3",
        _ => "",
    }
}

/// A glyph's strokes on the 4 × 6 grid (Particle Playground's text particles).
pub(crate) fn glyph_strokes(c: char) -> Vec<Vec<[f64; 2]>> {
    parse_glyph(c)
}

fn parse_glyph(c: char) -> Vec<Vec<[f64; 2]>> {
    glyph_src(c)
        .split(';')
        .map(|s| {
            s.split_whitespace()
                .filter_map(|pt| {
                    let (x, y) = pt.split_once(',')?;
                    Some([x.parse().ok()?, y.parse().ok()?])
                })
                .collect::<Vec<[f64; 2]>>()
        })
        .filter(|v| !v.is_empty())
        .collect()
}

/// Advance width in grid units (glyph ink + 1.6 units of side bearing).
fn advance(c: char, proportional: bool) -> f64 {
    if c == ' ' {
        return 3.0;
    }
    if !proportional {
        return 5.6;
    }
    let g = parse_glyph(c);
    let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
    for s in &g {
        for q in s {
            lo = lo.min(q[0]);
            hi = hi.max(q[0]);
        }
    }
    if lo > hi { 3.0 } else { (hi - lo) + 1.6 }
}

/// Left ink edge of a glyph (so proportional glyphs start at their ink).
fn ink_left(c: char, proportional: bool) -> f64 {
    if !proportional {
        return 0.8;
    }
    parse_glyph(c).iter().flatten().map(|q| q[0]).fold(f64::INFINITY, f64::min).min(4.0) - 0.8
}

/// Text metrics: grid unit in px for a font size (cap height ≈ 0.7 em) and stroke radius.
fn unit(size: f64) -> f64 {
    size * 0.7 / 6.0
}
fn weight(size: f64) -> f64 {
    (size * 0.045).max(0.35)
}

/// Width of a line of text in px (tracking in px between characters).
fn line_width(text: &str, size: f64, tracking: f64, proportional: bool) -> f64 {
    let u = unit(size);
    let n = text.chars().count();
    if n == 0 {
        return 0.0;
    }
    text.chars().map(|c| advance(c, proportional) * u).sum::<f64>() + tracking * (n as f64 - 1.0) - 1.6 * u
}

/// Polylines (px) of a line of text whose baseline starts at `origin`.
fn layout_line(text: &str, origin: [f64; 2], size: f64, tracking: f64, proportional: bool, out: &mut Vec<Vec<[f64; 2]>>) {
    let u = unit(size);
    let mut x = origin[0];
    for c in text.chars() {
        let left = ink_left(c, proportional);
        for s in parse_glyph(c) {
            out.push(s.iter().map(|q| [x + (q[0] - left - 0.8) * u, origin[1] + (q[1] - 6.0) * u]).collect());
        }
        x += advance(c, proportional) * u + tracking;
    }
}

/// How text paints: fill/stroke display option, colours, and compositing.
/// How text is drawn: Display Options (0 fill only, 1 stroke only, 2 fill over stroke, 3 stroke
/// over fill), colours, stroke width (buffer px), on the layer or on transparency, opacity.
#[derive(Clone, Copy, Debug)]
pub struct TextLook {
    pub display: u32,
    pub fill: [f32; 4],
    pub stroke: [f32; 4],
    pub stroke_w: f64,
    pub on_original: bool,
    pub opacity: f32,
}

/// One drawing pass of Basic Text / Path Text, shared with the GPU compositor (effectcraft-gpu
/// `fx_gen2`): glyph stroke polylines (buffer px) of radius `r`, drawn with `look` over the
/// result of the previous pass.
#[derive(Clone, Debug)]
pub struct TextPass {
    pub polys: Vec<Vec<[f64; 2]>>,
    pub r: f64,
    pub look: TextLook,
}

/// The [`TextPass`]es of Basic Text or Path Text on `b`'s geometry.
pub fn text_passes(id: &str, ctx: &EffectCtx, b: &Buf) -> Option<Vec<TextPass>> {
    match id {
        "ec.obsolete.basictext" => Some(vec![basic_text_pass(ctx, b)]),
        "ec.obsolete.pathtext" => Some(path_text_passes(ctx, b)),
        _ => None,
    }
}

fn over(dst: Px, src: Px) -> Px {
    let k = 1.0 - src[3];
    [src[0] + dst[0] * k, src[1] + dst[1] * k, src[2] + dst[2] * k, src[3] + dst[3] * k]
}

fn tint(c: [f32; 4], a: f32) -> Px {
    let a = (a * c[3]).clamp(0.0, 1.0);
    [c[0] * a, c[1] * a, c[2] * a, a]
}

/// Coverage of text polylines (`r` = glyph stroke radius in px): the fill and, when the look
/// strokes, the stroke ring.
fn text_coverage(w: usize, h: usize, polys: &[Vec<[f64; 2]>], r: f64, look: &TextLook) -> (Plane, Option<Plane>) {
    let segs_at = |rad: f64| {
        let mut s = Vec::new();
        for pl in polys {
            poly_segs(pl, false, rad.max(0.05), 1.0, &mut s);
        }
        s
    };
    let fill = raster_segs(w, h, &segs_at(r), 1.0);
    let sw = look.stroke_w.max(0.0);
    let ring = if look.display == 0 || sw <= 0.0 {
        None
    } else {
        let outer = raster_segs(w, h, &segs_at(r + sw * 0.5), 1.0);
        let inner = if r - sw * 0.5 > 0.05 { raster_segs(w, h, &segs_at(r - sw * 0.5), 1.0) } else { Plane::new(w, h) };
        Some(outer.zip_map(&inner, |a, b| (a - b).max(0.0)))
    };
    (fill, ring)
}

/// One pixel of drawn text: `px` under the fill / stroke coverages `f`, `s` (opacity applied).
fn text_px(px: Px, f: f32, s: f32, look: &TextLook) -> Px {
    let mut out = if look.on_original { px } else { [0.0; 4] };
    match look.display {
        1 => out = over(out, tint(look.stroke, s)),
        2 => {
            out = over(out, tint(look.stroke, s));
            out = over(out, tint(look.fill, f));
        }
        3 => {
            out = over(out, tint(look.fill, f));
            out = over(out, tint(look.stroke, s));
        }
        _ => out = over(out, tint(look.fill, f)),
    }
    out
}

/// Draw text polylines with the given look (`r` = glyph stroke radius in px).
fn draw_text(b: &mut Buf, polys: &[Vec<[f64; 2]>], r: f64, look: &TextLook) {
    let (fill, ring) = text_coverage(b.img.width as usize, b.img.height as usize, polys, r, look);
    let op = look.opacity;
    b.img.data.par_iter_mut().enumerate().for_each(|(i, px)| {
        let f = fill.data[i] * op;
        let s = ring.as_ref().map(|r| r.data[i]).unwrap_or(0.0) * op;
        *px = text_px(*px, f, s, look);
    });
}

/// Parameter `id`, read from its `group` twirl-down when the effect nests it there.
fn grouped<'a>(pr: &'a crate::Params, group: &str, id: &str) -> Option<&'a Value> {
    pr.get(&format!("{group}/{id}")).or_else(|| pr.get(id))
}

fn look_from(ctx: &EffectCtx, b: &Buf, on_original_id: &str) -> TextLook {
    let pr = ctx.params;
    let fs = |id: &str| grouped(pr, "fillAndStroke", id);
    TextLook {
        display: fs("displayOptions").map(Value::as_enum).unwrap_or(0),
        fill: fs("fillColor").map(Value::as_color).unwrap_or([1.0; 4]),
        stroke: fs("strokeColor").map(Value::as_color).unwrap_or([1.0; 4]),
        stroke_w: fs("strokeWidth").map(Value::as_f64).unwrap_or(0.0) * b.scale,
        on_original: pr.b(on_original_id),
        opacity: 1.0,
    }
}

// ---------------------------------------------------------------- Numbers

const MONTHS: [&str; 12] = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];
const DAYS: [&str; 7] = ["Saturday", "Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday"];

/// `num_days_from_ce` of 2000-01-01: the day count since 2000-01-01 (Numbers' date epoch, a
/// Saturday) rebased so that chrono's `num_days_from_ce` (0001-01-01 is day 1) can convert it.
const CE_FROM_2000: i64 = 730_120;

/// Days in one 400-year Gregorian cycle; the proleptic calendar repeats after it.
const DAYS_PER_400_YEARS: i64 = 146_097;

/// Proleptic Gregorian date (year, month, day, Saturday-indexed weekday) from a day count since
/// 2000-01-01 (a Saturday), for any `i64` count.
///
/// chrono's `NaiveDate` stops near ±262143, but Numbers' `value` slider is a plain scalar with
/// no range guard, so a keyframed count can run past that. The calendar repeats every 400 years,
/// so chrono converts the day within one cycle and the whole cycles are added back as years (via
/// i128 so `i64::MIN`/`MAX` can't overflow); the result matches the pre-chrono count for every
/// year. The weekday comes from the same resolved date, so it can never disagree with it.
fn date_from_days(days: i64) -> (i64, u32, u32, usize) {
    use chrono::Datelike;
    let from_ce = i128::from(days) + i128::from(CE_FROM_2000);
    let (cycles, rest) = (from_ce.div_euclid(i128::from(DAYS_PER_400_YEARS)), from_ce.rem_euclid(i128::from(DAYS_PER_400_YEARS)));
    // `rest` is 0..146_097, so it is always a valid `num_days_from_ce` day inside chrono's range.
    let date = i32::try_from(rest).ok().and_then(chrono::NaiveDate::from_num_days_from_ce_opt);
    // Unreachable fallback: every `rest` in 0..146_097 converts. 2000-01-01 keeps the epoch's date.
    let (y, m, d, wd) = match date {
        Some(date) => {
            let year = i128::from(date.year()) + cycles * 400;
            let year = i64::try_from(year).unwrap_or(if year < 0 { i64::MIN } else { i64::MAX });
            // DAYS is Saturday-indexed; chrono counts from Sunday.
            let wd = (date.weekday().num_days_from_sunday() as usize + 1) % 7;
            (year, date.month(), date.day(), wd)
        }
        None => (2000, 1, 1, 0),
    };
    (y, m, d, wd)
}

fn timecode(frames: i64, fps: f64) -> String {
    let fr = fps.round().max(1.0) as i64;
    let neg = frames < 0;
    let f = frames.abs();
    let s = format!("{:02}:{:02}:{:02}:{:02}", f / (fr * 3600), (f / (fr * 60)) % 60, (f / fr) % 60, f % fr);
    if neg { format!("-{s}") } else { s }
}

/// Numbers' twirl-down groups (Format, Fill and Stroke).
const NUMBERS_GROUPS: [&str; 2] = ["format/", "fillAndStroke/"];

/// `params` with Numbers' group prefixes stripped (`format/value` → `value`), so the drawing
/// helpers shared with Basic Text read plain ids. Plain keys already present win.
fn ungroup(params: &crate::Params) -> crate::Params {
    let mut out = params.clone();
    for (k, v) in &params.values {
        if let Some(leaf) = NUMBERS_GROUPS.iter().find_map(|g| k.strip_prefix(g)) {
            out.values.entry(leaf.to_string()).or_insert_with(|| v.clone());
        }
    }
    out
}

fn numbers_text(ctx: &EffectCtx) -> String {
    let flat = ungroup(ctx.params);
    let pr = &flat;
    let kind = pr.e("type");
    let dp = pr.f("decimalPlaces").round().clamp(0.0, 10.0) as usize;
    let mut v = pr.f("value");
    if pr.b("randomValues") {
        let frame = (ctx.time * ctx.fps()).floor() as i64 as u32;
        v = hash1(frame, 17, ctx.seed) as f64 * pr.f("value");
    }
    let cur = pr.b("currentTimeDate");
    let t = if cur { ctx.time + v } else { v };
    match kind {
        1 => {
            let s = format!("{:.*}", dp, v.abs());
            let (ip, fp) = s.split_once('.').map(|(a, b)| (a.to_string(), format!(".{b}"))).unwrap_or((s.clone(), String::new()));
            format!("{}{:0>5}{}", if v < 0.0 { "-" } else { "" }, ip, fp)
        }
        2..=4 => {
            let fps = [30.0, 25.0, 24.0][(kind - 2) as usize];
            timecode((t * fps + 1e-6).floor() as i64, fps)
        }
        5 => {
            let s = t.floor() as i64;
            let a = s.abs();
            format!("{}{:02}:{:02}:{:02}", if s < 0 { "-" } else { "" }, a / 3600, (a / 60) % 60, a % 60)
        }
        6..=8 => {
            let (y, m, d, wd) = date_from_days(v.floor() as i64);
            match kind {
                6 => format!("{:02}/{:02}/{:02}", m, d, y.rem_euclid(100)),
                7 => format!("{} {}, {}", &MONTHS[m as usize - 1][..3], d, y),
                _ => format!("{}, {} {}, {}", DAYS[wd], MONTHS[m as usize - 1], d, y),
            }
        }
        9 => {
            let i = v.round() as i64;
            if i < 0 { format!("-{:X}", -i) } else { format!("{i:X}") }
        }
        _ => format!("{:.*}", dp, if cur { t } else { v }),
    }
}

/// What Numbers or Timecode draws: glyph polylines, stroke radius and look, whether the layer
/// stays under the text, and Timecode's box (`[x0, x1, y0, y1]` in buffer pixels, colour,
/// opacity × colour alpha).
struct TextPlan {
    polys: Vec<Vec<[f64; 2]>>,
    r: f64,
    look: TextLook,
    keep: bool,
    boxed: Option<([f64; 4], [f32; 4], f32)>,
}

fn numbers_plan(ctx: &EffectCtx, b: &Buf) -> TextPlan {
    let flat = ungroup(ctx.params);
    let ctx = &EffectCtx { params: &flat, time: ctx.time, layer_size: ctx.layer_size, seed: ctx.seed, adjustment: ctx.adjustment, env: ctx.env };
    let pr = ctx.params;
    let text = numbers_text(ctx);
    let size = (pr.f("size") * b.scale).max(0.5);
    let tracking = pr.f("tracking") * b.scale;
    let prop = pr.b("proportionalSpacing");
    let pos = b.to_px(pr.v2("position"));
    let wdt = line_width(&text, size, tracking, prop);
    let mut polys = Vec::new();
    layout_line(&text, [pos.0 - wdt * 0.5, pos.1 + unit(size) * 3.0], size, tracking, prop, &mut polys);
    let mut look = look_from(ctx, b, "compositeOnOriginal");
    let keep = look.on_original;
    look.on_original = true;
    TextPlan { polys, r: weight(size), look, keep, boxed: None }
}

/// Timecode's box over pixel (`x`, `y`).
fn box_px(px: Px, x: usize, y: usize, (r, bc, a): ([f64; 4], [f32; 4], f32)) -> Px {
    let [x0, x1, y0, y1] = r;
    let fy = y as f64 + 0.5;
    let cy = ((fy - y0 + 0.5).clamp(0.0, 1.0) * (y1 - fy + 0.5).clamp(0.0, 1.0)) as f32;
    if cy <= 0.0 {
        return px;
    }
    let fx = x as f64 + 0.5;
    let cx = ((fx - x0 + 0.5).clamp(0.0, 1.0) * (x1 - fx + 0.5).clamp(0.0, 1.0)) as f32;
    let k = cx * cy * a;
    if k > 0.0 { over(px, [bc[0] * k, bc[1] * k, bc[2] * k, k]) } else { px }
}

fn draw_plan(mut b: Buf, plan: &TextPlan) -> Buf {
    let w = b.img.width as usize;
    let (fill, ring) = text_coverage(w, b.img.height as usize, &plan.polys, plan.r, &plan.look);
    let op = plan.look.opacity;
    b.img.data.par_iter_mut().enumerate().for_each(|(i, px)| {
        let mut o = if plan.keep { *px } else { [0.0; 4] };
        if let Some(bx) = plan.boxed {
            o = box_px(o, i % w.max(1), i / w.max(1), bx);
        }
        let f = fill.data[i] * op;
        let s = ring.as_ref().map(|r| r.data[i]).unwrap_or(0.0) * op;
        *px = text_px(o, f, s, &plan.look);
    });
    b
}

fn numbers(ctx: &EffectCtx, b: Buf) -> Buf {
    let plan = numbers_plan(ctx, &b);
    draw_plan(b, &plan)
}

/// Numbers and Timecode for the GPU compositor (effectcraft-gpu `fx_text`): the glyph
/// coverage is rasterised here (the fill in x, the stroke ring in y, opacity applied), the
/// kernel composites it.
#[derive(Clone, Debug)]
pub struct TextLayer {
    pub coverage: Image,
    /// Display Options (0 fill only, 1 stroke only, 2 fill over stroke, 3 stroke over fill).
    pub display: u32,
    pub fill: [f32; 4],
    pub stroke: [f32; 4],
    /// The layer stays under the text (else the text is drawn on transparency).
    pub keep: bool,
    /// Timecode's box: `[x0, x1, y0, y1]` (buffer pixels), colour, opacity × colour alpha.
    pub boxed: Option<([f64; 4], [f32; 4], f32)>,
}

/// [`TextLayer`] of Numbers (`ec.text.numbers`) or Timecode (`ec.text.timecode`) on a buffer
/// of `b`'s geometry.
pub fn text_layer(id: &str, ctx: &EffectCtx, b: &Buf) -> Option<TextLayer> {
    let plan = match id {
        "ec.text.numbers" => numbers_plan(ctx, b),
        "ec.text.timecode" => timecode_plan(ctx, b),
        _ => return None,
    };
    let (w, h) = (b.img.width as usize, b.img.height as usize);
    let (fill, ring) = text_coverage(w, h, &plan.polys, plan.r, &plan.look);
    let op = plan.look.opacity;
    let mut coverage = Image::new(b.img.width, b.img.height);
    coverage.data.par_iter_mut().enumerate().for_each(|(i, px)| {
        *px = [fill.data[i] * op, ring.as_ref().map(|r| r.data[i]).unwrap_or(0.0) * op, 0.0, 0.0];
    });
    Some(TextLayer { coverage, display: plan.look.display, fill: plan.look.fill, stroke: plan.look.stroke, keep: plan.keep, boxed: plan.boxed })
}

// ---------------------------------------------------------------- Timecode

/// SMPTE drop-frame label for nominal 30/60 fps rates.
fn drop_frame(frames: i64, nominal: i64) -> String {
    let drop = nominal / 15; // 2 at 30, 4 at 60
    let per_min = nominal * 60 - drop;
    let per_10 = per_min * 10 + drop;
    let f = frames.max(0);
    let d = f / per_10;
    let m = f % per_10;
    let adj = if m > drop { f + drop * 9 * d + drop * ((m - drop) / per_min) } else { f + drop * 9 * d };
    format!("{:02};{:02};{:02};{:02}", adj / (nominal * 3600), (adj / (nominal * 60)) % 60, (adj / nominal) % 60, adj % nominal)
}

fn timecode_plan(ctx: &EffectCtx, b: &Buf) -> TextPlan {
    let pr = ctx.params;
    let src = pr.e("timeSource");
    let (t, fps) = match src {
        1 => (ctx.env.comp_time, ctx.fps()),
        2 => (ctx.time, pr.f("timeUnits").max(1.0)),
        _ => (ctx.time, ctx.fps()),
    };
    let mut frame = (t * fps + 1e-6).floor() as i64;
    if src == 2 {
        frame += pr.f("startingFrame").round() as i64;
    }
    let nominal = fps.round() as i64;
    let text = match pr.e("displayFormat") {
        1 => format!("{frame}"),
        2 => format!("{}+{:02}", frame.div_euclid(16), frame.rem_euclid(16)),
        3 => format!("{}+{:02}", frame.div_euclid(40), frame.rem_euclid(40)),
        _ => {
            if pr.b("dropFrame") && (nominal == 30 || nominal == 60) {
                drop_frame(frame, nominal)
            } else {
                timecode(frame, fps)
            }
        }
    };
    let size = (pr.f("textSize") * b.scale).max(0.5);
    let pos = b.to_px(pr.v2("textPosition"));
    let wdt = line_width(&text, size, 0.0, false);
    let op = (pr.f("opacity") / 100.0) as f32;
    let boxed = pr.b("showBox").then(|| {
        let bc = pr.color("boxColor");
        let pad = size * 0.25;
        ([pos.0 - wdt * 0.5 - pad, pos.0 + wdt * 0.5 + pad, pos.1 - size * 0.5, pos.1 + size * 0.5], bc, op * bc[3])
    });
    let mut polys = Vec::new();
    layout_line(&text, [pos.0 - wdt * 0.5, pos.1 + unit(size) * 3.0], size, 0.0, false, &mut polys);
    let look = TextLook { display: 0, fill: pr.color("textColor"), stroke: [0.0; 4], stroke_w: 0.0, on_original: true, opacity: op };
    TextPlan { polys, r: weight(size), look, keep: pr.b("renderOnOriginal"), boxed }
}

fn timecode_fx(ctx: &EffectCtx, b: Buf) -> Buf {
    let plan = timecode_plan(ctx, &b);
    draw_plan(b, &plan)
}

// ---------------------------------------------------------------- Basic Text

fn basic_text(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let pass = basic_text_pass(ctx, &b);
    draw_text(&mut b, &pass.polys, pass.r, &pass.look);
    b
}

fn basic_text_pass(ctx: &EffectCtx, b: &Buf) -> TextPass {
    let pr = ctx.params;
    let size = (pr.f("size") * b.scale).max(0.5);
    let tracking = pr.f("tracking") * b.scale;
    let lead = size * pr.f("lineSpacing") / 100.0 * 1.2;
    let pos = b.to_px(pr.v2("position"));
    let align = pr.e("alignment");
    let lines: Vec<&str> = pr.s("text").split(['\n', '\r']).collect();
    let n = lines.len() as f64;
    let mut polys = Vec::new();
    for (i, l) in lines.iter().enumerate() {
        let wdt = line_width(l, size, tracking, true);
        let x = match align {
            0 => pos.0,
            2 => pos.0 - wdt,
            _ => pos.0 - wdt * 0.5,
        };
        let y = pos.1 + unit(size) * 3.0 + (i as f64 - (n - 1.0) * 0.5) * lead;
        layout_line(l, [x, y], size, tracking, true, &mut polys);
    }
    let look = look_from(ctx, b, "compositeOnOriginal");
    TextPass { polys, r: weight(size), look }
}

// ---------------------------------------------------------------- Path Text

fn path_text(ctx: &EffectCtx, mut b: Buf) -> Buf {
    for pass in path_text_passes(ctx, &b) {
        draw_text(&mut b, &pass.polys, pass.r, &pass.look);
    }
    b
}

fn path_text_passes(ctx: &EffectCtx, b: &Buf) -> Vec<TextPass> {
    let pr = ctx.params;
    let g = |group: &str, id: &str| grouped(pr, group, id).cloned().unwrap_or(Value::Scalar(0.0));
    let px = |id: &str| {
        let q = b.to_px(g("pathOptions/controlPoints", id).as_vec2());
        [q.0, q.1]
    };
    let (mut path, mut closed) = match mask_px(ctx, b, g("pathOptions", "customPath").as_f64().round() as usize) {
        Some((p, c, _)) => (p, c),
        None => match g("pathOptions", "shapeType").as_enum() {
            1 | 2 => {
                let c = px("vertex1");
                let s = px("tangent1");
                let r = ((s[0] - c[0]).powi(2) + (s[1] - c[1]).powi(2)).sqrt().max(1.0);
                let a0 = (s[1] - c[1]).atan2(s[0] - c[0]);
                let n = 128;
                (
                    (0..n)
                        .map(|i| {
                            let a = a0 + 2.0 * PI * i as f64 / n as f64;
                            [c[0] + a.cos() * r, c[1] + a.sin() * r]
                        })
                        .collect(),
                    true,
                )
            }
            3 => (vec![px("vertex1"), px("vertex2")], false),
            _ => {
                let (v1, t1, t2, v2) = (px("vertex1"), px("tangent1"), px("tangent2"), px("vertex2"));
                (
                    (0..=64)
                        .map(|i| {
                            let t = i as f64 / 64.0;
                            let u = 1.0 - t;
                            let f = |k: usize| u * u * u * v1[k] + 3.0 * u * u * t * t1[k] + 3.0 * u * t * t * t2[k] + t * t * t * v2[k];
                            [f(0), f(1)]
                        })
                        .collect(),
                    false,
                )
            }
        },
    };
    if g("pathOptions", "reversePath").as_bool() {
        path.reverse();
    }
    if path.len() < 2 {
        closed = false;
    }
    let len = poly_length(&path, closed);
    let visible = g("advanced", "visibleCharacters").as_f64().max(0.0);
    let all: Vec<char> = pr.s("text").chars().collect();
    let n_all = all.len();
    let size = (g("character", "size").as_f64() * b.scale).max(0.5);
    let u = unit(size);
    let tracking = g("character", "tracking").as_f64() * b.scale;
    let lm = g("paragraph", "leftMargin").as_f64() * b.scale;
    let rm = g("paragraph", "rightMargin").as_f64() * b.scale;
    let shift = g("paragraph", "baselineShift").as_f64() * b.scale;
    let line_spacing = pr.get("paragraph/lineSpacing").map(Value::as_f64).unwrap_or(size / b.scale * 1.2) * b.scale;
    let rot = g("character/orientation", "characterRotation").as_f64().to_radians();
    let shear = pr.f("character/orientation/characterShear").to_radians().tan();
    let hshear = pr.f("character/horizontalShear").to_radians().tan();
    let sx = pr.get("character/horizontalScale").map(Value::as_f64).unwrap_or(100.0) / 100.0;
    let sy = pr.get("character/verticalScale").map(Value::as_f64).unwrap_or(100.0) / 100.0;
    // Kerning: "index:value …" — the gap after character `index` (0-based) in 1/1000 em.
    let kerning: Vec<(usize, f64)> = pr
        .s("character/kerning")
        .split_whitespace()
        .filter_map(|t| {
            let (i, v) = t.split_once(':')?;
            Some((i.trim().parse().ok()?, v.trim().parse().ok()?))
        })
        .collect();
    let kern = |i: usize| kerning.iter().filter(|(k, _)| *k == i).map(|(_, v)| v / 1000.0 * size).sum::<f64>();
    // Jitter: per character, re-randomised every frame.
    let jit = |id: &str| pr.f(&format!("advanced/jitterSettings/{id}"));
    let (j_base, j_kern, j_rot, j_scale) =
        (jit("baselineJitterMax") * b.scale, jit("kerningJitterMax") * b.scale, jit("rotationJitterMax").to_radians(), jit("scaleJitterMax") / 100.0);
    let frame = (ctx.time * ctx.fps()).floor() as i64 as u32;
    let rnd = |i: usize, k: u32| hash1(i as u32, k.wrapping_add(frame.wrapping_mul(7919)), 0x7e17) as f64 * 2.0 - 1.0;
    // Fade Time: characters fade in over this share of the visible characters.
    let fade = pr.f("advanced/fadeTime").clamp(0.0, 100.0) / 100.0 * n_all.max(1) as f64;
    let char_alpha = |i: usize| -> f32 {
        if fade <= 0.0 { if (i as f64) < visible.floor() { 1.0 } else { 0.0 } } else { ((visible - i as f64) / fade).clamp(0.0, 1.0) as f32 }
    };
    // Lines (separated by line breaks) stack along the path's normal by Line Spacing.
    let mut polys_by_alpha: Vec<(f32, Vec<Vec<[f64; 2]>>)> = Vec::new();
    let mut gi = 0usize;
    for (li, line) in all.split(|c| *c == '\n' || *c == '\r').enumerate() {
        let idx0 = gi;
        gi += line.len() + 1;
        let n = line.len();
        let advs: Vec<f64> = line.iter().map(|&c| advance(c, true) * u * sx).collect();
        let kerns: Vec<f64> = (0..n).map(|k| kern(idx0 + k) + if j_kern != 0.0 { rnd(idx0 + k, 2) * j_kern } else { 0.0 }).collect();
        let natural: f64 = advs.iter().sum::<f64>() + tracking * (n.max(1) as f64 - 1.0) + kerns.iter().take(n.saturating_sub(1)).sum::<f64>();
        let avail = (len - lm - rm).max(0.0);
        let (mut cursor, extra) = match g("paragraph", "alignment").as_enum() {
            1 => (len - rm - natural, 0.0),
            2 => (lm + (avail - natural) * 0.5, 0.0),
            3 => (lm, if n > 1 { (avail - natural) / (n as f64 - 1.0) } else { 0.0 }),
            _ => (lm, 0.0),
        };
        let line_off = li as f64 * line_spacing;
        for (k, (&c, adv)) in line.iter().zip(advs).enumerate() {
            let i = idx0 + k;
            let mid = cursor + adv * 0.5;
            cursor += adv + tracking + extra + kerns[k];
            let alpha = char_alpha(i);
            if alpha <= 0.0 || (!closed && (mid < 0.0 || mid > len)) {
                continue;
            }
            let (q, d) = poly_point_at(&path, closed, if closed { mid.rem_euclid(len.max(1e-9)) } else { mid });
            let ang = d[1].atan2(d[0]) + rot + if j_rot != 0.0 { rnd(i, 3) * j_rot } else { 0.0 };
            let (s, co) = ang.sin_cos();
            let js = if j_scale != 0.0 { (1.0 + rnd(i, 4) * j_scale).max(0.05) } else { 1.0 };
            let jb = if j_base != 0.0 { rnd(i, 1) * j_base } else { 0.0 };
            let left = ink_left(c, true);
            if !polys_by_alpha.iter().any(|(a, _)| (*a - alpha).abs() < 1e-4) {
                polys_by_alpha.push((alpha, Vec::new()));
            }
            let Some((_, set)) = polys_by_alpha.iter_mut().find(|(a, _)| (*a - alpha).abs() < 1e-4) else { continue };
            for st in parse_glyph(c) {
                set.push(
                    st.iter()
                        .map(|gl| {
                            // Glyph space (baseline at 0, y down), then scale, shear and lift.
                            let gx = ((gl[0] - left - 0.8) * u + 0.8 * u) * sx * js - adv * 0.5;
                            let gy = (gl[1] - 6.0) * u * sy * js;
                            let gx = gx - gy * (shear + hshear);
                            let gy = gy - shift - jb + line_off;
                            [q[0] + gx * co - gy * s, q[1] + gx * s + gy * co]
                        })
                        .collect(),
                );
            }
        }
    }
    let mut look = look_from(ctx, b, "compositeOnOriginal");
    let base_op = look.opacity;
    let mut first = true;
    let mut passes = Vec::new();
    for (alpha, polys) in polys_by_alpha.iter_mut() {
        look.opacity = base_op * *alpha;
        passes.push(TextPass { polys: std::mem::take(polys), r: weight(size), look });
        if first {
            // Later passes go over the first one.
            look.on_original = true;
            first = false;
        }
    }
    if polys_by_alpha.is_empty() {
        passes.push(TextPass { polys: vec![], r: weight(size), look });
    }
    passes
}

// ---------------------------------------------------------------- Lightning (obsolete)

/// Smooth 1-D value noise in [-1, 1].
fn vnoise(i: u32, t: f64, seed: u32) -> f64 {
    let k = t.floor();
    let f = t - k;
    let a = hash1(i, k as i64 as u32, seed) as f64 * 2.0 - 1.0;
    let c = hash1(i, (k as i64 + 1) as u32, seed) as f64 * 2.0 - 1.0;
    let s = f * f * (3.0 - 2.0 * f);
    a + (c - a) * s
}

struct Bolt<'a> {
    seed: u32,
    phase: f64,
    stability: f64,
    detail: u32,
    detail_amp: f64,
    pull: [f64; 2],
    out: &'a mut Vec<Seg>,
    counter: u32,
}

impl Bolt<'_> {
    /// One bolt from `a` to `z` with `n` segments of amplitude `amp`; returns its points.
    fn path(&mut self, a: [f64; 2], z: [f64; 2], n: usize, amp: f64, fixed_end: bool) -> Vec<[f64; 2]> {
        let (dx, dy) = (z[0] - a[0], z[1] - a[1]);
        let len = (dx * dx + dy * dy).sqrt().max(1e-9);
        let nrm = [-dy / len, dx / len];
        let id = self.counter;
        self.counter += 1;
        let mut pts = Vec::with_capacity(n + 1);
        for i in 0..=n {
            let t = i as f64 / n as f64;
            let env = if fixed_end { (t * PI).sin() } else { t.sqrt() };
            let off = vnoise(id * 1000 + i as u32, self.phase * (1.0 - self.stability * 0.9) + i as f64 * 0.37, self.seed) * amp * env;
            let pull = t * t * len * 0.25;
            pts.push([a[0] + dx * t + nrm[0] * off + self.pull[0] * pull, a[1] + dy * t + nrm[1] * off + self.pull[1] * pull]);
        }
        // Detail: midpoint displacement.
        for lvl in 0..self.detail {
            let mut nx = Vec::with_capacity(pts.len() * 2);
            for (j, w) in pts.windows(2).enumerate() {
                let (p0, p1) = (w[0], w[1]);
                let (ex, ey) = (p1[0] - p0[0], p1[1] - p0[1]);
                let l = (ex * ex + ey * ey).sqrt();
                let o = vnoise(id * 7919 + lvl * 131 + j as u32, self.phase * 1.7 + j as f64 * 0.61, self.seed ^ 0x55) * l * self.detail_amp;
                nx.push(p0);
                nx.push([(p0[0] + p1[0]) * 0.5 - ey / l.max(1e-9) * o, (p0[1] + p1[1]) * 0.5 + ex / l.max(1e-9) * o]);
            }
            nx.push(*pts.last().unwrap_or(&a));
            pts = nx;
        }
        pts
    }
}

/// Lightning, shared with the GPU compositor (effectcraft-gpu `fx_gen2`): the bolt segments
/// (outer radius per segment) and how they are drawn.
#[derive(Clone, Debug)]
pub struct BoltPlan {
    pub segs: Vec<Seg>,
    /// Width (buffer px): the glow's blur.
    pub width: f64,
    /// Core Width (fraction of each segment's radius).
    pub core: f64,
    pub outside: [f32; 4],
    pub inside: [f32; 4],
    /// Blending Mode: 0 normal, 1 add, 2 screen.
    pub mode: u32,
}

fn lightning(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let plan = bolt_plan(ctx, &b);
    let (segs, width, core) = (&plan.segs, plan.width, plan.core);
    let (wd, ht) = (b.img.width as usize, b.img.height as usize);
    let outer = raster_segs(wd, ht, segs, 0.0);
    let core_segs: Vec<Seg> = segs.iter().map(|g| Seg { r: (g.r * core).max(0.3), ..*g }).collect();
    let inner = raster_segs(wd, ht, &core_segs, 0.6);
    let glow = gauss_plane(&outer, width * 0.5, width * 0.5);
    let (oc, ic, mode) = (plan.outside, plan.inside, plan.mode);
    b.img.data.par_iter_mut().enumerate().for_each(|(i, px)| {
        let o = outer.data[i].max(glow.data[i] * 0.8);
        let c = inner.data[i];
        let a = o.max(c).min(1.0);
        if a <= 0.0 {
            return;
        }
        let t = if a > 0.0 { c / a } else { 0.0 };
        let colr = [oc[0] + (ic[0] - oc[0]) * t, oc[1] + (ic[1] - oc[1]) * t, oc[2] + (ic[2] - oc[2]) * t];
        *px = match mode {
            1 => [px[0] + colr[0] * a, px[1] + colr[1] * a, px[2] + colr[2] * a, (px[3] + a * (1.0 - px[3])).min(1.0)],
            2 => {
                let sc = |d: f32, s: f32| d + s - d * s;
                [sc(px[0], colr[0] * a), sc(px[1], colr[1] * a), sc(px[2], colr[2] * a), sc(px[3], a)]
            }
            _ => over(*px, [colr[0] * a, colr[1] * a, colr[2] * a, a]),
        };
    });
    b
}

/// [`BoltPlan`] of Lightning on `b`'s geometry.
pub fn bolt_plan(ctx: &EffectCtx, b: &Buf) -> BoltPlan {
    let pr = ctx.params;
    let s = b.to_px(pr.v2("startPoint"));
    let e = b.to_px(pr.v2("endPoint"));
    let sc = b.scale;
    let rerun = pr.b("rerunAtEachFrame");
    let speed = pr.f("speed");
    let frame = (ctx.time * ctx.fps()).floor();
    let seed = (pr.f("randomSeed") as i64 as u32) ^ ctx.seed ^ if rerun { (frame as i64 as u32).wrapping_mul(2_654_435_761) } else { 0 };
    let phase = if rerun { 0.0 } else { ctx.time * speed * 0.25 };
    let pd = (pr.f("pullDirection") - 90.0).to_radians();
    let pf = pr.f("pullForce") / 100.0;
    let width = (pr.f("width") * sc).max(0.2);
    let wvar = pr.f("widthVariation").clamp(0.0, 1.0);
    let core = pr.f("coreWidth").clamp(0.0, 1.0);
    let mut segs: Vec<Seg> = Vec::new();
    let mut bolt = Bolt {
        seed,
        phase,
        stability: pr.f("stability").clamp(0.0, 1.0),
        detail: pr.f("detailLevel").round().clamp(0.0, 6.0) as u32,
        detail_amp: pr.f("detailAmplitude").clamp(0.0, 1.0) * 0.5,
        pull: [pd.cos() * pf, pd.sin() * pf],
        out: &mut segs,
        counter: 0,
    };
    let nseg = pr.f("segments").round().clamp(1.0, 64.0) as usize;
    let amp = pr.f("amplitude") * sc;
    let main = bolt.path([s.0, s.1], [e.0, e.1], nseg, amp, pr.b("fixedEndpoint"));
    let branching = pr.f("branching").clamp(0.0, 1.0);
    let rebranch = pr.f("rebranching").clamp(0.0, 1.0);
    let bang = pr.f("branchAngle").to_radians();
    let bseg_len = pr.f("branchSegLength").max(0.0);
    let bsegs = pr.f("branchSegments").round().clamp(1.0, 32.0) as usize;
    let bwidth = pr.f("branchWidth").clamp(0.0, 1.0);
    let main_len = poly_length(&main, false);
    let seg_len = main_len / nseg as f64;
    let mut lines: Vec<(Vec<[f64; 2]>, f64)> = vec![(main.clone(), 1.0)];
    let mut queue: Vec<(Vec<[f64; 2]>, f64, u32)> = vec![(main, 1.0, 0)];
    while let Some((pts, w, depth)) = queue.pop() {
        if depth >= 3 || lines.len() > 256 {
            continue;
        }
        let prob = if depth == 0 { branching } else { rebranch };
        for (i, wpair) in pts.windows(2).enumerate() {
            if hash1(i as u32 + depth * 977, lines.len() as u32, seed ^ 0xB7) as f64 >= prob * 0.35 {
                continue;
            }
            let (p0, p1) = (wpair[0], wpair[1]);
            let base = (p1[1] - p0[1]).atan2(p1[0] - p0[0]);
            let side = if hash1(i as u32, depth + 3, seed) < 0.5 { -1.0 } else { 1.0 };
            let a = base + side * bang * (0.5 + hash1(i as u32, depth + 9, seed) as f64);
            let l = seg_len * bseg_len * bsegs as f64;
            let z = [p0[0] + a.cos() * l, p0[1] + a.sin() * l];
            let br = bolt.path(p0, z, bsegs, amp * 0.5, false);
            let bw = w * bwidth;
            lines.push((br.clone(), bw));
            queue.push((br, bw, depth + 1));
        }
    }
    for (li, (pts, w)) in lines.iter().enumerate() {
        for (i, wpair) in pts.windows(2).enumerate() {
            let v = 1.0 - wvar * hash1(i as u32, li as u32, seed ^ 0x77) as f64;
            bolt.out.push(Seg { a: wpair[0], b: wpair[1], r: width * 0.5 * w * v, v: 1.0 });
        }
    }
    BoltPlan { segs, width, core, outside: pr.color("outsideColor"), inside: pr.color("insideColor"), mode: pr.e("blendingMode") }
}

// ---------------------------------------------------------------- registry

pub fn specs() -> Vec<EffectSpec> {
    let ang = || ParamUi::Angle;
    let check = |id: &'static str, name: &'static str, v: bool| p(id, name, Value::Bool(v), ParamUi::Checkbox);
    let pt = |x: f64, y: f64| Value::Vec2([x, y]);
    let display = || popup(&["Fill Only", "Stroke Only", "Fill Over Stroke", "Stroke Over Fill"]);
    let size = || slider(0.0, 512.0, 0.0, 200.0, 1);
    vec![
        spec(
            "ec.text.numbers",
            "Numbers",
            "Text",
            vec![
                p(
                    "format/type",
                    "Type",
                    Value::Enum(0),
                    popup(&[
                        "Number",
                        "Number [Leading Zeros]",
                        "Timecode [30]",
                        "Timecode [25]",
                        "Timecode [24]",
                        "Time",
                        "Numerical Date",
                        "Short Date",
                        "Long Date",
                        "Hexadecimal",
                    ]),
                ),
                check("format/randomValues", "Random Values", false),
                p("format/value", "Value/Offset/Random Max", num(0.0), slider(-30000.0, 30000.0, -1000.0, 1000.0, 3)),
                p("format/decimalPlaces", "Decimal Places", num(2.0), slider(0.0, 10.0, 0.0, 10.0, 0)),
                check("format/currentTimeDate", "Current Time/Date", false),
                p("fillAndStroke/position", "Position", pt(0.5, 0.5), ParamUi::Point),
                p("fillAndStroke/displayOptions", "Display Options", Value::Enum(0), display()),
                p("fillAndStroke/fillColor", "Fill Color", col(1.0, 0.0, 0.0), ParamUi::Color),
                p("fillAndStroke/strokeColor", "Stroke Color", col(1.0, 1.0, 1.0), ParamUi::Color),
                p("fillAndStroke/strokeWidth", "Stroke Width", num(2.0), slider(0.0, 50.0, 0.0, 50.0, 1)),
                p("size", "Size", num(36.0), size()),
                p("tracking", "Tracking", num(0.0), slider(-100.0, 100.0, -20.0, 50.0, 1)),
                check("proportionalSpacing", "Proportional Spacing", true),
                check("compositeOnOriginal", "Composite On Original", true),
            ],
            numbers,
        ),
        spec(
            "ec.text.timecode",
            "Timecode",
            "Text",
            vec![
                p("displayFormat", "Display Format", Value::Enum(0), popup(&["SMPTE HH:MM:SS:FF", "Frames", "Feet + Frames (35mm)", "Feet + Frames (16mm)"])),
                p("timeSource", "Time Source", Value::Enum(0), popup(&["Layer Source", "Composition", "Custom"])),
                p("timeUnits", "Time Units", num(30.0), slider(1.0, 120.0, 1.0, 60.0, 2)),
                check("dropFrame", "Drop Frame", false),
                p("startingFrame", "Starting Frame", num(0.0), slider(-1_000_000.0, 1_000_000.0, 0.0, 10_000.0, 0)),
                p("textPosition", "Text Position", pt(0.5, 0.9), ParamUi::Point),
                p("textSize", "Text Size", num(36.0), size()),
                p("textColor", "Text Color", col(1.0, 1.0, 1.0), ParamUi::Color),
                check("showBox", "Show Box", true),
                p("boxColor", "Box Color", col(0.0, 0.0, 0.0), ParamUi::Color),
                p("opacity", "Opacity", num(100.0), pct()),
                check("renderOnOriginal", "Composite on Original", true),
            ],
            timecode_fx,
        ),
        spec(
            "ec.obsolete.basictext",
            "Basic Text",
            "Obsolete",
            vec![
                p("text", "Text", Value::Str("Text".into()), ParamUi::Text),
                p("alignment", "Alignment", Value::Enum(1), popup(&["Left", "Center", "Right"])),
                p("position", "Position", pt(0.5, 0.5), ParamUi::Point),
                p("fillAndStroke/displayOptions", "Display Options", Value::Enum(0), display()),
                p("fillAndStroke/fillColor", "Fill Color", col(1.0, 0.0, 0.0), ParamUi::Color),
                p("fillAndStroke/strokeColor", "Stroke Color", col(1.0, 1.0, 1.0), ParamUi::Color),
                p("fillAndStroke/strokeWidth", "Stroke Width", num(2.0), slider(0.0, 50.0, 0.0, 50.0, 1)),
                p("size", "Size", num(36.0), size()),
                p("tracking", "Tracking", num(0.0), slider(-100.0, 100.0, -20.0, 50.0, 1)),
                p("lineSpacing", "Line Spacing", num(100.0), slider(0.0, 1000.0, 0.0, 300.0, 1)),
                check("compositeOnOriginal", "Composite On Original", true),
            ],
            basic_text,
        ),
        spec(
            "ec.obsolete.pathtext",
            "Path Text",
            "Obsolete",
            vec![
                p("text", "Text", Value::Str("Text".into()), ParamUi::Text),
                p("pathOptions/shapeType", "Shape Type", Value::Enum(0), popup(&["Bezier", "Circle", "Loop", "Line"])),
                p("pathOptions/controlPoints/tangent1", "Tangent 1/Circle Point", pt(0.3, 0.3), ParamUi::Point),
                p("pathOptions/controlPoints/vertex1", "Vertex 1/Circle Center", pt(0.1, 0.6), ParamUi::Point),
                p("pathOptions/controlPoints/tangent2", "Tangent 2", pt(0.7, 0.3), ParamUi::Point),
                p("pathOptions/controlPoints/vertex2", "Vertex 2/Line Right", pt(0.9, 0.6), ParamUi::Point),
                p("pathOptions/customPath", "Custom Path", num(0.0), slider(0.0, 32.0, 0.0, 8.0, 0)),
                check("pathOptions/reversePath", "Reverse Path", false),
                p("fillAndStroke/displayOptions", "Options", Value::Enum(0), display()),
                p("fillAndStroke/fillColor", "Fill Color", col(1.0, 0.0, 0.0), ParamUi::Color),
                p("fillAndStroke/strokeColor", "Stroke Color", col(1.0, 1.0, 1.0), ParamUi::Color),
                p("fillAndStroke/strokeWidth", "Stroke Width", num(2.0), slider(0.0, 50.0, 0.0, 50.0, 1)),
                p("character/size", "Size", num(36.0), size()),
                p("character/tracking", "Tracking", num(0.0), slider(-100.0, 100.0, -20.0, 50.0, 1)),
                // Kerning: "index:value" pairs, the gap after character `index` in 1/1000 em.
                p("character/kerning", "Kerning", Value::Str(String::new()), ParamUi::Text),
                p("character/orientation/characterRotation", "Character Rotation", num(0.0), ang()),
                p("character/orientation/characterShear", "Character Shear", num(0.0), slider(-70.0, 70.0, -70.0, 70.0, 1)),
                p("character/horizontalShear", "Horizontal Shear", num(0.0), slider(-70.0, 70.0, -70.0, 70.0, 1)),
                p("character/horizontalScale", "Horizontal Scale", num(100.0), slider(1.0, 1000.0, 10.0, 400.0, 1)),
                p("character/verticalScale", "Vertical Scale", num(100.0), slider(1.0, 1000.0, 10.0, 400.0, 1)),
                p("paragraph/alignment", "Alignment", Value::Enum(0), popup(&["Left", "Right", "Center", "Force"])),
                p("paragraph/leftMargin", "Left Margin", num(0.0), slider(-10000.0, 10000.0, -500.0, 500.0, 1)),
                p("paragraph/rightMargin", "Right Margin", num(0.0), slider(-10000.0, 10000.0, -500.0, 500.0, 1)),
                p("paragraph/lineSpacing", "Line Spacing", num(43.0), slider(-1000.0, 1000.0, 0.0, 200.0, 1)),
                p("paragraph/baselineShift", "Baseline Shift", num(0.0), slider(-1000.0, 1000.0, -100.0, 100.0, 1)),
                p("advanced/visibleCharacters", "Visible Characters", num(1024.0), slider(0.0, 1024.0, 0.0, 1024.0, 0)),
                p("advanced/fadeTime", "Fade Time", num(0.0), slider(0.0, 100.0, 0.0, 100.0, 1)),
                p("advanced/jitterSettings/baselineJitterMax", "Baseline Jitter Max", num(0.0), slider(0.0, 1000.0, 0.0, 100.0, 1)),
                p("advanced/jitterSettings/kerningJitterMax", "Kerning Jitter Max", num(0.0), slider(0.0, 1000.0, 0.0, 100.0, 1)),
                p("advanced/jitterSettings/rotationJitterMax", "Rotation Jitter Max", num(0.0), slider(0.0, 360.0, 0.0, 360.0, 1)),
                p("advanced/jitterSettings/scaleJitterMax", "Scale Jitter Max", num(0.0), slider(0.0, 1000.0, 0.0, 100.0, 1)),
                check("compositeOnOriginal", "Composite On Original", true),
            ],
            path_text,
        ),
        spec(
            "ec.obsolete.lightning",
            "Lightning",
            "Obsolete",
            vec![
                p("startPoint", "Start Point", pt(0.1, 0.5), ParamUi::Point),
                p("endPoint", "End Point", pt(0.9, 0.5), ParamUi::Point),
                p("segments", "Segments", num(7.0), slider(1.0, 64.0, 1.0, 32.0, 0)),
                p("amplitude", "Amplitude", num(10.0), slider(0.0, 1000.0, 0.0, 100.0, 1)),
                p("detailLevel", "Detail Level", num(2.0), slider(0.0, 6.0, 0.0, 6.0, 0)),
                p("detailAmplitude", "Detail Amplitude", num(0.3), slider(0.0, 1.0, 0.0, 1.0, 3)),
                p("branching", "Branching", num(0.4), slider(0.0, 1.0, 0.0, 1.0, 3)),
                p("rebranching", "Rebranching", num(0.3), slider(0.0, 1.0, 0.0, 1.0, 3)),
                p("branchAngle", "Branch Angle", num(20.0), ang()),
                p("branchSegLength", "Branch Seg. Length", num(0.5), slider(0.0, 2.0, 0.0, 2.0, 3)),
                p("branchSegments", "Branch Segments", num(4.0), slider(1.0, 32.0, 1.0, 16.0, 0)),
                p("branchWidth", "Branch Width", num(0.7), slider(0.0, 1.0, 0.0, 1.0, 3)),
                p("speed", "Speed", num(10.0), slider(-100.0, 100.0, -50.0, 50.0, 1)),
                p("stability", "Stability", num(0.3), slider(0.0, 1.0, 0.0, 1.0, 3)),
                check("fixedEndpoint", "Fixed Endpoint", true),
                p("width", "Width", num(10.0), slider(0.0, 100.0, 0.0, 50.0, 1)),
                p("widthVariation", "Width Variation", num(0.25), slider(0.0, 1.0, 0.0, 1.0, 3)),
                p("coreWidth", "Core Width", num(0.4), slider(0.0, 1.0, 0.0, 1.0, 3)),
                p("outsideColor", "Outside Color", col(0.36, 0.25, 0.9), ParamUi::Color),
                p("insideColor", "Inside Color", col(1.0, 1.0, 1.0), ParamUi::Color),
                p("pullForce", "Pull Force", num(0.0), slider(-100.0, 100.0, -100.0, 100.0, 1)),
                p("pullDirection", "Pull Direction", num(0.0), ang()),
                p("randomSeed", "Random Seed", num(1.0), slider(0.0, 10000.0, 0.0, 100.0, 0)),
                p("blendingMode", "Blending Mode", Value::Enum(0), popup(&["Normal", "Add", "Screen"])),
                check("rerunAtEachFrame", "Rerun At Each Frame", false),
            ],
            lightning,
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EffectEnv, MaskShape, Params};

    fn run_env(id: &str, set: &[(&str, Value)], img: Image, time: f64, env: EffectEnv) -> Buf {
        let s = crate::find(id).unwrap();
        let ls = [img.width as f64, img.height as f64];
        let mut params = Params { values: s.params.iter().map(|p| (p.id.to_string(), p.default.clone())).collect() };
        for p in &s.params {
            if let (ParamUi::Point, Value::Vec2(v)) = (&p.ui, &p.default) {
                params.values.insert(p.id.to_string(), Value::Vec2([v[0] * ls[0], v[1] * ls[1]]));
            }
        }
        for (k, v) in set {
            assert!(params.values.contains_key(*k), "{id}: unknown {k}");
            params.values.insert(k.to_string(), v.clone());
        }
        let ctx = EffectCtx { params: &params, time, layer_size: ls, seed: 3, adjustment: false, env };
        crate::apply(s, &ctx, Buf { img, offset: [0.0, 0.0], scale: 1.0 })
    }

    fn run(id: &str, set: &[(&str, Value)], img: Image, time: f64) -> Buf {
        run_env(id, set, img, time, EffectEnv::default())
    }

    fn ink(img: &Image) -> f32 {
        img.data.iter().map(|p| p[3]).sum()
    }

    #[test]
    fn every_glyph_parses_within_grid() {
        for c in (b'!'..=b'~').map(|c| c as char) {
            for s in parse_glyph(c) {
                for q in s {
                    assert!((-0.5..=4.5).contains(&q[0]) && (-0.5..=7.0).contains(&q[1]), "{c}: {q:?}");
                }
            }
        }
        assert!(!parse_glyph('A').is_empty() && !parse_glyph('a').is_empty() && parse_glyph(' ').is_empty());
    }

    #[test]
    fn numbers_formats() {
        let params = |vals: &[(&str, Value)]| {
            let s = crate::find("ec.text.numbers").unwrap();
            let mut p = Params { values: s.params.iter().map(|p| (p.id.to_string(), p.default.clone())).collect() };
            for (k, v) in vals {
                p.values.insert(k.to_string(), v.clone());
            }
            p
        };
        let txt = |vals: &[(&str, Value)], t: f64| {
            let pr = params(vals);
            let ctx = EffectCtx { params: &pr, time: t, layer_size: [10.0, 10.0], seed: 1, adjustment: false, env: Default::default() };
            numbers_text(&ctx)
        };
        assert_eq!(txt(&[("value", num(8.0))], 0.0), "8.00");
        assert_eq!(txt(&[("value", num(42.5)), ("type", Value::Enum(1))], 0.0), "00042.50");
        assert_eq!(txt(&[("value", num(3661.5)), ("type", Value::Enum(2))], 0.0), "01:01:01:15");
        assert_eq!(txt(&[("value", num(255.0)), ("type", Value::Enum(9))], 0.0), "FF");
        assert_eq!(txt(&[("value", num(0.0)), ("type", Value::Enum(6))], 0.0), "01/01/00");
        assert_eq!(txt(&[("value", num(60.0)), ("type", Value::Enum(8))], 0.0), "Wednesday, March 1, 2000");
        // A keyframed date count past chrono's ±262143 range still renders its real date and a
        // matching weekday (275790-09-13 is a Monday), not an epoch-derived stand-in.
        assert_eq!(txt(&[("value", num(100_000_000.0)), ("type", Value::Enum(8))], 0.0), "Monday, September 13, 275790");
        assert_eq!(txt(&[("value", num(-100_000_000.0)), ("type", Value::Enum(7))], 0.0), "Apr 20, -271791");
        assert_eq!(txt(&[("value", num(1.0)), ("currentTimeDate", Value::Bool(true)), ("decimalPlaces", num(1.0))], 2.0), "3.0");
        assert_ne!(
            txt(&[("value", num(100.0)), ("randomValues", Value::Bool(true))], 0.0),
            txt(&[("value", num(100.0)), ("randomValues", Value::Bool(true))], 1.0)
        );
    }

    #[test]
    fn date_from_days_past_chrono_range() {
        // chrono's NaiveDate stops near ±262143, but Numbers' `value` has no range guard, so a
        // keyframed day count can run past it. The 400-year cycle keeps the real date (and a
        // matching weekday) for every count; values from the pre-chrono formula, both directions,
        // both sides of the 2000-01-01 epoch.
        for (days, date, day) in [
            (0_i64, (2000, 1, 1), "Saturday"),
            (60, (2000, 3, 1), "Wednesday"),
            (95_015_644, (262143, 12, 31), "Tuesday"), // chrono's last representable date
            (100_000_000, (275790, 9, 13), "Monday"),  // past it: used to read as the epoch
            (-96_476_616, (-262145, 12, 31), "Monday"),
            (-100_000_000, (-271791, 4, 20), "Thursday"),
        ] {
            let (y, m, d, wd) = super::date_from_days(days);
            assert_eq!((y, m, d), date, "days={days}");
            assert_eq!(super::DAYS[wd], day, "days={days}");
        }
        // Extremes don't overflow (i128 intermediates).
        let _ = (super::date_from_days(i64::MAX), super::date_from_days(i64::MIN));
    }

    #[test]
    fn numbers_draws_glyph_at_position() {
        let img = Image::new(80, 60);
        let a = run("ec.text.numbers", &[("format/value", num(8.0)), ("format/decimalPlaces", num(0.0))], img.clone(), 0.0);
        assert_eq!(a.img.data, run("ec.text.numbers", &[("format/value", num(8.0)), ("format/decimalPlaces", num(0.0))], img.clone(), 0.0).img.data);
        // Ink only around the centre.
        let mut inside = 0.0;
        let mut outside = 0.0;
        for y in 0..60 {
            for x in 0..80 {
                let v = a.img.get(x, y)[3];
                if (25..55).contains(&x) && (10..50).contains(&y) {
                    inside += v;
                } else {
                    outside += v;
                }
            }
        }
        assert!(inside > 20.0 && outside < 0.5, "{inside} {outside}");
        let stroke = run("ec.text.numbers", &[("format/value", num(8.0)), ("fillAndStroke/displayOptions", Value::Enum(2))], img, 0.0);
        assert!(stroke.img.data.iter().any(|p| p[3] > 0.5 && p[1] > 0.5 * p[3]), "white stroke visible");
    }

    #[test]
    fn timecode_changes_over_time() {
        let img = Image::filled(120, 60, [0.2, 0.2, 0.2, 1.0]);
        let a = run("ec.text.timecode", &[], img.clone(), 0.0);
        let b = run("ec.text.timecode", &[], img.clone(), 1.0);
        assert_eq!(a.img.data, run("ec.text.timecode", &[], img.clone(), 0.0).img.data);
        assert_ne!(a.img.data, b.img.data);
        assert_eq!(timecode(30 * 61 + 5, 30.0), "00:01:01:05");
        assert_eq!(drop_frame(1800, 30), "00;01;00;02");
        assert_eq!(drop_frame(17982, 30), "00;10;00;00");
    }

    #[test]
    fn basic_text_draws_and_respects_composite() {
        let img = Image::filled(100, 50, [0.0, 0.0, 1.0, 1.0]);
        let a = run("ec.obsolete.basictext", &[("text", Value::Str("Hi".into()))], img.clone(), 0.0);
        assert_eq!(a.img.get(1, 1), [0.0, 0.0, 1.0, 1.0]);
        assert!(a.img.data.iter().any(|p| p[0] > 0.9));
        let t = run("ec.obsolete.basictext", &[("text", Value::Str("Hi".into())), ("compositeOnOriginal", Value::Bool(false))], img, 0.0);
        assert_eq!(t.img.get(1, 1)[3], 0.0);
        assert!(ink(&t.img) > 10.0);
    }

    #[test]
    fn path_text_follows_line_and_mask() {
        let img = Image::new(120, 60);
        let line = run(
            "ec.obsolete.pathtext",
            &[
                ("pathOptions/shapeType", Value::Enum(3)),
                ("pathOptions/controlPoints/vertex1", Value::Vec2([5.0, 50.0])),
                ("pathOptions/controlPoints/vertex2", Value::Vec2([115.0, 50.0])),
                ("character/size", num(20.0)),
            ],
            img.clone(),
            0.0,
        );
        assert!(ink(&line.img) > 10.0);
        // Text sits above the baseline at y = 50.
        let below: f32 = (52..60).flat_map(|y| (0..120).map(move |x| (x, y))).map(|(x, y)| line.img.get(x, y)[3]).sum();
        assert!(below < 1.0);
        let masks = vec![MaskShape { name: "M".into(), points: vec![[10.0, 20.0], [110.0, 20.0]], closed: false, inverted: false }];
        let env = EffectEnv { masks: &masks, ..Default::default() };
        let m = run_env("ec.obsolete.pathtext", &[("pathOptions/customPath", num(1.0)), ("character/size", num(20.0))], img.clone(), 0.0, env);
        let near: f32 = (0..24).flat_map(|y| (0..120).map(move |x| (x, y))).map(|(x, y)| m.img.get(x, y)[3]).sum();
        assert!(near > 10.0 && (ink(&m.img) - near).abs() < 1.0);
        let circle = run("ec.obsolete.pathtext", &[("pathOptions/shapeType", Value::Enum(1)), ("text", Value::Str("CIRCLE TEXT".into()))], img, 0.0);
        assert!(ink(&circle.img) > 10.0);
    }

    #[test]
    fn path_text_kerning_scale_lines_fade_and_jitter() {
        let img = Image::new(160, 100);
        let line = [
            ("pathOptions/shapeType", Value::Enum(3)),
            ("pathOptions/controlPoints/vertex1", Value::Vec2([5.0, 40.0])),
            ("pathOptions/controlPoints/vertex2", Value::Vec2([155.0, 40.0])),
            ("character/size", num(20.0)),
            ("text", Value::Str("AB".into())),
        ];
        let with = |extra: &[(&str, Value)], t: f64| run("ec.obsolete.pathtext", &[line.as_slice(), extra].concat(), img.clone(), t).img;
        let right_edge = |i: &Image| (0..160).rev().find(|&x| (0..100).any(|y| i.get(x, y)[3] > 0.3)).unwrap_or(0);
        let base = with(&[], 0.0);
        // Kerning after the first character pushes the second one along.
        assert!(right_edge(&with(&[("character/kerning", Value::Str("0:500".into()))], 0.0)) >= right_edge(&base) + 8);
        // Horizontal Scale widens, shear changes the shapes.
        assert!(right_edge(&with(&[("character/horizontalScale", num(200.0))], 0.0)) > right_edge(&base) + 5);
        assert_ne!(with(&[("character/horizontalShear", num(30.0))], 0.0), base);
        // A second line sits Line Spacing below the first.
        let two = with(&[("text", Value::Str("AB\rCD".into())), ("paragraph/lineSpacing", num(30.0))], 0.0);
        let ink_rows = |i: &Image, y0: i64, y1: i64| (y0..y1).flat_map(|y| (0..160).map(move |x| (x, y))).map(|(x, y)| i.get(x, y)[3]).sum::<f32>();
        assert!(ink_rows(&two, 52, 72) > 5.0 && ink_rows(&base, 52, 72) < 0.5);
        // Fade Time: the character being revealed is partly transparent.
        let fading = with(&[("advanced/visibleCharacters", num(1.5)), ("advanced/fadeTime", num(50.0))], 0.0);
        let total = |i: &Image| i.data.iter().map(|p| p[3]).sum::<f32>();
        let (one, both) = (with(&[("advanced/visibleCharacters", num(1.0))], 0.0), with(&[("advanced/visibleCharacters", num(2.0))], 0.0));
        assert!(total(&fading) > total(&one) + 1.0 && total(&fading) < total(&both) - 1.0, "{} {} {}", total(&one), total(&fading), total(&both));
        // Jitter differs between frames but is repeatable.
        let j = [("advanced/jitterSettings/baselineJitterMax", num(10.0))];
        assert_ne!(with(&j, 0.0), with(&j, 0.5));
        assert_eq!(with(&j, 0.5), with(&j, 0.5));
    }

    #[test]
    fn lightning_deterministic_and_animates() {
        let img = Image::new(96, 64);
        let a = run("ec.obsolete.lightning", &[], img.clone(), 0.5);
        let b = run("ec.obsolete.lightning", &[], img.clone(), 0.5);
        assert_eq!(a.img.data, b.img.data);
        assert!(a.img.get(48, 32)[3] > 0.0 || ink(&a.img) > 50.0);
        let c = run("ec.obsolete.lightning", &[], img.clone(), 1.5);
        assert_ne!(a.img.data, c.img.data);
        let r1 = run("ec.obsolete.lightning", &[("rerunAtEachFrame", Value::Bool(true))], img.clone(), 0.5);
        let r2 = run("ec.obsolete.lightning", &[("rerunAtEachFrame", Value::Bool(true))], img, 0.5);
        assert_eq!(r1.img.data, r2.img.data);
    }
}
