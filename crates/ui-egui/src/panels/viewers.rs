//! Several Composition viewers (View ▸ New Viewer), as in After Effects. Viewer 0 is the
//! Composition panel, the others are `PanelKind::Viewer(n)` panels. One viewer is *active*: it
//! is the full interactive viewer and shows the active comp. The others show their own comp's
//! frame at that comp's current time; a click (or their tab) makes them active and their comp
//! the active one. New Viewer locks the viewer in use, so it keeps its comp. Opening a comp while
//! the active viewer is locked shows it in an unlocked viewer, or in a new one if every viewer is
//! locked.

use crate::i18n::tr;
use std::sync::Arc;

use effectcraft_engine::project::ItemId;
use egui::{Align2, Color32, Rect, Sense, pos2, vec2};
use serde_json::json;

use crate::EffectcraftApp;
use crate::dock::PanelKind;
use crate::frames::{FrameImage, FrameKey};
use crate::theme::Tokens;

/// A passive viewer's texture: the frame it shows.
pub enum PassiveTexture {
    Cpu(egui::TextureHandle),
    Gpu(egui::TextureId, Arc<effectcraft_gpu::DisplayFrame>),
}

impl PassiveTexture {
    fn id(&self) -> egui::TextureId {
        match self {
            PassiveTexture::Cpu(h) => h.id(),
            PassiveTexture::Gpu(id, _) => *id,
        }
    }
}

/// The dock panel of viewer `id` (0 = the Composition panel).
pub fn panel(id: u32) -> PanelKind {
    if id == 0 { PanelKind::Composition } else { PanelKind::Viewer(id) }
}

/// The viewer a panel is, if it is one.
pub fn id_of(p: PanelKind) -> Option<u32> {
    match p {
        PanelKind::Composition => Some(0),
        PanelKind::Viewer(n) => Some(n),
        _ => None,
    }
}

/// The active viewer's panel.
pub fn active_panel(app: &EffectcraftApp) -> PanelKind {
    panel(app.ui.active_viewer)
}

pub fn locked(app: &EffectcraftApp, id: u32) -> bool {
    app.ui.locked_tabs.contains(&panel(id).id())
}

/// Viewer `id`'s panel is docked or floating.
fn open(app: &EffectcraftApp, id: u32) -> bool {
    let p = panel(id);
    app.ui.dock.contains(p) || app.ui.floating.iter().any(|f| f.panels.contains(&p))
}

/// The comp viewer `id` shows: the active comp in the active viewer, else its own.
pub fn comp_of(app: &EffectcraftApp, id: u32) -> Option<ItemId> {
    if id == app.ui.active_viewer {
        return app.session.active_comp_id();
    }
    app.ui.viewers.get(&id).copied().flatten().map(ItemId).filter(|c| app.session.project.comp(*c).is_some())
}

/// Draw viewer `id`: the active one is the full viewer; another shows its comp's frame and
/// becomes the active viewer when clicked.
pub fn show(app: &mut EffectcraftApp, ui: &mut egui::Ui, id: u32, rect: Rect) {
    if id == app.ui.active_viewer {
        // A comp opened earlier this frame (the Project panel) is routed next frame by
        // `on_open_comp`, which needs the comp this viewer showed (to keep it when locked).
        if app.session.events.iter().any(|e| matches!(e, effectcraft_engine::Event::OpenComp(_))) {
            ui.ctx().request_repaint();
        } else {
            let c = app.session.active_comp_id().map(|c| c.0);
            app.ui.viewers.insert(id, c);
        }
        super::viewer::show(app, ui, rect);
        return;
    }
    let t = app.tokens;
    let ctx = ui.ctx().clone();
    let p = ui.painter().with_clip_rect(rect);
    p.rect_filled(rect, 0.0, t.pasteboard);
    let resp = ui.interact(rect, egui::Id::new(("passive-viewer", id)), Sense::click()).on_hover_text(tr("Click to make this viewer active"));
    app.auto.add(&format!("viewers.{id}"), rect, "Composition viewer (click to make it active)");
    match comp_of(app, id).and_then(|cid| Some((cid, app.session.project.comp(cid)?.clone()))) {
        Some((cid, c)) => {
            let (cw, ch) = ((c.width.max(1) as f64 * c.pixel_aspect.max(0.01)) as f32, c.height.max(1) as f32);
            let fit = ((rect.width() - 24.0) / cw).min((rect.height() - 24.0) / ch).max(0.01);
            let r = Rect::from_center_size(rect.center(), vec2(cw * fit, ch * fit));
            // Render at the size it is drawn (eighths, so resizing reuses frames), at most 100%.
            let scale = ((fit * ctx.pixels_per_point()) as f64 * 8.0).ceil().clamp(1.0, 8.0) / 8.0;
            let frame = c.frame_rate.frame_at(app.session.time_of(cid));
            app.request_frame(cid, frame, scale);
            let key = app.frame_key(cid, frame, scale);
            if let Some((key, img)) = app.frames.get_for_viewer(&key, app.passive_tex.get(&id).map(|(k, _)| k)) {
                let dc = effectcraft_engine::viewer::DisplayColor::of(&app.session);
                let dkey = display_hash(&super::viewer_tools::display_key(app, dc.is_some()));
                set_texture(app, &ctx, id, key, img, dc, dkey);
            }
            match app.passive_tex.get(&id).filter(|(k, _)| k.comp == cid.0) {
                Some((_, tex)) => {
                    p.image(tex.id(), r, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
                }
                None => {
                    let bg = c.background;
                    p.rect_filled(r, 0.0, Color32::from_rgb((bg[0] * 255.0) as u8, (bg[1] * 255.0) as u8, (bg[2] * 255.0) as u8));
                    ctx.request_repaint_after(std::time::Duration::from_millis(50));
                }
            }
            p.rect_stroke(r, 0.0, egui::Stroke::new(1.0, t.separator), egui::StrokeKind::Outside);
        }
        None => {
            p.text(rect.center(), Align2::CENTER_CENTER, tr("Click to show the active composition here"), Tokens::ui(12.0), t.text_faint);
        }
    }
    if resp.clicked() {
        activate(app, &ctx, id);
    }
}

/// Keep the frame viewer `id` shows in its texture.
fn set_texture(
    app: &mut EffectcraftApp,
    ctx: &egui::Context,
    id: u32,
    key: FrameKey,
    img: FrameImage,
    dc: Option<effectcraft_engine::viewer::DisplayColor>,
    dkey: u64,
) {
    let dc_id = egui::Id::new(("passive-viewer-display", id));
    let same_display = ctx.data(|d| d.get_temp::<u64>(dc_id)) == Some(dkey);
    if let Some((k, _)) = app.passive_tex.get_mut(&id)
        && *k == key
        && same_display
    {
        // An undo or an edit elsewhere can reuse this content at a newer revision. Keep that
        // revision as the lower bound for subsequent intermediate edit frames.
        k.revision = key.revision;
        return;
    }
    ctx.data_mut(|d| d.insert_temp(dc_id, dkey));
    let opts = super::viewer::zoom_texture_options(app.session.prefs.viewer_zoom_smooth());
    // The monitor conversion the active viewer applies (Use Display Color Management).
    let img = match (img, &dc) {
        (FrameImage::Cpu(img), Some(dc)) => {
            let mut px: Vec<[u8; 4]> = img.pixels.iter().map(|c| c.to_array()).collect();
            dc.apply(&mut px);
            let pixels = px.into_iter().map(|a| Color32::from_rgba_premultiplied(a[0], a[1], a[2], a[3])).collect();
            FrameImage::Cpu(Arc::new(egui::ColorImage::new(img.size, pixels)))
        }
        (img, _) => img,
    };
    let tex = match (img, app.passive_tex.remove(&id)) {
        (FrameImage::Cpu(img), Some((_, PassiveTexture::Cpu(mut h))))
            if h.size() == crate::frames::fitted_size(img.size, crate::frames::max_texture_side(ctx)) =>
        {
            h.set(crate::frames::fit_texture((*img).clone(), crate::frames::max_texture_side(ctx)), opts);
            PassiveTexture::Cpu(h)
        }
        (FrameImage::Cpu(img), old) => {
            free(app, old.map(|(_, t)| t));
            PassiveTexture::Cpu(crate::frames::load_fitted(ctx, format!("viewer-{id}"), (*img).clone(), opts))
        }
        (FrameImage::Gpu(f), old) => {
            let Some(rs) = &app.wgpu else { return };
            let view = f.texture.create_view(&Default::default());
            let sampler = || super::viewer::zoom_sampler(app.session.prefs.viewer_zoom_smooth());
            let mut ren = rs.renderer.write();
            let tid = match old {
                Some((_, PassiveTexture::Gpu(tid, _))) => {
                    ren.update_egui_texture_from_wgpu_texture_with_sampler_options(&rs.device, &view, sampler(), tid);
                    tid
                }
                _ => ren.register_native_texture_with_sampler_options(&rs.device, &view, sampler()),
            };
            PassiveTexture::Gpu(tid, f)
        }
    };
    app.passive_tex.insert(id, (key, tex));
}

/// A hash of the display conversion's inputs (see `viewer_tools::display_key`).
fn display_hash(key: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    key.hash(&mut h);
    h.finish()
}

/// Give a passive viewer's texture back (GPU textures are registered with the renderer).
fn free(app: &EffectcraftApp, tex: Option<PassiveTexture>) {
    if let (Some(PassiveTexture::Gpu(id, _)), Some(rs)) = (tex, &app.wgpu) {
        rs.renderer.write().free_texture(&id);
    }
}

/// Make viewer `id` the active one: its comp becomes the active comp.
pub fn activate(app: &mut EffectcraftApp, ctx: &egui::Context, id: u32) {
    if id == app.ui.active_viewer || !open(app, id) {
        return;
    }
    let active = app.session.active_comp_id();
    let target = comp_of(app, id);
    app.ui.viewers.insert(app.ui.active_viewer, active.map(|c| c.0));
    app.ui.active_viewer = id;
    match target {
        Some(c) if Some(c) != active => {
            if let Err(e) = crate::menus::invoke(app, ctx, "comp.open", json!({"comp": c.0})) {
                app.ui.status = e;
            }
        }
        _ => {
            app.ui.viewers.insert(id, active.map(|c| c.0));
        }
    }
}

/// A comp was opened (comp.open, a Timeline tab, a precomp double-click): it shows in the active
/// viewer, unless that one is locked to another comp: then in an unlocked viewer, or a new one.
pub fn on_open_comp(app: &mut EffectcraftApp, comp: ItemId) {
    let av = app.ui.active_viewer;
    let shown = app.ui.viewers.get(&av).copied().flatten().filter(|s| app.session.project.comp(ItemId(*s)).is_some());
    if locked(app, av) && shown.is_some_and(|s| s != comp.0) {
        let free = app.ui.viewers.keys().copied().find(|v| *v != av && !locked(app, *v) && open(app, *v));
        app.ui.active_viewer = match free {
            Some(v) => v,
            None => add_viewer(app, av),
        };
    }
    app.ui.viewers.insert(app.ui.active_viewer, Some(comp.0));
    // The comp's viewer and its Timeline tab come to the front (unless locked), reopened if they
    // were closed (as in After Effects).
    for p in [active_panel(app), PanelKind::Timeline] {
        let closed = !app.ui.dock.contains(p) && !app.ui.floating.iter().any(|f| f.panels.contains(&p));
        if closed || !app.ui.locked_tabs.contains(&p.id()) {
            app.raise_panel(p);
        }
    }
}

/// View ▸ New Viewer: a new viewer of the active comp next to the one in use, which is locked
/// (so it keeps its comp). Returns the new viewer's id.
pub fn new_viewer(app: &mut EffectcraftApp) -> u32 {
    let prev = app.ui.active_viewer;
    let active = app.session.active_comp_id().map(|c| c.0);
    app.ui.viewers.insert(prev, active);
    app.ui.locked_tabs.insert(panel(prev).id());
    let id = add_viewer(app, prev);
    app.ui.viewers.insert(id, active);
    app.ui.active_viewer = id;
    app.show_panel(panel(id));
    id
}

/// Dock a new viewer panel as a tab next to viewer `near`.
fn add_viewer(app: &mut EffectcraftApp, near: u32) -> u32 {
    let id = app.ui.viewers.keys().copied().max().unwrap_or(0).saturating_add(1).max(1);
    app.ui.viewers.insert(id, None);
    let p = panel(id);
    let anchor = if app.ui.dock.contains(panel(near)) { panel(near) } else { PanelKind::Composition };
    app.ui.dock.open_near(p, anchor);
    id
}

/// A viewer panel was closed: forget it (the Composition panel keeps its slot), and if it was the
/// active viewer another open one takes over.
pub fn on_close(app: &mut EffectcraftApp, p: PanelKind) {
    let Some(id) = id_of(p) else { return };
    let tex = app.passive_tex.remove(&id).map(|(_, t)| t);
    free(app, tex);
    if id != 0 {
        app.ui.viewers.remove(&id);
        app.ui.locked_tabs.remove(&p.id());
    }
    if app.ui.active_viewer == id {
        let next = std::iter::once(0).chain(app.ui.viewers.keys().copied()).find(|v| *v != id && open(app, *v)).unwrap_or(0);
        app.ui.active_viewer = next;
        // The viewer taking over keeps its comp (which becomes the active comp).
        if let Some(c) = app.ui.viewers.get(&next).copied().flatten().map(ItemId).filter(|c| app.session.project.comp(*c).is_some())
            && app.session.active_comp_id() != Some(c)
        {
            app.session.open_comp(c);
        }
    }
}

/// The tab label of viewer `id` ("Composition Intro").
pub fn title(app: &EffectcraftApp, id: u32) -> Option<String> {
    let c = comp_of(app, id)?;
    Some(format!("{} {}", crate::i18n::command_label("window.panel", "Composition"), app.session.project.item(c)?.name))
}
