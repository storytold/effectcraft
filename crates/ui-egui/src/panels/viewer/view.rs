use super::tools::draw_rigs;
use super::{Align2, Color32, EffectcraftApp, EvalCtx, Rect, Sense, Stroke, StrokeKind, Tokens, View3D, json, multiview, pos2, three_d, vec2, vt, widgets};
use crate::frames::{FrameImage, FrameKey};

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
    // A position with no side pane in this pass (the layout hides it, or it is the active one)
    // waits for no frame: it would never draw the frame it asked for, so never release it.
    let side: Vec<usize> = panes.iter().map(|(pos, _)| *pos).filter(|pos| *pos != active).collect();
    app.frames.retain_panes(&side);
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
        let scale = pane_scale(map.zoom * ppp);
        // The Active Camera pane renders through the comp's own camera; the others through their
        // view camera.
        let through_active = v == View3D::ActiveCamera;
        let cam = if through_active { three_d::active_camera(&ectx) } else { views.cam(v, comp.width as f64, comp.height as f64).state() };
        // Rendered in the background, as a passive viewer's frame is: the pane draws its last
        // picture until the frame it asked for lands.
        let id = egui::Id::unique(("viewer-aux", pos));
        let mut pic: PanePicture = ctx.data(|d| d.get_temp(id)).unwrap_or_default();
        let drawn = pic.drawn.as_ref().map(|(k, _)| *k).filter(|k| k.comp == cid.0);
        let (key, got) = app.pane_frame(cid, comp.frame_rate.frame_at(t), scale, (comp.has_3d() && !through_active).then_some(cam), pos, drawn);
        let mut changed = false;
        if let Some((k, img)) = got
            && pane_step(k, drawn, pic.failed) == PaneStep::Convert
        {
            match pane_texture(app, &ctx, pos, img, pic.drawn.as_ref().map(|(_, tex)| tex)) {
                Some(tex) => {
                    // The frame it asked for: the pane holds it now, the cache need not keep it.
                    if k == key {
                        app.frames.release_pane(pos);
                    }
                    pic.drawn = Some((k, tex));
                }
                // Not tried again every UI frame: the next try is with another frame.
                None => pic.failed = Some(k),
            }
            changed = true;
        }
        // The frame drawn, asked for again at a later revision (an undo, an edit elsewhere): that
        // revision is the one it is now, the lower bound of the frames that may replace it.
        if let Some((k, _)) = pic.drawn.as_mut()
            && *k == key
            && k.revision < key.revision
        {
            k.revision = key.revision;
            changed = true;
        }
        if changed {
            ctx.data_mut(|d| d.insert_temp(id, pic.clone()));
        }
        // Never another comp's frame (after a comp switch), as the active pane does, nor another
        // view's (the pane was switched from Top to Left and Left's frame is still rendering): a
        // press would act through Left's projection on Top's picture. The earlier revision of
        // the same view stays (a drag), as `get_for_viewer` keeps it.
        let drawn = pic.drawn.as_ref().filter(|(k, _)| k.comp == cid.0 && k.view == key.view);
        let pp = p.with_clip_rect(inner);
        let b = comp.background;
        pp.rect_filled(cr, 0.0, Color32::from_rgb((b[0] * 255.0) as u8, (b[1] * 255.0) as u8, (b[2] * 255.0) as u8));
        if let Some((_, tex)) = drawn {
            pp.image(tex.id(), cr, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
        }
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

/// A pane of comp `cid` became active: the pane that was active, a side pane now, starts with
/// the frame it showed (its own view's) as its picture until its draft lands, rather than with an
/// empty frame. The viewer keeps no texture of that frame: the newly active pane's frame comes in
/// a texture of its own, its side-pane picture standing in meanwhile (`multiview::stand_in`). A
/// half-float frame (the display pass draws it) is not handed over, nor a frame of a region (the
/// region of interest, the Extended Viewer's area), also one turned off while its frame is still
/// on screen: a side pane draws the whole comp. Only a frame the viewer showed as the whole comp's
/// is handed over.
pub(super) fn hand_over(app: &mut EffectcraftApp, ctx: &egui::Context, cid: effectcraft_engine::project::ItemId) {
    let Some((comp, old)) = app.ui.viewer.shown_for else { return };
    let shown = multiview::shown_positions(app, cid, app.session.state.view_layout.max(1));
    if comp != cid.0 || old == multiview::active_pos(app, cid) || !shown.contains(&old) {
        return;
    }
    let Some((on_screen, key)) = app.viewer_shown else { return };
    if app.ui.viewer.shown_region != Some((key, None)) {
        return;
    }
    let tex = match (&app.viewer_native, &app.viewer_tex) {
        (Some((id, k, _)), _) if on_screen == Some(*id) && *k == key => app.viewer_native.take().map(|(id, _, f)| PaneTex::Gpu { id, _frame: f }),
        (_, Some((h, k, _))) if on_screen == Some(h.id()) && *k == key => app.viewer_tex.take().map(|(h, _, _)| PaneTex::Cpu(h)),
        _ => None,
    };
    let Some(tex) = tex else { return };
    app.viewer_shown = None;
    app.viewer_image = None;
    let id = egui::Id::unique(("viewer-aux", old));
    let mut pic: PanePicture = ctx.data(|d| d.get_temp(id)).unwrap_or_default();
    if let Some((_, prev)) = pic.drawn.replace((key, tex)) {
        free_native(app, Some(&prev));
    }
    pic.failed = None;
    ctx.data_mut(|d| d.insert_temp(id, pic));
}

/// The cache key of the frame the side pane at layout position `pos` draws of comp `cid` (for
/// tests).
#[doc(hidden)]
pub fn shown_frame_key(ctx: &egui::Context, cid: effectcraft_engine::project::ItemId, pos: usize) -> Option<FrameKey> {
    let pic: PanePicture = ctx.data(|d| d.get_temp(egui::Id::unique(("viewer-aux", pos))))?;
    pic.drawn.map(|(k, _)| k).filter(|k| k.comp == cid.0)
}

/// The last picture the side pane at layout position `pos` drew of comp `cid` through the view
/// whose frame keys carry `view` (None: none yet, or of another view).
pub(super) fn side_picture(ctx: &egui::Context, cid: effectcraft_engine::project::ItemId, pos: usize, view: u64) -> Option<egui::TextureId> {
    let pic: PanePicture = ctx.data(|d| d.get_temp(egui::Id::unique(("viewer-aux", pos))))?;
    pic.drawn.filter(|(k, _)| k.comp == cid.0 && k.view == view).map(|(_, tex)| tex.id())
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

/// The render scale of a side pane drawn at `pixels` screen pixels per comp pixel: in eighths, so
/// resizing reuses frames, in (0, 1] whatever the magnification, as a passive viewer's.
fn pane_scale(pixels: f32) -> f64 {
    let eighths = (f64::from(pixels) * 8.0).ceil();
    // (a hostile magnification, NaN too, renders the smallest frame)
    if eighths.is_nan() { 1.0 / 8.0 } else { eighths.clamp(1.0, 8.0) / 8.0 }
}

/// What a side pane drew last: the frame's key and its texture, and the key of a frame it could
/// not show.
#[derive(Clone, Default)]
struct PanePicture {
    drawn: Option<(FrameKey, PaneTex)>,
    failed: Option<FrameKey>,
}

/// A side pane's texture: pixels uploaded to egui, or a GPU frame registered as a native
/// egui-wgpu texture (no readback), the pane's registration updated in place.
#[derive(Clone)]
enum PaneTex {
    Cpu(egui::TextureHandle),
    /// The frame is held so its texture outlives the registration.
    Gpu {
        id: egui::TextureId,
        _frame: std::sync::Arc<effectcraft_gpu::DisplayFrame>,
    },
}

impl PaneTex {
    fn id(&self) -> egui::TextureId {
        match self {
            PaneTex::Cpu(h) => h.id(),
            PaneTex::Gpu { id, .. } => *id,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum PaneStep {
    /// Draw what the pane has.
    Keep,
    /// Make a texture of the new frame.
    Convert,
}

/// Whether a pane makes a texture of the frame `wanted`, which landed: not when it already drew
/// it (`drawn`) or could not show it before (`failed`: each frame is tried once).
fn pane_step(wanted: FrameKey, drawn: Option<FrameKey>, failed: Option<FrameKey>) -> PaneStep {
    if drawn == Some(wanted) || failed == Some(wanted) { PaneStep::Keep } else { PaneStep::Convert }
}

/// The texture of side pane `pos`'s frame `img`, after its last one, `old`: CPU frames are
/// uploaded (into the old texture when it has their size), GPU frames become native egui-wgpu
/// textures and are never read back. `None`: the frame cannot be shown (a GPU frame without
/// egui-wgpu's device).
fn pane_texture(app: &EffectcraftApp, ctx: &egui::Context, pos: usize, img: FrameImage, old: Option<&PaneTex>) -> Option<PaneTex> {
    let smooth = app.session.prefs.viewer_zoom_smooth();
    match crate::panels::viewers::cpu_passive_image(app, img, None) {
        FrameImage::Cpu(c) => {
            let max = crate::frames::max_texture_side(ctx);
            let opts = super::zoom_texture_options(smooth);
            let tex = match old {
                Some(PaneTex::Cpu(h)) if h.size() == crate::frames::fitted_size(c.size, max) => {
                    let mut h = h.clone();
                    h.set(crate::frames::fit_texture((*c).clone(), max), opts);
                    h
                }
                _ => {
                    // After a GPU frame: its native texture goes back to egui-wgpu.
                    free_native(app, old);
                    crate::frames::load_fitted(ctx, format!("viewer-aux-{pos}"), (*c).clone(), opts)
                }
            };
            Some(PaneTex::Cpu(tex))
        }
        FrameImage::Gpu(f) => {
            let rs = app.wgpu.as_ref()?;
            let view = f.texture.create_view(&Default::default());
            let sampler = || super::zoom_sampler(smooth);
            let mut ren = rs.renderer.write();
            let id = match old {
                // (a registration the renderer no longer has is made anew, not updated)
                Some(PaneTex::Gpu { id, .. }) if ren.texture(id).is_some() => {
                    ren.update_egui_texture_from_wgpu_texture_with_sampler_options(&rs.device, &view, sampler(), *id);
                    *id
                }
                _ => ren.register_native_texture_with_sampler_options(&rs.device, &view, sampler()),
            };
            Some(PaneTex::Gpu { id, _frame: f })
        }
        // (`cpu_passive_image` makes 8-bit pixels of half-float frames)
        FrameImage::CpuF16(_) => None,
    }
}

/// Give the native egui-wgpu texture of a side pane's GPU frame back to the renderer.
fn free_native(app: &EffectcraftApp, tex: Option<&PaneTex>) {
    if let (Some(PaneTex::Gpu { id, .. }), Some(rs)) = (tex, &app.wgpu) {
        rs.renderer.write().free_texture(id);
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

#[cfg(test)]
mod pane_tests {
    use super::*;

    fn key(frame: i64) -> FrameKey {
        FrameKey { revision: 1, content: 2, comp: 3, frame, scale: 1000, view: 5, opts: 6 }
    }

    /// A side pane renders in eighths of the comp's size (so resizing reuses frames), never past
    /// 100 % and never at zero, a negative or no scale (an agent's `ui.set {viewer: {zoom: -1}}`
    /// stored for a side pane rendered at -2).
    #[test]
    fn a_pane_renders_in_eighths_between_an_eighth_and_full_size() {
        assert_eq!([0.337, 0.79, 1.0, 5.0].map(pane_scale), [0.375, 0.875, 1.0, 1.0]);
        assert_eq!([0.0, -1.0, f32::NAN, f32::NEG_INFINITY].map(pane_scale), [0.125; 4]);
    }

    #[test]
    fn a_new_frame_is_converted_once() {
        assert_eq!(pane_step(key(1), None, None), PaneStep::Convert);
        assert_eq!(pane_step(key(1), Some(key(0)), None), PaneStep::Convert);
        assert_eq!(pane_step(key(1), Some(key(1)), None), PaneStep::Keep);
    }

    #[test]
    fn a_frame_that_failed_is_not_tried_every_ui_frame() {
        assert_eq!(pane_step(key(1), Some(key(0)), Some(key(1))), PaneStep::Keep);
        // A different frame is tried again.
        assert_eq!(pane_step(key(2), Some(key(0)), Some(key(1))), PaneStep::Convert);
    }

    /// A side pane draws a GPU frame from its own texture, registered with egui-wgpu once and
    /// updated in place. When its frames move to the CPU, the registration goes back to
    /// egui-wgpu; it was kept until the window closed, one more each time. (The web build's frame
    /// cache holds GPU frames, and CPU ones while the viewer shows a channel, an exposure, a
    /// region of interest or the Extended Viewer, or for a frame the GPU cannot render.) Skips
    /// without a GPU compositor.
    #[test]
    fn a_pane_gives_its_gpu_texture_back_when_its_frames_move_to_the_cpu() {
        use eframe::wgpu;
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        if pollster::block_on(instance.enumerate_adapters(wgpu::Backends::all())).is_empty() {
            eprintln!("no wgpu adapter: skipping");
            return;
        }
        let mut h = egui_kittest::Harness::builder()
            .with_size(egui::vec2(800.0, 600.0))
            .wgpu()
            .build_eframe(|_| crate::EffectcraftApp::new(effectcraft_engine::Session::default()));
        // GPU frames come once the compositor (built on a worker thread) is attached to the
        // window's device.
        h.step();
        for _ in 0..3000 {
            if !h.state().gpu_initializing() {
                break;
            }
            h.step();
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let Some(rs) = h.state().wgpu.clone() else {
            eprintln!("no GPU compositor on the window's device: skipping");
            return;
        };
        let gpu_frame = || {
            let texture = rs.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("side pane test frame"),
                size: wgpu::Extent3d { width: 4, height: 4, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            });
            FrameImage::Gpu(std::sync::Arc::new(effectcraft_gpu::DisplayFrame { texture, width: 4, height: 4 }))
        };
        let registered = |id: egui::TextureId| rs.renderer.read().texture(&id).is_some();
        let ctx = h.ctx.clone();
        let app = h.state();
        let first = pane_texture(app, &ctx, 0, gpu_frame(), None).unwrap_or_else(|| panic!("a GPU frame is shown"));
        let PaneTex::Gpu { id, .. } = &first else { panic!("as a native texture") };
        let id = *id;
        assert!(registered(id));
        let next = pane_texture(app, &ctx, 0, gpu_frame(), Some(&first)).unwrap_or_else(|| panic!("the next GPU frame is shown"));
        assert_eq!(next.id(), id, "the next GPU frame updates the pane's registration");
        let cpu = FrameImage::Cpu(std::sync::Arc::new(egui::ColorImage::new([4, 4], vec![Color32::RED; 16])));
        let shown = pane_texture(app, &ctx, 0, cpu, Some(&next)).unwrap_or_else(|| panic!("a CPU frame is shown"));
        assert!(matches!(shown, PaneTex::Cpu(_)));
        assert!(!registered(id), "the GPU frame's registration went back to egui-wgpu");
    }
}
