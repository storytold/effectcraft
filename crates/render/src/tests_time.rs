//! Time effects through the renderer: neighbouring frames via `EffectHost::self_at`, and the
//! layer cache staying correct while scrubbing.

use effectcraft_color::Label;
use effectcraft_keyframe::{Keyframe, ShapePath, Value};
use effectcraft_project::build::{self, Ids};
use effectcraft_project::{Comp, ItemId, ItemKind, LayerSource, MaskMode, Project, Solid};
use effectcraft_time::{FrameRate, Tick};

fn render(p: &Project, cid: ItemId, t: f64, cache: Option<&crate::LayerCache>) -> crate::Image {
    let mut r = crate::Renderer::new(p, &crate::NoFootage, crate::RenderOpts::default());
    r.cache = cache;
    r.comp_frame(cid, Tick::from_seconds_f64(t))
}

/// A white 200×100 solid whose full-frame mask fades in (opacity 0 → 100 over 0..1 s), with
/// effect `fx` (and parameter overrides) applied.
fn scene(fx: Option<(&str, &[(&str, Value)])>) -> (Project, ItemId) {
    let mut p = Project::default();
    let comp = Comp::new(200, 100, FrameRate::FPS_30, Tick::from_seconds_f64(2.0));
    let cid = p.add_item("Comp", Label::Sandstone, None, ItemKind::Comp(comp.clone().into()));
    let sid = p.add_item("Solid", Label::Red, None, ItemKind::Solid(Solid { color: [1.0, 1.0, 1.0], width: 200, height: 100, pixel_aspect: 1.0 }));
    let mut l = build::layer(&mut p, &comp, "Solid", LayerSource::Solid { item: sid }, (200, 100), None);
    let mut next = p.next_id;
    let mut m = build::mask(&mut Ids(&mut next), "Mask 1", ShapePath::rect([100.0, 50.0], 400.0, 200.0), MaskMode::Add, [255, 255, 0]);
    if let Some(o) = m.prop_mut("opacity") {
        o.keys = vec![Keyframe::new(Tick::ZERO, Value::Scalar(0.0)), Keyframe::new(Tick::from_seconds_f64(1.0), Value::Scalar(100.0))];
    }
    l.props.sub_mut("masks").unwrap().children.push(m.into());
    if let Some((id, vals)) = fx {
        let spec = effectcraft_effects::find(id).unwrap();
        let mut g = effectcraft_effects::instantiate(spec, &mut Ids(&mut next), spec.name, [200.0, 100.0]);
        for (k, v) in vals {
            g.prop_mut(k).unwrap().value = v.clone();
        }
        l.props.sub_mut("effects").unwrap().children.push(g.into());
    }
    p.next_id = next;
    p.comp_mut(cid).unwrap().layers.push(l);
    (p, cid)
}

fn alpha(img: &crate::Image) -> f32 {
    img.get(100, 50)[3]
}

#[test]
fn posterize_time_holds_frames() {
    let (p, cid) = scene(Some(("ec.time.posterizetime", &[("frameRate", Value::Scalar(4.0))])));
    for (t, want) in [(0.3, 0.25), (0.45, 0.25), (0.5, 0.5), (0.74, 0.5), (0.8, 0.75)] {
        let a = alpha(&render(&p, cid, t, None));
        assert!((a - want).abs() < 0.01, "t={t}: {a}");
    }
    let (plain, cid) = scene(None);
    assert!((alpha(&render(&plain, cid, 0.3, None)) - 0.3).abs() < 0.01);
}

#[test]
fn echo_blends_neighbouring_frames() {
    let vals = [("echoTime", Value::Scalar(-0.1)), ("numberOfEchoes", Value::Scalar(2.0)), ("decay", Value::Scalar(0.5))];
    let (p, cid) = scene(Some(("ec.time.echo", &vals)));
    let a = alpha(&render(&p, cid, 0.3, None));
    let want = 0.3 + 0.5 * 0.2 + 0.25 * 0.1;
    assert!((a - want).abs() < 0.01, "{a} vs {want}");
}

/// Time effects see the effects applied before them, as After Effects renders them (SceneYard
/// "Task Preparation UI": Echo after CC Particle Systems II echoed the black solid under the
/// fireworks and blacked out the white page). Fill (red) then Echo: the echoes are red.
#[test]
fn time_effects_see_the_effects_before_them() {
    let (mut p, cid) = scene(Some(("ec.generate.fill", &[])));
    let spec = effectcraft_effects::find("ec.time.echo").unwrap();
    let mut next = p.next_id;
    let mut g = effectcraft_effects::instantiate(spec, &mut Ids(&mut next), spec.name, [200.0, 100.0]);
    g.prop_mut("numberOfEchoes").unwrap().value = Value::Scalar(0.0);
    p.next_id = next;
    p.comp_mut(cid).unwrap().layers[0].props.sub_mut("effects").unwrap().children.push(g.into());
    let c = render(&p, cid, 1.5, None).get(100, 50);
    assert!(c[0] > 0.99 && c[1] < 0.01 && c[2] < 0.01 && c[3] > 0.99, "{c:?}");
}

#[test]
fn time_difference_of_static_layer_is_black() {
    let mut p = Project::default();
    let comp = Comp::new(64, 32, FrameRate::FPS_30, Tick::from_seconds_f64(2.0));
    let cid = p.add_item("Comp", Label::Sandstone, None, ItemKind::Comp(comp.clone().into()));
    let sid = p.add_item("Solid", Label::Red, None, ItemKind::Solid(Solid { color: [0.8, 0.4, 0.2], width: 64, height: 32, pixel_aspect: 1.0 }));
    let mut l = build::layer(&mut p, &comp, "Solid", LayerSource::Solid { item: sid }, (64, 32), None);
    let spec = effectcraft_effects::find("ec.time.timedifference").unwrap();
    let mut next = p.next_id;
    let mut g = effectcraft_effects::instantiate(spec, &mut Ids(&mut next), spec.name, [64.0, 32.0]);
    g.prop_mut("timeOffset").unwrap().value = Value::Scalar(-0.5);
    p.next_id = next;
    l.props.sub_mut("effects").unwrap().children.push(g.into());
    p.comp_mut(cid).unwrap().layers.push(l);
    let img = render(&p, cid, 1.0, None);
    assert!(img.data.iter().all(|px| px[0] == 0.0 && px[1] == 0.0 && px[2] == 0.0 && px[3] == 1.0), "{:?}", img.get(10, 10));
}

/// Scrubbing back and forth with the layer cache gives exactly the uncached frames (Echo's
/// output depends on neighbouring frames, so its key must include the layer time), and an edit
/// to the animation is picked up.
#[test]
fn cache_correct_while_scrubbing() {
    let vals = [("echoTime", Value::Scalar(-0.2)), ("numberOfEchoes", Value::Scalar(3.0)), ("decay", Value::Scalar(0.6)), ("echoOperator", Value::Enum(6))];
    let (mut p, cid) = scene(Some(("ec.time.echo", &vals)));
    let cache = crate::LayerCache::default();
    for t in [0.3, 0.5, 0.3, 1.2, 0.8, 0.5, 0.3, 1.5] {
        let a = render(&p, cid, t, Some(&cache));
        let b = render(&p, cid, t, None);
        assert_eq!(a.data, b.data, "t={t}");
    }
    let st = cache.stats();
    assert!(st.hits > 0, "{st:?}");
    // Re-key the fade (0 → 100 over 0..0.5 s): cached frames must not be served.
    p.comp_mut(cid).unwrap().layers[0].props.prop_mut("masks/#1/opacity").unwrap().keys[1].time = Tick::from_seconds_f64(0.5);
    for t in [0.3, 0.5, 0.8] {
        assert_eq!(render(&p, cid, t, Some(&cache)).data, render(&p, cid, t, None).data, "edited t={t}");
    }
}

/// A layer's input up to an effect keys only the effects before it: editing a later effect (a
/// Puppet pin being dragged, #212) keeps the input cached; editing an earlier one doesn't.
#[test]
fn input_keys_ignore_later_effects() {
    let (mut p, cid) = scene(Some(("ec.blur.gaussian", &[("blurriness", Value::Scalar(4.0))])));
    {
        let spec = effectcraft_effects::find("ec.stylize.mosaic").unwrap();
        let mut next = p.next_id;
        let g = effectcraft_effects::instantiate(spec, &mut Ids(&mut next), spec.name, [200.0, 100.0]);
        p.next_id = next;
        p.comp_mut(cid).unwrap().layers[0].props.sub_mut("effects").unwrap().children.push(g.into());
    }
    let key = |p: &Project| {
        let comp = p.comp(cid).unwrap();
        let ctx = crate::EvalCtx { project: p, comp_id: cid, comp, time: Tick::from_seconds_f64(0.5), expr: None, footage: None };
        let l = &comp.layers[0];
        (crate::cache::input_key(&ctx, l, 1.0, false, false, 1), crate::cache::input_key(&ctx, l, 1.0, false, false, 2))
    };
    let (one, two) = key(&p);
    assert!(one.is_some() && one != two);
    let set = |p: &mut Project, path: &str, v: f64| p.comp_mut(cid).unwrap().layers[0].props.prop_mut(path).unwrap().value = Value::Scalar(v);
    set(&mut p, "effects/#2/horizontal", 30.0);
    assert_eq!(key(&p).0, one, "the second effect doesn't change the input to it");
    assert_ne!(key(&p).1, two);
    set(&mut p, "effects/#1/blurriness", 8.0);
    assert_ne!(key(&p).0, one);
}
