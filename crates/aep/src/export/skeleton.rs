//! The layer, item and project skeletons the exporter fills, built in code (aep-format.md 3.1-3.4):
//! only the property groups the exporter writes into, the default records it edits, and the small
//! fixed records each layer carries. After Effects adds the groups that are left out (read-back
//! e1-sparse).

use crate::Chunk;
use crate::export::synth::{KeyTemplate, NO_NAME, RecSpec, class, tdbs, tdmn, tdsn};

/// A group: its match name and a `tdgp` with the state `state` (`tdsb`), no user name, `kids` and
/// the closing `ADBE Group End`.
pub(crate) fn group(name: &str, state: u32, kids: Vec<Chunk<'static>>) -> [Chunk<'static>; 2] {
    let mut body = vec![Chunk::data(*b"tdsb", state.to_be_bytes().to_vec()), tdsn(NO_NAME)];
    body.extend(kids);
    body.push(tdmn("ADBE Group End"));
    [tdmn(name), Chunk::list(*b"tdgp", body)]
}

/// The seven records that follow a layer or item block (`fvdv` 3, the others zero; the same in
/// every sample, meaning not decoded).
pub(crate) fn f_records() -> Vec<Chunk<'static>> {
    vec![
        Chunk::data(*b"fvdv", 3u32.to_be_bytes().to_vec()),
        Chunk::data(*b"fiop", vec![0]),
        Chunk::data(*b"ftts", vec![0; 4]),
        Chunk::data(*b"foac", vec![0]),
        Chunk::data(*b"fiac", vec![0]),
        Chunk::data(*b"fipc", vec![0; 2]),
        Chunk::data(*b"fifl", vec![0; 4]),
    ]
}

/// The layer styles in the order After Effects keeps their switch groups (950).
const STYLE_ORDER: [&str; 10] =
    ["dropShadow", "innerShadow", "outerGlow", "innerGlow", "bevelEmboss", "chromeFX", "solidFill", "gradientFill", "patternFill", "frameFX"];

/// The empty `ADBE Layer Styles` group (state 3), Blending Options with the advanced group, and
/// the switch group of every style (state 2: not applied), for `styles::put_styles` to fill.
pub(crate) fn layer_styles() -> [Chunk<'static>; 2] {
    let mut kids: Vec<Chunk<'static>> = group("ADBE Blend Options Group", 3, group("ADBE Adv Blend Group", 1, Vec::new()).into()).into();
    for s in STYLE_ORDER {
        kids.extend(group(&format!("{s}/enabled"), 2, Vec::new()));
    }
    group("ADBE Layer Styles", 3, kids)
}

/// The inert Time Remapping record every layer has (`tdsb` 3, value 0, the maximum written per
/// layer: aep-format.md 3.4).
fn time_remap() -> Chunk<'static> {
    let spec = RecSpec { dims: 1, template: KeyTemplate::Plain, class: class::LAYER_SCALAR, limits: Some((0.0, 0.0)), byte5: None };
    let mut rec = tdbs(&spec, false, 25_600);
    if let Some(state) = rec.children_mut().iter_mut().find(|c| c.is(b"tdsb")) {
        *state = Chunk::data(*b"tdsb", 3u32.to_be_bytes().to_vec());
    }
    rec
}

/// The block of an AV layer (solid, footage, precomposition): `Layr` (a zero `ldta` the exporter
/// fills with the decoded fields, read-back t-zldta; the name; the property tree with Time
/// Remapping, an empty Transform group and the layer styles), `Ewst` and the f-records.
pub(crate) fn av_layer() -> Vec<Chunk<'static>> {
    let mut root = vec![Chunk::data(*b"tdsb", 1u32.to_be_bytes().to_vec()), tdsn(""), tdmn("ADBE Time Remapping"), time_remap()];
    root.extend(group("ADBE Transform Group", 1, crate::export::tree_spec::build(crate::export::tree_defs::AV_TRANSFORM)));
    root.extend(layer_styles());
    root.push(tdmn("ADBE Group End"));
    let layr = Chunk::list(
        *b"Layr",
        vec![Chunk::data(*b"ldta", vec![0; 164]), Chunk::data(*b"Utf8", Vec::new()), Chunk::list(*b"tdgp", root), crate::export::base::guides()],
    );
    let mut out = vec![layr, Chunk::list(*b"Ewst", Vec::new())];
    // Two sets of f-records follow every layer (000).
    out.extend(f_records());
    out.extend(f_records());
    out
}

/// The block of a layer whose property tree is `nodes` (cameras, lights, shapes: `tree_defs`),
/// with a zero `ldta`, the guides, `Ewst` and the f-records.
pub(crate) fn layer_block(nodes: &[crate::export::tree_spec::Node]) -> Vec<Chunk<'static>> {
    let mut root = vec![Chunk::data(*b"tdsb", 1u32.to_be_bytes().to_vec()), tdsn("")];
    root.extend(crate::export::tree_spec::build(nodes));
    root.push(tdmn("ADBE Group End"));
    let layr = Chunk::list(
        *b"Layr",
        vec![Chunk::data(*b"ldta", vec![0; 164]), Chunk::data(*b"Utf8", Vec::new()), Chunk::list(*b"tdgp", root), crate::export::base::guides()],
    );
    let mut out = vec![layr, Chunk::list(*b"Ewst", Vec::new())];
    // Two sets of f-records follow every layer (000).
    out.extend(f_records());
    out.extend(f_records());
    out
}

/// The Source Text record (`btds/tdbs`, 110): one component, class `0x00010004`, a 4-byte zero
/// `cdat`.
const SOURCE_TEXT: crate::export::tree_spec::Rec = crate::export::tree_spec::Rec {
    state: 1,
    label: NO_NAME,
    dims: 1,
    template: 0,
    class: class::EFFECT_CHOICE,
    byte5: 0x01,
    header: &[],
    cdat_len: 4,
    cdat: &[],
    key_size: 0,
    limits: None,
    tdpi: None,
};

/// The block of a text layer (110, 245): `ADBE Text Properties` with the text document (the
/// Source Text record, the document `doc` as `LIST btdk`, and `btgu` with the document's id and
/// a zero one), the empty Path and More Options groups; the inert Transform records and the layer
/// styles; a zero `ldta`, the guides, `Ewst` and the f-records.
pub(crate) fn text_layer(doc: &[u8]) -> Vec<Chunk<'static>> {
    use crate::export::tree_spec::{Node, build};
    let btdk = Chunk { id: *b"LIST", body: crate::Body::OpaqueList { list_type: *b"btdk", data: doc.to_vec().into() }, pad: None };
    // The record without its match name (`build` gives the pair).
    let mut btds_kids: Vec<Chunk<'static>> = build(&[Node::R("ADBE Text Document", SOURCE_TEXT)]).into_iter().filter(|c| c.is_list(b"tdbs")).collect();
    btds_kids.push(btdk);
    let btgu = Chunk::list(*b"btgu", vec![Chunk::data(*b"pgui", vec![0; 16]), Chunk::data(*b"pgui", vec![0; 16])]);
    let mut props = vec![tdmn("ADBE Text Document"), Chunk::list(*b"btds", btds_kids), btgu];
    props.extend(group("ADBE Text Path Options", 1, Vec::new()));
    props.extend(group("ADBE Text More Options", 1, Vec::new()));
    let mut root = vec![Chunk::data(*b"tdsb", 1u32.to_be_bytes().to_vec()), tdsn("")];
    root.extend(group("ADBE Text Properties", 1, props));
    root.extend(group("ADBE Transform Group", 1, crate::export::tree_spec::build(crate::export::tree_defs::AV_TRANSFORM)));
    root.extend(layer_styles());
    root.push(tdmn("ADBE Group End"));
    let layr = Chunk::list(
        *b"Layr",
        vec![Chunk::data(*b"ldta", vec![0; 164]), Chunk::data(*b"Utf8", Vec::new()), Chunk::list(*b"tdgp", root), crate::export::base::guides()],
    );
    let mut out = vec![layr, Chunk::list(*b"Ewst", Vec::new())];
    out.extend(f_records());
    out.extend(f_records());
    out
}

/// `tdmn` + node pairs of `nodes`, by match name.
pub(crate) fn pairs(nodes: &[crate::export::tree_spec::Node]) -> Vec<(String, [Chunk<'static>; 2])> {
    let mut out = Vec::new();
    let mut it = crate::export::tree_spec::build(nodes).into_iter();
    while let (Some(m), Some(n)) = (it.next(), it.next()) {
        let name = String::from_utf8_lossy(m.bytes().unwrap_or_default().split(|b| *b == 0).next().unwrap_or_default()).into_owned();
        out.push((name, [m, n]));
    }
    out
}

/// The marker group (`tdmn ADBE Marker` + `LIST mrst`, aep-format.md 3.11) for
/// `markers::build`: the marker property as a keyed one-component record (class `0x00010004`,
/// 16-byte keys, 249) and `mrky` with one `Nmrd` whose header (`NmHd`) and strings are written
/// per marker.
pub(crate) fn marker_group() -> Vec<Chunk<'static>> {
    let spec = RecSpec { dims: 1, template: KeyTemplate::Plain, class: class::EFFECT_CHOICE, limits: None, byte5: None };
    let record = tdbs(&spec, true, 25_600);
    let nmrd = Chunk::list(*b"Nmrd", vec![Chunk::data(*b"NmHd", vec![0; 20])]);
    vec![tdmn("ADBE Marker"), Chunk::list(*b"mrst", vec![record, Chunk::list(*b"mrky", vec![nmrd])])]
}
