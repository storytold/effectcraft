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
    for (display, matte_only, in_alpha) in [(0, false, false), (1, false, false), (2, false, false), (0, true, false), (0, true, true)] {
        let out = run(
            "ec.3d.cryptomatte",
            &[
                ("selection", Value::Str(names.into())),
                ("display", Value::Enum(display)),
                ("matteOnly", Value::Bool(matte_only)),
                (MATTE_ONLY_IN_ALPHA, Value::Bool(in_alpha)),
            ],
            &host,
        );
        let expect = |c: [f32; 3], m: f32| -> [f32; 4] {
            if matte_only {
                return if in_alpha { [m; 4] } else { [m, m, m, 1.0] };
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
            assert_eq!(
                out.get(x as i64, y as i64).map(f32::to_bits),
                expect(c, m).map(f32::to_bits),
                "display {display} matteOnly {matte_only} inAlpha {in_alpha} at ({x}, {y})"
            );
        }
        for (x, y) in [(6u32, 0u32), (19, 0), (0, 2), (12, 9)] {
            assert_eq!(
                out.get(x as i64, y as i64).map(f32::to_bits),
                expect([0.0; 3], 0.0).map(f32::to_bits),
                "display {display} matteOnly {matte_only} inAlpha {in_alpha} outside the aux at ({x}, {y})"
            );
        }
    }
}

/// #3127: Matte Only shows the matte as After Effects' Cryptomatte plug-in does, in RGB over
/// alpha 1 (its Matte Only render of 2026-11-05 is opaque black where nothing is selected), not
/// in alpha. A partly covered pixel is grey, not translucent; the hidden Matte Only In Alpha
/// switch (on for projects saved before) keeps the old matte in every channel.
#[test]
fn cryptomatte_matte_only_is_the_matte_in_rgb_over_alpha_one() {
    let a = edge_aux(true);
    let host = AuxHost(Arc::new(a.clone()));
    let names = "sphere, <7fc00000>, <7fc01234>, <80000000>";
    let only = |in_alpha: bool| {
        run(
            "ec.3d.cryptomatte",
            &[("selection", Value::Str(names.into())), ("matteOnly", Value::Bool(true)), (MATTE_ONLY_IN_ALPHA, Value::Bool(in_alpha))],
            &host,
        )
    };
    let (new, old) = (only(false), only(true));
    let partial = (0..6u32 * 2).map(|i| (i % 6, i / 6)).find(|&(x, y)| {
        let m = reduce_by_name(&a, "CryptoObject", &edge_selection(), (y * 6 + x) as usize).1;
        m > 0.0 && m < 1.0
    });
    let (x, y) = partial.expect("the edge aux has a partly covered pixel");
    let m = reduce_by_name(&a, "CryptoObject", &edge_selection(), (y * 6 + x) as usize).1;
    assert_eq!(new.get(x as i64, y as i64), [m, m, m, 1.0]);
    assert_eq!(old.get(x as i64, y as i64), [m; 4]);
    // Outside the aux area (and unselected): opaque black, as After Effects renders it.
    assert_eq!(new.get(12, 9), [0.0, 0.0, 0.0, 1.0]);
    assert_eq!(old.get(12, 9), [0.0; 4]);
    // A fresh instance has the switch off.
    let spec = crate::find("ec.3d.cryptomatte").unwrap();
    let ps = spec.params.iter().find(|p| p.id == MATTE_ONLY_IN_ALPHA).expect("the hidden switch");
    assert_eq!((&ps.default, &ps.ui), (&Value::Bool(false), &ParamUi::Hidden));
}

#[test]
fn cryptomatte_selects_by_name_and_wildcard() {
    let h = scene();
    let m = run("ec.3d.cryptomatte", &[("selection", Value::Str("right".into())), ("matteOnly", Value::Bool(true))], &h);
    assert_eq!(m.get(2, 5), [0.0, 0.0, 0.0, 1.0]);
    assert_eq!(m.get(12, 5), [1.0; 4]);
    let w = run("ec.3d.cryptomatte", &[("selection", Value::Str("l*".into())), ("display", Value::Enum(1))], &h);
    assert_eq!(w.get(2, 5)[3], 1.0);
    assert_eq!(w.get(12, 5)[3], 0.0);
    assert!(wild("*ight", "right") && wild("r*t", "right") && !wild("x*", "right"));
    let hex = format!("<{:08x}>", crypto_hash("left"));
    assert_eq!(selection_hashes(&hex, None), vec![crypto_hash("left")]);
}

/// Layer Name picks a Cryptomatte layer named in full (`Crypto_Material`, which the Layer popup's
/// CryptoMaterial does not match); empty or missing it falls back to the Layer popup.
#[test]
fn cryptomatte_layer_name_picks_the_named_layer() {
    let base = scene();
    let mut a = (*base.0).clone();
    // A second layer with the IDs swapped: left is on the right half there.
    let swap = |name: &str| a.channels.iter().find(|(n, _)| n == name).map(|(_, v)| v.clone()).unwrap();
    let (r, g) = (swap("CryptoObject00.R"), swap("CryptoObject00.G"));
    let w = 20;
    let r2: Vec<f32> = (0..r.len()).map(|i| r[(i / w) * w + (w - 1 - i % w)]).collect();
    a.channels.push(("Crypto_Material00.R".into(), r2));
    a.channels.push(("Crypto_Material00.G".into(), g));
    let h = AuxHost(Arc::new(a));
    let sel = format!("<{:08x}>", crypto_hash("left"));
    let matte = |vals: &[(&str, Value)]| {
        let mut v = vec![("selection", Value::Str(sel.clone())), ("matteOnly", Value::Bool(true))];
        v.extend(vals.iter().cloned());
        let m = run("ec.3d.cryptomatte", &v, &h);
        (m.get(2, 5)[0], m.get(17, 5)[0])
    };
    assert_eq!(matte(&[("layerName", Value::Str("Crypto_Material".into()))]), (0.0, 1.0), "the named layer");
    assert_eq!(matte(&[]), (1.0, 0.0), "the Layer popup's CryptoObject");
    assert_eq!(matte(&[("layerName", Value::Str("Crypto_Name".into()))]), (1.0, 0.0), "a missing name falls back to the popup");
}

/// Layers whose names differ only in case are different layers: `layerName` "crypto_a" mattes
/// from crypto_a's own IDs, not from the earlier `Crypto_A`.
#[test]
fn cryptomatte_layer_name_is_case_sensitive() {
    let base = scene();
    let mut a = (*base.0).clone();
    let (r, g) = (
        a.channels.iter().find(|(n, _)| n == "CryptoObject00.R").unwrap().1.clone(),
        a.channels.iter().find(|(n, _)| n == "CryptoObject00.G").unwrap().1.clone(),
    );
    let w = 20;
    let flipped: Vec<f32> = (0..r.len()).map(|i| r[(i / w) * w + (w - 1 - i % w)]).collect();
    // `Crypto_A` first, with left/right as in the base scene; `crypto_a` has them swapped.
    a.channels.push(("Crypto_A00.R".into(), r));
    a.channels.push(("Crypto_A00.G".into(), g.clone()));
    a.channels.push(("crypto_a00.r".into(), flipped));
    a.channels.push(("crypto_a00.g".into(), g));
    let h = AuxHost(Arc::new(a));
    let sel = format!("<{:08x}>", crypto_hash("left"));
    let matte = |name: &str| {
        let v = [("selection", Value::Str(sel.clone())), ("matteOnly", Value::Bool(true)), ("layerName", Value::Str(name.into()))];
        let m = run("ec.3d.cryptomatte", &v, &h);
        (m.get(2, 5)[0], m.get(17, 5)[0])
    };
    assert_eq!(matte("Crypto_A"), (1.0, 0.0));
    assert_eq!(matte("crypto_a"), (0.0, 1.0));
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

/// `(copy)` (After Effects' EXtractoR choice, imported from .aep) is the effect input's own
/// channel as earlier effects left it, never the OpenEXR's plane of the same name: here the input
/// buffer differs from the footage's R, G, B and A planes, as after an earlier effect.
#[test]
fn extractor_copy_reads_the_current_input_not_the_footage_plane() {
    let (w, h) = (4usize, 3usize);
    let mut a = AuxChannels::new(w as u32, h as u32, 1.0);
    // The file's own planes (what the footage held before any effect) and a depth channel.
    a.channels = ["R", "G", "B", "A"].iter().map(|n| (n.to_string(), vec![0.9; w * h])).collect();
    a.channels.push(("Z".into(), vec![0.7; w * h]));
    let host = AuxHost(Arc::new(a));
    let env = || EffectEnv { host: Some(&host), ..Default::default() };
    // The input after an earlier effect: straight (0.2, 0.3, 0.4) at alpha 0.5, premultiplied.
    let input = Image::filled(w as u32, h as u32, [0.1, 0.15, 0.2, 0.5]);
    let copy = || Value::Str(EXTRACTOR_COPY.into());
    // All four `(copy)`: the input unchanged.
    let all = [("red", copy()), ("green", copy()), ("blue", copy()), ("alpha", copy())];
    let o = run_fx("ec.3d.extractor", &all, input.clone(), 0.0, env()).img;
    assert_eq!(o.data, input.data);
    // Mixed with a named channel (no pass-through): red from Z, green and alpha copied, blue the
    // footage's B plane.
    let vals = [("red", Value::Str("Z".into())), ("green", copy()), ("blue", Value::Str("B".into())), ("alpha", copy())];
    let px = run_fx("ec.3d.extractor", &vals, input.clone(), 0.0, env()).img.get(1, 1);
    let want = [0.7 * 0.5, 0.3 * 0.5, 0.9 * 0.5, 0.5];
    for (c, (got, want)) in px.iter().zip(want).enumerate() {
        assert!((got - want).abs() < 1e-6, "channel {c}: {got} != {want} ({px:?})");
    }
    // `A` by name is the footage's alpha plane (0.9), not the input's alpha (0.5).
    let named = [("alpha", Value::Str("A".into())), ("red", Value::Str("Z".into()))];
    assert!((run_fx("ec.3d.extractor", &named, input, 0.0, env()).img.get(1, 1)[3] - 0.9).abs() < 1e-6);
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

/// A 20×10 layer of one Cryptomatte layer `name` that holds "left" on its left half and
/// "right" on its right half, as [`scene`] does for `CryptoObject`.
fn crypto_only(names: &[&str]) -> AuxHost {
    let (w, h) = (20usize, 10usize);
    let mut a = AuxChannels::new(w as u32, h as u32, 1.0);
    let (hl, hr) = (crypto_hash("left"), crypto_hash("right"));
    for name in names {
        let ids: Vec<f32> = (0..w * h).map(|i| crypto_float(if i % w < 10 { hl } else { hr })).collect();
        a.channels.push((format!("{name}00.R"), ids));
        a.channels.push((format!("{name}00.G"), vec![1.0; w * h]));
        a.manifests.push((name.to_string(), vec![("left".into(), hl), ("right".into(), hr)]));
    }
    AuxHost(Arc::new(a))
}

fn choices(names: &[&str]) -> Vec<(usize, String)> {
    crypto_layer_choices(Some(&crypto_only(names).0))
}

/// #473: the Layer popup lists the Cryptomatte layers the footage has, and the three standard
/// ones when there is no footage, no OpenEXR or no Cryptomatte in it.
#[test]
fn crypto_layer_popup_lists_the_layers_of_the_footage() {
    let standard: Vec<(usize, String)> = CRYPTO_LAYERS.iter().enumerate().map(|(i, n)| (i, n.to_string())).collect();
    assert_eq!(crypto_layer_choices(None), standard);
    assert_eq!(crypto_layer_choices(Some(&AuxChannels::new(1, 1, 1.0))), standard, "an EXR without Cryptomatte");
    assert_eq!(crypto_layer_choices(Some(&scene().0)), vec![(0, "CryptoObject".to_string())]);
    // Only CryptoMaterial: it, not CryptoObject, with its own stored value.
    assert_eq!(choices(&["CryptoMaterial"]), vec![(1, "CryptoMaterial".to_string())]);
    // Standard layers come in slot order whatever the names sort like; prefixed ones fill their slot.
    assert_eq!(choices(&["CryptoAsset", "ViewLayer.CryptoObject"]), vec![(0, "ViewLayer.CryptoObject".to_string()), (2, "CryptoAsset".to_string())]);
    // A layer under another name gets a value after the standard ones, in name order.
    assert_eq!(choices(&["Zed", "CryptoMaterial", "Alpha"]), vec![(1, "CryptoMaterial".to_string()), (3, "Alpha".to_string()), (4, "Zed".to_string())]);
    // A name that only ends in a standard one fills its slot, as the effect always matched it.
    assert_eq!(
        choices(&["MyCryptoObject", "ViewLayer.CryptoMaterial", "Other"]),
        vec![(0, "MyCryptoObject".to_string()), (1, "ViewLayer.CryptoMaterial".to_string()), (3, "Other".to_string())]
    );
    // The entry shown: the value's own, else the first layer; with no layers, the standard one.
    let shown = |names: &[&str], i| crypto_layer_menu(Some(&crypto_only(names).0), i).1;
    assert_eq!(
        [0, 1, 7].map(|i| shown(&["CryptoMaterial"], i)),
        [Some("CryptoMaterial".to_string()), Some("CryptoMaterial".to_string()), Some("CryptoMaterial".to_string())]
    );
    assert_eq!(crypto_layer_menu(None, 2).1.as_deref(), Some("CryptoAsset"));
    assert_eq!(crypto_layer_menu(None, 9).1.as_deref(), Some("CryptoObject"), "a custom value with no footage");
    // Two layers for one slot: the first keeps it, the next gets a custom value.
    assert_eq!(choices(&["A.CryptoObject", "B.CryptoObject"]), vec![(0, "A.CryptoObject".to_string()), (3, "B.CryptoObject".to_string())]);
    assert_eq!(choices(&["A.CryptoObject", "B.CryptoObject", "C.CryptoObject", "Mine"]).iter().map(|c| c.0).collect::<Vec<_>>(), [0, 3, 4, 5]);
}

/// A project saved with Layer = CryptoObject / CryptoMaterial / CryptoAsset renders the same layer
/// as before: the stored value's own layer, else the file's first; and the custom layers are read
/// by their value.
#[test]
fn crypto_layer_values_keep_their_meaning() {
    let all = crypto_only(&["CryptoAsset", "CryptoMaterial", "CryptoObject", "Mine"]).0;
    let name = |i| crypto_layer_name(&all, i);
    assert_eq!([0, 1, 2, 3].map(name), ["CryptoObject", "CryptoMaterial", "CryptoAsset", "Mine"].map(|n| Some(n.to_string())));
    // A value the file has no layer for reads its first layer, as the effect always did.
    assert_eq!(name(4).as_deref(), Some("CryptoAsset"));
    let only = crypto_only(&["CryptoMaterial"]).0;
    assert_eq!(
        [0, 1, 2].map(|i| crypto_layer_name(&only, i)),
        [Some("CryptoMaterial".to_string()), Some("CryptoMaterial".to_string()), Some("CryptoMaterial".to_string())]
    );
    assert_eq!(crypto_layer_name(&AuxChannels::new(1, 1, 1.0), 0), None);
}

/// The effect keys the layer the Layer value names: a Material-only file renders its matte
/// whether Layer says Object (a saved project) or Material (what the popup now stores), and a
/// custom layer by its own value.
#[test]
fn cryptomatte_renders_the_layer_the_value_names() {
    let sel = ("selection", Value::Str("left".into()));
    let matte = |layer: u32, host: &AuxHost| run("ec.3d.cryptomatte", &[("layer", Value::Enum(layer)), sel.clone(), ("display", Value::Enum(1))], host);
    let only = crypto_only(&["CryptoMaterial"]);
    let (old, new) = (matte(0, &only), matte(1, &only));
    assert_eq!(old.data, new.data);
    assert_eq!((new.get(2, 5)[3], new.get(12, 5)[3]), (1.0, 0.0));
    // Two layers: "left" is in both, under hashes the manifests share, so tell them apart by
    // which has it on the right half.
    let (hl, hr) = (crypto_hash("left"), crypto_hash("right"));
    let mut a = AuxChannels::new(20, 10, 1.0);
    for (name, flip) in [("CryptoObject", false), ("Mine", true)] {
        let ids: Vec<f32> = (0..200).map(|i| crypto_float(if (i % 20 < 10) != flip { hl } else { hr })).collect();
        a.channels.push((format!("{name}00.R"), ids));
        a.channels.push((format!("{name}00.G"), vec![1.0; 200]));
    }
    let two = AuxHost(Arc::new(a));
    let (object, mine) = (matte(0, &two), matte(3, &two));
    assert_eq!((object.get(2, 5)[3], object.get(12, 5)[3]), (1.0, 0.0));
    assert_eq!((mine.get(2, 5)[3], mine.get(12, 5)[3]), (0.0, 1.0), "Layer 3 is the custom layer");
    // A second view's layer of a standard kind is selectable too: B.CryptoObject is Layer 3.
    let mut a = AuxChannels::new(20, 10, 1.0);
    for (name, flip) in [("A.CryptoObject", false), ("B.CryptoObject", true)] {
        let ids: Vec<f32> = (0..200).map(|i| crypto_float(if (i % 20 < 10) != flip { hl } else { hr })).collect();
        a.channels.push((format!("{name}00.R"), ids));
        a.channels.push((format!("{name}00.G"), vec![1.0; 200]));
    }
    let views = AuxHost(Arc::new(a));
    assert_eq!(crypto_layer_name(&views.0, 0).as_deref(), Some("A.CryptoObject"));
    assert_eq!(crypto_layer_name(&views.0, 3).as_deref(), Some("B.CryptoObject"));
    let (first, second) = (matte(0, &views), matte(3, &views));
    assert_eq!((first.get(2, 5)[3], first.get(12, 5)[3]), (1.0, 0.0));
    assert_eq!((second.get(2, 5)[3], second.get(12, 5)[3]), (0.0, 1.0), "Layer 3 renders B");
}

/// #474: [`reads_aux`] names exactly the 3D Channel effects that ask the host for channels.
#[test]
fn reads_aux_names_the_effects_that_read_channels() {
    struct Counting(AuxHost, std::sync::atomic::AtomicUsize);
    impl EffectHost for Counting {
        fn layer(&self, _: u64, _: bool) -> Option<LayerPixels> {
            None
        }
        fn audio(&self, _: u64, _: f64, _: usize, _: u32) -> Option<Vec<f32>> {
            None
        }
        fn aux(&self) -> Option<Arc<AuxChannels>> {
            self.1.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            self.0.aux()
        }
    }
    for s in specs() {
        let host = Counting(scene(), Default::default());
        run_fx(s.id, &[], img(), 0.0, EffectEnv { host: Some(&host), ..Default::default() });
        assert_eq!(host.1.into_inner() > 0, reads_aux(s.id), "{}", s.id);
    }
    assert!(READS_AUX.iter().all(|id| specs().iter().any(|s| s.id == *id)), "every listed effect exists");
}
