use crate::EffectcraftApp;
use effectcraft_engine::onion::{self, Ghost};
use effectcraft_engine::project::{Comp, ItemId};
use egui::{Color32, Rect, pos2};
use std::hash::{Hash, Hasher};

const BUILDS_PER_PASS: usize = 2;

fn last_frame(comp: &Comp) -> i64 {
    comp.frame_rate.frame_at(comp.duration).saturating_sub(1).max(0)
}

fn active(app: &EffectcraftApp, cid: ItemId) -> bool {
    let v = &app.session.state.viewer;
    v.onion.enabled
        && !v.show_snapshot
        && v.fast_previews != effectcraft_engine::commands::viewer_cmds::FastPreviews::Wireframe
        && app.viewer_region(cid).is_none()
}

pub(super) fn request(app: &mut EffectcraftApp, cid: ItemId, comp: &Comp, frame: i64, scale: f64) {
    if !active(app, cid) || app.playback.playing {
        return;
    }
    let o = app.session.state.viewer.onion.clone();
    let need = o.missing(frame, 0, last_frame(comp), |f| app.frames.is_cached(&app.frame_key(cid, f, scale)));
    for g in need {
        app.request_frame_background(cid, g.frame, scale);
    }
}

fn texture_of(app: &EffectcraftApp, ctx: &egui::Context, g: &Ghost, key: crate::frames::FrameKey, amount: f32) -> Option<(egui::TextureHandle, u64)> {
    let img = match app.frames.get(&key)? {
        crate::frames::FrameImage::Cpu(i) => i,
        crate::frames::FrameImage::CpuF16(f) => {
            let pass = effectcraft_engine::viewer::DisplayPass::new(effectcraft_engine::viewer::Channel::Rgb, false, 0.0, f.encoding, None);
            std::sync::Arc::new(f.to_color_image(&pass))
        }
        crate::frames::FrameImage::Gpu(_) => return None,
    };
    let bytes: Vec<u8> = img.pixels.iter().flat_map(|c| c.to_array()).collect();
    let ghost = onion::ghost_pixels(&bytes, g.tint, amount, g.alpha);
    let px = ghost.as_chunks::<4>().0.iter().map(|c| Color32::from_rgba_premultiplied(c[0], c[1], c[2], c[3])).collect();
    let image = egui::ColorImage::new(img.size, px);
    let opts = egui::TextureOptions::LINEAR;
    Some((crate::frames::load_fitted(ctx, "onion-ghost", image, opts), 0))
}

pub(super) fn draw(app: &mut EffectcraftApp, ctx: &egui::Context, painter: &egui::Painter, comp_rect: Rect, cid: ItemId, comp: &Comp, under_pass: bool) {
    if !active(app, cid) {
        if !under_pass {
            app.onion_tex.clear();
        }
        return;
    }
    let o = app.session.state.viewer.onion.clone();
    if o.under != under_pass {
        return;
    }
    let Some(scale) = app.viewer_shown.as_ref().filter(|(_, k)| k.comp == cid.0).map(|(_, k)| k.scale as f64 / 1000.0) else { return };
    let frame = comp.frame_rate.frame_at(app.session.time());
    let mut ghosts = o.ghosts(frame, 0, last_frame(comp));
    ghosts.sort_by_key(|g| std::cmp::Reverse((g.frame - frame).unsigned_abs()));
    app.onion_tex.retain(|(f, b), _| ghosts.iter().any(|g| g.frame == *f && g.before == *b));
    let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
    let mut builds = 0;
    for g in &ghosts {
        let key = app.frame_key(cid, g.frame, scale);
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (key, g.tint, g.alpha.to_bits(), o.tint_amount.to_bits()).hash(&mut h);
        let stamp = h.finish();
        let slot = (g.frame, g.before);
        if app.onion_tex.get(&slot).is_none_or(|(_, s)| *s != stamp) {
            if builds >= BUILDS_PER_PASS {
                ctx.request_repaint();
                continue;
            }
            let Some((tex, _)) = texture_of(app, ctx, g, key, o.tint_amount) else { continue };
            builds += 1;
            app.onion_tex.insert(slot, (tex, stamp));
        }
        if let Some((tex, _)) = app.onion_tex.get(&slot) {
            painter.image(tex.id(), comp_rect, uv, Color32::WHITE);
        }
    }
}
