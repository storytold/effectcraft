//! The Cryptomatte eyedropper (#473): the Selection's button arms it, the viewer names (and
//! highlights) the ID under the cursor, and clicks pick into the Selection: plain replaces, Shift
//! adds, Alt or Ctrl removes. It stays armed until Esc or the button. Checked over window and
//! panel sizes, display scales, viewer zoom, pan and resolution, proxies, overlapping popups, and
//! the asynchronous probe (hover and clicks before it is built, edits while it builds), playback
//! and scrubbing, navigation tools, errors and disarming.

use effectcraft_engine::Session;
use effectcraft_engine::keyframe::Value as KV;
use effectcraft_engine::project::{LayerId, Resolution};
use effectcraft_engine::raster::channels3d::{crypto_float, crypto_hash};
use effectcraft_ui_egui::dock::{self, PanelKind, SplitSize};
use effectcraft_ui_egui::panels::viewer::comp_to_screen;
use effectcraft_ui_egui::state::Tool;
use effectcraft_ui_egui::{Dialog, EffectcraftApp};
use egui::{Event, Modifiers, PointerButton, Pos2, pos2, vec2};
use egui_kittest::Harness;
use serde_json::json;

const READING: &str = "Reading Cryptomatte IDs…";
const HINT: &str = "Click: replace · Shift: add · Alt or Ctrl: remove";
const MAC_HINT: &str = "Click: replace · Shift: add · Option or Cmd: remove";
const PAUSED: &str = "Stop playback to pick";

/// The ID of the 37 × 29 px cell at pixel (x, y) of a full-size frame.
pub(crate) fn cell(x: usize, y: usize) -> String {
    format!("c{}_{}", x / 37, y / 29)
}

/// A `w` × `h` EXR (white RGBA) whose CryptoObject has an ID per cell of a `k`-times larger frame
/// (`k` = 2: a half-size proxy), all named in its manifest.
pub(crate) fn crypto_exr(dir: &std::path::Path, name: &str, w: usize, h: usize, k: usize) -> String {
    use exr::prelude::*;
    let ids: Vec<f32> = (0..w * h).map(|i| crypto_float(crypto_hash(&cell(i % w * k, i / w * k)))).collect();
    let ch = |n: &str, v: Vec<f32>| AnyChannel::new(n, FlatSamples::F32(v));
    let one = || vec![1.0f32; w * h];
    let channels = AnyChannels::sort(
        vec![ch("R", one()), ch("G", one()), ch("B", one()), ch("A", one()), ch("CryptoObject00.R", ids), ch("CryptoObject00.G", one())].into(),
    );
    let mut layer = Layer::new((w, h), LayerAttributes::default(), Encoding::FAST_LOSSLESS, channels);
    let names: serde_json::Map<String, serde_json::Value> = (0..(w * k).div_ceil(37))
        .flat_map(|x| (0..(h * k).div_ceil(29)).map(move |y| cell(x * 37, y * 29)))
        .map(|n| (n.clone(), json!(format!("{:08x}", crypto_hash(&n)))))
        .collect();
    for (key, v) in [("name", "CryptoObject".to_string()), ("manifest", serde_json::Value::Object(names).to_string())] {
        layer.attributes.other.insert(Text::from(format!("cryptomatte/f834d0a/{key}").as_str()), AttributeValue::Text(Text::from(v.as_str())));
    }
    let path = dir.join(name);
    Image::from_layer(layer).write().to_file(&path).unwrap();
    path.to_string_lossy().to_string()
}

struct Ids {
    layer: u64,
    sel: u64,
    footage: u64,
    dir: std::path::PathBuf,
}

/// A `w` × `h` comp with the EXR (and its half-size proxy file) under a Cryptomatte effect (after
/// a heavy blur with `blur`: its probe then takes a while to build).
fn crypto_session(tag: &str, w: usize, h: usize, blur: bool) -> (Session, Ids) {
    let dir = std::env::temp_dir().join(format!("ec-ui-crypick-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("proxy")).unwrap();
    let path = crypto_exr(&dir, "plate.exr", w, h, 1);
    crypto_exr(&dir.join("proxy"), "plate_proxy.exr", w.div_ceil(2), h.div_ceil(2), 2);
    let mut s = effectcraft_host::session();
    let r = s.execute("file.import", json!({"paths": [path]})).unwrap();
    assert_eq!(r["errors"], json!([]), "{r}");
    let footage = r["items"][0].as_u64().unwrap();
    s.execute("comp.new", json!({"name": "Crypto", "width": w, "height": h, "frameRate": 24, "duration": 2})).unwrap();
    let layer = s.execute("layer.addItem", json!({"item": footage})).unwrap()["layer"].as_u64().unwrap();
    s.state.selected_layers = vec![LayerId(layer)];
    if blur {
        s.execute_checked("effect.apply", json!({"effect": "Gaussian Blur"})).unwrap();
        s.execute_checked("prop.set", json!({"path": "effects/#1/blurriness", "value": 40.0})).unwrap();
    }
    s.execute_checked("effect.apply", json!({"effect": "Cryptomatte"})).unwrap();
    let fx = s.active_comp().unwrap().layers[0].effects().unwrap().groups().last().unwrap();
    let sel = fx.get("selection").unwrap().uid;
    (s, Ids { layer, sel, footage, dir })
}

/// The app with the Composition viewer and Effect Controls (`ec` points wide) side by side.
fn harness(s: Session, size: egui::Vec2, ppp: f32, ec: f32) -> Harness<'static, EffectcraftApp> {
    harness_dt(s, size, ppp, ec, 0.25)
}

/// [`harness`] whose steps advance the clock by `dt` seconds.
fn harness_dt(s: Session, size: egui::Vec2, ppp: f32, ec: f32, dt: f32) -> Harness<'static, EffectcraftApp> {
    let app = app(s, ec);
    let mut h = Harness::builder().with_size(size).with_pixels_per_point(ppp).with_step_dt(dt).build_eframe(|_| app);
    settle(&mut h);
    h
}

fn app(s: Session, ec: f32) -> EffectcraftApp {
    let mut app = EffectcraftApp::new(s);
    app.ui.viewer.show_layer_controls = false;
    app.ui.dock = dock::docked(dock::vsplit(
        SplitSize::FixedSecond(160.0),
        dock::hsplit(SplitSize::FixedSecond(ec), dock::tabs(&[PanelKind::Composition], 0), dock::tabs(&[PanelKind::EffectControls], 0)),
        dock::tabs(&[PanelKind::Timeline], 0),
    ));
    app
}

pub(crate) fn settle(h: &mut Harness<'_, EffectcraftApp>) {
    for _ in 0..1500 {
        h.step();
        if h.state().frames.inflight() == 0 && h.state().frames.last_ms.lock().is_ok_and(|v| *v > 0.0) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    h.run_steps(4);
}

pub(crate) fn chip(h: &Harness<'_, EffectcraftApp>) -> Option<String> {
    h.state().auto.find("viewer.cryptoPick").map(|e| e.label.clone())
}

/// Step until the chip stops saying the probe is being built (the job runs on its own thread).
pub(crate) fn wait_probe(h: &mut Harness<'_, EffectcraftApp>) {
    for _ in 0..3000 {
        h.step();
        if chip(h).as_deref() != Some(READING) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    panic!("the probe never finished");
}

fn hover(h: &mut Harness<'_, EffectcraftApp>, pos: Pos2) -> String {
    h.event(Event::PointerMoved(pos));
    h.step();
    wait_probe(h);
    chip(h).unwrap_or_default()
}

fn click(h: &mut Harness<'_, EffectcraftApp>, pos: Pos2, modifiers: Modifiers) {
    h.event(Event::PointerMoved(pos));
    h.event(Event::ModifiersChanged(modifiers));
    h.step();
    for pressed in [true, false] {
        h.event(Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers });
        h.step();
    }
    h.event(Event::ModifiersChanged(Modifiers::NONE));
    h.step();
    wait_probe(h);
    h.run_steps(2);
}

pub(crate) fn at(h: &Harness<'_, EffectcraftApp>, id: &str) -> Pos2 {
    let e = h.state().auto.find(id).unwrap_or_else(|| panic!("missing {id}")).clone();
    pos2(e.rect[0] + e.rect[2] * 0.5, e.rect[1] + e.rect[3] * 0.5)
}

fn selection(h: &Harness<'_, EffectcraftApp>, ids: &Ids) -> String {
    let s = &h.state().session;
    match &s.active_comp().unwrap().layer(LayerId(ids.layer)).unwrap().props.find(ids.sel).unwrap().value {
        KV::Str(t) => t.clone(),
        v => panic!("{v:?}"),
    }
}

fn arm(h: &mut Harness<'_, EffectcraftApp>, ids: &Ids) {
    let eye = at(h, &format!("effectControls.prop.{}.eyedropper", ids.sel));
    click(h, eye, Modifiers::NONE);
    assert!(h.state().ui.fx_pick.as_ref().is_some_and(|p| p.kind == "crypto" && p.prop == ids.sel), "armed");
}

/// The resolution of the frame the viewer shows: every n-th pixel (its key keeps thousandths).
fn shown_scale(h: &Harness<'_, EffectcraftApp>) -> f64 {
    let s = &h.state().session;
    1.0 / (1000.0 / h.state().shown_series(s.active_comp_id().unwrap()).scale as f64).round()
}

/// The comp point the viewer maps screen point `p` to (its own arithmetic).
fn to_comp(h: &Harness<'_, EffectcraftApp>, p: Pos2) -> [f64; 2] {
    let (o, k) = (comp_to_screen(&h.ctx, [0.0, 0.0]).unwrap(), comp_to_screen(&h.ctx, [1000.0, 1000.0]).unwrap());
    let z = (k - o) / 1000.0;
    [((p.x - o.x) / z.x) as f64, ((p.y - o.y) / z.y) as f64]
}

/// The rendered matte of ID `name` (as the viewer renders the frame at `scale`) at comp point `p`.
/// A downsampled proxy blends cells at their edges, so an edge pixel can be a mix of IDs.
fn lit(h: &mut Harness<'_, EffectcraftApp>, name: &str, p: [f64; 2], scale: f64) -> f32 {
    let s = &mut h.state_mut().session;
    s.execute_checked("prop.set", json!({"path": "effects/#1/selection", "value": name})).unwrap();
    let cid = s.active_comp_id().unwrap();
    let opts = effectcraft_engine::render::RenderOpts { roi: None, guides: false, ..h.state().frame_opts(cid, scale) };
    let img = h.state().session.render(cid, effectcraft_engine::time::Tick::ZERO, opts);
    img.get((p[0] * scale).floor() as i64, (p[1] * scale).floor() as i64)[0]
}

/// Whether ID `got` (a cell name) has the largest coverage of the shown pixel at comp point `p`
/// (a downsampled or scaled layer blends the cells around it).
fn largest(h: &mut Harness<'_, EffectcraftApp>, got: &str, p: [f64; 2], scale: f64) -> bool {
    let v = lit(h, got, p, scale);
    if v >= 0.5 {
        return true;
    }
    let (cx, cy) = ((p[0] / 37.0).floor() as i64, (p[1] / 29.0).floor() as i64);
    let others: Vec<String> =
        (-2..=2).flat_map(|dx| (-2..=2).map(move |dy| (cx + dx, cy + dy))).filter(|c| c.0 >= 0 && c.1 >= 0).map(|c| format!("c{}_{}", c.0, c.1)).collect();
    v > 0.0 && others.iter().filter(|o| *o != got).all(|o| lit(h, o, p, scale) <= v + 1e-4)
}

#[test]
fn eyedropper_arms_hovers_picks_with_modifiers_and_stays_armed_until_esc() {
    let (s, ids) = crypto_session("flow", 320, 180, false);
    let mut h = harness(s, vec2(1400.0, 800.0), 1.0, 420.0);
    let steps = h.state().session.history.undo.len();
    arm(&mut h, &ids);
    assert!(h.state().ui.status.contains("Shift adds"), "{}", h.state().ui.status);
    let hint = comp_to_screen(&h.ctx, [-3.0, 90.0]).unwrap();
    assert_eq!(hover(&mut h, hint), HINT, "off the layer: the hint");
    let a = comp_to_screen(&h.ctx, [50.0, 40.0]).unwrap();
    let b = comp_to_screen(&h.ctx, [120.0, 100.0]).unwrap();
    assert_eq!(hover(&mut h, a), "c1_1");
    assert_eq!(hover(&mut h, b), "c3_3");
    click(&mut h, a, Modifiers::NONE);
    assert_eq!(selection(&h, &ids), "c1_1");
    assert!(h.state().ui.status.contains("c1_1"), "{}", h.state().ui.status);
    assert_eq!(hover(&mut h, a), "c1_1 (selected)");
    click(&mut h, b, Modifiers::SHIFT);
    assert_eq!((selection(&h, &ids), pick_steps(&h)), ("c1_1, c3_3".to_string(), 2));
    click(&mut h, a, Modifiers::ALT);
    assert_eq!(selection(&h, &ids), "c3_3");
    click(&mut h, b, Modifiers::COMMAND);
    assert_eq!(selection(&h, &ids), "");
    // The modifiers of the press count: Shift let go before the button still adds.
    click(&mut h, a, Modifiers::NONE);
    h.event(Event::PointerMoved(b));
    h.event(Event::ModifiersChanged(Modifiers::SHIFT));
    h.step();
    h.event(Event::PointerButton { pos: b, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::SHIFT });
    h.step();
    h.event(Event::ModifiersChanged(Modifiers::NONE));
    h.step();
    h.event(Event::PointerButton { pos: b, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
    h.run_steps(3);
    assert_eq!(selection(&h, &ids), "c1_1, c3_3", "Shift held at the press adds");
    // On a Mac the hint says Option or Cmd, and Ctrl (no Cmd there) removes too.
    h.ctx.set_os(egui::os::OperatingSystem::Mac);
    assert_eq!(hover(&mut h, hint), MAC_HINT);
    click(&mut h, a, Modifiers::CTRL);
    assert_eq!(selection(&h, &ids), "c3_3", "Ctrl on a Mac");
    h.ctx.set_os(egui::os::OperatingSystem::Nix);
    click(&mut h, b, Modifiers::ALT);
    assert_eq!(selection(&h, &ids), "");
    assert_eq!(h.state().session.history.undo.len(), steps + 8, "one undo step per pick");
    // The pointer leaves the viewer: the hint again.
    let away = at(&h, &format!("effectControls.prop.{}.eyedropper", ids.sel)) + vec2(0.0, 60.0);
    h.event(Event::PointerMoved(away));
    h.run_steps(2);
    assert_eq!(chip(&h).as_deref(), Some(HINT), "hover ends with the pointer away");
    // Outside the layer: nothing to pick, no step; still armed.
    let off = comp_to_screen(&h.ctx, [-30.0, 40.0]).unwrap();
    if h.state().auto.find("viewer.fxPick").is_some_and(|e| off.x > e.rect[0]) {
        click(&mut h, off, Modifiers::NONE);
        assert_eq!(h.state().session.history.undo.len(), steps + 8);
    }
    assert!(h.state().ui.fx_pick.is_some(), "stays armed");
    h.key_press(egui::Key::Escape);
    h.run_steps(2);
    assert!(h.state().ui.fx_pick.is_none(), "Esc ends it");
    assert!(chip(&h).is_none());
    // The button toggles it too.
    arm(&mut h, &ids);
    let eye = at(&h, &format!("effectControls.prop.{}.eyedropper", ids.sel));
    click(&mut h, eye, Modifiers::NONE);
    assert!(h.state().ui.fx_pick.is_none());
    let _ = std::fs::remove_dir_all(&ids.dir);
}

// Window and panel sizes × display scales, one test each so they run in parallel. Together they
// took ~90 s as one test, and ~10 min under coverage, where that one test set the slowest shard.
#[test]
fn picks_what_the_viewer_shows_at_900x640_and_1x() {
    picks_what_the_viewer_shows(0, vec2(900.0, 640.0), 1.0, 300.0);
}

#[test]
fn picks_what_the_viewer_shows_at_1280x720_and_1_25x() {
    picks_what_the_viewer_shows(1, vec2(1280.0, 720.0), 1.25, 420.0);
}

#[test]
fn picks_what_the_viewer_shows_at_1600x1000_and_1_5x() {
    picks_what_the_viewer_shows(2, vec2(1600.0, 1000.0), 1.5, 360.0);
}

#[test]
fn picks_what_the_viewer_shows_at_2400x1400_and_2x() {
    picks_what_the_viewer_shows(3, vec2(2400.0, 1400.0), 2.0, 600.0);
}

/// Case `i`: a window of `size` at display scale `ppp`, with an Effect Controls panel `ec` wide,
/// × viewer zoom / pan × Resolution (and a proxy). Hover and click name the ID of the pixel the
/// viewer shows, next to cell edges too. Each hover is checked against the rendered matte of the
/// named ID at the point the screen maps back to. Case 0 also checks frames from the idle cache.
fn picks_what_the_viewer_shows(i: usize, size: egui::Vec2, ppp: f32, ec: f32) {
    let mut checked = 0;
    let mut failures = vec![];
    let (s, ids) = crypto_session(&format!("matrix{i}"), 1919, 1079, false);
    let mut h = harness(s, size, ppp, ec);
    h.state_mut().session.execute_checked("prop.set", json!({"path": "effects/#1/matteOnly", "value": true})).unwrap();
    arm(&mut h, &ids);
    for proxy in [false, true] {
        if proxy {
            let p = ids.dir.join("proxy").join("plate_proxy.exr");
            h.state_mut().session.execute("file.setProxy", json!({"item": ids.footage, "path": p.to_string_lossy()})).unwrap();
        }
        for res in [Resolution::Full, Resolution::Half, Resolution::Third, Resolution::Quarter, Resolution::Custom(6)] {
            for (zoom, pan) in [(None, [0.0, 0.0]), (Some(0.5), [0.0, 0.0]), (Some(2.0), [-300.0, 120.0])] {
                let cid = h.state().session.active_comp_id().unwrap();
                h.state_mut().session.set_resolution(cid, res).unwrap();
                h.state_mut().ui.viewer.zoom = zoom.map(|z: f32| z / ppp);
                h.state_mut().ui.viewer.pan = pan;
                settle(&mut h);
                let area = h.state().auto.find("viewer.fxPick").unwrap().rect;
                let scale = shown_scale(&h);
                // Points next to cell edges, in the middle and at the far corner.
                // (Cell edges far from the origin: 925, 1044 shows the 1/3 vs 0.333 rounding.)
                for p in [[36.6, 28.7], [37.4, 29.3], [959.7, 539.2], [1918.4, 1078.6], [74.1, 57.9], [925.4, 1044.4], [1110.6, 1043.6]] {
                    let Some(sp) = comp_to_screen(&h.ctx, [p[0] as f32, p[1] as f32]) else { continue };
                    if sp.x < area[0] + 4.0 || sp.y < area[1] + 40.0 || sp.x > area[0] + area[2] - 4.0 || sp.y > area[1] + area[3] - 4.0 {
                        continue;
                    }
                    let got = hover(&mut h, sp);
                    let got = got.trim_end_matches(" (selected)").to_string();
                    // The comp point under the cursor, as the viewer maps it back.
                    let q = to_comp(&h, sp);
                    checked += 1;
                    // The named ID has the largest coverage of the shown pixel (a blended edge
                    // pixel of a proxy at Quarter has several). No ID named: the frame shows
                    // (almost) no matte there either (an odd size's last half-resolution pixel
                    // samples just outside the layer).
                    let ok = if got.starts_with('c') { largest(&mut h, &got, q, scale) } else { got == HINT && lit(&mut h, "*", q, scale) < 0.5 };
                    if !ok {
                        failures
                            .push(format!("{size:?} ppp {ppp} proxy {proxy} {res:?} zoom {zoom:?} (scale {scale}): at {q:?} hover {got:?} is not lit there"));
                    }
                }
            }
        }
        // Frames from Cache Frames When Idle (the first case): rendered on the grid the
        // viewer renders and the pick reads (1/3, not the key's 0.333), so frame 20 from the
        // idle cache is frame 0 as the viewer rendered it, and hovers there are right too.
        if i == 0 && !proxy {
            (h.state_mut().ui.viewer.zoom, h.state_mut().ui.viewer.pan) = (None, [0.0, 0.0]);
            for res in [Resolution::Third, Resolution::Custom(6)] {
                let fails = idle_cached_frames_are_shown_frames(&mut h, res);
                checked += 1;
                failures.extend(fails);
            }
        }
        // A click picks what the hover named.
        (h.state_mut().ui.viewer.zoom, h.state_mut().ui.viewer.pan) = (None, [0.0, 0.0]);
        settle(&mut h);
        let sp = comp_to_screen(&h.ctx, [959.7, 539.2]).unwrap();
        let named = hover(&mut h, sp);
        click(&mut h, sp, Modifiers::NONE);
        assert_eq!(selection(&h, &ids), named.trim_end_matches(" (selected)"));
        if proxy {
            h.state_mut().session.execute("file.setProxy", json!({"item": ids.footage, "path": null})).ok();
        }
    }
    let _ = std::fs::remove_dir_all(&ids.dir);
    assert!(failures.is_empty(), "{} of {checked} hovers wrong:\n{}", failures.len(), failures.join("\n"));
    // more than 100 over the four cases, as when they were one test
    assert!(checked > 25, "{checked}");
}

fn set_frame(h: &mut Harness<'_, EffectcraftApp>, f: i64) {
    h.state_mut().session.set_time(effectcraft_engine::time::Tick::from_seconds_f64(f as f64 / 24.0));
}

/// The viewer's CPU frame `f` of the series shown.
fn cpu_frame(h: &Harness<'_, EffectcraftApp>, f: i64) -> Option<([usize; 2], Vec<egui::Color32>)> {
    let key = h.state().shown_series(h.state().session.active_comp_id().unwrap());
    match h.state().frames.get_for_viewer(&effectcraft_ui_egui::frames::FrameKey { frame: f, ..key }, None) {
        Some((_, effectcraft_ui_egui::frames::FrameImage::Cpu(c))) => Some((c.size, c.pixels.clone())),
        _ => None,
    }
}

/// At `res`, with the Colors display: frame 20 cached when idle against frame 0 rendered by the
/// viewer (the footage is a still), then hovers on frame 20. Returns what is wrong.
fn idle_cached_frames_are_shown_frames(h: &mut Harness<'_, EffectcraftApp>, res: Resolution) -> Vec<String> {
    let cid = h.state().session.active_comp_id().unwrap();
    let display = h.state().session.active_comp().unwrap().layers[0].effects().unwrap().groups().last().unwrap().get("display").unwrap().value.to_json();
    h.state_mut().session.execute_checked("prop.set", json!({"path": "effects/#1/matteOnly", "value": false})).unwrap();
    h.state_mut().session.execute_checked("prop.set", json!({"path": "effects/#1/display", "value": 0})).unwrap();
    h.state_mut().session.set_resolution(cid, res).unwrap();
    set_frame(h, 0);
    settle(h);
    let shown = cpu_frame(h, 0).expect("frame 0 shown");
    h.state_mut().ui.cache_when_idle = true;
    for _ in 0..2000 {
        h.step();
        let key = h.state().shown_series(cid);
        if h.state().frames.is_cached(&effectcraft_ui_egui::frames::FrameKey { frame: 20, ..key }) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    h.state_mut().ui.cache_when_idle = false;
    let mut out = vec![];
    match cpu_frame(h, 20) {
        None => out.push(format!("{res:?}: frame 20 was never cached when idle")),
        Some(c) if c.0 != shown.0 => out.push(format!("{res:?}: the idle cache rendered {:?}, the viewer {:?}", c.0, shown.0)),
        Some(c) if c.1 != shown.1 => out.push(format!("{res:?}: the idle-cached frame differs from the viewer's")),
        Some(_) => {}
    }
    h.state_mut().session.execute_checked("prop.set", json!({"path": "effects/#1/matteOnly", "value": true})).unwrap();
    h.state_mut().session.execute_checked("prop.set", json!({"path": "effects/#1/display", "value": display})).unwrap();
    set_frame(h, 20);
    settle(h);
    let scale = shown_scale(h);
    for p in [[959.7, 539.2], [925.4, 1044.4], [1110.6, 1043.6]] {
        let sp = comp_to_screen(&h.ctx, [p[0] as f32, p[1] as f32]).unwrap();
        let got = hover(h, sp).trim_end_matches(" (selected)").to_string();
        let q = to_comp(h, sp);
        if !(got.starts_with('c') && largest(h, &got, q, scale)) {
            out.push(format!("{res:?} idle-cached frame 20: at {q:?} hover {got:?} is not lit there"));
        }
    }
    set_frame(h, 0);
    settle(h);
    out
}

/// Press and release at `pos` without waiting for the probe.
fn press(h: &mut Harness<'_, EffectcraftApp>, pos: Pos2) {
    h.event(Event::PointerMoved(pos));
    h.run_steps(2);
    for pressed in [true, false] {
        h.event(Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE });
        h.step();
    }
}

/// An edit that drops the probe: the blur before the Cryptomatte effect changes, so the next
/// hover or click has to render it again to build the probe.
/// (Each blurriness `v` once: an earlier one would come from the layer cache.)
fn edit(h: &mut Harness<'_, EffectcraftApp>, v: f64) {
    h.state_mut().session.execute_checked("prop.set", json!({"path": "effects/#1/blurriness", "value": v})).unwrap();
}

/// Let a probe still being built finish with nothing waiting on it.
fn drain(h: &mut Harness<'_, EffectcraftApp>) {
    for _ in 0..100 {
        h.step();
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

/// Waiting clicks: the first hover starts the probe; a click made meanwhile waits then picks;
/// an edit, Esc, disarming, or hiding the viewer while it waits drops it.
/// Each section's probe build is held (`hold` / `log`) across its "while reading" / "waits"
/// asserts and the action that drops the click, then released before `wait_probe` / `drain` /
/// `hover` so a held build can finish.
fn waiting_clicks_are_dropped_by_edits_esc_disarm_and_hiding_the_viewer(
    h: &mut Harness<'_, EffectcraftApp>,
    ids: &Ids,
    a: Pos2,
    b: Pos2,
    hold: &std::sync::atomic::AtomicBool,
    log: &ReadLog,
) {
    use std::sync::atomic::Ordering;
    // A click before the probe is built waits for it.
    // (egui reports the hover a pass after the pointer moves.)
    hold.store(true, Ordering::Release);
    log.lock().unwrap().clear();
    h.event(Event::PointerMoved(a));
    h.run_steps(2);
    wait_held_id_read(h, log);
    assert_eq!(chip(h).as_deref(), Some(READING), "the first hover starts the probe off the UI thread");
    press(h, a);
    assert_eq!(selection(h, ids), "", "the click waits");
    hold.store(false, Ordering::Release);
    wait_probe(h);
    h.run_steps(2);
    assert_eq!(selection(h, ids), "c27_20", "then picks");
    assert_eq!(hover(h, b), "c50_40");
    let steps = h.state().session.history.undo.len();
    // An edit while a click waits drops the click.
    // edit(41) drops the probe; press starts a new build (held). edit(42) cancels that build and
    // drops the click — release so the cancelled build can leave the hold, then wait_probe.
    edit(h, 41.0);
    hold.store(true, Ordering::Release);
    log.lock().unwrap().clear();
    press(h, b);
    wait_held_id_read(h, log);
    assert_eq!(chip(h).as_deref(), Some(READING));
    edit(h, 42.0);
    hold.store(false, Ordering::Release);
    wait_probe(h);
    h.run_steps(2);
    assert_eq!(selection(h, ids), "c27_20", "not picked after the edit");
    assert_eq!(h.state().session.history.undo.len(), steps + 2, "only the two edits");
    // Esc while a click waits: no pick, disarmed. Hold the build started by press; Esc cancels it.
    edit(h, 43.0);
    hold.store(true, Ordering::Release);
    log.lock().unwrap().clear();
    press(h, b);
    wait_held_id_read(h, log);
    assert_eq!(chip(h).as_deref(), Some(READING));
    h.key_press(egui::Key::Escape);
    h.run_steps(2);
    assert!(h.state().ui.fx_pick.is_none() && chip(h).is_none());
    hold.store(false, Ordering::Release);
    drain(h);
    assert_eq!(selection(h, ids), "c27_20");
    // Disarmed while a click waits, then armed again: the old click is not made.
    // arm() waits the probe — hold only after arming, across the READING assert and disarm.
    edit(h, 44.0);
    arm(h, ids);
    hold.store(true, Ordering::Release);
    log.lock().unwrap().clear();
    press(h, b);
    wait_held_id_read(h, log);
    assert_eq!(chip(h).as_deref(), Some(READING));
    h.state_mut().ui.fx_pick = None;
    h.run_steps(2);
    hold.store(false, Ordering::Release);
    arm(h, ids);
    drain(h);
    assert_eq!(selection(h, ids), "c27_20", "the old click stays unmade");
    assert_eq!(hover(h, b), "c50_40", "still picks");
    // The viewer hidden while a click waits (Effect Controls maximised), then shown again: the
    // pick still on, the old click is not made either.
    edit(h, 45.0);
    hold.store(true, Ordering::Release);
    log.lock().unwrap().clear();
    press(h, b);
    wait_held_id_read(h, log);
    assert_eq!(chip(h).as_deref(), Some(READING));
    h.state_mut().ui.maximized = Some(PanelKind::EffectControls);
    h.run_steps(2);
    assert!(h.state().auto.find("viewer.fxPick").is_none(), "the viewer is hidden");
    hold.store(false, Ordering::Release);
    drain(h);
    h.state_mut().ui.maximized = None;
    h.run_steps(2);
    drain(h);
    assert!(h.state().ui.fx_pick.is_some());
    assert_eq!(selection(h, ids), "c27_20", "the click made before the viewer was hidden stays unmade");
}

/// Popups and dialogs over the viewer take the pointer. Another selected layer keeps the pick;
/// deleting the layer or switching comps ends it.
fn popups_dialogs_and_comp_switches_end_or_block_them(h: &mut Harness<'_, EffectcraftApp>, ids: &Ids, b: Pos2) {
    // A popup over the viewer takes the pointer: no hover, its click is its own.
    let area = h.state().auto.find("viewer.fxPick").unwrap().rect;
    let inside = |p: Pos2| p.x > area[0] && p.y > area[1] && p.x < area[0] + area[2] && p.y < area[1] + area[3];
    let res = at(h, "viewer.resolution");
    click(h, res, Modifiers::NONE);
    let item = at(h, "viewer.resolutionItem.0");
    assert!(inside(item), "the Resolution popup is over the viewer: {item:?} {area:?}");
    assert_eq!(hover(h, item), HINT, "nothing hovered under the popup");
    click(h, item, Modifiers::NONE);
    settle(h);
    assert_eq!(h.state().viewer_res(), effectcraft_ui_egui::state::Resolution::Full);
    assert_eq!(selection(h, ids), "c27_20", "the popup's click picked nothing");
    assert!(h.state().ui.fx_pick.is_some(), "still armed");
    assert_eq!(hover(h, b), "c50_40");
    // A click on the viewer while the popup is open only closes the popup.
    click(h, res, Modifiers::NONE);
    assert!(h.state().auto.find("viewer.resolutionItem.0").is_some(), "open again");
    click(h, b, Modifiers::SHIFT);
    assert!(h.state().auto.find("viewer.resolutionItem.0").is_none(), "closed");
    assert_eq!(selection(h, ids), "c27_20", "the closing click picked nothing");
    // So does a dialog.
    h.state_mut().dialog = Some(Dialog::CompSettings);
    h.run_steps(3);
    let d = at(h, "dialog.comp.name");
    assert!(inside(d), "the dialog is over the viewer: {d:?} {area:?}");
    assert_eq!(hover(h, d), HINT, "nothing hovered under the dialog");
    h.state_mut().dialog = None;
    h.run_steps(2);
    // Another layer selected: the pick stays with its layer.
    let solid = h.state_mut().session.execute("layer.newSolid", json!({"name": "Other", "width": 64, "height": 64})).unwrap()["layer"].as_u64().unwrap();
    h.state_mut().session.state.selected_layers = vec![LayerId(solid)];
    settle(h);
    assert!(h.state().ui.fx_pick.as_ref().is_some_and(|p| p.layer == ids.layer));
    click(h, b, Modifiers::SHIFT);
    assert_eq!(selection(h, ids), "c27_20, c50_40");
    // The layer deleted: the pick ends; undone, it is not armed again.
    h.state_mut().session.state.selected_layers = vec![LayerId(ids.layer)];
    h.state_mut().session.execute_checked("edit.clear", json!({"layers": [ids.layer]})).unwrap();
    h.run_steps(3);
    assert!(h.state().ui.fx_pick.is_none() && chip(h).is_none(), "ended with its layer");
    h.state_mut().session.execute_checked("edit.undo", json!({})).unwrap();
    h.run_steps(3);
    assert!(h.state().ui.fx_pick.is_none());
    // Another comp: the pick ends.
    h.state_mut().session.state.selected_layers = vec![LayerId(ids.layer)];
    h.run_steps(3);
    arm(h, ids);
    h.state_mut().session.execute("comp.new", json!({"name": "Other", "width": 320, "height": 180, "duration": 2})).unwrap();
    h.run_steps(3);
    assert!(h.state().ui.fx_pick.is_none() && chip(h).is_none(), "ended with the comp switch");
}

/// The asynchronous probe (a 4K plate behind a blur; each "while reading" section holds the
/// build): the first hover says it is reading, a click made meanwhile waits and then picks; an
/// edit, Esc or disarming while it waits drops it. Popups and dialogs over the viewer take the
/// pointer. Another selected layer keeps the pick; deleting the layer or switching comps ends it.
#[test]
fn clicks_wait_for_the_probe_and_edits_popups_dialogs_and_comp_switches_end_or_block_them() {
    let (mut s, ids) = crypto_session("async", 3840, 2160, true);
    let hold = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let log = ReadLog::default();
    s.footage = std::sync::Arc::new(Held(s.footage.clone(), hold.clone(), log.clone()));
    let mut h = harness(s, vec2(1280.0, 800.0), 1.0, 420.0);
    arm(&mut h, &ids);
    let (a, b) = (comp_to_screen(&h.ctx, [1017.5, 594.5]).unwrap(), comp_to_screen(&h.ctx, [1868.5, 1174.5]).unwrap());
    waiting_clicks_are_dropped_by_edits_esc_disarm_and_hiding_the_viewer(&mut h, &ids, a, b, &hold, &log);
    popups_dialogs_and_comp_switches_end_or_block_them(&mut h, &ids, b);
    let _ = std::fs::remove_dir_all(&ids.dir);
}

/// On the GPU compositor (egui_kittest with wgpu on the headless compositor's adapter, as in
/// `ui_gpu.rs`; lavapipe on CI-like boxes): hover and click name what the GPU viewer shows, at
/// each Resolution. Skips without a GPU adapter.
#[test]
fn picks_on_the_gpu_viewer() {
    let Some(probe) = effectcraft_gpu::Gpu::headless() else {
        eprintln!("no GPU adapter: skipping");
        return;
    };
    let adapter = effectcraft_engine::render::Accelerator::name(&probe);
    drop(probe);
    let mut setup = egui_kittest::wgpu::default_wgpu_setup();
    if let eframe::egui_wgpu::WgpuSetup::CreateNew(c) = &mut setup {
        let name = adapter.clone();
        c.native_adapter_selector = Some(std::sync::Arc::new(move |adapters, _| {
            adapters
                .iter()
                .find(|a| format!("{} ({:?})", a.get_info().name, a.get_info().backend) == name)
                .or(adapters.first())
                .cloned()
                .ok_or_else(|| "no adapter".to_owned())
        }));
    }
    let (s, ids) = crypto_session("gpu", 1919, 1079, false);
    let app = app(s, 420.0);
    let mut h = Harness::builder().with_size(vec2(1400.0, 900.0)).with_pixels_per_point(1.0).wgpu_setup(setup).build_eframe(|_| app);
    settle(&mut h);
    if h.state().gpu_adapter().is_none() {
        eprintln!("the viewer stayed on the CPU ({adapter}): skipping");
        return;
    }
    eprintln!("GPU viewer on {:?}", h.state().gpu_adapter());
    h.state_mut().session.execute_checked("prop.set", json!({"path": "effects/#1/matteOnly", "value": true})).unwrap();
    arm(&mut h, &ids);
    let mut checked = 0;
    for res in [Resolution::Full, Resolution::Half, Resolution::Third, Resolution::Quarter] {
        let cid = h.state().session.active_comp_id().unwrap();
        h.state_mut().session.set_resolution(cid, res).unwrap();
        settle(&mut h);
        let scale = shown_scale(&h);
        for p in [[36.6, 28.7], [37.4, 29.3], [959.7, 539.2], [925.4, 1044.4]] {
            let sp = comp_to_screen(&h.ctx, [p[0] as f32, p[1] as f32]).unwrap();
            let got = hover(&mut h, sp).trim_end_matches(" (selected)").to_string();
            let q = to_comp(&h, sp);
            assert!(got.starts_with('c') && lit(&mut h, &got, q, scale) >= 0.5, "{res:?}: at {q:?} hover {got:?} is not lit there");
            checked += 1;
        }
    }
    let sp = comp_to_screen(&h.ctx, [959.7, 539.2]).unwrap();
    let named = hover(&mut h, sp);
    // The highlight is drawn: a pixel of the hovered ID (just up-left of the pointer) is tinted
    // while hovered, not with the pointer off the layer (the frame itself unchanged).
    settle(&mut h);
    let px = |h: &mut Harness<'_, EffectcraftApp>| *h.render().expect("render").get_pixel(sp.x as u32 - 2, sp.y as u32 - 2);
    let lit_px = px(&mut h);
    let off = comp_to_screen(&h.ctx, [-3.0, 540.0]).unwrap_or(pos2(2.0, 2.0));
    hover(&mut h, off);
    let plain = px(&mut h);
    assert_eq!(hover(&mut h, sp), named);
    assert_eq!(px(&mut h), lit_px, "the same frame");
    assert_ne!(lit_px, plain, "the highlight tints the hovered ID");
    click(&mut h, sp, Modifiers::NONE);
    assert_eq!(selection(&h, &ids), named.trim_end_matches(" (selected)"));
    assert_eq!(checked, 16);
    let _ = std::fs::remove_dir_all(&ids.dir);
}

/// How many probes the session built so far, and whether one is kept.
/// Waits until the build thread started after `jobs` jobs has counted itself (it may not have
/// run yet when the pass that spawned it returns).
fn started(h: &Harness<'_, EffectcraftApp>, jobs: u64) {
    for _ in 0..2500 {
        if h.state().session.crypto_probe_jobs() > jobs {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    panic!("no build started");
}

fn builds(h: &Harness<'_, EffectcraftApp>) -> u64 {
    h.state().session.crypto_probe_stats().1
}

/// Clicks made while the IDs are being read (a 4K plate behind a blur; the build is held until
/// the clicks are checked) are all made, in order, once they are.
#[test]
fn every_click_made_while_reading_is_made_in_order() {
    use std::sync::atomic::Ordering;
    let (mut s, ids) = crypto_session("multi", 3840, 2160, true);
    let hold = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let log = ReadLog::default();
    s.footage = std::sync::Arc::new(Held(s.footage.clone(), hold.clone(), log.clone()));
    let mut h = harness(s, vec2(1280.0, 800.0), 1.0, 420.0);
    arm(&mut h, &ids);
    let (a, b) = (comp_to_screen(&h.ctx, [1017.5, 594.5]).unwrap(), comp_to_screen(&h.ctx, [1868.5, 1174.5]).unwrap());
    hold.store(true, Ordering::Release);
    log.lock().unwrap().clear();
    h.event(Event::PointerMoved(a));
    h.run_steps(2);
    wait_held_id_read(&mut h, &log);
    assert_eq!(chip(&h).as_deref(), Some(READING));
    let steps = h.state().session.history.undo.len();
    for (p, m) in [(a, Modifiers::NONE), (b, Modifiers::SHIFT), (a, Modifiers::ALT), (a, Modifiers::SHIFT)] {
        h.event(Event::PointerMoved(p));
        h.event(Event::ModifiersChanged(m));
        h.step();
        for pressed in [true, false] {
            h.event(Event::PointerButton { pos: p, button: PointerButton::Primary, pressed, modifiers: m });
            h.step();
        }
        h.event(Event::ModifiersChanged(Modifiers::NONE));
        h.step();
    }
    assert_eq!(chip(&h).as_deref(), Some(READING), "all four made while reading");
    assert_eq!(selection(&h, &ids), "");
    hold.store(false, Ordering::Release);
    wait_probe(&mut h);
    h.run_steps(3);
    assert_eq!(selection(&h, &ids), "c50_40, c27_20", "replace, add, remove, add, in order");
    assert_eq!(h.state().session.history.undo.len(), steps + 4);
    let _ = std::fs::remove_dir_all(&ids.dir);
}

/// A click that drifts past egui's drag threshold still picks at its press, and the Selection
/// tool under the pick never moves the layer.
#[test]
fn a_drifting_click_picks_and_never_moves_the_layer() {
    let (s, ids) = crypto_session("drag", 320, 180, false);
    let mut h = harness(s, vec2(1400.0, 800.0), 1.0, 420.0);
    arm(&mut h, &ids);
    let a = comp_to_screen(&h.ctx, [50.0, 40.0]).unwrap();
    assert_eq!(hover(&mut h, a), "c1_1");
    let position = |h: &Harness<'_, EffectcraftApp>| {
        format!(
            "{:?}",
            h.state().session.active_comp().unwrap().layer(LayerId(ids.layer)).unwrap().transform().and_then(|t| t.get("position")).map(|p| p.value.clone())
        )
    };
    let (steps, before) = (h.state().session.history.undo.len(), position(&h));
    let drag = |h: &mut Harness<'_, EffectcraftApp>, to: egui::Vec2| {
        h.event(Event::PointerButton { pos: a, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
        h.step();
        for k in 1..=4 {
            h.event(Event::PointerMoved(a + to * k as f32 / 4.0));
            h.step();
        }
        h.event(Event::PointerButton { pos: a + to, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
        h.run_steps(3);
    };
    drag(&mut h, vec2(12.0, 0.0));
    assert_eq!(selection(&h, &ids), "c1_1", "picked at the press");
    assert_eq!(h.state().session.history.undo.len(), steps + 1, "only the pick");
    // A real drag picks nothing and moves nothing either.
    drag(&mut h, vec2(120.0, 40.0));
    assert_eq!((h.state().session.history.undo.len(), position(&h)), (steps + 1, before), "the layer stays put");
    assert_eq!(h.state().ui.status, "Drag cancelled the pick", "not silently");
    assert!(h.state().ui.fx_pick.is_some());
    let _ = std::fs::remove_dir_all(&ids.dir);
}

/// Frame steps on a still (behind a blur that isn't animated) reuse the probe: no re-read.
#[test]
fn frame_steps_on_a_still_reuse_the_probe() {
    let (s, ids) = crypto_session("time", 3840, 2160, true);
    let mut h = harness(s, vec2(1280.0, 800.0), 1.0, 420.0);
    arm(&mut h, &ids);
    let a = comp_to_screen(&h.ctx, [1017.5, 594.5]).unwrap();
    assert_eq!(hover(&mut h, a), "c27_20");
    let n = builds(&h);
    for f in 1..=4 {
        h.state_mut().session.set_time(effectcraft_engine::time::Tick::from_seconds_f64(f as f64 / 24.0));
        h.run_steps(2);
        assert_eq!(chip(&h).as_deref(), Some("c27_20"), "frame {f}");
    }
    assert_eq!(builds(&h), n, "no probe built for the frame steps");
    let _ = std::fs::remove_dir_all(&ids.dir);
}

/// With an animated blur before the Cryptomatte effect each frame needs its own probe: none is
/// built while playing, and a scrub builds one once it settles, not one per frame.
#[test]
fn no_probe_builds_while_playing_and_one_per_settled_scrub() {
    let (mut s, ids) = crypto_session("play", 960, 540, true);
    for (t, v) in [(0.0, 2.0), (1.0, 6.0)] {
        s.execute_checked("prop.addKey", json!({"path": "effects/#1/blurriness", "time": t, "value": v})).unwrap();
    }
    let mut h = harness_dt(s, vec2(1280.0, 800.0), 1.0, 420.0, 0.02);
    arm(&mut h, &ids);
    let a = comp_to_screen(&h.ctx, [260.0, 150.0]).unwrap();
    assert_eq!(hover(&mut h, a), "c7_5");
    let n = builds(&h);
    let set = |h: &mut Harness<'_, EffectcraftApp>, f: i64| h.state_mut().session.set_time(effectcraft_engine::time::Tick::from_seconds_f64(f as f64 / 24.0));
    // Scrubbing: a frame per 20 ms step; nothing built until it stays put for a while.
    // (The viewer names what it shows: a new frame takes a step or two to arrive.)
    let mut read = false;
    for f in 1..=6 {
        set(&mut h, f);
        h.run_steps(2);
        read |= chip(&h).as_deref() == Some(READING);
    }
    assert!(read, "the scrub reached frames without a probe");
    assert_eq!(builds(&h), n, "no build while scrubbing");
    wait_probe(&mut h);
    assert_eq!(chip(&h).as_deref(), Some("c7_5"));
    assert_eq!(builds(&h), n + 1, "one build for the settled frame");
    // Playing: no builds, the chip says why; stopped, it reads.
    h.state_mut().playback.playing = true;
    set(&mut h, 9);
    for _ in 0..20 {
        h.step();
    }
    assert_eq!(chip(&h).as_deref(), Some(PAUSED));
    assert_eq!(builds(&h), n + 1, "no build while playing");
    // An audio-only preview plays with the frame held: still no builds.
    let now = h.ctx.input(|i| i.time);
    let pb = &mut h.state_mut().playback;
    (pb.start_wall, pb.start_frame) = (now, 9);
    pb.plan = Some(effectcraft_engine::preview::PreviewPlan {
        start: 0,
        end: 47,
        first: 9,
        step: 1,
        fps: 24.0,
        looping: true,
        cache_first: false,
        video: false,
        audio: true,
        origin: effectcraft_engine::time::Tick::ZERO,
    });
    for _ in 0..20 {
        h.step();
    }
    assert!(h.state().playback.playing);
    assert_eq!(chip(&h).as_deref(), Some(PAUSED));
    assert_eq!(builds(&h), n + 1, "no build while an audio preview holds the frame");
    h.state_mut().playback.plan = None;
    h.state_mut().playback.playing = false;
    h.run_steps(2);
    wait_probe(&mut h);
    assert_eq!(chip(&h).as_deref(), Some("c7_5"));
    assert_eq!(builds(&h), n + 2);
    let _ = std::fs::remove_dir_all(&ids.dir);
}

/// Disarming lets go of the probe (and the aux channels it holds): Esc, the button, or the pick
/// ending any other way.
#[test]
fn disarming_releases_the_probe() {
    let (s, ids) = crypto_session("release", 320, 180, false);
    let mut h = harness(s, vec2(1400.0, 800.0), 1.0, 420.0);
    let a = comp_to_screen(&h.ctx, [50.0, 40.0]).unwrap();
    let kept = |h: &Harness<'_, EffectcraftApp>| h.state().session.crypto_probe_stats().0;
    for how in ["esc", "button", "other"] {
        arm(&mut h, &ids);
        assert_eq!(hover(&mut h, a), "c1_1");
        assert!(kept(&h), "{how}: kept while armed");
        match how {
            "esc" => h.key_press(egui::Key::Escape),
            "button" => {
                let eye = at(&h, &format!("effectControls.prop.{}.eyedropper", ids.sel));
                click(&mut h, eye, Modifiers::NONE);
            }
            _ => h.state_mut().ui.fx_pick = None,
        }
        h.run_steps(2);
        assert!(h.state().ui.fx_pick.is_none() && !kept(&h), "{how}: released");
    }
    let _ = std::fs::remove_dir_all(&ids.dir);
}

/// Space (the temporary Hand) and the Hand and Zoom tools keep their clicks: no pick.
#[test]
fn space_hand_and_zoom_clicks_do_not_pick() {
    let (s, ids) = crypto_session("nav", 320, 180, false);
    let mut h = harness(s, vec2(1400.0, 800.0), 1.0, 420.0);
    arm(&mut h, &ids);
    let a = comp_to_screen(&h.ctx, [50.0, 40.0]).unwrap();
    assert_eq!(hover(&mut h, a), "c1_1");
    let space = |pressed| Event::Key { key: egui::Key::Space, physical_key: None, pressed, repeat: false, modifiers: Modifiers::NONE };
    h.event(space(true));
    h.step();
    click(&mut h, a, Modifiers::NONE);
    h.event(space(false));
    h.run_steps(2);
    assert_eq!(selection(&h, &ids), "", "Space + click");
    for tool in [Tool::Zoom, Tool::Hand] {
        h.state_mut().ui.tool = tool;
        h.run_steps(2);
        let a = comp_to_screen(&h.ctx, [50.0, 40.0]).unwrap();
        click(&mut h, a, Modifiers::NONE);
        assert_eq!(selection(&h, &ids), "", "{tool:?} click");
    }
    h.state_mut().ui.tool = Tool::Selection;
    h.run_steps(2);
    // The middle button pans, armed as unarmed; the right button opens the viewer's menu.
    let a = comp_to_screen(&h.ctx, [50.0, 40.0]).unwrap();
    let pan0 = h.state().ui.viewer.pan;
    h.event(Event::PointerMoved(a));
    h.run_steps(2);
    h.event(Event::PointerButton { pos: a, button: PointerButton::Middle, pressed: true, modifiers: Modifiers::NONE });
    h.step();
    for k in 1..=5 {
        h.event(Event::PointerMoved(a + vec2(10.0 * k as f32, 0.0)));
        h.step();
    }
    h.event(Event::PointerButton { pos: a + vec2(50.0, 0.0), button: PointerButton::Middle, pressed: false, modifiers: Modifiers::NONE });
    h.run_steps(3);
    let pan = h.state().ui.viewer.pan;
    assert!((pan[0] - pan0[0] - 50.0).abs() < 1.0 && (pan[1] - pan0[1]).abs() < 1.0, "a middle drag pans: {pan0:?} -> {pan:?}");
    assert!(h.state().ui.fx_pick.is_some() && selection(&h, &ids).is_empty(), "and picks nothing");
    let a = comp_to_screen(&h.ctx, [50.0, 40.0]).unwrap();
    h.event(Event::PointerMoved(a));
    h.run_steps(2);
    for pressed in [true, false] {
        h.event(Event::PointerButton { pos: a, button: PointerButton::Secondary, pressed, modifiers: Modifiers::NONE });
        h.step();
    }
    h.run_steps(3);
    assert!(egui::Popup::is_any_open(&h.ctx), "a right-click opens the viewer's context menu");
    assert!(h.state().ui.fx_pick.is_some() && selection(&h, &ids).is_empty(), "and picks nothing");
    h.key_press(egui::Key::Escape);
    h.run_steps(3);
    assert!(!egui::Popup::is_any_open(&h.ctx));
    let a = comp_to_screen(&h.ctx, [50.0, 40.0]).unwrap();
    click(&mut h, a, Modifiers::NONE);
    assert_eq!(selection(&h, &ids), "c1_1", "the Selection tool picks");
    let _ = std::fs::remove_dir_all(&ids.dir);
}

/// A layer without Cryptomatte channels says so on hover, in a chip no wider than allowed (the
/// long message ends in an ellipsis; the whole text stays in its automation label).
#[test]
fn a_layer_without_cryptomatte_shows_the_error_in_a_bounded_chip() {
    let (mut s, ids) = crypto_session("nocrypto", 320, 180, false);
    let solid = s.execute("layer.newSolid", json!({"name": "Plain", "width": 320, "height": 180})).unwrap()["layer"].as_u64().unwrap();
    s.state.selected_layers = vec![LayerId(solid)];
    s.execute_checked("effect.apply", json!({"layers": [solid], "effect": "Cryptomatte"})).unwrap();
    let sel = s.active_comp().unwrap().layer(LayerId(solid)).unwrap().effects().unwrap().groups().next().unwrap().get("selection").unwrap().uid;
    let mut h = harness(s, vec2(1400.0, 800.0), 1.0, 420.0);
    arm(&mut h, &Ids { layer: solid, sel, footage: ids.footage, dir: ids.dir.clone() });
    let p = comp_to_screen(&h.ctx, [50.0, 40.0]).unwrap();
    let got = hover(&mut h, p);
    // The message for the user, without the command's prefix, whole (its hint included).
    assert_eq!(got, "This layer has no Cryptomatte channels (a multi-layer OpenEXR with Cryptomatte, or a 3D precomp)");
    let w = h.state().auto.find("viewer.cryptoPick").unwrap().rect[2];
    assert!(w <= 480.0 + 24.0, "chip {w} wide");
    let e = h.state().auto.find("viewer.cryptoPick.error").expect("shown as an error");
    assert_eq!(e.label, "whole", "the error isn't cut off");
    // A narrow viewer still shows it whole, on more lines.
    h.set_size(vec2(760.0, 800.0));
    h.run_steps(4);
    let e = h.state().auto.find("viewer.cryptoPick.error").expect("shown as an error").clone();
    assert!(e.label == "whole" && e.rect[3] > 30.0, "{e:?}");
    // Narrower still (a viewer about 200 px wide): more lines, still whole.
    h.set_size(vec2(630.0, 800.0));
    h.run_steps(4);
    let q = comp_to_screen(&h.ctx, [50.0, 40.0]).unwrap();
    hover(&mut h, q);
    let e = h.state().auto.find("viewer.cryptoPick.error").expect("shown as an error").clone();
    assert_eq!(e.label, "whole", "{e:?} in a viewer {} wide", h.state().auto.find("viewer.fxPick").map_or(0.0, |v| v.rect[2]));
    // A click there: the same message in the status line.
    click(&mut h, q, Modifiers::NONE);
    assert!(h.state().ui.status.starts_with("This layer has no Cryptomatte channels"), "{}", h.state().ui.status);
    // A hover with an answer isn't an error.
    let _ = std::fs::remove_dir_all(&ids.dir);
}

/// Footage whose Cryptomatte reader panics on the probe's thread.
struct Broken(std::sync::Arc<dyn effectcraft_engine::render::FootageSource>);
impl effectcraft_engine::render::FootageSource for Broken {
    fn frame(
        &self,
        i: effectcraft_engine::project::ItemId,
        f: &effectcraft_engine::project::Footage,
        t: effectcraft_engine::time::Tick,
    ) -> Option<std::sync::Arc<effectcraft_engine::raster::Image>> {
        self.0.frame(i, f, t)
    }
    fn frame_with(
        &self,
        i: effectcraft_engine::project::ItemId,
        f: &effectcraft_engine::project::Footage,
        t: effectcraft_engine::time::Tick,
        size: Option<(u32, u32)>,
        float: effectcraft_engine::render::FloatCurve,
    ) -> Option<std::sync::Arc<effectcraft_engine::raster::Image>> {
        self.0.frame_with(i, f, t, size, float)
    }
    fn aux(
        &self,
        i: effectcraft_engine::project::ItemId,
        f: &effectcraft_engine::project::Footage,
        t: effectcraft_engine::time::Tick,
    ) -> Option<std::sync::Arc<effectcraft_engine::raster::AuxChannels>> {
        assert_ne!(std::thread::current().name(), Some("crypto-probe"), "a broken Cryptomatte reader");
        self.0.aux(i, f, t)
    }
}

/// A panic while the probe is built shows as an error in the chip, and the pick keeps working.
#[test]
fn a_panicking_probe_build_shows_an_error() {
    let (mut s, ids) = crypto_session("panic", 320, 180, false);
    s.footage = std::sync::Arc::new(Broken(s.footage.clone()));
    let mut h = harness(s, vec2(1400.0, 800.0), 1.0, 420.0);
    arm(&mut h, &ids);
    let p = comp_to_screen(&h.ctx, [50.0, 40.0]).unwrap();
    let got = hover(&mut h, p);
    assert!(got.contains("Reading the Cryptomatte IDs failed") && got.contains("a broken Cryptomatte reader"), "{got}");
    let p = comp_to_screen(&h.ctx, [120.0, 100.0]).unwrap();
    assert_eq!(hover(&mut h, p), got, "not retried for the frame");
    assert!(h.state().ui.fx_pick.is_some());
    let _ = std::fs::remove_dir_all(&ids.dir);
}

/// A long Selection cut off by a narrow Effect Controls panel shows whole on hover.
#[test]
fn a_cut_off_selection_shows_whole_on_hover() {
    use egui_kittest::kittest::Queryable;
    let (mut s, ids) = crypto_session("tip", 320, 180, false);
    let long = (0..8).map(|i| format!("c{i}_{i}")).collect::<Vec<_>>().join(", ");
    s.execute_checked("prop.set", json!({"path": "effects/#1/selection", "value": long})).unwrap();
    let mut h = harness(s, vec2(1100.0, 800.0), 1.0, 300.0);
    let field = at(&h, &format!("effectControls.prop.{}.value", ids.sel));
    h.event(Event::PointerMoved(field));
    h.run_steps(8);
    assert!(h.query_by_label(&long).is_some(), "the whole Selection on hover");
    let _ = std::fs::remove_dir_all(&ids.dir);
}

/// The highlight's screen corners (its automation label), when one is drawn.
fn highlight(h: &Harness<'_, EffectcraftApp>) -> Option<Vec<Pos2>> {
    let e = h.state().auto.find("viewer.cryptoPick.highlight")?;
    Some(e.label.split(' ').map(|c| c.split_once(',').map(|(x, y)| pos2(x.parse().unwrap(), y.parse().unwrap())).unwrap()).collect())
}

/// The hovered ID's highlight covers the layer as the viewer shows it: unscaled, and scaled and
/// rotated; none off the IDs.
#[test]
fn the_highlight_covers_the_layer_as_shown() {
    let (s, ids) = crypto_session("quad", 320, 180, false);
    let mut h = harness(s, vec2(1400.0, 800.0), 1.0, 420.0);
    arm(&mut h, &ids);
    let corners = [[0.0, 0.0], [320.0, 0.0], [320.0, 180.0], [0.0, 180.0]];
    for (scale, rot) in [(100.0, 0.0_f64), (50.0, 30.0), (150.0, -10.0)] {
        let s = &mut h.state_mut().session;
        s.execute_checked("prop.set", json!({"path": "transform/scale", "value": [scale, scale]})).unwrap();
        s.execute_checked("prop.set", json!({"path": "transform/rotation", "value": rot})).unwrap();
        settle(&mut h);
        // Layer point → comp: about the anchor (the centre), at the position (the comp's centre).
        let (k, (sn, cs)) = (scale / 100.0, rot.to_radians().sin_cos());
        let comp = |p: [f64; 2]| {
            let (x, y) = ((p[0] - 160.0) * k, (p[1] - 90.0) * k);
            [(160.0 + x * cs - y * sn) as f32, (90.0 + x * sn + y * cs) as f32]
        };
        let a = comp_to_screen(&h.ctx, comp([50.0, 40.0])).unwrap();
        assert_eq!(hover(&mut h, a).trim_end_matches(" (selected)"), "c1_1", "scale {scale} rotation {rot}");
        let got = highlight(&h).expect("a highlight");
        for (g, c) in got.iter().zip(corners) {
            let want = comp_to_screen(&h.ctx, comp(c)).unwrap();
            assert!(g.distance(want) < 0.6, "scale {scale} rotation {rot}: corner {c:?} at {g:?}, the layer's at {want:?}");
        }
    }
    let s = &mut h.state_mut().session;
    s.execute_checked("prop.set", json!({"path": "transform/scale", "value": [100.0, 100.0]})).unwrap();
    s.execute_checked("prop.set", json!({"path": "transform/rotation", "value": 0.0})).unwrap();
    settle(&mut h);
    let off = comp_to_screen(&h.ctx, [-3.0, 90.0]).unwrap();
    assert_eq!(hover(&mut h, off), HINT);
    assert!(highlight(&h).is_none(), "no highlight off the IDs");
    let _ = std::fs::remove_dir_all(&ids.dir);
}

/// A click waits for its frame's probe (the build has started and is held), then the frame steps
/// with the pointer kept on the viewer (each frame its own probe: an animated blur). The hover
/// and the waiting click never take turns cancelling each other's build: the click, for a frame
/// no longer shown, is dropped with a message, the hover reads the new frame once the held build
/// ends, and a new click picks, all within a few builds and without a thread per pass.
#[test]
fn a_frame_step_while_a_click_waits_settles_within_a_few_builds() {
    let (mut s, ids) = crypto_session("livelock", 960, 540, true);
    for (t, v) in [(0.0, 2.0), (1.0, 6.0)] {
        s.execute_checked("prop.addKey", json!({"path": "effects/#1/blurriness", "time": t, "value": v})).unwrap();
    }
    let hold = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    s.footage = std::sync::Arc::new(Held(s.footage.clone(), hold.clone(), Default::default()));
    let mut h = harness_dt(s, vec2(1280.0, 800.0), 1.0, 420.0, 0.02);
    arm(&mut h, &ids);
    let a = comp_to_screen(&h.ctx, [260.0, 150.0]).unwrap();
    set_frame(&mut h, 3);
    hold.store(true, std::sync::atomic::Ordering::Release);
    let before = h.state().session.crypto_probe_jobs();
    h.event(Event::PointerMoved(a));
    h.run_steps(10); // settled: the build for frame 3 starts (and is held)
    assert_eq!(chip(&h).as_deref(), Some(READING));
    started(&h, before);
    let b0 = builds(&h);
    for pressed in [true, false] {
        h.event(Event::PointerButton { pos: a, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE });
        h.step();
    }
    let jobs = |h: &Harness<'_, EffectcraftApp>| h.state().session.crypto_probe_jobs();
    let j0 = jobs(&h);
    set_frame(&mut h, 4);
    let step = |h: &mut Harness<'_, EffectcraftApp>| {
        h.step();
        std::thread::sleep(std::time::Duration::from_millis(5));
    };
    // 100 passes with the pointer on the viewer while frame 3's build is held.
    for _ in 0..100 {
        step(&mut h);
    }
    assert_eq!(chip(&h).as_deref(), Some(READING));
    assert_eq!((builds(&h), jobs(&h)), (b0, j0), "nothing else started while one build runs");
    // (Asked now: a status message goes after 5 s.)
    assert!(h.state().ui.status.starts_with("Pick not made"), "the click for frame 3 says it wasn't made: {:?}", h.state().ui.status);
    hold.store(false, std::sync::atomic::Ordering::Release);
    for _ in 0..1000 {
        step(&mut h);
        if chip(&h).as_deref() != Some(READING) {
            break;
        }
    }
    assert_eq!(chip(&h).as_deref(), Some("c7_5"), "frame 4's IDs are read");
    assert_eq!(selection(&h, &ids), "", "the click for frame 3 isn't made on frame 4");
    for pressed in [true, false] {
        h.event(Event::PointerButton { pos: a, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE });
        h.step();
    }
    for _ in 0..500 {
        step(&mut h);
        if !selection(&h, &ids).is_empty() {
            break;
        }
    }
    assert_eq!(selection(&h, &ids), "c7_5", "the click on frame 4 picks");
    // Frame 4's build, and no thread that did nothing.
    let (n, j) = (builds(&h) - b0, jobs(&h) - j0);
    assert!(n <= 2 && j == n, "{n} builds, {j} jobs (threads) after frame 3's");
    // Then nothing more is started.
    for _ in 0..50 {
        step(&mut h);
    }
    assert_eq!((builds(&h) - b0, jobs(&h) - j0), (n, j));
    let _ = std::fs::remove_dir_all(&ids.dir);
}

/// Footage whose Cryptomatte read on the probe's thread waits while `hold` is set. It logs the
/// probe threads' reads: (thread, a picture frame rather than the IDs).
pub(crate) struct Held(
    pub(crate) std::sync::Arc<dyn effectcraft_engine::render::FootageSource>,
    pub(crate) std::sync::Arc<std::sync::atomic::AtomicBool>,
    pub(crate) ReadLog,
);
pub(crate) type ReadLog = std::sync::Arc<std::sync::Mutex<Vec<(std::thread::ThreadId, bool)>>>;
impl Held {
    fn log(&self, frame: bool) {
        if std::thread::current().name() == Some("crypto-probe") {
            self.2.lock().unwrap().push((std::thread::current().id(), frame));
        }
    }
}
impl effectcraft_engine::render::FootageSource for Held {
    fn frame(
        &self,
        i: effectcraft_engine::project::ItemId,
        f: &effectcraft_engine::project::Footage,
        t: effectcraft_engine::time::Tick,
    ) -> Option<std::sync::Arc<effectcraft_engine::raster::Image>> {
        self.log(true);
        self.0.frame(i, f, t)
    }
    fn frame_with(
        &self,
        i: effectcraft_engine::project::ItemId,
        f: &effectcraft_engine::project::Footage,
        t: effectcraft_engine::time::Tick,
        size: Option<(u32, u32)>,
        float: effectcraft_engine::render::FloatCurve,
    ) -> Option<std::sync::Arc<effectcraft_engine::raster::Image>> {
        self.log(true);
        self.0.frame_with(i, f, t, size, float)
    }
    fn aux(
        &self,
        i: effectcraft_engine::project::ItemId,
        f: &effectcraft_engine::project::Footage,
        t: effectcraft_engine::time::Tick,
    ) -> Option<std::sync::Arc<effectcraft_engine::raster::AuxChannels>> {
        self.log(false);
        while std::thread::current().name() == Some("crypto-probe") && self.1.load(std::sync::atomic::Ordering::Acquire) {
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        self.0.aux(i, f, t)
    }
}

/// Step until a held probe build has reached the ID (`aux`) read, so READING asserts are about a
/// build that is really running.
pub(crate) fn wait_held_id_read(h: &mut Harness<'_, EffectcraftApp>, log: &ReadLog) {
    for _ in 0..3000 {
        h.step();
        if log.lock().unwrap().iter().any(|(_, frame)| !*frame) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    panic!("the held build never reached the ID read");
}

/// Disarmed and armed again while a probe is built: the new hover waits for that build to end
/// before it starts its own (one build at a time, so one probe's memory).
#[test]
fn rearming_during_a_build_never_runs_two_builds() {
    let (mut s, ids) = crypto_session("rearm", 320, 180, false);
    let hold = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    s.footage = std::sync::Arc::new(Held(s.footage.clone(), hold.clone(), Default::default()));
    let mut h = harness(s, vec2(1400.0, 800.0), 1.0, 420.0);
    arm(&mut h, &ids);
    let a = comp_to_screen(&h.ctx, [50.0, 40.0]).unwrap();
    hold.store(true, std::sync::atomic::Ordering::Release);
    let before = h.state().session.crypto_probe_jobs();
    h.event(Event::PointerMoved(a));
    h.run_steps(2);
    assert_eq!(chip(&h).as_deref(), Some(READING));
    started(&h, before);
    let b1 = builds(&h);
    h.key_press(egui::Key::Escape);
    h.run_steps(2);
    assert!(h.state().ui.fx_pick.is_none());
    arm(&mut h, &ids);
    h.event(Event::PointerMoved(a));
    for _ in 0..20 {
        h.step();
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(chip(&h).as_deref(), Some(READING));
    assert_eq!(builds(&h), b1, "a second build started while the first runs");
    hold.store(false, std::sync::atomic::Ordering::Release);
    assert_eq!(hover(&mut h, a), "c1_1");
    assert_eq!(builds(&h), b1 + 1, "then one build for the new pick");
    let _ = std::fs::remove_dir_all(&ids.dir);
}

/// Esc while typing in the Selection field ends the typing (the text goes back), not the pick.
#[test]
fn esc_in_the_selection_field_keeps_the_pick() {
    let (s, ids) = crypto_session("escfield", 320, 180, false);
    let mut h = harness(s, vec2(1400.0, 800.0), 1.0, 420.0);
    arm(&mut h, &ids);
    let field = at(&h, &format!("effectControls.prop.{}.value", ids.sel));
    click(&mut h, field, Modifiers::NONE);
    h.event(Event::Text("c9_9".into()));
    h.run_steps(2);
    h.key_press(egui::Key::Escape);
    h.run_steps(2);
    assert!(h.state().ui.fx_pick.is_some(), "still armed");
    assert_eq!(selection(&h, &ids), "", "the typing was dropped");
    h.key_press(egui::Key::Escape);
    h.run_steps(2);
    assert!(h.state().ui.fx_pick.is_none(), "Esc outside the field ends the pick");
    let _ = std::fs::remove_dir_all(&ids.dir);
}

/// A build cancelled while it reads the IDs (the pick turned off, or another project opened)
/// stops there: it doesn't go on to render the effect's input (a 4K plate behind a blur), so
/// the next pick's build, waiting for it (one at a time), starts at once.
#[test]
fn a_cancelled_build_does_not_keep_the_next_pick_waiting() {
    for how in ["disarm", "open"] {
        let (mut s, ids) = crypto_session(&format!("cancel-{how}"), 3840, 2160, true);
        let (hold, log) = (std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)), ReadLog::default());
        s.footage = std::sync::Arc::new(Held(s.footage.clone(), hold.clone(), log.clone()));
        let mut h = harness(s, vec2(1280.0, 800.0), 1.0, 420.0);
        arm(&mut h, &ids);
        let a = comp_to_screen(&h.ctx, [1017.5, 594.5]).unwrap();
        hold.store(true, std::sync::atomic::Ordering::Release);
        let before = h.state().session.crypto_probe_jobs();
        h.event(Event::PointerMoved(a));
        h.run_steps(2);
        started(&h, before);
        while log.lock().unwrap().is_empty() {
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        let held = log.lock().unwrap()[0].0;
        match how {
            "disarm" => h.key_press(egui::Key::Escape),
            _ => {
                let p = (*h.state().session.project).clone();
                h.state_mut().session.replace_project(p, None);
                h.state_mut().session.state.selected_layers = vec![LayerId(ids.layer)];
            }
        }
        h.run_steps(2);
        assert!(h.state().ui.fx_pick.is_none(), "{how}: the pick ended");
        arm(&mut h, &ids);
        h.event(Event::PointerMoved(a));
        h.run_steps(2);
        assert_eq!(chip(&h).as_deref(), Some(READING), "{how}: waits for the cancelled build");
        let jobs = h.state().session.crypto_probe_jobs();
        hold.store(false, std::sync::atomic::Ordering::Release);
        assert_eq!(hover(&mut h, a), "c27_20", "{how}");
        assert_eq!(h.state().session.crypto_probe_jobs(), jobs + 1, "{how}: one more build");
        let frames = log.lock().unwrap().iter().filter(|(t, f)| *t == held && *f).count();
        assert_eq!(frames, 0, "{how}: the cancelled build rendered its input");
        let _ = std::fs::remove_dir_all(&ids.dir);
    }
}

/// While playing, a frame whose IDs aren't read yet can't be picked: the chip says to stop, and
/// a click there says it wasn't made, at once, instead of waiting for a frame that is gone. A
/// frame whose IDs are read (here the one shown before playing) still picks while playing.
#[test]
fn a_click_while_playing_says_to_stop_playback() {
    let (mut s, ids) = crypto_session("playclick", 960, 540, true);
    for (t, v) in [(0.0, 2.0), (1.0, 6.0)] {
        s.execute_checked("prop.addKey", json!({"path": "effects/#1/blurriness", "time": t, "value": v})).unwrap();
    }
    let mut h = harness_dt(s, vec2(1280.0, 800.0), 1.0, 420.0, 0.02);
    arm(&mut h, &ids);
    let a = comp_to_screen(&h.ctx, [260.0, 150.0]).unwrap();
    assert_eq!(hover(&mut h, a), "c7_5");
    let (steps, n) = (h.state().session.history.undo.len(), builds(&h));
    // Playing with the frame held at 9 (an audio-only preview), so a queued click could be made.
    let play = |h: &mut Harness<'_, EffectcraftApp>| {
        set_frame(h, 9);
        let now = h.ctx.input(|i| i.time);
        let pb = &mut h.state_mut().playback;
        (pb.playing, pb.start_wall, pb.start_frame) = (true, now, 9);
        let origin = effectcraft_engine::time::Tick::ZERO;
        let plan = effectcraft_engine::preview::PreviewPlan {
            start: 0,
            end: 47,
            first: 9,
            step: 1,
            fps: 24.0,
            looping: true,
            cache_first: false,
            video: false,
            audio: true,
            origin,
        };
        pb.plan = Some(plan);
    };
    let stop = |h: &mut Harness<'_, EffectcraftApp>| (h.state_mut().playback.plan, h.state_mut().playback.playing) = (None, false);
    play(&mut h);
    for _ in 0..500 {
        h.step();
        if chip(&h).as_deref() == Some(PAUSED) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    assert_eq!(chip(&h).as_deref(), Some(PAUSED));
    for pressed in [true, false] {
        h.event(Event::PointerButton { pos: a, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE });
        h.step();
    }
    assert_eq!(h.state().ui.status, "Pick not made: stop playback to pick", "said at the click");
    // Not queued: stopped on the same frame, its IDs are read and nothing is picked.
    stop(&mut h);
    h.run_steps(2);
    wait_probe(&mut h);
    h.run_steps(4);
    assert_eq!(chip(&h).as_deref(), Some("c7_5"));
    assert_eq!((selection(&h, &ids), h.state().session.history.undo.len(), builds(&h)), (String::new(), steps, n + 1));
    // Playing over a frame whose IDs are read: the click picks.
    play(&mut h);
    h.run_steps(4);
    assert_eq!(chip(&h).as_deref(), Some("c7_5"));
    for pressed in [true, false] {
        h.event(Event::PointerButton { pos: a, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE });
        h.step();
    }
    h.run_steps(2);
    assert_eq!((selection(&h, &ids), h.state().session.history.undo.len()), ("c7_5".to_string(), steps + 1));
    stop(&mut h);
    let _ = std::fs::remove_dir_all(&ids.dir);
}

/// A 320 × 180 still whose probe builds are held (`hold`), the eyedropper armed and reading.
fn held_session(tag: &str) -> (Harness<'static, EffectcraftApp>, Ids, std::sync::Arc<std::sync::atomic::AtomicBool>) {
    let (mut s, ids) = crypto_session(tag, 320, 180, false);
    let hold = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    s.footage = std::sync::Arc::new(Held(s.footage.clone(), hold.clone(), Default::default()));
    let mut h = harness(s, vec2(1400.0, 800.0), 1.0, 420.0);
    arm(&mut h, &ids);
    hold.store(true, std::sync::atomic::Ordering::Release);
    let before = h.state().session.crypto_probe_jobs();
    h.event(Event::PointerMoved(comp_to_screen(&h.ctx, [50.0, 40.0]).unwrap()));
    h.run_steps(2);
    started(&h, before);
    (h, ids, hold)
}

/// Lets the held builds go and runs passes for a second, then returns whether one said a pick
/// wasn't made, and the undo labels.
fn let_go(h: &mut Harness<'static, EffectcraftApp>, hold: &std::sync::atomic::AtomicBool) -> (bool, Vec<String>) {
    hold.store(false, std::sync::atomic::Ordering::Release);
    let mut said = false;
    for _ in 0..200 {
        h.step();
        said |= h.state().ui.status.starts_with("Pick not made");
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    (said, h.state().session.history.undo.iter().map(|(l, _)| l.clone()).collect())
}

fn button(h: &mut Harness<'static, EffectcraftApp>, pos: Pos2, pressed: bool, modifiers: Modifiers) {
    h.event(Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers });
}

/// A click is the frame and the project shown at its press: a frame step or an edit before its
/// release (an arrow key or PageDown that the window delivers before the release, Windows
/// round 3) drops it and says so, and it isn't made afterwards. Keys with the release, after
/// the click, drop it too. Without a change it is made (the control).
#[test]
fn a_click_is_dropped_when_the_frame_or_project_changes_before_its_release_or_ids() {
    type Change = Box<dyn Fn(&mut Harness<'static, EffectcraftApp>)>;
    let exec = |c: &'static str, p: serde_json::Value| -> Change { Box::new(move |h| drop(h.state_mut().session.execute_checked(c, p.clone()).unwrap())) };
    let key = |key: egui::Key| -> Change {
        Box::new(move |h| {
            for pressed in [true, false] {
                h.event(Event::Key { key, physical_key: None, pressed, repeat: false, modifiers: Modifiers::NONE });
            }
        })
    };
    let cases: Vec<(&str, Option<Change>, bool)> = vec![
        ("control", None, false),
        ("time.nextFrame before the release", Some(exec("time.nextFrame", json!({}))), false),
        ("layer.nudge before the release", Some(exec("layer.nudge", json!({"x": 2}))), false),
        ("PageDown with the release", Some(key(egui::Key::PageDown)), true),
        ("Right with the release", Some(key(egui::Key::ArrowRight)), true),
    ];
    // And with the key in the pass whose end makes the click (its IDs stored just before).
    for k in [egui::Key::PageDown, egui::Key::ArrowRight] {
        let (mut h, ids, hold) = held_session(&format!("atomic-arrive-{k:?}"));
        let a = comp_to_screen(&h.ctx, [50.0, 40.0]).unwrap();
        for pressed in [true, false] {
            button(&mut h, a, pressed, Modifiers::NONE);
            h.step();
        }
        hold.store(false, std::sync::atomic::Ordering::Release);
        for _ in 0..2500 {
            if h.state().session.crypto_probe_stats().0 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        assert!(h.state().session.crypto_probe_stats().0, "{k:?}: the probe is stored, no pass since");
        key(k)(&mut h);
        h.step();
        let said = h.state().ui.status.starts_with("Pick not made");
        let (_, labels) = let_go(&mut h, &hold);
        assert_eq!(selection(&h, &ids), "", "{k:?} as the IDs arrive: made ({labels:?})");
        assert!(said && !labels.iter().any(|l| l == "Pick Cryptomatte"), "{k:?}: {labels:?} said {said}");
        let _ = std::fs::remove_dir_all(&ids.dir);
    }
    for (case, change, with_release) in cases {
        let (mut h, ids, hold) = held_session(&format!("atomic-{}", case.len()));
        let a = comp_to_screen(&h.ctx, [50.0, 40.0]).unwrap();
        let (t0, rev0, steps) = (h.state().session.time().seconds(), h.state().session.revision, h.state().session.history.undo.len());
        button(&mut h, a, true, Modifiers::NONE);
        h.step();
        if let Some(c) = change.as_ref().filter(|_| !with_release) {
            c(&mut h);
        }
        button(&mut h, a, false, Modifiers::NONE);
        if let Some(c) = change.as_ref().filter(|_| with_release) {
            c(&mut h);
        }
        h.step();
        let (said, labels) = let_go(&mut h, &hold);
        if change.is_none() {
            assert_eq!((selection(&h, &ids), labels.len(), said), ("c1_1".to_string(), steps + 1, false), "{case}: the click is made");
        } else {
            assert!(h.state().session.time().seconds() != t0 || h.state().session.revision != rev0, "{case}: changed nothing");
            assert_eq!(selection(&h, &ids), "", "{case}: the stale click was made ({labels:?})");
            assert!(!labels.iter().any(|l| l == "Pick Cryptomatte"), "{case}: {labels:?}");
            assert!(said, "{case}: not said");
        }
        let _ = std::fs::remove_dir_all(&ids.dir);
    }
}

/// A waiting click keeps the resolution it was made at: when the viewer then shows the frame at
/// another (a zoom with Auto, or a frame cached at another resolution arriving), it is still
/// made, before the next click (Windows round 3: a plain click lost, 1 run in 51).
#[test]
fn a_waiting_click_is_made_when_the_shown_resolution_changes() {
    let (mut h, ids, hold) = held_session("rescale");
    let a = comp_to_screen(&h.ctx, [50.0, 40.0]).unwrap();
    let (before, steps) = (shown_scale(&h), h.state().session.history.undo.len());
    for pressed in [true, false] {
        button(&mut h, a, pressed, Modifiers::NONE);
        h.step();
    }
    h.state_mut().ui.viewer.zoom = Some(0.3);
    for _ in 0..400 {
        h.step();
        if shown_scale(&h) != before {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_ne!(shown_scale(&h), before, "the shown resolution changes");
    let b = comp_to_screen(&h.ctx, [120.0, 100.0]).unwrap();
    h.event(Event::PointerMoved(b));
    h.event(Event::ModifiersChanged(Modifiers::SHIFT));
    h.step();
    for pressed in [true, false] {
        button(&mut h, b, pressed, Modifiers::SHIFT);
        h.step();
    }
    h.event(Event::ModifiersChanged(Modifiers::NONE));
    let (said, labels) = let_go(&mut h, &hold);
    assert_eq!((selection(&h, &ids), labels.len() - steps, said), ("c1_1, c3_3".to_string(), 2, false), "{labels:?}");
    let _ = std::fs::remove_dir_all(&ids.dir);
}

/// A Shift+click still held when the click before it is made (its build ends while the button
/// is down) is kept: the eyedropper's own pick is not "the project changed".
/// It is made after the first, in order.
#[test]
fn a_click_held_while_the_one_before_is_made_is_kept() {
    let (mut h, ids, hold) = held_session("held-second");
    let (a, b) = (comp_to_screen(&h.ctx, [50.0, 40.0]).unwrap(), comp_to_screen(&h.ctx, [120.0, 100.0]).unwrap());
    let steps = h.state().session.history.undo.len();
    for pressed in [true, false] {
        button(&mut h, a, pressed, Modifiers::NONE);
        h.step();
    }
    h.event(Event::PointerMoved(b));
    h.event(Event::ModifiersChanged(Modifiers::SHIFT));
    h.step();
    button(&mut h, b, true, Modifiers::SHIFT);
    h.step();
    hold.store(false, std::sync::atomic::Ordering::Release);
    for _ in 0..400 {
        if selection(&h, &ids) == "c1_1" {
            break;
        }
        h.step();
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(selection(&h, &ids), "c1_1", "the first click is made while the second is held");
    button(&mut h, b, false, Modifiers::SHIFT);
    h.step();
    let at_release = h.state().ui.status.clone();
    h.event(Event::ModifiersChanged(Modifiers::NONE));
    let (said, labels) = let_go(&mut h, &hold);
    assert_eq!((selection(&h, &ids), labels.len() - steps, said), ("c1_1, c3_3".to_string(), 2, false), "status at the release: {at_release:?}");
    let _ = std::fs::remove_dir_all(&ids.dir);
}

/// A drop clears every waiting click at once: a click made right after it starts a queue of its
/// own (on the new revision) and is made.
#[test]
fn a_drop_clears_the_whole_queue() {
    let (mut h, ids, hold) = held_session("drop-all");
    let (a, b) = (comp_to_screen(&h.ctx, [50.0, 40.0]).unwrap(), comp_to_screen(&h.ctx, [120.0, 100.0]).unwrap());
    let steps = h.state().session.history.undo.len();
    for p in [a, b] {
        for pressed in [true, false] {
            button(&mut h, p, pressed, Modifiers::NONE);
            h.step();
        }
    }
    // The edit drops both; a press in the pass that drops them and its release in the next are
    // the next click's, made on its own (not dropped with a click left over from the queue).
    h.state_mut().session.execute_checked("layer.nudge", json!({"x": 2})).unwrap();
    let c = comp_to_screen(&h.ctx, [120.0, 100.0]).unwrap();
    let ev = &mut h.input_mut().events;
    ev.push(Event::PointerMoved(c));
    ev.push(Event::PointerButton { pos: c, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
    h.step();
    assert!(h.state().ui.status.starts_with("Pick not made"), "{:?}", h.state().ui.status);
    button(&mut h, c, false, Modifiers::NONE);
    h.step();
    let (_, labels) = let_go(&mut h, &hold);
    assert_eq!(selection(&h, &ids), "c3_3", "the click after the drop is made: {labels:?}");
    assert_eq!(labels.iter().filter(|l| *l == "Pick Cryptomatte").count(), 1, "{labels:?} (from {steps})");
    let _ = std::fs::remove_dir_all(&ids.dir);
}

/// The drag distance is the press to the release: the pointer moving on after the release, in
/// the same pass, doesn't cancel the click (Windows round 4, B).
#[test]
fn a_click_measures_its_drag_to_the_release_not_the_pointer_after() {
    let (mut h, ids, hold) = held_session("release-pos");
    let a = comp_to_screen(&h.ctx, [50.0, 40.0]).unwrap();
    button(&mut h, a, true, Modifiers::NONE);
    h.step();
    // In one pass (`Harness::event` would give each event its own pass).
    let ev = &mut h.input_mut().events;
    ev.push(Event::PointerButton { pos: a, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
    ev.push(Event::PointerMoved(a + vec2(80.0, 0.0)));
    h.step();
    let at_release = h.state().ui.status.clone();
    let (said, _) = let_go(&mut h, &hold);
    assert_eq!((selection(&h, &ids), said), ("c1_1".to_string(), false), "status at the release: {at_release:?}");
    assert_ne!(at_release, "Drag cancelled the pick");
    let _ = std::fs::remove_dir_all(&ids.dir);
}

/// Playing, a click whose frame moved on between its press and release says "stop playback"
/// (not that the frame changed), and the message stays its 5 s even though another message
/// (the eyedropper's hint) showed for longer before it (Windows round 4, C).
#[test]
fn a_click_while_playing_says_stop_playback_and_the_message_stays() {
    let (s, _ids) = crypto_session("playmove", 960, 540, true);
    let mut h = harness_dt(s, vec2(1280.0, 800.0), 1.0, 420.0, 0.02);
    arm(&mut h, &_ids);
    let a = comp_to_screen(&h.ctx, [260.0, 150.0]).unwrap();
    h.event(Event::PointerMoved(a));
    let hint = h.state().ui.status.clone();
    assert!(!hint.is_empty(), "the eyedropper's hint shows");
    // The hint has shown for 4.6 s when the click comes.
    let t0 = h.ctx.input(|i| i.time);
    while h.ctx.input(|i| i.time) - t0 < 4.6 {
        h.step();
    }
    assert_eq!(h.state().ui.status, hint);
    play_at(&mut h, 9);
    h.step();
    button(&mut h, a, true, Modifiers::NONE);
    h.step();
    play_at(&mut h, 10);
    button(&mut h, a, false, Modifiers::NONE);
    h.step();
    assert!(h.state().playback.playing);
    assert_eq!(h.state().ui.status, "Pick not made: stop playback to pick", "said at the release");
    let t1 = h.ctx.input(|i| i.time);
    while h.ctx.input(|i| i.time) - t1 < 1.5 {
        h.step();
    }
    assert_eq!(h.state().ui.status, "Pick not made: stop playback to pick", "still shown 1.5 s later");
    let _ = std::fs::remove_dir_all(&_ids.dir);
}

/// A press held still past egui's click time (0.8 s) and let go before any pass sees it held
/// that long is neither a click nor a drag to egui (an idle app doesn't repaint while a mouse
/// button is held still): it is still a click here, and is made.
#[test]
fn a_long_still_press_with_no_pass_in_between_is_a_click() {
    let (mut h, ids, hold) = held_session("long-press");
    let a = comp_to_screen(&h.ctx, [50.0, 40.0]).unwrap();
    button(&mut h, a, true, Modifiers::NONE);
    h.step();
    // Passes 0.25 s apart: the last one held is at 0.75 s, the release at 1.0 s.
    let t0 = h.ctx.input(|i| i.time);
    while h.ctx.input(|i| i.time) - t0 + 0.25 < 0.8 {
        h.step();
    }
    button(&mut h, a, false, Modifiers::NONE);
    h.step();
    let at_release = h.state().ui.status.clone();
    let (said, labels) = let_go(&mut h, &hold);
    assert_eq!((selection(&h, &ids), said), ("c1_1".to_string(), false), "status at the release: {at_release:?}, {labels:?}");
    let _ = std::fs::remove_dir_all(&ids.dir);
}

// Several primary-button events in one pass (Windows round 5, c).
/// An armed eyedropper with the IDs read and hovered at two points (c1_1 and c3_3).
fn ready_at_two_ids(tag: &str) -> (Harness<'static, EffectcraftApp>, Ids, Pos2, Pos2) {
    let (s, ids) = crypto_session(tag, 320, 180, false);
    let mut h = harness(s, vec2(1400.0, 800.0), 1.0, 420.0);
    arm(&mut h, &ids);
    let (a, b) = (comp_to_screen(&h.ctx, [50.0, 40.0]).unwrap(), comp_to_screen(&h.ctx, [120.0, 100.0]).unwrap());
    assert_eq!(hover(&mut h, a), "c1_1");
    assert_eq!(hover(&mut h, b), "c3_3");
    (h, ids, a, b)
}

fn pick_steps(h: &Harness<'_, EffectcraftApp>) -> usize {
    h.state().session.history.undo.iter().filter(|(l, _)| l == "Pick Cryptomatte").count()
}

/// One click's release and the next one's Shift-press in one pass (a loaded UI thread, Windows
/// round 5 c): both are made, in order, the second as an add.
#[test]
fn a_release_and_the_next_press_in_one_pass_are_both_made() {
    let (mut h, ids, a, b) = ready_at_two_ids("batch1");
    h.event(Event::PointerMoved(a));
    h.step();
    h.event(Event::PointerButton { pos: a, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
    h.step();
    let ev = &mut h.input_mut().events;
    ev.push(Event::PointerButton { pos: a, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
    ev.push(Event::PointerMoved(b));
    ev.push(Event::ModifiersChanged(Modifiers::SHIFT));
    ev.push(Event::PointerButton { pos: b, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::SHIFT });
    h.step();
    let mid = (selection(&h, &ids), h.state().ui.status.clone());
    h.event(Event::PointerButton { pos: b, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::SHIFT });
    h.step();
    h.event(Event::ModifiersChanged(Modifiers::NONE));
    h.run_steps(4);
    assert_eq!(selection(&h, &ids), "c1_1, c3_3", "mid {mid:?}");
    let _ = std::fs::remove_dir_all(&ids.dir);
}

/// Two whole clicks in one pass: both are made, two steps.
#[test]
fn two_whole_clicks_in_one_pass_are_both_made() {
    let (mut h, ids, a, b) = ready_at_two_ids("batch2");
    h.event(Event::PointerMoved(a));
    h.step();
    let ev = &mut h.input_mut().events;
    ev.push(Event::PointerButton { pos: a, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
    ev.push(Event::PointerButton { pos: a, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
    ev.push(Event::PointerMoved(b));
    ev.push(Event::ModifiersChanged(Modifiers::SHIFT));
    ev.push(Event::PointerButton { pos: b, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::SHIFT });
    ev.push(Event::PointerButton { pos: b, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::SHIFT });
    h.step();
    h.event(Event::ModifiersChanged(Modifiers::NONE));
    h.run_steps(4);
    assert_eq!((selection(&h, &ids), pick_steps(&h)), ("c1_1, c3_3".to_string(), 2));
    let _ = std::fs::remove_dir_all(&ids.dir);
}

/// Arm, Esc (which clears the status early), re-arm 6 s later: the hint shows again for its own
/// time instead of inheriting the old one's.
#[test]
fn the_hint_shows_again_after_esc_and_rearm() {
    let (s, ids) = crypto_session("hint", 320, 180, false);
    let mut h = harness(s, vec2(1400.0, 800.0), 1.0, 420.0);
    arm(&mut h, &ids);
    let hint = h.state().ui.status.clone();
    assert!(!hint.is_empty());
    h.key_press(egui::Key::Escape);
    h.run_steps(2);
    assert!(h.state().ui.fx_pick.is_none() && h.state().ui.status.is_empty(), "Esc ends it: {:?}", h.state().ui.status);
    h.run_steps(24);
    arm(&mut h, &ids);
    let after: Vec<String> = (0..4)
        .map(|_| {
            h.step();
            h.state().ui.status.clone()
        })
        .collect();
    assert_eq!(after[3], hint, "the hint after re-arming");
    let _ = std::fs::remove_dir_all(&ids.dir);
}

/// One whole click (press and release) in one pass, the IDs read: it is made.
#[test]
fn a_whole_click_in_one_pass_is_made() {
    let (mut h, ids, a, _) = ready_at_two_ids("onepass");
    h.event(Event::PointerMoved(a));
    h.step();
    let ev = &mut h.input_mut().events;
    ev.push(Event::PointerButton { pos: a, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
    ev.push(Event::PointerButton { pos: a, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
    h.step();
    h.run_steps(4);
    assert_eq!((selection(&h, &ids), pick_steps(&h)), ("c1_1".to_string(), 1));
    let _ = std::fs::remove_dir_all(&ids.dir);
}

/// The same while the IDs are being read: the click waits and is made.
#[test]
fn a_whole_click_in_one_pass_waits_while_reading_and_is_made() {
    let (mut h, ids, hold) = held_session("s6-onepass-held");
    let a = comp_to_screen(&h.ctx, [50.0, 40.0]).unwrap();
    let ev = &mut h.input_mut().events;
    ev.push(Event::PointerButton { pos: a, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
    ev.push(Event::PointerButton { pos: a, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
    h.step();
    let (said, labels) = let_go(&mut h, &hold);
    assert_eq!((selection(&h, &ids), said), ("c1_1".to_string(), false), "{labels:?}");
    let _ = std::fs::remove_dir_all(&ids.dir);
}

/// Playback from `frame` (an audio-only preview, so the frame is the one set here).
fn play_at(h: &mut Harness<'_, EffectcraftApp>, frame: i64) {
    set_frame(h, frame);
    let now = h.ctx.input(|i| i.time);
    let pb = &mut h.state_mut().playback;
    (pb.playing, pb.start_wall, pb.start_frame) = (true, now, frame);
    let plan = effectcraft_engine::preview::PreviewPlan {
        start: 0,
        end: 47,
        first: frame,
        step: 1,
        fps: 24.0,
        looping: true,
        cache_first: false,
        video: false,
        audio: true,
        origin: effectcraft_engine::time::Tick::ZERO,
    };
    pb.plan = Some(plan);
}

/// Clicks waiting for their IDs when playback starts are dropped, and the status says to stop
/// playback (not that the frame changed).
#[test]
fn waiting_clicks_dropped_by_playback_say_stop_playback() {
    let (mut s, ids) = crypto_session("play-drop", 320, 180, true);
    let hold = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    s.footage = std::sync::Arc::new(Held(s.footage.clone(), hold.clone(), Default::default()));
    let mut h = harness(s, vec2(1400.0, 800.0), 1.0, 420.0);
    arm(&mut h, &ids);
    hold.store(true, std::sync::atomic::Ordering::Release);
    let before = h.state().session.crypto_probe_jobs();
    let a = comp_to_screen(&h.ctx, [50.0, 40.0]).unwrap();
    h.event(Event::PointerMoved(a));
    h.run_steps(2);
    started(&h, before);
    for pressed in [true, false] {
        button(&mut h, a, pressed, Modifiers::NONE);
        h.step();
    }
    assert!(!h.state().ui.status.starts_with("Pick not made"), "{:?}", h.state().ui.status);
    play_at(&mut h, 9);
    h.step();
    assert_eq!(h.state().ui.status, "Pick not made: stop playback to pick");
    h.state_mut().playback.playing = false;
    let (_, labels) = let_go(&mut h, &hold);
    assert_eq!(selection(&h, &ids), "", "{labels:?}");
    let _ = std::fs::remove_dir_all(&ids.dir);
}

/// Space pressed while the button is held (to pan) cancels the press: nothing is picked at the
/// release.
#[test]
fn space_while_the_button_is_held_cancels_the_click() {
    let (mut h, ids, a, _) = ready_at_two_ids("space-mid");
    h.event(Event::PointerMoved(a));
    h.step();
    button(&mut h, a, true, Modifiers::NONE);
    h.step();
    let space = |pressed| Event::Key { key: egui::Key::Space, physical_key: None, pressed, repeat: false, modifiers: Modifiers::NONE };
    h.event(space(true));
    h.step();
    button(&mut h, a, false, Modifiers::NONE);
    h.step();
    h.event(space(false));
    h.run_steps(4);
    assert_eq!(selection(&h, &ids), "");
    let _ = std::fs::remove_dir_all(&ids.dir);
}

/// A Shift-click whose Shift is let go in the same pass is still an add: the modifiers are the
/// press's own, not the pass's end state.
#[test]
fn a_shift_click_in_one_pass_with_shift_let_go_adds() {
    let (mut h, ids, a, b) = ready_at_two_ids("shift");
    for pressed in [true, false] {
        button(&mut h, a, pressed, Modifiers::NONE);
        h.step();
    }
    h.run_steps(4);
    assert_eq!(selection(&h, &ids), "c1_1");
    h.event(Event::PointerMoved(b));
    h.step();
    let ev = &mut h.input_mut().events;
    ev.push(Event::ModifiersChanged(Modifiers::SHIFT));
    ev.push(Event::PointerButton { pos: b, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::SHIFT });
    ev.push(Event::PointerButton { pos: b, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::SHIFT });
    ev.push(Event::ModifiersChanged(Modifiers::NONE));
    h.step();
    h.run_steps(4);
    assert_eq!((selection(&h, &ids), pick_steps(&h)), ("c1_1, c3_3".to_string(), 2));
    let _ = std::fs::remove_dir_all(&ids.dir);
}

/// The same message set again while it still shows gets its own 5 s: pick, undo 2.5 s later,
/// pick the same ID again (Windows round 6, A1).
#[test]
fn the_same_status_set_again_while_shown_gets_its_own_5_s() {
    let (mut h, ids, a, _) = ready_at_two_ids("status-again");
    for pressed in [true, false] {
        button(&mut h, a, pressed, Modifiers::NONE);
        h.step();
    }
    h.run_steps(2);
    let msg = h.state().ui.status.clone();
    assert_eq!(msg, "Cryptomatte Selection: c1_1");
    h.run_steps(10);
    let _ = h.state_mut().session.execute("edit.undo", json!({}));
    assert_eq!((selection(&h, &ids), h.state().ui.status.clone()), (String::new(), msg.clone()));
    for pressed in [true, false] {
        button(&mut h, a, pressed, Modifiers::NONE);
        h.step();
    }
    h.run_steps(12);
    assert_eq!((selection(&h, &ids), h.state().ui.status.clone()), ("c1_1".to_string(), msg), "3 s after the second pick");
    let _ = std::fs::remove_dir_all(&ids.dir);
}

/// A press, a PageDown and the release in one slow pass: the click is the frame on screen at the
/// press, so it is dropped and says so, not made on the next frame (Windows round 6, R-i pd8).
#[test]
fn a_frame_step_between_press_and_release_in_one_pass_drops_the_click() {
    let (mut h, ids, a, _) = ready_at_two_ids("key-one-pass");
    let key = |pressed| Event::Key { key: egui::Key::PageDown, physical_key: None, pressed, repeat: false, modifiers: Modifiers::NONE };
    let ev = &mut h.input_mut().events;
    ev.push(Event::PointerButton { pos: a, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
    ev.push(key(true));
    ev.push(key(false));
    ev.push(Event::PointerButton { pos: a, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
    h.step();
    assert!(h.state().session.time().seconds() > 0.0, "the PageDown stepped");
    assert!(h.state().ui.status.starts_with("Pick not made"), "{:?}", h.state().ui.status);
    for _ in 0..40 {
        h.step();
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!((selection(&h, &ids), pick_steps(&h)), (String::new(), 0));
    let _ = std::fs::remove_dir_all(&ids.dir);
}

/// The window loses the focus while the button is held (the release may go to another window):
/// the press is dropped, so a later release alone picks nothing.
#[test]
fn losing_the_focus_drops_a_held_press() {
    let (mut h, ids, a, _) = ready_at_two_ids("focus-lost");
    button(&mut h, a, true, Modifiers::NONE);
    h.step();
    h.event(Event::WindowFocused(false));
    h.step();
    h.event(Event::WindowFocused(true));
    h.step();
    button(&mut h, a, false, Modifiers::NONE);
    h.run_steps(4);
    assert_eq!((selection(&h, &ids), pick_steps(&h)), (String::new(), 0));
    let _ = std::fs::remove_dir_all(&ids.dir);
}
