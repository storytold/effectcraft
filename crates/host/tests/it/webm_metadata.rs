//! A WebM rendered by EffectCraft and imported back reports its real frame count (#1429). Matroska
//! stores the duration in whole milliseconds, so at 30 fps a 1-frame file lasts 33 ms, just short
//! of one 33.33 ms frame.

use serde_json::json;

fn tmp(name: &str) -> String {
    let d = std::env::temp_dir().join(format!("effectcraft-host-webm-meta-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d.join(name).to_string_lossy().to_string()
}

#[test]
fn a_webm_at_30_fps_reports_every_frame() {
    for n in [1u32, 4] {
        let mut s = effectcraft_host::session();
        let out = tmp(&format!("frames-{n}.webm"));
        s.execute("comp.new", json!({"name": "Main", "width": 64, "height": 36, "frameRate": 30, "duration": 1})).unwrap();
        s.execute("layer.newSolid", json!({"name": "BG", "color": "#142850"})).unwrap();
        let span = json!({"comp": "Main", "format": "webm", "output": out, "timeSpan": "custom", "start": 0, "duration": n as f64 / 30.0});
        s.execute("renderQueue.add", span).unwrap();
        s.execute("renderQueue.render", json!({"wait": true})).unwrap();
        let r = s.execute("file.import", json!({"paths": [out.clone()]})).unwrap();
        let m = s.execute("item.metadata", json!({"item": r["items"][0]})).unwrap();
        let _ = std::fs::remove_file(&out);
        assert_eq!(m["item"]["frames"], json!(n), "{n} rendered frames: {m}");
    }
}
