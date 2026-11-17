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
        keys: vec![],
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
        keys: vec![],
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
    // The kind is byte 0 of a record: the definition default of `extractor-00-defaults` has
    // `02 00 00 32` on Alpha, still the input's own channel.
    let c = ExtractorChannels::parse(&channels([(1, "R"), (1, "G"), (1, "B"), (0x3200_0002, "(copy)")])).expect("parses");
    assert!(c.is_default());
    assert_eq!(c.sources(effectcraft_effects::EXTRACTOR_COPY), Ok(["R", "G", "B", "(copy)"].map(String::from)));
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
    // After Effects' `(copy)` Alpha is the input's alpha: EffectCraft's `(copy)`, not `A` (an
    // OpenEXR's own alpha plane).
    for (id, want) in [("red", "R"), ("green", "G"), ("blue", "B"), ("alpha", "(copy)")] {
        assert_eq!(v(&g, id), Value::Str(want.into()), "{id}");
    }
}

/// `extractor-07-preset-channel-z` (a preset of our own): `Z` into Red, Green and Blue, Alpha
/// `(copy)`. EffectCraft's EXtractoR reads the footage's channel by the same name.
#[test]
fn extractor_channels_are_imported_by_name() {
    let (g, r) = run(&extractor(vec![], channels([(1, "Z"), (1, "Z"), (1, "Z"), (2, "(copy)")])), "ec.3d.extractor");
    assert_eq!(losses(&r, LossCategory::Effects, "EXtractoR"), Vec::<String>::new());
    let g = g.expect("imported");
    for (id, want) in [("red", "Z"), ("green", "Z"), ("blue", "Z"), ("alpha", "(copy)")] {
        assert_eq!(v(&g, id), Value::Str(want.into()), "{id}");
    }
    // `(copy)` on another channel is that channel of the input (EffectCraft's `(copy)` there).
    let (g, _) = run(&extractor(vec![], channels([(2, "(copy)"), (1, "CryptoObject00.R"), (2, "(copy)"), (1, "Z")])), "ec.3d.extractor");
    let g = g.expect("imported");
    for (id, want) in [("red", "(copy)"), ("green", "CryptoObject00.R"), ("blue", "(copy)"), ("alpha", "Z")] {
        assert_eq!(v(&g, id), Value::Str(want.into()), "{id}");
    }
}

#[test]
fn extractor_channels_of_an_unknown_kind_are_reported() {
    for recs in [[(1, "Z"), (3, "Z"), (1, "Z"), (2, "(copy)")], [(1, "Z"), (1, ""), (1, "Z"), (2, "(copy)")]] {
        let (g, r) = run(&extractor(vec![], channels(recs)), "ec.3d.extractor");
        let found = losses(&r, LossCategory::Effects, "EXtractoR");
        assert!(found.len() == 1 && found[0].contains("are not imported (record 2 is of an unknown kind or names no channel)"), "{recs:?}: {found:?}");
        assert_eq!(v(&g.expect("imported"), "red"), Value::Str("R".into()), "{recs:?}");
    }
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
        keys: vec![],
    }
}

#[test]
fn identifier_display_id_and_channel_type() {
    assert_eq!(identifier_channel_type(&id_info("Object ID")), Some("Object ID"));
    // `identifier-01-set-all` (ID 10) and `identifier-05-pop2-3` (Alpha Matte): no channel chosen
    // (After Effects found none or none was picked), so After Effects leaves the layer unchanged.
    let (g, r) = run(&identifier(vec![(2, 3.0), (3, 10.0)], id_info("Object ID")), "ec.3d.identifier");
    let found = losses(&r, LossCategory::Effects, "IDentifier");
    assert!(found.len() == 1 && found[0].contains("no ID channel is chosen"), "{found:?}");
    let g = g.expect("imported");
    assert_eq!((v(&g, "display"), v(&g, "channelType")), (Value::Enum(2), Value::Enum(0)));
    near(&g, "id", 10.0);
}

/// `identifier-09-preset-channel-objectid-id2` (1310, a preset of our own): the UINT channel
/// `objectId` chosen and ID 2. EffectCraft's IDentifier reads that channel itself (`ObjectID`
/// without regard to case), so nothing is lost; another name is reported.
#[test]
fn identifier_chosen_channels_by_name() {
    for (name, kind) in [("objectId", 0), ("ObjectID", 0), ("materialid", 1)] {
        let (g, r) = run(&identifier(vec![(3, 2.0)], id_info(name)), "ec.3d.identifier");
        assert_eq!(losses(&r, LossCategory::Effects, "IDentifier"), Vec::<String>::new(), "{name}");
        let g = g.expect("imported");
        assert_eq!(v(&g, "channelType"), Value::Enum(kind), "{name}");
        near(&g, "id", 2.0);
    }
    for name in ["IndexOB.X", "ID", "Material ID"] {
        let (_, r) = run(&identifier(vec![], id_info(name)), "ec.3d.identifier");
        let found = losses(&r, LossCategory::Effects, "IDentifier");
        assert!(found.len() == 1 && found[0].contains(&format!("the ID channel '{name}' (Channel Info) is not imported")), "{name}: {found:?}");
    }
}

/// EXtractoR's data with the name of record `k` from byte 35 replaced by `name` (no NUL added; the
/// rest of the record's name field is left as `fill`).
fn channels_with_raw_name(k: usize, name: &[u8], fill: u8) -> Vec<u8> {
    let mut d = channels(DEFAULT_CHANNELS);
    let at = 32 + k * 164 + 35;
    d[at..at + 129].fill(fill);
    d[at..at + name.len()].copy_from_slice(name);
    d
}

/// A channel name that never ends (no NUL up to the end of its record) or is not UTF-8 is
/// refused by the decoder and reported at the importer; the defaults R, G, B, A stay.
#[test]
fn extractor_names_without_a_terminator_or_in_invalid_utf8_are_refused() {
    let unterminated = channels_with_raw_name(2, b"Z", b'Z');
    let invalid = channels_with_raw_name(1, &[0x5a, 0xff, 0xfe, 0x00], 0);
    for (what, data) in [("unterminated", unterminated), ("invalid UTF-8", invalid)] {
        assert_eq!(ExtractorChannels::parse(&data), None, "{what}");
        let (g, r) = run(&extractor(vec![], data), "ec.3d.extractor");
        let found = losses(&r, LossCategory::Effects, "EXtractoR");
        assert!(found.len() == 1 && found[0].contains("not in the sampled layout"), "{what}: {found:?}");
        let g = g.expect("imported");
        for (id, want) in [("red", "R"), ("green", "G"), ("blue", "B"), ("alpha", "A")] {
            assert_eq!(v(&g, id), Value::Str(want.into()), "{what}: {id}");
        }
    }
}

/// The same for IDentifier: a channel name without NUL up to the end of the data, or in invalid
/// UTF-8, is refused and reported; the Object ID channel stays.
#[test]
fn identifier_names_without_a_terminator_or_in_invalid_utf8_are_refused() {
    let mut unterminated = id_info("objectId");
    unterminated[19..].fill(b'x');
    let mut invalid = id_info("objectId");
    invalid[19..22].copy_from_slice(&[b'o', 0xff, 0]);
    for (what, data) in [("unterminated", unterminated), ("invalid UTF-8", invalid)] {
        assert_eq!(identifier_channel_type(&data), None, "{what}");
        let (g, r) = run(&identifier(vec![], data), "ec.3d.identifier");
        let found = losses(&r, LossCategory::Effects, "IDentifier");
        assert!(found.len() == 1 && found[0].contains("not in the sampled layout"), "{what}: {found:?}");
        assert_eq!(v(&g.expect("imported"), "channelType"), Value::Enum(0), "{what}");
    }
    // Data too short to reach the name.
    assert_eq!(identifier_channel_type(&[1; 10]), None);
}

/// A Channel Info value record without its arbitrary data (no `aRbs`): the effect still imports,
/// with the defaults, nothing panics, and the missing data is reported as damaged (it may have
/// chosen other channels, so the defaults are not what After Effects renders).
#[test]
fn a_missing_channel_info_payload_is_reported_and_leaves_the_defaults() {
    let mut fx = extractor(vec![], vec![]);
    fx.data.clear();
    let (g, r) = run(&fx, "ec.3d.extractor");
    assert_eq!(losses(&r, LossCategory::Effects, "EXtractoR"), Vec::<String>::new());
    let found = losses(&r, LossCategory::DamagedData, "EXtractoR");
    assert!(found.len() == 1 && found[0].contains("Channel Info data is missing"), "{found:?}");
    let g = g.expect("imported");
    for (id, want) in [("red", "R"), ("green", "G"), ("blue", "B"), ("alpha", "A")] {
        assert_eq!(v(&g, id), Value::Str(want.into()), "{id}");
    }
    let mut fx = identifier(vec![(3, 5.0)], vec![]);
    fx.data.clear();
    let (g, r) = run(&fx, "ec.3d.identifier");
    assert_eq!(losses(&r, LossCategory::Effects, "IDentifier"), Vec::<String>::new());
    let found = losses(&r, LossCategory::DamagedData, "IDentifier");
    assert!(found.len() == 1 && found[0].contains("Channel Info data is missing"), "{found:?}");
    let g = g.expect("imported");
    assert_eq!(v(&g, "channelType"), Value::Enum(0));
    near(&g, "id", 5.0);
}
