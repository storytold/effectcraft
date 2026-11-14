//! A small committed text project (`tests/fixtures/text-synthetic.aep`), so the text animator
//! Stroke Opacity and the Source Text expression are checked without the external corpus, on
//! import and through an export and a second import (#3170).
//!
//! The project was made in After Effects 2026 with the working space set to None: one composition
//! `C` (64x48, 25 fps, 2 s) with two text layers. `T` holds `Hi` (Arial, 20 px, a blue 2 px
//! stroke), the Source Text expression `text.sourceText + "!"` (enabled), an animator `Stroke`
//! with Stroke Opacity 40 % and an animator `Keyed` with Stroke Opacity keyed 0 % at 0 s and
//! 100 % at 1 s. `Off` holds `Off` and the Source Text expression `"X"`, disabled. Nothing After
//! Effects wrote was changed.

use effectcraft_aep::import::{ImportOptions, ImportResult, LossCategory, import};
use effectcraft_aep::{AepFile, Body, Chunk, parse, write};
use effectcraft_keyframe::Value;
use effectcraft_project::{Expression, Layer, Project, PropGroup};
use effectcraft_time::{TICKS_PER_SECOND, Tick};

use crate::import::helpers::comp;

const TEXT_SYNTHETIC: &[u8] = include_bytes!("../../fixtures/text-synthetic.aep");

/// Imports `bytes` with losses allowed.
fn run(bytes: &[u8]) -> (Project, ImportResult) {
    let mut project = Project::default();
    let none = |_: &str| false;
    let r = import(&mut project, bytes, &ImportOptions { allow_losses: true, ..ImportOptions::new(&none) }).expect("the project imports");
    (project, r)
}

/// The loss details of `category`.
fn losses(r: &ImportResult, category: LossCategory) -> Vec<String> {
    r.report.losses.iter().filter(|l| l.category == category).flat_map(|l| l.details.clone()).collect()
}

/// The layer `name` of composition `C`.
fn layer<'p>(p: &'p Project, r: &ImportResult, name: &str) -> &'p Layer {
    let (_, c) = comp(p, r, "C");
    c.layers.iter().find(|l| l.name == name).unwrap_or_else(|| panic!("text layer {name}"))
}

/// The text animator `name` of `l`.
fn animator<'l>(l: &'l Layer, name: &str) -> &'l PropGroup {
    l.props.group("text/animators").expect("animators").groups().find(|a| a.name == name).unwrap_or_else(|| panic!("animator {name}"))
}

/// The Source Text of `l`: its stored text and its expression.
fn source_text(l: &Layer) -> (String, Option<Expression>) {
    let p = l.props.prop("text/sourceText").expect("Source Text");
    let Value::Text(doc) = &p.value else { panic!("{:?}", p.value) };
    (doc.text.clone(), p.expr.clone())
}

/// Stroke Opacity 40 % and its keys 0 % / 100 % arrive in percent, without a loss.
#[test]
fn text_animator_stroke_opacity_is_imported_static_and_keyed() {
    let (p, r) = run(TEXT_SYNTHETIC);
    let text = losses(&r, LossCategory::TextLayers);
    assert!(!text.iter().any(|d| d.contains("Stroke Opacity")), "{text:?}");
    let t = layer(&p, &r, "T");
    // Stored as After Effects shows it: percent.
    let st = animator(t, "Stroke").prop("properties/strokeOpacity").expect("strokeOpacity");
    assert_eq!((st.value.clone(), st.keys.len()), (Value::Scalar(40.0), 0));
    let keyed = animator(t, "Keyed").prop("properties/strokeOpacity").expect("strokeOpacity");
    let keys: Vec<_> = keyed.keys.iter().map(|k| (k.time, k.value.clone())).collect();
    assert_eq!(keys, vec![(Tick::ZERO, Value::Scalar(0.0)), (Tick(TICKS_PER_SECOND), Value::Scalar(100.0))]);
}

/// Both Source Text expressions arrive with their enable switch, next to the stored text.
#[test]
fn the_source_text_expression_is_imported_enabled_and_disabled() {
    let (p, r) = run(TEXT_SYNTHETIC);
    let text = losses(&r, LossCategory::TextLayers);
    assert!(!text.iter().any(|d| d.contains("Source Text expression")), "{text:?}");
    // The stored text stays the Source Text value; the expression comes next to it.
    assert_eq!(source_text(layer(&p, &r, "T")), ("Hi".to_string(), Some(Expression { text: "text.sourceText + \"!\"".into(), enabled: true })));
    assert_eq!(source_text(layer(&p, &r, "Off")), ("Off".to_string(), Some(Expression { text: "\"X\"".into(), enabled: false })));
    assert_eq!(r.report.imported.expressions, 2);
}

/// Calls `f` on every chunk below `chunks`, depth-first.
fn visit(chunks: &mut [Chunk<'_>], f: &mut dyn FnMut(&mut Chunk<'_>)) {
    for c in chunks {
        f(c);
        visit(c.children_mut(), f);
    }
}

/// Export and a second import keep the Source Text expressions and Stroke Opacity, unreported.
#[test]
fn stroke_opacity_and_source_text_expressions_survive_export() {
    let (p, r0) = run(TEXT_SYNTHETIC);
    let out = effectcraft_aep::export::export(&p, &effectcraft_aep::export::ExportOptions { allow_losses: true }).expect("export");
    let text: Vec<String> =
        out.report.losses.iter().filter(|l| l.category == effectcraft_aep::export::LossCategory::TextLayers).flat_map(|l| l.details.clone()).collect();
    assert!(!text.iter().any(|d| d.contains("Stroke Opacity") || d.contains("Source Text expression")), "{text:?}");
    assert_eq!(out.report.exported.expressions, 2);
    let (q, r) = run(&out.bytes);
    for name in ["T", "Off"] {
        assert_eq!(source_text(layer(&q, &r, name)), source_text(layer(&p, &r0, name)), "{name}");
    }
    let t = layer(&q, &r, "T");
    assert_eq!(animator(t, "Stroke").prop("properties/strokeOpacity").map(|p| p.value.clone()), Some(Value::Scalar(40.0)));
    let keys: Vec<_> = animator(t, "Keyed").prop("properties/strokeOpacity").expect("strokeOpacity").keys.iter().map(|k| (k.time, k.value.clone())).collect();
    assert_eq!(keys, vec![(Tick::ZERO, Value::Scalar(0.0)), (Tick(TICKS_PER_SECOND), Value::Scalar(100.0))]);
}

/// Guard: an expression whose `tdb4` flags give no state stays reported instead of imported.
#[test]
fn a_source_text_expression_with_unreadable_flags_stays_reported() {
    // `tdb4` byte 120 is 1 on a property with an expression; with 0 the expression's state is
    // unknown, so it is reported instead of imported.
    let mut file: AepFile<'_> = parse(TEXT_SYNTHETIC).expect("parse");
    visit(&mut file.chunks, &mut |c| {
        if !c.is_list(b"btds") {
            return;
        }
        for tdbs in c.children_mut().iter_mut().filter(|c| c.is_list(b"tdbs")) {
            for tdb4 in tdbs.children_mut().iter_mut().filter(|c| c.is(b"tdb4")) {
                if let Body::Data(d) = &mut tdb4.body
                    && let Some(b) = d.to_mut().get_mut(120)
                {
                    *b = 0;
                }
            }
        }
    });
    let (p, r) = run(&write(&file).expect("write"));
    let text = losses(&r, LossCategory::TextLayers);
    assert_eq!(text.iter().filter(|d| d.contains("the Source Text expression is not imported")).count(), 2, "{text:?}");
    assert_eq!(source_text(layer(&p, &r, "T")), ("Hi".to_string(), None));
}
