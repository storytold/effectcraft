//! #3087: Photoshop layers, effects without a counterpart (third-party and pseudo effects) and an
//! expression on Orientation, each built into the committed `synthetic.aep` (no external files;
//! the values are made up, the layouts are docs/aep-format.md's).

use super::fx_inject::{Default, Fx, losses, other, popup, run, slider};
use effectcraft_aep::import::{ImportOptions, ImportResult, LossCategory, import};
use effectcraft_aep::{Body, Chunk, ParamKind, parse, write};
use effectcraft_keyframe::Value;
use effectcraft_project::{GroupKind, ItemKind, Project, SourceLayer};

const SYNTHETIC: &[u8] = include_bytes!("../../fixtures/synthetic.aep");

fn visit(chunks: &mut [Chunk<'_>], f: &mut dyn FnMut(&mut Chunk<'_>)) {
    for c in chunks {
        f(c);
        visit(c.children_mut(), f);
    }
}

/// `synthetic.aep` with its Glow (the instance and the project's definitions) renamed to an effect
/// without a counterpart: a placeholder, reported as a loss.
pub fn with_unknown_glow() -> Vec<u8> {
    let from = [b"ADBE Glo2".as_slice(), &[0]].concat();
    let to = [b"XYZQ Glo2".as_slice(), &[0]].concat();
    let mut bytes = SYNTHETIC.to_vec();
    let mut i = 0;
    while let Some(at) = bytes.get(i..).and_then(|b| b.windows(from.len()).position(|w| w == from.as_slice())) {
        bytes[i + at..i + at + to.len()].copy_from_slice(&to);
        i += at + to.len();
    }
    assert!(i > 0, "the synthetic project has a Glow");
    bytes
}

fn import_all(bytes: &[u8]) -> (Project, ImportResult) {
    let mut project = Project::default();
    let none = |_: &str| false;
    let r = import(&mut project, bytes, &ImportOptions { allow_losses: true, ..ImportOptions::new(&none) }).unwrap();
    (project, r)
}

// ---- Photoshop layers ----

/// The `opti` of a footage item showing layer `index` (-1: the merged image) of a 64 x 48
/// Photoshop document, with the layer's bounds (top, left, bottom, right) and name.
fn psd_opti(index: i32, bounds: [i32; 4], name: &str) -> Vec<u8> {
    let mut b = vec![0u8; 602];
    b[..4].copy_from_slice(b"8BPS");
    b[4..6].copy_from_slice(&[1, 9]);
    b[8..10].copy_from_slice(&602u16.to_be_bytes());
    b[12..14].copy_from_slice(&[1, 1]);
    b[14..18].copy_from_slice(&index.to_be_bytes());
    b[18..22].copy_from_slice(b"SPB8");
    b[30..32].copy_from_slice(&4u16.to_le_bytes());
    b[32..36].copy_from_slice(&48u32.to_le_bytes());
    b[36..40].copy_from_slice(&64u32.to_le_bytes());
    for (i, v) in bounds.iter().enumerate() {
        b[78 + 4 * i..82 + 4 * i].copy_from_slice(&v.to_le_bytes());
    }
    b[344..344 + name.len()].copy_from_slice(name.as_bytes());
    b
}

/// `synthetic.aep` with its image sequence item turned into a footage item of `/art/poster.psd`
/// (`opti` from [`psd_opti`], the item `w` x `h` pixels).
fn with_psd_item(opti: Vec<u8>, w: u32, h: u32) -> Vec<u8> {
    let mut file = parse(SYNTHETIC).unwrap();
    let mut patched = 0;
    visit(&mut file.chunks, &mut |c| {
        if !c.is_list(b"Pin ") || !c.children().iter().any(|k| k.is(b"opti") && k.bytes().is_some_and(|d| d.starts_with(b"png!"))) {
            return;
        }
        for k in c.children_mut() {
            let id = k.id;
            match &id {
                b"opti" => *k = Chunk::data(*b"opti", opti.clone()),
                b"sspc" => {
                    if let Body::Data(d) = &mut k.body {
                        let d = d.to_mut();
                        d[22..26].copy_from_slice(b"8BPS");
                        d[30..34].copy_from_slice(&w.to_be_bytes());
                        d[34..38].copy_from_slice(&h.to_be_bytes());
                    }
                }
                _ if k.is_list(b"Als2") => {
                    *k = Chunk::list(*b"Als2", vec![Chunk::data(*b"alas", br#"{"fullpath":"/art/poster.psd","target_is_folder":false}"#.to_vec())]);
                }
                _ => {}
            }
        }
        patched += 1;
    });
    assert_eq!(patched, 1, "the synthetic project has one file item");
    write(&file).unwrap()
}

/// The imported footage item: its name and footage.
fn the_footage(p: &Project) -> (String, effectcraft_project::Footage) {
    p.items.values().find_map(|i| if let ItemKind::Footage(f) = &i.kind { Some((i.name.clone(), f.clone())) } else { None }).expect("the footage item")
}

/// A footage item showing one Photoshop layer shows that layer (it showed the whole document) and
/// is named `<layer>/<file>` (it was named after the file).
#[test]
fn a_photoshop_layer_item_shows_its_layer() {
    // Retain Layer Sizes: the item is as big as the layer's bounds.
    let (p, _) = import_all(&with_psd_item(psd_opti(2, [10, 20, 40, 120], "Title"), 100, 30));
    let (name, f) = the_footage(&p);
    assert_eq!(name, "Title/poster.psd");
    assert_eq!(f.path, "/art/poster.psd");
    assert_eq!(f.layer, Some(SourceLayer { index: 2, name: "Title".into(), layer_size: true, adjustment_white: true, ..SourceLayer::default() }));
    // Imported at the document size: the layer on the whole document.
    let (_, f) = the_footage(&import_all(&with_psd_item(psd_opti(2, [10, 20, 40, 120], "Title"), 64, 48)).0);
    assert_eq!(f.layer.map(|l| (l.index, l.layer_size)), Some((2, false)));
    // An adjustment layer has no bounds: the document size.
    let (name, f) = the_footage(&import_all(&with_psd_item(psd_opti(0, [0; 4], "Levels 1"), 64, 48)).0);
    assert_eq!((name.as_str(), f.layer.map(|l| (l.index, l.layer_size))), ("Levels 1/poster.psd", Some((0, false))));
}

/// A layer name is UTF-8 (a name in another encoding would come out replaced, not dropped).
#[test]
fn a_photoshop_layer_name_is_utf8() {
    let (name, f) = the_footage(&import_all(&with_psd_item(psd_opti(1, [0, 0, 48, 64], "Ébauche ✓"), 64, 48)).0);
    assert_eq!((name.as_str(), f.layer.map(|l| l.name)), ("Ébauche ✓/poster.psd", Some("Ébauche ✓".to_string())));
}

/// The merged image (index -1) stays the whole document, named after the file.
#[test]
fn a_merged_photoshop_item_shows_the_document() {
    let (p, _) = import_all(&with_psd_item(psd_opti(-1, [0; 4], ""), 64, 48));
    let (name, f) = the_footage(&p);
    assert_eq!((name.as_str(), f.layer), ("poster.psd", None));
}

// ---- effects without a counterpart ----

/// The comp and the layer holding the injected effect, and another layer of the comp.
fn layers_of(p: &Project, effect: &str) -> (effectcraft_project::ItemId, usize, effectcraft_project::LayerId) {
    for (cid, c) in p.comps() {
        let Some(at) = c.layers.iter().position(|l| {
            l.props
                .sub("effects")
                .is_some_and(|fx| fx.children.iter().any(|n| n.as_group().is_some_and(|g| g.kind == GroupKind::Effect { effect: effect.into() })))
        }) else {
            continue;
        };
        let other = c.layers.iter().enumerate().find(|(i, _)| *i != at).map(|(_, l)| l.id).expect("another layer");
        return (*cid, at + 1, other);
    }
    panic!("no layer has {effect}")
}

/// What `text` (an expression on another layer's Opacity) evaluates to.
fn evaluate(p: &Project, effect: &str, text: &str) -> Value {
    let (comp, _, other) = layers_of(p, effect);
    effectcraft_expr::eval_standalone(p, comp, other, "transform/opacity", text, 0.0).unwrap()
}

fn third_party() -> Fx {
    Fx {
        match_name: "Vendor Thing",
        name: "Thing",
        defs: vec![
            slider(ParamKind::FloatSlider, "Amount", 10.0, (0.0, 100.0)),
            popup("Mode", 1, &["Low", "Mid", "High"]),
            slider(ParamKind::Checkbox, "Enabled", 0.0, (0.0, 0.0)),
            other(ParamKind::Point, "Center", Default::Point(0.5, 0.25)),
            slider(ParamKind::FloatSlider, "Untouched", 7.0, (0.0, 100.0)),
        ],
        values: vec![(1, 42.0), (2, 3.0), (3, 1.0)],
        data: vec![],
        keys: vec![],
    }
}

/// An effect without a counterpart is kept as a placeholder with its parameters (it was
/// dropped), named and switched as in the file, and still reported (it does not render).
#[test]
fn an_effect_without_a_counterpart_is_kept_as_a_placeholder() {
    let fx = third_party();
    let (g, r) = run(&fx, "Vendor Thing");
    let g = g.expect("the placeholder");
    assert_eq!((g.name.as_str(), &g.kind), ("Thing", &GroupKind::Effect { effect: "Vendor Thing".into() }));
    let by = |name: &str| g.children.iter().filter_map(|n| n.as_prop()).find(|p| p.name == name).unwrap_or_else(|| panic!("{name}")).value.clone();
    assert_eq!(by("Amount"), Value::Scalar(42.0));
    assert_eq!(by("Mode"), Value::Enum(2));
    assert_eq!(by("Enabled"), Value::Bool(true));
    // A point is in layer pixels (the 8 x 8 sequence), a parameter without a record its default.
    assert_eq!(by("Center"), Value::Vec2([4.0, 2.0]));
    assert_eq!(by("Untouched"), Value::Scalar(7.0));
    let lost = losses(&r, LossCategory::Effects, "Vendor Thing");
    assert_eq!(lost.len(), 1, "{lost:?}");
    assert!(lost[0].contains("has the effect 'Thing' (Vendor Thing), kept as a placeholder"), "{}", lost[0]);
}

/// The placeholder's parameters read in expressions as After Effects reads them (a popup's item
/// 1-based), so expressions that use the effect keep working.
#[test]
fn expressions_read_a_placeholder() {
    let fx = third_party();
    let mut project = Project::default();
    let none = |_: &str| false;
    import(&mut project, &super::fx_inject::bytes_with(&fx), &ImportOptions { allow_losses: true, ..ImportOptions::new(&none) }).unwrap();
    let (_, index, _) = layers_of(&project, "Vendor Thing");
    let read = |param: &str| evaluate(&project, "Vendor Thing", &format!("thisComp.layer({index}).effect(\"Thing\")(\"{param}\")"));
    assert_eq!(read("Amount"), Value::Scalar(42.0));
    assert_eq!(read("Mode"), Value::Scalar(3.0));
}

/// A pseudo effect (expression controls only) arrives with its controls and is no loss: it renders
/// nothing in After Effects either.
#[test]
fn a_pseudo_effect_arrives_without_a_loss() {
    let fx = Fx {
        match_name: "Pseudo/0.5",
        name: "Settings",
        defs: vec![popup("Menu", 1, &["One", "Two", "Three"]), slider(ParamKind::FloatSlider, "Size", 0.0, (0.0, 100.0))],
        values: vec![(1, 2.0), (2, 12.5)],
        data: vec![],
        keys: vec![],
    };
    let (g, r) = run(&fx, "Pseudo_0.5");
    let g = g.expect("the placeholder");
    assert_eq!(g.kind, GroupKind::Effect { effect: "Pseudo/0.5".into() });
    assert!(losses(&r, LossCategory::Effects, "Pseudo/0.5").is_empty(), "{:?}", r.report.losses);
    let mut project = Project::default();
    let none = |_: &str| false;
    import(&mut project, &super::fx_inject::bytes_with(&fx), &ImportOptions { allow_losses: true, ..ImportOptions::new(&none) }).unwrap();
    let (_, index, _) = layers_of(&project, "Pseudo/0.5");
    assert_eq!(evaluate(&project, "Pseudo/0.5", &format!("thisComp.layer({index}).effect(\"Settings\")(\"Menu\")")), Value::Scalar(2.0));
    assert_eq!(evaluate(&project, "Pseudo/0.5", &format!("thisComp.layer({index}).effect(\"Settings\")(\"Size\")")), Value::Scalar(12.5));
}

/// A Dropdown Menu Control (`Pseudo/@@…`, one popup) becomes EffectCraft's with the file's items;
/// a separator stays an item (After Effects counts it), so expressions read After Effects' number.
#[test]
fn a_dropdown_menu_control_becomes_effectcrafts() {
    let fx = Fx {
        match_name: "Pseudo/@@aGFzaA",
        name: "Color Choice",
        defs: vec![popup("Menu", 1, &["Red", "(-", "Blue"])],
        values: vec![(1, 3.0)],
        data: vec![],
        keys: vec![],
    };
    let (g, r) = run(&fx, "ec.control.dropdown");
    let g = g.expect("EffectCraft's Dropdown Menu Control");
    assert_eq!(g.name, "Color Choice");
    let menu = g.prop("menu").expect("menu");
    assert_eq!(menu.value, Value::Enum(2));
    assert_eq!(menu.ui, effectcraft_project::ParamUi::Popup { options: vec!["Red".into(), "—".into(), "Blue".into()] });
    assert!(losses(&r, LossCategory::Effects, "Pseudo/@@aGFzaA").is_empty(), "{:?}", r.report.losses);
}

/// A slider range that is not finite or is reversed is not taken (the controls would clamp with
/// a panic and the project would save `null` and not load): the parameter is a plain number, and
/// the project survives a save and a load.
#[test]
fn hostile_slider_ranges_are_not_taken() {
    let fx = Fx {
        match_name: "Vendor Ranges",
        name: "Ranges",
        defs: vec![
            slider(ParamKind::FloatSlider, "Not A Number", 1.0, (f64::NAN, 100.0)),
            slider(ParamKind::FloatSlider, "Reversed", 1.0, (100.0, 0.0)),
            slider(ParamKind::FloatSlider, "Fine", 1.0, (0.0, 100.0)),
        ],
        values: vec![],
        data: vec![],
        keys: vec![],
    };
    let mut project = Project::default();
    let none = |_: &str| false;
    import(&mut project, &super::fx_inject::bytes_with(&fx), &ImportOptions { allow_losses: true, ..ImportOptions::new(&none) }).unwrap();
    let g = project
        .comps()
        .flat_map(|(_, c)| c.layers.iter())
        .find_map(|l| l.props.sub("effects").and_then(|fx| fx.children.iter().filter_map(|n| n.as_group()).find(|g| g.match_id == "Vendor Ranges").cloned()))
        .expect("the placeholder");
    let ui = |name: &str| g.children.iter().filter_map(|n| n.as_prop()).find(|p| p.name == name).unwrap_or_else(|| panic!("{name}")).ui.clone();
    assert_eq!(ui("Not A Number"), effectcraft_project::ParamUi::Number);
    assert_eq!(ui("Reversed"), effectcraft_project::ParamUi::Number);
    assert!(matches!(ui("Fine"), effectcraft_project::ParamUi::Slider { .. }));
    let saved = serde_json::to_string(&project).unwrap();
    let back: Project = serde_json::from_str(&saved).expect("the saved project loads");
    assert_eq!(back, project);
}

/// A hostile placeholder (thousands of popups claiming 65 535 items with no item names) stays
/// bounded: at most 1000 parameters per effect and 1024 items per popup, the rest reported.
#[test]
fn a_hostile_placeholder_stays_bounded() {
    let fx = Fx { match_name: "Vendor Huge", name: "Huge", defs: vec![popup("P", 1, &[]); 3000], values: vec![], data: vec![], keys: vec![] };
    let mut bytes = super::fx_inject::bytes_with(&fx);
    // Every popup claims 65 535 items, and no item names are stored.
    let mut file = parse(&bytes).unwrap();
    visit(&mut file.chunks, &mut |c| {
        if c.is_list(b"parT") {
            let Body::List { children, .. } = &mut c.body else { return };
            children.retain(|k| !k.is(b"pdnm"));
            for k in children.iter_mut() {
                if let Body::Data(d) = &mut k.body
                    && k.id == *b"pard"
                    && d.get(15) == Some(&7)
                {
                    d.to_mut()[60..62].copy_from_slice(&u16::MAX.to_be_bytes());
                }
            }
        }
    });
    bytes = write(&file).unwrap();
    let (p, r) = import_all(&bytes);
    let g = p
        .comps()
        .flat_map(|(_, c)| c.layers.iter())
        .find_map(|l| l.props.sub("effects").and_then(|fx| fx.children.iter().filter_map(|n| n.as_group()).find(|g| g.match_id == "Vendor Huge").cloned()))
        .expect("the placeholder");
    assert!(g.children.len() <= 1000, "{}", g.children.len());
    let items: usize = g
        .children
        .iter()
        .filter_map(|n| n.as_prop())
        .map(|p| if let effectcraft_project::ParamUi::Popup { options } = &p.ui { options.len() } else { 0 })
        .inspect(|n| assert!(*n <= 1024))
        .sum();
    assert!(items <= 200_000 + 1024, "{items}");
    let lost = losses(&r, LossCategory::Effects, "Vendor Huge");
    assert!(lost.iter().any(|d| d.contains("not kept")), "{lost:?}");
}

// ---- Orientation ----

/// `synthetic.aep` with every Orientation set to `[10, 20, 30]` and the expression `text`.
fn with_orientation_expression(text: &str) -> Vec<u8> {
    let mut file = parse(SYNTHETIC).unwrap();
    let mut patched = 0;
    visit(&mut file.chunks, &mut |c| {
        if !c.is_list(b"otst") {
            return;
        }
        for k in c.children_mut() {
            if k.is_list(b"tdbs") {
                let kids = k.children_mut();
                for r in kids.iter_mut() {
                    if let Body::Data(d) = &mut r.body {
                        let d = d.to_mut();
                        match &r.id {
                            b"tdsb" => d.copy_from_slice(&1u32.to_be_bytes()),
                            b"tdb4" => {
                                d[119] = 0;
                                d[120] = 1;
                            }
                            _ => {}
                        }
                    }
                }
                let Body::List { children, .. } = &mut k.body else { continue };
                children.push(Chunk::data(*b"Utf8", text.as_bytes().to_vec()));
            } else if k.is_list(b"otky") {
                let mut otda = Vec::new();
                for v in [10.0f64, 20.0, 30.0] {
                    otda.extend(v.to_be_bytes());
                }
                *k = Chunk::list(*b"otky", vec![Chunk::data(*b"otda", otda)]);
            }
        }
        patched += 1;
    });
    assert!(patched > 0, "the synthetic project has an Orientation");
    write(&file).unwrap()
}

/// An expression on Orientation arrives with the value (both were dropped and reported "not
/// decoded").
#[test]
fn an_expression_on_orientation_is_imported() {
    let (p, r) = import_all(&with_orientation_expression("[1, 2, 3]"));
    let mut seen = 0;
    for (_, c) in p.comps() {
        for l in &c.layers {
            let Some(o) = l.props.prop("transform/orientation") else { continue };
            assert_eq!(o.value, Value::Vec3([10.0, 20.0, 30.0]), "{}", l.name);
            assert_eq!(o.expr.as_ref().map(|e| (e.text.as_str(), e.enabled)), Some(("[1, 2, 3]", true)), "{}", l.name);
            seen += 1;
        }
    }
    assert!(seen > 0);
    let orientation: Vec<String> = r
        .report
        .losses
        .iter()
        .filter(|l| l.category == LossCategory::TransformProperties)
        .flat_map(|l| l.details.clone())
        .filter(|d| d.contains("Orientation"))
        .collect();
    assert!(orientation.is_empty(), "{orientation:?}");
}
