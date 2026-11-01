//! The Backgrounds animation presets through the wired session (with the expression engine):
//! each one renders a non-flat, finite, deterministic frame that keeps moving, and undoes.

use effectcraft_engine::render::RenderOpts;
use effectcraft_time::Tick;
use serde_json::json;

type Frame = Vec<[f32; 4]>;

/// The frame at `secs`, its mean luma, luma standard deviation and whether any channel is not finite.
fn frame(s: &effectcraft_engine::Session, secs: f64) -> (Frame, f64, f64, bool) {
    let Some(cid) = s.active_comp_id() else { panic!("no active comp") };
    let img = s.render(cid, Tick::from_seconds_f64(secs), RenderOpts { scale: 1.0, ..Default::default() });
    let bad = img.data.iter().any(|p| p.iter().any(|c| !c.is_finite()));
    let l: Vec<f64> = img.data.iter().map(|p| 0.2126 * p[0] as f64 + 0.7152 * p[1] as f64 + 0.0722 * p[2] as f64).collect();
    let n = l.len().max(1) as f64;
    let mean = l.iter().sum::<f64>() / n;
    let sd = (l.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / n).sqrt();
    (img.data.clone(), mean, sd, bad)
}

fn diff(a: &[[f32; 4]], b: &[[f32; 4]]) -> f64 {
    a.iter().zip(b).map(|(p, q)| (0..3).map(|i| (p[i] - q[i]).abs() as f64).sum::<f64>()).sum()
}

#[test]
fn every_background_preset_renders_moves_and_undoes() {
    let mut s = effectcraft_host::session();
    s.execute("comp.new", json!({"name": "BG", "width": 640, "height": 360, "frameRate": 30, "duration": 8})).unwrap();
    let solid = s.execute("layer.newSolid", json!({"name": "S", "color": "#ffffff"})).unwrap()["layer"].as_u64().unwrap();
    let list = s.execute("anim.builtinPresets", json!({"category": "Backgrounds"})).unwrap();
    let list = list.as_array().unwrap().clone();
    assert!(list.len() >= 15, "{}", list.len());
    for p in &list {
        let id = p["id"].as_str().unwrap();
        s.execute("time.set", json!({"time": 0.0})).unwrap();
        let r = s.execute("anim.applyBuiltinPreset", json!({"layers": [solid], "preset": id})).unwrap();
        assert_eq!(r["skipped"], json!([]), "{id}");
        let mut frames = Vec::new();
        for t in [0.0, 1.3, 3.7] {
            let (px, mean, sd, nan) = frame(&s, t);
            if let Some(dir) = std::env::var_os("EC_PRESET_DUMP") {
                // Contact-sheet helper for reviewing the looks by eye (not part of the check).
                let mut ppm = b"P6\n640 360\n255\n".to_vec();
                ppm.extend(px.iter().flat_map(|p| (0..3).map(|i| (p[i].clamp(0.0, 1.0) * 255.0) as u8)));
                let _ = std::fs::write(std::path::Path::new(&dir).join(format!("{id}_{t}.ppm")), ppm);
            }
            assert!(!nan, "{id} @ {t}: NaN or infinite pixel");
            assert!(sd > 0.015, "{id} @ {t}: flat frame (mean {mean:.3}, sd {sd:.4})");
            let (again, ..) = frame(&s, t);
            assert!(px == again, "{id} @ {t}: render is not deterministic");
            frames.push(px);
        }
        assert!(diff(&frames[0], &frames[1]) > 100.0, "{id}: frame does not move between 0 s and 1.3 s");
        assert!(diff(&frames[1], &frames[2]) > 100.0, "{id}: frame does not move between 1.3 s and 3.7 s");
        s.execute("edit.undo", json!({})).unwrap();
        let fx = s.active_comp().map_or(1, |c| c.layers[0].effects().map_or(0, |fx| fx.groups().count()));
        assert_eq!(fx, 0, "{id}: undo removes the effects");
    }
}

/// #1510: each Behaviors preset keeps its layer moving (position, scale, rotation or opacity
/// changes over time) and renders finite, deterministic frames.
#[test]
fn every_behavior_preset_renders_and_moves() {
    let mut s = effectcraft_host::session();
    s.execute("comp.new", json!({"name": "BH", "width": 160, "height": 90, "frameRate": 30, "duration": 8})).unwrap();
    // A small white square in the middle so position, scale and rotation changes are visible.
    let solid = s.execute("layer.newSolid", json!({"name": "S", "color": "#ffffff", "width": 60, "height": 60})).unwrap()["layer"].as_u64().unwrap();
    let list = s.execute("anim.builtinPresets", json!({"category": "Behaviors"})).unwrap();
    let list = list.as_array().unwrap().clone();
    assert!(list.len() >= 20, "{}", list.len());
    for p in &list {
        let id = p["id"].as_str().unwrap();
        s.execute("time.set", json!({"time": 0.0})).unwrap();
        let r = s.execute("anim.applyBuiltinPreset", json!({"layers": [solid], "preset": id})).unwrap();
        assert_eq!(r["skipped"], json!([]), "{id}");
        let mut frames = Vec::new();
        // Times chosen off the periods of every preset so no two samples coincide.
        for t in [0.03, 0.13, 0.41, 0.97, 1.73] {
            let (px, _, _, nan) = frame(&s, t);
            assert!(!nan, "{id} @ {t}: NaN or infinite pixel");
            let (again, ..) = frame(&s, t);
            assert!(px == again, "{id} @ {t}: render is not deterministic");
            frames.push(px);
        }
        let moved = (1..frames.len()).any(|i| diff(&frames[0], &frames[i]) > 20.0);
        assert!(moved, "{id}: the layer does not move ({:?})", (1..frames.len()).map(|i| diff(&frames[0], &frames[i])).collect::<Vec<_>>());
        s.execute("edit.undo", json!({})).unwrap();
    }
}
