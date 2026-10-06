//! Effect Controls widgets and their maths: the angle dial, the Curves graph, the Levels
//! histogram with its input/output triangles, the crosshair and eyedropper buttons, and the
//! helpers that map viewer clicks to effect parameters (point params in layer space, colours
//! sampled from the composited frame).
//!
//! The maths lives in plain functions so it can be tested without a UI.

use effectcraft_engine::geom::{Mat3, vec2 as gv2};
use egui::{Align2, Color32, Pos2, Rect, Sense, Stroke, StrokeKind, pos2, vec2};

use crate::theme::Tokens;

// ---------------------------------------------------------------------------------------------
// Angles

/// Split an angle in degrees into whole revolutions and the remainder, both with the sign of
/// the angle (After Effects: 405° = `1x+45°`, -400° = `-1x-40°`).
pub fn split_angle(v: f64) -> (i64, f64) {
    let rev = (v / 360.0).trunc();
    let deg = v - rev * 360.0;
    (rev as i64, if deg.abs() < 1e-9 { 0.0 } else { deg })
}

/// After Effects' angle display: `0x+45.0°`.
pub fn format_angle(v: f64, decimals: usize) -> String {
    let (rev, deg) = split_angle(v);
    let sign = if deg < 0.0 { "-" } else { "+" };
    format!("{rev}x{sign}{:.decimals$}°", deg.abs())
}

/// `v` with its revolutions set to `rev`, keeping the degrees as displayed: the degrees take
/// the sign of the new turns (as [`split_angle`] shows them), so the result reads `{rev}x` with
/// the same degree number (`0x+30°` → 2 → `2x+30°`; `-1x-30°` → 1 → `1x+30°`; 0 keeps the
/// degrees' own sign). Within one sign this is a step of 360° per turn.
pub fn with_revolutions(v: f64, rev: i64) -> f64 {
    let (_, deg) = split_angle(v);
    let deg = match rev.signum() {
        1 => deg.abs(),
        -1 => -deg.abs(),
        _ => deg,
    };
    rev as f64 * 360.0 + deg
}

/// Parse typed angles: `45`, `-30.5°`, `1x+45`, `-1x-40°`, `2x` (revolutions only).
pub fn parse_angle(s: &str) -> Option<f64> {
    let s = s.trim().trim_end_matches('°').trim().replace(',', ".");
    if let Some((r, d)) = s.split_once(['x', 'X']) {
        let rev: f64 = r.trim().parse().ok()?;
        let d = d.trim();
        let deg: f64 = if d.is_empty() { 0.0 } else { d.trim_start_matches('+').trim().parse().ok()? };
        return Some(rev * 360.0 + deg);
    }
    s.parse().ok()
}

/// Dial angle (degrees, 0 = up, clockwise) of `p` around `c` in screen space.
pub fn dial_angle(c: Pos2, p: Pos2) -> f64 {
    ((p.x - c.x) as f64).atan2(-(p.y - c.y) as f64).to_degrees()
}

/// Smallest signed difference `b - a` between two angles, in (-180, 180].
pub fn wrap_delta(a: f64, b: f64) -> f64 {
    let mut d = (b - a) % 360.0;
    if d > 180.0 {
        d -= 360.0;
    } else if d <= -180.0 {
        d += 360.0;
    }
    d
}

/// The AE angle dial: a circle with a needle; dragging turns it (accumulating revolutions),
/// Shift snaps to 45°. Returns the new angle while dragging.
pub fn angle_dial(ui: &mut egui::Ui, rect: Rect, value: f64, id: egui::Id, t: &Tokens) -> (egui::Response, Option<f64>) {
    let resp = ui.interact(rect, id, Sense::click_and_drag());
    let p = ui.painter();
    let c = rect.center();
    let r = rect.width().min(rect.height()) / 2.0 - 1.0;
    p.circle_filled(c, r, t.field_bg);
    p.circle_stroke(c, r, Stroke::new(1.0, if resp.hovered() || resp.dragged() { t.text } else { t.field_border }));
    let a = (value as f32).to_radians();
    let tip = c + vec2(a.sin(), -a.cos()) * (r - 2.0);
    p.line_segment([c, tip], Stroke::new(1.5, t.accent));
    p.circle_filled(c, 2.0, t.text_dim);
    let prev_id = id.with("prev");
    let mut out = None;
    if let Some(pt) = resp.interact_pointer_pos() {
        let now = dial_angle(c, pt);
        if resp.drag_started() || resp.clicked() {
            ui.data_mut(|d| d.insert_temp(prev_id, now));
            if resp.clicked() {
                // A click points the needle there (same revolution count).
                let (rev, _) = split_angle(value);
                out = Some(rev as f64 * 360.0 + now.rem_euclid(360.0));
            }
        } else if resp.dragged() {
            let prev: f64 = ui.data(|d| d.get_temp(prev_id)).unwrap_or(now);
            let mut nv = value + wrap_delta(prev, now);
            if ui.input(|i| i.modifiers.shift) {
                nv = (nv / 45.0).round() * 45.0;
            }
            ui.data_mut(|d| d.insert_temp(prev_id, now));
            if (nv - value).abs() > 1e-9 {
                out = Some(nv);
            }
        }
    }
    (resp, out)
}

// ---------------------------------------------------------------------------------------------
// Buttons

/// The point-parameter crosshair button (click, then click in the Composition viewer).
pub fn crosshair_button(ui: &mut egui::Ui, rect: Rect, active: bool, id: egui::Id, t: &Tokens) -> egui::Response {
    let resp = ui.interact(rect, id, Sense::click());
    let p = ui.painter();
    if active {
        p.rect_filled(rect, 3.0, t.accent);
    } else if resp.hovered() {
        p.rect_filled(rect, 3.0, t.hover);
    }
    let col = if active {
        Color32::WHITE
    } else if resp.hovered() {
        t.text
    } else {
        t.text_dim
    };
    draw_crosshair(p, rect.center(), rect.width() * 0.32, col);
    resp
}

/// ⊕: a circle with a cross through it (effect point controls and the crosshair button).
pub fn draw_crosshair(p: &egui::Painter, c: Pos2, r: f32, col: Color32) {
    p.circle_stroke(c, r, Stroke::new(1.2, col));
    let e = r * 1.45;
    p.line_segment([c - vec2(e, 0.0), c + vec2(e, 0.0)], Stroke::new(1.2, col));
    p.line_segment([c - vec2(0.0, e), c + vec2(0.0, e)], Stroke::new(1.2, col));
}

/// The colour-parameter eyedropper button (click, then click in the Composition viewer).
pub fn eyedropper_button(ui: &mut egui::Ui, rect: Rect, active: bool, id: egui::Id, t: &Tokens) -> egui::Response {
    let resp = ui.interact(rect, id, Sense::click());
    let p = ui.painter();
    if active {
        p.rect_filled(rect, 3.0, t.accent);
    } else if resp.hovered() {
        p.rect_filled(rect, 3.0, t.hover);
    }
    let col = if active {
        Color32::WHITE
    } else if resp.hovered() {
        t.text
    } else {
        t.text_dim
    };
    // A pipette: a diagonal glass tube with a bulb at the top right and a tip at the bottom left.
    let r = rect.shrink(rect.width() * 0.2);
    let at = |x: f32, y: f32| pos2(r.min.x + x * r.width(), r.min.y + y * r.height());
    p.line_segment([at(0.08, 0.92), at(0.62, 0.38)], Stroke::new(2.2, col));
    p.circle_filled(at(0.76, 0.24), r.width() * 0.2, col);
    p.line_segment([at(0.5, 0.3), at(0.7, 0.5)], Stroke::new(1.4, col));
    resp
}

// ---------------------------------------------------------------------------------------------
// Viewer mapping

/// A comp-space point (viewer click) in the layer space of a layer whose layer→comp matrix is
/// `l2c` (effect point parameters are layer coordinates).
pub fn comp_to_layer(l2c: &Mat3, comp: [f64; 2]) -> Option<[f64; 2]> {
    let v = l2c.inverse()?.apply(gv2(comp[0], comp[1]));
    Some([v.x, v.y])
}

/// A layer-space point in comp space.
pub fn layer_to_comp(l2c: &Mat3, p: [f64; 2]) -> [f64; 2] {
    let v = l2c.apply(gv2(p[0], p[1]));
    [v.x, v.y]
}

/// Straight RGBA (0..1) of the displayed frame at comp point `pt` (the frame may be rendered
/// at a reduced resolution: `comp` is the comp size in pixels). `None` outside the frame.
pub fn sample_frame(img: &egui::ColorImage, comp: [f32; 2], pt: [f64; 2]) -> Option<[f32; 4]> {
    if comp[0] <= 0.0 || comp[1] <= 0.0 || pt[0] < 0.0 || pt[1] < 0.0 {
        return None;
    }
    let sx = (pt[0] / comp[0] as f64 * img.size[0] as f64).floor() as usize;
    let sy = (pt[1] / comp[1] as f64 * img.size[1] as f64).floor() as usize;
    if sx >= img.size[0] || sy >= img.size[1] {
        return None;
    }
    let c = img.pixels[sy * img.size[0] + sx];
    let a = c.a() as f32 / 255.0;
    if a <= 0.0 {
        return Some([0.0, 0.0, 0.0, 0.0]);
    }
    let un = |v: u8| ((v as f32 / 255.0) / a).min(1.0);
    Some([un(c.r()), un(c.g()), un(c.b()), a])
}

// ---------------------------------------------------------------------------------------------
// Curves

/// Curves channels: the effect's hidden string parameter ids and display names.
pub const CURVE_CHANNELS: [(&str, &str); 5] = [("rgb", "RGB"), ("red", "Red"), ("green", "Green"), ("blue", "Blue"), ("alpha", "Alpha")];

/// Editable control points of one Curves channel (sorted by x, in 0..1).
#[derive(Clone, Debug, PartialEq)]
pub struct CurvePoints(pub Vec<[f32; 2]>);

impl CurvePoints {
    pub fn identity() -> CurvePoints {
        CurvePoints(vec![[0.0, 0.0], [1.0, 1.0]])
    }
    /// Parse the effect's `"x,y x,y …"` format (identity when fewer than two points).
    pub fn parse(s: &str) -> CurvePoints {
        let mut pts: Vec<[f32; 2]> = s
            .split(|c: char| c.is_whitespace() || c == ';')
            .filter_map(|t| {
                let (x, y) = t.split_once(',')?;
                let (x, y) = (x.trim().parse::<f32>().ok()?, y.trim().parse::<f32>().ok()?);
                (x.is_finite() && y.is_finite()).then_some([x.clamp(0.0, 1.0), y.clamp(0.0, 1.0)])
            })
            .collect();
        pts.sort_by(|a, b| a[0].total_cmp(&b[0]));
        pts.dedup_by(|a, b| (a[0] - b[0]).abs() < 1e-6);
        if pts.len() < 2 { CurvePoints::identity() } else { CurvePoints(pts) }
    }
    pub fn format(&self) -> String {
        let f = |v: f32| {
            let s = format!("{v:.4}");
            let s = s.trim_end_matches('0').trim_end_matches('.').to_string();
            if s.is_empty() || s == "-0" { "0".to_string() } else { s }
        };
        self.0.iter().map(|p| format!("{},{}", f(p[0]), f(p[1]))).collect::<Vec<_>>().join(" ")
    }
    /// Insert a point (kept sorted); returns its index. A point closer than 1/100 in x to an
    /// existing one replaces it.
    pub fn insert(&mut self, x: f32, y: f32) -> usize {
        let (x, y) = (x.clamp(0.0, 1.0), y.clamp(0.0, 1.0));
        if let Some(i) = self.0.iter().position(|p| (p[0] - x).abs() < 0.01) {
            self.0[i] = [x, y];
            return i;
        }
        let i = self.0.partition_point(|p| p[0] < x);
        self.0.insert(i, [x, y]);
        i
    }
    /// Remove point `i` (a curve keeps at least two points).
    pub fn remove(&mut self, i: usize) -> bool {
        if self.0.len() <= 2 || i >= self.0.len() {
            return false;
        }
        self.0.remove(i);
        true
    }
    /// Move point `i`, keeping x strictly between its neighbours.
    pub fn move_point(&mut self, i: usize, x: f32, y: f32) {
        let n = self.0.len();
        if i >= n {
            return;
        }
        let lo = if i == 0 { 0.0 } else { self.0[i - 1][0] + 0.005 };
        let hi = if i + 1 == n { 1.0 } else { self.0[i + 1][0] - 0.005 };
        self.0[i] = [x.clamp(lo, hi.max(lo)), y.clamp(0.0, 1.0)];
    }
    /// Index of the point within `r` (graph units) of `(x, y)`.
    pub fn hit(&self, x: f32, y: f32, r: f32) -> Option<usize> {
        self.0.iter().enumerate().map(|(i, p)| (i, (p[0] - x).hypot(p[1] - y))).filter(|(_, d)| *d <= r).min_by(|a, b| a.1.total_cmp(&b.1)).map(|(i, _)| i)
    }
    /// The curve the effect renders (the same monotone spline), evaluated at `x`.
    pub fn eval(&self, x: f32) -> f32 {
        effectcraft_engine::effects::Curve::parse(&self.format()).map(|c| c.eval(x)).unwrap_or(x)
    }
    /// Pencil mode: points from a free-drawn curve sampled at `samples.len()` even x steps.
    pub fn from_samples(samples: &[f32]) -> CurvePoints {
        let n = samples.len();
        if n < 2 {
            return CurvePoints::identity();
        }
        let step = ((n - 1) as f32 / 16.0).max(1.0);
        let mut pts = vec![];
        let mut k = 0.0f32;
        while (k.round() as usize) < n {
            let i = k.round() as usize;
            pts.push([i as f32 / (n - 1) as f32, samples[i].clamp(0.0, 1.0)]);
            k += step;
        }
        if pts.last().is_some_and(|p| p[0] < 1.0) {
            pts.push([1.0, samples[n - 1].clamp(0.0, 1.0)]);
        }
        CurvePoints(pts)
    }
}

/// What the Curves graph did this frame.
#[derive(Default)]
pub struct CurveEdit {
    pub changed: Option<CurvePoints>,
    /// The gesture ended (for undo merging).
    pub done: bool,
}

/// The Curves graph: grid, identity diagonal, the curve, its points. Click adds a point, drag
/// moves one, dragging a point off the graph (or Alt-clicking it) removes it. In pencil mode a
/// drag draws the curve freehand.
pub fn curves_graph(ui: &mut egui::Ui, rect: Rect, pts: &CurvePoints, channel: usize, pencil: bool, id: egui::Id, t: &Tokens) -> (egui::Response, CurveEdit) {
    let resp = ui.interact(rect.expand(6.0), id, Sense::click_and_drag());
    let p = ui.painter().with_clip_rect(rect.expand(8.0));
    p.rect_filled(rect, 0.0, Color32::from_rgb(0x1c, 0x1c, 0x1c));
    for k in 1..4 {
        let f = k as f32 / 4.0;
        let x = rect.min.x + rect.width() * f;
        let y = rect.min.y + rect.height() * f;
        p.line_segment([pos2(x, rect.min.y), pos2(x, rect.max.y)], Stroke::new(1.0, Color32::from_white_alpha(18)));
        p.line_segment([pos2(rect.min.x, y), pos2(rect.max.x, y)], Stroke::new(1.0, Color32::from_white_alpha(18)));
    }
    p.line_segment([rect.left_bottom(), rect.right_top()], Stroke::new(1.0, Color32::from_white_alpha(40)));
    p.rect_stroke(rect, 0.0, Stroke::new(1.0, t.field_border), StrokeKind::Outside);
    let col = match channel {
        1 => Color32::from_rgb(0xf0, 0x60, 0x60),
        2 => Color32::from_rgb(0x60, 0xe0, 0x70),
        3 => Color32::from_rgb(0x60, 0x90, 0xff),
        4 => Color32::from_gray(0xb0),
        _ => Color32::from_gray(0xe8),
    };
    let to_s = |x: f32, y: f32| pos2(rect.min.x + x * rect.width(), rect.max.y - y * rect.height());
    let to_g = |s: Pos2| [((s.x - rect.min.x) / rect.width()), ((rect.max.y - s.y) / rect.height())];
    let spline = effectcraft_engine::effects::Curve::parse(&pts.format());
    let line: Vec<Pos2> = (0..=96)
        .map(|i| {
            let x = i as f32 / 96.0;
            to_s(x, spline.as_ref().map(|c| c.eval(x)).unwrap_or(x).clamp(0.0, 1.0))
        })
        .collect();
    p.add(egui::Shape::line(line, Stroke::new(1.5, col)));
    let drag_id = id.with("drag");
    let pencil_id = id.with("pencil");
    let dragging: Option<usize> = ui.data(|d| d.get_temp(drag_id));
    for (i, q) in pts.0.iter().enumerate() {
        let s = to_s(q[0], q[1]);
        let r = Rect::from_center_size(s, vec2(7.0, 7.0));
        if dragging == Some(i) {
            p.rect_filled(r, 0.0, col);
        } else {
            p.rect_filled(r, 0.0, Color32::from_rgb(0x1c, 0x1c, 0x1c));
            p.rect_stroke(r, 0.0, Stroke::new(1.2, col), StrokeKind::Inside);
        }
    }
    let mut edit = CurveEdit::default();
    let hit_r = 7.0 / rect.width();
    let alt = ui.input(|i| i.modifiers.alt);
    if pencil {
        if resp.drag_started() {
            ui.data_mut(|d| d.insert_temp(pencil_id, (0..=64).map(|i| pts.eval(i as f32 / 64.0)).collect::<Vec<f32>>()));
        }
        if (resp.dragged() || resp.clicked())
            && let Some(pt) = resp.interact_pointer_pos()
        {
            let mut samples: Vec<f32> = ui.data(|d| d.get_temp(pencil_id)).unwrap_or_else(|| (0..=64).map(|i| pts.eval(i as f32 / 64.0)).collect());
            let g = to_g(pt);
            let i = (g[0].clamp(0.0, 1.0) * 64.0).round() as usize;
            samples[i.min(64)] = g[1].clamp(0.0, 1.0);
            ui.data_mut(|d| d.insert_temp(pencil_id, samples.clone()));
            edit.changed = Some(CurvePoints::from_samples(&samples));
        }
        if resp.drag_stopped() {
            ui.data_mut(|d| d.remove::<Vec<f32>>(pencil_id));
            edit.done = true;
        }
        return (resp, edit);
    }
    if resp.drag_started()
        && let Some(pt) = ui.input(|i| i.pointer.press_origin())
    {
        let g = to_g(pt);
        let i = match pts.hit(g[0], g[1], hit_r) {
            Some(i) => i,
            None => {
                let mut np = pts.clone();
                let i = np.insert(g[0], g[1]);
                edit.changed = Some(np);
                i
            }
        };
        ui.data_mut(|d| d.insert_temp(drag_id, i));
    } else if resp.dragged()
        && let (Some(i), Some(pt)) = (dragging, resp.interact_pointer_pos())
    {
        let g = to_g(pt);
        let mut np = pts.clone();
        np.move_point(i, g[0], g[1]);
        if np != *pts {
            edit.changed = Some(np);
        }
    }
    if resp.drag_stopped() {
        if let (Some(i), Some(pt)) = (dragging, resp.interact_pointer_pos())
            && !rect.expand(14.0).contains(pt)
        {
            let mut np = pts.clone();
            if np.remove(i) {
                edit.changed = Some(np);
            }
        }
        ui.data_mut(|d| d.remove::<usize>(drag_id));
        edit.done = true;
    }
    if resp.clicked()
        && let Some(pt) = resp.interact_pointer_pos()
    {
        let g = to_g(pt);
        let mut np = pts.clone();
        match pts.hit(g[0], g[1], hit_r) {
            Some(i) if alt => {
                np.remove(i);
            }
            Some(_) => {}
            None => {
                np.insert(g[0], g[1]);
            }
        }
        if np != *pts {
            edit.changed = Some(np);
        }
        edit.done = true;
    }
    (resp, edit)
}

// ---------------------------------------------------------------------------------------------
// Levels

/// Levels channels (for Levels (Individual Controls)): parameter id prefixes and names.
pub const LEVELS_CHANNELS: [(&str, &str); 5] = [("rgb", "RGB"), ("red", "Red"), ("green", "Green"), ("blue", "Blue"), ("alpha", "Alpha")];

/// Levels' mapping (same as the effect): input black/white, gamma, output black/white.
pub fn levels_map(v: f32, ib: f32, iw: f32, g: f32, ob: f32, ow: f32) -> f32 {
    let t = ((v - ib) / (iw - ib).max(1e-6)).clamp(0.0, 1.0);
    ob + (ow - ob) * t.powf(1.0 / g.max(0.01))
}

/// Where the gamma (grey) triangle sits between the input black and white triangles: the input
/// level that maps to mid-grey.
pub fn gamma_handle(ib: f64, iw: f64, gamma: f64) -> f64 {
    ib + (iw - ib) * 0.5f64.powf(gamma.max(0.01))
}

/// Gamma for a grey triangle dragged to input level `x` (inverse of [`gamma_handle`]).
pub fn gamma_from_handle(ib: f64, iw: f64, x: f64) -> f64 {
    let f = ((x - ib) / (iw - ib).max(1e-6)).clamp(0.01, 0.99);
    (f.ln() / 0.5f64.ln()).clamp(0.1, 10.0)
}

/// 256-bin histograms of a frame: luminance, red, green, blue, alpha (straight colour).
pub fn histogram(img: &egui::ColorImage) -> [[u32; 256]; 5] {
    let mut h = [[0u32; 256]; 5];
    let n = img.pixels.len();
    let step = (n / 250_000).max(1);
    for c in img.pixels.iter().step_by(step) {
        let a = c.a();
        h[4][a as usize] += 1;
        if a == 0 {
            continue;
        }
        let un = |v: u8| ((v as u32 * 255 + a as u32 / 2) / a as u32).min(255) as usize;
        let (r, g, b) = (un(c.r()), un(c.g()), un(c.b()));
        h[1][r] += 1;
        h[2][g] += 1;
        h[3][b] += 1;
        let l = (0.2126 * r as f32 + 0.7152 * g as f32 + 0.0722 * b as f32).round() as usize;
        h[0][l.min(255)] += 1;
    }
    h
}

/// Draw a histogram into `rect` (bars scaled to the 99th-percentile bin so a spike doesn't
/// flatten the rest).
pub fn draw_histogram(p: &egui::Painter, rect: Rect, bins: &[u32; 256], col: Color32) {
    p.rect_filled(rect, 0.0, Color32::from_rgb(0x1c, 0x1c, 0x1c));
    draw_histogram_overlay(p, rect, bins, col);
}

/// Bars of a histogram over an existing background (several channels overlaid).
pub fn draw_histogram_overlay(p: &egui::Painter, rect: Rect, bins: &[u32; 256], col: Color32) {
    let mut sorted: Vec<u32> = bins.to_vec();
    sorted.sort_unstable();
    let top = sorted[252].max(1) as f32;
    let w = rect.width() / 256.0;
    for (i, &v) in bins.iter().enumerate() {
        if v == 0 {
            continue;
        }
        let h = (v as f32 / top).min(1.0) * rect.height();
        let x = rect.min.x + i as f32 * w;
        p.rect_filled(Rect::from_min_max(pos2(x, rect.max.y - h), pos2(x + w.max(1.0), rect.max.y)), 0.0, col);
    }
}

/// A Levels triangle marker pointing up at `x` on top of `y`.
pub fn triangle_marker(p: &egui::Painter, x: f32, y: f32, fill: Color32, t: &Tokens) {
    let pts = vec![pos2(x, y), pos2(x + 5.0, y + 8.0), pos2(x - 5.0, y + 8.0)];
    p.add(egui::Shape::convex_polygon(pts, fill, Stroke::new(1.0, t.field_border)));
}

/// A draggable Levels triangle on a 0..1 track; returns the new value while dragging.
#[allow(clippy::too_many_arguments)]
pub fn level_handle(ui: &mut egui::Ui, track: Rect, value: f64, fill: Color32, id: egui::Id, t: &Tokens) -> (Rect, Option<f64>) {
    let x = track.min.x + track.width() * value.clamp(0.0, 1.0) as f32;
    let hr = Rect::from_center_size(pos2(x, track.min.y + 4.0), vec2(12.0, 12.0));
    let resp = ui.interact(hr, id, Sense::drag());
    triangle_marker(ui.painter(), x, track.min.y, if resp.hovered() || resp.dragged() { t.accent } else { fill }, t);
    let mut out = None;
    if resp.dragged()
        && let Some(pt) = resp.interact_pointer_pos()
    {
        out = Some((((pt.x - track.min.x) / track.width()) as f64).clamp(0.0, 1.0));
    }
    (hr, out)
}

/// Text in a small label style.
pub fn label(p: &egui::Painter, pos: Pos2, text: &str, t: &Tokens) {
    p.text(pos, Align2::LEFT_CENTER, text, Tokens::ui(11.0), t.text_dim);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn angle_formatting_matches_after_effects() {
        assert_eq!(format_angle(0.0, 1), "0x+0.0°");
        assert_eq!(format_angle(45.0, 1), "0x+45.0°");
        assert_eq!(format_angle(405.0, 1), "1x+45.0°");
        assert_eq!(format_angle(-30.5, 1), "0x-30.5°");
        assert_eq!(format_angle(-400.0, 1), "-1x-40.0°");
        assert_eq!(format_angle(720.0, 0), "2x+0°");
        assert_eq!(split_angle(-360.0), (-1, 0.0));
    }

    #[test]
    fn setting_revolutions_keeps_the_degrees() {
        assert_eq!(with_revolutions(30.0, 3), 1110.0);
        assert_eq!(with_revolutions(405.0, 0), 45.0);
        assert_eq!(with_revolutions(405.0, -2), -765.0);
        assert_eq!(with_revolutions(-390.0, 1), 390.0);
        assert_eq!(with_revolutions(-390.0, 0), -30.0);
        assert_eq!(with_revolutions(-390.0, -3), -1110.0);
        for (v, r) in [(30.0, 3), (-390.0, 1), (-30.0, -2), (725.5, 0)] {
            assert_eq!(split_angle(with_revolutions(v, r)).0, r, "{v} → {r}x");
        }
    }

    #[test]
    fn angle_parsing_round_trips() {
        for v in [0.0, 45.0, 405.0, -30.5, -400.0, 1080.25] {
            assert!((parse_angle(&format_angle(v, 2)).unwrap() - v).abs() < 1e-9, "{v}");
        }
        assert_eq!(parse_angle("90"), Some(90.0));
        assert_eq!(parse_angle(" -12,5° "), Some(-12.5));
        assert_eq!(parse_angle("2x"), Some(720.0));
        assert_eq!(parse_angle("1x-10"), Some(350.0));
        assert_eq!(parse_angle("abc"), None);
    }

    #[test]
    fn dial_maths() {
        let c = pos2(0.0, 0.0);
        assert!((dial_angle(c, pos2(0.0, -10.0)) - 0.0).abs() < 1e-9, "up is 0°");
        assert!((dial_angle(c, pos2(10.0, 0.0)) - 90.0).abs() < 1e-9, "right is 90° (clockwise)");
        assert!((dial_angle(c, pos2(-10.0, 0.0)) + 90.0).abs() < 1e-9);
        // Crossing the bottom (±180°) accumulates instead of jumping a revolution.
        assert!((wrap_delta(170.0, -170.0) - 20.0).abs() < 1e-9);
        assert!((wrap_delta(-170.0, 170.0) + 20.0).abs() < 1e-9);
        assert!((wrap_delta(10.0, 30.0) - 20.0).abs() < 1e-9);
    }

    #[test]
    fn curve_points_insert_remove_and_format() {
        let mut c = CurvePoints::parse("0,0 1,1");
        assert_eq!(c, CurvePoints::identity());
        let i = c.insert(0.5, 0.7);
        assert_eq!(i, 1);
        assert_eq!(c.0, vec![[0.0, 0.0], [0.5, 0.7], [1.0, 1.0]]);
        // A point close in x replaces the existing one.
        assert_eq!(c.insert(0.505, 0.6), 1);
        assert_eq!(c.0.len(), 3);
        assert_eq!(c.format(), "0,0 0.505,0.6 1,1");
        assert_eq!(CurvePoints::parse(&c.format()), c);
        // Moving keeps x between neighbours.
        c.move_point(1, 1.5, 2.0);
        assert!(c.0[1][0] < 1.0 && c.0[1][1] == 1.0);
        assert!(c.remove(1));
        assert!(!c.remove(0), "two points stay");
        assert_eq!(c.hit(0.02, 0.01, 0.05), Some(0));
        assert_eq!(c.hit(0.5, 0.5, 0.05), None);
        // Garbage parses to the identity.
        assert_eq!(CurvePoints::parse("x"), CurvePoints::identity());
        assert_eq!(CurvePoints::parse("0.8,0.2 0.1,0.9").0, vec![[0.1, 0.9], [0.8, 0.2]], "sorted");
    }

    #[test]
    fn curve_eval_matches_the_effect_lut() {
        let mut c = CurvePoints::identity();
        assert!((c.eval(0.3) - 0.3).abs() < 1e-6);
        c.insert(0.5, 0.75);
        let fx = effectcraft_engine::effects::Curve::parse(&c.format()).unwrap();
        for i in 0..=20 {
            let x = i as f32 / 20.0;
            assert!((c.eval(x) - fx.eval(x)).abs() < 1e-6);
        }
        assert!((c.eval(0.5) - 0.75).abs() < 1e-3, "passes through its points");
        assert!(c.eval(0.25) > 0.25, "lifted midtones");
        // Pencil samples become a point list the effect accepts.
        let s: Vec<f32> = (0..=64).map(|i| 1.0 - i as f32 / 64.0).collect();
        let p = CurvePoints::from_samples(&s);
        assert!(p.0.len() >= 16);
        assert_eq!(p.0.first(), Some(&[0.0, 1.0]));
        assert_eq!(p.0.last(), Some(&[1.0, 0.0]));
        assert!((p.eval(0.25) - 0.75).abs() < 0.02, "inverted");
    }

    #[test]
    fn levels_mapping_and_gamma_handle() {
        assert_eq!(levels_map(0.5, 0.0, 1.0, 1.0, 0.0, 1.0), 0.5);
        assert_eq!(levels_map(0.1, 0.2, 0.8, 1.0, 0.0, 1.0), 0.0, "below input black clips");
        assert!((levels_map(0.5, 0.0, 1.0, 1.0, 0.2, 0.6) - 0.4).abs() < 1e-6, "output range");
        // The grey triangle sits at the input level that maps to mid-grey.
        for (ib, iw, g) in [(0.0, 1.0, 1.0), (0.1, 0.9, 2.0), (0.2, 0.7, 0.5)] {
            let x = gamma_handle(ib, iw, g);
            let out = levels_map(x as f32, ib as f32, iw as f32, g as f32, 0.0, 1.0);
            assert!((out - 0.5).abs() < 1e-4, "{ib} {iw} {g}: {out}");
            assert!((gamma_from_handle(ib, iw, x) - g).abs() < 1e-6);
        }
        assert_eq!(gamma_handle(0.0, 1.0, 1.0), 0.5);
        // Dragging the grey triangle left brightens (gamma > 1).
        assert!(gamma_from_handle(0.0, 1.0, 0.3) > 1.0);
        assert!(gamma_from_handle(0.0, 1.0, 0.7) < 1.0);
    }

    #[test]
    fn eyedropper_samples_straight_colour() {
        let mut img = egui::ColorImage::new([4, 2], vec![Color32::TRANSPARENT; 8]);
        img.pixels[4 + 3] = Color32::from_rgba_premultiplied(128, 64, 0, 128); // (3, 1), half alpha
        img.pixels[0] = Color32::from_rgb(255, 0, 0);
        // The frame is rendered at half resolution of an 8 × 4 comp.
        let c = sample_frame(&img, [8.0, 4.0], [7.5, 3.0]).unwrap();
        assert!((c[0] - 1.0).abs() < 0.01 && (c[1] - 0.5).abs() < 0.01 && c[2] == 0.0, "{c:?}");
        assert!((c[3] - 128.0 / 255.0).abs() < 1e-6);
        assert_eq!(sample_frame(&img, [8.0, 4.0], [0.5, 0.5]), Some([1.0, 0.0, 0.0, 1.0]));
        assert_eq!(sample_frame(&img, [8.0, 4.0], [8.0, 1.0]), None, "outside");
        assert_eq!(sample_frame(&img, [8.0, 4.0], [-1.0, 1.0]), None);
    }

    #[test]
    fn viewer_points_map_to_layer_space() {
        // A 200 × 100 layer, anchor at its centre, positioned at (320, 180), scaled 200%,
        // rotated 90°.
        let l2c = Mat3::layer_2d(gv2(100.0, 50.0), gv2(320.0, 180.0), gv2(200.0, 200.0), 90.0);
        // The layer centre is at the position.
        let p = comp_to_layer(&l2c, [320.0, 180.0]).unwrap();
        assert!((p[0] - 100.0).abs() < 1e-9 && (p[1] - 50.0).abs() < 1e-9, "{p:?}");
        // 20 comp px to the right is 10 layer px "up" (rotated 90° clockwise, scale 2).
        let q = comp_to_layer(&l2c, [340.0, 180.0]).unwrap();
        assert!((q[0] - 100.0).abs() < 1e-9 && (q[1] - 40.0).abs() < 1e-9, "{q:?}");
        let back = layer_to_comp(&l2c, q);
        assert!((back[0] - 340.0).abs() < 1e-9 && (back[1] - 180.0).abs() < 1e-9);
        // An unscaled layer at the comp origin: layer space = comp space.
        let id = Mat3::layer_2d(gv2(0.0, 0.0), gv2(0.0, 0.0), gv2(100.0, 100.0), 0.0);
        assert_eq!(comp_to_layer(&id, [12.0, 34.0]), Some([12.0, 34.0]));
    }
}
