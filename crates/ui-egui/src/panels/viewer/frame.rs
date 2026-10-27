use super::*;

/// Draw the no-composition viewer with New Composition and New Composition From Footage buttons, accepting Project
/// items dropped on it to create the first composition.
pub(super) fn empty_state(app: &mut EffectcraftApp, ui: &mut egui::Ui, rect: Rect) {
    let t = app.tokens;
    let p = ui.painter().clone();
    p.rect_filled(rect, 0.0, t.pasteboard);
    let c = rect.center();
    p.text(c - vec2(0.0, 40.0), Align2::CENTER_CENTER, crate::i18n::tr("Composition"), Tokens::semibold(16.0), t.text);
    let b1 = Rect::from_center_size(c + vec2(0.0, 0.0), vec2(220.0, 30.0));
    if widgets::text_button(ui, b1, "New Composition", true, &t, egui::Id::unique("empty-newcomp")).clicked() {
        crate::panels::dialogs::open_new_comp(app);
    }
    app.auto.add("viewer.empty.newComp", b1, "New Composition");
    let b2 = Rect::from_center_size(c + vec2(0.0, 40.0), vec2(220.0, 30.0));
    if widgets::text_button(ui, b2, "New Composition From Footage", false, &t, egui::Id::unique("empty-fromfootage")).clicked() {
        let _ = crate::menus::invoke(app, &ui.ctx().clone(), "file.import", json!({}));
    }
    app.auto.add("viewer.empty.import", b2, "New Composition From Footage");
    // A Project item dropped on the empty viewer creates the first composition,
    // just like dropping it onto an empty Timeline (#684). Multiple selected
    // items use the existing New Comp from Selection dialog.
    let ctx = ui.ctx().clone();
    if let Some(crate::panels::DragPayload::Item(dragged)) = egui::DragAndDrop::payload::<crate::panels::DragPayload>(&ctx).as_deref()
        && ui.rect_contains_pointer(rect)
    {
        let items: Vec<effectcraft_engine::project::ItemId> = crate::panels::project_drop_items(app, *dragged)
            .into_iter()
            .map(effectcraft_engine::project::ItemId)
            .filter(|id| {
                app.session.project.item(*id).is_some_and(|item| match &item.kind {
                    effectcraft_engine::project::ItemKind::Footage(f) => f.kind != effectcraft_engine::project::FootageKind::Data,
                    effectcraft_engine::project::ItemKind::Solid(_) | effectcraft_engine::project::ItemKind::Comp(_) => true,
                    _ => false,
                })
            })
            .collect();
        if !items.is_empty() {
            p.rect_stroke(rect.shrink(2.0), 0.0, Stroke::new(2.0, t.accent), StrokeKind::Inside);
            app.auto.add("viewer.newCompDrop", rect, "Drop footage to create a composition");
            if ctx.input(|i| i.pointer.any_released()) {
                app.session.state.project_selection = items;
                if let Err(e) = crate::menus::invoke(app, &ctx, "file.newCompFromSelection", json!({})) {
                    app.ui.status = e;
                }
                egui::DragAndDrop::clear_payload(&ctx);
            }
        }
    }
}

/// `#rrggbb` → sRGB bytes.
/// A grid or guide line in the Grids & Guides style: `lines`, `dashed` or `dots`.
pub(crate) fn styled_line(painter: &egui::Painter, pts: [Pos2; 2], stroke: Stroke, style: &str) {
    match style {
        "dashed" => {
            painter.add(egui::Shape::dashed_line(&pts, stroke, 6.0, 4.0));
        }
        "dots" => {
            painter.add(egui::Shape::dotted_line(&pts, stroke.color, 4.0, stroke.width.max(1.0) * 0.75));
        }
        _ => {
            painter.line_segment(pts, stroke);
        }
    }
}

/// `p` (points) moved to the nearest physical pixel corner at `ppp` pixels per point.
pub(crate) fn snap_to_pixels(p: Pos2, ppp: f32) -> Pos2 {
    if !(ppp.is_finite() && ppp > 0.0 && p.x.is_finite() && p.y.is_finite()) {
        return p;
    }
    pos2((p.x * ppp).round() / ppp, (p.y * ppp).round() / ppp)
}

/// Texture filtering for Viewer Zoom Quality: More Accurate = bilinear, Faster = nearest. As in
/// After Effects, a magnified frame (above 100 %, or a lower Resolution) always shows its pixels
/// as blocks, never smoothed, in either mode (#882).
pub(crate) fn zoom_texture_options(smooth: bool) -> egui::TextureOptions {
    if smooth { egui::TextureOptions { magnification: egui::TextureFilter::Nearest, ..egui::TextureOptions::LINEAR } } else { egui::TextureOptions::NEAREST }
}

/// The sampler GPU frames (mipmapped display textures) are drawn with: More Accurate filters
/// bilinearly within and between mip levels, so a frame shown below 100 % is averaged instead of
/// skipping pixels (#417); Faster takes the nearest pixel of the full-size frame. Magnified, both
/// show the nearest pixel (#882).
pub(crate) fn zoom_sampler(smooth: bool) -> eframe::wgpu::SamplerDescriptor<'static> {
    use eframe::wgpu::{FilterMode, MipmapFilterMode, SamplerDescriptor};
    let label = Some("viewer frame");
    if smooth {
        SamplerDescriptor {
            label,
            mag_filter: FilterMode::Nearest,
            min_filter: FilterMode::Linear,
            mipmap_filter: MipmapFilterMode::Linear,
            ..Default::default()
        }
    } else {
        SamplerDescriptor { label, lod_max_clamp: 0.0, ..Default::default() }
    }
}

/// The whole factor a CPU frame is averaged down by for its texture when `pixels` of it are drawn
/// over `screen` screen pixels: about one texel per screen pixel, so bilinear filtering never
/// skips frame pixels — the CPU frames' counterpart of the GPU frames' mip levels (#417). Faster
/// keeps every pixel.
pub(crate) fn minify_factor(smooth: bool, pixels: f64, screen: f64) -> usize {
    let k = (pixels / screen + 1e-3).floor();
    if smooth && k >= 1.0 { k.min(256.0) as usize } else { 1 }
}

/// The Extended Viewer's render region for this frame (see [`ViewerState::extended`]):
/// Settings ▸ 3D ▸ Extended Viewer on, the comp has 3D layers and the view is a custom 3D view
/// (or Draft 3D is on), and part of the visible viewer lies outside the comp frame.
///
/// [`ViewerState::extended`]: crate::state::ViewerState::extended
pub(crate) fn extended_region(
    app: &EffectcraftApp,
    comp: &effectcraft_engine::project::Comp,
    cid: effectcraft_engine::project::ItemId,
    map: ViewerMap,
) -> Option<[f64; 4]> {
    if !app.session.prefs.three_d.extended_viewer || !comp.has_3d() || app.session.state.region_of_interest.is_some() {
        return None;
    }
    let view = app.session.state.views3d.get(&cid).map(|v| v.current).unwrap_or_default();
    if view == View3D::ActiveCamera && !comp.draft_3d {
        return None;
    }
    let a = map.to_comp(map.area.min);
    let b = map.to_comp(map.area.max);
    effectcraft_engine::render::extended_region(comp.width, comp.height, [a[0], a[1], b[0], b[1]], 1.0)
}

pub(super) fn rect_of(map: &ViewerMap, r: Option<[f64; 4]>) -> Rect {
    let [x, y, w, h] = r.unwrap_or_default();
    Rect::from_min_max(map.to_screen([x, y]), map.to_screen([x + w, y + h]))
}

/// Put a rendered frame on screen: CPU pixels go into an egui texture, averaged down by `minify`
/// (see [`minify_factor`]); GPU frames are drawn straight from their wgpu texture (registered with
/// egui-wgpu, no readback).
pub(super) fn show_frame(app: &mut EffectcraftApp, ctx: &egui::Context, key: crate::frames::FrameKey, img: crate::frames::FrameImage, minify: usize) {
    use crate::frames::FrameImage;
    // Settings ▸ Previews ▸ Viewer Zoom Quality.
    let smooth = app.session.prefs.viewer_zoom_smooth();
    let opts = zoom_texture_options(smooth);
    match img {
        FrameImage::Cpu(img) => {
            // Frames wider or taller than the GPU's texture limit go up averaged down too (#201).
            let max = crate::frames::max_texture_side(ctx);
            let fitted = crate::frames::fit_shared_texture_by(&img, max, minify);
            match &mut app.viewer_tex {
                Some((tex, k, m)) if tex.size() == fitted.size() => {
                    tex.set(fitted, opts);
                    (*k, *m) = (key, minify);
                }
                _ => app.viewer_tex = Some((ctx.load_texture("viewer-frame", fitted, opts), key, minify)),
            }
            app.viewer_shown = app.viewer_tex.as_ref().map(|(t, k, _)| (t.id(), *k));
            app.viewer_image = Some(img);
            vt::set_texture_roi(ctx, app.session.state.region_of_interest.or(app.ui.viewer.extended));
        }
        FrameImage::Gpu(f) => {
            let Some(rs) = &app.wgpu else { return };
            let view = f.texture.create_view(&Default::default());
            let mut ren = rs.renderer.write();
            let id = match &app.viewer_native {
                Some((id, _, _)) => {
                    ren.update_egui_texture_from_wgpu_texture_with_sampler_options(&rs.device, &view, zoom_sampler(smooth), *id);
                    *id
                }
                None => ren.register_native_texture_with_sampler_options(&rs.device, &view, zoom_sampler(smooth)),
            };
            drop(ren);
            app.viewer_native = Some((id, key, f));
            app.viewer_shown = Some((id, key));
            // Pixels are read back on demand (viewer_pixels).
            app.viewer_image = None;
        }
    }
}
