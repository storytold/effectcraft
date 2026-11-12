//! Photoshop import as a composition (File ▸ Import ▸ File… with Import As: Composition or
//! Composition – Retain Layer Sizes), and Layer ▸ Create ▸ Convert to Editable Text.
//!
//! Every pixel layer becomes a footage item that names its layer (`Footage::layer`), placed in
//! a composition the size of the document. Groups become precomps (pass-through groups collapse
//! so their blend modes act on the layers below, like Photoshop); type layers become editable
//! text layers; layer masks are baked into the layer's alpha and vector masks become masks;
//! layer effects map to Layer Styles; adjustment layers become adjustment layers with the
//! matching effect (best effort); clipped layers preserve the transparency underneath. Smart
//! objects whose file is embedded become footage of that file (an embedded Photoshop document's
//! merged image, or the embedded image) placed with the smart object's transform.

use std::sync::Arc;

use effectcraft_color::BlendMode;
use effectcraft_color::Label;
use effectcraft_keyframe::{Gradient, Justify, ShapePath, TextDoc, Value};
use effectcraft_project::build::{self, Ids};
use effectcraft_project::styles::{self, STYLE_BLEND_MODES};
use effectcraft_project::{AlphaMode, Comp, Footage, FootageKind, ItemId, ItemKind, Layer, LayerSource, MaskMode, Project, PropGroup, Solid, SourceLayer};
use effectcraft_psd::{Adjustment, DValue, Descriptor, Node, Psd};
use effectcraft_time::{FrameRate, Tick};

/// Photoshop blend mode key → compositor mode (`pass` = pass through, handled by collapsing).
pub fn blend_mode(key: &str) -> BlendMode {
    match key {
        "diss" => BlendMode::Dissolve,
        "dark" => BlendMode::Darken,
        "mul " => BlendMode::Multiply,
        "idiv" => BlendMode::ColorBurn,
        "lbrn" => BlendMode::LinearBurn,
        "dkCl" => BlendMode::DarkerColor,
        "lite" => BlendMode::Lighten,
        "scrn" => BlendMode::Screen,
        "div " => BlendMode::ColorDodge,
        "lddg" => BlendMode::LinearDodge,
        "lgCl" => BlendMode::LighterColor,
        "over" => BlendMode::Overlay,
        "sLit" => BlendMode::SoftLight,
        "hLit" => BlendMode::HardLight,
        "vLit" => BlendMode::VividLight,
        "lLit" => BlendMode::LinearLight,
        "pLit" => BlendMode::PinLight,
        "hMix" => BlendMode::HardMix,
        "diff" => BlendMode::Difference,
        "smud" => BlendMode::Exclusion,
        "fsub" => BlendMode::Subtract,
        "fdiv" => BlendMode::Divide,
        "hue " => BlendMode::Hue,
        "sat " => BlendMode::Saturation,
        "colr" => BlendMode::Color,
        "lum " => BlendMode::Luminosity,
        _ => BlendMode::Normal,
    }
}

/// Descriptor blend mode enum (`BlnM`) → compositor mode.
fn desc_blend(v: Option<&str>) -> Option<BlendMode> {
    Some(match v? {
        "Nrml" => BlendMode::Normal,
        "Dslv" => BlendMode::Dissolve,
        "Drkn" => BlendMode::Darken,
        "Mltp" => BlendMode::Multiply,
        "CBrn" => BlendMode::ColorBurn,
        "linearBurn" => BlendMode::LinearBurn,
        "darkerColor" => BlendMode::DarkerColor,
        "Lghn" => BlendMode::Lighten,
        "Scrn" => BlendMode::Screen,
        "CDdg" => BlendMode::ColorDodge,
        "linearDodge" => BlendMode::LinearDodge,
        "lighterColor" => BlendMode::LighterColor,
        "Ovrl" => BlendMode::Overlay,
        "SftL" => BlendMode::SoftLight,
        "HrdL" => BlendMode::HardLight,
        "vividLight" => BlendMode::VividLight,
        "linearLight" => BlendMode::LinearLight,
        "pinLight" => BlendMode::PinLight,
        "hardMix" => BlendMode::HardMix,
        "Dfrn" => BlendMode::Difference,
        "Xclu" => BlendMode::Exclusion,
        "blendSubtraction" => BlendMode::Subtract,
        "blendDivide" => BlendMode::Divide,
        "H   " => BlendMode::Hue,
        "Strt" => BlendMode::Saturation,
        "Clr " => BlendMode::Color,
        "Lmns" => BlendMode::Luminosity,
        _ => return None,
    })
}

/// What the import created.
pub struct PsdImport {
    pub comp: ItemId,
    pub folder: ItemId,
    pub items: Vec<ItemId>,
    pub warnings: Vec<String>,
}

struct Ctx<'a> {
    psd: &'a Psd,
    path: &'a str,
    retain: bool,
    folder: ItemId,
    rate: FrameRate,
    duration: Tick,
    items: Vec<ItemId>,
    warnings: Vec<String>,
}

/// Import `psd` (read from `path`) as a composition named `name`.
pub fn import(proj: &mut Project, psd: &Psd, path: &str, name: &str, retain: bool, rate: FrameRate, duration: Tick) -> PsdImport {
    let folder = proj.add_item(&format!("{name} Layers"), Label::Yellow, None, ItemKind::Folder);
    let mut cx = Ctx { psd, path, retain, folder, rate, duration, items: vec![], warnings: vec![] };
    cx.warnings.extend(psd.warnings.iter().cloned());
    let (tree, flattened) = psd.tree_report();
    if flattened > 0 {
        cx.warnings.push(format!("{flattened} group(s) nested deeper than {} levels were flattened into their parent", effectcraft_psd::MAX_GROUP_DEPTH));
    }
    let comp =
        if psd.layers.is_empty() && !psd.warnings.is_empty() { flattened_comp(proj, &mut cx, name) } else { build_comp(proj, &mut cx, name, &tree, None) };
    PsdImport { comp, folder, items: cx.items, warnings: cx.warnings }
}

/// The layers could not be read: the merged image as the composition's only layer.
fn flattened_comp(proj: &mut Project, cx: &mut Ctx, name: &str) -> ItemId {
    let cid = proj.add_item(name, Label::Sandstone, None, ItemKind::Comp(Arc::new(new_comp(cx))));
    cx.items.push(cid);
    let mut comp = new_comp(cx);
    let file = std::path::Path::new(cx.path).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let f = Footage {
        path: cx.path.to_string(),
        kind: FootageKind::Still,
        width: cx.psd.width,
        height: cx.psd.height,
        pixel_aspect: 1.0,
        frame_rate: cx.rate,
        has_video: true,
        alpha: if cx.psd.merged_alpha { AlphaMode::Straight } else { AlphaMode::Ignore },
        loop_count: 1,
        codec: "PSD".into(),
        layer: None,
        ..Default::default()
    };
    let id = proj.add_item(&file, Label::Lavender, Some(cx.folder), ItemKind::Footage(f));
    cx.items.push(id);
    let l = build::layer(proj, &comp, name, LayerSource::Footage { item: id }, (cx.psd.width, cx.psd.height), None);
    comp.layers.push(l);
    if let Some(item) = proj.item_mut(cid) {
        item.kind = ItemKind::Comp(Arc::new(comp));
    }
    cid
}

fn new_comp(cx: &Ctx) -> Comp {
    // A PSB can be 300,000 px a side; a composition is at most 30,000 (#788).
    let size = |v: u32| effectcraft_project::limits::size(v.into());
    Comp::new(size(cx.psd.width), size(cx.psd.height), cx.rate, cx.duration)
}

fn build_comp(proj: &mut Project, cx: &mut Ctx, name: &str, nodes: &[Node], parent: Option<ItemId>) -> ItemId {
    let cid = proj.add_item(name, Label::Sandstone, parent, ItemKind::Comp(Arc::new(new_comp(cx))));
    cx.items.push(cid);
    let mut comp = new_comp(cx);
    for n in nodes {
        if let Some(l) = build_layer(proj, cx, &comp, n) {
            comp.layers.push(l);
        }
    }
    styles::sync_comp(&mut comp);
    if let Some(item) = proj.item_mut(cid) {
        item.kind = ItemKind::Comp(Arc::new(comp));
    }
    cid
}

fn footage_item(proj: &mut Project, cx: &mut Ctx, index: usize, name: &str) -> (ItemId, (u32, u32)) {
    let l = &cx.psd.layers[index];
    let retain = cx.retain;
    let (w, h) = if retain && !l.rect.is_empty() { (l.rect.width(), l.rect.height()) } else { (cx.psd.width, cx.psd.height) };
    let file = std::path::Path::new(cx.path).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let f = Footage {
        path: cx.path.to_string(),
        kind: FootageKind::Still,
        width: w,
        height: h,
        pixel_aspect: 1.0,
        frame_rate: cx.rate,
        native_rate: None,
        duration: Tick::ZERO,
        has_video: true,
        has_audio: false,
        alpha: AlphaMode::Straight,
        premul_color: [0.0; 3],
        loop_count: 1,
        codec: "PSD".into(),
        missing: false,
        sequence: vec![],
        color_profile: None,
        layer: Some(SourceLayer { index: index as u32, name: name.to_string(), layer_size: retain && !l.rect.is_empty(), embedded: None, placed: false }),
        ..Default::default()
    };
    let id = proj.add_item(&format!("{name}/{file}"), Label::Lavender, Some(cx.folder), ItemKind::Footage(f));
    cx.items.push(id);
    (id, (w, h))
}

/// Size of an embedded file's image: the smart object's recorded size, else the embedded
/// document's header (Photoshop, PNG).
fn embedded_size(so: &effectcraft_psd::SmartObject, data: &[u8]) -> Option<(u32, u32)> {
    if let Some([w, h]) = so.size.filter(|s| s[0] >= 1.0 && s[1] >= 1.0) {
        return Some((w.round() as u32, h.round() as u32));
    }
    if effectcraft_psd::is_psd(data) && data.len() >= 22 {
        let h = u32::from_be_bytes([data[14], data[15], data[16], data[17]]);
        let w = u32::from_be_bytes([data[18], data[19], data[20], data[21]]);
        return Some((w, h));
    }
    if data.starts_with(b"\x89PNG") && data.len() >= 24 {
        let w = u32::from_be_bytes([data[16], data[17], data[18], data[19]]);
        let h = u32::from_be_bytes([data[20], data[21], data[22], data[23]]);
        return Some((w, h));
    }
    None
}

/// A smart object layer as footage of its embedded file, placed by the smart object's
/// transform. `None` when the file is not embedded (linked externally) or unreadable.
fn smart_object_layer(proj: &mut Project, cx: &mut Ctx, comp: &Comp, index: usize, name: &str) -> Option<Layer> {
    let pl = &cx.psd.layers[index];
    let so = pl.smart_object.clone()?;
    let data = cx.psd.linked_data(&so.uuid)?;
    let (cw, ch) = embedded_size(&so, data).filter(|s| s.0 > 0 && s.1 > 0)?;
    let file = cx.psd.linked.iter().find(|f| f.uuid == so.uuid).map(|f| f.name.clone()).unwrap_or_default();
    // Perspective quads and warps are baked over the placed bounds, at up to 4× the document
    // resolution when the embedded content has that much detail.
    let bake = so.needs_bake();
    let (w, h, k, bounds) = if bake {
        let bb = so.placed_bounds(cw as f64, ch as f64);
        let area = ((bb[2] - bb[0]) * (bb[3] - bb[1])).max(1.0);
        let k = ((cw as f64 * ch as f64) / area).sqrt().clamp(1.0, 4.0);
        // At most 4× the document across and 2^24 pixels, however far the file moves a corner.
        let (k, (w, h)) = so.bake_scale(cw as f64, ch as f64, k, (cx.psd.width, cx.psd.height));
        (w, h, k, bb)
    } else {
        (cw, ch, 1.0, [0.0; 4])
    };
    let f = Footage {
        path: cx.path.to_string(),
        kind: FootageKind::Still,
        width: w,
        height: h,
        pixel_aspect: 1.0,
        frame_rate: cx.rate,
        has_video: true,
        alpha: AlphaMode::Straight,
        loop_count: 1,
        codec: if effectcraft_psd::is_psd(data) { "PSD".into() } else { "PSD (embedded image)".into() },
        layer: Some(SourceLayer { index: index as u32, name: name.to_string(), layer_size: true, embedded: Some(so.uuid.clone()), placed: bake }),
        ..Default::default()
    };
    let label = if file.is_empty() { name.to_string() } else { file };
    let id = proj.add_item(&format!("{label} (Smart Object)"), Label::Lavender, Some(cx.folder), ItemKind::Footage(f));
    cx.items.push(id);
    let mut l = build::layer(proj, comp, name, LayerSource::Footage { item: id }, (w, h), None);
    if bake {
        // The baked pixels cover the placed bounds: centred there, scaled back to 1×.
        let c = [bounds[0] + w as f64 / k / 2.0, bounds[1] + h as f64 / k / 2.0];
        set_vec2(&mut l, "transform/position", c);
        set_vec2(&mut l, "transform/scale", [100.0 / k, 100.0 / k]);
        return Some(l);
    }
    let (c, sx, sy, rot) = so.placement(w as f64, h as f64);
    set_vec2(&mut l, "transform/position", c);
    set_vec2(&mut l, "transform/scale", [sx * 100.0, sy * 100.0]);
    if rot.abs() > 1e-9 {
        set_scalar(&mut l, "transform/rotation", rot);
    }
    Some(l)
}

fn build_layer(proj: &mut Project, cx: &mut Ctx, comp: &Comp, n: &Node) -> Option<Layer> {
    let (index, children) = match n {
        Node::Layer(i) => (*i, None),
        Node::Group { layer, children } => (*layer, Some(children)),
    };
    let pl = cx.psd.layers[index].clone();
    let name = if pl.name.is_empty() { format!("Layer {}", index + 1) } else { pl.name.clone() };
    let mut layer = if let Some(children) = children {
        let sub = build_comp(proj, cx, &name, children, Some(cx.folder));
        let mut l = build::layer(proj, comp, &name, LayerSource::Comp { item: sub }, (comp.width, comp.height), None);
        // Pass-through groups composite their layers straight into the parent.
        l.switches.collapse = pl.blend_key() == "pass";
        l
    } else if let Some(t) = &pl.text {
        text_layer(proj, comp, &name, t)
    } else if let Some(adj) = &pl.adjustment {
        let mut l = adjustment_layer(proj, comp, &name);
        add_adjustment_effect(proj, &mut l, adj, comp, &mut cx.warnings, &name);
        l
    } else if let Some(l) = pl.smart_object.as_ref().and_then(|_| smart_object_layer(proj, cx, comp, index, &name)) {
        l
    } else if pl.has_pixels() || pl.fill.is_some() {
        let (item, size) = footage_item(proj, cx, index, &name);
        let mut l = build::layer(proj, comp, &name, LayerSource::Footage { item }, size, None);
        if cx.retain && !pl.rect.is_empty() {
            set_vec2(&mut l, "transform/position", [pl.rect.left as f64 + size.0 as f64 / 2.0, pl.rect.top as f64 + size.1 as f64 / 2.0]);
        }
        l
    } else {
        // Empty layers have nothing to show.
        return None;
    };
    layer.name = comp.unique_layer_name(&name);
    layer.blend_mode = blend_mode(pl.blend_key());
    layer.switches.video = !pl.hidden;
    layer.preserve_transparency = pl.clipping;
    set_scalar(&mut layer, "transform/opacity", pl.opacity as f64 / 255.0 * 100.0);
    // Vector mask → a mask in layer space.
    let origin = layer_origin(&pl, cx.retain, &layer);
    if let Some(vm) = pl.vector_mask.as_ref().filter(|v| !v.disabled)
        && let Some(masks) = layer.props.sub_mut("masks")
    {
        let mut ids = Ids(&mut proj.next_id);
        for (k, sp) in vm.subpaths.iter().enumerate() {
            let n = sp.knots.len();
            let mut path = ShapePath { vertices: vec![], in_tangents: vec![], out_tangents: vec![], closed: sp.closed, feather: Vec::new() };
            for kn in &sp.knots {
                let v = [kn[1][0] - origin[0], kn[1][1] - origin[1]];
                path.vertices.push(v);
                path.in_tangents.push([kn[0][0] - kn[1][0], kn[0][1] - kn[1][1]]);
                path.out_tangents.push([kn[2][0] - kn[1][0], kn[2][1] - kn[1][1]]);
            }
            if n == 0 {
                continue;
            }
            let mode = if vm.inverted { MaskMode::Subtract } else { MaskMode::Add };
            masks.children.push(build::mask(&mut ids, &format!("Mask {}", k + 1), path, mode, [255, 255, 0]).into());
        }
    }
    // Layer effects → Layer Styles; Fill Opacity → Advanced Blending.
    let gl = comp.global_light.clone();
    if let Some(fx) = &pl.effects
        && fx.bool("masterFXSwitch") != Some(false)
        && layer.can_have_styles()
    {
        apply_effects(proj, &mut layer, fx, &gl);
    }
    if pl.fill_opacity < 255 && layer.can_have_styles() {
        let mut ids = Ids(&mut proj.next_id);
        if layer.props.sub(styles::GROUP).is_none() {
            let g = styles::layer_styles(&mut ids, &gl);
            let at = layer.props.children.iter().position(|c| c.match_id() == "transform").map(|i| i + 1).unwrap_or(layer.props.children.len());
            layer.props.children.insert(at, g.into());
        }
        if let Some(p) = layer.props.prop_mut("layerStyles/blendingOptions/advancedBlending/fillOpacity") {
            p.value = Value::Scalar(pl.fill_opacity as f64 / 255.0 * 100.0);
        }
    }
    Some(layer)
}

/// Where layer space's origin sits in document pixels.
fn layer_origin(pl: &effectcraft_psd::Layer, retain: bool, layer: &Layer) -> [f64; 2] {
    if let Some(t) = &pl.text {
        return [t.transform[4], t.transform[5]];
    }
    if matches!(layer.source, LayerSource::Footage { .. }) && retain && !pl.rect.is_empty() {
        return [pl.rect.left as f64, pl.rect.top as f64];
    }
    [0.0, 0.0]
}

/// Sets a scalar property; a non-finite value (from corrupt transforms) keeps the default.
fn set_scalar(l: &mut Layer, path: &str, v: f64) {
    if !v.is_finite() {
        return;
    }
    if let Some(p) = l.props.prop_mut(path) {
        p.value = Value::Scalar(v);
    }
}

/// Sets a 2D property; a non-finite component (from corrupt transforms) keeps the default.
fn set_vec2(l: &mut Layer, path: &str, v: [f64; 2]) {
    if !v.iter().all(|c| c.is_finite()) {
        return;
    }
    if let Some(p) = l.props.prop_mut(path) {
        p.value = match p.value {
            Value::Vec3(o) => Value::Vec3([v[0], v[1], o[2]]),
            _ => Value::Vec2(v),
        };
    }
}

/// Split a PostScript font name into family and style (`Inter-Bold` → `Inter`, `Bold`).
fn font_names(ps: &str) -> (String, String) {
    match ps.split_once('-') {
        Some((fam, style)) => (fam.to_string(), style.to_string()),
        None => (ps.to_string(), "Regular".to_string()),
    }
}

fn text_doc(t: &effectcraft_psd::TextInfo) -> TextDoc {
    let mut d = TextDoc::default();
    d.set_text(&t.text);
    let [xx, xy, yx, yy, _, _] = t.transform;
    let scale = (xx * yy - xy * yx).abs().sqrt().max(1e-6);
    // A transform too large to multiply (inf) falls back to 1×; products below stay finite.
    let scale = if scale.is_finite() { scale } else { 1.0 };
    if let Some(s) = t.style.size.map(|s| s * scale).filter(|s| s.is_finite() && *s > 0.0) {
        d.size = s;
    }
    if let Some(f) = &t.style.font {
        let (fam, style) = font_names(f);
        d.font = fam;
        d.style = style;
    }
    if let Some(c) = t.style.color {
        d.fill = [c[0] as f32, c[1] as f32, c[2] as f32, 1.0];
    }
    d.justify = match t.style.justification {
        Some(1) => Justify::Right,
        Some(2) => Justify::Center,
        Some(3) => Justify::JustifyLastLeft,
        Some(4) => Justify::JustifyLastRight,
        Some(5) => Justify::JustifyLastCenter,
        Some(6) => Justify::JustifyAll,
        _ => Justify::Left,
    };
    if let Some(tr) = t.style.tracking.filter(|v| v.is_finite()) {
        d.tracking = tr;
    }
    d.leading = t.style.leading.map(|l| l * scale).filter(|l| l.is_finite());
    d.faux_bold = t.style.faux_bold;
    d.faux_italic = t.style.faux_italic;
    if t.style.box_text
        && let Some([l, top, r, b]) = t.bounds
    {
        let (size, pos) = ([(r - l) * scale, (b - top) * scale], [l * scale, top * scale]);
        if size.iter().chain(&pos).all(|v| v.is_finite()) {
            d.box_size = Some(size);
            d.box_pos = pos;
        }
    }
    d
}

fn text_layer(proj: &mut Project, comp: &Comp, name: &str, t: &effectcraft_psd::TextInfo) -> Layer {
    let mut l = build::layer(proj, comp, name, LayerSource::Text, (0, 0), None);
    if let Some(p) = l.props.prop_mut("text/sourceText") {
        p.value = Value::Text(Box::new(text_doc(t)));
    }
    set_vec2(&mut l, "transform/position", [t.transform[4], t.transform[5]]);
    let [xx, xy, _, _, _, _] = t.transform;
    let rot = xy.atan2(xx).to_degrees();
    if rot.abs() > 1e-6 {
        set_scalar(&mut l, "transform/rotation", rot);
    }
    l
}

fn adjustment_layer(proj: &mut Project, comp: &Comp, name: &str) -> Layer {
    let folder = proj.folder_named("Solids").unwrap_or_else(|| proj.add_item("Solids", Label::Yellow, None, ItemKind::Folder));
    let sid = proj.add_item(
        name,
        Label::Red,
        Some(folder),
        ItemKind::Solid(Solid { color: [1.0, 1.0, 1.0], width: comp.width, height: comp.height, pixel_aspect: 1.0 }),
    );
    let mut l = build::layer(proj, comp, name, LayerSource::Solid { item: sid }, (comp.width, comp.height), None);
    l.switches.adjustment = true;
    l.label = Label::Purple;
    l
}

fn add_effect(proj: &mut Project, l: &mut Layer, effect: &str, comp: &Comp, params: &[(&str, Value)]) {
    let Some(spec) = effectcraft_effects::lookup(effect) else { return };
    let mut ids = Ids(&mut proj.next_id);
    let mut g = effectcraft_effects::instantiate(spec, &mut ids, spec.name, [comp.width as f64, comp.height as f64]);
    for (k, v) in params {
        // Paths reach parameters in twirl-down groups (`master/exposure`).
        if let Some(p) = g.prop_mut(k) {
            p.value = v.clone();
        }
    }
    if let Some(fx) = l.props.sub_mut("effects") {
        fx.children.push(g.into());
    }
}

fn add_adjustment_effect(proj: &mut Project, l: &mut Layer, adj: &Adjustment, comp: &Comp, warnings: &mut Vec<String>, name: &str) {
    match adj {
        Adjustment::Invert => add_effect(proj, l, "Invert", comp, &[]),
        Adjustment::BrightnessContrast { brightness, contrast, legacy } => add_effect(
            proj,
            l,
            "Brightness & Contrast",
            comp,
            &[("brightness", Value::Scalar(*brightness)), ("contrast", Value::Scalar(*contrast)), ("useLegacy", Value::Bool(*legacy))],
        ),
        Adjustment::HueSaturation { hue, saturation, lightness, colorize } => {
            let mut p = vec![("hue", Value::Scalar(*hue)), ("saturation", Value::Scalar(*saturation)), ("lightness", Value::Scalar(*lightness))];
            if *colorize {
                p = vec![
                    ("colorize", Value::Bool(true)),
                    ("colorizeHue", Value::Scalar(*hue)),
                    ("colorizeSaturation", Value::Scalar(*saturation)),
                    ("colorizeLightness", Value::Scalar(*lightness)),
                ];
            }
            add_effect(proj, l, "Hue/Saturation", comp, &p)
        }
        Adjustment::Levels { in_black, in_white, gamma, out_black, out_white } => add_effect(
            proj,
            l,
            "Levels",
            comp,
            &[
                ("inBlack", Value::Scalar(in_black / 255.0)),
                ("inWhite", Value::Scalar(in_white / 255.0)),
                ("gamma", Value::Scalar(*gamma)),
                ("outBlack", Value::Scalar(out_black / 255.0)),
                ("outWhite", Value::Scalar(out_white / 255.0)),
            ],
        ),
        Adjustment::Exposure { exposure, offset, gamma } => add_effect(
            proj,
            l,
            "Exposure",
            comp,
            &[("master/exposure", Value::Scalar(*exposure)), ("master/offset", Value::Scalar(*offset)), ("master/gamma", Value::Scalar(*gamma))],
        ),
        Adjustment::Other(k) => warnings.push(format!("{name}: adjustment `{k}` is not converted (layer kept as an empty adjustment layer)")),
    }
}

// ---------------------------------------------------------------- layer effects → styles

fn style_mode(m: BlendMode) -> Value {
    Value::Enum(STYLE_BLEND_MODES.iter().position(|x| x.1 == m).unwrap_or(0) as u32)
}

fn set(g: &mut PropGroup, k: &str, v: Value) {
    if let Some(p) = g.get_mut(k) {
        p.value = v;
    }
}

fn color4(c: [f64; 3]) -> Value {
    Value::Color([c[0], c[1], c[2], 1.0])
}

/// Copy the common style settings (`Md  `, `Clr `, `Opct`, angle, distance, size…).
fn common(g: &mut PropGroup, d: &Descriptor) {
    if let Some(m) = desc_blend(d.enum_value("Md  ")) {
        set(g, "blendMode", style_mode(m));
    }
    if let Some(c) = d.color("Clr ") {
        set(g, "color", color4(c));
    }
    for (k, prop) in [("Opct", "opacity"), ("Nose", "noise"), ("Dstn", "distance"), ("blur", "size"), ("lagl", "angle"), ("ShdN", "jitter"), ("Inpr", "range")]
    {
        if let Some(v) = d.num(k) {
            set(g, prop, Value::Scalar(v));
        }
    }
    if let Some(b) = d.bool("uglg") {
        set(g, "useGlobalLight", Value::Bool(b));
    }
}

fn gradient_of(d: &Descriptor) -> Option<Gradient> {
    let g = d.obj("Grad")?;
    let list = |k: &str| match g.get(k) {
        Some(DValue::List(v)) => v.clone(),
        _ => vec![],
    };
    let colors: Vec<(f64, [f32; 4])> = list("Clrs")
        .iter()
        .filter_map(|c| match c {
            DValue::Descriptor(s) => {
                let col = s.color("Clr ")?;
                Some((s.num("Lctn").unwrap_or(0.0) / 4096.0, [col[0] as f32, col[1] as f32, col[2] as f32, 1.0]))
            }
            _ => None,
        })
        .collect();
    let opacities: Vec<(f64, f32)> = list("Trns")
        .iter()
        .filter_map(|c| match c {
            DValue::Descriptor(s) => Some((s.num("Lctn").unwrap_or(0.0) / 4096.0, (s.num("Opct").unwrap_or(100.0) / 100.0) as f32)),
            _ => None,
        })
        .collect();
    (!colors.is_empty()).then(|| Gradient { colors, opacities: if opacities.is_empty() { vec![(0.0, 1.0), (1.0, 1.0)] } else { opacities } })
}

/// The effect descriptor for `key`, or the first enabled one of its `…Multi` list.
fn effect<'a>(fx: &'a Descriptor, key: &str, multi: &str) -> Option<&'a Descriptor> {
    fx.obj(key).or_else(|| match fx.get(multi) {
        Some(DValue::List(v)) => v.iter().find_map(|x| match x {
            DValue::Descriptor(d) if d.bool("enab") != Some(false) => Some(d),
            _ => None,
        }),
        _ => None,
    })
}

fn apply_effects(proj: &mut Project, layer: &mut Layer, fx: &Descriptor, gl: &styles::GlobalLight) {
    const MAP: &[(&str, &str, &str)] = &[
        ("DrSh", "dropShadowMulti", "dropShadow"),
        ("IrSh", "innerShadowMulti", "innerShadow"),
        ("OrGl", "outerGlowMulti", "outerGlow"),
        ("IrGl", "innerGlowMulti", "innerGlow"),
        ("ebbl", "bevelEmbossMulti", "bevelEmboss"),
        ("ChFX", "satinMulti", "satin"),
        ("SoFi", "solidFillMulti", "colorOverlay"),
        ("GrFl", "gradientFillMulti", "gradientOverlay"),
        ("FrFX", "frameFXMulti", "stroke"),
    ];
    for (key, multi, id) in MAP {
        let Some(d) = effect(fx, key, multi) else { continue };
        let enabled = d.bool("enab").unwrap_or(true);
        let mut ids = Ids(&mut proj.next_id);
        let Some(uid) = styles::add_style(layer, &mut ids, id, gl, enabled) else { continue };
        let Some(g) = layer.props.find_group_mut(uid) else { continue };
        common(g, d);
        match *id {
            "dropShadow" => {
                if let Some(v) = d.num("Ckmt") {
                    set(g, "spread", Value::Scalar(v));
                }
                if let Some(b) = d.bool("layerConceals") {
                    set(g, "knocksOut", Value::Bool(b));
                }
            }
            "innerShadow" | "innerGlow" => {
                if let Some(v) = d.num("Ckmt") {
                    set(g, "choke", Value::Scalar(v));
                }
                if d.enum_value("glwS") == Some("SrcC") {
                    set(g, "source", Value::Enum(0));
                }
            }
            "outerGlow" => {
                if let Some(v) = d.num("Ckmt") {
                    set(g, "spread", Value::Scalar(v));
                }
            }
            "bevelEmboss" => {
                let style = match d.enum_value("bvlS") {
                    Some("OtrB") => 0,
                    Some("Embs") => 2,
                    Some("PlEb") => 3,
                    Some("strokeEmboss") => 4,
                    _ => 1,
                };
                set(g, "style", Value::Enum(style));
                let tech = match d.enum_value("bvlT") {
                    Some("PrBL") => 1,
                    Some("Slmt") => 2,
                    _ => 0,
                };
                set(g, "technique", Value::Enum(tech));
                if let Some(v) = d.num("srgR") {
                    set(g, "depth", Value::Scalar(v));
                }
                set(g, "direction", Value::Enum(u32::from(d.enum_value("bvlD") == Some("Out "))));
                if let Some(v) = d.num("Sftn") {
                    set(g, "soften", Value::Scalar(v));
                }
                if let Some(v) = d.num("Lald") {
                    set(g, "altitude", Value::Scalar(v));
                }
                for (k, p) in [("hglM", "highlightMode"), ("sdwM", "shadowMode")] {
                    if let Some(m) = desc_blend(d.enum_value(k)) {
                        set(g, p, style_mode(m));
                    }
                }
                for (k, p) in [("hglC", "highlightColor"), ("sdwC", "shadowColor")] {
                    if let Some(c) = d.color(k) {
                        set(g, p, color4(c));
                    }
                }
                for (k, p) in [("hglO", "highlightOpacity"), ("sdwO", "shadowOpacity")] {
                    if let Some(v) = d.num(k) {
                        set(g, p, Value::Scalar(v));
                    }
                }
            }
            "satin" => {
                if let Some(b) = d.bool("Invr") {
                    set(g, "invert", Value::Bool(b));
                }
            }
            "gradientOverlay" => {
                if let Some(gr) = gradient_of(d) {
                    set(g, "colors", Value::Gradient(gr));
                }
                if let Some(a) = d.num("Angl") {
                    set(g, "angle", Value::Scalar(a));
                }
                let st = match d.enum_value("Type") {
                    Some("Rdl ") => 1,
                    Some("Angl") => 2,
                    Some("Rflc") => 3,
                    Some("Dmnd") => 4,
                    _ => 0,
                };
                set(g, "style", Value::Enum(st));
                if let Some(b) = d.bool("Rvrs") {
                    set(g, "reverse", Value::Bool(b));
                }
                if let Some(b) = d.bool("Algn") {
                    set(g, "alignWithLayer", Value::Bool(b));
                }
                if let Some(v) = d.num("Scl ") {
                    set(g, "scale", Value::Scalar(v));
                }
            }
            "stroke" => {
                if let Some(v) = d.num("Sz  ") {
                    set(g, "size", Value::Scalar(v));
                }
                let pos = match d.enum_value("Styl") {
                    Some("InsF") => 1,
                    Some("CtrF") => 2,
                    _ => 0,
                };
                set(g, "position", Value::Enum(pos));
            }
            _ => {}
        }
    }
}

/// Layer ▸ Create ▸ Convert to Editable Text: a footage layer showing a Photoshop type layer
/// becomes a text layer (`None` when the layer's source has no text data).
pub fn editable_text(proj: &mut Project, comp: &Comp, layer: &Layer, psd: &Psd, index: usize) -> Option<Layer> {
    let t = psd.layers.get(index)?.text.as_ref()?;
    let mut l = text_layer(proj, comp, &layer.name, t);
    l.name = layer.name.clone();
    l.blend_mode = layer.blend_mode;
    l.switches.video = layer.switches.video;
    l.in_point = layer.in_point;
    l.out_point = layer.out_point;
    l.start_time = layer.start_time;
    if let (Some(o), Some(p)) = (layer.props.prop("transform/opacity"), l.props.prop_mut("transform/opacity")) {
        *p = effectcraft_project::Property { uid: p.uid, ..o.clone() };
    }
    Some(l)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blend_keys() {
        assert_eq!(blend_mode("mul "), BlendMode::Multiply);
        assert_eq!(blend_mode("smud"), BlendMode::Exclusion);
        assert_eq!(blend_mode("pass"), BlendMode::Normal);
        assert_eq!(desc_blend(Some("linearDodge")), Some(BlendMode::LinearDodge));
        assert_eq!(font_names("Inter-Bold"), ("Inter".into(), "Bold".into()));
    }

    #[test]
    fn non_finite_text_values_never_reach_the_layer() {
        let style = effectcraft_psd::engine_data::TextStyle {
            size: Some(f64::NAN),
            tracking: Some(f64::INFINITY),
            leading: Some(f64::NEG_INFINITY),
            ..Default::default()
        };
        let t = effectcraft_psd::TextInfo {
            text: "x".into(),
            transform: [f64::INFINITY, 0.0, 0.0, 1.0, f64::NAN, 1.0],
            style,
            bounds: Some([0.0, 0.0, f64::INFINITY, 1.0]),
        };
        let d = text_doc(&t);
        assert!(d.size.is_finite() && d.tracking.is_finite());
        assert!(d.leading.is_none_or(f64::is_finite));
        assert!(d.box_size.is_none_or(|b| b.iter().all(|v| v.is_finite())));
        let mut proj = Project::default();
        let comp = Comp::new(64, 64, FrameRate::FPS_24, Tick::ZERO);
        let l = text_layer(&mut proj, &comp, "t", &t);
        match l.props.prop("transform/position").map(|p| &p.value) {
            Some(Value::Vec2(v)) => assert!(v.iter().all(|c| c.is_finite())),
            Some(Value::Vec3(v)) => assert!(v.iter().all(|c| c.is_finite())),
            other => panic!("unexpected position {other:?}"),
        }
    }
}
