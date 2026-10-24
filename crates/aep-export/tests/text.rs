//! Text layers: every text fixture is imported, exported and parsed back with `effectcraft-aep`;
//! the text document (text, paragraphs, style runs, fonts, box) must equal the one After Effects
//! wrote for every decoded field, and a second import gives the same Source Text. Documents built
//! in code check line breaks, runs, fonts, and what is reported.

mod common;

use common::*;
use effectcraft_aep::{Caps, Justification, TextDocument, parse};
use effectcraft_aep_export::LossCategory;
use effectcraft_aep_import::{ImportOptions, import};
use effectcraft_keyframe::Value;
use effectcraft_keyframe::text_doc::{BaselineOption, CharStyle, Justify, ParaStyle, StyleRun, TextDoc};
use effectcraft_project::{ItemId, ItemKind, LayerSource, Project};
use effectcraft_time::FrameRate;

fn fixture(name: &str) -> Vec<u8> {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../aep/tests/fixtures");
    std::fs::read(dir.join(name)).expect(name)
}

fn text_fixtures() -> Vec<String> {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../aep/tests/fixtures");
    let mut v: Vec<String> = std::fs::read_dir(&dir)
        .expect("fixtures")
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|n| n.ends_with(".aep") && n.contains("-text-"))
        .collect();
    v.sort();
    v
}

fn reimport(bytes: &[u8]) -> Project {
    let mut p = Project::default();
    let none = |_: &str| false;
    let opts = ImportOptions { allow_losses: true, ..ImportOptions::new(&none) };
    import(&mut p, bytes, &opts).expect("import");
    p
}

/// The text documents of all text layers, by composition name and layer index.
fn documents(bytes: &[u8]) -> Vec<((String, usize), TextDocument)> {
    let file = parse(bytes).expect("parse");
    let view = effectcraft_aep::Project::new(&file).expect("project");
    let mut out = Vec::new();
    for item in view.all_items() {
        let Some(comp) = item.comp() else { continue };
        for (i, layer) in comp.layers().enumerate() {
            if let Some(d) = layer.text_document() {
                out.push(((item.display_name().unwrap_or_default(), i), d));
            }
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-4
}

/// The decoded fields of two documents are equal (`font_of` resolves the font indices).
fn same_doc(a: &TextDocument, b: &TextDocument, w: &str) {
    // U+0003 is imported as a paragraph break and written as one (386: the same layout).
    assert_eq!(a.raw_text.replace('\u{3}', "\r"), b.raw_text, "{w}");
    let paras = |d: &TextDocument| -> Vec<(usize, Justification)> { d.paragraphs.iter().map(|p| (p.length, p.justification)).collect() };
    assert_eq!(paras(a), paras(b), "{w}");
    assert_eq!(a.style_runs.len(), b.style_runs.len(), "{w}: style runs");
    for (x, y) in a.style_runs.iter().zip(&b.style_runs) {
        assert_eq!(x.length, y.length, "{w}");
        assert_eq!(a.font_of(x), b.font_of(y), "{w}");
        assert!(close(x.font_size, y.font_size), "{w}: size {} {}", x.font_size, y.font_size);
        assert!((1..4).all(|i| close(x.fill_argb[i], y.fill_argb[i])), "{w}: fill {:?} {:?}", x.fill_argb, y.fill_argb);
        assert!(close(x.tracking, y.tracking), "{w}: tracking");
        assert_eq!((x.faux_bold, x.faux_italic, x.caps, x.auto_leading), (y.faux_bold, y.faux_italic, y.caps, y.auto_leading), "{w}");
        if !x.auto_leading {
            assert_eq!(x.leading.map(|l| (l * 1e4).round()), y.leading.map(|l| (l * 1e4).round()), "{w}: leading");
        }
    }
    let bx = |d: &TextDocument| d.text_box.map(|t| [t.x, t.y, t.width, t.height]);
    match (bx(a), bx(b)) {
        (None, None) => {}
        (Some(p), Some(q)) => assert!(p.iter().zip(q).all(|(u, v)| close(*u, v)), "{w}: box {p:?} {q:?}"),
        other => panic!("{w}: box {other:?}"),
    }
}

fn source_text(p: &Project) -> Vec<TextDoc> {
    let mut v = Vec::new();
    for item in p.items.values() {
        let ItemKind::Comp(c) = &item.kind else { continue };
        for l in &c.layers {
            if let Some(Value::Text(d)) = l.props.prop("text/sourceText").map(|s| &s.value) {
                v.push((**d).clone());
            }
        }
    }
    v.sort_by(|a, b| a.text.cmp(&b.text));
    v
}

#[test]
fn every_text_fixture_is_written_with_its_document() {
    let names = text_fixtures();
    assert!(names.len() >= 15, "{names:?}");
    for name in &names {
        let bytes = fixture(name);
        let p1 = reimport(&bytes);
        let r = export_all(&p1);
        // Text animators and vertical text are imported but not written: the only text losses
        // allowed.
        let animators = r
            .report
            .losses
            .iter()
            .filter(|l| l.category == LossCategory::TextLayers)
            .flat_map(|l| &l.details)
            .filter(|d| d.contains("text animator") || d.contains("vertical text"))
            .count();
        assert_eq!(r.report.count(LossCategory::TextLayers), animators, "{name}: {:?}", r.report.losses);
        let (a, b) = (documents(&bytes), documents(&r.bytes));
        assert_eq!(a.len(), b.len(), "{name}");
        for ((ka, da), (kb, db)) in a.iter().zip(&b) {
            assert_eq!(ka.0, kb.0, "{name}");
            same_doc(da, db, name);
        }
        // The container is consistent and a second import gives the same Source Text.
        let file = parse(&r.bytes).expect("parse");
        assert_eq!(effectcraft_aep::write(&file).expect("write"), r.bytes, "{name}");
        let p2 = reimport(&r.bytes);
        // Vertical text comes back horizontal (reported above).
        let horizontal = |v: Vec<TextDoc>| v.into_iter().map(|d| TextDoc { vertical: false, ..d }).collect::<Vec<_>>();
        assert_eq!(horizontal(source_text(&p1)), horizontal(source_text(&p2)), "{name}");
    }
}

fn style(font: &str, size: f64) -> CharStyle {
    CharStyle { font: font.into(), style: "Regular".into(), size, leading: Some(size * 1.2), ..CharStyle::default() }
}

fn text_project(docs: Vec<TextDoc>) -> (Project, ItemId) {
    let mut p = Project::default();
    let c = add_comp(&mut p, "C", 1920, 1080, FrameRate::new(25, 1), 5.0, None);
    for (i, d) in docs.into_iter().enumerate() {
        add_layer(&mut p, c, &format!("T{i}"), LayerSource::Text, (1920, 1080));
        with_layer(&mut p, c, 0, |l| {
            if let Some(st) = l.props.prop_mut("text/sourceText") {
                st.value = Value::Text(Box::new(d));
            }
        });
    }
    (p, c)
}

#[test]
fn documents_built_in_code_are_written_as_their_fields() {
    let mut runs = TextDoc::plain("Red\r\nBlue\nX");
    runs.set_runs(vec![
        StyleRun { len: 5, style: CharStyle { fill: [1.0, 0.0, 0.0, 1.0], tracking: 25.0, ..style("ArialMT", 50.0) } },
        StyleRun { len: 6, style: CharStyle { fill: [0.0, 0.0, 1.0, 1.0], faux_bold: true, small_caps: true, leading: None, ..style("Georgia", 70.0) } },
    ]);
    runs.set_paras(vec![
        ParaStyle { justify: Justify::Center, ..runs.base_para() },
        ParaStyle { justify: Justify::Right, ..runs.base_para() },
        ParaStyle { justify: Justify::Left, ..runs.base_para() },
    ]);
    let mut boxed = TextDoc::plain("In a box");
    boxed.set_runs(vec![StyleRun { len: 8, style: style("TimesNewRomanPSMT", 30.0) }]);
    boxed.box_size = Some([300.0, 120.0]);
    boxed.box_pos = [-150.0, -60.0];
    let empty = TextDoc::plain("");
    let (p, _) = text_project(vec![runs, boxed, empty]);
    let r = export_all(&p);
    assert!(r.report.is_lossless(), "{:?}", r.report.losses);
    assert_eq!(r.report.exported.text_layers, 3);
    let docs = documents(&r.bytes);
    let by_text = |t: &str| docs.iter().map(|(_, d)| d).find(|d| d.raw_text == t).unwrap_or_else(|| panic!("{t}"));
    // CRLF and LF are After Effects' CR; the final paragraph ends with one too.
    let d = by_text("Red\rBlue\rX\r");
    assert_eq!(
        d.paragraphs.iter().map(|p| (p.length, p.justification)).collect::<Vec<_>>(),
        vec![(4, Justification::Center), (5, Justification::Right), (2, Justification::Left)]
    );
    // Runs in After Effects' characters: "Red\r" + 'B' of the first style (the CRLF counted
    // once), then "lue\rX\r".
    let runs: Vec<_> = d.style_runs.iter().map(|s| (s.length, d.font_of(s).map(str::to_string), s.font_size, s.faux_bold, s.caps, s.auto_leading)).collect();
    assert_eq!(runs, vec![(4, Some("ArialMT".into()), 50.0, false, Caps::Normal, false), (7, Some("Georgia".into()), 70.0, true, Caps::Small, true)]);
    assert!(close(d.style_runs[0].tracking, 25.0) && close(d.style_runs[0].fill_argb[1], 1.0) && close(d.style_runs[1].fill_argb[3], 1.0));
    // The prototype's own fonts keep their places; new ones are appended.
    assert_eq!(d.fonts.first().map(String::as_str), Some("TimesNewRomanPSMT"));
    let b = by_text("In a box\r");
    let tb = b.text_box.expect("box");
    assert_eq!([tb.x, tb.y, tb.width, tb.height], [-150.0, -60.0, 300.0, 120.0]);
    let e = by_text("\r");
    assert_eq!(e.style_runs.iter().map(|s| s.length).sum::<usize>(), 1);
    // A text layer named after its text stores no name.
    let (p2, _) = text_project(vec![TextDoc::plain("Named")]);
    let mut p2 = p2;
    let c = p2.items.values().find(|i| i.name == "C").map(|i| i.id).expect("C");
    with_layer(&mut p2, c, 0, |l| l.name = "Named".into());
    let r2 = export_all(&p2);
    let file = parse(&r2.bytes).expect("parse");
    let view = effectcraft_aep::Project::new(&file).expect("project");
    let layer = view.all_items().into_iter().find_map(|i| i.comp()).and_then(|c| c.layers().next()).expect("layer");
    assert_eq!(layer.name(), None);
    assert_eq!(layer.header().map(|h| h.layer_type), Some(effectcraft_aep::LayerType::Text));
}

#[test]
fn what_a_text_document_cannot_hold_is_reported() {
    let mut d = TextDoc::plain("x");
    d.set_runs(vec![StyleRun { len: 1, style: CharStyle { apply_stroke: true, stroke_width: 3.0, h_scale: 80.0, ..style("ArialMT", 40.0) } }]);
    d.set_paras(vec![ParaStyle { justify: Justify::JustifyAll, indent_left: 10.0, ..d.base_para() }]);
    d.vertical = true;
    let (mut p, c) = text_project(vec![d]);
    with_layer(&mut p, c, 0, |l| {
        if let Some(st) = l.props.prop_mut("text/sourceText") {
            st.keys = vec![key(0.0, st.value.clone()), key(1.0, Value::Text(Box::new(TextDoc::plain("y"))))];
        }
    });
    let r = export_all(&p);
    // Vertical text and Source Text keys (stroke, scale, justification and indents are written).
    assert_eq!(r.report.count(LossCategory::TextLayers), 2, "{:?}", r.report.losses);
    // A 3D text layer is written; only per-character 3D is reported.
    let (mut p3, c3) = text_project(vec![TextDoc::plain("3D")]);
    with_layer(&mut p3, c3, 0, |l| l.switches.three_d = true);
    assert_eq!(export_all(&p3).report.count(LossCategory::ThreeD), 0);
    with_layer(&mut p3, c3, 0, |l| {
        if let Some(pc) = l.props.prop_mut("text/perChar3d") {
            pc.value = Value::Bool(true);
        }
    });
    assert_eq!(export_all(&p3).report.count(LossCategory::ThreeD), 1);
    assert_eq!(r.report.exported.text_layers, 1);
}

#[test]
fn hostile_text_never_panics() {
    let long: String = "ab\r".repeat(50_000);
    let mut nan = TextDoc::plain("nan");
    nan.set_runs(vec![StyleRun {
        len: 3,
        style: CharStyle { size: f64::NAN, tracking: f64::INFINITY, leading: Some(f64::NAN), fill: [f32::NAN, 2.0, -1.0, 1.0], ..style("", 10.0) },
    }]);
    let mut odd = TextDoc::plain("()\\ \u{3} \u{1F600} ünï");
    odd.box_size = Some([f64::NAN, 1e300]);
    odd.box_pos = [f64::INFINITY, -1e300];
    // Runs that do not cover the text, or cover more.
    let mut short = TextDoc::plain("short text");
    short.runs = vec![StyleRun { len: 2, style: style("ArialMT", 20.0) }];
    let mut over = TextDoc::plain("ab");
    over.runs = vec![StyleRun { len: 1000, style: style("ArialMT", 20.0) }, StyleRun { len: usize::MAX, style: style("ArialMT", 20.0) }];
    let mut max = TextDoc::plain("max");
    max.runs = vec![StyleRun { len: usize::MAX, style: style("ArialMT", 20.0) }];
    max.paragraphs = vec![effectcraft_keyframe::text_doc::ParaStyle::default(); 3];
    let (p, _) = text_project(vec![TextDoc::plain(&long), nan, odd, short, over, max]);
    let r = export_all(&p);
    let file = parse(&r.bytes).expect("parse");
    assert_eq!(effectcraft_aep::write(&file).expect("write"), r.bytes);
    let _ = reimport(&r.bytes);
}

#[test]
fn the_baseline_option_is_written_into_style_slot_13() {
    // 526-528: slot /13 is 1 superscript (After Effects' scripted default, also in the prototypes),
    // 0 normal, 2 subscript; normal text written as 1 is drawn by After Effects at 58.3 % size.
    let doc = |b: BaselineOption| {
        let mut d = TextDoc::plain("HHHH");
        d.set_runs(vec![StyleRun { len: 4, style: CharStyle { baseline: b, ..style("ArialMT", 100.0) } }]);
        d
    };
    for (b, slot_differs) in [(BaselineOption::Normal, true), (BaselineOption::Superscript, false), (BaselineOption::Subscript, true)] {
        let (p, _) = text_project(vec![doc(b)]);
        let r = export_all(&p);
        assert!(r.report.is_lossless(), "{b:?}: {:?}", r.report.losses);
        let docs = documents(&r.bytes);
        let d = &docs.first().expect("doc").1;
        // The reader decodes /13 now; the scripted default (also in the prototypes) is 1.
        assert!(d.undecoded_style_fields.iter().all(|f| f != "13"), "{b:?}: {:?}", d.undecoded_style_fields);
        let _ = slot_differs;
        // A second import gives the same baseline option back (normal stays normal).
        let back = source_text(&reimport(&r.bytes));
        let got = back.first().and_then(|t| t.runs().first().map(|r| r.style.baseline));
        assert_eq!(got, Some(b), "{b:?}");
        let text = String::from_utf8_lossy(&r.bytes).to_string();
        let want = match b {
            BaselineOption::Normal => "/13 0",
            BaselineOption::Superscript => "/13 1",
            BaselineOption::Subscript => "/13 2",
        };
        assert!(text.contains(want), "{b:?}");
    }
}

#[test]
fn superscript_stays_superscript_and_normal_stays_normal() {
    // 526 and 113 are superscript (the scripted default), 527 normal, 528 subscript: the merged
    // importer reads /13 and the exporter writes it back, so a round trip keeps each one.
    for (name, want) in [
        ("113-text-Hello-size60-fillorange.aep", BaselineOption::Superscript),
        ("526-text-point-HHHH-default.aep", BaselineOption::Superscript),
        ("527-text-point-HHHH-baseline-normal.aep", BaselineOption::Normal),
        ("528-text-point-HHHH-baseline-subscript.aep", BaselineOption::Subscript),
    ] {
        let p1 = reimport(&fixture(name));
        let baselines = |p: &Project| source_text(p).iter().flat_map(|d| d.runs().into_iter().map(|r| r.style.baseline)).collect::<Vec<_>>();
        let b1 = baselines(&p1);
        assert!(!b1.is_empty() && b1.iter().all(|b| *b == want), "{name}: {b1:?}");
        let r = export_all(&p1);
        let p2 = reimport(&r.bytes);
        assert_eq!(baselines(&p2), b1, "{name}");
        let docs = documents(&r.bytes);
        let stored = match want {
            BaselineOption::Normal => effectcraft_aep::FontBaseline::Normal,
            BaselineOption::Superscript => effectcraft_aep::FontBaseline::Superscript,
            BaselineOption::Subscript => effectcraft_aep::FontBaseline::Subscript,
        };
        assert!(docs.iter().flat_map(|(_, d)| &d.style_runs).all(|s| s.baseline == stored), "{name}");
    }
}

#[test]
fn a_text_made_in_effectcraft_is_written_with_the_every_line_composer() {
    // A plain box text without paragraph entries uses the document's settings: EffectCraft's
    // default composer is every-line, which After Effects reads back as such (t6).
    let mut d = TextDoc::plain("One two three four five six seven eight\rTwo");
    d.set_runs(vec![StyleRun { len: d.text.chars().count(), style: style("ArialMT", 30.0) }]);
    d.box_size = Some([300.0, 200.0]);
    assert!(d.paragraphs.is_empty());
    let (p, _) = text_project(vec![d.clone()]);
    let r = export_all(&p);
    assert!(r.report.is_lossless(), "{:?}", r.report.losses);
    let docs = documents(&r.bytes);
    let paras = &docs[0].1.paragraphs;
    assert_eq!(paras.len(), 2);
    assert!(paras.iter().all(|p| p.every_line_composer && p.direction == 0 && p.justification == Justification::Left), "{paras:?}");
    assert_eq!(source_text(&reimport(&r.bytes))[0].para(0).composer, effectcraft_keyframe::text_doc::Composer::EveryLine);
}

#[test]
fn right_to_left_justification_is_stored_relative_to_the_direction() {
    // t7: in a right-to-left paragraph After Effects lays code 0 and 3 out flush right and 1 and
    // 4 flush left; 2, 5, 6 are the same in both directions.
    use effectcraft_keyframe::text_doc::Direction;
    let cases = [
        (Justify::Left, Justification::Right),
        (Justify::Right, Justification::Left),
        (Justify::Center, Justification::Center),
        (Justify::JustifyLastLeft, Justification::FullRight),
        (Justify::JustifyLastRight, Justification::FullLeft),
        (Justify::JustifyLastCenter, Justification::FullCenter),
        (Justify::JustifyAll, Justification::FullAll),
    ];
    for (rtl, ltr_stored) in [(true, false), (false, true)] {
        let docs: Vec<TextDoc> = cases
            .iter()
            .enumerate()
            .map(|(i, (j, _))| {
                let mut d = TextDoc::plain(&format!("T{i}"));
                d.set_runs(vec![StyleRun { len: 2, style: style("ArialMT", 30.0) }]);
                let dir = if rtl { Direction::Rtl } else { Direction::Ltr };
                d.set_paras(vec![ParaStyle { justify: *j, direction: dir, ..d.base_para() }]);
                d
            })
            .collect();
        let (p, _) = text_project(docs);
        let r = export_all(&p);
        let written = documents(&r.bytes);
        let back = source_text(&reimport(&r.bytes));
        for (i, (j, swapped)) in cases.iter().enumerate() {
            let stored = written.iter().find(|(_, d)| d.raw_text == format!("T{i}\r")).map(|(_, d)| d.paragraphs[0].justification).unwrap();
            let want = if ltr_stored { effectcraft_aep_justification(*j) } else { *swapped };
            assert_eq!(stored, want, "rtl {rtl}: {j:?}");
            assert_eq!(back[i].para(0).justify, *j, "rtl {rtl}: round trip");
        }
    }
}

fn effectcraft_aep_justification(j: Justify) -> Justification {
    match j {
        Justify::Left => Justification::Left,
        Justify::Right => Justification::Right,
        Justify::Center => Justification::Center,
        Justify::JustifyLastLeft => Justification::FullLeft,
        Justify::JustifyLastRight => Justification::FullRight,
        Justify::JustifyLastCenter => Justification::FullCenter,
        Justify::JustifyAll => Justification::FullAll,
    }
}

/// Every `(path of match names, record)` below `ADBE Text Animators`, keyed records with the
/// unused ease before the first / after the last key and spatial arc lengths zeroed, and the
/// sequence of match names (the structure).
fn animator_records(bytes: &[u8]) -> (Vec<String>, Vec<(String, Vec<u8>)>) {
    fn walk(cs: &[effectcraft_aep::Chunk<'_>], path: &str, inside: bool, names: &mut Vec<String>, out: &mut Vec<(String, Vec<u8>)>) {
        for (i, c) in cs.iter().enumerate() {
            let Some(name) = c.bytes().filter(|_| c.is(b"tdmn")).map(|b| String::from_utf8_lossy(b.split(|x| *x == 0).next().unwrap_or(b"")).to_string())
            else {
                // Items, layers and other lists: look inside (a node after a `tdmn` is walked
                // with it).
                let after_tdmn = i > 0 && cs.get(i - 1).is_some_and(|p| p.is(b"tdmn"));
                if !inside && !after_tdmn {
                    walk(c.children(), path, inside, names, out);
                }
                continue;
            };
            let here = inside || name == "ADBE Text Animators";
            let key = format!("{path}/{name}");
            if here {
                names.push(key.clone());
            }
            let Some(node) = cs.get(i + 1) else { continue };
            if here && node.is_list(b"tdbs") {
                let mut f = effectcraft_aep::AepFile { chunks: vec![node.clone()], trailer: Default::default() };
                if let Some(list) = f.chunks[0].children_mut().iter_mut().find(|k| k.is_list(b"list")) {
                    let size = list
                        .child(b"lhd3")
                        .and_then(|c| c.bytes())
                        .and_then(|b| b.get(16..20))
                        .map_or(0, |b| u32::from_be_bytes(b.try_into().unwrap()) as usize);
                    if let Some(ldat) = list.children_mut().iter_mut().find(|k| k.is(b"ldat"))
                        && let effectcraft_aep::Body::Data(d) = &mut ldat.body
                    {
                        let mut v = d.to_vec();
                        let n = v.len();
                        if size == 48 && n >= 96 {
                            v[16..32].fill(0);
                            v[n - 16..n].fill(0);
                        }
                        if size >= 104 && size % 8 == 0 && (size - 56) % 24 == 0 {
                            for r in v.chunks_mut(size) {
                                r[16..24].fill(0);
                            }
                        }
                        *d = v.into();
                    }
                }
                out.push((key.clone(), effectcraft_aep::write(&f).unwrap()));
            }
            if node.is_list(b"tdgp") {
                walk(node.children(), &key, here, names, out);
            }
        }
    }
    let f = parse(bytes).expect("parse");
    let (mut names, mut out) = (Vec::new(), Vec::new());
    walk(&f.chunks, "", false, &mut names, &mut out);
    (names, out)
}

#[test]
fn text_animators_are_written_as_after_effects_wrote_them() {
    // 114, 579-594, 728: the animators, their properties and range selectors come back with
    // After Effects' structure and records.
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../aep/tests/fixtures");
    let mut names: Vec<String> = std::fs::read_dir(&dir)
        .expect("fixtures")
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|n| n.ends_with(".aep") && (n.contains("textanim") || n.contains("text-animator")))
        .collect();
    names.sort();
    assert!(names.len() >= 16, "{names:?}");
    let mut compared = 0;
    for name in &names {
        let bytes = fixture(name);
        let r = export_all(&reimport(&bytes));
        assert_eq!(r.report.count(LossCategory::TextLayers), 0, "{name}: {:?}", r.report.losses);
        let (mut na, mut ra) = animator_records(&bytes);
        let (nb, rb) = animator_records(&r.bytes);
        // With Units = Index After Effects keeps the percent range it had before (583); the
        // index range is what it uses, the export has only that.
        if na.iter().any(|n| n.ends_with("ADBE Text Index Start")) {
            na.retain(|n| !n.contains("ADBE Text Percent"));
            ra.retain(|(n, _)| !n.contains("ADBE Text Percent"));
        }
        // A record After Effects writes for a value at its default when another one makes it
        // matter (587: Smoothness 100 with Shape Ramp Up): absent = default.
        let explicit = |n: &str| n.ends_with("ADBE Text Selector Smoothness") && !nb.iter().any(|m| m == n);
        let default_100 = |rec: &[u8]| rec.windows(8).any(|w| w == 100.0f64.to_be_bytes());
        let drop: Vec<String> = ra.iter().filter(|(n, rec)| explicit(n) && default_100(rec)).map(|(n, _)| n.clone()).collect();
        na.retain(|n| !drop.contains(n));
        ra.retain(|(n, _)| !drop.contains(n));
        if na != nb {
            let short = |v: &[String]| v.iter().map(|s| s.rsplit('/').next().unwrap_or_default().to_string()).collect::<Vec<_>>();
            panic!(
                "{name}: structure
 AE  {:?}
 out {:?}",
                short(&na),
                short(&nb)
            );
        }
        for ((ka, a), (kb, b)) in ra.iter().zip(&rb) {
            assert_eq!(ka, kb, "{name}");
            assert_eq!(a, b, "{name}: {ka}");
            compared += 1;
        }
        assert_eq!(ra.len(), rb.len(), "{name}");
    }
    assert!(compared >= 30, "{compared}");
}

#[test]
fn hostile_text_animators_never_panic() {
    use effectcraft_project::build;
    let mut d = TextDoc::plain("Hostile");
    d.set_runs(vec![StyleRun { len: 7, style: style("ArialMT", 40.0) }]);
    let (mut p, c) = text_project(vec![d]);
    let mut next = p.next_id;
    {
        let mut ids = build::Ids(&mut next);
        let mut anims = Vec::new();
        for i in 0..300 {
            let props: Vec<_> =
                ["position", "scale", "fillColor", "tracking", "blur"].iter().flat_map(|k| build::text_anim_props(&mut ids, k, false)).collect();
            let mut a = build::text_animator(&mut ids, &format!("A{i}"), props);
            if let Some(pg) = a.sub_mut("properties") {
                pg.get_mut("position").unwrap().value = Value::Vec3([f64::NAN, 1e300, 0.0]);
                pg.get_mut("tracking").unwrap().keys = (0..2000).map(|k| key(f64::from(k) * 0.001, Value::Scalar(f64::from(k)))).collect();
                pg.get_mut("fillColor").unwrap().keys = vec![key(0.0, Value::Scalar(1.0)), key(1.0, Value::Color([f64::INFINITY, 0.0, 0.0, 1.0]))];
            }
            if let Some(sels) = a.sub_mut("selectors") {
                sels.children.push(build::wiggly_selector(&mut ids, "W").into());
            }
            anims.push(effectcraft_project::Node::Group(a));
        }
        with_layer(&mut p, c, 0, |l| l.props.sub_mut("text").unwrap().sub_mut("animators").unwrap().children = anims);
    }
    p.next_id = next;
    let r = export_all(&p);
    let file = parse(&r.bytes).expect("parse");
    assert_eq!(effectcraft_aep::write(&file).expect("write"), r.bytes);
    assert!(r.report.count(LossCategory::TextLayers) >= 300, "{:?}", r.report.losses);
    let _ = reimport(&r.bytes);
}
