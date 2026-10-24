//! The Project panel: item preview header (rendered thumbnail of the comp / footage frame),
//! search, item list with columns (Name, Label, then the visible optional columns: Type, Size,
//! Media Duration, Frame Rate, File Path, Comment — shown or hidden from the header's context
//! menu), folders (drag items into and out of them), renaming (Enter, or double-click the name),
//! the label colour picker, and the bottom bar (interpret, new folder, new comp: click, or drop
//! items on it for a comp from them, bit depth, delete), and the keyboard (the arrow keys select,
//! extend the selection and open / close folders; rows and buttons carry AccessKit names and
//! states for screen readers). Edits are engine commands, so they are undoable.

use std::sync::{Arc, Mutex};

use effectcraft_engine::color::Label;
use effectcraft_engine::project::{Item, ItemId, ItemKind};
use egui::{Align2, Color32, Rect, Sense, Stroke, pos2, vec2};
use serde_json::json;

use crate::icons::{self, Icon};
use crate::panels::DragPayload;
use crate::theme::Tokens;
use crate::{EffectcraftApp, widgets};

/// Item row height (After Effects' Project panel rows are compact, about 19 pt).
const ROW_H: f32 = 19.0;

fn item_icon(it: &Item) -> Icon {
    match &it.kind {
        ItemKind::Folder => Icon::Folder,
        ItemKind::Comp(_) => Icon::Comp,
        ItemKind::Solid(_) => Icon::Solid,
        ItemKind::Footage(f) => match f.kind {
            effectcraft_engine::project::FootageKind::Still | effectcraft_engine::project::FootageKind::Sequence => Icon::Image,
            effectcraft_engine::project::FootageKind::Audio => Icon::Audio,
            effectcraft_engine::project::FootageKind::Model => Icon::Cube,
            _ => Icon::Footage,
        },
    }
}

/// Duration as AE prints it in the Project panel header: `0:00:10:00` (h:mm:ss:ff).
pub fn fmt_dur(secs: f64, fps: f64) -> String {
    let fpsi = fps.round().max(1.0) as i64;
    let f = (secs * fps).round() as i64;
    let s = f / fpsi;
    format!("{}:{:02}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60, f % fpsi)
}

/// File size the way AE's Size column shows it (`512 KB`, `12.3 MB`).
pub fn fmt_size(bytes: u64) -> String {
    const K: f64 = 1024.0;
    let b = bytes as f64;
    if b < K * K {
        format!("{} KB", (b / K).ceil() as u64)
    } else if b < K * K * K {
        format!("{:.1} MB", b / (K * K))
    } else {
        format!("{:.2} GB", b / (K * K * K))
    }
}

/// Footage file size on disk (comps, solids, folders and missing footage have none).
fn file_size(it: &Item) -> Option<u64> {
    match &it.kind {
        ItemKind::Footage(f) if !f.missing => std::fs::metadata(&f.path).ok().map(|m| m.len()),
        _ => None,
    }
}

fn file_path(it: &Item) -> &str {
    match &it.kind {
        ItemKind::Footage(f) => &f.path,
        _ => "",
    }
}

/// Optional columns: (key, header, width).
pub const COLUMNS: [(&str, &str, f32); 6] = [
    ("type", "Type", 84.0),
    ("size", "Size", 56.0),
    ("duration", "Media Duration", 100.0),
    ("fps", "Frame Rate", 72.0),
    ("path", "File Path", 220.0),
    ("comment", "Comment", 150.0),
];

/// Sort siblings by the Project panel's sort column; ties fall back to the name.
pub fn sort_items(items: &mut [&Item], col: &str, desc: bool) {
    if col == "size" {
        sort_by_size(items, desc, file_size);
        return;
    }
    let name = |i: &Item| i.name.to_lowercase();
    items.sort_by(|a, b| {
        let o = match col {
            "type" => a.type_name().cmp(b.type_name()),
            "fps" => a.frame_rate().map(|r| r.as_f64()).partial_cmp(&b.frame_rate().map(|r| r.as_f64())).unwrap_or(std::cmp::Ordering::Equal),
            "duration" => a.duration().cmp(&b.duration()),
            "path" => file_path(a).cmp(file_path(b)),
            "comment" => a.comment.cmp(&b.comment),
            "label" => (a.label as u8).cmp(&(b.label as u8)),
            _ => std::cmp::Ordering::Equal,
        }
        .then_with(|| name(a).cmp(&name(b)));
        if desc { o.reverse() } else { o }
    });
}

fn sort_by_size(items: &mut [&Item], desc: bool, mut size_of: impl FnMut(&Item) -> Option<u64>) {
    let mut key = |it: &&Item| (size_of(it), it.name.to_lowercase());
    if desc {
        items.sort_by_cached_key(|it| std::cmp::Reverse(key(it)));
    } else {
        items.sort_by_cached_key(key);
    }
}

/// Where a drop at a row lands: into a folder row, else into the row's folder; `None` row = root.
pub fn drop_folder(project: &effectcraft_engine::project::Project, row: Option<ItemId>) -> Option<ItemId> {
    let it = project.item(row?)?;
    if it.is_folder() { Some(it.id) } else { it.parent }
}

/// A thumbnail being rendered off the UI thread: (revision, result slot).
type PendingThumb = (u64, Arc<Mutex<Option<Option<(u32, u32, Vec<u8>)>>>>);

/// Thumbnail of a comp (its current time) or footage (first frame), cached per item/revision.
/// Rendered on a background thread (inline on wasm32): until the new one lands the previous
/// thumbnail stays up, so selecting a big comp or editing it never stalls the panel.
fn thumbnail(app: &EffectcraftApp, ctx: &egui::Context, it: &Item) -> Option<egui::TextureHandle> {
    let key = egui::Id::new(("proj-thumb", it.id.0));
    let pkey = egui::Id::new(("proj-thumb-pending", it.id.0));
    let rev = app.session.revision;
    let cached: Option<(u64, egui::TextureHandle)> = ctx.data(|d| d.get_temp(key));
    let mut tex = cached.as_ref().map(|(_, t)| t.clone());
    let have = cached.as_ref().map(|(r, _)| *r);
    // A finished render: upload it.
    let pending: Option<PendingThumb> = ctx.data(|d| d.get_temp(pkey));
    if let Some((prev, slot)) = &pending {
        let done = slot.lock().ok().and_then(|mut s| s.take());
        match done {
            Some(Some((w, h, px))) => {
                let ci = egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &px);
                let t = ctx.load_texture(format!("proj-thumb-{}", it.id.0), ci, egui::TextureOptions::LINEAR);
                ctx.data_mut(|d| {
                    d.insert_temp(key, (*prev, t.clone()));
                    d.remove::<PendingThumb>(pkey);
                });
                tex = Some(t);
                if *prev == rev {
                    return tex;
                }
            }
            Some(None) => {
                ctx.data_mut(|d| d.remove::<PendingThumb>(pkey));
                return tex;
            }
            None => {
                ctx.request_repaint_after(std::time::Duration::from_millis(50));
                return tex;
            }
        }
    }
    if have == Some(rev) {
        return tex;
    }
    let t = app.session.state.times.get(&it.id).copied().unwrap_or_default();
    let job = app.session.thumbnail_job(it.id, t, 192)?;
    let slot: Arc<Mutex<Option<Option<(u32, u32, Vec<u8>)>>>> = Arc::default();
    if cfg!(target_arch = "wasm32") {
        *slot.lock().ok()? = Some(job());
    } else {
        let s = slot.clone();
        let spawned = std::thread::Builder::new().name("proj-thumb".into()).spawn(move || {
            let r = job();
            if let Ok(mut s) = s.lock() {
                *s = Some(r);
            }
        });
        if spawned.is_err() {
            return tex;
        }
    }
    ctx.data_mut(|d| d.insert_temp::<PendingThumb>(pkey, (rev, slot)));
    ctx.request_repaint();
    tex
}

fn fit(r: Rect, w: f32, h: f32) -> Rect {
    let s = (r.width() / w.max(1.0)).min(r.height() / h.max(1.0));
    Rect::from_center_size(r.center(), vec2(w * s, h * s))
}

/// An inline text edit in progress: (item, field `name`|`comment`, text).
type Editing = (u64, String, String);
fn edit_id() -> egui::Id {
    egui::Id::new("proj-inline-edit")
}

/// Start renaming the selected item (Enter in the Project panel).
pub fn begin_rename(app: &EffectcraftApp, ctx: &egui::Context) {
    if let Some(id) = app.session.state.project_selection.first()
        && let Some(it) = app.session.project.item(*id)
    {
        ctx.data_mut(|d| d.insert_temp::<Editing>(edit_id(), (id.0, "name".into(), it.name.clone())));
    }
}

/// Where a Shift-extended selection starts (the last item clicked or moved to without Shift).
fn anchor_id() -> egui::Id {
    egui::Id::new("project-selection-anchor")
}

/// The item the arrow keys move from: the last item clicked or moved to.
fn cursor_id() -> egui::Id {
    egui::Id::new("project-selection-cursor")
}

/// An item row's widget: its click / drag area, keyboard focus and accessibility node.
fn row_id(item: ItemId) -> egui::Id {
    egui::Id::new(("pitem", item.0))
}

/// The last row drawn with keyboard focus (egui drops the focus of a row scrolled out of view).
fn focused_row_id() -> egui::Id {
    egui::Id::new("project-focused-row")
}

/// The item whose selection the panel last scrolled into view.
fn selection_seen_id() -> egui::Id {
    egui::Id::new("proj-sel-seen")
}

/// The focused widget is one of the panel's rows, or nothing has keyboard focus: the panel's keys
/// apply. Any other focused widget (a text field, a button reached with Tab) keeps them.
fn keys_reach_rows(app: &EffectcraftApp, ctx: &egui::Context) -> bool {
    match ctx.memory(|m| m.focused()) {
        None => app.ui.focused == crate::dock::PanelKind::Project,
        Some(f) => ctx.data(|d| d.get_temp::<ItemId>(focused_row_id())).is_some_and(|row| row_id(row) == f),
    }
}

/// The Project panel's arrow keys, as in After Effects: Up / Down Arrow select the previous /
/// next item (the first or last when nothing is selected), Shift+Up / Down extend the selection
/// from its anchor, Right Arrow opens the selected folder and Left Arrow closes it (on an item
/// inside a folder, Left selects the folder). They apply while the panel has focus and nothing
/// else takes them: a dialog, an open menu, an inline rename. A row focused with Tab keeps them
/// too, and keyboard focus then follows the selection, so a screen reader reads the new item.
/// Returns the item the selection moved to.
fn keyboard_nav(app: &mut EffectcraftApp, ctx: &egui::Context) -> Option<ItemId> {
    use egui::Key;
    let keys: Vec<(Key, egui::Modifiers)> = ctx.input(|i| {
        i.events
            .iter()
            .filter_map(|e| match e {
                egui::Event::Key { key: k @ (Key::ArrowUp | Key::ArrowDown | Key::ArrowLeft | Key::ArrowRight), pressed: true, modifiers, .. } => {
                    Some((*k, *modifiers))
                }
                _ => None,
            })
            .collect()
    });
    if keys.is_empty()
        || app.dialog.is_some()
        || widgets::any_menu_open(ctx)
        || ctx.data(|d| d.get_temp::<Editing>(edit_id())).is_some()
        || !keys_reach_rows(app, ctx)
    {
        return None;
    }
    let row_focused = ctx.memory(|m| m.focused()).is_some();
    let mut moved = None;
    for (key, m) in keys {
        let (plain, shift) = (!m.any(), m.shift_only());
        // Folders opened or closed by an earlier key in this frame change the rows.
        let rows = visible_rows(app);
        let pos = |id: ItemId| rows.iter().position(|(r, _)| *r == id);
        let sel = &app.session.state.project_selection;
        let cursor = ctx.data(|d| d.get_temp::<ItemId>(cursor_id())).filter(|c| sel.contains(c)).or_else(|| sel.last().copied());
        let cur = cursor.and_then(pos);
        let to = match key {
            Key::ArrowUp | Key::ArrowDown if plain || shift => {
                let Some(last) = rows.len().checked_sub(1) else { continue };
                let next = match (cur, key == Key::ArrowDown) {
                    (Some(c), true) => c.saturating_add(1).min(last),
                    (Some(c), false) => c.saturating_sub(1),
                    (None, true) => 0,
                    (None, false) => last,
                };
                let Some((to, _)) = rows.get(next).copied() else { continue };
                if shift {
                    let anchor = ctx.data(|d| d.get_temp::<ItemId>(anchor_id())).and_then(pos).or(cur).unwrap_or(next);
                    app.session.state.project_selection = rows.get(anchor.min(next)..=anchor.max(next)).unwrap_or_default().iter().map(|(i, _)| *i).collect();
                    to
                } else {
                    app.session.state.project_selection = vec![to];
                    ctx.data_mut(|d| d.insert_temp(anchor_id(), to));
                    to
                }
            }
            Key::ArrowRight | Key::ArrowLeft if plain => {
                let Some((id, folder, parent)) = cursor.and_then(|c| app.session.project.item(c)).map(|it| (it.id, it.is_folder(), it.parent)) else {
                    continue;
                };
                let open = app.ui.project_open_folders.contains(&id.0);
                if key == Key::ArrowRight {
                    if folder {
                        app.ui.project_open_folders.insert(id.0);
                    }
                    continue;
                }
                if folder && open {
                    app.ui.project_open_folders.remove(&id.0);
                    continue;
                }
                let Some(parent) = parent.filter(|p| pos(*p).is_some()) else { continue };
                app.session.state.project_selection = vec![parent];
                ctx.data_mut(|d| d.insert_temp(anchor_id(), parent));
                parent
            }
            _ => continue,
        };
        ctx.data_mut(|d| d.insert_temp(cursor_id(), to));
        moved = Some(to);
    }
    // The arrows are the list's, not egui's spatial focus moves.
    if row_focused {
        ctx.memory_mut(|m| {
            m.move_focus(egui::FocusDirection::None);
            if let Some(to) = moved {
                m.request_focus(row_id(to));
            }
        });
    }
    moved
}

pub(crate) fn visible_rows(app: &EffectcraftApp) -> Vec<(ItemId, usize)> {
    fn walk(app: &EffectcraftApp, folder: Option<ItemId>, depth: usize, q: &str, out: &mut Vec<(ItemId, usize)>) {
        let mut kids = app.session.project.children(folder);
        sort_items(&mut kids, &app.ui.project_sort, app.ui.project_sort_desc);
        for it in kids {
            if !q.is_empty() && !it.is_folder() && !it.name.to_lowercase().contains(q) {
                continue;
            }
            out.push((it.id, depth));
            if it.is_folder() && (app.ui.project_open_folders.contains(&it.id.0) || !q.is_empty()) {
                walk(app, Some(it.id), depth + 1, q, out);
            }
        }
    }
    let mut rows = vec![];
    walk(app, None, 0, &app.ui.project_search.to_lowercase(), &mut rows);
    rows
}

/// Bring `items` into view: clear the panel's search, open every folder holding them, select
/// them, and show and focus the Project panel. Used by Reveal Layer Source in Project and
/// Reveal Composition in Project, so the item is visible even in a large, filtered project.
pub(crate) fn reveal(app: &mut EffectcraftApp, ctx: &egui::Context, items: &[ItemId]) {
    let items: Vec<ItemId> = items.iter().copied().filter(|item| app.session.project.item(*item).is_some()).collect();
    if items.is_empty() {
        return;
    }
    for item in &items {
        let mut parent = app.session.project.item(*item).and_then(|item| item.parent);
        let mut depth = 0;
        while let Some(folder) = parent {
            app.ui.project_open_folders.insert(folder.0);
            parent = app.session.project.item(folder).and_then(|item| item.parent);
            depth += 1;
            if depth > 256 {
                break;
            }
        }
    }
    app.ui.project_search.clear();
    app.session.state.project_selection = items;
    // Force the panel's selection scroll on its next draw, even when the item was already selected.
    ctx.data_mut(|d| d.remove_temp::<Option<u64>>(selection_seen_id()));
    app.show_panel(crate::dock::PanelKind::Project);
}

pub fn show(app: &mut EffectcraftApp, ui: &mut egui::Ui, rect: Rect) {
    let t = app.tokens;
    let p = ui.painter().clone();
    let ctx = ui.ctx().clone();
    // Preview header.
    let head = Rect::from_min_size(rect.min, vec2(rect.width(), 74.0));
    let sel = app.session.state.project_selection.first().copied();
    if let Some(it) = sel.and_then(|i| app.session.project.item(i)).cloned() {
        let thumb = Rect::from_min_size(head.min + vec2(10.0, 10.0), vec2(96.0, 54.0));
        p.rect_filled(thumb, 3.0, Color32::from_rgb(0x15, 0x15, 0x15));
        match &it.kind {
            ItemKind::Solid(s) => {
                p.rect_filled(thumb.shrink(4.0), 2.0, Color32::from_rgb((s.color[0] * 255.0) as u8, (s.color[1] * 255.0) as u8, (s.color[2] * 255.0) as u8));
            }
            _ => match thumbnail(app, &ctx, &it) {
                Some(tex) => {
                    let [w, h] = tex.size();
                    p.image(tex.id(), fit(thumb, w as f32, h as f32), Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
                }
                None => icons::paint(&p, Rect::from_center_size(thumb.center(), vec2(24.0, 24.0)), item_icon(&it), t.text_dim),
            },
        }
        app.auto.add("project.thumbnail", thumb, &it.name);
        // The details stay inside the panel (a margin on the right), cut short with "…".
        let tx = thumb.max.x + 10.0;
        let max_w = head.max.x - 10.0 - tx;
        // "Name ▾, used 3 times" (After Effects): the layers in every comp whose source the item
        // is; the ▾ lists those comps and opens the one picked. Nothing for an unused item.
        let users = app.session.project.item_users(it.id);
        let count = users.iter().fold(0usize, |n, (_, k)| n.saturating_add(*k));
        let name_y = thumb.min.y + 6.0;
        if count == 0 {
            let r = widgets::text_fit(&p, pos2(tx, name_y), Align2::LEFT_CENTER, &it.name, Tokens::semibold(12.0), max_w, t.text);
            app.auto.add("project.details.name", r, &it.name);
        } else {
            let used = if count == 1 { crate::i18n::tr("used once").to_owned() } else { crate::i18n::tr_args("used {} times", &[&count]) };
            let used = format!(", {used}");
            let used_w = p.layout_no_wrap(used.clone(), Tokens::ui(11.0), t.text_dim).size().x;
            let arrow_w = 14.0;
            // The name gives way first, but keeps at least half the room; the count is cut after it.
            let name_max = (max_w - arrow_w - used_w).max(max_w * 0.5);
            let r = widgets::text_fit(&p, pos2(tx, name_y), Align2::LEFT_CENTER, &it.name, Tokens::semibold(12.0), name_max, t.text);
            app.auto.add("project.details.name", r, &it.name);
            let ar = Rect::from_min_size(pos2(r.max.x + 1.0, name_y - 7.0), vec2(arrow_w, 14.0));
            let tip = crate::i18n::tr("Compositions that use this item");
            let aresp = ui.interact(ar, egui::Id::new("proj-used-in"), Sense::click()).on_hover_text(tip);
            icons::paint(&p, Rect::from_center_size(ar.center(), vec2(9.0, 9.0)), Icon::ChevronDown, if aresp.hovered() { t.text } else { t.text_dim });
            app.auto.add("project.details.usedIn", ar, tip);
            // The comma sits right after the arrow (its glyph ends short of the hit area).
            let ux = ar.max.x - 3.0;
            let room = (head.max.x - 10.0 - ux).max(0.0);
            let ur = widgets::text_fit(&p, pos2(ux, name_y), Align2::LEFT_CENTER, &used, Tokens::ui(11.0), room, t.text_dim);
            app.auto.add("project.details.used", ur, &used);
            let names: Vec<(u64, String, usize)> =
                users.iter().map(|(cid, n)| (cid.0, app.session.project.item(*cid).map(|c| c.name.clone()).unwrap_or_default(), *n)).collect();
            let mut open = None;
            egui::Popup::menu(&aresp).show(|ui| {
                for (cid, name, n) in &names {
                    let label = if *n == 1 { name.clone() } else { format!("{name} ({n})") };
                    let b = ui.button(label);
                    app.auto.add(&format!("project.details.usedIn.{cid}"), b.rect, name);
                    if b.clicked() {
                        open = Some(*cid);
                        ui.close();
                    }
                }
            });
            if let Some(cid) = open
                && let Err(e) = crate::menus::invoke(app, &ctx, "comp.open", json!({"comp": cid}))
            {
                app.ui.status = e;
            }
        }
        let mut lines = vec![];
        if let Some((w, h)) = it.dimensions() {
            lines.push(format!("{w} x {h} (1.00)"));
        }
        if let Some(d) = it.duration() {
            let fps = it.frame_rate().map(|r| r.as_f64()).unwrap_or(30.0);
            lines.push(format!("Δ {}, {:.2} fps", fmt_dur(d.seconds(), fps), fps));
        }
        if let ItemKind::Footage(f) = &it.kind {
            lines.push(f.codec.clone());
        }
        for (i, l) in lines.iter().enumerate() {
            let r = widgets::text_fit(&p, pos2(tx, thumb.min.y + 22.0 + 13.0 * i as f32), Align2::LEFT_CENTER, l, Tokens::ui(11.0), max_w, t.text_dim);
            app.auto.add(&format!("project.details.{i}"), r, l);
        }
    } else {
        let hint = "Select an item to see its details";
        let r = widgets::text_fit(&p, head.center(), Align2::CENTER_CENTER, crate::i18n::tr(hint), Tokens::ui(11.0), head.width() - 20.0, t.text_faint);
        app.auto.add("project.details.hint", r, hint);
    }
    // Search.
    let sr = Rect::from_min_size(pos2(rect.min.x + 8.0, head.max.y + 2.0), vec2(rect.width() - 16.0, 22.0));
    let mut q = app.ui.project_search.clone();
    widgets::search_field(ui, sr, &mut q, crate::i18n::tr("Search project"), &t);
    app.ui.project_search = q;
    app.auto.add("project.search", sr, "Search");
    // Column header: Name, Label, then the visible optional columns.
    let hdr = Rect::from_min_size(pos2(rect.min.x, sr.max.y + 6.0), vec2(rect.width(), 20.0));
    p.line_segment([hdr.left_bottom(), hdr.right_bottom()], Stroke::new(1.0, t.separator));
    let name_w = (rect.width() * 0.40).clamp(120.0, 260.0);
    let label_x = rect.min.x + name_w + 14.0;
    let mut cols: Vec<(&'static str, &'static str, f32, f32)> = vec![("name", "Name", rect.min.x + 26.0, name_w - 26.0), ("label", "", label_x - 8.0, 18.0)];
    // Optional columns wider than the panel scroll horizontally under the frozen Name and Label
    // columns (Shift+wheel / trackpad over the list, or the scroll bar under it).
    let opt_x0 = label_x + 16.0;
    let natural_end = opt_x0 + app.ui.project_columns.iter().filter_map(|k| COLUMNS.iter().find(|c| c.0 == k)).map(|c| c.2 + 4.0).sum::<f32>();
    let overflow = (natural_end - rect.max.x + 4.0).max(0.0);
    app.ui.project_hscroll = app.ui.project_hscroll.clamp(0.0, overflow);
    let hscroll = app.ui.project_hscroll;
    let mut x = opt_x0 - hscroll;
    for key in app.ui.project_columns.clone() {
        if let Some((k, l, w)) = COLUMNS.iter().find(|c| c.0 == key) {
            cols.push((k, l, x, *w));
            x += w + 4.0;
        }
    }
    let opt_clip = |r: Rect| Rect::from_min_max(pos2(r.min.x.max(opt_x0 - 4.0), r.min.y), r.max);
    let hp = p.with_clip_rect(hdr);
    for (key, label, x, w) in &cols {
        let hr = Rect::from_min_size(pos2(x - 4.0, hdr.min.y), vec2(*w, hdr.height()));
        let hr = if matches!(*key, "name" | "label") { hr } else { opt_clip(hr) };
        if hr.width() <= 0.0 {
            continue;
        }
        let resp = ui.interact(hr.intersect(hdr), egui::Id::new(("proj-sort", *key)), Sense::click());
        hp.with_clip_rect(hr).text(
            pos2(*x, hdr.center().y),
            Align2::LEFT_CENTER,
            crate::i18n::tr(label),
            Tokens::ui(11.0),
            if resp.hovered() { t.text } else { t.text_dim },
        );
        if *key == "label" {
            icons::paint(&hp, Rect::from_center_size(pos2(label_x, hdr.center().y), vec2(10.0, 10.0)), Icon::Keyframe, t.text_dim);
        }
        if app.ui.project_sort == *key {
            let c = pos2(hr.max.x - 10.0, hdr.center().y);
            let d = if app.ui.project_sort_desc { 1.0 } else { -1.0 };
            hp.add(egui::Shape::convex_polygon(
                vec![pos2(c.x - 4.0, c.y - 2.0 * d), pos2(c.x + 4.0, c.y - 2.0 * d), pos2(c.x, c.y + 2.5 * d)],
                t.text_dim,
                Stroke::NONE,
            ));
        }
        app.auto.add(&format!("project.sort.{key}"), hr, if label.is_empty() { "Label" } else { label });
        let column = crate::i18n::tr(if label.is_empty() { "Label" } else { label });
        let order = (app.ui.project_sort == *key).then(|| crate::i18n::tr(if app.ui.project_sort_desc { "descending" } else { "ascending" }));
        resp.widget_info(|| {
            let name = match order {
                Some(o) => crate::i18n::tr_args("Sort by {}, {}", &[&column, &o]),
                None => crate::i18n::tr_args("Sort by {}", &[&column]),
            };
            egui::WidgetInfo::labeled(egui::WidgetType::Button, true, name)
        });
        if resp.clicked() {
            if app.ui.project_sort == *key {
                app.ui.project_sort_desc = !app.ui.project_sort_desc;
            } else {
                app.ui.project_sort = key.to_string();
                app.ui.project_sort_desc = false;
            }
        }
        column_menu(app, &resp);
    }
    let hresp = ui.interact(Rect::from_min_max(pos2(x, hdr.min.y), hdr.max), egui::Id::new("proj-hdr-rest"), Sense::CLICK);
    column_menu(app, &hresp);
    app.auto.add("project.columns", hdr, "Columns (right-click to show or hide)");
    // Rows.
    let footer_h = 28.0;
    let list = Rect::from_min_max(pos2(rect.min.x, hdr.max.y), pos2(rect.max.x, rect.max.y - footer_h));
    let lp = p.with_clip_rect(list);
    // The list's empty area (under the rows and the scroll bar, which take their own clicks):
    // a click deselects, a drag draws a selection box, a double-click imports, as in After
    // Effects (File ▸ Import ▸ File...).
    // Not a Tab stop: the keyboard reaches the rows, and nothing else here takes a key.
    let empty = ui.interact(list, egui::Id::new("proj-empty"), Sense::CLICK | Sense::DRAG);
    app.auto.add("project.empty", list, "Double-click to import files; drag to select items; right-click for project actions");
    if overflow > 0.0 && ui.rect_contains_pointer(list) {
        let (dx, dy, shift) = ui.input(|i| (i.smooth_scroll_delta.x, i.smooth_scroll_delta.y, i.modifiers.shift));
        let d = if dx.abs() > 0.0 {
            dx
        } else if shift {
            dy
        } else {
            0.0
        };
        app.ui.project_hscroll = (app.ui.project_hscroll - d).clamp(0.0, overflow);
    }
    let moved = keyboard_nav(app, &ctx);
    let rows = visible_rows(app);
    // Enter renames the selected item (Project panel focused, not typing).
    if app.ui.focused == crate::dock::PanelKind::Project
        && app.dialog.is_none()
        && keys_reach_rows(app, &ctx)
        && ctx.data(|d| d.get_temp::<Editing>(edit_id())).is_none()
        && ctx.input(|i| i.key_pressed(egui::Key::Enter))
    {
        begin_rename(app, &ctx);
    }
    let editing: Option<Editing> = ctx.data(|d| d.get_temp(edit_id()));
    let dragging = egui::DragAndDrop::payload::<DragPayload>(&ctx).and_then(|p| match *p {
        DragPayload::Item(i) => Some(i),
        _ => None,
    });
    let hover_y = ctx.pointer_hover_pos().filter(|p| list.contains(*p)).map(|p| p.y);
    let mut drop_row: Option<ItemId> = None;
    let mut actions: Vec<(String, serde_json::Value)> = vec![];
    // Vertical scrolling: only the rows in view are drawn (and registered), so a project with
    // thousands of items costs what one screenful does.
    let content_h = rows.len() as f32 * ROW_H;
    let max_scroll = (content_h - list.height() + ROW_H).max(0.0);
    if ui.rect_contains_pointer(list) {
        let (dy, shift) = ui.input(|i| (i.smooth_scroll_delta.y, i.modifiers.shift));
        if !shift && dy != 0.0 {
            app.ui.project_scroll -= dy;
        }
    }
    // A selection made elsewhere (a command, an agent, the viewer) scrolls into view.
    let sel_key = selection_seen_id();
    let sel_now = app.session.state.project_selection.last().map(|i| i.0);
    if ctx.data(|d| d.get_temp::<Option<u64>>(sel_key)) != Some(sel_now) {
        ctx.data_mut(|d| d.insert_temp(sel_key, sel_now));
        if let Some(row) = sel_now.and_then(|s| rows.iter().position(|(id, _)| id.0 == s)) {
            let top = row as f32 * ROW_H;
            if top < app.ui.project_scroll {
                app.ui.project_scroll = top;
            } else if top + ROW_H > app.ui.project_scroll + list.height() {
                app.ui.project_scroll = top + ROW_H - list.height();
            }
        }
    }
    // The arrow keys' item scrolls into view (Shift+Up extends upward, past the selection's end).
    if let Some(row) = moved.and_then(|m| rows.iter().position(|(id, _)| *id == m)) {
        let top = row as f32 * ROW_H;
        // Not `clamp`: a panel shorter than a row would make its bounds cross. The row's top wins.
        app.ui.project_scroll = app.ui.project_scroll.max(top + ROW_H - list.height()).min(top);
    }
    app.ui.project_scroll = app.ui.project_scroll.clamp(0.0, max_scroll);
    let scroll = app.ui.project_scroll;
    let first = ((scroll / ROW_H).floor() as usize).min(rows.len());
    let mut y = list.min.y - (scroll - first as f32 * ROW_H);
    let project = app.session.project.clone();
    for (i, (id, depth)) in rows.iter().enumerate().skip(first) {
        let Some(it) = project.item(*id) else { continue };
        let r = Rect::from_min_size(pos2(list.min.x, y), vec2(list.width(), ROW_H));
        y += ROW_H;
        if r.min.y > list.max.y {
            break;
        }
        if hover_y.is_some_and(|hy| hy >= r.min.y && hy < r.max.y) {
            drop_row = Some(*id);
        }
        let selected = app.session.state.project_selection.contains(id);
        lp.rect_filled(
            r,
            0.0,
            // Flat rows like After Effects' Project panel (no stripes).
            if selected { t.row_selected } else { t.row },
        );
        // The row's click / drag area goes first, so the twirl, proxy, label and comment widgets
        // on top of it take their own clicks.
        let mut resp = ui.interact(r.intersect(list), row_id(*id), Sense::click_and_drag());
        app.auto.add(&format!("project.item.{}", id.0), r, &it.name);
        // For screen readers: a tree item named by the item and its type, with its selection,
        // depth and (a folder) open state. One Tab stop per row; the panel's keys then apply.
        let open = it.is_folder().then(|| app.ui.project_open_folders.contains(&id.0) || !app.ui.project_search.is_empty());
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Other, true, format!("{}, {}", it.name, crate::i18n::tr(it.type_name()))));
        ctx.accesskit_node_builder(resp.id, |n| {
            n.set_role(egui::accesskit::Role::TreeItem);
            n.set_selected(selected);
            n.set_level(depth.saturating_add(1));
            if let Some(open) = open {
                n.set_expanded(open);
            }
        });
        if resp.has_focus() {
            ctx.data_mut(|d| d.insert_temp(focused_row_id(), *id));
        }
        if resp.gained_focus() {
            app.ui.focused = crate::dock::PanelKind::Project;
        }
        let _ = i;
        let x0 = r.min.x + 8.0 + 14.0 * *depth as f32;
        if it.is_folder() {
            let open = app.ui.project_open_folders.contains(&id.0);
            let tw = Rect::from_center_size(pos2(x0 + 4.0, r.center().y), vec2(12.0, 12.0));
            let tresp = widgets::twirl(ui, tw, open, egui::Id::new(("pf", id.0)), &t);
            tresp.widget_info(|| {
                let name = crate::i18n::tr_args(if open { "Close folder {}" } else { "Open folder {}" }, &[&it.name]);
                egui::WidgetInfo::labeled(egui::WidgetType::Button, true, name)
            });
            if tresp.clicked() {
                // Alt/Cmd-click twirls every folder inside it too.
                let mut ids = vec![*id];
                if ui.input(|i| i.modifiers.alt || i.modifiers.command) {
                    let mut at = 0;
                    while let Some(f) = ids.get(at).copied() {
                        ids.extend(app.session.project.children(Some(f)).into_iter().filter(|c| c.is_folder()).map(|c| c.id));
                        at += 1;
                    }
                }
                for f in ids {
                    if open {
                        app.ui.project_open_folders.remove(&f.0);
                    } else {
                        app.ui.project_open_folders.insert(f.0);
                    }
                }
            }
            app.auto.add(&format!("project.item.{}.twirl", id.0), tw, &it.name);
        }
        icons::paint(&lp, Rect::from_center_size(pos2(x0 + 18.0, r.center().y), vec2(14.0, 14.0)), item_icon(it), t.text_dim);
        // Proxy indicator: filled = the proxy is used, hollow = set but off. Click to switch.
        if let Some(px) = it.proxy.as_ref() {
            let pr = Rect::from_center_size(pos2(x0 + 4.0, r.center().y), vec2(9.0, 9.0));
            if px.enabled {
                lp.rect_filled(pr, 1.0, t.text);
            } else {
                lp.rect_stroke(pr, 1.0, Stroke::new(1.0, t.text), egui::StrokeKind::Inside);
            }
            let presp = ui.interact(pr.expand(2.0).intersect(list), egui::Id::new(("pproxy", id.0)), Sense::click()).on_hover_text(format!(
                "Proxy: {} ({})",
                px.footage.path,
                if px.enabled { "in use" } else { "off" }
            ));
            app.auto.add(&format!("project.item.{}.proxy", id.0), pr, if px.enabled { "Proxy in use" } else { "Proxy off" });
            presp.widget_info(|| {
                let name = crate::i18n::tr_args(if px.enabled { "Proxy of {}: in use" } else { "Proxy of {}: off" }, &[&it.name]);
                egui::WidgetInfo::labeled(egui::WidgetType::Button, true, name)
            });
            if presp.clicked() {
                actions.push(("file.useProxy".into(), json!({"item": id.0})));
            }
        }
        // Footage that can't be decoded (no decoder for its codec / profile): a warning marker
        // at the end of the name cell, the reason on hover.
        let problem = match &it.kind {
            ItemKind::Footage(f) => app.session.footage.problem(f),
            _ => None,
        };
        let name_clip =
            Rect::from_min_max(pos2(x0 + 30.0, r.min.y), pos2(r.min.x + name_w + 4.0 - if problem.is_some() { 16.0 } else { 0.0 }, r.max.y)).intersect(list);
        if let Some(why) = &problem {
            let m = Rect::from_center_size(pos2(r.min.x + name_w - 8.0, r.center().y), vec2(13.0, 12.0));
            lp.add(egui::Shape::convex_polygon(vec![m.center_top(), m.right_bottom(), m.left_bottom()], t.warning, Stroke::NONE));
            lp.text(m.center() + vec2(0.0, 1.5), Align2::CENTER_CENTER, "!", Tokens::semibold(9.0), Color32::BLACK);
            ui.interact(m.expand(2.0).intersect(list), egui::Id::new(("pproblem", id.0)), Sense::hover()).on_hover_text(why.as_str());
            app.auto.add(&format!("project.item.{}.problem", id.0), m, why);
        }
        let name_rect = Rect::from_min_max(pos2(x0 + 28.0, r.min.y + 2.0), pos2(r.min.x + name_w + 2.0, r.max.y - 2.0));
        // Inline edits (rename / comment).
        let cell_edit = |app: &mut EffectcraftApp, ui: &mut egui::Ui, actions: &mut Vec<(String, serde_json::Value)>, field: &str, cell: Rect| -> bool {
            let Some((eid, ef, mut buf)) = editing.clone().filter(|(e, f, _)| *e == id.0 && f == field) else { return false };
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(cell));
            // The field keeps Tab (egui would move the keyboard to the next widget, where Enter no
            // longer renames, #881): Tab commits like Enter and gives the keyboard back to the panel.
            let filter = egui::EventFilter { tab: true, horizontal_arrows: true, vertical_arrows: true, ..Default::default() };
            let resp = child.add(egui::TextEdit::singleline(&mut buf).desired_width(cell.width()).font(Tokens::ui(12.0)).event_filter(filter));
            app.auto.add(&format!("project.item.{}.{field}Edit", id.0), cell, field);
            let tab = resp.has_focus() && ctx.input(|i| i.key_pressed(egui::Key::Tab));
            if tab {
                resp.surrender_focus();
            }
            if resp.lost_focus() || tab {
                ctx.data_mut(|d| d.remove::<Editing>(edit_id()));
                if !ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
                    let (cmd, params) = if ef == "name" {
                        ("project.rename", json!({"item": eid, "name": buf}))
                    } else {
                        ("project.setComment", json!({"items": [eid], "comment": buf}))
                    };
                    actions.push((cmd.into(), params));
                }
            } else {
                widgets::keep_focus(&ctx, &resp, &buf);
                ctx.data_mut(|d| d.insert_temp::<Editing>(edit_id(), (eid, ef, buf)));
            }
            true
        };
        if !cell_edit(app, ui, &mut actions, "name", name_rect) {
            // The selected item's name sits in a light cell with dark text.
            if selected {
                let g = lp.layout_no_wrap(it.name.clone(), Tokens::ui(12.0), Color32::BLACK);
                let cell = Rect::from_min_max(pos2(x0 + 27.0, r.min.y + 1.0), pos2((x0 + 33.0 + g.size().x).min(name_clip.max.x), r.max.y - 1.0));
                lp.with_clip_rect(name_clip.expand2(vec2(3.0, 0.0))).rect_filled(cell, 0.0, Color32::from_rgb(0xa6, 0xa6, 0xa6));
            }
            // Long names end in "…" before the label column instead of being cut mid-glyph.
            widgets::text_fit(
                &lp.with_clip_rect(name_clip),
                pos2(x0 + 30.0, r.center().y),
                Align2::LEFT_CENTER,
                &it.name,
                Tokens::ui(12.0),
                name_clip.max.x - (x0 + 30.0) - 2.0,
                if selected { Color32::from_gray(0x16) } else { t.text },
            );
        }
        // Label swatch: click for the label colour menu.
        let sw = Rect::from_center_size(pos2(label_x, r.center().y), vec2(10.0, 10.0));
        lp.rect_filled(sw, 2.0, t.label(it.label));
        let sresp = ui.interact(sw.expand(3.0).intersect(list), egui::Id::new(("plabel", id.0)), Sense::click());
        app.auto.add(&format!("project.item.{}.label", id.0), sw, it.label.name());
        sresp.widget_info(|| {
            let name = crate::i18n::tr_args("Label of {}: {}", &[&it.name, &app.session.prefs.label_name(it.label)]);
            egui::WidgetInfo::labeled(egui::WidgetType::Button, true, name)
        });
        let pop = egui::Id::new(("plabel-pop", id.0));
        if sresp.clicked() || sresp.secondary_clicked() {
            widgets::open_popup(ui, pop);
        }
        if let Some(l) = widgets::label_popup(ui, pop, sw.left_bottom(), it.label, |l| app.session.prefs.label_name(l), &t) {
            actions.push(("project.setLabel".into(), json!({"items": targets(app, *id, selected), "label": label_index(l)})));
        }
        // Optional columns.
        for (key, _, cx, w) in cols.iter().skip(2) {
            let cell = opt_clip(Rect::from_min_size(pos2(*cx, r.min.y), vec2(*w, r.height())).intersect(list));
            if cell.width() <= 0.0 {
                continue;
            }
            let cp = lp.with_clip_rect(cell);
            let txt = match *key {
                "type" => it.type_name().to_string(),
                "size" => file_size(it).map(fmt_size).unwrap_or_default(),
                "duration" => it.duration().map(|d| fmt_dur(d.seconds(), it.frame_rate().map(|r| r.as_f64()).unwrap_or(30.0))).unwrap_or_default(),
                "fps" => it.frame_rate().map(|f| format!("{:.2}", f.as_f64())).unwrap_or_default(),
                "path" => file_path(it).to_string(),
                "comment" => {
                    let cr = Rect::from_min_size(pos2(*cx - 2.0, r.min.y + 2.0), vec2(*w, r.height() - 4.0));
                    if cell_edit(app, ui, &mut actions, "comment", cr) {
                        continue;
                    }
                    let cresp = ui.interact(cell, egui::Id::new(("pcomment", id.0)), Sense::CLICK);
                    app.auto.add(&format!("project.item.{}.comment", id.0), cell, &it.comment);
                    cresp.widget_info(|| {
                        egui::WidgetInfo::labeled(egui::WidgetType::Other, true, crate::i18n::tr_args("Comment of {}: {}", &[&it.name, &it.comment]))
                    });
                    if cresp.double_clicked() {
                        ctx.data_mut(|d| d.insert_temp::<Editing>(edit_id(), (id.0, "comment".into(), it.comment.clone())));
                    } else {
                        // Other clicks select the row and open its context menu.
                        resp = resp.union(cresp);
                    }
                    it.comment.clone()
                }
                _ => String::new(),
            };
            let align = if *key == "fps" { Align2::RIGHT_CENTER } else { Align2::LEFT_CENTER };
            let tx = if *key == "fps" { cx + w - 6.0 } else { *cx };
            cp.text(pos2(tx, r.center().y), align, txt, Tokens::ui(11.5), t.text_dim);
        }
        if dragging.is_some_and(|d| d != id.0) && drop_row == Some(*id) && it.is_folder() {
            lp.rect_stroke(r.shrink(1.0), 2.0, Stroke::new(1.5, t.accent), egui::StrokeKind::Inside);
        }
        // The row with keyboard focus (Tab, or the arrow keys after it) shows a focus ring.
        if ctx.memory(|m| m.has_focus(row_id(*id))) {
            lp.rect_stroke(r.shrink(0.5), 0.0, Stroke::new(1.0, t.accent), egui::StrokeKind::Inside);
        }
        // Part of the row: not a second Tab stop.
        let nresp = ui.interact(name_clip, egui::Id::new(("pname", id.0)), Sense::CLICK | Sense::DRAG);
        app.auto.add(&format!("project.item.{}.name", id.0), name_clip, &it.name);
        let resp = resp.union(nresp);
        if resp.clicked() {
            let m = ui.input(|i| i.modifiers);
            let anchor_key = anchor_id();
            ctx.data_mut(|d| d.insert_temp(cursor_id(), *id));
            let anchor = ctx.data(|d| d.get_temp::<ItemId>(anchor_key)).or_else(|| app.session.state.project_selection.first().copied());
            if m.shift
                && let Some(a) = anchor.and_then(|a| rows.iter().position(|(i, _)| *i == a))
            {
                let b = rows.iter().position(|(i, _)| i == id).unwrap_or(a);
                app.session.state.project_selection = rows.get(a.min(b)..=a.max(b)).unwrap_or_default().iter().map(|(i, _)| *i).collect();
            } else if m.command {
                if app.session.state.project_selection.contains(id) {
                    app.session.state.project_selection.retain(|i| i != id);
                } else {
                    app.session.state.project_selection.push(*id);
                }
                ctx.data_mut(|d| d.insert_temp(anchor_key, *id));
            } else {
                app.session.state.project_selection = vec![*id];
                ctx.data_mut(|d| d.insert_temp(anchor_key, *id));
            }
        }
        // Double-click opens the item, as in After Effects (a comp gets its own Timeline tab and
        // the viewer); Enter or the context menu renames.
        if resp.double_clicked() {
            match &it.kind {
                ItemKind::Comp(_) => actions.push(("comp.open".into(), json!({"comp": id.0}))),
                ItemKind::Folder if !app.ui.project_open_folders.remove(&id.0) => {
                    app.ui.project_open_folders.insert(id.0);
                }
                // Footage (not data) and solids open in the Footage panel.
                ItemKind::Footage(f) if f.kind != effectcraft_engine::project::FootageKind::Data => {
                    actions.push(("footage.open".into(), json!({"item": id.0})))
                }
                ItemKind::Solid(_) => actions.push(("footage.open".into(), json!({"item": id.0}))),
                _ => {}
            }
        }
        if resp.drag_started() {
            egui::DragAndDrop::set_payload(&ctx, DragPayload::Item(id.0));
        }
        // Right-clicking an item selects it (After Effects): the menu's File commands act on the
        // selection.
        if resp.secondary_clicked() && !selected {
            app.session.state.project_selection = vec![*id];
        }
        let footage = matches!(it.kind, ItemKind::Footage(_));
        crate::widgets::context_menu(&resp, |ui| {
            // First on a comp, as in After Effects; for this comp, not the viewer's (#1058).
            if matches!(it.kind, ItemKind::Comp(_)) && ui.button(crate::i18n::tr("Composition Settings…")).clicked() {
                actions.push(("comp.settings".into(), json!({"comp": id.0})));
                ui.close();
            }
            if ui.button(crate::i18n::tr("Delete")).clicked() {
                actions.push(("project.delete".into(), json!({"items": targets(app, *id, selected)})));
                ui.close();
            }
            if matches!(it.kind, ItemKind::Comp(_)) && ui.button(crate::i18n::tr("Open Composition")).clicked() {
                actions.push(("comp.open".into(), json!({"comp": id.0})));
                ui.close();
            }
            if ui.button(crate::i18n::tr("Rename")).clicked() {
                ctx.data_mut(|d| d.insert_temp::<Editing>(edit_id(), (id.0, "name".into(), it.name.clone())));
                ui.close();
            }
            let duplicate_items = targets(app, *id, selected);
            // Like Edit > Duplicate: folders disable the whole operation, including mixed selections.
            let can_duplicate =
                !duplicate_items.is_empty() && duplicate_items.iter().all(|id| app.session.project.item(ItemId(*id)).is_some_and(|i| !i.is_folder()));
            let mut duplicate = egui::Button::new(crate::i18n::tr("Duplicate"));
            if let Some(shortcut) = app.session.shortcuts().shortcut_of("edit.duplicate", &serde_json::Value::Null) {
                duplicate = duplicate.shortcut_text(crate::menus::shortcut_text(shortcut));
            }
            let response = ui.add_enabled(can_duplicate, duplicate);
            app.auto.add("project.context.duplicate", response.rect, "Duplicate");
            if response.clicked() {
                actions.push(("project.duplicate".into(), json!({"items": duplicate_items})));
                ui.close();
            }
            if it.parent.is_some() && ui.button(crate::i18n::tr("Move to Project Root")).clicked() {
                actions.push(("project.move".into(), json!({"items": targets(app, *id, selected), "folder": null})));
                ui.close();
            }
            if !it.is_folder() && ui.button(crate::i18n::tr("Add to Composition")).clicked() {
                actions.push(("layer.addItem".into(), json!({"item": id.0})));
                ui.close();
            }
            // File ▸ New Comp from Selection on the selection (the right-clicked row joins it
            // above): several items ask how in its dialog, as in After Effects (#360).
            if ui.button(crate::i18n::tr("New Comp from Selection")).clicked() {
                actions.push(("file.newCompFromSelection".into(), json!({})));
                ui.close();
            }
            // File ▸ Replace Footage ▸ With Precomposition: every layer using the item uses a
            // new composition holding it instead. The label says how many layers that changes.
            let uses: usize = app.session.project.item_users(*id).iter().map(|(_, n)| n).sum();
            let label = match uses {
                0 => crate::i18n::tr("Replace with Precomposition").to_owned(),
                1 => format!("{} ({})", crate::i18n::tr("Replace with Precomposition"), crate::i18n::tr("used once")),
                n => format!("{} ({})", crate::i18n::tr("Replace with Precomposition"), crate::i18n::tr_args("used {} times", &[&n])),
            };
            if (footage || matches!(it.kind, ItemKind::Solid(_))) && ui.button(label).clicked() {
                actions.push(("file.replaceWithPrecomp".into(), json!({"item": id.0})));
                ui.close();
            }
            ui.separator();
            ui.menu_button(crate::i18n::tr("Label"), |ui| {
                for l in Label::ALL {
                    if widgets::label_entry(ui, l, &app.session.prefs.label_name(l), l == it.label, &t).clicked() {
                        actions.push(("project.setLabel".into(), json!({"items": targets(app, *id, selected), "label": label_index(l)})));
                        ui.close();
                    }
                }
            });
            // Footage: the File menu's footage commands (Interpret Footage, Replace Footage…).
            if footage {
                let mut chosen = None;
                crate::menus::submenu_at(app, ui, &["File", "Interpret Footage"], &mut chosen);
                crate::menus::submenu_at(app, ui, &["File", "Replace Footage"], &mut chosen);
                crate::menus::entry_for(app, ui, "file.reloadFootage", &mut chosen);
                crate::menus::entry_for(app, ui, "file.revealInFinder", &mut chosen);
                actions.extend(chosen);
            }
        });
    }
    // The scroll bars, over the rows (which would take their presses otherwise).
    if overflow > 0.0 {
        let track = Rect::from_min_max(pos2(opt_x0, list.max.y - 5.0), pos2(rect.max.x - 4.0, list.max.y - 1.0));
        app.ui.project_hscroll = widgets::scroll_bar(ui, &mut app.auto, "project.hscroll", track, hscroll, overflow, &t);
    }
    if max_scroll > 0.0 {
        let track = Rect::from_min_max(pos2(rect.max.x - 6.0, list.min.y + 1.0), pos2(rect.max.x - 2.0, list.max.y - 7.0));
        app.ui.project_scroll = widgets::scroll_bar(ui, &mut app.auto, "project.vscroll", track, scroll, max_scroll, &t);
    }
    // Dropping a dragged item on the list: into the folder under the pointer (or the folder of
    // the item under it), or the project root below the rows.
    if let Some(iid) = dragging
        && hover_y.is_some()
    {
        if drop_row.is_none() {
            lp.rect_stroke(list.shrink(1.0), 0.0, Stroke::new(1.0, t.accent.gamma_multiply(0.6)), egui::StrokeKind::Inside);
        }
        if ctx.input(|i| i.pointer.any_released()) && drop_row != Some(ItemId(iid)) {
            let folder = drop_folder(&app.session.project, drop_row);
            let cur = app.session.project.item(ItemId(iid)).and_then(|i| i.parent);
            if folder != cur {
                let moving: Vec<u64> = if app.session.state.project_selection.contains(&ItemId(iid)) {
                    app.session.state.project_selection.iter().map(|i| i.0).collect()
                } else {
                    vec![iid]
                };
                actions.push(("project.move".into(), json!({"items": moving, "folder": folder.map(|f| f.0)})));
                if let Some(f) = folder {
                    app.ui.project_open_folders.insert(f.0);
                }
            }
            egui::DragAndDrop::clear_payload(&ctx);
        }
    }
    // Files dragged from the Media Browser: import.
    if let Some(DragPayload::Files(paths)) = egui::DragAndDrop::payload::<DragPayload>(&ctx).as_deref()
        && hover_y.is_some()
    {
        lp.rect_stroke(list.shrink(1.0), 0.0, Stroke::new(1.0, t.accent.gamma_multiply(0.6)), egui::StrokeKind::Inside);
        if ctx.input(|i| i.pointer.any_released()) {
            actions.push(("mediaBrowser.import".into(), json!({"paths": paths})));
            egui::DragAndDrop::clear_payload(&ctx);
        }
    }
    // Drag ghost.
    if let Some(DragPayload::Item(iid)) = egui::DragAndDrop::payload::<DragPayload>(&ctx).as_deref()
        && let Some(pos) = ctx.pointer_hover_pos()
        && let Some(it) = app.session.project.item(ItemId(*iid))
    {
        let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Tooltip, egui::Id::new("dnd-ghost")));
        let g = painter.layout_no_wrap(it.name.clone(), Tokens::ui(12.0), t.text);
        let r = Rect::from_min_size(pos + vec2(12.0, 8.0), g.size() + vec2(16.0, 8.0));
        painter.rect_filled(r, 4.0, Color32::from_rgba_premultiplied(40, 40, 40, 230));
        painter.galley(r.min + vec2(8.0, 4.0), g, t.text);
    }
    // Footer.
    let foot = Rect::from_min_max(pos2(rect.min.x, rect.max.y - footer_h), rect.max);
    p.line_segment([foot.left_top(), foot.right_top()], Stroke::new(1.0, t.separator));
    let mut x = foot.min.x + 8.0;
    for (icon, id, tip) in [
        (Icon::Gear, "interpret", "Interpret Footage"),
        (Icon::NewFolder, "newFolder", "Create a new Folder"),
        (Icon::NewComp, "newComp", "Create a new Composition"),
    ] {
        let r = Rect::from_min_size(pos2(x, foot.min.y + 3.0), vec2(22.0, 22.0));
        let resp = widgets::icon_button(ui, r, icon, false, &t, egui::Id::new(("pfoot", id))).on_hover_text(tip);
        app.auto.add(&format!("project.{id}"), r, tip);
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, crate::i18n::tr(tip)));
        if resp.clicked() {
            let footage =
                app.session.state.project_selection.first().and_then(|i| app.session.project.item(*i)).is_some_and(|i| matches!(i.kind, ItemKind::Footage(_)));
            match id {
                "newComp" => crate::panels::dialogs::open_new_comp(app),
                "newFolder" => actions.push(("project.newFolder".into(), json!({}))),
                _ if footage => actions.push(("file.interpretFootage".into(), json!({}))),
                _ => app.ui.status = "Interpret Footage: select a footage item".into(),
            }
        }
        // Items dropped on Create a new Composition: New Comp from Selection with them (After
        // Effects; several items ask how in its dialog).
        if id == "newComp"
            && let Some(DragPayload::Item(iid)) = egui::DragAndDrop::payload::<DragPayload>(&ctx).as_deref()
            && ui.rect_contains_pointer(r)
        {
            p.rect_stroke(r, 4.0, Stroke::new(2.0, t.accent), egui::StrokeKind::Inside);
            if ctx.input(|i| i.pointer.any_released()) {
                if !app.session.state.project_selection.contains(&ItemId(*iid)) {
                    app.session.state.project_selection = vec![ItemId(*iid)];
                }
                actions.push(("file.newCompFromSelection".into(), json!({})));
                egui::DragAndDrop::clear_payload(&ctx);
            }
        }
        x += 26.0;
    }
    let depth = app.session.project.settings.bit_depth.label();
    let dr = Rect::from_min_size(pos2(x + 4.0, foot.min.y + 5.0), vec2(48.0, 18.0));
    let dresp = ui.interact(dr, egui::Id::new("bpc"), Sense::click());
    p.text(dr.center(), Align2::CENTER_CENTER, depth, Tokens::ui(11.5), if dresp.hovered() { t.text } else { t.text_dim });
    app.auto.add("project.bitDepth", dr, depth);
    dresp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, crate::i18n::tr_args("Project color depth: {}", &[&depth])));
    // Click cycles 8/16/32 bpc; Alt-click opens Project Settings (as in After Effects).
    if dresp.clicked() {
        if ui.input(|i| i.modifiers.alt) {
            actions.push(("file.projectSettings".into(), json!({})));
        } else {
            actions.push(("file.cycleBitDepth".into(), json!({})));
        }
    }
    dresp.on_hover_text(crate::i18n::tr("Project color depth: click to cycle 8/16/32 bpc, Alt-click for Project Settings"));
    let tr = Rect::from_min_size(pos2(foot.max.x - 30.0, foot.min.y + 3.0), vec2(22.0, 22.0));
    let trash = widgets::icon_button(ui, tr, Icon::Trash, false, &t, egui::Id::new("ptrash")).on_hover_text(crate::i18n::tr("Delete selected project items"));
    trash.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, crate::i18n::tr("Delete selected project items")));
    if trash.clicked() && !app.session.state.project_selection.is_empty() {
        actions.push(("project.delete".into(), json!({})));
    }
    app.auto.add("project.delete", tr, "Delete");
    if empty.double_clicked() {
        actions.push(("file.import".into(), json!({})));
    }
    crate::widgets::context_menu(&empty, |ui| {
        for (key, label, command) in
            [("newComp", "New Composition…", "comp.new"), ("newFolder", "New Folder", "project.newFolder"), ("import", "Import File…", "file.import")]
        {
            let response = ui.button(label);
            app.auto.add(&format!("project.context.{key}"), response.rect, label);
            if response.clicked() {
                actions.push((command.into(), json!({})));
                ui.close();
            }
        }
    });
    // Selection box over the rows it touches (#203); Shift or Cmd/Ctrl adds to the selection.
    let adding = ui.input(|i| i.modifiers.shift || i.modifiers.command);
    if empty.clicked() && !adding {
        app.session.state.project_selection.clear();
    }
    let box_id = egui::Id::new("proj-marquee");
    if let (Some(start), Some(end)) = (ctx.input(|i| i.pointer.press_origin()), empty.interact_pointer_pos())
        && empty.dragged()
    {
        let b = Rect::from_two_pos(start, end).intersect(list);
        lp.rect_filled(b, 0.0, t.accent.gamma_multiply(0.12));
        lp.rect_stroke(b, 0.0, Stroke::new(1.0, t.accent), egui::StrokeKind::Inside);
        ctx.data_mut(|d| d.insert_temp(box_id, b));
    }
    if empty.drag_stopped()
        && let Some(b) = ctx.data(|d| d.get_temp::<Rect>(box_id))
    {
        ctx.data_mut(|d| d.remove::<Rect>(box_id));
        // Rows sit at fixed heights from the top of the scrolled list.
        let top = list.min.y - app.ui.project_scroll;
        let picked = rows.iter().enumerate().filter(|(i, _)| {
            let y = top + *i as f32 * ROW_H;
            y < b.max.y && y + ROW_H > b.min.y
        });
        let picked: Vec<ItemId> = picked.map(|(_, (id, _))| *id).collect();
        let sel = &mut app.session.state.project_selection;
        if !adding {
            sel.clear();
        }
        for id in picked {
            if !sel.contains(&id) {
                sel.push(id);
            }
        }
    }
    for (id, params) in actions {
        if let Err(e) = crate::menus::invoke(app, &ctx, &id, params) {
            app.ui.status = e;
        }
    }
}

/// Items a row's menu acts on: the selection when the row is selected, else the row's item.
fn targets(app: &EffectcraftApp, id: ItemId, selected: bool) -> Vec<u64> {
    if selected { app.session.state.project_selection.iter().map(|i| i.0).collect() } else { vec![id.0] }
}

/// `project.setLabel`'s index of a label.
fn label_index(l: Label) -> usize {
    Label::ALL.iter().position(|x| *x == l).unwrap_or(0)
}

/// The column header's context menu: show or hide the optional columns.
fn column_menu(app: &mut EffectcraftApp, resp: &egui::Response) {
    crate::widgets::context_menu(resp, |ui| {
        ui.label(egui::RichText::new(crate::i18n::tr("Columns")).weak());
        for (key, label, _) in COLUMNS {
            let mut on = app.ui.project_columns.iter().any(|c| c == key);
            if ui.checkbox(&mut on, crate::i18n::tr(label)).changed() {
                set_column(&mut app.ui.project_columns, key, on);
            }
        }
    });
}

/// Show or hide a column, keeping the canonical column order.
pub fn set_column(cols: &mut Vec<String>, key: &str, on: bool) {
    cols.retain(|c| c != key);
    if on {
        cols.push(key.to_string());
        cols.sort_by_key(|c| COLUMNS.iter().position(|x| x.0 == c).unwrap_or(usize::MAX));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use effectcraft_engine::project::{Footage, Project};

    struct SizeFiles(std::path::PathBuf);

    impl SizeFiles {
        fn new() -> Self {
            static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
            let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!("effectcraft-size-{}-{n}", std::process::id()));
            std::fs::create_dir(&path).unwrap();
            Self(path)
        }

        fn add(&self, project: &mut Project, name: &str, bytes: Option<u64>) {
            let path = self.0.join(name);
            if let Some(bytes) = bytes {
                std::fs::File::create(&path).unwrap().set_len(bytes).unwrap();
            }
            project.add_item(name, Label::Aqua, None, ItemKind::Footage(Footage { path: path.to_string_lossy().into_owned(), ..Default::default() }));
        }
    }

    impl Drop for SizeFiles {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn size_keys_are_sampled_once_with_stable_ties() {
        let mut project = Project::default();
        for name in ["Zeta", "alpha", "ALPHA", "Missing", "Zero"] {
            project.add_item(name, Label::Aqua, None, ItemKind::Folder);
        }
        for (desc, expected) in [(false, ["Missing", "Zero", "alpha", "ALPHA", "Zeta"]), (true, ["Zeta", "alpha", "ALPHA", "Zero", "Missing"])] {
            let mut reads = std::collections::BTreeMap::new();
            let mut rows = project.children(None);
            sort_by_size(&mut rows, desc, |item| {
                *reads.entry(item.id).or_insert(0) += 1;
                match item.name.as_str() {
                    "Zeta" => Some(30),
                    "alpha" | "ALPHA" => Some(10),
                    "Zero" => Some(0),
                    _ => None,
                }
            });
            assert_eq!(rows.iter().map(|item| item.name.as_str()).collect::<Vec<_>>(), expected);
            assert_eq!(reads.len(), 5);
            assert!(reads.values().all(|&count| count == 1));
        }
    }

    #[test]
    fn size_sort_reads_files_again_after_they_change() {
        let files = SizeFiles::new();
        let mut project = Project::default();
        project.add_item("Folder", Label::Aqua, None, ItemKind::Folder);
        files.add(&mut project, "Missing", None);
        files.add(&mut project, "Zero", Some(0));
        files.add(&mut project, "Small", Some(10));
        files.add(&mut project, "Large", Some(20));
        let mut rows = project.children(None);
        sort_items(&mut rows, "size", false);
        assert_eq!(rows.iter().map(|item| item.name.as_str()).collect::<Vec<_>>(), ["Folder", "Missing", "Zero", "Small", "Large"]);
        sort_items(&mut rows, "size", true);
        assert_eq!(rows.iter().map(|item| item.name.as_str()).collect::<Vec<_>>(), ["Large", "Small", "Zero", "Missing", "Folder"]);
        std::fs::OpenOptions::new().write(true).open(files.0.join("Small")).unwrap().set_len(30).unwrap();
        sort_items(&mut rows, "size", false);
        assert_eq!(rows.iter().map(|item| item.name.as_str()).collect::<Vec<_>>(), ["Folder", "Missing", "Zero", "Large", "Small"]);
        std::fs::remove_file(files.0.join("Large")).unwrap();
        sort_items(&mut rows, "size", false);
        assert_eq!(rows.iter().map(|item| item.name.as_str()).collect::<Vec<_>>(), ["Folder", "Large", "Missing", "Zero", "Small"]);
    }

    #[test]
    fn size_sort_accepts_empty_and_singleton_lists() {
        let mut project = Project::default();
        let id = project.add_item("Only", Label::Aqua, None, ItemKind::Folder);
        for desc in [false, true] {
            sort_items(&mut [], "size", desc);
            let mut rows = project.children(None);
            sort_items(&mut rows, "size", desc);
            assert_eq!(rows.iter().map(|item| item.id).collect::<Vec<_>>(), [id]);
        }
    }

    #[test]
    #[ignore = "filesystem benchmark; run with --ignored --nocapture"]
    fn size_sort_benchmark() {
        let files = SizeFiles::new();
        let mut project = Project::default();
        for i in 0..1000 {
            files.add(&mut project, &format!("Clip {i:04}"), Some((i * 137) % 1000 + 1));
        }
        let original = project.children(None);
        for item in &original {
            assert!(file_size(item).is_some());
        }
        let mut before_times = Vec::new();
        let mut after_times = Vec::new();
        let mut before_reads = 0;
        let mut after_reads = 0;
        for _ in 0..5 {
            let mut before = original.clone();
            before_reads = 0;
            let start = std::time::Instant::now();
            before.sort_by(|a, b| {
                before_reads += 2;
                file_size(a).cmp(&file_size(b)).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            });
            before_times.push(start.elapsed());
            let mut after = original.clone();
            after_reads = 0;
            let start = std::time::Instant::now();
            sort_by_size(&mut after, false, |item| {
                after_reads += 1;
                file_size(item)
            });
            after_times.push(start.elapsed());
            assert_eq!(before.iter().map(|item| item.id).collect::<Vec<_>>(), after.iter().map(|item| item.id).collect::<Vec<_>>());
            assert_eq!(after_reads, 1000);
            assert!(before_reads > after_reads);
        }
        before_times.sort();
        after_times.sort();
        println!(
            "1,000 footage files; warmed metadata; five paired sorts\nmetadata reads/sort: {before_reads} -> {after_reads}\nmedian sort time: {:?} -> {:?}",
            before_times[2], after_times[2]
        );
    }

    #[test]
    fn ae_duration_and_size_formats() {
        assert_eq!(fmt_dur(10.0, 29.97), "0:00:10:00");
        assert_eq!(fmt_dur(3725.48, 25.0), "1:02:05:12");
        assert_eq!(fmt_size(1000), "1 KB");
        assert_eq!(fmt_size(5 * 1024 * 1024 + 300_000), "5.3 MB");
    }

    #[test]
    fn folders_sort_with_items_by_name() {
        let mut p = effectcraft_engine::project::Project::default();
        let l = effectcraft_engine::color::Label::Yellow;
        p.add_item("Solids", l, None, ItemKind::Folder);
        p.add_item("Precomps", l, None, ItemKind::Folder);
        p.add_item("EffectCraft Intro", l, None, ItemKind::Folder);
        let mut kids = p.children(None);
        sort_items(&mut kids, "name", false);
        let names: Vec<&str> = kids.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names, ["EffectCraft Intro", "Precomps", "Solids"]);
        sort_items(&mut kids, "name", true);
        assert_eq!(kids[0].name, "Solids");
    }

    #[test]
    fn drop_targets_and_column_toggles() {
        let mut p = effectcraft_engine::project::Project::default();
        let l = effectcraft_engine::color::Label::Yellow;
        let f = p.add_item("F", l, None, ItemKind::Folder);
        let inner = p.add_item("I", l, Some(f), ItemKind::Folder);
        assert_eq!(drop_folder(&p, Some(f)), Some(f));
        assert_eq!(drop_folder(&p, Some(inner)), Some(inner));
        assert_eq!(drop_folder(&p, None), None);
        let mut cols = crate::state::default_project_columns();
        set_column(&mut cols, "comment", true);
        set_column(&mut cols, "path", true);
        set_column(&mut cols, "size", false);
        assert_eq!(cols, ["type", "duration", "fps", "path", "comment"]);
    }
}
