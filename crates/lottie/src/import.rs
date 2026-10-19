//! Lottie document → EffectCraft compositions (the inverse of [`crate::export`]).

use std::collections::BTreeMap;
use std::sync::Arc;

use effectcraft_color::Label;
use effectcraft_keyframe::{ShapePath, Value};
use effectcraft_project::build::{self, Ids};
use effectcraft_project::{
    AlphaMode, AutoOrient, Comp, Footage, FootageKind, GroupKind, ItemId, ItemKind, Layer, LayerId, LayerSource, Marker, MatteKind, Node, ParamUi, Project,
    PropGroup, Property, Solid, TrackMatte,
};
use effectcraft_time::{FrameRate, Tick};
use serde_json::Value as Json;

use crate::anim::{self, Im, import_prop};
use crate::export::{fx_by_id, fx_by_type, fx_params, mask_mode_from};
use crate::{LottieError, Timebase, shapes, text};

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

fn f(j: &Json, k: &str) -> Option<f64> {
    j.get(k).and_then(Json::as_f64)
}

fn s<'a>(j: &'a Json, k: &str) -> Option<&'a str> {
    j.get(k).and_then(Json::as_str)
}

fn hex_color(sc: &str) -> [f32; 3] {
    let h = sc.trim_start_matches('#');
    let c = |i: usize| h.get(i..i + 2).and_then(|x| u8::from_str_radix(x, 16).ok()).unwrap_or(0) as f32 / 255.0;
    [c(0), c(2), c(4)]
}

/// The Lottie JSON inside raw bytes (plain JSON, or a stored dotLottie archive).
fn document(bytes: &[u8]) -> Result<Json, LottieError> {
    if bytes.starts_with(b"PK") {
        let entries = crate::zip::read_stored(bytes);
        let anim = entries
            .iter()
            .find(|(n, _)| n.starts_with("animations/") && n.ends_with(".json"))
            .or_else(|| entries.iter().find(|(n, _)| n.ends_with(".json") && n != "manifest.json"))
            .ok_or_else(|| LottieError::Parse("dotLottie archive without an uncompressed animation".into()))?;
        return serde_json::from_slice(&anim.1).map_err(|e| LottieError::Parse(e.to_string()));
    }
    serde_json::from_slice(bytes).map_err(|e| LottieError::Parse(e.to_string()))
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
    match craft_docpath::resolve(stored, doc.as_deref()) {
        Ok(path) if path.is_absolute() => Ok(path.to_string_lossy().into_owned()),
        Ok(_) => Err(format!("{stored} is relative and the Lottie file's folder is not known")),
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
    let (Some(w), Some(h)) = (f(&doc, "w"), f(&doc, "h")) else { return Err(LottieError::Parse("missing `w`/`h`".into())) };
    let fr = f(&doc, "fr").filter(|v| *v > 0.0).unwrap_or(30.0);
    let rate = FrameRate::from_f64(fr);
    let tb = Timebase { rate };
    let name = s(&doc, "nm").filter(|n| !n.is_empty()).unwrap_or(fallback_name).to_string();
    let ip = f(&doc, "ip").unwrap_or(0.0);
    let op = f(&doc, "op").unwrap_or(ip + fr).max(ip + 1.0);
    let mut cx =
        Ctx { im: Im { tb, warnings: vec![] }, assets: BTreeMap::new(), fonts: vec![], store, base_dir, items: vec![], folder: None, name: name.clone() };
    if let Some(list) = doc.get("fonts").and_then(|f| f.get("list")).and_then(Json::as_array) {
        for fo in list {
            let n = s(fo, "fName").unwrap_or("").to_string();
            let fam = s(fo, "fFamily").unwrap_or(&n).to_string();
            let st = s(fo, "fStyle").unwrap_or("Regular").to_string();
            cx.fonts.push((n, fam, st));
        }
    }
    let (w, h) = (w.round().max(1.0) as u32, h.round().max(1.0) as u32);
    let duration = tb.tick(op);
    // Pass 1: asset items (precomps empty for now, images stored).
    let assets = doc.get("assets").and_then(Json::as_array).cloned().unwrap_or_default();
    for a in &assets {
        let Some(id) = s(a, "id") else { continue };
        if a.get("layers").is_some() {
            let folder = cx.folder(project);
            let (aw, ah) = (f(a, "w").map(|v| v as u32).unwrap_or(w), f(a, "h").map(|v| v as u32).unwrap_or(h));
            let arate = f(a, "fr").map(FrameRate::from_f64).unwrap_or(rate);
            let c = Comp::new(aw, ah, arate, duration);
            let nm = s(a, "nm").unwrap_or(id).to_string();
            let item = project.add_item(&nm, Label::Sandstone, Some(folder), ItemKind::Comp(Arc::new(c)));
            cx.items.push(item);
            cx.assets.insert(id.to_string(), (item, f(a, "w").zip(f(a, "h")).map(|(x, y)| (x as u32, y as u32))));
        } else if let Some(p) = s(a, "p") {
            let (aw, ah) = (f(a, "w").unwrap_or(0.0) as u32, f(a, "h").unwrap_or(0.0) as u32);
            let nm = s(a, "nm").unwrap_or(id).to_string();
            let path = if let Some((mime, data)) = crate::base64::data_uri(p) {
                let ext = match mime.as_str() {
                    "image/jpeg" | "image/jpg" => "jpg",
                    "image/webp" => "webp",
                    "image/gif" => "gif",
                    "image/svg+xml" => "svg",
                    _ => "png",
                };
                (cx.store)(&ImportedImage { id: id.to_string(), name: nm.clone(), bytes: data, ext: ext.into() })
            } else {
                let u = s(a, "u").unwrap_or("");
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
    for a in &assets {
        let (Some(id), Some(layers)) = (s(a, "id"), a.get("layers").and_then(Json::as_array)) else { continue };
        let Some((item, _)) = cx.assets.get(id).copied() else { continue };
        let Some(mut c) = project.comp(item).cloned() else { continue };
        let end = layers.iter().filter_map(|l| f(l, "op")).fold(0.0f64, f64::max);
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
    if let Some(ms) = doc.get("markers").and_then(Json::as_array) {
        for m in ms {
            comp.markers.push(Marker {
                time: tb.tick(f(m, "tm").unwrap_or(0.0)),
                duration: tb.tick(f(m, "dr").unwrap_or(0.0)),
                comment: s(m, "cm").unwrap_or("").to_string(),
                ..Default::default()
            });
        }
    }
    let layers = doc.get("layers").and_then(Json::as_array).cloned().unwrap_or_default();
    build_layers(&mut cx, project, &mut comp, &layers);
    let id = project.add_item(&name, Label::Sandstone, None, ItemKind::Comp(Arc::new(comp)));
    cx.items.push(id);
    project.fix_next_id();
    Ok(ImportResult { comp: id, items: cx.items, warnings: cx.im.warnings })
}

fn transform(im: &Im, ks: &Json, tr: &mut PropGroup, next: &mut u64) {
    let set = |tr: &mut PropGroup, m: &str, j: Option<&Json>, conv: &dyn Fn(&Json) -> Option<Value>| {
        if let (Some(j), Some(p)) = (j, tr.get_mut(m)) {
            import_prop(im, j, p, conv);
        }
    };
    set(tr, "anchor", ks.get("a"), &|v| anim::to_vec3(v, 0.0));
    set(tr, "scale", ks.get("s"), &|v| anim::to_vec3(v, 100.0));
    set(tr, "rotation", ks.get("r").or(ks.get("rz")), &anim::to_scalar);
    set(tr, "rotationX", ks.get("rx"), &anim::to_scalar);
    set(tr, "rotationY", ks.get("ry"), &anim::to_scalar);
    set(tr, "orientation", ks.get("or"), &|v| anim::to_vec3(v, 0.0));
    set(tr, "opacity", ks.get("o"), &anim::to_scalar);
    let Some(p) = ks.get("p") else { return };
    if p.get("s").and_then(Json::as_bool) == Some(true) {
        // Separated dimensions: X/Y/Z Position after Position.
        let at = tr.children.iter().position(|c| c.match_id() == "position").map(|i| i + 1).unwrap_or(tr.children.len());
        let base = tr.get("position").map(|p| p.value.as_vec3()).unwrap_or([0.0; 3]);
        for (d, (key, m, name)) in
            [("x", "positionX", "X Position"), ("y", "positionY", "Y Position"), ("z", "positionZ", "Z Position")].iter().enumerate().rev()
        {
            let mut pr = Property::new(*next, m, name, Value::Scalar(base[d])).with_ui(ParamUi::Number);
            *next += 1;
            pr.three_d_only = d == 2;
            if let Some(j) = p.get(*key) {
                import_prop(im, j, &mut pr, &anim::to_scalar);
            }
            tr.children.insert(at, Node::Prop(pr));
        }
    } else {
        set(tr, "position", Some(p), &|v| anim::to_vec3(v, 0.0));
    }
}

fn masks(im: &Im, ids: &mut Ids, list: &[Json], g: &mut PropGroup) {
    for (i, m) in list.iter().enumerate() {
        let mode = mask_mode_from(s(m, "mode").unwrap_or("a"));
        let name = s(m, "nm").map(str::to_string).unwrap_or(format!("Mask {}", i + 1));
        let color = build::MASK_COLORS[i % build::MASK_COLORS.len()];
        let mut mg = build::mask(ids, &name, ShapePath::default(), mode, color);
        if let GroupKind::Mask { inverted, .. } = &mut mg.kind {
            *inverted = m.get("inv").and_then(Json::as_bool).unwrap_or(false);
        }
        let mut set = |k: &str, mid: &str, conv: &dyn Fn(&Json) -> Option<Value>| {
            if let (Some(j), Some(p)) = (m.get(k), mg.get_mut(mid)) {
                import_prop(im, j, p, conv);
            }
        };
        set("pt", "path", &shapes::to_path);
        set("o", "opacity", &anim::to_scalar);
        set("x", "expansion", &anim::to_scalar);
        set("f", "feather", &anim::to_vec2);
        g.children.push(Node::Group(mg));
    }
}

fn effects(im: &mut Im, ids: &mut Ids, list: &[Json], g: &mut PropGroup, size: [f64; 2]) {
    for e in list {
        let ty = e.get("ty").and_then(Json::as_u64).unwrap_or(0);
        let params = e.get("ef").and_then(Json::as_array).cloned().unwrap_or_default();
        let inner_ty = params.first().and_then(|p| p.get("ty")).and_then(Json::as_u64);
        let map = s(e, "mn").and_then(fx_by_id).or_else(|| fx_by_type(ty, inner_ty));
        let nm = s(e, "nm").unwrap_or("Effect");
        let Some(map) = map else {
            im.warn(format!("effect `{nm}` (type {ty}) is not supported (skipped)"));
            continue;
        };
        let Some(spec) = effectcraft_effects::find(map.id) else { continue };
        let mut fg = effectcraft_effects::instantiate(spec, ids, nm, size);
        if e.get("en").and_then(Json::as_u64) == Some(0) {
            fg.enabled = false;
        }
        for (i, (m, pty, scale, _)) in fx_params(map).iter().enumerate() {
            if m.is_empty() {
                continue;
            }
            let Some(pj) = params.get(i).and_then(|p| p.get("v")) else { continue };
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
            import_prop(im, pj, p, &conv);
        }
        g.children.push(Node::Group(fg));
    }
}

fn layer_source(cx: &mut Ctx, project: &mut Project, comp: &Comp, j: &Json) -> Option<(LayerSource, (u32, u32))> {
    let ty = j.get("ty").and_then(Json::as_u64).unwrap_or(99);
    let full = (comp.width, comp.height);
    let asset = |cx: &Ctx| s(j, "refId").and_then(|r| cx.assets.get(r).copied());
    Some(match ty {
        0 => {
            let Some((item, _)) = asset(cx) else {
                cx.im.warn(format!("{}: missing precomp asset", s(j, "nm").unwrap_or("layer")));
                return None;
            };
            let size = project.comp(item).map(|c| (c.width, c.height)).unwrap_or(full);
            (LayerSource::Comp { item }, size)
        }
        1 => {
            let sw = f(j, "sw").unwrap_or(comp.width as f64).round().max(1.0) as u32;
            let sh = f(j, "sh").unwrap_or(comp.height as f64).round().max(1.0) as u32;
            let color = hex_color(s(j, "sc").unwrap_or("#000000"));
            let name = s(j, "nm").unwrap_or("Solid");
            let folder = cx.folder(project);
            let item = project.add_item(name, Label::Red, Some(folder), ItemKind::Solid(Solid { color, width: sw, height: sh, pixel_aspect: 1.0 }));
            cx.items.push(item);
            (LayerSource::Solid { item }, (sw, sh))
        }
        2 => {
            let Some((item, size)) = asset(cx) else {
                cx.im.warn(format!("{}: missing image asset", s(j, "nm").unwrap_or("layer")));
                return None;
            };
            (LayerSource::Footage { item }, size.unwrap_or(full))
        }
        3 => (LayerSource::Null, (100, 100)),
        4 => (LayerSource::Shape, full),
        5 => (LayerSource::Text, full),
        13 => {
            cx.im.warn(format!("{}: camera layers are not imported", s(j, "nm").unwrap_or("camera")));
            return None;
        }
        other => {
            cx.im.warn(format!("{}: layer type {other} is not supported (skipped)", s(j, "nm").unwrap_or("layer")));
            return None;
        }
    })
}

/// Build the layers of a comp (Lottie order: first = top = layer #1).
fn build_layers(cx: &mut Ctx, project: &mut Project, comp: &mut Comp, layers: &[Json]) {
    let tb = cx.im.tb;
    let mut by_ind: BTreeMap<i64, LayerId> = BTreeMap::new();
    // (layer id, parent ind, matte: (kind, explicit tp ind), json position)
    let mut pending: Vec<(LayerId, Option<i64>, Option<(MatteKind, Option<i64>)>, usize)> = vec![];
    let mut pos_ind: Vec<Option<i64>> = vec![];
    for (ji, j) in layers.iter().enumerate() {
        let ind = j.get("ind").and_then(Json::as_i64);
        pos_ind.push(ind);
        let Some((source, size)) = layer_source(cx, project, comp, j) else { continue };
        let name = s(j, "nm").unwrap_or(source.type_name()).to_string();
        let mut l: Layer = build::layer(project, comp, &name, source.clone(), size, None);
        if let Some(i) = ind {
            by_ind.insert(i, l.id);
        }
        let ip = f(j, "ip").unwrap_or(0.0);
        let op = f(j, "op").unwrap_or(tb.frame(comp.duration));
        l.in_point = tb.tick(ip);
        l.out_point = tb.tick(op.max(ip));
        l.start_time = tb.tick(f(j, "st").unwrap_or(0.0));
        l.stretch = f(j, "sr").unwrap_or(1.0) * 100.0;
        if j.get("ddd").and_then(Json::as_u64) == Some(1) {
            l.switches.three_d = true;
        }
        if let Some(bm) = j.get("bm").and_then(Json::as_u64) {
            l.blend_mode = crate::blend_from_lottie(bm);
        }
        if j.get("ao").and_then(Json::as_u64) == Some(1) {
            l.auto_orient = AutoOrient::AlongPath;
        }
        if j.get("hd").and_then(Json::as_bool) == Some(true) {
            l.switches.video = false;
        }
        if j.get("td").and_then(Json::as_u64) == Some(1) {
            l.switches.video = false;
        }
        let mut next = project.next_id;
        if let (Some(ks), Some(tr)) = (j.get("ks"), l.props.sub_mut("transform")) {
            transform(&cx.im, ks, tr, &mut next);
        }
        let mut ids = Ids(&mut next);
        match &source {
            LayerSource::Shape => {
                let items = j.get("shapes").and_then(Json::as_array).cloned().unwrap_or_default();
                let groups = shapes::import_items(&mut cx.im, &mut ids, &items);
                if let Some(c) = l.props.sub_mut("contents") {
                    c.children.extend(groups.into_iter().map(Node::Group));
                }
            }
            LayerSource::Text => {
                if let (Some(t), Some(tg)) = (j.get("t"), l.props.sub_mut("text")) {
                    text::import_text(&mut cx.im, &mut ids, t, tg, &cx.fonts);
                }
            }
            LayerSource::Comp { .. } => {
                if let Some(tm) = j.get("tm") {
                    let mut pr = ids.prop("timeRemap", "Time Remap", Value::Scalar(0.0)).with_ui(ParamUi::Number);
                    import_prop(&cx.im, tm, &mut pr, &anim::to_scalar);
                    l.props.children.insert(0, Node::Prop(pr));
                }
            }
            _ => {}
        }
        if let Some(list) = j.get("masksProperties").and_then(Json::as_array)
            && let Some(g) = l.props.sub_mut("masks")
        {
            masks(&cx.im, &mut ids, list, g);
        }
        if let Some(list) = j.get("ef").and_then(Json::as_array)
            && let Some(g) = l.props.sub_mut("effects")
        {
            effects(&mut cx.im, &mut ids, list, g, [size.0 as f64, size.1 as f64]);
        }
        if j.get("sy").and_then(Json::as_array).is_some_and(|a| !a.is_empty()) {
            cx.im.warn(format!("{name}: layer styles are not imported"));
        }
        project.next_id = next;
        let parent = j.get("parent").and_then(Json::as_i64);
        let tt = j.get("tt").and_then(Json::as_u64).unwrap_or(0);
        let matte = match tt {
            1 => Some(MatteKind::Alpha),
            2 => Some(MatteKind::AlphaInverted),
            3 => Some(MatteKind::Luma),
            4 => Some(MatteKind::LumaInverted),
            _ => None,
        }
        .map(|k| (k, j.get("tp").and_then(Json::as_i64)));
        pending.push((l.id, parent, matte, ji));
        comp.layers.push(l);
    }
    for (id, parent, matte, ji) in pending {
        let parent_id = parent.and_then(|p| by_ind.get(&p).copied()).filter(|p| *p != id);
        let matte_id = matte.and_then(|(k, tp)| {
            let target = match tp {
                Some(tp) => by_ind.get(&tp).copied(),
                // Legacy files: the matte is the layer directly above.
                None => ji.checked_sub(1).and_then(|a| pos_ind.get(a).copied().flatten()).and_then(|i| by_ind.get(&i).copied()),
            };
            target.filter(|t| *t != id).map(|t| TrackMatte { layer: t, kind: k })
        });
        if let Some(l) = comp.layer_mut(id) {
            l.parent = parent_id;
            l.track_matte = matte_id;
        }
    }
}
