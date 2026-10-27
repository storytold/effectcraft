//! Home ▸ Templates: the New from Template gallery (engine model in
//! `effectcraft_engine::templates`). Built-in templates are original projects authored in code;
//! user templates come from File ▸ Save as Template… (the config Templates folder). Thumbnails
//! are rendered by our own renderer, one per frame, the first time the gallery shows them.
//!
//! Automation ids: `home.templates.<id>` (card), `home.templates.<id>.open`,
//! `home.templates.<id>.delete` (user templates), `home.templates.saveCurrent`. Deleting a user
//! template removes its file and can't be undone, so the card's delete button asks first
//! (`dialog.deleteTemplate.delete`, `dialog.deleteTemplate.cancel`); agents running
//! `templates.delete` with `engine.execute` are never asked.

use egui::{Align2, Color32, Rect, Sense, Stroke, StrokeKind, pos2, vec2};
use serde_json::json;

use effectcraft_engine::templates::{self, THUMB_H, THUMB_W, TemplateInfo};

use crate::icons::{self, Icon};
use crate::theme::Tokens;
use crate::{Dialog, EffectcraftApp, widgets};

/// The gallery's entries (re-listed when the user template files change).
pub fn entries(app: &mut EffectcraftApp) -> Vec<TemplateInfo> {
    let files = templates::user_files(&app.session);
    if app.template_list.as_ref().map(|(f, _)| f) != Some(&files) {
        let list = templates::list(&app.session);
        app.template_thumbs.retain(|id, _| !id.starts_with("user/") || list.iter().any(|t| &t.id == id));
        app.template_list = Some((files, list));
    }
    app.template_list.as_ref().map(|(_, l)| l.clone()).unwrap_or_default()
}

/// A template's thumbnail texture; renders at most one missing thumbnail per frame (`budget`).
fn thumb(app: &mut EffectcraftApp, ctx: &egui::Context, id: &str, budget: &mut bool) -> Option<egui::TextureHandle> {
    if let Some(t) = app.template_thumbs.get(id) {
        return t.clone();
    }
    if !*budget {
        ctx.request_repaint();
        return None;
    }
    *budget = false;
    let tex = templates::thumbnail(&app.session, id).ok().and_then(|t| templates::decode_thumb(&t)).map(|rgb| {
        let px = rgb.as_chunks::<3>().0.iter().map(|c| Color32::from_rgb(c[0], c[1], c[2])).collect();
        ctx.load_texture(format!("template-thumb-{id}"), egui::ColorImage::new([THUMB_W, THUMB_H], px), egui::TextureOptions::LINEAR)
    });
    app.template_thumbs.insert(id.to_string(), tex.clone());
    ctx.request_repaint();
    tex
}

fn fmt_secs(s: f64) -> String {
    if (s - s.round()).abs() < 0.05 { format!("{} s", s.round()) } else { format!("{s:.1} s") }
}

/// The Templates tab of the Home screen.
pub fn home_tab(app: &mut EffectcraftApp, ui: &mut egui::Ui, area: Rect) {
    let t = app.tokens;
    let ctx = ui.ctx().clone();
    let p = ui.painter().with_clip_rect(area);
    let (x, y, w) = (area.min.x, area.min.y, area.width());
    p.text(pos2(x, y + 6.0), Align2::LEFT_CENTER, crate::i18n::tr("New from Template"), Tokens::semibold(16.0), Color32::WHITE);
    p.text(
        pos2(x, y + 26.0),
        Align2::LEFT_CENTER,
        "Start from a ready-made project. Opening a template makes an untitled copy.",
        Tokens::ui(11.5),
        t.text_faint,
    );
    // Save the open project as a user template.
    let can_save = app.session.project.comps().next().is_some();
    let sr = Rect::from_min_size(pos2(area.max.x - 210.0, y - 4.0), vec2(210.0, 28.0));
    let save = widgets::text_button(ui, sr, "Save Current as Template…", false, &t, egui::Id::unique("home-templates-save")).clicked();
    app.auto.add("home.templates.saveCurrent", sr, "Save Current as Template…");
    if save {
        if can_save {
            if let Err(e) = crate::menus::invoke(app, &ctx, "templates.saveAs", json!({})) {
                app.ui.status = e;
            }
        } else {
            app.ui.status = "Make a composition first: a template is saved from the open project.".into();
        }
    }
    let list = entries(app);
    let top = y + 46.0;
    let gap = 16.0;
    let card_w = 220.0f32.min(w);
    let cols = (((w + gap) / (card_w + gap)).floor() as usize).max(1);
    let card_w = (w - gap * (cols - 1) as f32) / cols as f32;
    let thumb_h = card_w * THUMB_H as f32 / THUMB_W as f32;
    let card_h = thumb_h + 62.0;
    let view = Rect::from_min_max(pos2(x, top), area.max);
    let mut open: Option<String> = None;
    let mut delete: Option<(String, String)> = None;
    let mut budget = true;
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(view).layout(egui::Layout::top_down(egui::Align::Min)));
    egui::ScrollArea::vertical().id_salt("home-templates-scroll").auto_shrink([false, false]).show(&mut child, |ui| {
        let rows = list.len().div_ceil(cols);
        let (full, _) = ui.allocate_exact_size(vec2(view.width() - 8.0, rows as f32 * (card_h + gap)), Sense::hover());
        let p = ui.painter().with_clip_rect(ui.clip_rect());
        for (i, tpl) in list.iter().enumerate() {
            let (cx, cy) = (full.min.x + (i % cols) as f32 * (card_w + gap), full.min.y + (i / cols) as f32 * (card_h + gap));
            let r = Rect::from_min_size(pos2(cx, cy), vec2(card_w, card_h));
            let visible = r.intersects(ui.clip_rect());
            let resp = ui.interact(r, egui::Id::unique(("template-card", &tpl.id)), Sense::click()).on_hover_text(&tpl.description);
            p.rect_filled(r, 8.0, if resp.hovered() { Color32::from_rgb(0x2c, 0x2e, 0x37) } else { Color32::from_rgb(0x24, 0x26, 0x2e) });
            let tr = Rect::from_min_size(r.min, vec2(card_w, thumb_h));
            let tex = if visible { thumb(app, &ctx, &tpl.id, &mut budget) } else { None };
            match tex {
                Some(tex) => {
                    p.image(tex.id(), tr, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
                }
                None => {
                    p.rect_filled(tr, 6.0, Color32::from_rgb(0x1e, 0x20, 0x27));
                    icons::paint(&p, Rect::from_center_size(tr.center(), vec2(22.0, 22.0)), Icon::Comp, t.text_faint);
                }
            }
            p.rect_stroke(tr, 6.0, Stroke::new(1.0, if resp.hovered() { t.accent } else { t.separator }), StrokeKind::Inside);
            // Category chip (built-in) or "Mine".
            let chip = if tpl.builtin { tpl.category.clone() } else { format!("{} · Mine", tpl.category) };
            let g = p.layout_no_wrap(chip, Tokens::medium(10.0), Color32::WHITE);
            let cr = Rect::from_min_size(tr.min + vec2(8.0, 8.0), g.size() + vec2(12.0, 4.0));
            p.rect_filled(cr, 8.0, Color32::from_black_alpha(150));
            p.galley(cr.min + vec2(6.0, 2.0), g, Color32::WHITE);
            let text = p.with_clip_rect(r.shrink(2.0));
            text.text(pos2(r.min.x + 10.0, tr.max.y + 16.0), Align2::LEFT_CENTER, &tpl.name, Tokens::semibold(13.0), Color32::WHITE);
            let meta = format!("{}×{} · {} · {:.2} fps", tpl.width, tpl.height, fmt_secs(tpl.duration), tpl.frame_rate);
            text.text(pos2(r.min.x + 10.0, tr.max.y + 34.0), Align2::LEFT_CENTER, meta, Tokens::ui(11.0), t.text_dim);
            let ctl = match tpl.controls.len() {
                0 => "No Essential Graphics controls".to_string(),
                1 => format!("Essential Graphics: {}", tpl.controls[0]),
                n => format!("{n} Essential Graphics controls"),
            };
            text.text(pos2(r.min.x + 10.0, tr.max.y + 50.0), Align2::LEFT_CENTER, ctl, Tokens::ui(10.5), t.text_faint);
            let base = format!("home.templates.{}", tpl.id);
            app.auto.add(&base, r, &tpl.name);
            app.auto.add(&format!("{base}.open"), tr, &format!("New project from {}", tpl.name));
            let mut del_hit = false;
            if !tpl.builtin {
                let dr = Rect::from_min_size(pos2(tr.max.x - 30.0, tr.min.y + 6.0), vec2(24.0, 24.0));
                let dresp = ui.interact(dr, egui::Id::unique(("template-del", &tpl.id)), Sense::click()).on_hover_text(crate::i18n::tr("Delete this template"));
                if resp.hovered() || dresp.hovered() {
                    p.circle_filled(dr.center(), 12.0, if dresp.hovered() { Color32::from_rgb(0xc0, 0x3a, 0x3a) } else { Color32::from_black_alpha(170) });
                    icons::paint(&p, dr.shrink(6.0), Icon::Close, Color32::WHITE);
                }
                app.auto.add(&format!("{base}.delete"), dr, "Delete Template");
                if dresp.clicked() {
                    del_hit = true;
                    delete = Some((tpl.id.clone(), tpl.name.clone()));
                }
            }
            if resp.clicked() && !del_hit {
                open = Some(tpl.id.clone());
            }
        }
    });
    if let Some(pending) = delete {
        app.dialog_state.delete_template = Some(pending);
        app.dialog = Some(Dialog::DeleteTemplate);
    } else if let Some(id) = open {
        // A modified project asks to save first (the template opens once answered).
        if super::unsaved::guard(app, "templates.create", &json!({"id": id})) {
            return;
        }
        match app.session.execute("templates.create", json!({"id": id})) {
            Ok(_) => app.ui.start_screen = false,
            Err(e) => app.ui.status = e.to_string(),
        }
    }
}

/// Delete Template: the user template waiting on the prompt (`dialog_state.delete_template`, its
/// id and name).
pub fn show_delete(app: &mut EffectcraftApp, ctx: &egui::Context, t: &Tokens) {
    let Some((id, name)) = app.dialog_state.delete_template.clone() else {
        app.dialog = None;
        return;
    };
    let mut answer: Option<bool> = None;
    super::dialogs::modal(ctx, "Delete Template", vec2(460.0, 150.0), t, |ui| {
        ui.label(egui::RichText::new(format!("Delete the template “{name}”?")).color(t.tab_text_active));
        ui.add_space(4.0);
        ui.label(egui::RichText::new(crate::i18n::tr("You can't undo this.")).color(t.text_dim).size(11.0));
        ui.add_space(18.0);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let r = ui.add(egui::Button::new(egui::RichText::new(crate::i18n::tr("  Delete  ")).color(Color32::WHITE)).fill(t.danger));
            app.auto.add("dialog.deleteTemplate.delete", r.rect, "Delete");
            if r.clicked() {
                answer = Some(true);
            }
            let r = ui.button(crate::i18n::tr("Cancel"));
            app.auto.add("dialog.deleteTemplate.cancel", r.rect, "Cancel");
            if r.clicked() {
                answer = Some(false);
            }
        });
    });
    if answer.is_none() && ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        answer = Some(false);
    }
    // Cmd+D (Ctrl elsewhere) is Delete.
    if answer.is_none() && super::dialogs::ui_command_key(ctx, egui::Key::D) {
        answer = Some(true);
    }
    let Some(delete) = answer else { return };
    app.dialog = None;
    app.dialog_state.delete_template = None;
    if delete {
        match app.session.execute("templates.delete", json!({"id": id})) {
            Ok(_) => {
                app.template_thumbs.remove(&id);
            }
            Err(e) => app.ui.status = e.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(app: &mut EffectcraftApp, ctx: &egui::Context) {
        ctx.run_ui(Default::default(), |ui| {
            app.auto.begin_frame();
            if app.ui.start_screen {
                crate::panels::home::show(app, ui, Rect::from_min_size(pos2(0.0, 0.0), vec2(1280.0, 860.0)));
            }
            crate::panels::dialogs::show(app, ui.ctx());
        })
        .textures_delta
        .clear();
    }

    /// Click the widget registered as `id`, then draw the frame that follows.
    fn click(app: &mut EffectcraftApp, ctx: &egui::Context, id: &str) {
        let [x, y, w, h] = app.auto.find(id).unwrap_or_else(|| panic!("{id} is not on screen")).rect;
        let pos = pos2(x + w / 2.0, y + h / 2.0);
        let button = |pressed| egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() };
        for event in [egui::Event::PointerMoved(pos), button(true), button(false)] {
            ctx.run_ui(egui::RawInput { events: vec![event], ..Default::default() }, |ui| {
                app.auto.begin_frame();
                if app.ui.start_screen {
                    crate::panels::home::show(app, ui, Rect::from_min_size(pos2(0.0, 0.0), vec2(1280.0, 860.0)));
                }
                crate::panels::dialogs::show(app, ui.ctx());
            })
            .textures_delta
            .clear();
        }
        // Two more: a new dialog is laid out, and the automation ids of the clicked frame age out.
        frame(app, ctx);
        frame(app, ctx);
    }

    fn app_with_store() -> EffectcraftApp {
        let s = effectcraft_engine::Session { config: Some(std::sync::Arc::new(effectcraft_engine::config::MemoryConfig::default())), ..Default::default() };
        EffectcraftApp::new(s)
    }

    #[test]
    fn gallery_lists_renders_and_opens_templates() {
        let mut app = app_with_store();
        let ctx = egui::Context::default();
        crate::theme::install(&ctx, &app.tokens, crate::i18n::language(&app));
        // File ▸ New ▸ New Project from Template… opens the Templates tab.
        let ev = app.session.execute("file.newFromTemplate", json!({})).unwrap();
        assert_eq!(ev["frontend"], "file.newFromTemplate");
        crate::menus::invoke(&mut app, &ctx, "file.newFromTemplate", json!({})).unwrap();
        assert!(app.ui.start_screen && app.ui.home_templates && !app.ui.home_learn);
        // One thumbnail per frame until all are rendered.
        for _ in 0..12 {
            frame(&mut app, &ctx);
        }
        let list = entries(&mut app);
        assert!(list.len() >= 8);
        for t in &list {
            assert!(app.auto.find(&format!("home.templates.{}", t.id)).is_some(), "{}", t.id);
            assert!(app.auto.find(&format!("home.templates.{}.open", t.id)).is_some(), "{}", t.id);
            assert!(app.template_thumbs.get(&t.id).is_some_and(Option::is_some), "{}: no thumbnail", t.id);
        }
        for id in ["home.templates.saveCurrent", "home.tab.templates", "home.tab.learn"] {
            assert!(app.auto.find(id).is_some(), "{id}");
        }
        assert!(app.auto.find("home.recent.0").is_none());
        // Save as Template… opens its form (once there is a comp).
        app.session.execute("templates.create", json!({"id": "logo-reveal"})).unwrap();
        crate::menus::invoke(&mut app, &ctx, "templates.saveAs", json!({})).unwrap();
        assert!(matches!(app.dialog, Some(crate::Dialog::Form)));
        assert_eq!(app.dialog_state.form.command, "templates.saveAs");
        app.dialog_state.form.fields[0].kind = crate::panels::forms::FieldKind::Text("Brand Intro".into());
        frame(&mut app, &ctx);
        for id in ["form.field.name", "form.field.description", "form.field.embedFootage", "form.field.embedLimitMB", "form.ok", "form.cancel"] {
            assert!(app.auto.find(id).is_some(), "{id}");
        }
        let params = app.dialog_state.form.params();
        assert_eq!((params["embedFootage"].as_bool(), params["embedLimitMB"].as_f64()), (Some(true), Some(256.0)), "{params}");
        app.session.execute("templates.saveAs", params).unwrap();
        app.dialog = None;
        app.ui.start_screen = true;
        for _ in 0..3 {
            frame(&mut app, &ctx);
        }
        let mine = "user/Brand Intro.ectemplate";
        assert!(app.auto.find(&format!("home.templates.{mine}.delete")).is_some());
        assert!(app.template_thumbs.get(mine).is_some_and(Option::is_some));
        // Opening makes an untitled copy.
        app.session.execute("templates.create", json!({"id": mine})).unwrap();
        assert!(app.session.path.is_none());
        assert!(app.session.active_comp().is_some_and(|c| c.layers.iter().any(|l| l.name == "BRAND NAME")));
        // The card's delete button asks first: the file can't be brought back.
        app.ui.start_screen = true;
        frame(&mut app, &ctx);
        click(&mut app, &ctx, &format!("home.templates.{mine}.delete"));
        assert_eq!(app.dialog, Some(Dialog::DeleteTemplate));
        assert!(app.session.path.is_none() && entries(&mut app).iter().any(|t| t.id == mine), "nothing deleted or opened yet");
        click(&mut app, &ctx, "dialog.deleteTemplate.cancel");
        assert!(app.dialog.is_none() && entries(&mut app).iter().any(|t| t.id == mine), "Cancel keeps the template");
        // Enter is not Delete; Cmd+D is.
        click(&mut app, &ctx, &format!("home.templates.{mine}.delete"));
        crate::panels::dialogs::press_key(&mut app, &ctx, egui::Key::Enter, egui::Modifiers::NONE);
        assert_eq!(app.dialog, Some(Dialog::DeleteTemplate));
        crate::panels::dialogs::press_key(&mut app, &ctx, egui::Key::D, egui::Modifiers::COMMAND);
        assert!(app.dialog.is_none() && app.dialog_state.delete_template.is_none());
        frame(&mut app, &ctx);
        assert!(app.auto.find(&format!("home.templates.{mine}")).is_none());
        assert!(!entries(&mut app).iter().any(|t| t.id == mine));
    }

    #[test]
    fn my_custom_rgb_dialog() {
        let mut app = app_with_store();
        let ctx = egui::Context::default();
        crate::theme::install(&ctx, &app.tokens, crate::i18n::language(&app));
        // Menu check marks follow the active comp's viewer.
        app.session.execute("comp.new", json!({"name": "C", "width": 64, "height": 48})).unwrap();
        let item = crate::menus::menu_items(&app).into_iter().find(|m| m.id == "view.customRgb").expect("menu item");
        assert_eq!(item.label, "My Custom RGB...");
        assert_eq!(item.checked, Some(false));
        crate::menus::invoke(&mut app, &ctx, "view.customRgb", json!({})).unwrap();
        assert!(matches!(app.dialog, Some(crate::Dialog::Form)));
        frame(&mut app, &ctx);
        for id in ["form.field.red[0]", "form.field.white[1]", "form.field.gamma", "form.field.icc", "form.field.icc.browse", "form.ok"] {
            assert!(app.auto.find(id).is_some(), "{id}");
        }
        let mut p = app.dialog_state.form.params();
        assert_eq!(p["red"], json!([0.64, 0.33]));
        p["gamma"] = json!(1.8);
        app.session.execute("view.customRgb", p).unwrap();
        assert_eq!(app.session.prefs.custom_rgb.gamma, 1.8);
        assert_eq!(app.session.state.viewer.simulation.profile, effectcraft_engine::viewer::SimProfile::MyCustom);
        let item = crate::menus::menu_items(&app).into_iter().find(|m| m.id == "view.customRgb").unwrap();
        assert_eq!(item.checked, Some(true));
    }
}
