//! PhotoCraft's layer outline as a composition of lazily decoded, canvas-sized pixel layers.

use std::sync::Arc;

use craft_doc::{Layer as InputLayer, Outline};
use effectcraft_color::{BlendMode, Label};
use effectcraft_keyframe::Value;
use effectcraft_project::{AlphaMode, Comp, Footage, FootageKind, ItemId, ItemKind, LayerSource, Project, SourceLayer, build};
use effectcraft_time::{FrameRate, Tick};

pub(crate) struct Document {
    outline: Outline,
    width: u32,
    height: u32,
}

/// Metadata only: source pixels remain media and are decoded when a frame needs them, as PSD
/// footage is. Bound the number of project items before creating their property groups.
pub(crate) fn read(bytes: &[u8]) -> Result<Document, String> {
    if bytes.len() as u64 > 512 << 20 {
        return Err("PhotoCraft bundle exceeds the 512 MiB input limit".into());
    }
    fn count(layers: &[InputLayer], depth: usize, nodes: &mut usize, pixels: &mut usize) -> Result<(), String> {
        if depth > 100 {
            return Err("PhotoCraft groups exceed 100 levels".into());
        }
        for layer in layers {
            *nodes += 1;
            if *nodes > 4096 {
                return Err("PhotoCraft composition import exceeds 4096 layers; import as Footage for the saved composite".into());
            }
            if matches!(layer.kind.as_str(), "raster" | "text" | "shape" | "smart") {
                *pixels += 1;
            }
            if !layer.children.is_empty() {
                if layer.kind != "group" {
                    return Err(format!("PhotoCraft {} layer has unsupported nested content", layer.kind));
                }
                count(&layer.children, depth + 1, nodes, pixels)?;
            }
        }
        Ok(())
    }
    let outline = craft_doc::outline(bytes, &craft_doc::Limits::default()).map_err(|e| e.to_string())?;
    let canvas = outline.canvas.as_ref().ok_or("not a PhotoCraft canvas")?;
    let (width, height) = (canvas.width, canvas.height);
    if width == 0 || height == 0 || width > effectcraft_project::limits::MAX_SIZE || height > effectcraft_project::limits::MAX_SIZE {
        return Err(format!("PhotoCraft canvas {width} × {height} is outside the composition size limits"));
    }
    if u64::from(width) * u64::from(height) > 16 << 20 {
        return Err("PhotoCraft layer decoding exceeds 16777216 canvas pixels; import as Footage for the saved composite".into());
    }
    if matches!(canvas.mode.as_str(), "Cmyk" | "Lab") {
        return Err(format!("PhotoCraft {} layer pixels are not supported; import as Footage for the saved composite", canvas.mode));
    }
    let (mut nodes, mut pixels) = (0, 0);
    count(&outline.layers, 0, &mut nodes, &mut pixels)?;
    if nodes != 0 && pixels == 0 {
        return Err("PhotoCraft document has no supported saved pixel layers; import as Footage for the saved composite".into());
    }
    Ok(Document { outline, width, height })
}

pub(crate) struct Imported {
    pub comp: ItemId,
    pub folder: ItemId,
    pub items: Vec<ItemId>,
    pub warnings: Vec<String>,
}

struct Context<'a> {
    doc: &'a Document,
    path: &'a str,
    folder: ItemId,
    rate: FrameRate,
    duration: Tick,
    next: u32,
    items: Vec<ItemId>,
    warnings: Vec<String>,
}

pub(crate) fn import(project: &mut Project, doc: &Document, path: &str, name: &str, retain: bool, rate: FrameRate, duration: Tick) -> Imported {
    let folder = project.add_item(&format!("{name} Layers"), Label::Yellow, None, ItemKind::Folder);
    let mut ctx = Context {
        doc,
        path,
        folder,
        rate,
        duration,
        next: 0,
        items: Vec::new(),
        warnings: vec![
            "Layered PhotoCraft import uses saved pixels. Some masks, effects and profiles are unsupported; use Footage for the saved appearance.".into(),
        ],
    };
    if retain {
        ctx.warnings.push("PhotoCraft layer footage uses the document canvas; Retain Layer Sizes keeps those canvas bounds.".into());
    }
    let comp = build_comp(project, &mut ctx, name, &doc.outline.layers, None, 0);
    Imported { comp, folder, items: ctx.items, warnings: ctx.warnings }
}

fn build_comp(project: &mut Project, ctx: &mut Context, name: &str, layers: &[InputLayer], parent: Option<ItemId>, depth: usize) -> ItemId {
    let mut comp = Comp::new(ctx.doc.width, ctx.doc.height, ctx.rate, ctx.duration);
    let id = project.add_item(name, Label::Sandstone, parent, ItemKind::Comp(Arc::new(comp.clone())));
    ctx.items.push(id);
    if depth <= 100 {
        for input in layers {
            let index = ctx.next;
            ctx.next = ctx.next.saturating_add(1);
            let name = if input.name.is_empty() { format!("Layer {}", index + 1) } else { input.name.clone() };
            let source = if input.kind == "group" {
                let folder = ctx.folder;
                let sub = build_comp(project, ctx, &name, &input.children, Some(folder), depth + 1);
                LayerSource::Comp { item: sub }
            } else if matches!(input.kind.as_str(), "raster" | "text" | "shape" | "smart") {
                let file = std::path::Path::new(ctx.path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                let footage = Footage {
                    path: ctx.path.into(),
                    kind: FootageKind::Still,
                    width: ctx.doc.width,
                    height: ctx.doc.height,
                    has_video: true,
                    alpha: AlphaMode::Straight,
                    codec: "PhotoCraft".into(),
                    layer: Some(SourceLayer { index, name: name.clone(), layer_size: false, embedded: None, placed: false }),
                    ..Default::default()
                };
                let item = project.add_item(&format!("{name}/{file}"), Label::Lavender, Some(ctx.folder), ItemKind::Footage(footage));
                ctx.items.push(item);
                LayerSource::Footage { item }
            } else {
                ctx.warnings.push(format!("{name}: PhotoCraft {} content has no supported saved pixels and was skipped", input.kind));
                continue;
            };
            let mut layer = build::layer(project, &comp, &name, source, (ctx.doc.width, ctx.doc.height), None);
            layer.switches.video = input.visible;
            if let Some(opacity) = layer.props.prop_mut("transform/opacity") {
                opacity.value = Value::Scalar(f64::from(input.opacity.filter(|v| v.is_finite()).unwrap_or(1.0).clamp(0.0, 1.0)) * 100.0);
            }
            if let Some(blend) = input.blend.as_deref() {
                if input.kind == "group" && (blend.eq_ignore_ascii_case("PassThrough") || blend.eq_ignore_ascii_case("Pass Through")) {
                    layer.switches.collapse = true;
                } else if let Some(mode) = BlendMode::from_name(blend) {
                    layer.blend_mode = mode;
                } else {
                    ctx.warnings.push(format!("{name}: PhotoCraft blend mode {blend} is not supported; using Normal"));
                }
            }
            comp.layers.push(layer);
        }
    }
    // PhotoCraft stores bottom first; EffectCraft stores the top layer first. Reverse
    // after visiting the source so preorder pixel indexes still address the original tree.
    comp.layers.reverse();
    if let Some(item) = project.item_mut(id) {
        item.kind = ItemKind::Comp(Arc::new(comp));
    }
    id
}
