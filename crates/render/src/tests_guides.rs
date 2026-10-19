//! Guide layers: drawn in their own comp's viewer (`RenderOpts::guides`), never when that comp is
//! nested in another (precomp, collapsed precomp, 2D or 3D), and never in final renders.

use effectcraft_color::Label;
use effectcraft_keyframe::Value;
use effectcraft_project::build;
use effectcraft_project::{BitDepth, Comp, ItemId, ItemKind, Layer, LayerSource, Project, Solid};
use effectcraft_time::{FrameRate, Tick};

use crate::{NoFootage, RenderOpts, Renderer};

fn solid(p: &mut Project, comp: &Comp, color: [f32; 3], size: (u32, u32), three_d: bool) -> Layer {
    let sid = p.add_item("S", Label::Red, None, ItemKind::Solid(Solid { color, width: size.0, height: size.1, pixel_aspect: 1.0 }));
    let mut l = build::layer(p, comp, "S", LayerSource::Solid { item: sid }, size, None);
    l.props.prop_mut("transform/position").unwrap().value = Value::Vec3([50.0, 50.0, 0.0]);
    l.switches.three_d = three_d;
    l
}

#[derive(Clone, Copy, Debug)]
enum Nest {
    /// A plain precomp layer (the nested comp rendered on its own, then composited).
    Precomp,
    /// Collapse Transformations on a 2D precomp layer.
    Collapsed,
    /// A collapsed 3D precomp in a Classic 3D comp.
    Collapsed3d,
    /// A collapsed 3D precomp in an Advanced 3D comp.
    CollapsedAdvanced,
}

/// "Inner": a red 100×100 background with a green 40×40 guide layer in the middle; "Outer": the
/// same size, holding only Inner nested as `nest`.
fn scene(nest: Nest) -> (Project, ItemId, ItemId) {
    let three_d = matches!(nest, Nest::Collapsed3d | Nest::CollapsedAdvanced);
    let mut p = Project::default();
    p.settings.bit_depth = BitDepth::Bpc32;
    let inner = Comp::new(100, 100, FrameRate::FPS_30, Tick::from_seconds_f64(2.0));
    let mut guide = solid(&mut p, &inner, [0.0, 1.0, 0.0], (40, 40), three_d);
    guide.switches.guide = true;
    let bg = solid(&mut p, &inner, [1.0, 0.0, 0.0], (100, 100), three_d);
    let mut inner = inner;
    inner.layers = vec![guide, bg];
    let iid = p.add_item("Inner", Label::Sandstone, None, ItemKind::Comp(inner.into()));
    let mut outer = Comp::new(100, 100, FrameRate::FPS_30, Tick::from_seconds_f64(2.0));
    if matches!(nest, Nest::CollapsedAdvanced) {
        outer.renderer = effectcraft_project::Renderer::Advanced3D;
    }
    let mut pre = build::layer(&mut p, &outer, "Inner", LayerSource::Comp { item: iid }, (100, 100), None);
    pre.switches.collapse = !matches!(nest, Nest::Precomp);
    pre.switches.three_d = three_d;
    outer.layers = vec![pre];
    let oid = p.add_item("Outer", Label::Sandstone, None, ItemKind::Comp(outer.into()));
    (p, iid, oid)
}

/// The centre pixel of `comp` rendered with or without guide layers.
fn centre(p: &Project, comp: ItemId, guides: bool) -> [f32; 4] {
    Renderer::new(p, &NoFootage, RenderOpts { guides, ..Default::default() }).comp_frame(comp, Tick::ZERO).get(50, 50)
}

fn is_green(c: [f32; 4]) -> bool {
    c[1] > 0.9 && c[0] < 0.1
}

fn is_red(c: [f32; 4]) -> bool {
    c[0] > 0.9 && c[1] < 0.1
}

#[test]
fn guide_layer_renders_in_its_own_comp_with_guides_on() {
    let (p, inner, _) = scene(Nest::Precomp);
    let c = centre(&p, inner, true);
    assert!(is_green(c), "guide layer drawn in its own comp's viewer: {c:?}");
}

#[test]
fn final_renders_leave_guide_layers_out_by_default() {
    assert!(!RenderOpts::default().guides);
    let (p, inner, _) = scene(Nest::Precomp);
    let c = centre(&p, inner, false);
    assert!(is_red(c), "guide layer left out of final renders: {c:?}");
}

#[test]
fn guide_layers_of_a_nested_comp_never_render_in_the_parent() {
    for nest in [Nest::Precomp, Nest::Collapsed, Nest::Collapsed3d, Nest::CollapsedAdvanced] {
        let (p, inner, outer) = scene(nest);
        // The nested comp's own viewer still shows them…
        let own = centre(&p, inner, true);
        assert!(is_green(own), "{nest:?}: guide layer in its own comp: {own:?}");
        // …but not the parent's viewer (guides on) nor its final render.
        for guides in [true, false] {
            let c = centre(&p, outer, guides);
            assert!(is_red(c), "{nest:?}, guides {guides}: nested guide layer drawn in the parent: {c:?}");
        }
    }
}

#[test]
fn a_guide_precomp_layer_of_the_top_comp_follows_the_guides_option() {
    // The precomp layer itself is a guide layer in the top comp: shown in the viewer, not in
    // final renders.
    let (mut p, _, outer) = scene(Nest::Precomp);
    p.comp_mut(outer).unwrap().layers[0].switches.guide = true;
    assert!(is_red(centre(&p, outer, true)));
    assert!(centre(&p, outer, false)[3] < 0.01, "nothing drawn without guides");
}
