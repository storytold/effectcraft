//! The Cryptomatte plug-in (match name `Cryptomatte`) arrives as EffectCraft's Cryptomatte
//! (`ec.3d.cryptomatte`): the layer and selection from its Settings data, Output as Display and
//! Matte Only. The effect is built into the committed `synthetic.aep` in place of its Glow, in
//! the layout docs/aep-format.md 3.7 describes (made-up layer names and ID hashes).

use effectcraft_aep::import::{ImportOptions, ImportResult, LossCategory, import};
use effectcraft_aep::{Body, Chunk, ParamKind, Project as AepProject, parse, write};
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

/// A chunk whose body is chunks without being a `LIST`, as After Effects writes `pdnm` and `fnam`
/// (`NESTED_IDS`). A `LIST pdnm` is not read as popup items.
fn nested(id: [u8; 4], children: Vec<Chunk<'static>>) -> Chunk<'static> {
    Chunk { id, body: Body::Chunks(children), pad: None }
}

/// The plug-in's Output items in the order After Effects 2026 stores them (1400).
const OUTPUT: &[&str] = &["Colors", "Matted Colors", "Matted RGBA", "Matte Only"];

/// A static value record holding `v` in state `tdsb` (1 written, 3 the default state), after the
/// one After Effects wrote for Glow's A & B Midpoint (its header; without the expression).
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
    /// Selection Mode, if stored: on, and the record's `tdsb`.
    selection_mode: Option<(bool, u32)>,
    /// The instance has no definitions of its own (an empty `parT`) and borrows the project's,
    /// as every instance but the first of a project does (1400).
    borrowed: bool,
    /// Output's popup items as written in its `pdnm`; `None` writes no `pdnm` (a definition with
    /// four items then falls back to the plug-in's own list).
    items: Option<&'static [&'static str]>,
}

/// The Cryptomatte instance: its own definitions (input layer, Settings, Output, Selection Mode,
/// the built-in entry; none when `own` is false) and the stored values of `c`.
fn sspc(c: &Crypto, template: &Chunk<'_>, own: bool) -> Chunk<'static> {
    let name = |i: u32| format!("Cryptomatte-{i:04}");
    let mut part = vec![
        Chunk::data(*b"parn", 5u32.to_be_bytes().to_vec()),
        tdmn(&name(0)),
        pard(0, "", None),
        tdmn(&name(1)),
        pard(11, "Settings", None),
        Chunk::data(*b"aRbp", settings("", "")),
        tdmn(&name(2)),
        pard(7, "Output", Some((1, c.items.map_or(4, |i| u16::try_from(i.len()).unwrap())))),
        tdmn(&name(3)),
        pard(4, "Selection Mode", None),
        tdmn("ADBE Effect Built In Params"),
        pard(9, "", None),
    ];
    if let Some(items) = c.items {
        // After the Output definition, as After Effects writes it.
        part.insert(8, nested(*b"pdnm", vec![utf8(&items.join("|"))]));
    }
    let part = if own { part } else { Vec::new() };
    let mut values = vec![Chunk::data(*b"tdsb", 1u32.to_be_bytes().to_vec())];
    if let Some(d) = &c.settings {
        values.push(tdmn(&name(1)));
        values.push(record(template, 1, 0.0));
        values.push(Chunk::list(*b"aRbs", vec![Chunk::data(*b"aRbp", d.clone())]));
    }
    if let Some(o) = c.output {
        values.push(tdmn(&name(2)));
        values.push(record(template, 1, f64::from(o)));
    }
    if let Some((on, tdsb)) = c.selection_mode {
        values.push(tdmn(&name(3)));
        values.push(record(template, tdsb, if on { 1.0 } else { 0.0 }));
    }
    Chunk::list(*b"sspc", vec![nested(*b"fnam", vec![utf8("Cryptomatte")]), Chunk::list(*b"parT", part), Chunk::list(*b"tdgp", values)])
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
/// shared definitions, `defs` below `EfDf`); the number of places.
fn replace_glow(chunks: &mut [Chunk<'_>], c: &Crypto, template: &Chunk<'_>, defs: bool) -> usize {
    let mut n = 0;
    for ch in chunks {
        let defs = defs || ch.is_list(b"EfDf");
        let kids = ch.children_mut();
        if let Some(at) = kids.iter().position(|k| k.is(b"tdmn") && k.bytes().is_some_and(|b| b.starts_with(b"ADBE Glo2\0")))
            && kids.get(at + 1).is_some_and(|k| k.is_list(b"sspc"))
        {
            kids[at] = tdmn("Cryptomatte");
            kids[at + 1] = sspc(c, template, defs || !c.borrowed);
            n += 1;
        }
        n += replace_glow(kids, c, template, defs);
    }
    n
}

/// `synthetic.aep` with `c` in place of the Glow effect.
fn bytes_with(c: &Crypto) -> Vec<u8> {
    let file = parse(SYNTHETIC).unwrap();
    let template = find_glow_template(&file.chunks).expect("Glow's A & B Midpoint record");
    let mut file = parse(SYNTHETIC).unwrap();
    assert_eq!(replace_glow(&mut file.chunks, c, &template, false), 2, "the Glow effect and its shared definitions");
    write(&file).unwrap()
}

fn run(c: &Crypto) -> (Option<PropGroup>, ImportResult) {
    let bytes = bytes_with(c);
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
    let (g, r) = run(&Crypto { settings: Some(settings("Layer_A", sel)), output: Some(3), selection_mode: None, borrowed: false, items: Some(OUTPUT) });
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
        let (g, r) = run(&Crypto {
            settings: Some(settings("L", "<00000001>")),
            output: Some(output),
            selection_mode: Some((false, 1)),
            borrowed: false,
            items: Some(OUTPUT),
        });
        assert_eq!(losses(&r, LossCategory::Effects), Vec::<String>::new(), "output {output}");
        let g = g.expect("effect");
        assert_eq!((v(&g, "display"), v(&g, "matteOnly")), (Value::Enum(display), Value::Bool(matte_only)), "output {output}");
    }
    // No Output record: the default, Colors, from the instance's own definition or, borrowed,
    // the plug-in's (1400 `crypto-00-defaults`; every other composition there borrows).
    for borrowed in [false, true] {
        let (g, r) = run(&Crypto { settings: None, output: None, selection_mode: None, borrowed, items: Some(OUTPUT) });
        assert_eq!(losses(&r, LossCategory::Effects), Vec::<String>::new(), "borrowed {borrowed}");
        let g = g.expect("effect");
        assert_eq!(
            (v(&g, "display"), v(&g, "matteOnly"), v(&g, "layerName"), v(&g, "selection")),
            (Value::Enum(0), Value::Bool(false), str(""), str("")),
            "the default settings: Colors, no layer, no selection (borrowed {borrowed})"
        );
    }
    // A borrowed definition with a stored Output.
    let (g, r) = run(&Crypto { settings: None, output: Some(4), selection_mode: None, borrowed: true, items: Some(OUTPUT) });
    assert_eq!(losses(&r, LossCategory::Effects), Vec::<String>::new());
    let g = g.expect("effect");
    assert_eq!((v(&g, "display"), v(&g, "matteOnly")), (Value::Enum(1), Value::Bool(true)));
}

/// The harness writes Output's items as After Effects does: the reader finds them in the `pdnm`
/// (and the instance name in `fnam`), so the Output tests go through the item names.
#[test]
fn the_injected_definition_reads_back_with_its_popup_items() {
    let bytes = bytes_with(&Crypto { settings: None, output: Some(4), selection_mode: None, borrowed: false, items: Some(OUTPUT) });
    let file = parse(&bytes).unwrap();
    let proj = AepProject::new(&file).unwrap();
    let defs = proj.effect_definition_index();
    let fx = proj
        .all_items()
        .into_iter()
        .filter_map(|i| i.comp())
        .flat_map(|c| c.layers().collect::<Vec<_>>())
        .flat_map(|l| l.effects())
        .find(|e| e.match_name() == "Cryptomatte")
        .expect("the effect");
    let output = defs.params(&fx).into_iter().find(|p| p.index == 2).expect("Output");
    let want: Vec<String> = OUTPUT.iter().map(|s| s.to_string()).collect();
    assert_eq!((output.kind, output.popup_item_count(), output.items), (ParamKind::Popup, Some(4), Some(want)));
    assert_eq!(fx.name().as_deref(), Some("Cryptomatte"));
}

/// Output is mapped by the item the stored index names, not by its position in the plug-in's
/// list: with the items in another order, index 4 is Colors and index 1 Matte Only.
#[test]
fn cryptomatte_output_maps_by_item_name() {
    const REORDERED: &[&str] = &["Matte Only", "Matted RGBA", "Matted Colors", "Colors"];
    for (output, display, matte_only) in [(1, 1, true), (2, 1, false), (4, 0, false)] {
        let (g, r) = run(&Crypto { settings: None, output: Some(output), selection_mode: None, borrowed: false, items: Some(REORDERED) });
        assert_eq!(losses(&r, LossCategory::Effects), Vec::<String>::new(), "output {output}");
        let g = g.expect("effect");
        assert_eq!((v(&g, "display"), v(&g, "matteOnly")), (Value::Enum(display), Value::Bool(matte_only)), "output {output}");
    }
    // Item names are matched without regard to case and spacing.
    let (g, _) = run(&Crypto {
        settings: None,
        output: Some(2),
        selection_mode: None,
        borrowed: false,
        items: Some(&["colors", "MATTE ONLY", "Matted RGBA", "Matted Colors"]),
    });
    assert_eq!((v(&g.expect("effect"), "matteOnly")), Value::Bool(true));
    // An item EffectCraft does not know: reported, Colors used.
    let (g, r) = run(&Crypto {
        settings: None,
        output: Some(2),
        selection_mode: None,
        borrowed: false,
        items: Some(&["Colors", "Something New", "Matted RGBA", "Matte Only"]),
    });
    let found = losses(&r, LossCategory::Effects);
    assert!(found.iter().any(|d| d.contains("'Something New'") && d.contains("Colors used")), "{found:?}");
    assert_eq!((v(&g.expect("effect"), "display")), Value::Enum(0));
}

/// A file without Output's `pdnm` and four items in the definition: the plug-in's own list.
#[test]
fn cryptomatte_output_without_items_falls_back_to_the_plugin_list() {
    let (g, r) = run(&Crypto { settings: None, output: Some(4), selection_mode: None, borrowed: false, items: None });
    assert_eq!(losses(&r, LossCategory::Effects), Vec::<String>::new());
    assert_eq!((v(&g.expect("effect"), "matteOnly")), Value::Bool(true));
}

#[test]
fn cryptomatte_values_without_a_counterpart_are_reported() {
    // Matted Colors has no counterpart: reported, imported as Matte (the same matte).
    let (g, r) = run(&Crypto { settings: Some(settings("L", "")), output: Some(2), selection_mode: Some((true, 1)), borrowed: false, items: Some(OUTPUT) });
    let found = losses(&r, LossCategory::Effects);
    assert!(found.iter().any(|d| d.contains("'Matted Colors'") && d.contains("no counterpart") && d.contains("imported as Matte")), "{found:?}");
    assert!(found.iter().any(|d| d.contains("Selection Mode") && d.contains("is on")), "{found:?}");
    assert_eq!(found.len(), 2, "{found:?}");
    let g = g.expect("still imported");
    assert_eq!((v(&g, "display"), v(&g, "matteOnly"), v(&g, "layerName")), (Value::Enum(1), Value::Bool(false), str("L")));

    // Selection Mode on as After Effects writes it: a default-state record (`tdsb` 3) holding 1
    // (1400 `crypto-01-set-all`). Reported like a written one; off in that form is not.
    for (on, n) in [(true, 1), (false, 0)] {
        let (_, r) = run(&Crypto { settings: None, output: None, selection_mode: Some((on, 3)), borrowed: true, items: Some(OUTPUT) });
        let found = losses(&r, LossCategory::Effects);
        assert_eq!(found.iter().filter(|d| d.contains("Selection Mode") && d.contains("is on")).count(), n, "on {on}: {found:?}");
        assert_eq!(found.len(), n, "on {on}: {found:?}");
    }

    // Unreadable Settings data: reported as damaged, no layer and no selection.
    let mut bad = settings("L", "<00000001>");
    bad[0] = b'x';
    let (g, r) = run(&Crypto { settings: Some(bad), output: Some(3), selection_mode: None, borrowed: false, items: Some(OUTPUT) });
    let found = losses(&r, LossCategory::DamagedData);
    assert!(found.iter().any(|d| d.contains("settings (layer, selection) are unreadable")), "{found:?}");
    assert_eq!((v(&g.expect("effect"), "layerName")), str(""));
}

/// `bytes` with `f` applied to the children of every value group (`tdgp`) that holds the
/// parameter `match_name`, given the position of its match name.
fn edit_values(bytes: &[u8], match_name: &str, f: &dyn Fn(&mut Vec<Chunk<'static>>, usize)) -> Vec<u8> {
    fn walk(chunks: &mut [Chunk<'_>], match_name: &str, f: &dyn Fn(&mut Vec<Chunk<'static>>, usize)) {
        for c in chunks {
            if c.is_list(b"tdgp") {
                let mut kids: Vec<Chunk<'static>> = c.children().iter().cloned().map(Chunk::into_owned).collect();
                if let Some(at) = kids.iter().position(|k| k.is(b"tdmn") && k.bytes().is_some_and(|b| b.starts_with(format!("{match_name} ").as_bytes()))) {
                    f(&mut kids, at);
                    *c = Chunk::list(*b"tdgp", kids);
                    continue;
                }
            }
            walk(c.children_mut(), match_name, f);
        }
    }
    let mut file = parse(bytes).unwrap();
    walk(&mut file.chunks, match_name, f);
    write(&file).unwrap()
}

fn import_bytes(bytes: &[u8]) -> ImportResult {
    let mut project = Project::default();
    let none = |_: &str| false;
    import(&mut project, bytes, &ImportOptions { allow_losses: true, ..ImportOptions::new(&none) }).unwrap()
}

/// A stored record that has lost its data is reported as damaged, not read as the default.
#[test]
fn cryptomatte_records_without_their_data_are_reported() {
    let c = Crypto { settings: Some(settings("L", "<00000001>")), output: None, selection_mode: Some((true, 3)), borrowed: false, items: Some(OUTPUT) };
    let bytes = bytes_with(&c);
    // The Settings record without its `aRbs` list.
    let no_data = edit_values(&bytes, "Cryptomatte-0001", &|kids, at| {
        if kids.get(at + 2).is_some_and(|k| k.is_list(b"aRbs")) {
            kids.remove(at + 2);
        }
    });
    let found = losses(&import_bytes(&no_data), LossCategory::DamagedData);
    assert!(found.len() == 1 && found[0].contains("settings (layer, selection) have no data"), "{found:?}");
    // The Selection Mode record without a readable value: damaged, not the `pard` default.
    let unreadable = edit_values(&bytes, "Cryptomatte-0003", &|kids, at| {
        if let Some(rec) = kids.get_mut(at + 1) {
            let inner: Vec<Chunk<'static>> = rec.children().iter().filter(|k| !k.is(b"cdat")).cloned().map(Chunk::into_owned).collect();
            *rec = Chunk::list(*b"tdbs", inner);
        }
    });
    let r = import_bytes(&unreadable);
    let found = losses(&r, LossCategory::DamagedData);
    assert!(found.len() == 1 && found[0].contains("Selection Mode") && found[0].contains("unreadable value, off used"), "{found:?}");
    assert!(losses(&r, LossCategory::Effects).iter().all(|d| !d.contains("is on")));
    // Both intact: nothing damaged.
    assert_eq!(losses(&import_bytes(&bytes), LossCategory::DamagedData), Vec::<String>::new());
}

/// Our own After Effects 2026 sample of the plug-in (2.6.6), corpus `aep/cryptomatte.aep`: one
/// Cryptomatte on an EXR layer per composition. The first instance has its own definitions, the
/// others borrow the project's. The first six hold the fresh Settings data (no layer, no
/// selection); the last two come from presets that chose a layer and picked one object
/// (`box_red` on CryptoObject) or one material (`mat_paint` on CryptoMaterial) in the plug-in's
/// UI. The EXR's manifest names those hashes, and After Effects draws exactly those objects as
/// selected.
#[test]
#[ignore = "needs aep/cryptomatte.aep in the corpus (EFFECTCRAFT_AEP_FIXTURES); not pinned yet"]
fn the_after_effects_sample_arrives_as_after_effects_shows_it() {
    let bytes = fixture!("cryptomatte.aep");
    let mut project = Project::default();
    let none = |_: &str| false;
    let r = import(&mut project, &bytes, &ImportOptions { allow_losses: true, ..ImportOptions::new(&none) }).unwrap();
    assert_eq!(losses(&r, LossCategory::DamagedData), Vec::<String>::new());
    let found = losses(&r, LossCategory::Effects);
    // (composition, Display, Matte Only, layer, selection, the loss it reports): the readbacks
    // are Output 1 (fresh), 1, 1, 2, 3, 4, 1, 1 and Selection Mode 0, 1, 1, 0, 0, 0, 0, 0.
    let want = [
        ("crypto-00-defaults", 0, false, "", "", None),
        ("crypto-01-set-all", 0, false, "", "", Some("Selection Mode")),
        ("crypto-02-chk-toggled", 0, false, "", "", Some("Selection Mode")),
        ("crypto-03-pop2-2", 1, false, "", "", Some("'Matted Colors'")),
        ("crypto-04-pop2-3", 1, false, "", "", None),
        ("crypto-05-pop2-4", 1, true, "", "", None),
        ("crypto-06-preset-object-box", 0, false, "CryptoObject", "<11a97486>", None),
        ("crypto-07-preset-material-paint", 0, false, "CryptoMaterial", "<a5deeb9e>", None),
    ];
    for (comp, display, matte_only, layer, selection, loss) in want {
        let c = project.find_by_name(comp).and_then(|i| i.as_comp()).unwrap_or_else(|| panic!("no {comp}"));
        let g = c.layers.iter().find_map(|l| {
            l.props.sub("effects").and_then(|fx| fx.children.iter().filter_map(|n| n.as_group()).find(|g| g.match_id == "ec.3d.cryptomatte").cloned())
        });
        let g = g.unwrap_or_else(|| panic!("{comp}: no Cryptomatte"));
        assert_eq!(
            (v(&g, "display"), v(&g, "matteOnly"), v(&g, "layerName"), v(&g, "selection")),
            (Value::Enum(display), Value::Bool(matte_only), str(layer), str(selection)),
            "{comp}"
        );
        let mine: Vec<_> = found.iter().filter(|d| d.starts_with(&format!("{comp}:"))).collect();
        match loss {
            None => assert!(mine.is_empty(), "{comp}: {mine:?}"),
            Some(what) => assert!(mine.len() == 1 && mine[0].contains(what), "{comp}: {mine:?}"),
        }
    }
    assert_eq!(found.len(), 3, "{found:?}");
    // The picked tokens are the manifest's hashes, which are the hashes of the names: EffectCraft
    // reads `<11a97486>` and `box_red` as the same object.
    for (name, token) in [("box_red", "<11a97486>"), ("mat_paint", "<a5deeb9e>")] {
        assert_eq!(effectcraft_effects::selection_hashes(token, None), effectcraft_effects::selection_hashes(name, None), "{name}");
    }
    assert_eq!(effectcraft_effects::selection_hashes("<11a97486>", None), vec![0x11a9_7486]);
}
