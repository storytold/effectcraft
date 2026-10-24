//! Text document fields used by the importer: stroke colour, automatic leading, the font of each
//! run, the undecoded-slot check, damaged runs, the Source Text record and the text anchor rule.

mod common;

use common::{first_comp, fixture, layer_named, open};
use effectcraft_aep::{Caps, LayerType, Project, TextDocument, ValueSource};

/// Every synthetic text case that has a fixture, with its text.
const CASES: &[(&str, &str)] = &[
    ("110-text-A.aep", "A"),
    ("113-text-Hello-size60-fillorange.aep", "Hello"),
    ("114-text-animator-opacity.aep", "Hello"),
    ("243-text-multiline-A-CR-B.aep", "A\rB"),
    ("245-text-box-400x200-Hello.aep", "Hello"),
    ("246-text-box-center-align.aep", "Hello"),
    ("247-text-tracking-100.aep", "Hello"),
    ("248-text-range-fill-red-1-3.aep", "Hello"),
    ("334-text-box-right-align.aep", "Hello"),
    ("335-text-point-fauxbold.aep", "Hello"),
    ("336-text-point-fauxitalic.aep", "Hello"),
    ("337-text-point-allcaps.aep", "Hello"),
    ("338-text-point-smallcaps.aep", "Hello"),
    ("339-text-point-leading60.aep", "Hello"),
    ("340-text-range-two-styles.aep", "Hello"),
    ("385-text-anchor-10-20.aep", "A"),
    ("386-text-etx-A-B.aep", "A\u{3}B"),
];

fn doc(name: &str) -> TextDocument {
    let bytes = fixture(name);
    let file = open(&bytes);
    let project = Project::new(&file).unwrap();
    layer_named(&first_comp(&project), "T").text_document().unwrap_or_else(|| panic!("{name}: no text document"))
}

/// The raw `btdk` body of the text layer of a fixture.
fn body(name: &str) -> Vec<u8> {
    let bytes = fixture(name);
    let file = open(&bytes);
    let project = Project::new(&file).unwrap();
    let comp = first_comp(&project);
    let layer = layer_named(&comp, "T");
    layer.chunk().find_list(b"btdk").and_then(|c| c.bytes()).unwrap().to_vec()
}

fn replace_once(hay: &[u8], from: &[u8], to: &[u8]) -> Vec<u8> {
    replace_after(hay, b"", from, to)
}

/// Replaces the first `from` after the first `anchor` (the run style of 244 starts with
/// `/0 0 /1 100.0`; the document's style sheet before it has the same slots).
fn replace_after(hay: &[u8], anchor: &[u8], from: &[u8], to: &[u8]) -> Vec<u8> {
    let start = if anchor.is_empty() { 0 } else { hay.windows(anchor.len()).position(|w| w == anchor).unwrap() };
    let at = start + hay[start..].windows(from.len()).position(|w| w == from).unwrap_or_else(|| panic!("{:?} not found", String::from_utf8_lossy(from)));
    [&hay[..at], to, &hay[at + from.len()..]].concat()
}

#[test]
fn every_sample_is_complete_and_has_only_decoded_slots_changed() {
    for (name, text) in CASES {
        let d = doc(name);
        assert_eq!(d.text, *text, "{name}");
        assert_eq!(d.incomplete_runs, 0, "{name}");
        assert!(d.undecoded_style_fields.is_empty(), "{name}: {:?}", d.undecoded_style_fields);
        assert!(d.undecoded_paragraph_fields.is_empty(), "{name}: {:?}", d.undecoded_paragraph_fields);
        assert_eq!(d.style_runs.iter().map(|r| r.length).sum::<usize>(), d.raw_text.chars().count(), "{name}");
        for run in &d.style_runs {
            // No sample has automatic leading: the stored leading is used (98 by default).
            assert!(!run.auto_leading, "{name}");
            // The stroke colour is white in every sample (no stroke was sampled).
            assert_eq!(run.stroke_argb, Some([1.0; 4]), "{name}");
            assert_eq!(d.font_of(run), Some("TimesNewRomanPSMT"), "{name}");
        }
    }
}

#[test]
fn stored_leading_and_font_of_a_run() {
    assert_eq!(doc("243-text-multiline-A-CR-B.aep").style_runs[0].leading, Some(98.0));
    assert_eq!(doc("339-text-point-leading60.aep").style_runs[0].leading, Some(60.0));
    // 335: faux bold moves the run to font index 1, which is still Times New Roman.
    let d = doc("335-text-point-fauxbold.aep");
    assert_eq!((d.style_runs[0].font_index, d.font_of(&d.style_runs[0])), (1, Some("TimesNewRomanPSMT")));
}

#[test]
fn automatic_leading_is_slot_4() {
    let b = replace_once(&body("110-text-A.aep"), b"/4 false /5 98.0", b"/4 true /5 98.0");
    let d = TextDocument::parse(&b).unwrap();
    assert!(d.style_runs[0].auto_leading);
    assert!(d.undecoded_style_fields.is_empty());
}

#[test]
fn automatic_leading_as_after_effects_stores_it() {
    // 400: Auto Leading switched on for 387's "A" CR "B" (size 100, explicit leading 98 before).
    // Against its unedited save 399 the run style differs in `/4` (true) and `/5` (0.01) only, and
    // the composed second line sits 120 px (1.2 x 100) below the first, as After Effects measures.
    let d = doc("400-text-autoleading-on.aep");
    assert_eq!(d.text, "A\rB");
    assert_eq!(d.incomplete_runs, 0);
    assert!(d.undecoded_style_fields.is_empty(), "{:?}", d.undecoded_style_fields);
    assert!(d.undecoded_paragraph_fields.is_empty(), "{:?}", d.undecoded_paragraph_fields);
    for run in &d.style_runs {
        assert!(run.auto_leading);
        assert_eq!((run.font_size, run.leading), (100.0, Some(0.01)));
    }
    let b = body("400-text-autoleading-on.aep");
    let text = String::from_utf8_lossy(&b);
    assert!(text.contains("/0 << /0 [ 0.0 120.00001 ] >>"), "composed second baseline at 120 px");
}

#[test]
fn undecoded_slots_that_differ_are_listed() {
    let src = body("110-text-A.aep");
    // Style slot 68 and paragraph slot 24 changed to values outside their neutral ones (0 and 1,
    // 732, 734); a zero slot the samples lack (93) is not listed.
    const RUN: &[u8] = b"/0 0 /1 100.0";
    let b = replace_after(&src, RUN, b"/68 0 ", b"/68 2 ");
    let b = replace_after(&b, RUN, b"/96 true >>", b"/93 0.0 /94 0.0 /96 true >>");
    // The run's paragraph is the one with `/22 1 /23 true` (the style sheet's has `/22 0`).
    let para = b"/20 6 /21 false /22 1 /23 true /24 1";
    let b = replace_once(&b, para, b"/20 6 /21 false /22 1 /23 true /24 2");
    let d = TextDocument::parse(&b).unwrap();
    assert_eq!(d.undecoded_style_fields, ["68"]);
    assert_eq!(d.undecoded_paragraph_fields, ["24"]);
    // A non-zero unknown slot is listed.
    let d = TextDocument::parse(&replace_after(&src, RUN, b"/96 true >>", b"/97 2 /96 true >>")).unwrap();
    assert_eq!(d.undecoded_style_fields, ["97"]);
    // `/nil` is the stored form of a slot without a value (244 has three): not listed.
    let d = TextDocument::parse(&replace_once(&src, b"/22 1 /23 true /24 1", b"/22 1 /23 true /24 1 /48 /nil")).unwrap();
    assert!(d.undecoded_paragraph_fields.is_empty(), "{:?}", d.undecoded_paragraph_fields);
}

#[test]
fn a_damaged_run_is_kept_and_counted() {
    // 340 has three runs; the size of the third (60) is removed: the run stays and takes the size of
    // the one before it.
    let src = body("340-text-range-two-styles.aep");
    let at = src.windows(b"/1 60.0".len()).position(|w| w == b"/1 60.0").unwrap();
    let b = [&src[..at], &src[at + b"/1 60.0".len()..]].concat();
    let d = TextDocument::parse(&b).unwrap();
    assert_eq!(d.style_runs.len(), 3);
    assert_eq!(d.incomplete_runs, 1);
    assert_eq!(d.style_runs[2].font_size, 100.0);
    assert_eq!(d.style_runs[2].fill_argb, [1.0, 0.0, 0.0, 1.0]);
    assert_eq!(d.style_runs[2].caps, Caps::Normal);
}

#[test]
fn source_text_record_and_text_anchor() {
    for name in ["110-text-A.aep", "114-text-animator-opacity.aep", "248-text-range-fill-red-1-3.aep"] {
        let bytes = fixture(name);
        let file = open(&bytes);
        let project = Project::new(&file).unwrap();
        let comp = first_comp(&project);
        let t = layer_named(&comp, "T");
        assert_eq!(t.header().unwrap().layer_type, LayerType::Text);
        let st = t.source_text_property().expect("source text record");
        assert!(!st.is_animated() && st.expression().is_none(), "{name}");
        assert_eq!(t.text_document_count(), 1, "{name}");
        // The text anchor is absent: the text origin, not the centre of a source.
        let tr = t.transform(1920, 1080).unwrap();
        assert_eq!(tr.anchor_fraction.source, ValueSource::Default, "{name}");
        assert_eq!(tr.text_anchor_px(), [0.0; 3], "{name}");
        assert_eq!(tr.position.value, [960.0, 540.0, 0.0], "{name}");
    }
}

#[test]
fn a_stored_text_anchor_is_pixels() {
    // 385: 110's text layer with Anchor Point [10, 20], stored as pixels (a text layer has no source).
    let bytes = fixture("385-text-anchor-10-20.aep");
    let file = open(&bytes);
    let project = Project::new(&file).unwrap();
    let comp = first_comp(&project);
    let tr = layer_named(&comp, "T").transform(1920, 1080).unwrap();
    assert_eq!(tr.anchor_fraction.source, ValueSource::Stored);
    assert_eq!(tr.text_anchor_px(), [10.0, 20.0, 0.0]);
    assert_eq!(tr.position.value, [960.0, 540.0, 0.0]);
}

#[test]
fn etx_is_stored_as_it_is_and_splits_the_paragraph_runs() {
    // 386: "A" U+0003 "B" is stored with the U+0003; After Effects lays it out like 243's CR (two
    // lines, two paragraphs), and the paragraph runs say the same.
    let etx = doc("386-text-etx-A-B.aep");
    let cr = doc("243-text-multiline-A-CR-B.aep");
    assert_eq!(etx.raw_text, "A\u{3}B\r");
    assert_eq!(etx.paragraphs.len(), cr.paragraphs.len());
    assert_eq!(etx.paragraphs.iter().map(|p| p.length).collect::<Vec<_>>(), cr.paragraphs.iter().map(|p| p.length).collect::<Vec<_>>());
}

#[test]
fn hostile_documents_never_panic() {
    let src = body("340-text-range-two-styles.aep");
    // Every prefix, a run length that overflows, a string with a lone surrogate, deep nesting.
    for cut in (0..src.len()).step_by(97) {
        let _ = TextDocument::parse(&src[..cut]);
    }
    let _ = TextDocument::parse(&replace_once(&src, b"/1 3 >>", b"/1 99999999999999999999 >>"));
    let lone = [b"/1 << /1 [ << /0 << /0 (".as_slice(), &[0xfe, 0xff, 0xd8, 0x00, 0x00, b'A'], b") >> >> ] >>"].concat();
    let _ = TextDocument::parse(&lone);
    let deep = [b"/1 ".as_slice(), &b"<< /0 ".repeat(10_000), &b">> ".repeat(10_000)].concat();
    assert!(TextDocument::parse(&deep).is_none());
}
