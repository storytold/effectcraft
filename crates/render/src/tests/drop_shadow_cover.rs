use super::*;
use crate::{EvalCtx, Image, NoFootage, RenderOpts, Renderer};
use effectcraft_effects::Buf;

// A synthetic painted source avoids installed-font dependencies and the multi-gigabyte
// allocation in #3357 while exercising the same text/shape effect-stack bounds.
fn source(scale: f64) -> Buf {
    let mut img = Image::new(20, 12);
    for y in 2..10 {
        for x in 2..18 {
            img.set(x, y, [0.8, 0.2, 0.1, 1.0]);
        }
    }
    Buf { img, offset: [7.0, -3.0], scale }
}

#[test]
fn drop_shadow_does_not_cover_the_zoomed_layer_rectangle() {
    let (mut p, cid, comp) = setup();
    let mut layer = build::layer(&mut p, &comp, "Shape", LayerSource::Shape, (0, 0), None);
    add_effect_200(&mut p, &mut layer, "ec.perspective.dropshadow", &[("distance", Value::Scalar(2.0)), ("softness", Value::Scalar(1.0))]);
    let r = Renderer::new(&p, &NoFootage, RenderOpts::default());
    let ctx = EvalCtx::new(&p, cid, &comp, Tick::ZERO);
    let out = r.apply_effects(&ctx, &layer, source(4.0), false);
    // The old cover makes this at least 800 x 400. Alignment and the shadow's
    // own padding need less than 128 pixels per side for this source.
    assert!(out.img.width < 128 && out.img.height < 128, "{} x {}", out.img.width, out.img.height);
}

#[test]
fn drop_shadow_matches_the_covered_reference() {
    for scale in [0.5, 1.0, 4.0] {
        for only in [false, true] {
            for direction in [0.0, 135.0, 270.0] {
                let (mut p, cid, comp) = setup();
                let mut layer = build::layer(&mut p, &comp, "Text", LayerSource::Text, (0, 0), None);
                add_effect_200(
                    &mut p,
                    &mut layer,
                    "ec.perspective.dropshadow",
                    &[
                        ("distance", Value::Scalar(3.5)),
                        ("softness", Value::Scalar(2.0)),
                        ("direction", Value::Scalar(direction)),
                        ("shadowOnly", Value::Bool(only)),
                    ],
                );
                let r = Renderer::new(&p, &NoFootage, RenderOpts::default());
                let ctx = EvalCtx::new(&p, cid, &comp, Tick::ZERO);
                let input = source(scale);
                let mut covered = input.clone();
                let (size, origin) = ctx.effect_bounds(&layer);
                covered.rebase(origin);
                covered.cover(size);
                covered.rebase([-origin[0], -origin[1]]);
                let actual = r.apply_effects(&ctx, &layer, input, false);
                let reference = r.apply_effects(&ctx, &layer, covered, false);
                // Compare the union in layer-pixel coordinates, including the shadow outside
                // the original content. Buffer origins differ by integral pixels.
                let x0 = (-actual.offset[0]).min(-reference.offset[0]) as i32;
                let y0 = (-actual.offset[1]).min(-reference.offset[1]) as i32;
                let x1 = (f64::from(actual.img.width) - actual.offset[0]).max(f64::from(reference.img.width) - reference.offset[0]) as i32;
                let y1 = (f64::from(actual.img.height) - actual.offset[1]).max(f64::from(reference.img.height) - reference.offset[1]) as i32;
                for y in y0..y1 {
                    for x in x0..x1 {
                        let sample = |b: &Buf| b.img.sample_bilinear(f64::from(x) + b.offset[0] + 0.5, f64::from(y) + b.offset[1] + 0.5);
                        let (a, b) = (sample(&actual), sample(&reference));
                        for c in 0..4 {
                            assert!(
                                (a[c] - b[c]).abs() < 1e-5,
                                "scale {scale}, only {only}, direction {direction}, ({x}, {y}), channel {c}: {} != {}",
                                a[c],
                                b[c]
                            );
                        }
                    }
                }
            }
        }
    }
}
#[test]
fn drop_shadow_with_glow_stays_local_but_other_effects_keep_cover() {
    for other in ["ec.stylize.glow", "ec.color.tint"] {
        let (mut p, cid, comp) = setup();
        let mut layer = build::layer(&mut p, &comp, "Shape", LayerSource::Shape, (0, 0), None);
        add_effect_200(&mut p, &mut layer, "ec.perspective.dropshadow", &[("distance", Value::Scalar(2.0)), ("softness", Value::Scalar(1.0))]);
        let values = if other == "ec.stylize.glow" { vec![("radius", Value::Scalar(1.0))] } else { vec![] };
        add_effect_200(&mut p, &mut layer, other, &values);
        let r = Renderer::new(&p, &NoFootage, RenderOpts::default());
        let ctx = EvalCtx::new(&p, cid, &comp, Tick::ZERO);
        let out = r.apply_effects(&ctx, &layer, source(4.0), false);
        if other == "ec.stylize.glow" {
            assert!(out.img.width < 192 && out.img.height < 192, "{} x {}", out.img.width, out.img.height);
        } else {
            assert!(out.img.width >= 800 && out.img.height >= 400);
        }
    }
}
#[test]
fn parented_text_and_shape_keep_small_working_images_at_2200_percent() {
    for text in [false, true] {
        let mut p = Project::default();
        p.settings.bit_depth = effectcraft_project::BitDepth::Bpc32;
        let comp = Comp::new(80, 40, FrameRate::FPS_30, Tick::from_seconds_f64(1.0));
        let cid = p.add_item("Comp", Label::Sandstone, None, ItemKind::Comp(comp.clone().into()));
        let mut parent = build::layer(&mut p, &comp, "Null", LayerSource::Null, (100, 100), None);
        parent.props.prop_mut("transform/anchor").unwrap().value = Value::Vec3([0.0; 3]);
        parent.props.prop_mut("transform/position").unwrap().value = Value::Vec3([40.0, 20.0, 0.0]);
        parent.props.prop_mut("transform/scale").unwrap().value = Value::Vec3([2200.0, 2200.0, 100.0]);
        let source = if text { LayerSource::Text } else { LayerSource::Shape };
        let mut layer = build::layer(&mut p, &comp, "Painted", source, (80, 40), None);
        layer.parent = Some(parent.id);
        layer.props.prop_mut("transform/anchor").unwrap().value = Value::Vec3([0.0; 3]);
        layer.props.prop_mut("transform/position").unwrap().value = Value::Vec3([0.0; 3]);
        if text {
            layer.props.prop_mut("text/sourceText").unwrap().value = Value::Text(Box::new(TextDoc { text: "I".into(), size: 8.0, ..Default::default() }));
        } else {
            let mut next = p.next_id;
            let mut ids = Ids(&mut next);
            let rect = build::shape_rect(&mut ids, [4.0, 4.0], [0.0, 0.0], 0.0);
            let fill = build::shape_fill(&mut ids, [1.0; 4]);
            let group = build::shape_group(&mut ids, "Rectangle", vec![rect, fill]);
            p.next_id = next;
            layer.props.sub_mut("contents").unwrap().children.push(group.into());
        }
        add_effect(&mut p, &mut layer, "ec.stylize.glow", [80.0, 40.0], &[("radius", Value::Scalar(0.1))]);
        add_effect(&mut p, &mut layer, "ec.perspective.dropshadow", [80.0, 40.0], &[("distance", Value::Scalar(0.1)), ("softness", Value::Scalar(0.1))]);
        p.comp_mut(cid).unwrap().layers = vec![layer, parent].into();
        let comp = p.comp(cid).unwrap();
        let layer = &comp.layers[0];
        for scale in [1.0, 0.5] {
            let r = Renderer::new(&p, &NoFootage, RenderOpts { scale, ..Default::default() });
            let ctx = EvalCtx::new(&p, cid, comp, Tick::ZERO);
            let input = r.layer_input(&ctx, layer, usize::MAX).unwrap();
            assert!(input.img.width < 512 && input.img.height < 512, "text={text}, output scale={scale}: {} x {}", input.img.width, input.img.height);
            assert!(input.img.data.iter().any(|pixel| pixel[3] > 0.5), "the painted source is not empty");
            let frame = r.comp_frame(cid, Tick::ZERO);
            assert!(frame.data.iter().any(|pixel| pixel[3] > 0.5), "the layer still draws in the composition");
        }
    }
}
