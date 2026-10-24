//! Layer styles: the inverse of the importer, through the shared table
//! [`effectcraft_aep::style_map`].
//!
//! Every layer of the template already holds the `ADBE Layer Styles` group with Blending Options
//! and the ten switch groups at rest (950). A style is applied by setting its switch group's
//! `tdsb` to 1 and Layer Styles and Blending Options to 1 (951-960); a parameter that differs
//! from After Effects' default gets a record, cloned from the record After Effects wrote for that
//! parameter (`template/styles.aep`, from 977) with the value replaced, in After Effects'
//! property order (970). Keys on a scalar take the keyed form of 976 (`tdb4` bytes 5, 68 and 79
//! from it, plain 48-byte key records); an expression is stored as on an effect parameter
//! (`tdb4@120` 1, `@119` the disabled switch, the text before `tdum`).
//!
//! A style switched off keeps its records with its switch group's `tdsb` 0 (988). Not written,
//! reported: the whole Layer Styles group switched off (not sampled), gradients that differ from the default (`GCst` is not decoded), keys on
//! colours, points, popups and checkboxes (not sampled), Knockout (After Effects has none).

use std::collections::HashMap;

use effectcraft_aep::style_map::{self as sm, StyleConv, StyleParam};
use effectcraft_aep::{AepFile, Chunk};
use effectcraft_keyframe::{Interp, Value};
use effectcraft_project::build::Ids;
use effectcraft_project::styles::{self as st, STYLE_BLEND_MODES};
use effectcraft_project::{Comp, Layer, PropGroup, Property};

use crate::plan::{CompPlan, EaseSlot, KeyPlan};
use crate::report::{Collector, LossCategory};
use crate::tree::{R, child_data, kids_mut, put_f64, put_u8, put_u32, tpl};

/// A record's value.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum RecValue {
    Static(Vec<f64>),
    Keys(Vec<KeyPlan>),
}

/// One parameter record to write.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Record {
    /// The full match name (`dropShadow/opacity`, `ADBE Global Angle2`).
    pub name: String,
    pub value: RecValue,
    /// Expression text and its enabled switch.
    pub expr: Option<(String, bool)>,
}

/// The layer styles of one layer.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct StylesPlan {
    /// Applied styles (switch-group prefix) with their records and their switch, in menu order.
    pub styles: Vec<(&'static str, Vec<Record>, bool)>,
    /// Global Light Angle / Altitude records (Blending Options).
    pub global: Vec<Record>,
    /// Advanced Blending records.
    pub adv: Vec<Record>,
}

fn sane(v: f64) -> Option<f64> {
    (v.is_finite() && v.abs() <= crate::plan::LIMIT).then_some(v)
}

/// An EffectCraft value in the file's units.
pub(crate) fn stored(conv: StyleConv, v: &Value) -> Option<Vec<f64>> {
    let out = match (conv, v) {
        (StyleConv::Number, Value::Scalar(x)) => vec![sane(*x)?],
        (StyleConv::Color, Value::Color(c)) => vec![255.0, sane(c[0] * 255.0)?, sane(c[1] * 255.0)?, sane(c[2] * 255.0)?],
        (StyleConv::Mode, Value::Enum(i)) => {
            let name = STYLE_BLEND_MODES.get(usize::try_from(*i).ok()?)?.0;
            vec![f64::from(sm::mode_code(name)?)]
        }
        (StyleConv::Popup(map), Value::Enum(i)) => vec![f64::from(sm::popup_code(map, *i)?)],
        (StyleConv::Checkbox, Value::Bool(b)) => vec![if *b { 1.0 } else { 0.0 }],
        (StyleConv::Vec2, Value::Vec2(p)) => vec![sane(p[0])?, sane(p[1])?],
        _ => return None,
    };
    Some(out)
}

fn same(a: &[f64], b: &[f64]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| (x - y).abs() <= 1e-9 * x.abs().max(1.0))
}

/// The record of one parameter, `None` when it stays at After Effects' default (no record).
fn plan_record(spec: &StyleParam, full: String, prop: &Property, cp: &CompPlan, what: &str, col: &mut Collector) -> Option<Record> {
    let here = format!("{what}: {full}");
    let expr = prop.expr.as_ref().map(|e| (e.text.clone(), e.enabled));
    if !prop.keys.is_empty() {
        if spec.conv == StyleConv::Number {
            let keys = plan_number_keys(prop, cp, &here, col);
            if !keys.is_empty() {
                return Some(Record { name: full, value: RecValue::Keys(keys), expr });
            }
        } else {
            col.lose(LossCategory::LayerStyles, format!("{here}: keys are not written (not sampled for this kind), the current value is"));
        }
    }
    let Some(value) = stored(spec.conv, &prop.value) else {
        col.lose(LossCategory::InvalidValues, format!("{here}: a value that cannot be stored, After Effects' default written"));
        return None;
    };
    if same(&value, spec.default) && expr.is_none() {
        return None;
    }
    Some(Record { name: full, value: RecValue::Static(value), expr })
}

/// The keys of a scalar in the layer's time base: linear, hold and bezier sides with the ease in
/// stored units per second (the plain template of 976).
fn plan_number_keys(prop: &Property, cp: &CompPlan, what: &str, col: &mut Collector) -> Vec<KeyPlan> {
    let mut timed: Vec<(i64, &effectcraft_keyframe::Keyframe)> = Vec::with_capacity(prop.keys.len());
    for k in &prop.keys {
        let (u, inexact) = cp.tb.units(k.time);
        if inexact {
            col.lose(LossCategory::RoundedTimes, format!("{what}: key at {:.4} s moved to the time base", k.time.seconds()));
        }
        timed.push((u, k));
    }
    timed.sort_by_key(|(u, _)| *u);
    let before = timed.len();
    timed.dedup_by_key(|(u, _)| *u);
    if timed.len() != before {
        col.lose(LossCategory::ApproximatedKeyframes, format!("{what}: keys at the same time were merged"));
    }
    let mut out = Vec::with_capacity(timed.len());
    for (u, k) in timed {
        let (Ok(time), Some(v)) = (
            i32::try_from(u),
            match &k.value {
                Value::Scalar(x) => sane(*x),
                _ => None,
            },
        ) else {
            col.lose(LossCategory::InvalidValues, format!("{what}: a key that cannot be stored, key dropped"));
            continue;
        };
        let side = |i: Interp| -> u8 {
            match i {
                Interp::Linear => 1,
                Interp::Bezier => 2,
                Interp::Hold => 3,
            }
        };
        let (in_i, out_i) = (side(k.in_interp), side(k.out_interp));
        let ease = |eases: &[effectcraft_keyframe::Ease], on: bool| -> Vec<EaseSlot> {
            if !on {
                return vec![EaseSlot::default()];
            }
            let e = eases.first().copied().unwrap_or(effectcraft_keyframe::Ease { speed: 0.0, influence: 1.0 / 3.0 });
            let influence = if e.influence.is_finite() { e.influence.clamp(0.001, 1.0) } else { 1.0 / 3.0 };
            vec![EaseSlot { speed: sane(e.speed).unwrap_or(0.0), influence }]
        };
        out.push(KeyPlan {
            time,
            value: vec![v],
            in_interp: in_i,
            out_interp: out_i,
            in_ease: ease(&k.in_ease, in_i == 2),
            out_ease: ease(&k.out_ease, out_i == 2),
            spatial_in: [0.0; 3],
            spatial_out: [0.0; 3],
            spatial_auto: true,
            roving: false,
            label: k.label.min(16),
        });
        if k.roving {
            col.lose(LossCategory::ApproximatedKeyframes, format!("{what}: a roving key is written as a normal key"));
        }
    }
    out
}

/// EffectCraft's default of a style parameter (to tell an edited gradient).
fn model_default(style: &str, id: &str) -> Option<Value> {
    let mut next = 1;
    let g = st::style(&mut Ids(&mut next), style)?;
    g.get(id).map(|p| p.value.clone())
}

/// The layer styles of `layer`; `None` when it has none applied.
pub(crate) fn plan_styles(layer: &Layer, comp: &Comp, cp: &CompPlan, what: &str, col: &mut Collector) -> Option<StylesPlan> {
    let ls = layer.layer_styles()?;
    let mut plan = StylesPlan::default();
    let groups: Vec<&PropGroup> = ls.groups().filter(|g| g.match_id != st::BLENDING).collect();
    if groups.is_empty() {
        return None;
    }
    if !layer.can_have_styles() {
        col.lose(LossCategory::LayerStyles, format!("{what}: layer styles on a layer that takes none, not written"));
        return None;
    }
    if !ls.enabled {
        col.lose(LossCategory::LayerStyles, format!("{what}: Layer Styles switched off (the stored switch is not sampled), not written"));
        return None;
    }
    for g in groups {
        let Some(spec) = sm::by_ec(&g.match_id) else {
            col.lose(LossCategory::LayerStyles, format!("{what}: style {} has no After Effects counterpart", g.name));
            continue;
        };
        let mut recs = Vec::new();
        for prm in spec.params {
            let Some(prop) = g.get(prm.ec) else { continue };
            let full = spec.param_name(prm);
            if prm.conv == StyleConv::Gradient {
                let edited = !prop.keys.is_empty() || prop.expr.is_some() || model_default(spec.ec, prm.ec).is_some_and(|d| d != prop.value);
                if edited {
                    col.lose(LossCategory::LayerStyles, format!("{what}: {full} (gradient colours are not decoded; After Effects' default gradient)"));
                }
                continue;
            }
            if let Some(r) = plan_record(prm, full, prop, cp, what, col) {
                recs.push(r);
            }
        }
        plan.styles.push((spec.ae, recs, g.enabled));
    }
    if plan.styles.is_empty() {
        return None;
    }
    // Global Light: the composition's, on every styled layer.
    const ANGLE: &[f64] = &[st::DEFAULT_ANGLE];
    const ALTITUDE: &[f64] = &[st::DEFAULT_ALTITUDE];
    for (name, p, default) in [(sm::GLOBAL_ANGLE, &comp.global_light.angle, ANGLE), (sm::GLOBAL_ALTITUDE, &comp.global_light.altitude, ALTITUDE)] {
        let spec = StyleParam { ae: name, ec: name, conv: StyleConv::Number, default };
        if let Some(r) = plan_record(&spec, name.to_string(), p, cp, what, col) {
            plan.global.push(r);
        }
    }
    if let Some(adv) = ls.sub(st::BLENDING).and_then(|b| b.sub("advancedBlending")) {
        for prm in sm::ADVANCED_BLENDING {
            if let Some(p) = adv.get(prm.ec)
                && let Some(r) = plan_record(prm, prm.ae.to_string(), p, cp, what, col)
            {
                plan.adv.push(r);
            }
        }
        if adv.get("knockout").is_some_and(|p| p.value != Value::Enum(0) || !p.keys.is_empty()) {
            col.lose(LossCategory::LayerStyles, format!("{what}: Knockout (After Effects has none)"));
        }
    }
    col.exported.layer_styles += plan.styles.len();
    Some(plan)
}

// ---------------------------------------------------------------------------------------------
// Emit.

/// The style record prototypes (`template/styles.aep`): full match name -> (`tdmn`, `tdbs`), and
/// the keyed scalar record.
pub(crate) struct StyleProtos {
    records: HashMap<String, (Chunk<'static>, Chunk<'static>)>,
    keyed: Chunk<'static>,
}

fn tdmn_name(c: &Chunk<'_>) -> Option<String> {
    let d = c.bytes()?;
    Some(d.iter().take_while(|b| **b != 0).map(|b| char::from(*b)).collect())
}

impl StyleProtos {
    pub(crate) fn from_file(f: &AepFile<'static>) -> R<StyleProtos> {
        let mut records = HashMap::new();
        let list = f.list(b"stpr").ok_or_else(|| tpl("style prototypes missing"))?;
        let k = list.children();
        let mut i = 0;
        while i + 1 < k.len() {
            if let (Some(m), Some(n)) = (k.get(i), k.get(i + 1))
                && m.is(b"tdmn")
            {
                records.insert(tdmn_name(m).unwrap_or_default(), (m.clone(), n.clone()));
            }
            i += 2;
        }
        let keyed = f.list(b"stky").and_then(|l| l.children().get(1).cloned()).ok_or_else(|| tpl("keyed style prototype missing"))?;
        let all = sm::STYLES.iter().all(|s| s.params.iter().all(|p| p.conv == StyleConv::Gradient || records.contains_key(&s.param_name(p))))
            && sm::ADVANCED_BLENDING.iter().all(|p| records.contains_key(p.ae))
            && records.contains_key(sm::GLOBAL_ANGLE)
            && records.contains_key(sm::GLOBAL_ALTITUDE);
        if !all {
            return Err(tpl("style prototypes incomplete"));
        }
        Ok(StyleProtos { records, keyed })
    }

    /// The `tdmn` + `tdbs` of one record.
    fn build(&self, r: &Record) -> R<[Chunk<'static>; 2]> {
        let (m, proto) = self.records.get(&r.name).ok_or_else(|| tpl("style record prototype missing"))?;
        let mut rec = proto.clone();
        match &r.value {
            RecValue::Static(v) => {
                let cdat = child_data(&mut rec, b"cdat")?;
                cdat.iter_mut().for_each(|b| *b = 0);
                for (i, x) in v.iter().enumerate() {
                    put_f64(cdat, i * 8, *x)?;
                }
            }
            RecValue::Keys(keys) => {
                let ktdb4 = self.keyed.child(b"tdb4").and_then(|c| c.bytes()).ok_or_else(|| tpl("keyed style prototype without tdb4"))?;
                {
                    let tdb4 = child_data(&mut rec, b"tdb4")?;
                    for at in [5usize, 68, 79] {
                        put_u8(tdb4, at, *ktdb4.get(at).ok_or_else(|| tpl("short tdb4"))?)?;
                    }
                }
                let mut list = self.keyed.child_list(b"list").cloned().ok_or_else(|| tpl("keyed style prototype without keys"))?;
                let n = u32::try_from(keys.len()).map_err(|_| tpl("too many keys"))?;
                let size = 48usize;
                {
                    let lhd3 = child_data(&mut list, b"lhd3")?;
                    put_u32(lhd3, 8, n)?;
                    put_u32(lhd3, 12, n.div_ceil(4))?;
                    put_u32(lhd3, 16, 48)?;
                    put_u32(lhd3, 28, n.div_ceil(4).saturating_mul(4))?;
                }
                let mut ldat = Vec::with_capacity(size * keys.len());
                for k in keys {
                    ldat.extend(crate::emit::plain_record(1, k, size)?);
                }
                {
                    let lk = kids_mut(&mut list)?;
                    match lk.iter_mut().find(|c| c.is(b"ldat")) {
                        Some(c) => *c = Chunk::data(*b"ldat", ldat),
                        None => lk.push(Chunk::data(*b"ldat", ldat)),
                    }
                }
                let kids = kids_mut(&mut rec)?;
                let at = kids.iter().position(|c| c.is(b"cdat")).ok_or_else(|| tpl("style record without cdat"))?;
                if let Some(slot) = kids.get_mut(at) {
                    *slot = list;
                }
            }
        }
        if let Some((text, enabled)) = &r.expr {
            {
                let tdb4 = child_data(&mut rec, b"tdb4")?;
                put_u8(tdb4, 120, 1)?;
                put_u8(tdb4, 119, u8::from(!*enabled))?;
            }
            let kids = kids_mut(&mut rec)?;
            let at = kids.iter().position(|c| c.is(b"tdum")).unwrap_or(kids.len());
            kids.insert(at, Chunk::data(*b"Utf8", text.as_bytes().to_vec()));
        }
        Ok([m.clone(), rec])
    }
}

/// The node after the `tdmn` named `name` among `kids`.
fn named_mut<'a>(kids: &'a mut [Chunk<'static>], name: &str) -> R<&'a mut Chunk<'static>> {
    let i = kids.iter().position(|c| c.is(b"tdmn") && tdmn_name(c).as_deref() == Some(name)).ok_or_else(|| tpl("layer styles group incomplete"))?;
    kids.get_mut(i + 1).ok_or_else(|| tpl("layer styles group incomplete"))
}

fn set_flags(g: &mut Chunk<'static>, flags: u32) -> R<()> {
    put_u32(child_data(g, b"tdsb")?, 0, flags)
}

/// Inserts `recs` before the `tdmn` named `before` (or before `ADBE Group End`).
fn insert_records(protos: &StyleProtos, g: &mut Chunk<'static>, recs: &[Record], before: &str) -> R<()> {
    let mut built = Vec::with_capacity(recs.len() * 2);
    for r in recs {
        built.extend(protos.build(r)?);
    }
    let kids = kids_mut(g)?;
    let at = kids.iter().position(|c| c.is(b"tdmn") && tdmn_name(c).as_deref() == Some(before)).ok_or_else(|| tpl("layer styles group incomplete"))?;
    kids.splice(at..at, built);
    Ok(())
}

/// Applies `plan` to the layer's `ADBE Layer Styles` group.
pub(crate) fn put_styles(protos: &StyleProtos, layr: &mut Chunk<'static>, plan: &StylesPlan) -> R<()> {
    let root = crate::tree::list_mut(layr, b"tdgp")?;
    let ls = named_mut(kids_mut(root)?, sm::GROUP)?;
    set_flags(ls, sm::FLAGS_ON)?;
    {
        let bo = named_mut(kids_mut(ls)?, sm::BLEND_OPTIONS)?;
        set_flags(bo, sm::FLAGS_ON)?;
        insert_records(protos, bo, &plan.global, sm::ADV_BLEND)?;
        let adv = named_mut(kids_mut(bo)?, sm::ADV_BLEND)?;
        insert_records(protos, adv, &plan.adv, "ADBE Group End")?;
    }
    for (prefix, recs, on) in &plan.styles {
        let g = named_mut(kids_mut(ls)?, &format!("{prefix}/enabled"))?;
        set_flags(g, if *on { sm::FLAGS_ON } else { sm::FLAGS_OFF })?;
        insert_records(protos, g, recs, "ADBE Group End")?;
    }
    Ok(())
}
