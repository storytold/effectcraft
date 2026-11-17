use super::tools::draw_rigs;
use super::{Align2, Color32, EffectcraftApp, EvalCtx, Rect, Sense, Stroke, StrokeKind, Tokens, View3D, json, multiview, pos2, three_d, vec2, vt, widgets};

/// Lay out the panes of View ▸ Switch View Layout in `full`, draw the inactive ones and return
/// the active pane's rectangle, where the viewer's pipeline runs. In a comp with 3D layers an
/// inactive pane shows its view's picture, the camera and light wireframes and the selected
/// layers' outlines, anchor points and axis gizmos, and takes the press that makes it active
/// (`multiview`); in a 2D comp it is the picture alone, as before.
pub(super) fn aux_views(
    app: &mut EffectcraftApp,
    ui: &mut egui::Ui,
    comp: &effectcraft_engine::project::Comp,
    cid: effectcraft_engine::project::ItemId,
    full: Rect,
    bg: Color32,
) -> Rect {
    let n = app.session.state.view_layout.max(1);
    let views = app.session.state.views3d.get(&cid).cloned().unwrap_or_default();
    let active = multiview::active_pos(app, cid);
    let mut panes = multiview::layout_rects(full, n, &multiview::shown_positions(app, cid, n));
    // The layout commands keep the active position among the shown ones; editor state set
    // directly may not: then the active pane shows alone rather than over hidden panes.
    if !panes.iter().any(|(pos, _)| *pos == active) {
        panes = vec![(active, full)];
    }
    let main = panes.iter().find(|(pos, _)| *pos == active).map_or(full, |(_, r)| *r);
    let ctx = ui.ctx().clone();
    // The active pane draws no view selector: one left open would be open again when the pane
    // is a side pane again.
    close_selector(&ctx, active);
    let label_of = |v: View3D| if comp.has_3d() { super::view3d_label(v) } else { crate::i18n::tr("Active Camera") };
    for (pos, r) in &panes {
        app.auto.add(&format!("viewer.pane.{pos}"), *r, label_of(views.view_at(*pos)));
    }
    if panes.len() < 2 {
        multiview::publish(&ctx, cid, active, vec![(active, main)], vec![]);
        return main;
    }
    let p = ui.painter().clone();
    p.rect_filled(full, 0.0, Color32::from_black_alpha(200));
    let t = app.session.time();
    let (cw, ch) = (comp.width as f32, comp.height as f32);
    let ppp = ctx.pixels_per_point();
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
        reads: None,
    };
    let selected = app.session.state.selected_layers.clone();
    let mut picked: Option<(usize, View3D)> = None;
    let mut shown = vec![(active, main)];
    let mut corners = vec![];
    for (pos, r) in panes.into_iter().filter(|(pos, _)| *pos != active) {
        let v = views.view_at(pos);
        p.rect_filled(r, 0.0, bg);
        // Framed as the pane will be when it is active, on the area the rulers leave it, so its
        // picture and layer controls stay put then. The rulers' strip stays empty here.
        let inner = vt::inset(app, r);
        let frame = app.ui.viewer.pane_frames.get(pos).copied().unwrap_or_default();
        let (map, _) = multiview::fit_map(inner, [cw, ch], frame, app.ui.viewer.fit_up_to_100, ppp);
        let cr = Rect::from_min_size(map.origin, vec2(cw * map.zoom, ch * map.zoom));
        let scale = (map.zoom * ppp).min(1.0) as f64;
        // The Active Camera pane renders through the comp's own camera; the others through their
        // view camera.
        let through_active = v == View3D::ActiveCamera;
        let cam = if through_active { three_d::active_camera(&ectx) } else { views.cam(v, comp.width as f64, comp.height as f64).state() };
        let key = {
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            (app.session.revision, cid.0, t.0, pos, scale.to_bits(), format!("{cam:?}")).hash(&mut h);
            h.finish()
        };
        let id = egui::Id::unique(("viewer-aux", pos));
        let cached: Option<(u64, u64, egui::TextureHandle)> = ctx.data(|d| d.get_temp(id));
        let tex = match cached {
            Some((k, _, tex)) if k == key => tex,
            _ => {
                let opts = effectcraft_engine::render::RenderOpts {
                    scale,
                    view: (comp.has_3d() && !through_active).then_some(cam),
                    draft: true,
                    ..Default::default()
                };
                let img = app.session.render(cid, t, opts);
                let tex = crate::frames::load_fitted(&ctx, format!("viewer-aux-{pos}"), crate::frames::to_color_image(&img), egui::TextureOptions::LINEAR);
                ctx.data_mut(|d| d.insert_temp(id, (key, cid.0, tex.clone())));
                tex
            }
        };
        let pp = p.with_clip_rect(inner);
        let b = comp.background;
        pp.rect_filled(cr, 0.0, Color32::from_rgb((b[0] * 255.0) as u8, (b[1] * 255.0) as u8, (b[2] * 255.0) as u8));
        pp.image(tex.id(), cr, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
        pp.rect_stroke(cr, 0.0, Stroke::new(1.0, Color32::from_black_alpha(160)), StrokeKind::Outside);
        if share && app.ui.viewer.safe_margins {
            for k in [0.9, 0.8] {
                pp.rect_stroke(Rect::from_center_size(cr.center(), cr.size() * k), 0.0, Stroke::new(1.0, Color32::from_white_alpha(120)), StrokeKind::Middle);
            }
        }
        if comp.has_3d() {
            draw_rigs(app, &pp, &map, &ectx, &selected, cam, through_active, Some(v.id()));
            multiview::draw_selection(app, &pp, &map, &ectx, pos, (!through_active).then_some(cam));
        }
        let label = label_of(v);
        p.text(r.left_bottom() + vec2(8.0, -8.0), Align2::LEFT_BOTTOM, label, Tokens::ui(11.0), Color32::from_white_alpha(200));
        app.auto.add(&format!("viewer.view.{}", v.id()), r, label);
        // In a comp with 3D layers the pane takes a press, which `multiview::pre_activate` then
        // hands to it as the active pane in the same pass. Registered before the view selector,
        // which stays on top and takes its own presses. A comp with no 3D layer keeps its side
        // panes passive pictures, as before.
        if comp.has_3d() {
            ui.interact(r, multiview::input_id(pos), Sense::click_and_drag());
            corners.push(selector_rect(r));
            // The rulers' strips (empty here): a press there is a ruler's once the pane is
            // active, so it does not activate the pane.
            corners.extend([Rect::from_min_max(r.min, pos2(r.max.x, inner.min.y)), Rect::from_min_max(pos2(r.min.x, inner.min.y), pos2(inner.min.x, r.max.y))]);
            if let Some(chosen) = pane_selector(app, ui, pos, r, v) {
                picked = Some((pos, chosen));
            }
        }
        shown.push((pos, r));
    }
    multiview::publish(&ctx, cid, active, shown, corners);
    if let Some((pos, v)) = picked
        && let Err(e) = app.session.execute("view.setPaneView", json!({"position": pos, "view": v.id(), "comp": cid.0}))
    {
        app.ui.status = e.to_string();
    }
    main
}

/// The last picture the side pane at layout position `pos` drew of comp `cid` (None: none yet).
pub(super) fn side_picture(ctx: &egui::Context, cid: effectcraft_engine::project::ItemId, pos: usize) -> Option<egui::TextureId> {
    let cached: Option<(u64, u64, egui::TextureHandle)> = ctx.data(|d| d.get_temp(egui::Id::unique(("viewer-aux", pos))));
    cached.filter(|(_, comp, _)| *comp == cid.0).map(|(_, _, tex)| tex.id())
}

/// The popup id of the view selector of the pane at layout position `pos` (its automation ids
/// number the panes from 1).
fn selector_id(pos: usize) -> egui::Id {
    egui::Id::unique(("viewer-pane-view", pos + 1))
}

/// Close the view selector's popup of the pane at layout position `pos`, if it is open.
fn close_selector(ctx: &egui::Context, pos: usize) {
    let open = selector_id(pos).with("open");
    if ctx.data(|d| d.get_temp::<bool>(open)) == Some(true) {
        ctx.data_mut(|d| d.insert_temp(open, false));
    }
}

/// Where a pane's view selector sits: its top-left corner.
fn selector_rect(pane: Rect) -> Rect {
    Rect::from_min_size(pane.min + vec2(6.0, 6.0), vec2(124.0, 20.0))
}

/// The view selector in the top-left corner of the inactive pane at layout position `pos`, a
/// popup of every 3D view like the toolbar's 3D View menu. Its automation ids number the panes
/// from 1 (`viewer.view.{pos + 1}.type`). Returns the view chosen this frame.
fn pane_selector(app: &mut EffectcraftApp, ui: &mut egui::Ui, pos: usize, area: Rect, current: View3D) -> Option<View3D> {
    let t = app.tokens;
    let r = selector_rect(area);
    let pane = pos + 1;
    let pid = selector_id(pos);
    let response = widgets::dropdown(ui, r, super::view3d_label(current), &t, pid);
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
            let item = ui.selectable_label(*v == current, super::view3d_label(*v));
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
    let label =
        crate::i18n::tr_args("Locked: {} — {}", &[&name, &(if comp.has_3d() { super::view3d_label(lv.view) } else { crate::i18n::tr("Active Camera") })]);
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
