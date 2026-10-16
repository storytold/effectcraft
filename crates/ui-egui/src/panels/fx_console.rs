//! Effect Console (Ctrl+Space, `app.fxConsole`): a small search bar that opens at the pointer,
//! finds effects and animation presets as you type (name, word starts, initials such as `gb`
//! for Gaussian Blur, or the letters in order) and applies the chosen one to the selected
//! layers. Enter applies and closes; Shift+Enter applies and keeps the console open; ↑/↓ move
//! the selection; Escape, Ctrl+Space or a click outside closes it. With no text it lists the
//! recently used effects.

use std::collections::BTreeSet;

use egui::{Align2, Color32, Rect, Sense, Stroke, pos2, vec2};
use serde_json::json;

use super::effects_presets::{PresetFile, match_range, push_recent};
use crate::EffectcraftApp;
use crate::i18n::tr;
use crate::icons::{self, Icon};
use crate::theme::Tokens;

const WIDTH: f32 = 400.0;
const FIELD_H: f32 = 30.0;
const ROW_H: f32 = 24.0;
/// Results shown at once.
pub const MAX_RESULTS: usize = 10;

/// The open console.
#[derive(Clone, Debug, Default)]
pub struct Console {
    pub query: String,
    pub sel: usize,
    /// Where it opened (top-left of the bar is placed around it).
    pub at: Option<egui::Pos2>,
    /// Opened this frame: a click that opened it doesn't close it.
    pub fresh: bool,
}

/// What a result applies.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    Effect(String),
    TextPreset(String),
    PresetFile(String),
}

/// One search result.
#[derive(Clone, Debug, PartialEq)]
pub struct Hit {
    pub label: String,
    /// Category shown on the right.
    pub detail: String,
    pub target: Target,
    pub score: i32,
}

/// How well `name` matches `query` (lowercase, trimmed); higher is better, `None` when it
/// doesn't. Exact name, then name start, word start, initials (`gb` → Gaussian Blur), every
/// word starting a word, substring, every word a substring, then the letters in order.
pub fn score(name: &str, query: &str) -> Option<i32> {
    let q = query.trim();
    if q.is_empty() {
        return Some(0);
    }
    let n = name.to_lowercase();
    // Shorter names win ties ("Blur" before "Bilateral Blur").
    let short = i32::try_from(n.chars().count().min(60)).unwrap_or(60);
    if n == q {
        return Some(1000);
    }
    if n.starts_with(q) {
        return Some(900 - short);
    }
    let words: Vec<&str> = n.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).collect();
    if let Some(i) = words.iter().position(|w| w.starts_with(q)) {
        return Some(800 - 4 * i32::try_from(i.min(20)).unwrap_or(20) - short);
    }
    let tokens: Vec<&str> = q.split_whitespace().collect();
    let initials: String = words.iter().filter_map(|w| w.chars().next()).collect();
    if tokens.len() == 1 && q.chars().count() >= 2 && initials.starts_with(q) {
        return Some(750 - short);
    }
    if tokens.len() > 1 && tokens.iter().all(|t| words.iter().any(|w| w.starts_with(t))) {
        return Some(700 - short);
    }
    if n.contains(q) {
        return Some(600 - short);
    }
    if tokens.len() > 1 && tokens.iter().all(|t| n.contains(t)) {
        return Some(500 - short);
    }
    // The letters in order, fewest skipped first.
    let letters: Vec<char> = q.chars().filter(|c| !c.is_whitespace()).collect();
    if letters.len() < 2 {
        return None;
    }
    let mut chars = n.chars();
    let mut skipped = 0i32;
    for c in letters {
        loop {
            match chars.next() {
                Some(x) if x == c => break,
                Some(_) => skipped = skipped.saturating_add(1),
                None => return None,
            }
        }
    }
    Some(300 - skipped.min(200) - short)
}

/// Search effects, text presets and user preset files for `query`, best first. With an empty
/// query: the recently used effects. `recent` is most-recent-first; recently used and
/// favourite effects rank higher, Obsolete ones lower.
pub fn search(query: &str, recent: &[String], favorites: &BTreeSet<String>, presets: &[PresetFile]) -> Vec<Hit> {
    let q = query.trim().to_lowercase();
    let effects = effectcraft_engine::effects::all();
    if q.is_empty() {
        return recent
            .iter()
            .filter_map(|id| effects.iter().find(|e| e.id == id))
            .take(MAX_RESULTS)
            .map(|e| Hit { label: e.name.to_string(), detail: e.category.to_string(), target: Target::Effect(e.id.to_string()), score: 0 })
            .collect();
    }
    let mut hits: Vec<Hit> = vec![];
    for e in &effects {
        let best = std::iter::once(e.name).chain(effectcraft_engine::effects::aliases(e.id).iter().copied()).filter_map(|n| score(n, &q)).max();
        let Some(mut s) = best else { continue };
        if let Some(i) = recent.iter().position(|r| r == e.id) {
            s += 40 - 4 * i32::try_from(i.min(10)).unwrap_or(10);
        }
        if favorites.contains(e.id) {
            s += 30;
        }
        if e.category.eq_ignore_ascii_case("Obsolete") {
            s -= 150;
        }
        hits.push(Hit { label: e.name.to_string(), detail: e.category.to_string(), target: Target::Effect(e.id.to_string()), score: s });
    }
    for (id, name) in effectcraft_engine::text_presets() {
        if let Some(s) = score(&name, &q) {
            hits.push(Hit { label: name, detail: "Text Preset".into(), target: Target::TextPreset(id), score: s - 5 });
        }
    }
    for f in presets {
        if let Some(s) = score(&f.name, &q) {
            hits.push(Hit { label: f.name.clone(), detail: "User Preset".into(), target: Target::PresetFile(f.path.clone()), score: s - 5 });
        }
    }
    hits.sort_by(|a, b| b.score.cmp(&a.score).then_with(|| a.label.to_lowercase().cmp(&b.label.to_lowercase())));
    hits.truncate(MAX_RESULTS);
    hits
}

/// Open the console at the pointer (`query` prefilled).
pub fn open(app: &mut EffectcraftApp, ctx: &egui::Context, query: &str) {
    let at = ctx.input(|i| i.pointer.latest_pos());
    app.dialog_state.fx_console = Some(Console { query: query.to_string(), sel: 0, at, fresh: true });
}

/// Apply `target` to the selected layers. Errors are for the status bar.
pub fn apply(app: &mut EffectcraftApp, ctx: &egui::Context, target: &Target) -> Result<(), String> {
    if app.session.state.selected_layers.is_empty() {
        return Err(tr("Select a layer to apply to").to_string());
    }
    let (cmd, params) = match target {
        Target::Effect(id) => ("effect.apply", json!({"effect": id})),
        Target::TextPreset(id) => ("layer.applyTextPreset", json!({"preset": id})),
        Target::PresetFile(path) => ("anim.applyPreset", json!({"path": path})),
    };
    crate::menus::invoke(app, ctx, cmd, params)?;
    if let Target::Effect(id) = target {
        push_recent(&mut app.ui.effects_recent, id);
    }
    Ok(())
}

/// Text with the match highlighted (as in Effects & Presets).
fn highlighted(p: &egui::Painter, pos: egui::Pos2, name: &str, query: &str, t: &Tokens) {
    if let Some(m) = match_range(name, query.trim())
        && let (Some(pre), Some(hit)) = (name.get(..m.start), name.get(m.clone()))
    {
        let x0 = p.layout_no_wrap(pre.to_string(), Tokens::ui(12.5), t.text).size().x;
        let w = p.layout_no_wrap(hit.to_string(), Tokens::ui(12.5), t.text).size().x;
        p.rect_filled(Rect::from_min_size(pos2(pos.x + x0 - 1.0, pos.y - 8.0), vec2(w + 2.0, 16.0)), 2.0, t.accent.gamma_multiply(0.45));
    }
    p.text(pos, Align2::LEFT_CENTER, name, Tokens::ui(12.5), t.text);
}

pub fn show(app: &mut EffectcraftApp, ctx: &egui::Context) {
    let Some(mut c) = app.dialog_state.fx_console.take() else { return };
    let t = app.tokens;
    // Keys the console owns, taken before the text field sees them.
    let (esc, toggle, down, up, keep, enter) = ctx.input_mut(|i| {
        (
            i.consume_key(egui::Modifiers::NONE, egui::Key::Escape),
            i.consume_key(egui::Modifiers::CTRL, egui::Key::Space),
            i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown) | i.consume_key(egui::Modifiers::NONE, egui::Key::Tab),
            i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp) | i.consume_key(egui::Modifiers::SHIFT, egui::Key::Tab),
            i.consume_key(egui::Modifiers::SHIFT, egui::Key::Enter),
            i.consume_key(egui::Modifiers::NONE, egui::Key::Enter),
        )
    });
    if esc || (toggle && !c.fresh) {
        return;
    }
    let presets = super::effects_presets::user_presets(ctx, false);
    let hits = search(&c.query, &app.ui.effects_recent, &app.ui.effects_favorites, &presets);
    if down {
        c.sel = (c.sel + 1).min(hits.len().saturating_sub(1));
    }
    if up {
        c.sel = c.sel.saturating_sub(1);
    }
    c.sel = c.sel.min(hits.len().saturating_sub(1));

    // Placement: the bar centred on the pointer, kept on screen.
    let screen = ctx.content_rect();
    let rows = hits.len().max(1) as f32 + if app.session.state.selected_layers.is_empty() { 1.0 } else { 0.0 };
    let height = FIELD_H + 16.0 + 8.0 + rows * ROW_H + 8.0;
    let at = c.at.unwrap_or(screen.center());
    let x = (at.x - WIDTH / 2.0).clamp(screen.min.x + 8.0, (screen.max.x - WIDTH - 8.0).max(screen.min.x + 8.0));
    let y = (at.y - FIELD_H / 2.0 - 8.0).clamp(screen.min.y + 8.0, (screen.max.y - height - 8.0).max(screen.min.y + 8.0));

    let mut run: Option<(Target, bool)> = None;
    let mut q = c.query.clone();
    let area = egui::Area::new(egui::Id::new("fx-console")).order(egui::Order::Foreground).fixed_pos(pos2(x, y)).show(ctx, |ui| {
        egui::Frame::new()
            .fill(t.panel_bg)
            .stroke(Stroke::new(1.0, t.accent.gamma_multiply(0.8)))
            .corner_radius(8.0)
            .inner_margin(egui::Margin::same(8))
            .shadow(egui::epaint::Shadow { offset: [0, 6], blur: 18, spread: 0, color: Color32::from_black_alpha(110) })
            .show(ui, |ui| {
                ui.set_width(WIDTH - 16.0);
                let (fr, _) = ui.allocate_exact_size(vec2(WIDTH - 16.0, FIELD_H), Sense::hover());
                let resp = crate::widgets::search_field(ui, fr, &mut q, tr("Search effects and presets"), &t);
                resp.request_focus();
                app.auto.add("fxConsole.search", fr, "Effect Console");
                ui.add_space(6.0);
                if app.session.state.selected_layers.is_empty() {
                    let (r, _) = ui.allocate_exact_size(vec2(WIDTH - 16.0, ROW_H), Sense::hover());
                    ui.painter().text(pos2(r.min.x + 8.0, r.center().y), Align2::LEFT_CENTER, tr("No layer selected"), Tokens::ui(11.5), t.text_faint);
                }
                if hits.is_empty() {
                    let (r, _) = ui.allocate_exact_size(vec2(WIDTH - 16.0, ROW_H), Sense::hover());
                    let msg = if c.query.trim().is_empty() { tr("Type an effect or preset name") } else { tr("No matching effects or presets") };
                    ui.painter().text(pos2(r.min.x + 8.0, r.center().y), Align2::LEFT_CENTER, msg, Tokens::ui(12.0), t.text_faint);
                }
                for (i, h) in hits.iter().enumerate() {
                    let (r, resp) = ui.allocate_exact_size(vec2(WIDTH - 16.0, ROW_H), Sense::click());
                    if resp.hovered() && ui.input(|inp| inp.pointer.delta() != egui::Vec2::ZERO) {
                        c.sel = i;
                    }
                    let p = ui.painter();
                    if i == c.sel {
                        p.rect_filled(r, 4.0, t.row_selected);
                    }
                    let icon = match h.target {
                        Target::Effect(ref id) if app.ui.effects_favorites.contains(id) => Icon::Star,
                        Target::Effect(_) => Icon::Fx,
                        _ => Icon::Keyframe,
                    };
                    icons::paint(p, Rect::from_center_size(pos2(r.min.x + 14.0, r.center().y), vec2(12.0, 12.0)), icon, t.text_dim);
                    let dw = p.layout_no_wrap(h.detail.clone(), Tokens::ui(11.0), t.text_dim).size().x;
                    let name_clip = Rect::from_min_max(r.min, pos2(r.max.x - dw - 18.0, r.max.y));
                    highlighted(&p.with_clip_rect(name_clip), pos2(r.min.x + 28.0, r.center().y), &h.label, &c.query, &t);
                    p.text(pos2(r.max.x - 8.0, r.center().y), Align2::RIGHT_CENTER, &h.detail, Tokens::ui(11.0), t.text_dim);
                    app.auto.add(&format!("fxConsole.result.{i}"), r, &h.label);
                    if resp.clicked() {
                        run = Some((h.target.clone(), ui.input(|inp| inp.modifiers.shift)));
                    }
                }
            });
    });
    if q != c.query {
        c.query = q;
        c.sel = 0;
    }
    if run.is_none()
        && (enter || keep)
        && let Some(h) = hits.get(c.sel)
    {
        run = Some((h.target.clone(), keep));
    }
    // A click anywhere else closes it (and goes on to what was clicked).
    let outside = ctx.input(|i| i.pointer.any_pressed() && i.pointer.interact_pos().is_some_and(|p| !area.response.rect.contains(p)));
    let fresh = std::mem::take(&mut c.fresh);
    if outside && !fresh {
        return;
    }
    if let Some((target, stay)) = run {
        if let Err(e) = apply(app, ctx, &target) {
            app.ui.status = e;
        }
        if !stay {
            return;
        }
        c.query.clear();
        c.sel = 0;
    }
    app.dialog_state.fx_console = Some(c);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn top(query: &str) -> Vec<String> {
        search(query, &[], &BTreeSet::new(), &[]).into_iter().map(|h| h.label).collect()
    }

    #[test]
    fn scoring_prefers_exact_then_prefix_then_words_then_initials_then_letters() {
        let s = |n| score(n, "gaussian blur").unwrap_or(-1);
        assert_eq!(s("Gaussian Blur"), 1000);
        assert!(score("Gaussian Blur", "gau").unwrap() > score("Fast Box Blur", "blur").unwrap(), "name start beats a later word");
        assert!(score("Gaussian Blur", "blur").unwrap() > score("Gaussian Blur", "gb").unwrap(), "word start beats initials");
        assert!(score("Gaussian Blur", "gb").unwrap() > score("Gaussian Blur", "ssian").unwrap(), "initials beat a substring");
        assert!(score("Gaussian Blur", "ssian").unwrap() > score("Gaussian Blur", "gsnbr").unwrap(), "substring beats scattered letters");
        assert_eq!(score("Gaussian Blur", "xyz"), None);
        assert_eq!(score("Gaussian Blur", "z"), None, "one stray letter doesn't match");
        assert!(score("Fast Box Blur", "box bl").is_some(), "several words, each a word start");
        assert_eq!(score("Anything", "   "), Some(0));
        // Non-ASCII names and queries don't panic.
        assert!(score("Ünïcödé Blür", "blü").is_some());
        assert_eq!(score("日本", "語"), None);
    }

    #[test]
    fn search_finds_effects_by_name_initials_and_letters() {
        assert_eq!(top("gaussian blur").first().map(String::as_str), Some("Gaussian Blur"));
        assert!(top("gb").iter().take(3).any(|l| l == "Gaussian Blur"), "{:?}", top("gb"));
        assert!(top("curves").first().is_some_and(|l| l == "Curves"), "{:?}", top("curves"));
        assert!(top("glow").iter().any(|l| l == "Glow"));
        assert!(top("qqqqzzzz").is_empty());
        assert!(top("blur").len() <= MAX_RESULTS);
    }

    #[test]
    fn recent_and_favourites_rank_higher_and_empty_lists_recent() {
        let reg = effectcraft_engine::effects::all();
        let fast = reg.iter().find(|e| e.name == "Fast Box Blur").map(|e| e.id.to_string()).unwrap();
        let plain = search("blur", &[], &BTreeSet::new(), &[]);
        let boosted = search("blur", std::slice::from_ref(&fast), &BTreeSet::new(), &[]);
        let pos = |v: &[Hit]| v.iter().position(|h| h.target == Target::Effect(fast.clone()));
        assert!(pos(&boosted).unwrap_or(99) <= pos(&plain).unwrap_or(99));
        let favs: BTreeSet<String> = [fast.clone()].into();
        let fav = search("blur", &[], &favs, &[]);
        assert!(pos(&fav).unwrap_or(99) <= pos(&plain).unwrap_or(99));
        // No query: the recently used effects, most recent first; unknown ids are skipped.
        let recent = vec![fast.clone(), "nope.unknown".into()];
        let empty = search("", &recent, &BTreeSet::new(), &[]);
        assert_eq!(empty.iter().map(|h| &h.target).collect::<Vec<_>>(), vec![&Target::Effect(fast)]);
    }

    #[test]
    fn user_presets_are_searched() {
        let p = vec![PresetFile { name: "Bouncy Pop".into(), path: "/x/Bouncy Pop.ecpreset".into() }];
        let hits = search("bouncy", &[], &BTreeSet::new(), &p);
        assert_eq!(hits.first().map(|h| &h.target), Some(&Target::PresetFile("/x/Bouncy Pop.ecpreset".into())));
    }
}
