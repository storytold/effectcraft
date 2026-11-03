//! Key Light end to end: a green-screen plate (uneven screen, a subject with green spill and a
//! soft edge) keyed through the commands an agent uses: `effect.apply` by its After Effects name,
//! `effect.pickColor` on the screen (the effect's input, as the eyedropper does) and a render.

use std::sync::Arc;

use effectcraft_project::{Footage, FootageKind, ItemId};
use effectcraft_raster::Image;
use effectcraft_time::{FrameRate, Tick};
use serde_json::json;

use crate::Session;
use crate::render::{FootageSource, RenderOpts};

const W: u32 = 96;
const H: u32 = 64;
const SKIN: [f32; 3] = [0.86, 0.62, 0.52];

/// The plate's straight colour and alpha (opaque everywhere) at a pixel.
fn plate(x: u32, y: u32) -> [f32; 3] {
    // Screen: a slightly uneven, noisy green.
    let noise = (((x * 7919 + y * 104_729) % 97) as f32 / 97.0 - 0.5) * 0.02;
    let screen = [0.18 + noise, 0.66 + (x as f32 / W as f32 - 0.5) * 0.04 + noise, 0.30 - noise];
    let r = ((x as f32 + 0.5 - 48.0).powi(2) + (y as f32 + 0.5 - 32.0).powi(2)).sqrt();
    // Subject: skin, with green spill in its outer 3 px and a 3 px soft edge.
    let mut subject = SKIN;
    if r > 13.0 {
        subject[1] += 0.12;
    }
    let a = ((19.0 - r) / 3.0).clamp(0.0, 1.0);
    [0, 1, 2].map(|i| subject[i] * a + screen[i] * (1.0 - a))
}

struct Plate;

impl FootageSource for Plate {
    fn frame(&self, _: ItemId, _: &Footage, _: Tick) -> Option<Arc<Image>> {
        let mut img = Image::new(W, H);
        for y in 0..H {
            for x in 0..W {
                let c = plate(x, y);
                img.set(x, y, [c[0], c[1], c[2], 1.0]);
            }
        }
        Some(Arc::new(img))
    }
}

struct Probe;
impl crate::Importer for Probe {
    fn probe(&self, path: &str) -> Result<Footage, String> {
        Ok(Footage {
            path: path.into(),
            kind: FootageKind::Still,
            width: W,
            height: H,
            has_video: true,
            frame_rate: FrameRate::from_f64(24.0),
            duration: Tick::ZERO,
            ..Default::default()
        })
    }
}

fn keyed() -> (Session, ItemId) {
    let mut s = Session { footage: Arc::new(Plate), importer: Some(Arc::new(Probe)), ..Default::default() };
    let item = s.execute("file.import", json!({"paths": ["/plates/greenscreen.png"]})).unwrap()["items"][0].as_u64().unwrap();
    s.execute("comp.new", json!({"name": "Key", "width": W, "height": H, "frameRate": 24, "duration": 1})).unwrap();
    let c = s.active_comp_id().unwrap();
    s.execute("layer.addItem", json!({"item": item})).unwrap();
    // After Effects' name finds it; MCP's add_effect / list_effects go through the same lookup.
    let r = s.execute_checked("effect.apply", json!({"effect": "Keylight (1.2)"})).unwrap();
    assert_eq!(r["effect"], "ec.keying.keylight");
    (s, c)
}

#[test]
fn key_light_removes_a_green_screen() {
    let (mut s, c) = keyed();
    let steps = s.history.undo.len();
    // The default screen colour (pure green) already keys the screen, so the shown frame is
    // transparent there; the eyedropper samples the effect's input instead.
    let picked = s.execute_checked("effect.pickColor", json!({"effect": 1, "param": "screenColour", "x": 6, "y": 6, "average": true})).unwrap();
    let col: Vec<f64> = picked["color"].as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect();
    let want = plate(6, 6);
    assert!((0..3).all(|i| (col[i] - want[i] as f64).abs() < 0.02), "{col:?} vs {want:?}");
    let tree = s.execute("layer.tree", json!({})).unwrap().to_string();
    assert!(tree.contains("Screen Colour"), "{tree}");
    // One undo step.
    assert_eq!(s.history.undo.len(), steps + 1);
    // Clean the matte as one does in Screen Matte view: Clip Black for the uneven screen, Clip
    // White for the spill's partial transparency.
    for (path, v) in [("effects/#1/screenMatte/clipBlack", 10), ("effects/#1/screenMatte/clipWhite", 85)] {
        s.execute_checked("prop.set", json!({"path": path, "value": v})).unwrap();
    }

    let img = s.render(c, Tick::ZERO, RenderOpts::default());
    let px = |x, y| img.get(x, y);
    // The whole screen goes (corners, both ends of the gradient, noise).
    for (x, y) in [(1, 1), (94, 1), (1, 62), (94, 62), (20, 32), (76, 32)] {
        assert!(px(x, y)[3] < 0.03, "screen at {x},{y}: {:?}", px(x, y));
    }
    // The subject stays, with its colour.
    let fg = px(48, 32);
    assert!(fg[3] > 0.99 && (0..3).all(|i| (fg[i] - SKIN[i]).abs() < 0.02), "{fg:?}");
    // Spill on the subject's edge is suppressed: no green cast left.
    let sp = px(48, 32 - 14);
    assert!(sp[3] > 0.99 && sp[1] <= 0.5 * (sp[0] + sp[2]) + 0.01, "spill {sp:?}");
    // The soft edge keeps partial alpha, and its recovered colour has no green fringe.
    let e = px(48, 32 - 18);
    assert!(e[3] > 0.15 && e[3] < 0.85, "edge {e:?}");
    let a = e[3];
    assert!(e[1] / a <= 0.5 * (e[0] + e[2]) / a + 0.02, "edge fringe {e:?}");
}

#[test]
fn pick_color_rejects_bad_targets_and_screen_matte_view_shows_the_key() {
    let (mut s, c) = keyed();
    // Not a colour parameter / outside the layer.
    assert!(s.execute_checked("effect.pickColor", json!({"effect": 1, "param": "screenGain", "x": 6, "y": 6})).is_err());
    assert!(s.execute_checked("effect.pickColor", json!({"effect": 1, "param": "screenColour", "x": -50, "y": 6})).is_err());
    assert!(s.execute_checked("effect.pickColor", json!({"effect": 1, "param": "screenColour", "x": 1e300, "y": -1e300, "average": true})).is_err());
    assert!(s.execute_checked("effect.pickColor", json!({"effect": 1, "param": "screenColour"})).is_err());
    s.execute_checked("effect.pickColor", json!({"effect": 1, "param": "screenColour", "x": 90, "y": 60})).unwrap();
    // View ▸ Screen Matte: black screen (once Clip Black takes the uneven screen), white subject.
    for (path, v) in [("effects/#1/view", 4), ("effects/#1/screenMatte/clipBlack", 25)] {
        s.execute_checked("prop.set", json!({"path": path, "value": v})).unwrap();
    }
    let img = s.render(c, Tick::ZERO, RenderOpts::default());
    assert!(img.get(2, 2)[0] < 0.03 && img.get(48, 32)[0] > 0.97, "{:?} {:?}", img.get(2, 2), img.get(48, 32));
    // The Effects & Presets / list_effects search finds it by After Effects' name too.
    let list = s.execute("effect.list", json!({"filter": "Keylight (1.2)"})).unwrap();
    assert_eq!(list.as_array().map(|a| a.len()), Some(1), "{list}");
    assert_eq!(list[0]["id"], "ec.keying.keylight");
}
