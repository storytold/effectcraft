//! `effect.pickCryptomatte` / `effect.cryptomatteAt` (#473): the Cryptomatte eyedropper reads the
//! aux channels the effect renders from (footage at its source time, a 3D precomp's generated
//! pass) and edits the Selection text in one undo step.

use std::sync::Arc;

use effectcraft_project::{Footage, FootageKind, ItemId};
use effectcraft_raster::channels3d::{crypto_float, crypto_hash};
use effectcraft_raster::{AuxChannels, Image};
use effectcraft_time::{FrameRate, Tick};
use serde_json::{Value, json};

use crate::Session;
use crate::render::FootageSource;

const COMMA: &str = "Green, \"Glossy\"";

/// 8 × 4 footage with a 16 × 8 Cryptomatte (two aux pixels per layer pixel). Layer pixels in x:
/// 0-1 `left` (0.6) over `right` (0.4); 2-3 `left` (0.3) under the comma name (0.7), ranks out of
/// order; 4-5 `star*`; 6 an ID missing from the manifest; 7 a 0.5 / 0.5 tie of `left` and
/// `right`. Rows 3+ are empty. From 1 s of source time `late` replaces columns 0-1.
struct Crypto;

fn aux_at(t: Tick) -> AuxChannels {
    let (w, h) = (16usize, 8usize);
    let mut planes = vec![vec![0.0f32; w * h]; 4];
    for (i, x) in (0..w * h).map(|i| (i, i % w)).filter(|(i, _)| i / w < 6) {
        let late = if t >= Tick::from_seconds_f64(1.0) { "late" } else { "left" };
        let (a, ca, b, cb) = match x {
            0..4 => (late, 0.6, "right", 0.4),
            4..8 => ("left", 0.3, COMMA, 0.7),
            8..12 => ("star*", 1.0, "", 0.0),
            12..14 => ("ghost", 1.0, "", 0.0),
            _ => ("left", 0.5, "right", 0.5),
        };
        for (k, v) in [crypto_float(crypto_hash(a)), ca, crypto_float(crypto_hash(b)), cb].into_iter().enumerate() {
            planes[k][i] = v;
        }
    }
    let mut aux = AuxChannels::new(w as u32, h as u32, 1.0);
    for (k, c) in ["R", "G", "B", "A"].iter().enumerate() {
        aux.channels.push((format!("CryptoObject00.{c}"), planes[k].clone()));
    }
    aux.manifests = vec![("CryptoObject".into(), ["left", "right", "late", COMMA, "star*"].iter().map(|n| (n.to_string(), crypto_hash(n))).collect())];
    aux
}

impl FootageSource for Crypto {
    fn frame(&self, _: ItemId, f: &Footage, _: Tick) -> Option<Arc<Image>> {
        Some(Arc::new(Image::new(f.width, f.height)))
    }
    fn aux(&self, _: ItemId, _: &Footage, t: Tick) -> Option<Arc<AuxChannels>> {
        Some(Arc::new(aux_at(t)))
    }
}

struct Probe;
impl crate::Importer for Probe {
    fn probe(&self, path: &str) -> Result<Footage, String> {
        let duration = Tick::from_seconds_f64(2.0);
        // A half-size proxy of the EXR.
        let (width, height) = if path.contains("proxy") { (4, 2) } else { (8, 4) };
        Ok(Footage {
            path: path.into(),
            kind: FootageKind::Video,
            width,
            height,
            has_video: true,
            frame_rate: FrameRate::from_f64(24.0),
            duration,
            ..Default::default()
        })
    }
}

fn session() -> Session {
    let mut s = Session { footage: Arc::new(Crypto), importer: Some(Arc::new(Probe)), ..Default::default() };
    let item = s.execute("file.import", json!({"paths": ["/renders/crypto.exr"]})).unwrap()["items"][0].clone();
    s.execute("comp.new", json!({"name": "C", "width": 8, "height": 4, "frameRate": 24, "duration": 2})).unwrap();
    s.execute("layer.addItem", json!({"item": item})).unwrap();
    s.execute_checked("effect.apply", json!({"effect": "Cryptomatte"})).unwrap();
    s
}

fn pick(s: &mut Session, x: f64, mode: &str) -> Value {
    s.execute_checked("effect.pickCryptomatte", json!({"effect": 1, "x": x, "y": 1.5, "mode": mode})).unwrap()
}

fn selection(s: &Session) -> String {
    let l = &s.active_comp().unwrap().layers[0];
    let g = l.effects().unwrap().groups().next().unwrap();
    match &g.prop("selection").unwrap().value {
        effectcraft_keyframe::Value::Str(t) => t.clone(),
        v => panic!("{v:?}"),
    }
}

#[test]
fn picks_replace_add_and_remove_in_one_undo_step_each() {
    let mut s = session();
    let steps = s.history.undo.len();
    let r = pick(&mut s, 0.5, "replace");
    assert_eq!((r["name"].as_str(), r["selection"].as_str(), r["changed"].as_bool()), (Some("left"), Some("left"), Some(true)), "{r}");
    assert_eq!(r["hash"], format!("{:08x}", crypto_hash("left")));
    assert_eq!(s.history.undo.len(), steps + 1);
    // The highest coverage wins even when it is not rank 0; the name is quoted.
    assert_eq!(pick(&mut s, 2.5, "add")["selection"], r#"left, "Green, \"Glossy\"""#);
    // Adding it again changes nothing and leaves no undo step.
    let again = pick(&mut s, 3.5, "add");
    assert_eq!((again["changed"].as_bool(), s.history.undo.len()), (Some(false), steps + 2));
    // A `*` name and an unnamed ID go in as hashes; a tie goes to rank 0.
    pick(&mut s, 4.5, "add");
    assert_eq!(pick(&mut s, 6.5, "add")["selection"], format!(r#"left, "Green, \"Glossy\"", <{:08x}>, <{:08x}>"#, crypto_hash("star*"), crypto_hash("ghost")));
    assert_eq!(s.execute("effect.cryptomatteAt", json!({"effect": 1, "x": 7.5, "y": 0.5})).unwrap()["name"], "left");
    let r = pick(&mut s, 0.0, "remove");
    assert_eq!(r["selection"], format!(r#""Green, \"Glossy\"", <{:08x}>, <{:08x}>"#, crypto_hash("star*"), crypto_hash("ghost")));
    assert_eq!(r["selected"], false);
    // The text parses back to exactly the picked IDs.
    let manifest = aux_at(Tick::ZERO).manifests[0].1.clone();
    let got = effectcraft_effects::selection_hashes(&selection(&s), Some(&manifest));
    assert_eq!(got, [COMMA, "star*", "ghost"].map(crypto_hash));
    assert_eq!(s.history.undo.len(), steps + 5);
    s.execute("edit.undo", json!({})).unwrap();
    assert!(selection(&s).starts_with("left, "), "{}", selection(&s));
    pick(&mut s, 2.0, "replace");
    assert_eq!(selection(&s), r#""Green, \"Glossy\"""#);
    // The same selection with other spacing: no change, no undo step.
    s.execute_checked("prop.set", json!({"path": "effects/#1/selection", "value": " \"Green, \\\"Glossy\\\"\" "})).unwrap();
    let n = s.history.undo.len();
    assert_eq!((pick(&mut s, 2.0, "replace")["changed"].as_bool(), s.history.undo.len()), (Some(false), n));
}

#[test]
fn bad_points_layers_and_modes_are_errors_and_change_nothing() {
    let mut s = session();
    let at = |s: &mut Session, x: f64, y: f64| s.execute_checked("effect.pickCryptomatte", json!({"effect": 1, "x": x, "y": y}));
    for (x, y) in [(-0.1, 1.0), (8.0, 1.0), (1.0, 4.0), (1.0, 3.5), (1e300, -1e300)] {
        assert!(at(&mut s, x, y).is_err(), "{x},{y}");
        assert_eq!(s.execute("effect.cryptomatteAt", json!({"effect": 1, "x": x, "y": y})).unwrap(), Value::Null);
    }
    assert!(s.execute_checked("effect.pickCryptomatte", json!({"effect": 1, "x": 1, "y": 1, "mode": "toggle"})).is_err());
    assert!(s.execute_checked("effect.pickCryptomatte", json!({"effect": 1, "x": 1})).is_err());
    for scale in [0.0, -1.0, 2.0] {
        assert!(s.execute_checked("effect.pickCryptomatte", json!({"effect": 1, "x": 1, "y": 1, "scale": scale})).is_err(), "{scale}");
    }
    s.execute_checked("effect.apply", json!({"effect": "Gaussian Blur"})).unwrap();
    assert!(s.execute_checked("effect.pickCryptomatte", json!({"effect": 2, "x": 1, "y": 1})).is_err(), "not a Cryptomatte effect");
    assert_eq!(selection(&s), "");
    // A layer without aux channels.
    s.execute("layer.newSolid", json!({"name": "S"})).unwrap();
    s.execute_checked("effect.apply", json!({"effect": "Cryptomatte"})).unwrap();
    let e = s.execute_checked("effect.pickCryptomatte", json!({"effect": 1, "x": 1, "y": 1})).unwrap_err().to_string();
    assert!(e.contains("no Cryptomatte channels"), "{e}");
    // An expression-driven Selection would ignore the edit.
    let mut s = session();
    s.execute_checked("prop.setExpression", json!({"path": "effects/#1/selection", "expression": "\"right\""})).unwrap();
    assert!(s.execute_checked("effect.pickCryptomatte", json!({"effect": 1, "x": 1, "y": 1})).is_err());
    assert_eq!(s.execute("effect.cryptomatteAt", json!({"effect": 1, "x": 1, "y": 1})).unwrap()["selected"], false);
}

/// The pick reads the frame the render shows: Time Remap's source time, not the layer time.
#[test]
fn time_remap_picks_the_shown_source_frame() {
    let mut s = session();
    assert_eq!(pick(&mut s, 0.5, "replace")["name"], "left");
    s.execute_checked("layer.enableTimeRemap", json!({})).unwrap();
    s.execute_checked("prop.set", json!({"path": "timeRemap", "value": 1.5})).unwrap();
    assert_eq!(pick(&mut s, 0.5, "replace")["name"], "late");
}

/// A precomp of a 3D comp: the generated CryptoObject pass names its layers.
#[test]
fn picks_on_a_precomp_of_a_3d_comp() {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Outer", "width": 40, "height": 20, "frameRate": 24, "duration": 1})).unwrap();
    let l = s.execute("layer.newSolid", json!({"name": "Red, Ball", "width": 40, "height": 20})).unwrap()["layer"].clone();
    s.execute("layer.setSwitch", json!({"layers": [l], "switch": "threeD", "value": true})).unwrap();
    s.execute("layer.precompose", json!({"layers": [l], "name": "Inner"})).unwrap();
    s.execute("layer.select", json!({"layers": [s.active_comp().unwrap().layers[0].id.0]})).ok();
    s.execute_checked("effect.apply", json!({"effect": "Cryptomatte"})).unwrap();
    let r = s.execute_checked("effect.pickCryptomatte", json!({"effect": 1, "x": 20, "y": 10})).unwrap();
    assert_eq!((r["name"].as_str(), r["selection"].as_str()), (Some("Red, Ball"), Some("\"Red, Ball\"")), "{r}");
}

/// The matte the rendered Cryptomatte effect gives the picked ID at the buffer pixel under
/// `{x, y}`: the pick has to name the ID the effect's own input buffer samples there.
fn rendered_matte_of_pick(s: &mut Session, x: f64, y: f64, scale: f64) -> (String, f32) {
    let r = s.execute_checked("effect.pickCryptomatte", json!({"effect": "Cryptomatte", "x": x, "y": y, "scale": scale, "mode": "replace"})).unwrap();
    let fx = s.active_comp().unwrap().layers[0].effects().unwrap().groups().position(|g| g.match_id == "ec.3d.cryptomatte").unwrap() + 1;
    s.execute_checked("prop.set", json!({"path": format!("effects/#{fx}/matteOnly"), "value": true})).unwrap();
    let (cid, comp) = (s.active_comp_id().unwrap(), s.active_comp().unwrap());
    let layer = &comp.layers[0];
    let ctx = effectcraft_render::EvalCtx { project: &s.project, comp_id: cid, comp, time: Tick::ZERO, expr: None, footage: None };
    let buf = s.renderer(effectcraft_render::RenderOpts { scale, ..Default::default() }).content_buf(&ctx, layer).unwrap();
    let (px, py) = buf.to_px([x, y]);
    let m = buf.img.get(px.floor() as i64, py.floor() as i64)[0];
    (r["name"].as_str().unwrap_or("?").to_string(), m)
}

/// Between 3/4 and full resolution footage keeps its native pixels: at 0.8 the effect samples
/// layer pixel 2 (the comma name, 0.7) at x = 2.2, not the 0.8-grid pixel snapped back to 1.
#[test]
fn pick_at_scale_0_8_matches_the_native_buffer_the_effect_renders() {
    let mut s = session();
    let (name, m) = rendered_matte_of_pick(&mut s, 2.2, 1.5, 0.8);
    assert!(m >= 0.5, "picked {name:?}, but the render gives it matte {m} there");
    assert_eq!(name, COMMA);
}

/// A half-size proxy renders a buffer at 0.5 layer pixels per pixel even at full resolution:
/// x = 6.5 lies in buffer pixel 3, which samples layer x = 7 (the `left` / `right` tie).
#[test]
fn pick_on_a_proxy_matches_the_proxy_sized_buffer() {
    let mut s = session();
    let item = *s.project.items.iter().find(|(_, i)| matches!(i.kind, effectcraft_project::ItemKind::Footage(_))).unwrap().0;
    s.execute("file.setProxy", json!({"item": item.0, "path": "/renders/crypto_proxy.exr"})).unwrap();
    let (name, m) = rendered_matte_of_pick(&mut s, 6.5, 1.5, 1.0);
    assert!(m >= 0.5, "picked {name:?}, but the render gives it matte {m} there");
    assert_eq!(name, "left");
}

/// 1920-px footage with `left` up to x = 120 and `right` after it, and a 104-px proxy.
struct Edge;
impl FootageSource for Edge {
    fn frame(&self, _: ItemId, f: &Footage, _: Tick) -> Option<Arc<Image>> {
        Some(Arc::new(Image::new(f.width, f.height)))
    }
    fn aux(&self, _: ItemId, _: &Footage, _: Tick) -> Option<Arc<AuxChannels>> {
        let (w, h) = (1920usize, 40usize);
        let mut aux = AuxChannels::new(w as u32, h as u32, 1.0);
        let ids = (0..w * h).map(|i| crypto_float(crypto_hash(if i % w < 120 { "left" } else { "right" }))).collect();
        aux.channels = vec![("CryptoObject00.R".into(), ids), ("CryptoObject00.G".into(), vec![1.0; w * h])];
        aux.manifests = vec![("CryptoObject".into(), ["left", "right"].iter().map(|n| (n.to_string(), crypto_hash(n))).collect())];
        Some(Arc::new(aux))
    }
}

/// Proxy buffer pixel 6's centre maps to layer x = 6.5 / (104 / 1920): 120.0 by division, but
/// 119.99999999999999 by the reciprocal the effect multiplies with. The pick must agree.
#[test]
fn pick_maps_pixel_centres_bit_exactly_as_the_effect() {
    let mut s = Session { footage: Arc::new(Edge), ..Default::default() };
    let at = |path: &str, width| Footage {
        path: path.into(),
        kind: FootageKind::Video,
        width,
        height: 40,
        has_video: true,
        frame_rate: FrameRate::from_f64(24.0),
        duration: Tick::from_seconds_f64(2.0),
        ..Default::default()
    };
    s.importer = Some(Arc::new(EdgeProbe(at("/renders/edge.exr", 1920), at("/renders/edge_proxy.exr", 104))));
    let item = s.execute("file.import", json!({"paths": ["/renders/edge.exr"]})).unwrap()["items"][0].clone();
    s.execute("comp.new", json!({"name": "C", "width": 1920, "height": 40, "frameRate": 24, "duration": 2})).unwrap();
    s.execute("layer.addItem", json!({"item": item})).unwrap();
    s.execute_checked("effect.apply", json!({"effect": "Cryptomatte"})).unwrap();
    s.execute("file.setProxy", json!({"item": item, "path": "/renders/edge_proxy.exr"})).unwrap();
    let (name, m) = rendered_matte_of_pick(&mut s, 118.0, 5.0, 1.0);
    assert_eq!((name.as_str(), m), ("left", 1.0));
}

struct EdgeProbe(Footage, Footage);
impl crate::Importer for EdgeProbe {
    fn probe(&self, path: &str) -> Result<Footage, String> {
        Ok(if path.contains("proxy") { self.1.clone() } else { self.0.clone() })
    }
}

/// Footage whose decoded frames are `num / den` of the probed size (a file re-rendered at another
/// resolution, a relinked file): the render stretches them over the layer.
struct OffNominal(u32, u32);
impl FootageSource for OffNominal {
    fn frame(&self, _: ItemId, f: &Footage, _: Tick) -> Option<Arc<Image>> {
        Some(Arc::new(Image::new(f.width * self.0 / self.1, f.height * self.0 / self.1)))
    }
    fn aux(&self, _: ItemId, _: &Footage, t: Tick) -> Option<Arc<AuxChannels>> {
        Some(Arc::new(aux_at(t)))
    }
}

/// Points along rows `ys` where the picked ID isn't lit in the rendered matte under the point.
fn mismatches(s: &mut Session, scales: &[f64], xs: impl Iterator<Item = f64> + Clone, ys: &[f64]) -> Vec<String> {
    let mut bad = vec![];
    for &scale in scales {
        for (x, &y) in ys.iter().flat_map(|y| xs.clone().map(move |x| (x, y))) {
            if s.execute("effect.cryptomatteAt", json!({"effect": "Cryptomatte", "x": x, "y": y, "scale": scale})).unwrap().is_null() {
                continue;
            }
            let (name, m) = rendered_matte_of_pick(s, x, y, scale);
            if m < 0.5 {
                bad.push(format!("scale {scale} ({x:.2}, {y}): picked {name:?}, matte {m}"));
            }
        }
    }
    bad
}

const PREVIEW_SCALES: [f64; 7] = [1.0, 0.8, 0.75, 0.74, 0.5, 0.33, 0.25];

/// The pick follows the decoded frame's real size, not the footage's nominal one.
#[test]
fn pick_matches_the_render_when_frames_decode_off_nominal_size() {
    for (num, den) in [(2, 1), (1, 2), (3, 4)] {
        let mut s = Session { footage: Arc::new(OffNominal(num, den)), importer: Some(Arc::new(Probe)), ..Default::default() };
        let item = s.execute("file.import", json!({"paths": ["/renders/crypto.exr"]})).unwrap()["items"][0].clone();
        s.execute("comp.new", json!({"name": "C", "width": 8, "height": 4, "frameRate": 24, "duration": 2})).unwrap();
        s.execute("layer.addItem", json!({"item": item})).unwrap();
        s.execute_checked("effect.apply", json!({"effect": "Cryptomatte"})).unwrap();
        let bad = mismatches(&mut s, &PREVIEW_SCALES, (0..80).map(|i| i as f64 * 0.1 + 0.05), &[0.3, 1.7, 2.6]);
        assert!(bad.is_empty(), "decode {num}/{den}: {} mismatches:\n{}", bad.len(), bad.join("\n"));
    }
}

/// A precomp of a 3D comp, with and without Preserve Resolution (drawn at full size below full
/// resolution), behind a disabled effect: the pick names the ID the rendered matte shows.
#[test]
fn pick_matches_the_render_on_precomps_with_and_without_preserve_resolution() {
    for preserve in [false, true] {
        let mut s = Session::default();
        s.execute("comp.new", json!({"name": "Outer", "width": 40, "height": 20, "frameRate": 24, "duration": 2})).unwrap();
        let a = s.execute("layer.newSolid", json!({"name": "A", "width": 40, "height": 20})).unwrap()["layer"].clone();
        let b = s.execute("layer.newSolid", json!({"name": "B", "width": 7, "height": 20})).unwrap()["layer"].clone();
        s.execute_checked("prop.set", json!({"layer": b, "path": "transform/position", "value": [3.5, 10, -10]})).unwrap();
        s.execute("layer.setSwitch", json!({"layers": [a, b], "switch": "threeD", "value": true})).unwrap();
        s.execute("layer.precompose", json!({"layers": [a, b], "name": "Inner"})).unwrap();
        let inner = *s.project.items.iter().find(|(_, i)| i.name == "Inner").unwrap().0;
        s.execute_checked("comp.settings", json!({"comp": inner.0, "preserveResolution": preserve})).unwrap();
        s.execute("layer.select", json!({"layers": [s.active_comp().unwrap().layers[0].id.0]})).ok();
        s.execute_checked("effect.apply", json!({"effect": "Gaussian Blur"})).unwrap();
        s.execute_checked("effect.toggle", json!({"effect": 1, "value": false})).unwrap();
        s.execute_checked("effect.apply", json!({"effect": "Cryptomatte"})).unwrap();
        let bad = mismatches(&mut s, &PREVIEW_SCALES, (0..93).map(|i| i as f64 * 0.15 + 0.05), &[10.0]);
        assert!(bad.is_empty(), "preserve {preserve}: {} mismatches:\n{}", bad.len(), bad.join("\n"));
    }
}
