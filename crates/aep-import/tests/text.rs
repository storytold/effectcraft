//! Text layers: every text fixture imports with the values it was made with (the corpus
//! MANIFEST), style runs, a parented text layer with Opacity keys, the font report and hostile
//! text documents.

mod common;

use common::{comp, fixture_bytes, load};
use effectcraft_aep::{AepFile, Body, Chunk, parse, write};
use effectcraft_aep_import::{ImportOptions, ImportResult, LossCategory, import};
use effectcraft_keyframe::Value;
use effectcraft_keyframe::text_doc::{Justify, TextDoc};
use effectcraft_project::{Comp, ItemKind, Layer, LayerSource, Project};
use effectcraft_time::{TICKS_PER_SECOND, Tick};

const WHITE: [f32; 4] = [1.0, 1.0, 1.0, 1.0];
const RED: [f32; 4] = [1.0, 0.0, 0.0, 1.0];
const BLUE: [f32; 4] = [0.0, 0.0, 1.0, 1.0];

fn text_layer(c: &Comp) -> &Layer {
    c.layers.iter().find(|l| l.name == "T").expect("text layer T")
}

fn source_text(l: &Layer) -> TextDoc {
    match &l.props.prop("text/sourceText").expect("source text").value {
        Value::Text(t) => (**t).clone(),
        other => panic!("source text is {other:?}"),
    }
}

fn vec3(l: &Layer, path: &str) -> [f64; 3] {
    match l.props.prop(path).map(|p| p.value.clone()) {
        Some(Value::Vec3(v)) => v,
        other => panic!("{path}: {other:?}"),
    }
}

/// Imports a fixture and returns the text layer's Source Text, checking what every text layer
/// shares: a text layer on top of L, the text origin as anchor, Position at the comp centre, font
/// Times New Roman, no stroke.
fn imported(name: &str) -> (TextDoc, ImportResult) {
    let (p, r) = load(name);
    let (_, c) = comp(&p, &r, "C");
    assert_eq!(c.layers.len(), 2, "{name}");
    let t = text_layer(c);
    assert_eq!(c.layers[0].id, t.id, "{name}: T is the top layer");
    assert!(c.layers.iter().any(|l| l.name == "L"), "{name}");
    assert!(matches!(t.source, LayerSource::Text), "{name}");
    assert_eq!(vec3(t, "transform/anchor"), [0.0, 0.0, 0.0], "{name}");
    assert_eq!(vec3(t, "transform/position"), [960.0, 540.0, 0.0], "{name}");
    assert_eq!(r.report.imported.text_layers, 1, "{name}");
    let d = source_text(t);
    for run in d.runs() {
        assert_eq!(run.style.font, "TimesNewRomanPSMT", "{name}");
        assert_eq!(run.style.style, "Regular", "{name}");
        assert!(!run.style.apply_stroke, "{name}");
    }
    assert!(r.report.missing_fonts.is_empty(), "{name}: every font counts as installed by default");
    (d, r)
}

/// The fixtures with nothing to lose.
fn lossless(name: &str) -> TextDoc {
    let (d, r) = imported(name);
    assert!(r.report.is_lossless(), "{name}: {:?}", r.report.losses);
    d
}

#[test]
fn point_text_defaults_size_and_fill() {
    // 110: "A", Times New Roman 100 px, white.
    let d = lossless("110-text-A.aep");
    assert_eq!((d.text.as_str(), d.size, d.fill, d.box_size), ("A", 100.0, WHITE, None));
    assert_eq!((d.tracking, d.leading, d.justify), (0.0, Some(98.0), Justify::Left));
    assert!(d.is_uniform());
    assert!(!(d.faux_bold || d.faux_italic || d.all_caps || d.small_caps));
    // 113: "Hello" at 60 px, orange [1, 0.5, 0].
    let d = lossless("113-text-Hello-size60-fillorange.aep");
    assert_eq!((d.text.as_str(), d.size, d.fill), ("Hello", 60.0, [1.0, 0.5, 0.0, 1.0]));
}

#[test]
fn two_paragraphs() {
    // 243: "A" CR "B": two paragraphs, leading 98 (the second baseline is 98 px lower in AE).
    let d = lossless("243-text-multiline-A-CR-B.aep");
    assert_eq!(d.text, "A\rB");
    assert_eq!(d.para_count(), 2);
    assert_eq!(d.paras().iter().map(|p| p.justify).collect::<Vec<_>>(), [Justify::Left, Justify::Left]);
    assert_eq!(d.leading, Some(98.0));
}

#[test]
fn automatic_leading_imports_as_automatic_leading() {
    // 400: 387's "A" CR "B" with Auto Leading switched on (After Effects then sets the second
    // baseline 120 px below the first, 1.2 x the size 100): EffectCraft's automatic leading is the
    // same 120 % rule, so no explicit leading is set (the stored 0.01 is not used).
    let d = lossless("400-text-autoleading-on.aep");
    assert_eq!((d.text.as_str(), d.size, d.para_count()), ("A\rB", 100.0, 2));
    assert_eq!(d.leading, None);
}

#[test]
fn etx_is_a_paragraph_break() {
    // 386: "A" U+0003 "B". After Effects lays it out exactly as 387's "A" CR "B" (two lines, two
    // paragraphs, baselines 98 px apart): it imports as 243's CR text, without a loss.
    let d = lossless("386-text-etx-A-B.aep");
    let cr = lossless("243-text-multiline-A-CR-B.aep");
    assert_eq!(d.text, "A\rB");
    assert_eq!(d.para_count(), 2);
    assert_eq!(d.paras().iter().map(|p| p.justify).collect::<Vec<_>>(), [Justify::Left, Justify::Left]);
    assert_eq!((d.leading, d.size), (cr.leading, cr.size));
    assert_eq!(d.runs().iter().map(|r| r.len).sum::<usize>(), 3);
}

#[test]
fn an_unnamed_text_layer_is_named_after_its_first_paragraph() {
    // 386 and 243 with the text layer's own name removed: both are named "A", as U+0003 and CR
    // both end the first paragraph.
    for name in ["386-text-etx-A-B.aep", "243-text-multiline-A-CR-B.aep"] {
        let src = fixture_bytes(name);
        let mut file = parse(&src).unwrap();
        let mut done = false;
        visit(&mut file.chunks, &mut |c| {
            if c.is_list(b"Layr") && utf8_name(c).as_deref() == Some("T") {
                for k in c.children_mut() {
                    if k.is(b"Utf8")
                        && let Body::Data(d) = &mut k.body
                    {
                        *d = Vec::new().into();
                        done = true;
                    }
                }
            }
        });
        assert!(done, "{name}");
        let bytes = write(&file).unwrap();
        let mut p = Project::default();
        let none = |_: &str| false;
        let r = import(&mut p, &bytes, &ImportOptions::new(&none)).expect("lossless");
        let (_, c) = comp(&p, &r, "C");
        assert!(
            c.layers.iter().any(|l| l.name == "A" && matches!(l.source, LayerSource::Text)),
            "{name}: {:?}",
            c.layers.iter().map(|l| &l.name).collect::<Vec<_>>()
        );
    }
}

#[test]
fn other_control_characters_are_still_reported() {
    // A text with U+0003 and U+0007: the U+0003 becomes a paragraph break, the U+0007 is kept and reported.
    let src = fixture_bytes("386-text-etx-A-B.aep");
    let from = utf16("A\u{3}B\r");
    let to = utf16("A\u{3}\u{7}\r");
    let at = src.windows(from.len()).position(|w| w == from.as_slice()).expect("the stored text");
    let bytes = [&src[..at], to.as_slice(), &src[at + from.len()..]].concat();
    let mut p = Project::default();
    let none = |_: &str| false;
    let r = import(&mut p, &bytes, &ImportOptions { allow_losses: true, ..ImportOptions::new(&none) }).unwrap();
    let (_, c) = comp(&p, &r, "C");
    assert_eq!(source_text(text_layer(c)).text, "A\r\u{7}");
    let loss = r.report.losses.iter().find(|l| l.category == LossCategory::TextLayers).expect("reported");
    assert!(loss.details.iter().any(|d| d.contains("U+0007") && !d.contains("U+0003")), "{:?}", loss.details);
}

#[test]
fn a_stored_text_anchor_imports_as_pixels() {
    // 385: 110 with the text layer's Anchor Point set to [10, 20], stored as pixels.
    let (p, r) = load("385-text-anchor-10-20.aep");
    assert!(r.report.is_lossless(), "{:?}", r.report.losses);
    let (_, c) = comp(&p, &r, "C");
    let t = text_layer(c);
    assert_eq!(vec3(t, "transform/anchor"), [10.0, 20.0, 0.0]);
    assert_eq!(vec3(t, "transform/position"), [960.0, 540.0, 0.0]);
    assert_eq!(source_text(t).text, "A");
}

#[test]
fn box_text_and_justification() {
    // 245: box 400 x 200, top-left [-200, -100] in layer space, left; 246 centre; 334 right.
    for (name, justify) in [
        ("245-text-box-400x200-Hello.aep", Justify::Left),
        ("246-text-box-center-align.aep", Justify::Center),
        ("334-text-box-right-align.aep", Justify::Right),
    ] {
        let d = lossless(name);
        assert_eq!(d.text, "Hello", "{name}");
        assert_eq!((d.box_size, d.box_pos), (Some([400.0, 200.0]), [-200.0, -100.0]), "{name}");
        assert_eq!(d.justify, justify, "{name}");
    }
}

#[test]
fn character_styles() {
    // 247: tracking 100.
    assert_eq!(lossless("247-text-tracking-100.aep").tracking, 100.0);
    // 335 / 336: faux bold / italic (the font stays Times New Roman Regular).
    let d = lossless("335-text-point-fauxbold.aep");
    assert!(d.faux_bold && !d.faux_italic);
    let d = lossless("336-text-point-fauxitalic.aep");
    assert!(d.faux_italic && !d.faux_bold);
    // 337 / 338: all caps / small caps.
    let d = lossless("337-text-point-allcaps.aep");
    assert!(d.all_caps && !d.small_caps);
    let d = lossless("338-text-point-smallcaps.aep");
    assert!(d.small_caps && !d.all_caps);
    // 339: leading 60.
    assert_eq!(lossless("339-text-point-leading60.aep").leading, Some(60.0));
}

#[test]
fn style_runs() {
    // 248: characters 1..3 ("el") red, the rest white.
    let d = lossless("248-text-range-fill-red-1-3.aep");
    let runs = d.runs();
    assert_eq!(runs.iter().map(|r| r.len).collect::<Vec<_>>(), [1, 2, 2]);
    assert_eq!(runs.iter().map(|r| r.style.fill).collect::<Vec<_>>(), [WHITE, RED, WHITE]);
    assert_eq!(d.substring(1..3), "el");
    // 340: "el" red; "lo" blue at 60 px. AE's last run (3 characters) includes the final CR.
    let d = lossless("340-text-range-two-styles.aep");
    let runs = d.runs();
    assert_eq!(runs.iter().map(|r| r.len).collect::<Vec<_>>(), [1, 2, 2]);
    assert_eq!(runs.iter().map(|r| r.style.fill).collect::<Vec<_>>(), [WHITE, RED, BLUE]);
    assert_eq!(runs.iter().map(|r| r.style.size).collect::<Vec<_>>(), [100.0, 100.0, 60.0]);
    assert_eq!(d.style_at(4).size, 60.0);
}

#[test]
fn the_text_of_an_animated_layer_arrives() {
    // 114: 113 plus "Animator 1" (Opacity 0, Range Selector 1): the text arrives, and the animator
    // too (tests/text_anim.rs).
    let (d, r) = imported("114-text-animator-opacity.aep");
    assert_eq!((d.text.as_str(), d.size, d.fill), ("Hello", 60.0, [1.0, 0.5, 0.0, 1.0]));
    assert!(r.report.is_lossless(), "{:?}", r.report.losses);
}

#[test]
fn a_text_layer_is_no_loss_and_the_import_is_not_refused() {
    let mut p = Project::default();
    let none = |_: &str| false;
    let r = import(&mut p, &fixture_bytes("113-text-Hello-size60-fillorange.aep"), &ImportOptions::new(&none)).expect("lossless import");
    assert!(r.report.is_lossless());
}

#[test]
fn missing_fonts_are_listed_not_lost() {
    let mut p = Project::default();
    let none = |_: &str| false;
    let no_fonts = |_: &str| false;
    let opts = ImportOptions { has_font: &no_fonts, ..ImportOptions::new(&none) };
    // Losses are not allowed: a missing font must not refuse the import.
    let r = import(&mut p, &fixture_bytes("340-text-range-two-styles.aep"), &opts).expect("a missing font is not a loss");
    // The font list has four entries; only the one the runs use is checked and listed, once.
    assert_eq!(r.report.missing_fonts, ["TimesNewRomanPSMT"]);
    let (_, c) = comp(&p, &r, "C");
    assert!(source_text(text_layer(c)).runs().iter().all(|run| run.style.font == "TimesNewRomanPSMT"));
    // A checker that knows the font lists nothing.
    let mut p = Project::default();
    let times = |f: &str| f == "TimesNewRomanPSMT";
    let r = import(&mut p, &fixture_bytes("340-text-range-two-styles.aep"), &ImportOptions { has_font: &times, ..ImportOptions::new(&none) }).unwrap();
    assert!(r.report.missing_fonts.is_empty());
    // The dry run reports the same.
    let report = effectcraft_aep_import::analyze_with(&fixture_bytes("110-text-A.aep"), &opts).unwrap();
    assert_eq!(report.missing_fonts, ["TimesNewRomanPSMT"]);
}

// ---------------------------------------------------------------- built cases

fn visit(chunks: &mut [Chunk<'_>], f: &mut dyn FnMut(&mut Chunk<'_>)) {
    for c in chunks {
        f(c);
        visit(c.children_mut(), f);
    }
}

fn utf8_name(c: &Chunk<'_>) -> Option<String> {
    c.children().iter().find(|k| k.is(b"Utf8")).and_then(Chunk::bytes).map(|b| String::from_utf8_lossy(b).to_string())
}

fn tdmn_is(c: &Chunk<'_>, name: &str) -> bool {
    c.is(b"tdmn") && c.bytes().is_some_and(|b| b.split(|x| *x == 0).next() == Some(name.as_bytes()))
}

/// The list that follows the `tdmn` named `name` among `children`.
fn index_after(children: &[Chunk<'_>], name: &str) -> Option<usize> {
    let at = children.iter().position(|c| tdmn_is(c, name))?;
    Some(at + 1)
}

/// The id of a layer (`ldta` bytes 0..4).
fn layer_id(c: &Chunk<'_>) -> Option<u32> {
    let b = c.children().iter().find(|k| k.is(b"ldta"))?.bytes()?;
    Some(u32::from_be_bytes(b.get(0..4)?.try_into().ok()?))
}

/// The `tdmn` + `LIST tdbs` pair of `ADBE Opacity` in the transform group of the layer `id`.
fn opacity_pair<'a>(file: &AepFile<'a>, id: u32) -> (Chunk<'a>, Chunk<'a>) {
    let mut out = None;
    let mut stack: Vec<&Chunk<'a>> = file.chunks.iter().collect();
    while let Some(c) = stack.pop() {
        if c.is_list(b"Layr") && layer_id(c) == Some(id) {
            let root = c.child_list(b"tdgp").unwrap();
            let tr = &root.children()[index_after(root.children(), "ADBE Transform Group").unwrap()];
            let i = index_after(tr.children(), "ADBE Opacity").unwrap();
            out = Some((tr.children()[i - 1].clone(), tr.children()[i].clone()));
        }
        stack.extend(c.children());
    }
    out.expect("opacity of the layer")
}

#[test]
fn parented_text_with_opacity_keys() {
    // 113's text layer T, parented to the solid L (layer id 0x1b, `ldta` byte 132 as in 022), with
    // the two Opacity keys of 058 (0 % at 0 s, 100 % at 2 s) copied into T's transform group.
    let keys_src = fixture_bytes("058-key-opacity-2key.aep");
    let keys_file = parse(&keys_src).unwrap();
    let (tdmn, tdbs) = opacity_pair(&keys_file, 0x1b);
    let (tdmn, tdbs) = (tdmn.into_owned(), tdbs.into_owned());
    let src = fixture_bytes("113-text-Hello-size60-fillorange.aep");
    let mut file = parse(&src).unwrap();
    let mut done = (false, false);
    visit(&mut file.chunks, &mut |c| {
        if !(c.is_list(b"Layr") && utf8_name(c).as_deref() == Some("T")) {
            return;
        }
        for k in c.children_mut() {
            if k.is(b"ldta")
                && let Body::Data(d) = &mut k.body
            {
                d.to_mut()[132..136].copy_from_slice(&0x1b_u32.to_be_bytes());
                done.0 = true;
            }
            if k.is_list(b"tdgp") {
                let i = index_after(k.children(), "ADBE Transform Group").unwrap();
                if let Body::List { children, .. } = &mut k.children_mut()[i].body {
                    let end = children.iter().position(|c| tdmn_is(c, "ADBE Group End")).unwrap();
                    children.insert(end, tdbs.clone());
                    children.insert(end, tdmn.clone());
                    done.1 = true;
                }
            }
        }
    });
    assert_eq!(done, (true, true));
    let bytes = write(&file).unwrap();
    let mut p = Project::default();
    let none = |_: &str| false;
    let r = import(&mut p, &bytes, &ImportOptions::new(&none)).expect("lossless");
    let (_, c) = comp(&p, &r, "C");
    let t = text_layer(c);
    let l = c.layers.iter().find(|l| l.name == "L").unwrap();
    assert_eq!(t.parent, Some(l.id));
    let opacity = t.props.prop("transform/opacity").unwrap();
    let keys: Vec<(Tick, Value)> = opacity.keys.iter().map(|k| (k.time, k.value.clone())).collect();
    assert_eq!(keys, [(Tick::ZERO, Value::Scalar(0.0)), (Tick(2 * TICKS_PER_SECOND), Value::Scalar(100.0))]);
    assert_eq!(source_text(t).text, "Hello");
    assert_eq!(r.report.imported.keyframes, 2);
}

/// Replaces the body of the text layer's `btdk` list.
fn with_btdk(name: &str, body: &[u8]) -> Vec<u8> {
    let src = fixture_bytes(name);
    let mut file = parse(&src).unwrap();
    visit(&mut file.chunks, &mut |c| {
        if let Body::OpaqueList { list_type, data } = &mut c.body
            && list_type == b"btdk"
        {
            *data = body.to_vec().into();
        }
    });
    write(&file).unwrap()
}

/// Imports with and without losses allowed; checks the result is usable.
fn import_hostile(bytes: &[u8]) -> Option<ImportResult> {
    let none = |_: &str| false;
    let no_fonts = |_: &str| false;
    let mut last = None;
    for allow in [false, true] {
        let mut p = Project::default();
        let opts = ImportOptions { allow_losses: allow, has_font: &no_fonts, ..ImportOptions::new(&none) };
        if let Ok(r) = import(&mut p, bytes, &opts) {
            serde_json::to_string(&p).expect("the project serialises");
            for item in p.items.values() {
                let ItemKind::Comp(c) = &item.kind else { continue };
                for l in &c.layers {
                    if let Some(Value::Text(t)) = l.props.prop("text/sourceText").map(|p| &p.value) {
                        let runs = t.runs();
                        assert_eq!(runs.iter().map(|r| r.len).sum::<usize>(), t.char_len());
                        assert!(runs.iter().all(|r| r.style.size.is_finite() && r.style.size > 0.0 && r.style.fill.iter().all(|c| c.is_finite())));
                        assert!(t.leading.is_none_or(f64::is_finite) && t.tracking.is_finite());
                        assert!(t.box_size.is_none_or(|b| b.iter().all(|v| v.is_finite() && *v >= 1.0)));
                    }
                }
            }
            last = Some(r);
        }
    }
    last
}

fn utf16(s: &str) -> Vec<u8> {
    let mut out = vec![0xfe, 0xff];
    for u in s.encode_utf16() {
        out.extend_from_slice(&u.to_be_bytes());
    }
    out
}

#[test]
fn hostile_text_documents_never_panic() {
    let real = {
        let src = fixture_bytes("340-text-range-two-styles.aep");
        let f = parse(&src).unwrap();
        let mut body = None;
        let mut stack: Vec<&Chunk<'_>> = f.chunks.iter().collect();
        while let Some(c) = stack.pop() {
            if let Body::OpaqueList { list_type, data } = &c.body
                && list_type == b"btdk"
            {
                body = Some(data.to_vec());
            }
            stack.extend(c.children());
        }
        body.unwrap()
    };
    let hello = utf16("Hello\r");
    let at = real.windows(hello.len()).position(|w| w == hello.as_slice()).expect("the text in the body");
    let with_text = |t: &[u8]| [&real[..at], t, &real[at + hello.len()..]].concat();
    let mut cases: Vec<Vec<u8>> = vec![
        b"garbage".to_vec(),
        Vec::new(),
        b"/1 << /1 [ << /0 << /0 (".to_vec(),
        // Lone surrogates and an odd byte count.
        with_text(&[0xfe, 0xff, 0xd8, 0x00, 0x00, b'H', 0xdc, 0x00, 0x00]),
        // A huge text (2 MB of UTF-16) with the runs of a five-character one.
        with_text(&[vec![0xfe, 0xff], [0x00, b'x'].repeat(1 << 20)].concat()),
        // Control characters (U+0003 is a paragraph break, U+0007 is reported) and an empty text.
        with_text(&utf16("He\u{3}l\u{7}lo\r")),
        with_text(&utf16("")),
        // Run lengths that overflow, are negative or much longer than the text.
        real.replacen(b"/1 3 >>", b"/1 99999999999999999999999 >>"),
        real.replacen(b"/1 3 >>", b"/1 -3 >>"),
        real.replacen(b"/1 1 >>", b"/1 1000000 >>"),
        // Not-a-number sizes and colours, a font index far outside the font list.
        real.replacen(b"/1 60.0", b"/1 nan"),
        real.replacen(b"[ 1.0 0.0 0.0 1.0 ]", b"[ 1.0 inf -inf 1e309 ]"),
        real.replacen(b"/6 << /0 0 /1 100.0", b"/6 << /0 999999 /1 100.0"),
        // Unknown caps and justification codes.
        real.replacen(b"/12 0 ", b"/12 7 "),
        // Deep nesting.
        [b"/1 ".as_slice(), &b"<< /0 ".repeat(50_000), &b">> ".repeat(50_000)].concat(),
        [b"/1 ".as_slice(), &b"[ ".repeat(50_000)].concat(),
    ];
    for cut in (0..real.len()).step_by(211) {
        cases.push(real[..cut].to_vec());
    }
    for (i, body) in cases.iter().enumerate() {
        let r = import_hostile(&with_btdk("340-text-range-two-styles.aep", body));
        assert!(r.is_some(), "case {i}: the import with losses allowed must succeed");
    }
}

/// `replacen` for byte strings (first occurrence; panics when absent so a case cannot rot).
trait ReplaceBytes {
    fn replacen(&self, from: &[u8], to: &[u8]) -> Vec<u8>;
}

impl ReplaceBytes for Vec<u8> {
    fn replacen(&self, from: &[u8], to: &[u8]) -> Vec<u8> {
        let at = self.windows(from.len()).position(|w| w == from).unwrap_or_else(|| panic!("{:?} not in the body", String::from_utf8_lossy(from)));
        [&self[..at], to, &self[at + from.len()..]].concat()
    }
}

#[test]
fn unreadable_text_keeps_the_layer() {
    let r = import_hostile(&with_btdk("113-text-Hello-size60-fillorange.aep", b"not a text document")).unwrap();
    assert_eq!(r.report.imported.text_layers, 1);
    assert_eq!(r.report.count(LossCategory::DamagedData), 1, "{:?}", r.report.losses);
}

#[test]
fn superscript_and_subscript_import_as_such() {
    use effectcraft_keyframe::text_doc::BaselineOption;
    // 526: the corpus' default character state is superscript; 527 normal; 528 subscript.
    for (name, want) in [
        ("526-text-point-HHHH-default.aep", BaselineOption::Superscript),
        ("527-text-point-HHHH-baseline-normal.aep", BaselineOption::Normal),
        ("528-text-point-HHHH-baseline-subscript.aep", BaselineOption::Subscript),
        ("243-text-multiline-A-CR-B.aep", BaselineOption::Superscript),
    ] {
        let doc = lossless(name);
        assert_eq!(doc.style_at(0).baseline, want, "{name}");
    }
}

#[test]
fn a_wrapping_box_text_keeps_its_box() {
    let doc = lossless("517-text-box-600x400-wrap.aep");
    assert_eq!(doc.text, "The quick brown fox jumps over the lazy dog and keeps running far away");
    assert_eq!((doc.box_size, doc.box_pos), (Some([600.0, 400.0]), [-300.0, -200.0]));
    assert_eq!(doc.style_at(0).size, 100.0);
}
