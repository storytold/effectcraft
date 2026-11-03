use super::*;
use crate::{EffectEnv, EffectHost, LayerPixels, run_fx};
use effectcraft_raster::channels3d::crypto_float;

/// A 20×10 layer: left half object 1 at depth 100, right half object 2 at depth 500; the
/// top-right corner is background. Cryptomatte: "left" / "right".
struct AuxHost(Arc<AuxChannels>);
impl EffectHost for AuxHost {
    fn layer(&self, _: u64, _: bool) -> Option<LayerPixels> {
        None
    }
    fn audio(&self, _: u64, _: f64, _: usize, _: u32) -> Option<Vec<f32>> {
        None
    }
    fn aux(&self) -> Option<Arc<AuxChannels>> {
        Some(self.0.clone())
    }
}

fn scene() -> AuxHost {
    let (w, h) = (20usize, 10usize);
    let mut a = AuxChannels::new(w as u32, h as u32, 1.0);
    let (mut z, mut id, mut r, mut g) = (vec![0.0; w * h], vec![0.0; w * h], vec![0.0; w * h], vec![0.0; w * h]);
    let (hl, hr) = (crypto_hash("left"), crypto_hash("right"));
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            if x >= 15 && y < 3 {
                z[i] = BACKGROUND_DEPTH;
            } else if x < 10 {
                z[i] = 100.0;
                id[i] = 1.0;
                r[i] = crypto_float(hl);
                g[i] = 1.0;
            } else {
                z[i] = 500.0;
                id[i] = 2.0;
                r[i] = crypto_float(hr);
                g[i] = 1.0;
            }
        }
    }
    a.channels = vec![("Z".into(), z), ("ObjectID".into(), id), ("CryptoObject00.R".into(), r), ("CryptoObject00.G".into(), g)];
    a.manifests = vec![("CryptoObject".into(), vec![("left".into(), hl), ("right".into(), hr)])];
    AuxHost(Arc::new(a))
}

fn img() -> Image {
    Image::filled(20, 10, [0.5, 0.5, 0.5, 1.0])
}

fn run(id: &str, vals: &[(&str, Value)], host: &AuxHost) -> Image {
    run_fx(id, vals, img(), 0.0, EffectEnv { host: Some(host), ..Default::default() }).img
}

#[test]
fn passes_through_without_aux_channels() {
    for s in specs() {
        let out = run_fx(s.id, &[], img(), 0.0, EffectEnv::default());
        assert_eq!(out.img.data, img().data, "{}", s.id);
    }
}

#[test]
fn depth_matte_thresholds() {
    let h = scene();
    let o = run("ec.3d.depthmatte", &[("depth", num(300.0))], &h);
    assert_eq!(o.get(2, 5)[3], 0.0);
    assert_eq!(o.get(12, 5)[3], 1.0);
    let inv = run("ec.3d.depthmatte", &[("depth", num(300.0)), ("invert", Value::Bool(true))], &h);
    assert_eq!(inv.get(2, 5)[3], 1.0);
    assert_eq!(inv.get(12, 5)[3], 0.0);
    // Feather: a ramp of 800 units centred on 300 → depth 100 at 0.25, 500 at 0.75.
    let f = run("ec.3d.depthmatte", &[("depth", num(300.0)), ("feather", num(800.0))], &h);
    assert!((f.get(2, 5)[3] - 0.25).abs() < 1e-5 && (f.get(12, 5)[3] - 0.75).abs() < 1e-5);
}

#[test]
fn channel_extract_maps_depth_and_ids() {
    let h = scene();
    let o = run("ec.3d.channelextract", &[("blackPoint", num(0.0)), ("whitePoint", num(1000.0))], &h);
    assert!((o.get(2, 5)[0] - 0.1).abs() < 1e-6 && (o.get(12, 5)[0] - 0.5).abs() < 1e-6);
    assert_eq!(o.get(17, 1)[0], 1.0);
    let ids = run("ec.3d.channelextract", &[("channel", Value::Enum(1)), ("whitePoint", num(2.0))], &h);
    assert_eq!(ids.get(2, 5)[0], 0.5);
    // Invert Depth Map flips the ramp; without Clamp Output values run past 1.
    let inv = run("ec.3d.channelextract", &[("blackPoint", num(0.0)), ("whitePoint", num(1000.0)), ("invertDepthMap", Value::Bool(true))], &h);
    assert!((inv.get(2, 5)[0] - 0.9).abs() < 1e-6);
    let raw = run("ec.3d.channelextract", &[("blackPoint", num(0.0)), ("whitePoint", num(100.0)), ("clampOutput", Value::Bool(false))], &h);
    assert!(raw.get(12, 5)[0] > 1.5);
}

#[test]
fn id_matte_and_identifier() {
    let h = scene();
    let o = run("ec.3d.idmatte", &[("idSelection", num(2.0))], &h);
    assert_eq!(o.get(2, 5)[3], 0.0);
    assert_eq!(o.get(12, 5)[3], 1.0);
    let l = run("ec.3d.identifier", &[("display", Value::Enum(1)), ("id", num(1.0))], &h);
    assert_eq!(l.get(2, 5)[0], 1.0);
    assert_eq!(l.get(12, 5)[0], 0.0);
    let c = run("ec.3d.identifier", &[], &h);
    assert_ne!(c.get(2, 5), c.get(12, 5));
}

/// The parallel aux image (rank planes resolved once) is bit-identical to the per-pixel
/// reduction over `crypto_ranks` it replaces.
#[test]
fn crypto_aux_image_matches_the_per_pixel_reduction() {
    let (w, h) = (7u32, 5u32);
    let n = (w * h) as usize;
    let mut a = AuxChannels::new(w, h, 1.0);
    let ids = [crypto_hash("cube"), crypto_hash("sphere"), crypto_hash("ring")].map(effectcraft_raster::channels3d::crypto_float);
    for (k, ch) in ["00.R", "00.G", "00.B", "00.A", "01.R", "01.G", "01.B", "01.A"].iter().enumerate() {
        let v: Vec<f32> = (0..n).map(|i| if k % 2 == 0 { ids[(i + k) % 3] } else { ((i * 7 + k * 3) % 11) as f32 / 10.0 }).collect();
        a.channels.push((format!("CryptoObject{ch}"), v));
    }
    let sel = vec![crypto_hash("sphere"), crypto_hash("ring")];
    let img = crypto_aux_image(&a, "CryptoObject", &sel);
    for i in 0..n {
        let ranks = a.crypto_ranks("CryptoObject", i);
        let m: f32 = ranks.iter().filter(|(id, c)| *c > 0.0 && sel.contains(&id.to_bits())).map(|(_, c)| *c).sum::<f32>().clamp(0.0, 1.0);
        let mut c = [0.0f32; 3];
        for (id, cov) in &ranks {
            let k = id_color(*id);
            for j in 0..3 {
                c[j] += k[j] * cov;
            }
        }
        let want = [c[0], c[1], c[2], m];
        assert_eq!(img.data[i].map(f32::to_bits), want.map(f32::to_bits), "pixel {i}");
    }
}

/// The lookup before #473, kept as the reference: the rank channels formatted and searched
/// by name for every pixel.
fn ranks_by_name(a: &AuxChannels, layer: &str, i: usize) -> Vec<(f32, f32)> {
    let mut out = vec![];
    for k in 0.. {
        let get = |c: &str| a.get(&format!("{layer}{k:02}.{c}"));
        let (Some(r), Some(g)) = (get("R"), get("G")) else { break };
        out.push((r[i], g[i]));
        if let (Some(b), Some(al)) = (get("B"), get("A")) {
            out.push((b[i], al[i]));
        }
    }
    out
}

/// The reduction before #473 over those ranks: (ID colours, selection coverage).
fn reduce_by_name(a: &AuxChannels, layer: &str, sel: &[u32], i: usize) -> ([f32; 3], f32) {
    let ranks = ranks_by_name(a, layer, i);
    let m: f32 = ranks.iter().filter(|(id, c)| *c > 0.0 && sel.contains(&id.to_bits())).map(|(_, c)| *c).sum::<f32>().clamp(0.0, 1.0);
    let mut c = [0.0f32; 3];
    for (id, cov) in &ranks {
        let k = id_color(*id);
        for j in 0..3 {
            c[j] += k[j] * cov;
        }
    }
    (c, m)
}

/// A 6×2 Cryptomatte layer with awkward values: NaN, ±inf, negative and -0.0 coverage, NaN
/// and -0.0 IDs; rank 01 has no B/A pair, rank 02 is missing, so rank 03 (filled with values
/// that would change every pixel) must be ignored as before.
fn edge_aux(with_rank_after_gap: bool) -> AuxChannels {
    let (w, h) = (6u32, 2u32);
    let nan_payload = f32::from_bits(0x7fc0_1234);
    let (hs, hc) = (crypto_float(crypto_hash("sphere")), crypto_float(crypto_hash("cube")));
    let cov = [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -0.5, -0.0, 0.0, 0.25, 1.0, 2.0, f32::MIN_POSITIVE, nan_payload, 0.5];
    let ids = [hs, f32::NAN, hc, -0.0, 0.0, nan_payload, hs, hc, f32::NAN, hs, hc, -0.0];
    let rot = |v: &[f32; 12], k: usize| -> Vec<f32> { (0..12).map(|i| v[(i + k) % 12]).collect() };
    let mut a = AuxChannels::new(w, h, 1.0);
    let mut push = |n: &str, v: Vec<f32>| a.channels.push((format!("CryptoObject{n}"), v));
    push("00.R", rot(&ids, 0));
    push("00.G", rot(&cov, 0));
    push("00.B", rot(&ids, 3));
    push("00.A", rot(&cov, 5));
    push("01.R", rot(&ids, 7));
    push("01.G", rot(&cov, 2));
    if with_rank_after_gap {
        push("03.R", vec![hs; 12]);
        push("03.G", vec![1.0; 12]);
        push("03.B", vec![hc; 12]);
        push("03.A", vec![1.0; 12]);
    }
    a
}

/// The memo hands out exactly `id_color`'s bits for every rank and pixel of the edge values
/// (NaN IDs with and without a payload, -0.0 and 0.0 are keyed by their bits).
#[test]
fn id_color_memo_matches_id_color() {
    let a = edge_aux(true);
    let planes = a.crypto_rank_planes("CryptoObject");
    let mut memo = IdColorMemo::default();
    for i in 0..12 {
        for (r, (id, _)) in planes.iter().enumerate() {
            assert_eq!(memo.get(r, id[i]).map(f32::to_bits), id_color(id[i]).map(f32::to_bits), "pixel {i} rank {r}");
        }
    }
}

fn edge_selection() -> Vec<u32> {
    vec![crypto_hash("sphere"), f32::NAN.to_bits(), 0x7fc0_1234, (-0.0f32).to_bits()]
}

/// Edge values give bit-identical results to the lookup by name, the missing B/A pair is
/// skipped and a rank after a gap is ignored.
#[test]
fn crypto_aux_image_is_bit_exact_on_edge_values() {
    let sel = edge_selection();
    let a = edge_aux(true);
    assert_eq!(a.crypto_rank_planes("CryptoObject").len(), 3, "00.R/G, 00.B/A, 01.R/G; not 03 after the gap");
    let img = crypto_aux_image(&a, "CryptoObject", &sel);
    let without_gap_rank = crypto_aux_image(&edge_aux(false), "CryptoObject", &sel);
    for i in 0..12 {
        let (c, m) = reduce_by_name(&a, "CryptoObject", &sel, i);
        let want = [c[0], c[1], c[2], m];
        assert_eq!(img.data[i].map(f32::to_bits), want.map(f32::to_bits), "pixel {i}");
        assert_eq!(img.data[i].map(f32::to_bits), without_gap_rank.data[i].map(f32::to_bits), "pixel {i}: the rank after the gap was used");
    }
}

/// Without the ID colours the matte (A) is bit-identical and the RGB is 0, on the edge values.
#[test]
fn crypto_aux_image_without_colors_keeps_the_matte() {
    let a = edge_aux(true);
    let sel = edge_selection();
    let full = crypto_aux_image(&a, "CryptoObject", &sel);
    let matte = crypto_aux_image_with(&a, "CryptoObject", &sel, CryptoAux::MatteOnly);
    for (f, m) in full.data.iter().zip(&matte.data) {
        assert_eq!(f[3].to_bits(), m[3].to_bits());
        assert_eq!(m[..3], [0.0; 3]);
    }
}

/// Only the Colors display without Matte Only reads the ID colours.
#[test]
fn crypto_aux_for_display_reads_colors_only_for_the_colors_display() {
    assert_eq!(CryptoAux::for_display(0, false), CryptoAux::ColorsAndMatte);
    assert_eq!(CryptoAux::for_display(0, true), CryptoAux::MatteOnly);
    for display in [1, 2, 3, u32::MAX] {
        for matte_only in [false, true] {
            assert_eq!(CryptoAux::for_display(display, matte_only), CryptoAux::MatteOnly, "display {display}");
        }
    }
}

/// The CPU effect on a 20×10 layer over the 6×2 edge aux: pixels inside the aux area match
/// the lookup by name bit for bit in every display mode, pixels outside it (no aux index)
/// get an empty matte and no ID colour.
#[test]
fn cryptomatte_cpu_path_is_bit_exact_inside_and_outside_the_aux_area() {
    let a = edge_aux(true);
    let host = AuxHost(Arc::new(a.clone()));
    let names = "sphere, <7fc00000>, <7fc01234>, <80000000>";
    assert_eq!(selection_hashes(names, None), edge_selection());
    let src = img().data[0];
    for (display, matte_only) in [(0, false), (1, false), (2, false), (0, true)] {
        let out = run(
            "ec.3d.cryptomatte",
            &[("selection", Value::Str(names.into())), ("display", Value::Enum(display)), ("matteOnly", Value::Bool(matte_only))],
            &host,
        );
        let expect = |c: [f32; 3], m: f32| -> [f32; 4] {
            if matte_only {
                return [m; 4];
            }
            match display {
                0 => {
                    let c = c.map(|v| v * (1.0 - 0.5 * m) + 0.5 * m);
                    [c[0], c[1], c[2], 1.0]
                }
                1 => src.map(|v| v * m),
                _ => [src[0] * (1.0 - 0.5 * m) + 0.5 * m * src[3], src[1] * (1.0 - 0.5 * m) + 0.5 * m * src[3], src[2] * (1.0 - 0.5 * m), src[3]],
            }
        };
        for (x, y) in [(0u32, 0u32), (1, 0), (5, 0), (0, 1), (3, 1), (5, 1)] {
            let (c, m) = reduce_by_name(&a, "CryptoObject", &edge_selection(), (y * 6 + x) as usize);
            assert_eq!(out.get(x as i64, y as i64).map(f32::to_bits), expect(c, m).map(f32::to_bits), "display {display} matteOnly {matte_only} at ({x}, {y})");
        }
        for (x, y) in [(6u32, 0u32), (19, 0), (0, 2), (12, 9)] {
            assert_eq!(
                out.get(x as i64, y as i64).map(f32::to_bits),
                expect([0.0; 3], 0.0).map(f32::to_bits),
                "display {display} matteOnly {matte_only} outside the aux at ({x}, {y})"
            );
        }
    }
}

#[test]
fn cryptomatte_selects_by_name_and_wildcard() {
    let h = scene();
    let m = run("ec.3d.cryptomatte", &[("selection", Value::Str("right".into())), ("matteOnly", Value::Bool(true))], &h);
    assert_eq!(m.get(2, 5)[3], 0.0);
    assert_eq!(m.get(12, 5)[3], 1.0);
    let w = run("ec.3d.cryptomatte", &[("selection", Value::Str("l*".into())), ("display", Value::Enum(1))], &h);
    assert_eq!(w.get(2, 5)[3], 1.0);
    assert_eq!(w.get(12, 5)[3], 0.0);
    assert!(wild("*ight", "right") && wild("r*t", "right") && !wild("x*", "right"));
    let hex = format!("<{:08x}>", crypto_hash("left"));
    assert_eq!(selection_hashes(&hex, None), vec![crypto_hash("left")]);
}

/// #473: a name with a comma in it (Blender allows `Green, Glossy`) is selected when it is
/// quoted or its comma escaped, instead of being cut into two names that match nothing.
#[test]
fn cryptomatte_selection_keeps_quoted_names_with_commas_whole() {
    let manifest = vec![("Green, Glossy".to_string(), 7), ("Green".to_string(), 1), ("Glossy".to_string(), 2), ("ring".to_string(), 3)];
    let m = Some(manifest.as_slice());
    assert_eq!(selection_hashes(r#""Green, Glossy""#, m), vec![7]);
    assert_eq!(selection_hashes("'Green, Glossy', ring", m), vec![7, 3]);
    assert_eq!(selection_hashes(r"Green\, Glossy", m), vec![7]);
    assert_eq!(selection_hashes(r#"ring , "Green, Glossy""#, m), vec![3, 7]);
    // Without a manifest the whole name is hashed.
    assert_eq!(selection_hashes(r#""Green, Glossy""#, None), vec![crypto_hash("Green, Glossy")]);
    // Plain lists, apostrophes inside names and an unclosed quote still parse.
    assert_eq!(selection_hashes("Green, Glossy", m), vec![1, 2]);
    assert_eq!(selection_names("Bob's mat, ring"), vec!["Bob's mat", "ring"]);
    assert_eq!(selection_names(r#""Green, Glossy"#), vec!["Green, Glossy"]);
    assert_eq!(selection_names(r#"say \"hi\""#), vec![r#"say "hi""#]);
}

#[test]
fn fog_and_depth_of_field_follow_depth() {
    let h = scene();
    let f = run("ec.3d.fog3d", &[("fogColor", col(1.0, 0.0, 0.0)), ("fogEndDepth", num(1000.0)), ("scatteringDensity", num(0.0))], &h);
    // Linear fog: 10 % at depth 100, 50 % at 500, full on the background.
    assert!((f.get(2, 5)[0] - 0.55).abs() < 1e-4, "{:?}", f.get(2, 5));
    assert!((f.get(12, 5)[0] - 0.75).abs() < 1e-4);
    assert!((f.get(17, 1)[0] - 1.0).abs() < 1e-4 && f.get(17, 1)[1] < 1e-4);
    assert!((dof_radius(100.0, 100.0, 0.0, 10.0, 50.0)).abs() < 1e-6);
    assert!((dof_radius(200.0, 100.0, 0.0, 10.0, 50.0) - 10.0).abs() < 1e-6);
    assert_eq!(dof_radius(140.0, 100.0, 100.0, 10.0, 50.0), 0.0);
    // A sharp edge at the far object blurs when focused near.
    let mut pic = Image::new(20, 10);
    for y in 0..10 {
        for x in 0..20 {
            let v = if x % 2 == 0 { 1.0 } else { 0.0 };
            pic.set(x, y, [v, v, v, 1.0]);
        }
    }
    let env = EffectEnv { host: Some(&h), ..Default::default() };
    let d = run_fx("ec.3d.depthoffield", &[("focalPlane", num(100.0)), ("maximumRadius", num(4.0))], pic.clone(), 0.0, env).img;
    assert_eq!(d.get(4, 5), pic.get(4, 5));
    assert!((d.get(12, 5)[0] - 0.5).abs() < 0.3 && d.get(12, 5) != pic.get(12, 5));
}

#[test]
fn extractor_maps_named_channels() {
    let h = scene();
    let o = run("ec.3d.extractor", &[("red", Value::Str("Z".into())), ("whitePoint", num(1000.0))], &h);
    assert!((o.get(2, 5)[0] - 0.1).abs() < 1e-6);
    assert!((o.get(2, 5)[1] - 0.0005).abs() < 1e-6);
    // Without aux: the layer's own channels can be swapped.
    let s = run_fx("ec.3d.extractor", &[("red", Value::Str("A".into()))], Image::filled(2, 2, [0.2, 0.3, 0.4, 1.0]), 0.0, EffectEnv::default());
    assert_eq!(s.img.get(0, 0), [1.0, 0.3, 0.4, 1.0]);
}

#[test]
fn channel_extract_antialias_averages_sub_pixels() {
    // Depth at twice the layer resolution, alternating 0 / 1000 per aux column.
    let mut a = AuxChannels::new(40, 20, 2.0);
    a.channels = vec![("Z".into(), (0..800).map(|i| if i % 2 == 0 { 0.0 } else { 1000.0 }).collect())];
    let host = AuxHost(Arc::new(a));
    let vals = [("blackPoint", num(0.0)), ("whitePoint", num(1000.0))];
    let hard = run("ec.3d.channelextract", &vals, &host);
    let v = hard.get(5, 5)[0];
    assert!(v == 0.0 || v == 1.0, "{v}");
    let soft = run("ec.3d.channelextract", &[vals.as_slice(), &[("antialias", Value::Bool(true))]].concat(), &host);
    assert!((soft.get(5, 5)[0] - 0.5).abs() < 1e-5, "{:?}", soft.get(5, 5));
}

#[test]
fn instance3d_replaces_the_pixels_with_the_referenced_layer() {
    // Create 3D Layer Instance's copy: the chosen layer's render takes the place of the layer's
    // own pixels, alpha included — the copy is transparent where the referenced layer isn't —
    // and with no layer chosen the layer passes through.
    struct Host(Image);
    impl EffectHost for Host {
        fn layer(&self, id: u64, _: bool) -> Option<LayerPixels> {
            (id == 7).then(|| LayerPixels { buf: Buf { img: self.0.clone(), offset: [0.0; 2], scale: 1.0 }, size: [self.0.width as f64, self.0.height as f64] })
        }
        fn audio(&self, _: u64, _: f64, _: usize, _: u32) -> Option<Vec<f32>> {
            None
        }
    }
    let solid = || Image::filled(4, 4, [1.0, 1.0, 1.0, 1.0]);
    let mut render = Image::new(4, 4);
    render.set(1, 1, [1.0, 0.0, 0.0, 1.0]);
    let env = EffectEnv { host: Some(&Host(render)), ..Default::default() };
    let out = run_fx("ec.3d.instance3d", &[("layer", Value::Layer(Some(7)))], solid(), 0.0, env);
    assert_eq!(out.img.get(1, 1), [1.0, 0.0, 0.0, 1.0], "the referenced layer's pixel");
    assert_eq!(out.img.get(3, 3), [0.0; 4], "transparent where the referenced layer isn't");
    // No layer chosen: the layer's own pixels.
    let out = run_fx("ec.3d.instance3d", &[], solid(), 0.0, EffectEnv::default());
    assert_eq!(out.img.data, solid().data);
}
