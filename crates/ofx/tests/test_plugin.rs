//! Integration tests against the test OpenFX plug-in (`examples/ofx/ec-test-ofx`).
//!
//! Set `EC_TEST_OFX_BUNDLE` to the built `ec-test-ofx.ofx.bundle` (CI does; see
//! `.github/workflows/ofx-host.yml`). Without it every test prints a note and returns.

use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use effectcraft_effects::plugin::{EffectPlugin, PluginFrame, PluginHost, PluginImage, PluginParamKind, PluginParams, plugin};
use effectcraft_ofx::{LoadReport, load_path};

type Px = [f32; 4];

/// Loads the bundle once per test process (registration is global).
fn report() -> Option<&'static LoadReport> {
    static REPORT: OnceLock<Option<LoadReport>> = OnceLock::new();
    REPORT
        .get_or_init(|| match std::env::var_os("EC_TEST_OFX_BUNDLE") {
            Some(p) => Some(load_path(&PathBuf::from(p))),
            None => {
                println!("note: EC_TEST_OFX_BUNDLE is not set; skipping the OFX integration tests");
                None
            }
        })
        .as_ref()
}

fn effect(ofx_id: &str) -> Option<std::sync::Arc<dyn EffectPlugin>> {
    let r = report()?;
    assert!(r.errors.is_empty(), "load errors: {:?}", r.errors);
    let e = r.loaded.iter().find(|e| e.ofx_id == ofx_id).unwrap_or_else(|| panic!("{ofx_id} did not load: {:?}", r.loaded));
    Some(plugin(&e.id).unwrap_or_else(|| panic!("{} is not registered", e.id)))
}

/// A parameter's id by OFX name (the host may prefix ids with a group path).
fn find<'a>(p: &'a dyn EffectPlugin, name: &str) -> &'a effectcraft_effects::plugin::PluginParam {
    p.manifest()
        .params
        .iter()
        .find(|q| q.id.eq_ignore_ascii_case(name) || q.id.to_lowercase().ends_with(&name.to_lowercase()))
        .unwrap_or_else(|| panic!("no parameter `{name}` in {:?}", p.manifest().params.iter().map(|q| &q.id).collect::<Vec<_>>()))
}

/// Flattened default values and texts, in declaration order. `set` overrides by parameter name.
fn defaults(p: &dyn EffectPlugin, set: &[(&str, &[f64])]) -> (Vec<f64>, Vec<String>) {
    let (mut flat, mut texts) = (vec![], vec![]);
    for q in &p.manifest().params {
        if let Some((_, v)) = set.iter().find(|(n, _)| q.id.to_lowercase().ends_with(&n.to_lowercase())) {
            flat.extend_from_slice(v);
            continue;
        }
        match &q.kind {
            PluginParamKind::Slider { default, .. } => flat.push(*default),
            PluginParamKind::Angle { default } => flat.push(*default),
            PluginParamKind::Checkbox { default } => flat.push(*default as u8 as f64),
            PluginParamKind::Popup { default, .. } => flat.push(*default as f64),
            PluginParamKind::Point { default } => flat.extend(default),
            PluginParamKind::Color { default } => flat.extend(default),
            PluginParamKind::Point3 { default } => flat.extend(default),
            PluginParamKind::Layer => flat.push(-1.0),
            PluginParamKind::Text { default } | PluginParamKind::Hidden { default } => texts.push(default.clone()),
        }
    }
    (flat, texts)
}

/// What a fake host hands the plug-in: the same image for every other time / layer.
#[derive(Default)]
struct FakeHost {
    other: Option<(u32, u32, Vec<Px>)>,
    layer: Option<(u32, u32, Vec<Px>)>,
    fps: f64,
    input_times: Mutex<Vec<f64>>,
    layer_ids: Mutex<Vec<String>>,
}

fn image(i: &(u32, u32, Vec<Px>)) -> PluginImage {
    PluginImage { width: i.0, height: i.1, pixels: i.2.clone(), origin: [0.0; 2], scale: 1.0 }
}

impl PluginHost for FakeHost {
    fn input_at(&self, layer_time: f64) -> Option<PluginImage> {
        self.input_times.lock().unwrap().push(layer_time);
        self.other.as_ref().map(image)
    }
    fn layer_input(&self, param_id: &str, _layer_time: f64) -> Option<PluginImage> {
        self.layer_ids.lock().unwrap().push(param_id.to_string());
        self.layer.as_ref().map(image)
    }
    fn params_at(&self, _layer_time: f64) -> Option<(Vec<f64>, Vec<String>)> {
        None
    }
    fn frame_rate(&self) -> f64 {
        if self.fps > 0.0 { self.fps } else { 30.0 }
    }
    fn time(&self) -> f64 {
        1.0
    }
}

/// Renders a `w`×`h` layer (pixels row 0 = top) the way the engine does: pad by `padding()` on
/// every side, then `render_v2`. Returns the padded frame size, the padding and the pixels.
fn run(p: &dyn EffectPlugin, w: u32, h: u32, layer: &[Px], set: &[(&str, &[f64])], host: &FakeHost) -> (u32, u32, u32, Vec<Px>) {
    let (flat, texts) = defaults(p, set);
    let params = PluginParams { manifest: p.manifest(), flat: &flat, texts: &texts };
    let time = 1.0;
    let pad = p.padding(&params, time, 1.0, [w as f64, h as f64], 0);
    let (fw, fh) = (w + 2 * pad, h + 2 * pad);
    let mut pixels = vec![[0.0; 4]; (fw * fh) as usize];
    for y in 0..h {
        for x in 0..w {
            pixels[((y + pad) * fw + x + pad) as usize] = layer[(y * w + x) as usize];
        }
    }
    let mut frame = PluginFrame { width: fw, height: fh, pixels: &mut pixels, scale: 1.0, origin: [pad as f64; 2], layer_size: [w as f64, h as f64] };
    p.render_v2(&mut frame, &params, time, host).expect("render");
    (fw, fh, pad, pixels)
}

fn close(a: Px, b: Px) -> bool {
    a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-4)
}

#[test]
fn all_effects_register() {
    let Some(r) = report() else { return };
    assert!(r.errors.is_empty(), "load errors: {:?}", r.errors);
    for (ofx_id, ctx) in [
        ("org.effectcraft.test.Invert", "filter"),
        ("org.effectcraft.test.Echo", "filter"),
        ("org.effectcraft.test.Border", "filter"),
        ("org.effectcraft.test.Mix", "general"),
        ("org.effectcraft.test.AllParams", "filter"),
    ] {
        let e = r.loaded.iter().find(|e| e.ofx_id == ofx_id).unwrap_or_else(|| panic!("{ofx_id} missing from {:?}", r.loaded));
        assert_eq!(e.id, ofx_id.to_lowercase(), "effect id");
        assert_eq!(e.category, "EC Test");
        assert!(e.context.to_lowercase().contains(ctx), "{ofx_id}: context {}", e.context);
        let p = plugin(&e.id).expect("registered");
        assert_eq!(p.manifest().id, e.id);
        assert_eq!(p.manifest().category, "EC Test");
    }
}

#[test]
fn param_kinds() {
    let Some(p) = effect("org.effectcraft.test.AllParams") else { return };
    let kind = |n: &str| find(&*p, n).kind.clone();
    assert!(matches!(kind("double"), PluginParamKind::Slider { integer: false, .. }));
    assert!(matches!(kind("int"), PluginParamKind::Slider { integer: true, .. }));
    assert!(matches!(kind("bool"), PluginParamKind::Checkbox { default: true }));
    assert!(matches!(kind("choice"), PluginParamKind::Popup { ref options, default: 1 } if options.len() == 3));
    assert!(matches!(kind("rgba"), PluginParamKind::Color { default } if (default[0] - 0.25).abs() < 1e-9 && default[3] == 1.0));
    // The 2D double is an absolute XY position (a viewer point, canonical y-up default flipped to
    // y-down layer fractions); the 3D double is plain, so each component is a slider.
    assert!(
        matches!(kind("double2d"), PluginParamKind::Point { default } if (default[0] - 10.0 / 1920.0).abs() < 1e-9 && (default[1] - (1.0 - 20.0 / 1080.0)).abs() < 1e-9)
    );
    assert!(matches!(kind("double3d_x"), PluginParamKind::Slider { default, integer: false, .. } if default == 1.0));
    assert!(matches!(kind("double3d_y"), PluginParamKind::Slider { default, integer: false, .. } if default == 2.0));
    assert!(matches!(kind("double3d_z"), PluginParamKind::Slider { default, integer: false, .. } if default == 3.0));
    assert!(matches!(kind("string"), PluginParamKind::Text { ref default } if default == "hello"));
    assert!(matches!(kind("custom"), PluginParamKind::Hidden { ref default } if default == "opaque-data"));
    // It is a pass-through: rendering with the defaults must not change the image.
    let px: Vec<Px> = (0..12).map(|i| [i as f32 / 12.0, 0.5, 0.25, 1.0]).collect();
    let (.., out) = run(&*p, 4, 3, &px, &[], &FakeHost::default());
    assert!(out.iter().zip(&px).all(|(a, b)| close(*a, *b)));
}

#[test]
fn invert_inverts() {
    let Some(p) = effect("org.effectcraft.test.Invert") else { return };
    let amount = find(&*p, "amount");
    assert!(matches!(amount.kind, PluginParamKind::Slider { default, .. } if default == 1.0));
    let src = vec![[0.2, 0.4, 0.6, 1.0], [0.1, 0.2, 0.3, 0.5], [0.0; 4], [1.0; 4]];
    let (_, _, pad, out) = run(&*p, 2, 2, &src, &[], &FakeHost::default());
    assert_eq!(pad, 0);
    // Premultiplied: colour -> alpha - colour; alpha is kept.
    let want = [[0.8, 0.6, 0.4, 1.0], [0.4, 0.3, 0.2, 0.5], [0.0; 4], [0.0, 0.0, 0.0, 1.0]];
    for (g, w) in out.iter().zip(want) {
        assert!(close(*g, w), "got {g:?}, want {w:?}");
    }
    // amount = 0 leaves the image alone.
    let (.., out) = run(&*p, 2, 2, &src, &[("amount", &[0.0])], &FakeHost::default());
    assert!(out.iter().zip(&src).all(|(a, b)| close(*a, *b)));
}

#[test]
fn echo_averages_the_previous_frame() {
    let Some(p) = effect("org.effectcraft.test.Echo") else { return };
    let (w, h) = (4, 3);
    let now = vec![[0.2, 0.2, 0.2, 1.0]; 12];
    let host = FakeHost { other: Some((w, h, vec![[0.6, 0.0, 1.0, 1.0]; 12])), fps: 24.0, ..Default::default() };
    let (.., out) = run(&*p, w, h, &now, &[], &host);
    for g in &out {
        assert!(close(*g, [0.4, 0.1, 0.6, 1.0]), "got {g:?}");
    }
    // One frame back = 1/24 s before the render time (1 s).
    let times = host.input_times.lock().unwrap().clone();
    assert!(times.iter().any(|t| (t - 23.0 / 24.0).abs() < 1e-4), "input_at times: {times:?}");
}

#[test]
fn border_grows_and_paints_red() {
    let Some(p) = effect("org.effectcraft.test.Border") else { return };
    let (w, h) = (6, 4);
    let green = vec![[0.0, 1.0, 0.0, 1.0]; (w * h) as usize];
    let (fw, fh, pad, out) = run(&*p, w, h, &green, &[("size", &[3.0])], &FakeHost::default());
    assert!(pad > 0, "padding() should grow with the RoD");
    let at = |x: u32, y: u32| out[(y * fw + x) as usize];
    for (x, y) in [(0, 0), (fw - 1, fh - 1), (fw - 1, 0), (0, fh - 1)] {
        assert!(close(at(x, y), [1.0, 0.0, 0.0, 1.0]), "corner ({x},{y}) = {:?}", at(x, y));
    }
    // The original pixels stay inside.
    assert!(close(at(pad, pad), [0.0, 1.0, 0.0, 1.0]), "{:?}", at(pad, pad));
    assert!(close(at(pad + w - 1, pad + h - 1), [0.0, 1.0, 0.0, 1.0]));
}

#[test]
fn mix_uses_the_matte_layer() {
    let Some(p) = effect("org.effectcraft.test.Mix") else { return };
    let matte = find(&*p, "matte");
    assert!(matches!(matte.kind, PluginParamKind::Layer), "{:?}", matte.kind);
    let (w, h) = (4, 3);
    let src = vec![[1.0, 1.0, 1.0, 1.0]; 12];
    let host = FakeHost { layer: Some((w, h, vec![[0.0, 0.0, 0.0, 0.5]; 12])), ..Default::default() };
    let (.., out) = run(&*p, w, h, &src, &[("matte", &[7.0])], &host);
    for g in &out {
        assert!(close(*g, [0.5, 0.5, 0.5, 0.5]), "got {g:?}");
    }
    let ids = host.layer_ids.lock().unwrap().clone();
    assert!(ids.iter().any(|i| i.eq_ignore_ascii_case(&matte.id)), "layer_input ids: {ids:?}");
    // Without a matte layer the source passes through.
    let (.., out) = run(&*p, w, h, &src, &[], &FakeHost::default());
    assert!(out.iter().all(|g| close(*g, [1.0; 4])));
}
