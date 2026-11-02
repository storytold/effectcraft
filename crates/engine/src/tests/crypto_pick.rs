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
    let ctx = effectcraft_render::EvalCtx {
        project: &s.project,
        comp_id: cid,
        comp,
        time: Tick::ZERO,
        expr: None,
        footage: None,
        property_overrides: &[],
        media_overrides: &[],
    };
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

/// `w` × `h` footage whose Cryptomatte has an ID per 37 × 29 px cell (`c<col>_<row>`, all in the
/// manifest), one aux pixel per layer pixel. Built once, as the media pool keeps decoded aux.
struct Cells(u32, u32, std::sync::OnceLock<Arc<AuxChannels>>);
impl Cells {
    fn name(x: usize, y: usize) -> String {
        format!("c{}_{}", x / 37, y / 29)
    }
}
impl FootageSource for Cells {
    fn frame(&self, _: ItemId, f: &Footage, _: Tick) -> Option<Arc<Image>> {
        Some(Arc::new(Image::new(f.width, f.height)))
    }
    fn aux(&self, _: ItemId, _: &Footage, _: Tick) -> Option<Arc<AuxChannels>> {
        Some(self.2.get_or_init(|| Arc::new(self.build())).clone())
    }
}
impl Cells {
    fn build(&self) -> AuxChannels {
        let (w, h) = (self.0 as usize, self.1 as usize);
        let ids = (0..w * h).map(|i| crypto_float(crypto_hash(&Cells::name(i % w, i / w)))).collect();
        let mut aux = AuxChannels::new(w as u32, h as u32, 1.0);
        aux.channels = vec![("CryptoObject00.R".into(), ids), ("CryptoObject00.G".into(), vec![1.0; w * h])];
        let names = (0..w.div_ceil(37)).flat_map(|x| (0..h.div_ceil(29)).map(move |y| Cells::name(x * 37, y * 29)));
        aux.manifests = vec![("CryptoObject".into(), names.map(|n| (n.clone(), crypto_hash(&n))).collect())];
        aux
    }
}

struct CellsProbe(u32, u32);
impl crate::Importer for CellsProbe {
    fn probe(&self, path: &str) -> Result<Footage, String> {
        // `…_proxy` is a half-size proxy (rounded up); `…still…` a still image.
        let (width, height) = if path.contains("proxy") { (self.0.div_ceil(2), self.1.div_ceil(2)) } else { (self.0, self.1) };
        let (frame_rate, duration) = (FrameRate::from_f64(24.0), Tick::from_seconds_f64(2.0));
        let kind = if path.contains("still") { FootageKind::Still } else { FootageKind::Video };
        Ok(Footage { path: path.into(), kind, width, height, has_video: true, frame_rate, duration, ..Default::default() })
    }
}

fn cells_session(w: u32, h: u32, blur: bool) -> Session {
    cells_session_at(w, h, blur, "/renders/cells.exr")
}

fn cells_session_at(w: u32, h: u32, blur: bool, path: &str) -> Session {
    let mut s = Session { footage: Arc::new(Cells(w, h, Default::default())), importer: Some(Arc::new(CellsProbe(w, h))), ..Default::default() };
    let item = s.execute("file.import", json!({"paths": [path]})).unwrap()["items"][0].clone();
    s.execute("comp.new", json!({"name": "C", "width": w, "height": h, "frameRate": 24, "duration": 2})).unwrap();
    s.execute("layer.addItem", json!({"item": item})).unwrap();
    if blur {
        s.execute_checked("effect.apply", json!({"effect": "Gaussian Blur"})).unwrap();
        s.execute_checked("prop.set", json!({"path": "effects/#1/blurriness", "value": 40.0})).unwrap();
    }
    s.execute_checked("effect.apply", json!({"effect": "Cryptomatte"})).unwrap();
    s
}

/// Hover latency of `effect.cryptomatteAt` (dev profile): the first call after the viewer's frame
/// and the next ones, on footage (1080p, 4K, 4K behind a blur) and a 1080p 3D precomp. Run with
/// `--ignored --nocapture`; it measures, it doesn't assert.
#[test]
#[ignore]
fn hover_latency() {
    let mut cases: Vec<(&str, Session)> = vec![
        ("footage 1920x1080", cells_session(1920, 1080, false)),
        ("footage 3840x2160", cells_session(3840, 2160, false)),
        ("footage 3840x2160 + blur 40", cells_session(3840, 2160, true)),
    ];
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Outer", "width": 1920, "height": 1080, "frameRate": 24, "duration": 2})).unwrap();
    let a = s.execute("layer.newSolid", json!({"name": "A", "width": 1920, "height": 1080})).unwrap()["layer"].clone();
    let b = s.execute("layer.newSolid", json!({"name": "B", "width": 400, "height": 1080})).unwrap()["layer"].clone();
    s.execute("layer.setSwitch", json!({"layers": [a, b], "switch": "threeD", "value": true})).unwrap();
    s.execute("layer.precompose", json!({"layers": [a, b], "name": "Inner"})).unwrap();
    s.execute("layer.select", json!({"layers": [s.active_comp().unwrap().layers[0].id.0]})).ok();
    s.execute_checked("effect.apply", json!({"effect": "Cryptomatte"})).unwrap();
    cases.push(("3D precomp 1920x1080", s));
    for (name, mut s) in cases {
        let cid = s.active_comp_id().unwrap();
        let _ = s.render(cid, Tick::ZERO, effectcraft_render::RenderOpts::default());
        let ms: Vec<String> = (0..6)
            .map(|i| {
                let t0 = std::time::Instant::now();
                let r = s.execute("effect.cryptomatteAt", json!({"effect": "Cryptomatte", "x": 100.0 + 40.0 * i as f64, "y": 500.0})).unwrap();
                assert!(!r.is_null(), "{name}");
                format!("{:.3}", t0.elapsed().as_secs_f64() * 1e3)
            })
            .collect();
        eprintln!("HOVER {name}: first {} ms, then {} ms", ms[0], ms[1..].join(" / "));
    }
}

/// The hover's probe: built off the thread that asks, kept while only the Selection changes,
/// rebuilt when anything it reads changes, and a job finished for an older state is not used.
#[test]
fn hover_probe_is_built_by_a_job_kept_across_picks_and_dropped_on_other_edits() {
    let mut s = session();
    let q = json!({"effect": 1, "x": 0.5, "y": 1.5});
    let job = s.crypto_probe_job(&q).unwrap().expect("nothing built yet");
    std::thread::spawn(job).join().unwrap();
    assert!(s.crypto_probe_job(&q).unwrap().is_none(), "built");
    assert_eq!(s.execute("effect.cryptomatteAt", q.clone()).unwrap()["name"], "left");
    // Another scale or time is another probe.
    assert!(s.crypto_probe_job(&json!({"effect": 1, "x": 0.5, "y": 1.5, "scale": 0.5})).unwrap().is_some());
    assert!(s.crypto_probe_job(&json!({"effect": 1, "x": 0.5, "y": 1.5, "time": 1.0})).unwrap().is_some());
    // A pick edits only the Selection: the probe stays.
    pick(&mut s, 0.5, "replace");
    assert!(s.crypto_probe_job(&q).unwrap().is_none(), "kept across the pick");
    assert_eq!(s.execute("effect.cryptomatteAt", q.clone()).unwrap()["selected"], true);
    // Other edits rebuild it, and the answer follows them.
    s.execute_checked("layer.enableTimeRemap", json!({})).unwrap();
    s.execute_checked("prop.set", json!({"path": "timeRemap", "value": 1.5})).unwrap();
    let stale = s.crypto_probe_job(&q).unwrap().expect("time remap changed it");
    assert_eq!(s.execute("effect.cryptomatteAt", q.clone()).unwrap()["name"], "late");
    // A job that finishes after the next edit doesn't answer for it.
    s.execute_checked("prop.set", json!({"path": "timeRemap", "value": 0.5})).unwrap();
    stale();
    assert!(s.crypto_probe_job(&q).unwrap().is_some(), "stale job result is not used");
    assert_eq!(s.execute("effect.cryptomatteAt", q.clone()).unwrap()["name"], "left");
    // Errors are kept too, and still errors.
    s.execute("layer.newSolid", json!({"name": "S"})).unwrap();
    s.execute_checked("effect.apply", json!({"effect": "Cryptomatte"})).unwrap();
    let solid = json!({"effect": 1, "x": 1, "y": 1});
    s.crypto_probe_job(&solid).unwrap().expect("solid")();
    assert!(s.crypto_probe_job(&solid).unwrap().is_none());
    assert!(s.execute("effect.cryptomatteAt", solid).unwrap_err().to_string().contains("no Cryptomatte channels"));
}

/// Picks on 1080p, 4K and odd-sized footage, at the viewer's resolutions and on a half-size proxy,
/// name the ID the rendered matte shows under the point (points next to cell edges).
#[test]
fn picks_match_the_render_at_1080p_4k_odd_sizes_and_on_proxies() {
    for (w, h) in [(1920, 1080), (3840, 2160), (1919, 1079)] {
        let mut s = cells_session(w, h, false);
        let (wl, hl) = (f64::from(w), f64::from(h));
        let xs = [36.6, 37.2, 0.4, wl / 2.0 + 0.3, wl - 0.6, wl - 37.4];
        let ys = [28.7, 29.1, hl - 0.5, hl / 3.0];
        let mut bad = mismatches(&mut s, &[1.0, 0.5, 0.33, 0.25], xs.iter().copied(), &ys);
        let item = *s.project.items.iter().find(|(_, i)| matches!(i.kind, effectcraft_project::ItemKind::Footage(_))).unwrap().0;
        s.execute("file.setProxy", json!({"item": item.0, "path": "/renders/cells_proxy.exr"})).unwrap();
        bad.extend(mismatches(&mut s, &[1.0, 0.5, 0.33, 0.25], xs.iter().copied(), &ys).into_iter().map(|b| format!("proxy {b}")));
        assert!(bad.is_empty(), "{w}x{h}: {} mismatches:\n{}", bad.len(), bad.join("\n"));
    }
}

/// The kept probe, and the aux channels it holds, go when the eyedropper is disarmed, when its
/// layer or effect is deleted, on Open / New, and when its footage changes.
#[test]
fn hover_probe_is_released_on_disarm_delete_open_and_footage_changes() {
    let q = json!({"effect": "Cryptomatte", "x": 100.0, "y": 100.0});
    let ends: [(&str, fn(&mut Session)); 5] = [
        ("disarm", |s| s.drop_crypto_probe()),
        ("layer delete", |s| {
            let lid = s.active_comp().unwrap().layers[0].id.0;
            s.execute_checked("edit.clear", json!({"layers": [lid]})).unwrap();
        }),
        ("effect delete", |s| {
            s.execute_checked("effect.remove", json!({"effect": "Cryptomatte"})).unwrap();
        }),
        ("open / new", |s| s.replace_project(effectcraft_project::Project::default(), None)),
        ("footage change", |s| {
            let f = s.project.items.values().find_map(|i| match &i.kind {
                effectcraft_project::ItemKind::Footage(f) => Some(f.clone()),
                _ => None,
            });
            s.forget_decoded(&f.unwrap());
        }),
    ];
    for (what, end) in ends {
        let mut s = cells_session(640, 360, false);
        let aux = s.footage.aux(ItemId(0), &Footage::default(), Tick::ZERO).unwrap();
        let unpinned = Arc::strong_count(&aux);
        assert!(!s.execute("effect.cryptomatteAt", q.clone()).unwrap().is_null());
        assert!(s.crypto_probe_stats().0 && Arc::strong_count(&aux) > unpinned, "{what}: kept");
        end(&mut s);
        assert!(!s.crypto_probe_stats().0, "{what}: the probe is gone");
        assert_eq!(Arc::strong_count(&aux), unpinned, "{what}: its aux channels are let go");
    }
}

/// A probe is keyed by the frame it reads: on a still, or footage held by Time Remap, every comp
/// time shares one; each footage frame has its own, and so does each comp time once something
/// before the Cryptomatte effect is animated.
#[test]
fn hover_probe_is_keyed_by_the_source_frame() {
    let builds = |s: &mut Session, times: &[f64]| -> Vec<u64> {
        let b = s.crypto_probe_stats().1;
        times
            .iter()
            .map(|t| {
                let r = s.execute("effect.cryptomatteAt", json!({"effect": "Cryptomatte", "x": 40.0, "y": 30.0, "time": t})).unwrap();
                assert_eq!(r["name"], "c1_1");
                s.crypto_probe_stats().1 - b
            })
            .collect()
    };
    let f = 1.0 / 24.0;
    let mut s = cells_session_at(320, 180, false, "/renders/still.exr");
    assert_eq!(builds(&mut s, &[0.0, f, 0.5, 1.9]), [1, 1, 1, 1], "a still: one probe");
    let mut s = cells_session(320, 180, false);
    assert_eq!(builds(&mut s, &[0.0, f * 0.4, f, f * 1.5, 0.0]), [1, 1, 2, 2, 3], "footage: a probe per frame");
    s.execute_checked("layer.enableTimeRemap", json!({})).unwrap();
    s.execute_checked("prop.toggleAnimation", json!({"path": "timeRemap", "value": false})).unwrap();
    s.execute_checked("prop.set", json!({"path": "timeRemap", "value": 0.5})).unwrap();
    assert_eq!(builds(&mut s, &[0.0, f, 1.2]), [1, 1, 1], "held by Time Remap: one source frame");
    let mut s = cells_session_at(320, 180, true, "/renders/still.exr");
    assert_eq!(builds(&mut s, &[0.0, f]), [1, 1], "a still blur");
    s.execute_checked("prop.addKey", json!({"path": "effects/#1/blurriness", "time": 0.0, "value": 2.0})).unwrap();
    s.execute_checked("prop.addKey", json!({"path": "effects/#1/blurriness", "time": 1.0, "value": 6.0})).unwrap();
    assert_eq!(builds(&mut s, &[0.0, f, f]), [1, 2, 2], "an animated blur before it: per comp time");
}

/// Probes of two Cryptomatte effects (on two footage layers) at the same revision and frame
/// don't answer for each other.
#[test]
fn hover_probes_are_per_layer_and_effect() {
    /// `…late…` files have the IDs [`Crypto`] has from 1 s.
    struct ByFile;
    impl FootageSource for ByFile {
        fn frame(&self, _: ItemId, f: &Footage, _: Tick) -> Option<Arc<Image>> {
            Some(Arc::new(Image::new(f.width, f.height)))
        }
        fn aux(&self, _: ItemId, f: &Footage, t: Tick) -> Option<Arc<AuxChannels>> {
            Some(Arc::new(aux_at(if f.path.contains("late") { Tick::from_seconds_f64(1.0) } else { t })))
        }
    }
    let mut s = Session { footage: Arc::new(ByFile), importer: Some(Arc::new(Probe)), ..Default::default() };
    let items = s.execute("file.import", json!({"paths": ["/renders/crypto.exr", "/renders/late.exr"]})).unwrap()["items"].clone();
    s.execute("comp.new", json!({"name": "C", "width": 8, "height": 4, "frameRate": 24, "duration": 2})).unwrap();
    let layers: Vec<Value> = (0..2).map(|i| s.execute("layer.addItem", json!({"item": items[i]})).unwrap()["layer"].clone()).collect();
    for l in &layers {
        s.execute_checked("effect.apply", json!({"layers": [l], "effect": "Cryptomatte"})).unwrap();
    }
    let on = |l: &Value| json!({"layer": l, "effect": 1, "x": 0.5, "y": 1.5});
    for (l, want) in [(&layers[0], "left"), (&layers[1], "late"), (&layers[0], "left")] {
        assert_eq!(s.execute("effect.cryptomatteAt", on(l)).unwrap()["name"], want);
    }
}

/// [`Crypto`]'s IDs, counting the reads, and holding them while `hold` is set (a build caught
/// in the middle).
#[derive(Default)]
struct Gate {
    reads: std::sync::atomic::AtomicUsize,
    hold: std::sync::atomic::AtomicBool,
    entered: std::sync::atomic::AtomicBool,
    /// Picture frames read (the effect's input render).
    frames: std::sync::atomic::AtomicUsize,
}
impl FootageSource for Gate {
    fn frame(&self, _: ItemId, f: &Footage, _: Tick) -> Option<Arc<Image>> {
        self.frames.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Some(Arc::new(Image::new(f.width, f.height)))
    }
    fn aux(&self, _: ItemId, _: &Footage, t: Tick) -> Option<Arc<AuxChannels>> {
        use std::sync::atomic::Ordering::SeqCst;
        self.reads.fetch_add(1, SeqCst);
        self.entered.store(true, SeqCst);
        while self.hold.load(SeqCst) {
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        Some(Arc::new(aux_at(t)))
    }
}

/// Only the newest request's build is kept: a job that finishes late doesn't evict a newer probe,
/// a job superseded before it runs doesn't build, one superseded (or outdated by an edit) while
/// it builds stores nothing, and `probeOnly` never builds on the asking thread.
#[test]
fn hover_probe_builds_store_only_the_newest_and_never_build_for_probe_only() {
    use std::sync::atomic::Ordering::SeqCst;
    let gate = Arc::new(Gate::default());
    let mut s = session();
    s.footage = gate.clone();
    let at = |t: f64| json!({"effect": 1, "x": 0.5, "y": 1.5, "time": t});
    let reads = || gate.reads.load(SeqCst);
    // A late job for another frame doesn't evict the probe built since.
    let job0 = s.crypto_probe_job(&at(0.0)).unwrap().unwrap();
    s.execute("effect.cryptomatteAt", at(1.0)).unwrap();
    assert!(s.crypto_probe_job(&at(1.0)).unwrap().is_none());
    job0();
    assert!(s.crypto_probe_job(&at(1.0)).unwrap().is_none(), "the late job evicted the newer probe");
    // A request for another probe cancels the earlier job: it doesn't even build.
    let (old, new) = (s.crypto_probe_job(&at(0.5)).unwrap().unwrap(), s.crypto_probe_job(&at(0.25)).unwrap().unwrap());
    let before = reads();
    old();
    assert_eq!(reads(), before, "a superseded job doesn't build");
    new();
    assert!(reads() > before);
    assert!(s.crypto_probe_job(&at(0.25)).unwrap().is_none(), "the newest request's probe is kept");
    // The same request again doesn't cancel the job in flight.
    let (a, b) = (s.crypto_probe_job(&at(1.5)).unwrap().unwrap(), s.crypto_probe_job(&at(1.5)).unwrap().unwrap());
    a();
    drop(b);
    assert!(s.crypto_probe_job(&at(1.5)).unwrap().is_none());
    // Superseded, or outdated by an edit, while it builds: it stores nothing.
    for edit in [false, true] {
        let job = s.crypto_probe_job(&at(0.75)).unwrap().unwrap();
        gate.hold.store(true, SeqCst);
        gate.entered.store(false, SeqCst);
        let run = std::thread::spawn(job);
        while !gate.entered.load(SeqCst) {
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        if edit {
            s.execute_checked("prop.set", json!({"path": "effects/#1/matteOnly", "value": true})).unwrap();
        } else {
            drop(s.crypto_probe_job(&at(0.8)).unwrap());
        }
        gate.hold.store(false, SeqCst);
        run.join().unwrap();
        let stored = if edit { s.crypto_probe_stats().0 } else { s.crypto_probe_job(&at(0.75)).unwrap().is_none() };
        assert!(!stored, "edit {edit}: the outdated build was stored");
    }
    // `probeOnly`: pending without a probe, and nothing built.
    let before = reads();
    let mut q = at(0.0);
    q["probeOnly"] = json!(true);
    assert_eq!(s.execute("effect.cryptomatteAt", q.clone()).unwrap(), json!({"pending": true}));
    q["mode"] = json!("add");
    assert_eq!(s.execute("effect.pickCryptomatte", q.clone()).unwrap()["pending"], true);
    assert_eq!(reads(), before, "nothing built for probeOnly");
    s.crypto_probe_job(&q).unwrap().unwrap()();
    assert_eq!(s.execute("effect.pickCryptomatte", q.clone()).unwrap()["selection"], "left");
}

/// A build cancelled while it reads the IDs (a newer request, disarming, an edit, another
/// project) stops before it renders the effect's input, so the next build isn't kept waiting.
#[test]
fn a_cancelled_build_stops_before_rendering_its_input() {
    use std::sync::atomic::Ordering::SeqCst;
    let cancels: [(&str, fn(&mut Session)); 4] = [
        ("newer request", |s| drop(s.crypto_probe_job(&json!({"effect": 1, "x": 0.5, "y": 1.5, "time": 1.5})).unwrap())),
        ("disarm", |s| s.drop_crypto_probe()),
        ("edit", |s| drop(s.execute_checked("prop.set", json!({"path": "effects/#1/matteOnly", "value": true})).unwrap())),
        ("project switch", |s| s.replace_project(effectcraft_project::Project::default(), None)),
    ];
    for (what, cancel) in [("not cancelled", None), ("newer request", Some(0)), ("disarm", Some(1)), ("edit", Some(2)), ("project switch", Some(3))] {
        let gate = Arc::new(Gate::default());
        let mut s = session();
        s.footage = gate.clone();
        let job = s.crypto_probe_job(&json!({"effect": 1, "x": 0.5, "y": 1.5, "time": 0.5})).unwrap().unwrap();
        gate.hold.store(true, SeqCst);
        let run = std::thread::spawn(job);
        while !gate.entered.load(SeqCst) {
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        if let Some(k) = cancel {
            cancels[k].1(&mut s);
        }
        gate.hold.store(false, SeqCst);
        run.join().unwrap();
        let rendered = gate.frames.load(SeqCst) > 0;
        assert_eq!(rendered, cancel.is_none(), "{what}: the input was rendered: {rendered}");
    }
}

/// The probe doesn't read the Cryptomatte effect's own Selection: keyframed, a still keeps one
/// probe for every comp time (an animated effect before it still makes one per time).
#[test]
fn an_animated_selection_keeps_one_probe_on_a_still() {
    let mut s = cells_session_at(320, 180, false, "/renders/still.exr");
    for (t, v) in [(0.0, "c1_1"), (1.0, "c2_2")] {
        s.execute_checked("prop.addKey", json!({"path": "effects/#1/selection", "time": t, "value": v})).unwrap();
    }
    let b = s.crypto_probe_stats().1;
    for t in [0.0, 1.0 / 24.0, 0.5, 1.5] {
        assert_eq!(s.execute("effect.cryptomatteAt", json!({"effect": "Cryptomatte", "x": 40.0, "y": 30.0, "time": t})).unwrap()["name"], "c1_1");
    }
    assert_eq!(s.crypto_probe_stats().1 - b, 1, "one probe for the still");
}

/// A panic while a probe is built on another thread becomes the probe's error: the job returns,
/// the hover reports it, and it isn't retried for that frame.
#[test]
fn a_panicking_probe_build_becomes_an_error() {
    struct Broken;
    impl FootageSource for Broken {
        fn frame(&self, _: ItemId, f: &Footage, _: Tick) -> Option<Arc<Image>> {
            Some(Arc::new(Image::new(f.width, f.height)))
        }
        fn aux(&self, _: ItemId, _: &Footage, _: Tick) -> Option<Arc<AuxChannels>> {
            panic!("a broken EXR reader")
        }
    }
    let mut s = Session { footage: Arc::new(Broken), importer: Some(Arc::new(Probe)), ..Default::default() };
    let item = s.execute("file.import", json!({"paths": ["/renders/crypto.exr"]})).unwrap()["items"][0].clone();
    s.execute("comp.new", json!({"name": "C", "width": 8, "height": 4, "frameRate": 24, "duration": 2})).unwrap();
    s.execute("layer.addItem", json!({"item": item})).unwrap();
    s.execute_checked("effect.apply", json!({"effect": "Cryptomatte"})).unwrap();
    let q = json!({"effect": 1, "x": 0.5, "y": 1.5, "probeOnly": true});
    let job = s.crypto_probe_job(&q).unwrap().unwrap();
    assert!(std::thread::spawn(job).join().is_ok(), "the job caught the panic");
    assert!(s.crypto_probe_job(&q).unwrap().is_none(), "the error is kept");
    let e = s.execute("effect.cryptomatteAt", q).unwrap_err().to_string();
    assert!(e.contains("Reading the Cryptomatte IDs failed") && e.contains("a broken EXR reader"), "{e}");
}

/// Asking whether the probe is built changes nothing: asked for other frames while a build runs
/// (the eyedropper's hover while a click waits), it doesn't cancel that build.
#[test]
fn asking_whether_a_probe_is_ready_cancels_no_build() {
    let s = session();
    let at = |t: f64| json!({"effect": 1, "x": 0.5, "y": 1.5, "time": t, "probeOnly": true});
    let job = s.crypto_probe_job(&at(0.0)).unwrap().unwrap();
    for t in [1.0, 0.5, 1.0, 0.25] {
        assert!(!s.crypto_probe_ready(&at(t)).unwrap());
    }
    job();
    assert!(s.crypto_probe_ready(&at(0.0)).unwrap(), "the build was cancelled");
    assert!(s.crypto_probe_job(&at(0.0)).unwrap().is_none());
}

/// The probe renders with the render prefs (nested switches, draft shadows): changing them asks
/// for a new one.
#[test]
fn a_render_prefs_change_asks_for_a_new_probe() {
    let mut s = session();
    let q = json!({"effect": 1, "x": 0.5, "y": 1.5});
    s.crypto_probe_job(&q).unwrap().unwrap()();
    assert!(s.crypto_probe_ready(&q).unwrap());
    s.prefs.three_d.realtime_shadows = !s.prefs.three_d.realtime_shadows;
    assert!(!s.crypto_probe_ready(&q).unwrap(), "draft shadows changed");
    s.crypto_probe_job(&q).unwrap().unwrap()();
    s.prefs.general.switches_affect_nested_comps = !s.prefs.general.switches_affect_nested_comps;
    assert!(!s.crypto_probe_ready(&q).unwrap(), "nested switches changed");
}

/// A pick is an undo step named for it, and undoing, redoing or jumping over picks keeps the
/// probe (only the Selection changed); undoing any other edit doesn't.
#[test]
fn undoing_picks_keeps_the_probe_and_the_step_says_pick() {
    let mut s = session();
    let q = json!({"effect": 1, "x": 0.5, "y": 1.5, "probeOnly": true});
    s.crypto_probe_job(&q).unwrap().unwrap()();
    let before = crate::history::state_id(&s.project);
    pick(&mut s, 0.5, "replace");
    assert_eq!(s.history.undo.last().unwrap().0, "Pick Cryptomatte");
    pick(&mut s, 4.5, "add");
    for (what, cmd) in [("undo", "edit.undo"), ("undo", "edit.undo"), ("redo", "edit.redo")] {
        s.execute_checked(cmd, json!({})).unwrap();
        assert!(s.crypto_probe_ready(&q).unwrap(), "{what} {}: probe dropped", selection(&s));
    }
    assert_eq!(s.execute("effect.cryptomatteAt", q.clone()).unwrap()["selected"], true);
    assert!(s.goto_history(&before) && selection(&s).is_empty());
    assert!(s.crypto_probe_ready(&q).unwrap(), "history jump: probe dropped");
    s.execute_checked("prop.set", json!({"path": "transform/opacity", "value": 50})).unwrap();
    s.crypto_probe_job(&q).unwrap().unwrap()();
    s.execute_checked("edit.undo", json!({})).unwrap();
    assert!(!s.crypto_probe_ready(&q).unwrap(), "an undone opacity change needs a new probe");
}
