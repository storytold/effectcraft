//! Composition viewer state that agents and the CLI can drive too: snapping, Show Channel and
//! exposure, snapshots, Fast Previews, guides dragged out of the rulers, and the Pen tool for
//! shape layers (`shape.newPath`).

use effectcraft_keyframe::ShapePath;
use effectcraft_project::build::{self, Ids};
use effectcraft_render::RenderOpts;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{CommandSpec, always, b_p, bad, comp_id, f_p, has_comp, merge_p, str_p};
use crate::viewer::{Channel, CustomRgb, SimProfile, Simulation, SnapFeatures, Snapshot};
use crate::{EngineError, Result, Session, VertexRef, cmd, query};

/// Composition ▸ Preview ▸ Fast Previews modes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum FastPreviews {
    #[default]
    Off,
    AdaptiveResolution,
    Draft,
    FastDraft,
    Wireframe,
}

impl FastPreviews {
    pub const ALL: [FastPreviews; 5] =
        [FastPreviews::Off, FastPreviews::AdaptiveResolution, FastPreviews::Draft, FastPreviews::FastDraft, FastPreviews::Wireframe];
    pub fn id(self) -> &'static str {
        match self {
            FastPreviews::Off => "off",
            FastPreviews::AdaptiveResolution => "adaptive",
            FastPreviews::Draft => "draft",
            FastPreviews::FastDraft => "fastDraft",
            FastPreviews::Wireframe => "wireframe",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            FastPreviews::Off => "Off (Final Quality)",
            FastPreviews::AdaptiveResolution => "Adaptive Resolution",
            FastPreviews::Draft => "Draft",
            FastPreviews::FastDraft => "Fast Draft",
            FastPreviews::Wireframe => "Wireframe",
        }
    }
    pub fn from_name(s: &str) -> Option<FastPreviews> {
        let n = s.to_ascii_lowercase().replace([' ', '_', '-'], "");
        FastPreviews::ALL.into_iter().find(|m| m.id().to_ascii_lowercase() == n || m.label().to_ascii_lowercase().replace(' ', "").starts_with(&n))
    }
    /// Render options for the viewer: (draft quality, resolution factor while interacting).
    pub fn render(self, interacting: bool) -> (bool, f64) {
        match self {
            FastPreviews::Off => (false, 1.0),
            FastPreviews::AdaptiveResolution => (false, if interacting { 0.25 } else { 1.0 }),
            FastPreviews::Draft => (true, 1.0),
            FastPreviews::FastDraft => (true, if interacting { 0.25 } else { 1.0 }),
            FastPreviews::Wireframe => (true, 1.0),
        }
    }
}

/// Viewer display options (Show Channel, exposure, snapshot, Fast Previews, display colour).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ViewOptions {
    pub channel: Channel,
    /// Show Channel ▸ Colorize: single channels shown in their colour instead of gray.
    pub colorized: bool,
    /// Adjust Exposure, in stops.
    pub exposure: f32,
    /// Show Snapshot (F5 held): the viewer shows the snapshot instead of the frame.
    pub show_snapshot: bool,
    pub fast_previews: FastPreviews,
    /// View ▸ Use Display Color Management (with a working space): the frame is converted
    /// from the working space to the display; off shows the working space's numbers.
    pub display_color_management: bool,
    /// View ▸ Simulate Output.
    pub simulation: Simulation,
    /// The profile View ▸ Simulate Output ▸ Custom... uses.
    pub custom_simulation: Simulation,
}

impl Default for ViewOptions {
    fn default() -> Self {
        ViewOptions {
            channel: Channel::default(),
            colorized: false,
            exposure: 0.0,
            show_snapshot: false,
            fast_previews: FastPreviews::default(),
            display_color_management: true,
            simulation: Simulation::default(),
            custom_simulation: Simulation { profile: SimProfile::Linear, preserve_rgb: true },
        }
    }
}

/// View ▸ Split with New Locked Viewer: a second Composition viewer that keeps showing one
/// comp and 3D view whatever comp is active.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct LockedViewer {
    pub comp: effectcraft_project::ItemId,
    pub view: effectcraft_render::three_d::View3D,
}

fn snapping(s: &mut Session, p: &Value) -> Result<Value> {
    s.state.snapping = b_p(p, "value").unwrap_or(!s.state.snapping);
    Ok(json!(s.state.snapping))
}

/// Tools bar ▸ Snapping options: sets the options passed (`toggle` flips one by key).
fn snapping_options(s: &mut Session, p: &Value) -> Result<Value> {
    let f = &mut s.state.snap_features;
    if let Some(k) = str_p(p, "toggle") {
        let v = f.option_mut(k).ok_or_else(|| bad("view.snappingOptions", format!("unknown option `{k}`")))?;
        *v = !*v;
    }
    for (k, _) in SnapFeatures::OPTIONS {
        if let (Some(v), Some(o)) = (b_p(p, k), f.option_mut(k)) {
            *o = v;
        }
    }
    Ok(json!(s.state.snap_features))
}

fn channel(s: &mut Session, p: &Value) -> Result<Value> {
    let name = str_p(p, "channel").unwrap_or("rgb");
    let c = Channel::from_name(name).ok_or_else(|| bad("view.channel", format!("unknown channel `{name}` (rgb|red|green|blue|alpha|rgbStraight)")))?;
    // Choosing the active single channel again returns to RGB; not when that changes it between
    // grayscale and colorized.
    let same = c == s.state.viewer.channel && b_p(p, "colorized").is_none_or(|v| v == s.state.viewer.colorized);
    let c = if same && c != Channel::Rgb && b_p(p, "toggle").unwrap_or(false) { Channel::Rgb } else { c };
    s.state.viewer.channel = c;
    if let Some(v) = b_p(p, "colorized") {
        s.state.viewer.colorized = v;
    }
    Ok(json!({"channel": c.id(), "colorized": s.state.viewer.colorized}))
}

fn exposure(s: &mut Session, p: &Value) -> Result<Value> {
    let cur = s.state.viewer.exposure as f64;
    let v = match (f_p(p, "stops"), f_p(p, "delta")) {
        (Some(v), _) => v,
        (None, Some(d)) => cur + d,
        _ => return Err(bad("view.exposure", "pass `stops` or `delta`")),
    };
    s.state.viewer.exposure = v.clamp(-40.0, 40.0) as f32;
    Ok(json!(s.state.viewer.exposure))
}

fn reset_exposure(s: &mut Session, _: &Value) -> Result<Value> {
    s.state.viewer.exposure = 0.0;
    Ok(json!(0.0))
}

fn take_snapshot(s: &mut Session, p: &Value) -> Result<Value> {
    let cid = comp_id(s, p)?;
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?;
    let t = s.time_of(cid);
    let scale = f_p(p, "scale").unwrap_or(1.0).clamp(0.05, 1.0);
    let img = s.render(cid, t, RenderOpts { scale, guides: true, view: s.view_camera(cid).filter(|_| comp.has_3d()), ..Default::default() });
    let (w, h) = (img.width, img.height);
    s.snapshot = Some(Snapshot { comp: cid, time: t, scale, image: std::sync::Arc::new(img) });
    s.events.push(crate::Event::ProjectChanged { revision: s.revision });
    Ok(json!({"width": w, "height": h, "time": t.seconds()}))
}

fn has_snapshot(s: &Session) -> std::result::Result<(), String> {
    if s.snapshot.is_some() { Ok(()) } else { Err("take a snapshot first (Shift+F5)".into()) }
}

fn show_snapshot(s: &mut Session, p: &Value) -> Result<Value> {
    s.state.viewer.show_snapshot = b_p(p, "value").unwrap_or(!s.state.viewer.show_snapshot);
    Ok(json!(s.state.viewer.show_snapshot))
}

/// A `view.fastPreviews.*` command choosing one Fast Previews mode.
macro_rules! fast {
    ($id:literal, $label:literal, $sc:literal, $mode:literal) => {
        cmd!($id, $label, [], Some($sc), "{}", always, |s, _| fast_previews(s, &json!({"mode": $mode})))
    };
}

/// A `view.channel.*` command showing one channel (again: back to RGB), in grayscale or colorized.
macro_rules! chan {
    ($id:literal, $label:literal, $sc:literal, $channel:literal, $colorized:literal) => {
        cmd!($id, $label, [], Some($sc), "{}", always, |s, _| channel(s, &json!({"channel": $channel, "colorized": $colorized, "toggle": true})))
    };
}

fn fast_previews(s: &mut Session, p: &Value) -> Result<Value> {
    let name = str_p(p, "mode").unwrap_or("off");
    let m =
        FastPreviews::from_name(name).ok_or_else(|| bad("view.fastPreviewMode", format!("unknown mode `{name}` (off|adaptive|draft|fastDraft|wireframe)")))?;
    s.state.viewer.fast_previews = m;
    Ok(json!(m.id()))
}

fn guide_index(s: &Session, p: &Value, cmd: &str) -> Result<(effectcraft_project::ItemId, usize)> {
    let cid = comp_id(s, p)?;
    let n = s.project.comp(cid).ok_or(EngineError::NoComp)?.guides.len();
    let i = p.get("index").and_then(Value::as_u64).ok_or_else(|| bad(cmd, "missing `index`"))? as usize;
    if i >= n {
        return Err(bad(cmd, format!("no guide {i} (the comp has {n})")));
    }
    Ok((cid, i))
}

fn move_guide(s: &mut Session, p: &Value) -> Result<Value> {
    let (cid, i) = guide_index(s, p, "view.moveGuide")?;
    let pos = f_p(p, "position").ok_or_else(|| bad("view.moveGuide", "missing `position` (comp px)"))?;
    let pos = if pos.is_finite() { pos } else { return Err(bad("view.moveGuide", "`position` must be a finite number")) };
    s.edit("Move Guide", merge_p(p), |proj, _| {
        let c = proj.comp_mut(cid).ok_or(EngineError::NoComp)?;
        let (w, h) = (c.width, c.height);
        let g = c.guides.get_mut(i).ok_or(EngineError::NoComp)?;
        g.set_px(pos, w, h);
        Ok(())
    })?;
    Ok(json!(pos))
}

fn remove_guide(s: &mut Session, p: &Value) -> Result<Value> {
    let (cid, i) = guide_index(s, p, "view.removeGuide")?;
    s.edit("Remove Guide", None, |proj, _| {
        proj.comp_mut(cid).ok_or(EngineError::NoComp)?.guides.remove(i);
        Ok(())
    })?;
    Ok(Value::Null)
}

fn pts(v: Option<&Value>) -> Vec<[f64; 2]> {
    v.and_then(Value::as_array).map(|a| a.iter().filter_map(|p| Some([p.get(0)?.as_f64()?, p.get(1)?.as_f64()?])).collect()).unwrap_or_default()
}

/// Pen tool on a shape layer: a new Shape group ("Shape n" with Path 1 and the Tools bar's
/// Stroke and Fill unless given) on the given / selected shape layer, or on a new shape layer
/// when none is given. Vertices are in the layer's space, or comp space (`space: "comp"`, the
/// default for a new layer).
fn new_path(s: &mut Session, p: &Value) -> Result<Value> {
    let cid = comp_id(s, p)?;
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?.clone();
    let mut v = pts(p.get("vertices"));
    if v.is_empty() {
        return Err(bad("shape.newPath", "need `vertices` [[x,y], …]"));
    }
    let n = v.len();
    let mut ins = pts(p.get("inTangents"));
    let mut outs = pts(p.get("outTangents"));
    ins.resize(n, [0.0; 2]);
    outs.resize(n, [0.0; 2]);
    let target = super::shape_tool::draw_target(s, &comp, p, "shape.newPath")?;
    let comp_space = str_p(p, "space").map(|s| s == "comp").unwrap_or(target.is_none());
    if comp_space {
        // Comp → layer space of the target (a new layer sits at the comp centre, unrotated).
        let inv = match target.and_then(|l| comp.layer(l)) {
            Some(l) => {
                let ctx = effectcraft_render::EvalCtx::new(&s.project, cid, &comp, s.time_of(cid));
                ctx.layer_to_comp(l).0.inverse().unwrap_or(effectcraft_geom::Mat3::IDENTITY)
            }
            None => effectcraft_geom::Mat3::translate(effectcraft_geom::vec2(-(comp.width as f64) / 2.0, -(comp.height as f64) / 2.0)),
        };
        for q in &mut v {
            let r = inv.apply(effectcraft_geom::vec2(q[0], q[1]));
            *q = [r.x, r.y];
        }
        for t in ins.iter_mut().chain(outs.iter_mut()) {
            let r = inv.apply_vec(effectcraft_geom::vec2(t[0], t[1]));
            *t = [r.x, r.y];
        }
    }
    let path = ShapePath { vertices: v, in_tangents: ins, out_tangents: outs, closed: b_p(p, "closed").unwrap_or(false), feather: Vec::new() };
    let paint = super::shape_tool::paint_p(p, &s.state.shape_tool, "shape.newPath")?;
    // Gradients run across the path's points.
    let (lo, hi) =
        path.vertices.iter().fold(([f64::MAX; 2], [f64::MIN; 2]), |(lo, hi), v| ([lo[0].min(v[0]), lo[1].min(v[1])], [hi[0].max(v[0]), hi[1].max(v[1])]));
    let bounds = [lo[0], lo[1], hi[0] - lo[0], hi[1] - lo[1]];
    let name = str_p(p, "name").map(str::to_string);
    let (lid, group, path_uid) = s.edit("Pen Tool", None, |proj, st| {
        let lid = match target {
            Some(l) => l,
            None => super::shape_tool::new_shape_layer(proj, st, &comp, cid, name.as_deref(), None)?,
        };
        let mut next = proj.next_id;
        let mut ids = Ids(&mut next);
        let pg = build::shape_path(&mut ids, path);
        let path_uid = pg.uid;
        let mut items = vec![pg];
        items.extend(super::shape_tool::paint_items(&mut ids, &paint, bounds));
        let g = build::shape_group(&mut ids, "Shape 1", items);
        proj.next_id = next;
        let group = super::shape_tool::add_to_contents(proj, cid, lid, g, [0.0, 0.0], "shape.newPath")?;
        st.selected_layers = vec![lid];
        st.selected_vertices = vec![VertexRef { layer: lid, mask: path_uid, index: n - 1 }];
        Ok((lid, group, path_uid))
    })?;
    Ok(json!({"layer": lid.0, "group": group, "path": path_uid}))
}

/// Snapshot image as straight 8-bit RGBA rows (for frontends / `--out`).
pub fn snapshot_rgba8(s: &Session) -> Option<(u32, u32, Vec<u8>)> {
    let snap = s.snapshot.as_ref()?;
    let img = &snap.image;
    let mut out = Vec::with_capacity(img.data.len() * 4);
    for p in &img.data {
        let a = p[3].clamp(0.0, 1.0);
        let un = |c: f32| if a > 0.0 { (c / a).clamp(0.0, 1.0) } else { 0.0 };
        out.extend([un(p[0]), un(p[1]), un(p[2]), a].map(|v| (v * 255.0 + 0.5) as u8));
    }
    Some((img.width, img.height, out))
}

fn shape_layer_or_none(s: &Session) -> std::result::Result<(), String> {
    has_comp(s)
}

// ---------------------------------------------------------------- display colour, locked viewer

/// View ▸ Use Display Color Management (toggle, or `value`).
fn display_cm(s: &mut Session, p: &Value) -> Result<Value> {
    let v = b_p(p, "value").unwrap_or(!s.state.viewer.display_color_management);
    s.state.viewer.display_color_management = v;
    s.events.push(crate::Event::ProjectChanged { revision: s.revision });
    Ok(json!({"displayColorManagement": v, "workingSpace": s.project.settings.working_space.map(|w| w.id()), "display": s.prefs.previews.display_profile}))
}

/// View ▸ Simulate Output ▸ (profile). `custom` uses (and with `space` sets) the Custom profile.
fn simulate_output(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "view.simulateOutput";
    let name = str_p(p, "profile").unwrap_or("none");
    let ids = SimProfile::ALL.iter().map(|p| p.id()).collect::<Vec<_>>().join("|");
    let sim = if name.eq_ignore_ascii_case("custom") {
        let mut c = s.state.viewer.custom_simulation;
        if let Some(sp) = str_p(p, "space") {
            c.profile = SimProfile::parse(sp).ok_or_else(|| bad(cmd, format!("space: {ids}")))?;
        }
        if let Some(v) = b_p(p, "preserveRgb") {
            c.preserve_rgb = v;
        }
        s.state.viewer.custom_simulation = c;
        c
    } else {
        let profile = SimProfile::parse(name).ok_or_else(|| bad(cmd, format!("profile: {ids}|custom")))?;
        Simulation { profile, preserve_rgb: b_p(p, "preserveRgb").unwrap_or(false) }
    };
    s.state.viewer.simulation = sim;
    s.events.push(crate::Event::ProjectChanged { revision: s.revision });
    Ok(json!({"profile": sim.profile.id(), "label": sim.profile.label(), "preserveRgb": sim.preserve_rgb}))
}

fn custom_rgb_json(c: &CustomRgb) -> Value {
    let mut v = serde_json::to_value(c).unwrap_or_default();
    // A LUT-based profile's tables stay in Settings; the reply says it is one.
    if let Some(o) = v.as_object_mut() {
        o.remove("iccLut");
        o.insert("lutProfile".into(), json!(!c.icc_lut.is_empty()));
    }
    v
}

/// View ▸ Simulate Output ▸ My Custom RGB…: define the custom output device (kept in Settings)
/// and, unless `apply` is false, simulate it. An `icc` path that differs from the one the
/// definition came from is read (its primaries, white and curve replace the numbers); `from`
/// starts from a built-in profile; `reset` goes back to the default.
fn custom_rgb(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "view.customRgb";
    let mut c = if b_p(p, "reset") == Some(true) { CustomRgb::default() } else { s.prefs.custom_rgb.clone() };
    if let Some(from) = str_p(p, "from") {
        let prof = SimProfile::parse(from).ok_or_else(|| bad(cmd, format!("from: unknown profile `{from}`")))?;
        if let Some((prims, curve)) = prof.space() {
            [c.red, c.green, c.blue] = prims;
            c.white = [0.3127, 0.3290];
            (c.gamma, c.srgb_curve) = match curve {
                crate::viewer::Curve::Srgb => (2.2, true),
                crate::viewer::Curve::Gamma(g) => ((g as f64 * 1000.0).round() / 1000.0, false),
                _ => (1.0, false),
            };
            c.icc.clear();
            c.icc_lut = Default::default();
            c.name = format!("Custom ({})", prof.label());
        }
    }
    let pt = |k: &str| -> Result<Option<[f64; 2]>> {
        match p.get(k) {
            None | Some(Value::Null) => Ok(None),
            Some(v) => {
                let a = v.as_array().filter(|a| a.len() == 2).ok_or_else(|| bad(cmd, format!("{k}: [x, y]")))?;
                let g = |i: usize| a[i].as_f64().ok_or_else(|| bad(cmd, format!("{k}: [x, y] numbers")));
                Ok(Some([g(0)?, g(1)?]))
            }
        }
    };
    let mut typed = false;
    for (k, slot) in [("red", &mut c.red), ("green", &mut c.green), ("blue", &mut c.blue), ("white", &mut c.white)] {
        if let Some(v) = pt(k)? {
            typed |= *slot != v;
            *slot = v;
        }
    }
    if let Some(g) = f_p(p, "gamma") {
        typed |= c.gamma != g;
        c.gamma = g;
    }
    if let Some(b) = b_p(p, "srgbCurve") {
        typed |= c.srgb_curve != b;
        c.srgb_curve = b;
    }
    // Typed-in numbers replace a LUT-based profile's tables (they only approximated it).
    if typed {
        c.icc_lut = Default::default();
    }
    if let Some(n) = str_p(p, "name").map(str::trim).filter(|n| !n.is_empty()) {
        c.name = n.to_string();
    }
    match str_p(p, "icc").map(str::trim) {
        Some("") => {
            c.icc.clear();
            c.icc_lut = Default::default();
        }
        Some(path) if path != c.icc || b_p(p, "reload") == Some(true) => {
            let bytes = s.services.read_file(path).map_err(|e| EngineError::Other(format!("cannot read {path}: {e}")))?;
            let mut from = CustomRgb::from_icc(&bytes, path).map_err(|e| bad(cmd, format!("{path}: {e}")))?;
            if let Some(n) = str_p(p, "name").map(str::trim).filter(|n| !n.is_empty() && *n != s.prefs.custom_rgb.name) {
                from.name = n.to_string();
            }
            c = from;
        }
        _ => {}
    }
    c.validate().map_err(|e| bad(cmd, e))?;
    s.prefs.custom_rgb = c.clone();
    s.prefs_revision += 1;
    s.save_prefs();
    if b_p(p, "apply") != Some(false) {
        let preserve = b_p(p, "preserveRgb").unwrap_or(false);
        s.state.viewer.simulation = Simulation { profile: SimProfile::MyCustom, preserve_rgb: preserve };
    }
    s.events.push(crate::Event::ProjectChanged { revision: s.revision });
    let mut out = custom_rgb_json(&c);
    out["active"] = json!(s.state.viewer.simulation.profile == SimProfile::MyCustom);
    Ok(out)
}

/// View ▸ Split with New Locked Viewer: lock a second viewer to the comp (default the active
/// one) and its current 3D view (or `view`).
fn split_locked(s: &mut Session, p: &Value) -> Result<Value> {
    let cid = comp_id(s, p)?;
    let view = match str_p(p, "view") {
        Some(v) => effectcraft_render::three_d::View3D::from_id(v).ok_or_else(|| bad("view.splitLockedViewer", format!("unknown view `{v}`")))?,
        None => s.state.views3d.get(&cid).map(|v| v.current).unwrap_or_default(),
    };
    s.state.locked_viewer = Some(LockedViewer { comp: cid, view });
    s.events.push(crate::Event::ProjectChanged { revision: s.revision });
    Ok(json!({"comp": cid.0, "view": view.id()}))
}

fn close_locked(s: &mut Session, _: &Value) -> Result<Value> {
    let was = s.state.locked_viewer.take().is_some();
    s.events.push(crate::Event::ProjectChanged { revision: s.revision });
    Ok(json!({"closed": was}))
}

fn has_locked(s: &Session) -> std::result::Result<(), String> {
    if s.state.locked_viewer.is_some() { Ok(()) } else { Err("no locked viewer".into()) }
}

/// The viewer's colour settings and what they do.
fn display_color_state(s: &mut Session, _: &Value) -> Result<Value> {
    let v = &s.state.viewer;
    Ok(json!({
        "displayColorManagement": v.display_color_management,
        "simulation": {"profile": v.simulation.profile.id(), "preserveRgb": v.simulation.preserve_rgb},
        "customRgb": custom_rgb_json(&s.prefs.custom_rgb),
        "display": s.prefs.previews.display_profile,
        "workingSpace": s.project.settings.working_space.map(|w| w.id()),
        "active": crate::viewer::DisplayColor::of(s).is_some(),
        "lockedViewer": s.state.locked_viewer.map(|l| json!({"comp": l.comp.0, "view": l.view.id()})),
    }))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!("view.snapping", "Snapping", ["View"], None, "{value?}", always, snapping),
        cmd!(
            "view.snappingOptions",
            "Snapping Options",
            [],
            None,
            "{edgesExtended? (Snap Edges Extended), collapsedFeatures? (Snap to Features in Collapsed Compositions and Text Layers): bool, toggle?: one of those keys} → the options",
            always,
            snapping_options
        ),
        cmd!("view.displayColorManagement", "Use Display Color Management", ["View"], None, "{value?}", always, display_cm),
        cmd!(
            "view.simulateOutput",
            "Simulate Output",
            [],
            None,
            "{profile: none|rec709|ntsc|pal|mac18|srgb|rec2020|p3|linear|myCustom|custom, preserveRgb?, space? (custom)}",
            always,
            simulate_output
        ),
        cmd!(
            "view.customRgb",
            "My Custom RGB...",
            [],
            None,
            "{name?, red?/green?/blue?/white?: [x, y] (CIE xy), gamma?, srgbCurve?: bool, icc?: path (RGB ICC profile, matrix/TRC or LUT-based A2B0: read when it changes, \"\" clears; a LUT profile simulates through its tables, `lutProfile` in the reply; typed-in numbers replace it), reload?, from?: a Simulate Output profile to start from, reset?, preserveRgb?, apply?: bool (default true: simulate it)} → the definition (kept in Settings)",
            always,
            custom_rgb
        ),
        cmd!("view.splitLockedViewer", "Split with New Locked Viewer", ["View"], None, "{comp?, view?: 3D view id}", has_comp, split_locked),
        cmd!("view.closeLockedViewer", "Close Locked Viewer", [], None, "{}", has_locked, close_locked),
        query!(
            "view.displayColor",
            "Viewer Color State",
            "{} → display colour management, output simulation, display profile, locked viewer",
            display_color_state
        ),
        cmd!("view.channel", "Show Channel", [], None, "{channel: rgb|red|green|blue|alpha|rgbStraight, colorized?, toggle?}", always, channel),
        cmd!("view.exposure", "Adjust Exposure", [], None, "{stops? | delta?}", always, exposure),
        cmd!("view.resetExposure", "Reset Exposure", [], None, "{}", always, reset_exposure),
        cmd!("view.takeSnapshot", "Take Snapshot", [], Some("Shift+F5"), "{comp?, scale?: 0.05..1}", has_comp, take_snapshot),
        cmd!("view.showSnapshot", "Show Snapshot", [], None, "{value?}", has_snapshot, show_snapshot),
        cmd!("view.fastPreviewMode", "Fast Previews", [], None, "{mode: off|adaptive|draft|fastDraft|wireframe}", always, fast_previews),
        fast!("view.fastPreviews.off", "Fast Previews: Off (Final Quality)", "Cmd+Alt+1", "off"),
        fast!("view.fastPreviews.adaptive", "Fast Previews: Adaptive Resolution", "Cmd+Alt+2", "adaptive"),
        fast!("view.fastPreviews.draft", "Fast Previews: Draft", "Cmd+Alt+3", "draft"),
        fast!("view.fastPreviews.fastDraft", "Fast Previews: Fast Draft", "Cmd+Alt+4", "fastDraft"),
        fast!("view.fastPreviews.wireframe", "Fast Previews: Wireframe", "Cmd+Alt+5", "wireframe"),
        chan!("view.channel.red", "Show Channel: Red", "Alt+1", "red", false),
        chan!("view.channel.green", "Show Channel: Green", "Alt+2", "green", false),
        chan!("view.channel.blue", "Show Channel: Blue", "Alt+3", "blue", false),
        chan!("view.channel.alpha", "Show Channel: Alpha", "Alt+4", "alpha", false),
        chan!("view.channel.redColorized", "Show Channel: Red, Colorized", "Alt+Shift+1", "red", true),
        chan!("view.channel.greenColorized", "Show Channel: Green, Colorized", "Alt+Shift+2", "green", true),
        chan!("view.channel.blueColorized", "Show Channel: Blue, Colorized", "Alt+Shift+3", "blue", true),
        cmd!("view.channel.rgbStraight", "Show Channel: RGB Straight", [], Some("Alt+Shift+4"), "{}", always, |s, _| channel(
            s,
            &json!({"channel": "rgbStraight", "toggle": true})
        )),
        cmd!("view.moveGuide", "Move Guide", [], None, "{comp?, index, position (comp px), merge?}", has_comp, move_guide),
        cmd!("view.removeGuide", "Remove Guide", [], None, "{comp?, index}", has_comp, remove_guide),
        cmd!(
            "shape.newPath",
            "Pen Tool (Shape Path)",
            [],
            None,
            "{layer?, vertices: [[x,y]…], inTangents?, outTangents?, closed?, space?: comp|layer, fill?: [r,g,b]|#hex|false, fillType?, fillBlend?, fillOpacity?, stroke?, strokeType?, strokeBlend?, strokeOpacity?, strokeWidth? (default the Tools bar's, shape.toolOptions), name?}",
            shape_layer_or_none,
            new_path
        ),
    ]
}
