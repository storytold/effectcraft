//! The macOS menu bar takes the Edit menu's key equivalents before any window sees them: while a
//! text field has the keyboard, in a docked panel or a floating panel's own OS window, they edit
//! the field and leave the project alone, and the items stay enabled.

use effectcraft_engine::Session;
use effectcraft_ui_egui::EffectcraftApp;
use effectcraft_ui_egui::{menus, native_menu};
use egui::{Event, Modifiers, PointerButton, Pos2, ViewportCommand, ViewportId, ViewportInfo, pos2, vec2};
use egui_kittest::Harness;
use serde_json::json;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

fn app() -> EffectcraftApp {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Main", "width": 320, "height": 180, "duration": 4})).unwrap();
    s.execute("layer.newSolid", json!({"name": "Solid", "color": "#406080"})).unwrap();
    let mut app = EffectcraftApp::new(s);
    app.ui.start_screen = false;
    app.hooks.clipboard_text = Some(Box::new(|| Some("pasted".into())));
    app
}

/// What the Edit commands would change in the project.
fn project(app: &EffectcraftApp) -> (u64, usize, usize, usize) {
    let s = &app.session;
    (s.revision, s.state.clipboard.len(), s.history.undo.len(), s.history.redo.len())
}

/// Choose the native menu item that runs `command`, then deliver what it queued for the main
/// window (kittest does not call `raw_input_hook`).
fn menu(h: &mut Harness<'_, EffectcraftApp>, command: &str) {
    let spec = native_menu::build(h.state());
    let id = spec.items().into_iter().find(|i| i.command == command).unwrap().id.clone();
    let ctx = h.ctx.clone();
    native_menu::activate(h.state_mut(), &ctx, &spec, &id).unwrap();
    for _ in 0..3 {
        let mut raw = egui::RawInput::default();
        eframe::App::raw_input_hook(h.state_mut(), &ctx, &mut raw);
        h.input_mut().events.extend(raw.events);
        h.step();
        h.state_mut().mark_synthetic_applied();
    }
    h.run_steps(2);
}

/// Whether the menu bar shows `command`'s item enabled now, and whether the project alone would.
fn enabled(h: &Harness<'_, EffectcraftApp>, command: &str) -> (bool, bool) {
    let find = |menu: native_menu::NativeMenu| menu.items().into_iter().find(|i| i.command == command).unwrap().enabled;
    (find(native_menu::build_now(h.state(), &h.ctx)), find(native_menu::build(h.state())))
}

fn center(h: &Harness<'_, EffectcraftApp>, id: &str) -> Pos2 {
    let e = h.state().auto.find(id).unwrap_or_else(|| panic!("no {id}")).clone();
    pos2(e.rect[0] + e.rect[2] / 2.0, e.rect[1] + e.rect[3] / 2.0)
}

fn click(h: &mut Harness<'_, EffectcraftApp>, at: Pos2, window: Option<&Host>) {
    let send = |h: &mut Harness<'_, EffectcraftApp>, event: Event| match window {
        Some(host) => to_window(host, event),
        None => h.input_mut().events.push(event),
    };
    send(h, Event::PointerMoved(at));
    h.step();
    for pressed in [true, false] {
        send(h, Event::PointerButton { pos: at, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE });
        h.step();
    }
}

/// Stand-in OS windows for floating panels: each runs its own pass with its own input, and
/// asking one for the clipboard text gives it "pasted".
#[derive(Default)]
struct Windows {
    window: Option<ViewportId>,
    events: HashMap<ViewportId, Vec<Event>>,
}

type Host = Rc<RefCell<Windows>>;

fn to_window(host: &Host, event: Event) {
    let mut host = host.borrow_mut();
    let Some(window) = host.window else { panic!("no floating panel window") };
    host.events.entry(window).or_default().push(event);
}

fn native_windows(h: &mut Harness<'_, EffectcraftApp>) -> Host {
    let host = Host::default();
    let shared = host.clone();
    egui::Context::set_immediate_viewport_renderer(move |ctx, mut viewport| {
        let id = viewport.ids.this;
        let size = viewport.builder.inner_size.unwrap_or(vec2(400.0, 300.0));
        let time = ctx.input(|i| i.time);
        let events = {
            let mut host = shared.borrow_mut();
            host.window = Some(id);
            host.events.remove(&id).unwrap_or_default()
        };
        let viewports = [(ViewportId::ROOT, false), (id, true)]
            .into_iter()
            .map(|(id, focused)| (id, ViewportInfo { focused: Some(focused), native_pixels_per_point: Some(1.0), ..Default::default() }))
            .collect();
        let input = egui::RawInput {
            viewport_id: id,
            viewports,
            screen_rect: Some(egui::Rect::from_min_size(Pos2::ZERO, size)),
            events,
            focused: true,
            time: Some(time),
            ..Default::default()
        };
        let mut full = ctx.run_ui(input, |ui| (viewport.viewport_ui_cb)(ui));
        full.textures_delta.clear();
        if full.viewport_output.get(&id).is_some_and(|out| out.commands.contains(&ViewportCommand::RequestPaste)) {
            shared.borrow_mut().events.entry(id).or_default().push(Event::Paste("pasted".into()));
        }
    });
    h.ctx.set_embed_viewports(false);
    // A main window that reports its place: not Wayland, where floating panels stay in it.
    let screen = h.ctx.content_rect();
    h.input_mut().viewports.entry(ViewportId::ROOT).or_default().outer_rect = Some(screen);
    h.run_steps(3);
    let window = host.borrow().window.expect("the floating panel opens in an OS window of its own");
    // That window has the keyboard.
    let input = h.input_mut();
    input.focused = false;
    for (id, focused) in [(ViewportId::ROOT, false), (window, true)] {
        input.viewports.entry(id).or_default().focused = Some(focused);
    }
    h.run_steps(2);
    host
}

#[test]
fn edit_menu_keys_edit_a_text_field_in_a_panel_window() {
    let mut app = app();
    menus::invoke(&mut app, &egui::Context::default(), "window.panelWindow", json!({"panel": "Project", "window": true})).unwrap();
    let mut h = Harness::builder().with_size(vec2(1600.0, 1000.0)).build_eframe(|_| app);
    h.ctx.set_os(egui::os::OperatingSystem::Mac);
    let host = native_windows(&mut h);
    let at = center(&h, "project.search");
    click(&mut h, at, Some(&host));
    to_window(&host, Event::Text("abc".into()));
    h.run_steps(3);
    let window = host.borrow().window.unwrap();
    assert_eq!(h.state().ui.project_search, "abc");
    assert!(!h.ctx.egui_wants_keyboard_input(), "the main window has no focused widget");
    assert_eq!(native_menu::text_field_window(&h.ctx), Some(window));
    assert_eq!(enabled(&h, "edit.paste"), (true, false), "Paste stays enabled with nothing copied in the project");
    let before = project(h.state());
    menu(&mut h, "edit.selectAll");
    menu(&mut h, "edit.cut");
    assert_eq!(h.state().ui.project_search, "", "Select All and Cut act on the field");
    menu(&mut h, "edit.paste");
    assert_eq!(h.state().ui.project_search, "pasted");
    menu(&mut h, "edit.undo");
    assert_eq!(h.state().ui.project_search, "", "Undo undoes the paste");
    menu(&mut h, "edit.copy");
    assert_eq!(project(h.state()), before, "the project is untouched");
    assert_eq!(h.state().ui.project_search, "");
}

#[test]
fn edit_menu_keys_edit_a_docked_text_field() {
    let mut h = Harness::builder().with_size(vec2(1600.0, 1000.0)).build_eframe(|_| app());
    h.ctx.set_os(egui::os::OperatingSystem::Mac);
    h.run_steps(3);
    let at = center(&h, "project.search");
    h.input_mut().events.push(Event::PointerMoved(at));
    h.step();
    for pressed in [true, false] {
        h.input_mut().events.push(Event::PointerButton { pos: at, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE });
        h.step();
    }
    h.input_mut().events.push(Event::Text("abc".into()));
    h.run_steps(3);
    assert_eq!(h.state().ui.project_search, "abc");
    assert_eq!(native_menu::text_field_window(&h.ctx), Some(ViewportId::ROOT));
    assert_eq!(enabled(&h, "edit.paste"), (true, false), "Paste stays enabled with nothing copied in the project");
    let before = project(h.state());
    menu(&mut h, "edit.selectAll");
    menu(&mut h, "edit.cut");
    assert_eq!(h.state().ui.project_search, "", "Select All and Cut act on the field");
    menu(&mut h, "edit.paste");
    assert_eq!(h.state().ui.project_search, "pasted");
    menu(&mut h, "edit.undo");
    assert_eq!(h.state().ui.project_search, "", "Undo undoes the paste");
    menu(&mut h, "edit.copy");
    assert_eq!(project(h.state()), before, "the project is untouched");
}
