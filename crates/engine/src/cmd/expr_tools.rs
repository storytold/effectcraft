//! Expression tools: the error list behind the expression error bar, and the Expression
//! Language menu (the ⓕ button next to an expression), written from the public Expression
//! Language Reference.

use effectcraft_project::{Expression, ItemId, Layer, Property};
use effectcraft_render::EvalCtx;
use effectcraft_time::Tick;
use serde::Serialize;
use serde_json::{Value, json};

use super::{CommandSpec, comp_id, f_p};
use crate::{EngineError, Result, Session, query};

/// One failing expression.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ExprError {
    pub comp: u64,
    pub layer: u64,
    #[serde(rename = "layerIndex")]
    pub layer_index: usize,
    #[serde(rename = "layerName")]
    pub layer_name: String,
    pub prop: u64,
    /// Match path (`transform/opacity`).
    pub path: String,
    /// Property name path as shown in the timeline (`Transform/Opacity`).
    pub name: String,
    pub message: String,
    /// The expression is disabled (it failed to compile).
    pub disabled: bool,
}

impl ExprError {
    /// The error bar's text: `Error at line 1 in property 'Opacity' of layer 1 ('Solid') in comp 'Main'. …`
    pub fn describe(&self, comp_name: &str) -> String {
        let prop = self.name.rsplit('/').next().unwrap_or(&self.name);
        format!("{} — property '{}' of layer {} ('{}') in comp '{}'", self.message, prop, self.layer_index, self.layer_name, comp_name)
    }
}

/// Every failing expression of comp `cid` at comp time `t`: evaluation errors of enabled
/// expressions and the syntax errors that disabled others.
pub fn errors(s: &Session, cid: ItemId, t: Tick) -> Vec<ExprError> {
    let Some(comp) = s.project.comp(cid) else { return vec![] };
    let ctx = EvalCtx { footage: Some(s.footage.as_ref()), expr: s.expr.as_deref(), ..EvalCtx::new(&s.project, cid, comp, t) };
    errors_in(&ctx, s.expr_check, || true).unwrap_or_default()
}

/// Evaluate diagnostics against an immutable snapshot. Background callers can cancel between
/// properties; cancelled scans return no partial result. Includes hidden/non-solo layers and
/// syntax errors on disabled expressions, just like the explicit `expr.errors` query.
pub fn errors_in(ctx: &EvalCtx, check: Option<fn(&str) -> std::result::Result<(), String>>, mut keep_going: impl FnMut() -> bool) -> Option<Vec<ExprError>> {
    let mut out = vec![];
    for (i, l) in ctx.comp.layers.iter().enumerate() {
        if !keep_going() {
            return None;
        }
        let mut props: Vec<(&Property, &Expression)> = vec![];
        l.props.walk("", &mut |_, p| {
            if let Some(e) = p.expr.as_ref().filter(|e| !e.text.trim().is_empty()) {
                props.push((p, e));
            }
        });
        for (p, e) in props {
            if !keep_going() {
                return None;
            }
            let msg = if e.enabled {
                match &ctx.expr {
                    Some(h) => h.eval(ctx, l, p, &p.value_at(l.layer_time(ctx.time))).err(),
                    None => None,
                }
            } else {
                check.and_then(|check| check(&e.text).err())
            };
            if let Some(message) = msg {
                out.push(error_of(ctx.comp_id, i, l, p, message, !e.enabled));
            }
        }
    }
    keep_going().then_some(out)
}

fn error_of(cid: ItemId, i: usize, l: &Layer, p: &Property, message: String, disabled: bool) -> ExprError {
    ExprError {
        comp: cid.0,
        layer: l.id.0,
        layer_index: i + 1,
        layer_name: l.name.clone(),
        prop: p.uid,
        path: super::match_path_of(&l.props, p.uid).unwrap_or_default(),
        name: l.props.name_path_of(p.uid).unwrap_or_else(|| p.name.clone()),
        message,
        disabled,
    }
}

fn errors_cmd(s: &mut Session, p: &Value) -> Result<Value> {
    let cid = comp_id(s, p)?;
    let t = f_p(p, "time").map(Tick::from_seconds_f64).unwrap_or_else(|| s.time_of(cid));
    let name = s.project.item(cid).map(|i| i.name.clone()).ok_or(EngineError::NoComp)?;
    let list = errors(s, cid, t);
    let text: Vec<String> = list.iter().map(|e| e.describe(&name)).collect();
    Ok(json!({"comp": cid.0, "time": t.seconds(), "count": list.len(), "errors": list, "text": text}))
}

/// The Expression Language menu: (category, [(label, inserted text)]). Submenus are nested
/// with `▸` in the category name.
pub fn language_menu() -> Vec<(&'static str, Vec<(&'static str, &'static str)>)> {
    vec![
        (
            "Global",
            vec![
                ("comp(name)", "comp(\"name\")"),
                ("footage(name)", "footage(\"name\")"),
                ("thisComp", "thisComp"),
                ("thisLayer", "thisLayer"),
                ("thisProperty", "thisProperty"),
                ("thisProject", "thisProject"),
                ("time", "time"),
                ("colorDepth", "colorDepth"),
                ("posterizeTime(framesPerSecond)", "posterizeTime(framesPerSecond)"),
                ("value", "value"),
            ],
        ),
        (
            "Vector Math",
            vec![
                ("add(vec1, vec2)", "add(vec1, vec2)"),
                ("sub(vec1, vec2)", "sub(vec1, vec2)"),
                ("mul(vec, amount)", "mul(vec, amount)"),
                ("div(vec, amount)", "div(vec, amount)"),
                ("clamp(value, limit1, limit2)", "clamp(value, limit1, limit2)"),
                ("dot(vec1, vec2)", "dot(vec1, vec2)"),
                ("cross(vec1, vec2)", "cross(vec1, vec2)"),
                ("normalize(vec)", "normalize(vec)"),
                ("length(vec)", "length(vec)"),
                ("length(point1, point2)", "length(point1, point2)"),
                ("lookAt(fromPoint, atPoint)", "lookAt(fromPoint, atPoint)"),
            ],
        ),
        (
            "Random Numbers",
            vec![
                ("seedRandom(offset, timeless=false)", "seedRandom(offset, timeless=false)"),
                ("random()", "random()"),
                ("random(maxValOrArray)", "random(maxValOrArray)"),
                ("random(minValOrArray, maxValOrArray)", "random(minValOrArray, maxValOrArray)"),
                ("gaussRandom()", "gaussRandom()"),
                ("gaussRandom(maxValOrArray)", "gaussRandom(maxValOrArray)"),
                ("gaussRandom(minValOrArray, maxValOrArray)", "gaussRandom(minValOrArray, maxValOrArray)"),
                ("noise(valOrArray)", "noise(valOrArray)"),
            ],
        ),
        (
            "Interpolation",
            vec![
                ("linear(t, value1, value2)", "linear(t, value1, value2)"),
                ("linear(t, tMin, tMax, value1, value2)", "linear(t, tMin, tMax, value1, value2)"),
                ("ease(t, value1, value2)", "ease(t, value1, value2)"),
                ("ease(t, tMin, tMax, value1, value2)", "ease(t, tMin, tMax, value1, value2)"),
                ("easeIn(t, value1, value2)", "easeIn(t, value1, value2)"),
                ("easeIn(t, tMin, tMax, value1, value2)", "easeIn(t, tMin, tMax, value1, value2)"),
                ("easeOut(t, value1, value2)", "easeOut(t, value1, value2)"),
                ("easeOut(t, tMin, tMax, value1, value2)", "easeOut(t, tMin, tMax, value1, value2)"),
                ("bezier(t, value1, value2, [x1, y1, x2, y2])", "bezier(t, value1, value2, [x1, y1, x2, y2])"),
                ("bezier(t, tMin, tMax, value1, value2, [x1, y1, x2, y2])", "bezier(t, tMin, tMax, value1, value2, [x1, y1, x2, y2])"),
            ],
        ),
        (
            "Color Conversion",
            vec![
                ("rgbToHsl(rgbaArray)", "rgbToHsl(rgbaArray)"),
                ("hslToRgb(hslaArray)", "hslToRgb(hslaArray)"),
                ("hexToRgb(hexString)", "hexToRgb(hexString)"),
            ],
        ),
        ("Other Math", vec![("degreesToRadians(degrees)", "degreesToRadians(degrees)"), ("radiansToDegrees(radians)", "radiansToDegrees(radians)")]),
        (
            "JavaScript Math",
            vec![
                ("Math.cos(value)", "Math.cos(value)"),
                ("Math.sin(value)", "Math.sin(value)"),
                ("Math.atan2(y, x)", "Math.atan2(y, x)"),
                ("Math.sqrt(value)", "Math.sqrt(value)"),
                ("Math.abs(value)", "Math.abs(value)"),
                ("Math.round(value)", "Math.round(value)"),
                ("Math.floor(value)", "Math.floor(value)"),
                ("Math.min(value1, value2)", "Math.min(value1, value2)"),
                ("Math.max(value1, value2)", "Math.max(value1, value2)"),
                ("Math.PI", "Math.PI"),
            ],
        ),
        (
            "Comp",
            vec![
                ("layer(index)", "layer(index)"),
                ("layer(name)", "layer(\"name\")"),
                ("layer(otherLayer, relIndex)", "layer(otherLayer, relIndex)"),
                ("marker", "marker"),
                ("numLayers", "numLayers"),
                ("activeCamera", "activeCamera"),
                ("width", "width"),
                ("height", "height"),
                ("duration", "duration"),
                ("frameDuration", "frameDuration"),
                ("pixelAspect", "pixelAspect"),
                ("name", "name"),
                ("bgColor", "bgColor"),
                ("displayStartTime", "displayStartTime"),
            ],
        ),
        (
            "Footage",
            vec![
                ("width", "width"),
                ("height", "height"),
                ("duration", "duration"),
                ("frameDuration", "frameDuration"),
                ("pixelAspect", "pixelAspect"),
                ("name", "name"),
                ("sourceText", "sourceText"),
                ("sourceData", "sourceData"),
                ("dataValue(dataPath)", "dataValue(dataPath)"),
                ("dataKeyCount", "dataKeyCount"),
            ],
        ),
        (
            "Layer > Sub-objects",
            vec![
                ("source", "source"),
                ("effect(name)", "effect(\"name\")"),
                ("effect(index)", "effect(index)"),
                ("mask(name)", "mask(\"name\")"),
                ("mask(index)", "mask(index)"),
            ],
        ),
        (
            "Layer > General",
            vec![
                ("width", "width"),
                ("height", "height"),
                ("index", "index"),
                ("parent", "parent"),
                ("hasParent", "hasParent"),
                ("inPoint", "inPoint"),
                ("outPoint", "outPoint"),
                ("startTime", "startTime"),
                ("hasVideo", "hasVideo"),
                ("enabled", "enabled"),
                ("active", "active"),
            ],
        ),
        (
            "Layer > Properties",
            vec![
                ("anchorPoint", "anchorPoint"),
                ("position", "position"),
                ("scale", "scale"),
                ("rotation", "rotation"),
                ("opacity", "opacity"),
                ("marker", "marker"),
                ("name", "name"),
            ],
        ),
        (
            "Layer > Space Transforms",
            vec![
                ("toComp(point, t = time)", "toComp(point, t = time)"),
                ("fromComp(point, t = time)", "fromComp(point, t = time)"),
                ("toWorld(point, t = time)", "toWorld(point, t = time)"),
                ("fromWorld(point, t = time)", "fromWorld(point, t = time)"),
                ("toCompVec(vec, t = time)", "toCompVec(vec, t = time)"),
                ("fromCompVec(vec, t = time)", "fromCompVec(vec, t = time)"),
                ("toWorldVec(vec, t = time)", "toWorldVec(vec, t = time)"),
                ("fromWorldVec(vec, t = time)", "fromWorldVec(vec, t = time)"),
            ],
        ),
        (
            "Layer > Other",
            vec![
                ("sourceRectAtTime(t = time, includeExtents = false)", "sourceRectAtTime(t = time, includeExtents = false)"),
                ("sampleImage(point, radius = [.5, .5], postEffect = true, t = time)", "sampleImage(point, radius = [.5, .5], postEffect = true, t = time)"),
            ],
        ),
        (
            "Property",
            vec![
                ("value", "value"),
                ("valueAtTime(t)", "valueAtTime(t)"),
                ("velocity", "velocity"),
                ("velocityAtTime(t)", "velocityAtTime(t)"),
                ("speed", "speed"),
                ("speedAtTime(t)", "speedAtTime(t)"),
                ("wiggle(freq, amp, octaves = 1, amp_mult = .5, t = time)", "wiggle(freq, amp, octaves = 1, amp_mult = .5, t = time)"),
                ("temporalWiggle(freq, amp, octaves = 1, amp_mult = .5, t = time)", "temporalWiggle(freq, amp, octaves = 1, amp_mult = .5, t = time)"),
                ("smooth(width = .2, samples = 5, t = time)", "smooth(width = .2, samples = 5, t = time)"),
                ("loopIn(type = \"cycle\", numKeyframes = 0)", "loopIn(type = \"cycle\", numKeyframes = 0)"),
                ("loopOut(type = \"cycle\", numKeyframes = 0)", "loopOut(type = \"cycle\", numKeyframes = 0)"),
                ("loopInDuration(type = \"cycle\", duration = 0)", "loopInDuration(type = \"cycle\", duration = 0)"),
                ("loopOutDuration(type = \"cycle\", duration = 0)", "loopOutDuration(type = \"cycle\", duration = 0)"),
                ("key(index)", "key(index)"),
                ("key(markerName)", "key(markerName)"),
                ("nearestKey(t)", "nearestKey(t)"),
                ("numKeys", "numKeys"),
                ("name", "name"),
                ("propertyIndex", "propertyIndex"),
            ],
        ),
        (
            "Path Property",
            vec![
                ("points(t = time)", "points(t = time)"),
                ("inTangents(t = time)", "inTangents(t = time)"),
                ("outTangents(t = time)", "outTangents(t = time)"),
                ("isClosed()", "isClosed()"),
                (
                    "createPath(points, inTangents = [], outTangents = [], is_closed = true)",
                    "createPath(points, inTangents = [], outTangents = [], is_closed = true)",
                ),
            ],
        ),
        (
            "Text > Style",
            vec![
                ("text.sourceText.style", "text.sourceText.style"),
                ("getStyleAt(charIndex, t = time)", "getStyleAt(charIndex, t = time)"),
                ("createStyle()", "createStyle()"),
                ("setFontSize(value)", "setFontSize(value)"),
                ("setFillColor(value)", "setFillColor(value)"),
                ("setText(value)", "setText(value)"),
            ],
        ),
        ("Text > Expression Selector", vec![("textIndex", "textIndex"), ("textTotal", "textTotal"), ("selectorValue", "selectorValue")]),
        ("Key", vec![("value", "value"), ("time", "time"), ("index", "index")]),
        (
            "Marker Key",
            vec![
                ("duration", "duration"),
                ("comment", "comment"),
                ("chapter", "chapter"),
                ("url", "url"),
                ("frameTarget", "frameTarget"),
                ("eventCuePoint", "eventCuePoint"),
                ("cuePointName", "cuePointName"),
                ("parameters", "parameters"),
                ("protectedRegion", "protectedRegion"),
            ],
        ),
        (
            "Time Conversion",
            vec![
                (
                    "timeToFrames(t = time + thisComp.displayStartTime, fps = 1.0 / thisComp.frameDuration, isDuration = false)",
                    "timeToFrames(t = time + thisComp.displayStartTime, fps = 1.0 / thisComp.frameDuration, isDuration = false)",
                ),
                ("framesToTime(frames, fps = 1.0 / thisComp.frameDuration)", "framesToTime(frames, fps = 1.0 / thisComp.frameDuration)"),
                (
                    "timeToTimecode(t = time + thisComp.displayStartTime, timecodeBase = 30, isDuration = false)",
                    "timeToTimecode(t = time + thisComp.displayStartTime, timecodeBase = 30, isDuration = false)",
                ),
                (
                    "timeToFeetAndFrames(t = time + thisComp.displayStartTime, fps = 1.0 / thisComp.frameDuration, framesPerFoot = 16, isDuration = false)",
                    "timeToFeetAndFrames(t = time + thisComp.displayStartTime, fps = 1.0 / thisComp.frameDuration, framesPerFoot = 16, isDuration = false)",
                ),
                (
                    "timeToCurrentFormat(t = time + thisComp.displayStartTime, fps = 1.0 / thisComp.frameDuration, isDuration = false)",
                    "timeToCurrentFormat(t = time + thisComp.displayStartTime, fps = 1.0 / thisComp.frameDuration, isDuration = false)",
                ),
            ],
        ),
    ]
}

fn menu_cmd(_s: &mut Session, _p: &Value) -> Result<Value> {
    Ok(json!(
        language_menu()
            .into_iter()
            .map(|(c, items)| json!({"category": c, "items": items.into_iter().map(|(l, t)| json!({"label": l, "text": t})).collect::<Vec<_>>()}))
            .collect::<Vec<_>>()
    ))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        query!("expr.errors", "Expression errors (the error bar)", "{comp?, time? (s)}", errors_cmd),
        query!("expr.languageMenu", "Expression Language menu", "{}", menu_cmd),
    ]
}
