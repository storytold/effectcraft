//! Scenario 3 — compositing: an imported image sequence and a layered PSD (as a composition),
//! a blend mode, a track matte, an adjustment layer (Lumetri + Glow) limited by a feathered,
//! expanded mask, a camera with depth of field and a light in Classic 3D and Advanced 3D; then
//! ProRes 4444 and WebM VP9 renders with alpha, read back through import.

use serde_json::{Value, json};

use crate::harness::{Img, Qa, coverage, luma, mean_diff};

fn lum(i: &Img, x: u32, y: u32) -> f64 {
    luma(i.get_pixel(x, y))
}

fn px(img: &Img, x: u32, y: u32) -> [u8; 4] {
    img.get_pixel(x, y).0
}

/// A 12-frame PNG sequence of an orange ball crossing a blue background.
fn write_sequence(qa: &Qa) -> String {
    std::fs::create_dir_all(qa.dir.join("seq")).unwrap();
    for f in 0..12u32 {
        let cx = 20.0 + f as f64 * 10.0;
        let img = image::RgbaImage::from_fn(160, 90, |x, y| {
            let d = ((x as f64 - cx).powi(2) + (y as f64 - 45.0).powi(2)).sqrt();
            if d < 15.0 { image::Rgba([255, 170, 0, 255]) } else { image::Rgba([32, 64, 96, 255]) }
        });
        img.save(qa.dir.join(format!("seq/ball_{f:03}.png"))).unwrap();
    }
    qa.path("seq/ball_000.png")
}

/// Import the sequence (reinterpreted at 12 fps) and a PSD written by the app; returns the
/// sequence item and the PSD's composition.
fn import_footage(qa: &mut Qa) -> (Value, Value) {
    // Footage: the sequence, and a layered PSD written by the app itself (Save Frame As ▸
    // Photoshop Layers) from a small comp.
    let seq = write_sequence(qa);
    let imp = qa.exec("file.import", json!({"paths": [seq], "sequence": true}));
    let seq_item = imp["items"][0].clone();
    let project = qa.tool("get_project", json!({}));
    let it = project["items"].as_array().unwrap().iter().find(|i| i["id"] == seq_item).unwrap().clone();
    assert_eq!(it["type"], "Image Sequence", "{it}");
    // Sequences import at 30 fps (Settings ▸ Import); these frames were made for 12.
    assert!((it["duration"].as_f64().unwrap() - 0.4).abs() < 1e-6, "{it}");
    qa.exec("file.interpretFootage", json!({"items": [seq_item.clone()], "frameRate": 12}));
    let project = qa.tool("get_project", json!({}));
    let it = project["items"].as_array().unwrap().iter().find(|i| i["id"] == seq_item).unwrap().clone();
    assert!((it["duration"].as_f64().unwrap() - 1.0).abs() < 1e-6, "12 frames at 12 fps: {it}");
    qa.exec("comp.new", json!({"name": "Art", "width": 160, "height": 90, "frameRate": 12, "duration": 1}));
    qa.exec("layer.newSolid", json!({"name": "Gray", "color": "#606060", "width": 80, "height": 90}));
    qa.tool("set_property", json!({"layer": "Gray", "path": "transform/position", "value": [120, 45]}));
    qa.exec("layer.newShape", json!({"kind": "rect", "name": "Bar", "size": [160, 10], "fill": "#00ff00", "position": [80, 10]}));
    let psd = qa.path("art.psd");
    let saved = qa.exec("comp.saveFrameAsPsd", json!({"path": psd, "comp": "Art", "time": 0}));
    assert_eq!(saved["layers"], 2, "{saved}");
    let pimp = qa.exec("file.import", json!({"paths": [psd], "importAs": "composition"}));
    let psd_comp = pimp["comps"][0].clone();
    let pc = qa.tool("get_comp", json!({"comp": psd_comp.clone()}));
    let names: Vec<&str> = pc["layers"].as_array().unwrap().iter().map(|l| l["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["Bar", "Gray"], "the PSD's layers, top first: {pc}");
    (seq_item, psd_comp)
}

/// The main comp: the sequence plays, then a Screen-blended PSD; returns the Normal-mode frame.
fn main_comp_and_blend(qa: &mut Qa, seq_item: &Value, psd_comp: &Value) -> Img {
    // The main comp.
    qa.exec("comp.new", json!({"name": "Main", "width": 160, "height": 90, "frameRate": 12, "duration": 1}));
    qa.exec("layer.addItem", json!({"item": seq_item.clone()}));
    let a = qa.frame(None, 0.0);
    let b = qa.frame(None, 0.5);
    assert!(mean_diff(&a, &b) > 1.0, "the sequence plays");
    assert_eq!(px(&a, 20, 45)[..3], [255, 170, 0], "frame 0: the ball at x=20");
    assert_eq!(px(&b, 80, 45)[..3], [255, 170, 0], "frame 6 at 0.5 s: the ball at x=80");

    let art = qa.exec("layer.addItem", json!({"item": psd_comp.clone()}))["layer"].clone();
    let normal = qa.frame(None, 0.5);
    qa.exec("layer.setBlendMode", json!({"layers": [art.clone()], "mode": "Screen"}));
    let screen = qa.frame(None, 0.5);
    // Screen over the blue background: brighter than both inputs (0x60 gray over 0x20,0x40,0x60).
    let (n, s) = (px(&normal, 140, 60), px(&screen, 140, 60));
    assert_eq!(n[..3], [0x60, 0x60, 0x60], "Normal shows the gray");
    for c in 0..3 {
        assert!(s[c] > n[c] && s[c] > [32, 64, 96][c], "Screen brightens channel {c}: {s:?}");
    }
    normal
}

fn track_matte(qa: &mut Qa) {
    // Track matte: a magenta solid seen only through text.
    qa.exec("layer.newText", json!({"text": "MATTE", "name": "Matte", "size": 40, "fill": "#ffffff"}));
    qa.exec("layer.newSolid", json!({"name": "Fill", "color": "#ff00ff", "width": 160, "height": 90}));
    let magenta = |p: &image::Rgba<u8>| p[0] > 230 && p[1] < 30 && p[2] > 230;
    let full = coverage(&qa.frame(None, 0.5), magenta);
    assert!(full > 0.95);
    qa.exec("layer.setTrackMatte", json!({"layer": "Fill", "matte": "Matte", "kind": "alpha"}));
    let matted = qa.frame(None, 0.5);
    let m = coverage(&matted, magenta);
    assert!(m > 0.03 && m < 0.4, "magenta only inside the glyphs: {m}");
    assert!(coverage(&matted, |p| p[0] > 250 && p[1] > 250 && p[2] > 250) < 0.002, "the matte layer itself is hidden");
    let info = qa.tool("get_comp", json!({}));
    let fill = info["layers"].as_array().unwrap().iter().find(|l| l["name"] == "Fill").unwrap().clone();
    assert_eq!(fill["trackMatte"]["kind"], "Alpha Matte");
    qa.exec("layer.setSwitch", json!({"layers": ["Fill", "Matte"], "switch": "video", "value": false}));
}

fn adjustment_layer(qa: &mut Qa, normal: &Img) {
    // Adjustment layer: Lumetri exposure, inside a feathered, expanded elliptical mask; then Glow.
    qa.exec("layer.newAdjustment", json!({"name": "Grade"}));
    let fx = qa.exec("effect.apply", json!({"effect": "Lumetri Color", "layers": ["Grade"]}));
    assert_eq!(fx["paths"], json!(["effects/#1"]), "effect.apply answers with the instance path: {fx}");
    qa.tool("set_property", json!({"layer": "Grade", "path": "effects/#1/basicCorrection/tone/exposure", "value": 2}));
    let before = qa.frame(None, 0.5);
    let e = qa.exec_err("layer.mask.set", json!({"layer": "Grade", "field": "feather", "value": 10}));
    assert!(e.contains("layer.addMask"), "the error says how to make a mask: {e}");
    let mk = qa.exec("layer.addMask", json!({"layer": "Grade", "shape": "ellipse", "rect": [40, 20, 80, 50]}));
    assert!(mk["mask"].is_u64(), "{mk}");
    let hard = qa.frame(None, 0.5);
    assert!(lum(&hard, 80, 45) > lum(&before, 80, 45) - 1.0 && lum(&hard, 5, 5) < lum(&before, 5, 5) - 10.0, "the mask limits the grade to the ellipse");
    qa.exec("layer.mask.set", json!({"layer": "Grade", "field": "expansion", "value": 8}));
    let grown = qa.frame(None, 0.5);
    let bright = |i: &Img, base: &Img| {
        let mut n = 0;
        for (x, y, p) in i.enumerate_pixels() {
            if luma(p) > luma(base.get_pixel(x, y)) + 20.0 {
                n += 1;
            }
        }
        n
    };
    assert!(bright(&grown, normal) > bright(&hard, normal) + 100, "expansion grows the graded area");
    qa.exec("layer.mask.set", json!({"layer": "Grade", "field": "feather", "value": 20}));
    let soft = qa.frame(None, 0.5);
    // At the ellipse's edge (x = 40 − 8): feathered is in between none and full.
    let (edge_hard, edge_soft) = (lum(&grown, 33, 45), lum(&soft, 33, 45));
    assert!((edge_hard - edge_soft).abs() > 5.0, "feather softens the edge: {edge_hard} vs {edge_soft}");
    qa.exec("effect.apply", json!({"effect": "Glow", "layers": ["Grade"]}));
    let glow = qa.frame(None, 0.5);
    assert!(mean_diff(&soft, &glow) > 0.5, "Glow changes the frame");
}

/// The 3D stage with depth of field and a light; returns the stage comp.
fn stage_3d(qa: &mut Qa) -> Value {
    // 3D stage: a near and a far card, a camera with depth of field, a light.
    let stage = qa.exec("comp.new", json!({"name": "Stage", "width": 160, "height": 90, "frameRate": 12, "duration": 1}))["comp"].clone();
    qa.exec("layer.newSolid", json!({"name": "Near", "color": "#ff4040", "width": 40, "height": 40}));
    qa.exec("layer.newSolid", json!({"name": "Far", "color": "#40ff40", "width": 40, "height": 40}));
    qa.exec("layer.setSwitch", json!({"layers": ["Near", "Far"], "switch": "threeD", "value": true}));
    qa.tool("set_property", json!({"layer": "Near", "path": "transform/position", "value": [50, 45, 0]}));
    qa.tool("set_property", json!({"layer": "Far", "path": "transform/position", "value": [110, 45, 300]}));
    let cam = qa.exec("layer.newCamera", json!({"name": "Cam", "preset": "50mm", "dof": false}))["layer"].clone();
    let sharp = qa.frame(None, 0.0);
    qa.tool("set_property", json!({"layer": cam.clone(), "path": "cameraOptions/dof", "value": true}));
    let zoom = qa.tool("get_property", json!({"layer": cam.clone(), "path": "cameraOptions/zoom"}))["value"].as_f64().unwrap();
    qa.tool("set_property", json!({"layer": cam.clone(), "path": "cameraOptions/focusDistance", "value": zoom}));
    qa.tool("set_property", json!({"layer": cam.clone(), "path": "cameraOptions/aperture", "value": 40}));
    let dof = qa.frame(None, 0.0);
    let green = |p: &image::Rgba<u8>| p[1] > 200 && p[0] < 120;
    let red = |p: &image::Rgba<u8>| p[0] > 200 && p[1] < 120;
    assert!(coverage(&dof, green) < coverage(&sharp, green) * 0.8, "the far card defocuses");
    assert!((coverage(&dof, red) - coverage(&sharp, red)).abs() < 0.02, "the card at the focus distance stays sharp");
    // A light behind the near card leaves it dark; in front, lit.
    let light = qa.exec("layer.newLight", json!({"kind": "Point", "name": "Key", "intensity": 100, "position": [50, 45, 400]}))["layer"].clone();
    let back = qa.frame(None, 0.0);
    qa.tool("set_property", json!({"layer": light, "path": "transform/position", "value": [50, 45, -150]}));
    let front = qa.frame(None, 0.0);
    assert!(lum(&back, 50, 45) < 30.0 && lum(&front, 50, 45) > 60.0, "lighting: {} vs {}", lum(&back, 50, 45), lum(&front, 50, 45));
    let classic = front;
    assert_eq!(qa.exec("comp.renderer", json!({"renderer": "advanced3d"}))["renderer"], "advanced3d");
    let advanced = qa.frame(None, 0.0);
    assert!(mean_diff(&classic, &advanced) > 0.2, "Advanced 3D renders the stage its own way");
    assert!(lum(&advanced, 50, 45) > 60.0, "Advanced 3D still lights the card");
    stage
}

fn alpha_renders(qa: &mut Qa, stage: &Value) {
    // Renders with alpha: the stage has no background layer, so most of it is transparent.
    let mov = qa.path("stage.mov");
    let webm = qa.path("stage.webm");
    qa.exec("renderQueue.add", json!({"comp": stage.clone(), "format": "prores", "proresProfile": "4444", "channels": "rgba", "output": mov}));
    qa.exec("renderQueue.add", json!({"comp": stage.clone(), "format": "webm", "channels": "rgba", "output": webm}));
    let done = qa.exec("renderQueue.render", json!({"wait": true}));
    assert!(done["items"].as_array().unwrap().iter().all(|i| i["status"] == "Done"), "{done}");
    let reference = qa.tool("render_frame", json!({"comp": stage.clone(), "time": 0, "transparent": true, "inline": false}));
    assert_eq!(reference["width"], 160);
    let refa = alpha_frame(qa, stage.clone());
    assert!(px(&refa, 2, 2)[3] < 40 && px(&refa, 50, 45)[3] == 255, "render_frame transparent keeps alpha: {:?} {:?}", px(&refa, 2, 2), px(&refa, 50, 45));
    // ProRes 4444 carries alpha in the frame; WebM VP9 as a second VP9 stream in BlockAdditions
    // (AlphaMode 1), which FilmCraft's Matroska reader decodes into the frame's alpha plane.
    for path in [mov, webm] {
        let r = qa.exec("file.import", json!({"paths": [path]}));
        assert_eq!(r["errors"], json!([]));
        assert_eq!(r["unlabeledAlpha"], r["items"], "{path}: the importer sees an alpha channel");
        let c = qa.exec("file.newCompFromSelection", json!({}))["comps"][0].clone();
        let f = alpha_frame(qa, c.clone());
        // The opaque card decodes to the stage's colours (lossy codecs, so loosely).
        let (got, want) = (px(&f, 50, 45), px(&refa, 50, 45));
        assert!((0..3).all(|c| got[c].abs_diff(want[c]) < 12), "{path} decodes to the stage: {got:?} vs {want:?}");
        assert!(mean_diff(&qa.frame(Some(c.clone()), 0.0), &qa.frame(Some(stage.clone()), 0.0)) < 8.0, "{path} composites like the stage");
        assert!(px(&f, 2, 2)[3] < 40 && px(&f, 50, 45)[3] > 250, "{path}: transparent corners, opaque card: {:?} {:?}", px(&f, 2, 2), px(&f, 50, 45));
    }
}

#[test]
fn compositing_matte_blend_adjustment_3d_and_alpha_renders() {
    let mut qa = Qa::new("compositing");
    let (seq_item, psd_comp) = import_footage(&mut qa);
    let normal = main_comp_and_blend(&mut qa, &seq_item, &psd_comp);
    track_matte(&mut qa);
    adjustment_layer(&mut qa, &normal);
    let stage = stage_3d(&mut qa);
    alpha_renders(&mut qa, &stage);
}

fn alpha_frame(qa: &mut Qa, comp: Value) -> Img {
    let c = qa.try_tool("render_frame", json!({"comp": comp, "time": 0, "max_side": 0, "transparent": true})).unwrap();
    let png = effectcraft_automation::base64::decode(c[0]["data"].as_str().unwrap()).unwrap();
    image::load_from_memory(&png).unwrap().to_rgba8()
}
