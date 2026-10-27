//! Separate Dimensions and the property pick-whip (AE-style reference expressions).

use effectcraft_keyframe::{Ease, Interp, Keyframe, Value as KV};
use effectcraft_project::{Comp, Layer, Node, ParamUi, Property, Uid};
use effectcraft_time::{TICKS_PER_SECOND, Tick};
use serde_json::{Value, json};

use super::prop::prop_ref;
use super::{CommandSpec, b_p, bad, has_layers, layer_mut, layer_p, str_p};
use crate::{EngineError, Result, Session, cmd};

const AXES: [(&str, &str); 3] = [("positionX", "X Position"), ("positionY", "Y Position"), ("positionZ", "Z Position")];

/// Per-dimension scalar keys from a (spatial) position key list: same times and interpolation;
/// Bezier sides get the per-dimension velocity the combined property had at the key.
fn split_keys(keys: &[Keyframe], spatial: bool, d: usize) -> Vec<Keyframe> {
    let h = Tick(TICKS_PER_SECOND / 2000);
    keys.iter()
        .map(|k| {
            let mut s = Keyframe::new(k.time, KV::Scalar(k.value.components().get(d).copied().unwrap_or(0.0)));
            s.in_interp = k.in_interp;
            s.out_interp = k.out_interp;
            s.auto_bezier = k.auto_bezier;
            s.continuous = k.continuous;
            s.roving = false;
            let vel = |t: Tick| effectcraft_keyframe::velocity(keys, t, spatial).get(d).copied().unwrap_or(0.0);
            if k.in_interp == Interp::Bezier {
                let inf = k.in_ease.first().map(|e| e.influence).unwrap_or(1.0 / 3.0);
                s.in_ease = vec![Ease { speed: vel(k.time - h), influence: inf }];
            }
            if k.out_interp == Interp::Bezier {
                let inf = k.out_ease.first().map(|e| e.influence).unwrap_or(1.0 / 3.0);
                s.out_ease = vec![Ease { speed: vel(k.time + h), influence: inf }];
            }
            s
        })
        .collect()
}

fn separate(s: &mut Session, p: &Value) -> Result<Value> {
    let (cid, lid) = layer_p(s, p, "prop.separateDimensions")?;
    let want = b_p(p, "value");
    let on = s.edit("Separate Dimensions", None, |proj, _| {
        let mut next = proj.next_id;
        let l = layer_mut(proj, cid, lid)?;
        let tr = l.props.sub_mut("transform").ok_or_else(|| bad("prop.separateDimensions", "this layer has no Transform"))?;
        let has = tr.get("positionX").is_some();
        let target = want.unwrap_or(!has);
        if target && !has {
            let pos = tr.get("position").ok_or_else(|| bad("prop.separateDimensions", "no Position"))?.clone();
            let at = tr.children.iter().position(|c| c.uid() == pos.uid).map(|i| i + 1).unwrap_or(tr.children.len());
            let comps = pos.value.components();
            for (d, (m, name)) in AXES.iter().enumerate().rev() {
                let mut pr = Property::new(next, m, name, KV::Scalar(comps.get(d).copied().unwrap_or(0.0))).with_ui(ParamUi::Number);
                next += 1;
                pr.three_d_only = d == 2;
                pr.keys = split_keys(&pos.keys, pos.spatial, d);
                tr.children.insert(at, Node::Prop(pr));
            }
        } else if !target && has {
            let parts: Vec<Property> = AXES.iter().filter_map(|(m, _)| tr.get(m).cloned()).collect();
            let mut times: Vec<Tick> = parts.iter().flat_map(|p| p.keys.iter().map(|k| k.time)).collect();
            times.sort();
            times.dedup();
            let at = |t: Tick| KV::Vec3([0, 1, 2].map(|d| parts.get(d).map(|p| p.value_at(t).as_f64()).unwrap_or(0.0)));
            let pos = tr.get_mut("position").ok_or_else(|| bad("prop.separateDimensions", "no Position"))?;
            pos.value = at(Tick::ZERO);
            pos.keys = times.iter().map(|t| Keyframe::new(*t, at(*t))).collect();
            if pos.keys.is_empty() {
                pos.value = KV::Vec3([0, 1, 2].map(|d| parts.get(d).map(|p| p.value.as_f64()).unwrap_or(0.0)));
            }
            tr.children.retain(|c| !AXES.iter().any(|(m, _)| c.match_id() == *m));
        }
        proj.next_id = next;
        Ok(target)
    })?;
    Ok(json!(on))
}

// ---------------------------------------------------------------- pick-whip

/// `X Position` → `xPosition`, `Mask Path` → `maskPath`.
fn camel(name: &str) -> String {
    let mut out = String::new();
    for (i, w) in name.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).enumerate() {
        let mut cs = w.chars();
        if let Some(f) = cs.next() {
            if i == 0 {
                out.extend(f.to_lowercase());
            } else {
                out.extend(f.to_uppercase());
            }
            out.push_str(cs.as_str());
        }
    }
    out
}

fn quote(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| format!("\"{s}\""))
}

/// The AE expression reference to property `uid` of `target` as seen from a property of `from`.
pub(crate) fn reference(comp: &Comp, from: &Layer, target: &Layer, uid: Uid, compact: bool) -> Option<String> {
    let chain = target.props.node_chain(uid)?;
    let mut parts: Vec<String> = vec![];
    let mut pending: Option<&str> = None;
    let mut in_effect = false;
    for n in chain {
        if let Some(f) = pending.take() {
            parts.push(format!("{f}({})", quote(n.name())));
            in_effect |= f == "effect";
            continue;
        }
        match (n, n.match_id()) {
            (Node::Group(_), "masks") => pending = Some("mask"),
            (Node::Group(_), "effects") => pending = Some("effect"),
            (Node::Group(_), "contents") => pending = Some("content"),
            (Node::Group(_), "transform") if !in_effect => parts.push("transform".into()),
            (Node::Group(_), "text") if !in_effect => parts.push("text".into()),
            (Node::Prop(_), "sourceText") => parts.push("sourceText".into()),
            _ if in_effect => parts.push(format!("({})", quote(n.name()))),
            _ => parts.push(camel(n.name())),
        }
    }
    let mut out = if from.id == target.id { String::new() } else { format!("thisComp.layer({}).", quote(&target.name)) };
    let _ = comp;
    if !compact {
        // Match names: language-independent, like After Effects with Compact English off.
        let chain = target.props.node_chain(uid)?;
        let mut out = if from.id == target.id { "thisLayer".to_string() } else { format!("thisComp.layer({})", quote(&target.name)) };
        for n in chain {
            out.push_str(&format!("({})", quote(n.match_id())));
        }
        return Some(out);
    }
    for (i, p) in parts.iter().enumerate() {
        if i > 0 && !p.starts_with('(') {
            out.push('.');
        }
        out.push_str(p);
    }
    Some(out)
}

/// Dimensions an expression sees (2D layers see 2D Position/Scale/Anchor).
fn dims(l: &Layer, p: &Property) -> usize {
    match &p.value {
        KV::Vec3(_) if p.shown_dims == 2 && !l.is_3d() => 2,
        v => v.components().len().max(1),
    }
}

fn pick_whip(s: &mut Session, p: &Value) -> Result<Value> {
    let (cid, lid, uid) = prop_ref(s, p, "prop.pickWhip")?;
    let t = p.get("target").ok_or_else(|| bad("prop.pickWhip", "missing `target` {layer, path|prop}"))?;
    let (_, tl, tu) = prop_ref(s, t, "prop.pickWhip")?;
    if (tl, tu) == (lid, uid) {
        return Err(bad("prop.pickWhip", "a property can't reference itself"));
    }
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?;
    let from = comp.layer(lid).ok_or(EngineError::NoComp)?;
    let target = comp.layer(tl).ok_or(EngineError::NoComp)?;
    let r = reference(comp, from, target, tu, s.prefs.general.expression_pick_whip_compact).ok_or_else(|| bad("prop.pickWhip", "no such target property"))?;
    let (me, them) = (from.props.find(uid).ok_or(EngineError::NoComp)?, target.props.find(tu).ok_or(EngineError::NoComp)?);
    let (dm, dt) = (dims(from, me), dims(target, them));
    // Dropped on one of the target's values: that dimension (`position[0]`), as in After Effects.
    let (r, dt) = match t.get("dimension").and_then(Value::as_u64) {
        Some(d) if d < dt as u64 => (format!("{r}[{d}]"), 1),
        Some(d) => return Err(bad("prop.pickWhip", format!("the target has {dt} dimension(s), no dimension {d}"))),
        None => (r, dt),
    };
    // While the expression is being edited the reference goes in at the cursor, replacing the
    // selected text (After Effects): `expression` is the text being edited, `range` the
    // selection [start, end] in characters.
    if let Some(base) = str_p(p, "expression") {
        let range: Option<Vec<u64>> = p.get("range").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_u64).collect());
        let n = base.chars().count() as u64;
        let (a, b) = match range.as_deref() {
            Some(&[a, b]) => (a.min(b).min(n), a.max(b).min(n)),
            None => (n, n),
            Some(_) => return Err(bad("prop.pickWhip", "`range` must be [start, end] (characters)")),
        };
        let head: String = base.chars().take(a as usize).collect();
        let tail: String = base.chars().skip(b as usize).collect();
        let text = format!("{head}{r}{tail}");
        s.execute("prop.setExpression", json!({"layer": lid.0, "prop": uid, "expression": text}))?;
        return Ok(json!(text));
    }
    let text = if dt == 1 && dm > 1 {
        format!("temp = {r};\n[{}]", vec!["temp"; dm].join(", "))
    } else if dt > 1 && dm == 1 {
        format!("{r}[0]")
    } else {
        r
    };
    s.execute("prop.setExpression", json!({"layer": lid.0, "prop": uid, "expression": text}))?;
    Ok(json!(text))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!("prop.separateDimensions", "Separate Dimensions", ["Animation"], None, "{layer?, value?}", has_layers, separate),
        cmd!(
            "prop.pickWhip",
            "Pick Whip (Link Property)",
            [],
            None,
            "{layer?, path|prop, target: {layer, path|prop, dimension?}, expression?, range?: [start, end]} → sets an AE reference expression (`dimension`: one of the target's values, `position[0]`; with `expression`: inserts it there, replacing the characters in `range`, default the end)",
            has_layers,
            pick_whip
        ),
    ]
}
