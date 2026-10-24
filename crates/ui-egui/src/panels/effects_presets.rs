//! Effects & Presets: search (matches highlighted), the contents menu (colour-depth / GPU
//! filters, animation presets on/off, categories or alphabetical, refresh), Favorites (star an
//! effect), Recently Used, "* Animation Presets" (EffectCraft's own text animation and transition
//! presets and the user's `.ecpreset` files) and the effect categories. Double-click applies to the
//! selected layers; dragging an effect or an animation preset onto a layer (timeline, viewer,
//! Effect Controls) applies it there.

use crate::i18n::tr;
use egui::{Align2, Color32, Rect, Sense, Stroke, pos2, vec2};
use serde_json::json;
use std::sync::Arc;

use crate::a11y;
use crate::icons::{self, Icon};
use crate::panels::{DragPayload, PresetRef};
use crate::theme::Tokens;
use crate::{EffectcraftApp, widgets};

const ROW_H: f32 = 21.0;
const RECENT_MAX: usize = 8;

/// A user animation preset file.
#[derive(Clone, Debug, PartialEq)]
pub struct PresetFile {
    pub name: String,
    pub path: String,
}

/// Where user presets live: `$EC_PRESETS_DIR`, else `~/Documents/EffectCraft/Presets`.
pub fn presets_dir() -> Option<std::path::PathBuf> {
    if let Ok(d) = std::env::var("EC_PRESETS_DIR") {
        return Some(d.into());
    }
    let home = std::env::var("HOME").or_else(|_| std::env::var("USERPROFILE")).ok()?;
    Some(std::path::Path::new(&home).join("Documents").join("EffectCraft").join("Presets"))
}

/// The `.ecpreset` files in `dir`, sorted by name.
pub fn scan_presets(dir: &std::path::Path) -> Vec<PresetFile> {
    let Ok(rd) = std::fs::read_dir(dir) else { return vec![] };
    let mut v: Vec<PresetFile> = rd
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("ecpreset")))
        .filter_map(|p| Some(PresetFile { name: p.file_stem()?.to_string_lossy().to_string(), path: p.to_string_lossy().to_string() }))
        .collect();
    v.sort_by_key(|p| p.name.to_lowercase());
    v
}

/// Byte range of the (case-insensitive) search match in `name`, if any.
pub fn match_range(name: &str, query: &str) -> Option<std::ops::Range<usize>> {
    if query.is_empty() {
        return None;
    }
    let lower = name.to_lowercase();
    let i = lower.find(&query.to_lowercase())?;
    let end = i + query.len();
    // Lower-casing can change byte lengths outside ASCII; only highlight clean boundaries.
    (lower.len() == name.len() && name.is_char_boundary(i) && name.is_char_boundary(end)).then_some(i..end)
}

/// Remember `id` as the most recently used effect.
pub fn push_recent(recent: &mut Vec<String>, id: &str) {
    recent.retain(|r| r != id);
    recent.insert(0, id.to_string());
    recent.truncate(RECENT_MAX);
}

/// Does the contents-menu depth filter (`all`, `32`, `gpu`) keep effect `e`?
pub fn depth_keeps(depth: &str, e: &effectcraft_engine::effects::EffectSpec) -> bool {
    match depth {
        "32" => e.float,
        "gpu" => e.gpu,
        _ => true,
    }
}

fn filled_star(p: &egui::Painter, c: egui::Pos2, r: f32, fill: Option<Color32>, stroke: Color32) {
    let pts: Vec<egui::Pos2> = (0..10)
        .map(|i| {
            let a = std::f32::consts::PI * i as f32 / 5.0 - std::f32::consts::FRAC_PI_2;
            let rr = if i % 2 == 0 { r } else { r * 0.45 };
            c + vec2(a.cos(), a.sin()) * rr
        })
        .collect();
    if let Some(f) = fill {
        // The star is concave: fill it as five triangles around the centre plus the pentagon.
        for i in 0..5 {
            let tri = vec![pts[2 * i], pts[(2 * i + 1) % 10], pts[(2 * i + 9) % 10]];
            p.add(egui::Shape::convex_polygon(tri, f, Stroke::NONE));
        }
        let inner: Vec<egui::Pos2> = (0..5).map(|i| pts[2 * i + 1]).collect();
        p.add(egui::Shape::convex_polygon(inner, f, Stroke::NONE));
    }
    p.add(egui::Shape::closed_line(pts, Stroke::new(1.0, stroke)));
}

/// Text with the search match highlighted.
fn highlighted(p: &egui::Painter, pos: egui::Pos2, name: &str, query: &str, t: &Tokens) {
    if let Some(m) = match_range(name, query) {
        let pre = p.layout_no_wrap(name[..m.start].to_string(), Tokens::ui(12.0), t.text).size().x;
        let hit = p.layout_no_wrap(name[m.clone()].to_string(), Tokens::ui(12.0), t.text).size().x;
        let r = Rect::from_min_size(pos2(pos.x + pre - 1.0, pos.y - 8.0), vec2(hit + 2.0, 16.0));
        p.rect_filled(r, 2.0, t.accent.gamma_multiply(0.45));
    }
    p.text(pos, Align2::LEFT_CENTER, name, Tokens::ui(12.0), t.text);
}

#[derive(Clone)]
enum Apply {
    Effect(String),
    /// A text, built-in (Transitions…) or user file animation preset.
    Preset(PresetRef),
}

/// One row of the list as drawn this pass, including the rows scrolled out of view: what the
/// arrow keys, Home / End and Enter work on (the rows are painted by hand, so this is the list).
struct NavRow {
    id: egui::Id,
    /// The favourite star's widget, for an effect row.
    star: Option<egui::Id>,
    parent: Option<egui::Id>,
    /// Where the row starts in the scrolled content.
    top: f32,
    /// A folder: (its key, open, forced open by a search).
    folder: Option<(String, bool, bool)>,
    /// What Enter applies, for an effect or preset row.
    apply: Option<Apply>,
}

/// The list's keyboard state: the rows drawn, and the row the keys work from (the "cursor":
/// selected, and the one Tab stop of the list).
struct Nav {
    rows: Vec<NavRow>,
    /// The open folders on the way down to the row being drawn, by level.
    stack: Vec<egui::Id>,
    cursor: Option<egui::Id>,
    /// The scroll offset and the list's top, to place rows in the content.
    offset: f32,
    list_top: f32,
}

impl Nav {
    /// The row `id` is the list's Tab stop: the cursor row, or the first row while there is none.
    fn tab_stop(&self, id: egui::Id) -> bool {
        match self.cursor {
            Some(c) => c == id,
            None => self.rows.is_empty(),
        }
    }

    /// Record the row drawn at `y`; returns whether it is the cursor row.
    fn push(&mut self, id: egui::Id, y: f32, level: usize, folder: Option<(String, bool, bool)>, apply: Option<Apply>) -> bool {
        let parent = level.checked_sub(2).and_then(|i| self.stack.get(i)).copied();
        if folder.is_some() {
            self.stack.truncate(level.saturating_sub(1));
            self.stack.push(id);
        }
        self.rows.push(NavRow { id, star: None, parent, top: y - self.list_top + self.offset, folder, apply });
        self.cursor == Some(id)
    }
}

fn cursor_id() -> egui::Id {
    egui::Id::new("fx-cursor")
}

/// The accessible name of a folder: what the row shows, in the interface language.
fn folder_name(label: &str) -> &str {
    tr(label)
}

/// A folder row; returns whether it is open.
#[allow(clippy::too_many_arguments)]
fn folder_row(
    app: &mut EffectcraftApp,
    ui: &mut egui::Ui,
    lp: &egui::Painter,
    list: Rect,
    y: &mut f32,
    nav: &mut Nav,
    key: &str,
    label: &str,
    level: usize,
    indent: f32,
    forced_open: bool,
    icon: Option<Icon>,
) -> bool {
    let t = app.tokens;
    let open = forced_open || app.ui.effects_open.contains(key);
    let r = Rect::from_min_size(pos2(list.min.x, *y), vec2(list.width(), ROW_H));
    let id = egui::Id::new(("fxcat", key));
    let tab = nav.tab_stop(id);
    let selected = nav.push(id, *y, level, Some((key.to_string(), open, forced_open)), None);
    *y += ROW_H;
    if r.max.y < list.min.y || r.min.y > list.max.y {
        return open;
    }
    let shown = folder_name(label);
    let resp = ui.interact(r.intersect(list), id, if tab { Sense::click() } else { Sense::CLICK });
    if selected {
        lp.rect_filled(r, 0.0, t.row_selected);
    } else if resp.hovered() {
        lp.rect_filled(r, 0.0, t.hover);
    }
    let tw = Rect::from_center_size(pos2(r.min.x + 12.0 + indent, r.center().y), vec2(12.0, 12.0));
    icons::paint(lp, tw, if open { Icon::ChevronDown } else { Icon::ChevronRight }, t.text_dim);
    let ic = Rect::from_center_size(pos2(r.min.x + 28.0 + indent, r.center().y), vec2(13.0, 13.0));
    match icon {
        Some(Icon::Star) => filled_star(lp, ic.center(), 6.0, Some(Color32::from_rgb(0xe8, 0xc0, 0x40)), Color32::from_rgb(0xe8, 0xc0, 0x40)),
        Some(i) => icons::paint(lp, ic, i, t.text_dim),
        None => icons::paint(lp, ic, Icon::Folder, Color32::from_rgb(0xc8, 0xb0, 0x58)),
    }
    lp.text(pos2(r.min.x + 40.0 + indent, r.center().y), Align2::LEFT_CENTER, shown, Tokens::ui(12.0), t.text);
    app.auto.add(&format!("effects.category.{key}"), r, label);
    a11y::tree_item(&resp, shown, selected, level, Some(open));
    row_focus(app, ui, lp, &resp, r, id);
    if resp.clicked() {
        nav_click(ui.ctx(), app, id);
        if !forced_open {
            if open {
                app.ui.effects_open.remove(key);
            } else {
                app.ui.effects_open.insert(key.to_string());
            }
        }
    }
    open
}

/// What every row does with the keyboard focus: the focus ring, the panel taking the focus, the
/// list owning its keys, and moving the focus here when the arrow keys asked for it.
fn row_focus(app: &mut EffectcraftApp, ui: &egui::Ui, lp: &egui::Painter, resp: &egui::Response, r: Rect, id: egui::Id) {
    let ctx = ui.ctx();
    if a11y::take_pending_focus(ctx, "fx", id) {
        resp.request_focus();
    }
    if resp.gained_focus() {
        app.ui.focused = crate::dock::PanelKind::EffectsPresets;
    }
    if resp.has_focus() {
        lp.rect_stroke(r.shrink(0.5), 0.0, Stroke::new(1.0, app.tokens.accent), egui::StrokeKind::Inside);
    }
}

/// A click on a row moves the cursor there and gives the panel the keyboard.
fn nav_click(ctx: &egui::Context, app: &mut EffectcraftApp, id: egui::Id) {
    ctx.data_mut(|d| d.insert_temp(cursor_id(), id));
    app.ui.focused = crate::dock::PanelKind::EffectsPresets;
}

/// An effect row (fx icon, highlighted name, favourite star, 32 bpc badge).
#[allow(clippy::too_many_arguments)]
fn effect_row(
    app: &mut EffectcraftApp,
    ui: &mut egui::Ui,
    lp: &egui::Painter,
    list: Rect,
    y: &mut f32,
    nav: &mut Nav,
    e: &'static effectcraft_engine::effects::EffectSpec,
    level: usize,
    indent: f32,
    query: &str,
    auto_prefix: &str,
    apply: &mut Option<Apply>,
) {
    let t = app.tokens;
    let ctx = ui.ctx().clone();
    let r = Rect::from_min_size(pos2(list.min.x, *y), vec2(list.width(), ROW_H));
    let id = egui::Id::new((auto_prefix, e.id));
    let star_id = egui::Id::new((auto_prefix, e.id, "star"));
    let tab = nav.tab_stop(id);
    let selected = nav.push(id, *y, level, None, Some(Apply::Effect(e.id.to_string())));
    if let Some(row) = nav.rows.last_mut() {
        row.star = Some(star_id);
    }
    *y += ROW_H;
    if r.max.y < list.min.y || r.min.y > list.max.y {
        return;
    }
    let star = Rect::from_center_size(pos2(r.max.x - 38.0, r.center().y), vec2(16.0, 16.0));
    let resp = ui.interact(r.intersect(list), id, if tab { Sense::click_and_drag() } else { Sense::CLICK | Sense::DRAG });
    if selected {
        lp.rect_filled(r, 0.0, t.row_selected);
    } else if resp.hovered() {
        lp.rect_filled(r, 0.0, t.hover);
    }
    icons::paint(lp, Rect::from_center_size(pos2(r.min.x + 44.0 + indent, r.center().y), vec2(12.0, 12.0)), Icon::Fx, t.text_dim);
    // The row draws the effect's name in the interface language; the automation label keeps the
    // registry's English name, which is what scripts and the control channel address (#821).
    let shown = crate::i18n::effects::name(e.id, e.name);
    highlighted(lp, pos2(r.min.x + 56.0 + indent, r.center().y), &shown, query, &t);
    if e.float {
        lp.text(pos2(r.max.x - 10.0, r.center().y), Align2::RIGHT_CENTER, "32", Tokens::ui(9.5), t.text_faint);
    }
    app.auto.add(&format!("{auto_prefix}.{}", e.id), r, e.name);
    a11y::tree_item(&resp, &shown, selected, level, None);
    row_focus(app, ui, lp, &resp, r, id);
    // Favourite star (always shown when starred, else on hover or with the keyboard focus).
    let fav = app.session.prefs.is_effect_favorite(e.id);
    let sresp = ui.interact(star.intersect(list), star_id, if tab { Sense::click() } else { Sense::CLICK });
    if fav || resp.hovered() || sresp.hovered() || sresp.has_focus() {
        let gold = Color32::from_rgb(0xe8, 0xc0, 0x40);
        filled_star(lp, star.center(), 6.0, fav.then_some(gold), if fav || sresp.hovered() { gold } else { t.text_faint });
    }
    app.auto.add(&format!("{auto_prefix}.{}.star", e.id), star, if fav { "Remove from Favorites" } else { "Add to Favorites" });
    a11y::button(&sresp, &format!("{}: {shown}", tr(if fav { "Remove from favorites" } else { "Add to favorites" })), true);
    if sresp.gained_focus() {
        app.ui.focused = crate::dock::PanelKind::EffectsPresets;
    }
    if sresp.has_focus() {
        widgets::focus_outline(ui, &sresp, &t);
    }
    if sresp.clicked() {
        // Kept in the settings, so the stars are there next time (#1692).
        app.session.prefs.toggle_effect_favorite(e.id);
        app.session.save_prefs();
        return;
    }
    if resp.clicked() {
        nav_click(&ctx, app, id);
    }
    if resp.double_clicked() {
        *apply = Some(Apply::Effect(e.id.to_string()));
    }
    if resp.drag_started() {
        egui::DragAndDrop::set_payload(&ctx, DragPayload::Effect(e.id.to_string()));
    }
}

/// A preset row: double-click applies to the selected layers; drag it onto a layer to apply it there.
#[allow(clippy::too_many_arguments)]
fn preset_row(
    app: &mut EffectcraftApp,
    ui: &mut egui::Ui,
    lp: &egui::Painter,
    list: Rect,
    y: &mut f32,
    nav: &mut Nav,
    name: &str,
    auto_id: &str,
    query: &str,
    preset: PresetRef,
    apply: &mut Option<Apply>,
) {
    let t = app.tokens;
    let r = Rect::from_min_size(pos2(list.min.x, *y), vec2(list.width(), ROW_H));
    let id = egui::Id::new(("fxpreset", auto_id));
    let tab = nav.tab_stop(id);
    let selected = nav.push(id, *y, 3, None, Some(Apply::Preset(preset.clone())));
    *y += ROW_H;
    if r.max.y < list.min.y || r.min.y > list.max.y {
        return;
    }
    let resp = ui.interact(r.intersect(list), id, if tab { Sense::click_and_drag() } else { Sense::CLICK | Sense::DRAG });
    if selected {
        lp.rect_filled(r, 0.0, t.row_selected);
    } else if resp.hovered() {
        lp.rect_filled(r, 0.0, t.hover);
    }
    icons::paint(lp, Rect::from_center_size(pos2(r.min.x + 58.0, r.center().y), vec2(12.0, 12.0)), Icon::Keyframe, t.text_dim);
    highlighted(lp, pos2(r.min.x + 70.0, r.center().y), name, query, &t);
    app.auto.add(auto_id, r, name);
    a11y::tree_item(&resp, name, selected, 3, None);
    row_focus(app, ui, lp, &resp, r, id);
    if resp.clicked() {
        nav_click(ui.ctx(), app, id);
    }
    if resp.drag_started() {
        egui::DragAndDrop::set_payload(ui.ctx(), DragPayload::Preset { preset: preset.clone(), name: name.to_string() });
    }
    if resp.double_clicked() {
        *apply = Some(Apply::Preset(preset));
    }
}

/// The list's keys. With a row focused (Tab, a screen reader): Up / Down move between rows,
/// Home / End go to the first / last, Page Up / Page Down a page, Right opens a folder (then steps
/// into it) and Left closes it (then steps out to it), Enter and Space apply the row or open /
/// close the folder. With the mouse's last click in the panel and no widget focused, the arrows
/// and Enter work the same, so the list is operable without a mouse either way. The focus follows
/// the cursor, so a screen reader reads each row as it is reached. Returns what to apply.
fn list_keys(app: &mut EffectcraftApp, ctx: &egui::Context, nav: &Nav, list: Rect) -> Option<Apply> {
    use egui::Key;
    // A cursor whose row is gone (a search hid it): the first row is the Tab stop again.
    if let Some(c) = nav.cursor
        && !nav.rows.iter().any(|r| r.id == c)
    {
        ctx.data_mut(|d| d.remove::<egui::Id>(cursor_id()));
        ctx.request_repaint();
    }
    let focused = ctx.memory(|m| m.focused());
    let on_row = focused.is_some_and(|f| nav.rows.iter().any(|r| r.id == f || r.star == Some(f)));
    // Enter / Space on the star are the star button's (it toggles the favourite), not the row's.
    let on_star = focused.is_some_and(|f| nav.rows.iter().any(|r| r.star == Some(f)));
    let by_mouse = focused.is_none() && app.ui.focused == crate::dock::PanelKind::EffectsPresets;
    if (!on_row && !by_mouse) || app.dialog.is_some() || widgets::any_menu_open(ctx) {
        return None;
    }
    let keys: Vec<Key> = ctx.input(|i| {
        i.events
            .iter()
            .filter_map(|e| match e {
                egui::Event::Key { key, pressed: true, modifiers, .. } if !modifiers.any() => Some(*key),
                _ => None,
            })
            .collect()
    });
    let page = ((list.height() / ROW_H).floor() as usize).saturating_sub(1).max(1);
    // The row with the keyboard focus is where the keys start (the first Tab lands on a row
    // before anything is selected); otherwise the cursor.
    let focus_row = focused.and_then(|f| nav.rows.iter().find(|r| r.id == f || r.star == Some(f))).map(|r| r.id);
    let mut cursor = focus_row.or_else(|| nav.cursor.filter(|c| nav.rows.iter().any(|r| r.id == *c)));
    let mut apply = None;
    let mut moved = false;
    for key in keys {
        let at = cursor.and_then(|c| nav.rows.iter().position(|r| r.id == c));
        let last = nav.rows.len().checked_sub(1)?;
        let to = match key {
            Key::ArrowDown => Some(at.map_or(0, |a| a.saturating_add(1).min(last))),
            Key::ArrowUp => Some(at.map_or(last, |a| a.saturating_sub(1))),
            Key::Home if on_row => Some(0),
            Key::End if on_row => Some(last),
            Key::PageDown if on_row => Some(at.map_or(0, |a| a.saturating_add(page).min(last))),
            Key::PageUp if on_row => Some(at.map_or(last, |a| a.saturating_sub(page))),
            Key::ArrowRight | Key::ArrowLeft | Key::Enter | Key::Space if at.is_some() => {
                let row = at.and_then(|a| nav.rows.get(a))?;
                let toggle = |app: &mut EffectcraftApp, open: bool| {
                    if let Some((k, _, forced)) = &row.folder
                        && !forced
                    {
                        if open {
                            app.ui.effects_open.insert(k.clone());
                        } else {
                            app.ui.effects_open.remove(k);
                        }
                    }
                };
                match (key, &row.folder) {
                    (Key::ArrowRight, Some((_, false, _))) => {
                        toggle(app, true);
                        None
                    }
                    (Key::ArrowRight, Some((_, true, _))) => at.map(|a| a.saturating_add(1).min(last)),
                    (Key::ArrowLeft, Some((_, true, false))) => {
                        toggle(app, false);
                        None
                    }
                    (Key::ArrowLeft, _) => row.parent.and_then(|p| nav.rows.iter().position(|r| r.id == p)),
                    (Key::Enter, Some((_, open, _))) | (Key::Space, Some((_, open, _))) if on_row || key == Key::Enter => {
                        toggle(app, !*open);
                        None
                    }
                    (Key::Enter, None) if !on_star => {
                        apply = row.apply.clone();
                        None
                    }
                    (Key::Space, None) if on_row && !on_star => {
                        apply = row.apply.clone();
                        None
                    }
                    _ => None,
                }
            }
            _ => None,
        };
        if let Some(to) = to
            && let Some(row) = nav.rows.get(to)
        {
            cursor = Some(row.id);
            moved = true;
        }
    }
    if moved && let Some(c) = cursor {
        ctx.data_mut(|d| d.insert_temp(cursor_id(), c));
        // Scroll the row into view.
        if let Some(row) = nav.rows.iter().find(|r| r.id == c) {
            widgets::PanelScroll::reveal(ctx, egui::Id::new("fx-scroll"), row.top, row.top + ROW_H, list.height());
        }
        app.ui.focused = crate::dock::PanelKind::EffectsPresets;
        if on_row {
            a11y::focus_next_pass(ctx, "fx", c);
        }
        ctx.request_repaint();
    }
    // The arrows are the list's, not egui's spatial focus moves.
    if on_row {
        ctx.memory_mut(|m| m.move_focus(egui::FocusDirection::None));
    }
    apply
}

/// User preset files, rescanned every few seconds (and on Refresh List).
fn user_presets(ctx: &egui::Context, force: bool, scan: impl FnOnce() -> Vec<PresetFile>) -> Arc<Vec<PresetFile>> {
    let id = egui::Id::new("fx-user-presets");
    let now = ctx.input(|i| i.time);
    if !force
        && let Some((at, v)) = ctx.data(|d| d.get_temp::<(f64, Arc<Vec<PresetFile>>)>(id))
        && now - at < 5.0
    {
        return v;
    }
    let v = Arc::new(scan());
    ctx.data_mut(|d| d.insert_temp(id, (now, v.clone())));
    v
}

pub fn show(app: &mut EffectcraftApp, ui: &mut egui::Ui, rect: Rect) {
    let t = app.tokens;
    let ctx = ui.ctx().clone();
    let p = ui.painter().clone();
    // Track the last applied effect (menus, drag and drop, the control channel) as recently used.
    if let Some(last) = app.session.state.last_effect.clone()
        && app.ui.effects_recent.first() != Some(&last)
    {
        push_recent(&mut app.ui.effects_recent, &last);
    }
    let sr = Rect::from_min_size(rect.min + vec2(8.0, 8.0), vec2(rect.width() - 44.0, 24.0));
    let mut q = app.ui.effects_search.clone();
    let search_hint = tr("Search effects and presets");
    let search = widgets::search_field(ui, sr, &mut q, search_hint, &t);
    a11y::text_input(&search, search_hint, &q);
    app.ui.effects_search = q.clone();
    app.auto.add("effects.search", sr, "Search");
    // Contents menu.
    let mr = Rect::from_center_size(pos2(rect.max.x - 18.0, sr.center().y), vec2(22.0, 22.0));
    let mpop = egui::Id::new("fx-contents-menu");
    let menu_resp = widgets::icon_button(ui, mr, Icon::Hamburger, false, &t, egui::Id::new("fx-menu")).on_hover_text(tr("Effects & Presets options"));
    a11y::menu_button(&menu_resp, tr("Effects & Presets options"), widgets::popup_is_open(ui, mpop));
    if menu_resp.clicked() {
        widgets::open_popup(ui, mpop);
    }
    app.auto.add("effects.menu", mr, "Effects & Presets options");
    let view = app.ui.effects_view.clone();
    let mark = |on: bool, l: &str| if on { format!("✓ {l}") } else { format!("    {l}") };
    let copy_label = format!("    {}", tr("Copy Presets Folder Path"));
    let menu = vec![
        mark(view.depth == "all", "Show Effects for All Color Depths"),
        mark(view.depth == "32", "Show 32 bpc Effects Only"),
        mark(view.depth == "gpu", "Show GPU Accelerated Effects Only"),
        "-".into(),
        mark(view.presets, "Show Animation Presets"),
        "-".into(),
        mark(!view.alphabetical, "Categories"),
        mark(view.alphabetical, "Alphabetical"),
        "-".into(),
        "    Refresh List".into(),
        "    Clear Recently Used".into(),
        "-".into(),
        copy_label.clone(),
    ];
    let mut force_scan = false;
    let separators = menu.iter().filter(|label| *label == "-").count();
    let choice = if widgets::popup_is_open(ui, mpop) {
        widgets::popup_list(ui, mpop, mr.left_bottom() - vec2(220.0, 0.0), Rect::NOTHING, menu.len().saturating_sub(separators), separators, |ui| {
            ui.set_min_width(160.0);
            let mut chosen = None;
            for (i, label) in menu.iter().enumerate() {
                if label == "-" {
                    ui.separator();
                } else {
                    let resp = ui.selectable_label(false, label);
                    // The ✓ and the padding are the menu's drawing: say it as a state.
                    let checkable = matches!(i, 0 | 1 | 2 | 4 | 6 | 7);
                    a11y::menu_item(&resp, label.trim_start_matches('✓').trim(), checkable.then(|| label.starts_with('✓')));
                    if i == 0 {
                        widgets::focus_when_opened(ui, mpop, &resp);
                    }
                    if label == &copy_label {
                        app.auto.add("effects.copyPresetsFolderPath", resp.rect, "Copy Presets Folder Path");
                    }
                    if resp.clicked() {
                        chosen = Some(i);
                    }
                }
            }
            chosen
        })
    } else {
        None
    };
    if let Some(i) = choice {
        if widgets::by_keyboard(ui) {
            menu_resp.request_focus();
        }
        let v = &mut app.ui.effects_view;
        match i {
            i if menu.get(i) == Some(&copy_label) => {
                if let Err(e) = crate::menus::invoke(app, &ctx, "effects.copyPresetsFolderPath", json!({})) {
                    app.ui.status = e;
                }
            }
            0 => v.depth = "all".into(),
            1 => v.depth = "32".into(),
            2 => v.depth = "gpu".into(),
            4 => v.presets = !v.presets,
            6 => v.alphabetical = false,
            7 => v.alphabetical = true,
            9 => force_scan = true,
            10 => app.ui.effects_recent.clear(),
            _ => {}
        }
    }
    let view = app.ui.effects_view.clone();
    let list = Rect::from_min_max(pos2(rect.min.x, sr.max.y + 6.0), rect.max);
    let lp = p.with_clip_rect(list);
    let scroll = widgets::PanelScroll::begin(ui, egui::Id::new("fx-scroll"), list);
    let query = q.trim().to_lowercase();
    let searching = !query.is_empty();
    let mut y = list.min.y - scroll.offset;
    let mut apply: Option<Apply> = None;
    let mut nav = Nav { rows: vec![], stack: vec![], cursor: ctx.data(|d| d.get_temp::<egui::Id>(cursor_id())), offset: scroll.offset, list_top: list.min.y };
    let reg = effectcraft_engine::effects::all();
    let keep = |e: &effectcraft_engine::effects::EffectSpec| {
        depth_keeps(&view.depth, e)
            && (query.is_empty() || effectcraft_engine::effects::name_matches(e, &query) || crate::i18n::effects::matches(e.id, e.name, &query))
    };
    let lookup = |id: &str| reg.iter().find(|e| e.id == id);

    // Favorites and Recently Used.
    let favs: Vec<_> = app.session.prefs.effects_favorites.iter().filter_map(|id| lookup(id)).filter(|e| keep(e)).collect();
    if !favs.is_empty() && folder_row(app, ui, &lp, list, &mut y, &mut nav, "favorites", "Favorites", 1, 0.0, searching, Some(Icon::Star)) {
        let mut favs = favs;
        favs.sort_by_key(|e| e.name);
        for e in favs {
            effect_row(app, ui, &lp, list, &mut y, &mut nav, e, 2, 0.0, &query, "effects.favorites", &mut apply);
        }
    }
    let recent: Vec<_> = app.ui.effects_recent.iter().filter_map(|id| lookup(id)).filter(|e| keep(e)).collect();
    if !recent.is_empty() && !searching && folder_row(app, ui, &lp, list, &mut y, &mut nav, "recent", "Recently Used", 1, 0.0, false, None) {
        for e in recent {
            effect_row(app, ui, &lp, list, &mut y, &mut nav, e, 2, 0.0, &query, "effects.recent", &mut apply);
        }
    }

    // * Animation Presets: our own presets (Text, Transitions…) and the user's .ecpreset files.
    if view.presets {
        let text: Vec<(String, String)> =
            effectcraft_engine::text_presets().into_iter().filter(|(_, n)| query.is_empty() || n.to_lowercase().contains(&query)).collect();
        let builtin: Vec<(String, String, String)> =
            effectcraft_engine::animation_presets().into_iter().filter(|(_, n, _)| query.is_empty() || n.to_lowercase().contains(&query)).collect();
        let mut categories: Vec<&str> = Vec::new();
        for (_, _, c) in &builtin {
            if !categories.contains(&c.as_str()) {
                categories.push(c);
            }
        }
        let user = user_presets(&ctx, force_scan, || presets_dir().map(|d| scan_presets(&d)).unwrap_or_default());
        let files: Vec<&PresetFile> = user.iter().filter(|f| query.is_empty() || f.name.to_lowercase().contains(&query)).collect();
        let any = !text.is_empty() || !builtin.is_empty() || !files.is_empty();
        if any && folder_row(app, ui, &lp, list, &mut y, &mut nav, "presets", "* Animation Presets", 1, 0.0, searching, None) {
            if !text.is_empty() && folder_row(app, ui, &lp, list, &mut y, &mut nav, "presets/Text", "Text", 2, 14.0, searching, None) {
                for (id, name) in text {
                    preset_row(
                        app,
                        ui,
                        &lp,
                        list,
                        &mut y,
                        &mut nav,
                        &name,
                        &format!("effects.preset.text.{id}"),
                        &query,
                        PresetRef::Text(id.clone()),
                        &mut apply,
                    );
                }
            }
            for cat in &categories {
                if !folder_row(app, ui, &lp, list, &mut y, &mut nav, &format!("presets/{cat}"), cat, 2, 14.0, searching, None) {
                    continue;
                }
                for (id, name, _) in builtin.iter().filter(|(_, _, c)| c == cat) {
                    preset_row(
                        app,
                        ui,
                        &lp,
                        list,
                        &mut y,
                        &mut nav,
                        name,
                        &format!("effects.preset.builtin.{id}"),
                        &query,
                        PresetRef::Builtin(id.clone()),
                        &mut apply,
                    );
                }
            }
            if !files.is_empty() && folder_row(app, ui, &lp, list, &mut y, &mut nav, "presets/User", "User Presets", 2, 14.0, searching, None) {
                for f in files {
                    preset_row(
                        app,
                        ui,
                        &lp,
                        list,
                        &mut y,
                        &mut nav,
                        &f.name,
                        &format!("effects.preset.file.{}", f.name),
                        &query,
                        PresetRef::File(f.path.clone()),
                        &mut apply,
                    );
                }
            }
        }
    }

    // Effects: by category, or one alphabetical list.
    if view.alphabetical {
        let mut all: Vec<_> = reg.iter().filter(|e| keep(e)).collect();
        all.sort_by_key(|e| e.name.to_lowercase());
        for e in all {
            effect_row(app, ui, &lp, list, &mut y, &mut nav, e, 1, -16.0, &query, "effects.item", &mut apply);
        }
    } else {
        let mut cats: Vec<&str> = effectcraft_engine::effects::categories();
        cats.retain(|c| reg.iter().any(|e| e.category == *c));
        for cat in cats {
            let items: Vec<_> = reg.iter().filter(|e| e.category == cat && keep(e)).collect();
            if items.is_empty() {
                continue;
            }
            // The folder draws the category in the interface language; `cat` stays the key (the
            // open/closed state and the automation id are the registry's English name).
            if !folder_row(app, ui, &lp, list, &mut y, &mut nav, cat, &crate::i18n::effects::category(cat), 1, 0.0, searching, None) {
                continue;
            }
            for e in items {
                effect_row(app, ui, &lp, list, &mut y, &mut nav, e, 2, 0.0, &query, "effects.item", &mut apply);
            }
        }
    }
    if y == list.min.y - scroll.offset && searching {
        lp.text(pos2(list.center().x, list.min.y + 24.0), Align2::CENTER_CENTER, tr("No matching effects or presets"), Tokens::ui(12.0), t.text_faint);
    }
    let content = y + scroll.offset - list.min.y;
    scroll.end(ui, &mut app.auto, "effects.scroll", content, &t);
    if let Some(a) = list_keys(app, &ctx, &nav, list) {
        apply = Some(a);
    }
    if let Some(name) = egui::DragAndDrop::payload::<DragPayload>(&ctx).and_then(|p| match p.as_ref() {
        DragPayload::Effect(id) => Some(effectcraft_engine::effects::find(id).map(|e| crate::i18n::effects::name(e.id, e.name)).unwrap_or_default()),
        DragPayload::Preset { name, .. } => Some(name.clone()),
        _ => None,
    }) && let Some(pos) = ctx.pointer_hover_pos()
    {
        let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Tooltip, egui::Id::new("dnd-fx")));
        let g = painter.layout_no_wrap(name, Tokens::ui(12.0), t.text);
        let r = Rect::from_min_size(pos + vec2(12.0, 8.0), g.size() + vec2(16.0, 8.0));
        painter.rect_filled(r, 4.0, Color32::from_rgba_premultiplied(40, 40, 40, 230));
        painter.rect_stroke(r, 4.0, Stroke::new(1.0, t.accent), egui::StrokeKind::Inside);
        painter.galley(r.min + vec2(8.0, 4.0), g, t.text);
    }
    let Some(apply) = apply else { return };
    if app.session.state.selected_layers.is_empty() {
        app.ui.status = "Select a layer to apply to".into();
        return;
    }
    let (cmd, params) = match &apply {
        Apply::Effect(id) => ("effect.apply", json!({"effect": id})),
        Apply::Preset(preset) => preset.command(),
    };
    match app.session.execute(cmd, params) {
        Err(e) => app.ui.status = e.to_string(),
        Ok(applied) => {
            if let Apply::Effect(id) = &apply {
                push_recent(&mut app.ui.effects_recent, id);
                app.show_panel(crate::dock::PanelKind::EffectControls);
                app.ui.focused = crate::dock::PanelKind::EffectsPresets;
                crate::panels::effect_controls::choose_files_after_apply(app, &applied);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_match_ranges() {
        assert_eq!(match_range("Gaussian Blur", "blur"), Some(9..13));
        assert_eq!(match_range("Gaussian Blur", "GAUSS"), Some(0..5));
        assert_eq!(match_range("Gaussian Blur", "x"), None);
        assert_eq!(match_range("Gaussian Blur", ""), None);
    }

    #[test]
    fn recent_list_is_most_recent_first_and_bounded() {
        let mut r = vec![];
        for id in ["a", "b", "c", "a"] {
            push_recent(&mut r, id);
        }
        assert_eq!(r, vec!["a", "c", "b"]);
        for i in 0..20 {
            push_recent(&mut r, &format!("x{i}"));
        }
        assert_eq!(r.len(), RECENT_MAX);
        assert_eq!(r[0], "x19");
    }

    #[test]
    fn depth_filter() {
        let reg = effectcraft_engine::effects::registry();
        let all = reg.iter().filter(|e| depth_keeps("all", e)).count();
        let f32s = reg.iter().filter(|e| depth_keeps("32", e)).count();
        assert_eq!(all, reg.len());
        assert!(f32s <= all && f32s > 0);
        assert!(reg.iter().filter(|e| depth_keeps("32", e)).all(|e| e.float));
    }

    #[test]
    fn scans_user_preset_files() {
        let d = std::env::temp_dir().join(format!("ec-presets-test-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("Zoom In.ecpreset"), "{}").unwrap();
        std::fs::write(d.join("bounce.ECPRESET"), "{}").unwrap();
        std::fs::write(d.join("notes.txt"), "").unwrap();
        let v = scan_presets(&d);
        assert_eq!(v.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), vec!["bounce", "Zoom In"]);
        assert!(scan_presets(&d.join("missing")).is_empty());
        let ctx = egui::Context::default();
        let set_time = |time| {
            let mut output = ctx.run_ui(egui::RawInput { time: Some(time), ..Default::default() }, |_| {});
            output.textures_delta.clear();
        };
        let scans = std::cell::Cell::new(0);
        let scan = || {
            scans.set(scans.get() + 1);
            scan_presets(&d)
        };
        set_time(1.0);
        let first = user_presets(&ctx, false, scan);
        assert_eq!(first.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["bounce", "Zoom In"]);
        std::fs::remove_file(d.join("bounce.ECPRESET")).unwrap();
        std::fs::write(d.join("Another.ecpreset"), "{}").unwrap();
        set_time(5.999);
        let hit = user_presets(&ctx, false, scan);
        assert!(Arc::ptr_eq(&first, &hit));
        assert_eq!(hit.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["bounce", "Zoom In"]);
        assert_eq!(scans.get(), 1);
        set_time(6.0);
        let expired = user_presets(&ctx, false, scan);
        assert_eq!(expired.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["Another", "Zoom In"]);
        std::fs::remove_file(d.join("Another.ecpreset")).unwrap();
        let refreshed = user_presets(&ctx, true, scan);
        assert_eq!(refreshed.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["Zoom In"]);
        assert_eq!(scans.get(), 3);
        assert_eq!(first.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["bounce", "Zoom In"]);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    #[ignore = "warm custom-preset cache benchmark"]
    fn user_preset_cache_benchmark() {
        let ctx = egui::Context::default();
        let files = user_presets(&ctx, false, || {
            (0..1000).map(|i| PresetFile { name: format!("Preset {i:04}"), path: format!("/presets/Preset {i:04}.ecpreset") }).collect()
        });
        assert_eq!(files.first().unwrap().name, "Preset 0000");
        assert_eq!(files.last().unwrap().name, "Preset 0999");
        let mut before = Vec::new();
        let mut after = Vec::new();
        for _ in 0..5 {
            let start = std::time::Instant::now();
            for _ in 0..1000 {
                let (_, shared) = ctx.data(|d| d.get_temp::<(f64, Arc<Vec<PresetFile>>)>(egui::Id::new("fx-user-presets"))).unwrap();
                std::hint::black_box((*shared).clone());
            }
            before.push(start.elapsed());
            let start = std::time::Instant::now();
            for _ in 0..1000 {
                std::hint::black_box(user_presets(&ctx, false, Vec::new));
            }
            after.push(start.elapsed());
        }
        assert!(Arc::ptr_eq(&files, &user_presets(&ctx, false, Vec::new)));
        before.sort();
        after.sort();
        println!("1,000 custom presets, median per warm cache lookup: {:?} -> {:?}", before[2] / 1000, after[2] / 1000);
    }
}
