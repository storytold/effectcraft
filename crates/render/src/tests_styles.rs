//! Layer Styles pixel tests.

use effectcraft_color::Label;
use effectcraft_keyframe::{Gradient, Keyframe, ShapePath, Value};
use effectcraft_project::build::{self, Ids};
use effectcraft_project::styles::{self as st, GROUP};
use effectcraft_project::{Comp, ItemId, ItemKind, Layer, LayerSource, MaskMode, Project, Solid};
use effectcraft_time::{FrameRate, Tick};

use crate::{Image, NoFootage, RenderOpts, Renderer, render_frame};

const NORMAL: u32 = 0;
const MULTIPLY: u32 = 3;

fn setup() -> (Project, ItemId, Comp) {
    let mut p = Project::default();
    let comp = Comp::new(200, 100, FrameRate::FPS_30, Tick::from_seconds_f64(2.0));
    let cid = p.add_item("Comp", Label::Sandstone, None, ItemKind::Comp(comp.clone().into()));
    (p, cid, comp)
}

fn solid(p: &mut Project, comp: &Comp, color: [f32; 3], w: u32, h: u32) -> Layer {
    let sid = p.add_item("Solid", Label::Red, None, ItemKind::Solid(Solid { color, width: w, height: h, pixel_aspect: 1.0 }));
    build::layer(p, comp, "Solid", LayerSource::Solid { item: sid }, (w, h), None)
}

fn add(p: &mut Project, comp: &Comp, l: &mut Layer, style: &str) {
    let mut next = p.next_id;
    st::add_style(l, &mut Ids(&mut next), style, &comp.global_light, true).unwrap();
    p.next_id = next;
}

fn set(l: &mut Layer, path: &str, v: Value) {
    l.props.prop_mut(&format!("{GROUP}/{path}")).unwrap_or_else(|| panic!("no {path}")).value = v;
}

/// A 40×40 square of `color` centred in a 200×100 comp (x 80..120, y 30..70) over an optional
/// full-frame background.
fn scene(color: [f32; 3], bg: Option<[f32; 3]>, f: impl FnOnce(&mut Project, &Comp, &mut Layer)) -> (Project, ItemId) {
    let (mut p, cid, comp) = setup();
    let mut l = solid(&mut p, &comp, color, 40, 40);
    f(&mut p, &comp, &mut l);
    let mut layers = vec![l];
    if let Some(bg) = bg {
        layers.push(solid(&mut p, &comp, bg, 200, 100));
    }
    p.comp_mut(cid).unwrap().layers = layers;
    (p, cid)
}

fn render(p: &Project, cid: ItemId) -> Image {
    render_frame(p, cid, Tick::ZERO, 1.0)
}

fn near(a: f32, b: f32, tol: f32) -> bool {
    (a - b).abs() <= tol
}

#[test]
fn drop_shadow_offset_colour_and_opacity() {
    let (p, cid) = scene([1.0; 3], Some([0.5; 3]), |p, c, l| {
        add(p, c, l, "dropShadow");
        set(l, "dropShadow/size", Value::Scalar(0.0));
        set(l, "dropShadow/distance", Value::Scalar(10.0));
        set(l, "dropShadow/opacity", Value::Scalar(100.0));
        set(l, "dropShadow/blendMode", Value::Enum(NORMAL));
    });
    let img = render(&p, cid);
    // Global light 120° → the shadow falls down-right by (5, 8.66).
    assert!(near(img.get(123, 75)[0], 0.0, 0.02), "shadow: {:?}", img.get(123, 75));
    assert!(near(img.get(82, 75)[0], 0.5, 0.01), "left of the shadow stays grey");
    assert!(near(img.get(100, 50)[0], 1.0, 0.01), "layer on top");
    assert!(near(img.get(100, 20)[0], 0.5, 0.01), "above the layer");

    // 75 % Multiply (the default mode) over grey: 0.5 × (1 − 0.75).
    let (p, cid) = scene([1.0; 3], Some([0.5; 3]), |p, c, l| {
        add(p, c, l, "dropShadow");
        set(l, "dropShadow/size", Value::Scalar(0.0));
        set(l, "dropShadow/distance", Value::Scalar(10.0));
        set(l, "dropShadow/color", Value::Color([0.0, 0.0, 0.0, 1.0]));
    });
    let img = render(&p, cid);
    assert!(near(img.get(123, 75)[0], 0.125, 0.02), "{:?}", img.get(123, 75));

    // Coloured shadow on a transparent comp extends the layer's bounds.
    let (p, cid) = scene([1.0; 3], None, |p, c, l| {
        add(p, c, l, "dropShadow");
        set(l, "dropShadow/size", Value::Scalar(0.0));
        set(l, "dropShadow/distance", Value::Scalar(10.0));
        set(l, "dropShadow/opacity", Value::Scalar(50.0));
        set(l, "dropShadow/blendMode", Value::Enum(NORMAL));
        set(l, "dropShadow/color", Value::Color([0.0, 0.0, 1.0, 1.0]));
    });
    let img = render(&p, cid);
    let px = img.straight(123, 75);
    assert!(near(px[3], 0.5, 0.02) && near(px[2], 1.0, 0.01) && px[0] < 0.01, "{px:?}");
}

#[test]
fn soft_shadow_falls_off() {
    let (p, cid) = scene([1.0; 3], None, |p, c, l| {
        add(p, c, l, "dropShadow");
        set(l, "dropShadow/size", Value::Scalar(10.0));
        set(l, "dropShadow/distance", Value::Scalar(0.0));
        set(l, "dropShadow/opacity", Value::Scalar(100.0));
    });
    let img = render(&p, cid);
    let a1 = img.get(121, 50)[3];
    let a5 = img.get(125, 50)[3];
    let a20 = img.get(140, 50)[3];
    assert!(a1 > a5 && a5 > a20, "{a1} {a5} {a20}");
    assert!(a20 < 0.01);
}

#[test]
fn stroke_positions() {
    let render_stroke = |pos: u32| {
        let (p, cid) = scene([1.0; 3], None, |p, c, l| {
            add(p, c, l, "stroke");
            set(l, "stroke/size", Value::Scalar(4.0));
            set(l, "stroke/position", Value::Enum(pos));
        });
        render(&p, cid)
    };
    let red = |px: [f32; 4]| px[0] > 0.98 && px[1] < 0.02 && px[3] > 0.98;
    let white = |px: [f32; 4]| px[0] > 0.98 && px[1] > 0.98 && px[3] > 0.98;
    // Outside: 4 px ring beyond the right edge (x = 120).
    let o = render_stroke(0);
    assert!(red(o.get(122, 50)) && red(o.get(123, 50)), "{:?}", o.get(122, 50));
    assert!(o.get(126, 50)[3] < 0.01);
    assert!(white(o.get(118, 50)));
    // Inside: the ring is within the layer.
    let i = render_stroke(1);
    assert!(red(i.get(118, 50)) && red(i.get(117, 50)));
    assert!(white(i.get(110, 50)));
    assert!(i.get(122, 50)[3] < 0.01);
    // Centre: 2 px on each side.
    let c = render_stroke(2);
    assert!(red(c.get(119, 50)) && red(c.get(120, 50)) && red(c.get(121, 50)));
    assert!(white(c.get(116, 50)));
    assert!(c.get(124, 50)[3] < 0.01);
}

#[test]
fn colour_overlay_blend_modes() {
    let (p, cid) = scene([1.0; 3], None, |p, c, l| {
        add(p, c, l, "colorOverlay");
        set(l, "colorOverlay/color", Value::Color([0.0, 0.0, 1.0, 1.0]));
        set(l, "colorOverlay/opacity", Value::Scalar(50.0));
    });
    let px = render(&p, cid).get(100, 50);
    assert!(near(px[0], 0.5, 0.01) && near(px[2], 1.0, 0.01), "{px:?}");

    let (p, cid) = scene([0.5, 1.0, 1.0], None, |p, c, l| {
        add(p, c, l, "colorOverlay");
        set(l, "colorOverlay/color", Value::Color([1.0, 0.5, 0.0, 1.0]));
        set(l, "colorOverlay/blendMode", Value::Enum(MULTIPLY));
    });
    let img = render(&p, cid);
    let px = img.get(100, 50);
    assert!(near(px[0], 0.5, 0.01) && near(px[1], 0.5, 0.01) && near(px[2], 0.0, 0.01), "{px:?}");
    // Overlays stay inside the layer.
    assert!(img.get(130, 50)[3] < 0.01);
}

#[test]
fn gradient_overlay_follows_angle() {
    let (p, cid) = scene([1.0; 3], None, |p, c, l| {
        add(p, c, l, "gradientOverlay");
    });
    let img = render(&p, cid);
    // 90°: black at the bottom, white at the top.
    let top = img.get(100, 31)[0];
    let bottom = img.get(100, 68)[0];
    assert!(top > 0.9 && bottom < 0.1, "{top} {bottom}");
    let (p, cid) = scene([1.0; 3], None, |p, c, l| {
        add(p, c, l, "gradientOverlay");
        set(l, "gradientOverlay/angle", Value::Scalar(0.0));
        set(l, "gradientOverlay/reverse", Value::Bool(true));
    });
    let img = render(&p, cid);
    assert!(img.get(81, 50)[0] > 0.9 && img.get(118, 50)[0] < 0.1);
}

/// Overlays replace the layer's colour inside its alpha: an anti-aliased edge pixel mixes the
/// background with the overlay colour only, exactly as a layer of that colour would, with no
/// fringe of the layer's own colour (#416).
#[test]
fn overlays_leave_antialiased_edges_clean() {
    let (blue, bg) = ([0.2, 0.533, 1.0], [0.25, 0.22, 0.45]);
    // A mask half a pixel inside the square's left and right sides: in layer space, where the
    // styles run, columns 80 and 119 are half covered.
    let edge = |color: [f32; 3], f: &dyn Fn(&mut Project, &Comp, &mut Layer)| {
        let (p, cid) = scene(color, Some(bg), |p, c, l| {
            let mut next = p.next_id;
            let pts = vec![[0.5, 0.0], [39.5, 0.0], [39.5, 40.0], [0.5, 40.0]];
            let sp = ShapePath { in_tangents: vec![[0.0; 2]; 4], out_tangents: vec![[0.0; 2]; 4], vertices: pts, closed: true, feather: Vec::new() };
            let m = build::mask(&mut Ids(&mut next), "Mask 1", sp, MaskMode::Add, [255, 255, 0]);
            p.next_id = next;
            l.props.group_mut("masks").unwrap().children.push(m.into());
            f(p, c, l);
        });
        let img = render(&p, cid);
        [img.get(80, 50), img.get(119, 50), img.get(100, 50)]
    };
    let plain = edge(blue, &|_, _, _| {});
    assert!(near(plain[0][0], 0.5 * (blue[0] + bg[0]), 0.01), "half covered: {:?}", plain[0]);
    let color = Value::Color([blue[0] as f64, blue[1] as f64, blue[2] as f64, 1.0]);
    let flat =
        Gradient { colors: vec![(0.0, [blue[0], blue[1], blue[2], 1.0]), (1.0, [blue[0], blue[1], blue[2], 1.0])], opacities: vec![(0.0, 1.0), (1.0, 1.0)] };
    let styled: [(&str, Box<dyn Fn(&mut Project, &Comp, &mut Layer)>); 3] = [
        (
            "Color Overlay",
            Box::new(|p, c, l| {
                add(p, c, l, "colorOverlay");
                set(l, "colorOverlay/color", color.clone());
            }),
        ),
        (
            "Gradient Overlay",
            Box::new(|p, c, l| {
                add(p, c, l, "gradientOverlay");
                set(l, "gradientOverlay/colors", Value::Gradient(flat.clone()));
            }),
        ),
        (
            "Stroke (inside)",
            Box::new(|p, c, l| {
                add(p, c, l, "stroke");
                set(l, "stroke/position", Value::Enum(1));
                set(l, "stroke/size", Value::Scalar(25.0));
                set(l, "stroke/color", color.clone());
            }),
        ),
    ];
    for (name, f) in &styled {
        let px = edge([1.0; 3], f.as_ref());
        for (a, b) in px.iter().zip(&plain) {
            for ch in 0..4 {
                assert!(near(a[ch], b[ch], 0.01), "{name}: {a:?} against the plain layer {b:?}");
            }
        }
    }
}

fn bevel_scene(angle: f64) -> Image {
    let (p, cid) = scene([0.5; 3], None, |p, c, l| {
        add(p, c, l, "bevelEmboss");
        set(l, "bevelEmboss/size", Value::Scalar(6.0));
        set(l, "blendingOptions/globalLightAngle", Value::Scalar(angle));
    });
    render(&p, cid)
}

#[test]
fn bevel_lights_the_side_facing_the_light() {
    // Light from the upper left (120°): left and top edges brighten, right and bottom darken.
    let img = bevel_scene(120.0);
    let (l, r, t, b, mid) = (img.get(82, 50)[0], img.get(117, 50)[0], img.get(100, 32)[0], img.get(100, 67)[0], img.get(100, 50)[0]);
    assert!(l > 0.6 && t > 0.6, "highlights {l} {t}");
    assert!(r < 0.4 && b < 0.4, "shadows {r} {b}");
    assert!(near(mid, 0.5, 0.02), "flat centre {mid}");
    // Light from the lower right (−60°) swaps them.
    let img = bevel_scene(-60.0);
    assert!(img.get(82, 50)[0] < 0.4 && img.get(117, 50)[0] > 0.6);
}

#[test]
fn global_light_drives_linked_styles() {
    let shadow_at = |use_global: bool| {
        let (p, cid) = scene([1.0; 3], None, |p, c, l| {
            add(p, c, l, "dropShadow");
            set(l, "dropShadow/size", Value::Scalar(0.0));
            set(l, "dropShadow/distance", Value::Scalar(10.0));
            set(l, "dropShadow/opacity", Value::Scalar(100.0));
            set(l, "dropShadow/useGlobalLight", Value::Bool(use_global));
            set(l, "dropShadow/angle", Value::Scalar(90.0));
            set(l, "blendingOptions/globalLightAngle", Value::Scalar(0.0));
        });
        render(&p, cid)
    };
    // Global light at 0° (from the right): shadow to the left.
    let g = shadow_at(true);
    assert!(g.get(75, 50)[3] > 0.98 && g.get(100, 75)[3] < 0.01);
    // Own angle 90° (from above): shadow below.
    let o = shadow_at(false);
    assert!(o.get(100, 75)[3] > 0.98 && o.get(75, 50)[3] < 0.01);
}

#[test]
fn glows_and_inner_shadow() {
    let (p, cid) = scene([0.0, 0.0, 0.0], None, |p, c, l| {
        add(p, c, l, "outerGlow");
        add(p, c, l, "innerGlow");
        set(l, "outerGlow/opacity", Value::Scalar(100.0));
        set(l, "outerGlow/size", Value::Scalar(8.0));
        set(l, "innerGlow/opacity", Value::Scalar(100.0));
        set(l, "innerGlow/size", Value::Scalar(8.0));
    });
    let img = render(&p, cid);
    let out_near = img.straight(121, 50);
    assert!(out_near[3] > 0.6 && out_near[0] > 0.9 && out_near[2] < 0.9, "glow colour {out_near:?}");
    assert!(img.get(140, 50)[3] < 0.02, "glow fades");
    let edge = img.get(119, 50);
    let centre = img.get(100, 50);
    assert!(edge[0] > 0.6 && centre[0] < 0.05, "inner glow from the edge {edge:?} {centre:?}");

    // Inner glow from the centre instead.
    let (p, cid) = scene([0.0; 3], None, |p, c, l| {
        add(p, c, l, "innerGlow");
        set(l, "innerGlow/source", Value::Enum(0));
        set(l, "innerGlow/opacity", Value::Scalar(100.0));
    });
    let img = render(&p, cid);
    assert!(img.get(100, 50)[0] > 0.9 && img.get(119, 50)[0] < 0.6);

    let (p, cid) = scene([1.0; 3], None, |p, c, l| {
        add(p, c, l, "innerShadow");
        set(l, "innerShadow/size", Value::Scalar(0.0));
        set(l, "innerShadow/opacity", Value::Scalar(100.0));
        set(l, "innerShadow/blendMode", Value::Enum(NORMAL));
    });
    let img = render(&p, cid);
    // Light from the upper left: the shadow lines the top-left inside edges.
    assert!(img.get(81, 50)[0] < 0.05 && img.get(100, 32)[0] < 0.05, "{:?}", img.get(81, 50));
    assert!(img.get(117, 50)[0] > 0.95 && img.get(100, 66)[0] > 0.95);
    assert!(img.get(76, 50)[3] < 0.01, "inner shadow stays inside");
}

#[test]
fn satin_shades_inside() {
    let (p, cid) = scene([1.0; 3], None, |p, c, l| add(p, c, l, "satin"));
    let img = render(&p, cid);
    let vals: Vec<f32> = (82..118).map(|x| img.get(x, 50)[0]).collect();
    let min = vals.iter().cloned().fold(1.0, f32::min);
    assert!(min < 0.9, "satin darkens somewhere: {min}");
    assert!(img.get(130, 50)[3] < 0.01);
}

#[test]
fn fill_opacity_knockout_and_channels() {
    // Fill 0: the content disappears but the stroke stays.
    let (p, cid) = scene([1.0; 3], None, |p, c, l| {
        add(p, c, l, "stroke");
        set(l, "blendingOptions/advancedBlending/fillOpacity", Value::Scalar(0.0));
    });
    let img = render(&p, cid);
    assert!(img.get(100, 50)[3] < 0.01 && img.get(122, 50)[0] > 0.98);

    // Interior styles ignore Fill Opacity unless blended as a group.
    let overlay = |group: bool| {
        let (p, cid) = scene([1.0; 3], None, |p, c, l| {
            add(p, c, l, "colorOverlay");
            set(l, "blendingOptions/advancedBlending/fillOpacity", Value::Scalar(0.0));
            set(l, "blendingOptions/advancedBlending/blendInteriorAsGroup", Value::Bool(group));
        });
        render(&p, cid).get(100, 50)
    };
    assert!(overlay(false)[3] > 0.98 && overlay(true)[3] < 0.01);

    // Knockout with Fill 0 punches through the background.
    let (p, cid) = scene([1.0; 3], Some([0.5; 3]), |p, c, l| {
        add(p, c, l, "stroke");
        set(l, "blendingOptions/advancedBlending/fillOpacity", Value::Scalar(0.0));
        set(l, "blendingOptions/advancedBlending/knockout", Value::Enum(2));
    });
    let img = render(&p, cid);
    assert!(img.get(100, 50)[3] < 0.01 && img.get(150, 50)[3] > 0.99);

    // Red channel off: the white layer leaves red from below.
    let (p, cid) = scene([1.0; 3], Some([0.0; 3]), |p, c, l| {
        add(p, c, l, "stroke");
        set(l, "blendingOptions/advancedBlending/red", Value::Bool(false));
    });
    let px = render(&p, cid).get(100, 50);
    assert!(px[0] < 0.01 && px[1] > 0.99 && px[2] > 0.99, "{px:?}");
}

#[test]
fn eye_switch_and_group_switch() {
    let (mut p, cid) = scene([1.0; 3], None, |p, c, l| {
        add(p, c, l, "stroke");
    });
    assert!(render(&p, cid).get(122, 50)[3] > 0.98);
    p.comp_mut(cid).unwrap().layers[0].props.group_mut(&format!("{GROUP}/stroke")).unwrap().enabled = false;
    assert!(render(&p, cid).get(122, 50)[3] < 0.01);
    p.comp_mut(cid).unwrap().layers[0].props.group_mut(&format!("{GROUP}/stroke")).unwrap().enabled = true;
    p.comp_mut(cid).unwrap().layers[0].props.group_mut(GROUP).unwrap().enabled = false;
    assert!(render(&p, cid).get(122, 50)[3] < 0.01);
}

#[test]
fn styles_respect_render_scale() {
    let (p, cid) = scene([1.0; 3], None, |p, c, l| {
        add(p, c, l, "dropShadow");
        add(p, c, l, "stroke");
        set(l, "dropShadow/size", Value::Scalar(0.0));
        set(l, "dropShadow/distance", Value::Scalar(20.0));
        set(l, "dropShadow/opacity", Value::Scalar(100.0));
        set(l, "stroke/size", Value::Scalar(4.0));
    });
    let full = render_frame(&p, cid, Tick::ZERO, 1.0);
    let half = render_frame(&p, cid, Tick::ZERO, 0.5);
    // Shadow offset (10, 17.3): at half resolution the shadow sits at half the distance.
    assert!(full.get(125, 85)[3] > 0.98);
    assert!(half.get(62, 42)[3] > 0.98 && half.get(55, 25)[3] > 0.98);
    // Stroke 4 px → 2 px at half.
    assert!(half.get(61, 25)[0] > 0.98 && half.get(61, 25)[1] < 0.05);
    assert!(half.get(64, 25)[3] < 0.01 || half.get(64, 25)[1] < 0.05);
}

#[test]
fn animated_style_properties() {
    let (mut p, cid) = scene([1.0; 3], None, |p, c, l| add(p, c, l, "stroke"));
    let l = &mut p.comp_mut(cid).unwrap().layers[0];
    l.props.prop_mut(&format!("{GROUP}/stroke/size")).unwrap().keys =
        vec![Keyframe::new(Tick::ZERO, Value::Scalar(2.0)), Keyframe::new(Tick::from_seconds_f64(1.0), Value::Scalar(10.0))];
    let a = render_frame(&p, cid, Tick::ZERO, 1.0);
    let b = render_frame(&p, cid, Tick::from_seconds_f64(1.0), 1.0);
    assert!(a.get(126, 50)[3] < 0.01 && b.get(126, 50)[3] > 0.98);
}

fn render_cached(p: &Project, cid: ItemId, cache: &crate::LayerCache) -> Image {
    let mut r = Renderer::new(p, &NoFootage, RenderOpts::default());
    r.cache = Some(cache);
    r.comp_frame(cid, Tick::ZERO)
}

#[test]
fn style_edits_invalidate_the_cache() {
    let (mut p, cid) = scene([1.0; 3], Some([0.5; 3]), |p, c, l| {
        add(p, c, l, "dropShadow");
        add(p, c, l, "stroke");
    });
    let cache = crate::LayerCache::default();
    let a = render_cached(&p, cid, &cache);
    assert_eq!(a.data, render(&p, cid).data);
    let again = render_cached(&p, cid, &cache);
    assert_eq!(a.data, again.data);
    // Change a style value: the styled layer re-renders, its content (and the background) come
    // from the cache.
    let hits0 = cache.stats().hits;
    let l = &mut p.comp_mut(cid).unwrap().layers[0];
    l.props.prop_mut(&format!("{GROUP}/stroke/color")).unwrap().value = Value::Color([0.0, 1.0, 0.0, 1.0]);
    let b = render_cached(&p, cid, &cache);
    assert_eq!(b.data, render(&p, cid).data);
    assert_ne!(a.data, b.data);
    assert!(cache.stats().hits >= hits0 + 2, "content + background reused");
    // Global light change (linked) also invalidates.
    let l = &mut p.comp_mut(cid).unwrap().layers[0];
    l.props.prop_mut(&format!("{GROUP}/blendingOptions/globalLightAngle")).unwrap().value = Value::Scalar(0.0);
    let c = render_cached(&p, cid, &cache);
    assert_eq!(c.data, render(&p, cid).data);
    assert_ne!(b.data, c.data);
    // Eye switch off.
    p.comp_mut(cid).unwrap().layers[0].props.group_mut(&format!("{GROUP}/dropShadow")).unwrap().enabled = false;
    let d = render_cached(&p, cid, &cache);
    assert_eq!(d.data, render(&p, cid).data);
    assert_ne!(c.data, d.data);
}

#[test]
fn gradient_glow_uses_gradient() {
    let (p, cid) = scene([0.0; 3], None, |p, c, l| {
        add(p, c, l, "outerGlow");
        set(l, "outerGlow/colorType", Value::Enum(1));
        set(l, "outerGlow/opacity", Value::Scalar(100.0));
        set(
            l,
            "outerGlow/colors",
            Value::Gradient(Gradient { colors: vec![(0.0, [1.0, 0.0, 0.0, 1.0]), (1.0, [0.0, 0.0, 1.0, 1.0])], opacities: vec![(0.0, 1.0), (1.0, 0.0)] }),
        );
    });
    let img = render(&p, cid);
    let px = img.straight(121, 50);
    assert!(px[0] > px[2], "near the edge the glow takes the gradient's start colour {px:?}");
}

/// #1812 (review of #1928): a styled layer rendered by several workers at once shares its body and
/// style passes, as unstyled layers share their buffer.
#[test]
fn workers_styling_a_layer_at_once_share_its_buffers() {
    let (p, cid) = scene([1.0; 3], None, |p, c, l| {
        add(p, c, l, "dropShadow");
        add(p, c, l, "stroke");
    });
    let cache = crate::LayerCache::default();
    let start = std::sync::Barrier::new(8);
    let styled: Vec<_> = std::thread::scope(|s| {
        let hs: Vec<_> = (0..8)
            .map(|_| {
                s.spawn(|| {
                    let mut r = Renderer::new(&p, &NoFootage, RenderOpts::default());
                    r.cache = Some(&cache);
                    let ctx = r.eval_ctx(cid, Tick::ZERO).unwrap();
                    start.wait();
                    r.styled_layer(&ctx, &p.comp(cid).unwrap().layers[0]).unwrap()
                })
            })
            .collect();
        hs.into_iter().map(|h| h.join().unwrap()).collect()
    });
    let first = &styled[0];
    assert!(!first.passes.is_empty(), "the drop shadow is a pass");
    for st in &styled {
        assert!(std::sync::Arc::ptr_eq(&st.body, &first.body), "one body");
        assert!(st.passes.iter().zip(&first.passes).all(|(a, b)| std::sync::Arc::ptr_eq(&a.0, &b.0)), "one set of passes");
    }
}
