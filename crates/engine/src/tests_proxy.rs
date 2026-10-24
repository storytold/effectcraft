//! Proxies (Set Proxy, Use Proxy, Proxy Use render settings, comp proxies, Create Proxy) and
//! Interpret Footage (fields, pixel aspect, alpha guess / invert, linear light), rendered through
//! a synthetic footage source.

use std::sync::Arc;

use effectcraft_project::render_queue::ProxyUse;
use effectcraft_project::{AlphaMode, Footage, FootageKind, ItemId, ItemKind};
use effectcraft_raster::Image;
use effectcraft_time::{FrameRate, Tick};
use serde_json::json;

use crate::Session;
use crate::render::{FootageSource, RenderOpts};

/// Files by name: `red16.png` (16×16 red), `blue8.png` (8×8 blue), `lines.mov` (interlaced:
/// even rows red, odd rows blue; 4×4, 10 fps), `matte_black.png` / `matte_white.png` (a
/// semi-transparent green premultiplied against black / white, read as straight), `half.png`
/// (white at 25 % alpha), `wide.png` (10×10 white).
struct Synthetic;

fn file_px(path: &str, x: u32, y: u32) -> [f32; 4] {
    let name = std::path::Path::new(path).file_name().and_then(std::ffi::OsStr::to_str).unwrap_or("");
    match name {
        "red16.png" => [1.0, 0.0, 0.0, 1.0],
        "blue8.png" => [0.0, 0.0, 1.0, 1.0],
        "lines.mov" => {
            if y.is_multiple_of(2) {
                [1.0, 0.0, 0.0, 1.0]
            } else {
                [0.0, 0.0, 1.0, 1.0]
            }
        }
        // Straight interpretation of a file whose colour is green·a + matte·(1 − a), a = 0.5.
        "matte_black.png" => [0.0, 0.5, 0.0, 0.5],
        "matte_white.png" => [0.5, 1.0, 0.5, 0.5],
        "half.png" => [1.0, 1.0, 1.0, 0.25],
        _ => {
            let _ = x;
            [1.0, 1.0, 1.0, 1.0]
        }
    }
}

fn size_of(path: &str) -> (u32, u32) {
    match std::path::Path::new(path).file_name().and_then(std::ffi::OsStr::to_str).unwrap_or("") {
        "red16.png" => (16, 16),
        "blue8.png" => (8, 8),
        "lines.mov" | "matte_black.png" | "matte_white.png" | "half.png" => (4, 4),
        _ => (10, 10),
    }
}

impl FootageSource for Synthetic {
    fn frame(&self, _: ItemId, f: &Footage, _t: Tick) -> Option<Arc<Image>> {
        let (w, h) = size_of(&f.path);
        let mut img = Image::new(w, h);
        for y in 0..h {
            for x in 0..w {
                // Decoders hand out premultiplied pixels (straight file colour × alpha).
                let p = file_px(&f.path, x, y);
                img.set(x, y, [p[0] * p[3], p[1] * p[3], p[2] * p[3], p[3]]);
            }
        }
        Some(Arc::new(img))
    }
}

struct Probe;
impl crate::Importer for Probe {
    fn probe(&self, path: &str) -> Result<Footage, String> {
        let (w, h) = size_of(path);
        let video = path.ends_with(".mov");
        Ok(Footage {
            path: path.into(),
            kind: if video { FootageKind::Video } else { FootageKind::Still },
            width: w,
            height: h,
            has_video: true,
            frame_rate: FrameRate::from_f64(10.0),
            duration: Tick::from_seconds_f64(if video { 2.0 } else { 0.0 }),
            ..Default::default()
        })
    }
}

fn session() -> Session {
    Session { footage: Arc::new(Synthetic), importer: Some(Arc::new(Probe)), ..Default::default() }
}

fn import(s: &mut Session, path: &str) -> ItemId {
    ItemId(s.execute("file.import", json!({"paths": [path]})).unwrap()["items"][0].as_u64().unwrap())
}

fn px(s: &Session, comp: ItemId, t: f64, opts: RenderOpts, x: i64, y: i64) -> [f32; 4] {
    s.render(comp, Tick::from_seconds_f64(t), opts).get(x, y)
}

fn red(p: [f32; 4]) -> bool {
    p[0] > 0.99 && p[2] < 0.01 && p[3] > 0.99
}
fn blue(p: [f32; 4]) -> bool {
    p[2] > 0.99 && p[0] < 0.01 && p[3] > 0.99
}

#[test]
fn footage_proxy_follows_use_proxy_and_proxy_use() {
    let mut s = session();
    let full = import(&mut s, "/m/red16.png");
    s.execute("comp.new", json!({"name": "C", "width": 16, "height": 16, "frameRate": 10, "duration": 1})).unwrap();
    let c = s.active_comp_id().unwrap();
    s.execute("layer.addItem", json!({"item": full.0})).unwrap();
    let o = RenderOpts::default();
    assert!(red(px(&s, c, 0.0, o, 8, 8)));
    s.execute("file.setProxy", json!({"item": full.0, "path": "/m/blue8.png"})).unwrap();
    // The 8×8 proxy fills the 16×16 footage frame.
    for (x, y) in [(1, 1), (8, 8), (14, 14)] {
        assert!(blue(px(&s, c, 0.0, o, x, y)), "proxy at {x},{y}");
    }
    assert!(s.project.item(full).unwrap().proxy.as_ref().unwrap().enabled);
    // Render Settings ▸ Proxy Use.
    assert!(red(px(&s, c, 0.0, RenderOpts { proxy: ProxyUse::UseNone, ..o }, 8, 8)));
    assert!(red(px(&s, c, 0.0, RenderOpts { proxy: ProxyUse::UseCompOnly, ..o }, 8, 8)));
    // Use Proxy off: the viewer shows the source, Use All Proxies still uses it.
    s.execute("file.useProxy", json!({"item": full.0})).unwrap();
    assert!(!s.project.item(full).unwrap().proxy.as_ref().unwrap().enabled);
    assert!(red(px(&s, c, 0.0, o, 8, 8)));
    assert!(blue(px(&s, c, 0.0, RenderOpts { proxy: ProxyUse::UseAll, ..o }, 8, 8)));
    // Undo / Set Proxy ▸ None.
    s.execute("edit.undo", json!({})).unwrap();
    assert!(blue(px(&s, c, 0.0, o, 8, 8)));
    s.execute("file.setProxyNone", json!({"item": full.0})).unwrap();
    assert!(s.project.item(full).unwrap().proxy.is_none());
    assert!(red(px(&s, c, 0.0, RenderOpts { proxy: ProxyUse::UseAll, ..o }, 8, 8)));
    // Render queue items carry Proxy Use (Best Settings: none).
    let r = s.execute("renderQueue.add", json!({"proxyUse": "all"})).unwrap();
    assert_eq!(s.project.render_queue[0].settings.proxy_use, ProxyUse::UseAll, "{r}");
    assert_eq!(effectcraft_project::render_queue::RenderSettings::default().proxy_use, ProxyUse::UseNone);
}

#[test]
fn comp_proxy_and_create_proxy() {
    let mut s = session();
    s.execute("comp.new", json!({"name": "Inner", "width": 16, "height": 16, "frameRate": 10, "duration": 1})).unwrap();
    let inner = s.active_comp_id().unwrap();
    s.execute("layer.newSolid", json!({"color": "#ff0000"})).unwrap();
    s.execute("comp.new", json!({"name": "Outer", "width": 16, "height": 16, "frameRate": 10, "duration": 1})).unwrap();
    let outer = s.active_comp_id().unwrap();
    s.execute("layer.addItem", json!({"item": inner.0})).unwrap();
    let o = RenderOpts::default();
    assert!(red(px(&s, outer, 0.0, o, 8, 8)));
    s.execute("file.setProxy", json!({"item": inner.0, "path": "/m/blue8.png"})).unwrap();
    assert!(blue(px(&s, outer, 0.0, o, 14, 1)));
    assert!(blue(px(&s, outer, 0.0, RenderOpts { proxy: ProxyUse::UseCompOnly, ..o }, 8, 8)));
    assert!(red(px(&s, outer, 0.0, RenderOpts { proxy: ProxyUse::UseNone, ..o }, 8, 8)));
    s.execute("file.setProxyNone", json!({"item": inner.0})).unwrap();
    // Create Proxy queues a half-resolution render whose post-render action sets the proxy.
    s.state.project_selection = vec![inner];
    let r = s.execute("file.createProxy", json!({"kind": "still"})).unwrap();
    let it = s.project.render_queue.last().unwrap().clone();
    assert_eq!(it.comp, inner);
    assert_eq!(it.post_render, effectcraft_project::render_queue::PostRenderAction::SetProxy);
    assert_eq!(it.settings.resolution, 0.5);
    assert!(r["output"].as_str().unwrap().contains("Inner_proxy_"), "{r}");
    let m = s.execute("file.createProxy", json!({"kind": "movie", "comp": inner.0})).unwrap();
    assert!(m["output"].as_str().unwrap().ends_with("Inner_proxy.mov"), "{m}");
    // The post-render action (what a finished render runs) attaches the file.
    let id = it.id;
    s.post_render_action(id, "/m/blue8.png").unwrap();
    assert_eq!(s.project.item(inner).unwrap().proxy.as_ref().unwrap().footage.path, "/m/blue8.png");
    assert!(blue(px(&s, outer, 0.0, o, 8, 8)));
}

#[test]
fn interpret_proxy_settings_apply_to_the_proxy() {
    let mut s = session();
    let full = import(&mut s, "/m/red16.png");
    s.execute("file.setProxy", json!({"item": full.0, "path": "/m/half.png"})).unwrap();
    s.execute("file.interpretProxy", json!({"item": full.0, "alpha": "ignore", "invertAlpha": true})).unwrap();
    let px = s.project.item(full).unwrap().proxy.clone().unwrap();
    assert_eq!(px.footage.alpha, AlphaMode::Ignore);
    assert!(px.footage.invert_alpha);
    let f = match &s.project.item(full).unwrap().kind {
        ItemKind::Footage(f) => f.clone(),
        _ => panic!("not footage"),
    };
    assert_eq!(f.alpha, AlphaMode::Straight, "the main footage keeps its interpretation");
}

/// Separate Fields: each field becomes a frame at twice the frame rate, the other field's lines
/// interpolated; the dominant field comes first.
#[test]
fn separate_fields_make_frames_from_fields() {
    let mut s = session();
    let mov = import(&mut s, "/m/lines.mov");
    s.execute("comp.new", json!({"name": "C", "width": 4, "height": 4, "frameRate": 20, "duration": 1})).unwrap();
    let c = s.active_comp_id().unwrap();
    s.execute("layer.addItem", json!({"item": mov.0})).unwrap();
    let o = RenderOpts::default();
    // Progressive: alternating rows.
    assert!(red(px(&s, c, 0.0, o, 1, 0)) && blue(px(&s, c, 0.0, o, 1, 1)));
    s.execute("file.interpretFootage", json!({"item": mov.0, "fields": "upper"})).unwrap();
    // Field 1 (upper = even rows, red) then field 2 (odd rows, blue), each 1/20 s.
    for y in 0..3 {
        assert!(red(px(&s, c, 0.0, o, 1, y)), "upper field at row {y}");
        assert!(blue(px(&s, c, 0.05, o, 1, y + 1)), "lower field at row {}", y + 1);
    }
    s.execute("file.interpretFootage", json!({"item": mov.0, "fields": "lower"})).unwrap();
    assert!(blue(px(&s, c, 0.0, o, 1, 2)));
    assert!(red(px(&s, c, 0.05, o, 1, 1)));
    let r = s.execute("file.interpretFootage", json!({"item": mov.0, "fields": "off"})).unwrap();
    assert_eq!(r["items"][0]["fields"], "Off");
}

/// Non-square pixels: a 10×10 footage with pixel aspect 2 covers 20×10 comp pixels.
#[test]
fn pixel_aspect_stretches_footage_in_the_comp() {
    let mut s = session();
    let w = import(&mut s, "/m/wide.png");
    s.execute("comp.new", json!({"name": "C", "width": 40, "height": 20, "frameRate": 10, "duration": 1})).unwrap();
    let c = s.active_comp_id().unwrap();
    s.execute("layer.addItem", json!({"item": w.0})).unwrap();
    let o = RenderOpts::default();
    let covered = |s: &Session| s.render(c, Tick::ZERO, o).data.iter().filter(|p| p[3] > 0.5).count();
    assert_eq!(covered(&s), 100);
    s.execute("file.interpretFootage", json!({"item": w.0, "pixelAspect": 2.0})).unwrap();
    assert_eq!(covered(&s), 200);
    // Centered on the comp: x 10..30 (edge pixels are filtered).
    assert!(px(&s, c, 0.0, o, 11, 10)[3] > 0.99 && px(&s, c, 0.0, o, 28, 10)[3] > 0.99);
    assert!(px(&s, c, 0.0, o, 8, 10)[3] < 0.01 && px(&s, c, 0.0, o, 31, 10)[3] < 0.01);
    // A comp with the same pixel aspect shows the footage 1:1.
    s.execute("comp.settings", json!({"pixelAspect": 2.0})).unwrap();
    assert_eq!(covered(&s), 100);
}

/// Alpha ▸ Guess: premultiplied files are recognised with their matte colour; Invert Alpha
/// and Interpret As Linear Light change the decoded pixels.
#[test]
fn guess_alpha_invert_and_linear_light() {
    let mut s = session();
    let b = import(&mut s, "/m/matte_black.png");
    let w = import(&mut s, "/m/matte_white.png");
    let h = import(&mut s, "/m/half.png");
    let foot = |s: &Session, i: ItemId| match &s.project.item(i).unwrap().kind {
        ItemKind::Footage(f) => f.clone(),
        _ => panic!("not footage"),
    };
    s.execute("file.interpretFootage", json!({"items": [b.0, w.0, h.0], "alpha": "guess"})).unwrap();
    assert_eq!(foot(&s, b).alpha, AlphaMode::Premultiplied);
    assert_eq!(foot(&s, b).premul_color, [0.0; 3]);
    assert_eq!(foot(&s, w).alpha, AlphaMode::Premultiplied);
    assert_eq!(foot(&s, w).premul_color, [1.0; 3]);
    // White at 25 %: not explained by a black matte (1 > 0.25) but it is by white… either way
    // the guess is consistent with the pixels.
    assert!(matches!(foot(&s, h).alpha, AlphaMode::Straight | AlphaMode::Premultiplied));
    // An explicit matte colour.
    s.execute("file.interpretFootage", json!({"item": b.0, "alpha": "premultiplied", "matteColor": "#808080"})).unwrap();
    assert!((foot(&s, b).premul_color[0] - 128.0 / 255.0).abs() < 1e-3);
    // Invert Alpha: 25 % → 75 %.
    s.execute("file.interpretFootage", json!({"item": h.0, "alpha": "straight", "invertAlpha": true})).unwrap();
    s.execute("comp.new", json!({"name": "C", "width": 4, "height": 4, "frameRate": 10, "duration": 1})).unwrap();
    let c = s.active_comp_id().unwrap();
    s.execute("layer.addItem", json!({"item": h.0})).unwrap();
    let p = px(&s, c, 0.0, RenderOpts::default(), 1, 1);
    assert!((p[3] - 0.75).abs() < 0.01 && p[0] > 0.7, "{p:?}");
    // Interpret As Linear Light keeps alpha and re-encodes the colour.
    s.execute("file.interpretFootage", json!({"item": h.0, "invertAlpha": false, "linearLight": true})).unwrap();
    let p = px(&s, c, 0.0, RenderOpts::default(), 1, 1);
    assert!((p[3] - 0.25).abs() < 0.01, "{p:?}");
    // Remember / Apply Interpretation carries the new settings.
    s.state.project_selection = vec![h];
    s.execute("file.rememberInterpretation", json!({"item": h.0})).unwrap();
    s.execute("file.applyInterpretation", json!({"items": [w.0]})).unwrap();
    assert!(foot(&s, w).linear_light);
    // Color ▸ Preserve RGB (#411) is set, rendered and carried by Apply Interpretation.
    let out = s.execute("file.interpretFootage", json!({"item": h.0, "preserveRgb": true})).unwrap();
    assert_eq!(out["items"][0]["preserveRgb"], true);
    let p = px(&s, c, 0.0, RenderOpts::default(), 1, 1);
    assert!((p[0] - 0.25).abs() < 0.01, "white at 25 %, unconverted: {p:?}");
    s.execute("file.rememberInterpretation", json!({"item": h.0})).unwrap();
    s.execute("file.applyInterpretation", json!({"items": [w.0]})).unwrap();
    assert!(foot(&s, w).preserve_rgb);
}
