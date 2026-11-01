//! Motion blur of collapsed precomps (the precomp layer's own motion; the containing comp's
//! shutter) and of animated shape content, rendered through the commands.

use effectcraft_raster::Image;
use effectcraft_time::Tick;
use serde_json::{Value, json};

use crate::Session;
use crate::render::RenderOpts;

const W: u32 = 160;
const H: u32 = 90;

fn id(v: &Value, k: &str) -> u64 {
    v[k].as_u64().unwrap()
}

fn render(s: &Session, comp: u64) -> Image {
    s.render(effectcraft_project::ItemId(comp), Tick::from_seconds_f64(0.5), RenderOpts::default())
}

/// Pixels with partial alpha (motion-blur smear).
fn partial(img: &Image) -> usize {
    img.data.iter().filter(|p| p[3] > 0.05 && p[3] < 0.95).count()
}

fn max_diff(a: &Image, b: &Image) -> f32 {
    a.data.iter().zip(&b.data).flat_map(|(p, q)| (0..4).map(move |c| (p[c] - q[c]).abs())).fold(0.0, f32::max)
}

/// "Main" holding "Pre" (a 20×20 white square in the middle), the precomp layer moving 480 px/s
/// (20 px a frame at 24 fps) with motion blur on. `inner_move`: the square moves inside Pre too
/// (with its own motion blur switch), and Pre's shutter is closed (0°).
fn scene(collapse: bool, inner_move: bool) -> (Session, u64) {
    let mut s = Session::default();
    let pre = id(&s.execute("comp.new", json!({"name": "Pre", "width": W, "height": H, "frameRate": 24, "duration": 2})).unwrap(), "comp");
    let sq = id(&s.execute("layer.newSolid", json!({"name": "Square", "color": "#ffffff", "width": 20, "height": 20})).unwrap(), "layer");
    if inner_move {
        s.execute("comp.settings", json!({"shutterAngle": 0})).unwrap();
        s.execute("comp.setSwitch", json!({"switch": "motionBlur", "value": true})).unwrap();
        s.execute("layer.setSwitch", json!({"layers": [sq], "switch": "motionBlur", "value": true})).unwrap();
        for (t, x) in [(0.0, -160.0), (1.0, 320.0)] {
            s.execute("prop.addKey", json!({"layer": sq, "path": "transform/position", "time": t, "value": [x, 45.0]})).unwrap();
        }
    }
    let main = id(&s.execute("comp.new", json!({"name": "Main", "width": W, "height": H, "frameRate": 24, "duration": 2})).unwrap(), "comp");
    s.execute("comp.setSwitch", json!({"switch": "motionBlur", "value": true})).unwrap();
    let l = id(&s.execute("layer.addItem", json!({"item": pre})).unwrap(), "layer");
    s.execute("layer.setSwitch", json!({"layers": [l], "switch": "motionBlur", "value": true})).unwrap();
    s.execute("layer.setSwitch", json!({"layers": [l], "switch": "collapse", "value": collapse})).unwrap();
    if !inner_move {
        for (t, x) in [(0.0, -160.0), (1.0, 320.0)] {
            s.execute("prop.addKey", json!({"layer": l, "path": "transform/position", "time": t, "value": [x, 45.0]})).unwrap();
        }
    }
    (s, main)
}

#[test]
fn a_collapsed_precomp_layer_blurs_with_its_own_motion() {
    let (s, main) = scene(true, false);
    let collapsed = render(&s, main);
    // The square sits at x = 80 at 0.5 s and smears over 10 px (180° of 20 px).
    assert!(partial(&collapsed) > 100, "smeared: {}", partial(&collapsed));
    // As blurred as the same precomp rendered flat.
    let (s, main) = scene(false, false);
    let flat = render(&s, main);
    assert!(max_diff(&collapsed, &flat) < 0.08, "{}", max_diff(&collapsed, &flat));
    // Motion blur off for the render: sharp.
    let sharp = s.render(effectcraft_project::ItemId(main), Tick::from_seconds_f64(0.5), RenderOpts { motion_blur: false, ..Default::default() });
    assert!(partial(&sharp) < 10, "{}", partial(&sharp));
}

#[test]
fn collapsed_layers_use_the_containing_comps_shutter() {
    // Pre's own shutter is closed; drawn into Main (180°) the square's own motion blurs.
    let (s, main) = scene(true, true);
    assert!(partial(&render(&s, main)) > 100, "{}", partial(&render(&s, main)));
    // Rendered flat, Pre uses its own (closed) shutter.
    let (s, main) = scene(false, true);
    assert!(partial(&render(&s, main)) < 10, "{}", partial(&render(&s, main)));
}

/// A shape layer whose rectangle moves inside its contents (not its transform): motion blur
/// smears it like the same move made with the layer's Position.
#[test]
fn animated_shape_content_is_motion_blurred() {
    let shape = |content: bool, blur: bool| {
        let mut s = Session::default();
        let c = id(&s.execute("comp.new", json!({"name": "S", "width": W, "height": H, "frameRate": 24, "duration": 2})).unwrap(), "comp");
        s.execute("comp.setSwitch", json!({"switch": "motionBlur", "value": true})).unwrap();
        let l = id(&s.execute("layer.newShape", json!({"kind": "rect", "size": [20, 20], "fill": "#ffffff", "position": [80, 45]})).unwrap(), "layer");
        s.execute("layer.setSwitch", json!({"layers": [l], "switch": "motionBlur", "value": blur})).unwrap();
        let (target, base) = if content {
            let layer = s.active_comp().unwrap().layer(effectcraft_project::LayerId(l)).unwrap().clone();
            let mut found = None;
            layer.props.group("contents").unwrap().walk("contents", &mut |_, p| {
                if found.is_none() && p.match_id == "position" {
                    found = Some((json!({"prop": p.uid}), p.value.as_vec2()));
                }
            });
            found.unwrap()
        } else {
            (json!({"path": "transform/position"}), [80.0, 45.0])
        };
        for (t, dx) in [(0.0, -240.0), (1.0, 240.0)] {
            let mut p = target.clone();
            p["layer"] = json!(l);
            p["time"] = json!(t);
            p["value"] = json!([base[0] + dx, base[1]]);
            s.execute("prop.addKey", p).unwrap();
        }
        render(&s, c)
    };
    let content = shape(true, true);
    assert!(partial(&content) > 100, "smeared: {}", partial(&content));
    // Like the layer moved by its transform…
    let moved = shape(false, true);
    assert!(max_diff(&content, &moved) < 0.15, "{}", max_diff(&content, &moved));
    // …and sharp with the layer's motion blur switch off.
    assert!(partial(&shape(true, false)) < 40, "{}", partial(&shape(true, false)));
}
