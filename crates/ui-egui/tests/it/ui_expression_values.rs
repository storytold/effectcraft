//! #1306 (from #1072): a property driven by an expression shows its value in red, the expression
//! colour, in the Timeline, Effect Controls and the Properties panel, as in After Effects: scalars,
//! every part of a vector (with its separator) and both parts of an angle (with its sign), hovered
//! and dragged too, in dark and light themes. A disabled, removed or empty expression shows the
//! ordinary colour again. The tests read the colour the value text is painted with (the frame's
//! text shapes: the numbers are painted with an override colour, the separator and sign as plain
//! text), and require every expected part to be there.

use effectcraft_engine::Session;
use effectcraft_engine::project::{LayerId, PropGroup};
use effectcraft_ui_egui::EffectcraftApp;
use effectcraft_ui_egui::dock::PanelKind;
use effectcraft_ui_egui::theme::ThemeKind;
use egui::epaint::Shape;
use egui::{Color32, Event, Modifiers, PointerButton, Pos2, Rect, pos2, vec2};
use egui_kittest::Harness;
use serde_json::json;

const TIMELINE: &str = "timeline.prop";
const EFFECT_CONTROLS: &str = "effectControls.prop";
const PROPERTIES: &str = "properties.prop";

#[derive(Clone, Copy, Debug, PartialEq)]
enum Kind {
    /// Two components (X, Y).
    Vector,
    /// Revolutions, sign and degrees.
    Angle,
    Scalar,
}

/// A property whose value the tests check, and the panels that must show it.
#[derive(Clone, Copy, Debug)]
struct Checked {
    uid: u64,
    kind: Kind,
    panels: &'static [&'static str],
}

/// A comp with one solid and the Transform effect on it.
struct Ids {
    layer: u64,
    /// Transform Position, Rotation and Opacity (Timeline and Properties panel), and the Transform
    /// effect's Position, Rotation and Opacity (Effect Controls).
    driven: [Checked; 6],
    /// Transform Scale: no expression.
    scale: Checked,
}

fn setup() -> (Harness<'static, EffectcraftApp>, Ids) {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "E", "width": 320, "height": 180, "duration": 4})).unwrap();
    let l = s.execute("layer.newSolid", json!({"color": "#406080"})).unwrap()["layer"].as_u64().unwrap();
    let fx = s.execute("effect.apply", json!({"layers": [l], "effect": "Transform"})).unwrap()["effects"][0].as_u64().unwrap();
    let layer = s.active_comp().unwrap().layer(LayerId(l)).unwrap().clone();
    let uid = |path: &str| layer.props.prop(path).unwrap_or_else(|| panic!("no {path}")).uid;
    let g: &PropGroup = layer.props.find_group(fx).unwrap();
    let fuid = |m: &str| g.get(m).unwrap_or_else(|| panic!("no effect {m}")).uid;
    const LAYER: &[&str] = &[TIMELINE, PROPERTIES];
    const EFFECT: &[&str] = &[EFFECT_CONTROLS];
    let c = |uid, kind, panels| Checked { uid, kind, panels };
    let ids = Ids {
        layer: l,
        driven: [
            c(uid("transform/position"), Kind::Vector, LAYER),
            c(uid("transform/rotation"), Kind::Angle, LAYER),
            c(uid("transform/opacity"), Kind::Scalar, LAYER),
            c(fuid("position"), Kind::Vector, EFFECT),
            c(fuid("rotation"), Kind::Angle, EFFECT),
            c(fuid("opacity"), Kind::Scalar, EFFECT),
        ],
        scale: c(uid("transform/scale"), Kind::Vector, LAYER),
    };
    let transform = layer.props.sub("transform").unwrap().uid;
    s.state.selected_layers = vec![LayerId(l)];
    let mut app = EffectcraftApp::new(s);
    app.show_panel(PanelKind::EffectControls);
    app.ui.timeline.open_layers.insert(l);
    app.ui.timeline.open_groups.insert(transform);
    let mut h = Harness::builder().with_size(vec2(1600.0, 1000.0)).build_eframe(|_| app);
    h.run_steps(4);
    (h, ids)
}

fn set_expression(h: &mut Harness<'_, EffectcraftApp>, ids: &Ids, prop: u64, p: &serde_json::Value) {
    let mut p = p.clone();
    p["layer"] = json!(ids.layer);
    p["prop"] = json!(prop);
    h.state_mut().session.execute("prop.setExpression", p).unwrap();
}

fn set_theme(h: &mut Harness<'_, EffectcraftApp>, theme: ThemeKind) {
    let ctx = h.ctx.clone();
    h.state_mut().set_theme(&ctx, theme);
    h.run_steps(3);
}

fn rect_of(h: &Harness<'_, EffectcraftApp>, id: &str) -> Rect {
    let e = h.state().auto.find(id).unwrap_or_else(|| panic!("no {id}: the part isn't shown"));
    Rect::from_min_size(pos2(e.rect[0], e.rect[1]), vec2(e.rect[2], e.rect[3]))
}

/// The area of `c`'s value in `panel`, every expected part required: both components of a vector,
/// the revolutions and degrees of an angle (the separator and sign lie between them).
fn value_area(h: &Harness<'_, EffectcraftApp>, panel: &str, c: Checked) -> Rect {
    let uid = c.uid;
    match c.kind {
        Kind::Vector => rect_of(h, &format!("{panel}.{uid}.value.0")).union(rect_of(h, &format!("{panel}.{uid}.value.1"))),
        Kind::Angle => rect_of(h, &format!("{panel}.{uid}.revolutions")).union(rect_of(h, &format!("{panel}.{uid}.value"))),
        Kind::Scalar => rect_of(h, &format!("{panel}.{uid}.value")),
    }
}

/// The text painted inside `area` in the last frame, with its colour.
fn texts(h: &Harness<'_, EffectcraftApp>, area: Rect) -> Vec<(String, Color32)> {
    fn walk(s: &Shape, area: Rect, out: &mut Vec<(String, Color32)>) {
        match s {
            Shape::Vec(v) => v.iter().for_each(|s| walk(s, area, out)),
            Shape::Text(t) if area.expand(1.0).contains_rect(t.visual_bounding_rect()) && !t.galley.text().trim().is_empty() => {
                out.push((t.galley.text().trim().to_string(), t.override_text_color.unwrap_or(t.fallback_color)));
            }
            _ => {}
        }
    }
    let mut out = vec![];
    for c in &h.output().shapes {
        walk(&c.shape, area, &mut out);
    }
    out
}

/// `c`'s value in `panel` shows every part, all in `want`: two numbers and (in the Timeline) the
/// `,` of a vector, the revolutions, sign and degrees of an angle, the number of a scalar.
fn assert_value(h: &Harness<'_, EffectcraftApp>, panel: &str, c: Checked, want: Color32, what: &str) {
    let shown = texts(h, value_area(h, panel, c));
    let numbers = shown.iter().filter(|(s, _)| s.chars().any(|ch| ch.is_ascii_digit())).count();
    let has = |text: &str| shown.iter().any(|(s, _)| s == text);
    let ctx = format!("{what}: {:?} @{} in {panel}: {shown:?}", c.kind, c.uid);
    match c.kind {
        Kind::Vector => {
            assert!(numbers >= 2, "{ctx}: both components");
            if panel == TIMELINE {
                assert!(has(","), "{ctx}: the separator");
            }
        }
        Kind::Angle => {
            assert!(numbers >= 2, "{ctx}: revolutions and degrees");
            assert!(has("+") || has("-"), "{ctx}: the sign");
        }
        Kind::Scalar => assert!(numbers >= 1, "{ctx}: the number"),
    }
    assert!(shown.iter().all(|(_, col)| *col == want), "{ctx}: want {want:?}");
}

/// Every checked property in every panel it belongs to.
fn assert_all(h: &Harness<'_, EffectcraftApp>, checked: &[Checked], want: Color32, what: &str) {
    for c in checked {
        for panel in c.panels {
            assert_value(h, panel, *c, want, what);
        }
    }
}

/// Expression-driven values are red in every panel, in both themes; Scale (none) stays blue.
#[test]
fn expression_driven_values_are_red_in_every_panel() {
    let (mut h, ids) = setup();
    for c in ids.driven {
        set_expression(&mut h, &ids, c.uid, &json!({"expression": "value"}));
    }
    for theme in [ThemeKind::Dark, ThemeKind::Light] {
        set_theme(&mut h, theme);
        let t = h.state().tokens;
        assert_ne!(t.expr_text, t.hot_text);
        assert_all(&h, &ids.driven, t.expr_text, &format!("{theme:?} expression"));
        assert_all(&h, &[ids.scale], t.hot_text, &format!("{theme:?} no expression"));
    }
}

/// Disabled, removed or whitespace-only: the ordinary colour; enabled again: red. Every part of
/// every value is checked again after each change, in both themes.
#[test]
fn expression_state_changes_switch_the_colour() {
    let (mut h, ids) = setup();
    let states = [
        (json!({"expression": "value"}), true, "enabled"),
        (json!({"enabled": false}), false, "disabled"),
        (json!({"enabled": true}), true, "enabled again"),
        (json!({"expression": ""}), false, "removed"),
        (json!({"expression": "value"}), true, "set again"),
        (json!({"expression": "   "}), false, "whitespace only"),
    ];
    for theme in [ThemeKind::Dark, ThemeKind::Light] {
        set_theme(&mut h, theme);
        let t = h.state().tokens;
        for (p, red, what) in &states {
            for c in ids.driven {
                set_expression(&mut h, &ids, c.uid, p);
            }
            h.run_steps(3);
            let want = if *red { t.expr_text } else { t.hot_text };
            assert_all(&h, &ids.driven, want, &format!("{theme:?} {what}"));
            assert_all(&h, &[ids.scale], t.hot_text, &format!("{theme:?} {what}: Scale"));
        }
    }
}

fn hover(h: &mut Harness<'_, EffectcraftApp>, at: Pos2) {
    h.input_mut().events.push(Event::PointerMoved(at));
    h.run_steps(2);
}

/// Hovering a value (an ordinary one takes the hover accent) keeps an expression-driven one red,
/// in every panel and both themes.
#[test]
fn a_hovered_expression_value_stays_red() {
    let (mut h, ids) = setup();
    let opacity = ids.driven.iter().filter(|c| c.kind == Kind::Scalar).copied().collect::<Vec<_>>();
    for c in &opacity {
        set_expression(&mut h, &ids, c.uid, &json!({"expression": "value"}));
    }
    h.run_steps(3);
    for theme in [ThemeKind::Dark, ThemeKind::Light] {
        set_theme(&mut h, theme);
        let t = h.state().tokens;
        for c in &opacity {
            for panel in c.panels {
                let area = value_area(&h, panel, *c);
                hover(&mut h, area.center());
                let at = h.ctx.input(|i| i.pointer.hover_pos());
                assert!(at.is_some_and(|p| area.contains(p)), "{theme:?} {panel}: the pointer is over the value");
                assert_value(&h, panel, *c, t.expr_text, &format!("{theme:?} hovered"));
            }
        }
        // An ordinary value does take the hover accent.
        let area = value_area(&h, TIMELINE, ids.scale);
        let x = rect_of(&h, &format!("{TIMELINE}.{}.value.0", ids.scale.uid)).center();
        hover(&mut h, x);
        let shown = texts(&h, area);
        assert!(shown.iter().any(|(_, c)| *c == t.accent_hover), "{theme:?} an ordinary hovered value: {shown:?}");
        hover(&mut h, pos2(5.0, 5.0));
    }
}

/// Dragging (scrubbing) an expression-driven value keeps it red while the drag is on, in the
/// Timeline and Effect Controls.
#[test]
fn a_dragged_expression_value_stays_red() {
    let (mut h, ids) = setup();
    let opacity = ids.driven.iter().filter(|c| c.kind == Kind::Scalar).copied().collect::<Vec<_>>();
    for c in &opacity {
        set_expression(&mut h, &ids, c.uid, &json!({"expression": "value"}));
    }
    h.run_steps(3);
    let t = h.state().tokens;
    for (c, panel) in [(opacity[0], TIMELINE), (opacity[1], EFFECT_CONTROLS)] {
        let at = value_area(&h, panel, c).center();
        h.input_mut().events.push(Event::PointerMoved(at));
        h.step();
        h.input_mut().events.push(Event::PointerButton { pos: at, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
        h.step();
        for k in 1..=4 {
            h.input_mut().events.push(Event::PointerMoved(at + vec2(3.0 * k as f32, 0.0)));
            h.step();
        }
        assert!(h.ctx.dragged_id().is_some(), "{panel}: a drag is on");
        assert_value(&h, panel, c, t.expr_text, &format!("{panel} dragged"));
        let end = at + vec2(12.0, 0.0);
        h.input_mut().events.push(Event::PointerButton { pos: end, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
        h.run_steps(3);
        hover(&mut h, pos2(5.0, 5.0));
    }
}

/// Separate Dimensions: X Position and Y Position are separate scalars, so an expression on X Position
/// turns only X red, in the Timeline and Properties panel and both themes; Y Position stays blue.
#[test]
fn separated_position_colours_each_dimension_on_its_own() {
    let (mut h, ids) = setup();
    h.state_mut().session.execute("prop.separateDimensions", json!({"layer": ids.layer, "value": true})).unwrap();
    let uid = |h: &Harness<'_, EffectcraftApp>, path: &str| {
        let s = &h.state().session;
        s.active_comp().unwrap().layer(LayerId(ids.layer)).unwrap().props.prop(path).unwrap_or_else(|| panic!("no {path}")).uid
    };
    const LAYER: &[&str] = &[TIMELINE, PROPERTIES];
    let x = Checked { uid: uid(&h, "transform/positionX"), kind: Kind::Scalar, panels: LAYER };
    let y = Checked { uid: uid(&h, "transform/positionY"), kind: Kind::Scalar, panels: LAYER };
    set_expression(&mut h, &ids, x.uid, &json!({"expression": "value"}));
    h.run_steps(3);
    for theme in [ThemeKind::Dark, ThemeKind::Light] {
        set_theme(&mut h, theme);
        let t = h.state().tokens;
        assert_all(&h, &[x], t.expr_text, &format!("{theme:?} X Position (expression)"));
        assert_all(&h, &[y], t.hot_text, &format!("{theme:?} Y Position (none)"));
    }
}
