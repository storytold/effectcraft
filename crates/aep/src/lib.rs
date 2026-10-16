//! Imports loss-accounted JSON project IR exported from AEP files by py-aep.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![recursion_limit = "256"]

use std::{borrow::Cow, collections::BTreeMap, path::Path, sync::Arc};

use effectcraft_color::{BlendMode, Label};
use effectcraft_keyframe::{Ease, Gradient, Interp, Keyframe, ShapePath, Value};
use effectcraft_project::{
    AlphaMode, BitDepth, Comp, Footage, FootageKind, GroupKind, ItemId, ItemKind, Layer, LayerId, LayerSource, MatteKind, Node, ParamUi, Project, PropGroup,
    Property, TrackMatte, build,
};
use effectcraft_time::{FrameRate, Tick};
use serde::Deserialize;
use serde_json::{Value as Json, value::RawValue};

mod color_ease;
mod effect_defaults;
mod effects;
mod looks;
mod markers_essential;
mod models3d;
mod orient;
mod puppet;
mod roto;
mod shapes;
mod styles;
mod text;
mod text_anim;

const IMPLICIT_CAMERA_COMMENT: &str = "AEP implicit default camera";

#[derive(Debug, thiserror::Error)]
pub enum AepError {
    #[error("invalid AE project IR: {0}")]
    Invalid(String),
    #[error("unsupported AE project IR schema version {0}")]
    Schema(u64),
}

#[derive(Debug, Default)]
pub struct ImportResult {
    pub primary_comp: Option<ItemId>,
    pub comps: Vec<ItemId>,
    pub items: Vec<ItemId>,
    /// AE project item id → EffectCraft id. Never keyed by display name.
    pub item_ids: BTreeMap<i64, ItemId>,
    /// (AE comp id, AE layer id) → EffectCraft id.
    pub layer_ids: BTreeMap<(i64, i64), LayerId>,
    /// Hidden AE default-camera ids mapped to editable native camera layers.
    pub implicit_camera_ids: BTreeMap<(i64, i64), LayerId>,
    pub losses: Vec<String>,
    pub warnings: Vec<String>,
    pub missing_media: Vec<String>,
}

fn field<'a>(j: &'a Json, keys: &[&str]) -> Option<&'a Json> {
    keys.iter().find_map(|k| j.get(*k))
}
fn string<'a>(j: &'a Json, keys: &[&str]) -> Option<&'a str> {
    field(j, keys).and_then(Json::as_str)
}
fn number(j: &Json, keys: &[&str]) -> Option<f64> {
    field(j, keys).and_then(Json::as_f64).filter(|n| n.is_finite())
}
fn integer(j: &Json, keys: &[&str]) -> Option<i64> {
    field(j, keys).and_then(Json::as_i64)
}
fn boolean(j: &Json, keys: &[&str]) -> Option<bool> {
    field(j, keys).and_then(Json::as_bool)
}
fn tick(s: f64) -> Tick {
    Tick::from_seconds_f64(s)
}
/// AE saves a rate as the float of the decimal it shows (23.976 is 23.9759979…) and its frame
/// grid is that decimal (2997/125), not NTSC's 24000/1001: layer cuts at 3.003003 s (frame 72)
/// would land 3 µs after an x/1001 frame start and show the previous shot.
fn ae_frame_rate(fps: Option<f64>) -> FrameRate {
    match fps.filter(|v| (0.001..=10_000.0).contains(v)) {
        Some(v) => FrameRate::new((v * 1000.0).round() as i64, 1000),
        None => FrameRate::FPS_30,
    }
}
/// Seconds to ticks, on `rate`'s frame start when within 1 µs of one: rates like 2997/125 have
/// no exact tick per frame, and a rounded cut a tick after the frame start would miss the frame.
fn frame_tick(s: f64, rate: FrameRate) -> Tick {
    let t = tick(s);
    let frame = rate.snap_nearest(t);
    if (t - frame).abs() <= Tick::from_units(1, 1_000_000) { frame } else { t }
}
/// AE property key times are composition times; native properties use layer time.
#[derive(Clone, Copy)]
struct LayerTime {
    start: Tick,
    stretch: f64,
}

impl LayerTime {
    fn of(layer: &Layer) -> Self {
        Self { start: layer.start_time, stretch: layer.stretch }
    }

    fn at(self, comp_seconds: f64) -> Tick {
        let delta = i128::from(tick(comp_seconds).0) - i128::from(self.start.0);
        if (self.stretch - 100.0).abs() < 1e-9 {
            Tick(delta.clamp(i128::from(i64::MIN), i128::from(i64::MAX)) as i64)
        } else {
            Tick((delta as f64 * 100.0 / self.stretch) as i64)
        }
    }

    fn reversed(self) -> bool {
        self.stretch < 0.0
    }
}
fn array<'a>(j: &'a Json, key: &str) -> &'a [Json] {
    j.get(key).and_then(Json::as_array).map(Vec::as_slice).unwrap_or(&[])
}
fn dimensions(j: &Json) -> (u32, u32) {
    let bounded = |n: Option<f64>| n.unwrap_or(1.0).round().clamp(1.0, 32_768.0) as u32;
    (bounded(number(j, &["width"])), bounded(number(j, &["height"])))
}

fn source_issue(j: &Json) -> String {
    let parts: Vec<_> = ["scope", "reason", "detail"].into_iter().filter_map(|k| string(j, &[k])).filter(|s| !s.is_empty()).collect();
    if parts.is_empty() { j.to_string() } else { parts.join(": ") }
}

/// The IR's top level, read in one pass with each item's `layers` and `default_camera` left
/// unparsed: the layer pass parses one comp's layers at a time, so the dynamic tree of the whole
/// export (several times its few hundred MiB of JSON) never exists at once. Fields read as a
/// whole-document parse reads them: the last of a duplicated key wins.
struct RawDoc<'a> {
    fields: BTreeMap<String, Json>,
    /// `None` when `items` is missing or not an array.
    items: Option<Vec<RawItem<'a>>>,
}

/// An item's own fields, and its unparsed `layers` and `default_camera`. An item that is not
/// an object reads as null: neither has an id.
struct RawItem<'a> {
    header: Json,
    layers: Option<&'a RawValue>,
    default_camera: Option<&'a RawValue>,
}

impl RawItem<'_> {
    fn not_object() -> Self {
        RawItem { header: Json::Null, layers: None, default_camera: None }
    }

    /// The whole item, as a whole-document parse would have read it.
    fn parse(self) -> Result<Json, AepError> {
        let mut item = self.header;
        if let Json::Object(fields) = &mut item {
            for (key, raw) in [("layers", self.layers), ("default_camera", self.default_camera)] {
                if let Some(raw) = raw {
                    fields.insert(key.to_owned(), parse_ir(raw)?);
                }
            }
        }
        Ok(item)
    }
}

fn parse_ir(raw: &RawValue) -> Result<Json, AepError> {
    serde_json::from_str(raw.get()).map_err(|e| AepError::Invalid(e.to_string()))
}

/// Parses a value that a later duplicate key replaces, so malformed JSON in it still fails.
fn check_shadowed<E: serde::de::Error>(raw: Option<&RawValue>) -> Result<(), E> {
    raw.map_or(Ok(()), |raw| serde_json::from_str::<Json>(raw.get()).map(drop).map_err(E::custom))
}

impl<'de> Deserialize<'de> for RawDoc<'de> {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Doc;
        impl<'de> serde::de::Visitor<'de> for Doc {
            type Value = RawDoc<'de>;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("an object")
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut doc = RawDoc { fields: BTreeMap::new(), items: None };
                while let Some(key) = map.next_key::<String>()? {
                    if key == "items" {
                        for shadowed in std::mem::replace(&mut doc.items, map.next_value::<Items>()?.0).into_iter().flatten() {
                            check_shadowed(shadowed.layers)?;
                            check_shadowed(shadowed.default_camera)?;
                        }
                    } else {
                        doc.fields.insert(key, map.next_value()?);
                    }
                }
                Ok(doc)
            }
        }
        deserializer.deserialize_map(Doc)
    }
}

/// `items`: the items when it is an array, else nothing.
struct Items<'a>(Option<Vec<RawItem<'a>>>);

impl<'de> Deserialize<'de> for Items<'de> {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Array;
        impl<'de> serde::de::Visitor<'de> for Array {
            type Value = Items<'de>;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("any JSON value")
            }
            fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                let mut items = Vec::new();
                while let Some(item) = seq.next_element()? {
                    items.push(item);
                }
                Ok(Items(Some(items)))
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(self, map: A) -> Result<Self::Value, A::Error> {
                Json::deserialize(serde::de::value::MapAccessDeserializer::new(map)).map(|_| Items(None))
            }
            fn visit_bool<E>(self, _: bool) -> Result<Self::Value, E> {
                Ok(Items(None))
            }
            fn visit_i64<E>(self, _: i64) -> Result<Self::Value, E> {
                Ok(Items(None))
            }
            fn visit_u64<E>(self, _: u64) -> Result<Self::Value, E> {
                Ok(Items(None))
            }
            fn visit_f64<E>(self, _: f64) -> Result<Self::Value, E> {
                Ok(Items(None))
            }
            fn visit_str<E>(self, _: &str) -> Result<Self::Value, E> {
                Ok(Items(None))
            }
            fn visit_unit<E>(self) -> Result<Self::Value, E> {
                Ok(Items(None))
            }
        }
        deserializer.deserialize_any(Array)
    }
}

impl<'de> Deserialize<'de> for RawItem<'de> {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Item;
        impl<'de> serde::de::Visitor<'de> for Item {
            type Value = RawItem<'de>;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("any JSON value")
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut fields = serde_json::Map::new();
                let (mut layers, mut default_camera) = (None, None);
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "layers" => check_shadowed(layers.replace(map.next_value()?))?,
                        "default_camera" => check_shadowed(default_camera.replace(map.next_value()?))?,
                        _ => {
                            fields.insert(key, map.next_value()?);
                        }
                    }
                }
                Ok(RawItem { header: Json::Object(fields), layers, default_camera })
            }
            fn visit_seq<A: serde::de::SeqAccess<'de>>(self, seq: A) -> Result<Self::Value, A::Error> {
                Json::deserialize(serde::de::value::SeqAccessDeserializer::new(seq)).map(|_| RawItem::not_object())
            }
            fn visit_bool<E>(self, _: bool) -> Result<Self::Value, E> {
                Ok(RawItem::not_object())
            }
            fn visit_i64<E>(self, _: i64) -> Result<Self::Value, E> {
                Ok(RawItem::not_object())
            }
            fn visit_u64<E>(self, _: u64) -> Result<Self::Value, E> {
                Ok(RawItem::not_object())
            }
            fn visit_f64<E>(self, _: f64) -> Result<Self::Value, E> {
                Ok(RawItem::not_object())
            }
            fn visit_str<E>(self, _: &str) -> Result<Self::Value, E> {
                Ok(RawItem::not_object())
            }
            fn visit_unit<E>(self) -> Result<Self::Value, E> {
                Ok(RawItem::not_object())
            }
        }
        deserializer.deserialize_any(Item)
    }
}

fn preferred_comp_name(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    name == "main" || name == "final" || name.starts_with("main ") || name.starts_with("final ")
}

/// Import into a cloned project first, so malformed IR leaves the caller's document untouched.
/// `resolve_media` receives the path from the export and returns a usable local path, or None.
pub fn import_ir(
    project: &mut Project,
    bytes: &[u8],
    source_dir: &str,
    resolve_media: &mut dyn FnMut(&str) -> Option<String>,
) -> Result<ImportResult, AepError> {
    let mut next = project.clone();
    let result = import_ir_into(&mut next, bytes, source_dir, resolve_media)?;
    *project = next;
    Ok(result)
}

/// [`import_ir`] without the copy: on an error `next` may be left half imported, so the caller
/// must discard it (an undoable edit, which already works on a copy of the project, does).
pub fn import_ir_into(
    next: &mut Project,
    bytes: &[u8],
    source_dir: &str,
    resolve_media: &mut dyn FnMut(&str) -> Option<String>,
) -> Result<ImportResult, AepError> {
    // Large template packs export to a few hundred MiB of IR; the cap only stops runaway input.
    const MAX_BYTES: usize = 1024 * 1024 * 1024;
    if bytes.len() > MAX_BYTES {
        return Err(AepError::Invalid("IR exceeds 1 GiB".into()));
    }
    let depth = json_depth(bytes, 128);
    if depth > 128 {
        return Err(AepError::Invalid("IR nesting exceeds 128 levels".into()));
    }
    // serde_json's recursion limit (128) only fires on a whole-document parse; near it, parse the
    // whole document once so the error matches.
    if depth > 100 {
        serde_json::from_slice::<Json>(bytes).map_err(|e| AepError::Invalid(e.to_string()))?;
    }
    import_doc(next, bytes, source_dir, resolve_media).map_err(|e| {
        // Unparsed spans are skipped, not checked: malformed JSON reports the whole-document parse
        // error, as when the document was parsed up front.
        match serde_json::from_slice::<Json>(bytes) {
            Err(parse) => AepError::Invalid(parse.to_string()),
            Ok(_) => e,
        }
    })
}

fn import_doc(next: &mut Project, bytes: &[u8], source_dir: &str, resolve_media: &mut dyn FnMut(&str) -> Option<String>) -> Result<ImportResult, AepError> {
    // Valid JSON that is not an object has no schema_version; invalid JSON reports its parse
    // error from `import_ir_into`.
    let doc = serde_json::from_slice::<RawDoc>(bytes).map_err(|_| AepError::Invalid("missing schema_version".into()))?;
    let version = doc.fields.get("schema_version").and_then(Json::as_u64).ok_or_else(|| AepError::Invalid("missing schema_version".into()))?;
    if version != 1 {
        return Err(AepError::Schema(version));
    }
    let items = doc.items.ok_or_else(|| AepError::Invalid("missing items".into()))?;
    if items.len() > 100_000 {
        return Err(AepError::Invalid("too many items".into()));
    }
    let mut result = ImportResult::default();
    let mut property_budget = 1_000_000usize;
    let source_depth = match doc.fields.get("project").and_then(|p| integer(p, &["bits_per_channel"])) {
        Some(16) => BitDepth::Bpc16,
        Some(32) => BitDepth::Bpc32,
        _ => BitDepth::Bpc8,
    };
    if next.items.is_empty() {
        next.settings.bit_depth = source_depth;
    } else if next.settings.bit_depth != source_depth {
        result.losses.push("project bit depth differs from imported AEP".into());
    }
    for loss in doc.fields.get("losses").and_then(Json::as_array).map(Vec::as_slice).unwrap_or(&[]) {
        result.losses.push(source_issue(loss));
    }
    for issue in doc.fields.get("media_issues").and_then(Json::as_array).map(Vec::as_slice).unwrap_or(&[]) {
        result.warnings.push(source_issue(issue));
    }
    // A model layer's scale lives on its source item, which the layer pass no longer holds.
    let mut model_scales = BTreeMap::new();
    // Create every item before resolving parent folders or precomp references.
    for raw in &items {
        let item = &raw.header;
        let ae_id = integer(item, &["id"]).ok_or_else(|| AepError::Invalid("item without integer id".into()))?;
        if result.item_ids.contains_key(&ae_id) {
            return Err(AepError::Invalid(format!("duplicate item id {ae_id}")));
        }
        model_scales.insert(ae_id, number(item, &["model_scale"]));
        let kind = string(item, &["kind"]).unwrap_or("unknown");
        let name = string(item, &["name"]).unwrap_or("Untitled");
        if ae_id == 0 && kind == "folder" {
            continue;
        } // EffectCraft's root folder is implicit.
        let (w, h) = dimensions(item);
        let rate = ae_frame_rate(number(item, &["frame_rate"]));
        let duration = tick(number(item, &["duration"]).unwrap_or(0.0).max(0.0));
        let native = match kind {
            "folder" => ItemKind::Folder,
            "comp" => {
                let mut comp = Comp::new(w, h, rate, duration);
                if let Some(renderer) = string(item, &["renderer"]) {
                    comp.renderer = match renderer.to_ascii_lowercase().as_str() {
                        // AE's saved IDs predate their display labels; Adobe restored them after
                        // an accidental beta rename (community.adobe.com/questions-534/match-names-for-renderers-changing-314356).
                        "adbe advanced 3d" | "adbe classic 3d" => effectcraft_project::Renderer::Classic3D,
                        "adbe calder" => effectcraft_project::Renderer::Advanced3D,
                        _ => {
                            result.losses.push(format!("comp {ae_id}: unsupported renderer {renderer}"));
                            effectcraft_project::Renderer::Classic3D
                        }
                    };
                }
                comp.pixel_aspect = number(item, &["pixel_aspect"]).filter(|v| *v > 0.0).unwrap_or(1.0);
                comp.display_start = tick(number(item, &["display_start_time"]).unwrap_or(0.0));
                let work_start = number(item, &["work_area_start"]).unwrap_or(0.0);
                let work_duration = number(item, &["work_area_duration"]).unwrap_or(duration.seconds());
                comp.work_area = (tick(work_start), tick(work_start + work_duration.max(0.0)));
                if let Some(bg) = field(item, &["bg_color", "background_color"]).and_then(Json::as_array) {
                    for (i, v) in bg.iter().take(3).enumerate() {
                        comp.background[i] = v.as_f64().unwrap_or(0.0).clamp(0.0, 1.0) as f32;
                    }
                }
                ItemKind::Comp(Arc::new(comp))
            }
            "footage" => {
                let solid = string(item, &["asset_type"]) == Some("solid");
                let source = string(item, &["file", "path", "file_path", "source_path"]).unwrap_or("");
                let windows_absolute = source.as_bytes().get(1) == Some(&b':');
                let candidate = if Path::new(source).is_absolute() || windows_absolute || source_dir.is_empty() {
                    source.to_owned()
                } else {
                    format!("{source_dir}/{source}")
                };
                let resolved = string(item, &["resolved_path"])
                    .filter(|p| Path::new(p).is_file())
                    .map(str::to_owned)
                    .or_else(|| if source.is_empty() || solid { None } else { resolve_media(&candidate) });
                if resolved.is_none() && !solid {
                    result.missing_media.push(source.to_owned());
                }
                if solid {
                    let color = item.get("color").and_then(Json::as_array);
                    let channel = |i: usize| color.and_then(|a| a.get(i)).and_then(Json::as_f64).unwrap_or(0.0).clamp(0.0, 1.0) as f32;
                    ItemKind::Solid(effectcraft_project::Solid {
                        color: [channel(0), channel(1), channel(2)],
                        width: w,
                        height: h,
                        pixel_aspect: number(item, &["pixel_aspect"]).unwrap_or(1.0),
                    })
                } else {
                    let missing = resolved.is_none();
                    let media_path = resolved.unwrap_or(candidate);
                    let model = Path::new(&media_path)
                        .extension()
                        .and_then(|e| e.to_str())
                        .is_some_and(|e| matches!(e.to_ascii_lowercase().as_str(), "glb" | "gltf" | "obj"));
                    let source_layer = item.get("source_layer").and_then(|raw| {
                        let layer = effectcraft_project::SourceLayer::deserialize(raw).ok();
                        match layer {
                            Some(layer)
                                if layer.index < 65535
                                    && layer.crop_bounds.is_none_or(|b| {
                                        b.iter().all(|v| v.is_finite() && v.abs() <= 1_000_000.0)
                                            && b[2] > b[0]
                                            && b[3] > b[1]
                                            && b[2] - b[0] <= 30000.0
                                            && b[3] - b[1] <= 30000.0
                                    }) =>
                            {
                                Some(layer)
                            }
                            _ => {
                                result.losses.push(format!("{name}: invalid footage source layer binding"));
                                None
                            }
                        }
                    });
                    ItemKind::Footage(Footage {
                        path: media_path,
                        layer: source_layer,
                        kind: if model {
                            FootageKind::Model
                        } else if boolean(item, &["has_video"]) == Some(false) {
                            FootageKind::Audio
                        } else if duration == Tick::ZERO {
                            FootageKind::Still
                        } else {
                            FootageKind::Video
                        },
                        width: w,
                        height: h,
                        pixel_aspect: number(item, &["pixel_aspect"]).filter(|v| *v > 0.0).unwrap_or(1.0),
                        frame_rate: rate,
                        duration,
                        has_video: boolean(item, &["has_video"]).unwrap_or(true),
                        has_audio: boolean(item, &["has_audio"]).unwrap_or(false),
                        alpha: AlphaMode::Straight,
                        missing,
                        ..Footage::default()
                    })
                }
            }
            other => {
                result.losses.push(format!("item {ae_id} ({name}): unsupported kind {other}"));
                continue;
            }
        };
        let label = integer(item, &["label"]).and_then(|n| usize::try_from(n).ok()).and_then(|i| Label::ALL.get(i).copied()).unwrap_or(Label::None);
        let id = next.add_item(name, label, None, native);
        if let Some(comment) = string(item, &["comment"])
            && let Some(created) = next.item_mut(id)
        {
            created.comment = comment.to_owned();
        }
        result.item_ids.insert(ae_id, id);
        result.items.push(id);
        if kind == "comp" {
            result.comps.push(id);
        }
    }
    // One item's full tree at a time: a template pack's IR is a few hundred MiB of JSON and
    // several times that as a dynamic tree.
    for raw in items {
        let item = &raw.parse()?;
        let Some(ae_id) = integer(item, &["id"]) else { continue };
        let Some(&id) = result.item_ids.get(&ae_id) else { continue };
        if let Some(parent) = integer(item, &["parent_id"]).filter(|p| *p != 0) {
            match result.item_ids.get(&parent).copied() {
                Some(pid) if matches!(next.item(pid).map(|i| &i.kind), Some(ItemKind::Folder)) => {
                    if let Some(i) = next.item_mut(id) {
                        i.parent = Some(pid);
                    }
                }
                _ => result.warnings.push(format!("item {ae_id}: missing parent folder {parent}")),
            }
        }
        if !result.comps.contains(&id) {
            continue;
        }
        if item.get("layers").is_some_and(|v| !v.is_array()) {
            return Err(AepError::Invalid(format!("comp {ae_id}: layers must be an array")));
        }
        let Some(mut comp) = next.comp(id).cloned() else { continue };
        let layers = array(item, "layers");
        if layers.len() > 100_000 {
            return Err(AepError::Invalid(format!("comp {ae_id}: too many layers")));
        }
        let mut refs = Vec::new();
        let mut local_ids = BTreeMap::new();
        for (n, ir) in layers.iter().enumerate() {
            let ae_layer = integer(ir, &["id", "index"]).unwrap_or((n + 1) as i64);
            if local_ids.contains_key(&ae_layer) {
                return Err(AepError::Invalid(format!("comp {ae_id}: duplicate layer id {ae_layer}")));
            }
            if ir.get("properties").is_some_and(|v| !v.is_array()) {
                return Err(AepError::Invalid(format!("comp {ae_id} layer {ae_layer}: properties must be an array")));
            }
            let Some(source) = layer_source(ir, next, &result.item_ids, ae_id, &mut result.losses) else { continue };
            let size = source.item().and_then(|i| next.item(i)).and_then(|i| i.dimensions()).unwrap_or((comp.width, comp.height));
            let name = string(ir, &["name"]).unwrap_or(source.type_name());
            let mut layer = build::layer(next, &comp, name, source, size, None);
            layer.name = name.to_owned();
            let timing = ir.get("timing").unwrap_or(ir);
            layer.start_time = tick(number(timing, &["start_time"]).unwrap_or(0.0));
            layer.in_point = frame_tick(number(timing, &["in_point"]).unwrap_or(0.0), comp.frame_rate);
            layer.out_point = frame_tick(number(timing, &["out_point"]).unwrap_or(comp.duration.seconds()), comp.frame_rate);
            layer.stretch = number(timing, &["stretch"]).filter(|v| v.abs() > f64::EPSILON).unwrap_or(100.0);
            if let Some(sw) = ir.get("switches") {
                apply_switches(sw, &mut layer.switches);
                layer.preserve_transparency = boolean(sw, &["preserve_transparency"]).unwrap_or(false);
                layer.environment = boolean(sw, &["environment_layer"]).unwrap_or(false);
            }
            apply_auto_orient(ir, &mut layer, &mut result.losses, &format!("comp {ae_id} layer {ae_layer}"));
            if let Some(blend) = string(ir, &["blend_mode"]) {
                if let Some(mode) = BlendMode::from_name(blend) {
                    layer.blend_mode = mode;
                } else {
                    result.losses.push(format!("comp {ae_id} layer {ae_layer}: blend mode {blend}"));
                }
            } else if let Some(raw) = ir.get("switches").and_then(|s| integer(s, &["blending_mode"])) {
                if let Some(mode) = blend_from_ae(raw) {
                    layer.blend_mode = mode;
                } else {
                    result.losses.push(format!("comp {ae_id} layer {ae_layer}: unknown blend mode {raw}"));
                }
            }
            for prop in array(ir, "properties") {
                if !check_property_budget(prop, &mut property_budget, 0) {
                    return Err(AepError::Invalid(format!("comp {ae_id} layer {ae_layer}: property or keyframe limit exceeded")));
                }
            }
            let poi_anchor = layer.auto_orient == effectcraft_project::AutoOrient::TowardsPointOfInterest
                && matches!(&layer.source, LayerSource::Camera | LayerSource::Light { .. });
            let layer_time = LayerTime::of(&layer);
            for prop in array(ir, "properties") {
                if string(prop, &["match_name"]) == Some("ADBE Time Remapping")
                    && ir.get("switches").and_then(|sw| boolean(sw, &["time_remap_enabled"])) != Some(true)
                {
                    if !array(prop, "keyframes").is_empty() || string(prop, &["expression"]).is_some_and(|s| !s.is_empty()) {
                        result.losses.push(format!("comp {ae_id} layer {ae_layer}: disabled Time Remap animation retained but not applied"));
                        let value_type = string(prop, &["value_type"]).unwrap_or("");
                        let value = prop
                            .get("value")
                            .and_then(|v| convert_value(v, value_type))
                            .or_else(|| array(prop, "keyframes").first().and_then(|k| k.get("value")).and_then(|v| convert_value(v, value_type)));
                        if let Some(value) = value {
                            let mut inactive = Property::new(next.alloc(), "ADBE Time Remapping", "Time Remap (disabled)", value.clone());
                            fill_property(
                                &mut inactive,
                                prop,
                                value,
                                value_type,
                                1.0,
                                layer_time,
                                &mut result.losses,
                                &format!("comp {ae_id} layer {ae_layer}"),
                            );
                            layer.props.children.push(Node::Prop(inactive));
                        } else {
                            result.losses.push(format!("comp {ae_id} layer {ae_layer}: disabled Time Remap value is unsupported"));
                        }
                    }
                    continue;
                }
                if string(prop, &["match_name"]) == Some("ADBE Marker") {
                    markers_essential::markers(
                        &mut layer.markers,
                        prop,
                        comp.frame_rate.as_f64(),
                        layer_time,
                        &mut result.losses,
                        &format!("comp {ae_id} layer {ae_layer}"),
                    );
                    continue;
                }
                if string(prop, &["match_name"]) == Some("ADBE Layer Overrides") {
                    markers_essential::overrides(next, &mut layer, prop, &mut result.losses, &format!("comp {ae_id} layer {ae_layer}"));
                    continue;
                }
                if string(prop, &["match_name"]) == Some("ADBE Layer Styles") {
                    styles::import(next, &mut layer, prop, &mut result.losses, &format!("comp {ae_id} layer {ae_layer}"));
                    continue;
                }
                import_property(
                    next,
                    &mut layer.props,
                    prop,
                    0,
                    [size.0 as f64, size.1 as f64],
                    comp.frame_rate.as_f64(),
                    poi_anchor,
                    layer_time,
                    &mut result.losses,
                    &format!("comp {ae_id} layer {ae_layer}"),
                );
            }
            place_unauthored_camera_eye(&mut layer, ir);
            if matches!(&layer.source, LayerSource::Light { kind: effectcraft_project::LightKind::Environment }) {
                keep_environment_light_defaults(&mut layer, &mut result.losses, &format!("comp {ae_id} layer {ae_layer}"));
            }
            if matches!(&layer.source, LayerSource::Model { .. }) {
                let model_scale = integer(ir, &["source_id"]).and_then(|id| model_scales.get(&id).copied().flatten());
                models3d::import(next, &mut layer, ir, model_scale, &mut result.losses, &format!("comp {ae_id} layer {ae_layer}"));
                result.losses.push(format!("comp {ae_id} layer {ae_layer}: 3D model coordinate/render parity with AE is unverified"));
            }
            local_ids.insert(ae_layer, layer.id);
            result.layer_ids.insert((ae_id, ae_layer), layer.id);
            refs.push((layer.id, integer(ir, &["parent_id"]), ir.get("matte").filter(|m| !m.is_null())));
            comp.layers.push(layer);
        }
        for (id, parent, matte) in refs {
            if let Some(layer) = comp.layer_mut(id) {
                if let Some(p) = parent {
                    layer.parent = local_ids.get(&p).copied();
                }
                if let Some(m) = matte {
                    let mid = integer(m, &["layer_id"]);
                    let kind = match m.get("type") {
                        Some(Json::Number(n)) => match n.as_i64() {
                            Some(5013) => Some(MatteKind::Alpha),
                            Some(5014) => Some(MatteKind::AlphaInverted),
                            Some(5015) => Some(MatteKind::Luma),
                            Some(5016) => Some(MatteKind::LumaInverted),
                            _ => None,
                        },
                        Some(Json::String(s)) => match s.to_ascii_lowercase().as_str() {
                            "alpha" => Some(MatteKind::Alpha),
                            "alpha_inverted" | "alphainverted" => Some(MatteKind::AlphaInverted),
                            "luma" => Some(MatteKind::Luma),
                            "luma_inverted" | "lumainverted" => Some(MatteKind::LumaInverted),
                            _ => None,
                        },
                        _ => None,
                    };
                    layer.track_matte = mid.and_then(|m| local_ids.get(&m).copied()).zip(kind).map(|(layer, kind)| TrackMatte { layer, kind });
                    if layer.track_matte.is_none() {
                        result.losses.push(format!("comp {ae_id} layer {}: unresolved matte", id.0));
                    }
                }
            }
        }
        // AE effect layer selectors store 1-based timeline indices, while native
        // effect controls store stable layer IDs. Resolve after every layer exists.
        let mut by_index = BTreeMap::<u64, Option<u64>>::new();
        for source_layer in array(item, "layers") {
            let Some(index) = integer(source_layer, &["index"]).and_then(|n| u64::try_from(n).ok()).filter(|n| *n > 0) else { continue };
            let uid = integer(source_layer, &["id"]).and_then(|id| local_ids.get(&id)).map(|id| id.0);
            by_index.entry(index).and_modify(|slot| *slot = None).or_insert(uid);
        }
        for layer in &mut comp.layers {
            let context = format!("comp {ae_id} layer {}", layer.name);
            layer.props.walk_mut(&mut |prop| resolve_layer_refs(prop, &by_index, &mut result.losses, &context));
        }
        if item.get("default_camera").is_none() && comp.has_3d() {
            result.losses.push(format!("comp {ae_id}: 3D content has no default_camera field in this IR version"));
        }
        if let Some(default) = item.get("default_camera").filter(|v| !v.is_null()) {
            let (ae_camera, camera) = import_implicit_camera(next, &comp, ae_id, default, &mut property_budget, &mut result.losses)?;
            if local_ids.contains_key(&ae_camera) {
                return Err(AepError::Invalid(format!("comp {ae_id}: implicit camera id {ae_camera} collides with a timeline layer")));
            }
            result.implicit_camera_ids.insert((ae_id, ae_camera), camera.id);
            comp.layers.push(camera); // bottom-most: explicit active cameras above it win.
        }
        if comp.layers.iter().any(|l| matches!(&l.source, LayerSource::Model { .. })) {
            comp.renderer = effectcraft_project::Renderer::Advanced3D;
        }
        orient::ae_orientation(&mut comp, &mut result.losses, &format!("comp {ae_id}"));
        if let Some(i) = next.item_mut(id) {
            i.kind = ItemKind::Comp(Arc::new(comp));
        }
    }
    markers_essential::bind(next, &mut result.losses);
    let referenced: std::collections::BTreeSet<ItemId> =
        next.comps().flat_map(|(_, c)| c.layers.iter().filter_map(|l| if let LayerSource::Comp { item } = &l.source { Some(*item) } else { None })).collect();
    let roots: Vec<ItemId> = result.comps.iter().copied().filter(|id| !referenced.contains(id)).collect();
    result.primary_comp = roots
        .iter()
        .copied()
        .find(|id| next.item(*id).is_some_and(|i| preferred_comp_name(&i.name)))
        .or_else(|| roots.first().copied())
        .or_else(|| result.comps.first().copied());
    if roots.len() > 1 && !roots.iter().any(|id| next.item(*id).is_some_and(|i| preferred_comp_name(&i.name))) {
        result.warnings.push(format!("{} top-level compositions; selected first as primary", roots.len()));
    }
    next.fix_next_id();
    Ok(result)
}

fn import_implicit_camera(
    project: &mut Project,
    comp: &Comp,
    ae_comp: i64,
    ir: &Json,
    property_budget: &mut usize,
    losses: &mut Vec<String>,
) -> Result<(i64, Layer), AepError> {
    if string(ir, &["kind"]) != Some("CameraLayer") || ir.get("properties").is_some_and(|v| !v.is_array()) {
        return Err(AepError::Invalid(format!("comp {ae_comp}: malformed implicit camera")));
    }
    let ae_id = integer(ir, &["id"]).ok_or_else(|| AepError::Invalid(format!("comp {ae_comp}: implicit camera has no id")))?;
    let pose = ir.get("pose").ok_or_else(|| AepError::Invalid(format!("comp {ae_comp}: implicit camera has no pose")))?;
    let position =
        field(pose, &["position"]).and_then(vec3).ok_or_else(|| AepError::Invalid(format!("comp {ae_comp}: implicit camera position is invalid")))?;
    let poi = field(pose, &["point_of_interest"])
        .and_then(vec3)
        .ok_or_else(|| AepError::Invalid(format!("comp {ae_comp}: implicit camera point of interest is invalid")))?;
    let zoom = number(pose, &["zoom"]).filter(|v| *v > 0.0).ok_or_else(|| AepError::Invalid(format!("comp {ae_comp}: implicit camera zoom is invalid")))?;
    // Older IR exposed the hidden default camera's unauthored Position as its POI. The eye
    // actually sits one zoom distance behind that plane; otherwise z=0 layers are clipped.
    let eye_z = position[2] - zoom;
    let default_eye = position == poi && eye_z.is_finite() && unauthored_position(ir);
    for prop in array(ir, "properties") {
        if !check_property_budget(prop, property_budget, 0) {
            return Err(AepError::Invalid(format!("comp {ae_comp}: implicit camera property or keyframe limit exceeded")));
        }
    }
    let mut camera = build::layer(project, comp, "AE Default Camera", LayerSource::Camera, (comp.width, comp.height), None);
    camera.comment = format!("{IMPLICIT_CAMERA_COMMENT}; AE comp {ae_comp}, camera {ae_id}");
    camera.start_time = Tick::ZERO;
    camera.in_point = Tick::ZERO;
    camera.out_point = comp.duration;
    if let Some(sw) = ir.get("switches") {
        apply_switches(sw, &mut camera.switches);
    }
    apply_auto_orient(ir, &mut camera, losses, &format!("comp {ae_comp} implicit camera {ae_id}"));
    if let Some(transform) = camera.transform_mut() {
        if let Some(p) = transform.get_mut("position") {
            p.value = Value::Vec3(position);
        }
        if let Some(p) = transform.get_mut("poi") {
            p.value = Value::Vec3(poi);
        }
    }
    if let Some(options) = camera.props.sub_mut("cameraOptions")
        && let Some(p) = options.get_mut("zoom")
    {
        p.value = Value::Scalar(zoom);
    }
    let layer_time = LayerTime::of(&camera);
    for prop in array(ir, "properties") {
        import_property(
            project,
            &mut camera.props,
            prop,
            0,
            [comp.width as f64, comp.height as f64],
            comp.frame_rate.as_f64(),
            true,
            layer_time,
            losses,
            &format!("comp {ae_comp} implicit camera {ae_id}"),
        );
    }
    if default_eye && let Some(position_prop) = camera.transform_mut().and_then(|transform| transform.get_mut("position")) {
        position_prop.value = Value::Vec3([position[0], position[1], eye_z]);
    }
    Ok((ae_id, camera))
}

/// Whether a camera's Position stream was never set (not live, unkeyed, no expression). AE
/// then places the eye one zoom distance in front of the Point of Interest; the IR reports the
/// stream's placeholder, which equals the Point of Interest and would put the eye on the z = 0
/// plane where every layer is clipped.
fn unauthored_position(ir: &Json) -> bool {
    array(ir, "properties")
        .iter()
        .find(|group| string(group, &["match_name"]) == Some("ADBE Transform Group"))
        .and_then(|group| array(group, "children").iter().find(|prop| string(prop, &["match_name"]) == Some("ADBE Position")))
        .is_some_and(|prop| {
            !boolean(prop, &["authored"]).unwrap_or(false)
                && !boolean(prop, &["dimensions_separated"]).unwrap_or(false)
                && array(prop, "keyframes").is_empty()
                && string(prop, &["expression"]).unwrap_or("").is_empty()
        })
}

/// An explicit two-node camera with an unauthored Position sits one zoom distance in front of
/// its Point of Interest, the same pose AE gives its hidden default camera.
fn place_unauthored_camera_eye(layer: &mut Layer, ir: &Json) {
    if !matches!(layer.source, LayerSource::Camera) || !unauthored_position(ir) {
        return;
    }
    let zoom = layer
        .props
        .sub("cameraOptions")
        .and_then(|options| options.get("zoom"))
        .filter(|zoom| zoom.keys.is_empty() && zoom.expr.is_none())
        .map(|zoom| zoom.value.as_f64())
        .filter(|zoom| zoom.is_finite() && *zoom > 0.0);
    let Some(zoom) = zoom else { return };
    let Some(transform) = layer.transform_mut() else { return };
    let Some(poi) = transform.get("poi").filter(|poi| poi.keys.is_empty()).map(|poi| poi.value.as_vec3()) else { return };
    let Some(position) = transform.get_mut("position") else { return };
    let eye_z = poi[2] - zoom;
    if position.keys.is_empty() && position.value.as_vec3() == poi && eye_z.is_finite() {
        position.value = Value::Vec3([poi[0], poi[1], eye_z]);
    }
}

fn blend_from_ae(raw: i64) -> Option<BlendMode> {
    Some(match raw {
        5212 => BlendMode::Normal,
        5213 => BlendMode::Dissolve,
        5214 => BlendMode::DancingDissolve,
        5215 => BlendMode::Darken,
        5216 => BlendMode::Multiply,
        5217 => BlendMode::LinearBurn,
        5218 => BlendMode::ColorBurn,
        5219 => BlendMode::ClassicColorBurn,
        5220 => BlendMode::Add,
        5221 => BlendMode::Lighten,
        5222 => BlendMode::Screen,
        5223 => BlendMode::LinearDodge,
        5224 => BlendMode::ColorDodge,
        5225 => BlendMode::ClassicColorDodge,
        5226 => BlendMode::Overlay,
        5227 => BlendMode::SoftLight,
        5228 => BlendMode::HardLight,
        5229 => BlendMode::LinearLight,
        5230 => BlendMode::VividLight,
        5231 => BlendMode::PinLight,
        5232 => BlendMode::HardMix,
        5233 => BlendMode::Difference,
        5234 => BlendMode::ClassicDifference,
        5235 => BlendMode::Exclusion,
        5236 => BlendMode::Hue,
        5237 => BlendMode::Saturation,
        5238 => BlendMode::Color,
        5239 => BlendMode::Luminosity,
        5240 => BlendMode::StencilAlpha,
        5241 => BlendMode::StencilLuma,
        5242 => BlendMode::SilhouetteAlpha,
        5243 => BlendMode::SilhouetteLuma,
        5244 => BlendMode::AlphaAdd,
        5245 => BlendMode::LuminescentPremul,
        5246 => BlendMode::LighterColor,
        5247 => BlendMode::DarkerColor,
        5248 => BlendMode::Subtract,
        5249 => BlendMode::Divide,
        _ => return None,
    })
}

fn check_property_budget(ir: &Json, remaining: &mut usize, depth: usize) -> bool {
    if depth >= 64 || *remaining == 0 {
        return false;
    }
    if ir.get("children").is_some_and(|v| !v.is_array()) || ir.get("keyframes").is_some_and(|v| !v.is_array()) {
        return false;
    }
    *remaining -= 1;
    let keys = array(ir, "keyframes").len();
    if keys > *remaining {
        return false;
    }
    *remaining -= keys;
    array(ir, "children").iter().all(|child| check_property_budget(child, remaining, depth + 1))
}

/// The deepest nesting of `bytes`, or `limit + 1` as soon as it exceeds `limit`.
fn json_depth(bytes: &[u8], limit: usize) -> usize {
    let mut depth = 0usize;
    let mut deepest = 0usize;
    let mut quoted = false;
    let mut escaped = false;
    for &b in bytes {
        if quoted {
            if escaped {
                escaped = false;
            } else if b == b'\\' {
                escaped = true;
            } else if b == b'"' {
                quoted = false;
            }
        } else {
            match b {
                b'"' => quoted = true,
                b'[' | b'{' => {
                    depth += 1;
                    if depth > limit {
                        return depth;
                    }
                    deepest = deepest.max(depth);
                }
                b']' | b'}' => depth = depth.saturating_sub(1),
                _ => {}
            }
        }
    }
    deepest
}

fn layer_source(ir: &Json, project: &Project, item_ids: &BTreeMap<i64, ItemId>, comp_id: i64, losses: &mut Vec<String>) -> Option<LayerSource> {
    if ir.get("switches").and_then(|s| boolean(s, &["null_layer"])) == Some(true) {
        return Some(LayerSource::Null);
    }
    let kind = string(ir, &["kind"]).unwrap_or("unknown");
    let source_id = integer(ir, &["source_id"]);
    let src = source_id.and_then(|id| item_ids.get(&id)).and_then(|id| project.item(*id));
    match kind {
        "text" | "TextLayer" => Some(LayerSource::Text),
        "shape" | "ShapeLayer" => Some(LayerSource::Shape),
        "null" => Some(LayerSource::Null),
        "camera" | "CameraLayer" => Some(LayerSource::Camera),
        "light" | "LightLayer" => Some(LayerSource::Light {
            kind: match integer(ir, &["light_type"]) {
                Some(4412) => effectcraft_project::LightKind::Parallel,
                Some(4413) => effectcraft_project::LightKind::Spot,
                Some(4415) => effectcraft_project::LightKind::Ambient,
                Some(4416) => effectcraft_project::LightKind::Environment,
                Some(4414) | None => effectcraft_project::LightKind::Point,
                Some(raw) => {
                    losses.push(format!("comp {comp_id}: unsupported light type {raw}"));
                    effectcraft_project::LightKind::Point
                }
            },
        }),
        "ThreeDModelLayer" => models3d::layer_source(ir, project, item_ids, comp_id, losses),
        "solid" | "av" | "footage" | "precomp" | "AVLayer" => match src {
            Some(i) => match &i.kind {
                ItemKind::Comp(_) => Some(LayerSource::Comp { item: i.id }),
                ItemKind::Footage(f) if f.kind == FootageKind::Model => Some(LayerSource::Model { item: i.id }),
                ItemKind::Footage(_) => Some(LayerSource::Footage { item: i.id }),
                ItemKind::Solid(_) => Some(LayerSource::Solid { item: i.id }),
                _ => None,
            },
            None => {
                losses.push(format!("comp {comp_id}: layer {} has missing source {:?}", string(ir, &["name"]).unwrap_or("?"), source_id));
                None
            }
        },
        _ => {
            losses.push(format!("comp {comp_id}: unsupported layer kind {kind}"));
            None
        }
    }
}

fn apply_switches(ir: &Json, sw: &mut effectcraft_project::Switches) {
    macro_rules! set {
        ($name:ident, $key:expr) => {
            if let Some(v) = boolean(ir, &[$key]) {
                sw.$name = v;
            }
        };
    }
    set!(video, "video");
    set!(audio, "audio");
    set!(solo, "solo");
    set!(locked, "locked");
    set!(shy, "shy");
    set!(collapse, "collapse");
    set!(effects, "effects");
    set!(motion_blur, "motion_blur");
    set!(adjustment, "adjustment");
    set!(three_d, "three_d");
    set!(guide, "guide");
    if let Some(v) = boolean(ir, &["enabled"]) {
        sw.video = v;
    }
    if let Some(v) = boolean(ir, &["audio_enabled"]) {
        sw.audio = v;
    }
    if let Some(v) = boolean(ir, &["collapse_transformation"]) {
        sw.collapse = v;
    }
    if let Some(v) = boolean(ir, &["effects_active"]) {
        sw.effects = v;
    }
    if let Some(v) = boolean(ir, &["adjustment_layer"]) {
        sw.adjustment = v;
    }
    if let Some(v) = boolean(ir, &["is_3d", "three_d_layer"]) {
        sw.three_d = v;
    }
    if let Some(v) = boolean(ir, &["guide_layer"]) {
        sw.guide = v;
    }
    if let Some(v) = integer(ir, &["quality"]) {
        sw.quality = match v {
            4612 => effectcraft_project::Quality::Wireframe,
            4613 => effectcraft_project::Quality::Draft,
            _ => effectcraft_project::Quality::Best,
        };
    }
    if integer(ir, &["sampling_quality"]) == Some(4813) {
        sw.sampling = effectcraft_project::Sampling::Bicubic;
    }
    if let Some(v) = integer(ir, &["frame_blending_type"]) {
        sw.frame_blend = match v {
            4013 => effectcraft_project::FrameBlend::FrameMix,
            4014 => effectcraft_project::FrameBlend::PixelMotion,
            _ => effectcraft_project::FrameBlend::Off,
        };
    }
}

fn apply_auto_orient(ir: &Json, layer: &mut Layer, losses: &mut Vec<String>, context: &str) {
    let Some(raw) = ir.get("switches").and_then(|sw| integer(sw, &["auto_orient"])) else {
        if matches!(&layer.source, LayerSource::Camera | LayerSource::Light { .. }) {
            losses.push(format!("{context}: camera/light auto-orient is not present in IR"));
        }
        return;
    };
    layer.auto_orient = match raw {
        4212 => effectcraft_project::AutoOrient::Off,
        4213 => effectcraft_project::AutoOrient::AlongPath,
        4214 if matches!(&layer.source, LayerSource::Camera | LayerSource::Light { .. }) => effectcraft_project::AutoOrient::TowardsPointOfInterest,
        4214 => effectcraft_project::AutoOrient::TowardsCamera,
        4215 => {
            losses.push(format!("{context}: per-character camera orientation is not represented"));
            effectcraft_project::AutoOrient::Off
        }
        _ => {
            losses.push(format!("{context}: unknown auto-orient mode {raw}"));
            layer.auto_orient
        }
    };
}

/// The environment light renders at its native Intensity and Colour, as before Light Options
/// were imported: Advanced 3D's response to them differs per scene and is not modelled yet. AE's
/// values stay on the layer under their original ids.
fn keep_environment_light_defaults(layer: &mut Layer, losses: &mut Vec<String>, context: &str) {
    let Some(g) = layer.props.sub_mut("lightOptions") else { return };
    let mut changed = false;
    for (id, default) in [("intensity", Value::Scalar(100.0)), ("color", Value::Color([1.0, 1.0, 1.0, 1.0]))] {
        let Some(p) = g.prop_mut(id) else { continue };
        if p.value == default && p.keys.is_empty() {
            continue;
        }
        let mut raw = p.clone();
        raw.match_id = format!("AE {}", raw.match_id);
        p.value = default;
        p.keys.clear();
        p.expr = None;
        g.children.push(Node::Prop(raw));
        changed = true;
    }
    if changed {
        losses.push(format!("{context}: Environment light Intensity / Colour kept at 100 % white (AE response not modelled)"));
    }
}

fn native_match(ae: &str) -> &str {
    if let Some(shape) = shapes::native_match(ae) {
        return shape;
    }
    match ae {
        "ADBE Transform Group" => "transform",
        "ADBE Anchor Point" => "anchor",
        "ADBE Point of Interest" => "poi",
        "ADBE Position" => "position",
        "ADBE Position_0" => "positionX",
        "ADBE Position_1" => "positionY",
        "ADBE Position_2" => "positionZ",
        "ADBE Scale" => "scale",
        "ADBE Orientation" => "orientation",
        "ADBE Rotate X" => "rotationX",
        "ADBE Rotate Y" => "rotationY",
        "ADBE Rotate Z" | "ADBE Rotation" => "rotation",
        "ADBE Opacity" => "opacity",
        "ADBE Time Remapping" => "timeRemap",
        "ADBE Effect Parade" => "effects",
        "ADBE Mask Parade" => "masks",
        "ADBE Mask Atom" => "mask",
        "ADBE Mask Shape" => "path",
        "ADBE Mask Feather" => "feather",
        "ADBE Mask Opacity" => "opacity",
        "ADBE Mask Offset" => "expansion",
        "ADBE Root Vectors Group" => "contents",
        "ADBE Text Properties" => "text",
        "ADBE Text Document" => "sourceText",
        "ADBE Audio Group" => "audio",
        "ADBE Audio Levels" => "levels",
        "ADBE Material Options Group" => "materialOptions",
        "ADBE Camera Options Group" => "cameraOptions",
        "ADBE Camera Zoom" => "zoom",
        "ADBE Camera Depth of Field" => "dof",
        "ADBE Camera Focus Distance" => "focusDistance",
        "ADBE Camera Aperture" => "aperture",
        "ADBE Camera Blur Level" => "blurLevel",
        "ADBE Iris Shape" => "irisShape",
        "ADBE Iris Rotation" => "irisRotation",
        "ADBE Iris Roundness" => "irisRoundness",
        "ADBE Iris Aspect Ratio" => "irisAspectRatio",
        "ADBE Iris Diffraction Fringe" => "irisDiffractionFringe",
        "ADBE Iris Highlight Gain" => "highlightGain",
        "ADBE Iris Highlight Threshold" => "highlightThreshold",
        "ADBE Iris Hightlight Saturation" => "highlightSaturation",
        "ADBE Light Options Group" => "lightOptions",
        "ADBE Light Intensity" => "intensity",
        "ADBE Light Color" => "color",
        "ADBE Light Cone Angle" => "coneAngle",
        "ADBE Light Cone Feather 2" => "coneFeather",
        "ADBE Light Falloff Type" => "falloff",
        "ADBE Light Falloff Start" => "radius",
        "ADBE Light Falloff Distance" => "falloffDistance",
        "ADBE Light Casts Shadows" => "castsShadows",
        "ADBE Light Shadow Darkness" => "shadowDarkness",
        "ADBE Light Shadow Diffusion" => "shadowDiffusion",
        "ADBE Extrsn Options Group" | "ADBE Plane Options Group" => "geometryOptions",
        "ADBE Casts Shadows" => "castsShadows",
        "ADBE Light Transmission" => "lightTransmission",
        "ADBE Accepts Shadows" => "acceptsShadows",
        "ADBE Accepts Lights" => "acceptsLights",
        "ADBE Ambient Coefficient" => "ambient",
        "ADBE Diffuse Coefficient" => "diffuse",
        "ADBE Specular Coefficient" | "ADBE Specular Intensity" => "specularIntensity",
        "ADBE Shininess Coefficient" | "ADBE Specular Shininess" => "specularShininess",
        "ADBE Metal Coefficient" => "metal",
        "ADBE Bevel Styles" => "bevelStyle",
        "ADBE Bevel Depth" => "bevelDepth",
        "ADBE Hole Bevel Depth" => "holeBevelDepth",
        "ADBE Extrsn Depth" => "extrusionDepth",
        _ => ae,
    }
}

fn import_property(
    project: &mut Project,
    parent: &mut PropGroup,
    ir: &Json,
    depth: usize,
    size: [f64; 2],
    fps: f64,
    poi_anchor: bool,
    layer_time: LayerTime,
    losses: &mut Vec<String>,
    context: &str,
) {
    if depth >= 64 {
        losses.push(format!("{context}: property depth exceeds 64"));
        return;
    }
    let ae_match = string(ir, &["match_name"]).unwrap_or("");
    let mut match_id = native_match(ae_match);
    if let Some(native) = text_anim::native_match(parent.match_id.as_str(), ae_match, boolean(ir, &["authored"])) {
        match_id = native;
    }
    if poi_anchor && parent.match_id == "transform" && ae_match == "ADBE Anchor Point" {
        match_id = "poi";
    }
    if let GroupKind::Effect { effect } = &parent.kind
        && let Some(native) = effects::param_id(effect, ae_match)
    {
        match_id = native;
    }
    let name = string(ir, &["name"]).unwrap_or(match_id);
    let kind = string(ir, &["kind"]).unwrap_or("unknown");
    if ae_match == "ADBE Text Animators"
        && array(ir, "children").iter().any(|child| boolean(child, &["enabled"]).unwrap_or(true))
        && !text_anim::fully_supported(ir)
    {
        losses.push(format!("{context}: active Text Animators are preserved as editable properties but not rendered natively"));
    }
    if matches!(ae_match, "ADBE Layer Styles" | "ADBE MTrackers" | "ADBE Layer Overrides" | "ADBE Data Group")
        && boolean(ir, &["enabled"]).unwrap_or(true)
        && !array(ir, "children").is_empty()
    {
        losses.push(format!("{context}: {name} is preserved as editable properties but not applied natively"));
    }
    if kind == "group" || !array(ir, "children").is_empty() {
        if parent.match_id == "effects"
            && ae_match == "MB LookSuite3"
            && let Some(encoded) = string(ir, &["mbl2_state_hex"])
        {
            match looks::parse_hex(encoded) {
                Ok(state) => {
                    let mut group = state.editable_group(project, name, losses, context);
                    group.enabled = boolean(ir, &["enabled"]).unwrap_or(true);
                    for child in array(ir, "children") {
                        import_property(project, &mut group, child, depth + 1, size, fps, poi_anchor, layer_time, losses, context);
                    }
                    parent.children.push(Node::Group(group));
                    return;
                }
                Err(error) => losses.push(format!("{context}: invalid MB LookSuite3 state: {error}")),
            }
        }
        if parent.match_id == "effects"
            && let Some(spec) = effects::effect_id(ae_match).and_then(effectcraft_effects::find)
        {
            let mut group = effectcraft_effects::instantiate(spec, &mut build::Ids(&mut project.next_id), name, size);
            group.enabled = boolean(ir, &["enabled"]).unwrap_or(true);
            if spec.id == effectcraft_effects::puppet::ID {
                puppet::import(project, &mut group, ir, layer_time, losses, context);
            } else if spec.id == effectcraft_effects::roto::ID {
                roto::import(project, &mut group, ir, size, fps, layer_time, losses, context);
            } else {
                if spec.id == "ec.color.curves" {
                    import_curves_state(&mut group, ir, losses, context);
                }
                import_native_effect_params(project, &mut group, ir, ir, spec.id, layer_time, losses, context, depth + 1);
                if spec.id == "ec.color.colorama" {
                    import_colorama_cycle(&mut group, ir, losses, context);
                }
                if spec.id == "S_CartoonPaint" {
                    losses.push(format!("{context}: S_CartoonPaint brush-stroke rendering is approximate; visual parity unverified"));
                }
            }
            if spec.id == "ec.perspective.ccsphere"
                && group.prop("metal").is_some_and(|metal| {
                    metal.value.as_f64() != 0.0
                        || metal.keys.iter().any(|key| key.value.as_f64() != 0.0)
                        || metal.expr.as_ref().is_some_and(|expression| expression.enabled)
                })
            {
                losses.push(format!("{context}: CC Sphere metal specular approximation; AE parity unverified"));
            }
            parent.children.push(Node::Group(group));
            return;
        }
        let indexed =
            matches!(parent.match_id.as_str(), "contents" | "effects" | "masks" | "animators" | "selectors") || matches!(parent.kind, GroupKind::Indexed);
        let existing = if indexed { None } else { parent.children.iter().position(|n| n.match_id() == match_id && matches!(n, Node::Group(_))) };
        let at = if let Some(at) = existing {
            at
        } else {
            let mut group = if parent.match_id == "masks" && match_id == "mask" {
                let index = parent.children.len();
                build::mask(
                    &mut build::Ids(&mut project.next_id),
                    name,
                    ShapePath::default(),
                    effectcraft_project::MaskMode::Add,
                    build::MASK_COLORS[index % build::MASK_COLORS.len()],
                )
            } else {
                PropGroup::new(project.alloc(), match_id, name)
            };
            if matches!(match_id, "animator" | "rangeSelector") {
                group.kind = GroupKind::Indexed;
            }
            if parent.match_id == "effects" {
                if !effects::control_only(ae_match) {
                    losses.push(format!("{context}: effect {ae_match} has no native renderer"));
                }
            } else if parent.match_id == "contents" {
                group.kind = GroupKind::Indexed;
            }
            parent.children.push(Node::Group(group));
            parent.children.len() - 1
        };
        if let Some(Node::Group(group)) = parent.children.get_mut(at) {
            group.enabled = boolean(ir, &["enabled"]).unwrap_or(true);
            if let GroupKind::Mask { mode, inverted, color, locked, motion_blur, feather_falloff, roto_bezier } = &mut group.kind
                && let Some(meta) = ir.get("metadata")
            {
                if let Some(raw) = integer(meta, &["mask_mode"]) {
                    *mode = match raw {
                        6812 => effectcraft_project::MaskMode::None,
                        6813 => effectcraft_project::MaskMode::Add,
                        6814 => effectcraft_project::MaskMode::Subtract,
                        6815 => effectcraft_project::MaskMode::Intersect,
                        6816 => effectcraft_project::MaskMode::Lighten,
                        6817 => effectcraft_project::MaskMode::Darken,
                        6818 => effectcraft_project::MaskMode::Difference,
                        _ => {
                            losses.push(format!("{context}: unknown mask mode {raw}"));
                            effectcraft_project::MaskMode::Add
                        }
                    };
                }
                if let Some(v) = boolean(meta, &["inverted"]) {
                    *inverted = v;
                }
                if let Some(v) = boolean(meta, &["locked"]) {
                    *locked = v;
                }
                if let Some(v) = boolean(meta, &["roto_bezier"]) {
                    *roto_bezier = v;
                }
                if let Some(v) = integer(meta, &["mask_motion_blur"]) {
                    *motion_blur = match v {
                        7012 => effectcraft_project::MaskMotionBlur::SameAsLayer,
                        7013 => effectcraft_project::MaskMotionBlur::On,
                        7014 => effectcraft_project::MaskMotionBlur::Off,
                        _ => {
                            losses.push(format!("{context}: unknown mask motion blur {v}"));
                            effectcraft_project::MaskMotionBlur::SameAsLayer
                        }
                    };
                }
                if let Some(v) = integer(meta, &["mask_feather_falloff"]) {
                    *feather_falloff = match v {
                        7212 => effectcraft_project::FeatherFalloff::Smooth,
                        7213 => effectcraft_project::FeatherFalloff::Linear,
                        _ => {
                            losses.push(format!("{context}: unknown mask feather falloff {v}"));
                            effectcraft_project::FeatherFalloff::Smooth
                        }
                    };
                }
                if let Some(a) = meta.get("color").and_then(Json::as_array) {
                    for (i, channel) in color.iter_mut().enumerate() {
                        if let Some(v) = a.get(i).and_then(Json::as_f64).filter(|v| v.is_finite()) {
                            *channel = if v <= 1.0 { (v.clamp(0.0, 1.0) * 255.0).round() as u8 } else { v.clamp(0.0, 255.0).round() as u8 };
                        }
                    }
                }
            }
            let separated = ae_match == "ADBE Transform Group"
                && array(ir, "children")
                    .iter()
                    .find(|child| string(child, &["match_name"]) == Some("ADBE Position"))
                    .and_then(|position| boolean(position, &["dimensions_separated"]))
                    == Some(true);
            for child in array(ir, "children") {
                if ae_match == "ADBE Transform Group"
                    && matches!(string(child, &["match_name"]), Some("ADBE Position_0" | "ADBE Position_1" | "ADBE Position_2"))
                    && !separated
                {
                    continue;
                }
                import_property(project, group, child, depth + 1, size, fps, poi_anchor, layer_time, losses, context);
            }
        }
    } else if kind == "property" {
        // py-aep exposes placeholder 10px Dash/Gap slots even for solid strokes. Only
        // authored slots belong to the stroke; importing placeholders makes it dashed.
        if parent.match_id == "dashes"
            && boolean(ir, &["authored"]) == Some(false)
            && array(ir, "keyframes").is_empty()
            && string(ir, &["expression"]).unwrap_or("").is_empty()
        {
            return;
        }
        let value_json = ir.get("value").unwrap_or(&Json::Null);
        let value_type = string(ir, &["value_type"]).unwrap_or("");
        if value_json.is_null() && array(ir, "keyframes").is_empty() {
            return;
        }
        let Some(value) = convert_value(value_json, value_type) else {
            losses.push(format!("{context}: unsupported property {ae_match} ({value_type})"));
            return;
        };
        // Built-in groups already have target properties; other groups keep duplicate AE siblings.
        let native_group = matches!(
            parent.match_id.as_str(),
            "transform"
                | "text"
                | "pathOptions"
                | "mask"
                | "audio"
                | "materialOptions"
                | "cameraOptions"
                | "lightOptions"
                | "geometryOptions"
                | "rangeSelector"
                | "advanced"
                | "properties"
        );
        let existing = native_group.then(|| parent.children.iter().position(|n| n.match_id() == match_id && matches!(n, Node::Prop(_)))).flatten();
        let at = if let Some(at) = existing {
            at
        } else {
            let template = if shapes::is_popup(parent.match_id.as_str(), match_id) {
                Value::Enum(0)
            } else {
                text_anim::template(parent.match_id.as_str(), match_id).unwrap_or_else(|| value.clone())
            };
            parent.children.push(Node::Prop(Property::new(project.alloc(), match_id, name, template)));
            parent.children.len() - 1
        };
        if let Some(Node::Prop(prop)) = parent.children.get_mut(at) {
            let scale = if parent.match_id == "cameraOptions" && match_id == "highlightThreshold" { 255.0 } else { 1.0 };
            if matches!(prop.value, Value::Vec2(_) | Value::Vec3(_)) && matches!(match_id, "position" | "anchor") {
                prop.spatial = true;
            }
            // Casts / Accepts Shadows and the text Path (0 None, k = mask k) are zero-based in
            // AE where the generic popup adapter subtracts one.
            if (parent.match_id == "materialOptions" && matches!(match_id, "castsShadows" | "acceptsShadows"))
                || (parent.match_id == "pathOptions" && match_id == "path")
            {
                let f = |v: f64| v + 1.0;
                let value = if let Value::Scalar(v) = value { Value::Scalar(f(v)) } else { value };
                fill_property_mapped(prop, ir, value, value_type, scale, Some(&f), layer_time, losses, context);
            } else if match_id == "direction" && matches!(parent.match_id.as_str(), "path" | "rect" | "ellipse" | "star") {
                // Path Direction is 1 Default / 3 Reversed in AE (2 is normal too); ours is 0 / 1
                // after the popup adapter's minus one.
                let f = |v: f64| if v >= 2.5 { 2.0 } else { 1.0 };
                let value = if let Value::Scalar(v) = value { Value::Scalar(f(v)) } else { value };
                fill_property_mapped(prop, ir, value, value_type, scale, Some(&f), layer_time, losses, context);
            } else {
                fill_property(prop, ir, value, value_type, scale, layer_time, losses, context);
            }
            if match_id == "sourceText" {
                prop.ui = ParamUi::Text;
            }
            if matches!(prop.value, Value::Path(_)) {
                prop.ui = ParamUi::Path;
            }
        }
    } else {
        losses.push(format!("{context}: unknown property node {name}"));
    }
}

/// Colorama's Output Cycle wheel as a palette string ("pos:r,g,b,a …"), from the effect's
/// 520-byte state: 64 stops of (u32 BE position / 65536, A R G B bytes), then a u32 BE count
/// of the stops in use. The wheel holds the palette AE draws with whether it came from a
/// preset or was edited (SceneYard Puppet Control Documentary: black → blue, not the hue
/// cycle that turned the sky green).
fn colorama_cycle_palette(hex: &str) -> Option<String> {
    let bytes: Vec<u8> = (0..hex.len()).step_by(2).map(|i| hex.get(i..i + 2).and_then(|b| u8::from_str_radix(b, 16).ok())).collect::<Option<_>>()?;
    if bytes.len() != 520 {
        return None;
    }
    let be = |at: usize| bytes.get(at..at + 4).map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]));
    let count = be(512)? as usize;
    if count == 0 || count > 64 {
        return None;
    }
    let stops: Vec<String> = (0..count)
        .map(|i| {
            let at = i * 8;
            let pos = (be(at)? as f64 / 65536.0).clamp(0.0, 1.0);
            let c = |k: usize| bytes.get(at + 4 + k).map(|v| *v as f64 / 255.0);
            Some(format!("{pos:.5}:{:.4},{:.4},{:.4},{:.4}", c(1)?, c(2)?, c(3)?, c(0)?))
        })
        .collect::<Option<_>>()?;
    Some(stops.join(" "))
}

fn import_colorama_cycle(group: &mut PropGroup, ir: &Json, losses: &mut Vec<String>, context: &str) {
    let Some(hex) = string(ir, &["colorama_cycle_hex"]) else {
        losses.push(format!("{context}: missing Colorama Output Cycle; the preset palette is used"));
        return;
    };
    let Some(palette) = colorama_cycle_palette(hex) else {
        losses.push(format!("{context}: invalid Colorama Output Cycle; the preset palette is used"));
        return;
    };
    if let Some(prop) = group.prop_mut("outputCycle/palette") {
        prop.value = Value::Str(palette);
    }
    if let Some(prop) = group.prop_mut("outputCycle/usePresetPalette") {
        prop.value = Value::Enum(effectcraft_effects::COLORAMA_PRESETS.len().saturating_sub(1) as u32);
    }
}

fn import_curves_state(group: &mut PropGroup, ir: &Json, losses: &mut Vec<String>, context: &str) {
    let Some(hex) = string(ir, &["curves_state_hex"]) else {
        losses.push(format!("{context}: missing AE Curves lookup tables"));
        return;
    };
    if hex.len() != 1644 * 2 || !hex.starts_with("00010001") || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        losses.push(format!("{context}: invalid AE Curves lookup tables"));
        return;
    }
    for (i, name) in ["rgb", "red", "green", "blue", "alpha"].iter().enumerate() {
        let start = 8 + i * 512;
        if let Some(prop) = group.prop_mut(name) {
            prop.value = Value::Str(format!("lut8:{}", &hex[start..start + 512]));
        }
    }
    if let Some(prop) = group.prop_mut("aeStateHex") {
        prop.value = Value::Str(hex.to_owned());
    }
    if (1..5).any(|channel| {
        (0..256).any(|input| {
            let at = 8 + channel * 512 + input * 2;
            hex.get(at..at + 2).and_then(|s| u8::from_str_radix(s, 16).ok()).is_none_or(|value| value != input as u8)
        })
    }) {
        losses.push(format!("{context}: AE Curves individual-channel LUT visual parity unverified"));
    }
}

fn import_native_effect_params(
    project: &mut Project,
    group: &mut PropGroup,
    ir: &Json,
    effect_ir: &Json,
    effect_id: &str,
    layer_time: LayerTime,
    losses: &mut Vec<String>,
    context: &str,
    depth: usize,
) {
    if depth >= 64 {
        losses.push(format!("{context}: effect property depth exceeds 64"));
        return;
    }
    for child in array(ir, "children") {
        if string(child, &["kind"]) == Some("group") {
            import_native_effect_params(project, group, child, effect_ir, effect_id, layer_time, losses, context, depth + 1);
            continue;
        }
        let Some(raw) = string(child, &["match_name"]) else { continue };
        let value_json = child.get("value").unwrap_or(&Json::Null);
        if value_json.is_null() && array(child, "keyframes").is_empty() {
            continue;
        }
        if matches!((raw, value_json.as_f64()), ("ADBE Effect Mask Opacity", Some(100.0)) | ("ADBE Force CPU GPU", Some(1.0)))
            && array(child, "keyframes").is_empty()
        {
            continue;
        }
        if integer(child, &["value_type"]) == Some(6412)
            && value_json.as_f64() == Some(0.0)
            && array(child, "keyframes").is_empty()
            && string(child, &["expression"]).unwrap_or("").is_empty()
        {
            continue; // AE NO_VALUE with no animation or expression is UI-only.
        }
        if effects::inert_diagnostic(
            effect_id,
            raw,
            value_json,
            !array(child, "keyframes").is_empty(),
            !string(child, &["expression"]).unwrap_or("").is_empty(),
        ) {
            continue;
        }
        let Some(path) = effects::param_id(effect_id, raw) else {
            if !effect_defaults::inert(effect_id, raw, child, effect_ir) {
                losses.push(format!("{context}: unsupported {effect_id} parameter {raw}"));
            }
            if let Some(value) = convert_value(value_json, string(child, &["value_type"]).unwrap_or("")) {
                let mut prop = Property::new(project.alloc(), raw, string(child, &["name"]).unwrap_or(raw), value.clone());
                fill_property(&mut prop, child, value, string(child, &["value_type"]).unwrap_or(""), 1.0, layer_time, losses, context);
                group.children.push(Node::Prop(prop));
            }
            continue;
        };
        let Some(prop) = group.prop_mut(path) else {
            losses.push(format!("{context}: native {effect_id} parameter {path} missing"));
            continue;
        };
        let value_type = string(child, &["value_type"]).unwrap_or("");
        let Some(value) = convert_value(value_json, value_type) else {
            losses.push(format!("{context}: unsupported {effect_id} value {raw}"));
            continue;
        };
        if unauthored_raw_default_point(child, &value) {
            continue; // Keep the native default (the same layer-relative point).
        }
        // Match each effect's native opacity unit before importing static and keyed values.
        let scale = if effect_id == "ec.perspective.dropshadow" && path == "opacity" {
            100.0 / 255.0
        } else if effect_id == "ec.generate.fill" && path == "opacity" {
            100.0
        } else if effect_id == "ec.stylize.glow" {
            match path {
                // AE stores Glow Threshold in 0..255 (shown as 60% = 153) and A & B Midpoint as
                // 0..1; the native controls are percentages.
                "threshold" => 100.0 / 255.0,
                "abMidpoint" => 100.0,
                _ => 1.0,
            }
        } else if effect_id == "ec.generate.scribble" {
            match path {
                // AE stores Scribble's Opacity, Curviness and Curviness Variation as 0..1
                // fractions; the native controls are percentages (SceneYard Magnifying Glass:
                // an untouched 100% Opacity read as 1% hid the scribble).
                "opacity" | "strokeOptions/curviness" | "strokeOptions/curvinessVariation" => 100.0,
                _ => 1.0,
            }
        } else if effect_id == "ec.generate.radiowaves" {
            match path {
                // AE's 0..1 Opacity: 1 draws the waves at half strength (measured with aerender).
                "waveStroke/opacity" => 50.0,
                // AE's Expansion / Velocity unit is 30 px per second.
                "waveMotion/expansion" | "waveMotion/velocity" => 30.0,
                _ => 1.0,
            }
        } else {
            1.0
        };
        // Keylight's AE mask selector is already zero-based (0=None, 1=Mask 1) and the generic
        // AE popup adapter subtracts one; its Source Crops Right / Bottom are edge positions
        // (100% = uncropped) where the native controls crop that much from the edge.
        let remap: Option<fn(f64) -> f64> = match (effect_id, path) {
            ("ec.keying.keylight", "insideMask/insideMask" | "outsideMask/outsideMask") => Some(|v| v + 1.0),
            ("ec.keying.keylight", "sourceCrops/cropRight" | "sourceCrops/cropBottom") => Some(|v| 100.0 - v),
            // CC Page Turn's Controls is numeric natively but keeps AE's popup order less one
            // (SceneYard Patient ID: AE 4 = drag the top-right corner).
            ("ec.distort.ccpageturn", "controls") => Some(|v| v - 1.0),
            // Glow Operation: AE lists an option between Normal (1) and Add (3, its default) that
            // the native list lacks (aerender, SceneYard UI Envelope: 3 adds, 4 multiplies).
            ("ec.stylize.glow", "glowOperation") => Some(|v| if v >= 3.0 { v - 1.0 } else { 1.0 }),
            _ => None,
        };
        if let Some(f) = remap {
            let adjusted_value = if let Value::Scalar(v) = value { Value::Scalar(f(v)) } else { value };
            fill_property_mapped(prop, child, adjusted_value, value_type, scale, Some(&f), layer_time, losses, context);
        } else {
            fill_property(prop, child, value, value_type, scale, layer_time, losses, context);
        }
    }
}

/// An unauthored effect point that py-aep reports in AE's raw fixed-point units (the layer
/// centre 540, 960 as 54000, 96000; SceneYard Photos Collage's CC Vignette centre): no layer
/// is that large, so the native default stands in for it.
fn unauthored_raw_default_point(ir: &Json, value: &Value) -> bool {
    boolean(ir, &["authored"]) == Some(false)
        && array(ir, "keyframes").is_empty()
        && string(ir, &["expression"]).unwrap_or("").is_empty()
        && matches!(value, Value::Vec2(v) if v.iter().any(|c| c.abs() >= 10_000.0))
}

/// An IR value with a numeric value mapped by `f` (a non-finite result reads as null, as JSON
/// can't hold it); other values are borrowed unchanged.
fn remap_json<'a>(j: &'a Json, f: Option<&dyn Fn(f64) -> f64>) -> Cow<'a, Json> {
    match (f, j.as_f64()) {
        (Some(f), Some(v)) => Cow::Owned(Json::from(f(v))),
        _ => Cow::Borrowed(j),
    }
}

fn fill_property(prop: &mut Property, ir: &Json, value: Value, value_type: &str, scale: f64, layer_time: LayerTime, losses: &mut Vec<String>, context: &str) {
    fill_property_mapped(prop, ir, value, value_type, scale, None, layer_time, losses, context);
}

/// [`fill_property`] with keyframe values mapped by `remap` as they are read (`value` is
/// already mapped), so a remapped property's IR is never copied.
fn fill_property_mapped(
    prop: &mut Property,
    ir: &Json,
    value: Value,
    value_type: &str,
    scale: f64,
    remap: Option<&dyn Fn(f64) -> f64>,
    layer_time: LayerTime,
    losses: &mut Vec<String>,
    context: &str,
) {
    let raw = string(ir, &["match_name"]).unwrap_or("?");
    let native_template = prop.value.clone();
    prop.value = adapt_value(scale_value(value, scale), &native_template);
    if matches!(&native_template, Value::Layer(_)) && !matches!(&prop.value, Value::Layer(_)) {
        losses.push(format!("{context}: invalid AE layer selector on {raw}"));
    }
    if let Some(expr) = string(ir, &["expression"]).filter(|s| !s.is_empty()) {
        prop.expr = Some(effectcraft_project::Expression { text: expr.to_owned(), enabled: boolean(ir, &["expression_enabled"]).unwrap_or(false) });
    }
    for key in array(ir, "keyframes") {
        let Some(time) = number(key, &["time"]) else {
            losses.push(format!("{context}: invalid keyframe time on {raw}"));
            continue;
        };
        let Some(kval) = key.get("value").and_then(|v| convert_value(&remap_json(v, remap), value_type)) else {
            losses.push(format!("{context}: keyframe on {raw} has unsupported value"));
            continue;
        };
        let keyed = adapt_value(scale_value(kval, scale), &native_template);
        if matches!(&native_template, Value::Layer(_)) && !matches!(&keyed, Value::Layer(_)) {
            losses.push(format!("{context}: invalid AE keyed layer selector on {raw}"));
        }
        let mut k = Keyframe::new(layer_time.at(time), keyed);
        k.in_interp = interpolation(field(key, &["in_interpolation_type", "in_interpolation", "in_interp"]));
        k.out_interp = interpolation(field(key, &["out_interpolation_type", "out_interpolation", "out_interp"]));
        k.in_ease = eases(field(key, &["in_temporal_ease", "in_ease"]));
        k.out_ease = eases(field(key, &["out_temporal_ease", "out_ease"]));
        if let Some(v) = field(key, &["in_spatial_tangent", "spatial_in"]).and_then(vec3) {
            k.spatial_in = v;
            k.spatial_auto = false;
        }
        if let Some(v) = field(key, &["out_spatial_tangent", "spatial_out"]).and_then(vec3) {
            k.spatial_out = v;
            k.spatial_auto = false;
        }
        k.continuous = boolean(key, &["temporal_continuous"]).unwrap_or(false);
        k.auto_bezier = boolean(key, &["temporal_auto_bezier"]).unwrap_or(false);
        k.spatial_continuous = boolean(key, &["spatial_continuous"]).unwrap_or(false);
        k.spatial_auto = boolean(key, &["spatial_auto_bezier"]).unwrap_or(k.spatial_auto);
        k.roving = boolean(key, &["roving"]).unwrap_or(false);
        k.label = integer(key, &["label"]).unwrap_or(0).clamp(0, 255) as u8;
        // Reversed layers play the source curve backwards in layer time.
        if layer_time.reversed() {
            std::mem::swap(&mut k.in_interp, &mut k.out_interp);
            std::mem::swap(&mut k.in_ease, &mut k.out_ease);
            std::mem::swap(&mut k.spatial_in, &mut k.spatial_out);
        }
        let speed_scale = if prop.spatial || matches!(prop.value, Value::Color(_)) || prop.value.components().is_empty() {
            layer_time.stretch.abs() / 100.0
        } else {
            layer_time.stretch / 100.0
        };
        for ease in k.in_ease.iter_mut().chain(k.out_ease.iter_mut()) {
            ease.speed *= speed_scale;
        }
        prop.keys.push(k);
    }
    prop.keys.sort_by_key(|k| k.time);
    prop.keys.dedup_by_key(|k| k.time);
    if matches!(prop.value, Value::Color(_)) {
        color_ease::normalize(&mut prop.keys);
    }
}

fn scale_value(v: Value, scale: f64) -> Value {
    if let Value::Scalar(x) = v { Value::Scalar(x * scale) } else { v }
}

fn adapt_value(value: Value, native: &Value) -> Value {
    match (value, native) {
        (Value::Scalar(v), Value::Bool(_)) => Value::Bool(v != 0.0),
        (Value::Scalar(v), Value::Enum(_)) => Value::Enum(v.round().max(1.0) as u32 - 1),
        (Value::Scalar(0.0), Value::Layer(_)) => Value::Layer(None),
        (Value::Scalar(v), Value::Layer(_)) if v > 0.0 && v.fract() == 0.0 && v <= u32::MAX as f64 => Value::Layer(Some(v as u64)),
        (Value::Vec3(v), Value::Vec2(_)) => Value::Vec2([v[0], v[1]]),
        (Value::Vec2(v), Value::Vec3(_)) => Value::Vec3([v[0], v[1], 0.0]),
        (v, _) => v,
    }
}

fn resolve_layer_refs(prop: &mut Property, by_index: &BTreeMap<u64, Option<u64>>, losses: &mut Vec<String>, context: &str) {
    let mut unresolved = std::collections::BTreeSet::new();
    let mut resolve = |value: &mut Value| {
        let Value::Layer(Some(index)) = value else { return };
        if let Some(Some(uid)) = by_index.get(index) {
            *value = Value::Layer(Some(*uid));
        } else {
            unresolved.insert(*index);
            *value = Value::Scalar(*index as f64); // inert, but the original selector stays editable.
        }
    };
    resolve(&mut prop.value);
    for key in &mut prop.keys {
        resolve(&mut key.value);
    }
    for index in unresolved {
        losses.push(format!("{context}: unresolved AE layer index {index} on {}", prop.match_id));
    }
}

fn interpolation(j: Option<&Json>) -> Interp {
    if let Some(n) = j.and_then(Json::as_i64) {
        return match n {
            6613 => Interp::Bezier,
            6614 => Interp::Hold,
            _ => Interp::Linear,
        };
    }
    match j.and_then(Json::as_str).unwrap_or("").to_ascii_lowercase().as_str() {
        "bezier" => Interp::Bezier,
        "hold" => Interp::Hold,
        _ => Interp::Linear,
    }
}
fn eases(j: Option<&Json>) -> Vec<Ease> {
    j.and_then(Json::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|e| {
                    let e = e.get("data").unwrap_or(e);
                    Some(Ease { speed: number(e, &["speed"])?, influence: number(e, &["influence"])? / 100.0 })
                })
                .collect()
        })
        .unwrap_or_default()
}
fn vec3(j: &Json) -> Option<[f64; 3]> {
    let a = j.as_array()?;
    Some([a.first()?.as_f64()?, a.get(1)?.as_f64()?, a.get(2).and_then(Json::as_f64).unwrap_or(0.0)])
}
fn convert_value(j: &Json, ty: &str) -> Option<Value> {
    if let Some(tag) = string(j, &["type"]) {
        if tag == "TextDocument" {
            return convert_value(j.get("data")?, "text_document");
        }
        if tag == "Shape" {
            return convert_value(j.get("data")?, "shape");
        }
        if tag == "Gradient" {
            return convert_value(j.get("data")?, "gradient");
        }
        return None;
    }
    if let Some(v) = j.as_bool() {
        return Some(Value::Bool(v));
    }
    if let Some(v) = j.as_f64().filter(|v| v.is_finite()) {
        return Some(Value::Scalar(v));
    }
    if let Some(s) = j.as_str() {
        return Some(Value::Str(s.to_owned()));
    }
    if let Some(a) = j.as_array() {
        let nums: Vec<f64> = a.iter().map(Json::as_f64).collect::<Option<Vec<_>>>()?;
        if nums.iter().any(|v| !v.is_finite()) {
            return None;
        }
        return match (ty.to_ascii_lowercase().as_str(), nums.len()) {
            ("color", 3) => Some(Value::Color([nums[0], nums[1], nums[2], 1.0])),
            ("color", 4) => Some(Value::Color([nums[0], nums[1], nums[2], nums[3]])),
            (_, 2) => Some(Value::Vec2([nums[0], nums[1]])),
            (_, 3) => Some(Value::Vec3([nums[0], nums[1], nums[2]])),
            (_, 4) => Some(Value::Color([nums[0], nums[1], nums[2], nums[3]])),
            _ => None,
        };
    }
    if ty == "shape" || ty == "shape_path" {
        return shapes::value(j);
    }
    if ty == "text_document" {
        return text::value(j);
    }
    if ty == "gradient" {
        return gradient_value(j).or_else(|| Value::Gradient(Default::default()).coerce_json(j));
    }
    None
}

fn gradient_value(j: &Json) -> Option<Value> {
    let midpoint = |data: &Json| -> Option<f64> {
        let raw = if data.get("midpoint").is_some() { number(data, &["midpoint"])? } else { 0.5 };
        let value = if raw > 1.0 { raw / 100.0 } else { raw };
        (0.0..=1.0).contains(&value).then_some(value)
    };
    let colors_ir = j.get("color_stops")?.as_array()?;
    if !(2..=4096).contains(&colors_ir.len()) {
        return None;
    }
    let mut colors = Vec::with_capacity(colors_ir.len());
    for entry in colors_ir {
        let data = entry.get("data").unwrap_or(entry);
        let pos = number(data, &["offset"])?;
        if !(0.0..=1.0).contains(&pos) {
            return None;
        }
        let rgb = data.get("color")?.as_array()?;
        if rgb.len() < 3 {
            return None;
        }
        let mut rgba = [0.0f32; 4];
        for (i, channel) in rgba.iter_mut().enumerate().take(3) {
            *channel = rgb.get(i)?.as_f64().filter(|v| v.is_finite())? as f32;
        }
        rgba[3] = 1.0;
        colors.push((pos, rgba, midpoint(data)?));
    }
    let mut opacities = Vec::new();
    if let Some(alpha_ir) = j.get("alpha_stops").and_then(Json::as_array) {
        if alpha_ir.len() > 4096 {
            return None;
        }
        for entry in alpha_ir {
            let data = entry.get("data").unwrap_or(entry);
            let pos = number(data, &["offset"])?;
            if !(0.0..=1.0).contains(&pos) {
                return None;
            }
            opacities.push((pos, number(data, &["alpha"])? as f32, midpoint(data)?));
        }
    }
    if opacities.is_empty() {
        opacities = colors.iter().map(|(pos, _, _)| (*pos, 1.0, 0.5)).collect();
    }
    colors.sort_by(|a, b| a.0.total_cmp(&b.0));
    opacities.sort_by(|a, b| a.0.total_cmp(&b.0));
    let color_midpoints = colors.iter().map(|(_, _, m)| *m).collect();
    let opacity_midpoints = opacities.iter().map(|(_, _, m)| *m).collect();
    Some(Value::Gradient(Gradient {
        colors: colors.into_iter().map(|(pos, color, _)| (pos, color)).collect(),
        opacities: opacities.into_iter().map(|(pos, alpha, _)| (pos, alpha)).collect(),
        color_midpoints,
        opacity_midpoints,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The importer as it was when it parsed the whole document up front: every outcome must match.
    fn whole_document_outcome(bytes: &[u8]) -> Result<(String, String), String> {
        if json_depth(bytes, 128) > 128 {
            return Err(AepError::Invalid("IR nesting exceeds 128 levels".into()).to_string());
        }
        let doc: Json = serde_json::from_slice(bytes).map_err(|e| AepError::Invalid(e.to_string()).to_string())?;
        // A re-serialized document has no duplicate keys or skippable spans left.
        import_outcome(doc.to_string().as_bytes())
    }

    fn import_outcome(bytes: &[u8]) -> Result<(String, String), String> {
        let mut project = Project::default();
        let result = import_ir(&mut project, bytes, "", &mut |_| None).map_err(|e| e.to_string())?;
        Ok((project.to_file_json().map_err(|e| e.to_string())?, format!("{result:?}")))
    }

    fn fidelity_fixture() -> String {
        let keyed = |a: f64, b: f64| {
            format!(r#""keyframes":[{{"time":0.0,"value":{a},"in_temporal_ease":[{{"speed":1,"influence":33}}]}},{{"time":1.0,"value":{b}}}]"#)
        };
        format!(
            r#"{{"schema_version":1,"source":{{"path":"x.aep"}},"counts":{{"items":5}},"project":{{"bits_per_channel":16}},
            "losses":[{{"scope":"p","reason":"r","detail":"d"}},7],"media_issues":[{{"scope":"m"}}],
            "items":[{{"id":1,"kind":"comp","name":"shadowed","layers":[]}}],
            "items":[
              {{"id":0,"kind":"folder","name":"Root"}},
              {{"id":9,"kind":"folder","name":"Bin"}},
              {{"id":2,"kind":"footage","name":"model.glb","file":"/offline/model.glb","model_scale":0.5,"parent_id":9}},
              {{"id":3,"kind":"footage","name":"art.psd","file":"/offline/art.psd","source_layer":{{"index":2,"name":"Hand","layer_size":true,"crop_bounds":[1.0,2.0,30.0,40.0]}}}},
              {{"id":4,"kind":"comp","name":"Nested","width":100,"height":100,"duration":5,"frame_rate":30,"layers":[]}},
              {{"id":5,"kind":"comp","name":"Main","width":400,"height":200,"duration":5,"frame_rate":23.976,"unknown_field":{{"deep":[1,2,{{"x":null}}]}},
               "layers":[{{"id":99,"kind":"null","name":"shadowed layer"}}],
               "default_camera":null,
               "layers":[
                {{"id":10,"index":1,"kind":"text","name":"Card","three_d_layer":true,"parent_id":11,"matte":{{"layer_id":11,"type":5013}},"properties":[
                  {{"kind":"group","match_name":"ADBE Transform Group","children":[
                    {{"kind":"property","match_name":"ADBE Position","value":[10,20,0],"keyframes":[{{"time":0.5,"value":[1,2,0],"in_spatial_tangent":[1,0,0]}},{{"time":1.5,"value":[3,4,0]}}]}},
                    {{"kind":"property","match_name":"ADBE Opacity","value":50,"expression":"value","expression_enabled":true}}]}},
                  {{"kind":"group","match_name":"ADBE Material Options Group","children":[
                    {{"kind":"property","match_name":"ADBE Casts Shadows","value":1.0,{}}},
                    {{"kind":"property","match_name":"ADBE Accepts Shadows","value":2}}]}},
                  {{"kind":"group","match_name":"ADBE Text Properties","children":[
                    {{"kind":"property","match_name":"ADBE Text Document","value":{{"type":"TextDocument","data":{{"text":"Hi","font_size":50}}}}}},
                    {{"kind":"group","match_name":"ADBE Text Path Options","children":[
                      {{"kind":"property","match_name":"ADBE Text Path","authored":true,"value":1,{}}}]}}]}}]}},
                {{"id":11,"index":2,"kind":"ShapeLayer","name":"Dirs","properties":[{{"kind":"group","match_name":"ADBE Root Vectors Group","children":[
                  {{"kind":"group","match_name":"ADBE Vector Shape - Ellipse","children":[
                    {{"kind":"property","match_name":"ADBE Vector Shape Direction","value":3,{}}},
                    {{"kind":"property","match_name":"ADBE Vector Ellipse Size","value":[40,40]}}]}}]}}]}},
                {{"id":12,"index":3,"kind":"solid","name":"Effects","switches":{{"blending_mode":5}},"timing":{{"start_time":0.5,"stretch":-100}},"properties":[
                  {{"kind":"group","match_name":"ADBE Effect Parade","children":[
                    {{"kind":"group","match_name":"Keylight 906","name":"Keylight","children":[
                      {{"kind":"property","match_name":"Keylight 906-0023","value":1.0,{}}},
                      {{"kind":"property","match_name":"Keylight 906-0005","value":100.0}}]}},
                    {{"kind":"group","match_name":"ADBE Glo2","name":"Glow","children":[{{"kind":"property","match_name":"ADBE Glo2-0002","value":153.0}}]}},
                    {{"kind":"group","match_name":"CC Page Turn","name":"Page","children":[{{"kind":"property","match_name":"CC Page Turn-0001","value":4.0,{}}}]}},
                    {{"kind":"group","match_name":"ADBE Set Matte3","name":"Set Matte","children":[{{"kind":"property","match_name":"ADBE Set Matte3-0001","value":2.0}}]}}]}}]}},
                {{"id":13,"index":4,"kind":"footage","source_id":2,"name":"Model","three_d_layer":true,"properties":[
                  {{"kind":"group","match_name":"ADBE Material Options Group","children":[
                    {{"kind":"property","match_name":"ADBE Casts Shadows","value_type":6417,"value":2.0,{}}},
                    {{"kind":"property","match_name":"ADBE Accepts Lights","value_type":6417,"value":0.0}}]}}]}},
                {{"id":14,"index":5,"kind":"precomp","source_id":4,"name":"Nested","switches":{{"time_remap_enabled":false}},"properties":[
                  {{"kind":"property","match_name":"ADBE Time Remapping","value":0.0,{}}}]}},
                {{"id":15,"index":6,"kind":"footage","source_id":3,"name":"Art"}}
              ]}}
            ]}}"#,
            keyed(0.0, 2.0),
            keyed(0.0, 1.0),
            keyed(1.0, 3.0),
            keyed(0.0, 2.0),
            keyed(1.0, 4.0),
            keyed(0.0, 2.0),
            keyed(0.0, 1.0),
        )
    }

    #[test]
    fn item_at_a_time_import_matches_the_whole_document_parse() {
        let fixture = fidelity_fixture();
        let imported = import_outcome(fixture.as_bytes()).unwrap();
        assert_eq!(Ok(imported.clone()), whole_document_outcome(fixture.as_bytes()));
        // The fixture reaches every remapped property path, both item passes and duplicate keys.
        assert!(imported.0.contains("Hand") && !imported.0.contains("shadowed"), "{}", imported.1);
    }

    #[test]
    fn malformed_or_odd_ir_fails_as_the_whole_document_parse_did() {
        let layered = |layers: &str| format!(r#"{{"schema_version":1,"items":[{{"id":1,"kind":"comp","name":"C","layers":{layers}}}]}}"#);
        let deep = |n: usize| layered(&format!("[{{\"id\":2,\"x\":{}1{}}}]", "[".repeat(n), "]".repeat(n)));
        let mut cases: Vec<Vec<u8>> = [
            layered(r#"[{"id":2,"x":1e400}]"#),
            layered(r#"[{"id":2,"name":"\ud800"}]"#),
            layered(r#"[{"id":2,"name":"\x"}]"#),
            layered(r#"[{"id":2,}]"#),
            layered(r#"{"not":"an array"}"#),
            layered("[1,2"),
            deep(100),
            deep(120),
            deep(122),
            deep(123),
            deep(124),
            deep(125),
            deep(126),
            r#"{"schema_version":1,"counts":1e400,"items":[]}"#.into(),
            r#"{"schema_version":1,"items":[{"id":1,"layers":[1e400]}],"items":[]}"#.into(),
            r#"{"schema_version":1,"items":[{"id":1,"kind":"comp","layers":[1e400],"layers":[]}]}"#.into(),
            r#"{"schema_version":1,"items":[{"id":1,"kind":"comp","default_camera":{"a":1e400},"default_camera":null}]}"#.into(),
            r#"{"schema_version":1,"items":[{"id":1,"kind":"comp"},{"id":1,"layers":[1e400]}]}"#.into(),
            r#"{"schema_version":1,"items":[5,[1e400]]}"#.into(),
            r#"{"schema_version":1,"items":[5]}"#.into(),
            r#"{"schema_version":1,"items":[[1]]}"#.into(),
            r#"{"schema_version":1,"items":{"a":1}}"#.into(),
            r#"{"schema_version":1,"items":{"a":1e400}}"#.into(),
            r#"{"schema_version":1,"items":"x"}"#.into(),
            r#"{"schema_version":1}"#.into(),
            r#"{"schema_version":2,"items":[]}"#.into(),
            r#"{"items":[]}"#.into(),
            r#"{"schema_version":1,"items":[]} x"#.into(),
            "[1,2]".into(),
            "7".into(),
            "".into(),
        ]
        .into_iter()
        .map(String::into_bytes)
        .collect();
        let mut invalid_utf8 = layered(r#"[{"id":2,"name":"ab"}]"#).into_bytes();
        let at = invalid_utf8.iter().position(|b| *b == b'a').unwrap();
        invalid_utf8[at] = 0xff;
        cases.push(invalid_utf8);
        for case in cases {
            assert_eq!(import_outcome(&case), whole_document_outcome(&case), "{}", String::from_utf8_lossy(&case));
        }
    }

    #[test]
    fn remapped_keys_match_a_remapped_copy_of_the_ir() {
        // The importer once copied a property's IR with its values mapped; mapping on read must agree.
        fn remapped(ir: &Json, f: impl Fn(f64) -> f64) -> Json {
            let mut adjusted = ir.clone();
            if let Some(v) = adjusted.get("value").and_then(Json::as_f64) {
                adjusted["value"] = Json::from(f(v));
            }
            if let Some(keys) = adjusted.get_mut("keyframes").and_then(Json::as_array_mut) {
                for key in keys {
                    if let Some(v) = key.get("value").and_then(Json::as_f64) {
                        key["value"] = Json::from(f(v));
                    }
                }
            }
            adjusted
        }
        let ir = serde_json::json!({"match_name":"ADBE Text Path","value":1,"expression":"value","keyframes":[
            {"time":0.0,"value":0},{"time":0.5,"value":1.5e308},{"time":1.0,"value":[1,2]},{"time":1.5,"value":"x"},{"time":2.0},
            {"time":2.5,"value":true},{"time":3.0,"value":-1},{"time":3.5,"value":null}]});
        let funcs: [fn(f64) -> f64; 3] = [|v| v + 1.0, |v| v * 10.0, |v| if v >= 2.5 { 2.0 } else { 1.0 }];
        for f in funcs {
            for (template, stretch) in [(Value::Enum(0), 100.0), (Value::Scalar(0.0), -50.0), (Value::Layer(None), 100.0)] {
                let clock = LayerTime { start: Tick::ZERO, stretch };
                let (mut copied, mut mapped) = (Property::new(1, "p", "P", template.clone()), Property::new(1, "p", "P", template));
                let (mut copied_losses, mut mapped_losses) = (Vec::new(), Vec::new());
                fill_property(&mut copied, &remapped(&ir, f), Value::Scalar(f(1.0)), "", 1.0, clock, &mut copied_losses, "t");
                fill_property_mapped(&mut mapped, &ir, Value::Scalar(f(1.0)), "", 1.0, Some(&f), clock, &mut mapped_losses, "t");
                assert_eq!(copied, mapped);
                assert_eq!(copied_losses, mapped_losses);
            }
        }
    }

    /// Colorama's Output Cycle state becomes a custom palette (SceneYard Puppet Control
    /// Documentary: two live stops, black → blue, after four stale hue-cycle stops).
    #[test]
    fn colorama_cycle_state_becomes_the_palette() {
        let mut hex = String::from("00000000ff0000000000ffffff0000ff00005555ff00ff0000008000ff00ffff0000aaaaff0000ff0000d555ffff00ff");
        hex.push_str(&"0".repeat(1024 - hex.len()));
        hex.push_str("0000000200000000");
        assert_eq!(colorama_cycle_palette(&hex).as_deref(), Some("0.00000:0.0000,0.0000,0.0000,1.0000 0.99998:0.0000,0.0000,1.0000,1.0000"));
        assert_eq!(colorama_cycle_palette(&hex[..100]), None);
        let palette = effectcraft_effects::ColoramaPalette::parse(&colorama_cycle_palette(&hex).unwrap()).unwrap();
        let mid = palette.eval(0.5, true);
        assert!(mid[0] == 0.0 && (mid[2] - 0.5).abs() < 1e-3, "{mid:?}");
    }

    #[test]
    fn property_keys_follow_layer_start_and_stretch() {
        let ir = serde_json::json!({
            "match_name": "ADBE Opacity", "value": 0.0,
            "keyframes": [
                {"time": 0.96, "value": 10.0},
                {"time": 4.0, "value": 90.0}
            ]
        });
        for (start, stretch) in [(1.24, 100.0), (-1.24, 50.0), (1.24, -100.0)] {
            let clock = LayerTime { start: tick(start), stretch };
            let mut prop = Property::new(1, "opacity", "Opacity", Value::Scalar(0.0));
            let mut losses = Vec::new();
            fill_property(&mut prop, &ir, Value::Scalar(0.0), "", 1.0, clock, &mut losses, "test");
            assert!(losses.is_empty());
            assert_eq!(prop.keys.len(), 2);
            assert_eq!(prop.keys.iter().map(|k| k.time).collect::<Vec<_>>(), {
                let mut times = vec![clock.at(0.96), clock.at(4.0)];
                times.sort();
                times
            });
            assert_eq!(effectcraft_keyframe::evaluate(&prop.keys, clock.at(0.96), false), Some(Value::Scalar(10.0)));
            assert_eq!(effectcraft_keyframe::evaluate(&prop.keys, clock.at(4.0), false), Some(Value::Scalar(90.0)));
        }
    }

    #[test]
    fn reversed_stretch_preserves_bezier_curve_at_composition_times() {
        let ir = serde_json::json!({
            "match_name":"ADBE Opacity", "value":0.0,
            "keyframes":[
                {"time":1.0,"value":0.0,"out_interpolation_type":6613,"out_temporal_ease":[{"speed":20.0,"influence":40.0}]},
                {"time":3.0,"value":100.0,"in_interpolation_type":6613,"in_temporal_ease":[{"speed":40.0,"influence":30.0}]}
            ]
        });
        let mut baseline = Property::new(1, "opacity", "Opacity", Value::Scalar(0.0));
        let mut reversed = Property::new(2, "opacity", "Opacity", Value::Scalar(0.0));
        let mut losses = Vec::new();
        let normal_clock = LayerTime { start: Tick::ZERO, stretch: 100.0 };
        let reverse_clock = LayerTime { start: tick(4.0), stretch: -100.0 };
        fill_property(&mut baseline, &ir, Value::Scalar(0.0), "", 1.0, normal_clock, &mut losses, "test");
        fill_property(&mut reversed, &ir, Value::Scalar(0.0), "", 1.0, reverse_clock, &mut losses, "test");
        assert!(losses.is_empty());
        for comp_time in [1.0, 1.25, 1.5, 2.0, 2.75, 3.0] {
            let a = effectcraft_keyframe::evaluate(&baseline.keys, normal_clock.at(comp_time), false).and_then(|v| match v {
                Value::Scalar(x) => Some(x),
                _ => None,
            });
            let b = effectcraft_keyframe::evaluate(&reversed.keys, reverse_clock.at(comp_time), false).and_then(|v| match v {
                Value::Scalar(x) => Some(x),
                _ => None,
            });
            assert!(a.zip(b).is_some_and(|(a, b)| (a - b).abs() < 1e-6), "time {comp_time}: {a:?} vs {b:?}");
        }
    }

    #[test]
    fn precomp_time_remap_keys_use_layer_clock() {
        let ir = serde_json::json!({"schema_version":1,"items":[
            {"id":1,"kind":"comp","name":"Nested","width":100,"height":100,"duration":5,"frame_rate":30,"layers":[]},
            {"id":2,"kind":"comp","name":"Main","width":100,"height":100,"duration":5,"frame_rate":30,"layers":[
                {"id":3,"kind":"precomp","source_id":1,"name":"Nested","switches":{"time_remap_enabled":true},"timing":{"start_time":1.24,"in_point":0,"out_point":5,"stretch":50},
                 "properties":[{"kind":"property","match_name":"ADBE Time Remapping","name":"Time Remap","value":1.0,
                    "keyframes":[{"time":1.24,"value":0.0},{"time":2.24,"value":1.0}]}]},
                {"id":4,"kind":"precomp","source_id":1,"name":"Nested without remap","switches":{"time_remap_enabled":false},
                 "properties":[{"kind":"property","match_name":"ADBE Time Remapping","name":"Time Remap","value":0.0,"keyframes":[]}]},
                {"id":5,"kind":"precomp","source_id":1,"name":"Disabled animated remap","switches":{"time_remap_enabled":false},
                 "properties":[{"kind":"property","match_name":"ADBE Time Remapping","name":"Time Remap","value":0.0,
                    "keyframes":[{"time":0.0,"value":0.0},{"time":1.0,"value":1.0}]}]}
            ]}
        ]});
        let mut project = Project::default();
        let result = import_ir(&mut project, ir.to_string().as_bytes(), "", &mut |_| None).unwrap();
        let layer = &project.comp(result.item_ids[&2]).unwrap().layers[0];
        let remap = layer.props.get("timeRemap").unwrap();
        assert_eq!(remap.keys.iter().map(|k| k.time).collect::<Vec<_>>(), vec![tick(0.0), tick(2.0)]);
        assert!(project.comp(result.item_ids[&2]).unwrap().layers[1].props.get("timeRemap").is_none());
        assert!(project.comp(result.item_ids[&2]).unwrap().layers[2].props.get("timeRemap").is_none());
        assert_eq!(project.comp(result.item_ids[&2]).unwrap().layers[2].props.get("ADBE Time Remapping").unwrap().keys.len(), 2);
        assert!(result.losses.iter().any(|loss| loss.contains("disabled Time Remap animation")));
    }

    #[test]
    fn ae_layer_indices_resolve_to_stable_ids_with_zero_and_missing_safe() {
        let ir = serde_json::json!({"match_name":"ADBE Set Matte3-0001","value":2.0,
            "keyframes":[{"time":0.0,"value":0.0},{"time":1.0,"value":2.0}]});
        let clock = LayerTime { start: Tick::ZERO, stretch: 100.0 };
        let mut prop = Property::new(1, "takeMatteFromLayer", "Matte Layer", Value::Layer(None));
        let mut losses = Vec::new();
        fill_property(&mut prop, &ir, Value::Scalar(2.0), "", 1.0, clock, &mut losses, "test");
        assert_eq!(prop.value, Value::Layer(Some(2)));
        resolve_layer_refs(&mut prop, &BTreeMap::from([(2, Some(789))]), &mut losses, "test");
        assert_eq!(prop.value, Value::Layer(Some(789)));
        assert_eq!(prop.keys.iter().map(|k| k.value.clone()).collect::<Vec<_>>(), vec![Value::Layer(None), Value::Layer(Some(789))]);
        assert!(losses.is_empty());

        let mut unresolved = Property::new(2, "takeMatteFromLayer", "Matte Layer", Value::Layer(Some(5)));
        resolve_layer_refs(&mut unresolved, &BTreeMap::new(), &mut losses, "test");
        assert_eq!(unresolved.value, Value::Scalar(5.0));
        assert_eq!(losses.len(), 1);
    }

    #[test]
    fn imported_effect_layer_selector_resolves_after_later_layer() {
        let ir = serde_json::json!({"schema_version":1,"items":[
            {"id":1,"kind":"comp","name":"Main","width":100,"height":100,"duration":2,"frame_rate":30,"layers":[
                {"id":10,"index":1,"kind":"null","name":"Effect owner","properties":[
                    {"kind":"group","match_name":"ADBE Effect Parade","children":[
                        {"kind":"group","match_name":"ADBE Set Matte3","name":"Set Matte","children":[
                            {"kind":"property","match_name":"ADBE Set Matte3-0001","name":"Take Matte From Layer","value":2.0}
                        ]}
                    ]}
                ]},
                {"id":20,"index":2,"kind":"null","name":"Matte"}
            ]}
        ]});
        let mut project = Project::default();
        let result = import_ir(&mut project, ir.to_string().as_bytes(), "", &mut |_| None).unwrap();
        let comp = project.comp(result.item_ids[&1]).unwrap();
        let mut selected = None;
        comp.layers[0].props.walk("", &mut |_, prop| {
            if prop.match_id == "takeMatteFromLayer" {
                selected = Some(prop.value.clone());
            }
        });
        assert_eq!(selected, Some(Value::Layer(Some(comp.layers[1].id.0))));
    }

    #[test]
    fn unauthored_raw_fixed_point_effect_points_keep_the_native_default() {
        // SceneYard Photos Collage: CC Vignette's unauthored centre arrives as 54000, 96000.
        let mut project = Project::default();
        let spec = effectcraft_effects::find("ec.stylize.ccvignette").unwrap();
        let mut effect = effectcraft_effects::instantiate(spec, &mut build::Ids(&mut project.next_id), "CC Vignette", [1080.0, 1920.0]);
        let ir = serde_json::json!({"children":[
            {"kind":"property","match_name":"CS Vignette-0001","authored":true,"value":39.0},
            {"kind":"property","match_name":"CS Vignette-0003","authored":false,"value":[54000.0, 96000.0]}
        ]});
        let mut losses = Vec::new();
        import_native_effect_params(&mut project, &mut effect, &ir, &ir, spec.id, LayerTime { start: Tick::ZERO, stretch: 100.0 }, &mut losses, "test", 0);
        assert_eq!(effect.prop("center").unwrap().value, Value::Vec2([540.0, 960.0]));
        assert_eq!(effect.prop("amount").unwrap().value, Value::Scalar(39.0));
    }

    #[test]
    fn ae_fill_opacity_uses_native_percent_for_static_and_keys() {
        let mut project = Project::default();
        let spec = effectcraft_effects::find("ec.generate.fill").unwrap();
        let mut effect = effectcraft_effects::instantiate(spec, &mut build::Ids(&mut project.next_id), "Fill", [100.0, 100.0]);
        let ir = serde_json::json!({"children":[
            {"kind":"property","match_name":"ADBE Fill-0005","value":0.5,
             "keyframes":[{"time":0.0,"value":0.2},{"time":1.0,"value":1.0}]}
        ]});
        let mut losses = Vec::new();
        import_native_effect_params(&mut project, &mut effect, &ir, &ir, spec.id, LayerTime { start: Tick::ZERO, stretch: 100.0 }, &mut losses, "test", 0);
        let opacity = effect.get("opacity").unwrap();
        assert_eq!(opacity.value, Value::Scalar(50.0));
        assert_eq!(opacity.keys.iter().map(|k| k.value.clone()).collect::<Vec<_>>(), vec![Value::Scalar(20.0), Value::Scalar(100.0)]);
        assert!(losses.is_empty());
    }

    #[test]
    fn keylight_source_crop_edges_become_crop_amounts() {
        // AE's Right / Bottom are edge positions (100% = uncropped); the native controls crop that
        // much from the edge. SceneYard Apple Apps' untouched crops (0, 100, 0, 100) cropped
        // the whole background away.
        let mut project = Project::default();
        let spec = effectcraft_effects::find("ec.keying.keylight").unwrap();
        let mut effect = effectcraft_effects::instantiate(spec, &mut build::Ids(&mut project.next_id), "Keylight", [100.0, 100.0]);
        let ir = serde_json::json!({"children":[
            {"kind":"property","match_name":"Keylight 906-0075","value":0.0},
            {"kind":"property","match_name":"Keylight 906-0076","value":100.0},
            {"kind":"property","match_name":"Keylight 906-0077","value":10.0},
            {"kind":"property","match_name":"Keylight 906-0078","value":80.0,
             "keyframes":[{"time":0.0,"value":100.0},{"time":1.0,"value":75.0}]}
        ]});
        let mut losses = Vec::new();
        import_native_effect_params(&mut project, &mut effect, &ir, &ir, spec.id, LayerTime { start: Tick::ZERO, stretch: 100.0 }, &mut losses, "test", 0);
        let v = |path: &str| effect.prop(path).unwrap().value.clone();
        assert_eq!(v("sourceCrops/cropLeft"), Value::Scalar(0.0));
        assert_eq!(v("sourceCrops/cropRight"), Value::Scalar(0.0));
        assert_eq!(v("sourceCrops/cropTop"), Value::Scalar(10.0));
        assert_eq!(v("sourceCrops/cropBottom"), Value::Scalar(20.0));
        let keys: Vec<Value> = effect.prop("sourceCrops/cropBottom").unwrap().keys.iter().map(|k| k.value.clone()).collect();
        assert_eq!(keys, vec![Value::Scalar(0.0), Value::Scalar(25.0)]);
    }

    #[test]
    fn scribble_opacity_and_curviness_become_percentages() {
        // AE's untouched Scribble Opacity is stored as 1.0 (100%); read as 1% the SceneYard
        // Magnifying Glass scribble was invisible. Curviness 0.2 is 20%.
        let mut project = Project::default();
        let spec = effectcraft_effects::find("ec.generate.scribble").unwrap();
        let mut effect = effectcraft_effects::instantiate(spec, &mut build::Ids(&mut project.next_id), "Scribble", [100.0, 100.0]);
        let ir = serde_json::json!({"children":[
            {"kind":"property","match_name":"ADBE Scribble Fill-0024","value":1.0},
            {"kind":"property","match_name":"ADBE Scribble Fill-0012","value":0.2},
            {"kind":"property","match_name":"ADBE Scribble Fill-0066","value":0.19},
            {"kind":"property","match_name":"ADBE Scribble Fill-0060","value":3.4}
        ]});
        let mut losses = Vec::new();
        import_native_effect_params(&mut project, &mut effect, &ir, &ir, spec.id, LayerTime { start: Tick::ZERO, stretch: 100.0 }, &mut losses, "test", 0);
        let f = |path: &str| effect.prop(path).unwrap().value.as_f64();
        assert!((f("opacity") - 100.0).abs() < 1e-9);
        assert!((f("strokeOptions/curviness") - 20.0).abs() < 1e-9);
        assert!((f("strokeOptions/curvinessVariation") - 19.0).abs() < 1e-9);
        assert!((f("strokeOptions/spacing") - 3.4).abs() < 1e-9);
    }

    #[test]
    fn glow_threshold_and_midpoint_become_percentages() {
        // AE's default Glow Threshold 60% is stored as 153 (of 255); read as 153% nothing glowed.
        let mut project = Project::default();
        let spec = effectcraft_effects::find("ec.stylize.glow").unwrap();
        let mut effect = effectcraft_effects::instantiate(spec, &mut build::Ids(&mut project.next_id), "Glow", [100.0, 100.0]);
        let ir = serde_json::json!({"children":[
            {"kind":"property","match_name":"ADBE Glo2-0002","value":153.0},
            {"kind":"property","match_name":"ADBE Glo2-0011","value":0.5}
        ]});
        let mut losses = Vec::new();
        import_native_effect_params(&mut project, &mut effect, &ir, &ir, spec.id, LayerTime { start: Tick::ZERO, stretch: 100.0 }, &mut losses, "test", 0);
        let f = |path: &str| effect.prop(path).unwrap().value.as_f64();
        assert!((f("threshold") - 60.0).abs() < 1e-9);
        assert!((f("abMidpoint") - 50.0).abs() < 1e-9);
    }

    #[test]
    fn material_options_keep_ae_shadow_modes_and_specular() {
        // Glass UI List: AE's match names for Specular Intensity / Shininess are the
        // "Coefficient" ones, and Casts / Accepts Shadows count from 0 (Off).
        let ir = serde_json::json!({"schema_version":1,"items":[{"id":1,"kind":"comp","name":"Main","width":100,"height":100,"duration":2,"layers":[
            {"id":2,"kind":"text","name":"Card","three_d_layer":true,"properties":[
                {"kind":"group","match_name":"ADBE Material Options Group","children":[
                    {"kind":"property","match_name":"ADBE Casts Shadows","value":1.0,"keyframes":[{"time":0.0,"value":0.0},{"time":1.0,"value":2.0}]},
                    {"kind":"property","match_name":"ADBE Accepts Shadows","value":1.0},
                    {"kind":"property","match_name":"ADBE Specular Coefficient","value":70.0},
                    {"kind":"property","match_name":"ADBE Shininess Coefficient","value":90.0}
                ]}
            ]}
        ]}]});
        let mut project = Project::default();
        let result = import_ir(&mut project, ir.to_string().as_bytes(), "", &mut |_| None).unwrap();
        let layer = &project.comp(result.item_ids[&1]).unwrap().layers[0];
        let p = |path: &str| layer.props.prop(&format!("materialOptions/{path}")).unwrap();
        assert_eq!(p("castsShadows").value, Value::Enum(1));
        assert_eq!(p("castsShadows").keys.iter().map(|k| &k.value).collect::<Vec<_>>(), [&Value::Enum(0), &Value::Enum(2)]);
        assert_eq!(p("acceptsShadows").value, Value::Enum(1));
        assert_eq!(p("specularIntensity").value, Value::Scalar(70.0));
        assert_eq!(p("specularShininess").value, Value::Scalar(90.0));
    }

    #[test]
    fn active_text_animator_is_editable_and_declares_render_loss() {
        let ir = serde_json::json!({"schema_version":1,"items":[{"id":1,"kind":"comp","name":"Main","width":100,"height":100,"duration":2,"layers":[
            {"id":2,"kind":"text","name":"Title","properties":[
                {"kind":"group","match_name":"ADBE Text Animators","children":[
                    {"kind":"group","match_name":"ADBE Text Animator","name":"Animator 1","enabled":true,"children":[
                        {"kind":"property","match_name":"ADBE Text Selector Start","name":"Start","value":0.0,
                         "keyframes":[{"time":0.0,"value":0.0},{"time":1.0,"value":100.0}]}
                    ]}
                ]}
            ]}
        ]}]});
        let mut project = Project::default();
        let result = import_ir(&mut project, ir.to_string().as_bytes(), "", &mut |_| None).unwrap();
        assert_eq!(result.losses.iter().filter(|loss| loss.contains("Text Animators")).count(), 1);
        let layer = &project.comp(result.item_ids[&1]).unwrap().layers[0];
        let mut keyed = false;
        layer.props.walk("", &mut |_, prop| {
            if prop.match_id == "ADBE Text Selector Start" && prop.keys.len() == 2 {
                keyed = true;
            }
        });
        assert!(keyed);
    }

    #[test]
    fn authored_word_range_scale_and_blur_import_as_native_text_animator() {
        let ir = serde_json::json!({"schema_version":1,"items":[{"id":1,"kind":"comp","name":"Main","width":400,"height":200,"duration":2,"layers":[
            {"id":2,"kind":"text","name":"lesson 1","properties":[
                {"kind":"group","match_name":"ADBE Text Properties","children":[
                    {"kind":"property","match_name":"ADBE Text Document","value":{"type":"TextDocument","data":{"text":"lesson 1","font_size":100}}},
                    {"kind":"group","match_name":"ADBE Text Animators","children":[
                        {"kind":"group","match_name":"ADBE Text Animator","name":"Animator 1","enabled":true,"children":[
                            {"kind":"group","match_name":"ADBE Text Selectors","children":[
                                {"kind":"group","match_name":"ADBE Text Selector","name":"Range Selector 1","children":[
                                    {"kind":"property","match_name":"ADBE Text Percent Start","authored":true,"value":0.0,
                                     "keyframes":[{"time":0.0,"value":0.0},{"time":1.0,"value":100.0}]},
                                    {"kind":"group","match_name":"ADBE Text Range Advanced","children":[
                                        {"kind":"property","match_name":"ADBE Text Range Type2","authored":true,"value":3.0}
                                    ]}
                                ]}
                            ]},
                            {"kind":"group","match_name":"ADBE Text Animator Properties","children":[
                                {"kind":"property","match_name":"ADBE Text Scale 3D","authored":true,"value":[0.0,0.0,0.0]},
                                {"kind":"property","match_name":"ADBE Text Blur","authored":true,"value":[40.0,40.0]},
                                {"kind":"property","match_name":"ADBE Text Fill Color","authored":false,"value":[1.0,0.0,0.0,1.0]}
                            ]}
                        ]}
                    ]}
                ]}
            ]}
        ]}]});
        let mut project = Project::default();
        let result = import_ir(&mut project, ir.to_string().as_bytes(), "", &mut |_| None).unwrap();
        assert!(!result.losses.iter().any(|loss| loss.contains("Text Animators")), "{:?}", result.losses);
        let layer = &project.comp(result.item_ids[&1]).unwrap().layers[0];
        let start = layer.props.prop("text/animators/#1/selectors/#1/start").unwrap();
        assert_eq!(start.keys.len(), 2);
        assert_eq!(layer.props.prop("text/animators/#1/selectors/#1/advanced/basedOn").unwrap().value, Value::Enum(2));
        assert_eq!(layer.props.prop("text/animators/#1/properties/scale").unwrap().value, Value::Vec3([0.0; 3]));
        assert_eq!(layer.props.prop("text/animators/#1/properties/blur").unwrap().value, Value::Vec2([40.0; 2]));
        assert!(layer.props.prop("text/animators/#1/properties/fillColor").is_none());
        assert!(layer.props.prop("text/animators/#1/properties/ADBE Text Fill Color").is_some());
    }

    #[test]
    fn text_path_options_import_onto_the_native_group() {
        // SceneYard Money trap collage: "Money trap" runs along Mask 1 (AE Path = 1); it was
        // dropped into an unrecognised group and the text sat straight.
        let ir = serde_json::json!({"schema_version":1,"items":[{"id":1,"kind":"comp","name":"Main","width":400,"height":200,"duration":2,"layers":[
            {"id":2,"kind":"text","name":"Money trap","properties":[
                {"kind":"group","match_name":"ADBE Text Properties","children":[
                    {"kind":"property","match_name":"ADBE Text Document","value":{"type":"TextDocument","data":{"text":"Money trap","font_size":100}}},
                    {"kind":"group","match_name":"ADBE Text Path Options","children":[
                        {"kind":"property","match_name":"ADBE Text Path","authored":true,"value":1},
                        {"kind":"property","match_name":"ADBE Text Reverse Path","authored":true,"value":1.0},
                        {"kind":"property","match_name":"ADBE Text Perpendicular To Path","authored":true,"value":0.0},
                        {"kind":"property","match_name":"ADBE Text First Margin","authored":true,"value":12.0}
                    ]}
                ]}
            ]}
        ]}]});
        let mut project = Project::default();
        let result = import_ir(&mut project, ir.to_string().as_bytes(), "", &mut |_| None).unwrap();
        let layer = &project.comp(result.item_ids[&1]).unwrap().layers[0];
        let po = |id: &str| layer.props.prop(&format!("text/pathOptions/{id}")).map(|p| p.value.clone());
        assert_eq!(po("path"), Some(Value::Enum(1)));
        assert_eq!(po("reversePath"), Some(Value::Bool(true)));
        assert_eq!(po("perpendicular"), Some(Value::Bool(false)));
        assert_eq!(po("firstMargin"), Some(Value::Scalar(12.0)));
        let text = layer.props.sub("text").unwrap();
        assert_eq!(text.groups().filter(|g| g.match_id.contains("Path")).count(), 0);
    }

    #[test]
    fn model_layer_keeps_reference_and_declares_unverified_render() {
        let ir = serde_json::json!({"schema_version":1,"items":[
            {"id":1,"kind":"footage","name":"offline.glb","file":"offline.glb"},
            {"id":2,"kind":"comp","name":"Main","width":100,"height":100,"duration":2,"layers":[
                {"id":3,"kind":"ThreeDModelLayer","name":"Model","source_id":1}
            ]}
        ]});
        let mut project = Project::default();
        let result = import_ir(&mut project, ir.to_string().as_bytes(), "", &mut |_| None).unwrap();
        assert!(matches!(project.comp(result.item_ids[&2]).unwrap().layers[0].source, LayerSource::Model { .. }));
        assert_eq!(result.losses.iter().filter(|loss| loss.contains("3D model coordinate/render parity")).count(), 1);
    }

    fn structure(project: &Project) -> [usize; 9] {
        let mut counts = [0usize; 9]; // comps, visible layers/props/keys/markers, font docs, implicit cameras/keys
        for (_, comp) in project.comps() {
            counts[0] += 1;
            for layer in &comp.layers {
                let implicit = layer.comment.starts_with(IMPLICIT_CAMERA_COMMENT);
                if implicit {
                    counts[7] += 1;
                } else {
                    counts[1] += 1;
                    counts[4] += layer.markers.len();
                }
                let mut stack = vec![&layer.props];
                while let Some(group) = stack.pop() {
                    for node in &group.children {
                        match node {
                            Node::Group(g) => stack.push(g),
                            Node::Prop(p) => {
                                if implicit {
                                    counts[8] += p.keys.len();
                                    continue;
                                }
                                counts[2] += 1;
                                counts[3] += p.keys.len();
                                for value in std::iter::once(&p.value).chain(p.keys.iter().map(|k| &k.value)) {
                                    if let Value::Text(doc) = value {
                                        counts[5] += usize::from(doc.render_font.is_some() || doc.runs.iter().any(|r| r.style.render_font.is_some()));
                                        counts[6] += usize::from(doc.font_file.is_some() || doc.runs.iter().any(|r| r.style.font_file.is_some()));
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        counts
    }

    #[test]
    fn imports_ids_links_and_animated_transform() {
        let ir = serde_json::json!({"schema_version":1,"items":[
            {"id":10,"kind":"comp","name":"Main","width":1920,"height":1080,"duration":2,"frame_rate":30,"renderer":"ADBE Advanced 3d","layers":[
                {"id":5,"kind":"null","name":"Parent","timing":{"out_point":2}},
                {"id":7,"kind":"text","name":"Title","parent_id":5,"timing":{"out_point":2},"properties":[
                    {"kind":"group","match_name":"ADBE Transform Group","name":"Transform","children":[
                        {"kind":"property","match_name":"ADBE Position","name":"Position","value_type":"ThreeD_SPATIAL","value":[100,200,0],"dimensions_separated":false,"keyframes":[{"time":0,"value":[100,200,0]},{"time":1,"value":[300,400,0]}]},
                        {"kind":"property","match_name":"ADBE Position_0","name":"X Position","value":0,"keyframes":[]}
                    ]}
                ]}
            ]}
        ]});
        let mut project = Project::default();
        let result = import_ir(&mut project, ir.to_string().as_bytes(), "", &mut |_| None).unwrap();
        let comp = project.comp(result.item_ids[&10]).unwrap();
        assert_eq!(comp.layers.len(), 2);
        assert_eq!(comp.renderer, effectcraft_project::Renderer::Classic3D);
        assert_eq!(comp.layers[1].parent, Some(result.layer_ids[&(10, 5)]));
        assert_eq!(comp.layers[1].transform().unwrap().get("position").unwrap().keys.len(), 2);
        assert!(comp.layers[1].transform().unwrap().get("positionX").is_none());
    }

    #[test]
    fn failure_does_not_mutate_project() {
        let mut project = Project::default();
        let before = project.clone();
        assert!(import_ir(&mut project, br#"{"schema_version":2,"items":[]}"#, "", &mut |_| None).is_err());
        assert_eq!(project, before);
    }

    #[test]
    fn calder_renderer_maps_to_advanced_3d() {
        let ir = serde_json::json!({"schema_version":1,"items":[{"id":1,"kind":"comp","name":"3D","width":100,"height":100,"duration":1,"renderer":"ADBE Calder","layers":[]}]});
        let mut project = Project::default();
        let result = import_ir(&mut project, ir.to_string().as_bytes(), "", &mut |_| None).unwrap();
        assert_eq!(project.comp(result.primary_comp.unwrap()).unwrap().renderer, effectcraft_project::Renderer::Advanced3D);
    }

    #[test]
    fn implicit_camera_is_editable_below_explicit_cameras() {
        let ir = serde_json::json!({"schema_version":1,"items":[{"id":1,"kind":"comp","name":"C","width":100,"height":100,"duration":2,
            "default_camera":{"id":9,"kind":"CameraLayer","implicit":true,"pose":{"position":[50,-40,-300],"point_of_interest":[50,-40,0],"zoom":300},"switches":{"enabled":true,"is_3d":true,"auto_orient":4214},"properties":[]},
            "layers":[{"id":2,"kind":"CameraLayer","name":"Explicit","timing":{"in_point":0,"out_point":1},"switches":{"auto_orient":4212},"properties":[]}]}]});
        let mut project = Project::default();
        let result = import_ir(&mut project, ir.to_string().as_bytes(), "", &mut |_| None).unwrap();
        let comp = project.comp(result.primary_comp.unwrap()).unwrap();
        assert_eq!(comp.layers.len(), 2);
        assert_eq!(comp.active_camera(tick(0.5)).unwrap().id, result.layer_ids[&(1, 2)]);
        let camera = comp.active_camera(tick(1.5)).unwrap();
        assert_eq!(camera.id, result.implicit_camera_ids[&(1, 9)]);
        assert!(camera.comment.starts_with(IMPLICIT_CAMERA_COMMENT));
        assert_eq!(camera.transform().unwrap().get("position").unwrap().value.as_vec3(), [50.0, -40.0, -300.0]);
        assert_eq!(camera.transform().unwrap().get("poi").unwrap().value.as_vec3(), [50.0, -40.0, 0.0]);
        assert_eq!(camera.props.sub("cameraOptions").unwrap().get("zoom").unwrap().value.as_f64(), 300.0);
        assert_eq!(structure(&project)[1], 1);
        assert_eq!(structure(&project)[7], 1);
    }

    #[test]
    fn unauthored_implicit_camera_eye_uses_zoom_depth() {
        let mut ir = serde_json::json!({"schema_version":1,"items":[{"id":1,"kind":"comp","name":"C","width":100,"height":100,"duration":1,
            "default_camera":{"id":9,"kind":"CameraLayer","implicit":true,
                "pose":{"position":[50,50,0],"point_of_interest":[50,50,0],"zoom":300},
                "switches":{"enabled":true,"is_3d":true,"auto_orient":4214},
                "properties":[{"kind":"group","match_name":"ADBE Transform Group","children":[
                    {"kind":"property","match_name":"ADBE Position","authored":false,"value":[50,50,0],"keyframes":[]}]}]},
            "layers":[]}]});
        let mut project = Project::default();
        let result = import_ir(&mut project, ir.to_string().as_bytes(), "", &mut |_| None).unwrap();
        let camera = project.comp(result.primary_comp.unwrap()).unwrap().active_camera(Tick::ZERO).unwrap();
        assert_eq!(camera.transform().unwrap().get("position").unwrap().value.as_vec3(), [50.0, 50.0, -300.0]);
        ir["items"][0]["default_camera"]["properties"][0]["children"][0]["authored"] = Json::Bool(true);
        let mut project = Project::default();
        let result = import_ir(&mut project, ir.to_string().as_bytes(), "", &mut |_| None).unwrap();
        let camera = project.comp(result.primary_comp.unwrap()).unwrap().active_camera(Tick::ZERO).unwrap();
        assert_eq!(camera.transform().unwrap().get("position").unwrap().value.as_vec3(), [50.0, 50.0, 0.0]);
    }

    #[test]
    fn unauthored_explicit_camera_eye_uses_zoom_depth() {
        // 3D Items Slider / Dental Carousel: an explicit two-node camera whose Position was never
        // set reports Position == Point of Interest; left there, every z = 0 layer is clipped.
        let mut ir = serde_json::json!({"schema_version":1,"items":[{"id":1,"kind":"comp","name":"C","width":1920,"height":1080,"duration":1,
            "layers":[{"id":2,"kind":"CameraLayer","name":"Camera","timing":{"in_point":0,"out_point":1},"switches":{"auto_orient":4214},"properties":[
                {"kind":"group","match_name":"ADBE Transform Group","children":[
                    {"kind":"property","match_name":"ADBE Anchor Point","authored":true,"value":[960,540,0],"keyframes":[]},
                    {"kind":"property","match_name":"ADBE Position","authored":false,"value":[960,540,0],"keyframes":[]}]},
                {"kind":"group","match_name":"ADBE Camera Options Group","children":[
                    {"kind":"property","match_name":"ADBE Camera Zoom","authored":true,"value":1500,"keyframes":[]}]}]}]}]});
        let eye = |ir: &Json| {
            let mut project = Project::default();
            let result = import_ir(&mut project, ir.to_string().as_bytes(), "", &mut |_| None).unwrap();
            let camera = project.comp(result.primary_comp.unwrap()).unwrap().active_camera(Tick::ZERO).unwrap().clone();
            camera.transform().unwrap().get("position").unwrap().value.as_vec3()
        };
        assert_eq!(eye(&ir), [960.0, 540.0, -1500.0]);
        // An authored Position is kept exactly, even when it equals the Point of Interest.
        ir["items"][0]["layers"][0]["properties"][0]["children"][1]["authored"] = Json::Bool(true);
        assert_eq!(eye(&ir), [960.0, 540.0, 0.0]);
    }

    /// Light Options were kept under their AE ids and never reached the renderer, so every
    /// imported light rendered at 100 % without falloff (SceneYard Glass UI List: 600–1500 %
    /// point lights with Smooth falloff).
    #[test]
    fn light_options_use_ae_ids() {
        let ir = serde_json::json!({"schema_version":1,"items":[{"id":1,"kind":"comp","name":"Lit","width":100,"height":100,"duration":2,"layers":[
            {"id":2,"kind":"LightLayer","light_type":4414,"name":"Point Light 1","properties":[
                {"kind":"group","match_name":"ADBE Light Options Group","children":[
                    {"kind":"property","match_name":"ADBE Light Intensity","value":600.0},
                    {"kind":"property","match_name":"ADBE Light Color","value":[1.0,0.5,0.25,1.0]},
                    {"kind":"property","match_name":"ADBE Light Falloff Type","value":2.0},
                    {"kind":"property","match_name":"ADBE Light Falloff Start","value":250.0},
                    {"kind":"property","match_name":"ADBE Light Falloff Distance","value":2000.0}
                ]}
            ]}
        ]}]});
        let mut project = Project::default();
        let result = import_ir(&mut project, ir.to_string().as_bytes(), "", &mut |_| None).unwrap();
        let layer = &project.comp(result.primary_comp.unwrap()).unwrap().layers[0];
        let opts = layer.props.sub("lightOptions").unwrap();
        assert_eq!(opts.get("intensity").unwrap().value.as_f64(), 600.0);
        assert_eq!(opts.get("color").unwrap().value, Value::Color([1.0, 0.5, 0.25, 1.0]));
        assert_eq!(opts.get("falloff").unwrap().value, Value::Enum(1));
        assert_eq!(opts.get("radius").unwrap().value.as_f64(), 250.0);
        assert_eq!(opts.get("falloffDistance").unwrap().value.as_f64(), 2000.0);
        assert!(opts.get("ADBE Light Intensity").is_none());
        // Environment lights keep their native Intensity (AE's value stays on the layer).
        let mut ir = ir;
        ir["items"][0]["layers"][0]["light_type"] = serde_json::json!(4416);
        let mut project = Project::default();
        let result = import_ir(&mut project, ir.to_string().as_bytes(), "", &mut |_| None).unwrap();
        let layer = &project.comp(result.primary_comp.unwrap()).unwrap().layers[0];
        let opts = layer.props.sub("lightOptions").unwrap();
        assert_eq!(opts.get("intensity").unwrap().value.as_f64(), 100.0);
        assert_eq!(opts.get("AE intensity").unwrap().value.as_f64(), 600.0);
    }

    #[test]
    fn classic_3d_orientation_is_rewritten_z_first() {
        // SceneYard Chess Strategy (Classic 3D): the king's Orientation 180°, 0°, 287° composes Z
        // first in AE; in EffectCraft's X-first order that is 180°, 0°, 73° (Rz·Rx(180) = Rx(180)·Rz(-z)).
        let ir = serde_json::json!({"schema_version":1,"items":[{"id":1,"kind":"comp","name":"Main","width":100,"height":100,"duration":1,"renderer":"ADBE Advanced 3d","layers":[
            {"id":2,"kind":"null","name":"king","switches":{"is_3d":true},"properties":[
                {"kind":"group","match_name":"ADBE Transform Group","children":[{"kind":"property","match_name":"ADBE Orientation","value":[180.0,0.0,287.0]}]}
            ]}
        ]}]});
        let mut project = Project::default();
        let result = import_ir(&mut project, ir.to_string().as_bytes(), "", &mut |_| None).unwrap();
        let layer = &project.comp(result.item_ids[&1]).unwrap().layers[0];
        let Value::Vec3(o) = layer.props.prop("transform/orientation").unwrap().value else { panic!("orientation") };
        assert!((o[0] - 180.0).abs() < 1e-6 && o[1].abs() < 1e-6 && (o[2] - 73.0).rem_euclid(360.0) < 1e-6, "{o:?}");
    }

    #[test]
    fn camera_options_and_one_node_orientation_use_ae_ids() {
        let mut ir = serde_json::json!({"schema_version":1,"items":[{"id":1,"kind":"comp","name":"Camera Comp","width":2160,"height":3840,"duration":2,"layers":[{"id":2,"kind":"CameraLayer","name":"Camera","switches":{"auto_orient":4212},"properties":[
            {"kind":"group","match_name":"ADBE Transform Group","children":[{"kind":"property","match_name":"ADBE Position","value":[0,0,-1000]}]},
            {"kind":"group","match_name":"ADBE Camera Options Group","children":[
                {"kind":"property","match_name":"ADBE Camera Zoom","value":1500},
                {"kind":"property","match_name":"ADBE Camera Focus Distance","value":1500},
                {"kind":"property","match_name":"ADBE Iris Highlight Threshold","value":1}
            ]}
        ]}]}]});
        let mut project = Project::default();
        let result = import_ir(&mut project, ir.to_string().as_bytes(), "", &mut |_| None).unwrap();
        let layer = &project.comp(result.primary_comp.unwrap()).unwrap().layers[0];
        assert_eq!(layer.auto_orient, effectcraft_project::AutoOrient::Off);
        let opts = layer.props.sub("cameraOptions").unwrap();
        assert_eq!(opts.get("zoom").unwrap().value.as_f64(), 1500.0);
        assert_eq!(opts.get("focusDistance").unwrap().value.as_f64(), 1500.0);
        assert_eq!(opts.get("highlightThreshold").unwrap().value.as_f64(), 255.0);
        assert!(opts.get("ADBE Camera Zoom").is_none());
        ir["items"][0]["layers"][0]["switches"]["auto_orient"] = serde_json::json!(4214);
        ir["items"][0]["layers"][0]["properties"][0]["children"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({"kind":"property","match_name":"ADBE Anchor Point","name":"Point of Interest","value":[1080,1920,0]}));
        let mut project = Project::default();
        let result = import_ir(&mut project, ir.to_string().as_bytes(), "", &mut |_| None).unwrap();
        let layer = &project.comp(result.primary_comp.unwrap()).unwrap().layers[0];
        assert_eq!(layer.auto_orient, effectcraft_project::AutoOrient::TowardsPointOfInterest);
        assert_eq!(layer.transform().unwrap().get("poi").unwrap().value.as_vec3(), [1080.0, 1920.0, 0.0]);
        assert!(layer.transform().unwrap().get("anchor").is_none());
    }

    #[test]
    fn gradient_stop_midpoints_survive_import() {
        let ir = serde_json::json!({"type":"Gradient","data":{"color_stops":[
            {"data":{"offset":0.0,"midpoint":0.25,"color":[1,0,0]}},
            {"data":{"offset":0.5,"midpoint":75.0,"color":[0,1,0]}},
            {"data":{"offset":1.0,"midpoint":0.5,"color":[0,0,1]}}
        ],"alpha_stops":[{"data":{"offset":0.0,"midpoint":0.4,"alpha":1.0}},{"data":{"offset":1.0,"midpoint":0.5,"alpha":0.0}}]}});
        let Value::Gradient(g) = convert_value(&ir, "").unwrap() else { panic!("expected gradient") };
        assert_eq!(g.color_midpoints, [0.25, 0.75, 0.5]);
        assert_eq!(g.opacity_midpoints, [0.4, 0.5]);
    }

    #[test]
    fn no_value_effect_placeholder_is_inert_only_at_zero() {
        let mut project = Project::default();
        let spec = effectcraft_effects::find("ec.perspective.dropshadow").unwrap();
        let mut effect = effectcraft_effects::instantiate(spec, &mut build::Ids(&mut project.next_id), "Drop Shadow", [100.0, 100.0]);
        let mut losses = Vec::new();
        let ir = serde_json::json!({"children":[{"kind":"property","match_name":"ADBE Drop Shadow-9999","value_type":6412,"value":0,"expression":"","keyframes":[]}]});
        import_native_effect_params(&mut project, &mut effect, &ir, &ir, spec.id, LayerTime { start: Tick::ZERO, stretch: 100.0 }, &mut losses, "test", 0);
        assert!(losses.is_empty());
        assert!(effect.get("ADBE Drop Shadow-9999").is_none());
        let changed = serde_json::json!({"children":[{"kind":"property","match_name":"ADBE Drop Shadow-9999","value_type":6412,"value":1,"expression":"","keyframes":[]}]});
        import_native_effect_params(
            &mut project,
            &mut effect,
            &changed,
            &changed,
            spec.id,
            LayerTime { start: Tick::ZERO, stretch: 100.0 },
            &mut losses,
            "test",
            0,
        );
        assert_eq!(losses.len(), 1);
        assert!(effect.get("ADBE Drop Shadow-9999").is_some());
    }

    #[test]
    fn shape_and_mask_use_native_editable_tree() {
        let shape = serde_json::json!({"type":"Shape","data":{"vertices":[[0,0],[10,0],[10,10]],"in_tangents":[[0,0],[0,0],[0,0]],"out_tangents":[[0,0],[0,0],[0,0]],"closed":true,"feather_points":[]}});
        let ir = serde_json::json!({"schema_version":1,"items":[{"id":1,"kind":"comp","name":"Main","width":100,"height":100,"duration":1,"layers":[
            {"id":2,"kind":"ShapeLayer","name":"Vector","properties":[{"kind":"group","match_name":"ADBE Root Vectors Group","children":[
                {"kind":"group","match_name":"ADBE Vector Shape - Group","name":"Path 1","children":[{"kind":"property","match_name":"ADBE Vector Shape","value":shape}]},
                {"kind":"group","match_name":"ADBE Vector Graphic - Fill","name":"Fill 1","children":[{"kind":"property","match_name":"ADBE Vector Fill Color","value":[1,0,0,1]}]}
            ]},{"kind":"group","match_name":"ADBE Mask Parade","children":[{"kind":"group","match_name":"ADBE Mask Atom","name":"Mask 1","children":[{"kind":"property","match_name":"ADBE Mask Shape","value":shape}]}]}]}
        ]}]});
        let mut project = Project::default();
        let result = import_ir(&mut project, ir.to_string().as_bytes(), "", &mut |_| None).unwrap();
        let layer = &project.comp(result.primary_comp.unwrap()).unwrap().layers[0];
        let contents = layer.props.sub("contents").unwrap();
        assert_eq!(contents.groups().map(|g| g.match_id.as_str()).collect::<Vec<_>>(), ["path", "fill"]);
        assert!(matches!(contents.sub("path").unwrap().get("path").unwrap().value, Value::Path(_)));
        assert!(matches!(layer.props.sub("masks").unwrap().sub("mask").unwrap().kind, GroupKind::Mask { .. }));
        let image = effectcraft_render::render_frame(&project, result.primary_comp.unwrap(), Tick::ZERO, 1.0);
        assert!(image.data.iter().any(|pixel| pixel[3] > 0.5));
    }

    #[test]
    fn shape_repeater_transform_imports_offset_scale_and_opacity() {
        // SceneYard UI Calendar Task: the dots repeat 196 px apart, not at the native 100 px default.
        let ir = serde_json::json!({"schema_version":1,"items":[{"id":1,"kind":"comp","name":"Main","width":400,"height":100,"duration":1,"layers":[
            {"id":2,"kind":"ShapeLayer","name":"Dots","properties":[{"kind":"group","match_name":"ADBE Root Vectors Group","children":[
                {"kind":"group","match_name":"ADBE Vector Shape - Ellipse","children":[{"kind":"property","match_name":"ADBE Vector Ellipse Size","value":[10,10]}]},
                {"kind":"group","match_name":"ADBE Vector Graphic - Fill","children":[{"kind":"property","match_name":"ADBE Vector Fill Color","value":[1,1,1,1]}]},
                {"kind":"group","match_name":"ADBE Vector Filter - Repeater","children":[
                    {"kind":"property","match_name":"ADBE Vector Repeater Copies","value":2},
                    {"kind":"group","match_name":"ADBE Vector Repeater Transform","children":[
                        {"kind":"property","match_name":"ADBE Vector Repeater Anchor","value":[0,0]},
                        {"kind":"property","match_name":"ADBE Vector Repeater Position","value":[196,0]},
                        {"kind":"property","match_name":"ADBE Vector Repeater Scale","value":[50,50]},
                        {"kind":"property","match_name":"ADBE Vector Repeater Rotation","value":15},
                        {"kind":"property","match_name":"ADBE Vector Repeater Opacity 1","value":40},
                        {"kind":"property","match_name":"ADBE Vector Repeater Opacity 2","value":80}
                    ]}
                ]}
            ]}]}
        ]}]});
        let mut project = Project::default();
        let result = import_ir(&mut project, ir.to_string().as_bytes(), "", &mut |_| None).unwrap();
        let layer = &project.comp(result.primary_comp.unwrap()).unwrap().layers[0];
        let transform = layer.props.sub("contents").unwrap().sub("repeater").unwrap().sub("transform").unwrap();
        assert_eq!(transform.prop("position").unwrap().value, Value::Vec2([196.0, 0.0]));
        assert_eq!(transform.prop("anchor").unwrap().value, Value::Vec2([0.0, 0.0]));
        assert_eq!(transform.prop("scale").unwrap().value, Value::Vec2([50.0, 50.0]));
        assert_eq!(transform.prop("rotation").unwrap().value.as_f64(), 15.0);
        assert_eq!(transform.prop("startOpacity").unwrap().value.as_f64(), 40.0);
        assert_eq!(transform.prop("endOpacity").unwrap().value.as_f64(), 80.0);
    }

    #[test]
    fn ntsc_comp_rate_keeps_ae_decimal_grid() {
        // SceneYard TV Channel Glitch cuts cameras at 3.003003 s = frame 72 of AE's 23.976 fps.
        let ir = serde_json::json!({"schema_version":1,"items":[{"id":1,"kind":"comp","name":"Main","width":100,"height":100,"duration":5,"frame_rate":23.9759979248,"layers":[
            {"id":2,"kind":"ShapeLayer","name":"Camera 2","timing":{"start_time":0,"in_point":1.5015015015015014,"out_point":3.003003003}},
            {"id":3,"kind":"ShapeLayer","name":"Camera 3","timing":{"start_time":3.003003003,"in_point":3.003003003,"out_point":5}}
        ]}]});
        let mut project = Project::default();
        let result = import_ir(&mut project, ir.to_string().as_bytes(), "", &mut |_| None).unwrap();
        let comp = project.comp(result.primary_comp.unwrap()).unwrap();
        assert_eq!(comp.frame_rate, FrameRate::new(2997, 125));
        let (before, cut) = (comp.frame_rate.tick_of(71), comp.frame_rate.tick_of(72));
        let layer = |name: &str| comp.layers.iter().find(|l| l.name == name).unwrap();
        assert!(layer("Camera 2").is_active_at(before) && !layer("Camera 2").is_active_at(cut));
        assert!(!layer("Camera 3").is_active_at(before) && layer("Camera 3").is_active_at(cut));
        assert!(layer("Camera 3").is_active_at(Tick::from_seconds_f64(72.0 / comp.frame_rate.as_f64())));
        assert_eq!(ae_frame_rate(Some(29.9699993134)), FrameRate::new(2997, 100));
        assert_eq!(ae_frame_rate(Some(f64::INFINITY)), FrameRate::FPS_30);
        assert_eq!(ae_frame_rate(Some(1e300)), FrameRate::FPS_30);
        assert_eq!(ae_frame_rate(Some(0.0)), FrameRate::FPS_30);
    }

    #[test]
    fn shape_stroke_popup_imports_butt_cap_and_keyed_round_cap() {
        let path =
            serde_json::json!({"type":"Shape","data":{"vertices":[[-20,0],[20,0]],"in_tangents":[[0,0],[0,0]],"out_tangents":[[0,0],[0,0]],"closed":false}});
        let ir = serde_json::json!({"schema_version":1,"items":[{"id":1,"kind":"comp","name":"Main","width":100,"height":100,"duration":2,"layers":[
        {"id":2,"kind":"ShapeLayer","name":"Line","properties":[{"kind":"group","match_name":"ADBE Root Vectors Group","children":[
            {"kind":"group","match_name":"ADBE Vector Shape - Group","children":[{"kind":"property","match_name":"ADBE Vector Shape","value":path}]},
            {"kind":"group","match_name":"ADBE Vector Graphic - Stroke","children":[
                {"kind":"property","match_name":"ADBE Vector Stroke Color","value":[1,0,0,1]},
                {"kind":"property","match_name":"ADBE Vector Stroke Width","value":10},
                {"kind":"property","match_name":"ADBE Vector Stroke Line Cap","value":1,"keyframes":[{"time":0,"value":1},{"time":1,"value":2}]},
                {"kind":"group","match_name":"ADBE Vector Stroke Dashes","authored":true,"children":[
                    {"kind":"property","match_name":"ADBE Vector Stroke Dash 1","authored":false,"value":10},
                    {"kind":"property","match_name":"ADBE Vector Stroke Gap 1","authored":false,"value":10}
                ]}
            ]}
        ]}]}]}]});
        let mut project = Project::default();
        let result = import_ir(&mut project, ir.to_string().as_bytes(), "", &mut |_| None).unwrap();
        let comp = result.primary_comp.unwrap();
        let stroke = project.comp(comp).unwrap().layers[0].props.sub("contents").unwrap().sub("stroke").unwrap();
        let cap = stroke.prop("cap").unwrap();
        assert_eq!(cap.value, Value::Enum(0));
        assert_eq!(cap.keys.iter().map(|k| &k.value).collect::<Vec<_>>(), [&Value::Enum(0), &Value::Enum(1)]);
        assert!(stroke.sub("dashes").unwrap().children.is_empty());
        let frame = effectcraft_render::render_frame(&project, comp, Tick::ZERO, 1.0);
        assert!(frame.data.iter().any(|pixel| pixel[3] > 0.5));
    }

    #[test]
    fn shape_path_direction_reversed_imports_as_reversed() {
        let ellipse = |d: u32| {
            serde_json::json!({"kind":"group","match_name":"ADBE Vector Shape - Ellipse","children":[
                {"kind":"property","match_name":"ADBE Vector Shape Direction","value":d},
                {"kind":"property","match_name":"ADBE Vector Ellipse Size","value":[40,40]}
            ]})
        };
        let ir = serde_json::json!({"schema_version":1,"items":[{"id":1,"kind":"comp","name":"Main","width":100,"height":100,"duration":2,"layers":[
        {"id":2,"kind":"ShapeLayer","name":"Dirs","properties":[{"kind":"group","match_name":"ADBE Root Vectors Group","children":[ellipse(1), ellipse(3), ellipse(2)]}]}]}]});
        let mut project = Project::default();
        let result = import_ir(&mut project, ir.to_string().as_bytes(), "", &mut |_| None).unwrap();
        let comp = result.primary_comp.unwrap();
        let contents = project.comp(comp).unwrap().layers[0].props.sub("contents").unwrap();
        let dirs: Vec<Value> = contents.groups().filter_map(|g| g.get("direction")).map(|p| p.value.clone()).collect();
        assert_eq!(dirs, [Value::Enum(0), Value::Enum(1), Value::Enum(0)], "AE 1 Default, 3 Reversed, 2 normal");
    }

    #[test]
    fn shape_gradient_controls_render_saved_colors_and_keep_popup_keys() {
        let gradient = serde_json::json!({"type":"Gradient","data":{
            "color_stops":[
                {"data":{"offset":0,"midpoint":0.5,"color":[0.02,0.05,0.1]}},
                {"data":{"offset":1,"midpoint":0.5,"color":[0.12,0.15,0.2]}}
            ],"alpha_stops":[{"data":{"offset":0,"midpoint":0.5,"alpha":1}},{"data":{"offset":1,"midpoint":0.5,"alpha":1}}]
        }});
        let ir = serde_json::json!({"schema_version":1,"items":[{"id":1,"kind":"comp","name":"Gradient","width":100,"height":100,"duration":2,"layers":[
            {"id":2,"kind":"ShapeLayer","name":"Panel","timing":{"out_point":2},"properties":[
                {"kind":"group","match_name":"ADBE Root Vectors Group","children":[
                    {"kind":"group","match_name":"ADBE Vector Shape - Rect","children":[
                        {"kind":"property","match_name":"ADBE Vector Rect Size","value":[100,100]}
                    ]},
                    {"kind":"group","match_name":"ADBE Vector Graphic - G-Fill","children":[
                        {"kind":"property","match_name":"ADBE Vector Grad Colors","value":gradient},
                        {"kind":"property","match_name":"ADBE Vector Grad Type","value":1,"keyframes":[{"time":0,"value":1},{"time":1,"value":2}]},
                        {"kind":"property","match_name":"ADBE Vector Grad Start Pt","value":[-50,0]},
                        {"kind":"property","match_name":"ADBE Vector Grad End Pt","value":[50,0]},
                        {"kind":"property","match_name":"ADBE Vector Grad HiLite Length","value":20},
                        {"kind":"property","match_name":"ADBE Vector Grad HiLite Angle","value":30},
                        {"kind":"property","match_name":"ADBE Vector Fill Opacity","value":100}
                    ]}
                ]}
            ]}
        ]}]});
        let mut project = Project::default();
        let imported = import_ir(&mut project, ir.to_string().as_bytes(), "", &mut |_| None).unwrap();
        assert!(imported.losses.is_empty(), "{:?}", imported.losses);
        let comp = imported.primary_comp.unwrap();
        let fill = project.comp(comp).unwrap().layers[0].props.sub("contents").unwrap().sub("gfill").unwrap();
        assert!(matches!(fill.prop("colors").unwrap().value, Value::Gradient(_)));
        assert_eq!(fill.prop("start").unwrap().value, Value::Vec2([-50.0, 0.0]));
        assert_eq!(fill.prop("end").unwrap().value, Value::Vec2([50.0, 0.0]));
        assert_eq!(fill.prop("highlightLength").unwrap().value.as_f64(), 20.0);
        assert_eq!(fill.prop("highlightAngle").unwrap().value.as_f64(), 30.0);
        assert_eq!(fill.prop("type").unwrap().keys.iter().map(|k| &k.value).collect::<Vec<_>>(), [&Value::Enum(0), &Value::Enum(1)]);
        let frame = effectcraft_render::render_frame(&project, comp, Tick::ZERO, 1.0);
        let visible: Vec<_> = frame.data.iter().filter(|p| p[3] > 0.9).collect();
        assert!(!visible.is_empty());
        assert!(visible.iter().all(|p| p[0] < 0.2 && p[2] > p[0]), "saved blue gradient must not fall back to white/black");
    }

    #[test]
    fn numeric_aep_track_matte_type_keeps_inversion() {
        let ir = serde_json::json!({"schema_version":1,"items":[{"id":1,"kind":"comp","name":"Main","width":100,"height":100,"duration":1,"layers":[
            {"id":2,"kind":"null","name":"Text","matte":{"layer_id":3,"type":5014}},
            {"id":3,"kind":"null","name":"Matte"}
        ]}]});
        let mut project = Project::default();
        let result = import_ir(&mut project, ir.to_string().as_bytes(), "", &mut |_| None).unwrap();
        let layer = &project.comp(result.primary_comp.unwrap()).unwrap().layers[0];
        assert!(matches!(layer.track_matte, Some(TrackMatte { kind: MatteKind::AlphaInverted, .. })));
    }

    #[test]
    fn offline_vector_layer_binding_survives_native_reopen() {
        let ir = serde_json::json!({"schema_version":1,"items":[
            {"id":1,"kind":"footage","name":"Art/source.ai","file":"/missing/source.ai",
             "width":21,"height":41,"has_video":true,
             "source_layer":{"index":1,"name":"Art","layer_size":true,"crop_bounds":[20.25,20.5,40.5,60.75]}}
        ]});
        let mut project = Project::default();
        let result = import_ir(&mut project, ir.to_string().as_bytes(), "", &mut |_| None).unwrap();
        assert!(result.losses.is_empty(), "{:?}", result.losses);
        assert_eq!(result.missing_media, ["/missing/source.ai"]);
        let reopened = Project::from_json(&project.to_file_json().unwrap()).unwrap();
        let footage = reopened
            .items
            .values()
            .find_map(|item| match &item.kind {
                ItemKind::Footage(f) => Some(f),
                _ => None,
            })
            .unwrap();
        assert!(footage.missing);
        let binding = footage.layer.as_ref().unwrap();
        assert_eq!(binding.index, 1);
        assert_eq!(binding.name, "Art");
        assert_eq!(binding.crop_bounds, Some([20.25, 20.5, 40.5, 60.75]));
        let mut bad = ir;
        bad["items"][0]["source_layer"]["crop_bounds"] = serde_json::json!([0, 0, -1, 40]);
        let rejected = import_ir(&mut Project::default(), bad.to_string().as_bytes(), "", &mut |_| None).unwrap();
        assert!(rejected.losses.iter().any(|s| s.contains("source layer binding")));
    }

    #[test]
    fn local_corpus_smoke() {
        let Ok(path) = std::env::var("EFFECTCRAFT_AEP_IR_SAMPLE") else { return };
        let bytes = std::fs::read(path).unwrap();
        let mut project = Project::default();
        let result = import_ir(&mut project, &bytes, "", &mut |p| Some(p.to_owned())).unwrap();
        assert!(!result.comps.is_empty());
        assert!(!result.layer_ids.is_empty());
        eprintln!("{} comps, {} layers, {} fidelity losses", result.comps.len(), result.layer_ids.len(), result.losses.len());
        for loss in &result.losses {
            eprintln!("{loss}");
        }
    }

    #[test]
    fn local_intro_frame() {
        let Ok(path) = std::env::var("EFFECTCRAFT_AEP_RENDER_PROJECT") else { return };
        let out = std::env::var("EFFECTCRAFT_AEP_RENDER_OUT").unwrap_or_else(|_| "/tmp/effectcraft-intro-current.png".into());
        let comp_name = std::env::var("EFFECTCRAFT_AEP_RENDER_COMP").unwrap_or_else(|_| "Main Comp".into());
        let time = std::env::var("EFFECTCRAFT_AEP_RENDER_TIME").ok().and_then(|s| s.parse::<f64>().ok()).unwrap_or(2.0);
        let project = Project::from_json(&std::fs::read_to_string(path).unwrap()).unwrap();
        let comp = project.items.values().find(|item| item.name.eq_ignore_ascii_case(&comp_name) && item.as_comp().is_some()).unwrap();
        let image = effectcraft_render::render_frame(&project, comp.id, tick(time), 1.0);
        image::save_buffer(&out, &image.to_rgba8(), image.width, image.height, image::ColorType::Rgba8).unwrap();
        eprintln!("rendered current worktree frame to {out}");
    }

    #[test]
    fn local_corpus_all() {
        let Ok(dir) = std::env::var("EFFECTCRAFT_AEP_IR_CORPUS") else { return };
        let output_dir = std::env::var("EFFECTCRAFT_AEP_PROJECT_OUTPUT").ok().map(std::path::PathBuf::from);
        if let Some(dir) = &output_dir {
            std::fs::create_dir_all(dir).unwrap();
        }
        let mut files: Vec<_> = std::fs::read_dir(dir)
            .unwrap()
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| {
                p.extension().is_some_and(|e| e == "json")
                    && p.file_stem().and_then(|s| s.to_str()).is_some_and(|s| s.len() == 16 && s.bytes().all(|b| b.is_ascii_hexdigit()))
            })
            .collect();
        files.sort();
        let mut ok = 0usize;
        let mut failures = Vec::new();
        let mut panics = Vec::new();
        let mut losses = std::collections::BTreeMap::<String, usize>::new();
        let mut loss_details = std::collections::BTreeMap::<String, usize>::new();
        let mut evidence = Vec::<Json>::new();
        let mut slowest = (std::time::Duration::ZERO, String::new());
        for file in &files {
            let bytes = std::fs::read(file).unwrap();
            let name = file.file_name().unwrap().to_string_lossy().into_owned();
            let start = std::time::Instant::now();
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let mut project = Project::default();
                import_ir(&mut project, &bytes, "", &mut |_| None).map(|result| (result, project))
            }));
            let elapsed = start.elapsed();
            if elapsed > slowest.0 {
                slowest = (elapsed, name.clone());
            }
            match outcome {
                Ok(Ok((result, project))) => {
                    ok += 1;
                    if let Some(dir) = &output_dir {
                        let stem = file.file_stem().unwrap().to_string_lossy();
                        let output = dir.join(format!("{stem}.ecproj"));
                        let json = project.to_file_json().unwrap();
                        std::fs::write(&output, &json).unwrap();
                        let ecproj_sha = effectcraft_segment::sha256::hex(json.as_bytes());
                        let mut reopened = Project::from_json(&std::fs::read_to_string(&output).unwrap()).unwrap();
                        let before = structure(&project);
                        let after = structure(&reopened);
                        let counts_equal = before == after;
                        let same_content = project.same_content(&reopened);
                        assert!(counts_equal && same_content);
                        let snapshot = reopened.clone();
                        let editable =
                            reopened.comps().find_map(|(id, c)| c.layers.iter().find(|l| l.props.prop("transform/opacity").is_some()).map(|l| (*id, l.id)));
                        let mut edit_restore = false;
                        if let Some((comp_id, layer_id)) = editable
                            && let Some(prop) =
                                reopened.comp_mut(comp_id).and_then(|c| c.layer_mut(layer_id)).and_then(|l| l.props.prop_mut("transform/opacity"))
                        {
                            prop.value = Value::Scalar(prop.value.as_f64() + 1.0);
                            assert!(!reopened.same_content(&snapshot));
                            reopened = snapshot.clone(); // same snapshot restoration as command undo
                            edit_restore = reopened.same_content(&snapshot);
                            assert!(edit_restore);
                        }
                        assert!(edit_restore, "{name}: no editable opacity property was restored");
                        let source: Json = serde_json::from_slice(&bytes).unwrap();
                        let ir_version = source.get("schema_version").and_then(Json::as_u64).unwrap();
                        let ir_implicit_cameras =
                            array(&source, "items").iter().filter(|item| item.get("default_camera").is_some_and(|camera| !camera.is_null())).count();
                        assert_eq!(before[7], ir_implicit_cameras);
                        let sha = source.pointer("/source/sha256").and_then(Json::as_str).unwrap();
                        assert!(sha.len() == 64 && sha.bytes().all(|b| b.is_ascii_hexdigit()));
                        let counts = serde_json::json!({"comps":before[0],"layers":before[1],"properties":before[2],"property_keyframes":before[3],"markers":before[4],"keyframes":before[3]+before[4],"render_font_docs":before[5],"font_file_docs":before[6],"implicit_cameras":before[7],"implicit_camera_keyframes":before[8]});
                        let reopened_counts = serde_json::json!({"comps":after[0],"layers":after[1],"properties":after[2],"property_keyframes":after[3],"markers":after[4],"keyframes":after[3]+after[4],"render_font_docs":after[5],"font_file_docs":after[6],"implicit_cameras":after[7],"implicit_camera_keyframes":after[8]});
                        let manifest = serde_json::json!({"id":stem,"source_sha256":sha,"ecproj_sha256":ecproj_sha,"source_ir":name,"ir_schema_version":ir_version,"ir_implicit_cameras":ir_implicit_cameras,"native_project_schema":project.schema,"project":output.file_name().unwrap().to_string_lossy(),"counts":counts,"reopened_counts":reopened_counts,"same_content":same_content,"counts_equal":counts_equal,"edit_restore":edit_restore,"loss_count":result.losses.len(),"losses":result.losses.clone()});
                        std::fs::write(dir.join(format!("{stem}.source.json")), serde_json::to_vec_pretty(&manifest).unwrap()).unwrap();
                        evidence.push(manifest);
                    }
                    for loss in result.losses {
                        if let Some((_, detail)) = loss.rsplit_once(": ") {
                            *loss_details.entry(detail.to_owned()).or_default() += 1;
                        }
                        let reason = if loss.contains("has no native renderer") {
                            "unknown effect"
                        } else if loss.contains("unsupported") && loss.contains("parameter") {
                            "unknown effect parameter"
                        } else if loss.contains("unsupported property") {
                            "unknown property value"
                        } else if loss.contains("missing source") {
                            "missing layer source"
                        } else {
                            "other"
                        };
                        *losses.entry(reason.to_owned()).or_default() += 1;
                    }
                }
                Ok(Err(e)) => failures.push(format!("{name}: {e}")),
                Err(_) => panics.push(name),
            }
        }
        eprintln!("corpus: {ok}/{} imported, {} errors, {} panics, max {:?} ({})", files.len(), failures.len(), panics.len(), slowest.0, slowest.1);
        eprintln!("losses by reason: {losses:?}");
        let mut top: Vec<_> = loss_details.into_iter().collect();
        top.sort_by_key(|item| std::cmp::Reverse(item.1));
        eprintln!("top loss details: {:?}", &top[..top.len().min(20)]);
        if let Some(dir) = &output_dir {
            let render_font_docs: u64 = evidence.iter().filter_map(|p| p.pointer("/counts/render_font_docs").and_then(Json::as_u64)).sum();
            let font_file_docs: u64 = evidence.iter().filter_map(|p| p.pointer("/counts/font_file_docs").and_then(Json::as_u64)).sum();
            let implicit_cameras: u64 = evidence.iter().filter_map(|p| p.pointer("/counts/implicit_cameras").and_then(Json::as_u64)).sum();
            let implicit_camera_keyframes: u64 = evidence.iter().filter_map(|p| p.pointer("/counts/implicit_camera_keyframes").and_then(Json::as_u64)).sum();
            let proof = serde_json::json!({"schema_version":1,"summary":{"projects":files.len(),"imported":ok,"errors":failures.len(),"panics":panics.len(),"ir_schema_version":1,"slowest_import_ms":slowest.0.as_secs_f64()*1000.0,"render_font_docs":render_font_docs,"font_file_docs":font_file_docs,"implicit_cameras":implicit_cameras,"implicit_camera_keyframes":implicit_camera_keyframes,"losses_by_reason":losses.clone()},"projects":evidence,"failures":failures.clone(),"panics":panics.clone()});
            std::fs::write(dir.join("reopen-evidence.json"), serde_json::to_vec_pretty(&proof).unwrap()).unwrap();
        }
        for fail in &failures {
            eprintln!("ERROR {fail}");
        }
        for panic in &panics {
            eprintln!("PANIC {panic}");
        }
        assert!(failures.is_empty() && panics.is_empty());
    }
}
