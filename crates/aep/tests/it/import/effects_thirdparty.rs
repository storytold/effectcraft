//! CC Color Neutralizer, EXtractoR and IDentifier (#3112) arrive as EffectCraft's effects of the
//! same name. Each is built into `synthetic.aep` (`fx_inject`) with the definitions After Effects
//! 2026 wrote for it in the importer block of 2026-11-05 (`1300-thirdparty.aep`) and the values
//! of that block's compositions; the Channel Info data is laid out as those instances store it.

use crate::import::fx_inject::{Default, Fx, losses, near, other, popup, run, slider, v};
use effectcraft_aep::import::LossCategory;
use effectcraft_aep::{ExtractorChannels, ParamKind as K, identifier_channel_type};
use effectcraft_keyframe::Value;

fn neutralizer(values: Vec<(usize, f64)>) -> Fx {
    let range = |name| slider(K::FloatSlider, name, 0.0, (-100.0, 100.0));
    let group = |name| other(K::GroupStart, name, Default::None);
    let end = || other(K::GroupEnd, "", Default::None);
    Fx {
        match_name: "CS Color Neutralizer",
        name: "CC Color Neutralizer",
        defs: vec![
            other(K::Color, "Shadows Unbalance", Default::Color([0, 0, 0, 0])),
            group("Shadows"),
            range("Red - Shadows"),
            range("Green - Shadows"),
            range("Blue - Shadows"),
            end(),
            other(K::Color, "Midtones Unbalance", Default::Color([255, 127, 127, 127])),
            group("Midtones"),
            range("Red - Midtones"),
            range("Green - Midtones"),
            range("Blue - Midtones"),
            end(),
            other(K::Color, "Highlights Unbalance", Default::Color([255, 255, 255, 255])),
            group("Highlights"),
            range("Red - Highlights"),
            range("Green - Highlights"),
            range("Blue - Highlights"),
            end(),
            slider(K::FloatSlider, "Pinning", 0.0, (0.0, 100.0)),
            slider(K::FloatSlider, "Blend w. Original", 0.0, (0.0, 100.0)),
            group("Special"),
            popup("View", 1, &["Final Result", "Correction B to W", "Correction Stair"]),
            slider(K::FloatSlider, "Black Point", 0.0, (0.0, 49.9)),
            slider(K::FloatSlider, "White Point", 100.0, (50.1, 100.0)),
            end(),
        ],
        values,
        data: vec![],
    }
}

#[test]
fn cc_color_neutralizer_pinning_and_blend() {
    // `ccneutral-00-defaults` and the Pinning / Blend of `ccneutral-01-set-all`.
    let (g, r) = run(&neutralizer(vec![(19, 16.0), (20, 19.0)]), "ec.color.cccolorneutralizer");
    assert_eq!(losses(&r, LossCategory::Effects, "CS Color Neutralizer"), Vec::<String>::new());
    let g = g.expect("imported");
    near(&g, "pinning", 16.0);
    near(&g, "blendWOriginal", 19.0);
    // EffectCraft's own (identity) balance stays.
    assert_eq!(v(&g, "midtonesUnbalance"), v(&g, "midtonesBalance"));
}

#[test]
fn cc_color_neutralizer_corrections_without_a_counterpart_are_reported() {
    // `ccneutral-03-sec-shadows`, `ccneutral-09-pop22-2`, `ccneutral-06-sec-special`.
    let (g, r) = run(&neutralizer(vec![(3, 13.0), (4, 16.0), (5, 19.0), (22, 2.0), (23, 10.0), (24, 87.0)]), "ec.color.cccolorneutralizer");
    let found = losses(&r, LossCategory::Effects, "CS Color Neutralizer");
    for name in ["Red - Shadows", "Green - Shadows", "Blue - Shadows", "View", "Black Point", "White Point"] {
        assert!(found.iter().any(|d| d.contains(&format!("parameter {name} (")) && d.contains("not imported")), "{name}: {found:?}");
    }
    assert_eq!(found.len(), 6, "{found:?}");
    assert!(g.is_some(), "the effect itself is imported");
}

/// EXtractoR's Channel Info data with these record kinds and names.
fn channels(recs: [(u32, &str); 4]) -> Vec<u8> {
    let mut d = vec![0u8; ExtractorChannels::LEN];
    d[..4].copy_from_slice(&1u32.to_le_bytes());
    for (k, (kind, name)) in recs.iter().enumerate() {
        let at = 32 + k * 164;
        d[at..at + 4].copy_from_slice(&kind.to_le_bytes());
        d[at + 35..at + 35 + name.len()].copy_from_slice(name.as_bytes());
    }
    d
}

const DEFAULT_CHANNELS: [(u32, &str); 4] = [(1, "R"), (1, "G"), (1, "B"), (2, "(copy)")];

fn extractor(values: Vec<(usize, f64)>, data: Vec<u8>) -> Fx {
    let mut values = values;
    values.insert(0, (1, 0.0));
    Fx {
        match_name: "EXtractoR",
        name: "EXtractoR",
        defs: vec![
            other(K::ArbitraryData, "Channel Info", Default::None),
            other(K::GroupStart, "Process", Default::None),
            slider(K::FloatSlider, "Black Point", 0.0, (-f64::from(f32::MAX), f64::from(f32::MAX))),
            slider(K::FloatSlider, "White Point", 1.0, (-f64::from(f32::MAX), f64::from(f32::MAX))),
            other(K::Checkbox, "", Default::Number(0.0)),
            other(K::GroupEnd, "", Default::None),
        ],
        values,
        data: vec![(1, data)],
    }
}

#[test]
fn extractor_channel_info_layout() {
    let c = ExtractorChannels::parse(&channels(DEFAULT_CHANNELS)).expect("the default layout");
    assert!(c.is_default());
    let c = ExtractorChannels::parse(&channels([(1, "Z"), (1, "G"), (1, "B"), (2, "(copy)")])).expect("parses");
    assert!(!c.is_default());
    assert_eq!(c.channels[0], (1, "Z".to_string()));
    assert_eq!(ExtractorChannels::parse(&[0u8; 10]), None);
}

#[test]
fn extractor_points_unmult_and_default_channels() {
    // `extractor-01-set-all`: Black Point 10, White Point 14, UnMult on.
    let (g, r) = run(&extractor(vec![(3, 10.0), (4, 14.0), (5, 1.0)], channels(DEFAULT_CHANNELS)), "ec.3d.extractor");
    assert_eq!(losses(&r, LossCategory::Effects, "EXtractoR"), Vec::<String>::new());
    let g = g.expect("imported");
    near(&g, "blackPoint", 10.0);
    near(&g, "whitePoint", 14.0);
    assert_eq!((v(&g, "unMult"), v(&g, "clip")), (Value::Bool(true), Value::Bool(false)));
    // After Effects' `(copy)` Alpha is the input's alpha: EffectCraft's A.
    for (id, want) in [("red", "R"), ("green", "G"), ("blue", "B"), ("alpha", "A")] {
        assert_eq!(v(&g, id), Value::Str(want.into()), "{id}");
    }
}

#[test]
fn extractor_channels_other_than_the_default_are_reported() {
    let (g, r) = run(&extractor(vec![], channels([(1, "Z"), (1, "Z"), (1, "Z"), (2, "(copy)")])), "ec.3d.extractor");
    let found = losses(&r, LossCategory::Effects, "EXtractoR");
    assert!(found.len() == 1 && found[0].contains("the channels Z, Z, Z, (copy) are not imported"), "{found:?}");
    assert_eq!(v(&g.expect("imported"), "red"), Value::Str("R".into()));
}

/// IDentifier's Channel Info: the channel type's name at 19 among bytes that are not read.
fn id_info(name: &str) -> Vec<u8> {
    let mut d: Vec<u8> = (0..148u32).map(|i| (i * 37 % 251) as u8 | 1).collect();
    d[19..19 + name.len()].copy_from_slice(name.as_bytes());
    d[19 + name.len()] = 0;
    d
}

fn identifier(values: Vec<(usize, f64)>, data: Vec<u8>) -> Fx {
    let mut values = values;
    values.insert(0, (1, 0.0));
    Fx {
        match_name: "IDentifier",
        name: "IDentifier",
        defs: vec![
            other(K::ArbitraryData, "Channel Info", Default::None),
            popup("Display", 1, &["Colors", "Luma Matte", "Alpha Matte", "Raw"]),
            slider(K::IntSlider, "ID", 0.0, (0.0, 2_147_483_647.0)),
        ],
        values,
        data: vec![(1, data)],
    }
}

#[test]
fn identifier_display_id_and_channel_type() {
    assert_eq!(identifier_channel_type(&id_info("Object ID")), Some("Object ID"));
    // `identifier-01-set-all` (ID 10) and `identifier-05-pop2-3` (Alpha Matte).
    let (g, r) = run(&identifier(vec![(2, 3.0), (3, 10.0)], id_info("Object ID")), "ec.3d.identifier");
    // Which channel After Effects reads is not decoded (it left the sampled EXR unchanged): reported.
    let found = losses(&r, LossCategory::Effects, "IDentifier");
    assert!(found.len() == 1 && found[0].contains("is not decoded"), "{found:?}");
    let g = g.expect("imported");
    assert_eq!((v(&g, "display"), v(&g, "channelType")), (Value::Enum(2), Value::Enum(0)));
    near(&g, "id", 10.0);
    // A channel type that is not sampled is reported.
    let (_, r) = run(&identifier(vec![], id_info("Material ID")), "ec.3d.identifier");
    let found = losses(&r, LossCategory::Effects, "IDentifier");
    assert!(found.len() == 1 && found[0].contains("\"Material ID\""), "{found:?}");
}
