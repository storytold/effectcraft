use super::*;
use effectcraft_effects::{EffectCtx, Params};

fn input() -> Buf {
    let mut img = Image::new(4, 3);
    img.set(0, 0, [0.5, 0.0, 0.0, 0.5]);
    img.set(3, 2, [0.0, 1.0, 0.0, 1.0]);
    Buf { img, offset: [2.0, -3.0], scale: 1.0 }
}
fn try_run(input: &Buf, changes: &[(&str, f64)]) -> Result<Buf, String> {
    let plugin = TextBox::new().unwrap();
    let m = plugin.manifest();
    let mut flat = Vec::new();
    for p in &m.params {
        use effectcraft_effects::plugin::PluginParamKind;
        match p.kind {
            PluginParamKind::Slider { default, .. } => flat.push(changes.iter().find(|(id, _)| *id == p.id).map(|(_, v)| *v).unwrap_or(default)),
            PluginParamKind::Color { default } => flat.extend(default),
            PluginParamKind::Checkbox { default } => {
                flat.push(changes.iter().find(|(id, _)| *id == p.id).map(|(_, v)| *v).unwrap_or(if default { 1.0 } else { 0.0 }))
            }
            _ => return Err("unsupported test manifest parameter".into()),
        }
    }
    plugin.render_buffer(input, &PluginParams { manifest: m, flat: &flat }, 0.0)
}
fn run(input: &Buf, changes: &[(&str, f64)]) -> Buf {
    try_run(input, changes).unwrap()
}

fn plate_input() -> Buf {
    let mut img = Image::new(12, 8);
    img.data.fill([0.5, 0.0, 0.0, 0.5]);
    Buf { img, offset: [0.0; 2], scale: 1.0 }
}

#[test]
fn signed_padding_shrinks_the_background_without_clipping_source_by_default() {
    let mut b = plate_input();
    b.img.data.fill([0.0; 4]);
    b.img.set(0, 0, [1.0; 4]);
    b.img.set(11, 7, [1.0; 4]);
    let out = run(&b, &[("paddingX", -2.0), ("paddingY", -1.0)]);
    assert_eq!(layer_pixel(&out, 1.0, 3.0), [0.0; 4]);
    assert_eq!(layer_pixel(&out, 2.0, 3.0), [0.0, 0.0, 0.0, 1.0]);
    assert_eq!(layer_pixel(&out, 10.0, 3.0), [0.0; 4]);
    assert_eq!(layer_pixel(&out, 0.0, 0.0), [1.0; 4]);
}

#[test]
fn matte_clips_source_even_when_fill_is_invisible() {
    let b = plate_input();
    for opacity in [0.0, 25.0, 100.0] {
        let out = run(&b, &[("paddingX", -2.0), ("paddingY", -1.0), ("matte", 1.0), ("fillOpacity", opacity)]);
        assert_eq!(layer_pixel(&out, 0.0, 3.0), [0.0; 4]);
        let inside = layer_pixel(&out, 3.0, 3.0);
        assert_eq!(inside[0], 0.5, "text brightness must not depend on fill opacity");
        if opacity == 0.0 {
            assert_eq!(inside, [0.5, 0.0, 0.0, 0.5]);
        }
    }
}

#[test]
fn matte_respects_signed_side_padding_and_offset() {
    let b = plate_input();
    let out = run(
        &b,
        &[
            ("paddingX", 0.0),
            ("paddingY", 0.0),
            ("paddingLeft", -3.0),
            ("paddingRight", -2.0),
            ("paddingTop", -2.0),
            ("paddingBottom", -1.0),
            ("offsetX", 1.0),
            ("matte", 1.0),
            ("fillOpacity", 0.0),
        ],
    );
    for (x, y) in [(3.0, 3.0), (11.0, 3.0), (5.0, 1.0), (5.0, 7.0)] {
        assert_eq!(layer_pixel(&out, x, y), [0.0; 4]);
    }
    assert_eq!(layer_pixel(&out, 4.0, 2.0), [0.5, 0.0, 0.0, 0.5]);
}

#[test]
fn rounded_matte_multiplies_premultiplied_channels_and_moves_without_text() {
    let b = plate_input();
    let out = run(&b, &[("paddingX", 0.0), ("paddingY", 0.0), ("cornerRadius", 4.0), ("matte", 1.0), ("fillOpacity", 0.0)]);
    assert_eq!(layer_pixel(&out, 0.0, 0.0), [0.0; 4]);
    let edge = layer_pixel(&out, 1.0, 1.0);
    assert!(edge[3] > 0.0 && edge[3] < 0.5);
    assert_eq!(edge[0], edge[3]);
    assert_eq!(layer_pixel(&out, 6.0, 4.0), [0.5, 0.0, 0.0, 0.5]);
    let moved = run(&b, &[("paddingX", 0.0), ("paddingY", 0.0), ("offsetX", 20.0), ("matte", 1.0), ("fillOpacity", 0.0)]);
    assert!(moved.img.data.iter().all(|p| *p == [0.0; 4]));
}

#[test]
fn collapsed_rectangle_is_empty_instead_of_inverted_or_outlined() {
    let b = plate_input();
    for shrink in [-6.0, -100.0] {
        let out = run(&b, &[("paddingX", shrink), ("paddingY", 0.0), ("matte", 1.0), ("strokeWidth", 5.0)]);
        assert!(out.img.data.iter().all(|p| *p == [0.0; 4]));
        let off = run(&b, &[("paddingX", shrink), ("paddingY", 0.0), ("strokeWidth", 5.0)]);
        assert_eq!(off.img, b.img);
        assert_eq!(off.offset, b.offset);
    }
}

#[test]
fn matte_padding_tracks_preview_scale() {
    let mut b = plate_input();
    b.scale = 0.5;
    let out = run(&b, &[("paddingX", -4.0), ("paddingY", -2.0), ("matte", 1.0), ("fillOpacity", 0.0)]);
    assert_eq!(layer_pixel(&out, 2.0, 6.0), [0.0; 4]);
    assert_eq!(layer_pixel(&out, 4.0, 6.0), [0.5, 0.0, 0.0, 0.5]);
}

#[test]
fn padding_controls_accept_negative_values() {
    let plugin = TextBox::new().unwrap();
    for id in ["paddingX", "paddingY", "paddingLeft", "paddingRight", "paddingTop", "paddingBottom"] {
        let p = plugin.manifest().params.iter().find(|p| p.id == id).unwrap();
        let effectcraft_effects::plugin::PluginParamKind::Slider { min, slider_min, .. } = p.kind else { panic!("slider") };
        assert!(min <= -10000.0 && slider_min.unwrap_or(min) < 0.0, "{id}");
    }
}
fn layer_pixel(b: &Buf, x: f64, y: f64) -> [f32; 4] {
    b.img.get((x * b.scale + b.offset[0]) as i64, (y * b.scale + b.offset[1]) as i64)
}
#[test]
fn bounds_include_antialias_and_multiline_gaps() {
    let mut b = input();
    b.img.set(0, 0, [0.0, 0.0, 0.0, 1e-7]);
    assert_eq!(alpha_bounds(&b.img), Some([0.0, 0.0, 4.0, 3.0]));
    b.img.data.fill([0.0; 4]);
    assert_eq!(alpha_bounds(&b.img), None);
    assert_eq!(alpha_bounds(&Image::default()), None);
    b.img.set(1, 1, [0.0, 0.0, 0.0, f32::NAN]);
    assert_eq!(alpha_bounds(&b.img), None);
}
#[test]
fn padding_expands_all_sides_and_preserves_text_coordinates() {
    let b = input();
    let out = run(&b, &[("paddingX", 2.0), ("paddingY", 2.0)]);
    assert!(out.img.width > b.img.width && out.img.height > b.img.height);
    assert_eq!(layer_pixel(&out, -3.0, 2.0), [0.0, 0.0, 0.0, 1.0]);
    // Original half-alpha red over black, not replaced by the box.
    assert_eq!(layer_pixel(&out, -2.0, 3.0), [0.5, 0.0, 0.0, 1.0]);
    assert_eq!(layer_pixel(&out, 1.0, 5.0), [0.0, 1.0, 0.0, 1.0]);
}
#[test]
fn offset_both_directions_never_moves_original_text() {
    let b = input();
    for ox in [-12.0, 12.0] {
        let out = run(&b, &[("paddingX", 0.0), ("paddingY", 0.0), ("offsetX", ox), ("offsetY", ox)]);
        assert_eq!(layer_pixel(&out, -2.0, 3.0), [0.5, 0.0, 0.0, 0.5]);
        assert_eq!(layer_pixel(&out, -1.0 + ox, 4.0 + ox), [0.0, 0.0, 0.0, 1.0]);
    }
}
#[test]
fn blank_input_stays_blank() {
    let mut b = input();
    b.img.data.fill([0.0; 4]);
    let out = run(&b, &[]);
    assert_eq!(out.img, b.img);
    assert_eq!(out.offset, b.offset);
}
#[test]
fn preview_scale_and_asymmetric_padding() {
    let mut b = input();
    b.scale = 0.5;
    let out = run(&b, &[("paddingX", 0.0), ("paddingY", 0.0), ("paddingLeft", 8.0)]);
    assert_eq!(out.offset[0] - b.offset[0], 5.0); // four pixels plus antialias guard
    assert_eq!(out.offset[1] - b.offset[1], 1.0);
}
#[test]
fn radius_stroke_and_fill_opacity() {
    let b = input();
    let out = run(&b, &[("paddingX", 4.0), ("paddingY", 4.0), ("cornerRadius", 4.0), ("fillOpacity", 50.0)]);
    assert_eq!(layer_pixel(&out, -3.0, 4.0), [0.0, 0.0, 0.0, 0.5]);
    assert!(layer_pixel(&out, -6.0, -1.0)[3] < 0.1);
    let line = run(&b, &[("paddingX", 2.0), ("paddingY", 2.0), ("strokeWidth", 2.0), ("fillOpacity", 0.0)]);
    assert_eq!(layer_pixel(&line, -4.0, 4.0), [1.0; 4]);
}
#[test]
fn oversized_and_nonfinite_geometry_returns_error() {
    assert!(try_run(&input(), &[("paddingX", 10000.0), ("paddingY", 10000.0)]).is_err());
    assert!(try_run(&input(), &[("offsetX", f64::NAN)]).is_err());
    let p = TextBox::new().unwrap();
    let mut b = input();
    b.scale = f64::INFINITY;
    assert!(p.render_buffer(&b, &PluginParams { manifest: p.manifest(), flat: &[] }, 0.0).is_err());
    b.scale = 1.0;
    b.img.width = u32::MAX;
    assert!(p.render_buffer(&b, &PluginParams { manifest: p.manifest(), flat: &[] }, 0.0).is_err());
}
#[test]
fn trampoline_dispatches_expanding_plugin() {
    register().unwrap();
    let spec = effectcraft_effects::find(ID).unwrap();
    let mut params = Params::default();
    for p in &spec.params {
        params.values.insert(p.id.into(), p.default.clone());
    }
    let ctx = EffectCtx { params: &params, time: 0.0, layer_size: [4.0, 3.0], seed: 0, adjustment: false, env: Default::default() };
    let out = effectcraft_effects::apply(spec, &ctx, input());
    assert!(out.img.width > 4 && out.img.height > 3);
}

struct Failure(PluginManifest);
impl EffectPlugin for Failure {
    fn manifest(&self) -> &PluginManifest {
        &self.0
    }
    fn render(&self, frame: &mut PluginFrame, _params: &PluginParams, _time: f64) -> Result<(), String> {
        frame.pixels.fill([1.0; 4]);
        Err("synthetic failure after mutation".into())
    }
}
#[test]
fn existing_in_place_api_restores_input_on_error() {
    let mut manifest = TextBox::new().unwrap().0;
    manifest.id = "org.test.textbox-rollback".into();
    let spec = register_plugin(Arc::new(Failure(manifest))).unwrap();
    let b = input();
    let params = Params::default();
    let ctx = EffectCtx { params: &params, time: 0.0, layer_size: [4.0, 3.0], seed: 0, adjustment: false, env: Default::default() };
    let out = effectcraft_effects::apply(spec, &ctx, b.clone());
    assert_eq!(out.img, b.img);
    assert_eq!(out.offset, b.offset);
}

/// Manual microbenchmark: excludes source rasterization and UI/GPU transfer.
#[test]
#[ignore = "manual performance measurement"]
fn benchmark_large_transparent_input() {
    for (w, h) in [(1920, 1080), (3840, 2160)] {
        let mut img = Image::new(w, h);
        for y in h / 2..h / 2 + 60 {
            for x in w / 2..w / 2 + 300 {
                img.set(x, y, [1.0; 4]);
            }
        }
        let b = Buf { img, offset: [0.0; 2], scale: 1.0 };
        let _ = run(&b, &[]); // warm rayon pool
        let start = std::time::Instant::now();
        for _ in 0..5 {
            std::hint::black_box(run(&b, &[]));
        }
        eprintln!("TextBox {w}x{h}: {:.2} ms/frame (5 frames, CPU only)", start.elapsed().as_secs_f64() * 200.0);
    }
}

struct InvalidBuffer {
    m: PluginManifest,
    kind: u8,
}
impl EffectPlugin for InvalidBuffer {
    fn manifest(&self) -> &PluginManifest {
        &self.m
    }
    fn render(&self, _frame: &mut PluginFrame, _params: &PluginParams, _time: f64) -> Result<(), String> {
        Ok(())
    }
    fn render_buffer(&self, input: &Buf, _params: &PluginParams, _time: f64) -> Result<Buf, String> {
        let mut out = input.clone();
        match self.kind {
            0 => out.img.width += 1,
            1 => out.offset[0] = f64::NAN,
            2 => out.scale *= 2.0,
            _ => out.scale = f64::NAN,
        }
        Ok(out)
    }
}
#[test]
fn host_rejects_invalid_buffer_geometry_without_losing_source() {
    for kind in 0..4 {
        let mut m = TextBox::new().unwrap().0;
        m.id = format!("org.test.textbox-invalid-{kind}");
        let spec = register_plugin(Arc::new(InvalidBuffer { m, kind })).unwrap();
        let b = input();
        let params = Params::default();
        let ctx = EffectCtx { params: &params, time: 0.0, layer_size: [4.0, 3.0], seed: 0, adjustment: false, env: Default::default() };
        let out = effectcraft_effects::apply(spec, &ctx, b.clone());
        assert_eq!(out.img, b.img);
        assert_eq!(out.offset, b.offset);
        assert_eq!(out.scale, b.scale);
    }
}
