use super::common::{Actions, DIAL_ROW, PARAM_NAME_X, ROW, TREE_INDENT, gesture_key};
use super::{
    Align2, Color32, EffectcraftApp, EvalCtx, FxPick, Icon, Layer, Node, ParamUi, PropGroup, Property, Rect, Sense, Stroke, Tokens, Value, fw, icons, json,
    pos2, vec2, widgets,
};

fn triangle(p: &egui::Painter, c: egui::Pos2, left: bool, col: Color32) {
    let (dx, h) = (if left { -3.5 } else { 3.5 }, 4.0);
    p.add(egui::Shape::convex_polygon(vec![pos2(c.x + dx, c.y), pos2(c.x - dx, c.y - h), pos2(c.x - dx, c.y + h)], col, Stroke::NONE));
}

/// The layer and whether the time is on a key, for the key navigator.
#[derive(Clone, Copy)]
struct KeyNav {
    layer_id: u64,
    on_key: bool,
}

/// ◀ ◆ ▶ at the right end of an animated parameter's row.
fn key_navigator(app: &mut EffectcraftApp, ui: &mut egui::Ui, p: &egui::Painter, nav: KeyNav, prop: &Property, r: Rect, actions: &mut Actions) {
    let KeyNav { layer_id, on_key } = nav;
    let t = app.tokens;
    let cy = r.center().y;
    let uid = prop.uid;
    for (what, dx) in [("prev", -44.0), ("key", -32.0), ("next", -20.0)] {
        let cr = Rect::from_center_size(pos2(r.max.x + dx, cy), vec2(12.0, 16.0));
        let resp = ui.interact(cr, egui::Id::unique(("ec-nav", uid, what)), Sense::click());
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
        app.auto.add(&format!("effectControls.prop.{uid}.{what}Key"), cr, &prop.name);
        if resp.clicked() {
            match what {
                "prev" => actions.push(("time.go".into(), json!({"to": "prevKey", "prop": uid}))),
                "next" => actions.push(("time.go".into(), json!({"to": "nextKey", "prop": uid}))),
                _ => actions.push(("prop.toggleKey".into(), json!({"layer": layer_id, "prop": uid, "selected": true}))),
            }
        }
    }
}

/// Toggle a viewer pick (crosshair / eyedropper) for a parameter (also the Timeline's colour
/// eyedroppers).
pub(crate) fn toggle_pick(app: &mut EffectcraftApp, kind: &str, layer: &Layer, prop: &Property) {
    let pick = FxPick { kind: kind.into(), layer: layer.id.0, prop: prop.uid, name: prop.name.clone() };
    if app.ui.fx_pick.as_ref() == Some(&pick) {
        app.ui.fx_pick = None;
    } else {
        app.ui.fx_pick = Some(pick);
        app.ui.text_color_pick = None;
        app.ui.status = if kind == "point" {
            format!("Click in the Composition panel to set {}", prop.name)
        } else {
            format!("Click in the Composition panel to sample {}", prop.name)
        };
    }
}

/// What the value controls of one property row share.
#[derive(Clone, Copy)]
struct PropCx<'a> {
    p: &'a egui::Painter,
    layer: &'a Layer,
    group: &'a PropGroup,
    prop: &'a Property,
    r: Rect,
    vx: f32,
    cy: f32,
    swr: Rect,
    t: Tokens,
    uid: u64,
    scrub: bool,
}

impl PropCx<'_> {
    /// The tokens of the value fields: red while an expression drives the parameter (#1306).
    fn value_tokens(&self) -> Tokens {
        self.t.for_value(self.prop.has_expression())
    }

    fn set(&self, actions: &mut Actions, v: serde_json::Value) {
        let (layer, uid, merge, scrub) = (self.layer, self.uid, format!("ec-{}", self.uid), self.scrub);
        actions.push(("prop.set".into(), json!({"layer": layer.id.0, "prop": uid, "value": v, "merge": merge, "selected": true, "offset": scrub})))
    }

    fn twirl_open(&self, app: &mut EffectcraftApp, ui: &mut egui::Ui) {
        let PropCx { swr, cy, uid, prop, t, .. } = self;
        let (cy, uid) = (*cy, *uid);
        let open = app.ui.fx_slider_open.contains(&uid);
        let tw = Rect::from_center_size(pos2(swr.min.x - 9.0, cy), vec2(10.0, 10.0));
        if widgets::twirl(ui, tw, open, egui::Id::unique(("ec-stw", uid)), t).clicked() {
            if open {
                app.ui.fx_slider_open.remove(&uid);
            } else {
                app.ui.fx_slider_open.insert(uid);
            }
        }
        app.auto.add(&format!("effectControls.prop.{uid}.twirl"), tw, &prop.name);
    }
}

/// One property row: stopwatch, name, value control (and the keyframe navigator). Returns the
/// value fields of a vector, one per dimension, for the pick whip.
#[allow(clippy::too_many_arguments)]
fn prop_row(
    app: &mut EffectcraftApp,
    ui: &mut egui::Ui,
    p: &egui::Painter,
    layer: &Layer,
    group: &PropGroup,
    prop: &Property,
    ectx: &EvalCtx,
    r: Rect,
    indent: f32,
    actions: &mut Actions,
) -> Vec<Rect> {
    let t = app.tokens;
    let mut dims = vec![];
    let cy = r.center().y;
    let uid = prop.uid;
    if ui.input(|i| i.pointer.primary_pressed()) && ui.rect_contains_pointer(r) {
        let add = ui.input(|i| i.modifiers.shift || i.modifiers.command);
        actions.push(("prop.select".into(), json!({"layer": layer.id.0, "prop": uid, "add": add})));
    }
    let swr = Rect::from_center_size(pos2(r.min.x + indent, cy), vec2(14.0, 14.0));
    if !prop.static_only && !matches!(prop.ui, ParamUi::Hidden) {
        let resp = ui.interact(swr, egui::Id::unique(("ec-sw", uid)), Sense::click());
        icons::paint(
            p,
            swr,
            Icon::Stopwatch,
            if prop.is_animated() {
                t.hot_text
            } else if resp.hovered() {
                t.text
            } else {
                t.text_dim
            },
        );
        app.auto.add(&format!("effectControls.prop.{uid}.stopwatch"), swr, &prop.name);
        if resp.clicked() {
            actions.push(("prop.toggleAnimation".into(), json!({"layer": layer.id.0, "prop": uid, "selected": true})));
        }
    }
    if prop.is_animated() {
        let half = ectx.comp.frame_duration().0 / 2;
        let on_key = prop.keys.iter().any(|k| (layer.comp_time(k.time).0 - ectx.time.0).abs() <= half);
        key_navigator(app, ui, p, KeyNav { layer_id: layer.id.0, on_key }, prop, r, actions);
    }
    let value = ectx.value(layer, prop);
    // Keep a shared value column while names and their icons follow the group hierarchy.
    // Extremely deep groups still leave a little name space before their value.
    let vx = (r.min.x + r.width() * 0.48).max(r.min.x + PARAM_NAME_X + 124.0).max(swr.max.x + 40.0);
    // A vector's values end before the keyframe navigator: in a narrow panel they move left over
    // the name (cut short) instead of running under the navigator (#271).
    let vx = match &value {
        Value::Vec2(_) | Value::Vec3(_) => {
            let point = if matches!(prop.ui, ParamUi::Point | ParamUi::Point3) { 22.0 } else { 0.0 };
            let fields: f32 = value
                .components()
                .iter()
                .take(shown_dims(layer, prop, &value))
                .map(|v| p.layout_no_wrap(format!("{v:.1}"), Tokens::ui(12.0), t.hot_text).size().x + 12.0)
                .sum();
            let right = r.max.x - if prop.is_animated() { 52.0 } else { 8.0 };
            vx.min(right - point - fields + 8.0).max(swr.max.x + 60.0)
        }
        _ => vx,
    };
    let name_clip = Rect::from_min_max(pos2(swr.max.x + 6.0, r.min.y), pos2(vx - 6.0, r.max.y));
    app.auto.add(&format!("effectControls.prop.{uid}.name"), name_clip, &prop.name);
    p.with_clip_rect(name_clip.intersect(p.clip_rect())).text(pos2(swr.max.x + 6.0, cy), Align2::LEFT_CENTER, &prop.name, Tokens::ui(12.0), t.text);
    let name_response = ui.interact(name_clip, egui::Id::unique(("ec-prop-name", uid)), Sense::click());
    if name_response.double_clicked() {
        app.ui.timeline.search.clear();
        app.ui.timeline.scroll_to = Some(crate::state::ScrollTo::Property(uid));
        if let Err(e) = crate::menus::frontend(app, ui.ctx(), "timeline.revealProps", json!({"props": [{"layer": layer.id.0, "prop": uid}]})) {
            app.ui.status = e;
        }
        app.show_panel(crate::dock::PanelKind::Timeline);
    }
    app.auto.add(&format!("effectControls.prop.{uid}.name"), name_clip, &prop.name);
    // With several layers selected the edit applies to the same effect property of each (#491).
    let scrub = ui.ctx().dragged_id().is_some() && ui.input(|i| i.pointer.is_decidedly_dragging());
    let cx = PropCx { p, layer, group, prop, r, vx, cy, swr, t, uid, scrub };
    match &value {
        Value::Scalar(_) if matches!(prop.ui, ParamUi::Mask) => value_mask(app, ui, &cx, &value, actions),
        Value::Scalar(_) if matches!(prop.ui, ParamUi::Angle) => value_angle(app, ui, &cx, &value, actions),
        Value::Scalar(_) => value_scalar(app, ui, &cx, &value, actions),
        Value::Vec2(_) | Value::Vec3(_) => dims = value_vector(app, ui, &cx, &value, actions),
        Value::Color(_) => value_color(app, ui, &cx, &value, actions),
        Value::Bool(_) => value_toggle(app, ui, &cx, &value, actions),
        Value::Enum(_) => value_popup(app, ui, &cx, &value, actions),
        Value::Layer(_) => value_layer_ref(app, ui, &cx, &value, actions),
        // EXtractoR's red / green / blue / alpha: a popup of the footage's OpenEXR channels (#295).
        Value::Str(_) if crate::panels::fx_editors::is_extractor_channel(group, prop) => value_extractor(app, ui, &cx, &value, actions),
        // A file parameter (Apply Color LUT's LUT, an OCIO config): its file name and Choose…,
        // as in After Effects.
        Value::Str(_) if matches!(prop.ui, ParamUi::File { .. }) => value_file(app, ui, &cx, &value, actions),
        // A text parameter (Cryptomatte's Selection, Colorama's Output Cycle, config names):
        // typed in a field, set when it loses the focus (Esc keeps the old text).
        Value::Str(_) if matches!(prop.ui, ParamUi::Text) => value_text(app, ui, &cx, &value, actions),
        Value::Gradient(_) => value_gradient(app, &cx, &value),
        _ => {}
    }
    dims
}

fn value_mask(app: &mut EffectcraftApp, ui: &mut egui::Ui, cx: &PropCx, value: &Value, actions: &mut Actions) {
    let Value::Scalar(v) = value else {
        return;
    };
    let PropCx { t, uid, cy, vx, r, layer, prop, .. } = *cx;
    // A mask of this layer ("Path" popup).
    let mut opts = vec!["None".to_string()];
    if let Some(m) = layer.masks() {
        opts.extend(m.groups().map(|g| g.name.clone()));
    }
    let i = v.round().max(0.0) as usize;
    let label = opts.get(i).cloned().unwrap_or_else(|| format!("Mask {i}"));
    let dr = Rect::from_min_size(pos2(vx, cy - 9.0), vec2((r.max.x - vx - 56.0).clamp(80.0, 200.0), 18.0));
    let pop = egui::Id::unique(("ec-mpop", uid));
    if widgets::dropdown(ui, dr, &label, &t, egui::Id::unique(("ec-m", uid))).clicked() {
        widgets::open_popup(ui, pop);
    }
    app.auto.add(&format!("effectControls.prop.{uid}.value"), dr, &prop.name);
    if let Some(ni) = widgets::popup_menu(ui, pop, dr.left_bottom(), &opts, Some(i)) {
        cx.set(actions, json!(ni as f64));
    }
}

fn value_angle(app: &mut EffectcraftApp, ui: &mut egui::Ui, cx: &PropCx, value: &Value, actions: &mut Actions) {
    let Value::Scalar(v) = value else {
        return;
    };
    let PropCx { uid, cy, vx, prop, .. } = *cx;
    let (rr, dr, nv) = fw::angle_field(ui, pos2(vx, cy - 9.0), egui::Id::unique(("ec-v", uid)), *v, 1, &cx.value_tokens());
    app.auto.add(&format!("effectControls.prop.{uid}.value"), rr.union(dr), &fw::format_angle(*v, 1));
    app.auto.add(&format!("effectControls.prop.{uid}.revolutions"), rr, &prop.name);
    if let Some(nv) = nv {
        cx.set(actions, json!(nv));
    }
    cx.twirl_open(app, ui);
}

fn value_scalar(app: &mut EffectcraftApp, ui: &mut egui::Ui, cx: &PropCx, value: &Value, actions: &mut Actions) {
    let Value::Scalar(v) = value else {
        return;
    };
    let PropCx { uid, cy, vx, prop, .. } = *cx;
    let (min, max, dec) = match &prop.ui {
        ParamUi::Slider { min, max, decimals, .. } => (*min, *max, *decimals as usize),
        _ => (-1e9, 1e9, 1),
    };
    let suffix = match prop.ui {
        ParamUi::Percent => "%",
        _ => "",
    };
    let speed = widgets::slider_scrub_speed(&prop.ui);
    let (vr, nv, _) = widgets::hot_number_at(
        ui,
        pos2(vx, cy - 9.0),
        egui::Id::unique(("ec-v", uid)),
        *v,
        &widgets::HotSpec { speed, range: (min, max), decimals: dec.max(if dec == 0 { 0 } else { 1 }), suffix },
        &cx.value_tokens(),
    );
    app.auto.add(&format!("effectControls.prop.{uid}.value"), vr, &prop.name);
    if let Some(nv) = nv {
        cx.set(actions, json!(nv));
    }
    // AE hides a slider param's slider until its twirl is opened (see `slider_row`).
    if matches!(prop.ui, ParamUi::Slider { .. }) {
        cx.twirl_open(app, ui);
    }
}

fn value_vector(app: &mut EffectcraftApp, ui: &mut egui::Ui, cx: &PropCx, value: &Value, actions: &mut Actions) -> Vec<Rect> {
    let mut dims = vec![];
    let PropCx { t, uid, cy, vx, layer, prop, .. } = *cx;
    let c = value.components();
    let n = shown_dims(layer, prop, value);
    let vt = cx.value_tokens();
    let mut x = vx;
    if matches!(prop.ui, ParamUi::Point | ParamUi::Point3) {
        let cr = Rect::from_center_size(pos2(x + 8.0, cy), vec2(18.0, 18.0));
        let active = app.ui.fx_pick.as_ref().is_some_and(|k| k.prop == uid && k.kind == "point");
        if fw::crosshair_button(ui, cr, active, egui::Id::unique(("ec-xh", uid)), &t)
            .on_hover_text(crate::i18n::tr("Click, then click in the Composition panel"))
            .clicked()
        {
            toggle_pick(app, "point", layer, prop);
        }
        app.auto.add(&format!("effectControls.prop.{uid}.crosshair"), cr, &prop.name);
        x += 22.0;
    }
    for d in 0..n.min(c.len()) {
        let (vr, nv, _) = widgets::hot_number_at(
            ui,
            pos2(x, cy - 9.0),
            egui::Id::unique(("ec-v", uid, d)),
            c[d],
            &widgets::HotSpec { speed: 1.0, range: (-1e9, 1e9), decimals: 1, suffix: "" },
            &vt,
        );
        app.auto.add(&format!("effectControls.prop.{uid}.value.{d}"), vr, &prop.name);
        dims.push(vr);
        if let Some(nv) = nv {
            let mut nc = c.clone();
            nc[d] = nv;
            cx.set(actions, json!(nc));
            // Typed into this component only: other selected layers keep the rest of
            // their own value (#1434).
            if let Some((_, a)) = actions.last_mut() {
                a["dims"] = json!([d]);
            }
        }
        x = vr.max.x + 8.0;
    }
    dims
}

fn value_color(app: &mut EffectcraftApp, ui: &mut egui::Ui, cx: &PropCx, value: &Value, actions: &mut Actions) {
    let Value::Color(c) = value else {
        return;
    };
    let PropCx { t, uid, cy, vx, layer, prop, .. } = *cx;
    let sr = Rect::from_min_size(pos2(vx, cy - 8.0), vec2(30.0, 16.0));
    let pop = egui::Id::unique(("ec-cpop", uid));
    if widgets::swatch(ui, sr, [c[0] as f32, c[1] as f32, c[2] as f32, 1.0], egui::Id::unique(("ec-c", uid)), &t).clicked() {
        widgets::open_popup(ui, pop);
    }
    app.auto.add(&format!("effectControls.prop.{uid}.value"), sr, &prop.name);
    let er = Rect::from_center_size(pos2(sr.max.x + 14.0, cy), vec2(18.0, 18.0));
    let active = app.ui.fx_pick.as_ref().is_some_and(|k| k.prop == uid && k.kind == "color");
    if fw::eyedropper_button(ui, er, active, egui::Id::unique(("ec-eye", uid)), &t)
        .on_hover_text(crate::i18n::tr("Eyedropper: click, then click in the Composition panel"))
        .clicked()
    {
        toggle_pick(app, "color", layer, prop);
    }
    app.auto.add(&format!("effectControls.prop.{uid}.eyedropper"), er, &prop.name);
    let mut rgb = [c[0] as f32, c[1] as f32, c[2] as f32];
    if crate::widgets::color_popup(ui, pop, sr.left_bottom(), &mut rgb) {
        cx.set(actions, json!([rgb[0], rgb[1], rgb[2], 1.0]));
    }
}

fn value_toggle(app: &mut EffectcraftApp, ui: &mut egui::Ui, cx: &PropCx, value: &Value, actions: &mut Actions) {
    let Value::Bool(b) = value else {
        return;
    };
    let PropCx { t, uid, cy, vx, p, prop, .. } = *cx;
    let cr = Rect::from_min_size(pos2(vx, cy - 8.0), vec2(16.0, 16.0));
    if widgets::checkbox(ui, cr, *b, &t, egui::Id::unique(("ec-b", uid))).clicked() {
        cx.set(actions, json!(!b));
    }
    p.text(pos2(cr.max.x + 6.0, cy), Align2::LEFT_CENTER, &prop.name, Tokens::ui(12.0), t.text_dim);
    app.auto.add(&format!("effectControls.prop.{uid}.value"), cr, &prop.name);
}

fn value_popup(app: &mut EffectcraftApp, ui: &mut egui::Ui, cx: &PropCx, value: &Value, actions: &mut Actions) {
    let Value::Enum(i) = value else {
        return;
    };
    let PropCx { t, uid, cy, vx, r, p, prop, .. } = *cx;
    if let ParamUi::Popup { options } = &prop.ui {
        let widest = options.iter().map(|o| p.layout_no_wrap(o.clone(), Tokens::ui(11.5), t.text).size().x).fold(0.0, f32::max);
        let dr = Rect::from_min_size(pos2(vx, cy - 9.0), vec2((widest + 30.0).clamp(80.0, (r.max.x - vx - 56.0).max(80.0)), 18.0));
        let pop = egui::Id::unique(("ec-epop", uid));
        if widgets::dropdown(ui, dr, options.get(*i as usize).map(String::as_str).unwrap_or(""), &t, egui::Id::unique(("ec-e", uid))).clicked() {
            widgets::open_popup(ui, pop);
        }
        app.auto.add(&format!("effectControls.prop.{uid}.value"), dr, &prop.name);
        if let Some(ni) = widgets::popup_menu(ui, pop, dr.left_bottom(), options, Some(*i as usize)) {
            cx.set(actions, json!(ni));
        }
    }
}

fn value_layer_ref(app: &mut EffectcraftApp, ui: &mut egui::Ui, cx: &PropCx, value: &Value, actions: &mut Actions) {
    let Value::Layer(l) = value else {
        return;
    };
    let PropCx { t, uid, cy, vx, r, layer, prop, group, .. } = *cx;
    let comp = app.session.active_comp_arc();
    if let Some(comp) = comp {
        let dr = Rect::from_min_size(pos2(vx, cy - 9.0), vec2(((r.max.x - vx) * 0.55).clamp(90.0, 170.0), 18.0));
        if let Some(chosen) = widgets::layer_picker(ui, dr, &comp.layers, *l, &t, egui::Id::unique(("ec-l", uid))) {
            cx.set(actions, chosen.map(|id| json!(id)).unwrap_or(serde_json::Value::Null));
        }
        app.auto.add(&format!("effectControls.prop.{uid}.value"), dr, &prop.name);
        // Source / Masks / Effects & Masks (the hidden companion parameter).
        if let Some(src) = group.get(&effectcraft_engine::effects::layer_source_id(&prop.match_id)) {
            let si = src.value.as_enum() as usize;
            let opts: Vec<String> = effectcraft_engine::effects::LAYER_SOURCE_OPTIONS.iter().map(|s| s.to_string()).collect();
            let sr = Rect::from_min_size(pos2(dr.max.x + 6.0, cy - 9.0), vec2(((r.max.x - dr.max.x) - 64.0).clamp(70.0, 120.0), 18.0));
            let spop = egui::Id::unique(("ec-lspop", uid));
            if widgets::dropdown(ui, sr, opts.get(si).map(String::as_str).unwrap_or(""), &t, egui::Id::unique(("ec-ls", uid))).clicked() {
                widgets::open_popup(ui, spop);
            }
            app.auto.add(&format!("effectControls.prop.{uid}.source"), sr, &src.name);
            if let Some(ni) = widgets::popup_menu(ui, spop, sr.left_bottom(), &opts, Some(si)) {
                actions.push(("prop.set".into(), json!({"layer": layer.id.0, "prop": src.uid, "value": ni})));
            }
        }
    }
}

fn value_extractor(app: &mut EffectcraftApp, ui: &mut egui::Ui, cx: &PropCx, value: &Value, actions: &mut Actions) {
    let Value::Str(s) = value else {
        return;
    };
    let PropCx { cy, vx, r, layer, prop, .. } = *cx;
    let dr = Rect::from_min_size(pos2(vx, cy - 9.0), vec2((r.max.x - vx - 56.0).clamp(80.0, 200.0), 18.0));
    crate::panels::fx_editors::channel_popup(app, ui, layer, prop, s, dr, actions);
}

fn value_file(app: &mut EffectcraftApp, ui: &mut egui::Ui, cx: &PropCx, value: &Value, actions: &mut Actions) {
    let Value::Str(s) = value else {
        return;
    };
    let PropCx { t, uid, cy, vx, r, p, layer, prop, .. } = *cx;
    let bw = 70.0;
    let br = Rect::from_min_size(pos2(vx, cy - 9.0), vec2(bw, 18.0));
    if widgets::text_button(ui, br, crate::i18n::tr("Choose..."), false, &t, egui::Id::unique(("ec-file", uid))).clicked() {
        let filters: Vec<&str> = match &prop.ui {
            ParamUi::File { filters } => filters.iter().map(String::as_str).collect(),
            _ => vec![],
        };
        match app.hooks.pick_files.as_ref() {
            Some(pick) => {
                if let Some(path) = pick(&filters).into_iter().next() {
                    actions.push(("prop.set".into(), json!({"layer": layer.id.0, "prop": uid, "value": path})));
                }
            }
            None => app.ui.status = "no file dialog available: set the path with prop.set".into(),
        }
    }
    app.auto.add(&format!("effectControls.prop.{uid}.choose"), br, &prop.name);
    let shown = if s.trim().is_empty() {
        crate::i18n::tr("None").to_string()
    } else if s.contains('\n') {
        prop.name.clone()
    } else {
        effectcraft_engine::sequence::file_name(s)
    };
    let nr = Rect::from_min_max(pos2(br.max.x + 8.0, r.min.y), pos2(r.max.x - 8.0, r.max.y));
    let np = p.with_clip_rect(nr.intersect(p.clip_rect()));
    np.text(pos2(nr.min.x, cy), Align2::LEFT_CENTER, &shown, Tokens::ui(12.0), if s.trim().is_empty() { t.text_dim } else { t.text });
    let hover = ui.interact(nr, egui::Id::unique(("ec-file-name", uid)), Sense::hover());
    if !s.trim().is_empty() && !s.contains('\n') {
        hover.on_hover_text(s);
    }
    app.auto.add(&format!("effectControls.prop.{uid}.value"), nr, &shown);
}

fn value_text(app: &mut EffectcraftApp, ui: &mut egui::Ui, cx: &PropCx, value: &Value, actions: &mut Actions) {
    let Value::Str(s) = value else {
        return;
    };
    let PropCx { t, uid, cy, vx, r, layer, prop, .. } = *cx;
    let fr = Rect::from_min_size(pos2(vx, cy - 9.0), vec2((r.max.x - vx - 12.0).clamp(80.0, 320.0), 18.0));
    let bid = egui::Id::unique(("ec-text", uid));
    let mut buf: String = ui.data(|d| d.get_temp(bid)).unwrap_or_else(|| s.to_owned());
    let resp = widgets::text_field(ui, fr, &mut buf, "", &t);
    // The field has no visible hint: give the screen reader the parameter's name.
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::TextInput, ui.is_enabled(), &prop.name));
    app.auto.add(&format!("effectControls.prop.{uid}.value"), fr, &prop.name);
    if resp.lost_focus() {
        if buf != *s && !ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            actions.push(("prop.set".into(), json!({"layer": layer.id.0, "prop": uid, "value": buf})));
        }
        ui.data_mut(|d| d.remove::<String>(bid));
    } else if resp.has_focus() {
        ui.data_mut(|d| d.insert_temp(bid, buf));
    }
}

fn value_gradient(app: &mut EffectcraftApp, cx: &PropCx, value: &Value) {
    let Value::Gradient(g) = value else {
        return;
    };
    let PropCx { t, uid, cy, vx, r, p, prop, .. } = *cx;
    let gr = Rect::from_min_size(pos2(vx, cy - 7.0), vec2((r.max.x - vx - 56.0).clamp(60.0, 180.0), 14.0));
    let n = 48;
    for k in 0..n {
        let f = k as f64 / (n - 1) as f64;
        let c = gradient_at(g, f);
        let x0 = gr.min.x + gr.width() * k as f32 / n as f32;
        let x1 = gr.min.x + gr.width() * (k + 1) as f32 / n as f32;
        p.rect_filled(Rect::from_min_max(pos2(x0, gr.min.y), pos2(x1, gr.max.y)), 0.0, c);
    }
    p.rect_stroke(gr, 2.0, Stroke::new(1.0, t.field_border), egui::StrokeKind::Inside);
    app.auto.add(&format!("effectControls.prop.{uid}.value"), gr, &prop.name);
}

/// How many of a vector parameter's values show (a 3D point's Z only on a 3D layer).
fn shown_dims(layer: &Layer, prop: &Property, value: &Value) -> usize {
    let n = value.components().len();
    if prop.shown_dims > 0 && !(layer.is_3d() && n == 3) { prop.shown_dims as usize } else { n }
}

/// Colour of a gradient at `f` (colour stops only).
fn gradient_at(g: &effectcraft_engine::keyframe::Gradient, f: f64) -> Color32 {
    let stops = &g.colors;
    let c = if stops.is_empty() {
        [0.0, 0.0, 0.0, 1.0]
    } else if f <= stops[0].0 {
        stops[0].1
    } else if f >= stops[stops.len() - 1].0 {
        stops[stops.len() - 1].1
    } else {
        let i = stops.iter().position(|s| s.0 > f).unwrap_or(stops.len() - 1).max(1);
        let (a, b) = (stops[i - 1], stops[i]);
        let k = ((f - a.0) / (b.0 - a.0).max(1e-9)) as f32;
        [0, 1, 2, 3].map(|j| a.1[j] + (b.1[j] - a.1[j]) * k)
    };
    Color32::from_rgb((c[0].clamp(0.0, 1.0) * 255.0) as u8, (c[1].clamp(0.0, 1.0) * 255.0) as u8, (c[2].clamp(0.0, 1.0) * 255.0) as u8)
}

/// The twirled-open slider under a slider param: track, knob and the slider range's ends.
#[allow(clippy::too_many_arguments)]
fn slider_row(
    app: &mut EffectcraftApp,
    ui: &mut egui::Ui,
    p: &egui::Painter,
    layer: &Layer,
    prop: &Property,
    ectx: &EvalCtx,
    r: Rect,
    x0: f32,
    actions: &mut Actions,
) {
    let t = app.tokens;
    let ParamUi::Slider { slider_min, slider_max, decimals, .. } = prop.ui else { return };
    let Value::Scalar(v) = ectx.value(layer, prop) else { return };
    let uid = prop.uid;
    let tr = Rect::from_min_max(pos2(r.min.x + x0, r.min.y + 8.0), pos2(r.max.x - 16.0, r.min.y + 12.0));
    if tr.width() < 40.0 {
        return;
    }
    p.rect_filled(tr, 2.0, t.field_bg);
    let f = ((v - slider_min) / (slider_max - slider_min)).clamp(0.0, 1.0) as f32;
    p.rect_filled(Rect::from_min_max(tr.min, pos2(tr.min.x + tr.width() * f, tr.max.y)), 2.0, t.accent.gamma_multiply(0.7));
    p.circle_filled(pos2(tr.min.x + tr.width() * f, tr.center().y), 5.0, t.text);
    let dec = decimals as usize;
    p.text(pos2(tr.min.x, tr.max.y + 4.0), Align2::LEFT_TOP, format!("{slider_min:.dec$}"), Tokens::ui(10.5), t.text_dim);
    p.text(pos2(tr.max.x, tr.max.y + 4.0), Align2::RIGHT_TOP, format!("{slider_max:.dec$}"), Tokens::ui(10.5), t.text_dim);
    let sresp = ui.interact(tr.expand2(vec2(4.0, 6.0)), egui::Id::unique(("ec-slider", uid)), Sense::click_and_drag());
    app.auto.add(&format!("effectControls.prop.{uid}.slider"), tr, &prop.name);
    if (sresp.dragged() || sresp.clicked())
        && let Some(pt) = sresp.interact_pointer_pos()
    {
        let f = ((pt.x - tr.min.x) / tr.width()).clamp(0.0, 1.0) as f64;
        actions.push((
            "prop.set".into(),
            json!({"layer": layer.id.0, "prop": uid, "value": slider_min + (slider_max - slider_min) * f, "merge": format!("ec-{uid}")}),
        ));
    }
}

/// The twirled-open angle dial under an angle param.
#[allow(clippy::too_many_arguments)]
fn dial_row(app: &mut EffectcraftApp, ui: &mut egui::Ui, layer: &Layer, prop: &Property, ectx: &EvalCtx, r: Rect, x0: f32, actions: &mut Actions) {
    let t = app.tokens;
    let Value::Scalar(v) = ectx.value(layer, prop) else { return };
    let uid = prop.uid;
    let d = (r.height() - 12.0).min(52.0);
    let dr = Rect::from_min_size(pos2(r.min.x + x0 + 40.0, r.min.y + 6.0), vec2(d, d));
    let id = egui::Id::unique(("ec-dial", uid));
    let (resp, nv) = fw::angle_dial(ui, dr, v, id, &t);
    app.auto.add(&format!("effectControls.prop.{uid}.dial"), dr, &prop.name);
    if let Some(nv) = nv {
        let key = gesture_key(ui, id, resp.drag_started() || resp.clicked());
        actions.push(("prop.set".into(), json!({"layer": layer.id.0, "prop": uid, "value": nv, "merge": key})));
    }
}

/// The effect instance a row belongs to, for conditional visibility
/// ([`effectcraft_engine::effects::catalog::param_shown`]): effect id, the instance's root
/// group and the spec-id path of the group being drawn (`""` or `cameraPosition/`).
pub(super) struct FxScope<'a> {
    pub(super) effect: &'a str,
    pub(super) root: &'a PropGroup,
    pub(super) path: String,
}

impl<'a> FxScope<'a> {
    pub(super) fn sub(&self, m: &str) -> FxScope<'a> {
        FxScope { effect: self.effect, root: self.root, path: format!("{}{m}/", self.path) }
    }
    /// Whether child `m` of the current group is shown.
    pub(super) fn shown(&self, layer: &Layer, ectx: &EvalCtx, m: &str) -> bool {
        let root = self.root;
        let value = |id: &str| -> Option<Value> {
            let mut g = root;
            let mut parts = id.split('/').peekable();
            while let Some(part) = parts.next() {
                if parts.peek().is_none() {
                    return g.get(part).map(|p| ectx.value(layer, p));
                }
                g = g.sub(part)?;
            }
            None
        };
        effectcraft_engine::effects::catalog::param_shown(self.effect, &format!("{}{m}", self.path), &value)
    }
}

/// Twirl `g` closed (it was `open`) or open; with `all` (Alt/Cmd-click) every group under it follows.
pub(super) fn toggle_closed(app: &mut EffectcraftApp, g: &PropGroup, open: bool, all: bool) {
    fn walk(g: &PropGroup, out: &mut Vec<u64>) {
        for sg in g.groups() {
            out.push(sg.uid);
            walk(sg, out);
        }
    }
    let mut uids = vec![g.uid];
    if all {
        walk(g, &mut uids);
    }
    for u in uids {
        if open {
            app.ui.fx_closed.insert(u);
        } else {
            app.ui.fx_closed.remove(&u);
        }
    }
}

/// Lays out one property group's rows below `*y` (which it advances), recursing into open
/// subgroups: one row per visible property with its controls, plus the dial or slider row when its
/// slider is open, and an inline editor for groups that have one. Rows scrolled out of `rect` are
/// skipped without being drawn or registered, though their height is still counted. Pick-whip
/// drops are registered for each visible property; edits go on `actions`.
#[allow(clippy::too_many_arguments)]
pub(super) fn group_rows(
    app: &mut EffectcraftApp,
    ui: &mut egui::Ui,
    p: &egui::Painter,
    layer: &Layer,
    g: &PropGroup,
    ectx: &EvalCtx,
    rect: Rect,
    y: &mut f32,
    depth: usize,
    fx: &FxScope,
    actions: &mut Actions,
) {
    let t = app.tokens;
    for c in &g.children {
        let r = Rect::from_min_size(pos2(rect.min.x, *y), vec2(rect.width(), ROW));
        match c {
            Node::Prop(pr) => {
                if matches!(pr.ui, ParamUi::Hidden) || pr.three_d_only && !layer.is_3d() || !fx.shown(layer, ectx, &pr.match_id) {
                    continue;
                }
                *y += ROW;
                let indent = PARAM_NAME_X - 13.0 + TREE_INDENT * depth as f32;
                let visible = !(r.max.y < rect.min.y || r.min.y > rect.max.y);
                if visible {
                    app.auto.add(&format!("effectControls.row.{}", pr.uid), r, &pr.name);
                    let dims = prop_row(app, ui, p, layer, g, pr, ectx, r, indent, actions);
                    // A Timeline pick whip can be dropped here (After Effects).
                    crate::panels::timeline::whip_target(ui.ctx(), r.intersect(rect), layer.id.0, pr.uid, dims);
                }
                if app.ui.fx_slider_open.contains(&pr.uid) {
                    if matches!(pr.ui, ParamUi::Angle) {
                        let sr = Rect::from_min_size(pos2(rect.min.x, *y), vec2(rect.width(), DIAL_ROW));
                        *y += DIAL_ROW;
                        if sr.max.y >= rect.min.y && sr.min.y <= rect.max.y {
                            dial_row(app, ui, layer, pr, ectx, sr, indent + 22.0, actions);
                        }
                    } else if matches!(pr.ui, ParamUi::Slider { .. }) {
                        let sr = Rect::from_min_size(pos2(rect.min.x, *y), vec2(rect.width(), 34.0));
                        *y += 34.0;
                        if sr.max.y >= rect.min.y && sr.min.y <= rect.max.y {
                            slider_row(app, ui, p, layer, pr, ectx, sr, indent + 22.0, actions);
                        }
                    }
                }
            }
            Node::Group(sg) => {
                // Paint strokes live in the Timeline only (AE's Paint shows Paint on Transparent).
                if effectcraft_engine::effects::paint::is_paint(g) || !fx.shown(layer, ectx, &sg.match_id) {
                    continue;
                }
                *y += ROW;
                let open = !app.ui.fx_closed.contains(&sg.uid);
                let tw = Rect::from_center_size(pos2(r.min.x + PARAM_NAME_X - 12.0 + TREE_INDENT * depth as f32, r.center().y), vec2(12.0, 12.0));
                if r.max.y >= rect.min.y && r.min.y <= rect.max.y {
                    app.auto.add(&format!("effectControls.row.{}", sg.uid), r, &sg.name);
                    app.auto.add(&format!("effectControls.group.{}.twirl", sg.uid), tw, &sg.name);
                    if widgets::twirl(ui, tw, open, egui::Id::unique(("ec-g", sg.uid)), &t).clicked() {
                        toggle_closed(app, sg, open, ui.input(|i| i.modifiers.alt || i.modifiers.command));
                    }
                    let name = p.text(pos2(tw.max.x + 6.0, r.center().y), Align2::LEFT_CENTER, &sg.name, Tokens::ui(12.0), t.text);
                    app.auto.add(&format!("effectControls.group.{}.name", sg.uid), name, &sg.name);
                }
                if open {
                    let sub = fx.sub(&sg.match_id);
                    // A visual editor for the group's hidden parameters (Lumetri curves, …).
                    let eh = crate::panels::fx_editors::inline_height(fx.effect, &sub.path, rect.width());
                    if eh > 0.0 {
                        let er = Rect::from_min_size(pos2(rect.min.x, *y), vec2(rect.width(), eh));
                        *y += eh;
                        if er.max.y >= rect.min.y && er.min.y <= rect.max.y {
                            crate::panels::fx_editors::inline_editor(app, ui, p, layer, fx.root, sg, fx.effect, &sub.path, ectx, er, actions);
                        }
                    }
                    group_rows(app, ui, p, layer, sg, ectx, rect, y, depth + 1, &sub, actions);
                }
            }
        }
    }
}
