//! The Timeline panel.
//!
//! Left: current-time display, search, comp switches, then per layer A/V features (video, audio,
//! solo, lock), label, number, name, switches (shy, collapse, quality, fx, frame blend, motion
//! blur, adjustment, 3D), mode, track matte and parent, with twirl-down property trees (stopwatch,
//! keyframe navigator, scrubbable values). Right: time navigator, ruler with work area and cache
//! bar, layer duration bars, keyframes, the CTI, and the value graph editor.

use effectcraft_engine::color::BlendMode;
use effectcraft_engine::keyframe::{Interp, Value};
use effectcraft_engine::project::{Comp, GroupKind, Layer, LayerId, LayerSource, MatteKind, Node, ParamUi, PropGroup, Property};
use effectcraft_engine::render::EvalCtx;
use effectcraft_engine::time::Tick;
use egui::{Align2, Color32, Pos2, Rect, Sense, Stroke, StrokeKind, pos2, vec2};
use serde_json::json;

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

/// A layer bar's fill: the label colour muted toward grey (a little brighter when selected).
pub(crate) fn bar_color(label: Color32, selected: bool) -> Color32 {
    let k = if selected { 0.62 } else { 0.45 };
    let g = 0x3a as f32;
    let mix = |c: u8| (c as f32 * k + g * (1.0 - k)).round() as u8;
    Color32::from_rgb(mix(label.r()), mix(label.g()), mix(label.b()))
}

/// Row height (expression editors grow with their text).
fn row_height(row: &Row, rh: f32) -> f32 {
    match row.kind {
        RowKind::Expr { lines, .. } => rh * lines.clamp(1, 8) as f32 + 6.0,
        RowKind::Waveform { .. } => rh * 3.0,
        _ => rh,
    }
}

/// Insert an expression editor row after every property that has an expression.
fn with_expr_rows(rows: Vec<Row>, comp: &Comp, closed: &std::collections::BTreeSet<u64>) -> Vec<Row> {
    let mut out = Vec::with_capacity(rows.len());
    // Rows come grouped by layer: look each layer up once (a linear search per property row
    // made twirling open many layers of a big comp quadratic).
    let mut cur: Option<&Layer> = None;
    for r in rows {
        let mut extra = None;
        if let RowKind::Prop { uid } = r.kind
            && !closed.contains(&uid)
        {
            if cur.is_none_or(|l| l.id != r.layer) {
                cur = comp.layer(r.layer);
            }
            extra = cur.and_then(|l| l.props.find(uid)).and_then(|p| p.expr.as_ref()).map(|e| Row {
                layer: r.layer,
                depth: r.depth,
                kind: RowKind::Expr { uid, lines: e.text.lines().count().max(1) },
            });
        }
        out.push(r);
        out.extend(extra);
    }
    out
}

/// A pick-whip drag in progress: (parent = 0 / property = 1, layer, prop uid, start point).
type PickWhip = (u8, u64, u64, Pos2);

fn pick_whip_id() -> egui::Id {
    egui::Id::new("tl-pickwhip")
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

fn tl_map_id() -> egui::Id {
    egui::Id::new("timeline-map")
}

/// The most a time ruler zooms in (pixels per second).
const MAX_PPS: f64 = 4000.0;

/// Pixels per second that fit `comp` in a time graph `width` points wide.
fn fit_pps(width: f32, comp: &Comp) -> f64 {
    (width - 10.0).max(1.0) as f64 / comp.duration.seconds().max(0.01)
}

/// Zoom the time ruler to `pps` pixels per second, keeping comp time `at_t` at `at_px` pixels
/// from the ruler's start. Zooming out stops at `fit`, where the whole comp shows (and the
/// ruler keeps fitting it).
fn set_zoom(tl: &mut crate::state::TimelineState, comp: &Comp, pps: f64, fit: f64, at_t: f64, at_px: f64) {
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
    let Some((_, w)) = ctx.data(|d| d.get_temp::<(f32, f32)>(egui::Id::new("timeline-graph-area"))) else { return };
    let Some(comp) = app.session.active_comp_arc() else { return };
    let fit = fit_pps(w, &comp);
    if app.ui.timeline.pps.is_some_and(|p| p > fit) {
        set_zoom(&mut app.ui.timeline, &comp, fit, fit, 0.0, 0.0);
        return;
    }
    let cti = app.session.time().seconds();
    set_zoom(&mut app.ui.timeline, &comp, MAX_PPS, fit, cti, w as f64 / 2.0);
}

/// Zoom the time ruler around the CTI.
pub fn zoom(app: &mut EffectcraftApp, ctx: &egui::Context, k: f64) {
    let Some((_, w)) = ctx.data(|d| d.get_temp::<(f32, f32)>(egui::Id::new("timeline-graph-area"))) else { return };
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
    if let Some(l) = app.session.state.selected_layers.first().and_then(|id| app.session.active_comp().and_then(|c| c.layer(*id))) {
        start_rename(ctx, l.id.0, &l.name);
    }
}

/// Open the rename field on layer `id`; it takes the keyboard focus on its first frame only.
fn start_rename(ctx: &egui::Context, id: u64, name: &str) {
    ctx.data_mut(|d| {
        d.insert_temp(egui::Id::new("tl-rename"), (id, name.to_string()));
        d.insert_temp(rename_focus_id(), true);
    });
}

fn rename_focus_id() -> egui::Id {
    egui::Id::new("tl-rename-focus")
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
    fn group_row(l: &Layer, g: &PropGroup, depth: usize, parent: &PropGroup) -> Row {
        let fx = matches!(g.kind, GroupKind::Effect { .. }).then_some(g.enabled);
        let eye =
            (parent.match_id == effectcraft_engine::project::styles::GROUP && g.match_id != effectcraft_engine::project::styles::BLENDING).then_some(g.enabled);
        Row { layer: l.id, depth, kind: RowKind::Group { uid: g.uid, name: g.name.clone(), open: true, has_children: !g.children.is_empty(), fx, eye } }
    }
    fn all(l: &Layer, g: &PropGroup, depth: usize, out: &mut Vec<Row>) {
        for c in &g.children {
            match c {
                Node::Group(sg) => {
                    out.push(group_row(l, sg, depth, g));
                    all(l, sg, depth + 1, out);
                }
                Node::Prop(p) if prop_visible(p, l) => out.push(Row { layer: l.id, depth, kind: RowKind::Prop { uid: p.uid } }),
                _ => {}
            }
        }
    }
    fn rec(l: &Layer, g: &PropGroup, depth: usize, q: &str, out: &mut Vec<Row>) {
        for c in &g.children {
            match c {
                Node::Group(sg) if depth > 1 || group_visible(sg, l) => {
                    if sg.name.to_lowercase().contains(q) {
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
        for c in &l.props.children {
            match c {
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
    let group_rows = |rows: &mut Vec<Row>, g: &PropGroup, fx: bool| {
        for g in g.groups() {
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
    if !wanted.is_empty() {
        let mut found = vec![];
        collect_props(&l.props, &mut found, &|p| wanted.contains(&p.uid));
        rows.extend(found.into_iter().map(|uid| Row { layer: l.id, depth: 1, kind: RowKind::Prop { uid } }));
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
        }
        let mut found = vec![];
        collect_props(&l.props, &mut found, &|p| prop_visible(p, l) && reveal_props.contains(&p.uid) && !wanted.contains(&p.uid));
        rows.extend(found.into_iter().map(|uid| Row { layer: l.id, depth: 1, kind: RowKind::Prop { uid } }));
    }
    if has("waveform")
        && let Some(item) = super::waveform::audio_item(&app.session.project, l)
    {
        rows.push(Row { layer: l.id, depth: 1, kind: RowKind::Waveform { item: item.0 } });
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

fn push_group(rows: &mut Vec<Row>, l: &Layer, g: &PropGroup, depth: usize, open: &std::collections::BTreeSet<u64>) {
    for c in &g.children {
        match c {
            // Text animators list their selectors and properties directly (as in AE).
            Node::Group(sg) if g.match_id == "animator" && matches!(sg.match_id.as_str(), "selectors" | "properties") => {
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
    egui::Id::new("tl-layer-drag")
}

/// A keyframe drag in progress: where the pointer started, the grabbed key's comp time
/// (seconds) and how many frames the selected keys have moved so far.
#[derive(Clone, Copy, Debug)]
struct KeyDrag {
    origin_x: f32,
    anchor: f64,
    applied: i64,
}

fn key_drag_id() -> egui::Id {
    egui::Id::new("tl-key-drag")
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

fn layer_drop_id() -> egui::Id {
    egui::Id::new("tl-layer-drop")
}

pub fn show(app: &mut EffectcraftApp, ui: &mut egui::Ui, rect: Rect) {
    let t = app.tokens;
    let ctx = ui.ctx().clone();
    let Some(cid) = app.session.active_comp_id() else {
        ui.painter().text(rect.center(), Align2::CENTER_CENTER, "(no composition)", Tokens::ui(12.0), t.text_faint);
        return;
    };
    let Some(comp) = app.session.project.comp_arc(cid) else { return };
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
    let graph_x1 = rect.max.x - 10.0;
    ctx.data_mut(|d| d.insert_temp(egui::Id::new("timeline-graph-area"), (graph_x0, graph_x1 - graph_x0)));
    let fit_pps = fit_pps(graph_x1 - graph_x0, &comp);
    let pps = app.ui.timeline.pps.unwrap_or(fit_pps);
    if app.ui.timeline.pps.is_none() {
        app.ui.timeline.start = 0.0;
    }
    let tm = TMap { x0: graph_x0 + 6.0, start: app.ui.timeline.start, pps };
    ctx.data_mut(|d| d.insert_temp(tl_map_id(), (tm.x0, tm.start, tm.pps)));
    let top = rect.min.y;
    let rows_top = top + header_h + colhdr_h;
    let rows_rect = Rect::from_min_max(pos2(rect.min.x, rows_top), pos2(rect.max.x, rect.max.y - footer_h));
    let graph_rect = Rect::from_min_max(pos2(graph_x0, top + header_h - ruler_h + colhdr_h), pos2(graph_x1 + 10.0, rect.max.y - footer_h));
    p.rect_filled(Rect::from_min_max(pos2(graph_x0, top), rect.max), 0.0, t.tl_bg);
    p.line_segment([pos2(graph_x0 - 1.0, top), pos2(graph_x0 - 1.0, rect.max.y)], Stroke::new(1.0, t.app_bg));

    let mut actions: Vec<(String, serde_json::Value)> = Vec::new();
    // ---- header (left): time display, search, switches.
    let tc = crate::panels::timecode(&app.session, &comp, time);
    let tc_rect = Rect::from_min_size(pos2(rect.min.x + 12.0, top + 6.0), vec2(150.0, 24.0));
    let tc_resp = ui.interact(tc_rect, egui::Id::new("tl-timecode"), Sense::click_and_drag());
    p.text(tc_rect.left_center(), Align2::LEFT_CENTER, &tc, Tokens::semibold(16.0), t.timecode);
    let sub = format!("{:05} ({:.2} fps)", fr.frame_at(time) + app.session.project.settings.frame_start, fr.as_f64());
    p.text(pos2(tc_rect.min.x, tc_rect.max.y + 8.0), Align2::LEFT_CENTER, sub, Tokens::ui(10.5), t.text_faint);
    app.auto.add("timeline.timecode", tc_rect, &tc);
    if tc_resp.dragged() {
        let d = tc_resp.drag_delta().x as f64;
        let f = fr.frame_at(time) + (d * 0.5).round() as i64;
        let _ = app.session.execute("time.set", json!({"frame": f.max(0)}));
    }
    if tc_resp.double_clicked() {
        ctx.data_mut(|d| d.insert_temp(egui::Id::new("tl-tc-edit"), tc.clone()));
    }
    if let Some(mut buf) = ctx.data(|d| d.get_temp::<String>(egui::Id::new("tl-tc-edit"))) {
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(tc_rect));
        let r = child.add(egui::TextEdit::singleline(&mut buf).font(Tokens::semibold(16.0)).desired_width(150.0));
        if r.lost_focus() {
            if !ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                let _ = app.session.execute("time.set", json!({"timecode": buf}));
            }
            ctx.data_mut(|d| d.remove::<String>(egui::Id::new("tl-tc-edit")));
        } else {
            widgets::keep_focus(&ctx, &r, &buf);
            ctx.data_mut(|d| d.insert_temp(egui::Id::new("tl-tc-edit"), buf));
        }
    }
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
        let resp = widgets::icon_button(ui, r, icon, on, &t, egui::Id::new(("tl-sw", id))).on_hover_text(tip);
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

    // ---- ruler (right).
    let ruler = Rect::from_min_max(pos2(graph_x0, top + header_h - ruler_h), pos2(rect.max.x, top + header_h));
    let nav = Rect::from_min_max(pos2(graph_x0 + 6.0, top + 6.0), pos2(graph_x1, top + 14.0));
    // Time navigator: the visible span as a grey bar with blue end handles (as in After
    // Effects), over a dark track.
    p.rect_filled(nav, 1.0, t.field_bg);
    let vis0 = (tm.start / comp.duration.seconds()) as f32;
    let vis1 = (tm.t(graph_x1) / comp.duration.seconds()) as f32;
    let nav_vis =
        Rect::from_min_max(pos2(nav.min.x + nav.width() * vis0.clamp(0.0, 1.0), nav.min.y), pos2(nav.min.x + nav.width() * vis1.clamp(0.0, 1.0), nav.max.y));
    p.rect_filled(nav_vis, 0.0, t.work_area);
    for x in [nav_vis.min.x, nav_vis.max.x] {
        p.rect_filled(Rect::from_min_max(pos2(x - 3.0, nav.min.y - 1.0), pos2(x + 3.0, nav.max.y + 1.0)), 3.0, t.accent);
    }
    // The middle scrolls at the current zoom; each end is a handle that moves that side of the
    // visible span (pulled in: zoomed in; stretched to the whole track: the whole comp).
    let nresp = ui.interact(nav, egui::Id::new("tl-nav"), Sense::drag());
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
        let hresp = ui.interact(hr, egui::Id::new(("tl-nav", end)), Sense::drag());
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
    p.rect_filled(ruler, 0.0, t.tl_ruler_bg);
    app.auto.add("timeline.ruler", ruler, &format!("{},{}", tm.start, tm.pps));
    // Work area bar: below the ruler, beside the column headers (After Effects' layout), with
    // blue begin / end handles; the cache bar runs under it.
    let wa_row = Rect::from_min_max(pos2(graph_x0, top + header_h), pos2(rect.max.x, top + header_h + colhdr_h));
    let wa = Rect::from_min_max(pos2(tm.x(comp.work_area.0.seconds()), wa_row.min.y + 3.0), pos2(tm.x(comp.work_area.1.seconds()), wa_row.max.y - 6.0));
    p.with_clip_rect(wa_row).rect_filled(wa, 0.0, t.work_area);
    for (hx, set) in [(wa.min.x, "begin"), (wa.max.x, "end")] {
        let hr = Rect::from_center_size(pos2(hx, wa.center().y), vec2(8.0, 14.0));
        let handle = if set == "begin" {
            Rect::from_min_max(pos2(hx - 1.0, wa.min.y), pos2(hx + 4.0, wa.max.y))
        } else {
            Rect::from_min_max(pos2(hx - 4.0, wa.min.y), pos2(hx + 1.0, wa.max.y))
        };
        p.rect_filled(handle, 2.0, t.accent);
        let resp = ui.interact(hr, egui::Id::new(("wa", set)), Sense::drag());
        app.auto.add(&format!("timeline.workArea.{set}"), hr, set);
        if resp.dragged()
            && let Some(pt) = resp.interact_pointer_pos()
        {
            let secs = fr.snap_nearest(Tick::from_seconds_f64(tm.t(pt.x).max(0.0))).seconds();
            let key = if set == "begin" { "start" } else { "end" };
            let _ = app.session.execute("comp.workArea", json!({key: secs, "merge": "wa-drag"}));
        }
    }
    // Cache bar (green: cached frames of what the viewer shows: its resolution, view and options).
    let series = app.shown_series(cid);
    let cached = app.frames.cached_frames(&series);
    let fd = comp.frame_duration().seconds();
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
        let disk = app.frames.disk_frames(&app.render_source(), &series, frames, &opts);
        let draw_blue = |a: i64, b: i64| {
            let x0 = tm.x(a as f64 * fd);
            let x1 = tm.x((b + 1) as f64 * fd);
            p.rect_filled(Rect::from_min_max(pos2(x0, cy0), pos2(x1, cy0 + 2.5)), 0.0, t.cache_blue);
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
    let pr = p.with_clip_rect(Rect::from_min_max(pos2(graph_x0, ruler.min.y), ruler.max));
    while s <= tm.t(graph_x1) + secs_per_label {
        let x = tm.x(s);
        pr.line_segment([pos2(x, ruler.max.y - 10.0), pos2(x, ruler.max.y)], Stroke::new(1.0, t.tl_ruler_tick));
        let fno = (s / fd).round() as i64;
        let label = if fno >= 60 * fps_i * 60 {
            format!("{}:{:02}:{:02}f", fno / (fps_i * 60), (fno / fps_i) % 60, fno % fps_i)
        } else {
            format!("{:02}:{:02}f", fno / fps_i, fno % fps_i)
        };
        pr.text(pos2(x + 3.0, ruler.max.y - 16.0), Align2::LEFT_CENTER, label, Tokens::ui(10.0), t.tl_ruler_text);
        for k in 1..5 {
            let xx = tm.x(s + secs_per_label * k as f64 / 5.0);
            pr.line_segment([pos2(xx, ruler.max.y - 4.0), pos2(xx, ruler.max.y)], Stroke::new(1.0, t.tl_ruler_tick.gamma_multiply(0.6)));
        }
        s += secs_per_label;
    }
    // Scrub in the ruler.
    let rresp = ui.interact(Rect::from_min_max(pos2(graph_x0, ruler.min.y + 12.0), ruler.max), egui::Id::new("tl-ruler"), Sense::click_and_drag());
    if (rresp.dragged() || rresp.clicked())
        && let Some(pt) = rresp.interact_pointer_pos()
    {
        let mut secs = tm.t(pt.x).max(0.0);
        // Shift-drag: snap to keyframes, in/out points, markers and the work area (8 px).
        if ui.input(|i| i.modifiers.shift) {
            let cands = snap_candidates(&comp, &build_rows(app, &comp));
            secs = snap_time(&cands, secs, 8.0 / pps.max(1e-6));
        }
        let tt = Tick::from_seconds_f64(secs);
        app.session.set_time(tt);
        app.stop();
        // Ctrl/Cmd-drag scrubs the audio too (After Effects).
        if rresp.dragged()
            && ui.input(|i| i.modifiers.command)
            && let Some(cid) = app.session.active_comp_id()
        {
            let now = ui.input(|i| i.time);
            let t = app.session.time();
            app.scrub_audio(cid, t, now);
        }
    }

    // ---- column headers (right-click: show/hide columns; click the name header: Source/Layer Name).
    let ch = Rect::from_min_max(pos2(rect.min.x, top + header_h), pos2(rect.max.x, top + header_h + colhdr_h));
    p.rect_filled(Rect::from_min_max(ch.min, pos2(graph_x0 - 1.0, ch.max.y)), 0.0, t.panel_bg);
    p.line_segment([pos2(rect.min.x, ch.max.y), pos2(rect.max.x, ch.max.y)], Stroke::new(1.0, t.separator));
    let hy = ch.center().y;
    // Column headers scroll with the outline and never spill into the time ruler.
    let hp = p.with_clip_rect(Rect::from_min_max(ch.min, pos2(graph_x0 - 1.0, ch.max.y)));
    let hic = |icon: Icon, x: f32| icons::paint(&hp, Rect::from_center_size(pos2(x, hy), vec2(12.0, 12.0)), icon, t.text_dim);
    let htext = |x: f32, s: &str| hp.text(pos2(x, hy), Align2::LEFT_CENTER, s, Tokens::ui(11.0), t.text_dim);
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
    let hdr_rect = Rect::from_min_max(ch.min, pos2(graph_x0 - 1.0, ch.max.y));
    let hresp = ui.interact(hdr_rect, egui::Id::new("tl-colhdr"), Sense::click());
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
        && column_at(&cw, pt.x) == Some("name")
    {
        app.ui.timeline.source_name = !app.ui.timeline.source_name;
    }
    if hresp.secondary_clicked() {
        let under = hresp.interact_pointer_pos().and_then(|pt| column_at(&cw, pt.x));
        ctx.data_mut(|d| d.insert_temp(egui::Id::new("tl-colhdr-under"), under.unwrap_or("")));
    }
    hresp.context_menu(|ui| {
        let under: &str = ctx.data(|d| d.get_temp(egui::Id::new("tl-colhdr-under"))).unwrap_or("");
        if !under.is_empty() && under != "name" && ui.button("Hide This").clicked() {
            actions.push(("timeline.column".into(), json!({"column": under, "visible": false})));
            ui.close();
        }
        ui.menu_button("Columns", |ui| {
            for (id, label) in COLUMNS {
                let label = if id == "name" { if app.ui.timeline.source_name { "Source Name" } else { "Layer Name" } } else { label };
                let on = column_visible(&app.ui.timeline, id);
                if ui.add_enabled(id != "name", egui::Button::new(label).selected(on)).clicked() {
                    actions.push(("timeline.column".into(), json!({"column": id, "visible": !on})));
                    ui.close();
                }
            }
        });
        if ui.button(if app.ui.timeline.source_name { "Show Layer Name" } else { "Show Source Name" }).clicked() {
            actions.push(("timeline.sourceName".into(), json!({})));
            ui.close();
        }
    });
    let _ = cw.end;

    // ---- rows.
    let rows = with_expr_rows(build_rows(app, &comp), &comp, &app.ui.timeline.expr_closed);
    let rh = t.row_h;
    let total_h: f32 = rows.iter().map(|r| row_height(r, rh)).sum();
    let max_scroll = (total_h - rows_rect.height() + rh).max(0.0);
    if ui.rect_contains_pointer(rows_rect) {
        let (dy, dx, zoom) = ui.input(|i| (i.smooth_scroll_delta.y, i.smooth_scroll_delta.x, i.modifiers.alt));
        if zoom && dy.abs() > 0.0 {
            // Alt+wheel zooms around the pointer, out until the whole comp shows.
            let at_x = ui.input(|i| i.pointer.hover_pos()).map(|p| p.x).unwrap_or(graph_x0);
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
    app.ui.timeline.scroll_y = app.ui.timeline.scroll_y.min(max_scroll);
    let snap_project = app.session.project.clone();
    let snap_expr = app.session.expr.clone();
    let ectx = EvalCtx { project: &snap_project, comp_id: cid, comp: &comp, time, expr: snap_expr.as_deref(), footage: None };
    let lp = p.with_clip_rect(rows_rect.intersect(Rect::from_min_max(rows_rect.min, pos2(graph_x0 - 1.0, rows_rect.max.y))));
    let gp = p.with_clip_rect(rows_rect.intersect(Rect::from_min_max(pos2(graph_x0, rows_rect.min.y), rows_rect.max)));
    let mut y = rows_rect.min.y - app.ui.timeline.scroll_y;
    let selected = app.session.state.selected_layers.clone();
    let sel_keys = app.session.state.selected_keys.clone();
    let mut ui_actions: Vec<UiAct> = Vec::new();
    let idx_of = |id: LayerId| comp.index_of(id).unwrap_or(0);
    let graph_on = app.ui.timeline.graph_editor;
    let full_clip = ui.clip_rect();
    let left_clip = full_clip.intersect(Rect::from_min_max(pos2(rect.min.x, rows_rect.min.y), pos2(graph_x0 - 1.0, rows_rect.max.y)));
    let right_clip = full_clip.intersect(Rect::from_min_max(pos2(graph_x0, rows_rect.min.y), rows_rect.max));
    // Registered before the rows so keys, bars and editors on top of it get the pointer first.
    let empty_rect = if graph_on { Rect::NOTHING } else { Rect::from_min_max(pos2(graph_x0, rows_rect.min.y), rows_rect.max) };
    let empty = ui.interact(empty_rect, egui::Id::new("tl-graph-bg"), Sense::click_and_drag());
    let outline_rect = Rect::from_min_max(rows_rect.min, pos2(graph_x0 - 1.0, rows_rect.max.y));
    let outline_empty = ui.interact(outline_rect, egui::Id::new("tl-layer-bg"), Sense::click_and_drag());
    app.auto.add("timeline.layerMarquee", outline_rect, "Drag empty outline to select layers");
    let mut hit_rows: Vec<(Rect, Row)> = vec![];
    for (ri, row) in rows.iter().enumerate() {
        // Outline widgets never spill into the time graph (narrow timelines).
        ui.set_clip_rect(left_clip);
        let hh = row_height(row, rh);
        let r = Rect::from_min_size(pos2(rect.min.x, y), vec2(rect.width(), hh));
        y += hh;
        if r.max.y < rows_rect.min.y || r.min.y > rows_rect.max.y {
            continue;
        }
        hit_rows.push((r, row.clone()));
        let Some(layer) = comp.layer(row.layer) else { continue };
        let is_sel = selected.contains(&layer.id);
        let left = Rect::from_min_max(r.min, pos2(graph_x0 - 1.0, r.max.y));
        let cy = r.min.y + rh / 2.0;
        match &row.kind {
            RowKind::Layer => {
                // After Effects draws layer rows as one flat tone separated by dark hairlines
                // (outline and time graph alike), not as alternating stripes.
                let _ = ri;
                lp.rect_filled(left, 0.0, if is_sel { t.row_selected } else { t.row_alt });
                gp.rect_filled(Rect::from_min_max(pos2(graph_x0, r.min.y), r.max), 0.0, if is_sel { t.row_selected } else { t.tl_bg });
                lp.line_segment([pos2(rect.min.x, r.max.y - 0.5), pos2(graph_x0, r.max.y - 0.5)], Stroke::new(1.0, t.app_bg));
                gp.line_segment([pos2(graph_x0, r.max.y - 0.5), pos2(r.max.x, r.max.y - 0.5)], Stroke::new(1.0, t.app_bg));
                // A/V features.
                let sw = &layer.switches;
                let av_items: [(Icon, bool, &str, bool); 4] = [
                    (Icon::Eye, sw.video, "video", layer.source.is_av()),
                    (Icon::Speaker, sw.audio, "audio", layer.props.sub("audio").is_some()),
                    (Icon::Solo, sw.solo, "solo", layer.source.is_av()),
                    (Icon::Lock, sw.locked, "lock", true),
                ];
                for (i, (icon, on, name, applicable)) in av_items.into_iter().enumerate() {
                    let br = Rect::from_center_size(pos2(cw.av + 10.0 + 18.0 * i as f32, cy), vec2(15.0, 15.0));
                    if !applicable || !vis.av {
                        continue;
                    }
                    lp.rect_filled(br.shrink(0.5), 1.0, t.field_bg);
                    let resp = widgets::icon_toggle(ui, br, icon, on, &t, egui::Id::new(("av", layer.id.0, i)), None);
                    app.auto.add(&format!("timeline.layer.{}.{name}", layer.id.0), br, name);
                    if resp.clicked() {
                        actions.push(("layer.setSwitch".into(), json!({"layers": [layer.id.0], "switch": name})));
                    }
                }
                // Label swatch.
                if vis.label {
                    let lr = Rect::from_center_size(pos2(cw.label + 10.0, cy), vec2(12.0, 12.0));
                    lp.rect_filled(lr, 2.0, t.label(layer.label));
                    let lresp = ui.interact(lr, egui::Id::new(("label", layer.id.0)), Sense::click());
                    app.auto.add(&format!("timeline.layer.{}.label", layer.id.0), lr, layer.label.name());
                    lresp.context_menu(|ui| {
                        for lab in effectcraft_engine::color::Label::ALL {
                            let name = if lab == effectcraft_engine::color::Label::None { lab.name().to_string() } else { app.session.prefs.label_name(lab) };
                            if ui.button(name).clicked() {
                                actions.push(("edit.label".into(), json!({"layers": [layer.id.0], "label": lab.name()})));
                                ui.close();
                            }
                        }
                    });
                }
                if vis.num {
                    lp.text(pos2(cw.num + 13.0, cy), Align2::CENTER_CENTER, format!("{}", idx_of(layer.id)), Tokens::ui(11.5), t.text_dim);
                }
                layer_cells(app, ui, &lp, &cw, &comp, layer, r, cy, &mut actions);
                // Row click → select; double-click → rename; drag → reorder. Registered before the
                // twirl and the name field so those sit on top and get their clicks.
                let row_x0 = if vis.num { cw.num } else { cw.name + 16.0 };
                let row_resp = ui.interact(
                    Rect::from_min_max(pos2(row_x0, r.min.y), pos2(cw.name_end, r.max.y)),
                    egui::Id::new(("row", layer.id.0)),
                    Sense::click_and_drag(),
                );
                // Twirl + icon + name.
                let tw = Rect::from_center_size(pos2(cw.name + 8.0, cy), vec2(14.0, 14.0));
                let open = app.ui.timeline.open_layers.contains(&layer.id.0);
                if widgets::twirl(ui, tw, open, egui::Id::new(("twirl", layer.id.0)), &t).clicked() {
                    ui_actions.push(UiAct::ToggleLayer(layer.id.0, ui.input(|i| i.modifiers.alt || i.modifiers.command)));
                }
                app.auto.add(&format!("timeline.layer.{}.twirl", layer.id.0), tw, "twirl");
                icons::paint(&lp, Rect::from_center_size(pos2(cw.name + 24.0, cy), vec2(13.0, 13.0)), layer_icon(layer, &app.session.project), t.text_dim);
                let name_rect = Rect::from_min_max(pos2(cw.name + 34.0, r.min.y), pos2(cw.name_end - 4.0, r.max.y));
                let rename_id = egui::Id::new("tl-rename");
                let renaming: Option<(u64, String)> = ctx.data(|d| d.get_temp(rename_id));
                if let Some((rl, mut buf)) = renaming.filter(|(rl, _)| *rl == layer.id.0) {
                    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(name_rect.shrink2(vec2(0.0, 2.0))));
                    let er = child.add(egui::TextEdit::singleline(&mut buf).font(Tokens::ui(12.0)).desired_width(name_rect.width()));
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
                    // Keep the name cell and graph selection in the chosen theme.
                    let name_col = t.text;
                    if is_sel {
                        let g = lp.layout_no_wrap(display_name(app, layer), Tokens::ui(12.0), name_col);
                        let cell = Rect::from_min_max(
                            pos2(name_rect.min.x - 3.0, r.min.y + 1.0),
                            pos2((name_rect.min.x + g.size().x + 4.0).max(cw.name_end - 4.0).min(cw.name_end - 2.0), r.max.y - 1.0),
                        );
                        lp.rect_filled(cell, 0.0, t.row_selected);
                    }
                    let lpn = lp.with_clip_rect(name_rect.intersect(lp.clip_rect()));
                    lpn.text(pos2(name_rect.min.x, cy), Align2::LEFT_CENTER, display_name(app, layer), Tokens::ui(12.0), name_col);
                }
                // Locked layers can't be selected or moved (unlock them with the lock switch).
                let locked = layer.switches.locked;
                if row_resp.drag_started() && !locked {
                    // The selected layers move together; an unselected row moves alone.
                    let moving: Vec<u64> =
                        if is_sel { comp.layers.iter().filter(|l| selected.contains(&l.id)).map(|l| l.id.0).collect() } else { vec![layer.id.0] };
                    ctx.data_mut(|d| d.insert_temp(layer_drag_id(), moving));
                }
                if row_resp.drag_stopped() {
                    ctx.data_mut(|d| d.insert_temp(layer_drop_id(), true));
                }
                app.auto.add(&format!("timeline.layer.{}.row", layer.id.0), left, &layer.name);
                if row_resp.clicked() && !locked {
                    let m = ui.input(|i| i.modifiers);
                    let anchor_key = egui::Id::new(("timeline-selection-anchor", cid.0));
                    let anchor = ctx.data(|d| d.get_temp::<LayerId>(anchor_key)).or_else(|| selected.first().copied());
                    let range = if m.shift {
                        anchor.and_then(|a| comp.layers.iter().position(|l| l.id == a)).map(|a| {
                            let b = comp.layers.iter().position(|l| l.id == layer.id).unwrap_or(a);
                            comp.layers
                                .get(a.min(b)..=a.max(b))
                                .unwrap_or_default()
                                .iter()
                                .filter(|l| !l.switches.locked && rows.iter().any(|r| r.layer == l.id))
                                .map(|l| l.id.0)
                                .collect::<Vec<_>>()
                        })
                    } else {
                        None
                    };
                    if let Some(layers) = range {
                        actions.push(("layer.select".into(), json!({"layers": layers})));
                    } else {
                        actions.push(("layer.select".into(), json!({"layers": [layer.id.0], "toggle": m.command})));
                        ctx.data_mut(|d| d.insert_temp(anchor_key, layer.id));
                    }
                }
                if row_resp.double_clicked() {
                    match &layer.source {
                        LayerSource::Comp { item } => actions.push(("comp.open".into(), json!({"comp": item.0}))),
                        // Footage and solids open in the Layer panel (paint happens there).
                        LayerSource::Footage { .. } | LayerSource::Solid { .. } => {
                            actions.push(("layer.openLayer".into(), json!({"layer": layer.id.0})));
                        }
                        _ => start_rename(&ctx, layer.id.0, &layer.name),
                    }
                }
                layer_context_menu(&row_resp, layer, &mut actions);
                // Switches.
                let sws: [(Icon, bool, &str, bool); 8] = [
                    (Icon::Shy, sw.shy, "shy", true),
                    (Icon::Collapse, sw.collapse, "collapse", matches!(layer.source, LayerSource::Comp { .. } | LayerSource::Shape | LayerSource::Text)),
                    (Icon::Quality, sw.quality == effectcraft_engine::project::Quality::Best, "quality", layer.source.is_av()),
                    (Icon::Fx, sw.effects, "fx", layer.effects().is_some_and(|f| !f.children.is_empty())),
                    (
                        Icon::FrameBlend,
                        sw.frame_blend != effectcraft_engine::project::FrameBlend::Off,
                        "frameBlend",
                        matches!(layer.source, LayerSource::Footage { .. } | LayerSource::Comp { .. }),
                    ),
                    (Icon::MotionBlur, sw.motion_blur, "motionBlur", layer.source.is_av()),
                    (Icon::Adjustment, sw.adjustment, "adjustment", layer.source.is_av()),
                    (Icon::Cube, layer.is_3d(), "threeD", layer.source.is_av() || matches!(layer.source, LayerSource::Null)),
                ];
                for (i, (icon, on, name, applicable)) in sws.into_iter().enumerate() {
                    if !applicable || !vis.switches {
                        continue;
                    }
                    let br = Rect::from_center_size(pos2(cw.switches + SW * i as f32 + SW / 2.0, cy), vec2(16.0, 16.0));
                    lp.rect_filled(br.shrink(1.0), 1.0, t.field_bg);
                    let resp = widgets::icon_toggle(ui, br, icon, on, &t, egui::Id::new(("sw", layer.id.0, i)), None);
                    app.auto.add(&format!("timeline.layer.{}.switch.{name}", layer.id.0), br, name);
                    if resp.clicked() {
                        actions.push(("layer.setSwitch".into(), json!({"layers": [layer.id.0], "switch": name})));
                    }
                }
                // Modes.
                if app.ui.timeline.show_modes && layer.source.is_av() {
                    let mr = Rect::from_min_size(pos2(cw.mode + 2.0, cy - 9.0), vec2(80.0, 18.0));
                    let mresp = widgets::dropdown(ui, mr, layer.blend_mode.label(), &t, egui::Id::new(("mode", layer.id.0)));
                    app.auto.add(&format!("timeline.layer.{}.mode", layer.id.0), mr, "Mode");
                    let pop = egui::Id::new(("mode-pop", layer.id.0));
                    if mresp.clicked() {
                        widgets::open_popup(ui, pop);
                    }
                    let opts: Vec<String> = BlendMode::ALL
                        .iter()
                        .flat_map(|m| if m.ends_group() { vec![m.label().to_string(), "-".to_string()] } else { vec![m.label().to_string()] })
                        .collect();
                    if let Some(i) = widgets::popup_menu(ui, pop, mr.left_bottom(), &opts, opts.iter().position(|o| o == layer.blend_mode.label())) {
                        actions.push(("layer.setBlendMode".into(), json!({"layers": [layer.id.0], "mode": opts[i]})));
                    }
                    let tr = Rect::from_center_size(pos2(cw.mode + 76.0 + 6.0, cy), vec2(14.0, 14.0));
                    if widgets::checkbox(ui, tr, layer.preserve_transparency, &t, egui::Id::new(("pt", layer.id.0))).clicked() {
                        actions.push(("layer.setSwitch".into(), json!({"layers": [layer.id.0], "switch": "preserveTransparency"})));
                    }
                    // Track matte: layer picker + kind.
                    let mr = Rect::from_min_size(pos2(cw.trkmat + 2.0, cy - 9.0), vec2(104.0, 18.0));
                    let label = match layer.track_matte {
                        Some(m) => comp.layer(m.layer).map(|l| format!("{} {}", idx_of(l.id), l.name)).unwrap_or("None".into()),
                        None => "No Track Matte".into(),
                    };
                    let tresp = widgets::dropdown(ui, mr, &label, &t, egui::Id::new(("trkmat", layer.id.0)));
                    app.auto.add(&format!("timeline.layer.{}.trackMatte", layer.id.0), mr, "Track Matte");
                    let pop = egui::Id::new(("trk-pop", layer.id.0));
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
                    if let Some(i) = widgets::popup_menu(ui, pop, mr.left_bottom(), &opts, None) {
                        if i == 0 {
                            actions.push(("layer.setTrackMatte".into(), json!({"layer": layer.id.0, "matte": null})));
                        } else if i >= 2 && i < 2 + candidates.len() {
                            let kind = layer.track_matte.map(|m| m.kind).unwrap_or_default();
                            actions
                                .push(("layer.setTrackMatte".into(), json!({"layer": layer.id.0, "matte": candidates[i - 2].id.0, "kind": kind_name(kind)})));
                        } else if let Some(k) = MatteKind::ALL.into_iter().find(|k| k.label() == opts[i])
                            && let Some(m) = layer.track_matte
                        {
                            actions.push(("layer.setTrackMatte".into(), json!({"layer": layer.id.0, "matte": m.layer.0, "kind": kind_name(k)})));
                        }
                    }
                    if layer.track_matte.is_some() {
                        let kr = Rect::from_center_size(pos2(mr.max.x - 26.0, cy), vec2(10.0, 10.0));
                        let _ = kr;
                    }
                }
                // Parent.
                if vis.parent {
                    let pw_rect = Rect::from_center_size(pos2(cw.parent + 9.0, cy), vec2(15.0, 15.0));
                    let pw =
                        ui.interact(pw_rect, egui::Id::new(("parent-whip", layer.id.0)), Sense::drag()).on_hover_text("Parent pick whip: drag onto a layer");
                    icons::paint(&lp, pw_rect.shrink(1.0), Icon::PickWhip, if pw.hovered() || pw.dragged() { t.text } else { t.text_dim });
                    app.auto.add(&format!("timeline.layer.{}.pickWhip", layer.id.0), pw_rect, "Parent pick whip");
                    if pw.drag_started() {
                        let pw_state: PickWhip = (0, layer.id.0, 0, pw_rect.center());
                        ctx.data_mut(|d| d.insert_temp(pick_whip_id(), pw_state));
                    }
                    let pr_rect = Rect::from_min_size(pos2(cw.parent + 20.0, cy - 9.0), vec2(94.0, 18.0));
                    let plabel = layer.parent.and_then(|p| comp.layer(p)).map(|l| format!("{}. {}", idx_of(l.id), l.name)).unwrap_or_else(|| "None".into());
                    let presp = widgets::dropdown(ui, pr_rect, &plabel, &t, egui::Id::new(("parent", layer.id.0)));
                    app.auto.add(&format!("timeline.layer.{}.parent", layer.id.0), pr_rect, "Parent");
                    let pop = egui::Id::new(("par-pop", layer.id.0));
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
                // Layer bar.
                ui.set_clip_rect(right_clip);
                if !graph_on {
                    let x_in = tm.x(layer.in_point.seconds());
                    let x_out = tm.x(layer.out_point.seconds());
                    let bar = Rect::from_min_max(pos2(x_in, r.min.y + 1.0), pos2(x_out, r.max.y - 1.0));
                    let lc = t.label(layer.label);
                    let fill = bar_color(lc, is_sel);
                    gp.rect_filled(bar, 0.0, fill);
                    if is_sel {
                        gp.rect_stroke(bar, 0.0, Stroke::new(1.0, Color32::from_white_alpha(90)), StrokeKind::Inside);
                    }
                    // Source extent (e.g. trimmed footage) shown faint.
                    if let LayerSource::Footage { .. } | LayerSource::Comp { .. } = layer.source
                        && let Some(d) = layer.source.item().and_then(|i| app.session.project.item(i)).and_then(|i| i.duration())
                    {
                        let xs = tm.x(layer.start_time.seconds());
                        let xe = tm.x(layer.comp_time(d).seconds());
                        let ghost = Rect::from_min_max(pos2(xs.min(xe), r.min.y + 9.0), pos2(xs.max(xe), r.max.y - 9.0));
                        gp.rect_filled(ghost, 1.0, lc.gamma_multiply(0.18));
                        // Dragging the source bar outside the in/out span slips the source.
                        for (side, gr) in [
                            ("l", Rect::from_min_max(ghost.min, pos2(bar.min.x, ghost.max.y))),
                            ("r", Rect::from_min_max(pos2(bar.max.x, ghost.min.y), ghost.max)),
                        ] {
                            if gr.width() < 2.0 {
                                continue;
                            }
                            let gresp = ui.interact(gr, egui::Id::new(("ghost", side, layer.id.0)), Sense::drag()).on_hover_text("Drag to slip the source");
                            app.auto.add(&format!("timeline.layer.{}.source{}", layer.id.0, side), gr, "Slip");
                            if gresp.hovered() || gresp.dragged() {
                                ctx.set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
                            }
                            let acc_id = egui::Id::new(("slip-acc", layer.id.0));
                            if gresp.dragged() {
                                let mut acc: f32 = ctx.data(|d| d.get_temp(acc_id).unwrap_or(0.0));
                                acc += gresp.drag_delta().x;
                                let frames = ((acc as f64 / pps) / fd).trunc() as i64;
                                if frames != 0 {
                                    acc -= (frames as f64 * fd * pps) as f32;
                                    actions.push((
                                        "layer.slip".into(),
                                        json!({"layers": [layer.id.0], "frames": frames, "merge": format!("slip-{}", layer.id.0)}),
                                    ));
                                }
                                ctx.data_mut(|d| d.insert_temp(acc_id, acc));
                            }
                            if gresp.drag_stopped() {
                                ctx.data_mut(|d| d.remove::<f32>(acc_id));
                                ui_actions.push(UiAct::EndMerge);
                            }
                        }
                    }
                    app.auto.add(&format!("timeline.layer.{}.bar", layer.id.0), bar, &layer.name);
                    // Interactions: move body, trim edges.
                    let edge_w = 6.0;
                    let body = ui.interact(bar.shrink2(vec2(edge_w, 0.0)), egui::Id::new(("bar", layer.id.0)), Sense::click_and_drag());
                    let (in_r, out_r) =
                        (Rect::from_min_max(bar.min, pos2(bar.min.x + edge_w, bar.max.y)), Rect::from_min_max(pos2(bar.max.x - edge_w, bar.min.y), bar.max));
                    let lin = ui.interact(in_r, egui::Id::new(("bar-in", layer.id.0)), Sense::drag());
                    let lout = ui.interact(out_r, egui::Id::new(("bar-out", layer.id.0)), Sense::drag());
                    app.auto.add(&format!("timeline.layer.{}.bar.in", layer.id.0), in_r, "Trim In");
                    app.auto.add(&format!("timeline.layer.{}.bar.out", layer.id.0), out_r, "Trim Out");
                    if lin.hovered() || lout.hovered() || lin.dragged() || lout.dragged() {
                        ctx.set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
                    }
                    if (body.clicked() || body.drag_started()) && !is_sel {
                        actions.push(("layer.select".into(), json!({"layers": [layer.id.0], "add": ui.input(|i| i.modifiers.shift)})));
                    }
                    layer_context_menu(&body, layer, &mut actions);
                    let drag_key = format!("bar-{}", layer.id.0);
                    for (resp, kind) in [(&body, "move"), (&lin, "in"), (&lout, "out")] {
                        if resp.dragged() {
                            let acc_id = egui::Id::new(("bar-acc", layer.id.0, kind));
                            let mut acc: f32 = ctx.data(|d| d.get_temp(acc_id).unwrap_or(0.0));
                            acc += resp.drag_delta().x;
                            let frames = ((acc as f64 / pps) / fd).trunc() as i64;
                            if frames != 0 {
                                acc -= (frames as f64 * fd * pps) as f32;
                                let d = frames as f64 * fd;
                                let params = match kind {
                                    "move" => {
                                        json!({"layers": if is_sel { json!(selected.iter().map(|l| l.0).collect::<Vec<_>>()) } else { json!([layer.id.0]) }, "delta": d, "merge": drag_key})
                                    }
                                    "in" => json!({"layers": [layer.id.0], "in": (layer.in_point.seconds() + d).max(0.0), "merge": drag_key}),
                                    _ => {
                                        json!({"layers": [layer.id.0], "out": (layer.out_point.seconds() + d).min(comp.duration.seconds()), "merge": drag_key})
                                    }
                                };
                                actions.push(("layer.timing".into(), params));
                            }
                            ctx.data_mut(|d| d.insert_temp(acc_id, acc));
                        }
                        if resp.drag_stopped() {
                            ctx.data_mut(|d| d.remove::<f32>(egui::Id::new(("bar-acc", layer.id.0, kind))));
                            ui_actions.push(UiAct::EndMerge);
                        }
                    }
                    // Markers on the layer (drag, Alt-drag duration, Cmd-click delete, dialog).
                    let gclip = rows_rect.intersect(Rect::from_min_max(pos2(graph_x0, rows_rect.min.y), rows_rect.max));
                    super::markers_ui::layer_markers(app, ui, gclip, &comp, layer, tm, r);
                }
            }
            RowKind::Group { uid, name, open, has_children, fx, eye } => {
                // Selected groups (an effect, a mask, a shape group…) highlight like selected
                // property rows.
                let sel_group = app.session.state.selected_props.contains(&(layer.id, *uid));
                lp.rect_filled(
                    left,
                    0.0,
                    if sel_group {
                        t.row_selected
                    } else if ri % 2 == 0 {
                        t.row
                    } else {
                        t.row_alt
                    },
                );
                gp.rect_filled(Rect::from_min_max(pos2(graph_x0, r.min.y), r.max), 0.0, t.tl_bg);
                let indent = cw.name + 6.0 + 14.0 * row.depth as f32;
                if *has_children {
                    let tw = Rect::from_center_size(pos2(indent, cy), vec2(13.0, 13.0));
                    if widgets::twirl(ui, tw, *open, egui::Id::new(("gtw", uid)), &t).clicked() {
                        ui_actions.push(UiAct::ToggleGroup(*uid));
                    }
                    app.auto.add(&format!("timeline.group.{uid}.twirl"), tw, name);
                }
                if let Some(en) = fx {
                    let fr_ = Rect::from_center_size(pos2(cw.switches + SW * 3.0 + SW / 2.0, cy), vec2(16.0, 16.0));
                    if widgets::icon_toggle(ui, fr_, Icon::Fx, *en, &t, egui::Id::new(("gfx", uid)), None).clicked() {
                        actions.push(("effect.toggle".into(), json!({"layer": layer.id.0, "effect": uid})));
                    }
                    app.auto.add(&format!("timeline.group.{uid}.fx"), fr_, name);
                }
                if let Some(en) = eye {
                    // Layer style eye switch in the A/V column, like AE.
                    let er = Rect::from_center_size(pos2(cw.av + 10.0, cy), vec2(15.0, 15.0));
                    lp.rect_stroke(er.shrink(1.5), 2.0, Stroke::new(1.0, t.separator), StrokeKind::Inside);
                    if widgets::icon_toggle(ui, er, Icon::Eye, *en, &t, egui::Id::new(("geye", uid)), None).clicked() {
                        actions.push(("layer.style.toggle".into(), json!({"layer": layer.id.0, "style": uid})));
                    }
                    app.auto.add(&format!("timeline.group.{uid}.eye"), er, name);
                }
                lp.text(pos2(indent + 10.0, cy), Align2::LEFT_CENTER, name, Tokens::ui(12.0), t.text);
                text_anim_popups(app, ui, &lp, layer, *uid, cw.switches, cy, &mut actions);
                dash_buttons(app, ui, &lp, layer, *uid, cw.switches, cy, &mut actions);
                // Mask mode + inverted inline.
                if let Some(g) = layer.props.find_group(*uid)
                    && let GroupKind::Mask { mode, inverted, .. } = g.kind
                {
                    let mr = Rect::from_min_size(pos2(cw.switches + 4.0, cy - 9.0), vec2(80.0, 18.0));
                    let pop = egui::Id::new(("maskmode", uid));
                    if widgets::dropdown(ui, mr, mode.label(), &t, egui::Id::new(("mm", uid))).clicked() {
                        widgets::open_popup(ui, pop);
                    }
                    let opts: Vec<String> = effectcraft_engine::project::MaskMode::ALL.iter().map(|m| m.label().to_string()).collect();
                    if let Some(i) = widgets::popup_menu(ui, pop, mr.left_bottom(), &opts, None) {
                        actions.push(("layer.setMask".into(), json!({"layer": layer.id.0, "mask": uid, "mode": opts[i]})));
                    }
                    let ir = Rect::from_min_size(pos2(mr.max.x + 8.0, cy - 8.0), vec2(16.0, 16.0));
                    if widgets::checkbox(ui, ir, inverted, &t, egui::Id::new(("minv", uid))).clicked() {
                        actions.push(("layer.setMask".into(), json!({"layer": layer.id.0, "mask": uid, "inverted": !inverted})));
                    }
                    lp.text(pos2(ir.max.x + 4.0, cy), Align2::LEFT_CENTER, "Inverted", Tokens::ui(11.0), t.text_dim);
                }
                let gr_rect = Rect::from_min_max(pos2(indent + 8.0, r.min.y), pos2(cw.switches, r.max.y));
                let gr = ui.interact(gr_rect, egui::Id::new(("grow", uid)), Sense::click());
                app.auto.add(&format!("timeline.group.{uid}.name"), gr_rect, name);
                if gr.clicked() && uid & WAVE_BIT == 0 {
                    actions.push(("prop.select".into(), json!({"layer": layer.id.0, "prop": uid, "selectKeys": false})));
                }
                if gr.double_clicked() {
                    if fx.is_some() {
                        // An effect's name: open it in Effect Controls (as in After Effects).
                        actions.push(("layer.select".into(), json!({"layers": [layer.id.0]})));
                        actions.push(("prop.select".into(), json!({"layer": layer.id.0, "prop": uid, "selectKeys": false})));
                        actions.push(("window.panel".into(), json!({"panel": "EffectControls"})));
                    } else {
                        ui_actions.push(UiAct::ToggleGroup(*uid));
                    }
                }
            }
            RowKind::Waveform { item } => {
                lp.rect_filled(left, 0.0, if ri % 2 == 0 { t.row } else { t.row_alt });
                let indent = cw.name + 6.0 + 14.0 * row.depth as f32;
                lp.text(pos2(indent + 10.0, cy), Align2::LEFT_CENTER, "Waveform", Tokens::ui(12.0), t.text);
                let wr = Rect::from_min_max(pos2(graph_x0, r.min.y), r.max);
                gp.rect_filled(wr, 0.0, t.tl_bg);
                app.auto.add(&format!("timeline.layer.{}.waveform", layer.id.0), wr, "Waveform");
                match super::waveform::summary(app, &ctx, effectcraft_engine::project::ItemId(*item)) {
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
            RowKind::Expr { uid, lines } => {
                let Some(prop) = layer.props.find(*uid) else { continue };
                let Some(ex) = prop.expr.clone() else { continue };
                lp.rect_filled(left, 0.0, t.row_alt);
                let indent = cw.name + 6.0 + 14.0 * row.depth as f32;
                lp.text(pos2(indent + 12.0, cy), Align2::LEFT_CENTER, "Expression:", Tokens::ui(11.5), t.text_dim);
                lp.text(pos2(indent + 82.0, cy), Align2::LEFT_CENTER, &prop.name, Tokens::ui(11.5), t.text);
                // "=" enable switch and the property pick whip (AE's expression controls).
                let en_r = Rect::from_center_size(pos2(cw.switches + 10.0, cy), vec2(16.0, 16.0));
                let en = ui.interact(en_r, egui::Id::new(("expr-en", uid)), Sense::click()).on_hover_text("Enable Expression");
                lp.rect_stroke(en_r.shrink(1.0), 2.0, Stroke::new(1.0, t.separator), StrokeKind::Inside);
                let expr_col = t.expression;
                lp.text(
                    en_r.center(),
                    Align2::CENTER_CENTER,
                    if ex.enabled { "=" } else { "≠" },
                    Tokens::semibold(13.0),
                    if ex.enabled { expr_col } else { t.text_dim },
                );
                app.auto.add(&format!("timeline.prop.{uid}.exprEnable"), en_r, "Enable Expression");
                if en.clicked() {
                    actions.push(("prop.setExpression".into(), json!({"layer": layer.id.0, "prop": uid, "enabled": !ex.enabled})));
                }
                let pw_rect = Rect::from_center_size(pos2(cw.switches + 30.0, cy), vec2(15.0, 15.0));
                let pw = ui.interact(pw_rect, egui::Id::new(("expr-whip", uid)), Sense::drag()).on_hover_text("Expression pick whip: drag onto a property");
                icons::paint(&lp, pw_rect.shrink(1.0), Icon::PickWhip, if pw.hovered() || pw.dragged() { t.text } else { t.text_dim });
                app.auto.add(&format!("timeline.prop.{uid}.pickWhip"), pw_rect, "Expression pick whip");
                if pw.drag_started() {
                    let pw_state: PickWhip = (1, layer.id.0, *uid, pw_rect.center());
                    ctx.data_mut(|d| d.insert_temp(pick_whip_id(), pw_state));
                }
                // Syntax errors: warning in the outline (the expression is disabled).
                if let Some(check) = app.session.expr_check
                    && let Err(e) = check(&ex.text)
                {
                    let wr = Rect::from_center_size(pos2(cw.switches + 50.0, cy), vec2(14.0, 14.0));
                    lp.text(wr.center(), Align2::CENTER_CENTER, "⚠", Tokens::ui(12.0), t.warning);
                    let _ = ui.interact(wr, egui::Id::new(("expr-err", uid)), Sense::hover()).on_hover_text(e);
                }
                // ⓕ: the Expression Language menu inserts at the cursor.
                let lang_r = Rect::from_center_size(pos2(cw.switches + 70.0, cy), vec2(16.0, 16.0));
                let lang = ui.interact(lang_r, egui::Id::new(("expr-lang", uid)), Sense::click()).on_hover_text("Expression Language Menu");
                lp.circle_stroke(lang_r.center(), 6.5, Stroke::new(1.0, if lang.hovered() { t.text } else { t.text_dim }));
                lp.text(lang_r.center(), Align2::CENTER_CENTER, "f", Tokens::semibold(10.0), if lang.hovered() { t.text } else { t.text_dim });
                app.auto.add(&format!("timeline.prop.{uid}.exprLanguage"), lang_r, "Expression Language Menu");
                let mut picked: Option<&'static str> = None;
                egui::Popup::menu(&lang).show(|ui| {
                    for (cat, items) in effectcraft_engine::commands::expr_tools::language_menu() {
                        ui.menu_button(cat, |ui| {
                            for (label, text) in items {
                                if ui.button(label).clicked() {
                                    picked = Some(text);
                                    ui.close();
                                }
                            }
                        });
                    }
                });
                if let Some(text) = picked {
                    let edit_id = egui::Id::new(("expr-edit", uid));
                    let buf_id = egui::Id::new(("expr-buf", uid));
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
                // The editor in the time-graph area.
                ui.set_clip_rect(right_clip);
                gp.rect_filled(Rect::from_min_max(pos2(graph_x0, r.min.y), r.max), 0.0, t.field_bg);
                let er = Rect::from_min_max(pos2(graph_x0 + 8.0, r.min.y + 3.0), pos2(rect.max.x - 14.0, r.max.y - 3.0));
                let buf_id = egui::Id::new(("expr-buf", uid));
                let mut buf: String = ctx.data(|d| d.get_temp(buf_id)).unwrap_or_else(|| ex.text.clone());
                // Settings ▸ Scripting & Expressions ▸ Expressions Editor.
                let sp = app.session.prefs.scripting.clone();
                let resp = super::expr_editor::editor(
                    ui,
                    egui::Id::new(("expr-edit", uid)),
                    &mut buf,
                    er,
                    &sp,
                    if ex.enabled { expr_col } else { t.text_dim },
                    (*lines).clamp(1, 8),
                    &t,
                );
                app.auto.add(&format!("timeline.prop.{uid}.expression"), er, &prop.name);
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
            RowKind::Prop { uid } => {
                let Some(prop) = layer.props.find(*uid) else { continue };
                let sel_prop = app.session.state.selected_props.contains(&(layer.id, *uid));
                lp.rect_filled(
                    left,
                    0.0,
                    if sel_prop {
                        t.row_selected
                    } else if ri % 2 == 0 {
                        t.row
                    } else {
                        t.row_alt
                    },
                );
                gp.rect_filled(Rect::from_min_max(pos2(graph_x0, r.min.y), r.max), 0.0, t.tl_bg);
                let indent = cw.name + 6.0 + 14.0 * row.depth as f32;
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
                    if ui.interact(prev, egui::Id::new(("kprev", uid)), Sense::click()).clicked()
                        && let Some(k) = prop.keys.iter().rev().find(|k| layer.comp_time(k.time).0 < time.0 - fr.frame_duration().0 / 2)
                    {
                        ui_actions.push(UiAct::SetTime(layer.comp_time(k.time)));
                    }
                    if ui.interact(next, egui::Id::new(("knext", uid)), Sense::click()).clicked()
                        && let Some(k) = prop.keys.iter().find(|k| layer.comp_time(k.time).0 > time.0 + fr.frame_duration().0 / 2)
                    {
                        ui_actions.push(UiAct::SetTime(layer.comp_time(k.time)));
                    }
                    if ui.interact(mid, egui::Id::new(("kmid", uid)), Sense::click()).clicked() {
                        actions.push(("prop.toggleKey".into(), json!({"layer": layer.id.0, "prop": uid})));
                    }
                    app.auto.add(&format!("timeline.prop.{uid}.addKey"), mid, "Add or remove keyframe");
                }
                // Stopwatch.
                if !prop.static_only {
                    let swr = Rect::from_center_size(pos2(indent, cy), vec2(15.0, 15.0));
                    let resp = ui.interact(swr, egui::Id::new(("stopwatch", uid)), Sense::click());
                    let col = if prop.is_animated() {
                        t.hot_text
                    } else if resp.hovered() {
                        t.text
                    } else {
                        t.text_dim
                    };
                    icons::paint(&lp, swr, Icon::Stopwatch, col);
                    app.auto.add(&format!("timeline.prop.{uid}.stopwatch"), swr, &prop.name);
                    if resp.clicked() {
                        if ui.input(|i| i.modifiers.alt) {
                            actions.push(("prop.setExpression".into(), json!({"layer": layer.id.0, "prop": uid})));
                        } else {
                            actions.push(("prop.toggleAnimation".into(), json!({"layer": layer.id.0, "prop": uid})));
                        }
                    }
                }
                let name_x = indent + 12.0;
                // 3D layers show Rotation as "Z Rotation" next to X/Y Rotation.
                let is_tr_rot = layer.transform().and_then(|t| t.get("rotation")).is_some_and(|r| r.uid == prop.uid);
                let base = if is_tr_rot && prop.name == "Rotation" && layer.is_3d() { "Z Rotation".to_string() } else { prop.name.clone() };
                let pname = if prop.has_expression() { format!("{base}  =") } else { base };
                lp.text(pos2(name_x, cy), Align2::LEFT_CENTER, &pname, Tokens::ui(12.0), if prop.has_expression() { t.expression } else { t.text });
                let name_rect = Rect::from_min_max(pos2(name_x, r.min.y), pos2(cw.switches - 2.0, r.max.y));
                let name_resp = ui.interact(name_rect, egui::Id::new(("pname", uid)), Sense::click_and_drag());
                app.auto.add(&format!("timeline.prop.{uid}.name"), name_rect, &prop.name);
                if name_resp.drag_started() {
                    // Drop on the Essential Graphics panel to expose the property.
                    egui::DragAndDrop::set_payload(&ctx, crate::panels::DragPayload::Property { layer: layer.id.0, prop: *uid });
                }
                if name_resp.dragged() {
                    egui::Tooltip::always_open(ctx.clone(), ui.layer_id(), egui::Id::new("prop-drag-tip"), egui::PopupAnchor::Pointer)
                        .show(|ui| ui.label(&prop.name));
                }
                if name_resp.clicked() {
                    actions.push(("prop.select".into(), json!({"layer": layer.id.0, "prop": uid, "add": ui.input(|i| i.modifiers.shift)})));
                }
                // Value editors.
                let value = ectx.value(layer, prop);
                let vx = (cw.switches + 4.0).max(name_x + 120.0);
                value_editor(app, ui, &lp, layer, prop, &value, pos2(vx, cy), &mut actions);
                ui.set_clip_rect(right_clip);
                // Expression text row hint.
                if let Some(e) = prop.expr.as_ref().filter(|e| e.enabled && app.ui.timeline.expr_closed.contains(uid)) {
                    gp.text(
                        pos2(graph_x0 + 8.0, cy),
                        Align2::LEFT_CENTER,
                        format!("= {}", e.text.lines().next().unwrap_or("")),
                        Tokens::mono(11.0),
                        t.expression,
                    );
                }
                // Keyframes.
                if !graph_on {
                    for k in &prop.keys {
                        let ct = layer.comp_time(k.time);
                        let x = tm.x(ct.seconds());
                        if x < graph_x0 - 8.0 || x > rect.max.x + 8.0 {
                            continue;
                        }
                        let kref = effectcraft_engine::KeyRef { layer: layer.id, prop: *uid, time: k.time };
                        let ks = sel_keys.contains(&kref);
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
                            icons::keyframe(&gp, c, 11.0, icon.left, icon.right, kcol, Color32::from_black_alpha(200));
                        }
                        let kr = Rect::from_center_size(c, vec2(12.0, 14.0));
                        let kresp = ui.interact(kr, egui::Id::new(("key", uid, k.time.0)), Sense::click_and_drag());
                        app.auto.add(&format!("timeline.key.{uid}.{}", fr.frame_at(ct)), kr, &format!("{} key", prop.name));
                        let mods = ui.input(|i| i.modifiers);
                        let this_key = json!({"keys": [{"layer": layer.id.0, "prop": uid, "time": k.time.seconds()}]});
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
                        if kresp.double_clicked() {
                            // Edit the key's value (numeric properties), as After Effects does.
                            if matches!(k.value, Value::Scalar(_) | Value::Vec2(_) | Value::Vec3(_)) {
                                actions.push(("keys.set".into(), json!({"layer": layer.id.0, "prop": uid, "time": k.time.seconds()})));
                            } else {
                                ui_actions.push(UiAct::SetTime(ct));
                            }
                        }
                        kresp.context_menu(|ui| {
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
                                    ui.menu_button("Label", |ui| {
                                        for (i, l) in effectcraft_engine::color::Label::ALL.iter().enumerate() {
                                            if ui.add(egui::Button::new(l.name()).selected(k.label as usize == i)).clicked() {
                                                if !ks {
                                                    actions.push((
                                                        "keys.select".into(),
                                                        json!({"keys": [{"layer": layer.id.0, "prop": uid, "time": k.time.seconds()}]}),
                                                    ));
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
                        // Alt-drag the first or last key of a selected group: scale the group in time.
                        if kresp.drag_started()
                            && ui.input(|i| i.modifiers.alt)
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
                            ctx.data_mut(|d| d.insert_temp(key_drag_id(), KeyDrag { origin_x: p.x, anchor: ct.seconds(), applied: 0 }));
                        }
                    }
                }
            }
        }
    }
    ui.set_clip_rect(full_clip);
    if outline_empty.clicked() && !ui.input(|i| i.modifiers.shift || i.modifiers.command) {
        actions.push(("layer.select".into(), json!({"layers": []})));
    }
    if let (Some(start), Some(end)) = (ctx.input(|i| i.pointer.press_origin()), outline_empty.interact_pointer_pos())
        && outline_empty.dragged()
    {
        let box_rect = Rect::from_two_pos(start, end).intersect(outline_rect);
        lp.rect_filled(box_rect, 0.0, t.accent.gamma_multiply(0.12));
        lp.rect_stroke(box_rect, 0.0, Stroke::new(1.0, t.accent), StrokeKind::Inside);
        ctx.data_mut(|d| d.insert_temp(egui::Id::new("tl-layer-box"), box_rect));
    }
    if outline_empty.drag_stopped()
        && let Some(box_rect) = ctx.data(|d| d.get_temp::<Rect>(egui::Id::new("tl-layer-box")))
    {
        ctx.data_mut(|d| d.remove::<Rect>(egui::Id::new("tl-layer-box")));
        let layers: Vec<u64> = hit_rows
            .iter()
            .filter(|(r, row)| matches!(row.kind, RowKind::Layer) && r.intersects(box_rect))
            .filter(|(_, row)| comp.layer(row.layer).is_some_and(|l| !l.switches.locked))
            .map(|(_, row)| row.layer.0)
            .collect();
        actions.push(("layer.select".into(), json!({"layers": layers, "add": ui.input(|i| i.modifiers.shift || i.modifiers.command)})));
    }
    // Pick whips: a line follows the pointer; releasing over a row links to it.
    if let Some((kind, src_layer, src_prop, start)) = ctx.data(|d| d.get_temp::<PickWhip>(pick_whip_id())) {
        let ptr = ctx.input(|i| i.pointer.latest_pos()).unwrap_or(start);
        let fg = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new("tl-pickwhip-line")));
        fg.line_segment([start, ptr], Stroke::new(1.5, t.accent));
        fg.circle_stroke(ptr, 4.0, Stroke::new(1.5, t.accent));
        let target = hit_rows.iter().find(|(r, _)| r.contains(ptr)).map(|(r, row)| (*r, row.clone()));
        if let Some((tr, _)) = &target {
            fg.rect_stroke(Rect::from_min_max(tr.min, pos2(graph_x0 - 1.0, tr.max.y)), 0.0, Stroke::new(1.0, t.accent), StrokeKind::Inside);
        }
        if ctx.input(|i| !i.pointer.any_down()) {
            ctx.data_mut(|d| d.remove::<PickWhip>(pick_whip_id()));
            match (kind, target) {
                (0, Some((_, row))) if row.layer.0 != src_layer => {
                    actions.push(("layer.setParent".into(), json!({"layers": [src_layer], "parent": row.layer.0})));
                }
                (1, Some((_, Row { layer, kind: RowKind::Prop { uid }, .. }))) if uid != src_prop => {
                    actions.push(("prop.pickWhip".into(), json!({"layer": src_layer, "prop": src_prop, "target": {"layer": layer.0, "prop": uid}})));
                }
                _ => {}
            }
        }
    }
    // Alt-drag key group scaling (follows the pointer until release).
    if let Some(c) = app.session.active_comp_arc() {
        super::graph_tools::alt_scale_update(app, &ctx, &c, tm, &mut actions);
    }
    // Graph editor.
    if graph_on {
        super::graph::show(app, ui, &gp, &comp, &ectx, tm, Rect::from_min_max(pos2(graph_x0, rows_rect.min.y), rows_rect.max), &mut actions);
    }
    // Empty-area click in the graph: deselect keys; drag: box-select keys.
    if empty.clicked() {
        actions.push(("keys.select".into(), json!({"keys": []})));
    }
    if let (true, Some(origin), Some(cur)) =
        (empty.dragged(), empty.interact_pointer_pos().and_then(|_| ctx.input(|i| i.pointer.press_origin())), empty.interact_pointer_pos())
    {
        let br = Rect::from_two_pos(origin, cur);
        gp.rect_filled(br, 0.0, t.accent.gamma_multiply(0.12));
        gp.rect_stroke(br, 0.0, Stroke::new(1.0, t.accent), StrokeKind::Middle);
        ctx.data_mut(|d| d.insert_temp(egui::Id::new("tl-box"), br));
    }
    // Layer reorder drag: a line where the layers will land; the release moves them there.
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
    // Keyframe drag: the selected keys move by whole frames with the pointer until it is
    // released (one undo step); Shift snaps the grabbed key to the current time, the work area
    // and composition markers.
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
                    let mut cands = snap_candidates(&comp, &rows);
                    cands.retain(|c| !moving.iter().any(|m| (m - c).abs() < 1e-9));
                    cands.push(time.seconds());
                    target = snap_time(&cands, target, 8.0 / pps);
                }
                let total = ((target - kd.anchor) / fd).round() as i64;
                if total != kd.applied {
                    actions.push(("keys.move".into(), json!({"delta": (total - kd.applied) as f64 * fd, "merge": "key-drag"})));
                    kd.applied = total;
                    ctx.data_mut(|d| d.insert_temp(key_drag_id(), kd));
                }
                ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
                // Where the grabbed key is and how far the keys moved (After Effects shows this
                // in the Info panel).
                let at = Tick::from_seconds_f64(kd.anchor + kd.applied as f64 * fd);
                let note = format!("{}   {:+} f", super::timecode(&app.session, &comp, at), kd.applied);
                let fg = ctx.layer_painter(egui::LayerId::new(egui::Order::Tooltip, egui::Id::new("tl-key-drag-tip")));
                let g = fg.layout_no_wrap(note, Tokens::ui(11.5), t.text);
                let r = Rect::from_min_size(ptr + vec2(14.0, 14.0), g.size() + vec2(10.0, 6.0));
                fg.rect_filled(r, 3.0, t.panel_bg);
                fg.rect_stroke(r, 3.0, Stroke::new(1.0, t.field_border), StrokeKind::Inside);
                fg.galley(r.min + vec2(5.0, 3.0), g, t.text);
                app.ui.status = format!("Keyframe at {}", super::timecode(&app.session, &comp, at));
            }
            None => {
                ctx.data_mut(|d| d.remove::<KeyDrag>(key_drag_id()));
                ui_actions.push(UiAct::EndMerge);
            }
        }
    }
    if empty.drag_stopped()
        && let Some(br) = ctx.data(|d| d.get_temp::<Rect>(egui::Id::new("tl-box")))
    {
        ctx.data_mut(|d| d.remove::<Rect>(egui::Id::new("tl-box")));
        let mut keys = vec![];
        let mut yy = rows_rect.min.y - app.ui.timeline.scroll_y;
        for row in &rows {
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
        actions.push(("keys.select".into(), json!({"keys": keys, "add": ui.input(|i| i.modifiers.shift)})));
    }

    // CTI.
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
    let _ = graph_rect;

    // Footer.
    let footer = Rect::from_min_max(pos2(rect.min.x, rect.max.y - footer_h), rect.max);
    p.rect_filled(footer, 0.0, t.panel_bg);
    p.line_segment([footer.left_top(), footer.right_top()], Stroke::new(1.0, t.separator));
    if overflow > 0.0 {
        // Outline scroll bar along the top of the footer, under the columns.
        let track = Rect::from_min_max(pos2(rect.min.x + 2.0, footer.min.y + 1.0), pos2(graph_x0 - 3.0, footer.min.y + 5.0));
        let frac = left_w / natural_w;
        let tw = (track.width() * frac).max(20.0);
        let tx = track.min.x + (track.width() - tw) * (oscroll / overflow);
        let thumb = Rect::from_min_size(pos2(tx, track.min.y), vec2(tw, track.height()));
        p.rect_filled(track, 2.0, t.field_bg);
        let sresp = ui.interact(track.expand2(vec2(0.0, 2.0)), egui::Id::new("tl-outline-scroll"), Sense::drag());
        p.rect_filled(thumb, 2.0, if sresp.dragged() || sresp.hovered() { t.text_dim } else { t.text_faint });
        app.auto.add("timeline.outlineScroll", track, &format!("{oscroll}/{overflow}"));
        if sresp.dragged() {
            let k = overflow / (track.width() - tw).max(1.0);
            app.ui.timeline.outline_scroll = (oscroll + sresp.drag_delta().x * k).clamp(0.0, overflow);
        }
    }
    let tgl = Rect::from_min_size(pos2(footer.min.x + 10.0, footer.min.y + 3.0), vec2(150.0, 18.0));
    let tresp = ui.interact(tgl, egui::Id::new("tl-toggle-modes"), Sense::click());
    p.text(tgl.left_center(), Align2::LEFT_CENTER, "Toggle Switches / Modes", Tokens::ui(11.0), if tresp.hovered() { t.text } else { t.text_dim });
    app.auto.add("timeline.toggleSwitchesModes", tgl, "Toggle Switches / Modes");
    if tresp.clicked() {
        app.ui.timeline.show_modes = !app.ui.timeline.show_modes;
    }
    let ms = app.frames.last_ms.lock().map(|v| *v).unwrap_or(0.0);
    p.text(pos2(graph_x0 - 12.0, footer.center().y), Align2::RIGHT_CENTER, format!("Frame Render Time  {ms:.0}ms"), Tokens::ui(11.0), t.text_faint);
    // Zoom slider.
    let zs = Rect::from_min_max(pos2(graph_x0 + 30.0, footer.center().y - 2.0), pos2((graph_x0 + 230.0).min(rect.max.x - 30.0), footer.center().y + 2.0));
    icons::paint(&p, Rect::from_center_size(pos2(zs.min.x - 14.0, footer.center().y), vec2(14.0, 14.0)), Icon::MountainSmall, t.text_dim);
    icons::paint(&p, Rect::from_center_size(pos2(zs.max.x + 14.0, footer.center().y), vec2(16.0, 16.0)), Icon::MountainLarge, t.text_dim);
    p.rect_filled(zs, 2.0, t.field_bg);
    let maxpps = MAX_PPS;
    let frac = ((pps / fit_pps).ln() / (maxpps / fit_pps).ln()).clamp(0.0, 1.0) as f32;
    let knob = pos2(zs.min.x + zs.width() * frac, zs.center().y);
    p.circle_filled(knob, 6.0, t.text);
    let zresp = ui.interact(zs.expand2(vec2(6.0, 8.0)), egui::Id::new("tl-zoom"), Sense::drag());
    app.auto.add("timeline.zoomSlider", zs, "Zoom");
    if zresp.dragged()
        && let Some(pt) = zresp.interact_pointer_pos()
    {
        let f = ((pt.x - zs.min.x) / zs.width()).clamp(0.0, 1.0) as f64;
        let npps = fit_pps * (maxpps / fit_pps).powf(f);
        let cti = time.seconds();
        let rel = (cti - tm.start) * pps;
        app.ui.timeline.start = (cti - rel / npps).max(0.0);
        app.ui.timeline.pps = Some(npps);
    }

    // Composition markers in the ruler; protected regions shade the layer rows.
    let mstrip = Rect::from_min_max(pos2(graph_x0, ruler.max.y - 12.0), pos2(graph_x1 + 10.0, ruler.max.y));
    let mrows = rows_rect.intersect(Rect::from_min_max(pos2(graph_x0, rows_rect.min.y), rows_rect.max));
    super::markers_ui::comp_markers(app, ui, &comp, tm, mstrip, mrows);

    // Drop targets: effects from Effects & Presets go on the layer under the pointer. Footage and
    // comps from the Project panel and files from the Media Browser go where they are dropped: a
    // line shows the place in the stack, and over the time graph a marker shows the In point too
    // (Shift: at the current time).
    if let Some(payload) = egui::DragAndDrop::payload::<crate::panels::DragPayload>(&ctx)
        && !matches!(*payload, crate::panels::DragPayload::Property { .. })
        && let Some(ptr) = ctx.input(|i| i.pointer.hover_pos()).filter(|_| ui.rect_contains_pointer(rows_rect))
    {
        let released = ctx.input(|i| i.pointer.any_released());
        if let crate::panels::DragPayload::Effect(e) = payload.as_ref() {
            p.rect_stroke(rows_rect, 0.0, Stroke::new(2.0, t.accent), StrokeKind::Inside);
            let target = hit_rows.iter().find(|(r, _)| ptr.y >= r.min.y && ptr.y < r.max.y).map(|(_, row)| row.layer);
            if released && let Some(l) = target {
                actions.push(("effect.apply".into(), json!({"effect": e, "layers": [l.0]})));
            }
        } else {
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
                let (index, time) = (json!(index), json!(start.map(Tick::seconds)));
                match payload.as_ref() {
                    crate::panels::DragPayload::Item(id) => actions.push(("layer.addItem".into(), json!({"item": id, "index": index, "time": time}))),
                    crate::panels::DragPayload::Files(paths) => {
                        actions.push(("mediaBrowser.import".into(), json!({"paths": paths, "addToComp": true, "index": index, "time": time})))
                    }
                    crate::panels::DragPayload::Effect(_) | crate::panels::DragPayload::Property { .. } => {}
                }
            }
        }
        if released {
            egui::DragAndDrop::clear_payload(&ctx);
        }
    }

    // Apply.
    for a in ui_actions {
        match a {
            UiAct::ToggleLayer(id, all) => {
                let tl = &mut app.ui.timeline;
                // Twirled by hand, the layer shows its whole tree again.
                tl.reveal.clear();
                tl.layer_reveal.remove(&id);
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
            UiAct::ToggleGroup(uid) => {
                if !app.ui.timeline.open_groups.remove(&uid) {
                    app.ui.timeline.open_groups.insert(uid);
                }
            }
            UiAct::SetTime(tt) => app.session.set_time(tt),
            UiAct::EndMerge => app.session.history.merge_key = None,
        }
    }
    if ui.input(|i| i.pointer.primary_down()) && actions.iter().any(|(id, _)| id == "prop.set") {
        app.ui.viewer.property_interacting = true;
        ctx.request_repaint();
    }
    for (id, params) in actions {
        if id == "__endMerge" {
            app.session.history.merge_key = None;
            continue;
        }
        if let Err(e) = crate::menus::invoke(app, &ctx, &id, params) {
            app.ui.status = e;
        }
    }
}

/// The optional per-layer cells: Comment (double-click to edit), In / Out / Duration (drag to
/// change by frames), Stretch (click: Time Stretch dialog).
#[allow(clippy::too_many_arguments)]
fn layer_cells(
    app: &mut EffectcraftApp,
    ui: &mut egui::Ui,
    lp: &egui::Painter,
    cw: &Cols,
    comp: &Comp,
    layer: &Layer,
    r: Rect,
    cy: f32,
    actions: &mut Vec<(String, serde_json::Value)>,
) {
    let t = app.tokens;
    let ctx = ui.ctx().clone();
    let lid = layer.id.0;
    if cw.vis.comment {
        let cr = Rect::from_min_max(pos2(cw.comment + 4.0, r.min.y + 1.0), pos2(cw.comment + COMMENT_W - 4.0, r.max.y - 1.0));
        let edit_id = egui::Id::new("tl-comment-edit");
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
            let resp = ui.interact(cr, egui::Id::new(("tl-comment", lid)), Sense::click()).on_hover_text("Double-click to edit the comment");
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
        let tc = crate::panels::timecode(&app.session, comp, Tick::from_seconds_f64(secs) - if kind == "duration" { comp.display_start } else { Tick(0) });
        let resp = ui.interact(cr, egui::Id::new(("tl-cell", kind, lid)), Sense::drag()).on_hover_text("Drag to change");
        lp.text(pos2(cr.min.x + 4.0, cy), Align2::LEFT_CENTER, &tc, Tokens::mono(11.0), if resp.hovered() || resp.dragged() { t.hot_text } else { t.timecode });
        app.auto.add(&format!("timeline.layer.{lid}.{kind}"), cr, &tc);
        if resp.hovered() || resp.dragged() {
            ctx.set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
        }
        let acc_id = egui::Id::new(("tl-cell-acc", kind, lid));
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
        let resp = ui.interact(cr, egui::Id::new(("tl-stretch", lid)), Sense::click()).on_hover_text("Time Stretch…");
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
    ToggleGroup(u64),
    SetTime(Tick),
    EndMerge,
}

fn walk_groups(g: &PropGroup, out: &mut Vec<u64>) {
    for sg in g.groups() {
        out.push(sg.uid);
        walk_groups(sg, out);
    }
}

/// A stroke's Dashes "+" / "−" buttons (add or remove a Dash/Gap pair).
#[allow(clippy::too_many_arguments)]
fn dash_buttons(
    app: &mut EffectcraftApp,
    ui: &mut egui::Ui,
    p: &egui::Painter,
    layer: &Layer,
    uid: u64,
    x: f32,
    cy: f32,
    actions: &mut Vec<(String, serde_json::Value)>,
) {
    if !matches!(layer.source, LayerSource::Shape) || layer.props.find_group(uid).is_none_or(|g| g.match_id != "dashes") {
        return;
    }
    let Some(stroke) = layer.props.parent_of(uid).map(|g| g.uid) else { return };
    let t = app.tokens;
    for (i, (sym, id, tip)) in
        [("+", "shape.dashes.add", "Add a Dash or Gap"), ("\u{2212}", "shape.dashes.remove", "Remove a Dash or Gap")].into_iter().enumerate()
    {
        let r = Rect::from_center_size(pos2(x + 12.0 + i as f32 * 20.0, cy), vec2(16.0, 16.0));
        let resp = ui.interact(r, egui::Id::new(("tl-dash", uid, i)), Sense::click()).on_hover_text(tip);
        p.rect_stroke(r.shrink(1.0), 2.0, Stroke::new(1.0, t.separator), StrokeKind::Inside);
        p.text(r.center(), Align2::CENTER_CENTER, sym, Tokens::ui(12.0), if resp.hovered() { t.text } else { t.text_dim });
        app.auto.add(&format!("timeline.group.{uid}.{}", if i == 0 { "addDash" } else { "removeDash" }), r, tip);
        if resp.clicked() {
            actions.push((id.into(), json!({"layer": layer.id.0, "prop": stroke})));
        }
    }
}

/// The Text group's "Animate:" and an animator's "Add:" pop-up menus (AE's twirl-down menus).
#[allow(clippy::too_many_arguments)]
fn text_anim_popups(
    app: &mut EffectcraftApp,
    ui: &mut egui::Ui,
    p: &egui::Painter,
    layer: &Layer,
    uid: u64,
    x: f32,
    cy: f32,
    actions: &mut Vec<(String, serde_json::Value)>,
) {
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
    let resp = ui.interact(br, egui::Id::new(("tl-textanim", uid)), Sense::click()).on_hover_text(if key == "add" {
        "Add property or selector"
    } else {
        "Animate text"
    });
    icons::paint(p, br.shrink(3.0), Icon::ChevronRight, if resp.hovered() { t.text } else { t.text_dim });
    app.auto.add(&format!("timeline.group.{uid}.{key}"), br, label);
    let pop = egui::Id::new(("tl-textanim-pop", uid));
    if resp.clicked() {
        widgets::open_popup(ui, pop);
    }
    let per_char = layer.props.prop("text/perChar3d").is_some_and(|q| q.value.as_bool());
    let mut opts: Vec<String> = Vec::new();
    let mut acts: Vec<(String, serde_json::Value)> = Vec::new();
    if key == "animate" {
        opts.push(if per_char { "Disable Per-character 3D" } else { "Enable Per-character 3D" }.into());
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

fn kind_name(k: MatteKind) -> &'static str {
    match k {
        MatteKind::Alpha => "alpha",
        MatteKind::AlphaInverted => "alphaInverted",
        MatteKind::Luma => "luma",
        MatteKind::LumaInverted => "lumaInverted",
    }
}

fn layer_context_menu(resp: &egui::Response, layer: &Layer, actions: &mut Vec<(String, serde_json::Value)>) {
    resp.context_menu(|ui| layer_menu(ui, &[layer.id.0], actions));
}

/// Shared layer actions for Timeline rows and Composition context menus.
pub(crate) fn layer_menu(ui: &mut egui::Ui, layers: &[u64], actions: &mut Vec<(String, serde_json::Value)>) {
    let Some(&id) = layers.first() else { return };
    let mut item = |ui: &mut egui::Ui, label: &str, cmd: &str, params: serde_json::Value| {
        if ui.button(label).clicked() {
            actions.push(("layer.select".into(), json!({"layers": layers})));
            actions.push((cmd.into(), params));
            ui.close();
        }
    };
    ui.menu_button("New", |ui| {
        item(ui, "Text", "layer.newText", json!({"text": "Text"}));
        item(ui, "Solid…", "app.solidSettings", json!({}));
        item(ui, "Light…", "layer.newLight", json!({}));
        item(ui, "Camera…", "layer.newCamera", json!({}));
        item(ui, "Null Object", "layer.newNull", json!({}));
        item(ui, "Shape Layer", "layer.newShape", json!({"kind": "none"}));
        item(ui, "Adjustment Layer", "layer.newAdjustment", json!({}));
    });
    item(ui, "Layer Settings…", "layer.settings", json!({}));
    ui.separator();
    ui.menu_button("Masks", |ui| {
        item(ui, "New Mask", "layer.addMask", json!({"layer": id}));
    });
    ui.menu_button("Transform", |ui| {
        item(ui, "Reset", "layer.transform", json!({"layers": layers, "op": "reset"}));
        item(ui, "Center In View", "layer.transform", json!({"layers": layers, "op": "center"}));
        item(ui, "Fit to Comp", "layer.transform", json!({"layers": layers, "op": "fit"}));
        item(ui, "Flip Horizontal", "layer.transform", json!({"layers": layers, "op": "flipH"}));
        item(ui, "Flip Vertical", "layer.transform", json!({"layers": layers, "op": "flipV"}));
    });
    ui.menu_button("Time", |ui| {
        item(ui, "Enable Time Remapping", "layer.enableTimeRemap", json!({"layers": layers}));
        item(ui, "Time-Reverse Layer", "layer.timeReverse", json!({"layers": layers}));
        item(ui, "Time Stretch…", "layer.timeStretch", json!({}));
        item(ui, "Freeze Frame", "layer.freezeFrame", json!({"layers": layers}));
        item(ui, "Freeze on Last Frame", "layer.freezeOnLastFrame", json!({"layers": layers}));
    });
    ui.menu_button("Blending Mode", |ui| {
        for m in BlendMode::ALL {
            item(ui, m.label(), "layer.setBlendMode", json!({"layers": layers, "mode": m.label()}));
            if m.ends_group() {
                ui.separator();
            }
        }
    });
    ui.menu_button("Arrange", |ui| {
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
}

fn fmt_num(v: f64, d: usize) -> String {
    format!("{v:.d$}")
}

/// Inline value editor for a property row; pushes `prop.set` actions.
fn value_editor(
    app: &mut EffectcraftApp,
    ui: &mut egui::Ui,
    p: &egui::Painter,
    layer: &Layer,
    prop: &Property,
    value: &Value,
    at: Pos2,
    actions: &mut Vec<(String, serde_json::Value)>,
) {
    let t = app.tokens;
    let uid = prop.uid;
    let merge = format!("scrub-{uid}");
    let set = |actions: &mut Vec<(String, serde_json::Value)>, v: serde_json::Value| {
        actions.push(("prop.set".into(), json!({"layer": layer.id.0, "prop": uid, "value": v, "merge": merge})))
    };
    let is_3d = layer.is_3d();
    let mut x = at.x;
    let y = at.y - 9.0;
    match value {
        Value::Scalar(v) => {
            let (min, max, dec, speed) = match &prop.ui {
                ParamUi::Slider { min, max, decimals, .. } => (*min, *max, *decimals as usize, ((max - min) / 400.0).clamp(0.01, 10.0)),
                ParamUi::Percent => (-1e6, 1e6, 1, 0.5),
                _ => (-1e9, 1e9, 1, 1.0),
            };
            if matches!(prop.ui, ParamUi::Angle) {
                let (rr, dr, nv) = super::fx_widgets::angle_field(ui, pos2(x, y), egui::Id::new(("v", uid, 0)), *v, 1, &t);
                app.auto.add(&format!("timeline.prop.{uid}.value"), dr, &prop.name);
                app.auto.add(&format!("timeline.prop.{uid}.revolutions"), rr, &prop.name);
                if let Some(nv) = nv {
                    set(actions, json!(nv));
                }
            } else {
                let suffix = if matches!(prop.ui, ParamUi::Percent) || prop.match_id == "opacity" { "%" } else { "" };
                let (r, nv, _) = widgets::hot_number_at(ui, pos2(x, y), egui::Id::new(("v", uid, 0)), *v, speed, (min, max), dec.max(1), suffix, &t);
                app.auto.add(&format!("timeline.prop.{uid}.value"), r, &prop.name);
                if let Some(nv) = nv {
                    set(actions, json!(nv));
                }
            }
        }
        Value::Vec2(_) | Value::Vec3(_) => {
            let c = value.components();
            let n = if prop.shown_dims > 0 && !(is_3d && c.len() == 3) { prop.shown_dims as usize } else { c.len() };
            let pct = matches!(prop.ui, ParamUi::Percent);
            for d in 0..n.min(c.len()) {
                let suffix = if pct && d + 1 == n { "%" } else { "" };
                let (r, nv, _) =
                    widgets::hot_number_at(ui, pos2(x, y), egui::Id::new(("v", uid, d)), c[d], if pct { 0.5 } else { 1.0 }, (-1e9, 1e9), 1, suffix, &t);
                app.auto.add(&format!("timeline.prop.{uid}.value.{d}"), r, &prop.name);
                if let Some(nv) = nv {
                    let mut nc = c.clone();
                    if pct && prop.match_id == "scale" && !ui.input(|i| i.modifiers.alt) {
                        // Constrain proportions (AE's chain link, on by default).
                        let k = if c[d].abs() > 1e-9 { nv / c[d] } else { 1.0 };
                        for e in 0..n {
                            nc[e] = if e == d { nv } else { c[e] * k };
                        }
                    } else {
                        nc[d] = nv;
                    }
                    set(actions, json!(nc));
                }
                x = r.max.x + if d + 1 < n { 2.0 } else { 0.0 };
                if d + 1 < n {
                    p.text(pos2(x, at.y), Align2::LEFT_CENTER, ",", Tokens::ui(12.0), t.hot_text);
                    x += 6.0;
                }
            }
        }
        Value::Color(c) => {
            let r = Rect::from_min_size(pos2(x, at.y - 7.0), vec2(26.0, 14.0));
            let resp = widgets::swatch(ui, r, [c[0] as f32, c[1] as f32, c[2] as f32, 1.0], egui::Id::new(("sw", uid)), &t);
            app.auto.add(&format!("timeline.prop.{uid}.value"), r, &prop.name);
            let pop = egui::Id::new(("cpop", uid));
            if resp.clicked() {
                widgets::open_popup(ui, pop);
            }
            let mut rgb = [c[0] as f32, c[1] as f32, c[2] as f32];
            if crate::header::color_popup(ui, pop, r.left_bottom(), &mut rgb) {
                set(actions, json!([rgb[0], rgb[1], rgb[2], 1.0]));
            }
        }
        Value::Bool(b) => {
            let r = Rect::from_min_size(pos2(x, at.y - 8.0), vec2(16.0, 16.0));
            if widgets::checkbox(ui, r, *b, &t, egui::Id::new(("cb", uid))).clicked() {
                set(actions, json!(!b));
            }
            app.auto.add(&format!("timeline.prop.{uid}.value"), r, &prop.name);
        }
        Value::Enum(i) => {
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
                let pop = egui::Id::new(("epop", uid));
                if widgets::dropdown(ui, r, &label, &t, egui::Id::new(("en", uid))).clicked() {
                    widgets::open_popup(ui, pop);
                }
                app.auto.add(&format!("timeline.prop.{uid}.value"), r, &prop.name);
                if let Some(ni) = widgets::popup_menu(ui, pop, r.left_bottom(), options, Some(*i as usize)) {
                    set(actions, json!(ni));
                }
            } else {
                p.text(pos2(x, at.y), Align2::LEFT_CENTER, fmt_num(*i as f64, 0), Tokens::ui(12.0), t.hot_text);
            }
        }
        Value::Path(_) => {
            p.text(pos2(x, at.y), Align2::LEFT_CENTER, "Shape…", Tokens::ui(12.0), t.hot_text);
        }
        Value::Text(_) => {}
        Value::Layer(l) => {
            p.text(pos2(x, at.y), Align2::LEFT_CENTER, l.map(|l| format!("Layer {l}")).unwrap_or("None".into()), Tokens::ui(12.0), t.hot_text);
        }
        Value::Gradient(_) => {
            p.text(pos2(x, at.y), Align2::LEFT_CENTER, "Edit Gradient…", Tokens::ui(12.0), t.hot_text);
        }
        Value::Str(s) => {
            p.text(pos2(x, at.y), Align2::LEFT_CENTER, s.chars().take(30).collect::<String>(), Tokens::ui(12.0), t.text_dim);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A comp with two solids; "Box" has an animated Position and a Gaussian Blur.
    fn app() -> EffectcraftApp {
        let mut s = effectcraft_engine::Session::default();
        s.execute("comp.new", json!({"name": "T", "width": 320, "height": 180, "duration": 4})).unwrap();
        s.execute("layer.newSolid", json!({"name": "Plate", "color": "#406080", "width": 320, "height": 180})).unwrap();
        s.execute("layer.newSolid", json!({"name": "Box", "color": "#e04020", "width": 40, "height": 40})).unwrap();
        let comp = s.active_comp().unwrap();
        let bx = comp.layers.iter().find(|l| l.name == "Box").unwrap().id.0;
        s.execute("layer.select", json!({"layers": [bx]})).unwrap();
        s.execute("effect.apply", json!({"effect": "ec.blur.gaussian"})).unwrap();
        let pos = s.active_comp().unwrap().layer(LayerId(bx)).unwrap().transform().unwrap().get("position").unwrap().uid;
        s.execute("prop.toggleAnimation", json!({"layer": bx, "prop": pos})).unwrap();
        EffectcraftApp::new(s)
    }

    fn labels(app: &EffectcraftApp) -> Vec<String> {
        let comp = app.session.active_comp().unwrap().clone();
        build_rows(app, &comp)
            .into_iter()
            .map(|r| {
                let l = comp.layer(r.layer).unwrap();
                match r.kind {
                    RowKind::Layer => l.name.clone(),
                    RowKind::Group { name, .. } => format!("{}>{name}", "  ".repeat(r.depth)),
                    RowKind::Prop { uid } => format!("{}{}", "  ".repeat(r.depth), l.props.find(uid).unwrap().name),
                    _ => "?".into(),
                }
            })
            .collect()
    }

    #[test]
    fn search_filters_layers_and_reveals_matching_properties() {
        let mut app = app();
        app.ui.timeline.search = "opac".into();
        let rows = labels(&app);
        // Both layers have Transform > Opacity; nothing else shows.
        assert_eq!(rows.iter().filter(|r| r.trim() == "Opacity").count(), 2, "{rows:?}");
        assert!(rows.iter().any(|r| r.trim() == ">Transform"), "{rows:?}");
        assert!(!rows.iter().any(|r| r.trim() == "Position"), "{rows:?}");
        // A layer name match shows the layer alone; a group match brings its contents.
        app.ui.timeline.search = "plate".into();
        assert_eq!(labels(&app), vec!["Plate".to_string()]);
        app.ui.timeline.search = "GAUSSIAN".into();
        let rows = labels(&app);
        assert_eq!(rows[0], "Box");
        assert!(rows.iter().any(|r| r.trim() == ">Effects") && rows.iter().any(|r| r.trim() == "Blurriness"), "{rows:?}");
        assert!(!rows.contains(&"Plate".to_string()));
        // Nothing matches → no rows.
        app.ui.timeline.search = "zzz".into();
        assert!(labels(&app).is_empty());
        // Through the command.
        let ctx = egui::Context::default();
        crate::menus::invoke(&mut app, &ctx, "timeline.search", json!({"query": "scale"})).unwrap();
        assert!(labels(&app).iter().any(|r| r.trim() == "Scale"));
    }

    #[test]
    fn columns_toggle_and_layout() {
        let mut app = app();
        let tl = &mut app.ui.timeline;
        assert!(column_visible(tl, "switches") && !column_visible(tl, "comment") && !column_visible(tl, "modes"));
        set_column(tl, "comment", true).unwrap();
        set_column(tl, "stretch", true).unwrap();
        set_column(tl, "keys", false).unwrap();
        set_column(tl, "label", false).unwrap();
        set_column(tl, "modes", true).unwrap();
        assert!(tl.show_modes && column_visible(tl, "comment") && !column_visible(tl, "label"));
        assert!(set_column(tl, "name", false).is_err());
        assert!(set_column(tl, "bogus", true).is_err());
        let vis = Vis::of(tl);
        let cw = cols(0.0, 1400.0, vis);
        // Hidden columns take no room; visible ones are in After Effects' order.
        assert_eq!(cw.keys, cw.label);
        assert_eq!(cw.label, cw.num);
        assert!(cw.name < cw.comment && cw.comment < cw.switches && cw.switches < cw.mode && cw.parent < cw.stretch);
        assert_eq!(column_at(&cw, cw.comment + 5.0), Some("comment"));
        assert_eq!(column_at(&cw, cw.name + 5.0), Some("name"));
        assert_eq!(column_at(&cw, cw.stretch + 5.0), Some("stretch"));
        // Through the command (as agents do).
        let ctx = egui::Context::default();
        let r = crate::menus::invoke(&mut app, &ctx, "timeline.column", json!({"column": "duration"})).unwrap();
        assert!(r["columns"].as_array().unwrap().iter().any(|c| c == "duration"));
        crate::menus::invoke(&mut app, &ctx, "timeline.column", json!({"column": "duration", "visible": false})).unwrap();
        assert!(!column_visible(&app.ui.timeline, "duration"));
        crate::menus::invoke(&mut app, &ctx, "timeline.sourceName", json!({})).unwrap();
        assert!(app.ui.timeline.source_name);
    }

    #[test]
    fn reveal_keys_add_with_shift_and_uu_reveals_modified() {
        let mut app = app();
        let ctx = egui::Context::default();
        crate::menus::invoke(&mut app, &ctx, "timeline.reveal.opacity", json!({})).unwrap();
        assert_eq!(app.ui.timeline.reveal, vec!["opacity"]);
        crate::menus::invoke(&mut app, &ctx, "timeline.revealAdd.scale", json!({})).unwrap();
        crate::menus::invoke(&mut app, &ctx, "timeline.revealAdd.effects", json!({})).unwrap();
        assert_eq!(app.ui.timeline.reveal, vec!["opacity", "scale", "effects"]);
        let rows = labels(&app);
        assert!(rows.iter().any(|r| r.trim() == "Scale") && rows.iter().any(|r| r.trim() == "Opacity"), "{rows:?}");
        assert!(rows.iter().any(|r| r.trim() == ">Gaussian Blur"), "{rows:?}");
        // Tree order: Scale before Opacity.
        let si = rows.iter().position(|r| r.trim() == "Scale").unwrap();
        let oi = rows.iter().position(|r| r.trim() == "Opacity").unwrap();
        assert!(si < oi);
        // Shift+S again removes Scale.
        crate::menus::invoke(&mut app, &ctx, "timeline.revealAdd.scale", json!({})).unwrap();
        assert_eq!(app.ui.timeline.reveal, vec!["opacity", "effects"]);
        // U: keyframed properties (Position); UU (twice quickly): every modified property.
        crate::menus::invoke(&mut app, &ctx, "anim.reveal", json!({"kind": "keyframes"})).unwrap();
        let n_keys = app.ui.timeline.reveal_props.len();
        assert_eq!(app.ui.timeline.reveal, vec!["props"]);
        assert!(n_keys >= 1);
        crate::menus::invoke(&mut app, &ctx, "anim.reveal", json!({"kind": "keyframes"})).unwrap();
        assert!(app.ui.timeline.reveal_props.len() > n_keys, "UU reveals modified properties too");
    }

    /// U and the other reveal shortcuts act on the selected layers only: other layers keep their
    /// twirl state and their own reveal (#160).
    #[test]
    fn reveal_shortcuts_leave_unselected_layers_alone() {
        let mut app = app();
        let ctx = egui::Context::default();
        let comp = app.session.active_comp().unwrap().clone();
        let id = |n: &str| comp.layers.iter().find(|l| l.name == n).unwrap().id.0;
        let (plate, bx) = (id("Plate"), id("Box"));
        let select = |app: &mut EffectcraftApp, l: u64| app.session.execute("layer.select", json!({"layers": [l]})).unwrap();
        // Plate shows Scale (S); then U on Box.
        select(&mut app, plate);
        crate::menus::invoke(&mut app, &ctx, "timeline.reveal.scale", json!({})).unwrap();
        select(&mut app, bx);
        crate::menus::invoke(&mut app, &ctx, "anim.reveal", json!({"kind": "keyframes"})).unwrap();
        let tl = &app.ui.timeline;
        assert!(tl.open_layers.contains(&plate) && tl.open_layers.contains(&bx), "{:?}", tl.open_layers);
        assert_eq!(tl.layer_reveal.get(&plate), Some(&vec!["scale".to_string()]), "Plate keeps its reveal");
        assert_eq!(tl.layer_reveal.get(&bx), Some(&vec!["props".to_string()]));
        let rows = labels(&app);
        let at = |n: &str| rows.iter().position(|r| r == n).unwrap();
        assert_eq!(rows[at("Plate") + 1].trim(), "Scale", "{rows:?}");
        assert_eq!(rows[at("Box") + 1].trim(), "Position", "{rows:?}");
        // UU (modified), then U later: Box shows its keyframed properties only again.
        crate::menus::invoke(&mut app, &ctx, "anim.reveal", json!({"kind": "keyframes"})).unwrap();
        assert!(labels(&app).iter().any(|r| r.trim() == ">Gaussian Blur"), "UU shows the effect");
        app.last_reveal = Some(("keyframes".into(), -10.0));
        crate::menus::invoke(&mut app, &ctx, "anim.reveal", json!({"kind": "keyframes"})).unwrap();
        app.last_reveal = Some(("keyframes".into(), -10.0));
        crate::menus::invoke(&mut app, &ctx, "anim.reveal", json!({"kind": "keyframes"})).unwrap();
        let rows = labels(&app);
        assert!(!rows.iter().any(|r| r.trim() == ">Gaussian Blur") && rows.iter().any(|r| r.trim() == "Position"), "{rows:?}");
        // U again on Box (later than a double press) hides only Box's.
        app.last_reveal = Some(("keyframes".into(), -10.0));
        crate::menus::invoke(&mut app, &ctx, "anim.reveal", json!({"kind": "keyframes"})).unwrap();
        let tl = &app.ui.timeline;
        assert!(!tl.open_layers.contains(&bx) && tl.open_layers.contains(&plate), "{:?}", tl.open_layers);
        // A layer twirled open by hand shows its whole tree, whatever was revealed elsewhere.
        app.session.execute("layer.select", json!({"layers": []})).unwrap();
        app.ui.timeline.open_layers.insert(bx);
        let rows = labels(&app);
        assert!(rows.iter().any(|r| r.trim() == ">Transform"), "{rows:?}");
    }

    /// Enabling time remapping twirls the layer open on Time Remap (#157); disabling it reveals
    /// nothing.
    #[test]
    fn enabling_time_remapping_reveals_time_remap() {
        let mut s = effectcraft_engine::Session::default();
        let inner = s.execute("comp.new", json!({"name": "Inner", "width": 32, "height": 32, "duration": 2})).unwrap()["comp"].as_u64().unwrap();
        s.execute("comp.new", json!({"name": "Main", "width": 64, "height": 64, "duration": 4})).unwrap();
        s.execute("layer.newSolid", json!({"name": "Other"})).unwrap();
        let nested = s.execute("layer.addItem", json!({"item": inner})).unwrap()["layer"].as_u64().unwrap();
        let mut app = EffectcraftApp::new(s);
        let ctx = egui::Context::default();
        crate::menus::invoke(&mut app, &ctx, "layer.enableTimeRemap", json!({"layers": [nested]})).unwrap();
        assert_eq!(app.ui.timeline.layer_reveal.get(&nested), Some(&vec!["timeRemap".to_string()]));
        let rows = labels(&app);
        let at = rows.iter().position(|r| r == "Inner").unwrap();
        assert_eq!(rows[at + 1].trim(), "Time Remap", "{rows:?}");
        // Through the selection (the Layer menu), turning it off again: nothing new is revealed.
        app.session.execute("layer.select", json!({"layers": [nested]})).unwrap();
        app.ui.timeline.layer_reveal.clear();
        crate::menus::invoke(&mut app, &ctx, "layer.enableTimeRemap", json!({})).unwrap();
        assert!(app.ui.timeline.layer_reveal.is_empty());
    }

    #[test]
    fn cti_snaps_to_the_nearest_candidate() {
        assert_eq!(snap_time(&[0.0, 1.0, 2.0], 1.04, 0.1), 1.0);
        assert_eq!(snap_time(&[0.0, 1.0, 2.0], 1.5, 0.1), 1.5);
        assert_eq!(snap_time(&[1.0, 1.1], 1.06, 0.1), 1.1);
        let app = app();
        let comp = app.session.active_comp().unwrap().clone();
        let c = snap_candidates(&comp, &build_rows(&app, &comp));
        assert!(c.contains(&comp.duration.seconds()) && c.contains(&0.0));
    }

    /// UI fidelity (M13.5): compact After Effects-like rows and muted label-coloured bars.
    #[test]
    fn rows_and_bars_follow_after_effects_proportions() {
        let t = crate::theme::Tokens::for_kind(crate::theme::ThemeKind::Dark);
        assert_eq!(t.row_h, 19.0);
        let red = t.label(effectcraft_engine::color::Label::Red);
        let bar = bar_color(red, false);
        // Muted: darker and less saturated than the label, still reddish.
        assert!(bar.r() < red.r() && bar.r() > bar.g() + 30 && bar.g() >= red.g().min(0x3a) - 1);
        let sel = bar_color(red, true);
        assert!(sel.r() > bar.r());
    }
}
