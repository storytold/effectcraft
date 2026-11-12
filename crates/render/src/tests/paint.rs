//! Paint and Puppet through the full pipeline: layer cache keys, resolution, determinism.

use effectcraft_color::Label;
use effectcraft_effects::paint::{self, StrokeSpec};
use effectcraft_effects::puppet::{self, MeshOpts, PinKind};
use effectcraft_keyframe::{Keyframe, Value};
use effectcraft_project::build::{self, Ids};
use effectcraft_project::{Comp, ItemId, ItemKind, LayerSource, Project, PropGroup, Solid};
use effectcraft_time::{FrameRate, Tick};

use crate::{LayerCache, NoFootage, RenderOpts, Renderer};

fn scene(fx: impl FnOnce(&mut Ids) -> PropGroup) -> (Project, ItemId) {
    let mut p = Project::default();
    let comp = Comp::new(100, 80, FrameRate::FPS_30, Tick::from_seconds_f64(3.0));
    let cid = p.add_item("Comp", Label::Sandstone, None, ItemKind::Comp(comp.clone().into()));
    let sid = p.add_item("Solid", Label::Red, None, ItemKind::Solid(Solid { color: [0.0, 0.0, 1.0], width: 60, height: 40, pixel_aspect: 1.0 }));
    let mut l = build::layer(&mut p, &comp, "Solid", LayerSource::Solid { item: sid }, (60, 40), None);
    let mut next = p.next_id;
    let g = fx(&mut Ids(&mut next));
    p.next_id = next;
    l.props.sub_mut("effects").unwrap().children.push(g.into());
    p.comp_mut(cid).unwrap().layers.push(l);
    (p, cid)
}

fn paint_fx(ids: &mut Ids, strokes: &[StrokeSpec]) -> PropGroup {
    let mut g = effectcraft_effects::instantiate(effectcraft_effects::find(paint::ID).unwrap(), ids, "Paint", [60.0, 40.0]);
    for (i, s) in strokes.iter().enumerate() {
        g.children.push(paint::stroke_group(ids, &format!("Brush {}", i + 1), s).into());
    }
    g
}

fn frame(p: &Project, cid: ItemId, t: f64, scale: f64, cache: Option<&LayerCache>) -> crate::Image {
    let mut r = Renderer::new(p, &NoFootage, RenderOpts { scale, ..Default::default() });
    r.cache = cache;
    r.comp_frame(cid, Tick::from_seconds_f64(t))
}

#[test]
fn stroke_spans_are_part_of_the_cache_key() {
    // Layer at comp (20, 20)–(80, 60). A red dab at layer (10, 10) = comp (30, 30), shown 1–2 s.
    let s = StrokeSpec { points: vec![[10.5, 10.5]], color: [1.0, 0.0, 0.0, 1.0], in_time: 1.0, out_time: 2.0, size_pressure: false, ..Default::default() };
    let (p, cid) = scene(|ids| paint_fx(ids, &[s]));
    let cache = LayerCache::default();
    for t in [0.5, 1.5, 0.5, 2.5, 1.5] {
        let a = frame(&p, cid, t, 1.0, Some(&cache));
        let b = frame(&p, cid, t, 1.0, None);
        assert_eq!(a, b, "t={t}");
        let red = a.get(30, 30)[0] > 0.99;
        assert_eq!(red, (1.0..2.0).contains(&t), "t={t}");
    }
    assert!(cache.stats().hits > 0);
}

#[test]
fn write_on_keys_animate_end() {
    let s = StrokeSpec { points: vec![[5.5, 20.5], [55.5, 20.5]], color: [1.0, 0.0, 0.0, 1.0], size_pressure: false, ..Default::default() };
    let (mut p, cid) = scene(|ids| paint_fx(ids, &[s]));
    {
        let l = &mut p.comp_mut(cid).unwrap().layers[0];
        let end = l.props.prop_mut("effects/#1/brush/stroke_options/end").unwrap();
        end.keys = vec![Keyframe::new(Tick::ZERO, Value::Scalar(0.0)), Keyframe::new(Tick::from_seconds_f64(1.0), Value::Scalar(100.0))];
    }
    let cache = LayerCache::default();
    let at = |t: f64, x: u32| frame(&p, cid, t, 1.0, Some(&cache)).get(x as i64, 40)[0] > 0.99;
    // Layer x 5..55 → comp x 25..75.
    assert!(!at(0.0, 30));
    assert!(at(0.5, 30) && at(0.5, 45) && !at(0.5, 65));
    assert!(at(1.0, 74));
}

#[test]
fn paint_respects_resolution() {
    let s = StrokeSpec { points: vec![[10.5, 10.5], [40.5, 30.5]], diameter: 8.0, color: [1.0, 0.0, 0.0, 1.0], ..Default::default() };
    let (p, cid) = scene(|ids| paint_fx(ids, &[s]));
    let full = frame(&p, cid, 0.0, 1.0, None);
    let half = frame(&p, cid, 0.0, 0.5, None);
    assert_eq!((half.width, half.height), (50, 40));
    // Same spot, both resolutions.
    assert!(full.get(45, 40)[0] > 0.9);
    assert!(half.get(22, 20)[0] > 0.9);
}

#[test]
fn puppet_moves_pixels_and_caches() {
    let (mut p, cid) = scene(|ids| {
        let mut fx = effectcraft_effects::instantiate(effectcraft_effects::find(puppet::ID).unwrap(), ids, "Puppet", [60.0, 40.0]);
        let mut m = puppet::mesh_group(ids, "Mesh 1", [30.0, 20.0], &MeshOpts::default());
        let mut pin = puppet::pin_group(ids, "Puppet Pin 1", PinKind::Position, [30.0, 20.0]);
        pin.get_mut("position").unwrap().value = Value::Vec2([30.0, 20.0]);
        m.sub_mut("deform").unwrap().children.push(pin.into());
        fx.children.push(m.into());
        fx
    });
    let cache = LayerCache::default();
    let still = frame(&p, cid, 0.0, 1.0, Some(&cache));
    assert_eq!(still.get(50, 40), [0.0, 0.0, 1.0, 1.0]);
    assert_eq!(still.get(85, 40)[3], 0.0);
    {
        let l = &mut p.comp_mut(cid).unwrap().layers[0];
        let pos = l.props.prop_mut("effects/#1/mesh/deform/pin/position").unwrap();
        pos.keys = vec![Keyframe::new(Tick::ZERO, Value::Vec2([30.0, 20.0])), Keyframe::new(Tick::from_seconds_f64(1.0), Value::Vec2([40.0, 20.0]))];
    }
    let moved = frame(&p, cid, 1.0, 1.0, Some(&cache));
    assert_eq!(moved, frame(&p, cid, 1.0, 1.0, None));
    // The 60 px wide layer shifted right by 10: comp x 30..90.
    assert_eq!(moved.get(85, 40), [0.0, 0.0, 1.0, 1.0]);
    assert_eq!(moved.get(25, 40)[3], 0.0);
    let half = frame(&p, cid, 1.0, 0.5, None);
    assert!(half.get(42, 20)[3] > 0.99);
}
