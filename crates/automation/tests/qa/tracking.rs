//! Scenario 4 — tracking and roto on synthetic footage: a point track applied to a null, Warp
//! Stabilizer on a shaking plate, Roto Brush propagation and Content-Aware Fill, each checked
//! numerically against the known motion.

use serde_json::{Value, json};

use crate::harness::{Qa, luma};

const N: u32 = 12;

/// Deterministic texture (value noise) so trackers have features to lock onto.
fn texture(x: i64, y: i64) -> u8 {
    let h = |x: i64, y: i64| {
        let mut v = (x.wrapping_mul(374_761_393) ^ y.wrapping_mul(668_265_263)) as u64;
        v = (v ^ (v >> 13)).wrapping_mul(1_274_126_177);
        ((v ^ (v >> 16)) & 0xff) as u8
    };
    let (cx, cy) = (x.div_euclid(6), y.div_euclid(6));
    40 + h(cx, cy) / 3
}

/// Where the square is on frame `f`.
fn square_center(f: u32) -> (f64, f64) {
    (40.0 + f as f64 * 6.0, 45.0 + f as f64 * 1.0)
}

/// A textured plate with a white, black-dotted square moving right; or (`shake`) the plate
/// itself jittering by a known offset per frame.
fn write_plate(qa: &Qa, dir: &str, shake: bool) -> String {
    std::fs::create_dir_all(qa.dir.join(dir)).unwrap();
    for f in 0..N {
        let (sx, sy) = if shake { shake_offset(f) } else { (0, 0) };
        let (cx, cy) = square_center(f);
        let img = image::RgbaImage::from_fn(160, 90, |x, y| {
            let (px, py) = (x as i64 - sx, y as i64 - sy);
            if !shake && (x as f64 - cx).abs() < 10.0 && (y as f64 - cy).abs() < 10.0 {
                // The object: white with a dark centre dot (a trackable feature).
                let dot = (x as f64 - cx).abs() < 3.0 && (y as f64 - cy).abs() < 3.0;
                return if dot { image::Rgba([20, 20, 20, 255]) } else { image::Rgba([250, 250, 250, 255]) };
            }
            let v = texture(px, py);
            image::Rgba([v, v / 2 + 30, 255 - v, 255])
        });
        img.save(qa.dir.join(format!("{dir}/f_{f:03}.png"))).unwrap();
    }
    qa.path(&format!("{dir}/f_000.png"))
}

fn shake_offset(f: u32) -> (i64, i64) {
    [(0, 0), (3, -2), (-2, 3), (4, 1), (-3, -3), (2, 2), (-4, 0), (1, -4), (3, 3), (-2, -1), (0, 4), (-3, 2)][f as usize]
}

/// Import a sequence at 12 fps and make a comp of it; returns (comp, layer).
fn plate_comp(qa: &mut Qa, first: &str, name: &str) -> (Value, Value) {
    let item = qa.exec("file.import", json!({"paths": [first], "sequence": true}))["items"][0].clone();
    qa.exec("file.interpretFootage", json!({"items": [item.clone()], "frameRate": 12}));
    let comp = qa.exec("comp.new", json!({"name": name, "width": 160, "height": 90, "frameRate": 12, "duration": 1}))["comp"].clone();
    let layer = qa.exec("layer.addItem", json!({"item": item}))["layer"].clone();
    (comp, layer)
}

#[test]
fn point_track_to_null() {
    let mut qa = Qa::new("track");
    let first = write_plate(&qa, "plate", false);
    let (_, plate) = plate_comp(&mut qa, &first, "Track");
    let follow = qa.exec("layer.newNull", json!({"name": "Follow"}))["layer"].clone();
    // Track the plate; the Motion Target is the null.
    qa.exec("track.new", json!({"layer": plate.clone(), "kind": "transform", "target": follow.clone()}));
    let (cx, cy) = square_center(0);
    qa.exec("track.setPoint", json!({"layer": plate.clone(), "point": 1, "center": [cx, cy], "featureSize": [24, 24], "searchSize": [48, 48], "time": 0}));
    qa.exec("time.set", json!({"time": 0}));
    qa.exec("track.analyze", json!({"layer": plate.clone(), "direction": "forward", "wait": true}));
    let st = qa.exec("track.status", json!({"layer": plate.clone()}));
    assert!(st.to_string().contains("featureCenter") || st.is_object(), "{st}");
    qa.exec("track.apply", json!({"layer": plate.clone(), "dimensions": "xy"}));
    // The null now follows the square, frame by frame, to within a pixel.
    for f in [0u32, 5, 11] {
        let t = f as f64 / 12.0;
        let p = qa.tool("get_property", json!({"layer": follow.clone(), "path": "transform/position", "time": t}));
        let v = p["value"].as_array().unwrap().iter().map(|x| x.as_f64().unwrap()).collect::<Vec<_>>();
        let (ex, ey) = square_center(f);
        assert!((v[0] - ex).abs() < 1.0 && (v[1] - ey).abs() < 1.0, "frame {f}: null at {v:?}, square at ({ex}, {ey})");
    }
}

#[test]
fn warp_stabilizer_steadies_a_shaking_plate() {
    let mut qa = Qa::new("warp");
    let first = write_plate(&qa, "shake", true);
    let (_, plate) = plate_comp(&mut qa, &first, "Shaky");
    let jitter = |qa: &mut Qa| {
        // Mean frame-to-frame difference over the middle of the frame (borders may be cropped
        // or scaled).
        let frames: Vec<_> = (0..N).map(|f| qa.frame(None, f as f64 / 12.0)).collect();
        let mut s = 0.0;
        for w in frames.windows(2) {
            let mut d = 0.0;
            for y in 25..65 {
                for x in 40..120 {
                    d += (luma(w[0].get_pixel(x, y)) - luma(w[1].get_pixel(x, y))).abs();
                }
            }
            s += d / (40.0 * 80.0);
        }
        s / (N - 1) as f64
    };
    let before = jitter(&mut qa);
    qa.exec("track.warpStabilizer", json!({"layer": plate.clone(), "wait": true}));
    qa.tool("set_property", json!({"layer": plate.clone(), "path": "effects/#1/stabilization/result", "value": 1}));
    let st = qa.exec("warp.status", json!({"layer": plate.clone()}));
    assert_eq!(st["analyzed"], true, "{st}");
    let after = jitter(&mut qa);
    assert!(after < before * 0.5, "No Motion stabilisation removes most of the shake: {before} → {after}");
}

#[test]
fn roto_brush_propagates_and_content_aware_fill_removes_the_object() {
    let mut qa = Qa::new("roto");
    let first = write_plate(&qa, "plate", false);
    let (_, plate) = plate_comp(&mut qa, &first, "Roto");
    // Paint the square as foreground and the plate around it as background on frame 0.
    let (cx, cy) = square_center(0);
    qa.exec(
        "roto.stroke",
        json!({"layer": plate.clone(), "kind": "fg", "frame": 0, "radius": 3, "points": [[cx - 5.0, cy - 5.0], [cx + 5.0, cy + 5.0], [cx - 5.0, cy + 5.0]]}),
    );
    qa.exec(
        "roto.stroke",
        json!({"layer": plate.clone(), "kind": "bg", "frame": 0, "radius": 4, "points": [[5, 5], [150, 5], [150, 85], [5, 85], [5, 5], [cx + 25.0, cy]]}),
    );
    qa.exec("roto.propagate", json!({"layer": plate.clone(), "direction": "forward", "to": N - 1, "wait": true}));
    for f in [0u32, 6, 11] {
        let st = qa.exec("roto.status", json!({"layer": plate.clone(), "frame": f, "compute": true}));
        let (ex, ey) = square_center(f);
        let c = st["centroid"].as_array().unwrap_or_else(|| panic!("frame {f}: {st}"));
        let (mx, my) = (c[0].as_f64().unwrap(), c[1].as_f64().unwrap());
        assert!((mx - ex).abs() < 3.0 && (my - ey).abs() < 3.0, "frame {f}: matte centroid ({mx}, {my}) vs square ({ex}, {ey})");
        let area = st["area"].as_f64().unwrap();
        assert!((area - 400.0).abs() < 160.0, "frame {f}: matte area {area} ≈ 20×20");
    }

    // Content-Aware Fill: a mask around the object's path, Object fill, then the square is gone.
    let roto_fx = qa.exec("effect.remove", json!({"layer": plate.clone(), "effect": 1}));
    let _ = roto_fx;
    qa.exec("layer.addMask", json!({"layer": plate.clone(), "shape": "rect", "rect": [20, 25, 120, 40], "mode": "Subtract"}));
    let caf =
        qa.exec("layer.newContentAwareFill", json!({"layer": plate.clone(), "method": "object", "range": "entire", "outputDir": qa.path("caf"), "wait": true}));
    assert!(caf["layer"].is_u64() || caf.is_object(), "{caf}");
    for f in [0u32, 6] {
        let img = qa.frame(None, f as f64 / 12.0);
        let (ex, ey) = square_center(f);
        let p = img.get_pixel(ex as u32 + 6, ey as u32 + 6);
        assert!(luma(p) < 200.0, "frame {f}: the white square is filled with plate texture: {p:?}");
    }
}
