//! One confirmation for the queue's existing output files, including sequence frames.

use crate::i18n::tr;
use crate::theme::Tokens;
use crate::{Dialog, EffectcraftApp};
use egui::vec2;
use serde_json::{Value, json};

/// All UI render entry points come here, including a stored queue's Render button.
pub fn request(app: &mut EffectcraftApp, ctx: &egui::Context, mut params: Value) -> Result<Value, String> {
    if app.dialog == Some(Dialog::OverwriteRender) {
        return Ok(json!({"dialog": "overwriteRender"}));
    }
    if !params.is_object() {
        params = json!({});
    }
    if params.get("wait").is_none() {
        params["wait"] = json!(false);
    }
    // UI callers cannot turn a previous answer into a standing overwrite preference.
    if let Some(p) = params.as_object_mut() {
        p.remove("overwrite");
        p.remove("confirmation");
    }
    let review = crate::menus::run_engine(app, ctx, "renderQueue.preflight", json!({}))?;
    params["confirmation"] = review["confirmation"].clone();
    app.show_panel(crate::dock::PanelKind::RenderQueue);
    if review["collisions"].as_array().is_some_and(|p| !p.is_empty()) {
        app.dialog_state.overwrite_render = Some((params, review));
        app.dialog = Some(Dialog::OverwriteRender);
        Ok(json!({"dialog": "overwriteRender"}))
    } else {
        crate::menus::run_engine(app, ctx, "renderQueue.render", params)
    }
}

fn answer(app: &mut EffectcraftApp, ctx: &egui::Context, replace: bool) -> Result<Value, String> {
    let Some((mut params, _)) = app.dialog_state.overwrite_render.take() else { return Err("no render is awaiting confirmation".into()) };
    app.dialog = None;
    if replace {
        params["overwrite"] = json!(true);
        crate::menus::run_engine(app, ctx, "renderQueue.render", params)
    } else {
        crate::menus::run_engine(app, ctx, "renderQueue.cancelPreflight", json!({"confirmation": params["confirmation"]}))
    }
}

pub fn show(app: &mut EffectcraftApp, ctx: &egui::Context, t: &Tokens) {
    let Some(pending) = app.dialog_state.overwrite_render.take() else {
        app.dialog = None;
        return;
    };
    let paths = pending.1["collisions"].as_array().map(Vec::as_slice).unwrap_or_default();
    let mut replace = None;
    super::dialogs::modal(ctx, "Replace existing render outputs?", vec2(580.0, 340.0), t, |ui| {
        let count = paths.len();
        ui.label(format!("This render may replace {} existing {}.", count, if count == 1 { "file" } else { "files" }));
        ui.add_space(8.0);
        egui::ScrollArea::both().max_height(170.0).show_rows(ui, 20.0, paths.len(), |ui, rows| {
            for path in paths.get(rows).unwrap_or_default() {
                let path = path.as_str().unwrap_or_default();
                let name = path.rsplit(['/', '\\']).next().filter(|name| !name.is_empty()).unwrap_or(path);
                ui.add(egui::Label::new(format!("{name}  —  {path}")).wrap_mode(egui::TextWrapMode::Extend)).on_hover_text(path);
            }
        });
        ui.add_space(8.0);
        ui.label(egui::RichText::new(tr("Sequence frames beyond this render's range are kept.")).color(t.text_dim));
        ui.add_space(12.0);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let r = ui.button(tr("Replace and Render"));
            app.auto.add_response("dialog.overwriteRender.replace", &r, "Replace and Render");
            if r.clicked() {
                replace = Some(true);
            }
            let r = ui.button(tr("Cancel"));
            app.auto.add_response("dialog.overwriteRender.cancel", &r, "Cancel");
            if r.clicked() {
                replace = Some(false);
            }
        });
    });
    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        replace = Some(false);
    }
    app.dialog_state.overwrite_render = Some(pending);
    if let Some(replace) = replace {
        let _ = answer(app, ctx, replace);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use effectcraft_engine::project::render_queue::OutputFormat;
    use effectcraft_engine::render_queue::{ExportPreflight, LocalFuture};
    use effectcraft_engine::{ExportJob, ExportResult, Exporter, Session};
    use std::sync::{Arc, Mutex};

    struct Output {
        collisions: Vec<String>,
        calls: Mutex<usize>,
    }
    impl Exporter for Output {
        fn formats(&self) -> Vec<OutputFormat> {
            vec![OutputFormat::PngSequence]
        }
        fn preflight(&self, _: &ExportJob) -> Result<ExportPreflight, String> {
            Ok(ExportPreflight { collisions: self.collisions.clone(), destinations: vec![], authorization: json!({"reviewed": true}) })
        }
        fn export(&self, _: &ExportJob, _: &mut dyn FnMut(u64, u64) -> bool) -> Result<ExportResult, String> {
            Err("unreviewed export".into())
        }
        fn export_authorized_async<'a>(
            &'a self,
            job: &'a ExportJob<'a>,
            token: &'a Value,
            _: &'a mut dyn FnMut(u64, u64) -> bool,
        ) -> LocalFuture<'a, Result<ExportResult, String>> {
            Box::pin(async move {
                assert_eq!(token["reviewed"], true);
                *self.calls.lock().unwrap() += 1;
                Ok(ExportResult { path: job.path.into(), ..Default::default() })
            })
        }
    }

    fn setup(existing: bool) -> (EffectcraftApp, egui::Context, Arc<Output>) {
        let ex = Arc::new(Output { collisions: if existing { vec!["seq_00.png".into(), "seq_01.png".into()] } else { vec![] }, calls: Mutex::new(0) });
        let mut session = Session { exporter: Some(ex.clone()), ..Default::default() };
        session.execute("comp.new", json!({"width": 4, "height": 4, "duration": 0.2})).unwrap();
        session.execute("renderQueue.add", json!({"format": "png", "output": "seq_[##].png"})).unwrap();
        let app = EffectcraftApp::new(session);
        let ctx = egui::Context::default();
        crate::theme::install(&ctx, &app.tokens, crate::i18n::language(&app));
        (app, ctx, ex)
    }

    fn click(app: &mut EffectcraftApp, ctx: &egui::Context, id: &str) {
        let frame = |app: &mut EffectcraftApp, events: Vec<egui::Event>| {
            let raw = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, vec2(1024.0, 768.0))), events, ..Default::default() };
            let _ = ctx.run_ui(raw, |ui| {
                app.auto.begin_frame();
                super::super::dialogs::show(app, ui.ctx());
            });
        };
        for _ in 0..3 {
            frame(app, vec![]);
        }
        let [x, y, w, h] = app.auto.find(id).unwrap().rect;
        let pos = egui::pos2(x + w * 0.5, y + h * 0.5);
        for pressed in [true, false] {
            frame(
                app,
                vec![
                    egui::Event::PointerMoved(pos),
                    egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE },
                ],
            );
        }
    }

    #[test]
    fn stored_queue_sequence_asks_once_and_cancel_does_not_export() {
        let (mut app, ctx, ex) = setup(true);
        // Same path as the real Render button, through the menu dispatcher with wait supplied.
        crate::menus::invoke(&mut app, &ctx, "renderQueue.render", json!({"wait": true})).unwrap();
        assert_eq!(app.dialog, Some(Dialog::OverwriteRender));
        assert_eq!(*ex.calls.lock().unwrap(), 0);
        let id = app.dialog_state.overwrite_render.as_ref().unwrap().1["confirmation"].clone();
        crate::menus::invoke(&mut app, &ctx, "renderQueue.render", json!({"wait": true})).unwrap();
        assert_eq!(app.dialog_state.overwrite_render.as_ref().unwrap().1["confirmation"], id, "repeat clicks keep one review");
        super::super::dialogs::press_key(&mut app, &ctx, egui::Key::Escape, egui::Modifiers::NONE);
        assert!(app.dialog.is_none());
        assert_eq!(*ex.calls.lock().unwrap(), 0);
        assert!(app.session.execute("renderQueue.render", json!({"confirmation": id, "overwrite": true})).is_err());
        crate::menus::invoke(&mut app, &ctx, "renderQueue.render", json!({"wait": true})).unwrap();
        // Enter is deliberately not a destructive answer.
        super::super::dialogs::press_key(&mut app, &ctx, egui::Key::Enter, egui::Modifiers::NONE);
        assert_eq!(app.dialog, Some(Dialog::OverwriteRender));
        click(&mut app, &ctx, "dialog.overwriteRender.replace");
        assert!(app.dialog.is_none());
        assert_eq!(*ex.calls.lock().unwrap(), 1);
        app.session.execute("renderQueue.setRender", json!({"render": true})).unwrap();
        crate::menus::invoke(&mut app, &ctx, "renderQueue.render", json!({"wait": true})).unwrap();
        assert_eq!(app.dialog, Some(Dialog::OverwriteRender), "a saved queue rerender asks again");
    }

    #[test]
    fn fresh_destination_renders_without_a_dialog() {
        let (mut app, ctx, ex) = setup(false);
        crate::menus::invoke(&mut app, &ctx, "renderQueue.render", json!({"wait": true})).unwrap();
        assert!(app.dialog.is_none());
        assert_eq!(*ex.calls.lock().unwrap(), 1);
    }
}
