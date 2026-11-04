//! #2861: Full Screen hides the interface and blows the viewer's panel up to the screen, from
//! View ▸ Enter Full Screen and from a preview with Full Screen on.

use craft_ui::docking::Action;
use effectcraft_engine::Session;
use effectcraft_ui_egui::EffectcraftApp;
use effectcraft_ui_egui::dock::PanelKind;
use egui::{Event, Key, Modifiers, ViewportCommand, ViewportId, ViewportInfo, pos2, vec2};
use egui_kittest::Harness;
use serde_json::json;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

fn app() -> EffectcraftApp {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Clip", "width": 320, "height": 180, "frameRate": 24, "duration": 10})).unwrap();
    s.execute("layer.newSolid", json!({"name": "A", "color": "#406080", "width": 40, "height": 30})).unwrap();
    EffectcraftApp::new(s)
}

fn harness() -> Harness<'static, EffectcraftApp> {
    let app = app();
    let mut h = Harness::builder().with_size(vec2(1600.0, 1000.0)).build_eframe(|_| app);
    h.run_steps(3);
    h
}

fn invoke(h: &mut Harness<'_, EffectcraftApp>, id: &str) -> serde_json::Value {
    let ctx = h.ctx.clone();
    effectcraft_ui_egui::menus::invoke(h.state_mut(), &ctx, id, json!({})).unwrap()
}

fn has(h: &Harness<'_, EffectcraftApp>, id: &str) -> bool {
    h.state().auto.find(id).is_some()
}

fn escape(h: &mut Harness<'_, EffectcraftApp>) {
    for pressed in [true, false] {
        h.input_mut().events.push(Event::Key { key: Key::Escape, physical_key: None, pressed, repeat: false, modifiers: Modifiers::NONE });
    }
    h.step();
}

fn comp_size(h: &Harness<'_, EffectcraftApp>) -> [f32; 2] {
    let e = h.state().auto.find("viewer.comp").expect("the viewer shows the comp");
    [e.rect[2], e.rect[3]]
}

/// The window commands of the pass just run.
fn window_commands(h: &Harness<'_, EffectcraftApp>) -> Vec<ViewportCommand> {
    h.output().viewport_output.get(&ViewportId::ROOT).map(|v| v.commands.clone()).unwrap_or_default()
}

fn os_full_screen(h: &mut Harness<'_, EffectcraftApp>, full: bool) {
    h.input_mut().viewports.entry(ViewportId::ROOT).or_default().fullscreen = Some(full);
}

#[test]
fn full_screen_hides_the_interface_and_keeps_the_video() {
    let mut h = harness();
    for id in ["menu.Edit", "panel.tab.Composition", "panel.tab.Timeline"] {
        assert!(has(&h, id), "{id} shows in the workspace");
    }
    invoke(&mut h, "view.fullScreen");
    h.run_steps(3);
    for id in ["menu.Edit", "panel.tab.Composition", "panel.tab.Timeline", "panel.Timeline", "panel.Composition"] {
        assert!(!has(&h, id), "{id} is hidden in Full Screen");
    }
    assert!(has(&h, "viewer.comp"), "the video stays");
    let left: Vec<String> = h.state().auto.query("viewer.").iter().map(|e| e.id.clone()).collect();
    assert_eq!(left, ["viewer.comp", "viewer.area"], "nothing of the viewer but the video");
    // The whole window is the viewer.
    let area = h.state().auto.find("viewer.area").unwrap().rect;
    assert_eq!(area, [0.0, 0.0, 1600.0, 1000.0]);
    invoke(&mut h, "view.fullScreen");
    h.run_steps(3);
    assert!(has(&h, "menu.Edit") && has(&h, "panel.tab.Composition"), "the interface is back");
}

#[test]
fn full_screen_blows_the_panel_up_and_leaving_puts_the_view_back() {
    let mut h = harness();
    assert_eq!(h.state().ui.viewer.zoom, None, "Fit");
    let fitted = comp_size(&h);
    invoke(&mut h, "view.fullScreen");
    h.run_steps(3);
    assert_eq!(h.state().ui.viewer.zoom, None, "still Fit");
    assert!(comp_size(&h)[0] > fitted[0], "fitted to the larger screen: {:?} from {fitted:?}", comp_size(&h));
    escape(&mut h);
    h.run_steps(3);
    assert_eq!(h.state().ui.viewer.zoom, None, "Fit again");
    assert_eq!(comp_size(&h), fitted);

    // A set zoom and pan scale together, so the same part of the comp shows in the same place.
    h.state_mut().ui.viewer.zoom = Some(0.5);
    h.state_mut().ui.viewer.pan = [30.0, 12.0];
    h.run_steps(2);
    let small = comp_size(&h);
    invoke(&mut h, "view.fullScreen");
    h.run_steps(3);
    let s = comp_size(&h)[0] / small[0];
    assert!(s > 1.0, "{:?} from {small:?}", comp_size(&h));
    let pan = h.state().ui.viewer.pan;
    assert!((pan[0] - 30.0 * s).abs() < 0.01 && (pan[1] - 12.0 * s).abs() < 0.01, "{pan:?} at {s}");
    invoke(&mut h, "view.fullScreen");
    assert!(h.state().full_screen.is_none());
    h.run_steps(2);
    assert_eq!(h.state().ui.viewer.zoom, Some(0.5));
    assert_eq!(h.state().ui.viewer.pan, [30.0, 12.0]);
}

#[test]
fn escape_leaves_full_screen_but_goes_to_a_dialog_first() {
    let mut h = harness();
    h.state_mut().ui.focused = PanelKind::Timeline;
    invoke(&mut h, "view.fullScreen");
    h.run_steps(2);
    assert_eq!(h.state().ui.focused, PanelKind::Composition, "the viewer's shortcuts apply");
    h.state_mut().dialog = Some(effectcraft_ui_egui::Dialog::Info);
    h.run_steps(2);
    escape(&mut h);
    assert!(h.state().dialog.is_none(), "Escape closes the dialog");
    assert!(h.state().full_screen.is_some(), "and only the dialog");
    escape(&mut h);
    assert!(h.state().full_screen.is_none());
    assert_eq!(h.state().ui.focused, PanelKind::Timeline, "focus returns");
}

#[test]
fn the_window_follows_the_mode_and_the_mode_follows_the_window() {
    let mut h = harness();
    os_full_screen(&mut h, false);
    h.run_steps(2);
    invoke(&mut h, "view.fullScreen");
    h.step();
    assert!(window_commands(&h).contains(&ViewportCommand::Fullscreen(true)), "{:?}", window_commands(&h));
    // Not there yet: the window hasn't left full screen, it hasn't reached it.
    h.step();
    assert!(h.state().full_screen.is_some());
    os_full_screen(&mut h, true);
    h.run_steps(2);
    assert!(h.state().full_screen.is_some());
    // Leaving with Escape takes the window along.
    escape(&mut h);
    assert!(window_commands(&h).contains(&ViewportCommand::Fullscreen(false)), "{:?}", window_commands(&h));
    assert!(h.state().full_screen.is_none());
    os_full_screen(&mut h, false);
    h.run_steps(2);

    // The OS takes the window out: the interface comes back, with nothing more to tell it.
    invoke(&mut h, "view.fullScreen");
    h.step();
    os_full_screen(&mut h, true);
    h.run_steps(2);
    os_full_screen(&mut h, false);
    h.step();
    assert!(h.state().full_screen.is_none());
    assert!(!window_commands(&h).iter().any(|c| matches!(c, ViewportCommand::Fullscreen(_))), "{:?}", window_commands(&h));
    h.run_steps(2);
    assert!(has(&h, "menu.Edit"));
}

/// What moves the OS window in the pass just run.
fn moves(h: &Harness<'_, EffectcraftApp>) -> Vec<ViewportCommand> {
    use ViewportCommand as C;
    window_commands(h).into_iter().filter(|c| matches!(c, C::Fullscreen(_) | C::Maximized(_) | C::OuterPosition(_) | C::InnerSize(_))).collect()
}

/// The OS window, as eframe reports it at the start of a pass, on a 1920 x 1080 monitor.
fn os_window(h: &mut Harness<'_, EffectcraftApp>, fullscreen: bool, maximized: bool, outer: egui::Rect) {
    let info = ViewportInfo {
        fullscreen: Some(fullscreen),
        maximized: Some(maximized),
        outer_rect: Some(outer),
        monitor_size: Some(vec2(1920.0, 1080.0)),
        ..Default::default()
    };
    h.input_mut().viewports.insert(ViewportId::ROOT, info);
}

/// Nothing moves the window on the passes that follow: no command repeats.
fn settled(h: &mut Harness<'_, EffectcraftApp>) {
    for _ in 0..4 {
        h.step();
        assert_eq!(moves(h), [], "the window is left alone");
    }
}

fn rect(x: f32, y: f32, w: f32, h: f32) -> egui::Rect {
    egui::Rect::from_min_size(pos2(x, y), vec2(w, h))
}

/// Windows from maximized: full screen first, un-maximized there and put back on the monitor one
/// pass later, without a title bar for the OS to animate. Leaving is one command: winit restores
/// the saved placement, so nothing maximizes the window from its normal size.
#[cfg(windows)]
#[test]
fn a_maximized_window_goes_full_screen_and_back_without_passing_through_its_normal_size() {
    let mut h = harness();
    let maximized = rect(-8.0, -8.0, 1936.0, 1056.0);
    let screen = rect(0.0, 0.0, 1920.0, 1080.0);
    os_window(&mut h, false, true, maximized);
    h.run_steps(2);
    for _ in 0..2 {
        invoke(&mut h, "view.fullScreen");
        h.step();
        assert_eq!(moves(&h), [ViewportCommand::Fullscreen(true)]);
        os_window(&mut h, true, true, screen);
        h.step();
        assert_eq!(
            moves(&h),
            [ViewportCommand::Maximized(false), ViewportCommand::OuterPosition(pos2(0.0, 0.0)), ViewportCommand::InnerSize(vec2(1920.0, 1080.0))]
        );
        os_window(&mut h, true, false, screen);
        settled(&mut h);
        invoke(&mut h, "view.fullScreen");
        h.step();
        assert_eq!(moves(&h), [ViewportCommand::Fullscreen(false)]);
        os_window(&mut h, false, true, maximized);
        settled(&mut h);
    }
}

/// The same from a preview with Full Screen on, on a monitor left of the primary one.
#[cfg(windows)]
#[test]
fn a_preview_takes_a_maximized_window_full_screen_the_same_way() {
    let mut h = harness();
    h.state_mut().session.execute("playback.settings.set", json!({"shortcut": "spacebar", "fullScreen": true})).unwrap();
    let maximized = rect(-1928.0, -8.0, 1936.0, 1056.0);
    let screen = rect(-1920.0, 0.0, 1920.0, 1080.0);
    os_window(&mut h, false, true, maximized);
    h.run_steps(2);
    invoke(&mut h, "playback.toggle");
    h.step();
    assert_eq!(moves(&h), [ViewportCommand::Fullscreen(true)]);
    os_window(&mut h, true, true, screen);
    h.step();
    assert_eq!(
        moves(&h),
        [ViewportCommand::Maximized(false), ViewportCommand::OuterPosition(pos2(-1920.0, 0.0)), ViewportCommand::InnerSize(vec2(1920.0, 1080.0))]
    );
    os_window(&mut h, true, false, screen);
    settled(&mut h);
    invoke(&mut h, "playback.toggle");
    h.step();
    assert_eq!(moves(&h), [ViewportCommand::Fullscreen(false)]);
    os_window(&mut h, false, true, maximized);
    settled(&mut h);
}

/// The monitor's place comes from the window covering it, so it waits for that.
#[cfg(windows)]
#[test]
fn a_maximized_window_waits_to_cover_the_monitor_before_it_is_un_maximized() {
    let mut h = harness();
    let maximized = rect(-8.0, -8.0, 1936.0, 1056.0);
    os_window(&mut h, false, true, maximized);
    h.run_steps(2);
    invoke(&mut h, "view.fullScreen");
    h.step();
    os_window(&mut h, true, true, maximized);
    settled(&mut h);
    os_window(&mut h, true, true, rect(0.0, 0.0, 1920.0, 1080.0));
    h.step();
    assert!(moves(&h).contains(&ViewportCommand::Maximized(false)), "{:?}", moves(&h));
}

/// Left before the un-maximize: a window still maximized would stay over the taskbar.
#[cfg(windows)]
#[test]
fn leaving_on_the_next_pass_still_un_maximizes_first() {
    let mut h = harness();
    os_window(&mut h, false, true, rect(-8.0, -8.0, 1936.0, 1056.0));
    h.run_steps(2);
    invoke(&mut h, "view.fullScreen");
    h.step();
    os_window(&mut h, true, true, rect(0.0, 0.0, 1920.0, 1080.0));
    invoke(&mut h, "view.fullScreen");
    h.step();
    assert_eq!(moves(&h), [ViewportCommand::Maximized(false), ViewportCommand::Fullscreen(false)]);
    settled(&mut h);
}

/// An element the pass just run drew (`auto.find` also reads the pass before).
#[cfg(windows)]
fn drawn(h: &Harness<'_, EffectcraftApp>, id: &str) -> Option<[f32; 4]> {
    h.state().auto.elements.iter().find(|e| e.id == id).map(|e| e.rect)
}

/// Windows from a window that isn't full screen yet: a pass is painted before its window commands
/// run, so the video waits for the pass that has the monitor's rect. Painted in the old client
/// area, under the title bar, it would show too low for a frame.
#[cfg(windows)]
#[test]
fn the_video_first_shows_on_the_pass_the_window_covers_the_monitor() {
    for (start, id) in [(rect(-8.0, -8.0, 1936.0, 1056.0), "view.fullScreen"), (rect(100.0, 80.0, 1200.0, 800.0), "playback.toggle")] {
        let mut h = harness();
        h.state_mut().session.execute("playback.settings.set", json!({"shortcut": "spacebar", "fullScreen": true})).unwrap();
        let maximized = start.min.x < 0.0;
        os_window(&mut h, false, maximized, start);
        h.run_steps(2);
        let workspace = drawn(&h, "viewer.comp");
        invoke(&mut h, id);
        h.step();
        assert_eq!(moves(&h)[0], ViewportCommand::Fullscreen(true));
        assert!(drawn(&h, "menu.Edit").is_some() && drawn(&h, "panel.tab.Timeline").is_some(), "{id}: the workspace stays until the window has changed");
        assert_eq!(drawn(&h, "viewer.comp"), workspace, "{id}: and the video stays where it was");

        os_window(&mut h, true, maximized, rect(0.0, 0.0, 1920.0, 1080.0));
        h.set_size(vec2(1920.0, 1080.0));
        h.step();
        assert!(drawn(&h, "menu.Edit").is_none() && drawn(&h, "panel.tab.Timeline").is_none(), "{id}: the interface is gone");
        assert_eq!(drawn(&h, "viewer.area"), Some([0.0, 0.0, 1920.0, 1080.0]), "{id}");
        let comp = drawn(&h, "viewer.comp").unwrap();
        let centre = pos2(comp[0] + comp[2] / 2.0, comp[1] + comp[3] / 2.0);
        assert!(centre.distance(pos2(960.0, 540.0)) < 1.0, "{id}: the video is in the middle of the screen, not {centre:?}");
    }
}

/// A window that isn't maximized only goes full screen and back, on every system.
#[test]
fn a_normal_window_only_goes_full_screen_and_back() {
    let mut h = harness();
    let normal = rect(100.0, 80.0, 1200.0, 800.0);
    os_window(&mut h, false, false, normal);
    h.run_steps(2);
    for _ in 0..2 {
        invoke(&mut h, "view.fullScreen");
        h.step();
        assert_eq!(moves(&h), [ViewportCommand::Fullscreen(true)]);
        os_window(&mut h, true, false, rect(0.0, 0.0, 1920.0, 1080.0));
        settled(&mut h);
        invoke(&mut h, "view.fullScreen");
        h.step();
        assert_eq!(moves(&h), [ViewportCommand::Fullscreen(false)]);
        os_window(&mut h, false, false, normal);
        settled(&mut h);
    }
}

#[test]
fn a_preview_with_full_screen_on_ends_it_with_the_preview() {
    let mut h = harness();
    h.state_mut().session.execute("playback.settings.set", json!({"shortcut": "spacebar", "fullScreen": true})).unwrap();
    invoke(&mut h, "playback.toggle");
    h.run_steps(2);
    assert!(h.state().full_screen.is_some_and(|f| f.from_preview));
    assert!(!has(&h, "menu.Edit"));
    invoke(&mut h, "playback.toggle");
    h.run_steps(2);
    assert!(h.state().full_screen.is_none());
    assert!(has(&h, "menu.Edit"));

    // Escape ends the preview too.
    invoke(&mut h, "playback.toggle");
    h.run_steps(2);
    escape(&mut h);
    assert!(h.state().full_screen.is_none());
    assert!(!h.state().playback.playing);

    // A preview inside View ▸ Enter Full Screen leaves it on when it stops.
    invoke(&mut h, "view.fullScreen");
    invoke(&mut h, "playback.toggle");
    h.run_steps(2);
    invoke(&mut h, "playback.toggle");
    h.run_steps(2);
    assert!(h.state().full_screen.is_some());
}

#[test]
fn full_screen_without_a_comp_only_changes_the_window() {
    let mut h = harness();
    h.state_mut().ui.start_screen = true;
    h.run_steps(2);
    invoke(&mut h, "view.fullScreen");
    h.step();
    assert!(h.state().full_screen.is_none());
    assert!(window_commands(&h).contains(&ViewportCommand::Fullscreen(true)), "{:?}", window_commands(&h));
    assert!(has(&h, "menu.Edit"));
}

/// A floating panel's OS window leaves with the rest of the interface and comes back where the
/// layout says it is, not where it first opened.
#[test]
fn floating_windows_close_in_full_screen_and_reopen_where_they_were() {
    let mut app = app();
    effectcraft_ui_egui::menus::invoke(&mut app, &egui::Context::default(), "window.panelWindow", json!({"panel": "Project", "window": true})).unwrap();
    let mut h = Harness::builder().with_size(vec2(1600.0, 1000.0)).build_eframe(|_| app);
    // Stand-in OS windows: what each pass shows, and where a window opening now would open.
    let shown: Rc<RefCell<HashMap<ViewportId, [f32; 2]>>> = Rc::default();
    let shared = shown.clone();
    egui::Context::set_immediate_viewport_renderer(move |ctx, mut viewport| {
        let id = viewport.ids.this;
        let position = viewport.builder.position.unwrap_or(pos2(0.0, 0.0));
        shared.borrow_mut().insert(id, [position.x, position.y]);
        let size = viewport.builder.inner_size.unwrap_or(vec2(400.0, 300.0));
        let info = ViewportInfo { native_pixels_per_point: Some(1.0), ..Default::default() };
        let input = egui::RawInput {
            viewport_id: id,
            viewports: [(ViewportId::ROOT, ViewportInfo::default()), (id, info)].into_iter().collect(),
            screen_rect: Some(egui::Rect::from_min_size(pos2(0.0, 0.0), size)),
            time: Some(ctx.input(|i| i.time)),
            ..Default::default()
        };
        let mut full = ctx.run_ui(input, |ui| (viewport.viewport_ui_cb)(ui));
        full.textures_delta.clear();
    });
    h.ctx.set_embed_viewports(false);
    let screen = h.ctx.content_rect();
    h.input_mut().viewports.entry(ViewportId::ROOT).or_default().outer_rect = Some(screen);
    h.run_steps(3);
    assert_eq!(shown.borrow().len(), 1, "the panel has its OS window");

    // The window moves after it opened.
    let moved = [300.0, 200.0, 500.0, 400.0];
    h.state_mut().apply_dock(Action::SetNative { panel: PanelKind::Project, rect: Some(moved) }).unwrap();
    h.run_steps(3);
    let layout = h.state().ui.dock.layout.clone();

    invoke(&mut h, "view.fullScreen");
    h.step();
    shown.borrow_mut().clear();
    h.run_steps(3);
    assert!(shown.borrow().is_empty(), "no OS window shows over the video: {:?}", shown.borrow());
    assert_eq!(h.state().ui.dock.layout, layout, "the workspace is untouched");

    invoke(&mut h, "view.fullScreen");
    h.run_steps(3);
    let reopened: Vec<[f32; 2]> = shown.borrow().values().copied().collect();
    assert_eq!(reopened, [[moved[0], moved[1]]], "it reopens where it was");
    assert_eq!(h.state().ui.dock.layout.floating.first().and_then(|w| w.native), Some(moved), "and the layout keeps its place");
}

/// A window that reaches the screen after the video first shows (off Windows, where no pass waits
/// for it): a set view follows it there instead of staying scaled for the old window.
#[test]
fn a_set_view_follows_the_window_to_the_screen() {
    let mut h = harness();
    h.state_mut().ui.viewer.zoom = Some(0.5);
    h.state_mut().ui.viewer.pan = [30.0, 12.0];
    h.run_steps(2);
    invoke(&mut h, "view.fullScreen");
    h.step();
    let zoom = h.state().ui.viewer.zoom.unwrap();
    let pan = h.state().ui.viewer.pan;
    assert!(zoom > 0.5, "scaled for the old window: {zoom}");
    h.set_size(vec2(1920.0, 1200.0));
    h.step();
    let followed = h.state().ui.viewer.zoom.unwrap();
    assert!((followed - zoom * 1.2).abs() < 1e-4, "{followed} from {zoom}");
    let moved = h.state().ui.viewer.pan;
    assert!((moved[0] - pan[0] * 1.2).abs() < 1e-3 && (moved[1] - pan[1] * 1.2).abs() < 1e-3, "{moved:?} from {pan:?}");
    h.step();
    assert_eq!(h.state().ui.viewer.zoom, Some(followed), "once");
}

/// The scale keeps to the viewer's zoom range, and a window with no area leaves the view alone
/// until there is one to scale to.
#[test]
fn the_full_screen_scale_stays_in_the_zoom_range_and_waits_for_a_window_with_area() {
    let mut h = harness();
    h.state_mut().ui.viewer.zoom = Some(30.0);
    h.state_mut().ui.viewer.pan = [300.0, 120.0];
    h.run_steps(2);
    invoke(&mut h, "view.fullScreen");
    h.run_steps(2);
    assert_eq!(h.state().ui.viewer.zoom, Some(32.0));
    let pan = h.state().ui.viewer.pan;
    assert!((pan[0] - 320.0).abs() < 1e-3 && (pan[1] - 128.0).abs() < 1e-3, "the pan follows the zoom it got: {pan:?}");
    invoke(&mut h, "view.fullScreen");
    h.run_steps(2);

    h.state_mut().ui.viewer.zoom = Some(0.5);
    h.state_mut().ui.viewer.pan = [30.0, 12.0];
    h.run_steps(2);
    let panel = h.state().auto.find("viewer.area").unwrap().rect;
    invoke(&mut h, "view.fullScreen");
    h.set_size(vec2(0.0, 0.0));
    h.step();
    assert_eq!(h.state().ui.viewer.zoom, Some(0.5));
    assert_eq!(h.state().ui.viewer.pan, [30.0, 12.0]);
    h.set_size(vec2(1600.0, 1000.0));
    h.step();
    let s = (1600.0 / panel[2]).min(1000.0 / panel[3]);
    let zoom = h.state().ui.viewer.zoom.unwrap();
    assert!((zoom - 0.5 * s).abs() < 1e-4, "{zoom} at {s}");
}
