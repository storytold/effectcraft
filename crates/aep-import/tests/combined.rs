//! Masks, effects and text together on one layer, a common setup (an adjustment layer
//! with a Gradient Ramp or Tint and a mask, a text layer with an effect), and the same through the
//! `.aepx` reader.
//!
//! No fixture has both, so the files are built here from the fixtures: the `ADBE Mask Parade` or
//! `ADBE Effect Parade` group of one fixture's layer is grafted into the layer of another (all of
//! them are one change against the same base project, so the parades fit). The expected values are
//! those of importing each donor fixture on its own.

mod common;

use common::{comp, fixture_bytes};
use effectcraft_aep::{AepFile, Chunk, parse, parse_any, write};
use effectcraft_aep_import::{ImportOptions, ImportResult, import};
use effectcraft_keyframe::Value;
use effectcraft_project::{GroupKind, Layer, Project, PropGroup};

const MASK_PARADE: &[u8] = b"ADBE Mask Parade";
const EFFECT_PARADE: &[u8] = b"ADBE Effect Parade";

/// Whether `c` is the `tdmn` that names `name`.
fn names(c: &Chunk<'_>, name: &[u8]) -> bool {
    c.is(b"tdmn") && c.bytes().is_some_and(|b| b.strip_prefix(name).is_some_and(|rest| rest.first() == Some(&0)))
}

/// The root property group (`LIST tdgp`) of every layer (`LIST Layr`) under `chunks`.
fn layer_roots<'c, 'a>(chunks: &'c mut [Chunk<'a>], out: &mut Vec<&'c mut Chunk<'a>>) {
    for c in chunks {
        if c.is_list(b"Layr") {
            if let Some(root) = c.children_mut().iter_mut().find(|k| k.is_list(b"tdgp")) {
                out.push(root);
            }
        } else {
            layer_roots(c.children_mut(), out);
        }
    }
}

/// The `tdmn` naming `name` in the first layer's root group of `file`, and the group after it.
fn parade<'a>(file: &mut AepFile<'a>, name: &[u8]) -> Vec<Chunk<'a>> {
    let mut roots = Vec::new();
    layer_roots(&mut file.chunks, &mut roots);
    let root = roots.into_iter().next().expect("a layer");
    let kids = root.children();
    let at = kids.iter().position(|k| names(k, name)).unwrap_or_else(|| panic!("no {}", String::from_utf8_lossy(name)));
    kids[at..at + 2].to_vec()
}

/// Puts `pair` (a `tdmn` and its group) into the root group of every layer of `file`: in place of
/// the group of the same name, or after the mask parade (After Effects' order), or first.
fn graft<'a>(file: &mut AepFile<'a>, pair: &[Chunk<'a>]) {
    let name = pair[0].bytes().unwrap().split(|b| *b == 0).next().unwrap().to_vec();
    let mut roots = Vec::new();
    layer_roots(&mut file.chunks, &mut roots);
    assert!(!roots.is_empty());
    for root in roots {
        let effectcraft_aep::Body::List { children, .. } = &mut root.body else { panic!("tdgp is a list") };
        if let Some(at) = children.iter().position(|k| names(k, &name)) {
            children.splice(at..at + 2, pair.iter().cloned());
        } else {
            let at = children.iter().position(|k| names(k, MASK_PARADE)).map_or(0, |i| i + 2);
            children.splice(at..at, pair.iter().cloned());
        }
    }
}

/// `target` (read with the `.aep` or `.aepx` reader) with the named parades of the donors, written
/// as an `.aep`.
fn build(target: &str, donors: &[(&str, &[u8])]) -> Vec<u8> {
    let target_bytes = fixture_bytes(target);
    let donor_bytes: Vec<Vec<u8>> = donors.iter().map(|(d, _)| fixture_bytes(d)).collect();
    let mut file = parse_any(&target_bytes).unwrap();
    for ((_, name), bytes) in donors.iter().zip(&donor_bytes) {
        let mut donor = parse(bytes).unwrap();
        let pair = parade(&mut donor, name);
        graft(&mut file, &pair);
    }
    write(&file).unwrap()
}

/// Imports without allowing losses: every combination here is supported, so a loss fails.
fn import_lossless(bytes: &[u8]) -> (Project, ImportResult) {
    let mut project = Project::default();
    let none = |_: &str| false;
    let r = import(&mut project, bytes, &ImportOptions::new(&none)).unwrap_or_else(|e| panic!("{e}"));
    assert!(r.report.is_lossless(), "{:?}", r.report.losses);
    (project, r)
}

fn only_layer(project: &Project, r: &ImportResult) -> Layer {
    let (_, c) = comp(project, r, "C");
    assert_eq!(c.layers.len(), 1);
    c.layers[0].clone()
}

fn text_layer(project: &Project, r: &ImportResult) -> Layer {
    let (_, c) = comp(project, r, "C");
    c.layers.iter().find(|l| matches!(l.source, effectcraft_project::LayerSource::Text)).expect("a text layer").clone()
}

fn groups<'l>(l: &'l Layer, id: &str) -> Vec<&'l PropGroup> {
    l.props.sub(id).map(|g| g.groups().collect()).unwrap_or_default()
}

/// The values of a group's properties by id (ids of the project tree differ between imports).
fn values(g: &PropGroup) -> Vec<(String, Value, usize)> {
    g.props().map(|p| (p.match_id.clone(), p.value.clone(), p.keys.len())).collect()
}

fn mask_kind(g: &PropGroup) -> String {
    match &g.kind {
        GroupKind::Mask { mode, inverted, .. } => format!("{mode:?} {inverted}"),
        other => panic!("{other:?} is not a mask"),
    }
}

/// The layer `L` of a donor fixture imported on its own.
fn donor_layer(name: &str) -> Layer {
    let (p, r) = import_lossless(&fixture_bytes(name));
    only_layer(&p, &r)
}

/// Asserts that `l` carries the masks of `mask_donor` and the effects of `effect_donor`.
fn assert_same_masks_and_effects(l: &Layer, mask_donor: &str, effect_donor: &str) {
    let want = donor_layer(mask_donor);
    let (got_m, want_m) = (groups(l, "masks"), groups(&want, "masks"));
    assert_eq!(got_m.len(), want_m.len());
    for (g, w) in got_m.iter().zip(&want_m) {
        assert_eq!((g.name.as_str(), mask_kind(g), values(g)), (w.name.as_str(), mask_kind(w), values(w)));
    }
    let want = donor_layer(effect_donor);
    let (got_e, want_e) = (groups(l, "effects"), groups(&want, "effects"));
    assert_eq!(got_e.len(), want_e.len());
    for (g, w) in got_e.iter().zip(&want_e) {
        assert_eq!((g.match_id.as_str(), g.name.as_str(), values(g)), (w.match_id.as_str(), w.name.as_str(), values(w)));
    }
}

#[test]
fn a_mask_and_a_tint_on_one_solid() {
    // 091: a rectangle mask with feather, opacity and expansion; 356: Tint.
    let bytes = build("091-mask-rect-props.aep", &[("356-fx-tint-default.aep", EFFECT_PARADE)]);
    let (p, r) = import_lossless(&bytes);
    assert_eq!((r.report.imported.masks, r.report.imported.effects), (1, 1));
    assert_same_masks_and_effects(&only_layer(&p, &r), "091-mask-rect-props.aep", "356-fx-tint-default.aep");
}

#[test]
fn an_adjustment_layer_with_a_keyed_ramp_and_masks() {
    // 025: the layer is an adjustment layer; 328: an ellipse and a rectangle; 382: Gradient Ramp
    // with Start of Ramp keys.
    let bytes = build("025-layer-adjustment.aep", &[("328-mask-two-ellipse-rect.aep", MASK_PARADE), ("382-fx-ramp-start-2key.aep", EFFECT_PARADE)]);
    let (p, r) = import_lossless(&bytes);
    let i = &r.report.imported;
    assert_eq!((i.adjustment_layers, i.masks, i.effects), (1, 2, 1), "{i:?}");
    let l = only_layer(&p, &r);
    assert!(l.switches.adjustment);
    assert_same_masks_and_effects(&l, "328-mask-two-ellipse-rect.aep", "382-fx-ramp-start-2key.aep");
    // The ramp's keys arrive next to the masks.
    assert!(groups(&l, "effects")[0].props().any(|p| p.keys.len() == 2));
}

#[test]
fn the_grafting_order_does_not_matter() {
    // Grafting the effect first and the mask after it gives the same layer.
    let a = build("000-base.aep", &[("090-mask-rect.aep", MASK_PARADE), ("368-fx-ramp-default.aep", EFFECT_PARADE)]);
    let b = build("000-base.aep", &[("368-fx-ramp-default.aep", EFFECT_PARADE), ("090-mask-rect.aep", MASK_PARADE)]);
    for bytes in [a, b] {
        let (p, r) = import_lossless(&bytes);
        assert_same_masks_and_effects(&only_layer(&p, &r), "090-mask-rect.aep", "368-fx-ramp-default.aep");
    }
}

#[test]
fn a_text_layer_with_a_tint() {
    // 110 has the text layer above the solid `L`; both get the Tint.
    let (p, r) = import_lossless(&build("110-text-A.aep", &[("356-fx-tint-default.aep", EFFECT_PARADE)]));
    assert_eq!((r.report.imported.text_layers, r.report.imported.effects), (1, 2));
    let l = text_layer(&p, &r);
    let (wp, wr) = import_lossless(&fixture_bytes("110-text-A.aep"));
    let want = text_layer(&wp, &wr);
    assert_eq!(l.props.prop("text/sourceText").unwrap().value, want.props.prop("text/sourceText").unwrap().value);
    let fx = groups(&l, "effects");
    assert_eq!(fx.len(), 1);
    let tint = donor_layer("356-fx-tint-default.aep");
    assert_eq!(values(fx[0]), values(groups(&tint, "effects")[0]));
}

#[test]
fn the_aepx_tree_takes_masks_and_effects_like_the_aep() {
    // The same grafts on the chunk tree read from 110's `.aepx` and from its `.aep` import alike:
    // text, a mask and an effect.
    let donors: &[(&str, &[u8])] = &[("368-fx-ramp-default.aep", EFFECT_PARADE)];
    let from_aep = build("110-text-A.aep", donors);
    let from_aepx = build("110-text-A.aepx", donors);
    let (pa, ra) = import_lossless(&from_aep);
    let (px, rx) = import_lossless(&from_aepx);
    assert_eq!(ra, rx);
    assert_eq!(pa, px);

    // With a mask as well (on the text layer too: its box is read in layer pixels, 705).
    let donors: &[(&str, &[u8])] = &[("090-mask-rect.aep", MASK_PARADE), ("368-fx-ramp-default.aep", EFFECT_PARADE)];
    let none = |_: &str| false;
    let opts = ImportOptions { allow_losses: true, ..ImportOptions::new(&none) };
    let mut results = Vec::new();
    for target in ["110-text-A.aep", "110-text-A.aepx"] {
        let mut p = Project::default();
        let r = import(&mut p, &build(target, donors), &opts).unwrap();
        assert!(r.report.is_lossless(), "{target}: {:?}", r.report.losses);
        // Both layers get the mask and the ramp.
        assert_eq!((r.report.imported.text_layers, r.report.imported.masks, r.report.imported.effects), (1, 2, 2), "{target}");
        let fx = groups(&text_layer(&p, &r), "effects").len();
        assert_eq!(fx, 1, "{target}");
        results.push((p, r));
    }
    assert_eq!(results[0], results[1]);
}
