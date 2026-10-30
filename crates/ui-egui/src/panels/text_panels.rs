//! Character, Paragraph and Align panels. While a text layer is edited in the viewer the
//! Character and Paragraph panels show and change the selected text (creating style runs);
//! otherwise they apply to the whole selected text layer.

use crate::i18n::tr;
use effectcraft_engine::keyframe::{BaselineOption, Composer, Direction, FigureStyle, FigureWidth, Justify, Kerning, LineJoin, TextDoc};
use effectcraft_engine::render::EvalCtx;
use egui::{Align2, Rect, Sense, pos2, vec2};
use serde_json::json;

use crate::state::TextColorPick;
use crate::theme::Tokens;
use crate::{EffectcraftApp, widgets};

/// What the text panels act on: a text layer, a document whose base style and first paragraph
/// show the selection's formatting, and the selected character range while editing.
#[derive(Clone, Debug)]
pub struct TextTarget {
    pub layer: u64,
    pub doc: TextDoc,
    pub range: Option<[usize; 2]>,
}

impl TextTarget {
    /// `layer.setText` params for this target.
    pub fn params(&self, mut v: serde_json::Value) -> serde_json::Value {
        v["layer"] = json!(self.layer);
        if let Some(r) = self.range {
            v["range"] = json!(r);
        }
        v
    }
}

/// The edited text layer's selection, else the selected text layer's Source Text at the CTI.
pub fn text_target(app: &EffectcraftApp) -> Option<TextTarget> {
    let comp = app.session.active_comp()?;
    let cid = app.session.active_comp_id()?;
    if let Some(e) = app.session.state.text_edit.clone()
        && let Some(full) = effectcraft_engine::commands::text_edit::layer_doc(&app.session, e.layer)
    {
        let r = e.range();
        let style = e.pending.clone().unwrap_or_else(|| if r.is_empty() { full.insertion_style(r.start) } else { full.style_at(r.start) });
        let para = full.para(full.para_of(r.start));
        let mut view = full.clone();
        view.runs.clear();
        view.paragraphs.clear();
        view.apply_style_all(|s| *s = style.clone());
        let n = view.para_count();
        view.set_paras(vec![para; n]);
        return Some(TextTarget { layer: e.layer.0, doc: view, range: Some([r.start, r.end]) });
    }
    let layer = app
        .session
        .state
        .selected_layers
        .iter()
        .filter_map(|id| comp.layer(*id))
        .find(|l| matches!(l.source, effectcraft_engine::project::LayerSource::Text))?;
    let ectx = EvalCtx {
        project: &app.session.project,
        comp_id: cid,
        comp,
        time: app.session.time(),
        expr: app.session.expr.as_deref(),
        footage: None,
        property_overrides: &[],
        media_overrides: &[],
    };
    effectcraft_engine::render::text::source_text(&ectx, layer).map(|d| TextTarget { layer: layer.id.0, doc: d, range: None })
}

/// A small toggle button at `r` showing `label` (semibold 13 when `bold`, else 12), filled with
/// the accent while `on`; returns whether it was clicked. It only reports the click: the caller
/// decides the new state.
fn toggle(app: &mut EffectcraftApp, ui: &mut egui::Ui, p: &egui::Painter, r: Rect, id: &str, look: (&str, bool, bool)) -> bool {
    let (label, on, bold) = look;
    let t = app.tokens;
    let resp = ui.interact(r, egui::Id::unique(("text-toggle", id)), Sense::click());
    p.rect_filled(
        r,
        3.0,
        if on {
            t.accent
        } else if resp.hovered() {
            t.hover
        } else {
            t.field_bg
        },
    );
    let font = if bold { Tokens::semibold(13.0) } else { Tokens::ui(12.0) };
    p.text(r.center(), Align2::CENTER_CENTER, label, font, if on { egui::Color32::WHITE } else { t.text });
    app.auto.add(id, r, label);
    resp.clicked()
}

/// Position of the numeric grid (`x0`, top `y`, width `w`) and whether edits are collected.
#[derive(Clone, Copy)]
struct Grid {
    x0: f32,
    y: f32,
    w: f32,
    enabled: bool,
}

/// Numeric fields in AE's two-column grid (Kerning is a popup, at row 1 left).
fn numeric_fields(app: &mut EffectcraftApp, ui: &mut egui::Ui, p: &egui::Painter, g: Grid, doc: &TextDoc, actions: &mut Vec<serde_json::Value>) {
    let t = app.tokens;
    let Grid { x0, y, w, enabled } = g;
    let fields: [(&str, &str, f64, (f64, f64), &str); 10] = [
        ("size", "T", doc.size, (1.0, 2000.0), " px"),
        ("leading", "A", doc.leading.unwrap_or(doc.size * 1.2), (0.0, 5000.0), " px"),
        ("", "", 0.0, (0.0, 0.0), ""),
        ("tracking", "VA", doc.tracking, (-1000.0, 10000.0), ""),
        ("strokeWidth", "W", doc.stroke_width, (0.0, 500.0), " px"),
        ("", "", 0.0, (0.0, 0.0), ""),
        ("vScale", "↕T", doc.v_scale, (1.0, 1000.0), " %"),
        ("hScale", "↔T", doc.h_scale, (1.0, 1000.0), " %"),
        ("baselineShift", "A↑", doc.baseline_shift, (-1000.0, 1000.0), " px"),
        ("tsume", "Ts", doc.tsume, (0.0, 100.0), " %"),
    ];
    for (i, (key, glyph, v, range, suffix)) in fields.into_iter().enumerate() {
        if key.is_empty() {
            continue;
        }
        let col = i % 2;
        let row = i / 2;
        let fx = x0 + col as f32 * (w / 2.0);
        let fy = y + row as f32 * 28.0;
        p.text(pos2(fx, fy + 9.0), Align2::LEFT_CENTER, glyph, Tokens::semibold(11.0), t.text_dim);
        let (r, nv, _) = widgets::hot_number_at(
            ui,
            pos2(fx + 26.0, fy),
            egui::Id::unique(("char", key)),
            v,
            &widgets::HotSpec { speed: 0.5, range, decimals: 0, suffix },
            &t,
        );
        app.auto.add(&format!("character.{key}"), r, key);
        if let Some(nv) = nv
            && enabled
        {
            actions.push(json!({key: nv, "merge": format!("char-{key}")}));
        }
    }
}

/// Kerning: Metrics / Optical / a manual value (grid row 1, left).
fn kerning_field(app: &mut EffectcraftApp, ui: &mut egui::Ui, p: &egui::Painter, g: Grid, doc: &TextDoc, actions: &mut Vec<serde_json::Value>) {
    let t = app.tokens;
    let Grid { x0, y, w, enabled } = g;
    let fy = y + 28.0;
    p.text(pos2(x0, fy + 9.0), Align2::LEFT_CENTER, "V/A", Tokens::semibold(11.0), t.text_dim);
    let label = match doc.kerning {
        Kerning::Metrics => "Metrics".to_string(),
        Kerning::Optical => "Optical".to_string(),
        Kerning::Manual(v) => format!("{v:.0}"),
    };
    let kr = Rect::from_min_size(pos2(x0 + 26.0, fy), vec2((w / 2.0 - 32.0).max(50.0), 20.0));
    let kpop = egui::Id::unique("char-kerning-pop");
    if widgets::dropdown(ui, kr, &label, &t, egui::Id::unique("char-kerning")).clicked() && enabled {
        widgets::open_popup(ui, kpop);
    }
    app.auto.add("character.kerning", kr, "Kerning");
    let opts: Vec<String> = ["Metrics", "Optical", "0", "-50", "-25", "-10", "10", "25", "50", "100"].iter().map(|s| s.to_string()).collect();
    if let Some(i) = widgets::popup_menu(ui, kpop, kr.left_bottom(), &opts, None) {
        let v = match i {
            0 => json!("metrics"),
            1 => json!("optical"),
            _ => json!(opts[i].parse::<f64>().unwrap_or(0.0)),
        };
        actions.push(json!({"kerning": v}));
    }
}

/// Stroke Over Fill / Fill Over Stroke (grid row 2, right).
fn stroke_order_field(app: &mut EffectcraftApp, ui: &mut egui::Ui, g: Grid, doc: &TextDoc, actions: &mut Vec<serde_json::Value>) {
    let t = app.tokens;
    let Grid { x0, y, w, enabled } = g;
    let fx = x0 + w / 2.0;
    let fy = y + 2.0 * 28.0;
    let dr = Rect::from_min_size(pos2(fx, fy), vec2(w / 2.0, 20.0));
    let labels = vec!["Fill Over Stroke".to_string(), "Stroke Over Fill".to_string()];
    let pop = egui::Id::unique("char-stroke-order-pop");
    if widgets::dropdown(ui, dr, &labels[doc.stroke_over_fill as usize], &t, egui::Id::unique("char-stroke-order")).clicked() && enabled {
        widgets::open_popup(ui, pop);
    }
    app.auto.add("character.strokeOverFill", dr, "Stroke order");
    if let Some(i) = widgets::popup_menu(ui, pop, dr.left_bottom(), &labels, Some(doc.stroke_over_fill as usize)) {
        actions.push(json!({"strokeOverFill": i == 1}));
    }
}

/// Line Join of the stroke: Miter / Round / Bevel (grid row 5, under the stroke width).
fn line_join_field(app: &mut EffectcraftApp, ui: &mut egui::Ui, p: &egui::Painter, g: Grid, doc: &TextDoc, actions: &mut Vec<serde_json::Value>) {
    let Grid { x0, y, w, enabled } = g;
    let bw = (w - 2.0 * 4.0) / 3.0;
    for (i, (key, label, join)) in
        [("miter", "Miter", LineJoin::Miter), ("round", "Round", LineJoin::Round), ("bevel", "Bevel", LineJoin::Bevel)].into_iter().enumerate()
    {
        let r = Rect::from_min_size(pos2(x0 + i as f32 * (bw + 4.0), y + 5.0 * 28.0), vec2(bw, 22.0));
        if toggle(app, ui, p, r, &format!("character.lineJoin.{key}"), (label, doc.line_join == join, false)) && enabled {
            actions.push(json!({"lineJoin": key}));
        }
    }
}

/// The Character panel for the text being edited (or the selected text layer): font family and
/// style, size, leading, tracking, stroke, scale, baseline shift and other numeric fields, kerning,
/// fill/stroke colours with eyedroppers, style toggles, OpenType features and variable-font axes.
/// Without a text target the panel is inert — no edit is collected and it asks for a text layer.
/// Edits are collected and, when a target exists, sent as `layer.setText` commands scoped to the
/// edited range; failures show in `app.ui.status`.
pub fn character(app: &mut EffectcraftApp, ui: &mut egui::Ui, rect: Rect) {
    let t = app.tokens;
    let p = ui.painter().with_clip_rect(rect);
    let ctx = ui.ctx().clone();
    let target = text_target(app);
    let enabled = target.is_some();
    let doc = target.as_ref().map(|t| t.doc.clone()).unwrap_or_default();
    let scroll = widgets::PanelScroll::begin(ui, egui::Id::unique("character-scroll"), rect);
    let top = rect.min.y + 10.0 - scroll.offset;
    let mut y = top;
    let x0 = rect.min.x + 10.0;
    let w = rect.width() - 20.0;
    let mut actions: Vec<serde_json::Value> = vec![];
    // Font family + style.
    let fr = Rect::from_min_size(pos2(x0, y), vec2(w, 22.0));
    let pop = egui::Id::unique("char-font-pop");
    if let Some(f) = font_picker(app, ui, pop, fr, &doc.font, "character.font", enabled) {
        actions.push(json!({"font": f}));
    }
    y += 28.0;
    let sr = Rect::from_min_size(pos2(x0, y), vec2(w - 84.0, 22.0));
    let spop = egui::Id::unique("char-style-pop");
    if widgets::dropdown(ui, sr, &doc.style, &t, egui::Id::unique("char-style")).clicked() && enabled {
        widgets::open_popup(ui, spop);
    }
    app.auto.add("character.style", sr, "Font style");
    // The family's own styles.
    let styles: Vec<String> = if widgets::popup_is_open(ui, spop) { effectcraft_engine::font_styles(&doc.font) } else { vec![] };
    if let Some(st) = widgets::popup_menu(ui, spop, sr.left_bottom(), &styles, styles.iter().position(|s| *s == doc.style)).and_then(|i| styles.get(i)) {
        actions.push(json!({"style": st}));
    }
    y += 34.0;
    let grid = Grid { x0, y, w, enabled };
    numeric_fields(app, ui, &p, grid, &doc, &mut actions);
    kerning_field(app, ui, &p, grid, &doc, &mut actions);
    stroke_order_field(app, ui, grid, &doc, &mut actions);
    line_join_field(app, ui, &p, grid, &doc, &mut actions);
    y += 6.0 * 28.0 + 8.0;
    // Fill / stroke swatches with their enable boxes.
    for (i, (key, label, on, c)) in [("fill", "Fill", doc.apply_fill, doc.fill), ("stroke", "Stroke", doc.apply_stroke, doc.stroke)].into_iter().enumerate() {
        let fx = x0 + i as f32 * (w / 2.0);
        let cr = Rect::from_min_size(pos2(fx, y + 3.0), vec2(14.0, 14.0));
        if widgets::checkbox(ui, cr, on, &t, egui::Id::unique(("char-apply", key))).clicked() && enabled {
            actions.push(json!({if key == "fill" { "applyFill" } else { "applyStroke" }: !on}));
        }
        app.auto.add(&format!("character.{key}.enabled"), cr, label);
        let fs = Rect::from_min_size(pos2(fx + 20.0, y), vec2(26.0, 20.0));
        let pop = egui::Id::unique(("char-color-pop", key));
        if widgets::swatch(ui, fs, c, egui::Id::unique(("char-color", key)), &t).clicked() && enabled {
            widgets::open_popup(ui, pop);
        }
        app.auto.add(&format!("character.{key}"), fs, label);
        let mut rgb = [c[0], c[1], c[2]];
        if crate::widgets::color_popup(ui, pop, fs.left_bottom(), &mut rgb) {
            actions.push(json!({key: [rgb[0], rgb[1], rgb[2]], "merge": format!("char-{key}")}));
        }
        let er = Rect::from_min_size(pos2(fs.max.x + 4.0, y + 1.0), vec2(18.0, 18.0));
        let active = app.ui.text_color_pick.as_ref().is_some_and(|pick| pick.layer == target.as_ref().map_or(0, |tt| tt.layer) && pick.field == key);
        let eye = super::fx_widgets::eyedropper_button(ui, er, active, egui::Id::unique(("char-eye", key)), &t);
        app.auto.add(&format!("character.{key}.eyedropper"), er, &format!("{label} eyedropper"));
        if eye.clicked() {
            if active {
                app.ui.text_color_pick = None;
                app.ui.status.clear();
            } else if let Some(tt) = &target {
                app.ui.fx_pick = None;
                app.ui.text_color_pick = Some(TextColorPick { layer: tt.layer, field: key.into(), range: tt.range });
                app.ui.status = crate::i18n::tr_args("Click in the Composition panel to sample text {}", &[&label]);
            }
        }
        p.text(pos2(er.max.x + 6.0, y + 10.0), Align2::LEFT_CENTER, label, Tokens::ui(11.5), t.text_dim);
    }
    y += 32.0;
    // Style toggles: Faux Bold, Faux Italic, All Caps, Small Caps, Superscript, Subscript.
    let sup = doc.baseline == BaselineOption::Superscript;
    let sub = doc.baseline == BaselineOption::Subscript;
    let toggles = [
        ("fauxBold", "T", doc.faux_bold, true),
        ("fauxItalic", "T", doc.faux_italic, false),
        ("allCaps", "TT", doc.all_caps, false),
        ("smallCaps", "Tт", doc.small_caps, false),
        ("superscript", "T¹", sup, false),
        ("subscript", "T₁", sub, false),
    ];
    let bw = ((w - 5.0 * 4.0) / 6.0).clamp(22.0, 30.0);
    for (i, (key, label, on, bold)) in toggles.into_iter().enumerate() {
        let r = Rect::from_min_size(pos2(x0 + i as f32 * (bw + 4.0), y), vec2(bw, 24.0));
        if toggle(app, ui, &p, r, &format!("character.{key}"), (label, on, bold)) && enabled {
            actions.push(json!({key: !on}));
        }
    }
    y += 32.0;
    // Ligatures.
    let lr = Rect::from_min_size(pos2(x0, y), vec2(14.0, 14.0));
    if widgets::checkbox(ui, lr, doc.ligatures, &t, egui::Id::unique("char-ligatures")).clicked() && enabled {
        actions.push(json!({"ligatures": !doc.ligatures}));
    }
    app.auto.add("character.ligatures", lr, "Ligatures");
    p.text(pos2(x0 + 20.0, y + 7.0), Align2::LEFT_CENTER, tr("Ligatures"), Tokens::ui(11.5), t.text_dim);
    // OpenType: a popup with the font's layout features (like the Character panel's OpenType menu).
    let or = Rect::from_min_size(pos2(sr.max.x + 6.0, sr.min.y), vec2(78.0, 22.0));
    let opop = egui::Id::unique("char-opentype-pop");
    let n_on = doc.opentype.feature_settings().len();
    let olabel = if n_on > 0 { format!("OT ({n_on})") } else { "OpenType".to_string() };
    if widgets::dropdown(ui, or, &olabel, &t, egui::Id::unique("char-opentype")).clicked() && enabled {
        widgets::open_popup(ui, opop);
    }
    app.auto.add("character.opentype", or, "OpenType");
    opentype_popup(app, ui, opop, pos2((or.max.x - 300.0).max(rect.min.x), or.max.y), &doc, &mut actions);
    // Vertical type: Tate-Chu-Yoko and Standard Vertical Roman Alignment (Character panel menu).
    if doc.vertical {
        y += 22.0;
        for (i, (key, label, on)) in
            [("tateChuYoko", "Tate-Chu-Yoko", doc.tate_chu_yoko), ("verticalRomanUpright", "Standard Vertical Roman", doc.vertical_roman_upright)]
                .into_iter()
                .enumerate()
        {
            let cr = Rect::from_min_size(pos2(x0 + i as f32 * 120.0, y), vec2(14.0, 14.0));
            if widgets::checkbox(ui, cr, on, &t, egui::Id::unique(("char-vert", key))).clicked() && enabled {
                actions.push(json!({key: !on}));
            }
            app.auto.add(&format!("character.{key}"), cr, label);
            p.text(pos2(cr.max.x + 6.0, y + 7.0), Align2::LEFT_CENTER, label, Tokens::ui(11.0), t.text_dim);
        }
    }
    // Variable Font Axes: one value per axis of a variable font (`character.axis.<tag>`).
    let resolved = effectcraft_engine::text::resolve(&doc.font, &doc.style);
    let face = resolved.face;
    let axes = effectcraft_engine::text::variable::font_axes(face);
    // A named instance (`Black`) sets the axes the text doesn't set itself.
    let instance = effectcraft_engine::text::fonts::instance_variations(&resolved);
    if !axes.is_empty() {
        y += 26.0;
        p.text(pos2(x0, y + 7.0), Align2::LEFT_CENTER, tr("Variable Font Axes"), Tokens::semibold(11.0), t.text_dim);
        for (i, a) in axes.iter().enumerate() {
            let (col, row) = (i % 2, i / 2);
            let fx = x0 + col as f32 * (w / 2.0);
            let fy = y + 20.0 + row as f32 * 24.0;
            let tag = a.tag.trim_end().to_string();
            let v = doc.variations.iter().chain(&instance).find(|(t2, _)| t2.trim_end() == tag).map(|(_, v)| *v).unwrap_or(a.default);
            p.text(pos2(fx, fy + 9.0), Align2::LEFT_CENTER, &tag, Tokens::semibold(10.5), t.text_dim);
            let (r, nv, _) = widgets::hot_number_at(
                ui,
                pos2(fx + 34.0, fy),
                egui::Id::unique(("char-axis", &tag)),
                v as f64,
                &widgets::HotSpec { speed: 1.0, range: (a.min as f64, a.max as f64), decimals: 0, suffix: "" },
                &t,
            );
            app.auto.add(&format!("character.axis.{tag}"), r, &a.name);
            if let Some(nv) = nv
                && enabled
            {
                actions.push(json!({"variations": {tag.clone(): nv}, "merge": format!("char-axis-{tag}")}));
            }
        }
        y += 20.0 + axes.len().div_ceil(2) as f32 * 24.0;
    }
    // (Room for the status line at the bottom.)
    scroll.end(ui, &mut app.auto, "character.scroll", y + 44.0 - top, &t);
    match &target {
        None => {
            p.text(pos2(rect.center().x, rect.max.y - 20.0), Align2::CENTER_CENTER, tr("Select a text layer"), Tokens::ui(11.0), t.text_faint);
        }
        Some(tt) if tt.range.is_some() => {
            let [a, b] = tt.range.unwrap_or_default();
            let msg = if a == b { "Editing: caret (applies to typed text)".to_string() } else { format!("Editing: {} characters selected", b - a) };
            p.text(pos2(x0, rect.max.y - 14.0), Align2::LEFT_CENTER, msg, Tokens::ui(10.5), t.text_faint);
        }
        _ => {}
    }
    let Some(tt) = target else { return };
    for a in actions {
        if let Err(e) = crate::menus::invoke(app, &ctx, "layer.setText", tt.params(a)) {
            app.ui.status = e;
        }
    }
}

/// The Character panel's OpenType popup: feature toggles (dimmed when the font lacks them),
/// figure styles and the twenty stylistic sets. Every row is an automation target
/// (`character.opentype.<key>`), and every change is a `layer.setText` attribute.
fn opentype_popup(app: &mut EffectcraftApp, ui: &mut egui::Ui, id: egui::Id, pos: egui::Pos2, doc: &TextDoc, actions: &mut Vec<serde_json::Value>) {
    let open_id = id.with("open");
    if !ui.data(|d| d.get_temp::<bool>(open_id).unwrap_or(false)) {
        return;
    }
    let t = app.tokens;
    let feats = effectcraft_engine::font_features(&doc.font, &doc.style);
    let has = |tag: &str| feats.iter().any(|f| f == tag);
    let o = doc.opentype;
    let mut rects: Vec<(String, Rect, String)> = vec![];
    let area = egui::Area::new(id.with("area")).order(egui::Order::Foreground).fixed_pos(pos).show(ui.ctx(), |ui| {
        egui::Frame::popup(ui.style()).show(ui, |ui| {
            ui.set_min_width(230.0);
            ui.label(egui::RichText::new(format!("{} {}", doc.font, doc.style)).small().color(t.text_dim));
            let toggles: [(&str, &str, bool, &[&str]); 12] = [
                ("ligatures", "Standard Ligatures", doc.ligatures, &["liga", "clig"]),
                ("discretionaryLigatures", "Discretionary Ligatures", o.discretionary_ligatures, &["dlig"]),
                ("contextualAlternates", "Contextual Alternates", o.contextual_alternates, &["calt"]),
                ("stylisticAlternates", "Stylistic Alternates", o.stylistic_alternates, &["salt"]),
                ("swash", "Swash", o.swash, &["swsh"]),
                ("titling", "Titling Alternates", o.titling, &["titl"]),
                ("ordinals", "Ordinals", o.ordinals, &["ordn"]),
                ("fractions", "Fractions", o.fractions, &["frac"]),
                ("smallCaps", "Small Caps", doc.small_caps, &["smcp"]),
                ("allSmallCaps", "All Small Caps", o.all_small_caps, &["c2sc"]),
                ("superscript", "Superscript / Superior", doc.baseline == BaselineOption::Superscript, &["sups"]),
                ("subscript", "Subscript / Inferior", doc.baseline == BaselineOption::Subscript, &["subs"]),
            ];
            for (key, label, on, tags) in toggles {
                let avail = tags.iter().any(|g| has(g));
                // Unavailable features stay clickable: small caps and scripts are synthesized.
                let synth = matches!(key, "smallCaps" | "allSmallCaps" | "superscript" | "subscript");
                let text = match (avail, synth) {
                    (true, _) => label.to_string(),
                    (false, true) => format!("{label}  (synthesized)"),
                    (false, false) => format!("{label}  (not in font)"),
                };
                let r = ui.selectable_label(on, egui::RichText::new(text).color(if avail { t.text } else { t.text_faint }));
                rects.push((format!("character.opentype.{key}"), r.rect, label.to_string()));
                if r.clicked() {
                    actions.push(json!({key: !on}));
                }
            }
            ui.separator();
            ui.label(egui::RichText::new(tr("Figures")).small().color(t.text_dim));
            let figs = [
                ("default", "Default Figure Style", FigureStyle::Default, FigureWidth::Default),
                ("tabularLining", "Tabular Lining", FigureStyle::Lining, FigureWidth::Tabular),
                ("proportionalOldstyle", "Proportional Oldstyle", FigureStyle::OldStyle, FigureWidth::Proportional),
                ("proportionalLining", "Proportional Lining", FigureStyle::Lining, FigureWidth::Proportional),
                ("tabularOldstyle", "Tabular Oldstyle", FigureStyle::OldStyle, FigureWidth::Tabular),
            ];
            for (key, label, st, fw) in figs {
                let on = o.figure_style == st && o.figure_width == fw;
                let r = ui.selectable_label(on, label);
                rects.push((format!("character.opentype.figures.{key}"), r.rect, label.to_string()));
                if r.clicked() {
                    actions.push(json!({"figures": key}));
                }
            }
            ui.separator();
            ui.label(egui::RichText::new(tr("Stylistic Sets")).small().color(t.text_dim));
            egui::Grid::new(id.with("sets")).spacing(vec2(3.0, 3.0)).show(ui, |ui| {
                for n in 1..=20u32 {
                    let key = format!("ss{n:02}");
                    let on = o.stylistic_set(n);
                    let avail = has(&key);
                    let r = ui.add_sized(
                        vec2(38.0, 20.0),
                        egui::Button::selectable(on, egui::RichText::new(format!("{n}")).color(if avail || on { t.text } else { t.text_faint })),
                    );
                    rects.push((format!("character.opentype.{key}"), r.rect, format!("Stylistic Set {n}")));
                    if r.clicked() {
                        actions.push(json!({key: !on}));
                    }
                    if n % 5 == 0 {
                        ui.end_row();
                    }
                }
            });
        });
    });
    for (aid, r, label) in rects {
        app.auto.add(&aid, r, &label);
    }
    let outside = widgets::pressed_outside(ui.ctx(), &area.response);
    if outside || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        ui.data_mut(|d| d.insert_temp(open_id, false));
    }
}

/// The seven Paragraph panel alignment buttons: (justify, `layer.setText` key).
const ALIGNS: [(Justify, &str); 7] = [
    (Justify::Left, "left"),
    (Justify::Center, "center"),
    (Justify::Right, "right"),
    (Justify::JustifyLastLeft, "justifyLeft"),
    (Justify::JustifyLastCenter, "justifyCenter"),
    (Justify::JustifyLastRight, "justifyRight"),
    (Justify::JustifyAll, "justifyAll"),
];

/// Lines glyph of an alignment button.
pub fn paint_justify_glyph(p: &egui::Painter, r: Rect, j: Justify, c: egui::Color32) {
    for k in 0..4 {
        let ly = r.min.y + 6.0 + k as f32 * 4.0;
        let full = r.width() - 10.0;
        let last = k == 3;
        let justified = !matches!(j, Justify::Left | Justify::Center | Justify::Right);
        let lw = if (justified && !last) || j == Justify::JustifyAll {
            full
        } else if k % 2 == 1 || last {
            full * 0.6
        } else {
            full
        };
        let anchor = match j {
            Justify::Center | Justify::JustifyLastCenter => 1,
            Justify::Right | Justify::JustifyLastRight => 2,
            _ => 0,
        };
        let lx = match anchor {
            1 => r.center().x - lw / 2.0,
            2 => r.max.x - 5.0 - lw,
            _ => r.min.x + 5.0,
        };
        p.line_segment([pos2(lx, ly), pos2(lx + lw, ly)], egui::Stroke::new(1.4, c));
    }
}

/// The Paragraph panel: alignment, left/right/first-line indents, space before and after,
/// paragraph direction (when the text engine allows it), the composer, and Roman hanging
/// punctuation. Edits are queued and sent as `layer.setText` for the edited range when a text
/// target exists; failures show in `app.ui.status`.
pub fn paragraph(app: &mut EffectcraftApp, ui: &mut egui::Ui, rect: Rect) {
    let t = app.tokens;
    let p = ui.painter().with_clip_rect(rect);
    let ctx = ui.ctx().clone();
    let target = text_target(app);
    let enabled = target.is_some();
    let doc = target.as_ref().map(|t| t.doc.clone()).unwrap_or_default();
    let x0 = rect.min.x + 10.0;
    let w = rect.width() - 20.0;
    let scroll = widgets::PanelScroll::begin(ui, egui::Id::unique("paragraph-scroll"), rect);
    let top = rect.min.y + 10.0 - scroll.offset;
    let mut y = top;
    let mut actions: Vec<serde_json::Value> = vec![];
    let bw = ((w - 6.0 * 3.0) / 7.0).clamp(20.0, 28.0);
    for (i, (j, key)) in ALIGNS.into_iter().enumerate() {
        let r = Rect::from_min_size(pos2(x0 + i as f32 * (bw + 3.0), y), vec2(bw, 24.0));
        let on = doc.justify == j;
        let resp = ui.interact(r, egui::Id::unique(("para", key)), Sense::click());
        p.rect_filled(
            r,
            3.0,
            if on {
                t.accent
            } else if resp.hovered() {
                t.hover
            } else {
                t.field_bg
            },
        );
        paint_justify_glyph(&p, r, j, if on { egui::Color32::WHITE } else { t.text });
        app.auto.add(&format!("paragraph.{key}"), r, key);
        if resp.clicked() && enabled {
            actions.push(json!({"justify": key}));
        }
    }
    y += 34.0;
    // Indents and spacing (AE's two-column grid).
    let fields: [(&str, &str, f64); 6] = [
        ("indentLeft", "→|", doc.indent_left),
        ("indentRight", "|←", doc.indent_right),
        ("indentFirst", "→¶", doc.indent_first),
        ("", "", 0.0),
        ("spaceBefore", "↑¶", doc.space_before),
        ("spaceAfter", "¶↓", doc.space_after),
    ];
    for (i, (key, glyph, v)) in fields.into_iter().enumerate() {
        if key.is_empty() {
            continue;
        }
        let fx = x0 + (i % 2) as f32 * (w / 2.0);
        let fy = y + (i / 2) as f32 * 28.0;
        p.text(pos2(fx, fy + 9.0), Align2::LEFT_CENTER, glyph, Tokens::semibold(11.0), t.text_dim);
        let (r, nv, _) = widgets::hot_number_at(
            ui,
            pos2(fx + 26.0, fy),
            egui::Id::unique(("para-f", key)),
            v,
            &widgets::HotSpec { speed: 0.5, range: (-5000.0, 5000.0), decimals: 0, suffix: " px" },
            &t,
        );
        app.auto.add(&format!("paragraph.{key}"), r, key);
        if let Some(nv) = nv
            && enabled
        {
            actions.push(json!({key: nv, "merge": format!("para-{key}")}));
        }
    }
    y += 3.0 * 28.0 + 6.0;
    // Direction and composer popups. Settings ▸ Type ▸ Text Engine: the South Asian and Middle
    // Eastern engine offers the paragraph direction and the World-Ready composers (the Latin
    // engine shows the direction only for right-to-left text already set).
    let world_ready = app.session.prefs.type_.text_engine == "southAsian";
    let rtl = doc.direction == Direction::Rtl;
    if world_ready || rtl {
        let dr = Rect::from_min_size(pos2(x0, y), vec2(w / 2.0 - 4.0, 22.0));
        let dirs = vec!["Left-to-Right Text".to_string(), "Right-to-Left Text".to_string()];
        let dpop = egui::Id::unique("para-direction-pop");
        if widgets::dropdown(ui, dr, &dirs[rtl as usize], &t, egui::Id::unique("para-direction")).clicked() && enabled {
            widgets::open_popup(ui, dpop);
        }
        app.auto.add("paragraph.direction", dr, "Text direction");
        if let Some(i) = widgets::popup_menu(ui, dpop, dr.left_bottom(), &dirs, Some(rtl as usize)) {
            actions.push(json!({"direction": if i == 1 { "rtl" } else { "ltr" }}));
        }
    }
    let cr = Rect::from_min_size(pos2(x0 + w / 2.0, y), vec2(w / 2.0, 22.0));
    let comps = if world_ready {
        vec!["World-Ready Every-line Composer".to_string(), "World-Ready Single-line Composer".to_string()]
    } else {
        vec!["Every-line Composer".to_string(), "Single-line Composer".to_string()]
    };
    let cpop = egui::Id::unique("para-composer-pop");
    let single = doc.composer == Composer::SingleLine;
    if widgets::dropdown(ui, cr, &comps[single as usize], &t, egui::Id::unique("para-composer")).clicked() && enabled {
        widgets::open_popup(ui, cpop);
    }
    app.auto.add("paragraph.composer", cr, "Composer");
    if let Some(i) = widgets::popup_menu(ui, cpop, cr.left_bottom(), &comps, Some(single as usize)) {
        actions.push(json!({"composer": if i == 1 { "singleLine" } else { "everyLine" }}));
    }
    y += 30.0;
    let hr = Rect::from_min_size(pos2(x0, y), vec2(14.0, 14.0));
    if widgets::checkbox(ui, hr, doc.hanging_punctuation, &t, egui::Id::unique("para-hanging")).clicked() && enabled {
        actions.push(json!({"hangingPunctuation": !doc.hanging_punctuation}));
    }
    app.auto.add("paragraph.hangingPunctuation", hr, "Roman Hanging Punctuation");
    p.text(pos2(x0 + 20.0, y + 7.0), Align2::LEFT_CENTER, tr("Roman Hanging Punctuation"), Tokens::ui(11.5), t.text_dim);
    // (Room for the status line at the bottom.)
    scroll.end(ui, &mut app.auto, "paragraph.scroll", y + 44.0 - top, &t);
    if !enabled {
        p.text(pos2(rect.center().x, rect.max.y - 20.0), Align2::CENTER_CENTER, tr("Select a text layer"), Tokens::ui(11.0), t.text_faint);
    }
    let Some(tt) = target else { return };
    for a in actions {
        if let Err(e) = crate::menus::invoke(app, &ctx, "layer.setText", tt.params(a)) {
            app.ui.status = e;
        }
    }
}

#[derive(Clone, Default)]
struct FontSearch {
    query: String,
    selected: usize,
}

/// Shared font menu for Character and Properties: search, recent fonts and previews.
/// Finish a font rescan started from the font menu and report how many faces it added.
/// While the scan is still walking font folders, ask for another frame so the menu updates.
pub(crate) fn poll_font_rescan(app: &mut EffectcraftApp, ctx: &egui::Context) {
    if effectcraft_engine::text::fonts::rescan_busy() {
        ctx.request_repaint_after(std::time::Duration::from_millis(100));
    }
    if let Some(n) = effectcraft_engine::text::fonts::poll_rescan() {
        app.ui.status = format!("{} +{n}", tr("Rescan Fonts"));
    }
}

pub(super) fn font_picker(app: &mut EffectcraftApp, ui: &mut egui::Ui, id: egui::Id, rect: Rect, current: &str, auto: &str, enabled: bool) -> Option<String> {
    let state_id = id.with("search-state");
    app.auto.add(auto, rect, "Font family");
    if !widgets::popup_is_open(ui, id) || !enabled {
        ui.data_mut(|d| d.remove::<FontSearch>(state_id));
        if widgets::dropdown(ui, rect, current, &app.tokens, id.with("field")).clicked() && enabled {
            widgets::open_popup(ui, id);
        }
        return None;
    }
    let rows = effectcraft_engine::font_menu(&app.session.prefs);
    let saved = ui.data(|d| d.get_temp::<FontSearch>(state_id));
    let fresh = saved.is_none();
    let mut state = saved.unwrap_or_default();
    let search_id = id.with("search");
    let (up, down, enter, escape) = ui.input_mut(|i| {
        (
            i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp),
            i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown),
            i.consume_key(egui::Modifiers::NONE, egui::Key::Enter),
            i.consume_key(egui::Modifiers::NONE, egui::Key::Escape),
        )
    });
    let preview = app.session.prefs.type_.font_preview;
    let t = app.tokens;
    // The Variable Fonts filter (After Effects 26.3) stays as set while the app runs, like the
    // menu's other settings; the search query resets on every open.
    let variable_id = id.with("variable-only");
    let mut only_variable = ui.data(|d| d.get_temp::<bool>(variable_id)).unwrap_or(false);
    // The font field itself becomes editable. The popup contains only results.
    let arrow = Rect::from_min_max(pos2(rect.max.x - 22.0, rect.min.y), rect.max);
    let edit_rect = Rect::from_min_max(rect.min, pos2(arrow.min.x, rect.max.y));
    let search = ui.put(edit_rect, egui::TextEdit::singleline(&mut state.query).id(search_id).hint_text(current).font(Tokens::ui(12.0)));
    app.auto.add(&format!("{auto}.search"), search.rect, "Search fonts");
    if fresh {
        search.request_focus();
    }
    ui.memory_mut(|m| m.set_focus_lock_filter(search_id, egui::EventFilter { vertical_arrows: true, escape: true, ..Default::default() }));
    if widgets::icon_button(ui, arrow, crate::icons::Icon::ChevronDown, false, &t, id.with("arrow"))
        .on_hover_text(crate::i18n::tr("Close font search"))
        .clicked()
    {
        ui.data_mut(|d| {
            d.insert_temp(id.with("open"), false);
            d.remove::<FontSearch>(state_id);
        });
        ui.memory_mut(|m| m.surrender_focus(search_id));
        return None;
    }
    app.auto.add(&format!("{auto}.toggle"), arrow, "Close font list");
    widgets::note_open_popup_list(ui.ctx());
    let changed = search.changed();
    let mut open = true;
    // The search lives in the field; the menu's header is the filter row, its list the fonts.
    let menu = craft_ui::combo::Menu::new(ui, &search)
        .open(&mut open)
        .height(380.0)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .style(egui::style::StyleModifier::default());
    let app_ref = &mut *app;
    let header = |ui: &mut egui::Ui| {
        ui.set_width(rect.width().max(180.0));
        // Filter row: a checkbox that keeps only the variable fonts.
        let filter_h = 22.0;
        let row = ui.add_sized([ui.available_width(), filter_h], egui::Button::selectable(false, ""));
        let box_rect = Rect::from_min_size(row.rect.min, vec2(18.0, filter_h));
        let label_rect = Rect::from_min_max(pos2(box_rect.max.x + 2.0, row.rect.min.y), row.rect.max);
        let boxed = widgets::checkbox(ui, box_rect, only_variable, &t, id.with("variable-check"));
        ui.painter().text(label_rect.left_center(), Align2::LEFT_CENTER, tr("Variable fonts only"), Tokens::ui(11.5), t.text);
        app_ref.auto.add(&format!("{auto}.variable"), row.rect, "Variable fonts only");
        // #1089: fonts installed after launch appear without restarting. The button is its own
        // row: the popup is only 180 px wide, so a control pinned to the filter row covers
        // "Variable fonts only". The scan itself runs off the UI thread (`poll_font_rescan`).
        let rescan_h = 22.0;
        let (resc, _) = ui.allocate_exact_size(vec2(ui.available_width(), rescan_h), egui::Sense::hover());
        if widgets::text_button(ui, resc, tr("Rescan Fonts"), false, &t, id.with("rescan")).clicked() && effectcraft_engine::text::fonts::start_rescan() {
            app_ref.ui.status = format!("{}…", tr("Rescan Fonts"));
        }
        app_ref.auto.add(&format!("{auto}.rescan"), resc, "Rescan Fonts");
        let toggled = row.clicked() || boxed.clicked();
        if toggled {
            only_variable = !only_variable;
            ui.data_mut(|d| d.insert_temp(variable_id, only_variable));
            // Typing goes on into the search field.
            ui.memory_mut(|m| m.request_focus(search_id));
        }
        let query = state.query.trim().to_lowercase();
        let mut seen = std::collections::HashSet::new();
        let filtered: Vec<_> = rows
            .iter()
            .filter(|r| {
                !r.family.is_empty()
                    && (!only_variable || r.variable)
                    && (query.is_empty() || ((r.family.to_lowercase().contains(&query) || r.display.to_lowercase().contains(&query)) && seen.insert(&r.family)))
            })
            .collect();
        if fresh {
            state.selected = filtered.iter().position(|r| r.family == current).unwrap_or(0);
        }
        if changed || toggled {
            state.selected = 0;
        }
        let last = filtered.len().saturating_sub(1);
        state.selected = state.selected.min(last);
        if down {
            state.selected = state.selected.saturating_add(1).min(last);
        }
        if up {
            state.selected = state.selected.saturating_sub(1);
        }
        let entered = if enter { filtered.get(state.selected).map(|r| r.family.clone()) } else { None };
        (app_ref, filtered, query.is_empty(), state.selected, changed || toggled, entered)
    };
    let list = |ui: &mut egui::Ui,
                (app, filtered, all, selected, restart, entered): (
        &mut EffectcraftApp,
        Vec<&effectcraft_engine::FontRow>,
        bool,
        usize,
        bool,
        Option<String>,
    )| {
        if restart {
            ui.scroll_to_cursor(Some(egui::Align::TOP));
        }
        let mut chosen = entered;
        if filtered.is_empty() {
            let r = ui.label(tr("No matching fonts"));
            app.auto.add(&format!("{auto}.empty"), r.rect, "No matching fonts");
        }
        let mut was_recent = false;
        for (index, r) in filtered.iter().enumerate() {
            if all && was_recent && !r.recent {
                ui.separator();
            }
            was_recent = r.recent;
            let row_width = ui.available_width();
            let label_width = if preview { (row_width - 100.0).max(40.0) } else { row_width };
            let resp = ui.add_sized([row_width, 24.0], egui::Button::selectable(index == selected, ""));
            resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, ui.is_enabled(), index == selected, &r.display));
            let label_rect = Rect::from_min_size(resp.rect.min + vec2(5.0, 0.0), vec2(label_width - 5.0, resp.rect.height()));
            let name = drawn_font_name(ui.ctx(), r);
            ui.painter().with_clip_rect(label_rect.intersect(ui.clip_rect())).text(
                label_rect.left_center(),
                Align2::LEFT_CENTER,
                name,
                Tokens::ui(12.0),
                t.text,
            );
            if ui.is_rect_visible(resp.rect) {
                app.auto.add(&format!("{auto}.option.{}", r.family), resp.rect, &r.display);
            }
            if index == selected && (fresh || up || down) {
                resp.scroll_to_me(None);
            }
            if preview && ui.is_rect_visible(resp.rect) {
                let key = egui::Id::unique(("font-preview", &r.family));
                let lines: std::sync::Arc<Vec<Vec<[f32; 2]>>> = match ui.data(|d| d.get_temp(key)) {
                    Some(l) => l,
                    None => {
                        let l = std::sync::Arc::new(effectcraft_engine::font_preview(&r.family, 14.0));
                        ui.data_mut(|d| d.insert_temp(key, l.clone()));
                        l
                    }
                };
                // Scaled and placed to fit the row's preview column, and clipped to the row (#577).
                let room = Rect::from_min_max(pos2(resp.rect.max.x - 95.0, resp.rect.min.y + 1.0), pos2(resp.rect.max.x - 6.0, resp.rect.max.y - 1.0));
                if let Some((k, o)) = fit_preview(&lines, room, resp.rect.center().y + 5.0) {
                    let painter = ui.painter().with_clip_rect(resp.rect.intersect(ui.clip_rect()));
                    for poly in lines.iter() {
                        let pts: Vec<egui::Pos2> =
                            poly.iter().filter(|p| p[0].is_finite() && p[1].is_finite()).map(|p| (o + vec2(p[0], p[1]) * k).to_pos2()).collect();
                        painter.add(egui::Shape::line(pts, egui::Stroke::new(1.0, t.text_dim)));
                    }
                }
            }
            if resp.clicked() {
                chosen = Some(r.family.clone());
            }
            resp.on_hover_text(name);
        }
        chosen
    };
    let chosen = menu.show_ui_with_header(header, list).flatten();
    if chosen.is_some() || !open || escape {
        ui.data_mut(|d| {
            d.insert_temp(id.with("open"), false);
            d.remove::<FontSearch>(state_id);
        });
        ui.memory_mut(|m| m.surrender_focus(search_id));
    } else {
        ui.data_mut(|d| d.insert_temp(state_id, state));
    }
    chosen
}

/// Align panel: Align Layers to Selection / Composition, the six align buttons and the six
/// Distribute Layers buttons. Buttons run `layer.align {edge, to}` and `layer.distribute
/// {mode}` on the selected layers.
pub fn align(app: &mut EffectcraftApp, ui: &mut egui::Ui, rect: Rect) {
    let t = app.tokens;
    let p = ui.painter().with_clip_rect(rect);
    let x0 = rect.min.x + 10.0;
    let mut y = rect.min.y + 10.0;
    p.text(pos2(x0, y + 10.0), Align2::LEFT_CENTER, tr("Align Layers to:"), Tokens::ui(11.5), t.text_dim);
    let dr = Rect::from_min_size(pos2(x0 + 100.0, y), vec2(110.0, 20.0));
    let to_sel = app.ui.align_to_selection;
    let pid = egui::Id::unique("align-to-pop");
    if widgets::dropdown(ui, dr, if to_sel { "Selection" } else { "Composition" }, &t, pid.with("btn")).clicked() {
        widgets::open_popup(ui, pid);
    }
    app.auto.add("align.to", dr, if to_sel { "Selection" } else { "Composition" });
    if let Some(i) =
        widgets::popup_menu(ui, pid, dr.left_bottom() + vec2(0.0, 2.0), &["Selection".to_string(), "Composition".to_string()], Some(if to_sel { 0 } else { 1 }))
    {
        app.ui.align_to_selection = i == 0;
    }
    y += 28.0;
    let ops = ["left", "hcenter", "right", "top", "vcenter", "bottom"];
    let mut run: Option<(&str, &str)> = None;
    for (row, kind) in ["align", "distribute"].into_iter().enumerate() {
        if row == 1 {
            p.text(pos2(x0, y + 8.0), Align2::LEFT_CENTER, tr("Distribute Layers:"), Tokens::ui(11.5), t.text_dim);
            y += 20.0;
        }
        for (i, op) in ops.iter().enumerate() {
            let r = Rect::from_min_size(pos2(x0 + i as f32 * 34.0 + if i >= 3 { 12.0 } else { 0.0 }, y), vec2(30.0, 28.0));
            let resp = ui.interact(r, egui::Id::unique((kind, *op)), Sense::click());
            p.rect_filled(r, 3.0, if resp.hovered() { t.hover } else { t.field_bg });
            align_glyph(&p, r, op, row == 1, &t);
            let tip = align_tip(kind, op);
            app.auto.add(&format!("{kind}.{op}"), r, &tip);
            if resp.on_hover_text(tip).clicked() {
                run = Some((kind, op));
            }
        }
        y += 34.0;
    }
    if let Some((kind, op)) = run {
        let r = if kind == "align" {
            let to = if app.ui.align_to_selection { "selection" } else { "composition" };
            app.session.execute("layer.align", json!({"edge": op, "to": to}))
        } else {
            app.session.execute("layer.distribute", json!({"mode": op}))
        };
        if let Err(e) = r {
            app.ui.status = e.to_string();
        }
    }
}

fn align_tip(kind: &str, op: &str) -> String {
    let what = match op {
        "left" => "Left",
        "hcenter" => "Horizontal Center",
        "right" => "Right",
        "top" => "Top",
        "vcenter" => "Vertical Center",
        _ => "Bottom",
    };
    if kind == "align" { format!("Align {what}") } else { format!("Distribute {what}") }
}

/// Button glyph: the alignment edge as a line and two boxes (distribute: three boxes spread,
/// the line through each).
fn align_glyph(p: &egui::Painter, r: Rect, op: &str, distribute: bool, t: &Tokens) {
    let c = r.center();
    let st = egui::Stroke::new(1.3, t.text);
    let horizontal = matches!(op, "left" | "hcenter" | "right");
    if distribute {
        for k in 0..3 {
            let f = (k as f32 - 1.0) * 7.0;
            if horizontal {
                let bx = c.x + f;
                let ex = match op {
                    "left" => bx - 2.5,
                    "right" => bx + 2.5,
                    _ => bx,
                };
                p.rect_filled(Rect::from_center_size(pos2(bx, c.y), vec2(5.0, 10.0 + k as f32 * 2.0)), 1.0, t.text_dim);
                p.line_segment([pos2(ex, r.min.y + 5.0), pos2(ex, r.max.y - 5.0)], st);
            } else {
                let by = c.y + f;
                let ey = match op {
                    "top" => by - 2.5,
                    "bottom" => by + 2.5,
                    _ => by,
                };
                p.rect_filled(Rect::from_center_size(pos2(c.x, by), vec2(10.0 + k as f32 * 2.0, 5.0)), 1.0, t.text_dim);
                p.line_segment([pos2(r.min.x + 5.0, ey), pos2(r.max.x - 5.0, ey)], st);
            }
        }
        return;
    }
    if horizontal {
        let ex = match op {
            "left" => r.min.x + 7.0,
            "right" => r.max.x - 7.0,
            _ => c.x,
        };
        p.line_segment([pos2(ex, r.min.y + 5.0), pos2(ex, r.max.y - 5.0)], st);
        let off = |w: f32| match op {
            "left" => ex,
            "right" => ex - w,
            _ => ex - w / 2.0,
        };
        p.rect_filled(Rect::from_min_size(pos2(off(14.0), c.y - 7.0), vec2(14.0, 5.0)), 1.0, t.text_dim);
        p.rect_filled(Rect::from_min_size(pos2(off(9.0), c.y + 2.0), vec2(9.0, 5.0)), 1.0, t.text_dim);
    } else {
        let ey = match op {
            "top" => r.min.y + 6.0,
            "bottom" => r.max.y - 6.0,
            _ => c.y,
        };
        p.line_segment([pos2(r.min.x + 5.0, ey), pos2(r.max.x - 5.0, ey)], st);
        let off = |h: f32| match op {
            "top" => ey,
            "bottom" => ey - h,
            _ => ey - h / 2.0,
        };
        p.rect_filled(Rect::from_min_size(pos2(c.x - 8.0, off(14.0)), vec2(5.0, 14.0)), 1.0, t.text_dim);
        p.rect_filled(Rect::from_min_size(pos2(c.x + 3.0, off(9.0)), vec2(5.0, 9.0)), 1.0, t.text_dim);
    }
}

/// The name a font menu row draws: the font's own-language name when the interface fonts have a
/// glyph for each of its characters, else its English family name. A Simplified Chinese name
/// such as 微软雅黑 can have characters that the Han face of an English interface lacks.
fn drawn_font_name<'a>(ctx: &egui::Context, row: &'a effectcraft_engine::FontRow) -> &'a str {
    if ctx.fonts_mut(|f| f.has_glyphs(&Tokens::ui(12.0), &row.display)) { &row.display } else { &row.family }
}

/// Where a font's "Sample" outlines (`lines`, baseline at y = 0) go in `room`: `(scale, origin)`,
/// a point `p` landing at `origin + p * scale`. At their natural size on `baseline` when they fit;
/// fonts with large or odd metrics are scaled down (never up) and moved inside, so no preview
/// spills out of its row (#577). `None`: nothing to draw.
fn fit_preview(lines: &[Vec<[f32; 2]>], room: Rect, baseline: f32) -> Option<(f32, egui::Vec2)> {
    let b = lines
        .iter()
        .flatten()
        .filter(|p| p[0].is_finite() && p[1].is_finite())
        .fold(Rect::NOTHING, |b, p| b.union(Rect::from_min_max(pos2(p[0], p[1]), pos2(p[0], p[1]))));
    if !(b.min.x <= b.max.x && b.min.y <= b.max.y && room.is_positive()) {
        return None;
    }
    let k = (room.width() / b.width().max(1e-3)).min(room.height() / b.height().max(1e-3)).min(1.0);
    let mut o = vec2(room.min.x - b.min.x * k, baseline);
    let (top, bottom) = (o.y + b.min.y * k, o.y + b.max.y * k);
    if top < room.min.y {
        o.y += room.min.y - top;
    } else if bottom > room.max.y {
        o.y -= bottom - room.max.y;
    }
    Some((k, o))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// #577: every font's "Sample" preview lands inside its row's preview box: at its natural size
    /// when it fits, else scaled down; nothing for no outlines.
    #[test]
    fn font_previews_fit_their_row() {
        let room = Rect::from_min_max(pos2(180.0, 101.0), pos2(294.0, 119.0));
        let baseline = 115.0;
        let inside = |lines: &[Vec<[f32; 2]>]| {
            let (k, o) = fit_preview(lines, room, baseline).expect("a preview");
            for p in lines.iter().flatten() {
                let q = o + vec2(p[0], p[1]) * k;
                assert!(room.expand(0.01).contains(pos2(q.x, q.y)), "{p:?} → {q:?} outside {room:?}");
            }
            k
        };
        // A plain 14 px sample fits as it is, on the baseline.
        let plain = vec![vec![[0.0, -10.0], [60.0, -10.0], [60.0, 3.0], [0.0, 3.0]]];
        assert_eq!(inside(&plain), 1.0);
        assert_eq!(fit_preview(&plain, room, baseline).map(|(_, o)| o.y), Some(baseline));
        // Huge, wide, tall, offset and negative-bearing outlines are scaled into the box.
        for lines in [
            vec![vec![[0.0, -80.0], [400.0, 30.0]]],
            vec![vec![[-50.0, -5.0], [500.0, 2.0]]],
            vec![vec![[10.0, -200.0], [12.0, 0.0]]],
            vec![vec![[0.0, 20.0], [30.0, 40.0]]],
        ] {
            assert!(inside(&lines) <= 1.0);
        }
        assert!(fit_preview(&[], room, baseline).is_none());
        assert!(fit_preview(&[vec![[f32::NAN, 1.0]]], room, baseline).is_none());
    }

    /// A font menu row draws the font's own-language name when the interface fonts have its
    /// glyphs, and the English family name when they do not.
    #[test]
    fn font_menu_rows_draw_a_name_the_interface_fonts_have() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx, &Tokens::for_kind(Default::default()), "en");
        ctx.run_ui(Default::default(), |_| {}).textures_delta.clear();
        let row = |display: &str| effectcraft_engine::FontRow { family: "Menu Name Test".into(), display: display.into(), recent: false, variable: false };
        // U+F1234 is in a private use plane that no interface font maps.
        assert_eq!(drawn_font_name(&ctx, &row("Menu \u{F1234}")), "Menu Name Test");
        assert_eq!(drawn_font_name(&ctx, &row("Nom du menu")), "Nom du menu");
    }
}
