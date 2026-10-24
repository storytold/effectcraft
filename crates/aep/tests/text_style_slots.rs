//! Character and paragraph slots of the text document, each found with one scripted change
//! against the reference point text 538 (`HHHH`, Times New Roman 100 px, normal baseline).

mod common;

use common::{first_comp, fixture, layer_named, open};
use effectcraft_aep::{AutoKern, Justification, KernRun, Project, TextDocument};

fn text(name: &str) -> TextDocument {
    let bytes = fixture(name);
    let file = open(&bytes);
    let project = Project::new(&file).unwrap();
    layer_named(&first_comp(&project), "T").text_document().unwrap_or_else(|| panic!("{name}: no text document"))
}

#[test]
fn the_reference_text_has_the_defaults_and_nothing_undecoded() {
    let t = text("538-text-point-HHHH-normal-ref.aep");
    let r = t.style_runs[0];
    assert_eq!((r.horizontal_scale, r.vertical_scale, r.baseline_shift, r.tsume, r.stroke_width), (1.0, 1.0, 0.0, 0.0, 1.0));
    assert_eq!(r.auto_kern, AutoKern::Metrics);
    assert_eq!((r.ligatures, r.no_break, r.apply_fill, r.apply_stroke, r.stroke_over_fill), (false, false, true, false, true));
    assert_eq!((r.line_join, r.baseline_direction, r.digit_set), (0, 1, 0));
    let p = t.paragraphs[0];
    assert_eq!(p.justification, Justification::Left);
    assert_eq!((p.first_line_indent, p.start_indent, p.end_indent, p.space_before, p.space_after), (0.0, 0.0, 0.0, 0.0, 0.0));
    assert_eq!((p.leading_type, p.auto_hyphenate, p.hanging_roman, p.every_line_composer, p.direction), (0, true, false, false, 0));
    assert!(t.kerning.is_empty());
    assert_eq!(t.line_orientation, None);
    assert!(t.undecoded_style_fields.is_empty() && t.undecoded_paragraph_fields.is_empty(), "{t:?}");
}

#[test]
fn character_slots_each_hold_one_setting() {
    let run = |name: &str| {
        let t = text(name);
        assert!(t.undecoded_style_fields.is_empty() && t.undecoded_paragraph_fields.is_empty(), "{name}: {:?}", t.undecoded_style_fields);
        t.style_runs[0]
    };
    assert_eq!(run("539-text-hscale-0.9.aep").horizontal_scale, 0.9);
    assert_eq!(run("540-text-vscale-1.5.aep").vertical_scale, 1.5);
    assert_eq!(run("541-text-baselineshift-41.aep").baseline_shift, 41.0);
    assert_eq!(run("542-text-autokern-none.aep").auto_kern, AutoKern::None);
    assert_eq!(run("543-text-autokern-optical.aep").auto_kern, AutoKern::Optical);
    assert!(run("544-text-applystroke-on.aep").apply_stroke);
    assert_eq!(run("545-text-strokewidth-5.aep").stroke_width, 5.0);
    assert!(!run("546-text-applyfill-off.aep").apply_fill);
    assert_eq!(run("548-text-linejoin-round.aep").line_join, 1);
    assert!(!run("550-text-strokeoverfill-off.aep").stroke_over_fill);
    assert!(run("552-text-nobreak-on.aep").no_break);
    assert_eq!(run("553-text-tsume-0.5.aep").tsume, 0.5);
    assert!(run("555-text-ligature-on.aep").ligatures);
    assert_eq!(run("571-text-digitset-arabic.aep").digit_set, 1);
    assert_eq!(run("578-text-digitset-hindi.aep").digit_set, 2);
    assert_eq!(run("573-text-baselinedir-rotated.aep").baseline_direction, 2);
    assert_eq!(run("576-text-baselinedir-crossstream.aep").baseline_direction, 3);
}

#[test]
fn paragraph_slots_each_hold_one_setting() {
    let para = |name: &str| {
        let t = text(name);
        assert!(t.undecoded_style_fields.is_empty() && t.undecoded_paragraph_fields.is_empty(), "{name}: {:?}", t.undecoded_paragraph_fields);
        t.paragraphs[0]
    };
    assert_eq!(para("560-text-firstlineindent-20.aep").first_line_indent, 20.0);
    assert_eq!(para("561-text-startindent-30.aep").start_indent, 30.0);
    assert_eq!(para("562-text-endindent-40.aep").end_indent, 40.0);
    assert_eq!(para("563-text-spacebefore-10.aep").space_before, 10.0);
    assert_eq!(para("564-text-spaceafter-12.aep").space_after, 12.0);
    assert_eq!(para("572-text-leadingtype-japanese.aep").leading_type, 1);
    assert!(!para("556-text-autohyphenate-off.aep").auto_hyphenate);
    assert!(para("558-text-hangingroman-on.aep").hanging_roman);
    assert!(para("559-text-everyline-on.aep").every_line_composer);
    assert_eq!(para("569-text-direction-rtl.aep").direction, 1);
    assert_eq!(para("565-text-justify-fullleft.aep").justification, Justification::FullLeft);
    assert_eq!(para("566-text-justify-fullcenter.aep").justification, Justification::FullCenter);
    assert_eq!(para("567-text-justify-fullright.aep").justification, Justification::FullRight);
    assert_eq!(para("568-text-justify-fullall.aep").justification, Justification::FullAll);
}

#[test]
fn manual_kerning_is_a_list_of_character_runs() {
    // 554: kerning 50 on the whole text: automatic kerning is switched off (0) and each of the four
    // characters carries 50; the closing `\r` has no amount.
    let t = text("554-text-kerning-50.aep");
    assert_eq!(t.style_runs[0].auto_kern, AutoKern::None);
    assert_eq!(t.kerning.iter().map(|k| k.length).sum::<usize>(), 5);
    assert!(t.kerning.iter().take(4).all(|k| *k == KernRun { length: 1, amount: Some(50.0) }), "{:?}", t.kerning);
    assert_eq!(t.kerning.last().and_then(|k| k.amount), None);
}

#[test]
fn vertical_text_has_its_line_orientation() {
    assert_eq!(text("575-text-vertical-rtl.aep").line_orientation, Some(2));
}

#[test]
fn several_settings_in_one_document() {
    // 595: stroke on, width 5, blue, horizontal 0.9, vertical 1.5, baseline shift 41; 596 the same
    // with the fill over the stroke.
    let a = text("595-text-stroke-scale-shift-combined.aep").style_runs[0];
    assert_eq!((a.apply_stroke, a.stroke_width, a.horizontal_scale, a.vertical_scale, a.baseline_shift), (true, 5.0, 0.9, 1.5, 41.0));
    assert_eq!(a.stroke_argb, Some([1.0, 0.0, 0.0, 1.0]));
    assert!(a.stroke_over_fill);
    assert!(!text("596-text-combined-fill-over-stroke.aep").style_runs[0].stroke_over_fill);
}

#[test]
fn paragraph_slot_34_is_neutral_for_its_two_sampled_values() {
    // 727: Reset Paragraph changes only /34 (0 -> 7); both values lay text out alike (S18).
    assert_eq!(text("725-text-point-HHHH-normal-ref-S18.aep").undecoded_paragraph_fields, Vec::<String>::new());
    let t = text("727-text-reset-paragraph.aep");
    assert!(t.undecoded_paragraph_fields.is_empty(), "{:?}", t.undecoded_paragraph_fields);
    // Another value of /34 is still listed.
    let bytes = fixture("727-text-reset-paragraph.aep");
    let file = open(&bytes);
    let project = Project::new(&file).unwrap();
    let layer = layer_named(&first_comp(&project), "T");
    let body = layer.chunk().find_list(b"btdk").and_then(|c| c.bytes()).unwrap().to_vec();
    // Every `/34 7` (the run's paragraph and the document's paragraph sheet) becomes `/34 5`.
    let mut changed = Vec::with_capacity(body.len());
    let mut i = 0;
    while i < body.len() {
        if body[i..].starts_with(b"/34 7 ") {
            changed.extend_from_slice(b"/34 5 ");
            i += 6;
        } else {
            changed.push(body[i]);
            i += 1;
        }
    }
    assert_eq!(TextDocument::parse(&changed).unwrap().undecoded_paragraph_fields, ["34"]);
}

#[test]
fn slots_24_35_and_68_are_neutral_for_0_and_1() {
    // 732-734: 517 with paragraph /24, paragraph /35 or style /68 edited; After Effects keeps the
    // value and lays the text out the same (S21).
    for name in ["732-text-box-byteedit-para24-0.aep", "733-text-box-byteedit-para35-0.aep", "734-text-box-byteedit-style68-1.aep"] {
        let t = text(name);
        assert!(
            t.undecoded_style_fields.is_empty() && t.undecoded_paragraph_fields.is_empty(),
            "{name}: {:?} {:?}",
            t.undecoded_style_fields,
            t.undecoded_paragraph_fields
        );
    }
}
