//! The Flowchart panel (Composition ▸ Composition Flowchart): a node graph of a composition and
//! everything it uses — nested comps, footage, solids — optionally with its layers and their
//! effects. Left to right or top to bottom; double-click a comp to open it; Cmd/Ctrl-click a
//! node to reveal it in the Project panel (layers: select the layer); drag to pan.
//!
//! [`build`] is pure and cycle-safe (a comp nested in itself is drawn once, with an edge back),
//! so agents get the same graph through `flowchart.graph`.

use crate::i18n::tr_args;
use std::collections::{BTreeMap, BTreeSet};

use effectcraft_engine::project::{ItemId, ItemKind, LayerSource, Project};
use egui::{Align2, Color32, Rect, Sense, Stroke, StrokeKind, pos2, vec2};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::icons::{self, Icon};
use crate::theme::Tokens;
use crate::{EffectcraftApp, widgets};

/// Flowchart panel options (the panel's switches).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FlowOptions {
    /// Show layers between a comp and its sources.
    pub layers: bool,
    /// Show each layer's effects (with layers shown).
    pub effects: bool,
    /// Show solids.
    pub solids: bool,
    /// Top to bottom instead of left to right.
    pub vertical: bool,
    /// The comp the chart starts from (None = the active comp).
    pub root: Option<u64>,
    #[serde(skip)]
    pub pan: [f32; 2],
    /// Selected node id.
    #[serde(skip)]
    pub selected: Option<String>,
}

impl Default for FlowOptions {
    fn default() -> Self {
        FlowOptions { layers: false, effects: false, solids: true, vertical: false, root: None, pan: [0.0, 0.0], selected: None }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct FlowNode {
    /// `item:<id>`, `layer:<comp>:<layer>` or `fx:<comp>:<layer>:<uid>`.
    pub id: String,
    /// `comp`, `footage`, `solid`, `layer` or `effect`.
    pub kind: &'static str,
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub item: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub comp: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub layer: Option<u64>,
    /// Distance from the root (column, or row when vertical).
    pub depth: usize,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct FlowGraph {
    pub nodes: Vec<FlowNode>,
    /// (from, to) node ids: from a comp to what it uses.
    pub edges: Vec<(String, String)>,
}

/// Build the flowchart of `root` (a comp). Shared items appear once; cycles stop at the comp
/// already on the path.
pub fn build(project: &Project, root: ItemId, opts: &FlowOptions) -> FlowGraph {
    struct B<'a> {
        p: &'a Project,
        o: &'a FlowOptions,
        nodes: Vec<FlowNode>,
        index: BTreeMap<String, usize>,
        edges: Vec<(String, String)>,
        stack: BTreeSet<u64>,
    }
    impl B<'_> {
        fn node(&mut self, n: FlowNode) -> String {
            let id = n.id.clone();
            match self.index.get(&id) {
                Some(&i) => self.nodes[i].depth = self.nodes[i].depth.min(n.depth),
                None => {
                    self.index.insert(id.clone(), self.nodes.len());
                    self.nodes.push(n);
                }
            }
            id
        }
        fn edge(&mut self, a: &str, b: &str) {
            let e = (a.to_string(), b.to_string());
            if !self.edges.contains(&e) {
                self.edges.push(e);
            }
        }
        fn item(&mut self, id: ItemId, depth: usize) -> Option<String> {
            let it = self.p.item(id)?;
            let kind = match &it.kind {
                ItemKind::Comp(_) => "comp",
                ItemKind::Footage(_) => "footage",
                ItemKind::Solid(_) => "solid",
                ItemKind::Folder => return None,
            };
            if kind == "solid" && !self.o.solids {
                return None;
            }
            let seen = self.index.contains_key(&format!("item:{}", id.0));
            let nid = self.node(FlowNode { id: format!("item:{}", id.0), kind, label: it.name.clone(), item: Some(id.0), comp: None, layer: None, depth });
            if kind == "comp" && !seen && !self.stack.contains(&id.0) {
                self.comp(id, depth);
            }
            Some(nid)
        }
        fn comp(&mut self, cid: ItemId, depth: usize) {
            let Some(c) = self.p.comp(cid) else { return };
            self.stack.insert(cid.0);
            let from = format!("item:{}", cid.0);
            for l in &c.layers {
                let src_depth = if self.o.layers { depth + 2 } else { depth + 1 };
                let mut parent = from.clone();
                if self.o.layers {
                    let lid = self.node(FlowNode {
                        id: format!("layer:{}:{}", cid.0, l.id.0),
                        kind: "layer",
                        label: l.name.clone(),
                        item: None,
                        comp: Some(cid.0),
                        layer: Some(l.id.0),
                        depth: depth + 1,
                    });
                    self.edge(&from, &lid);
                    if self.o.effects
                        && let Some(fx) = l.effects()
                    {
                        for g in fx.groups() {
                            let fid = self.node(FlowNode {
                                id: format!("fx:{}:{}:{}", cid.0, l.id.0, g.uid),
                                kind: "effect",
                                label: g.name.clone(),
                                item: None,
                                comp: Some(cid.0),
                                layer: Some(l.id.0),
                                depth: depth + 2,
                            });
                            self.edge(&lid, &fid);
                        }
                    }
                    parent = lid;
                }
                let src = match l.source {
                    LayerSource::Footage { item } | LayerSource::Comp { item } | LayerSource::Solid { item } | LayerSource::Model { item } => Some(item),
                    _ => None,
                };
                if let Some(item) = src {
                    // A comp already on the path: link back to it, don't descend (cycle).
                    if self.stack.contains(&item.0) {
                        self.edge(&parent, &format!("item:{}", item.0));
                    } else if let Some(sid) = self.item(item, src_depth) {
                        self.edge(&parent, &sid);
                    }
                }
            }
            self.stack.remove(&cid.0);
        }
    }
    let mut b = B { p: project, o: opts, nodes: vec![], index: BTreeMap::new(), edges: vec![], stack: BTreeSet::new() };
    b.item(root, 0);
    FlowGraph { nodes: b.nodes, edges: b.edges }
}

const NODE_W: f32 = 168.0;
const NODE_H: f32 = 34.0;

/// Node positions (top-left, before panning): one column (or row) per depth, in discovery order.
pub fn layout(g: &FlowGraph, vertical: bool) -> Vec<[f32; 2]> {
    let mut slot: BTreeMap<usize, usize> = BTreeMap::new();
    g.nodes
        .iter()
        .map(|n| {
            let k = slot.entry(n.depth).or_insert(0);
            let i = *k as f32;
            *k += 1;
            let d = n.depth as f32;
            if vertical { [i * (NODE_W + 24.0), d * (NODE_H + 46.0)] } else { [d * (NODE_W + 60.0), i * (NODE_H + 14.0)] }
        })
        .collect()
}

/// The comp the panel charts.
pub fn root(app: &EffectcraftApp) -> Option<ItemId> {
    app.ui.flowchart.root.map(ItemId).filter(|r| app.session.project.comp(*r).is_some()).or_else(|| app.session.active_comp_id())
}

/// `flowchart.options` / `flowchart.graph` (UI commands).
pub fn command(app: &mut EffectcraftApp, id: &str, p: &Value) -> Result<Value, String> {
    let o = &mut app.ui.flowchart;
    let b = |k: &str| p.get(k).and_then(Value::as_bool);
    if let Some(v) = b("layers") {
        o.layers = v;
    }
    if let Some(v) = b("effects") {
        o.effects = v;
    }
    if let Some(v) = b("solids") {
        o.solids = v;
    }
    if let Some(d) = p.get("direction").and_then(Value::as_str) {
        o.vertical = match d {
            "lr" | "leftToRight" => false,
            "tb" | "topToBottom" => true,
            _ => return Err(format!("direction: lr|tb, not `{d}`")),
        };
    }
    if let Some(c) = p.get("comp") {
        let cid = match c {
            Value::Number(n) => n.as_u64().map(ItemId),
            Value::String(s) => app.session.project.find_by_name(s).map(|i| i.id),
            _ => None,
        };
        let cid = cid.filter(|c| app.session.project.comp(*c).is_some()).ok_or("comp: a composition id or name")?;
        app.ui.flowchart.root = Some(cid.0);
    }
    let o = app.ui.flowchart.clone();
    if id == "flowchart.graph" {
        let r = root(app).ok_or("no composition")?;
        return Ok(serde_json::to_value(build(&app.session.project, r, &o)).unwrap_or_default());
    }
    Ok(json!({"layers": o.layers, "effects": o.effects, "solids": o.solids, "direction": if o.vertical { "tb" } else { "lr" }, "root": o.root}))
}

fn kind_style(kind: &str) -> (Icon, Color32) {
    match kind {
        "comp" => (Icon::Comp, Color32::from_rgb(0x3d, 0x5a, 0x8c)),
        "footage" => (Icon::Footage, Color32::from_rgb(0x3c, 0x6e, 0x58)),
        "solid" => (Icon::Solid, Color32::from_rgb(0x6a, 0x58, 0x3a)),
        "effect" => (Icon::Fx, Color32::from_rgb(0x6c, 0x44, 0x6e)),
        _ => (Icon::Null, Color32::from_rgb(0x44, 0x48, 0x52)),
    }
}

/// Draw the Flowchart panel.
pub fn show(app: &mut EffectcraftApp, ui: &mut egui::Ui, rect: Rect) {
    let t = app.tokens;
    let ctx = ui.ctx().clone();
    let p = ui.painter().with_clip_rect(rect);
    p.rect_filled(rect, 0.0, t.tl_bg);
    let Some(rid) = root(app) else {
        p.text(rect.center(), Align2::CENTER_CENTER, "(no composition)", Tokens::ui(12.0), t.text_faint);
        return;
    };
    // ---- toolbar: switches and direction.
    let bar = Rect::from_min_size(rect.min, vec2(rect.width(), 30.0));
    p.rect_filled(bar, 0.0, t.panel_bg);
    let root_name = app.session.project.item(rid).map(|i| i.name.clone()).unwrap_or_default();
    p.text(pos2(bar.min.x + 10.0, bar.center().y), Align2::LEFT_CENTER, tr_args("Flowchart: {}", &[&root_name]), Tokens::medium(12.0), t.text);
    let mut x = bar.max.x - 8.0;
    let mut cmds: Vec<(String, Value)> = vec![];
    let o = app.ui.flowchart.clone();
    for (key, label, on) in [
        ("direction", if o.vertical { "Top to Bottom" } else { "Left to Right" }, false),
        ("effects", "Effects", o.effects),
        ("layers", "Layers", o.layers),
        ("solids", "Solids", o.solids),
    ] {
        let w = 18.0 + label.len() as f32 * 6.6;
        let r = Rect::from_min_size(pos2(x - w, bar.min.y + 5.0), vec2(w, 20.0));
        x -= w + 6.0;
        if widgets::text_button(ui, r, label, on, &t, egui::Id::unique(("flow-opt", key))).clicked() {
            let v = if key == "direction" { json!({"direction": if o.vertical { "lr" } else { "tb" }}) } else { json!({key: !on}) };
            cmds.push(("flowchart.options".into(), v));
        }
        app.auto.add(&format!("flowchart.option.{key}"), r, label);
    }

    // ---- graph.
    let body = Rect::from_min_max(pos2(rect.min.x, bar.max.y), rect.max);
    let g = build(&app.session.project, rid, &o);
    let pos = layout(&g, o.vertical);
    let origin = body.min + vec2(24.0 + o.pan[0], 24.0 + o.pan[1]);
    let bg = ui.interact(body, egui::Id::unique("flow-bg"), Sense::click_and_drag());
    if bg.dragged() {
        let d = bg.drag_delta();
        app.ui.flowchart.pan[0] += d.x;
        app.ui.flowchart.pan[1] += d.y;
    }
    if ui.rect_contains_pointer(body) {
        let s = ui.input(|i| i.smooth_scroll_delta);
        if s != egui::Vec2::ZERO {
            app.ui.flowchart.pan[0] += s.x;
            app.ui.flowchart.pan[1] += s.y;
        }
    }
    if bg.clicked() {
        app.ui.flowchart.selected = None;
    }
    let gp = p.with_clip_rect(body);
    let rect_of = |i: usize| Rect::from_min_size(origin + vec2(pos[i][0], pos[i][1]), vec2(NODE_W, NODE_H));
    let idx: BTreeMap<&str, usize> = g.nodes.iter().enumerate().map(|(i, n)| (n.id.as_str(), i)).collect();
    // Edges first (curves with an arrowhead at the target).
    for (a, b) in &g.edges {
        let (Some(&ia), Some(&ib)) = (idx.get(a.as_str()), idx.get(b.as_str())) else { continue };
        let (ra, rb) = (rect_of(ia), rect_of(ib));
        let back = g.nodes[ib].depth <= g.nodes[ia].depth;
        let (s, e, n) = if o.vertical { (ra.center_bottom(), rb.center_top(), vec2(0.0, 1.0)) } else { (ra.right_center(), rb.left_center(), vec2(1.0, 0.0)) };
        let k = if o.vertical { (e.y - s.y).abs() * 0.5 } else { (e.x - s.x).abs() * 0.5 }.max(30.0);
        let col = if back { Color32::from_rgb(0xd0, 0x80, 0x40) } else { t.text_faint };
        let curve = egui::epaint::CubicBezierShape::from_points_stroke([s, s + n * k, e - n * k, e], false, Color32::TRANSPARENT, Stroke::new(1.2, col));
        gp.add(curve);
        let (l, r) = if o.vertical { (vec2(-4.0, -7.0), vec2(4.0, -7.0)) } else { (vec2(-7.0, -4.0), vec2(-7.0, 4.0)) };
        gp.add(egui::Shape::convex_polygon(vec![e, e + l, e + r], col, Stroke::NONE));
    }
    let cmd_down = ui.input(|i| i.modifiers.command || i.modifiers.ctrl);
    for (i, n) in g.nodes.iter().enumerate() {
        let r = rect_of(i);
        if !r.intersects(body) {
            continue;
        }
        let (icon, fill) = kind_style(n.kind);
        let selected = o.selected.as_deref() == Some(n.id.as_str());
        let resp = ui.interact(r.intersect(body), egui::Id::unique(("flow-node", &n.id)), Sense::click());
        let fill = if resp.hovered() { fill.gamma_multiply(1.25) } else { fill };
        gp.rect_filled(r, 5.0, fill);
        gp.rect_stroke(
            r,
            5.0,
            Stroke::new(if selected { 2.0 } else { 1.0 }, if selected { t.accent } else { Color32::from_black_alpha(120) }),
            StrokeKind::Inside,
        );
        icons::paint(&gp, Rect::from_center_size(pos2(r.min.x + 16.0, r.center().y), vec2(14.0, 14.0)), icon, Color32::WHITE);
        gp.with_clip_rect(r.shrink(2.0).intersect(body)).text(
            pos2(r.min.x + 30.0, r.center().y),
            Align2::LEFT_CENTER,
            &n.label,
            Tokens::ui(12.0),
            Color32::WHITE,
        );
        app.auto.add(&format!("flowchart.node.{}", n.id), r, &n.label);
        if resp.clicked() {
            app.ui.flowchart.selected = Some(n.id.clone());
            if cmd_down {
                // Reveal: items in the Project panel, layers selected in their comp.
                match (n.item, n.comp, n.layer) {
                    (Some(item), ..) => {
                        cmds.push(("project.select".into(), json!({"items": [item]})));
                        cmds.push(("window.panel".into(), json!({"panel": "project"})));
                    }
                    (None, Some(c), Some(l)) => {
                        cmds.push(("comp.open".into(), json!({"comp": c})));
                        cmds.push(("layer.select".into(), json!({"layers": [l]})));
                    }
                    _ => {}
                }
            }
        }
        if resp.double_clicked() {
            match (n.kind, n.item, n.comp) {
                ("comp", Some(c), _) => cmds.push(("comp.open".into(), json!({"comp": c}))),
                (_, _, Some(c)) => cmds.push(("comp.open".into(), json!({"comp": c}))),
                _ => {}
            }
        }
        resp.on_hover_text(tr_args("{} — double-click to open, {}-click to reveal", &[&n.label, &(if cfg!(target_os = "macos") { "Cmd" } else { "Ctrl" })]));
    }
    for (id, params) in cmds {
        if let Err(e) = crate::menus::invoke(app, &ctx, &id, params) {
            app.ui.status = e;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use effectcraft_engine::Session;

    /// Main ⊃ Mid ⊃ Inner (with a solid), Main also uses Inner directly.
    fn session() -> (Session, ItemId, ItemId, ItemId) {
        let mut s = Session::default();
        let id = |v: Value| ItemId(v["id"].as_u64().or_else(|| v["comp"].as_u64()).or_else(|| v["item"].as_u64()).unwrap());
        let inner = id(s.execute("comp.new", json!({"name": "Inner", "width": 100, "height": 100, "duration": 2})).unwrap());
        s.execute("layer.newSolid", json!({"name": "Red", "color": "#ff0000", "width": 100, "height": 100})).unwrap();
        s.execute("layer.select", json!({"layers": []})).unwrap();
        let mid = id(s.execute("comp.new", json!({"name": "Mid", "width": 100, "height": 100, "duration": 2})).unwrap());
        s.execute("layer.addItem", json!({"item": inner.0})).unwrap();
        let main = id(s.execute("comp.new", json!({"name": "Main", "width": 100, "height": 100, "duration": 2})).unwrap());
        s.execute("layer.addItem", json!({"item": mid.0})).unwrap();
        s.execute("layer.addItem", json!({"item": inner.0})).unwrap();
        s.execute("layer.newText", json!({"text": "Title"})).unwrap();
        (s, main, mid, inner)
    }

    #[test]
    fn nested_comps_make_a_graph() {
        let (s, main, mid, inner) = session();
        let g = build(&s.project, main, &FlowOptions::default());
        let label = |id: &str| g.nodes.iter().find(|n| n.id == id).map(|n| (n.label.clone(), n.depth));
        assert_eq!(label(&format!("item:{}", main.0)), Some(("Main".into(), 0)));
        assert_eq!(label(&format!("item:{}", mid.0)), Some(("Mid".into(), 1)));
        // Inner is reached at depth 1 (directly) and 2 (through Mid): drawn once, at depth 1.
        assert_eq!(label(&format!("item:{}", inner.0)), Some(("Inner".into(), 1)));
        assert_eq!(g.nodes.iter().filter(|n| n.label == "Inner").count(), 1);
        let has = |a: ItemId, b: ItemId| g.edges.contains(&(format!("item:{}", a.0), format!("item:{}", b.0)));
        assert!(has(main, mid) && has(mid, inner) && has(main, inner));
        // The solid is shown (Solids on) and hidden with Solids off; text layers have no source.
        assert!(g.nodes.iter().any(|n| n.kind == "solid"));
        let g2 = build(&s.project, main, &FlowOptions { solids: false, ..Default::default() });
        assert!(!g2.nodes.iter().any(|n| n.kind == "solid"));
        // Layers between comps and sources; effects under layers.
        let g3 = build(&s.project, main, &FlowOptions { layers: true, ..Default::default() });
        assert!(g3.nodes.iter().any(|n| n.kind == "layer" && n.label == "Title"));
        let mid_layer = g3.nodes.iter().find(|n| n.kind == "layer" && n.comp == Some(main.0) && n.label == "Mid").unwrap();
        assert!(g3.edges.contains(&(mid_layer.id.clone(), format!("item:{}", mid.0))));
        // Layout: one column per depth, no overlaps.
        let pos = layout(&g3, false);
        for i in 0..pos.len() {
            for j in i + 1..pos.len() {
                assert!(pos[i] != pos[j], "{} and {} overlap", g3.nodes[i].label, g3.nodes[j].label);
            }
        }
    }

    #[test]
    fn cycles_are_safe() {
        let (mut s, main, mid, _) = session();
        // Make Mid contain Main: Main → Mid → Main.
        s.execute("comp.open", json!({"comp": mid.0})).unwrap();
        // Adding a cyclic layer may be refused by the engine; build a cycle in the model directly.
        let mut p = (*s.project).clone();
        let mut c = (*p.comp(mid).unwrap()).clone();
        let mut l = c.layers[0].clone();
        l.id = effectcraft_engine::project::LayerId(9999);
        l.source = LayerSource::Comp { item: main };
        c.layers.push(l);
        if let Some(ItemKind::Comp(arc)) = p.items.get_mut(&mid).map(|i| &mut i.kind) {
            *arc = std::sync::Arc::new(c);
        }
        let g = build(&p, main, &FlowOptions { layers: true, effects: true, ..Default::default() });
        assert_eq!(g.nodes.iter().filter(|n| n.kind == "comp" && n.label == "Main").count(), 1);
        // The back edge points at the root.
        assert!(g.edges.iter().any(|(_, b)| *b == format!("item:{}", main.0)));
        assert!(g.nodes.len() < 50);
    }

    #[test]
    fn options_command() {
        let (s, main, ..) = session();
        let mut app = EffectcraftApp::new(s);
        let r = command(&mut app, "flowchart.options", &json!({"layers": true, "direction": "tb"})).unwrap();
        assert_eq!(r["direction"], "tb");
        assert!(app.ui.flowchart.layers && app.ui.flowchart.vertical);
        assert!(command(&mut app, "flowchart.options", &json!({"direction": "diagonal"})).is_err());
        let g = command(&mut app, "flowchart.graph", &json!({"comp": "Main"})).unwrap();
        assert_eq!(g["nodes"][0]["id"], format!("item:{}", main.0));
        assert!(g["edges"].as_array().is_some_and(|e| !e.is_empty()));
    }
}
