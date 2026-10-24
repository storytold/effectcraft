//! Developer tool: extracts the effect prototypes the exporter clones from our own synthetic
//! cases (the local synthetic case folder, not in this repository; see `docs/aep-export.md` R24, R45):
//!
//! - per effect of the shared table (`effectcraft_aep::effect_map`) the default instance
//!   (`tdmn`, `sspc`) and its `EfDf` definition copy, from the case that adds the effect at its
//!   defaults, or, where the corpus has none, the case that changes one value, with that value
//!   record removed (absent = default);
//! - one static value record (`tdmn` + `tdbs`) per parameter type (`fxkd`): integer slider 414,
//!   fixed slider 361, angle 219, checkbox 217, colour 357, point 369, popup 377, float slider 071;
//! - one keyed value record per keyed kind: fixed slider 363, colour 364, point 382, float slider
//!   397.
//!
//! Curves (961) and Mesh Warp (965) keep their always-present arbitrary-data record.
//!
//! `cargo run -p effectcraft-aep-export --example mkfxprotos -- <synthetic dir> crates/aep-export/template/effects.aep`
use effectcraft_aep::{AepFile, Chunk, parse, write};
use std::borrow::Cow;

fn tdmn_name(c: &Chunk<'_>) -> Option<String> {
    let d = c.bytes()?;
    Some(d.iter().take_while(|b| **b != 0).map(|b| *b as char).collect())
}

fn load(dir: &std::path::Path, prefix: &str) -> AepFile<'static> {
    let name = std::fs::read_dir(dir)
        .expect("dir")
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .find(|n| n.starts_with(prefix) && n.ends_with(".aep"))
        .unwrap_or_else(|| panic!("case {prefix}"));
    let bytes = std::fs::read(dir.join(name)).expect("read");
    parse(&bytes).expect("parse").into_owned()
}

/// Every `(tdmn, sspc)` pair below `c` (effect instances of every layer and the `EfDf` copies).
fn instances(c: &Chunk<'static>, out: &mut Vec<(Chunk<'static>, Chunk<'static>)>, in_efdg: bool) {
    let k = c.children();
    for (i, x) in k.iter().enumerate() {
        if x.is_list(b"sspc") && i > 0 && k[i - 1].is(b"tdmn") && !in_efdg {
            out.push((k[i - 1].clone(), x.clone()));
        }
        if x.is_list(b"EfdG") {
            continue;
        }
        instances(x, out, in_efdg);
    }
}

/// The first instance of `match_name` on a layer.
fn instance(f: &AepFile<'static>, match_name: &str) -> (Chunk<'static>, Chunk<'static>) {
    let mut all = Vec::new();
    for c in &f.chunks {
        if c.is_list(b"EfdG") {
            continue;
        }
        instances(c, &mut all, false);
    }
    all.into_iter().find(|(m, _)| tdmn_name(m).as_deref() == Some(match_name)).unwrap_or_else(|| panic!("{match_name}"))
}

/// The `EfDf` copy of `match_name`.
fn efdf(f: &AepFile<'static>, match_name: &str) -> Chunk<'static> {
    let efdg = f.list(b"EfdG").expect("EfdG");
    efdg.children()
        .iter()
        .find(|c| c.is_list(b"EfDf") && c.children().first().and_then(tdmn_name).as_deref() == Some(match_name))
        .unwrap_or_else(|| panic!("EfDf {match_name}"))
        .clone()
}

/// The `pard` type of each parameter of an instance, by its record name.
fn pard_types(sspc: &Chunk<'static>) -> Vec<(String, u32)> {
    let Some(part) = sspc.child_list(b"parT") else { return Vec::new() };
    let k = part.children();
    let mut out = Vec::new();
    for (i, c) in k.iter().enumerate() {
        if c.is(b"pard")
            && let (Some(m), Some(d)) = (k.get(i.wrapping_sub(1)), c.bytes())
        {
            out.push((tdmn_name(m).unwrap_or_default(), u32::from_be_bytes(d[12..16].try_into().unwrap())));
        }
    }
    out
}

/// Removes the value records of value parameters (sliders, angles, checkboxes, colours, points,
/// popups) from the value group of `sspc` (definitions from `defs`): absent = default.
fn strip(sspc: &mut Chunk<'static>, defs: &[(String, u32)]) {
    let values = sspc.children_mut().iter_mut().find(|c| c.is_list(b"tdgp")).expect("values");
    let effectcraft_aep::Body::List { children, .. } = &mut values.body else { panic!("values") };
    let mut out = Vec::new();
    let mut i = 0;
    while i < children.len() {
        let c = &children[i];
        if c.is(b"tdmn")
            && let Some(name) = tdmn_name(c)
            && defs.iter().any(|(n, t)| *n == name && matches!(t, 1 | 2 | 3 | 4 | 5 | 6 | 7 | 10))
        {
            i += 2;
            continue;
        }
        out.push(c.clone());
        i += 1;
    }
    *children = out;
}

/// The `(tdmn, tdbs)` value record named `name` of the instance.
fn record(sspc: &Chunk<'static>, name: &str) -> (Chunk<'static>, Chunk<'static>) {
    let tdgp = sspc.child_list(b"tdgp").expect("values");
    let k = tdgp.children();
    let i = k.iter().position(|c| c.is(b"tdmn") && tdmn_name(c).as_deref() == Some(name)).unwrap_or_else(|| panic!("{name}"));
    (k[i].clone(), k[i + 1].clone())
}

/// The default case of each effect of the table.
const DEFAULTS: &[(&str, &str)] = &[
    ("ADBE Tint", "356"),
    ("ADBE Ramp", "368"),
    ("ADBE Exposure2", "389"),
    ("ADBE Invert", "409"),
    ("ADBE Box Blur2", "412"),
    ("ADBE Gaussian Blur 2", "417"),
    ("ADBE Drop Shadow", "420"),
    ("ADBE Timewarp", "529"),
    ("ADBE Glo2", "427"),
    ("ADBE Fill", "215"),
    ("ADBE HUE SATURATION", "443"),
    ("ADBE Easy Levels2", "511"),
    // No default-only case: 711 sets Direction 30 and Length 40 (records removed).
    ("ADBE Motion Blur", "711"),
    ("ADBE Radial Blur", "712"),
    ("CC Radial Blur", "712"),
    ("ADBE Venetian Blinds", "712"),
    ("ADBE Channel Blur", "712"),
    ("ADBE Black&White", "712"),
    ("ADBE Tritone", "712"),
    ("ADBE Luma Key", "712"),
    ("ADBE Simple Choker", "712"),
    ("ADBE Matte Choker", "712"),
    ("ADBE Geometry2", "712"),
    // Batch 6: 903 adds them at their defaults, nothing set.
    ("ADBE Brightness & Contrast 2", "903"),
    ("ADBE Turbulent Displace", "903"),
    ("ADBE Fractal Noise", "903"),
    ("ADBE Laser", "903"),
    ("CC Radial Fast Blur", "903"),
    ("ADBE Noise2", "903"),
    ("ADBE Pro Levels2", "903"),
    ("ADBE Corner Pin", "903"),
    ("ADBE Grid", "903"),
    ("ADBE Sharpen", "903"),
    ("ADBE Linear Color Key2", "903"),
    ("ADBE Radial Wipe", "903"),
    ("ADBE Remove Color Matting", "903"),
    ("CC Bend It", "903"),
    ("CC Power Pin", "903"),
    ("ADBE 4ColorGradient", "903"),
    ("ADBE Change Color", "903"),
    ("ADBE Extract", "903"),
    ("ADBE Noise HLS2", "903"),
    ("ADBE Scatter", "903"),
    ("CC Light Sweep", "903"),
    ("ADBE Compound Blur", "903"),
    ("ADBE Displacement Map", "903"),
    ("ADBE Set Matte3", "903"),
    ("ADBE Gaussian Blur", "903"),
    ("ADBE OFMotionBlur", "903"),
    // The layer parameter's record always exists; its `tdpi` is set on export.
    ("ADBE Layer Control", "076"),
    ("ADBE Slider Control", "070"),
    ("ADBE Angle Control", "075"),
    ("ADBE Checkbox Control", "072"),
    // No default-only case: 073 sets the colour (record removed).
    ("ADBE Color Control", "073"),
    ("ADBE Point Control", "074"),
    // Applied from presets at their defaults (FORMAT S): the arbitrary data is always stored.
    ("ADBE CurvesCustom", "961"),
    ("ADBE MESH WARP", "965"),
];

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let dir = std::path::Path::new(&a[1]);
    for e in effectcraft_aep::effect_map::EFFECTS {
        assert!(DEFAULTS.iter().any(|(m, _)| *m == e.match_name), "no default case for {}", e.match_name);
    }
    let mut chunks = Vec::new();
    for (match_name, case) in DEFAULTS {
        let f = load(dir, case);
        let (tdmn, mut sspc) = instance(&f, match_name);
        let defs = pard_types(&sspc);
        strip(&mut sspc, &defs);
        let mut def = efdf(&f, match_name);
        if let Some(copy) = def.children_mut().iter_mut().find(|c| c.is_list(b"sspc")) {
            strip(copy, &defs);
        }
        chunks.push(Chunk::list(*b"fxdf", vec![tdmn, sspc, def]));
    }
    let kinds: &[(u32, &str, &str)] = &[
        (1, "414", "ADBE Box Blur2-0002"),
        (2, "361", "ADBE Tint-0003"),
        (3, "219", "ADBE Angle Control-0001"),
        (4, "217", "ADBE Checkbox Control-0001"),
        (5, "357", "ADBE Tint-0001"),
        (6, "369", "ADBE Ramp-0001"),
        (7, "377", "ADBE Ramp-0005"),
        (10, "071", "ADBE Slider Control-0001"),
    ];
    let mut kd = Vec::new();
    for (code, case, name) in kinds {
        let match_name = name.rsplit_once('-').unwrap().0;
        let (_, sspc) = instance(&load(dir, case), match_name);
        let (_, r) = record(&sspc, name);
        let mut m = vec![0u8; 40];
        let label = format!("kind-{code}");
        m[..label.len()].copy_from_slice(label.as_bytes());
        kd.push(Chunk::data(*b"tdmn", m));
        kd.push(r);
    }
    chunks.push(Chunk::list(*b"fxkd", kd));
    let keyed: &[(&str, &str)] = &[("363", "ADBE Tint-0003"), ("364", "ADBE Tint-0002"), ("382", "ADBE Ramp-0001"), ("397", "ADBE Exposure2-0003")];
    let mut ky = Vec::new();
    for (case, name) in keyed {
        let match_name = name.rsplit_once('-').unwrap().0;
        let (_, sspc) = instance(&load(dir, case), match_name);
        let (m, r) = record(&sspc, name);
        ky.push(m);
        ky.push(r);
    }
    chunks.push(Chunk::list(*b"fxky", ky));
    let out = AepFile { chunks, trailer: Cow::Owned(vec![]) };
    std::fs::write(&a[2], write(&out).expect("write")).expect("write file");
}
