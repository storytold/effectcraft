use super::*;
use crate::{EvalCtx, Image, NoFootage, RenderOpts, Renderer};
use effectcraft_effects::Buf;

// The same synthetic painted source as `drop_shadow_cover.rs`: no installed fonts, and none of
// the multi-gigabyte allocation from #4158, but the same text effect-stack bounds.
fn source(scale: f64) -> Buf {
    let mut img = Image::new(20, 12);
    for y in 2..10 {
        for x in 2..18 {
            img.set(x, y, [0.8, 0.2, 0.1, 1.0]);
        }
    }
    Buf { img, offset: [7.0, -3.0], scale }
}

fn text_with_fill(fill: &[(&str, Value)], then_glow: bool) -> (Project, ItemId, Comp, effectcraft_project::Layer) {
    let (mut p, cid, comp) = setup();
    let mut layer = build::layer(&mut p, &comp, "Text", LayerSource::Text, (0, 0), None);
    add_effect_200(&mut p, &mut layer, "ec.generate.fill", fill);
    if then_glow {
        add_effect_200(&mut p, &mut layer, "ec.stylize.glow", &[("radius", Value::Scalar(1.0))]);
    }
    (p, cid, comp, layer)
}

#[test]
fn fill_does_not_cover_the_zoomed_layer_rectangle() {
    for then_glow in [false, true] {
        let (p, cid, comp, layer) = text_with_fill(&[("opacity", Value::Scalar(70.0))], then_glow);
        let r = Renderer::new(&p, &NoFootage, RenderOpts::default());
        let ctx = EvalCtx::new(&p, cid, &comp, Tick::ZERO);
        let out = r.apply_effects(&ctx, &layer, source(4.0), false);
        // The cover of the layer rectangle makes this at least 800 x 400; the painted source,
        // its grid alignment and Glow's margin need far less.
        assert!(out.img.width < 192 && out.img.height < 192, "glow {then_glow}: {} x {}", out.img.width, out.img.height);
    }
}

#[test]
fn fill_matches_the_covered_reference() {
    for scale in [0.5, 1.0, 4.0] {
        for opacity in [100.0, 70.0] {
            for then_glow in [false, true] {
                let (p, cid, comp, layer) = text_with_fill(&[("opacity", Value::Scalar(opacity))], then_glow);
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
                // Compare the union in layer-pixel coordinates. Buffer origins differ by integral pixels.
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
                                "scale {scale}, opacity {opacity}, glow {then_glow}, ({x}, {y}), channel {c}: {} != {}",
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
fn inverted_fill_keeps_the_cover() {
    // Inverted, Fill paints the layer's transparent area, so it still needs the whole rectangle.
    let (p, cid, comp, layer) = text_with_fill(&[("invert", Value::Bool(true))], false);
    let r = Renderer::new(&p, &NoFootage, RenderOpts::default());
    let ctx = EvalCtx::new(&p, cid, &comp, Tick::ZERO);
    let out = r.apply_effects(&ctx, &layer, source(4.0), false);
    assert!(out.img.width >= 800 && out.img.height >= 400, "{} x {}", out.img.width, out.img.height);
}

#[test]
fn feathered_mask_fill_matches_the_covered_reference() {
    // A feathered Fill mask whose edge lies just outside the painted content: the feather reaches
    // into the painted pixels only if the coverage outside them is computed too.
    for scale in [1.0, 4.0] {
        let (mut p, cid, comp, mut layer) =
            text_with_fill(&[("fillMask", Value::Scalar(1.0)), ("horizontalFeather", Value::Scalar(6.0)), ("verticalFeather", Value::Scalar(6.0))], false);
        let mut next = p.next_id;
        // Painted layer x spans about -5..11 at scale 1; the mask's right edge sits at 13.
        let m = build::mask(&mut Ids(&mut next), "Mask 1", ShapePath::rect([-3.5, 5.0], 33.0, 50.0), MaskMode::None, [255, 255, 0]);
        p.next_id = next;
        layer.props.sub_mut("masks").unwrap().children.push(effectcraft_project::Node::Group(m));
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
        for y in 2..10 {
            for x in 2..18 {
                // The painted pixels, in the input buffer's coordinates.
                let lx = f64::from(x) - 7.0;
                let ly = f64::from(y) + 3.0;
                let sample = |b: &Buf| b.img.sample_bilinear(lx + b.offset[0] + 0.5, ly + b.offset[1] + 0.5);
                let (a, b) = (sample(&actual), sample(&reference));
                for c in 0..4 {
                    assert!((a[c] - b[c]).abs() < 1e-4, "scale {scale}, ({x}, {y}), channel {c}: {} != {}", a[c], b[c]);
                }
            }
        }
    }
}
