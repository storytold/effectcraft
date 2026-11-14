//! EffectCraft composition → Lottie document.

use std::collections::BTreeSet;

use craft_lottie::model::{Asset, Effect, Font, Fonts, Layer as LottieLayer, LayerContent, Marker, Mask, PrecompLayer, SolidLayer, Transform};
use craft_lottie::{Animated, Animation, Json};
use effectcraft_keyframe::Value;
use effectcraft_project::{Comp, FootageKind, GroupKind, ItemId, ItemKind, Layer, LayerSource, MaskMode, MatteKind, Project, PropGroup};
use serde_json::json;

use crate::anim::{self, Ex, export_prop};
use crate::shapes::fixed;
use crate::{LottieError, Timebase, shapes, text};

/// Export settings.
#[derive(Clone, Debug, Default)]
pub struct ExportOptions {
    /// Export expressions as Lottie `x` strings (players that run expressions evaluate them).
    pub include_expressions: bool,
    /// Export text layers as glyph shapes (exact look, no font needed) instead of text layers.
    pub text_as_shapes: bool,
}

/// An exported Lottie document and what could not be expressed.
#[derive(Clone, Debug)]
pub struct ExportResult {
    pub json: Json,
    pub warnings: Vec<String>,
}

impl ExportResult {
    pub fn to_string_compact(&self) -> String {
        serde_json::to_string(&self.json).unwrap_or_default()
    }
}

/// One supported effect: Lottie effect type and its parameter list in Lottie order:
/// (EffectCraft param id or "" for a Lottie-only param, Lottie value type, scale to Lottie,
/// constant value for Lottie-only params).
pub(crate) struct FxMap {
    pub id: &'static str,
    ty: u32,
    params: &'static [(&'static str, u32, f64, f64)],
}

/// The effects Lottie players implement.
const EFFECTS: &[FxMap] = &[
    FxMap { id: "ec.blur.gaussian", ty: 29, params: &[("blurriness", 0, 1.0, 0.0), ("dimensions", 7, 1.0, 0.0), ("repeatEdge", 4, 1.0, 0.0)] },
    FxMap { id: "ec.color.tint", ty: 20, params: &[("black", 2, 1.0, 0.0), ("white", 2, 1.0, 0.0), ("amount", 0, 1.0, 0.0)] },
    FxMap {
        id: "ec.color.tritone",
        ty: 23,
        params: &[("highlights", 2, 1.0, 0.0), ("midtones", 2, 1.0, 0.0), ("shadows", 2, 1.0, 0.0), ("blend", 0, 1.0, 0.0)],
    },
    FxMap {
        id: "ec.perspective.dropshadow",
        ty: 25,
        params: &[
            ("color", 2, 1.0, 0.0),
            ("opacity", 0, 2.55, 0.0),
            ("direction", 1, 1.0, 0.0),
            ("distance", 0, 1.0, 0.0),
            ("softness", 0, 1.0, 0.0),
            ("shadowOnly", 4, 1.0, 0.0),
        ],
    },
    FxMap {
        id: "ec.generate.fill",
        ty: 21,
        params: &[
            ("", 10, 1.0, 0.0),
            ("", 4, 1.0, 1.0),
            ("color", 2, 1.0, 0.0),
            ("invert", 4, 1.0, 0.0),
            ("", 0, 1.0, 0.0),
            ("", 0, 1.0, 0.0),
            ("opacity", 0, 0.01, 0.0),
        ],
    },
    FxMap { id: "ec.control.slider", ty: 5, params: &[("slider", 0, 1.0, 0.0)] },
    FxMap { id: "ec.control.angle", ty: 5, params: &[("angle", 1, 1.0, 0.0)] },
    FxMap { id: "ec.control.color", ty: 5, params: &[("color", 2, 1.0, 0.0)] },
    FxMap { id: "ec.control.point", ty: 5, params: &[("point", 3, 1.0, 0.0)] },
    FxMap { id: "ec.control.checkbox", ty: 5, params: &[("checkbox", 4, 1.0, 0.0)] },
];

pub(crate) fn fx_by_id(id: &str) -> Option<&'static FxMap> {
    EFFECTS.iter().find(|f| f.id == id)
}

pub(crate) fn fx_by_type(ty: u64, inner_ty: Option<u64>) -> Option<&'static FxMap> {
    if ty == 5 {
        return EFFECTS.iter().find(|f| f.ty == 5 && Some(f.params[0].1 as u64) == inner_ty);
    }
    EFFECTS.iter().find(|f| f.ty as u64 == ty)
}

pub(crate) fn fx_params(f: &FxMap) -> &'static [(&'static str, u32, f64, f64)] {
    f.params
}

fn mask_mode(m: MaskMode) -> &'static str {
    match m {
        MaskMode::None => "n",
        MaskMode::Add => "a",
        MaskMode::Subtract => "s",
        MaskMode::Intersect => "i",
        MaskMode::Lighten => "l",
        MaskMode::Darken => "d",
        MaskMode::Difference => "f",
    }
}

pub(crate) fn mask_mode_from(s: &str) -> MaskMode {
    match s {
        "n" => MaskMode::None,
        "s" => MaskMode::Subtract,
        "i" => MaskMode::Intersect,
        "l" => MaskMode::Lighten,
        "d" => MaskMode::Darken,
        "f" => MaskMode::Difference,
        _ => MaskMode::Add,
    }
}

pub(crate) fn matte_code(k: MatteKind) -> u32 {
    match k {
        MatteKind::Alpha => 1,
        MatteKind::AlphaInverted => 2,
        MatteKind::Luma => 3,
        MatteKind::LumaInverted => 4,
    }
}

struct Doc<'a> {
    project: &'a Project,
    opts: &'a ExportOptions,
    read: &'a dyn Fn(&str) -> Option<Vec<u8>>,
    ex: Ex,
    assets: Vec<Asset>,
    done: BTreeSet<String>,
    fonts: Vec<Font>,
    any_3d: bool,
}

/// The image type `bytes` hold, by their signature; `None` for anything else.
pub(crate) fn image_mime(bytes: &[u8]) -> Option<&'static str> {
    craft_lottie::image_mime(bytes)
}

impl Doc<'_> {
    fn frame(&self, t: effectcraft_time::Tick) -> f64 {
        self.ex.tb.frame(t)
    }

    fn prop(&mut self, g: &PropGroup, m: &str, conv: &dyn Fn(&Value) -> Json, label: &str) -> Option<Animated> {
        let p = g.get(m)?;
        Some(export_prop(&mut self.ex, p, conv, &format!("{label} ▸ {}", p.name)))
    }

    fn transform(&mut self, l: &Layer) -> Transform {
        let mut ks = Transform::default();
        let Some(tr) = l.transform() else { return ks };
        let label = format!("{} ▸ Transform", l.name);
        let three = l.is_3d();
        ks.anchor = self.prop(tr, "anchor", &anim::arr3, &label);
        if tr.get("positionX").is_some() {
            let mut p = json!({"s": true});
            for (k, m) in [("x", "positionX"), ("y", "positionY"), ("z", "positionZ")] {
                if (k != "z" || three)
                    && let Some(v) = self.prop(tr, m, &anim::num, &label)
                {
                    p[k] = v.0;
                }
            }
            ks.position = Some(Animated(p));
        } else {
            ks.position = self.prop(tr, "position", &anim::arr3, &label);
        }
        ks.scale = self.prop(tr, "scale", &anim::arr3, &label);
        if three {
            ks.rotation_x = self.prop(tr, "rotationX", &anim::num, &label);
            ks.rotation_y = self.prop(tr, "rotationY", &anim::num, &label);
            ks.rotation_z = self.prop(tr, "rotation", &anim::num, &label);
            ks.orientation = self.prop(tr, "orientation", &anim::arr3, &label);
        } else {
            ks.rotation = self.prop(tr, "rotation", &anim::num, &label);
        }
        ks.opacity = self.prop(tr, "opacity", &anim::num, &label);
        ks
    }

    fn masks(&mut self, l: &Layer) -> Vec<Mask> {
        let Some(masks) = l.masks() else { return vec![] };
        let mut out = vec![];
        for m in masks.groups() {
            let GroupKind::Mask { mode, inverted, .. } = m.kind else { continue };
            let label = format!("{} ▸ {}", l.name, m.name);
            let mut o = Mask { name: Some(m.name.clone()), mode: Some(mask_mode(mode).into()), inverted: Some(inverted), ..Default::default() };
            o.path = self.prop(m, "path", &shapes::path_value, &label);
            o.opacity = self.prop(m, "opacity", &anim::num, &label);
            o.expansion = self.prop(m, "expansion", &anim::num, &label);
            if let Some(f) = m.get("feather") {
                if f.is_animated() || f.value.components().iter().any(|v| *v != 0.0) {
                    self.ex.warn(format!("{label}: mask feather is exported as `f` (not every Lottie player renders it)"));
                }
                o.extra.insert("f".into(), export_prop(&mut self.ex, f, &anim::arr2, &format!("{label} ▸ Mask Feather")).0);
            }
            if !m.enabled {
                o.mode = Some("n".into());
            }
            out.push(o);
        }
        out
    }

    fn effects(&mut self, l: &Layer) -> Vec<Effect> {
        let Some(fx) = l.effects() else { return vec![] };
        let mut out = vec![];
        for (i, g) in fx.groups().enumerate() {
            let GroupKind::Effect { effect } = &g.kind else { continue };
            let label = format!("{} ▸ {}", l.name, g.name);
            let Some(map) = fx_by_id(effect) else {
                self.ex.warn(format!("{label}: effect not supported by Lottie (skipped)"));
                continue;
            };
            let mut params = vec![];
            for (pi, (m, ty, scale, konst)) in map.params.iter().enumerate() {
                let (nm, v) = match g.get(m).filter(|_| !m.is_empty()) {
                    Some(p) => {
                        let (ty, scale) = (*ty, *scale);
                        let conv = move |v: &Value| -> Json {
                            match ty {
                                2 => anim::arr(v),
                                3 => anim::arr2(v),
                                4 => json!(v.as_bool() as u8),
                                7 => json!(v.as_enum() + 1),
                                _ => json!(v.as_f64() * scale),
                            }
                        };
                        (p.name.clone(), export_prop(&mut self.ex, p, &conv, &format!("{label} ▸ {}", p.name)))
                    }
                    None => (format!("Param {}", pi + 1), fixed(json!(konst))),
                };
                params.push(Effect {
                    kind: Some(i64::from(*ty)),
                    name: Some(nm),
                    match_name: Some((*m).into()),
                    index: Some(pi as i64 + 1),
                    value: Some(v),
                    ..Default::default()
                });
            }
            out.push(Effect {
                kind: Some(i64::from(map.ty)),
                name: Some(g.name.clone()),
                match_name: Some(effect.clone()),
                index: Some(i as i64 + 1),
                enabled: Some(i64::from(g.enabled)),
                count: Some(params.len() as i64),
                values: Some(params),
                ..Default::default()
            });
        }
        out
    }

    fn image_asset(&mut self, item: ItemId, l: &Layer) -> Option<String> {
        let id = format!("image_{}", item.0);
        if self.done.contains(&id) {
            return Some(id);
        }
        let it = self.project.item(item)?;
        let ItemKind::Footage(f) = &it.kind else { return None };
        let Some(bytes) = (self.read)(&f.path) else {
            self.ex.warn(format!("{}: cannot read image `{}` (layer exported as a null)", l.name, f.path));
            return None;
        };
        // What a project calls a still is embedded only when it is one: the path can name any
        // file (an imported animation's asset), and the export would carry its bytes verbatim.
        let Some(mime) = image_mime(&bytes) else {
            self.ex.warn(format!("{}: `{}` is not a PNG, JPEG, GIF or WebP image (layer exported as a null)", l.name, f.path));
            return None;
        };
        self.assets.push(Asset {
            id: Some(id.clone()),
            name: Some(it.name.clone()),
            width: Some(f64::from(f.width)),
            height: Some(f64::from(f.height)),
            dir: Some(String::new()),
            path: Some(craft_lottie::base64::to_data_uri(mime, &bytes)),
            embedded: Some(1),
            ..Default::default()
        });
        self.done.insert(id.clone());
        Some(id)
    }

    fn precomp_asset(&mut self, item: ItemId) -> Option<String> {
        let id = format!("comp_{}", item.0);
        if self.done.contains(&id) {
            return Some(id);
        }
        let it = self.project.item(item)?;
        let c = it.as_comp()?;
        self.done.insert(id.clone());
        let layers = self.layers(c);
        self.assets.push(Asset {
            id: Some(id.clone()),
            name: Some(it.name.clone()),
            frame_rate: Some(c.frame_rate.as_f64()),
            width: Some(f64::from(c.width)),
            height: Some(f64::from(c.height)),
            layers: Some(layers),
            ..Default::default()
        });
        Some(id)
    }

    fn layers(&mut self, comp: &Comp) -> Vec<LottieLayer> {
        let mattes: BTreeSet<_> = comp.layers.iter().filter_map(|l| l.track_matte.map(|m| m.layer)).collect();
        let mut out = vec![];
        for (idx, l) in comp.layers.iter().enumerate() {
            out.push(self.layer(comp, idx, l, mattes.contains(&l.id)));
        }
        out
    }

    fn layer(&mut self, comp: &Comp, idx: usize, l: &Layer, is_matte: bool) -> LottieLayer {
        let name = l.name.clone();
        let mut o = LottieLayer { index: Some(idx as i64 + 1), name: Some(name.clone()), ..Default::default() };
        let three = l.is_3d();
        if three {
            self.any_3d = true;
            if !l.is_camera() && !l.is_light() {
                self.ex.warn(format!("{name}: 3D layer exported with ddd: 1 (most Lottie players render 2D only)"));
            }
        }
        o.three_d = Some(i64::from(three));
        let mut hidden = !l.switches.video || l.switches.guide;
        o.content = LayerContent::Null;
        match &l.source {
            LayerSource::Shape => {
                let items = l.props.sub("contents").map(|c| shapes::export_items(&mut self.ex, c, &name)).unwrap_or_default();
                o.content = LayerContent::Shape(items);
            }
            LayerSource::Text => {
                if self.opts.text_as_shapes {
                    o.content = LayerContent::Shape(text::glyph_shapes(&mut self.ex, l));
                } else {
                    o.content = LayerContent::Text(Box::new(text::export_text(&mut self.ex, l, &mut self.fonts)));
                }
            }
            LayerSource::Solid { item } => {
                let sol = self.project.item(*item).and_then(|i| if let ItemKind::Solid(s) = &i.kind { Some(s.clone()) } else { None });
                match sol {
                    Some(s) if !l.switches.adjustment => {
                        o.content = LayerContent::Solid(SolidLayer {
                            color: Some(craft_lottie::to_hex(s.color)),
                            width: Some(f64::from(s.width)),
                            height: Some(f64::from(s.height)),
                        });
                    }
                    _ => {
                        self.ex.warn(format!("{name}: adjustment layers are not part of Lottie (exported as a null)"));
                        hidden = true;
                    }
                }
            }
            LayerSource::Comp { item } => {
                if let Some(id) = self.precomp_asset(*item) {
                    let (w, h) = self.project.comp(*item).map(|c| (c.width, c.height)).unwrap_or((comp.width, comp.height));
                    let time_remap = l.props.get("timeRemap").map(|tm| export_prop(&mut self.ex, tm, &anim::num, &format!("{name} ▸ Time Remap")));
                    o.content = LayerContent::Precomp(PrecompLayer { ref_id: Some(id), width: Some(f64::from(w)), height: Some(f64::from(h)), time_remap });
                }
            }
            LayerSource::Footage { item } => {
                let kind = self.project.item(*item).and_then(|i| if let ItemKind::Footage(f) = &i.kind { Some((f.kind, f.has_video)) } else { None });
                match kind {
                    Some((FootageKind::Still, true)) => {
                        if let Some(id) = self.image_asset(*item, l) {
                            o.content = LayerContent::Image(Some(id));
                        } else {
                            hidden = true;
                        }
                    }
                    Some((FootageKind::Audio, _)) | Some((_, false)) => {
                        self.ex.warn(format!("{name}: audio is not part of Lottie (layer exported as a hidden null)"));
                        hidden = true;
                    }
                    _ => {
                        self.ex.warn(format!("{name}: video and image-sequence footage is not part of Lottie (layer exported as a null)"));
                        hidden = true;
                    }
                }
            }
            LayerSource::Null => {}
            LayerSource::Camera | LayerSource::Light { .. } => {
                self.ex.warn(format!("{name}: cameras and lights are not exported (3D; exported as a hidden null to keep parenting)"));
                hidden = true;
            }
            LayerSource::Model { .. } | LayerSource::Primitive { .. } => {
                self.ex.warn(format!("{name}: 3D model layers are not part of Lottie (layer exported as a hidden null)"));
                hidden = true;
            }
        }
        o.parent = l.parent.and_then(|p| comp.index_of(p)).map(|p| p as i64);
        o.stretch = Some(l.stretch / 100.0);
        if l.stretch < 0.0 {
            self.ex.warn(format!("{name}: reversed time stretch is not supported by most Lottie players"));
        }
        o.transform = Some(self.transform(l));
        o.auto_orient = Some(i64::from(l.auto_orient == effectcraft_project::AutoOrient::AlongPath));
        o.in_point = Some(self.frame(l.in_point));
        o.out_point = Some(self.frame(l.out_point));
        o.start_time = Some(self.frame(l.start_time));
        let bm = match crate::blend_to_lottie(l.blend_mode) {
            Some(b) => b,
            None => {
                self.ex.warn(format!("{name}: blend mode {:?} has no Lottie equivalent (Normal used)", l.blend_mode));
                0
            }
        };
        o.blend_mode = Some(i64::from(bm));
        let masks = self.masks(l);
        if !masks.is_empty() {
            o.has_mask = Some(true);
            o.masks = Some(masks);
        }
        let ef = self.effects(l);
        if !ef.is_empty() {
            o.effects = Some(ef);
        }
        if let Some(tm) = l.track_matte
            && let Some(mi) = comp.index_of(tm.layer)
        {
            o.matte_mode = Some(i64::from(matte_code(tm.kind)));
            o.matte_parent = Some(mi as i64);
            if mi != idx {
                self.ex.warn(format!("{name}: track matte layer is not directly above (players without `tp` support use the layer above)"));
            }
        }
        if is_matte {
            o.is_matte = Some(1);
            if l.switches.video {
                self.ex.warn(format!("{name}: visible track matte layer (Lottie always hides matte layers)"));
            }
        } else if hidden {
            o.hidden = Some(true);
        }
        if !l.markers.is_empty() {
            self.ex.warn(format!("{name}: layer markers are not part of Lottie (composition markers are exported)"));
        }
        if l.props.sub(effectcraft_project::styles::GROUP).is_some_and(|g| g.groups().any(|s| s.enabled && s.match_id != effectcraft_project::styles::BLENDING))
        {
            self.ex.warn(format!("{name}: layer styles are not exported"));
        }
        if l.preserve_transparency {
            self.ex.warn(format!("{name}: Preserve Underlying Transparency is not part of Lottie"));
        }
        o
    }
}

/// Export composition `comp` as a Lottie document. `read` loads footage files (still images are
/// embedded as base64 assets).
pub fn export_comp(project: &Project, comp: ItemId, opts: &ExportOptions, read: &dyn Fn(&str) -> Option<Vec<u8>>) -> Result<ExportResult, LottieError> {
    let item = project.item(comp).ok_or_else(|| LottieError::Invalid(format!("no item {}", comp.0)))?;
    let c = item.as_comp().ok_or_else(|| LottieError::Invalid(format!("`{}` is not a composition", item.name)))?;
    let tb = Timebase { rate: c.frame_rate };
    let mut doc = Doc {
        project,
        opts,
        read,
        ex: Ex { tb, include_expressions: opts.include_expressions, warnings: vec![] },
        assets: vec![],
        done: BTreeSet::new(),
        fonts: vec![],
        any_3d: false,
    };
    doc.done.insert(format!("comp_{}", comp.0));
    let layers = doc.layers(c);
    let markers: Vec<Marker> = c
        .markers
        .iter()
        .map(|m| Marker { time: Some(doc.frame(m.time)), comment: Some(m.comment.clone()), duration: Some(doc.frame(m.duration)), ..Default::default() })
        .collect();
    let animation = Animation {
        version: Some("5.12.0".into()),
        frame_rate: Some(c.frame_rate.as_f64()),
        in_point: Some(0.0),
        out_point: Some(doc.frame(c.duration)),
        width: Some(f64::from(c.width)),
        height: Some(f64::from(c.height)),
        name: Some(item.name.clone()),
        three_d: Some(i64::from(doc.any_3d)),
        assets: Some(doc.assets),
        layers: Some(layers),
        markers: Some(markers),
        fonts: (!doc.fonts.is_empty()).then(|| Fonts { list: Some(doc.fonts), ..Default::default() }),
        ..Default::default()
    };
    Ok(ExportResult { json: animation.to_json(), warnings: doc.ex.warnings })
}

/// Package an export as a dotLottie archive (`manifest.json` + `animations/<id>.json`).
pub fn to_dotlottie(res: &ExportResult, id: &str) -> Vec<u8> {
    let id: String = id.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '_' }).collect();
    let id = if id.is_empty() { "animation".to_string() } else { id };
    let manifest = json!({"version": "1", "generator": "EffectCraft", "author": "", "animations": [{"id": id, "speed": 1, "loop": true}]});
    crate::zip::store(&[
        ("manifest.json".into(), serde_json::to_vec(&manifest).unwrap_or_default()),
        (format!("animations/{id}.json"), serde_json::to_vec(&res.json).unwrap_or_default()),
    ])
}
