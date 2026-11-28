//! Essential Graphics: a composition's Essential Properties controls (`LIST CIFO`, `CIF2`, `CIF3`
//! in the composition item) and the values each of its precomp layers overrides (`ADBE Layer
//! Overrides`), as After Effects 26.5 stores them (aep-format.md 3.12c; the corpus file
//! `17_essential_graphics` and the labs `essential-props`, `essential-direct`, `essential-nested`).
//!
//! Written: controls on a layer's Anchor Point, Position, Scale, Rotation and Opacity and on a
//! text layer's Source Text, with every further property a control drives, and the overrides of
//! each instance. Other controls (effect parameters, shape properties, a nested instance's
//! Essential Properties, comments, groups, mirrors, Media Replacement) are reported and left out of
//! the lists, which then agree between the composition and its instances.

use std::collections::HashMap;

use effectcraft_keyframe::Value;
use effectcraft_project::essential::{EgControl, EgKind};
use effectcraft_project::{Comp, GroupKind, ItemId, Layer, LayerId, LayerSource, Project, PropGroup, Uid};

use crate::Chunk;
use crate::export::report::{Collector, LossCategory};
use crate::export::synth::{self, KeyTemplate, NO_NAME, RecSpec, class};
use crate::export::text::TextPlan;
use crate::export::tree::{R, put_f64, put_u8, put_u32, tpl};

/// The JSON `index` of a named (not indexed) step of a property path.
const NO_INDEX: u64 = 4_294_967_295;
/// The locale After Effects names next to each control name.
const LOCALE: &str = "en_US";
/// `CTov` of a text control and `btov` of its override (every sample: 7).
const TEXT_OVERRIDE_FLAGS: u32 = 7;
/// Controls of a composition that are written, at most.
const MAX_CONTROLS: usize = 4096;
/// Group nesting followed when the controls are listed.
const MAX_DEPTH: usize = 32;

/// What a written control drives.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Driven {
    Anchor,
    Position,
    Scale,
    Rotation,
    Opacity,
    SourceText,
}

impl Driven {
    fn of(path: &str) -> Option<Driven> {
        Some(match path {
            "transform/anchor" => Driven::Anchor,
            "transform/position" => Driven::Position,
            "transform/scale" => Driven::Scale,
            "transform/rotation" => Driven::Rotation,
            "transform/opacity" => Driven::Opacity,
            "text/sourceText" => Driven::SourceText,
            _ => return None,
        })
    }

    /// `CTyp`: 5 point, 9 scale, 3 angle, 2 slider, 6 text.
    fn ctyp(self) -> u32 {
        match self {
            Driven::Anchor | Driven::Position => 5,
            Driven::Scale => 9,
            Driven::Rotation => 3,
            Driven::Opacity => 2,
            Driven::SourceText => 6,
        }
    }

    /// The path from the layer, as the control's `CPrp` names it.
    fn path(self) -> [&'static str; 2] {
        match self {
            Driven::Anchor => ["ADBE Transform Group", "ADBE Anchor Point"],
            Driven::Position => ["ADBE Transform Group", "ADBE Position"],
            Driven::Scale => ["ADBE Transform Group", "ADBE Scale"],
            Driven::Rotation => ["ADBE Transform Group", "ADBE Rotate Z"],
            Driven::Opacity => ["ADBE Transform Group", "ADBE Opacity"],
            Driven::SourceText => ["ADBE Text Properties", "ADBE Text Document"],
        }
    }

    /// The match name of an instance's override record.
    fn override_match_name(self) -> &'static str {
        match self {
            Driven::SourceText => "ADBE EP Text Document",
            other => other.path()[1],
        }
    }
}

/// A property a control drives.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TargetPlan {
    pub layer: LayerId,
    pub driven: Driven,
}

/// A control's value, in the composition, in the control's terms.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum CtlValue {
    /// Slider (Opacity, percent) and angle (Rotation, degrees).
    Number(f64),
    /// Anchor Point and Position in layer / composition pixels (x, y).
    Point([f64; 2]),
    /// Scale in percent (x, y, z), whether its dimensions are locked, and 2 or 3 dimensions.
    Scale([f64; 3], bool, u32),
    /// The text, its font (PostScript name) and size.
    Text { text: String, font: String, size: f64 },
}

/// One written control.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct CtlPlan {
    pub id: u64,
    pub name: String,
    pub uuid: String,
    pub value: CtlValue,
    pub targets: Vec<TargetPlan>,
}

impl CtlPlan {
    fn driven(&self) -> Option<Driven> {
        self.targets.first().map(|t| t.driven)
    }
}

/// A composition's written Essential Graphics.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct EgPlan {
    pub name: String,
    pub controls: Vec<CtlPlan>,
}

/// One record of an instance's overrides, in the file's units.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct OvPlan {
    pub uuid: String,
    pub name: String,
    pub driven: Driven,
    /// Whether the instance overrides the value (`tdsb` 1) or follows the composition (0).
    pub overridden: bool,
    /// The value as the driven property stores it (opacity and scale as fractions, an anchor as a
    /// fraction of the source, pixels and degrees otherwise).
    pub value: Vec<f64>,
    /// Source Text: the document.
    pub text: Option<TextPlan>,
    /// Anchor Point: width / height of the driven layer's source (`tdb4` byte 24).
    pub aspect: f64,
}

/// A control's UUID, made from the composition and the control (the file only needs it to be
/// unique and the same in the composition and its instances).
fn uuid(comp: ItemId, control: u64) -> String {
    let c = comp.0 & 0xffff_ffff;
    let k = control & 0xffff_ffff_ffff;
    format!("{c:08x}-ec00-4000-8000-{k:012x}")
}

/// The controls of `eg` in panel order, groups opened (the group itself is reported).
fn flat<'a>(controls: &'a [EgControl], depth: usize, out: &mut Vec<&'a EgControl>, lost: &mut Vec<String>) {
    if depth > MAX_DEPTH {
        lost.push("a group nested too deeply".to_string());
        return;
    }
    for c in controls {
        match &c.kind {
            EgKind::Group { children } => {
                lost.push(format!("'{}': the panel group is not written (its controls are)", c.name));
                flat(children, depth + 1, out, lost);
            }
            _ => out.push(c),
        }
    }
}

/// The source size the anchor of `layer` is a fraction of, and whether it is stored in pixels
/// (text and shape layers).
fn anchor_unit(project: &Project, layer: &Layer) -> ((f64, f64), bool) {
    let size = match &layer.source {
        LayerSource::Text | LayerSource::Shape => return ((1.0, 1.0), true),
        LayerSource::Comp { item } => project.comp(*item).map(|c| (f64::from(c.width), f64::from(c.height))),
        LayerSource::Solid { item } | LayerSource::Footage { item } => match project.item(*item).map(|i| &i.kind) {
            Some(effectcraft_project::ItemKind::Solid(s)) => Some((f64::from(s.width), f64::from(s.height))),
            Some(effectcraft_project::ItemKind::Footage(f)) => Some((f64::from(f.width), f64::from(f.height))),
            _ => None,
        },
        // A null's source is 100 x 100 (R10).
        LayerSource::Null => Some((100.0, 100.0)),
        _ => None,
    };
    let (w, h) = size.unwrap_or((1.0, 1.0));
    ((w.max(1.0), h.max(1.0)), false)
}

/// The control's value in the composition, from the property it drives.
fn ctl_value(driven: Driven, v: &Value, layer: &Layer) -> Option<CtlValue> {
    let c = v.components();
    let num = |i: usize| c.get(i).copied().filter(|x| x.is_finite());
    Some(match driven {
        Driven::Opacity | Driven::Rotation => CtlValue::Number(num(0)?),
        Driven::Anchor | Driven::Position => CtlValue::Point([num(0)?, num(1)?]),
        Driven::Scale => {
            let s = [num(0)?, num(1)?, num(2).unwrap_or(100.0)];
            CtlValue::Scale(s, s[0] == s[1], if layer.is_3d() { 3 } else { 2 })
        }
        Driven::SourceText => {
            let Value::Text(doc) = v else { return None };
            CtlValue::Text { text: doc.text.replace("\r\n", "\r").replace('\n', "\r"), font: doc.font.clone(), size: doc.size }
        }
    })
}

/// The written controls of composition `comp_id` and what is left out of them.
pub(crate) fn plan_controls(comp_id: ItemId, comp: &Comp, exported: &dyn Fn(LayerId) -> bool) -> (Option<EgPlan>, Vec<String>) {
    let Some(eg) = &comp.essential else { return (None, Vec::new()) };
    let mut list = Vec::new();
    let mut lost = Vec::new();
    flat(&eg.controls, 0, &mut list, &mut lost);
    let mut controls = Vec::new();
    for c in list {
        if controls.len() >= MAX_CONTROLS {
            lost.push(format!("'{}': more than {MAX_CONTROLS} controls", c.name));
            continue;
        }
        // A Font control sets only the font of an instance's text; written as a Source Text
        // control its override would replace the whole text.
        if c.as_type == Some(effectcraft_project::essential::ControlType::Font) {
            lost.push(format!("'{}': a Font control is not written", c.name));
            continue;
        }
        let EgKind::Property { .. } = &c.kind else {
            let what = match &c.kind {
                EgKind::Mirror { .. } => "a mirrored control",
                EgKind::Media { .. } => "Media Replacement",
                EgKind::Comment { .. } => "a comment",
                _ => "this control",
            };
            lost.push(format!("'{}': {what} is not written", c.name));
            continue;
        };
        let mut targets = Vec::new();
        let mut value = None;
        let mut why = None;
        for (i, (lid, uid)) in c.kind.targets().into_iter().enumerate() {
            let Some(layer) = comp.layers.iter().find(|l| l.id == lid) else {
                why = Some("its layer is missing");
                break;
            };
            if !exported(lid) {
                why = Some("its layer is not exported");
                break;
            }
            let Some(driven) = layer.props.match_path_of(uid).as_deref().and_then(Driven::of) else {
                why = Some("it drives a property not written yet (only Transform and Source Text are)");
                break;
            };
            if let Some(first) = targets.first().map(|t: &TargetPlan| t.driven)
                && first.ctyp() != driven.ctyp()
            {
                why = Some("it drives properties of different kinds");
                break;
            }
            if i == 0 {
                value = layer.props.find(uid).and_then(|p| ctl_value(driven, p.keys.first().map(|k| &k.value).unwrap_or(&p.value), layer));
                if value.is_none() {
                    why = Some("its value cannot be read");
                    break;
                }
            }
            targets.push(TargetPlan { layer: lid, driven });
        }
        match (why, value) {
            (None, Some(value)) if !targets.is_empty() => controls.push(CtlPlan { id: c.id, name: c.name.clone(), uuid: uuid(comp_id, c.id), value, targets }),
            (why, _) => lost.push(format!("'{}': not written: {}", c.name, why.unwrap_or("it drives no property"))),
        }
    }
    (Some(EgPlan { name: eg.name.clone(), controls }), lost)
}

/// The instance property `m` in an instance's Essential Properties group or one of its panel
/// sub-groups (depth-limited: groups come from the file).
fn instance_prop<'a>(g: &'a PropGroup, m: &str, depth: usize) -> Option<&'a effectcraft_project::Property> {
    if depth > MAX_DEPTH {
        return None;
    }
    g.get(m).or_else(|| g.groups().find_map(|sub| instance_prop(sub, m, depth + 1)))
}

/// The overrides of precomp layer `layer`, whose source `inner` has the written controls `eg`.
pub(crate) fn plan_overrides(project: &Project, inner: &Comp, eg: &EgPlan, layer: &Layer, what: &str, col: &mut Collector) -> Vec<OvPlan> {
    let group = layer.props.sub(effectcraft_project::essential::GROUP);
    let overridden: Vec<Uid> = match group.map(|g| &g.kind) {
        Some(GroupKind::Essential { overridden }) => overridden.clone(),
        _ => Vec::new(),
    };
    let mut out = Vec::with_capacity(eg.controls.len());
    for c in &eg.controls {
        let Some(t) = c.targets.first() else { continue };
        let Some(src_layer) = inner.layers.iter().find(|l| l.id == t.layer) else { continue };
        // Controls inside a panel group sit in a sub-group of the instance's group.
        let instance = group.and_then(|g| instance_prop(g, &effectcraft_project::essential::match_id(c.id), 0));
        let is_over = instance.is_some_and(|p| overridden.contains(&p.uid));
        if let Some(p) = instance.filter(|_| is_over) {
            if !p.keys.is_empty() {
                col.lose(
                    LossCategory::CompositionOptions,
                    format!("{what}: Essential Property '{}': keyframes are not written (the first key's value is)", c.name),
                );
            }
            if p.has_expression() {
                col.lose(LossCategory::CompositionOptions, format!("{what}: Essential Property '{}': the expression is not written", c.name));
            }
        }
        // The comp's value, unless the instance overrides it.
        let comp_value = src_layer.props.match_path_of_driven(t.driven).map(|p| p.keys.first().map(|k| k.value.clone()).unwrap_or_else(|| p.value.clone()));
        let value = match instance.filter(|_| is_over) {
            Some(p) => Some(p.keys.first().map(|k| k.value.clone()).unwrap_or_else(|| p.value.clone())),
            None => comp_value,
        };
        let Some(value) = value else { continue };
        if is_over && value.components().iter().any(|x| !x.is_finite()) {
            col.lose(
                LossCategory::CompositionOptions,
                format!("{what}: Essential Property '{}': the override is not a number, the default is written", c.name),
            );
        }
        let ((w, h), pixels) = anchor_unit(project, src_layer);
        let c3 = value.components();
        let get = |i: usize, d: f64| c3.get(i).copied().filter(|x| x.is_finite()).unwrap_or(d);
        let (numbers, text) = match t.driven {
            Driven::Opacity => (vec![get(0, 100.0) / 100.0], None),
            Driven::Rotation => (vec![get(0, 0.0)], None),
            Driven::Scale => (vec![get(0, 100.0) / 100.0, get(1, 100.0) / 100.0, get(2, 100.0) / 100.0], None),
            Driven::Position => (vec![get(0, 0.0), get(1, 0.0), get(2, 0.0)], None),
            Driven::Anchor if pixels => (vec![get(0, 0.0), get(1, 0.0), get(2, 0.0)], None),
            Driven::Anchor => (vec![get(0, 0.0) / w, get(1, 0.0) / h, get(2, 0.0)], None),
            Driven::SourceText => {
                let Value::Text(doc) = &value else { continue };
                (Vec::new(), Some(crate::export::text::plan_doc(doc, what, col)))
            }
        };
        out.push(OvPlan {
            uuid: c.uuid.clone(),
            name: c.name.clone(),
            driven: t.driven,
            overridden: is_over,
            value: numbers,
            text,
            aspect: if pixels { 1.0 } else { w / h },
        });
    }
    out
}

/// The property a driven kind names on a layer of the inner composition.
trait DrivenProp {
    fn match_path_of_driven(&self, d: Driven) -> Option<&effectcraft_project::Property>;
}

impl DrivenProp for effectcraft_project::PropGroup {
    fn match_path_of_driven(&self, d: Driven) -> Option<&effectcraft_project::Property> {
        self.prop(match d {
            Driven::Anchor => "transform/anchor",
            Driven::Position => "transform/position",
            Driven::Scale => "transform/scale",
            Driven::Rotation => "transform/rotation",
            Driven::Opacity => "transform/opacity",
            Driven::SourceText => "text/sourceText",
        })
    }
}

// --- Writing -------------------------------------------------------------------------------

fn data(id: &[u8; 4], bytes: Vec<u8>) -> Chunk<'static> {
    Chunk::data(*id, bytes)
}

fn utf8(text: &str) -> Chunk<'static> {
    data(b"Utf8", text.as_bytes().to_vec())
}

fn f64s(v: &[f64]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_be_bytes()).collect()
}

/// `LIST CpS2` and `LIST CapS` naming a control or a template.
fn names(name: &str) -> [Chunk<'static>; 2] {
    let one = data(b"CsCt", 1u32.to_le_bytes().to_vec());
    [Chunk::list(*b"CpS2", vec![one.clone(), utf8(name), utf8(LOCALE)]), Chunk::list(*b"CapS", vec![one, data(b"CapL", vec![0; 4]), utf8(name)])]
}

/// The JSON path of a driven property.
fn json_path(d: Driven) -> String {
    let [a, b] = d.path();
    format!("{{\"0\":{{\"index\":{NO_INDEX},\"matchName\":\"{a}\"}},\"1\":{{\"index\":{NO_INDEX},\"matchName\":\"{b}\"}}}}")
}

/// A JSON string literal.
fn json_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn control(c: &CtlPlan, comp_aep_id: u32, layer_ids: &HashMap<LayerId, u32>) -> R<Chunk<'static>> {
    let driven = c.driven().ok_or_else(|| tpl("control without a property"))?;
    let mut kids: Vec<Chunk<'static>> = names(&c.name).into();
    kids.push(utf8(&c.uuid));
    kids.push(data(b"CTyp", driven.ctyp().to_be_bytes().to_vec()));
    match &c.value {
        CtlValue::Number(v) => {
            kids.push(data(b"CVal", f64s(&[*v])));
            kids.push(data(b"CDef", f64s(&[*v])));
            if driven == Driven::Opacity {
                kids.push(data(b"Smin", f64s(&[0.0])));
                kids.push(data(b"Smax", f64s(&[100.0])));
            }
        }
        CtlValue::Point(p) => {
            kids.push(data(b"CVal", f64s(p)));
            kids.push(data(b"CDef", f64s(p)));
        }
        CtlValue::Scale(s, locked, dims) => {
            let v = [s[0], s[1], s[2], 0.0];
            kids.push(data(b"CVal", f64s(&v)));
            kids.push(data(b"CDef", f64s(&v)));
            kids.push(data(b"CSLk", vec![u8::from(*locked)]));
            kids.push(data(b"CDim", dims.to_be_bytes().to_vec()));
        }
        CtlValue::Text { text, font, size } => {
            kids.push(utf8(text));
            kids.push(utf8(text));
            kids.push(data(b"CFEd", vec![0]));
            kids.push(data(b"CSEd", vec![0]));
            kids.push(data(b"CFEd", vec![0]));
            let size = if size.is_finite() { *size } else { 12.0 };
            kids.push(utf8(&format!(
                "{{\"capPropFontEdit\":false,\"capPropFontFauxStyleEdit\":false,\"capPropFontSizeEdit\":false,\"fontEditValue\":{},\"fontFSAllCapsValue\":false,\"fontFSBoldValue\":false,\"fontFSItalicValue\":false,\"fontFSSmallCapsValue\":false,\"fontSizeEditValue\":{}}}",
                json_str(font),
                size
            )));
            kids.push(utf8("{\"compId\":-1,\"isEnabled\":false,\"layerId\":-1}"));
            kids.push(data(b"CTov", TEXT_OVERRIDE_FLAGS.to_le_bytes().to_vec()));
        }
    }
    let count = u32::try_from(c.targets.len()).map_err(|_| tpl("too many properties"))?;
    kids.push(data(b"CprC", count.to_be_bytes().to_vec()));
    for t in &c.targets {
        let lid = *layer_ids.get(&t.layer).ok_or_else(|| tpl("control on a layer without an id"))?;
        kids.push(Chunk::list(
            *b"CPrp",
            vec![data(b"CCId", comp_aep_id.to_be_bytes().to_vec()), data(b"CLId", lid.to_be_bytes().to_vec()), utf8(&json_path(t.driven))],
        ));
    }
    Ok(Chunk::list(*b"CCtl", kids))
}

/// `CIFO` lists only the control types older versions know (checkbox, slider, colour, text): the
/// samples leave out angle, point and scale controls there and keep the others in panel order.
fn in_cifo(d: Driven) -> bool {
    matches!(d.ctyp(), 1 | 2 | 4 | 6)
}

/// The three control lists of a composition, `CIFO`, `CIF2` and `CIF3`.
pub(crate) fn control_lists(eg: &EgPlan, comp_aep_id: u32, layer_ids: &HashMap<LayerId, u32>) -> R<Vec<Chunk<'static>>> {
    let mut out = Vec::with_capacity(3);
    for (id, old) in [(*b"CIFO", true), (*b"CIF2", false), (*b"CIF3", false)] {
        let chosen: Vec<&CtlPlan> = eg.controls.iter().filter(|c| !old || c.driven().is_some_and(in_cifo)).collect();
        let mut kids: Vec<Chunk<'static>> = names(&eg.name).into();
        kids.push(data(b"CPTm", 1u64.to_be_bytes().to_vec()));
        kids.push(data(b"CROI", vec![0; 8]));
        let count = u32::try_from(chosen.len()).map_err(|_| tpl("too many controls"))?;
        kids.push(data(b"CcCt", count.to_be_bytes().to_vec()));
        for c in chosen {
            kids.push(control(c, comp_aep_id, layer_ids)?);
        }
        out.push(Chunk::list(id, kids));
    }
    Ok(out)
}

/// The record spec of a driven Transform property (as the layer's own records, [`super::template`]).
fn spec(d: Driven) -> RecSpec {
    match d {
        Driven::Anchor | Driven::Position => RecSpec { dims: 3, template: KeyTemplate::Spatial(3), class: class::SPATIAL, limits: None, byte5: None },
        Driven::Scale => RecSpec { dims: 3, template: KeyTemplate::Plain, class: class::LAYER_SCALAR, limits: Some((0.0, 0.0)), byte5: None },
        Driven::Rotation => RecSpec { dims: 1, template: KeyTemplate::Plain, class: class::LAYER_SCALAR, limits: None, byte5: None },
        Driven::Opacity | Driven::SourceText => {
            RecSpec { dims: 1, template: KeyTemplate::Plain, class: class::LAYER_SCALAR, limits: Some((0.0, 100.0)), byte5: None }
        }
    }
}

/// `tdb4` bytes 59 and 60 of an override record (every sample: 8, 9 on Transform properties;
/// 0, 8 on Source Text). Meaning not decoded.
fn override_marks(d: Driven) -> [u8; 2] {
    if d == Driven::SourceText { [0, 8] } else { [8, 9] }
}

/// One instance record: `tdmn` and its node (and, for Source Text, `btgu` and `btov`).
fn override_record(o: &OvPlan, text_proto: &Chunk<'static>, time_scale: u32, guid: [u8; 16]) -> R<Vec<Chunk<'static>>> {
    let state = u32::from(o.overridden);
    let marks = override_marks(o.driven);
    if o.driven == Driven::SourceText {
        let tp = o.text.as_ref().ok_or_else(|| tpl("text override without a document"))?;
        // The Source Text record and document of the text layer prototype, renamed.
        let mut btds = text_proto.clone();
        crate::export::text::put_doc(&mut btds, tp, [0; 16])?;
        if let Some(tdbs) = crate::export::tree::kids_mut(&mut btds)?.iter_mut().find(|k| k.is_list(b"tdbs")) {
            for k in crate::export::tree::kids_mut(tdbs)?.iter_mut() {
                if k.is(b"tdsb") {
                    *k = data(b"tdsb", state.to_be_bytes().to_vec());
                } else if k.is(b"tdsn") {
                    *k = synth::tdsn(&o.name);
                } else if k.is(b"tdb4") {
                    let d = crate::export::tree::data_mut(k)?;
                    put_u32(d, 12, time_scale)?;
                    put_u8(d, 59, marks[0])?;
                    put_u8(d, 60, marks[1])?;
                }
            }
        }
        let btgu = Chunk::list(*b"btgu", vec![data(b"pgui", guid.to_vec()), data(b"pgui", vec![0; 16])]);
        return Ok(vec![synth::tdmn(o.driven.override_match_name()), btds, btgu, data(b"btov", TEXT_OVERRIDE_FLAGS.to_le_bytes().to_vec())]);
    }
    let s = spec(o.driven);
    let mut tdbs = synth::tdbs(&s, false, time_scale);
    for k in crate::export::tree::kids_mut(&mut tdbs)?.iter_mut() {
        if k.is(b"tdsb") {
            *k = data(b"tdsb", state.to_be_bytes().to_vec());
        } else if k.is(b"tdsn") {
            *k = synth::tdsn(&o.name);
        } else if k.is(b"tdb4") {
            let d = crate::export::tree::data_mut(k)?;
            put_u8(d, 59, marks[0])?;
            put_u8(d, 60, marks[1])?;
            match o.driven {
                // As the layer's own Position record (`tree_defs`: header 76, 77).
                Driven::Position => {
                    put_u8(d, 76, 1)?;
                    put_u8(d, 77, 1)?;
                }
                Driven::Anchor => put_f64(d, 24, o.aspect)?,
                _ => {}
            }
        } else if k.is(b"cdat") {
            let d = crate::export::tree::data_mut(k)?;
            for (i, v) in o.value.iter().enumerate() {
                put_f64(d, i * 8, *v)?;
            }
        }
    }
    Ok(vec![synth::tdmn(o.driven.override_match_name()), tdbs])
}

/// Inserts `ADBE Layer Overrides` (`tdmn`, `LIST OvG2`, `LIST tdgp`) into a precomp layer's root
/// group, before `ADBE Layer Sets` or the group's end, as After Effects places it.
pub(crate) fn put_overrides(layr: &mut Chunk<'static>, ovs: &[OvPlan], text_proto: &Chunk<'static>, time_scale: u32, guids: &[[u8; 16]]) -> R<()> {
    let count = u32::try_from(ovs.len()).map_err(|_| tpl("too many overrides"))?;
    let mut ovg2 = vec![data(b"CprC", count.to_le_bytes().to_vec())];
    let mut records = vec![data(b"tdsb", 1u32.to_be_bytes().to_vec()), synth::tdsn(NO_NAME)];
    for (i, o) in ovs.iter().enumerate() {
        ovg2.push(Chunk::list(*b"CPrp", vec![utf8(&o.uuid)]));
        records.extend(override_record(o, text_proto, time_scale, guids.get(i).copied().unwrap_or([0; 16]))?);
    }
    records.push(synth::tdmn(synth::GROUP_END));
    let group = [synth::tdmn("ADBE Layer Overrides"), Chunk::list(*b"OvG2", ovg2), Chunk::list(*b"tdgp", records)];
    let root = crate::export::tree::list_mut(layr, b"tdgp")?;
    let kids = crate::export::tree::kids_mut(root)?;
    let at = kids
        .iter()
        .position(|c| synth::is_tdmn(c, "ADBE Layer Sets"))
        .or_else(|| kids.iter().rposition(|c| synth::is_tdmn(c, synth::GROUP_END)))
        .unwrap_or(kids.len());
    kids.splice(at..at, group);
    Ok(())
}

/// Inserts the control lists into a composition item, before its guides (`GdV2`, `Gide`) as After
/// Effects places them.
pub(crate) fn put_control_lists(item: &mut Chunk<'static>, lists: Vec<Chunk<'static>>) -> R<()> {
    let kids = crate::export::tree::kids_mut(item)?;
    let at = kids.iter().position(|c| c.is_list(b"GdV2") || c.is_list(b"Gide")).unwrap_or(kids.len());
    kids.splice(at..at, lists);
    Ok(())
}

/// The Source Text node (`LIST btds`) of a text layer block, at most 32 levels down.
pub(crate) fn text_proto(block: &[Chunk<'static>]) -> Option<Chunk<'static>> {
    fn find(c: &Chunk<'static>, depth: usize) -> Option<Chunk<'static>> {
        if depth > MAX_DEPTH {
            return None;
        }
        if c.is_list(b"btds") {
            return Some(c.clone());
        }
        c.children().iter().find_map(|k| find(k, depth + 1))
    }
    block.iter().find_map(|c| find(c, 0))
}
