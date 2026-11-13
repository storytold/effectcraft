//! Builds an effect instance into the committed `synthetic.aep` in place of its Glow (the instance
//! on the layer and the project's shared definitions), so an effect mapping can be tested without
//! the external corpus. The parameter definitions are written in the `pard` layout of
//! docs/aep-format.md 3.7 (type at byte 15, name at 16, default at 56, a float slider's valid
//! range as `f32` at 104 / 108 and its slider range at 112 / 116, a fixed or integer slider's valid
//! maximum at 128, a popup's item count at 60); `pard_layout_decodes_as_written` reads them back.
//! The ranges and defaults the tests use are the ones After Effects 2026 wrote in the importer
//! block of 2026-11-05 (`1100-quickwins.aep`, `1300-thirdparty.aep`, own projects).

use effectcraft_aep::import::{ImportOptions, ImportResult, LossCategory, import};
use effectcraft_aep::{Body, Chunk, ParamKind, ParamValue, Project as AepProject, parse, write};
use effectcraft_keyframe::Value;
use effectcraft_project::{GroupKind, Project, PropGroup};

const SYNTHETIC: &[u8] = include_bytes!("../../fixtures/synthetic.aep");

/// A parameter definition.
#[derive(Clone)]
pub struct Def {
    pub kind: ParamKind,
    pub name: &'static str,
    pub default: Default,
    /// A float slider's valid range, a fixed or integer slider's valid maximum (`.1`).
    pub valid: (f64, f64),
    /// A popup's items.
    pub items: &'static [&'static str],
}

/// The default a definition holds at byte 56.
#[derive(Clone, Copy)]
pub enum Default {
    None,
    Number(f64),
    Index(u32),
    Point(f64, f64),
    /// ARGB 0..255.
    Color([u8; 4]),
}

/// The parameter type code `pard` byte 15 holds for `k`.
fn kind_code(k: ParamKind) -> u8 {
    match k {
        ParamKind::Layer => 0,
        ParamKind::IntSlider => 1,
        ParamKind::FixedSlider => 2,
        ParamKind::Angle => 3,
        ParamKind::Checkbox => 4,
        ParamKind::Color => 5,
        ParamKind::Point => 6,
        ParamKind::Popup => 7,
        ParamKind::NoData => 9,
        ParamKind::FloatSlider => 10,
        ParamKind::ArbitraryData => 11,
        ParamKind::GroupStart => 13,
        ParamKind::GroupEnd => 14,
        ParamKind::Button => 15,
        _ => 9,
    }
}

/// `v` as a big-endian 16.16 fixed-point value.
fn fixed(v: f64) -> [u8; 4] {
    ((v * 65536.0).round() as i32).to_be_bytes()
}

/// The 148-byte parameter definition (`pard`) of `d`: type, name, default and range.
pub fn pard(d: &Def) -> Chunk<'static> {
    let mut b = vec![0u8; 148];
    b[15] = kind_code(d.kind);
    b[16..16 + d.name.len()].copy_from_slice(d.name.as_bytes());
    match (d.kind, d.default) {
        (ParamKind::FloatSlider, Default::Number(v)) => b[56..64].copy_from_slice(&v.to_be_bytes()),
        (ParamKind::FixedSlider | ParamKind::Angle, Default::Number(v)) => b[56..60].copy_from_slice(&fixed(v)),
        (ParamKind::IntSlider | ParamKind::Checkbox, Default::Number(v)) => b[56..60].copy_from_slice(&(v as i32).to_be_bytes()),
        (ParamKind::Popup, Default::Index(i)) => b[56..60].copy_from_slice(&i.to_be_bytes()),
        (ParamKind::Point, Default::Point(x, y)) => {
            b[56..60].copy_from_slice(&fixed(x));
            b[60..64].copy_from_slice(&fixed(y));
        }
        (ParamKind::Color, Default::Color(c)) => b[56..60].copy_from_slice(&c),
        _ => {}
    }
    match d.kind {
        ParamKind::FloatSlider => {
            b[104..108].copy_from_slice(&(d.valid.0 as f32).to_be_bytes());
            b[108..112].copy_from_slice(&(d.valid.1 as f32).to_be_bytes());
            b[112..116].copy_from_slice(&(d.valid.0 as f32).to_be_bytes());
            b[116..120].copy_from_slice(&(d.valid.1 as f32).to_be_bytes());
        }
        ParamKind::FixedSlider => b[128..132].copy_from_slice(&fixed(d.valid.1)),
        ParamKind::IntSlider => b[128..132].copy_from_slice(&(d.valid.1 as i32).to_be_bytes()),
        ParamKind::Popup => b[60..62].copy_from_slice(&(d.items.len() as u16).to_be_bytes()),
        _ => {}
    }
    Chunk::data(*b"pard", b)
}

/// A match name chunk (`tdmn`, 40 bytes).
pub fn tdmn(name: &str) -> Chunk<'static> {
    let mut b = name.as_bytes().to_vec();
    b.resize(40, 0);
    Chunk::data(*b"tdmn", b)
}

/// A UTF-8 string chunk (`Utf8`).
pub fn utf8(s: &str) -> Chunk<'static> {
    Chunk::data(*b"Utf8", s.as_bytes().to_vec())
}

/// A chunk whose body is chunks without being a `LIST` (`pdnm`, `fnam`: `NESTED_IDS`).
pub fn nested(id: [u8; 4], children: Vec<Chunk<'static>>) -> Chunk<'static> {
    Chunk { id, body: Body::Chunks(children), pad: None }
}

/// A static value record holding `v`, after the one After Effects wrote for Glow's A & B Midpoint
/// (its header; without the expression).
pub fn record(template: &Chunk<'_>, v: f64) -> Chunk<'static> {
    let mut kids = Vec::new();
    for c in template.children() {
        match &c.body {
            Body::Data(d) if c.id == *b"tdsb" || c.id == *b"tdb4" => kids.push(Chunk::data(c.id, d.to_vec())),
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

/// A keyframed value record: `record`'s header with a key list of linear one-component keys.
pub fn keyed_record(template: &Chunk<'_>, keys: &[(i32, f64)]) -> Chunk<'static> {
    let Body::List { children, .. } = record(template, keys.first().map_or(0.0, |k| k.1)).body else { unreachable!("record builds a LIST") };
    let mut children = children;
    let mut lhd3 = vec![0u8; 52];
    lhd3[0..4].copy_from_slice(&0x00d0_0beeu32.to_be_bytes());
    lhd3[8..12].copy_from_slice(&(keys.len() as u32).to_be_bytes());
    lhd3[16..20].copy_from_slice(&48u32.to_be_bytes());
    let mut ldat = Vec::new();
    for (time, value) in keys {
        let mut rec = vec![0u8; 48];
        rec[0..4].copy_from_slice(&time.to_be_bytes());
        rec[4] = 1;
        rec[5] = 1;
        rec[8..16].copy_from_slice(&value.to_be_bytes());
        ldat.extend(rec);
    }
    children.push(Chunk::list(*b"list", vec![Chunk::data(*b"lhd3", lhd3), Chunk::data(*b"ldat", ldat)]));
    Chunk::list(*b"tdbs", children)
}

/// An effect instance: its match name, display name, parameter definitions (after the input
/// layer, before the built-in entry), the 1-D values stored (`parT` index, value) and the
/// arbitrary data stored (`parT` index, bytes).
pub struct Fx {
    pub match_name: &'static str,
    pub name: &'static str,
    pub defs: Vec<Def>,
    pub values: Vec<(usize, f64)>,
    pub data: Vec<(usize, Vec<u8>)>,
    /// Keyframed 1-D parameters (`parT` index, (time units, value) per key), linear keys; the
    /// parameter then has no entry in `values`.
    pub keys: Vec<(usize, Vec<(i32, f64)>)>,
}

/// The match name of parameter `i` of `fx` (`<match>-NNNN`).
fn param_name(fx: &Fx, i: usize) -> String {
    format!("{}-{i:04}", fx.match_name)
}

/// The effect's `sspc`: its name, the parameter definitions (`parT`) and the stored values
/// (`tdgp`), each value record copied from `template`.
fn sspc(fx: &Fx, template: &Chunk<'_>) -> Chunk<'static> {
    let n = fx.defs.len() + 2;
    let mut part = vec![
        Chunk::data(*b"parn", (n as u32).to_be_bytes().to_vec()),
        tdmn(&param_name(fx, 0)),
        pard(&Def { kind: ParamKind::Layer, name: "", default: Default::None, valid: (0.0, 0.0), items: &[] }),
    ];
    for (i, d) in fx.defs.iter().enumerate() {
        part.push(tdmn(&param_name(fx, i + 1)));
        part.push(pard(d));
        if d.kind == ParamKind::Popup {
            part.push(nested(*b"pdnm", vec![utf8(&d.items.join("|"))]));
        }
    }
    part.push(tdmn("ADBE Effect Built In Params"));
    part.push(pard(&Def { kind: ParamKind::NoData, name: "", default: Default::None, valid: (0.0, 0.0), items: &[] }));
    let mut values = vec![Chunk::data(*b"tdsb", 1u32.to_be_bytes().to_vec())];
    for (i, v) in &fx.values {
        values.push(tdmn(&param_name(fx, *i)));
        values.push(record(template, *v));
        if let Some((_, d)) = fx.data.iter().find(|(j, _)| j == i) {
            values.push(Chunk::list(*b"aRbs", vec![Chunk::data(*b"aRbp", d.clone())]));
        }
    }
    for (i, keys) in &fx.keys {
        values.push(tdmn(&param_name(fx, *i)));
        values.push(keyed_record(template, keys));
    }
    Chunk::list(*b"sspc", vec![nested(*b"fnam", vec![utf8(fx.name)]), Chunk::list(*b"parT", part), Chunk::list(*b"tdgp", values)])
}

/// The value record of Glow's A & B Midpoint (`ADBE Glo2-0011`): a 1-D static record.
fn find_glow_template(chunks: &[Chunk<'_>]) -> Option<Chunk<'static>> {
    for c in chunks {
        let kids = c.children();
        if let Some(at) = kids.iter().position(|k| k.is(b"tdmn") && k.bytes().is_some_and(|b| b.starts_with(b"ADBE Glo2-0011\0")))
            && let Some(record) = kids.get(at + 1).filter(|k| k.is_list(b"tdbs"))
        {
            return Some(Chunk::list(
                *b"tdbs",
                record
                    .children()
                    .iter()
                    .map(|k| match &k.body {
                        Body::Data(d) => Chunk::data(k.id, d.to_vec()),
                        _ => Chunk::data(k.id, Vec::new()),
                    })
                    .collect(),
            ));
        }
        if let Some(t) = find_glow_template(kids) {
            return Some(t);
        }
    }
    None
}

/// Replaces every Glow (`ADBE Glo2` with its `sspc`) below `chunks` with the effect `match_name`
/// and its `sspc`; the number replaced.
fn replace_glow(chunks: &mut [Chunk<'_>], match_name: &str, sspc: &Chunk<'static>) -> usize {
    let mut n = 0;
    for ch in chunks {
        let kids = ch.children_mut();
        if let Some(at) = kids.iter().position(|k| k.is(b"tdmn") && k.bytes().is_some_and(|b| b.starts_with(b"ADBE Glo2\0")))
            && kids.get(at + 1).is_some_and(|k| k.is_list(b"sspc"))
        {
            kids[at] = tdmn(match_name);
            kids[at + 1] = sspc.clone();
            n += 1;
        }
        n += replace_glow(kids, match_name, sspc);
    }
    n
}

/// `synthetic.aep` with the effect `match_name` in place of the Glow effect; `make` builds its
/// `sspc` from the Glow's value record (the template of every record).
pub fn bytes_with_effect(match_name: &str, make: impl Fn(&Chunk<'_>) -> Chunk<'static>) -> Vec<u8> {
    let file = parse(SYNTHETIC).unwrap();
    let template = find_glow_template(&file.chunks).expect("Glow's A & B Midpoint record");
    let sspc = make(&template);
    let mut file = parse(SYNTHETIC).unwrap();
    assert_eq!(replace_glow(&mut file.chunks, match_name, &sspc), 2, "the Glow effect and its shared definitions");
    write(&file).unwrap()
}

/// `synthetic.aep` with `fx` in place of the Glow effect.
pub fn bytes_with(fx: &Fx) -> Vec<u8> {
    bytes_with_effect(fx.match_name, |t| sspc(fx, t))
}

/// Imports `synthetic.aep` with `fx` in place of its Glow: the EffectCraft effect `effect` (if
/// imported) and the result.
pub fn run(fx: &Fx, effect: &str) -> (Option<PropGroup>, ImportResult) {
    import_bytes(&bytes_with(fx), effect)
}

/// Imports `bytes`: the first EffectCraft effect `effect` (if imported) and the result.
pub fn import_bytes(bytes: &[u8], effect: &str) -> (Option<PropGroup>, ImportResult) {
    let mut project = Project::default();
    let none = |_: &str| false;
    let r = import(&mut project, bytes, &ImportOptions { allow_losses: true, ..ImportOptions::new(&none) }).unwrap();
    let g = project.comps().flat_map(|(_, c)| c.layers.iter()).find_map(|l| {
        l.props.sub("effects").and_then(|fx| {
            fx.children
                .iter()
                .filter_map(|n| n.as_group())
                .find(|g| g.match_id == effect || matches!(&g.kind, GroupKind::Effect { effect: e } if e == effect))
                .cloned()
        })
    });
    (g, r)
}

/// The losses of `category` that name the effect `match_name`.
pub fn losses(r: &ImportResult, category: LossCategory, match_name: &str) -> Vec<String> {
    let tag = format!("({match_name})");
    r.report.losses.iter().filter(|l| l.category == category).flat_map(|l| l.details.clone()).filter(|d| d.contains(&tag)).collect()
}

/// The value of property `id` of `g` (panics when it is missing).
pub fn v(g: &PropGroup, id: &str) -> Value {
    g.prop(id).unwrap_or_else(|| panic!("no {id} in {}", g.match_id)).value.clone()
}

/// The scalar value of property `id` of `g`.
pub fn num(g: &PropGroup, id: &str) -> f64 {
    match v(g, id) {
        Value::Scalar(x) => x,
        other => panic!("{id}: {other:?}"),
    }
}

/// Asserts that property `id` of `g` is `want` (relative 1e-6).
pub fn near(g: &PropGroup, id: &str, want: f64) {
    let got = num(g, id);
    assert!((got - want).abs() <= want.abs().max(1.0) * 1e-6, "{} {id}: {got} != {want}", g.match_id);
}

/// Asserts that the point property `id` of `g` is `want` (within 0.01).
pub fn pt(g: &PropGroup, id: &str, want: [f64; 2]) {
    let Value::Vec2(a) = v(g, id) else { panic!("{id}") };
    assert!((a[0] - want[0]).abs() < 1e-2 && (a[1] - want[1]).abs() < 1e-2, "{} {id}: {a:?} != {want:?}", g.match_id);
}

/// A slider definition.
pub fn slider(kind: ParamKind, name: &'static str, default: f64, valid: (f64, f64)) -> Def {
    Def { kind, name, default: Default::Number(default), valid, items: &[] }
}

/// A popup definition: default item (1-based) and items.
pub fn popup(name: &'static str, default: u32, items: &'static [&'static str]) -> Def {
    Def { kind: ParamKind::Popup, name, default: Default::Index(default), valid: (0.0, 0.0), items }
}

/// A definition of another kind (angle, point, colour, ...) with `default`.
pub fn other(kind: ParamKind, name: &'static str, default: Default) -> Def {
    Def { kind, name, default, valid: (0.0, 0.0), items: &[] }
}

/// The definitions written here read back as the reader decodes After Effects' `pard`.
#[test]
fn pard_layout_decodes_as_written() {
    // Echo's Decay (float, valid 0..100, default 1), Linear Wipe's Feather (fixed, valid max
    // 32 000), Echo's Number Of Echoes (integer, max 30 000, default 1), Offset's centre point,
    // Color Stabilizer's Stabilize (default 2 of 3): what the sample dump of 2026-11-05 read.
    let fx = Fx {
        match_name: "Test FX",
        name: "Test FX",
        defs: vec![
            slider(ParamKind::FloatSlider, "Decay", 1.0, (0.0, 100.0)),
            slider(ParamKind::FixedSlider, "Feather", 0.0, (0.0, 32000.0)),
            slider(ParamKind::IntSlider, "Number Of Echoes", 1.0, (0.0, 30000.0)),
            other(ParamKind::Point, "Shift Center To", Default::Point(0.5, 0.5)),
            popup("Stabilize", 2, &["Brightness", "Levels", "Curves"]),
            other(ParamKind::Color, "Shadows Unbalance", Default::Color([0, 0, 0, 0])),
        ],
        values: vec![(1, 20.0)],
        data: vec![],
        keys: vec![],
    };
    let bytes = bytes_with(&fx);
    let file = parse(&bytes).unwrap();
    let proj = AepProject::new(&file).unwrap();
    let defs = proj.effect_definition_index();
    let found = proj
        .all_items()
        .into_iter()
        .filter_map(|i| i.comp())
        .flat_map(|c| c.layers().collect::<Vec<_>>())
        .flat_map(|l| l.effects())
        .find(|e| e.match_name() == "Test FX")
        .expect("the effect");
    let p = defs.params(&found);
    let by = |i: usize| p.iter().find(|q| q.index == i).expect("parameter").clone();
    let decay = by(1);
    assert_eq!(
        (decay.kind, decay.name.as_str(), decay.valid_min(), decay.valid_max(), decay.pard_default()),
        (ParamKind::FloatSlider, "Decay", Some(0.0), Some(100.0), Some(ParamValue::Number(1.0)))
    );
    assert_eq!(decay.stored_value(), Some(ParamValue::Number(20.0)));
    assert_eq!((by(2).kind, by(2).valid_max(), by(2).pard_default()), (ParamKind::FixedSlider, Some(32000.0), Some(ParamValue::Number(0.0))));
    assert_eq!((by(3).kind, by(3).valid_max(), by(3).pard_default()), (ParamKind::IntSlider, Some(30000.0), Some(ParamValue::Number(1.0))));
    assert_eq!(by(4).pard_default(), Some(ParamValue::Point([0.5, 0.5])));
    let stab = by(5);
    assert_eq!((stab.popup_item_count(), stab.items.clone()), (Some(3), Some(vec!["Brightness".to_string(), "Levels".into(), "Curves".into()])));
    assert_eq!(found.display_name().as_deref(), Some("Test FX"));
    assert_eq!(by(6).pard_default(), Some(ParamValue::Argb([0.0, 0.0, 0.0, 0.0])));
}
