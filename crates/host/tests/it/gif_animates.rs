//! Animated GIF footage through the wired session (#651): the imported item is a movie whose
//! frames follow the file's own delays in preview rendering, not one frozen still.

use effectcraft_engine::render::RenderOpts;
use effectcraft_project::{ItemId, ItemKind};
use effectcraft_time::Tick;
use serde_json::json;

fn tmp(name: &str) -> String {
    let d = std::env::temp_dir().join(format!("effectcraft-host-gif-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d.join(name).to_string_lossy().to_string()
}

/// A 3-frame 16×16 GIF: red, green, blue, one tenth of a second each.
fn animated_gif() -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut enc = gif::Encoder::new(&mut out, 16, 16, &[]).unwrap();
        for rgb in [[255u8, 0, 0], [0, 255, 0], [0, 0, 255]] {
            let mut f = gif::Frame::from_rgb(16, 16, &rgb.repeat(256));
            f.delay = 10;
            enc.write_frame(&f).unwrap();
        }
    }
    out
}

fn centre(s: &effectcraft_engine::Session, comp: ItemId, t: f64) -> [f32; 3] {
    let img = s.render(comp, Tick::from_seconds_f64(t), RenderOpts::default());
    let p = img.data[8 * 16 + 8];
    [p[0], p[1], p[2]]
}

#[test]
fn an_imported_animated_gif_plays_its_frames() {
    let mut s = effectcraft_host::session();
    let path = tmp("anim.gif");
    std::fs::write(&path, animated_gif()).unwrap();
    s.execute("comp.new", json!({"name": "Gif", "width": 16, "height": 16, "frameRate": 10, "duration": 1})).unwrap();
    let before = s.project.items.len();
    s.execute("file.import", json!({"paths": [path.clone()], "addToComp": true})).unwrap();
    let f = match s.project.items.values().find_map(|i| match &i.kind {
        ItemKind::Footage(f) if f.path == path => Some(f),
        _ => None,
    }) {
        Some(f) => f.clone(),
        None => panic!("no footage item for the GIF"),
    };
    assert_eq!(s.project.items.len(), before + 1, "one footage item, not a comp");
    assert_eq!((f.codec.as_str(), f.has_video), ("GIF", true), "probed as an animated movie");
    let cid = s.active_comp_id().unwrap();
    let (r, g, b) = (centre(&s, cid, 0.05), centre(&s, cid, 0.15), centre(&s, cid, 0.25));
    let _ = std::fs::remove_file(&path);
    assert!(r[0] > 0.9 && r[1] < 0.1, "frame at 0.05 s is red, got {r:?}");
    assert!(g[1] > 0.9 && g[0] < 0.1, "frame at 0.15 s is green, got {g:?}");
    assert!(b[2] > 0.9 && b[0] < 0.1, "frame at 0.25 s is blue, got {b:?}");
}
