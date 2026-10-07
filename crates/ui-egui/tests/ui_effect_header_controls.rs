//! Synthetic pointer input: compact modern controls must keep their original commands usable.
use effectcraft_engine::{Session, project::LayerId};
use effectcraft_ui_egui::{
    EffectcraftApp,
    panels::effect_controls,
    theme::{ThemeKind, Tokens},
};
use egui::{Context, Event, Modifiers, PointerButton, RawInput, Rect, pos2, vec2};
use serde_json::json;

fn draw(app: &mut EffectcraftApp, ctx: &Context, rect: Rect, events: Vec<Event>) {
    app.auto.begin_frame();
    ctx.run_ui(RawInput { events, ..Default::default() }, |ui| effect_controls::show(app, ui, rect)).drop_without_applying_deltas();
}

fn click(app: &mut EffectcraftApp, ctx: &Context, rect: Rect, id: &str) {
    let e = app.auto.elements.iter().find(|e| e.id == id).unwrap();
    let pos = pos2(e.rect[0] + e.rect[2] / 2.0, e.rect[1] + e.rect[3] / 2.0);
    draw(
        app,
        ctx,
        rect,
        vec![Event::PointerMoved(pos), Event::PointerButton { pos, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE }],
    );
    draw(app, ctx, rect, vec![Event::PointerButton { pos, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE }]);
}

#[test]
fn compact_effect_controls_toggle_reset_and_undo_in_dark_and_light() {
    for kind in [ThemeKind::Dark, ThemeKind::Light] {
        let mut session = Session::default();
        session.execute("comp.new", json!({"width":2,"height":2,"duration":1})).unwrap();
        let lid = session.execute("layer.newSolid", json!({"width":2,"height":2})).unwrap()["layer"].as_u64().unwrap();
        let uid = session.execute("effect.apply", json!({"effect":"ec.blur.gaussian"})).unwrap()["effects"][0].as_u64().unwrap();
        let prop = session
            .project
            .comp(session.active_comp_id().unwrap())
            .unwrap()
            .layer(LayerId(lid))
            .unwrap()
            .effects()
            .unwrap()
            .groups()
            .find(|g| g.uid == uid)
            .unwrap()
            .get("blurriness")
            .unwrap()
            .uid;
        session.execute("prop.set", json!({"layer":lid,"prop":prop,"value":27.0})).unwrap();
        let mut app = EffectcraftApp::new(session);
        app.tokens = Tokens::for_kind(kind);
        let ctx = Context::default();
        effectcraft_ui_egui::theme::install(&ctx, &app.tokens);
        let rect = Rect::from_min_size(pos2(20.0, 20.0), vec2(180.0, 500.0));
        for _ in 0..3 {
            draw(&mut app, &ctx, rect, vec![]);
        }
        for suffix in ["fx", "reset", "about"] {
            let id = format!("effectControls.effect.{uid}.{suffix}");
            let e = app.auto.elements.iter().find(|e| e.id == id).unwrap();
            let control = Rect::from_min_size(pos2(e.rect[0], e.rect[1]), vec2(e.rect[2], e.rect[3]));
            assert!(rect.contains_rect(control), "{id} must fit the narrow panel");
            assert!(control.width() <= 22.0, "{id} should leave space for the effect name");
        }
        let effect = |app: &EffectcraftApp| {
            app.session
                .project
                .comp(app.session.active_comp_id().unwrap())
                .unwrap()
                .layer(LayerId(lid))
                .unwrap()
                .effects()
                .unwrap()
                .groups()
                .find(|g| g.uid == uid)
                .unwrap()
                .clone()
        };
        click(&mut app, &ctx, rect, &format!("effectControls.effect.{uid}.fx"));
        assert!(!effect(&app).enabled);
        app.session.undo();
        assert!(effect(&app).enabled);
        draw(&mut app, &ctx, rect, vec![]);
        click(&mut app, &ctx, rect, &format!("effectControls.effect.{uid}.reset"));
        assert_eq!(effect(&app).get("blurriness").unwrap().value.as_f64(), 0.0);
        app.session.undo();
        assert_eq!(effect(&app).get("blurriness").unwrap().value.as_f64(), 27.0);
        assert!(app.ui.status.is_empty(), "{}", app.ui.status);
    }
}

#[test]
fn tiny_effect_headers_do_not_overlap_action_targets() {
    for width in [65.0, 70.0, 86.0, 100.0, 180.0] {
        let mut session = Session::default();
        session.execute("comp.new", json!({"width":2,"height":2,"duration":1})).unwrap();
        session.execute("layer.newSolid", json!({"width":2,"height":2})).unwrap();
        let uid = session.execute("effect.apply", json!({"effect":"ec.blur.gaussian"})).unwrap()["effects"][0].as_u64().unwrap();
        let mut app = EffectcraftApp::new(session);
        let ctx = Context::default();
        effectcraft_ui_egui::theme::install(&ctx, &app.tokens);
        let rect = Rect::from_min_size(pos2(20.0, 20.0), vec2(width, 500.0));
        for _ in 0..3 {
            draw(&mut app, &ctx, rect, vec![]);
        }
        let controls: Vec<_> = ["fx", "twirl", "label", "reset", "about"]
            .into_iter()
            .filter_map(|suffix| {
                let id = format!("effectControls.effect.{uid}.{suffix}");
                app.auto.elements.iter().find(|e| e.id == id).map(|e| (id, Rect::from_min_size(pos2(e.rect[0], e.rect[1]), vec2(e.rect[2], e.rect[3]))))
            })
            .collect();
        for (index, (id, r)) in controls.iter().enumerate() {
            assert!(rect.contains_rect(*r), "{width}: {id} escapes panel");
            for (other_id, other) in controls.iter().skip(index + 1) {
                assert!(!r.intersects(*other), "{width}: {id} overlaps {other_id}");
            }
        }
    }
}
