//! Visual editors for effect parameters stored as hidden or text parameters (Effect Controls):
//!
//! - Lumetri Color ▸ Curves ▸ RGB Curves: master / red / green / blue curve graphs (channel
//!   swatches like After Effects'), and ▸ Hue Saturation Curves: Hue vs Sat, Hue vs Hue, Hue vs
//!   Luma, Luma vs Sat and Sat vs Sat graphs over their colour gradients (y = ½ is neutral);
//! - Colorama ▸ Output Cycle: the palette wheel, with draggable colour stops (click the ring to
//!   add one, Alt-click or drag one off to remove it, select one to pick its colour); editing a
//!   preset palette turns it into Custom;
//! - Glow: the colour map (A & B Colors: a preview strip of the gradient over glow brightness;
//!   Arbitrary Map: red / green / blue curve graphs and the resulting strip);
//! - Reshape: correspondence point list buttons here, and on the Composition viewer the source
//!   and destination mask outlines with draggable correspondence handles ([`reshape_overlay`]);
//! - EXtractoR: a Layer popup of the footage's OpenEXR layers, and Red / Green / Blue / Alpha
//!   popups of its channels ([`channel_popup`]);
//! - Mesh Warp: on the Composition viewer, the distortion mesh's grid over the layer with a
//!   draggable handle on every vertex ([`mesh_warp_overlay`], #303).
//!
//! Every edit is a `prop.set` engine command (undoable; one drag = one undo step) and every
//! handle registers an automation id.

use crate::i18n::{tr, tr_args};
use effectcraft_engine::effects::{ColoramaPalette, OffsetCurve};
use effectcraft_engine::geom::Mat3;
use effectcraft_engine::keyframe::Value;
use effectcraft_engine::project::{GroupKind, Layer, PropGroup};
use effectcraft_engine::render::EvalCtx;
use egui::{Align2, Color32, Pos2, Rect, Sense, Stroke, StrokeKind, pos2, vec2};
use serde_json::json;

use super::effect_controls::gesture_key;
use super::fx_widgets as fw;
use crate::theme::Tokens;
use crate::{EffectcraftApp, widgets};

type Actions = Vec<(String, serde_json::Value)>;

pub const LUMETRI: &str = "ec.color.lumetri";
pub const COLORAMA: &str = "ec.color.colorama";
pub const GLOW: &str = "ec.stylize.glow";
pub const RESHAPE: &str = "ec.distort.reshape";
pub const EXTRACTOR: &str = "ec.3d.extractor";
pub const MESH_WARP: &str = "ec.distort.meshwarp";

fn str_value(layer: &Layer, ectx: &EvalCtx, g: &PropGroup, id: &str) -> String {
    match g.get(id).map(|p| ectx.value(layer, p)) {
        Some(Value::Str(s)) => s,
        _ => String::new(),
    }
}

fn set_str(actions: &mut Actions, layer: &Layer, g: &PropGroup, id: &str, v: String, merge: Option<String>) {
    let Some(p) = g.get(id) else { return };
    let mut a = json!({"layer": layer.id.0, "prop": p.uid, "value": v});
    if let Some(m) = merge {
        a["merge"] = json!(m);
    }
    actions.push(("prop.set".into(), a));
}

/// Register automation ids for a curve's control points (`<prefix>.point.<i>`).
fn register_points(app: &mut EffectcraftApp, prefix: &str, gr: Rect, pts: &[[f32; 2]]) {
    for (i, q) in pts.iter().enumerate() {
        let s = pos2(gr.min.x + q[0] * gr.width(), gr.max.y - q[1] * gr.height());
        app.auto.add(&format!("{prefix}.point.{i}"), Rect::from_center_size(s, vec2(9.0, 9.0)), &format!("Point {}", i + 1));
    }
}

/// Small coloured tabs (`names[i]`, swatch `cols[i]`); returns the clicked one.
fn tabs(app: &mut EffectcraftApp, ui: &mut egui::Ui, p: &egui::Painter, at: Pos2, cur: usize, (names, cols): (&[&str], &[Color32]), id: &str) -> Option<usize> {
    let t = app.tokens;
    let (mut x, cy) = (at.x, at.y);
    let mut out = None;
    for (i, name) in names.iter().enumerate() {
        let w = if cols.is_empty() { 16.0 + name.chars().count() as f32 * 6.2 } else { 20.0 };
        let r = Rect::from_min_size(pos2(x, cy - 9.0), vec2(w, 18.0));
        let resp = ui.interact(r, egui::Id::unique((id, i)), Sense::click());
        let sel = i == cur;
        if sel {
            p.rect_filled(r, 3.0, t.row_selected);
        } else if resp.hovered() {
            p.rect_filled(r, 3.0, t.hover);
        }
        match cols.get(i) {
            Some(c) => {
                p.circle_filled(r.center(), 5.5, *c);
                if sel {
                    p.circle_stroke(r.center(), 7.0, Stroke::new(1.0, t.text));
                }
            }
            None => {
                p.text(r.center(), Align2::CENTER_CENTER, *name, Tokens::ui(11.0), if sel { t.text } else { t.text_dim });
            }
        }
        app.auto.add(&format!("{id}.{i}"), r, name);
        if resp.clicked() {
            out = Some(i);
        }
        resp.on_hover_text(*name);
        x += w + 4.0;
    }
    out
}

/// A small "Reset" link centred in `r`, registered under the automation id `id`; returns whether
/// it was clicked.
fn reset_link(app: &mut EffectcraftApp, ui: &mut egui::Ui, p: &egui::Painter, r: Rect, id: &str) -> bool {
    let t = app.tokens;
    let resp = ui.interact(r, egui::Id::unique(id), Sense::click());
    p.text(r.center(), Align2::CENTER_CENTER, tr("Reset"), Tokens::ui(11.5), if resp.hovered() { t.hot_text } else { t.text_dim });
    app.auto.add(id, r, "Reset");
    resp.clicked()
}

fn graph_size(width: f32) -> f32 {
    (width - 72.0).clamp(120.0, 256.0)
}

// ---------------------------------------------------------------------------------------------
// Inline editors (inside a parameter group)

/// Height of the editor shown at the top of group `path` (spec-id path with a trailing `/`)
/// of `effect` (0 when none).
pub fn inline_height(effect: &str, path: &str, width: f32) -> f32 {
    match (effect, path) {
        (LUMETRI, "curves/rgbCurves/") => 30.0 + graph_size(width) + 22.0,
        (LUMETRI, "curves/hueSaturationCurves/") => 30.0 + graph_size(width) * 0.6 + 34.0,
        (COLORAMA, "outputCycle/") => 30.0 + wheel_size(width) + 34.0,
        _ => 0.0,
    }
}

/// Draw the editor of group `g` (at `path`) in `r`. `root`: the effect's group.
#[allow(clippy::too_many_arguments)]
pub fn inline_editor(
    app: &mut EffectcraftApp,
    ui: &mut egui::Ui,
    p: &egui::Painter,
    layer: &Layer,
    root: &PropGroup,
    g: &PropGroup,
    effect: &str,
    path: &str,
    ectx: &EvalCtx,
    r: Rect,
    actions: &mut Actions,
) {
    match (effect, path) {
        (LUMETRI, "curves/rgbCurves/") => rgb_curves(app, ui, p, layer, root, g, ectx, r, actions),
        (LUMETRI, "curves/hueSaturationCurves/") => huesat_curves(app, ui, p, layer, root, g, ectx, r, actions),
        (COLORAMA, "outputCycle/") => colorama_wheel(app, ui, p, layer, root, g, ectx, r, actions),
        _ => {}
    }
}

const RGB_CURVES: [(&str, &str); 4] = [("master", "Master"), ("red", "Red"), ("green", "Green"), ("blue", "Blue")];

/// The Lumetri RGB Curves editor for effect group `g`: channel tabs (Master, Red, Green, Blue),
/// the selected channel's curve graph with its Reset, and the point positions registered for
/// automation. A curve edit or Reset is queued on `actions` as a `prop.set` of that channel's
/// curve string; the visible channel is remembered in `app.ui.fx_curve_channel` per group.
#[allow(clippy::too_many_arguments)]
fn rgb_curves(
    app: &mut EffectcraftApp,
    ui: &mut egui::Ui,
    p: &egui::Painter,
    layer: &Layer,
    root: &PropGroup,
    g: &PropGroup,
    ectx: &EvalCtx,
    r: Rect,
    actions: &mut Actions,
) {
    let t = app.tokens;
    let euid = root.uid;
    let prefix = format!("effectControls.effect.{euid}.lumetri.rgbCurves");
    let ch = app.ui.fx_curve_channel.get(&g.uid).copied().unwrap_or(0).min(3);
    let x0 = r.min.x + 38.0;
    let cy = r.min.y + 15.0;
    let cols = [Color32::from_gray(0xe8), Color32::from_rgb(0xf0, 0x50, 0x50), Color32::from_rgb(0x50, 0xd0, 0x60), Color32::from_rgb(0x50, 0x80, 0xff)];
    let names: Vec<&str> = RGB_CURVES.iter().map(|c| c.1).collect();
    if let Some(n) = tabs(app, ui, p, pos2(x0, cy), ch, (&names, &cols), &format!("{prefix}.channel")) {
        app.ui.fx_curve_channel.insert(g.uid, n);
    }
    let id = RGB_CURVES[ch].0;
    let cur = fw::CurvePoints::parse(&str_value(layer, ectx, g, id));
    let size = graph_size(r.width());
    if reset_link(app, ui, p, Rect::from_min_size(pos2(x0 + size - 44.0, cy - 9.0), vec2(44.0, 18.0)), &format!("{prefix}.reset")) {
        set_str(actions, layer, g, id, fw::CurvePoints::identity().format(), None);
    }
    let gr = Rect::from_min_size(pos2(x0, r.min.y + 30.0), vec2(size, size));
    let gid = egui::Id::unique(("lumetri-rgb", euid, ch));
    let (resp, edit) = fw::curves_graph(ui, gr, &cur, ch, fw::CurveMode { pencil: false, natural: false }, gid, &t);
    app.auto.add(&format!("{prefix}.graph"), gr, "RGB Curves");
    register_points(app, &prefix, gr, &cur.0);
    let key = gesture_key(ui, gid, resp.drag_started() || resp.clicked());
    if let Some(np) = edit.changed {
        set_str(actions, layer, g, id, np.format(), Some(key));
    }
    p.text(pos2(gr.min.x, gr.max.y + 11.0), Align2::LEFT_CENTER, tr("Click adds a point; drag one off to remove"), Tokens::ui(10.5), t.text_faint);
}

/// A hue / saturation curve's points (`x,y` in 0..1, y = ½ neutral), any count (empty = flat).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct OffsetPoints(pub Vec<[f32; 2]>);

impl OffsetPoints {
    pub fn parse(s: &str) -> OffsetPoints {
        let mut v: Vec<[f32; 2]> = s
            .split(|c: char| c.is_whitespace() || c == ';')
            .filter_map(|tk| {
                let (x, y) = tk.split_once(',')?;
                let (x, y) = (x.trim().parse::<f32>().ok()?, y.trim().parse::<f32>().ok()?);
                (x.is_finite() && y.is_finite()).then_some([x.clamp(0.0, 1.0), y.clamp(0.0, 1.0)])
            })
            .collect();
        v.sort_by(|a, b| a[0].total_cmp(&b[0]));
        OffsetPoints(v)
    }
    pub fn format(&self) -> String {
        let f = |v: f32| {
            let s = format!("{v:.4}");
            let s = s.trim_end_matches('0').trim_end_matches('.').to_string();
            if s.is_empty() || s == "-0" { "0".into() } else { s }
        };
        self.0.iter().map(|q| format!("{},{}", f(q[0]), f(q[1]))).collect::<Vec<_>>().join(" ")
    }
    pub fn insert(&mut self, x: f32, y: f32) -> usize {
        let i = self.0.partition_point(|q| q[0] < x);
        self.0.insert(i, [x.clamp(0.0, 1.0), y.clamp(0.0, 1.0)]);
        i
    }
    /// Move point `i`, keeping the order.
    pub fn move_point(&mut self, i: usize, x: f32, y: f32) {
        let n = self.0.len();
        if i >= n {
            return;
        }
        let lo = if i == 0 { 0.0 } else { self.0[i - 1][0] + 0.002 };
        let hi = if i + 1 == n { 1.0 } else { self.0[i + 1][0] - 0.002 };
        self.0[i] = [x.clamp(lo, hi.max(lo)), y.clamp(0.0, 1.0)];
    }
    pub fn hit(&self, x: f32, y: f32, r: [f32; 2]) -> Option<usize> {
        self.0
            .iter()
            .enumerate()
            .map(|(i, q)| (i, ((q[0] - x) / r[0]).hypot((q[1] - y) / r[1])))
            .filter(|(_, d)| *d <= 1.0)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(i, _)| i)
    }
}

/// Lumetri's hue / saturation curves: parameter id, tab name, x axis (0 hue, 1 luma, 2 sat),
/// whether x wraps around.
const HUESAT: [(&str, &str, u8, bool); 5] = [
    ("hueVsSat", "Hue vs Sat", 0, true),
    ("hueVsHue", "Hue vs Hue", 0, true),
    ("hueVsLuma", "Hue vs Luma", 0, true),
    ("lumaVsSat", "Luma vs Sat", 1, false),
    ("satVsSat", "Sat vs Sat", 2, false),
];

fn hue_color(h: f32) -> Color32 {
    let c = egui::ecolor::Hsva::new(h.rem_euclid(1.0), 0.85, 0.9, 1.0);
    Color32::from(c)
}

/// The Lumetri Hue/Saturation Curves editor for effect group `g`: tabs for the Hue vs Sat,
/// Hue vs Hue, Hue vs Luma, Luma vs Sat and Sat vs Sat curves, drawn as a neutral-relative offset
/// curve over the axis' colour band, with point drag/add/remove gestures and a Reset. Changes are
/// queued on `actions` as a `prop.set` of the selected curve's point list; the visible tab is kept
/// in `app.ui.fx_curve_channel` per group.
#[allow(clippy::too_many_arguments)]
fn huesat_curves(
    app: &mut EffectcraftApp,
    ui: &mut egui::Ui,
    p: &egui::Painter,
    layer: &Layer,
    root: &PropGroup,
    g: &PropGroup,
    ectx: &EvalCtx,
    r: Rect,
    actions: &mut Actions,
) {
    let t = app.tokens;
    let euid = root.uid;
    let prefix = format!("effectControls.effect.{euid}.lumetri.hueSatCurves");
    let k = app.ui.fx_curve_channel.get(&g.uid).copied().unwrap_or(0).min(4);
    let x0 = r.min.x + 38.0;
    let cy = r.min.y + 15.0;
    let names: Vec<&str> = HUESAT.iter().map(|c| c.1).collect();
    if let Some(n) = tabs(app, ui, p, pos2(x0, cy), k, (&names, &[]), &format!("{prefix}.curve")) {
        app.ui.fx_curve_channel.insert(g.uid, n);
    }
    let (id, name, axis, periodic) = HUESAT[k];
    let cur = OffsetPoints::parse(&str_value(layer, ectx, g, id));
    let size = graph_size(r.width());
    let gr = Rect::from_min_size(pos2(x0, r.min.y + 30.0), vec2(size, size * 0.6));
    // Background: the x axis' colours (hue spectrum, grey ramp) as a band, the neutral line.
    p.rect_filled(gr, 0.0, Color32::from_rgb(0x1c, 0x1c, 0x1c));
    let n = 48;
    for i in 0..n {
        let f = (i as f32 + 0.5) / n as f32;
        let c = match axis {
            0 => hue_color(f),
            1 => Color32::from_gray((f * 255.0) as u8),
            _ => Color32::from(egui::ecolor::Hsva::new(0.0, f, 0.85, 1.0)),
        };
        let x = gr.min.x + gr.width() * i as f32 / n as f32;
        let band = Rect::from_min_max(pos2(x, gr.max.y - 8.0), pos2(x + gr.width() / n as f32 + 0.5, gr.max.y));
        p.rect_filled(band, 0.0, c);
        p.rect_filled(Rect::from_min_max(pos2(band.min.x, gr.min.y), pos2(band.max.x, gr.max.y - 8.0)), 0.0, c.gamma_multiply(0.12));
    }
    let to_s = |x: f32, y: f32| pos2(gr.min.x + x * gr.width(), gr.max.y - y * gr.height());
    p.line_segment([to_s(0.0, 0.5), to_s(1.0, 0.5)], Stroke::new(1.0, Color32::from_white_alpha(60)));
    p.rect_stroke(gr, 0.0, Stroke::new(1.0, t.field_border), StrokeKind::Outside);
    let curve = OffsetCurve::parse(&cur.format(), periodic);
    let line: Vec<Pos2> = (0..=96)
        .map(|i| {
            let x = i as f32 / 96.0;
            to_s(x, 0.5 + curve.as_ref().map(|c| c.at(x)).unwrap_or(0.0))
        })
        .collect();
    p.add(egui::Shape::line(line, Stroke::new(1.5, Color32::from_gray(0xe8))));
    let gid = egui::Id::unique(("lumetri-hs", euid, k));
    let resp = ui.interact(gr.expand(6.0), gid, Sense::click_and_drag());
    app.auto.add(&format!("{prefix}.graph"), gr, name);
    let drag_id = gid.with("drag");
    let dragging: Option<usize> = ui.data(|d| d.get_temp(drag_id));
    for (i, q) in cur.0.iter().enumerate() {
        let s = to_s(q[0], q[1]);
        let hr = Rect::from_center_size(s, vec2(7.0, 7.0));
        p.circle_filled(s, 3.5, if dragging == Some(i) { Color32::WHITE } else { Color32::from_rgb(0x1c, 0x1c, 0x1c) });
        p.circle_stroke(s, 3.5, Stroke::new(1.2, Color32::WHITE));
        app.auto.add(&format!("{prefix}.point.{i}"), hr, &format!("Point {}", i + 1));
    }
    let to_g = |s: Pos2| [((s.x - gr.min.x) / gr.width()).clamp(0.0, 1.0), ((gr.max.y - s.y) / gr.height()).clamp(0.0, 1.0)];
    let hit_r = [7.0 / gr.width(), 7.0 / gr.height()];
    let mut changed: Option<OffsetPoints> = None;
    let alt = ui.input(|i| i.modifiers.alt);
    // One undo step per click or drag.
    let key = gesture_key(ui, gid, resp.drag_started() || resp.clicked());
    if resp.drag_started()
        && let Some(pt) = ui.input(|i| i.pointer.press_origin())
    {
        let q = to_g(pt);
        let i = match cur.hit(q[0], q[1], hit_r) {
            Some(i) => i,
            None => {
                let mut np = cur.clone();
                let i = np.insert(q[0], q[1]);
                changed = Some(np);
                i
            }
        };
        ui.data_mut(|d| d.insert_temp(drag_id, i));
    } else if resp.dragged()
        && let (Some(i), Some(pt)) = (dragging, resp.interact_pointer_pos())
    {
        let q = to_g(pt);
        let mut np = cur.clone();
        np.move_point(i, q[0], q[1]);
        if np != cur {
            changed = Some(np);
        }
    }
    if resp.drag_stopped() {
        if let (Some(i), Some(pt)) = (dragging, resp.interact_pointer_pos())
            && !gr.expand(14.0).contains(pt)
            && i < cur.0.len()
        {
            let mut np = cur.clone();
            np.0.remove(i);
            changed = Some(np);
        }
        ui.data_mut(|d| d.remove::<usize>(drag_id));
    }
    if resp.clicked()
        && let Some(pt) = resp.interact_pointer_pos()
    {
        let q = to_g(pt);
        let mut np = cur.clone();
        match cur.hit(q[0], q[1], hit_r) {
            Some(i) if alt => {
                np.0.remove(i);
            }
            Some(_) => {}
            None => {
                np.insert(q[0], q[1]);
            }
        }
        if np != cur {
            changed = Some(np);
        }
    }
    if let Some(np) = changed {
        set_str(actions, layer, g, id, np.format(), Some(key));
    }
    if reset_link(app, ui, p, Rect::from_min_size(pos2(gr.max.x - 44.0, gr.max.y + 4.0), vec2(44.0, 18.0)), &format!("{prefix}.reset")) {
        set_str(actions, layer, g, id, String::new(), None);
    }
    p.text(pos2(gr.min.x, gr.max.y + 13.0), Align2::LEFT_CENTER, tr("Click adds a point (½ height = no change)"), Tokens::ui(10.5), t.text_faint);
}

// ---------------------------------------------------------------------------------------------
// Colorama output cycle

fn wheel_size(width: f32) -> f32 {
    (width - 90.0).clamp(110.0, 200.0)
}

/// Colorama's palette as stops: (position, straight RGBA).
pub fn format_palette(stops: &[(f32, [f32; 4])]) -> String {
    let f = |v: f32| {
        let s = format!("{v:.4}");
        let s = s.trim_end_matches('0').trim_end_matches('.').to_string();
        if s.is_empty() || s == "-0" { "0".into() } else { s }
    };
    stops
        .iter()
        .map(|(pos, c)| {
            let rgb = format!("{},{},{}", f(c[0]), f(c[1]), f(c[2]));
            if (c[3] - 1.0).abs() > 1e-4 { format!("{}:{rgb},{}", f(*pos), f(c[3])) } else { format!("{}:{rgb}", f(*pos)) }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn rgba8(c: [f32; 4]) -> Color32 {
    let b = |v: f32| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
    Color32::from_rgb(b(c[0]), b(c[1]), b(c[2]))
}

/// Phase (0..1, clockwise from the top) of a point around `c`.
fn phase_at(c: Pos2, q: Pos2) -> f32 {
    let a = (q.x - c.x).atan2(-(q.y - c.y));
    (a / std::f32::consts::TAU).rem_euclid(1.0)
}

fn wheel_point(c: Pos2, radius: f32, phase: f32) -> Pos2 {
    let a = phase * std::f32::consts::TAU;
    pos2(c.x + radius * a.sin(), c.y - radius * a.cos())
}

/// Colorama's Output Cycle wheel for effect group `g`: the palette drawn as a ring with its stops.
/// Clicking the ring adds a stop with the colour there, dragging moves one, and Alt-clicking or
/// dragging one off removes it while another stop remains; the selected stop's colour can be
/// edited below. A change queues
/// the rewritten palette on `actions`, and for a preset palette also switches the effect to the
/// custom palette in the same undo step.
#[allow(clippy::too_many_arguments)]
fn colorama_wheel(
    app: &mut EffectcraftApp,
    ui: &mut egui::Ui,
    p: &egui::Painter,
    layer: &Layer,
    root: &PropGroup,
    g: &PropGroup,
    ectx: &EvalCtx,
    r: Rect,
    actions: &mut Actions,
) {
    let t = app.tokens;
    let euid = root.uid;
    let prefix = format!("effectControls.effect.{euid}.colorama");
    let preset = g.get("usePresetPalette").map(|pr| ectx.value(layer, pr).as_enum()).unwrap_or(0);
    let custom = preset as usize == effectcraft_engine::effects::COLORAMA_PRESETS.len() - 1;
    let interp = g.get("interpolatePalette").is_none_or(|pr| ectx.value(layer, pr).as_bool());
    let pal = if custom { ColoramaPalette::parse(&str_value(layer, ectx, g, "palette")) } else { None }.unwrap_or_else(|| ColoramaPalette::preset(preset));
    let x0 = r.min.x + 38.0;
    let label = if custom { "Custom palette" } else { effectcraft_engine::effects::COLORAMA_PRESETS.get(preset as usize).copied().unwrap_or("Preset") };
    p.text(pos2(x0, r.min.y + 15.0), Align2::LEFT_CENTER, tr_args("Output Cycle: {}", &[&label]), Tokens::ui(11.5), t.text_dim);
    let size = wheel_size(r.width());
    let c = pos2(x0 + size / 2.0, r.min.y + 30.0 + size / 2.0);
    let (outer, inner) = (size / 2.0 - 8.0, size / 2.0 - 26.0);
    // The ring: palette colours around the wheel.
    let n = 96;
    for i in 0..n {
        let (a0, a1) = (i as f32 / n as f32, (i + 1) as f32 / n as f32);
        let col = rgba8(pal.eval((a0 + a1) * 0.5, interp));
        let quad = vec![wheel_point(c, inner, a0), wheel_point(c, outer, a0), wheel_point(c, outer, a1 + 0.002), wheel_point(c, inner, a1 + 0.002)];
        p.add(egui::Shape::convex_polygon(quad, col, Stroke::NONE));
    }
    p.circle_stroke(c, outer, Stroke::new(1.0, Color32::from_black_alpha(160)));
    p.circle_stroke(c, inner, Stroke::new(1.0, Color32::from_black_alpha(160)));
    let wid = egui::Id::unique(("colorama-wheel", euid));
    let ring = Rect::from_center_size(c, vec2(outer * 2.0 + 40.0, outer * 2.0 + 40.0));
    let resp = ui.interact(ring, wid, Sense::click_and_drag());
    app.auto.add(&format!("{prefix}.wheel"), ring, "Output Cycle");
    let sel_id = wid.with("sel");
    let drag_id = wid.with("drag");
    let mut sel: usize = ui.data(|d| d.get_temp(sel_id)).unwrap_or(0);
    if sel >= pal.stops.len() {
        sel = 0;
    }
    let dragging: Option<usize> = ui.data(|d| d.get_temp(drag_id));
    // Stops: triangles on the outside of the ring.
    let mut stop_rects = vec![];
    for (i, (ph, col)) in pal.stops.iter().enumerate() {
        let tip = wheel_point(c, outer + 2.0, *ph);
        let a = wheel_point(c, outer + 12.0, *ph - 0.018);
        let b = wheel_point(c, outer + 12.0, *ph + 0.018);
        let fill = rgba8(*col);
        p.add(egui::Shape::convex_polygon(
            vec![tip, a, b],
            fill,
            Stroke::new(if i == sel || dragging == Some(i) { 1.6 } else { 1.0 }, if i == sel { Color32::WHITE } else { Color32::from_gray(0x80) }),
        ));
        let hr = Rect::from_center_size(wheel_point(c, outer + 8.0, *ph), vec2(14.0, 14.0));
        app.auto.add(&format!("{prefix}.stop.{i}"), hr, &format!("Stop {}", i + 1));
        stop_rects.push(hr);
    }
    let hit = |q: Pos2| -> Option<usize> {
        stop_rects
            .iter()
            .enumerate()
            .filter(|(_, hr)| hr.expand(3.0).contains(q))
            .min_by(|a, b| a.1.center().distance(q).total_cmp(&b.1.center().distance(q)))
            .map(|(i, _)| i)
    };
    let mut new_stops: Option<Vec<(f32, [f32; 4])>> = None;
    let alt = ui.input(|i| i.modifiers.alt);
    // One undo step per click or drag (a colour pick is its own step).
    let key = gesture_key(ui, wid, resp.drag_started() || resp.clicked() || !resp.dragged());
    if resp.drag_started()
        && let Some(q) = ui.input(|i| i.pointer.press_origin())
        && let Some(i) = hit(q)
    {
        ui.data_mut(|d| {
            d.insert_temp(drag_id, i);
            d.insert_temp(sel_id, i);
        });
    } else if resp.dragged()
        && let (Some(i), Some(q)) = (dragging, resp.interact_pointer_pos())
        && i < pal.stops.len()
    {
        let mut st = pal.stops.clone();
        st[i].0 = phase_at(c, q);
        new_stops = Some(st);
    }
    if resp.drag_stopped() {
        if let (Some(i), Some(q)) = (dragging, resp.interact_pointer_pos())
            && q.distance(c) > outer + 40.0
            && pal.stops.len() > 1
            && i < pal.stops.len()
        {
            let mut st = pal.stops.clone();
            st.remove(i);
            new_stops = Some(st);
        }
        ui.data_mut(|d| d.remove::<usize>(drag_id));
    }
    if resp.clicked()
        && let Some(q) = resp.interact_pointer_pos()
    {
        match hit(q) {
            Some(i) if alt && pal.stops.len() > 1 => {
                let mut st = pal.stops.clone();
                st.remove(i);
                new_stops = Some(st);
            }
            Some(i) => {
                ui.data_mut(|d| d.insert_temp(sel_id, i));
            }
            // A click on the ring adds a stop with the colour there (and selects it).
            None if (inner - 4.0..=outer + 4.0).contains(&q.distance(c)) => {
                let ph = phase_at(c, q);
                let mut st = pal.stops.clone();
                st.push((ph, pal.eval(ph, interp)));
                st.sort_by(|a, b| a.0.total_cmp(&b.0));
                let k = st.iter().position(|s| s.0 == ph).unwrap_or(0);
                ui.data_mut(|d| d.insert_temp(sel_id, k));
                new_stops = Some(st);
            }
            None => {}
        }
    }
    // The selected stop's colour.
    let sr = Rect::from_min_size(pos2(x0, c.y + size / 2.0 + 8.0), vec2(30.0, 16.0));
    if let Some((ph, col)) = pal.stops.get(sel).copied() {
        let pop = wid.with("color");
        if widgets::swatch(ui, sr, col, wid.with("swatch"), &t).clicked() {
            widgets::open_popup(ui, pop);
        }
        app.auto.add(&format!("{prefix}.color"), sr, "Stop color");
        p.text(
            pos2(sr.max.x + 8.0, sr.center().y),
            Align2::LEFT_CENTER,
            tr_args("Stop {} at {}°", &[&(sel + 1), &format!("{:.0}", ph * 360.0)]),
            Tokens::ui(11.0),
            t.text_dim,
        );
        let mut rgb = [col[0], col[1], col[2]];
        if crate::widgets::color_popup(ui, pop, sr.left_bottom(), &mut rgb) {
            let mut st = pal.stops.clone();
            st[sel].1 = [rgb[0], rgb[1], rgb[2], col[3]];
            new_stops = Some(st);
        }
    }
    p.text(
        pos2(sr.min.x, sr.max.y + 9.0),
        Align2::LEFT_CENTER,
        "Click the ring to add a colour; Alt-click a stop (or drag it off) to remove it",
        Tokens::ui(10.5),
        t.text_faint,
    );
    if let Some(mut st) = new_stops {
        st.sort_by(|a, b| a.0.total_cmp(&b.0));
        // Editing a preset makes it the custom palette (both in one undo step).
        if !custom && let Some(pr) = g.get("usePresetPalette") {
            actions.push((
                "prop.set".into(),
                json!({"layer": layer.id.0, "prop": pr.uid, "value": effectcraft_engine::effects::COLORAMA_PRESETS.len() - 1, "merge": key}),
            ));
        }
        set_str(actions, layer, g, "palette", format_palette(&st), Some(key));
    }
}

// ---------------------------------------------------------------------------------------------
// Header editors (under the effect's title)

/// Height of the editor under `effect`'s header (`g`: the instance), 0 when none.
pub fn header_height(effect: &str, g: &PropGroup, width: f32) -> f32 {
    match effect {
        GLOW => match g.get("colors").map(|p| p.value.as_enum()) {
            Some(2) => 30.0 + graph_size(width) + 40.0,
            Some(1) => 34.0,
            _ => 0.0,
        },
        RESHAPE => 50.0,
        EXTRACTOR => 30.0,
        _ => 0.0,
    }
}

#[allow(clippy::too_many_arguments)]
pub fn header_editor(
    app: &mut EffectcraftApp,
    ui: &mut egui::Ui,
    p: &egui::Painter,
    layer: &Layer,
    g: &PropGroup,
    effect: &str,
    ectx: &EvalCtx,
    r: Rect,
    actions: &mut Actions,
) {
    match effect {
        GLOW => glow_map(app, ui, p, layer, g, ectx, r, actions),
        RESHAPE => reshape_buttons(app, ui, p, layer, g, ectx, r, actions),
        EXTRACTOR => extractor_layers(app, ui, p, layer, g, ectx, r, actions),
        _ => {}
    }
}

/// Glow's Arbitrary Map: "red | green | blue" Curves point lists.
pub fn split_map(s: &str) -> [fw::CurvePoints; 3] {
    let mut parts = s.split('|');
    std::array::from_fn(|_| parts.next().map(fw::CurvePoints::parse).unwrap_or_else(fw::CurvePoints::identity))
}

pub fn join_map(m: &[fw::CurvePoints; 3]) -> String {
    m.iter().map(|c| c.format()).collect::<Vec<_>>().join(" | ")
}

/// Glow's Colour Map for effect group `g`: a strip showing the gradient the glow uses (A/B colours,
/// or the arbitrary map sampled through the loop settings). In arbitrary-map mode it also draws the
/// RGB curve tabs and the selected channel's graph with a Reset; edits queue a `prop.set` of
/// `arbitraryMap` on `actions`.
#[allow(clippy::too_many_arguments)]
fn glow_map(app: &mut EffectcraftApp, ui: &mut egui::Ui, p: &egui::Painter, layer: &Layer, g: &PropGroup, ectx: &EvalCtx, r: Rect, actions: &mut Actions) {
    let t = app.tokens;
    let euid = g.uid;
    let prefix = format!("effectControls.effect.{euid}.glow");
    let mode = g.get("colors").map(|pr| ectx.value(layer, pr).as_enum()).unwrap_or(0);
    let x0 = r.min.x + 38.0;
    let num = |id: &str| g.get(id).map(|pr| ectx.value(layer, pr).as_f64()).unwrap_or(0.0);
    let looping = g.get("colorLooping").map(|pr| ectx.value(layer, pr).as_enum()).unwrap_or(2);
    let (loops, phase, mid) = (num("colorLoops") as f32, (num("colorPhase") / 360.0) as f32, (num("abMidpoint") / 100.0) as f32);
    let map = split_map(&str_value(layer, ectx, g, "arbitraryMap"));
    let color_at = |l: f32| -> Color32 {
        let tt = effectcraft_engine::effects::glow_ab_t(l, looping, loops, phase, mid);
        if mode == 2 {
            Color32::from_rgb(
                (map[0].eval(tt).clamp(0.0, 1.0) * 255.0) as u8,
                (map[1].eval(tt).clamp(0.0, 1.0) * 255.0) as u8,
                (map[2].eval(tt).clamp(0.0, 1.0) * 255.0) as u8,
            )
        } else {
            let col = |id: &str| match g.get(id).map(|pr| ectx.value(layer, pr)) {
                Some(Value::Color(c)) => c,
                _ => [0.0; 4],
            };
            let (a, b) = (col("colorA"), col("colorB"));
            let m = |i: usize| ((a[i] + (b[i] - a[i]) * tt as f64).clamp(0.0, 1.0) * 255.0) as u8;
            Color32::from_rgb(m(0), m(1), m(2))
        }
    };
    let size = graph_size(r.width());
    let strip_y = if mode == 2 { r.min.y + 30.0 + size + 12.0 } else { r.min.y + 8.0 };
    let strip = Rect::from_min_size(pos2(x0, strip_y), vec2(size, 12.0));
    let n = 64;
    for i in 0..n {
        let f = (i as f32 + 0.5) / n as f32;
        let x = strip.min.x + strip.width() * i as f32 / n as f32;
        p.rect_filled(Rect::from_min_max(pos2(x, strip.min.y), pos2(x + strip.width() / n as f32 + 0.5, strip.max.y)), 0.0, color_at(f));
    }
    p.rect_stroke(strip, 0.0, Stroke::new(1.0, t.field_border), StrokeKind::Outside);
    app.auto.add(&format!("{prefix}.colorMap"), strip, "Glow colour map");
    p.text(pos2(strip.max.x + 8.0, strip.center().y), Align2::LEFT_CENTER, tr("Colour map"), Tokens::ui(10.5), t.text_faint);
    if mode != 2 {
        return;
    }
    let ch = app.ui.fx_curve_channel.get(&euid).copied().unwrap_or(0).min(2);
    let cols = [Color32::from_rgb(0xf0, 0x50, 0x50), Color32::from_rgb(0x50, 0xd0, 0x60), Color32::from_rgb(0x50, 0x80, 0xff)];
    if let Some(n) = tabs(app, ui, p, pos2(x0, r.min.y + 15.0), ch, (&["Red", "Green", "Blue"], &cols), &format!("{prefix}.channel")) {
        app.ui.fx_curve_channel.insert(euid, n);
    }
    if reset_link(app, ui, p, Rect::from_min_size(pos2(x0 + size - 44.0, r.min.y + 6.0), vec2(44.0, 18.0)), &format!("{prefix}.reset")) {
        let mut m = map.clone();
        m[ch] = fw::CurvePoints::identity();
        set_str(actions, layer, g, "arbitraryMap", join_map(&m), None);
    }
    let gr = Rect::from_min_size(pos2(x0, r.min.y + 30.0), vec2(size, size));
    let gid = egui::Id::unique(("glow-map", euid, ch));
    let (resp, edit) = fw::curves_graph(ui, gr, &map[ch], ch + 1, fw::CurveMode { pencil: false, natural: false }, gid, &t);
    app.auto.add(&format!("{prefix}.graph"), gr, "Arbitrary Map");
    register_points(app, &prefix, gr, &map[ch].0);
    let key = gesture_key(ui, gid, resp.drag_started() || resp.clicked());
    if let Some(np) = edit.changed {
        let mut m = map.clone();
        m[ch] = np;
        set_str(actions, layer, g, "arbitraryMap", join_map(&m), Some(key));
    }
}

// ---------------------------------------------------------------------------------------------
// Reshape

/// Reshape's correspondence pairs (source, destination outline fractions), in stored order.
pub fn parse_pairs(s: &str) -> Vec<(f64, f64)> {
    s.split_whitespace()
        .filter_map(|tk| {
            let (a, b) = tk.split_once(',')?;
            Some((a.trim().parse::<f64>().ok()?.rem_euclid(1.0), b.trim().parse::<f64>().ok()?.rem_euclid(1.0)))
        })
        .collect()
}

pub fn format_pairs(v: &[(f64, f64)]) -> String {
    let f = |x: f64| {
        let s = format!("{x:.4}");
        let s = s.trim_end_matches('0').trim_end_matches('.').to_string();
        if s.is_empty() { "0".into() } else { s }
    };
    v.iter().map(|(a, b)| format!("{},{}", f(*a), f(*b))).collect::<Vec<_>>().join(" ")
}

/// A new pair in the widest gap between the destination fractions (the source fraction
/// interpolated between its neighbours'), or (0, 0) for the first.
pub fn add_pair(v: &[(f64, f64)]) -> (f64, f64) {
    if v.is_empty() {
        return (0.0, 0.0);
    }
    let mut s: Vec<(f64, f64)> = v.to_vec();
    s.sort_by(|a, b| a.1.total_cmp(&b.1));
    let n = s.len();
    let (mut best, mut gap) = (0, -1.0);
    for i in 0..n {
        let g = (s[(i + 1) % n].1 - s[i].1).rem_euclid(1.0);
        let g = if n == 1 { 1.0 } else { g };
        if g > gap {
            gap = g;
            best = i;
        }
    }
    let (a, b) = (s[best], s[(best + 1) % n]);
    let ds = if n == 1 { 1.0 } else { (b.0 - a.0).rem_euclid(1.0) };
    ((a.0 + ds * 0.5).rem_euclid(1.0), (a.1 + gap * 0.5).rem_euclid(1.0))
}

/// Reshape's correspondence buttons: Add Point appends a pair at the widest destination-fraction
/// gap (or `(0, 0)` for an empty list), Remove Point drops the last stored pair, and Clear removes
/// all pairs. A status line says whether points are automatic. Each change
/// queues a `prop.set` of the `correspondencePoints` string on `actions`.
#[allow(clippy::too_many_arguments)]
fn reshape_buttons(
    app: &mut EffectcraftApp,
    ui: &mut egui::Ui,
    p: &egui::Painter,
    layer: &Layer,
    g: &PropGroup,
    ectx: &EvalCtx,
    r: Rect,
    actions: &mut Actions,
) {
    let t = app.tokens;
    let euid = g.uid;
    let pairs = parse_pairs(&str_value(layer, ectx, g, "correspondencePoints"));
    let b1 = Rect::from_min_size(pos2(r.min.x + 24.0, r.min.y + 4.0), vec2(84.0, 22.0));
    let b2 = Rect::from_min_size(pos2(b1.max.x + 8.0, b1.min.y), vec2(96.0, 22.0));
    let b3 = Rect::from_min_size(pos2(b2.max.x + 8.0, b1.min.y), vec2(56.0, 22.0));
    if widgets::text_button(ui, b1, "Add Point", false, &t, egui::Id::unique(("reshape-add", euid))).clicked() {
        let mut v = pairs.clone();
        v.push(add_pair(&pairs));
        set_str(actions, layer, g, "correspondencePoints", format_pairs(&v), None);
    }
    app.auto.add(&format!("effectControls.effect.{euid}.reshape.add"), b1, "Add Point");
    if widgets::text_button(ui, b2, "Remove Point", false, &t, egui::Id::unique(("reshape-remove", euid))).clicked() && !pairs.is_empty() {
        let mut v = pairs.clone();
        v.pop();
        set_str(actions, layer, g, "correspondencePoints", format_pairs(&v), None);
    }
    app.auto.add(&format!("effectControls.effect.{euid}.reshape.remove"), b2, "Remove Point");
    if widgets::text_button(ui, b3, "Clear", false, &t, egui::Id::unique(("reshape-clear", euid))).clicked() && !pairs.is_empty() {
        set_str(actions, layer, g, "correspondencePoints", String::new(), None);
    }
    app.auto.add(&format!("effectControls.effect.{euid}.reshape.clear"), b3, "Clear");
    let status = if pairs.is_empty() {
        "Correspondence: automatic (select the effect to edit points on the viewer)".to_string()
    } else {
        format!("{} correspondence point(s): drag them along the mask outlines in the viewer", pairs.len())
    };
    p.text(pos2(r.min.x + 24.0, r.min.y + 38.0), Align2::LEFT_CENTER, status, Tokens::ui(11.5), t.text_dim);
}

// ---------------------------------------------------------------------------------------------
// EXtractoR: the layer's OpenEXR layers and channels

/// EXtractoR's channel parameters, shown in red, green, blue and alpha.
const EXTRACTOR_CHANNELS: [&str; 4] = ["red", "green", "blue", "alpha"];
/// What an empty channel name shows.
const NO_CHANNEL: &str = "(none)";

/// Whether `prop` (in group `g`) is one of EXtractoR's channel parameters.
pub fn is_extractor_channel(g: &PropGroup, prop: &effectcraft_engine::project::Property) -> bool {
    matches!(&g.kind, GroupKind::Effect { effect } if effect == EXTRACTOR) && EXTRACTOR_CHANNELS.contains(&prop.match_id.as_str())
}

/// The OpenEXR layer EXtractoR's red, green and blue come from (`""`: the unnamed layer), or
/// `None` when they mix layers.
fn extractor_layer(vals: &[String; 4]) -> Option<&str> {
    let mut layers = vals[..3].iter().filter(|v| !v.trim().is_empty()).map(|v| effectcraft_engine::raster::channels3d::split_channel(v.trim()).0);
    let first = layers.next()?;
    layers.all(|l| l == first).then_some(first)
}

/// EXtractoR's Layer popup (above its channels): picking an OpenEXR layer of the footage
/// (`diffuse`, `ViewLayer.Combined`, …) shows its channels in red, green, blue and alpha in one
/// undo step, as After Effects' EXtractoR dialog does.
#[allow(clippy::too_many_arguments)]
fn extractor_layers(
    app: &mut EffectcraftApp,
    ui: &mut egui::Ui,
    p: &egui::Painter,
    layer: &Layer,
    g: &PropGroup,
    ectx: &EvalCtx,
    r: Rect,
    actions: &mut Actions,
) {
    let t = app.tokens;
    let euid = g.uid;
    let vals = EXTRACTOR_CHANNELS.map(|id| str_value(layer, ectx, g, id));
    let label = match extractor_layer(&vals) {
        Some("") => "RGBA".to_string(),
        Some(l) => l.to_string(),
        None => "Custom".to_string(),
    };
    // Lined up with the parameter names and value popups below (Effect Controls' row layout).
    p.text(pos2(r.min.x + 37.0, r.center().y), Align2::LEFT_CENTER, tr("Layer"), Tokens::ui(12.0), t.text);
    let vx = (r.min.x + r.width() * 0.48).max(r.min.x + 161.0);
    let dr = Rect::from_min_size(pos2(vx, r.center().y - 9.0), vec2((r.max.x - vx - 56.0).clamp(80.0, 200.0), 18.0));
    let pop = egui::Id::unique(("extractor-layers", euid));
    if widgets::dropdown(ui, dr, &label, &t, egui::Id::unique(("extractor-layer", euid)))
        .on_hover_text(tr("The OpenEXR layer shown in red, green, blue and alpha"))
        .clicked()
    {
        widgets::open_popup(ui, pop);
    }
    app.auto.add(&format!("effectControls.effect.{euid}.extractor.layer"), dr, "Layer");
    // The footage's layers, read only while the list is open (each frame of a sequence is a file).
    if !widgets::popup_is_open(ui, pop) {
        return;
    }
    let aux = app.session.layer_channels(layer.id);
    let layers: Vec<&str> = aux.as_ref().map(|a| a.layers()).unwrap_or_default();
    let names: Vec<String> = match layers.as_slice() {
        [] => vec!["No layers: not a multi-layer OpenEXR file".into()],
        _ => layers.iter().map(|l| if l.is_empty() { "RGBA".to_string() } else { l.to_string() }).collect(),
    };
    let cur = names.iter().position(|n| *n == label);
    if let (Some(i), Some(aux)) = (widgets::popup_menu(ui, pop, dr.left_bottom(), &names, cur), aux.as_ref()) {
        let Some(l) = layers.get(i) else { return };
        let merge = format!("extractor-layer-{euid}-{}", ui.ctx().cumulative_pass_nr());
        for (id, v) in EXTRACTOR_CHANNELS.iter().zip(aux.layer_rgba(l)) {
            set_str(actions, layer, g, id, v, Some(merge.clone()));
        }
    }
}

/// An EXtractoR channel popup (in place of a text field): the layer's own R, G, B and A, then
/// every channel of its OpenEXR footage, and (none).
pub fn channel_popup(
    app: &mut EffectcraftApp,
    ui: &mut egui::Ui,
    layer: &Layer,
    prop: &effectcraft_engine::project::Property,
    cur: &str,
    r: Rect,
    actions: &mut Actions,
) {
    let t = app.tokens;
    let uid = prop.uid;
    let label = if cur.trim().is_empty() { NO_CHANNEL } else { cur };
    let pop = egui::Id::unique(("ec-chpop", uid));
    if widgets::dropdown(ui, r, label, &t, egui::Id::unique(("ec-ch", uid))).clicked() {
        widgets::open_popup(ui, pop);
    }
    app.auto.add(&format!("effectControls.prop.{uid}.value"), r, &prop.name);
    if !widgets::popup_is_open(ui, pop) {
        return;
    }
    let mut opts: Vec<String> = ["R", "G", "B", "A"].map(String::from).to_vec();
    if let Some(aux) = app.session.layer_channels(layer.id) {
        opts.extend(aux.names().into_iter().filter(|n| !opts.iter().any(|o| o.eq_ignore_ascii_case(n))).map(String::from).collect::<Vec<_>>());
    }
    opts.push(NO_CHANNEL.into());
    let sel = opts.iter().position(|o| o.eq_ignore_ascii_case(label));
    if let Some(i) = widgets::popup_menu(ui, pop, r.left_bottom(), &opts, sel) {
        let v = opts.get(i).filter(|o| *o != NO_CHANNEL).cloned().unwrap_or_default();
        actions.push(("prop.set".into(), json!({"layer": layer.id.0, "prop": uid, "value": v})));
    }
}

/// Arc-length fraction (0..1) of the point of a polyline nearest to `q`.
pub fn nearest_fraction(pts: &[[f64; 2]], closed: bool, q: [f64; 2]) -> f64 {
    let n = pts.len();
    if n < 2 {
        return 0.0;
    }
    let total = effectcraft_engine::effects::util::poly_length(pts, closed).max(1e-9);
    let segs = if closed { n } else { n - 1 };
    let (mut best, mut best_s, mut acc) = (f64::INFINITY, 0.0, 0.0);
    for i in 0..segs {
        let (a, b) = (pts[i], pts[(i + 1) % n]);
        let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
        let l2 = dx * dx + dy * dy;
        let l = l2.sqrt();
        let tt = if l2 > 0.0 { (((q[0] - a[0]) * dx + (q[1] - a[1]) * dy) / l2).clamp(0.0, 1.0) } else { 0.0 };
        let d = (a[0] + dx * tt - q[0]).hypot(a[1] + dy * tt - q[1]);
        if d < best {
            best = d;
            best_s = acc + l * tt;
        }
        acc += l;
    }
    (best_s / total).rem_euclid(1.0)
}

/// The Composition viewer's Reshape controls while the effect is selected: the source (yellow)
/// and destination (cyan) mask outlines and each correspondence pair as a line between its two
/// handles; dragging a handle slides it along its outline (one undo step per drag).
#[allow(clippy::too_many_arguments)]
pub fn reshape_overlay(
    app: &mut EffectcraftApp,
    ui: &mut egui::Ui,
    painter: &egui::Painter,
    map: &super::viewer::ViewerMap,
    ectx: &EvalCtx,
    layer: &Layer,
    g: &PropGroup,
    m: &Mat3,
    actions: &mut Actions,
) {
    let shapes = effectcraft_engine::render::masks::shapes(ectx, layer);
    let mask = |id: &str| -> Option<(Vec<[f64; 2]>, bool)> {
        let i = g.get(id).map(|pr| ectx.value(layer, pr).as_enum()).unwrap_or(0) as usize;
        let s = shapes.get(i.checked_sub(1)?)?;
        (s.points.len() >= 2).then(|| (s.points.clone(), s.closed))
    };
    let (Some(src), Some(dst)) = (mask("sourceMask"), mask("destinationMask")) else { return };
    let to_screen = |q: [f64; 2]| map.to_screen(fw::layer_to_comp(m, q));
    let outline = |pts: &[[f64; 2]], closed: bool, col: Color32| {
        let mut v: Vec<Pos2> = pts.iter().map(|q| to_screen(*q)).collect();
        if closed && let Some(f) = v.first().copied() {
            v.push(f);
        }
        painter.add(egui::Shape::dashed_line(&v, Stroke::new(1.0, col), 5.0, 3.0));
    };
    let (ys, cs) = (Color32::from_rgb(0xff, 0xd8, 0x40), Color32::from_rgb(0x40, 0xd8, 0xff));
    outline(&src.0, src.1, ys);
    outline(&dst.0, dst.1, cs);
    let at = |pts: &[[f64; 2]], closed: bool, f: f64| {
        let len = effectcraft_engine::effects::util::poly_length(pts, closed);
        effectcraft_engine::effects::util::poly_point_at(pts, closed, f * len).0
    };
    let raw = match g.get("correspondencePoints").map(|pr| ectx.value(layer, pr)) {
        Some(Value::Str(s)) => s,
        _ => String::new(),
    };
    let pairs = parse_pairs(&raw);
    let ctx = ui.ctx().clone();
    for (i, (fs, fd)) in pairs.iter().enumerate() {
        let (ps, pd) = (to_screen(at(&src.0, src.1, *fs)), to_screen(at(&dst.0, dst.1, *fd)));
        painter.line_segment([ps, pd], Stroke::new(1.0, Color32::from_white_alpha(150)));
        for (end, s, col) in [(0usize, ps, ys), (1, pd, cs)] {
            let hr = Rect::from_center_size(s, vec2(12.0, 12.0));
            let id = egui::Id::unique(("reshape-pt", g.uid, i, end));
            let resp = ui.interact(hr, id, Sense::drag());
            let hot = resp.hovered() || resp.dragged();
            painter.rect_filled(Rect::from_center_size(s, vec2(7.0, 7.0)), 0.0, if hot { Color32::WHITE } else { col });
            painter.rect_stroke(Rect::from_center_size(s, vec2(7.0, 7.0)), 0.0, Stroke::new(1.0, Color32::BLACK), StrokeKind::Outside);
            let which = if end == 0 { "source" } else { "dest" };
            app.auto.add(&format!("viewer.reshape.{}.{which}.{i}", g.uid), hr, &format!("Correspondence point {} ({which})", i + 1));
            if hot {
                ctx.set_cursor_icon(egui::CursorIcon::Move);
            }
            if resp.dragged()
                && let Some(pos) = resp.interact_pointer_pos()
                && let Some(lp) = fw::comp_to_layer(m, map.to_comp(pos))
            {
                let mut v = pairs.clone();
                if end == 0 {
                    v[i].0 = nearest_fraction(&src.0, src.1, lp);
                } else {
                    v[i].1 = nearest_fraction(&dst.0, dst.1, lp);
                }
                let key = gesture_key(ui, id, resp.drag_started());
                if let Some(pr) = g.get("correspondencePoints") {
                    actions.push(("prop.set".into(), json!({"layer": layer.id.0, "prop": pr.uid, "value": format_pairs(&v), "merge": key})));
                }
            }
        }
    }
}

/// Mesh Warp's grid: (rows, columns, vertex offsets in layer pixels, row-major). A stored mesh
/// that doesn't match the grid (Rows or Columns changed since) reads as undistorted, as it
/// renders.
fn mesh_grid(layer: &Layer, ectx: &EvalCtx, g: &PropGroup) -> (usize, usize, Vec<(f64, f64)>) {
    let n = |id: &str| {
        let v = g.get(id).map(|p| ectx.value(layer, p).as_f64()).unwrap_or(7.0);
        if v.is_finite() { v.round().clamp(1.0, 31.0) as usize } else { 7 }
    };
    let (rows, cols) = (n("rows"), n("columns"));
    let count = (rows + 1) * (cols + 1);
    let mut offs = effectcraft_engine::effects::parse_mesh(&str_value(layer, ectx, g, "mesh"));
    if offs.len() != count {
        offs = vec![(0.0, 0.0); count];
    }
    (rows, cols, offs)
}

/// The `mesh` parameter's text for these offsets (the inverse of `parse_mesh`).
fn format_mesh(offs: &[(f64, f64)]) -> String {
    let r = |v: f64| if v.is_finite() { (v * 1000.0).round() / 1000.0 + 0.0 } else { 0.0 };
    offs.iter().map(|o| format!("{},{}", r(o.0), r(o.1))).collect::<Vec<_>>().join(" ")
}

/// Mesh Warp on the Composition viewer (#303): the distortion mesh's grid over the layer, with
/// a handle on every vertex. Dragging a vertex moves it (its offset in the hidden `mesh`
/// parameter; the first drag of an undistorted mesh writes the whole grid). `fm` maps effect
/// space to comp.
#[allow(clippy::too_many_arguments)]
pub fn mesh_warp_overlay(
    app: &mut EffectcraftApp,
    ui: &mut egui::Ui,
    painter: &egui::Painter,
    map: &super::viewer::ViewerMap,
    ectx: &EvalCtx,
    layer: &Layer,
    g: &PropGroup,
    fm: &Mat3,
    actions: &mut Actions,
) {
    let (rows, cols, offs) = mesh_grid(layer, ectx, g);
    let [lw, lh] = ectx.effect_bounds(layer).0;
    let rest = |k: usize| [(k % (cols + 1)) as f64 / cols as f64 * lw, (k / (cols + 1)) as f64 / rows as f64 * lh];
    let screen: Vec<Pos2> = offs
        .iter()
        .enumerate()
        .map(|(k, o)| {
            let r = rest(k);
            map.to_screen(fw::layer_to_comp(fm, [r[0] + o.0, r[1] + o.1]))
        })
        .collect();
    let line = Stroke::new(1.0, Color32::from_white_alpha(150));
    for (k, s) in screen.iter().enumerate() {
        if k % (cols + 1) < cols
            && let Some(e) = screen.get(k + 1)
        {
            painter.line_segment([*s, *e], line);
        }
        if let Some(e) = screen.get(k + cols + 1) {
            painter.line_segment([*s, *e], line);
        }
    }
    let ctx = ui.ctx().clone();
    let accent = app.tokens.accent;
    for (k, s) in screen.iter().enumerate() {
        let hr = Rect::from_center_size(*s, vec2(10.0, 10.0));
        let id = egui::Id::unique(("mesh-warp-vertex", g.uid, k));
        let resp = ui.interact(hr, id, Sense::drag());
        let hot = resp.hovered() || resp.dragged();
        let dot = Rect::from_center_size(*s, vec2(5.0, 5.0));
        painter.rect_filled(dot, 0.0, if hot { accent } else { Color32::WHITE });
        painter.rect_stroke(dot, 0.0, Stroke::new(1.0, Color32::BLACK), StrokeKind::Outside);
        let (row, col) = (k / (cols + 1), k % (cols + 1));
        app.auto.add(&format!("viewer.meshWarp.{}.{row}.{col}", g.uid), hr, &format!("Mesh vertex (row {row}, column {col})"));
        if hot {
            ctx.set_cursor_icon(egui::CursorIcon::Move);
        }
        if resp.dragged()
            && let Some(pos) = resp.interact_pointer_pos()
            && let Some(lp) = fw::comp_to_layer(fm, map.to_comp(pos))
        {
            let mut v = offs.clone();
            let r = rest(k);
            if let Some(o) = v.get_mut(k) {
                *o = (lp[0] - r[0], lp[1] - r[1]);
            }
            let key = gesture_key(ui, id, resp.drag_started());
            set_str(actions, layer, g, "mesh", format_mesh(&v), Some(key));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mesh_text_round_trips() {
        let offs = vec![(0.0, 0.0), (12.5, -3.25), (1.0 / 3.0, f64::NAN)];
        let s = format_mesh(&offs);
        assert_eq!(s, "0,0 12.5,-3.25 0.333,0");
        assert_eq!(effectcraft_engine::effects::parse_mesh(&s), vec![(0.0, 0.0), (12.5, -3.25), (0.333, 0.0)]);
    }

    #[test]
    fn offset_points_and_palettes_round_trip() {
        let p = OffsetPoints::parse("0.5,0.75 0.1,0.5");
        assert_eq!(p.0, vec![[0.1, 0.5], [0.5, 0.75]]);
        assert_eq!(OffsetPoints::parse(&p.format()), p);
        assert!(OffsetPoints::parse("").0.is_empty());
        let stops = vec![(0.0, [1.0, 0.0, 0.0, 1.0]), (0.5, [0.0, 0.25, 1.0, 0.5])];
        let s = format_palette(&stops);
        assert_eq!(s, "0:1,0,0 0.5:0,0.25,1,0.5");
        assert_eq!(ColoramaPalette::parse(&s).unwrap().stops, stops);
        let m = split_map("0,0 1,0.5 | | 0,1 1,0");
        assert_eq!(m[1], fw::CurvePoints::identity());
        assert_eq!(split_map(&join_map(&m)), m);
    }

    #[test]
    fn reshape_pairs_and_outline_fractions() {
        assert_eq!(parse_pairs("0.25,0 1.5,0.5"), vec![(0.25, 0.0), (0.5, 0.5)]);
        assert_eq!(format_pairs(&[(0.25, 0.0), (0.5, 0.125)]), "0.25,0 0.5,0.125");
        assert_eq!(add_pair(&[]), (0.0, 0.0));
        assert_eq!(add_pair(&[(0.1, 0.0)]), (0.6, 0.5));
        let (s, d) = add_pair(&[(0.0, 0.0), (0.2, 0.25)]);
        assert!((d - 0.625).abs() < 1e-9 && (s - 0.6).abs() < 1e-9, "{s} {d}");
        // A 10×10 square (closed): the point nearest (10, 5) is 3/8 of the way round.
        let sq = [[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]];
        assert!((nearest_fraction(&sq, true, [12.0, 5.0]) - 0.375).abs() < 1e-9);
        assert!(nearest_fraction(&sq, true, [-1.0, -1.0]).abs() < 1e-9);
    }
}
