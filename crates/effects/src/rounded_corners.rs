//! Rounded Corners (Stylize): cuts a layer to a rectangle with rounded corners.
//!
//! An EffectCraft effect with no After Effects counterpart, written from scratch. The rectangle is
//! the layer's bounds or the bounds of its visible pixels (for text and shape layers, whose
//! layer is the size of the comp), grown or shrunk by Padding. Each corner has its own radius,
//! in pixels or in percent of the rectangle's shorter side, so 50% gives a pill or a circle at
//! any size. Radii that do not fit are scaled down together, as CSS `border-radius` does, so the
//! shape keeps its proportions. Edges are anti-aliased and can be feathered.
//!
//! On an adjustment layer the effect leaves the frame unchanged: its input is the comp below in
//! comp space, and the effect is not told where the adjustment layer sits, so it has no rectangle
//! to round there. Rounding a whole composition is done by precomposing it.

use effectcraft_keyframe::Value;
use effectcraft_project::ParamUi;
use rayon::prelude::*;

use crate::{Buf, EffectCtx, EffectSpec, num, p, popup, slider};

pub(crate) const ID: &str = "ec.stylize.roundedcorners";
const CORNERS: [&str; 4] = ["topLeft", "topRight", "bottomRight", "bottomLeft"];
/// Parameter ranges (layer pixels, or percent for Units: Percent of Shorter Side). Values from
/// expressions are clamped to them, so the geometry below never overflows.
const MAX_RADIUS: f64 = 10000.0;
const MAX_PADDING: f64 = 10000.0;
const MAX_FEATHER: f64 = 1000.0;

/// Parameter `id` clamped to `lo..=hi`; 0 when it is not a finite number.
fn clamped(ctx: &EffectCtx, id: &str, lo: f64, hi: f64) -> f64 {
    let v = ctx.params.f(id);
    if v.is_finite() { v.clamp(lo, hi) } else { 0.0 }
}

/// Whether Effect Controls shows `param`: the four corners replace Radius when Individual
/// Corners is on.
pub(crate) fn shown(param: &str, individual: bool) -> bool {
    match param {
        "radius" => !individual,
        _ if CORNERS.contains(&param) => individual,
        _ => true,
    }
}

/// Bounding box `[x0, y0, x1, y1]` (buffer pixels, `x1`/`y1` exclusive) of the pixels with
/// any alpha (HDR colour over a tiny alpha still shows), or `None` when there are none.
fn content_box(b: &Buf) -> Option<[f64; 4]> {
    let w = b.img.width.max(1) as usize;
    b.img
        .data
        .par_chunks(w)
        .enumerate()
        .filter_map(|(y, row)| {
            let first = row.iter().position(|px| px[3] > 0.0)?;
            let last = row.iter().rposition(|px| px[3] > 0.0)?;
            Some([first, y, last + 1, y + 1])
        })
        .reduce_with(|a, c| [a[0].min(c[0]), a[1].min(c[1]), a[2].max(c[2]), a[3].max(c[3])])
        .map(|r| r.map(|v| v as f64))
}

/// Corner radii (top left, top right, bottom right, bottom left) for a `w`×`h` rectangle, in
/// buffer pixels: scaled down together when two neighbours don't fit along a side, then each
/// capped at half the shorter side.
fn fit_radii(r: [f64; 4], w: f64, h: f64) -> [f64; 4] {
    let r = r.map(|v| if v.is_finite() { v.max(0.0) } else { 0.0 });
    let [tl, tr, br, bl] = r;
    let ratio = |side: f64, sum: f64| if sum > side { side / sum } else { 1.0 };
    let k = ratio(w, tl + tr).min(ratio(w, bl + br)).min(ratio(h, tl + bl)).min(ratio(h, tr + br));
    let cap = w.min(h) * 0.5;
    r.map(|v| (v * k).min(cap))
}

/// Signed distance from `(x, y)` to a rectangle centred on `c` with half size `half` whose
/// corner nearest the point has radius `r` (`r` ≤ the smaller half size): negative inside.
fn rounded_box_distance(x: f64, y: f64, c: [f64; 2], half: [f64; 2], r: f64) -> f64 {
    let qx = (x - c[0]).abs() - half[0] + r;
    let qy = (y - c[1]).abs() - half[1] + r;
    qx.max(qy).min(0.0) + qx.max(0.0).hypot(qy.max(0.0)) - r
}

fn rounded_corners(ctx: &EffectCtx, mut b: Buf) -> Buf {
    if ctx.adjustment {
        return b;
    }
    let pr = ctx.params;
    let rect = if pr.e("bounds") == 1 {
        match content_box(&b) {
            Some(r) => r,
            None => return b,
        }
    } else {
        let (x0, y0) = b.to_px([0.0, 0.0]);
        let (x1, y1) = b.to_px(ctx.layer_size);
        [x0.min(x1), y0.min(y1), x0.max(x1), y0.max(y1)]
    };
    let pad = clamped(ctx, "padding", -MAX_PADDING, MAX_PADDING) * b.scale;
    let [x0, y0, x1, y1] = [rect[0] - pad, rect[1] - pad, rect[2] + pad, rect[3] + pad];
    let (w, h) = (x1 - x0, y1 - y0);
    if !(w.is_finite() && h.is_finite()) {
        return b;
    }
    if w <= 0.0 || h <= 0.0 {
        // Padding shrank the rectangle away: nothing is left inside it.
        b.img.data.par_iter_mut().for_each(|px| *px = [0.0; 4]);
        return b;
    }
    let radius = |id: &str| clamped(ctx, id, 0.0, MAX_RADIUS);
    let radii = if pr.b("individualCorners") { CORNERS.map(radius) } else { [radius("radius"); 4] };
    // Pixels are layer pixels at the buffer's scale; percent is of the shorter side.
    let unit = if pr.e("units") == 1 { w.min(h) / 100.0 } else { b.scale };
    let [tl, tr, br, bl] = fit_radii(radii.map(|r| r * unit), w, h);
    // At least one pixel wide, so a hard edge is still anti-aliased.
    let soft = (clamped(ctx, "feather", 0.0, MAX_FEATHER) * b.scale).max(1.0);
    let c = [(x0 + x1) * 0.5, (y0 + y1) * 0.5];
    let half = [w * 0.5, h * 0.5];
    b.img.rows_mut().for_each(|(y, row)| {
        let py = y as f64 + 0.5;
        for (x, px) in row.iter_mut().enumerate() {
            let px_x = x as f64 + 0.5;
            let r = match (px_x < c[0], py < c[1]) {
                (true, true) => tl,
                (false, true) => tr,
                (false, false) => br,
                (true, false) => bl,
            };
            let cover = (0.5 - rounded_box_distance(px_x, py, c, half, r) / soft).clamp(0.0, 1.0) as f32;
            if cover <= 0.0 {
                // Written outright: an infinite HDR value times 0 would be NaN.
                *px = [0.0; 4];
            } else if cover < 1.0 {
                *px = px.map(|v| v * cover);
            }
        }
    });
    b
}

pub fn specs() -> Vec<EffectSpec> {
    let radius = |id: &'static str, name: &'static str| p(id, name, num(40.0), slider(0.0, MAX_RADIUS, 0.0, 200.0, 1));
    vec![EffectSpec {
        id: ID,
        name: "Rounded Corners",
        category: "Stylize",
        params: vec![
            radius("radius", "Radius"),
            p("units", "Units", Value::Enum(0), popup(&["Pixels", "Percent of Shorter Side"])),
            p("individualCorners", "Individual Corners", Value::Bool(false), ParamUi::Checkbox),
            radius("topLeft", "Top Left"),
            radius("topRight", "Top Right"),
            radius("bottomRight", "Bottom Right"),
            radius("bottomLeft", "Bottom Left"),
            p("bounds", "Bounds", Value::Enum(0), popup(&["Layer", "Layer Content"])),
            p("padding", "Padding", num(0.0), slider(-MAX_PADDING, MAX_PADDING, -100.0, 100.0, 1)),
            p("feather", "Feather", num(0.0), slider(0.0, MAX_FEATHER, 0.0, 50.0, 1)),
        ],
        render: rounded_corners,
        gpu: false,
        float: true,
    }]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EffectEnv, run_fx};
    use effectcraft_raster::Image;

    fn solid(w: u32, h: u32) -> Image {
        let mut img = Image::new(w, h);
        img.data.iter_mut().for_each(|px| *px = [0.6, 0.3, 0.1, 1.0]);
        img
    }

    fn run(vals: &[(&str, Value)], img: Image) -> Image {
        run_fx(ID, vals, img, 0.0, EffectEnv::default()).img
    }

    fn alpha(img: &Image, x: u32, y: u32) -> f32 {
        img.data[(y * img.width + x) as usize][3]
    }

    /// Pixels the effect made (partly) transparent.
    fn cut(img: &Image) -> usize {
        img.data.iter().filter(|px| px[3] < 1.0).count()
    }

    #[test]
    fn deterministic() {
        let vals = [("radius", num(9.0)), ("feather", num(3.0))];
        assert_eq!(run(&vals, solid(48, 32)).data, run(&vals, solid(48, 32)).data);
    }

    #[test]
    fn zero_radius_keeps_the_layer_and_a_radius_cuts_only_its_corners() {
        let img = solid(48, 32);
        assert_eq!(run(&[("radius", num(0.0))], img.clone()).data, img.data);
        let o = run(&[("radius", num(10.0))], img.clone());
        for (x, y) in [(0, 0), (47, 0), (47, 31), (0, 31)] {
            assert_eq!(alpha(&o, x, y), 0.0, "corner {x},{y}");
        }
        for (x, y) in [(24, 0), (0, 16), (47, 16), (24, 31), (24, 16)] {
            assert_eq!(alpha(&o, x, y), 1.0, "edge or centre {x},{y}");
        }
        // Premultiplied colour goes with alpha.
        assert!(o.data.iter().all(|px| px[0] <= 0.6 * px[3] + 1e-6));
    }

    #[test]
    fn individual_corners_round_only_the_corners_given() {
        let vals =
            [("individualCorners", Value::Bool(true)), ("topLeft", num(12.0)), ("topRight", num(0.0)), ("bottomRight", num(0.0)), ("bottomLeft", num(0.0))];
        let o = run(&vals, solid(48, 32));
        assert_eq!(alpha(&o, 0, 0), 0.0);
        for (x, y) in [(47, 0), (47, 31), (0, 31)] {
            assert_eq!(alpha(&o, x, y), 1.0, "square corner {x},{y}");
        }
    }

    #[test]
    fn radii_too_large_make_the_same_pill_as_full_percent() {
        let pixels = run(&[("radius", num(10000.0))], solid(60, 24));
        let percent = run(&[("radius", num(50.0)), ("units", Value::Enum(1))], solid(60, 24));
        assert_eq!(pixels.data, percent.data);
        // A pill: the ends are round, the long sides are straight in the middle.
        assert_eq!(alpha(&pixels, 0, 0), 0.0);
        assert_eq!(alpha(&pixels, 30, 0), 1.0);
    }

    #[test]
    fn percent_scales_with_the_layer() {
        let pct = [("radius", num(25.0)), ("units", Value::Enum(1))];
        let small = cut(&run(&pct, solid(40, 40)));
        let large = cut(&run(&pct, solid(80, 80)));
        assert!(large > small * 3, "{small} → {large}");
    }

    #[test]
    fn content_bounds_round_the_visible_pixels() {
        let mut img = Image::new(64, 64);
        for y in 20..44 {
            for x in 16..48 {
                img.set(x, y, [0.2, 0.4, 0.6, 1.0]);
            }
        }
        // The layer's corners are empty already: rounding them changes nothing.
        assert_eq!(run(&[("radius", num(8.0))], img.clone()).data, img.data);
        let o = run(&[("radius", num(8.0)), ("bounds", Value::Enum(1))], img.clone());
        assert_eq!(alpha(&o, 16, 20), 0.0);
        assert_eq!(alpha(&o, 32, 32), 1.0);
        // Padding grows the rectangle past the content, so its corners are kept.
        let o = run(&[("radius", num(8.0)), ("bounds", Value::Enum(1)), ("padding", num(6.0))], img);
        assert_eq!(alpha(&o, 16, 20), 1.0);
    }

    #[test]
    fn out_of_range_values_are_clamped_not_overflowed() {
        let huge = run(&[("radius", num(1e308))], solid(48, 32));
        assert_eq!(huge.data, run(&[("radius", num(10000.0))], solid(48, 32)).data);
        assert_eq!(alpha(&huge, 0, 0), 0.0);
        let gone = run(&[("padding", num(-1e308))], solid(48, 32));
        assert!(gone.data.iter().all(|px| *px == [0.0; 4]));
    }

    #[test]
    fn a_cut_corner_is_cleared_even_when_its_pixel_is_infinite() {
        let mut img = solid(48, 32);
        img.set(0, 0, [f32::INFINITY, 0.0, 0.0, 1.0]);
        let o = run(&[("radius", num(10.0))], img);
        assert_eq!(o.data[0], [0.0; 4]);
    }

    #[test]
    fn padding_that_empties_the_rectangle_clears_the_layer() {
        let o = run(&[("padding", num(-40.0))], solid(48, 32));
        assert!(o.data.iter().all(|px| *px == [0.0; 4]));
    }

    #[test]
    fn feather_softens_the_edge() {
        let hard = cut(&run(&[("radius", num(8.0))], solid(48, 32)));
        let soft = cut(&run(&[("radius", num(8.0)), ("feather", num(6.0))], solid(48, 32)));
        assert!(soft > hard, "{hard} → {soft}");
    }
}
