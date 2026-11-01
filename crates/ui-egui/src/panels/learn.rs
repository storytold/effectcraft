//! Learn: the Home screen's tutorial list and the coach card that walks through a running
//! tutorial (engine model in `effectcraft_engine::learn`). The coach highlights the step's target
//! (an automation id) and moves on when the user performs the step's command; "Show me" runs it.

use crate::i18n::{tr, tr_args};
use egui::{Align2, Color32, Rect, Sense, Stroke, StrokeKind, pos2, vec2};
use serde_json::json;

use crate::theme::Tokens;
use crate::{EffectcraftApp, widgets};

/// The Learn tab of the Home screen: one card per tutorial with its steps and a Start button.
pub fn home_tab(app: &mut EffectcraftApp, ui: &mut egui::Ui, area: Rect) {
    let t = app.tokens;
    let p = ui.painter().with_clip_rect(area);
    let (x, mut y, w) = (area.min.x, area.min.y, area.width());
    p.text(pos2(x, y + 6.0), Align2::LEFT_CENTER, tr("Learn"), Tokens::semibold(16.0), Color32::WHITE);
    p.text(
        pos2(x, y + 26.0),
        Align2::LEFT_CENTER,
        crate::i18n::tr("Hands-on tutorials that run inside EffectCraft. Do each step yourself, or press Show me."),
        Tokens::ui(11.5),
        t.text_faint,
    );
    y += 44.0;
    let tutorials = effectcraft_engine::learn::tutorials();
    let running = app.session.learn.as_ref().map(|l| l.tutorial.clone());
    let card_h = 128.0;
    for tut in &tutorials {
        let r = Rect::from_min_size(pos2(x, y), vec2(w, card_h - 10.0));
        if r.min.y > area.max.y {
            break;
        }
        let resp = ui.interact(r, egui::Id::unique(("learn-card", &tut.id)), Sense::click());
        p.rect_filled(r, 8.0, if resp.hovered() { Color32::from_rgb(0x2c, 0x2e, 0x37) } else { Color32::from_rgb(0x24, 0x26, 0x2e) });
        // A small original badge: the step count in a circle.
        let badge = Rect::from_min_size(pos2(r.min.x + 16.0, r.min.y + 16.0), vec2(44.0, 44.0));
        p.circle_filled(badge.center(), 22.0, t.accent.gamma_multiply(0.25));
        p.circle_stroke(badge.center(), 22.0, Stroke::new(1.5, t.accent));
        p.text(badge.center() - vec2(0.0, 5.0), Align2::CENTER_CENTER, tut.steps.len().to_string(), Tokens::semibold(15.0), Color32::WHITE);
        p.text(badge.center() + vec2(0.0, 10.0), Align2::CENTER_CENTER, crate::i18n::tr("steps"), Tokens::ui(9.0), t.text_dim);
        let tx = badge.max.x + 16.0;
        p.text(pos2(tx, r.min.y + 22.0), Align2::LEFT_CENTER, &tut.title, Tokens::semibold(14.0), Color32::WHITE);
        p.text(pos2(tx, r.min.y + 40.0), Align2::LEFT_CENTER, format!("{} min", tut.minutes), Tokens::ui(11.0), t.text_faint);
        let text_w = (r.max.x - tx - 120.0).max(120.0);
        let g = p.layout(tut.summary.clone(), Tokens::ui(12.0), t.text_dim, text_w);
        p.galley(pos2(tx, r.min.y + 52.0), g, t.text_dim);
        let steps: Vec<String> = tut.steps.iter().map(|s| s.title.clone()).collect();
        let g = p.layout(steps.join("  ·  "), Tokens::ui(10.5), t.text_faint, text_w);
        p.galley(pos2(tx, r.min.y + 84.0), g, t.text_faint);
        let br = Rect::from_min_size(pos2(r.max.x - 104.0, r.min.y + 16.0), vec2(88.0, 28.0));
        let label = if running.as_deref() == Some(tut.id.as_str()) { "Resume" } else { "Start" };
        let clicked = widgets::text_button(ui, br, label, true, &t, egui::Id::unique(("learn-start", &tut.id))).clicked();
        app.auto.add(&format!("home.learn.{}", tut.id), r, &tut.title);
        app.auto.add(&format!("home.learn.{}.start", tut.id), br, label);
        if clicked || resp.clicked() {
            if running.as_deref() != Some(tut.id.as_str())
                && let Err(e) = app.session.execute("learn.start", json!({"id": tut.id}))
            {
                app.ui.status = e.to_string();
            }
            app.ui.start_screen = false;
        }
        y += card_h;
    }
}

/// The automation element a step's target pattern points at (last frame's registry).
fn target_rect(app: &EffectcraftApp, pattern: &str) -> Option<Rect> {
    let lid = app.session.state.selected_layers.first().map(|l| l.0);
    let pat = effectcraft_engine::learn::resolve_target(pattern, lid);
    app.auto
        .previous
        .iter()
        .find(|e| effectcraft_engine::learn::target_matches(&pat, &e.id) && e.rect[2] > 0.0 && e.rect[3] > 0.0)
        .map(|e| Rect::from_min_size(pos2(e.rect[0], e.rect[1]), vec2(e.rect[2], e.rect[3])))
}

/// The coach card (bottom right) and the highlight around the current step's target.
pub fn coach(app: &mut EffectcraftApp, ctx: &egui::Context) {
    let Some((tut, i)) = app.session.learn_current() else { return };
    let t = app.tokens;
    let n = tut.steps.len();
    let screen = ctx.content_rect();
    let step = tut.steps.get(i).cloned();
    // Highlight the target (pulsing outline over everything but the card).
    if app.ui.start_screen {
        // The Home screen covers the panels: just the card.
    } else if let Some(st) = &step
        && let Some(tp) = &st.target
        && let Some(r) = target_rect(app, tp)
    {
        let time = ctx.input(|i| i.time);
        let pulse = (0.5 + 0.5 * (time * 4.0).sin()) as f32;
        let p = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::unique("learn-highlight")));
        let hr = r.expand(3.0 + 2.0 * pulse);
        p.rect_stroke(hr, 5.0, Stroke::new(2.0, t.accent.gamma_multiply(0.6 + 0.4 * pulse)), StrokeKind::Outside);
        app.auto.add("learn.highlight", hr, tp);
        ctx.request_repaint_after(std::time::Duration::from_millis(33));
    }
    let w = 340.0;
    let mut action: Option<&str> = None;
    let area = egui::Area::new(egui::Id::unique("learn-coach"))
        .order(egui::Order::Foreground)
        .fixed_pos(pos2(screen.max.x - w - 20.0, screen.max.y - 250.0))
        .show(ctx, |ui| {
            egui::Frame::new()
                .fill(Color32::from_rgb(0x26, 0x28, 0x30))
                .stroke(Stroke::new(1.0, t.accent.gamma_multiply(0.7)))
                .corner_radius(8.0)
                .inner_margin(egui::Margin::same(14))
                .shadow(egui::epaint::Shadow { offset: [0, 4], blur: 16, spread: 0, color: Color32::from_black_alpha(140) })
                .show(ui, |ui| {
                    ui.set_width(w - 28.0);
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(tr("LEARN")).font(Tokens::semibold(10.0)).color(t.accent));
                        ui.label(egui::RichText::new(&tut.title).font(Tokens::ui(11.0)).color(t.text_dim));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let (cr, r) = ui.allocate_exact_size(vec2(16.0, 16.0), Sense::click());
                            let r = r.on_hover_text(tr("Close tutorial"));
                            crate::icons::paint(ui.painter(), cr, crate::icons::Icon::Close, if r.hovered() { Color32::WHITE } else { t.text_dim });
                            app.auto.add("learn.close", r.rect, "Close tutorial");
                            if r.clicked() {
                                action = Some("close");
                            }
                        });
                    });
                    // Progress dots.
                    let (dr, _) = ui.allocate_exact_size(vec2(w - 28.0, 10.0), Sense::hover());
                    for k in 0..n {
                        let c = pos2(dr.min.x + 4.0 + k as f32 * 12.0, dr.center().y);
                        let col = if k < i {
                            t.accent
                        } else if k == i {
                            Color32::WHITE
                        } else {
                            Color32::from_gray(80)
                        };
                        ui.painter().circle_filled(c, 3.0, col);
                    }
                    ui.add_space(4.0);
                    match &step {
                        Some(st) => {
                            ui.label(
                                egui::RichText::new(tr_args("Step {} of {}: {}", &[&(i + 1), &n, &st.title]))
                                    .font(Tokens::semibold(13.0))
                                    .color(Color32::WHITE),
                            );
                            ui.add_space(2.0);
                            ui.label(egui::RichText::new(&st.text).font(Tokens::ui(12.0)).color(t.text));
                        }
                        None => {
                            ui.label(egui::RichText::new(tr("Tutorial complete")).font(Tokens::semibold(13.0)).color(Color32::WHITE));
                            ui.label(
                                egui::RichText::new(tr("Nice work. Pick another tutorial on the Home screen's Learn tab."))
                                    .font(Tokens::ui(12.0))
                                    .color(t.text),
                            );
                        }
                    }
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        let back = ui.add_enabled(i > 0, egui::Button::new(tr("Back")));
                        app.auto.add("learn.back", back.rect, "Back");
                        if back.clicked() {
                            action = Some("back");
                        }
                        if step.is_some() {
                            let show = ui.add(egui::Button::new(tr("Show me")));
                            app.auto.add("learn.showMe", show.rect, "Show me");
                            if show.clicked() {
                                action = Some("showMe");
                            }
                            let next = ui.add(egui::Button::new(tr("Next")));
                            app.auto.add("learn.next", next.rect, "Next");
                            if next.clicked() {
                                action = Some("next");
                            }
                        } else {
                            let more = ui.add(egui::Button::new(tr("More tutorials")));
                            app.auto.add("learn.more", more.rect, "More tutorials");
                            if more.clicked() {
                                action = Some("more");
                            }
                        }
                    });
                });
        });
    app.auto.add("learn.coach", area.response.rect, &tut.title);
    match action {
        Some("close") => {
            let _ = app.session.execute("learn.stop", json!({}));
        }
        Some("more") => {
            let _ = app.session.execute("learn.stop", json!({}));
            app.ui.home_learn = true;
            app.ui.home_templates = false;
            app.ui.start_screen = true;
        }
        Some(a) => {
            if let Err(e) = app.session.execute("learn.step", json!({"action": a})) {
                app.ui.status = e.to_string();
            }
        }
        None => {}
    }
}
