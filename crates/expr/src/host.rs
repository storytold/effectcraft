//! Host requests: everything an expression reads from the project.
//!
//! Scripts never hold references into the project. Instead they ask for plain data
//! ([`Req`] → [`Resp`]) through one native function. A request that isn't answered yet is
//! recorded as a *miss* and the script is aborted; the evaluator then resolves the misses here
//! (with full access to the project, which may recursively evaluate other expressions), memoizes
//! the answers and runs the script again. Expressions are pure functions of these answers, so
//! re-running is exact, and the runtime stays free of borrowed state.

use effectcraft_geom::{Mat3, Mat4};
use effectcraft_keyframe::Value;
use effectcraft_project::{Comp, ItemId, ItemKind, Layer, LayerId, LayerSource, Node, PropGroup, Property};
use effectcraft_render::{EvalCtx, source_size};
use effectcraft_time::Tick;

/// A name-or-index argument (`layer("A")`, `layer(2)`, `effect(1)`).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Key {
    Index(i64),
    Name(String),
}

/// Seconds as hashable bits.
pub type Secs = u64;

pub fn secs(t: f64) -> Secs {
    t.to_bits()
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Req {
    /// `[name, width, height, duration, frameDuration, numLayers, pixelAspect, displayStart, bgColor]`.
    CompInfo { comp: u64 },
    /// Comp item id or null.
    CompByName { name: String },
    /// Layer id or null.
    LayerLookup { comp: u64, key: Key },
    /// `[name, index, inPoint, outPoint, startTime, width, height, parentId|null, hasVideo, is3D, source, enabled]`.
    LayerInfo { comp: u64, layer: u64 },
    /// `[path, isGroup, name, matchName, numChildren, propertyIndex]` or null.
    Child { comp: u64, layer: u64, group: String, key: Key },
    /// `propertyGroup(up)` of the property or group at `path`: the group `up` levels above it as
    /// `[path, isGroup, name, matchName, numChildren, propertyIndex]`, `"layer"` when that is the
    /// layer itself, or null above the layer.
    Ancestor { comp: u64, layer: u64, path: String, up: i64 },
    /// `[name, uid, kind, dims, keyTimes, keyValues, hasExpression, propertyIndex, matchName]`.
    PropInfo { comp: u64, layer: u64, path: String },
    /// Value at comp time `t`; `pre` = keyframes only (no expression).
    Value { comp: u64, layer: u64, path: String, t: Secs, pre: bool },
    /// `[layerToComp (3×3), compToLayer, layerToWorld (4×4), worldToLayer]`, row-major.
    Xform { comp: u64, layer: u64, t: Secs },
    /// `[top, left, width, height]` in layer space.
    SourceRect { comp: u64, layer: u64, t: Secs, extents: bool },
    /// `[[time, duration, comment, chapter, url], …]` for a layer, or the comp when `layer` is None.
    Markers { comp: u64, layer: Option<u64> },
    /// A Source Text value as JSON `{doc, text, runs: [{len, style}], paras}` (styles as
    /// `layer.setText` keys), or null for other properties.
    Doc { comp: u64, layer: u64, path: String, t: Secs, pre: bool },
    /// `footage(name)`: `[name, width, height, duration, frameDuration, pixelAspect, kind,
    /// sourceText|null, dataFormat|null]` or null.
    Footage { name: String },
    /// A data footage item's parsed data as JSON `{data, rows, header}` (`rows`/`header` for
    /// CSV/TSV), or null.
    FootageData { name: String },
    /// `sampleImage`: the average straight `[r, g, b, a]` of the layer's pixels in the box
    /// `point ± radius` (layer space) at comp time `t`, after masks and effects when `post`.
    Sample { comp: u64, layer: u64, x: Secs, y: Secs, rx: Secs, ry: Secs, post: bool, t: Secs },
    /// `thisProject`: `[bitsPerChannel, linearBlending, workingSpace]`.
    Project,
    /// A layer's source item (AE's `Layer.source`): `[kind, id, name, width, height, duration,
    /// frameDuration, pixelAspect]`, kind `"comp"` (id = the precomp's composition) or `"footage"`
    /// (footage, solids and models), or null for layers without an item (text, shape, null…).
    SourceInfo { comp: u64, layer: u64 },
}

/// Parse CSV/TSV text (RFC 4180 quoting) into rows of cells.
pub fn parse_delimited(text: &str, sep: char) -> Vec<Vec<String>> {
    let mut rows = vec![];
    let mut row = vec![];
    let mut cell = String::new();
    let mut quoted = false;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if quoted {
            match c {
                '"' if chars.peek() == Some(&'"') => {
                    chars.next();
                    cell.push('"');
                }
                '"' => quoted = false,
                _ => cell.push(c),
            }
            continue;
        }
        match c {
            '"' if cell.is_empty() => quoted = true,
            c if c == sep => row.push(std::mem::take(&mut cell)),
            '\r' => {}
            '\n' => {
                row.push(std::mem::take(&mut cell));
                rows.push(std::mem::take(&mut row));
            }
            _ => cell.push(c),
        }
    }
    if !cell.is_empty() || !row.is_empty() {
        row.push(cell);
        rows.push(row);
    }
    rows.retain(|r| !(r.len() == 1 && r[0].trim().is_empty()));
    rows
}

/// A CSV/TSV cell as JSON: numbers become numbers.
fn cell_json(c: &str) -> serde_json::Value {
    let t = c.trim();
    match t.parse::<f64>() {
        Ok(x) if x.is_finite() && !t.is_empty() => serde_json::json!(x),
        _ => serde_json::json!(c),
    }
}

/// The data of a data footage item as `{data, rows, header}` JSON (see [`Req::FootageData`]).
pub fn footage_data_json(f: &effectcraft_project::Footage) -> Option<String> {
    let text = f.data.as_deref()?;
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    Some(match f.data_format()? {
        "json" => {
            let v: serde_json::Value = serde_json::from_str(text).ok()?;
            serde_json::json!({"data": v, "rows": null, "header": null}).to_string()
        }
        fmt => {
            let rows = parse_delimited(text, if fmt == "tsv" { '\t' } else { ',' });
            let (header, body) = match rows.split_first() {
                Some((h, b)) => (h.clone(), b.to_vec()),
                None => (vec![], vec![]),
            };
            let objects: Vec<serde_json::Value> = body
                .iter()
                .map(|r| {
                    serde_json::Value::Object(
                        header.iter().enumerate().map(|(i, k)| (k.clone(), r.get(i).map(|c| cell_json(c)).unwrap_or(serde_json::Value::Null))).collect(),
                    )
                })
                .collect();
            let cells: Vec<Vec<serde_json::Value>> = body.iter().map(|r| r.iter().map(|c| cell_json(c)).collect()).collect();
            serde_json::json!({"data": objects, "rows": cells, "header": header}).to_string()
        }
    })
}

/// A text document as the style API's JSON.
pub fn doc_json(d: &effectcraft_keyframe::TextDoc) -> String {
    use effectcraft_keyframe::text_doc::{char_style_json, para_style_json};
    let runs: Vec<serde_json::Value> = d.runs().iter().map(|r| serde_json::json!({"len": r.len, "style": char_style_json(&r.style)})).collect();
    let paras: Vec<serde_json::Value> = d.paras().iter().map(para_style_json).collect();
    serde_json::json!({"doc": d, "text": d.text, "runs": runs, "paras": paras}).to_string()
}

/// Plain data answer, converted to a JS value by the runtime.
#[derive(Clone, Debug, PartialEq)]
pub enum Resp {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    List(Vec<Resp>),
}

impl Resp {
    fn nums(v: impl IntoIterator<Item = f64>) -> Resp {
        Resp::List(v.into_iter().map(Resp::Num).collect())
    }
}

/// Number of components an expression sees: 2D layers see two-dimensional Position, Scale and
/// Anchor Point even though they're stored in 3D.
pub fn prop_dims(layer: &Layer, prop: &Property) -> usize {
    match &prop.value {
        Value::Vec3(_) if prop.shown_dims == 2 && !layer.is_3d() => 2,
        v => v.dims(),
    }
}

/// JS-facing kind of a property's value.
pub fn kind_name(v: &Value) -> &'static str {
    match v {
        Value::Scalar(_) => "number",
        Value::Vec2(_) | Value::Vec3(_) => "array",
        Value::Color(_) => "color",
        Value::Bool(_) => "bool",
        Value::Enum(_) => "enum",
        Value::Path(_) => "path",
        Value::Text(_) | Value::Str(_) => "text",
        Value::Layer(_) => "layer",
        Value::Gradient(_) => "other",
    }
}

/// A property value as expression data.
pub fn value_resp(v: &Value, dims: usize) -> Resp {
    match v {
        Value::Scalar(x) => Resp::Num(*x),
        Value::Vec2(_) | Value::Vec3(_) | Value::Color(_) => Resp::nums(v.components().into_iter().take(dims.max(1))),
        Value::Bool(b) => Resp::Num(if *b { 1.0 } else { 0.0 }),
        // Popups are 1-based in expressions.
        Value::Enum(i) => Resp::Num(*i as f64 + 1.0),
        Value::Text(t) => Resp::Str(t.text.clone()),
        Value::Str(s) => Resp::Str(s.clone()),
        Value::Layer(l) => l.map_or(Resp::Null, |l| Resp::Num(l as f64)),
        Value::Path(p) => {
            let pts = |v: &[[f64; 2]]| Resp::List(v.iter().map(|q| Resp::nums(q.iter().copied())).collect());
            Resp::List(vec![pts(&p.vertices), pts(&p.in_tangents), pts(&p.out_tangents), Resp::Bool(p.closed)])
        }
        Value::Gradient(_) => Resp::Null,
    }
}

/// The property being evaluated (it may not be reachable by path, e.g. in standalone use).
pub struct Own<'a> {
    pub comp: u64,
    pub layer: &'a Layer,
    pub prop: &'a Property,
    pub path: String,
}

pub struct Resolver<'a> {
    pub ctx: EvalCtx<'a>,
    pub own: Own<'a>,
}

fn mat3(m: &Mat3) -> Resp {
    Resp::nums(m.0.iter().flatten().copied())
}
fn mat4(m: &Mat4) -> Resp {
    Resp::nums(m.0.iter().flatten().copied())
}

fn compact(s: &str) -> String {
    s.chars().filter(|c| !c.is_whitespace()).flat_map(char::to_lowercase).collect()
}

/// AE attribute names that differ from our match ids.
fn alias(key: &str) -> &str {
    match key {
        "anchorPoint" => "anchor",
        "xRotation" => "rotationX",
        "yRotation" => "rotationY",
        "zRotation" => "rotation",
        "pointOfInterest" => "poi",
        "maskPath" => "path",
        "maskOpacity" => "opacity",
        "maskFeather" => "feather",
        "maskExpansion" => "expansion",
        "effect" => "effects",
        "mask" => "masks",
        "content" => "contents",
        "strokeWidth" => "width",
        k => k,
    }
}

/// A child node as expression data (see [`Req::Child`]); `index` is 1-based among its siblings.
fn node_resp(path: String, n: &Node, index: usize) -> Resp {
    Resp::List(vec![
        Resp::Str(path),
        Resp::Bool(n.as_group().is_some()),
        Resp::Str(n.name().into()),
        Resp::Str(n.match_id().into()),
        Resp::Num(n.as_group().map_or(0, |g| g.children.len()) as f64),
        Resp::Num(index as f64),
    ])
}

fn find_child<'g>(g: &'g PropGroup, key: &Key) -> Option<&'g Node> {
    match key {
        Key::Index(i) => usize::try_from(*i).ok().filter(|i| *i >= 1).and_then(|i| g.children.get(i - 1)),
        Key::Name(name) => {
            let m = alias(name);
            let c = compact(name);
            g.children
                .iter()
                .find(|n| n.match_id() == m)
                .or_else(|| g.children.iter().find(|n| n.name().eq_ignore_ascii_case(name)))
                .or_else(|| g.children.iter().find(|n| compact(n.name()) == c || compact(n.match_id()) == c))
        }
    }
}

impl<'a> Resolver<'a> {
    fn comp(&self, c: u64) -> Option<&'a Comp> {
        if c == self.ctx.comp_id.0 { Some(self.ctx.comp) } else { self.ctx.project.comp(ItemId(c)) }
    }
    fn layer(&self, c: u64, l: u64) -> Option<&'a Layer> {
        if c == self.own.comp && l == self.own.layer.id.0 {
            return Some(self.own.layer);
        }
        self.comp(c)?.layer(LayerId(l))
    }
    fn ctx_at(&self, c: u64, t: f64) -> Option<EvalCtx<'a>> {
        let comp = self.comp(c)?;
        Some(EvalCtx { project: self.ctx.project, comp_id: ItemId(c), comp, time: Tick::from_seconds_f64(t), expr: self.ctx.expr, footage: self.ctx.footage })
    }
    fn prop(&self, c: u64, l: u64, path: &str) -> Option<(&'a Layer, &'a Property)> {
        let layer = self.layer(c, l)?;
        if c == self.own.comp && l == self.own.layer.id.0 && path == self.own.path {
            return Some((layer, self.own.prop));
        }
        Some((layer, layer.props.prop(path)?))
    }

    pub fn resolve(&self, req: &Req) -> Resp {
        self.try_resolve(req).unwrap_or(Resp::Null)
    }

    /// Resolve an ancestor from its UID path while preserving child indexing.
    fn ancestor(&self, comp: u64, layer: u64, path: &str, up: i64) -> Option<Resp> {
        let l = self.layer(comp, layer)?;
        // The node's full `@uid/@uid/…` path from the layer: its ancestors are its prefixes.
        let uid = match self.prop(comp, layer, path) {
            Some((_, p)) => p.uid,
            None => l.props.node(path)?.uid(),
        };
        let full = l.props.path_of(uid)?;
        let segs: Vec<&str> = full.split('/').collect();
        let up = usize::try_from(up).ok().filter(|u| *u >= 1 && *u <= segs.len())?;
        if up == segs.len() {
            return Some(Resp::Str("layer".into()));
        }
        let keep = segs.len() - up;
        let group_path = segs.get(..keep)?.join("/");
        let parent = match keep {
            1 => &l.props,
            _ => l.props.group(&segs.get(..keep - 1)?.join("/"))?,
        };
        let uid = segs.get(keep - 1)?.strip_prefix('@')?.parse::<u64>().ok()?;
        let index = parent.children.iter().position(|c| c.uid() == uid)?;
        let n = parent.children.get(index)?;
        Some(node_resp(group_path, n, index + 1))
    }

    fn try_resolve(&self, req: &Req) -> Option<Resp> {
        let project = self.ctx.project;
        Some(match req {
            Req::CompInfo { comp } => {
                let c = self.comp(*comp)?;
                let name = project.item(ItemId(*comp)).map(|i| i.name.clone()).unwrap_or_default();
                Resp::List(vec![
                    Resp::Str(name),
                    Resp::Num(c.width as f64),
                    Resp::Num(c.height as f64),
                    Resp::Num(c.duration.seconds()),
                    Resp::Num(c.frame_duration().seconds()),
                    Resp::Num(c.layers.len() as f64),
                    Resp::Num(c.pixel_aspect),
                    Resp::Num(c.display_start.seconds()),
                    Resp::nums([c.background[0] as f64, c.background[1] as f64, c.background[2] as f64, 1.0]),
                ])
            }
            Req::CompByName { name } => {
                let item = project.items.values().find(|i| matches!(i.kind, ItemKind::Comp(_)) && &i.name == name)?;
                Resp::Num(item.id.0 as f64)
            }
            Req::LayerLookup { comp, key } => {
                let c = self.comp(*comp)?;
                let l = match key {
                    Key::Index(i) => usize::try_from(*i).ok().filter(|i| *i >= 1).and_then(|i| c.layers.get(i - 1))?,
                    Key::Name(n) => c.layer_by_name(n)?,
                };
                Resp::Num(l.id.0 as f64)
            }
            Req::LayerInfo { comp, layer } => {
                let c = self.comp(*comp)?;
                let l = self.layer(*comp, *layer)?;
                let (mut w, mut h) = source_size(project, l);
                if w == 0 || h == 0 {
                    (w, h) = (c.width, c.height);
                }
                Resp::List(vec![
                    Resp::Str(l.name.clone()),
                    Resp::Num(c.index_of(l.id).unwrap_or(0) as f64),
                    Resp::Num(l.in_point.seconds()),
                    Resp::Num(l.out_point.seconds()),
                    Resp::Num(l.start_time.seconds()),
                    Resp::Num(w as f64),
                    Resp::Num(h as f64),
                    l.parent.map_or(Resp::Null, |p| Resp::Num(p.0 as f64)),
                    Resp::Bool(l.has_video()),
                    Resp::Bool(l.is_3d()),
                    Resp::Str(l.source.type_name().to_string()),
                    Resp::Bool(l.switches.video),
                ])
            }
            Req::Child { comp, layer, group, key } => {
                let l = self.layer(*comp, *layer)?;
                let g = if group.is_empty() { &l.props } else { l.props.group(group)? };
                let n = find_child(g, key)?;
                let index = g.children.iter().position(|c| c.uid() == n.uid()).unwrap_or(0) + 1;
                let path = if group.is_empty() { format!("@{}", n.uid()) } else { format!("{group}/@{}", n.uid()) };
                node_resp(path, n, index)
            }
            Req::Ancestor { comp, layer, path, up } => return self.ancestor(*comp, *layer, path, *up),
            Req::PropInfo { comp, layer, path } => {
                let (l, p) = self.prop(*comp, *layer, path)?;
                let dims = prop_dims(l, p);
                let times = p.keys.iter().map(|k| Resp::Num(l.comp_time(k.time).seconds())).collect();
                let values = p.keys.iter().map(|k| value_resp(&k.value, dims)).collect();
                let index = path.rsplit('/').next().and_then(|last| {
                    let parent = path.rsplit_once('/').map(|(g, _)| g);
                    let g = match parent {
                        Some(g) => l.props.group(g)?,
                        None => &l.props,
                    };
                    g.children.iter().position(|n| format!("@{}", n.uid()) == last).map(|i| i + 1)
                });
                Resp::List(vec![
                    Resp::Str(p.name.clone()),
                    Resp::Num(p.uid as f64),
                    Resp::Str(kind_name(&p.value).into()),
                    Resp::Num(dims as f64),
                    Resp::List(times),
                    Resp::List(values),
                    Resp::Bool(p.has_expression()),
                    Resp::Num(index.unwrap_or(1) as f64),
                    Resp::Str(p.match_id.clone()),
                ])
            }
            Req::Value { comp, layer, path, t, pre } => {
                let (l, p) = self.prop(*comp, *layer, path)?;
                let t = f64::from_bits(*t);
                let v = if *pre { p.value_at(l.layer_time(Tick::from_seconds_f64(t))) } else { self.ctx_at(*comp, t)?.value(l, p) };
                value_resp(&v, prop_dims(l, p))
            }
            Req::Doc { comp, layer, path, t, pre } => {
                let (l, p) = self.prop(*comp, *layer, path)?;
                let t = f64::from_bits(*t);
                let v = if *pre { p.value_at(l.layer_time(Tick::from_seconds_f64(t))) } else { self.ctx_at(*comp, t)?.value(l, p) };
                match v {
                    Value::Text(d) => Resp::Str(doc_json(&d)),
                    Value::Str(s) => Resp::Str(doc_json(&effectcraft_keyframe::TextDoc::plain(&s))),
                    _ => Resp::Null,
                }
            }
            Req::Xform { comp, layer, t } => {
                let l = self.layer(*comp, *layer)?;
                let ctx = self.ctx_at(*comp, f64::from_bits(*t))?;
                let (m3, _) = ctx.layer_to_comp(l);
                let m4 = ctx.world_matrix(l);
                Resp::List(vec![mat3(&m3), mat3(&m3.inverse().unwrap_or_default()), mat4(&m4), mat4(&m4.inverse().unwrap_or_default())])
            }
            Req::SourceRect { comp, layer, t, extents: _ } => {
                let l = self.layer(*comp, *layer)?;
                let ctx = self.ctx_at(*comp, f64::from_bits(*t))?;
                let r = match &l.source {
                    LayerSource::Text => {
                        let paths: Vec<_> = effectcraft_render::text::glyph_paths(&ctx, l).into_iter().map(|(p, _)| p).collect();
                        effectcraft_path::bounds(&paths).map(|b| [b.y0, b.x0, b.width(), b.height()]).unwrap_or([0.0; 4])
                    }
                    LayerSource::Shape => {
                        let contents = l.props.sub("contents")?;
                        let buf = effectcraft_render::shapes::render(&ctx, l, contents, 1.0);
                        if buf.img.width <= 4 {
                            [0.0; 4]
                        } else {
                            // `render` pads its buffer by 2 px on each side.
                            [2.0 - buf.offset[1], 2.0 - buf.offset[0], buf.img.width as f64 - 4.0, buf.img.height as f64 - 4.0]
                        }
                    }
                    _ => {
                        let (w, h) = source_size(project, l);
                        [0.0, 0.0, w as f64, h as f64]
                    }
                };
                Resp::nums(r)
            }
            Req::Footage { name } => {
                let it = project.items.values().find(|i| &i.name == name && matches!(i.kind, ItemKind::Footage(_)))?;
                let ItemKind::Footage(f) = &it.kind else { return None };
                Resp::List(vec![
                    Resp::Str(it.name.clone()),
                    Resp::Num(f.width as f64),
                    Resp::Num(f.height as f64),
                    Resp::Num(f.duration.seconds()),
                    Resp::Num(f.frame_rate.frame_duration().seconds()),
                    Resp::Num(f.pixel_aspect),
                    Resp::Str(it.type_name().into()),
                    f.data.clone().map_or(Resp::Null, Resp::Str),
                    f.data_format().map_or(Resp::Null, |x| Resp::Str(x.into())),
                ])
            }
            Req::FootageData { name } => {
                let it = project.items.values().find(|i| &i.name == name && matches!(i.kind, ItemKind::Footage(_)))?;
                let ItemKind::Footage(f) = &it.kind else { return None };
                Resp::Str(footage_data_json(f)?)
            }
            Req::Project => Resp::List(vec![
                Resp::Num(project.settings.bit_depth.bits() as f64),
                Resp::Bool(project.settings.blend_linear || project.settings.linearize),
                Resp::Str(project.settings.working_space.map(|w| format!("{w:?}")).unwrap_or_default()),
            ]),
            Req::SourceInfo { comp, layer } => {
                let c = self.comp(*comp)?;
                let l = self.layer(*comp, *layer)?;
                let it = project.item(l.source.item()?)?;
                let (kind, aspect, frame, duration) = match &it.kind {
                    ItemKind::Comp(nc) => ("comp", nc.pixel_aspect, nc.frame_duration().seconds(), nc.duration.seconds()),
                    ItemKind::Footage(f) => ("footage", f.pixel_aspect, f.frame_rate.frame_duration().seconds(), f.duration.seconds()),
                    ItemKind::Solid(s) => ("footage", s.pixel_aspect, c.frame_duration().seconds(), c.duration.seconds()),
                    ItemKind::Folder => return None,
                };
                let (w, h) = it.dimensions().unwrap_or((0, 0));
                Resp::List(vec![
                    Resp::Str(kind.into()),
                    Resp::Num(it.id.0 as f64),
                    Resp::Str(it.name.clone()),
                    Resp::Num(w as f64),
                    Resp::Num(h as f64),
                    Resp::Num(duration),
                    Resp::Num(frame),
                    Resp::Num(aspect),
                ])
            }
            Req::Sample { comp, layer, x, y, rx, ry, post, t } => {
                let l = self.layer(*comp, *layer)?;
                let f = |b: &Secs| f64::from_bits(*b);
                Resp::nums(crate::sample::sample_image(self.ctx, ItemId(*comp), l, [f(x), f(y)], [f(rx), f(ry)], *post, f(t))?.map(|v| v as f64))
            }
            Req::Markers { comp, layer } => {
                let marks: Vec<(f64, &effectcraft_project::Marker)> = match layer {
                    Some(l) => {
                        let l = self.layer(*comp, *l)?;
                        l.markers.iter().map(|m| (l.comp_time(m.time).seconds(), m)).collect()
                    }
                    None => self.comp(*comp)?.markers.iter().map(|m| (m.time.seconds(), m)).collect(),
                };
                Resp::List(
                    marks
                        .into_iter()
                        .map(|(t, m)| {
                            Resp::List(vec![
                                Resp::Num(t),
                                Resp::Num(m.duration.seconds()),
                                Resp::Str(m.comment.clone()),
                                Resp::Str(m.chapter.clone()),
                                Resp::Str(m.url.clone()),
                                Resp::Str(m.frame_target.clone()),
                                Resp::Bool(m.cue_point.as_ref().is_some_and(|c| !c.navigation)),
                                Resp::Str(m.cue_point.as_ref().map(|c| c.name.clone()).unwrap_or_default()),
                                Resp::List(
                                    m.cue_point
                                        .iter()
                                        .flat_map(|c| c.params.iter())
                                        .map(|(k, v)| Resp::List(vec![Resp::Str(k.clone()), Resp::Str(v.clone())]))
                                        .collect(),
                                ),
                                Resp::Bool(m.protected),
                            ])
                        })
                        .collect(),
                )
            }
        })
    }
}
