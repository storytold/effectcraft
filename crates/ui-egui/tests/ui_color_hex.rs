//! The colour picker every colour input opens: its hex field sets an effect's colour (CC Toner),
//! the dialogs' colour button stores sRGB (egui's own `color_edit_button_rgba` read the stored
//! sRGB values as linear, so mid-grey `#808080` was stored as `#373737`), the square, slider and
//! HSB / RGB fields set the colour, and Cancel / Escape / the original swatch restore it.

use effectcraft_engine::Session;
use effectcraft_engine::project::LayerId;
use effectcraft_ui_egui::EffectcraftApp;
use effectcraft_ui_egui::dock::PanelKind;
use effectcraft_ui_egui::widgets;
use egui::accesskit::Role;
use egui::{Event, Key, Modifiers, Pos2, pos2, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use serde_json::json;

const GREY: f32 = 128.0 / 255.0;

fn click<S>(h: &mut Harness<'_, S>, p: Pos2) {
    h.input_mut().events.push(Event::PointerMoved(p));
    h.input_mut().events.push(Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
    h.step();
    h.input_mut().events.push(Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
    h.run_steps(3);
}

/// Click the open popup's Hex field and type `text` over the code it shows.
fn type_hex<S>(h: &mut Harness<'_, S>, text: &str) {
    let at = h.get_by_role_and_label(Role::TextInput, "#").rect().center();
    click(h, at);
    for pressed in [true, false] {
        h.input_mut().events.push(Event::Key { key: Key::A, physical_key: None, pressed, repeat: false, modifiers: Modifiers::COMMAND });
    }
    h.step();
    h.input_mut().events.push(Event::Text(text.into()));
    h.run_steps(3);
}

fn close(c: [f32; 3], want: [f32; 3]) -> bool {
    c.iter().zip(want).all(|(a, b)| (a - b).abs() < 1e-4)
}

#[test]
fn hex_code_sets_an_effect_colour() {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Hex", "width": 320, "height": 180, "duration": 4})).unwrap();
    let layer = s.execute("layer.newSolid", json!({"name": "Plate", "color": "#40b080"})).unwrap()["layer"].as_u64().unwrap();
    let fx = s.execute("effect.apply", json!({"layer": layer, "effect": "CC Toner"})).unwrap()["effects"][0].as_u64().unwrap();
    let uid = s.active_comp().unwrap().layer(LayerId(layer)).unwrap().props.find_group(fx).unwrap().get("midtones").unwrap().uid;
    s.state.selected_layers = vec![LayerId(layer)];
    let mut app = EffectcraftApp::new(s);
    app.show_panel(PanelKind::EffectControls);
    app.toggle_maximize(PanelKind::EffectControls);
    let mut h = Harness::builder().with_size(vec2(1600.0, 1000.0)).build_eframe(|_| app);
    h.run_steps(4);
    let midtones = |h: &Harness<'_, EffectcraftApp>| {
        let c = h.state().session.active_comp().unwrap().layer(LayerId(layer)).unwrap().props.find(uid).unwrap().value.as_color();
        [c[0], c[1], c[2]]
    };

    let [x, y, w, ht] = h.state().auto.find(&format!("effectControls.prop.{uid}.value")).expect("the Midtones swatch").rect;
    click(&mut h, pos2(x + w / 2.0, y + ht / 2.0));
    type_hex(&mut h, "#FF8000");
    assert!(close(midtones(&h), [1.0, GREY, 0.0]), "{:?}", midtones(&h));
    // A partial code doesn't apply; the short form does.
    type_hex(&mut h, "#12");
    assert!(close(midtones(&h), [1.0, GREY, 0.0]), "{:?}", midtones(&h));
    type_hex(&mut h, "0f8");
    assert!(close(midtones(&h), [0.0, 1.0, 136.0 / 255.0]), "{:?}", midtones(&h));
}

/// A colour button in a harness of its own, its picker open.
fn picker(c: [f32; 3]) -> Harness<'static, [f32; 3]> {
    let mut h = Harness::builder().with_size(vec2(560.0, 400.0)).build_ui_state(
        |ui, c: &mut [f32; 3]| {
            widgets::srgb_color_button(ui, c);
        },
        c,
    );
    h.run_steps(2);
    let button = h.get_by_role(Role::ColorWell).rect().center();
    click(&mut h, button);
    h
}

fn press(h: &mut Harness<'_, [f32; 3]>, key: Key, modifiers: Modifiers) {
    for pressed in [true, false] {
        h.input_mut().events.push(Event::Key { key, physical_key: None, pressed, repeat: false, modifiers });
    }
    h.run_steps(2);
}

fn click_label(h: &mut Harness<'_, [f32; 3]>, label: &str) {
    let at = h.get_by_label(label).rect().center();
    click(h, at);
}

/// Type `text` into the first number field labelled `label` ("R:", "H:" …; "B:" is Brightness
/// before Blue, as in After Effects) and press Enter.
fn type_field(h: &mut Harness<'_, [f32; 3]>, label: &str, text: &str) {
    let at = h.get_all_by_role_and_label(Role::SpinButton, label).next().expect("the field").rect().center();
    click(h, at);
    press(h, Key::A, Modifiers::COMMAND);
    h.input_mut().events.push(Event::Text(text.into()));
    h.step();
    press(h, Key::Enter, Modifiers::NONE);
}

fn near(c: [f32; 3], want: [f32; 3]) -> bool {
    c.iter().zip(want).all(|(a, b)| (a - b).abs() < 0.02)
}

fn is_open(h: &Harness<'_, [f32; 3]>) -> bool {
    h.query_by_role_and_label(Role::TextInput, "#").is_some()
}

/// #1476: the click that opens the picker must not close it. The picker's area only reports its
/// real position from its second pass — on the pass it first draws, its rect is at the origin —
/// so a click on a swatch outside that stale rect was read as a press outside the popup and the
/// picker flashed and closed in the frame it opened.
#[test]
fn a_click_that_opens_the_picker_does_not_close_it_in_the_same_frame() {
    let mut h = Harness::builder().with_size(vec2(560.0, 400.0)).build_ui_state(
        |ui, c: &mut [f32; 3]| {
            // Away from the top-left corner: the stale first-pass rect covers it.
            ui.allocate_space(vec2(440.0, 120.0));
            ui.horizontal(|ui| {
                ui.add_space(440.0);
                widgets::srgb_color_button(ui, c);
            });
        },
        [1.0, 0.0, 0.0],
    );
    h.run_steps(2);
    let button = h.get_by_role(Role::ColorWell).rect().center();
    h.input_mut().events.push(Event::PointerMoved(button));
    h.input_mut().events.push(Event::PointerButton { pos: button, button: egui::PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
    h.input_mut().events.push(Event::PointerButton { pos: button, button: egui::PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
    h.run_steps(2);
    assert!(is_open(&h), "the click that opens the picker must not close it");
}

#[test]
fn the_dialog_colour_button_stores_srgb() {
    let mut h = picker([1.0, 1.0, 1.0]);
    type_hex(&mut h, "808080");
    assert!(close(*h.state(), [GREY; 3]), "{:?}", h.state());
    click_label(&mut h, "OK");
    assert!(!is_open(&h));
    assert!(close(*h.state(), [GREY; 3]));
}

/// Changes apply as they are made; Cancel, Escape and the original swatch go back to the colour
/// the picker opened with, OK keeps the new one.
#[test]
fn cancel_escape_and_the_original_swatch_restore_the_colour() {
    let red = [1.0, 0.0, 0.0];
    let mut h = picker(red);
    type_hex(&mut h, "00FF00");
    assert!(close(*h.state(), [0.0, 1.0, 0.0]));
    click_label(&mut h, "Cancel");
    assert!(!is_open(&h));
    assert!(close(*h.state(), red), "{:?}", h.state());

    let button = h.get_by_role(Role::ColorWell).rect().center();
    click(&mut h, button);
    type_hex(&mut h, "0000FF");
    press(&mut h, Key::Escape, Modifiers::NONE);
    assert!(!is_open(&h));
    assert!(close(*h.state(), red), "{:?}", h.state());

    click(&mut h, button);
    type_hex(&mut h, "0000FF");
    click_label(&mut h, "Original color");
    assert!(is_open(&h), "going back to the original keeps the picker open");
    assert!(close(*h.state(), red), "{:?}", h.state());
    type_hex(&mut h, "FFFF00");
    click_label(&mut h, "OK");
    assert!(close(*h.state(), [1.0, 1.0, 0.0]), "{:?}", h.state());
}

/// With H on the slider the square is saturation × brightness; the hue is kept while the colour is
/// black, where RGB has none. HSB and RGB values can be typed.
#[test]
fn the_hsb_square_slider_and_fields_set_the_colour() {
    let mut h = picker([1.0, 0.0, 0.0]);
    let square = h.get_by_label("Color field").rect();
    let slider = h.get_by_label("Color slider").rect();

    click(&mut h, pos2(square.center().x, square.max.y - 1.0));
    assert!(near(*h.state(), [0.0; 3]), "the bottom edge is black: {:?}", h.state());
    click(&mut h, slider.center());
    assert!(near(*h.state(), [0.0; 3]), "a new hue leaves black black: {:?}", h.state());
    click(&mut h, pos2(square.max.x - 1.0, square.min.y + 1.0));
    assert!(near(*h.state(), [0.0, 1.0, 1.0]), "the hue chosen while black is kept: {:?}", h.state());

    type_field(&mut h, "R:", "64");
    assert!((h.state()[0] - 64.0 / 255.0).abs() < 1e-4 && near(*h.state(), [64.0 / 255.0, 1.0, 1.0]), "{:?}", h.state());
    type_field(&mut h, "H:", "0");
    type_field(&mut h, "S:", "100");
    type_field(&mut h, "B:", "50");
    assert!(near(*h.state(), [0.5, 0.0, 0.0]), "{:?}", h.state());
}

/// With R chosen the slider sets red and the square blue (across) and green (up), as in After
/// Effects; the choice is remembered by the next picker.
#[test]
fn choosing_r_puts_red_on_the_slider_and_blue_green_on_the_square() {
    let mut h = picker([0.0, 0.0, 0.0]);
    let radio = h.get_by_role_and_label(Role::RadioButton, "R:").rect().center();
    click(&mut h, radio);
    let square = h.get_by_label("Color field").rect();
    let slider = h.get_by_label("Color slider").rect();
    click(&mut h, pos2(slider.center().x, slider.min.y + 1.0));
    assert!(near(*h.state(), [1.0, 0.0, 0.0]), "the top of the slider is full red: {:?}", h.state());
    click(&mut h, pos2(square.max.x - 1.0, square.max.y - 1.0));
    assert!(near(*h.state(), [1.0, 0.0, 1.0]), "bottom right adds full blue: {:?}", h.state());
    click(&mut h, pos2(square.min.x + 1.0, square.min.y + 1.0));
    assert!(near(*h.state(), [1.0, 1.0, 0.0]), "top left is full green, no blue: {:?}", h.state());
    click_label(&mut h, "OK");

    let button = h.get_by_role(Role::ColorWell).rect().center();
    click(&mut h, button);
    let slider = h.get_by_label("Color slider").rect();
    click(&mut h, pos2(slider.center().x, slider.max.y - 1.0));
    assert!(near(*h.state(), [0.0, 1.0, 0.0]), "the slider still sets red: {:?}", h.state());
}

#[test]
fn hex_codes_parse() {
    assert_eq!(widgets::hex_rgb("#ff8000"), Some([255, 128, 0]));
    assert_eq!(widgets::hex_rgb(" FF8000 "), Some([255, 128, 0]));
    assert_eq!(widgets::hex_rgb("#0f8"), Some([0, 255, 136]));
    for bad in ["", "#", "#12", "#12345", "#1234567", "#gg0000", "#ffé000"] {
        assert_eq!(widgets::hex_rgb(bad), None, "{bad}");
    }
    assert_eq!(widgets::srgb_hex([1.0, GREY, 0.0]), "#FF8000");
}

#[test]
fn hsv_round_trips() {
    for c in [[1.0, 0.0, 0.0], [0.0, 1.0, 1.0], [GREY, 0.25, 0.75], [0.2, 0.2, 0.2], [0.0, 0.0, 0.0], [1.0, 1.0, 1.0]] {
        let back = widgets::hsv_to_rgb(widgets::rgb_to_hsv(c));
        assert!(close(back, c), "{c:?} -> {back:?}");
    }
    assert!(close(widgets::hsv_to_rgb([0.5, 1.0, 1.0]), [0.0, 1.0, 1.0]));
}

/// The fields round as the hex code does (0.7 × 255 = 178.5 shows as 179, `B3`), and a hue just
/// under 360° shows as 0°.
#[test]
fn the_fields_agree_with_the_hex_code() {
    let h = picker([1.0, 0.699, 0.7]);
    let value = |label: &str, nth: usize| h.get_all_by_role_and_label(Role::SpinButton, label).nth(nth).and_then(|n| n.value()).unwrap_or_default();
    assert_eq!(h.get_by_role_and_label(Role::TextInput, "#").value().as_deref(), Some("FFB2B3"));
    assert_eq!(value("G:", 0), "178");
    assert_eq!(value("B:", 1), "179");
    assert_eq!(value("H:", 0), "0°");
}
