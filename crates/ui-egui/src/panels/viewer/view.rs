use super::tools::draw_rigs;
use super::{Align2, Color32, EffectcraftApp, EvalCtx, Rect, Sense, Stroke, StrokeKind, Tokens, View3D, ViewerMap, json, pos2, three_d, vec2, widgets};

/// Draw the extra views of a 2- or 4-view layout and return the main view's rectangle.
pub(super) fn aux_views(
    app: &mut EffectcraftApp,
    ui: &mut egui::Ui,
    comp: &effectcraft_engine::project::Comp,
    cid: effectcraft_engine::project::ItemId,
    full: Rect,
    bg: Color32,
) -> Rect {
    let n = app.session.state.view_layout;
    if n < 2 {
        return full;
    }
    let gap = 2.0;
    let views = app.session.state.views3d.get(&cid).cloned().unwrap_or_default();
    let (main, aux): (Rect, Vec<(Rect, View3D)>) = if n == 2 {
        let mid = full.center().x;
        (
            Rect::from_min_max(pos2(mid + gap / 2.0, full.min.y), full.max),
            vec![(Rect::from_min_max(full.min, pos2(mid - gap / 2.0, full.max.y)), views.pane(0))],
        )
    } else {
        let c = full.center();
        let q = |x0: f32, y0: f32, x1: f32, y1: f32| Rect::from_min_max(pos2(x0, y0), pos2(x1, y1));
        (
            q(c.x + gap / 2.0, c.y + gap / 2.0, full.max.x, full.max.y),
            vec![
                (q(full.min.x, full.min.y, c.x - gap / 2.0, c.y - gap / 2.0), views.pane(0)),
                (q(c.x + gap / 2.0, full.min.y, full.max.x, c.y - gap / 2.0), views.pane(1)),
                (q(full.min.x, c.y + gap / 2.0, c.x - gap / 2.0, full.max.y), views.pane(2)),
            ],
        )
    };
    let ctx = ui.ctx().clone();
    let p = ui.painter().clone();
    p.rect_filled(full, 0.0, Color32::from_black_alpha(200));
    let t = app.session.time();
    let (cw, ch) = (comp.width as f32, comp.height as f32);
    let share = app.session.state.share_view_options;
    // The camera and light wireframes of each view (as in After Effects, where every view of the
    // layout shows them), evaluated on a snapshot of the project.
    let snap_project = app.session.project.clone();
    let snap_expr = app.session.expr.clone();
    let ectx = EvalCtx {
        project: &snap_project,
        comp_id: cid,
        comp,
        time: t,
        expr: snap_expr.as_deref(),
        footage: None,
        property_overrides: &[],
        media_overrides: &[],
    };
    let selected = app.session.state.selected_layers.clone();
    let mut picked: Option<(usize, View3D)> = None;
    for (i, (r, v)) in aux.into_iter().enumerate() {
        p.rect_filled(r, 0.0, bg);
        let fit = ((r.width() - 20.0) / cw).min((r.height() - 20.0) / ch).max(0.01);
        let cr = Rect::from_center_size(r.center(), vec2(cw * fit, ch * fit));
        let scale = (fit * ctx.pixels_per_point()).min(1.0) as f64;
        // The Active Camera pane renders through the comp's own camera; the others through their
        // view camera.
        let through_active = v == View3D::ActiveCamera;
        let cam = if through_active { three_d::active_camera(&ectx) } else { views.cam(v, comp.width as f64, comp.height as f64).state() };
        let key = {
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            (app.session.revision, cid.0, t.0, i, scale.to_bits(), format!("{cam:?}")).hash(&mut h);
            h.finish()
        };
        let id = egui::Id::unique(("viewer-aux", i));
        let cached: Option<(u64, egui::TextureHandle)> = ctx.data(|d| d.get_temp(id));
        let tex = match cached {
            Some((k, tex)) if k == key => tex,
            _ => {
                let opts = effectcraft_engine::render::RenderOpts {
                    scale,
                    view: (comp.has_3d() && !through_active).then_some(cam),
                    draft: true,
                    ..Default::default()
                };
                let img = app.session.render(cid, t, opts);
                let tex = crate::frames::load_fitted(&ctx, format!("viewer-aux-{i}"), crate::frames::to_color_image(&img), egui::TextureOptions::LINEAR);
                ctx.data_mut(|d| d.insert_temp(id, (key, tex.clone())));
                tex
            }
        };
        let b = comp.background;
        p.rect_filled(cr, 0.0, Color32::from_rgb((b[0] * 255.0) as u8, (b[1] * 255.0) as u8, (b[2] * 255.0) as u8));
        p.image(tex.id(), cr, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
        p.rect_stroke(cr, 0.0, Stroke::new(1.0, Color32::from_black_alpha(160)), StrokeKind::Outside);
        if share && app.ui.viewer.safe_margins {
            for k in [0.9, 0.8] {
                p.rect_stroke(Rect::from_center_size(cr.center(), cr.size() * k), 0.0, Stroke::new(1.0, Color32::from_white_alpha(120)), StrokeKind::Middle);
            }
        }
        if comp.has_3d() {
            let map = ViewerMap { origin: cr.min, zoom: fit, comp: [cw, ch], area: r };
            draw_rigs(app, &p.with_clip_rect(r), &map, &ectx, &selected, cam, through_active, Some(v.id()));
        }
        let label = if comp.has_3d() { v.label() } else { crate::i18n::tr("Active Camera") };
        p.text(r.left_bottom() + vec2(8.0, -8.0), Align2::LEFT_BOTTOM, label, Tokens::ui(11.0), Color32::from_white_alpha(200));
        app.auto.add(&format!("viewer.view.{}", v.id()), r, label);
        if comp.has_3d()
            && let Some(chosen) = pane_selector(app, ui, i + 1, r, v)
        {
            picked = Some((i + 1, chosen));
        }
    }
    if let Some((pane, v)) = picked
        && let Err(e) = app.session.execute("view.setPaneView", json!({"pane": pane, "view": v.id(), "comp": cid.0}))
    {
        app.ui.status = e.to_string();
    }
    main
}

/// The view selector in the top-left corner of pane `pane` (1 to 3) of a multi-view layout, a
/// popup of every 3D view like the toolbar's 3D View menu. Returns the view chosen this frame.
fn pane_selector(app: &mut EffectcraftApp, ui: &mut egui::Ui, pane: usize, area: Rect, current: View3D) -> Option<View3D> {
    let t = app.tokens;
    let r = Rect::from_min_size(area.min + vec2(6.0, 6.0), vec2(124.0, 20.0));
    let pid = egui::Id::unique(("viewer-pane-view", pane));
    let response = widgets::dropdown(ui, r, crate::i18n::tr(current.label()), &t, pid);
    if response.clicked() {
        let open = widgets::popup_is_open(ui, pid);
        ui.data_mut(|d| d.insert_temp(pid.with("open"), !open));
    }
    app.auto.add_response(&format!("viewer.view.{pane}.type"), &response, "View of pane");
    if !widgets::popup_is_open(ui, pid) {
        return None;
    }
    let chosen = widgets::popup_list(ui, pid, r.left_bottom() + vec2(0.0, 2.0), r, View3D::ALL.len(), 0, |ui| {
        ui.set_min_width(124.0);
        let mut chosen = None;
        for (k, v) in View3D::ALL.iter().enumerate() {
            let item = ui.selectable_label(*v == current, crate::i18n::tr(v.label()));
            app.auto.add(&format!("viewer.view.{pane}.typeItem.{k}"), item.rect, v.label());
            if item.clicked() {
                chosen = Some(*v);
            }
        }
        chosen
    });
    chosen.filter(|v| *v != current)
}

/// View ▸ Split with New Locked Viewer: the locked viewer on the left of the Composition panel
/// (its comp and 3D view stay put whatever comp is active). Returns the main viewer's rectangle.
pub(super) fn locked_pane(app: &mut EffectcraftApp, ui: &mut egui::Ui, full: Rect, bg: Color32) -> Rect {
    let Some(lv) = app.session.state.locked_viewer else { return full };
    let Some(comp) = app.session.project.comp_arc(lv.comp) else { return full };
    let name = app.session.project.item(lv.comp).map(|i| i.name.clone()).unwrap_or_default();
    let gap = 2.0;
    let mid = full.center().x;
    let r = Rect::from_min_max(full.min, pos2(mid - gap / 2.0, full.max.y));
    let main = Rect::from_min_max(pos2(mid + gap / 2.0, full.min.y), full.max);
    let ctx = ui.ctx().clone();
    let p = ui.painter().clone();
    p.rect_filled(Rect::from_min_max(pos2(r.max.x, full.min.y), pos2(main.min.x, full.max.y)), 0.0, Color32::from_black_alpha(200));
    p.rect_filled(r, 0.0, bg);
    let (cw, ch) = (comp.width as f32, comp.height as f32);
    let fit = ((r.width() - 20.0) / cw).min((r.height() - 44.0) / ch).max(0.01);
    let cr = Rect::from_center_size(r.center(), vec2(cw * fit, ch * fit));
    let scale = (fit * ctx.pixels_per_point()).min(1.0) as f64;
    let t = app.session.time_of(lv.comp);
    let cam = (lv.view != View3D::ActiveCamera && comp.has_3d())
        .then(|| app.session.state.views3d.get(&lv.comp).cloned().unwrap_or_default().cam(lv.view, comp.width as f64, comp.height as f64).state());
    let dc = effectcraft_engine::viewer::DisplayColor::of(&app.session);
    // The display conversion's inputs, keyed as in the main viewer's texture caches.
    let dc_key = format!("{cam:?}{}", crate::panels::viewer_tools::display_key(app, dc.is_some()));
    let key = {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (app.session.revision, lv.comp.0, t.0, scale.to_bits(), dc_key).hash(&mut h);
        h.finish()
    };
    let id = egui::Id::unique("viewer-locked");
    let cached: Option<(u64, egui::TextureHandle)> = ctx.data(|d| d.get_temp(id));
    let tex = match cached {
        Some((k, tex)) if k == key => tex,
        _ => {
            let opts = effectcraft_engine::render::RenderOpts { scale, view: cam, ..Default::default() };
            let img = app.session.render(lv.comp, t, opts);
            let mut ci = crate::frames::to_color_image(&img);
            if let Some(dc) = dc {
                let mut px: Vec<[u8; 4]> = ci.pixels.iter().map(|c| c.to_array()).collect();
                dc.apply(&mut px);
                ci = egui::ColorImage::new(ci.size, px.into_iter().map(|a| Color32::from_rgba_premultiplied(a[0], a[1], a[2], a[3])).collect());
            }
            let tex = crate::frames::load_fitted(&ctx, "viewer-locked", ci, egui::TextureOptions::LINEAR);
            ctx.data_mut(|d| d.insert_temp(id, (key, tex.clone())));
            tex
        }
    };
    let b = comp.background;
    p.rect_filled(cr, 0.0, Color32::from_rgb((b[0] * 255.0) as u8, (b[1] * 255.0) as u8, (b[2] * 255.0) as u8));
    p.image(tex.id(), cr, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
    p.rect_stroke(cr, 0.0, Stroke::new(1.0, Color32::from_black_alpha(160)), StrokeKind::Outside);
    let label = crate::i18n::tr_args("Locked: {} — {}", &[&name, &(if comp.has_3d() { lv.view.label() } else { crate::i18n::tr("Active Camera") })]);
    p.text(r.left_top() + vec2(8.0, 8.0), Align2::LEFT_TOP, &label, Tokens::ui(11.0), Color32::from_white_alpha(210));
    app.auto.add("viewer.locked", r, &label);
    // Close the locked viewer.
    let close = Rect::from_min_size(pos2(r.max.x - 26.0, r.min.y + 4.0), vec2(20.0, 20.0));
    let resp = ui.interact(close, egui::Id::unique("viewer-locked-close"), Sense::click());
    p.text(close.center(), Align2::CENTER_CENTER, "\u{00d7}", Tokens::ui(14.0), if resp.hovered() { Color32::WHITE } else { Color32::from_white_alpha(170) });
    app.auto.add("viewer.locked.close", close, "Close Locked Viewer");
    if resp.clicked() {
        let _ = app.session.execute("view.closeLockedViewer", serde_json::json!({}));
    }
    main
}

pub(crate) fn checker(p: &egui::Painter, r: Rect) {
    let s = 10.0;
    p.rect_filled(r, 0.0, Color32::from_gray(0xcc));
    let mut y = r.min.y;
    let mut row = 0;
    while y < r.max.y {
        let mut x = r.min.x + if row % 2 == 0 { 0.0 } else { s };
        while x < r.max.x {
            p.rect_filled(Rect::from_min_max(pos2(x, y), pos2((x + s).min(r.max.x), (y + s).min(r.max.y))), 0.0, Color32::from_gray(0x99));
            x += 2.0 * s;
        }
        y += s;
        row += 1;
    }
}
