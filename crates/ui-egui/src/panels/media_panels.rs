//! Media Browser, Metadata and Progress panels (Window menu).
//!
//! - **Media Browser**: favourites and the folder tree on the left, the folder's files on the
//!   right with thumbnails (images and video, decoded through the media layer's probe), double-
//!   click to open a folder or import a file, drag files into the Project panel or the timeline.
//!   Desktop only (the web build has no file system to browse).
//! - **Metadata**: what we read from the selected item's file (codec, size, rate, duration,
//!   colour profile, dates) and the project's, with editable item and project comments.
//! - **Progress**: every background job (render queue, track / mask track / Warp Stabilizer /
//!   3D Camera Tracker / Roto Brush analyses, Content-Aware Fill, Scene Edit Detection) with a
//!   progress bar and a cancel button, and the recently finished ones.

use crate::i18n::tr;
use effectcraft_engine::media_browser as mb;
use effectcraft_engine::project::ItemId;
use egui::{Align2, Color32, Rect, Sense, Stroke, pos2, vec2};
use serde_json::{Value, json};
use std::sync::Arc;

use super::DragPayload;
use super::panel_kit as kit;
use crate::theme::Tokens;
use crate::{EffectcraftApp, widgets};

// ---------------------------------------------------------------- Media Browser

type Thumb = Option<(egui::TextureHandle, [u32; 2])>;
type Listing = (f64, Arc<Result<Vec<mb::Entry>, String>>);

/// Thumbnail of a file (cached per path; `None` while not decoded or not decodable).
fn thumbnail(app: &EffectcraftApp, ctx: &egui::Context, path: &str, budget: &mut u32) -> Thumb {
    let id = egui::Id::new(("mb-thumb", path));
    if let Some(t) = ctx.data(|d| d.get_temp::<Thumb>(id)) {
        return t;
    }
    if *budget == 0 {
        return None;
    }
    *budget -= 1;
    let thumb = (|| {
        let imp = app.session.importer.clone()?;
        let f = imp.probe(path).ok()?;
        if !f.has_video {
            return None;
        }
        // A synthetic item id keyed by the path (the frame cache keys on the path).
        let hid = path.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ b as u64).wrapping_mul(0x100_0000_01b3)) | (1 << 62);
        let img = app.session.footage.frame(ItemId(hid), &f, effectcraft_engine::time::Tick::ZERO)?;
        let step = (img.width.max(img.height) as f32 / 128.0).ceil().max(1.0) as u32;
        let (w, h) = (img.width.div_ceil(step), img.height.div_ceil(step));
        let full = img.to_rgba8();
        let mut px = Vec::with_capacity((w * h * 4) as usize);
        for y in 0..h {
            for x in 0..w {
                let i = (((y * step) * img.width + x * step) * 4) as usize;
                px.extend_from_slice(&full[i..i + 4]);
            }
        }
        let tex = ctx.load_texture(format!("mb-{path}"), egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &px), egui::TextureOptions::LINEAR);
        Some((tex, [w, h]))
    })();
    ctx.data_mut(|d| d.insert_temp(id, thumb.clone()));
    thumb
}

pub fn media_browser(app: &mut EffectcraftApp, ui: &mut egui::Ui, rect: Rect) {
    let t = app.tokens;
    let ctx = ui.ctx().clone();
    let p = ui.painter().with_clip_rect(rect);
    p.rect_filled(rect, 0.0, t.panel_bg);
    let Some(b) = app.session.media_browser() else {
        p.text(rect.center(), Align2::CENTER_CENTER, tr("The Media Browser is not available here (use File ▸ Import)"), Tokens::ui(12.0), t.text_faint);
        app.auto.add("mediaBrowser.unavailable", rect, "Media Browser unavailable");
        return;
    };
    let st = app.session.state.media_browser.clone();
    let dir = st.folder.clone().unwrap_or_else(|| b.home());
    // Listing cached per folder for a second.
    let lid = egui::Id::new(("mb-list", dir.clone(), st.importable_only));
    let now = ctx.input(|i| i.time);
    let listing: Option<Listing> = ctx.data(|d| d.get_temp(lid));
    let entries = match listing {
        Some((at, l)) if now - at < 1.0 => l,
        _ => {
            let l = Arc::new(b.list(&dir, st.importable_only));
            ctx.data_mut(|d| d.insert_temp(lid, (now, l.clone())));
            l
        }
    };
    // Toolbar: up, home, path, favourite, importable only.
    let y = rect.min.y + 6.0;
    let mut x = rect.min.x + 8.0;
    if kit::button(app, ui, Rect::from_min_size(pos2(x, y), vec2(30.0, 20.0)), "mediaBrowser.up", "↑", false) {
        kit::exec(app, "mediaBrowser.go", json!({"path": ".."}));
    }
    x += 34.0;
    if kit::button(app, ui, Rect::from_min_size(pos2(x, y), vec2(48.0, 20.0)), "mediaBrowser.home", "Home", false) {
        kit::exec(app, "mediaBrowser.go", json!({}));
    }
    x += 54.0;
    let fav = st.favorites.contains(&dir);
    if kit::button(app, ui, Rect::from_min_size(pos2(x, y), vec2(24.0, 20.0)), "mediaBrowser.favorite", if fav { "★" } else { "☆" }, false) {
        kit::exec(app, if fav { "mediaBrowser.removeFavorite" } else { "mediaBrowser.addFavorite" }, json!({"path": dir}));
    }
    x += 30.0;
    let only = kit::checkbox(app, ui, pos2(x, y + 2.0), "mediaBrowser.importableOnly", "Media only", st.importable_only);
    if only != st.importable_only {
        kit::exec(app, "mediaBrowser.go", json!({"path": dir, "importableOnly": only}));
    }
    // The browser's own actions (web: Open Folder…, Add Files…), right-aligned.
    let mut ax = rect.max.x - 8.0;
    for (id, label) in b.actions().iter().rev() {
        let w = 14.0 + label.chars().count() as f32 * 6.4;
        ax -= w;
        if ax < x + 96.0 {
            break;
        }
        if kit::button(app, ui, Rect::from_min_size(pos2(ax, y), vec2(w, 20.0)), &format!("mediaBrowser.action.{id}"), label, false) {
            kit::exec(app, "mediaBrowser.action", json!({"action": id}));
            ctx.data_mut(|d| d.remove::<Listing>(lid));
        }
        ax -= 6.0;
    }
    // The path on its own row.
    let _ = x;
    let py = y + 24.0;
    p.text(pos2(rect.min.x + 10.0, py + 9.0), Align2::LEFT_CENTER, &dir, Tokens::ui(11.5), t.text_dim);
    app.auto.add("mediaBrowser.path", Rect::from_min_max(pos2(rect.min.x + 8.0, py), pos2(rect.max.x - 8.0, py + 18.0)), &dir);
    // Left column: favourites.
    let side = Rect::from_min_max(pos2(rect.min.x, rect.min.y + 54.0), pos2(rect.min.x + 160.0_f32.min(rect.width() * 0.3), rect.max.y));
    p.line_segment([side.right_top(), side.right_bottom()], Stroke::new(1.0, t.separator));
    p.text(pos2(side.min.x + 10.0, side.min.y + 10.0), Align2::LEFT_CENTER, tr("Favorites"), Tokens::semibold(11.5), t.text_dim);
    let mut fy = side.min.y + 22.0;
    let mut places: Vec<(String, String)> = b.places();
    places.extend(
        st.favorites.iter().map(|f| (std::path::Path::new(f).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| f.clone()), f.clone())),
    );
    for (i, (name, path)) in places.iter().enumerate() {
        let r = Rect::from_min_size(pos2(side.min.x, fy), vec2(side.width(), 20.0));
        let resp = ui.interact(r, egui::Id::new(("mb-fav", i)), Sense::click());
        if *path == dir {
            p.rect_filled(r, 0.0, t.row_selected);
        } else if resp.hovered() {
            p.rect_filled(r, 0.0, t.hover);
        }
        p.text(pos2(r.min.x + 14.0, r.center().y), Align2::LEFT_CENTER, name, Tokens::ui(12.0), t.text);
        app.auto.add(&format!("mediaBrowser.place.{i}"), r, name);
        if resp.clicked() {
            kit::exec(app, "mediaBrowser.go", json!({"path": path}));
        }
        fy += 20.0;
    }
    // Files.
    let list = Rect::from_min_max(pos2(side.max.x + 1.0, side.min.y), rect.max);
    let entries = match entries.as_ref() {
        Ok(e) => e,
        Err(e) => {
            p.text(list.center(), Align2::CENTER_CENTER, e, Tokens::ui(12.0), t.danger);
            return;
        }
    };
    let tile = vec2(140.0, 112.0);
    let cols = ((list.width() - 8.0) / tile.x).floor().max(1.0) as usize;
    let mut budget = 2u32;
    let scroll = widgets::PanelScroll::begin(ui, egui::Id::new("mb-scroll"), list);
    let lp = p.with_clip_rect(list);
    let mut actions: Vec<(&str, Value)> = vec![];
    for (i, e) in entries.iter().enumerate() {
        let r = Rect::from_min_size(
            pos2(list.min.x + 8.0 + (i % cols) as f32 * tile.x, list.min.y + 8.0 + (i / cols) as f32 * tile.y - scroll.offset),
            tile - vec2(8.0, 8.0),
        );
        if !r.intersects(list) {
            continue;
        }
        let resp = ui.interact(r.intersect(list), egui::Id::new(("mb-entry", &e.path)), Sense::click_and_drag());
        app.auto.add(&format!("mediaBrowser.entry.{}", e.name), r, &e.name);
        lp.rect_filled(r, 4.0, if resp.hovered() { t.hover } else { t.row });
        let thumb = Rect::from_min_max(r.min + vec2(6.0, 6.0), pos2(r.max.x - 6.0, r.max.y - 26.0));
        lp.rect_filled(thumb, 3.0, Color32::from_rgb(0x18, 0x18, 0x18));
        let kind = if e.is_dir { "Folder" } else { e.kind.unwrap_or("File") };
        match (e.is_dir, e.kind) {
            (false, Some("image" | "video")) => match thumbnail(app, &ctx, &e.path, &mut budget) {
                Some((tex, [w, h])) => {
                    let s = (thumb.width() / w as f32).min(thumb.height() / h as f32);
                    lp.image(
                        tex.id(),
                        Rect::from_center_size(thumb.center(), vec2(w as f32 * s, h as f32 * s)),
                        Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                        Color32::WHITE,
                    );
                }
                None => {
                    lp.text(thumb.center(), Align2::CENTER_CENTER, kind, Tokens::ui(11.0), t.text_faint);
                }
            },
            _ => {
                lp.text(thumb.center(), Align2::CENTER_CENTER, if e.is_dir { "▣" } else { kind }, Tokens::ui(if e.is_dir { 28.0 } else { 11.0 }), t.text_faint);
            }
        }
        let name = if e.name.chars().count() > 20 { format!("{}…", e.name.chars().take(19).collect::<String>()) } else { e.name.clone() };
        lp.text(pos2(r.min.x + 8.0, r.max.y - 12.0), Align2::LEFT_CENTER, name, Tokens::ui(11.5), t.text);
        if resp.double_clicked() {
            if e.is_dir {
                actions.push(("mediaBrowser.go", json!({"path": e.path})));
            } else if e.kind.is_some() {
                actions.push(("mediaBrowser.import", json!({"paths": [e.path]})));
            }
        }
        if resp.drag_started() && !e.is_dir && e.kind.is_some() {
            egui::DragAndDrop::set_payload(&ctx, DragPayload::Files(vec![e.path.clone()]));
        }
        crate::widgets::context_menu(&resp, |ui| {
            let copy = ui.button(tr("Copy Source Path"));
            app.auto.add(&format!("mediaBrowser.entry.{}.copySourcePath", e.name), copy.rect, "Copy Source Path");
            if copy.clicked() {
                if let Err(e) = crate::menus::invoke(app, &ctx, "mediaBrowser.copySourcePath", json!({"path": e.path})) {
                    app.ui.status = e;
                }
                ui.close();
            }
            if !e.is_dir && ui.button(tr("Import")).clicked() {
                actions.push(("mediaBrowser.import", json!({"paths": [e.path]})));
                ui.close();
            }
            if !e.is_dir && ui.button(tr("Import and Add to Composition")).clicked() {
                actions.push(("mediaBrowser.import", json!({"paths": [e.path], "addToComp": true})));
                ui.close();
            }
            if e.is_dir && ui.button(tr("Add to Favorites")).clicked() {
                actions.push(("mediaBrowser.addFavorite", json!({"path": e.path})));
                ui.close();
            }
        });
    }
    if entries.is_empty() {
        lp.text(list.center(), Align2::CENTER_CENTER, tr("Empty folder"), Tokens::ui(12.0), t.text_faint);
    }
    scroll.end(ui, &mut app.auto, "mediaBrowser.scroll", entries.len().div_ceil(cols) as f32 * tile.y + 8.0, &t);
    if budget == 0 {
        ctx.request_repaint();
    }
    for (id, p) in actions {
        kit::exec(app, id, p);
    }
}

// ---------------------------------------------------------------- Metadata

fn rows_of(v: &Value) -> Vec<(String, String)> {
    let Some(o) = v.as_object() else { return vec![] };
    o.iter()
        .filter(|(k, v)| !v.is_null() && !matches!(k.as_str(), "comment" | "id"))
        .map(|(k, v)| {
            let s = match v {
                Value::String(s) => s.clone(),
                Value::Number(n) => n.as_f64().map(|f| if f.fract() == 0.0 { format!("{f:.0}") } else { format!("{f:.3}") }).unwrap_or_default(),
                other => other.to_string(),
            };
            (k.clone(), s)
        })
        .collect()
}

/// An editable comment field; returns the edited text when it changes.
fn comment_field(ui: &mut egui::Ui, r: Rect, id: &str, text: &str) -> Option<String> {
    let key = egui::Id::new(("meta-comment", id));
    let mut buf: String = ui.data(|d| d.get_temp(key)).unwrap_or_else(|| text.to_string());
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(r));
    let resp = child.add(egui::TextEdit::multiline(&mut buf).desired_width(r.width()).desired_rows(2).hint_text(tr("Add a comment")));
    if resp.has_focus() || resp.changed() {
        ui.data_mut(|d| d.insert_temp(key, buf.clone()));
    } else {
        ui.data_mut(|d| d.remove::<String>(key));
    }
    (resp.lost_focus() && buf != text).then_some(buf)
}

pub fn metadata(app: &mut EffectcraftApp, ui: &mut egui::Ui, rect: Rect) {
    let t = app.tokens;
    let p = ui.painter().with_clip_rect(rect);
    p.rect_filled(rect, 0.0, t.panel_bg);
    let Ok(v) = app.session.execute("item.metadata", json!({})) else { return };
    let mut y = rect.min.y + 8.0;
    let x0 = rect.min.x + 12.0;
    let xv = x0 + 120.0;
    let section = |p: &egui::Painter, y: &mut f32, title: &str| {
        p.text(pos2(x0, *y + 8.0), Align2::LEFT_CENTER, title, Tokens::semibold(12.0), t.text);
        *y += 22.0;
    };
    if let Some(item) = v.get("item").filter(|i| !i.is_null()) {
        let name = item["name"].as_str().unwrap_or_default().to_string();
        let id = item["id"].as_u64().unwrap_or(0);
        section(&p, &mut y, &format!("{name} ({})", item["type"].as_str().unwrap_or("")));
        kit::label(ui, pos2(x0, y), "Comment", &t);
        let r = Rect::from_min_size(pos2(xv, y), vec2((rect.max.x - xv - 12.0).max(80.0), 40.0));
        app.auto.add("metadata.itemComment", r, item["comment"].as_str().unwrap_or(""));
        if let Some(c) = comment_field(ui, r, &format!("item{id}"), item["comment"].as_str().unwrap_or("")) {
            kit::exec(app, "project.setComment", json!({"items": [id], "comment": c}));
        }
        y += 48.0;
        for (k, val) in rows_of(item) {
            if matches!(k.as_str(), "name" | "type") {
                continue;
            }
            p.text(pos2(x0, y + 8.0), Align2::LEFT_CENTER, &k, Tokens::ui(11.5), t.text_dim);
            p.text(pos2(xv, y + 8.0), Align2::LEFT_CENTER, &val, Tokens::ui(11.5), t.text);
            app.auto.add(&format!("metadata.item.{k}"), Rect::from_min_size(pos2(xv, y), vec2(200.0, 16.0)), &val);
            y += 18.0;
        }
        y += 10.0;
    } else {
        p.text(pos2(x0, y + 8.0), Align2::LEFT_CENTER, tr("Select an item in the Project panel"), Tokens::ui(12.0), t.text_faint);
        y += 28.0;
    }
    let proj = v["project"].clone();
    section(&p, &mut y, "Project");
    kit::label(ui, pos2(x0, y), "Comment", &t);
    let r = Rect::from_min_size(pos2(xv, y), vec2((rect.max.x - xv - 12.0).max(80.0), 40.0));
    app.auto.add("metadata.projectComment", r, proj["comment"].as_str().unwrap_or(""));
    if let Some(c) = comment_field(ui, r, "project", proj["comment"].as_str().unwrap_or("")) {
        kit::exec(app, "project.setProjectComment", json!({"comment": c}));
    }
    y += 48.0;
    for (k, val) in rows_of(&proj) {
        p.text(pos2(x0, y + 8.0), Align2::LEFT_CENTER, &k, Tokens::ui(11.5), t.text_dim);
        p.text(pos2(xv, y + 8.0), Align2::LEFT_CENTER, &val, Tokens::ui(11.5), t.text);
        y += 18.0;
    }
}

// ---------------------------------------------------------------- Progress

pub fn progress(app: &mut EffectcraftApp, ui: &mut egui::Ui, rect: Rect) {
    let t = app.tokens;
    let p = ui.painter().with_clip_rect(rect);
    p.rect_filled(rect, 0.0, t.panel_bg);
    if !app.session.tasks.is_empty() {
        app.session.poll_jobs();
    }
    let jobs = app.session.jobs();
    let mut y = rect.min.y + 8.0;
    let x0 = rect.min.x + 12.0;
    let w = (rect.width() - 24.0).max(60.0);
    if jobs.is_empty() {
        p.text(pos2(x0, y + 10.0), Align2::LEFT_CENTER, tr("No background jobs running"), Tokens::ui(12.0), t.text_faint);
        y += 28.0;
    } else {
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(150));
    }
    for j in &jobs {
        p.text(pos2(x0, y + 8.0), Align2::LEFT_CENTER, &j.label, Tokens::medium(12.0), t.text);
        let frac = j.fraction.unwrap_or(0.0) as f32;
        let pct = j.fraction.map(|f| format!("{:.0} %", f * 100.0)).unwrap_or_else(|| "…".into());
        p.text(pos2(x0 + w - 70.0, y + 8.0), Align2::RIGHT_CENTER, format!("{pct}  {}", j.message), Tokens::ui(11.0), t.text_dim);
        progress_bar(&p, Rect::from_min_size(pos2(x0, y + 20.0), vec2(w - 76.0, 6.0)), frac, &t);
        app.auto.add(&format!("progress.job.{}", j.id), Rect::from_min_size(pos2(x0, y), vec2(w, 28.0)), &j.label);
        if j.cancellable
            && kit::button(app, ui, Rect::from_min_size(pos2(x0 + w - 64.0, y + 12.0), vec2(64.0, 18.0)), &format!("progress.cancel.{}", j.id), "Cancel", false)
        {
            kit::exec(app, "jobs.cancel", json!({"job": j.id}));
        }
        y += 38.0;
    }
    if !app.session.job_log.is_empty() {
        p.line_segment([pos2(x0, y), pos2(x0 + w, y)], Stroke::new(1.0, t.separator));
        y += 6.0;
        p.text(pos2(x0, y + 8.0), Align2::LEFT_CENTER, tr("Finished"), Tokens::semibold(11.5), t.text_dim);
        y += 20.0;
        for r in app.session.job_log.iter().rev().take(20) {
            let col = match r.status.as_str() {
                "done" => t.text,
                "cancelled" => t.text_dim,
                _ => t.danger,
            };
            let msg = if r.message.is_empty() { format!("{} · {:.1} s", r.status, r.seconds) } else { format!("{} · {}", r.status, r.message) };
            p.text(pos2(x0, y + 8.0), Align2::LEFT_CENTER, &r.label, Tokens::ui(11.5), col);
            p.text(pos2(x0 + w, y + 8.0), Align2::RIGHT_CENTER, msg, Tokens::ui(11.0), t.text_faint);
            y += 18.0;
            if y > rect.max.y {
                break;
            }
        }
    }
}

/// A job's progress bar (`frac` 0…1).
fn progress_bar(p: &egui::Painter, bar: Rect, frac: f32, t: &Tokens) {
    p.rect_filled(bar, 3.0, t.field_bg);
    p.rect_filled(Rect::from_min_size(bar.min, vec2(bar.width() * frac.clamp(0.0, 1.0), bar.height())), 3.0, t.accent);
}

/// The Importing card, bottom left over the panels while files import in the background (dropped
/// or picked in the Import dialog): the file being read, how many are left, and Cancel (#270).
/// Returns the height it takes, so toasts sit above it.
pub fn import_card(app: &mut EffectcraftApp, ui: &mut egui::Ui, full: Rect) -> f32 {
    if app.session.tasks.is_empty() {
        return 0.0;
    }
    let t = app.tokens;
    let w = 320.0_f32.min(full.width() - 32.0).max(120.0);
    let mut bottom = full.max.y - 16.0;
    for j in app.session.jobs().into_iter().filter(|j| j.kind == "import" && j.running) {
        let r = Rect::from_min_max(pos2(full.min.x + 16.0, bottom - 56.0), pos2(full.min.x + 16.0 + w, bottom));
        let p = ui.painter();
        p.rect_filled(r, 6.0, t.panel_bg);
        p.rect_stroke(r, 6.0, Stroke::new(1.0, t.field_border), egui::StrokeKind::Inside);
        let x0 = r.min.x + 12.0;
        let inner = w - 24.0;
        widgets::text_fit(p, pos2(x0, r.min.y + 15.0), Align2::LEFT_CENTER, &j.label, Tokens::medium(12.0), inner - 64.0, t.text);
        widgets::text_fit(p, pos2(x0, r.min.y + 32.0), Align2::LEFT_CENTER, &j.message, Tokens::ui(11.5), inner, t.text_dim);
        progress_bar(p, Rect::from_min_size(pos2(x0, r.min.y + 42.0), vec2(inner, 6.0)), j.fraction.unwrap_or(0.0) as f32, &t);
        app.auto.add(&format!("import.job.{}", j.id), r, &j.message);
        if kit::button(app, ui, Rect::from_min_size(pos2(r.max.x - 68.0, r.min.y + 6.0), vec2(56.0, 18.0)), &format!("import.cancel.{}", j.id), "Cancel", false)
        {
            kit::exec(app, "jobs.cancel", json!({"job": j.id}));
        }
        bottom = r.min.y - 8.0;
    }
    full.max.y - 16.0 - bottom
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "Media Browser warm listing cache benchmark"]
    fn listing_cache_benchmark() {
        let ctx = egui::Context::default();
        let entries: Vec<_> = (0..1000)
            .map(|i| mb::Entry {
                name: format!("Clip {i:04}.mov"),
                path: format!("/Shoot/作品 Clip {i:04}.mov"),
                is_dir: false,
                size: 1,
                modified: None,
                kind: Some("video"),
            })
            .collect();
        let old_id = egui::Id::new("old-list");
        let new_id = egui::Id::new("new-list");
        ctx.data_mut(|d| {
            d.insert_temp(old_id, (0.0, Ok::<_, String>(entries.clone())));
            d.insert_temp(new_id, (0.0, Arc::new(Ok::<_, String>(entries))));
        });
        let (_, original) = ctx.data(|d| d.get_temp::<Listing>(new_id)).unwrap();
        assert_eq!(original.as_ref().as_ref().unwrap()[0].name, "Clip 0000.mov");
        let mut before = Vec::new();
        let mut after = Vec::new();
        for _ in 0..5 {
            let start = std::time::Instant::now();
            for _ in 0..1000 {
                let _ = std::hint::black_box(ctx.data(|d| d.get_temp::<(f64, Result<Vec<mb::Entry>, String>)>(old_id)).unwrap());
            }
            before.push(start.elapsed());
            let start = std::time::Instant::now();
            for _ in 0..1000 {
                std::hint::black_box(ctx.data(|d| d.get_temp::<Listing>(new_id)).unwrap());
            }
            after.push(start.elapsed());
        }
        let (_, cached) = ctx.data(|d| d.get_temp::<Listing>(new_id)).unwrap();
        assert!(Arc::ptr_eq(&original, &cached));
        assert_eq!(cached.as_ref().as_ref().unwrap().last().unwrap().name, "Clip 0999.mov");
        before.sort();
        after.sort();
        println!("1,000 Media Browser entries, median per warm cache lookup: {:?} -> {:?}", before[2] / 1000, after[2] / 1000);
    }
}
