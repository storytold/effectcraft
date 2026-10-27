//! The demo project: an animated title sequence built entirely from procedural content (original
//! work, MIT OR Apache-2.0): gradient background, orbiting shape rings with trim paths and a
//! repeater burst, an animated title with a text animator and glow, a lower-third precomp; and a
//! "3D Showcase" comp: a two-node camera move with depth of field over lit, shadow-casting 3D
//! cards, intersecting planes and a gridded floor.

use effectcraft_color::BlendMode;
use effectcraft_color::Label;
use effectcraft_keyframe::{Ease, Gradient, Interp, Justify, Keyframe, TextDoc, Value};
use effectcraft_project::build::{self, Ids};
use effectcraft_project::{Comp, ItemId, ItemKind, Layer, LayerSource, Project, PropGroup, Solid};
use effectcraft_time::{FrameRate, Tick};

pub const MAIN_COMP: &str = "EffectCraft Intro";
pub const SHOWCASE_3D: &str = "3D Showcase";

/// Seconds → the nearest frame of the demo's 29.97 fps comps (AE keeps times frame-aligned).
fn t(s: f64) -> Tick {
    FrameRate::FPS_29_97.snap_nearest(Tick::from_seconds_f64(s))
}

fn hex(h: &str) -> [f64; 4] {
    let c = effectcraft_color::Rgba::from_hex(h).unwrap_or(effectcraft_color::Rgba::WHITE);
    [c.r as f64, c.g as f64, c.b as f64, 1.0]
}

/// Eased keyframes (Easy Ease on every key).
fn keys(list: &[(f64, Value)]) -> Vec<Keyframe> {
    list.iter().map(|(s, v)| Keyframe::new(t(*s), v.clone()).eased()).collect()
}

/// Keyframes with a strong "expo out" ease: fast start, long settle.
fn keys_out(list: &[(f64, Value)]) -> Vec<Keyframe> {
    let mut k: Vec<Keyframe> = list.iter().map(|(s, v)| Keyframe::new(t(*s), v.clone())).collect();
    let n = k.len();
    for (i, key) in k.iter_mut().enumerate() {
        let d = key.value.dims().max(1);
        if i + 1 < n {
            key.out_interp = Interp::Bezier;
            key.out_ease = vec![Ease { speed: 0.0, influence: 0.05 }; d];
        }
        if i > 0 {
            key.in_interp = Interp::Bezier;
            key.in_ease = vec![Ease { speed: 0.0, influence: 0.85 }; d];
        }
    }
    k
}

fn set(l: &mut Layer, path: &str, v: Value) {
    if let Some(p) = l.props.prop_mut(path) {
        p.value = v;
    }
}
fn anim(l: &mut Layer, path: &str, k: Vec<Keyframe>) {
    if let Some(p) = l.props.prop_mut(path) {
        p.keys = k;
    }
}

fn effect(p: &mut Project, l: &mut Layer, id: &str, size: [f64; 2], params: &[(&str, Value)]) {
    let Some(spec) = effectcraft_effects::find(id) else { return };
    let mut next = p.next_id;
    let mut g = effectcraft_effects::instantiate(spec, &mut Ids(&mut next), spec.name, size);
    p.next_id = next;
    for (k, v) in params {
        if let Some(pr) = g.get_mut(k) {
            pr.value = v.clone();
        }
    }
    if let Some(fx) = l.props.sub_mut("effects") {
        fx.children.push(g.into());
    }
}

fn contents(l: &mut Layer, items: Vec<PropGroup>) {
    if let Some(c) = l.props.sub_mut("contents") {
        for g in items {
            c.children.push(g.into());
        }
    }
}

fn text_layer(p: &mut Project, comp: &Comp, name: &str, doc: TextDoc, pos: [f64; 2]) -> Layer {
    let mut l = build::layer(p, comp, name, LayerSource::Text, (comp.width, comp.height), None);
    set(&mut l, "text/sourceText", Value::Text(Box::new(doc)));
    set(&mut l, "transform/position", Value::Vec3([pos[0], pos[1], 0.0]));
    l
}

fn solid_layer(p: &mut Project, comp: &Comp, folder: ItemId, name: &str, color: [f32; 3]) -> Layer {
    let sid = p.add_item(name, Label::Red, Some(folder), ItemKind::Solid(Solid { color, width: comp.width, height: comp.height, pixel_aspect: 1.0 }));
    build::layer(p, comp, name, LayerSource::Solid { item: sid }, (comp.width, comp.height), None)
}

fn lower_third(p: &mut Project) -> Comp {
    let mut c = Comp::new(1920, 1080, FrameRate::FPS_29_97, t(4.0));
    let mut bar = build::layer(p, &c, "Bar", LayerSource::Shape, (1920, 1080), None);
    let mut next = p.next_id;
    {
        let mut ids = Ids(&mut next);
        let rect = build::shape_rect(&mut ids, [560.0, 96.0], [0.0, 0.0], 14.0);
        let gfill = build::shape_gradient_fill(
            &mut ids,
            false,
            [-280.0, 0.0],
            [280.0, 0.0],
            Gradient { colors: vec![(0.0, [0.18, 0.55, 0.92, 1.0]), (1.0, [0.55, 0.32, 0.98, 1.0])], opacities: vec![(0.0, 1.0), (1.0, 1.0)] },
        );
        let g = build::shape_group(&mut ids, "Bar", vec![rect, gfill]);
        contents(&mut bar, vec![g]);
    }
    p.next_id = next;
    set(&mut bar, "transform/position", Value::Vec3([440.0, 900.0, 0.0]));
    anim(&mut bar, "transform/scale", keys_out(&[(0.0, Value::Vec3([0.0, 100.0, 100.0])), (0.7, Value::Vec3([100.0, 100.0, 100.0]))]));
    let mut title = text_layer(
        p,
        &c,
        "Built with EffectCraft",
        TextDoc { text: "Built with EffectCraft".into(), size: 40.0, style: "SemiBold".into(), justify: Justify::Center, ..Default::default() },
        [440.0, 914.0],
    );
    anim(&mut title, "transform/opacity", keys(&[(0.4, Value::Scalar(0.0)), (0.9, Value::Scalar(100.0))]));
    c.layers = vec![title, bar];
    c
}

fn solid_sized(p: &mut Project, comp: &Comp, folder: ItemId, name: &str, color: [f32; 3], w: u32, h: u32) -> Layer {
    let sid = p.add_item(name, Label::Red, Some(folder), ItemKind::Solid(Solid { color, width: w, height: h, pixel_aspect: 1.0 }));
    build::layer(p, comp, name, LayerSource::Solid { item: sid }, (w, h), None)
}

/// A 3D card: a solid with a diagonal gradient and a thin border grid.
fn card(p: &mut Project, comp: &Comp, folder: ItemId, name: &str, a: &str, b: &str, size: (u32, u32), pos: [f64; 3], rot_y: f64) -> Layer {
    let mut l = solid_sized(p, comp, folder, name, [1.0, 1.0, 1.0], size.0, size.1);
    let (w, h) = (size.0 as f64, size.1 as f64);
    effect(
        p,
        &mut l,
        "ec.generate.gradientramp",
        [w, h],
        &[("start", Value::Vec2([0.0, 0.0])), ("end", Value::Vec2([w, h])), ("startColor", Value::Color(hex(a))), ("endColor", Value::Color(hex(b)))],
    );
    effect(
        p,
        &mut l,
        "ec.generate.grid",
        [w, h],
        &[
            ("sizeFrom", Value::Enum(2)),
            ("width", Value::Scalar(w)),
            ("height", Value::Scalar(h)),
            ("border", Value::Scalar(10.0)),
            ("opacity", Value::Scalar(35.0)),
            // Normal: the grid over the gradient.
            ("blendingMode", Value::Enum(1)),
        ],
    );
    l.switches.three_d = true;
    set(&mut l, "transform/position", Value::Vec3(pos));
    set(&mut l, "transform/rotationY", Value::Scalar(rot_y));
    set(&mut l, "materialOptions/castsShadows", Value::Enum(1));
    set(&mut l, "materialOptions/specularIntensity", Value::Scalar(35.0));
    set(&mut l, "materialOptions/specularShininess", Value::Scalar(40.0));
    l
}

/// The "3D Showcase" comp: camera move, key + fill lights, shadows, DOF, intersecting planes.
fn showcase_3d(p: &mut Project, solids: ItemId) -> Comp {
    let mut c = Comp::new(1920, 1080, FrameRate::FPS_29_97, t(10.0));
    c.background = [0.02, 0.02, 0.035];
    // Floor: a gridded plane laid flat (X rotation 90°) below the cards.
    let mut floor = solid_sized(p, &c, solids, "Floor", [0.16, 0.17, 0.22], 1000, 1000);
    effect(
        p,
        &mut floor,
        "ec.generate.grid",
        [1000.0, 1000.0],
        &[
            ("width", Value::Scalar(100.0)),
            ("height", Value::Scalar(100.0)),
            ("border", Value::Scalar(3.0)),
            ("color", Value::Color(hex("#6F7BB8"))),
            ("opacity", Value::Scalar(45.0)),
            ("blendingMode", Value::Enum(1)),
        ],
    );
    floor.switches.three_d = true;
    set(&mut floor, "transform/position", Value::Vec3([960.0, 800.0, 500.0]));
    set(&mut floor, "transform/scale", Value::Vec3([400.0, 400.0, 100.0]));
    set(&mut floor, "transform/rotationX", Value::Scalar(90.0));
    set(&mut floor, "materialOptions/specularIntensity", Value::Scalar(10.0));

    // Cards at different depths and angles; the centre pair intersects.
    let left = card(p, &c, solids, "Card Blue", "#2E7BF0", "#7A4DFF", (520, 340), [520.0, 560.0, 120.0], 28.0);
    let right = card(p, &c, solids, "Card Coral", "#FF6A5C", "#FFB347", (520, 340), [1420.0, 560.0, 260.0], -32.0);
    let mut x1 = card(p, &c, solids, "Cross A", "#1FD1A5", "#2E7BF0", (460, 460), [960.0, 470.0, 520.0], 40.0);
    set(&mut x1, "materialOptions/lightTransmission", Value::Scalar(30.0));
    let x2 = card(p, &c, solids, "Cross B", "#F0E14A", "#FF6A5C", (460, 460), [960.0, 470.0, 520.0], -40.0);
    // Hero title in 3D space, gently turning.
    let mut title = text_layer(
        p,
        &c,
        "3D Title",
        TextDoc { text: "CLASSIC 3D".into(), size: 150.0, style: "Bold".into(), tracking: 60.0, justify: Justify::Center, ..Default::default() },
        [960.0, 560.0],
    );
    title.switches.three_d = true;
    set(&mut title, "transform/position", Value::Vec3([960.0, 600.0, -160.0]));
    anim(&mut title, "transform/rotationY", keys(&[(0.0, Value::Scalar(-18.0)), (10.0, Value::Scalar(18.0))]));
    set(&mut title, "materialOptions/castsShadows", Value::Enum(1));

    // Camera: two-node, 35 mm, sweeping around the set with depth of field on the title.
    let mut cam = build::layer(p, &c, "Camera 1", LayerSource::Camera, (1920, 1080), None);
    let zoom = 1920.0 * 35.0 / 36.0;
    set(&mut cam, "cameraOptions/zoom", Value::Scalar(zoom));
    set(&mut cam, "cameraOptions/dof", Value::Bool(true));
    set(&mut cam, "cameraOptions/aperture", Value::Scalar(40.0));
    set(&mut cam, "cameraOptions/blurLevel", Value::Scalar(100.0));
    set(&mut cam, "transform/poi", Value::Vec3([960.0, 560.0, 150.0]));
    anim(
        &mut cam,
        "transform/position",
        keys(&[(0.0, Value::Vec3([-200.0, 160.0, -1500.0])), (5.0, Value::Vec3([1100.0, 80.0, -1850.0])), (10.0, Value::Vec3([2150.0, 220.0, -1300.0]))]),
    );
    anim(&mut cam, "cameraOptions/focusDistance", keys(&[(0.0, Value::Scalar(1700.0)), (5.0, Value::Scalar(1900.0)), (10.0, Value::Scalar(1650.0))]));

    // Key light: warm spot casting soft shadows. Fill: cool ambient.
    let mut key = build::layer(p, &c, "Key Light", LayerSource::Light { kind: effectcraft_project::LightKind::Spot }, (1920, 1080), None);
    set(&mut key, "transform/position", Value::Vec3([500.0, -500.0, -700.0]));
    set(&mut key, "transform/poi", Value::Vec3([960.0, 600.0, 300.0]));
    set(&mut key, "lightOptions/intensity", Value::Scalar(150.0));
    set(&mut key, "lightOptions/color", Value::Color(hex("#FFE6C7")));
    set(&mut key, "lightOptions/coneAngle", Value::Scalar(110.0));
    set(&mut key, "lightOptions/coneFeather", Value::Scalar(60.0));
    set(&mut key, "lightOptions/castsShadows", Value::Bool(true));
    set(&mut key, "lightOptions/shadowDarkness", Value::Scalar(70.0));
    set(&mut key, "lightOptions/shadowDiffusion", Value::Scalar(12.0));
    let mut fill = build::layer(p, &c, "Fill Light", LayerSource::Light { kind: effectcraft_project::LightKind::Ambient }, (1920, 1080), None);
    set(&mut fill, "lightOptions/intensity", Value::Scalar(35.0));
    set(&mut fill, "lightOptions/color", Value::Color(hex("#B9C8FF")));

    // 2D caption on top (2D layers break 3D groups, so it always draws over the 3D scene).
    let mut caption = text_layer(
        p,
        &c,
        "Caption",
        TextDoc {
            text: "CAMERAS  ·  LIGHTS  ·  SHADOWS  ·  DEPTH OF FIELD".into(),
            size: 30.0,
            style: "Medium".into(),
            tracking: 220.0,
            fill: [0.78, 0.83, 1.0, 1.0],
            justify: Justify::Center,
            ..Default::default()
        },
        [960.0, 1000.0],
    );
    anim(&mut caption, "transform/opacity", keys(&[(0.3, Value::Scalar(0.0)), (1.2, Value::Scalar(100.0))]));

    c.layers = vec![caption, cam, key, fill, title, left, right, x1, x2, floor];
    c
}

/// Build the demo project.
pub fn demo_project() -> Project {
    let mut p = Project::default();
    let mut comp = Comp::new(1920, 1080, FrameRate::FPS_29_97, t(10.0));
    comp.background = [0.03, 0.035, 0.07];
    let solids = p.add_item("Solids", Label::Yellow, None, ItemKind::Folder);
    let precomps = p.add_item("Precomps", Label::Yellow, None, ItemKind::Folder);
    let (cw, ch) = (1920.0, 1080.0);
    let size = [cw, ch];

    // Lower third precomp.
    let lt = lower_third(&mut p);
    let lt_id = p.add_item("Lower Third", Label::Sandstone, Some(precomps), ItemKind::Comp(lt.into()));

    // Background: radial gradient.
    let mut bg = solid_layer(&mut p, &comp, solids, "Background", [0.0, 0.0, 0.0]);
    effect(
        &mut p,
        &mut bg,
        "ec.generate.gradientramp",
        size,
        &[
            ("start", Value::Vec2([960.0, 470.0])),
            ("end", Value::Vec2([960.0, 1500.0])),
            ("startColor", Value::Color(hex("#26306E"))),
            ("endColor", Value::Color(hex("#05060D"))),
            ("shape", Value::Enum(1)),
        ],
    );

    // Controller null drives the rings' rotation.
    let mut null = build::layer(&mut p, &comp, "Controller", LayerSource::Null, (100, 100), None);
    set(&mut null, "transform/anchor", Value::Vec3([50.0, 50.0, 0.0]));
    set(&mut null, "transform/position", Value::Vec3([960.0, 540.0, 0.0]));
    anim(&mut null, "transform/rotation", vec![Keyframe::new(t(0.0), Value::Scalar(-30.0)), Keyframe::new(t(10.0), Value::Scalar(60.0))]);
    let null_id = null.id;

    // Orbit rings with trim-path write-on.
    let mut rings = build::layer(&mut p, &comp, "Orbit Rings", LayerSource::Shape, (1920, 1080), None);
    let mut next = p.next_id;
    {
        let mut ids = Ids(&mut next);
        let e1 = build::shape_ellipse(&mut ids, [720.0, 720.0], [0.0, 0.0]);
        let s1 = build::shape_stroke(&mut ids, hex("#3D8FF5"), 4.0);
        let tr1 = build::shape_trim(&mut ids, 0.0, 0.0, 0.0);
        let g1 = build::shape_group(&mut ids, "Inner Ring", vec![e1, tr1, s1]);
        let e2 = build::shape_ellipse(&mut ids, [860.0, 860.0], [0.0, 0.0]);
        let mut s2 = build::shape_stroke(&mut ids, hex("#8E6BFF"), 2.0);
        if let Some(d) = s2.sub_mut("dashes") {
            if let Some(x) = d.get_mut("dash") {
                x.value = Value::Scalar(18.0);
            }
            if let Some(x) = d.get_mut("gap") {
                x.value = Value::Scalar(14.0);
            }
        }
        if let Some(x) = s2.get_mut("cap") {
            x.value = Value::Enum(1);
        }
        let tr2 = build::shape_trim(&mut ids, 0.0, 0.0, 0.0);
        let g2 = build::shape_group(&mut ids, "Outer Ring", vec![e2, tr2, s2]);
        contents(&mut rings, vec![g1, g2]);
    }
    p.next_id = next;
    set(&mut rings, "transform/position", Value::Vec3([50.0, 50.0, 0.0]));
    rings.parent = Some(null_id);
    anim(&mut rings, "contents/group#1/contents/trim/end", keys(&[(0.2, Value::Scalar(0.0)), (1.8, Value::Scalar(100.0))]));
    anim(&mut rings, "contents/group#2/contents/trim/end", keys(&[(0.5, Value::Scalar(0.0)), (2.3, Value::Scalar(100.0))]));
    anim(&mut rings, "contents/group#2/contents/trim/offset", vec![Keyframe::new(t(0.0), Value::Scalar(0.0)), Keyframe::new(t(10.0), Value::Scalar(-240.0))]);

    // Tick burst: a repeater of rounded bars around the centre.
    let mut burst = build::layer(&mut p, &comp, "Tick Burst", LayerSource::Shape, (1920, 1080), None);
    let mut next = p.next_id;
    {
        let mut ids = Ids(&mut next);
        let r = build::shape_rect(&mut ids, [6.0, 36.0], [0.0, -470.0], 3.0);
        let f = build::shape_fill(&mut ids, hex("#E8EEFF"));
        let mut rep = build::shape_repeater(&mut ids, 60.0, [0.0, 0.0]);
        if let Some(tr) = rep.sub_mut("transform") {
            if let Some(x) = tr.get_mut("rotation") {
                x.value = Value::Scalar(6.0);
            }
            if let Some(x) = tr.get_mut("endOpacity") {
                x.value = Value::Scalar(8.0);
            }
        }
        let g = build::shape_group(&mut ids, "Ticks", vec![r, f, rep]);
        contents(&mut burst, vec![g]);
    }
    p.next_id = next;
    anim(&mut burst, "transform/scale", keys_out(&[(0.0, Value::Vec3([60.0, 60.0, 100.0])), (1.6, Value::Vec3([100.0, 100.0, 100.0]))]));
    anim(&mut burst, "transform/rotation", vec![Keyframe::new(t(0.0), Value::Scalar(0.0)), Keyframe::new(t(10.0), Value::Scalar(-45.0))]);
    anim(&mut burst, "transform/opacity", keys(&[(0.0, Value::Scalar(0.0)), (1.0, Value::Scalar(55.0))]));
    burst.blend_mode = BlendMode::Add;

    // Accent line under the title.
    let mut line = build::layer(&mut p, &comp, "Accent Line", LayerSource::Shape, (1920, 1080), None);
    let mut next = p.next_id;
    {
        let mut ids = Ids(&mut next);
        let r = build::shape_rect(&mut ids, [640.0, 5.0], [0.0, 0.0], 2.5);
        let gf = build::shape_gradient_fill(
            &mut ids,
            false,
            [-320.0, 0.0],
            [320.0, 0.0],
            Gradient {
                colors: vec![(0.0, [0.24, 0.56, 0.96, 1.0]), (1.0, [0.56, 0.42, 1.0, 1.0])],
                opacities: vec![(0.0, 0.0), (0.2, 1.0), (0.8, 1.0), (1.0, 0.0)],
            },
        );
        let g = build::shape_group(&mut ids, "Line", vec![r, gf]);
        contents(&mut line, vec![g]);
    }
    p.next_id = next;
    set(&mut line, "transform/position", Value::Vec3([960.0, 600.0, 0.0]));
    anim(&mut line, "transform/scale", keys_out(&[(1.1, Value::Vec3([0.0, 100.0, 100.0])), (2.2, Value::Vec3([100.0, 100.0, 100.0]))]));

    // Title with a per-character rise + fade animator.
    let mut title = text_layer(
        &mut p,
        &comp,
        "EFFECTCRAFT",
        TextDoc { text: "EFFECTCRAFT".into(), size: 148.0, style: "Bold".into(), tracking: 120.0, justify: Justify::Center, ..Default::default() },
        [960.0, 560.0],
    );
    let mut next = p.next_id;
    {
        let mut ids = Ids(&mut next);
        let pos = build::text_anim_prop(&mut ids, "position").map(|mut pr| {
            pr.value = Value::Vec3([0.0, 90.0, 0.0]);
            pr
        });
        let op = build::text_anim_prop(&mut ids, "opacity").map(|mut pr| {
            pr.value = Value::Scalar(0.0);
            pr
        });
        let sc = build::text_anim_prop(&mut ids, "scale").map(|mut pr| {
            pr.value = Value::Vec3([60.0, 60.0, 100.0]);
            pr
        });
        let mut a = build::text_animator(&mut ids, "Animator 1", [pos, op, sc].into_iter().flatten().collect());
        if let Some(sel) = a.group_mut("selectors/#1")
            && let Some(adv) = sel.sub_mut("advanced")
            && let Some(sh) = adv.get_mut("shape")
        {
            sh.value = Value::Enum(1); // ramp up
        }
        if let Some(anims) = title.props.group_mut("text/animators") {
            anims.children.push(a.into());
        }
    }
    p.next_id = next;
    anim(&mut title, "text/animators/#1/selectors/#1/start", keys(&[(0.6, Value::Scalar(0.0)), (2.2, Value::Scalar(100.0))]));
    effect(
        &mut p,
        &mut title,
        "ec.stylize.glow",
        size,
        &[("threshold", Value::Scalar(40.0)), ("radius", Value::Scalar(36.0)), ("intensity", Value::Scalar(0.9))],
    );

    // Subtitle.
    let mut sub = text_layer(
        &mut p,
        &comp,
        "Tagline",
        TextDoc {
            text: "MOTION GRAPHICS  ·  VISUAL EFFECTS  ·  PURE RUST".into(),
            size: 30.0,
            style: "Medium".into(),
            tracking: 260.0,
            fill: [0.68, 0.75, 1.0, 1.0],
            justify: Justify::Center,
            ..Default::default()
        },
        [960.0, 666.0],
    );
    anim(&mut sub, "transform/opacity", keys(&[(1.8, Value::Scalar(0.0)), (2.6, Value::Scalar(100.0))]));
    anim(&mut sub, "transform/position", keys(&[(1.8, Value::Vec3([960.0, 690.0, 0.0])), (2.6, Value::Vec3([960.0, 666.0, 0.0]))]));

    // Lower third precomp in at 5s.
    let ltc = p.comp(lt_id).cloned();
    let mut lower = build::layer(&mut p, &comp, "Lower Third", LayerSource::Comp { item: lt_id }, (1920, 1080), ltc.map(|c| c.duration));
    lower.start_time = t(5.0);
    lower.in_point = t(5.0);
    lower.out_point = t(9.5);

    // Stack: top first.
    comp.layers = vec![lower, title, sub, line, burst, rings, null, bg];
    for l in &mut comp.layers {
        if l.name == "Controller" {
            l.switches.video = false;
        }
    }
    comp.work_area = (Tick::ZERO, t(10.0));
    p.add_item(MAIN_COMP, Label::Sandstone, None, ItemKind::Comp(comp.into()));
    let show = showcase_3d(&mut p, solids);
    p.add_item(SHOWCASE_3D, Label::Sandstone, None, ItemKind::Comp(show.into()));
    p.fix_next_id();
    p
}
