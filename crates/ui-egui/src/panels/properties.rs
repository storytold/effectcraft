//! Properties panel: the selected layer's essential properties in one context-sensitive stack,
//! like After Effects' Properties panel. Sections appear by layer type:
//!
//! - **Layer Transform** (all layers but cameras/lights): keyframe navigator (◀ ◆ ▶) or
//!   stopwatch, scrubbable values, linked Scale, `Nx+N°` Rotation (revolutions and degrees scrub on their own), Reset.
//! - **Text** (text layers): font family/style, size, leading, tracking, stroke width, fill and
//!   stroke with enable checkboxes; "More" opens the Character panel.
//! - **Paragraph** (text layers): the seven alignment buttons; "More" opens the Paragraph panel.
//! - **Text Animation** (text layers): Add Animator menu.

use effectcraft_engine::keyframe::{Justify, TextDoc, Value};
use effectcraft_engine::project::{Layer, LayerSource, Property};
use effectcraft_engine::render::EvalCtx;
use egui::{Align2, Color32, Rect, Sense, Stroke, pos2, vec2};
use serde_json::json;

use crate::dock::PanelKind;
use crate::i18n::tr;
use crate::icons::{self, Icon};
use crate::theme::Tokens;
use crate::{EffectcraftApp, widgets};

type Actions = Vec<(String, serde_json::Value)>;

const ROW_H: f32 = 22.0;
const PAD: f32 = 12.0;

/// Text Animation › Add Animator entries: (menu label, `layer.addTextAnimator` property; `""`
/// for separators). The engine's list (`build::TEXT_ANIMATOR_KINDS`) after Enable Per-character 3D.
fn animator_entries(per_char: bool) -> Vec<(String, &'static str)> {
    let mut v = vec![((if per_char { "Disable Per-character 3D" } else { "Enable Per-character 3D" }).to_string(), "perChar3d"), ("-".to_string(), "")];
    for (k, l) in effectcraft_engine::project::build::TEXT_ANIMATOR_KINDS {
        v.push((l.to_string(), if *k == "-" { "" } else { k }));
    }
    v
}

/// AE's seven Paragraph alignment buttons: (justify, `layer.setText` key).
const ALIGNS: [(Justify, &str); 7] = [
    (Justify::Left, "left"),
    (Justify::Center, "center"),
    (Justify::Right, "right"),
    (Justify::JustifyLastLeft, "justifyLeft"),
    (Justify::JustifyLastCenter, "justifyCenter"),
    (Justify::JustifyLastRight, "justifyRight"),
    (Justify::JustifyAll, "justifyAll"),
];

/// Value text the way AE prints it: whole numbers without decimals, otherwise one decimal.
fn decimals(v: f64) -> usize {
    if (v - v.round()).abs() < 1e-6 { 0 } else { 1 }
}

pub fn show(app: &mut EffectcraftApp, ui: &mut egui::Ui, rect: Rect) {
    let t = app.tokens;
    let p = ui.painter().with_clip_rect(rect);
    let ctx = ui.ctx().clone();
    let comp = app.session.active_comp_arc();
    let layer = comp.as_ref().and_then(|c| app.session.state.selected_layers.first().and_then(|id| c.layer(*id)).cloned());
    let (Some(comp), Some(cid), Some(layer)) = (comp, app.session.active_comp_id(), layer) else {
        p.text(rect.center(), Align2::CENTER_CENTER, crate::i18n::tr("Select a layer to see its properties"), Tokens::ui(12.0), t.text_faint);
        return;
    };
    let project = app.session.project.clone();
    let expr = app.session.expr.clone();
    let now = app.session.time();
    let ectx = EvalCtx { project: &project, comp_id: cid, comp: &comp, time: now, expr: expr.as_deref(), footage: None };
    let x0 = rect.min.x + PAD;
    let w = rect.width() - 2.0 * PAD;
    let scroll = widgets::PanelScroll::begin(ui, egui::Id::unique("props-scroll"), rect);
    let top = rect.min.y + 8.0 - scroll.offset;
    let mut y = top;
    let mut actions: Actions = vec![];

    // Several layers selected: "(3) Selected Objects", as in After Effects. The rows show the
    // first layer; the stopwatch, keyframe button and value edits apply to all of them (#355).
    let selected = app.session.state.selected_layers.iter().filter(|id| comp.layer(**id).is_some()).count();
    if selected > 1 {
        let n = selected.to_string();
        let label = crate::i18n::tr_args("({}) Selected Objects", &[&n]);
        p.text(pos2(x0, y + ROW_H / 2.0), Align2::LEFT_CENTER, &label, Tokens::semibold(12.0), t.text);
        app.auto.add("properties.selectedObjects", Rect::from_min_size(pos2(x0, y), vec2(w, ROW_H)), &label);
        y += ROW_H + 4.0;
    }

    // ---------------------------------------------------------------- Layer Transform
    if let Some(tr) = layer.transform().cloned() {
        section_header(app, ui, &p, x0, w, y, "Layer Transform", "properties.transform");
        let rr = Rect::from_min_size(pos2(x0 + w - 40.0, y), vec2(40.0, ROW_H));
        let resp = ui.interact(rr, egui::Id::unique("props-reset"), Sense::click());
        p.text(rr.right_center(), Align2::RIGHT_CENTER, crate::i18n::tr("Reset"), Tokens::ui(12.0), if resp.hovered() { t.accent_hover } else { t.accent });
        app.auto.add("properties.transform.reset", rr, "Reset Transform");
        if resp.clicked() {
            for pr in tr.props() {
                actions.push(("prop.reset".into(), json!({"layer": layer.id.0, "prop": pr.uid})));
            }
        }
        y += ROW_H + 2.0;
        for pr in tr.props() {
            if pr.three_d_only && !layer.is_3d() {
                continue;
            }
            transform_row(app, ui, &p, &layer, pr, &ectx, Rect::from_min_size(pos2(x0, y), vec2(w, ROW_H)), &mut actions);
            y += ROW_H;
        }
        y += 10.0;
    }

    // ---------------------------------------------------------------- Text / Paragraph / Animation
    // While the layer's text is edited in the viewer, the text sections show and change the
    // selected characters.
    let text_target = crate::panels::text_panels::text_target(app).filter(|tt| tt.layer == layer.id.0);
    if matches!(layer.source, LayerSource::Text)
        && let Some(doc) = text_target.as_ref().map(|tt| tt.doc.clone()).or_else(|| effectcraft_engine::render::text::source_text(&ectx, &layer))
    {
        divider(&p, x0, w, y - 4.0, &t);
        y = text_section(app, ui, &p, &layer, &doc, x0, w, y + 4.0, &mut actions);
        divider(&p, x0, w, y - 4.0, &t);
        y = paragraph_section(app, ui, &p, &layer, &doc, x0, w, y + 4.0, &mut actions);
        divider(&p, x0, w, y - 4.0, &t);
        section_header(app, ui, &p, x0, w, y + 4.0, "Text Animation", "properties.textAnimation");
        y += ROW_H + 10.0;
        // Both buttons grow with their (translated) labels; English keeps its 116 / 84 px.
        let text_w = |s: &str| p.layout_no_wrap(s.to_string(), Tokens::ui(12.0), t.text).size().x;
        let br = Rect::from_min_size(pos2(x0, y), vec2(116f32.max(text_w(crate::i18n::tr("Add Animator")) + 36.0), 24.0));
        let resp = ui.interact(br, egui::Id::unique("props-add-animator"), Sense::click());
        p.rect_stroke(br, 4.0, Stroke::new(1.0, if resp.hovered() { t.text_dim } else { t.field_border }), egui::StrokeKind::Inside);
        icons::paint(&p, Rect::from_center_size(pos2(br.min.x + 14.0, br.center().y), vec2(10.0, 10.0)), Icon::Plus, t.text);
        p.text(pos2(br.min.x + 26.0, br.center().y), Align2::LEFT_CENTER, crate::i18n::tr("Add Animator"), Tokens::ui(12.0), t.text);
        app.auto.add("properties.addAnimator", br, "Add Animator");
        let pop = egui::Id::unique("props-add-animator-pop");
        if resp.clicked() {
            widgets::open_popup(ui, pop);
        }
        let per_char = layer.props.prop("text/perChar3d").is_some_and(|q| q.value.as_bool());
        let entries = animator_entries(per_char);
        // The entries are the English source strings; the menu draws them translated.
        let labels: Vec<String> = entries.iter().map(|(l, _)| tr(l).to_string()).collect();
        if let Some(i) = widgets::popup_menu(ui, pop, br.left_bottom(), &labels, None) {
            match entries[i].1 {
                "" => {}
                "perChar3d" => actions.push(("layer.enablePerChar3D".into(), json!({"layer": layer.id.0, "enabled": !per_char}))),
                k => actions.push(("layer.addTextAnimator".into(), json!({"layer": layer.id.0, "property": k}))),
            }
        }
        // Our own presets (original work, see presets/text_animators.json).
        let pr = Rect::from_min_size(pos2(br.max.x + 8.0, y), vec2(84f32.max(text_w(crate::i18n::tr("Presets")) + 16.0), 24.0));
        let presp = ui.interact(pr, egui::Id::unique("props-text-presets"), Sense::click());
        p.rect_stroke(pr, 4.0, Stroke::new(1.0, if presp.hovered() { t.text_dim } else { t.field_border }), egui::StrokeKind::Inside);
        p.text(pr.center(), Align2::CENTER_CENTER, crate::i18n::tr("Presets"), Tokens::ui(12.0), t.text);
        app.auto.add("properties.textPresets", pr, "Text Animation Presets");
        let ppop = egui::Id::unique("props-text-presets-pop");
        if presp.clicked() {
            widgets::open_popup(ui, ppop);
        }
        let presets = effectcraft_engine::text_presets();
        let names: Vec<String> = presets.iter().map(|(_, n)| n.clone()).collect();
        if let Some(i) = widgets::popup_menu(ui, ppop, pr.left_bottom(), &names, None) {
            actions.push(("layer.applyTextPreset".into(), json!({"layer": layer.id.0, "preset": presets[i].0})));
        }
        y += 24.0;
    }

    y += 40.0;
    scroll.end(ui, &mut app.auto, "properties.scroll", y - top, &t);
    for (id, params) in actions {
        let params = match &text_target {
            Some(tt) if id == "layer.setText" => tt.params(params),
            _ => params,
        };
        if let Err(e) = crate::menus::invoke(app, &ctx, &id, params) {
            app.ui.status = e;
        }
    }
}

fn section_header(app: &mut EffectcraftApp, ui: &mut egui::Ui, p: &egui::Painter, x0: f32, w: f32, y: f32, title: &str, auto: &str) {
    let t = app.tokens;
    p.text(pos2(x0, y + ROW_H / 2.0), Align2::LEFT_CENTER, title, Tokens::semibold(12.0), t.text);
    let r = Rect::from_min_size(pos2(x0, y), vec2(w, ROW_H));
    let _ = ui;
    app.auto.add(auto, r, title);
}

fn divider(p: &egui::Painter, x0: f32, w: f32, y: f32, t: &Tokens) {
    p.line_segment([pos2(x0 - 4.0, y), pos2(x0 + w + 4.0, y)], Stroke::new(1.0, t.separator));
}

fn triangle(p: &egui::Painter, c: egui::Pos2, left: bool, col: Color32) {
    let (dx, h) = (if left { -3.5 } else { 3.5 }, 4.0);
    p.add(egui::Shape::convex_polygon(vec![pos2(c.x + dx, c.y), pos2(c.x - dx, c.y - h), pos2(c.x - dx, c.y + h)], col, Stroke::NONE));
}

/// One transform row: keyframe navigator or stopwatch, name, value fields.
#[allow(clippy::too_many_arguments)]
fn transform_row(app: &mut EffectcraftApp, ui: &mut egui::Ui, p: &egui::Painter, layer: &Layer, pr: &Property, ectx: &EvalCtx, r: Rect, actions: &mut Actions) {
    let t = app.tokens;
    let cy = r.center().y;
    let uid = pr.uid;
    let lid = layer.id.0;
    let base = format!("properties.prop.{uid}");
    if pr.is_animated() {
        // ◀ ◆ ▶: previous key, add/remove key at the CTI, next key.
        let now = ectx.time;
        let half = ectx.comp.frame_duration().0 / 2;
        let on_key = pr.keys.iter().any(|k| (layer.comp_time(k.time).0 - now.0).abs() <= half);
        let cells = [("prev", r.min.x + 4.0), ("key", r.min.x + 16.0), ("next", r.min.x + 28.0)];
        for (what, cx) in cells {
            let cr = Rect::from_center_size(pos2(cx, cy), vec2(12.0, 16.0));
            let resp = ui.interact(cr, egui::Id::unique(("props-nav", uid, what)), Sense::click());
            let col = if resp.hovered() { t.text } else { t.text_dim };
            match what {
                "prev" => triangle(p, cr.center(), true, col),
                "next" => triangle(p, cr.center(), false, col),
                _ => {
                    let kr = Rect::from_center_size(cr.center(), vec2(10.0, 10.0));
                    if on_key {
                        icons::paint(p, kr, Icon::Keyframe, t.keyframe);
                    } else {
                        let c = kr.center();
                        let d = 4.5;
                        p.add(egui::Shape::closed_line(
                            vec![pos2(c.x, c.y - d), pos2(c.x + d, c.y), pos2(c.x, c.y + d), pos2(c.x - d, c.y)],
                            Stroke::new(1.0, col),
                        ));
                    }
                }
            }
            app.auto.add(&format!("{base}.{what}Key"), cr, &pr.name);
            if resp.clicked() {
                match what {
                    "prev" => actions.push(("time.go".into(), json!({"to": "prevKey", "prop": uid}))),
                    "next" => actions.push(("time.go".into(), json!({"to": "nextKey", "prop": uid}))),
                    _ => actions.push(("prop.toggleKey".into(), json!({"layer": lid, "prop": uid, "selected": true}))),
                }
            }
        }
    } else {
        let swr = Rect::from_center_size(pos2(r.min.x + 16.0, cy), vec2(14.0, 14.0));
        let resp = ui.interact(swr, egui::Id::unique(("props-sw", uid)), Sense::click());
        icons::paint(p, swr, Icon::Stopwatch, if resp.hovered() { t.text } else { t.text_dim });
        app.auto.add(&format!("{base}.stopwatch"), swr, &pr.name);
        if resp.clicked() {
            actions.push(("prop.toggleAnimation".into(), json!({"layer": lid, "prop": uid, "selected": true})));
        }
    }
    p.text(pos2(r.min.x + 42.0, cy), Align2::LEFT_CENTER, &pr.name, Tokens::ui(12.0), t.text);
    let vx = r.min.x + (r.width() * 0.5).max(130.0);
    let label = Rect::from_min_max(pos2(r.min.x + 42.0, r.min.y), pos2(vx - 20.0, r.max.y));
    let resp = ui.interact(label, egui::Id::unique(("props-label", uid)), Sense::click());
    app.auto.add(&format!("{base}.label"), label, &pr.name);
    if resp.double_clicked() {
        app.ui.timeline.search.clear();
        app.ui.timeline.scroll_to = Some(crate::state::ScrollTo::Property(uid));
        if let Err(e) = crate::menus::frontend(app, ui.ctx(), "timeline.revealProps", json!({"props": [{"layer": lid, "prop": uid}]})) {
            app.ui.status = e;
        }
        app.show_panel(PanelKind::Timeline);
    }
    let merge = format!("props-{uid}");
    // With several layers selected the edit applies to all: a scrub by the same change, a typed
    // value as it is.
    let scrub = ui.ctx().dragged_id().is_some() && ui.input(|i| i.pointer.is_decidedly_dragging());
    let set = |actions: &mut Actions, v: serde_json::Value| {
        actions.push(("prop.set".into(), json!({"layer": lid, "prop": uid, "value": v, "merge": merge, "selected": true, "offset": scrub})))
    };
    let value = ectx.value(layer, pr);
    let is_scale = pr.name == "Scale";
    let pct = is_scale || pr.name == "Opacity";
    let suffix = if pct { "%" } else { "" };
    match &value {
        Value::Scalar(v) if pr.name == "Rotation" || pr.name.ends_with(" Rotation") => {
            let (_, deg) = super::fx_widgets::split_angle(*v);
            let (rr, dr, nv) = super::fx_widgets::angle_field(ui, pos2(vx, cy - 9.0), egui::Id::unique(("props-v", uid)), *v, decimals(deg), &t);
            app.auto.add(&format!("{base}.value"), dr, &pr.name);
            app.auto.add(&format!("{base}.revolutions"), rr, &pr.name);
            if let Some(nv) = nv {
                set(actions, json!(nv));
            }
        }
        Value::Scalar(v) => {
            let range = if pr.name == "Opacity" { (0.0, 100.0) } else { (-1e9, 1e9) };
            let (vr, nv, _) = widgets::hot_number_at(ui, pos2(vx, cy - 9.0), egui::Id::unique(("props-v", uid)), *v, 0.5, range, decimals(*v), suffix, &t);
            app.auto.add(&format!("{base}.value"), vr, &pr.name);
            if let Some(nv) = nv {
                set(actions, json!(nv));
            }
        }
        Value::Vec2(_) | Value::Vec3(_) => {
            let c = value.components();
            let n = if pr.shown_dims > 0 && !(layer.is_3d() && c.len() == 3) { pr.shown_dims as usize } else { c.len() };
            let mut x = vx;
            // Scale: the chain link (Constrain Proportions, on by default), sharing the
            // Timeline's per-property state so both panels agree (#956).
            let linkable = pr.match_id == "scale";
            let linked = linkable && !app.ui.timeline.unlinked.contains(&uid);
            if linkable {
                // Constrain Proportions toggle, left of the values.
                let lr = Rect::from_center_size(pos2(x - 12.0, cy), vec2(12.0, 12.0));
                let resp = ui.interact(lr, egui::Id::unique(("props-link", uid)), Sense::click()).on_hover_text(tr("Constrain Proportions"));
                icons::paint(p, lr, Icon::Link, if linked { t.accent } else { t.text_faint });
                app.auto.add(&format!("{base}.constrain"), lr, "Constrain Proportions");
                if resp.clicked() && !app.ui.timeline.unlinked.remove(&uid) {
                    app.ui.timeline.unlinked.insert(uid);
                }
            }
            for d in 0..n.min(c.len()) {
                let (vr, nv, _) =
                    widgets::hot_number_at(ui, pos2(x, cy - 9.0), egui::Id::unique(("props-v", uid, d)), c[d], 1.0, (-1e9, 1e9), decimals(c[d]), suffix, &t);
                app.auto.add(&format!("{base}.value.{d}"), vr, &pr.name);
                if let Some(nv) = nv {
                    let mut nc = c.clone();
                    // Alt edits one value of a linked pair, as in the Timeline.
                    let shown = if linked && !ui.input(|i| i.modifiers.alt && !widgets::proportional_scrub(i.modifiers)) { n.min(c.len()) } else { 1 };
                    let from = if shown == 1 { d } else { 0 };
                    for (e, v) in widgets::constrained(c.get(from..from + shown).unwrap_or_default(), d - from, nv).into_iter().enumerate() {
                        if let Some(slot) = nc.get_mut(from + e) {
                            *slot = v;
                        }
                    }
                    set(actions, json!(nc));
                    // Typed into these components only: other selected layers keep the rest of
                    // their own value (#1434).
                    if let Some((_, a)) = actions.last_mut() {
                        a["dims"] = json!((from..from + shown).collect::<Vec<_>>());
                    }
                }
                x = vr.max.x + 10.0;
            }
        }
        _ => {}
    }
}

/// Text section; returns the y below it.
#[allow(clippy::too_many_arguments)]
fn text_section(
    app: &mut EffectcraftApp,
    ui: &mut egui::Ui,
    p: &egui::Painter,
    layer: &Layer,
    doc: &TextDoc,
    x0: f32,
    w: f32,
    mut y: f32,
    actions: &mut Actions,
) -> f32 {
    let t = app.tokens;
    let lid = layer.id.0;
    let set = |actions: &mut Actions, v: serde_json::Value| {
        let mut v = v;
        v["layer"] = json!(lid);
        actions.push(("layer.setText".into(), v));
    };
    section_header(app, ui, p, x0, w, y, "Text", "properties.text");
    y += ROW_H + 4.0;
    // Font family and style.
    let fr = Rect::from_min_size(pos2(x0, y), vec2(w, 22.0));
    let pop = egui::Id::unique("props-font-pop");
    if let Some(f) = super::text_panels::font_picker(app, ui, pop, fr, &doc.font, "properties.text.font", true) {
        set(actions, json!({"font": f}));
    }
    y += 28.0;
    let sr = Rect::from_min_size(pos2(x0, y), vec2(w, 22.0));
    let spop = egui::Id::unique("props-style-pop");
    if widgets::dropdown(ui, sr, &doc.style, &t, egui::Id::unique("props-style")).clicked() {
        widgets::open_popup(ui, spop);
    }
    app.auto.add("properties.text.style", sr, "Font style");
    // The family's own styles.
    let styles: Vec<String> = if widgets::popup_is_open(ui, spop) { effectcraft_engine::font_styles(&doc.font) } else { vec![] };
    if let Some(st) = widgets::popup_menu(ui, spop, sr.left_bottom(), &styles, styles.iter().position(|s| *s == doc.style)).and_then(|i| styles.get(i)) {
        set(actions, json!({"style": st}));
    }
    y += 32.0;
    // Size / leading, tracking / stroke width.
    let col2 = x0 + w / 2.0;
    let field = |app: &mut EffectcraftApp, ui: &mut egui::Ui, x: f32, y: f32, glyph: &str, key: &str, v: f64, range: (f64, f64), suffix: &str| {
        p.text(pos2(x, y + 9.0), Align2::LEFT_CENTER, glyph, Tokens::semibold(11.0), t.text_dim);
        let (r, nv, _) = widgets::hot_number_at(ui, pos2(x + 26.0, y), egui::Id::unique(("props-text", key)), v, 0.5, range, decimals(v), suffix, &t);
        app.auto.add(&format!("properties.text.{key}"), r, key);
        nv
    };
    if let Some(v) = field(app, ui, x0, y, "T", "size", doc.size, (1.0, 2000.0), " px") {
        set(actions, json!({"size": v, "merge": "props-text-size"}));
    }
    match doc.leading {
        Some(l) => {
            if let Some(v) = field(app, ui, col2, y, "A", "leading", l, (0.0, 5000.0), " px") {
                set(actions, json!({"leading": v, "merge": "props-text-leading"}));
            }
        }
        None => {
            // Auto Leading: click to switch to an explicit value (120% of the size).
            p.text(pos2(col2, y + 9.0), Align2::LEFT_CENTER, "A", Tokens::semibold(11.0), t.text_dim);
            let r = Rect::from_min_size(pos2(col2 + 26.0, y), vec2(40.0, 18.0));
            let resp = ui.interact(r, egui::Id::unique("props-leading-auto"), Sense::click());
            p.text(
                pos2(r.min.x + 2.0, r.center().y),
                Align2::LEFT_CENTER,
                crate::i18n::tr("Auto"),
                Tokens::ui(12.0),
                if resp.hovered() { t.accent_hover } else { t.hot_text },
            );
            app.auto.add("properties.text.leading", r, "leading");
            if resp.clicked() {
                set(actions, json!({"leading": (doc.size * 1.2).round()}));
            }
        }
    }
    y += 26.0;
    if let Some(v) = field(app, ui, x0, y, "VA", "tracking", doc.tracking, (-1000.0, 10000.0), "") {
        set(actions, json!({"tracking": v.round(), "merge": "props-text-tracking"}));
    }
    if let Some(v) = field(app, ui, col2, y, "W", "strokeWidth", doc.stroke_width, (0.0, 500.0), " px") {
        set(actions, json!({"strokeWidth": v, "merge": "props-text-strokeWidth"}));
    }
    y += 28.0;
    // Fill and Stroke, each with an enable checkbox, swatch and label.
    for (i, (key, label, on, c)) in [("fill", "Fill", doc.apply_fill, doc.fill), ("stroke", "Stroke", doc.apply_stroke, doc.stroke)].into_iter().enumerate() {
        let ry = y + i as f32 * 24.0;
        let cr = Rect::from_min_size(pos2(x0, ry + 2.0), vec2(14.0, 14.0));
        if widgets::checkbox(ui, cr, on, &t, egui::Id::unique(("props-apply", key))).clicked() {
            let k = if key == "fill" { "applyFill" } else { "applyStroke" };
            set(actions, json!({k: !on}));
        }
        app.auto.add(&format!("properties.text.{key}.enabled"), cr, label);
        let sw = Rect::from_min_size(pos2(x0 + 24.0, ry + 1.0), vec2(16.0, 16.0));
        let pop = egui::Id::unique(("props-color-pop", key));
        if widgets::swatch(ui, sw, c, egui::Id::unique(("props-color", key)), &t).clicked() {
            widgets::open_popup(ui, pop);
        }
        app.auto.add(&format!("properties.text.{key}"), sw, label);
        let mut rgb = [c[0], c[1], c[2]];
        if crate::widgets::color_popup(ui, pop, sw.left_bottom(), &mut rgb) {
            set(actions, json!({key: [rgb[0], rgb[1], rgb[2]], "merge": format!("props-text-{key}")}));
        }
        p.text(pos2(sw.max.x + 8.0, sw.center().y), Align2::LEFT_CENTER, label, Tokens::ui(12.0), t.text);
    }
    y += 52.0;
    y = more_button(app, ui, p, x0, y, "props-text-more", "properties.text.more", PanelKind::Character);
    y + 10.0
}

/// Paragraph section; returns the y below it.
#[allow(clippy::too_many_arguments)]
fn paragraph_section(
    app: &mut EffectcraftApp,
    ui: &mut egui::Ui,
    p: &egui::Painter,
    layer: &Layer,
    doc: &TextDoc,
    x0: f32,
    w: f32,
    mut y: f32,
    actions: &mut Actions,
) -> f32 {
    let t = app.tokens;
    section_header(app, ui, p, x0, w, y, "Paragraph", "properties.paragraph");
    y += ROW_H + 4.0;
    for (i, (j, key)) in ALIGNS.into_iter().enumerate() {
        let r = Rect::from_min_size(pos2(x0 + i as f32 * 26.0, y), vec2(22.0, 22.0));
        let on = doc.justify == j;
        let resp = ui.interact(r, egui::Id::unique(("props-para", key)), Sense::click());
        p.rect_filled(
            r,
            3.0,
            if on {
                t.pressed
            } else if resp.hovered() {
                t.hover
            } else {
                Color32::TRANSPARENT
            },
        );
        paint_align_glyph(p, r, j, if on { t.text } else { t.text_dim });
        app.auto.add(&format!("properties.paragraph.{key}"), r, key);
        if resp.clicked() {
            actions.push(("layer.setText".into(), json!({"layer": layer.id.0, "justify": key})));
        }
    }
    y += 30.0;
    y = more_button(app, ui, p, x0, y, "props-para-more", "properties.paragraph.more", PanelKind::Paragraph);
    y + 10.0
}

/// Four lines showing an alignment: last line short for the "justify last …" modes.
fn paint_align_glyph(p: &egui::Painter, r: Rect, j: Justify, c: Color32) {
    let full = r.width() - 8.0;
    for k in 0..4 {
        let ly = r.min.y + 6.0 + k as f32 * 3.5;
        let last = k == 3;
        let lw = match j {
            Justify::Left | Justify::Center | Justify::Right => {
                if k % 2 == 1 {
                    full * 0.6
                } else {
                    full
                }
            }
            Justify::JustifyAll => full,
            _ if last => full * 0.55,
            _ => full,
        };
        let lx = match j {
            Justify::Center | Justify::JustifyLastCenter => r.center().x - lw / 2.0,
            Justify::Right | Justify::JustifyLastRight => r.max.x - 4.0 - lw,
            _ => r.min.x + 4.0,
        };
        p.line_segment([pos2(lx, ly), pos2(lx + lw, ly)], Stroke::new(1.2, c));
    }
}

/// "··· More" button that opens the full panel; returns the y below it.
#[allow(clippy::too_many_arguments)]
fn more_button(app: &mut EffectcraftApp, ui: &mut egui::Ui, p: &egui::Painter, x0: f32, y: f32, id: &str, auto: &str, panel: PanelKind) -> f32 {
    let t = app.tokens;
    let r = Rect::from_min_size(pos2(x0, y), vec2(64.0, 22.0));
    let resp = ui.interact(r, egui::Id::unique(id), Sense::click());
    p.rect_filled(r, 4.0, if resp.hovered() { t.hover } else { t.field_bg });
    p.text(pos2(r.min.x + 8.0, r.center().y), Align2::LEFT_CENTER, "···", Tokens::semibold(12.0), t.text);
    p.text(pos2(r.min.x + 26.0, r.center().y), Align2::LEFT_CENTER, crate::i18n::tr("More"), Tokens::ui(12.0), t.text);
    app.auto.add(auto, r, "More");
    if resp.clicked() {
        app.show_panel(panel);
    }
    y + 28.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decimals_hide_whole_numbers() {
        assert_eq!(decimals(960.0), 0);
        assert_eq!(decimals(960.5), 1);
    }
}
