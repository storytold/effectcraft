//! The Timeline panel.
//!
//! Left: current-time display, search, comp switches, then per layer A/V features (video, audio,
//! solo, lock), label, number, name, switches (shy, collapse, quality, fx, frame blend, motion
//! blur, adjustment, 3D), mode, track matte and parent, with twirl-down property trees (stopwatch,
//! keyframe navigator, scrubbable values). Right: time navigator, ruler with work area and cache
//! bar, layer duration bars, keyframes, the CTI, and the value graph editor.

use effectcraft_engine::color::BlendMode;
use effectcraft_engine::keyframe::{Interp, Value};
use effectcraft_engine::project::{
    Comp, FootageKind, GroupKind, ItemId, ItemKind, Layer, LayerId, LayerSource, MatteKind, Node, ParamUi, Project, PropGroup, Property,
};
use effectcraft_engine::render::EvalCtx;
use effectcraft_engine::time::Tick;
use egui::{Align2, Color32, Pos2, Rect, Sense, Stroke, StrokeKind, pos2, vec2};
use serde_json::json;
use std::sync::Arc;

use crate::i18n::tr;
use crate::icons::{self, Icon};
use crate::theme::Tokens;
use crate::{EffectcraftApp, widgets};

#[derive(Clone, Debug)]
enum RowKind {
    Layer,
    Group {
        uid: u64,
        name: String,
        open: bool,
        has_children: bool,
        fx: Option<bool>,
        /// Eye switch (layer styles).
        eye: Option<bool>,
    },
    Prop {
        uid: u64,
    },
    /// Inline expression editor under a property with an expression.
    Expr {
        uid: u64,
        lines: usize,
    },
    /// Audio > Waveform of a footage layer with audio (drawn from the item's peak summary).
    Waveform {
        item: u64,
    },
}

/// Synthetic group uid for a layer's Audio > Waveform twirl (never a real property uid).
const WAVE_BIT: u64 = 1 << 60;

/// Synthetic group uid for the Geometry Options row of a 3D text or shape layer in a Classic 3D
/// comp, which only offers "Change Renderer…" (never a real property uid).
const GEOM_BIT: u64 = 1 << 59;

/// Classic 3D comps don't extrude: a 3D text or shape layer's Geometry Options row offers to
/// change the renderer instead, as in After Effects.
fn classic_geometry_row(comp: &Comp, l: &Layer) -> bool {
    comp.renderer == effectcraft_engine::project::Renderer::Classic3D && l.is_3d() && matches!(l.source, LayerSource::Text | LayerSource::Shape)
}

/// Background of the selected layer's name cell.
const SELECTED_NAME_BG: Color32 = Color32::from_rgb(0xa6, 0xa6, 0xa6);

/// A layer bar's fill: the label colour muted toward grey (a little brighter when selected).
pub(crate) fn bar_color(label: Color32, selected: bool) -> Color32 {
    let k = if selected { 0.62 } else { 0.45 };
    let g = 0x3a as f32;
    let mix = |c: u8| (c as f32 * k + g * (1.0 - k)).round() as u8;
    Color32::from_rgb(mix(label.r()), mix(label.g()), mix(label.b()))
}

/// How far a layer bar's trim zones reach past its ends and into it, in points. After Effects'
/// zones straddle each end, so the pointer needn't land inside the bar.
const TRIM_OUTSIDE: f32 = 4.0;
const TRIM_INSIDE: f32 = 5.0;

/// A layer bar's body (drag to move) and its In/Out trim zones. Short bars keep their middle
/// third as body; a bar too short for that is all trim zones (the Out one on top).
fn trim_zones(bar: Rect) -> (Rect, Rect, Rect) {
    let inside = TRIM_INSIDE.min(bar.width() / 3.0).max(0.0);
    let in_r = Rect::from_min_max(pos2(bar.min.x - TRIM_OUTSIDE, bar.min.y), pos2(bar.min.x + inside, bar.max.y));
    let out_r = Rect::from_min_max(pos2(bar.max.x - inside, bar.min.y), pos2(bar.max.x + TRIM_OUTSIDE, bar.max.y));
    (bar.shrink2(vec2(inside, 0.0)), in_r, out_r)
}

/// The trim cursor over a layer bar's ends: the horizontal resize arrow.
fn trim_cursor(ctx: &egui::Context) {
    ctx.set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
}

/// Darken part `r` of a layer bar and stripe it diagonally in its label colour (where the layer
/// runs past its source). Only the visible part is drawn.
fn stripes(p: &egui::Painter, r: Rect, label: Color32) {
    let v = r.intersect(p.clip_rect());
    if !v.is_positive() {
        return;
    }
    let p = p.with_clip_rect(v);
    p.rect_filled(v, 0.0, Color32::from_black_alpha(90));
    let h = v.height();
    let mut x = v.min.x - h;
    while x < v.max.x {
        p.line_segment([pos2(x, v.max.y), pos2(x + h, v.min.y)], Stroke::new(1.0, label.gamma_multiply(0.7)));
        x += 6.0;
    }
}

/// Row height (expression editors grow with their text).
fn row_height(row: &Row, rh: f32) -> f32 {
    match row.kind {
        RowKind::Expr { lines, .. } => rh * lines as f32,
        RowKind::Waveform { .. } => rh * 3.0,
        _ => rh,
    }
}

/// An inline expression editor grows with its text up to this many lines (After Effects)...
const EXPR_AUTO_LINES: usize = 8;
/// ...and its bottom edge drags it up to this many.
const EXPR_MAX_LINES: usize = 100;

/// The text of the expression being edited (while its editor has the keyboard).
fn expr_buf_id(prop: u64) -> egui::Id {
    egui::Id::unique(("expr-buf", prop))
}

/// Insert an expression editor row after every property that has an expression: as many lines
/// as its text (the text being edited, so it grows while typing), up to 8, or the height its
/// bottom edge was dragged to.
fn with_expr_rows(rows: Vec<Row>, comp: &Comp, tl: &crate::state::TimelineState, ctx: &egui::Context) -> Vec<Row> {
    let mut out = Vec::with_capacity(rows.len());
    // Rows come grouped by layer: look each layer up once (a linear search per property row
    // made twirling open many layers of a big comp quadratic).
    let mut cur: Option<&Layer> = None;
    for r in rows {
        let mut extra = None;
        if let RowKind::Prop { uid } = r.kind
            && !tl.expr_closed.contains(&uid)
        {
            if cur.is_none_or(|l| l.id != r.layer) {
                cur = comp.layer(r.layer);
            }
            extra = cur.and_then(|l| l.props.find(uid)).and_then(|p| p.expr.as_ref()).map(|e| {
                let lines = tl.expr_lines.get(&uid).copied().unwrap_or_else(|| {
                    let edited = ctx.data(|d| d.get_temp::<String>(expr_buf_id(uid)));
                    edited.as_deref().unwrap_or(&e.text).split('\n').count().min(EXPR_AUTO_LINES)
                });
                Row { layer: r.layer, depth: r.depth, kind: RowKind::Expr { uid, lines: lines.clamp(1, EXPR_MAX_LINES) } }
            });
        }
        out.push(r);
        out.extend(extra);
    }
    out
}

/// A pick-whip drag in progress: (kind, layer, prop uid, start point). The kinds are the
/// layer's parent pick whip, an expression's and a property's (Parent & Link column).
type PickWhip = (u8, u64, u64, Pos2);
const WHIP_PARENT: u8 = 0;
const WHIP_EXPR: u8 = 1;
const WHIP_PROP: u8 = 2;

/// The `egui::Id` under which a pick-whip drag in progress ([`PickWhip`]) is kept during the
/// drag.
fn pick_whip_id() -> egui::Id {
    egui::Id::unique("tl-pickwhip")
}

/// The automation id, label and hover hint of a pick whip.
struct WhipText<'a> {
    auto: &'a str,
    label: &'a str,
    hint: &'a str,
}

/// A pick whip at `rect`: dragging it starts a pick whip of `kind` from `layer`'s `prop` (0 for
/// the layer's own), drawn and dropped at the end of [`show`].
fn pick_whip(
    app: &mut EffectcraftApp,
    ui: &egui::Ui,
    p: &egui::Painter,
    rect: Rect,
    (kind, layer, prop): (u8, u64, u64),
    text: WhipText<'_>,
) -> egui::Response {
    let WhipText { auto, label, hint } = text;
    let resp = ui.interact(rect, egui::Id::unique(("pick-whip", kind, layer, prop)), Sense::drag()).on_hover_text(hint);
    icons::paint(p, rect.shrink(1.0), Icon::PickWhip, if resp.hovered() || resp.dragged() { app.tokens.text } else { app.tokens.text_dim });
    app.auto.add(auto, rect, label);
    if resp.drag_started() {
        let state: PickWhip = (kind, layer, prop, rect.center());
        ui.ctx().data_mut(|d| d.insert_temp(pick_whip_id(), state));
    }
    resp
}

/// Properties shown outside the Timeline (Effect Controls) that a pick whip can be dropped on:
/// (row, layer, prop uid, its value fields per dimension), and the pass they were shown in.
type WhipTargets = (u64, Vec<(Rect, u64, u64, Vec<Rect>)>);

/// The `egui::Id` of the drop targets other panels register for a pick-whip drag
/// ([`whip_target`]); [`show`] clears it when the whip is released or dropped.
fn whip_targets_id() -> egui::Id {
    egui::Id::unique("tl-pickwhip-targets")
}

/// While a pick whip is being dragged, `layer`'s property `prop` shown at `row` in another panel
/// is a target too; `dims` are its value fields, one per dimension.
pub(crate) fn whip_target(ctx: &egui::Context, row: Rect, layer: u64, prop: u64, dims: Vec<Rect>) {
    if ctx.data(|d| d.get_temp::<PickWhip>(pick_whip_id())).is_none() {
        return;
    }
    let pass = ctx.cumulative_pass_nr();
    ctx.data_mut(|d| {
        let targets = d.get_temp_mut_or_default::<WhipTargets>(whip_targets_id());
        if targets.0 != pass {
            *targets = (pass, vec![]);
        }
        targets.1.push((row, layer, prop, dims));
    });
}

/// Autoscroll speed (points per second, up negative) of a drag held at height `y` within
/// `zone` of the top or bottom edge of `rows`, or past it: faster the deeper.
fn edge_scroll_speed(rows: Rect, y: f32, zone: f32) -> f32 {
    let zone = zone.min(rows.height() / 2.0).max(1.0);
    let depth = if y < rows.min.y + zone {
        y - (rows.min.y + zone)
    } else if y > rows.max.y - zone {
        y - (rows.max.y - zone)
    } else {
        return 0.0;
    };
    (depth / zone).clamp(-3.0, 3.0) * 400.0
}

/// Autoscroll speed (points per second, left negative) of a ruler scrub held at `x`, past the
/// left (`x0`) or right (`x1`) end of the visible time span: faster the further past.
fn edge_scroll_speed_x(x0: f32, x1: f32, x: f32) -> f32 {
    let depth = if x < x0 {
        x - x0
    } else if x > x1 {
        x - x1
    } else {
        return 0.0;
    };
    (depth / 40.0).clamp(-3.0, 3.0) * 400.0 + depth.signum() * 100.0
}

/// The timeline's first visible time (`start`, showing `span` seconds of a `duration` comp)
/// paged so the CTI at `cti` is in view: unchanged while it is, else the page that starts at
/// the CTI (as After Effects pages the Timeline during a preview).
fn page_to_cti(start: f64, span: f64, duration: f64, cti: f64) -> f64 {
    if !(start.is_finite() && span.is_finite() && cti.is_finite()) || (cti >= start && cti <= start + span) {
        return start;
    }
    cti.min(duration - span).max(0.0)
}

/// The visible span's start (seconds) that puts `cti` in its middle, kept inside the comp.
fn centered_on(span: f64, duration: f64, cti: f64) -> f64 {
    if !(span.is_finite() && duration.is_finite() && cti.is_finite()) {
        return 0.0;
    }
    (cti - span / 2.0).min(duration - span).max(0.0)
}

/// The text of an expression being edited when its pick whip was pressed, and the selection in
/// it (characters): the pick whip's reference goes in there.
type WhipInto = (String, [usize; 2]);

/// The `egui::Id` under which the expression text captured by an expression pick whip
/// ([`WhipInto`]) is held for property `prop` until that editor is drawn again.
fn whip_into_id(prop: u64) -> egui::Id {
    egui::Id::unique(("tl-pickwhip-into", prop))
}

/// An expression editor to take the keyboard when it is next drawn: its property, the text to
/// select as counts of characters before the end (so it fits the expression as it will be then)
/// and the pass it was asked for in. After a pick whip into the expression being edited, the
/// cursor goes just after the reference; after an Alt-click on a stopwatch adds an expression,
/// its whole text is selected, so typing replaces it.
#[derive(Clone, Copy, Debug)]
struct ExprFocus {
    prop: u64,
    select: [usize; 2],
    pass: u64,
}

/// The `egui::Id` of the pending [`ExprFocus`]: which expression editor takes the keyboard on
/// its next draw, and what to select in it.
fn expr_focus_id() -> egui::Id {
    egui::Id::unique("tl-expr-focus")
}

fn focus_expression(ctx: &egui::Context, prop: u64, select: [usize; 2]) {
    let pass = ctx.cumulative_pass_nr();
    ctx.data_mut(|d| d.insert_temp(expr_focus_id(), ExprFocus { prop, select, pass }));
}

#[derive(Clone, Debug)]
struct Row {
    layer: LayerId,
    depth: usize,
    kind: RowKind,
}

const AV_W: f32 = 76.0;
const LABEL_W: f32 = 20.0;
const NUM_W: f32 = 26.0;
const SW: f32 = 19.0;

/// The Timeline's columns in After Effects' order: (id, header label). Right-click a column
/// header to show or hide them; the name column is always shown.
pub const COLUMNS: [(&str, &str); 13] = [
    ("av", "A/V Features"),
    ("keys", "Keys"),
    ("label", "Label"),
    ("num", "#"),
    ("name", "Layer Name"),
    ("comment", "Comment"),
    ("switches", "Switches"),
    ("modes", "Modes"),
    ("parent", "Parent & Link"),
    ("in", "In"),
    ("out", "Out"),
    ("duration", "Duration"),
    ("stretch", "Stretch"),
];

/// Is a column shown?
pub fn column_visible(tl: &crate::state::TimelineState, id: &str) -> bool {
    match id {
        "name" => true,
        "modes" => tl.show_modes,
        _ => tl.columns.contains(id),
    }
}

/// Show or hide a column (`modes` is the Switches/Modes toggle, F4).
pub fn set_column(tl: &mut crate::state::TimelineState, id: &str, on: bool) -> Result<(), String> {
    match id {
        "name" if on => Ok(()),
        "name" => Err("the name column can't be hidden".into()),
        "modes" => {
            tl.show_modes = on;
            Ok(())
        }
        _ if COLUMNS.iter().any(|c| c.0 == id) => {
            if on {
                tl.columns.insert(id.to_string());
            } else {
                tl.columns.remove(id);
            }
            Ok(())
        }
        _ => Err(format!("unknown column `{id}` (one of: {})", COLUMNS.map(|c| c.0).join(", "))),
    }
}

const KEYS_W: f32 = 56.0;
const COMMENT_W: f32 = 120.0;
const TIME_W: f32 = 72.0;
const STRETCH_W: f32 = 60.0;
const PARENT_W: f32 = 116.0;
const MODES_W: f32 = 92.0 + 112.0;

#[derive(Clone, Copy, Default)]
struct Vis {
    av: bool,
    keys: bool,
    label: bool,
    num: bool,
    comment: bool,
    switches: bool,
    modes: bool,
    parent: bool,
    in_: bool,
    out: bool,
    duration: bool,
    stretch: bool,
}

impl Vis {
    fn of(tl: &crate::state::TimelineState) -> Vis {
        let v = |id| column_visible(tl, id);
        Vis {
            av: v("av"),
            keys: v("keys"),
            label: v("label"),
            num: v("num"),
            comment: v("comment"),
            switches: v("switches"),
            modes: v("modes"),
            parent: v("parent"),
            in_: v("in"),
            out: v("out"),
            duration: v("duration"),
            stretch: v("stretch"),
        }
    }
    /// Width of every visible column but the name.
    fn fixed(&self) -> f32 {
        let w = |on: bool, w: f32| if on { w } else { 0.0 };
        w(self.av, AV_W)
            + w(self.keys, KEYS_W)
            + w(self.label, LABEL_W)
            + w(self.num, NUM_W)
            + w(self.comment, COMMENT_W)
            + w(self.switches, SW * 8.0 + 6.0)
            + w(self.modes, MODES_W)
            + w(self.parent, PARENT_W)
            + w(self.in_, TIME_W)
            + w(self.out, TIME_W)
            + w(self.duration, TIME_W)
            + w(self.stretch, STRETCH_W)
    }
}

struct Cols {
    vis: Vis,
    av: f32,
    keys: f32,
    label: f32,
    num: f32,
    name: f32,
    /// End of the name column.
    name_end: f32,
    comment: f32,
    switches: f32,
    mode: f32,
    trkmat: f32,
    parent: f32,
    in_: f32,
    out: f32,
    duration: f32,
    stretch: f32,
    end: f32,
}

fn cols(x0: f32, width: f32, vis: Vis) -> Cols {
    let name_w = (width - vis.fixed()).max(120.0);
    let mut x = x0;
    let mut next = |on: bool, w: f32| {
        let at = x;
        if on {
            x += w;
        }
        at
    };
    let av = next(vis.av, AV_W);
    let keys = next(vis.keys, KEYS_W);
    let label = next(vis.label, LABEL_W);
    let num = next(vis.num, NUM_W);
    let name = next(true, name_w);
    let comment = next(vis.comment, COMMENT_W);
    let switches = next(vis.switches, SW * 8.0 + 6.0);
    let mode = next(vis.modes, 92.0);
    let trkmat = next(vis.modes, 112.0);
    let parent = next(vis.parent, PARENT_W);
    let in_ = next(vis.in_, TIME_W);
    let out = next(vis.out, TIME_W);
    let duration = next(vis.duration, TIME_W);
    let stretch = next(vis.stretch, STRETCH_W);
    let end = next(true, 0.0);
    Cols { vis, av, keys, label, num, name, name_end: name + name_w, comment, switches, mode, trkmat, parent, in_, out, duration, stretch, end }
}

/// Which column a header x position falls in.
fn column_at(cw: &Cols, x: f32) -> Option<&'static str> {
    let v = cw.vis;
    let spans = [
        ("av", v.av, cw.av, cw.av + AV_W),
        ("keys", v.keys, cw.keys, cw.keys + KEYS_W),
        ("label", v.label, cw.label, cw.label + LABEL_W),
        ("num", v.num, cw.num, cw.num + NUM_W),
        ("name", true, cw.name, cw.name_end),
        ("comment", v.comment, cw.comment, cw.comment + COMMENT_W),
        ("switches", v.switches, cw.switches, cw.switches + SW * 8.0 + 6.0),
        ("modes", v.modes, cw.mode, cw.mode + MODES_W),
        ("parent", v.parent, cw.parent, cw.parent + PARENT_W),
        ("in", v.in_, cw.in_, cw.in_ + TIME_W),
        ("out", v.out, cw.out, cw.out + TIME_W),
        ("duration", v.duration, cw.duration, cw.duration + TIME_W),
        ("stretch", v.stretch, cw.stretch, cw.stretch + STRETCH_W),
    ];
    spans.into_iter().find(|(_, on, a, b)| *on && x >= *a && x < *b).map(|s| s.0)
}

/// Snap a time to the nearest candidate within `tolerance` (all seconds); Shift-dragging the
/// current-time indicator snaps to keyframes, layer in/out points, markers and the work area.
pub fn snap_time(candidates: &[f64], t: f64, tolerance: f64) -> f64 {
    candidates.iter().copied().filter(|c| (c - t).abs() <= tolerance).min_by(|a, b| (a - t).abs().total_cmp(&(b - t).abs())).unwrap_or(t)
}

/// Times the current-time indicator snaps to (comp seconds).
fn snap_candidates(comp: &Comp, rows: &[Row]) -> Vec<f64> {
    let mut v = vec![0.0, comp.work_area.0.seconds(), comp.work_area.1.seconds(), comp.duration.seconds()];
    v.extend(comp.markers.iter().map(|m| m.time.seconds()));
    for l in &comp.layers {
        v.push(l.in_point.seconds());
        v.push(l.out_point.seconds());
        v.extend(l.markers.iter().map(|m| l.comp_time(m.time).seconds()));
    }
    for r in rows {
        if let RowKind::Prop { uid } = r.kind
            && let Some(l) = comp.layer(r.layer)
            && let Some(p) = l.props.find(uid)
        {
            v.extend(p.keys.iter().map(|k| l.comp_time(k.time).seconds()));
        }
    }
    v
}

/// Shift-dragging the work area or one of its ends (#252): the correction (seconds) that puts
/// the nearest of the `moving` times on the current time, a key, a layer in / out point or a
/// marker within 8 px (the work area's own ends are no targets).
fn work_area_snap(app: &EffectcraftApp, comp: &Comp, moving: &[f64], pps: f64) -> f64 {
    let (a, b) = (comp.work_area.0.seconds(), comp.work_area.1.seconds());
    let mut cands = snap_candidates(comp, &build_rows(app, comp));
    cands.retain(|c| (c - a).abs() > 1e-9 && (c - b).abs() > 1e-9);
    cands.push(app.session.time().seconds());
    let tol = 8.0 / pps.max(1e-6);
    moving.iter().map(|t| snap_time(&cands, *t, tol) - t).filter(|d| *d != 0.0).min_by(|x, y| x.abs().total_cmp(&y.abs())).unwrap_or(0.0)
}

/// Timeline horizontal mapping.
#[derive(Clone, Copy, Debug)]
pub(crate) struct TMap {
    pub x0: f32,
    pub start: f64,
    pub pps: f64,
}

impl TMap {
    pub fn x(&self, secs: f64) -> f32 {
        self.x0 + ((secs - self.start) * self.pps) as f32
    }
    pub fn t(&self, x: f32) -> f64 {
        self.start + (x - self.x0) as f64 / self.pps
    }
}

fn disk_bar_range(tm: &TMap, clip: Rect, fd: f64, frames: i64) -> std::ops::Range<i64> {
    if clip.width() <= 0.0 || clip.height() <= 0.0 || !fd.is_finite() || fd <= 0.0 || !tm.pps.is_finite() || tm.pps <= 0.0 {
        return 0..0;
    }
    let start = tm.t(clip.min.x) / fd;
    let end = tm.t(clip.max.x) / fd;
    if !start.is_finite() || !end.is_finite() {
        return 0..0;
    }
    let limit = frames.clamp(0, 100_000) as f64;
    (start.clamp(0.0, limit).floor() as i64)..(end.clamp(0.0, limit).ceil() as i64)
}

/// A Timeline ruler tick label for comp frame `fno`: seconds:frames (`00:15f`, AE) or, when
/// the project's Time Display Style is Frames, the frame number the current-time display shows
/// (`00015`, counted from Start Numbering Frames At) (#710).
fn ruler_label(fno: i64, fps_i: i64, settings: &effectcraft_engine::project::ProjectSettings) -> String {
    if settings.time_display == effectcraft_engine::project::TimeDisplayStyle::Frames {
        return format!("{:05}", fno.saturating_add(settings.frame_start));
    }
    let fps_i = fps_i.max(1);
    if fno >= 60 * fps_i * 60 {
        format!("{}:{:02}:{:02}f", fno / (fps_i * 60), (fno / fps_i) % 60, fno % fps_i)
    } else {
        format!("{:02}:{:02}f", fno / fps_i, fno % fps_i)
    }
}

/// The `egui::Id` under which [`show`] records this frame's time mapping as `(x0, start, pps)`,
/// i.e. the [`TMap`] fields it lays the ruler and graph out with.
fn tl_map_id() -> egui::Id {
    egui::Id::unique("timeline-map")
}

/// The column right of the time graph: the composition marker bin over the layers' scroll bar.
const GUTTER_W: f32 = 18.0;

/// The most a time ruler zooms in (pixels per second).
const MAX_PPS: f64 = 4000.0;

/// One `=` / `-` step (and one click on the footer's zoom buttons).
pub(crate) const ZOOM_STEP: f64 = 1.5;

/// Pixels per second that fit `comp` in a time graph `width` points wide.
pub(crate) fn fit_pps(width: f32, comp: &Comp) -> f64 {
    (width - 10.0).max(1.0) as f64 / comp.duration.seconds().max(0.01)
}

/// Zoom the time ruler to `pps` pixels per second, keeping comp time `at_t` at `at_px` pixels
/// from the ruler's start. Zooming out stops at `fit`, where the whole comp shows (and the
/// ruler keeps fitting it).
pub(crate) fn set_zoom(tl: &mut crate::state::TimelineState, comp: &Comp, pps: f64, fit: f64, at_t: f64, at_px: f64) {
    let npps = pps.min(MAX_PPS.max(fit)).max(fit);
    if npps <= fit {
        tl.pps = None;
        tl.start = 0.0;
        return;
    }
    tl.start = (at_t - at_px / npps).clamp(0.0, comp.duration.seconds());
    tl.pps = Some(npps);
}

/// `;`: from the whole comp, zoom in to frame level around the CTI; otherwise show the whole comp.
pub fn toggle_frame_zoom(app: &mut EffectcraftApp, ctx: &egui::Context) {
    let Some((_, w)) = ctx.data(|d| d.get_temp::<(f32, f32)>(egui::Id::unique("timeline-graph-area"))) else { return };
    let Some(comp) = app.session.active_comp_arc() else { return };
    let fit = fit_pps(w, &comp);
    if app.ui.timeline.pps.is_some_and(|p| p > fit) {
        set_zoom(&mut app.ui.timeline, &comp, fit, fit, 0.0, 0.0);
        return;
    }
    let cti = app.session.time().seconds();
    set_zoom(&mut app.ui.timeline, &comp, MAX_PPS, fit, cti, w as f64 / 2.0);
}

/// `Shift+;`: show the whole comp; from there, go back to the span it was pressed in.
pub fn toggle_fit_zoom(app: &mut EffectcraftApp, ctx: &egui::Context) {
    let tl = &mut app.ui.timeline;
    if let Some(pps) = tl.pps {
        tl.before_fit = Some((tl.start, pps));
        tl.pps = None;
        tl.start = 0.0;
        return;
    }
    let Some((start, pps)) = tl.before_fit.take() else { return };
    let Some((_, w)) = ctx.data(|d| d.get_temp::<(f32, f32)>(egui::Id::unique("timeline-graph-area"))) else { return };
    let Some(comp) = app.session.active_comp_arc() else { return };
    set_zoom(&mut app.ui.timeline, &comp, pps, fit_pps(w, &comp), start, 0.0);
}

/// Zoom the time ruler around the CTI.
pub fn zoom(app: &mut EffectcraftApp, ctx: &egui::Context, k: f64) {
    let Some((_, w)) = ctx.data(|d| d.get_temp::<(f32, f32)>(egui::Id::unique("timeline-graph-area"))) else { return };
    let Some(comp) = app.session.active_comp_arc() else { return };
    let fit = fit_pps(w, &comp);
    let pps = app.ui.timeline.pps.unwrap_or(fit);
    let cti = app.session.time().seconds();
    let rel = (cti - app.ui.timeline.start) * pps;
    set_zoom(&mut app.ui.timeline, &comp, pps * k, fit, cti, rel);
}

/// Screen point of a layer bar (centre, or at a comp time).
pub fn locate(app: &EffectcraftApp, layer: u64, time: Option<f64>) -> Option<(f32, f32)> {
    let e = app.auto.find(&format!("timeline.layer.{layer}.bar"))?;
    let y = e.rect[1] + e.rect[3] / 2.0;
    match time {
        Some(t) => {
            // The ruler element's label carries "start,pps".
            let tm = app.auto.find("timeline.ruler")?;
            let (start, pps) = tm.label.split_once(',').and_then(|(a, b)| Some((a.parse::<f64>().ok()?, b.parse::<f64>().ok()?)))?;
            Some((tm.rect[0] + 6.0 + ((t - start) * pps) as f32, y))
        }
        None => Some((e.rect[0] + e.rect[2] / 2.0, y)),
    }
}

/// Start renaming the first selected layer (Enter in the Timeline).
pub fn begin_rename(app: &mut EffectcraftApp, ctx: &egui::Context) {
    // A selected effect, mask, shape group or text animator renames before its layer (as in
    // After Effects, #1061).
    let group = app.session.state.selected_props.iter().rev().find_map(|(lid, uid)| {
        let g = app.session.active_comp()?.layer(*lid)?.props.find_group(*uid).filter(|g| g.is_instance())?;
        Some((lid.0, g.uid, g.name.clone(), matches!(g.kind, GroupKind::Effect { .. })))
    });
    if let Some((layer, uid, name, is_fx)) = group {
        // An effect in a visible Effect Controls panel renames there, the rest in the Timeline.
        if is_fx && app.auto.previous.iter().chain(app.auto.elements.iter()).any(|e| e.id == format!("effectControls.effect.{uid}.name")) {
            super::effect_controls::start_rename(ctx, uid, &name);
        } else {
            // Show the group's row first: the field lives in it.
            fn ancestors(g: &PropGroup, uid: u64, out: &mut Vec<u64>) -> bool {
                for sg in g.groups() {
                    if sg.uid == uid || ancestors(sg, uid, out) {
                        out.push(sg.uid);
                        return true;
                    }
                }
                false
            }
            let mut open = vec![];
            if let Some(l) = app.session.active_comp().and_then(|c| c.layer(LayerId(layer))) {
                ancestors(&l.props, uid, &mut open);
            }
            open.retain(|u| *u != uid);
            app.ui.timeline.open_layers.insert(layer);
            app.ui.timeline.open_groups.extend(open);
            ctx.data_mut(|d| {
                d.insert_temp(group_rename_id(), (layer, uid, name));
                d.insert_temp(rename_focus_id(), true);
            });
        }
        return;
    }
    if let Some(l) = app.session.state.selected_layers.first().and_then(|id| app.session.active_comp().and_then(|c| c.layer(*id))) {
        start_rename(ctx, l.id.0, &l.name);
    }
}

/// What double-clicking a layer's name or bar does, as in After Effects: a precomp opens, footage
/// and solids open in the Layer panel (paint happens there), text is edited in the viewer with the
/// whole text selected. `None`: the name renames, the bar does nothing.
fn double_click_action(layer: &Layer) -> Option<(String, serde_json::Value)> {
    match &layer.source {
        LayerSource::Comp { item } => Some(("comp.open".into(), json!({"comp": item.0}))),
        LayerSource::Footage { .. } | LayerSource::Solid { .. } => Some(("layer.openLayer".into(), json!({"layer": layer.id.0}))),
        LayerSource::Text => Some(("text.edit".into(), json!({"layer": layer.id.0}))),
        _ => None,
    }
}

/// Open the rename field on layer `id`; it takes the keyboard focus on its first frame only.
fn start_rename(ctx: &egui::Context, id: u64, name: &str) {
    ctx.data_mut(|d| {
        d.insert_temp(egui::Id::unique("tl-rename"), (id, name.to_string()));
        d.insert_temp(rename_focus_id(), true);
    });
}

/// The Timeline's open group rename field: (layer, group uid, text).
fn group_rename_id() -> egui::Id {
    egui::Id::unique("tl-group-rename")
}

/// The `egui::Id` of the one-shot flag that makes the open layer rename field take the keyboard
/// on its first frame (see [`start_rename`]).
fn rename_focus_id() -> egui::Id {
    egui::Id::unique("tl-rename-focus")
}

fn group_visible(g: &PropGroup, layer: &Layer) -> bool {
    match g.match_id.as_str() {
        "masks" | "effects" => !g.children.is_empty(),
        "materialOptions" | "geometryOptions" => layer.is_3d(),
        "audio" => true,
        _ => true,
    }
}

fn prop_visible(p: &Property, layer: &Layer) -> bool {
    if matches!(p.ui, ParamUi::Hidden) {
        return false;
    }
    // Path Options show only the Path popup until a mask path is chosen.
    if let Some(po) = layer.props.group("text/pathOptions")
        && p.match_id != "path"
        && po.get(&p.match_id).is_some_and(|q| q.uid == p.uid)
        && po.get("path").is_some_and(|q| q.value.as_enum() == 0 && q.keys.is_empty())
    {
        return false;
    }
    if p.three_d_only && !layer.is_3d() {
        return false;
    }
    if p.two_d_only && layer.is_3d() {
        return false;
    }
    // One-node cameras (and lights with auto-orient off) have no Point of Interest.
    if p.match_id == "poi" && (layer.is_camera() || layer.is_light()) && layer.auto_orient != effectcraft_engine::project::AutoOrient::TowardsPointOfInterest {
        return false;
    }
    // Separate Dimensions: X/Y/Z Position replace Position.
    if p.match_id == "position" && layer.transform().is_some_and(|tr| tr.get("positionX").is_some() && tr.get("position").is_some_and(|q| q.uid == p.uid)) {
        return false;
    }
    true
}

fn reveal_matches(p: &Property, path_matches: &[&str], kind: &str) -> bool {
    match kind {
        "animated" => p.is_animated() || p.has_expression(),
        "expressions" => p.has_expression(),
        _ => path_matches.contains(&p.match_id.as_str()),
    }
}

/// The name shown for a layer: its name, or its source's name (Source Name column).
fn display_name(app: &EffectcraftApp, l: &Layer) -> String {
    if app.ui.timeline.source_name
        && let Some(item) = l.source.item().and_then(|i| app.session.project.item(i))
    {
        return item.name.clone();
    }
    l.name.clone()
}

/// The timeline search (After Effects' search field): rows for the properties and groups of a
/// layer whose names contain `q` (lowercase), each with its enclosing groups. A matching group
/// brings its whole contents.
fn search_rows(l: &Layer, q: &str) -> Vec<Row> {
    /// The row for a group in search results: depth, its effect's enabled flag, and the eye
    /// switch for layer-style groups (not Blending Options).
    fn group_row(l: &Layer, g: &PropGroup, depth: usize, parent: &PropGroup) -> Row {
        let fx = matches!(g.kind, GroupKind::Effect { .. }).then_some(g.enabled);
        let eye =
            (parent.match_id == effectcraft_engine::project::styles::GROUP && g.match_id != effectcraft_engine::project::styles::BLENDING).then_some(g.enabled);
        Row { layer: l.id, depth, kind: RowKind::Group { uid: g.uid, name: g.name.clone(), open: true, has_children: !g.children.is_empty(), fx, eye } }
    }
    /// Every visible child of `g` at `depth`, groups recursed into whole — a matching group
    /// brings its contents. A shape group's inner Contents is elided (as in AE): no row, and
    /// its items list at the same depth as the group itself.
    fn all(l: &Layer, g: &PropGroup, depth: usize, out: &mut Vec<Row>) {
        for c in &g.children {
            match c {
                // A shape group's inner Contents is elided (as in AE): no row, items at the same depth.
                Node::Group(sg) if g.match_id == "group" && sg.match_id == "contents" => all(l, sg, depth, out),
                Node::Group(sg) => {
                    out.push(group_row(l, sg, depth, g));
                    all(l, sg, depth + 1, out);
                }
                Node::Prop(p) if prop_visible(p, l) => out.push(Row { layer: l.id, depth, kind: RowKind::Prop { uid: p.uid } }),
                _ => {}
            }
        }
    }
    /// The children of `g` that match `q`: a name-matching group brings its whole contents
    /// (`all`); otherwise the search recurses and keeps only subtrees with hits. A shape
    /// group's inner Contents is searched through without a row of its own (as in AE).
    fn rec(l: &Layer, g: &PropGroup, depth: usize, q: &str, out: &mut Vec<Row>) {
        for c in &g.children {
            match c {
                // A shape group's inner Contents is elided (as in AE): search through it, no row.
                Node::Group(sg) if g.match_id == "group" && sg.match_id == "contents" => rec(l, sg, depth, q, out),
                Node::Group(sg) if depth > 1 || group_visible(sg, l) => {
                    // An effect group also matches the name the row draws, so a search finds
                    // `高斯模糊` in a Chinese interface and `Gaussian Blur` in an English one.
                    let hit = sg.name.to_lowercase().contains(q)
                        || matches!(&sg.kind, GroupKind::Effect { effect } if crate::i18n::effects::matches(effect, &sg.name, q));
                    if hit {
                        out.push(group_row(l, sg, depth, g));
                        all(l, sg, depth + 1, out);
                        continue;
                    }
                    let mut inner = vec![];
                    rec(l, sg, depth + 1, q, &mut inner);
                    if !inner.is_empty() {
                        out.push(group_row(l, sg, depth, g));
                        out.extend(inner);
                    }
                }
                Node::Prop(p) if prop_visible(p, l) && p.name.to_lowercase().contains(q) => {
                    out.push(Row { layer: l.id, depth, kind: RowKind::Prop { uid: p.uid } });
                }
                _ => {}
            }
        }
    }
    let mut out = vec![];
    rec(l, &l.props, 1, q, &mut out);
    out
}

/// The property each reveal key shows (P, S, R, T, A, F, L; `animated` = keyframed or with an
/// expression): (group searched, property match ids).
fn reveal_targets(kind: &str) -> Option<(&'static str, &'static [&'static str])> {
    Some(match kind {
        "position" => ("transform", &["position", "positionX", "positionY", "positionZ"]),
        "scale" => ("transform", &["scale"]),
        "rotation" => ("transform", &["rotation", "rotationX", "rotationY", "orientation"]),
        "opacity" => ("transform", &["opacity"]),
        "anchor" => ("transform", &["anchor"]),
        "feather" => ("masks", &["feather"]),
        "maskPath" => ("masks", &["path"]),
        "maskOpacity" => ("masks", &["opacity"]),
        "levels" => ("audio", &["levels"]),
        "timeRemap" => ("", &["timeRemap"]),
        "animated" | "expressions" => ("", &[]),
        _ => return None,
    })
}

/// The properties the Timeline shows (revealed rows), as `{layer, prop}`: J / K and Select All
/// Keyframes act on these, as in After Effects.
pub(crate) fn visible_props(app: &EffectcraftApp, comp: &Comp) -> Vec<serde_json::Value> {
    build_rows(app, comp)
        .iter()
        .filter_map(|r| match r.kind {
            RowKind::Prop { uid } => Some(json!({"layer": r.layer.0, "prop": uid})),
            _ => None,
        })
        .collect()
}

fn build_rows(app: &EffectcraftApp, comp: &Comp) -> Vec<Row> {
    let tl = &app.ui.timeline;
    let search = tl.search.trim().to_lowercase();
    let mut rows = Vec::new();
    for l in &comp.layers {
        if comp.hide_shy && l.switches.shy {
            continue;
        }
        if !search.is_empty() {
            // Layers whose name or properties match; matching properties are revealed.
            let matches = search_rows(l, &search);
            if matches.is_empty() && !display_name(app, l).to_lowercase().contains(&search) {
                continue;
            }
            rows.push(Row { layer: l.id, depth: 0, kind: RowKind::Layer });
            rows.extend(matches);
            continue;
        }
        rows.push(Row { layer: l.id, depth: 0, kind: RowKind::Layer });
        // Essential Graphics ▸ Solo Supported Properties: every property it can expose.
        if app.session.state.essential_solo {
            let mut v = vec![];
            for (i, c) in l.props.children.iter().enumerate() {
                if c.match_id() == effectcraft_engine::project::essential::GROUP {
                    continue;
                }
                let g = PropGroup { children: vec![l.props.children[i].clone()], ..PropGroup::new(0, "", "") };
                collect_props(&g, &mut v, &|p| prop_visible(p, l) && effectcraft_engine::project::essential::control_type(p).is_some());
            }
            rows.extend(v.into_iter().map(|uid| Row { layer: l.id, depth: 1, kind: RowKind::Prop { uid } }));
            continue;
        }
        if !tl.open_layers.contains(&l.id.0) {
            continue;
        }
        if let Some(kinds) = tl.layer_reveal.get(&l.id.0).filter(|k| !k.is_empty()) {
            reveal_rows(app, l, kinds, &mut rows);
            continue;
        }
        let classic_geometry = classic_geometry_row(comp, l);
        for c in &l.props.children {
            match c {
                // The Change Renderer row (after Transform) stands in for the extrusion options.
                Node::Group(g) if classic_geometry && g.match_id == "geometryOptions" => {}
                Node::Group(g) if group_visible(g, l) => {
                    let open = tl.open_groups.contains(&g.uid);
                    rows.push(Row {
                        layer: l.id,
                        depth: 1,
                        kind: RowKind::Group {
                            uid: g.uid,
                            name: g.name.clone(),
                            open,
                            has_children: !g.children.is_empty(),
                            fx: None,
                            eye: (g.match_id == effectcraft_engine::project::styles::GROUP).then_some(g.enabled),
                        },
                    });
                    if open {
                        push_group(&mut rows, l, g, 2, &tl.open_groups);
                        // Audio > Waveform > Waveform (footage with audio).
                        if g.match_id == "audio"
                            && let Some(item) = super::waveform::audio_item(&app.session.project, l)
                        {
                            let wuid = WAVE_BIT | g.uid;
                            let wopen = tl.open_groups.contains(&wuid);
                            rows.push(Row {
                                layer: l.id,
                                depth: 2,
                                kind: RowKind::Group { uid: wuid, name: "Waveform".into(), open: wopen, has_children: true, fx: None, eye: None },
                            });
                            if wopen {
                                rows.push(Row { layer: l.id, depth: 3, kind: RowKind::Waveform { item: item.0 } });
                            }
                        }
                    }
                    if classic_geometry && g.match_id == "transform" {
                        rows.push(Row {
                            layer: l.id,
                            depth: 1,
                            kind: RowKind::Group {
                                uid: GEOM_BIT | l.id.0,
                                name: "Geometry Options".into(),
                                open: false,
                                has_children: false,
                                fx: None,
                                eye: None,
                            },
                        });
                    }
                }
                Node::Prop(p) if prop_visible(p, l) => rows.push(Row { layer: l.id, depth: 1, kind: RowKind::Prop { uid: p.uid } }),
                _ => {}
            }
        }
    }
    rows
}

/// Rows of a twirled-open layer under the reveal shortcuts (one, or several added with Shift):
/// masks, effects, the revealed properties in property-tree order, the Reveal Properties
/// selection, then the waveform.
fn reveal_rows(app: &EffectcraftApp, l: &Layer, kinds: &[String], rows: &mut Vec<Row>) {
    let tl = &app.ui.timeline;
    let has = |k: &str| kinds.iter().any(|r| r == k);
    // Groups shown with their whole contents: the revealed properties inside aren't listed again.
    let mut shown_groups: Vec<u64> = vec![];
    let mut group_rows = |rows: &mut Vec<Row>, g: &PropGroup, fx: bool| {
        for g in g.groups() {
            shown_groups.push(g.uid);
            rows.push(Row {
                layer: l.id,
                depth: 1,
                kind: RowKind::Group {
                    uid: g.uid,
                    name: g.name.clone(),
                    open: tl.open_groups.contains(&g.uid),
                    has_children: true,
                    fx: fx.then_some(g.enabled),
                    eye: None,
                },
            });
            if tl.open_groups.contains(&g.uid) {
                push_group(rows, l, g, 2, &tl.open_groups);
            }
        }
    };
    if has("masks")
        && let Some(m) = l.masks()
    {
        group_rows(rows, m, false);
    }
    if has("effects")
        && let Some(fx) = l.effects()
    {
        group_rows(rows, fx, true);
    }
    // PP: paint, Roto Brush and Puppet effects; FF: effects whose plug-in is missing.
    for (k, keep) in [
        ("paint", &(|id: &str| matches!(id, "ec.paint.paint" | "ec.matte.rotobrush" | "ec.distort.puppet")) as &dyn Fn(&str) -> bool),
        ("missingEffects", &|id: &str| effectcraft_engine::effects::find(id).is_none()),
    ] {
        if has(k)
            && let Some(fx) = l.effects()
        {
            let mut only = fx.clone();
            only.children.retain(|c| matches!(c, Node::Group(g) if matches!(&g.kind, GroupKind::Effect { effect } if keep(effect))));
            group_rows(rows, &only, true);
        }
    }
    // AA: Material Options (3D layers).
    if has("material")
        && let Some(m) = l.props.sub("materialOptions")
        && l.is_3d()
    {
        let mut only = PropGroup::new(0, "", "");
        only.children.push(Node::Group(m.clone()));
        group_rows(rows, &only, false);
    }
    // P/S/R/T/A/F/L/animated: matching properties, in tree order, each once.
    let mut wanted = std::collections::BTreeSet::new();
    for kind in kinds {
        let Some((group, ids)) = reveal_targets(kind) else { continue };
        let root = match group {
            "" => Some(&l.props),
            "masks" => l.masks(),
            g => l.props.sub(g),
        };
        if let Some(g) = root {
            let mut found = vec![];
            collect_props(g, &mut found, &|p| prop_visible(p, l) && reveal_matches(p, ids, kind));
            wanted.extend(found);
        }
    }
    // SS: the selected properties and groups (as Animation ▸ Reveal Properties shows them).
    let selected: std::collections::BTreeSet<u64> = app.session.state.selected_props.iter().filter(|(lid, _)| *lid == l.id).map(|(_, u)| *u).collect();
    if has("props") || has("selected") {
        let picked: std::collections::BTreeSet<u64> =
            if has("props") { tl.reveal_props.iter().copied().chain(selected.iter().copied().filter(|_| has("selected"))).collect() } else { selected };
        let reveal_props = &picked;
        // Animation ▸ Reveal Properties…: the engine picked the uids.
        fn groups_in<'a>(g: &'a PropGroup, set: &std::collections::BTreeSet<u64>, out: &mut Vec<&'a PropGroup>) {
            for sg in g.groups() {
                if set.contains(&sg.uid) {
                    out.push(sg);
                } else {
                    groups_in(sg, set, out);
                }
            }
        }
        let mut groups = vec![];
        groups_in(&l.props, reveal_props, &mut groups);
        for g in groups {
            let open = tl.open_groups.contains(&g.uid);
            rows.push(Row {
                layer: l.id,
                depth: 1,
                kind: RowKind::Group { uid: g.uid, name: g.name.clone(), open, has_children: !g.children.is_empty(), fx: None, eye: None },
            });
            if open {
                push_group(rows, l, g, 2, &tl.open_groups);
            }
            shown_groups.push(g.uid);
        }
        wanted.extend(picked.iter().copied());
    }
    revealed_rows(rows, l, &wanted, &shown_groups, &tl.open_groups, tl.full_groups.get(&l.id.0));
    if has("waveform")
        && let Some(item) = super::waveform::audio_item(&app.session.project, l)
    {
        rows.push(Row { layer: l.id, depth: 1, kind: RowKind::Waveform { item: item.0 } });
    }
}

/// Rows for the revealed properties `shown` of `l`, in tree order. Those inside an effect show
/// under the effect and its groups, as in After Effects (Puppet ▸ Mesh 1 ▸ Deform ▸ Puppet
/// Pin 1 ▸ Position: every pin's Position is told apart, #273); the others (Transform, masks…)
/// on their own. Groups in `skip` already show with their contents; those in `full` (twirled
/// open by hand since the reveal) show all of theirs.
fn revealed_rows(
    rows: &mut Vec<Row>,
    l: &Layer,
    shown: &std::collections::BTreeSet<u64>,
    skip: &[u64],
    open: &std::collections::BTreeSet<u64>,
    full: Option<&std::collections::BTreeSet<u64>>,
) {
    struct Tree<'a> {
        l: &'a Layer,
        wanted: &'a dyn Fn(&Property) -> bool,
        skip: &'a [u64],
        open: &'a std::collections::BTreeSet<u64>,
        full: Option<&'a std::collections::BTreeSet<u64>>,
    }
    fn holds(g: &PropGroup, wanted: &dyn Fn(&Property) -> bool) -> bool {
        g.children.iter().any(|c| match c {
            Node::Prop(p) => wanted(p),
            Node::Group(sg) => holds(sg, wanted),
        })
    }
    // A group holding revealed properties (twirled as the user left it), then those inside.
    fn group(rows: &mut Vec<Row>, t: &Tree<'_>, g: &PropGroup, depth: usize) {
        if t.skip.contains(&g.uid) || !holds(g, t.wanted) {
            return;
        }
        let o = t.open.contains(&g.uid);
        let fx = matches!(g.kind, GroupKind::Effect { .. }).then_some(g.enabled);
        rows.push(Row { layer: t.l.id, depth, kind: RowKind::Group { uid: g.uid, name: g.name.clone(), open: o, has_children: true, fx, eye: None } });
        if !o {
            return;
        }
        if t.full.is_some_and(|f| f.contains(&g.uid)) {
            push_group(rows, t.l, g, depth + 1, t.open);
            return;
        }
        for c in &g.children {
            match c {
                Node::Prop(p) if (t.wanted)(p) => rows.push(Row { layer: t.l.id, depth: depth + 1, kind: RowKind::Prop { uid: p.uid } }),
                Node::Group(sg) => group(rows, t, sg, depth + 1),
                Node::Prop(_) => {}
            }
        }
    }
    let wanted = |p: &Property| shown.contains(&p.uid) && prop_visible(p, l);
    let tree = Tree { l, wanted: &wanted, skip, open, full };
    for c in &l.props.children {
        match c {
            Node::Group(fx) if fx.match_id == "effects" => {
                for e in fx.groups() {
                    group(rows, &tree, e, 1);
                }
            }
            Node::Group(g) => {
                let mut found = vec![];
                collect_props(g, &mut found, &wanted);
                rows.extend(found.into_iter().map(|uid| Row { layer: l.id, depth: 1, kind: RowKind::Prop { uid } }));
            }
            Node::Prop(p) if wanted(p) => rows.push(Row { layer: l.id, depth: 1, kind: RowKind::Prop { uid: p.uid } }),
            Node::Prop(_) => {}
        }
    }
}

/// Uids of the groups that hold property `uid` of `l`, outermost first.
fn enclosing_groups(l: &Layer, uid: u64) -> Vec<u64> {
    let path = l.props.path_of(uid).unwrap_or_default();
    let mut ids: Vec<u64> = path.split('/').filter_map(|s| s.strip_prefix('@')?.parse().ok()).collect();
    ids.pop();
    ids
}

/// Twirl open the groups holding `props` of `layer` that sit inside effects, so their rows show
/// (a U reveal, an EE reveal, a puppet pin placed or moved in the viewer).
pub(crate) fn open_effect_paths(app: &mut EffectcraftApp, layer: u64, props: &[u64]) {
    let Some(l) = app.session.active_comp().and_then(|c| c.layer(LayerId(layer))) else { return };
    let Some(fx) = l.effects().map(|g| g.uid) else { return };
    let mut open = vec![];
    for p in props {
        let path = enclosing_groups(l, *p);
        if path.first() == Some(&fx) {
            open.extend(path);
        }
    }
    app.ui.timeline.open_groups.extend(open);
}

/// After a reveal shortcut that picks properties by state (EE expressions): those inside effects
/// on `layers` show under their effect, twirled open.
pub(crate) fn open_revealed(app: &mut EffectcraftApp, layers: &[u64], kinds: &[String]) {
    for kind in kinds.iter().filter(|k| matches!(k.as_str(), "animated" | "expressions")) {
        for id in layers {
            let Some(l) = app.session.active_comp().and_then(|c| c.layer(LayerId(*id))) else { continue };
            let mut found = vec![];
            collect_props(&l.props, &mut found, &|p| prop_visible(p, l) && reveal_matches(p, &[], kind));
            open_effect_paths(app, *id, &found);
        }
    }
}

/// After the viewer edits a property (a puppet pin placed, moved or recorded): when `layer` is
/// twirled open, its groups open, and a U reveal of the layer takes it in, so its keyframes show
/// (#273). A collapsed layer stays collapsed, as in After Effects.
pub fn keep_in_view(app: &mut EffectcraftApp, layer: LayerId, prop: u64) {
    if !app.ui.timeline.open_layers.contains(&layer.0) {
        return;
    }
    open_effect_paths(app, layer.0, &[prop]);
    let tl = &mut app.ui.timeline;
    if tl.layer_reveal.get(&layer.0).is_some_and(|k| k.iter().any(|k| k == "props")) {
        tl.reveal_props.insert(prop);
    }
}

fn collect_props(g: &PropGroup, out: &mut Vec<u64>, f: &dyn Fn(&Property) -> bool) {
    for c in &g.children {
        match c {
            Node::Prop(p) if f(p) => out.push(p.uid),
            Node::Group(g) => collect_props(g, out, f),
            _ => {}
        }
    }
}

/// The rows for the children of `g` at `depth`: groups recurse into when their twirl-down is
/// in `open`, text animators list their selectors and properties directly (as in AE), and a
/// shape group's inner Contents (AE's elided `ADBE Vectors Group`) is skipped so its items
/// list directly under the group.
fn push_group(rows: &mut Vec<Row>, l: &Layer, g: &PropGroup, depth: usize, open: &std::collections::BTreeSet<u64>) {
    for c in &g.children {
        match c {
            // Text animators list their selectors and properties directly (as in AE).
            Node::Group(sg) if g.match_id == "animator" && matches!(sg.match_id.as_str(), "selectors" | "properties") => {
                push_group(rows, l, sg, depth, open);
            }
            // A shape group's inner Contents (AE's elided `ADBE Vectors Group`): the group's items
            // (path, fill/stroke, Transform) list directly under it, with no "Contents" row.
            Node::Group(sg) if g.match_id == "group" && sg.match_id == "contents" => {
                push_group(rows, l, sg, depth, open);
            }
            Node::Group(sg) => {
                let o = open.contains(&sg.uid);
                let fx = matches!(sg.kind, GroupKind::Effect { .. }).then_some(sg.enabled);
                // Layer style groups have eye switches (not Blending Options).
                let eye = (g.match_id == effectcraft_engine::project::styles::GROUP && sg.match_id != effectcraft_engine::project::styles::BLENDING)
                    .then_some(sg.enabled);
                rows.push(Row {
                    layer: l.id,
                    depth,
                    kind: RowKind::Group { uid: sg.uid, name: sg.name.clone(), open: o, has_children: !sg.children.is_empty(), fx, eye },
                });
                if o {
                    push_group(rows, l, sg, depth + 1, open);
                }
            }
            Node::Prop(p) if prop_visible(p, l) => rows.push(Row { layer: l.id, depth, kind: RowKind::Prop { uid: p.uid } }),
            _ => {}
        }
    }
}

fn layer_icon(l: &Layer, project: &effectcraft_engine::project::Project) -> Icon {
    match &l.source {
        LayerSource::Text => Icon::TextLayer,
        LayerSource::Shape => Icon::ShapeLayer,
        LayerSource::Null => Icon::Null,
        LayerSource::Camera => Icon::Camera,
        LayerSource::Light { .. } => Icon::Light,
        LayerSource::Model { .. } | LayerSource::Primitive { .. } => Icon::Cube,
        LayerSource::Comp { .. } => Icon::Comp,
        LayerSource::Solid { .. } => {
            if l.switches.adjustment {
                Icon::Adjustment
            } else {
                Icon::Solid
            }
        }
        LayerSource::Footage { item } => match project.item(*item).map(|i| i.type_name()) {
            Some("Image") | Some("Image Sequence") => Icon::Image,
            Some("Audio") => Icon::Audio,
            _ => Icon::Footage,
        },
    }
}

/// Layers being dragged to a new place in the stack (timeline rows).
fn layer_drag_id() -> egui::Id {
    egui::Id::unique("tl-layer-drag")
}

/// A keyframe drag in progress: where the pointer started, the grabbed key's comp time
/// (seconds), how many frames the selected keys have moved so far and, for a Quick Offset
/// (Ctrl/Cmd+Alt), how many layers it staggers.
#[derive(Clone, Copy, Debug)]
struct KeyDrag {
    origin_x: f32,
    anchor: f64,
    applied: i64,
    quick: Option<usize>,
}

/// The `egui::Id` of the keyframe drag in progress ([`KeyDrag`]): its start, the grabbed key's
/// time and how far the selection has moved so far.
fn key_drag_id() -> egui::Id {
    egui::Id::unique("tl-key-drag")
}

/// A Quick Offset drag of selected layer bars (Ctrl/Cmd+Alt, #1012): the bar it started on, where
/// the pointer started, how many layers it staggers and how many frames the last one has moved.
#[derive(Clone, Copy, Debug)]
struct QuickBar {
    layer: LayerId,
    origin_x: f32,
    layers: usize,
    applied: i64,
}

/// The `egui::Id` of a Quick Offset drag of layer bars in progress ([`QuickBar`]).
fn quick_bar_id() -> egui::Id {
    egui::Id::unique("tl-quick-bar")
}

/// How many layers a Quick Offset of `layers` staggers (#1012): the unlocked ones, each once, if
/// there are at least two.
fn quick_offset_layers(comp: &Comp, layers: impl IntoIterator<Item = LayerId>) -> Option<usize> {
    let mut seen: Vec<LayerId> = vec![];
    for id in layers {
        if !seen.contains(&id) && comp.layer(id).is_some_and(|l| !l.switches.locked) {
            seen.push(id);
        }
    }
    (seen.len() >= 2).then_some(seen.len())
}

/// A Quick Offset's feedback: the last layer's move and the step from one layer to the next.
fn quick_offset_note(frames: i64, layers: usize) -> String {
    let per = frames as f64 / layers.saturating_sub(1).max(1) as f64;
    format!("{}: {:+} f   {}: {:+.2} f", tr("Total Offset"), frames, tr("Per Layer"), per)
}

/// A Timeline drag's readout by the pointer (After Effects shows it in the Info panel).
fn drag_tip(ctx: &egui::Context, t: &Tokens, ptr: Pos2, note: String) {
    let fg = ctx.layer_painter(egui::LayerId::new(egui::Order::Tooltip, egui::Id::unique("tl-key-drag-tip")));
    let g = fg.layout_no_wrap(note, Tokens::ui(11.5), t.text);
    let r = Rect::from_min_size(ptr + vec2(14.0, 14.0), g.size() + vec2(10.0, 6.0));
    fg.rect_filled(r, 3.0, t.panel_bg);
    fg.rect_stroke(r, 3.0, Stroke::new(1.0, t.field_border), StrokeKind::Inside);
    fg.galley(r.min + vec2(5.0, 3.0), g, t.text);
}

/// A switch pressed and dragged over other rows: the switch, the state the press gave it, and
/// the undo step the drag folds into.
#[derive(Clone, Debug)]
struct SwitchDrag {
    switch: &'static str,
    value: bool,
    merge: String,
}

/// The `egui::Id` of the switch press-and-drag in progress ([`SwitchDrag`]): the switch, the
/// state the press gave it and the undo step the drag folds into.
fn switch_drag_id() -> egui::Id {
    egui::Id::unique("tl-switch-drag")
}

/// The press-and-drag gesture of the Timeline's switches and stopwatches (`switch`, on `row`,
/// now `on`, pressed through `resp`): the press gives it the opposite state, and dragging on
/// gives every `switch` the pointer passes that same state, in one undo step, whatever its own
/// (After Effects). The state to set and the undo step to fold it into, when it changes now.
fn switch_gesture(ui: &egui::Ui, resp: &egui::Response, row: Rect, switch: &'static str, on: bool) -> Option<(bool, String)> {
    if resp.is_pointer_button_down_on() && ui.input(|i| i.pointer.primary_pressed()) {
        let merge = format!("tl-switch-{}", ui.input(|i| i.time));
        ui.data_mut(|d| d.insert_temp(switch_drag_id(), SwitchDrag { switch, value: !on, merge: merge.clone() }));
        return Some((!on, merge));
    }
    let d = ui.data(|d| d.get_temp::<SwitchDrag>(switch_drag_id())).filter(|d| d.switch == switch && d.value != on)?;
    // The heights the pointer moved over since the last frame, so a quick drag skips no row.
    let (a, b) = ui.input(|i| i.pointer.interact_pos().map(|p| (p.y - i.pointer.delta().y, p.y)))?;
    (a.min(b) < row.max.y && a.max(b) >= row.min.y).then_some((d.value, d.merge))
}

/// The layers an edit made on `layer`'s row applies to (a switch, its label, its blending mode):
/// the selection when the row is selected, else that layer alone, as in After Effects.
fn edited_layers(layer: LayerId, selected: &[LayerId]) -> Vec<u64> {
    if selected.contains(&layer) { selected.iter().map(|l| l.0).collect() } else { vec![layer.0] }
}

/// Whether the Timeline shows switch `name`'s cell on `layer`'s row (A/V Features and Switches
/// columns): a layer that has no such cell isn't one a click gives the switch to either.
fn switch_applies(layer: &Layer, name: &str, project: &Project) -> bool {
    match name {
        "video" | "lock" | "shy" => true,
        "audio" => layer.props.sub("audio").is_some(),
        "solo" | "quality" | "motionBlur" | "adjustment" => layer.source.is_av(),
        "collapse" => collapse_applies(layer, project),
        "fx" => layer.effects().is_some_and(|f| !f.children.is_empty()),
        "frameBlend" => matches!(layer.source, LayerSource::Footage { .. } | LayerSource::Comp { .. }),
        "threeD" => layer.source.is_av() || matches!(layer.source, LayerSource::Null),
        _ => true,
    }
}

/// The layers a click on the `name` switch of `layer`'s row acts on: [`edited_layers`] without
/// the layers the switch doesn't apply to.
fn switch_targets(comp: &Comp, project: &Project, selected: &[LayerId], layer: u64, name: &str) -> Vec<u64> {
    edited_layers(LayerId(layer), selected).into_iter().filter(|l| comp.layer(LayerId(*l)).is_some_and(|l| switch_applies(l, name, project))).collect()
}

/// The layers a Modes column edit on `layer`'s row (its blending mode, Preserve Transparency)
/// acts on: [`edited_layers`] that have video.
fn mode_targets(comp: &Comp, layer: LayerId, selected: &[LayerId]) -> Vec<u64> {
    edited_layers(layer, selected).into_iter().filter(|l| comp.layer(LayerId(*l)).is_some_and(|l| l.source.is_av())).collect()
}

/// Where and what a layer switch is: its cell, icon, state and the switch it sets.
#[derive(Clone, Copy)]
struct SwitchSpec {
    row: Rect,
    br: Rect,
    icon: Icon,
    on: bool,
    id: egui::Id,
    layer: u64,
    name: &'static str,
    others: bool,
}

/// A layer's switch (A/V Features and Switches columns) in `row`, with the
/// [`switch_gesture`].
fn layer_switch(
    ui: &mut egui::Ui,
    t: &Tokens,
    spec: &SwitchSpec,
    comp: &Comp,
    project: &Project,
    selected: &[LayerId],
    actions: &mut Vec<(String, serde_json::Value)>,
) {
    let SwitchSpec { row, br, icon, on, id, layer, name, others } = *spec;
    let resp = widgets::icon_toggle(ui, br, icon, on, t, id, Sense::click_and_drag());
    // Alt-click on an A/V feature sets the layer apart: the other layers go off (or locked),
    // and come back on the next Alt-click. With several layers selected, the whole selection is
    // set apart.
    if others && resp.is_pointer_button_down_on() && ui.input(|i| i.pointer.primary_pressed() && i.modifiers.alt) {
        let layers = edited_layers(LayerId(layer), selected);
        actions.push(("layer.setSwitch".into(), json!({"layers": layers, "switch": name, "others": true})));
        return;
    }
    // A press on a selected layer's switch gives the switch to every selected layer, as in After
    // Effects; the targets are picked when the press happens, never while the row is drawn.
    if let Some((value, merge)) = switch_gesture(ui, &resp, row, name, on) {
        let layers = switch_targets(comp, project, selected, layer, name);
        actions.push(("layer.setSwitch".into(), json!({"layers": layers, "switch": name, "value": value, "merge": merge})));
    } else if resp.clicked() && !resp.clicked_by(egui::PointerButton::Primary) {
        // Keyboard or accessibility activation (a pointer press was handled above).
        let layers = switch_targets(comp, project, selected, layer, name);
        actions.push(("layer.setSwitch".into(), json!({"layers": layers, "switch": name, "value": !on})));
    }
}

/// The `egui::Id` under which composition `comp` remembers the layer the last click selected, so
/// a following Shift-click can select the range from there ([`shift_range`]).
fn anchor_id(comp: ItemId) -> egui::Id {
    egui::Id::unique(("timeline-selection-anchor", comp.0))
}

/// Remember `layer` as where the next Shift-click selection starts.
fn set_anchor(ctx: &egui::Context, comp: ItemId, layer: LayerId) {
    ctx.data_mut(|d| d.insert_temp(anchor_id(comp), layer));
}

/// The layers a Shift-click on `layer` (its name or its bar) selects: those from the last layer
/// clicked (else the first selected) to it in stack order, but locked layers and layers the
/// Timeline doesn't show. `None` without such a layer.
fn shift_range(ctx: &egui::Context, cid: ItemId, comp: &Comp, rows: &[Row], selected: &[LayerId], layer: LayerId) -> Option<Vec<u64>> {
    let anchor = ctx.data(|d| d.get_temp::<LayerId>(anchor_id(cid))).or_else(|| selected.first().copied())?;
    let a = comp.layers.iter().position(|l| l.id == anchor)?;
    let b = comp.layers.iter().position(|l| l.id == layer)?;
    let span = comp.layers.get(a.min(b)..=a.max(b))?;
    Some(span.iter().filter(|l| !l.switches.locked && rows.iter().any(|r| r.layer == l.id)).map(|l| l.id.0).collect())
}

/// Set on the frame the reorder drag is released.
/// Where layers dropped at height `py` land among the visible layer rows: above the first row
/// whose middle is below it (its index in `layer_rows`), else below the last row. Returns that
/// and the y of the line that shows it (`top` without rows).
fn insertion(layer_rows: &[(Rect, LayerId)], py: f32, top: f32) -> (Option<usize>, f32) {
    let at = layer_rows.iter().position(|(r, _)| py < r.center().y);
    let y = match at {
        Some(i) => layer_rows.get(i).map_or(top, |(r, _)| r.min.y),
        None => layer_rows.last().map_or(top, |(r, _)| r.max.y),
    };
    (at, y)
}

/// The `egui::Id` of the flag the row drag's release sets, telling the same frame's reorder
/// drawing code to apply the drop.
fn layer_drop_id() -> egui::Id {
    egui::Id::unique("tl-layer-drop")
}

/// Whether a Project item can replace a layer's source (the same set `layer.replaceSource`
/// accepts): compositions, footage and solids; not folders, data files or 3D models.
fn replaceable_item(it: &effectcraft_engine::project::Item) -> bool {
    match &it.kind {
        ItemKind::Comp(_) | ItemKind::Solid(_) => true,
        ItemKind::Footage(f) => !matches!(f.kind, FootageKind::Data | FootageKind::Model),
        ItemKind::Folder => false,
    }
}

/// The Alt/Option-drag target: the layer under the pointer can take this item as its source, and
/// the item wouldn't put the composition inside itself.
fn replace_source_target(app: &EffectcraftApp, cid: ItemId, item: ItemId, lid: LayerId, comp: &Comp) -> bool {
    app.session.project.item(item).is_some_and(replaceable_item)
        && !app.session.project.comp_contains(item, cid)
        && comp
            .layer(lid)
            .is_some_and(|l| !l.switches.locked && matches!(l.source, LayerSource::Footage { .. } | LayerSource::Comp { .. } | LayerSource::Solid { .. }))
}

/// The frame's layout and state shared by the Timeline's drawing steps.
struct Tl {
    t: Tokens,
    cid: ItemId,
    time: Tick,
    fr: effectcraft_engine::time::FrameRate,
    rect: Rect,
    header_h: f32,
    colhdr_h: f32,
    footer_h: f32,
    vis: Vis,
    left_w: f32,
    overflow: f32,
    oscroll: f32,
    cw: Cols,
    graph_x0: f32,
    graph_x1: f32,
    fit_pps: f64,
    pps: f64,
    scroll_to: Option<crate::state::ScrollTo>,
    tm: TMap,
    top: f32,
    rows_rect: Rect,
    ruler: Rect,
    bin: Rect,
    tc_rect: Rect,
    wa_row: Rect,
    fd: f64,
    rh: f32,
    max_scroll: f32,
    ctx: egui::Context,
    comp: Arc<Comp>,
    p: egui::Painter,
    rows: Arc<Vec<Row>>,
    actions: Vec<(String, serde_json::Value)>,
}

/// [`Tl`] plus what the layer rows and the interactions after them share.
struct Rw {
    tl: Tl,
    lp: egui::Painter,
    gp: egui::Painter,
    graph_on: bool,
    full_clip: Rect,
    left_clip: Rect,
    right_clip: Rect,
    empty_rect: Rect,
    outline_rect: Rect,
    empty: egui::Response,
    outline_empty: egui::Response,
    selected: Vec<LayerId>,
    sel_keys: Vec<effectcraft_engine::KeyRef>,
    ui_actions: Vec<UiAct>,
    hit_rows: Vec<(Rect, Row)>,
    dim_hits: Vec<(u64, Vec<Rect>)>,
}

/// One visible row: what the per-kind drawing steps need to know about it.
struct RowCtx<'a> {
    row: &'a Row,
    layer: &'a Layer,
    r: Rect,
    left: Rect,
    cy: f32,
    is_sel: bool,
    ri: usize,
    indent: f32,
}

/// The responses of a layer bar's body and its two trim edges.
struct BarResps {
    body: egui::Response,
    lin: egui::Response,
    lout: egui::Response,
}

/// An expression row's property, expression and editor ids.
#[derive(Clone, Copy)]
struct ExprRow<'a> {
    prop: &'a Property,
    ex: &'a effectcraft_engine::project::Expression,
    buf_id: egui::Id,
    edit_id: egui::Id,
}

/// A keyframe marker's hit result.
struct KeyHit<'a> {
    k: &'a effectcraft_engine::keyframe::Keyframe,
    ct: Tick,
    ks: bool,
    kresp: egui::Response,
    mods: egui::Modifiers,
    this_key: serde_json::Value,
}

/// Computes this frame's layout: zoom, scroll and the panel's regions.
fn layout(app: &mut EffectcraftApp, ui: &egui::Ui, rect: Rect, cid: ItemId, comp: &Arc<Comp>) -> Tl {
    let t = app.tokens;
    let ctx = ui.ctx().clone();
    let p = ui.painter().clone();
    let time = app.session.time();
    let fr = comp.frame_rate;
    // Geometry.
    let header_h = 48.0;
    let ruler_h = 34.0;
    let colhdr_h = 22.0;
    let footer_h = 24.0;
    let vis = Vis::of(&app.ui.timeline);
    let left_w = (vis.fixed() + 190.0).clamp(420.0, (rect.width() * 0.72).max(420.0));
    // Columns wider than the pane scroll horizontally (Shift+wheel / trackpad over the outline,
    // or the scroll bar above the footer); the name column keeps its minimum width.
    let natural_w = vis.fixed() + 190.0;
    let overflow = (natural_w - left_w).max(0.0);
    app.ui.timeline.outline_scroll = app.ui.timeline.outline_scroll.clamp(0.0, overflow);
    let oscroll = app.ui.timeline.outline_scroll;
    let cw = cols(rect.min.x - oscroll, left_w + overflow, vis);
    let graph_x0 = rect.min.x + left_w + 1.0;
    let graph_x1 = rect.max.x - GUTTER_W;
    ctx.data_mut(|d| d.insert_temp(egui::Id::unique("timeline-graph-area"), (graph_x0, graph_x1 - graph_x0)));
    let fit_pps = fit_pps(graph_x1 - graph_x0, comp);
    let pps = app.ui.timeline.pps.unwrap_or(fit_pps);
    if app.ui.timeline.pps.is_none() {
        app.ui.timeline.start = 0.0;
    } else if app.playback.playing {
        // During playback the view pages to keep the CTI in sight (After Effects).
        let span = ((graph_x1 - graph_x0 - 6.0) as f64 / pps).max(0.0);
        app.ui.timeline.start = page_to_cti(app.ui.timeline.start, span, comp.duration.seconds(), app.session.time().seconds());
    }
    let scroll_to = app.ui.timeline.scroll_to.take();
    if scroll_to == Some(crate::state::ScrollTo::Time) && app.ui.timeline.pps.is_some() {
        let span = ((graph_x1 - graph_x0 - 6.0) as f64 / pps).max(0.0);
        app.ui.timeline.start = centered_on(span, comp.duration.seconds(), app.session.time().seconds());
    }
    // As in After Effects, the ruler never scrolls past the comp's end: zoomed in, the last
    // visible time is at most the comp duration.
    let visible_secs = ((graph_x1 - graph_x0 - 6.0).max(1.0) as f64 / pps.max(1e-6)).max(0.0);
    let max_start = (comp.duration.seconds() - visible_secs).max(0.0);
    app.ui.timeline.start = app.ui.timeline.start.clamp(0.0, max_start);
    let tm = TMap { x0: graph_x0 + 6.0, start: app.ui.timeline.start, pps };
    ctx.data_mut(|d| d.insert_temp(tl_map_id(), (tm.x0, tm.start, tm.pps)));
    let top = rect.min.y;
    let rows_top = top + header_h + colhdr_h;
    let rows_rect = Rect::from_min_max(pos2(rect.min.x, rows_top), pos2(rect.max.x, rect.max.y - footer_h));
    p.rect_filled(Rect::from_min_max(pos2(graph_x0, top), rect.max), 0.0, t.tl_bg);
    p.line_segment([pos2(graph_x0 - 1.0, top), pos2(graph_x0 - 1.0, rect.max.y)], Stroke::new(1.0, t.app_bg));

    let ruler = Rect::from_min_max(pos2(graph_x0, top + header_h - ruler_h), pos2(rect.max.x, top + header_h));
    // The composition marker bin, at the right end of the ruler above the layers' scroll bar.
    let bin = Rect::from_min_max(pos2(graph_x1 + 2.0, ruler.max.y - 16.0), pos2(rect.max.x - 2.0, ruler.max.y - 2.0));
    let tc_rect = Rect::from_min_size(pos2(rect.min.x + 12.0, top + 6.0), vec2(150.0, 24.0));
    let wa_row = Rect::from_min_max(pos2(graph_x0, top + header_h), pos2(rect.max.x, top + header_h + colhdr_h));
    let fd = comp.frame_duration().seconds();
    Tl {
        t,
        cid,
        time,
        fr,
        rect,
        header_h,
        colhdr_h,
        footer_h,
        vis,
        left_w,
        overflow,
        oscroll,
        cw,
        graph_x0,
        graph_x1,
        fit_pps,
        pps,
        scroll_to,
        tm,
        top,
        rows_rect,
        ruler,
        bin,
        tc_rect,
        wa_row,
        fd,
        rh: t.row_h,
        max_scroll: 0.0,
        ctx,
        comp: comp.clone(),
        p,
        rows: Arc::new(Vec::new()),
        actions: Vec::new(),
    }
}

/// Without an active composition: a hint, and dropping footage creates the first composition.
fn empty_timeline(app: &mut EffectcraftApp, ui: &mut egui::Ui, rect: Rect) {
    let t = app.tokens;
    let ctx = ui.ctx().clone();
    ui.painter().text(rect.center(), Align2::CENTER_CENTER, "(no composition)", Tokens::ui(12.0), t.text_faint);
    // Dropping footage into an empty Timeline should create its first composition,
    // just like dropping onto the Project panel's New Composition button (#649).
    if let Some(crate::panels::DragPayload::Item(dragged)) = egui::DragAndDrop::payload::<crate::panels::DragPayload>(&ctx).as_deref()
        && ui.rect_contains_pointer(rect)
    {
        let items: Vec<ItemId> = crate::panels::project_drop_items(app, *dragged)
            .into_iter()
            .map(ItemId)
            .filter(|id| {
                app.session.project.item(*id).is_some_and(|item| match &item.kind {
                    effectcraft_engine::project::ItemKind::Footage(footage) => footage.kind != effectcraft_engine::project::FootageKind::Data,
                    effectcraft_engine::project::ItemKind::Solid(_) | effectcraft_engine::project::ItemKind::Comp(_) => true,
                    _ => false,
                })
            })
            .collect();
        if !items.is_empty() {
            ui.painter().rect_stroke(rect.shrink(2.0), 0.0, Stroke::new(2.0, t.accent), StrokeKind::Inside);
            app.auto.add("timeline.newCompDrop", rect, "Drop footage to create a composition");
            if ctx.input(|i| i.pointer.any_released()) {
                app.session.state.project_selection = items;
                if let Err(e) = crate::menus::invoke(app, &ctx, "file.newCompFromSelection", json!({})) {
                    app.ui.status = e;
                }
                egui::DragAndDrop::clear_payload(&ctx);
            }
        }
    }
}

/// Starts the rows pass: the clipped painters and the background responses.
fn begin_rows(app: &mut EffectcraftApp, ui: &mut egui::Ui, tl: Tl) -> Rw {
    let rows_rect = tl.rows_rect;
    let graph_x0 = tl.graph_x0;
    let rect = tl.rect;
    let lp = tl.p.with_clip_rect(rows_rect.intersect(Rect::from_min_max(rows_rect.min, pos2(graph_x0 - 1.0, rows_rect.max.y))));
    let gp = tl.p.with_clip_rect(rows_rect.intersect(Rect::from_min_max(pos2(graph_x0, rows_rect.min.y), rows_rect.max)));
    let graph_on = app.ui.timeline.graph_editor;
    super::graph::track_open(ui.ctx(), graph_on);
    let full_clip = ui.clip_rect();
    let left_clip = full_clip.intersect(Rect::from_min_max(pos2(rect.min.x, rows_rect.min.y), pos2(graph_x0 - 1.0, rows_rect.max.y)));
    let right_clip = full_clip.intersect(Rect::from_min_max(pos2(graph_x0, rows_rect.min.y), rows_rect.max));
    // Registered before the rows so keys, bars and editors on top of it get the pointer first.
    let empty_rect = if graph_on { Rect::NOTHING } else { Rect::from_min_max(pos2(graph_x0, rows_rect.min.y), rows_rect.max) };
    let empty = ui.interact(empty_rect, egui::Id::unique("tl-graph-bg"), Sense::click_and_drag());
    let outline_rect = Rect::from_min_max(rows_rect.min, pos2(graph_x0 - 1.0, rows_rect.max.y));
    let outline_empty = ui.interact(outline_rect, egui::Id::unique("tl-layer-bg"), Sense::click_and_drag());
    app.auto.add("timeline.layerMarquee", outline_rect, "Drag empty outline to select layers");
    Rw {
        lp,
        gp,
        graph_on,
        full_clip,
        left_clip,
        right_clip,
        empty_rect,
        outline_rect,
        empty,
        outline_empty,
        selected: app.session.state.selected_layers.clone(),
        sel_keys: app.session.state.selected_keys.clone(),
        ui_actions: Vec::new(),
        hit_rows: Vec::new(),
        dim_hits: Vec::new(),
        tl,
    }
}

/// The Timeline panel for the active composition inside `rect`: draws the header, ruler, work
/// area and cache bars, the layer rows with their switch and value cells, the time graph and its
/// keys, and the footer; and turns this frame's clicks, drags and drops into commands. Without an
/// active comp it shows a hint, and dropping footage on it creates a composition. Most edits
/// use [`crate::menus::invoke`]; time, switch and work-area gestures call `Session::execute`
/// directly and ignore errors. Reported failures go to `app.ui.status`. UI state (scroll, zoom,
/// open rows and the drags in progress) lives in
/// `app.ui.timeline` and egui's temp data.
pub fn show(app: &mut EffectcraftApp, ui: &mut egui::Ui, rect: Rect) {
    let Some(cid) = app.session.active_comp_id() else {
        empty_timeline(app, ui, rect);
        return;
    };
    let Some(comp) = app.session.project.comp_arc(cid) else { return };
    let mut tl = layout(app, ui, rect, cid, &comp);
    let g = &mut tl;
    if !ui.input(|i| i.pointer.primary_down()) {
        g.ctx.data_mut(|d| d.remove::<SwitchDrag>(switch_drag_id()));
    }
    header_timecode(app, ui, g);
    header_search_and_switches(app, ui, g);
    time_navigator(app, ui, g);
    ruler_background(app, g);
    work_area_bar(app, ui, g);
    cache_bars(app, g);
    ruler_ticks(app, g);
    ruler_scrub(app, ui, g);
    column_header_icons(app, g);
    column_header_interact(app, ui, g);
    let rows = Arc::new(with_expr_rows(build_rows(app, &comp), &comp, &app.ui.timeline, &g.ctx));
    let total_h: f32 = rows.iter().map(|r| row_height(r, g.rh)).sum();
    g.max_scroll = (total_h - g.rows_rect.height() + g.rh).max(0.0);
    g.rows = rows;
    reveal_scroll(app, g);
    wheel_zoom_scroll(app, ui, g);
    pick_whip_edge_scroll(app, g);
    let snap_project = app.session.project.clone();
    let snap_expr = app.session.expr.clone();
    let ectx = EvalCtx { project: &snap_project, comp_id: cid, comp: &comp, time: g.time, expr: snap_expr.as_deref(), footage: None };
    let mut rw = begin_rows(app, ui, tl);
    let g = &mut rw;
    draw_rows(app, ui, g, &ectx);
    ui.set_clip_rect(g.full_clip);
    background_menus(app, ui, g);
    outline_marquee(ui, g);
    pick_whip_drag(g);
    alt_scale_and_graph(app, ui, g, &ectx);
    graph_empty_area(ui, g);
    layer_reorder_drag(ui, g);
    quick_offset_follow(g);
    key_drag_follow(app, g);
    key_box_select(app, ui, g);
    current_time_indicator(app, g);
    let footer = footer_bar(app, ui, g);
    footer_zoom(app, ui, g, footer);
    comp_markers_ui(app, ui, g);
    hand_tool_pan(app, ui, g);
    drop_targets(app, ui, g);
    let Rw { tl, ui_actions, .. } = rw;
    apply_ui_acts(app, &tl.comp, ui_actions);
    apply_actions(app, ui, &tl.ctx, tl.actions);
}

/// Applies the UI-only changes the frame collected (twirls, time, ending an undo merge).
fn apply_ui_acts(app: &mut EffectcraftApp, comp: &Comp, ui_actions: Vec<UiAct>) {
    for a in ui_actions {
        match a {
            UiAct::ToggleLayer(id, all) => {
                let tl = &mut app.ui.timeline;
                // Twirled by hand, the layer shows its whole tree again.
                tl.reveal.clear();
                tl.layer_reveal.remove(&id);
                tl.full_groups.remove(&id);
                let open = !tl.open_layers.contains(&id);
                if open {
                    tl.open_layers.insert(id);
                } else {
                    tl.open_layers.remove(&id);
                }
                if all && let Some(l) = comp.layer(LayerId(id)) {
                    let mut uids = vec![];
                    walk_groups(&l.props, &mut uids);
                    for u in uids {
                        if open {
                            tl.open_groups.insert(u);
                        } else {
                            tl.open_groups.remove(&u);
                        }
                    }
                }
            }
            UiAct::ToggleGroup(layer, uid, all) => {
                let tl = &mut app.ui.timeline;
                tl.toggle_group(layer, uid);
                let open = tl.open_groups.contains(&uid);
                if all && let Some(g) = comp.layer(LayerId(layer)).and_then(|l| find_group(&l.props, uid)) {
                    let mut uids = vec![];
                    walk_groups(g, &mut uids);
                    for u in uids {
                        if open {
                            tl.open_groups.insert(u);
                        } else {
                            tl.open_groups.remove(&u);
                        }
                    }
                }
            }
            UiAct::SetTime(tt) => app.session.set_time(tt),
            UiAct::EndMerge => app.session.history.merge_key = None,
            UiAct::ChangeRenderer => {
                if super::dialogs::open_comp_settings(app).is_ok() {
                    app.dialog_state.comp.tab = super::comp_settings::Tab::Renderer;
                }
            }
        }
    }
}

/// Runs the commands the frame collected through [`crate::menus::invoke`].
fn apply_actions(app: &mut EffectcraftApp, ui: &mut egui::Ui, ctx: &egui::Context, actions: Vec<(String, serde_json::Value)>) {
    if ui.input(|i| i.pointer.primary_down()) && actions.iter().any(|(id, _)| id == "prop.set") {
        app.ui.viewer.property_interacting = true;
        ctx.request_repaint();
    }
    for (id, params) in actions {
        if id == "__endMerge" {
            app.session.history.merge_key = None;
            continue;
        }
        if let Err(e) = crate::menus::invoke(app, ctx, &id, params) {
            app.ui.status = e;
        }
    }
}

/// Draws the visible rows, top to bottom, each by its kind.
fn draw_rows(app: &mut EffectcraftApp, ui: &mut egui::Ui, g: &mut Rw, ectx: &EvalCtx<'_>) {
    let comp = g.tl.comp.clone();
    let rows = g.tl.rows.clone();
    let (rect, rows_rect, rh, cw_name) = (g.tl.rect, g.tl.rows_rect, g.tl.rh, g.tl.cw.name);
    let mut y = rows_rect.min.y - app.ui.timeline.scroll_y;
    // Rows stripe by their index among the rows that are not expression editors, so adding or
    // removing an expression leaves every other row's stripe alone; an editor takes the index
    // (and so the stripe) of its property's row, the one just above.
    let mut striped = 0usize;
    for row in rows.iter() {
        let ri = if matches!(row.kind, RowKind::Expr { .. }) {
            striped.saturating_sub(1)
        } else {
            striped += 1;
            striped - 1
        };
        // Outline widgets never spill into the time graph (narrow timelines).
        ui.set_clip_rect(g.left_clip);
        let hh = row_height(row, rh);
        let r = Rect::from_min_size(pos2(rect.min.x, y), vec2(rect.width(), hh));
        y += hh;
        if r.max.y < rows_rect.min.y || r.min.y > rows_rect.max.y {
            continue;
        }
        g.hit_rows.push((r, row.clone()));
        let Some(layer) = comp.layer(row.layer) else { continue };
        let rc = RowCtx {
            row,
            layer,
            r,
            left: Rect::from_min_max(r.min, pos2(g.tl.graph_x0 - 1.0, r.max.y)),
            cy: r.min.y + rh / 2.0,
            is_sel: g.selected.contains(&layer.id),
            ri,
            indent: cw_name + 6.0 + 14.0 * row.depth as f32,
        };
        match &row.kind {
            RowKind::Layer => layer_row(app, ui, g, &rc),
            RowKind::Group { .. } => group_row(app, ui, g, &rc),
            RowKind::Waveform { .. } => waveform_row(app, g, &rc),
            RowKind::Expr { .. } => expr_row(app, ui, g, &rc),
            RowKind::Prop { .. } => prop_row(app, ui, g, &rc, ectx),
        }
    }
}

/// A layer row: switches, label, name, cells and the layer bar.
fn layer_row(app: &mut EffectcraftApp, ui: &mut egui::Ui, g: &mut Rw, rc: &RowCtx<'_>) {
    layer_row_background(g, rc);
    layer_av_switches(app, ui, g, rc);
    layer_label_swatch(app, ui, g, rc);
    layer_number_and_cells(app, ui, g, rc);
    let (row_resp, name_x) = layer_twirl_and_icon(app, ui, g, rc);
    layer_name(app, ui, g, rc, name_x);
    layer_row_actions(app, ui, g, rc, &row_resp);
    layer_switches(app, ui, g, rc);
    if app.ui.timeline.show_modes && rc.layer.source.is_av() {
        layer_mode_cell(app, ui, g, rc);
        layer_matte_cell(app, ui, g, rc);
    }
    if g.tl.vis.parent {
        layer_parent_cell(app, ui, g, rc);
    }
    ui.set_clip_rect(g.right_clip);
    if !g.graph_on {
        layer_bar(app, ui, g, rc);
    }
}

/// The layer's duration bar: source extent, move and trim, markers.
fn layer_bar(app: &mut EffectcraftApp, ui: &mut egui::Ui, g: &mut Rw, rc: &RowCtx<'_>) {
    let (bar, lc) = layer_bar_paint(g, rc);
    layer_source_extent(app, ui, g, rc, bar, lc);
    let bars = layer_bar_hits(app, ui, g, rc, bar);
    let quick = layer_quick_offset_begin(ui, g, rc, &bars.body);
    layer_bar_drag(g, rc, &bars, quick);
    layer_bar_markers(app, ui, g, rc);
}

/// A group row (effect, mask, shape group, layer style).
fn group_row(app: &mut EffectcraftApp, ui: &mut egui::Ui, g: &mut Rw, rc: &RowCtx<'_>) {
    let shown = group_background(app, g, rc);
    group_switches(app, ui, g, rc);
    group_name(app, ui, g, rc, &shown);
    group_popups(app, ui, g, rc);
    group_mask_mode(ui, g, rc);
    group_row_click(app, ui, g, rc);
}

/// An expression row: its controls and the editor.
fn expr_row(app: &mut EffectcraftApp, ui: &mut egui::Ui, g: &mut Rw, rc: &RowCtx<'_>) {
    let RowKind::Expr { uid, .. } = &rc.row.kind else { return };
    let Some(prop) = rc.layer.props.find(*uid) else { return };
    let Some(ex) = prop.expr.as_ref() else { return };
    let st = ExprRow { prop, ex, buf_id: expr_buf_id(*uid), edit_id: egui::Id::unique(("expr-edit", uid)) };
    expr_controls(app, ui, g, rc, &st);
    expr_language_menu(app, ui, g, rc, &st);
    expr_editor_cell(app, ui, g, rc, &st);
}

/// A property row: keyframe navigator, stopwatch, name, value editors and the keyframes.
fn prop_row(app: &mut EffectcraftApp, ui: &mut egui::Ui, g: &mut Rw, rc: &RowCtx<'_>, ectx: &EvalCtx<'_>) {
    let RowKind::Prop { uid } = &rc.row.kind else { return };
    let Some(prop) = rc.layer.props.find(*uid) else { return };
    prop_background(app, g, rc, prop);
    prop_key_navigator(app, ui, g, rc, prop);
    prop_stopwatch(app, ui, g, rc, prop);
    let name_x = prop_name(app, ui, g, rc, prop);
    prop_value_cell(app, ui, g, rc, ectx, prop, name_x);
    prop_expression_hint(app, ui, g, rc, prop);
    if !g.graph_on {
        for k in &prop.keys {
            key_marker(app, ui, g, rc, prop, k);
        }
    }
}

/// One keyframe marker: its look, selection, menu and drag.
fn key_marker(app: &mut EffectcraftApp, ui: &mut egui::Ui, g: &mut Rw, rc: &RowCtx<'_>, prop: &Property, k: &effectcraft_engine::keyframe::Keyframe) {
    let RowKind::Prop { uid } = &rc.row.kind else { return };
    let ct = rc.layer.comp_time(k.time);
    let x = g.tl.tm.x(ct.seconds());
    if x < g.tl.graph_x0 - 8.0 || x > g.tl.rect.max.x + 8.0 {
        return;
    }
    let kref = effectcraft_engine::KeyRef { layer: rc.layer.id, prop: *uid, time: k.time };
    let ks = g.sel_keys.contains(&kref);
    let kr = key_paint(g, rc, k, x, ks);
    let kresp = key_hit(app, ui, g, rc, prop, k, kr);
    let mods = ui.input(|i| i.modifiers);
    let this_key = json!({"keys": [{"layer": rc.layer.id.0, "prop": uid, "time": k.time.seconds()}], "selectProperties": true});
    let kh = KeyHit { k, ct, ks, kresp, mods, this_key };
    key_select(g, &kh);
    key_double_click(g, rc, prop, &kh);
    key_context_menu(app, g, rc, prop, &kh);
    key_drag_begin(app, g, &kh);
}

/// Drop targets: effects and presets on a layer, footage and files into the stack.
fn drop_targets(app: &mut EffectcraftApp, ui: &mut egui::Ui, g: &mut Rw) {
    let ctx = g.tl.ctx.clone();
    let rows_rect = g.tl.rows_rect;
    if let Some(payload) = egui::DragAndDrop::payload::<crate::panels::DragPayload>(&ctx)
        && !matches!(*payload, crate::panels::DragPayload::Property { .. })
        && let Some(ptr) = ctx.input(|i| i.pointer.hover_pos()).filter(|_| ui.rect_contains_pointer(rows_rect))
    {
        let released = ctx.input(|i| i.pointer.any_released());
        // Alt/Option-drag from the Project panel replaces the source of the layer under the
        // pointer (After Effects). The shortcut replaces every selected layer instead.
        let alt_item = ctx.input(|i| i.modifiers.alt) && matches!(payload.as_ref(), crate::panels::DragPayload::Item(_));
        if alt_item {
            drop_replace_source(app, g, &payload, ptr, released);
        } else if matches!(payload.as_ref(), crate::panels::DragPayload::Effect(_) | crate::panels::DragPayload::Preset { .. }) {
            drop_on_layer(app, g, &payload, ptr, released);
        } else {
            drop_insert_layers(app, g, &payload, ptr, released);
        }
        if released {
            egui::DragAndDrop::clear_payload(&ctx);
        }
    }
}

/// The header's time display: click to switch style, drag to scrub, double-click to type a time.
fn header_timecode(app: &mut EffectcraftApp, ui: &mut egui::Ui, g: &Tl) {
    let t = g.t;
    let time = g.time;
    let fr = g.fr;
    let tc_rect = g.tc_rect;
    let ctx = g.ctx.clone();
    let comp = g.comp.clone();
    let p = g.p.clone();
    let tc = crate::panels::timecode(&app.session, &comp, time);
    let tc_resp = ui.interact(tc_rect, egui::Id::unique("tl-timecode"), Sense::click_and_drag());
    p.text(tc_rect.left_center(), Align2::LEFT_CENTER, &tc, Tokens::semibold(16.0), t.timecode);
    // Under it the other style (frames under timecode, timecode under frames) and the rate.
    let settings = &app.session.project.settings;
    let other = if settings.time_display == effectcraft_engine::project::TimeDisplayStyle::Timecode {
        format!("{:05}", fr.frame_at(time) + settings.frame_start)
    } else {
        effectcraft_engine::time::format_timecode_ae(fr.frame_at(time + comp.display_start), fr, false)
    };
    // Audio-only preview (Video off in the Preview panel): the current time stays put, so that
    // line says why and where the sound is instead (#1118).
    if let Some(af) = app.video_preview_off() {
        let at = af.map(|f| format!(" · {}", effectcraft_engine::time::format_timecode_ae(f + fr.frame_at(comp.display_start), fr, false))).unwrap_or_default();
        let msg = format!("{}{at}", crate::i18n::tr("Video preview off: audio only"));
        p.text(pos2(tc_rect.min.x, tc_rect.max.y + 8.0), Align2::LEFT_CENTER, msg, Tokens::ui(10.5), t.cache_green);
    } else {
        p.text(pos2(tc_rect.min.x, tc_rect.max.y + 8.0), Align2::LEFT_CENTER, format!("{other} ({:.2} fps)", fr.as_f64()), Tokens::ui(10.5), t.text_faint);
    }
    app.auto.add("timeline.timecode", tc_rect, &tc);
    super::toggle_time_display(app, &tc_resp);
    if tc_resp.dragged() {
        let d = tc_resp.drag_delta().x as f64;
        let f = fr.frame_at(time) + (d * 0.5).round() as i64;
        let _ = app.session.execute("time.set", json!({"frame": f.max(0)}));
    }
    if tc_resp.double_clicked() {
        ctx.data_mut(|d| d.insert_temp(egui::Id::unique("tl-tc-edit"), tc.clone()));
    }
    if let Some(mut buf) = ctx.data(|d| d.get_temp::<String>(egui::Id::unique("tl-tc-edit"))) {
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(tc_rect));
        let r = child.add(egui::TextEdit::singleline(&mut buf).font(Tokens::semibold(16.0)).desired_width(150.0));
        if r.lost_focus() {
            if !ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                let _ = app.session.execute("time.set", json!({"timecode": buf}));
            }
            ctx.data_mut(|d| d.remove::<String>(egui::Id::unique("tl-tc-edit")));
        } else {
            widgets::keep_focus(&ctx, &r, &buf);
            ctx.data_mut(|d| d.insert_temp(egui::Id::unique("tl-tc-edit"), buf));
        }
    }
}

/// The header's search field and the composition switches.
fn header_search_and_switches(app: &mut EffectcraftApp, ui: &mut egui::Ui, g: &Tl) {
    let t = g.t;
    let rect = g.rect;
    let left_w = g.left_w;
    let top = g.top;
    let tc_rect = g.tc_rect;
    let comp = g.comp.clone();
    let sr = Rect::from_min_size(pos2(tc_rect.max.x + 14.0, top + 12.0), vec2((left_w - 420.0).clamp(120.0, 220.0), 22.0));
    let mut search = app.ui.timeline.search.clone();
    widgets::search_field(ui, sr, &mut search, "Search", &t);
    app.ui.timeline.search = search;
    app.auto.add("timeline.search", sr, "Search");
    let mut bx = rect.min.x + left_w - 10.0;
    for (icon, on, id, tip) in [
        (Icon::Graph, app.ui.timeline.graph_editor, "graph", "Graph Editor"),
        (Icon::MotionBlur, comp.enable_motion_blur, "motionBlur", "Enables Motion Blur for all layers with the Motion Blur switch set"),
        (Icon::FrameBlend, comp.enable_frame_blending, "frameBlending", "Enables Frame Blending for all layers with the Frame Blend switch set"),
        (Icon::Shy, comp.hide_shy, "hideShy", "Hides all layers for which the Shy switch is set"),
        (Icon::Draft3D, comp.draft_3d, "draft3d", "Draft 3D"),
        (Icon::Flowchart, false, "flowchart", "Composition Mini-Flowchart"),
    ] {
        let r = Rect::from_min_size(pos2(bx - 24.0, top + 11.0), vec2(24.0, 24.0));
        let resp = widgets::icon_button(ui, r, icon, on, &t, egui::Id::unique(("tl-sw", id))).on_hover_text(tip);
        app.auto.add(&format!("timeline.switch.{id}"), r, tip);
        if resp.clicked() {
            match id {
                "graph" => app.ui.timeline.graph_editor = !app.ui.timeline.graph_editor,
                "flowchart" => app.show_panel(crate::dock::PanelKind::Flowchart),
                _ => {
                    let _ = app.session.execute("comp.setSwitch", json!({"switch": id}));
                }
            }
        }
        bx -= 28.0;
    }
}

/// The time navigator: the visible span as a bar with end handles.
fn time_navigator(app: &mut EffectcraftApp, ui: &mut egui::Ui, g: &Tl) {
    let t = g.t;
    let graph_x0 = g.graph_x0;
    let graph_x1 = g.graph_x1;
    let fit_pps = g.fit_pps;
    let pps = g.pps;
    let tm = g.tm;
    let top = g.top;
    let ctx = g.ctx.clone();
    let comp = g.comp.clone();
    let p = g.p.clone();
    let nav = Rect::from_min_max(pos2(graph_x0 + 6.0, top + 6.0), pos2(graph_x1, top + 14.0));
    // Time navigator: the visible span as a grey bar with blue end handles (as in After
    // Effects), over a dark track.
    p.rect_filled(nav, 1.0, t.field_bg);
    let vis0 = (tm.start / comp.duration.seconds()) as f32;
    let vis1 = (tm.t(graph_x1) / comp.duration.seconds()) as f32;
    let nav_vis =
        Rect::from_min_max(pos2(nav.min.x + nav.width() * vis0.clamp(0.0, 1.0), nav.min.y), pos2(nav.min.x + nav.width() * vis1.clamp(0.0, 1.0), nav.max.y));
    p.rect_filled(nav_vis, 0.0, t.tl_nav_bar);
    for x in [nav_vis.min.x, nav_vis.max.x] {
        p.rect_filled(Rect::from_min_max(pos2(x - 3.0, nav.min.y - 1.0), pos2(x + 3.0, nav.max.y + 1.0)), 3.0, t.accent);
    }
    // The middle scrolls at the current zoom; each end is a handle that moves that side of the
    // visible span (pulled in: zoomed in; stretched to the whole track: the whole comp).
    let nresp = ui.interact(nav, egui::Id::unique("tl-nav"), Sense::drag());
    app.auto.add("timeline.navigator", nav, "Time navigator");
    if nresp.dragged() {
        let d = nresp.drag_delta().x as f64 / nav.width() as f64 * comp.duration.seconds();
        app.ui.timeline.start = (app.ui.timeline.start + d).clamp(0.0, comp.duration.seconds());
        if app.ui.timeline.pps.is_none() {
            app.ui.timeline.pps = Some(pps);
        }
    }
    let span_px = (graph_x1 - tm.x0) as f64;
    let (t0, t1) = (tm.start, tm.t(graph_x1));
    for (end, x) in [("start", nav_vis.min.x), ("end", nav_vis.max.x)] {
        let hr = Rect::from_center_size(pos2(x, nav.center().y), vec2(10.0, nav.height() + 6.0));
        let hresp = ui.interact(hr, egui::Id::unique(("tl-nav", end)), Sense::drag());
        app.auto.add(&format!("timeline.navigator.{end}"), hr, if end == "start" { "Time navigator start" } else { "Time navigator end" });
        if hresp.hovered() || hresp.dragged() {
            ctx.set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
        }
        if let Some(px) = hresp.interact_pointer_pos().filter(|_| hresp.dragged()) {
            let t = ((px.x - nav.min.x) / nav.width()) as f64 * comp.duration.seconds();
            // The other end stays; the span is at least what the most zoomed-in ruler shows.
            let min_span = span_px / MAX_PPS;
            let (keep_t, keep_px, span) = if end == "start" { (t1, span_px, t1 - t.min(t1 - min_span)) } else { (t0, 0.0, t.max(t0 + min_span) - t0) };
            set_zoom(&mut app.ui.timeline, &comp, span_px / span.max(min_span), fit_pps, keep_t, keep_px);
        }
    }
}

fn ruler_background(app: &mut EffectcraftApp, g: &Tl) {
    let t = g.t;
    let tm = g.tm;
    let ruler = g.ruler;
    let p = g.p.clone();
    p.rect_filled(ruler, 0.0, t.tl_ruler_bg);
    app.auto.add("timeline.ruler", ruler, &format!("{},{}", tm.start, tm.pps));
}

/// The work area bar beside the column headers, with its begin / end handles.
fn work_area_bar(app: &mut EffectcraftApp, ui: &mut egui::Ui, g: &mut Tl) {
    let t = g.t;
    let fr = g.fr;
    let pps = g.pps;
    let tm = g.tm;
    let wa_row = g.wa_row;
    let ctx = g.ctx.clone();
    let comp = g.comp.clone();
    let p = g.p.clone();
    let actions = &mut g.actions;
    // Work area bar: below the ruler, beside the column headers (After Effects' layout), with
    // blue begin / end handles; the cache bar runs under it. Dragging the bar moves the work
    // area, the handles move its ends; Shift snaps (#252).
    let (wa0, wa1) = (comp.work_area.0.seconds(), comp.work_area.1.seconds());
    let wa = Rect::from_min_max(pos2(tm.x(wa0), wa_row.min.y + 3.0), pos2(tm.x(wa1), wa_row.max.y - 6.0));
    p.with_clip_rect(wa_row).rect_filled(wa, 0.0, t.tl_work_area_bar);
    let shift = ui.input(|i| i.modifiers.shift);
    let bar = wa.intersect(wa_row);
    let bresp = ui.interact(bar, egui::Id::unique("wa-bar"), Sense::click_and_drag());
    app.auto.add("timeline.workArea.bar", bar, "Work area");
    // Right-click: the work area's commands (After Effects).
    crate::widgets::context_menu(&bresp, |ui| {
        let mut chosen = None;
        for id in ["comp.trimToWorkArea", "edit.liftWorkArea", "edit.extractWorkArea"] {
            crate::menus::entry_for(app, ui, id, &mut chosen);
        }
        actions.extend(chosen);
    });
    let bar_start = egui::Id::unique("wa-bar-start");
    if bresp.drag_started() {
        ctx.data_mut(|d| d.insert_temp(bar_start, wa0));
    }
    if bresp.dragged()
        && let (Some(a0), Some(d)) = (ctx.data(|d| d.get_temp::<f64>(bar_start)), bresp.total_drag_delta())
    {
        let len = wa1 - wa0;
        let mut a = a0 + d.x as f64 / pps.max(1e-6);
        if shift {
            a += work_area_snap(app, &comp, &[a, a + len], pps);
        }
        let a = fr.snap_nearest(Tick::from_seconds_f64(a.clamp(0.0, (comp.duration.seconds() - len).max(0.0)))).seconds();
        let _ = app.session.execute("comp.workArea", json!({"start": a, "end": a + len, "merge": "wa-drag"}));
    }
    for (hx, set) in [(wa.min.x, "begin"), (wa.max.x, "end")] {
        let hr = Rect::from_center_size(pos2(hx, wa.center().y), vec2(8.0, 14.0));
        let handle = if set == "begin" {
            Rect::from_min_max(pos2(hx - 1.0, wa.min.y), pos2(hx + 4.0, wa.max.y))
        } else {
            Rect::from_min_max(pos2(hx - 4.0, wa.min.y), pos2(hx + 1.0, wa.max.y))
        };
        p.rect_filled(handle, 2.0, t.accent);
        let resp = ui.interact(hr, egui::Id::unique(("wa", set)), Sense::drag());
        app.auto.add(&format!("timeline.workArea.{set}"), hr, set);
        if resp.dragged() || resp.hovered() {
            ctx.set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
        }
        if resp.dragged()
            && let Some(pt) = resp.interact_pointer_pos()
        {
            let mut secs = tm.t(pt.x).max(0.0);
            if shift {
                secs += work_area_snap(app, &comp, &[secs], pps);
            }
            let secs = fr.snap_nearest(Tick::from_seconds_f64(secs)).seconds();
            let key = if set == "begin" { "start" } else { "end" };
            let _ = app.session.execute("comp.workArea", json!({key: secs, "merge": "wa-drag"}));
        }
    }
}

/// The RAM (green) and disk (blue) cache bars under the work area.
fn cache_bars(app: &mut EffectcraftApp, g: &Tl) {
    let t = g.t;
    let cid = g.cid;
    let tm = g.tm;
    let wa_row = g.wa_row;
    let fd = g.fd;
    let comp = g.comp.clone();
    let p = g.p.clone();
    // Cache bar (green: cached frames of what the viewer shows: its resolution, view and options).
    let series = app.shown_series(cid);
    let cached = app.frames.cached_frames(&series);
    let cy0 = wa_row.max.y - 4.0;
    let mut run: Option<(i64, i64)> = None;
    let draw_run = |a: i64, b: i64| {
        let x0 = tm.x(a as f64 * fd);
        let x1 = tm.x((b + 1) as f64 * fd);
        p.rect_filled(Rect::from_min_max(pos2(x0, cy0), pos2(x1, cy0 + 2.5)), 0.0, t.cache_green);
    };
    for f in cached {
        run = match run {
            Some((a, b)) if f == b + 1 => Some((a, f)),
            Some((a, b)) => {
                draw_run(a, b);
                Some((f, f))
            }
            None => Some((f, f)),
        };
    }
    if let Some((a, b)) = run {
        draw_run(a, b);
    }
    // Disk-cached frames not in RAM (blue), like After Effects.
    if app.session.disk_cache.is_some() || app.frames.remote_disk() {
        let frames = comp.frame_rate.frame_at(comp.duration);
        let opts = app.frame_opts(cid, series.scale as f64 / 1000.0);
        let clip = wa_row.intersect(p.clip_rect());
        let range = disk_bar_range(&tm, clip, fd, frames);
        let disk = app.frames.disk_frames(&app.render_source(), &series, range, &opts);
        let blue_painter = p.with_clip_rect(clip);
        let draw_blue = |a: i64, b: i64| {
            let x0 = tm.x(a as f64 * fd);
            let x1 = tm.x((b + 1) as f64 * fd);
            blue_painter.rect_filled(Rect::from_min_max(pos2(x0, cy0), pos2(x1, cy0 + 2.5)), 0.0, t.cache_blue);
        };
        let mut run: Option<(i64, i64)> = None;
        for f in disk {
            run = match run {
                Some((a, b)) if f == b + 1 => Some((a, f)),
                Some((a, b)) => {
                    draw_blue(a, b);
                    Some((f, f))
                }
                None => Some((f, f)),
            };
        }
        if let Some((a, b)) = run {
            draw_blue(a, b);
        }
    }
}

/// The ruler's tick marks and labels.
fn ruler_ticks(app: &mut EffectcraftApp, g: &Tl) {
    let t = g.t;
    let fr = g.fr;
    let graph_x0 = g.graph_x0;
    let graph_x1 = g.graph_x1;
    let pps = g.pps;
    let tm = g.tm;
    let ruler = g.ruler;
    let bin = g.bin;
    let fd = g.fd;
    let p = g.p.clone();
    // Ticks + labels.
    // Label spacing in whole frames (AE: `00:15f`-style seconds:frames labels).
    let fps_i = fr.as_f64().round().max(1.0) as i64;
    let half = (fps_i / 2).max(1);
    let label_frames = [1, 2, 5, 10, half, fps_i, 2 * fps_i, 5 * fps_i, 10 * fps_i, 30 * fps_i, 60 * fps_i]
        .into_iter()
        .find(|f| *f as f64 * fd * pps >= 52.0)
        .unwrap_or(60 * fps_i);
    let secs_per_label = label_frames as f64 * fd;
    let first = (tm.start / secs_per_label).floor() * secs_per_label;
    let mut s = first;
    let pr = p.with_clip_rect(Rect::from_min_max(pos2(graph_x0, ruler.min.y), pos2(bin.min.x, ruler.max.y)));
    while s <= tm.t(graph_x1) + secs_per_label {
        let x = tm.x(s);
        pr.line_segment([pos2(x, ruler.max.y - 10.0), pos2(x, ruler.max.y)], Stroke::new(1.0, t.tl_ruler_tick));
        let fno = (s / fd).round() as i64;
        let label = ruler_label(fno, fps_i, &app.session.project.settings);
        // A label that would run under the marker bin is left out rather than cut to a stray digit.
        let g = pr.layout_no_wrap(label, Tokens::ui(10.0), t.tl_ruler_text);
        if x + 3.0 + g.size().x <= bin.min.x {
            pr.galley(pos2(x + 3.0, ruler.max.y - 16.0 - g.size().y / 2.0), g, t.tl_ruler_text);
        }
        for k in 1..5 {
            let xx = tm.x(s + secs_per_label * k as f64 / 5.0);
            pr.line_segment([pos2(xx, ruler.max.y - 4.0), pos2(xx, ruler.max.y)], Stroke::new(1.0, t.tl_ruler_tick.gamma_multiply(0.6)));
        }
        s += secs_per_label;
    }
}

/// Scrubbing in the ruler: the current time follows the pointer.
fn ruler_scrub(app: &mut EffectcraftApp, ui: &mut egui::Ui, g: &Tl) {
    let graph_x0 = g.graph_x0;
    let graph_x1 = g.graph_x1;
    let pps = g.pps;
    let tm = g.tm;
    let ruler = g.ruler;
    let bin = g.bin;
    let comp = g.comp.clone();
    // Scrub in the ruler: the current time jumps to the pointer as soon as the button goes down
    // (After Effects), then follows it.
    let rresp = ui.interact(
        Rect::from_min_max(pos2(graph_x0, ruler.min.y + 12.0), pos2(bin.min.x, ruler.max.y)),
        egui::Id::unique("tl-ruler"),
        Sense::click_and_drag(),
    );
    if ((rresp.is_pointer_button_down_on() && ui.input(|i| i.pointer.primary_down())) || rresp.clicked())
        && let Some(pt) = rresp.interact_pointer_pos()
    {
        let mut x = pt.x;
        // Zoomed in, a scrub held past either end of the ruler scrolls the timeline that way
        // (faster the further), with the CTI at the edge (After Effects).
        let mut tm = tm;
        if rresp.dragged() && app.ui.timeline.pps.is_some() {
            let speed = edge_scroll_speed_x(tm.x0, graph_x1, x);
            if speed != 0.0 {
                let dt = f64::from(ui.input(|i| i.stable_dt).clamp(0.0, 0.1));
                let span = f64::from(graph_x1 - tm.x0) / pps;
                let cur = app.ui.timeline.start;
                let hi = (comp.duration.seconds() - span).max(0.0).max(cur);
                app.ui.timeline.start = (cur + f64::from(speed) * dt / pps).clamp(0.0, hi);
                tm.start = app.ui.timeline.start;
                ui.ctx().request_repaint();
            }
            x = x.clamp(tm.x0, graph_x1);
        }
        let mut secs = tm.t(x).max(0.0);
        // Shift-drag: snap to keyframes, in/out points, markers and the work area (8 px).
        if ui.input(|i| i.modifiers.shift) {
            let cands = snap_candidates(&comp, &build_rows(app, &comp));
            secs = snap_time(&cands, secs, 8.0 / pps.max(1e-6));
        }
        let tt = Tick::from_seconds_f64(secs);
        app.session.set_time(tt);
        app.stop();
        // Dragged past either edge of the ruler (over the layer outline, say), the view scrolls so
        // the current time indicator stays in sight (After Effects).
        if app.ui.timeline.pps.is_some() {
            let now = app.session.time().seconds();
            let span = tm.t(bin.min.x) - tm.start;
            if now < tm.start {
                app.ui.timeline.start = now.max(0.0);
            } else if now > tm.start + span {
                app.ui.timeline.start = (now - span).max(0.0);
            }
        }
        // Ctrl/Cmd-drag scrubs the audio too (After Effects).
        if rresp.dragged()
            && ui.input(|i| i.modifiers.command)
            && let Some(cid) = app.session.active_comp_id()
        {
            // The app clock: the audio scrub's idle timeout compares against it in the tick.
            let now = app.now();
            let t = app.session.time();
            app.scrub_audio(cid, t, now);
        }
    }
}

/// The column headers' background, icons and titles.
fn column_header_icons(app: &mut EffectcraftApp, g: &Tl) {
    let t = g.t;
    let rect = g.rect;
    let header_h = g.header_h;
    let colhdr_h = g.colhdr_h;
    let vis = g.vis;
    let graph_x0 = g.graph_x0;
    let top = g.top;
    let p = g.p.clone();
    let cw = &g.cw;
    let ch = Rect::from_min_max(pos2(rect.min.x, top + header_h), pos2(rect.max.x, top + header_h + colhdr_h));
    p.rect_filled(Rect::from_min_max(ch.min, pos2(graph_x0 - 1.0, ch.max.y)), 0.0, t.panel_bg);
    p.line_segment([pos2(rect.min.x, ch.max.y), pos2(rect.max.x, ch.max.y)], Stroke::new(1.0, t.separator));
    let hy = ch.center().y;
    // Column headers scroll with the outline and never spill into the time ruler.
    let hp = p.with_clip_rect(Rect::from_min_max(ch.min, pos2(graph_x0 - 1.0, ch.max.y)));
    let hic = |icon: Icon, x: f32| icons::paint(&hp, Rect::from_center_size(pos2(x, hy), vec2(12.0, 12.0)), icon, t.text_dim);
    let htext = |x: f32, s: &str| hp.text(pos2(x, hy), Align2::LEFT_CENTER, crate::i18n::tr(s), Tokens::ui(11.0), t.text_dim);
    if vis.av {
        hic(Icon::Eye, cw.av + 10.0);
        hic(Icon::Speaker, cw.av + 28.0);
        hic(Icon::Solo, cw.av + 46.0);
        hic(Icon::Lock, cw.av + 64.0);
    }
    if vis.keys {
        htext(cw.keys + 6.0, "Keys");
    }
    if vis.label {
        icons::paint(&hp, Rect::from_center_size(pos2(cw.label + 10.0, hy), vec2(10.0, 10.0)), Icon::Keyframe, t.text_dim);
    }
    if vis.num {
        htext(cw.num + 6.0, "#");
    }
    let name_hdr = if app.ui.timeline.source_name { "Source Name" } else { "Layer Name" };
    htext(cw.name + 22.0, name_hdr);
    if vis.comment {
        htext(cw.comment + 6.0, "Comment");
    }
    if vis.switches {
        for (i, icon) in
            [Icon::Shy, Icon::Collapse, Icon::Quality, Icon::Fx, Icon::FrameBlend, Icon::MotionBlur, Icon::Adjustment, Icon::Cube].into_iter().enumerate()
        {
            hic(icon, cw.switches + SW * i as f32 + SW / 2.0);
        }
    }
    if vis.modes {
        htext(cw.mode + 4.0, "Mode");
        htext(cw.mode + 74.0, "T");
        htext(cw.trkmat + 4.0, "Track Matte");
    }
    if vis.parent {
        icons::paint(&hp, Rect::from_center_size(pos2(cw.parent + 10.0, hy), vec2(12.0, 12.0)), Icon::PickWhip, t.text_dim);
        htext(cw.parent + 20.0, "Parent & Link");
    }
    for (on, x, s) in [(vis.in_, cw.in_, "In"), (vis.out, cw.out, "Out"), (vis.duration, cw.duration, "Duration"), (vis.stretch, cw.stretch, "Stretch")] {
        if on {
            htext(x + 6.0, s);
        }
    }
}

/// Column header clicks, automation ids and the right-click column menu.
fn column_header_interact(app: &mut EffectcraftApp, ui: &mut egui::Ui, g: &mut Tl) {
    let rect = g.rect;
    let header_h = g.header_h;
    let colhdr_h = g.colhdr_h;
    let vis = g.vis;
    let graph_x0 = g.graph_x0;
    let top = g.top;
    let ctx = g.ctx.clone();
    let cw = &g.cw;
    let actions = &mut g.actions;
    let ch = Rect::from_min_max(pos2(rect.min.x, top + header_h), pos2(rect.max.x, top + header_h + colhdr_h));
    let hdr_rect = Rect::from_min_max(ch.min, pos2(graph_x0 - 1.0, ch.max.y));
    let hresp = ui.interact(hdr_rect, egui::Id::unique("tl-colhdr"), Sense::click());
    app.auto.add("timeline.header", hdr_rect, "Columns (right-click)");
    for (id, label) in COLUMNS {
        if let Some(x0) = match id {
            "av" => vis.av.then_some((cw.av, AV_W)),
            "keys" => vis.keys.then_some((cw.keys, KEYS_W)),
            "label" => vis.label.then_some((cw.label, LABEL_W)),
            "num" => vis.num.then_some((cw.num, NUM_W)),
            "name" => Some((cw.name, cw.name_end - cw.name)),
            "comment" => vis.comment.then_some((cw.comment, COMMENT_W)),
            "switches" => vis.switches.then_some((cw.switches, SW * 8.0 + 6.0)),
            "modes" => vis.modes.then_some((cw.mode, MODES_W)),
            "parent" => vis.parent.then_some((cw.parent, PARENT_W)),
            "in" => vis.in_.then_some((cw.in_, TIME_W)),
            "out" => vis.out.then_some((cw.out, TIME_W)),
            "duration" => vis.duration.then_some((cw.duration, TIME_W)),
            _ => vis.stretch.then_some((cw.stretch, STRETCH_W)),
        } {
            app.auto.add(&format!("timeline.header.{id}"), Rect::from_min_size(pos2(x0.0, ch.min.y), vec2(x0.1, colhdr_h)), label);
        }
    }
    if hresp.clicked()
        && let Some(pt) = hresp.interact_pointer_pos()
        && column_at(cw, pt.x) == Some("name")
    {
        app.ui.timeline.source_name = !app.ui.timeline.source_name;
    }
    if hresp.secondary_clicked() {
        let under = hresp.interact_pointer_pos().and_then(|pt| column_at(cw, pt.x));
        ctx.data_mut(|d| d.insert_temp(egui::Id::unique("tl-colhdr-under"), under.unwrap_or("")));
    }
    crate::widgets::context_menu(&hresp, |ui| {
        let under: &str = ctx.data(|d| d.get_temp(egui::Id::unique("tl-colhdr-under"))).unwrap_or("");
        if !under.is_empty() && under != "name" && ui.button(crate::i18n::tr("Hide This")).clicked() {
            actions.push(("timeline.column".into(), json!({"column": under, "visible": false})));
            ui.close();
        }
        ui.menu_button(crate::i18n::tr("Columns"), |ui| {
            for (id, label) in COLUMNS {
                let label = if id == "name" { if app.ui.timeline.source_name { "Source Name" } else { "Layer Name" } } else { label };
                let on = column_visible(&app.ui.timeline, id);
                if ui.add_enabled(id != "name", egui::Button::new(crate::i18n::tr(label)).selected(on)).clicked() {
                    actions.push(("timeline.column".into(), json!({"column": id, "visible": !on})));
                    ui.close();
                }
            }
        });
        if ui.button(if app.ui.timeline.source_name { crate::i18n::tr("Show Layer Name") } else { crate::i18n::tr("Show Source Name") }).clicked() {
            actions.push(("timeline.sourceName".into(), json!({})));
            ui.close();
        }
    });
    let _ = cw.end;
}

/// Scrolls an expression editor or the requested layer / property into view.
fn reveal_scroll(app: &mut EffectcraftApp, g: &Tl) {
    let scroll_to = g.scroll_to;
    let rows_rect = g.rows_rect;
    let rh = g.rh;
    let ctx = g.ctx.clone();
    let comp = g.comp.clone();
    let rows = &g.rows;
    // An expression editor asked to take the keyboard comes into view; a request whose
    // expression didn't appear, or that wasn't taken within a few passes, is dropped (it never
    // takes the keyboard later).
    if let Some(f) = ctx.data(|d| d.get_temp::<ExprFocus>(expr_focus_id())) {
        match rows.iter().position(|r| matches!(r.kind, RowKind::Expr { uid, .. } if uid == f.prop)) {
            Some(i) if ctx.cumulative_pass_nr() <= f.pass.saturating_add(8) => {
                let top: f32 = rows.iter().take(i).map(|r| row_height(r, rh)).sum();
                let fits = (top + rows.get(i).map_or(rh, |r| row_height(r, rh)) - rows_rect.height()).max(0.0).min(top);
                app.ui.timeline.scroll_y = app.ui.timeline.scroll_y.min(top).max(fits);
            }
            _ => ctx.data_mut(|d| d.remove::<ExprFocus>(expr_focus_id())),
        }
    }
    if scroll_to == Some(crate::state::ScrollTo::Layer) {
        // The topmost selected layer, in stack order.
        let sel = &app.session.state.selected_layers;
        let top = comp.layers.iter().find(|l| sel.contains(&l.id)).map(|l| l.id);
        if let Some(i) = rows.iter().position(|r| matches!(r.kind, RowKind::Layer) && Some(r.layer) == top) {
            app.ui.timeline.scroll_y = rows.iter().take(i).map(|r| row_height(r, rh)).sum();
        }
    } else if let Some(crate::state::ScrollTo::Property(uid)) = scroll_to
        && let Some(i) = rows.iter().position(|r| matches!(r.kind, RowKind::Prop { uid: p } if p == uid))
    {
        app.ui.timeline.scroll_y = rows.iter().take(i).map(|r| row_height(r, rh)).sum();
    }
}

/// Mouse wheel and pinch over the rows: zoom time or scroll.
fn wheel_zoom_scroll(app: &mut EffectcraftApp, ui: &mut egui::Ui, g: &Tl) {
    let overflow = g.overflow;
    let graph_x0 = g.graph_x0;
    let fit_pps = g.fit_pps;
    let pps = g.pps;
    let tm = g.tm;
    let rows_rect = g.rows_rect;
    let max_scroll = g.max_scroll;
    let comp = g.comp.clone();
    if ui.rect_contains_pointer(rows_rect) {
        let (dy, dx, zoom) = ui.input(|i| (i.smooth_scroll_delta.y, i.smooth_scroll_delta.x, i.modifiers.alt));
        // Ctrl/Cmd+wheel and a trackpad pinch reach egui as a zoom factor rather than a scroll
        // (#428); like Alt+wheel they zoom time.
        let pinch = ui.input(|i| i.zoom_delta()) as f64;
        let at_x = ui.input(|i| i.pointer.hover_pos()).map(|p| p.x).unwrap_or(graph_x0);
        if (pinch - 1.0).abs() > 1e-4 {
            set_zoom(&mut app.ui.timeline, &comp, tm.pps * pinch, fit_pps, tm.t(at_x), (at_x - tm.x0) as f64);
        } else if zoom && dy.abs() > 0.0 {
            // Alt+wheel zooms around the pointer, out until the whole comp shows.
            set_zoom(&mut app.ui.timeline, &comp, tm.pps * (dy as f64 / 200.0).exp(), fit_pps, tm.t(at_x), (at_x - tm.x0) as f64);
        } else {
            let over_outline = ui.input(|i| i.pointer.hover_pos()).is_some_and(|p| p.x < graph_x0);
            let shift = ui.input(|i| i.modifiers.shift);
            if over_outline && overflow > 0.0 && (dx.abs() > 0.0 || (shift && dy.abs() > 0.0)) {
                let d = if dx.abs() > 0.0 { dx } else { dy };
                app.ui.timeline.outline_scroll = (app.ui.timeline.outline_scroll - d).clamp(0.0, overflow);
            } else {
                app.ui.timeline.scroll_y = (app.ui.timeline.scroll_y - dy).clamp(0.0, max_scroll);
            }
            if dx.abs() > 0.0 && app.ui.timeline.pps.is_some() && !over_outline {
                app.ui.timeline.start = (app.ui.timeline.start - dx as f64 / pps).max(0.0);
            }
        }
    }
}

/// A held pick whip near the top or bottom scrolls the rows.
fn pick_whip_edge_scroll(app: &mut EffectcraftApp, g: &Tl) {
    let rect = g.rect;
    let rows_rect = g.rows_rect;
    let rh = g.rh;
    let max_scroll = g.max_scroll;
    let ctx = g.ctx.clone();
    // A pick whip held near the top or bottom of the layers (or past it) scrolls them, as in
    // After Effects.
    if ctx.data(|d| d.get_temp::<PickWhip>(pick_whip_id())).is_some()
        && let Some(ptr) = ctx.input(|i| i.pointer.latest_pos()).filter(|p| rect.x_range().contains(p.x))
    {
        let speed = edge_scroll_speed(rows_rect, ptr.y, rh);
        if speed != 0.0 {
            app.ui.timeline.scroll_y += speed * ctx.input(|i| i.stable_dt).min(0.1);
            ctx.request_repaint();
        }
    }
    app.ui.timeline.scroll_y = app.ui.timeline.scroll_y.clamp(0.0, max_scroll);
}

fn layer_row_background(g: &Rw, rc: &RowCtx<'_>) {
    let t = g.tl.t;
    let rect = g.tl.rect;
    let graph_x0 = g.tl.graph_x0;
    let lp = g.lp.clone();
    let gp = g.gp.clone();
    let r = rc.r;
    let left = rc.left;
    let is_sel = rc.is_sel;
    lp.rect_filled(left, 0.0, if is_sel { t.row_selected } else { t.row_alt });
    gp.rect_filled(Rect::from_min_max(pos2(graph_x0, r.min.y), r.max), 0.0, if is_sel { t.tl_graph_selected } else { t.tl_bg });
    lp.line_segment([pos2(rect.min.x, r.max.y - 0.5), pos2(graph_x0, r.max.y - 0.5)], Stroke::new(1.0, t.app_bg));
    gp.line_segment([pos2(graph_x0, r.max.y - 0.5), pos2(r.max.x, r.max.y - 0.5)], Stroke::new(1.0, t.app_bg));
}

/// The A/V feature switches (video, audio, solo, lock).
fn layer_av_switches(app: &mut EffectcraftApp, ui: &mut egui::Ui, g: &mut Rw, rc: &RowCtx<'_>) {
    let t = g.tl.t;
    let vis = g.tl.vis;
    let comp = g.tl.comp.clone();
    let cw = &g.tl.cw;
    let actions = &mut g.tl.actions;
    let lp = g.lp.clone();
    let selected = &g.selected;
    let layer = rc.layer;
    let r = rc.r;
    let cy = rc.cy;
    let sw = &layer.switches;
    // Every layer has the Video switch (the eye), nulls, cameras and lights included,
    // as in After Effects: it hides a null's outline in the viewer and turns a camera or
    // light off.
    let av_items: [(Icon, bool, &str, bool); 4] = [
        (Icon::Eye, sw.video, "video", switch_applies(layer, "video", &app.session.project)),
        (Icon::Speaker, sw.audio, "audio", switch_applies(layer, "audio", &app.session.project)),
        (Icon::Solo, sw.solo, "solo", switch_applies(layer, "solo", &app.session.project)),
        (Icon::Lock, sw.locked, "lock", switch_applies(layer, "lock", &app.session.project)),
    ];
    for (i, (icon, on, name, applicable)) in av_items.into_iter().enumerate() {
        let br = Rect::from_center_size(pos2(cw.av + 10.0 + 18.0 * i as f32, cy), vec2(15.0, 15.0));
        if !applicable || !vis.av {
            continue;
        }
        lp.rect_filled(br.shrink(0.5), 1.0, t.switch_well);
        layer_switch(
            ui,
            &t,
            &SwitchSpec { row: r, br, icon, on, id: egui::Id::unique(("av", layer.id.0, i)), layer: layer.id.0, name, others: true },
            &comp,
            &app.session.project,
            selected,
            &mut *actions,
        );
        app.auto.add(&format!("timeline.layer.{}.{name}", layer.id.0), br, name);
    }
}

/// The label swatch and its colour menu.
fn layer_label_swatch(app: &mut EffectcraftApp, ui: &mut egui::Ui, g: &mut Rw, rc: &RowCtx<'_>) {
    let t = g.tl.t;
    let vis = g.tl.vis;
    let cw = &g.tl.cw;
    let actions = &mut g.tl.actions;
    let lp = g.lp.clone();
    let selected = &g.selected;
    let layer = rc.layer;
    let cy = rc.cy;
    // Label swatch.
    if vis.label {
        let lr = Rect::from_center_size(pos2(cw.label + 10.0, cy), vec2(12.0, 12.0));
        lp.rect_filled(lr, 2.0, t.label(layer.label));
        let lresp = ui.interact(lr, egui::Id::unique(("label", layer.id.0)), Sense::click());
        app.auto.add(&format!("timeline.layer.{}.label", layer.id.0), lr, layer.label.name());
        // Click (or right-click) the swatch for the label menu (After Effects).
        let pop = egui::Id::unique(("label-pop", layer.id.0));
        if lresp.clicked() || lresp.secondary_clicked() {
            widgets::open_popup(ui, pop);
        }
        if let Some(lab) = widgets::label_popup(ui, pop, lr.left_bottom(), layer.label, |l| app.session.prefs.label_name(l), &t) {
            actions.push(("edit.label".into(), json!({"layers": edited_layers(layer.id, selected), "label": lab.name()})));
        }
    }
}

fn layer_number_and_cells(app: &mut EffectcraftApp, ui: &mut egui::Ui, g: &mut Rw, rc: &RowCtx<'_>) {
    let t = g.tl.t;
    let vis = g.tl.vis;
    let comp = g.tl.comp.clone();
    let cw = &g.tl.cw;
    let actions = &mut g.tl.actions;
    let lp = g.lp.clone();
    let layer = rc.layer;
    let cy = rc.cy;
    if vis.num {
        lp.text(pos2(cw.num + 13.0, cy), Align2::CENTER_CENTER, format!("{}", comp.index_of(layer.id).unwrap_or(0)), Tokens::ui(11.5), t.text_dim);
    }
    layer_cells(app, ui, &lp, cw, &comp, rc, &mut *actions);
}

/// The row's click target, twirl, icon and guide mark; returns the row response and the name's x.
fn layer_twirl_and_icon(app: &mut EffectcraftApp, ui: &mut egui::Ui, g: &mut Rw, rc: &RowCtx<'_>) -> (egui::Response, f32) {
    let t = g.tl.t;
    let vis = g.tl.vis;
    let cw = &g.tl.cw;
    let lp = g.lp.clone();
    let ui_actions = &mut g.ui_actions;
    let layer = rc.layer;
    let r = rc.r;
    let cy = rc.cy;
    // Row click → select; double-click → rename; drag → reorder. Registered before the
    // twirl and the name field so those sit on top and get their clicks.
    let row_x0 = if vis.num { cw.num } else { cw.name + 16.0 };
    let row_resp =
        ui.interact(Rect::from_min_max(pos2(row_x0, r.min.y), pos2(cw.name_end, r.max.y)), egui::Id::unique(("row", layer.id.0)), Sense::click_and_drag());
    // Twirl + icon + name.
    let tw = Rect::from_center_size(pos2(cw.name + 8.0, cy), vec2(14.0, 14.0));
    let open = app.ui.timeline.open_layers.contains(&layer.id.0);
    if widgets::twirl(ui, tw, open, egui::Id::unique(("twirl", layer.id.0)), &t).clicked() {
        ui_actions.push(UiAct::ToggleLayer(layer.id.0, ui.input(|i| i.modifiers.alt || i.modifiers.command)));
    }
    app.auto.add(&format!("timeline.layer.{}.twirl", layer.id.0), tw, "twirl");
    icons::paint(&lp, Rect::from_center_size(pos2(cw.name + 24.0, cy), vec2(13.0, 13.0)), layer_icon(layer, &app.session.project), t.text_dim);
    // A guide layer carries a guide mark before its name (After Effects).
    let name_x = if layer.switches.guide {
        let mark = Rect::from_center_size(pos2(cw.name + 40.0, cy), vec2(11.0, 11.0));
        icons::paint(&lp, mark, Icon::Grid, t.text_dim);
        app.auto.add(&format!("timeline.layer.{}.guide", layer.id.0), mark, "Guide Layer");
        cw.name + 50.0
    } else {
        cw.name + 34.0
    };
    (row_resp, name_x)
}

/// The layer name: the rename field or the (selected) name cell.
fn layer_name(app: &mut EffectcraftApp, ui: &mut egui::Ui, g: &mut Rw, rc: &RowCtx<'_>, name_x: f32) {
    let t = g.tl.t;
    let ctx = g.tl.ctx.clone();
    let cw = &g.tl.cw;
    let actions = &mut g.tl.actions;
    let lp = g.lp.clone();
    let layer = rc.layer;
    let r = rc.r;
    let cy = rc.cy;
    let is_sel = rc.is_sel;
    let name_rect = Rect::from_min_max(pos2(name_x, r.min.y), pos2(cw.name_end - 4.0, r.max.y));
    let rename_id = egui::Id::unique("tl-rename");
    let renaming: Option<(u64, String)> = ctx.data(|d| d.get_temp(rename_id));
    if let Some((rl, mut buf)) = renaming.filter(|(rl, _)| *rl == layer.id.0) {
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(name_rect.shrink2(vec2(0.0, 2.0))));
        let er = child.add(egui::TextEdit::singleline(&mut buf).font(Tokens::ui(12.0)).desired_width(name_rect.width()));
        app.auto.add(&format!("timeline.layer.{rl}.nameEdit"), er.rect, &buf);
        if ctx.data_mut(|d| d.remove_temp::<bool>(rename_focus_id())).unwrap_or(false) {
            // Just started: focus the field with the whole name selected.
            er.request_focus();
            widgets::select_all(&ctx, er.id, &buf);
        }
        if er.lost_focus() {
            if !ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                actions.push(("layer.rename".into(), json!({"layer": rl, "name": buf})));
            }
            ctx.data_mut(|d| d.remove::<(u64, String)>(rename_id));
        } else {
            ctx.data_mut(|d| d.insert_temp(rename_id, (rl, buf)));
        }
    } else {
        // The selected layer's name sits in a light cell with dark text.
        let name_col = if is_sel { Color32::from_gray(0x16) } else { t.text };
        if is_sel {
            let g = lp.layout_no_wrap(display_name(app, layer), Tokens::ui(12.0), name_col);
            let cell = Rect::from_min_max(
                pos2(name_rect.min.x - 3.0, r.min.y + 1.0),
                pos2((name_rect.min.x + g.size().x + 4.0).max(cw.name_end - 4.0).min(cw.name_end - 2.0), r.max.y - 1.0),
            );
            lp.rect_filled(cell, 0.0, SELECTED_NAME_BG);
        }
        let lpn = lp.with_clip_rect(name_rect.intersect(lp.clip_rect()));
        lpn.text(pos2(name_rect.min.x, cy), Align2::LEFT_CENTER, display_name(app, layer), Tokens::ui(12.0), name_col);
    }
}

/// Row click, drag, double-click and context menu.
fn layer_row_actions(app: &mut EffectcraftApp, ui: &mut egui::Ui, g: &mut Rw, rc: &RowCtx<'_>, row_resp: &egui::Response) {
    let cid = g.tl.cid;
    let ctx = g.tl.ctx.clone();
    let comp = g.tl.comp.clone();
    let rows = &g.tl.rows;
    let actions = &mut g.tl.actions;
    let selected = &g.selected;
    let layer = rc.layer;
    let left = rc.left;
    let is_sel = rc.is_sel;
    // Locked layers can't be selected or moved (unlock them with the lock switch).
    let locked = layer.switches.locked;
    if row_resp.drag_started() && !locked {
        // The selected layers move together; an unselected row moves alone.
        let moving: Vec<u64> = if is_sel { comp.layers.iter().filter(|l| selected.contains(&l.id)).map(|l| l.id.0).collect() } else { vec![layer.id.0] };
        ctx.data_mut(|d| d.insert_temp(layer_drag_id(), moving));
    }
    if row_resp.drag_stopped() {
        ctx.data_mut(|d| d.insert_temp(layer_drop_id(), true));
    }
    app.auto.add(&format!("timeline.layer.{}.row", layer.id.0), left, &layer.name);
    if row_resp.clicked() && !locked {
        let m = ui.input(|i| i.modifiers);
        if let Some(layers) = m.shift.then(|| shift_range(&ctx, cid, &comp, rows, selected, layer.id)).flatten() {
            actions.push(("layer.select".into(), json!({"layers": layers})));
        } else {
            actions.push(("layer.select".into(), json!({"layers": [layer.id.0], "toggle": m.command})));
            set_anchor(&ctx, cid, layer.id);
        }
    }
    if row_resp.double_clicked() {
        match double_click_action(layer) {
            Some(a) => actions.push(a),
            None => start_rename(&ctx, layer.id.0, &layer.name),
        }
    }
    layer_context_menu(row_resp, &comp, layer, selected, LayerMenuCaps::of(&app.session.project, layer), &mut app.auto, &mut *actions);
}

/// The eight layer switches.
fn layer_switches(app: &mut EffectcraftApp, ui: &mut egui::Ui, g: &mut Rw, rc: &RowCtx<'_>) {
    let t = g.tl.t;
    let vis = g.tl.vis;
    let comp = g.tl.comp.clone();
    let cw = &g.tl.cw;
    let actions = &mut g.tl.actions;
    let lp = g.lp.clone();
    let selected = &g.selected;
    let layer = rc.layer;
    let r = rc.r;
    let cy = rc.cy;
    let sw = &layer.switches;
    let sws: [(Icon, bool, &str, bool); 8] = [
        (Icon::Shy, sw.shy, "shy", switch_applies(layer, "shy", &app.session.project)),
        (Icon::Collapse, sw.collapse, "collapse", switch_applies(layer, "collapse", &app.session.project)),
        (Icon::Quality, sw.quality == effectcraft_engine::project::Quality::Best, "quality", switch_applies(layer, "quality", &app.session.project)),
        (Icon::Fx, sw.effects, "fx", switch_applies(layer, "fx", &app.session.project)),
        (
            Icon::FrameBlend,
            sw.frame_blend != effectcraft_engine::project::FrameBlend::Off,
            "frameBlend",
            switch_applies(layer, "frameBlend", &app.session.project),
        ),
        (Icon::MotionBlur, sw.motion_blur, "motionBlur", switch_applies(layer, "motionBlur", &app.session.project)),
        (Icon::Adjustment, sw.adjustment, "adjustment", switch_applies(layer, "adjustment", &app.session.project)),
        (Icon::Cube, layer.is_3d(), "threeD", switch_applies(layer, "threeD", &app.session.project)),
    ];
    for (i, (icon, on, name, applicable)) in sws.into_iter().enumerate() {
        if !applicable || !vis.switches {
            continue;
        }
        let br = Rect::from_center_size(pos2(cw.switches + SW * i as f32 + SW / 2.0, cy), vec2(16.0, 16.0));
        lp.rect_filled(br.shrink(1.0), 1.0, t.switch_well);
        layer_switch(
            ui,
            &t,
            &SwitchSpec { row: r, br, icon, on, id: egui::Id::unique(("sw", layer.id.0, i)), layer: layer.id.0, name, others: false },
            &comp,
            &app.session.project,
            selected,
            &mut *actions,
        );
        app.auto.add(&format!("timeline.layer.{}.switch.{name}", layer.id.0), br, name);
    }
}

/// The blend mode dropdown and Preserve Transparency.
fn layer_mode_cell(app: &mut EffectcraftApp, ui: &mut egui::Ui, g: &mut Rw, rc: &RowCtx<'_>) {
    let t = g.tl.t;
    let comp = g.tl.comp.clone();
    let cw = &g.tl.cw;
    let actions = &mut g.tl.actions;
    let selected = &g.selected;
    let layer = rc.layer;
    let cy = rc.cy;
    let mr = Rect::from_min_size(pos2(cw.mode + 2.0, cy - 9.0), vec2(80.0, 18.0));
    let mresp = widgets::dropdown(ui, mr, layer.blend_mode.label(), &t, egui::Id::unique(("mode", layer.id.0)));
    app.auto.add(&format!("timeline.layer.{}.mode", layer.id.0), mr, "Mode");
    let pop = egui::Id::unique(("mode-pop", layer.id.0));
    if mresp.clicked() {
        widgets::open_popup(ui, pop);
    }
    let opts: Vec<String> =
        BlendMode::ALL.iter().flat_map(|m| if m.ends_group() { vec![m.label().to_string(), "-".to_string()] } else { vec![m.label().to_string()] }).collect();
    if let Some(i) = widgets::popup_menu(ui, pop, mr.left_bottom(), &opts, opts.iter().position(|o| o == layer.blend_mode.label())) {
        actions.push(("layer.setBlendMode".into(), json!({"layers": mode_targets(&comp, layer.id, selected), "mode": opts[i]})));
    }
    let tr = Rect::from_center_size(pos2(cw.mode + 76.0 + 6.0, cy), vec2(14.0, 14.0));
    app.auto.add(&format!("timeline.layer.{}.preserveTransparency", layer.id.0), tr, "Preserve Transparency");
    if widgets::checkbox(ui, tr, layer.preserve_transparency, &t, egui::Id::unique(("pt", layer.id.0))).clicked() {
        // The click gives the selection the state the clicked row toggles to,
        // whatever the first selected layer's value (as the switches do).
        actions.push((
            "layer.setSwitch".into(),
            json!({
                "layers": mode_targets(&comp, layer.id, selected),
                "switch": "preserveTransparency",
                "value": !layer.preserve_transparency
            }),
        ));
    }
}

/// The track matte picker.
fn layer_matte_cell(app: &mut EffectcraftApp, ui: &mut egui::Ui, g: &mut Rw, rc: &RowCtx<'_>) {
    let t = g.tl.t;
    let comp = g.tl.comp.clone();
    let cw = &g.tl.cw;
    let actions = &mut g.tl.actions;
    let layer = rc.layer;
    let cy = rc.cy;
    // Track matte: layer picker + kind.
    let mr = Rect::from_min_size(pos2(cw.trkmat + 2.0, cy - 9.0), vec2(104.0, 18.0));
    let label = match layer.track_matte {
        Some(m) => comp.layer(m.layer).map(|l| format!("{} {}", comp.index_of(l.id).unwrap_or(0), l.name)).unwrap_or("None".into()),
        None => "No Track Matte".into(),
    };
    // The cell is too narrow for the mode as well: it is in the tooltip and ticked in the menu.
    let tip = layer.track_matte.map(|m| format!("{label} · {}", crate::i18n::tr(m.kind.label())));
    let tresp = widgets::dropdown_tip(ui, mr, &label, tip.as_deref(), &t, egui::Id::unique(("trkmat", layer.id.0)));
    app.auto.add(&format!("timeline.layer.{}.trackMatte", layer.id.0), mr, "Track Matte");
    let pop = egui::Id::unique(("trk-pop", layer.id.0));
    if tresp.clicked() {
        widgets::open_popup(ui, pop);
    }
    // The menu lists every layer: built only while it is open (per visible row
    // it made a big comp's timeline quadratic).
    let mut opts = vec!["No Track Matte".to_string(), "-".to_string()];
    let mut candidates: Vec<&Layer> = vec![];
    if widgets::popup_is_open(ui, pop) {
        for (i, l) in comp.layers.iter().enumerate().filter(|(_, l)| l.id != layer.id && l.source.is_av()) {
            opts.push(format!("{}. {}", i + 1, l.name));
            candidates.push(l);
        }
        opts.push("-".into());
        for k in MatteKind::ALL {
            opts.push(k.label().into());
        }
    }
    let current = layer.track_matte.and_then(|m| opts.iter().position(|o| o == m.kind.label()));
    if let Some(i) = widgets::popup_menu(ui, pop, mr.left_bottom(), &opts, current) {
        let ids: Vec<LayerId> = candidates.iter().map(|l| l.id).collect();
        if let Some(a) = opts.get(i).and_then(|o| track_matte_pick(layer, &ids, i, o)) {
            actions.push(a);
        }
    }
    if layer.track_matte.is_some() {
        let kr = Rect::from_center_size(pos2(mr.max.x - 26.0, cy), vec2(10.0, 10.0));
        let _ = kr;
    }
}

/// The parent pick whip and dropdown.
fn layer_parent_cell(app: &mut EffectcraftApp, ui: &mut egui::Ui, g: &mut Rw, rc: &RowCtx<'_>) {
    let t = g.tl.t;
    let comp = g.tl.comp.clone();
    let cw = &g.tl.cw;
    let actions = &mut g.tl.actions;
    let lp = g.lp.clone();
    let layer = rc.layer;
    let cy = rc.cy;
    pick_whip(
        app,
        ui,
        &lp,
        Rect::from_center_size(pos2(cw.parent + 9.0, cy), vec2(15.0, 15.0)),
        (WHIP_PARENT, layer.id.0, 0),
        WhipText {
            auto: &format!("timeline.layer.{}.pickWhip", layer.id.0),
            label: "Parent pick whip",
            hint: crate::i18n::tr("Parent pick whip: drag onto a layer"),
        },
    );
    let pr_rect = Rect::from_min_size(pos2(cw.parent + 20.0, cy - 9.0), vec2(94.0, 18.0));
    let plabel = layer.parent.and_then(|p| comp.layer(p)).map(|l| format!("{}. {}", comp.index_of(l.id).unwrap_or(0), l.name)).unwrap_or_else(|| "None".into());
    let presp = widgets::dropdown(ui, pr_rect, &plabel, &t, egui::Id::unique(("parent", layer.id.0)));
    app.auto.add(&format!("timeline.layer.{}.parent", layer.id.0), pr_rect, "Parent");
    let pop = egui::Id::unique(("par-pop", layer.id.0));
    if presp.clicked() {
        widgets::open_popup(ui, pop);
    }
    let mut cands: Vec<&Layer> = vec![];
    let mut popts = vec!["None".to_string(), "-".to_string()];
    if widgets::popup_is_open(ui, pop) {
        for (i, l) in comp.layers.iter().enumerate().filter(|(_, l)| l.id != layer.id) {
            popts.push(format!("{}. {}", i + 1, l.name));
            cands.push(l);
        }
    }
    if let Some(i) = widgets::popup_menu(ui, pop, pr_rect.left_bottom(), &popts, None) {
        let par = if i == 0 { serde_json::Value::Null } else { json!(cands[i - 2].id.0) };
        actions.push(("layer.setParent".into(), json!({"layers": [layer.id.0], "parent": par})));
    }
}

/// Paints the layer bar; returns it and its label colour.
fn layer_bar_paint(g: &Rw, rc: &RowCtx<'_>) -> (Rect, Color32) {
    let t = g.tl.t;
    let tm = g.tl.tm;
    let gp = g.gp.clone();
    let layer = rc.layer;
    let r = rc.r;
    let is_sel = rc.is_sel;
    let x_in = tm.x(layer.in_point.seconds());
    let x_out = tm.x(layer.out_point.seconds());
    let bar = Rect::from_min_max(pos2(x_in, r.min.y + 1.0), pos2(x_out, r.max.y - 1.0));
    let lc = t.label(layer.label);
    let fill = bar_color(lc, is_sel);
    gp.rect_filled(bar, 0.0, fill);
    if is_sel {
        gp.rect_stroke(bar, 0.0, Stroke::new(1.0, Color32::from_white_alpha(90)), StrokeKind::Inside);
    }
    (bar, lc)
}

/// Dragging the source bar outside the in/out span slips the source.
fn slip_source_handles(app: &mut EffectcraftApp, ui: &mut egui::Ui, g: &mut Rw, rc: &RowCtx<'_>, ghost: Rect, bar: Rect) {
    let pps = g.tl.pps;
    let fd = g.tl.fd;
    let ctx = g.tl.ctx.clone();
    let actions = &mut g.tl.actions;
    let ui_actions = &mut g.ui_actions;
    let layer = rc.layer;
    // Dragging the source bar outside the in/out span slips the source.
    for (side, gr) in [("l", Rect::from_min_max(ghost.min, pos2(bar.min.x, ghost.max.y))), ("r", Rect::from_min_max(pos2(bar.max.x, ghost.min.y), ghost.max))] {
        if gr.width() < 2.0 {
            continue;
        }
        let gresp =
            ui.interact(gr, egui::Id::unique(("ghost", side, layer.id.0)), Sense::click_and_drag()).on_hover_text(crate::i18n::tr("Drag to slip the source"));
        // Outside the bar it is empty track area: a plain click deselects.
        if gresp.clicked() && !ui.input(|i| i.modifiers.shift || i.modifiers.command) {
            actions.push(("layer.select".into(), json!({"layers": []})));
        }
        app.auto.add(&format!("timeline.layer.{}.source{}", layer.id.0, side), gr, "Slip");
        if gresp.hovered() || gresp.dragged() {
            ctx.set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
        }
        let acc_id = egui::Id::unique(("slip-acc", layer.id.0));
        if gresp.dragged() {
            let mut acc: f32 = ctx.data(|d| d.get_temp(acc_id).unwrap_or(0.0));
            acc += gresp.drag_delta().x;
            let frames = ((acc as f64 / pps) / fd).trunc() as i64;
            if frames != 0 {
                acc -= (frames as f64 * fd * pps) as f32;
                actions.push(("layer.slip".into(), json!({"layers": [layer.id.0], "frames": frames, "merge": format!("slip-{}", layer.id.0)})));
            }
            ctx.data_mut(|d| d.insert_temp(acc_id, acc));
        }
        if gresp.drag_stopped() {
            ctx.data_mut(|d| d.remove::<f32>(acc_id));
            ui_actions.push(UiAct::EndMerge);
        }
    }
}

/// The source extent behind the bar (striped past its ends).
fn layer_source_extent(app: &mut EffectcraftApp, ui: &mut egui::Ui, g: &mut Rw, rc: &RowCtx<'_>, bar: Rect, lc: Color32) {
    let tm = g.tl.tm;
    let gp = g.gp.clone();
    let layer = rc.layer;
    let r = rc.r;
    if let LayerSource::Footage { .. } | LayerSource::Comp { .. } = layer.source
        && let Some(d) = layer.source.item().and_then(|i| app.session.project.item(i)).and_then(|i| i.duration())
    {
        let xs = tm.x(layer.start_time.seconds());
        let xe = tm.x(layer.comp_time(d).seconds());
        let ghost = Rect::from_min_max(pos2(xs.min(xe), r.min.y + 9.0), pos2(xs.max(xe), r.max.y - 9.0));
        gp.rect_filled(ghost, 1.0, lc.gamma_multiply(0.18));
        // The bar past the source's end (its last frame held) or before its start
        // is striped, so the source's own span shows (#290). Time remapping
        // decides what shows there itself.
        if layer.props.get("timeRemap").is_none() {
            for (side, part) in [
                ("beforeSource", Rect::from_min_max(bar.min, pos2(ghost.min.x, bar.max.y))),
                ("afterSource", Rect::from_min_max(pos2(ghost.max.x, bar.min.y), bar.max)),
            ] {
                if part.width() >= 1.0 {
                    stripes(&gp, part, lc);
                    app.auto.add(&format!("timeline.layer.{}.bar.{side}", layer.id.0), part, "Past the source");
                }
            }
        }
        slip_source_handles(app, ui, g, rc, ghost, bar);
    }
}

/// The bar's move body and trim edges, click selection.
fn layer_bar_hits(app: &mut EffectcraftApp, ui: &mut egui::Ui, g: &mut Rw, rc: &RowCtx<'_>, bar: Rect) -> BarResps {
    let cid = g.tl.cid;
    let ctx = g.tl.ctx.clone();
    let comp = g.tl.comp.clone();
    let rows = &g.tl.rows;
    let actions = &mut g.tl.actions;
    let selected = &g.selected;
    let layer = rc.layer;
    let is_sel = rc.is_sel;
    app.auto.add(&format!("timeline.layer.{}.bar", layer.id.0), bar, &layer.name);
    // Interactions: move body, trim edges.
    let (body_r, in_r, out_r) = trim_zones(bar);
    let body = ui.interact(body_r, egui::Id::unique(("bar", layer.id.0)), Sense::click_and_drag());
    let lin = ui.interact(in_r, egui::Id::unique(("bar-in", layer.id.0)), Sense::drag());
    let lout = ui.interact(out_r, egui::Id::unique(("bar-out", layer.id.0)), Sense::drag());
    app.auto.add(&format!("timeline.layer.{}.bar.in", layer.id.0), in_r, "Trim In");
    app.auto.add(&format!("timeline.layer.{}.bar.out", layer.id.0), out_r, "Trim Out");
    if lin.hovered() || lout.hovered() || lin.dragged() || lout.dragged() {
        trim_cursor(&ctx);
    }
    if body.double_clicked()
        && let Some(a) = double_click_action(layer)
    {
        actions.push(a);
    }
    let shift = ui.input(|i| i.modifiers.shift);
    if let Some(layers) = (body.clicked() && shift).then(|| shift_range(&ctx, cid, &comp, rows, selected, layer.id)).flatten() {
        // Shift-click: the layers from the last one clicked to this one, as in
        // the layer outline.
        actions.push(("layer.select".into(), json!({"layers": layers})));
    } else {
        if (body.clicked() || body.drag_started()) && !is_sel {
            actions.push(("layer.select".into(), json!({"layers": [layer.id.0], "add": shift})));
        }
        if (body.clicked() || body.drag_started()) && !shift {
            set_anchor(&ctx, cid, layer.id);
        }
    }
    layer_context_menu(&body, &comp, layer, selected, LayerMenuCaps::of(&app.session.project, layer), &mut app.auto, &mut *actions);
    BarResps { body, lin, lout }
}

fn layer_quick_offset_begin(ui: &mut egui::Ui, g: &Rw, rc: &RowCtx<'_>, body: &egui::Response) -> bool {
    let ctx = g.tl.ctx.clone();
    let comp = g.tl.comp.clone();
    let selected = &g.selected;
    let layer = rc.layer;
    let is_sel = rc.is_sel;
    let mods = ui.input(|i| i.modifiers);
    if body.drag_started()
        && is_sel
        && mods.command
        && mods.alt
        && let Some(n) = quick_offset_layers(&comp, selected.iter().copied())
        && let Some(p) = ctx.input(|i| i.pointer.press_origin())
    {
        ctx.data_mut(|d| d.insert_temp(quick_bar_id(), QuickBar { layer: layer.id, origin_x: p.x, layers: n, applied: 0 }));
    }
    ctx.data(|d| d.get_temp::<QuickBar>(quick_bar_id())).is_some_and(|q| q.layer == layer.id)
}

/// Moving and trimming a layer bar by whole frames.
fn layer_bar_drag(g: &mut Rw, rc: &RowCtx<'_>, bars: &BarResps, quick: bool) {
    let BarResps { body, lin, lout } = bars;
    let pps = g.tl.pps;
    let fd = g.tl.fd;
    let ctx = g.tl.ctx.clone();
    let actions = &mut g.tl.actions;
    let selected = &g.selected;
    let ui_actions = &mut g.ui_actions;
    let layer = rc.layer;
    let is_sel = rc.is_sel;
    let drag_key = format!("bar-{}", layer.id.0);
    for (resp, kind) in [(&body, "move"), (&lin, "in"), (&lout, "out")] {
        if kind == "move" && quick {
            continue;
        }
        if resp.dragged() {
            let acc_id = egui::Id::unique(("bar-acc", layer.id.0, kind));
            let mut acc: f32 = ctx.data(|d| d.get_temp(acc_id).unwrap_or(0.0));
            acc += resp.drag_delta().x;
            let frames = ((acc as f64 / pps) / fd).trunc() as i64;
            if frames != 0 {
                acc -= (frames as f64 * fd * pps) as f32;
                let d = frames as f64 * fd;
                // Moving or trimming a selected layer acts on the whole
                // selection; each layer trims from its own in/out point.
                let targets = if is_sel { json!(selected.iter().map(|l| l.0).collect::<Vec<_>>()) } else { json!([layer.id.0]) };
                let params = match kind {
                    "move" => json!({"layers": targets, "delta": d, "merge": drag_key}),
                    "in" => json!({"layers": targets, "inDelta": d, "merge": drag_key}),
                    _ => json!({"layers": targets, "outDelta": d, "merge": drag_key}),
                };
                actions.push(("layer.timing".into(), params));
            }
            ctx.data_mut(|d| d.insert_temp(acc_id, acc));
        }
        if resp.drag_stopped() {
            ctx.data_mut(|d| d.remove::<f32>(egui::Id::unique(("bar-acc", layer.id.0, kind))));
            ui_actions.push(UiAct::EndMerge);
        }
    }
}

fn layer_bar_markers(app: &mut EffectcraftApp, ui: &mut egui::Ui, g: &Rw, rc: &RowCtx<'_>) {
    let graph_x0 = g.tl.graph_x0;
    let tm = g.tl.tm;
    let rows_rect = g.tl.rows_rect;
    let comp = g.tl.comp.clone();
    let layer = rc.layer;
    let r = rc.r;
    let gclip = rows_rect.intersect(Rect::from_min_max(pos2(graph_x0, rows_rect.min.y), rows_rect.max));
    super::markers_ui::layer_markers(app, ui, gclip, &comp, layer, tm, r);
}

/// The group row's background and effect label bar; returns the name to draw.
fn group_background(app: &mut EffectcraftApp, g: &Rw, rc: &RowCtx<'_>) -> String {
    let t = g.tl.t;
    let graph_x0 = g.tl.graph_x0;
    let lp = g.lp.clone();
    let gp = g.gp.clone();
    let layer = rc.layer;
    let r = rc.r;
    let left = rc.left;
    let ri = rc.ri;
    let RowKind::Group { uid, name, .. } = &rc.row.kind else { return String::new() };
    let group = layer.props.find_group(*uid);
    let shown = match group.and_then(|g| match &g.kind {
        GroupKind::Effect { effect } => Some(effect.as_str()),
        _ => None,
    }) {
        Some(effect) => crate::i18n::effects::group(effect, name),
        None => name.clone(),
    };
    // Selected groups (an effect, a mask, a shape group…) highlight like selected
    // property rows.
    let sel_group = app.session.state.selected_props.contains(&(layer.id, *uid));
    lp.rect_filled(
        left,
        0.0,
        if sel_group {
            t.row_selected
        } else if ri.is_multiple_of(2) {
            t.row
        } else {
            t.row_alt
        },
    );
    gp.rect_filled(Rect::from_min_max(pos2(graph_x0, r.min.y), r.max), 0.0, t.tl_bg);
    // An effect's colour label (26.5): the same bar the Effect Controls header wears,
    // on the row's left edge.
    if let Some(lab) =
        group.filter(|g| matches!(g.kind, GroupKind::Effect { .. })).and_then(|g| g.label).filter(|l| *l != effectcraft_engine::color::Label::None)
    {
        let [lr, lg, lb] = app.session.prefs.label_rgb(lab);
        let bar = Rect::from_min_size(left.min, vec2(4.0, left.height()));
        lp.rect_filled(bar, 0.0, Color32::from_rgb(lr, lg, lb));
        app.auto.add(&format!("timeline.group.{uid}.label"), bar, &app.session.prefs.label_name(lab));
    }
    shown
}

/// The group's twirl, effect switch and style eye.
fn group_switches(app: &mut EffectcraftApp, ui: &mut egui::Ui, g: &mut Rw, rc: &RowCtx<'_>) {
    let t = g.tl.t;
    let cw = &g.tl.cw;
    let actions = &mut g.tl.actions;
    let lp = g.lp.clone();
    let ui_actions = &mut g.ui_actions;
    let layer = rc.layer;
    let cy = rc.cy;
    let indent = rc.indent;
    let RowKind::Group { uid, name, open, has_children, fx, eye, .. } = &rc.row.kind else { return };
    if *has_children {
        let tw = Rect::from_center_size(pos2(indent, cy), vec2(13.0, 13.0));
        if widgets::twirl(ui, tw, *open, egui::Id::unique(("gtw", uid)), &t).clicked() {
            // Alt/Cmd-click twirls the whole tree under the group (as on a layer).
            ui_actions.push(UiAct::ToggleGroup(layer.id.0, *uid, ui.input(|i| i.modifiers.alt || i.modifiers.command)));
        }
        app.auto.add(&format!("timeline.group.{uid}.twirl"), tw, name);
    }
    if let Some(en) = fx {
        let fr_ = Rect::from_center_size(pos2(cw.switches + SW * 3.0 + SW / 2.0, cy), vec2(16.0, 16.0));
        if widgets::icon_toggle(ui, fr_, Icon::Fx, *en, &t, egui::Id::unique(("gfx", uid)), Sense::click()).clicked() {
            actions.push(("effect.toggle".into(), json!({"layer": layer.id.0, "effect": uid})));
        }
        app.auto.add(&format!("timeline.group.{uid}.fx"), fr_, name);
    }
    if let Some(en) = eye {
        // Layer style eye switch in the A/V column, like AE.
        let er = Rect::from_center_size(pos2(cw.av + 10.0, cy), vec2(15.0, 15.0));
        lp.rect_stroke(er.shrink(1.5), 2.0, Stroke::new(1.0, t.separator), StrokeKind::Inside);
        if widgets::icon_toggle(ui, er, Icon::Eye, *en, &t, egui::Id::unique(("geye", uid)), Sense::click()).clicked() {
            actions.push(("layer.style.toggle".into(), json!({"layer": layer.id.0, "style": uid})));
        }
        app.auto.add(&format!("timeline.group.{uid}.eye"), er, name);
    }
}

/// The group name, or its rename field.
fn group_name(app: &mut EffectcraftApp, ui: &mut egui::Ui, g: &mut Rw, rc: &RowCtx<'_>, shown: &str) {
    let t = g.tl.t;
    let ctx = g.tl.ctx.clone();
    let cw = &g.tl.cw;
    let actions = &mut g.tl.actions;
    let lp = g.lp.clone();
    let layer = rc.layer;
    let r = rc.r;
    let cy = rc.cy;
    let indent = rc.indent;
    let RowKind::Group { uid, name, .. } = &rc.row.kind else { return };
    let group_edit: Option<(u64, u64, String)> = ctx.data(|d| d.get_temp(group_rename_id()));
    if let Some((_, _, mut buf)) = group_edit.filter(|(rl, ru, _)| *rl == layer.id.0 && ru == uid) {
        let field = Rect::from_min_max(pos2(indent + 8.0, r.min.y + 2.0), pos2((cw.switches - 4.0).max(indent + 60.0), r.max.y - 2.0));
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(field));
        let er = child.add(egui::TextEdit::singleline(&mut buf).font(Tokens::ui(12.0)).desired_width(field.width()));
        app.auto.add(&format!("timeline.group.{uid}.nameEdit"), er.rect, &buf);
        if ctx.data_mut(|d| d.remove_temp::<bool>(rename_focus_id())).unwrap_or(false) {
            er.request_focus();
            widgets::select_all(&ctx, er.id, &buf);
        }
        if er.lost_focus() {
            if !ui.input(|i| i.key_pressed(egui::Key::Escape)) && !buf.trim().is_empty() && buf != *name {
                actions.push(("prop.renameGroup".into(), json!({"layer": layer.id.0, "prop": uid, "name": buf})));
            }
            ctx.data_mut(|d| d.remove::<(u64, u64, String)>(group_rename_id()));
        } else {
            ctx.data_mut(|d| d.insert_temp(group_rename_id(), (layer.id.0, *uid, buf)));
        }
    } else {
        lp.text(pos2(indent + 10.0, cy), Align2::LEFT_CENTER, shown, Tokens::ui(12.0), t.text);
    }
}

/// Add / animate popups, Change Renderer and dash buttons.
fn group_popups(app: &mut EffectcraftApp, ui: &mut egui::Ui, g: &mut Rw, rc: &RowCtx<'_>) {
    let t = g.tl.t;
    let ctx = g.tl.ctx.clone();
    let cw = &g.tl.cw;
    let actions = &mut g.tl.actions;
    let lp = g.lp.clone();
    let ui_actions = &mut g.ui_actions;
    let layer = rc.layer;
    let cy = rc.cy;
    let RowKind::Group { uid, .. } = &rc.row.kind else { return };
    text_anim_popups(app, ui, &lp, layer, *uid, pos2(cw.switches, cy), &mut *actions);
    shape_add_popup(app, ui, &lp, layer, *uid, pos2(cw.switches, cy), &mut *actions);
    if *uid == GEOM_BIT | layer.id.0 {
        let lr = lp.text(pos2(cw.switches + 6.0, cy), Align2::LEFT_CENTER, crate::i18n::tr("Change Renderer…"), Tokens::ui(11.5), t.accent);
        let resp = ui
            .interact(lr, egui::Id::unique(("tl-change-renderer", layer.id.0)), Sense::click())
            .on_hover_text(crate::i18n::tr("Composition Settings ▸ 3D Renderer"));
        if resp.hovered() {
            ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        app.auto.add(&format!("timeline.layer.{}.changeRenderer", layer.id.0), lr, "Change Renderer…");
        if resp.clicked() {
            ui_actions.push(UiAct::ChangeRenderer);
        }
    }
    dash_buttons(app, ui, &lp, layer, *uid, pos2(cw.switches, cy), &mut *actions);
}

/// Mask mode and inverted, inline.
fn group_mask_mode(ui: &mut egui::Ui, g: &mut Rw, rc: &RowCtx<'_>) {
    let t = g.tl.t;
    let cw = &g.tl.cw;
    let actions = &mut g.tl.actions;
    let lp = g.lp.clone();
    let layer = rc.layer;
    let cy = rc.cy;
    let RowKind::Group { uid, .. } = &rc.row.kind else { return };
    // Mask mode + inverted inline.
    if let Some(g) = layer.props.find_group(*uid)
        && let GroupKind::Mask { mode, inverted, .. } = g.kind
    {
        let mr = Rect::from_min_size(pos2(cw.switches + 4.0, cy - 9.0), vec2(80.0, 18.0));
        let pop = egui::Id::unique(("maskmode", uid));
        if widgets::dropdown(ui, mr, mode.label(), &t, egui::Id::unique(("mm", uid))).clicked() {
            widgets::open_popup(ui, pop);
        }
        let opts: Vec<String> = effectcraft_engine::project::MaskMode::ALL.iter().map(|m| m.label().to_string()).collect();
        if let Some(i) = widgets::popup_menu(ui, pop, mr.left_bottom(), &opts, None) {
            actions.push(("layer.setMask".into(), json!({"layer": layer.id.0, "mask": uid, "mode": opts[i]})));
        }
        let ir = Rect::from_min_size(pos2(mr.max.x + 8.0, cy - 8.0), vec2(16.0, 16.0));
        if widgets::checkbox(ui, ir, inverted, &t, egui::Id::unique(("minv", uid))).clicked() {
            actions.push(("layer.setMask".into(), json!({"layer": layer.id.0, "mask": uid, "inverted": !inverted})));
        }
        lp.text(pos2(ir.max.x + 4.0, cy), Align2::LEFT_CENTER, crate::i18n::tr("Inverted"), Tokens::ui(11.0), t.text_dim);
    }
}

/// Group row click and double-click.
fn group_row_click(app: &mut EffectcraftApp, ui: &mut egui::Ui, g: &mut Rw, rc: &RowCtx<'_>) {
    let cw = &g.tl.cw;
    let actions = &mut g.tl.actions;
    let ui_actions = &mut g.ui_actions;
    let layer = rc.layer;
    let r = rc.r;
    let indent = rc.indent;
    let RowKind::Group { uid, name, fx, .. } = &rc.row.kind else { return };
    let gr_rect = Rect::from_min_max(pos2(indent + 8.0, r.min.y), pos2(cw.switches, r.max.y));
    let gr = ui.interact(gr_rect, egui::Id::unique(("grow", uid)), Sense::click());
    app.auto.add(&format!("timeline.group.{uid}.name"), gr_rect, name);
    if gr.clicked() && uid & (WAVE_BIT | GEOM_BIT) == 0 {
        actions.push(("prop.select".into(), json!({"layer": layer.id.0, "prop": uid, "selectKeys": false})));
    }
    if gr.double_clicked() {
        if fx.is_some() {
            // An effect's name: open it in Effect Controls (as in After Effects).
            actions.push(("layer.select".into(), json!({"layers": [layer.id.0]})));
            actions.push(("prop.select".into(), json!({"layer": layer.id.0, "prop": uid, "selectKeys": false})));
            actions.push(("window.panel".into(), json!({"panel": "EffectControls"})));
        } else {
            ui_actions.push(UiAct::ToggleGroup(layer.id.0, *uid, false));
        }
    }
}

fn waveform_row(app: &mut EffectcraftApp, g: &Rw, rc: &RowCtx<'_>) {
    let t = g.tl.t;
    let graph_x0 = g.tl.graph_x0;
    let tm = g.tl.tm;
    let ctx = g.tl.ctx.clone();
    let cw = &g.tl.cw;
    let lp = g.lp.clone();
    let gp = g.gp.clone();
    let row = rc.row;
    let layer = rc.layer;
    let r = rc.r;
    let left = rc.left;
    let cy = rc.cy;
    let ri = rc.ri;
    let RowKind::Waveform { item, .. } = &rc.row.kind else { return };
    lp.rect_filled(left, 0.0, if ri.is_multiple_of(2) { t.row } else { t.row_alt });
    let indent = cw.name + 6.0 + 14.0 * row.depth as f32;
    lp.text(pos2(indent + 10.0, cy), Align2::LEFT_CENTER, "Waveform", Tokens::ui(12.0), t.text);
    let wr = Rect::from_min_max(pos2(graph_x0, r.min.y), r.max);
    gp.rect_filled(wr, 0.0, t.tl_bg);
    app.auto.add(&format!("timeline.layer.{}.waveform", layer.id.0), wr, "Waveform");
    match super::waveform::summary(app, &ctx, ItemId(*item)) {
        Some(s) => super::waveform::draw(&gp, wr.shrink2(vec2(0.0, 2.0)), &tm, layer, &s, Color32::from_rgb(0x5f, 0xc8, 0x8a)),
        None => {
            gp.text(
                pos2(tm.x(layer.in_point.seconds()).max(wr.min.x) + 6.0, wr.center().y),
                Align2::LEFT_CENTER,
                "Building waveform…",
                Tokens::ui(11.0),
                t.text_faint,
            );
        }
    }
}

/// The expression row's outline side: enable, pick whip and syntax warning.
fn expr_controls(app: &mut EffectcraftApp, ui: &mut egui::Ui, g: &mut Rw, rc: &RowCtx<'_>, st: &ExprRow<'_>) {
    let ExprRow { prop, ex, buf_id, edit_id, .. } = *st;
    let t = g.tl.t;
    let ctx = g.tl.ctx.clone();
    let cw = &g.tl.cw;
    let actions = &mut g.tl.actions;
    let lp = g.lp.clone();
    let row = rc.row;
    let layer = rc.layer;
    let left = rc.left;
    let cy = rc.cy;
    let ri = rc.ri;
    let RowKind::Expr { uid, .. } = &rc.row.kind else { return };
    let sel_prop = app.session.state.selected_props.contains(&(layer.id, *uid));
    lp.rect_filled(
        left,
        0.0,
        if sel_prop {
            t.row_selected
        } else if ri.is_multiple_of(2) {
            t.row
        } else {
            t.row_alt
        },
    );
    let indent = cw.name + 6.0 + 14.0 * row.depth as f32;
    lp.text(pos2(indent + 12.0, cy), Align2::LEFT_CENTER, crate::i18n::tr("Expression:"), Tokens::ui(11.5), t.text_dim);
    lp.text(pos2(indent + 82.0, cy), Align2::LEFT_CENTER, &prop.name, Tokens::ui(11.5), t.text);
    // "=" enable switch and the property pick whip (AE's expression controls).
    let en_r = Rect::from_center_size(pos2(cw.switches + 10.0, cy), vec2(16.0, 16.0));
    let en = ui.interact(en_r, egui::Id::unique(("expr-en", uid)), Sense::click()).on_hover_text(crate::i18n::tr("Enable Expression"));
    lp.rect_stroke(en_r.shrink(1.0), 2.0, Stroke::new(1.0, t.separator), StrokeKind::Inside);
    let expr_col = t.expr_text;
    lp.text(en_r.center(), Align2::CENTER_CENTER, if ex.enabled { "=" } else { "≠" }, Tokens::semibold(13.0), if ex.enabled { expr_col } else { t.text_dim });
    app.auto.add(&format!("timeline.prop.{uid}.exprEnable"), en_r, "Enable Expression");
    if en.clicked() {
        actions.push(("prop.setExpression".into(), json!({"layer": layer.id.0, "prop": uid, "enabled": !ex.enabled})));
    }
    let pw = pick_whip(
        app,
        ui,
        &lp,
        Rect::from_center_size(pos2(cw.switches + 30.0, cy), vec2(15.0, 15.0)),
        (WHIP_EXPR, layer.id.0, *uid),
        WhipText {
            auto: &format!("timeline.prop.{uid}.pickWhip"),
            label: "Expression pick whip",
            hint: crate::i18n::tr("Expression pick whip: drag onto a property"),
        },
    );
    if pw.is_pointer_button_down_on() && ui.input(|i| i.pointer.primary_pressed()) {
        // Pressed while the expression is being edited (its text is buffered while
        // the editor has the focus): the reference will go in at the cursor,
        // replacing the selection (#284).
        let into = ctx.data(|d| d.get_temp::<String>(buf_id)).map(|buf| {
            let n = buf.chars().count();
            let sel = egui::text_edit::TextEditState::load(&ctx, edit_id)
                .and_then(|st| st.cursor.char_range())
                .map_or([n, n], |r| [r.primary.index.0.min(r.secondary.index.0).min(n), r.primary.index.0.max(r.secondary.index.0).min(n)]);
            (buf, sel)
        });
        ctx.data_mut(|d| {
            if let Some(into) = into {
                d.insert_temp(whip_into_id(*uid), into);
            } else {
                d.remove::<WhipInto>(whip_into_id(*uid));
            }
        });
    }
    // Syntax errors: warning in the outline (the expression is disabled).
    if let Some(check) = app.session.expr_check
        && let Err(e) = check(&ex.text)
    {
        let wr = Rect::from_center_size(pos2(cw.switches + 50.0, cy), vec2(14.0, 14.0));
        lp.text(wr.center(), Align2::CENTER_CENTER, "⚠", Tokens::ui(12.0), Color32::from_rgb(0xf0, 0xa0, 0x30));
        let _ = ui.interact(wr, egui::Id::unique(("expr-err", uid)), Sense::hover()).on_hover_text(e);
    }
}

/// The Expression Language menu inserts at the cursor.
fn expr_language_menu(app: &mut EffectcraftApp, ui: &mut egui::Ui, g: &mut Rw, rc: &RowCtx<'_>, st: &ExprRow<'_>) {
    let ExprRow { ex, buf_id, edit_id, .. } = *st;
    let t = g.tl.t;
    let ctx = g.tl.ctx.clone();
    let cw = &g.tl.cw;
    let actions = &mut g.tl.actions;
    let lp = g.lp.clone();
    let layer = rc.layer;
    let cy = rc.cy;
    let RowKind::Expr { uid, .. } = &rc.row.kind else { return };
    // ⓕ: the Expression Language menu inserts at the cursor.
    let lang_r = Rect::from_center_size(pos2(cw.switches + 70.0, cy), vec2(16.0, 16.0));
    let lang = ui.interact(lang_r, egui::Id::unique(("expr-lang", uid)), Sense::click()).on_hover_text(crate::i18n::tr("Expression Language Menu"));
    lp.circle_stroke(lang_r.center(), 6.5, Stroke::new(1.0, if lang.hovered() { t.text } else { t.text_dim }));
    lp.text(lang_r.center(), Align2::CENTER_CENTER, "f", Tokens::semibold(10.0), if lang.hovered() { t.text } else { t.text_dim });
    app.auto.add(&format!("timeline.prop.{uid}.exprLanguage"), lang_r, "Expression Language Menu");
    let mut picked: Option<&'static str> = None;
    egui::Popup::menu(&lang).style(crate::widgets::menu_style).show(|ui| {
        for (cat, items) in effectcraft_engine::commands::expr_tools::language_menu() {
            ui.menu_button(cat, |ui| {
                widgets::menu_scroll(ui, false, |ui| {
                    for (label, text) in items {
                        if ui.button(label).clicked() {
                            picked = Some(text);
                            ui.close();
                        }
                    }
                })
            });
        }
    });
    if let Some(text) = picked {
        let cur: String = ctx.data(|d| d.get_temp(buf_id)).unwrap_or_else(|| ex.text.clone());
        let at = egui::text_edit::TextEditState::load(&ctx, edit_id)
            .and_then(|st| st.cursor.char_range())
            .map(|r| r.primary.index.0)
            .unwrap_or(cur.chars().count())
            .min(cur.chars().count());
        let byte = cur.char_indices().nth(at).map(|(b, _)| b).unwrap_or(cur.len());
        let mut next = cur.clone();
        next.insert_str(byte, text);
        actions.push(("prop.setExpression".into(), json!({"layer": layer.id.0, "prop": uid, "expression": next})));
        ctx.data_mut(|d| d.remove::<String>(buf_id));
    }
}

/// The expression editor in the time-graph area.
fn expr_editor_cell(app: &mut EffectcraftApp, ui: &mut egui::Ui, g: &mut Rw, rc: &RowCtx<'_>, st: &ExprRow<'_>) {
    let ExprRow { prop, ex, buf_id, edit_id, .. } = *st;
    let t = g.tl.t;
    let rect = g.tl.rect;
    let graph_x0 = g.tl.graph_x0;
    let rh = g.tl.rh;
    let ctx = g.tl.ctx.clone();
    let actions = &mut g.tl.actions;
    let right_clip = g.right_clip;
    let gp = g.gp.clone();
    let layer = rc.layer;
    let r = rc.r;
    let RowKind::Expr { uid, lines, .. } = &rc.row.kind else { return };
    let expr_col = t.expr_text;
    // The editor in the time-graph area.
    ui.set_clip_rect(right_clip);
    gp.rect_filled(Rect::from_min_max(pos2(graph_x0, r.min.y), r.max), 0.0, t.expr_bg);
    let er = Rect::from_min_max(pos2(graph_x0 + 8.0, r.min.y + 3.0), pos2(rect.max.x - 14.0, r.max.y - 3.0));
    let mut buf: String = ctx.data(|d| d.get_temp(buf_id)).unwrap_or_else(|| ex.text.clone());
    if let Some(f) = ctx.data(|d| d.get_temp::<ExprFocus>(expr_focus_id())).filter(|f| f.prop == *uid) {
        // Asked for once (a pick whip put a reference in, or an Alt-click added the
        // expression): editing starts in the expression as it is now.
        ctx.data_mut(|d| d.remove::<ExprFocus>(expr_focus_id()));
        buf = ex.text.clone();
        let n = buf.chars().count();
        let [a, b] = f.select.map(|before_end| egui::text::CCursor::new(n.saturating_sub(before_end)));
        let mut st = egui::text_edit::TextEditState::load(&ctx, edit_id).unwrap_or_default();
        st.cursor.set_char_range(Some(egui::text::CCursorRange::two(a, b)));
        st.store(&ctx, edit_id);
        ctx.memory_mut(|m| m.request_focus(edit_id));
    }
    // Settings ▸ Scripting & Expressions ▸ Expressions Editor.
    let sp = app.session.prefs.scripting.clone();
    let resp = super::expr_editor::editor(
        ui,
        edit_id,
        &mut buf,
        er,
        super::expr_editor::EditorOpts { prefs: &sp, color: if ex.enabled { expr_col } else { t.text_dim }, rows: *lines },
        &t,
    );
    app.auto.add(&format!("timeline.prop.{uid}.expression"), er, &prop.name);
    // Its bottom edge drags to set its height in whole lines, as in After Effects.
    let edge = Rect::from_min_max(pos2(er.min.x, r.max.y - 4.0), pos2(er.max.x, r.max.y));
    let eresp = ui.interact(edge, egui::Id::unique(("expr-edge", uid)), Sense::drag());
    app.auto.add(&format!("timeline.prop.{uid}.expressionHeight"), edge, "Expression field height");
    if eresp.hovered() || eresp.dragged() {
        ctx.set_cursor_icon(egui::CursorIcon::ResizeVertical);
    }
    if let Some(py) = eresp.interact_pointer_pos().filter(|_| eresp.dragged()).map(|p| p.y) {
        let n = ((py - r.min.y) / rh).round().clamp(1.0, EXPR_MAX_LINES as f32);
        app.ui.timeline.expr_lines.insert(*uid, n as usize);
    }
    let commit = resp.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter) && (i.modifiers.command || i.modifiers.ctrl));
    if commit {
        resp.surrender_focus();
    }
    if resp.has_focus() && !commit {
        ctx.data_mut(|d| d.insert_temp(buf_id, buf));
    } else {
        if (resp.lost_focus() || commit) && buf.trim_end() != ex.text {
            actions.push(("prop.setExpression".into(), json!({"layer": layer.id.0, "prop": uid, "expression": buf.trim_end()})));
        }
        ctx.data_mut(|d| d.remove::<String>(buf_id));
    }
}

fn prop_background(app: &mut EffectcraftApp, g: &Rw, rc: &RowCtx<'_>, prop: &Property) {
    let t = g.tl.t;
    let graph_x0 = g.tl.graph_x0;
    let lp = g.lp.clone();
    let gp = g.gp.clone();
    let layer = rc.layer;
    let r = rc.r;
    let left = rc.left;
    let ri = rc.ri;
    let uid = &prop.uid;

    app.auto.add(&format!("timeline.prop.{uid}.row"), r, &prop.name);
    let sel_prop = app.session.state.selected_props.contains(&(layer.id, *uid));
    lp.rect_filled(
        left,
        0.0,
        if sel_prop {
            t.row_selected
        } else if ri.is_multiple_of(2) {
            t.row
        } else {
            t.row_alt
        },
    );
    gp.rect_filled(Rect::from_min_max(pos2(graph_x0, r.min.y), r.max), 0.0, t.tl_bg);
}

/// The keyframe navigator (previous, toggle, next).
fn prop_key_navigator(app: &mut EffectcraftApp, ui: &mut egui::Ui, g: &mut Rw, rc: &RowCtx<'_>, prop: &Property) {
    let t = g.tl.t;
    let time = g.tl.time;
    let fr = g.tl.fr;
    let vis = g.tl.vis;
    let cw = &g.tl.cw;
    let actions = &mut g.tl.actions;
    let lp = g.lp.clone();
    let ui_actions = &mut g.ui_actions;
    let layer = rc.layer;
    let cy = rc.cy;
    let uid = &prop.uid;

    // Keyframe navigator (in the A/V column, like AE).
    // (in the Keys column when it is shown).
    if prop.is_animated() && (vis.keys || vis.av) {
        let lt = layer.layer_time(time);
        let at_key = effectcraft_engine::keyframe::key_at(&prop.keys, lt).is_some();
        let nav_x = if vis.keys { cw.keys + 12.0 } else { cw.av + 18.0 };
        let prev = Rect::from_center_size(pos2(nav_x, cy), vec2(12.0, 14.0));
        let mid = Rect::from_center_size(pos2(nav_x + 16.0, cy), vec2(14.0, 14.0));
        let next = Rect::from_center_size(pos2(nav_x + 32.0, cy), vec2(12.0, 14.0));
        icons::paint(&lp, prev.shrink(2.0), Icon::ChevronLeft, t.text_dim);
        icons::paint(&lp, next.shrink(2.0), Icon::ChevronRight, t.text_dim);
        let kc = if at_key { t.keyframe_selected } else { Color32::TRANSPARENT };
        lp.add(egui::Shape::convex_polygon(
            vec![mid.center() + vec2(0.0, -5.0), mid.center() + vec2(5.0, 0.0), mid.center() + vec2(0.0, 5.0), mid.center() + vec2(-5.0, 0.0)],
            kc,
            Stroke::new(1.0, t.text_dim),
        ));
        if ui.interact(prev, egui::Id::unique(("kprev", uid)), Sense::click()).clicked()
            && let Some(k) = prop.keys.iter().rev().find(|k| layer.comp_time(k.time).0 < time.0 - fr.frame_duration().0 / 2)
        {
            ui_actions.push(UiAct::SetTime(layer.comp_time(k.time)));
        }
        if ui.interact(next, egui::Id::unique(("knext", uid)), Sense::click()).clicked()
            && let Some(k) = prop.keys.iter().find(|k| layer.comp_time(k.time).0 > time.0 + fr.frame_duration().0 / 2)
        {
            ui_actions.push(UiAct::SetTime(layer.comp_time(k.time)));
        }
        if ui.interact(mid, egui::Id::unique(("kmid", uid)), Sense::click()).clicked() {
            actions.push(("prop.toggleKey".into(), json!({"layer": layer.id.0, "prop": uid})));
        }
        app.auto.add(&format!("timeline.prop.{uid}.addKey"), mid, "Add or remove keyframe");
    }
}

/// The stopwatch.
fn prop_stopwatch(app: &mut EffectcraftApp, ui: &mut egui::Ui, g: &mut Rw, rc: &RowCtx<'_>, prop: &Property) {
    let t = g.tl.t;
    let ctx = g.tl.ctx.clone();
    let actions = &mut g.tl.actions;
    let lp = g.lp.clone();
    let layer = rc.layer;
    let r = rc.r;
    let cy = rc.cy;
    let indent = rc.indent;
    let uid = &prop.uid;

    // Stopwatch: Alt+click adds an expression; a press toggles the animation, and
    // dragging on over other stopwatches gives them the same state (#284).
    if !prop.static_only {
        let swr = Rect::from_center_size(pos2(indent, cy), vec2(15.0, 15.0));
        let resp = ui.interact(swr, egui::Id::unique(("stopwatch", uid)), Sense::click_and_drag());
        let col = if prop.is_animated() {
            t.hot_text
        } else if resp.hovered() {
            t.text
        } else {
            t.text_dim
        };
        icons::paint(&lp, swr, Icon::Stopwatch, col);
        app.auto.add(&format!("timeline.prop.{uid}.stopwatch"), swr, &prop.name);
        if ui.input(|i| i.modifiers.alt) {
            if resp.clicked() {
                actions.push(("prop.setExpression".into(), json!({"layer": layer.id.0, "prop": uid})));
                if prop.expr.is_none() {
                    // Adding one: its editor opens, comes into view and takes the
                    // keyboard with the new expression selected (#1072).
                    app.ui.timeline.expr_closed.remove(uid);
                    focus_expression(&ctx, *uid, [usize::MAX, 0]);
                } else if ctx.data(|d| d.get_temp::<ExprFocus>(expr_focus_id())).is_some_and(|f| f.prop == *uid) {
                    // Removing it: nothing is left to take the keyboard.
                    ctx.data_mut(|d| d.remove::<ExprFocus>(expr_focus_id()));
                }
            }
        } else if let Some((value, merge)) = switch_gesture(ui, &resp, r, "stopwatch", prop.is_animated()) {
            actions.push(("prop.toggleAnimation".into(), json!({"layer": layer.id.0, "prop": uid, "value": value, "merge": merge})));
        } else if resp.clicked() && !resp.clicked_by(egui::PointerButton::Primary) {
            // Keyboard or accessibility activation.
            actions.push(("prop.toggleAnimation".into(), json!({"layer": layer.id.0, "prop": uid})));
        }
    }
}

/// The property name; returns its x.
fn prop_name(app: &mut EffectcraftApp, ui: &mut egui::Ui, g: &mut Rw, rc: &RowCtx<'_>, prop: &Property) -> f32 {
    let t = g.tl.t;
    let cid = g.tl.cid;
    let ctx = g.tl.ctx.clone();
    let cw = &g.tl.cw;
    let actions = &mut g.tl.actions;
    let lp = g.lp.clone();
    let layer = rc.layer;
    let r = rc.r;
    let cy = rc.cy;
    let indent = rc.indent;
    let uid = &prop.uid;

    let name_x = indent + 12.0;
    // 3D layers show Rotation as "Z Rotation" next to X/Y Rotation.
    let is_tr_rot = layer.transform().and_then(|t| t.get("rotation")).is_some_and(|r| r.uid == prop.uid);
    let base = if is_tr_rot && prop.name == "Rotation" && layer.is_3d() { "Z Rotation".to_string() } else { prop.name.clone() };
    let pname = if prop.has_expression() { format!("{base}  =") } else { base };
    lp.text(pos2(name_x, cy), Align2::LEFT_CENTER, &pname, Tokens::ui(12.0), if prop.has_expression() { t.expr_text } else { t.text });
    let name_rect = Rect::from_min_max(pos2(name_x, r.min.y), pos2(cw.switches - 2.0, r.max.y));
    let name_resp = ui.interact(name_rect, egui::Id::unique(("pname", uid)), Sense::click_and_drag());
    app.auto.add(&format!("timeline.prop.{uid}.name"), name_rect, &prop.name);
    if name_resp.drag_started() {
        // Drop on the Essential Graphics panel to expose the property.
        egui::DragAndDrop::set_payload(&ctx, crate::panels::DragPayload::Property { layer: layer.id.0, prop: *uid });
    }
    if name_resp.dragged() {
        egui::Tooltip::always_open(ctx.clone(), ui.layer_id(), egui::Id::unique("prop-drag-tip"), egui::PopupAnchor::Pointer).show(|ui| ui.label(&prop.name));
    }
    if name_resp.clicked() {
        actions.push(("prop.select".into(), json!({"layer": layer.id.0, "prop": uid, "add": ui.input(|i| i.modifiers.shift)})));
    }
    crate::widgets::context_menu(&name_resp, |ui| prop_menu(ui, &mut app.auto, cid, layer, prop, &mut *actions));
    name_x
}

/// The value editors and the property pick whip.
fn prop_value_cell(app: &mut EffectcraftApp, ui: &mut egui::Ui, g: &mut Rw, rc: &RowCtx<'_>, ectx: &EvalCtx<'_>, prop: &Property, name_x: f32) {
    let vis = g.tl.vis;
    let cw = &g.tl.cw;
    let actions = &mut g.tl.actions;
    let lp = g.lp.clone();
    let dim_hits = &mut g.dim_hits;
    let layer = rc.layer;
    let cy = rc.cy;
    let uid = &prop.uid;

    let value = ectx.value(layer, prop);
    let vx = (cw.switches + 4.0).max(name_x + 120.0);
    let ve = ValueEdit::new(ui, &lp, layer, prop, pos2(vx, cy));
    let dims = value_editor(app, ui, &ve, &value, &mut *actions);
    if !dims.is_empty() {
        dim_hits.push((*uid, dims));
    }
    // The property pick whip (Parent & Link column, AE CC 2018 and later): dropped on
    // a property it links this one to it, adding the expression if there is none.
    if vis.parent && !prop.static_only {
        pick_whip(
            app,
            ui,
            &lp,
            Rect::from_center_size(pos2(cw.parent + 9.0, cy), vec2(15.0, 15.0)),
            (WHIP_PROP, layer.id.0, *uid),
            WhipText {
                auto: &format!("timeline.prop.{uid}.propertyPickWhip"),
                label: "Property pick whip",
                hint: crate::i18n::tr("Property pick whip: drag onto a property"),
            },
        );
    }
}

/// The one-line expression hint when its editor is closed.
fn prop_expression_hint(app: &mut EffectcraftApp, ui: &mut egui::Ui, g: &Rw, rc: &RowCtx<'_>, prop: &Property) {
    let t = g.tl.t;
    let graph_x0 = g.tl.graph_x0;
    let right_clip = g.right_clip;
    let gp = g.gp.clone();
    let cy = rc.cy;
    let uid = &prop.uid;

    ui.set_clip_rect(right_clip);
    // Expression text row hint.
    if let Some(e) = prop.expr.as_ref().filter(|e| e.enabled && app.ui.timeline.expr_closed.contains(uid)) {
        gp.text(pos2(graph_x0 + 8.0, cy), Align2::LEFT_CENTER, format!("= {}", e.text.lines().next().unwrap_or("")), Tokens::mono(11.0), t.expr_text);
    }
}

/// Key click selection and interpolation shortcuts.
fn key_select(g: &mut Rw, kh: &KeyHit<'_>) {
    let KeyHit { k, ks, mods, ref kresp, ref this_key, .. } = *kh;
    let actions = &mut g.tl.actions;
    if kresp.clicked() && mods.command {
        // Ctrl+click: Linear ↔ Auto Bezier; Ctrl+Alt+click: Hold on / off.
        actions.push(("keys.select".into(), this_key.clone()));
        if mods.alt {
            actions.push(("keys.toggleHold".into(), json!({})));
        } else {
            let linear = k.in_interp == Interp::Linear && k.out_interp == Interp::Linear;
            actions.push(("keys.interpolation".into(), json!({"interpolation": if linear { "autoBezier" } else { "linear" }})));
        }
    } else if kresp.clicked() && mods.shift {
        // Shift+click: in or out of the selection.
        let mut p = this_key.clone();
        p["toggle"] = json!(true);
        actions.push(("keys.select".into(), p));
    } else if (kresp.clicked() || kresp.drag_started()) && !ks {
        let mut p = this_key.clone();
        p["add"] = json!(mods.shift);
        actions.push(("keys.select".into(), p));
    }
}

fn key_double_click(g: &mut Rw, rc: &RowCtx<'_>, prop: &Property, kh: &KeyHit<'_>) {
    let KeyHit { k, ct, ref kresp, .. } = *kh;
    let actions = &mut g.tl.actions;
    let ui_actions = &mut g.ui_actions;
    let layer = rc.layer;
    let uid = &prop.uid;

    if kresp.double_clicked() {
        // Edit the key's value (numeric properties), as After Effects does.
        if matches!(k.value, Value::Scalar(_) | Value::Vec2(_) | Value::Vec3(_)) {
            actions.push(("keys.set".into(), json!({"layer": layer.id.0, "prop": uid, "time": k.time.seconds()})));
        } else {
            ui_actions.push(UiAct::SetTime(ct));
        }
    }
}

/// The keyframe right-click menu.
fn key_context_menu(app: &mut EffectcraftApp, g: &mut Rw, rc: &RowCtx<'_>, prop: &Property, kh: &KeyHit<'_>) {
    let KeyHit { k, ks, ref kresp, .. } = *kh;
    let t = g.tl.t;
    let actions = &mut g.tl.actions;
    let layer = rc.layer;
    let uid = &prop.uid;

    crate::widgets::context_menu(kresp, |ui| {
        for (lbl, cmd, params) in [
            ("Edit Value…", "keys.set", json!({"layer": layer.id.0, "prop": uid, "time": k.time.seconds()})),
            ("-", "", json!({})),
            ("Copy", "keys.copy", json!({})),
            ("Paste", "keys.paste", json!({})),
            ("-", "", json!({})),
            ("Select Equal Keyframes", "keys.selectEqual", json!({})),
            ("Select Previous Keyframes", "keys.selectPrevious", json!({})),
            ("Select Following Keyframes", "keys.selectFollowing", json!({})),
            ("-", "", json!({})),
            ("Keyframe Interpolation…", "keys.interpolation", json!({})),
            ("Keyframe Velocity…", "keys.velocity", json!({})),
            ("Toggle Hold Keyframe", "keys.toggleHold", json!({})),
            ("Rove Across Time", "keys.interpolation", json!({"roving": !k.roving})),
            ("-", "", json!({})),
            ("Easy Ease", "keys.easyEase", json!({})),
            ("Easy Ease In", "keys.easyEaseIn", json!({})),
            ("Easy Ease Out", "keys.easyEaseOut", json!({})),
            ("Time-Reverse Keyframes", "keys.timeReverse", json!({})),
            ("-", "", json!({})),
            ("Linear", "keys.interpolation", json!({"interpolation": "linear"})),
            ("Bezier", "keys.interpolation", json!({"interpolation": "bezier"})),
            ("Auto Bezier", "keys.interpolation", json!({"interpolation": "autoBezier"})),
            ("Select All Keyframes", "keys.selectAll", json!({})),
            ("Delete", "keys.delete", json!({})),
        ] {
            if lbl == "Select All Keyframes" {
                // Keyframe colour labels.
                ui.menu_button(crate::i18n::tr("Label"), |ui| {
                    for (i, l) in effectcraft_engine::color::Label::ALL.into_iter().enumerate() {
                        if widgets::label_entry(ui, l, &app.session.prefs.label_name(l), k.label as usize == i, &t).clicked() {
                            if !ks {
                                actions.push(("keys.select".into(), json!({"keys": [{"layer": layer.id.0, "prop": uid, "time": k.time.seconds()}]})));
                            }
                            actions.push(("keys.setLabel".into(), json!({"label": i})));
                            ui.close();
                        }
                    }
                });
            }
            if lbl == "-" {
                ui.separator();
                continue;
            }
            if lbl == "Rove Across Time" && !prop.spatial {
                continue;
            }
            if lbl == "Edit Value…" && !matches!(k.value, Value::Scalar(_) | Value::Vec2(_) | Value::Vec3(_)) {
                continue;
            }
            if ui.button(lbl).clicked() {
                if !ks {
                    actions.push(("keys.select".into(), json!({"keys": [{"layer": layer.id.0, "prop": uid, "time": k.time.seconds()}]})));
                }
                actions.push((cmd.into(), params));
                ui.close();
            }
        }
    });
}

/// Starts a key drag (or Alt-scale).
fn key_drag_begin(app: &mut EffectcraftApp, g: &Rw, kh: &KeyHit<'_>) {
    let KeyHit { ct, ks, mods, ref kresp, .. } = *kh;
    let ctx = g.tl.ctx.clone();
    let comp = g.tl.comp.clone();
    let sel_keys = &g.sel_keys;
    // Alt-drag the first or last key of a selected group: scale the group in time.
    if kresp.drag_started()
        && mods.alt
        && !mods.command
        && let Some(c) = app.session.active_comp_arc()
    {
        super::graph_tools::alt_scale_begin(app, &ctx, &c, ct.seconds());
    }
    // A plain drag moves the selected keys. The timeline follows it (below): this
    // widget's id changes with the key's time, so its own drag would end after
    // the first frame of movement.
    // (From where the button went down: a drag starts a few pixels later.)
    if kresp.drag_started()
        && !super::graph_tools::alt_scale_active(&ctx)
        && let Some(p) = ctx.input(|i| i.pointer.press_origin())
    {
        // Ctrl/Cmd+Alt: Quick Offset of the selected keys' layers (#1012).
        let quick = (mods.command && mods.alt && ks).then(|| quick_offset_layers(&comp, sel_keys.iter().map(|k| k.layer))).flatten();
        ctx.data_mut(|d| d.insert_temp(key_drag_id(), KeyDrag { origin_x: p.x, anchor: ct.seconds(), applied: 0, quick }));
    }
}

/// Paints one keyframe marker; returns its hit rect.
fn key_paint(g: &Rw, rc: &RowCtx<'_>, k: &effectcraft_engine::keyframe::Keyframe, x: f32, ks: bool) -> Rect {
    let t = g.tl.t;
    let gp = g.gp.clone();
    let cy = rc.cy;
    let icon = k.icon();
    let c = pos2(x, cy);
    // Labelled keys take their label colour (brighter when selected).
    let kcol = match effectcraft_engine::color::Label::ALL.get(k.label as usize).filter(|_| k.label > 0) {
        Some(l) => {
            let c = t.label(*l);
            if ks { c } else { c.gamma_multiply(0.75) }
        }
        None if ks => t.keyframe_selected,
        None => t.keyframe,
    };
    if k.roving {
        // Roving keys: a small dot (their time follows their neighbours).
        gp.circle_filled(c, 2.5, kcol);
    } else {
        icons::keyframe(&gp, c, 11.0, icon.left, icon.right, kcol);
    }
    Rect::from_center_size(c, vec2(12.0, 14.0))
}

/// The key's hit target and automation ids.
fn key_hit(
    app: &mut EffectcraftApp,
    ui: &mut egui::Ui,
    g: &Rw,
    rc: &RowCtx<'_>,
    prop: &Property,
    k: &effectcraft_engine::keyframe::Keyframe,
    kr: Rect,
) -> egui::Response {
    let fr = g.tl.fr;
    let layer = rc.layer;
    let uid = &prop.uid;
    let ct = layer.comp_time(k.time);
    let kresp = ui.interact(kr, egui::Id::unique(("key", uid, k.time.0)), Sense::click_and_drag());
    app.auto.add(&format!("timeline.key.{uid}.{}", fr.frame_at(ct)), kr, &format!("{} key", prop.name));
    // Exact (layer-time ticks): keys a Quick Offset puts on subframes can share a frame.
    app.auto.add(&format!("timeline.keyAt.{uid}.{}", k.time.0), kr, &format!("{} key", prop.name));
    kresp
}

/// Right-click menus on the empty outline and graph areas.
fn background_menus(app: &mut EffectcraftApp, ui: &mut egui::Ui, g: &mut Rw) {
    let actions = &mut g.tl.actions;
    let graph_on = g.graph_on;
    let full_clip = g.full_clip;
    let empty_rect = g.empty_rect;
    let empty = g.empty.clone();
    let outline_empty = g.outline_empty.clone();
    ui.set_clip_rect(full_clip);
    // The background responses sit below rows, bars and editors, preserving their menus.
    if !graph_on {
        app.auto.add("timeline.empty", empty_rect, "Right-click to create a layer");
    }
    for (response, area) in [(&outline_empty, "outline"), (&empty, "graph")] {
        crate::widgets::context_menu(response, |ui| {
            let menu = ui.menu_button(crate::i18n::tr("New"), |ui| {
                for (key, label, command, params) in [
                    ("text", "Text", "layer.newText", json!({})),
                    ("solid", "Solid…", "layer.newSolid", json!({})),
                    ("light", "Light…", "layer.newLight", json!({})),
                    ("camera", "Camera…", "layer.newCamera", json!({})),
                    ("null", "Null Object", "layer.newNull", json!({})),
                    ("shape", "Shape Layer", "layer.newShape", json!({"kind": "none"})),
                    ("adjustment", "Adjustment Layer", "layer.newAdjustment", json!({})),
                ] {
                    let response = ui.button(crate::i18n::context_item(command, &params, label));
                    app.auto.add(&format!("timeline.context.{area}.{key}"), response.rect, label);
                    if response.clicked() {
                        actions.push((command.into(), params));
                        ui.close();
                    }
                }
            });
            app.auto.add(&format!("timeline.context.{area}.new"), menu.response.rect, "New");
            let response = ui.button(crate::i18n::tr("Composition Settings…"));
            app.auto.add(&format!("timeline.context.{area}.settings"), response.rect, "Composition Settings…");
            if response.clicked() {
                actions.push(("comp.settings".into(), json!({})));
                ui.close();
            }
            let response = ui.button(crate::i18n::tr("Open in Essential Graphics"));
            app.auto.add(&format!("timeline.context.{area}.essentialGraphics"), response.rect, "Open in Essential Graphics");
            if response.clicked() {
                actions.push(("comp.openInEssentialGraphics".into(), json!({})));
                ui.close();
            }
        });
    }
}

/// Click and marquee-select on the empty outline.
fn outline_marquee(ui: &mut egui::Ui, g: &mut Rw) {
    let t = g.tl.t;
    let ctx = g.tl.ctx.clone();
    let comp = g.tl.comp.clone();
    let actions = &mut g.tl.actions;
    let outline_rect = g.outline_rect;
    let lp = g.lp.clone();
    let outline_empty = g.outline_empty.clone();
    let hit_rows = &mut g.hit_rows;
    if outline_empty.clicked() && !ui.input(|i| i.modifiers.shift || i.modifiers.command) {
        actions.push(("layer.select".into(), json!({"layers": []})));
    }
    if let (Some(start), Some(end)) = (ctx.input(|i| i.pointer.press_origin()), outline_empty.interact_pointer_pos())
        && outline_empty.dragged()
    {
        let box_rect = Rect::from_two_pos(start, end).intersect(outline_rect);
        lp.rect_filled(box_rect, 0.0, t.accent.gamma_multiply(0.12));
        lp.rect_stroke(box_rect, 0.0, Stroke::new(1.0, t.accent), StrokeKind::Inside);
        ctx.data_mut(|d| d.insert_temp(egui::Id::unique("tl-layer-box"), box_rect));
    }
    if outline_empty.drag_stopped()
        && let Some(box_rect) = ctx.data(|d| d.get_temp::<Rect>(egui::Id::unique("tl-layer-box")))
    {
        ctx.data_mut(|d| d.remove::<Rect>(egui::Id::unique("tl-layer-box")));
        let layers: Vec<u64> = hit_rows
            .iter()
            .filter(|(r, row)| matches!(row.kind, RowKind::Layer) && r.intersects(box_rect))
            .filter(|(_, row)| comp.layer(row.layer).is_some_and(|l| !l.switches.locked))
            .map(|(_, row)| row.layer.0)
            .collect();
        actions.push(("layer.select".into(), json!({"layers": layers, "add": ui.input(|i| i.modifiers.shift || i.modifiers.command)})));
    }
    // Pick whips: a line follows the pointer; releasing over a row links to it (an expression's
}

/// A pick whip in flight: its line, the target and the link on release.
fn pick_whip_drag(g: &mut Rw) {
    let t = g.tl.t;
    let graph_x0 = g.tl.graph_x0;
    let ctx = g.tl.ctx.clone();
    let actions = &mut g.tl.actions;
    let hit_rows = &mut g.hit_rows;
    let dim_hits = &mut g.dim_hits;
    // or a property's, also over a property in Effect Controls).
    if let Some((kind, src_layer, src_prop, start)) = ctx.data(|d| d.get_temp::<PickWhip>(pick_whip_id())) {
        let ptr = ctx.input(|i| i.pointer.latest_pos()).unwrap_or(start);
        let fg = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::unique("tl-pickwhip-line")));
        fg.line_segment([start, ptr], Stroke::new(1.5, t.accent));
        fg.circle_stroke(ptr, 4.0, Stroke::new(1.5, t.accent));
        let target = hit_rows.iter().find(|(r, _)| r.contains(ptr)).map(|(r, row)| (*r, row.clone()));
        // Shown this pass or the last one (Effect Controls may draw after the Timeline).
        let pass = ctx.cumulative_pass_nr();
        let elsewhere = (target.is_none() && kind != WHIP_PARENT)
            .then(|| ctx.data(|d| d.get_temp::<WhipTargets>(whip_targets_id())))
            .flatten()
            .filter(|(p, _)| p.saturating_add(1) >= pass)
            .and_then(|(_, targets)| targets.into_iter().find(|(r, ..)| r.contains(ptr)));
        if let Some((tr, _)) = &target {
            fg.rect_stroke(Rect::from_min_max(tr.min, pos2(graph_x0 - 1.0, tr.max.y)), 0.0, Stroke::new(1.0, t.accent), StrokeKind::Inside);
        } else if let Some((tr, ..)) = &elsewhere {
            fg.rect_stroke(*tr, 0.0, Stroke::new(1.0, t.accent), StrokeKind::Inside);
        }
        if ctx.input(|i| !i.pointer.any_down()) {
            ctx.data_mut(|d| {
                d.remove::<PickWhip>(pick_whip_id());
                d.remove::<WhipTargets>(whip_targets_id());
            });
            let into = (kind == WHIP_EXPR).then(|| ctx.data_mut(|d| d.remove_temp::<WhipInto>(whip_into_id(src_prop)))).flatten();
            // The property dropped on, and the value (dimension) if on one of them: `position[0]`.
            let dim = |fields: &[Rect]| fields.iter().position(|r| r.contains(ptr));
            let prop_target = match (&target, elsewhere) {
                (Some((_, Row { layer, kind: RowKind::Prop { uid }, .. })), _) => {
                    Some((layer.0, *uid, dim_hits.iter().find(|(u, _)| u == uid).and_then(|(_, rs)| dim(rs))))
                }
                (None, Some((_, l, u, fields))) => Some((l, u, dim(&fields))),
                _ => None,
            };
            match (kind, target, prop_target) {
                (WHIP_PARENT, Some((_, row)), _) if row.layer.0 != src_layer => {
                    actions.push(("layer.setParent".into(), json!({"layers": [src_layer], "parent": row.layer.0})));
                }
                // The property pick whip replaces the expression with the reference (or adds one),
                // as the expression pick whip does when its expression isn't being edited.
                (WHIP_EXPR | WHIP_PROP, _, Some((tl, tu, d))) if tu != src_prop => {
                    let mut params = json!({"layer": src_layer, "prop": src_prop, "target": {"layer": tl, "prop": tu}});
                    if let Some(d) = d {
                        params["target"]["dimension"] = json!(d);
                    }
                    if let Some((text, [a, b])) = into {
                        // Into the expression being edited, which goes on being edited.
                        params["expression"] = json!(text);
                        params["range"] = json!([a, b]);
                        let after = text.chars().count().saturating_sub(b);
                        focus_expression(&ctx, src_prop, [after, after]);
                    }
                    actions.push(("prop.pickWhip".into(), params));
                }
                _ => {}
            }
        }
    }
    // Alt-drag key group scaling (follows the pointer until release).
}

fn alt_scale_and_graph(app: &mut EffectcraftApp, ui: &mut egui::Ui, g: &mut Rw, ectx: &EvalCtx<'_>) {
    let graph_x0 = g.tl.graph_x0;
    let tm = g.tl.tm;
    let rows_rect = g.tl.rows_rect;
    let ctx = g.tl.ctx.clone();
    let comp = g.tl.comp.clone();
    let actions = &mut g.tl.actions;
    let graph_on = g.graph_on;
    let gp = g.gp.clone();
    if let Some(c) = app.session.active_comp_arc() {
        super::graph_tools::alt_scale_update(app, &ctx, &c, tm, &mut *actions);
    }
    // Graph editor.
    if graph_on {
        super::graph::show(
            app,
            ui,
            &gp,
            super::graph::GraphView { comp: &comp, ectx, tm },
            Rect::from_min_max(pos2(graph_x0, rows_rect.min.y), rows_rect.max),
            &mut *actions,
        );
    }
    // A plain click on empty track area (beside a layer's bar, on a property row's empty span or
    // below the last row) deselects the layers and their keys and properties, as in After
}

/// Click / marquee on empty track area.
fn graph_empty_area(ui: &mut egui::Ui, g: &mut Rw) {
    let t = g.tl.t;
    let ctx = g.tl.ctx.clone();
    let actions = &mut g.tl.actions;
    let gp = g.gp.clone();
    let empty = g.empty.clone();
    if empty.clicked() && !ui.input(|i| i.modifiers.shift || i.modifiers.command) {
        actions.push(("layer.select".into(), json!({"layers": []})));
    }
    if let (true, Some(origin), Some(cur)) =
        (empty.dragged(), empty.interact_pointer_pos().and_then(|_| ctx.input(|i| i.pointer.press_origin())), empty.interact_pointer_pos())
    {
        let br = Rect::from_two_pos(origin, cur);
        gp.rect_filled(br, 0.0, t.accent.gamma_multiply(0.12));
        gp.rect_stroke(br, 0.0, Stroke::new(1.0, t.accent), StrokeKind::Middle);
        ctx.data_mut(|d| d.insert_temp(egui::Id::unique("tl-box"), br));
    }
}

/// Layer reorder drag: a line where the layers land.
fn layer_reorder_drag(ui: &mut egui::Ui, g: &mut Rw) {
    let t = g.tl.t;
    let rect = g.tl.rect;
    let graph_x0 = g.tl.graph_x0;
    let rows_rect = g.tl.rows_rect;
    let ctx = g.tl.ctx.clone();
    let comp = g.tl.comp.clone();
    let p = g.tl.p.clone();
    let actions = &mut g.tl.actions;
    let hit_rows = &mut g.hit_rows;
    if let Some(moving) = ctx.data(|d| d.get_temp::<Vec<u64>>(layer_drag_id())) {
        let dropped = ctx.data_mut(|d| d.remove_temp::<bool>(layer_drop_id())).unwrap_or(false);
        let layer_rows: Vec<(Rect, LayerId)> = hit_rows.iter().filter(|(_, r)| matches!(r.kind, RowKind::Layer)).map(|(rr, r)| (*rr, r.layer)).collect();
        if let Some(py) = ui.input(|i| i.pointer.interact_pos().or(i.pointer.latest_pos())).map(|p| p.y)
            && !layer_rows.is_empty()
        {
            let (at, line_y) = insertion(&layer_rows, py, rows_rect.min.y);
            let target = at.and_then(|i| layer_rows[i..].iter().find(|(_, l)| !moving.contains(&l.0)).map(|(_, l)| *l));
            if dropped {
                ctx.data_mut(|d| d.remove::<Vec<u64>>(layer_drag_id()));
                // Only when the order changes (no empty undo step for a drop in place).
                let mut order: Vec<u64> = comp.layers.iter().map(|l| l.id.0).filter(|id| !moving.contains(id)).collect();
                let at = target.and_then(|t| order.iter().position(|id| *id == t.0)).unwrap_or(order.len());
                order.splice(at..at, moving.iter().copied());
                if order != comp.layers.iter().map(|l| l.id.0).collect::<Vec<_>>() {
                    let params = match target {
                        Some(t) => json!({"layers": moving, "above": t.0}),
                        None => json!({"layers": moving, "to": "back"}),
                    };
                    actions.push(("layer.arrange".into(), params));
                }
            } else {
                ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
                p.with_clip_rect(rows_rect).line_segment([pos2(rect.min.x, line_y), pos2(graph_x0, line_y)], Stroke::new(2.0, t.accent));
            }
        } else if dropped {
            ctx.data_mut(|d| d.remove::<Vec<u64>>(layer_drag_id()));
        }
    }
    // Quick Offset of layer bars: the last selected layer moves by whole frames with the pointer
}

/// Quick Offset of layer bars follows the pointer.
fn quick_offset_follow(g: &mut Rw) {
    let t = g.tl.t;
    let pps = g.tl.pps;
    let fd = g.tl.fd;
    let ctx = g.tl.ctx.clone();
    let actions = &mut g.tl.actions;
    let ui_actions = &mut g.ui_actions;
    if let Some(mut q) = ctx.data(|d| d.get_temp::<QuickBar>(quick_bar_id())) {
        let (down, ptr) = ctx.input(|i| (i.pointer.primary_down(), i.pointer.latest_pos()));
        match ptr.filter(|_| down) {
            Some(ptr) => {
                let frames = (((ptr.x - q.origin_x) as f64 / pps) / fd).round() as i64;
                if frames != q.applied {
                    q.applied = frames;
                    ctx.data_mut(|d| d.insert_temp(quick_bar_id(), q));
                    let merge = format!("bar-quick-{}", q.layer.0);
                    actions.push(("layer.quickOffset".into(), json!({"delta": frames as f64 * fd, "merge": merge, "fromStart": true})));
                }
                drag_tip(&ctx, &t, ptr, quick_offset_note(q.applied, q.layers));
            }
            None => {
                ctx.data_mut(|d| d.remove::<QuickBar>(quick_bar_id()));
                ui_actions.push(UiAct::EndMerge);
            }
        }
    }
    // Keyframe drag: the selected keys move by whole frames with the pointer until it is
    // released (one undo step); Shift snaps the grabbed key to the current time, the work area
}

/// The key drag moves the selected keys by whole frames.
fn key_drag_follow(app: &mut EffectcraftApp, g: &mut Rw) {
    let t = g.tl.t;
    let time = g.tl.time;
    let pps = g.tl.pps;
    let fd = g.tl.fd;
    let ctx = g.tl.ctx.clone();
    let comp = g.tl.comp.clone();
    let rows = &g.tl.rows;
    let actions = &mut g.tl.actions;
    let ui_actions = &mut g.ui_actions;
    if let Some(mut kd) = ctx.data(|d| d.get_temp::<KeyDrag>(key_drag_id())) {
        let (down, ptr, shift) = ctx.input(|i| (i.pointer.primary_down(), i.pointer.latest_pos(), i.modifiers.shift));
        match ptr.filter(|_| down) {
            Some(ptr) => {
                let mut target = kd.anchor + (ptr.x - kd.origin_x) as f64 / pps;
                if shift {
                    // The current time and whatever it snaps to (keys, layer ends, markers), but
                    // not the moving keys themselves.
                    let moving: Vec<f64> =
                        app.session.state.selected_keys.iter().filter_map(|k| comp.layer(k.layer).map(|l| l.comp_time(k.time).seconds())).collect();
                    let mut cands = snap_candidates(&comp, rows);
                    cands.retain(|c| !moving.iter().any(|m| (m - c).abs() < 1e-9));
                    cands.push(time.seconds());
                    target = snap_time(&cands, target, 8.0 / pps);
                }
                let total = ((target - kd.anchor) / fd).round() as i64;
                if total != kd.applied {
                    actions.push(match kd.quick {
                        Some(_) => ("keys.quickOffset".into(), json!({"delta": total as f64 * fd, "merge": "key-quick-offset", "fromStart": true})),
                        None => ("keys.move".into(), json!({"delta": (total - kd.applied) as f64 * fd, "merge": "key-drag"})),
                    });
                    kd.applied = total;
                    ctx.data_mut(|d| d.insert_temp(key_drag_id(), kd));
                }
                ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
                // Where the grabbed key is and how far the keys moved (After Effects shows this
                // in the Info panel).
                let at = Tick::from_seconds_f64(kd.anchor + kd.applied as f64 * fd);
                let note = match kd.quick {
                    Some(n) => quick_offset_note(kd.applied, n),
                    None => format!("{}   {:+} f", super::timecode(&app.session, &comp, at), kd.applied),
                };
                drag_tip(&ctx, &t, ptr, note);
                app.ui.status = format!("Keyframe at {}", super::timecode(&app.session, &comp, at));
            }
            None => {
                ctx.data_mut(|d| d.remove::<KeyDrag>(key_drag_id()));
                ui_actions.push(UiAct::EndMerge);
            }
        }
    }
}

/// Releasing the key marquee selects the keys inside it.
fn key_box_select(app: &mut EffectcraftApp, ui: &mut egui::Ui, g: &mut Rw) {
    let tm = g.tl.tm;
    let rows_rect = g.tl.rows_rect;
    let rh = g.tl.rh;
    let ctx = g.tl.ctx.clone();
    let comp = g.tl.comp.clone();
    let rows = &g.tl.rows;
    let actions = &mut g.tl.actions;
    let empty = g.empty.clone();
    if empty.drag_stopped()
        && let Some(br) = ctx.data(|d| d.get_temp::<Rect>(egui::Id::unique("tl-box")))
    {
        ctx.data_mut(|d| d.remove::<Rect>(egui::Id::unique("tl-box")));
        let mut keys = vec![];
        let mut yy = rows_rect.min.y - app.ui.timeline.scroll_y;
        for row in rows.iter() {
            let cy = yy + rh / 2.0;
            yy += row_height(row, rh);
            if let RowKind::Prop { uid } = row.kind
                && cy >= br.min.y
                && cy <= br.max.y
                && let Some(l) = comp.layer(row.layer)
                && let Some(pr) = l.props.find(uid)
            {
                for k in &pr.keys {
                    let x = tm.x(l.comp_time(k.time).seconds());
                    if x >= br.min.x && x <= br.max.x {
                        keys.push(json!({"layer": l.id.0, "prop": uid, "time": k.time.seconds()}));
                    }
                }
            }
        }
        let mods = ui.input(|i| i.modifiers);
        if keys.is_empty() && !(mods.shift || mods.command) {
            // A marquee that catches nothing leaves nothing selected (layers included).
            actions.push(("layer.select".into(), json!({"layers": []})));
        } else {
            actions.push(("keys.select".into(), json!({"keys": keys, "add": mods.shift, "selectProperties": true})));
        }
    }
}

fn current_time_indicator(app: &mut EffectcraftApp, g: &Rw) {
    let t = g.tl.t;
    let time = g.tl.time;
    let rect = g.tl.rect;
    let graph_x0 = g.tl.graph_x0;
    let tm = g.tl.tm;
    let rows_rect = g.tl.rows_rect;
    let ruler = g.tl.ruler;
    let p = g.tl.p.clone();
    let cx = tm.x(time.seconds());
    if cx >= graph_x0 && cx <= rect.max.x {
        p.line_segment([pos2(cx, ruler.min.y + 12.0), pos2(cx, rows_rect.max.y)], Stroke::new(1.0, t.cti));
        let head = vec![
            pos2(cx - 6.0, ruler.min.y + 12.0),
            pos2(cx + 6.0, ruler.min.y + 12.0),
            pos2(cx + 6.0, ruler.max.y - 8.0),
            pos2(cx, ruler.max.y - 2.0),
            pos2(cx - 6.0, ruler.max.y - 8.0),
        ];
        p.add(egui::Shape::convex_polygon(head, t.cti, Stroke::NONE));
        app.auto.add("timeline.cti", Rect::from_center_size(pos2(cx, ruler.center().y), vec2(12.0, 20.0)), "Current time indicator");
    }
}

/// The footer: scroll bars, the switches / modes toggle and render time.
fn footer_bar(app: &mut EffectcraftApp, ui: &mut egui::Ui, g: &Rw) -> Rect {
    let t = g.tl.t;
    let rect = g.tl.rect;
    let footer_h = g.tl.footer_h;
    let overflow = g.tl.overflow;
    let oscroll = g.tl.oscroll;
    let graph_x0 = g.tl.graph_x0;
    let rows_rect = g.tl.rows_rect;
    let max_scroll = g.tl.max_scroll;
    let p = g.tl.p.clone();
    let footer = Rect::from_min_max(pos2(rect.min.x, rect.max.y - footer_h), rect.max);
    p.rect_filled(footer, 0.0, t.panel_bg);
    p.line_segment([footer.left_top(), footer.right_top()], Stroke::new(1.0, t.separator));
    if overflow > 0.0 {
        // Outline scroll bar along the top of the footer, under the columns.
        let track = Rect::from_min_max(pos2(rect.min.x + 2.0, footer.min.y + 1.0), pos2(graph_x0 - 3.0, footer.min.y + 5.0));
        app.ui.timeline.outline_scroll = widgets::scroll_bar(ui, &mut app.auto, "timeline.outlineScroll", track, oscroll, overflow, &t);
    }
    if max_scroll > 0.0 {
        // The layer rows' scroll bar, on the right edge beside the time graph.
        let track = Rect::from_min_max(pos2(rect.max.x - 6.0, rows_rect.min.y + 1.0), pos2(rect.max.x - 2.0, rows_rect.max.y - 1.0));
        app.ui.timeline.scroll_y = widgets::scroll_bar(ui, &mut app.auto, "timeline.scroll", track, app.ui.timeline.scroll_y, max_scroll, &t);
    }
    let tgl = Rect::from_min_size(pos2(footer.min.x + 10.0, footer.min.y + 3.0), vec2(150.0, 18.0));
    let tresp = ui.interact(tgl, egui::Id::unique("tl-toggle-modes"), Sense::click());
    p.text(
        tgl.left_center(),
        Align2::LEFT_CENTER,
        crate::i18n::tr("Toggle Switches / Modes"),
        Tokens::ui(11.0),
        if tresp.hovered() { t.text } else { t.text_dim },
    );
    app.auto.add("timeline.toggleSwitchesModes", tgl, "Toggle Switches / Modes");
    if tresp.clicked() {
        app.ui.timeline.show_modes = !app.ui.timeline.show_modes;
    }
    let ms = app.frames.last_ms.lock().map(|v| *v).unwrap_or(0.0);
    p.text(
        pos2(graph_x0 - 12.0, footer.center().y),
        Align2::RIGHT_CENTER,
        crate::i18n::tr_args("Frame Render Time  {}ms", &[&format!("{ms:.0}")]),
        Tokens::ui(11.0),
        t.text_faint,
    );
    // Zoom slider.
    footer
}

/// The zoom buttons and slider.
fn footer_zoom(app: &mut EffectcraftApp, ui: &mut egui::Ui, g: &Rw, footer: Rect) {
    let t = g.tl.t;
    let time = g.tl.time;
    let rect = g.tl.rect;
    let graph_x0 = g.tl.graph_x0;
    let fit_pps = g.tl.fit_pps;
    let pps = g.tl.pps;
    let tm = g.tl.tm;
    let comp = g.tl.comp.clone();
    let p = g.tl.p.clone();
    let zs = Rect::from_min_max(pos2(graph_x0 + 30.0, footer.center().y - 2.0), pos2((graph_x0 + 230.0).min(rect.max.x - 30.0), footer.center().y + 2.0));
    // Zoom out / zoom in buttons either side of the slider: the `-` / `=` steps. Everything here
    // zooms around the CTI and sets the one time zoom the layer bars and the Graph Editor share.
    let cti = time.seconds();
    let cti_px = (cti - tm.start) * pps;
    for (icon, size, cx, id, tip, k) in [
        (Icon::MountainSmall, 14.0, zs.min.x - 14.0, "timeline.zoomOutButton", "Zoom Out (Time)", 1.0 / ZOOM_STEP),
        (Icon::MountainLarge, 16.0, zs.max.x + 14.0, "timeline.zoomInButton", "Zoom In (Time)", ZOOM_STEP),
    ] {
        let r = Rect::from_center_size(pos2(cx, footer.center().y), vec2(20.0, 20.0));
        let resp = ui.interact(r, egui::Id::unique(id), Sense::click()).on_hover_text(crate::i18n::tr(tip));
        icons::paint(&p, Rect::from_center_size(r.center(), vec2(size, size)), icon, if resp.hovered() { t.text } else { t.text_dim });
        app.auto.add(id, r, tip);
        if resp.clicked() {
            set_zoom(&mut app.ui.timeline, &comp, pps * k, fit_pps, cti, cti_px);
        }
    }
    p.rect_filled(zs, 2.0, t.field_bg);
    let maxpps = MAX_PPS.max(fit_pps * 1.001);
    let frac = ((pps / fit_pps).ln() / (maxpps / fit_pps).ln()).clamp(0.0, 1.0) as f32;
    let knob = pos2(zs.min.x + zs.width() * frac, zs.center().y);
    p.circle_filled(knob, 6.0, t.text);
    // Drag the knob, or click the track to jump there.
    let zresp = ui.interact(zs.expand2(vec2(6.0, 8.0)), egui::Id::unique("tl-zoom"), Sense::click_and_drag());
    app.auto.add("timeline.zoomSlider", zs, "Zoom");
    if (zresp.dragged() || zresp.clicked())
        && let Some(pt) = zresp.interact_pointer_pos()
    {
        let f = ((pt.x - zs.min.x) / zs.width().max(1.0)).clamp(0.0, 1.0) as f64;
        set_zoom(&mut app.ui.timeline, &comp, fit_pps * (maxpps / fit_pps).powf(f), fit_pps, cti, cti_px);
    }
}

fn comp_markers_ui(app: &mut EffectcraftApp, ui: &mut egui::Ui, g: &Rw) {
    let graph_x0 = g.tl.graph_x0;
    let tm = g.tl.tm;
    let rows_rect = g.tl.rows_rect;
    let ruler = g.tl.ruler;
    let bin = g.tl.bin;
    let comp = g.tl.comp.clone();
    // Composition markers in the ruler; protected regions shade the layer rows.
    let mstrip = Rect::from_min_max(pos2(graph_x0, ruler.max.y - 12.0), pos2(bin.min.x, ruler.max.y));
    let mrows = rows_rect.intersect(Rect::from_min_max(pos2(graph_x0, rows_rect.min.y), rows_rect.max));
    super::markers_ui::comp_markers(app, ui, &comp, tm, mstrip, mrows);
    super::markers_ui::marker_bin(app, ui, &comp, tm, bin, mstrip);
}

/// Spacebar held: the Hand tool.
fn hand_tool_pan(app: &mut EffectcraftApp, ui: &mut egui::Ui, g: &Rw) {
    let rect = g.tl.rect;
    let graph_x0 = g.tl.graph_x0;
    let graph_x1 = g.tl.graph_x1;
    let pps = g.tl.pps;
    let tm = g.tl.tm;
    let rows_rect = g.tl.rows_rect;
    let ruler = g.tl.ruler;
    let max_scroll = g.tl.max_scroll;
    let ctx = g.tl.ctx.clone();
    let comp = g.tl.comp.clone();
    if super::space_hand(&ctx) {
        let area = Rect::from_min_max(pos2(graph_x0, ruler.min.y), pos2(rect.max.x, rows_rect.max.y));
        let hand = ui.interact(area, egui::Id::unique("tl-hand"), Sense::drag());
        app.auto.add("timeline.hand", area, "Hand (hold Spacebar)");
        if hand.hovered() || hand.dragged() {
            ctx.set_cursor_icon(if hand.dragged() { egui::CursorIcon::Grabbing } else { egui::CursorIcon::Grab });
        }
        if hand.dragged() {
            let dy = ui.input(|i| i.pointer.delta().y);
            app.ui.timeline.scroll_y = (app.ui.timeline.scroll_y - dy).clamp(0.0, max_scroll);
            if app.ui.timeline.pps.is_some() {
                let start = app.ui.timeline.start;
                let last = (comp.duration.seconds() - (tm.t(graph_x1) - tm.start)).max(start).max(0.0);
                app.ui.timeline.start = (start - hand.drag_delta().x as f64 / pps).clamp(0.0, last);
            }
        }
    }
}

/// Alt-drag of an item over a layer: replace its source.
fn drop_replace_source(app: &mut EffectcraftApp, g: &mut Rw, payload: &std::sync::Arc<crate::panels::DragPayload>, ptr: Pos2, released: bool) {
    let t = g.tl.t;
    let cid = g.tl.cid;
    let rows_rect = g.tl.rows_rect;
    let comp = g.tl.comp.clone();
    let p = g.tl.p.clone();
    let actions = &mut g.tl.actions;
    let hit_rows = &mut g.hit_rows;
    if let crate::panels::DragPayload::Item(id) = payload.as_ref() {
        let target = hit_rows.iter().find(|(r, _)| ptr.y >= r.min.y && ptr.y < r.max.y).map(|(_, row)| row.layer);
        if let Some(l) = target
            && replace_source_target(app, cid, ItemId(*id), l, &comp)
            && let Some(span) = hit_rows.iter().filter(|(_, row)| row.layer == l).map(|(r, _)| *r).reduce(|a, b| a.union(b))
        {
            let lit = span.intersect(rows_rect);
            let rp = p.with_clip_rect(rows_rect);
            rp.rect_filled(lit, 0.0, t.accent.gamma_multiply(0.18));
            rp.rect_stroke(lit, 0.0, Stroke::new(2.0, t.accent), StrokeKind::Inside);
            app.auto.add("timeline.replaceSourceTarget", lit, "Replace the layer's source");
            if released {
                actions.push(("layer.replaceSource".into(), json!({"layers": [l.0], "item": *id, "fixExpressions": false})));
            }
        }
    }
}

/// Effects and presets drop onto the layer under the pointer.
fn drop_on_layer(app: &mut EffectcraftApp, g: &mut Rw, payload: &std::sync::Arc<crate::panels::DragPayload>, ptr: Pos2, released: bool) {
    let t = g.tl.t;
    let rows_rect = g.tl.rows_rect;
    let p = g.tl.p.clone();
    let actions = &mut g.tl.actions;
    let hit_rows = &mut g.hit_rows;
    let target = hit_rows.iter().find(|(r, _)| ptr.y >= r.min.y && ptr.y < r.max.y).map(|(_, row)| row.layer);
    // The layer the effect would land on (its row and its open properties) is the one
    // that lights up, not the whole timeline.
    if let Some(l) = target
        && let Some(span) = hit_rows.iter().filter(|(_, row)| row.layer == l).map(|(r, _)| *r).reduce(|a, b| a.union(b))
    {
        let lit = span.intersect(rows_rect);
        let rp = p.with_clip_rect(rows_rect);
        rp.rect_filled(lit, 0.0, t.accent.gamma_multiply(0.18));
        rp.rect_stroke(lit, 0.0, Stroke::new(2.0, t.accent), StrokeKind::Inside);
        app.auto.add("timeline.dropTarget", lit, "Drop target layer");
    }
    if released && let Some(l) = target {
        match payload.as_ref() {
            crate::panels::DragPayload::Effect(e) => actions.push(("effect.apply".into(), json!({"effect": e, "layers": [l.0]}))),
            crate::panels::DragPayload::Preset { preset, .. } => {
                let (id, params) = preset.apply_to(l.0);
                actions.push((id.into(), params));
            }
            _ => {}
        }
    }
}

/// Footage and files drop into the stack: a line shows the place.
fn drop_insert_layers(app: &mut EffectcraftApp, g: &mut Rw, payload: &std::sync::Arc<crate::panels::DragPayload>, ptr: Pos2, released: bool) {
    let t = g.tl.t;
    let time = g.tl.time;
    let rect = g.tl.rect;
    let graph_x0 = g.tl.graph_x0;
    let tm = g.tl.tm;
    let rows_rect = g.tl.rows_rect;
    let ctx = g.tl.ctx.clone();
    let comp = g.tl.comp.clone();
    let p = g.tl.p.clone();
    let actions = &mut g.tl.actions;
    let graph_on = g.graph_on;
    let hit_rows = &mut g.hit_rows;
    let layer_rows: Vec<(Rect, LayerId)> = hit_rows.iter().filter(|(_, r)| matches!(r.kind, RowKind::Layer)).map(|(rr, r)| (*rr, r.layer)).collect();
    let (at, line_y) = insertion(&layer_rows, ptr.y, rows_rect.min.y);
    let index = match at.and_then(|i| layer_rows.get(i)) {
        Some((_, l)) => comp.index_of(*l).unwrap_or(1),
        None => layer_rows.last().and_then(|(_, l)| comp.index_of(*l)).unwrap_or(comp.layers.len()) + 1,
    };
    let start = (ptr.x >= graph_x0 && !graph_on).then(|| {
        let t = if ctx.input(|i| i.modifiers.shift) { time } else { Tick::from_seconds_f64(tm.t(ptr.x)) };
        comp.frame_rate.snap_nearest(t).clamp(Tick::ZERO, comp.duration - comp.frame_duration())
    });
    let rp = p.with_clip_rect(rows_rect);
    rp.line_segment([pos2(rect.min.x, line_y), pos2(rows_rect.max.x, line_y)], Stroke::new(2.0, t.accent));
    if let Some(s) = start {
        let x = tm.x(s.seconds());
        rp.line_segment([pos2(x, rows_rect.min.y), pos2(x, rows_rect.max.y)], Stroke::new(1.0, t.accent));
        // The In point's time in a tag beside the marker.
        let g = rp.layout_no_wrap(super::timecode(&app.session, &comp, s), Tokens::ui(11.0), Color32::WHITE);
        let tag = Align2::LEFT_BOTTOM.anchor_size(pos2(x + 4.0, line_y - 3.0), g.size());
        rp.rect_filled(tag.expand(2.0), 2.0, t.accent);
        rp.galley(tag.min, g, Color32::WHITE);
    }
    if released {
        let time = json!(start.map(Tick::seconds));
        match payload.as_ref() {
            crate::panels::DragPayload::Item(id) => {
                // Insert each selected item at the drop point, in selection order.
                for (offset, item) in crate::panels::project_drop_items(app, *id).into_iter().enumerate() {
                    actions.push(("layer.addItem".into(), json!({"item": item, "index": index.saturating_add(offset), "time": time.clone()})));
                }
            }
            crate::panels::DragPayload::Files(paths) => {
                actions.push(("mediaBrowser.import".into(), json!({"paths": paths, "addToComp": true, "index": index, "time": time})))
            }
            crate::panels::DragPayload::Effect(_) | crate::panels::DragPayload::Preset { .. } | crate::panels::DragPayload::Property { .. } => {}
        }
    }
}

/// The optional per-layer cells: Comment (double-click to edit), In / Out / Duration (drag to
/// change by frames), Stretch (click: Time Stretch dialog).
fn layer_cells(
    app: &mut EffectcraftApp,
    ui: &mut egui::Ui,
    lp: &egui::Painter,
    cw: &Cols,
    comp: &Comp,
    rc: &RowCtx<'_>,
    actions: &mut Vec<(String, serde_json::Value)>,
) {
    let (layer, r, cy) = (rc.layer, rc.r, rc.cy);
    let t = app.tokens;
    let ctx = ui.ctx().clone();
    let lid = layer.id.0;
    if cw.vis.comment {
        let cr = Rect::from_min_max(pos2(cw.comment + 4.0, r.min.y + 1.0), pos2(cw.comment + COMMENT_W - 4.0, r.max.y - 1.0));
        let edit_id = egui::Id::unique("tl-comment-edit");
        let editing: Option<(u64, String)> = ctx.data(|d| d.get_temp(edit_id));
        app.auto.add(&format!("timeline.layer.{lid}.comment"), cr, &layer.comment);
        if let Some((el, mut buf)) = editing.filter(|(el, _)| *el == lid) {
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(cr));
            let er = child.add(egui::TextEdit::singleline(&mut buf).font(Tokens::ui(11.5)).desired_width(cr.width()));
            if er.lost_focus() {
                if !ui.input(|i| i.key_pressed(egui::Key::Escape)) && buf != layer.comment {
                    actions.push(("layer.setComment".into(), json!({"layers": [el], "comment": buf})));
                }
                ctx.data_mut(|d| d.remove::<(u64, String)>(edit_id));
            } else {
                widgets::keep_focus(&ctx, &er, &buf);
                ctx.data_mut(|d| d.insert_temp(edit_id, (el, buf)));
            }
        } else {
            let resp =
                ui.interact(cr, egui::Id::unique(("tl-comment", lid)), Sense::click()).on_hover_text(crate::i18n::tr("Double-click to edit the comment"));
            lp.with_clip_rect(cr.intersect(lp.clip_rect())).text(pos2(cr.min.x + 2.0, cy), Align2::LEFT_CENTER, &layer.comment, Tokens::ui(11.5), t.text_dim);
            if resp.double_clicked() {
                ctx.data_mut(|d| d.insert_temp(edit_id, (lid, layer.comment.clone())));
            }
        }
    }
    let fd = comp.frame_duration().seconds();
    let cells = [
        (cw.vis.in_, cw.in_, "in", layer.in_point.seconds()),
        (cw.vis.out, cw.out, "out", layer.out_point.seconds()),
        (cw.vis.duration, cw.duration, "duration", (layer.out_point - layer.in_point).seconds()),
    ];
    for (on, x, kind, secs) in cells {
        if !on {
            continue;
        }
        let cr = Rect::from_min_max(pos2(x + 2.0, r.min.y + 1.0), pos2(x + TIME_W - 2.0, r.max.y - 1.0));
        let tc = if kind == "duration" {
            crate::panels::duration(&app.session, comp, Tick::from_seconds_f64(secs))
        } else {
            crate::panels::timecode(&app.session, comp, Tick::from_seconds_f64(secs))
        };
        let resp = ui.interact(cr, egui::Id::unique(("tl-cell", kind, lid)), Sense::drag()).on_hover_text(crate::i18n::tr("Drag to change"));
        lp.text(pos2(cr.min.x + 4.0, cy), Align2::LEFT_CENTER, &tc, Tokens::mono(11.0), if resp.hovered() || resp.dragged() { t.hot_text } else { t.timecode });
        app.auto.add(&format!("timeline.layer.{lid}.{kind}"), cr, &tc);
        if resp.hovered() || resp.dragged() {
            ctx.set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
        }
        let acc_id = egui::Id::unique(("tl-cell-acc", kind, lid));
        if resp.dragged() {
            let mut acc: f32 = ctx.data(|d| d.get_temp(acc_id).unwrap_or(0.0));
            acc += resp.drag_delta().x;
            let frames = (acc / 4.0).trunc() as i64;
            if frames != 0 {
                acc -= frames as f32 * 4.0;
                let d = frames as f64 * fd;
                let merge = format!("cell-{kind}-{lid}");
                let params = match kind {
                    "in" => json!({"layers": [lid], "in": (layer.in_point.seconds() + d).max(0.0), "merge": merge}),
                    _ => json!({"layers": [lid], "out": (layer.out_point.seconds() + d).max(layer.in_point.seconds() + fd), "merge": merge}),
                };
                actions.push(("layer.timing".into(), params));
            }
            ctx.data_mut(|d| d.insert_temp(acc_id, acc));
        }
        if resp.drag_stopped() {
            ctx.data_mut(|d| d.remove::<f32>(acc_id));
            actions.push(("__endMerge".into(), json!({})));
        }
    }
    if cw.vis.stretch {
        let cr = Rect::from_min_max(pos2(cw.stretch + 2.0, r.min.y + 1.0), pos2(cw.stretch + STRETCH_W - 2.0, r.max.y - 1.0));
        let label = format!("{:.1}%", layer.stretch * 100.0);
        let resp = ui.interact(cr, egui::Id::unique(("tl-stretch", lid)), Sense::click()).on_hover_text(crate::i18n::tr("Time Stretch…"));
        lp.text(pos2(cr.min.x + 4.0, cy), Align2::LEFT_CENTER, &label, Tokens::ui(11.5), if resp.hovered() { t.hot_text } else { t.text });
        app.auto.add(&format!("timeline.layer.{lid}.stretch"), cr, &label);
        if resp.clicked() {
            actions.push(("layer.select".into(), json!({"layers": [lid]})));
            actions.push(("layer.timeStretch".into(), json!({})));
        }
    }
}

enum UiAct {
    ToggleLayer(u64, bool),
    /// Layer, group uid, and whether the whole tree under the group follows.
    ToggleGroup(u64, u64, bool),
    SetTime(Tick),
    EndMerge,
    /// Composition Settings on its 3D Renderer tab.
    ChangeRenderer,
}

fn walk_groups(g: &PropGroup, out: &mut Vec<u64>) {
    // Audio's Waveform row is not in the property tree; it twirls along.
    if g.match_id == "audio" {
        out.push(WAVE_BIT | g.uid);
    }
    for sg in g.groups() {
        out.push(sg.uid);
        walk_groups(sg, out);
    }
}

/// The group `uid` anywhere under `g`.
fn find_group(g: &PropGroup, uid: u64) -> Option<&PropGroup> {
    g.groups().find_map(|sg| if sg.uid == uid { Some(sg) } else { find_group(sg, uid) })
}

/// A stroke's Dashes "+" / "−" buttons (add or remove a Dash/Gap pair).
fn dash_buttons(
    app: &mut EffectcraftApp,
    ui: &mut egui::Ui,
    p: &egui::Painter,
    layer: &Layer,
    uid: u64,
    at: Pos2,
    actions: &mut Vec<(String, serde_json::Value)>,
) {
    let (x, cy) = (at.x, at.y);
    if !matches!(layer.source, LayerSource::Shape) || layer.props.find_group(uid).is_none_or(|g| g.match_id != "dashes") {
        return;
    }
    let Some(stroke) = layer.props.parent_of(uid).map(|g| g.uid) else { return };
    let t = app.tokens;
    for (i, (sym, id, tip)) in
        [("+", "shape.dashes.add", "Add a Dash or Gap"), ("\u{2212}", "shape.dashes.remove", "Remove a Dash or Gap")].into_iter().enumerate()
    {
        let r = Rect::from_center_size(pos2(x + 12.0 + i as f32 * 20.0, cy), vec2(16.0, 16.0));
        let resp = ui.interact(r, egui::Id::unique(("tl-dash", uid, i)), Sense::click()).on_hover_text(tip);
        p.rect_stroke(r.shrink(1.0), 2.0, Stroke::new(1.0, t.separator), StrokeKind::Inside);
        p.text(r.center(), Align2::CENTER_CENTER, sym, Tokens::ui(12.0), if resp.hovered() { t.text } else { t.text_dim });
        app.auto.add(&format!("timeline.group.{uid}.{}", if i == 0 { "addDash" } else { "removeDash" }), r, tip);
        if resp.clicked() {
            actions.push((id.into(), json!({"layer": layer.id.0, "prop": stroke})));
        }
    }
}

/// The shape items the Contents row's "Add:" menu offers, as `layer.addShapeItem` kinds ("-" is
/// a separator), in After Effects' order.
const SHAPE_ADD_ITEMS: &[(&str, &str)] = &[
    ("group", "Group (empty)"),
    ("-", ""),
    ("rect", "Rectangle"),
    ("ellipse", "Ellipse"),
    ("star", "Polystar"),
    ("path", "Path"),
    ("-", ""),
    ("fill", "Fill"),
    ("stroke", "Stroke"),
    ("gfill", "Gradient Fill"),
    ("gstroke", "Gradient Stroke"),
    ("-", ""),
    ("frontColor", "Front: Color"),
    ("bevelColor", "Bevel: Color"),
    ("sideColor", "Side: Color"),
    ("backColor", "Back: Color"),
    ("-", ""),
    ("merge", "Merge Paths"),
    ("offset", "Offset Paths"),
    ("pucker", "Pucker & Bloat"),
    ("repeater", "Repeater"),
    ("round", "Round Corners"),
    ("trim", "Trim Paths"),
    ("twist", "Twist"),
    ("wiggle", "Wiggle Paths"),
    ("zigzag", "Zig Zag"),
];

/// A shape layer's Contents "Add:" pop-up menu: adds a shape, paint or path operation to the
/// selected shape group, else to the layer's contents.
fn shape_add_popup(
    app: &mut EffectcraftApp,
    ui: &mut egui::Ui,
    p: &egui::Painter,
    layer: &Layer,
    uid: u64,
    at: Pos2,
    actions: &mut Vec<(String, serde_json::Value)>,
) {
    let (x, cy) = (at.x, at.y);
    if !matches!(layer.source, LayerSource::Shape) || layer.props.sub("contents").is_none_or(|c| c.uid != uid) {
        return;
    }
    let t = app.tokens;
    p.text(pos2(x + 6.0, cy), Align2::LEFT_CENTER, crate::i18n::tr("Add:"), Tokens::ui(11.5), t.text_dim);
    let br = Rect::from_center_size(pos2(x + 40.0, cy), vec2(16.0, 16.0));
    let resp =
        ui.interact(br, egui::Id::unique(("tl-shapeadd", uid)), Sense::click()).on_hover_text(crate::i18n::tr("Add a shape, fill, stroke or path operation"));
    icons::paint(p, br.shrink(3.0), Icon::ChevronRight, if resp.hovered() { t.text } else { t.text_dim });
    app.auto.add(&format!("timeline.group.{uid}.add"), br, "Add:");
    let pop = egui::Id::unique(("tl-shapeadd-pop", uid));
    if resp.clicked() {
        widgets::open_popup(ui, pop);
    }
    let opts: Vec<String> = SHAPE_ADD_ITEMS.iter().map(|(k, l)| if *k == "-" { "-".to_string() } else { l.to_string() }).collect();
    if let Some(i) = widgets::popup_menu(ui, pop, br.left_bottom(), &opts, None)
        && let Some((kind, _)) = SHAPE_ADD_ITEMS.get(i).filter(|(k, _)| *k != "-")
    {
        let mut params = json!({"layer": layer.id.0, "kind": kind});
        // Into the selected shape group, as After Effects does.
        let group = app.session.state.selected_props.iter().filter(|(l, _)| *l == layer.id).find_map(|(_, u)| {
            let g = layer.props.find_group(*u)?;
            (g.match_id == "group" && g.sub("contents").is_some()).then_some(g.uid)
        });
        if let Some(g) = group {
            params["group"] = json!(g);
        }
        actions.push(("layer.addShapeItem".into(), params));
    }
}

/// The Text group's "Animate:" and an animator's "Add:" pop-up menus (AE's twirl-down menus).
fn text_anim_popups(
    app: &mut EffectcraftApp,
    ui: &mut egui::Ui,
    p: &egui::Painter,
    layer: &Layer,
    uid: u64,
    at: Pos2,
    actions: &mut Vec<(String, serde_json::Value)>,
) {
    let (x, cy) = (at.x, at.y);
    use effectcraft_engine::project::build::TEXT_ANIMATOR_KINDS;
    if !matches!(layer.source, LayerSource::Text) {
        return;
    }
    let Some(g) = layer.props.find_group(uid) else { return };
    let t = app.tokens;
    let (label, key) = match g.match_id.as_str() {
        "text" if layer.props.sub("text").is_some_and(|tg| tg.uid == uid) => ("Animate:", "animate"),
        "animator" => ("Add:", "add"),
        _ => return,
    };
    p.text(pos2(x + 6.0, cy), Align2::LEFT_CENTER, label, Tokens::ui(11.5), t.text_dim);
    let br = Rect::from_center_size(pos2(x + 6.0 + if key == "add" { 34.0 } else { 62.0 }, cy), vec2(16.0, 16.0));
    let resp = ui.interact(br, egui::Id::unique(("tl-textanim", uid)), Sense::click()).on_hover_text(if key == "add" {
        "Add property or selector"
    } else {
        "Animate text"
    });
    icons::paint(p, br.shrink(3.0), Icon::ChevronRight, if resp.hovered() { t.text } else { t.text_dim });
    app.auto.add(&format!("timeline.group.{uid}.{key}"), br, label);
    let pop = egui::Id::unique(("tl-textanim-pop", uid));
    if resp.clicked() {
        widgets::open_popup(ui, pop);
    }
    let per_char = layer.props.prop("text/perChar3d").is_some_and(|q| q.value.as_bool());
    let mut opts: Vec<String> = Vec::new();
    let mut acts: Vec<(String, serde_json::Value)> = Vec::new();
    if key == "animate" {
        opts.push(tr(if per_char { "Disable Per-character 3D" } else { "Enable Per-character 3D" }).to_string());
        acts.push(("layer.enablePerChar3D".into(), json!({"layer": layer.id.0, "enabled": !per_char})));
        opts.push("-".into());
        acts.push((String::new(), json!(null)));
        for (k, l) in TEXT_ANIMATOR_KINDS {
            opts.push(l.to_string());
            acts.push(if *k == "-" { (String::new(), json!(null)) } else { ("layer.addTextAnimator".into(), json!({"layer": layer.id.0, "property": k})) });
        }
        // Variable Font Axes of the layer's font.
        if let Some(effectcraft_engine::keyframe::Value::Text(doc)) = layer.props.prop("text/sourceText").map(|p| p.value.clone()) {
            let face = effectcraft_engine::text::resolve(&doc.font, &doc.style).face;
            let axes = effectcraft_engine::text::variable::font_axes(face);
            if !axes.is_empty() {
                opts.push("-".into());
                acts.push((String::new(), json!(null)));
            }
            for a in axes {
                opts.push(format!("Variable Font Axes: {}", a.name));
                acts.push(("text.animatorFontAxes".into(), json!({"layer": layer.id.0, "axis": a.tag})));
            }
        }
    } else {
        for (kind, l) in [("range", "Selector: Range"), ("wiggly", "Selector: Wiggly"), ("expression", "Selector: Expression")] {
            opts.push(l.into());
            acts.push(("layer.addTextSelector".into(), json!({"layer": layer.id.0, "animator": uid, "kind": kind})));
        }
        opts.push("-".into());
        acts.push((String::new(), json!(null)));
        for (k, l) in TEXT_ANIMATOR_KINDS {
            opts.push(if *k == "-" { "-".into() } else { format!("Property: {l}") });
            acts.push(if *k == "-" {
                (String::new(), json!(null))
            } else {
                ("layer.addTextAnimatorProperty".into(), json!({"layer": layer.id.0, "animator": uid, "property": k}))
            });
        }
    }
    if let Some(i) = widgets::popup_menu(ui, pop, br.left_bottom(), &opts, None)
        && let Some((id, params)) = acts.get(i).filter(|a| !a.0.is_empty())
    {
        actions.push((id.clone(), params.clone()));
    }
}

/// The command for the Track Matte menu item `i` (`choice`) of `layer`, whose menu lists "No Track
/// Matte", a separator, the matte `candidates`, a separator and the modes. A mode without a matte
/// layer takes the layer above, like Layer ▸ Track Matte (#864: it used to do nothing).
fn track_matte_pick(layer: &Layer, candidates: &[LayerId], i: usize, choice: &str) -> Option<(String, serde_json::Value)> {
    if i == 0 {
        return Some(("layer.setTrackMatte".into(), json!({"layer": layer.id.0, "matte": null})));
    }
    if let Some(m) = i.checked_sub(2).and_then(|j| candidates.get(j)) {
        let kind = layer.track_matte.map(|m| m.kind).unwrap_or_default();
        return Some(("layer.setTrackMatte".into(), json!({"layer": layer.id.0, "matte": m.0, "kind": kind_name(kind)})));
    }
    let k = MatteKind::ALL.into_iter().find(|k| k.label() == choice)?;
    Some(("layer.trackMatte".into(), json!({"layers": [layer.id.0], "op": kind_name(k)})))
}

fn kind_name(k: MatteKind) -> &'static str {
    match k {
        MatteKind::Alpha => "alpha",
        MatteKind::AlphaInverted => "alphaInverted",
        MatteKind::Luma => "luma",
        MatteKind::LumaInverted => "lumaInverted",
    }
}

/// Is property `uid` the layer's Position (or, separated, its X, Y or Z Position)?
pub(crate) fn is_position(layer: &Layer, uid: u64) -> bool {
    layer.transform().is_some_and(|tr| ["position", "positionX", "positionY", "positionZ"].iter().any(|m| tr.get(m).is_some_and(|p| p.uid == uid)))
}

/// A property's context menu (right-click its name in the Timeline), as in After Effects.
fn prop_menu(
    ui: &mut egui::Ui,
    auto: &mut crate::automation::Registry,
    comp: ItemId,
    layer: &Layer,
    prop: &Property,
    actions: &mut Vec<(String, serde_json::Value)>,
) {
    let (l, uid) = (layer.id.0, prop.uid);
    let mut item = |ui: &mut egui::Ui, key: &str, label: &str, selected: bool, cmd: &str, params: serde_json::Value| {
        let resp = ui.add(egui::Button::new(crate::i18n::context_item(cmd, &params, label)).selected(selected));
        auto.add(&format!("timeline.prop.{uid}.menu.{key}"), resp.rect, label);
        if resp.clicked() {
            actions.push((cmd.into(), params));
            ui.close();
        }
    };
    item(ui, "reset", "Reset", false, "prop.reset", json!({"layer": l, "prop": uid}));
    if matches!(prop.value, Value::Scalar(_) | Value::Vec2(_) | Value::Vec3(_)) {
        item(ui, "editValue", "Edit Value…", false, "prop.set", json!({"layer": l, "prop": uid}));
    }
    if is_position(layer, uid) {
        let separated = layer.transform().is_some_and(|tr| tr.get("positionX").is_some());
        item(ui, "separateDimensions", "Separate Dimensions", separated, "prop.separateDimensions", json!({"layer": l, "value": !separated}));
    }
    ui.separator();
    let (key, label) = if prop.expr.is_some() { ("removeExpression", "Remove Expression") } else { ("addExpression", "Add Expression") };
    item(ui, key, label, false, "prop.setExpression", json!({"layer": l, "prop": uid}));
    item(ui, "essentialGraphics", "Add Property to Essential Graphics", false, "essential.addProperty", json!({"comp": comp.0, "layer": l, "prop": uid}));
}

/// Whether `layer` shows vector footage (SVG, PDF, AI, EPS).
pub(crate) fn vector_footage_layer(project: &effectcraft_engine::project::Project, layer: &Layer) -> bool {
    layer
        .source
        .item()
        .and_then(|item| project.item(item))
        .is_some_and(|item| matches!(&item.kind, effectcraft_engine::project::ItemKind::Footage(footage) if effectcraft_engine::vector::is_vector(footage)))
}

/// Whether the Collapse Transformations / Continuously Rasterize switch applies: precomps, shape and text
/// layers, and vector footage (SVG, PDF, AI, EPS), which the renderer re-rasterizes at the layer's scale.
fn collapse_applies(layer: &Layer, project: &effectcraft_engine::project::Project) -> bool {
    match &layer.source {
        LayerSource::Comp { .. } | LayerSource::Shape | LayerSource::Text => true,
        LayerSource::Footage { item } => {
            let kind = project.item(*item).map(|i| &i.kind);
            matches!(kind, Some(effectcraft_engine::project::ItemKind::Footage(f)) if effectcraft_engine::render::is_vector_footage(f))
        }
        _ => false,
    }
}

/// Which Create ▸ conversions and Reveal ▸ actions the layer context menu offers, from the
/// targeted layer's kind.
#[derive(Clone, Copy, Default)]
pub(crate) struct LayerMenuCaps {
    /// SVG / PDF / AI / EPS footage: Create Shapes from Vector Layer.
    pub vector: bool,
    /// A text layer: Create Shapes from Text / Create Masks from Text.
    pub text: bool,
    /// The layer has a source item to reveal in the Project panel.
    pub source: bool,
}

impl LayerMenuCaps {
    pub(crate) fn of(project: &effectcraft_engine::project::Project, layer: &Layer) -> Self {
        Self {
            vector: vector_footage_layer(project, layer),
            text: matches!(layer.source, LayerSource::Text),
            source: layer.source.item().is_some_and(|item| project.item(item).is_some()),
        }
    }
}

/// A layer row's or bar's context menu. As in After Effects, right-clicking a layer that is part
/// of the selection keeps the selection and the menu acts on all of it; right-clicking an
/// unselected layer selects only that layer.
fn layer_context_menu(
    resp: &egui::Response,
    comp: &Comp,
    layer: &Layer,
    selected: &[LayerId],
    caps: LayerMenuCaps,
    auto: &mut crate::automation::Registry,
    actions: &mut Vec<(String, serde_json::Value)>,
) {
    let in_selection = selected.contains(&layer.id);
    if resp.secondary_clicked() && !in_selection && !layer.switches.locked {
        actions.push(("layer.select".into(), json!({"layers": [layer.id.0]})));
    }
    let targets: Vec<u64> = if in_selection { comp.layers.iter().filter(|l| selected.contains(&l.id)).map(|l| l.id.0).collect() } else { vec![layer.id.0] };
    // The menu acts on the whole selection: offer Reveal when any target has a source, even if
    // the layer under the pointer is a text, shape or null layer.
    let any_source = targets.iter().any(|id| comp.layer(LayerId(*id)).is_some_and(|l| l.source.item().is_some()));
    let caps = LayerMenuCaps { source: caps.source || any_source, ..caps };
    crate::widgets::context_menu(resp, |ui| layer_menu(ui, &targets, caps, (auto, "timeline.layerMenu"), actions));
}

/// The layer switches the layer context menus toggle (After Effects lists them after Time):
/// `layer.setSwitch` name and label.
const MENU_SWITCHES: [(&str, &str); 1] = [("guide", "Guide Layer")];

/// Shared layer actions for Timeline rows and Composition context menus. `auto` is the automation
/// registry and the id prefix of the menu's items.
pub(crate) fn layer_menu(
    ui: &mut egui::Ui,
    layers: &[u64],
    caps: LayerMenuCaps,
    auto: (&mut crate::automation::Registry, &str),
    actions: &mut Vec<(String, serde_json::Value)>,
) {
    let Some(&id) = layers.first() else { return };
    let (auto, prefix) = auto;
    let mut item = |ui: &mut egui::Ui, label: &str, cmd: &str, params: serde_json::Value| {
        if ui.button(crate::i18n::context_item(cmd, &params, label)).clicked() {
            actions.push(("layer.select".into(), json!({"layers": layers})));
            actions.push((cmd.into(), params));
            ui.close();
        }
    };
    ui.menu_button(crate::i18n::tr("New"), |ui| {
        item(ui, "Text", "layer.newText", json!({}));
        item(ui, "Solid…", "layer.newSolid", json!({}));
        item(ui, "Light…", "layer.newLight", json!({}));
        item(ui, "Camera…", "layer.newCamera", json!({}));
        item(ui, "Null Object", "layer.newNull", json!({}));
        item(ui, "Shape Layer", "layer.newShape", json!({"kind": "none"}));
        item(ui, "Adjustment Layer", "layer.newAdjustment", json!({}));
    });
    item(ui, "Layer Settings…", "layer.settings", json!({}));
    if caps.source {
        item(ui, "Reveal Layer Source in Project", "layer.revealSource", json!({"layers": layers}));
    }
    if caps.vector {
        item(ui, "Create Shapes from Vector Layer", "layer.create", json!({"op": "shapesFromVector", "layers": layers}));
    }
    if caps.text {
        item(ui, "Create Shapes from Text", "layer.create", json!({"op": "shapesFromText", "layers": layers}));
        item(ui, "Create Masks from Text", "layer.create", json!({"op": "masksFromText", "layers": layers}));
    }
    ui.separator();
    ui.menu_button(crate::i18n::tr("Masks"), |ui| {
        item(ui, "New Mask", "layer.addMask", json!({"layer": id}));
    });
    ui.menu_button(crate::i18n::tr("Transform"), |ui| {
        item(ui, "Reset", "layer.transform", json!({"layers": layers, "op": "reset"}));
        item(ui, "Center In View", "layer.transform", json!({"layers": layers, "op": "center"}));
        item(ui, "Fit to Comp", "layer.transform", json!({"layers": layers, "op": "fit"}));
        item(ui, "Flip Horizontal", "layer.transform", json!({"layers": layers, "op": "flipH"}));
        item(ui, "Flip Vertical", "layer.transform", json!({"layers": layers, "op": "flipV"}));
    });
    ui.menu_button(crate::i18n::tr("Time"), |ui| {
        item(ui, "Enable Time Remapping", "layer.enableTimeRemap", json!({"layers": layers}));
        item(ui, "Time-Reverse Layer", "layer.timeReverse", json!({"layers": layers}));
        item(ui, "Time Stretch…", "layer.timeStretch", json!({}));
        item(ui, "Freeze Frame", "layer.freezeFrame", json!({"layers": layers}));
        item(ui, "Freeze On Last Frame", "layer.freezeOnLastFrame", json!({"layers": layers}));
    });
    ui.menu_button(crate::i18n::tr("Markers"), |ui| {
        item(ui, "Add Marker", "layer.addMarker", json!({"layers": layers}));
    });
    // A click toggles the switch on the target layers, as Layer ▸ Guide Layer does.
    // (Pushed after the other items: `item` holds `actions` until its last use.)
    let mut switch_actions: Vec<(String, serde_json::Value)> = Vec::new();
    for (switch, label) in MENU_SWITCHES {
        let response = ui.button(crate::i18n::tr(label));
        auto.add(&format!("{prefix}.switch.{switch}"), response.rect, label);
        if response.clicked() {
            switch_actions.push(("layer.select".into(), json!({"layers": layers})));
            switch_actions.push(("layer.setSwitch".into(), json!({"layers": layers, "switch": switch})));
            ui.close();
        }
    }
    ui.menu_button(crate::i18n::tr("Blending Mode"), |ui| {
        widgets::menu_scroll(ui, false, |ui| {
            for m in BlendMode::ALL {
                item(ui, m.label(), "layer.setBlendMode", json!({"layers": layers, "mode": m.label()}));
                if m.ends_group() {
                    ui.separator();
                }
            }
        })
    });
    ui.menu_button(crate::i18n::tr("Layer Styles"), |ui| layer_styles_submenu(ui, layers, &mut item));
    ui.menu_button(crate::i18n::tr("Effects"), |ui| effects_submenu(ui, layers, &mut item));
    ui.menu_button(crate::i18n::tr("Arrange"), |ui| {
        item(ui, "Bring Layer to Front", "layer.arrange", json!({"layers": layers, "to": "front"}));
        item(ui, "Bring Layer Forward", "layer.arrange", json!({"layers": layers, "to": "forward"}));
        item(ui, "Send Layer Backward", "layer.arrange", json!({"layers": layers, "to": "backward"}));
        item(ui, "Send Layer to Back", "layer.arrange", json!({"layers": layers, "to": "back"}));
    });
    ui.separator();
    item(ui, "Pre-compose…", "layer.precompose", json!({"layers": layers}));
    item(ui, "Duplicate", "edit.duplicate", json!({"layers": layers}));
    item(ui, "Split Layer", "edit.splitLayer", json!({"layers": layers}));
    item(ui, "Delete", "edit.clear", json!({"layers": layers}));
    actions.extend(switch_actions);
}

/// The layer context menu's Layer Styles submenu, matching the Layer ▸ Layer Styles application menu.
fn layer_styles_submenu(ui: &mut egui::Ui, layers: &[u64], item: &mut dyn FnMut(&mut egui::Ui, &str, &str, serde_json::Value)) {
    item(ui, "Layer Style Options...", "layer.style.options", json!({}));
    ui.separator();
    item(ui, "Convert to Editable Styles", "layer.style.convertToEditable", json!({"layers": layers}));
    item(ui, "Show All", "layer.style.showAll", json!({"layers": layers}));
    item(ui, "Remove All", "layer.style.removeAll", json!({"layers": layers}));
    ui.separator();
    for (id, label) in effectcraft_engine::project::styles::STYLES {
        let cmd = match *id {
            "dropShadow" => "layer.style.dropShadow",
            "innerShadow" => "layer.style.innerShadow",
            "outerGlow" => "layer.style.outerGlow",
            "innerGlow" => "layer.style.innerGlow",
            "bevelEmboss" => "layer.style.bevelEmboss",
            "satin" => "layer.style.satin",
            "colorOverlay" => "layer.style.colorOverlay",
            "gradientOverlay" => "layer.style.gradientOverlay",
            "stroke" => "layer.style.stroke",
            _ => continue,
        };
        item(ui, label, cmd, json!({}));
    }
}

/// The layer context menu's Effects submenu: every effect from the registry grouped by category,
/// in the same order as the Effect menu's palette, so the two stay in sync automatically.
fn effects_submenu(ui: &mut egui::Ui, layers: &[u64], item: &mut dyn FnMut(&mut egui::Ui, &str, &str, serde_json::Value)) {
    let mut cats: Vec<(&str, Vec<&'static effectcraft_engine::effects::EffectSpec>)> = Vec::new();
    for e in effectcraft_engine::effects::all() {
        match cats.iter_mut().find(|(c, _)| *c == e.category) {
            Some((_, v)) => v.push(e),
            None => cats.push((e.category, vec![e])),
        }
    }
    cats.sort_by_key(|(c, _)| c.to_ascii_lowercase());
    widgets::menu_scroll(ui, false, |ui| {
        for (cat, mut fx) in cats {
            fx.sort_by_key(|e| e.name.to_ascii_lowercase());
            ui.menu_button(crate::i18n::effects::category(cat), |ui| {
                widgets::menu_scroll(ui, false, |ui| {
                    for e in fx {
                        item(ui, e.name, "effect.apply", json!({"effect": e.id, "layers": layers}));
                    }
                });
            });
        }
    });
}

fn fmt_num(v: f64, d: usize) -> String {
    format!("{v:.d$}")
}

/// What the property value editors share: where to draw, what they edit, and how a change is
/// worded as a `prop.set` command.
struct ValueEdit<'a> {
    p: &'a egui::Painter,
    layer: &'a Layer,
    prop: &'a Property,
    at: Pos2,
    uid: u64,
    merge: String,
    scrub: bool,
    proportional: bool,
}

impl<'a> ValueEdit<'a> {
    /// The editor of `prop` on `layer` at `at`; `ui` says whether a scrub is under way.
    fn new(ui: &egui::Ui, p: &'a egui::Painter, layer: &'a Layer, prop: &'a Property, at: Pos2) -> Self {
        let uid = prop.uid;
        // With several layers selected the edit applies to all of them (#491): a scrub by the same
        // change, a typed value as it is.
        let scrub = ui.ctx().dragged_id().is_some() && ui.input(|i| i.pointer.is_decidedly_dragging());
        // Proportional Scrubbing (After Effects 26.2): Ctrl+Alt (Cmd+Opt) spreads the change over the
        // selected layers by selection order, the first unchanged and the last by all of it.
        let proportional = scrub && ui.input(|i| widgets::proportional_scrub(i.modifiers));
        Self { p, layer, prop, at, uid, merge: format!("scrub-{uid}"), scrub, proportional }
    }

    /// Queues the `prop.set` of one edit.
    fn set(&self, actions: &mut Vec<(String, serde_json::Value)>, v: serde_json::Value) {
        actions.push((
            "prop.set".into(),
            json!({"layer": self.layer.id.0, "prop": self.uid, "value": v, "merge": self.merge, "selected": true, "offset": self.scrub, "proportional": self.proportional}),
        ))
    }
}

/// Inline value editor for a property row; pushes `prop.set` actions. Returns the rects of a
/// multi-dimension value's fields, one per dimension (none for other values).
fn value_editor(app: &mut EffectcraftApp, ui: &mut egui::Ui, ve: &ValueEdit<'_>, value: &Value, actions: &mut Vec<(String, serde_json::Value)>) -> Vec<Rect> {
    match value {
        Value::Scalar(_) => scalar_editor(app, ui, ve, actions, value),
        Value::Vec2(_) | Value::Vec3(_) => return vector_editor(app, ui, ve, actions, value),
        Value::Color(_) => color_editor(app, ui, ve, actions, value),
        Value::Bool(_) => bool_editor(app, ui, ve, actions, value),
        Value::Enum(_) => enum_editor(app, ui, ve, actions, value),
        Value::Layer(_) => layer_editor(app, ui, ve, actions, value),
        Value::Gradient(_) => gradient_editor(app, ui, ve, actions),
        Value::Path(_) => {
            ve.p.text(pos2(ve.at.x, ve.at.y), Align2::LEFT_CENTER, crate::i18n::tr("Shape…"), Tokens::ui(12.0), app.tokens.hot_text);
        }
        Value::Text(_) => {}
        Value::Str(s) => {
            ve.p.text(pos2(ve.at.x, ve.at.y), Align2::LEFT_CENTER, s.chars().take(30).collect::<String>(), Tokens::ui(12.0), app.tokens.text_dim);
        }
    }
    Vec::new()
}

/// A number: scrubbable hot text, or the angle field.
fn scalar_editor(app: &mut EffectcraftApp, ui: &mut egui::Ui, ve: &ValueEdit<'_>, actions: &mut Vec<(String, serde_json::Value)>, value: &Value) {
    let Value::Scalar(v) = value else { return };
    let (layer, prop, at, uid) = (ve.layer, ve.prop, ve.at, ve.uid);
    let t = app.tokens;
    let (x, y) = (at.x, at.y - 9.0);
    let (min, max, dec, speed) = match &prop.ui {
        ParamUi::Slider { min, max, decimals, .. } => (*min, *max, *decimals as usize, widgets::slider_scrub_speed(&prop.ui)),
        ParamUi::Percent => (-1e6, 1e6, 1, 0.5),
        _ => (-1e9, 1e9, 1, 1.0),
    };
    if matches!(prop.ui, ParamUi::Angle) {
        let (rr, dr, nv) = super::fx_widgets::angle_field(ui, pos2(x, y), egui::Id::unique(("v", uid, 0)), *v, 1, &t);
        app.auto.add(&format!("timeline.prop.{uid}.value"), dr, &prop.name);
        app.auto.add(&format!("timeline.prop.{uid}.revolutions"), rr, &prop.name);
        if let Some(nv) = nv {
            ve.set(actions, json!(nv));
        }
    } else {
        // A Range Selector's Start, End and Offset count characters with Units = Index (#1814).
        let pct = (matches!(prop.ui, ParamUi::Percent) || prop.match_id == "opacity") && !effectcraft_engine::render::text::in_index_units(layer, uid);
        let suffix = if pct { "%" } else { "" };
        let (r, nv, _) = widgets::hot_number_at(
            ui,
            pos2(x, y),
            egui::Id::unique(("v", uid, 0)),
            *v,
            &widgets::HotSpec { speed, range: (min, max), decimals: dec.max(1), suffix },
            &t,
        );
        app.auto.add(&format!("timeline.prop.{uid}.value"), r, &prop.name);
        if let Some(nv) = nv {
            ve.set(actions, json!(nv));
        }
    }
}

/// A multi-dimension value: one hot-text field per dimension, with the link chain. Returns their rects.
fn vector_editor(app: &mut EffectcraftApp, ui: &mut egui::Ui, ve: &ValueEdit<'_>, actions: &mut Vec<(String, serde_json::Value)>, value: &Value) -> Vec<Rect> {
    let (p, layer, prop, at, uid) = (ve.p, ve.layer, ve.prop, ve.at, ve.uid);
    let t = app.tokens;
    let (mut x, y) = (at.x, at.y - 9.0);
    let is_3d = layer.is_3d();
    let mut dims = vec![];
    let c = value.components();
    let n = if prop.shown_dims > 0 && !(is_3d && c.len() == 3) { prop.shown_dims as usize } else { c.len() };
    let pct = matches!(prop.ui, ParamUi::Percent);
    // Mask Feather: two linked values, never negative (as in After Effects, #203).
    let feather = prop.match_id == "feather" && layer.props.parent_of(uid).is_some_and(|g| matches!(g.kind, GroupKind::Mask { .. }));
    let range = if feather { (0.0, 1e9) } else { (-1e9, 1e9) };
    // Scale and Mask Feather: the chain link (Constrain Proportions, on by default).
    let linkable = feather || (pct && prop.match_id == "scale");
    let linked = linkable && !app.ui.timeline.unlinked.contains(&uid);
    if linkable {
        let lr = Rect::from_center_size(pos2(x + 7.0, at.y), vec2(14.0, 14.0));
        let resp = ui.interact(lr, egui::Id::unique(("tl-link", uid)), Sense::click()).on_hover_text(crate::i18n::tr("Constrain Proportions"));
        icons::paint(p, lr, Icon::Link, if linked { t.accent } else { t.text_faint });
        app.auto.add(&format!("timeline.prop.{uid}.link"), lr, "Constrain Proportions");
        if resp.clicked() && !app.ui.timeline.unlinked.remove(&uid) {
            app.ui.timeline.unlinked.insert(uid);
        }
        x += 18.0;
    }
    for d in 0..n.min(c.len()) {
        let suffix = if pct && d + 1 == n { "%" } else { "" };
        let (r, nv, _) = widgets::hot_number_at(
            ui,
            pos2(x, y),
            egui::Id::unique(("v", uid, d)),
            c[d],
            &widgets::HotSpec { speed: if pct { 0.5 } else { 1.0 }, range, decimals: 1, suffix },
            &t,
        );
        app.auto.add(&format!("timeline.prop.{uid}.value.{d}"), r, &prop.name);
        dims.push(r);
        if let Some(nv) = nv {
            let mut nc = c.clone();
            // Alt edits one value of a linked pair (Ctrl+Alt is a proportional scrub).
            let shown = if linked && !ui.input(|i| i.modifiers.alt && !widgets::proportional_scrub(i.modifiers)) { n.min(c.len()) } else { 1 };
            let from = if shown == 1 { d } else { 0 };
            for (e, v) in widgets::constrained(c.get(from..from + shown).unwrap_or_default(), d - from, nv).into_iter().enumerate() {
                if let Some(slot) = nc.get_mut(from + e) {
                    *slot = v;
                }
            }
            ve.set(actions, json!(nc));
            // Typed into these components only: other selected layers keep the rest of
            // their own value (#1434).
            if let Some((_, a)) = actions.last_mut() {
                a["dims"] = json!((from..from + shown).collect::<Vec<_>>());
            }
        }
        x = r.max.x + if d + 1 < n { 2.0 } else { 0.0 };
        if d + 1 < n {
            p.text(pos2(x, at.y), Align2::LEFT_CENTER, ",", Tokens::ui(12.0), t.hot_text);
            x += 6.0;
        }
    }
    dims
}

/// A colour: swatch, eyedropper and picker.
fn color_editor(app: &mut EffectcraftApp, ui: &mut egui::Ui, ve: &ValueEdit<'_>, actions: &mut Vec<(String, serde_json::Value)>, value: &Value) {
    let Value::Color(c) = value else { return };
    let (layer, prop, at, uid) = (ve.layer, ve.prop, ve.at, ve.uid);
    let t = app.tokens;
    let x = at.x;
    let r = Rect::from_min_size(pos2(x, at.y - 7.0), vec2(26.0, 14.0));
    let resp = widgets::swatch(ui, r, [c[0] as f32, c[1] as f32, c[2] as f32, 1.0], egui::Id::unique(("sw", uid)), &t);
    app.auto.add(&format!("timeline.prop.{uid}.value"), r, &prop.name);
    let pop = egui::Id::unique(("cpop", uid));
    if resp.clicked() {
        widgets::open_popup(ui, pop);
    }
    // The eyedropper after the swatch samples the Composition panel (shape Fill and
    // Stroke colours, text and effect colours), as in After Effects' Timeline (#633).
    let er = Rect::from_center_size(pos2(r.max.x + 12.0, at.y), vec2(16.0, 16.0));
    let active = app.ui.fx_pick.as_ref().is_some_and(|k| k.prop == uid && k.kind == "color");
    if super::fx_widgets::eyedropper_button(ui, er, active, egui::Id::unique(("tl-eye", uid)), &t)
        .on_hover_text(crate::i18n::tr("Eyedropper: click, then click in the Composition panel"))
        .clicked()
    {
        super::effect_controls::toggle_pick(app, "color", layer, prop);
    }
    app.auto.add(&format!("timeline.prop.{uid}.eyedropper"), er, &prop.name);
    let mut rgb = [c[0] as f32, c[1] as f32, c[2] as f32];
    if crate::widgets::color_popup(ui, pop, r.left_bottom(), &mut rgb) {
        ve.set(actions, json!([rgb[0], rgb[1], rgb[2], 1.0]));
    }
}

/// A checkbox.
fn bool_editor(app: &mut EffectcraftApp, ui: &mut egui::Ui, ve: &ValueEdit<'_>, actions: &mut Vec<(String, serde_json::Value)>, value: &Value) {
    let Value::Bool(b) = value else { return };
    let (prop, at, uid) = (ve.prop, ve.at, ve.uid);
    let t = app.tokens;
    let x = at.x;
    let r = Rect::from_min_size(pos2(x, at.y - 8.0), vec2(16.0, 16.0));
    if widgets::checkbox(ui, r, *b, &t, egui::Id::unique(("cb", uid))).clicked() {
        ve.set(actions, json!(!b));
    }
    app.auto.add(&format!("timeline.prop.{uid}.value"), r, &prop.name);
}

/// A popup (or the number when the property has no options).
fn enum_editor(app: &mut EffectcraftApp, ui: &mut egui::Ui, ve: &ValueEdit<'_>, actions: &mut Vec<(String, serde_json::Value)>, value: &Value) {
    let Value::Enum(i) = value else { return };
    let (p, layer, prop, at, uid) = (ve.p, ve.layer, ve.prop, ve.at, ve.uid);
    let t = app.tokens;
    let x = at.x;
    // Text Path Options ▸ Path lists the layer's masks.
    let mask_opts: Option<Vec<String>> = layer
        .props
        .prop("text/pathOptions/path")
        .filter(|q| q.uid == uid)
        .map(|_| std::iter::once("None".to_string()).chain(layer.masks().into_iter().flat_map(|m| m.groups().map(|g| g.name.clone()))).collect());
    let ui_opts = mask_opts.map(|options| ParamUi::Popup { options });
    if let ParamUi::Popup { options } = ui_opts.as_ref().unwrap_or(&prop.ui) {
        let r = Rect::from_min_size(pos2(x, at.y - 9.0), vec2(150.0, 18.0));
        let label = options.get(*i as usize).cloned().unwrap_or_default();
        let pop = egui::Id::unique(("epop", uid));
        if widgets::dropdown(ui, r, &label, &t, egui::Id::unique(("en", uid))).clicked() {
            widgets::open_popup(ui, pop);
        }
        app.auto.add(&format!("timeline.prop.{uid}.value"), r, &prop.name);
        if let Some(ni) = widgets::popup_menu(ui, pop, r.left_bottom(), options, Some(*i as usize)) {
            ve.set(actions, json!(ni));
        }
    } else {
        p.text(pos2(x, at.y), Align2::LEFT_CENTER, fmt_num(*i as f64, 0), Tokens::ui(12.0), t.hot_text);
    }
}

/// A layer reference picker.
fn layer_editor(app: &mut EffectcraftApp, ui: &mut egui::Ui, ve: &ValueEdit<'_>, actions: &mut Vec<(String, serde_json::Value)>, value: &Value) {
    let Value::Layer(l) = value else { return };
    let (prop, at, uid) = (ve.prop, ve.at, ve.uid);
    let t = app.tokens;
    let x = at.x;
    // Layer references (a light's Source, a Displacement Map's layer…) pick from the
    // comp's layers, as in Effect Controls; the choice is written through the same
    // undoable prop.set as the other value arms.
    let r = Rect::from_min_size(pos2(x, at.y - 9.0), vec2(150.0, 18.0));
    if let Some(comp) = app.session.active_comp_arc()
        && let Some(chosen) = widgets::layer_picker(ui, r, &comp.layers, *l, &t, egui::Id::unique(("tl-lay", uid)))
    {
        ve.set(actions, chosen.map(|id| json!(id)).unwrap_or(serde_json::Value::Null));
    }
    app.auto.add(&format!("timeline.prop.{uid}.value"), r, &prop.name);
}

/// Opens the Gradient Editor at the current time (#612).
fn gradient_editor(app: &mut EffectcraftApp, ui: &mut egui::Ui, ve: &ValueEdit<'_>, actions: &mut Vec<(String, serde_json::Value)>) {
    let (p, layer, prop, at, uid) = (ve.p, ve.layer, ve.prop, ve.at, ve.uid);
    let t = app.tokens;
    let x = at.x;
    // Opens the Gradient Editor at the current time (#612).
    let r = p.text(pos2(x, at.y), Align2::LEFT_CENTER, crate::i18n::tr("Edit Gradient…"), Tokens::ui(12.0), t.hot_text);
    let resp = ui.interact(r.expand(2.0), egui::Id::unique(("gradient-edit", uid)), Sense::click());
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    app.auto.add(&format!("timeline.prop.{uid}.value"), r, &prop.name);
    if resp.clicked() {
        actions.push(("prop.editGradient".into(), json!({"layer": layer.id.0, "prop": uid})));
    }
}

#[cfg(test)]
mod tests;
