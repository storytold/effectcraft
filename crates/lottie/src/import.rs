//! Lottie document → EffectCraft compositions (the inverse of [`crate::export`]).

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use craft_lottie::model::{Asset, Effect, Layer as LottieLayer, LayerContent, Mask, Transform};
use craft_lottie::{Animated, Animation, Json};
use effectcraft_color::Label;
use effectcraft_keyframe::{ShapePath, Value};
use effectcraft_project::build::{self, Ids};
use effectcraft_project::{
    AlphaMode, AutoOrient, Comp, Footage, FootageKind, GroupKind, ItemId, ItemKind, Layer, LayerId, LayerSource, Marker, MatteKind, Node, ParamUi, Project,
    PropGroup, Property, Solid, TrackMatte,
};
use effectcraft_time::{FrameRate, Tick};

use crate::anim::{self, Im, import_animated, import_prop};
use crate::export::{fx_by_id, fx_by_type, fx_params, mask_mode_from};
use crate::{LottieError, Timebase, shapes, text};

/// Longest font family or style name kept (each text key using the font holds a copy).
const MAX_FONT_NAME_CHARS: usize = 256;

/// An embedded image asset handed to the caller to store (the returned path becomes the
/// footage item's file).
#[derive(Clone, Debug)]
pub struct ImportedImage {
    pub id: String,
    pub name: String,
    pub bytes: Vec<u8>,
    /// File extension from the data URI (`png`, `jpg`, …).
    pub ext: String,
}

#[derive(Clone, Debug)]
pub struct ImportResult {
    /// The new top-level composition.
    pub comp: ItemId,
    /// Every item created (comps, solids, images, folder).
    pub items: Vec<ItemId>,
    pub warnings: Vec<String>,
}

/// The Lottie document inside raw bytes (plain JSON, or a stored dotLottie archive), read into
/// `craft-lottie`'s model within its default limits.
fn document(bytes: &[u8]) -> Result<Animation, LottieError> {
    let read = |b: &[u8]| {
        Animation::read(b).map_err(|e| match e {
            craft_lottie::LottieError::Parse(m) => LottieError::Parse(m),
            other => LottieError::Invalid(other.to_string()),
        })
    };
    if bytes.starts_with(b"PK") {
        let entries = crate::zip::read_stored(bytes);
        let anim = entries
            .iter()
            .find(|(n, _)| n.starts_with("animations/") && n.ends_with(".json"))
            .or_else(|| entries.iter().find(|(n, _)| n.ends_with(".json") && n != "manifest.json"))
            .ok_or_else(|| LottieError::Parse("dotLottie archive without an uncompressed animation".into()))?;
        return read(&anim.1);
    }
    read(bytes)
}

struct Ctx<'a> {
    im: Im,
    assets: BTreeMap<String, (ItemId, Option<(u32, u32)>)>,
    fonts: Vec<(String, String, String)>,
    store: &'a mut dyn FnMut(&ImportedImage) -> Option<String>,
    base_dir: &'a str,
    items: Vec<ItemId>,
    folder: Option<ItemId>,
    name: String,
}

impl Ctx<'_> {
    fn folder(&mut self, project: &mut Project) -> ItemId {
        if let Some(f) = self.folder {
            return f;
        }
        let f = project.add_item(&format!("{} Assets", self.name), Label::None, None, ItemKind::Folder);
        self.items.push(f);
        self.folder = Some(f);
        f
    }
}

/// The file an external image asset (`u` + `p`) names, taken from `base_dir`, the folder of the
/// Lottie file. Only local files, or files on the network share the Lottie file is on, are
/// imported (see `craft_docpath`).
fn external_image(stored: &str, base_dir: &str) -> Result<String, String> {
    let doc = (!base_dir.is_empty()).then(|| {
        let doc = std::path::Path::new(base_dir).join("lottie.json");
        std::path::absolute(&doc).unwrap_or(doc)
    });
    match craft_docpath::resolve_absolute(stored, doc.as_deref()) {
        Ok(path) => Ok(path.to_string_lossy().into_owned()),
        Err(craft_docpath::Refused::Invalid) if doc.is_none() && craft_docpath::resolve(stored, None).is_ok() => {
            Err(format!("{stored} is relative and the Lottie file's folder is not known"))
        }
        Err(why) => Err(format!("{stored}: {why}")),
    }
}

/// Import a Lottie document (JSON or a stored `.lottie` archive) into `project` as a new
/// composition. `store` saves embedded images and returns the file path to use;
/// `base_dir` resolves external image references.
pub fn import(
    project: &mut Project,
    bytes: &[u8],
    fallback_name: &str,
    base_dir: &str,
    store: &mut dyn FnMut(&ImportedImage) -> Option<String>,
) -> Result<ImportResult, LottieError> {
    let doc = document(bytes)?;
    let (Some(w), Some(h)) = (doc.width, doc.height) else { return Err(LottieError::Parse("missing `w`/`h`".into())) };
    let fr = doc.frame_rate.filter(|v| *v > 0.0).unwrap_or(30.0);
    let rate = FrameRate::from_f64(fr);
    let tb = Timebase { rate };
    let name = doc.name.as_deref().filter(|n| !n.is_empty()).unwrap_or(fallback_name).to_string();
    let ip = doc.in_point.unwrap_or(0.0);
    let op = doc.out_point.unwrap_or(ip + fr).max(ip + 1.0);
    let mut cx =
        Ctx { im: Im { tb, warnings: vec![] }, assets: BTreeMap::new(), fonts: vec![], store, base_dir, items: vec![], folder: None, name: name.clone() };
    for fo in doc.fonts.as_ref().and_then(|f| f.list.as_deref()).unwrap_or_default() {
        // Every text key referring to the font copies its family and style: keep them to
        // a font name's length.
        let short = |v: &str| v.chars().take(MAX_FONT_NAME_CHARS).collect::<String>();
        let n = fo.name.clone().unwrap_or_default();
        let fam = short(fo.family.as_deref().unwrap_or(&n));
        let st = short(fo.style.as_deref().unwrap_or("Regular"));
        cx.fonts.push((n, fam, st));
    }
    // Glyph outlines embedded for players without the fonts. Text is set in installed fonts.
    if let Some(chars) = doc.chars.as_deref().filter(|c| !c.is_empty()) {
        let mut fams: Vec<&str> = vec![];
        for fam in chars.iter().filter_map(|c| c.family.as_deref()).filter(|f| !f.trim().is_empty()) {
            if !fams.contains(&fam) {
                fams.push(fam);
            }
        }
        let of = if fams.is_empty() { String::new() } else { format!(" of {}", fams.join(", ")) };
        cx.im.warn(format!("embedded glyphs (`chars`){of} are not imported; text uses the installed fonts"));
    }
    let (w, h) = (w.round().max(1.0) as u32, h.round().max(1.0) as u32);
    let duration = tb.tick(op);
    // Pass 1: asset items (precomps empty for now, images stored).
    let assets: &[Asset] = doc.assets.as_deref().unwrap_or_default();
    let top: &[LottieLayer] = doc.layers.as_deref().unwrap_or_default();
    let layer_sizes = precomp_layer_sizes(top, assets);
    for a in assets {
        let Some(id) = a.id.as_deref() else { continue };
        let size = a.width.zip(a.height).map(|(x, y)| (x as u32, y as u32));
        if a.layers.is_some() {
            let folder = cx.folder(project);
            let layer = layer_sizes.get(id).copied();
            let aw = a.width.map(|v| v as u32).or(layer.map(|l| l.0)).unwrap_or(w);
            let ah = a.height.map(|v| v as u32).or(layer.map(|l| l.1)).unwrap_or(h);
            let arate = a.frame_rate.map(FrameRate::from_f64).unwrap_or(rate);
            let c = Comp::new(aw, ah, arate, duration);
            let nm = a.name.as_deref().unwrap_or(id).to_string();
            let item = project.add_item(&nm, Label::Sandstone, Some(folder), ItemKind::Comp(Arc::new(c)));
            cx.items.push(item);
            cx.assets.insert(id.to_string(), (item, size));
        } else if let Some(p) = a.path.as_deref() {
            let (aw, ah) = (a.width.unwrap_or(0.0) as u32, a.height.unwrap_or(0.0) as u32);
            let nm = a.name.as_deref().unwrap_or(id).to_string();
            let path = if let Some((mime, data)) = craft_lottie::base64::data_uri(p) {
                let ext = match mime.as_str() {
                    "image/jpeg" | "image/jpg" => "jpg",
                    "image/webp" => "webp",
                    "image/gif" => "gif",
                    "image/svg+xml" => "svg",
                    _ => "png",
                };
                (cx.store)(&ImportedImage { id: id.to_string(), name: nm.clone(), bytes: data, ext: ext.into() })
            } else {
                let u = a.dir.as_deref().unwrap_or("");
                match external_image(&format!("{u}{p}"), cx.base_dir) {
                    Ok(path) => Some(path),
                    Err(why) => {
                        cx.im.warn(format!("image asset `{id}` is not imported: {why}"));
                        continue;
                    }
                }
            };
            let Some(path) = path else {
                cx.im.warn(format!("image asset `{id}` could not be stored"));
                continue;
            };
            let folder = cx.folder(project);
            let foot = Footage {
                path,
                kind: FootageKind::Still,
                width: aw.max(1),
                height: ah.max(1),
                pixel_aspect: 1.0,
                frame_rate: rate,
                native_rate: None,
                duration: Tick::ZERO,
                has_video: true,
                has_audio: false,
                alpha: AlphaMode::Straight,
                premul_color: [0.0; 3],
                loop_count: 1,
                codec: String::new(),
                missing: false,
                sequence: vec![],
                color_profile: None,
                ..Default::default()
            };
            let item = project.add_item(&nm, Label::Lavender, Some(folder), ItemKind::Footage(foot));
            cx.items.push(item);
            cx.assets.insert(id.to_string(), (item, Some((aw, ah))));
        } else {
            cx.im.warn(format!("asset `{id}` is not supported (skipped)"));
        }
    }
    // Pass 2: precomp contents.
    for a in assets {
        let (Some(id), Some(layers)) = (a.id.as_deref(), a.layers.as_deref()) else { continue };
        let Some((item, _)) = cx.assets.get(id).copied() else { continue };
        let Some(mut c) = project.comp(item).cloned() else { continue };
        let end = layers.iter().filter_map(|l| l.out_point).fold(0.0f64, f64::max);
        if end > 0.0 {
            c.duration = tb.tick(end);
            c.work_area = (Tick::ZERO, c.duration);
        }
        build_layers(&mut cx, project, &mut c, layers);
        if let Some(it) = project.item_mut(item) {
            it.kind = ItemKind::Comp(Arc::new(c));
        }
    }
    // The main comp.
    let mut comp = Comp::new(w, h, rate, duration);
    if ip > 0.0 {
        comp.work_area = (tb.tick(ip), duration);
    }
    for m in doc.markers.as_deref().unwrap_or_default() {
        comp.markers.push(Marker {
            time: tb.tick(m.time.unwrap_or(0.0)),
            duration: tb.tick(m.duration.unwrap_or(0.0)),
            comment: m.comment.clone().unwrap_or_default(),
            ..Default::default()
        });
    }
    build_layers(&mut cx, project, &mut comp, top);
    let id = project.add_item(&name, Label::Sandstone, None, ItemKind::Comp(Arc::new(comp)));
    cx.items.push(id);
    project.fix_next_id();
    Ok(ImportResult { comp: id, items: cx.items, warnings: cx.im.warnings })
}

/// By asset id, the `w`/`h` of the first precomp layer, in the main comp or in any asset, that
/// shows it. lottie-spec gives a precomposition asset no size: the layer's `w`/`h` are its
/// clipping rect. One pass over all layers, not one per asset.
fn precomp_layer_sizes<'a>(top: &'a [LottieLayer], assets: &'a [Asset]) -> HashMap<&'a str, (u32, u32)> {
    let mut out = HashMap::new();
    let lists = std::iter::once(top).chain(assets.iter().filter_map(|a| a.layers.as_deref()));
    for l in lists.flatten() {
        let LayerContent::Precomp(p) = &l.content else { continue };
        let (Some(id), Some((lw, lh))) = (p.ref_id.as_deref(), p.width.zip(p.height)) else { continue };
        out.entry(id).or_insert((lw.round().max(1.0) as u32, lh.round().max(1.0) as u32));
    }
    out
}

fn transform(im: &Im, ks: &Transform, tr: &mut PropGroup, next: &mut u64) {
    let set = |tr: &mut PropGroup, m: &str, j: Option<&Animated>, conv: &dyn Fn(&Json) -> Option<Value>| {
        if let Some(p) = tr.get_mut(m) {
            import_animated(im, j, p, conv);
        }
    };
    set(tr, "anchor", ks.anchor.as_ref(), &|v| anim::to_vec3(v, 0.0));
    set(tr, "scale", ks.scale.as_ref(), &|v| anim::to_vec3(v, 100.0));
    set(tr, "rotation", ks.rotation.as_ref().or(ks.rotation_z.as_ref()), &anim::to_scalar);
    set(tr, "rotationX", ks.rotation_x.as_ref(), &anim::to_scalar);
    set(tr, "rotationY", ks.rotation_y.as_ref(), &anim::to_scalar);
    set(tr, "orientation", ks.orientation.as_ref(), &|v| anim::to_vec3(v, 0.0));
    set(tr, "opacity", ks.opacity.as_ref(), &anim::to_scalar);
    let Some(p) = &ks.position else { return };
    if let Some(dims) = p.split() {
        // Separated dimensions: X/Y/Z Position after Position.
        let at = tr.children.iter().position(|c| c.match_id() == "position").map(|i| i + 1).unwrap_or(tr.children.len());
        let base = tr.get("position").map(|p| p.value.as_vec3()).unwrap_or([0.0; 3]);
        for (d, (j, m, name)) in dims
            .iter()
            .zip([("positionX", "X Position"), ("positionY", "Y Position"), ("positionZ", "Z Position")])
            .map(|(j, (m, n))| (j, m, n))
            .enumerate()
            .rev()
        {
            let mut pr = Property::new(*next, m, name, Value::Scalar(base[d])).with_ui(ParamUi::Number);
            *next += 1;
            pr.three_d_only = d == 2;
            if let Some(j) = j {
                import_prop(im, j, &mut pr, &anim::to_scalar);
            }
            tr.children.insert(at, Node::Prop(pr));
        }
    } else {
        set(tr, "position", Some(p), &|v| anim::to_vec3(v, 0.0));
    }
}

fn masks(im: &Im, ids: &mut Ids, list: &[Mask], g: &mut PropGroup) {
    for (i, m) in list.iter().enumerate() {
        let mode = mask_mode_from(m.mode.as_deref().unwrap_or("a"));
        let name = m.name.clone().unwrap_or(format!("Mask {}", i + 1));
        let color = build::MASK_COLORS[i % build::MASK_COLORS.len()];
        let mut mg = build::mask(ids, &name, ShapePath::default(), mode, color);
        if let GroupKind::Mask { inverted, .. } = &mut mg.kind {
            *inverted = m.inverted.unwrap_or(false);
        }
        let mut set = |j: Option<&Json>, mid: &str, conv: &dyn Fn(&Json) -> Option<Value>| {
            if let (Some(j), Some(p)) = (j, mg.get_mut(mid)) {
                import_prop(im, j, p, conv);
            }
        };
        let closed = m.closed.unwrap_or(false);
        set(m.path.as_ref().map(|a| &a.0), "path", &|v| shapes::to_path(v, closed));
        set(m.opacity.as_ref().map(|a| &a.0), "opacity", &anim::to_scalar);
        set(m.expansion.as_ref().map(|a| &a.0), "expansion", &anim::to_scalar);
        set(m.extra.get("f"), "feather", &anim::to_vec2);
        g.children.push(Node::Group(mg));
    }
}

fn effects(im: &mut Im, ids: &mut Ids, list: &[Effect], g: &mut PropGroup, size: [f64; 2]) {
    for e in list {
        let ty = e.kind.and_then(|t| u64::try_from(t).ok()).unwrap_or(0);
        let params: &[Effect] = e.values.as_deref().unwrap_or_default();
        let inner_ty = params.first().and_then(|p| p.kind).and_then(|t| u64::try_from(t).ok());
        let map = e.match_name.as_deref().and_then(fx_by_id).or_else(|| fx_by_type(ty, inner_ty));
        let nm = e.name.as_deref().unwrap_or("Effect");
        let Some(map) = map else {
            im.warn(format!("effect `{nm}` (type {ty}) is not supported (skipped)"));
            continue;
        };
        let Some(spec) = effectcraft_effects::find(map.id) else { continue };
        let mut fg = effectcraft_effects::instantiate(spec, ids, nm, size);
        if e.enabled == Some(0) {
            fg.enabled = false;
        }
        for (i, (m, pty, scale, _)) in fx_params(map).iter().enumerate() {
            if m.is_empty() {
                continue;
            }
            let Some(pj) = params.get(i).and_then(|p| p.value.as_ref()) else { continue };
            let Some(p) = fg.get_mut(m) else { continue };
            let (pty, scale) = (*pty, *scale);
            let conv = move |v: &Json| -> Option<Value> {
                match pty {
                    2 => anim::to_color(v),
                    3 => anim::to_vec2(v),
                    4 => anim::to_bool(v),
                    7 => anim::to_enum1(v),
                    _ => anim::to_scalar(v).map(|x| Value::Scalar(x.as_f64() / scale)),
                }
            };
            import_prop(im, &pj.0, p, &conv);
        }
        g.children.push(Node::Group(fg));
    }
}

fn layer_source(cx: &mut Ctx, project: &mut Project, comp: &Comp, j: &LottieLayer) -> Option<(LayerSource, (u32, u32))> {
    let full = (comp.width, comp.height);
    let nm = j.name.as_deref();
    let asset = |cx: &Ctx, r: Option<&str>| r.and_then(|r| cx.assets.get(r).copied());
    Some(match &j.content {
        LayerContent::Precomp(p) => {
            let Some((item, _)) = asset(cx, p.ref_id.as_deref()) else {
                cx.im.warn(format!("{}: missing precomp asset", nm.unwrap_or("layer")));
                return None;
            };
            let size = project.comp(item).map(|c| (c.width, c.height)).unwrap_or(full);
            (LayerSource::Comp { item }, size)
        }
        LayerContent::Solid(sl) => {
            let sw = sl.width.unwrap_or(comp.width as f64).round().max(1.0) as u32;
            let sh = sl.height.unwrap_or(comp.height as f64).round().max(1.0) as u32;
            let color = craft_lottie::hex_color(sl.color.as_deref().unwrap_or("#000000"));
            let name = nm.unwrap_or("Solid");
            let folder = cx.folder(project);
            let item = project.add_item(name, Label::Red, Some(folder), ItemKind::Solid(Solid { color, width: sw, height: sh, pixel_aspect: 1.0 }));
            cx.items.push(item);
            (LayerSource::Solid { item }, (sw, sh))
        }
        LayerContent::Image(r) => {
            let Some((item, size)) = asset(cx, r.as_deref()) else {
                cx.im.warn(format!("{}: missing image asset", nm.unwrap_or("layer")));
                return None;
            };
            (LayerSource::Footage { item }, size.unwrap_or(full))
        }
        LayerContent::Null => (LayerSource::Null, (100, 100)),
        LayerContent::Shape(_) => (LayerSource::Shape, full),
        LayerContent::Text(_) => (LayerSource::Text, full),
        LayerContent::Other(ty) => match ty.as_u64().unwrap_or(99) {
            // A shape or text layer whose content is missing or unreadable is an empty one.
            4 => (LayerSource::Shape, full),
            5 => (LayerSource::Text, full),
            13 => {
                cx.im.warn(format!("{}: camera layers are not imported", nm.unwrap_or("camera")));
                return None;
            }
            other => {
                cx.im.warn(format!("{}: layer type {other} is not supported (skipped)", nm.unwrap_or("layer")));
                return None;
            }
        },
    })
}

/// Build the layers of a comp (Lottie order: first = top = layer #1).
fn build_layers(cx: &mut Ctx, project: &mut Project, comp: &mut Comp, layers: &[LottieLayer]) {
    let tb = cx.im.tb;
    let mut by_ind: BTreeMap<i64, LayerId> = BTreeMap::new();
    // (layer id, its index in `comp.layers`, parent ind, matte: (kind, explicit tp ind), json position)
    let mut pending: Vec<(LayerId, usize, Option<i64>, Option<(MatteKind, Option<i64>)>, usize)> = vec![];
    let mut pos_ind: Vec<Option<i64>> = vec![];
    for (ji, j) in layers.iter().enumerate() {
        let ind = j.index;
        pos_ind.push(ind);
        let Some((source, size)) = layer_source(cx, project, comp, j) else { continue };
        let name = j.name.clone().unwrap_or_else(|| source.type_name().to_string());
        let mut l: Layer = build::layer(project, comp, &name, source.clone(), size, None);
        if let Some(i) = ind {
            by_ind.insert(i, l.id);
        }
        let ip = j.in_point.unwrap_or(0.0);
        let op = j.out_point.unwrap_or(tb.frame(comp.duration));
        l.in_point = tb.tick(ip);
        l.out_point = tb.tick(op.max(ip));
        l.start_time = tb.tick(j.start_time.unwrap_or(0.0));
        // Below 1 % (or 0) the layer time divides by ~0, and layer.timeStretch refuses it too.
        let sr = j.stretch.unwrap_or(1.0);
        l.stretch = if sr.is_finite() && sr.abs() >= 0.01 {
            // Past ±9900 % too (#3247): the largest stretch a layer supports, sign kept.
            let pct = effectcraft_project::limits::stretch(sr * 100.0);
            if pct.to_bits() != (sr * 100.0).to_bits() {
                cx.im.warn(format!("{name}: a time stretch of {} % is not supported ({pct} % used)", sr * 100.0));
            }
            pct
        } else {
            cx.im.warn(format!("{name}: a time stretch of {} % is not supported (100 % used)", sr * 100.0));
            100.0
        };
        if j.three_d == Some(1) {
            l.switches.three_d = true;
        }
        if let Some(bm) = j.blend_mode.and_then(|b| u64::try_from(b).ok()) {
            l.blend_mode = crate::blend_from_lottie(bm);
        }
        if j.auto_orient == Some(1) {
            l.auto_orient = AutoOrient::AlongPath;
        }
        if j.hidden == Some(true) {
            l.switches.video = false;
        }
        if j.is_matte == Some(1) {
            l.switches.video = false;
        }
        let mut next = project.next_id;
        if let (Some(ks), Some(tr)) = (&j.transform, l.props.sub_mut("transform")) {
            transform(&cx.im, ks, tr, &mut next);
        }
        let mut ids = Ids(&mut next);
        match (&source, &j.content) {
            (LayerSource::Shape, LayerContent::Shape(items)) => {
                let groups = shapes::import_items(&mut cx.im, &mut ids, &mut items.iter());
                if let Some(c) = l.props.sub_mut("contents") {
                    c.children.extend(groups.into_iter().map(Node::Group));
                }
            }
            (LayerSource::Text, LayerContent::Text(t)) => {
                if let Some(tg) = l.props.sub_mut("text") {
                    text::import_text(&mut cx.im, &mut ids, t, tg, &cx.fonts);
                }
            }
            (LayerSource::Comp { .. }, LayerContent::Precomp(p)) => {
                if let Some(tm) = &p.time_remap {
                    let mut pr = ids.prop("timeRemap", "Time Remap", Value::Scalar(0.0)).with_ui(ParamUi::Number);
                    import_prop(&cx.im, &tm.0, &mut pr, &anim::to_scalar);
                    l.props.children.insert(0, Node::Prop(pr));
                }
            }
            _ => {}
        }
        if let Some(list) = &j.masks
            && let Some(g) = l.props.sub_mut("masks")
        {
            masks(&cx.im, &mut ids, list, g);
        }
        if let Some(list) = &j.effects
            && let Some(g) = l.props.sub_mut("effects")
        {
            effects(&mut cx.im, &mut ids, list, g, [size.0 as f64, size.1 as f64]);
        }
        if j.extra.get("sy").and_then(Json::as_array).is_some_and(|a| !a.is_empty()) {
            cx.im.warn(format!("{name}: layer styles are not imported"));
        }
        project.next_id = next;
        let matte = match j.matte_mode.unwrap_or(0) {
            1 => Some(MatteKind::Alpha),
            2 => Some(MatteKind::AlphaInverted),
            3 => Some(MatteKind::Luma),
            4 => Some(MatteKind::LumaInverted),
            _ => None,
        }
        .map(|k| (k, j.matte_parent));
        pending.push((l.id, comp.layers.len(), j.parent, matte, ji));
        comp.layers.push(l);
    }
    for (id, at, parent, matte, ji) in pending {
        let parent_id = parent.and_then(|p| by_ind.get(&p).copied()).filter(|p| *p != id);
        let matte_id = matte.and_then(|(k, tp)| {
            let target = match tp {
                Some(tp) => by_ind.get(&tp).copied(),
                // Legacy files: the matte is the layer directly above.
                None => ji.checked_sub(1).and_then(|a| pos_ind.get(a).copied().flatten()).and_then(|i| by_ind.get(&i).copied()),
            };
            target.filter(|t| *t != id).map(|t| TrackMatte { layer: t, kind: k })
        });
        if let Some(l) = comp.layers.get_mut(at) {
            l.parent = parent_id;
            l.track_matte = matte_id;
        }
    }
}
