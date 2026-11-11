//! Values the export cannot write as they are (clamped, rounded, replaced or left out) are
//! reported, and a strict export refuses before writing anything. Every project is built in code,
//! so these run without the corpus.

use crate::export::helpers::*;
use effectcraft_aep::export::{AepExportError, ExportOptions, LossCategory, analyze, export};
use effectcraft_color::Label;
use effectcraft_keyframe::text_doc::{CharStyle, StyleRun, TextDoc};
use effectcraft_keyframe::{Ease, Interp, Value};
use effectcraft_project::{AutoOrient, Footage, FootageKind, ItemKind, Layer, LayerSource, LightKind, Marker, Node, Project, PropGroup, Proxy, build};
use effectcraft_time::FrameRate;

/// `p` reports a loss in `category` whose detail contains `needle`, and a strict export refuses.
#[track_caller]
fn reported(p: &Project, category: LossCategory, needle: &str) {
    let r = analyze(p);
    let hit = r.losses.iter().any(|l| l.category == category && l.details.iter().any(|d| d.contains(needle)));
    assert!(hit, "no {category:?} loss with '{needle}': {:?}", r.losses);
    assert!(matches!(export(p, &ExportOptions { allow_losses: false }), Err(AepExportError::Refused(_))));
}

#[track_caller]
fn lossless(p: &Project) {
    let r = analyze(p);
    assert!(r.is_lossless(), "{:?}", r.losses);
}

fn first_layer(p: &mut Project, c: effectcraft_project::ItemId, f: impl FnOnce(&mut Layer)) {
    with_layer(p, c, 0, f);
}

#[test]
fn composition_settings_the_format_cannot_hold_are_reported() {
    let (p, c, _) = simple();
    lossless(&p);
    // A duration that is not a whole number of frames (the rate changed afterwards).
    let mut q = p.clone();
    with_comp(&mut q, c, |comp| comp.duration = secs(5.01));
    reported(&q, LossCategory::RoundedTimes, "duration moved to whole frames");
    // Shutter angle and phase are whole degrees.
    let mut q = p.clone();
    with_comp(&mut q, c, |comp| comp.shutter_angle = 180.5);
    reported(&q, LossCategory::CompositionSettings, "shutter angle 180.5");
    let mut q = p.clone();
    with_comp(&mut q, c, |comp| comp.shutter_phase = -12.25);
    reported(&q, LossCategory::CompositionSettings, "shutter phase -12.25");
    // The background has 8 bits per channel: the nearest level is written, a channel outside
    // 0 to 1 is reported.
    let mut q = p.clone();
    with_comp(&mut q, c, |comp| comp.background = [0.5, 0.0, 1.0]);
    lossless(&q);
    with_comp(&mut q, c, |comp| comp.background = [1.5, 0.0, 1.0]);
    reported(&q, LossCategory::CompositionSettings, "background channel 1.5");
    // The start timecode is a whole, non-negative number of frames.
    let mut q = p.clone();
    with_comp(&mut q, c, |comp| comp.display_start = secs(-2.0));
    reported(&q, LossCategory::CompositionSettings, "start timecode");
    let mut q = p.clone();
    with_comp(&mut q, c, |comp| comp.display_start = secs(1.0 + 1.0 / 51_200.0));
    reported(&q, LossCategory::CompositionSettings, "start timecode");
    // A pixel aspect without a small exact fraction.
    let mut q = p;
    with_comp(&mut q, c, |comp| comp.pixel_aspect = 1.0926);
    reported(&q, LossCategory::CompositionSettings, "pixel aspect 1.0926");
}

#[test]
fn solid_values_the_format_cannot_hold_are_reported() {
    let (p, _, s) = simple();
    let set = |f: &dyn Fn(&mut effectcraft_project::Solid)| {
        let mut q = p.clone();
        if let Some(ItemKind::Solid(solid)) = q.item_mut(s).map(|i| &mut i.kind) {
            f(solid);
        }
        q
    };
    reported(&set(&|s| s.color = [1.5, 0.0, 0.0]), LossCategory::InvalidValues, "colour channel 1.5");
    reported(&set(&|s| s.pixel_aspect = 2.0), LossCategory::FootageItems, "pixel aspect 2");
}

#[test]
fn items_lose_their_comment_proxy_and_footage_markers_with_a_report() {
    let (p, c, s) = simple();
    let mut q = p.clone();
    if let Some(i) = q.item_mut(c) {
        i.comment = "a note".into();
    }
    reported(&q, LossCategory::Names, "item comment");
    let still = Footage { path: "C:/x/a.png".into(), kind: FootageKind::Still, width: 64, height: 64, has_video: true, ..Footage::default() };
    let mut q = p.clone();
    if let Some(i) = q.item_mut(s) {
        i.proxy = Some(Box::new(Proxy { footage: still.clone(), enabled: true }));
    }
    reported(&q, LossCategory::FootageItems, "proxy");
    let mut q = p;
    let marked = Footage { markers: vec![Marker { time: secs(0.5), ..Marker::default() }], ..still };
    q.add_item("a.png", Label::None, None, ItemKind::Footage(marked));
    reported(&q, LossCategory::Markers, "footage marker");
}

#[test]
fn key_settings_the_export_does_not_keep_are_reported() {
    let (mut p, c, _) = simple();
    first_layer(&mut p, c, |l| {
        let o = l.props.prop_mut("transform/opacity").expect("opacity");
        o.keys = vec![key(0.0, Value::Scalar(0.0)), key(1.0, Value::Scalar(50.0)), key(2.0, Value::Scalar(100.0))];
    });
    lossless(&p);
    let keyed = |f: &dyn Fn(&mut effectcraft_keyframe::Keyframe)| {
        let mut q = p.clone();
        first_layer(&mut q, c, |l| {
            if let Some(k) = l.props.prop_mut("transform/opacity").and_then(|o| o.keys.get_mut(1)) {
                f(k);
            }
        });
        q
    };
    reported(&keyed(&|k| k.muted = true), LossCategory::ApproximatedKeyframes, "muted key");
    reported(
        &keyed(&|k| {
            k.in_interp = Interp::Bezier;
            k.out_interp = Interp::Bezier;
            k.auto_bezier = true;
        }),
        LossCategory::ApproximatedKeyframes,
        "auto-Bezier",
    );
    reported(
        &keyed(&|k| {
            k.out_interp = Interp::Bezier;
            k.out_ease = vec![Ease { speed: 1e12, influence: 0.5 }];
        }),
        LossCategory::InvalidValues,
        "key speed",
    );
}

#[test]
fn a_time_stretch_that_moves_a_layer_end_is_reported() {
    let (mut p, c, _) = simple();
    first_layer(&mut p, c, |l| {
        l.stretch = 150.0;
        l.start_time = secs(0.4);
        l.in_point = secs(0.0);
    });
    reported(&p, LossCategory::RoundedTimes, "after the time stretch");
}

fn text_doc(f: impl FnOnce(&mut CharStyle)) -> TextDoc {
    let mut d = TextDoc::plain("Text");
    let mut style = CharStyle { font: "ArialMT".into(), style: "Regular".into(), size: 50.0, leading: Some(60.0), ..CharStyle::default() };
    f(&mut style);
    d.set_runs(vec![StyleRun { len: 4, style }]);
    d
}

fn text_project(doc: TextDoc) -> Project {
    let mut p = Project::default();
    let c = add_comp(&mut p, "C", 1920, 1080, FrameRate::new(25, 1), 5.0, None);
    add_layer(&mut p, c, "T", LayerSource::Text, (1920, 1080));
    first_layer(&mut p, c, |l| {
        if let Some(st) = l.props.prop_mut("text/sourceText") {
            st.value = Value::Text(Box::new(doc));
        }
    });
    p
}

#[test]
fn text_values_the_document_cannot_hold_are_reported() {
    lossless(&text_project(text_doc(|_| {})));
    reported(&text_project(text_doc(|s| s.fill = [2.0, 0.0, 0.0, 1.0])), LossCategory::TextLayers, "fill colour channel 2");
    reported(&text_project(text_doc(|s| s.fill = [1.0, 0.0, 0.0, 0.5])), LossCategory::TextLayers, "fill colour opacity 0.5");
    reported(&text_project(text_doc(|s| s.tracking = 200_000.0)), LossCategory::TextLayers, "tracking 200000");
    reported(&text_project(text_doc(|s| s.baseline_shift = -2e5)), LossCategory::TextLayers, "baseline shift");
    reported(&text_project(text_doc(|s| s.font = String::new())), LossCategory::TextLayers, "without a font name");
    let mut boxed = text_doc(|_| {});
    boxed.box_size = Some([2e6, 100.0]);
    reported(&text_project(boxed), LossCategory::TextLayers, "box width");
}

#[test]
fn a_switched_off_range_selector_is_reported() {
    let mut p = text_project(text_doc(|_| {}));
    let c = p.comps().next().map(|(id, _)| *id).expect("comp");
    let mut next = p.next_id;
    let mut animator = build::text_animator(&mut build::Ids(&mut next), "Animator 1", Vec::new());
    p.next_id = next;
    first_layer(&mut p, c, |l| {
        if let Some(Node::Group(sel)) = animator.sub_mut("selectors").and_then(|s| s.children.first_mut()) {
            sel.enabled = false;
        }
        l.props.sub_mut("text").and_then(|t| t.sub_mut("animators")).expect("animators").children.push(animator.into());
    });
    reported(&p, LossCategory::TextLayers, "switched-off selector");
}

#[test]
fn camera_and_light_options_that_are_not_written_are_reported() {
    let mut p = Project::default();
    let c = add_comp(&mut p, "C", 1920, 1080, FrameRate::new(25, 1), 5.0, None);
    add_layer(&mut p, c, "Cam", LayerSource::Camera, (1920, 1080));
    let mut q = p.clone();
    first_layer(&mut q, c, |l| l.auto_orient = AutoOrient::AlongPath);
    reported(&q, LossCategory::Cameras, "Auto-Orient");

    let mut p = Project::default();
    let c = add_comp(&mut p, "C", 1920, 1080, FrameRate::new(25, 1), 5.0, None);
    add_layer(&mut p, c, "Spot", LayerSource::Light { kind: LightKind::Spot }, (1920, 1080));
    let mut q = p.clone();
    first_layer(&mut q, c, |l| l.auto_orient = AutoOrient::Off);
    reported(&q, LossCategory::Lights, "Auto-Orient");
    let mut q = p;
    first_layer(&mut q, c, |l| {
        if let Some(col) = l.props.prop_mut("lightOptions/color") {
            col.value = Value::Color([2.0, 0.5, 0.5, 1.0]);
        }
    });
    reported(&q, LossCategory::Lights, "light colour channel");
}

/// `simple()` with a new instance of the effect `id` on its layer, changed by `f`.
fn with_effect(id: &str, f: impl FnOnce(&mut PropGroup)) -> Project {
    let (mut p, c, _) = simple();
    let spec = effectcraft_effects::find(id).expect(id);
    let mut next = p.next_id;
    let mut g = effectcraft_effects::instantiate(spec, &mut build::Ids(&mut next), spec.name, [1920.0, 1080.0]);
    p.next_id = next;
    f(&mut g);
    first_layer(&mut p, c, |l| l.props.sub_mut("effects").expect("effects").children.push(Node::Group(g)));
    p
}

fn set(g: &mut PropGroup, path: &str, v: Value) {
    g.prop_mut(path).unwrap_or_else(|| panic!("{path}")).value = v;
}

#[test]
fn effect_values_the_format_cannot_hold_are_reported() {
    lossless(&with_effect("ec.color.tint", |_| {}));
    reported(&with_effect("ec.color.tint", |g| set(g, "white", Value::Color([1.5, 0.0, 0.0, 1.0]))), LossCategory::InvalidValues, "colour channel");
    reported(&with_effect("ec.blur.fastbox", |g| set(g, "iterations", Value::Scalar(2.5))), LossCategory::Effects, "whole number");
    reported(&with_effect("ec.color.tint", |g| g.label = Some(Label::Red)), LossCategory::Effects, "label colour");
    // After Effects' colour parameters have no opacity; Drop Shadow uses EffectCraft's.
    reported(&with_effect("ec.perspective.dropshadow", |g| set(g, "color", Value::Color([0.0, 0.0, 0.0, 0.5]))), LossCategory::Effects, "opacity");
    // A parameter the table has no record for, at a value other than a new instance's.
    lossless(&with_effect("ec.time.timewarp", |_| {}));
    reported(&with_effect("ec.time.timewarp", |g| set(g, "tuning/vectorDetail", Value::Scalar(50.0))), LossCategory::Effects, "Vector Detail");
    // Curves points outside 0..1.
    reported(&with_effect("ec.color.curves", |g| set(g, "rgb", Value::Str("0,0;1.2,1".into()))), LossCategory::Effects, "outside 0 to 1");
}

#[test]
fn per_corner_roundness_is_reported() {
    let mut p = Project::default();
    let c = add_comp(&mut p, "C", 1920, 1080, FrameRate::new(25, 1), 2.0, None);
    add_layer(&mut p, c, "S", LayerSource::Shape, (1920, 1080));
    let mut next = p.next_id;
    let mut ids = build::Ids(&mut next);
    let mut rect = build::shape_rect(&mut ids, [400.0, 300.0], [0.0, 0.0], 15.0);
    set(&mut rect, "perCorner", Value::Bool(true));
    let fill = build::shape_fill(&mut ids, [0.0, 1.0, 0.0, 1.0]);
    let group = build::shape_group(&mut ids, "Group 1", vec![rect, fill]);
    p.next_id = next;
    first_layer(&mut p, c, |l| l.props.sub_mut("contents").expect("contents").children.push(group.into()));
    reported(&p, LossCategory::ShapeLayers, "per-corner roundness");
}

#[test]
fn footage_interpretation_and_durations_that_are_not_written_are_reported() {
    let still = Footage { path: "C:/x/a.png".into(), kind: FootageKind::Still, width: 64, height: 64, has_video: true, ..Footage::default() };
    let with = |f: Footage| {
        let (mut p, _, _) = simple();
        p.add_item("a", Label::None, None, ItemKind::Footage(f));
        p
    };
    lossless(&with(still.clone()));
    reported(&with(Footage { alpha: effectcraft_project::AlphaMode::Premultiplied, ..still.clone() }), LossCategory::FootageItems, "alpha interpretation");
    reported(&with(Footage { pixel_aspect: 2.0, ..still.clone() }), LossCategory::FootageItems, "pixel aspect");
    reported(&with(Footage { loop_count: 3, ..still.clone() }), LossCategory::FootageItems, "loop count");
    let clip = Footage { path: "C:/x/a.mp4".into(), kind: FootageKind::Video, frame_rate: FrameRate::new(25, 1), duration: secs(0.0), ..still };
    reported(&with(clip), LossCategory::FootageItems, "written as 1 frame");
}

#[test]
fn mask_values_beyond_the_record_limits_are_reported() {
    let (mut p, c, _) = simple();
    let mut next = p.next_id;
    let mut g = build::mask(
        &mut build::Ids(&mut next),
        "Mask 1",
        effectcraft_keyframe::ShapePath::rect([960.0, 540.0], 400.0, 200.0),
        effectcraft_project::MaskMode::Add,
        build::MASK_COLORS[0],
    );
    p.next_id = next;
    set(&mut g, "feather", Value::Vec2([40_000.0, 10.0]));
    let mut q = p.clone();
    let h = g.clone();
    first_layer(&mut q, c, |l| l.props.sub_mut("masks").expect("masks").children.push(h.into()));
    reported(&q, LossCategory::InvalidValues, "feather");
    set(&mut g, "feather", Value::Vec2([10.0, 10.0]));
    set(&mut g, "expansion", Value::Scalar(-50_000.0));
    first_layer(&mut p, c, |l| l.props.sub_mut("masks").expect("masks").children.push(g.into()));
    reported(&p, LossCategory::InvalidValues, "Mask expansion");
}

#[test]
fn camera_options_stretch_curves_and_expressions_that_change_are_reported() {
    // A camera option that is not a number.
    let mut p = Project::default();
    let c = add_comp(&mut p, "C", 1920, 1080, FrameRate::new(25, 1), 5.0, None);
    add_layer(&mut p, c, "Cam", LayerSource::Camera, (1920, 1080));
    first_layer(&mut p, c, |l| {
        if let Some(z) = l.props.prop_mut("cameraOptions/zoom") {
            z.value = Value::Bool(true);
        }
    });
    reported(&p, LossCategory::InvalidValues, "cameraOptions/zoom");
    // A stretch that is not a number is reported, not taken as 100 %.
    let (mut p, c, _) = simple();
    first_layer(&mut p, c, |l| l.stretch = f64::NAN);
    reported(&p, LossCategory::TimeStretch, "");
    // A curve that is not text.
    reported(&with_effect("ec.color.curves", |g| set(g, "rgb", Value::Scalar(1.0))), LossCategory::InvalidValues, "not text");
    // An expression on a popup whose items are After Effects' in another order (Invert's
    // channels: After Effects counts its separators, EffectCraft has none).
    let p = with_effect("ec.channel.invert", |g| {
        g.prop_mut("channel").expect("channel").expr = Some(effectcraft_project::Expression { text: "6".into(), enabled: true });
    });
    reported(&p, LossCategory::Expressions, "other units");
}

/// A 3D layer's Material Options are not written: a value other than a new layer's, a key or an
/// expression is reported (one loss each), and a strict export refuses (review finding: changed
/// options were dropped without a report). The defaults, and any value on a 2D layer (where they
/// do nothing), stay lossless.
#[test]
fn changed_material_options_of_a_3d_layer_are_reported() {
    let three_d = |f: &dyn Fn(&mut PropGroup)| {
        let (mut p, c, _) = simple();
        first_layer(&mut p, c, |l| {
            l.switches.three_d = true;
            f(l.props.sub_mut("materialOptions").expect("material options"));
        });
        p
    };
    lossless(&three_d(&|_| {}));
    for (name, change) in [
        ("Accepts Lights", &(|g: &mut PropGroup| set(g, "acceptsLights", Value::Bool(false))) as &dyn Fn(&mut PropGroup)),
        ("Casts Shadows", &|g: &mut PropGroup| set(g, "castsShadows", Value::Enum(1))),
        ("Diffuse", &|g: &mut PropGroup| g.prop_mut("diffuse").expect("diffuse").keys = vec![key(0.0, Value::Scalar(50.0)), key(1.0, Value::Scalar(80.0))]),
        ("Ambient", &|g: &mut PropGroup| {
            g.prop_mut("ambient").expect("ambient").expr = Some(effectcraft_project::Expression { text: "100".into(), enabled: true })
        }),
    ] {
        let p = three_d(change);
        reported(&p, LossCategory::ThreeD, &format!("material option {name}"));
        assert_eq!(analyze(&p).count(LossCategory::ThreeD), 1, "{name}");
    }
    let (mut flat, c, _) = simple();
    first_layer(&mut flat, c, |l| set(l.props.sub_mut("materialOptions").expect("material options"), "acceptsLights", Value::Bool(false)));
    lossless(&flat);
}

/// A popup expression is written as it is: both programs count popup items from 1, so where the
/// items are the same in the same order (Fast Box Blur's Blur Dimensions) its result picks the
/// same item, and nothing is reported (review finding: every popup expression was reported).
#[test]
fn a_popup_expression_with_the_same_items_is_not_a_loss() {
    let p = with_effect("ec.blur.fastbox", |g| {
        g.prop_mut("dimensions").expect("dimensions").expr = Some(effectcraft_project::Expression { text: "1".into(), enabled: true });
    });
    let r = effectcraft_aep::export::export(&p, &effectcraft_aep::export::ExportOptions { allow_losses: true }).expect("export");
    assert_eq!(r.report.count(LossCategory::Expressions), 0, "{:?}", r.report.losses);
    assert!(r.report.is_lossless(), "{:?}", r.report.losses);
    assert!(effectcraft_aep::export::export(&p, &effectcraft_aep::export::ExportOptions { allow_losses: false }).is_ok());
    assert_eq!(r.report.exported.expressions, 1);
}

#[test]
fn a_repeaters_order_is_written_not_reported() {
    let mut p = Project::default();
    let c = add_comp(&mut p, "C", 1920, 1080, FrameRate::new(25, 1), 2.0, None);
    add_layer(&mut p, c, "S", LayerSource::Shape, (1920, 1080));
    let mut next = p.next_id;
    let mut ids = build::Ids(&mut next);
    let rect = build::shape_rect(&mut ids, [100.0, 100.0], [0.0, 0.0], 0.0);
    let fill = build::shape_fill(&mut ids, [0.0, 1.0, 0.0, 1.0]);
    let mut rep = build::shape_repeater(&mut ids, 3.0, [120.0, 0.0]);
    set(&mut rep, "composite", Value::Enum(1));
    let group = build::shape_group(&mut ids, "Group 1", vec![rect, fill, rep]);
    p.next_id = next;
    first_layer(&mut p, c, |l| l.props.sub_mut("contents").expect("contents").children.push(group.into()));
    lossless(&p);
}

#[test]
fn composition_options_and_the_render_queue_that_are_not_written_are_reported() {
    // The base writes these at After Effects' defaults (cdta 196 and 200 hold 128 and 16 in every
    // sample, the switches are not decoded) and writes an empty render queue: each non-default
    // value is reported, and a strict export refuses.
    let (p, c, _) = simple();
    lossless(&p);
    let cases: [(&dyn Fn(&mut effectcraft_project::Comp), &str); 4] = [
        (&|comp| comp.preserve_frame_rate = true, "Preserve Frame Rate"),
        (&|comp| comp.preserve_resolution = true, "Preserve Resolution"),
        (&|comp| comp.motion_blur_samples = 32, "motion blur samples per frame"),
        (&|comp| comp.motion_blur_adaptive_limit = 256, "adaptive sample limit"),
    ];
    for (set, needle) in cases {
        let mut q = p.clone();
        with_comp(&mut q, c, set);
        reported(&q, LossCategory::CompositionOptions, needle);
        assert_eq!(analyze(&q).count(LossCategory::CompositionOptions), 1, "{needle}");
    }
    // Viewer state is a listed limitation, not a loss (the Home templates set a poster time).
    let mut q = p.clone();
    with_comp(&mut q, c, |comp| {
        comp.hide_shy = true;
        comp.draft_3d = true;
        comp.poster_time = secs(1.5);
        comp.resolution = effectcraft_project::Resolution::Half;
    });
    lossless(&q);
    // A render queue item is not written.
    let mut q = p;
    q.render_queue.push(effectcraft_project::render_queue::RenderQueueItem::new(1, c));
    reported(&q, LossCategory::RenderQueue, "render queue item of 'C'");
}
