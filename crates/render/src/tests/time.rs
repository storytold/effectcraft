//! Time effects through the renderer: neighbouring frames via `EffectHost::self_at`, and the
//! layer cache staying correct while scrubbing.

use std::sync::Arc;

use effectcraft_color::Label;
use effectcraft_effects::plugin::{EffectPlugin, PluginFrame, PluginManifest, PluginParams, register_plugin};
use effectcraft_keyframe::{Keyframe, ShapePath, Value};
use effectcraft_project::build::{self, Ids};
use effectcraft_project::styles as st;
use effectcraft_project::{Comp, Footage, FootageKind, ItemId, ItemKind, LayerSource, MaskMode, Project, Quality, Solid};
use effectcraft_time::{FrameRate, Tick};

use crate::{FootageSource, Image};

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
        let ctx = crate::EvalCtx {
            project: p,
            comp_id: cid,
            comp,
            time: Tick::from_seconds_f64(0.5),
            expr: None,
            footage: None,
            property_overrides: &[],
            media_overrides: &[],
            reads: None,
        };
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

/// A plug-in that reads the layer's neighbouring frames (WebAssembly ABI 2) makes its layer's
/// cache key depend on the layer time, even when the source and every property are static. A
/// plug-in that does not keeps the old key (#1977: a static layer stays cached across a scrub).
#[test]
fn abi2_plugin_layers_key_on_layer_time() {
    struct NeighbourReader {
        m: PluginManifest,
    }

    impl EffectPlugin for NeighbourReader {
        fn manifest(&self) -> &PluginManifest {
            &self.m
        }

        fn render(&self, _frame: &mut PluginFrame, _params: &PluginParams, _time: f64) -> Result<(), String> {
            Ok(())
        }

        fn reads_other_times(&self) -> bool {
            true
        }
    }

    struct Plain {
        m: PluginManifest,
    }

    impl EffectPlugin for Plain {
        fn manifest(&self) -> &PluginManifest {
            &self.m
        }

        fn render(&self, _frame: &mut PluginFrame, _params: &PluginParams, _time: f64) -> Result<(), String> {
            Ok(())
        }
    }

    /// A static white solid with one plug-in effect, registered by `plugin`.
    fn plugin_scene(plugin: Arc<dyn EffectPlugin>) -> (Project, ItemId) {
        let mut p = Project::default();
        let comp = Comp::new(200, 100, FrameRate::FPS_30, Tick::from_seconds_f64(2.0));
        let cid = p.add_item("Comp", Label::Sandstone, None, ItemKind::Comp(comp.clone().into()));
        let sid = p.add_item("Solid", Label::Red, None, ItemKind::Solid(Solid { color: [1.0, 1.0, 1.0], width: 200, height: 100, pixel_aspect: 1.0 }));
        let mut l = build::layer(&mut p, &comp, "Solid", LayerSource::Solid { item: sid }, (200, 100), None);
        let spec = register_plugin(plugin).unwrap();
        let mut next = p.next_id;
        let g = effectcraft_effects::instantiate(spec, &mut Ids(&mut next), spec.name, [200.0, 100.0]);
        p.next_id = next;
        l.props.sub_mut("effects").unwrap().children.push(g.into());
        p.comp_mut(cid).unwrap().layers.push(l);
        (p, cid)
    }

    fn key(p: &Project, cid: ItemId, t: f64) -> Option<u64> {
        let comp = p.comp(cid).unwrap();
        let ctx = crate::EvalCtx {
            project: p,
            comp_id: cid,
            comp,
            time: Tick::from_seconds_f64(t),
            expr: None,
            footage: None,
            property_overrides: &[],
            media_overrides: &[],
            reads: None,
        };
        crate::cache::layer_key(&ctx, &comp.layers[0], 1.0, false, false)
    }

    let manifest = |id: &str| PluginManifest {
        api: 2,
        id: id.into(),
        name: "Key".into(),
        category: "Test Plug-ins".into(),
        version: "1".into(),
        author: String::new(),
        description: String::new(),
        params: vec![],
    };

    let (plain, c1) = plugin_scene(Arc::new(Plain { m: manifest("org.test.render.time-key.plain") }));
    assert_eq!(key(&plain, c1, 0.25), key(&plain, c1, 0.5), "a plain plug-in keeps the old key");
    let (reader, c2) = plugin_scene(Arc::new(NeighbourReader { m: manifest("org.test.render.time-key.reader") }));
    assert_ne!(key(&reader, c2, 0.25), key(&reader, c2, 0.5), "an ABI 2 plug-in keys on the layer time");
}

/// Footage whose frame `i` is a flat grey of `i / 100`; `bright` starts from 0.5 instead, so two
/// clips can be told apart. The path picks the level.
struct GreyFrames;
impl FootageSource for GreyFrames {
    fn frame(&self, _: ItemId, f: &Footage, t: Tick) -> Option<Arc<Image>> {
        let i = f.frame_rate.frame_at(t) as f32;
        let g = if f.path.contains("b.mp4") { 0.5 - i / 100.0 } else { i / 100.0 };
        Some(Arc::new(Image::filled(f.width, f.height, [g, g, g, 1.0])))
    }
}

/// A 96×54 comp at 30 fps with footage `path` as its only content layer, and an adjustment layer
/// carrying Posterize Time at `rate` fps. Returns the project, the comp and the clip item.
fn posterize_scene(path: &str, rate: f64) -> (Project, ItemId) {
    let mut p = Project::default();
    p.settings.bit_depth = effectcraft_project::BitDepth::Bpc32;
    let comp = Comp::new(96, 54, FrameRate::FPS_30, Tick::from_seconds_f64(2.0));
    let cid = p.add_item("Comp", Label::Sandstone, None, ItemKind::Comp(comp.clone().into()));
    let f = Footage {
        path: path.into(),
        kind: FootageKind::Video,
        width: 96,
        height: 54,
        pixel_aspect: 1.0,
        frame_rate: FrameRate::FPS_30,
        duration: Tick::from_seconds_f64(2.0),
        has_video: true,
        alpha: effectcraft_project::AlphaMode::Ignore,
        loop_count: 1,
        ..Default::default()
    };
    let fid = p.add_item("clip", Label::Aqua, None, ItemKind::Footage(f));
    let footage = build::layer(&mut p, &comp, "clip", LayerSource::Footage { item: fid }, (96, 54), None);
    let sid = p.add_item("Solid", Label::Red, None, ItemKind::Solid(Solid { color: [1.0, 1.0, 1.0], width: 96, height: 54, pixel_aspect: 1.0 }));
    let mut adj = build::layer(&mut p, &comp, "Adj", LayerSource::Solid { item: sid }, (96, 54), None);
    adj.switches.adjustment = true;
    let spec = effectcraft_effects::find("ec.time.posterizetime").unwrap();
    let mut next = p.next_id;
    let mut g = effectcraft_effects::instantiate(spec, &mut Ids(&mut next), spec.name, [96.0, 54.0]);
    g.prop_mut("frameRate").unwrap().value = Value::Scalar(rate);
    p.next_id = next;
    adj.props.sub_mut("effects").unwrap().children.push(g.into());
    p.comp_mut(cid).unwrap().layers = vec![adj, footage].into();
    (p, cid)
}

/// #4001: Posterize Time on an adjustment layer posterizes the comp below. The adjustment's
/// effects run on the composite, so the time effect must read that composite at the held time,
/// not pass its own (solid white) source through.
#[test]
fn posterize_time_on_an_adjustment_layer_holds_the_comp_below() {
    let (p, cid) = posterize_scene("clip.mp4", 4.0);
    let img = |t: f64| {
        let r = crate::Renderer::new(&p, &GreyFrames, crate::RenderOpts::default());
        r.comp_frame(cid, Tick::from_seconds_f64(t))
    };
    // At 4 fps the layer holds t rounded down to 1/4 s: 0.3 s holds frame 7, 0.55 s frame 15,
    // 0.8 s frame 22 (frame i is grey i / 100).
    for (t, want) in [(0.05, 0.0), (0.3, 0.07), (0.55, 0.15), (0.8, 0.22)] {
        let got = img(t).get(48, 27)[0];
        assert!((got - want).abs() < 1e-6, "t={t}: {got}, want {want}");
    }
}

/// The held time is a *comp* time: an adjustment layer starting at 0.5 s maps its own layer time to
/// comp time before reading the composite below (at comp time 1.3 s it holds 1.25 s, not 0.75 s).
#[test]
fn posterize_time_on_an_adjustment_layer_maps_layer_time_to_comp_time() {
    let (mut p, cid) = posterize_scene("clip.mp4", 4.0);
    p.comp_mut(cid).unwrap().layers[0].start_time = Tick::from_seconds_f64(0.5);
    let r = crate::Renderer::new(&p, &GreyFrames, crate::RenderOpts::default());
    let got = r.comp_frame(cid, Tick::from_seconds_f64(1.3)).get(48, 27)[0];
    // Layer time 0.8 s → held 0.75 s → comp time 1.25 s → frame 37.
    assert!((got - 0.37).abs() < 1e-6, "{got}");
}

/// The composite below follows the compositor's visibility rules: with the middle clip soloed, the
/// non-solo clip under it is not part of the held frame (the solo clip is a small bright square, so
/// a rule that ignored Solo would show the other clip around it).
#[test]
fn posterize_time_on_an_adjustment_layer_respects_solo() {
    let mut p = Project::default();
    p.settings.bit_depth = effectcraft_project::BitDepth::Bpc32;
    let comp = Comp::new(96, 54, FrameRate::FPS_30, Tick::from_seconds_f64(2.0));
    let cid = p.add_item("Comp", Label::Sandstone, None, ItemKind::Comp(comp.clone().into()));
    let clip = |p: &mut Project, path: &str| {
        let f = Footage {
            path: path.into(),
            kind: FootageKind::Video,
            width: 96,
            height: 54,
            pixel_aspect: 1.0,
            frame_rate: FrameRate::FPS_30,
            duration: Tick::from_seconds_f64(2.0),
            has_video: true,
            alpha: effectcraft_project::AlphaMode::Ignore,
            loop_count: 1,
            ..Default::default()
        };
        let fid = p.add_item("clip", Label::Aqua, None, ItemKind::Footage(f));
        build::layer(p, &comp, "clip", LayerSource::Footage { item: fid }, (96, 54), None)
    };
    let under = clip(&mut p, "a.mp4");
    let mut solo = clip(&mut p, "b.mp4");
    solo.switches.solo = true;
    solo.props.prop_mut("transform/position").unwrap().value = Value::Vec3([48.0, 27.0, 0.0]);
    // A 20×20 window onto the solo clip's level: scaled down so only its centre reaches the comp.
    solo.props.prop_mut("transform/scale").unwrap().value = Value::Vec3([20.0, 20.0, 100.0]);
    let sid = p.add_item("Solid", Label::Red, None, ItemKind::Solid(Solid { color: [1.0, 1.0, 1.0], width: 96, height: 54, pixel_aspect: 1.0 }));
    let mut adj = build::layer(&mut p, &comp, "Adj", LayerSource::Solid { item: sid }, (96, 54), None);
    adj.switches.adjustment = true;
    adj.switches.solo = true;
    let spec = effectcraft_effects::find("ec.time.posterizetime").unwrap();
    let mut next = p.next_id;
    let mut g = effectcraft_effects::instantiate(spec, &mut Ids(&mut next), spec.name, [96.0, 54.0]);
    g.prop_mut("frameRate").unwrap().value = Value::Scalar(4.0);
    p.next_id = next;
    adj.props.sub_mut("effects").unwrap().children.push(g.into());
    p.comp_mut(cid).unwrap().layers = vec![adj, solo, under].into();
    let r = crate::Renderer::new(&p, &GreyFrames, crate::RenderOpts::default());
    let img = r.comp_frame(cid, Tick::from_seconds_f64(0.3));
    // Only the solo clip draws: inside its square it holds frame 7 of `b` (0.5 − 0.07), outside it
    // is transparent (the non-solo clip must not be composited).
    let inside = img.get(48, 27)[0];
    let outside = img.get(5, 5);
    assert!((inside - (0.5 - 0.07)).abs() < 1e-6, "inside: {inside}");
    assert!(outside[3] == 0.0, "outside: {outside:?}");
}

/// A Wireframe-quality layer draws as a one-pixel outline in the comp, not as its filled content.
/// The sampled composite must decline it, so the time effect passes the outline through instead of
/// holding a filled frame the comp never shows.
#[test]
fn posterize_time_on_an_adjustment_layer_declines_wireframe_layers() {
    let mut p = Project::default();
    p.settings.bit_depth = effectcraft_project::BitDepth::Bpc32;
    let comp = Comp::new(96, 54, FrameRate::FPS_30, Tick::from_seconds_f64(2.0));
    let cid = p.add_item("Comp", Label::Sandstone, None, ItemKind::Comp(comp.clone().into()));
    let sid = p.add_item("Solid", Label::Red, None, ItemKind::Solid(Solid { color: [1.0, 0.0, 0.0], width: 96, height: 54, pixel_aspect: 1.0 }));
    let mut under = build::layer(&mut p, &comp, "Wire", LayerSource::Solid { item: sid }, (96, 54), None);
    under.switches.quality = Quality::Wireframe;
    let aid = p.add_item("AdjSolid", Label::Red, None, ItemKind::Solid(Solid { color: [1.0, 1.0, 1.0], width: 96, height: 54, pixel_aspect: 1.0 }));
    let mut adj = build::layer(&mut p, &comp, "Adj", LayerSource::Solid { item: aid }, (96, 54), None);
    adj.switches.adjustment = true;
    let spec = effectcraft_effects::find("ec.time.posterizetime").unwrap();
    let mut next = p.next_id;
    let mut g = effectcraft_effects::instantiate(spec, &mut Ids(&mut next), spec.name, [96.0, 54.0]);
    g.prop_mut("frameRate").unwrap().value = Value::Scalar(4.0);
    p.next_id = next;
    adj.props.sub_mut("effects").unwrap().children.push(g.into());
    p.comp_mut(cid).unwrap().layers = vec![adj, under].into();
    let r = crate::Renderer::new(&p, &crate::NoFootage, crate::RenderOpts::default());
    let img = r.comp_frame(cid, Tick::from_seconds_f64(0.3));
    // The compositor draws the wireframe layer as an outline: the centre stays unfilled.
    // Compositing its filled content buffer here would show a held red frame instead.
    let centre = img.get(48, 27);
    assert_eq!(centre[3], 0.0, "centre must stay unfilled: {centre:?}");
    assert_eq!(img.get(0, 0), [1.0, 1.0, 1.0, 1.0], "outline");
}

/// A lower layer whose Advanced Blending leaves its defaults partway through the echo window needs
/// the compositor's styled path from then on, so the sample at that time is unsafe. Echo Minimum
/// must pass its input through — not take the minimum with a transparent stand-in for the declined
/// sample, which would clear the result.
#[test]
fn echo_on_an_adjustment_layer_declines_when_a_sample_turns_unsafe() {
    let mut p = Project::default();
    p.settings.bit_depth = effectcraft_project::BitDepth::Bpc32;
    let comp = Comp::new(96, 54, FrameRate::FPS_30, Tick::from_seconds_f64(2.0));
    let cid = p.add_item("Comp", Label::Sandstone, None, ItemKind::Comp(comp.clone().into()));
    let sid = p.add_item("Solid", Label::Red, None, ItemKind::Solid(Solid { color: [1.0, 0.0, 0.0], width: 96, height: 54, pixel_aspect: 1.0 }));
    let mut under = build::layer(&mut p, &comp, "Under", LayerSource::Solid { item: sid }, (96, 54), None);
    // A disabled style creates the Blending Options tree; the fill opacity alone flips the layer
    // from plain content to the styled path between the echo samples.
    let mut next = p.next_id;
    st::add_style(&mut under, &mut Ids(&mut next), "dropShadow", &comp.global_light, false).unwrap();
    p.next_id = next;
    let fill = under.props.prop_mut(&format!("{}/blendingOptions/advancedBlending/fillOpacity", st::GROUP)).unwrap();
    fill.keys = vec![Keyframe::new(Tick::from_seconds_f64(0.49), Value::Scalar(100.0)), Keyframe::new(Tick::from_seconds_f64(0.5), Value::Scalar(50.0))];
    let aid = p.add_item("AdjSolid", Label::Red, None, ItemKind::Solid(Solid { color: [1.0, 1.0, 1.0], width: 96, height: 54, pixel_aspect: 1.0 }));
    let mut adj = build::layer(&mut p, &comp, "Adj", LayerSource::Solid { item: aid }, (96, 54), None);
    adj.switches.adjustment = true;
    let spec = effectcraft_effects::find("ec.time.echo").unwrap();
    let mut next = p.next_id;
    let mut g = effectcraft_effects::instantiate(spec, &mut Ids(&mut next), spec.name, [96.0, 54.0]);
    g.prop_mut("echoTime").unwrap().value = Value::Scalar(-0.1);
    g.prop_mut("numberOfEchoes").unwrap().value = Value::Scalar(2.0);
    g.prop_mut("echoOperator").unwrap().value = Value::Enum(2); // Minimum
    p.next_id = next;
    adj.props.sub_mut("effects").unwrap().children.push(g.into());
    p.comp_mut(cid).unwrap().layers = vec![adj, under].into();
    let r = crate::Renderer::new(&p, &crate::NoFootage, crate::RenderOpts::default());
    let img = r.comp_frame(cid, Tick::from_seconds_f64(0.55));
    // Samples 0.35 s and 0.45 s are plain; 0.55 s needs the styled path. The whole effect declines
    // and the input (red at 50 % fill) passes through instead of being cleared by the Minimum.
    let c = img.get(48, 27);
    assert!(c[0] > 0.4 && c[3] > 0.4, "input must pass through: {c:?}");
}

/// #4001: a Posterize Time adjustment inside a same-size collapsed precomp runs on the parent
/// canvas. `below_at` only sees the precomp's own layers (none below the adjustment here), so it
/// cannot reconstruct the inherited composite and must decline: the time effect passes the parent
/// background through instead of holding transparency and erasing it.
#[test]
fn posterize_time_on_an_adjustment_layer_in_a_collapsed_precomp_keeps_the_inherited_comp() {
    let mut p = Project::default();
    p.settings.bit_depth = effectcraft_project::BitDepth::Bpc32;
    // Inner comp: only a full-frame Posterize Time adjustment.
    let inner = Comp::new(96, 54, FrameRate::FPS_30, Tick::from_seconds_f64(2.0));
    let iid = p.add_item("Inner", Label::Sandstone, None, ItemKind::Comp(inner.clone().into()));
    let aid = p.add_item("AdjSolid", Label::Red, None, ItemKind::Solid(Solid { color: [1.0, 1.0, 1.0], width: 96, height: 54, pixel_aspect: 1.0 }));
    let mut adj = build::layer(&mut p, &inner, "Adj", LayerSource::Solid { item: aid }, (96, 54), None);
    adj.switches.adjustment = true;
    let spec = effectcraft_effects::find("ec.time.posterizetime").unwrap();
    let mut next = p.next_id;
    let mut g = effectcraft_effects::instantiate(spec, &mut Ids(&mut next), spec.name, [96.0, 54.0]);
    g.prop_mut("frameRate").unwrap().value = Value::Scalar(4.0);
    p.next_id = next;
    adj.props.sub_mut("effects").unwrap().children.push(g.into());
    p.comp_mut(iid).unwrap().layers = vec![adj].into();
    // Outer comp: a red background beneath the same-size collapsed precomp (Normal blend, no matte).
    let outer = Comp::new(96, 54, FrameRate::FPS_30, Tick::from_seconds_f64(2.0));
    let cid = p.add_item("Outer", Label::Sandstone, None, ItemKind::Comp(outer.clone().into()));
    let rid = p.add_item("Red", Label::Red, None, ItemKind::Solid(Solid { color: [1.0, 0.0, 0.0], width: 96, height: 54, pixel_aspect: 1.0 }));
    let red = build::layer(&mut p, &outer, "Red", LayerSource::Solid { item: rid }, (96, 54), None);
    let mut pre = build::layer(&mut p, &outer, "Inner", LayerSource::Comp { item: iid }, (96, 54), None);
    pre.switches.collapse = true;
    p.comp_mut(cid).unwrap().layers = vec![pre, red].into();
    let r = crate::Renderer::new(&p, &crate::NoFootage, crate::RenderOpts::default());
    let img = r.comp_frame(cid, Tick::from_seconds_f64(0.3));
    // The inherited red background must survive: holding transparency here would erase it.
    assert_eq!(img.get(48, 27), [1.0, 0.0, 0.0, 1.0], "inherited background erased");
}
