//! Pixel Motion Blur must synthesize blur from motion vectors, not average decoded frames
//! (#535). Two bars move in opposite directions between discrete source frames; the gap
//! pixels between their old and new positions must fill in, and Vector Detail must matter.

use effectcraft_color::Label;
use effectcraft_keyframe::Value;
use effectcraft_project::build::{self, Ids};
use effectcraft_project::{AlphaMode, BitDepth, Comp, Footage, FootageKind, FrameBlend, ItemId, ItemKind, LayerSource, Project};
use effectcraft_render::{FootageSource, Image, RenderOpts, Renderer};
use effectcraft_time::{FrameRate, Tick};
use std::sync::Arc;

// A 12×8 block with a horizontal brightness gradient slides right by 12 px between source
// frames 9 and 10. The gradient gives the block matcher real texture, so Pixel Motion can find
// the +12 vector and smear the block across the gap — unlike a flat bar, whose motion is
// undefined (the aperture problem) and which no synthesizer can invent coverage for.
struct Clip;
impl FootageSource for Clip {
    fn frame(&self, _: ItemId, f: &Footage, t: Tick) -> Option<Arc<Image>> {
        let k = f.frame_rate.frame_at(t).clamp(0, 19);
        let dx = 12 * (k - 10);
        let mut image = Image::filled(64, 48, [0.0, 0.0, 0.0, 1.0]);
        for y in 0..48 {
            for x in 0..64 {
                if (16..32).contains(&y) && (20 + dx..32 + dx).contains(&x) {
                    let g = (x - (20 + dx)) as f32 / 12.0;
                    image.set(x as u32, y, [0.4 + 0.6 * g, 0.3, 0.2, 1.0]);
                }
            }
        }
        Some(Arc::new(image))
    }
}

fn scene(detail: Option<f64>) -> (Project, ItemId) {
    let mut p = Project::default();
    p.settings.bit_depth = BitDepth::Bpc32;
    let comp = Comp::new(64, 48, FrameRate::FPS_25, Tick::from_seconds_f64(0.8));
    let cid = p.add_item("Comp", Label::Sandstone, None, ItemKind::Comp(comp.clone().into()));
    let footage = Footage {
        path: "generated.mov".into(),
        kind: FootageKind::Video,
        width: 64,
        height: 48,
        pixel_aspect: 1.0,
        frame_rate: FrameRate::FPS_25,
        duration: Tick::from_seconds_f64(0.8),
        has_video: true,
        has_audio: false,
        alpha: AlphaMode::Ignore,
        missing: false,
        ..Default::default()
    };
    let fid = p.add_item("Clip", Label::Aqua, None, ItemKind::Footage(footage));
    let mut layer = build::layer(&mut p, &comp, "Clip", LayerSource::Footage { item: fid }, (64, 48), None);
    layer.switches.frame_blend = FrameBlend::Off;
    layer.switches.motion_blur = false;
    if let Some(detail) = detail {
        let spec = effectcraft_effects::find("ec.time.pixelmotionblur").unwrap();
        let mut next = p.next_id;
        let mut effect = effectcraft_effects::instantiate(spec, &mut Ids(&mut next), spec.name, [64.0, 48.0]);
        for (name, value) in [
            ("shutterControl", Value::Enum(1)),
            ("shutterAngle", Value::Scalar(180.0)),
            ("shutterSamples", Value::Scalar(16.0)),
            ("vectorDetail", Value::Scalar(detail)),
        ] {
            effect.prop_mut(name).unwrap().value = value;
        }
        p.next_id = next;
        layer.props.sub_mut("effects").unwrap().children.push(effect.into());
    }
    p.comp_mut(cid).unwrap().layers.push(layer);
    (p, cid)
}

fn render(p: &Project, cid: ItemId, time: f64) -> Image {
    Renderer::new(p, &Clip, RenderOpts::default()).comp_frame(cid, Tick::from_seconds_f64(time))
}

fn difference(a: &Image, b: &Image) -> f64 {
    a.data.iter().zip(&b.data).flat_map(|(p, q)| (0..4).map(move |c| (p[c] - q[c]).abs() as f64)).sum()
}

#[test]
fn pixel_motion_blur_synthesizes_from_motion_vectors() {
    let (plain, cid) = scene(None);
    let still = render(&plain, cid, 0.4);
    let average = average_of(&plain, cid);
    let (low, low_id) = scene(Some(0.0));
    let (high, high_id) = scene(Some(100.0));
    let a = render(&low, low_id, 0.4);
    let b = render(&high, high_id, 0.4);
    assert!(difference(&b, &still) > 1.0, "the blur equals the still frame");
    assert!(difference(&b, &average) > 1.0, "the blur is just the plain overlay of the two decoded positions");
    assert!((24..32).any(|x| b.get(x, 24)[0] > 0.2), "no motion trail behind the moving block: {:?}", (18..46).map(|x| b.get(x, 24)[0]).collect::<Vec<f32>>());
    assert!(difference(&a, &b) > 0.5, "Vector Detail is inert: the motion field does not change with it");
}

fn average_of(p: &Project, cid: ItemId) -> Image {
    let mut average = Image::new(64, 48);
    for i in 0..16 {
        let time = ((0.39 + 0.02 * i as f64 / 15.0) * 1e6).round() / 1e6;
        let frame = render(p, cid, time);
        for (p, q) in average.data.iter_mut().zip(&frame.data) {
            for c in 0..4 {
                p[c] += q[c] / 16.0;
            }
        }
    }
    average
}
