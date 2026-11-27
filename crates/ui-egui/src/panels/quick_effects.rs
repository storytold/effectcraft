//! Quick Effects Search: a compact, non-modal search palette that applies a registered effect to
//! the selected timeline layers. The shortcut is configured in the existing Keyboard Shortcuts
//! editor (`effects.quickSearch`, Shift+Space by default); this module never hardcodes the chord.
//!
//! The palette is deliberately minimal: one search field, and nothing else until the user types.
//! Results are capped to five visible rows in a fixed-height scroller, so the popup never grows.

use egui::{Align, Align2, Id, Order, Rect, Sense, Stroke, StrokeKind, pos2, vec2};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::i18n::tr;
use crate::icons::{self, Icon};
use crate::panels::{PresetRef, effects_presets};
use crate::theme::Tokens;
use crate::widgets;
use crate::{EffectcraftApp, a11y};

const OPEN_S: f64 = 0.12;
const CLOSE_S: f64 = 0.10;
const ROW_H: f32 = 30.0;
/// Results visible at once. More matches scroll inside the same fixed box.
const VISIBLE_ROWS: usize = 5;
const WIDTH: f32 = 440.0;
const MAX_HEIGHT: f32 = 350.0;
const PAD: f32 = 12.0;
const FIELD_H: f32 = 36.0;
/// Open a little below the exact centre, leaving the comp viewer's middle less covered.
const OPEN_Y_OFFSET: f32 = 48.0;

fn area_id() -> Id {
    Id::NULL.with("quick-effects.area")
}

fn field_id() -> Id {
    Id::NULL.with("quick-effects.search")
}

fn previous_focus_id() -> Id {
    Id::NULL.with("quick-effects.previous-focus")
}

/// Wrapped so it can be stored in egui temp data (`remove_temp` needs `Default`).
#[derive(Clone, Copy, Default)]
struct PreviousFocus(Option<Id>);

/// A searchable result: either a registered effect or an animation preset from Effects & Presets.
#[derive(Clone)]
enum Hit {
    Effect(&'static effectcraft_engine::effects::EffectSpec),
    Preset { preset: PresetRef, name: String, category: String },
}

impl Hit {
    /// Text shown in the result row.
    fn name(&self) -> &str {
        match self {
            Hit::Effect(spec) => spec.name,
            Hit::Preset { name, .. } => name,
        }
    }

    /// Small right-hand label: real effect flags, or the preset's category.
    fn badge(&self) -> String {
        match self {
            Hit::Effect(spec) => match (spec.float, spec.gpu) {
                (true, true) => "32 · GPU".to_string(),
                (true, false) => "32".to_string(),
                (false, true) => "GPU".to_string(),
                (false, false) => String::new(),
            },
            Hit::Preset { category, .. } => category.clone(),
        }
    }

    fn icon(&self) -> Icon {
        match self {
            Hit::Effect(_) => Icon::Fx,
            Hit::Preset { .. } => Icon::Keyframe,
        }
    }

    /// Automation id suffix: effect id for effects, preset id/path for presets.
    fn auto_suffix(&self) -> String {
        match self {
            Hit::Effect(spec) => spec.id.to_string(),
            Hit::Preset { preset, .. } => match preset {
                PresetRef::Text(id) | PresetRef::Builtin(id) => id.clone(),
                PresetRef::File(path) => path.clone(),
            },
        }
    }
}

/// Session state of the popup. Deliberately small: the search resets every time it opens.
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct State {
    pub open: bool,
    /// Set when closing starts, so the popup can fade before it is removed.
    pub closing_since: Option<f64>,
    pub opened_at: f64,
    pub query: String,
    /// The query the current `sel` was chosen for, so a new search highlights its best match.
    pub shown_query: String,
    /// Index into the currently displayed result rows.
    pub sel: usize,
    /// Top-left of the last dragged position, in screen points. `None` means centre it.
    pub pos: Option<[f32; 2]>,
    /// Last query and its ranked results, so idle frames do not rescan the registry or presets.
    #[serde(skip)]
    cache: Option<(String, Vec<Hit>)>,
}

/// `EffectSpec` has no `Debug`, so spell out the fields and report only the cache's size.
impl std::fmt::Debug for State {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("State")
            .field("open", &self.open)
            .field("closing_since", &self.closing_since)
            .field("opened_at", &self.opened_at)
            .field("query", &self.query)
            .field("shown_query", &self.shown_query)
            .field("sel", &self.sel)
            .field("pos", &self.pos)
            .field("cache", &self.cache.as_ref().map(|(q, v)| (q, v.len())))
            .finish()
    }
}

/// Open the popup, resetting the search. Only one popup exists: this is a bool in [`UiState`].
pub fn open(app: &mut EffectcraftApp, ctx: &egui::Context) {
    let previous = ctx.memory(|m| m.focused());
    ctx.data_mut(|d| d.insert_temp(previous_focus_id(), PreviousFocus(previous)));
    // Focus the field now, not only when it is first drawn: the very first keystroke after the
    // shortcut must land in the search box. (`data_mut` and `memory_mut` share egui's lock, so
    // these calls must stay separate.)
    // A chord such as Shift+Space also emits a Text event; with the field focused that would
    // insert a stray space, so drop this frame's text events as we open.
    ctx.input_mut(|i| i.events.retain(|e| !matches!(e, egui::Event::Text(_))));
    ctx.data_mut(|d| d.remove::<egui::widgets::text_edit::TextEditState>(field_id()));
    ctx.memory_mut(|m| m.request_focus(field_id()));
    let now = app.now();
    let st = &mut app.ui.quick_effects;
    st.open = true;
    st.closing_since = None;
    st.opened_at = now;
    st.query.clear();
    st.shown_query.clear();
    st.sel = 0;
    // The last position lives in global preferences, so it survives both a new project and a
    // restart. `UiState` is per-project and deliberately does not carry UI-only geometry.
    if st.pos.is_none() {
        st.pos = app.session.prefs.quick_effects_pos;
    }
}

fn begin_close(st: &mut State, now: f64) {
    if st.open && st.closing_since.is_none() {
        st.closing_since = Some(now);
    }
}

fn finish_close(ctx: &egui::Context) {
    let previous = ctx.data_mut(|d| d.remove_temp::<PreviousFocus>(previous_focus_id())).unwrap_or_default();
    ctx.memory_mut(|m| {
        m.surrender_focus(field_id());
        if let Some(id) = previous.0 {
            m.request_focus(id);
        }
    });
}

/// The selected layers that still exist in the active comp, in their original order.
fn valid_targets(app: &EffectcraftApp) -> Vec<u64> {
    let Some(comp) = app.session.active_comp() else { return vec![] };
    app.session.state.selected_layers.iter().map(|l| l.0).filter(|id| comp.layer(effectcraft_engine::project::LayerId(*id)).is_some()).collect()
}

fn field_score(lower: &str, term: &str, weight: i32) -> i32 {
    if lower == term {
        weight * 2
    } else if lower.starts_with(term) {
        (weight * 3) / 2
    } else if lower.contains(term) {
        weight
    } else {
        0
    }
}

/// Relevance of `spec` for the whitespace-separated, lowercase `terms`; `None` = no match.
/// Only real registry metadata is searched: name, translated name, id, category, aliases and
/// parameter names.
fn score(spec: &effectcraft_engine::effects::EffectSpec, terms: &[String]) -> Option<i32> {
    if terms.is_empty() {
        return Some(0);
    }
    // Lowercase each field once per effect, not once per term and field_score call.
    let name = spec.name.to_lowercase();
    let shown = crate::i18n::effects::name(spec.id, spec.name).to_lowercase();
    let id = spec.id.to_lowercase();
    let category = spec.category.to_lowercase();
    let aliases: Vec<String> = effectcraft_engine::effects::aliases(spec.id).iter().map(|a| a.to_lowercase()).collect();
    let params: Vec<String> = spec.params.iter().map(|p| p.name.to_lowercase()).collect();
    let mut total = 0;
    for term in terms {
        let mut best = field_score(&name, term, 100).max(field_score(&shown, term, 90)).max(field_score(&id, term, 80)).max(field_score(&category, term, 30));
        for alias in &aliases {
            best = best.max(field_score(alias, term, 70));
        }
        for p in &params {
            best = best.max(field_score(p, term, 12));
        }
        if best == 0 {
            return None;
        }
        total += best;
    }
    Some(total)
}

/// Relevance of a preset for the whitespace-separated, lowercase `terms`. Presets have no parameter
/// list, so their name, id and Effects & Presets category are searched.
fn score_preset(name: &str, id: &str, category: &str, terms: &[String]) -> Option<i32> {
    let name = name.to_lowercase();
    let id = id.to_lowercase();
    let category = category.to_lowercase();
    let mut total = 0;
    for term in terms {
        let best = field_score(&name, term, 90).max(field_score(&id, term, 70)).max(field_score(&category, term, 25));
        if best == 0 {
            return None;
        }
        total += best;
    }
    Some(total)
}

/// The ranked matches for `query`, best first. An empty query matches nothing: the palette shows
/// no effects or presets until the user types. Everything Effects & Presets can show is searched:
/// every registered effect, all text and built-in animation presets (in every category), and the
/// user's `.ecpreset` files.
fn search(query: &str) -> Vec<Hit> {
    let terms: Vec<String> = query.split_whitespace().map(|w| w.to_lowercase()).collect();
    if terms.is_empty() {
        return vec![];
    }
    let mut scored: Vec<(i32, Hit)> = Vec::new();
    for e in effectcraft_engine::effects::all() {
        if let Some(s) = score(e, &terms) {
            scored.push((s, Hit::Effect(e)));
        }
    }
    for (id, name) in effectcraft_engine::text_presets() {
        if let Some(s) = score_preset(&name, &id, "Text", &terms) {
            scored.push((s, Hit::Preset { preset: PresetRef::Text(id), name, category: "Text".to_string() }));
        }
    }
    for (id, name, category) in effectcraft_engine::animation_presets() {
        if let Some(s) = score_preset(&name, &id, &category, &terms) {
            scored.push((s, Hit::Preset { preset: PresetRef::Builtin(id), name, category }));
        }
    }
    let files = effects_presets::presets_dir().map(|dir| effects_presets::scan_presets(&dir)).unwrap_or_default();
    for file in files {
        if let Some(s) = score_preset(&file.name, "", "User Presets", &terms) {
            scored.push((s, Hit::Preset { preset: PresetRef::File(file.path), name: file.name, category: "User Presets".to_string() }));
        }
    }
    // Best score first. `sort_by_key` is stable, so equal scores keep the Effects & Presets order:
    // effects, then text presets, built-in presets by category, then user presets.
    scored.sort_by_key(|(s, _)| std::cmp::Reverse(*s));
    scored.into_iter().map(|(_, hit)| hit).collect()
}

/// Return the ranked results for `query`, reusing the last frame's work while the query and the
/// registry are unchanged. Idle repaints (fade, hover, scrolling) then cost no scanning at all.
fn search_cached(st: &mut State, query: &str) -> Vec<Hit> {
    if st.cache.as_ref().is_some_and(|(q, _)| q == query) {
        return st.cache.as_ref().map(|(_, v)| v.clone()).unwrap_or_default();
    }
    let found = search(query);
    st.cache = Some((query.to_string(), found.clone()));
    found
}

/// Apply a result to the current selection through the same engine commands Effects & Presets
/// uses, then mirror its follow-up and close. Never records an effect that failed to apply.
fn apply(app: &mut EffectcraftApp, hit: Hit) {
    let (cmd, mut params, effect_id) = match &hit {
        Hit::Effect(spec) => ("effect.apply", json!({"effect": spec.id}), Some(spec.id)),
        Hit::Preset { preset, .. } => {
            let (cmd, params) = preset.command();
            (cmd, params, None)
        }
    };
    if app.session.state.selected_layers.is_empty() {
        app.ui.status = tr("Select a layer to apply to").to_string();
        return;
    }
    if effect_id.is_some() {
        let layers = valid_targets(app);
        if layers.is_empty() {
            app.ui.status = tr("Select a layer to apply to").to_string();
            return;
        }
        params["layers"] = json!(layers);
    }
    match app.session.execute(cmd, params) {
        Err(e) => app.ui.status = e.to_string(),
        Ok(applied) => {
            if let Some(id) = effect_id {
                crate::panels::effects_presets::push_recent(&mut app.ui.effects_recent, id);
                app.show_panel(crate::dock::PanelKind::EffectControls);
                app.ui.focused = crate::dock::PanelKind::EffectControls;
                crate::panels::effect_controls::choose_files_after_apply(app, &applied);
            }
            let now = app.now();
            begin_close(&mut app.ui.quick_effects, now);
        }
    }
}

/// Draw the popup, if it is open or fading out. Called once per frame from `app::frame`.
pub fn show(app: &mut EffectcraftApp, ctx: &egui::Context) {
    // Work on a copy: the popup's widgets and the engine both need `app` during this frame.
    let mut st = app.ui.quick_effects.clone();
    // A whitespace-only query is not a search: keep the field visually and semantically empty.
    if !st.query.is_empty() && st.query.trim().is_empty() {
        st.query.clear();
    }
    if !st.open {
        return;
    }
    if app.dialog.is_some() && st.closing_since.is_none() {
        begin_close(&mut st, app.now());
    }
    let now = app.now();
    let alpha = match st.closing_since {
        Some(at) => (1.0 - (now - at) / CLOSE_S).clamp(0.0, 1.0),
        None => ((now - st.opened_at) / OPEN_S).clamp(0.0, 1.0),
    };
    // The fade is time-based, so keep painting until it finishes (or the close is done).
    if st.closing_since.is_some() || now - st.opened_at < OPEN_S {
        ctx.request_repaint();
    }
    if st.closing_since.is_some_and(|at| now - at >= CLOSE_S) {
        finish_close(ctx);
        st.open = false;
        st.closing_since = None;
        st.query.clear();
        st.sel = 0;
        app.ui.quick_effects = st;
        return;
    }

    let targets = valid_targets(app);
    let query = st.query.clone();
    let rows = search_cached(&mut st, &query);
    // A changed query starts again from the best match, so Enter applies what the search ranks
    // first rather than wherever the highlight happened to be.
    if st.query != st.shown_query {
        st.shown_query = st.query.clone();
        st.sel = 0;
    }
    if st.sel >= rows.len() {
        st.sel = rows.len().saturating_sub(1);
    }

    // Keyboard: navigation is read before drawing so the highlighted row is already current.
    let (up, down, enter, escape) = ctx
        .input(|i| (i.key_pressed(egui::Key::ArrowUp), i.key_pressed(egui::Key::ArrowDown), i.key_pressed(egui::Key::Enter), i.key_pressed(egui::Key::Escape)));
    if st.closing_since.is_none() {
        if up && !rows.is_empty() {
            st.sel = (st.sel + rows.len() - 1) % rows.len();
        }
        if down && !rows.is_empty() {
            st.sel = (st.sel + 1) % rows.len();
        }
        ctx.input_mut(|i| {
            if up {
                i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp);
            }
            if down {
                i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown);
            }
            if escape {
                i.consume_key(egui::Modifiers::NONE, egui::Key::Escape);
            }
            if enter {
                i.consume_key(egui::Modifiers::NONE, egui::Key::Enter);
            }
        });
        if escape {
            begin_close(&mut st, app.now());
        }
    }
    let mut apply_hit = None;
    if enter && st.closing_since.is_none() {
        apply_hit = rows.get(st.sel).cloned();
    }
    let scroll_to = (up || down).then_some(st.sel);

    let t = app.tokens;
    let screen = ctx.content_rect();
    // Compact and fixed: only the row box is capped, never the popup.
    let width = WIDTH.min((screen.width() - 24.0).max(260.0));
    let list_h = (ROW_H * VISIBLE_ROWS as f32).min((MAX_HEIGHT - FIELD_H - PAD * 3.0).max(ROW_H));
    let height = PAD * 2.0 + FIELD_H + 10.0 + list_h;
    let centered = pos2(screen.center().x - width / 2.0, screen.center().y - height / 2.0 + OPEN_Y_OFFSET);
    // `default_pos` (not `fixed_pos`) lets egui drag the Area; our own copy makes the last
    // position survive closing, reopening and even a restart of the app.
    let at = st.pos.map(|[x, y]| pos2(x, y)).unwrap_or(centered);
    let mut clicked: Option<Hit> = None;

    let area = egui::Area::new(area_id()).order(Order::Foreground).default_pos(at).movable(true).constrain(true).show(ctx, |ui| {
        ui.set_opacity(alpha as f32);
        // No forced min/max size: egui's Area rect then matches the visible frame. Forcing the
        // full five-row height left an invisible interactive strip below the frame, so a click
        // just outside the visible bottom was treated as "inside" and did not close the popup.
        egui::Frame::new()
            .fill(t.panel_bg)
            .stroke(Stroke::new(1.0, t.field_border))
            .corner_radius(t.radius)
            .inner_margin(egui::Margin::same(PAD as i8))
            .shadow(egui::Shadow { offset: [0, 8], blur: 24, spread: 0, color: egui::Color32::from_black_alpha(140) })
            .show(ui, |ui| {
                ui.set_width(width - PAD * 2.0);

                // One clean search field: magnifier, hint, and a clear button once typed in.
                let field = Rect::from_min_size(ui.cursor().min, vec2(ui.available_width(), FIELD_H));
                ui.add_space(FIELD_H);
                ui.painter().rect_filled(field, t.radius_sm, t.field_bg);
                ui.painter().rect_stroke(field, t.radius_sm, Stroke::new(1.0, t.field_border), StrokeKind::Inside);
                icons::paint(
                    ui.painter(),
                    Rect::from_center_size(pos2(field.min.x + 15.0, field.center().y), vec2(13.0, 13.0)),
                    Icon::Search,
                    if st.query.is_empty() { t.text_dim } else { t.text },
                );
                let inner = Rect::from_min_max(pos2(field.min.x + 30.0, field.min.y), pos2(field.max.x - 6.0, field.max.y));
                // Give TextEdit the whole inner rect and centre its text atom in it. Both the
                // placeholder and typed text (and the cursor) then share one baseline; no manual
                // y offset that drifts with font size or window scaling.
                let search = ui.put(
                    inner,
                    egui::TextEdit::singleline(&mut st.query)
                        .id(field_id())
                        .frame(egui::Frame::NONE)
                        .hint_text(tr("Search effects..."))
                        .font(Tokens::ui(13.5))
                        .align(Align2::LEFT_CENTER)
                        .min_size(inner.size())
                        .desired_width(inner.width()),
                );
                widgets::keep_focus(ctx, &search, &st.query);
                a11y::text_input(&search, tr("Search effects"), &st.query);
                app.auto.add("quickEffects.search", search.rect, "Search effects");
                if st.query.trim().is_empty() {
                    // An empty search shows only the field: no results box and no extra gap.
                    return;
                }
                ui.add_space(10.0);

                // Fixed-height results box: five rows visible, the rest scroll inside it.
                let list_w = ui.available_width();
                let box_top = ui.cursor().min.y;
                ui.allocate_ui_with_layout(vec2(list_w, list_h), egui::Layout::top_down(Align::Min), |ui| {
                    if targets.is_empty() {
                        ui.add_space(list_h / 2.0 - 20.0);
                        ui.vertical_centered(|ui| {
                            ui.label(egui::RichText::new(tr("Select a layer to browse and apply effects.")).color(t.text_dim));
                        });
                        return;
                    }
                    if rows.is_empty() {
                        ui.add_space(list_h / 2.0 - 10.0);
                        ui.vertical_centered(|ui| {
                            ui.label(egui::RichText::new(tr("No effects match your search.")).color(t.text_dim));
                        });
                        return;
                    }
                    // Only the rows in view are built, so the cost and the drawn rows stay at five
                    // however many effects match. `show_rows` counts `item_spacing` in the row
                    // pitch, so zero it: five rows then fill the box exactly.
                    ui.spacing_mut().item_spacing.y = 0.0;
                    egui::ScrollArea::vertical().id_salt("quick-effects-list").max_height(list_h).auto_shrink(false).show_rows(
                        ui,
                        ROW_H,
                        rows.len(),
                        |ui, range| {
                            ui.set_width(list_w);
                            // A selected row outside the rendered range (arrow-key navigation past
                            // the five visible) scrolls into view by its computed position.
                            if let Some(i) = scroll_to
                                && !range.contains(&i)
                            {
                                let top = ui.max_rect().top() + i as f32 * ROW_H;
                                let row = Rect::from_min_size(pos2(ui.max_rect().left(), top), vec2(ui.max_rect().width(), ROW_H));
                                ui.scroll_to_rect(row, Some(Align::Center));
                            }
                            for i in range {
                                let hit = &rows[i];
                                let (row_rect, r) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::click());
                                let selected = i == st.sel;
                                // A one-pixel inset keeps adjacent highlights from touching.
                                let highlight = row_rect.shrink2(vec2(0.0, 1.0));
                                if selected {
                                    ui.painter().rect_filled(highlight, t.radius_sm, t.row_selected);
                                } else if r.hovered() {
                                    ui.painter().rect_filled(highlight, t.radius_sm, t.hover);
                                }
                                icons::paint(
                                    ui.painter(),
                                    Rect::from_center_size(pos2(row_rect.min.x + 12.0, row_rect.center().y), vec2(13.0, 13.0)),
                                    hit.icon(),
                                    if selected { t.accent } else { t.text_dim },
                                );
                                let shown = match hit {
                                    Hit::Effect(spec) => crate::i18n::effects::name(spec.id, spec.name),
                                    Hit::Preset { name, .. } => name.clone(),
                                };
                                ui.painter().text(
                                    pos2(row_rect.min.x + 26.0, row_rect.center().y),
                                    Align2::LEFT_CENTER,
                                    shown.clone(),
                                    Tokens::ui(12.5),
                                    t.text,
                                );
                                // Real effect flags, or the preset's Effects & Presets category.
                                let badges = hit.badge();
                                if !badges.is_empty() {
                                    ui.painter().text(
                                        pos2(row_rect.max.x - 6.0, row_rect.center().y),
                                        Align2::RIGHT_CENTER,
                                        badges,
                                        Tokens::ui(10.0),
                                        t.text_faint,
                                    );
                                }
                                app.auto.add(&format!("quickEffects.item.{}", hit.auto_suffix()), row_rect, hit.name());
                                a11y::list_item(&r, &shown, selected, i, rows.len());
                                if scroll_to == Some(i) {
                                    r.scroll_to_me(Some(Align::Center));
                                }
                                if r.clicked() {
                                    st.sel = i;
                                    clicked = Some(hit.clone());
                                }
                            }
                        },
                    );
                });
                // The visible results box, so tests and agents can check its fixed height.
                app.auto.add("quickEffects.list", Rect::from_min_size(pos2(ui.min_rect().left(), box_top), vec2(list_w, list_h)), "Results");
            });
    });
    // Remember wherever the user dragged it, including egui's own constrain adjustment. The
    // preferences copy is global, so the next project opens the palette in the same place.
    let r = area.response.rect;
    let at = [r.min.x.round(), r.min.y.round()];
    st.pos = Some(at);
    if app.session.prefs.quick_effects_pos != st.pos {
        app.session.prefs.quick_effects_pos = st.pos;
        app.session.save_prefs();
    }
    let outside = ctx.input(|i| i.pointer.any_pressed() && i.pointer.press_origin().is_some_and(|p| !r.contains(p)) && now - st.opened_at > 0.05);
    if outside && st.closing_since.is_none() {
        begin_close(&mut st, app.now());
    }
    // Commit the frame's state before applying: `apply` starts the closing animation itself.
    app.ui.quick_effects = st;
    if let Some(hit) = apply_hit.or(clicked) {
        apply(app, hit);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn score_matches_names_ids_categories_and_aliases() {
        let all = effectcraft_engine::effects::all();
        assert!(all.iter().any(|e| score(e, &["drop".to_string()]).is_some()), "drop matches real effects");
        assert!(all.iter().any(|e| score(e, &["blur".to_string()]).is_some()), "blur matches real effects");
        assert!(all.iter().any(|e| score(e, &["color".to_string()]).is_some()), "color matches real effects");
        assert!(all.iter().any(|e| score(e, &["cc".to_string()]).is_some()), "aliases are searchable");
        assert!(all.iter().any(|e| score(e, &[e.id.to_string()]).is_some()), "ids are searchable");
        assert!(all.iter().all(|e| score(e, &["zzzznotaneffect".to_string()]).is_none()));
    }

    #[test]
    fn scoring_is_case_insensitive_and_ranks_exact_names_first() {
        let all = effectcraft_engine::effects::all();
        let mut found = all.iter().filter_map(|e| score(e, &["blur".to_string()]).map(|s| (s, e.name))).collect::<Vec<_>>();
        assert!(found.len() > 1);
        found.sort_by_key(|(s, _)| std::cmp::Reverse(*s));
        assert!(found[0].1.to_lowercase().contains("blur"));
    }

    #[test]
    fn field_score_rewards_prefixes() {
        assert!(field_score("gaussian blur", "gauss", 100) > field_score("gaussian blur", "ussian", 100));
        assert!(field_score("blur", "blur", 100) > field_score("blur", "blurx", 100));
    }

    /// Nothing is listed before the user types: no catalog, no favorites, no recents.
    #[test]
    fn an_empty_query_lists_nothing() {
        assert!(search("").is_empty());
        assert!(search("   ").is_empty());
    }

    /// The search covers the whole registry; the five-row cap is display-only.
    #[test]
    fn search_returns_every_match_not_just_five() {
        let hits = search("blur");
        assert!(hits.len() > VISIBLE_ROWS, "{} matches, more than the {} visible rows", hits.len(), VISIBLE_ROWS);
    }

    /// Every query in the acceptance list finds real results, case-insensitively.
    #[test]
    fn documented_queries_find_real_effects() {
        let keys = |hits: &[Hit]| hits.iter().map(Hit::auto_suffix).collect::<Vec<_>>();
        for query in ["blur", "drop", "shadow", "color", "glow", "transform", "audio"] {
            let hits = search(query);
            assert!(!hits.is_empty(), "`{query}` finds nothing");
            assert!(
                hits.iter().all(|h| match h {
                    Hit::Effect(e) => effectcraft_engine::effects::find(e.id).is_some(),
                    Hit::Preset { .. } => true,
                }),
                "`{query}` returned a non-registered effect"
            );
            assert_eq!(keys(&search(&query.to_uppercase())), keys(&hits), "`{query}` is case-insensitive");
        }
    }

    /// Partial words and registered aliases both match; an empty query clears all results.
    #[test]
    fn partial_and_alias_matches_but_empty_returns_nothing() {
        assert!(search("blu").iter().any(|h| h.name().to_lowercase().contains("blur")), "partial `blu` matches blur effects");
        assert!(
            search("cc")
                .iter()
                .any(|h| matches!(h, Hit::Effect(e) if effectcraft_engine::effects::aliases(e.id).iter().any(|a| a.to_lowercase().contains("cc")))),
            "aliases are searched"
        );
        assert!(
            search("blur").iter().any(|h| !h.name().to_lowercase().starts_with("blur") && h.name().to_lowercase().contains("blur")),
            "substring matches are included"
        );
        assert!(search("").is_empty() && search("  ").is_empty(), "clearing the query returns no rows");
    }

    /// Exact names outrank prefix hits, which outrank loose substring hits.
    #[test]
    fn exact_matches_rank_above_prefix_and_partial() {
        let hits = search("blur");
        let names: Vec<&str> = hits.iter().map(Hit::name).collect();
        let exact = names.iter().position(|n| n.eq_ignore_ascii_case("blur"));
        let partial = names.iter().position(|n| n.to_lowercase().contains("blur") && !n.to_lowercase().starts_with("blur"));
        if let (Some(e), Some(p)) = (exact, partial) {
            assert!(e < p, "an exact name ranks above a partial match: {names:?}");
        }
    }

    /// Everything Effects & Presets can show is findable: every registered effect, every text
    /// preset, every built-in preset in every category, and user `.ecpreset` files by name.
    #[test]
    fn every_registered_preset_and_effect_is_findable() {
        for spec in effectcraft_engine::effects::all() {
            assert!(search(spec.name).iter().any(|h| h.name() == spec.name), "effect `{}` is not findable", spec.name);
        }
        for (_, name) in effectcraft_engine::text_presets() {
            assert!(search(&name).iter().any(|h| h.name() == name), "text preset `{name}` is not findable");
        }
        for (_, name, _) in effectcraft_engine::animation_presets() {
            assert!(search(&name).iter().any(|h| h.name() == name), "animation preset `{name}` is not findable");
        }
        assert!(
            search("fade up").iter().any(|h| matches!(h, Hit::Preset { name, .. } if name == "Fade Up Characters")),
            "the Fade Up Characters text preset is findable"
        );
        assert!(score_preset("My Fade", "", "User Presets", &["fade".to_string()]).is_some(), "user preset names are searchable");
    }
}
