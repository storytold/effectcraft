//! Long menus and lists scroll instead of running off the window, with a scroll bar to drag
//! (egui_kittest, UI logic only).

use effectcraft_engine::Session;
use effectcraft_ui_egui::EffectcraftApp;
use egui::{Event, Pos2, Rect, pos2, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use serde_json::json;

// The pointer/geometry helpers are shared with the sibling menu and long-tail test modules:
// `rect`, `click_at`, `move_to` and press-and-hold (`hold`) live in ui_menus, `drag` in ui_tail.
use super::ui_menus::{click_at, move_to, press as hold, rect};
use super::ui_tail::drag;

/// Open the demo project in a UI test harness with the requested window size.
fn harness(w: f32, h: f32) -> Harness<'static, EffectcraftApp> {
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    let mut h = Harness::builder().with_size(vec2(w, h)).build_eframe(|_| EffectcraftApp::new(s));
    h.run_steps(3);
    h
}

fn wheel(h: &mut Harness<'_, EffectcraftApp>, at: Pos2, dy: f32) {
    h.input_mut().events.push(Event::PointerMoved(at));
    h.input_mut().events.push(Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        delta: vec2(0.0, dy),
        modifiers: Default::default(),
        phase: egui::TouchPhase::Move,
        source: egui::MouseWheelSource::Wheel,
    });
    h.run_steps(20);
}

/// The open menu popup's rect (frame included), from egui's area memory: the only tall area up.
fn open_popup(h: &Harness<'_, EffectcraftApp>) -> Rect {
    h.ctx
        .memory(|m| {
            m.areas()
                .visible_layer_ids()
                .into_iter()
                .filter(|l| l.order != egui::Order::Background)
                .filter_map(|l| m.area_rect(l.id))
                .find(|r| r.height() > 100.0)
        })
        .unwrap_or_else(|| panic!("no menu popup is open"))
}

/// Edit is taller than a short window: it scrolls (with the wheel, and a scroll bar that always
/// shows) so its last entries can be reached and clicked, instead of running off the bottom of
/// the window (#269).
#[test]
fn a_menu_taller_than_the_window_scrolls() {
    let mut h = harness(1200.0, 520.0);
    assert!(!h.ctx.global_style().spacing.scroll.floating, "scroll bars show without hovering the list");
    let edit = rect(&h, "menu.Edit");
    click_at(&mut h, edit.center());
    let last = h.query_by_label_contains("Keyboard Shortcuts").expect("the Edit menu is open").rect();
    assert!(last.max.y > 520.0, "Edit is taller than the window ({last:?})");
    let first = h.query_by_label_contains("Quick Apply").expect("Quick Apply").rect();
    wheel(&mut h, first.center(), -2000.0);
    let last = h.query_by_label_contains("Keyboard Shortcuts").expect("still open").rect();
    assert!(last.min.y >= 0.0 && last.max.y <= 520.0, "scrolled into the window ({last:?})");
    click_at(&mut h, last.center());
    assert_eq!(h.state().dialog, Some(effectcraft_ui_egui::Dialog::Shortcuts), "the entry takes the click");
}

/// Open Edit in a window shorter than the menu (holding or clicking its title) and return the
/// last entry's rect before any scroll, plus the open popup's rect. `opened` is the failure
/// message when the menu did not open.
fn open_tall_edit(h: &mut Harness<'_, EffectcraftApp>, held: bool, opened: &str) -> (Rect, Rect) {
    let edit = rect(h, "menu.Edit");
    if held {
        hold(h, edit.center());
    } else {
        click_at(h, edit.center());
    }
    let before = h.query_by_label_contains("Keyboard Shortcuts").expect(opened).rect();
    assert!(before.max.y > 520.0, "Edit is taller than the window ({before:?})");
    let menu = open_popup(h);
    (before, menu)
}

/// With the title still held down (press-and-browse), resting the pointer against the bottom
/// edge of a menu too tall for the window scrolls it from there — a crawl, then a run, as native
/// menus do — so the long list can be walked with no wheel.
#[test]
fn holding_the_title_scrolls_the_menu_from_its_bottom_edge() {
    let mut h = harness(1200.0, 520.0);
    let (before, menu) = open_tall_edit(&mut h, true, "the Edit menu opens on the press");
    // Creeping down the edge: the 18 pt band above the menu's inner bottom lies at one of these
    // positions, whatever the frame's margin (they are 6 pt apart).
    let mut walked = false;
    for step in 0..5 {
        move_to(&mut h, pos2(menu.center().x, menu.max.y - 6.0 - 6.0 * step as f32));
        h.run_steps(8);
        let now = h.query_by_label_contains("Keyboard Shortcuts").expect("still open").rect();
        if now.min.y < before.min.y {
            walked = true;
            break;
        }
    }
    assert!(walked, "resting on the bottom edge of the held-down menu scrolled its entries up");
}

/// Without the title held down — the menu opened by a plain click — the same edge rest scrolls
/// nothing: the wheel (above) is how a click-opened menu scrolls, the pointer alone must not
/// move it (a hover may align the row under it by a few points; a crawl there would be a bug).
#[test]
fn without_the_hold_the_edge_does_not_scroll_the_menu() {
    let mut h = harness(1200.0, 520.0);
    let (before, menu) = open_tall_edit(&mut h, false, "the Edit menu is open");
    for step in 0..5 {
        move_to(&mut h, pos2(menu.center().x, menu.max.y - 6.0 - 6.0 * step as f32));
        h.run_steps(8);
        let now = h.query_by_label_contains("Keyboard Shortcuts").expect("still open").rect();
        let crawled = before.min.y - now.min.y;
        assert!(crawled < 60.0, "step {step}: the edge scrolled the menu with no title held down ({before:?} -> {now:?})");
    }
}

/// Find the actual Edit popup's bottom crawl band while keeping the title press held.
fn held_edit_crawl(h: &mut Harness<'_, EffectcraftApp>) -> Pos2 {
    let edit = rect(h, "menu.Edit");
    hold(h, edit.center());
    let before = h.query_by_label_contains("Keyboard Shortcuts").expect("Edit opens on press").rect();
    let menu = open_popup(h);
    for step in 0..5 {
        let p = pos2(menu.center().x, menu.max.y - 6.0 - 6.0 * step as f32);
        move_to(h, p);
        h.run_steps(8);
        let after = h.query_by_label_contains("Keyboard Shortcuts").expect("Edit stays open").rect();
        if after.min.y < before.min.y - 0.1 {
            return p;
        }
    }
    panic!("the held Edit menu never crawled from its bottom edge");
}

fn menu_at(h: &mut Harness<'_, EffectcraftApp>, time: f64) {
    h.input_mut().time = Some(time);
    h.step();
}

/// A discarded layout pass may consume the queued offset once, but must not crawl again for
/// the same RawInput time. A subsequent real iteration must still queue and consume its step.
#[test]
fn menu_edge_crawl_advances_once_on_replay_then_continues_at_a_later_time() {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    let mut h = harness(1200.0, 520.0);
    held_edit_crawl(&mut h);
    let discard = Arc::new(AtomicBool::new(true));
    h.ctx.on_end_pass(
        "menu crawl replay regression",
        Arc::new(move |ui| {
            if discard.swap(false, Ordering::SeqCst) {
                ui.ctx().request_discard("replay an actual held menu");
            }
        }),
    );
    let time = h.ctx.input(|i| i.time) + 1.0 / 60.0;
    menu_at(&mut h, time);
    assert_eq!(h.output().platform_output.num_completed_passes, 2, "the actual menu was replayed");
    let once = h.query_by_label_contains("Keyboard Shortcuts").unwrap().rect();
    for _ in 0..3 {
        menu_at(&mut h, time);
        assert_eq!(h.query_by_label_contains("Keyboard Shortcuts").unwrap().rect(), once, "same-time replay cannot queue another crawl");
    }
    let later = time + 1.0 / 60.0;
    menu_at(&mut h, later);
    menu_at(&mut h, later);
    let advanced = h.query_by_label_contains("Keyboard Shortcuts").unwrap().rect();
    assert!(advanced.min.y < once.min.y - 0.1, "a later input time advances the held menu ({once:?} -> {advanced:?})");
    menu_at(&mut h, later);
    assert_eq!(h.query_by_label_contains("Keyboard Shortcuts").unwrap().rect(), advanced, "the later time also takes only one step");
}

/// The actual menu viewport's vertical range, rather than its outer Area/frame rectangle.
/// Popup's accessibility parent is its title, so background panels and other menus cannot match.
fn menu_scrollbar(h: &Harness<'_, EffectcraftApp>, title: &str) -> Rect {
    let menu = h.get_by_label(title);
    let bars: Vec<_> = menu.query_all_by_role(egui::accesskit::Role::ScrollBar).map(|node| node.rect()).filter(|r| r.height() > r.width()).collect();
    assert_eq!(bars.len(), 1, "{title}: exactly one actual vertical menu scrollbar ({bars:?})");
    let bar = bars[0];
    assert!(bar.height() > 36.0, "{title}: both crawl bands fit inside the viewport ({bar:?})");
    bar
}

/// Exercise the real popup, its bounds, band exit and subsequent wheel input. Switching to a
/// different real menu must not consume Edit's pending target or reuse its crawl clock.
#[test]
fn menu_edge_crawl_is_bounded_stops_and_keeps_wheel_and_menu_state_independent() {
    let mut h = harness(1200.0, 520.0);
    held_edit_crawl(&mut h);
    h.run_steps(180);
    let bottom = h.query_by_label_contains("Keyboard Shortcuts").unwrap().rect();
    assert!(bottom.max.y <= 520.0, "the last item is inside the viewport at the bottom ({bottom:?})");
    h.run_steps(20);
    assert_eq!(h.query_by_label_contains("Keyboard Shortcuts").unwrap().rect(), bottom, "the bottom bound stops the crawl");

    let bar = menu_scrollbar(&h, "Edit");
    let x = bar.min.x - 12.0; // Inside the menu content, not on the scroll-bar track.
    let top_band = pos2(x, bar.min.y + 6.0);
    eprintln!("Edit actual viewport scrollbar {bar:?}; top-band input {top_band:?}");
    move_to(&mut h, top_band);
    h.run_steps(180);
    let top = h.query_by_label_contains("Quick Apply").unwrap().rect();
    assert!(top.min.y >= 0.0 && top.max.y <= 520.0, "the first entries return into view ({top:?}); actual bar {bar:?}, pointer {top_band:?}");
    h.run_steps(20);
    assert_eq!(h.query_by_label_contains("Quick Apply").unwrap().rect(), top, "the top bound stops the crawl");

    let last_at_top = h.query_by_label_contains("Keyboard Shortcuts").unwrap().rect();
    move_to(&mut h, pos2(x, bar.max.y - 6.0));
    h.run_steps(15);
    let scrolled = h.query_by_label_contains("Keyboard Shortcuts").unwrap().rect();
    assert!(scrolled.min.y < last_at_top.min.y - 0.1, "the bottom band starts scrolling again ({last_at_top:?} -> {scrolled:?})");
    move_to(&mut h, pos2(x, bar.center().y));
    h.run_steps(2); // Consume the last one-shot target before checking the stopped menu.
    let stopped = h.query_by_label_contains("Keyboard Shortcuts").unwrap().rect();
    h.run_steps(20);
    assert_eq!(h.query_by_label_contains("Keyboard Shortcuts").unwrap().rect(), stopped, "leaving the band stops the crawl");
    assert!(
        last_at_top.min.y > stopped.min.y + 1.0,
        "there is upward wheel room: bottom {bottom:?}, top {last_at_top:?}, resumed {scrolled:?}, stopped {stopped:?}"
    );
    wheel(&mut h, pos2(x, bar.center().y), 120.0);
    let wheeled_up = h.query_by_label_contains("Keyboard Shortcuts").unwrap().rect();
    assert!(
        wheeled_up.min.y > stopped.min.y + 1.0,
        "upward wheel input moves the menu after the one-shot target: bottom {bottom:?}, top {last_at_top:?}, resumed {scrolled:?}, stopped {stopped:?}, wheeled {wheeled_up:?}"
    );
    assert!(wheeled_up.min.y > bottom.min.y + 1.0, "there is downward wheel room ({bottom:?} -> {wheeled_up:?})");
    wheel(&mut h, pos2(x, bar.center().y), -120.0);
    let wheeled = h.query_by_label_contains("Keyboard Shortcuts").unwrap().rect();
    assert!(
        wheeled.min.y < wheeled_up.min.y - 1.0,
        "wheel input still moves the menu after the one-shot target ({wheeled_up:?} -> {wheeled:?}); bottom {bottom:?}, top {last_at_top:?}, resumed {scrolled:?}, stopped {stopped:?}"
    );

    // Queue another Edit crawl, then switch titles before its next draw can consume the target.
    move_to(&mut h, pos2(x, bar.max.y - 6.0));
    h.run_steps(15);
    let file = rect(&h, "menu.File");
    move_to(&mut h, file.center());
    let fresh = h.query_by_label_contains("Open Project").expect("File opens while browsing").rect();
    assert!(fresh.min.y >= file.max.y && fresh.max.y <= 520.0, "File starts at its own top, not Edit's offset ({fresh:?})");
    h.run_steps(12);
    assert_eq!(h.query_by_label_contains("Open Project").unwrap().rect(), fresh, "Edit's crawl does not continue in File");

    // Release on its title rather than an entry, retaining a click-opened popup when possible.
    h.input_mut().events.push(Event::PointerButton { pos: file.center(), button: egui::PointerButton::Primary, pressed: false, modifiers: Default::default() });
    h.run_steps(2);
    if h.query_by_label_contains("Open Project").is_none() {
        click_at(&mut h, file.center());
    }
    let bar = menu_scrollbar(&h, "File");
    let x = bar.min.x - 12.0;
    let released = h.query_by_label_contains("Open Project").unwrap().rect();
    move_to(&mut h, pos2(x, bar.max.y - 6.0));
    h.run_steps(20);
    assert_eq!(h.query_by_label_contains("Open Project").unwrap().rect(), released, "without the hold, the edge does not restart a crawl");
}

/// A scroll bar's "scroll/max" automation label.
fn scrolled(h: &Harness<'_, EffectcraftApp>, id: &str) -> (f32, f32) {
    let label = h.state().auto.find(id).unwrap_or_else(|| panic!("no {id}")).label.clone();
    let (s, max) = label.split_once('/').unwrap_or_else(|| panic!("{id}: {label}"));
    (s.parse().unwrap(), max.parse().unwrap())
}

/// Every panel that scrolls its own rows shows a scroll bar while they overflow, and dragging it
/// scrolls them: without a mouse wheel or touchpad there was no way down most panels (#271).
#[test]
fn panels_taller_than_their_space_have_a_scroll_bar_to_drag() {
    use effectcraft_ui_egui::dock::PanelKind;
    // (The Project panel's list starts below its item preview and search: a taller window.)
    for (panel, id, height) in [
        (PanelKind::EffectControls, "effectControls.scroll", 240.0),
        (PanelKind::Properties, "properties.scroll", 240.0),
        (PanelKind::EffectsPresets, "effects.scroll", 240.0),
        (PanelKind::Project, "project.vscroll", 330.0),
        (PanelKind::Timeline, "timeline.scroll", 240.0),
        (PanelKind::RenderQueue, "renderQueue.scroll", 240.0),
        (PanelKind::History, "history.scroll", 240.0),
        (PanelKind::Preview, "preview.scroll", 240.0),
        (PanelKind::Character, "character.scroll", 240.0),
        (PanelKind::Paragraph, "paragraph.scroll", 240.0),
        (PanelKind::Info, "info.scroll", 240.0),
    ] {
        let mut h = harness(1200.0, height);
        let app = h.state_mut();
        let lid = app.session.active_comp().unwrap().layers[0].id.0;
        app.session.execute("layer.select", json!({"layers": [lid]})).unwrap();
        for e in ["slider", "angle", "checkbox", "color", "point"] {
            app.session.execute("effect.apply", json!({"effect": format!("ec.control.{e}"), "layers": [lid]})).unwrap();
        }
        for _ in 0..6 {
            app.session.execute("renderQueue.add", json!({})).unwrap();
        }
        app.show_panel(panel);
        app.ui.maximized = Some(panel);
        h.run_steps(3);
        let (_, max) = scrolled(&h, id);
        assert!(max > 0.0, "{id}: the rows overflow");
        let bar = rect(&h, id);
        // A press on the track moves the thumb there (History starts at its current state, the
        // last); then the thumb drags to the end.
        click_at(&mut h, pos2(bar.center().x, bar.min.y + 2.0));
        assert_eq!(scrolled(&h, id).0, 0.0, "{id}: at the top");
        drag(&mut h, pos2(bar.center().x, bar.min.y + 6.0), pos2(bar.center().x, bar.max.y + 40.0));
        let end = scrolled(&h, id).0;
        assert!((end - max).abs() < 0.01, "{id}: dragged to the end: {end} of {max}");
    }
}

/// In a narrow Effect Controls panel an animated point's values stay clear of its keyframe
/// navigator (â—€ â—† â–¶), which was drawn over them (#271).
#[test]
fn effect_values_stay_clear_of_the_keyframe_navigator() {
    use effectcraft_ui_egui::dock::PanelKind;
    let mut h = harness(1600.0, 1000.0);
    let app = h.state_mut();
    let lid = app.session.active_comp().unwrap().layers[0].id.0;
    app.session.execute("layer.select", json!({"layers": [lid]})).unwrap();
    app.session.execute("effect.apply", json!({"effect": "ec.control.point", "layers": [lid]})).unwrap();
    let path = "effects/#1/point";
    app.session.execute("prop.toggleAnimation", json!({"layer": lid, "path": path})).unwrap();
    app.session.execute("prop.set", json!({"layer": lid, "path": path, "value": [123456.7, 123456.7]})).unwrap();
    let uid = app.session.active_comp().unwrap().layer(effectcraft_engine::project::LayerId(lid)).unwrap().props.prop(path).unwrap().uid;
    app.apply_dock(craft_ui::docking::Action::Float { panel: PanelKind::EffectControls, rect: [300.0, 150.0, 330.0, 500.0] }).unwrap();
    h.run_steps(3);
    let value = rect(&h, &format!("effectControls.prop.{uid}.value.1"));
    let nav = rect(&h, &format!("effectControls.prop.{uid}.prevKey"));
    assert!(value.max.x <= nav.min.x, "the value ({value:?}) ends before the navigator ({nav:?})");
    assert!(value.min.x > rect(&h, &format!("effectControls.prop.{uid}.stopwatch")).max.x, "and after the stopwatch");
}

/// The Settings dialog fits its width: its page column was wider than the dialog, so the whole
/// dialog scrolled sideways a little (#284).
#[test]
fn settings_dialog_does_not_scroll_sideways() {
    let mut h = harness(1400.0, 900.0);
    let ctx = h.ctx.clone();
    effectcraft_ui_egui::menus::invoke(h.state_mut(), &ctx, "app.settings", json!({"page": "general"})).unwrap();
    h.run_steps(3);
    let (ok, page) = (rect(&h, "settings.ok"), rect(&h, "settings.page.general"));
    h.input_mut().events.push(Event::PointerMoved(page.center()));
    h.input_mut().events.push(Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        delta: vec2(-400.0, 0.0),
        modifiers: Default::default(),
        phase: egui::TouchPhase::Move,
        source: egui::MouseWheelSource::Wheel,
    });
    h.run_steps(20);
    assert_eq!((rect(&h, "settings.ok"), rect(&h, "settings.page.general")), (ok, page), "nothing moved sideways");
}

/// #2034: the Settings sidebar uses the translated page names when deciding its width.
#[test]
fn settings_page_column_fits_japanese_names() {
    use effectcraft_text::fonts;
    use effectcraft_ui_egui::theme::Tokens;

    let mut session = Session::default();
    session.prefs.general.language = "ja".into();
    let app = EffectcraftApp::new(session);
    // The panel titles a page with its Settings menu row, minus the ellipsis.
    let name = effectcraft_ui_egui::menus::menu_items(&app)
        .into_iter()
        .find(|m| m.id == "app.settings" && m.params["page"] == "scripting")
        .and_then(|m| m.label.strip_suffix("...").map(str::to_owned))
        .expect("the Settings menu lists the Scripting & Expressions page");
    assert!(name.chars().any(|c| c >= '\u{3000}'), "the actual sidebar label is Japanese: {name}");
    // Use the same external-directory scan and face coverage as production, before rendering.
    // The managed job supplies CRAFT_FONTS_DIR; register its canonical fonts folder through the
    // same directory source production scans for EFFECTCRAFT_FONT_DIRS, without changing the env.
    crate::add_canonical_test_fonts();
    fonts::scan_system();
    let base = fonts::resolve("Inter", "Regular").face;
    let japanese = fonts::face(fonts::fallback_for_locale('あ', base, "ja"));
    for ch in ['あ', 'フ'].into_iter().chain(name.chars()) {
        assert!(japanese.has_char(ch), "Japanese fixture lacks {ch:?} for {name:?}: {:?}", japanese.info);
    }
    let required_canonical = std::env::var("CRAFT_FONTS_REQUIRED").as_deref() == Ok("1");
    if required_canonical {
        let root = std::env::var_os("CRAFT_FONTS_DIR").expect("required canonical craft-fonts input");
        let expected =
            std::path::PathBuf::from(root).join("fonts/biz-ud-pgothic/BIZUDPGothic-Regular.ttf").canonicalize().expect("canonical BIZ UDPGothic Regular input");
        let resolved = fonts::resolve("BIZ UDPGothic", "Regular");
        assert!(!resolved.missing, "required BIZ UDPGothic Regular must resolve without substitution");
        let face = fonts::face(resolved.face);
        assert_eq!(face.info.family, "BIZ UDPGothic", "{:?}", face.info);
        assert_eq!(face.info.style, "Regular", "{:?}", face.info);
        assert_eq!(japanese.id, face.id, "required production fallback is the canonical BIZ face: selected {:?}, canonical {:?}", japanese.info, face.info);
        let fonts::FaceData::File(path) = &face.info.data else { panic!("required BIZ face must come from canonical external input: {:?}", face.info) };
        assert_eq!(path.canonicalize().unwrap(), expected, "required face source: {:?}", face.info);
        let expected_bytes = std::fs::read(&expected).unwrap();
        let font = face.font().expect("required canonical BIZ face parses");
        assert!(font.data().as_bytes() == expected_bytes.as_slice(), "resolved BIZ bytes match canonical input {expected:?}");
        for ch in ['あ', 'フ'].into_iter().chain(name.chars()) {
            assert!(face.has_char(ch), "canonical BIZ input lacks actual label glyph {ch:?}: {:?}", face.info);
        }
        eprintln!("required Japanese input {expected:?}: {:?}", face.info);
    }

    let mut h = Harness::builder().with_size(vec2(1400.0, 900.0)).build_eframe(|_| app);
    h.run_steps(3);
    let ctx = h.ctx.clone();
    effectcraft_ui_egui::menus::invoke(h.state_mut(), &ctx, "app.settings", json!({"page": "scripting"})).unwrap();
    h.run_steps(3);
    h.ctx.fonts_mut(|fonts| {
        let definitions = fonts.definitions();
        assert!(
            definitions.font_data.contains_key("japanese-system"),
            "production installs the Japanese fallback; installed {:?}",
            definitions.font_data.keys().collect::<Vec<_>>()
        );
        assert!(
            definitions.families.get(&egui::FontFamily::Proportional).unwrap().iter().any(|name| name == "japanese-system"),
            "production proportional stack reaches its Japanese fallback"
        );
        if required_canonical {
            let registered = definitions.font_data.get("japanese-system").unwrap();
            let selected = japanese.font().expect("selected canonical Japanese face parses");
            assert_eq!(registered.index, japanese.info.index, "production registration retains selected face index");
            assert!(
                registered.font.as_ref().as_ref() == selected.data().as_bytes(),
                "production japanese-system bytes equal the selected canonical fallback: {:?}",
                japanese.info
            );
        }
        assert!(fonts.has_glyphs(&Tokens::ui(12.5), &name), "production glyph lookup covers actual Japanese sidebar label {name:?}");
    });
    let text_width = h.ctx.fonts_mut(|fonts| fonts.layout_no_wrap(name.clone(), Tokens::ui(12.5), egui::Color32::WHITE).size().x);
    let row = rect(&h, "settings.page.scripting");
    assert!(row.width() >= text_width + 20.0, "{name} needs {text_width} points; row is {row:?}");
}
