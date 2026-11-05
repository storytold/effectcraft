//! Lumetri Color (`ADBE Lumetri`) arrives as EffectCraft's Lumetri Color (`ec.color.lumetri`),
//! its parameters found by match name. The effect is built into the committed `synthetic.aep` in
//! place of its Glow, in the layout of the 1200 samples (After Effects 2026, docs/aep-format.md
//! 3.7): the definitions are a subset in an order of our own, so a mapping by `parT` index fails.

use effectcraft_aep::import::{ImportOptions, ImportResult, LossCategory, import};
use effectcraft_aep::{Body, Chunk, parse, write};
use effectcraft_keyframe::Value;
use effectcraft_project::{Project, PropGroup};

const SYNTHETIC: &[u8] = include_bytes!("../../fixtures/synthetic.aep");

fn tdmn(name: &str) -> Chunk<'static> {
    let mut b = name.as_bytes().to_vec();
    b.resize(40, 0);
    Chunk::data(*b"tdmn", b)
}

/// A chunk holding chunks (`pdnm`, `fnam`: `NESTED_IDS`) with one `Utf8`.
fn nested(id: [u8; 4], text: &[u8]) -> Chunk<'static> {
    Chunk { id, body: Body::Chunks(vec![Chunk::data(*b"Utf8", text.to_vec())]), pad: None }
}

fn mn(suffix: u16) -> String {
    format!("ADBE Lumetri-{suffix:04}")
}

/// A parameter definition (`pard`, 148 bytes): the type at byte 15, the name at 16, the default at
/// 56 (a float slider's `f64`, a checkbox's or popup's `u32`), a popup's item count at 60, a float
/// slider's valid range at 104 / 108 and slider range at 112 / 116 (`f32`).
#[derive(Clone)]
enum Def {
    Float { name: &'static str, default: f64, min: f32, max: f32 },
    Check { default: bool },
    Popup { name: &'static str, items: &'static [&'static str] },
    Data { name: &'static str },
    Group { name: &'static str, end: bool },
}

fn pard(d: &Def) -> Vec<Chunk<'static>> {
    let mut b = vec![0u8; 148];
    let name = |b: &mut Vec<u8>, n: &str| b[16..16 + n.len()].copy_from_slice(n.as_bytes());
    let mut extra = Vec::new();
    match d {
        Def::Float { name: n, default, min, max } => {
            b[15] = 10;
            name(&mut b, n);
            b[56..64].copy_from_slice(&default.to_be_bytes());
            b[104..108].copy_from_slice(&min.to_be_bytes());
            b[108..112].copy_from_slice(&max.to_be_bytes());
            b[112..116].copy_from_slice(&min.to_be_bytes());
            b[116..120].copy_from_slice(&max.to_be_bytes());
        }
        Def::Check { default } => {
            b[15] = 4;
            name(&mut b, " ");
            b[56..60].copy_from_slice(&u32::from(*default).to_be_bytes());
        }
        Def::Popup { name: n, items } => {
            b[15] = 7;
            name(&mut b, n);
            b[56..60].copy_from_slice(&1u32.to_be_bytes());
            b[60..62].copy_from_slice(&u16::try_from(items.len()).unwrap().to_be_bytes());
            extra.push(nested(*b"pdnm", items.join("|").as_bytes()));
        }
        Def::Data { name: n } => {
            b[15] = 11;
            name(&mut b, n);
        }
        Def::Group { name: n, end } => {
            b[15] = if *end { 14 } else { 13 };
            name(&mut b, n);
        }
    }
    let mut v = vec![Chunk::data(*b"pard", b)];
    v.extend(extra);
    v
}

/// A stored value.
#[derive(Clone)]
enum Rec {
    /// A number in a record of state `tdsb` (1 written, 3 the default state).
    Num(f64, u32),
    /// Arbitrary data.
    Data(Vec<u8>),
    /// A value record whose arbitrary data (`aRbs`) is missing.
    NoData,
}

/// A static value record after the one After Effects wrote for Glow's A & B Midpoint (its header,
/// one component).
fn record(template: &Chunk<'_>, tdsb: u32, v: f64) -> Chunk<'static> {
    let mut kids = Vec::new();
    for c in template.children() {
        match &c.body {
            Body::Data(_) if c.id == *b"tdsb" => kids.push(Chunk::data(*b"tdsb", tdsb.to_be_bytes().to_vec())),
            Body::Data(d) if c.id == *b"tdb4" => kids.push(Chunk::data(c.id, d.to_vec())),
            Body::Data(_) if c.id == *b"cdat" => {
                let mut d = vec![0u8; 40];
                d[..8].copy_from_slice(&v.to_be_bytes());
                kids.push(Chunk::data(*b"cdat", d));
            }
            _ => {}
        }
    }
    Chunk::list(*b"tdbs", kids)
}

/// The defaults of a fresh instance's data (1200 `lumetri-00-defaults`).
fn empty() -> Vec<u8> {
    vec![0xfe, 0xfe]
}

/// A Lumetri curve: type, point count, points (`f64` x, y), padded to 520 bytes.
fn curve(kind: i32, pts: &[(f64, f64)]) -> Vec<u8> {
    let mut d = kind.to_le_bytes().to_vec();
    d.extend(u32::try_from(pts.len()).unwrap().to_le_bytes());
    for (x, y) in pts {
        d.extend(x.to_le_bytes());
        d.extend(y.to_le_bytes());
    }
    d.resize(520, 0);
    d
}

fn rgb_curves() -> Vec<u8> {
    (0..4).flat_map(|_| curve(0, &[(0.0, 0.0), (1.0, 1.0)])).collect()
}

fn wheels(n: usize) -> Vec<u8> {
    (0..n).flat_map(|_| [0.0f64, 0.5, 0.0].into_iter().flat_map(f64::to_le_bytes)).collect()
}

fn utf16(s: &str) -> Vec<u8> {
    s.encode_utf16().chain([0]).flat_map(u16::to_le_bytes).collect()
}

/// HSL Secondary's key data as 1200 `lumetri-01-set-all` stores it (Set color RGB 0.02 0.55 0.85).
const HSL_PICKED: &str = "01327f0f3fcdcc4c3dcdcccc3d01d519683f2e57c93dcade173e01dfdede3ecdcc4c3dcdcccc3d01";
/// A fresh instance's key data (`-00-defaults`).
const HSL_FRESH: &str = "000000003f0000000000000000010000003f0000000000000000010000003f000000000000000001";

fn hex(s: &str) -> Vec<u8> {
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
}

const MASK_STYLE: &[&str] = &["Color/Gray", "Color/Black", "White/Black"];
const LOOKS: &[&str] = &["None", "[Custom]", "Browse...", "(-", "SL BIG", "SL BLUE ICE"];

/// The definitions, an order of our own (not After Effects' `parT` order) and without the ones
/// After Effects 2026 added late (-0127 ... -0130).
fn defs() -> Vec<(u16, Def)> {
    let f = |name, default, min, max| Def::Float { name, default, min, max };
    vec![
        (1, Def::Data { name: "Blob" }),
        (62, Def::Check { default: false }),
        (2, Def::Group { name: "Basic Correction", end: false }),
        (3, Def::Check { default: true }),
        (95, Def::Data { name: " " }),
        (64, f("HDR Specular", 0.0, -150.0, 150.0)),
        (16, f("Blacks", 0.0, -150.0, 150.0)),
        (11, f("Exposure", 0.0, -7.0, 7.0)),
        (12, f("Contrast", 0.0, -150.0, 150.0)),
        (7, f("Temperature", 0.0, -300.0, 300.0)),
        (21, Def::Group { name: "", end: true }),
        (22, Def::Group { name: "Creative", end: false }),
        (23, Def::Check { default: true }),
        (96, Def::Data { name: " " }),
        (25, Def::Popup { name: "Look", items: LOOKS }),
        (26, f("Intensity", 100.0, 0.0, 200.0)),
        (32, Def::Data { name: "Split Toning" }),
        (35, Def::Group { name: "", end: true }),
        (39, Def::Data { name: "RGB Curve" }),
        (104, Def::Check { default: true }),
        (42, Def::Data { name: "Hue vs Sat" }),
        (106, Def::Data { name: "Hue vs Hue" }),
        (47, Def::Data { name: "Wheels" }),
        (68, Def::Check { default: true }),
        (73, Def::Data { name: "HSL Sliders" }),
        (74, Def::Check { default: false }),
        (75, Def::Popup { name: " ", items: MASK_STYLE }),
        (85, Def::Data { name: "Color Wheels" }),
        (86, f("Temperature", 0.0, -300.0, 300.0)),
        (50, Def::Check { default: true }),
        (51, f("Amount", 0.0, -5.0, 5.0)),
        (54, f("Feather", 50.0, 0.0, 100.0)),
        (97, Def::Check { default: false }),
    ]
}

/// The data every instance stores, at the defaults.
fn default_data() -> Vec<(u16, Rec)> {
    vec![
        (1, Rec::Data(empty())),
        (95, Rec::Data(empty())),
        (96, Rec::Data(empty())),
        (32, Rec::Data(vec![0; 32])),
        (39, Rec::Data(rgb_curves())),
        (42, Rec::Data(curve(3, &[]))),
        (106, Rec::Data(curve(3, &[]))),
        (47, Rec::Data(wheels(4))),
        (73, Rec::Data(hex(HSL_FRESH))),
        (85, Rec::Data(wheels(8))),
    ]
}

/// The Lumetri instance: the definitions, then the stored values `recs` (data replaces the
/// default data of the same parameter).
fn sspc(defs: &[(u16, Def)], recs: &[(u16, Rec)], template: &Chunk<'_>) -> Chunk<'static> {
    let mut part = vec![Chunk::data(*b"parn", u32::try_from(defs.len() + 2).unwrap().to_be_bytes().to_vec()), tdmn(&mn(0))];
    let mut layer = vec![0u8; 148];
    layer[15] = 0;
    part.push(Chunk::data(*b"pard", layer));
    for (s, d) in defs {
        part.push(tdmn(&mn(*s)));
        part.extend(pard(d));
    }
    part.push(tdmn("ADBE Effect Built In Params"));
    let mut end = vec![0u8; 148];
    end[15] = 9;
    part.push(Chunk::data(*b"pard", end));
    let mut all: Vec<(u16, Rec)> = default_data().into_iter().filter(|(s, _)| !recs.iter().any(|(r, _)| r == s)).collect();
    all.extend(recs.iter().cloned());
    let mut values = vec![Chunk::data(*b"tdsb", 1u32.to_be_bytes().to_vec())];
    for (s, r) in all {
        values.push(tdmn(&mn(s)));
        match r {
            Rec::Num(v, tdsb) => values.push(record(template, tdsb, v)),
            Rec::Data(d) => {
                values.push(record(template, 1, 0.0));
                values.push(Chunk::list(*b"aRbs", vec![Chunk::data(*b"aRbp", d)]));
            }
            Rec::NoData => values.push(record(template, 1, 0.0)),
        }
    }
    Chunk::list(*b"sspc", vec![nested(*b"fnam", b"Lumetri Color"), Chunk::list(*b"parT", part), Chunk::list(*b"tdgp", values)])
}

fn find_glow_template<'a>(chunks: &'a [Chunk<'a>]) -> Option<Chunk<'a>> {
    for c in chunks {
        let kids = c.children();
        if let Some(at) = kids.iter().position(|k| k.is(b"tdmn") && k.bytes().is_some_and(|b| b.starts_with(b"ADBE Glo2-0011\0")))
            && let Some(record) = kids.get(at + 1).filter(|k| k.is_list(b"tdbs"))
        {
            return Some(record.clone());
        }
        if let Some(t) = find_glow_template(kids) {
            return Some(t);
        }
    }
    None
}

/// Puts the instance in place of the Glow effect (in the effect parade and the project's shared
/// definitions); the number of places.
fn replace_glow(chunks: &mut [Chunk<'_>], instance: &Chunk<'static>) -> usize {
    let mut n = 0;
    for ch in chunks {
        let kids = ch.children_mut();
        if let Some(at) = kids.iter().position(|k| k.is(b"tdmn") && k.bytes().is_some_and(|b| b.starts_with(b"ADBE Glo2\0")))
            && kids.get(at + 1).is_some_and(|k| k.is_list(b"sspc"))
        {
            kids[at] = tdmn("ADBE Lumetri");
            kids[at + 1] = instance.clone();
            n += 1;
        }
        n += replace_glow(kids, instance);
    }
    n
}

fn run_with(defs: &[(u16, Def)], recs: &[(u16, Rec)]) -> (PropGroup, ImportResult) {
    let file = parse(SYNTHETIC).unwrap();
    let template = find_glow_template(&file.chunks).expect("Glow's A & B Midpoint record");
    let instance = sspc(defs, recs, &template);
    let mut file = parse(SYNTHETIC).unwrap();
    assert_eq!(replace_glow(&mut file.chunks, &instance), 2, "the Glow effect and its shared definitions");
    let bytes = write(&file).unwrap();
    let mut project = Project::default();
    let none = |_: &str| false;
    let r = import(&mut project, &bytes, &ImportOptions { allow_losses: true, ..ImportOptions::new(&none) }).unwrap();
    let g = project
        .comps()
        .flat_map(|(_, c)| c.layers.iter())
        .find_map(|l| l.props.sub("effects").and_then(|fx| fx.children.iter().filter_map(|n| n.as_group()).find(|g| g.match_id == "ec.color.lumetri").cloned()))
        .expect("an EffectCraft Lumetri Color");
    (g, r)
}

fn run(recs: &[(u16, Rec)]) -> (PropGroup, ImportResult) {
    run_with(&defs(), recs)
}

fn losses(r: &ImportResult) -> Vec<String> {
    r.report
        .losses
        .iter()
        .filter(|l| matches!(l.category, LossCategory::Effects | LossCategory::DamagedData))
        .flat_map(|l| l.details.clone())
        .filter(|d| d.contains("Lumetri"))
        .collect()
}

fn num(g: &PropGroup, id: &str) -> f64 {
    g.prop(id).unwrap_or_else(|| panic!("no {id}")).value.as_f64()
}

fn val(g: &PropGroup, id: &str) -> Value {
    g.prop(id).unwrap_or_else(|| panic!("no {id}")).value.clone()
}

#[test]
fn a_fresh_lumetri_arrives_without_losses() {
    let (g, r) = run(&[]);
    assert_eq!(losses(&r), Vec::<String>::new());
    assert_eq!((num(&g, "basicCorrection/tone/exposure"), num(&g, "creative/lookIntensity"), num(&g, "vignette/vignetteFeather")), (0.0, 100.0, 50.0));
    assert_eq!(val(&g, "creative/look"), Value::Enum(0));
}

#[test]
fn lumetri_values_arrive_by_match_name_also_from_default_state_records() {
    // 1200 `lumetri-01-set-all` / `-06-sec-light`: High Dynamic Range and HDR Specular are
    // recorded in the default state (`tdsb` 3) and read back as set.
    let (g, r) = run(&[
        (62, Rec::Num(1.0, 3)),
        (64, Rec::Num(22.0, 3)),
        (11, Rec::Num(2.589_999_914_169_311_5, 1)),
        (12, Rec::Num(37.0, 1)),
        (16, Rec::Num(-150.0, 1)),
        (7, Rec::Num(300.0, 1)),
        (26, Rec::Num(134.0, 1)),
        (51, Rec::Num(1.85, 1)),
        (74, Rec::Num(1.0, 1)),
        (75, Rec::Num(3.0, 1)),
        (73, Rec::Data(hex(HSL_PICKED))),
    ]);
    assert_eq!(losses(&r), Vec::<String>::new());
    assert_eq!(val(&g, "highDynamicRange"), Value::Bool(true));
    for (id, want) in [
        ("basicCorrection/tone/hdrSpecular", 22.0),
        ("basicCorrection/tone/exposure", 2.589_999_914_169_311_5),
        ("basicCorrection/tone/contrast", 37.0),
        ("basicCorrection/tone/blacks", -150.0),
        // Past EffectCraft's old slider range: kept (the range is After Effects' now).
        ("basicCorrection/whiteBalance/temperature", 300.0),
        ("creative/lookIntensity", 134.0),
        ("vignette/vignetteAmount", 1.85),
    ] {
        assert_eq!(num(&g, id), want, "{id}");
    }
    assert_eq!((val(&g, "hslSecondary/key/showMask"), val(&g, "hslSecondary/key/maskStyle")), (Value::Bool(true), Value::Enum(2)));
}

#[test]
fn a_switched_off_section_is_reported_and_not_applied() {
    // 1200 `-03-sec-basic-correction`, `-17-sec-vignette`: a section that is off does nothing.
    let (g, r) = run(&[(3, Rec::Num(0.0, 1)), (11, Rec::Num(2.0, 1)), (50, Rec::Num(0.0, 1)), (51, Rec::Num(-2.0, 1)), (12, Rec::Num(37.0, 1))]);
    assert_eq!((num(&g, "basicCorrection/tone/exposure"), num(&g, "basicCorrection/tone/contrast"), num(&g, "vignette/vignetteAmount")), (0.0, 0.0, 0.0));
    let l = losses(&r);
    assert_eq!(l.len(), 2, "{l:?}");
    assert!(l[0].contains("section Basic Correction is switched off") && l[0].contains("Exposure 2, Contrast 37"), "{l:?}");
    assert!(l[1].contains("section Vignette is switched off") && l[1].contains("Amount -2"), "{l:?}");
}

#[test]
fn a_look_is_reported_by_name() {
    // 1200 `-01-set-all`: the Look's name in -0096, the popup's record in the default state.
    let (g, r) = run(&[(96, Rec::Data(utf16("SL BLUE ICE"))), (25, Rec::Num(6.0, 3))]);
    assert_eq!(val(&g, "creative/look"), Value::Enum(0));
    let l = losses(&r);
    assert_eq!(l.len(), 1, "{l:?}");
    assert!(l[0].contains("the Look 'SL BLUE ICE' is not imported"), "{l:?}");
    // Only the popup: its item.
    let (_, r) = run(&[(25, Rec::Num(5.0, 1))]);
    assert!(losses(&r).iter().any(|d| d.contains("the Look 'SL BIG' is not imported")), "{:?}", losses(&r));
}

#[test]
fn hue_vs_sat_arrives_with_its_y_scaled_other_moved_curves_are_reported() {
    // 1200 `-11-sec-hue-saturation-curves`: the selector put three neutral points on the curve.
    let neutral = curve(3, &[(0.742_483, 0.0), (0.798_038, 0.0), (0.853_594, 0.0)]);
    let (g, r) = run(&[(42, Rec::Data(neutral))]);
    assert_eq!(losses(&r), Vec::<String>::new());
    assert_eq!(val(&g, "curves/hueSaturationCurves/hueVsSat"), Value::Str(String::new()));
    // 1200 `-61-preset-curves-huesat`: red raised to 0.97 (After Effects nearly doubles its
    // saturation), the neighbours pinned near 0. EffectCraft's y: 0.5 + y / 2.
    let preset = curve(3, &[(0.015_463_917_525_773_196, 0.97), (0.262_886_597_938_144_34, -0.03), (0.764_604_810_996_563_5, -0.02)]);
    let (g, r) = run(&[(42, Rec::Data(preset))]);
    assert_eq!(losses(&r), Vec::<String>::new());
    let Value::Str(text) = val(&g, "curves/hueSaturationCurves/hueVsSat") else { panic!("a point list") };
    let pts: Vec<(f64, f64)> = text
        .split(' ')
        .map(|p| {
            let (x, y) = p.split_once(',').unwrap();
            (x.parse().unwrap(), y.parse().unwrap())
        })
        .collect();
    let want = [(0.015_464, 0.985), (0.262_887, 0.485), (0.764_605, 0.49)];
    assert_eq!(pts.len(), 3, "{text}");
    for ((x, y), (wx, wy)) in pts.iter().zip(want) {
        assert!((x - wx).abs() < 1e-5 && (y - wy).abs() < 1e-9, "{text}");
    }
    // Hue vs Hue moved: no sample tells its scale, reported and left neutral.
    let (g, r) = run(&[(106, Rec::Data(curve(3, &[(0.5, 0.25)])))]);
    let l = losses(&r);
    assert!(l.len() == 1 && l[0].contains("Hue vs Hue (ADBE Lumetri-0106) is not imported"), "{l:?}");
    assert_eq!(val(&g, "curves/hueSaturationCurves/hueVsHue"), Value::Str(String::new()));
    // A point count past the data: not a default, reported as damaged, no panic.
    let mut bad = curve(3, &[]);
    bad[4..8].copy_from_slice(&1000u32.to_le_bytes());
    let (g, r) = run(&[(42, Rec::Data(bad))]);
    let l = losses(&r);
    assert!(l.len() == 1 && l[0].contains("Hue vs Sat (ADBE Lumetri-0042) is unreadable"), "{l:?}");
    assert_eq!(val(&g, "curves/hueSaturationCurves/hueVsSat"), Value::Str(String::new()));
}

#[test]
fn the_master_rgb_curve_arrives_moved_channel_curves_are_reported() {
    // 1200 `-60-preset-curves-rgb`: the first of the four curves, its midpoint raised.
    let master = curve(0, &[(0.0, 0.0), (0.491_408_934_707_903_76, 0.786_885_245_901_639_3), (1.0, 1.0)]);
    let identity = curve(0, &[(0.0, 0.0), (1.0, 1.0)]);
    let mut d = master.clone();
    (0..3).for_each(|_| d.extend(&identity));
    let (g, r) = run(&[(39, Rec::Data(d))]);
    assert_eq!(losses(&r), Vec::<String>::new());
    assert_eq!(val(&g, "curves/rgbCurves/master"), Value::Str("0,0 0.49140893470790376,0.7868852459016393 1,1".into()));
    assert_eq!(val(&g, "curves/rgbCurves/red"), Value::Str("0,0 1,1".into()));
    // A moved channel curve: which channel it is is not known, reported.
    let mut d = identity.clone();
    d.extend(&master);
    (0..2).for_each(|_| d.extend(&identity));
    let (g, r) = run(&[(39, Rec::Data(d))]);
    let l = losses(&r);
    assert!(l.len() == 1 && l[0].contains("1 channel curve(s) of RGB Curve (ADBE Lumetri-0039) are not imported"), "{l:?}");
    assert_eq!(val(&g, "curves/rgbCurves/master"), Value::Str("0,0 1,1".into()));
    // Damaged: the wrong length.
    let (_, r) = run(&[(39, Rec::Data(master))]);
    assert!(losses(&r).iter().any(|x| x.contains("RGB Curve (ADBE Lumetri-0039) is unreadable")), "{:?}", losses(&r));
}

/// A value record without its arbitrary data is damaged, not the default: reported for the RGB
/// curve and the HSL key, the neighbours stay as they were.
#[test]
fn a_value_record_without_its_data_is_reported_as_damaged() {
    for (suffix, name) in [(39, "RGB Curve (ADBE Lumetri-0039)"), (73, "HSL Sliders (ADBE Lumetri-0073)")] {
        let (g, r) = run(&[(suffix, Rec::NoData)]);
        let l = losses(&r);
        assert!(l.len() == 1 && l[0].contains(name) && l[0].contains("is missing"), "{suffix}: {l:?}");
        assert!(r.report.losses.iter().any(|x| x.category == LossCategory::DamagedData), "{suffix}");
        // The grade stays at the default.
        assert_eq!(val(&g, "curves/rgbCurves/master"), Value::Str("0,0 1,1".into()));
        assert_eq!(num(&g, "hslSecondary/key/hueRange"), 180.0);
    }
}

#[test]
fn colour_wheels_arrive_as_colours_their_brightness_is_reported() {
    // 1200 `-62-preset-wheels`: Shadows towards blue with the slider down, Midtones towards red,
    // Highlights towards yellow-green with the slider up; the fourth wheel at its default.
    let w: Vec<u8> = [
        229.119_895_976_548_18,
        0.369_892_467_494_294_2,
        0.547_209_348_004_638,
        0.104_745_366_878_020_72,
        0.5,
        0.394_961_840_132_529_44,
        93.374_076_744_668_57,
        0.726_881_718_523_399_9,
        0.318_976_840_675_999_35,
        0.0,
        0.5,
        0.0,
    ]
    .into_iter()
    .flat_map(f64::to_le_bytes)
    .collect();
    let (g, r) = run(&[(47, Rec::Data(w.clone()))]);
    let colour = |id: &str| match val(&g, id) {
        Value::Color(c) => c,
        other => panic!("{id}: {other:?}"),
    };
    // The hue's direction: blue strongest in Shadows, red in Midtones, green in Highlights.
    let s = colour("colorWheels/shadowsWheel");
    assert!(s[2] > 0.77 && s[0] < 0.23 && (s[0] + s[2] - 1.0).abs() < 1e-9, "{s:?}");
    let m = colour("colorWheels/midtonesWheel");
    assert!((m[0] - 0.697_480_920_066_264_7).abs() < 1e-9 && m[1] > m[2], "{m:?}");
    let h = colour("colorWheels/highlightsWheel");
    assert!(h[1] > h[0] && h[0] > h[2], "{h:?}");
    let l = losses(&r);
    assert!(l.len() == 1 && l[0].contains("Shadows brightness 0.370") && l[0].contains("Highlights brightness 0.727"), "{l:?}");
    assert!(!l[0].contains("Midtones") && !l[0].contains("fourth"), "{l:?}");
    // The fourth wheel set: reported; damaged data: reported, the wheels stay grey.
    let mut w4 = w;
    w4[88..96].copy_from_slice(&0.3f64.to_le_bytes());
    let (_, r) = run(&[(47, Rec::Data(w4))]);
    assert!(losses(&r).iter().any(|x| x.contains("the fourth wheel")), "{:?}", losses(&r));
    let (g, r) = run(&[(47, Rec::Data(vec![1; 95]))]);
    assert!(losses(&r).iter().any(|x| x.contains("Wheels (ADBE Lumetri-0047) is unreadable")), "{:?}", losses(&r));
    assert_eq!(val(&g, "colorWheels/shadowsWheel"), Value::Color([0.5, 0.5, 0.5, 1.0]));
}

#[test]
fn the_picked_hsl_key_becomes_hue_saturation_and_lightness_ranges() {
    let (g, r) = run(&[(73, Rec::Data(hex(HSL_PICKED)))]);
    assert_eq!(losses(&r), Vec::<String>::new());
    let close = |id: &str, want: f64| assert!((num(&g, id) - want).abs() < 0.01, "{id}: {} != {want}", num(&g, id));
    // Hue centre 0.5605 of the circle, range 0.05, tolerance 0.1.
    close("hslSecondary/key/hueCenter", 201.79);
    close("hslSecondary/key/hueRange", 18.0);
    close("hslSecondary/key/hueSoftness", 36.0);
    // Lightness centre 0.435 +- 0.05, tolerance 0.1, as percent.
    close("hslSecondary/key/lightnessLow", 38.53);
    close("hslSecondary/key/lightnessHigh", 48.53);
    close("hslSecondary/key/lightnessSoftness", 10.0);
    assert_eq!(val(&g, "hslSecondary/key/hueEnabled"), Value::Bool(true));
    // With the key picked the correction applies; without, After Effects applies it to nothing
    // (1200 `-16-sec-correction` renders as `-00-defaults`): left out and reported.
    let (g, _) = run(&[(73, Rec::Data(hex(HSL_PICKED))), (86, Rec::Num(19.0, 1))]);
    assert_eq!(num(&g, "hslSecondary/correction/temperature"), 19.0);
    let (g, r) = run(&[(86, Rec::Num(19.0, 1))]);
    assert_eq!(num(&g, "hslSecondary/correction/temperature"), 0.0);
    let l = losses(&r);
    assert!(l.len() == 1 && l[0].contains("HSL Secondary has no key colour picked") && l[0].contains("Temperature 19"), "{l:?}");
    // Damaged key data: reported, the default key kept.
    let (g, r) = run(&[(73, Rec::Data(vec![7; 40]))]);
    assert!(losses(&r).iter().any(|d| d.contains("HSL Secondary key data is unreadable")), "{:?}", losses(&r));
    assert_eq!(num(&g, "hslSecondary/key/hueRange"), 180.0);
}

#[test]
fn undecoded_data_and_controls_without_counterpart_are_reported_when_set() {
    let mut split = vec![0u8; 32];
    split[0] = 1;
    let (_, r) = run(&[(32, Rec::Data(split)), (97, Rec::Num(1.0, 3)), (95, Rec::Data(utf16("my.cube")))]);
    let l = losses(&r);
    assert_eq!(l.len(), 3, "{l:?}");
    assert!(l.iter().any(|d| d.contains("Split Toning (ADBE Lumetri-0032) is not imported")), "{l:?}");
    assert!(l.iter().any(|d| d.contains("Legacy Tone And Saturation (ADBE Lumetri-0097) is on")), "{l:?}");
    assert!(l.iter().any(|d| d.contains("the Input LUT 'my.cube' is not imported")), "{l:?}");
}

#[test]
fn legacy_tone_on_is_reported_whatever_the_pard_default_says() {
    // After the project is opened and saved again, the `pard` default of Legacy Tone And
    // Saturation reads on (versions/1200-lumetri-26 / -25x / -24x; off in 1200 as the script
    // saved it): a record that says on is still reported, a missing record is not.
    let mut d = defs();
    if let Some((_, Def::Check { default })) = d.iter_mut().find(|(s, _)| *s == 97) {
        *default = true;
    }
    let (_, r) = run_with(&d, &[(97, Rec::Num(1.0, 3))]);
    let l = losses(&r);
    assert!(l.len() == 1 && l[0].contains("Legacy Tone And Saturation (ADBE Lumetri-0097) is on"), "{l:?}");
    let (_, r) = run_with(&d, &[]);
    assert_eq!(losses(&r), Vec::<String>::new());
}

#[test]
fn an_unknown_scale_or_parameter_is_reported_not_guessed() {
    let mut d = defs();
    // Exposure with another valid range (an older or newer version): not guessed at.
    if let Some((_, Def::Float { min, max, .. })) = d.iter_mut().find(|(s, _)| *s == 11) {
        (*min, *max) = (-100.0, 100.0);
    }
    // A parameter this importer does not know, set.
    d.push((200, Def::Float { name: "Future", default: 0.0, min: -1.0, max: 1.0 }));
    let (g, r) = run_with(&d, &[(11, Rec::Num(3.0, 1)), (200, Rec::Num(0.5, 1))]);
    assert_eq!(num(&g, "basicCorrection/tone/exposure"), 0.0);
    let l = losses(&r);
    assert!(l.iter().any(|x| x.contains("Exposure (ADBE Lumetri-0011) has the valid maximum Some(100.0), expected 7")), "{l:?}");
    assert!(l.iter().any(|x| x.contains("Future (ADBE Lumetri-0200) not imported")), "{l:?}");
}
