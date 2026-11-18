//! Compositing Options ▸ Effect Opacity: an effect's result is blended with its input by the
//! percent, as in After Effects (input + (result − input) × opacity, premultiplied).

use effectcraft_color::Label;
use effectcraft_keyframe::Value;
use effectcraft_project::build::{self, Ids};
use effectcraft_project::{Comp, ItemId, ItemKind, LayerSource, Node, Project, Solid};
use effectcraft_time::{FrameRate, Tick};

use crate::{NoFootage, RenderOpts, Renderer};

const W: u32 = 200;
const H: u32 = 100;

/// A white solid with `effect` (its parameters set) at Effect Opacity `opacity`, or without the
/// Effect Opacity property at all (a project saved before it existed) when `None`.
fn frame(effect: &str, set: &[(&str, Value)], opacity: Option<f64>) -> effectcraft_raster::Image {
    let mut p = Project::default();
    let comp = Comp::new(W, H, FrameRate::FPS_30, Tick::from_seconds_f64(1.0));
    let sid = p.add_item("S", Label::Red, None, ItemKind::Solid(Solid { color: [1.0; 3], width: W, height: H, pixel_aspect: 1.0 }));
    let mut l = build::layer(&mut p, &comp, "S", LayerSource::Solid { item: sid }, (W, H), None);
    let spec = effectcraft_effects::find(effect).unwrap();
    let mut next = p.next_id;
    let mut g = effectcraft_effects::instantiate(spec, &mut Ids(&mut next), spec.name, [f64::from(W), f64::from(H)]);
    p.next_id = next;
    for (id, v) in set {
        g.prop_mut(id).unwrap().value = v.clone();
    }
    match opacity {
        Some(o) => g.prop_mut(effectcraft_effects::EFFECT_OPACITY).unwrap().value = Value::Scalar(o),
        None => g.children.retain(|n| !matches!(n, Node::Prop(pr) if pr.match_id == effectcraft_effects::EFFECT_OPACITY)),
    }
    l.props.sub_mut("effects").unwrap().children.push(Node::Group(g));
    let cid: ItemId = p.add_item("C", Label::Sandstone, None, ItemKind::Comp(comp.into()));
    p.comp_mut(cid).unwrap().layers.push(l);
    Renderer::new(&p, &NoFootage, RenderOpts::default()).comp_frame(cid, Tick::ZERO)
}

fn red() -> Vec<(&'static str, Value)> {
    vec![("color", Value::Color([1.0, 0.0, 0.0, 1.0]))]
}

#[test]
fn a_new_effect_has_effect_opacity_100() {
    let spec = effectcraft_effects::find("ec.generate.fill").unwrap();
    let mut next = 1;
    let g = effectcraft_effects::instantiate(spec, &mut Ids(&mut next), spec.name, [100.0, 100.0]);
    assert_eq!(g.prop(effectcraft_effects::EFFECT_OPACITY).map(|p| p.value.clone()), Some(Value::Scalar(100.0)));
}

#[test]
fn effect_opacity_blends_the_result_with_the_input() {
    let g = |o: Option<f64>| frame("ec.generate.fill", &red(), o).get(100, 50)[1];
    // Fill turns white red: green goes 1 -> 0, so green = 1 - opacity.
    assert!(g(Some(100.0)) < 0.01, "{}", g(Some(100.0)));
    assert!((g(Some(37.0)) - 0.63).abs() < 0.01, "{}", g(Some(37.0)));
    assert!((g(Some(0.0)) - 1.0).abs() < 0.01, "{}", g(Some(0.0)));
    // A project from before Effect Opacity existed renders the effect fully.
    assert!(g(None) < 0.01, "{}", g(None));
}

#[test]
fn effect_opacity_keeps_the_input_where_the_effect_moved_pixels() {
    // Transform moves the layer 60 px right: at 50 % the input shows at half strength where the
    // result left it, and the result at half strength where it moved to.
    let set = [("position", Value::Vec2([160.0, 50.0]))];
    let img = frame("ec.distort.transform", &set, Some(50.0));
    assert!((img.get(20, 50)[3] - 0.5).abs() < 0.02, "left of the moved layer: {:?}", img.get(20, 50));
    assert!((img.get(100, 50)[3] - 1.0).abs() < 0.02, "both: {:?}", img.get(100, 50));
}
