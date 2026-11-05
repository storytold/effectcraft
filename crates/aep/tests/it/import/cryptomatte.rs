//! The Cryptomatte plug-in (match name `Cryptomatte`) arrives as EffectCraft's Cryptomatte
//! (`ec.3d.cryptomatte`): the layer and selection from its Settings data, Output as Display and
//! Matte Only. The effect is built into the committed `synthetic.aep` in place of its Glow, in
//! the layout docs/aep-format.md 3.7 describes (made-up layer names and ID hashes).

use effectcraft_aep::import::{ImportOptions, ImportResult, LossCategory, import};
use effectcraft_aep::{Body, Chunk, parse, write};
use effectcraft_keyframe::Value;
use effectcraft_project::{Project, PropGroup};

const SYNTHETIC: &[u8] = include_bytes!("../../fixtures/synthetic.aep");

/// A match name chunk (`tdmn`, 40 bytes).
fn tdmn(name: &str) -> Chunk<'static> {
    let mut b = name.as_bytes().to_vec();
    b.resize(40, 0);
    Chunk::data(*b"tdmn", b)
}

/// A parameter definition: the type at byte 15, the name at 16; a popup's default at 56 and
/// item count at 60.
fn pard(kind: u8, name: &str, popup: Option<(u32, u16)>) -> Chunk<'static> {
    let mut b = vec![0u8; 148];
    b[15] = kind;
    b[16..16 + name.len()].copy_from_slice(name.as_bytes());
    if let Some((default, count)) = popup {
        b[56..60].copy_from_slice(&default.to_be_bytes());
        b[60..62].copy_from_slice(&count.to_be_bytes());
    }
    Chunk::data(*b"pard", b)
}

fn utf8(s: &str) -> Chunk<'static> {
    Chunk::data(*b"Utf8", s.as_bytes().to_vec())
}

/// A static value record holding `v`, after the one After Effects wrote for Glow's A & B Midpoint
/// (its header; without the expression).
fn record(template: &Chunk<'_>, v: f64) -> Chunk<'static> {
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

/// The plug-in's Settings data: `cry1`, 28 bytes not read, the layer name in 64 bytes, a `u32` 1,
/// a `u32` not read, the text's length with its NUL, a `u32` not read, a zero byte, the text.
fn settings(layer: &str, selection: &str) -> Vec<u8> {
    let mut d = b"cry1".to_vec();
    d.resize(32, 0x5a);
    d.extend(layer.as_bytes());
    d.resize(96, 0);
    d.extend(1u32.to_le_bytes());
    d.extend(0x1234_5678u32.to_le_bytes());
    d.extend(u32::try_from(selection.len() + 1).unwrap().to_le_bytes());
    d.extend(0x9abc_def0u32.to_le_bytes());
    d.push(0);
    d.extend(selection.as_bytes());
    d.push(0);
    d.extend(b" <1"); // left-overs of a longer earlier text
    d
}

struct Crypto {
    /// The Settings data, if stored.
    settings: Option<Vec<u8>>,
    /// Output (1-based), if stored.
    output: Option<u32>,
    /// Selection Mode, if stored.
    selection_mode: Option<bool>,
}

/// The Cryptomatte instance: its own definitions (input layer, Settings, Output, Selection Mode,
/// the built-in entry) and the stored values of `c`.
fn sspc(c: &Crypto, template: &Chunk<'_>) -> Chunk<'static> {
    let name = |i: u32| format!("Cryptomatte-{i:04}");
    let part = vec![
        Chunk::data(*b"parn", 5u32.to_be_bytes().to_vec()),
        tdmn(&name(0)),
        pard(0, "", None),
        tdmn(&name(1)),
        pard(11, "Settings", None),
        Chunk::data(*b"aRbp", settings("", "")),
        tdmn(&name(2)),
        pard(7, "Output", Some((1, 4))),
        Chunk::list(*b"pdnm", vec![utf8("Colors|Matted Colors|Matted RGBA|Matte Only")]),
        tdmn(&name(3)),
        pard(4, "Selection Mode", None),
        tdmn("ADBE Effect Built In Params"),
        pard(9, "", None),
    ];
    let mut values = vec![Chunk::data(*b"tdsb", 1u32.to_be_bytes().to_vec())];
    if let Some(d) = &c.settings {
        values.push(tdmn(&name(1)));
        values.push(record(template, 0.0));
        values.push(Chunk::list(*b"aRbs", vec![Chunk::data(*b"aRbp", d.clone())]));
    }
    if let Some(o) = c.output {
        values.push(tdmn(&name(2)));
        values.push(record(template, f64::from(o)));
    }
    if let Some(on) = c.selection_mode {
        values.push(tdmn(&name(3)));
        values.push(record(template, if on { 1.0 } else { 0.0 }));
    }
    Chunk::list(*b"sspc", vec![Chunk::list(*b"fnam", vec![utf8("Cryptomatte")]), Chunk::list(*b"parT", part), Chunk::list(*b"tdgp", values)])
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

/// Puts `c` in place of the Glow effect (the instance in the effect parade and the project's
/// shared definitions); the number of places.
fn replace_glow(chunks: &mut [Chunk<'_>], c: &Crypto, template: &Chunk<'_>) -> usize {
    let mut n = 0;
    for ch in chunks {
        let kids = ch.children_mut();
        if let Some(at) = kids.iter().position(|k| k.is(b"tdmn") && k.bytes().is_some_and(|b| b.starts_with(b"ADBE Glo2\0")))
            && kids.get(at + 1).is_some_and(|k| k.is_list(b"sspc"))
        {
            kids[at] = tdmn("Cryptomatte");
            kids[at + 1] = sspc(c, template);
            n += 1;
        }
        n += replace_glow(kids, c, template);
    }
    n
}

fn run(c: &Crypto) -> (Option<PropGroup>, ImportResult) {
    let file = parse(SYNTHETIC).unwrap();
    let template = find_glow_template(&file.chunks).expect("Glow's A & B Midpoint record");
    let mut file = parse(SYNTHETIC).unwrap();
    assert_eq!(replace_glow(&mut file.chunks, c, &template), 2, "the Glow effect and its shared definitions");
    let bytes = write(&file).unwrap();
    let mut project = Project::default();
    let none = |_: &str| false;
    let r = import(&mut project, &bytes, &ImportOptions { allow_losses: true, ..ImportOptions::new(&none) }).unwrap();
    let g = project.comps().flat_map(|(_, c)| c.layers.iter()).find_map(|l| {
        l.props.sub("effects").and_then(|fx| fx.children.iter().filter_map(|n| n.as_group()).find(|g| g.match_id == "ec.3d.cryptomatte").cloned())
    });
    (g, r)
}

fn losses(r: &ImportResult, category: LossCategory) -> Vec<String> {
    r.report.losses.iter().filter(|l| l.category == category).flat_map(|l| l.details.clone()).filter(|d| d.contains("Cryptomatte")).collect()
}

fn v(g: &PropGroup, id: &str) -> Value {
    g.prop(id).unwrap_or_else(|| panic!("no {id}")).value.clone()
}

fn str(s: &str) -> Value {
    Value::Str(s.into())
}

#[test]
fn cryptomatte_arrives_with_its_layer_selection_and_output() {
    let sel = "<0000002a>, <deadbeef>";
    let (g, r) = run(&Crypto { settings: Some(settings("Layer_A", sel)), output: Some(3), selection_mode: None });
    assert_eq!(losses(&r, LossCategory::Effects), Vec::<String>::new());
    assert_eq!(losses(&r, LossCategory::DamagedData), Vec::<String>::new());
    let g = g.expect("an EffectCraft Cryptomatte");
    // Matted RGBA: the layer keyed by the selection.
    assert_eq!((v(&g, "layerName"), v(&g, "selection"), v(&g, "display"), v(&g, "matteOnly")), (str("Layer_A"), str(sel), Value::Enum(1), Value::Bool(false)));
    // The selection's `<hex>` tokens are EffectCraft's own grammar.
    assert_eq!(effectcraft_effects::selection_hashes(sel, None), vec![0x2a, 0xdead_beef]);
}

#[test]
fn cryptomatte_output_maps_to_display_and_matte_only() {
    for (output, display, matte_only) in [(1, 0, false), (3, 1, false), (4, 1, true)] {
        let (g, r) = run(&Crypto { settings: Some(settings("L", "<00000001>")), output: Some(output), selection_mode: Some(false) });
        assert_eq!(losses(&r, LossCategory::Effects), Vec::<String>::new(), "output {output}");
        let g = g.expect("effect");
        assert_eq!((v(&g, "display"), v(&g, "matteOnly")), (Value::Enum(display), Value::Bool(matte_only)), "output {output}");
    }
    // No Output record: the instance's own definition has the default (Colors here).
    let (g, r) = run(&Crypto { settings: None, output: None, selection_mode: None });
    assert_eq!(losses(&r, LossCategory::Effects), Vec::<String>::new());
    let g = g.expect("effect");
    assert_eq!((v(&g, "display"), v(&g, "layerName"), v(&g, "selection")), (Value::Enum(0), str(""), str("")), "the default settings: no layer, no selection");
}

#[test]
fn cryptomatte_values_without_a_counterpart_are_reported() {
    // Matted Colors has no counterpart: reported, EffectCraft's display kept.
    let (g, r) = run(&Crypto { settings: Some(settings("L", "")), output: Some(2), selection_mode: Some(true) });
    let found = losses(&r, LossCategory::Effects);
    assert!(found.iter().any(|d| d.contains("'Matted Colors'") && d.contains("no counterpart")), "{found:?}");
    assert!(found.iter().any(|d| d.contains("Selection Mode") && d.contains("is on")), "{found:?}");
    assert_eq!(found.len(), 2, "{found:?}");
    let g = g.expect("still imported");
    assert_eq!((v(&g, "display"), v(&g, "matteOnly"), v(&g, "layerName")), (Value::Enum(2), Value::Bool(false), str("L")));

    // Unreadable Settings data: reported as damaged, no layer and no selection.
    let mut bad = settings("L", "<00000001>");
    bad[0] = b'x';
    let (g, r) = run(&Crypto { settings: Some(bad), output: Some(3), selection_mode: None });
    let found = losses(&r, LossCategory::DamagedData);
    assert!(found.iter().any(|d| d.contains("settings (layer, selection) are unreadable")), "{found:?}");
    assert_eq!((v(&g.expect("effect"), "layerName")), str(""));
}
