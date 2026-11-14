//! Mask paths for effects that take the shape itself (external effect helpers, #3665): a
//! layer's masks in its effect space with their settings, another layer's at another time, a
//! text layer's glyph outlines, and the owner's cache key following another layer's masks.

use effectcraft_color::Label;
use effectcraft_keyframe::{ShapePath, TextDoc, Value};
use effectcraft_project::build::{self, Ids};
use effectcraft_project::{Comp, GroupKind, ItemId, ItemKind, Layer, LayerSource, MaskMode, Node, Project, Solid};
use effectcraft_time::{FrameRate, Tick};

use crate::fx_host::FxHost;
use crate::{EffectHost, EvalCtx, NoFootage, RenderOpts, Renderer};

fn project() -> (Project, ItemId, Comp) {
    let mut p = Project::default();
    let comp = Comp::new(200, 100, FrameRate::FPS_30, Tick::from_seconds_f64(2.0));
    let cid = p.add_item("Comp", Label::Sandstone, None, ItemKind::Comp(comp.clone().into()));
    (p, cid, comp)
}

fn add_mask(p: &mut Project, l: &mut Layer, shape: ShapePath, mode: MaskMode) {
    let mut next = p.next_id;
    let mut m = build::mask(&mut Ids(&mut next), "Mask 1", shape, mode, [255, 0, 0]);
    p.next_id = next;
    m.get_mut("opacity").unwrap().value = Value::Scalar(50.0);
    m.get_mut("expansion").unwrap().value = Value::Scalar(3.0);
    m.get_mut("feather").unwrap().value = Value::Vec2([4.0, 5.0]);
    if let GroupKind::Mask { inverted, .. } = &mut m.kind {
        *inverted = true;
    }
    l.props.sub_mut("masks").unwrap().children.push(Node::Group(m));
}

fn solid(p: &mut Project, comp: &Comp, w: u32, h: u32) -> Layer {
    let sid = p.add_item("S", Label::Red, None, ItemKind::Solid(Solid { color: [1.0; 3], width: w, height: h, pixel_aspect: 1.0 }));
    build::layer(p, comp, "S", LayerSource::Solid { item: sid }, (w, h), None)
}

fn text(p: &mut Project, comp: &Comp, s: &str) -> Layer {
    let mut l = build::layer(p, comp, "T", LayerSource::Text, (200, 100), None);
    l.props.prop_mut("text/sourceText").unwrap().value = Value::Text(Box::new(TextDoc { text: s.into(), size: 40.0, ..Default::default() }));
    l
}

/// Run `f` with the effect host of layer `layer`'s stack, as the pipeline makes it.
fn with_host<T>(p: &Project, cid: ItemId, layer: usize, f: impl FnOnce(&dyn EffectHost) -> T) -> T {
    let comp = p.comp(cid).unwrap();
    let r = Renderer::new(p, &NoFootage, RenderOpts::default());
    let ctx = EvalCtx::new(p, cid, comp, Tick::ZERO);
    let l = &comp.layers[layer];
    let origin = ctx.effect_bounds(l).1;
    let host = FxHost { r: &r, ctx: &ctx, layer: l, origin, index: 0, original: None };
    f(&host)
}

#[test]
fn a_layers_masks_come_with_their_settings() {
    let (mut p, cid, comp) = project();
    let mut l = solid(&mut p, &comp, 120, 80);
    add_mask(&mut p, &mut l, ShapePath::rect([30.0, 40.0], 40.0, 40.0), MaskMode::Subtract);
    p.comp_mut(cid).unwrap().layers.push(l);
    let masks = with_host(&p, cid, 0, |h| h.mask_paths());
    assert_eq!(masks.len(), 1);
    let m = &masks[0];
    assert_eq!((m.mode.as_str(), m.inverted, m.closed), ("subtract", true, true));
    assert_eq!((m.opacity, m.expansion, m.feather), (0.5, 3.0, [4.0, 5.0]));
    assert!(m.vertices.contains(&[10.0, 20.0]) && m.vertices.contains(&[50.0, 60.0]), "{:?}", m.vertices);
    assert_eq!((m.in_tangents.len(), m.out_tangents.len()), (m.vertices.len(), m.vertices.len()));
}

#[test]
fn a_text_layers_masks_are_in_its_effect_space_and_its_glyphs_come_along() {
    let (mut p, cid, comp) = project();
    let mut t = text(&mut p, &comp, "OK");
    add_mask(&mut p, &mut t, ShapePath::rect([5.0, 5.0], 10.0, 10.0), MaskMode::Add);
    p.comp_mut(cid).unwrap().layers.push(t);
    // The text layer's effect space is comp-sized, centred on its origin: layer (0, 0) is (100, 50).
    let own = with_host(&p, cid, 0, |h| h.mask_paths());
    assert!(own[0].vertices.contains(&[100.0, 50.0]), "{:?}", own[0].vertices);
    // Seen from another layer: the mask, then a closed difference path per glyph contour.
    let s = solid(&mut p, &comp, 50, 50);
    p.comp_mut(cid).unwrap().layers.push(s);
    let id = p.comp(cid).unwrap().layers[0].id.0;
    let (paths, size) = with_host(&p, cid, 1, |h| h.layer_mask_paths(id, 0.0)).unwrap();
    assert_eq!(size, [200.0, 100.0]);
    assert!(paths[0].vertices.contains(&[100.0, 50.0]));
    let glyphs = &paths[1..];
    // "O" has two contours, "K" at least one.
    assert!(glyphs.len() >= 3, "{}", glyphs.len());
    assert!(glyphs.iter().all(|g| g.closed && g.mode == "difference" && !g.vertices.is_empty()));
    assert_eq!(glyphs[0].name, "O");
    // A layer that isn't there has none.
    assert!(with_host(&p, cid, 1, |h| h.layer_mask_paths(999_999, 0.0)).is_none());
}

#[test]
fn another_layers_masks_are_read_at_the_time_asked() {
    let (mut p, cid, comp) = project();
    let mut other = solid(&mut p, &comp, 100, 100);
    add_mask(&mut p, &mut other, ShapePath::rect([5.0, 5.0], 10.0, 10.0), MaskMode::Add);
    // The mask path moves: a keyframe at 1 s puts it at (40, 40).
    {
        let m = other
            .props
            .sub_mut("masks")
            .unwrap()
            .children
            .iter_mut()
            .find_map(|n| match n {
                Node::Group(g) => Some(g),
                _ => None,
            })
            .unwrap();
        let pr = m.get_mut("path").unwrap();
        let moved = Value::Path(ShapePath::rect([45.0, 45.0], 10.0, 10.0));
        pr.keys =
            vec![effectcraft_keyframe::Keyframe::new(Tick::ZERO, pr.value.clone()), effectcraft_keyframe::Keyframe::new(Tick::from_seconds_f64(1.0), moved)];
    }
    p.comp_mut(cid).unwrap().layers.push(other);
    let s = solid(&mut p, &comp, 100, 100);
    p.comp_mut(cid).unwrap().layers.push(s);
    let id = p.comp(cid).unwrap().layers[0].id.0;
    let at = |t: f64| with_host(&p, cid, 1, |h| h.layer_mask_paths(id, t)).unwrap().0;
    assert!(at(0.0)[0].vertices.contains(&[0.0, 0.0]));
    assert!(at(1.0)[0].vertices.contains(&[40.0, 40.0]));
}

/// The cache key of layer 1 (Compound Blur reading layer 0) changes when layer 0's mask does, and
/// only then: an effect asking for that layer's masks (`layer_masks`) never sees stale ones.
#[test]
fn the_owner_key_follows_the_other_layers_masks() {
    let (mut p, cid, comp) = project();
    let mut other = solid(&mut p, &comp, 100, 100);
    add_mask(&mut p, &mut other, ShapePath::rect([5.0, 5.0], 10.0, 10.0), MaskMode::Add);
    let other_id = other.id.0;
    p.comp_mut(cid).unwrap().layers.push(other);
    let mut owner = solid(&mut p, &comp, 100, 100);
    let spec = effectcraft_effects::find("ec.blur.compound").unwrap();
    let mut next = p.next_id;
    let mut g = effectcraft_effects::instantiate(spec, &mut Ids(&mut next), spec.name, [100.0, 100.0]);
    p.next_id = next;
    g.prop_mut("blurLayer").unwrap().value = Value::Layer(Some(other_id));
    owner.props.sub_mut("effects").unwrap().children.push(g.into());
    p.comp_mut(cid).unwrap().layers.push(owner);
    let key = |p: &Project| {
        let comp = p.comp(cid).unwrap();
        let ctx = EvalCtx::new(p, cid, comp, Tick::ZERO);
        crate::cache::layer_key(&ctx, &comp.layers[1], 1.0, false, false)
    };
    let before = key(&p);
    assert!(before.is_some());
    assert_eq!(key(&p.clone()), before, "nothing changed");
    let mut q = p.clone();
    let m = q.comp_mut(cid).unwrap().layers[0]
        .props
        .sub_mut("masks")
        .unwrap()
        .children
        .iter_mut()
        .find_map(|n| match n {
            Node::Group(g) => Some(g),
            _ => None,
        })
        .unwrap();
    m.get_mut("path").unwrap().value = Value::Path(ShapePath::rect([10.0, 5.0], 20.0, 10.0));
    assert_ne!(key(&q), before, "the other layer's mask path");
    let mut q = p.clone();
    let m = q.comp_mut(cid).unwrap().layers[0]
        .props
        .sub_mut("masks")
        .unwrap()
        .children
        .iter_mut()
        .find_map(|n| match n {
            Node::Group(g) => Some(g),
            _ => None,
        })
        .unwrap();
    m.get_mut("expansion").unwrap().value = Value::Scalar(9.0);
    assert_ne!(key(&q), before, "the other layer's mask expansion");
}
