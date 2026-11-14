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
    // A second view's layer of the same kind (`Z.CryptoObject`, stored Layer 3): "right" everywhere.
    aux.channels.push(("Z.CryptoObject00.R".into(), vec![crypto_float(crypto_hash("right")); w * h]));
    aux.channels.push(("Z.CryptoObject00.G".into(), vec![1.0; w * h]));
    aux.manifests.push(("Z.CryptoObject".into(), vec![("right".into(), crypto_hash("right"))]));
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
    session_with(Arc::new(Crypto))
}

fn session_with(footage: Arc<dyn FootageSource>) -> Session {
    let mut s = Session { footage, importer: Some(Arc::new(Probe)), ..Default::default() };
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

/// A second layer that fits a standard slot (`Z.CryptoObject` beside `CryptoObject`) is picked by
/// its own Layer value (3); Layer 0 still reads the first one. The agent commands follow that
/// Layer value too (#3020): ids/coverage/pick-by-name read `Z.CryptoObject`, and unknown-name
/// errors name it.
#[test]
fn pick_reads_the_second_layer_of_a_kind() {
    let mut s = session();
    assert_eq!(pick(&mut s, 1.0, "replace")["name"], "left");
    s.execute_checked("prop.set", json!({"path": "effects/#1/layer", "value": 3})).unwrap();
    assert_eq!(pick(&mut s, 1.0, "replace")["name"], "right");
    assert_eq!(s.execute("effect.cryptomatteAt", json!({"effect": 1, "x": 1.0, "y": 1.5})).unwrap()["name"], "right");
    let r = s.execute_checked("effect.cryptomatteIds", json!({"effect": 1})).unwrap();
    assert_eq!(r["layer"].as_str(), Some("Z.CryptoObject"), "{r}");
    assert_eq!(ids(&r), [json!("right")]);
    let cov = s.execute_checked("effect.cryptomatteCoverage", json!({"effect": 1, "name": "right"})).unwrap();
    assert!((cov["coverage"].as_f64().unwrap() - 1.0).abs() < 1e-9, "{cov}");
    assert_eq!(s.execute_checked("effect.pickCryptomatte", json!({"effect": 1, "name": "right"})).unwrap()["name"], "right");
    let e = s.execute_checked("effect.cryptomatteCoverage", json!({"effect": 1, "name": "left"})).unwrap_err().to_string();
    assert!(e.contains("in Z.CryptoObject"), "{e}");
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
        reads: None,
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
        // The highlight of the hovered ID (built once per ID, at most 1024 samples wide).
        let q = json!({"effect": "Cryptomatte", "x": 100.0, "y": 500.0});
        let hash = u32::from_str_radix(s.execute("effect.cryptomatteAt", q.clone()).unwrap()["hash"].as_str().unwrap(), 16).unwrap();
        let t0 = std::time::Instant::now();
        let c = s.crypto_coverage(&q, hash, 1024).unwrap();
        let cov = t0.elapsed().as_secs_f64() * 1e3;
        eprintln!("HOVER {name}: first {} ms, then {} ms; highlight {}x{} {cov:.2} ms", ms[0], ms[1..].join(" / "), c.width, c.height);
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

/// The hover highlight: an ID's coverage from the kept probe, one sample at the centre of each
/// cell of aux pixels, over the layer's extent (the last cells cut by it).
#[test]
fn coverage_of_an_id_from_the_kept_probe() {
    let s = session();
    let q = json!({"effect": 1, "x": 0.5, "y": 1.5});
    let left = crypto_hash("left");
    assert!(s.crypto_coverage(&q, left, 64).is_none(), "no coverage before the probe");
    s.crypto_probe_job(&q).unwrap().unwrap()();
    let c = s.crypto_coverage(&q, left, 64).unwrap();
    assert_eq!((c.width, c.height, c.size, c.uv), (16, 8, [8.0, 4.0], [1.0, 1.0]), "the aux pixels over the layer");
    assert!(c.coverage.iter().all(|v| (0.0..=1.0).contains(v)) && c.coverage.iter().any(|v| *v > 0.5) && c.coverage.contains(&0.0), "{:?}", c.coverage);
    assert_eq!(c.coverage[0], 0.6, "left's coverage at the first aux pixel");
    assert!(s.crypto_coverage(&q, crypto_hash("nobody"), 64).unwrap().coverage.iter().all(|v| *v == 0.0));
    assert_eq!(s.crypto_coverage(&q, left, 2).map(|c| (c.width, c.height)), Some((2, 1)));
    // An odd size: 2 × 2 cells, the last ones half outside the layer; samples at cell centres.
    let mut s = cells_session(1919, 1079, false);
    let q = json!({"effect": "Cryptomatte", "x": 40.0, "y": 10.0});
    assert_eq!(s.execute("effect.cryptomatteAt", q.clone()).unwrap()["name"], "c1_0");
    let c = s.crypto_coverage(&q, crypto_hash("c1_0"), 1024).unwrap();
    assert_eq!((c.width, c.height, c.size), (960, 540, [1919.0, 1079.0]));
    assert_eq!(c.uv, [1919.0 / 1920.0, 1079.0 / 1080.0]);
    // Sample 18 covers pixels 36-37: its centre (37) is in c1_0, its corner (36) in c0_0.
    assert_eq!((c.coverage[17], c.coverage[18], c.coverage[37]), (0.0, 1.0, 0.0));
}

struct Ranked(u32, u32, std::sync::OnceLock<Arc<AuxChannels>>);
impl FootageSource for Ranked {
    fn frame(&self, _: ItemId, f: &Footage, _: Tick) -> Option<Arc<Image>> {
        Some(Arc::new(Image::new(f.width, f.height)))
    }
    fn aux(&self, _: ItemId, _: &Footage, _: Tick) -> Option<Arc<AuxChannels>> {
        Some(
            self.2
                .get_or_init(|| {
                    let (w, h) = (self.0 as usize, self.1 as usize);
                    let mut aux = AuxChannels::new(w as u32, h as u32, 1.0);
                    for part in 0..3 {
                        for (k, c) in ["R", "G", "B", "A"].iter().enumerate() {
                            let rank = part * 2 + k / 2;
                            let v: Vec<f32> = (0..w * h)
                                .map(|i| if k % 2 == 0 { crypto_float(crypto_hash(&format!("r{rank}_{}", Cells::name(i % w, i / w)))) } else { 1.0 / 6.0 })
                                .collect();
                            aux.channels.push((format!("CryptoObject{part:02}.{c}"), v));
                        }
                    }
                    Arc::new(aux)
                })
                .clone(),
        )
    }
}

fn rss_mb() -> f64 {
    let s = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    s.lines().find(|l| l.starts_with("VmRSS")).and_then(|l| l.split_whitespace().nth(1)).and_then(|v| v.parse::<f64>().ok()).unwrap_or(0.0) / 1024.0
}

/// Highlight cost with 6 ranks (Cryptomatte's default 3 parts) at 1080p, 4K and 8K, and the memory the
/// probe pins. Run with `--ignored --nocapture`; it measures, it doesn't assert.
#[test]
#[ignore]
fn highlight_cost_with_six_ranks() {
    for (w, h) in [(1920u32, 1080u32), (3840, 2160), (7680, 4320)] {
        let r0 = rss_mb();
        let mut s = Session { footage: Arc::new(Ranked(w, h, Default::default())), importer: Some(Arc::new(CellsProbe(w, h))), ..Default::default() };
        let item = s.execute("file.import", json!({"paths": ["/renders/ranked.exr"]})).unwrap()["items"][0].clone();
        s.execute("comp.new", json!({"name": "C", "width": w, "height": h, "frameRate": 24, "duration": 2})).unwrap();
        s.execute("layer.addItem", json!({"item": item})).unwrap();
        s.execute_checked("effect.apply", json!({"effect": "Cryptomatte"})).unwrap();
        let q = json!({"effect": "Cryptomatte", "x": 100.0, "y": 500.0});
        let t0 = std::time::Instant::now();
        let r = s.execute("effect.cryptomatteAt", q.clone()).unwrap();
        let build = t0.elapsed().as_secs_f64() * 1e3;
        let hash = u32::from_str_radix(r["hash"].as_str().unwrap(), 16).unwrap();
        let mut ms = vec![];
        for _ in 0..5 {
            let t0 = std::time::Instant::now();
            let c = s.crypto_coverage(&q, hash, 1024).unwrap();
            ms.push(t0.elapsed().as_secs_f64() * 1e3);
            assert!(c.coverage.iter().any(|v| *v > 0.0));
        }
        ms.sort_by(f64::total_cmp);
        let aux = s.footage.aux(ItemId(0), &Footage::default(), Tick::ZERO).unwrap();
        let bytes: usize = aux.channels.iter().map(|(_, v)| v.len() * 4).sum();
        eprintln!(
            "HIGHLIGHT 6 ranks {w}x{h}: aux {:.0} MB ({} planes), probe build {build:.1} ms, highlight median {:.2} ms (min {:.2}), RSS +{:.0} MB",
            bytes as f64 / 1e6,
            aux.channels.len(),
            ms[2],
            ms[0],
            rss_mb() - r0
        );
    }
}

/// 8 × 4 footage whose Cryptomatte layers are `Mine` (`left` / `right` halves) and `CryptoMaterial`
/// (the other way round).
struct Layers;
impl FootageSource for Layers {
    fn frame(&self, _: ItemId, f: &Footage, _: Tick) -> Option<Arc<Image>> {
        Some(Arc::new(Image::new(f.width, f.height)))
    }
    fn aux(&self, _: ItemId, _: &Footage, _: Tick) -> Option<Arc<AuxChannels>> {
        let mut aux = AuxChannels::new(8, 4, 1.0);
        for (name, flip) in [("Mine", false), ("CryptoMaterial", true)] {
            let ids = (0..32).map(|i| crypto_float(crypto_hash(if (i % 8 < 4) != flip { "left" } else { "right" }))).collect();
            aux.channels.push((format!("{name}00.R"), ids));
            aux.channels.push((format!("{name}00.G"), vec![1.0; 32]));
        }
        Some(Arc::new(aux))
    }
}

/// [`Crypto`] with a manifest that names two IDs `twin` (`right` and the unnamed `ghost`).
struct Twins;
impl FootageSource for Twins {
    fn frame(&self, item: ItemId, f: &Footage, t: Tick) -> Option<Arc<Image>> {
        Crypto.frame(item, f, t)
    }
    fn aux(&self, _: ItemId, _: &Footage, t: Tick) -> Option<Arc<AuxChannels>> {
        let mut aux = aux_at(t);
        aux.manifests[0].1.retain(|(n, _)| n != "right");
        aux.manifests[0].1.extend([("twin".to_string(), crypto_hash("right")), ("twin".to_string(), crypto_hash("ghost"))]);
        Some(Arc::new(aux))
    }
}

/// #473: the probe reads the layer the Layer value names, as the effect does: 0 (a saved
/// CryptoObject) on a file without it reads the file's first layer, 1 is CryptoMaterial, and 3
/// is the file's other layer.
#[test]
fn pick_reads_the_layer_the_layer_value_names() {
    let mut s = Session { footage: Arc::new(Layers), importer: Some(Arc::new(Probe)), ..Default::default() };
    let item = s.execute("file.import", json!({"paths": ["/renders/layers.exr"]})).unwrap()["items"][0].clone();
    s.execute("comp.new", json!({"name": "C", "width": 8, "height": 4, "frameRate": 24, "duration": 2})).unwrap();
    s.execute("layer.addItem", json!({"item": item})).unwrap();
    s.execute_checked("effect.apply", json!({"effect": "Cryptomatte"})).unwrap();
    let name_at = |s: &mut Session, layer: u32| {
        s.execute_checked("prop.set", json!({"path": "effects/#1/layer", "value": layer})).unwrap();
        let r = s.execute("effect.cryptomatteAt", json!({"effect": 1, "x": 0.5, "y": 1.5})).unwrap();
        (r["name"].clone(), r["layer"].clone())
    };
    // The files' layers sort as CryptoMaterial, Mine (no manifest here, so the IDs have no name).
    assert_eq!(name_at(&mut s, 0).1, "CryptoMaterial", "a saved CryptoObject: the first layer");
    assert_eq!(name_at(&mut s, 1).1, "CryptoMaterial");
    assert_eq!(name_at(&mut s, 3).1, "Mine");
}

fn ids(r: &Value) -> Vec<Value> {
    r["ids"].as_array().unwrap().iter().map(|i| i["name"].clone()).collect()
}

#[test]
fn ids_list_the_manifest_then_unnamed_ids_with_their_coverage() {
    let mut s = session();
    pick(&mut s, 0.5, "replace");
    let steps = s.history.undo.len();
    let r = s.execute_checked("effect.cryptomatteIds", json!({"effect": 1})).unwrap();
    assert_eq!(
        (r["layer"].as_str(), r["layers"].clone(), r["total"].as_u64()),
        (Some("CryptoObject"), json!(["CryptoObject", "Z.CryptoObject"]), Some(6)),
        "{r}"
    );
    // The manifest in its order, then the unnamed `ghost`, by hash.
    assert_eq!(ids(&r), [json!("left"), json!("right"), json!("late"), json!(COMMA), json!("star*"), Value::Null]);
    assert_eq!(r["ids"][5]["hash"], format!("{:08x}", crypto_hash("ghost")));
    assert_eq!((r["ids"][0]["selected"].as_bool(), r["ids"][1]["selected"].as_bool()), (Some(true), Some(false)));
    // `left`: 0.6, 0.3 and 0.5 of 4, 4 and 2 aux columns, on 6 of the 8 rows of a 16 × 8 aux.
    let want = (0.6 * 4.0 + 0.3 * 4.0 + 0.5 * 2.0) * 6.0 / 128.0;
    assert!((r["ids"][0]["coverage"].as_f64().unwrap() - want).abs() < 1e-6, "{}", r["ids"][0]);
    assert_eq!(r["ids"][2]["coverage"], 0.0, "`late` isn't on this frame");
    // `present` drops IDs that aren't on the frame; `filter` matches names, case-insensitively.
    let r = s.execute_checked("effect.cryptomatteIds", json!({"effect": 1, "present": true, "filter": "L"})).unwrap();
    assert_eq!(ids(&r), [json!("left"), json!(COMMA)]);
    let r = s.execute_checked("effect.cryptomatteIds", json!({"effect": 1, "time": 1.5, "present": true})).unwrap();
    assert!(ids(&r).contains(&json!("late")), "from 1 s `late` is on the frame: {r}");
    let r = s.execute_checked("effect.cryptomatteIds", json!({"effect": 1, "limit": 2})).unwrap();
    assert_eq!((ids(&r).len(), r["total"].as_u64()), (2, Some(6)));
    // A query: no undo step, the Selection as it was.
    assert_eq!((s.history.undo.len(), selection(&s)), (steps, "left".to_string()));
    assert!(s.execute_checked("effect.cryptomatteIds", json!({"effect": 1, "probeOnly": true})).is_err(), "not a parameter");
}

#[test]
fn coverage_point_is_deep_inside_the_id_and_picks_it() {
    let mut s = session();
    let r = s.execute_checked("effect.cryptomatteCoverage", json!({"effect": 1, "name": "star*"})).unwrap();
    assert_eq!((r["hashes"].clone(), r["bounds"].clone()), (json!([format!("{:08x}", crypto_hash("star*"))]), json!([4.0, 0.0, 6.0, 3.0])), "{r}");
    assert!((r["coverage"].as_f64().unwrap() - 24.0 / 128.0).abs() < 1e-9, "{r}");
    // `star*` is aux columns 8-11, rows 0-5: a pick reads it at layer pixels 4-5, rows 0-2 (the
    // aux pixel under each centre), all as deep; the one nearest the centre comes first (4, 1).
    // The first of them, the corner (4, 0), would give [4.5, 0.5], on the matte's top edge.
    assert_eq!(r["point"], json!([4.5, 1.5]));
    let at =
        |s: &mut Session, r: &Value| s.execute("effect.cryptomatteAt", json!({"effect": 1, "x": r["point"][0], "y": r["point"][1]})).unwrap()["name"].clone();
    assert_eq!(at(&mut s, &r), "star*");
    // Default: the whole Selection (a wildcard here); empty, it covers nothing.
    let none = s.execute_checked("effect.cryptomatteCoverage", json!({"effect": 1})).unwrap();
    assert_eq!((none["coverage"].as_f64(), none["bounds"].is_null(), none["point"].is_null()), (Some(0.0), true, true));
    s.execute_checked("prop.set", json!({"path": "effects/#1/selection", "value": "le*"})).unwrap();
    let sel = s.execute_checked("effect.cryptomatteCoverage", json!({"effect": 1})).unwrap();
    assert_eq!(sel["hashes"], json!([format!("{:08x}", crypto_hash("left"))]));
    assert_eq!(at(&mut s, &sel), "left");
    // An ID that is never on top (`right` loses every pixel, a tie to the lower rank) has
    // coverage and bounds but no point a pick would read.
    let right = s.execute_checked("effect.cryptomatteCoverage", json!({"effect": 1, "name": "right"})).unwrap();
    assert!(right["coverage"].as_f64().unwrap() > 0.0 && right["bounds"].is_array() && right["point"].is_null(), "{right}");
    // By hash, with or without the brackets.
    for hash in [format!("{:08x}", crypto_hash("ghost")), format!("<{:08x}>", crypto_hash("ghost"))] {
        let g = s.execute_checked("effect.cryptomatteCoverage", json!({"effect": 1, "hash": hash})).unwrap();
        assert_eq!(at(&mut s, &g), Value::Null, "`ghost` has no name");
        assert_eq!(g["bounds"], json!([6.0, 0.0, 7.0, 3.0]));
    }
}

#[test]
fn picks_by_name_or_hash_in_one_undo_step_each() {
    let mut s = session();
    let steps = s.history.undo.len();
    let r = s.execute_checked("effect.pickCryptomatte", json!({"effect": 1, "name": COMMA})).unwrap();
    assert_eq!((r["selection"].as_str(), r["changed"].as_bool()), (Some(r#""Green, \"Glossy\"""#), Some(true)), "{r}");
    let hash = format!("{:08x}", crypto_hash("ghost"));
    let r = s.execute_checked("effect.pickCryptomatte", json!({"effect": 1, "hash": hash, "mode": "add"})).unwrap();
    assert_eq!(r["selection"], format!(r#""Green, \"Glossy\"", <{hash}>"#));
    // `star*` goes in as its hash, so it can't act as a wildcard.
    let r = s.execute_checked("effect.pickCryptomatte", json!({"effect": 1, "name": "star*", "mode": "add"})).unwrap();
    assert!(r["selection"].as_str().unwrap().ends_with(&format!("<{:08x}>", crypto_hash("star*"))), "{r}");
    let r = s.execute_checked("effect.pickCryptomatte", json!({"effect": 1, "name": COMMA, "mode": "remove"})).unwrap();
    assert_eq!(r["selection"], format!("<{hash}>, <{:08x}>", crypto_hash("star*")));
    // An ID that isn't on this frame can be picked by name too.
    s.execute_checked("effect.pickCryptomatte", json!({"effect": 1, "name": "late"})).unwrap();
    assert_eq!(selection(&s), "late");
    assert_eq!(s.history.undo.len(), steps + 5);
    // Picking what is already there changes nothing and adds no step.
    let r = s.execute_checked("effect.pickCryptomatte", json!({"effect": 1, "name": "late", "mode": "add"})).unwrap();
    assert_eq!((r["changed"].as_bool(), s.history.undo.len()), (Some(false), steps + 5));
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(selection(&s), format!("<{hash}>, <{:08x}>", crypto_hash("star*")));
}

#[test]
fn named_ids_that_are_unknown_ambiguous_or_malformed_are_errors() {
    let mut s = session();
    let steps = s.history.undo.len();
    let err = |s: &mut Session, c: &str, p: Value| s.execute_checked(c, p).unwrap_err().to_string();
    for c in ["effect.pickCryptomatte", "effect.cryptomatteCoverage"] {
        assert!(err(&mut s, c, json!({"effect": 1, "name": "nobody"})).contains("no ID named \"nobody\" in CryptoObject"), "{c}");
        assert!(err(&mut s, c, json!({"effect": 1, "name": "left", "hash": "00000001"})).contains("not both"), "{c}");
        for bad in ["xyz", "123", "1234567890", ""] {
            assert!(err(&mut s, c, json!({"effect": 1, "hash": bad})).contains("8 hex digits"), "{c} {bad}");
        }
        assert!(err(&mut s, c, json!({"effect": 1, "name": 3})).contains("string"), "{c}");
    }
    assert!(err(&mut s, "effect.pickCryptomatte", json!({"effect": 1, "name": "left", "x": 1, "y": 1})).contains("not both"));
    assert!(err(&mut s, "effect.pickCryptomatte", json!({"effect": 1, "name": "left", "mode": "toggle"})).contains("mode"));
    assert_eq!((s.history.undo.len(), selection(&s)), (steps, String::new()));
    // A name the manifest gives to two IDs: pick by hash instead.
    let mut s = session_with(Arc::new(Twins));
    let e = err(&mut s, "effect.pickCryptomatte", json!({"effect": 1, "name": "twin"}));
    let (a, b) = (format!("{:08x}", crypto_hash("right")), format!("{:08x}", crypto_hash("ghost")));
    assert!(e.contains("2 IDs in CryptoObject are named \"twin\"") && e.contains(&a) && e.contains(&b), "{e}");
    assert!(err(&mut s, "effect.cryptomatteCoverage", json!({"effect": 1, "name": "twin"})).contains("pass `hash`"));
    let r = s.execute_checked("effect.pickCryptomatte", json!({"effect": 1, "hash": b})).unwrap();
    assert_eq!(r["selection"], format!("<{b}>"), "a shared name doesn't select one ID");
    // Without a manifest a name is hashed, as the Selection reads it.
    let mut s = session_with(Arc::new(NoManifest));
    let r = s.execute_checked("effect.pickCryptomatte", json!({"effect": 1, "name": "left"})).unwrap();
    assert_eq!((r["hash"].clone(), r["selection"].clone()), (json!(format!("{:08x}", crypto_hash("left"))), json!(format!("<{:08x}>", crypto_hash("left")))));
    let r = s.execute_checked("effect.cryptomatteIds", json!({"effect": 1})).unwrap();
    assert!(r["ids"].as_array().unwrap().iter().all(|i| i["name"].is_null()) && r["total"] == 5, "{r}");
}

/// [`Crypto`] without a manifest.
struct NoManifest;
impl FootageSource for NoManifest {
    fn frame(&self, item: ItemId, f: &Footage, t: Tick) -> Option<Arc<Image>> {
        Crypto.frame(item, f, t)
    }
    fn aux(&self, _: ItemId, _: &Footage, t: Tick) -> Option<Arc<AuxChannels>> {
        let mut aux = aux_at(t);
        aux.manifests.clear();
        Some(Arc::new(aux))
    }
}

#[test]
fn new_commands_need_a_cryptomatte_effect_on_a_layer_with_cryptomatte() {
    let mut s = session();
    s.execute_checked("effect.apply", json!({"effect": "Gaussian Blur"})).unwrap();
    let queries =
        [("effect.cryptomatteIds", json!({})), ("effect.cryptomatteCoverage", json!({"name": "left"})), ("effect.pickCryptomatte", json!({"name": "left"}))];
    for (c, p) in &queries {
        let mut p = p.clone();
        p["effect"] = json!(2);
        assert!(s.execute_checked(c, p).unwrap_err().to_string().contains("not a Cryptomatte effect"), "{c}");
    }
    s.execute("layer.newSolid", json!({"name": "S"})).unwrap();
    s.execute_checked("effect.apply", json!({"effect": "Cryptomatte"})).unwrap();
    for (c, p) in queries {
        let mut p = p;
        p["effect"] = json!(1);
        let e = s.execute_checked(c, p).unwrap_err().to_string();
        assert!(e.contains("no Cryptomatte channels"), "{c}: {e}");
    }
}

#[test]
fn memory_purges_drop_the_probe() {
    for (what, drops) in [("memory", true), ("all", true), ("image", true), ("memoryAndDisk", true), ("disk", false), ("snapshot", false)] {
        let mut s = session();
        s.execute_checked("effect.cryptomatteIds", json!({"effect": 1})).unwrap();
        assert!(s.crypto_probe.lock().unwrap().kept(), "the queries keep the probe like a hover");
        s.execute_checked("edit.purge", json!({"what": what})).unwrap();
        assert_eq!(!s.crypto_probe.lock().unwrap().kept(), drops, "{what}");
    }
}

/// What the undo tests compare after every step: the Cryptomatte Selection (`None` without the
/// effect), the layer's opacity and position, and the undo and redo labels (next redo last).
#[derive(Clone, Debug, PartialEq)]
struct Snap {
    selection: Option<String>,
    opacity: Value,
    position: Value,
    undo: Vec<String>,
    redo: Vec<String>,
}

fn snap(s: &mut Session) -> Snap {
    let l = &s.active_comp().unwrap().layers[0];
    let selection = l.effects().and_then(|fx| fx.groups().find(|g| g.match_id == "ec.3d.cryptomatte")).map(|g| match &g.prop("selection").unwrap().value {
        effectcraft_keyframe::Value::Str(t) => t.clone(),
        v => panic!("{v:?}"),
    });
    let mut get = |path: &str| s.execute("prop.get", json!({"layer": 1, "path": path})).unwrap()["value"].clone();
    let (opacity, position) = (get("transform/opacity"), get("transform/position"));
    let labels = |v: &[(String, Arc<effectcraft_project::Project>)]| v.iter().map(|(l, _)| l.clone()).collect();
    Snap { selection, opacity, position, undo: labels(&s.history.undo), redo: labels(&s.history.redo) }
}

#[test]
fn name_and_hash_picks_are_one_undo_step_and_queries_and_purges_none() {
    let mut s = session();
    let base = s.history.undo.len();
    s.execute_checked("effect.pickCryptomatte", json!({"effect": 1, "name": "left"})).unwrap();
    assert_eq!(s.history.undo.len(), base + 1);
    s.execute_checked("effect.pickCryptomatte", json!({"effect": 1, "hash": format!("{:08x}", crypto_hash("ghost")), "mode": "add"})).unwrap();
    assert_eq!(s.history.undo.len(), base + 2);
    s.execute_checked("effect.pickCryptomatte", json!({"effect": 1, "name": COMMA, "mode": "add"})).unwrap();
    s.execute("edit.undo", json!({})).unwrap();
    let before = snap(&mut s);
    assert_eq!(before.redo.len(), 1);
    let rev = s.revision;
    // Read-only commands and the probe's purge: no step, the redo stack untouched.
    s.execute_checked("effect.cryptomatteIds", json!({"effect": 1})).unwrap();
    s.execute_checked("effect.cryptomatteCoverage", json!({"effect": 1, "name": "left"})).unwrap();
    s.execute_checked("effect.cryptomatteCoverage", json!({"effect": 1})).unwrap();
    s.execute_checked("effect.cryptomatteAt", json!({"effect": 1, "x": 0.5, "y": 0.5})).unwrap();
    s.execute_checked("edit.purge", json!({"what": "memory"})).unwrap();
    s.execute_checked("effect.cryptomatteAt", json!({"effect": 1, "x": 2.5, "y": 0.5})).unwrap();
    assert_eq!((snap(&mut s), s.revision), (before.clone(), rev));
    // Picks that change nothing make no step either, and keep the redo stack.
    s.execute_checked("effect.pickCryptomatte", json!({"effect": 1, "name": "left", "mode": "add"})).unwrap();
    s.execute_checked("effect.pickCryptomatte", json!({"effect": 1, "name": "right", "mode": "remove"})).unwrap();
    assert_eq!(snap(&mut s), before);
    s.execute("edit.redo", json!({})).unwrap();
    assert_eq!(selection(&s), format!(r#"left, <{:08x}>, "Green, \"Glossy\"""#, crypto_hash("ghost")));
    // Undo after a purge restores the Selection, and a pick after it rebuilds the probe.
    s.execute_checked("edit.purge", json!({"what": "all"})).unwrap();
    assert!(!s.crypto_probe.lock().unwrap().kept());
    s.execute("edit.undo", json!({})).unwrap();
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(selection(&s), "left");
    let builds = s.crypto_probe.lock().unwrap().builds().1;
    let r = s.execute_checked("effect.pickCryptomatte", json!({"effect": 1, "x": 2.5, "y": 0.5, "mode": "add"})).unwrap();
    assert_eq!(r["selection"], r#"left, "Green, \"Glossy\"""#);
    assert_eq!(s.crypto_probe.lock().unwrap().builds().1, builds + 1, "rebuilt after the undo");
    assert!(s.history.redo.is_empty(), "a new step clears the redo stack");
}

#[test]
fn a_long_mixed_history_undoes_and_redoes_step_by_step() {
    let mut s = session();
    let g = format!("<{:08x}>", crypto_hash("ghost"));
    let st = format!("<{:08x}>", crypto_hash("star*"));
    let gh = format!("{:08x}", crypto_hash("ghost"));
    let pick = |p: Value| ("effect.pickCryptomatte", p);
    // Each step, then the Selection, opacity and position it leaves.
    #[rustfmt::skip]
    let steps: Vec<((&str, Value), Option<String>, f64, [f64; 3])> = vec![
        (pick(json!({"effect": 1, "name": "left"})), Some("left".into()), 100.0, [4.0, 2.0, 0.0]),
        (("prop.set", json!({"layer": 1, "path": "transform/opacity", "value": 50})), Some("left".into()), 50.0, [4.0, 2.0, 0.0]),
        (pick(json!({"effect": 1, "x": 2.5, "y": 0.5, "mode": "add"})), Some(r#"left, "Green, \"Glossy\"""#.to_string()), 50.0, [4.0, 2.0, 0.0]),
        (pick(json!({"effect": 1, "hash": gh, "mode": "add"})), Some(format!(r#"left, "Green, \"Glossy\"", {g}"#)), 50.0, [4.0, 2.0, 0.0]),
        (("prop.set", json!({"layer": 1, "path": "transform/position", "value": [6, 3]})), Some(format!(r#"left, "Green, \"Glossy\"", {g}"#)), 50.0, [6.0, 3.0, 0.0]),
        (pick(json!({"effect": 1, "name": "left", "mode": "remove"})), Some(format!(r#""Green, \"Glossy\"", {g}"#)), 50.0, [6.0, 3.0, 0.0]),
        (pick(json!({"effect": 1, "x": 4.5, "y": 1.5})), Some(st.clone()), 50.0, [6.0, 3.0, 0.0]),
        (("effect.remove", json!({"layer": 1, "effect": 1})), None, 50.0, [6.0, 3.0, 0.0]),
        (("prop.set", json!({"layer": 1, "path": "transform/opacity", "value": 25})), None, 25.0, [6.0, 3.0, 0.0]),
        (("effect.apply", json!({"effect": "Cryptomatte"})), Some(String::new()), 25.0, [6.0, 3.0, 0.0]),
        (pick(json!({"effect": 1, "name": "right"})), Some("right".into()), 25.0, [6.0, 3.0, 0.0]),
        (pick(json!({"effect": 1, "x": 0.5, "y": 0.5, "mode": "add"})), Some("right, left".into()), 25.0, [6.0, 3.0, 0.0]),
        (("prop.set", json!({"layer": 1, "path": "transform/position", "value": [1, 1]})), Some("right, left".into()), 25.0, [1.0, 1.0, 0.0]),
        (pick(json!({"effect": 1, "name": "right", "mode": "remove"})), Some("left".into()), 25.0, [1.0, 1.0, 0.0]),
    ];
    let start = snap(&mut s);
    assert_eq!((start.selection.as_deref(), start.opacity.clone(), start.position.clone()), (Some(""), json!(100.0), json!([4.0, 2.0, 0.0])), "{start:?}");
    // Forward: each step is one undo step, and leaves what the table says.
    let mut states = vec![start.clone()];
    for (i, ((c, p), sel, op, pos)) in steps.iter().enumerate() {
        s.execute_checked(c, p.clone()).unwrap_or_else(|e| panic!("step {i} {c}: {e}"));
        let now = snap(&mut s);
        assert_eq!((now.selection.clone(), now.opacity.clone(), now.position.clone()), (sel.clone(), json!(op), json!(pos)), "step {i} {c}");
        assert_eq!(now.undo.len(), start.undo.len() + i + 1, "step {i} {c}: one undo step");
        assert!(now.redo.is_empty());
        states.push(now);
    }
    let labels: Vec<String> = states.last().unwrap().undo[start.undo.len()..].to_vec();
    let picks = labels.iter().zip(&steps).filter(|(_, ((c, _), ..))| *c == "effect.pickCryptomatte").map(|(l, _)| l.clone()).collect::<Vec<_>>();
    assert!(!picks.is_empty() && picks.iter().all(|l| l == "Pick Cryptomatte"), "every pick is labelled as a pick: {labels:?}");
    let n = steps.len();
    // State k with the steps after it undone: the forward state, its later labels on the redo stack.
    let expect = |k: usize| Snap { redo: labels[k..].iter().rev().cloned().collect(), ..states[k].clone() };
    for k in (0..n).rev() {
        s.execute_checked("edit.undo", json!({})).unwrap();
        assert_eq!(snap(&mut s), expect(k), "undo to {k}");
    }
    for k in 1..=n {
        s.execute_checked("edit.redo", json!({})).unwrap();
        assert_eq!(snap(&mut s), expect(k), "redo to {k}");
    }
    // Undo part way (back into the first effect's picks), then a new edit drops the redo stack.
    for k in (6..n).rev() {
        s.execute_checked("edit.undo", json!({})).unwrap();
        assert_eq!(snap(&mut s), expect(k), "undo to {k} again");
    }
    let r = s.execute_checked("effect.pickCryptomatte", json!({"effect": 1, "name": "star*", "mode": "add"})).unwrap();
    let now = snap(&mut s);
    assert_eq!(now.selection, Some(format!(r#""Green, \"Glossy\"", {g}, {st}"#)), "{r}");
    assert_eq!((now.opacity.clone(), now.position.clone()), (json!(50.0), json!([6.0, 3.0, 0.0])));
    assert_eq!(now.undo, [states[6].undo.clone(), vec![picks[0].clone()]].concat());
    assert!(now.redo.is_empty(), "the new edit cleared the redo stack");
    s.execute_checked("edit.undo", json!({})).unwrap();
    assert_eq!(snap(&mut s), Snap { redo: vec![picks[0].clone()], ..states[6].clone() });
}

/// [`Crypto`] with an ID `thin` on aux column `.0` only (rows 0-5, where `star*` was), in the
/// manifest. The aux is twice the layer's size: a pick reads only the odd aux columns.
struct Thin(usize);
impl FootageSource for Thin {
    fn frame(&self, item: ItemId, f: &Footage, t: Tick) -> Option<Arc<Image>> {
        Crypto.frame(item, f, t)
    }
    fn aux(&self, _: ItemId, _: &Footage, t: Tick) -> Option<Arc<AuxChannels>> {
        let mut aux = aux_at(t);
        let w = aux.width as usize;
        for y in 0..6 {
            aux.channels[0].1[y * w + self.0] = crypto_float(crypto_hash("thin"));
            aux.channels[1].1[y * w + self.0] = 1.0;
        }
        aux.manifests[0].1.push(("thin".into(), crypto_hash("thin")));
        Some(Arc::new(aux))
    }
}

/// `point` is always a pixel a pick reads as the ID (an aux finer than the effect's input has
/// pixels no pick reads): on an even aux column `thin` has coverage and bounds but no point; on
/// an odd one its point picks it. Every named ID's point round-trips through cryptomatteAt.
#[test]
fn coverage_point_is_a_pixel_a_pick_reads() {
    let cov = |s: &mut Session, name: &str| s.execute_checked("effect.cryptomatteCoverage", json!({"effect": 1, "name": name})).unwrap();
    let at =
        |s: &mut Session, r: &Value| s.execute("effect.cryptomatteAt", json!({"effect": 1, "x": r["point"][0], "y": r["point"][1]})).unwrap()["name"].clone();
    let mut s = session_with(Arc::new(Thin(8)));
    let r = cov(&mut s, "thin");
    assert!(r["coverage"].as_f64().unwrap() > 0.0 && r["bounds"] == json!([4.0, 0.0, 4.5, 3.0]), "{r}");
    assert!(r["point"].is_null(), "no pick reads aux column 8: {r}");
    let reads: Vec<Value> =
        (0..8).flat_map(|x| (0..4).map(move |y| (x, y))).map(|(x, y)| json!({"effect": 1, "x": x as f64 + 0.5, "y": y as f64 + 0.5})).collect();
    for q in &reads {
        assert_ne!(s.execute("effect.cryptomatteAt", q.clone()).unwrap()["name"], "thin", "{q}");
    }
    let mut s = session_with(Arc::new(Thin(9)));
    let r = cov(&mut s, "thin");
    assert_eq!((r["point"].clone(), at(&mut s, &r)), (json!([4.5, 1.5]), json!("thin")), "{r}");
    for name in ["left", COMMA, "star*", "thin"] {
        let r = cov(&mut s, name);
        assert_eq!(at(&mut s, &r), name, "{r}");
    }
}

/// [`Ranked`] with a manifest naming its rank-0 IDs (`r0_c<col>_<row>`), so a wildcard Selection
/// stands for thousands of them.
struct Named(Ranked, std::sync::OnceLock<Arc<AuxChannels>>);
impl FootageSource for Named {
    fn frame(&self, item: ItemId, f: &Footage, t: Tick) -> Option<Arc<Image>> {
        self.0.frame(item, f, t)
    }
    fn aux(&self, item: ItemId, f: &Footage, t: Tick) -> Option<Arc<AuxChannels>> {
        Some(
            self.1
                .get_or_init(|| {
                    let mut aux = (*self.0.aux(item, f, t).unwrap()).clone();
                    let (w, h) = (self.0.0 as usize, self.0.1 as usize);
                    let names = (0..w.div_ceil(37)).flat_map(|x| (0..h.div_ceil(29)).map(move |y| format!("r0_{}", Cells::name(x * 37, y * 29))));
                    aux.manifests = vec![("CryptoObject".into(), names.map(|n| (n.clone(), crypto_hash(&n))).collect())];
                    Arc::new(aux)
                })
                .clone(),
        )
    }
}

fn named_session(w: u32, h: u32) -> Session {
    let mut s = Session {
        footage: Arc::new(Named(Ranked(w, h, Default::default()), Default::default())),
        importer: Some(Arc::new(CellsProbe(w, h))),
        ..Default::default()
    };
    let item = s.execute("file.import", json!({"paths": ["/renders/ranked.exr"]})).unwrap()["items"][0].clone();
    s.execute("comp.new", json!({"name": "C", "width": w, "height": h, "frameRate": 24, "duration": 2})).unwrap();
    s.execute("layer.addItem", json!({"item": item})).unwrap();
    s.execute_checked("effect.apply", json!({"effect": "Cryptomatte"})).unwrap();
    s
}

/// Seconds `cmd` takes (and its reply); the first query on a session includes the probe build.
fn timed(s: &mut Session, cmd: &str, p: Value) -> (f64, Value) {
    let t0 = std::time::Instant::now();
    let r = s.execute_checked(cmd, p).unwrap();
    (t0.elapsed().as_secs_f64(), r)
}

/// The coverage of a wildcard Selection (`*`: 18×13 = 234 named IDs at 640×360, 6 ranks) names
/// every manifest ID and covers rank 0 (1/6). More than 16 IDs, so `crypto_cov` takes its set
/// path, as for thousands. Timing bounds (1080p, 8K) live in [`wildcard_coverage_cost`].
#[test]
fn coverage_of_a_wildcard_selection_names_every_id() {
    let mut s = named_session(640, 360);
    s.execute_checked("prop.set", json!({"path": "effects/#1/selection", "value": "*"})).unwrap();
    let (cov, r) = timed(&mut s, "effect.cryptomatteCoverage", json!({"effect": 1}));
    assert_eq!(r["hashes"].as_array().unwrap().len(), 18 * 13, "every manifest name (more than 16: the set path)");
    assert!((r["coverage"].as_f64().unwrap() - 1.0 / 6.0).abs() < 1e-6, "rank 0 covers 1/6: {}", r["coverage"]);
    eprintln!("COST 640x360 6 ranks, `*` (234 IDs): coverage {cov:.3} s, probe build included");
    // One ID: its point is looked for over its bounds only.
    let (one, one_r) = timed(&mut s, "effect.cryptomatteCoverage", json!({"effect": 1, "name": "r0_c1_1"}));
    eprintln!("COST 640x360 6 ranks, one ID: coverage {one:.3} s");
    assert_eq!(one_r["hashes"].as_array().unwrap().len(), 1);
    assert!(one_r["coverage"].as_f64().unwrap() > 0.0, "{one_r}");
}

/// The coverage reply (coverage, bounds, point, hashes) is the same as the whole-grid version's
/// for names, unnamed hashes, Selections (wildcards included), the 2× aux fixtures and the
/// small session's IDs. At 640×360 (not 1080p: cost lives in `wildcard_coverage_cost`) the last
/// column/row of 37×29 cells are still cut off by the layer edge (18×13 = 234 named rank-0 IDs).
#[test]
fn coverage_matches_the_whole_grid_version() {
    let same = |s: &mut Session, q: Value| {
        let fast = s.execute_checked("effect.cryptomatteCoverage", q.clone()).unwrap();
        let reference = crate::commands::crypto_cov_reference(s, &q).unwrap();
        assert_eq!(fast, reference, "{q}");
        fast
    };
    let mut s = named_session(640, 360);
    let ids = s.execute_checked("effect.cryptomatteIds", json!({"effect": 1, "limit": 100000})).unwrap();
    let ids = ids["ids"].as_array().unwrap().clone();
    let (named, unnamed): (Vec<&Value>, Vec<&Value>) = ids.iter().partition(|i| i["name"].is_string());
    let mut points = 0;
    let samples: Vec<&&Value> = named.iter().step_by(7).take(34).collect();
    for i in &samples {
        points += usize::from(!same(&mut s, json!({"effect": 1, "name": i["name"]}))["point"].is_null());
    }
    for i in unnamed.iter().step_by(unnamed.len().div_ceil(10).max(1)).take(10) {
        same(&mut s, json!({"effect": 1, "hash": i["hash"]}));
    }
    for name in ["r0_c0_0", "r0_c17_0", "r0_c0_12", "r0_c17_12"] {
        assert!(!same(&mut s, json!({"effect": 1, "name": name}))["point"].is_null(), "{name}");
    }
    assert!(points == samples.len() && unnamed.len() >= 10, "{points} points of {}, {} unnamed", samples.len(), unnamed.len());
    for sel in ["*", "r0_c1*", "r0_c3_4, r0_c17_12"] {
        s.execute_checked("prop.set", json!({"path": "effects/#1/selection", "value": sel})).unwrap();
        same(&mut s, json!({"effect": 1}));
    }
    for col in [8, 9] {
        let mut s = session_with(Arc::new(Thin(col)));
        for name in ["thin", "left", COMMA, "star*"] {
            same(&mut s, json!({"effect": 1, "name": name}));
        }
    }
    let mut s = session();
    for name in ["left", COMMA, "star*"] {
        same(&mut s, json!({"effect": 1, "name": name}));
    }
    s.execute_checked("prop.set", json!({"path": "effects/#1/selection", "value": "le*"})).unwrap();
    same(&mut s, json!({"effect": 1}));
}

/// Cost of `cryptomatteCoverage` and `cryptomatteIds` with 6 ranks: one ID and a wildcard
/// Selection, at 1080p (`*`, 1976 IDs) and 8K (`r0_c1*`, 16539 IDs). Run with `--ignored
/// --nocapture`. At 1080p it asserts coverage stays within a few times of listing the IDs
/// (minimum of several runs); at 8K it only measures.
#[test]
#[ignore]
fn wildcard_coverage_cost() {
    for (w, h, wild) in [(1920u32, 1080u32, "*"), (7680, 4320, "r0_c1*")] {
        let r0 = rss_mb();
        let mut s = named_session(w, h);
        let (build, _) = timed(&mut s, "effect.cryptomatteIds", json!({"effect": 1}));
        let runs = if w == 1920 { 3 } else { 1 };
        let mut ids_min = f64::INFINITY;
        let mut one_min = f64::INFINITY;
        for _ in 0..runs {
            let (ids, _) = timed(&mut s, "effect.cryptomatteIds", json!({"effect": 1}));
            ids_min = ids_min.min(ids);
            let (one, _) = timed(&mut s, "effect.cryptomatteCoverage", json!({"effect": 1, "name": "r0_c1_1"}));
            one_min = one_min.min(one);
        }
        s.execute_checked("prop.set", json!({"path": "effects/#1/selection", "value": wild})).unwrap();
        s.execute_checked("effect.cryptomatteIds", json!({"effect": 1})).unwrap();
        let mut all_min = f64::INFINITY;
        let mut n = 0;
        for _ in 0..runs {
            let (all, r) = timed(&mut s, "effect.cryptomatteCoverage", json!({"effect": 1}));
            all_min = all_min.min(all);
            n = r["hashes"].as_array().unwrap().len();
        }
        eprintln!(
            "COST {w}x{h} 6 ranks: probe build {build:.2} s, ids {ids_min:.2} s, coverage of one ID {one_min:.2} s, of `{wild}` ({n} IDs) {all_min:.2} s; RSS +{:.0} MB",
            rss_mb() - r0
        );
        if w == 1920 {
            assert!(all_min < 4.0 * ids_min + 0.3, "coverage {all_min:.3} s against ids {ids_min:.3} s");
            assert!(one_min < ids_min + 0.05, "one ID {one_min:.3} s against ids {ids_min:.3} s");
        }
    }
}

/// The new commands' parameters: a hash as hex or as a number, an unknown hash or name an
/// error, `name` exact (no wildcards), wrong types for `limit`, `present` and `filter` errors,
/// and a pick with nothing to pick says what to pass.
#[test]
fn cryptomatte_params_are_checked() {
    let mut s = session();
    let err = |s: &mut Session, c: &str, p: Value| s.execute_checked(c, p).unwrap_err().to_string();
    let left = crypto_hash("left");
    for c in ["effect.pickCryptomatte", "effect.cryptomatteCoverage"] {
        let r = s.execute_checked(c, json!({"effect": 1, "hash": left})).unwrap();
        assert_eq!(r["hash"].as_str().or(r["hashes"][0].as_str()), Some(format!("{left:08x}").as_str()), "{c}: a number is the hash");
        for bad in [json!(-1), json!(4294967296u64), json!(1.5), json!(true), json!(null)] {
            assert!(err(&mut s, c, json!({"effect": 1, "hash": bad})).contains("8 hex digits"), "{c} {bad}");
        }
        let e = err(&mut s, c, json!({"effect": 1, "hash": "deadbeef"}));
        assert!(e.contains("no ID with that hash deadbeef in CryptoObject"), "{c}: {e}");
        assert!(err(&mut s, c, json!({"effect": 1, "hash": 0xdeadbeefu32})).contains("deadbeef"), "{c}");
        assert!(err(&mut s, c, json!({"effect": 1, "name": "le*"})).contains("no ID named \"le*\""), "{c}: no wildcards");
    }
    // A hash only in the Selection (an ID from another frame or file) can still be removed.
    s.execute_checked("prop.set", json!({"path": "effects/#1/selection", "value": "left, <deadbeef>"})).unwrap();
    let r = s.execute_checked("effect.pickCryptomatte", json!({"effect": 1, "hash": "deadbeef", "mode": "remove"})).unwrap();
    assert_eq!(r["selection"], "left");
    for (k, v, says) in
        [("limit", json!(-1), "`limit`"), ("limit", json!("3"), "`limit`"), ("present", json!("yes"), "`present`"), ("filter", json!(3), "`filter`")]
    {
        let e = err(&mut s, "effect.cryptomatteIds", json!({"effect": 1, k: v}));
        assert!(e.contains(says), "{k} {v}: {e}");
    }
    let e = err(&mut s, "effect.pickCryptomatte", json!({"effect": 1}));
    assert!(e.contains("pass a point (`x` and `y`, layer pixels), `name` or `hash`"), "{e}");
    assert!(err(&mut s, "effect.pickCryptomatte", json!({"effect": 1, "x": 1})).contains("pass a point"));
    // Without a manifest a name is hashed, and an ID nowhere is an error too.
    let mut s = session_with(Arc::new(NoManifest));
    let e = err(&mut s, "effect.pickCryptomatte", json!({"effect": 1, "name": "nobody"}));
    assert!(e.contains(&format!("no ID named that (its hash) {:08x}", crypto_hash("nobody"))), "{e}");
}
